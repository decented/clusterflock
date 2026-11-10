//! Runner and ghost bots (SPEC §9). Both are seeded and serialisable; the
//! world drives them inside `step` for players with `PlayerSpec.bot`.

use rand::{Rng, SeedableRng};
use rand_pcg::Pcg32;
use serde::{Deserialize, Serialize};

use crate::course::ObjKind;
use crate::feas::{self, DpRow};
use crate::runner::Loco;
use crate::{BotSkill, Command, ObjId, Phase, PlaceRequest, PlayerId, Tick, Tool, World, DT};

fn bot_rng(seed: u64, id: PlayerId, salt: u64) -> Pcg32 {
    Pcg32::seed_from_u64(seed ^ (0xB07_u64 << 32) ^ ((id.0 as u64) << 8) ^ salt)
}

/// Rough ticks until `r` accepts commands directly.
fn ticks_until_controllable(r: &crate::Runner, now: Tick, t: &crate::Tuning) -> u64 {
    match r.loco {
        Loco::Running => 0,
        Loco::WindUp { kind, until } => {
            let rest = until.saturating_sub(now);
            if kind.is_jump() {
                rest + t.ticks(2.0 * t.jump_vy / t.gravity)
            } else {
                rest + t.ticks(t.collapse_time + t.standup_time)
            }
        }
        Loco::Airborne { .. } => {
            let (y, vy, g) = (r.pos.y, r.vel.y, t.gravity.max(1.0));
            let tl = (vy + (vy * vy + 2.0 * g * y.max(0.0)).sqrt()) / g;
            t.ticks(tl)
        }
        Loco::Collapsed { until } => until.saturating_sub(now) + t.ticks(t.standup_time),
        Loco::Tumbling { until, .. } => until.saturating_sub(now) + t.ticks(t.standup_time),
        Loco::Recovering { until } => until.saturating_sub(now),
    }
}

/// Lane cost bonus per nearby runner: bots drift toward the pack, so contact happens.
const HERD_PULL: f32 = 0.3;

fn lerp(a: f32, b: f32, s: f32) -> f32 {
    a + (b - a) * s
}

/// Pending timed action against one obstacle.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct ActionPlan {
    obj: ObjId,
    cmd: Command,
    /// Position in the action window (0 = earliest, 1 = latest).
    frac: f32,
    /// Timing noise (s).
    offset_s: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunnerBot {
    pub skill: BotSkill,
    rng: Pcg32,
    next_plan: Tick,
    desired_lane: u8,
    lane_due: Option<Tick>,
    plan: Option<ActionPlan>,
    handled: Vec<ObjId>,
    /// Planned lane per upcoming row.
    path: Vec<(u32, u8)>,
    /// Earliest z for the next planned lane change.
    lane_start: f64,
}

impl RunnerBot {
    pub fn new(skill: BotSkill, seed: u64, id: PlayerId) -> Self {
        Self { skill, rng: bot_rng(seed, id, 1), next_plan: 0, desired_lane: 1, lane_due: None, plan: None, handled: Vec::new(), path: Vec::new(), lane_start: -1.0e18 }
    }

    fn react_ticks(&self) -> u64 {
        lerp(22.0, 4.0, self.skill.level()) as u64
    }
    fn error_rate(&self) -> f32 {
        lerp(0.22, 0.004, self.skill.level())
    }

    /// Choose this tick's command (if any) for `me`.
    pub fn think(&mut self, w: &World, me: PlayerId) -> Option<Command> {
        if w.phase() != Phase::Running {
            return None;
        }
        let r = w.runner(me)?;
        let now = w.tick();
        let t = w.tuning();
        let s = self.skill.level();
        let z = r.pos.z;

        // 1. Lane plan (every reaction interval).
        if now >= self.next_plan {
            self.next_plan = now + (self.react_ticks() / 2).max(3);
            if let Some(l) = self.plan_lane(w, me) {
                if l != self.desired_lane {
                    self.lane_due = None;
                }
                self.desired_lane = l;
            }
        }
        if self.desired_lane != r.target_lane && self.lane_due.is_none() && z >= self.lane_start - 0.3 {
            let delay = (self.react_ticks() as f32 * self.rng.gen_range(0.3..0.8)) as u64;
            self.lane_due = Some(now + delay);
        }

        // 2. Timed action against the next solid in the lane we are (going to be) in.
        let lane = r.target_lane;
        let fp = *w.course().feas_params();
        let v = r.forward_speed(w.v_course()).max(1.0) as f64;
        let window = |o: &crate::Obj| -> Option<(Command, f64, f64)> {
            match o.kind {
                ObjKind::Hay => {
                    let (lo, hi) = fp.jump_window(o.z0(), o.z1(), v + t.jump_fwd_boost as f64 * 0.7);
                    Some((Command::Jump, lo, hi))
                }
                ObjKind::Barrel => {
                    let vv = v + t.jump_fwd_boost as f64 * 0.7;
                    let hz = &t.hazards;
                    let speed = if o.dir < 0 { hz.barrel_speed_toward } else { hz.barrel_speed_away } as f64;
                    // Velocity it will roll at; an unreleased barrel starts when we are
                    // within the trigger distance (it is then at its home z).
                    let vb = if o.released && o.vz == 0.0 { 0.0 } else { o.dir as f64 * speed };
                    let mut gap = o.z - z;
                    let mut zr = z;
                    if !o.released && gap > hz.barrel_trigger_m as f64 {
                        zr += gap - hz.barrel_trigger_m as f64;
                        gap = hz.barrel_trigger_m as f64;
                    }
                    let u = (vv - vb).max(1.0);
                    let hr = o.half_d as f64 + fp.r;
                    let g_hi = u * (fp.jump_windup + fp.t2) - hr;
                    let g_lo = u * (fp.jump_windup + fp.t1) + hr;
                    Some((Command::Jump, zr + (gap - g_hi) * vv / u, zr + (gap - g_lo) * vv / u))
                }
                ObjKind::Beam { .. } => {
                    let (lo, hi) = fp.duck_window(o.z0(), o.z1(), v + t.jump_fwd_boost as f64);
                    Some((Command::Down, lo, hi))
                }
                _ => None,
            }
        };
        // Always aim at the nearest unhandled solid in our lane.
        let objs = w.course().objects_in(z - 1.0, z + 45.0);
        let next = objs
            .iter()
            .filter(|o| o.kind.is_solid() && o.covers_lane(lane) && o.z1() + t.body_radius as f64 > z && !self.handled.contains(&o.id))
            .min_by(|a, b| a.z.total_cmp(&b.z));
        if self.plan.is_some_and(|p| next.is_none_or(|o| o.id != p.obj)) {
            self.plan = None;
        }
        if self.plan.is_none() {
            if let Some(o) = next {
                if let Some((cmd, _, _)) = window(o) {
                    let sd = lerp(0.12, 0.015, s);
                    let noise: f32 = (0..3).map(|_| self.rng.gen_range(-1.0f32..1.0)).sum::<f32>() / 1.7;
                    let mut frac = 0.25;
                    let mut offset_s = noise * sd;
                    let mut cmd = cmd;
                    if self.rng.gen::<f32>() < self.error_rate() {
                        // A real mistake: badly timed or the wrong action.
                        if self.rng.gen::<bool>() {
                            frac = if self.rng.gen::<bool>() { 1.6 } else { -0.7 };
                            offset_s = 0.0;
                        } else {
                            cmd = if cmd == Command::Jump { Command::Down } else { Command::Jump };
                        }
                    }
                    self.plan = Some(ActionPlan { obj: o.id, cmd, frac, offset_s });
                } else {
                    self.handled.push(o.id);
                }
            }
        }
        if let Some(p) = self.plan {
            // Cap: finish this action in time for the next solid in the lane.
            let after = w.course().obj(p.obj).and_then(|o| {
                objs.iter()
                    .filter(|q| q.kind.is_solid() && q.covers_lane(lane) && q.z0() > o.z1())
                    .min_by(|a, b| a.z.total_cmp(&b.z))
                    .and_then(&window)
                    .map(|(_, _, hi_next)| {
                        let len = if p.cmd.is_jump() { fp.jump_len(v + t.jump_fwd_boost as f64) } else { fp.duck_len(v) };
                        hi_next - len - 0.4
                    })
            });
            let trig = w.course().obj(p.obj).and_then(|o| window(&o)).map(|(_, lo, hi)| {
                let mut tz = lo + (hi - lo).max(0.0) * p.frac as f64 + (p.offset_s as f64) * v;
                if let Some(cap) = after {
                    if p.frac > 0.0 && p.frac < 1.0 {
                        tz = tz.min(cap).max(lo + (hi - lo).max(0.0) * 0.03);
                    }
                }
                tz
            });
            let obj = w.course().obj(p.obj);
            let elsewhere = obj.is_some_and(|o| self.path.iter().any(|&(row, l)| row == o.row && !o.covers_lane(l)));
            let last_chance = obj.and_then(|o| window(&o)).is_some_and(|(_, _, hi)| z >= hi - 0.12 * v);
            // Hold the command until it would survive the input buffer.
            let buffer = t.ticks(t.input_buffer).saturating_sub(1);
            let soon = ticks_until_controllable(r, now, t) <= buffer;
            // A jump cannot start in mud: hold it until the body is out.
            let stuck = p.cmd.is_jump() && r.in_mud;
            let fire = trig.is_none_or(|tz| z >= tz) && soon && !stuck && (!elsewhere || last_chance || !r.controllable());
            if fire {
                self.handled.push(p.obj);
                if self.handled.len() > 32 {
                    self.handled.remove(0);
                }
                self.plan = None;
                if trig.is_some() {
                    return Some(p.cmd);
                }
            }
        }

        // 3. Lane command once the reaction delay has passed.
        if let Some(due) = self.lane_due {
            if now >= due && r.controllable() {
                self.lane_due = None;
                let d = self.desired_lane as i8 - r.target_lane as i8;
                if d != 0 {
                    if self.rng.gen::<f32>() < self.error_rate() * 0.5 {
                        return None; // hesitated
                    }
                    return Some(if d < 0 { Command::Left } else { Command::Right });
                }
            }
        }
        None
    }

    /// Cheapest feasible lane path over the next few rows; returns the lane
    /// for the first upcoming row.
    fn plan_lane(&mut self, w: &World, me: PlayerId) -> Option<u8> {
        let r = w.runner(me)?;
        let t = w.tuning();
        let c = w.course();
        let p = c.feas_params();
        let z = r.pos.z;
        let s = self.skill.level();
        let first = c.row_at((z - 2.0).max(0.0));
        let mut rows: Vec<(DpRow, [f32; 3])> = Vec::new();
        let mut prev: Option<DpRow> = None;
        if first > 0 {
            let idx = first - 1;
            if c.row(idx).is_some() {
                prev = Some(c.dp_row(idx, w.plausible_speed(c.row_z(idx)), &[]));
            }
        }
        let mut idx = first;
        while rows.len() < 16 {
            if c.row(idx).is_none() {
                break;
            }
            let rz = c.row_z(idx);
            let dp = c.dp_row(idx, w.plausible_speed(rz), &[]);
            let mut extra = [0.0f32; 3];
            for o in c.row_objects(idx) {
                if !o.is_live(w.tick() + 30) {
                    continue;
                }
                let cost = match o.kind {
                    ObjKind::Coin => -0.25 * lerp(1.4, 0.8, s),
                    ObjKind::Mud => 0.5,
                    ObjKind::Hay | ObjKind::Barrel => 0.5,
                    ObjKind::Beam { .. } => 0.7,
                    _ => 0.0,
                };
                let (a, b) = o.lanes();
                for l in a..=b.min(2) {
                    extra[l as usize] += cost;
                }
            }
            if rows.len() < 5 {
                for o in w.runners().filter(|o| o.id != me) {
                    let dz = o.pos.z - z;
                    if dz > 0.0 && dz < 5.0 {
                        // Skilled bots mildly avoid a runner just ahead (not perfectly).
                        if rows.is_empty() {
                            extra[t.nearest_lane(o.pos.x) as usize] += 0.4 * s;
                        }
                    } else if dz.abs() < 12.0 {
                        // Flock instinct: drift toward the lanes the pack runs in.
                        extra[t.nearest_lane(o.pos.x) as usize] -= HERD_PULL;
                    }
                }
            }
            if dp.z > z - 2.0 {
                rows.push((dp, extra));
            }
            idx += 1;
        }
        if rows.is_empty() {
            return None;
        }
        let a0 = if r.controllable() { r.target_lane } else { r.settle_lane };
        let v_now = r.forward_speed(w.v_course()).max(1.0);
        let react = self.react_ticks() as f32 * DT + 0.05;
        let free0 = z + ((ticks_until_controllable(r, w.tick(), t) as f32 * DT + react) * v_now) as f64;
        // Prefer routes that survive a pessimistic model (extra reaction
        // slack on every manoeuvre); fall back to the exact model.
        let mut robust = *p;
        robust.slack += (react + 0.1) as f64;
        let best = match search(&robust, prev.as_ref(), &rows, a0, free0) {
            (true, Some(b)) => b,
            _ => search(p, prev.as_ref(), &rows, a0, free0).1?,
        };
        self.path = rows.iter().zip(best.path.iter()).map(|(r, l)| (c.row_at(r.0.z + 0.1), *l)).collect();
        // First lane change on the path, and the earliest z it may start.
        let mut prev_lane = a0;
        for (k, &l) in best.path.iter().enumerate() {
            if l != prev_lane {
                let pr = if k == 0 { prev.as_ref() } else { Some(&rows[k - 1].0) };
                let mut start = -1.0e18;
                if let Some(pr) = pr {
                    start = pr.z;
                    for ln in prev_lane.min(l)..=prev_lane.max(l) {
                        if let Some((_, z1)) = pr.lanes[ln as usize].extent() {
                            start = start.max(z1 + p.r);
                        }
                    }
                }
                self.lane_start = start;
                return Some(l);
            }
            prev_lane = l;
        }
        self.lane_start = -1.0e18;
        Some(a0)
    }
}

#[derive(Clone)]
struct PathState {
    cost: f32,
    free: f64,
    lane: u8,
    path: Vec<u8>,
}

/// Forward search with per-lane Pareto pruning on (cost, free_z). The
/// min-free state per lane is always kept, so a feasible path is never lost.
/// Returns (reached the horizon, cheapest state).
fn search(p: &feas::FeasParams, prev: Option<&DpRow>, rows: &[(DpRow, [f32; 3])], a0: u8, free0: f64) -> (bool, Option<PathState>) {
    let mut states = vec![PathState { cost: 0.0, free: free0, lane: a0, path: Vec::new() }];
    let mut disc = 1.0f32;
    let mut full = true;
    for k in 0..rows.len() {
        let pr = if k == 0 { prev } else { Some(&rows[k - 1].0) };
        let mut next: Vec<PathState> = Vec::new();
        for st in &states {
            for b in 0..3u8 {
                if let Some(f) = feas::transition(p, pr, &rows[k].0, st.lane, b, st.free) {
                    let mut path = st.path.clone();
                    path.push(b);
                    let cost = st.cost + (st.lane as i32 - b as i32).abs() as f32 * (0.6 + 0.08 * k as f32) + disc * rows[k].1[b as usize];
                    next.push(PathState { cost, free: f, lane: b, path });
                }
            }
        }
        if next.is_empty() {
            full = false;
            break; // nothing reachable further: keep the best partial plan
        }
        let mut kept: Vec<PathState> = Vec::new();
        for lane in 0..3u8 {
            let mut v: Vec<PathState> = next.iter().filter(|s| s.lane == lane).cloned().collect();
            v.sort_by(|x, y| x.free.total_cmp(&y.free).then(x.cost.total_cmp(&y.cost)));
            let mut best_cost = f32::INFINITY;
            let mut n = 0;
            for st in v {
                if st.cost < best_cost && n < 4 {
                    best_cost = st.cost;
                    n += 1;
                    kept.push(st);
                }
            }
        }
        states = kept;
        disc *= 0.85;
    }
    (full, states.into_iter().min_by(|x, y| x.cost.total_cmp(&y.cost)))
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GhostBot {
    pub skill: BotSkill,
    rng: Pcg32,
    next_try: Tick,
    req: u32,
}

impl GhostBot {
    pub fn new(skill: BotSkill, seed: u64, id: PlayerId) -> Self {
        Self { skill, rng: bot_rng(seed, id, 2), next_try: 0, req: 0 }
    }

    /// A valid placement every few seconds: lures early, hay/beam later.
    pub fn think(&mut self, w: &World, me: PlayerId) -> Option<PlaceRequest> {
        if w.phase() != Phase::Running {
            return None;
        }
        let now = w.tick();
        if self.next_try == 0 {
            // First action soon after becoming a ghost.
            self.next_try = now + self.rng.gen_range(30..90);
            return None;
        }
        if now < self.next_try {
            return None;
        }
        let energy = w.ghost_energy(me)?;
        let t = w.tuning();
        let s = self.skill.level();
        let f = (w.running_secs() / t.mp_cap.max(1.0)).clamp(0.0, 1.0);
        let weights = [
            (Tool::Coins, 3.0 * (1.0 - f) + 0.4),
            (Tool::Mud, 1.2),
            (Tool::Hay, 0.4 + 3.0 * f),
            (Tool::Beam, 0.3 + 2.5 * f),
        ];
        let affordable: Vec<_> = weights.iter().filter(|(tl, _)| t.tool_cost(*tl) <= energy).copied().collect();
        if affordable.is_empty() {
            self.next_try = now + 30;
            return None;
        }
        let total: f32 = affordable.iter().map(|x| x.1).sum();
        let mut pick = self.rng.gen::<f32>() * total;
        let mut tool = affordable[0].0;
        for (tl, wgt) in &affordable {
            if pick < *wgt {
                tool = *tl;
                break;
            }
            pick -= wgt;
        }
        let band = w.placement_band();
        if band.is_empty() {
            self.next_try = now + 20;
            return None;
        }
        // Candidate cells, preferring lanes the runners are heading for.
        let mut cells: Vec<(f32, u32, u8)> = Vec::new();
        for row in band.iter() {
            for lane in 0..3u8 {
                let mut score = self.rng.gen::<f32>();
                if w.runners().any(|r| r.target_lane == lane) {
                    score += 0.6 * s;
                }
                cells.push((score, row, lane));
            }
        }
        cells.sort_by(|a, b| b.0.total_cmp(&a.0));
        for tl in [tool, Tool::Coins] {
            for &(_, row, lane) in cells.iter().take(24) {
                if w.check_placement(me, tl, lane, row).is_ok() {
                    self.req = self.req.wrapping_add(1);
                    let secs = lerp(4.5, 2.0, s) * self.rng.gen_range(0.7..1.3);
                    self.next_try = now + t.ticks(secs);
                    return Some(PlaceRequest { player: me, req: self.req, tool: tl, lane, row });
                }
            }
        }
        self.next_try = now + 30;
        None
    }
}
