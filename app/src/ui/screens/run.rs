//! The Run screen: one screen for Solo, Flock Practice and online play. It
//! drives a `Box<dyn RunDriver>`, presents its `FlockView` in 3D, reads swipes
//! and keys, and shows the HUD, the countdown, the pause menu and (as a ghost)
//! the architect tools. When the match is over it hands the results to the
//! next screen (Solo game over, or a results table).

use super::results::{ResultsLabels, ResultsScreen};
use super::solo_over::{DailyOver, SoloOverScreen};
use super::{Ctx, Screen, Transition};
use crate::ui::game::callouts::Callouts;
use crate::ui::game::cameras::Projector;
use crate::ui::game::fx::{Fx, Signal};
use crate::ui::game::ghost_ui::{self, GhostUi};
use crate::ui::game::gesture::InputState;
use crate::ui::game::hints::Hints;
use crate::ui::game::impact::Impact;
use crate::ui::game::streak::{self, ClearTracker};
use crate::ui::game::hud::{self, Data, HudState, Mode, ToolView};
use crate::ui::game::presenter::Presenter;
use crate::ui::game::stage::{self, Draw};
use crate::ui::game::{debug, local};
use crate::render::ScenePass;
use crate::ui::art::{self, rgba};
use crate::ui::widgets::{self, Btn};
use crate::ui::{feedback, theme};
use egui::{pos2, vec2, Align2, Color32, Id, Rect, Sense, Stroke};
use flock_audio::MusicTrack;
use crate::ui::game::mood::{music_mood, Inputs as MoodInputs};
use crate::haptics::Pulse;
use flock_core::{BotSkill, Command, Event, Phase, PlayerId, Role, Tool};
use flock_render::FrameScene;
use flock_session::{FlockView, ResultsView, RunDriver};
use std::collections::VecDeque;

/// Seconds the finished run stays on screen before the results appear.
const OVER_HOLD_SOLO: f32 = 1.5;
const OVER_HOLD_MULTI: f32 = 1.2;
/// Seconds the ghost instruction shows.
const INSTRUCTION_SECS: f32 = 6.0;
/// Frames of scene history kept in shot mode (particles need a past).
const HISTORY: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RunKind {
    Solo,
    Practice { bots: u8, skill: usize },
    /// A debug test run (no best saved, restarts the same test).
    Debug(u8),
    /// Driven by a network session: no pause, Back needs two presses.
    Online,
}

/// Builds the screen shown when the match ends online / custom flows.
pub type OnOver = Box<dyn FnMut(&ResultsView, PlayerId, &mut Ctx) -> Box<dyn Screen>>;

pub struct RunScreen {
    driver: Box<dyn RunDriver>,
    kind: RunKind,
    on_over: Option<OnOver>,
    pres: Presenter,
    input: InputState,
    hud: HudState,
    ghost: GhostUi,
    t: f32,
    clock_ms: f64,
    last_real: Option<std::time::Instant>,
    paused: bool,
    pause_age: f32,
    over_t: Option<f32>,
    go_t: Option<f32>,
    ghost_t: Option<f32>,
    back_armed: f32,
    best_at_start: u32,
    best_set: bool,
    /// Staged shots / demos: a bot plays the local runner.
    autoplay: Option<flock_core::bot::RunnerBot>,
    record: bool,
    history: VecDeque<FrameScene>,
    paint_dt: f32,
    quality: crate::platform::data_dir::Quality,
    size: egui::Vec2,
    left_handed: bool,
    transition: Option<Transition>,
    issued: Vec<Command>,
    saved_result: bool,
    /// Floating words (streaks, milestones, close dodges).
    callouts: Callouts,
    streak: ClearTracker,
    streak_out: Vec<streak::Out>,
    /// Last 250 m milestone announced.
    milestone: u32,
    best_called: bool,
    /// Flash and time scale after a hit.
    impact: Impact,
    /// First-run gesture hints (solo, until `seen_intro`).
    hints: Option<Hints>,
    hints_done: bool,
    /// Frames since the music was first told about this run.
    audio_frames: u8,
    /// Staging: show this callout once the screen is this old.
    demo_callout: Option<(f32, &'static str)>,
    /// The daily run: `(block height, seed)`. The rules are Solo's.
    daily: Option<(u32, u64)>,
    /// The local player's `(colour, hat)`, for "Run again".
    look: (u8, u8),
    /// This run's coins are in the wallet (they go in exactly once).
    banked: bool,
    /// The daily best on this block before this run (metres).
    daily_prev_best: u32,
    /// Haptic timing (coin rate limit, double ticks, the once-only long buzz).
    buzz: crate::haptics::RunBuzz,
    /// Screen clock of the last "STUCK IN MUD" word (at most one per 2 s).
    mud_word_t: f32,
}

impl RunScreen {
    fn with(driver: Box<dyn RunDriver>, kind: RunKind) -> RunScreen {
        RunScreen {
            driver,
            kind,
            on_over: None,
            pres: Presenter::new(),
            input: InputState::new(),
            hud: HudState::new(),
            ghost: GhostUi::new(),
            t: 0.0,
            clock_ms: 0.0,
            last_real: None,
            paused: false,
            pause_age: 0.0,
            over_t: None,
            go_t: None,
            ghost_t: None,
            back_armed: 0.0,
            best_at_start: 0,
            best_set: false,
            autoplay: None,
            record: false,
            history: VecDeque::new(),
            paint_dt: 0.0,
            quality: Default::default(),
            size: vec2(412.0, 915.0),
            left_handed: false,
            transition: None,
            issued: Vec::new(),
            saved_result: false,
            callouts: Callouts::new(),
            streak: ClearTracker::new(0.42),
            streak_out: Vec::new(),
            milestone: 0,
            best_called: false,
            impact: Impact::new(),
            hints: None,
            hints_done: false,
            audio_frames: 0,
            demo_callout: None,
            daily: None,
            look: (0, flock_render::HAT_DEFAULT),
            banked: false,
            daily_prev_best: 0,
            buzz: Default::default(),
            mud_word_t: -9.0,
        }
    }

    pub fn solo() -> RunScreen {
        RunScreen::solo_as((0, flock_render::HAT_DEFAULT))
    }

    /// Solo with the local bird's `(colour, hat)` (`Profile::look`).
    pub fn solo_as(look: (u8, u8)) -> RunScreen {
        let seed = debug::seed_or(debug::fresh_seed());
        let d: Box<dyn RunDriver> = Box::new(local::LocalDriver::new(flock_session::LocalRun::solo_styled(seed, look.0, look.1)));
        let mut s = RunScreen::with(d, RunKind::Solo);
        s.look = look;
        s
    }

    /// The daily run of block `height`: Solo's rules on `seed`, the same course
    /// for everybody. Logs the block and the course fingerprint.
    pub fn daily(seed: u64, height: u32, look: (u8, u8)) -> RunScreen {
        crate::daily::log_start(height, seed);
        let d: Box<dyn RunDriver> = Box::new(local::LocalDriver::new(flock_session::LocalRun::daily_styled(seed, look.0, look.1)));
        let mut s = RunScreen::with(d, RunKind::Solo);
        s.daily = Some((height, seed));
        s.look = look;
        s
    }

    /// Flock Practice: `bots` 1..=5, `skill` 0 easy / 1 normal / 2 hard.
    pub fn practice(bots: u8, skill: usize) -> RunScreen {
        RunScreen::practice_as(bots, skill, (0, flock_render::HAT_DEFAULT))
    }

    /// Flock Practice with the local bird's `(colour, hat)`.
    pub fn practice_as(bots: u8, skill: usize, look: (u8, u8)) -> RunScreen {
        let seed = debug::seed_or(debug::fresh_seed());
        let d: Box<dyn RunDriver> = Box::new(local::LocalDriver::new(flock_session::LocalRun::practice_styled(seed, bots.clamp(1, 5), local::skill(skill), look.0, look.1)));
        let mut s = RunScreen::with(d, RunKind::Practice { bots, skill });
        s.look = look;
        s
    }

    /// Debug test runs: 0 = six in one lane, 1 = one runner and five ghosts.
    pub fn debug_test(which: u8) -> RunScreen {
        let d = if which == 0 { local::debug_six_in_one_lane() } else { local::debug_one_runner_five_ghosts() };
        RunScreen::with(d, RunKind::Debug(which))
    }

    /// An online match (or any custom driver). `on_over` builds the next screen.
    pub fn online(driver: Box<dyn RunDriver>, on_over: OnOver) -> RunScreen {
        let mut s = RunScreen::with(driver, RunKind::Online);
        s.on_over = Some(on_over);
        s
    }

    /// A run on any driver with the given kind (staging, tests).
    pub fn with_driver(driver: Box<dyn RunDriver>, kind: RunKind) -> RunScreen {
        RunScreen::with(driver, kind)
    }

    pub fn autoplay(mut self, skill: BotSkill, seed: u64) -> RunScreen {
        self.autoplay = Some(flock_core::bot::RunnerBot::new(skill, seed, PlayerId(0)));
        self
    }

    fn offline(&self) -> bool {
        !matches!(self.kind, RunKind::Online)
    }

    fn restart(&self) -> Box<dyn Screen> {
        match self.kind {
            RunKind::Solo | RunKind::Online => match self.daily {
                Some((height, seed)) => Box::new(RunScreen::daily(seed, height, self.look)),
                None => Box::new(RunScreen::solo_as(self.look)),
            },
            RunKind::Practice { bots, skill } => Box::new(RunScreen::practice_as(bots, skill, self.look)),
            RunKind::Debug(w) => Box::new(RunScreen::debug_test(w)),
        }
    }

    fn set_paused(&mut self, p: bool) {
        if self.paused != p {
            self.paused = p;
            self.pause_age = 0.0;
            self.driver.set_paused(p);
            self.input.clear();
            if p {
                Fx::quiet();
            }
            crate::sound::with(|a| if p { a.pause_all() } else { a.resume_all() });
        }
    }

    fn mode(&self, ghost: bool, solo: bool) -> Mode {
        Mode { solo, pause: self.offline(), ghost, left_handed: self.left_handed }
    }

    fn is_ghost(&self) -> bool {
        self.driver.view().is_some_and(|v| v.world.player(v.local).is_some_and(|p| p.role.is_ghost()))
    }

    /// Coins the local player holds in the running world (a run that is being left).
    fn live_coins(&self) -> u32 {
        self.driver.view().map_or(0, |v| {
            let w = &v.world;
            w.runner(v.local).map_or_else(|| w.player(v.local).map_or(0, |p| p.runner.coins), |r| r.coins)
        })
    }

    /// Put this run's coins in the wallet, once. A daily run also counts as an
    /// attempt on its block. Debug test runs bank nothing.
    fn bank(&mut self, ctx: &mut Ctx, coins: u32, metres: u32) {
        if self.banked || matches!(self.kind, RunKind::Debug(_)) {
            return;
        }
        self.banked = true;
        ctx.profile.bank(coins);
        if let Some((height, _)) = self.daily {
            crate::daily::record_attempt(ctx.profile, height, metres, coins);
        }
        ctx.save_profile();
    }

    /// The run is left before it ended (pause menu Restart or Quit, online Back): bank what it holds.
    fn bank_abandoned(&mut self, ctx: &mut Ctx) {
        let metres = self.driver.view().and_then(|v| v.world.solo_result()).map_or(0, |r| r.distance_m.max(0.0) as u32);
        let coins = self.live_coins();
        self.bank(ctx, coins, metres);
    }

    fn finish(&mut self, ctx: &mut Ctx) -> Option<Transition> {
        let res = self.driver.results()?.clone();
        let local_id = self.driver.view().map_or(PlayerId(0), |v| v.local);
        {
            let coins = res.solo.map(|r| r.coins).or_else(|| res.rows.iter().find(|r| r.player == local_id).map(|r| r.coins)).unwrap_or(0);
            let metres = res.solo.map_or(0, |r| r.distance_m.max(0.0) as u32);
            self.bank(ctx, coins, metres);
        }
        if let Some(mut f) = self.on_over.take() {
            let screen = f(&res, local_id, ctx);
            self.on_over = Some(f);
            return Some(Transition::Replace(screen));
        }
        match self.kind {
            RunKind::Solo => {
                let r = res.solo.unwrap_or(flock_core::SoloResult { distance_m: 0.0, coins: 0 });
                let dist = r.distance_m.max(0.0) as u32;
                if let Some((height, _)) = self.daily {
                    // The daily keeps its own best; the solo bests are untouched.
                    let seed = self.daily.map_or(0, |d| d.1);
                    let b = ctx.profile.daily_best_on(height).cloned().unwrap_or_default();
                    let prev = self.daily_prev_best;
                    return Some(Transition::Replace(Box::new(SoloOverScreen::daily(dist, r.coins, DailyOver { height, seed, prev_best: prev, best: b.metres, attempts: b.attempts }))));
                }
                let prev = ctx.profile.best_distance_m;
                let new_best = dist > prev;
                if new_best {
                    ctx.profile.best_distance_m = dist;
                }
                if r.coins > ctx.profile.best_coins {
                    ctx.profile.best_coins = r.coins;
                }
                ctx.save_profile();
                Some(Transition::Replace(Box::new(SoloOverScreen::new(dist, r.coins, prev, new_best))))
            }
            RunKind::Practice { bots, skill } => Some(Transition::Replace(Box::new(practice_results(res, local_id, bots, skill)))),
            RunKind::Debug(w) => Some(Transition::Replace(Box::new(debug_results(res, local_id, w)))),
            RunKind::Online => None,
        }
    }

    fn boot_audio(&mut self, ctx: &mut Ctx) {
        crate::sound::init(ctx.opts.shot.is_some());
        let (music, effects, muted) = (ctx.profile.music_volume, ctx.profile.effects_volume, ctx.muted);
        // A new run's music must not inherit the last round's mood (a ghost's dark, slowed
        // music): the mixer builds a deck from the mood it saw a moment ago, so write the
        // calm mood now and give it a frame to land before the run track is started.
        let first = self.audio_frames == 0;
        self.audio_frames = self.audio_frames.saturating_add(1);
        let paused = self.paused;
        crate::sound::with(|a| {
            a.set_volumes(1.0, music, effects);
            if first {
                a.set_mood(flock_audio::Mood::neutral());
            } else if !muted && !paused {
                a.set_music(Some(MusicTrack::Run));
            }
        });
        ctx.haptics.set_enabled(ctx.profile.haptics);
    }
}

fn practice_results(res: ResultsView, local: PlayerId, bots: u8, skill: usize) -> ResultsScreen {
    ResultsScreen::new(
        res,
        local,
        ResultsLabels { again: "Play again".into(), again_sub: None, leave: "Home".into() },
        Box::new(move |ctx| Transition::Cut(Box::new(RunScreen::practice_as(bots, skill, ctx.profile.look())))),
        Box::new(|_ctx| Transition::PopTo("home")),
    )
}

fn debug_results(res: ResultsView, local: PlayerId, which: u8) -> ResultsScreen {
    ResultsScreen::new(
        res,
        local,
        ResultsLabels { again: "Run again".into(), again_sub: Some("debug test".into()), leave: "Back".into() },
        Box::new(move |_ctx| Transition::Cut(Box::new(RunScreen::debug_test(which)))),
        Box::new(|_ctx| Transition::Pop),
    )
}

impl Drop for RunScreen {
    fn drop(&mut self) {
        Fx::quiet();
        crate::sound::with(|a| {
            a.resume_all();
            a.set_mood(flock_audio::Mood::neutral());
        });
        widgets::set_over_3d(false);
    }
}

impl Screen for RunScreen {
    fn name(&self) -> &'static str {
        "run"
    }

    fn keep_awake(&self) -> bool {
        self.over_t.is_none()
    }

    fn on_background(&mut self, ctx: &mut Ctx) {
        self.input.clear();
        if self.offline() && self.over_t.is_none() {
            self.set_paused(true);
        }
        crate::ui::game::fx::Fx::quiet();
        feedback::on_background();
        let _ = ctx;
    }

    fn back(&mut self, ctx: &mut Ctx) -> Transition {
        feedback::back(ctx);
        if self.over_t.is_some() {
            return Transition::None;
        }
        if self.offline() {
            let p = !self.paused;
            self.set_paused(p);
            Transition::None
        } else if self.back_armed > 0.0 {
            self.bank_abandoned(ctx);
            Transition::Pop
        } else {
            self.back_armed = 2.5;
            ctx.say("Press Back again to leave the game");
            Transition::None
        }
    }

    fn clear_color(&self, _ctx: &Ctx) -> [f32; 4] {
        [0.55, 0.75, 0.95, 1.0]
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        self.t += ctx.dt;
        self.size = ctx.viewport.screen.size();
        self.quality = ctx.profile.quality;
        self.left_handed = ctx.profile.left_handed;
        self.record = ctx.opts.shot.is_some();
        self.back_armed = (self.back_armed - ctx.dt).max(0.0);
        if !self.best_set {
            self.best_set = true;
            self.best_at_start = ctx.profile.best_distance_m;
            if let Some((height, _)) = self.daily {
                // the daily's own best, not the solo one
                self.best_at_start = ctx.profile.daily_best_on(height).map_or(0, |b| b.metres);
                self.daily_prev_best = self.best_at_start;
            }
        }
        self.boot_audio(ctx);
        if let Some(t) = self.transition.take() {
            return t;
        }

        // ---- time: a real clock online, the frame clock offline (slow-mo scales it)
        let real_dt = if ctx.opts.shot.is_some() {
            ctx.dt
        } else {
            let now = std::time::Instant::now();
            let d = self.last_real.map_or(ctx.dt, |l| now.duration_since(l).as_secs_f32());
            self.last_real = Some(now);
            d
        };
        let slow = self.offline() && debug::with(|d| d.slow_mo);
        // Hit-stop and the final-feather slow motion scale a LOCAL sim only (solo); a networked
        // or shared sim never has its time touched. Paused freezes the timers too.
        let live_dt = if self.paused { 0.0 } else { real_dt };
        let hit_scale = if self.kind == RunKind::Solo { self.impact.time_scale() } else { 1.0 };
        self.impact.advance(live_dt);
        self.callouts.update(live_dt);
        let sim_dt = real_dt * if slow { 0.25 } else { 1.0 } * hit_scale;
        self.clock_ms += sim_dt as f64 * 1000.0;
        if let Some((at, text)) = self.demo_callout {
            if self.t >= at {
                self.demo_callout = None;
                self.callouts.push(text, true);
            }
        }

        // ---- input
        let ghost = self.is_ghost();
        let (solo, phase, combo) = match self.driver.view() {
            Some(v) => (v.mode == flock_core::Mode::Solo, v.phase, v.world.tuning().combo_window),
            None => (true, Phase::Countdown, 0.08),
        };
        let safe = ctx.viewport.safe;
        let rc = hud::layout(safe, self.mode(ghost, solo));
        let blocked = rc.controls(self.mode(ghost, solo));
        self.issued.clear();
        let playing = !self.paused && self.over_t.is_none() && phase == Phase::Running && !ghost;
        if playing {
            self.input.poll(ctx.touches, ctx.keys, &blocked, ctx.dt, combo, &mut self.issued);
            if let (Some(bot), Some(v)) = (self.autoplay.as_mut(), self.driver.view()) {
                if let Some(c) = bot.think(&v.world, v.local) {
                    self.issued.push(c);
                    self.input.push_command(c);
                }
            }
        } else {
            self.input.clear();
        }
        for c in self.issued.clone() {
            self.pres.anticipate(c);
        }
        // first-run hints (solo): after GO, until all three have been shown
        if self.kind == RunKind::Solo && self.autoplay.is_none() && !ctx.profile.seen_intro && !self.hints_done && self.hints.is_none() && phase == Phase::Running && !ghost {
            self.hints = Some(Hints::new());
        }
        if let Some(h) = self.hints.as_mut() {
            if !self.paused && self.over_t.is_none() && h.update(live_dt, &self.issued) {
                self.hints = None;
                self.hints_done = true;
                ctx.profile.seen_intro = true;
                ctx.save_profile();
            }
        }
        let frame = self.input.frame();
        self.driver.update(self.clock_ms as u64, frame);

        // ---- present
        let aspect = self.size.x / self.size.y.max(1.0);
        let Some(view) = self.driver.view() else { return Transition::None };
        let paint_dt = if self.paused { 0.0 } else { sim_dt };
        self.paint_dt = paint_dt;
        self.pres.left_handed = self.left_handed;
        self.pres.build(view, if self.paused { 0.0 } else { sim_dt }, aspect, ctx.profile.reduced_shake);
        let me = view.local;
        let w = &view.world;

        let signals: Vec<Signal> = self.pres.signals.clone();
        let mut hz = crate::haptics::Buzzer { buzz: &mut self.buzz, h: &mut *ctx.haptics, now: self.t, on: !self.paused };
        if self.paused {
            hz.buzz.clear();
        } else {
            hz.buzz.update(hz.h, hz.now);
        }
        for s in signals {
            match s {
                Signal::Go => {
                    self.go_t = Some(0.0);
                    hz.pulse(Pulse::Boost);
                }
                Signal::Beep => hz.pulse(Pulse::Landing),
                Signal::LocalCoin => hz.pulse(Pulse::Coin),
                Signal::LocalLanded => hz.pulse(Pulse::Landing),
                Signal::LocalBump => hz.pulse(Pulse::Boost),
                Signal::LocalSlam => hz.pulse(Pulse::Boost),
                Signal::LocalSwagger => {
                    hz.pulse(Pulse::Boost);
                    self.callouts.push("PHEW!", false);
                }
                Signal::LocalMudStuck => {
                    hz.pulse(Pulse::Landing);
                    if self.t - self.mud_word_t >= 2.0 {
                        self.mud_word_t = self.t;
                        self.callouts.push("STUCK IN MUD", false);
                    }
                }
                Signal::LocalPower(kind) => {
                    hz.double_tick(Pulse::Boost);
                    self.callouts.push(
                        match kind {
                            flock_core::PowerKind::Magnet => "MAGNET!",
                            flock_core::PowerKind::Doubler => "DOUBLE COINS!",
                            flock_core::PowerKind::Shield => "SHIELD!",
                        },
                        true,
                    );
                }
                Signal::LocalShieldBroken => {
                    hz.pulse(Pulse::Hit);
                    self.callouts.push("SHIELD BROKEN", false);
                }
                Signal::LocalBecameGhost => {
                    self.ghost_t = Some(0.0);
                    self.input.clear();
                    hz.long_once();
                }
                Signal::Placed { by, .. } if by == me => {
                    self.pres.released();
                    hz.pulse(Pulse::Landing);
                }
                Signal::LocalFeatherLost => {
                    hz.pulse(Pulse::Bash);
                    self.impact.flash();
                    if self.kind == RunKind::Solo {
                        if view.over || w.runner(me).is_none_or(|r| r.feathers == 0) {
                            self.impact.slow_mo();
                        } else {
                            self.impact.hit_stop();
                        }
                    }
                }
                _ => {}
            }
        }
        // the end of a solo run is the player's elimination too
        if view.over && self.kind == RunKind::Solo {
            hz.long_once();
        }

        // ---- callouts: clear streaks, close dodges, solo milestones
        if phase == Phase::Running && self.over_t.is_none() && !self.paused {
            feed_streak(&mut self.streak, &mut self.streak_out, view);
            for o in self.streak_out.drain(..) {
                match o {
                    streak::Out::Clear { streak } => {
                        if let Some(word) = streak::streak_word(streak) {
                            self.callouts.push(word, true);
                        }
                    }
                    streak::Out::Close => self.callouts.push("CLOSE!", false),
                    streak::Out::Reset { .. } => {}
                }
            }
            if solo {
                let d = w.solo_result().map_or(0.0, |r| r.distance_m);
                let k = (d / 250.0) as u32;
                if k > self.milestone {
                    self.milestone = k;
                    self.callouts.push(format!("{} m", k * 250), (k * 250) % 1000 == 0);
                }
                if self.best_at_start > 0 && !self.best_called && d as u32 > self.best_at_start {
                    self.best_called = true;
                    self.callouts.push("NEW BEST!", true);
                }
            }
        }
        if let Some(t) = self.go_t.as_mut() {
            *t += sim_dt;
        }
        if w.player(me).is_some_and(|p| p.role.is_ghost()) && self.ghost_t.is_none() {
            self.ghost_t = Some(0.0);
        }
        if let Some(t) = self.ghost_t.as_mut() {
            *t += sim_dt;
        }

        // ---- HUD numbers
        self.hud.set_doubler(w.runner(me).is_some_and(|r| r.doubler(w.tick())));
        if let Some(r) = w.runner(me) {
            self.hud.update(sim_dt, r.feathers, w.tuning().feathers, r.coins);
        } else {
            let (f, c) = w.player(me).map_or((0, 0), |p| (p.runner.feathers, p.runner.coins));
            self.hud.update(sim_dt, f, w.tuning().feathers, c);
        }

        // ---- the music follows the run
        {
            let tun = w.tuning();
            let solo = view.mode == flock_core::Mode::Solo;
            let r = w.runner(me);
            let mood = music_mood(&MoodInputs {
                solo,
                running: phase == Phase::Running && self.over_t.is_none(),
                v_course: w.v_course(),
                v_start: tun.course_v0,
                v_top: if solo { tun.course_vmax } else { tun.mp_v_end },
                living: w.living_count(),
                total: w.players().iter().filter(|p| !matches!(p.role, Role::Disconnected)).count(),
                time_left: (!solo).then(|| (tun.mp_cap - w.running_secs()).max(0.0)),
                feathers: r.map_or(0, |r| r.feathers),
                max_feathers: tun.feathers,
                ghost: w.player(me).is_some_and(|p| p.role.is_ghost()),
            });
            crate::sound::with(|a| a.set_mood(mood));
        }

        // ---- ghost mode
        let is_ghost = w.player(me).is_some_and(|p| matches!(p.role, Role::Ghost { .. }));
        let mut place_now = false;
        if is_ghost && !self.paused {
            let proj = Projector::new(&self.pres.scene.camera, self.size);
            let controls = rc.controls(self.mode(true, solo));
            self.ghost.update(view, &proj, ctx.touches, ctx.keys, &controls, ctx.dt, self.pres.origin);
            place_now = true;
            let mut cells = std::mem::take(&mut self.pres.scene.placement);
            let mut coins = Vec::new();
            let preview = self.ghost.layer(w, me, self.pres.origin, self.t, &mut cells, &mut coins);
            self.pres.scene.placement = cells;
            self.pres.scene.farm_props.extend(preview);
            self.pres.scene.coins.extend(coins);
        } else {
            self.pres.clear_ghost_layer();
        }
        let _ = place_now;
        if ghost_ui::GhostUi::accepted_now(view) {
            self.pres.released();
        }

        // ---- over
        let over = view.over;
        if self.record {
            if self.history.len() >= HISTORY {
                self.history.pop_front();
            }
            self.history.push_back(self.pres.scene.clone());
        }
        let _ = self.saved_result;
        // placement requests (after the view borrow ends)
        let out: Vec<(Tool, u8, u32)> = std::mem::take(&mut self.ghost.outbox);
        for (tool, lane, row) in out {
            self.driver.place(tool, lane, row);
        }
        if over {
            let t = self.over_t.get_or_insert(0.0);
            *t += ctx.dt;
            let hold = if self.kind == RunKind::Solo { OVER_HOLD_SOLO } else { OVER_HOLD_MULTI };
            if *t >= hold {
                if let Some(tr) = self.finish(ctx) {
                    return tr;
                }
            }
        }
        Transition::None
    }

    fn paint_scene(&mut self, pass: &mut ScenePass) {
        let shift = std::mem::take(&mut self.pres.pending_shift);
        let n = self.history.len().saturating_sub(1);
        let (replay, _) = if self.record && n > 0 {
            let s = self.history.make_contiguous();
            (&s[..n], ())
        } else {
            (&[][..], ())
        };
        stage::paint(pass, Draw { scene: &self.pres.scene, dt: self.paint_dt.min(0.05), quality: self.quality, shift_z: shift, replay });
        if self.record {
            // shots paint once: forget the history so a window never re-replays
            self.history.clear();
        }
    }

    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        widgets::set_over_3d(true);
        let safe = ctx.viewport.safe;
        let screen = ctx.viewport.screen;
        let tm = ctx.time as f32;
        let brand = art::brand(ui);
        let mut out = Transition::None;
        let Some(view) = self.driver.view() else {
            // online and not started yet
            let p = ui.layer_painter(egui::LayerId::background());
            p.text(screen.center(), Align2::CENTER_CENTER, "Getting ready...", theme::title(26.0), theme::TEXT);
            return out;
        };
        let w = &view.world;
        let me = view.local;
        let solo = view.mode == flock_core::Mode::Solo;
        let ghost = w.player(me).is_some_and(|p| p.role.is_ghost());
        let mode = self.mode(ghost, solo);
        let rc = hud::layout(safe, mode);
        let tun = w.tuning();
        // numbers
        let runner = w.runner(me);
        let data = Data {
            feathers: runner.map_or(0, |r| r.feathers),
            max_feathers: tun.feathers,
            coins: runner.map_or_else(|| w.player(me).map_or(0, |p| p.runner.coins), |r| r.coins),
            distance_m: w.solo_result().map_or(0.0, |r| r.distance_m),
            best_m: (solo && self.best_at_start > 0).then_some(self.best_at_start),
            time_left: (!solo).then(|| (tun.mp_cap - w.running_secs()).max(0.0)),
            living: w.living_count(),
            total: w.players().iter().filter(|p| !matches!(p.role, Role::Disconnected)).count(),
            protected: runner.and_then(|r| r.protected_until).map(|u| (u.saturating_sub(w.tick()) as f32 / (tun.protect_time / flock_core::DT)).clamp(0.0, 1.0)),
            free_hit: runner.is_some_and(|r| r.free_hit),
            powers: runner.map_or([None; 3], |r| {
                let now = w.tick();
                let left = |until: flock_core::Tick, secs: f32| {
                    (now < until).then(|| {
                        let l = (until.saturating_sub(now) as f32 * flock_core::DT).min(secs);
                        ((l / secs.max(0.1)).clamp(0.0, 1.0), l)
                    })
                };
                [left(r.magnet_until, tun.magnet_s), left(r.double_until, tun.doubler_s), left(r.shield_until, tun.shield_s)]
            }),
            solo,
            ghost,
        };
        let phase = view.phase;
        let countdown = view.countdown;
        let go_t = self.go_t;
        let over = view.over;
        let paused = self.paused;
        let energy = w.ghost_energy(me).unwrap_or(0.0);
        let living_txt = format!("{} running", w.living_count());
        let tools: Vec<ToolView> = Tool::ALL
            .iter()
            .map(|t| {
                let cost = ghost_ui::tool_cost(w, *t);
                ToolView { icon: ghost_ui::tool_icon(*t), label: ghost_ui::tool_name(*t), cost, selected: self.ghost.tool == *t, affordable: energy + 1e-3 >= cost, tint: ghost_ui::tool_tint(*t) }
            })
            .collect();
        let info = if ghost { self.ghost.info(w, me) } else { None };
        let proj = Projector::new(&self.pres.scene.camera, screen.size());
        let tags = self.pres.tags.clone();
        let banners = self.pres.banners.clone();
        let dbg = debug::with(|d| d.clone());
        let offline = self.offline();
        let ghost_msg = self.ghost.msg.clone();
        let ghost_shake = self.ghost.shake;
        let ghost_t = self.ghost_t;
        let glide = self.pres.rig.glide();
        let pause_age = self.pause_age;
        let left_handed = self.left_handed;
        let is_solo_kind = self.kind == RunKind::Solo;
        let daily_block = self.daily;
        let debug_cells: Vec<_> = if dbg.placement && ghost { self.ghost.debug_cells().collect() } else { Vec::new() };
        let anchor = w.anchor_z();
        let origin = self.pres.origin;
        let pack_band = tun.pack_band;
        let fence = tun.fence_x;
        let protect_time = tun.protect_time;
        let objs = if dbg.boxes { w.course().objects_in(anchor - 10.0, anchor + 50.0) } else { Vec::new() };
        let flash_a = self.impact.flash_alpha(if ctx.profile.reduced_shake { 0.18 } else { 1.0 });
        let mut tool_clicked: Option<Tool> = None;
        let mut want_pause = false;
        let mut pause_choice: Option<u8> = None;

        #[allow(deprecated)]
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let p = ui.painter().clone();
            // ---- hit flash (under the HUD)
            if flash_a > 0.0 {
                p.rect_filled(screen, 0.0, Color32::from_white_alpha((flash_a * 255.0).round() as u8));
            }
            // ---- runner HUD (dimmed in ghost mode, which has its own top row)
            if !ghost {
                hud::draw_runner(&p, &brand, &rc, &self.hud, &data, tm);
                if let Some((height, _)) = daily_block {
                    let label = format!("DAILY \u{b7} {}", crate::daily::thousands(height));
                    widgets::pill(&p, pos2(rc.sub.center().x, rc.sub.bottom() + 12.0), &label, rgba(0x14, 0x1E, 0x46, 0.7), theme::GOLD, 11.0);
                }
            } else {
                hud::draw_energy(&p, rc.energy, energy, tun.ghost_energy_max, 0.0, tm);
                let r = ghost_ui::living_pill(rc.banner, left_handed);
                widgets::pill(&p, r.center(), &living_txt, rgba(14, 6, 30, 0.6), theme::TEXT, 12.0);
            }
            // pause button
            if let Some(pr) = rc.pause {
                let resp = ui.interact(pr, Id::new("run-pause"), Sense::click());
                hud::draw_pause_icon(&p, pr, resp.is_pointer_button_down_on());
                if resp.clicked() && !paused && !over {
                    want_pause = true;
                }
            }
            // protection rings above birds
            for tag in &tags {
                if let (Some(frac), Some(s)) = (tag.protected, proj.project(tag.head)) {
                    let k = (proj.scale_at(tag.head) / 60.0).clamp(0.5, 1.2);
                    hud::ring(&p, s + vec2(0.0, -4.0 * k), 12.0 * k, frac, 3.2 * k, theme::MINT);
                }
                if dbg.protection {
                    if let Some(s) = proj.project(tag.head + glam::Vec3::Y * 0.4) {
                        let txt = match tag.protected {
                            Some(f) => format!("P{} {:.1}s", tag.id.0, f * protect_time),
                            None => format!("P{} vuln", tag.id.0),
                        };
                        widgets::pill(&p, s, &txt, rgba(0, 0, 0, 0.7), theme::MINT, 10.0);
                    }
                }
                if dbg.boxes && !tag.ghost {
                    let (r, h) = tag.body;
                    wire_box(&p, &proj, tag.feet - glam::Vec3::new(r, 0.0, r), tag.feet + glam::Vec3::new(r, h, r), theme::MINT);
                }
            }
            if dbg.boxes {
                for o in &objs {
                    let c = crate::ui::game::cameras::to_render(o.x, 0.0, o.z, origin);
                    let col = match o.damage() {
                        flock_core::Damage::Crash => theme::LASER,
                        flock_core::Damage::Slow => theme::ACCENT,
                        flock_core::Damage::None => theme::GOLD,
                    };
                    wire_box(&p, &proj, glam::Vec3::new(c.x - o.half_w, o.y, c.z - o.half_d), glam::Vec3::new(c.x + o.half_w, o.top, c.z + o.half_d), col);
                }
            }
            if dbg.pack {
                for (dz, col) in [(0.0, theme::GOLD), (pack_band as f64, theme::LASER), (-(pack_band as f64), theme::LASER)] {
                    let z = (anchor + dz - origin) as f32;
                    if let (Some(a), Some(b)) = (proj.project(glam::Vec3::new(fence, 0.2, z)), proj.project(glam::Vec3::new(-fence, 0.2, z))) {
                        p.line_segment([a, b], Stroke::new(2.0_f32, col));
                    }
                }
            }
            for (lane, row, check) in &debug_cells {
                let x = (*lane as f32 - 1.0) * tun.lane_width;
                let c = crate::ui::game::cameras::to_render(x, 0.2, *row as f64 * tun.row_spacing as f64, origin);
                if let Some(s) = proj.project(c) {
                    let txt = match check {
                        Ok(_) => format!("{row}"),
                        Err(r) => format!("{row} {r:?}"),
                    };
                    p.text(s, Align2::CENTER_CENTER, txt, theme::ui_bold(9.5), Color32::WHITE);
                }
            }

            // ---- countdown / GO
            if phase == Phase::Countdown {
                hud::draw_countdown(&p, screen, countdown, None);
            } else if let Some(t) = go_t {
                hud::draw_countdown(&p, screen, 0.0, Some(t));
            }

            // ---- callouts and first-run hints
            if !ghost {
                self.callouts.draw(&p, screen);
                if !paused && !over {
                    if let Some(h) = &self.hints {
                        h.draw(&p, screen);
                    }
                }
            }

            // ---- ghost HUD
            if ghost {
                // instruction banner (first seconds) or the latest message
                let (text, tint, a) = match (&ghost_msg, ghost_t) {
                    (Some(m), _) => (m.text.as_str(), if m.ok { theme::MINT } else { theme::LASER }, 1.0 - art::phase(m.age, 1.7, 0.5)),
                    (None, Some(t)) if t < INSTRUCTION_SECS => (ghost_ui::INSTRUCTION, theme::GOLD, art::phase(t, 0.5 * glide.max(0.2), 0.4) * (1.0 - art::phase(t, INSTRUCTION_SECS - 0.8, 0.8))),
                    _ => ("", theme::GOLD, 0.0),
                };
                let shake = ghost_shake * (tm * 70.0).sin() * 6.0;
                hud::draw_banner(&p, rc.banner.translate(vec2(shake, 0.0)), text, a, tint);
                for (i, t) in tools.iter().enumerate() {
                    let resp = ui.interact(rc.tools[i], Id::new(("run-tool", i)), Sense::click());
                    hud::draw_tool(&p, rc.tools[i], t, resp.is_pointer_button_down_on());
                    if resp.clicked() {
                        tool_clicked = Some(Tool::ALL[i]);
                    }
                }
                // cursor verdict
                if let Some((txt, ok)) = &info {
                    let tint = if *ok { theme::MINT } else { theme::LASER };
                    hud::draw_banner(&p, Rect::from_center_size(rc.info.center(), vec2(rc.info.width(), 38.0)), txt, 1.0, tint);
                }
                // The view faces the flock: a small pointer in each runner's colour above its head
                // (the birds are small at this distance, and the pointer says which is which).
                for tag in tags.iter().filter(|t| !t.ghost) {
                    let Some(top) = proj.project(tag.head) else { continue };
                    if !screen.shrink(4.0).contains(top) {
                        continue;
                    }
                    let col = art::from_f3(crate::ui::game::presenter::body_colour(tags_colour(view, tag.id)));
                    let c = top + vec2(0.0, -7.0);
                    p.add(egui::Shape::convex_polygon(vec![c + vec2(0.0, 6.0), c + vec2(-5.5, -3.0), c + vec2(5.5, -3.0)], col, Stroke::new(1.5_f32, Color32::WHITE)));
                }
            }

            // ---- the architect's name on a gate's banner (every viewer)
            for b in &banners {
                let Some(s) = proj.project(b.centre) else { continue };
                if !screen.expand(30.0).contains(s) {
                    continue;
                }
                let k = proj.scale_at(b.centre);
                let name = short_name(&b.name);
                let n = name.chars().count().max(1) as f32;
                // as big as the banner allows, hidden when it would be unreadable
                let font = (b.size.y * k * 0.62).min(b.size.x * k * 0.92 / (n * 0.56)).min(34.0);
                if font < 8.5 {
                    continue;
                }
                art::outlined(&p, s, Align2::CENTER_CENTER, &name, theme::ui_bold(font), Color32::WHITE, rgba(20, 10, 40, 0.95), (font * 0.14).max(1.5), 0.0);
            }

            // ---- over banner
            if over {
                let k = art::ease_out_back(art::phase(self.over_t.unwrap_or(0.0), 0.0, 0.35));
                let label = if is_solo_kind { "GAME OVER" } else { "ROUND OVER" };
                art::outlined(&p, screen.center() - vec2(0.0, screen.height() * 0.1), Align2::CENTER_CENTER, label, theme::title(46.0 * (0.6 + 0.4 * k)), theme::TEXT, theme::OUTLINE, 4.0, 2.0);
            }

            // ---- pause menu
            if paused {
                pause_choice = pause_menu(ui, ctx, screen, safe, pause_age, tm, offline && is_solo_kind);
            }
        });
        if let Some(t) = tool_clicked {
            self.ghost.select(t);
            feedback::tap(ctx);
        }
        if want_pause {
            feedback::tap(ctx);
            self.set_paused(true);
        }
        match pause_choice {
            Some(0) => {
                feedback::select(ctx);
                self.set_paused(false);
            }
            Some(1) => {
                feedback::select(ctx);
                self.bank_abandoned(ctx);
                out = Transition::Cut(self.restart());
            }
            Some(2) => {
                feedback::back(ctx);
                self.bank_abandoned(ctx);
                out = Transition::PopTo("home");
            }
            _ => {}
        }
        if self.paused {
            self.pause_age += ctx.dt;
        }
        out
    }
}

/// Feed the clear-streak detector from the display world: damaging obstacles around the
/// local runner and the local crash events of this frame.
fn feed_streak(tr: &mut ClearTracker, out: &mut Vec<streak::Out>, view: &FlockView) {
    let w = &view.world;
    let Some(r) = w.runner(view.local) else { return };
    let tun = w.tuning();
    let (tick, lw) = (w.tick(), tun.lane_width);
    let obstacles: Vec<streak::Obs> = w
        .course()
        .objects_in(r.pos.z - 6.0, r.pos.z + 30.0)
        .iter()
        .filter(|o| o.kind.is_solid() && o.is_live(tick))
        .map(|o| {
            let (a, b) = o.lanes();
            streak::Obs { id: o.id.0, x0: tun.lane_x(a) - lw * 0.5, x1: tun.lane_x(b) + lw * 0.5, z0: o.z0(), z1: o.z1() }
        })
        .collect();
    let crashes: Vec<streak::Crash> = view
        .events
        .iter()
        .filter_map(|e| match *e {
            Event::Crash { player, obj, feather_lost, .. } if player == view.local => Some(streak::Crash { obj: obj.0, feather_lost }),
            _ => None,
        })
        .collect();
    tr.update(&streak::Frame { x: r.pos.x, z: r.pos.z, speed: r.forward_speed(w.v_course()), obstacles: &obstacles, crashes: &crashes }, out);
}

/// Pause card: Resume, Restart, Home. Returns the choice.
fn pause_menu(ui: &mut egui::Ui, _ctx: &mut Ctx, screen: Rect, safe: Rect, age: f32, _tm: f32, _solo: bool) -> Option<u8> {
    let e = art::ease_out_back(art::phase(age, 0.0, 0.3)).min(1.05);
    let fade = art::ease_out_cubic(art::phase(age, 0.0, 0.2));
    let p = ui.painter().clone();
    p.rect_filled(screen, 0.0, rgba(6, 2, 14, 0.62 * fade));
    let w = (safe.width() - 48.0).min(320.0);
    let h = 292.0;
    let card = Rect::from_center_size(pos2(safe.center().x, safe.center().y), vec2(w, h)).translate(vec2(0.0, (1.0 - e.min(1.0)) * 30.0));
    let mut choice = None;
    widgets::staged(ui, fade, vec2(0.0, 0.0), |ui, _| {
        let p = ui.painter().clone();
        widgets::panel_solid(&p, card, 24.0);
        widgets::heading(&p, pos2(card.center().x, card.top() + 38.0), Align2::CENTER_CENTER, "PAUSED", 34.0, theme::TEXT);
        let bw = card.width() - 40.0;
        let at = |i: usize| Rect::from_min_size(pos2(card.left() + 20.0, card.top() + 74.0 + i as f32 * 68.0), vec2(bw, 58.0));
        if widgets::button(ui, at(0), "pause-resume", &Btn::primary("Resume").icon(crate::ui::icons::Icon::Play).size(24.0)).clicked() {
            choice = Some(0);
        }
        if widgets::button(ui, at(1), "pause-restart", &Btn::new("Restart").icon(crate::ui::icons::Icon::Restart).size(20.0)).clicked() {
            choice = Some(1);
        }
        if widgets::button(ui, at(2), "pause-home", &Btn::ghost("Home").icon(crate::ui::icons::Icon::Home).size(20.0)).clicked() {
            choice = Some(2);
        }
    });
    choice
}

/// A wireframe box (debug overlays).
fn wire_box(p: &egui::Painter, proj: &Projector, min: glam::Vec3, max: glam::Vec3, col: Color32) {
    let c = |x: bool, y: bool, z: bool| glam::Vec3::new(if x { max.x } else { min.x }, if y { max.y } else { min.y }, if z { max.z } else { min.z });
    let corners: Vec<Option<egui::Pos2>> = (0..8).map(|i| proj.project(c(i & 1 != 0, i & 2 != 0, i & 4 != 0))).collect();
    let st = Stroke::new(1.5_f32, col);
    for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
        if let (Some(x), Some(y)) = (corners[a], corners[b]) {
            p.line_segment([x, y], st);
        }
    }
}

// ---- staged states for --shot (and desktop demos) -------------------------------

fn advance(d: &mut dyn RunDriver, from_ms: &mut u64, ms: u64) {
    let end = *from_ms + ms;
    while *from_ms < end {
        *from_ms += 16;
        d.update(*from_ms, flock_core::InputFrame::default());
    }
}

impl RunScreen {
    /// A solo run played by a bot, a few seconds in.
    pub fn sample_solo() -> RunScreen {
        RunScreen::solo_fixed(11).autoplay(BotSkill::HARD, 3)
    }

    /// The daily run of a made-up block, played by a bot, in a hat.
    pub fn sample_daily() -> RunScreen {
        let mut s = RunScreen::daily(0x4b9c_c6b9_ba92_b55f, 921_344, (2, 8)).autoplay(BotSkill::HARD, 3);
        s.demo_callout = None;
        s
    }

    fn solo_fixed(seed: u64) -> RunScreen {
        let d: Box<dyn RunDriver> = Box::new(local::LocalDriver::new(flock_session::LocalRun::solo(seed)));
        RunScreen::with(d, RunKind::Solo)
    }

    /// A solo run staged with the local bird curled into its roll just before a gate: a scripted Down
    /// is issued a few metres short of the first gate in the runner's lane, and the run is handed over
    /// the moment the bird goes `Collapsed` (the shot's own frames then ease the ball in).
    pub fn sample_roll() -> RunScreen {
        use flock_core::{Loco, ObjKind};
        let mut run = flock_session::LocalRun::solo(11);
        let mut frame = flock_core::InputFrame::default();
        // Step exactly as the screen does in a shot (a 1/60 s frame clock) so the run replays identically.
        let mut clock = 0.0f64;
        let mut t = 0u64;
        let mut sent = false;
        while t < 90_000 {
            clock += (1.0f32 / 60.0) as f64 * 1000.0;
            t = clock as u64;
            {
                let Some(v) = flock_session::RunDriver::view(&run) else { break };
                if v.phase == Phase::Running && !sent {
                    if let Some(me) = v.world.runner(PlayerId(0)) {
                        let lane = ((me.pos.x / 2.2).round() as i32 + 1).clamp(0, 2) as u8;
                        let near = v.world.course().objects_in(me.pos.z - 1.0, me.pos.z + 40.0);
                        let solid: Vec<_> = near.iter().filter(|o| o.kind.is_solid() && o.covers_lane(lane)).collect();
                        // the next gate in this lane, with nothing else solid in front of it
                        let gap = solid
                            .iter()
                            .filter(|o| o.z0() > me.pos.z)
                            .min_by(|a, b| a.z0().total_cmp(&b.z0()))
                            .filter(|o| matches!(o.kind, ObjKind::Beam { .. }))
                            .map_or(f64::MAX, |o| o.z0() - me.pos.z);
                        // on the ground and running, a few metres short of the gate: crouch
                        if gap < 8.0 && gap > 4.0 && me.pos.y < 0.02 && matches!(me.loco, Loco::Running) {
                            frame = flock_core::InputFrame { seq: 1, cmd: Some(Command::Down) };
                            sent = true;
                        }
                    }
                }
            }
            flock_session::RunDriver::update(&mut run, t, frame);
            if sent {
                let Some(v) = flock_session::RunDriver::view(&run) else { break };
                if v.world.runner(PlayerId(0)).is_some_and(|me| matches!(me.loco, Loco::Collapsed { .. })) {
                    break;
                }
            }
        }
        let mut s = RunScreen::with(Box::new(local::LocalDriver::new(run)), RunKind::Solo);
        s.clock_ms = clock;
        s
    }

    /// Six in one lane, mid pile-up.
    pub fn sample_pileup() -> RunScreen {
        let mut d = local::debug_six_in_one_lane();
        let mut t = 0;
        advance(d.as_mut(), &mut t, 4300);
        let mut s = RunScreen::with(d, RunKind::Debug(0));
        s.clock_ms = t as f64;
        s
    }

    /// A practice run where the local player is already a ghost, mid band.
    pub fn sample_ghost() -> RunScreen {
        let mut run = flock_session::LocalRun::practice(7, 5, BotSkill::NORMAL);
        run.world_mut().force_eliminate(PlayerId(0));
        let mut d: Box<dyn RunDriver> = Box::new(local::LocalDriver::new(run));
        let mut t = 0;
        advance(d.as_mut(), &mut t, 9000);
        let mut s = RunScreen::with(d, RunKind::Practice { bots: 5, skill: 1 });
        s.clock_ms = t as f64;
        s.ghost.select(Tool::Hay);
        s.ghost_t = Some(0.8);
        s
    }

    /// Ghost mode with a square selected (preview showing).
    pub fn sample_ghost_preview() -> RunScreen {
        let mut s = RunScreen::sample_ghost();
        s.ghost.demo_cursor = Some((2, 2));
        s
    }

    /// A living local runner with three ghosts floating ahead of the pack (the outline look).
    pub fn sample_ghosts_overhead() -> RunScreen {
        let mut run = flock_session::LocalRun::practice(7, 5, BotSkill::NORMAL);
        for g in 1..=3 {
            run.world_mut().force_eliminate(PlayerId(g));
        }
        let mut d: Box<dyn RunDriver> = Box::new(local::LocalDriver::new(run));
        let mut t = 0;
        advance(d.as_mut(), &mut t, 6200);
        let mut s = RunScreen::with(d, RunKind::Practice { bots: 5, skill: 1 }).autoplay(BotSkill::HARD, 3);
        s.clock_ms = t as f64;
        s.hints_done = true;
        s
    }

    /// The architect's view: the local player is a ghost looking back at the flock, a tool selected
    /// on a legal square, the dimmed buffer in front of the runners, and a couple of drops from
    /// another ghost already on the course.
    pub fn sample_architect() -> RunScreen {
        let mut run = flock_session::LocalRun::practice(7, 5, BotSkill::NORMAL);
        run.world_mut().force_eliminate(PlayerId(0));
        run.world_mut().force_eliminate(PlayerId(1));
        let mut d = ScriptDriver::new(run);
        d.at(8300, PlayerId(1), Tool::Hay, 0, 1);
        d.at(8400, PlayerId(1), Tool::Beam, 2, 3);
        let mut d: Box<dyn RunDriver> = Box::new(d);
        let mut t = 0;
        advance(d.as_mut(), &mut t, 9000);
        let mut s = RunScreen::with(d, RunKind::Practice { bots: 5, skill: 1 });
        s.clock_ms = t as f64;
        s.ghost.select(Tool::Hay);
        s.ghost.demo_cursor = Some((1, 4));
        s.ghost_t = Some(0.8);
        s
    }

    /// Drops as a runner sees them: a gate that has landed carrying its architect's name, a hay
    /// bale mid-fall with its landing ring, a mud splat and a coin lure raining in. (The band is
    /// pulled in close so the drops land inside the runner camera; the rules are the sim's.)
    pub fn sample_drop() -> RunScreen {
        let mut cfg = flock_core::MatchConfig::multi(7, 4, Some(BotSkill::NORMAL));
        for (i, p) in cfg.players.iter_mut().enumerate() {
            p.bot = if i == 0 { None } else { Some(BotSkill::NORMAL) };
            p.name = ["You", "Marigold", "Pip", "Bot 3"][i].into();
        }
        cfg.tuning.band_near_s = 0.5;
        cfg.tuning.min_reaction_s = 0.3;
        let mut run = flock_session::LocalRun::new(cfg, PlayerId(0));
        run.world_mut().force_eliminate(PlayerId(1));
        run.world_mut().force_eliminate(PlayerId(2));
        let mut d = ScriptDriver::new(run);
        // the screen clock starts here; the shot is taken 2 s later
        let start = 6000u64;
        d.at(start, PlayerId(1), Tool::Beam, 1, 4);
        d.at(start + 60, PlayerId(2), Tool::Mud, 2, 6);
        d.at(start + 1700, PlayerId(2), Tool::Hay, 0, 1);
        d.at(start + 1650, PlayerId(1), Tool::Coins, 2, 2);
        let mut d: Box<dyn RunDriver> = Box::new(d);
        let mut t = 0;
        advance(d.as_mut(), &mut t, start - 10);
        let mut s = RunScreen::with(d, RunKind::Practice { bots: 3, skill: 1 }).autoplay(BotSkill::HARD, 3);
        s.clock_ms = t as f64;
        s.hints_done = true;
        s
    }

    /// A solo run with no input, set up a quarter second before its first feather is lost
    /// (feathers, tumble, protection).
    pub fn sample_crash() -> RunScreen {
        let mk = || flock_session::LocalRun::solo(11);
        // find the crash moment on a first pass (the sim is deterministic without input)
        let mut probe = mk();
        let mut t = 0u64;
        let mut crash_ms = 9000;
        'find: while t < 60_000 {
            t += 16;
            flock_session::RunDriver::update(&mut probe, t, flock_core::InputFrame::default());
            if let Some(v) = flock_session::RunDriver::view(&probe) {
                if v.events.iter().any(|e| matches!(e, flock_core::Event::Crash { feather_lost: true, .. })) {
                    crash_ms = t;
                    break 'find;
                }
            }
        }
        let mut d: Box<dyn RunDriver> = Box::new(local::LocalDriver::new(mk()));
        let mut now = 0;
        advance(d.as_mut(), &mut now, crash_ms.saturating_sub(300));
        let mut s = RunScreen::with(d, RunKind::Solo);
        s.clock_ms = now as f64;
        s
    }

    /// Flock Practice with five bots, set up just before the first bump.
    pub fn sample_practice() -> RunScreen {
        let mk = || flock_session::LocalRun::practice(21, 5, BotSkill::NORMAL);
        let mut probe = mk();
        let mut t = 0u64;
        let mut bump_ms = 9000;
        while t < 90_000 {
            t += 16;
            flock_session::RunDriver::update(&mut probe, t, flock_core::InputFrame::default());
            if flock_session::RunDriver::view(&probe).is_some_and(|v| v.events.iter().any(|e| matches!(e, flock_core::Event::Bump { .. } | flock_core::Event::Crash { .. }))) {
                bump_ms = t;
                break;
            }
        }
        let mut d: Box<dyn RunDriver> = Box::new(local::LocalDriver::new(mk()));
        let mut now = 0;
        advance(d.as_mut(), &mut now, bump_ms.saturating_sub(200));
        let mut s = RunScreen::with(d, RunKind::Practice { bots: 5, skill: 1 });
        s.clock_ms = now as f64;
        s
    }

    /// A solo run just past GO with a big "ON FIRE!" callout popping in near the end of a 1 s shot.
    pub fn sample_callout() -> RunScreen {
        let mut s = RunScreen::sample_after_go(11, 4000);
        s.demo_callout = Some((0.88, "ON FIRE!"));
        s.hints_done = true;
        s
    }

    /// A solo run just after GO, showing the jump hint (the second of the three).
    pub fn sample_hint() -> RunScreen {
        let mut s = RunScreen::sample_after_go(11, 3500);
        s.hints = Some(Hints::at_step(1));
        s
    }

    /// A solo run played `ms` of sim time in (no input, so well inside the clear run-in).
    fn sample_after_go(seed: u64, ms: u64) -> RunScreen {
        let mut d: Box<dyn RunDriver> = Box::new(local::LocalDriver::new(flock_session::LocalRun::solo(seed)));
        let mut t = 0;
        advance(d.as_mut(), &mut t, ms);
        let mut s = RunScreen::with(d, RunKind::Solo);
        s.clock_ms = t as f64;
        s
    }

    /// The countdown.
    pub fn sample_countdown() -> RunScreen {
        RunScreen::solo_fixed(5)
    }

    /// Step a run `ms` forward on the 16 ms clock the staging helpers share.
    fn step(run: &mut flock_session::LocalRun, t: &mut u64, ms: u64) {
        advance(run, t, ms);
    }

    /// Clear solids from `rows` and put `(row offset, lane, kind)` power-ups ahead of the local runner.
    fn plant_powerups(run: &mut flock_session::LocalRun, rows: u32, power: &[(u32, u8, flock_core::PowerKind)]) {
        use flock_core::course::{Cell, Row};
        let w = run.world_mut();
        let z = w.runner(PlayerId(0)).map_or(0.0, |r| r.pos.z);
        let r0 = w.course().row_at(z) + 2;
        for i in 0..rows {
            let mut row = w.course().row(r0 + i).copied().unwrap_or_else(|| Row::empty(r0 + i));
            for c in row.cells.iter_mut() {
                *c = c.with_solid(None).with_power(None).with_coins(flock_core::course::CoinPattern::None);
            }
            for (off, lane, kind) in power {
                if *off == i {
                    row.cells[*lane as usize] = Cell::EMPTY.with_power(Some(*kind));
                }
            }
            w.debug_course_mut().insert_row(r0 + i, row);
        }
    }

    /// Solo with the three power-ups on the track ahead (Magnet, Doubler, Shield at different
    /// distances), Magnet and Doubler running, and a shield bubble round the bird.
    pub fn sample_powerups() -> RunScreen {
        use flock_core::PowerKind;
        let mut run = flock_session::LocalRun::solo(11);
        let mut t = 0u64;
        RunScreen::step(&mut run, &mut t, 3000);
        // the shot runs 2 s: put the pickups well beyond where the bird will be by then
        RunScreen::plant_powerups(&mut run, 14, &[(4, 0, PowerKind::Magnet), (7, 2, PowerKind::Doubler), (10, 1, PowerKind::Shield)]);
        let w = run.world_mut();
        let now = w.tick();
        if let Some(r) = w.debug_runner_mut(PlayerId(0)) {
            r.magnet_until = now + (6.0 / flock_core::DT) as u64;
            r.double_until = now + (8.5 / flock_core::DT) as u64;
            r.shield_until = now + (30.0 / flock_core::DT) as u64;
        }
        let mut s = RunScreen::with(Box::new(local::LocalDriver::new(run)), RunKind::Solo);
        s.clock_ms = t as f64;
        s.hints_done = true;
        s
    }

    /// The local bird (protected, free hit ready) is run into a hay wall: its free hit goes off as a
    /// swagger, and the shot lands in the middle of the stagger, with the CAREFUL chip showing.
    pub fn sample_swagger() -> RunScreen {
        let stage = || {
            let mut run = flock_session::LocalRun::solo(11);
            let mut t = 0u64;
            RunScreen::step(&mut run, &mut t, 3000);
            RunScreen::plant_powerups(&mut run, 12, &[]);
            {
                use flock_core::course::{Cell, Row, SolidKind};
                let w = run.world_mut();
                let z = w.runner(PlayerId(0)).map_or(0.0, |r| r.pos.z);
                let wall = w.course().row_at(z) + 2 + 8;
                let mut row = Row::empty(wall);
                for c in row.cells.iter_mut() {
                    *c = Cell::EMPTY.with_solid(Some(SolidKind::Hay));
                }
                w.debug_course_mut().insert_row(wall, row);
                let now = w.tick();
                if let Some(r) = w.debug_runner_mut(PlayerId(0)) {
                    r.protected_until = Some(now + (60.0 / flock_core::DT) as u64);
                    r.protect_since = Some(now);
                    r.free_hit = true;
                }
            }
            (run, t)
        };
        // find the swagger on a first pass
        let (mut probe, mut pt) = stage();
        let mut hit_ms = pt + 4000;
        'find: while pt < 60_000 {
            RunScreen::step(&mut probe, &mut pt, 16);
            if let Some(v) = flock_session::RunDriver::view(&probe) {
                if v.events.iter().any(|e| matches!(e, flock_core::Event::Swagger { .. })) {
                    hit_ms = pt;
                    break 'find;
                }
            }
        }
        let (mut run, mut t) = stage();
        let lead = hit_ms.saturating_sub(t).saturating_sub(1700);
        RunScreen::step(&mut run, &mut t, lead);
        let mut s = RunScreen::with(Box::new(local::LocalDriver::new(run)), RunKind::Solo);
        s.clock_ms = t as f64;
        s.hints_done = true;
        s
    }
}

/// A local run that makes other players' ghost drops on a schedule (sample screens only): at sim
/// time `at_ms`, `player` places `tool` on `lane`, `row_off` rows into the placement band.
struct ScriptDriver {
    run: flock_session::LocalRun,
    steps: Vec<(u64, PlayerId, Tool, u8, u32)>,
    next: usize,
    req: u32,
}

impl ScriptDriver {
    fn new(run: flock_session::LocalRun) -> ScriptDriver {
        ScriptDriver { run, steps: Vec::new(), next: 0, req: 5000 }
    }

    fn at(&mut self, at_ms: u64, player: PlayerId, tool: Tool, lane: u8, row_off: u32) {
        self.steps.push((at_ms, player, tool, lane, row_off));
    }
}

impl RunDriver for ScriptDriver {
    fn update(&mut self, now_ms: u64, input: flock_core::InputFrame) {
        while self.next < self.steps.len() && now_ms >= self.steps[self.next].0 {
            let (_, player, tool, lane, off) = self.steps[self.next];
            self.next += 1;
            let w = self.run.world_mut();
            w.debug_set_energy(player, 10.0);
            let band = w.placement_band();
            if !band.is_empty() {
                self.req += 1;
                let _ = w.place(flock_core::PlaceRequest { player, req: self.req, tool, lane, row: band.first + off });
            }
        }
        self.run.update(now_ms, input);
    }
    fn place(&mut self, tool: Tool, lane: u8, row: u32) -> u32 {
        self.run.place(tool, lane, row)
    }
    fn view(&self) -> Option<&FlockView> {
        self.run.view()
    }
    fn results(&self) -> Option<&ResultsView> {
        self.run.results()
    }
    fn set_paused(&mut self, p: bool) {
        self.run.set_paused(p)
    }
}

#[allow(dead_code)]
fn _kind_is_used(k: RunKind) -> bool {
    matches!(k, RunKind::Solo | RunKind::Practice { .. } | RunKind::Debug(_) | RunKind::Online)
}

/// A banner's name: at most ten characters.
fn short_name(n: &str) -> String {
    let n = n.trim();
    if n.chars().count() <= 10 {
        n.to_string()
    } else {
        let mut t: String = n.chars().take(9).collect();
        t.push('.');
        t
    }
}

fn tags_colour(view: &flock_session::FlockView, id: PlayerId) -> u8 {
    view.name_of(id).map_or(id.0, |n| n.1)
}
