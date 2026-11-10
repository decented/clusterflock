//! SPEC §9 required core tests (core-level ones).

use flock_core::bot::RunnerBot;
use flock_core::course::{Cell, CoinPattern, Row, SolidKind};
use flock_core::*;

const P0: PlayerId = PlayerId(0);
const P1: PlayerId = PlayerId(1);
const P2: PlayerId = PlayerId(2);

fn to_running(w: &mut World) {
    while w.phase() == Phase::Countdown {
        w.step(&[]);
    }
}

fn empty_multi(n: usize, seed: u64) -> World {
    let mut c = MatchConfig::multi(seed, n, None);
    c.tuning.debug_empty_course = true;
    World::new(c)
}

fn solid_row(w: &mut World, idx: u32, cells: [Option<SolidKind>; 3]) {
    let mut r = Row::empty(idx);
    for (c, s) in r.cells.iter_mut().zip(cells) {
        *c = Cell::EMPTY.with_solid(s);
    }
    w.debug_course_mut().insert_row(idx, r);
}

/// Row index a bit ahead of `z`.
fn row_ahead(w: &World, z: f64, metres: f64) -> u32 {
    w.course().row_at(z + metres) + 1
}

fn has<F: Fn(&Event) -> bool>(w: &World, f: F) -> bool {
    w.events().iter().any(f)
}

fn assert_finite(r: &Runner) {
    assert!(r.pos.x.is_finite() && r.pos.y.is_finite() && r.pos.z.is_finite(), "{:?}", r.pos);
    assert!(r.vel.x.is_finite() && r.vel.y.is_finite() && r.vel.z_off.is_finite(), "{:?}", r.vel);
}

#[test]
fn solo_runs_with_no_network() {
    let mut w = World::new(MatchConfig::solo(42));
    assert_eq!(w.phase(), Phase::Countdown);
    to_running(&mut w);
    for _ in 0..60 * 3 {
        w.step(&[]);
    }
    assert_eq!(w.phase(), Phase::Running);
    let r = w.solo_result().unwrap();
    assert!(r.distance_m > 25.0, "{r:?}");
    // Still inside the run-in: nothing hit.
    assert_eq!(w.runner(P0).unwrap().feathers, 3);
}

#[test]
fn final_feather_ends_solo_and_restart_works() {
    let mut c = MatchConfig::solo(7);
    c.tuning.debug_empty_course = true;
    c.tuning.feathers = 1;
    let mut w = World::new(c.clone());
    to_running(&mut w);
    let z = w.runner(P0).unwrap().pos.z;
    let idx = row_ahead(&w, z, 10.0);
    solid_row(&mut w, idx, [None, Some(SolidKind::Block), None]);
    let mut over_seen = false;
    for _ in 0..600 {
        w.step(&[]);
        if has(&w, |e| matches!(e, Event::MatchOver { reason: EndReason::FinalFeather })) {
            over_seen = true;
            assert!(has(&w, |e| matches!(e, Event::Eliminated { player } if *player == P0)));
            break;
        }
    }
    assert!(over_seen);
    assert_eq!(w.phase(), Phase::Over);
    assert_eq!(w.end_reason(), Some(EndReason::FinalFeather));
    let res = w.solo_result().unwrap();
    assert!(res.distance_m > 0.0);
    // Stepping after the end is a no-op.
    let t = w.tick();
    w.step(&[]);
    assert_eq!(w.tick(), t);
    // Immediate restart.
    let mut w2 = World::new(c);
    to_running(&mut w2);
    assert_eq!(w2.phase(), Phase::Running);
}

#[test]
fn two_four_six_player_worlds_share_one_course() {
    let mut first_rows: Option<Vec<Row>> = None;
    for n in [2usize, 4, 6] {
        let mut w = World::new(MatchConfig::multi(99, n, None));
        assert_eq!(w.players().len(), n);
        assert_eq!(w.runners().count(), n);
        // Distinct, non-overlapping start slots.
        let rs: Vec<_> = w.runners().cloned().collect();
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = rs[i].pos.x - rs[j].pos.x;
                let dz = (rs[i].pos.z - rs[j].pos.z) as f32;
                assert!((dx * dx + dz * dz).sqrt() > 2.0 * w.tuning().body_radius);
            }
        }
        // Same seed → same generated course regardless of player count.
        let rows: Vec<Row> = w.course().rows().copied().collect();
        if let Some(f) = &first_rows {
            assert_eq!(&rows, f, "n={n}");
        } else {
            first_rows = Some(rows);
        }
        to_running(&mut w);
        for _ in 0..120 {
            w.step(&[]);
        }
        let (lo, hi) = (w.course().row_z(w.course().first_index().unwrap()), w.course().row_z(w.course().last_index().unwrap()));
        for r in w.runners() {
            assert!(r.pos.z >= lo && r.pos.z <= hi);
            assert_finite(r);
        }
    }
}

#[test]
fn six_in_one_lane_stay_simulated_30s() {
    let mut w = World::debug_six_in_one_lane(5);
    to_running(&mut w);
    let mut prev: Vec<Runner> = w.runners().cloned().collect();
    for _ in 0..60 * 30 {
        w.step(&[]);
        let cur: Vec<Runner> = w.runners().cloned().collect();
        assert_eq!(cur.len(), 6, "nobody removed");
        let v = w.v_course();
        for (a, b) in prev.iter().zip(cur.iter()) {
            assert_eq!(a.id, b.id);
            assert_finite(b);
            let dx = (b.pos.x - a.pos.x).abs();
            let dz = ((b.pos.z - a.pos.z) as f32 - v * DT).abs();
            assert!(dx < 0.5 && dz < 0.5, "teleport {:?}: dx {dx} dz {dz}", b.id);
        }
        prev = cur;
    }
    // Still a queue in one lane-ish band, all bodies separated.
    let rs: Vec<_> = w.runners().cloned().collect();
    let spread = rs.iter().map(|r| r.pos.z).fold(f64::MIN, f64::max) - rs.iter().map(|r| r.pos.z).fold(f64::MAX, f64::min);
    assert!(spread < 20.0, "pack stays bounded, spread {spread}");
    assert_eq!(w.phase(), Phase::Running);
}

#[test]
fn entering_a_crowded_lane_displaces_without_teleport() {
    let mut w = empty_multi(5, 3);
    to_running(&mut w);
    let t = w.tuning().clone();
    let base = w.runner(P0).unwrap().pos.z;
    for k in 0..4u8 {
        let r = w.debug_runner_mut(PlayerId(k)).unwrap();
        *r = Runner::new(PlayerId(k), 1, 0.0, base - k as f64 * 0.95, 3);
    }
    let r = w.debug_runner_mut(PlayerId(4)).unwrap();
    *r = Runner::new(PlayerId(4), 0, t.lane_x(0), base - 1.45, 3);
    let mut cmds = vec![(PlayerId(4), Command::Right)];
    let mut touched = false;
    let mut max_disp_others = 0.0f32;
    let mut prev: Vec<Runner> = w.runners().cloned().collect();
    for _ in 0..90 {
        w.step(&cmds);
        cmds.clear();
        let cur: Vec<Runner> = w.runners().cloned().collect();
        let front = cur.iter().filter(|r| r.id != PlayerId(4)).map(|r| r.pos.z).fold(f64::MIN, f64::max);
        let me = cur.iter().find(|r| r.id == PlayerId(4)).unwrap();
        assert!(me.pos.z < front + 0.5, "lane-changer teleported to the front: {} vs {}", me.pos.z, front);
        touched |= me.contacts_prev != 0;
        for (a, b) in prev.iter().zip(cur.iter()) {
            assert!((b.pos.x - a.pos.x).abs() < 0.5);
            if b.id != PlayerId(4) {
                max_disp_others = max_disp_others.max(b.pos.x.abs());
            }
        }
        prev = cur;
    }
    assert!(touched, "real contact happened");
    assert!(max_disp_others > 0.1, "someone in the queue was shoved ({max_disp_others})");
}

#[test]
fn collapse_in_front_forces_hop_and_blocks_next_jump() {
    let mut w = empty_multi(3, 11);
    to_running(&mut w);
    let t = w.tuning().clone();
    let z = w.runner(P0).unwrap().pos.z;
    *w.debug_runner_mut(P0).unwrap() = Runner::new(P0, 1, 0.0, z + 1.5, 3);
    *w.debug_runner_mut(P1).unwrap() = Runner::new(P1, 1, 0.0, z, 3);
    *w.debug_runner_mut(P2).unwrap() = Runner::new(P2, 2, t.lane_x(2), z, 3);
    let mut cmds = vec![(P0, Command::Down)];
    let mut hopped_at = None;
    for i in 0..90 {
        w.step(&cmds);
        cmds.clear();
        // Bystander in another lane is untouched.
        let c = w.runner(P2).unwrap();
        assert_eq!(c.loco, Loco::Running);
        assert_eq!(c.pos.y, 0.0);
        if has(&w, |e| matches!(e, Event::Hop { player, over } if *player == P1 && *over == P0)) {
            hopped_at = Some(i);
            break;
        }
    }
    assert!(hopped_at.is_some(), "follower hopped over the collapsed body");
    let b = w.runner(P1).unwrap();
    assert_eq!(b.loco, Loco::Airborne { voluntary: false, can_act: false });
    // The follower presses jump mid-hop: no new jump before landing.
    let mut cmds = vec![(P1, Command::Jump)];
    let mut landed = false;
    for _ in 0..60 {
        w.step(&cmds);
        cmds.clear();
        let jumped = has(&w, |e| matches!(e, Event::Jumped { player } if *player == P1));
        if has(&w, |e| matches!(e, Event::Landed { player } if *player == P1)) {
            landed = true;
            break;
        }
        assert!(!jumped, "jumped while still in the involuntary hop");
        let b = w.runner(P1).unwrap();
        assert!(b.vel.y < t.jump_vy - 0.5, "no fresh jump impulse mid-air");
    }
    assert!(landed);
}

#[test]
fn damaging_obstacle_across_many_ticks_costs_one_feather() {
    for kind in [SolidKind::Block, SolidKind::Hay, SolidKind::Beam] {
        let mut c = MatchConfig::solo(3);
        c.tuning.debug_empty_course = true;
        let mut w = World::new(c);
        to_running(&mut w);
        let z = w.runner(P0).unwrap().pos.z;
        let idx = row_ahead(&w, z, 8.0);
        solid_row(&mut w, idx, [None, Some(kind), None]);
        let mut crashes = 0;
        let mut overlap_ticks = 0;
        for _ in 0..240 {
            w.step(&[]);
            crashes += w.events().iter().filter(|e| matches!(e, Event::Crash { .. })).count();
            if !w.runner(P0).unwrap().touched_prev.is_empty() {
                overlap_ticks += 1;
            }
        }
        assert_eq!(crashes, 1, "{kind:?}");
        if kind != SolidKind::Beam {
            // (A crashed body tumbles low and slides under a beam at once.)
            assert!(overlap_ticks > 1, "{kind:?}: body touched the obstacle over several ticks ({overlap_ticks})");
        }
        assert_eq!(w.runner(P0).unwrap().feathers, 2, "{kind:?}");
    }
}

/// Crash P0 into hay in front of it; returns the crash tick.
fn crash_p0(w: &mut World) -> Tick {
    let r = w.runner(P0).unwrap();
    let (z, lane) = (r.pos.z, r.target_lane as usize);
    let idx = row_ahead(w, z, 6.0);
    let mut cells = [None; 3];
    cells[lane] = Some(SolidKind::Hay);
    solid_row(w, idx, cells);
    for _ in 0..200 {
        let now = w.tick();
        w.step(&[]);
        if has(w, |e| matches!(e, Event::Crash { player, .. } if *player == P0)) {
            return now;
        }
    }
    panic!("no crash");
}

/// 0.2 rule change: protection is never extended. SPEC's old
/// "crash then significant impact -> protection until 3 s after the impact"
/// is replaced by: a bump 2 s into an episode changes nothing about it.
#[test]
fn crash_then_significant_impact_does_not_extend_protection() {
    let mut w = empty_multi(2, 21);
    to_running(&mut w);
    let t = w.tuning().clone();
    let pt = t.ticks(t.protect_time);
    let t0 = crash_p0(&mut w);
    assert_eq!(w.runner(P0).unwrap().feathers, 2);
    assert_eq!(w.runner(P0).unwrap().protected_until, Some(t0 + pt));
    // 2 s later: a significant lateral impact from P1.
    while w.tick() < t0 + 120 {
        w.step(&[]);
    }
    assert!(w.runner(P0).unwrap().controllable(), "recovered before the impact");
    let a = w.runner(P0).unwrap().clone();
    {
        let b = w.debug_runner_mut(P1).unwrap();
        *b = Runner::new(P1, 1, a.pos.x + 0.9, a.pos.z, 3);
        b.vel.x = -5.0;
        b.target_lane = 1;
    }
    w.step(&[]);
    assert!(has(&w, |e| matches!(e, Event::Bump { .. })), "{:?}", w.events());
    assert!(!has(&w, |e| matches!(e, Event::ProtectionStart { .. })));
    let r = w.runner(P0).unwrap();
    assert_eq!(r.protected_until, Some(t0 + pt), "a bump never extends protection");
    assert!(r.free_hit, "a bump never uses the free hit");
    // Ends at the original expiry once controllable.
    let mut ended = None;
    for _ in 0..pt + 120 {
        let now = w.tick();
        w.step(&[]);
        if has(&w, |e| matches!(e, Event::ProtectionEnd { player } if *player == P0)) {
            ended = Some(now);
            break;
        }
    }
    let ended = ended.expect("protection ended");
    assert!(ended >= t0 + pt);
    let r = w.runner(P0).unwrap();
    assert!(ended <= t0 + pt + 60 || !r.controllable(), "ended late at {ended} (expiry {})", t0 + pt);
    assert!(r.vulnerable());
    assert!(!r.free_hit, "no free hit while vulnerable");
    assert_eq!(r.feathers, 2);
}

#[test]
fn resting_contact_does_not_refresh_protection() {
    let mut w = empty_multi(2, 4);
    to_running(&mut w);
    let now = w.tick();
    let z = w.runner(P0).unwrap().pos.z;
    *w.debug_runner_mut(P0).unwrap() = Runner::new(P0, 1, 0.0, z, 3);
    *w.debug_runner_mut(P1).unwrap() = Runner::new(P1, 1, 0.0, z - 0.8, 3);
    {
        let r = w.debug_runner_mut(P0).unwrap();
        r.protected_until = Some(now + 120);
        r.protect_since = Some(now);
    }
    let mut contact_ticks = 0;
    let mut ended_at = None;
    for _ in 0..300 {
        let t = w.tick();
        w.step(&[]);
        assert!(!has(&w, |e| matches!(e, Event::ProtectionStart { .. })), "resting contact restarted protection");
        if w.runner(P0).unwrap().contacts_prev != 0 {
            contact_ticks += 1;
        }
        if has(&w, |e| matches!(e, Event::ProtectionEnd { player } if *player == P0)) {
            ended_at = Some(t);
        }
    }
    assert!(contact_ticks > 30, "they stayed in contact ({contact_ticks})");
    assert_eq!(ended_at, Some(now + 120));
}

/// 0.2 rule change: the first obstacle hit of a protection episode
/// is free and passes through (no tumble, no knockback). The protected body
/// is still solid for other runners.
#[test]
fn protected_collision_is_solid_without_feather_loss() {
    let mut w = empty_multi(2, 8);
    to_running(&mut w);
    let now = w.tick();
    {
        let r = w.debug_runner_mut(P0).unwrap();
        r.protected_until = Some(now + 10_000);
        r.protect_since = Some(now);
        r.free_hit = true;
    }
    let lane = w.runner(P0).unwrap().target_lane as usize;
    let z = w.runner(P0).unwrap().pos.z;
    let idx = row_ahead(&w, z, 6.0);
    let mut cells = [None; 3];
    cells[lane] = Some(SolidKind::Hay);
    solid_row(&mut w, idx, cells);
    let mut swag = false;
    for _ in 0..200 {
        w.step(&[]);
        assert!(!has(&w, |e| matches!(e, Event::Crash { player, .. } if *player == P0)), "the free hit is no crash");
        if has(&w, |e| matches!(e, Event::Swagger { player, .. } if *player == P0)) {
            swag = true;
            break;
        }
    }
    assert!(swag, "swagger through the bale");
    let r = w.runner(P0).unwrap().clone();
    assert_eq!(r.feathers, 3);
    assert!(r.knock >= 0.0, "no knockback");
    assert!(matches!(r.loco, Loco::Running), "keeps control: {:?}", r.loco);
    assert!(!r.free_hit, "free hit used");
    assert!(r.protected());
    // Still solid for other runners: an overlapping body gets pushed out.
    {
        let b = w.debug_runner_mut(P1).unwrap();
        *b = Runner::new(P1, 1, r.pos.x + 0.3, r.pos.z, 3);
        b.pos.y = r.pos.y;
    }
    for _ in 0..3 {
        w.step(&[]);
    }
    let (a, b) = (w.runner(P0).unwrap(), w.runner(P1).unwrap());
    let d = ((a.pos.x - b.pos.x).powi(2) + ((a.pos.z - b.pos.z) as f32).powi(2)).sqrt();
    let vertical_overlap = a.pos.y < b.pos.y + b.height(w.tuning()) && b.pos.y < a.pos.y + a.height(w.tuning());
    assert!(!vertical_overlap || d > 2.0 * w.tuning().body_radius - 0.05, "protected body is solid (d={d})");
    assert_eq!(w.runner(P0).unwrap().feathers, 3);
}

#[test]
fn final_feather_in_multi_becomes_ghost_same_tick() {
    let mut w = empty_multi(2, 13);
    to_running(&mut w);
    w.debug_runner_mut(P0).unwrap().feathers = 1;
    crash_p0(&mut w);
    assert!(has(&w, |e| matches!(e, Event::Eliminated { player } if *player == P0)));
    assert!(has(&w, |e| matches!(e, Event::BecameGhost { player } if *player == P0)));
    let p = w.player(P0).unwrap();
    assert!(p.role.is_ghost());
    assert!(w.runner(P0).is_none(), "body removed from the runner set");
    assert!(w.ghost_energy(P0).unwrap() >= w.tuning().ghost_energy_start);
    assert_eq!(w.phase(), Phase::Running, "one runner left keeps the round going");
}

#[test]
fn last_runner_keeps_running_against_ghosts() {
    let mut w = World::debug_one_runner_five_ghosts(17);
    let mut bot = RunnerBot::new(BotSkill::MAX, 17, P0);
    let mut placed = 0;
    let mut events_before_over = Vec::new();
    for _ in 0..60 * 40 {
        let cmd = bot.think(&w, P0);
        let cmds: Vec<_> = cmd.map(|c| (P0, c)).into_iter().collect();
        w.step(&cmds);
        placed += w.events().iter().filter(|e| matches!(e, Event::Placed { .. })).count();
        events_before_over.extend(w.events().iter().cloned());
        if w.phase() == Phase::Over {
            break;
        }
        assert!(w.runner(P0).is_some(), "while the runner lives the round continues");
    }
    assert_eq!(w.ghost_count(), 5);
    assert!(placed >= 5, "ghosts kept playing ({placed})");
    if w.phase() == Phase::Over {
        assert_eq!(w.end_reason(), Some(EndReason::NoRunnersLeft));
    }
}

/// Two ghosts + one runner on an empty course, past the countdown.
fn ghost_world(seed: u64) -> World {
    let mut w = empty_multi(3, seed);
    w.force_eliminate(P1);
    w.force_eliminate(P2);
    to_running(&mut w);
    for _ in 0..60 {
        w.step(&[]);
    }
    w
}

#[test]
fn two_ghosts_one_cell_only_one_accepted_and_charged() {
    let mut w = ghost_world(31);
    let band = w.placement_band();
    assert!(!band.is_empty());
    let row = band.first;
    let (e1, e2) = (w.ghost_energy(P1).unwrap(), w.ghost_energy(P2).unwrap());
    assert!(w.check_placement(P1, Tool::Hay, 1, row).is_ok());
    let a = w.place(PlaceRequest { player: P1, req: 1, tool: Tool::Hay, lane: 1, row });
    let b = w.place(PlaceRequest { player: P2, req: 1, tool: Tool::Hay, lane: 1, row });
    let acc = a.unwrap();
    assert_eq!(b, Err(Reject::Occupied));
    assert!((w.ghost_energy(P1).unwrap() - (e1 - acc.cost)).abs() < 1e-5);
    assert_eq!(w.ghost_energy(P2).unwrap(), e2);
    // Visible at once as a pending reservation, solid after materialising.
    let o = w.course().obj(acc.obj).unwrap();
    assert!(!o.is_live(w.tick()));
    w.step(&[]);
    assert!(has(&w, |e| matches!(e, Event::Placed { by, .. } if *by == P1)));
    let mut mat = false;
    for _ in 0..60 {
        w.step(&[]);
        mat |= has(&w, |e| matches!(e, Event::Materialised { obj } if *obj == acc.obj));
    }
    assert!(mat);
}

#[test]
fn wall_attempt_rejected_without_energy_spent() {
    // Density: a second architect solid in the same row.
    let mut w = ghost_world(41);
    let row = w.placement_band().first;
    let e2 = w.ghost_energy(P2).unwrap();
    w.place(PlaceRequest { player: P1, req: 1, tool: Tool::Hay, lane: 0, row }).unwrap();
    let r = w.place(PlaceRequest { player: P2, req: 1, tool: Tool::Hay, lane: 1, row });
    assert!(matches!(r, Err(Reject::Density) | Err(Reject::Unsafe)), "{r:?}");
    assert_eq!(w.ghost_energy(P2).unwrap(), e2);

    // Unsafe: lanes 0/1 blocked for two rows with hay in lane 2 on the
    // second; a beam in lane 2 on the first would need a duck and a jump 6 m
    // apart.
    let mut w = ghost_world(43);
    let row = w.placement_band().first;
    solid_row(&mut w, row, [Some(SolidKind::Block), Some(SolidKind::Block), None]);
    solid_row(&mut w, row + 1, [Some(SolidKind::Block), Some(SolidKind::Block), Some(SolidKind::Hay)]);
    assert!(w.check_placement(P1, Tool::Coins, 2, row).is_ok(), "base course is fine");
    assert!(w.course().is_feasible(&|z| w.plausible_speed(z)));
    let e1 = w.ghost_energy(P1).unwrap();
    let r = w.place(PlaceRequest { player: P1, req: 9, tool: Tool::Beam, lane: 2, row });
    assert_eq!(r, Err(Reject::Unsafe));
    assert_eq!(w.ghost_energy(P1).unwrap(), e1);
    assert!(w.course().placed().is_empty());
}

#[test]
fn contested_coin_single_award() {
    let mut w = empty_multi(2, 51);
    to_running(&mut w);
    let z = w.runner(P0).unwrap().pos.z;
    let idx = row_ahead(&w, z, 10.0);
    let mut row = Row::empty(idx);
    row.cells[1] = Cell::EMPTY.with_coins(CoinPattern::Line);
    w.debug_course_mut().insert_row(idx, row);
    let first = w.course().row_objects(idx).into_iter().find(|o| o.kind == ObjKind::Coin).unwrap();
    for (id, x) in [(P0, -0.45f32), (P1, 0.45)] {
        let r = w.debug_runner_mut(id).unwrap();
        *r = Runner::new(id, 1, x, first.z - 0.95, 3);
    }
    let mut awards = Vec::new();
    for _ in 0..30 {
        w.step(&[]);
        for e in w.events() {
            if let Event::CoinTaken { obj, by } = e {
                awards.push((*obj, *by));
            }
        }
    }
    let first_awards: Vec<_> = awards.iter().filter(|(o, _)| *o == first.id).collect();
    assert_eq!(first_awards.len(), 1, "{awards:?}");
    assert_eq!(first_awards[0].1, P0, "tie → lower PlayerId");
    let total: u32 = w.runners().map(|r| r.coins).sum();
    assert_eq!(total as usize, awards.len());
    assert!(w.course().obj(first.id).is_none(), "coin removed for everyone");
}

#[test]
fn duplicate_place_request_is_idempotent() {
    let mut w = ghost_world(61);
    let row = w.placement_band().first;
    let e = w.ghost_energy(P1).unwrap();
    let req = PlaceRequest { player: P1, req: 5, tool: Tool::Mud, lane: 2, row };
    let a = w.place(req);
    let b = w.place(req);
    assert!(a.is_ok());
    assert_eq!(a, b);
    assert_eq!(w.course().placed().len(), 1);
    assert!((w.ghost_energy(P1).unwrap() - (e - w.tuning().cost.mud)).abs() < 1e-5);
    w.step(&[]);
    let placed = w.events().iter().filter(|e| matches!(e, Event::Placed { .. })).count();
    assert_eq!(placed, 1);
    // Rejections are idempotent too, and runners cannot place.
    let bad = PlaceRequest { player: P1, req: 6, tool: Tool::Hay, lane: 9, row };
    assert_eq!(w.place(bad), Err(Reject::OutsideBand));
    assert_eq!(w.place(bad), Err(Reject::OutsideBand));
    assert_eq!(w.place(PlaceRequest { player: P0, req: 1, tool: Tool::Hay, lane: 1, row }), Err(Reject::NotGhost));
    assert_eq!(w.place(PlaceRequest { player: PlayerId(200), req: 1, tool: Tool::Hay, lane: 1, row }), Err(Reject::NotGhost));
}

#[test]
fn ghosts_cannot_run_and_bad_input_is_harmless() {
    let mut w = ghost_world(71);
    w.step(&[(P1, Command::Jump), (PlayerId(77), Command::Left), (P0, Command::Left)]);
    assert_eq!(w.player(P1).unwrap().runner.stats.ignored_cmds, 1);
    w.disconnect(PlayerId(99));
    w.force_eliminate(PlayerId(42));
    assert!(w.check_placement(P1, Tool::Hay, 1, u32::MAX).is_err());
    assert!(w.course().obj(ObjId(u32::MAX)).is_none());
}

#[test]
fn disconnect_removes_body_and_ends_when_too_few() {
    let mut w = empty_multi(2, 81);
    to_running(&mut w);
    w.step(&[]);
    w.disconnect(P1);
    assert!(w.runner(P1).is_none());
    w.step(&[]);
    assert!(has(&w, |e| matches!(e, Event::Disconnected { player } if *player == P1)));
    assert_eq!(w.end_reason(), Some(EndReason::NotEnoughPlayers));
    let st = w.standings();
    assert_eq!(st.last().unwrap().player, P1);
    assert!(st.last().unwrap().disconnected);
}

#[test]
fn predict_runner_matches_world_step_bit_for_bit() {
    let mut w = empty_multi(2, 91);
    to_running(&mut w);
    let z = w.runner(P0).unwrap().pos.z;
    *w.debug_runner_mut(P0).unwrap() = Runner::new(P0, 1, 0.0, z, 3);
    w.debug_runner_mut(P0).unwrap().speed_bonus = 1.7; // the coin bonus is part of the shared integration
    // Keep the other runner well out of reach.
    *w.debug_runner_mut(P1).unwrap() = Runner::new(P1, 2, w.tuning().lane_x(2), z - 30.0, 3);
    let mut pred = w.runner(P0).unwrap().clone();
    let script: Vec<Option<Command>> = (0..240)
        .map(|i| match i {
            3 => Some(Command::Right),
            40 => Some(Command::Jump),
            50 => Some(Command::Down), // buffered while airborne
            100 => Some(Command::DiveLeft),
            160 => Some(Command::JumpRight),
            200 => Some(Command::Right), // into the fence
            _ => None,
        })
        .collect();
    for cmd in script {
        let ctx = w.next_ctx();
        predict_runner(&mut pred, w.course(), w.tuning(), ctx, cmd);
        let cmds: Vec<_> = cmd.map(|c| (P0, c)).into_iter().collect();
        w.step(&cmds);
        let real = w.runner(P0).unwrap();
        assert_eq!(pred.pos, real.pos);
        assert_eq!(pred.vel, real.vel);
        assert_eq!(pred.speed_bonus.to_bits(), real.speed_bonus.to_bits());
        assert_eq!(pred.loco, real.loco);
        assert_eq!(pred.target_lane, real.target_lane);
        assert_eq!(pred.buffered, real.buffered);
        assert_eq!((pred.boost.to_bits(), pred.knock.to_bits()), (real.boost.to_bits(), real.knock.to_bits()));
        assert_eq!(real.contacts_prev, 0, "keep the test free of contacts");
    }
}

#[test]
fn world_snapshot_roundtrip_continues_identically() {
    let mut w = World::new(MatchConfig::multi(5, 4, Some(BotSkill::NORMAL)));
    for _ in 0..60 * 20 {
        w.step(&[]);
    }
    let json = serde_json::to_string(&w).unwrap();
    let mut w2: World = serde_json::from_str(&json).unwrap();
    for _ in 0..600 {
        w.step(&[]);
        w2.step(&[]);
        assert_eq!(w.events(), w2.events());
    }
    assert_eq!(serde_json::to_string(&w).unwrap(), serde_json::to_string(&w2).unwrap());
}

#[test]
fn remote_course_follows_host_events() {
    let mut w = World::new(MatchConfig::multi(77, 4, Some(BotSkill::NORMAL)));
    let mut remote = flock_core::Course::new_remote(w.tuning());
    for r in w.course().rows() {
        remote.insert_row(r.row_index, *r);
    }
    let mut released = 0;
    for _ in 0..60 * 150 {
        w.step(&[]);
        for e in w.events() {
            match *e {
                Event::CoinTaken { obj, .. } => {
                    remote.remove_coin(obj);
                }
                Event::Placed { by, obj, tool, lane, row, solid_at } => remote.add_placement(by, obj, tool, lane, row, solid_at),
                Event::BarrelReleased { row, tick } => {
                    released += 1;
                    remote.release_barrel(row, tick);
                }
                _ => {}
            }
        }
        remote.set_clock(w.course().clock());
        for r in w.course().rows() {
            if remote.row(r.row_index).is_none() {
                remote.insert_row(r.row_index, *r);
            }
        }
        if let Some(f) = w.course().first_index() {
            remote.release_before(f);
        }
        if w.phase() == Phase::Over {
            break;
        }
    }
    let a: Vec<Obj> = w.course().objects();
    let b: Vec<Obj> = remote.objects();
    assert_eq!(a, b);
    assert!(released > 0, "the match had rolling barrels to sync");
}

// ---------------------------------------------------------------------------
// Play-test feedback: jump moves you forward through the pack, crouch back,
// coin speed bonus.

/// Two runners in one lane, `gap` metres apart (P1 behind P0), P0 standing still
/// in the pack; returns the world after `P1` presses `cmd`.
fn pair_in_lane(gap: f64, extra_ahead: Option<f64>) -> World {
    let mut w = empty_multi(3, 21);
    to_running(&mut w);
    let t = w.tuning().clone();
    let z = w.runner(P0).unwrap().pos.z;
    *w.debug_runner_mut(P0).unwrap() = Runner::new(P0, 1, 0.0, z + gap, 3);
    *w.debug_runner_mut(P1).unwrap() = Runner::new(P1, 1, 0.0, z, 3);
    // The third bird is either far away or a second bird ahead in the same lane.
    let z2 = extra_ahead.map_or(z + 40.0, |d| z + gap + d);
    *w.debug_runner_mut(P2).unwrap() = Runner::new(P2, if extra_ahead.is_some() { 1 } else { 2 }, if extra_ahead.is_some() { 0.0 } else { t.lane_x(2) }, z2, 3);
    w
}

/// Run `cmd` for P1 and return (front z, jumper z) after landing/standing, asserting
/// continuity (no teleport) and no damage on the way.
fn run_pair(mut w: World, cmd: Command, ticks: u32) -> World {
    let mut cmds = vec![(P1, cmd)];
    let mut prev = w.runner(P1).unwrap().pos.z;
    for _ in 0..ticks {
        w.step(&cmds);
        cmds.clear();
        let z = w.runner(P1).unwrap().pos.z;
        assert!((z - prev).abs() < 1.2, "P1 teleported: {prev} -> {z}");
        prev = z;
        assert!(!has(&w, |e| matches!(e, Event::Crash { .. })));
    }
    w
}

#[test]
fn jump_from_behind_a_standing_bird_ends_ahead_of_it() {
    // Contact distance (2 * body radius) and a little closer/further.
    // A jump from further back (2.5 m) comes down on the bird's back: a plain bump, no
    // damage, no teleport (checked by `run_pair`), but not necessarily ahead.
    run_pair(pair_in_lane(2.5, None), Command::Jump, 120);
    for gap in [0.9, 1.4] {
        let w = run_pair(pair_in_lane(gap, None), Command::Jump, 120);
        let (a, b) = (w.runner(P0).unwrap(), w.runner(P1).unwrap());
        assert!(b.pos.z > a.pos.z, "gap {gap}: jumper {} should be ahead of {}", b.pos.z, a.pos.z);
        assert_eq!(b.feathers, 3);
        assert_eq!(a.feathers, 3);
    }
}

#[test]
fn jump_clears_two_standing_birds_in_a_lane() {
    let w = run_pair(pair_in_lane(1.0, Some(1.0)), Command::Jump, 150);
    let (a, c, b) = (w.runner(P0).unwrap(), w.runner(P2).unwrap(), w.runner(P1).unwrap());
    eprintln!("two birds: jumper {:.2}, first {:.2}, second {:.2}", b.pos.z, a.pos.z, c.pos.z);
    assert!(b.pos.z > a.pos.z.max(c.pos.z), "jumper ends ahead of both");
}

#[test]
fn crouch_lets_the_bird_behind_pass_over_and_ahead() {
    // P0 crouches with P1 right behind it.
    for gap in [0.9, 1.5, 2.5] {
        let mut w = pair_in_lane(gap, None);
        // The crouching bird is P0; the follower P1 just runs.
        let mut cmds = vec![(P0, Command::Down)];
        let mut hopped = false;
        for _ in 0..150 {
            w.step(&cmds);
            cmds.clear();
            hopped |= has(&w, |e| matches!(e, Event::Hop { player, over } if *player == P1 && *over == P0));
            assert!(!has(&w, |e| matches!(e, Event::Crash { .. })));
        }
        let (a, b) = (w.runner(P0).unwrap(), w.runner(P1).unwrap());
        assert!(b.pos.z > a.pos.z, "gap {gap}: follower {} should be ahead of the crouched bird {}", b.pos.z, a.pos.z);
        assert!(hopped || b.pos.z > a.pos.z + 1.0, "gap {gap}: passed over via the ramp hop");
    }
}

#[test]
fn crouch_drops_a_bird_back_about_three_to_four_metres() {
    let mut w = World::new({
        let mut c = MatchConfig::solo(3);
        c.tuning.debug_empty_course = true;
        c
    });
    to_running(&mut w);
    let mut ref_w = w.clone();
    w.step(&[(P0, Command::Down)]);
    ref_w.step(&[]);
    for _ in 0..180 {
        w.step(&[]);
        ref_w.step(&[]);
    }
    let lost = ref_w.runner(P0).unwrap().pos.z - w.runner(P0).unwrap().pos.z;
    assert!((3.0..=4.5).contains(&lost), "crouch lost {lost} m");
    // ... and a jump gains about as much.
    let mut w = ref_w.clone();
    let mut r2 = World::new({
        let mut c = MatchConfig::solo(3);
        c.tuning.debug_empty_course = true;
        c
    });
    to_running(&mut r2);
    r2.step(&[(P0, Command::Jump)]);
    for _ in 0..180 {
        r2.step(&[]);
    }
    // Compare against a runner that never jumped over the same number of ticks.
    let mut base = World::new({
        let mut c = MatchConfig::solo(3);
        c.tuning.debug_empty_course = true;
        c
    });
    to_running(&mut base);
    for _ in 0..181 {
        base.step(&[]);
    }
    let gained = r2.runner(P0).unwrap().pos.z - base.runner(P0).unwrap().pos.z;
    assert!((2.4..=4.0).contains(&gained), "jump gained {gained} m");
    w.step(&[]);
}

fn solo_empty(seed: u64) -> World {
    let mut c = MatchConfig::solo(seed);
    c.tuning.debug_empty_course = true;
    let mut w = World::new(c);
    to_running(&mut w);
    w
}

#[test]
fn coin_bonus_accumulates_caps_and_halves_on_crash() {
    let mut w = solo_empty(5);
    let t = w.tuning().clone();
    let z = w.runner(P0).unwrap().pos.z;
    // A long line of coins in lane 1 (rows with a coin line each).
    let first = row_ahead(&w, z, 12.0);
    for i in 0..80u32 {
        let mut r = Row::empty(first + i);
        r.cells[1] = Cell::EMPTY.with_coins(CoinPattern::Line);
        w.debug_course_mut().insert_row(first + i, r);
    }
    let mut last = 0.0f32;
    let mut capped_at = None;
    for tick in 0..60 * 40 {
        w.step(&[]);
        let r = w.runner(P0).unwrap();
        assert!(r.speed_bonus >= last - 1e-6, "bonus only grows while collecting");
        assert!(r.speed_bonus <= t.coin_speed_cap + 1e-6, "capped: {}", r.speed_bonus);
        if r.speed_bonus >= t.coin_speed_cap - 1e-6 && capped_at.is_none() {
            capped_at = Some(tick);
        }
        last = r.speed_bonus;
        // It really is part of the forward speed.
        if tick == 120 && r.coins > 0 {
            assert!(r.forward_speed(w.v_course()) > w.v_course() + r.speed_bonus * 0.9);
        }
        if r.coins >= 80 * 4 - 8 {
            break;
        }
    }
    let r = w.runner(P0).unwrap();
    assert!(capped_at.is_some(), "reached the cap ({} coins, bonus {})", r.coins, r.speed_bonus);
    assert!((r.speed_bonus - t.coin_speed_cap).abs() < 1e-5);
    // One coin adds exactly coin_speed (from a fresh runner).
    let mut w2 = solo_empty(6);
    let z = w2.runner(P0).unwrap().pos.z;
    let idx = row_ahead(&w2, z, 12.0);
    let mut r0 = Row::empty(idx);
    r0.cells[1] = Cell::EMPTY.with_coins(CoinPattern::Line);
    w2.debug_course_mut().insert_row(idx, r0);
    for _ in 0..120 {
        w2.step(&[]);
    }
    let r2 = w2.runner(P0).unwrap();
    assert_eq!(r2.coins, 4);
    assert!((r2.speed_bonus - 4.0 * t.coin_speed).abs() < 1e-5, "{}", r2.speed_bonus);
    // A damaging crash halves the bonus.
    let before = w.runner(P0).unwrap().speed_bonus;
    let z = w.runner(P0).unwrap().pos.z;
    let hidx = row_ahead(&w, z, 14.0);
    let mut hr = Row::empty(hidx);
    hr.cells[1] = Cell::EMPTY.with_solid(Some(SolidKind::Hay));
    w.debug_course_mut().insert_row(hidx, hr);
    let mut crashed = false;
    for _ in 0..120 {
        w.step(&[]);
        crashed |= has(&w, |e| matches!(e, Event::Crash { feather_lost: true, .. }));
        if crashed {
            break;
        }
    }
    assert!(crashed);
    let after = w.runner(P0).unwrap().speed_bonus;
    assert!((after - before * 0.5).abs() < 0.2, "halved: {before} -> {after}");
}

