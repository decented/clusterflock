//! The platform-independent frame: input state, the router, egui and the
//! services screens use. The windowed app (`app.rs`) and the headless
//! screenshot mode (`shot.rs`) both drive an [`Engine`] and then render its
//! output, so a screen behaves the same in both.

use crate::cli::Options;
use crate::haptics::Haptics;
use crate::platform::data_dir::Profile;
use crate::platform::os::Platform;
use crate::platform::viewport::ui_ppp;
use crate::platform::{Insets, Keys, KeyCode, TouchPhase, Touches, Viewport};
use crate::ui::screens::{Applied, Ctx, Router, Screen, Toast};
use crate::ui::theme;
use egui::{pos2, Pos2};
use std::path::PathBuf;

/// Screens that need online approval from My Signet (see `Account::online_allowed`).
/// The router wraps a push/cut/replace to one of these in the approval screen
/// when approval is missing: the multiplayer menu and everything behind it.
const ONLINE_SCREENS: &[&str] = &["multiplayer", "join", "nearby", "bluetooth", "lobby", "host", "friends"];

/// Screens a join link may open the join flow over (the menu family and Home).
const LINK_OK_ON: &[&str] = &["home", "multiplayer", "join", "nearby", "bluetooth", "settings"];

/// How long a join link waits for a screen it may open over (seconds).
const LINK_WAIT: f64 = 30.0;

/// Where egui's pointer lets go when a screen change drops a finger (no widget there).
const OFFSCREEN: Pos2 = Pos2::new(-10_000.0, -10_000.0);

/// The services a screen reaches through [`Ctx`]. Separate from the router so
/// both can be borrowed at once.
pub struct Services {
    pub touches: Touches,
    pub keys: Keys,
    pub haptics: Box<dyn Haptics>,
    pub platform: Box<dyn Platform>,
    pub profile: Profile,
    pub data_dir: PathBuf,
    /// Backgrounded or unfocused: sound off (flock-audio reads this via `Ctx::muted`).
    pub muted: bool,
    pub opts: Options,
    pub toast: Option<Toast>,
    /// In-app updates and the install path More games shares (see `crate::release`).
    pub updates: nostr_release_kit::Updater,
    pub account: Option<crate::account::Account>,
    /// What screens see while a fade-out holds input back.
    no_touches: Touches,
}

impl Services {
    fn ctx(&mut self, dt: f32, time: f64, viewport: Viewport, input_blocked: bool) -> Ctx<'_> {
        Ctx {
            dt,
            time,
            viewport,
            touches: if input_blocked { &self.no_touches } else { &self.touches },
            keys: &self.keys,
            haptics: self.haptics.as_mut(),
            platform: self.platform.as_mut(),
            profile: &mut self.profile,
            data_dir: &self.data_dir,
            input_blocked,
            muted: self.muted,
            opts: &self.opts,
            toast: &mut self.toast,
            updates: &mut self.updates,
            account: &mut self.account,
        }
    }
}

/// What a frame produced (render it, then present).
pub struct FrameOut {
    pub egui: egui::FullOutput,
    /// The colour to clear the frame to (sRGB 0..1, rgba).
    pub clear: [f32; 4],
    pub viewport: Viewport,
    /// The event loop must end.
    pub quit: bool,
}

pub struct Engine {
    pub router: Router,
    pub svc: Services,
    pub egui: egui::Context,
    /// Safe-area insets in UI points (set by the platform before each frame).
    pub insets: Insets,
    /// Engine time in seconds.
    pub time: f64,
    pub frames: u64,
    /// CPU time of the last frame's logic and UI, milliseconds.
    pub last_ms: f32,
    back_requested: bool,
    keep_on: bool,
    orientation: Option<crate::platform::os::Orientation>,
    backgrounded: bool,
    /// The platform's room service (keeps the process in use) is on.
    room_service: bool,
    /// UI pixels per point of the current surface (see `platform::viewport::ui_ppp`).
    ppp: f32,
    /// egui pointer events since the last frame.
    egui_events: Vec<egui::Event>,
    /// The touch (or mouse button) that drives egui's single pointer.
    egui_primary: Option<u64>,
    cursor: Pos2,
    /// A join code from a launch link, and when it arrived (engine time), until a screen can take it.
    pending_link: Option<(String, f64)>,
}

impl Engine {
    pub fn new(first: Box<dyn Screen>, profile: Profile, data_dir: PathBuf, opts: Options, platform: Box<dyn Platform>, haptics: Box<dyn Haptics>) -> Engine {
        let egui = egui::Context::default();
        theme::install(&egui);
        let insets = opts.insets;
        // Screenshots keep the state in memory only (their staged offers are throwaway-signed).
        let mut updates = crate::release::updater(if opts.shot.is_some() { None } else { Some(&data_dir) });
        updates.set_user_relays(crate::platform::relays());
        // Staged screenshots and unit harnesses have no persisted accounts or sockets.
        let account = if opts.shot.is_some() || cfg!(test) { None } else { Some(crate::account::Account::open(&data_dir)) };
        // My Signet is needed for multiplayer only: solo and practice start
        // with no account and no network.
        let first = match &account {
            Some(a) if ONLINE_SCREENS.contains(&first.name()) && !a.signed_in() => Box::new(crate::ui::screens::account::AccountScreen::login(first)) as Box<dyn Screen>,
            Some(a) if ONLINE_SCREENS.contains(&first.name()) && !a.online_allowed() => Box::new(crate::ui::screens::account::AccountScreen::online(Some(first))),
            _ => first,
        };
        Engine {
            router: Router::new(first),
            svc: Services {
                touches: Touches::new(),
                keys: Keys::default(),
                haptics,
                platform,
                profile,
                data_dir,
                muted: false,
                opts,
                toast: None,
                updates,
                account,
                no_touches: Touches::new(),
            },
            egui,
            insets,
            time: 0.0,
            frames: 0,
            last_ms: 0.0,
            back_requested: false,
            keep_on: false,
            orientation: None,
            backgrounded: false,
            room_service: false,
            ppp: 1.0,
            egui_events: Vec::new(),
            egui_primary: None,
            cursor: Pos2::ZERO,
            pending_link: None,
        }
    }

    pub fn screen_name(&self) -> &'static str {
        self.router.top().name()
    }

    // ── surface ─────────────────────────────────────────────────────────────

    /// The surface is `size_px` big at OS scale factor `native_ppp`. Input
    /// positions (physical pixels) are divided by the UI scale this sets.
    pub fn set_surface(&mut self, size_px: (u32, u32), native_ppp: f32) {
        self.ppp = ui_ppp(size_px, native_ppp);
    }

    /// UI pixels per point right now.
    pub fn ppp(&self) -> f32 {
        self.ppp
    }

    // ── input (positions in physical pixels) ────────────────────────────────

    fn to_pt(&self, px: (f64, f64)) -> Pos2 {
        pos2(px.0 as f32 / self.ppp, px.1 as f32 / self.ppp)
    }

    /// A finger: raw to `Ctx::touches`, and the newest one down also drives egui's pointer.
    pub fn touch_event(&mut self, id: u64, phase: TouchPhase, px: (f64, f64)) {
        let pos = self.to_pt(px);
        self.svc.touches.touch(id, phase, pos);
        self.egui_pointer(id, phase, pos);
    }

    /// The mouse cursor moved (desktop).
    pub fn cursor_moved(&mut self, px: (f64, f64)) {
        let pos = self.to_pt(px);
        self.cursor = pos;
        self.svc.touches.cursor_moved(pos);
        if self.egui_primary.is_none() || self.egui_primary == Some(crate::platform::touch::MOUSE_LEFT) {
            self.egui_events.push(egui::Event::PointerMoved(pos));
        }
    }

    /// The cursor left the window (desktop).
    pub fn cursor_left(&mut self) {
        if self.egui_primary.is_none() {
            self.egui_events.push(egui::Event::PointerGone);
        }
    }

    /// A mouse button (desktop): left = touch [`crate::platform::touch::MOUSE_LEFT`], right = second touch.
    pub fn mouse_button(&mut self, left: bool, pressed: bool) {
        self.svc.touches.mouse_button(left, pressed);
        if left {
            let phase = if pressed { TouchPhase::Started } else { TouchPhase::Ended };
            let pos = self.cursor;
            self.egui_pointer(crate::platform::touch::MOUSE_LEFT, phase, pos);
            if !pressed {
                // Back to hovering.
                self.egui_events.push(egui::Event::PointerMoved(pos));
            }
        }
    }

    fn egui_pointer(&mut self, id: u64, phase: TouchPhase, pos: Pos2) {
        let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        match phase {
            // The newest finger down drives egui's pointer: a steering thumb that
            // stays on the glass must not stop taps on pause / results buttons.
            TouchPhase::Started => {
                self.egui_primary = Some(id);
                self.egui_events.push(egui::Event::PointerMoved(pos));
                self.egui_events.push(button(true));
            }
            TouchPhase::Moved if self.egui_primary == Some(id) => self.egui_events.push(egui::Event::PointerMoved(pos)),
            TouchPhase::Ended if self.egui_primary == Some(id) => {
                self.egui_primary = None;
                self.egui_events.push(egui::Event::PointerMoved(pos));
                self.egui_events.push(button(false));
                if id != crate::platform::touch::MOUSE_LEFT {
                    self.egui_events.push(egui::Event::PointerGone);
                }
            }
            _ => {}
        }
    }

    /// Drop every finger (screen change): raw touches and egui's pointer.
    pub(crate) fn clear_touches(&mut self) {
        self.svc.touches.clear();
        if let Some(id) = self.egui_primary.take() {
            // Let go off-screen, so no widget sees a click, then forget the pointer.
            self.egui_events.push(egui::Event::PointerMoved(OFFSCREEN));
            self.egui_events.push(egui::Event::PointerButton { pos: OFFSCREEN, button: egui::PointerButton::Primary, pressed: false, modifiers: egui::Modifiers::NONE });
            if id == crate::platform::touch::MOUSE_LEFT {
                self.egui_events.push(egui::Event::PointerMoved(self.cursor));
            } else {
                self.egui_events.push(egui::Event::PointerGone);
            }
        }
    }

    /// A physical key (desktop).
    pub fn key_event(&mut self, key: KeyCode, down: bool) {
        self.svc.keys.key(key, down);
    }

    /// System Back (Android Back key, Esc on desktop). Acted on at the start
    /// of the next frame: the current screen's `Screen::back`.
    pub fn request_back(&mut self) {
        self.back_requested = true;
    }

    // ── lifecycle ───────────────────────────────────────────────────────────

    /// The app went to the background or lost focus: tell the screen, drop
    /// fingers and keys, mute, release the wake lock, save the profile.
    pub fn on_background(&mut self) {
        crate::sound::set_suspended(true);
        if std::mem::replace(&mut self.backgrounded, true) {
            return;
        }
        self.svc.touches.clear();
        self.svc.keys.clear();
        self.egui_primary = None;
        self.egui_events.clear();
        self.svc.muted = true;
        crate::online::set_away(true);
        let viewport = self.viewport_now();
        let mut ctx = self.svc.ctx(0.0, self.time, viewport, false);
        self.router.top_mut().on_background(&mut ctx);
        self.sync_wake_lock();
        self.svc.profile.save(&self.svc.data_dir);
    }

    /// Back in the foreground: sound on again.
    pub fn on_foreground(&mut self) {
        self.backgrounded = false;
        crate::online::set_away(false);
        if let Some(account) = &self.svc.account { account.resume(); }
        self.sync_orientation();
        self.svc.muted = false;
        crate::sound::set_suspended(false);
    }

    /// Does something need driving even with no frames (the app in the
    /// background)? The platform loop calls [`Engine::background_tick`] every few
    /// tens of milliseconds while this is true: a My Signet request waiting for
    /// its answer.
    pub fn wants_background_tick(&self) -> bool {
        self.svc.account.as_ref().is_some_and(|a| a.busy()) || crate::online::is_some()
    }

    /// Drive account work without a frame.
    pub fn background_tick(&mut self) {
        self.account_upkeep();
        // Keep a hosted room (and our seat) alive while the UI is paused.
        crate::online::pump_auto();
        crate::online::end_frame();
        self.sync_room_service();
    }

    /// Keep the platform's room service on exactly while there is an online session.
    fn sync_room_service(&mut self) {
        let want = self.svc.account.as_ref().is_some_and(|a| a.busy()) || crate::online::is_some();
        if want != self.room_service {
            self.room_service = want;
            self.svc.platform.set_room_service(want);
        }
    }

    /// Keep account work alive. Sign-in is asked for only on the way into a
    /// multiplayer screen (see `apply`), never for solo or practice.
    fn account_upkeep(&mut self) {
        let mut needs_login = false;
        if let Some(account) = &mut self.svc.account {
            account.tick(self.svc.platform.as_mut());
            needs_login = !account.signed_in() && ONLINE_SCREENS.contains(&self.router.top().name());
            // Online approval withdrawn (or signed out) mid-session: leave the room.
            if !account.online_allowed() && crate::online::is_some() {
                crate::online::leave();
                self.svc.toast = Some(crate::ui::screens::Toast { text: "My Signet approval ended, so you left the game.".into(), left: 4.0 });
            }
            if let Some(name) = account.name() {
                self.svc.profile.name = crate::platform::data_dir::sanitize_name(&name, "Flocker");
            }
        }
        if needs_login && self.router.top().name() != "signin" {
            self.clear_touches();
            self.router = Router::new(Box::new(crate::ui::screens::account::AccountScreen::login(
                Box::new(crate::ui::screens::home::HomeScreen::new()))));
        }
    }

    /// The loop is ending (Android: the process ends right after).
    pub fn on_exit(&mut self) {
        self.svc.platform.set_keep_screen_on(false);
        self.svc.profile.save(&self.svc.data_dir);
    }

    fn viewport_now(&self) -> Viewport {
        // Only used outside frames (background hook): a nominal portrait size.
        Viewport::new(egui::vec2(411.0, 890.0), self.ppp, self.insets)
    }

    fn sync_orientation(&mut self) {
        if self.backgrounded { return; }
        let want = self.router.top().orientation();
        if self.orientation != Some(want) {
            self.orientation = Some(want);
            self.svc.platform.set_orientation(want);
        }
    }

    fn sync_wake_lock(&mut self) {
        let want = !self.backgrounded && self.router.top().keep_awake();
        if want != self.keep_on {
            self.keep_on = want;
            self.svc.platform.set_keep_screen_on(want);
        }
    }

    /// Apply a transition; true when the event loop must end. A quit first
    /// asks the platform to leave the app its own way: on Android that sends
    /// the game to the background and the loop goes on.
    fn apply(&mut self, t: crate::ui::screens::Transition) -> bool {
        use crate::ui::screens::Transition;
        let gated = |next: &dyn Screen, account: &Option<crate::account::Account>| ONLINE_SCREENS.contains(&next.name()) && account.as_ref().is_some_and(|a| !a.signed_in() || !a.online_allowed());
        let signed_in = self.svc.account.as_ref().is_none_or(|a| a.signed_in());
        let ask = |next: Box<dyn Screen>| {
            if signed_in {
                Box::new(crate::ui::screens::account::AccountScreen::online(Some(next))) as Box<dyn Screen>
            } else {
                Box::new(crate::ui::screens::account::AccountScreen::login_for(next)) as Box<dyn Screen>
            }
        };
        let t = match t {
            Transition::Push(next) if gated(next.as_ref(), &self.svc.account) => Transition::Push(ask(next)),
            Transition::Cut(next) if gated(next.as_ref(), &self.svc.account) => Transition::Cut(ask(next)),
            Transition::Replace(next) if gated(next.as_ref(), &self.svc.account) => Transition::Replace(ask(next)),
            other => other,
        };
        match self.router.apply(t) {
            Applied::Stay => false,
            Applied::Quit => !self.svc.platform.leave_app(),
        }
    }

    /// Hand a waiting join code to the multiplayer flow: from Home or the menu
    /// family it opens the join screen over the menu (the gate asks for My
    /// Signet first when needed). Over anything else (a match, the sign-in
    /// page) it waits a little; in a room it is dropped with a hint.
    fn route_join_link(&mut self, now: f64) {
        let Some((code, at)) = self.pending_link.clone() else { return };
        if now - at > LINK_WAIT {
            self.pending_link = None;
            return;
        }
        if self.router.input_blocked() {
            return;
        }
        let top = self.router.top().name();
        // A signed-out player is asked to sign in by the gate in `apply` once
        // the join screen is pushed; only wait while a sign-in is in flight.
        if self.svc.account.as_ref().is_some_and(|a| a.busy()) {
            return;
        }
        if crate::online::is_some() {
            self.pending_link = None;
            self.svc.toast = Some(Toast { text: "Leave your current game to join another.".into(), left: 3.5 });
            return;
        }
        if !LINK_OK_ON.contains(&top) {
            return;
        }
        self.pending_link = None;
        self.clear_touches();
        if top == "join" || top == "multiplayer" || top == "nearby" || top == "bluetooth" {
            // The screen on top takes the code itself.
            crate::ui::screens::online_flow::stash_join(code);
        } else {
            self.apply(crate::ui::screens::Transition::Push(Box::new(crate::ui::screens::multiplayer::MultiplayerScreen::joining(code))));
        }
    }

    // ── the frame ───────────────────────────────────────────────────────────

    /// Run one frame of `dt` seconds on a surface of `size_px` at OS scale
    /// `native_ppp`. `raw` is egui's input for this frame (from egui-winit,
    /// or `RawInput::default()` headless).
    pub fn frame(&mut self, dt: f32, size_px: (u32, u32), native_ppp: f32, mut raw: egui::RawInput) -> FrameOut {
        use crate::ui::screens::Transition;
        let t0 = std::time::Instant::now();
        let dt = dt.clamp(0.0, 0.1);
        self.set_surface(size_px, native_ppp);
        let ppp = self.ppp;
        self.time += dt as f64;
        self.frames += 1;
        self.svc.touches.time = self.time;
        let viewport = Viewport::new(egui::vec2(size_px.0 as f32 / ppp, size_px.1 as f32 / ppp), ppp, self.insets);
        let time = self.time;
        let mut quit = false;

        // Updates: the saved verdict, the launch check, and any download being followed.
        self.svc.updates.tick(dt, self.svc.platform.as_mut());
        self.account_upkeep();

        // Launch links (cold start or while running): `gameflock://join#CODE`.
        while let Some(link) = self.svc.platform.take_launch_link() {
            match crate::platform::parse_join_link(&link) {
                Some(code) => {
                    log::info!("launch link: join a room ({} chars)", code.len());
                    self.pending_link = Some((code, time));
                }
                None => log::info!("launch link ignored ({} chars)", link.len()),
            }
        }
        self.route_join_link(time);
        if let Some(t) = self.svc.toast.as_mut() {
            t.left -= dt;
        }
        if self.svc.toast.as_ref().is_some_and(|t| t.left <= 0.0) {
            self.svc.toast = None;
        }

        // System Back, before the screen's own logic.
        if std::mem::take(&mut self.back_requested) && !self.router.input_blocked() {
            let mut ctx = self.svc.ctx(dt, time, viewport, false);
            let t = self.router.top_mut().back(&mut ctx);
            if !t.is_none() {
                self.clear_touches();
            }
            quit |= self.apply(t);
        }

        // Logic.
        {
            let blocked = self.router.input_blocked();
            let mut ctx = self.svc.ctx(dt, time, viewport, blocked);
            let t = self.router.top_mut().update(&mut ctx);
            if !t.is_none() {
                self.clear_touches();
            }
            quit |= self.apply(t);
        }
        self.router.tick(dt);

        // UI.
        raw.screen_rect = Some(egui::Rect::from_min_size(Pos2::ZERO, viewport.size()));
        raw.viewports.entry(egui::ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
        if self.router.input_blocked() {
            // A fade-out holds input back from egui too (the old screen is still
            // drawn and live): only let go of anything held, off-screen.
            for e in self.egui_events.drain(..) {
                match e {
                    egui::Event::PointerButton { pressed: false, button, modifiers, .. } => raw.events.push(egui::Event::PointerButton { pos: OFFSCREEN, button, pressed: false, modifiers }),
                    egui::Event::PointerGone => raw.events.push(e),
                    _ => {}
                }
            }
        } else {
            raw.events.append(&mut self.egui_events);
        }
        // egui's clock is ours, not egui-winit's: that one restarts at 0 when
        // the window (and its egui_winit::State) is rebuilt on an Android
        // resume, and with time running backwards every Area's fade-in sat at
        // opacity 0 until the clock caught up again (seen in Twang Thang: buttons
        // gone after pause/resume).
        raw.time = Some(time);
        self.egui.begin_pass(raw);
        let clear;
        let t = {
            let blocked = self.router.input_blocked();
            let mut ctx = self.svc.ctx(dt, time, viewport, blocked);
            clear = self.router.top().clear_color(&ctx);
            self.router.top_mut().draw(&mut ctx, &self.egui)
        };
        if let Some(toast) = &self.svc.toast {
            draw_toast(&self.egui, &viewport, toast);
        }
        let alpha = self.router.fade_alpha();
        if alpha > 0.0 {
            let p = self.egui.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("flock-fade")));
            p.rect_filled(viewport.screen, 0.0, theme::BG.gamma_multiply(alpha));
        }
        let out = self.egui.end_pass();
        if !matches!(t, Transition::None) {
            self.clear_touches();
        }
        quit |= self.apply(t);

        // The online room runs whichever screen is up: drive it if no screen did.
        crate::online::pump_auto();
        crate::online::end_frame();

        self.sync_wake_lock();
        self.sync_room_service();
        self.sync_orientation();
        self.svc.touches.end_frame(dt);
        self.svc.keys.end_frame();
        self.last_ms = t0.elapsed().as_secs_f32() * 1000.0;
        FrameOut { egui: out, clear, viewport, quit }
    }
}

/// A rounded message near the bottom centre.
fn draw_toast(ctx: &egui::Context, vp: &Viewport, toast: &Toast) {
    let a = ((toast.left / 0.3).min(1.0)).clamp(0.0, 1.0);
    let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("flock-toast")));
    let galley = p.layout_no_wrap(toast.text.clone(), theme::ui_bold(15.0), theme::TEXT.gamma_multiply(a));
    let size = galley.size() + egui::vec2(28.0, 16.0);
    let rect = egui::Rect::from_center_size(pos2(vp.safe.center().x, vp.safe.bottom() - 36.0), size);
    p.rect_filled(rect, size.y / 2.0, theme::BG.gamma_multiply(0.92 * a));
    p.rect_stroke(rect, size.y / 2.0, egui::Stroke::new(1.5_f32, theme::ACCENT.gamma_multiply(a)), egui::StrokeKind::Inside);
    p.galley(rect.center() - galley.size() / 2.0, galley, theme::TEXT);
}

#[cfg(test)]
mod account_tests {
    use super::*;
    use crate::platform::os::RecordingPlatform;
    use crate::haptics::NoHaptics;
    use crate::ui::screens::{settings::SettingsScreen, Transition};

    fn signed_out_engine(tag: &str, first: Box<dyn Screen>) -> (Engine, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("gameflock-account-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut e = Engine::new(first, Profile::default(), dir.clone(), Options::default(),
            Box::new(RecordingPlatform::default()), Box::new(NoHaptics::default()));
        e.svc.account = Some(crate::account::Account::open(&dir));
        (e, dir)
    }

    #[test]
    fn signed_out_players_can_use_solo_screens_without_signing_in() {
        let _slot = crate::online::TEST_SLOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut e, dir) = signed_out_engine("solo", Box::new(SettingsScreen::new()));
        for _ in 0..30 { e.frame(1.0 / 60.0, (1080, 2400), 3.0, egui::RawInput::default()); }
        assert_eq!(e.router.names(), ["settings"]);
        e.apply(Transition::Push(Box::new(crate::ui::screens::practice::PracticeScreen::new())));
        for _ in 0..30 { e.frame(1.0 / 60.0, (1080, 2400), 3.0, egui::RawInput::default()); }
        assert_eq!(e.router.names(), ["settings", "practice"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn multiplayer_asks_a_signed_out_player_to_sign_in_and_back_returns() {
        let _slot = crate::online::TEST_SLOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let (mut e, dir) = signed_out_engine("mp", Box::new(SettingsScreen::new()));
        e.apply(Transition::Push(Box::new(crate::ui::screens::multiplayer::MultiplayerScreen::new())));
        for _ in 0..30 { e.frame(1.0 / 60.0, (1080, 2400), 3.0, egui::RawInput::default()); }
        assert_eq!(e.router.names(), ["settings", "signin"]);
        e.request_back();
        for _ in 0..30 { e.frame(1.0 / 60.0, (1080, 2400), 3.0, egui::RawInput::default()); }
        assert_eq!(e.router.names(), ["settings"]);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::platform::os::RecordingPlatform;
    use crate::haptics::NoHaptics;
    struct Plain;
    impl Screen for Plain {
        fn name(&self) -> &'static str { "plain-test" }
    }
    #[test]
    fn screen_off_silences_audio_without_frames_or_a_screen_background_hook() {
        let _slot = crate::online::TEST_SLOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("gameflock-screen-off-{}", std::process::id()));
        let profile = Profile { music_volume: 0.37, effects_volume: 0.64, ..Default::default() };
        let mut e = Engine::new(Box::new(Plain), profile, dir.clone(),
            Options { shot: Some("test".into()), ..Default::default() },
            Box::new(RecordingPlatform::default()), Box::new(NoHaptics::default()));
        crate::sound::init(true);
        e.on_foreground();
        assert!(!crate::sound::with(|a| a.is_suspended()));
        for _ in 0..2 { // Focus loss followed by surface suspension is idempotent.
            e.on_background();
            assert!(e.svc.muted);
            assert!(crate::sound::with(|a| a.is_suspended()));
        }
        crate::sound::with(|a| a.resume_all());
        assert!(crate::sound::with(|a| a.is_suspended()));
        e.on_foreground();
        assert!(!e.svc.muted);
        assert!(!crate::sound::with(|a| a.is_suspended()));
        assert_eq!(e.svc.profile.music_volume, 0.37);
        assert_eq!(e.svc.profile.effects_volume, 0.64);
        let _ = std::fs::remove_dir_all(dir);
    }
}

#[cfg(test)]
mod orientation_tests {
    use super::*;
    use crate::platform::os::{Orientation, RecordingPlatform};
    use crate::haptics::NoHaptics;
    use crate::ui::screens::{self, Transition};

    #[test]
    fn the_app_is_portrait_through_fades_and_resume() {
        let _slot = crate::online::TEST_SLOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut e = Engine::new(Box::new(screens::home::HomeScreen::new()), Profile::default(),
            std::env::temp_dir().join("gameflock-orientation"),
            Options { shot: Some("test".into()), ..Default::default() },
            Box::new(RecordingPlatform::default()), Box::new(NoHaptics::default()));
        e.sync_orientation();
        assert_eq!(e.orientation, Some(Orientation::Portrait));
        e.router.apply(Transition::Push(screens::by_name("settings").unwrap()));
        e.router.tick(1.0);
        e.sync_orientation();
        e.on_background();
        e.on_foreground();
        assert_eq!(e.orientation, Some(Orientation::Portrait));
        for (name, _) in screens::SCREENS {
            assert_eq!(screens::by_name(name).unwrap().orientation(), Orientation::Portrait, "{name}");
        }
    }
}

#[cfg(test)]
mod multiplayer_tests {
    use super::*;
    use crate::haptics::NoHaptics;
    use crate::platform::os::RecordingPlatform;
    use crate::ui::screens::{self, bluetooth::BluetoothScreen, join::JoinScreen, lobby::LobbyScreen, multiplayer::MultiplayerScreen, nearby::NearbyScreen};

    #[test]
    fn every_multiplayer_screen_is_behind_the_online_approval_gate() {
        let names = [MultiplayerScreen::new().name(), JoinScreen::new().name(), NearbyScreen::new().name(), BluetoothScreen::new().name(), LobbyScreen::new().name()];
        for n in names {
            assert!(ONLINE_SCREENS.contains(&n), "{n} is not in ONLINE_SCREENS");
        }
        // Solo, practice and settings never need it.
        for n in ["home", "settings", "run", "practice", "results"] {
            assert!(!ONLINE_SCREENS.contains(&n), "{n} must not need approval");
        }
    }

    #[test]
    fn a_join_link_opens_the_join_flow_over_home_and_never_over_a_room() {
        let _slot = crate::online::TEST_SLOT_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::online::leave();
        screens::online_flow::seam::reset();
        screens::online_flow::seam::SEAM.with(|s| s.borrow_mut().join_code = Some(Box::new(|_| Err("test: no network".into()))));
        let mut platform = RecordingPlatform::default();
        platform.launch.push_back("gameflock://join#kwtqpm".into());
        let dir = std::env::temp_dir().join(format!("gameflock-link-{}", std::process::id()));
        let mut e = Engine::new(Box::new(screens::home::HomeScreen::new()), Profile::default(), dir, Options { shot: Some("test".into()), ..Default::default() }, Box::new(platform), Box::new(NoHaptics::default()));
        for _ in 0..120 {
            e.frame(1.0 / 60.0, (1080, 2400), 2.625, egui::RawInput::default());
        }
        assert_eq!(e.router.names(), ["home", "multiplayer", "join"], "the link opened the join screen over the menu");
        screens::online_flow::seam::SEAM.with(|s| assert_eq!(s.borrow().joined, ["KWTQPM"], "and joined that room"));

        // In a room, a link is refused with a hint and opens nothing.
        crate::online::put(crate::online::OnlineSession::offline_host("A", 0, flock_session::HostConfig::new(1)));
        e.router = Router::new(Box::new(screens::home::HomeScreen::new()));
        e.svc.platform = {
            let mut p = RecordingPlatform::default();
            p.launch.push_back("gameflock://join#HDXNZB".into());
            Box::new(p)
        };
        e.frame(1.0 / 60.0, (1080, 2400), 2.625, egui::RawInput::default());
        assert_eq!(e.router.names(), ["home"]);
        assert_eq!(e.svc.toast.as_ref().map(|t| t.text.as_str()), Some("Leave your current game to join another."));
        crate::online::leave();
        screens::online_flow::seam::reset();
    }
}
