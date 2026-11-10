//! Settings: name (an in-game keypad, no OS text field), graphics, volumes,
//! haptics and left-handed. Changes apply live and are saved when the finger
//! lifts. With a My Signet account the first row opens the account page instead
//! of the keypad.
//!
//! Below the panel are the UPDATES and MORE GAMES cards (`settings_cards.rs`),
//! ported from Stampede's Settings. Stampede's relay list card was dropped with the
//! fork. FLOCK-TODO(session): bring a relay card back if multiplayer needs one.

use super::common;
use super::settings_cards::{self, CardAction};
use super::{Ctx, Screen, Transition};
use crate::platform::data_dir::{Quality, NAME_MAX};
use crate::ui::art::{ease_out_back, ease_out_cubic, phase, rgba};
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, Btn, Seg};
use crate::ui::{art, feedback, theme};
use egui::{pos2, vec2, Align2, Color32, Rect, Stroke};

/// Gap between the panel and the cards below it.
const GAP: f32 = 12.0;

/// What a staged screenshot shows in the UPDATES card.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Demo {
    None,
    /// A newer release is on offer.
    Offer,
    /// ...and its download is half way.
    Downloading,
}

pub struct SettingsScreen {
    t: f32,
    keypad: Option<Keypad>,
    dirty: bool,
    /// Taps on the version line (7 opens the debug panel).
    version_taps: u8,
    debug: bool,
    /// Scroll to the cards on the first frame (screenshots).
    to_cards: bool,
    demo: Demo,
    /// The first frame ran (staged state is set up once).
    started: bool,
}

struct Keypad {
    buf: String,
    shift: bool,
    /// Seconds open.
    age: f32,
}

impl Keypad {
    fn new(name: &str) -> Keypad {
        Keypad { buf: name.to_string(), shift: name.is_empty(), age: 0.0 }
    }

    fn press(&mut self, c: char) {
        if self.buf.chars().count() < NAME_MAX {
            self.buf.push(if self.shift { c.to_ascii_uppercase() } else { c.to_ascii_lowercase() });
            self.shift = self.buf.ends_with(' ');
        }
    }

    fn backspace(&mut self) {
        self.buf.pop();
        if self.buf.is_empty() || self.buf.ends_with(' ') {
            self.shift = true;
        }
    }
}

impl Default for SettingsScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsScreen {
    pub fn new() -> SettingsScreen {
        SettingsScreen { t: 0.0, keypad: None, dirty: false, version_taps: 0, debug: false, to_cards: false, demo: Demo::None, started: false }
    }

    /// Settings scrolled to the cards (screenshots).
    pub fn at_cards() -> SettingsScreen {
        SettingsScreen { to_cards: true, ..SettingsScreen::new() }
    }

    /// A newer release on offer in the UPDATES card (screenshots).
    pub fn updates_sample(downloading: bool) -> SettingsScreen {
        SettingsScreen { to_cards: true, demo: if downloading { Demo::Downloading } else { Demo::Offer }, ..SettingsScreen::new() }
    }

    /// The debug panel open (screenshots).
    pub fn debug_sample() -> SettingsScreen {
        let mut s = SettingsScreen::new();
        s.debug = true;
        s
    }

    /// The name keypad open (screenshots).
    pub fn keypad_sample() -> SettingsScreen {
        let mut s = SettingsScreen::new();
        s.keypad = Some(Keypad { buf: "Fluffy".into(), shift: false, age: 1.0 });
        s
    }
}

impl SettingsScreen {
    /// Act on a tap in one of the cards. Returns a screen change, if any.
    fn card_action(&mut self, ctx: &mut Ctx, a: CardAction) -> Option<Transition> {
        match a {
            CardAction::CheckUpdates => {
                feedback::tap(ctx);
                ctx.updates.check_now();
            }
            CardAction::InstallUpdate => {
                feedback::select(ctx);
                if let Err(notice) = ctx.updates.install(&mut *ctx.platform) {
                    ctx.say(&notice);
                }
            }
            CardAction::CancelUpdate => {
                feedback::tap(ctx);
                ctx.updates.cancel_install(&mut *ctx.platform);
            }
            CardAction::MoreGames => {
                feedback::tap(ctx);
                ctx.save_profile();
                return Some(Transition::Push(Box::new(super::games::GamesScreen::new())));
            }
        }
        None
    }
}

impl Screen for SettingsScreen {
    fn name(&self) -> &'static str {
        "settings"
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        common::begin(ctx, &mut self.t);
        if !std::mem::replace(&mut self.started, true) && self.demo != Demo::None && ctx.opts.shot.is_some() {
            // Staged screenshots only: a throwaway-signed offer, never on a real run.
            ctx.updates.demo_offer("0.3.0", "Faster starts, a new track and fixes.");
            if self.demo == Demo::Downloading {
                ctx.updates.demo_install_state(crate::platform::os::ApkStatus::Downloading { done: 12_400_000, total: 27_700_000 });
            }
        }
        if let Some(k) = self.keypad.as_mut() {
            k.age += ctx.dt;
        }
        // save once the finger lifts after a change
        if self.dirty && ctx.touches.active().is_empty() {
            self.dirty = false;
            ctx.save_profile();
        }
        Transition::None
    }

    fn back(&mut self, ctx: &mut Ctx) -> Transition {
        feedback::back(ctx);
        if self.keypad.take().is_some() {
            return Transition::None;
        }
        ctx.save_profile();
        Transition::Pop
    }

    fn on_background(&mut self, ctx: &mut Ctx) {
        ctx.save_profile();
        feedback::on_background();
    }

    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        let safe = ctx.viewport.safe;
        let (t, tm) = (self.t, ctx.time as f32);
        let mut out = Transition::None;
        let mut dirty = false;
        let mut debug_toggle = false;
        let debug_open = self.debug;
        let mut keypad = self.keypad.take();
        let keypad_open = keypad.is_some();
        let mut to_cards = std::mem::take(&mut self.to_cards);
        let mut card_act: Option<CardAction> = None;
        let upd = ctx.updates.view(crate::update::unix_now());
        let screen_rect = ctx.viewport.screen;
        common::page(ctx.viewport.screen, tm, ui, 0.55, |ui, _| {
            let hd = widgets::header(ui, safe, "Settings", widgets::enter(t, 0));
            if hd.back.clicked() && !keypad_open {
                feedback::back(ctx);
                ctx.save_profile();
                out = Transition::Pop;
            }
            let body_area = hd.body;
            let rh = 44.0;
            let label_w = 118.0;
            let rows = 8usize;
            let panel_h = 20.0 + rows as f32 * (rh + 4.0);
            let drows = 10usize;
            let debug_h = 20.0 + drows as f32 * (rh + 4.0) + 30.0;
            // The panel keeps its natural height; the cards follow below it, then the version line.
            let upd_h = settings_cards::updates_height(ui.painter(), body_area.width(), &upd);
            let more_h = settings_cards::more_games_height();
            let cards_h = upd_h + GAP + more_h;
            let content_h = panel_h + GAP + cards_h + 36.0 + if debug_open { 12.0 + debug_h } else { 0.0 } + 8.0;
            common::scroll(ui, body_area, "settings-scroll", content_h, |ui, body| {
            let panel = Rect::from_min_size(body.min, vec2(body.width(), panel_h));

            // a row's label and hairline; returns the control rect
            let row = |p: &egui::Painter, i: usize, label: &str, sub: Option<&str>| -> Rect {
                let inner = panel.shrink2(vec2(16.0, 10.0));
                let r = Rect::from_min_size(pos2(inner.left(), inner.top() + i as f32 * (rh + 4.0)), vec2(inner.width(), rh));
                if let Some(sub) = sub {
                    p.text(pos2(r.left(), r.center().y - 7.0), Align2::LEFT_CENTER, label, theme::ui_bold(14.0), theme::TEXT);
                    p.text(pos2(r.left(), r.center().y + 9.0), Align2::LEFT_CENTER, sub, theme::ui_medium(10.5), theme::TEXT_DIM);
                } else {
                    p.text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, label, theme::ui_bold(14.0), theme::TEXT);
                }
                if i > 0 {
                    p.hline((r.left())..=(r.right()), r.top() - 2.0, Stroke::new(1.0_f32, rgba(255, 255, 255, 0.07)));
                }
                Rect::from_min_max(pos2(r.left() + label_w, r.top() + 4.0), pos2(r.right(), r.bottom() - 4.0))
            };

            widgets::staged(ui, widgets::enter(t, 1), vec2(0.0, 24.0), |ui, off| {
                let p = ui.painter().clone();
                let panel_at = panel.translate(off);
                widgets::panel(&p, panel_at, 20.0);
                let row = |i: usize, label: &str, sub: Option<&str>| row(&p, i, label, sub).translate(off);
                // name / account
                let c = row(0, if ctx.account.is_some() { "My Signet" } else { "Name" }, None);
                let name = ctx.profile.name.clone();
                let b = widgets::button(ui, c.shrink2(vec2(0.0, -2.0)), "set-name", &Btn::new(&name).size(17.0));
                p.text(pos2(c.right() - 14.0, c.center().y), Align2::RIGHT_CENTER, if ctx.account.is_some() { "account" } else { "edit" }, theme::ui_bold(10.5), theme::TEXT_DIM);
                if b.clicked() && !keypad_open {
                    feedback::tap(ctx);
                    if ctx.account.is_some() {
                        out = Transition::Push(Box::new(super::account::AccountScreen::manage()));
                    } else {
                        keypad = Some(Keypad::new(&ctx.profile.name));
                    }
                }
                // graphics
                let c = row(1, "Graphics", None);
                let segs = [Seg::new("Low"), Seg::new("Balanced"), Seg::new("High")];
                let sel = match ctx.profile.quality {
                    Quality::Low => 0,
                    Quality::Balanced => 1,
                    Quality::High => 2,
                };
                if let Some(i) = widgets::segmented(ui, c, "set-quality", &segs, sel) {
                    if !keypad_open {
                        feedback::tap(ctx);
                        ctx.profile.quality = [Quality::Low, Quality::Balanced, Quality::High][i];
                        dirty = true;
                    }
                }
                // volumes
                for (i, (label, salt)) in [("Music", "set-music"), ("Effects", "set-fx")].into_iter().enumerate() {
                    let c = row(2 + i, label, None);
                    let mut v = if i == 0 { ctx.profile.music_volume } else { ctx.profile.effects_volume };
                    let sl = Rect::from_min_max(pos2(c.left() - 6.0, c.top() - 4.0), pos2(c.right() - 44.0, c.bottom() + 4.0));
                    let resp = widgets::slider(ui, sl, salt, &mut v);
                    p.text(pos2(c.right(), c.center().y), Align2::RIGHT_CENTER, format!("{}%", (v * 100.0).round() as i32), theme::numbers(15.0), theme::SAND);
                    if resp.changed() && !keypad_open {
                        if i == 0 {
                            ctx.profile.music_volume = v;
                        } else {
                            ctx.profile.effects_volume = v;
                        }
                        dirty = true;
                    }
                    if resp.drag_stopped() && i == 1 && !keypad_open {
                        feedback::tap(ctx);
                    }
                }
                // switches
                let switch = |ui: &mut egui::Ui, i: usize, label: &str, sub: &str, salt: &str, v: &mut bool| -> bool {
                    let c = row(i, label, Some(sub));
                    let r = Rect::from_min_max(pos2(panel_at.left() + 16.0, c.top() - 4.0), pos2(panel_at.right() - 16.0, c.bottom() + 4.0));
                    widgets::toggle(ui, r, salt, v).changed() && !keypad_open
                };
                let mut v = ctx.profile.reduced_shake;
                if switch(ui, 4, "Reduced shake", "A gentler camera on hits", "set-shake", &mut v) {
                    ctx.profile.reduced_shake = v;
                    feedback::tap(ctx);
                    dirty = true;
                }
                let mut v = ctx.profile.left_handed;
                if switch(ui, 5, "Left-handed", "Ghost toolbar on the left edge", "set-left", &mut v) {
                    feedback::tap(ctx);
                    ctx.profile.left_handed = v;
                    dirty = true;
                }
                let mut v = ctx.profile.haptics;
                if switch(ui, 6, "Haptics", "Vibration on hits, coins and taps", "set-haptics", &mut v) {
                    ctx.profile.haptics = v;
                    ctx.haptics.set_enabled(v);
                    feedback::tap(ctx);
                    // a taste of it when turning on
                    ctx.haptics.pulse(crate::haptics::Pulse::Boost);
                    dirty = true;
                }
                // first-run tips
                let c = row(7, "Tips", Some("The swipe hints on your first run"));
                let br = Rect::from_min_size(pos2(c.right() - 150.0, c.top() + 2.0), vec2(150.0, c.height() - 4.0));
                if widgets::button(ui, br, "set-tips", &Btn::new("Show tips again").size(14.0)).clicked() && !keypad_open {
                    feedback::tap(ctx);
                    ctx.profile.seen_intro = false;
                    ctx.say("Tips will show on your next run");
                    dirty = true;
                }
            });

            // ── the cards: UPDATES, then MORE GAMES ──
            let upd_card = Rect::from_min_size(pos2(body.left(), body.top() + panel_h + GAP), vec2(body.width(), upd_h));
            let more_card = Rect::from_min_size(pos2(body.left(), upd_card.bottom() + GAP), vec2(body.width(), more_h));
            if std::mem::take(&mut to_cards) {
                ui.scroll_to_rect(upd_card.expand2(vec2(0.0, 8.0)), Some(egui::Align::TOP));
            }
            if !keypad_open {
                settings_cards::draw_updates(ui, body_area, upd_card, &upd, tm, &mut card_act);
                settings_cards::draw_more_games(ui, body_area, more_card, &mut card_act);
            } else {
                // Same look, no taps while the keypad is up.
                let mut ignored = None;
                settings_cards::draw_updates(ui, body_area, upd_card, &upd, tm, &mut ignored);
                settings_cards::draw_more_games(ui, body_area, more_card, &mut ignored);
            }

            // version line: seven taps open the debug panel
            let vr = Rect::from_center_size(pos2(body.center().x, more_card.bottom() + 20.0 + 0.0), vec2(220.0, 32.0));
            let resp = ui.interact(vr, egui::Id::new("set-version"), egui::Sense::click());
            let label = format!("Cluster Flock {}", crate::version_label());
            ui.painter().text(vr.center() + vec2(0.0, 1.0), Align2::CENTER_CENTER, &label, theme::ui_medium(11.0), Color32::from_black_alpha(140));
            ui.painter().text(vr.center(), Align2::CENTER_CENTER, &label, theme::ui_medium(11.0), theme::TEXT);
            if resp.clicked() && !keypad_open {
                self.version_taps = self.version_taps.saturating_add(1);
                if self.version_taps == 7 {
                    debug_toggle = true;
                }
            }
            if debug_open {
                let dp = Rect::from_min_size(pos2(body.left(), more_card.bottom() + 36.0 + 12.0), vec2(body.width(), debug_h));
                draw_debug(ctx, ui, dp, &mut out);
            }
            });

            // ── the keypad (name) ──
            ui.set_clip_rect(screen_rect);
            if let Some(k) = keypad.as_mut() {
                let mut close = None;
                draw_keypad(ctx, ui, safe, k, &mut close);
                match close {
                    Some(true) => {
                        let n = k.buf.trim().to_string();
                        ctx.profile.name = if n.is_empty() { ctx.profile.fallback_name() } else { n };
                        dirty = true;
                        feedback::select(ctx);
                        keypad = None;
                    }
                    Some(false) => {
                        feedback::back(ctx);
                        keypad = None;
                    }
                    None => {}
                }
            }
        });
        self.keypad = keypad;
        self.dirty |= dirty;
        if let Some(a) = card_act {
            if let Some(t) = self.card_action(ctx, a) {
                return t;
            }
        }
        if debug_toggle {
            self.debug = true;
            ctx.say("Debug panel on");
        }
        out
    }
}

/// The modal keypad: the name field, digits, QWERTY, shift, backspace, space, Done.
fn draw_keypad(ctx: &mut Ctx, ui: &mut egui::Ui, safe: Rect, k: &mut Keypad, close: &mut Option<bool>) {
    let e = ease_out_back(phase(k.age, 0.0, 0.32)).min(1.04);
    let p0 = ui.painter().clone();
    p0.rect_filled(ctx.viewport.screen, 0.0, rgba(6, 2, 14, 0.82 * ease_out_cubic(phase(k.age, 0.0, 0.2))));
    widgets::staged(ui, phase(k.age, 0.0, 0.2), vec2(0.0, 0.0), |ui, _| {
        let w = (safe.width() - 16.0).min(640.0);
        let h = (safe.height() - 16.0).min(340.0);
        let card = Rect::from_min_size(pos2(safe.center().x - w / 2.0, safe.bottom() - 8.0 - h + (1.0 - e.min(1.0)) * 30.0), vec2(w, h));
        let p = ui.painter().clone();
        widgets::panel_solid(&p, card, 22.0);
        let inner = card.shrink(12.0);
        // the field
        let field_h = 48.0;
        let field = Rect::from_min_size(inner.min, vec2(inner.width() - 96.0, field_h));
        widgets::well(&p, field, 14.0);
        let caret_on = (ctx.time * 2.0).fract() < 0.6;
        let tx = field.left() + 14.0;
        let max = NAME_MAX;
        {
            let name_size = art::fit_size(&p, &k.buf, theme::title, 28.0, field.width() - 66.0);
            let g = p.layout_no_wrap(k.buf.clone(), theme::title(name_size), theme::TEXT);
            art::outlined(&p, pos2(tx, field.center().y), Align2::LEFT_CENTER, &k.buf, theme::title(name_size), theme::TEXT, theme::OUTLINE, 2.0, 0.0);
            if caret_on && k.buf.chars().count() < max {
                let cx = tx + g.size().x + 3.0;
                p.rect_filled(Rect::from_center_size(pos2(cx, field.center().y), vec2(3.0, 28.0)), 1.5, theme::ACCENT);
            }
        }
        p.text(pos2(field.right() - 10.0, field.center().y), Align2::RIGHT_CENTER, format!("{}/{}", k.buf.chars().count(), max), theme::ui_bold(11.5), theme::TEXT_DIM);
        // cancel
        let cancel = Rect::from_min_size(pos2(inner.right() - 88.0, inner.top()), vec2(88.0, field_h));
        if widgets::button(ui, cancel, "kp-cancel", &Btn::ghost("Cancel").size(15.0)).clicked() {
            *close = Some(false);
        }
        // keys
        let rows = ["1234567890", "QWERTYUIOP", "ASDFGHJKL", "ZXCVBNM"];
        let gap = if common::Layout::new(safe).portrait { 3.0 } else { 6.0 };
        let top = inner.top() + field_h + 10.0;
        let kh = ((inner.bottom() - top) - gap * 4.0) / 5.0;
        let kw = (inner.width() - gap * 9.0) / 10.0;
        for (ri, keys) in rows.iter().enumerate() {
            let n = keys.chars().count();
            let mut x0 = inner.left() + (inner.width() - (n as f32 * kw + (n as f32 - 1.0) * gap)) / 2.0;
            if ri == 3 {
                // shift and backspace flank the last row
                let side = kw * 1.5;
                let total = side * 2.0 + n as f32 * kw + (n as f32 + 1.0) * gap;
                x0 = inner.left() + (inner.width() - total) / 2.0;
                let y = top + ri as f32 * (kh + gap);
                let sh = Rect::from_min_size(pos2(x0, y), vec2(side, kh));
                if widgets::button(ui, sh, "kp-shift", &Btn::new("").icon(Icon::Jump).selected(k.shift)).clicked() {
                    feedback::tap(ctx);
                    k.shift = !k.shift;
                }
                x0 += side + gap;
                let bs = Rect::from_min_size(pos2(x0 + n as f32 * (kw + gap), y), vec2(side, kh));
                if widgets::button(ui, bs, "kp-bs", &Btn::new("").icon(Icon::Backspace)).clicked() {
                    feedback::tap(ctx);
                    k.backspace();
                }
            }
            for (ci, c) in keys.chars().enumerate() {
                let r = Rect::from_min_size(pos2(x0 + ci as f32 * (kw + gap), top + ri as f32 * (kh + gap)), vec2(kw, kh));
                let shown = if c.is_ascii_digit() || k.shift { c } else { c.to_ascii_lowercase() };
                if widgets::button(ui, r, &format!("kp-{c}"), &Btn::new(&shown.to_string()).size(21.0)).clicked() {
                    feedback::tap(ctx);
                    k.press(c);
                }
            }
        }
        // space and done
        let y = top + 4.0 * (kh + gap);
        let portrait = common::Layout::new(safe).portrait;
        let done_w = if portrait { inner.width() * 0.30 } else { 190.0 };
        let space_x = inner.left() + if portrait { 0.0 } else { kw * 1.5 + gap };
        let space_w = inner.width() - done_w - gap - if portrait { 0.0 } else { 2.0 * (kw * 1.5 + gap) };
        let space = Rect::from_min_size(pos2(space_x, y), vec2(space_w, kh));
        if widgets::button(ui, space, "kp-space", &Btn::new("space").size(15.0)).clicked() {
            feedback::tap(ctx);
            if !k.buf.is_empty() && !k.buf.ends_with(' ') && k.buf.chars().count() < NAME_MAX {
                k.buf.push(' ');
                k.shift = true;
            }
        }
        let done = Rect::from_min_size(pos2(space.right() + gap, y), vec2(done_w, kh));
        if widgets::button(ui, done, "kp-done", &Btn::primary("Done").icon(Icon::Check).size(22.0)).clicked() {
            *close = Some(true);
        }
    });
}

/// The hidden debug panel (session-only switches, see `game::debug`).
fn draw_debug(ctx: &mut Ctx, ui: &mut egui::Ui, panel: Rect, out: &mut Transition) {
    use crate::ui::game::debug::{self, DebugAction};
    let p = ui.painter().clone();
    widgets::panel(&p, panel, 20.0);
    let rh = 44.0;
    let inner = panel.shrink2(vec2(16.0, 10.0));
    p.text(pos2(panel.center().x, panel.bottom() - 15.0), Align2::CENTER_CENTER, "debug: not saved, this session only", theme::ui_medium(10.5), theme::TEXT_DIM);
    let row_rect = |i: usize| Rect::from_min_size(pos2(inner.left(), inner.top() + i as f32 * (rh + 4.0)), vec2(inner.width(), rh));
    let mut d = debug::with(|d| d.clone());
    let label = |i: usize, text: &str| {
        let r = row_rect(i);
        p.text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, text, theme::ui_bold(14.0), theme::TEXT);
        if i > 0 {
            p.hline(r.left()..=r.right(), r.top() - 2.0, Stroke::new(1.0_f32, rgba(255, 255, 255, 0.07)));
        }
    };
    // fixed seed with a stepper
    label(0, "Fixed seed");
    let r = row_rect(0);
    if widgets::toggle(ui, r, "dbg-seedon", &mut d.fixed_seed).changed() {
        feedback::tap(ctx);
    }
    let sr = Rect::from_min_size(pos2(inner.left(), row_rect(1).top()), vec2(inner.width(), rh));
    p.text(pos2(sr.left(), sr.center().y), Align2::LEFT_CENTER, "Seed", theme::ui_bold(14.0), theme::TEXT);
    let minus = Rect::from_min_size(pos2(sr.right() - 150.0, sr.top() + 4.0), vec2(40.0, 36.0));
    let plus = Rect::from_min_size(pos2(sr.right() - 40.0, sr.top() + 4.0), vec2(40.0, 36.0));
    if widgets::button(ui, minus, "dbg-seed-", &Btn::new("").icon(Icon::Minus)).clicked() {
        d.seed = d.seed.saturating_sub(1).max(1);
    }
    p.text(pos2((minus.right() + plus.left()) / 2.0, sr.center().y), Align2::CENTER_CENTER, format!("{}", d.seed), theme::numbers(17.0), theme::SAND);
    if widgets::button(ui, plus, "dbg-seed+", &Btn::new("").icon(Icon::Plus)).clicked() {
        d.seed += 1;
    }
    let toggles: [(&str, &str, fn(&mut debug::DebugState) -> &mut bool); 5] = [
        ("Slow motion", "dbg-slow", |d| &mut d.slow_mo),
        ("Collision boxes", "dbg-boxes", |d| &mut d.boxes),
        ("Protection status", "dbg-prot", |d| &mut d.protection),
        ("Pack bounds", "dbg-pack", |d| &mut d.pack),
        ("Placement cells", "dbg-place", |d| &mut d.placement),
    ];
    for (i, (text, salt, f)) in toggles.iter().enumerate() {
        label(2 + i, text);
        let mut v = *f(&mut d);
        if widgets::toggle(ui, row_rect(2 + i), salt, &mut v).changed() {
            *f(&mut d) = v;
            feedback::tap(ctx);
        }
    }
    let btn = |i: usize| row_rect(i).shrink2(vec2(0.0, 2.0));
    if widgets::button(ui, btn(7), "dbg-elim", &Btn::new("Force-eliminate me (next run)").size(15.0)).clicked() {
        d.pending.push(DebugAction::ForceEliminateMe);
        ctx.say("You will be eliminated at the start of the next run");
    }
    if widgets::button(ui, btn(8), "dbg-six", &Btn::new("Six in one lane").size(16.0)).clicked() {
        feedback::select(ctx);
        *out = Transition::Push(Box::new(super::run::RunScreen::debug_test(0)));
    }
    if widgets::button(ui, btn(9), "dbg-ghosts", &Btn::new("One runner, five ghosts").size(16.0)).clicked() {
        feedback::select(ctx);
        *out = Transition::Push(Box::new(super::run::RunScreen::debug_test(1)));
    }
    debug::with(|s| *s = d);
}
