//! **One game, two doors** (ported from Twang Thang's `online/host_link.rs`).
//!
//! A hosted room is a LAN/WAN host (room code + Nearby) and, when Bluetooth
//! is on, a second host fed by the BLE radio (`fray_net::host_virtual`). Both
//! report into the same [`crate::HostAuthority`], so there is one lobby, one
//! seed and one match. [`HostLink`] merges their peer numbering (Bluetooth
//! peers carry [`BT_BASE`]) and decides each peer's path from *which door it
//! came through*: a joiner's own claim of a path is never believed (fray's
//! UDP host reports the path from the joiner's hello, so a LAN peer claiming
//! "virtual" is read as LAN here; only the Bluetooth door's peers are
//! Bluetooth, and only those need the host's Accept).
//!
//! Persona checks are done by each door's transport ([`crate::net::QuicHost`]
//! verifies every joiner's attestation before `Joined`); this type passes the
//! verified persona through unchanged.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::transport::{HostNetEvent, HostTransport, LinkPath, PeerKey, LOCAL_PEER_BASE};

/// LAN/WAN peers are `1..BT_BASE`, Bluetooth peers `BT_BASE..BT_END`, and the
/// local host seat `LOCAL_PEER_BASE` up. A key is built from the door and the
/// host instance that issued it; an id outside its door's range is refused,
/// not wrapped.
pub const BT_BASE: PeerKey = 1 << 30;
/// One past the last Bluetooth key (the local seat's base).
pub const BT_END: PeerKey = LOCAL_PEER_BASE;
const GEN_SHIFT: u32 = 20;
/// Ids one Bluetooth host instance may issue.
const RAW_END: PeerKey = 1 << GEN_SHIFT;
/// Bluetooth host instances (door openings) one room may make.
const GEN_END: u32 = 1 << (30 - GEN_SHIFT);
/// How long after advertising starts the radio's asynchronous failure is read.
const ADVERT_CHECK_AFTER: Duration = Duration::from_secs(1);

/// The radio behind a Bluetooth door (the app's `platform::ble`).
pub trait Radio: Send {
    fn stop_advertising(&self);
    /// An advertising failure the radio reported since it started.
    fn advertising_error(&self) -> Option<String>;
    /// The room as joiners see it ("1A2B").
    fn room_label(&self) -> String;
}

/// Where the Bluetooth door stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BtStatus {
    Off,
    /// Advertising; the room as joiners see it.
    On { room: String },
    /// Advertising failed; the reason, in the player's words.
    Failed(String),
}

struct BtRoom {
    gen: u32,
    host: Box<dyn HostTransport>,
    radio: Box<dyn Radio>,
    started: Instant,
    checked: bool,
}

impl BtRoom {
    fn shut(mut self, stop_radio: bool) {
        if stop_radio {
            self.radio.stop_advertising();
        }
        self.host.shutdown();
    }
}

struct Inner<L> {
    lan: L,
    bt: Option<BtRoom>,
    /// Doors closed while players were still seated through them.
    draining: Vec<BtRoom>,
    next_gen: u32,
    bt_error: Option<String>,
    bt_peers: HashSet<PeerKey>,
    queue: VecDeque<HostNetEvent>,
}

/// The room's transport: the LAN/WAN door `L` plus an optional Bluetooth
/// door. Clone it to keep a handle for status reads while the authority owns
/// another clone as its `HostTransport`.
pub struct HostLink<L: HostTransport>(Arc<Mutex<Inner<L>>>);

impl<L: HostTransport> Clone for HostLink<L> {
    fn clone(&self) -> Self {
        HostLink(Arc::clone(&self.0))
    }
}

impl<L: HostTransport> HostLink<L> {
    pub fn new(lan: L) -> HostLink<L> {
        HostLink(Arc::new(Mutex::new(Inner {
            lan,
            bt: None,
            draining: Vec::new(),
            next_gen: 0,
            bt_error: None,
            bt_peers: HashSet::new(),
            queue: VecDeque::new(),
        })))
    }

    fn lock(&self) -> MutexGuard<'_, Inner<L>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Read or drive the LAN/WAN door (room code, relays, LAN token).
    pub fn with_lan<R>(&self, f: impl FnOnce(&mut L) -> R) -> R {
        f(&mut self.lock().lan)
    }

    /// Open the Bluetooth door: `host` takes the radio's links (a
    /// `host_virtual` QUIC host), `radio` is what advertises. Closes an
    /// earlier door first (its seated players keep playing).
    pub fn start_bluetooth(&self, host: Box<dyn HostTransport>, radio: Box<dyn Radio>) -> Result<(), String> {
        let mut g = self.lock();
        if g.next_gen >= GEN_END {
            return Err("Bluetooth has been switched on too many times in this game.".into());
        }
        g.close_bt_door(false);
        g.bt_error = None;
        let gen = g.next_gen;
        g.next_gen += 1;
        g.bt = Some(BtRoom { gen, host, radio, started: Instant::now(), checked: false });
        Ok(())
    }

    /// Close the Bluetooth door: advertising stops and no new link is taken.
    /// Players already seated through it keep playing.
    pub fn stop_bluetooth(&self) {
        let mut g = self.lock();
        g.bt_error = None;
        g.close_bt_door(true);
    }

    pub fn bluetooth(&self) -> BtStatus {
        let g = self.lock();
        match (&g.bt, &g.bt_error) {
            (Some(b), _) => BtStatus::On { room: b.radio.room_label() },
            (None, Some(e)) => BtStatus::Failed(e.clone()),
            (None, None) => BtStatus::Off,
        }
    }

    /// Peers connected through a Bluetooth door.
    pub fn bluetooth_peers(&self) -> usize {
        self.lock().bt_peers.len()
    }
}

impl<L: HostTransport> Inner<L> {
    fn close_bt_door(&mut self, stop_radio: bool) {
        let Some(mut old) = self.bt.take() else { return };
        if stop_radio {
            old.radio.stop_advertising();
        }
        if self.bt_peers.iter().any(|&k| bt_split(k).0 == old.gen) {
            old.host.set_open(false);
            self.draining.push(old);
        } else {
            old.shut(false);
        }
    }

    fn retire_bt(&mut self) {
        if let Some(old) = self.bt.take() {
            old.shut(true);
        }
        for old in self.draining.drain(..) {
            old.shut(false);
        }
        for peer in self.bt_peers.drain() {
            self.queue.push_back(HostNetEvent::Left { peer });
        }
    }

    fn reap_draining(&mut self) {
        let peers = &self.bt_peers;
        let (keep, done): (Vec<BtRoom>, Vec<BtRoom>) =
            std::mem::take(&mut self.draining).into_iter().partition(|r| peers.iter().any(|&k| bt_split(k).0 == r.gen));
        self.draining = keep;
        for r in done {
            r.shut(false);
        }
    }

    fn bt_room_mut(&mut self, key: PeerKey) -> Option<(&mut BtRoom, PeerKey)> {
        if !is_bt(key) {
            return None;
        }
        let (gen, raw) = bt_split(key);
        self.bt.iter_mut().chain(self.draining.iter_mut()).find(|r| r.gen == gen).map(|r| (r, raw))
    }

    fn check_advertising(&mut self) {
        let Some(b) = self.bt.as_mut() else { return };
        if b.checked || b.started.elapsed() < ADVERT_CHECK_AFTER {
            return;
        }
        b.checked = true;
        if let Some(why) = b.radio.advertising_error() {
            log::warn!("[bt] advertising failed: {why}");
            self.bt_error = Some(why);
            self.close_bt_door(true);
        }
    }
}

/// An event from the LAN/WAN door, or `None` for a peer number outside the
/// LAN range. A claim of "Bluetooth" through this door is a lie: it is LAN.
fn from_udp_door(ev: HostNetEvent) -> Option<HostNetEvent> {
    if peer_of(&ev) >= BT_BASE {
        return None;
    }
    Some(match ev {
        HostNetEvent::Joined { peer, path: LinkPath::Bluetooth | LinkPath::Local, persona } => {
            HostNetEvent::Joined { peer, path: LinkPath::Lan, persona }
        }
        other => other,
    })
}

/// An event from a Bluetooth door (opening `gen`): the peer gets its
/// Bluetooth key and is on a Bluetooth link whatever its hello said.
fn from_bluetooth_door(gen: u32, ev: HostNetEvent) -> Option<HostNetEvent> {
    let key = bt_key(gen, peer_of(&ev))?;
    Some(match ev {
        HostNetEvent::Joined { persona, .. } => HostNetEvent::Joined { peer: key, path: LinkPath::Bluetooth, persona },
        HostNetEvent::Left { .. } => HostNetEvent::Left { peer: key },
        HostNetEvent::Reliable { bytes, .. } => HostNetEvent::Reliable { peer: key, bytes },
        HostNetEvent::Lossy { bytes, .. } => HostNetEvent::Lossy { peer: key, bytes },
    })
}

fn peer_of(ev: &HostNetEvent) -> PeerKey {
    match ev {
        HostNetEvent::Joined { peer, .. }
        | HostNetEvent::Left { peer }
        | HostNetEvent::Reliable { peer, .. }
        | HostNetEvent::Lossy { peer, .. } => *peer,
    }
}

fn bt_key(gen: u32, raw: PeerKey) -> Option<PeerKey> {
    (gen < GEN_END && raw < RAW_END).then_some(BT_BASE + (gen << GEN_SHIFT) + raw)
}

fn bt_split(key: PeerKey) -> (u32, PeerKey) {
    let k = key - BT_BASE;
    (k >> GEN_SHIFT, k & (RAW_END - 1))
}

fn is_bt(peer: PeerKey) -> bool {
    (BT_BASE..BT_END).contains(&peer)
}

impl<L: HostTransport> HostTransport for HostLink<L> {
    fn poll(&mut self) -> Option<HostNetEvent> {
        let mut g = self.lock();
        g.check_advertising();
        g.reap_draining();
        if let Some(ev) = g.queue.pop_front() {
            return Some(ev);
        }
        while let Some(ev) = g.lan.poll() {
            let (peer, joined) = (peer_of(&ev), matches!(ev, HostNetEvent::Joined { .. }));
            match from_udp_door(ev) {
                Some(ev) => return Some(ev),
                None if joined => g.lan.disconnect(peer),
                None => {}
            }
        }
        let Inner { bt, draining, bt_peers, .. } = &mut *g;
        for room in bt.iter_mut().chain(draining.iter_mut()) {
            while let Some(raw) = room.host.poll() {
                let (peer, joined) = (peer_of(&raw), matches!(raw, HostNetEvent::Joined { .. }));
                let Some(ev) = from_bluetooth_door(room.gen, raw) else {
                    if joined {
                        room.host.disconnect(peer);
                    }
                    continue;
                };
                match &ev {
                    HostNetEvent::Joined { peer, .. } => {
                        bt_peers.insert(*peer);
                    }
                    HostNetEvent::Left { peer } => {
                        bt_peers.remove(peer);
                    }
                    _ => {}
                }
                return Some(ev);
            }
        }
        None
    }
    fn send_reliable(&mut self, peer: PeerKey, bytes: Vec<u8>) {
        let mut g = self.lock();
        if peer < BT_BASE {
            g.lan.send_reliable(peer, bytes);
        } else if let Some((b, raw)) = g.bt_room_mut(peer) {
            b.host.send_reliable(raw, bytes);
        }
    }
    fn send_lossy(&mut self, peer: PeerKey, bytes: Vec<u8>) {
        let mut g = self.lock();
        if peer < BT_BASE {
            g.lan.send_lossy(peer, bytes);
        } else if let Some((b, raw)) = g.bt_room_mut(peer) {
            b.host.send_lossy(raw, bytes);
        }
    }
    fn kick(&mut self, peer: PeerKey) {
        let mut g = self.lock();
        if peer < BT_BASE {
            g.lan.kick(peer);
        } else if let Some((b, raw)) = g.bt_room_mut(peer) {
            b.host.kick(raw);
        }
    }
    fn disconnect(&mut self, peer: PeerKey) {
        let mut g = self.lock();
        if peer < BT_BASE {
            g.lan.disconnect(peer);
        } else if let Some((b, raw)) = g.bt_room_mut(peer) {
            b.host.disconnect(raw);
        }
    }
    fn set_open(&mut self, open: bool) {
        let mut g = self.lock();
        g.lan.set_open(open);
        if let Some(b) = g.bt.as_mut() {
            b.host.set_open(open);
        }
    }
    fn rtt_ms(&self, peer: PeerKey) -> Option<u32> {
        let mut g = self.lock();
        if peer < BT_BASE {
            return g.lan.rtt_ms(peer);
        }
        let (b, raw) = g.bt_room_mut(peer)?;
        b.host.rtt_ms(raw)
    }
    fn shutdown(&mut self) {
        let mut g = self.lock();
        g.retire_bt();
        g.queue.clear();
        g.lan.shutdown()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(peer: PeerKey, path: LinkPath) -> HostNetEvent {
        HostNetEvent::Joined { peer, path, persona: None }
    }

    #[test]
    fn a_peer_is_bluetooth_only_through_the_bluetooth_door() {
        assert_eq!(from_udp_door(joined(3, LinkPath::Bluetooth)), Some(joined(3, LinkPath::Lan)));
        assert_eq!(from_udp_door(joined(3, LinkPath::Local)), Some(joined(3, LinkPath::Lan)));
        assert_eq!(from_udp_door(joined(3, LinkPath::Direct)), Some(joined(3, LinkPath::Direct)));
        assert_eq!(from_udp_door(joined(3, LinkPath::Lan)), Some(joined(3, LinkPath::Lan)));
        for claimed in [LinkPath::Lan, LinkPath::Direct, LinkPath::Bluetooth, LinkPath::Local] {
            assert_eq!(from_bluetooth_door(0, joined(3, claimed)), Some(joined(3 + BT_BASE, LinkPath::Bluetooth)));
        }
        assert_eq!(from_bluetooth_door(0, HostNetEvent::Left { peer: 3 }), Some(HostNetEvent::Left { peer: 3 + BT_BASE }));
        assert!(is_bt(3 + BT_BASE) && !is_bt(3));
    }

    #[test]
    fn a_door_that_is_opened_again_never_reuses_a_peer_key() {
        let a = bt_key(0, 1).unwrap();
        let b = bt_key(1, 1).unwrap();
        assert_ne!(a, b);
        assert_eq!(bt_split(a), (0, 1));
        assert_eq!(bt_split(b), (1, 1));
    }

    #[test]
    fn peer_key_ranges_never_overlap() {
        let top = bt_key(GEN_END - 1, RAW_END - 1).unwrap();
        assert!(is_bt(top) && top < BT_END);
        assert_eq!(bt_key(GEN_END, 1), None);
        assert_eq!(bt_key(0, RAW_END), None);
        assert_eq!(from_udp_door(joined(BT_BASE, LinkPath::Lan)), None);
        assert_eq!(from_udp_door(HostNetEvent::Reliable { peer: BT_BASE + 5, bytes: Vec::new() }), None);
        assert_eq!(from_udp_door(HostNetEvent::Left { peer: BT_END }), None);
        assert!(from_udp_door(joined(BT_BASE - 1, LinkPath::Lan)).is_some());
        assert_eq!(from_bluetooth_door(0, joined(RAW_END, LinkPath::Bluetooth)), None);
        assert_eq!(from_bluetooth_door(GEN_END, joined(1, LinkPath::Bluetooth)), None);
    }
}
