//! The app glue: an offline-hosted room through to a running match, the
//! Bluetooth door over the in-memory radio (`ble_fake`) with the Accept gate
//! and the post-Accept host check, and the player-facing words.

use super::*;
use crate::platform::ble_fake::{FakeAir, FakeBle};
use std::time::{Duration, Instant};

/// The app's online slot is one per process: every test that puts a session in it takes this
/// lock first (and leaves the slot and the screen seam clean on both ends).
use super::TEST_SLOT_LOCK as SLOT_LOCK;

struct SlotGuard(#[allow(dead_code)] std::sync::MutexGuard<'static, ()>);

impl Drop for SlotGuard {
    fn drop(&mut self) {
        leave();
        crate::ui::screens::online_flow::seam::reset();
    }
}

fn slot_lock() -> SlotGuard {
    let g = SLOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    leave();
    crate::ui::screens::online_flow::seam::reset();
    SlotGuard(g)
}

fn pump_until(sessions: &mut [&mut OnlineSession], secs: u64, mut f: impl FnMut(&[&mut OnlineSession]) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        for s in sessions.iter_mut() {
            s.pump(InputFrame::default());
            s.end_frame();
        }
        if f(sessions) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn an_offline_room_with_a_bot_starts_a_match_the_run_screen_can_drive() {
    let mut s = OnlineSession::offline_host("Ann", 2, HostConfig::new(7));
    assert!(pump_until(&mut [&mut s], 5, |s| s[0].lobby().is_some()));
    assert!(s.is_host());
    assert!(!s.can_start(), "one seat is not enough");
    assert!(s.add_bot());
    assert!(pump_until(&mut [&mut s], 5, |s| s[0].lobby().is_some_and(|l| l.seats.len() == 2)));
    assert!(s.can_start());
    assert!(s.start());
    assert!(pump_until(&mut [&mut s], 5, |s| *s[0].phase() == ClientPhase::Running));
    let d = s.driver();
    d.update(0, InputFrame::default());
    let v = d.view().expect("a view");
    assert_eq!(v.names.len(), 2);
    assert!(v.online);
    assert!(d.results().is_none());
    s.leave();
    assert!(s.dead_reason().is_some());
}

#[test]
fn the_slot_holds_one_session_and_leave_empties_it() {
    let _slot = slot_lock();
    put(OnlineSession::offline_host("A", 0, HostConfig::new(1)));
    assert!(is_some());
    assert_eq!(with(|s| s.is_host()), Some(true));
    // Re-entrant use is refused, not deadlocked.
    assert_eq!(with(|_| with(|_| ())), Some(None));
    pump_auto();
    end_frame();
    leave();
    assert!(!is_some());
    assert!(with(|_| ()).is_none());
}

fn quiet_net(tag: &str) -> NetConfig {
    let dir = std::env::temp_dir().join(format!("flock-app-{tag}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let mut c = net::net_config(&dir);
    c.relays.clear();
    c.stun.clear();
    c.lan_beacon = false;
    c
}

/// Bluetooth end to end on the fake radio: the joiner sees the advert,
/// connects, waits for Accept (named by its verified persona), is seated
/// through the Bluetooth door, and then verifies the host's persona.
#[test]
fn bluetooth_door_on_the_fake_radio_needs_accept() {
    let air = FakeAir::new();
    let host_radio: Arc<dyn Ble> = Arc::new(FakeBle::new(air.clone()));
    let join_radio: Arc<dyn Ble> = Arc::new(FakeBle::new(air));
    let net_h = quiet_net("bt-host");
    let host_id = net::test_identity();
    let quic = net::host_room(&net_h, &host_id, "Ann's flock");
    let mut host = OnlineSession::network_host(net_h, host_id.clone(), quic, "Ann", 0, HostConfig::new(3));
    assert!(pump_until(&mut [&mut host], 10, |s| s[0].lobby().is_some()));
    host.start_bluetooth_with(host_radio, 0).expect("door opens");
    assert!(matches!(host.bluetooth(), BtStatus::On { .. }));

    join_radio.start_scan().unwrap();
    let seen = join_radio.seen();
    assert_eq!(seen.len(), 1);
    assert!(bluetooth_room_name(&seen[0]).starts_with("Bluetooth game"));
    let net_j = quiet_net("bt-join");
    let join_id = net::test_identity();
    let mut joiner = OnlineSession::bluetooth_client(join_radio, &net_j, &join_id, &seen[0], "Bo", 1).unwrap();
    assert_eq!(joiner.joined_by(), JoinedBy::Bluetooth);
    assert!(
        pump_until(&mut [&mut host, &mut joiner], 15, |s| !s[0].pending_joiners().is_empty() && *s[1].phase() == ClientPhase::Waiting),
        "joiner waits: {:?}",
        joiner.phase()
    );
    let p = host.pending_joiners()[0].clone();
    assert_eq!(p.name, "Bo");
    let (_, joiner_persona) = net::own_persona(&join_id).unwrap();
    assert_eq!(p.persona.as_deref(), Some(joiner_persona.as_str()));
    assert!(joiner.lobby().is_none(), "no lobby before Accept");
    assert!(host.accept(p.peer));
    assert!(pump_until(&mut [&mut host, &mut joiner], 15, |s| *s[1].phase() == ClientPhase::Lobby), "{:?}", joiner.phase());
    let (_, host_persona) = net::own_persona(&host_id).unwrap();
    assert_eq!(joiner.host_persona(), Some(host_persona.as_str()));
    assert!(pump_until(&mut [&mut host, &mut joiner], 5, |s| s[0].lobby().is_some_and(|l| l.seats.len() == 2)));
    let seat = host.lobby().unwrap().seats.iter().find(|s| !s.host).unwrap().clone();
    assert_eq!(seat.path, flock_session::SeatPath::Bluetooth);
    assert_eq!(seat.persona.as_deref(), Some(joiner_persona.as_str()));
    // Host leaves: the joiner is told.
    host.leave();
    assert!(pump_until(&mut [&mut joiner], 10, |s| s[0].host_left()), "{:?}", joiner.phase());
    assert_eq!(joiner.dead_reason().as_deref(), Some("The host left the game."));
}

#[test]
fn words() {
    assert_eq!(join_link("ABCDEF"), "gameflock://join#ABCDEF");
    assert_eq!(crate::platform::parse_join_link(&join_link("ABCDEF")), Some("ABCDEF".into()));
    assert_eq!(spaced_code("KWTQPM"), "KWT QPM");
    assert_eq!(dead_text(&ClientPhase::HostLeft).as_deref(), Some("The host left the game."));
    assert_eq!(friendly_failure("Room full. [Connecting]"), "Room full.");
    assert!(closed_text("lobby full").starts_with("Room full"));
    assert!(dead_text(&ClientPhase::Lobby).is_none());
    assert_eq!(quality(Some(50)), 0);
    assert_eq!(quality(Some(500)), 2);
    // The app tag that namespaces rooms and attestations is the account's app id.
    assert_eq!(flock_session::net::APP_TAG, crate::account::APP_ID);
}

// ─── the multiplayer screens, end to end ────────────────────────────────────
//
// Two real `Engine`s (router, egui, the screens) in one process, taking turns
// with the process's one online slot: each phone keeps its session in a stash
// and swaps it into the slot for its own frame. The screens open sessions
// through `online_flow`'s seam, which these tests point at a loopback hub (or
// the fake Bluetooth radio). Real clocks, so the loops sleep a little.

mod flow {
    use super::*;
    use crate::cli::Options;
    use crate::engine::Engine;
    use crate::haptics::NoHaptics;
    use crate::platform::data_dir::Profile;
    use crate::platform::os::RecordingPlatform;
    use crate::platform::TouchPhase;
    use crate::ui::screens::online_flow::seam;
    use crate::ui::screens::{bluetooth::BluetoothScreen, lobby::LobbyScreen, multiplayer::MultiplayerScreen, Screen};
    use crate::ui::widgets;
    use flock_core::{Phase, PlayerId};
    use flock_session::{LinkPath, LoopbackConfig, LoopbackHub};
    use std::rc::Rc;

    struct Phone {
        e: Engine,
        stash: Option<OnlineSession>,
        name: &'static str,
    }

    impl Phone {
        fn new(name: &'static str, first: Box<dyn Screen>) -> Phone {
            let dir = std::env::temp_dir().join(format!("gameflock-flow-{name}-{}", std::process::id()));
            let opts = Options { shot: Some("test".into()), ..Default::default() };
            let e = Engine::new(first, Profile::default(), dir, opts, Box::new(RecordingPlatform::default()), Box::new(NoHaptics::default()));
            Phone { e, stash: None, name }
        }

        fn frame(&mut self) {
            if let Some(s) = self.stash.take() {
                put(s);
            }
            self.e.frame(1.0 / 60.0, (1080, 2400), 2.625, egui::RawInput::default());
            self.stash = take();
        }

        fn top(&self) -> &'static str {
            self.e.screen_name()
        }

        fn names(&self) -> Vec<&'static str> {
            self.e.router.names()
        }

        fn s(&self) -> &OnlineSession {
            self.stash.as_ref().unwrap_or_else(|| panic!("{} has no session (screens: {:?})", self.name, self.names()))
        }

        fn toast(&self) -> Option<String> {
            self.e.svc.toast.as_ref().map(|t| t.text.clone())
        }

        /// Let the entry animations and fades play out.
        fn settle(&mut self) {
            for _ in 0..150 {
                self.frame();
            }
        }

        /// Tap the button `salt` of the screen on top: find its rectangle in a frame, press, release.
        fn click(&mut self, salt: &str) {
            widgets::BUTTON_RECTS.with(|v| *v.borrow_mut() = Some(Vec::new()));
            self.frame();
            let hit = widgets::BUTTON_RECTS.with(|v| v.borrow().as_ref().and_then(|v| v.iter().rev().find(|(s, ..)| s == salt).map(|(_, r, c)| (*r, *c))));
            widgets::BUTTON_RECTS.with(|v| *v.borrow_mut() = None);
            let (rect, clip) = hit.unwrap_or_else(|| panic!("{}: no button {salt:?} on {:?}", self.name, self.top()));
            assert!(clip.contains(rect.center()), "{}: button {salt:?} is outside its clip", self.name);
            let ppp = self.e.ppp() as f64;
            let px = (rect.center().x as f64 * ppp, rect.center().y as f64 * ppp);
            self.e.touch_event(1, TouchPhase::Started, px);
            self.frame();
            self.e.touch_event(1, TouchPhase::Ended, px);
            self.frame();
        }
    }

    fn run_until(a: &mut Phone, b: &mut Phone, secs: u64, mut done: impl FnMut(&Phone, &Phone) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            a.frame();
            b.frame();
            if done(a, b) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(4));
        }
        false
    }

    fn both_settle(a: &mut Phone, b: &mut Phone) {
        for _ in 0..150 {
            a.frame();
            b.frame();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Host and guest sessions on one loopback hub, opened by the menu cards and the join keypad.
    fn loopback_seam() -> Rc<LoopbackHub> {
        let hub = Rc::new(LoopbackHub::new(LoopClock::real(), 42));
        seam::SEAM.with(|s| {
            let mut s = s.borrow_mut();
            let h = hub.clone();
            s.host = Some(Box::new(move || {
                let (host, me) = host_with_local_player(Some(Box::new(h.host())), HostConfig::new(5), "Ann", 0);
                Ok(OnlineSession::wrap(me, Some(host), None, JoinedBy::Host))
            }));
            let h = hub.clone();
            s.join_code = Some(Box::new(move |code| {
                assert_eq!(code, "KWTQPM");
                let link = h.connect_as(LoopbackConfig::PERFECT, LinkPath::Direct, Some("npub1guest".into()));
                Ok(OnlineSession::wrap(ClientSession::new(Box::new(link), "Bo", 1), None, None, JoinedBy::Code))
            }));
        });
        hub
    }

    /// Both phones on the menu; the host opens a room, the guest types its code: both in the lobby.
    fn into_the_lobby(a: &mut Phone, b: &mut Phone) {
        a.settle();
        a.click("mp-0"); // Host a game
        a.settle();
        assert_eq!(a.names(), ["multiplayer", "lobby"]);
        assert!(a.stash.as_ref().is_some_and(|s| s.is_host()), "the room is in the app's slot");

        b.settle();
        b.click("mp-1"); // Join with code
        b.settle();
        assert_eq!(b.names(), ["multiplayer", "join"]);
        for c in "KWTQPM".chars() {
            b.click(&format!("join-key-{c}"));
        }
        b.click("join-go");
        assert!(
            run_until(a, b, 15, |a, b| b.top() == "lobby" && a.s().lobby().is_some_and(|l| l.seats.len() == 2)),
            "guest seated: host {:?}, guest {:?} / {:?}",
            a.stash.as_ref().map(|s| s.phase().clone()),
            b.stash.as_ref().map(|s| s.phase().clone()),
            b.names()
        );
        assert_eq!(b.names(), ["multiplayer", "lobby"]);
        seam::SEAM.with(|s| assert_eq!(s.borrow().joined, ["KWTQPM"]));
        assert_eq!(b.s().joined_by(), JoinedBy::Code);
    }

    fn running(p: &Phone) -> bool {
        p.top() == "run" && *p.s().phase() == ClientPhase::Running
    }

    #[test]
    fn menu_host_lobby_guest_joins_by_code_start_run_results_rematch() {
        let _slot = slot_lock();
        let _hub = loopback_seam();
        let mut a = Phone::new("host", Box::new(MultiplayerScreen::new()));
        let mut b = Phone::new("guest", Box::new(MultiplayerScreen::new()));
        into_the_lobby(&mut a, &mut b);
        both_settle(&mut a, &mut b);

        // The guest takes a free colour; the host sees it.
        let before = b.s().lobby().unwrap().seat(b.s().my_seat().unwrap()).unwrap().colour;
        b.stash.as_mut().unwrap().set_colour((before + 1) % 6);
        assert!(run_until(&mut a, &mut b, 5, |a, _| a.s().lobby().unwrap().seats.iter().any(|s| !s.host && s.colour == (before + 1) % 6)));

        // Start: the run screen opens on both phones.
        a.click("lobby-start");
        assert!(run_until(&mut a, &mut b, 20, |a, b| running(a) && running(b)), "run screens: {:?} / {:?}", a.names(), b.names());
        assert_eq!(a.names(), ["multiplayer", "run"]);
        assert_eq!(a.s().lobby().unwrap().seats.len(), 2);

        // The match is on; end it (both runners out), and the results come up on both.
        assert!(run_until(&mut a, &mut b, 20, |a, _| a.s().me().view().is_some_and(|v| v.phase == Phase::Running)));
        let ids: Vec<PlayerId> = a.s().me().view().unwrap().names.iter().map(|n| n.0).collect();
        {
            let host = a.stash.as_mut().unwrap().host.as_mut().unwrap();
            let w = host.world_mut().expect("the match runs");
            for id in &ids {
                w.force_eliminate(*id);
            }
        }
        assert!(
            run_until(&mut a, &mut b, 20, |a, b| a.top() == "results" && b.top() == "results" && *a.s().phase() == ClientPhase::Results && *b.s().phase() == ClientPhase::Results),
            "results: {:?} / {:?}",
            a.names(),
            b.names()
        );
        assert_eq!(a.s().results().unwrap().rows.len(), 2);

        // Rematch: both ask, the 3-2-1 runs, round 1 starts on the run screens again.
        both_settle(&mut a, &mut b);
        a.click("results-again");
        b.click("results-again");
        assert!(run_until(&mut a, &mut b, 25, |a, b| running(a) && running(b)), "rematch: {:?} / {:?}", a.names(), b.names());
        assert_eq!(a.s().me().view().unwrap().round, 1);
        assert_eq!(b.s().me().view().unwrap().round, 1);
    }

    #[test]
    fn the_host_leaving_sends_guests_back_to_the_menu_with_a_toast() {
        let _slot = slot_lock();
        let _hub = loopback_seam();
        let mut a = Phone::new("host", Box::new(MultiplayerScreen::new()));
        let mut b = Phone::new("guest", Box::new(MultiplayerScreen::new()));
        into_the_lobby(&mut a, &mut b);
        both_settle(&mut a, &mut b);
        // The host presses Back: the room closes.
        a.e.request_back();
        assert!(run_until(&mut a, &mut b, 10, |a, b| a.top() == "multiplayer" && b.top() == "multiplayer"), "{:?} / {:?}", a.names(), b.names());
        assert_eq!(b.toast().as_deref(), Some("The host left the game."));
        assert_eq!(a.toast().as_deref(), Some("Room closed"));
        assert!(b.stash.is_none() && a.stash.is_none(), "no room is left in either slot");
    }

    #[test]
    fn bluetooth_join_through_the_screens_waits_for_the_hosts_accept() {
        let _slot = slot_lock();
        let air = FakeAir::new();
        let host_radio: Arc<dyn Ble> = Arc::new(FakeBle::new(air.clone()));
        let join_radio: Arc<dyn Ble> = Arc::new(FakeBle::new(air));
        let net_h = quiet_net("flow-bt-host");
        let host_id = net::test_identity();
        let quic = net::host_room(&net_h, &host_id, "Ann's flock");
        let host = OnlineSession::network_host(net_h, host_id.clone(), quic, "Ann", 0, HostConfig::new(3));
        let net_j = quiet_net("flow-bt-join");
        let join_id = net::test_identity();
        let (jr, hr) = (join_radio.clone(), host_radio.clone());
        let jid = join_id.clone();
        seam::SEAM.with(|s| {
            let mut s = s.borrow_mut();
            s.radio = Some(jr.clone());
            s.start_bluetooth = Some(Box::new(move |sess, colour| sess.start_bluetooth_with(hr.clone(), colour)));
            s.bluetooth_join = Some(Box::new(move |seen| OnlineSession::bluetooth_client(jr.clone(), &net_j, &jid, seen, "Bo", 1)));
        });
        let mut a = Phone::new("host", Box::new(LobbyScreen::bluetooth_host()));
        a.stash = Some(host);
        let mut b = Phone::new("guest", Box::new(BluetoothScreen::new()));
        // The host's lobby opens the Bluetooth door; the guest's list finds it.
        assert!(run_until(&mut a, &mut b, 15, |a, _| matches!(a.s().bluetooth(), BtStatus::On { .. })), "door: {:?}", a.s().bluetooth());
        assert!(run_until(&mut a, &mut b, 10, |_, _| join_radio.seen().len() == 1));
        both_settle(&mut a, &mut b);
        b.click("bt-join-0");
        assert!(run_until(&mut a, &mut b, 5, |_, b| b.top() == "join"), "the connecting page: {:?}", b.names());
        // The joiner waits; the host is asked, naming the verified persona.
        assert!(
            run_until(&mut a, &mut b, 20, |a, b| !a.s().pending_joiners().is_empty() && b.stash.as_ref().is_some_and(|s| *s.phase() == ClientPhase::Waiting)),
            "waiting: {:?}",
            b.stash.as_ref().map(|s| s.phase().clone())
        );
        let (_, joiner_persona) = net::own_persona(&join_id).unwrap();
        assert_eq!(a.s().pending_joiners()[0].persona.as_deref(), Some(joiner_persona.as_str()));
        assert!(b.s().lobby().is_none(), "no lobby before Accept");
        // Accept on the host's screen.
        a.click("lobby-accept");
        assert!(run_until(&mut a, &mut b, 20, |a, b| b.top() == "lobby" && a.s().lobby().is_some_and(|l| l.seats.len() == 2)), "after accept: {:?}", b.names());
        let seat = a.s().lobby().unwrap().seats.iter().find(|s| !s.host).unwrap().clone();
        assert_eq!(seat.path, flock_session::SeatPath::Bluetooth);
        let (_, host_persona) = net::own_persona(&host_id).unwrap();
        assert_eq!(b.s().host_persona(), Some(host_persona.as_str()));
        assert!(a.s().pending_joiners().is_empty());
    }
}
