//! 30 minutes of headless solo play by a max-skill bot: rows stay bounded,
//! the course stays feasible, positions stay finite.

use flock_core::feas;
use flock_core::*;

#[test]
fn long_solo_30_minutes_bounded_feasible_finite() {
    let mut c = MatchConfig::solo(2024);
    c.players[0].bot = Some(BotSkill::MAX);
    c.tuning.feathers = 255; // keep running the full half hour
    let t = c.tuning.clone();
    let mut w = World::new(c);
    let bound = ((t.gen_ahead + t.keep_behind) / t.row_spacing) as usize + 12;
    let minutes = 30;
    let mut max_rows = 0;
    let mut last_gen = w.course().last_index().unwrap();
    for _ in 0..(60 * 60 * minutes + 200) {
        w.step(&[]);
        assert_eq!(w.phase(), if w.tick() <= 180 { w.phase() } else { Phase::Running }, "run ended early");
        let r = w.player(PlayerId(0)).unwrap().runner.clone();
        assert!(r.pos.x.is_finite() && r.pos.y.is_finite() && r.pos.z.is_finite());
        assert!(r.vel.x.is_finite() && r.vel.y.is_finite() && r.vel.z_off.is_finite());
        assert!(r.pos.x.abs() <= t.fence_x);
        let n = w.course().len();
        max_rows = max_rows.max(n);
        assert!(n <= bound, "rows unbounded: {n} > {bound}");
        // Every newly generated row keeps the generator frontier alive.
        let lg = w.course().last_index().unwrap();
        if lg != last_gen {
            assert!(feas::alive(&w.course().frontier()), "generator emitted an infeasible row {lg}");
            last_gen = lg;
        }
        // Full-window DP from the runner every second.
        if w.tick().is_multiple_of(60) {
            let m = w.clone();
            assert!(w.course().is_feasible(&|z| m.plausible_speed(z)), "course infeasible near z={}", r.pos.z);
        }
    }
    let d = w.solo_result().unwrap().distance_m;
    assert!(d > 25_000.0, "distance {d}");
    assert!((d as f64) > 1.0e4 && w.course().row_z(w.course().first_index().unwrap()) > 20_000.0, "old rows released");
    eprintln!("long solo: {d:.0} m, max rows held {max_rows}, feathers left {}", w.player(PlayerId(0)).unwrap().runner.feathers);
}
