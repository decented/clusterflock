//! In-memory Bluetooth for the desktop (`--fake-ble`): hosts advertise into a
//! shared [`FakeAir`], scanners in the same process see them, and a connect
//! cross-wires two packet channels. No threads: dropping either side's `tx`
//! closes the other side's `rx`, just as a dropped radio link would.
//!
//! The app uses [`FakeAir::shared`] (one air per process: a window can host
//! and a second engine in the same process can join, as `app/src/tests.rs`
//! runs two engines). Tests make their own air so parallel tests never see
//! each other's adverts.

use super::ble::{sort_seen, Ble, BleAdvert, BleSeen, BleState, HostLinks, LinkChannels, HOST_QUEUE, LINK_QUEUE, SEEN_MAX};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Instant;
use tokio::sync::mpsc;

/// The shared "radio space" fake devices advertise into.
#[derive(Default)]
pub struct FakeAir {
    inner: Mutex<Air>,
}

#[derive(Default)]
struct Air {
    next_device: u32,
    next_psm: u16,
    /// Advertising hosts by device id.
    hosts: HashMap<String, Hosting>,
}

struct Hosting {
    advert: BleAdvert,
    joiners: mpsc::Sender<LinkChannels>,
}

impl FakeAir {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The process-wide air used by `--fake-ble`.
    pub fn shared() -> Arc<Self> {
        static AIR: OnceLock<Arc<FakeAir>> = OnceLock::new();
        AIR.get_or_init(FakeAir::new).clone()
    }

    fn lock(&self) -> MutexGuard<'_, Air> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// One fake phone's Bluetooth.
pub struct FakeBle {
    air: Arc<FakeAir>,
    device_id: String,
    state: Mutex<BleState>,
    scanning: AtomicBool,
}

impl FakeBle {
    /// A new fake device in `air`, permissions granted and the radio on.
    pub fn new(air: Arc<FakeAir>) -> Self {
        let device_id = {
            let mut a = air.lock();
            a.next_device += 1;
            format!("fake-{}", a.next_device)
        };
        FakeBle { air, device_id, state: Mutex::new(BleState::Ready), scanning: AtomicBool::new(false) }
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Put the fake in another state (tests: the permission and radio-off flows).
    pub fn set_state(&self, s: BleState) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = s;
        if s != BleState::Ready {
            self.stop_advertising();
            self.scanning.store(false, Ordering::Relaxed);
        }
    }

    fn ready(&self) -> Result<(), String> {
        match self.state() {
            BleState::Ready => Ok(()),
            BleState::Off => Err("Bluetooth is off.".into()),
            BleState::NeedsPermission | BleState::PermissionDenied => Err("Cluster Flock needs the Bluetooth permission.".into()),
            BleState::Unsupported => Err(super::ble::UNSUPPORTED.into()),
        }
    }
}

impl Drop for FakeBle {
    fn drop(&mut self) {
        self.stop_advertising();
    }
}

/// Two cross-wired channel pairs: (host side, joiner side).
fn link_pair(host_label: String, joiner_label: String) -> (LinkChannels, LinkChannels) {
    let (to_joiner, from_host) = mpsc::channel(LINK_QUEUE);
    let (to_host, from_joiner) = mpsc::channel(LINK_QUEUE);
    (
        LinkChannels { peer_label: host_label, tx: to_joiner, rx: from_joiner },
        LinkChannels { peer_label: joiner_label, tx: to_host, rx: from_host },
    )
}

impl Ble for FakeBle {
    fn state(&self) -> BleState {
        *self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn request_permission(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *s == BleState::NeedsPermission {
            *s = BleState::Ready;
        }
    }

    fn request_enable(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if *s == BleState::Off {
            *s = BleState::Ready;
        }
    }

    fn start_advertising(&self, mut ad: BleAdvert) -> Result<HostLinks, String> {
        self.ready()?;
        let (tx, rx) = mpsc::channel(HOST_QUEUE);
        let mut a = self.air.lock();
        a.next_psm = a.next_psm.wrapping_add(1);
        // Dynamic LE PSMs live in 0x0080..=0x00FF, as Android assigns them.
        ad.psm = 0x0080 + (a.next_psm % 0x80);
        log::info!("fake-ble: {} advertising room {} psm {:#06x}", self.device_id, ad.room_label(), ad.psm);
        a.hosts.insert(self.device_id.clone(), Hosting { advert: ad, joiners: tx });
        Ok(rx)
    }

    fn stop_advertising(&self) {
        if self.air.lock().hosts.remove(&self.device_id).is_some() {
            log::info!("fake-ble: {} stopped advertising", self.device_id);
        }
    }

    fn start_scan(&self) -> Result<(), String> {
        self.ready()?;
        self.scanning.store(true, Ordering::Relaxed);
        Ok(())
    }

    fn stop_scan(&self) {
        self.scanning.store(false, Ordering::Relaxed);
    }

    fn seen(&self) -> Vec<BleSeen> {
        if !self.scanning.load(Ordering::Relaxed) {
            return Vec::new();
        }
        let now = Instant::now();
        let a = self.air.lock();
        let mut v: Vec<BleSeen> = a
            .hosts
            .iter()
            .filter(|(id, h)| **id != self.device_id && !h.joiners.is_closed())
            // Through the same gate as a real radio: an advert that doesn't
            // decode (bad colour, PSM, version) is never listed.
            .filter_map(|(id, h)| BleAdvert::decode(&h.advert.encode()).map(|advert| BleSeen { device_id: id.clone(), advert, rssi: -55, last_seen: now }))
            .collect();
        sort_seen(&mut v);
        v.truncate(SEEN_MAX);
        v
    }

    fn connect(&self, device_id: &str) -> Result<LinkChannels, String> {
        self.ready()?;
        let a = self.air.lock();
        let host = a.hosts.get(device_id).ok_or("That Bluetooth game has gone.")?;
        let (host_side, joiner_side) = link_pair(format!("BLE {}", self.device_id), format!("BLE {device_id}"));
        host.joiners.try_send(host_side).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => "That Bluetooth game is busy. Try again.".to_string(),
            mpsc::error::TrySendError::Closed(_) => "That Bluetooth game has gone.".to_string(),
        })?;
        log::info!("fake-ble: {} connected to {device_id}", self.device_id);
        Ok(joiner_side)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::ble::ADVERT_VERSION;

    fn pair() -> (FakeBle, FakeBle) {
        let air = FakeAir::new();
        (FakeBle::new(air.clone()), FakeBle::new(air))
    }

    #[test]
    fn host_advertises_joiner_sees_connects_and_packets_flow_both_ways() {
        let (host, joiner) = pair();
        let mut links = host.start_advertising(BleAdvert::new([0x1A, 0x2B, 3, 4], 2)).unwrap();
        assert!(joiner.seen().is_empty(), "nothing seen before scanning");
        joiner.start_scan().unwrap();
        let seen = joiner.seen();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].device_id, host.device_id());
        assert_eq!(seen[0].advert.version, ADVERT_VERSION);
        assert_eq!(seen[0].advert.room_id, [0x1A, 0x2B, 3, 4]);
        assert_eq!(seen[0].advert.colour, 2);
        assert_ne!(seen[0].advert.psm, 0, "the platform fills in the PSM");
        assert!(host.seen().is_empty(), "the host isn't scanning and never sees itself");

        let mut j = joiner.connect(&seen[0].device_id).unwrap();
        let mut h = links.try_recv().expect("the host gets the joiner's link");
        assert_eq!(h.peer_label, format!("BLE {}", joiner.device_id()));
        assert_eq!(j.peer_label, format!("BLE {}", host.device_id()));

        j.tx.try_send(b"hello host".to_vec()).unwrap();
        h.tx.try_send(b"hello joiner".to_vec()).unwrap();
        h.tx.try_send(vec![7; 1452]).unwrap();
        assert_eq!(h.rx.try_recv().unwrap(), b"hello host");
        assert_eq!(j.rx.try_recv().unwrap(), b"hello joiner");
        assert_eq!(j.rx.try_recv().unwrap(), vec![7; 1452]);
    }

    #[test]
    fn a_closed_link_closes_and_stopping_ends_the_host_stream() {
        let (host, joiner) = pair();
        let mut links = host.start_advertising(BleAdvert::new([9; 4], 0)).unwrap();
        joiner.start_scan().unwrap();
        let id = joiner.seen()[0].device_id.clone();

        // Joiner hangs up: the host's rx ends (after draining what was sent).
        let j = joiner.connect(&id).unwrap();
        let mut h = links.try_recv().unwrap();
        j.tx.try_send(b"bye".to_vec()).unwrap();
        drop(j);
        assert_eq!(h.rx.blocking_recv().as_deref(), Some(&b"bye"[..]));
        assert_eq!(h.rx.blocking_recv(), None);
        assert!(h.tx.try_send(vec![1]).is_err(), "sending on a closed link fails");

        // Host kicks: the joiner's rx ends.
        let mut j2 = joiner.connect(&id).unwrap();
        drop(links.try_recv().unwrap());
        assert_eq!(j2.rx.blocking_recv(), None);

        // Stop advertising: no longer listed, connects fail, the stream ends.
        let j3 = joiner.connect(&id).unwrap();
        let mut h3 = links.try_recv().unwrap();
        host.stop_advertising();
        assert!(joiner.seen().is_empty());
        assert!(joiner.connect(&id).is_err());
        assert!(links.blocking_recv().is_none());
        // ...but a link already open stays open.
        j3.tx.try_send(b"still here".to_vec()).unwrap();
        assert_eq!(h3.rx.try_recv().unwrap(), b"still here");
    }

    #[test]
    fn a_full_channel_rejects_instead_of_blocking() {
        let (host, joiner) = pair();
        let mut links = host.start_advertising(BleAdvert::new([1; 4], 1)).unwrap();
        let id = host.device_id().to_string();
        let j = joiner.connect(&id).unwrap();
        let _h = links.try_recv().unwrap();
        for _ in 0..LINK_QUEUE {
            j.tx.try_send(vec![0]).unwrap();
        }
        assert!(matches!(j.tx.try_send(vec![0]), Err(mpsc::error::TrySendError::Full(_))));
    }

    #[test]
    fn dropping_the_host_device_or_its_link_stream_hides_the_game() {
        let air = FakeAir::new();
        let joiner = FakeBle::new(air.clone());
        joiner.start_scan().unwrap();
        {
            let host = FakeBle::new(air.clone());
            let links = host.start_advertising(BleAdvert::new([2; 4], 3)).unwrap();
            assert_eq!(joiner.seen().len(), 1);
            drop(links);
            assert!(joiner.seen().is_empty(), "nobody is accepting");
            let _again = host.start_advertising(BleAdvert::new([2; 4], 3)).unwrap();
            assert_eq!(joiner.seen().len(), 1);
        }
        assert!(joiner.seen().is_empty(), "a dropped device stops advertising");
    }

    #[test]
    fn a_host_advertising_an_out_of_palette_colour_is_never_listed() {
        let (host, joiner) = pair();
        let _links = host.start_advertising(BleAdvert::new([5; 4], 200)).unwrap();
        joiner.start_scan().unwrap();
        assert!(joiner.seen().is_empty());
    }

    #[test]
    fn permission_and_radio_off_flows() {
        let (host, joiner) = pair();
        joiner.set_state(BleState::NeedsPermission);
        assert!(joiner.start_scan().is_err());
        joiner.request_enable();
        assert_eq!(joiner.state(), BleState::NeedsPermission);
        joiner.request_permission();
        assert_eq!(joiner.state(), BleState::Ready);

        host.set_state(BleState::Off);
        assert!(host.start_advertising(BleAdvert::new([0; 4], 0)).is_err());
        host.request_enable();
        assert_eq!(host.state(), BleState::Ready);
        let _links = host.start_advertising(BleAdvert::new([0; 4], 0)).unwrap();

        // Losing the radio stops advertising and scanning.
        joiner.start_scan().unwrap();
        assert_eq!(joiner.seen().len(), 1);
        host.set_state(BleState::Off);
        assert!(joiner.seen().is_empty());
    }

    #[test]
    fn two_hosts_are_listed_and_the_platform_picks_the_fake() {
        let air = FakeAir::new();
        let (a, b, c) = (FakeBle::new(air.clone()), FakeBle::new(air.clone()), FakeBle::new(air));
        let _la = a.start_advertising(BleAdvert::new([1; 4], 1)).unwrap();
        let _lb = b.start_advertising(BleAdvert::new([2; 4], 2)).unwrap();
        c.start_scan().unwrap();
        let ids: Vec<String> = c.seen().into_iter().map(|s| s.device_id).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&a.device_id().to_string()) && ids.contains(&b.device_id().to_string()));
        assert_eq!(crate::platform::ble::platform(true).state(), BleState::Ready);
    }
}
