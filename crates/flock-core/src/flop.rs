//! Purely visual floppiness (not networked). The app keeps one [`Flop`] per
//! runner and calls [`Flop::step`] every rendered frame (or every tick) with
//! the latest (interpolated) body. Deterministic: no RNG, no wall clock.
//!
//! Sign conventions: angles in radians; `+` sway/roll leans toward sim `+x`
//! (the runner's right — remember the presenter maps sim `x` to render `-x`);
//! `+` pitch leans forward (+z).

use serde::{Deserialize, Serialize};

use crate::runner::{Loco, Runner};

/// Damped spring on one scalar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Spring {
    pub x: f32,
    pub v: f32,
}

impl Spring {
    pub fn at(x: f32) -> Self {
        Self { x, v: 0.0 }
    }
    /// Semi-implicit step toward `target` with stiffness `k` and damping ratio `zeta`.
    pub fn step(&mut self, target: f32, k: f32, zeta: f32, dt: f32) {
        let a = k * (target - self.x) - 2.0 * zeta * k.sqrt() * self.v;
        self.v += a * dt;
        self.x += self.v * dt;
        if !self.x.is_finite() || !self.v.is_finite() {
            *self = Spring::at(target);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Flop {
    // ---------------------------------------------------------- outputs
    /// Neck sideways swing (lags lateral acceleration; big on lane lurches).
    pub neck_sway: f32,
    /// Neck forward/back pitch (recoils on knockback, reaches in the air).
    pub neck_pitch: f32,
    /// Neck length factor (≈0.8 crouched .. 1.25 stretched).
    pub neck_stretch: f32,
    pub body_roll: f32,
    pub body_pitch: f32,
    /// Vertical scale (1 neutral; <1 squash, >1 stretch). Scale x/z by `1/sqrt(squash)`.
    pub squash: f32,
    /// 0 folded .. 1 fully spread.
    pub wing_open: f32,
    /// Current wing flap angle (oscillates; amplitude grows with `wing_open`).
    pub wing_flap: f32,
    /// 0 calm trot .. 1 frantic scramble.
    pub leg_scramble: f32,
    /// Stride phase (radians, wraps).
    pub leg_phase: f32,
    /// Somersault angle about the body's forward axis while tumbling (radians).
    pub tumble_angle: f32,
    /// Small vertical head bob (m).
    pub head_bob: f32,

    // ---------------------------------------------------------- state
    prev_vel: Option<[f32; 3]>,
    prev_grounded: bool,
    prev_loco: u8,
    neck: Spring,
    pitch: Spring,
    stretch: Spring,
    roll: Spring,
    bpitch: Spring,
    sq: Spring,
    wings: Spring,
    flap_phase: f32,
    scramble: Spring,
    tumble: Spring,
    tumbling: bool,
}

impl Default for Flop {
    fn default() -> Self {
        Self::new()
    }
}

fn loco_code(l: &Loco) -> u8 {
    match l {
        Loco::Running => 0,
        Loco::WindUp { .. } => 1,
        Loco::Airborne { voluntary: true, .. } => 2,
        Loco::Airborne { voluntary: false, .. } => 3,
        Loco::Collapsed { .. } => 4,
        Loco::Tumbling { .. } => 5,
        Loco::Recovering { .. } => 6,
    }
}

fn wrap_pi(a: f32) -> f32 {
    let t = std::f32::consts::TAU;
    let mut x = a % t;
    if x > std::f32::consts::PI {
        x -= t;
    } else if x < -std::f32::consts::PI {
        x += t;
    }
    x
}

impl Flop {
    pub fn new() -> Self {
        Self {
            neck_sway: 0.0,
            neck_pitch: 0.0,
            neck_stretch: 1.0,
            body_roll: 0.0,
            body_pitch: 0.0,
            squash: 1.0,
            wing_open: 0.1,
            wing_flap: 0.0,
            leg_scramble: 0.3,
            leg_phase: 0.0,
            tumble_angle: 0.0,
            head_bob: 0.0,
            prev_vel: None,
            prev_grounded: true,
            prev_loco: 0,
            neck: Spring::default(),
            pitch: Spring::default(),
            stretch: Spring::at(1.0),
            roll: Spring::default(),
            bpitch: Spring::default(),
            sq: Spring::at(1.0),
            wings: Spring::at(0.1),
            flap_phase: 0.0,
            scramble: Spring::at(0.3),
            tumble: Spring::default(),
            tumbling: false,
        }
    }

    /// Advance by `dt` seconds from the runner's current body. `v_course` is
    /// not needed: forward speed changes are read from `vel.z_off`.
    pub fn step(&mut self, runner: &Runner, dt: f32) {
        if dt <= 0.0 || !dt.is_finite() {
            return;
        }
        let dt = dt.min(0.1);
        let vel = [runner.vel.x, runner.vel.y, runner.vel.z_off];
        let pv = self.prev_vel.unwrap_or(vel);
        let acc = [
            ((vel[0] - pv[0]) / dt).clamp(-200.0, 200.0),
            ((vel[1] - pv[1]) / dt).clamp(-400.0, 400.0),
            ((vel[2] - pv[2]) / dt).clamp(-200.0, 200.0),
        ];
        self.prev_vel = Some(vel);
        let code = loco_code(&runner.loco);
        let entered = code != self.prev_loco;
        let grounded = runner.pos.y <= 1e-3;

        // Landing thump: squash proportional to impact speed.
        if grounded && !self.prev_grounded {
            let impact = (-pv[1]).max(0.0);
            self.sq.v -= impact * 0.55;
            self.pitch.v += impact * 0.4;
        }
        // Event kicks.
        if entered {
            match runner.loco {
                Loco::Airborne { voluntary, .. } => {
                    self.sq.v += 3.0;
                    self.neck.v += if voluntary { 0.0 } else { 6.0 };
                    self.wings.v += 8.0;
                }
                Loco::Collapsed { .. } => {
                    self.bpitch.v += 6.0;
                    self.neck.v += 5.0 * if runner.vel.x >= 0.0 { -1.0 } else { 1.0 };
                }
                Loco::Recovering { .. } => {
                    // Stand-up wobble.
                    self.roll.v += 5.0;
                    self.neck.v -= 7.0;
                    self.sq.v += 2.0;
                }
                Loco::Running if self.prev_loco == 6 => {
                    self.roll.v -= 2.5;
                }
                _ => {}
            }
        }
        self.prev_loco = code;
        self.prev_grounded = grounded;

        let (mut sq_t, mut stretch_t, mut wings_t, mut scramble_t, mut bpitch_t) = (1.0, 1.0, 0.12, 0.35, 0.08);
        let mut flap_hz = 2.5;
        match runner.loco {
            Loco::Running => {
                scramble_t = if runner.in_mud { 1.0 } else { 0.35 };
                wings_t = if runner.in_mud { 0.6 } else { 0.12 + (acc[0].abs() * 0.01).min(0.5) };
            }
            Loco::WindUp { kind, .. } => {
                if kind.is_jump() {
                    sq_t = 0.68;
                    stretch_t = 0.8;
                    wings_t = 0.4;
                } else {
                    sq_t = 0.8;
                    bpitch_t = 0.5;
                }
            }
            Loco::Airborne { voluntary, .. } => {
                sq_t = if runner.vel.y > 0.0 { 1.18 } else { 0.95 };
                stretch_t = 1.22;
                wings_t = 1.0;
                flap_hz = if voluntary { 9.0 } else { 15.0 };
                scramble_t = if voluntary { 0.6 } else { 1.0 };
            }
            Loco::Collapsed { .. } => {
                sq_t = 0.55;
                stretch_t = 0.85;
                wings_t = 0.7;
                flap_hz = 5.0;
                scramble_t = 0.15;
                bpitch_t = 1.1;
            }
            Loco::Tumbling { .. } => {
                sq_t = 0.8;
                stretch_t = 1.1;
                wings_t = 1.0;
                flap_hz = 13.0;
                scramble_t = 1.0;
                bpitch_t = 0.4;
            }
            Loco::Recovering { .. } => {
                sq_t = 0.9;
                wings_t = 0.5;
                flap_hz = 6.0;
                scramble_t = 0.8;
            }
        }

        // Neck: lags lateral acceleration (lurches throw it sideways), recoils on knockback.
        let neck_t = (-acc[0] * 0.018).clamp(-1.0, 1.0);
        self.neck.step(neck_t, 70.0, 0.22, dt);
        let pitch_t = (runner.knock * -0.06 - acc[2] * 0.01 + if runner.loco == Loco::Running { 0.15 } else { 0.0 }).clamp(-0.9, 0.9);
        self.pitch.step(pitch_t, 80.0, 0.3, dt);
        self.stretch.step(stretch_t, 120.0, 0.35, dt);
        // Body: rolls into lateral motion, overshoots.
        let roll_t = (acc[0] * 0.01 + runner.vel.x * 0.035).clamp(-0.7, 0.7);
        self.roll.step(roll_t, 110.0, 0.3, dt);
        self.bpitch.step(bpitch_t, 90.0, 0.4, dt);
        self.sq.step(sq_t, 260.0, 0.28, dt);
        self.wings.step(wings_t, 150.0, 0.5, dt);
        self.scramble.step(scramble_t, 60.0, 0.9, dt);

        // Tumble: spin while tumbling, then settle (wobbling) to the nearest upright.
        if let Loco::Tumbling { spin_dir, .. } = runner.loco {
            if !self.tumbling {
                self.tumble.x = wrap_pi(self.tumble.x);
            }
            self.tumbling = true;
            self.tumble.v = spin_dir as f32 * 11.0;
            self.tumble.x += self.tumble.v * dt;
        } else {
            if self.tumbling {
                self.tumbling = false;
                self.tumble.x = wrap_pi(self.tumble.x);
            }
            self.tumble.step(0.0, 90.0, 0.25, dt);
        }

        let open = self.wings.x.clamp(0.0, 1.2);
        self.flap_phase = (self.flap_phase + dt * flap_hz * std::f32::consts::TAU) % std::f32::consts::TAU;
        let speed = (runner.vel.z_off + 12.0).max(0.0);
        let scr = self.scramble.x.clamp(0.0, 1.0);
        self.leg_phase = (self.leg_phase + dt * (speed / 1.4) * (1.0 + scr) * std::f32::consts::TAU) % std::f32::consts::TAU;

        self.neck_sway = self.neck.x;
        self.neck_pitch = self.pitch.x;
        self.neck_stretch = self.stretch.x.clamp(0.6, 1.5);
        self.body_roll = self.roll.x;
        self.body_pitch = self.bpitch.x;
        self.squash = self.sq.x.clamp(0.45, 1.5);
        self.wing_open = open.min(1.0);
        self.wing_flap = self.flap_phase.sin() * (0.15 + 0.85 * open.min(1.0));
        self.leg_scramble = scr;
        self.tumble_angle = self.tumble.x;
        self.head_bob = self.leg_phase.sin() * 0.04 * (1.0 + scr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PlayerId, Runner};

    #[test]
    fn lane_lurch_throws_neck_and_settles() {
        let mut f = Flop::new();
        let mut r = Runner::new(PlayerId(0), 1, 0.0, 0.0, 3);
        let mut max_sway = 0.0f32;
        for i in 0..120 {
            r.vel.x = if i < 15 { i as f32 * 0.8 } else { 0.0 };
            f.step(&r, 1.0 / 60.0);
            max_sway = max_sway.max(f.neck_sway.abs());
        }
        assert!(max_sway > 0.2, "neck should swing, got {max_sway}");
        for _ in 0..240 {
            f.step(&r, 1.0 / 60.0);
        }
        assert!(f.neck_sway.abs() < 0.05);
        assert!(f.neck_sway.is_finite() && f.squash.is_finite());
    }

    #[test]
    fn tumble_spins_then_uprights() {
        let mut f = Flop::new();
        let mut r = Runner::new(PlayerId(0), 1, 0.0, 0.0, 3);
        r.loco = Loco::Tumbling { until: 100, spin_dir: 1 };
        for _ in 0..30 {
            f.step(&r, 1.0 / 60.0);
        }
        assert!(f.tumble_angle.abs() > 1.0);
        r.loco = Loco::Running;
        for _ in 0..300 {
            f.step(&r, 1.0 / 60.0);
        }
        assert!(f.tumble_angle.abs() < 0.05);
    }
}
