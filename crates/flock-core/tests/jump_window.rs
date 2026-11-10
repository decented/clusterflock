//! Human-fairness tuning: the hay jump window and beam rules, measured on the
//! real discrete trajectory (a full `World` with a lone runner, one hazard,
//! and one command issued on each possible tick).

use flock_core::course::{Cell, Row, SolidKind};
use flock_core::feas::FeasParams;
use flock_core::*;

const P0: PlayerId = PlayerId(0);

/// Solo world at constant course speed `v` with an empty course except one
/// row holding `kind` in the runner's lane, ~45 m ahead.
fn world_with(kind: SolidKind, v: f32, bonus: f32) -> World {
    let mut c = MatchConfig::solo(5);
    c.tuning.debug_empty_course = true;
    c.tuning.course_v0 = v;
    c.tuning.course_vmax = v;
    let mut w = World::new(c);
    while w.phase() == Phase::Countdown {
        w.step(&[]);
    }
    w.debug_runner_mut(P0).unwrap().speed_bonus = bonus;
    let z = w.runner(P0).unwrap().pos.z;
    let idx = w.course().row_at(z + 45.0) + 1;
    let mut r = Row::empty(idx);
    r.cells[1] = Cell::EMPTY.with_solid(Some(kind));
    w.debug_course_mut().insert_row(idx, r);
    w
}

/// For every tick offset at which `cmd` can be issued, whether the runner
/// passes the hazard without a crash. Returns one flag per offset.
fn sweep(kind: SolidKind, v: f32, bonus: f32, cmd: Command) -> Vec<bool> {
    let mut base = world_with(kind, v, bonus);
    let hazard_z = base.course().row_z(base.course().row_at(base.runner(P0).unwrap().pos.z + 45.0) + 1);
    let mut out = Vec::new();
    let row = base.course().row_at(hazard_z + 0.1);
    let obj_z = |w: &World| w.course().row_objects(row).iter().find(|o| o.kind.is_solid()).map_or(hazard_z, |o| o.z);
    // Scan command offsets until the runner is about to meet the (possibly moving) obstacle.
    while base.runner(P0).unwrap().pos.z < obj_z(&base) - 1.0 {
        let mut w = base.clone();
        w.step(&[(P0, cmd)]);
        let mut crashed = false;
        for _ in 0..900 {
            crashed |= w.events().iter().any(|e| matches!(e, Event::Crash { .. }));
            if w.runner(P0).unwrap().pos.z > hazard_z + 40.0 {
                break;
            }
            w.step(&[]);
        }
        crashed |= w.events().iter().any(|e| matches!(e, Event::Crash { .. }));
        out.push(!crashed);
        base.step(&[]);
    }
    out
}

/// Longest contiguous run of `true` (seconds) — the window must be one piece.
fn window_secs(ok: &[bool]) -> f32 {
    let total = ok.iter().filter(|&&b| b).count();
    let (mut best, mut cur) = (0, 0);
    for &b in ok {
        cur = if b { cur + 1 } else { 0 };
        best = best.max(cur);
    }
    assert_eq!(best, total, "window is not contiguous");
    total as f32 * DT
}

/// The feasibility DP's window (exact discrete jump trajectory, constant
/// speed, no forward boost, 0.05 m clearance margin): the conservative figure
/// the generator and ghost validation rely on.
fn dp_window_secs(v: f64) -> f64 {
    let t = Tuning::default();
    let p = FeasParams::from_tuning(&t);
    let half = t.hazards.hay_depth as f64 * 0.5;
    let (lo, hi) = p.jump_window(-half, half, v);
    (hi - lo) / v
}

/// Top solo course speed and the largest coin bonus (what a runner can reach).
fn top() -> (f32, f32) {
    let t = Tuning::default();
    (t.course_vmax, t.coin_speed_cap)
}

#[test]
fn hay_jump_window_dp_figure_meets_targets() {
    let t = Tuning::default();
    let plausible_top = (t.course_vmax + t.plausible_margin()) as f64;
    let (w13, wtop) = (dp_window_secs(13.0), dp_window_secs(plausible_top));
    eprintln!("DP hay jump window: {w13:.3} s at 13 m/s, {wtop:.3} s at {plausible_top} m/s");
    assert!(w13 >= 0.22, "DP window at 13 m/s: {w13}");
    assert!(wtop >= 0.22, "DP window at top speed: {wtop}");
}

#[test]
fn hay_jump_window_is_generous_at_13_mps_and_at_top_speed_with_coin_bonus() {
    let (vt, bonus) = top();
    let w13 = window_secs(&sweep(SolidKind::Hay, 13.0, 0.0, Command::Jump));
    let wtop = window_secs(&sweep(SolidKind::Hay, vt, bonus, Command::Jump));
    eprintln!("hay jump window: {w13:.3} s at 13 m/s, {wtop:.3} s at {vt} m/s + {bonus} bonus");
    assert!(w13 >= 0.22, "window at 13 m/s: {w13}");
    assert!(wtop >= 0.22, "window at top speed: {wtop}");
}

#[test]
fn beams_cannot_be_jumped_at_any_speed_but_ducking_is_comfortable() {
    let (vt, bonus) = top();
    for (v, b) in [(13.0, 0.0), (18.0, 0.0), (vt, 0.0), (vt, bonus)] {
        let jump = sweep(SolidKind::Beam, v, b, Command::Jump);
        assert!(jump.iter().all(|&ok| !ok), "a jump cleared a beam at {v} m/s + {b} bonus");
        let duck = window_secs(&sweep(SolidKind::Beam, v, b, Command::Down));
        eprintln!("beam duck window at {v} m/s + {b}: {duck:.3} s");
        assert!(duck >= 0.25, "duck window at {v} m/s + {b}: {duck}");
    }
    // The apex never reaches the beam's collision top, whatever the speed.
    let t = Tuning::default();
    let apex = t.jump_vy * t.jump_vy / (2.0 * t.gravity);
    assert!(apex + 0.2 < t.hazards.beam_top(), "apex {apex} vs beam top {}", t.hazards.beam_top());
    assert!(t.hazards.beam_underside > t.height_low + 0.5, "ducked body fits under");
}

#[test]
fn barrel_windows_toward_and_away_at_13_mps_and_top_speed() {
    let (vt, bonus) = top();
    for kind in [SolidKind::BarrelToward, SolidKind::BarrelAway] {
        for (v, b) in [(13.0, 0.0), (vt, bonus)] {
            let ok = sweep(kind, v, b, Command::Jump);
            let w = window_secs(&ok);
            eprintln!("{kind:?} jump window at {v} m/s + {b}: {w:.3} s");
            assert!(w >= 0.18, "{kind:?} window at {v} + {b}: {w}");
            // Not jumping (a command offset of the whole sweep range) is a crash:
            // the sweep contains failing offsets.
            assert!(ok.iter().any(|&o| !o), "{kind:?}: every offset passed, the barrel is not an obstacle");
        }
    }
}

#[test]
fn barrel_without_a_jump_crashes() {
    for kind in [SolidKind::BarrelToward, SolidKind::BarrelAway] {
        let mut w = world_with(kind, 13.0, 0.0);
        let mut crashed = false;
        for _ in 0..900 {
            w.step(&[]);
            crashed |= w.events().iter().any(|e| matches!(e, Event::Crash { kind: ObjKind::Barrel, .. }));
        }
        assert!(crashed, "{kind:?} did not crash a runner that never jumped");
    }
}
