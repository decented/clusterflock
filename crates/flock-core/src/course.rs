//! The course: compact generated rows plus ghost placements (SPEC §4, §7).
//!
//! A [`Row`] is 13 bytes of data (`row_index`, three [`Cell`] bytes, a coin
//! taken-mask, the barrel release tick) so the host can stream rows to
//! clients. Concrete objects are *derived* from rows as [`Obj`] values with
//! stable [`ObjId`]s.
//!
//! **Determinism.** Generated rows are a pure function of the match seed,
//! the row index, the tuning and [`GENERATOR_VERSION`] whenever the
//! generator context is ([`GenCtx::difficulty_ramp_m`] > 0, solo rules): the
//! RNG is [`SplitMix64`] with pinned draw functions and every number that
//! feeds a decision comes from IEEE `+ - * /` (see `det.rs`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::det::{self, mix64, SplitMix64};
use crate::feas::{self, CellReq, DpRow, FeasParams, Frontier, MudSpans};
use crate::tuning::{HazardGeom, Tuning};
use crate::{PlayerId, Tick, Tool};

/// Version of the course generator. Bump it whenever a change alters the rows
/// some seed produces (and update the golden fingerprint test): daily runs
/// compare scores only within one version.
pub const GENERATOR_VERSION: u32 = 3;

/// Stable object id. Generated objects: `row * 32 + lane * 8 + slot`
/// (slot 0 solid, 1 mud, 2..=6 coins, 7 power-up). Ghost placements: sequential from
/// [`ObjId::ARCHITECT_BASE`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjId(pub u32);

impl ObjId {
    pub const ARCHITECT_BASE: u32 = 0x8000_0000;
    pub fn generated(row: u32, lane: u8, slot: u8) -> ObjId {
        ObjId(row.wrapping_mul(32) | ((lane as u32 & 3) << 3) | (slot as u32 & 7))
    }
    pub fn is_architect(self) -> bool {
        self.0 >= Self::ARCHITECT_BASE
    }
    /// For generated ids: (row, lane, slot).
    pub fn decode(self) -> Option<(u32, u8, u8)> {
        if self.is_architect() {
            None
        } else {
            Some((self.0 / 32, ((self.0 >> 3) & 3) as u8, (self.0 & 7) as u8))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BeamSpan {
    One,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ObjKind {
    Hay,
    Beam { span: BeamSpan },
    Block,
    Mud,
    Coin,
    /// A lying barrel that rolls along its lane (`Obj::dir`, `vz`, `roll`).
    /// Jumpable like hay; frontal contact is a `Crash`.
    Barrel,
    /// A power-up pickup (collected like a coin; `Event::PowerTaken`).
    Power { kind: PowerKind },
}

/// Power-up kinds (cell bits 6-7: 1 Magnet, 2 Doubler, 3 Shield).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PowerKind {
    /// Collects coins near the runner (`Tuning::magnet_*`) for `magnet_s`.
    Magnet,
    /// Every coin counts 2 for `doubler_s`.
    Doubler,
    /// Held up to `shield_s`: the next feather-losing crash costs no feather.
    Shield,
}

impl PowerKind {
    pub const ALL: [PowerKind; 3] = [PowerKind::Magnet, PowerKind::Doubler, PowerKind::Shield];
    fn code(self) -> u8 {
        match self {
            PowerKind::Magnet => 1,
            PowerKind::Doubler => 2,
            PowerKind::Shield => 3,
        }
    }
}

impl ObjKind {
    pub fn damage(self) -> Damage {
        match self {
            ObjKind::Hay | ObjKind::Beam { .. } | ObjKind::Block | ObjKind::Barrel => Damage::Crash,
            ObjKind::Mud => Damage::Slow,
            ObjKind::Coin | ObjKind::Power { .. } => Damage::None,
        }
    }
    pub fn is_solid(self) -> bool {
        self.damage() == Damage::Crash
    }
}

/// Explicit damage class (SPEC §4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Damage {
    Crash,
    Slow,
    None,
}

/// A concrete course object (derived view; cheap to copy).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Obj {
    pub id: ObjId,
    pub kind: ObjKind,
    /// Lane 0..=2 (an all-lanes beam reports lane 1).
    pub lane: u8,
    pub row: u32,
    /// Centre.
    pub x: f32,
    pub z: f64,
    /// Bottom of the object (beam underside; coin centre height for coins).
    pub y: f32,
    /// Top of the object (for coins: centre + radius).
    pub top: f32,
    pub half_w: f32,
    pub half_d: f32,
    /// Placing ghost, if any.
    pub by: Option<PlayerId>,
    /// Tick at which it becomes solid/live (0 for generated objects).
    pub solid_at: Tick,
    /// Barrels: direction of travel along z (-1 rolls toward the runners, +1
    /// away); 0 for everything else.
    pub dir: i8,
    /// Barrels: current z velocity (0 until released and once the roll has
    /// finished); 0 for everything else.
    pub vz: f32,
    /// Barrels: roll angle in radians about the x axis (`travel / radius`;
    /// positive = rolled toward +z). 0 for everything else.
    pub roll: f32,
    /// Barrels: has the roll started (see `Course::release_barrel`)?
    pub released: bool,
}

impl Obj {
    pub fn damage(&self) -> Damage {
        self.kind.damage()
    }
    pub fn is_live(&self, now: Tick) -> bool {
        now >= self.solid_at
    }
    pub fn z0(&self) -> f64 {
        self.z - self.half_d as f64
    }
    pub fn z1(&self) -> f64 {
        self.z + self.half_d as f64
    }
    /// Lanes covered (inclusive).
    pub fn lanes(&self) -> (u8, u8) {
        match self.kind {
            ObjKind::Beam { span: BeamSpan::All } => (0, 2),
            _ => (self.lane, self.lane),
        }
    }
    pub fn covers_lane(&self, lane: u8) -> bool {
        let (a, b) = self.lanes();
        lane >= a && lane <= b
    }
    /// What the feasibility DP needs in a covered lane.
    pub fn cell_req(&self) -> CellReq {
        let (z0, z1) = (self.z0(), self.z1());
        match self.kind {
            ObjKind::Hay | ObjKind::Barrel => CellReq::Jump { z0, z1 },
            ObjKind::Beam { .. } => CellReq::Duck { z0, z1 },
            ObjKind::Block => CellReq::Blocked { z0, z1 },
            ObjKind::Mud | ObjKind::Coin | ObjKind::Power { .. } => CellReq::Free,
        }
    }
}

/// Solid content of a generated cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SolidKind {
    Hay,
    Beam,
    /// Part of a beam across all three lanes (set in every cell of the row).
    BeamAll,
    Block,
    /// A barrel that rolls toward the runners (towards -z).
    BarrelToward,
    /// A barrel that rolls away from the runners (towards +z).
    BarrelAway,
}

/// Coin layout in a generated cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CoinPattern {
    None,
    /// Four ground coins along the row.
    Line,
    /// Five coins along a jump arc (over hay).
    Arc,
}

/// One lane of a generated row, packed in a byte:
/// bits 0-2 solid (0 none, 1 hay, 2 beam, 3 beam-all, 4 block, 5 barrel
/// toward, 6 barrel away), bit 3 mud,
/// bits 4-5 coins (0 none, 1 line, 2 arc),
/// bits 6-7 power-up (0 none, 1 magnet, 2 doubler, 3 shield).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Cell(pub u8);

impl Cell {
    pub const EMPTY: Cell = Cell(0);
    pub fn solid(self) -> Option<SolidKind> {
        match self.0 & 7 {
            1 => Some(SolidKind::Hay),
            2 => Some(SolidKind::Beam),
            3 => Some(SolidKind::BeamAll),
            4 => Some(SolidKind::Block),
            5 => Some(SolidKind::BarrelToward),
            6 => Some(SolidKind::BarrelAway),
            _ => None,
        }
    }
    pub fn mud(self) -> bool {
        self.0 & 8 != 0
    }
    pub fn coins(self) -> CoinPattern {
        match (self.0 >> 4) & 3 {
            1 => CoinPattern::Line,
            2 => CoinPattern::Arc,
            _ => CoinPattern::None,
        }
    }
    pub fn with_solid(self, s: Option<SolidKind>) -> Cell {
        let code = match s {
            None => 0,
            Some(SolidKind::Hay) => 1,
            Some(SolidKind::Beam) => 2,
            Some(SolidKind::BeamAll) => 3,
            Some(SolidKind::Block) => 4,
            Some(SolidKind::BarrelToward) => 5,
            Some(SolidKind::BarrelAway) => 6,
        };
        Cell((self.0 & !7) | code)
    }
    pub fn with_mud(self, m: bool) -> Cell {
        Cell(if m { self.0 | 8 } else { self.0 & !8 })
    }
    pub fn with_coins(self, c: CoinPattern) -> Cell {
        let code = match c {
            CoinPattern::None => 0,
            CoinPattern::Line => 1,
            CoinPattern::Arc => 2,
        };
        Cell((self.0 & !0x30) | (code << 4))
    }
    pub fn power(self) -> Option<PowerKind> {
        match self.0 >> 6 {
            1 => Some(PowerKind::Magnet),
            2 => Some(PowerKind::Doubler),
            3 => Some(PowerKind::Shield),
            _ => None,
        }
    }
    pub fn with_power(self, p: Option<PowerKind>) -> Cell {
        Cell((self.0 & 0x3F) | (p.map_or(0, PowerKind::code) << 6))
    }
}

/// A generated course row at `z = row_index * row_spacing`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Row {
    pub row_index: u32,
    pub cells: [Cell; 3],
    /// Bit `lane * 5 + k` set = coin k of that lane already collected; bit 15
    /// set = the row's power-up was collected.
    pub coins_taken: u16,
    /// Tick at which this row's barrels started rolling (0 = not yet). Set by
    /// the host on release (`Event::BarrelReleased`, `Course::release_barrel`).
    #[serde(default)]
    pub release: u32,
}

impl Row {
    pub fn empty(row_index: u32) -> Row {
        Row { row_index, cells: [Cell::EMPTY; 3], coins_taken: 0, release: 0 }
    }
    /// Does this row hold a barrel?
    pub fn has_barrel(&self) -> bool {
        self.cells.iter().any(|c| matches!(c.solid(), Some(SolidKind::BarrelToward | SolidKind::BarrelAway)))
    }
    pub fn has_solid(&self) -> bool {
        self.cells.iter().any(|c| c.solid().is_some())
    }
    pub fn coin_taken(&self, lane: u8, k: u8) -> bool {
        self.coins_taken & (1 << (lane as u16 * 5 + k as u16)) != 0
    }
    /// The row's power-up (at most one per row) was collected.
    pub fn power_taken(&self) -> bool {
        self.coins_taken & POWER_TAKEN != 0
    }
    /// The row's power-up, if any: `(lane, kind)` (taken or not).
    pub fn power(&self) -> Option<(u8, PowerKind)> {
        (0..3u8).find_map(|l| self.cells[l as usize].power().map(|k| (l, k)))
    }
}

const POWER_TAKEN: u16 = 1 << 15;

/// Inputs the generator needs from the world for one batch of rows.
#[derive(Clone, Copy, Debug)]
pub struct GenCtx {
    pub seed: u64,
    /// 0..1, already including time (multi). Used when `difficulty_ramp_m` is 0.
    pub difficulty: f32,
    /// `> 0`: per-row difficulty `1 - e^(-max(row_z - run_in, difficulty_floor_m) / difficulty_ramp_m)`
    /// (solo; a pure function of the row, independent of when it is generated).
    pub difficulty_ramp_m: f32,
    /// See `difficulty_ramp_m`.
    pub difficulty_floor_m: f32,
    /// Procedural density multiplier (multi: `1 / (1 + 0.25 * ghosts)`).
    pub density: f32,
    /// Rows before this distance get only gentle patterns (a single hay or beam, wide gaps).
    /// 0 = no gentle opening (solo).
    pub gentle_until: f64,
    /// Metres after `gentle_until` over which the harder patterns are phased in.
    pub ramp_m: f64,
    /// Mean metres between power-ups (0 = none).
    pub powerup_gap_m: f32,
    /// Relative seeded jitter of each gap (0.35 = ±35 %).
    pub powerup_jitter: f32,
    /// Spawn weights of Magnet, Doubler, Shield.
    pub powerup_weights: [u32; 3],
}

impl GenCtx {
    /// A context with no power-ups and a fixed difficulty (tests, tools).
    pub fn fixed(seed: u64, difficulty: f32) -> GenCtx {
        GenCtx {
            seed,
            difficulty,
            difficulty_ramp_m: 0.0,
            difficulty_floor_m: 0.0,
            density: 1.0,
            gentle_until: 0.0,
            ramp_m: 0.0,
            powerup_gap_m: 0.0,
            powerup_jitter: 0.0,
            powerup_weights: [0; 3],
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct GenState {
    last_solid_row: Option<u32>,
    trail_lane: u8,
    trail_left: u8,
    /// Row index at or after which the next power-up goes.
    #[serde(default)]
    next_power_row: Option<u32>,
}

/// Geometry the course needs to derive objects (copied from `Tuning`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CourseGeom {
    pub row_spacing: f32,
    pub lane_width: f32,
    pub fence_x: f32,
    pub run_in: f32,
    pub empty: bool,
    pub h: HazardGeom,
    /// Coin heights for an `Arc` (z offsets −3, −1.5, 0, 1.5, 3 m).
    pub arc_heights: [f32; 5],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Course {
    pub geom: CourseGeom,
    feas: FeasParams,
    rows: BTreeMap<u32, Row>,
    placed: Vec<Obj>,
    /// Next row index the generator will produce.
    next_gen: u32,
    remote: bool,
    frontier: Frontier,
    next_arch: u32,
    gen: GenState,
    /// Tick the barrel positions are evaluated at (`set_clock`; the world
    /// sets it every step, a remote client must too).
    #[serde(default)]
    clock: Tick,
}

pub const ARC_OFFSETS: [f32; 5] = [-3.0, -1.5, 0.0, 1.5, 3.0];
pub const LINE_OFFSETS: [f32; 4] = [-2.25, -0.75, 0.75, 2.25];

/// Stable per-row RNG from `(match_seed, row_index)` (pinned: SplitMix64).
pub fn row_rng(seed: u64, row: u32) -> SplitMix64 {
    SplitMix64::new(mix64(seed ^ mix64(row as u64 ^ 0xF10C_C0DE)))
}

/// Separate per-row stream for power-ups (independent of obstacle draws).
fn power_rng(seed: u64, row: u32) -> SplitMix64 {
    SplitMix64::new(mix64(seed ^ mix64(row as u64 ^ 0x9077_E2B0_57A2)))
}

impl Course {
    pub fn new(t: &Tuning) -> Course {
        // Arc: a nominal 12 m/s jump peaking over the row centre; coins ride
        // half a metre above the body bottom.
        let v = 12.0f32;
        let t_apex = t.jump_vy / t.gravity;
        let mut arc = [0.0f32; 5];
        for (i, dz) in ARC_OFFSETS.iter().enumerate() {
            let tt = (t_apex + dz / v).max(0.0);
            let y = (t.jump_vy * tt - 0.5 * t.gravity * tt * tt).max(0.0);
            arc[i] = y + 0.55;
        }
        Course {
            geom: CourseGeom {
                row_spacing: t.row_spacing.max(1.0),
                lane_width: t.lane_width,
                fence_x: t.fence_x,
                run_in: t.run_in,
                empty: t.debug_empty_course,
                h: t.hazards.clone(),
                arc_heights: arc,
            },
            feas: FeasParams::from_tuning(t),
            rows: BTreeMap::new(),
            placed: Vec::new(),
            next_gen: 0,
            remote: false,
            frontier: [Some(-1.0e9); 3],
            next_arch: ObjId::ARCHITECT_BASE,
            gen: GenState::default(),
            clock: 0,
        }
    }

    /// A client-side course: never generates; filled by `insert_row`,
    /// `add_placement`, `remove_coin` from host data.
    pub fn new_remote(t: &Tuning) -> Course {
        Course { remote: true, ..Course::new(t) }
    }

    pub fn is_remote(&self) -> bool {
        self.remote
    }

    /// Set the tick at which barrels are positioned (`z(tick)`). The world
    /// does this every step; a client calls it with the tick it renders.
    pub fn set_clock(&mut self, tick: Tick) {
        self.clock = tick;
    }
    pub fn clock(&self) -> Tick {
        self.clock
    }

    /// Start the roll of the barrels of `row` at `tick` (idempotent: the
    /// first release wins). Host: from the trigger; client: on
    /// `Event::BarrelReleased { row, tick }`. Returns true if it changed.
    pub fn release_barrel(&mut self, row: u32, tick: Tick) -> bool {
        match self.rows.get_mut(&row) {
            Some(r) if r.release == 0 && r.has_barrel() => {
                r.release = (tick.min(u32::MAX as u64 - 1) as u32).max(1);
                true
            }
            _ => false,
        }
    }

    /// Unreleased barrel rows whose trigger distance the point `z` has reached
    /// (home z minus `barrel_trigger_m` <= `z`), lowest row first.
    pub fn barrels_to_release(&self, z: f64) -> Vec<u32> {
        let reach = z + self.geom.h.barrel_trigger_m as f64;
        let hi = self.row_at(reach.max(0.0)).saturating_add(1);
        self.rows
            .range(..=hi)
            .rev()
            .take(((self.geom.h.barrel_trigger_m / self.geom.row_spacing) as usize) + 4)
            .filter(|(_, r)| r.release == 0 && r.has_barrel() && self.row_z(r.row_index) <= reach)
            .map(|(&i, _)| i)
            .collect()
    }

    /// Barrel state of `row` for travel direction `dir` at the current clock:
    /// `(z, vz, roll, released)`. A pure function of the row data and the
    /// clock: `z = home + dir * speed * min(elapsed, barrel_travel_s)`.
    pub fn barrel_state(&self, row: &Row, dir: i8) -> (f64, f32, f32, bool) {
        let h = &self.geom.h;
        let home = row.row_index as f64 * self.geom.row_spacing as f64;
        if row.release == 0 {
            return (home, 0.0, 0.0, false);
        }
        let speed = if dir < 0 { h.barrel_speed_toward } else { h.barrel_speed_away };
        let e = self.clock.saturating_sub(row.release as u64) as f32 * crate::DT;
        let cap = h.barrel_travel_s.max(0.0);
        let rolling = self.clock >= row.release as u64 && e < cap;
        let travel = dir as f32 * speed * e.min(cap);
        let vz = if rolling { dir as f32 * speed } else { 0.0 };
        (home + travel as f64, vz, travel / h.barrel_radius.max(0.05), true)
    }

    /// The stationary-equivalent z extent of a barrel with home `home` as the
    /// feasibility DP sees it for a runner at speed `v` (max plausible):
    /// the runner meets it after it has rolled for the time the runner
    /// needs to close the trigger distance, and in the barrel's frame the
    /// closing speed is `v - vz`. Includes a 1 m uncertainty margin.
    pub fn barrel_extent(&self, home: f64, dir: i8, v: f32) -> (f64, f64) {
        let h = &self.geom.h;
        let speed = if dir < 0 { h.barrel_speed_toward } else { h.barrel_speed_away };
        let vb = dir as f32 * speed;
        let u = (v - vb).max(1.0);
        let c = home + (vb * h.barrel_trigger_m / u) as f64;
        let r = self.feas.r as f32;
        let eff = ((h.barrel_radius + r) * v.max(0.5) / u - r).max(0.05);
        let m = 1.0f32;
        (c - (eff + m) as f64, c + (eff + m) as f64)
    }

    /// DP demand of an object for a runner at speed `v`.
    fn req_at(&self, o: &Obj, v: f32) -> CellReq {
        if o.kind == ObjKind::Barrel {
            let home = self.row_z(o.row);
            let (z0, z1) = self.barrel_extent(home, o.dir, v);
            CellReq::Jump { z0, z1 }
        } else {
            o.cell_req()
        }
    }
    pub fn feas_params(&self) -> &FeasParams {
        &self.feas
    }
    pub fn row_spacing(&self) -> f32 {
        self.geom.row_spacing
    }
    pub fn row_z(&self, index: u32) -> f64 {
        index as f64 * self.geom.row_spacing as f64
    }
    /// Row whose centre is nearest at or below `z` (clamped at 0).
    pub fn row_at(&self, z: f64) -> u32 {
        let r = (z / self.geom.row_spacing as f64).floor();
        if r.is_finite() && r > 0.0 {
            r.min(u32::MAX as f64) as u32
        } else {
            0
        }
    }
    pub fn lane_x(&self, lane: u8) -> f32 {
        (lane.min(2) as f32 - 1.0) * self.geom.lane_width
    }

    // ---------------------------------------------------------------- rows

    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.rows.values()
    }
    pub fn row(&self, index: u32) -> Option<&Row> {
        self.rows.get(&index)
    }
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    pub fn first_index(&self) -> Option<u32> {
        self.rows.keys().next().copied()
    }
    pub fn last_index(&self) -> Option<u32> {
        self.rows.keys().next_back().copied()
    }
    /// Rows whose centre lies in `[z0, z1]`.
    pub fn rows_in(&self, z0: f64, z1: f64) -> impl Iterator<Item = &Row> {
        let a = self.row_at(z0.max(0.0));
        let b = self.row_at(z1.max(0.0)).saturating_add(1);
        self.rows.range(a..=b)
            .map(|(_, r)| r)
            .filter(move |r| {
                let z = r.row_index as f64 * self.geom.row_spacing as f64;
                z >= z0 && z <= z1
            })
    }

    /// Client: store a row received from the host (replaces any existing one).
    pub fn insert_row(&mut self, index: u32, mut row: Row) {
        row.row_index = index;
        self.rows.insert(index, row);
        if index >= self.next_gen {
            self.next_gen = index.saturating_add(1);
        }
    }

    /// Drop rows (and placements) with index below `index`.
    pub fn release_before(&mut self, index: u32) {
        while let Some((&k, _)) = self.rows.iter().next() {
            if k >= index {
                break;
            }
            self.rows.remove(&k);
        }
        self.placed.retain(|o| o.row >= index);
    }

    // ---------------------------------------------------------------- objects

    fn push_row_objs(&self, row: &Row, out: &mut Vec<Obj>) {
        let g = &self.geom;
        let h = &g.h;
        let rz = row.row_index as f64 * g.row_spacing as f64;
        for lane in 0..3u8 {
            let c = row.cells[lane as usize];
            let lx = self.lane_x(lane);
            let base = |kind, slot, x, z, y, top, hw, hd| Obj {
                id: ObjId::generated(row.row_index, lane, slot),
                kind,
                lane,
                row: row.row_index,
                x,
                z,
                y,
                top,
                half_w: hw,
                half_d: hd,
                by: None,
                solid_at: 0,
                dir: 0,
                vz: 0.0,
                roll: 0.0,
                released: false,
            };
            match c.solid() {
                Some(SolidKind::Hay) => out.push(base(ObjKind::Hay, 0, lx, rz, 0.0, h.hay_top, h.hay_width * 0.5, h.hay_depth * 0.5)),
                Some(SolidKind::Beam) => out.push(base(
                    ObjKind::Beam { span: BeamSpan::One },
                    0,
                    lx,
                    rz,
                    h.beam_underside,
                    h.beam_top(),
                    h.beam_width * 0.5,
                    h.beam_depth * 0.5,
                )),
                Some(SolidKind::BeamAll) if lane == 1 => out.push(base(
                    ObjKind::Beam { span: BeamSpan::All },
                    0,
                    0.0,
                    rz,
                    h.beam_underside,
                    h.beam_top(),
                    g.fence_x,
                    h.beam_depth * 0.5,
                )),
                Some(SolidKind::Block) => out.push(base(ObjKind::Block, 0, lx, rz, 0.0, h.block_height, h.block_width * 0.5, h.block_depth * 0.5)),
                Some(b @ (SolidKind::BarrelToward | SolidKind::BarrelAway)) => {
                    let dir: i8 = if b == SolidKind::BarrelToward { -1 } else { 1 };
                    let (z, vz, roll, released) = self.barrel_state(row, dir);
                    let mut o = base(ObjKind::Barrel, 0, lx, z, 0.0, h.barrel_radius * 2.0, h.barrel_width * 0.5, h.barrel_radius);
                    o.dir = dir;
                    o.vz = vz;
                    o.roll = roll;
                    o.released = released;
                    out.push(o);
                }
                _ => {}
            }
            if c.mud() {
                out.push(base(ObjKind::Mud, 1, lx, rz, 0.0, 0.05, h.mud_width * 0.5, h.mud_depth * 0.5));
            }
            let coin = |k: u8, dz: f32, y: f32| {
                base(ObjKind::Coin, 2 + k, lx, rz + dz as f64, y, y + h.coin_radius, h.coin_radius, h.coin_radius)
            };
            match c.coins() {
                CoinPattern::Line => {
                    for (k, dz) in LINE_OFFSETS.iter().enumerate() {
                        if !row.coin_taken(lane, k as u8) {
                            out.push(coin(k as u8, *dz, h.coin_height));
                        }
                    }
                }
                CoinPattern::Arc => {
                    for (k, dz) in ARC_OFFSETS.iter().enumerate() {
                        if !row.coin_taken(lane, k as u8) {
                            out.push(coin(k as u8, *dz, g.arc_heights[k]));
                        }
                    }
                }
                CoinPattern::None => {}
            }
            if let Some(kind) = c.power() {
                if !row.power_taken() {
                    let y = h.coin_height;
                    out.push(base(ObjKind::Power { kind }, 7, lx, rz, y, y + h.coin_radius, h.coin_radius, h.coin_radius));
                }
            }
        }
    }

    /// All live-or-pending objects whose centre row lies in `[z0, z1]`
    /// (generated and placed; collected coins excluded). Allocates.
    pub fn objects_in(&self, z0: f64, z1: f64) -> Vec<Obj> {
        let mut out = Vec::new();
        // Objects extend at most half a block/mud depth from their row centre.
        let h = &self.geom.h;
        let roll = h.barrel_speed_toward.max(h.barrel_speed_away) * h.barrel_travel_s;
        let pad = (h.mud_depth.max(h.block_depth) * 0.5).max(roll) as f64 + 3.0;
        for row in self.rows_in(z0 - pad, z1 + pad) {
            self.push_row_objs(row, &mut out);
        }
        out.extend(self.placed.iter().copied());
        out.retain(|o| o.z1() >= z0 && o.z0() <= z1);
        out
    }

    /// Every object currently held (allocates).
    pub fn objects(&self) -> Vec<Obj> {
        let mut out = Vec::new();
        for row in self.rows.values() {
            self.push_row_objs(row, &mut out);
        }
        out.extend(self.placed.iter().copied());
        out
    }

    /// Objects of one row (generated + placed).
    pub fn row_objects(&self, index: u32) -> Vec<Obj> {
        let mut out = Vec::new();
        if let Some(r) = self.rows.get(&index) {
            self.push_row_objs(r, &mut out);
        }
        out.extend(self.placed.iter().filter(|o| o.row == index).copied());
        out
    }

    pub fn obj(&self, id: ObjId) -> Option<Obj> {
        if id.is_architect() {
            return self.placed.iter().find(|o| o.id == id).copied();
        }
        let (row, _, _) = id.decode()?;
        self.row_objects(row).into_iter().find(|o| o.id == id)
    }

    /// Ghost placements still held (including pending ones).
    pub fn placed(&self) -> &[Obj] {
        &self.placed
    }

    /// Remove a collected coin. Returns false if it was not present.
    pub fn remove_coin(&mut self, id: ObjId) -> bool {
        if id.is_architect() {
            let n = self.placed.len();
            self.placed.retain(|o| !(o.id == id && o.kind == ObjKind::Coin));
            return self.placed.len() != n;
        }
        let Some((row, lane, slot)) = id.decode() else { return false };
        if !(2..=6).contains(&slot) || lane > 2 {
            return false;
        }
        let Some(r) = self.rows.get_mut(&row) else { return false };
        let k = slot - 2;
        let pat = r.cells[lane as usize].coins();
        let n = match pat {
            CoinPattern::Line => 4,
            CoinPattern::Arc => 5,
            CoinPattern::None => 0,
        };
        if k >= n || r.coin_taken(lane, k) {
            return false;
        }
        r.coins_taken |= 1 << (lane as u16 * 5 + k as u16);
        true
    }

    /// Collect a power-up (host on pickup, client on `Event::PowerTaken`).
    /// Returns its kind, or `None` if it was not present (idempotent).
    pub fn take_power(&mut self, id: ObjId) -> Option<PowerKind> {
        let (row, lane, slot) = id.decode()?;
        if slot != 7 || lane > 2 {
            return None;
        }
        let r = self.rows.get_mut(&row)?;
        let kind = r.cells[lane as usize].power()?;
        if r.power_taken() {
            return None;
        }
        r.coins_taken |= POWER_TAKEN;
        Some(kind)
    }

    /// Build the object(s) a ghost tool creates. Coins: five ids `obj..obj+4`.
    pub fn placement_objs(&self, by: PlayerId, obj: ObjId, tool: Tool, lane: u8, row: u32, solid_at: Tick) -> Vec<Obj> {
        let g = &self.geom;
        let h = &g.h;
        let lane = lane.min(2);
        let x = self.lane_x(lane);
        let z = self.row_z(row);
        let mk = |id: u32, kind, z: f64, y, top, hw, hd| Obj {
            id: ObjId(id),
            kind,
            lane,
            row,
            x,
            z,
            y,
            top,
            half_w: hw,
            half_d: hd,
            by: Some(by),
            solid_at,
            dir: 0,
            vz: 0.0,
            roll: 0.0,
            released: false,
        };
        match tool {
            Tool::Hay => vec![mk(obj.0, ObjKind::Hay, z, 0.0, h.hay_top, h.hay_width * 0.5, h.hay_depth * 0.5)],
            Tool::Beam => vec![mk(
                obj.0,
                ObjKind::Beam { span: BeamSpan::One },
                z,
                h.beam_underside,
                h.beam_top(),
                h.beam_width * 0.5,
                h.beam_depth * 0.5,
            )],
            Tool::Mud => vec![mk(obj.0, ObjKind::Mud, z, 0.0, 0.05, h.mud_width * 0.5, h.mud_depth * 0.5)],
            Tool::Coins => (0..5u32)
                .map(|k| {
                    let dz = (k as f32 - 2.0) * h.lure_spacing;
                    mk(
                        obj.0.wrapping_add(k),
                        ObjKind::Coin,
                        z + dz as f64,
                        h.coin_height,
                        h.coin_height + h.coin_radius,
                        h.coin_radius,
                        h.coin_radius,
                    )
                })
                .collect(),
        }
    }

    /// Store a placement (host after acceptance, or client from a `Placed` event).
    pub fn add_placement(&mut self, by: PlayerId, obj: ObjId, tool: Tool, lane: u8, row: u32, solid_at: Tick) {
        if self.placed.iter().any(|o| o.id == obj) {
            return; // idempotent
        }
        let objs = self.placement_objs(by, obj, tool, lane, row, solid_at);
        self.placed.extend(objs);
        let n = if tool == Tool::Coins { 5 } else { 1 };
        let end = obj.0.saturating_add(n);
        if end > self.next_arch {
            self.next_arch = end;
        }
    }

    /// Next architect id (host).
    pub fn next_architect_id(&self) -> ObjId {
        ObjId(self.next_arch)
    }

    // ---------------------------------------------------------------- feasibility

    /// DP demand of one row (generated + placed incl. pending + `extra`).
    pub fn dp_row(&self, index: u32, v: f32, extra: &[Obj]) -> DpRow {
        let mut lanes = [CellReq::Free; 3];
        let objs = self.row_objects(index);
        for o in objs.iter().chain(extra.iter().filter(|o| o.row == index)) {
            if !o.kind.is_solid() {
                continue;
            }
            let (a, b) = o.lanes();
            let req = self.req_at(o, v);
            for l in a..=b.min(2) {
                lanes[l as usize] = lanes[l as usize].merge(req);
            }
        }
        self.finish_dp_row(index, lanes, v, None, extra)
    }

    fn finish_dp_row(&self, index: u32, lanes: [CellReq; 3], v: f32, cand: Option<&Row>, extra: &[Obj]) -> DpRow {
        let v_min = (v as f64 - self.feas.v_spread).max(1.0).min(v as f64) as f32;
        let mud = [0u8, 1, 2].map(|l| self.mud_spans(index, l, cand, extra));
        DpRow { z: self.row_z(index), lanes, v, v_min, mud }
    }

    /// Mud spans (z where a body counts as standing in mud, `runner::mud_at`)
    /// in `lane` around row `index`: generated mud of rows `index-4 ..= index+1`
    /// (`cand` stands in for row `index` while generating), placed mud
    /// (pending included) and `extra`.
    fn mud_spans(&self, index: u32, lane: u8, cand: Option<&Row>, extra: &[Obj]) -> MudSpans {
        let h = &self.geom.h;
        let half = (h.mud_depth * 0.5) as f64 + self.feas.r * 0.5;
        let mut raw: Vec<(f64, f64)> = Vec::new();
        for i in index.saturating_sub(4)..=index.saturating_add(1) {
            let cell = match cand {
                Some(c) if i == index => Some(c.cells[lane as usize]),
                Some(_) if i > index => None,
                _ => self.rows.get(&i).map(|r| r.cells[lane as usize]),
            };
            if cell.is_some_and(|c| c.mud()) {
                let z = self.row_z(i);
                raw.push((z - half, z + half));
            }
        }
        let lo = index.saturating_sub(4);
        let hi = index.saturating_add(1);
        for o in self.placed.iter().chain(extra.iter()) {
            if o.kind == ObjKind::Mud && o.covers_lane(lane) && o.row >= lo && o.row <= hi {
                let hd = o.half_d as f64 + self.feas.r * 0.5;
                raw.push((o.z - hd, o.z + hd));
            }
        }
        feas::pack_mud(raw)
    }

    /// Run the DP over all held rows with index ≥ `from_row`, starting with
    /// every lane actionable at `start_z`. `extra` objects are treated as
    /// already placed. Returns the final frontier or `None` if infeasible.
    pub fn feasible_from(&self, from_row: u32, start_z: f64, speed: &dyn Fn(f64) -> f32, extra: &[Obj]) -> Option<Frontier> {
        let p = self.feas;
        let mut f: Frontier = [Some(start_z); 3];
        let mut prev: Option<DpRow> = None;
        if from_row > 0 && self.rows.contains_key(&(from_row - 1)) {
            let z = self.row_z(from_row - 1);
            prev = Some(self.dp_row(from_row - 1, speed(z), extra));
        }
        for (&idx, _) in self.rows.range(from_row..) {
            let row = self.dp_row(idx, speed(self.row_z(idx)), extra);
            f = feas::step(&p, prev.as_ref(), &row, &f);
            if !feas::alive(&f) {
                return None;
            }
            prev = Some(row);
        }
        Some(f)
    }

    /// Recompute the generator frontier over the whole window (after a placement).
    pub fn recompute_frontier(&mut self, speed: &dyn Fn(f64) -> f32) {
        if let Some(f) = self.feasible_window(speed, &[]) {
            self.frontier = f;
        }
    }

    pub fn frontier(&self) -> Frontier {
        self.frontier
    }

    /// Whole-window feasibility for an unobstructed runner (DP from the
    /// first held row with every lane free). Used by placement validation,
    /// tests and debug overlays.
    pub fn is_feasible(&self, speed: &dyn Fn(f64) -> f32) -> bool {
        self.feasible_window(speed, &[]).is_some()
    }

    /// As `is_feasible`, with `extra` objects treated as placed; returns the frontier.
    pub fn feasible_window(&self, speed: &dyn Fn(f64) -> f32, extra: &[Obj]) -> Option<Frontier> {
        let Some(first) = self.first_index() else { return Some([Some(-1.0e9); 3]) };
        self.feasible_from(first, -1.0e9, speed, extra)
    }

    // ---------------------------------------------------------------- generation

    /// Generate rows until the last row's centre is at least `z_target`.
    /// No-op on a remote course.
    pub fn generate_until(&mut self, z_target: f64, ctx: &GenCtx, speed: &dyn Fn(f64) -> f32) {
        if self.remote {
            return;
        }
        let mut guard = 0;
        while self.row_z(self.next_gen) <= z_target && guard < 10_000 {
            let idx = self.next_gen;
            self.generate_row(idx, ctx, speed);
            self.next_gen = idx.saturating_add(1);
            guard += 1;
        }
    }

    pub fn next_gen_index(&self) -> u32 {
        self.next_gen
    }

    fn generate_row(&mut self, idx: u32, ctx: &GenCtx, speed: &dyn Fn(f64) -> f32) {
        let z = self.row_z(idx);
        let mut rng = row_rng(ctx.seed, idx);
        let mut candidates = if self.geom.empty || z < self.geom.run_in as f64 {
            vec![Row::empty(idx)]
        } else {
            let d = self.difficulty_at(idx, ctx);
            self.roll_candidates(&mut rng, idx, ctx, d)
        };
        // No mud right behind a barrel in its lane: a rolling-away barrel is
        // met past its row, where that mud would sit under the take-off
        // (the DP only looks back from a jump, so it would not see it).
        if let Some(prev) = idx.checked_sub(1).and_then(|i| self.rows.get(&i)).copied() {
            for cand in candidates.iter_mut() {
                for l in 0..3 {
                    if cand.cells[l].mud() && matches!(prev.cells[l].solid(), Some(SolidKind::BarrelToward | SolidKind::BarrelAway)) {
                        cand.cells[l] = cand.cells[l].with_mud(false);
                    }
                }
            }
        }
        self.accept_row(idx, candidates, ctx, speed);
    }

    /// Generator difficulty of row `idx` (0..1).
    fn difficulty_at(&self, idx: u32, ctx: &GenCtx) -> f32 {
        if ctx.difficulty_ramp_m > 0.0 {
            let d = (self.row_z(idx) - self.geom.run_in as f64).max(ctx.difficulty_floor_m.max(0.0) as f64) as f32;
            1.0 - det::exp_f32(-d / ctx.difficulty_ramp_m)
        } else {
            ctx.difficulty
        }
    }

    /// Take the first candidate (fullest first) the DP accepts, add the
    /// row's power-up and store it.
    fn accept_row(&mut self, idx: u32, candidates: Vec<Row>, ctx: &GenCtx, speed: &dyn Fn(f64) -> f32) {
        let z = self.row_z(idx);
        let prev = if idx > 0 && self.rows.contains_key(&(idx - 1)) {
            Some(self.dp_row(idx - 1, speed(self.row_z(idx - 1)), &[]))
        } else {
            None
        };
        let p = self.feas;
        let v = speed(z);
        let n = candidates.len();
        for (k, cand) in candidates.into_iter().enumerate() {
            let row_dp = self.dp_of_cells(&cand, v);
            let f = feas::step(&p, prev.as_ref(), &row_dp, &self.frontier);
            // Keep at least two live lanes so routes never funnel into one
            // lane far ahead (the emptiest candidate is always acceptable).
            let live = f.iter().filter(|x| x.is_some()).count();
            if live >= 2 || (k + 1 == n && live >= 1) {
                if cand.has_solid() {
                    self.gen.last_solid_row = Some(idx);
                }
                self.frontier = f;
                let mut row = cand;
                self.place_power(&mut row, ctx);
                self.rows.insert(idx, row);
                return;
            }
        }
        // Reached when even the emptiest candidate kills the frontier (an
        // empty row is always feasible from a live frontier, so in practice
        // only for a candidate whose fallbacks keep mud); keep the course valid.
        let row = Row::empty(idx);
        let row_dp = self.dp_of_cells(&row, v);
        self.frontier = feas::step(&p, prev.as_ref(), &row_dp, &self.frontier);
        if !feas::alive(&self.frontier) {
            self.frontier = [Some(z); 3];
        }
        let mut row = row;
        self.place_power(&mut row, ctx);
        self.rows.insert(idx, row);
    }

    /// Power-ups: one every `powerup_gap_m` (± jitter) metres after the
    /// run-in, in a random lane of the due row without a solid or mud (else
    /// the next row that has one). Draws come from the row's own power
    /// stream, so this is as pure as the rows themselves.
    fn place_power(&mut self, row: &mut Row, ctx: &GenCtx) {
        let g = &self.geom;
        if ctx.powerup_gap_m <= 0.0 || g.empty || self.row_z(row.row_index) < g.run_in as f64 {
            return;
        }
        let idx = row.row_index;
        let mut prng = power_rng(ctx.seed, idx);
        let spacing = g.row_spacing;
        let jitter = ctx.powerup_jitter.clamp(0.0, 0.95);
        let gap_rows = |r: &mut SplitMix64| -> u32 {
            let f = 1.0 + jitter * (2.0 * r.unit() - 1.0);
            ((ctx.powerup_gap_m * f / spacing).round() as u32).max(1)
        };
        let due = match self.gen.next_power_row {
            Some(d) => d,
            None => {
                // First one: a gap after the end of the opening clear run.
                let run_in_row = (g.run_in / spacing).ceil() as u32;
                let d = run_in_row.max(idx) + gap_rows(&mut prng);
                self.gen.next_power_row = Some(d);
                d
            }
        };
        if idx < due {
            return;
        }
        let lanes: Vec<u8> = (0..3u8).filter(|&l| row.cells[l as usize].solid().is_none() && !row.cells[l as usize].mud()).collect();
        if lanes.is_empty() {
            return; // try the next row
        }
        let lane = lanes[prng.below(lanes.len() as u32) as usize];
        let total: u32 = ctx.powerup_weights.iter().sum();
        if total == 0 {
            return;
        }
        let mut pick = prng.below(total);
        let mut kind = PowerKind::Magnet;
        for (k, w) in PowerKind::ALL.iter().zip(ctx.powerup_weights) {
            if pick < w {
                kind = *k;
                break;
            }
            pick -= w;
        }
        row.cells[lane as usize] = row.cells[lane as usize].with_power(Some(kind));
        self.gen.next_power_row = Some(idx + gap_rows(&mut prng));
    }

    fn dp_of_cells(&self, row: &Row, v: f32) -> DpRow {
        let mut objs = Vec::new();
        self.push_row_objs(row, &mut objs);
        let mut lanes = [CellReq::Free; 3];
        for o in objs.iter().filter(|o| o.kind.is_solid()) {
            let (a, b) = o.lanes();
            let req = self.req_at(o, v);
            for l in a..=b {
                lanes[l as usize] = lanes[l as usize].merge(req);
            }
        }
        self.finish_dp_row(row.row_index, lanes, v, Some(row), &[])
    }

    /// Candidate rows from fullest to emptiest.
    fn roll_candidates(&mut self, rng: &mut SplitMix64, idx: u32, ctx: &GenCtx, difficulty: f32) -> Vec<Row> {
        let d = difficulty.clamp(0.0, 1.0);
        let dens = ctx.density.clamp(0.0, 1.5);
        let mut gap = if d < 0.25 {
            3
        } else if d < 0.6 {
            2
        } else {
            1
        };
        // Multiplayer opening: the pack is still bunched, so start with one lane at a time and
        // long gaps, then phase the full pool in. `q` is 0 while gentle, 1 once fully ramped.
        let z = self.row_z(idx);
        let q = if ctx.gentle_until <= 0.0 {
            1.0
        } else {
            ((z - ctx.gentle_until) / ctx.ramp_m.max(1.0)).clamp(0.0, 1.0) as f32
        };
        if q < 1.0 {
            gap = gap.max(if q <= 0.0 { 5 } else if q < 0.5 { 3 } else { 2 });
        }
        let since = self.gen.last_solid_row.map_or(u32::MAX, |l| idx.saturating_sub(l));
        let p_obst = (0.45 + 0.45 * d) * dens;
        let mut row = Row::empty(idx);
        let lane = |rng: &mut SplitMix64| rng.range_u8(0, 3);
        let mut obstacle = false;
        if since >= gap && rng.unit() < p_obst {
            obstacle = true;
            // (pattern, min difficulty, weight)
            let pool: [(u8, f32, f32); 10] = [
                (0, 0.0, 3.0),  // single hay
                (1, 0.0, 2.0),  // single beam
                (2, 0.15, 1.2), // beam across all three
                (3, 0.2, 1.6),  // hay in two lanes
                (4, 0.35, 1.4), // block + hay
                (5, 0.1, 0.8),  // mud lane + hay elsewhere
                (6, 0.55, 1.0), // two blocks
                (7, 0.45, 1.0), // hay + beam
                (8, 0.35, 1.4), // barrel rolling toward the runners
                (9, 0.35, 1.0), // barrel rolling away
            ];
            // Weight of a pattern: the two single-lane ones always, the rest phased in by `q`.
            let w = |p: &(u8, f32, f32)| if p.0 <= 1 { p.2 } else if p.0 >= 8 && since < 2 { 0.0 } else { p.2 * q };
            let total: f32 = pool.iter().filter(|p| d >= p.1 && w(p) > 0.0).map(w).sum();
            let mut pick = rng.unit() * total;
            let mut pat = 0;
            for p in pool.iter().filter(|p| d >= p.1 && w(p) > 0.0) {
                let pw = w(p);
                if pick < pw {
                    pat = p.0;
                    break;
                }
                pick -= pw;
            }
            let a = lane(rng);
            let b = (a + 1 + rng.range_u8(0, 2)) % 3;
            let c = 3 - a - b;
            let set = |row: &mut Row, l: u8, s: SolidKind| row.cells[l as usize] = row.cells[l as usize].with_solid(Some(s));
            match pat {
                0 => set(&mut row, a, SolidKind::Hay),
                1 => set(&mut row, a, SolidKind::Beam),
                2 => {
                    for l in 0..3 {
                        set(&mut row, l, SolidKind::BeamAll);
                    }
                }
                3 => {
                    set(&mut row, a, SolidKind::Hay);
                    set(&mut row, b, SolidKind::Hay);
                }
                4 => {
                    set(&mut row, a, SolidKind::Block);
                    set(&mut row, b, SolidKind::Hay);
                }
                5 => {
                    row.cells[a as usize] = row.cells[a as usize].with_mud(true);
                    set(&mut row, b, SolidKind::Hay);
                }
                6 => {
                    set(&mut row, a, SolidKind::Block);
                    set(&mut row, b, SolidKind::Block);
                }
                7 => {
                    set(&mut row, a, SolidKind::Hay);
                    set(&mut row, b, SolidKind::Beam);
                }
                8 => set(&mut row, a, SolidKind::BarrelToward),
                _ => set(&mut row, a, SolidKind::BarrelAway),
            }
            // Coin arc over one hay, sometimes.
            if rng.unit() < 0.35 {
                for l in [a, b, c] {
                    if row.cells[l as usize].solid() == Some(SolidKind::Hay) {
                        row.cells[l as usize] = row.cells[l as usize].with_coins(CoinPattern::Arc);
                        break;
                    }
                }
            }
        }
        // Coin trails (zig-zag) on rows without a solid in the trail lane.
        if self.gen.trail_left == 0 && rng.unit() < 0.22 {
            self.gen.trail_left = rng.range_u8(2, 6);
            self.gen.trail_lane = lane(rng);
        }
        if self.gen.trail_left > 0 {
            self.gen.trail_left -= 1;
            if rng.unit() < 0.45 {
                let l = self.gen.trail_lane as i8 + if rng.flip() { 1 } else { -1 };
                self.gen.trail_lane = l.clamp(0, 2) as u8;
            }
            let l = self.gen.trail_lane as usize;
            if row.cells[l].solid().is_none() && row.cells[l].coins() == CoinPattern::None {
                row.cells[l] = row.cells[l].with_coins(CoinPattern::Line);
            }
        }
        // Occasional stray mud.
        if !obstacle && d > 0.1 && rng.unit() < 0.06 + 0.08 * d {
            let l = lane(rng) as usize;
            row.cells[l] = row.cells[l].with_mud(true);
        }
        let mut out = vec![row];
        if row.has_solid() {
            // Emptier fallback 1: keep only one solid (the first hay/beam).
            let mut one = row;
            let mut kept = false;
            for l in 0..3 {
                let s = one.cells[l].solid();
                let keep = !kept && matches!(s, Some(SolidKind::Hay | SolidKind::Beam | SolidKind::BarrelToward | SolidKind::BarrelAway));
                if s.is_some() && !keep {
                    one.cells[l] = one.cells[l].with_solid(None);
                    if one.cells[l].coins() == CoinPattern::Arc {
                        one.cells[l] = one.cells[l].with_coins(CoinPattern::None);
                    }
                }
                kept |= keep;
            }
            if one.has_solid() {
                out.push(one);
            }
            // Fallback 2: no solids.
            let mut none = row;
            for l in 0..3 {
                none.cells[l] = none.cells[l].with_solid(None);
                if none.cells[l].coins() == CoinPattern::Arc {
                    none.cells[l] = none.cells[l].with_coins(CoinPattern::Line);
                }
            }
            out.push(none);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_roundtrip() {
        let c = Cell::EMPTY.with_solid(Some(SolidKind::Block)).with_mud(true).with_coins(CoinPattern::Arc);
        assert_eq!(c.solid(), Some(SolidKind::Block));
        assert!(c.mud());
        assert_eq!(c.coins(), CoinPattern::Arc);
        assert_eq!(c.with_solid(None).solid(), None);
    }

    #[test]
    fn objid_roundtrip() {
        let id = ObjId::generated(1234, 2, 5);
        assert_eq!(id.decode(), Some((1234, 2, 5)));
        assert!(!id.is_architect());
    }

    #[test]
    fn generation_is_feasible_and_deterministic() {
        let t = Tuning::default();
        let speed = |z: f64| t.solo_speed(z) + t.plausible_margin();
        let mk = || {
            let mut c = Course::new(&t);
            c.generate_until(3000.0, &GenCtx::fixed(7, 0.9), &speed);
            c
        };
        let a = mk();
        let b = mk();
        assert_eq!(a.rows().collect::<Vec<_>>(), b.rows().collect::<Vec<_>>());
        assert!(a.feasible_from(0, -10.0, &speed, &[]).is_some());
        assert!(a.rows().filter(|r| r.has_solid()).count() > 100);
    }

    #[test]
    fn gentle_opening_has_single_lane_hay_and_beams_only() {
        let t = Tuning::default();
        let speed = |z: f64| t.solo_speed(z) + t.plausible_margin();
        for seed in 0..20 {
            let mut c = Course::new(&t);
            let ctx = GenCtx { gentle_until: 300.0, ramp_m: 200.0, ..GenCtx::fixed(seed, 0.6) };
            c.generate_until(500.0, &ctx, &speed);
            let mut last_solid: Option<u32> = None;
            for r in c.rows().filter(|r| c.row_z(r.row_index) < 300.0) {
                let solids: Vec<_> = (0..3).filter_map(|l| r.cells[l].solid()).collect();
                assert!(solids.len() <= 1, "seed {seed} row {}: {solids:?}", r.row_index);
                assert!(solids.iter().all(|s| matches!(s, SolidKind::Hay | SolidKind::Beam)));
                if !solids.is_empty() {
                    if let Some(l) = last_solid {
                        assert!(r.row_index - l >= 5, "seed {seed}: obstacles {l} and {} too close", r.row_index);
                    }
                    last_solid = Some(r.row_index);
                }
            }
        }
    }

    /// A course of empty rows `0..first`, then `mud_rows` with mud in every
    /// lane, the frontier at the end; `next_gen` = the row after.
    fn with_mud(mud_rows: &[u32], first: u32) -> (Course, impl Fn(f64) -> f32) {
        let t = Tuning::default();
        let speed = move |z: f64| t.solo_speed(z) + t.plausible_margin();
        let mut c = Course::new(&Tuning::default());
        for i in 0..first {
            let mut r = Row::empty(i);
            if mud_rows.contains(&i) {
                for l in 0..3 {
                    r.cells[l] = r.cells[l].with_mud(true);
                }
            }
            c.rows.insert(i, r);
        }
        c.frontier = c.feasible_from(0, -1.0e9, &speed, &[]).unwrap();
        c.next_gen = first;
        (c, speed)
    }

    #[test]
    fn generator_rejects_a_jump_only_row_right_after_mud() {
        // (solid, muddy rows that leave no mud-free take-off, muddy rows that do).
        // A toward barrel is met ~4 m before its row, so its run-up starts earlier.
        for (solid, bad, good) in [(SolidKind::Hay, &[19u32][..], &[18u32][..]), (SolidKind::BarrelToward, &[18, 19], &[17])] {
            let wall = |i: u32| {
                let mut r = Row::empty(i);
                for l in 0..3 {
                    r.cells[l] = r.cells[l].with_solid(Some(solid));
                }
                r
            };
            // Mud right before: every take-off would be in mud.
            let (mut c, speed) = with_mud(bad, 20);
            c.accept_row(20, vec![wall(20), Row::empty(20)], &GenCtx::fixed(1, 0.5), &speed);
            assert!(!c.rows[&20].has_solid(), "{solid:?} wall right after mud must be rejected");
            assert!(c.feasible_from(0, -1.0e9, &speed, &[]).is_some());
            // Mud further back leaves room to take off after it.
            let (mut c, speed) = with_mud(good, 20);
            c.accept_row(20, vec![wall(20), Row::empty(20)], &GenCtx::fixed(1, 0.5), &speed);
            assert!(c.rows[&20].has_solid(), "{solid:?} wall with a mud-free run-up is fine");
        }
    }

    #[test]
    fn generated_courses_never_need_a_jump_from_mud() {
        // Whole generated courses stay feasible under the mud rule, and the
        // DP really sees mud: dropping a hay wall right after a muddy row
        // makes it infeasible.
        let t = Tuning::default();
        let speed = |z: f64| t.solo_speed(z) + t.plausible_margin();
        for seed in 0..8 {
            let mut c = Course::new(&t);
            c.generate_until(2500.0, &GenCtx::fixed(seed, 0.8), &speed);
            assert!(c.feasible_from(0, -10.0, &speed, &[]).is_some(), "seed {seed}");
            assert!(c.rows().any(|r| r.cells.iter().any(|x| x.mud())), "seed {seed} has mud");
        }
    }

    #[test]
    fn coin_removal() {
        let t = Tuning::default();
        let mut c = Course::new_remote(&t);
        let mut r = Row::empty(10);
        r.cells[1] = r.cells[1].with_coins(CoinPattern::Line);
        c.insert_row(10, r);
        let coins: Vec<_> = c.row_objects(10).into_iter().filter(|o| o.kind == ObjKind::Coin).collect();
        assert_eq!(coins.len(), 4);
        assert!(c.remove_coin(coins[0].id));
        assert!(!c.remove_coin(coins[0].id));
        assert_eq!(c.row_objects(10).len(), 3);
        assert!(!c.remove_coin(ObjId(u32::MAX)));
    }
}
