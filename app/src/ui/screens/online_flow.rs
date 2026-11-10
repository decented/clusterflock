//! How the multiplayer screens open sessions and hand over to the match.
//!
//! * One seam for opening sessions (`open_host`, `open_join_code`,
//!   `open_nearby`, `open_bluetooth`, `radio`): production code calls the real
//!   `OnlineSession` constructors (which need My Signet's approval); tests
//!   install closures that build loopback / fake-radio sessions instead.
//! * The join-link stash the engine fills from `gameflock://join#CODE`.
//! * The hand-off to the match: `start_run` builds the run screen
//!   (`RunScreen::online`) on a [`SlotDriver`] and `show_results` the results
//!   table with Rematch / Leave. `// HOOK:` marks the two small places that
//!   depend on the gameplay screens' constructors.

use super::results::{ResultsLabels, ResultsScreen};
use super::run::RunScreen;
use super::{Ctx, Screen, Transition};
use crate::online::{self, OnlineSession};
use crate::platform::ble::{self, Ble, BleSeen};
use flock_core::{InputFrame, PlayerId, Tool};
use flock_session::net::LanRoom;
use flock_session::{ClientPhase, FlockView, ResultsView, RunDriver};
use std::sync::{Arc, Mutex};

/// The screen the whole flow returns to.
pub const MENU: &str = "multiplayer";

// ─── join-link stash ────────────────────────────────────────────────────────

static PENDING_JOIN: Mutex<Option<String>> = Mutex::new(None);

/// A `gameflock://join#CODE` link arrived: remember its code for the join screen.
pub fn stash_join(code: String) {
    *PENDING_JOIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(code);
}

/// The stashed join code, once.
pub fn take_join() -> Option<String> {
    PENDING_JOIN.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Is a join code waiting?
pub fn join_waiting() -> bool {
    PENDING_JOIN.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

// ─── the seam ───────────────────────────────────────────────────────────────

#[cfg(test)]
pub mod seam {
    use super::*;
    use std::cell::RefCell;

    /// What tests replace.
    #[derive(Default)]
    pub struct Seam {
        pub host: Option<Box<dyn FnMut() -> Result<OnlineSession, String>>>,
        pub join_code: Option<Box<dyn FnMut(&str) -> Result<OnlineSession, String>>>,
        pub bluetooth_join: Option<Box<dyn FnMut(&BleSeen) -> Result<OnlineSession, String>>>,
        pub start_bluetooth: Option<Box<dyn FnMut(&mut OnlineSession, u8) -> Result<(), String>>>,
        pub radio: Option<Arc<dyn Ble>>,
        /// Codes joined (in order), for assertions.
        pub joined: Vec<String>,
    }

    thread_local! {
        pub static SEAM: RefCell<Seam> = RefCell::new(Seam::default());
    }

    pub fn reset() {
        SEAM.with(|s| *s.borrow_mut() = Seam::default());
    }
}

/// The Bluetooth radio the screens talk to.
pub fn radio() -> Arc<dyn Ble> {
    #[cfg(test)]
    if let Some(r) = seam::SEAM.with(|s| s.borrow().radio.clone()) {
        return r;
    }
    ble::get()
}

/// Open a room (code + Nearby at once) and make it the app's session.
pub fn open_host(ctx: &mut Ctx) -> Result<(), String> {
    #[cfg(test)]
    if let Some(mut f) = seam::SEAM.with(|s| s.borrow_mut().host.take()) {
        let res = f();
        seam::SEAM.with(|s| s.borrow_mut().host = Some(f));
        return res.map(online::put);
    }
    OnlineSession::host(ctx.data_dir, &ctx.profile.name, ctx.profile.preferred_colour()).map(online::put)
}

/// Join by room code.
pub fn open_join_code(ctx: &mut Ctx, code: &str) -> Result<(), String> {
    #[cfg(test)]
    {
        seam::SEAM.with(|s| s.borrow_mut().joined.push(code.to_string()));
        if let Some(mut f) = seam::SEAM.with(|s| s.borrow_mut().join_code.take()) {
            let res = f(code);
            seam::SEAM.with(|s| s.borrow_mut().join_code = Some(f));
            return res.map(online::put);
        }
    }
    OnlineSession::join_code(ctx.data_dir, code, &ctx.profile.name, ctx.profile.preferred_colour()).map(online::put)
}

/// Join a room heard on the LAN.
pub fn open_nearby(ctx: &mut Ctx, room: &LanRoom) -> Result<(), String> {
    OnlineSession::join_nearby(ctx.data_dir, room, &ctx.profile.name, ctx.profile.preferred_colour()).map(online::put)
}

/// Join a Bluetooth game found by the scan.
pub fn open_bluetooth(ctx: &mut Ctx, seen: &BleSeen) -> Result<(), String> {
    #[cfg(test)]
    if let Some(mut f) = seam::SEAM.with(|s| s.borrow_mut().bluetooth_join.take()) {
        let res = f(seen);
        seam::SEAM.with(|s| s.borrow_mut().bluetooth_join = Some(f));
        return res.map(online::put);
    }
    OnlineSession::join_bluetooth_with(radio(), ctx.data_dir, seen, &ctx.profile.name, ctx.profile.preferred_colour()).map(online::put)
}

/// Open the hosting session's Bluetooth door.
pub fn start_bluetooth(s: &mut OnlineSession, colour: u8) -> Result<(), String> {
    #[cfg(test)]
    if let Some(mut f) = seam::SEAM.with(|c| c.borrow_mut().start_bluetooth.take()) {
        let res = f(s, colour);
        seam::SEAM.with(|c| c.borrow_mut().start_bluetooth = Some(f));
        return res;
    }
    s.start_bluetooth_with(radio(), colour)
}

/// Back to the multiplayer menu: leave the room, say why.
pub fn leave_to_menu(ctx: &mut Ctx, why: Option<&str>) -> Transition {
    online::leave();
    if let Some(w) = why {
        ctx.say(w);
    }
    Transition::PopTo(MENU)
}

// ─── hand-off to the match ──────────────────────────────────────────────────

/// The online session as the run screen's `Box<dyn RunDriver>`. The session
/// lives in the process slot (the engine keeps pumping it and leaves it when
/// My Signet approval ends), so the driver forwards to it and keeps its own
/// copy of the frame's view and results for `view()` / `results()` to borrow.
#[derive(Default)]
pub struct SlotDriver {
    view: Option<FlockView>,
    results: Option<ResultsView>,
}

impl RunDriver for SlotDriver {
    fn update(&mut self, _now_ms: u64, input: InputFrame) {
        let got = online::with(|s| {
            s.pump(input);
            (s.me().view().cloned(), s.results().cloned())
        });
        if let Some((v, r)) = got {
            self.view = v;
            self.results = r;
        }
    }
    fn place(&mut self, tool: Tool, lane: u8, row: u32) -> u32 {
        online::with(|s| s.driver().place(tool, lane, row)).unwrap_or(0)
    }
    fn view(&self) -> Option<&FlockView> {
        self.view.as_ref()
    }
    fn results(&self) -> Option<&ResultsView> {
        self.results.as_ref()
    }
    fn set_paused(&mut self, _p: bool) {}
}

/// The match has started (phase `Running` with a view): swap the lobby /
/// results screen for the run screen on the online session.
///
/// HOOK: `RunScreen::online(driver, on_over)` is the gameplay code's
/// constructor. Leaving mid-match (its Back pressed twice) only pops; the
/// multiplayer menu leaves the room when it shows up again, so the session
/// never outlives its screens.
pub fn start_run() -> Transition {
    Transition::Replace(Box::new(RunScreen::online(
        Box::new(SlotDriver::default()),
        Box::new(|results, local, _ctx| Box::new(OnlineResults::new(results.clone(), local))),
    )))
}

/// The match is over (phase `Results`) and no run screen handed over (a lobby
/// that finds the room already at its results): the results table now.
pub fn show_results() -> Transition {
    let got = online::with(|s| {
        let local = s.me().view().map(|v| v.local).or_else(|| s.my_seat().and_then(|id| s.lobby().and_then(|l| l.seat(id)).and_then(|x| x.player)));
        (s.results().cloned(), local)
    });
    match got {
        Some((Some(r), local)) => Transition::Replace(Box::new(OnlineResults::new(r, local.unwrap_or(PlayerId(0))))),
        _ => Transition::None,
    }
}

/// What a frame of the session said, for the lobby and the results.
pub enum Next {
    Dead(String),
    Run,
    Results,
    Lobby,
    Stay,
}

/// Pump the session one frame and say where it stands.
pub fn pump(input: InputFrame) -> Next {
    online::with(|s| {
        s.pump(input);
        if let Some(why) = s.dead_reason() {
            return Next::Dead(why);
        }
        match s.phase() {
            ClientPhase::Running if s.me().view().is_some() => Next::Run,
            ClientPhase::Results => Next::Results,
            ClientPhase::Lobby => Next::Lobby,
            _ => Next::Stay,
        }
    })
    .unwrap_or_else(|| Next::Dead("The game ended.".into()))
}

// ─── the online results ─────────────────────────────────────────────────────

/// The results table (`ResultsScreen`) wired to the online session: Rematch
/// asks the room for another round (the shared 3-2-1 shows on the table and
/// the next round starts the run screen again), Leave goes back to the menu,
/// and the room ending (host left, link lost) goes back with the reason.
pub struct OnlineResults {
    inner: ResultsScreen,
    local: PlayerId,
    /// The people in the match (rematch ticks).
    humans: Vec<PlayerId>,
    pressed: std::rc::Rc<std::cell::Cell<bool>>,
}

impl OnlineResults {
    pub fn new(results: ResultsView, local: PlayerId) -> OnlineResults {
        let pressed = std::rc::Rc::new(std::cell::Cell::new(false));
        let p2 = pressed.clone();
        let again: super::results::Action = Box::new(move |ctx: &mut Ctx| {
            if !p2.replace(true) {
                online::with(|s| s.set_rematch_ready(true));
                ctx.say("Waiting for the others to be ready");
            }
            Transition::None
        });
        let leave: super::results::Action = Box::new(|ctx: &mut Ctx| leave_to_menu(ctx, None));
        let labels = ResultsLabels { again: "Rematch".into(), again_sub: None, leave: "Leave".into() };
        let humans = results.rows.iter().filter(|r| !r.bot && !r.disconnected).map(|r| r.player).collect();
        OnlineResults { inner: ResultsScreen::new(results, local, labels, again, leave), local, humans, pressed }
    }
}

impl Screen for OnlineResults {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn on_background(&mut self, ctx: &mut Ctx) {
        self.inner.on_background(ctx);
    }
    fn back(&mut self, ctx: &mut Ctx) -> Transition {
        self.inner.back(ctx)
    }
    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        match pump(InputFrame::default()) {
            Next::Dead(why) => return leave_to_menu(ctx, Some(&why)),
            Next::Run => return start_run(),
            Next::Lobby => return Transition::Replace(Box::new(super::lobby::LobbyScreen::new())),
            Next::Results | Next::Stay => {}
        }
        let left = online::with(|s| s.rematch_ms_left()).flatten();
        // Who is ready is only known for us; the others show as waiting.
        let ready = self.humans.iter().map(|p| (*p, *p == self.local && self.pressed.get())).collect();
        self.inner.set_rematch(ready, left.map(|ms| ms as u32));
        self.inner.update(ctx)
    }
    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        self.inner.draw(ctx, ui)
    }
    fn clear_color(&self, ctx: &Ctx) -> [f32; 4] {
        self.inner.clear_color(ctx)
    }
}
