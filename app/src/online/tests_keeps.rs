//! Keeps in the app: a join by invite link against an in-process Keep (real QUIC
//! over loopback and Fray's in-memory relay, a host with no player seat behind
//! an admission gate), the pass rule, My Keeps through the engine, and what a
//! launch link may and may not do. Modelled on Stampede's and Twang Thang's
//! Keep tests. Nothing here touches a public relay.

use super::*;
use crate::cli::Options;
use crate::engine::Engine;
use crate::haptics::NoHaptics;
use crate::keeps;
use crate::platform::data_dir::Profile;
use crate::platform::os::RecordingPlatform;
use crate::platform::TouchPhase;
use crate::ui::screens::online_flow::seam;
use crate::ui::screens::{self, lobby::LobbyScreen, multiplayer::MultiplayerScreen, Screen};
use crate::ui::widgets;
use decented_net::relay::FakeRelayHub;
use decented_net::{Listener, ListenerConfig, RoomInfo};
use flock_session::net::{Admission, AdmissionGate, InviteId};
use flock_session::KeepPass;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const KEEP_ROOM: &str = "Keep room";

/// The process's one online slot: every test here holds it, and leaves it clean.
struct SlotGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        leave();
        seam::reset();
    }
}

fn slot_lock() -> SlotGuard {
    let g = TEST_SLOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    leave();
    seam::reset();
    SlotGuard(g)
}

fn test_dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("gameflock-keeps-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn hex(k: &[u8; 32]) -> String {
    k.iter().map(|b| format!("{b:02x}")).collect()
}

fn link(host_key: &[u8; 32], secret: &[u8; 32]) -> String {
    flock_session::format_invite_link(host_key, secret)
}

/// A runtime identity with a valid persona attestation (the persona check passes).
fn approved() -> Identity {
    let persona = nostr::Keys::generate();
    net::sign_identity(Identity::ephemeral(), &persona).expect("signing")
}

// ─── An in-process Keep ─────────────────────────────────────────────────────

/// A Keep's room: a Fray listener, an in-memory relay, a host with no player
/// seat and a gate that lets only invite joiners in.
struct KeepRig {
    listener: Listener,
    hub: FakeRelayHub,
    keep: Identity,
    cfg: NetConfig,
    link: HostLink<QuicHost>,
    host: HostAuthority,
    start: Instant,
    dir: PathBuf,
}

impl KeepRig {
    /// `by_code`: the gate admits room-code joiners too (a Keep that lets in by code).
    fn new(tag: &str, bots: usize, by_code: bool) -> KeepRig {
        let dir = test_dir(&format!("rig-{tag}"));
        let listener = Listener::bind(ListenerConfig { stun: Vec::new(), upnp: false, ..ListenerConfig::new() }).expect("bind");
        let hub = FakeRelayHub::new();
        let keep = net::test_identity();
        let mut cfg = net::net_config(&dir);
        cfg.relays = vec!["wss://relay.invalid".into()];
        cfg.relay_fallback = false;
        cfg.stun = Vec::new();
        cfg.lan_beacon = false;
        cfg.host_has_player = false;
        let handle = listener.host_with_relay(&cfg, &keep, RoomInfo { name: KEEP_ROOM.into(), max_players: 6 }, Some(Box::new(hub.client())));
        let mut quic = QuicHost::new(handle);
        // The Keep's rule: an invite gets in (and a room code only when `by_code`).
        let gate: AdmissionGate = Box::new(move |join| match join.admitted_by {
            Admission::Invite(_) => Ok(()),
            _ if by_code => Ok(()),
            other => Err(format!("not by {other:?}")),
        });
        quic.set_admission_gate(gate);
        let started = Instant::now();
        while !quic.is_ready() {
            use flock_session::HostTransport;
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
        KeepRig { listener, hub, keep, cfg, link, host, start: Instant::now(), dir }
    }

    fn keep_key(&self) -> [u8; 32] {
        self.keep.pubkey().to_bytes()
    }

    /// Register an invite for this room.
    fn invite(&self, n: u8) -> [u8; 32] {
        let mut s = [0x5a; 32];
        s[0] = n;
        let id = self.listener.add_invite(net::APP_TAG, s).expect("add_invite");
        assert_eq!(id, InviteId::of(&s));
        self.link.with_lan(|q| q.handle().expect("handle").set_invite_target(true)).expect("target");
        std::thread::sleep(Duration::from_millis(400));
        s
    }

    /// A phone joining the Keep by its invite link, as the app does.
    fn join_invite(&self, secret: &[u8; 32], name: &str) -> OnlineSession {
        let link = net::join_invite_with_relay(&self.cfg, &approved(), secret, self.keep_key(), Box::new(self.hub.client()));
        OnlineSession::keep_client(link, name, 0)
    }

    /// A phone joining the room by its code, as for any phone host.
    fn join_code(&self, name: &str) -> OnlineSession {
        let code = self.link.with_lan(|q| q.code().expect("code").to_string());
        let handle = decented_net::join_with_relay(&self.cfg, &approved(), &code, Box::new(self.hub.client()));
        OnlineSession::network_client(QuicClient::new(handle), name, 0, JoinedBy::Code)
    }

    fn now(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn step(&mut self, sessions: &mut [&mut OnlineSession]) {
        self.host.update(self.now());
        for s in sessions.iter_mut() {
            s.pump(InputFrame::default());
            s.end_frame();
        }
        std::thread::sleep(Duration::from_millis(3));
    }

    fn run_until(&mut self, sessions: &mut [&mut OnlineSession], secs: u64, mut done: impl FnMut(&mut KeepRig, &mut [&mut OnlineSession]) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            self.step(sessions);
            if done(self, sessions) {
                return true;
            }
        }
        false
    }

    fn peer_of(&self, s: &OnlineSession) -> flock_session::PeerKey {
        self.host.peer_of_seat(s.my_seat().expect("seated")).expect("peer")
    }
}

impl Drop for KeepRig {
    fn drop(&mut self) {
        self.host.shutdown();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn in_lobby(s: &OnlineSession) -> bool {
    *s.phase() == ClientPhase::Lobby && s.lobby().is_some()
}

// ─── Joining by invite ──────────────────────────────────────────────────────

#[test]
fn a_join_by_invite_reaches_play_on_a_host_with_no_player_seat() {
    let _slot = slot_lock();
    let mut rig = KeepRig::new("play", 1, false);
    let secret = rig.invite(1);
    let mut guest = rig.join_invite(&secret, "Ada");
    assert_eq!(guest.joined_by(), JoinedBy::Keep);
    assert!(rig.run_until(&mut [&mut guest], 20, |_, s| in_lobby(s[0])), "seated: {:?}", guest.phase());
    // The Keep's room has no host player and the phone is never the authority.
    assert!(!guest.is_host());
    assert!(guest.authority().is_none());
    assert!(guest.lobby().unwrap().seats.iter().all(|s| !s.host), "no seat is the host's");
    assert!(guest.lobby().unwrap().host_seat().is_none());
    assert!(!guest.can_start(), "a guest cannot start the match");
    assert!(!guest.add_bot() && !guest.start() && !guest.kick(0), "host-only actions do nothing for a guest of a Keep");
    assert_eq!(guest.host_name().as_deref(), Some(KEEP_ROOM), "the room's name comes from the link, not from a host seat");
    assert_eq!(rig.host.human_count(), 1);
    // Ready: the Keep starts the match (one person and its bot) and the guest is in play.
    guest.set_ready(true);
    assert!(
        rig.run_until(&mut [&mut guest], 20, |_, s| *s[0].phase() == ClientPhase::Running && s[0].me().view().is_some()),
        "in play: {:?}",
        guest.phase()
    );
    assert!(guest.driver().view().is_some());
    guest.leave();
}

#[test]
fn a_keep_guest_sees_no_host_only_control_in_the_lobby() {
    let _slot = slot_lock();
    let mut rig = KeepRig::new("lobbyui", 1, false);
    let secret = rig.invite(1);
    let mut guest = rig.join_invite(&secret, "Ada");
    assert!(rig.run_until(&mut [&mut guest], 20, |_, s| in_lobby(s[0])));
    put(guest);
    let mut e = engine(Box::new(LobbyScreen::new()), test_dir("lobbyui-app"), RecordingPlatform::default());
    let mut salts: Vec<String> = Vec::new();
    for _ in 0..90 {
        widgets::BUTTON_RECTS.with(|v| *v.borrow_mut() = Some(Vec::new()));
        rig.host.update(rig.now());
        e.frame(1.0 / 60.0, (1080, 2400), 2.625, egui::RawInput::default());
        salts.extend(widgets::BUTTON_RECTS.with(|v| v.borrow_mut().take()).unwrap().into_iter().map(|(s, ..)| s));
        std::thread::sleep(Duration::from_millis(3));
    }
    assert!(salts.iter().any(|s| s == "lobby-ready"), "the guest can ready up: {salts:?}");
    for host_only in ["lobby-start", "lobby-copy", "lobby-link", "lobby-share"] {
        assert!(!salts.iter().any(|s| s == host_only), "{host_only} is for the phone that hosts: {salts:?}");
    }
    assert_eq!(e.router.names(), ["lobby"]);
}

// ─── Passes ─────────────────────────────────────────────────────────────────

fn engine(first: Box<dyn Screen>, dir: PathBuf, platform: RecordingPlatform) -> Engine {
    Engine::new(first, Profile::default(), dir, Options { shot: Some("test".into()), ..Default::default() }, Box::new(platform), Box::new(NoHaptics::default()))
}

fn frames(e: &mut Engine, n: usize) {
    for _ in 0..n {
        e.frame(1.0 / 60.0, (1080, 2400), 2.625, egui::RawInput::default());
    }
}

/// Run the host and the engine's frames together for `secs`, stopping early when `done`.
fn run_engine_until(rig: &mut KeepRig, e: &mut Engine, secs: u64, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        rig.host.update(rig.now());
        frames(e, 1);
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    false
}

#[test]
fn a_pass_from_the_keep_updates_the_saved_keep_and_other_passes_are_not_believed() {
    let _slot = slot_lock();
    let mut rig = KeepRig::new("pass", 1, false);
    let secret = rig.invite(1);
    let keep_key = rig.keep_key();
    let dir = test_dir("pass-app");
    // The Keep is saved already, with an old secret and name.
    keeps::record_join(&dir, &keep_key, &[7; 32], Some("Old name"), 1000).unwrap();
    let mut guest = rig.join_invite(&secret, "Ada");
    assert!(rig.run_until(&mut [&mut guest], 20, |_, s| in_lobby(s[0])));
    let peer = rig.peer_of(&guest);
    put(guest);
    let mut e = engine(Box::new(LobbyScreen::new()), dir.clone(), RecordingPlatform::default());
    // A pass naming another Keep, then the Keep's own with a new secret: messages arrive in order,
    // so seeing the second saved shows the first was delivered and not believed.
    assert!(rig.host.send_keep_pass(peer, KeepPass { host_key: [9; 32], secret: [3; 32] }));
    assert!(rig.host.send_keep_pass(peer, KeepPass { host_key: keep_key, secret: [8; 32] }));
    let saved = run_engine_until(&mut rig, &mut e, 20, || keeps::load(&dir).first().is_some_and(|k| k.secret == hex(&[8; 32])));
    assert!(saved, "the Keep's own pass was saved: {:?}", keeps::load(&dir));
    let list = keeps::load(&dir);
    assert_eq!(list.len(), 1, "the other Keep's pass saved nothing: {list:?}");
    assert_eq!(list[0].host_key, hex(&keep_key));
    assert_eq!(list[0].name, KEEP_ROOM, "named by the host the link proved");
    assert!(list[0].last_joined > 1000);
    assert_eq!(list[0].keys(), Some((keep_key, [8; 32])));
}

#[test]
fn a_room_code_joiner_saves_the_keep_that_names_itself_and_ignores_a_pass_naming_another_key() {
    let _slot = slot_lock();
    // A Keep that lets players in by room code too: it hands the code joiner a pass.
    let mut rig = KeepRig::new("code", 1, true);
    let keep_key = rig.keep_key();
    let dir = test_dir("code-app");
    let mut by_code = rig.join_code("Bo");
    assert!(rig.run_until(&mut [&mut by_code], 20, |_, s| in_lobby(s[0])));
    assert_eq!(by_code.joined_by(), JoinedBy::Code);
    let pc = rig.peer_of(&by_code);
    put(by_code);
    let mut e = engine(Box::new(LobbyScreen::new()), dir.clone(), RecordingPlatform::default());
    // Another key's pass (a hostile host naming a Keep it does not run), then the proven host's own.
    assert!(rig.host.send_keep_pass(pc, KeepPass { host_key: [9; 32], secret: [3; 32] }));
    assert!(rig.host.send_keep_pass(pc, KeepPass { host_key: keep_key, secret: [8; 32] }));
    assert!(run_engine_until(&mut rig, &mut e, 20, || !keeps::load(&dir).is_empty()), "the proven host's pass was saved");
    let list = keeps::load(&dir);
    assert_eq!(list.len(), 1, "the pass naming another key saved nothing: {list:?}");
    assert_eq!(list[0].keys(), Some((keep_key, [8; 32])));
    assert_eq!(list[0].name, KEEP_ROOM);
}

// ─── My Keeps through fray-keep ─────────────────────────────────────────────

#[test]
fn my_keeps_add_remove_and_order_come_from_fray_keep() {
    let dir = test_dir("list");
    keeps::record_join(&dir, &[1; 32], &[2; 32], Some("One"), 100).unwrap();
    keeps::record_join(&dir, &[3; 32], &[4; 32], Some("Two"), 200).unwrap();
    keeps::record_join(&dir, &[5; 32], &[6; 32], None, 150).unwrap();
    let names: Vec<String> = keeps::load(&dir).into_iter().map(|k| k.name).collect();
    assert_eq!(names, ["Two", "Keep 050505", "One"], "newest play first");
    // The same Keep again with a new secret replaces its entry and moves it to the top.
    keeps::record_join(&dir, &[1; 32], &[9; 32], Some("One"), 300).unwrap();
    let list = keeps::load(&dir);
    assert_eq!(list.len(), 3);
    assert_eq!(list[0].keys(), Some(([1; 32], [9; 32])));
    // Forget takes one out and is saved.
    keeps::forget_saved(&dir, &hex(&[3; 32])).unwrap();
    assert_eq!(keeps::load(&dir).len(), 2);
    assert!(!keeps::contains(&keeps::load(&dir), &hex(&[3; 32])));
    // At the cap the oldest goes (fray-keep's rule, unchanged).
    for i in 0..keeps::MAX_KEEPS as u8 + 2 {
        keeps::record_join(&dir, &[100 + i; 32], &[1; 32], None, 1000 + u64::from(i)).unwrap();
    }
    let list = keeps::load(&dir);
    assert_eq!(list.len(), keeps::MAX_KEEPS);
    assert_eq!(list[0].host_key, hex(&[100 + keeps::MAX_KEEPS as u8 + 1; 32]));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join(keeps::FILE)).unwrap().permissions().mode() & 0o777, 0o600, "the file holds secrets");
    }
    let _ = std::fs::remove_dir_all(dir);
}

// ─── Launch links, Paste invite and the question before connecting ──────────

const HOME_LINK_KEY: [u8; 32] = [0xab; 32];
const HOME_LINK_SECRET: [u8; 32] = [0x01; 32];

fn engine_with_link(tag: &str, launch: &str) -> (Engine, PathBuf) {
    let dir = test_dir(tag);
    let mut p = RecordingPlatform::default();
    p.launch.push_back(launch.to_string());
    (engine(Box::new(screens::home::HomeScreen::new()), dir.clone(), p), dir)
}

/// Tap the button `salt` of the screen on top: find its rectangle in a frame, press, release.
fn click(e: &mut Engine, salt: &str) {
    widgets::BUTTON_RECTS.with(|v| *v.borrow_mut() = Some(Vec::new()));
    frames(e, 1);
    let hit = widgets::BUTTON_RECTS.with(|v| v.borrow().as_ref().and_then(|v| v.iter().rev().find(|(s, ..)| s == salt).map(|(_, r, c)| (*r, *c))));
    widgets::BUTTON_RECTS.with(|v| *v.borrow_mut() = None);
    let (rect, clip) = hit.unwrap_or_else(|| panic!("no button {salt:?} on {:?}", e.router.names()));
    assert!(clip.contains(rect.center()), "button {salt:?} is outside its clip");
    let ppp = e.ppp() as f64;
    let px = (rect.center().x as f64 * ppp, rect.center().y as f64 * ppp);
    e.touch_event(1, TouchPhase::Started, px);
    frames(e, 1);
    e.touch_event(1, TouchPhase::Ended, px);
    frames(e, 1);
}

/// A seam that refuses every dial (no network) and records it.
fn no_network() {
    seam::SEAM.with(|s| {
        let mut s = s.borrow_mut();
        s.join_invite = Some(Box::new(|_, _| Err("test: no network".into())));
        s.join_code = Some(Box::new(|_| Err("test: no network".into())));
    });
}

fn invited() -> Vec<[u8; 32]> {
    seam::SEAM.with(|s| s.borrow().invited.clone())
}

#[test]
fn a_tapped_link_for_a_keep_not_played_on_asks_and_never_connects_by_itself() {
    let _slot = slot_lock();
    no_network();
    let (mut e, _dir) = engine_with_link("link-ask", &link(&HOME_LINK_KEY, &HOME_LINK_SECRET));
    frames(&mut e, 200);
    assert_eq!(e.router.names(), ["home", "multiplayer", "join"], "the link opened the join screen over the menu");
    assert!(invited().is_empty(), "nothing was dialled: the player has not agreed");
    // Not now: back to the menu, still nothing dialled.
    click(&mut e, "keep-confirm-left");
    frames(&mut e, 120);
    assert_eq!(e.router.names(), ["home", "multiplayer"]);
    assert!(invited().is_empty());
    // The same link again, and this time yes: now it connects, to that Keep.
    e.svc.platform = {
        let mut p = RecordingPlatform::default();
        p.launch.push_back(link(&HOME_LINK_KEY, &HOME_LINK_SECRET));
        Box::new(p)
    };
    frames(&mut e, 200);
    assert_eq!(e.router.names(), ["home", "multiplayer", "join"]);
    assert!(invited().is_empty());
    click(&mut e, "keep-confirm-right");
    assert_eq!(invited(), [HOME_LINK_KEY], "dialled after the player said yes");
}

#[test]
fn a_link_for_a_saved_keep_does_not_ask() {
    let _slot = slot_lock();
    no_network();
    let dir = test_dir("link-saved");
    keeps::record_join(&dir, &HOME_LINK_KEY, &[9; 32], Some("Cousin's Keep"), 100).unwrap();
    let mut p = RecordingPlatform::default();
    p.launch.push_back(link(&HOME_LINK_KEY, &HOME_LINK_SECRET));
    let mut e = engine(Box::new(screens::home::HomeScreen::new()), dir, p);
    frames(&mut e, 200);
    assert_eq!(e.router.names(), ["home", "multiplayer", "join"]);
    assert_eq!(invited(), [HOME_LINK_KEY], "a Keep the player has played on is joined at once");
}

#[test]
fn a_bad_keep_link_opens_the_join_screen_and_dials_nothing() {
    let _slot = slot_lock();
    no_network();
    for bad in ["gameflock://keep/abc/def", "gameflock://keep", "gameflock://keep/", &format!("{}/extra", link(&HOME_LINK_KEY, &HOME_LINK_SECRET))] {
        let (mut e, _dir) = engine_with_link("link-bad", bad);
        frames(&mut e, 200);
        assert_eq!(e.router.names(), ["home", "multiplayer", "join"], "{bad}");
        assert!(invited().is_empty(), "{bad}");
        crate::online::leave();
    }
}

#[test]
fn links_of_other_schemes_and_the_signin_return_open_nothing_and_a_room_link_still_joins() {
    let _slot = slot_lock();
    no_network();
    for other in ["stampede://keep/x", "https://example.com/keep", "gameflock://signin", "gameflock://other#ABC", ""] {
        let (mut e, _dir) = engine_with_link("link-other", other);
        frames(&mut e, 120);
        assert_eq!(e.router.names(), ["home"], "{other:?} opens nothing");
        assert!(invited().is_empty());
        assert!(seam::SEAM.with(|s| s.borrow().joined.is_empty()));
    }
    let (mut e, _dir) = engine_with_link("link-room", "gameflock://join#kwtqpm");
    frames(&mut e, 200);
    assert_eq!(e.router.names(), ["home", "multiplayer", "join"]);
    seam::SEAM.with(|s| assert_eq!(s.borrow().joined, ["KWTQPM"]));
    assert!(invited().is_empty());
}

#[test]
fn a_keep_link_in_a_room_is_refused_like_a_room_link() {
    let _slot = slot_lock();
    no_network();
    put(OnlineSession::offline_host("A", 0, HostConfig::new(1)));
    let (mut e, _dir) = engine_with_link("link-room-busy", &link(&HOME_LINK_KEY, &HOME_LINK_SECRET));
    frames(&mut e, 5);
    assert_eq!(e.router.names(), ["home"]);
    assert_eq!(e.svc.toast.as_ref().map(|t| t.text.as_str()), Some("Leave your current game to join another."));
    assert!(invited().is_empty());
}

#[test]
fn paste_invite_reads_the_clipboard_and_asks_before_dialling() {
    let _slot = slot_lock();
    no_network();
    let dir = test_dir("paste");
    let mut p = RecordingPlatform::default();
    p.clipboard = Some(format!("Join my Keep: {}", link(&HOME_LINK_KEY, &HOME_LINK_SECRET)));
    let mut e = engine(Box::new(MultiplayerScreen::new()), dir.clone(), p);
    frames(&mut e, 150);
    click(&mut e, "mp-paste-invite");
    frames(&mut e, 120);
    assert_eq!(e.router.names(), ["multiplayer", "join"]);
    assert!(invited().is_empty(), "pasting a link for a Keep not played on asks first");
    // No link on the clipboard: a message, no screen.
    let mut p = RecordingPlatform::default();
    p.clipboard = Some("KWTQPM".into());
    let mut e = engine(Box::new(MultiplayerScreen::new()), dir.clone(), p);
    frames(&mut e, 150);
    click(&mut e, "mp-paste-invite");
    assert_eq!(e.router.names(), ["multiplayer"]);
    assert_eq!(e.svc.toast.as_ref().map(|t| t.text.as_str()), Some(keeps::NO_INVITE_ON_CLIPBOARD));
    // A broken link: said so, no screen.
    let mut p = RecordingPlatform::default();
    p.clipboard = Some("gameflock://keep/nope".into());
    let mut e = engine(Box::new(MultiplayerScreen::new()), dir, p);
    frames(&mut e, 150);
    click(&mut e, "mp-paste-invite");
    assert_eq!(e.router.names(), ["multiplayer"]);
    assert_eq!(e.svc.toast.as_ref().map(|t| t.text.as_str()), Some(keeps::NOT_AN_INVITE));
    assert!(invited().is_empty());
}

#[test]
fn my_keeps_lists_the_saved_keeps_and_a_row_dials_the_secret_in_the_file() {
    let _slot = slot_lock();
    no_network();
    let dir = test_dir("mykeeps");
    keeps::record_join(&dir, &HOME_LINK_KEY, &[1; 32], Some("Cousin's Keep"), keeps::unix_now() - 100).unwrap();
    let mut e = engine(Box::new(MultiplayerScreen::new()), dir.clone(), RecordingPlatform::default());
    frames(&mut e, 150);
    click(&mut e, "mp-keeps");
    frames(&mut e, 120);
    assert_eq!(e.router.names(), ["multiplayer", "keeps"]);
    // Another screen joined the same Keep again with a newer secret meanwhile.
    keeps::record_join(&dir, &HOME_LINK_KEY, &[2; 32], Some("Cousin's Keep"), keeps::unix_now()).unwrap();
    click(&mut e, "keep-join-0");
    frames(&mut e, 120);
    assert_eq!(e.router.names(), ["multiplayer", "keeps", "join"]);
    assert_eq!(invited(), [HOME_LINK_KEY], "a saved Keep is joined without asking");
    // Forget: the first tap asks, the second removes.
    crate::online::leave();
    e.request_back();
    frames(&mut e, 120);
    assert_eq!(e.router.names(), ["multiplayer", "keeps"]);
    click(&mut e, "keep-forget-0");
    frames(&mut e, 3);
    assert_eq!(keeps::load(&dir).len(), 1, "one tap only asks");
    click(&mut e, "keep-sure-0");
    frames(&mut e, 3);
    assert!(keeps::load(&dir).is_empty());
}

// ─── Layout ─────────────────────────────────────────────────────────────────

/// Show `first` on a phone of `w` x `h` points (`top` points of notch) for a while and give back
/// the safe area and every button painted in the last frame.
fn painted_buttons(first: Box<dyn Screen>, w: f32, h: f32, top: f32, dir: PathBuf) -> (egui::Rect, Vec<(String, egui::Rect, egui::Rect)>) {
    use crate::platform::{Insets, Viewport};
    let insets = Insets { top, bottom: 24.0, left: 0.0, right: 0.0 };
    let mut e = Engine::new(
        first,
        Profile::default(),
        dir,
        Options { shot: Some("test".into()), insets, ..Default::default() },
        Box::new(RecordingPlatform::default()),
        Box::new(NoHaptics::default()),
    );
    let size = ((w * 2.0) as u32, (h * 2.0) as u32);
    for _ in 0..150 {
        e.frame(1.0 / 60.0, size, 2.0, egui::RawInput::default());
    }
    widgets::BUTTON_RECTS.with(|v| *v.borrow_mut() = Some(Vec::new()));
    e.frame(1.0 / 60.0, size, 2.0, egui::RawInput::default());
    let rects = widgets::BUTTON_RECTS.with(|v| v.borrow_mut().take()).unwrap();
    let ppp = e.ppp();
    let vp = Viewport::new(egui::vec2(size.0 as f32 / ppp, size.1 as f32 / ppp), ppp, insets);
    (vp.safe, rects)
}

const PHONES: [(f32, f32, f32); 3] = [(360.0, 800.0, 48.0), (412.0, 915.0, 48.0), (360.0, 740.0, 24.0)];

#[test]
fn the_multiplayer_menu_with_keeps_stays_inside_the_safe_area_and_apart() {
    let _slot = slot_lock();
    for (w, h, top) in PHONES {
        let (safe, rects) = painted_buttons(Box::new(MultiplayerScreen::sample()), w, h, top, test_dir("layout-mp"));
        let paste = rects.iter().find(|(s, ..)| s == "mp-paste-invite").unwrap_or_else(|| panic!("no Paste invite button {w}x{h}"));
        let keeps_btn = rects.iter().find(|(s, ..)| s == "mp-keeps").expect("My Keeps button");
        for (salt, r, _) in &rects {
            assert!(safe.contains_rect(*r), "{salt} {w}x{h}: {r:?} outside safe {safe:?}");
        }
        assert!(paste.1.height() >= 44.0 && keeps_btn.1.height() >= 44.0, "tap targets are at least 44 points");
        assert!(!paste.1.intersects(keeps_btn.1), "the two buttons do not overlap");
        // And they sit clear of the four ways in.
        for (salt, r, _) in rects.iter().filter(|(s, ..)| s.starts_with("mp-") && s.as_bytes().get(3).is_some_and(|c| c.is_ascii_digit())) {
            assert!(!r.intersects(paste.1) && !r.intersects(keeps_btn.1), "{salt} {w}x{h} overlaps the Keeps row");
        }
    }
}

#[test]
fn my_keeps_and_the_question_before_connecting_stay_inside_the_safe_area() {
    let _slot = slot_lock();
    for (w, h, top) in PHONES {
        let (safe, rects) = painted_buttons(Box::new(screens::keeps::KeepsScreen::sample()), w, h, top, test_dir("layout-keeps"));
        assert!(rects.iter().any(|(s, ..)| s == "keep-join-0") && rects.iter().any(|(s, ..)| s == "keep-sure-1"), "rows and the forget question are painted {w}x{h}");
        for (salt, r, _) in &rects {
            assert!(r.min.x >= safe.min.x && r.max.x <= safe.max.x, "{salt} {w}x{h}: {r:?} outside safe {safe:?} sideways");
        }
        // The question card of a Keep not played on: both buttons inside the screen and apart.
        let (_, rects) = painted_buttons(Box::new(screens::join::JoinScreen::confirm_sample()), w, h, top, test_dir("layout-confirm"));
        let no = rects.iter().find(|(s, ..)| s == "keep-confirm-left").expect("Not now");
        let yes = rects.iter().find(|(s, ..)| s == "keep-confirm-right").expect("Join");
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(w, h));
        assert!(screen.contains_rect(no.1) && screen.contains_rect(yes.1) && !no.1.intersects(yes.1), "{w}x{h}");
        // The failed-keep card offers Back, Forget and Try again side by side inside the screen.
        let (_, rects) = painted_buttons(Box::new(screens::join::JoinScreen::failed_keep_sample()), w, h, top, test_dir("layout-failed"));
        let names = ["join-fail-back", "join-forget", "join-retry"];
        let rs: Vec<_> = names.iter().map(|n| rects.iter().find(|(s, ..)| s == n).unwrap_or_else(|| panic!("{n}")).1).collect();
        for r in &rs {
            assert!(screen.contains_rect(*r), "{w}x{h} {r:?}");
        }
        assert!(!rs[0].intersects(rs[1]) && !rs[1].intersects(rs[2]));
    }
}
