//! The multiplayer results table. It descends from Twang Thang's results screen
//! (podium rising with `ease_out_cubic` and text clipped to the rising pillar, a
//! squeeze layout that shrinks the podium, then the rows, then drops the podium,
//! then scrolls, a highlighted local row, fraction-of-width columns, the rematch
//! ready ticks and the shared 3-2-1) restyled in Flock's tokens.
//!
//! Ranking is decided by the host: `ResultRow::place` is shared on exact ties
//! and is the only thing the table trusts (never the row index). Disconnected
//! runners get a LEFT pill, never a normal result.

use super::common;
use super::{Ctx, Screen, Transition};
use crate::ui::art::{self, ease_out_cubic, fade, from_f3, outlined, phase, rgba};
use crate::ui::icons::{self, Icon};
use crate::ui::widgets::{self, Btn, S2};
use crate::ui::{feedback, theme};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Id, LayerId, Order, Painter, Rect, Sense, Stroke, StrokeKind};
use flock_core::{EndReason, PlayerId};
use flock_render::PlayerStyle;
use flock_session::{ResultRow, ResultsView};

/// What a button does when pressed: returns the transition to make.
pub type Action = Box<dyn FnMut(&mut Ctx) -> Transition>;

/// The two button captions.
#[derive(Clone, Debug, PartialEq)]
pub struct ResultsLabels {
    pub again: String,
    pub again_sub: Option<String>,
    pub leave: String,
}

impl Default for ResultsLabels {
    fn default() -> Self {
        ResultsLabels { again: "Play again".into(), again_sub: None, leave: "Home".into() }
    }
}

// ── pure data helpers ───────────────────────────────────────────────────────

/// "1st", "2nd", "3rd", "4th" ... (shared places repeat).
pub fn ordinal(place: u8) -> String {
    let suffix = match (place % 100, place % 10) {
        (11..=13, _) => "th",
        (_, 1) => "st",
        (_, 2) => "nd",
        (_, 3) => "rd",
        _ => "th",
    };
    format!("{place}{suffix}")
}

const SILVER: Color32 = Color32::from_rgb(0xD5, 0xD9, 0xE0);
const BRONZE: Color32 = Color32::from_rgb(0xD8, 0x8A, 0x4A);

fn medal_col(place: u8) -> Option<Color32> {
    match place {
        1 => Some(theme::GOLD),
        2 => Some(SILVER),
        3 => Some(BRONZE),
        _ => None,
    }
}

/// `m:ss.s`.
pub fn fmt_time(secs: f32) -> String {
    let tenths = (secs.max(0.0) * 10.0).floor() as u32;
    format!("{}:{:02}.{}", tenths / 600, (tenths / 10) % 60, tenths % 10)
}

/// `m:ss`, whole seconds (for the share text).
pub fn fmt_mss(secs: f32) -> String {
    let s = secs.max(0.0).floor() as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// What Share hands to the share sheet, or `None` when there is nothing fair to
/// say (the local runner left, is missing, or the round had no rivals).
pub fn share_message(v: &ResultsView, local: PlayerId) -> Option<String> {
    let r = v.row(local).filter(|r| !r.disconnected)?;
    let n = v.rows.len();
    if n < 2 {
        return None;
    }
    let tail = if r.alive && v.end_reason == Some(EndReason::TimeCap) {
        "still standing at the bell".to_string()
    } else {
        format!("survived {}", fmt_mss(r.survived_secs()))
    };
    Some(format!("I came {} of {n} in a Cluster Flock round \u{2014} {tail}. Rematch?", ordinal(r.place)))
}

/// The runner's body colour as the table's chip colour.
fn chip_col(colour: u8) -> Color32 {
    from_f3(PlayerStyle::of(colour).body)
}

/// The subtitle for how the match ended. A disconnect is never presented as a win.
pub fn subtitle(v: &ResultsView, local: PlayerId) -> String {
    match v.end_reason {
        Some(EndReason::NoRunnersLeft) => {
            let winners: Vec<&ResultRow> = v.rows.iter().filter(|r| r.place == 1 && !r.disconnected).collect();
            match winners.as_slice() {
                [r] if r.player == local => "Last runner standing".into(),
                [r] => format!("{} was the last runner standing", r.name),
                [] => "Match ended early".into(),
                _ => "Tied for the win".into(),
            }
        }
        Some(EndReason::TimeCap) => "Time's up".into(),
        Some(EndReason::NotEnoughPlayers) | Some(EndReason::Aborted) | Some(EndReason::FinalFeather) | None => {
            let gone: Vec<&ResultRow> = v.rows.iter().filter(|r| r.disconnected && r.player != local).collect();
            match gone.as_slice() {
                [r] if v.end_reason == Some(EndReason::NotEnoughPlayers) => format!("{} left the match", r.name),
                _ => "Match ended early".into(),
            }
        }
    }
}

/// Did the local runner win a normal match? (A win by everyone else leaving is not shown as VICTORY.)
pub fn local_won(v: &ResultsView, local: PlayerId) -> bool {
    let normal = matches!(v.end_reason, Some(EndReason::NoRunnersLeft) | Some(EndReason::TimeCap));
    normal && v.row(local).is_some_and(|r| r.place == 1 && !r.disconnected)
}

/// How many rows stand on the podium (places 1..=3), or 0 if it should be hidden.
pub fn podium_count(rows: &[ResultRow]) -> usize {
    let n = rows.iter().filter(|r| r.place <= 3).count();
    if rows.len() >= 2 && (2..=4).contains(&n) {
        n
    } else {
        0
    }
}

// ── rematch state ───────────────────────────────────────────────────────────

/// What the primary button area says about the rematch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Nobody has said anything.
    Idle,
    /// Some runners are ready.
    Waiting { ready: usize, total: usize },
    /// The shared countdown is running: 3, 2 or 1.
    Count(u8),
}

/// The online rematch hooks' state: per-row ready flags and the 3-2-1.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rematch {
    pub ready: Vec<(PlayerId, bool)>,
    pub countdown_ms: Option<u32>,
}

impl Rematch {
    pub fn set(&mut self, ready: Vec<(PlayerId, bool)>, countdown_ms: Option<u32>) {
        self.ready = ready;
        self.countdown_ms = countdown_ms;
    }

    /// The 3, 2 or 1 on screen (`None` = no countdown).
    pub fn count(&self) -> Option<u8> {
        self.countdown_ms.map(|ms| (ms.div_ceil(1000)).clamp(1, 3) as u8)
    }

    /// How far into the current number we are, 0..1 (for the pop).
    pub fn count_progress(&self) -> f32 {
        self.countdown_ms.map_or(0.0, |ms| 1.0 - (ms % 1000) as f32 / 1000.0)
    }

    pub fn is_ready(&self, id: PlayerId) -> Option<bool> {
        self.ready.iter().find(|(p, _)| *p == id).map(|(_, r)| *r)
    }

    pub fn flow(&self) -> Flow {
        if let Some(n) = self.count() {
            Flow::Count(n)
        } else if self.ready.iter().any(|(_, r)| *r) {
            Flow::Waiting { ready: self.ready.iter().filter(|(_, r)| *r).count(), total: self.ready.len() }
        } else {
            Flow::Idle
        }
    }
}

// ── geometry ────────────────────────────────────────────────────────────────

const MARGIN_X: f32 = 14.0;
const PANEL_PAD: f32 = 8.0;
const HEADER_ROW: f32 = 22.0;
const PRIMARY_H: f32 = 64.0;
const LEAVE_H: f32 = 56.0;
const BTN_GAP: f32 = 10.0;
const BOTTOM_PAD: f32 = 14.0;
const MIN_ROW_H: f32 = 32.0;
/// Steepest pillar, as a share of the first place pillar, for places 1, 2 and 3 (and below).
const STEP: [f32; 3] = [1.0, 0.82, 0.68];

/// Where the podium stands: horizontal extent, the line the pillars rise from and the first-place height.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PodiumBox {
    pub x0: f32,
    pub x1: f32,
    pub base_y: f32,
    pub h1: f32,
}

/// Every y of the screen for a given size (see [`layout_for`]).
#[derive(Clone, Debug)]
pub struct Layout {
    pub title_y: f32,
    pub sub_y: f32,
    pub title_size: f32,
    pub podium: Option<PodiumBox>,
    /// The whole glass table panel.
    pub table: Rect,
    /// Centre y of the column header row.
    pub header_y: f32,
    /// The window the rows scroll in.
    pub rows: Rect,
    pub row_h: f32,
    pub content_h: f32,
    pub max_scroll: f32,
    pub primary: Rect,
    /// Share and Leave sit side by side under the primary button.
    pub share: Rect,
    pub leave: Rect,
}

/// The squeeze: with `n_rows` rows and `podium_n` pillars, inside `safe`. The
/// podium shrinks first, then the row height, then the podium goes, and only
/// then does the table scroll. The table never reaches the buttons.
pub fn layout_for(safe: Rect, n_rows: usize, podium_n: usize) -> Layout {
    let n = n_rows.max(1) as f32;
    let btn_w = (safe.width() - 40.0).clamp(120.0, 340.0);
    let row = Rect::from_min_size(pos2(safe.center().x - btn_w / 2.0, safe.bottom() - BOTTOM_PAD - LEAVE_H), vec2(btn_w, LEAVE_H));
    let half = (btn_w - S2) / 2.0;
    let share = Rect::from_min_size(row.min, vec2(half, LEAVE_H));
    let leave = Rect::from_min_size(pos2(row.right() - half, row.top()), vec2(half, LEAVE_H));
    let primary = Rect::from_min_size(pos2(row.left(), row.top() - BTN_GAP - PRIMARY_H), vec2(btn_w, PRIMARY_H));
    let limit = primary.top() - 12.0;

    let table_h = |rows: f32, row_h: f32| 2.0 * PANEL_PAD + HEADER_ROW + rows * row_h;
    let zone = |h1: f32| if h1 > 0.0 { h1 + 44.0 } else { 0.0 };
    // (first-place height, row height, tight title)
    let steps: &[(f32, f32, bool)] = if podium_n >= 2 {
        &[(100.0, 44.0, false), (80.0, 44.0, false), (60.0, 44.0, true), (60.0, 40.0, true), (60.0, 36.0, true), (0.0, 44.0, false), (0.0, 40.0, false), (0.0, 36.0, true), (0.0, MIN_ROW_H, true)]
    } else {
        &[(0.0, 44.0, false), (0.0, 40.0, false), (0.0, 36.0, true), (0.0, MIN_ROW_H, true)]
    };
    let content_top = |tight: bool| safe.top() + if tight { 66.0 } else { 80.0 };
    let fits = |&(h1, row_h, tight): &(f32, f32, bool)| content_top(tight) + zone(h1) + table_h(n, row_h) <= limit;
    let pick = steps.iter().position(fits);
    let (mut h1, mut row_h, tight) = pick.map_or(*steps.last().unwrap(), |i| steps[i]);
    let mut shift = 0.0;
    if pick == Some(0) {
        // room to spare: the podium and the rows grow, and what is left pushes the content down a little
        let mut spare = limit - (content_top(tight) + zone(h1) + table_h(n, row_h));
        if h1 > 0.0 {
            let g = (spare * 0.5).clamp(0.0, 60.0);
            h1 += g;
            spare -= g;
        }
        let g = (spare / n).clamp(0.0, 8.0);
        row_h += g;
        spare -= g * n;
        shift = (spare * 0.5).clamp(0.0, 40.0);
    }
    let top = content_top(tight) + shift;
    let table_top = top + zone(h1);
    let want = table_h(n, row_h);
    let table_bottom = (table_top + want).min(limit).max(table_top + 2.0 * PANEL_PAD + HEADER_ROW + row_h.min(MIN_ROW_H));
    let table = Rect::from_min_max(pos2(safe.left() + MARGIN_X, table_top), pos2(safe.right() - MARGIN_X, table_bottom));
    let rows = Rect::from_min_max(pos2(table.left(), table.top() + PANEL_PAD + HEADER_ROW), pos2(table.right(), table.bottom() - PANEL_PAD));
    let content_h = n * row_h;
    Layout {
        title_y: safe.top() + if tight { 24.0 } else { 30.0 },
        sub_y: safe.top() + if tight { 52.0 } else { 62.0 },
        title_size: if tight { 30.0 } else { 34.0 },
        podium: (h1 > 0.0).then_some(PodiumBox { x0: table.left(), x1: table.right(), base_y: table_top - 6.0, h1 }),
        table,
        header_y: table.top() + PANEL_PAD + HEADER_ROW / 2.0,
        rows,
        row_h,
        content_h,
        max_scroll: (content_h - rows.height()).max(0.0),
        primary,
        share,
        leave,
    }
}

/// Pillar rectangles (full height), left to right, each with the index into `places`.
/// Co-winners with the same place get the same step height. 1st goes in the middle.
pub fn pillar_rects(b: &PodiumBox, places: &[u8]) -> Vec<(usize, Rect)> {
    let k = places.len();
    if k == 0 {
        return Vec::new();
    }
    let order: Vec<usize> = match k {
        3 => vec![1, 0, 2],
        4 => vec![1, 0, 2, 3],
        _ => (0..k).collect(),
    };
    let gap = 6.0;
    let w = ((b.x1 - b.x0 - gap * (k as f32 - 1.0)) / k as f32).min(104.0);
    let total = w * k as f32 + gap * (k as f32 - 1.0);
    let start = (b.x0 + b.x1) / 2.0 - total / 2.0;
    order
        .into_iter()
        .enumerate()
        .map(|(slot, idx)| {
            let h = b.h1 * STEP[(places[idx].clamp(1, 3) - 1) as usize];
            let x = start + slot as f32 * (w + gap);
            (idx, Rect::from_min_max(pos2(x, b.base_y - h), pos2(x + w, b.base_y)))
        })
        .collect()
}

/// Where the text of a pillar goes: `(name centre y, sub-line centre y)` from the pillar's top.
/// Short pillars (< 52 dp) are compact: the name alone, centred.
pub fn pillar_text_ys(top: f32, full_h: f32) -> (f32, Option<f32>) {
    if full_h < 52.0 {
        (top + full_h / 2.0, None)
    } else {
        (top + 17.0, Some(top + 36.0))
    }
}

/// Column positions as fractions of the table's inner width.
#[derive(Clone, Copy, Debug)]
pub struct Cols {
    pub x0: f32,
    pub w: f32,
}

const F_ORD: f32 = 0.0;
const F_ORD_END: f32 = 0.105;
const F_CHIP: f32 = 0.14;
const F_NAME: f32 = 0.19;
const F_NAME_END: f32 = 0.42;
const F_TIME: f32 = 0.525;
const F_TIME_HALF: f32 = 0.086;
const F_FEATH: f32 = 0.685;
const F_COIN: f32 = 0.765;
const F_COIN_END: f32 = 0.88;
const F_DIV: f32 = 0.89;
const F_BUILT: f32 = 0.945;

impl Cols {
    pub fn new(table: Rect) -> Cols {
        Cols { x0: table.left() + PANEL_PAD, w: table.width() - 2.0 * PANEL_PAD }
    }
    pub fn x(&self, f: f32) -> f32 {
        self.x0 + self.w * f
    }
    /// Left edge of the first numeric column (the survived time).
    pub fn first_numeric_left(&self) -> f32 {
        self.x(F_TIME - F_TIME_HALF)
    }
}

// ── text fitting ────────────────────────────────────────────────────────────

fn text_w(p: &Painter, s: &str, font: FontId) -> f32 {
    p.layout_no_wrap(s.to_string(), font, Color32::WHITE).size().x
}

/// Shrink `text` from `size` to `min`, then cut it with an ellipsis, until it fits `max_w`.
pub fn fit_text(p: &Painter, text: &str, mk: fn(f32) -> FontId, size: f32, min: f32, max_w: f32) -> (String, f32) {
    let mut s = size;
    while s > min && text_w(p, text, mk(s)) > max_w {
        s -= 0.5;
    }
    if text_w(p, text, mk(s)) <= max_w {
        return (text.to_string(), s);
    }
    let mut chars: Vec<char> = text.chars().collect();
    while chars.pop().is_some() {
        let cand = format!("{}\u{2026}", chars.iter().collect::<String>().trim_end());
        if text_w(p, &cand, mk(s)) <= max_w {
            return (cand, s);
        }
    }
    ("\u{2026}".into(), s)
}

fn pill_size(p: &Painter, text: &str, size: f32) -> egui::Vec2 {
    p.layout_no_wrap(text.to_string(), theme::ui_bold(size), Color32::WHITE).size() + vec2(size, size * 0.55)
}

/// What a name cell shows besides the name.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tags {
    pub you: bool,
    pub bot: bool,
}

/// The runner cell: the name (shrunk, then ellipsised, to `cell`), and on a second line the
/// "(you)" suffix and a BOT pill, neither of which is ever cut. Returns the union of what is
/// drawn. With `paint == false` only measures (for tests).
pub fn name_cell(p: &Painter, cell: Rect, name: &str, tags: Tags, name_col: Color32, paint: bool) -> Rect {
    let two = tags.you || tags.bot;
    let (txt, size) = fit_text(p, name, theme::ui_bold, if two { 13.5 } else { 15.0 }, 10.0, cell.width());
    let y1 = if two { cell.center().y - 8.0 } else { cell.center().y };
    let g = p.layout_no_wrap(txt.clone(), theme::ui_bold(size), name_col);
    let mut union = Rect::from_min_size(pos2(cell.left(), y1 - g.size().y / 2.0), g.size());
    if paint {
        p.galley(union.min, g, name_col);
    }
    if two {
        let y2 = cell.center().y + 9.0;
        let mut x = cell.left();
        if tags.you {
            let g = p.layout_no_wrap("(you)".into(), theme::ui_bold(10.5), theme::GOLD);
            let r = Rect::from_min_size(pos2(x, y2 - g.size().y / 2.0), g.size());
            if paint {
                p.galley(r.min, g, theme::GOLD);
            }
            union = union.union(r);
            x = r.right() + 5.0;
        }
        if tags.bot {
            let sz = pill_size(p, "BOT", 8.5);
            let r = Rect::from_min_size(pos2(x, y2 - sz.y / 2.0), sz);
            if paint {
                widgets::pill(p, r.center(), "BOT", rgba(255, 255, 255, 0.13), theme::TEXT_DIM, 8.5);
            }
            union = union.union(r);
        }
    }
    union
}

// ── the screen ──────────────────────────────────────────────────────────────

enum Press {
    Again,
    Share,
    Leave,
}

pub struct ResultsScreen {
    results: ResultsView,
    local: PlayerId,
    labels: ResultsLabels,
    again: Action,
    leave: Action,
    rematch: Rematch,
    note: Option<String>,
    t: f32,
    scroll: f32,
    last_count: Option<u8>,
}

impl ResultsScreen {
    pub fn new(mut results: ResultsView, local: PlayerId, labels: ResultsLabels, again: Action, leave: Action) -> ResultsScreen {
        // The host sends rows sorted; a stable sort by the shared place keeps ties in the given order.
        results.rows.sort_by_key(|r| r.place);
        ResultsScreen { results, local, labels, again, leave, rematch: Rematch::default(), note: None, t: 0.0, scroll: 0.0, last_count: None }
    }

    /// Online rematch: per-row ready ticks (shown only when set) and the shared 3-2-1 (ms left).
    pub fn set_rematch(&mut self, ready: Vec<(PlayerId, bool)>, countdown_ms: Option<u32>) {
        self.rematch.set(ready, countdown_ms);
    }

    /// A subtitle override such as "Host left".
    pub fn set_note(&mut self, note: Option<String>) {
        self.note = note;
    }

    /// Staged for `--shot`: six rows, the local runner 2nd, a tie for 3rd (next place 5), one survivor, one who left.
    pub fn sample() -> ResultsScreen {
        let row = |id: u8, name: &str, bot: bool, place: u8, alive: bool, secs: f32, feathers: u8, coins: u32, placements: u32, disconnected: bool| ResultRow {
            player: PlayerId(id),
            name: name.into(),
            colour: id,
            hat: 255,
            bot,
            place,
            alive,
            survived_ticks: (secs / flock_core::DT).round() as u64,
            feathers,
            coins,
            placements,
            disconnected,
        };
        let rows = vec![
            row(0, "Mina", false, 1, true, 90.0, 2, 18, 2, false),
            row(1, "Juniper", false, 2, false, 74.3, 0, 12, 5, false),
            row(2, "Bartholomew", false, 3, false, 61.0, 0, 9, 0, false),
            row(3, "Pip", true, 3, false, 61.0, 0, 9, 3, false),
            row(4, "Wren", false, 5, false, 40.2, 0, 4, 7, false),
            row(5, "Moss", false, 6, false, 12.5, 0, 1, 0, true),
        ];
        let v = ResultsView { rows, end_reason: Some(EndReason::TimeCap), round: 1, solo: None };
        ResultsScreen::new(v, PlayerId(1), ResultsLabels::default(), Box::new(|_| Transition::None), Box::new(|_| Transition::None))
    }

    /// Staged for `--shot`: the local runner won, some rematch ticks are in.
    pub fn sample_win() -> ResultsScreen {
        let mut s = ResultsScreen::sample();
        s.local = PlayerId(0);
        s.labels = ResultsLabels { again: "Rematch".into(), again_sub: Some("Tap when you are ready".into()), leave: "Leave".into() };
        s.set_rematch(vec![(PlayerId(0), false), (PlayerId(1), true), (PlayerId(2), true), (PlayerId(3), true), (PlayerId(4), false)], None);
        s
    }

    fn won(&self) -> bool {
        local_won(&self.results, self.local)
    }

    /// Paint the screen and report a button press. Takes no `Ctx` so the tests can drive it.
    fn paint(&mut self, ui: &egui::Context, screen: Rect, safe: Rect, tm: f32) -> Option<Press> {
        let t = self.t;
        let mut press = None;
        let n = self.results.rows.len();
        let places: Vec<u8> = self.results.rows.iter().filter(|r| r.place <= 3).map(|r| r.place).collect();
        let pn = podium_count(&self.results.rows);
        let lay = layout_for(safe, n, pn);
        let won = self.won();
        let title = if won { "VICTORY" } else { "RESULTS" };
        let sub_text = self.note.clone().unwrap_or_else(|| subtitle(&self.results, self.local));
        let again_sub: Option<String> = match self.rematch.flow() {
            Flow::Idle => self.labels.again_sub.clone(),
            Flow::Waiting { ready, total } => Some(format!("{ready} of {total} ready")),
            Flow::Count(_) => Some("Starting...".into()),
        };
        self.scroll = self.scroll.clamp(0.0, lay.max_scroll);
        let (local, results, rematch, labels, scroll) = (self.local, &self.results, &self.rematch, &self.labels, &mut self.scroll);

        common::page(screen, tm, ui, 0.32, |ui, brand| {
            let p = ui.painter().clone();
            if won {
                // a shower over the title and the podium only, never over the table or the buttons
                let sky = Rect::from_min_max(pos2(safe.left(), safe.top()), pos2(safe.right(), lay.table.top() - 4.0));
                widgets::confetti(&p.with_clip_rect(sky), brand, sky, t, 12);
            }

            // title and subtitle
            let e = ease_out_cubic(phase(t, 0.05, 0.4));
            widgets::staged(ui, e, vec2(0.0, -14.0), |ui, off| {
                let p = ui.painter().clone();
                widgets::heading(&p, pos2(safe.center().x, lay.title_y) + off, Align2::CENTER_CENTER, title, lay.title_size, if won { theme::GOLD } else { theme::TEXT });
                let (txt, _) = fit_text(&p, &sub_text, theme::ui_bold, 12.5, 10.0, safe.width() - 2.0 * (MARGIN_X + 12.0));
                widgets::pill(&p, pos2(safe.center().x, lay.sub_y) + off, &txt, rgba(20, 8, 40, 0.62), theme::TEXT, 12.5);
            });

            // podium
            if let Some(b) = lay.podium {
                let podium_rows: Vec<&ResultRow> = results.rows.iter().filter(|r| r.place <= 3).collect();
                for (i, (idx, full)) in pillar_rects(&b, &places).into_iter().enumerate() {
                    let rise = ease_out_cubic(phase(t, 0.22 + 0.08 * i as f32, 0.7));
                    paint_pillar(&p, podium_rows[idx], full, rise, t, tm, podium_rows[idx].player == local);
                }
            }

            // the table
            let e = ease_out_cubic(phase(t, 0.30, 0.4));
            widgets::staged(ui, e, vec2(0.0, 16.0), |ui, off| {
                let p = ui.painter().clone();
                p.rect_filled(lay.table.translate(off), rr(18.0), rgba(14, 6, 28, 0.5));
                widgets::panel(&p, lay.table.translate(off), 18.0);
                let cols = Cols::new(lay.table.translate(off));
                paint_header(&p, cols, lay.header_y + off.y);
            });
            let cols = Cols::new(lay.table);
            // scroll: drag or wheel inside the rows window only
            if lay.max_scroll > 0.0 {
                let resp = ui.interact(lay.rows, Id::new("results-rows"), Sense::drag());
                *scroll -= resp.drag_delta().y;
                if resp.hovered() {
                    *scroll -= ui.input(|i| i.smooth_scroll_delta.y);
                }
                *scroll = scroll.clamp(0.0, lay.max_scroll);
                let track = Rect::from_min_max(pos2(lay.table.right() - 5.0, lay.rows.top() + 2.0), pos2(lay.table.right() - 2.0, lay.rows.bottom() - 2.0));
                p.rect_filled(track, CornerRadius::same(2), rgba(255, 255, 255, 0.08));
                let bar_h = track.height() * lay.rows.height() / lay.content_h;
                let bar_y = track.top() + (track.height() - bar_h) * (*scroll / lay.max_scroll);
                p.rect_filled(Rect::from_min_size(pos2(track.left(), bar_y), vec2(track.width(), bar_h)), CornerRadius::same(2), rgba(255, 255, 255, 0.35));
            }
            // rows divider (the unranked "Built" column sits apart)
            let window = lay.rows.expand2(vec2(MARGIN_X, 0.0));
            let pw = p.with_clip_rect(lay.rows);
            pw.vline(cols.x(F_DIV), lay.rows.y_range(), Stroke::new(1.0_f32, rgba(255, 255, 255, 0.10)));
            for (i, r) in results.rows.iter().enumerate() {
                let y = lay.rows.top() + i as f32 * lay.row_h - *scroll;
                if y + lay.row_h < lay.rows.top() || y > lay.rows.bottom() {
                    continue;
                }
                let e = ease_out_cubic(phase(t, 0.55 + 0.08 * i as f32, 0.32));
                ui.scope(|ui| {
                    ui.set_opacity(e);
                    let pr = ui.painter().clone().with_clip_rect(window);
                    let row = Rect::from_min_size(pos2(cols.x0, y), vec2(cols.w, lay.row_h)).translate(vec2(-24.0 * (1.0 - e), 0.0));
                    let tick = rematch.is_ready(r.player).filter(|_| !rematch.ready.is_empty());
                    paint_row(&pr, brand, cols, row, r, r.player == local, i, tick, safe.left() + MARGIN_X / 2.0);
                });
            }

            // buttons
            let e = ease_out_cubic(phase(t, 0.75, 0.4));
            widgets::staged(ui, e, vec2(0.0, 24.0), |ui, off| {
                let mut primary = Btn::primary(&labels.again).size(24.0);
                if let Some(s) = again_sub.as_deref() {
                    primary = primary.sub(s);
                }
                if widgets::button(ui, lay.primary.translate(off), "results-again", &primary).clicked() {
                    press = Some(Press::Again);
                }
                let can_share = share_message(results, local).is_some();
                if widgets::button(ui, lay.share.translate(off), "results-share", &Btn::ghost("Share").icon(Icon::Share).enabled(can_share).size(18.0)).clicked() {
                    press = Some(Press::Share);
                }
                if widgets::button(ui, lay.leave.translate(off), "results-leave", &Btn::ghost(&labels.leave).size(18.0)).clicked() {
                    press = Some(Press::Leave);
                }
            });
        });

        // the shared 3-2-1 over everything above the buttons
        if let Some(n) = self.rematch.count() {
            let veil = Rect::from_min_max(screen.min, pos2(screen.right(), lay.primary.top() - S2));
            let p = ui.layer_painter(LayerId::new(Order::Foreground, Id::new("results-count")));
            p.rect_filled(veil, 0.0, rgba(10, 4, 24, 0.82));
            let f = self.rematch.count_progress();
            let pop = 1.0 + 0.28 * (1.0 - f).powi(3);
            let a = (0.45 + f * 6.0).min(1.0);
            let c = veil.center() - vec2(0.0, 12.0);
            outlined(&p, c, Align2::CENTER_CENTER, &n.to_string(), theme::title(120.0 * pop), fade(theme::GOLD, a), theme::OUTLINE, 6.0, 4.0);
            p.text(c + vec2(0.0, 92.0), Align2::CENTER_CENTER, "Next round starting", theme::ui_bold(16.0), theme::TEXT);
        }
        press
    }
}

impl Screen for ResultsScreen {
    fn name(&self) -> &'static str {
        "results"
    }

    fn on_background(&mut self, _ctx: &mut Ctx) {
        feedback::on_background();
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        common::begin(ctx, &mut self.t);
        let count = self.rematch.count();
        if count != self.last_count {
            if count.is_some() {
                feedback::tap(ctx);
            }
            self.last_count = count;
        }
        Transition::None
    }

    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        let (screen, safe, tm) = (ctx.viewport.screen, ctx.viewport.safe, ctx.time as f32);
        match self.paint(ui, screen, safe, tm) {
            Some(Press::Again) => {
                feedback::select(ctx);
                (self.again)(ctx)
            }
            Some(Press::Share) => {
                feedback::tap(ctx);
                if let Some(text) = share_message(&self.results, self.local) {
                    let r = ctx.platform.share_text(&text);
                    if let Some(m) = crate::platform::os::share_note(&*ctx.platform, r) {
                        ctx.say(m);
                    }
                }
                Transition::None
            }
            Some(Press::Leave) => {
                feedback::back(ctx);
                (self.leave)(ctx)
            }
            None => Transition::None,
        }
    }

    fn back(&mut self, ctx: &mut Ctx) -> Transition {
        feedback::back(ctx);
        (self.leave)(ctx)
    }
}

// ── painting ────────────────────────────────────────────────────────────────

fn rr(r: f32) -> CornerRadius {
    CornerRadius::same(r.round().clamp(0.0, 255.0) as u8)
}

fn paint_header(p: &Painter, cols: Cols, y: f32) {
    let col = rgba(0xE8, 0xC6, 0x8A, 0.75);
    let item = |x: f32, anchor: Align2, text: &str, max_w: f32, col: Color32| {
        let s = art::fit_size(p, text, theme::ui_bold, 9.5, max_w);
        p.text(pos2(x, y), anchor, text, theme::ui_bold(s), col);
    };
    item(cols.x(F_NAME), Align2::LEFT_CENTER, "RUNNER", cols.w * 0.2, col);
    item(cols.x(F_TIME), Align2::CENTER_CENTER, "SURVIVED", cols.w * 0.17, col);
    item(cols.x(F_FEATH), Align2::CENTER_CENTER, "FEATHERS", cols.w * 0.15, col);
    item(cols.x(F_COIN + (F_COIN_END - F_COIN) / 2.0), Align2::CENTER_CENTER, "COINS", cols.w * 0.115, col);
    item(cols.x(F_BUILT), Align2::CENTER_CENTER, "BUILT", cols.w * 0.105, fade(col, 0.6));
    p.hline(cols.x0..=(cols.x0 + cols.w), y + 11.0, Stroke::new(1.0_f32, rgba(255, 255, 255, 0.10)));
}

#[allow(clippy::too_many_arguments)]
fn paint_row(p: &Painter, brand: &art::Brand, cols: Cols, row: Rect, r: &ResultRow, local: bool, idx: usize, tick: Option<bool>, tick_x: f32) {
    let y = row.center().y;
    let dim = if r.disconnected { 0.55 } else { 1.0 };
    let band = Rect::from_min_max(pos2(row.left() - 5.0, row.top() + 1.5), pos2(row.right() + 5.0, row.bottom() - 1.5));
    if local {
        p.rect_filled(band, rr(10.0), rgba(0xFF, 0xC8, 0x3D, 0.13));
        p.rect_stroke(band, rr(10.0), Stroke::new(2.0_f32, theme::GOLD), StrokeKind::Inside);
    } else if idx % 2 == 0 {
        p.rect_filled(band, rr(10.0), rgba(255, 255, 255, 0.035));
    }

    // rematch tick in the left margin
    if let Some(ready) = tick {
        let c = pos2(tick_x + (row.left() - cols.x0), y);
        if ready {
            icons::draw(p, Icon::Check, c, 14.0, theme::MINT);
        } else {
            p.circle_stroke(c, 4.0, Stroke::new(1.4_f32, rgba(255, 255, 255, 0.35)));
        }
    }

    // place
    let ord = ordinal(r.place);
    let size = art::fit_size(p, &ord, theme::title, 16.0, cols.w * (F_ORD_END - F_ORD));
    let (fill, line) = match medal_col(r.place) {
        Some(c) => (fade(c, dim), theme::OUTLINE),
        None => (fade(theme::TEXT_DIM, dim), theme::OUTLINE),
    };
    outlined(p, pos2(cols.x(F_ORD) + 1.0, y), Align2::LEFT_CENTER, &ord, theme::title(size), fill, line, 1.3, 1.0);

    // colour chip and name
    widgets::chip(p, pos2(cols.x(F_CHIP), y), 7.0, fade(chip_col(r.colour), dim));
    let name_col = if local { Color32::WHITE } else { theme::mix(theme::TEXT, theme::TEXT_DIM, 0.4) };
    let cell = Rect::from_min_max(pos2(cols.x(F_NAME), row.top()), pos2(cols.x(F_NAME_END), row.bottom()));
    let tags = Tags { you: local && !r.name.trim().eq_ignore_ascii_case("you"), bot: r.bot };
    name_cell(p, cell, &r.name, tags, fade(name_col, dim), true);

    // survived: a time, SURVIVED, or LEFT
    let tc = pos2(cols.x(F_TIME), y);
    if r.disconnected {
        widgets::pill(p, tc, "LEFT", rgba(0xFF, 0x2D, 0x3F, 0.22), Color32::from_rgb(0xFF, 0x8A, 0x94), 9.5);
    } else if r.alive {
        widgets::pill(p, tc, "SURVIVED", rgba(0x6B, 0xE0, 0xA8, 0.20), theme::MINT, 8.0);
    } else {
        p.text(tc, Align2::CENTER_CENTER, fmt_time(r.survived_secs()), theme::numbers(14.0), theme::TEXT);
    }

    // feathers
    let fc = cols.x(F_FEATH);
    if r.feathers == 0 {
        p.text(pos2(fc, y), Align2::CENTER_CENTER, "OUT", theme::ui_bold(11.0), fade(theme::LASER, dim));
    } else {
        for i in 0..3u8 {
            let c = pos2(fc + (i as f32 - 1.0) * 12.5, y);
            let col = if i < r.feathers { fade(theme::TEXT, dim) } else { rgba(255, 255, 255, 0.16) };
            paint_feather(p, c, 16.0, col);
        }
    }

    // coins
    let cx = cols.x(F_COIN);
    art::coin(p, brand, pos2(cx + 6.0, y), 12.5, None, if r.coins == 0 { 0.35 } else { dim });
    let txt = r.coins.to_string();
    let max_w = cols.x(F_COIN_END) - (cx + 16.0);
    let s = art::fit_size(p, &txt, theme::numbers, 14.0, max_w);
    p.text(pos2(cx + 16.0, y), Align2::LEFT_CENTER, txt, theme::numbers(s), if r.coins == 0 { rgba(0xE8, 0xC6, 0x8A, 0.35) } else { fade(theme::SAND, dim) });

    // built: unranked, quieter, apart
    let bx = cols.x(F_BUILT);
    let quiet = if r.placements == 0 { 0.28 } else { 0.62 };
    icons::draw(p, Icon::Ghost, pos2(bx, y - 7.0), 11.0, fade(theme::TEXT_DIM, quiet * dim));
    let txt = r.placements.to_string();
    let s = art::fit_size(p, &txt, theme::numbers, 12.0, cols.w * 0.1);
    p.text(pos2(bx, y + 8.0), Align2::CENTER_CENTER, txt, theme::numbers(s), fade(theme::TEXT_DIM, quiet * dim));
}

/// A filled quill (the shared icon's vane is a hairline at this size): a lens-shaped vane on a slanted spine.
fn paint_feather(p: &Painter, c: egui::Pos2, s: f32, col: Color32) {
    let (tip, base) = (c + vec2(0.34, -0.5) * s, c + vec2(-0.34, 0.5) * s);
    let axis = tip - base;
    let side = vec2(-axis.y, axis.x).normalized();
    let mut pts = Vec::new();
    for sign in [1.0_f32, -1.0] {
        for i in 0..=10 {
            let t = if sign > 0.0 { i as f32 / 10.0 } else { 1.0 - i as f32 / 10.0 };
            pts.push(base + axis * t + side * sign * (t * std::f32::consts::PI).sin() * 0.21 * s);
        }
    }
    p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
    p.line_segment([base - axis * 0.12, tip], Stroke::new(1.1_f32, theme::mix(col, theme::OUTLINE, 0.6)));
}

fn paint_pillar(p: &Painter, r: &ResultRow, full: Rect, rise: f32, t: f32, tm: f32, local: bool) {
    let h = full.height() * rise;
    if h < 2.0 {
        return;
    }
    let rect = Rect::from_min_max(pos2(full.left(), full.bottom() - h), full.max);
    let col = chip_col(r.colour);
    let dim = if r.disconnected { 0.6 } else { 1.0 };
    let medal = medal_col(r.place).unwrap_or(theme::SAND);

    widgets::soft_shadow(p, rect, 10.0, 4.0, 12.0, 0.4);
    widgets::rrect_gradient(p, rect, 10.0, theme::mix(theme::DEEP, col, 0.34), theme::mix(theme::DEEP, col, 0.10));
    p.rect_stroke(rect, rr(10.0), Stroke::new(2.0_f32, fade(col, 0.9 * dim)), StrokeKind::Inside);
    if local {
        p.rect_stroke(rect.expand(1.5), rr(11.5), Stroke::new(2.0_f32, theme::GOLD), StrokeKind::Outside);
    }
    p.hline((rect.left() + 8.0)..=(rect.right() - 8.0), rect.top() + 2.5, Stroke::new(1.6_f32, theme::mix(col, Color32::WHITE, 0.6)));

    // name and time, clipped to the rising pillar so the rise reveals them
    let pc = p.with_clip_rect(rect.shrink(1.0));
    let (ny, sy) = pillar_text_ys(rect.top(), full.height());
    let pad = 6.0;
    let (name, size) = fit_text(p, &r.name, theme::ui_bold, 13.5, 10.0, rect.width() - 2.0 * pad);
    pc.text(pos2(rect.center().x, ny), Align2::CENTER_CENTER, name, theme::ui_bold(size), fade(theme::TEXT, dim));
    if let Some(sy) = sy {
        let (txt, c) = if r.disconnected {
            ("LEFT".to_string(), Color32::from_rgb(0xFF, 0x8A, 0x94))
        } else if r.alive {
            ("SURVIVED".to_string(), theme::MINT)
        } else {
            (fmt_time(r.survived_secs()), theme::SAND)
        };
        let s = art::fit_size(p, &txt, theme::ui_bold, 11.0, rect.width() - 2.0 * pad);
        pc.text(pos2(rect.center().x, sy), Align2::CENTER_CENTER, txt, theme::ui_bold(s), fade(c, dim));
    }

    // the place medal rides on top
    let pulse = if r.place == 1 { 1.0 + 0.06 * (3.0 * tm).sin() } else { 1.0 };
    let mc = pos2(rect.center().x, rect.top() - 17.0);
    let rad = 12.5 * pulse;
    art::glow(p, mc, rad * 2.6, fade(medal, 0.30 * rise));
    p.circle_filled(mc + vec2(0.0, 1.5), rad + 1.5, rgba(0, 0, 0, 0.35 * rise));
    p.circle_filled(mc, rad, fade(medal, rise));
    p.circle_stroke(mc, rad, Stroke::new(2.0_f32, fade(theme::OUTLINE, rise)));
    p.text(mc + vec2(0.0, 0.5), Align2::CENTER_CENTER, r.place.to_string(), theme::title(16.0), fade(theme::OUTLINE, rise));
    if r.place == 1 && rise > 0.9 {
        for k in 0..5 {
            let a = tm * 1.3 + k as f32 * std::f32::consts::TAU / 5.0;
            let c = mc + vec2(a.cos() * 22.0, a.sin() * 8.0 - 12.0);
            p.circle_filled(c, 1.8, fade(theme::GOLD, 0.5 + 0.5 * (a * 2.0).sin().abs()));
        }
    }
    let _ = t;
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u8, name: &str, place: u8) -> ResultRow {
        ResultRow { player: PlayerId(id), name: name.into(), colour: id, hat: 255, bot: false, place, alive: false, survived_ticks: 600, feathers: 0, coins: 3, placements: 0, disconnected: false }
    }

    fn safe_for(w: f32, h: f32, top: f32) -> Rect {
        Rect::from_min_max(pos2(0.0, top), pos2(w, h - top / 2.0))
    }

    /// Run `f` inside an egui pass with the app's fonts and a painter.
    fn with_painter<R>(f: impl FnOnce(&egui::Context, &Painter) -> R) -> R {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        ctx.begin_pass(egui::RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(412.0, 915.0))), ..Default::default() });
        let p = ctx.layer_painter(LayerId::background());
        let out = f(&ctx, &p);
        let _ = ctx.end_pass();
        out
    }

    #[test]
    fn layout_fits_and_never_reaches_the_buttons() {
        let sizes = [(320.0, 480.0), (320.0, 568.0), (360.0, 640.0), (360.0, 800.0), (412.0, 732.0), (412.0, 915.0)];
        for (w, h) in sizes {
            for n in [1usize, 2, 4, 6] {
                for top in [0.0, 28.0, 48.0] {
                    let safe = safe_for(w, h, top);
                    let pn = if n >= 2 { n.min(3) } else { 0 };
                    let l = layout_for(safe, n, pn);
                    let tag = format!("{w}x{h} n={n} top={top}");
                    assert!(l.title_y - 20.0 >= safe.top() - 0.5, "{tag}: title inside safe");
                    assert!(l.leave.bottom() <= safe.bottom() + 0.01 && l.leave.height() >= 56.0, "{tag}: leave inside safe");
                    assert!(l.primary.height() >= 56.0 && l.primary.bottom() < l.leave.top(), "{tag}: primary above leave");
                    assert!(l.table.bottom() <= l.primary.top() - 6.0, "{tag}: table reaches the buttons ({} vs {})", l.table.bottom(), l.primary.top());
                    assert!(l.table.left() >= safe.left() && l.table.right() <= safe.right(), "{tag}: table width");
                    assert!(l.sub_y + 11.0 <= l.table.top() - if l.podium.is_some() { 40.0 } else { 0.0 } + 0.01, "{tag}: subtitle collides with the next block");
                    assert!(l.rows.height() >= MIN_ROW_H - 0.01, "{tag}: rows window too short");
                    assert!(l.max_scroll >= 0.0 && (l.max_scroll == 0.0 || (l.rows.height() + l.max_scroll - l.content_h).abs() < 0.01));
                    if let Some(b) = l.podium {
                        assert!(b.base_y <= l.table.top(), "{tag}: podium overlaps table");
                        assert!(b.base_y - b.h1 - 32.0 >= l.sub_y + 10.0 - 0.01, "{tag}: medal reaches the subtitle");
                    }
                }
            }
        }
    }

    #[test]
    fn squeeze_order_is_podium_then_rows_then_drop_then_scroll() {
        let tall = layout_for(safe_for(360.0, 915.0, 0.0), 6, 3);
        assert!(tall.podium.is_some() && tall.max_scroll == 0.0);
        let mid = layout_for(safe_for(360.0, 700.0, 28.0), 6, 3);
        let short = layout_for(safe_for(360.0, 600.0, 28.0), 6, 3);
        assert!(mid.podium.map_or(0.0, |b| b.h1) < tall.podium.unwrap().h1 || mid.podium.is_none());
        assert!(short.podium.is_none() || short.row_h < 44.0);
        let tiny = layout_for(safe_for(320.0, 480.0, 48.0), 6, 3);
        assert!(tiny.podium.is_none() && tiny.max_scroll > 0.0, "tiny screens drop the podium and scroll");
    }

    #[test]
    fn podium_text_stays_inside_the_pillars() {
        with_painter(|_, p| {
            for h1 in [60.0, 80.0, 100.0, 136.0] {
                for k in 2..=4usize {
                    let places: Vec<u8> = match k {
                        2 => vec![1, 2],
                        3 => vec![1, 2, 3],
                        _ => vec![1, 2, 3, 3],
                    };
                    let b = PodiumBox { x0: 14.0, x1: 346.0, base_y: 400.0, h1 };
                    let rects = pillar_rects(&b, &places);
                    assert_eq!(rects.len(), k);
                    for (idx, r) in rects {
                        assert!(r.left() >= b.x0 - 0.01 && r.right() <= b.x1 + 0.01);
                        let (ny, sy) = pillar_text_ys(r.top(), r.height());
                        let (name, size) = fit_text(p, "Bartholomew Longname", theme::ui_bold, 13.5, 10.0, r.width() - 12.0);
                        let w = text_w(p, &name, theme::ui_bold(size));
                        assert!(w <= r.width() - 12.0 + 0.01, "name {w} wider than pillar {}", r.width());
                        assert!(ny - size * 0.65 >= r.top() && ny + size * 0.65 <= r.bottom(), "name y {ny} outside pillar {r:?} (h1 {h1}, place {})", places[idx]);
                        if let Some(sy) = sy {
                            assert!(sy - 7.0 >= r.top() && sy + 7.0 <= r.bottom(), "sub y {sy} outside pillar {r:?}");
                            assert!(sy - 7.0 >= ny + size * 0.5 - 0.5, "sub line overlaps name");
                        }
                    }
                }
            }
        });
    }

    #[test]
    fn name_cell_never_touches_the_first_numeric_column() {
        with_painter(|_, p| {
            for w in [320.0_f32, 360.0, 412.0] {
                let safe = safe_for(w, 800.0, 0.0);
                let lay = layout_for(safe, 6, 3);
                let cols = Cols::new(lay.table);
                let cell = Rect::from_min_max(pos2(cols.x(F_NAME), 0.0), pos2(cols.x(F_NAME_END), 44.0));
                for (name, tags) in [
                    ("Twelve chars", Tags { you: true, bot: true }),
                    ("Twelve chars", Tags { you: false, bot: false }),
                    ("WWWWWWWWWWWW", Tags { you: true, bot: false }),
                    ("Bartholomew the Great", Tags { you: true, bot: true }),
                ] {
                    let u = name_cell(p, cell, name, tags, Color32::WHITE, false);
                    assert!(u.right() <= cols.first_numeric_left() - 3.0, "{w}dp {name}: name cell right {} touches column at {}", u.right(), cols.first_numeric_left());
                    assert!(u.left() >= cols.x(F_CHIP) + 7.0, "{w}dp {name}: name overlaps the chip");
                    if tags.you {
                        // "(you)" is whole: its box is inside the cell and the first line is the name
                        assert!(u.bottom() <= 44.0 && u.top() >= 0.0);
                    }
                }
                // pills in the time column fit between the name and the feathers
                let sz = pill_size(p, "SURVIVED", 8.0);
                assert!(sz.x <= 2.0 * cols.w * F_TIME_HALF + 0.5, "{w}dp: SURVIVED pill {} wider than its column {}", sz.x, 2.0 * cols.w * F_TIME_HALF);
                let tw = text_w(p, "12:34.5", theme::numbers(14.0));
                assert!(tw <= 2.0 * cols.w * F_TIME_HALF, "{w}dp: time too wide");
            }
        });
    }

    #[test]
    fn ties_share_the_ordinal_in_any_row_order() {
        let base = vec![row(0, "A", 1), row(1, "B", 2), row(2, "C", 3), row(3, "D", 3), row(4, "E", 5)];
        let want = |rows: &[ResultRow]| -> Vec<(u8, String)> {
            let mut v: Vec<(u8, String)> = rows.iter().map(|r| (r.player.0, ordinal(r.place))).collect();
            v.sort();
            v
        };
        let expect = want(&base);
        assert_eq!(expect.iter().map(|(_, s)| s.as_str()).collect::<Vec<_>>(), ["1st", "2nd", "3rd", "3rd", "5th"]);
        let mut shuffled = base.clone();
        shuffled.reverse();
        shuffled.swap(0, 2);
        assert_eq!(want(&shuffled), expect);
        // the screen sorts by place only (stable), and equal places get equal pillar heights
        let view = ResultsView { rows: shuffled, end_reason: Some(EndReason::TimeCap), round: 1, solo: None };
        let s = ResultsScreen::new(view, PlayerId(1), ResultsLabels::default(), Box::new(|_| Transition::None), Box::new(|_| Transition::None));
        let places: Vec<u8> = s.results.rows.iter().filter(|r| r.place <= 3).map(|r| r.place).collect();
        assert_eq!(places, [1, 2, 3, 3]);
        let b = PodiumBox { x0: 0.0, x1: 360.0, base_y: 300.0, h1: 100.0 };
        let rects = pillar_rects(&b, &places);
        let h = |idx: usize| rects.iter().find(|(i, _)| *i == idx).unwrap().1.height();
        assert_eq!(h(2), h(3), "tied runners stand on the same step");
        assert!(h(0) > h(1) && h(1) > h(2));
        assert_eq!(ordinal(11), "11th");
        assert_eq!(ordinal(12), "12th");
        assert_eq!(ordinal(22), "22nd");
    }

    #[test]
    fn podium_hides_with_fewer_than_two_rows_or_a_big_tie() {
        assert_eq!(podium_count(&[row(0, "A", 1)]), 0);
        assert_eq!(podium_count(&[row(0, "A", 1), row(1, "B", 2)]), 2);
        assert_eq!(podium_count(&[row(0, "A", 1), row(1, "B", 1), row(2, "C", 3), row(3, "D", 3)]), 4);
        assert_eq!(podium_count(&(0..5).map(|i| row(i, "X", 1)).collect::<Vec<_>>()), 0);
        assert_eq!(podium_count(&[row(0, "A", 1), row(1, "B", 3)]), 2);
    }

    #[test]
    fn rematch_ready_and_countdown_flow() {
        let mut r = Rematch::default();
        assert_eq!(r.flow(), Flow::Idle);
        assert_eq!(r.count(), None);
        r.set(vec![(PlayerId(0), false), (PlayerId(1), true), (PlayerId(2), false)], None);
        assert_eq!(r.flow(), Flow::Waiting { ready: 1, total: 3 });
        assert_eq!(r.is_ready(PlayerId(1)), Some(true));
        assert_eq!(r.is_ready(PlayerId(0)), Some(false));
        assert_eq!(r.is_ready(PlayerId(9)), None);
        // everybody ready: the host starts the 3-2-1
        r.set(vec![(PlayerId(0), true), (PlayerId(1), true), (PlayerId(2), true)], Some(3000));
        assert_eq!(r.flow(), Flow::Count(3));
        r.countdown_ms = Some(2100);
        assert_eq!(r.count(), Some(3));
        r.countdown_ms = Some(2000);
        assert_eq!(r.count(), Some(2));
        r.countdown_ms = Some(1000);
        assert_eq!(r.count(), Some(1));
        r.countdown_ms = Some(1);
        assert_eq!(r.count(), Some(1));
        r.countdown_ms = Some(5000);
        assert_eq!(r.count(), Some(3), "never shows more than 3");
        // someone un-readies: the countdown is cancelled and we are back to waiting
        r.set(vec![(PlayerId(0), true), (PlayerId(1), false), (PlayerId(2), true)], None);
        assert_eq!(r.flow(), Flow::Waiting { ready: 2, total: 3 });
        // everyone cancels
        r.set(vec![(PlayerId(0), false), (PlayerId(1), false), (PlayerId(2), false)], None);
        assert_eq!(r.flow(), Flow::Idle);
    }

    #[test]
    fn subtitles_and_wins() {
        let mut v = ResultsView { rows: vec![row(0, "Ann", 1), row(1, "Bo", 2)], end_reason: Some(EndReason::NoRunnersLeft), round: 1, solo: None };
        assert_eq!(subtitle(&v, PlayerId(0)), "Last runner standing");
        assert_eq!(subtitle(&v, PlayerId(1)), "Ann was the last runner standing");
        assert!(local_won(&v, PlayerId(0)) && !local_won(&v, PlayerId(1)));
        v.end_reason = Some(EndReason::TimeCap);
        assert_eq!(subtitle(&v, PlayerId(1)), "Time's up");
        v.end_reason = Some(EndReason::NotEnoughPlayers);
        v.rows[1].disconnected = true;
        assert_eq!(subtitle(&v, PlayerId(0)), "Bo left the match");
        assert!(!local_won(&v, PlayerId(0)), "a win by everyone leaving is not VICTORY");
        v.end_reason = Some(EndReason::Aborted);
        assert_eq!(subtitle(&v, PlayerId(0)), "Match ended early");
        assert_eq!(fmt_time(74.34), "1:14.3");
        assert_eq!(fmt_time(5.0), "0:05.0");
    }

    #[test]
    fn painted_buttons_stay_inside_safe_and_clear_of_the_table() {
        for (w, h, top) in [(360.0_f32, 800.0, 48.0), (320.0, 480.0, 28.0), (412.0, 915.0, 48.0), (360.0, 640.0, 0.0)] {
            for n in [1usize, 4, 6] {
                let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h));
                let safe = safe_for(w, h, top);
                let mut s = ResultsScreen::sample();
                s.results.rows.truncate(n);
                s.t = 3.0;
                s.set_rematch(vec![(PlayerId(0), true)], None);
                crate::ui::widgets::BUTTON_RECTS.with(|v| *v.borrow_mut() = Some(Vec::new()));
                let ctx = egui::Context::default();
                theme::install(&ctx);
                ctx.begin_pass(egui::RawInput { screen_rect: Some(screen), ..Default::default() });
                let _ = s.paint(&ctx, screen, safe, 3.0);
                let _ = ctx.end_pass();
                let rects = crate::ui::widgets::BUTTON_RECTS.with(|v| v.borrow_mut().take()).unwrap();
                let lay = layout_for(safe, n, podium_count(&s.results.rows));
                assert_eq!(rects.len(), 3, "again, share, leave");
                for (salt, r, _) in rects {
                    assert!(r.height() >= 56.0, "{salt} {w}x{h}: {} dp tall", r.height());
                    assert!(safe.contains_rect(r), "{salt} {w}x{h} n={n}: {r:?} outside safe {safe:?}");
                    assert!(!r.intersects(lay.table), "{salt} overlaps the table");
                }
            }
        }
    }

    #[test]
    fn share_text_for_every_finish() {
        let view = |rows: Vec<ResultRow>, end: EndReason| ResultsView { rows, end_reason: Some(end), round: 1, solo: None };
        let mut me = row(0, "Me", 2);
        me.survived_ticks = (83.4 / flock_core::DT).round() as u64;
        let v = view(vec![row(1, "A", 1), me.clone(), row(2, "B", 3)], EndReason::NoRunnersLeft);
        assert_eq!(share_message(&v, PlayerId(0)).unwrap(), "I came 2nd of 3 in a Cluster Flock round \u{2014} survived 1:23. Rematch?");
        // alive at the bell
        let mut alive = me.clone();
        alive.alive = true;
        alive.place = 1;
        let v = view(vec![alive.clone(), row(1, "A", 2)], EndReason::TimeCap);
        assert_eq!(share_message(&v, PlayerId(0)).unwrap(), "I came 1st of 2 in a Cluster Flock round \u{2014} still standing at the bell. Rematch?");
        // alive but the round ended another way: a time, not "the bell"
        let v = view(vec![alive.clone(), row(1, "A", 2)], EndReason::NoRunnersLeft);
        assert!(share_message(&v, PlayerId(0)).unwrap().contains("survived"));
        // nothing to share: left, absent, or alone
        let mut gone = me.clone();
        gone.disconnected = true;
        assert!(share_message(&view(vec![gone, row(1, "A", 1)], EndReason::TimeCap), PlayerId(0)).is_none());
        assert!(share_message(&view(vec![row(1, "A", 1)], EndReason::TimeCap), PlayerId(0)).is_none());
        assert!(share_message(&view(vec![me], EndReason::TimeCap), PlayerId(0)).is_none());
    }

    #[test]
    fn ordinals_and_whole_second_times() {
        let o: Vec<String> = [1u8, 2, 3, 4, 11, 12, 13, 21, 22, 23].iter().map(|p| ordinal(*p)).collect();
        assert_eq!(o, ["1st", "2nd", "3rd", "4th", "11th", "12th", "13th", "21st", "22nd", "23rd"]);
        assert_eq!(fmt_mss(0.0), "0:00");
        assert_eq!(fmt_mss(59.99), "0:59");
        assert_eq!(fmt_mss(60.0), "1:00");
        assert_eq!(fmt_mss(605.4), "10:05");
        assert_eq!(fmt_mss(-3.0), "0:00");
    }
}
