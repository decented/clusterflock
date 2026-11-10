//! [`HostTransport`] / [`ClientTransport`] over fray-net (QUIC), plus the
//! ways in: host a room (online code and Nearby beacon at once), join by
//! room code, join a room found on the LAN, and Bluetooth (a fray virtual
//! link per joiner).
//!
//! ```ignore
//! let net = flock_session::net::net_config(data_dir);      // app tag "gameflock"
//! let id = identity_with_attestation;                      // the app's Signet-approved identity
//! let host = flock_session::net::host_room(&net, &id, "Ann's flock");  // code via host.code()
//! let client = flock_session::net::join_code(&net, &id, "ABCDEF");
//! let mut browser = flock_session::net::browse_nearby(&net);          // browser.poll() -> Vec<LanRoom>
//! let client = flock_session::net::join_nearby(&net, &id, &room);
//! let bt_host = flock_session::net::host_bluetooth(&net, &id, "1A2B", links);
//! let client = flock_session::net::join_bluetooth(&net, &id, link);
//! ```
//!
//! ## Admission (reviewed code)
//!
//! * Host: every joiner's My Signet attestation must verify for the runtime
//!   key its QUIC handshake proved, for app id [`APP_TAG`], before it is
//!   reported `Joined` (with the verified persona). A failing joiner is
//!   kicked and never reaches the authority. Expiry is enforced live.
//! * Client, online/Nearby: the host's attestation must verify at
//!   `Connected`, or the join fails.
//! * Client, Bluetooth (`PathKind::Virtual`): fray's virtual host withholds
//!   its attestation from the radio-range welcome, so the check is deferred:
//!   [`ClientTransport::host_check_pending`] stays true until
//!   [`ClientTransport::verify_host`] verifies the attestation the host sends
//!   after Accept (`ToClient::HostAttestation`) against the host key QUIC
//!   proved; [`crate::ClientSession`] shows no lobby until then.

use std::path::Path;

pub use decented_net::{Identity, LanBrowser, LanRoom, NetConfig, VirtualLink, VirtualLinkReceiver};
use decented_net::{
    Bytes, ClientEvent, ClientHandle, DisconnectReason, FailReason, HostEvent, HostHandle, JoinStage, PathKind, PeerId,
    RoomInfo,
};

use crate::transport::{ClientNetEvent, ClientTransport, HostIdentity, HostNetEvent, HostTransport, LinkPath, PeerKey};

/// The app tag that namespaces Flock's room codes, QUIC ALPN, LAN beacon and
/// the attestation's app id. Must equal the app's `account::APP_ID`.
pub const APP_TAG: &str = "gameflock";

const _: () = assert!(decented_net::MAX_RELIABLE_LEN == crate::proto::MAX_RELIABLE_LEN);

/// Network settings for Flock with the data dir where the identity key lives.
pub fn net_config(data_dir: impl AsRef<Path>) -> NetConfig {
    NetConfig::new(data_dir.as_ref(), APP_TAG)
}

/// This install's network identity (created on first use, stored in `data_dir`).
pub fn load_identity(data_dir: impl AsRef<Path>) -> Result<Identity, String> {
    Identity::load_or_create(data_dir.as_ref())
}

/// Start hosting a room. The same room is reachable by its room code online
/// and shows up to Nearby browsers on the LAN. Read the code from
/// [`QuicHost::code`] once `is_ready()`.
pub fn host_room(cfg: &NetConfig, id: &Identity, room_name: &str) -> QuicHost {
    let room = RoomInfo { name: room_name.chars().take(24).collect(), max_players: crate::proto::MAX_SEATS as u8 };
    QuicHost::new(decented_net::host(cfg, id, room))
}

/// Host the Bluetooth door: a QUIC host over the radio's joiner links. The
/// room is named by its radio id only ("Bluetooth game · room 1A2B"), never
/// the host's name; fray withholds the host's attestation from the welcome
/// (anyone in radio range can read it).
pub fn host_bluetooth(cfg: &NetConfig, id: &Identity, room_label: &str, links: VirtualLinkReceiver) -> QuicHost {
    let room = RoomInfo {
        name: bluetooth_room_name(room_label),
        max_players: crate::proto::MAX_SEATS as u8,
    };
    QuicHost::new(decented_net::host_virtual(cfg, id, room, links))
}

/// "Bluetooth game · room 1A2B".
pub fn bluetooth_room_name(room_label: &str) -> String {
    format!("Bluetooth game \u{b7} room {}", room_label.chars().take(8).collect::<String>())
}

/// Join by room code (online, through the relays).
pub fn join_code(cfg: &NetConfig, id: &Identity, code: &str) -> QuicClient {
    QuicClient::new(decented_net::join(cfg, id, code))
}

/// Join a room at a known address with its LAN admission token (the
/// headless smoke check; tests).
pub fn join_addr(cfg: &NetConfig, id: &Identity, addr: std::net::SocketAddr, lan_token: &str) -> QuicClient {
    QuicClient::new(decented_net::join_addr(cfg, id, addr, lan_token))
}

/// Join a room found by [`browse_nearby`].
pub fn join_nearby(cfg: &NetConfig, id: &Identity, room: &LanRoom) -> QuicClient {
    QuicClient::new(decented_net::join_lan(cfg, id, room))
}

/// Join over a Bluetooth link (the radio's `connect` result).
pub fn join_bluetooth(cfg: &NetConfig, id: &Identity, link: VirtualLink) -> QuicClient {
    QuicClient::new(decented_net::join_virtual(cfg, id, link))
}

/// Listen for Nearby rooms; `poll()` returns the current list.
pub fn browse_nearby(cfg: &NetConfig) -> LanBrowser {
    decented_net::lan_browse(cfg)
}

/// A failed join's text: the reason, then the stage in brackets.
pub fn join_failure_text(reason: &FailReason, stage: JoinStage) -> String {
    format!("{reason} [{stage:?}]")
}

/// A hosted room as a [`HostTransport`]. Room-level events (`Ready`,
/// `Failed`) are kept for the app to read via [`QuicHost::code`] /
/// [`QuicHost::failure`].
pub struct QuicHost {
    handle: Option<HostHandle>,
    ready: bool,
    failure: Option<String>,
    /// Admitted peers and when their approval expires (unix s).
    approved: std::collections::HashMap<PeerKey, u64>,
    paths: std::collections::HashMap<PeerKey, LinkPath>,
}

impl QuicHost {
    pub fn new(handle: HostHandle) -> Self {
        QuicHost { handle: Some(handle), ready: false, failure: None, approved: Default::default(), paths: Default::default() }
    }

    /// How an approved peer reached this host, while it is connected.
    pub fn peer_path(&self, peer: PeerKey) -> Option<LinkPath> {
        self.paths.get(&peer).copied()
    }

    /// The room code to show (valid once `is_ready`).
    pub fn code(&self) -> Option<&str> {
        self.handle.as_ref().map(|h| h.code())
    }

    pub fn is_ready(&self) -> bool {
        self.ready
    }

    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    pub fn handle(&self) -> Option<&HostHandle> {
        self.handle.as_ref()
    }

    /// The listener's current LAN admission token (tests dialling a LAN
    /// address).
    pub fn lan_token(&self) -> Option<String> {
        self.handle.as_ref().map(|h| h.lan_token())
    }

    /// `(connected, configured)` relays.
    pub fn relays(&self) -> (usize, usize) {
        self.handle.as_ref().map(|h| h.relays_connected()).unwrap_or((0, 0))
    }

    pub fn local_addr(&self) -> Option<std::net::SocketAddr> {
        self.handle.as_ref().and_then(|h| h.local_addr())
    }
}

impl HostTransport for QuicHost {
    fn poll(&mut self) -> Option<HostNetEvent> {
        let h = self.handle.as_ref()?;
        let now = unix_now();
        // Approval expiry is enforced live: the player is removed.
        if let Some(peer) = self.approved.iter().find_map(|(&peer, &until)| (until <= now).then_some(peer)) {
            self.approved.remove(&peer);
            self.paths.remove(&peer);
            // Disconnect, not kick: a kick bans the runtime key for the room's
            // life, and a lapsed approval can be renewed (the runtime key does
            // not change), so the player must be able to come back.
            h.disconnect(PeerId(peer));
            return Some(HostNetEvent::Left { peer });
        }
        loop {
            match h.poll()? {
                HostEvent::Ready { .. } => self.ready = true,
                HostEvent::Failed { reason } => self.failure = Some(reason),
                HostEvent::PeerJoined { peer, path, pubkey, attestation, .. } => {
                    // ADMISSION: the persona attestation must verify for the
                    // runtime key this peer's handshake proved.
                    let info = match verify_persona(attestation.as_deref(), &pubkey, now) {
                        Ok(info) => info,
                        Err(why) => {
                            log::info!("[net] {peer} refused: {why}");
                            // Refused, not banned: they may sign in and come back.
                            h.disconnect(peer);
                            continue;
                        }
                    };
                    let path = match path {
                        PathKind::Lan => LinkPath::Lan,
                        PathKind::Direct => LinkPath::Direct,
                        PathKind::Virtual => LinkPath::Bluetooth,
                    };
                    self.approved.insert(peer.0, info.valid_until);
                    self.paths.insert(peer.0, path);
                    let persona = Some(decented_net::npub_of(&info.persona));
                    return Some(HostNetEvent::Joined { peer: peer.0, path, persona });
                }
                HostEvent::PeerLeft { peer, reason } => {
                    self.paths.remove(&peer.0);
                    if self.approved.remove(&peer.0).is_none() {
                        continue;
                    }
                    log::info!("[net] {peer} left: {reason:?}");
                    return Some(HostNetEvent::Left { peer: peer.0 });
                }
                HostEvent::Reliable { peer, bytes } => {
                    if !self.approved.contains_key(&peer.0) {
                        continue;
                    }
                    return Some(HostNetEvent::Reliable { peer: peer.0, bytes: bytes.to_vec() });
                }
                HostEvent::Lossy { peer, bytes } => {
                    if !self.approved.contains_key(&peer.0) {
                        continue;
                    }
                    return Some(HostNetEvent::Lossy { peer: peer.0, bytes: bytes.to_vec() });
                }
            }
        }
    }

    fn send_reliable(&mut self, peer: PeerKey, bytes: Vec<u8>) {
        if let Some(h) = &self.handle {
            h.send_reliable(PeerId(peer), Bytes::from(bytes));
        }
    }

    fn send_lossy(&mut self, peer: PeerKey, bytes: Vec<u8>) {
        if let Some(h) = &self.handle {
            h.send_lossy(PeerId(peer), Bytes::from(bytes));
        }
    }

    fn kick(&mut self, peer: PeerKey) {
        if let Some(h) = &self.handle {
            h.kick(PeerId(peer));
        }
    }

    fn disconnect(&mut self, peer: PeerKey) {
        if let Some(h) = &self.handle {
            h.disconnect(PeerId(peer));
        }
    }

    fn set_open(&mut self, open: bool) {
        if let Some(h) = &self.handle {
            if h.is_open() != open {
                h.set_open(open);
            }
        }
    }

    fn rtt_ms(&self, peer: PeerKey) -> Option<u32> {
        self.handle.as_ref()?.rtt_ms(PeerId(peer))
    }

    fn shutdown(&mut self) {
        if let Some(h) = self.handle.take() {
            h.shutdown();
        }
    }
}

/// A join (by code, LAN or Bluetooth) as a [`ClientTransport`].
pub struct QuicClient {
    handle: Option<ClientHandle>,
    /// When the host's approval expires (unix s), once verified.
    allowed_until: Option<u64>,
    path: Option<LinkPath>,
    failed: Option<(JoinStage, FailReason)>,
    /// Bluetooth: the host key QUIC proved, kept for the deferred check.
    host_key: Option<nostr::PublicKey>,
    /// Bluetooth: the host's attestation is still to be verified.
    deferred: bool,
}

impl QuicClient {
    pub fn new(handle: ClientHandle) -> Self {
        QuicClient { handle: Some(handle), allowed_until: None, path: None, failed: None, host_key: None, deferred: false }
    }

    /// Where and why the join failed, once its `Failed` event was polled.
    pub fn failed_join(&self) -> Option<&(JoinStage, FailReason)> {
        self.failed.as_ref()
    }

    /// How this join reached the host, once connected.
    pub fn link_path(&self) -> Option<LinkPath> {
        self.path
    }

    fn reject(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = std::thread::Builder::new().name("flock-persona-close".into()).spawn(move || handle.shutdown());
        }
    }
}

fn stage_text(stage: JoinStage) -> &'static str {
    match stage {
        JoinStage::Preparing => "Getting ready...",
        JoinStage::FindingHost => "Looking for the room...",
        JoinStage::Connecting => "Connecting...",
        JoinStage::Handshake => "Joining...",
    }
}

impl ClientTransport for QuicClient {
    fn poll(&mut self) -> Option<ClientNetEvent> {
        let h = self.handle.as_ref()?;
        if self.allowed_until.is_some_and(|until| until <= unix_now()) {
            self.reject();
            return Some(ClientNetEvent::Disconnected {
                host_left: false,
                reason: "The host's My Signet approval expired.".into(),
            });
        }
        Some(match h.poll()? {
            ClientEvent::Progress { stage } => ClientNetEvent::Progress(stage_text(stage).into()),
            ClientEvent::Connected { path, host_name, host_pubkey, attestation, .. } => {
                let path = match path {
                    PathKind::Lan => LinkPath::Lan,
                    PathKind::Direct => LinkPath::Direct,
                    PathKind::Virtual => LinkPath::Bluetooth,
                };
                self.path = Some(path);
                let mut persona = None;
                if path == LinkPath::Bluetooth {
                    // ADMISSION: a Bluetooth host withholds its attestation
                    // here; it is verified after Accept (`verify_host`).
                    self.host_key = Some(host_pubkey);
                    self.deferred = true;
                } else {
                    match verify_persona(attestation.as_deref(), &host_pubkey, unix_now()) {
                        Ok(info) => {
                            self.allowed_until = Some(info.valid_until);
                            persona = Some(decented_net::npub_of(&info.persona));
                        }
                        Err(_) => {
                            self.reject();
                            return Some(ClientNetEvent::Failed(crate::client::HOST_UNVERIFIED.into()));
                        }
                    }
                }
                ClientNetEvent::Connected {
                    host: Some(HostIdentity {
                        name: host_name,
                        pubkey: host_pubkey.to_hex(),
                        npub: decented_net::npub_of(&host_pubkey),
                        persona,
                    }),
                }
            }
            ClientEvent::Failed { stage, reason } => {
                log::info!("[net] join failed at {stage:?}: {reason:?}");
                let text = join_failure_text(&reason, stage);
                self.failed = Some((stage, reason));
                ClientNetEvent::Failed(text)
            }
            ClientEvent::Reliable(b) => ClientNetEvent::Reliable(b.to_vec()),
            ClientEvent::Lossy(b) => ClientNetEvent::Lossy(b.to_vec()),
            ClientEvent::Disconnected { reason } => ClientNetEvent::Disconnected {
                host_left: reason == DisconnectReason::HostLeft,
                reason: format!("{reason:?}"),
            },
        })
    }

    fn send_reliable(&mut self, bytes: Vec<u8>) {
        if let Some(h) = &self.handle {
            h.send_reliable(Bytes::from(bytes));
        }
    }

    fn send_lossy(&mut self, bytes: Vec<u8>) {
        if let Some(h) = &self.handle {
            h.send_lossy(Bytes::from(bytes));
        }
    }

    fn rtt_ms(&self) -> Option<u32> {
        self.handle.as_ref()?.rtt_ms()
    }

    fn close(&mut self) {
        if let Some(h) = self.handle.take() {
            h.shutdown();
        }
    }

    fn host_check_pending(&self) -> bool {
        self.deferred
    }

    fn verify_host(&mut self, attestation: &str) -> Result<String, String> {
        // Only the key this link's handshake proved can be vouched for.
        let key = self.host_key.ok_or("no host key was proved on this link")?;
        let info = verify_persona(Some(attestation), &key, unix_now())?;
        self.deferred = false;
        self.allowed_until = Some(info.valid_until);
        Ok(decented_net::npub_of(&info.persona))
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|t| t.as_secs()).unwrap_or(0)
}

/// Verify a My Signet persona attestation for `runtime` (the key a QUIC
/// handshake proved) and this app.
pub fn verify_persona(
    attestation: Option<&str>,
    runtime: &nostr::PublicKey,
    now: u64,
) -> Result<decented_signet::attestation::PersonaInfo, String> {
    let json = attestation.ok_or("Missing My Signet approval")?;
    decented_signet::verify_attestation(APP_TAG, json.as_bytes(), runtime, now).map_err(|e| e.to_string())
}

/// The identity's own attestation and the persona it names (`npub1...`), if
/// it verifies for this app now. The host hands both to
/// [`crate::HostAuthority::set_host_identity`].
pub fn own_persona(id: &Identity) -> Option<(String, String)> {
    let json = id.attestation()?.to_string();
    let info = verify_persona(Some(&json), &id.pubkey(), unix_now()).ok()?;
    Some((json, decented_net::npub_of(&info.persona)))
}

/// Shown to a host who tries to open the Bluetooth door without a valid
/// My Signet approval of its own.
pub const BLUETOOTH_NEEDS_SIGNET: &str = "Sign in with My Signet to host over Bluetooth.";

/// Whether `id` can host over Bluetooth now: Bluetooth joiners verify the
/// host's own attestation after Accept, so a host without a currently valid
/// one would only accept joiners who then fail. `Err` is text for the UI.
pub fn bluetooth_host_check(id: &Identity) -> Result<(), String> {
    own_persona(id).map(|_| ()).ok_or_else(|| BLUETOOTH_NEEDS_SIGNET.to_string())
}

/// A runtime identity vouched for by a synthetic `persona`, signed now (the
/// headless host and tests; never the app, which signs in with My Signet).
pub fn sign_identity(runtime: Identity, persona: &nostr::Keys) -> Result<Identity, String> {
    let event = decented_signet::attestation::unsigned_attestation(APP_TAG, &persona.public_key(), &runtime.pubkey(), unix_now())
        .sign_with_keys(persona)
        .map_err(|e| e.to_string())?;
    Ok(runtime.with_attestation(serde_json::to_string(&event).map_err(|e| e.to_string())?))
}

/// A throwaway identity with a fresh synthetic persona.
pub fn test_identity() -> Identity {
    sign_identity(Identity::ephemeral(), &nostr::Keys::generate()).expect("signing a fresh identity")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{ClientPhase, ClientSession};
    use crate::door::{HostLink, Radio};
    use crate::host::{HostAuthority, HostConfig};
    use crate::proto::SeatPath;
    use flock_core::InputFrame;
    use nostr::Keys;
    use std::time::{Duration, Instant};

    #[test]
    fn network_approval_is_signed_for_this_game_and_the_proven_runtime_key() {
        let persona = Keys::generate();
        let runtime = Keys::generate();
        let now = unix_now();
        let attest = |app, key: &nostr::PublicKey, at| {
            let event = decented_signet::attestation::unsigned_attestation(app, &persona.public_key(), key, at)
                .sign_with_keys(&persona)
                .unwrap();
            serde_json::to_string(&event).unwrap()
        };
        let valid = attest(APP_TAG, &runtime.public_key(), now);
        assert!(verify_persona(Some(&valid), &runtime.public_key(), now).is_ok());
        assert!(verify_persona(None, &runtime.public_key(), now).is_err());
        assert!(verify_persona(Some("{}"), &runtime.public_key(), now).is_err());
        assert!(verify_persona(Some(&valid), &Keys::generate().public_key(), now).is_err());
        assert!(verify_persona(Some(&attest("stampede", &runtime.public_key(), now)), &runtime.public_key(), now).is_err());
        assert!(verify_persona(Some(&valid), &runtime.public_key(), now + 100 * 86400).is_err());
        let tampered = valid.replace("gameflock", "gameflock-edited");
        assert!(verify_persona(Some(&tampered), &runtime.public_key(), now).is_err());
        let id = sign_identity(Identity::from_keys(runtime.clone()), &persona).unwrap();
        let (_, npub) = own_persona(&id).unwrap();
        assert_eq!(npub, decented_net::npub_of(&persona.public_key()));
        assert!(own_persona(&Identity::ephemeral()).is_none());
    }

    fn quiet_cfg(dir: &std::path::Path) -> NetConfig {
        let mut c = net_config(dir);
        c.relays.clear();
        c.lan_beacon = false;
        c.stun.clear();
        c
    }

    struct FakeRadio;
    impl Radio for FakeRadio {
        fn stop_advertising(&self) {}
        fn advertising_error(&self) -> Option<String> {
            None
        }
        fn room_label(&self) -> String {
            "1A2B".into()
        }
    }

    fn pump(host: &mut HostAuthority, clients: &mut [&mut ClientSession], t0: Instant, until: impl Fn(&HostAuthority, &[&mut ClientSession]) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            let now = t0.elapsed().as_millis() as u64 + 1;
            host.update(now);
            for c in clients.iter_mut() {
                c.update(now, InputFrame::default());
            }
            if until(host, clients) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    /// Bluetooth over real QUIC on in-memory fray virtual links: the joiner
    /// waits for Accept, is seated through the Bluetooth door with its
    /// verified persona, and verifies the host's persona after Accept.
    #[test]
    fn bluetooth_join_over_virtual_links_needs_accept_and_verifies_the_host_after() {
        let dir = std::env::temp_dir().join(format!("flock-bt-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let cfg = quiet_cfg(&dir);
        let host_persona = Keys::generate();
        let host_id = sign_identity(Identity::ephemeral(), &host_persona).unwrap();
        let joiner_persona = Keys::generate();
        let joiner_id = sign_identity(Identity::ephemeral(), &joiner_persona).unwrap();

        let (link_tx, link_rx) = decented_net::virtual_link_queue(4);
        let bt = host_bluetooth(&cfg, &host_id, "1A2B", link_rx);
        let lan = NullLan;
        let door = HostLink::new(lan);
        door.start_bluetooth(Box::new(bt), Box::new(FakeRadio)).unwrap();
        let mut host = HostAuthority::new(Box::new(door.clone()), HostConfig::new(5));
        let (json, npub) = own_persona(&host_id).unwrap();
        host.set_host_identity(Some(json), Some(npub.clone()));

        let (a, b) = decented_net::virtual_pair("joiner", "host", 64);
        link_tx.try_send(b).expect("queue a link");
        let mut me = ClientSession::new(Box::new(join_bluetooth(&cfg, &joiner_id, a)), "Radio", 2);
        let t0 = Instant::now();
        assert!(
            pump(&mut host, &mut [&mut me], t0, |h, c| !h.pending_joiners().is_empty() && *c[0].phase() == ClientPhase::Waiting),
            "joiner waits: {:?}",
            me.phase()
        );
        let p = host.pending_joiners()[0].clone();
        assert_eq!(p.persona.as_deref(), Some(decented_net::npub_of(&joiner_persona.public_key()).as_str()));
        assert_eq!(host.human_count(), 0);
        assert!(host.accept_pending(p.peer));
        assert!(pump(&mut host, &mut [&mut me], t0, |_, c| *c[0].phase() == ClientPhase::Lobby), "seated: {:?}", me.phase());
        assert_eq!(me.host_persona(), Some(npub.as_str()), "host verified after Accept");
        let l = me.lobby().unwrap();
        assert_eq!(l.seats[0].path, SeatPath::Bluetooth);
        assert_eq!(l.seats[0].persona.as_deref(), p.persona.as_deref());
        host.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A Bluetooth host whose attestation does not verify for the key it
    /// proved: the joiner is accepted, then refuses the host.
    #[test]
    fn a_bluetooth_host_with_a_bad_attestation_is_refused_after_accept() {
        let dir = std::env::temp_dir().join(format!("flock-bt-bad-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let cfg = quiet_cfg(&dir);
        let host_id = sign_identity(Identity::ephemeral(), &Keys::generate()).unwrap();
        let joiner_id = test_identity();
        let (link_tx, link_rx) = decented_net::virtual_link_queue(4);
        let door = HostLink::new(NullLan);
        door.start_bluetooth(Box::new(host_bluetooth(&cfg, &host_id, "1A2B", link_rx)), Box::new(FakeRadio)).unwrap();
        let mut host = HostAuthority::new(Box::new(door), HostConfig::new(6));
        // Someone else's attestation (valid, but for another runtime key).
        let other = test_identity();
        host.set_host_identity(other.attestation().map(str::to_string), None);
        let (a, b) = decented_net::virtual_pair("joiner", "host", 64);
        link_tx.try_send(b).expect("queue a link");
        let mut me = ClientSession::new(Box::new(join_bluetooth(&cfg, &joiner_id, a)), "Radio", 2);
        let t0 = Instant::now();
        assert!(pump(&mut host, &mut [&mut me], t0, |h, _| !h.pending_joiners().is_empty()));
        let peer = host.pending_joiners()[0].peer;
        host.accept_pending(peer);
        assert!(pump(&mut host, &mut [&mut me], t0, |_, c| c[0].phase().is_dead()), "{:?}", me.phase());
        assert_eq!(*me.phase(), ClientPhase::Failed(crate::client::HOST_UNVERIFIED.into()));
        assert!(me.lobby().is_none() || me.host_persona().is_none());
        host.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A joiner without a valid attestation never reaches the authority.
    #[test]
    fn a_joiner_without_an_attestation_is_kicked_by_the_transport() {
        let dir = std::env::temp_dir().join(format!("flock-bt-noatt-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let cfg = quiet_cfg(&dir);
        let host_id = test_identity();
        let (link_tx, link_rx) = decented_net::virtual_link_queue(4);
        let mut bt = host_bluetooth(&cfg, &host_id, "1A2B", link_rx);
        let (a, b) = decented_net::virtual_pair("joiner", "host", 64);
        link_tx.try_send(b).unwrap();
        let mut c = join_bluetooth(&cfg, &Identity::ephemeral(), a);
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut joined = false;
        let mut gone = false;
        while Instant::now() < deadline && !gone {
            while let Some(ev) = bt.poll() {
                joined |= matches!(ev, HostNetEvent::Joined { .. });
            }
            while let Some(ev) = c.poll() {
                gone |= matches!(ev, ClientNetEvent::Disconnected { .. } | ClientNetEvent::Failed(_));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!joined, "never reported as joined");
        assert!(gone, "the joiner was dropped");
        bt.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L2: a refused admission disconnects without banning the runtime key:
    /// the same key may come back with a valid approval.
    #[test]
    fn a_refused_joiner_is_not_banned_and_can_come_back_approved() {
        let dir = std::env::temp_dir().join(format!("flock-bt-retry-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let cfg = quiet_cfg(&dir);
        let host_id = test_identity();
        let (link_tx, link_rx) = decented_net::virtual_link_queue(4);
        let mut bt = host_bluetooth(&cfg, &host_id, "1A2B", link_rx);
        let runtime = Identity::ephemeral();
        let approved = sign_identity(runtime.clone(), &Keys::generate()).unwrap();
        assert_eq!(runtime.pubkey(), approved.pubkey());
        let try_join = |bt: &mut QuicHost, id: &Identity| -> bool {
            let (a, b) = decented_net::virtual_pair("joiner", "host", 64);
            link_tx.try_send(b).unwrap();
            let mut c = join_bluetooth(&cfg, id, a);
            let deadline = Instant::now() + Duration::from_secs(10);
            while Instant::now() < deadline {
                while let Some(ev) = bt.poll() {
                    if matches!(ev, HostNetEvent::Joined { .. }) {
                        return true;
                    }
                }
                while let Some(ev) = c.poll() {
                    if matches!(ev, ClientNetEvent::Disconnected { .. } | ClientNetEvent::Failed(_)) {
                        return false;
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        };
        assert!(!try_join(&mut bt, &runtime), "no approval: refused");
        assert!(try_join(&mut bt, &approved), "the same key is not banned once it has an approval");
        bt.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// L3: only a host with a valid approval of its own can open the
    /// Bluetooth door.
    #[test]
    fn the_bluetooth_door_needs_the_hosts_own_approval() {
        assert_eq!(bluetooth_host_check(&Identity::ephemeral()), Err(BLUETOOTH_NEEDS_SIGNET.to_string()));
        assert!(bluetooth_host_check(&test_identity()).is_ok());
    }

    /// A LAN door that never has peers.
    struct NullLan;
    impl HostTransport for NullLan {
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
}
