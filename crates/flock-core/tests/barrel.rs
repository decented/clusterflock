//! Rolling barrels: z(tick) is a pure function of the row data and the clock,
//! identical on a remote course; the generator never makes them infeasible.

use flock_core::course::{Cell, GenCtx, Row, SolidKind};
use flock_core::*;

const P0: PlayerId = PlayerId(0);

fn barrel_world(kind: SolidKind) -> (World, u32) {
    let mut c = MatchConfig::solo(9);
    c.tuning.debug_empty_course = true;
    let mut w = World::new(c);
    while w.phase() == Phase::Countdown {
        w.step(&[]);
    }
    let z = w.runner(P0).unwrap().pos.z;
    let idx = w.course().row_at(z + 120.0) + 1;
    let mut r = Row::empty(idx);
    r.cells[1] = Cell::EMPTY.with_solid(Some(kind));
    w.debug_course_mut().insert_row(idx, r);
    (w, idx)
}

fn barrel_opt(c: &Course, row: u32) -> Option<Obj> {
    c.row_objects(row).into_iter().find(|o| o.kind == ObjKind::Barrel)
}

fn barrel(c: &Course, row: u32) -> Obj {
    barrel_opt(c, row).expect("barrel")
}

#[test]
fn barrel_position_is_a_pure_function_of_release_tick_and_clock() {
    for (kind, dir) in [(SolidKind::BarrelToward, -1.0f64), (SolidKind::BarrelAway, 1.0)] {
        let t = Tuning::default();
        let speed = if dir < 0.0 { t.hazards.barrel_speed_toward } else { t.hazards.barrel_speed_away } as f64;
        let mut host = Course::new(&t);
        let mut remote = Course::new_remote(&t);
        let mut r = Row::empty(40);
        r.cells[2] = Cell::EMPTY.with_solid(Some(kind));
        host.insert_row(40, r);
        remote.insert_row(40, r);
        let home = host.row_z(40);
        // Not released: sits at home, still, whatever the clock says.
        host.set_clock(500);
        let o = barrel(&host, 40);
        assert_eq!((o.z, o.vz, o.roll, o.released), (home, 0.0, 0.0, false));
        assert_eq!(o.dir as f64, dir);
        assert!(host.release_barrel(40, 1000));
        assert!(!host.release_barrel(40, 1200), "first release wins");
        assert!(remote.release_barrel(40, 1000));
        for tick in [900u64, 1000, 1030, 1060, 1200, 1359, 1360, 1361, 5000] {
            host.set_clock(tick);
            remote.set_clock(tick);
            let (a, b) = (barrel(&host, 40), barrel(&remote, 40));
            assert_eq!(a, b, "tick {tick}");
            let e = (tick.saturating_sub(1000) as f64 / 60.0).min(t.hazards.barrel_travel_s as f64);
            assert!((a.z - (home + dir * speed * e)).abs() < 1e-4, "tick {tick}: z {} ", a.z);
            let rolling = tick >= 1000 && ((tick - 1000) as f64 / 60.0) < t.hazards.barrel_travel_s as f64;
            assert_eq!(a.vz != 0.0, rolling);
            // Roll angle = travelled distance / radius (positive = toward +z).
            assert!((a.roll as f64 - dir * speed * e / t.hazards.barrel_radius as f64).abs() < 1e-3);
        }
        // Rolls toward the runners: z decreases; away: increases.
        host.set_clock(1120);
        assert_eq!(barrel(&host, 40).z < home, dir < 0.0);
    }
}

#[test]
fn host_releases_barrels_when_the_leader_is_close_and_events_rebuild_the_remote_course() {
    let (mut w, idx) = barrel_world(SolidKind::BarrelToward);
    let t = w.tuning().clone();
    let mut remote = Course::new_remote(&t);
    for r in w.course().rows() {
        remote.insert_row(r.row_index, *r);
    }
    let home = w.course().row_z(idx);
    let mut released_at = None;
    let mut crashed = false;
    for _ in 0..60 * 20 {
        w.step(&[]);
        for e in w.events() {
            if let Event::BarrelReleased { row, tick } = *e {
                assert_eq!(row, idx);
                assert!(remote.release_barrel(row, tick));
                released_at = Some((w.tick(), w.runner(P0).unwrap().pos.z));
            }
            crashed |= matches!(e, Event::Crash { kind: ObjKind::Barrel, .. });
        }
        remote.set_clock(w.course().clock());
        // Identical host/remote positions on every tick.
        if let Some(b) = barrel_opt(w.course(), idx) {
            assert_eq!(b, barrel(&remote, idx));
        }
        if released_at.is_none() {
            assert_eq!(barrel(w.course(), idx).z, home, "still before release");
        }
    }
    let (_, z) = released_at.expect("released");
    assert!((home - z) <= t.hazards.barrel_trigger_m as f64 + 1.0 && (home - z) > 0.0, "released at gap {}", home - z);
    assert!(crashed, "a runner who never jumped is hit");
}

#[test]
fn barrels_keep_rolling_after_a_crash_and_cost_one_incident_each() {
    let (mut w, idx) = barrel_world(SolidKind::BarrelToward);
    let mut crashes = 0;
    let mut zs = Vec::new();
    for _ in 0..60 * 12 {
        w.step(&[]);
        crashes += w.events().iter().filter(|e| matches!(e, Event::Crash { kind: ObjKind::Barrel, .. })).count();
        if let Some(b) = barrel_opt(w.course(), idx) {
            zs.push(b.z);
        }
    }
    assert_eq!(crashes, 1, "one incident per runner per barrel");
    // It was still rolling (z strictly decreasing) while the crash happened and after.
    let moving = zs.windows(2).filter(|p| p[1] < p[0]).count();
    assert!(moving > 60 * 3, "kept rolling");
    assert!(w.player(P0).unwrap().runner.feathers == 2);
}

#[test]
fn generated_courses_with_barrels_are_always_feasible_at_max_plausible_speed() {
    let t = Tuning::default();
    let speed = |z: f64| t.solo_speed(z) + t.plausible_margin();
    let mut total = 0;
    for seed in 1..=24u64 {
        let mut c = Course::new(&t);
        let ctx = GenCtx::fixed(seed, 0.9);
        c.generate_until(2500.0, &ctx, &speed);
        assert!(c.feasible_from(0, -10.0, &speed, &[]).is_some(), "seed {seed} infeasible");
        total += c.rows().filter(|r| r.has_barrel()).count();
        // Multiplayer opening: no barrels in the gentle stretch.
        let mut m = Course::new(&t);
        let mctx = GenCtx { gentle_until: 300.0, ramp_m: 200.0, ..GenCtx::fixed(seed, 0.9) };
        m.generate_until(900.0, &mctx, &|_| 18.0 + Tuning::default().plausible_margin());
        assert!(m.rows().filter(|r| m.row_z(r.row_index) < 300.0).all(|r| !r.has_barrel()), "seed {seed}: barrel in the gentle opening");
        assert!(m.feasible_from(0, -10.0, &|_| 18.0 + Tuning::default().plausible_margin(), &[]).is_some(), "seed {seed} (multi) infeasible");
    }
    assert!(total > 100, "barrels appear in generated courses ({total})");
}

#[test]
fn barrels_are_not_generated_before_mid_difficulty() {
    let t = Tuning::default();
    let speed = |z: f64| t.solo_speed(z) + t.plausible_margin();
    for seed in 1..=10u64 {
        let mut c = Course::new(&t);
        let ctx = GenCtx::fixed(seed, 0.2);
        c.generate_until(1500.0, &ctx, &speed);
        assert!(c.rows().all(|r| !r.has_barrel()), "seed {seed}");
    }
}
