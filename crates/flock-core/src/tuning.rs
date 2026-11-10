//! All gameplay numbers (SPEC §3). `Default` gives the SPEC defaults; the
//! struct is JSON-loadable and every field is optional in JSON
//! (`#[serde(default)]`), so a tuning file only needs the values it changes.

use serde::{Deserialize, Serialize};

use crate::TICK_HZ;

/// Hazard geometry (SPEC §3, "Hazard geometry").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HazardGeom {
    pub hay_top: f32,
    pub hay_depth: f32,
    pub hay_width: f32,
    /// Height of the beam's underside (the visual and the collision agree).
    pub beam_underside: f32,
    /// Beam thickness: its collision/visual top is `beam_underside + beam_thickness`
    /// (kept above the jump apex so a jump can never clear it).
    pub beam_thickness: f32,
    pub beam_depth: f32,
    /// Width of a one-lane beam (an all-lanes beam spans fence to fence).
    pub beam_width: f32,
    pub block_height: f32,
    pub block_depth: f32,
    pub block_width: f32,
    pub mud_depth: f32,
    pub mud_width: f32,
    pub coin_radius: f32,
    pub collect_radius: f32,
    pub coin_height: f32,
    /// Spacing of the five coins in a ghost-placed lure line.
    pub lure_spacing: f32,
    /// Barrel (a lying cylinder that rolls along its lane): radius = top
    /// height = half its depth along z.
    pub barrel_radius: f32,
    /// Barrel width across the lane.
    pub barrel_width: f32,
    /// Roll speed (m/s) of a barrel rolling toward the runners (towards -z).
    pub barrel_speed_toward: f32,
    /// Roll speed (m/s) of a barrel rolling away (towards +z).
    pub barrel_speed_away: f32,
    /// A barrel starts rolling when the leading runner is this far (m) behind its home z.
    pub barrel_trigger_m: f32,
    /// A barrel stops (and rests) after rolling this long (s).
    pub barrel_travel_s: f32,
}

impl HazardGeom {
    /// Top of a beam (collision and visual).
    pub fn beam_top(&self) -> f32 {
        self.beam_underside + self.beam_thickness
    }
}

impl Default for HazardGeom {
    fn default() -> Self {
        Self {
            hay_top: 0.70,
            hay_depth: 1.0,
            hay_width: 1.6,
            beam_underside: 1.15,
            beam_thickness: 0.8,
            beam_depth: 0.4,
            beam_width: 2.0,
            block_height: 2.5,
            block_depth: 3.0,
            block_width: 2.0,
            mud_depth: 6.0,
            mud_width: 2.0,
            coin_radius: 0.35,
            collect_radius: 0.6,
            coin_height: 0.8,
            lure_spacing: 1.2,
            barrel_radius: 0.35,
            barrel_width: 1.2,
            barrel_speed_toward: 4.0,
            barrel_speed_away: 3.0,
            barrel_trigger_m: 24.0,
            barrel_travel_s: 6.0,
        }
    }
}

/// Ghost tool energy costs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolCosts {
    pub coins: f32,
    pub mud: f32,
    pub hay: f32,
    pub beam: f32,
}

impl Default for ToolCosts {
    fn default() -> Self {
        Self { coins: 1.0, mud: 2.0, hay: 3.0, beam: 4.0 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tuning {
    pub lane_width: f32,
    pub fence_x: f32,
    pub body_radius: f32,
    pub height_stand: f32,
    pub height_low: f32,
    /// Runner-runner contacts use the torso band `[y, y + torso_height]`
    /// (capped by the body's own height); neck and head do not block, so an
    /// airborne bird whose feet are above another's torso passes over it.
    pub torso_height: f32,
    pub gravity: f32,
    pub jump_vy: f32,
    pub jump_windup: f32,
    pub jump_fwd_boost: f32,
    pub boost_tau: f32,
    pub down_windup: f32,
    pub collapse_time: f32,
    pub collapse_fwd_loss: f32,
    pub standup_time: f32,
    pub lateral_k: f32,
    pub lateral_zeta: f32,
    pub lateral_amax: f32,
    /// Time the feasibility DP allows per lane of lateral travel (SPEC §4).
    /// Measured with the default spring: within 0.3 m of the target lane
    /// after 0.27 s, at the centre after 0.30 s.
    pub lane_change_s: f32,
    /// Reaction slack (s) the feasibility DP leaves around every action and
    /// lane change, so generated/architect routes never need frame-perfect play.
    pub feas_reaction_s: f32,
    pub diag_jump_fwd_bias: f32,
    pub diag_dive_back_bias: f32,
    pub input_buffer: f32,
    /// App-side keyboard diagonal window; carried here so one file tunes all.
    pub combo_window: f32,
    pub course_v0: f32,
    pub course_vmax: f32,
    pub course_ramp_m: f32,
    pub mp_v_end: f32,
    /// Speed (m/s) each collected coin adds to the runner's personal bonus.
    pub coin_speed: f32,
    /// Cap of the personal coin speed bonus (m/s); a damaging crash halves it.
    pub coin_speed_cap: f32,
    /// Fraction of `jump_fwd_boost` the feasibility DP adds to the course speed
    /// as its "max plausible speed" (the boost decays within a jump, so the
    /// full value would over-pad every row).
    pub plausible_boost_frac: f32,
    pub pack_k: f32,
    pub pack_band: f32,
    pub pack_anchor_follow: f32,
    pub bump_restitution: f32,
    pub significant_impact: f32,
    pub tumble_impulse: f32,
    pub tumble_time: f32,
    pub ramp_hop_vy: f32,
    pub feathers: u8,
    pub protect_time: f32,
    pub crash_knockback: f32,
    /// Decay time constant of knockback / shove forward velocity.
    pub knock_tau: f32,
    pub mud_speed_mult: f32,
    pub mud_wobble: f32,
    pub countdown: f32,
    pub run_in: f32,
    /// Multiplayer clear run-in (longer than solo so the start grid spreads out).
    pub mp_run_in: f32,
    /// Metres after the run-in with only single-lane hay/beam patterns (multiplayer).
    pub mp_gentle_m: f32,
    /// Metres after the gentle stretch over which the full pattern pool is phased in.
    pub mp_ramp_m: f32,
    pub mp_cap: f32,
    pub row_spacing: f32,
    pub gen_ahead: f32,
    pub keep_behind: f32,
    pub band_near_s: f32,
    pub band_far_s: f32,
    pub min_reaction_s: f32,
    pub materialise_s: f32,
    pub ghost_energy_max: f32,
    pub ghost_energy_start: f32,
    pub ghost_energy_regen: f32,
    pub cost: ToolCosts,
    pub density_per_row: u8,
    pub density_per_4_rows: u8,
    pub hazards: HazardGeom,
    /// Seconds of protection after which a `LongProtection` event is logged.
    pub long_protection_log_s: f32,
    /// Debug: generate no procedural hazards or coins at all.
    pub debug_empty_course: bool,
    /// Solo generator difficulty ramp (m): row difficulty
    /// `1 - e^(-(row_z - run_in) / solo_difficulty_ramp_m)`.
    pub solo_difficulty_ramp_m: f32,
    /// Solo generator difficulty never uses fewer metres than this (175 m,
    /// difficulty ~0.11: what 0.1.x produced for its whole first 220 m batch).
    pub solo_difficulty_floor_m: f32,
    /// Mean metres between power-ups in solo (0 disables).
    pub powerup_gap_m: f32,
    /// Mean metres between power-ups in multiplayer (0 disables).
    pub mp_powerup_gap_m: f32,
    /// Seeded jitter of each power-up gap (0.35 = ±35 %).
    pub powerup_jitter: f32,
    /// Spawn weights: Magnet, Doubler, Shield.
    pub powerup_weights: [u32; 3],
    /// Magnet duration (s).
    pub magnet_s: f32,
    /// Magnet reach across lanes, in lane widths (from the body centre).
    pub magnet_reach_lanes: f32,
    /// Magnet reach ahead / behind (m), any height.
    pub magnet_reach_z: f32,
    /// Doubler duration (s): each coin counts 2.
    pub doubler_s: f32,
    /// Longest a shield is held (s) before it fades unused.
    pub shield_s: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            lane_width: 2.2,
            fence_x: 3.7,
            body_radius: 0.42,
            height_stand: 1.5,
            height_low: 0.55,
            torso_height: 1.1,
            gravity: 28.0,
            jump_vy: 9.0,
            jump_windup: 0.09,
            jump_fwd_boost: 6.0,
            boost_tau: 0.6,
            down_windup: 0.05,
            collapse_time: 0.70,
            collapse_fwd_loss: 6.0,
            standup_time: 0.30,
            lateral_k: 140.0,
            lateral_zeta: 0.5,
            lateral_amax: 60.0,
            lane_change_s: 0.32,
            feas_reaction_s: 0.15,
            diag_jump_fwd_bias: 1.0,
            diag_dive_back_bias: -3.0,
            input_buffer: 0.15,
            combo_window: 0.08,
            course_v0: 13.0,
            course_vmax: 22.0,
            course_ramp_m: 2500.0,
            mp_v_end: 16.0,
            coin_speed: 0.05,
            coin_speed_cap: 3.0,
            plausible_boost_frac: 0.36,
            pack_k: 0.5,
            pack_band: 7.0,
            pack_anchor_follow: 0.5,
            bump_restitution: 0.35,
            significant_impact: 2.5,
            tumble_impulse: 6.0,
            tumble_time: 1.2,
            ramp_hop_vy: 6.0,
            feathers: 3,
            protect_time: 4.0,
            crash_knockback: 6.0,
            knock_tau: 0.35,
            mud_speed_mult: 0.7,
            mud_wobble: 1.0,
            countdown: 3.0,
            run_in: 45.0,
            mp_run_in: 75.0,
            mp_gentle_m: 230.0,
            mp_ramp_m: 200.0,
            mp_cap: 180.0,
            row_spacing: 6.0,
            gen_ahead: 220.0,
            keep_behind: 30.0,
            band_near_s: 3.0,
            band_far_s: 5.0,
            min_reaction_s: 1.6,
            materialise_s: 0.5,
            ghost_energy_max: 10.0,
            ghost_energy_start: 3.0,
            ghost_energy_regen: 0.3,
            cost: ToolCosts::default(),
            density_per_row: 1,
            density_per_4_rows: 2,
            hazards: HazardGeom::default(),
            long_protection_log_s: 8.0,
            debug_empty_course: false,
            solo_difficulty_ramp_m: 1500.0,
            solo_difficulty_floor_m: 175.0,
            powerup_gap_m: 240.0,
            mp_powerup_gap_m: 160.0,
            powerup_jitter: 0.35,
            powerup_weights: [40, 35, 25],
            magnet_s: 8.0,
            magnet_reach_lanes: 1.15,
            magnet_reach_z: 2.5,
            doubler_s: 10.0,
            shield_s: 20.0,
        }
    }
}

impl Tuning {
    /// Seconds → whole ticks (rounded, at least 1 for positive durations).
    pub fn ticks(&self, secs: f32) -> u64 {
        if secs <= 0.0 || !secs.is_finite() {
            return 0;
        }
        ((secs * TICK_HZ as f32).round() as u64).max(1)
    }

    /// Margin the DP adds to `v_course`: the full coin bonus plus a decayed share
    /// of the jump boost.
    pub fn plausible_margin(&self) -> f32 {
        self.coin_speed_cap.max(0.0) + self.plausible_boost_frac * self.jump_fwd_boost
    }

    /// Lane centre `x` for lane 0..=2.
    pub fn lane_x(&self, lane: u8) -> f32 {
        (lane.min(2) as f32 - 1.0) * self.lane_width
    }

    /// Nearest lane to a lateral position.
    pub fn nearest_lane(&self, x: f32) -> u8 {
        let l = (x / self.lane_width + 1.0).round();
        if l.is_finite() {
            l.clamp(0.0, 2.0) as u8
        } else {
            1
        }
    }

    /// Solo course speed at distance `d` (SPEC §3). Deterministic `exp`
    /// (`det::exp_f32`): identical on every platform.
    pub fn solo_speed(&self, d: f64) -> f32 {
        let d = d.max(0.0) as f32;
        self.course_v0 + (self.course_vmax - self.course_v0) * (1.0 - crate::det::exp_f32(-d / self.course_ramp_m))
    }

    /// Multiplayer course speed after `t` seconds of running.
    pub fn multi_speed(&self, t: f32) -> f32 {
        let f = if self.mp_cap > 0.0 { (t / self.mp_cap).clamp(0.0, 1.0) } else { 1.0 };
        self.course_v0 + (self.mp_v_end - self.course_v0) * f
    }

    pub fn tool_cost(&self, tool: crate::Tool) -> f32 {
        match tool {
            crate::Tool::Coins => self.cost.coins,
            crate::Tool::Mud => self.cost.mud,
            crate::Tool::Hay => self.cost.hay,
            crate::Tool::Beam => self.cost.beam,
        }
    }

    /// Parse a (possibly partial) JSON tuning file.
    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}
