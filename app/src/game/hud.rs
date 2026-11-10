//! The Run HUD: pure layout ([`layout`], unit-tested at phone sizes) and the
//! painters. Everything lives inside the safe rectangle and stays out of the
//! way of the course: slim panels along the top, the ghost toolbar down one
//! thumb edge.

use crate::ui::art::{self, ease_out_back, fade, phase, rgba};
use crate::ui::icons::{self, Icon};
use crate::ui::{theme, widgets};
use egui::{pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Shape, Stroke};

pub const MARGIN: f32 = 12.0;
pub const ROW: f32 = 44.0;
pub const TOOL_W: f32 = 70.0;
pub const TOOL_H: f32 = 76.0;
pub const TOOL_GAP: f32 = 8.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    /// Solo: distance + best. Otherwise time left + living count.
    pub solo: bool,
    /// Show the pause button (offline runs).
    pub pause: bool,
    /// The ghost toolbar is up.
    pub ghost: bool,
    pub left_handed: bool,
}

/// Every HUD rectangle for a frame.
#[derive(Clone, Copy, Debug)]
pub struct Rects {
    pub feathers: Rect,
    pub coins: Rect,
    /// Protection chip (ring, SAFE / CAREFUL and the free-hit pip), under the coins. The
    /// power-up chips stack below it (see [`Rects::slot`]).
    pub prot: Rect,
    /// Big number: distance (solo) or time left.
    pub main: Rect,
    /// Small line under it: best (solo) or living count.
    pub sub: Rect,
    pub pause: Option<Rect>,
    // ghost
    pub energy: Rect,
    pub banner: Rect,
    pub tools: [Rect; 4],
    pub info: Rect,
}

impl Rects {
    /// The `k`th chip in the column under the coin counter (0 = the protection chip's place).
    pub fn slot(&self, k: usize) -> Rect {
        self.prot.translate(vec2(0.0, k as f32 * (self.prot.height() + 6.0)))
    }

    /// Rectangles that take taps (swipes may not start inside them).
    pub fn controls(&self, m: Mode) -> Vec<Rect> {
        let mut v = Vec::new();
        if let Some(p) = self.pause {
            v.push(p.expand(6.0));
        }
        if m.ghost {
            v.extend(self.tools.iter().map(|r| r.expand(4.0)));
        }
        v
    }
}

/// Lay the HUD out inside `safe`.
pub fn layout(safe: Rect, m: Mode) -> Rects {
    let l = safe.left() + MARGIN;
    let r = safe.right() - MARGIN;
    let top = safe.top() + 8.0;
    let pause = m.pause.then(|| Rect::from_min_size(pos2(r - 44.0, top), vec2(44.0, 44.0)));
    let right_edge = pause.map_or(r, |p| p.left() - 8.0);
    // top row: feathers left, the big number centred in what is left
    let feathers = Rect::from_min_size(pos2(l, top), vec2(104.0, ROW));
    let main_w = 116.0;
    // centred on the screen when there is room (the clamp below squeezes it aside otherwise)
    let main_x = safe.center().x;
    let main_x = main_x.clamp(feathers.right() + 8.0 + main_w / 2.0, (right_edge - main_w / 2.0).max(feathers.right() + 8.0 + main_w / 2.0));
    let main = Rect::from_center_size(pos2(main_x, top + ROW / 2.0), vec2(main_w, ROW));
    let row2 = top + ROW + 6.0;
    let coins = Rect::from_min_size(pos2(l, row2), vec2(96.0, 30.0));
    let prot = Rect::from_min_size(pos2(l, coins.bottom() + 6.0), vec2(108.0, 30.0));
    let sub = Rect::from_center_size(pos2(main.center().x, row2 + 15.0), vec2(main_w, 28.0));
    // ghost layer
    let energy = Rect::from_min_size(pos2(l, top), vec2(right_edge - l, 40.0));
    let banner = Rect::from_min_size(pos2(l, top + 48.0), vec2(r - l, 34.0));
    let col_x = if m.left_handed { l } else { r - TOOL_W };
    let total = 4.0 * TOOL_H + 3.0 * TOOL_GAP;
    let y0 = safe.bottom() - MARGIN - total;
    let tools = std::array::from_fn(|i| Rect::from_min_size(pos2(col_x, y0 + i as f32 * (TOOL_H + TOOL_GAP)), vec2(TOOL_W, TOOL_H)));
    let (ix0, ix1) = if m.left_handed { (l + TOOL_W + 8.0, r) } else { (l, r - TOOL_W - 8.0) };
    let info = Rect::from_min_max(pos2(ix0, safe.bottom() - MARGIN - 58.0), pos2(ix1, safe.bottom() - MARGIN));
    Rects { feathers, coins, prot, main, sub, pause, energy, banner, tools, info }
}

/// Animation state of the HUD.
#[derive(Default)]
pub struct HudState {
    /// Feather icons: seconds since each was lost (None = still there).
    lost: [Option<f32>; 3],
    shown_feathers: Option<u8>,
    pub coin_shown: f32,
    coin_pulse: f32,
    /// "+2" pops while the Doubler runs: seconds since each.
    pops: Vec<f32>,
    doubler: bool,
    last_coins: Option<u32>,
    pub age: f32,
    hurt: f32,
    pub best_flash: f32,
}

impl HudState {
    pub fn new() -> HudState {
        HudState::default()
    }

    /// Tell the HUD whether the Doubler is running (before [`HudState::update`]).
    pub fn set_doubler(&mut self, on: bool) {
        self.doubler = on;
    }

    /// Advance by `dt`; `feathers` is the local runner's count, `coins` its coins.
    pub fn update(&mut self, dt: f32, feathers: u8, max: u8, coins: u32) {
        self.age += dt;
        if self.last_coins.is_some_and(|c| coins > c) && self.doubler {
            self.pops.push(0.0);
        }
        self.last_coins = Some(coins);
        for a in self.pops.iter_mut() {
            *a += dt;
        }
        self.pops.retain(|a| *a < 0.7);
        let prev = self.shown_feathers.unwrap_or(feathers);
        if feathers < prev {
            for i in feathers..prev.min(3) {
                self.lost[i as usize] = Some(0.0);
            }
            self.hurt = 1.0;
        }
        self.shown_feathers = Some(feathers);
        for l in self.lost.iter_mut().flatten() {
            *l += dt;
        }
        for (i, l) in self.lost.iter_mut().enumerate() {
            if (i as u8) < feathers.min(max) {
                *l = None;
            }
        }
        self.hurt = (self.hurt - dt * 2.2).max(0.0);
        // coins count up (never overshoots)
        let diff = coins as f32 - self.coin_shown;
        if diff.abs() < 0.01 {
            self.coin_shown = coins as f32;
        } else {
            let before = self.coin_shown.round();
            let step = (diff.abs() * (1.0 - (-dt / 0.15).exp()) + dt * 8.0).min(diff.abs());
            self.coin_shown += step * diff.signum();
            if self.coin_shown.round() != before {
                self.coin_pulse = 1.0;
            }
        }
        self.coin_pulse = (self.coin_pulse - dt * 5.0).max(0.0);
    }

    pub fn coin_count(&self) -> u32 {
        self.coin_shown.round().max(0.0) as u32
    }
}

/// What the runner HUD shows.
pub struct Data {
    pub feathers: u8,
    pub max_feathers: u8,
    pub coins: u32,
    pub distance_m: f32,
    pub best_m: Option<u32>,
    /// Multi: seconds left in the round.
    pub time_left: Option<f32>,
    pub living: usize,
    pub total: usize,
    /// Protection time left 0..1.
    pub protected: Option<f32>,
    /// The protection episode's one free hit has not been used yet.
    pub free_hit: bool,
    /// Active power-ups (Magnet, Doubler, Shield): (fraction of time left, seconds left).
    pub powers: [Option<(f32, f32)>; 3],
    pub solo: bool,
    /// Local player is out and watching (ghost) - the runner HUD is dimmed.
    pub ghost: bool,
}

/// The CAREFUL chip's amber.
const AMBER: Color32 = Color32::from_rgb(0xFF, 0xB0, 0x2E);
/// Chip colours: Magnet red, Doubler violet, Shield green.
pub const POWER_COL: [Color32; 3] = [Color32::from_rgb(0xF2, 0x45, 0x3A), Color32::from_rgb(0x9C, 0x84, 0xFF), Color32::from_rgb(0x3C, 0xE0, 0x6A)];

/// A tiny painted icon for power-up `k` (0 Magnet, 1 Doubler, 2 Shield) centred on `c`.
fn power_icon(p: &Painter, k: usize, c: Pos2, col: Color32) {
    match k {
        0 => {
            // a horseshoe: arch on top, legs down, white tips
            let r = 4.6;
            let pts: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let a = std::f32::consts::PI * i as f32 / 10.0;
                    c + vec2(a.cos() * r, -1.0 - a.sin() * r)
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new(3.2_f32, col)));
            for sx in [-1.0f32, 1.0] {
                p.line_segment([c + vec2(sx * r, -1.0), c + vec2(sx * r, 3.0)], Stroke::new(3.2_f32, col));
                p.line_segment([c + vec2(sx * r, 3.0), c + vec2(sx * r, 5.2)], Stroke::new(3.2_f32, Color32::WHITE));
            }
        }
        1 => {
            p.text(c + vec2(0.0, 0.5), Align2::CENTER_CENTER, "x2", theme::ui_bold(11.0), col);
        }
        _ => icons::draw(p, Icon::Shield, c, 12.0, col),
    }
}

fn glass(p: &Painter, r: Rect) {
    p.rect_filled(r, r.height() / 2.0, rgba(14, 6, 30, 0.74));
    p.rect_stroke(r, r.height() / 2.0, Stroke::new(1.0_f32, rgba(255, 255, 255, 0.12)), egui::StrokeKind::Inside);
}

/// An arc ring: `frac` of a full circle clockwise from the top.
pub fn ring(p: &Painter, c: Pos2, r: f32, frac: f32, w: f32, col: Color32) {
    p.circle_stroke(c, r, Stroke::new(w, rgba(0, 0, 0, 0.35)));
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.0 {
        return;
    }
    let n = ((frac * 36.0).ceil() as usize).max(2);
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + frac * std::f32::consts::TAU * i as f32 / n as f32;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(Shape::line(pts, Stroke::new(w, col)));
}

/// Draw the runner HUD.
pub fn draw_runner(p: &Painter, brand: &art::Brand, rc: &Rects, st: &HudState, d: &Data, time: f32) {
    let dim = if d.ghost { 0.55 } else { 1.0 };
    // ---- feathers
    let shake = (st.hurt * (time * 60.0).sin() * 3.0).abs() * st.hurt;
    let fr = rc.feathers.translate(vec2(shake, 0.0));
    glass(p, fr);
    let n = d.max_feathers.min(3) as usize;
    let step = 30.0;
    let x0 = fr.left() + 12.0 + 14.0;
    for i in 0..n {
        let c = pos2(x0 + i as f32 * step, fr.center().y);
        let alive = (i as u8) < d.feathers;
        if alive {
            let pop = 1.0 + 0.0 * time;
            icons::draw(p, Icon::Feather, c, 28.0 * pop, fade(theme::GOLD, dim));
        } else if let Some(t) = st.lost[i].filter(|t| *t < 0.7) {
            let k = t / 0.7;
            icons::draw(p, Icon::Feather, c + vec2(k * 6.0 * (i as f32 - 1.0), k * 26.0), 28.0 * (1.0 + 0.5 * k), fade(theme::LASER, (1.0 - k) * dim));
        } else {
            icons::draw(p, Icon::Feather, c, 24.0, rgba(255, 255, 255, 0.16));
        }
    }
    // protection chip under the coins
    let mut slot = 0;
    if let Some(frac) = d.protected {
        let pr = rc.slot(slot);
        slot += 1;
        glass(p, pr);
        // one free hit per episode: SAFE with a full pip, then CAREFUL (amber) once it is spent
        let col = if d.free_hit { theme::MINT } else { AMBER };
        let c = pos2(pr.left() + 17.0, pr.center().y);
        ring(p, c, 10.0, frac, 3.2, col);
        icons::draw(p, Icon::Shield, c, 12.0, col);
        p.text(pos2(pr.left() + 34.0, pr.center().y), Align2::LEFT_CENTER, if d.free_hit { "SAFE" } else { "CAREFUL" }, theme::ui_bold(12.0), col);
        let pip = pos2(pr.right() - 14.0, pr.center().y);
        if d.free_hit {
            p.circle_filled(pip, 5.0, theme::MINT);
            p.circle_stroke(pip, 5.0, Stroke::new(1.0_f32, rgba(255, 255, 255, 0.6)));
        } else {
            p.circle_stroke(pip, 4.5, Stroke::new(1.5_f32, fade(AMBER, 0.7)));
        }
    }
    // power-up chips: icon in a draining ring, seconds left
    for (k, pw) in d.powers.iter().enumerate() {
        let Some((frac, secs)) = *pw else { continue };
        let pr = Rect::from_min_size(rc.slot(slot).min, vec2(76.0, rc.prot.height()));
        slot += 1;
        glass(p, pr);
        let col = POWER_COL[k];
        let c = pos2(pr.left() + 17.0, pr.center().y);
        let low = frac < 0.2 && (time * 6.0).sin() > 0.0;
        ring(p, c, 10.0, frac, 3.2, if low { fade(col, 0.45) } else { col });
        power_icon(p, k, c, col);
        p.text(pos2(pr.left() + 34.0, pr.center().y), Align2::LEFT_CENTER, format!("{}s", secs.ceil() as u32), theme::numbers(15.0), col);
    }
    // ---- coins
    let cr = rc.coins;
    glass(p, cr);
    let dbl = d.powers[1].is_some();
    if dbl {
        p.rect_stroke(cr, cr.height() / 2.0, Stroke::new(1.8_f32, fade(POWER_COL[1], 0.9)), egui::StrokeKind::Inside);
    }
    let pulse = 1.0 + (if dbl { 0.4 } else { 0.25 }) * st.coin_pulse;
    art::coin(p, brand, pos2(cr.left() + 17.0, cr.center().y), 22.0 * pulse, None, dim);
    let num_col = if dbl { Color32::from_rgb(0xC9, 0xB8, 0xFF) } else { theme::SAND };
    p.text(pos2(cr.left() + 36.0, cr.center().y), Align2::LEFT_CENTER, format!("{}", st.coin_count()), theme::numbers(19.0), fade(num_col, dim));
    if dbl {
        p.text(pos2(cr.right() - 11.0, cr.center().y), Align2::RIGHT_CENTER, "x2", theme::ui_bold(12.0), POWER_COL[1]);
    }
    for a in &st.pops {
        let k = a / 0.7;
        art::outlined(p, pos2(cr.right() + 14.0, cr.center().y - 4.0 - 16.0 * k), Align2::CENTER_CENTER, "+2", theme::title(20.0), fade(POWER_COL[1], 1.0 - k), fade(theme::OUTLINE, 1.0 - k), 2.0, 1.0);
    }
    // ---- centre
    if d.solo {
        art::outlined(p, rc.main.center(), Align2::CENTER_CENTER, &format!("{} m", d.distance_m.max(0.0) as u32), theme::title(32.0), fade(theme::TEXT, dim), theme::OUTLINE, 2.4, 1.5);
        if let Some(best) = d.best_m {
            if best > 0 {
                if d.distance_m as u32 > best {
                    let b = 1.0 + 0.06 * (time * 8.0).sin();
                    widgets::pill(p, rc.sub.center(), "NEW BEST!", theme::GOLD, theme::OUTLINE, 12.0 * b);
                } else {
                    let w = 110.0;
                    let bar = Rect::from_center_size(rc.sub.center() + vec2(0.0, 4.0), vec2(w, 5.0));
                    p.rect_filled(bar, 2.5, rgba(0, 0, 0, 0.4));
                    let f = (d.distance_m / best as f32).clamp(0.0, 1.0);
                    p.rect_filled(Rect::from_min_size(bar.min, vec2(w * f, 5.0)), 2.5, theme::GOLD);
                    // Dark outline so the label reads against a pale sky.
                    let at = pos2(rc.sub.center().x, bar.top() - 7.0);
                    for (dx, dy) in [(-1.0, 0.0), (1.0, 0.0), (0.0, -1.0), (0.0, 1.0), (1.0, 1.0)] {
                        p.text(at + egui::vec2(dx, dy), Align2::CENTER_CENTER, format!("best {best} m"), theme::ui_bold(12.5), rgba(30, 20, 50, 0.9));
                    }
                    p.text(at, Align2::CENTER_CENTER, format!("best {best} m"), theme::ui_bold(12.5), rgba(255, 255, 255, 1.0));
                }
            }
        }
    } else {
        if let Some(t) = d.time_left {
            let s = t.max(0.0).ceil() as u32;
            let col = if t < 15.0 { theme::LASER } else { theme::TEXT };
            art::outlined(p, rc.main.center(), Align2::CENTER_CENTER, &format!("{}:{:02}", s / 60, s % 60), theme::title(32.0), col, theme::OUTLINE, 2.4, 1.5);
        }
        // the little bird sits left of the pill; shift the pill so the pair is centred under the clock
        let c = rc.sub.center() + vec2(10.0, 0.0);
        let r = widgets::pill(p, c, &format!("{} / {} running", d.living, d.total), rgba(14, 6, 30, 0.6), theme::TEXT, 12.0);
        icons::draw(p, Icon::Ostrich, pos2(r.left() - 12.0, c.y), 18.0, theme::SAND);
    }
}

/// The pause button.
pub fn draw_pause_icon(p: &Painter, r: Rect, pressed: bool) {
    let r = if pressed { r.shrink(2.0) } else { r };
    p.circle_filled(r.center(), r.width() / 2.0, rgba(14, 6, 30, 0.78));
    p.circle_stroke(r.center(), r.width() / 2.0, Stroke::new(1.5_f32, rgba(255, 255, 255, 0.25)));
    icons::draw(p, Icon::Pause, r.center(), 22.0, theme::TEXT);
}

/// The 3-2-1-GO overlay at the centre of `area`. `left` is seconds to GO,
/// `since_go` seconds since GO (None while counting).
pub fn draw_countdown(p: &Painter, area: Rect, left: f32, since_go: Option<f32>) {
    let c = area.center() - vec2(0.0, area.height() * 0.06);
    if let Some(t) = since_go {
        if t < 0.8 {
            let a = 1.0 - art::smooth(phase(t, 0.35, 0.45));
            let k = ease_out_back(phase(t, 0.0, 0.3));
            art::outlined(p, c, Align2::CENTER_CENTER, "GO!", theme::title(96.0 * (0.6 + 0.4 * k)), fade(theme::GOLD, a), fade(theme::OUTLINE, a), 7.0, 3.0);
        }
        return;
    }
    if left <= 0.0 {
        return;
    }
    let n = left.ceil();
    let f = n - left; // 0 at the start of each second
    let k = ease_out_back(phase(f, 0.0, 0.28));
    let a = 1.0 - art::smooth(phase(f, 0.7, 0.3));
    art::outlined(p, c, Align2::CENTER_CENTER, &format!("{}", n as u32), theme::title(150.0 * (0.55 + 0.45 * k)), fade(theme::TEXT, a), fade(theme::OUTLINE, a), 8.0, 3.0);
}

// ---- ghost HUD ---------------------------------------------------------------

/// What the ghost toolbar shows for one tool.
pub struct ToolView {
    pub icon: Icon,
    pub label: &'static str,
    pub cost: f32,
    pub selected: bool,
    pub affordable: bool,
    /// Icon colour when not selected.
    pub tint: Color32,
}

/// One toolbar button (painted; the caller hit-tests `rect`). Returns nothing.
pub fn draw_tool(p: &Painter, rect: Rect, t: &ToolView, pressed: bool) {
    let r = if pressed { rect.shrink(2.0) } else if t.selected { rect.expand(3.0) } else { rect };
    let dim = if t.affordable { 1.0 } else { 0.5 };
    widgets::soft_shadow(p, r, 16.0, 4.0, 10.0, 0.4);
    if t.selected {
        widgets::rrect_gradient(p, r, 16.0, Color32::from_rgb(0xFF, 0xBC, 0x4A), Color32::from_rgb(0xF0, 0x78, 0x1A));
        p.rect_stroke(r, 16.0, Stroke::new(3.0_f32, theme::GOLD), egui::StrokeKind::Outside);
        p.rect_stroke(r, 16.0, Stroke::new(2.0_f32, theme::OUTLINE), egui::StrokeKind::Inside);
    } else {
        widgets::rrect_gradient(p, r, 16.0, fade(Color32::from_rgb(0x56, 0x2E, 0x86), dim), fade(Color32::from_rgb(0x2A, 0x12, 0x4A), dim));
        p.rect_stroke(r, 16.0, Stroke::new(2.0_f32, fade(theme::OUTLINE, dim)), egui::StrokeKind::Inside);
    }
    let ink = if t.selected { Color32::WHITE } else { fade(t.tint, dim) };
    icons::draw(p, t.icon, pos2(r.center().x, r.top() + r.height() * 0.38), r.width() * 0.62, ink);
    // cost chip
    let chip = Rect::from_center_size(pos2(r.center().x, r.bottom() - 15.0), vec2(r.width() - 14.0, 20.0));
    p.rect_filled(chip, 10.0, rgba(14, 6, 30, if t.selected { 0.45 } else { 0.55 }));
    icons::draw(p, Icon::Bolt, pos2(chip.left() + 11.0, chip.center().y), 13.0, if t.affordable { theme::GOLD } else { theme::LASER });
    p.text(pos2(chip.right() - 9.0, chip.center().y), Align2::RIGHT_CENTER, format!("{}", t.cost as u32), theme::numbers(14.0), if t.affordable { Color32::WHITE } else { theme::LASER });
}

/// The energy bar: bolt, fill, and `3.4 / 10`.
pub fn draw_energy(p: &Painter, r: Rect, energy: f32, max: f32, regen_ping: f32, time: f32) {
    glass(p, r);
    let bolt = pos2(r.left() + 20.0, r.center().y);
    icons::draw(p, Icon::Bolt, bolt, 24.0, theme::GOLD);
    let bar = Rect::from_min_max(pos2(r.left() + 40.0, r.center().y - 8.0), pos2(r.right() - 64.0, r.center().y + 8.0));
    p.rect_filled(bar, 8.0, rgba(0, 0, 0, 0.45));
    let f = (energy / max.max(1.0)).clamp(0.0, 1.0);
    if f > 0.0 {
        let fill = Rect::from_min_size(bar.min, vec2((bar.width() * f).max(14.0), bar.height()));
        widgets::rrect_gradient(p, fill, 8.0, Color32::from_rgb(0xFF, 0xE0, 0x70), Color32::from_rgb(0xFF, 0x9E, 0x1A));
        let glint = 0.5 + 0.5 * (time * 3.0).sin();
        p.rect_filled(Rect::from_min_size(fill.min + vec2(4.0, 2.0), vec2((fill.width() - 8.0).max(0.0), 3.0)), 1.5, rgba(255, 255, 255, 0.35 + 0.2 * glint));
    }
    // whole-energy ticks
    for i in 1..max as u32 {
        let x = bar.left() + bar.width() * i as f32 / max;
        p.vline(x, bar.y_range(), Stroke::new(1.0_f32, rgba(0, 0, 0, 0.35)));
    }
    let _ = regen_ping;
    p.text(pos2(r.right() - 10.0, r.center().y), Align2::RIGHT_CENTER, format!("{:.1}", energy), theme::numbers(17.0), theme::TEXT);
}

/// A rounded banner line (the one-line ghost instruction, reject text).
pub fn draw_banner(p: &Painter, r: Rect, text: &str, alpha: f32, tint: Color32) {
    if alpha <= 0.0 {
        return;
    }
    p.rect_filled(r, r.height() / 2.0, fade(rgba(14, 6, 30, 0.74), alpha));
    p.rect_stroke(r, r.height() / 2.0, Stroke::new(1.5_f32, fade(tint, alpha * 0.7)), egui::StrokeKind::Inside);
    let size = art::fit_size(p, text, theme::ui_bold, 15.0, r.width() - 24.0);
    p.text(r.center(), Align2::CENTER_CENTER, text, theme::ui_bold(size), fade(Color32::WHITE, alpha));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::viewport::{Insets, Viewport};

    fn safe(w: f32, h: f32, top: f32, bottom: f32) -> Rect {
        Viewport::new(vec2(w, h), 2.625, Insets { top, bottom, left: 0.0, right: 0.0 }).safe
    }

    fn disjoint(rs: &[(&str, Rect)]) {
        for (i, (an, a)) in rs.iter().enumerate() {
            for (bn, b) in rs.iter().skip(i + 1) {
                assert!(!a.intersects(*b), "{an} {a:?} overlaps {bn} {b:?}");
            }
        }
    }

    fn inside(s: Rect, rs: &[(&str, Rect)]) {
        for (n, r) in rs {
            assert!(s.contains_rect(*r), "{n} {r:?} outside safe {s:?}");
        }
    }

    #[test]
    fn runner_hud_fits_and_does_not_overlap() {
        for (w, h) in [(360.0, 800.0), (412.0, 915.0), (320.0, 640.0)] {
            for top in [0.0, 28.0, 48.0] {
                for (solo, pause) in [(true, true), (false, true), (false, false)] {
                    let s = safe(w, h, top, 16.0);
                    let rc = layout(s, Mode { solo, pause, ghost: false, left_handed: false });
                    let mut v = vec![("feathers", rc.feathers), ("coins", rc.coins), ("prot", rc.prot), ("main", rc.main), ("sub", rc.sub)];
                    if let Some(p) = rc.pause {
                        v.push(("pause", p));
                    }
                    inside(s, &v);
                    disjoint(&v);
                    assert!(rc.feathers.width() + rc.main.width() + 44.0 + 40.0 <= s.width() + 0.5, "{w}: top row fits");
                }
            }
        }
    }

    #[test]
    fn ghost_hud_fits_and_toolbar_flips() {
        for (w, h) in [(360.0, 800.0), (412.0, 915.0)] {
            for lh in [false, true] {
                let s = safe(w, h, 48.0, 20.0);
                let rc = layout(s, Mode { solo: false, pause: true, ghost: true, left_handed: lh });
                let mut v = vec![("energy", rc.energy), ("banner", rc.banner), ("info", rc.info)];
                for (i, t) in rc.tools.iter().enumerate() {
                    v.push((["t0", "t1", "t2", "t3"][i], *t));
                }
                inside(s, &v);
                disjoint(&v);
                assert!(rc.tools[0].top() > rc.banner.bottom() + 40.0, "toolbar leaves the course visible above it");
                assert_eq!(rc.tools[0].left() < s.center().x, lh, "left-handed puts the toolbar on the left");
                for t in rc.tools {
                    assert!(t.width() >= 44.0 && t.height() >= 44.0, "tap targets");
                }
            }
        }
    }

    #[test]
    fn coin_counter_counts_up_and_lands_exactly() {
        let mut st = HudState::new();
        for _ in 0..120 {
            st.update(1.0 / 60.0, 3, 3, 12);
        }
        assert_eq!(st.coin_count(), 12);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..40 {
            st.update(1.0 / 60.0, 3, 3, 15);
            seen.insert(st.coin_count());
        }
        assert!(seen.contains(&13) || seen.contains(&14), "counts through the numbers: {seen:?}");
        assert_eq!(st.coin_count(), 15);
    }

    #[test]
    fn feather_loss_animates_then_rests() {
        let mut st = HudState::new();
        st.update(0.016, 3, 3, 0);
        st.update(0.016, 2, 3, 0);
        assert!(st.lost[2].is_some());
        for _ in 0..60 {
            st.update(0.016, 2, 3, 0);
        }
        assert!(st.lost[2].unwrap() > 0.7);
    }
}
