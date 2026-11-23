//! Transports: the [`HostTransport`] / [`ClientTransport`] traits, the
//! in-process [`LoopbackHub`] (injectable clock, latency, jitter and lossy
//! loss), and [`MuxHostTransport`], which puts the host player's own
//! zero-latency loopback link next to the network peers.
//!
//! Forked from Stampede's `race-session` (itself from Twang Thang). Flock
//! adds the joiner's verified persona to `Joined` (the lobby's Bluetooth
//! Accept prompt names it) and the deferred host check of a Bluetooth join.

/// A peer as numbered by a host transport.
pub type PeerKey = u32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkPath {
    /// In-process (the host's own player, tests).
    Local,
    Lan,
    Direct,
    /// Over a fray virtual link (Bluetooth LE).
    Bluetooth,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostNetEvent {
    /// A peer connected and passed the transport's admission (QUIC: its My
    /// Signet attestation verified for the runtime key it proved). `path` is
    /// decided by the host's own door; `persona` is the verified persona
    /// (`npub1...`).
    Joined { peer: PeerKey, path: LinkPath, persona: Option<String> },
    Left { peer: PeerKey },
    Reliable { peer: PeerKey, bytes: Vec<u8> },
    Lossy { peer: PeerKey, bytes: Vec<u8> },
}

/// Who the joiner reached, as proven by the host's signed welcome.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostIdentity {
    /// The host's display name (self-chosen).
    pub name: String,
    /// The host's runtime public key, hex.
    pub pubkey: String,
    /// The host's runtime public key, `npub1...`.
    pub npub: String,
    /// The host's verified My Signet persona (`npub1...`), when the link
    /// checked it at connect time (online/Nearby; Bluetooth checks later).
    pub persona: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientNetEvent {
    /// Free-text progress for the join screen.
    Progress(String),
    /// Linked to the host. `host` is `Some` when the transport proved who the
    /// host is (QUIC). In-process transports pass `None`.
    Connected { host: Option<HostIdentity> },
    /// The join failed before connecting (friendly text).
    Failed(String),
    Reliable(Vec<u8>),
    Lossy(Vec<u8>),
    /// The connection to the host ended.
    Disconnected { host_left: bool, reason: String },
}

/// The host side of a match's links. Non-blocking.
pub trait HostTransport: Send {
    fn poll(&mut self) -> Option<HostNetEvent>;
    fn send_reliable(&mut self, peer: PeerKey, bytes: Vec<u8>);
    fn send_lossy(&mut self, peer: PeerKey, bytes: Vec<u8>);
    fn kick(&mut self, peer: PeerKey);
    /// Drop a peer without banning it (it may come back). Defaults to `kick`.
    fn disconnect(&mut self, peer: PeerKey) {
        self.kick(peer);
    }
    /// Advertise the room and admit new peers (lobby) or not (match running).
    fn set_open(&mut self, open: bool);
    fn rtt_ms(&self, peer: PeerKey) -> Option<u32>;
    /// Tell every peer the host is leaving.
    fn shutdown(&mut self);
}

/// A client's link to the host. Non-blocking.
pub trait ClientTransport: Send {
    fn poll(&mut self) -> Option<ClientNetEvent>;
    fn send_reliable(&mut self, bytes: Vec<u8>);
    fn send_lossy(&mut self, bytes: Vec<u8>);
    fn rtt_ms(&self) -> Option<u32>;
    /// Leave.
    fn close(&mut self);
    /// The host's persona could not be checked when connecting (a Bluetooth
    /// host withholds it from the radio-range welcome) and must still be
    /// verified with [`ClientTransport::verify_host`] before the lobby is
    /// trusted. In-process links: false.
    fn host_check_pending(&self) -> bool {
        false
    }
    /// Verify the host's My Signet attestation (JSON) for the host key this
    /// link proved. `Ok` carries the host's persona (`npub1...`).
    fn verify_host(&mut self, _attestation: &str) -> Result<String, String> {
        Err("this link has no host key to check".into())
    }
}

/// A host transport with no remote side (a room nobody can join): the host's
/// own player still comes through [`MuxHostTransport`].
#[derive(Default)]
pub struct NullHostTransport;

impl HostTransport for NullHostTransport {
    fn poll(&mut self) -> Option<HostNetEvent> {
        None
    }
    fn send_reliable(&mut self, _: PeerKey, _: Vec<u8>) {}
    fn send_lossy(&mut self, _: PeerKey, _: Vec<u8>) {}
    fn kick(&mut self, _: PeerKey) {}
    fn set_open(&mut self, _: bool) {}
    fn rtt_ms(&self, _: PeerKey) -> Option<u32> {
        None
    }
    fn shutdown(&mut self) {}
}

/// Peer keys at or above this belong to the host's own (loopback) side.
pub const LOCAL_PEER_BASE: PeerKey = 1 << 31;

mod native;
pub use native::{LoopClock, LoopbackClient, LoopbackConfig, LoopbackHost, LoopbackHub, MuxHostTransport};
