//! What a Keep (a home game server) needs of the session crate, on real QUIC
//! over loopback and Fray's in-memory relay (no public relays): a host with no
//! player seat behind an admission gate, `peers_admitted_by`, polite removal,
//! the Keep pass, joining by invite, and the headless client. Everything runs
//! on the wall clock; the links are real.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use decented_net::relay::FakeRelayHub;
use decented_net::{Listener, ListenerConfig, RoomInfo};
use flock_core::{InputFrame, Phase};

use crate::client::{ClientPhase, ClientSession};
use crate::door::HostLink;
use crate::host::{HostAuthority, HostConfig, HostPhase};
use crate::net::{self, Admission, AdmissionGate, Identity, InviteId, JoinInfo, NetConfig, QuicHost};
use crate::probe::{run_clients, ProbeConfig, ProbeEnd, ProbeError};
use crate::proto::{self, KeepPass, ToClient, MAX_SEATS, PROTO_VERSION};
use crate::transport::PeerKey;

/// A runtime identity with a valid persona attestation, and the persona key.
fn approved() -> (Identity, [u8; 32]) {
    let persona = nostr::Keys::generate();
    let id = net::sign_identity(Identity::ephemeral(), &persona).expect("signing");
    (id, persona.public_key().to_bytes())
}

fn secret(n: u8) -> [u8; 32] {
    let mut s = [0x5a; 32];
    s[0] = n;
    s
}

/// A gate that records every join it is asked about and lets `admit` decide.
fn recording_gate(
    admit: impl Fn(&JoinInfo) -> Result<(), String> + Send + 'static,
) -> (AdmissionGate, Arc<Mutex<Vec<JoinInfo>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    let gate: AdmissionGate = Box::new(move |join: &JoinInfo| {
        log.lock().unwrap().push(*join);
        admit(join)
    });
    (gate, seen)
}

/// A Keep's room: a Fray listener, an in-memory relay, a host with no player
/// seat (`host_has_player = false`) and, optionally, a gate in front.
struct Rig {
    listener: Listener,
    hub: FakeRelayHub,
    keep: Identity,
    cfg: NetConfig,
    link: HostLink<QuicHost>,
    host: HostAuthority,
    start: Instant,
    dir: std::path::PathBuf,
}

impl Rig {
    fn new(gate: Option<AdmissionGate>, bots: usize) -> Rig {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "flock-keep-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::create_dir_all(&dir);
        let listener =
            Listener::bind(ListenerConfig { stun: Vec::new(), upnp: false, ..ListenerConfig::new() }).expect("bind");
        let hub = FakeRelayHub::new();
        let keep = net::test_identity();
        let mut cfg = net::net_config(&dir);
        cfg.relays = vec!["wss://relay.invalid".into()];
        cfg.relay_fallback = false;
        cfg.stun = Vec::new();
        cfg.lan_beacon = false;
        cfg.host_has_player = false;
        let handle = listener.host_with_relay(
            &cfg,
            &keep,
            RoomInfo { name: "Keep room".into(), max_players: MAX_SEATS as u8 },
            Some(Box::new(hub.client())),
        );
        let mut quic = QuicHost::new(handle);
        if let Some(gate) = gate {
            quic.set_admission_gate(gate);
        }
        let started = Instant::now();
        while !quic.is_ready() {
            use crate::transport::HostTransport;
            assert!(quic.poll().is_none());
            assert!(quic.failure().is_none(), "room: {:?}", quic.failure());
            assert!(started.elapsed() < Duration::from_secs(10), "room not ready");
            std::thread::sleep(Duration::from_millis(2));
        }
        let link = HostLink::new(quic);
        let mut host = HostAuthority::new(Box::new(link.clone()), HostConfig::new(9));
        for _ in 0..bots {
            assert!(host.add_bot());
        }
        Rig { listener, hub, keep, cfg, link, host, start: Instant::now(), dir }
    }

    fn keep_key(&self) -> [u8; 32] {
        self.keep.pubkey().to_bytes()
    }

    /// Register an invite and make this room its target.
    fn invite(&self, n: u8) -> ([u8; 32], InviteId) {
        let s = secret(n);
        let id = self.listener.add_invite(net::APP_TAG, s).expect("add_invite");
        assert_eq!(id, InviteId::of(&s));
        self.link.with_lan(|q| q.handle().expect("handle").set_invite_target(true)).expect("target");
        std::thread::sleep(Duration::from_millis(400));
        (s, id)
    }

    fn code(&self) -> String {
        self.link.with_lan(|q| q.code().expect("code").to_string())
    }

    fn join_invite(&self, id: &Identity, secret: &[u8; 32], name: &str) -> ClientSession {
        let link = net::join_invite_with_relay(&self.cfg, id, secret, self.keep_key(), Box::new(self.hub.client()));
        ClientSession::new(Box::new(link), name, 0)
    }

    fn join_code(&self, id: &Identity, name: &str) -> ClientSession {
        let handle = decented_net::join_with_relay(&self.cfg, id, &self.code(), Box::new(self.hub.client()));
        ClientSession::new(Box::new(net::QuicClient::new(handle)), name, 0)
    }

    fn now(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn step(&mut self, clients: &mut [&mut ClientSession]) {
        let now = self.now();
        self.host.update(now);
        for c in clients.iter_mut() {
            c.update(now, InputFrame::default());
        }
        std::thread::sleep(Duration::from_millis(3));
    }

    fn run_until(
        &mut self,
        clients: &mut [&mut ClientSession],
        secs: u64,
        mut done: impl FnMut(&Rig, &mut [&mut ClientSession]) -> bool,
    ) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            self.step(clients);
            if done(self, clients) {
                return true;
            }
        }
        false
    }

    fn peer_of(&self, c: &ClientSession) -> PeerKey {
        self.host.peer_of_seat(c.my_seat().expect("seated")).expect("peer")
    }

    fn admitted_by(&self, invite: InviteId) -> Vec<PeerKey> {
        self.link.with_lan(|q| q.peers_admitted_by(invite))
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.host.shutdown();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn dead(c: &ClientSession) -> bool {
    c.phase().is_dead()
}

fn in_lobby(c: &ClientSession) -> bool {
    *c.phase() == ClientPhase::Lobby
}

#[test]
fn a_gated_host_admits_an_invite_joiner_and_refuses_a_code_joiner() {
    // The Keep's rule here: only the invite gets in.
    let (gate, seen) = recording_gate(|join| match join.admitted_by {
        Admission::Invite(_) => Ok(()),
        other => Err(format!("not by {other:?}")),
    });
    let mut rig = Rig::new(Some(gate), 1);
    let (secret, invite) = rig.invite(1);
    let (guest, guest_persona) = approved();
    let mut a = rig.join_invite(&guest, &secret, "Ada");
    assert!(rig.run_until(&mut [&mut a], 20, |_, c| in_lobby(c[0])), "invited joiner seated: {:?}", a.phase());
    assert_eq!(rig.host.human_count(), 1, "the host has no seat of its own");
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].admitted_by, Admission::Invite(invite));
        assert_eq!(seen[0].persona, guest_persona);
        assert_ne!(seen[0].path, crate::transport::LinkPath::Bluetooth);
    }
    // By room code: refused by the gate; the host never seats it.
    let (other, _) = approved();
    let mut b = rig.join_code(&other, "Bea");
    assert!(rig.run_until(&mut [&mut a, &mut b], 20, |_, c| dead(c[1])), "code joiner refused: {:?}", b.phase());
    assert!(b.my_seat().is_none());
    assert_eq!(seen.lock().unwrap().last().unwrap().admitted_by, Admission::Code);
    assert_eq!(rig.host.human_count(), 1);
    assert!(in_lobby(&a), "the admitted player is not disturbed");
    // Refused, not banned: the same key comes back by the invite and is seated.
    let mut again = rig.join_invite(&other, &secret, "Bea");
    assert!(rig.run_until(&mut [&mut a, &mut again], 20, |_, c| in_lobby(c[1])), "{:?}", again.phase());
    assert_eq!(rig.host.human_count(), 2);
}

#[test]
fn an_invite_that_is_not_registered_never_reaches_the_gate() {
    let (gate, seen) = recording_gate(|_| Ok(()));
    let mut rig = Rig::new(Some(gate), 1);
    rig.invite(1);
    let (id, _) = approved();
    let mut stranger = rig.join_invite(&id, &secret(2), "Stranger");
    assert!(rig.run_until(&mut [&mut stranger], 20, |_, c| dead(c[0])), "{:?}", stranger.phase());
    assert!(seen.lock().unwrap().is_empty(), "Fray refused it before the gate");
    assert_eq!(rig.host.human_count(), 0);
}

#[test]
fn the_gate_sees_the_persona_that_passed_the_check_and_a_borrowed_approval_never_reaches_it() {
    let (gate, seen) = recording_gate(|_| Ok(()));
    let mut rig = Rig::new(Some(gate), 1);
    let (secret, _) = rig.invite(1);
    // Two players with the same display name, two personas: the gate hears each persona.
    let (id1, persona1) = approved();
    let (id2, persona2) = approved();
    assert_ne!(persona1, persona2);
    let mut a = rig.join_invite(&id1, &secret, "Sam");
    let mut b = rig.join_invite(&id2, &secret, "Sam");
    assert!(rig.run_until(&mut [&mut a, &mut b], 20, |_, c| in_lobby(c[0]) && in_lobby(c[1])));
    {
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        let mut got: Vec<[u8; 32]> = seen.iter().map(|j| j.persona).collect();
        let mut want = vec![persona1, persona2];
        got.sort();
        want.sort();
        assert_eq!(got, want, "the attested personas, whatever the names say");
        assert!(seen.iter().all(|j| j.persona != id1.pubkey().to_bytes() && j.persona != id2.pubkey().to_bytes()));
    }
    // The persona the gate saw is the one the lobby shows for that seat.
    let seats = rig.host.lobby_view().seats;
    let npub1 = decented_net::npub_of(&nostr::PublicKey::from_slice(&persona1).unwrap());
    assert_eq!(seats.iter().filter(|s| s.persona.as_deref() == Some(npub1.as_str())).count(), 1);
    // Someone else's approval on my runtime key (a persona it was not signed
    // for), an approval for another game, and no approval at all: the
    // transport's own check fails them, so the gate is never asked.
    let before = seen.lock().unwrap().len();
    let stolen = Identity::ephemeral().with_attestation(id1.attestation().expect("attested").to_string());
    let persona = nostr::Keys::generate();
    let other_game = {
        let runtime = Identity::ephemeral();
        let event = decented_signet::attestation::unsigned_attestation(
            "stampede",
            &persona.public_key(),
            &runtime.pubkey(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs(),
        )
        .sign_with_keys(&persona)
        .unwrap();
        runtime.with_attestation(serde_json::to_string(&event).unwrap())
    };
    let mut liars: Vec<ClientSession> = [stolen, other_game, Identity::ephemeral()]
        .iter()
        .map(|id| rig.join_invite(id, &secret, "Sam"))
        .collect();
    let mut all: Vec<&mut ClientSession> = vec![&mut a, &mut b];
    all.extend(liars.iter_mut());
    assert!(rig.run_until(&mut all, 20, |_, c| c[2..].iter().all(|l| dead(l))), "every claim refused");
    drop(all);
    assert_eq!(seen.lock().unwrap().len(), before, "none of them reached the gate");
    assert_eq!(rig.host.human_count(), 2);
}

#[test]
fn peers_admitted_by_lists_exactly_the_peers_of_that_invite_and_drops_those_who_leave() {
    let mut rig = Rig::new(None, 1);
    let (secret_a, invite_a) = rig.invite(1);
    let (secret_b, invite_b) = rig.invite(2);
    assert!(rig.admitted_by(invite_a).is_empty());
    let mut a1 = rig.join_invite(&approved().0, &secret_a, "A1");
    let mut a2 = rig.join_invite(&approved().0, &secret_a, "A2");
    let mut b1 = rig.join_invite(&approved().0, &secret_b, "B1");
    let mut by_code = rig.join_code(&approved().0, "Code");
    assert!(rig.run_until(&mut [&mut a1, &mut a2, &mut b1, &mut by_code], 30, |_, c| c.iter().all(|x| in_lobby(x))));
    let (pa1, pa2, pb1, pc) = (rig.peer_of(&a1), rig.peer_of(&a2), rig.peer_of(&b1), rig.peer_of(&by_code));
    let mut want_a = vec![pa1, pa2];
    want_a.sort_unstable();
    assert_eq!(rig.admitted_by(invite_a), want_a);
    assert_eq!(rig.admitted_by(invite_b), vec![pb1]);
    assert_eq!(rig.link.with_lan(|q| q.peer_admission(pc)), Some(Admission::Code));
    assert_eq!(rig.link.with_lan(|q| q.peer_admission(pa1)), Some(Admission::Invite(invite_a)));
    // a1 leaves: it is gone from the list, the others are not.
    a1.leave();
    assert!(rig.run_until(&mut [&mut a1, &mut a2, &mut b1, &mut by_code], 20, |r, _| r.admitted_by(invite_a) == vec![pa2]));
    assert_eq!(rig.admitted_by(invite_b), vec![pb1]);
    assert_eq!(rig.host.human_count(), 3);
}

#[test]
fn an_ungated_host_admits_as_before() {
    let mut rig = Rig::new(None, 1);
    let (secret, _) = rig.invite(1);
    let mut by_code = rig.join_code(&approved().0, "Code");
    let mut by_invite = rig.join_invite(&approved().0, &secret, "Invite");
    assert!(rig.run_until(&mut [&mut by_code, &mut by_invite], 30, |_, c| in_lobby(c[0]) && in_lobby(c[1])));
    assert_eq!(rig.host.human_count(), 2);
    // A joiner without an approval is still refused by the transport's own check.
    let mut bare = rig.join_code(&Identity::ephemeral(), "Bare");
    assert!(rig.run_until(&mut [&mut by_code, &mut by_invite, &mut bare], 20, |_, c| dead(c[2])));
    assert_eq!(rig.host.human_count(), 2);
}

#[test]
fn a_keep_removes_a_player_politely_without_a_ban_and_hands_them_a_pass() {
    let mut rig = Rig::new(None, 1);
    let (secret, _) = rig.invite(1);
    let (id, _) = approved();
    let mut a = rig.join_invite(&id, &secret, "Ada");
    let mut b = rig.join_invite(&approved().0, &secret, "Bob");
    assert!(rig.run_until(&mut [&mut a, &mut b], 30, |_, c| in_lobby(c[0]) && in_lobby(c[1])));
    // A pass for this very Keep arrives once, to the one it was sent to.
    let pass = KeepPass { host_key: rig.keep_key(), secret };
    assert!(rig.host.send_keep_pass(rig.peer_of(&a), pass.clone()));
    let mut got = Vec::new();
    rig.run_until(&mut [&mut a, &mut b], 10, |_, c| {
        got.extend(c[0].take_keep_passes());
        !got.is_empty()
    });
    assert_eq!(got, vec![pass]);
    assert!(b.take_keep_passes().is_empty(), "the other player gets none");
    // A pass naming another Keep is not believed: a good pass sent right
    // after it is the only one that arrives, so the bad one was delivered
    // (messages are in order) and dropped, not lost.
    assert!(rig.host.send_keep_pass(rig.peer_of(&b), KeepPass { host_key: [7; 32], secret }));
    let good = KeepPass { host_key: rig.keep_key(), secret: [9; 32] };
    assert!(rig.host.send_keep_pass(rig.peer_of(&b), good.clone()));
    let mut got_b = Vec::new();
    assert!(
        rig.run_until(&mut [&mut a, &mut b], 20, |_, c| {
            got_b.extend(c[1].take_keep_passes());
            !got_b.is_empty()
        }),
        "the good pass never arrived"
    );
    assert_eq!(got_b, vec![good], "a host may only hand out a way back into itself");
    // Remove Ada: told why, seat freed, Bob untouched.
    assert!(rig.host.remove_peer_with_reason(rig.peer_of(&a), "The Keep removed you: your invite was cancelled."));
    assert!(rig.run_until(&mut [&mut a, &mut b], 20, |_, c| dead(c[0])));
    assert_eq!(*a.phase(), ClientPhase::Closed("The Keep removed you: your invite was cancelled.".into()));
    assert_eq!(rig.host.human_count(), 1);
    assert!(in_lobby(&b));
    // Nobody is left on that peer: nothing is sent, nothing removed.
    assert!(!rig.host.remove_peer(PeerKey::MAX));
    assert!(!rig.host.send_keep_pass(PeerKey::MAX, KeepPass { host_key: [0; 32], secret: [0; 32] }));
    // No ban: the same identity is seated again.
    let mut again = rig.join_invite(&id, &secret, "Ada");
    assert!(rig.run_until(&mut [&mut b, &mut again], 20, |_, c| in_lobby(c[1])), "{:?}", again.phase());
    assert_eq!(rig.host.human_count(), 2);
}

#[test]
fn a_room_code_joiner_keeps_a_pass_naming_the_host_it_proved_and_drops_any_other() {
    // A gated Keep that admits by room code as well as by invite.
    let (gate, _) = recording_gate(|_| Ok(()));
    let mut rig = Rig::new(Some(gate), 1);
    let mut by_code = rig.join_code(&approved().0, "Code");
    assert!(rig.run_until(&mut [&mut by_code], 30, |_, c| in_lobby(c[0])), "{:?}", by_code.phase());
    assert_eq!(rig.host.human_count(), 1, "the Keep has no seat of its own");
    let peer = rig.peer_of(&by_code);
    // Another host's key first, then the proven host's own: messages arrive in order, so the
    // good one being the only pass shows the bad one was delivered and dropped.
    assert!(rig.host.send_keep_pass(peer, KeepPass { host_key: [7; 32], secret: [1; 32] }));
    let own = KeepPass { host_key: rig.keep_key(), secret: [2; 32] };
    assert!(rig.host.send_keep_pass(peer, own.clone()));
    let mut got = Vec::new();
    assert!(rig.run_until(&mut [&mut by_code], 20, |_, c| {
        got.extend(c[0].take_keep_passes());
        !got.is_empty()
    }));
    assert_eq!(got, vec![own], "a pass naming the host the code join proved is kept; another key's is not");
}

/// A pass is believed only when the transport proved this very host key: a
/// fake link, no network, so the "no key proved" case can be reached.
mod pass_rule {
    use std::collections::VecDeque;

    use super::*;
    use crate::client::ClientSession;
    use crate::transport::{ClientNetEvent, ClientTransport, HostIdentity};

    struct Fake {
        inq: Arc<Mutex<VecDeque<ClientNetEvent>>>,
    }
    impl ClientTransport for Fake {
        fn poll(&mut self) -> Option<ClientNetEvent> {
            self.inq.lock().unwrap().pop_front()
        }
        fn send_reliable(&mut self, _: Vec<u8>) {}
        fn send_lossy(&mut self, _: Vec<u8>) {}
        fn rtt_ms(&self) -> Option<u32> {
            None
        }
        fn close(&mut self) {}
    }

    fn hex(k: &[u8; 32]) -> String {
        k.iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Deliver `pass` on a link that reports `host` as the key it proved; what the client kept.
    fn kept(host: Option<[u8; 32]>, pass: &KeepPass) -> Vec<KeepPass> {
        let q = Arc::new(Mutex::new(VecDeque::new()));
        let host = host.map(|k| HostIdentity { name: "h".into(), pubkey: hex(&k), npub: "npub1x".into(), persona: None });
        q.lock().unwrap().push_back(ClientNetEvent::Connected { host });
        q.lock().unwrap().push_back(ClientNetEvent::Reliable(proto::encode(&ToClient::KeepPass(pass.clone()))));
        let mut c = ClientSession::new(Box::new(Fake { inq: q }), "Joiner", 0);
        c.update(0, InputFrame::default());
        c.take_keep_passes()
    }

    #[test]
    fn a_pass_is_kept_only_for_the_key_the_transport_proved() {
        let key = [0x11; 32];
        let pass = KeepPass { host_key: key, secret: [2; 32] };
        assert_eq!(kept(Some(key), &pass), vec![pass.clone()]);
        // No proven key at all (an in-process link): dropped, not stored.
        assert!(kept(None, &pass).is_empty());
        // The pass names another host than the one proved: dropped.
        assert!(kept(Some([0x22; 32]), &pass).is_empty());
        let mut near = key;
        near[31] ^= 1;
        assert!(kept(Some(near), &pass).is_empty(), "one bit off");
    }
}

/// The `ToClient` variants a client built before `KeepPass` knew (the first
/// thirteen, payloads kept the same shape), decoded the way it decodes.
#[allow(dead_code, clippy::large_enum_variant)]
#[derive(serde::Deserialize)]
enum OldToClient {
    Welcome { proto: u32, seat: u8 },
    Lobby(proto::LobbyView),
    Start { config: flock_core::MatchConfig, start_host_ms: u64, you: flock_core::PlayerId, run: u8 },
    Rows { run: u8, rows: Vec<flock_core::Row> },
    Events { run: u8, events: Vec<flock_core::Event> },
    PlaceResult { run: u8, req: u32, result: flock_core::PlaceResult },
    Results(crate::view::ResultsView),
    Rematch(proto::RematchMsg),
    Pong { id: u32, client_t: u64, host_t: u64 },
    Closed(String),
    Left { player: flock_core::PlayerId },
    Waiting,
    HostAttestation(String),
}

#[test]
fn a_keep_pass_is_an_appended_variant_that_changes_no_existing_message_and_needs_no_version_bump() {
    assert_eq!(PROTO_VERSION, 3, "a bump would make every older phone refuse the host; KeepPass is an appended variant");
    let pass = KeepPass { host_key: [0x5a; 32], secret: [0xa5; 32] };
    let bytes = proto::encode(&ToClient::KeepPass(pass.clone()));
    assert_eq!(bytes[0], 13);
    assert_eq!(bytes.len(), 1 + 32 + 32);
    assert!(proto::decode::<OldToClient>(&bytes).is_none(), "an older client fails to decode it and ignores it");
    assert_eq!(proto::decode::<ToClient>(&bytes), Some(ToClient::KeepPass(pass)));
    // Every existing message keeps its tag and its bytes.
    let enc = |m: ToClient| proto::encode(&m);
    assert_eq!(enc(ToClient::Welcome { proto: PROTO_VERSION, seat: 3 }), vec![0, 3, 3]);
    assert_eq!(enc(ToClient::Pong { id: 1, client_t: 2, host_t: 3 }), vec![8, 1, 2, 3]);
    assert_eq!(enc(ToClient::Closed("x".into())), vec![9, 1, b'x']);
    assert_eq!(enc(ToClient::Left { player: flock_core::PlayerId(4) })[0], 10);
    assert_eq!(enc(ToClient::Waiting), vec![11]);
    assert_eq!(enc(ToClient::HostAttestation("y".into())), vec![12, 1, b'y']);
    // The messages it does know still decode the same way.
    let welcome = enc(ToClient::Welcome { proto: PROTO_VERSION, seat: 3 });
    assert!(matches!(proto::decode::<OldToClient>(&welcome), Some(OldToClient::Welcome { seat: 3, .. })));
    // The secret never reaches a log line.
    let shown = format!("{:?}", ToClient::KeepPass(KeepPass { host_key: [1; 32], secret: [0xab; 32] }));
    assert!(!shown.contains(&"ab".repeat(32)) && !shown.contains("171"));
    assert!(shown.contains("<hidden>"));
}

/// End the round the way `--smoke` does, but at once: everyone becomes a ghost.
fn end_round_when_running(host: &mut HostAuthority) {
    if host.phase() == HostPhase::Running && host.world().is_some_and(|w| w.phase() == Phase::Running) {
        let w = host.world_mut().expect("world");
        let ids: Vec<_> = w.players().iter().map(|p| p.id()).collect();
        for id in ids {
            w.force_eliminate(id);
        }
    }
}

#[test]
fn the_headless_client_completes_a_round_against_a_gated_host_with_no_player_seat() {
    let (gate, seen) = recording_gate(|join| match join.admitted_by {
        Admission::Invite(_) => Ok(()),
        _ => Err("invites only".into()),
    });
    let mut rig = Rig::new(Some(gate), 2);
    let (secret, invite) = rig.invite(1);
    let keep_key = rig.keep_key();
    let link = crate::invite::format_invite_link(&keep_key, &secret);
    let (host_key, parsed) = crate::invite::parse_invite_link(&link).expect("link");
    assert_eq!((host_key, parsed), (keep_key, secret));
    let cfg = ProbeConfig { players: 2, ticks: 0, timeout: Duration::from_secs(60) };
    let (net_cfg, hub) = (rig.cfg.clone(), rig.hub.clone());
    let report = run_clients(
        &cfg,
        |id, _| net::join_invite_with_relay(&net_cfg, id, &parsed, host_key, Box::new(hub.client())),
        |now| {
            rig.host.update(now);
            end_round_when_running(&mut rig.host);
        },
    )
    .expect("a round completes");
    assert_eq!(report.end, ProbeEnd::RoundCompleted);
    assert_eq!(report.seated, 2);
    assert!(report.with_results >= 1);
    assert!(report.json_line().starts_with("{\"event\":\"join\",\"completed\":true,\"end\":\"round\""));
    assert_eq!(seen.lock().unwrap().iter().filter(|j| j.admitted_by == Admission::Invite(invite)).count(), 2);
    assert_eq!(rig.host.rounds(), 1);
}

#[test]
fn the_headless_client_can_stop_after_a_number_of_ticks() {
    let mut rig = Rig::new(None, 1);
    let (secret, _) = rig.invite(1);
    let keep_key = rig.keep_key();
    let cfg = ProbeConfig { players: 1, ticks: 30, timeout: Duration::from_secs(60) };
    let (net_cfg, hub) = (rig.cfg.clone(), rig.hub.clone());
    let report = run_clients(
        &cfg,
        |id, _| net::join_invite_with_relay(&net_cfg, id, &secret, keep_key, Box::new(hub.client())),
        |now| rig.host.update(now),
    )
    .expect("ticks arrive");
    assert_eq!(report.end, ProbeEnd::TicksReceived);
    assert!(report.ticks >= 30 && report.snapshots > 0);
}

#[test]
fn the_headless_client_reports_a_refusal_and_a_timeout() {
    // Refused: the gate turns the invite away; the client sees its link end.
    let (gate, _) = recording_gate(|_| Err("full".into()));
    let mut rig = Rig::new(Some(gate), 1);
    let (secret, _) = rig.invite(1);
    let keep_key = rig.keep_key();
    let (net_cfg, hub) = (rig.cfg.clone(), rig.hub.clone());
    let cfg = ProbeConfig { players: 1, ticks: 0, timeout: Duration::from_secs(30) };
    let result = run_clients(
        &cfg,
        |id, _| net::join_invite_with_relay(&net_cfg, id, &secret, keep_key, Box::new(hub.client())),
        |now| rig.host.update(now),
    );
    assert!(matches!(result, Err(ProbeError::Refused(_))), "{result:?}");
    drop(rig);
    // Timed out: seated and ready, but a lone player with no bots cannot start.
    let mut rig = Rig::new(None, 0);
    let (secret, _) = rig.invite(1);
    let keep_key = rig.keep_key();
    let (net_cfg, hub) = (rig.cfg.clone(), rig.hub.clone());
    let cfg = ProbeConfig { players: 1, ticks: 0, timeout: Duration::from_secs(4) };
    let result = run_clients(
        &cfg,
        |id, _| net::join_invite_with_relay(&net_cfg, id, &secret, keep_key, Box::new(hub.client())),
        |now| rig.host.update(now),
    );
    assert!(matches!(result, Err(ProbeError::TimedOut(_))), "{result:?}");
}
