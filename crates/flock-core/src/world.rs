//! The authoritative world: match flow, contacts, damage, protection, coins,
//! roles and standings. Placement lives in `place.rs`.

use serde::{Deserialize, Serialize};

use crate::bot::{GhostBot, RunnerBot};
use crate::course::{Course, GenCtx, ObjKind, PowerKind, Row};
use crate::runner::{runner_tick, Loco, PredictCtx, Runner};
use crate::{
    det, BotSkill, Command, EndReason, Event, MatchConfig, Mode, ObjId, Phase, PlaceRequest, PlaceResult, PlayerId,
    PlayerSpec, Role, RowRange, SoloResult, Standing, Tick, Tuning, DT, MAX_PLAYERS,
};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Player {
    pub spec: PlayerSpec,
    pub role: Role,
    /// The body. While a ghost / disconnected it is frozen and not simulated.
    pub runner: Runner,
    /// Accepted ghost placements.
    pub placements: u32,
    /// Ticks survived as a runner (frozen on elimination/disconnect).
    pub survived_ticks: u64,
    pub eliminated_at: Option<Tick>,
    pub disconnected: bool,
}

impl Player {
    pub fn id(&self) -> PlayerId {
        self.spec.id
    }
    pub fn ghost_energy(&self) -> Option<f32> {
        match self.role {
            Role::Ghost { energy, .. } => Some(energy),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct BotSlot {
    pub player: PlayerId,
    pub runner: RunnerBot,
    pub ghost: GhostBot,
}

/// Max plausible speed model used by every feasibility caller.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpeedModel {
    solo: bool,
    v0: f32,
    vmax: f32,
    ramp: f32,
    v_end: f32,
    cap: f32,
    boost: f32,
    elapsed_s: f32,
    lead_z: f64,
    v_now: f32,
}

impl SpeedModel {
    /// The model for `cfg` (solo: a pure function of `z`; multi: from the
    /// elapsed time and the leader).
    fn of(cfg: &MatchConfig, elapsed_s: f32, lead_z: f64, v_course: f32) -> SpeedModel {
        let t = &cfg.tuning;
        SpeedModel {
            solo: cfg.mode == Mode::Solo,
            v0: t.course_v0,
            vmax: t.course_vmax,
            ramp: t.course_ramp_m,
            v_end: t.mp_v_end,
            cap: t.mp_cap,
            // In a pack the pull toward the anchor cancels a coin bonus (equilibrium speed =
            // course speed), so multiplayer only pads for the decayed jump boost.
            boost: if cfg.mode == Mode::Solo { t.plausible_margin() } else { t.plausible_boost_frac * t.jump_fwd_boost },
            elapsed_s,
            lead_z,
            v_now: v_course.max(t.course_v0),
        }
    }

    pub fn at(&self, z: f64) -> f32 {
        let v = if self.solo {
            let d = z.max(0.0) as f32;
            self.v0 + (self.vmax - self.v0) * (1.0 - det::exp_f32(-d / self.ramp.max(1.0)))
        } else {
            // Conservative: arrival time estimated at today's (lower) speed.
            let dt = ((z - self.lead_z).max(0.0) as f32) / self.v_now.max(1.0);
            let f = if self.cap > 0.0 { ((self.elapsed_s + dt) / self.cap).clamp(0.0, 1.0) } else { 1.0 };
            self.v0 + (self.v_end - self.v0) * f
        };
        v + self.boost
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct World {
    cfg: MatchConfig,
    tick: Tick,
    phase: Phase,
    end: Option<EndReason>,
    running_since: Option<Tick>,
    players: Vec<Player>,
    course: Course,
    anchor: f64,
    v_course: f32,
    events: Vec<Event>,
    queued: Vec<Event>,
    bots: Vec<BotSlot>,
    /// Per player slot: recent (req, result) for idempotent retries.
    pub(crate) place_log: Vec<Vec<(u32, PlaceResult)>>,
}

pub(crate) const PLACE_LOG_LEN: usize = 64;

fn pair_mut<T>(v: &mut [T], i: usize, j: usize) -> (&mut T, &mut T) {
    debug_assert!(i != j);
    if i < j {
        let (a, b) = v.split_at_mut(j);
        (&mut a[i], &mut b[0])
    } else {
        let (a, b) = v.split_at_mut(i);
        (&mut b[0], &mut a[j])
    }
}

impl World {
    pub fn new(cfg: MatchConfig) -> World {
        let mut cfg = cfg;
        // Sanitise players: unique ids 0..=5, at most 6; solo keeps one.
        let mut seen = [false; MAX_PLAYERS];
        cfg.players.retain(|p| {
            let i = p.id.0 as usize;
            if i < MAX_PLAYERS && !seen[i] {
                seen[i] = true;
                true
            } else {
                false
            }
        });
        if cfg.players.is_empty() {
            cfg.players.push(PlayerSpec::new(PlayerId(0), "P1", 0, None));
        }
        if cfg.mode == Mode::Solo {
            cfg.players.truncate(1);
        }
        let t = cfg.tuning.clone();
        let n = cfg.players.len();
        // Start grid: front row (z 0) then back row (z −3), centre lane first;
        // rotated by round + seed so nobody is always at the back.
        let slots: [(u8, f64); MAX_PLAYERS] = [(1, 0.0), (0, 0.0), (2, 0.0), (1, -3.0), (0, -3.0), (2, -3.0)];
        let rot = ((cfg.round as u64).wrapping_add(cfg.seed) % n as u64) as usize;
        let players: Vec<Player> = cfg
            .players
            .iter()
            .enumerate()
            .map(|(i, spec)| {
                let (lane, z) = slots[(i + rot) % n];
                Player {
                    spec: spec.clone(),
                    role: Role::Runner,
                    runner: Runner::new(spec.id, lane, t.lane_x(lane), z, t.feathers.max(1)),
                    placements: 0,
                    survived_ticks: 0,
                    eliminated_at: None,
                    disconnected: false,
                }
            })
            .collect();
        let bots = players
            .iter()
            .filter_map(|p| {
                p.spec.bot.map(|skill| BotSlot {
                    player: p.id(),
                    runner: RunnerBot::new(skill, cfg.seed, p.id()),
                    ghost: GhostBot::new(skill, cfg.seed, p.id()),
                })
            })
            .collect();
        let anchor = players.iter().map(|p| p.runner.pos.z).sum::<f64>() / players.len() as f64;
        let course = fresh_course(&cfg);
        let mut w = World {
            course,
            cfg,
            tick: 0,
            phase: Phase::Countdown,
            end: None,
            running_since: None,
            players,
            anchor,
            v_course: 0.0,
            events: Vec::new(),
            queued: Vec::new(),
            bots,
            place_log: vec![Vec::new(); MAX_PLAYERS],
        };
        w.maintain_course();
        w
    }

    // ------------------------------------------------------------ accessors

    pub fn config(&self) -> &MatchConfig {
        &self.cfg
    }
    pub fn tuning(&self) -> &Tuning {
        &self.cfg.tuning
    }
    pub fn mode(&self) -> Mode {
        self.cfg.mode
    }
    pub fn tick(&self) -> Tick {
        self.tick
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn end_reason(&self) -> Option<EndReason> {
        self.end
    }
    pub fn anchor_z(&self) -> f64 {
        self.anchor
    }
    pub fn v_course(&self) -> f32 {
        self.v_course
    }
    /// Events of the last step (plus anything queued before it).
    pub fn events(&self) -> &[Event] {
        &self.events
    }
    pub fn course(&self) -> &Course {
        &self.course
    }
    pub fn players(&self) -> &[Player] {
        &self.players
    }
    pub fn player(&self, id: PlayerId) -> Option<&Player> {
        self.players.iter().find(|p| p.id() == id)
    }
    pub(crate) fn player_index(&self, id: PlayerId) -> Option<usize> {
        self.players.iter().position(|p| p.id() == id)
    }
    /// Living runner bodies.
    pub fn runners(&self) -> impl Iterator<Item = &Runner> {
        self.players.iter().filter(|p| p.role.is_runner()).map(|p| &p.runner)
    }
    /// The body of `id` if it is a living runner.
    pub fn runner(&self, id: PlayerId) -> Option<&Runner> {
        self.player(id).filter(|p| p.role.is_runner()).map(|p| &p.runner)
    }
    pub fn ghost_energy(&self, id: PlayerId) -> Option<f32> {
        self.player(id).and_then(|p| p.ghost_energy())
    }
    /// Ticks since `Go` (0 during countdown).
    pub fn running_ticks(&self) -> u64 {
        self.running_since.map_or(0, |s| self.tick.saturating_sub(s))
    }
    pub fn running_secs(&self) -> f32 {
        self.running_ticks() as f32 * DT
    }
    pub fn living_count(&self) -> usize {
        self.players.iter().filter(|p| p.role.is_runner()).count()
    }
    pub fn ghost_count(&self) -> usize {
        self.players.iter().filter(|p| p.role.is_ghost()).count()
    }
    pub fn leader_z(&self) -> Option<f64> {
        self.runners().map(|r| r.pos.z).fold(None, |a, z| Some(a.map_or(z, |a: f64| a.max(z))))
    }
    pub fn trailing_z(&self) -> Option<f64> {
        self.runners().map(|r| r.pos.z).fold(None, |a, z| Some(a.map_or(z, |a: f64| a.min(z))))
    }

    /// Solo: live distance/coins (`Some` in solo mode, at any phase).
    pub fn solo_result(&self) -> Option<SoloResult> {
        if self.cfg.mode != Mode::Solo {
            return None;
        }
        let r = &self.players.first()?.runner;
        Some(SoloResult { distance_m: r.pos.z.max(0.0) as f32, coins: r.coins })
    }

    pub(crate) fn speed_model(&self) -> SpeedModel {
        SpeedModel::of(&self.cfg, self.running_secs(), self.leader_z().unwrap_or(self.anchor), self.v_course)
    }

    /// Max plausible speed at `z`: `v_course` plus `Tuning::plausible_margin()` in solo
    /// (coin cap + a share of the jump boost) or just the boost share in multi.
    pub fn plausible_speed(&self, z: f64) -> f32 {
        self.speed_model().at(z)
    }

    /// The context the next `step` will integrate runners with. Clients use
    /// the host's values from snapshots for `predict_runner`.
    pub fn next_ctx(&self) -> PredictCtx {
        let t = &self.cfg.tuning;
        let solo = self.cfg.mode == Mode::Solo;
        let now = self.tick;
        let elapsed = self.running_since.map_or(0, |s| now.saturating_sub(s)) as f32 * DT;
        if solo {
            let z = self.players.first().map_or(0.0, |p| p.runner.pos.z);
            PredictCtx { tick: now, v_course: t.solo_speed(z), anchor_z: z, pack: false }
        } else {
            let v = t.multi_speed(elapsed);
            let n = self.living_count();
            let mean = if n > 0 { self.runners().map(|r| r.pos.z).sum::<f64>() / n as f64 } else { self.anchor };
            let anchor = self.anchor + (v * DT) as f64 + (t.pack_anchor_follow * DT) as f64 * (mean - self.anchor);
            PredictCtx { tick: now, v_course: v, anchor_z: anchor, pack: true }
        }
    }

    // ------------------------------------------------------------ stepping

    /// Advance one tick. Commands for unknown ids are ignored; commands from
    /// ghosts are ignored and counted.
    pub fn step(&mut self, cmds: &[(PlayerId, Command)]) {
        self.events.clear();
        self.events.append(&mut self.queued);
        if self.phase == Phase::Over {
            return;
        }
        let now = self.tick;
        let t = self.cfg.tuning.clone();
        self.course.set_clock(now);

        if self.phase == Phase::Countdown {
            if now >= t.ticks(t.countdown) {
                self.phase = Phase::Running;
                self.running_since = Some(now);
                self.events.push(Event::Go);
            } else {
                self.check_end();
                self.tick += 1;
                return;
            }
        }

        self.release_barrels(now);
        let ctx = self.next_ctx();
        self.v_course = ctx.v_course;
        self.anchor = ctx.anchor_z;

        // Bots.
        let mut all_cmds: Vec<(PlayerId, Command)> = cmds.to_vec();
        let mut ghost_reqs: Vec<PlaceRequest> = Vec::new();
        let mut bots = std::mem::take(&mut self.bots);
        for b in bots.iter_mut() {
            match self.player(b.player).map(|p| p.role) {
                Some(Role::Runner) => {
                    if let Some(c) = b.runner.think(self, b.player) {
                        all_cmds.push((b.player, c));
                    }
                }
                Some(Role::Ghost { .. }) => {
                    if let Some(r) = b.ghost.think(self, b.player) {
                        ghost_reqs.push(r);
                    }
                }
                _ => {}
            }
        }
        self.bots = bots;
        for r in ghost_reqs {
            let _ = self.place(r);
        }
        // Placements made inside this step report in this step.
        self.events.append(&mut self.queued);

        // Group commands per player.
        let mut per: Vec<Vec<Command>> = vec![Vec::new(); self.players.len()];
        for (id, c) in all_cmds {
            if let Some(i) = self.player_index(id) {
                match self.players[i].role {
                    Role::Runner => per[i].push(c),
                    _ => self.players[i].runner.stats.ignored_cmds += 1,
                }
            }
        }

        // Integration (shared with client prediction).
        for (i, p) in self.players.iter_mut().enumerate() {
            if p.role.is_runner() {
                runner_tick(&mut p.runner, &self.course, &t, ctx, &per[i], &mut self.events);
            }
        }

        self.runner_contacts(now, &t);
        let eliminated = self.obstacles(now, &t);
        self.pickups(now, &t);
        self.protection(now, &t);
        for i in eliminated {
            self.eliminate(i, now);
        }
        for p in self.players.iter_mut() {
            if p.role.is_runner() {
                p.runner.alive_ticks += 1;
                p.survived_ticks = p.runner.alive_ticks;
            } else if let Role::Ghost { energy, .. } = &mut p.role {
                *energy = (*energy + t.ghost_energy_regen * DT).min(t.ghost_energy_max);
            }
        }
        for o in self.course.placed() {
            if o.solid_at == now && o.by.is_some() {
                self.events.push(Event::Materialised { obj: o.id });
            }
        }
        if self.phase != Phase::Over {
            self.maintain_course();
            self.check_end();
        }
        self.tick += 1;
    }

    /// Start the roll of every barrel the leading runner has come within
    /// `barrel_trigger_m` of.
    fn release_barrels(&mut self, now: Tick) {
        let Some(lead) = self.leader_z() else { return };
        for row in self.course.barrels_to_release(lead) {
            if self.course.release_barrel(row, now) {
                self.events.push(Event::BarrelReleased { row, tick: now });
            }
        }
    }

    fn maintain_course(&mut self) {
        let t = &self.cfg.tuning;
        let (Some(lead), Some(trail)) = (self.leader_z(), self.trailing_z()) else { return };
        // Release behind the trailing living runner.
        let keep = trail - t.keep_behind as f64;
        if keep > 0.0 {
            let idx = self.course.row_at(keep);
            if self.course.first_index().is_some_and(|f| f < idx) {
                self.course.release_before(idx);
                let first = idx;
                let course = &self.course;
                for p in self.players.iter_mut() {
                    p.runner.incidents.retain(|id| match id.decode() {
                        Some((row, _, _)) => row >= first,
                        None => course.placed().iter().any(|o| o.id == *id),
                    });
                }
            }
        }
        let target = lead + t.gen_ahead as f64;
        if self.course.row_z(self.course.next_gen_index()) <= target {
            let speed = self.speed_model();
            let ctx = gen_ctx(&self.cfg, self.running_secs(), self.ghost_count());
            self.course.generate_until(target, &ctx, &|z| speed.at(z));
        }
    }

    fn check_end(&mut self) {
        if self.phase == Phase::Over || self.cfg.mode == Mode::Solo {
            return;
        }
        let connected = self.players.iter().filter(|p| !matches!(p.role, Role::Disconnected)).count();
        let reason = if self.phase == Phase::Running && self.living_count() == 0 {
            Some(EndReason::NoRunnersLeft)
        } else if self.phase == Phase::Running && self.running_secs() >= self.cfg.tuning.mp_cap {
            Some(EndReason::TimeCap)
        } else if connected < 2 {
            Some(EndReason::NotEnoughPlayers)
        } else {
            None
        };
        if let Some(r) = reason {
            self.finish(r);
        }
    }

    fn finish(&mut self, r: EndReason) {
        self.phase = Phase::Over;
        self.end = Some(r);
        self.events.push(Event::MatchOver { reason: r });
    }

    // ------------------------------------------------------------ contacts

    fn tumble(r: &mut Runner, now: Tick, t: &Tuning, spin: i8, out: &mut Vec<Event>) {
        let until = now + t.ticks(t.tumble_time);
        r.loco = match r.loco {
            Loco::Tumbling { until: u, spin_dir } => Loco::Tumbling { until: u.max(now + t.ticks(t.tumble_time * 0.5)), spin_dir },
            _ => Loco::Tumbling { until, spin_dir: if spin >= 0 { 1 } else { -1 } },
        };
        r.buffered = None;
        r.target_lane = t.nearest_lane(r.pos.x);
        r.stats.tumbles += 1;
        out.push(Event::Tumble { player: r.id });
    }

    /// Start a protection episode (after a feather loss or a broken shield)
    /// with its one free hit.
    fn start_protection(r: &mut Runner, now: Tick, t: &Tuning, out: &mut Vec<Event>) {
        let until = now + t.ticks(t.protect_time);
        r.protected_until = Some(until);
        r.protect_since = Some(now);
        r.long_logged = false;
        r.free_hit = true;
        out.push(Event::ProtectionStart { player: r.id, until });
    }

    fn runner_contacts(&mut self, now: Tick, t: &Tuning) {
        let living: Vec<usize> = (0..self.players.len()).filter(|&i| self.players[i].role.is_runner()).collect();
        let r2 = 2.0 * t.body_radius;
        let mut cur = [0u8; MAX_PLAYERS];
        let mut passover: Vec<(usize, usize)> = Vec::new();
        let lim = t.fence_x - t.body_radius;
        for iter in 0..3 {
            for ai in 0..living.len() {
                for bi in (ai + 1)..living.len() {
                    let (i, j) = (living[ai], living[bi]);
                    if iter > 0 && passover.contains(&(i, j)) {
                        continue;
                    }
                    let (pa, pb) = pair_mut(&mut self.players, i, j);
                    let (a, b) = (&mut pa.runner, &mut pb.runner);
                    let dx = b.pos.x - a.pos.x;
                    let dz = (b.pos.z - a.pos.z) as f32;
                    let d2 = dx * dx + dz * dz;
                    if d2 >= r2 * r2 {
                        continue;
                    }
                    let (ha, hb) = (a.torso(t), b.torso(t));
                    if a.pos.y >= b.pos.y + hb || b.pos.y >= a.pos.y + ha {
                        continue;
                    }
                    let (ia, ib) = (a.id.0 as usize % MAX_PLAYERS, b.id.0 as usize % MAX_PLAYERS);
                    if iter == 0 {
                        let new = a.contacts_prev & (1 << ib) == 0;
                        cur[ia] |= 1 << ib;
                        cur[ib] |= 1 << ia;
                        // Collapsed ramp / disruption (SPEC §6.2).
                        let mut over = false;
                        for flip in [false, true] {
                            let (hi, lo) = if flip { (&mut *a, &mut *b) } else { (&mut *b, &mut *a) };
                            if !lo.loco.is_low() || hi.loco.is_low() {
                                continue;
                            }
                            let standing = hi.grounded() && matches!(hi.loco, Loco::Running | Loco::WindUp { .. } | Loco::Recovering { .. });
                            if standing && hi.pos.z <= lo.pos.z + 0.15 {
                                hi.loco = Loco::Airborne { voluntary: false, can_act: false };
                                hi.vel.y = t.ramp_hop_vy;
                                hi.pos.y = hi.pos.y.max(1e-3);
                                hi.stats.hops += 1;
                                self.events.push(Event::Hop { player: hi.id, over: lo.id });
                                over = true;
                            } else if matches!(hi.loco, Loco::Airborne { .. }) {
                                if new {
                                    hi.knock -= 2.0;
                                    hi.vel.y = hi.vel.y.min(2.0);
                                    self.events.push(Event::Disrupted { player: hi.id, by: lo.id });
                                }
                                over = true;
                            }
                        }
                        // A bird that has just jumped (still rising, feet below the other's torso)
                        // vaults through it; once it starts down it is a normal contact again, so
                        // coming down on a standing bird's back resolves as a plain bump.
                        for (hi, lo) in [(&*a, &*b), (&*b, &*a)] {
                            if matches!(hi.loco, Loco::Airborne { voluntary: true, .. }) && hi.vel.y > 0.0 && !lo.loco.is_low() {
                                over = true;
                            }
                        }
                        if over {
                            passover.push((i, j));
                            continue;
                        }
                        // Impulse.
                        let d = d2.sqrt();
                        let (nx, nz) = if d > 1e-4 { (dx / d, dz / d) } else { (0.0, 1.0) };
                        let vn = (b.vel.x - a.vel.x) * nx + (b.vel.z_off - a.vel.z_off) * nz;
                        if vn < 0.0 {
                            let jm = -(1.0 + t.bump_restitution) * vn * 0.5;
                            a.vel.x -= jm * nx;
                            a.knock -= jm * nz;
                            a.vel.z_off -= jm * nz;
                            b.vel.x += jm * nx;
                            b.knock += jm * nz;
                            b.vel.z_off += jm * nz;
                            let closing = -vn;
                            if new && closing > t.significant_impact {
                                a.stats.bumps += 1;
                                b.stats.bumps += 1;
                                // Physical only: never extends protection or uses the free hit.
                                self.events.push(Event::Bump { a: a.id, b: b.id, speed: closing });
                            }
                            if new {
                                // Receiver tumbles on a lateral or from-behind impulse.
                                let (jax, jaz) = (-jm * nx, -jm * nz);
                                let (jbx, jbz) = (jm * nx, jm * nz);
                                if jax.abs() > t.tumble_impulse || jaz > t.tumble_impulse {
                                    Self::tumble(a, now, t, if jax >= 0.0 { 1 } else { -1 }, &mut self.events);
                                }
                                if jbx.abs() > t.tumble_impulse || jbz > t.tumble_impulse {
                                    Self::tumble(b, now, t, if jbx >= 0.0 { 1 } else { -1 }, &mut self.events);
                                }
                            }
                        }
                    }
                    // Positional correction (mass-equal).
                    let d = d2.sqrt();
                    let (nx, nz) = if d > 1e-4 { (dx / d, dz / d) } else { (0.0, 1.0) };
                    let pen = (r2 - d).max(0.0) * 0.5;
                    a.pos.x = (a.pos.x - nx * pen).clamp(-lim, lim);
                    b.pos.x = (b.pos.x + nx * pen).clamp(-lim, lim);
                    a.pos.z -= (nz * pen) as f64;
                    b.pos.z += (nz * pen) as f64;
                }
            }
        }
        for &i in &living {
            let r = &mut self.players[i].runner;
            r.contacts_prev = cur[r.id.0 as usize % MAX_PLAYERS];
        }
    }

    /// Obstacle contacts; returns indices of runners who lost their final feather.
    ///
    /// Protection (0.2 rule): a feather-losing crash starts an episode
    /// of `protect_time` with ONE free hit. The free hit passes through the
    /// obstacle untouched (`Swagger`: no tumble, knockback, speed or control
    /// loss). Any further damaging hit in the episode ends it at once and is
    /// a vulnerable crash (feather, tumble, a new episode with a new free
    /// hit). A held shield takes a vulnerable crash instead of a feather.
    fn obstacles(&mut self, now: Tick, t: &Tuning) -> Vec<usize> {
        let mut out = Vec::new();
        let rad = t.body_radius;
        for i in 0..self.players.len() {
            if !self.players[i].role.is_runner() {
                continue;
            }
            let z = self.players[i].runner.pos.z;
            let objs = self.course.objects_in(z - 3.0, z + 3.0);
            let r = &mut self.players[i].runner;
            // A crash (feather or shield) already happened this tick: further
            // new contacts this tick are part of the same incident.
            let mut lost = false;
            let mut touched = Vec::new();
            let mut through: Option<ObjId> = None;
            for o in objs.iter().filter(|o| o.kind.is_solid() && o.is_live(now)) {
                // Circle (x-z) vs rectangle.
                let cx = r.pos.x.clamp(o.x - o.half_w, o.x + o.half_w);
                let cz = r.pos.z.clamp(o.z0(), o.z1());
                let (ex, ez) = (r.pos.x - cx, (r.pos.z - cz) as f32);
                if ex * ex + ez * ez >= rad * rad {
                    continue;
                }
                let h = r.height(t);
                let hit = match o.kind {
                    ObjKind::Hay | ObjKind::Block | ObjKind::Barrel => r.pos.y < o.top,
                    ObjKind::Beam { .. } => r.pos.y + h > o.y && r.pos.y < o.top,
                    _ => false,
                };
                if !hit {
                    continue;
                }
                touched.push(o.id);
                if r.swagger_obj == Some(o.id) {
                    // Still passing through the free-hit obstacle.
                    through = Some(o.id);
                    continue;
                }
                let new = !r.incidents.contains(&o.id);
                if new && !lost && r.protected() && r.free_hit {
                    // The free hit: carry on through it.
                    r.incidents.push(o.id);
                    r.free_hit = false;
                    r.swagger_obj = Some(o.id);
                    through = Some(o.id);
                    r.stats.swaggers += 1;
                    self.events.push(Event::Swagger { player: r.id, obj: o.id });
                    continue;
                }
                if o.kind == ObjKind::Block {
                    // Solid wall: resolve out of it + anti-stuck toward a free lane.
                    let pz = ((r.pos.z + rad as f64) - o.z0()) as f32;
                    let px = ((r.pos.x + rad) - (o.x - o.half_w)).min((o.x + o.half_w) - (r.pos.x - rad));
                    if pz <= px {
                        r.pos.z = o.z0() - rad as f64;
                        r.knock = r.knock.min(0.0);
                        let side = if r.pos.x >= o.x { 1i8 } else { -1 };
                        let free = |l: i8| -> bool {
                            (0..=2).contains(&l)
                                && !objs.iter().any(|q| q.kind == ObjKind::Block && q.row == o.row && q.covers_lane(l as u8))
                        };
                        let l = o.lane as i8;
                        let pick = if free(l + side) {
                            Some(l + side)
                        } else if free(l - side) {
                            Some(l - side)
                        } else {
                            None
                        };
                        if let Some(nl) = pick {
                            r.target_lane = nl as u8;
                            if matches!(r.loco, Loco::Tumbling { .. }) {
                                let dir = (t.lane_x(nl as u8) - r.pos.x).signum();
                                r.vel.x += dir * 30.0 * DT;
                            }
                        }
                    } else if r.pos.x >= o.x {
                        r.pos.x = o.x + o.half_w + rad;
                        r.vel.x = r.vel.x.max(0.0);
                    } else {
                        r.pos.x = o.x - o.half_w - rad;
                        r.vel.x = r.vel.x.min(0.0);
                    }
                }
                if !new {
                    continue;
                }
                // New incident → crash.
                r.incidents.push(o.id);
                r.stats.crashes += 1;
                r.knock = r.knock.min(-t.crash_knockback);
                if matches!(o.kind, ObjKind::Hay | ObjKind::Barrel) {
                    // Trip and somersault over the bale.
                    r.vel.y = r.vel.y.max((2.0 * t.gravity * (o.top + 0.15)).sqrt());
                    r.pos.y = r.pos.y.max(1e-3);
                }
                Self::tumble(r, now, t, if r.pos.x >= o.x { 1 } else { -1 }, &mut self.events);
                let mut feather_lost = false;
                if !lost {
                    lost = true;
                    // Free hit already spent: the episode ends here and this is
                    // an ordinary vulnerable crash.
                    r.protected_until = None;
                    r.protect_since = None;
                    r.free_hit = false;
                    if r.shield(now) {
                        r.shield_until = 0;
                        self.events.push(Event::ShieldBroken { player: r.id });
                        Self::start_protection(r, now, t, &mut self.events);
                    } else {
                        feather_lost = true;
                        r.feathers = r.feathers.saturating_sub(1);
                        r.stats.feathers_lost += 1;
                        r.speed_bonus *= 0.5;
                        if r.feathers == 0 {
                            out.push(i);
                        } else {
                            Self::start_protection(r, now, t, &mut self.events);
                        }
                    }
                }
                self.events.push(Event::Crash { player: r.id, obj: o.id, kind: o.kind, feather_lost });
            }
            r.swagger_obj = through;
            r.touched_prev = touched;
        }
        out
    }

    /// Coins and power-ups. A pickup goes to the nearest touching runner
    /// (ties: lower `PlayerId`); a magnet also reaches coins within
    /// `magnet_reach_lanes` lane widths across and `magnet_reach_z` along,
    /// at any height.
    fn pickups(&mut self, now: Tick, t: &Tuning) {
        let (Some(lo), Some(hi)) = (self.trailing_z(), self.leader_z()) else { return };
        let reach_z = t.magnet_reach_z.max(0.0);
        let reach_x = t.magnet_reach_lanes.max(0.0) * t.lane_width;
        let items: Vec<_> = self
            .course
            .objects_in(lo - 2.0 - reach_z as f64, hi + 2.0 + reach_z as f64)
            .into_iter()
            .filter(|o| matches!(o.kind, ObjKind::Coin | ObjKind::Power { .. }) && o.is_live(now))
            .collect();
        let cr = t.hazards.coin_radius;
        for c in items {
            let mut best: Option<(f32, PlayerId, usize)> = None;
            for (i, p) in self.players.iter().enumerate() {
                if !p.role.is_runner() {
                    continue;
                }
                let r = &p.runner;
                let (dx, dz) = (r.pos.x - c.x, (r.pos.z - c.z) as f32);
                let hd = (dx * dx + dz * dz).sqrt();
                let touch = hd <= t.body_radius + t.hazards.collect_radius
                    && !(c.y + cr < r.pos.y || c.y - cr > r.pos.y + r.height(t));
                let magnet = c.kind == ObjKind::Coin && r.magnet(now) && dx.abs() <= reach_x && dz.abs() <= reach_z;
                if !touch && !magnet {
                    continue;
                }
                let better = match best {
                    None => true,
                    Some((bd, bid, _)) => hd < bd || (hd == bd && r.id < bid),
                };
                if better {
                    best = Some((hd, r.id, i));
                }
            }
            let Some((_, id, i)) = best else { continue };
            match c.kind {
                ObjKind::Coin => {
                    if self.course.remove_coin(c.id) {
                        let r = &mut self.players[i].runner;
                        r.coins += if r.doubler(now) { 2 } else { 1 };
                        r.speed_bonus = (r.speed_bonus + t.coin_speed).min(t.coin_speed_cap.max(0.0));
                        self.events.push(Event::CoinTaken { obj: c.id, by: id });
                    }
                }
                ObjKind::Power { .. } => {
                    if let Some(kind) = self.course.take_power(c.id) {
                        let r = &mut self.players[i].runner;
                        match kind {
                            PowerKind::Magnet => r.magnet_until = now + t.ticks(t.magnet_s),
                            PowerKind::Doubler => r.double_until = now + t.ticks(t.doubler_s),
                            PowerKind::Shield => r.shield_until = now + t.ticks(t.shield_s),
                        }
                        self.events.push(Event::PowerTaken { obj: c.id, by: id, kind });
                    }
                }
                _ => {}
            }
        }
    }

    fn protection(&mut self, now: Tick, t: &Tuning) {
        let long = t.ticks(t.long_protection_log_s);
        for p in self.players.iter_mut().filter(|p| p.role.is_runner()) {
            let r = &mut p.runner;
            let Some(until) = r.protected_until else { continue };
            let since = r.protect_since.unwrap_or(now);
            let dur = now.saturating_sub(since);
            r.stats.longest_protection_ticks = r.stats.longest_protection_ticks.max(dur);
            if now >= until && r.controllable() {
                r.protected_until = None;
                r.protect_since = None;
                r.free_hit = false;
                self.events.push(Event::ProtectionEnd { player: r.id });
            } else if dur > long && !r.long_logged {
                r.long_logged = true;
                self.events.push(Event::LongProtection { player: r.id, ticks: dur });
            }
        }
    }

    fn eliminate(&mut self, i: usize, now: Tick) {
        let start = self.cfg.tuning.ghost_energy_start;
        let solo = self.cfg.mode == Mode::Solo;
        let p = &mut self.players[i];
        if !p.role.is_runner() {
            return;
        }
        let id = p.id();
        p.runner.feathers = 0;
        p.runner.protected_until = None;
        p.runner.free_hit = false;
        p.survived_ticks = p.runner.alive_ticks;
        p.eliminated_at = Some(now);
        self.events.push(Event::Eliminated { player: id });
        if solo {
            self.finish(EndReason::FinalFeather);
        } else {
            p.role = Role::Ghost { energy: start, placed: 0 };
            self.events.push(Event::BecameGhost { player: id });
        }
    }

    // ------------------------------------------------------------ external

    /// Player left: body removed at once, role `Disconnected`, survival frozen.
    /// Takes effect immediately; its event is reported by the next `step`.
    pub fn disconnect(&mut self, id: PlayerId) {
        let Some(i) = self.player_index(id) else { return };
        let p = &mut self.players[i];
        if matches!(p.role, Role::Disconnected) {
            return;
        }
        if p.role.is_runner() {
            p.survived_ticks = p.runner.alive_ticks;
        }
        p.role = Role::Disconnected;
        p.disconnected = true;
        self.queued.push(Event::Disconnected { player: id });
        self.bots.retain(|b| b.player != id);
        if self.cfg.mode == Mode::Solo && self.phase != Phase::Over {
            self.phase = Phase::Over;
            self.end = Some(EndReason::Aborted);
            self.queued.push(Event::MatchOver { reason: EndReason::Aborted });
        }
    }

    /// End the match now (e.g. host left).
    pub fn abort(&mut self) {
        if self.phase != Phase::Over {
            self.phase = Phase::Over;
            self.end = Some(EndReason::Aborted);
            self.queued.push(Event::MatchOver { reason: EndReason::Aborted });
        }
    }

    /// Debug: eliminate a runner now (multi → ghost; solo → game over).
    /// Events are reported by the next `step`.
    pub fn force_eliminate(&mut self, id: PlayerId) {
        let Some(i) = self.player_index(id) else { return };
        if !self.players[i].role.is_runner() || self.phase == Phase::Over {
            return;
        }
        let mark = self.events.len();
        self.eliminate(i, self.tick);
        let evs: Vec<Event> = self.events.drain(mark..).collect();
        self.queued.extend(evs);
    }

    /// Debug: mutable access to a body (tests, debug tools).
    pub fn debug_runner_mut(&mut self, id: PlayerId) -> Option<&mut Runner> {
        let i = self.player_index(id)?;
        Some(&mut self.players[i].runner)
    }
    /// Debug: mutable course (insert hand-made rows in tests).
    pub fn debug_course_mut(&mut self) -> &mut Course {
        &mut self.course
    }
    /// Debug: set a ghost's energy.
    pub fn debug_set_energy(&mut self, id: PlayerId, e: f32) {
        if let Some(i) = self.player_index(id) {
            if let Role::Ghost { energy, .. } = &mut self.players[i].role {
                *energy = e;
            }
        }
    }

    /// Debug: six runners (no bots, empty course) queued in the centre lane.
    pub fn debug_six_in_one_lane(seed: u64) -> World {
        let mut cfg = MatchConfig::multi(seed, MAX_PLAYERS, None);
        cfg.tuning.debug_empty_course = true;
        let mut w = World::new(cfg);
        let t = w.cfg.tuning.clone();
        for (k, p) in w.players.iter_mut().enumerate() {
            p.runner = Runner::new(p.id(), 1, 0.0, -(k as f64) * 1.0, t.feathers);
        }
        w.anchor = -2.5;
        w
    }

    /// Debug: player 0 runs (human) against five ghost bots.
    pub fn debug_one_runner_five_ghosts(seed: u64) -> World {
        let mut cfg = MatchConfig::multi(seed, MAX_PLAYERS, Some(BotSkill::NORMAL));
        cfg.players[0].bot = None;
        let mut w = World::new(cfg);
        for k in 1..MAX_PLAYERS as u8 {
            w.force_eliminate(PlayerId(k));
        }
        w
    }

    pub(crate) fn push_queued(&mut self, e: Event) {
        self.queued.push(e);
    }
    pub(crate) fn course_mut(&mut self) -> &mut Course {
        &mut self.course
    }
    pub(crate) fn players_mut(&mut self) -> &mut [Player] {
        &mut self.players
    }

    // ------------------------------------------------------------ results

    /// Current standings (valid at any time; final once `phase() == Over`).
    pub fn standings(&self) -> Vec<Standing> {
        let key = |p: &Player| -> (u8, u64, u32) {
            match p.role {
                Role::Runner => (0, p.runner.feathers as u64, p.runner.coins),
                Role::Ghost { .. } => (1, p.survived_ticks, p.runner.coins),
                Role::Disconnected => (2, p.survived_ticks, p.runner.coins),
            }
        };
        // a better than b?
        let better = |a: (u8, u64, u32), b: (u8, u64, u32)| a.0 < b.0 || (a.0 == b.0 && (a.1, a.2) > (b.1, b.2));
        let mut out: Vec<Standing> = self
            .players
            .iter()
            .map(|p| {
                let k = key(p);
                let place = 1 + self.players.iter().filter(|q| better(key(q), k)).count() as u8;
                Standing {
                    player: p.id(),
                    place,
                    alive: p.role.is_runner(),
                    survived_ticks: if p.role.is_runner() { p.runner.alive_ticks } else { p.survived_ticks },
                    feathers: p.runner.feathers,
                    coins: p.runner.coins,
                    placements: p.placements,
                    disconnected: matches!(p.role, Role::Disconnected),
                }
            })
            .collect();
        out.sort_by_key(|s| (s.place, s.player));
        out
    }

    /// Placement band for ghosts (rows), empty when not running.
    pub fn placement_band(&self) -> RowRange {
        let t = &self.cfg.tuning;
        if self.phase != Phase::Running {
            return RowRange::EMPTY;
        }
        let (Some(lead), Some(last)) = (self.leader_z(), self.course.last_index()) else { return RowRange::EMPTY };
        let v = (self.v_course + t.jump_fwd_boost) as f64;
        let sp = self.course.row_spacing() as f64;
        let near = lead + t.band_near_s as f64 * v;
        let far = lead + t.band_far_s as f64 * v;
        let first = (near / sp).ceil().max(0.0) as u32;
        let lastb = ((far / sp).floor().max(0.0) as u32).min(last);
        RowRange { first, last: lastb }
    }

    /// Accept or reject a ghost placement (atomic). See SPEC §7.
    pub fn place(&mut self, req: PlaceRequest) -> PlaceResult {
        crate::place::place(self, req)
    }

    /// Same validation as `place`, without side effects. Returns the cost.
    pub fn check_placement(&self, player: PlayerId, tool: crate::Tool, lane: u8, row: u32) -> Result<f32, crate::Reject> {
        crate::place::check(self, player, tool, lane, row)
    }

    #[allow(dead_code)]
    pub(crate) fn obj_exists(&self, id: ObjId) -> bool {
        self.course.obj(id).is_some()
    }
}

/// The course a world for `cfg` starts with (no rows yet).
fn fresh_course(cfg: &MatchConfig) -> Course {
    let t = &cfg.tuning;
    let mut course = Course::new(t);
    if cfg.mode == Mode::Multi {
        course.geom.run_in = t.mp_run_in;
    }
    course
}

/// Generator context: solo is a pure function of the config (per-row
/// difficulty); multi depends on time and the ghost count (SPEC §4).
fn gen_ctx(cfg: &MatchConfig, running_secs: f32, ghosts: usize) -> GenCtx {
    let t = &cfg.tuning;
    let base = GenCtx {
        seed: cfg.seed,
        difficulty: 0.0,
        difficulty_ramp_m: 0.0,
        difficulty_floor_m: 0.0,
        density: 1.0,
        gentle_until: 0.0,
        ramp_m: t.mp_ramp_m as f64,
        powerup_gap_m: t.powerup_gap_m,
        powerup_jitter: t.powerup_jitter,
        powerup_weights: t.powerup_weights,
    };
    match cfg.mode {
        Mode::Solo => GenCtx {
            difficulty_ramp_m: t.solo_difficulty_ramp_m.max(1.0),
            difficulty_floor_m: t.solo_difficulty_floor_m,
            ..base
        },
        Mode::Multi => {
            let f = if t.mp_cap > 0.0 { (running_secs / t.mp_cap).clamp(0.0, 1.0) } else { 1.0 };
            GenCtx {
                difficulty: 0.2 + 0.7 * f,
                density: 1.0 / (1.0 + 0.25 * ghosts as f32),
                gentle_until: (t.mp_run_in + t.mp_gentle_m) as f64,
                powerup_gap_m: t.mp_powerup_gap_m,
                ..base
            }
        }
    }
}

/// The first `n` rows of the solo course for `cfg` (its mode is ignored:
/// solo rules), exactly as a solo `World` generates them, whatever the
/// runner does. Multiplayer rows depend on host state (time, ghosts,
/// placements) and are streamed instead.
pub fn course_rows(cfg: &MatchConfig, n: u32) -> Vec<Row> {
    let mut solo = cfg.clone();
    solo.mode = Mode::Solo;
    let mut course = fresh_course(&solo);
    let speed = SpeedModel::of(&solo, 0.0, 0.0, 0.0);
    let ctx = gen_ctx(&solo, 0.0, 0);
    if n > 0 {
        course.generate_until(course.row_z(n - 1), &ctx, &|z| speed.at(z));
    }
    course.rows().take(n as usize).copied().collect()
}

/// SHA-256 of the first `n` solo rows for `cfg` (see [`course_rows`]).
/// Bytes hashed per row: `row_index` (u32 little-endian) then the three cell
/// bytes. Log it on a device to prove the daily course matches.
pub fn course_fingerprint(cfg: &MatchConfig, n: u32) -> [u8; 32] {
    let mut bytes = Vec::with_capacity(n as usize * 7);
    for r in course_rows(cfg, n) {
        bytes.extend_from_slice(&r.row_index.to_le_bytes());
        bytes.extend(r.cells.iter().map(|c| c.0));
    }
    det::sha256(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world4() -> World {
        let mut c = MatchConfig::multi(1, 4, None);
        c.tuning.debug_empty_course = true;
        let mut w = World::new(c);
        while w.phase() == Phase::Countdown {
            w.step(&[]);
        }
        w
    }

    fn place_of(w: &World, id: u8) -> u8 {
        w.standings().iter().find(|s| s.player == PlayerId(id)).unwrap().place
    }

    #[test]
    fn standings_alive_rank_by_feathers_then_coins_with_shared_ties() {
        let mut w = world4();
        for (id, f, c) in [(0u8, 3u8, 5u32), (1, 3, 5), (2, 3, 9), (3, 2, 50)] {
            let r = w.debug_runner_mut(PlayerId(id)).unwrap();
            r.feathers = f;
            r.coins = c;
        }
        assert_eq!(place_of(&w, 2), 1);
        assert_eq!(place_of(&w, 0), 2);
        assert_eq!(place_of(&w, 1), 2, "equal feathers and coins share the place");
        assert_eq!(place_of(&w, 3), 4, "fewer feathers rank lower despite coins");
    }

    #[test]
    fn standings_eliminated_by_survival_then_coins_and_disconnected_last() {
        let mut w = world4();
        for _ in 0..60 {
            w.step(&[]);
        }
        w.debug_runner_mut(PlayerId(1)).unwrap().coins = 4;
        w.debug_runner_mut(PlayerId(2)).unwrap().coins = 4;
        w.force_eliminate(PlayerId(1));
        w.force_eliminate(PlayerId(2));
        for _ in 0..60 {
            w.step(&[]);
        }
        w.force_eliminate(PlayerId(3));
        w.debug_runner_mut(PlayerId(0)).unwrap().coins = 0;
        // P0 alive: first regardless of coins.
        assert_eq!(place_of(&w, 0), 1);
        // P3 survived longer than P1/P2.
        assert_eq!(place_of(&w, 3), 2);
        // P1 and P2 eliminated the same tick with equal coins: shared.
        assert_eq!(place_of(&w, 1), 3);
        assert_eq!(place_of(&w, 2), 3);
        // Coins break survival ties.
        w.players[1].runner.coins = 5;
        assert_eq!(place_of(&w, 1), 3);
        assert_eq!(place_of(&w, 2), 4);
        // Disconnected rank last and are flagged, even if they lasted longest.
        w.disconnect(PlayerId(0));
        let s = w.standings();
        let last = s.last().unwrap();
        assert_eq!(last.player, PlayerId(0));
        assert!(last.disconnected);
        assert_eq!(s[0].player, PlayerId(3));
    }

    #[test]
    fn speed_model_is_conservative_in_multi() {
        let w = world4();
        let m = w.speed_model();
        assert!(m.at(w.anchor_z() + 100.0) >= w.v_course());
    }
}
