//! Ghost-architect mode: tool selection, the placement grid, finger/keyboard
//! picking, the preview (`World::check_placement`, green valid / red invalid
//! with the reason) and placement results.
//!
//! Finger flow: press on a square to see its preview, lift on it to drop the
//! tool (a quick tap places straight away; slide off the square to cancel).
//! Keyboard: 1-4 tools, arrows move the cursor, Enter places.

use super::cameras::{to_render, to_sim_x, Projector};
use crate::platform::{KeyCode, Keys, Touches};
use crate::ui::icons::Icon;
use egui::Rect;
use flock_core::{PlaceResult, PlayerId, Reject, Role, Tool, World};
use flock_render::{CellState, CoinInst, FarmPropInst, FarmPropKind, PlacementCell};
use flock_session::FlockView;
use glam::{Vec2, Vec3};

/// Most dimmed buffer rows drawn in front of the legal band.
const MAX_BUFFER_ROWS: u32 = 14;

/// Where the "N running" pill sits under the banner. The architect view is mirrored for a
/// left-handed layout, so the pill goes to the other side from where the flock appears.
pub fn living_pill(banner: Rect, left_handed: bool) -> Rect {
    let size = egui::vec2(124.0, 26.0);
    if left_handed {
        Rect::from_min_size(egui::pos2(banner.right() - size.x, banner.bottom() + 6.0), size)
    } else {
        Rect::from_min_size(banner.left_bottom() + egui::vec2(0.0, 6.0), size)
    }
}

/// Banner for the first moments as a ghost.
pub const INSTRUCTION: &str = "You're a ghost! Tap a lit square to drop trouble";

pub fn tool_icon(t: Tool) -> Icon {
    match t {
        Tool::Coins => Icon::Coins,
        Tool::Mud => Icon::Mud,
        Tool::Hay => Icon::Hay,
        Tool::Beam => Icon::Beam,
    }
}

pub fn tool_tint(t: Tool) -> egui::Color32 {
    match t {
        Tool::Coins => egui::Color32::from_rgb(0xFF, 0xD2, 0x4A),
        Tool::Mud => egui::Color32::from_rgb(0xC8, 0x92, 0x62),
        Tool::Hay => egui::Color32::from_rgb(0xF2, 0xD0, 0x6A),
        Tool::Beam => egui::Color32::from_rgb(0xE0, 0xB0, 0x80),
    }
}

pub fn tool_name(t: Tool) -> &'static str {
    match t {
        Tool::Coins => "Coins",
        Tool::Mud => "Mud",
        Tool::Hay => "Hay bale",
        Tool::Beam => "Beam",
    }
}

pub fn tool_cost(w: &World, t: Tool) -> f32 {
    let c = &w.tuning().cost;
    match t {
        Tool::Coins => c.coins,
        Tool::Mud => c.mud,
        Tool::Hay => c.hay,
        Tool::Beam => c.beam,
    }
}

/// Plain words for a rejection.
pub fn reason(r: Reject) -> &'static str {
    match r {
        Reject::NotGhost => "Only ghosts can build",
        Reject::Duplicate => "Try that again",
        Reject::OutsideBand => "Too far away to reach",
        Reject::TooClose => "Too close to a runner",
        Reject::Occupied => "Already taken",
        Reject::Density => "Too crowded there",
        Reject::Unsafe => "That would block the way",
        Reject::NoEnergy => "Not enough energy",
    }
}

/// The cell (lane, row) under a screen point, if it is inside the placement `band`. Render x is
/// mirrored from the sim's and the ghost view faces back down the course, but the ground ray
/// takes care of both: this only converts the hit to a lane and a row.
pub fn pick_cell(proj: &Projector, p: egui::Pos2, band: flock_core::RowRange, spacing: f64, lane_w: f32, origin: f64) -> Option<(u8, u32)> {
    let g = proj.ground(p, 0.0)?;
    let x = to_sim_x(g.x);
    let lane = (x / lane_w).round() as i32 + 1;
    if !(0..=2).contains(&lane) {
        return None;
    }
    let row = ((g.z as f64 + origin) / spacing).round();
    if row < 0.0 || !band.contains(row as u32) {
        return None;
    }
    Some((lane as u8, row as u32))
}

#[derive(Clone, Copy, Debug)]
struct Cell {
    lane: u8,
    row: u32,
    check: Result<f32, Reject>,
    reserved: bool,
}

/// Feedback line: text, age, good/bad.
#[derive(Clone, Debug)]
pub struct Message {
    pub text: String,
    pub age: f32,
    pub ok: bool,
}

pub struct GhostUi {
    pub tool: Tool,
    /// Selected square (lane, row).
    pub cursor: Option<(u8, u32)>,
    finger: Option<u64>,
    grid: Vec<Cell>,
    grid_tick: u64,
    grid_tool: Option<Tool>,
    pub msg: Option<Message>,
    /// Reject shake 0..1 (decays).
    pub shake: f32,
    /// Seconds since the last accepted drop (for the toolbar flash).
    pub since_drop: f32,
    kb: bool,
    /// Staged shots: keep the cursor on this lane and this many rows into the band.
    pub demo_cursor: Option<(u8, u32)>,
    /// Placement requests to hand to the driver (drained by the Run screen).
    pub outbox: Vec<(Tool, u8, u32)>,
}

impl Default for GhostUi {
    fn default() -> Self {
        Self::new()
    }
}

impl GhostUi {
    pub fn new() -> GhostUi {
        GhostUi { tool: Tool::Hay, cursor: None, finger: None, grid: Vec::new(), grid_tick: 0, grid_tool: None, msg: None, shake: 0.0, since_drop: 9.0, kb: false, demo_cursor: None, outbox: Vec::new() }
    }

    pub fn select(&mut self, t: Tool) {
        self.tool = t;
    }

    fn say(&mut self, text: impl Into<String>, ok: bool) {
        self.msg = Some(Message { text: text.into(), age: 0.0, ok });
    }

    /// Check one square for the current tool.
    pub fn check(&self, w: &World, me: PlayerId, lane: u8, row: u32) -> Result<f32, Reject> {
        w.check_placement(me, self.tool, lane, row)
    }

    fn refresh_grid(&mut self, w: &World, me: PlayerId) {
        let band = w.placement_band();
        self.grid.clear();
        self.grid_tick = w.tick();
        self.grid_tool = Some(self.tool);
        if band.is_empty() {
            return;
        }
        let pending: Vec<(u8, u32)> = w.course().placed().iter().filter(|o| !o.is_live(w.tick())).map(|o| (o.lane, o.row)).collect();
        for row in band.iter() {
            for lane in 0..3u8 {
                let check = w.check_placement(me, self.tool, lane, row);
                let reserved = matches!(check, Err(Reject::Occupied)) && pending.contains(&(lane, row));
                self.grid.push(Cell { lane, row, check, reserved });
            }
        }
    }

    /// Per-frame logic; placement requests collect in `outbox`.
    #[allow(clippy::too_many_arguments)]
    pub fn update(&mut self, view: &FlockView, proj: &Projector, touches: &Touches, keys: &Keys, ui_rects: &[Rect], dt: f32, origin: f64) {
        let w = &view.world;
        let me = view.local;
        self.shake = (self.shake - dt * 3.0).max(0.0);
        self.since_drop += dt;
        if let Some(m) = self.msg.as_mut() {
            m.age += dt;
        }
        if self.msg.as_ref().is_some_and(|m| m.age > 2.2) {
            self.msg = None;
        }
        if !matches!(w.player(me).map(|p| p.role), Some(Role::Ghost { .. })) {
            self.finger = None;
            return;
        }
        // results of earlier placements
        for (_, res) in &view.place_results {
            self.on_result(*res);
        }
        if let Some((lane, off)) = self.demo_cursor {
            let b = w.placement_band();
            self.cursor = (!b.is_empty()).then(|| (lane, (b.first + off).min(b.last)));
        }
        // grid refresh at ~10 Hz or when the tool changed
        if self.grid_tool != Some(self.tool) || w.tick().saturating_sub(self.grid_tick) >= 6 || self.grid.is_empty() {
            self.refresh_grid(w, me);
        }
        let band = w.placement_band();
        let spacing = w.tuning().row_spacing as f64;
        let lane_w = w.tuning().lane_width;
        let cell_at = |p: egui::Pos2| pick_cell(proj, p, band, spacing, lane_w, origin);
        // finger
        for t in touches.began() {
            if self.finger.is_none() && !ui_rects.iter().any(|r| r.contains(t.start)) {
                self.finger = Some(t.id);
                self.kb = false;
            }
        }
        if let Some(id) = self.finger {
            if let Some(t) = touches.get(id) {
                self.cursor = cell_at(t.pos);
            } else if let Some(t) = touches.ended().iter().find(|t| t.id == id) {
                let c = cell_at(t.pos);
                self.finger = None;
                self.cursor = None;
                match c {
                    Some((lane, row)) => self.try_place(w, me, lane, row),
                    None => {
                        if t.travel() < 18.0 {
                            self.say("Tap a lit square", false);
                        }
                    }
                }
            } else {
                self.finger = None;
            }
        }
        // keyboard
        let mut moved = false;
        let (mut dl, mut dr) = (0i32, 0i64);
        // The view faces back toward the runners: screen right is the low lanes, and the squares
        // nearest the camera (the top of the keyboard's "up") are the high rows.
        if keys.pressed(KeyCode::ArrowLeft) || keys.pressed(KeyCode::KeyA) {
            dl -= 1;
            moved = true;
        }
        if keys.pressed(KeyCode::ArrowRight) || keys.pressed(KeyCode::KeyD) {
            dl += 1;
            moved = true;
        }
        if keys.pressed(KeyCode::ArrowUp) || keys.pressed(KeyCode::KeyW) {
            dr -= 1; // up the screen = further from the camera = nearer the runners = a lower row
            moved = true;
        }
        if keys.pressed(KeyCode::ArrowDown) || keys.pressed(KeyCode::KeyS) {
            dr += 1;
            moved = true;
        }
        for (k, t) in [(KeyCode::Digit1, Tool::Coins), (KeyCode::Digit2, Tool::Mud), (KeyCode::Digit3, Tool::Hay), (KeyCode::Digit4, Tool::Beam)] {
            if keys.pressed(k) {
                self.tool = t;
            }
        }
        if moved && !band.is_empty() {
            self.kb = true;
            let (lane, row) = self.cursor.unwrap_or((1, band.first));
            let lane = (lane as i32 - dl).clamp(0, 2) as u8;
            let row = (row as i64 + dr).clamp(band.first as i64, band.last as i64) as u32;
            self.cursor = Some((lane, row));
        }
        if self.kb {
            if self.cursor.is_some_and(|(_, r)| !band.contains(r)) {
                self.cursor = None;
            }
            if keys.pressed(KeyCode::Enter) || keys.pressed(KeyCode::NumpadEnter) {
                if let Some((lane, row)) = self.cursor {
                    self.try_place(w, me, lane, row);
                }
            }
        }
    }

    fn try_place(&mut self, w: &World, me: PlayerId, lane: u8, row: u32) {
        match w.check_placement(me, self.tool, lane, row) {
            Ok(_) => self.outbox.push((self.tool, lane, row)),
            Err(r) => self.on_result(Err(r)),
        }
    }

    fn on_result(&mut self, res: PlaceResult) {
        match res {
            Ok(a) => {
                self.since_drop = 0.0;
                self.say(format!("Dropped! -{} energy", a.cost as u32), true);
                self.grid_tool = None; // refresh now
            }
            Err(r) => {
                self.shake = 1.0;
                self.say(reason(r), false);
                crate::sound::with(|a| a.play(flock_audio::Sfx::PlaceDenied, 0.9, 1.0));
            }
        }
    }

    /// Grid squares for the debug overlay: lane, row, verdict.
    pub fn debug_cells(&self) -> impl Iterator<Item = (u8, u32, Result<f32, Reject>)> + '_ {
        self.grid.iter().map(|c| (c.lane, c.row, c.check))
    }

    /// What was accepted this frame, for the ghost's release animation.
    pub fn accepted_now(view: &FlockView) -> bool {
        view.place_results.iter().any(|(_, r)| r.is_ok())
    }

    /// Text under the toolbar line: the cursor square's verdict.
    pub fn info(&self, w: &World, me: PlayerId) -> Option<(String, bool)> {
        let (lane, row) = self.cursor?;
        Some(match w.check_placement(me, self.tool, lane, row) {
            Ok(c) => (format!("{}  -  {} energy", tool_name(self.tool), c as u32), true),
            Err(r) => (reason(r).to_string(), false),
        })
    }

    /// Placement cells, preview prop and lure coins for the scene.
    pub fn layer(&self, w: &World, me: PlayerId, origin: f64, time: f32, cells: &mut Vec<PlacementCell>, coins: &mut Vec<CoinInst>) -> Option<FarmPropInst> {
        cells.clear();
        let t = w.tuning();
        let size = Vec2::new(t.lane_width - 0.2, t.row_spacing - 0.4);
        for c in &self.grid {
            let selected = self.cursor == Some((c.lane, c.row));
            let state = if selected {
                CellState::Selected
            } else if c.reserved {
                CellState::Reserved
            } else if c.check.is_ok() {
                CellState::Valid
            } else {
                CellState::Invalid
            };
            let x = (c.lane as f32 - 1.0) * t.lane_width;
            let z = c.row as f64 * t.row_spacing as f64;
            cells.push(PlacementCell { pos: to_render(x, 0.0, z, origin), size, state });
        }
        // The buffer: rows between the leading runner and the nearest square the sim will accept
        // (it refuses anything inside the reaction window). Dim and not tappable, so the gap the
        // runners get is visible.
        let band = w.placement_band();
        if !band.is_empty() {
            let sp = t.row_spacing as f64;
            let first = ((w.leader_z().unwrap_or(0.0) / sp).floor() as i64 + 1).max(0) as u32;
            for row in first.max(band.first.saturating_sub(MAX_BUFFER_ROWS))..band.first {
                for lane in 0..3u8 {
                    let x = (lane as f32 - 1.0) * t.lane_width;
                    cells.push(PlacementCell { pos: to_render(x, 0.0, row as f64 * sp, origin), size, state: CellState::Closed });
                }
            }
        }
        let (lane, row) = self.cursor?;
        let ok = w.check_placement(me, self.tool, lane, row).is_ok();
        let x = (lane as f32 - 1.0) * t.lane_width;
        let z = row as f64 * t.row_spacing as f64;
        let pos = to_render(x, 0.0, z, origin);
        let h = &t.hazards;
        let mut inst = match self.tool {
            Tool::Hay => FarmPropInst::new(FarmPropKind::HayBale, pos, Vec3::new(h.hay_width, h.hay_top, h.hay_depth)),
            Tool::Beam => {
                let mut b = FarmPropInst::new(FarmPropKind::Beam { span_lanes: 1 }, pos, Vec3::new(h.beam_width, h.beam_underside, h.beam_depth));
                b.bar_top = h.beam_underside + h.beam_thickness;
                b
            }
            Tool::Mud => FarmPropInst::new(FarmPropKind::Mud, pos, Vec3::new(h.mud_width, 0.1, h.mud_depth)),
            Tool::Coins => {
                for k in 0..5 {
                    let dz = (k as f32 - 2.0) * h.lure_spacing;
                    coins.push(CoinInst { pos: Vec3::new(pos.x, h.coin_height, pos.z + dz), spin: time * 3.0 + k as f32, visible: true });
                }
                return None;
            }
        };
        inst.alpha = 0.6;
        inst.danger = !ok;
        Some(inst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reject_has_words() {
        for r in [Reject::NotGhost, Reject::Duplicate, Reject::OutsideBand, Reject::TooClose, Reject::Occupied, Reject::Density, Reject::Unsafe, Reject::NoEnergy] {
            assert!(reason(r).len() > 8);
        }
    }

    #[test]
    fn finger_down_previews_and_lifting_on_a_valid_square_places() {
        use super::super::presenter::Presenter;
        use crate::platform::TouchPhase;
        use flock_core::{BotSkill, InputFrame};
        let mut run = flock_session::LocalRun::practice(5, 5, BotSkill::NORMAL);
        run.world_mut().force_eliminate(PlayerId(0));
        let mut pres = Presenter::new();
        let mut t = 0;
        while t < 9000 {
            t += 16;
            flock_session::RunDriver::update(&mut run, t, InputFrame::default());
            pres.build(flock_session::RunDriver::view(&run).unwrap(), 1.0 / 60.0, 0.45, false);
        }
        let mut ui = GhostUi::new();
        ui.select(Tool::Hay);
        let size = egui::vec2(412.0, 915.0);
        let proj = Projector::new(&pres.scene.camera, size);
        let keys = Keys::default();
        let mut touches = Touches::new();
        let view = flock_session::RunDriver::view(&run).unwrap();
        ui.update(view, &proj, &touches, &keys, &[], 0.016, pres.origin);
        let (lane, row, _) = ui.debug_cells().find(|c| c.2.is_ok()).expect("a valid square in the band");
        let w = &view.world;
        let at = to_render((lane as f32 - 1.0) * w.tuning().lane_width, 0.0, row as f64 * w.tuning().row_spacing as f64, pres.origin);
        let pos = proj.project(at).expect("on screen");
        touches.touch(1, TouchPhase::Started, pos);
        ui.update(view, &proj, &touches, &keys, &[], 0.016, pres.origin);
        assert_eq!(ui.cursor, Some((lane, row)), "finger down shows the preview square");
        assert!(ui.outbox.is_empty(), "nothing placed until the finger lifts");
        touches.end_frame(0.016);
        touches.touch(1, TouchPhase::Ended, pos);
        ui.update(view, &proj, &touches, &keys, &[], 0.016, pres.origin);
        assert_eq!(ui.outbox, vec![(Tool::Hay, lane, row)]);
        // a finger that starts on the toolbar is ignored by the grid
        let mut touches = Touches::new();
        let mut ui2 = GhostUi::new();
        touches.touch(2, TouchPhase::Started, pos);
        ui2.update(view, &proj, &touches, &keys, &[egui::Rect::from_center_size(pos, egui::vec2(60.0, 60.0))], 0.016, pres.origin);
        assert_eq!(ui2.cursor, None);
    }

    /// The real sim: an eliminated local player, the flock running. Through the architect camera
    /// the survivors are on screen, every legal cell is big enough, and tapping any part of a
    /// cell picks that lane and row back (render x is mirrored, the view faces the other way).
    #[test]
    fn architect_camera_sees_the_runners_and_every_cell_picks_back() {
        use super::super::cameras::to_render;
        use super::super::presenter::Presenter;
        use flock_core::{BotSkill, InputFrame};
        let size = egui::vec2(412.0, 915.0);
        for (seed, left) in [(5u64, false), (9, true)] {
            let mut run = flock_session::LocalRun::practice(seed, 5, BotSkill::NORMAL);
            run.world_mut().force_eliminate(PlayerId(0));
            let mut pres = Presenter::new();
            pres.left_handed = left;
            let mut ui = GhostUi::new();
            let (mut t, mut checks, mut max_lag) = (0u64, 0, 0.0f64);
            let (mut min_cw, mut min_ch, mut min_rh) = (f32::MAX, f32::MAX, f32::MAX);
            while t < 40_000 {
                t += 16;
                flock_session::RunDriver::update(&mut run, t, InputFrame::default());
                let view = flock_session::RunDriver::view(&run).unwrap();
                pres.build(view, 1.0 / 60.0, size.x / size.y, false);
                if t < 7000 || t % 400 != 0 || view.world.living_count() == 0 {
                    continue;
                }
                let w = &view.world;
                let proj = Projector::new(&pres.scene.camera, size);
                // ---- the flock
                let lead = w.leader_z().unwrap();
                for r in w.runners() {
                    max_lag = max_lag.max(lead - r.pos.z);
                    let feet = proj.project(to_render(r.pos.x, r.pos.y, r.pos.z, pres.origin)).expect("in front of the camera");
                    let head = proj.project(to_render(r.pos.x, r.pos.y + 2.2, r.pos.z, pres.origin)).unwrap();
                    assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains(feet) && egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains(head), "runner off screen: {feet:?} {head:?} (t {t})");
                    min_rh = min_rh.min((feet.y - head.y).abs());
                }
                // ---- the cells
                ui.update(view, &proj, &Touches::new(), &Keys::default(), &[], 0.016, pres.origin);
                let band = w.placement_band();
                let (lane_w, sp) = (w.tuning().lane_width, w.tuning().row_spacing as f64);
                for (lane, row, _) in ui.debug_cells() {
                    let c = to_render((lane as f32 - 1.0) * lane_w, 0.0, row as f64 * sp, pres.origin);
                    let p = |dx: f32, dz: f32| proj.project(c + Vec3::new(dx, 0.0, dz)).unwrap();
                    let (hw, hd) = ((lane_w - 0.2) / 2.0, (w.tuning().row_spacing - 0.4) / 2.0);
                    min_cw = min_cw.min((p(hw, -hd).x - p(-hw, -hd).x).abs()).min((p(hw, hd).x - p(-hw, hd).x).abs());
                    min_ch = min_ch.min(((p(0.0, hd).y) - (p(0.0, -hd).y)).abs());
                    // tapping the middle and well inside each corner picks this very cell
                    for (dx, dz) in [(0.0, 0.0), (hw * 0.8, hd * 0.8), (-hw * 0.8, hd * 0.8), (hw * 0.8, -hd * 0.8), (-hw * 0.8, -hd * 0.8)] {
                        let tap = p(dx, dz);
                        assert_eq!(pick_cell(&proj, tap, band, sp, lane_w, pres.origin), Some((lane, row)), "tap {tap:?} on lane {lane} row {row} (left {left})");
                    }
                    checks += 1;
                }
                // a tap in the buffer or off the road picks nothing
                let buffer = proj.project(to_render(0.0, 0.0, (band.first as f64 - 2.0) * sp, pres.origin)).unwrap();
                assert_eq!(pick_cell(&proj, buffer, band, sp, lane_w, pres.origin), None);
            }
            assert!(checks > 100, "checked {checks} cells");
            println!("sim check (seed {seed}, left-handed {left}): {checks} cells picked back; min cell {min_cw:.1} x {min_ch:.1} dp; min runner height {min_rh:.1} dp; pack lag behind the leader {max_lag:.1} m");
            assert!(min_cw >= 44.0 && min_ch >= 30.0 && min_rh >= 18.0, "cell {min_cw} x {min_ch}, runner {min_rh}");
            assert!(max_lag <= 14.0, "the pack stays within the 14 m the camera was tuned for: {max_lag}");
        }
    }

    #[test]
    fn tool_costs_come_from_tuning() {
        let w = World::new(flock_core::MatchConfig::multi(1, 2, None));
        assert_eq!(tool_cost(&w, Tool::Hay), w.tuning().cost.hay);
        assert_eq!(tool_cost(&w, Tool::Coins), w.tuning().cost.coins);
    }
}
