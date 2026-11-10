//! Runner body, locomotion state machine and the per-runner integration that
//! both `World::step` (host/solo) and client prediction use (`predict_runner`).

use serde::{Deserialize, Serialize};

use crate::course::{Course, ObjKind};
use crate::det;
use crate::tuning::Tuning;
use crate::{Command, Event, ObjId, PlayerId, Tick, DT};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Pos {
    pub x: f32,
    pub y: f32,
    pub z: f64,
}

/// `z_off` is forward speed minus `v_course` (SPEC §5 `vz_offset`); use
/// [`Runner::forward_speed`] for the total.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Vel {
    pub x: f32,
    pub y: f32,
    pub z_off: f32,
}

/// Locomotion (separate from protection).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Loco {
    Running,
    /// `kind` is the command being wound up (a jump or down variant).
    WindUp { kind: Command, until: Tick },
    /// `can_act` is always false (kept for the SPEC shape).
    Airborne { voluntary: bool, can_act: bool },
    Collapsed { until: Tick },
    Tumbling { until: Tick, spin_dir: i8 },
    Recovering { until: Tick },
}

impl Loco {
    pub fn is_low(&self) -> bool {
        matches!(self, Loco::Collapsed { .. } | Loco::Tumbling { .. })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerStats {
    pub jumps: u32,
    pub collapses: u32,
    pub crashes: u32,
    pub bumps: u32,
    pub tumbles: u32,
    pub hops: u32,
    pub feathers_lost: u32,
    /// Free hits used (obstacles passed through while protected).
    #[serde(default)]
    pub swaggers: u32,
    /// Jump commands refused because the body stood in mud.
    #[serde(default)]
    pub mud_stuck: u32,
    /// Runner commands sent while a ghost (ignored).
    pub ignored_cmds: u32,
    pub longest_protection_ticks: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Runner {
    pub id: PlayerId,
    pub pos: Pos,
    pub vel: Vel,
    pub loco: Loco,
    pub feathers: u8,
    /// `Some` = protected. Cleared only once the timer has passed **and** the
    /// runner is controllable again (`Running`), or at once by a second
    /// obstacle hit in the episode; `None` = vulnerable. Never extended.
    pub protected_until: Option<Tick>,
    pub coins: u32,
    /// Nearest lane to the body (updated every tick).
    pub settle_lane: u8,
    pub target_lane: u8,
    pub buffered: Option<(Command, Tick)>,
    /// Bitset of other runners (by `PlayerId`) in contact last tick.
    pub contacts_prev: u8,
    /// Objects the body touched last tick.
    pub touched_prev: Vec<ObjId>,
    pub alive_ticks: u64,
    pub stats: RunnerStats,
    /// Forward velocity components (m/s), each decaying.
    pub boost: f32,
    pub knock: f32,
    pub in_mud: bool,
    /// Obstacles that already produced an incident for this runner.
    pub incidents: Vec<ObjId>,
    /// Start of the current protection episode.
    pub protect_since: Option<Tick>,
    pub long_logged: bool,
    /// Personal speed bonus (m/s) earned from coins: `+coin_speed` per coin up
    /// to `coin_speed_cap`, halved by a damaging crash. Added to the forward
    /// velocity (`vel.z_off`) by the shared integration.
    #[serde(default)]
    pub speed_bonus: f32,
    /// The current protection episode's one free obstacle hit is still
    /// available (always false while vulnerable). HUD: show it.
    #[serde(default)]
    pub free_hit: bool,
    /// The obstacle being passed through on the free hit (no push-out, no
    /// further contact with it).
    #[serde(default)]
    pub swagger_obj: Option<ObjId>,
    /// Power-up timers: active while `tick < *_until` (0 = never active).
    /// Magnet: also collects coins near the runner.
    #[serde(default)]
    pub magnet_until: Tick,
    /// Doubler: every coin counts 2.
    #[serde(default)]
    pub double_until: Tick,
    /// Shield held (consumed by the next feather-losing crash).
    #[serde(default)]
    pub shield_until: Tick,
}

impl Runner {
    pub fn new(id: PlayerId, lane: u8, x: f32, z: f64, feathers: u8) -> Runner {
        Runner {
            id,
            pos: Pos { x, y: 0.0, z },
            vel: Vel::default(),
            loco: Loco::Running,
            feathers,
            protected_until: None,
            coins: 0,
            settle_lane: lane,
            target_lane: lane,
            buffered: None,
            contacts_prev: 0,
            touched_prev: Vec::new(),
            alive_ticks: 0,
            stats: RunnerStats::default(),
            boost: 0.0,
            knock: 0.0,
            in_mud: false,
            incidents: Vec::new(),
            protect_since: None,
            long_logged: false,
            speed_bonus: 0.0,
            free_hit: false,
            swagger_obj: None,
            magnet_until: 0,
            double_until: 0,
            shield_until: 0,
        }
    }

    /// Collision height for the current loco.
    pub fn height(&self, t: &Tuning) -> f32 {
        if self.loco.is_low() {
            t.height_low
        } else {
            t.height_stand
        }
    }
    /// Height of the band that blocks other runners: the body height capped
    /// at `torso_height` (neck and head do not block).
    pub fn torso(&self, t: &Tuning) -> f32 {
        self.height(t).min(t.torso_height)
    }
    pub fn controllable(&self) -> bool {
        self.loco == Loco::Running
    }
    pub fn protected(&self) -> bool {
        self.protected_until.is_some()
    }
    pub fn vulnerable(&self) -> bool {
        self.protected_until.is_none()
    }
    pub fn grounded(&self) -> bool {
        self.pos.y <= 1e-4
    }
    pub fn forward_speed(&self, v_course: f32) -> f32 {
        v_course + self.vel.z_off
    }
    /// Magnet active at tick `now`.
    pub fn magnet(&self, now: Tick) -> bool {
        now < self.magnet_until
    }
    /// Doubler active at tick `now`.
    pub fn doubler(&self, now: Tick) -> bool {
        now < self.double_until
    }
    /// Shield held at tick `now`.
    pub fn shield(&self, now: Tick) -> bool {
        now < self.shield_until
    }
}

/// What the integration needs from the world for one tick.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PredictCtx {
    /// The tick being simulated (`World::tick()` before the step).
    pub tick: Tick,
    pub v_course: f32,
    pub anchor_z: f64,
    /// Apply pack correction (multi); false in solo.
    pub pack: bool,
}

/// Client prediction / shared integration: applies `cmd` (same buffer and
/// wind-up rules), loco timers, lateral spring, forward speed incl. pack
/// correction, gravity, fences and mud. Contacts, damage, coins and
/// protection are host-only and not applied.
pub fn predict_runner(runner: &mut Runner, course: &Course, tuning: &Tuning, ctx: PredictCtx, cmd: Option<Command>) {
    let mut sink = Vec::new();
    let cmds: &[Command] = match &cmd {
        Some(c) => std::slice::from_ref(c),
        None => &[],
    };
    runner_tick(runner, course, tuning, ctx, cmds, &mut sink);
}

/// As [`predict_runner`], several commands, collecting events.
pub fn runner_tick(runner: &mut Runner, course: &Course, t: &Tuning, ctx: PredictCtx, cmds: &[Command], out: &mut Vec<Event>) {
    let now = ctx.tick;
    for &c in cmds {
        apply_command(runner, c, now, t, out);
    }
    update_loco(runner, now, t, out);
    integrate(runner, course, t, ctx, out);
}

fn apply_command(r: &mut Runner, cmd: Command, now: Tick, t: &Tuning, out: &mut Vec<Event>) {
    if r.controllable() {
        start_command(r, cmd, now, t, out);
    } else {
        r.buffered = Some((cmd, now));
    }
}

fn start_command(r: &mut Runner, cmd: Command, now: Tick, t: &Tuning, out: &mut Vec<Event>) {
    // Stuck in mud: no jump (straight or diagonal) can start. The jump is
    // dropped, not buffered; a diagonal jump's lane change still happens.
    let stuck = cmd.is_jump() && r.in_mud;
    if stuck {
        r.stats.mud_stuck += 1;
        out.push(Event::MudStuck { player: r.id });
    }
    let d = cmd.lane_delta();
    if d != 0 {
        let nl = r.target_lane as i8 + d;
        if (0..=2).contains(&nl) {
            r.target_lane = nl as u8;
            out.push(Event::LaneChange { player: r.id, to: r.target_lane });
        } else {
            // Lurch into the fence: bounce + wobble, no damage.
            r.vel.x += d as f32 * 5.0;
            out.push(Event::FenceBump { player: r.id });
        }
    }
    if stuck {
        // (lane change above only)
    } else if cmd.is_jump() {
        r.loco = Loco::WindUp { kind: cmd, until: now + t.ticks(t.jump_windup) };
        out.push(Event::WindUp { player: r.id, cmd });
    } else if cmd.is_down() {
        r.loco = Loco::WindUp { kind: cmd, until: now + t.ticks(t.down_windup) };
        out.push(Event::WindUp { player: r.id, cmd });
    }
}

fn update_loco(r: &mut Runner, now: Tick, t: &Tuning, out: &mut Vec<Event>) {
    match r.loco {
        Loco::WindUp { kind, until } if now >= until => {
            if kind.is_jump() {
                r.loco = Loco::Airborne { voluntary: true, can_act: false };
                r.vel.y = t.jump_vy;
                r.pos.y = r.pos.y.max(1e-3);
                let bias = if kind == Command::Jump { 0.0 } else { t.diag_jump_fwd_bias };
                r.boost = t.jump_fwd_boost + bias;
                r.stats.jumps += 1;
                out.push(Event::Jumped { player: r.id });
            } else {
                r.loco = Loco::Collapsed { until: now + t.ticks(t.collapse_time) };
                let bias = if kind == Command::Down { 0.0 } else { t.diag_dive_back_bias };
                r.boost = -t.collapse_fwd_loss + bias;
                r.stats.collapses += 1;
                out.push(Event::Collapsed { player: r.id });
            }
        }
        Loco::Collapsed { until } if now >= until => {
            r.loco = Loco::Recovering { until: now + t.ticks(t.standup_time) };
        }
        Loco::Tumbling { until, .. } if now >= until && r.grounded() => {
            r.target_lane = t.nearest_lane(r.pos.x);
            r.loco = Loco::Recovering { until: now + t.ticks(t.standup_time) };
        }
        Loco::Recovering { until } if now >= until => {
            r.loco = Loco::Running;
            out.push(Event::StoodUp { player: r.id });
        }
        _ => {}
    }
    if let Some((cmd, at)) = r.buffered {
        if now.saturating_sub(at) > t.ticks(t.input_buffer) {
            r.buffered = None;
        } else if r.controllable() {
            r.buffered = None;
            start_command(r, cmd, now, t, out);
        }
    }
}

fn integrate(r: &mut Runner, course: &Course, t: &Tuning, ctx: PredictCtx, out: &mut Vec<Event>) {
    let dt = DT;
    let now = ctx.tick;
    // Mud (ground contact only).
    let was_mud = r.in_mud;
    r.in_mud = r.pos.y < 0.2 && mud_at(course, r.pos.x, r.pos.z, t.body_radius, now);
    if r.in_mud && !was_mud {
        out.push(Event::MudEnter { player: r.id });
    }

    // Lateral: underdamped spring toward the target lane (off while tumbling).
    let tumbling = matches!(r.loco, Loco::Tumbling { .. });
    let mut ax = if tumbling {
        -2.5 * r.vel.x
    } else {
        let k = t.lateral_k * if matches!(r.loco, Loco::Recovering { .. }) { 0.6 } else { 1.0 };
        let a = k * (t.lane_x(r.target_lane) - r.pos.x) - 2.0 * t.lateral_zeta * k.sqrt() * r.vel.x;
        a.clamp(-t.lateral_amax, t.lateral_amax)
    };
    if r.in_mud && !tumbling {
        let phase = now as f32 * (std::f32::consts::TAU * 2.4 / 60.0) + r.id.0 as f32 * 1.7;
        ax += t.mud_wobble * 14.0 * det::sin_f32(phase);
    }
    r.vel.x += ax * dt;
    r.pos.x += r.vel.x * dt;
    let lim = t.fence_x - t.body_radius;
    if r.pos.x > lim {
        r.pos.x = lim;
        if r.vel.x > 0.0 {
            r.vel.x *= -0.5;
        }
    } else if r.pos.x < -lim {
        r.pos.x = -lim;
        if r.vel.x < 0.0 {
            r.vel.x *= -0.5;
        }
    }
    r.settle_lane = t.nearest_lane(r.pos.x);

    // Vertical.
    if r.pos.y > 0.0 || r.vel.y > 0.0 {
        r.vel.y -= t.gravity * dt;
        r.pos.y += r.vel.y * dt;
        if r.pos.y <= 0.0 {
            r.pos.y = 0.0;
            r.vel.y = 0.0;
            if let Loco::Airborne { .. } = r.loco {
                r.loco = Loco::Running;
                out.push(Event::Landed { player: r.id });
            }
        }
    }

    // Forward.
    r.boost *= det::exp_f32(-dt / t.boost_tau.max(1e-3));
    r.knock *= det::exp_f32(-dt / t.knock_tau.max(1e-3));
    let pack = if ctx.pack {
        let d = (r.pos.z - ctx.anchor_z) as f32;
        let mut c = -t.pack_k * d;
        if d.abs() > t.pack_band {
            c -= t.pack_k * 3.0 * (d - t.pack_band * d.signum());
        }
        c.clamp(-6.0, 6.0)
    } else {
        0.0
    };
    let bonus = r.speed_bonus.clamp(0.0, t.coin_speed_cap.max(0.0));
    let mud = if r.in_mud { (t.mud_speed_mult - 1.0) * (ctx.v_course + r.boost.max(0.0) + bonus) } else { 0.0 };
    let mut z_off = r.boost + r.knock + pack + mud + bonus;
    if ctx.v_course + z_off < 0.0 {
        z_off = -ctx.v_course;
    }
    if !z_off.is_finite() {
        z_off = 0.0;
    }
    r.vel.z_off = z_off;
    r.pos.z += ((ctx.v_course + z_off) * dt) as f64;
}

/// Is a body at `(x, z)` standing in live mud?
pub fn mud_at(course: &Course, x: f32, z: f64, radius: f32, now: Tick) -> bool {
    let h = &course.geom.h;
    let reach = (h.mud_depth * 0.5 + radius) as f64;
    let check = |ox: f32, oz: f64, hw: f32, hd: f32| (x - ox).abs() < hw + radius * 0.5 && (z - oz).abs() < (hd + radius * 0.5) as f64;
    for row in course.rows_in(z - reach, z + reach) {
        for lane in 0..3u8 {
            if row.cells[lane as usize].mud() && check(course.lane_x(lane), course.row_z(row.row_index), h.mud_width * 0.5, h.mud_depth * 0.5) {
                return true;
            }
        }
    }
    course
        .placed()
        .iter()
        .any(|o| o.kind == ObjKind::Mud && o.is_live(now) && check(o.x, o.z, o.half_w, o.half_d))
}
