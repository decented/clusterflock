//! The ostrich: procedural part meshes plus the articulated rig that places
//! them from an [`OstrichPose`] (run cycle from `gait_phase`, neck curve with
//! the 180 degree look-back, bash swing, drift lean, tuck, flip).
//!
//! Rig-local frame: +Z forward, +Y up, +X is the racer's LEFT (the racer's
//! right is -X, SPEC section 3), origin on the ground under the body.

use crate::mesh::{basis, limb, MeshData, Style};
use crate::meshes::MeshId;
use crate::types::*;
use glam::{Mat3, Mat4, Vec3};
use std::f32::consts::{PI, TAU};

const L_THIGH: f32 = 0.52;
const L_SHIN: f32 = 0.66;
const NECK_SEGS: usize = 6;
const NECK_LEN: f32 = 0.2;
const HIP_X: f32 = 0.21;
const BODY_UP: f32 = 0.30;
const EYE_LOCAL: Vec3 = Vec3::new(0.15, 0.07, 0.09);
/// Radius of a googly eye.
const EYE_R: f32 = 0.112;
/// Accessories are drawn bigger than they were on the racing birds, to read from behind.
const ACC_S: f32 = 1.6;
/// Big comic feet.
pub(crate) const FOOT_S: f32 = 1.45;
/// Radius (m, at scale 1) of the tucked ball a crouching bird rolls as: the presenter turns it by
/// `distance / BALL_RADIUS`.
pub const BALL_RADIUS: f32 = 0.58;
/// Ball centre height above the hip anchor when tucked (hip `y` that puts the body centre at `BALL_RADIUS`).
const BALL_HIP_Y: f32 = 0.265;
/// Body squash (x, y, z) of the tucked ball.
const BALL_SQUASH: Vec3 = Vec3::new(1.0, 1.05, 0.72);
/// Wings wrapped round the ball: pressed to the flanks and swept forward over the belly.
const BALL_WING_SPREAD: f32 = -0.05;
const BALL_WING_SWEEP: f32 = -0.9;
/// Neck segment tilt from vertical (rad) when curled: forward, down, and back against the chest.
const BALL_NECK: [f32; 6] = [1.5, 2.4, 3.1, 3.7, 4.1, 4.3];

/// One drawable piece of an ostrich.
#[derive(Clone, Copy, Debug)]
pub struct OstrichPart {
    pub mesh: MeshId,
    pub model: Mat4,
    /// Instance tint 0 (the main colour of the part).
    pub tint0: Rgb,
    /// Instance tint 1 (accent / beak, depending on the part).
    pub tint1: Rgb,
}

/// First-person camera frame at the ostrich's eyes.
#[derive(Clone, Copy, Debug)]
pub struct FirstPersonEye {
    pub eye: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Smoothed piecewise-linear lookup (anchors sorted by x).
fn pw(x: f32, pts: &[(f32, f32)]) -> f32 {
    if x <= pts[0].0 {
        return pts[0].1;
    }
    for w in pts.windows(2) {
        if x <= w[1].0 {
            let t = (x - w[0].0) / (w[1].0 - w[0].0);
            return lerp(w[0].1, w[1].1, t * t * (3.0 - 2.0 * t));
        }
    }
    pts[pts.len() - 1].1
}

fn dleg(theta: f32) -> Vec3 {
    Vec3::new(0.0, -theta.cos(), theta.sin())
}

/// Speed at which the legs start to turn into the turbo wheel (m/s), and
/// where the wheel is fully formed. Normal top speed is about 32 m/s.
const TURBO_FROM: f32 = 36.0;
const TURBO_FULL: f32 = 41.0;
/// Radius (vertical) of the foot circle in turbo, and its horizontal stretch.
const WHEEL_R: f32 = 0.50;
const WHEEL_STRETCH: f32 = 1.3;
/// The forced wheel (drift, Laser Trump) turns at least this fast (revolutions
/// per second), so it reads as a blur rather than as running legs.
const WHEEL_MIN_REVS: f32 = 5.0;

/// How many wheel turns per gait cycle. The sim advances `gait_phase` by
/// `speed / stride` cycles a second, with the stride growing from about 1.8 m
/// to 6 m; a whole-number multiple keeps the phase continuous across the wrap
/// while making the forced wheel spin at `WHEEL_MIN_REVS` or more.
fn wheel_turns_per_cycle(speed: f32) -> f32 {
    let cycles_per_s = speed / (1.8 + 4.2 * (speed / 32.0).clamp(0.0, 1.0));
    (WHEEL_MIN_REVS / cycles_per_s.max(0.01))
        .ceil()
        .clamp(1.0, 4.0)
}

/// Speed-dependent gait envelope: stride reach, knee lift, stance share and
/// the steady hip height.
#[derive(Clone, Copy)]
struct Gait {
    /// Half-span of the stance foot sweep (m, ankle fore/aft of the hip).
    r: f32,
    /// Peak ankle lift in the swing (m).
    lift: f32,
    /// Fraction of the cycle a foot is in contact (>= 0.5: no flight phase).
    duty: f32,
    /// Hip height (rig y of the body anchor).
    hip_h: f32,
    /// Amplitude of the subtle low-speed bob (m).
    bob: f32,
    /// 0..1: how much of a "run" the legs are doing (toe-off etc).
    act: f32,
}

fn gait(speed: f32) -> Gait {
    let r = pw(speed, &[(0.0, 0.0), (0.4, 0.0), (3.0, 0.26), (5.0, 0.38), (18.0, 0.60), (32.0, 0.72)]);
    Gait {
        r,
        lift: pw(speed, &[(0.0, 0.0), (0.4, 0.0), (3.0, 0.09), (5.0, 0.17), (18.0, 0.40), (32.0, 0.62)]),
        duty: pw(speed, &[(0.0, 1.0), (0.4, 1.0), (3.0, 0.66), (5.0, 0.60), (18.0, 0.53), (32.0, 0.50)]),
        hip_h: pw(speed, &[(0.0, 1.22), (0.4, 1.22), (3.0, 1.19), (5.0, 1.15), (18.0, 1.05), (32.0, 0.97)]),
        bob: pw(speed, &[(0.0, 0.0), (0.4, 0.0), (3.0, 0.02), (5.0, 0.025), (18.0, 0.012), (28.0, 0.0)]),
        act: (r / 0.38).min(1.0),
    }
}

/// Height of the lowest sole point above the ankle for foot pitch `pf` (m, <= 0).
fn sole(pf: f32) -> f32 {
    let (sp, cp) = pf.sin_cos();
    FOOT_S * (-0.07 * cp - 0.24 * sp).min(-0.07 * cp + 0.06 * sp)
}

/// Everything the gait solve shares between the rig and the turbo effects.
struct Rig {
    sc: f32,
    speed: f32,
    sfp: f32,
    /// 0..1 turbo wheel blend.
    tw: f32,
    /// 0..1 forced wheel (drift, trump), part of `tw`.
    wheel: f32,
    /// Wheel turns per gait cycle (see `wheel_turns_per_cycle`).
    wk: f32,
    g: Gait,
    phi: f32,
    airborne: bool,
    tuck: f32,
    hip_y: f32,
    #[allow(dead_code)]
    body_pitch: f32,
    /// 0..1 belly-flop, wind-up squash, ghost.
    col: f32,
    /// 0..1 tuck-and-roll ball.
    ball: f32,
    sq: f32,
    ghost: f32,
    /// Body frame (hip height, pitch).
    u: Mat4,
    /// Rig-local to world.
    root: Mat4,
}

fn rig(p: &OstrichPose) -> Rig {
    rig_t(p, 0.0)
}

fn rig_t(p: &OstrichPose, time: f32) -> Rig {
    let sc = p.scale.max(0.05);
    let speed = p.speed.max(0.0);
    let sfp = (speed / 32.0).clamp(0.0, 1.0);
    let wheel = if p.trumping {
        1.0
    } else {
        p.wheel.clamp(0.0, 1.0)
    };
    let tw = smooth(TURBO_FROM, TURBO_FULL, speed).max(wheel);
    let wk = if wheel > 0.0 {
        wheel_turns_per_cycle(speed)
    } else {
        1.0
    };
    let phi = p.gait_phase.rem_euclid(1.0);
    let airborne = p.airborne;
    let tuck = if airborne { p.tuck.clamp(0.0, 1.0) } else { p.tuck.clamp(0.0, 1.0) * 0.5 };
    let col = p.collapsed.clamp(0.0, 1.0);
    let ball = p.ball.clamp(0.0, 1.0);
    let sq = p.squash.clamp(0.0, 1.0);
    let ghost = p.ghost.clamp(0.0, 1.0);
    let mut g = gait(speed);
    // Hip height: a steady crouch that deepens with speed (only a faint bob at a jog).
    let bob = g.bob * (phi * TAU * 2.0 + 0.6).sin();
    let mut hip_y = if airborne { 1.0 } else { g.hip_h + 0.20 * tw + bob };
    // Wind-up squash folds the knees; the belly-flop drops the body onto the ground.
    hip_y -= 0.40 * sq * if airborne { 0.4 } else { 1.0 };
    hip_y = lerp(hip_y, 0.30, col);
    hip_y = lerp(hip_y, BALL_HIP_Y, ball);
    // Keep the stance foot reachable from this hip height.
    let drop = hip_y + 0.02 - 0.07;
    let dmax = (L_THIGH + L_SHIN) * 0.995;
    let reach = (dmax * dmax - drop * drop).max(0.0).sqrt();
    g.r = g.r.min(reach * 0.97);
    // The torso glides: a steady forward lean, no gait wobble; turbo leans in.
    let body_pitch = 0.05 * sfp
        + 0.12 * tw
        + 0.10 * p.trumping as u32 as f32
        + 0.004 * speed.min(8.0) / 8.0 * (phi * TAU * 2.0).sin() * (1.0 - sfp)
        + 0.25 * tuck
        + p.body_pitch
        + 0.12 * col;
    let body_pitch = body_pitch * (1.0 - ball);
    let squash = Mat4::from_scale(
        Vec3::new(1.0 + 0.16 * sq + 0.30 * col, 1.0 - 0.30 * sq - 0.40 * col, 1.0 + 0.08 * sq + 0.15 * col).lerp(BALL_SQUASH, ball),
    );
    let u = Mat4::from_translation(Vec3::new(0.0, hip_y, 0.0))
        * Mat4::from_rotation_z(p.body_roll * (1.0 - ball))
        * Mat4::from_rotation_x(body_pitch)
        * squash;

    // Root: scale, flip, tumble roll, pitch, lean, yaw, position.
    let yaw = p.yaw + p.visual_yaw + p.spin;
    let flip_c = Vec3::new(0.0, 1.25 * sc, 0.0);
    let flip = Mat4::from_translation(flip_c)
        * Mat4::from_rotation_x(p.trick_flip.clamp(0.0, 1.0) * TAU)
        * Mat4::from_translation(-flip_c);
    // A rolling bird pivots about its middle and is lifted so nothing digs into the ground.
    let pivot = Vec3::Y * 0.95 * sc;
    let tumble = Mat4::from_translation(Vec3::Y * (0.75 * p.tumble.sin().abs() * sc))
        * Mat4::from_translation(pivot)
        * Mat4::from_rotation_z(p.tumble)
        * Mat4::from_translation(-pivot);
    // The tucked ball somersaults forward about its own centre (`p.roll`, distance / radius).
    let ball_pivot = Vec3::new(0.0, lerp(0.95, BALL_RADIUS, ball), -0.03) * sc;
    let somersault = Mat4::from_translation(ball_pivot) * Mat4::from_rotation_x(p.roll) * Mat4::from_translation(-ball_pivot);
    // Ghosts float above the ground and bob.
    let float = ghost * (0.95 + 0.12 * (time * 2.3).sin()) * sc;
    let root = Mat4::from_translation(p.pos + Vec3::Y * float)
        * Mat4::from_rotation_y(yaw)
        * Mat4::from_rotation_z(p.lean)
        * Mat4::from_rotation_x(p.pitch)
        * tumble
        * somersault
        * crate::riders::ostrich_knock(p)
        * flip
        * Mat4::from_scale(Vec3::splat(sc));
    Rig {
        sc,
        speed,
        sfp,
        tw,
        wheel,
        wk,
        g,
        phi,
        airborne,
        tuck,
        hip_y,
        body_pitch,
        col,
        ball,
        sq,
        ghost,
        u,
        root,
    }
}

impl Rig {
    /// Stride and wheel phase of leg `k`, `lag` wheel turns behind (the ghost legs).
    fn phases(&self, k: usize, lag: f32) -> (f32, f32) {
        let off = 0.5 * k as f32;
        (
            self.phi + off - lag / self.wk,
            self.phi * self.wk + off - lag,
        )
    }
}

/// Two-bone IK in the sagittal plane (knee forward): ankle target relative to
/// the hip as (z fore/aft, y). Returns thigh and shin angles (as `dleg`).
fn ik(az: f32, ay: f32) -> (f32, f32) {
    let dmax = (L_THIGH + L_SHIN) * 0.995;
    let dmin = (L_THIGH - L_SHIN).abs() + 0.03;
    let (mut az, mut ay) = (az, ay);
    let d = (az * az + ay * ay).sqrt();
    if d > dmax {
        // Over-reach: keep the fore/aft reach, lift the ankle (toe-off).
        az = az.clamp(-dmax * 0.999, dmax * 0.999);
        ay = -(dmax * dmax - az * az).max(0.0).sqrt();
    } else if d < dmin {
        let s = dmin / d.max(1e-4);
        az *= s;
        ay *= s;
    }
    let d = (az * az + ay * ay).sqrt();
    let phi = az.atan2(-ay);
    let ca = ((L_THIGH * L_THIGH + d * d - L_SHIN * L_SHIN) / (2.0 * L_THIGH * d)).clamp(-1.0, 1.0);
    let th1 = phi + ca.acos();
    let knee = L_THIGH * dleg(th1);
    let (dz, dy) = (az - knee.z, ay - knee.y);
    let th2 = dz.atan2(-dy);
    (th1, th2)
}

/// Where the foot wants to be at cycle phase `ph`: (ankle z rel. hip, ankle y
/// rel. hip, foot pitch), blending the stride path into the turbo wheel.
fn foot_target(r: &Rig, g: &Gait, ph: f32, wph: f32) -> (f32, f32, f32) {
    let ph = ph.rem_euclid(1.0);
    let (zn, yn, pfn);
    if ph < g.duty {
        // Stance: the foot slides back under the body while planted.
        let u = ph / g.duty;
        zn = g.r * (1.0 - 2.0 * u);
        pfn = g.act * 0.5 * smooth(0.55, 1.0, u);
        yn = -sole(pfn) + 0.05 * g.act * smooth(0.8, 1.0, u);
    } else {
        // Swing: knee drives up and the foot reaches forward past touchdown.
        let u = (ph - g.duty) / (1.0 - g.duty).max(1e-3);
        let e = u * u * (3.0 - 2.0 * u);
        zn = lerp(-g.r, g.r, e) + 0.18 * g.r * (PI * u).sin();
        pfn = g.act * lerp(0.5, 0.05, smooth(0.0, 0.8, u));
        yn = -sole(pfn) + g.lift * (PI * u).sin().max(0.0).powf(0.7);
    }
    let hip_wy = r.hip_y + 0.02;
    let mut z = zn;
    let mut y = yn - hip_wy; // ankle y relative to the hip
    let mut pf = pfn;
    if r.tw > 0.0 {
        // Turbo: the ankle sweeps a full circle whose lowest point touches the ground.
        let a = wph * TAU;
        let pfw = 0.35;
        let zw = -WHEEL_STRETCH * WHEEL_R * a.sin();
        let yw = -sole(pfw) + WHEEL_R * (1.0 - a.cos()) - hip_wy;
        z = lerp(z, zw, r.tw);
        y = lerp(y, yw, r.tw);
        pf = lerp(pf, pfw, r.tw);
    }
    (z, y, pf)
}

/// Thigh, shin and foot angles for cycle phase `ph` with gait `g`, with the jump tuck mixed in.
fn leg_angles_g(r: &Rig, g: &Gait, ph: f32, wph: f32, tuck: f32) -> (f32, f32, f32) {
    let (z, y, pf) = foot_target(r, g, ph, wph);
    let (mut th1, mut th2) = ik(z, y);
    let mut pf = pf;
    let (t1, t2, tf) = (0.95, 0.95 - 1.75, 0.7);
    th1 += (t1 - th1) * tuck;
    th2 += (t2 - th2) * tuck;
    pf += (tf - pf) * tuck;
    (th1, th2, pf)
}

fn leg_angles(r: &Rig, ph: f32, wph: f32) -> (f32, f32, f32) {
    leg_angles_g(r, &r.g, ph, wph, r.tuck)
}

/// Hip, knee, ankle (rig-local) for leg `k` at cycle phase `ph`.
fn leg_chain(r: &Rig, k: usize, ph: f32, wph: f32) -> [Vec3; 3] {
    let side = if k == 0 { 1.0 } else { -1.0 };
    let (th1, th2, _) = leg_angles(r, ph, wph);
    let hip = (r.u * Vec3::new(side * HIP_X, 0.02, 0.02).extend(1.0)).truncate();
    let knee = hip + L_THIGH * dleg(th1);
    let ank = knee + L_SHIN * dleg(th2);
    [hip, knee, ank]
}

struct Solved {
    parts: Vec<OstrichPart>,
    head: Mat4,
    eyes: [Vec3; 2],
    feet: [Vec3; 2],
    fp: FirstPersonEye,
}

fn solve(p: &OstrichPose, time: f32) -> Solved {
    let r = rig_t(p, time);
    let (speed, sfp, tw, phi, airborne, tuck, root, u) = (r.speed, r.sfp, r.tw, r.phi, r.airborne, r.tuck, r.root, r.u);
    let moving = smooth(0.4, 3.5, speed);
    // Gait-synchronous flutter fades out with speed: a steady glide at the top end.
    let calm = 1.0 - 0.9 * sfp;
    let (col, sq, ghost, ball) = (r.col, r.sq, r.ghost, r.ball);
    let flap = p.wing_flap.clamp(0.0, 1.0);
    let scr = p.scramble.clamp(0.0, 1.0);
    let stars = p.stars.clamp(0.0, 1.0);
    let dizzy = stars.max((p.tumble.abs() * 3.0).clamp(0.0, 1.0));

    let mut parts: Vec<OstrichPart> = Vec::with_capacity(48);
    let mut push = |mesh: MeshId, local: Mat4, t0: Rgb, t1: Rgb| {
        parts.push(OstrichPart { mesh, model: root * local, tint0: t0, tint1: t1 });
    };

    // Body, tail, wings.
    push(MeshId::Body, u * Mat4::from_translation(Vec3::new(0.0, BODY_UP, -0.04)), p.body, p.accent);
    let tail_fl = 0.05 * (time * 5.0).sin() + 0.04 * calm * (phi * TAU * 2.0).sin() * moving;
    let tail_up = 0.35 * sq - 0.30 * col + 0.20 * flap * (time * 17.0).sin() + 0.55 * ball;
    push(
        MeshId::Tail,
        u * Mat4::from_translation(Vec3::new(0.0, 0.42, -0.60)) * Mat4::from_rotation_x(-0.22 * sfp + tail_fl - tail_up) * Mat4::from_rotation_y(0.05 * (time * 3.1).sin() + 0.12 * (time * 9.0).sin() * scr),
        p.body,
        p.accent,
    );
    let idle_flap = 0.22 * calm * (phi * TAU * 2.0).sin() * moving + 0.05 * (time * 2.0).sin();
    let base_spread = if airborne { 0.9 + 0.09 * (time * 14.0).sin() } else { 0.07 + 0.08 * sfp + idle_flap };
    // Wings: tucked at a run, thrown wide in a belly-flop, beating when flapping.
    let spread0 = lerp(lerp(base_spread, 1.3, col) + 0.35 * sq, BALL_WING_SPREAD, ball);
    let sweep = lerp(-0.25 * sfp - 0.2 * tw, BALL_WING_SWEEP, ball);
    for (side, mesh) in [(1.0f32, MeshId::WingL), (-1.0, MeshId::WingR)] {
        let ph = side * 0.5;
        let beat = flap * (0.62 * (time * 19.0 + ph).sin() + 0.36);
        let fwd = flap * 0.35 * (time * 19.0 + ph + 1.3).cos();
        push(
            mesh,
            u * Mat4::from_translation(Vec3::new(side * 0.44, 0.40, 0.12))
                * Mat4::from_rotation_z(side * (spread0 + beat))
                * Mat4::from_rotation_x(sweep + fwd),
            p.body,
            p.accent,
        );
    }

    if p.burning {
        for k in 0..3 {
            let wave = (time * 16.0 + k as f32 * 2.1).sin();
            let m = u * Mat4::from_translation(Vec3::new((k as f32 - 1.0) * 0.18, 0.40, -0.74))
                * Mat4::from_rotation_z(wave * 0.14)
                * Mat4::from_scale(Vec3::new(0.8, 0.65 + wave * 0.15, 0.8));
            push(MeshId::Flame, m, [1.0, 0.22, 0.01], [1.0, 0.62, 0.04]);
        }
    }
    // Legs (or the wavy tail of a ghost).
    let mut feet = [Vec3::ZERO; 2];
    if ghost > 0.5 {
        // A wavy tail instead of legs: a tapering tube following a travelling sine wave.
        let segs = 8;
        let pt = |i: usize| {
            let f = i as f32 / segs as f32;
            let wave = (time * 4.5 - i as f32 * 0.8).sin();
            let wave2 = (time * 3.1 - i as f32 * 0.55 + 1.0).cos();
            u.transform_point3(Vec3::new(wave * 0.30 * f.powf(0.8), 0.02 - 0.17 * i as f32, -0.10 - 0.07 * i as f32 + 0.10 * wave2 * f))
        };
        let mut rad = 0.36f32;
        for i in 0..segs {
            let (a, c) = (pt(i), pt(i + 1));
            push(MeshId::NeckSeg, limb(a - (c - a) * 0.25, c + (c - a) * 0.1, rad), p.body, p.body);
            rad *= 0.80;
        }
        feet = [(root * Vec3::new(HIP_X, -1.2, 0.0).extend(1.0)).truncate(), (root * Vec3::new(-HIP_X, -1.2, 0.0).extend(1.0)).truncate()];
    } else {
        for (k, foot) in feet.iter_mut().enumerate() {
            let side = if k == 0 { 1.0 } else { -1.0 };
            let (ph, wph) = r.phases(k, 0.0);
            let (mut th1, mut th2, mut pf) = leg_angles_g(&r, &r.g, ph, wph, tuck);
            if scr > 0.0 {
                // Flailing: a fast, wide, irregular pedalling that ignores the ground.
                let gf = Gait { r: 0.62, lift: 0.55, duty: 0.5, hip_h: r.g.hip_h, bob: 0.0, act: 1.0 };
                let fph = time * 2.7 + k as f32 * 0.5 + 0.2 * (time * 6.1 + k as f32 * 2.0).sin();
                let (a1, a2, af) = leg_angles_g(&r, &gf, fph, fph, tuck * 0.25);
                th1 = lerp(th1, a1, scr);
                th2 = lerp(th2, a2, scr);
                pf = lerp(pf, af, scr);
            }
            let hip = u.transform_point3(Vec3::new(side * HIP_X, 0.02, 0.02));
            let d1n = dleg(th1);
            let d2n = dleg(th2);
            let (mut d1, mut d2) = (d1n, d2n);
            if ball > 0.0 {
                // Tucked: thighs forward and up against the chest, shins folded back under the belly.
                let d1b = Vec3::new(side * 0.20, 0.60, 0.65).normalize();
                let d2b = Vec3::new(-side * 0.15, 0.10, -0.97).normalize();
                d1 = d1n.lerp(d1b, ball).normalize();
                d2 = d2n.lerp(d2b, ball).normalize();
                pf = lerp(pf, 0.1, ball);
            }
            if col > 0.0 {
                // Belly-flop: legs trail out behind, splayed, feet dragging.
                let d1c = Vec3::new(side * 0.30, -0.12, -1.0).normalize();
                let d2c = Vec3::new(side * 0.55, 0.02, -1.0).normalize();
                d1 = d1n.lerp(d1c, col).normalize();
                d2 = d2n.lerp(d2c, col).normalize();
                pf = lerp(pf, 1.15, col);
            }
            let knee = hip + L_THIGH * d1;
            let ank = knee + L_SHIN * d2;
            let splay = Mat4::from_rotation_y(side * 0.35 * col);
            push(MeshId::Thigh, limb(hip, knee, 0.17), p.body, p.legs);
            push(MeshId::Shin, limb(knee, ank, 0.062), p.legs, p.legs);
            push(MeshId::Ball, Mat4::from_translation(knee) * Mat4::from_scale(Vec3::splat(0.095)), p.legs, p.legs);
            push(MeshId::Foot, Mat4::from_translation(ank) * splay * Mat4::from_rotation_x(pf), p.legs, p.legs);
            let sl = sole(pf);
            let gpt = (root * Vec3::new(ank.x, ank.y + sl, ank.z + 0.1).extend(1.0)).truncate();
            *foot = gpt;
        }
    }

    // ---- neck ----
    let lb = p.look_back.clamp(0.0, 1.0);
    let lbe = lb * lb * (3.0 - 2.0 * lb);
    let bash = p.bash.clamp(-1.0, 1.0);
    let babs = bash.abs();
    let base_tilt = [0.45, 0.12, -0.12, -0.2, -0.02, 0.26];
    let sway = p.neck_sway;
    let mut pos = u.transform_point3(Vec3::new(0.0, 0.58, 0.46));
    let mut dirs = [Vec3::Y; NECK_SEGS];
    let mut joints = [Vec3::ZERO; NECK_SEGS + 1];
    joints[0] = pos;
    let mut last_psi = 0.0;
    for i in 0..NECK_SEGS {
        let w = ((i + 1) as f32 / NECK_SEGS as f32).powf(0.8);
        let mut t = base_tilt[i];
        // Neck stretches forward with speed, then holds steady (no gait wobble).
        t += (0.30 + 0.30 * (i as f32 / 5.0)) * sfp + (0.10 + 0.05 * (i as f32 / 5.0)) * tw;
        t += moving * 0.012 * calm * (phi * TAU * 2.0 + 0.5 * i as f32).sin();
        t += 0.55 * lbe * (PI * w).sin();
        t += babs * (0.55 * w + 0.15);
        t -= 0.10 * tuck * w;
        // Belly-flop: the neck stretches out ahead, a little raised at the head.
        t = lerp(t, 1.30 - 0.10 * w, col * smooth(0.0, 0.35, w));
        // Tucked ball: the neck curls forward and down and the head tucks into the chest.
        t = lerp(t, BALL_NECK[i], ball);
        let psi = PI * lbe * w - bash * 1.35 * smooth(0.0, 1.0, w);
        last_psi = psi;
        let psi = psi * (1.0 - ball);
        let mut d = Vec3::new(t.sin() * psi.sin(), t.cos(), t.sin() * psi.cos());
        // Floppy: the presenter's sway grows towards the head; a gentle always-on wobble rides on top.
        let wg = w * w;
        let wob = 0.05 * (time * 3.3 + i as f32 * 0.75).sin() * w + 0.03 * (time * 5.1 + i as f32 * 1.3).cos() * w * scr.max(sq);
        d += Vec3::new(-sway.x * 0.85 * wg + wob, 0.0, sway.y * 0.7 * wg + 0.4 * wob) * (1.0 - 0.85 * ball);
        let d = d.normalize_or_zero();
        dirs[i] = d;
        pos += d * NECK_LEN;
        joints[i + 1] = pos;
    }
    let r_at = |i: usize| 0.135 - 0.06 * (i as f32 / NECK_SEGS as f32);
    for i in 0..NECK_SEGS {
        push(MeshId::NeckSeg, limb(joints[i] - dirs[i] * 0.035, joints[i + 1] + dirs[i] * 0.035, r_at(i)), p.neck, p.neck);
    }

    // Head frame (rig-local).
    // The head is the stable point: kept level, beak slightly down, almost no gait nod.
    let nose_down = 0.14 - 0.08 * sfp + 0.35 * babs + 0.01 * moving * calm * (phi * TAU * 2.0).sin() - 0.30 * col + 0.25 * sq + 0.55 * ball;
    let hpsi = last_psi - sway.x * 0.35;
    let hz = Vec3::new(hpsi.sin() * nose_down.cos(), -nose_down.sin(), hpsi.cos() * nose_down.cos()).normalize();
    let hx = Vec3::Y.cross(hz).normalize();
    let hy = hz.cross(hx);
    let head_pos = joints[NECK_SEGS] + hy * 0.06 + hz * 0.02;
    let head_local = basis(head_pos, hx, hy, hz);
    if !p.hide_head {
        push(MeshId::Head, head_local, p.neck, p.beak);
    }
    push(MeshId::Beak, head_local, p.beak, p.beak);

    // Eyes: big googly eyes on the sides of the head, with lids that carry the expression.
    let eyes_local = [head_local.transform_point3(EYE_LOCAL), head_local.transform_point3(Vec3::new(-EYE_LOCAL.x, EYE_LOCAL.y, EYE_LOCAL.z))];
    if !p.hide_head {
        let mut lid = -0.8 + 0.25 * sfp;
        if airborne {
            lid = -1.45;
        }
        lid = lerp(lid, -1.45, stars * 0.6 + ghost);
        lid = lerp(lid, 0.55, sq);
        lid = lerp(lid, 1.2, col);
        lid = lerp(lid, 1.0, ball);
        let wide = (if airborne { 1.18 } else { 1.0 }) * (1.0 + 0.12 * ghost) * (1.0 - 0.12 * sq);
        for (k, s) in [1.0f32, -1.0].into_iter().enumerate() {
            let ep = Vec3::new(s * EYE_LOCAL.x, EYE_LOCAL.y, EYE_LOCAL.z);
            let ph = time * 8.5 + k as f32 * 1.7;
            let look = Mat4::from_rotation_y(s * 0.32 + dizzy * 0.9 * ph.sin()) * Mat4::from_rotation_x(-0.05 + dizzy * 0.6 * ph.cos());
            let er = EYE_R * wide;
            push(MeshId::Eye, head_local * Mat4::from_translation(ep) * look * Mat4::from_scale(Vec3::splat(er)), [1.0; 3], [1.0; 3]);
            push(
                MeshId::Eyelid,
                head_local * Mat4::from_translation(ep) * Mat4::from_rotation_y(s * 0.32) * Mat4::from_rotation_x(lid) * Mat4::from_scale(Vec3::splat(er * 1.03)),
                p.neck,
                p.neck,
            );
        }
    }
    if p.laser_on && !p.hide_head {
        for e in eyes_local {
            push(MeshId::Glow, Mat4::from_translation(e) * Mat4::from_scale(Vec3::splat(0.085)), [1.0, 0.1, 0.06], [1.0, 0.1, 0.06]);
        }
    }

    // Protection stars circling the head.
    if stars > 0.02 && !p.hide_head {
        let n = 5;
        let ease = 1.0 - (1.0 - stars).powi(3);
        for i in 0..n {
            let a = time * 4.6 + i as f32 * TAU / n as f32;
            let c = head_pos + Vec3::new(a.cos() * 0.46, 0.34 + 0.05 * (time * 7.0 + i as f32 * 2.0).sin(), a.sin() * 0.46);
            let facing = Mat4::from_rotation_y((a.cos()).atan2(a.sin()));
            push(MeshId::Star, Mat4::from_translation(c) * facing * Mat4::from_rotation_z(time * 3.0 + i as f32) * Mat4::from_scale(Vec3::splat(0.17 * ease)), [1.0; 3], [1.0; 3]);
        }
    }

    // ---- accessories ----
    let neck_frame = |i: usize, back: f32| {
        // Frame at joint i, y along the neck direction.
        let d = dirs[i.min(NECK_SEGS - 1)];
        let x = Vec3::Y.cross(d);
        let x = if x.length_squared() < 1e-4 { Vec3::X } else { x.normalize() };
        let z = d.cross(x);
        basis(joints[i] - d * back, x, d, z)
    };
    let big = Mat4::from_scale(Vec3::splat(ACC_S));
    match p.accessory {
        Accessory::None => {}
        Accessory::Scarf => {
            let f = neck_frame(1, 0.0) * Mat4::from_scale(Vec3::new(1.5, 1.5, 1.5));
            push(MeshId::Scarf, f, p.accent, p.accent);
            // Trailing ends stream behind in the neck's own plane.
            let wave = (time * 9.0).sin() * 0.25;
            let lift = 0.25 + 0.5 * sfp;
            let trail = Vec3::new(wave * 0.5, lift, -1.0).normalize();
            let tx = Vec3::Y.cross(-trail).normalize();
            let tm = basis(joints[1] + Vec3::new(0.0, 0.0, -0.12), tx * 1.5, Vec3::Y.cross(tx).normalize() * -1.5, trail * 1.3);
            push(MeshId::ScarfTail, tm, p.accent, p.accent);
        }
        Accessory::ChainShades => {
            push(MeshId::Chain, neck_frame(1, 0.0), p.accent, p.accent);
            if !p.hide_head {
                push(MeshId::Shades, head_local, p.accent, p.accent);
            }
        }
        a if !p.hide_head => {
            let mesh = match a {
                Accessory::Goggles => MeshId::Goggles,
                Accessory::Crown => MeshId::Crown,
                Accessory::StarHeadband => MeshId::StarHeadband,
                Accessory::VikingHelmet => MeshId::VikingHelmet,
                Accessory::Mohawk => MeshId::Mohawk,
                _ => MeshId::Cap,
            };
            push(mesh, head_local * big, p.accent, p.accent);
        }
        _ => {}
    }

    if p.shells {
        for k in 0..3 {
            let o = Vec3::new((k as f32 - 1.0) * 0.09, 0.15, -0.04);
            push(MeshId::Shell, head_local * Mat4::from_translation(o)
                * Mat4::from_rotation_z(k as f32 * 0.4 - 0.4)
                * Mat4::from_scale(Vec3::new(0.1, 0.14, 0.12)), [1.0, 0.96, 0.85], p.accent);
        }
    }
    if p.jockey { parts.extend(crate::riders::jockey(p, time, r.u)); }
    // World-space results.
    let head_world = root * head_local;
    let eyes = [
        (root * eyes_local[0].extend(1.0)).truncate(),
        (root * eyes_local[1].extend(1.0)).truncate(),
    ];
    // First-person eye: just above and behind the skull, looking along the head.
    let fp_eye_local = head_pos + hy * 0.24 - hz * 0.16;
    let fp = FirstPersonEye {
        eye: (root * fp_eye_local.extend(1.0)).truncate(),
        forward: Mat3::from_mat4(root) * hz.normalize(),
        up: (Mat3::from_mat4(root) * hy).normalize(),
    };
    let fp = FirstPersonEye { forward: fp.forward.normalize(), ..fp };
    Solved { parts, head: head_world, eyes, feet, fp }
}

/// All pieces of one ostrich in world space, ready to be instanced.
pub fn ostrich_parts(pose: &OstrichPose, time: f32) -> Vec<OstrichPart> {
    solve(pose, time).parts
}

/// World matrix of the head: origin at the head centre, +Z along the beak,
/// +Y up, scaled by `pose.scale`. Follows look-back, bash, gait bob.
/// Matches the rendered head exactly, except for the ghost's float bob and wobbles that depend on
/// `time` (this evaluates at time 0).
pub fn head_transform(pose: &OstrichPose) -> Mat4 {
    solve(pose, 0.0).head
}

/// World positions of the two eyes, `[left, right]` (the laser origins).
pub fn eye_positions(pose: &OstrichPose) -> [Vec3; 2] {
    solve(pose, 0.0).eyes
}

/// World positions of the two feet on the ground (for dust).
pub fn foot_positions(pose: &OstrichPose) -> [Vec3; 2] {
    solve(pose, 0.0).feet
}

/// Eye point, view direction and up for a first-person camera.
pub fn first_person_eye(pose: &OstrichPose) -> FirstPersonEye {
    solve(pose, 0.0).fp
}

/// One additive effect piece of the turbo leg wheel (world space, sRGB colours).
#[derive(Clone, Copy, Debug)]
pub(crate) enum Fx {
    /// A glowing segment (drawn with the beam pipeline).
    Beam { from: Vec3, to: Vec3, width: f32, intensity: f32, color: Rgb },
    /// A soft round glow (drawn with the additive particle pipeline).
    Glow { pos: Vec3, size: f32, alpha: f32, color: Rgb },
}

/// Turbo "Sonic" effects: ghost copies of each leg trailing round the cycle, a
/// swirl ring and disc, and faint streaks. Shown above normal top speed (about
/// 36 m/s and up) and whenever the wheel is forced (`pose.wheel`, drifting and
/// Laser Trump), where it is louder and tinted by `pose.drift_tier`. With `cam`, a
/// forced wheel also gets camera-facing motion arcs, so it reads from behind.
pub(crate) fn turbo_fx(p: &OstrichPose, time: f32, cam: Option<Vec3>) -> Vec<Fx> {
    let r = rig(p);
    let tw = r.tw;
    if tw < 0.02 {
        return Vec::new();
    }
    let sc = r.sc;
    let w = |v: Vec3| (r.root * v.extend(1.0)).truncate();
    let mut out = Vec::with_capacity(80);
    let leg = p.legs;
    // A forced wheel has to read at a glance, so it is drawn brighter and denser.
    let loud = 1.0 + 0.6 * r.wheel;
    let tier = crate::color::drift_tier_rgb(p.drift_tier);
    let tint = tier.unwrap_or(leg);
    let streak_col = tier.unwrap_or(if r.wheel > 0.0 {
        [1.0, 0.97, 0.9]
    } else {
        [1.0, 0.62, 0.22]
    });
    let glint_col = tier.map_or([1.0, 0.8, 0.55], |c| crate::color::lerp3(c, [1.0; 3], 0.35));
    // Ghost legs: earlier moments of the sweep, fading.
    let ghosts: &[(f32, f32)] = if r.wheel > 0.3 {
        &[
            (1.0, 0.46),
            (2.0, 0.36),
            (3.0, 0.27),
            (4.0, 0.18),
            (5.0, 0.10),
        ]
    } else {
        &[(1.0, 0.38), (2.0, 0.26), (3.0, 0.15)]
    };
    for &(j, a) in ghosts {
        for k in 0..2 {
            let (ph, wph) = r.phases(k, 0.055 * j);
            let [hip, knee, ank] = leg_chain(&r, k, ph, wph);
            out.push(Fx::Beam {
                from: w(hip),
                to: w(knee),
                width: 0.17 * sc,
                intensity: a * tw,
                color: leg,
            });
            out.push(Fx::Beam {
                from: w(knee),
                to: w(ank),
                width: 0.12 * sc,
                intensity: a * tw * 1.1,
                color: leg,
            });
            out.push(Fx::Glow {
                pos: w(ank),
                size: 0.34 * sc,
                alpha: 0.5 * a * tw,
                color: leg,
            });
        }
    }
    // Swirl ring (one per leg plane) with a highlight that rotates round it.
    let cy = -sole(0.35) + WHEEL_R;
    let spin = time * (16.0 + 28.0 * r.wheel);
    let (rz, ry) = (WHEEL_STRETCH * WHEEL_R * 1.2, WHEEL_R * 1.2);
    const N: usize = 16;
    for x in [HIP_X, -HIP_X] {
        for i in 0..N {
            let a0 = TAU * i as f32 / N as f32;
            let a1 = TAU * (i + 1) as f32 / N as f32;
            let pt = |a: f32| w(Vec3::new(x, cy - ry * a.cos(), -rz * a.sin()));
            let hl = 0.5 + 0.5 * (a0 - spin).cos();
            out.push(Fx::Beam {
                from: pt(a0),
                to: pt(a1),
                width: 0.10 * sc * loud,
                intensity: (0.10 + 0.22 * hl) * tw * loud,
                color: tint,
            });
        }
    }
    // Camera-facing motion arcs: the wheel lies in the bird's own plane, so seen from
    // behind it is edge-on. Comet-tailed arcs round the hip and round each foot, drawn
    // in the plane facing the camera, say "spinning legs" from any angle.
    if let (Some(cam), true) = (cam, r.wheel > 0.05) {
        let centre = w(Vec3::new(0.0, cy, 0.0));
        let view = (cam - centre).try_normalize().unwrap_or(Vec3::Z);
        let ux = Vec3::Y.cross(view).try_normalize().unwrap_or(Vec3::X);
        let uy = view.cross(ux);
        let k = r.wheel;
        let ring = |out: &mut Vec<Fx>, c: Vec3, rad: f32, phase: f32, arcs: usize, sweep: f32, wd: f32, a: f32, col: Rgb| {
            const SEG: usize = 6;
            for n in 0..arcs {
                let head = phase + TAU * n as f32 / arcs as f32;
                for s in 0..SEG {
                    // Segment 0 is the bright head, the tail trails behind it (against the spin).
                    let (t0, t1) = (s as f32 / SEG as f32, (s + 1) as f32 / SEG as f32);
                    let pt = |t: f32| {
                        let ang = head - sweep * t;
                        c + (ux * ang.cos() + uy * ang.sin()) * rad
                    };
                    let fade = (1.0 - t0) * (1.0 - t0);
                    out.push(Fx::Beam {
                        from: pt(t0),
                        to: pt(t1),
                        width: wd * sc * (1.0 - 0.55 * t0),
                        intensity: a * fade * k * loud,
                        color: col,
                    });
                }
            }
        };
        let hot = crate::color::lerp3(tint, [1.0; 3], 0.5);
        // Big outer arcs and a tighter, faster inner pair round the whole wheel.
        ring(&mut out, centre, rz * 1.15, spin * 0.5, 4, 2.0, 0.42, 1.4, tint);
        ring(&mut out, centre, rz * 0.7, -spin * 0.8, 3, 2.0, 0.30, 1.4, hot);
        // A swirl of arcs round each foot, which is where the eye goes.
        for kk in 0..2 {
            let (ph, wph) = r.phases(kk, 0.0);
            let [_, _, ank] = leg_chain(&r, kk, ph, wph);
            let foot = w(ank);
            ring(&mut out, foot, 0.7 * sc, spin * 1.3 + kk as f32 * 2.0, 3, 2.2, 0.32, 1.6, hot);
            out.push(Fx::Glow { pos: foot, size: 0.9 * sc, alpha: 0.55 * k, color: tint });
        }
    }
    // Disc: a big soft glow plus glints chasing round the rim.
    out.push(Fx::Glow {
        pos: w(Vec3::new(0.0, cy, 0.0)),
        size: 2.0 * rz * 1.25 * sc * (1.0 + 0.3 * r.wheel),
        alpha: (0.32 * loud * (1.0 + 0.3 * r.wheel)).min(0.8) * tw,
        color: tint,
    });
    let glints = if r.wheel > 0.0 { 8 } else { 4 };
    for i in 0..glints {
        let a = spin + TAU * i as f32 / glints as f32;
        out.push(Fx::Glow {
            pos: w(Vec3::new(0.0, cy - ry * a.cos(), -rz * a.sin())),
            size: 0.5 * sc,
            alpha: 0.45 * tw,
            color: glint_col,
        });
    }
    // Faint streaks trailing back from the wheel.
    for (i, (dy, len, wd)) in [
        (-0.30f32, 2.4f32, 0.12f32),
        (0.0, 3.2, 0.16),
        (0.28, 2.0, 0.10),
        (0.12, 1.5, 0.06),
    ]
    .into_iter()
    .enumerate()
    {
        let flick = 0.8 + 0.2 * (time * 31.0 + i as f32 * 2.1).sin();
        let col = if i == 3 {
            [1.0, 0.95, 0.85]
        } else {
            streak_col
        };
        let len = len * (1.0 + 0.5 * r.wheel);
        let from = w(Vec3::new(0.0, cy + dy, -rz * 0.9));
        let to = w(Vec3::new(0.0, cy + dy * 0.8, -rz * 0.9 - len * flick));
        out.push(Fx::Beam {
            from,
            to,
            width: wd * sc,
            intensity: 0.28 * tw * loud,
            color: col,
        });
    }
    out
}

/// Length of the Laser Trump beam out of the tail (m); matches the sim's tail reach.
pub(crate) const TRUMP_BEAM_LEN: f32 = 8.0;

/// Where the Laser Trump leaves the ostrich: the rump, under the tail feathers.
pub(crate) fn tail_point(p: &OstrichPose) -> Vec3 {
    let r = rig(p);
    r.root
        .transform_point3(r.u.transform_point3(Vec3::new(0.0, 0.42, -0.85)))
}

/// Laser Trump beam: a flickering green-white laser from the tail, straight back
/// along the heading, with a glare at the source.
pub(crate) fn trump_fx(p: &OstrichPose, time: f32) -> Vec<Fx> {
    if !p.trumping {
        return Vec::new();
    }
    let sc = p.scale.max(0.05);
    let src = tail_point(p);
    let back = -Vec3::new(p.yaw.sin(), 0.0, p.yaw.cos());
    let len = TRUMP_BEAM_LEN * (0.95 + 0.05 * (time * 41.0).sin());
    let mut out = Vec::with_capacity(16);
    const SEGS: usize = 4;
    for s in 0..SEGS {
        let (t0, t1) = (s as f32 / SEGS as f32, (s + 1) as f32 / SEGS as f32);
        let flick = (0.8
            + 0.2 * (time * 53.0 + s as f32 * 1.7).sin()
            + 0.1 * (time * 97.0 + s as f32 * 3.1).sin())
        .max(0.4);
        let fade = 1.0 - 0.45 * t0;
        let (from, to) = (src + back * len * t0, src + back * len * t1);
        for (wd, k, col) in [
            (1.15f32, 0.5f32, [0.2f32, 1.0, 0.3]),
            (0.6, 0.95, [0.55, 1.0, 0.55]),
            (0.22, 1.6, [1.0, 1.0, 0.95]),
        ] {
            out.push(Fx::Beam {
                from,
                to,
                width: wd * sc * fade,
                intensity: k * flick * fade,
                color: col,
            });
        }
    }
    let pulse = 0.9 + 0.1 * (time * 44.0).sin();
    // Camera-facing beads along the beam, so it still reads as a glowing column when
    // seen end-on from the chase camera.
    for k in 1..6 {
        let t = k as f32 / 6.0;
        let flick = 0.75 + 0.25 * (time * 61.0 + k as f32 * 2.3).sin();
        out.push(Fx::Glow { pos: src + back * len * t, size: (1.5 - 0.9 * t) * sc, alpha: 0.5 * (1.0 - 0.6 * t) * flick, color: [0.5, 1.0, 0.55] });
    }
    out.push(Fx::Glow {
        pos: src,
        size: 3.4 * sc * pulse,
        alpha: 0.75,
        color: [0.4, 1.0, 0.45],
    });
    out.push(Fx::Glow {
        pos: src,
        size: 1.0 * sc * pulse,
        alpha: 0.95,
        color: [1.0, 1.0, 0.9],
    });
    out.push(Fx::Glow {
        pos: src + back * len,
        size: 0.8 * sc,
        alpha: 0.4 * pulse,
        color: [0.5, 1.0, 0.5],
    });
    out
}

// ------------------------------------------------------------- meshes ----

fn orient(z: Vec3, thick: Vec3) -> (Vec3, Vec3, Vec3) {
    // x = thickness axis, y = width axis, z = along.
    let z = z.normalize();
    let y = thick.cross(z).normalize();
    let x = y.cross(z).normalize();
    // y x z must be right-handed for a clean matrix: x = y x z.
    (x, y, z)
}

/// A plume with its own width profile along the four points (`[0.5, 1.0, 0.95, 0.5]` is a fat, blunt cartoon feather).
#[allow(clippy::too_many_arguments)]
fn plume_w(b: &mut MeshData, points: [Vec3; 4], width: f32, thick: f32, axis: Vec3, shade: f32, quill: bool, widths: [f32; 4]) {
    let mut rings = [[Vec3::ZERO; 4]; 4];
    for k in 0..4 {
        let d = points[(k + 1).min(3)] - points[k.saturating_sub(1)];
        let (x, y, _) = orient(d, axis);
        let w = width * widths[k];
        let t = thick * if k == 3 { 0.0 } else { 1.0 };
        rings[k] = [points[k] + x * t, points[k] + y * w, points[k] - x * t * 0.4, points[k] - y * w];
    }
    for k in 0..3 {
        let st = if k == 2 && quill { Style::tint1(0.94) } else { Style::tint0(shade - k as f32 * 0.035) }.flat().ao(0.12);
        for j in 0..4 {
            let n = (j + 1) % 4;
            let p = [rings[k][j], rings[k][n], rings[k + 1][n], rings[k + 1][j]];
            let normal = ((p[0] + p[1] + p[2] + p[3]) * 0.25 - (points[k] + points[k + 1]) * 0.5).normalize_or_zero();
            b.quad4(p, normal, &st);
        }
    }
    if quill {
        for k in 0..2 {
            b.cyl(&limb(points[k] + axis * thick, points[k + 1] + axis * thick, 0.006), 5, 1.0, 0.65, false, &Style::tint0(shade * 1.04));
        }
    }
}

/// A plump, fluffy body: smooth egg, overlapping feather scallops, fluffy thigh puffs.
pub(crate) fn build_body() -> MeshData {
    let mut b = MeshData::new();
    b.sphere(&Mat4::from_scale(Vec3::new(0.52, 0.48, 0.68)), 14, 9, &Style::tint0(1.0).ao(0.32));
    // Chest and rump puffs make the silhouette round and fluffy.
    b.sphere(&(Mat4::from_translation(Vec3::new(0.0, -0.06, 0.36)) * Mat4::from_scale(Vec3::new(0.42, 0.40, 0.36))), 10, 6, &Style::tint0(1.04).ao(0.15));
    b.sphere(&(Mat4::from_translation(Vec3::new(0.0, 0.08, -0.42)) * Mat4::from_scale(Vec3::new(0.40, 0.40, 0.38))), 10, 6, &Style::tint0(0.95).ao(0.18));
    // Thigh puffs where the legs go in.
    for sx in [-1.0f32, 1.0] {
        b.sphere(&(Mat4::from_translation(Vec3::new(sx * 0.30, -0.14, 0.04)) * Mat4::from_scale(Vec3::new(0.27, 0.3, 0.36))), 8, 5, &Style::tint0(0.97).ao(0.25));
    }
    // Feather scallops across the back: rounded, overlapping, two alternating tones.
    let rows: [(f32, &[f32]); 4] = [(0.30, &[-0.26, 0.0, 0.26]), (0.12, &[-0.32, -0.11, 0.11, 0.32]), (-0.06, &[-0.30, 0.0, 0.30]), (-0.24, &[-0.20, 0.20])];
    for (ri, (z, xs)) in rows.iter().enumerate() {
        for (i, x) in xs.iter().enumerate() {
            let y = 0.40 - 0.05 * (x / 0.3).powi(2) - 0.02 * ri as f32;
            let shade = if (ri + i) % 2 == 0 { 1.06 } else { 0.93 };
            let m = Mat4::from_translation(Vec3::new(*x, y, *z)) * Mat4::from_rotation_x(-0.25) * Mat4::from_scale(Vec3::new(0.19, 0.11, 0.17));
            b.sphere(&m, 6, 4, &Style::tint0(shade).ao(0.2));
        }
    }
    b
}

/// The tail: a fan of fat, rounded plumes curling up and back, tips in the accent colour.
pub(crate) fn build_tail() -> MeshData {
    let mut b = MeshData::new();
    for k in -3i32..=3 {
        let psi = k as f32 * 0.27;
        let edge = k.abs() as f32;
        let (sx, cz) = (psi.sin(), psi.cos());
        let len = 0.84 - edge * 0.05;
        // Leaf blades in a fan that rises up and back like a feather duster.
        plume_w(
            &mut b,
            [
                Vec3::ZERO,
                Vec3::new(sx * 0.26, 0.26, -cz * 0.24),
                Vec3::new(sx * len * 0.66, 0.58 - edge * 0.02, -cz * len * 0.60),
                Vec3::new(sx * len * 0.95, 0.72 - edge * 0.04, -cz * len * 0.78),
            ],
            0.17 - edge * 0.006,
            0.045,
            Vec3::Y,
            1.0 - edge * 0.03,
            true,
            [0.55, 1.0, 1.0, 0.7],
        );
    }
    b
}

/// Little flappy wings: a fan of five fat feathers on a round shoulder, tips in the accent colour.
pub(crate) fn build_wing(side: f32) -> MeshData {
    let mut b = MeshData::new();
    for k in 0..5 {
        let a = -0.20 + 0.23 * k as f32;
        let dir = Vec3::new(side * 0.12, -a.cos(), -a.sin()).normalize();
        let len = 0.46 + 0.06 * k as f32;
        let p = Vec3::new(0.0, 0.0, 0.035 * k as f32 - 0.06);
        plume_w(
            &mut b,
            [p, p + dir * len * 0.3 + Vec3::X * side * 0.035, p + dir * len * 0.7 + Vec3::X * side * 0.04, p + dir * len],
            0.15,
            0.04,
            Vec3::X,
            1.05 - k as f32 * 0.03,
            true,
            [0.6, 1.0, 0.95, 0.5],
        );
    }
    b.sphere(&Mat4::from_scale(Vec3::new(0.13, 0.22, 0.28)), 10, 6, &Style::tint0(1.0).ao(0.2));
    b
}

pub(crate) fn build_neck_seg() -> MeshData {
    let mut b = MeshData::new();
    b.lathe(&Mat4::IDENTITY, 8, &[(0.88, 0.0), (1.0, 0.12), (0.80, 1.0)], &Style::tint0(1.0).sheen(0.06));
    b
}

pub(crate) fn build_ball(segs: u32, rings: u32, emissive: f32) -> MeshData {
    let mut b = MeshData::new();
    b.sphere(&Mat4::IDENTITY, segs, rings, &Style::tint0(1.0).emit(emissive));
    b
}

pub(crate) fn build_head() -> MeshData {
    let mut b = MeshData::new();
    b.sphere(&Mat4::from_scale(Vec3::new(0.2, 0.18, 0.24)), 12, 8, &Style::tint0(1.0).ao(0.2));
    // Head tuft.
    for l in 0..3 {
        let a = -1.0 + l as f32;
        let base = Vec3::new(a * 0.05, 0.15, -0.05);
        b.cyl(&limb(base, base + Vec3::new(a * 0.04, 0.12, -0.08), 0.03), 5, 1.0, 0.2, false, &Style::tint0(0.85));
    }
    b
}

pub(crate) fn build_beak() -> MeshData {
    let mut b = MeshData::new();
    // Upper beak: flattened cone along +Z.
    let up = basis(Vec3::new(0.0, 0.0, 0.15), Vec3::X * 0.105, Vec3::Z * 0.32, Vec3::Y * 0.058);
    b.cyl(&up, 10, 1.0, 0.30, true, &Style::tint0(1.0).ao(0.15).sheen(0.30));
    let low = basis(Vec3::new(0.0, -0.05, 0.14), Vec3::X * 0.085, Vec3::Z * 0.26, Vec3::Y * 0.036);
    b.cyl(&low, 8, 1.0, 0.35, true, &Style::tint0(0.80));
    // Dark nail tip.
    b.sphere(&(Mat4::from_translation(Vec3::new(0.0, 0.0, 0.47)) * Mat4::from_scale(Vec3::new(0.036, 0.026, 0.03))), 6, 4, &Style::col(0.15, 0.1, 0.08));
    // Nostrils.
    for s in [1.0f32, -1.0] {
        b.sphere(&(Mat4::from_translation(Vec3::new(s * 0.035, 0.03, 0.31)) * Mat4::from_scale(Vec3::new(0.014, 0.01, 0.024))), 5, 4, &Style::col(0.2, 0.12, 0.1));
    }
    b
}

pub(crate) fn build_thigh() -> MeshData {
    let mut b = MeshData::new();
    b.cyl(&Mat4::IDENTITY, 10, 1.0, 0.62, false, &Style::tint0(1.0).ao(0.2));
    b.sphere(&(Mat4::from_translation(Vec3::new(0.0, 0.0, 0.0)) * Mat4::from_scale(Vec3::new(1.0, 0.25, 1.0))), 8, 4, &Style::tint0(1.0));
    b
}

pub(crate) fn build_shin() -> MeshData {
    let mut b = MeshData::new();
    b.cyl(&Mat4::IDENTITY, 8, 1.0, 0.72, false, &Style::tint0(1.0).ao(0.1));
    b
}

/// Big comic feet: the raw foot, scaled up.
pub(crate) fn build_foot() -> MeshData {
    let mut b = MeshData::new();
    b.append(&build_foot_raw(), &Mat4::from_scale(Vec3::splat(FOOT_S)));
    b
}

fn build_foot_raw() -> MeshData {
    let mut b = MeshData::new();
    b.sphere(&(Mat4::from_translation(Vec3::new(0.0, -0.04, 0.03)) * Mat4::from_scale(Vec3::new(0.075, 0.05, 0.12))), 8, 5, &Style::tint0(1.0));
    // Two toes: a big one and a small one.
    let toes = [(0.045f32, 0.07f32, 0.30f32, 0.075f32), (-0.05, -0.30, 0.20, 0.058)];
    for (x, ang, len, w) in toes {
        let dir = Vec3::new(ang.sin(), -0.04, ang.cos()).normalize();
        let a = Vec3::new(x, -0.052, 0.02);
        let tip = a + dir * len;
        let m = basis(a, Vec3::X * w, tip - a, Vec3::Y * (w * 0.62));
        b.cyl(&m, 8, 1.0, 0.55, true, &Style::tint0(0.95));
        // Claw.
        b.cone(&limb(tip - dir * 0.01, tip + dir * 0.055 + Vec3::new(0.0, -0.01, 0.0), 0.028), 5, &Style::col(0.12, 0.09, 0.08));
    }
    b
}

// ---- accessories (head-local: origin head centre, +Z beak, +Y up) ----

pub(crate) fn build_scarf() -> MeshData {
    // In neck frame (y along the neck): a thick ring and a knot.
    let mut b = MeshData::new();
    b.torus(&Mat4::from_scale(Vec3::new(1.0, 1.0, 1.0)), 14, 7, 0.145, 0.062, &Style::tint1(1.0).ao(0.2));
    b.sphere(&(Mat4::from_translation(Vec3::new(0.0, 0.0, -0.16)) * Mat4::from_scale(Vec3::new(0.09, 0.08, 0.07))), 8, 5, &Style::tint1(0.85));
    b
}

pub(crate) fn build_scarf_tail() -> MeshData {
    // Local +Z is the trailing direction; two ribbons with a gentle curve.
    let mut b = MeshData::new();
    for (k, x) in [-0.045f32, 0.045].iter().enumerate() {
        let mut prev = Vec3::new(*x, 0.0, 0.0);
        for s in 1..=4 {
            let t = s as f32 / 4.0;
            let cur = Vec3::new(*x + (t * 3.0).sin() * 0.02 * (1.0 + k as f32), -0.03 * t * t * (1.0 + 0.8 * k as f32), 0.62 * t);
            let w = 0.075 * (1.0 - 0.25 * t);
            let a = prev;
            let c = cur;
            b.quad4(
                [a + Vec3::X * w, a - Vec3::X * w, c - Vec3::X * w, c + Vec3::X * w],
                Vec3::Y,
                &Style::tint1(if s % 2 == 0 { 1.0 } else { 0.88 }),
            );
            b.quad4(
                [a + Vec3::X * w, a - Vec3::X * w, c - Vec3::X * w, c + Vec3::X * w],
                -Vec3::Y,
                &Style::tint1(0.7),
            );
            prev = cur;
        }
    }
    b
}

pub(crate) fn build_goggles() -> MeshData {
    let mut b = MeshData::new();
    // Strap around the head.
    b.torus(&(Mat4::from_translation(Vec3::new(0.0, 0.055, 0.02)) * Mat4::from_scale(Vec3::new(1.0, 1.0, 1.28))), 16, 5, 0.152, 0.018, &Style::col(0.12, 0.1, 0.1));
    for s in [1.0f32, -1.0] {
        let c = Vec3::new(s * 0.098, 0.055, 0.09);
        // Rim and lens (axis out to the side and a little forward).
        let axis = Vec3::new(s * 0.78, 0.0, 0.62).normalize();
        let m = limb(c - axis * 0.012, c + axis * 0.045, 0.098);
        b.cyl(&m, 14, 1.0, 1.0, true, &Style::col(0.1, 0.1, 0.12));
        let m2 = limb(c + axis * 0.03, c + axis * 0.065, 0.078);
        b.cyl(&m2, 14, 1.0, 0.9, true, &Style::tint1(1.0).emit(0.35));
    }
    // Bridge.
    b.cube(&(Mat4::from_translation(Vec3::new(0.0, 0.06, 0.17)) * Mat4::from_scale(Vec3::new(0.05, 0.02, 0.02))), &Style::col(0.1, 0.1, 0.12));
    b
}

pub(crate) fn build_crown() -> MeshData {
    let mut b = MeshData::new();
    let gold = Style::col(1.0, 0.78, 0.16).emit(0.18).flat();
    let base = Mat4::from_translation(Vec3::new(0.0, 0.115, -0.01)) * Mat4::from_rotation_x(0.12);
    b.cyl(&(base * Mat4::from_scale(Vec3::new(0.13, 0.05, 0.145))), 12, 1.0, 1.08, false, &gold.ao(0.2));
    b.torus(&(base * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.0)) * Mat4::from_scale(Vec3::new(1.0, 1.0, 1.1))), 14, 5, 0.128, 0.016, &gold);
    for i in 0..6 {
        let a = TAU * i as f32 / 6.0;
        let pos = Vec3::new(a.sin() * 0.13, 0.05, a.cos() * 0.145);
        let tall = if i % 2 == 0 { 0.115 } else { 0.08 };
        b.cone(&(base * Mat4::from_translation(pos) * Mat4::from_scale(Vec3::new(0.035, tall, 0.035))), 5, &gold);
        b.sphere(&(base * Mat4::from_translation(pos + Vec3::Y * tall) * Mat4::from_scale(Vec3::splat(0.017))), 6, 4, &Style::tint1(1.0).emit(0.4));
    }
    b
}

pub(crate) fn build_star_headband() -> MeshData {
    let mut b = MeshData::new();
    b.torus(&(Mat4::from_translation(Vec3::new(0.0, 0.07, 0.0)) * Mat4::from_rotation_x(-0.18) * Mat4::from_scale(Vec3::new(1.0, 1.0, 1.25))), 16, 5, 0.148, 0.024, &Style::tint1(1.0));
    // The star on the brow.
    let m = Mat4::from_translation(Vec3::new(0.0, 0.135, 0.145)) * Mat4::from_rotation_x(-0.95) * Mat4::from_scale(Vec3::splat(0.095));
    b.star(&m, 5, 0.46, 0.35, &Style::col(1.0, 0.88, 0.2).emit(0.5));
    b
}

pub(crate) fn build_viking() -> MeshData {
    let mut b = MeshData::new();
    let steel = Style::col(0.66, 0.68, 0.74).ao(0.25);
    let gold = Style::col(0.95, 0.72, 0.2).emit(0.1);
    // Dome.
    b.sphere(&(Mat4::from_translation(Vec3::new(0.0, 0.035, -0.015)) * Mat4::from_scale(Vec3::new(0.168, 0.15, 0.2))), 14, 8, &steel);
    // Brow band and nose guard.
    b.torus(&(Mat4::from_translation(Vec3::new(0.0, 0.055, 0.0)) * Mat4::from_scale(Vec3::new(1.0, 1.0, 1.22))), 16, 5, 0.158, 0.02, &gold);
    b.cube(&(Mat4::from_translation(Vec3::new(0.0, 0.02, 0.178)) * Mat4::from_scale(Vec3::new(0.025, 0.09, 0.02))), &gold);
    b.cube(&(Mat4::from_translation(Vec3::new(0.0, 0.17, -0.015)) * Mat4::from_scale(Vec3::new(0.02, 0.03, 0.2))), &gold);
    // Horns: curved, cream with a darker tip.
    for s in [1.0f32, -1.0] {
        let pts = [
            Vec3::new(s * 0.14, 0.07, 0.0),
            Vec3::new(s * 0.23, 0.07, 0.0),
            Vec3::new(s * 0.30, 0.12, 0.0),
            Vec3::new(s * 0.34, 0.20, 0.0),
            Vec3::new(s * 0.34, 0.28, 0.0),
        ];
        for i in 0..4 {
            let r0 = 0.045 - 0.008 * i as f32;
            let col = if i == 3 { Style::col(0.85, 0.8, 0.65) } else { Style::col(0.96, 0.92, 0.8) };
            b.cyl(&limb(pts[i], pts[i + 1], r0), 7, 1.0, (r0 - 0.008) / r0, false, &col.ao(0.15));
        }
        b.cone(&limb(pts[4], pts[4] + Vec3::new(-s * 0.01, 0.07, 0.0), 0.028), 7, &Style::col(0.8, 0.74, 0.58));
    }
    b
}

pub(crate) fn build_mohawk() -> MeshData {
    let mut b = MeshData::new();
    for i in 0..9 {
        let t = i as f32 / 8.0;
        let z = 0.10 - 0.30 * t;
        let y = 0.12 + 0.01 * (1.0 - t) - 0.03 * t * t;
        let h = 0.15 * (1.0 - 0.45 * (t - 0.35).abs()).max(0.5);
        let base = Vec3::new(0.0, y, z);
        let tip = base + Vec3::new(0.0, h, -0.045 - 0.05 * t);
        let m = basis(base, Vec3::X * 0.026, tip - base, Vec3::Z * 0.075);
        b.cone(&m, 6, &Style::tint1(if i % 2 == 0 { 1.0 } else { 0.86 }).flat());
    }
    b
}

pub(crate) fn build_shades() -> MeshData {
    let mut b = MeshData::new();
    let dark = Style::col(0.05, 0.05, 0.07).emit(0.05);
    for s in [1.0f32, -1.0] {
        let c = Vec3::new(s * 0.088, 0.055, 0.145);
        let axis = Vec3::new(s * 0.45, 0.0, 0.9).normalize();
        b.cyl(&limb(c - axis * 0.01, c + axis * 0.03, 0.082), 10, 1.0, 1.0, true, &dark);
        b.cyl(&limb(c + axis * 0.026, c + axis * 0.038, 0.066), 10, 1.0, 1.0, true, &Style::col(0.12, 0.14, 0.2).emit(0.2));
        // Arms back along the head.
        b.cube(&(Mat4::from_translation(Vec3::new(s * 0.16, 0.06, 0.05)) * Mat4::from_scale(Vec3::new(0.012, 0.014, 0.17))), &dark);
    }
    b.cube(&(Mat4::from_translation(Vec3::new(0.0, 0.065, 0.185)) * Mat4::from_scale(Vec3::new(0.06, 0.016, 0.014))), &dark);
    b
}

pub(crate) fn build_chain() -> MeshData {
    // Neck frame: a gold chain ring with a ₿ medallion hanging at the front.
    let mut b = MeshData::new();
    let gold = Style::col(1.0, 0.8, 0.2).emit(0.2);
    b.torus(&Mat4::IDENTITY, 16, 5, 0.16, 0.025, &gold);
    b.torus(&(Mat4::from_translation(Vec3::new(0.0, -0.05, 0.0)) * Mat4::from_scale(Vec3::new(0.93, 1.0, 0.93))), 16, 5, 0.16, 0.02, &gold);
    let med = Mat4::from_translation(Vec3::new(0.0, -0.17, 0.15)) * Mat4::from_rotation_x(-0.25);
    b.cyl(&(med * Mat4::from_rotation_x(PI * 0.5) * Mat4::from_translation(Vec3::new(0.0, -0.02, 0.0)) * Mat4::from_scale(Vec3::new(0.085, 0.04, 0.085))), 12, 1.0, 1.0, true, &gold);
    b.cube(&(med * Mat4::from_translation(Vec3::new(0.0, 0.0, 0.03)) * Mat4::from_scale(Vec3::new(0.014, 0.08, 0.012))), &Style::col(0.7, 0.4, 0.05));
    b.cube(&(med * Mat4::from_translation(Vec3::new(0.02, 0.012, 0.03)) * Mat4::from_scale(Vec3::new(0.04, 0.015, 0.012))), &Style::col(0.7, 0.4, 0.05));
    b.cube(&(med * Mat4::from_translation(Vec3::new(0.025, -0.02, 0.03)) * Mat4::from_scale(Vec3::new(0.045, 0.015, 0.012))), &Style::col(0.7, 0.4, 0.05));
    b
}

pub(crate) fn build_cap() -> MeshData {
    let mut b = MeshData::new();
    let st = Style::tint1(1.0).ao(0.2);
    // Dome: upper part of a sphere over the head.
    b.lathe(
        &(Mat4::from_translation(Vec3::new(0.0, 0.015, -0.01)) * Mat4::from_scale(Vec3::new(0.172, 0.17, 0.21))),
        14,
        &[(1.0, 0.0), (0.98, 0.25), (0.88, 0.5), (0.65, 0.75), (0.35, 0.92), (0.0, 1.0)],
        &st,
    );
    // Brim band.
    b.torus(&(Mat4::from_translation(Vec3::new(0.0, 0.025, -0.01)) * Mat4::from_scale(Vec3::new(1.0, 1.0, 1.22))), 16, 5, 0.166, 0.014, &Style::tint1(0.78));
    // Visor.
    let visor = Mat4::from_translation(Vec3::new(0.0, 0.032, 0.20)) * Mat4::from_rotation_x(0.22);
    b.sphere(&(visor * Mat4::from_scale(Vec3::new(0.135, 0.014, 0.115))), 12, 5, &Style::tint1(0.82));
    // Button.
    b.sphere(&(Mat4::from_translation(Vec3::new(0.0, 0.185, -0.01)) * Mat4::from_scale(Vec3::splat(0.02))), 6, 4, &Style::col(0.95, 0.95, 0.97));
    b
}



#[cfg(test)]
mod tests {
    use super::*;

    fn fwd(yaw: f32) -> Vec3 {
        Vec3::new(yaw.sin(), 0.0, yaw.cos())
    }
    fn right(yaw: f32) -> Vec3 {
        Vec3::new(-yaw.cos(), 0.0, yaw.sin())
    }

    #[test]
    fn spec_axes_match_rotation_y() {
        for yaw in [0.0f32, 0.7, 2.0, -1.3, 3.0] {
            let f = Mat4::from_rotation_y(yaw).transform_vector3(Vec3::Z);
            assert!((f - fwd(yaw)).length() < 1e-5);
            // right = forward x up
            assert!((fwd(yaw).cross(Vec3::Y) - right(yaw)).length() < 1e-5);
            // The racer's right is local -X.
            let r = Mat4::from_rotation_y(yaw).transform_vector3(-Vec3::X);
            assert!((r - right(yaw)).length() < 1e-5);
        }
    }

    #[test]
    fn rig_is_finite_over_the_pose_space() {
        let mut n = 0;
        for accessory in [Accessory::None, Accessory::Scarf, Accessory::Goggles, Accessory::Crown, Accessory::StarHeadband, Accessory::VikingHelmet, Accessory::Mohawk, Accessory::ChainShades, Accessory::Cap] {
            for k in 0..60 {
                let t = k as f32 / 59.0;
                let p = OstrichPose {
                    pos: Vec3::new(3.0 * t, 1.0, -2.0),
                    yaw: 6.0 * t - 3.0,
                    visual_yaw: 0.4 * (t * 9.0).sin(),
                    lean: 0.4 * (t * 7.0).cos(),
                    pitch: 0.2 * (t * 5.0).sin(),
                    gait_phase: 13.7 * t,
                    speed: 45.0 * t,
                    airborne: k % 3 == 0,
                    tuck: t,
                    bash: 2.0 * t - 1.0,
                    look_back: t,
                    laser_on: k % 2 == 0,
                    spin: 12.0 * t,
                    trick_flip: t,
                    scale: 0.8 + 0.5 * t,
                    accessory,
                    hide_head: k % 5 == 0,
                    jockey: k % 7 != 0,
                    rider: k as u8,
                    attire: [(k % 4) as u8, ((k + 1) % 4) as u8],
                    laptop: k % 4 == 0,
                    cake: k % 4 == 1,
                    shells: k % 3 == 0,
                    burning: k % 4 == 2,
                    tumble: if k % 2 == 0 { t } else { 0.0 },
                    ..Default::default()
                };
                for part in ostrich_parts(&p, 3.3 * t) {
                    n += 1;
                    assert!(part.model.to_cols_array().iter().all(|v| v.is_finite()), "{:?}", part.mesh);
                    assert!(part.tint0.iter().chain(part.tint1.iter()).all(|v| v.is_finite()));
                }
                assert!(head_transform(&p).to_cols_array().iter().all(|v| v.is_finite()));
                assert!(eye_positions(&p).iter().all(|v| v.is_finite()));
                assert!(foot_positions(&p).iter().all(|v| v.is_finite()));
                let fp = first_person_eye(&p);
                assert!(fp.eye.is_finite() && (fp.forward.length() - 1.0).abs() < 1e-3 && fp.up.is_finite());
            }
        }
        assert!(n > 1000);
    }

    #[test]
    fn head_is_ahead_and_above_at_yaw_zero() {
        let p = OstrichPose::default();
        let h = head_transform(&p);
        let c = h.transform_point3(Vec3::ZERO);
        assert!(c.z > 0.2, "head should be forward of the body centre: {c:?}");
        assert!(c.y > 1.8, "head should be high: {c:?}");
        // The head looks along +Z.
        assert!(h.transform_vector3(Vec3::Z).normalize().dot(Vec3::Z) > 0.9);
        let eyes = eye_positions(&p);
        assert!(eyes[0].x > 0.0 && eyes[1].x < 0.0, "first eye is the left one (+X)");
    }

    #[test]
    fn look_back_turns_the_head_round() {
        let mut p = OstrichPose { yaw: 0.9, ..Default::default() };
        let f0 = head_transform(&p).transform_vector3(Vec3::Z).normalize();
        assert!(f0.dot(fwd(p.yaw)) > 0.9);
        p.look_back = 1.0;
        let f1 = head_transform(&p).transform_vector3(Vec3::Z).normalize();
        assert!(f1.dot(fwd(p.yaw)) < -0.8, "head should face backwards, dot={}", f1.dot(fwd(p.yaw)));
    }

    #[test]
    fn bash_sign_is_right_positive() {
        let mut p = OstrichPose::default();
        let base = head_transform(&p).transform_point3(Vec3::ZERO);
        p.bash = 1.0;
        let hr = head_transform(&p).transform_point3(Vec3::ZERO);
        p.bash = -1.0;
        let hl = head_transform(&p).transform_point3(Vec3::ZERO);
        // Racer's right at yaw 0 is -X.
        assert!(hr.x < base.x - 0.1, "right bash goes to -X: {hr:?} vs {base:?}");
        assert!(hl.x > base.x + 0.1, "left bash goes to +X: {hl:?} vs {base:?}");
    }

    #[test]
    fn feet_touch_the_ground_while_running() {
        for k in 0..16 {
            let p = OstrichPose { gait_phase: k as f32 / 16.0, speed: 28.0, ..Default::default() };
            let feet = foot_positions(&p);
            let low = feet[0].y.min(feet[1].y);
            assert!(low.abs() < 0.05, "lowest foot should sit on the ground, got {low} at phase {k}");
            assert!(feet[0].y > -0.05 && feet[1].y > -0.05, "no foot below ground");
        }
    }

    #[test]
    fn legs_alternate() {
        let a = OstrichPose { gait_phase: 0.1, speed: 28.0, ..Default::default() };
        let fa = foot_positions(&a);
        let b = OstrichPose { gait_phase: 0.6, speed: 28.0, ..Default::default() };
        let fb = foot_positions(&b);
        // Half a cycle later the feet have swapped roles (z positions swap).
        assert!((fa[0].z - fb[1].z).abs() < 0.05 && (fa[1].z - fb[0].z).abs() < 0.05);
    }

    #[test]
    fn drift_lean_rolls_towards_the_right() {
        let p = OstrichPose { lean: 0.4, ..Default::default() };
        let parts = ostrich_parts(&p, 0.0);
        let head = parts.iter().find(|q| q.mesh == MeshId::Head).unwrap();
        // Leaning right: the head's x moves to the racer's right (-X).
        assert!(head.model.w_axis.x < 0.0);
    }

    fn head_y(speed: f32, phase: f32) -> f32 {
        let p = OstrichPose { speed, gait_phase: phase, ..Default::default() };
        head_transform(&p).transform_point3(Vec3::ZERO).y
    }

    #[test]
    fn body_and_head_glide_at_speed() {
        let range = |speed: f32| {
            let v: Vec<f32> = (0..32).map(|k| head_y(speed, k as f32 / 32.0)).collect();
            v.iter().cloned().fold(f32::MIN, f32::max) - v.iter().cloned().fold(f32::MAX, f32::min)
        };
        assert!(range(30.0) < 0.01, "head bob at top speed: {}", range(30.0));
        assert!(range(5.0) < 0.08, "head bob at a jog: {}", range(5.0));
    }

    #[test]
    fn stride_grows_with_speed() {
        let span = |speed: f32| {
            let z: Vec<f32> = (0..32).map(|k| foot_positions(&OstrichPose { speed, gait_phase: k as f32 / 32.0, ..Default::default() })[0].z).collect();
            z.iter().cloned().fold(f32::MIN, f32::max) - z.iter().cloned().fold(f32::MAX, f32::min)
        };
        let (a, b, c) = (span(5.0), span(18.0), span(30.0));
        assert!(a < b && b < c && c > 1.4, "foot spans {a} {b} {c}");
    }

    #[test]
    fn turbo_wheel_only_above_top_speed() {
        // The wheel, unforced, still belongs to speeds above normal top speed.
        let at = |speed: f32| {
            turbo_fx(
                &OstrichPose {
                    speed,
                    ..Default::default()
                },
                0.5,
                None,
            )
            .len()
        };
        assert_eq!(at(32.0), 0);
        assert_eq!(at(36.0), 0);
        assert!(at(42.0) > 20 && at(42.0) < 80);
        // Forced, it shows at any speed.
        let forced = |speed: f32| {
            turbo_fx(
                &OstrichPose {
                    speed,
                    wheel: 1.0,
                    ..Default::default()
                },
                0.5,
                None,
            )
            .len()
        };
        assert!(forced(18.0) > 20 && forced(18.0) < 90);
        // The wheel path keeps the feet at or above the ground.
        for k in 0..32 {
            let f = foot_positions(&OstrichPose { speed: 42.0, gait_phase: k as f32 / 32.0, ..Default::default() });
            assert!(f[0].y > -0.06 && f[1].y > -0.06);
            let f = foot_positions(&OstrichPose {
                speed: 18.0,
                wheel: 1.0,
                gait_phase: k as f32 / 32.0,
                ..Default::default()
            });
            assert!(f[0].y > -0.06 && f[1].y > -0.06);
        }
    }

    #[test]
    fn forced_wheel_is_full_at_low_speed() {
        for speed in [0.0, 6.0, 15.0, 25.0] {
            let p = OstrichPose {
                speed,
                wheel: 1.0,
                ..Default::default()
            };
            assert!(rig(&p).tw > 0.99, "speed {speed}");
            assert!(
                rig(&OstrichPose {
                    speed,
                    ..Default::default()
                })
                .tw < 0.01
            );
        }
        // Half a wheel is half blended; trumping is always a full wheel.
        let half = rig(&OstrichPose {
            speed: 15.0,
            wheel: 0.5,
            ..Default::default()
        })
        .tw;
        assert!((half - 0.5).abs() < 1e-3);
        assert!(
            rig(&OstrichPose {
                speed: 15.0,
                trumping: true,
                ..Default::default()
            })
            .tw > 0.99
        );
        // At speed the feet really ride the wheel circle: they leave the ground by
        // a wheel's diameter, which a running stride at 15 m/s does not.
        let lift = |wheel: f32| {
            let y: Vec<f32> = (0..32)
                .map(|k| {
                    foot_positions(&OstrichPose {
                        speed: 15.0,
                        wheel,
                        gait_phase: k as f32 / 32.0,
                        ..Default::default()
                    })[0]
                        .y
                })
                .collect();
            y.iter().cloned().fold(f32::MIN, f32::max) - y.iter().cloned().fold(f32::MAX, f32::min)
        };
        assert!(
            lift(1.0) > 0.85 && lift(1.0) > lift(0.0) + 0.3,
            "{} vs {}",
            lift(1.0),
            lift(0.0)
        );
    }

    #[test]
    fn forced_wheel_spins_fast_at_every_drift_speed() {
        // The sim's gait cycles a second: speed over stride (1.8 m standing to 6 m at top speed).
        for speed in (4..=60).map(|s| s as f32) {
            let cycles = speed / (1.8 + 4.2 * (speed / 32.0).min(1.0));
            let revs = cycles * wheel_turns_per_cycle(speed);
            assert!((4.5..=12.5).contains(&revs), "speed {speed}: {revs} rev/s");
            assert_eq!(wheel_turns_per_cycle(speed).fract(), 0.0);
        }
        // A whole number of turns per cycle: no jump where the gait phase wraps.
        let at = |g: f32| {
            foot_positions(&OstrichPose {
                speed: 12.0,
                wheel: 1.0,
                gait_phase: g,
                ..Default::default()
            })
        };
        let (a, b) = (at(0.9999), at(0.0));
        assert!((a[0] - b[0]).length() < 0.02 && (a[1] - b[1]).length() < 0.02);
    }

    #[test]
    fn forced_wheel_gets_camera_facing_arcs_only_with_a_camera() {
        let pose = OstrichPose { speed: 20.0, wheel: 1.0, ..Default::default() };
        let plain = turbo_fx(&pose, 0.3, None).len();
        let seen = turbo_fx(&pose, 0.3, Some(Vec3::new(0.0, 4.0, -7.0))).len();
        assert!(seen > plain + 20, "{seen} vs {plain}");
        // No forced wheel, no arcs.
        let still = OstrichPose { speed: 20.0, ..Default::default() };
        assert_eq!(turbo_fx(&still, 0.3, Some(Vec3::new(0.0, 4.0, -7.0))).len(), turbo_fx(&still, 0.3, None).len());
    }

    #[test]
    fn drift_tier_tints_the_wheel() {
        let beams = |tier: u8| -> Vec<Rgb> {
            turbo_fx(
                &OstrichPose {
                    speed: 20.0,
                    wheel: 1.0,
                    drift_tier: tier,
                    ..Default::default()
                },
                0.3,
                None,
            )
            .into_iter()
            .filter_map(|f| {
                if let Fx::Beam { color, width, .. } = f {
                    (width > 0.09).then_some(color)
                } else {
                    None
                }
            })
            .collect()
        };
        let legs = OstrichPose::default().legs;
        // Tier 0 keeps the leg colour; each tier brings its own colour to the ring.
        assert!(beams(0).contains(&legs));
        for (tier, want) in [
            (1u8, [0.25, 0.55, 1.0]),
            (2, [1.0, 0.55, 0.12]),
            (3, [0.75, 0.25, 1.0]),
        ] {
            assert!(beams(tier).contains(&want), "tier {tier}");
        }
    }

    #[test]
    fn trump_beam_runs_eight_metres_back_from_the_tail() {
        assert!(trump_fx(&OstrichPose::default(), 0.2).is_empty());
        for yaw in [0.0f32, 1.1, -2.4] {
            let p = OstrichPose {
                trumping: true,
                yaw,
                pos: Vec3::new(3.0, 0.0, -5.0),
                speed: 50.0,
                ..Default::default()
            };
            let src = tail_point(&p);
            let back = -Vec3::new(yaw.sin(), 0.0, yaw.cos());
            // The source is behind the body centre, at rump height.
            assert!(
                (src - p.pos).dot(back) > 0.5 && src.y > 0.8 && src.y < 2.2,
                "{src:?}"
            );
            let fx = trump_fx(&p, 0.2);
            let mut far = 0.0f32;
            for f in &fx {
                if let Fx::Beam {
                    from,
                    to,
                    intensity,
                    width,
                    ..
                } = f
                {
                    assert!(*intensity > 0.0 && *width > 0.0);
                    for v in [from, to] {
                        far = far.max((*v - src).dot(back));
                        // Straight back along the heading.
                        assert!(((*v - src) - back * (*v - src).dot(back)).length() < 0.01);
                    }
                }
            }
            assert!((far - TRUMP_BEAM_LEN).abs() < 0.5, "beam reaches {far} m");
            // A glare at the source.
            assert!(fx
                .iter()
                .any(|f| matches!(f, Fx::Glow { pos, .. } if (*pos - src).length() < 0.01)));
        }
        // The beam flickers.
        let p = OstrichPose {
            trumping: true,
            ..Default::default()
        };
        let sum = |t: f32| {
            trump_fx(&p, t)
                .iter()
                .map(|f| {
                    if let Fx::Beam { intensity, .. } = f {
                        *intensity
                    } else {
                        0.0
                    }
                })
                .sum::<f32>()
        };
        assert!((sum(0.0) - sum(0.013)).abs() > 0.05);
    }

    #[test]
    fn trumping_pitches_the_body_forward() {
        let pitch = |trumping: bool| {
            rig(&OstrichPose {
                speed: 40.0,
                trumping,
                ..Default::default()
            })
            .body_pitch
        };
        assert!(pitch(true) > pitch(false) + 0.05);
    }

    // ---- Cluster Flock: the jockey-less floppy Nostrich ----

    #[test]
    fn the_default_bird_has_no_jockey_or_props() {
        for style in 0..6u8 {
            let p = OstrichPose::for_player(style);
            assert!(!p.jockey);
            for part in ostrich_parts(&p, 1.3) {
                assert!(
                    !matches!(
                        part.mesh,
                        MeshId::JockeyHead | MeshId::JockeyHelmet | MeshId::Jersey | MeshId::Saddle | MeshId::LaptopBase | MeshId::LaptopLid | MeshId::CakeSlice | MeshId::Plate | MeshId::Shell | MeshId::Flame
                    ),
                    "{:?} must not be drawn by default",
                    part.mesh
                );
            }
        }
    }

    #[test]
    fn six_players_look_different_and_have_different_accessories() {
        let styles: Vec<PlayerStyle> = (0..6).map(PlayerStyle::of).collect();
        for i in 0..6 {
            for j in (i + 1)..6 {
                assert_ne!(styles[i].accessory, styles[j].accessory, "players {i} and {j} share an accessory shape");
                let d: f32 = (0..3).map(|k| (styles[i].body[k] - styles[j].body[k]).abs()).sum();
                assert!(d > 0.5, "players {i} and {j} have similar body colours");
            }
        }
        assert_eq!(PlayerStyle::of(6).name, PlayerStyle::of(0).name);
    }

    fn pose_head_y(p: &OstrichPose) -> f32 {
        head_transform(p).transform_point3(Vec3::ZERO).y
    }

    #[test]
    fn squash_crouches_and_the_belly_flop_lies_on_the_ground() {
        let base = OstrichPose { speed: 8.0, ..Default::default() };
        let squashed = OstrichPose { squash: 1.0, ..base };
        let flop = OstrichPose { collapsed: 1.0, ..base };
        assert!(pose_head_y(&squashed) < pose_head_y(&base) - 0.3, "squash lowers the head");
        assert!(pose_head_y(&flop) < 1.1, "belly-flop keeps the head low: {}", pose_head_y(&flop));
        // The body (the first part) lies on the ground and is flattened.
        let body = |p: &OstrichPose| ostrich_parts(p, 0.0)[0].model;
        assert!(body(&flop).transform_point3(Vec3::ZERO).y < 0.5);
        assert!(body(&flop).y_axis.length() < body(&base).y_axis.length());
        // Legs trail out behind: the feet end up behind the hips.
        let feet = foot_positions(&flop);
        assert!(feet[0].z < -0.5 && feet[1].z < -0.5, "{feet:?}");
    }

    #[test]
    fn neck_sway_and_roll_move_the_head_the_right_way() {
        let base = OstrichPose { speed: 10.0, ..Default::default() };
        let c = |p: &OstrichPose| head_transform(p).transform_point3(Vec3::ZERO);
        // Sway + = the runner's right = -x.
        assert!(c(&OstrichPose { neck_sway: glam::Vec2::new(1.0, 0.0), ..base }).x < c(&base).x - 0.15);
        assert!(c(&OstrichPose { neck_sway: glam::Vec2::new(-1.0, 0.0), ..base }).x > c(&base).x + 0.15);
        assert!(c(&OstrichPose { neck_sway: glam::Vec2::new(0.0, 1.0), ..base }).z > c(&base).z + 0.1);
        // Body roll + = right: the head goes to -x.
        assert!(c(&OstrichPose { body_roll: 0.3, ..base }).x < c(&base).x - 0.05);
        // Nose-down pitch moves the head forward and down.
        let pitched = c(&OstrichPose { body_pitch: 0.4, ..base });
        assert!(pitched.y < c(&base).y && pitched.z > c(&base).z);
    }

    #[test]
    fn tumble_is_a_roll_angle_and_keeps_the_bird_above_the_ground() {
        let p = OstrichPose { tumble: std::f32::consts::FRAC_PI_2, ..Default::default() };
        let parts = ostrich_parts(&p, 0.3);
        for part in &parts {
            let c = part.model.transform_point3(Vec3::ZERO);
            assert!(c.y > -0.05, "{:?} below the ground at {c:?}", part.mesh);
        }
        // Lying on its side the head is well to the side of the body.
        assert!(head_transform(&p).transform_point3(Vec3::ZERO).x.abs() > 0.8);
        // A whole turn comes back to upright.
        let full = OstrichPose { tumble: TAU, ..Default::default() };
        let up = OstrichPose::default();
        assert!((head_transform(&full).transform_point3(Vec3::ZERO) - head_transform(&up).transform_point3(Vec3::ZERO)).length() < 1e-3);
    }

    #[test]
    fn stars_wings_and_ghost_change_the_parts_as_specified() {
        let count = |p: &OstrichPose, m: MeshId| ostrich_parts(p, 0.7).iter().filter(|x| x.mesh == m).count();
        let base = OstrichPose::default();
        assert_eq!(count(&base, MeshId::Star), 0);
        assert_eq!(count(&OstrichPose { stars: 1.0, ..base }, MeshId::Star), 5, "4-5 stars circle the head");
        // Ghost: no thighs or feet, a wavy tail instead, floating above the ground.
        let ghost = OstrichPose { ghost: 1.0, ..base };
        assert_eq!(count(&ghost, MeshId::Thigh) + count(&ghost, MeshId::Foot), 0);
        assert!(count(&ghost, MeshId::NeckSeg) > count(&base, MeshId::NeckSeg) + 4);
        assert!(pose_head_y(&ghost) > pose_head_y(&base) + 0.7);
        // The tail sways with time.
        let tail = |t: f32| ostrich_parts(&ghost, t).iter().rfind(|x| x.mesh == MeshId::NeckSeg).unwrap().model.w_axis;
        assert!((tail(0.0) - tail(0.5)).length() > 0.05);
        // Wing flap moves the wings over time, a still bird does not flap.
        let wing = |p: &OstrichPose, t: f32| ostrich_parts(p, t).iter().find(|x| x.mesh == MeshId::WingL).unwrap().model.transform_point3(Vec3::new(0.0, -0.8, 0.0));
        let flapping = OstrichPose { wing_flap: 1.0, ..base };
        assert!((wing(&flapping, 0.0) - wing(&flapping, 0.08)).length() > 0.1);
        // Scrambling legs flail: the feet move with time even when the body stands still.
        let scr = OstrichPose { scramble: 1.0, airborne: true, ..base };
        let foot = |t: f32| ostrich_parts(&scr, t).iter().find(|x| x.mesh == MeshId::Foot).unwrap().model.w_axis;
        assert!((foot(0.0) - foot(0.1)).length() > 0.1);
    }

    #[test]
    fn floppy_inputs_stay_finite_across_the_whole_range() {
        let mut n = 0;
        for k in 0..64 {
            let t = k as f32 / 63.0;
            let p = OstrichPose {
                squash: t,
                collapsed: 1.0 - t,
                neck_sway: glam::Vec2::new(2.0 * t - 1.0, 1.0 - 2.0 * t),
                body_roll: 1.0 - 2.0 * t,
                body_pitch: 0.5 * t,
                tumble: TAU * t,
                wing_flap: t,
                scramble: 1.0 - t,
                stars: t,
                ghost: if k % 3 == 0 { t } else { 0.0 },
                speed: 30.0 * t,
                airborne: k % 2 == 0,
                tuck: t,
                ..OstrichPose::for_player((k % 6) as u8)
            };
            for part in ostrich_parts(&p, 5.1 * t) {
                n += 1;
                assert!(part.model.to_cols_array().iter().all(|v| v.is_finite()), "{:?} at {k}", part.mesh);
            }
        }
        assert!(n > 1500);
    }
}

