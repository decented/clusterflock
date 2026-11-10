//! Course feasibility (SPEC §4): a DP over rows shared by the generator,
//! ghost placement validation and the runner bot.
//!
//! State per lane = the earliest course `z` at which a runner who passed the
//! current row in that lane is actionable again (`None` = unreachable).
//! Timing uses the *maximum plausible* speed of each row (`v_course +
//! jump_fwd_boost`), passed in by the caller on each [`DpRow`].
//!
//! Model (conservative, documented in the README):
//! * a lateral move from lane `a` to `b` takes `|a-b| * lane_change_s` and
//!   starts once the runner is actionable, has passed the previous row's
//!   centre and cleared its solids in every lane it crosses; it must finish
//!   before this row's solids in every lane it crosses (including `b`) and
//!   before the next row's earliest possible solid (`row_clear`), so a move
//!   never silently crosses a later row;
//! * `Hay` needs a jump whose clear-of-the-top window covers the hay's depth
//!   plus the body radius; `Beam` needs a collapse whose low window covers it;
//!   `Block` is never passable; mud and coins never block;
//! * an action may start during the lateral move (diagonal), but not before it;
//! * a jump cannot be started (command or wind-up) while the body is in mud
//!   ([`DpRow::mud`]): the jump command must fall outside every mud span at
//!   every speed in `[v_min, v]` (see [`jump_command`]).

use serde::{Deserialize, Serialize};

use crate::{tuning::Tuning, DT};

/// What a lane at a row demands. Extents are the object's raw z extent.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum CellReq {
    Free,
    Jump { z0: f64, z1: f64 },
    Duck { z0: f64, z1: f64 },
    Blocked { z0: f64, z1: f64 },
}

impl CellReq {
    pub fn extent(&self) -> Option<(f64, f64)> {
        match *self {
            CellReq::Free => None,
            CellReq::Jump { z0, z1 } | CellReq::Duck { z0, z1 } | CellReq::Blocked { z0, z1 } => Some((z0, z1)),
        }
    }
    /// Combine two demands in one cell (e.g. a pending placement on top of a
    /// generated solid). Incompatible demands block.
    pub fn merge(self, other: CellReq) -> CellReq {
        use CellReq::*;
        match (self, other) {
            (Free, o) | (o, Free) => o,
            (Jump { z0: a0, z1: a1 }, Jump { z0: b0, z1: b1 }) => Jump { z0: a0.min(b0), z1: a1.max(b1) },
            (Duck { z0: a0, z1: a1 }, Duck { z0: b0, z1: b1 }) => Duck { z0: a0.min(b0), z1: a1.max(b1) },
            (a, b) => {
                let (a0, a1) = a.extent().unwrap_or((0.0, 0.0));
                let (b0, b1) = b.extent().unwrap_or((0.0, 0.0));
                Blocked { z0: a0.min(b0), z1: a1.max(b1) }
            }
        }
    }
}

/// Most mud spans the DP tracks per lane and row (more are merged, which is
/// conservative).
pub const MUD_SPANS: usize = 4;

/// Body-in-mud z spans `[m0, m1]` of one lane near a row, ascending and
/// disjoint (`None` = unused slot).
pub type MudSpans = [Option<(f64, f64)>; MUD_SPANS];

/// One row as the DP sees it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct DpRow {
    pub z: f64,
    pub lanes: [CellReq; 3],
    /// Max plausible forward speed at this row (m/s).
    pub v: f32,
    /// Slowest speed a runner plausibly has here (m/s); the mud rule must hold
    /// for every speed in `[v_min, v]`.
    pub v_min: f32,
    /// Per lane: mud spans near this row where a jump cannot be started
    /// (the z range in which a body counts as standing in that mud).
    pub mud: [MudSpans; 3],
}

impl DpRow {
    /// A row without mud, `v_min = v`.
    pub fn plain(z: f64, lanes: [CellReq; 3], v: f32) -> DpRow {
        DpRow { z, lanes, v, v_min: v, mud: [[None; MUD_SPANS]; 3] }
    }
}

/// Sort and merge raw spans into a [`MudSpans`] (overflow merges into the
/// last slot, which only forbids more).
pub fn pack_mud(mut raw: Vec<(f64, f64)>) -> MudSpans {
    raw.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (a, b) in raw {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    let mut out: MudSpans = [None; MUD_SPANS];
    for (i, sp) in merged.into_iter().enumerate() {
        let k = i.min(MUD_SPANS - 1);
        out[k] = Some(match out[k] {
            Some((a, b)) => (a.min(sp.0), b.max(sp.1)),
            None => sp,
        });
    }
    out
}

/// Earliest actionable `z` per lane after the last processed row.
pub type Frontier = [Option<f64>; 3];

/// Timing constants derived from `Tuning` (discrete-tick accurate).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeasParams {
    pub r: f64,
    pub jump_windup: f64,
    /// Seconds after takeoff when body bottom first / last clears hay top (+margin).
    pub t1: f64,
    pub t2: f64,
    pub t_air: f64,
    pub down_windup: f64,
    pub collapse: f64,
    pub standup: f64,
    pub lane_change: f64,
    /// Speed lost while collapsed, worst case (max plausible boost + coin bonus, minus the
    /// collapse loss: the plausible speed includes both).
    pub collapse_drop: f64,
    /// A lateral move attributed to a row must end within this distance past
    /// the row centre (before the next row's earliest possible solid).
    pub row_clear: f64,
    /// Human reaction slack (s) added to every action and lane-change
    /// duration, so chained manoeuvres never need frame-perfect play.
    pub slack: f64,
    /// `v - v_min` of a row: how much slower than the max plausible speed a
    /// runner may be (the plausible margin: no coin bonus, no jump boost).
    #[serde(default)]
    pub v_spread: f64,
}

impl FeasParams {
    pub fn from_tuning(t: &Tuning) -> Self {
        let dt = DT as f64;
        // Discrete trajectory exactly as the sim integrates it.
        let margin = 0.05f32;
        let top = t.hazards.hay_top + margin;
        let (mut y, mut vy) = (0.0f32, t.jump_vy);
        let (mut t1, mut t2, mut t_air) = (None, 0.0f64, 0.0f64);
        for n in 1..600u32 {
            vy -= t.gravity * DT;
            y += vy * DT;
            let tt = n as f64 * dt;
            if y > top {
                if t1.is_none() {
                    t1 = Some(tt);
                }
                t2 = tt;
            }
            if y <= 0.0 {
                t_air = tt;
                break;
            }
        }
        let t1 = t1.unwrap_or(f64::INFINITY);
        Self {
            r: t.body_radius as f64,
            jump_windup: t.ticks(t.jump_windup) as f64 * dt,
            t1,
            t2: if t1.is_finite() { t2 } else { f64::NEG_INFINITY },
            t_air,
            down_windup: t.ticks(t.down_windup) as f64 * dt,
            collapse: t.ticks(t.collapse_time) as f64 * dt,
            standup: t.ticks(t.standup_time) as f64 * dt,
            lane_change: t.lane_change_s as f64,
            collapse_drop: (t.plausible_margin() + t.collapse_fwd_loss) as f64,
            row_clear: (t.row_spacing
                - 0.5 * t.hazards.hay_depth.max(t.hazards.beam_depth).max(t.hazards.block_depth)
                - t.body_radius) as f64,
            slack: t.feas_reaction_s.max(0.0) as f64,
            v_spread: t.plausible_margin().max(0.0) as f64,
        }
    }

    /// Window of jump *command* positions that clear an obstacle spanning
    /// `[z0, z1]` at speed `v`. Empty when `lo > hi`.
    pub fn jump_window(&self, z0: f64, z1: f64, v: f64) -> (f64, f64) {
        let lo = z1 + self.r - (self.jump_windup + self.t2) * v;
        let hi = z0 - self.r - (self.jump_windup + self.t1) * v;
        (lo, hi)
    }

    /// Distance from a jump command to landing.
    pub fn jump_len(&self, v: f64) -> f64 {
        (self.jump_windup + self.t_air) * v
    }

    /// Window of collapse *command* positions that keep the body low over `[z0, z1]`.
    pub fn duck_window(&self, z0: f64, z1: f64, v: f64) -> (f64, f64) {
        let v_slow = (v - self.collapse_drop).max(2.0);
        let lo = z1 + self.r - self.down_windup * v - self.collapse * v_slow;
        let hi = z0 - self.r - self.down_windup * v;
        (lo, hi)
    }

    /// Distance from a collapse command to being controllable again.
    pub fn duck_len(&self, v: f64) -> f64 {
        (self.down_windup + self.collapse + self.standup) * v
    }
}

/// One transition: passed `prev` in lane `a` (actionable from `free_a`), pass
/// `row` in lane `b`. Returns the new earliest actionable z, or `None`.
pub fn transition(p: &FeasParams, prev: Option<&DpRow>, row: &DpRow, a: u8, b: u8, free_a: f64) -> Option<f64> {
    if a > 2 || b > 2 {
        return None;
    }
    let v = row.v.max(0.5) as f64;
    let (lo_l, hi_l) = (a.min(b) as usize, a.max(b) as usize);
    let sl = p.slack * v;
    let dl = if hi_l > lo_l { (hi_l - lo_l) as f64 * p.lane_change * v + sl } else { 0.0 };
    let mut s = free_a;
    if a != b {
        if let Some(pr) = prev {
            // The runner passed the previous row in lane `a`.
            s = s.max(pr.z);
            for l in lo_l..=hi_l {
                if let Some((_, z1)) = pr.lanes[l].extent() {
                    s = s.max(z1 + p.r);
                }
            }
        }
        let e = s + dl;
        if e > row.z + p.row_clear {
            return None;
        }
        for l in lo_l..=hi_l {
            if let Some((z0, _)) = row.lanes[l].extent() {
                if e > z0 - p.r {
                    return None;
                }
            }
        }
    }
    let e = s + dl;
    match row.lanes[b as usize] {
        CellReq::Free => Some(e),
        CellReq::Blocked { .. } => None,
        CellReq::Jump { z0, z1 } => {
            let zj = jump_command(p, &row.mud[b as usize], z0, z1, v, row.v_min as f64, s)?;
            Some((zj + p.jump_len(v) + sl).max(e))
        }
        CellReq::Duck { z0, z1 } => {
            let (lo, hi) = p.duck_window(z0, z1, v);
            let zc = s.max(lo);
            if zc > hi {
                return None;
            }
            Some((zc + p.duck_len(v) + sl).max(e))
        }
    }
}

/// Earliest jump command `z >= s` at speed `v` that clears `[z0, z1]` with
/// neither the command nor the wind-up in a mud span, or `None`.
///
/// A jump is only counted when some mud-free command exists at **every**
/// speed in `[v_min, v]`: at lower speed the window slides toward the
/// obstacle, so "take off before the mud" is hardest at `v_min` and "take
/// off after it" hardest at `v`. Each gap between mud spans is open for an
/// interval of speeds (linear bounds); the gaps' intervals must cover
/// `[v_min, v]`. (Like the rest of the DP, the window itself is sized at `v`.)
pub fn jump_command(p: &FeasParams, mud: &MudSpans, z0: f64, z1: f64, v: f64, v_min: f64, s: f64) -> Option<f64> {
    let (lo, hi) = p.jump_window(z0, z1, v);
    let spans: Vec<(f64, f64)> = mud.iter().flatten().copied().filter(|&(_, m1)| m1 >= s.min(lo) - 1.0).collect();
    if spans.is_empty() {
        let zj = s.max(lo);
        return (zj <= hi).then_some(zj);
    }
    let w = p.jump_windup.max(1e-6);
    let (t1, t2) = (p.t1, p.t2);
    let v_min = v_min.min(v);
    // Gap k lies after span k-1 (end `prev`) and before span k (start `next`).
    let mut best: Option<f64> = None;
    let mut cover: Vec<(f64, f64)> = Vec::new();
    for k in 0..=spans.len() {
        let prev = if k > 0 { Some(spans[k - 1].1) } else { None };
        let next = spans.get(k).map(|sp| sp.0);
        // Region at speed v.
        let l = lo.max(s).max(prev.unwrap_or(f64::NEG_INFINITY));
        let u = hi.min(next.map_or(f64::INFINITY, |n| n - w * v));
        if l <= u && best.is_none() {
            best = Some(l);
        }
        // Speeds for which the gap is open (window non-emptiness excluded,
        // as everywhere in the DP).
        let a = next.map_or(f64::NEG_INFINITY, |n| (z1 + p.r - n) / t2);
        let mut b = (z0 - p.r - s) / (w + t1);
        if let Some(n) = next {
            b = b.min((n - s) / w);
        }
        if let Some(pv) = prev {
            b = b.min((z0 - p.r - pv) / (w + t1));
            if let Some(n) = next {
                b = b.min((n - pv) / w);
            }
        }
        if a <= b {
            cover.push((a, b));
        }
    }
    let zj = best?;
    cover.sort_by(|x, y| x.0.total_cmp(&y.0));
    let mut reach = v_min;
    for (a, b) in cover {
        if a > reach {
            break;
        }
        reach = reach.max(b);
        if reach >= v {
            return Some(zj);
        }
    }
    None
}

/// Advance the frontier over one row.
pub fn step(p: &FeasParams, prev: Option<&DpRow>, row: &DpRow, from: &Frontier) -> Frontier {
    let mut out: Frontier = [None; 3];
    for a in 0..3u8 {
        let Some(fa) = from[a as usize] else { continue };
        for b in 0..3u8 {
            if let Some(z) = transition(p, prev, row, a, b, fa) {
                let slot = &mut out[b as usize];
                *slot = Some(slot.map_or(z, |o: f64| o.min(z)));
            }
        }
    }
    out
}

pub fn alive(f: &Frontier) -> bool {
    f.iter().any(|x| x.is_some())
}

/// Run the DP over `rows` (ascending z) from `start`, with `prev` the row
/// before the first one (if any). Returns the final frontier, or `None` if
/// some row is unreachable in every lane.
pub fn run(p: &FeasParams, prev: Option<&DpRow>, rows: &[DpRow], start: Frontier) -> Option<Frontier> {
    let mut f = start;
    let mut pr = prev;
    for row in rows {
        f = step(p, pr, row, &f);
        if !alive(&f) {
            return None;
        }
        pr = Some(row);
    }
    Some(f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> FeasParams {
        FeasParams::from_tuning(&Tuning::default())
    }
    const H: (f64, f64) = (-0.5, 0.5);
    fn hay(z: f64) -> CellReq {
        CellReq::Jump { z0: z + H.0, z1: z + H.1 }
    }
    fn beam(z: f64) -> CellReq {
        CellReq::Duck { z0: z - 0.2, z1: z + 0.2 }
    }
    fn block(z: f64) -> CellReq {
        CellReq::Blocked { z0: z - 1.5, z1: z + 1.5 }
    }
    fn row(z: f64, l: [CellReq; 3], v: f32) -> DpRow {
        DpRow::plain(z, l, v)
    }
    fn start(z: f64) -> Frontier {
        [Some(z); 3]
    }

    #[test]
    fn params_sane() {
        let p = p();
        assert!(p.t1 > 0.05 && p.t1 < 0.2, "t1 {}", p.t1);
        assert!(p.t2 > 0.4 && p.t2 < 0.55, "t2 {}", p.t2);
        assert!((p.t_air - 0.6).abs() < 0.05, "t_air {}", p.t_air);
    }

    #[test]
    fn empty_course_feasible() {
        let rows: Vec<_> = (0..20).map(|i| row(i as f64 * 6.0, [CellReq::Free; 3], 20.0)).collect();
        let f = run(&p(), None, &rows, start(-10.0)).unwrap();
        assert!(f.iter().all(|x| x.is_some()));
    }

    #[test]
    fn full_block_wall_infeasible() {
        let rows = [row(30.0, [block(30.0); 3], 12.0)];
        assert!(run(&p(), None, &rows, start(0.0)).is_none());
    }

    #[test]
    fn beam_all_and_hay_rows_feasible_alone() {
        let rows = [row(30.0, [beam(30.0); 3], 20.0), row(60.0, [hay(60.0); 3], 20.0)];
        assert!(run(&p(), None, &rows, start(0.0)).is_some());
    }

    #[test]
    fn two_blocks_and_hay_feasible() {
        let rows = [row(30.0, [block(30.0), block(30.0), hay(30.0)], 12.0)];
        let f = run(&p(), None, &rows, start(0.0)).unwrap();
        assert_eq!(f[0], None);
        assert_eq!(f[1], None);
        assert!(f[2].is_some());
    }

    #[test]
    fn hay_wall_then_beam_wall_too_close_infeasible() {
        // Landing from the hay jump is too late to start a collapse for a beam 4 m on.
        let v = 20.0;
        let rows = [row(30.0, [hay(30.0); 3], v), row(34.0, [beam(34.0); 3], v)];
        assert!(run(&p(), None, &rows, start(0.0)).is_none());
        // ... but with a free row between them it works.
        let rows = [row(30.0, [hay(30.0); 3], v), row(36.0, [CellReq::Free; 3], v), row(42.0, [beam(42.0); 3], v)];
        assert!(run(&p(), None, &rows, start(0.0)).is_some());
    }

    #[test]
    fn lane_change_needs_time_between_blocks() {
        // Lanes 0,1 blocked at row A; then lanes 1,2 blocked at row B 6 m later:
        // the runner in lane 2 must cross two lanes inside the gap — too far at speed.
        let v = 20.0;
        let rows = [
            row(30.0, [block(30.0), block(30.0), CellReq::Free], v),
            row(36.0, [CellReq::Free, block(36.0), block(36.0)], v),
        ];
        assert!(run(&p(), None, &rows, start(0.0)).is_none());
        // Slow speed and free rows in between: feasible.
        let rows = [
            row(30.0, [block(30.0), block(30.0), CellReq::Free], 10.0),
            row(36.0, [CellReq::Free; 3], 10.0),
            row(42.0, [CellReq::Free; 3], 10.0),
            row(48.0, [CellReq::Free, block(48.0), block(48.0)], 10.0),
        ];
        assert!(run(&p(), None, &rows, start(0.0)).is_some());
    }

    #[test]
    fn lane_move_cannot_hide_in_an_empty_row() {
        // From lane 0, reaching lane 2 needs to cross lane 1, which has hay in
        // the row right after an empty one: the move must not be attributed to
        // the empty row and silently cross the hay.
        let v = 16.0;
        let rows = [
            row(36.0, [CellReq::Free; 3], v),
            row(42.0, [CellReq::Free, hay(42.0), block(42.0)], v),
        ];
        let f = run(&p(), None, &rows, [Some(35.0), None, None]).unwrap();
        assert!(f[0].is_some());
        assert!(f[2].is_none());
    }

    #[test]
    fn frontier_prefers_earliest() {
        let p = p();
        let r = row(30.0, [CellReq::Free, hay(30.0), CellReq::Free], 12.0);
        let f = step(&p, None, &r, &[None, Some(0.0), None]);
        // From lane 1, lanes 0/2 need one lane change; lane 1 needs a jump.
        let lc = (p.lane_change + p.slack) * 12.0;
        assert!((f[0].unwrap() - lc).abs() < 1e-9);
        assert!((f[2].unwrap() - lc).abs() < 1e-9);
        assert!(f[1].unwrap() > 30.0);
    }

    /// Mud span of a 6 m mud cell at row `z` (body half-depth 3 + r/2).
    fn mud_at(z: f64) -> (f64, f64) {
        (z - 3.21, z + 3.21)
    }

    #[test]
    fn hay_right_after_mud_cannot_be_jumped() {
        let p = p();
        for v in [13.0f32, 18.0] {
            let mut r = row(36.0, [hay(36.0); 3], v);
            r.v_min = v - 5.0;
            assert!(step(&p, None, &r, &start(0.0)).iter().all(|x| x.is_some()), "no mud: jumpable at {v}");
            // Mud filling the row before (z 30) in every lane: every take-off is in mud.
            r.mud = [pack_mud(vec![mud_at(30.0)]); 3];
            assert!(step(&p, None, &r, &start(0.0)).iter().all(|x| x.is_none()), "mud before the hay at {v}");
            // Mud two rows before (z 24) leaves room to take off after it.
            r.mud = [pack_mud(vec![mud_at(24.0)]); 3];
            assert!(step(&p, None, &r, &start(0.0)).iter().all(|x| x.is_some()), "mud far enough back at {v}");
        }
    }

    #[test]
    fn mud_rule_must_hold_across_the_speed_range() {
        let p = p();
        // Mud ending just before the hay: at 22 m/s the whole window sits
        // before the mud, but a slower runner's window slides into it.
        let z = 60.0;
        let mud = (z - 9.0, z - 2.0);
        let mut r = row(z, [hay(z); 3], 22.0);
        r.mud = [pack_mud(vec![mud]); 3];
        r.v_min = 22.0;
        let fast_only = jump_command(&p, &r.mud[0], z - 0.5, z + 0.5, 22.0, 22.0, 0.0);
        r.v_min = 13.0;
        let range = jump_command(&p, &r.mud[0], z - 0.5, z + 0.5, 22.0, 13.0, 0.0);
        assert!(fast_only.is_some(), "jumpable for a fast runner");
        assert!(range.is_none(), "not for a slow one: rejected");
    }

    #[test]
    fn pack_mud_merges_and_caps() {
        let m = pack_mud(vec![(10.0, 12.0), (0.0, 3.0), (2.0, 5.0), (20.0, 21.0), (30.0, 31.0), (40.0, 41.0), (50.0, 51.0)]);
        assert_eq!(m[0], Some((0.0, 5.0)));
        assert_eq!(m[1], Some((10.0, 12.0)));
        assert_eq!(m[3], Some((30.0, 51.0)));
    }

    #[test]
    fn merge_rules() {
        assert_eq!(CellReq::Free.merge(hay(0.0)), hay(0.0));
        assert!(matches!(hay(0.0).merge(beam(0.0)), CellReq::Blocked { .. }));
    }
}
