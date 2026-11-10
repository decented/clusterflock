//! The screen router: a stack of [`Screen`]s, [`Transition`]s between them, and
//! the [`Ctx`] every screen method gets. To add a screen: implement [`Screen`]
//! in a new file here and add a line to [`SCREENS`] (see `docs/api/app-shell.md`).

pub mod account;
pub mod bluetooth;
pub mod common;
pub mod games;
pub mod home;
pub mod join;
pub mod lobby;
pub mod multiplayer;
pub mod nearby;
pub mod online_common;
pub mod online_flow;
pub mod practice;
pub mod run;
pub mod results;
pub mod settings;
pub mod settings_cards;
pub mod solo_over;
pub mod splash;
pub mod wardrobe;

use crate::cli::Options;
use crate::haptics::Haptics;
use crate::platform::data_dir::Profile;
use crate::platform::{Keys, Platform, Touches, Viewport};
use crate::render::ScenePass;
use crate::ui::theme;
use std::path::Path;

/// How long a message from `Ctx::say` shows.
pub const TOAST_SECS: f32 = 2.6;
/// Dip to the background colour before a screen change, seconds.
pub const FADE_OUT: f32 = 0.14;
/// ...and back up from it.
pub const FADE_IN: f32 = 0.22;

/// A short message over any screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Toast {
    pub text: String,
    pub left: f32,
}

/// Everything a screen may use. Built by the engine for every `update`, `draw`
/// and `back` call; not stored.
pub struct Ctx<'a> {
    /// Seconds since the previous frame (0.1 at most).
    pub dt: f32,
    /// Engine time in seconds (a virtual clock in `--shot`).
    pub time: f64,
    /// Screen size, insets and safe rectangle in UI points.
    pub viewport: Viewport,
    /// Every finger down (raw multi-touch). Empty while a screen change fades out.
    pub touches: &'a Touches,
    /// Keyboard (desktop).
    pub keys: &'a Keys,
    pub haptics: &'a mut dyn Haptics,
    /// Clipboard, launch links, wake lock, multicast lock.
    pub platform: &'a mut dyn Platform,
    /// The player's name and settings. Changes are saved on exit and when the app is backgrounded.
    pub profile: &'a mut Profile,
    pub data_dir: &'a Path,
    /// A screen change is fading this screen out: it is still updated, but not on top for long.
    pub input_blocked: bool,
    /// The app is backgrounded or unfocused: keep sound off.
    pub muted: bool,
    pub opts: &'a Options,
    pub(crate) toast: &'a mut Option<Toast>,
    /// In-app updates, and the download / install path More games shares.
    pub updates: &'a mut crate::update::Updater,
    pub account: &'a mut Option<crate::account::Account>,
}

impl Ctx<'_> {
    /// Show a message over the screen for a couple of seconds.
    pub fn say(&mut self, text: impl Into<String>) {
        *self.toast = Some(Toast { text: text.into(), left: TOAST_SECS });
    }

    /// Write the profile now.
    pub fn save_profile(&self) {
        self.profile.save(self.data_dir);
    }
}

/// What a screen asks the router to do after a call.
pub enum Transition {
    None,
    /// Put a screen on top (Back returns to the current one), with a fade.
    Push(Box<dyn Screen>),
    /// Swap the current screen for another, with a fade.
    Replace(Box<dyn Screen>),
    /// Swap with no fade.
    Cut(Box<dyn Screen>),
    /// Go back one screen. On the first screen this quits (Android: to the background).
    Pop,
    /// Pop screens until the one with this `name()` is on top.
    PopTo(&'static str),
    /// Leave the app.
    Quit,
}

impl Transition {
    pub fn is_none(&self) -> bool {
        matches!(self, Transition::None)
    }
}

/// One screen of the app.
pub trait Screen {
    /// Stable lower-case name: the key `--shot NAME` / `--screen NAME` use, and
    /// what `Transition::PopTo` matches.
    fn name(&self) -> &'static str;

    /// Logic, once per frame, before drawing: read `ctx.touches` / `ctx.keys`,
    /// advance state by `ctx.dt`. A returned transition takes effect this frame.
    fn update(&mut self, _ctx: &mut Ctx) -> Transition {
        Transition::None
    }

    /// Draw the UI with egui (and read egui widget input), once per frame after `update`.
    fn draw(&mut self, _ctx: &mut Ctx, _ui: &egui::Context) -> Transition {
        Transition::None
    }

    /// System Back (Android Back key, Esc on desktop). Default: go back one screen.
    fn back(&mut self, _ctx: &mut Ctx) -> Transition {
        Transition::Pop
    }

    /// The colour (sRGB 0..1, rgba) the frame is cleared to before anything is drawn.
    fn clear_color(&self, _ctx: &Ctx) -> [f32; 4] {
        theme::bg_clear()
    }

    /// Draw your own scene (a 3D view) into the frame, after the clear
    /// and before egui. See [`ScenePass`].
    fn paint_scene(&mut self, _pass: &mut ScenePass) {}

    /// The whole app is portrait; a screen overrides this only if it must not be.
    fn orientation(&self) -> crate::platform::os::Orientation {
        crate::platform::os::Orientation::Portrait
    }

    /// Keep the screen on while this screen is showing (a run in progress).
    fn keep_awake(&self) -> bool {
        false
    }

    /// The app went to the background (or lost focus): pause what must pause.
    fn on_background(&mut self, _ctx: &mut Ctx) {}
}

/// The screens `--shot` and `--screen` can name, plus staged variants (`*_sample`)
/// that fake the state a screen needs.
pub const SCREENS: &[(&str, fn() -> Box<dyn Screen>)] = &[
    ("signin", || Box::new(account::AccountScreen::sample())),
    ("play_online", || Box::new(account::AccountScreen::online(None))),
    ("splash", || Box::new(splash::SplashScreen::new())),
    ("home", || Box::new(home::HomeScreen::new())),
    ("settings", || Box::new(settings::SettingsScreen::new())),
    ("settings_keypad", || Box::new(settings::SettingsScreen::keypad_sample())),
    ("settings_cards", || Box::new(settings::SettingsScreen::at_cards())),
    ("settings_updates_sample", || Box::new(settings::SettingsScreen::updates_sample(false))),
    ("settings_downloading_sample", || Box::new(settings::SettingsScreen::updates_sample(true))),
    ("more_games", || Box::new(games::GamesScreen::sample_get())),
    ("more_games_none_sample", || Box::new(games::GamesScreen::sample_none())),
    ("more_games_downloading_sample", || Box::new(games::GamesScreen::sample_downloading())),
    ("more_games_open_sample", || Box::new(games::GamesScreen::sample_open())),
    ("multiplayer_menu", || Box::new(multiplayer::MultiplayerScreen::sample())),
    ("join_keypad", || Box::new(join::JoinScreen::sample())),
    ("join_waiting", || Box::new(join::JoinScreen::waiting_sample())),
    ("join_failed", || Box::new(join::JoinScreen::failed_sample())),
    ("nearby_list", || Box::new(nearby::NearbyScreen::sample())),
    ("nearby_empty", || Box::new(nearby::NearbyScreen::empty_sample())),
    ("bluetooth_scan", || Box::new(bluetooth::BluetoothScreen::scan_sample())),
    ("bluetooth_host", || Box::new(bluetooth::BluetoothScreen::host_sample())),
    ("bluetooth_permission", || Box::new(bluetooth::BluetoothScreen::permission_sample())),
    ("lobby_host", || Box::new(lobby::LobbyScreen::host_sample())),
    ("lobby_guest", || Box::new(lobby::LobbyScreen::guest_sample())),
    ("lobby_waiting", || Box::new(lobby::LobbyScreen::waiting_sample())),
    ("practice", || Box::new(practice::PracticeScreen::new())),
    ("solo_over", || Box::new(solo_over::SoloOverScreen::sample())),
    ("solo_over_plain", || Box::new(solo_over::SoloOverScreen::sample_plain())),
    ("run_solo", || Box::new(run::RunScreen::sample_solo())),
    ("run_practice", || Box::new(run::RunScreen::sample_practice())),
    ("run_crash", || Box::new(run::RunScreen::sample_crash())),
    ("run_countdown", || Box::new(run::RunScreen::sample_countdown())),
    ("run_callout", || Box::new(run::RunScreen::sample_callout())),
    ("run_hint", || Box::new(run::RunScreen::sample_hint())),
    ("run_roll", || Box::new(run::RunScreen::sample_roll())),
    ("run_pileup", || Box::new(run::RunScreen::sample_pileup())),
    ("run_ghost", || Box::new(run::RunScreen::sample_ghost())),
    ("run_ghost_preview", || Box::new(run::RunScreen::sample_ghost_preview())),
    ("run_ghosts_overhead", || Box::new(run::RunScreen::sample_ghosts_overhead())),
    ("run_architect", || Box::new(run::RunScreen::sample_architect())),
    ("run_drop", || Box::new(run::RunScreen::sample_drop())),
    ("run_powerups", || Box::new(run::RunScreen::sample_powerups())),
    ("run_swagger", || Box::new(run::RunScreen::sample_swagger())),
    ("settings_debug", || Box::new(settings::SettingsScreen::debug_sample())),
    ("results", || Box::new(results::ResultsScreen::sample())),
    ("results_win", || Box::new(results::ResultsScreen::sample_win())),
    ("wardrobe", || Box::new(wardrobe::WardrobeScreen::sample())),
    ("home_daily", || Box::new(home::HomeScreen::sample_daily())),
    ("solo_over_daily", || Box::new(solo_over::SoloOverScreen::sample_daily())),
    ("run_daily", || Box::new(run::RunScreen::sample_daily())),
];

/// A new screen by name (see [`SCREENS`]).
pub fn by_name(name: &str) -> Option<Box<dyn Screen>> {
    SCREENS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, f)| f())
}

/// The names in [`SCREENS`].
pub fn names() -> Vec<&'static str> {
    SCREENS.iter().map(|(n, _)| *n).collect()
}

enum Fade {
    Idle,
    /// Fading to the background; the transition runs at the end.
    Out { t: f32, then: Transition },
    /// Fading back in.
    In { t: f32 },
}

/// Did a transition ask to leave the app?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    Stay,
    Quit,
}

/// The stack of screens and the fade between them.
pub struct Router {
    stack: Vec<Box<dyn Screen>>,
    fade: Fade,
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Router {
    pub fn new(first: Box<dyn Screen>) -> Router {
        Router { stack: vec![first], fade: Fade::Idle }
    }

    pub fn top(&self) -> &dyn Screen {
        self.stack.last().expect("the router always holds a screen").as_ref()
    }

    pub fn top_mut(&mut self) -> &mut (dyn Screen + 'static) {
        self.stack.last_mut().expect("the router always holds a screen").as_mut()
    }

    /// Names of the stack, bottom first.
    pub fn names(&self) -> Vec<&'static str> {
        self.stack.iter().map(|s| s.name()).collect()
    }

    /// Input is held back while the old screen fades out.
    pub fn input_blocked(&self) -> bool {
        matches!(self.fade, Fade::Out { .. })
    }

    /// Opacity (0..1) of the background-coloured sheet drawn over everything.
    pub fn fade_alpha(&self) -> f32 {
        match &self.fade {
            Fade::Idle => 0.0,
            Fade::Out { t, .. } => ease(t / FADE_OUT),
            Fade::In { t } => 1.0 - ease(t / FADE_IN),
        }
    }

    /// Ask for a transition. Fades start here and finish in [`Router::tick`];
    /// while one fades out, further requests are dropped.
    pub fn apply(&mut self, t: Transition) -> Applied {
        match t {
            Transition::None => Applied::Stay,
            // A fade-out in progress owns the next change.
            _ if matches!(self.fade, Fade::Out { .. }) => Applied::Stay,
            Transition::Cut(s) => {
                self.stack.pop();
                self.stack.push(s);
                self.fade = Fade::Idle;
                Applied::Stay
            }
            Transition::Pop if self.stack.len() <= 1 => Applied::Quit,
            Transition::Quit => Applied::Quit,
            other => {
                self.fade = Fade::Out { t: 0.0, then: other };
                Applied::Stay
            }
        }
    }

    /// Advance the fade by `dt`; at the bottom of a fade-out the pending
    /// transition happens.
    pub fn tick(&mut self, dt: f32) {
        match std::mem::replace(&mut self.fade, Fade::Idle) {
            Fade::Idle => {}
            Fade::Out { t, then } => {
                let t = t + dt;
                if t < FADE_OUT {
                    self.fade = Fade::Out { t, then };
                } else {
                    self.perform(then);
                    self.fade = Fade::In { t: 0.0 };
                }
            }
            Fade::In { t } => {
                let t = t + dt;
                if t < FADE_IN {
                    self.fade = Fade::In { t };
                }
            }
        }
    }

    fn perform(&mut self, t: Transition) {
        match t {
            Transition::Push(s) => self.stack.push(s),
            Transition::Replace(s) | Transition::Cut(s) => {
                self.stack.pop();
                self.stack.push(s);
            }
            Transition::Pop => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                }
            }
            Transition::PopTo(name) => {
                if self.stack.iter().any(|s| s.name() == name) {
                    while self.stack.len() > 1 && self.stack.last().is_some_and(|s| s.name() != name) {
                        self.stack.pop();
                    }
                }
            }
            Transition::None | Transition::Quit => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Named(&'static str);
    impl Screen for Named {
        fn name(&self) -> &'static str {
            self.0
        }
    }

    fn settle(r: &mut Router) {
        for _ in 0..60 {
            r.tick(1.0 / 60.0);
        }
    }

    #[test]
    fn push_pop_and_quit_on_the_first_screen() {
        let mut r = Router::new(Box::new(Named("a")));
        assert_eq!(r.apply(Transition::Push(Box::new(Named("b")))), Applied::Stay);
        assert!(r.input_blocked());
        assert_eq!(r.names(), ["a"], "the change waits for the fade");
        settle(&mut r);
        assert_eq!(r.names(), ["a", "b"]);
        assert_eq!(r.fade_alpha(), 0.0);
        assert_eq!(r.apply(Transition::Pop), Applied::Stay);
        settle(&mut r);
        assert_eq!(r.names(), ["a"]);
        assert_eq!(r.apply(Transition::Pop), Applied::Quit, "Back on the first screen leaves the app");
    }

    #[test]
    fn replace_cut_and_pop_to() {
        let mut r = Router::new(Box::new(Named("a")));
        r.apply(Transition::Cut(Box::new(Named("b"))));
        assert_eq!(r.names(), ["b"]);
        r.apply(Transition::Push(Box::new(Named("c"))));
        settle(&mut r);
        r.apply(Transition::Push(Box::new(Named("d"))));
        settle(&mut r);
        r.apply(Transition::PopTo("b"));
        settle(&mut r);
        assert_eq!(r.names(), ["b"]);
        r.apply(Transition::Replace(Box::new(Named("e"))));
        settle(&mut r);
        assert_eq!(r.names(), ["e"]);
    }

    #[test]
    fn fade_dips_and_returns() {
        let mut r = Router::new(Box::new(Named("a")));
        r.apply(Transition::Push(Box::new(Named("b"))));
        let mut peak = 0.0f32;
        for _ in 0..60 {
            r.tick(1.0 / 60.0);
            peak = peak.max(r.fade_alpha());
        }
        assert!(peak > 0.95, "fully dark at the swap, got {peak}");
        assert_eq!(r.fade_alpha(), 0.0);
    }

    #[test]
    fn registry_names() {
        for n in names() {
            // staged variants (`*_sample`, `settings_keypad`) report the screen they stage
            let got = by_name(n).unwrap().name();
            assert!(got == n || n.ends_with("_sample") || n.starts_with(got), "{n} reports itself as {got}");
        }
        assert!(by_name("nope").is_none());
    }
}
