//! The presenter: `World` (via `FlockView`) to a render `FrameScene`.
//!
//! One `Flop` per player drives the floppy visuals; positions are smoothed
//! with velocity feed-forward so the 60 Hz tick stepping of the offline
//! driver does not judder; sim x is mirrored into render space in exactly one
//! function ([`cameras::to_render`]); the floating origin keeps render z small.
//! The scene's vectors are reused frame to frame.

use super::cameras::{to_render, CameraRig};
use super::fx::{shape_of, Fx, Signal, Where};
use flock_core::flop::Flop;
use flock_core::{Command, Event, Loco, Obj, ObjId, ObjKind, Phase, PlayerId, PowerKind, Role, Runner, World, DT};
use flock_render::BALL_RADIUS;
use flock_render::{
    BlockVariant, Burst, BurstKind, CellState, CoinInst, DropMark, Emitter, FarmPropInst, FarmPropKind, FrameScene, OstrichPose, PlacementCell, PlayerStyle, PowerInst, Shield, DROP_HEIGHT, TILE_LEN,
};
use flock_session::FlockView;
use glam::{Vec2, Vec3};
use std::f32::consts::{PI, TAU};
use std::collections::HashMap;

/// The local player's rim colour.
pub const OUTLINE: [f32; 3] = [1.0, 0.93, 0.45];
const TRACK_DUST: [f32; 3] = [0.78, 0.6, 0.38];
/// Seconds of local anticipation (a little longer than the longest wind-up).
const ANTICIPATE: f32 = 0.24;
/// Fog scale of the run scene (home uses 0.8).
pub const RUN_FOG: f32 = 0.75;
/// Widest extra vertical FOV at top speed (degrees) and its easing time constant (seconds).
pub const FOV_BOOST_DEG: f32 = 6.0;
const FOV_TAU: f32 = 0.5;
/// Seconds an architect drop takes to settle after it lands (squash and spring back).
pub const SETTLE_SECS: f32 = 0.45;
/// Ghost looks: the local ghost (you, the architect) is a little bolder than the others a runner sees.
const OWN_GHOST_FILL: f32 = 0.20;
const OWN_GHOST_LINE: f32 = 0.85;
/// Chance of a landing shaking the camera: strength by kind at no distance, falling off over ~10 m.
const KICK_HAY: f32 = 0.35;
const KICK_BEAM: f32 = 0.45;
const KICK_MUD: f32 = 0.15;
/// Seconds of the comic stagger after the free hit (pose only: the sim keeps control).
pub const SWAGGER_SECS: f32 = 0.6;
/// Seconds of the squelch bob when a jump is refused in mud.
pub const STUCK_SECS: f32 = 0.3;
/// Seconds a magnetised coin takes to fly to the bird (visual only).
pub const FLY_SECS: f32 = 0.15;
/// Seconds the shield bubble takes to burst.
const POP_SECS: f32 = 0.28;
/// Radius of the shield bubble round a bird, metres.
const BUBBLE_R: f32 = 1.75;

/// A coin or power-up the sim reports taken, flying the last metres to the bird (visual only).
#[derive(Clone, Copy, Debug)]
struct Flyer {
    /// Sim position it left from.
    from: (f32, f32, f64),
    by: PlayerId,
    power: Option<PowerKind>,
    age: f32,
}

/// Who dropped an architect object, kept from its `Placed` event. A late joiner never sees the
/// event: the object's own `by` and the name list stand in, and failing that it is unmarked.
#[derive(Clone, Debug)]
pub struct Placer {
    pub by: PlayerId,
    pub colour: u8,
    pub name: Option<String>,
}

/// A gate's name banner this frame, for the Run screen's name label.
#[derive(Clone, Debug)]
pub struct BannerTag {
    /// Render-space middle of the banner (falling with the gate), and its size in metres.
    pub centre: Vec3,
    pub size: Vec2,
    pub name: String,
    pub colour: u8,
}

#[derive(Clone, Copy, Default)]
struct Body {
    x: f32,
    y: f32,
    z: f64,
    init: bool,
}

#[derive(Clone, Copy, Default)]
struct Eased {
    /// Tuck-and-roll ball 0..1 (Collapsed in, Recovering out).
    ball: f32,
    /// Somersault angle, unwrapped (radians): distance rolled while Collapsed / `BALL_RADIUS`.
    roll: f32,
    stars: f32,
    ghost: f32,
    gait: f32,
    /// Ghost hover offset from the anchor (x sim, y, z metres).
    goff: Vec3,
    goff_init: bool,
}

/// A bird drawn this frame, for overlays.
#[derive(Clone, Copy, Debug)]
pub struct BirdTag {
    pub id: PlayerId,
    /// Render-space point above the head.
    pub head: Vec3,
    /// Render-space feet.
    pub feet: Vec3,
    /// Protection time left, 0..1 (None = vulnerable).
    pub protected: Option<f32>,
    pub ghost: bool,
    pub local: bool,
    /// Sim body (x, z) radius for debug boxes: (radius, height).
    pub body: (f32, f32),
}

pub struct Presenter {
    pub scene: FrameScene,
    pub rig: CameraRig,
    pub fx: Fx,
    /// Render-space z of the floating origin's sim z.
    pub origin: f64,
    last_origin: f64,
    /// Metres to hand to `shift_world_z` at the next paint.
    pub pending_shift: f32,
    flops: [Flop; 6],
    body: [Body; 6],
    eased: [Eased; 6],
    anticip: Option<(Command, f32)>,
    /// Ghost "release" animation of the local ghost (seconds since the drop).
    release: f32,
    hover_z: f64,
    hover_init: bool,
    pub signals: Vec<Signal>,
    /// One per bird drawn this frame (HUD rings and debug overlays).
    pub tags: Vec<BirdTag>,
    /// Rolling barrels: the 1.2 m dust cell last puffed, by object id.
    barrel_dust: HashMap<u32, i64>,
    time: f32,
    started: bool,
    /// Extra vertical FOV (radians) for speed: eased, solo runner view only.
    fov_extra: f32,
    /// Architect objects by id: who placed them (from `Placed`), pruned as the course drops them.
    placers: HashMap<ObjId, Placer>,
    /// The name banners of marked gates this frame.
    pub banners: Vec<BannerTag>,
    /// The tool column is on the left: the ghost view keeps its cells clear of it.
    pub left_handed: bool,
    /// Seconds since each bird's free hit / refused jump (large = not playing).
    swagger: [f32; 6],
    stuck: [f32; 6],
    /// Render-space feet of each bird last drawn (shield pops).
    feet: [Vec3; 6],
    /// Shields bursting: (bird index, age).
    pops: Vec<(usize, f32)>,
    /// Coins and power-ups seen last frame by object id (sim position), for the magnet fly-in.
    seen: HashMap<u32, (Option<PowerKind>, f32, f32, f64)>,
    flyers: Vec<Flyer>,
}

impl Default for Presenter {
    fn default() -> Self {
        Self::new()
    }
}

struct Pos<'a> {
    world: &'a World,
    view: &'a FlockView,
    origin: f64,
}

impl Where for Pos<'_> {
    fn at(&self, id: PlayerId) -> Vec3 {
        match self.world.player(id) {
            Some(p) => to_render(p.runner.pos.x, p.runner.pos.y, p.runner.pos.z, self.origin),
            None => Vec3::ZERO,
        }
    }
    fn colour(&self, id: PlayerId) -> u8 {
        self.view.name_of(id).map_or(id.0, |n| n.1)
    }
    fn doubler(&self, id: PlayerId) -> bool {
        self.world.runner(id).is_some_and(|r| r.doubler(self.world.tick()))
    }
}

fn smooth_to(cur: &mut f32, target: f32, dt: f32, tau: f32) {
    *cur += (target - *cur) * (1.0 - (-dt / tau).exp());
}

impl Presenter {
    pub fn new() -> Presenter {
        Presenter {
            // a little less fog than the default so the lane ahead reads further
            scene: FrameScene { fog_scale: RUN_FOG, ..FrameScene::default() },
            rig: CameraRig::new(),
            fx: Fx::new(),
            origin: 0.0,
            last_origin: 0.0,
            pending_shift: 0.0,
            flops: std::array::from_fn(|_| Flop::new()),
            body: [Body::default(); 6],
            eased: [Eased::default(); 6],
            anticip: None,
            release: 9.0,
            hover_z: 0.0,
            hover_init: false,
            signals: Vec::new(),
            tags: Vec::new(),
            barrel_dust: HashMap::new(),
            time: 0.0,
            started: false,
            fov_extra: 0.0,
            placers: HashMap::new(),
            banners: Vec::new(),
            left_handed: false,
            swagger: [9.0; 6],
            stuck: [9.0; 6],
            feet: [Vec3::ZERO; 6],
            pops: Vec::new(),
            seen: HashMap::new(),
            flyers: Vec::new(),
        }
    }

    /// The local player just issued `cmd`: react this very frame (the sim
    /// applies it after its wind-up).
    pub fn anticipate(&mut self, cmd: Command) {
        self.anticip = Some((cmd, 0.0));
    }

    /// The local ghost dropped something: play the release animation.
    pub fn released(&mut self) {
        self.release = 0.0;
    }

    /// Build this frame's scene from `view`. `reduced_shake` scales the shake.
    pub fn build(&mut self, view: &FlockView, dt: f32, aspect: f32, reduced_shake: bool) {
        let w = &view.world;
        let tun = w.tuning();
        self.time += dt;
        self.signals.clear();
        let anchor = w.anchor_z();
        let tile = TILE_LEN as f64;
        self.origin = (anchor / tile).floor() * tile;
        if !self.started {
            self.last_origin = self.origin;
            self.started = true;
        }
        if self.origin != self.last_origin {
            self.pending_shift += (self.last_origin - self.origin) as f32;
            self.last_origin = self.origin;
        }
        let origin = self.origin;
        let running = w.phase() == Phase::Running;
        let local = view.local;
        let local_ghost = w.player(local).is_some_and(|p| p.role.is_ghost());

        // The ghost view hangs from the far edge of the placement band (derived from the sim's
        // numbers, so it is smooth and does not jump a row at a time); the local ghost floats in the
        // buffer between the runners and the nearest legal row.
        let leader = w.leader_z().unwrap_or(anchor);
        let reach_v = (w.v_course() + tun.jump_fwd_boost) as f64;
        let band_far = leader + tun.band_far_s as f64 * reach_v;
        let band_near = leader + tun.band_near_s as f64 * reach_v;
        let focus = band_far;
        let hover_target = if local_ghost { band_near - 10.0 } else { anchor };
        self.hover_z = if self.hover_init { self.hover_z + (hover_target - self.hover_z) * (1.0 - (-dt / 0.35).exp()) as f64 } else { self.hover_init = true; hover_target };

        // ---- events -> bursts / sounds / signals
        self.scene.bursts.clear();
        let mut shake = 0.0;
        {
            let pos = Pos { world: w, view, origin };
            for e in &view.events {
                self.fx.event(e, local, &pos, &mut self.scene.bursts, &mut self.signals, &mut shake);
            }
        }
        // architect drops: who placed what, and the thud and dust of each landing
        let view_z = if local_ghost { (band_near + band_far) * 0.5 } else { w.runner(local).map_or(anchor, |r| r.pos.z) };
        for e in &view.events {
            match *e {
                Event::Placed { by, obj, .. } => {
                    let n = view.name_of(by);
                    let name = n.map(|n| n.0.trim().to_string()).filter(|n| !n.is_empty());
                    self.placers.insert(obj, Placer { by, colour: n.map_or(by.0, |n| n.1), name });
                }
                Event::Materialised { obj } => {
                    if self.placers.get(&obj).is_some_and(|p| p.by == local) {
                        self.signals.push(Signal::LocalSlam);
                    }
                    shake += self.landed(w, view, obj, origin, view_z)
                }
                Event::Swagger { player, .. } => self.swagger[player.0 as usize % 6] = 0.0,
                Event::MudStuck { player } => self.stuck[player.0 as usize % 6] = 0.0,
                Event::ShieldBroken { player } => self.pops.push((player.0 as usize % 6, 0.0)),
                Event::CoinTaken { obj, by } => self.fly(obj, by, w, dt),
                Event::PowerTaken { obj, by, .. } => self.fly(obj, by, w, dt),
                _ => {}
            }
        }
        for a in self.swagger.iter_mut().chain(self.stuck.iter_mut()) {
            *a = (*a + dt).min(9.0);
        }
        self.fx.countdown(view.countdown, &mut self.signals);
        if shake > 0.0 {
            self.rig.kick(shake);
        }

        // ---- birds
        self.scene.ostriches.clear();
        self.scene.shields.clear();
        self.scene.emitters.clear();
        self.tags.clear();
        let ghosts: Vec<PlayerId> = w.players().iter().filter(|p| p.role.is_ghost()).map(|p| p.id()).collect();
        let anim = self.anticip.map(|(c, a)| (c, a + dt));
        self.anticip = anim.filter(|(_, a)| *a < ANTICIPATE);
        self.release += dt;
        let mut local_pos = (0.0f32, 0.0f32);
        for p in w.players() {
            let id = p.id();
            let i = id.0 as usize % 6;
            let colour = view.name_of(id).map_or(p.spec.colour, |n| n.1);
            match p.role {
                Role::Disconnected => continue,
                Role::Runner => {
                    let r = &p.runner;
                    let idx = self.scene.ostriches.len();
                    let is_local = id == local;
                    let pose = self.runner_pose(i, colour, view.hat_of(id), r, w, dt, running, is_local, anim);
                    if is_local {
                        local_pos = (r.pos.x, pose.speed);
                    }
                    let grounded = pose.pos.y < 0.05 && !pose.airborne;
                    if grounded && w.phase() != Phase::Countdown && pose.speed > 1.0 {
                        let dust = if r.loco.is_low() { 1.0 } else { (pose.speed / 14.0).clamp(0.2, 1.0) * 0.5 };
                        self.scene.emitters.push(Emitter {
                            racer: idx,
                            dust,
                            dust_color: TRACK_DUST,
                            drift_tier: 0,
                            drifting: false,
                            wheel: 0.0,
                            offroad: r.in_mud,
                            boost: false,
                            burning: false,
                        });
                    }
                    let frac = r.protected_until.map(|u| (u.saturating_sub(w.tick()) as f32 / (tun.protect_time / DT)).clamp(0.0, 1.0));
                    self.feet[i] = pose.pos;
                    if r.shield(w.tick()) {
                        let wobble = 1.0 + 0.03 * (self.time * 5.0 + i as f32).sin();
                        self.scene.shields.push(Shield { pos: pose.pos + Vec3::Y * 1.3, radius: BUBBLE_R * wobble, alpha: 0.30, aura: false });
                    }
                    self.tags.push(BirdTag { id, head: pose.pos + Vec3::new(0.0, 2.9, 0.0), feet: pose.pos, protected: frac, ghost: false, local: is_local, body: (tun.body_radius, r.height(tun)) });
                    self.scene.ostriches.push(pose);
                }
                Role::Ghost { .. } => {
                    let gi = ghosts.iter().position(|g| *g == id).unwrap_or(0);
                    let pose = self.ghost_pose(i, colour, view.hat_of(id), p.runner.pos.x, p.runner.pos.y, p.runner.pos.z, gi, ghosts.len(), self.hover_z, dt, id == local);
                    self.tags.push(BirdTag { id, head: pose.pos + Vec3::new(0.0, 2.0, 0.0), feet: pose.pos, protected: None, ghost: true, local: id == local, body: (0.0, 0.0) });
                    self.scene.ostriches.push(pose);
                }
            }
        }

        // ---- the course
        self.build_props(w, view, anchor, origin);
        self.fly_in(w, dt, origin);
        for (i, age) in self.pops.iter_mut() {
            *age += dt;
            let k = (*age / POP_SECS).min(1.0);
            self.scene.shields.push(Shield { pos: self.feet[*i] + Vec3::Y * 1.3, radius: BUBBLE_R * (1.0 + 0.9 * k), alpha: 0.5 * (1.0 - k), aura: false });
        }
        self.pops.retain(|(_, a)| *a < POP_SECS);

        // ---- camera
        // speed feel (solo only): the view widens from course_v0 to course_vmax and eases back on a crash
        let fov_target = match w.runner(local) {
            Some(r) if view.mode == flock_core::Mode::Solo && running && !local_ghost => {
                let span = (tun.course_vmax - tun.course_v0).max(0.1);
                ((r.forward_speed(w.v_course()) - tun.course_v0) / span).clamp(0.0, 1.0) * FOV_BOOST_DEG.to_radians()
            }
            _ => 0.0,
        };
        smooth_to(&mut self.fov_extra, fov_target, dt, FOV_TAU);
        self.rig.left_handed = self.left_handed;
        self.rig.update(dt, anchor, focus, origin, local_pos.0, local_ghost, aspect, self.fov_extra);
        self.scene.camera = self.rig.camera;
        self.scene.time = self.time;
        // The architect view is a long lens far from the pack: thin the fog so the runners stay readable.
        self.scene.fog_scale = RUN_FOG + (0.18 - RUN_FOG) * self.rig.glide();
        self.scene.shake = self.rig.shake_offset(self.time, if reduced_shake { 0.18 } else { 1.0 });

        // ---- footsteps and wind
        if let Some(r) = w.runner(local) {
            self.fx.loops(dt, r.forward_speed(w.v_course()), r.grounded(), running && r.loco == Loco::Running, tun.course_vmax);
        } else {
            self.fx.loops(dt, 0.0, false, false, tun.course_vmax);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn runner_pose(&mut self, i: usize, colour: u8, hat: u8, r: &Runner, w: &World, dt: f32, running: bool, is_local: bool, anim: Option<(Command, f32)>) -> OstrichPose {
        let vc = w.v_course();
        // smoothed body with feed-forward
        {
            let b = &mut self.body[i];
            if !b.init || (b.z - r.pos.z).abs() > 3.0 || (b.x - r.pos.x).abs() > 3.0 {
                *b = Body { x: r.pos.x, y: r.pos.y, z: r.pos.z, init: true };
            } else if dt > 0.0 {
                if running {
                    b.x += r.vel.x * dt;
                    b.y += r.vel.y * dt;
                    b.z += r.forward_speed(vc) as f64 * dt as f64;
                }
                let k = 1.0 - (-dt / 0.05).exp();
                b.x += (r.pos.x - b.x) * k;
                b.y += (r.pos.y - b.y) * k;
                b.z += (r.pos.z - b.z) * k as f64;
                if r.pos.y <= 1e-3 && b.y < 0.0 {
                    b.y = 0.0;
                }
                b.y = b.y.max(0.0);
            }
        }
        let b = self.body[i];
        self.flops[i].step(r, dt);
        let f = &self.flops[i];
        let e = &mut self.eased[i];
        let speed = if running { r.forward_speed(vc).max(0.0) } else { 0.0 };
        e.gait = (e.gait + dt * speed / (1.8 + 4.2 * (speed / 32.0).clamp(0.0, 1.0))).fract();
        // Crouch = tuck and roll: curl into a ball on Collapsed, somersault by distance / radius
        // while it lasts, finish the turn and unfurl as the bird stands up (Recovering).
        let rolling = matches!(r.loco, Loco::Collapsed { .. });
        let prev_roll = e.roll;
        if rolling {
            smooth_to(&mut e.ball, 1.0, dt, 0.05);
            e.roll += speed.max(2.0) * dt / BALL_RADIUS;
        } else {
            smooth_to(&mut e.ball, 0.0, dt, 0.07);
            if e.roll != 0.0 {
                let end = (e.roll / TAU).ceil() * TAU;
                e.roll += (end - e.roll) * (1.0 - (-dt / 0.06).exp());
                if e.ball < 0.03 {
                    e.roll = 0.0;
                }
            }
        }
        if e.ball < 0.001 && !rolling {
            e.ball = 0.0;
        }
        // A little dust each time the ball thumps down (twice a turn).
        let thump = rolling && (e.roll / PI).floor() != (prev_roll / PI).floor();
        smooth_to(&mut e.stars, if r.protected() { 1.0 } else { 0.0 }, dt, 0.12);
        e.ghost = 0.0;
        e.goff_init = false;

        let airborne = b.y > 0.03 || matches!(r.loco, Loco::Airborne { .. });
        let voluntary = matches!(r.loco, Loco::Airborne { voluntary: true, .. });
        let lateral = r.vel.x;
        let mut pose = OstrichPose::for_player_hat(colour, hat);
        pose.pos = to_render(b.x, b.y, b.z, self.origin);
        pose.yaw = (-lateral * 0.03).clamp(-0.3, 0.3);
        pose.speed = speed;
        pose.gait_phase = e.gait;
        pose.squash = ((1.0 - f.squash) / 0.4).clamp(0.0, 1.0) * (1.0 - e.ball);
        pose.neck_sway = Vec2::new(f.neck_sway, f.neck_pitch);
        pose.body_roll = f.body_roll;
        pose.body_pitch = f.body_pitch;
        pose.lean = (lateral * 0.012).clamp(-0.25, 0.25);
        pose.tumble = if matches!(r.loco, Loco::Tumbling { .. }) || f.tumble_angle.abs() > 0.05 { f.tumble_angle } else { 0.0 };
        pose.collapsed = 0.0;
        pose.ball = e.ball;
        pose.roll = e.roll;
        pose.wing_flap = if airborne { 1.0 } else { f.wing_open.clamp(0.0, 1.0) };
        pose.scramble = f.leg_scramble.clamp(0.0, 1.0);
        pose.airborne = airborne && !r.loco.is_low();
        pose.tuck = if voluntary { 0.55 } else { 0.0 };
        pose.stars = e.stars;
        if is_local {
            pose.outline = Some(OUTLINE);
            // immediate local feedback: same-frame squash / head turn
            if let Some((cmd, age)) = anim {
                if matches!(r.loco, Loco::Running | Loco::WindUp { .. } | Loco::Recovering { .. }) {
                    let env = if age < 0.06 { age / 0.06 } else { (1.0 - (age - 0.06) / (ANTICIPATE - 0.06)).max(0.0) };
                    if cmd.is_jump() || cmd.is_down() {
                        pose.squash = pose.squash.max(0.85 * env);
                        pose.neck_sway.y = pose.neck_sway.y.min(-0.25 * env);
                    }
                    let d = cmd.lane_delta() as f32;
                    if d != 0.0 {
                        pose.neck_sway.x += d * 0.7 * env;
                        pose.body_roll += d * 0.2 * env;
                        pose.yaw += -d * 0.22 * env;
                    }
                }
            }
        }
        // The free hit: a comic stagger. The mud: a squelchy bob. Pose only; the sim is untouched.
        let sw = self.swagger[i];
        if sw < SWAGGER_SECS {
            let k = 1.0 - sw / SWAGGER_SECS;
            let ph = sw * 24.0;
            pose.body_roll += 0.6 * k * ph.sin();
            pose.neck_sway.x += 1.4 * k * (ph * 0.8).cos();
            pose.neck_sway.y += 0.4 * k * (ph * 1.3).sin();
            pose.yaw += 0.4 * k * (ph + 1.0).sin();
            pose.squash = pose.squash.max(0.35 * k * (0.5 + 0.5 * (ph * 2.0).sin()));
            pose.wing_flap = pose.wing_flap.max(0.9 * k);
        }
        let st = self.stuck[i];
        if st < STUCK_SECS {
            let k = (st / STUCK_SECS * PI).sin();
            pose.squash = pose.squash.max(0.7 * k);
            pose.neck_sway.y -= 0.35 * k;
            pose.pos.y -= 0.07 * k;
        }
        if thump && !airborne {
            self.scene.bursts.push(Burst::new(BurstKind::DustTrail, pose.pos + Vec3::new(0.0, 0.0, -0.3), 5));
        }
        pose
    }

    #[allow(clippy::too_many_arguments)]
    fn ghost_pose(&mut self, i: usize, colour: u8, hat: u8, fx: f32, fy: f32, fz: f64, gi: usize, gn: usize, anchor: f64, dt: f32, is_local: bool) -> OstrichPose {
        let t = self.time;
        let e = &mut self.eased[i];
        if !e.goff_init {
            // start where the runner was (its frozen body), relative to the anchor
            e.goff = Vec3::new(fx, fy, (fz - anchor) as f32);
            e.goff_init = true;
        }
        let target = Vec3::new(
            (gi as f32 - (gn as f32 - 1.0) * 0.5) * 1.7,
            (if is_local { 3.4 } else { 1.8 }) + 0.22 * (t * 1.9 + gi as f32 * 1.7).sin(),
            if is_local { 0.5 } else { 2.0 + 2.2 * (gi % 2) as f32 },
        );
        let k = 1.0 - (-dt / 0.3).exp();
        e.goff += (target - e.goff) * k;
        smooth_to(&mut e.ghost, 1.0, dt, 0.16);
        e.ghost = e.ghost.min(1.0);
        let mut pose = OstrichPose::for_player_hat(colour, hat);
        pose.pos = to_render(e.goff.x, e.goff.y, anchor + e.goff.z as f64, self.origin);
        pose.ghost = e.ghost;
        pose.speed = 0.0;
        pose.neck_sway = Vec2::new(0.25 * (t * 1.3 + gi as f32).sin(), 0.15);
        pose.body_roll = 0.06 * (t * 1.7 + gi as f32).sin();
        pose.wing_flap = 0.3;
        if is_local {
            pose.ghost_fill = OWN_GHOST_FILL;
            pose.ghost_line = OWN_GHOST_LINE;
            if self.release < 0.5 {
                let k = (self.release / 0.5 * std::f32::consts::PI).sin();
                pose.squash = 0.7 * k;
                pose.wing_flap = 1.0;
                pose.neck_sway.y = 0.5 * k;
            }
        }
        pose
    }

    /// An architect object just became solid: dust, a puff in the placer's colour, and the camera
    /// kick (as shake energy, scaled by how far the landing is from where the viewer is looking).
    fn landed(&mut self, w: &World, view: &FlockView, obj: ObjId, origin: f64, view_z: f64) -> f32 {
        let Some(o) = w.course().obj(obj) else { return 0.0 };
        let pos = to_render(if matches!(o.kind, ObjKind::Beam { span: flock_core::BeamSpan::All }) { 0.0 } else { o.x }, 0.0, o.z, origin);
        let rgb = placer_of(&self.placers, view, &o).map(|p| PlayerStyle::of(p.colour).body);
        let base = match o.kind {
            ObjKind::Hay => {
                self.scene.bursts.push(Burst::new(BurstKind::LandingDust, pos, 26));
                KICK_HAY
            }
            ObjKind::Beam { .. } => {
                self.scene.bursts.push(Burst::new(BurstKind::LandingDust, pos + Vec3::Y * 0.1, 30));
                KICK_BEAM
            }
            ObjKind::Mud => {
                self.scene.bursts.push(Burst::new(BurstKind::MudSplat, pos + Vec3::Y * 0.1, 26));
                KICK_MUD
            }
            ObjKind::Coin => {
                let mut b = Burst::new(BurstKind::CoinSparkle, to_render(o.x, o.y, o.z, origin), 8);
                if let Some(c) = rgb {
                    b = b.with_color(c);
                }
                self.scene.bursts.push(b);
                0.0
            }
            _ => 0.0,
        };
        if let (Some(c), true) = (rgb, matches!(o.kind, ObjKind::Hay | ObjKind::Beam { .. })) {
            self.scene.bursts.push(Burst::new(BurstKind::GhostPoof, pos + Vec3::Y * 0.5, 9).with_color(c));
        }
        base / (1.0 + (o.z - view_z).abs() as f32 / 10.0)
    }

    fn build_props(&mut self, w: &World, view: &FlockView, anchor: f64, origin: f64) {
        self.scene.farm_props.clear();
        self.scene.coins.clear();
        self.scene.powerups.clear();
        self.seen.clear();
        self.scene.drops.clear();
        self.banners.clear();
        if self.barrel_dust.len() > 64 {
            self.barrel_dust.clear();
        }
        let tun = w.tuning();
        let now = w.tick();
        let mat = (tun.materialise_s / DT).max(1.0);
        let t = self.time;
        for o in w.course().objects_in(anchor - 8.0, anchor + 140.0) {
            if !matches!(o.kind, ObjKind::Coin) && o.z1() < anchor - 6.5 {
                continue;
            }
            let pos = to_render(o.x, 0.0, o.z, origin);
            let live = o.is_live(now);
            // Architect drops: fall from DROP_HEIGHT timed to land exactly at `solid_at`, then
            // squash and settle; the sim's non-solid reservation is the fall itself.
            let (drop, settle) = if live {
                (1.0, if o.by.is_some() { ((now - o.solid_at) as f32 * DT / SETTLE_SECS).clamp(0.0, 1.0) } else { 1.0 })
            } else {
                let left = (o.solid_at - now) as f32;
                ((1.0 - left / mat).clamp(0.0, 1.0), 0.0)
            };
            let placer = if o.by.is_some() || self.placers.contains_key(&o.id) { placer_of(&self.placers, view, &o) } else { None };
            let rgb = placer.as_ref().map(|p| PlayerStyle::of(p.colour).body);
            let ring = |scene: &mut FrameScene, centre: Vec3, size: Vec2| {
                if !live {
                    scene.drops.push(DropMark { pos: centre, size, colour: rgb.unwrap_or([0.92, 0.92, 0.92]), t: drop });
                }
            };
            let mut inst = match o.kind {
                ObjKind::Coin => {
                    if o.z < anchor - 4.5 {
                        continue;
                    }
                    let mut y = o.y;
                    let mut visible = true;
                    if !live {
                        // coins rain in one at a time, the last landing exactly at `solid_at`
                        let cz = o.row as f64 * tun.row_spacing as f64;
                        let k = (((o.z - cz) / tun.hazards.lure_spacing as f64).round() as i32 + 2).clamp(0, 4);
                        let p = ((drop - 0.1 * k as f32) / 0.6).clamp(0.0, 1.0);
                        y += DROP_HEIGHT * (1.0 - p * p);
                        visible = p > 0.0;
                        if k == 2 {
                            ring(&mut self.scene, to_render(o.x, 0.0, cz, origin), Vec2::new(tun.lane_width - 0.6, tun.hazards.lure_spacing * 5.0));
                        }
                    }
                    self.seen.insert(o.id.0, (None, o.x, o.y, o.z));
                    self.scene.coins.push(CoinInst { pos: to_render(o.x, y, o.z, origin), spin: t * 3.0 + o.z as f32 * 0.4, visible });
                    continue;
                }
                ObjKind::Power { kind } => {
                    if o.z < anchor - 4.5 {
                        continue;
                    }
                    self.seen.insert(o.id.0, (Some(kind), o.x, o.y, o.z));
                    self.scene.powerups.push(PowerInst { pos: to_render(o.x, o.y, o.z, origin), shape: shape_of(kind), spin: (o.id.0 % 7) as f32 * 0.9, scale: 1.0 });
                    continue;
                }
                ObjKind::Hay => FarmPropInst::new(FarmPropKind::HayBale, pos, Vec3::new(o.half_w * 2.0, o.top, o.half_d * 2.0)),
                ObjKind::Beam { span } => {
                    let lanes = if matches!(span, flock_core::BeamSpan::All) { 3 } else { 1 };
                    let p = if lanes == 3 { Vec3::new(0.0, 0.0, pos.z) } else { pos };
                    // size.y = the underside (clearance); the striped bar runs up to the sim's top.
                    let mut b = FarmPropInst::new(FarmPropKind::Beam { span_lanes: lanes }, p, Vec3::new(o.half_w * 2.0, o.y, o.half_d * 2.0));
                    b.bar_top = o.top;
                    b
                }
                ObjKind::Barrel => {
                    // A lying cylinder: width across the lane, diameter = top; the sim's roll angle turns it.
                    let mut b = FarmPropInst::new(FarmPropKind::Barrel, pos, Vec3::new(o.half_w * 2.0, o.top, o.half_d * 2.0));
                    b.roll = o.roll;
                    if live && o.vz != 0.0 {
                        // dust puffs behind it, one per 1.2 m rolled
                        let cell = (o.z / 1.2).floor() as i64;
                        let seen = self.barrel_dust.insert(o.id.0, cell);
                        if seen.is_some_and(|c| c != cell) {
                            let behind = -(o.dir.signum() as f32) * o.half_d;
                            self.scene.bursts.push(Burst::new(BurstKind::DustTrail, pos + Vec3::new(0.0, 0.0, behind), 4));
                        }
                    }
                    b
                }
                ObjKind::Block => FarmPropInst::new(FarmPropKind::Block { variant: block_variant(&o) }, pos, Vec3::new(o.half_w * 2.0, o.top, o.half_d * 2.0)),
                ObjKind::Mud => FarmPropInst::new(FarmPropKind::Mud, pos, Vec3::new(o.half_w * 2.0, 0.1, o.half_d * 2.0)),
            };
            inst.drop = drop;
            inst.settle = settle;
            ring(&mut self.scene, Vec3::new(inst.pos.x, 0.0, inst.pos.z), Vec2::new(inst.size.x.max(1.4) + 0.3, inst.size.z.max(1.2) + 0.5));
            if let (Some(c), true) = (rgb, matches!(o.kind, ObjKind::Hay | ObjKind::Beam { .. })) {
                inst.placer = Some(c);
                if let (Some(name), Some((centre, size))) = (placer.as_ref().and_then(|p| p.name.clone()), flock_render::banner_anchor(&inst)) {
                    self.banners.push(BannerTag { centre, size, name, colour: placer.as_ref().map_or(0, |p| p.colour) });
                }
            }
            self.scene.farm_props.push(inst);
        }
        // forget placers the course has released
        if !self.placers.is_empty() {
            let course = w.course();
            self.placers.retain(|id, _| course.obj(*id).is_some());
        }
    }

    /// The sim took coin/power-up `obj` for `by`: when `by` has the magnet and the item was a
    /// little way off, let it fly the last bit to the bird (the item itself is already gone).
    fn fly(&mut self, obj: ObjId, by: PlayerId, w: &World, _dt: f32) {
        let Some(&(power, x, y, z)) = self.seen.get(&obj.0) else { return };
        let Some(r) = w.runner(by) else { return };
        if power.is_none() && !r.magnet(w.tick()) {
            return;
        }
        let d = ((r.pos.x - x).powi(2) as f64 + (r.pos.z - z).powi(2)).sqrt();
        if d < 0.6 {
            return;
        }
        if self.flyers.len() < 24 {
            self.flyers.push(Flyer { from: (x, y, z), by, power, age: 0.0 });
        }
    }

    /// Draw the items in flight to their bird.
    fn fly_in(&mut self, w: &World, dt: f32, origin: f64) {
        for f in self.flyers.iter_mut() {
            f.age += dt;
            let secs = if f.power.is_some() { FLY_SECS * 1.6 } else { FLY_SECS };
            let k = (f.age / secs).clamp(0.0, 1.0);
            let e = k * k * (3.0 - 2.0 * k);
            let Some(r) = w.runner(f.by) else { continue };
            let from = to_render(f.from.0, f.from.1, f.from.2, origin);
            let to = to_render(r.pos.x, r.pos.y + 1.1, r.pos.z, origin);
            let pos = from.lerp(to, e);
            match f.power {
                None => self.scene.coins.push(CoinInst { pos, spin: self.time * 14.0, visible: true }),
                Some(kind) => self.scene.powerups.push(PowerInst { pos, shape: shape_of(kind), spin: self.time * 6.0, scale: 1.0 - 0.75 * e }),
            }
        }
        self.flyers.retain(|f| f.age < if f.power.is_some() { FLY_SECS * 1.6 } else { FLY_SECS });
    }

    /// Append the ghost layer: the grid cells and a preview prop.
    pub fn add_ghost_layer(&mut self, cells: &[PlacementCell], preview: Option<FarmPropInst>, lure: &[CoinInst]) {
        self.scene.placement.clear();
        self.scene.placement.extend_from_slice(cells);
        if let Some(p) = preview {
            self.scene.farm_props.push(p);
        }
        self.scene.coins.extend_from_slice(lure);
    }

    /// Drop the ghost layer (not a ghost).
    pub fn clear_ghost_layer(&mut self) {
        self.scene.placement.clear();
    }

    /// A one-shot burst from outside (the Run screen's own effects).
    pub fn burst(&mut self, b: Burst) {
        self.scene.bursts.push(b);
    }

}

fn block_variant(o: &Obj) -> BlockVariant {
    match (o.id.0 >> 3) % 3 {
        0 => BlockVariant::BarnWall,
        1 => BlockVariant::Tractor,
        _ => BlockVariant::Trough,
    }
}

/// Who placed `o`: the client-side map (from `Placed`), else the object's own `by` and the name
/// list, else nobody (an unmarked object; never a panic).
fn placer_of(placers: &HashMap<ObjId, Placer>, view: &FlockView, o: &Obj) -> Option<Placer> {
    if let Some(p) = placers.get(&o.id) {
        return Some(p.clone());
    }
    let by = o.by?;
    let n = view.name_of(by);
    Some(Placer { by, colour: n.map_or(by.0, |n| n.1), name: n.map(|n| n.0.trim().to_string()).filter(|n| !n.is_empty()) })
}

/// Style helper for the HUD: a player's body colour.
pub fn body_colour(colour: u8) -> [f32; 3] {
    PlayerStyle::of(colour).body
}

/// Cell state for the placement grid from a validity check.
pub fn cell_state(valid: bool, reserved: bool, selected: bool) -> CellState {
    if selected {
        CellState::Selected
    } else if reserved {
        CellState::Reserved
    } else if valid {
        CellState::Valid
    } else {
        CellState::Invalid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flock_core::{BotSkill, InputFrame, MatchConfig};
    use flock_session::{LocalRun, RunDriver};

    fn run_for(r: &mut LocalRun, ms: u64, mut f: impl FnMut(&FlockView)) {
        let mut t = 0;
        while t < ms {
            t += 16;
            r.update(t, InputFrame::default());
            f(r.view().unwrap());
        }
    }

    #[test]
    fn x_is_mirrored_in_one_place_and_the_local_bird_is_outlined() {
        let mut r = LocalRun::practice(3, 3, BotSkill::NORMAL);
        let mut p = Presenter::new();
        run_for(&mut r, 4500, |v| p.build(v, 1.0 / 60.0, 0.45, false));
        let v = r.view().unwrap();
        let me = v.world.runner(PlayerId(0)).unwrap();
        let pose = &p.scene.ostriches[0];
        assert!((pose.pos.x + me.pos.x).abs() < 1.0, "render x is minus sim x (smoothed): {} vs {}", pose.pos.x, me.pos.x);
        assert!(pose.outline.is_some(), "the local bird gets the outline");
        assert!(p.scene.ostriches[1..].iter().all(|o| o.outline.is_none()));
        assert_eq!(p.scene.ostriches.len(), 4);
    }

    #[test]
    fn local_anticipation_reacts_in_the_same_frame() {
        let mut r = LocalRun::solo(2);
        let mut p = Presenter::new();
        run_for(&mut r, 3600, |v| p.build(v, 1.0 / 60.0, 0.45, false));
        let before = p.scene.ostriches[0].squash;
        p.anticipate(Command::Jump);
        let v = r.view().unwrap();
        p.build(v, 1.0 / 60.0, 0.45, false);
        assert!(p.scene.ostriches[0].squash > before + 0.1, "squash {} -> {}", before, p.scene.ostriches[0].squash);
        p.anticipate(Command::Right);
        p.build(v, 1.0 / 60.0, 0.45, false);
        assert!(p.scene.ostriches[0].neck_sway.x > 0.1, "head turns toward the lane");
    }

    #[test]
    fn floating_origin_steps_by_whole_tiles_and_shifts_particles() {
        let mut cfg = MatchConfig::solo(4);
        cfg.tuning.debug_empty_course = true;
        let mut r = LocalRun::new(cfg, PlayerId(0));
        let mut p = Presenter::new();
        let mut shifts = 0.0f32;
        let mut origins = std::collections::BTreeSet::new();
        run_for(&mut r, 30_000, |v| {
            p.build(v, 1.0 / 60.0, 0.45, false);
            shifts += std::mem::take(&mut p.pending_shift);
            origins.insert(p.origin as i64);
            assert!((p.scene.camera.eye.z).abs() < 2.0 * TILE_LEN, "camera z stays small: {}", p.scene.camera.eye.z);
        });
        assert!(origins.len() >= 2, "the origin moved: {origins:?}");
        assert!(origins.iter().all(|o| (*o as f64 % TILE_LEN as f64) == 0.0));
        assert!((shifts + (origins.iter().max().unwrap() - origins.iter().min().unwrap()) as f32).abs() < 1e-3, "shifts {shifts}");
    }

    #[test]
    fn eliminated_player_becomes_a_ghost_pose_and_the_camera_glides() {
        let mut r = LocalRun::practice(5, 2, BotSkill::NORMAL);
        let mut p = Presenter::new();
        run_for(&mut r, 4500, |v| p.build(v, 1.0 / 60.0, 0.45, false));
        assert!(p.scene.ostriches[0].ghost == 0.0);
        let eye_before = p.scene.camera.eye;
        r.world_mut().force_eliminate(PlayerId(0));
        let mut t = 4500;
        let mut mid = None;
        for k in 0..90 {
            t += 16;
            r.update(t, InputFrame::default());
            p.build(r.view().unwrap(), 1.0 / 60.0, 0.45, false);
            if k == 25 {
                mid = Some(p.rig.glide());
            }
        }
        assert!(p.scene.ostriches[0].ghost > 0.9, "ghost pose {}", p.scene.ostriches[0].ghost);
        let m = mid.unwrap();
        assert!(m > 0.05 && m < 0.95, "mid-glide at 0.4 s: {m}");
        assert!(p.scene.camera.eye.y > eye_before.y + 5.0, "camera rose to the ghost view");
        assert!(r.view().unwrap().world.player(PlayerId(0)).unwrap().role.is_ghost());
    }

    /// A ghost drops a bale: it falls from 12 m and is down exactly when the sim makes it solid,
    /// squashes and settles, wears its placer's colour, and a presenter that never saw the `Placed`
    /// event (a late joiner) still marks it from the object's own `by`, and copes without names.
    #[test]
    fn architect_drops_land_at_solid_at_carry_their_placer_and_survive_a_late_join() {
        use flock_core::{PlaceRequest, Tool};
        let mut cfg = MatchConfig::multi(3, 3, Some(flock_core::BotSkill::NORMAL));
        cfg.players[1].name = "Marigold".into();
        let mut r = LocalRun::new(cfg, PlayerId(0));
        r.world_mut().force_eliminate(PlayerId(1));
        let mut p = Presenter::new();
        let mut t = 0u64;
        let mut placed = None;
        let mut late = Presenter::new();
        let (mut max_lift, mut saw_ring, mut landed_at, mut min_settle_after) = (0.0f32, false, None, 1.0f32);
        let mut late_marked = false;
        while t < 20_000 {
            t += 16;
            r.update(t, InputFrame::default());
            let v = r.view().unwrap();
            if placed.is_none() && v.phase == Phase::Running && !v.world.placement_band().is_empty() {
                let w = r.world_mut();
                w.debug_set_energy(PlayerId(1), 10.0);
                let row = w.placement_band().first + 2;
                let res = w.place(PlaceRequest { player: PlayerId(1), req: 1, tool: Tool::Hay, lane: 0, row });
                placed = res.ok().map(|a| (a.obj, w.course().obj(a.obj).unwrap().solid_at));
                assert!(placed.is_some(), "the drop was accepted: {res:?}");
                continue;
            }
            let v = r.view().unwrap();
            p.build(v, 1.0 / 60.0, 0.45, false);
            if placed.is_some() && t % 64 == 0 {
                late.build(v, 1.0 / 60.0, 0.45, false); // only sees the bale mid-air, never the event
            }
            let Some((obj, solid_at)) = placed else { continue };
            let now = v.world.tick();
            let Some(o) = v.world.course().obj(obj) else { break };
            let mine: Vec<_> = p.scene.farm_props.iter().filter(|f| matches!(f.kind, FarmPropKind::HayBale) && f.placer.is_some() && (f.pos.z as f64 + p.origin - o.z).abs() < 0.01).collect();
            if mine.is_empty() {
                continue;
            }
            let f = mine[0];
            max_lift = max_lift.max(flock_render::DROP_HEIGHT * (1.0 - f.drop * f.drop));
            if now < solid_at {
                assert!(f.drop < 1.0 && f.settle == 0.0, "still falling at tick {now} of {solid_at}");
                saw_ring |= p.scene.drops.iter().any(|d| (d.pos.z as f64 + p.origin - o.z).abs() < 0.01);
            } else {
                assert_eq!(f.drop, 1.0, "down when solid");
                landed_at.get_or_insert(now);
                min_settle_after = min_settle_after.min(f.settle);
                assert!(p.scene.drops.iter().all(|d| (d.pos.z as f64 + p.origin - o.z).abs() > 0.01), "no ring once it has landed");
            }
            if late.scene.farm_props.iter().any(|f| f.placer.is_some()) {
                late_marked = true;
            }
            if now > solid_at + 40 {
                break;
            }
        }
        let (obj, solid_at) = placed.unwrap();
        assert_eq!(landed_at, Some(solid_at), "lands on the very tick it turns solid");
        assert!(max_lift > 6.0 && saw_ring && min_settle_after < 0.1, "fell from height {max_lift}, ring {saw_ring}, settle {min_settle_after}");
        assert!(p.placers.get(&obj).is_some_and(|x| x.name.as_deref() == Some("Marigold") && x.by == PlayerId(1)), "the placer was kept from the Placed event");
        assert!(late.placers.is_empty() && late_marked, "a late joiner marks the bale from the object's `by`");
        // without any names: tinted by the player's slot, no name, no panic
        let mut v2 = flock_session::FlockView::new(r.world().clone(), PlayerId(0), false);
        v2.names.clear();
        let o = v2.world.course().obj(obj).unwrap();
        let pl = placer_of(&HashMap::new(), &v2, &o).expect("still marked by `by`");
        assert!(pl.name.is_none() && pl.by == PlayerId(1));
        let mut generated = o;
        generated.by = None;
        assert!(placer_of(&HashMap::new(), &v2, &generated).is_none(), "unknown placer: no mark, no crash");
        // released objects are forgotten
        for _ in 0..4000 {
            t += 16;
            r.update(t, InputFrame::default());
            p.build(r.view().unwrap(), 1.0 / 60.0, 0.45, false);
            if r.view().unwrap().world.phase() == Phase::Over {
                break;
            }
        }
        let course = r.world().course();
        assert!(p.placers.keys().all(|id| course.obj(*id).is_some()), "only objects the course still holds are remembered");
        assert_eq!(p.placers.contains_key(&obj), course.obj(obj).is_some(), "ours goes when the course lets it go");
    }

    #[test]
    fn scene_buffers_are_reused() {
        let mut r = LocalRun::solo(9);
        let mut p = Presenter::new();
        run_for(&mut r, 5000, |v| p.build(v, 1.0 / 60.0, 0.45, false));
        let cap = (p.scene.ostriches.capacity(), p.scene.farm_props.capacity(), p.scene.coins.capacity());
        run_for(&mut r, 500, |v| p.build(v, 1.0 / 60.0, 0.45, false));
        assert!(p.scene.ostriches.capacity() <= cap.0 + 1 && p.scene.farm_props.capacity() <= cap.1 * 2 + 8 && p.scene.coins.capacity() <= cap.2 * 2 + 8);
    }
}
