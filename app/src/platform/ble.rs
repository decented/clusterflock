//! Bluetooth LE links for off-grid play: the platform layer Cluster Flock's
//! session code uses to open Bluetooth links as Fray virtual links. Ported
//! from Twang Thang; platform only (no lobby UI, no admission logic).
//!
//! A Bluetooth game is a LAN-style game whose QUIC packets travel over a
//! point-to-point BLE L2CAP channel instead of UDP. This layer only moves
//! bytes; identity, admission and the lobby are the session layer's.
//!
//! # API
//!
//! Get the radio once per process: [`init`]`(fake)` at startup (the desktop
//! `--fake-ble` flag selects the in-memory fake, otherwise Android's radio or
//! [`NoBle`]), then [`get`]`()` anywhere for the shared `Arc<dyn Ble>`.
//! [`platform`] builds one without touching the global (tests).
//!
//! * State and permissions: [`Ble::state`] -> [`BleState`] (`Unsupported`
//!   hides all Bluetooth UI; `NeedsPermission` -> [`Ble::request_permission`];
//!   `PermissionDenied` -> [`Ble::open_settings`]; `Off` ->
//!   [`Ble::request_enable`]; `Ready`). Poll it every frame; it is cheap and
//!   tracks changes made in system settings.
//! * Host: [`Ble::start_advertising`]`(`[`BleAdvert::new`]`(room_id, colour))`
//!   -> [`HostLinks`] (a stream of joiner [`LinkChannels`]), [`Ble::stop_advertising`]
//!   (links already handed out stay open), async failures via
//!   [`Ble::advertising_error`] (poll ~1 s after start).
//! * Joiner: [`Ble::start_scan`] / [`Ble::stop_scan`], [`Ble::seen`] ->
//!   `Vec<`[`BleSeen`]`>` (opaque `device_id`, `advert.room_label()` = radio
//!   id, `advert.colour`, `rssi`; strongest first, aged out after [`SEEN_TTL`]),
//!   async failures via [`Ble::scan_error`]; [`Ble::connect`]`(device_id)` ->
//!   [`LinkChannels`]. Connecting stops the scan; restart it after a failure.
//! * A link is [`LinkChannels`] = `decented_net::VirtualLink { peer_label, tx,
//!   rx }` (tokio mpsc of whole `Vec<u8>` datagrams, at most [`MAX_FRAME`]).
//!   Hand a joiner's link to `join_virtual(cfg, id, link)`; hand the host's
//!   [`HostLinks`] to `host_virtual(cfg, id, room, links)`. Dropping `tx`/`rx`
//!   closes the link; a dead link or failed connect shows as `rx` yielding `None`.
//!
//! Every method returns at once; nothing blocks on the radio. Needs Android 10
//! (API 29) for L2CAP CoC; below that, and on desktop without `--fake-ble`,
//! the state is `Unsupported`.
//!
//! Implementations:
//! * `ble_android::AndroidBle` - Android: `BtBridge.java` advertises, scans
//!   and owns the L2CAP CoC sockets (or insecure RFCOMM, the fallback) and
//!   their threads; packets cross JNI through registered native methods.
//! * [`super::ble_fake::FakeBle`] - desktop `--fake-ble`: in-memory links in
//!   one process (several engines, or one window that both hosts and joins).
//! * [`NoBle`] - everywhere else: [`BleState::Unsupported`].
//!
//! Link contract: packets are whole datagrams of at most [`MAX_FRAME`] bytes.
//! Packets that arrive while the receiving channel is full are dropped (QUIC
//! recovers), never blocking a Java thread.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

/// The app's fixed BLE service UUID (advert filter and service-data key; also
/// the RFCOMM service record of the fallback).
pub const SERVICE_UUID: &str = "38fbf737-1b3e-44d9-a432-3f94fc568c57";
/// Current advert layout ([`BleAdvert::encode`]).
pub const ADVERT_VERSION: u8 = 1;
/// Encoded advert length: version u8, PSM u16 BE, room id [u8; 4], colour u8.
pub const ADVERT_LEN: usize = 8;
/// Largest packet a link carries (the u16 length-prefixed frame payload on
/// the L2CAP/RFCOMM byte stream; `Framing.MAX` in Java).
pub const MAX_FRAME: usize = 2048;
/// Depth of each link's packet channels, both directions.
pub const LINK_QUEUE: usize = 256;
/// Depth of the host's queue of new joiner links.
pub const HOST_QUEUE: usize = 16;
/// A scanned advert older than this is no longer listed by [`Ble::seen`].
pub const SEEN_TTL: std::time::Duration = std::time::Duration::from_secs(10);
/// At most this many scanned hosts are remembered (the oldest is evicted):
/// a device rotating random addresses can't grow the list without bound.
pub const SEEN_MAX: usize = 64;
/// PSM value meaning "the host listens on insecure RFCOMM (service record
/// [`SERVICE_UUID`]), not an L2CAP CoC": the joiner then connects with RFCOMM.
/// A diagnostic only (`debug.flock.bt=rfcomm`): RFCOMM pages the classic
/// address, and a joiner only knows the host's (random) LE address, so it
/// most likely fails between unpaired phones.
pub const PSM_RFCOMM: u16 = 0;
/// LE dynamic SPSM range (Bluetooth Core: 0x0080..=0x00FF); Android assigns
/// `listenUsingInsecureL2capChannel` PSMs from it.
pub const LE_PSM_DYNAMIC: std::ops::RangeInclusive<u16> = 0x0080..=0x00FF;
/// Colour indices an advert may carry: the player colours (`flock_core::MAX_PLAYERS`
/// = 6 body/accent sets). The app does not depend on flock-core, so this is
/// pinned by hand; keep them equal if the player count changes.
pub const ADVERT_COLOURS: u8 = 6;

/// What the Bluetooth radio can do for us right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BleState {
    /// No BLE, no L2CAP CoC (Android < 10), or not this platform.
    Unsupported,
    /// Bluetooth is switched off: [`Ble::request_enable`].
    Off,
    /// Not asked yet (or asked, no answer yet): [`Ble::request_permission`].
    NeedsPermission,
    /// The player said no. Show a friendly why and [`Ble::open_settings`].
    PermissionDenied,
    Ready,
}

/// The host's advert (BLE service data under [`SERVICE_UUID`]). Never carries
/// a name: the joiner sees "Bluetooth game · colour · room 1A2B" and learns
/// the verified persona only after connecting, as on LAN.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BleAdvert {
    pub version: u8,
    /// L2CAP PSM the host listens on; [`PSM_RFCOMM`] for the RFCOMM fallback.
    /// The platform fills it in: [`Ble::start_advertising`] ignores the
    /// caller's value.
    pub psm: u16,
    /// Random per lobby.
    pub room_id: [u8; 4],
    /// The host's colour index.
    pub colour: u8,
}

impl BleAdvert {
    pub fn new(room_id: [u8; 4], colour: u8) -> Self {
        BleAdvert { version: ADVERT_VERSION, psm: 0, room_id, colour }
    }

    pub fn encode(&self) -> [u8; ADVERT_LEN] {
        let p = self.psm.to_be_bytes();
        let r = self.room_id;
        [self.version, p[0], p[1], r[0], r[1], r[2], r[3], self.colour]
    }

    /// Every field comes from any radio in range, so this is the gate: `None`
    /// for a short payload, another advert version (a newer build's layout
    /// may differ), a PSM outside [`LE_PSM_DYNAMIC`] (other than
    /// [`PSM_RFCOMM`]) or a colour outside the palette. A decoded advert is
    /// safe to index with. Trailing bytes are ignored.
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < ADVERT_LEN || b[0] != ADVERT_VERSION {
            return None;
        }
        let psm = u16::from_be_bytes([b[1], b[2]]);
        if psm != PSM_RFCOMM && !LE_PSM_DYNAMIC.contains(&psm) {
            return None;
        }
        if b[7] >= ADVERT_COLOURS {
            return None;
        }
        Some(BleAdvert { version: b[0], psm, room_id: [b[3], b[4], b[5], b[6]], colour: b[7] })
    }

    /// The room id as the UI shows it, e.g. "1A2B" (first two bytes, hex).
    pub fn room_label(&self) -> String {
        format!("{:02X}{:02X}", self.room_id[0], self.room_id[1])
    }
}

/// One scanned host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BleSeen {
    /// Opaque, platform-specific (an Android BLE address, often random).
    pub device_id: String,
    pub advert: BleAdvert,
    pub rssi: i32,
    pub last_seen: Instant,
}

/// The scan results a platform keeps: aged out after [`SEEN_TTL`] and capped
/// at [`SEEN_MAX`] (oldest evicted) on every insert, not only when listed.
#[derive(Default)]
pub struct SeenTable {
    map: HashMap<String, BleSeen>,
}

impl SeenTable {
    /// Record a sighting at `s.last_seen`; returns true if the host is new.
    pub fn insert(&mut self, s: BleSeen) -> bool {
        let now = s.last_seen;
        self.map.retain(|_, e| now.saturating_duration_since(e.last_seen) < SEEN_TTL);
        let new = self.map.insert(s.device_id.clone(), s).is_none();
        while self.map.len() > SEEN_MAX {
            let Some(oldest) = self.map.values().min_by_key(|e| e.last_seen).map(|e| e.device_id.clone()) else { break };
            self.map.remove(&oldest);
        }
        new
    }

    /// Hosts seen within [`SEEN_TTL`] of `now`, strongest first.
    pub fn list(&mut self, now: Instant) -> Vec<BleSeen> {
        self.map.retain(|_, e| now.saturating_duration_since(e.last_seen) < SEEN_TTL);
        let mut v: Vec<BleSeen> = self.map.values().cloned().collect();
        sort_seen(&mut v);
        v
    }

    pub fn get(&self, device_id: &str) -> Option<&BleSeen> {
        self.map.get(device_id)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }
}

/// One point-to-point packet link: decented-net's `VirtualLink { peer_label,
/// tx, rx }`, so a link goes straight to `host_virtual` / `join_virtual`.
/// `peer_label` is for logs/UI ("BLE …A1:B2"), not an identity; `tx` takes
/// packets of at most [`MAX_FRAME`] bytes; `rx` yields `None` once the link
/// has closed.
pub type LinkChannels = decented_net::VirtualLink;

/// The host's stream of joiner links, one per accepted connection (it is
/// `host_virtual`'s input). Yields `None` once advertising stops; links
/// already handed out stay open.
pub type HostLinks = decented_net::VirtualLinkReceiver;

// QUIC's largest virtual packet must fit one frame.
const _: () = assert!(decented_net::MAX_VIRTUAL_PACKET <= MAX_FRAME);
/// Bytes the legacy (31-byte) advert uses: flags AD (len, type, value = 3)
/// + one 128-bit service-data AD (len, type 0x21, UUID 16, data).
pub const ADVERT_PAYLOAD_LEN: usize = 3 + 2 + 16 + ADVERT_LEN;
const _: () = assert!(ADVERT_PAYLOAD_LEN <= 31);

/// The platform's Bluetooth LE radio. Every method returns at once: nothing
/// blocks on radio I/O.
pub trait Ble: Send + Sync {
    fn state(&self) -> BleState;
    /// Ask for the Bluetooth permissions (Android runtime prompt).
    fn request_permission(&self);
    /// Ask the player to switch Bluetooth on (system prompt).
    fn request_enable(&self);
    /// Open the app's system settings page (after "don't ask again").
    fn open_settings(&self) {}
    /// Listen for joiners and advertise `ad` (its `psm` is filled in by the
    /// platform). Replaces any previous advertising. Returns at once: the
    /// radio starts in the background, so `Ok` is not confirmation; poll
    /// [`Ble::advertising_error`] (and then call `stop_advertising`).
    fn start_advertising(&self, ad: BleAdvert) -> Result<HostLinks, String>;
    fn stop_advertising(&self);
    /// Why advertising failed after `start_advertising` returned `Ok` (the
    /// radio answers asynchronously), if it did.
    fn advertising_error(&self) -> Option<String> {
        None
    }
    fn start_scan(&self) -> Result<(), String>;
    fn stop_scan(&self);
    /// Why scanning isn't finding anything, if the radio said so after
    /// `start_scan` returned `Ok`: the scan failed or was throttled (Android
    /// allows ~5 scan starts per 30 s), or (Android 10-11) Location is off.
    fn scan_error(&self) -> Option<String> {
        None
    }
    /// Hosts seen in the last [`SEEN_TTL`] (at most [`SEEN_MAX`]), strongest first.
    fn seen(&self) -> Vec<BleSeen>;
    /// Connect to a scanned host. Returns at once: the link opens in the
    /// background, and a failed connect shows as `rx` yielding `None`.
    /// Connecting stops the scan (it slows connection setup): call
    /// `start_scan` again after a failed connect.
    fn connect(&self, device_id: &str) -> Result<LinkChannels, String>;
}

/// No Bluetooth here (plain desktop builds).
pub struct NoBle;

impl Ble for NoBle {
    fn state(&self) -> BleState {
        BleState::Unsupported
    }
    fn request_permission(&self) {}
    fn request_enable(&self) {}
    fn start_advertising(&self, _ad: BleAdvert) -> Result<HostLinks, String> {
        Err(UNSUPPORTED.into())
    }
    fn stop_advertising(&self) {}
    fn start_scan(&self) -> Result<(), String> {
        Err(UNSUPPORTED.into())
    }
    fn stop_scan(&self) {}
    fn seen(&self) -> Vec<BleSeen> {
        Vec::new()
    }
    fn connect(&self, _device_id: &str) -> Result<LinkChannels, String> {
        Err(UNSUPPORTED.into())
    }
}

pub(crate) const UNSUPPORTED: &str = "Bluetooth games aren't available on this device.";

static RADIO: std::sync::OnceLock<Arc<dyn Ble>> = std::sync::OnceLock::new();

/// Choose the process-wide radio (once, at startup; `fake` = `--fake-ble`) and
/// return it. A second call returns the first call's radio.
pub fn init(fake: bool) -> Arc<dyn Ble> {
    RADIO.get_or_init(|| platform(fake)).clone()
}

/// The process-wide radio: whatever [`init`] chose, or [`NoBle`] if it was
/// never called (nothing is initialised implicitly, so tests stay isolated).
pub fn get() -> Arc<dyn Ble> {
    RADIO.get().cloned().unwrap_or_else(|| Arc::new(NoBle))
}

/// The Bluetooth radio for this launch: Android's, the in-memory fake
/// (`--fake-ble`), or none.
pub fn platform(fake: bool) -> Arc<dyn Ble> {
    if fake {
        log::warn!("fake Bluetooth: in-memory links (--fake-ble)");
        return Arc::new(super::ble_fake::FakeBle::new(super::ble_fake::FakeAir::shared()));
    }
    #[cfg(target_os = "android")]
    {
        Arc::new(super::ble_android::AndroidBle::new())
    }
    #[cfg(not(target_os = "android"))]
    {
        Arc::new(NoBle)
    }
}

/// Sort for [`Ble::seen`]: strongest signal first, then a stable order.
pub(crate) fn sort_seen(v: &mut [BleSeen]) {
    v.sort_by(|a, b| b.rssi.cmp(&a.rssi).then_with(|| a.device_id.cmp(&b.device_id)));
}

// ─── Framing (a Rust port of Framing.java, same rules) ──────────────────────
//
// On the L2CAP/RFCOMM byte stream each packet is a u16 big-endian length then
// that many bytes, 0..=MAX_FRAME. Java does the real framing on device
// (`app/android/java/.../Framing.java`, tested by `tools/build-apk.sh`); this
// port pins the format down for the Rust side and its tests.

/// A frame longer than [`MAX_FRAME`]: the stream is corrupt, close the link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameTooLong(pub usize);

/// One packet as a frame.
pub fn encode_frame(data: &[u8]) -> Result<Vec<u8>, FrameTooLong> {
    if data.len() > MAX_FRAME {
        return Err(FrameTooLong(data.len()));
    }
    let mut out = Vec::with_capacity(2 + data.len());
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
    Ok(out)
}

/// Reassembles frames from a byte stream that arrives in arbitrary pieces.
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    /// Feed bytes; returns every frame they complete.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, FrameTooLong> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut at = 0;
        while self.buf.len() - at >= 2 {
            let n = u16::from_be_bytes([self.buf[at], self.buf[at + 1]]) as usize;
            if n > MAX_FRAME {
                return Err(FrameTooLong(n));
            }
            if self.buf.len() - at - 2 < n {
                break;
            }
            out.push(self.buf[at + 2..at + 2 + n].to_vec());
            at += 2 + n;
        }
        self.buf.drain(..at);
        Ok(out)
    }

    /// Bytes of an unfinished frame (non-zero at EOF = truncated stream).
    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_is_unsupported_until_init() {
        // `init` is never called by tests (it is process-global), so `get`
        // must be the unsupported stub.
        assert_eq!(get().state(), BleState::Unsupported);
    }

    #[test]
    fn advert_round_trips_and_is_eight_bytes() {
        let mut a = BleAdvert::new([0x1A, 0x2B, 0x3C, 0x4D], 5);
        a.psm = 0x0085;
        let b = a.encode();
        assert_eq!(b, [1, 0x00, 0x85, 0x1A, 0x2B, 0x3C, 0x4D, 5]);
        assert_eq!(BleAdvert::decode(&b), Some(a));
        assert_eq!(a.room_label(), "1A2B");
        // Big-endian PSM.
        a.psm = 0x1234;
        assert_eq!(&a.encode()[1..3], &[0x12, 0x34]);
    }

    #[test]
    fn advert_decode_rejects_short_and_unknown_versions_and_ignores_trailing() {
        let b = BleAdvert::new([1, 2, 3, 4], 5).encode();
        assert_eq!(BleAdvert::decode(&b[..7]), None);
        assert_eq!(BleAdvert::decode(&[]), None);
        let mut v2 = b;
        v2[0] = 2;
        assert_eq!(BleAdvert::decode(&v2), None);
        let mut longer = b.to_vec();
        longer.extend_from_slice(&[9, 9]);
        assert_eq!(BleAdvert::decode(&longer), BleAdvert::decode(&b));
    }

    #[test]
    fn advert_fits_one_legacy_advert_with_no_scan_response() {
        // Flags (3) + service data AD: len, 0x21, 128-bit UUID, 8 bytes.
        // (<= 31 is a const assert next to ADVERT_PAYLOAD_LEN.)
        assert_eq!(ADVERT_PAYLOAD_LEN, 29);
        assert_eq!(SERVICE_UUID.replace('-', "").len(), 32, "a 128-bit UUID");
    }

    #[test]
    fn hostile_advert_fields_never_decode() {
        let good = {
            let mut a = BleAdvert::new([0xDE, 0xAD, 0xBE, 0xEF], 0);
            a.psm = 0x0080;
            a.encode()
        };
        assert!(BleAdvert::decode(&good).is_some());
        // Colour outside the palette (ADVERT_COLOURS).
        for c in [ADVERT_COLOURS, 9, 0x7F, 0xFF] {
            let mut b = good;
            b[7] = c;
            assert_eq!(BleAdvert::decode(&b), None, "colour {c}");
        }
        // PSM outside the LE dynamic range (0 = RFCOMM is allowed).
        for psm in [0x0001u16, 0x0025, 0x007F, 0x0100, 0x1001, 0xFFFF] {
            let mut b = good;
            b[1..3].copy_from_slice(&psm.to_be_bytes());
            assert_eq!(BleAdvert::decode(&b), None, "psm {psm:#06x}");
        }
        for psm in [PSM_RFCOMM, 0x0080, 0x00C3, 0x00FF] {
            let mut b = good;
            b[1..3].copy_from_slice(&psm.to_be_bytes());
            assert_eq!(BleAdvert::decode(&b).map(|a| a.psm), Some(psm));
        }
        // Exhaustive over the two fields that index or route: whatever
        // decodes is in range.
        for psm in 0..=u16::MAX {
            for c in [0u8, 5, 6, 7, 8, 255] {
                let mut b = good;
                b[1..3].copy_from_slice(&psm.to_be_bytes());
                b[7] = c;
                if let Some(a) = BleAdvert::decode(&b) {
                    assert!(a.colour < ADVERT_COLOURS);
                    assert!(a.psm == PSM_RFCOMM || LE_PSM_DYNAMIC.contains(&a.psm));
                }
            }
        }
    }

    fn sighting(id: &str, at: Instant, rssi: i32) -> BleSeen {
        BleSeen { device_id: id.into(), advert: BleAdvert::new([1, 2, 3, 4], 1), rssi, last_seen: at }
    }

    #[test]
    fn seen_table_is_bounded_and_evicts_the_oldest() {
        let t0 = Instant::now();
        let mut t = SeenTable::default();
        for i in 0..(SEEN_MAX + 40) {
            let at = t0 + std::time::Duration::from_millis(i as u64);
            assert!(t.insert(sighting(&format!("dev-{i}"), at, -60)));
            assert!(t.len() <= SEEN_MAX);
        }
        assert_eq!(t.len(), SEEN_MAX);
        // The 40 oldest went; the newest stayed.
        assert!(t.get("dev-0").is_none() && t.get("dev-39").is_none());
        assert!(t.get("dev-40").is_some() && t.get(&format!("dev-{}", SEEN_MAX + 39)).is_some());
        // A repeat sighting refreshes, it doesn't add.
        let later = t0 + std::time::Duration::from_secs(1);
        assert!(!t.insert(sighting("dev-40", later, -40)));
        assert_eq!(t.len(), SEEN_MAX);
    }

    #[test]
    fn seen_table_ages_out_on_insert_and_list() {
        let t0 = Instant::now();
        let mut t = SeenTable::default();
        t.insert(sighting("old", t0, -80));
        t.insert(sighting("near", t0 + SEEN_TTL / 2, -40));
        let v = t.list(t0 + SEEN_TTL / 2);
        assert_eq!(v.iter().map(|s| s.device_id.as_str()).collect::<Vec<_>>(), ["near", "old"], "strongest first");
        // Past the TTL "old" is gone from the list...
        assert_eq!(t.list(t0 + SEEN_TTL).len(), 1);
        // ...and an insert alone (nobody listing) prunes too.
        let mut t = SeenTable::default();
        t.insert(sighting("a", t0, -50));
        t.insert(sighting("b", t0 + SEEN_TTL + std::time::Duration::from_millis(1), -50));
        assert_eq!(t.len(), 1);
        assert!(t.get("a").is_none());
        t.clear();
        assert!(t.is_empty());
    }

    #[test]
    fn frames_round_trip_including_empty_and_max() {
        let mut r = FrameReader::default();
        for len in [0usize, 1, 1200, 1452, MAX_FRAME] {
            let data: Vec<u8> = (0..len).map(|i| (i * 7) as u8).collect();
            let f = encode_frame(&data).unwrap();
            assert_eq!(f.len(), 2 + len);
            assert_eq!(&f[..2], &(len as u16).to_be_bytes());
            assert_eq!(r.push(&f).unwrap(), vec![data]);
            assert_eq!(r.pending(), 0);
        }
        assert_eq!(encode_frame(&vec![0; MAX_FRAME + 1]), Err(FrameTooLong(MAX_FRAME + 1)));
    }

    #[test]
    fn frames_reassemble_from_one_byte_reads() {
        let packets: Vec<Vec<u8>> = vec![b"hello".to_vec(), vec![], vec![0xAB; MAX_FRAME], b"x".to_vec()];
        let stream: Vec<u8> = packets.iter().flat_map(|p| encode_frame(p).unwrap()).collect();
        // Byte by byte, then in odd-sized chunks: same packets, same order.
        for chunk in [1usize, 3, 1000, stream.len()] {
            let mut r = FrameReader::default();
            let mut got = Vec::new();
            for piece in stream.chunks(chunk) {
                got.extend(r.push(piece).unwrap());
            }
            assert_eq!(got, packets, "chunk size {chunk}");
            assert_eq!(r.pending(), 0);
        }
        // A truncated stream leaves the partial frame pending.
        let mut r = FrameReader::default();
        assert!(r.push(&stream[..4]).unwrap().is_empty());
        assert_eq!(r.pending(), 4);
    }

    #[test]
    fn an_oversized_length_is_a_corrupt_stream() {
        let mut r = FrameReader::default();
        assert_eq!(r.push(&[0x08, 0x01]), Err(FrameTooLong(0x0801)));
        let mut r = FrameReader::default();
        assert_eq!(r.push(&[0xFF]).unwrap(), Vec::<Vec<u8>>::new());
        assert_eq!(r.push(&[0xFF]), Err(FrameTooLong(0xFFFF)));
    }

    #[test]
    fn no_ble_is_unsupported() {
        let b = NoBle;
        assert_eq!(b.state(), BleState::Unsupported);
        assert!(b.start_scan().is_err());
        assert!(b.start_advertising(BleAdvert::new([0; 4], 0)).is_err());
        assert!(b.connect("x").is_err());
        assert!(b.seen().is_empty());
    }
}
