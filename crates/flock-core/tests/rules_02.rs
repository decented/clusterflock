//! 0.2 rules: protection free hit, shield and power-ups, mud, generator purity.

use std::collections::BTreeMap;

use flock_core::bot::RunnerBot;
use flock_core::course::{Cell, CoinPattern, PowerKind, Row, SolidKind};
use flock_core::*;

const P0: PlayerId = PlayerId(0);
const P1: PlayerId = PlayerId(1);
const P2: PlayerId = PlayerId(2);

fn to_running(w: &mut World) {
    while w.phase() == Phase::Countdown {
        w.step(&[]);
    }
}

fn solo_empty(seed: u64) -> World {
    let mut c = MatchConfig::solo(seed);
    c.tuning.debug_empty_course = true;
    let mut w = World::new(c);
    to_running(&mut w);
    w
}

fn has<F: Fn(&Event) -> bool>(w: &World, f: F) -> bool {
    w.events().iter().any(f)
}

/// Insert a row `metres` ahead of P0 with `f` applied to the cell of P0's lane.
fn ahead(w: &mut World, metres: f64, f: impl Fn(Cell) -> Cell) -> u32 {
    let r = w.runner(P0).unwrap();
    let (z, lane) = (r.pos.z, r.target_lane as usize);
    let idx = w.course().row_at(z + metres) + 1;
    let mut row = w.course().row(idx).copied().unwrap_or(Row::empty(idx));
    row.cells[lane] = f(row.cells[lane]);
    w.debug_course_mut().insert_row(idx, row);
    idx
}

fn hay(c: Cell) -> Cell {
    c.with_solid(Some(SolidKind::Hay))
}

/// Step until `f` holds for this tick's events (at most `max` ticks).
fn step_until<F: Fn(&Event) -> bool>(w: &mut World, max: u32, f: F) -> bool {
    for _ in 0..max {
        w.step(&[]);
        if w.events().iter().any(&f) {
            return true;
        }
    }
    false
}

fn wait_controllable(w: &mut World) {
    for _ in 0..300 {
        if w.runner(P0).unwrap().controllable() {
            return;
        }
        w.step(&[]);
    }
    panic!("never recovered");
}

// ---------------------------------------------------------------- protection

#[test]
fn second_obstacle_hit_in_an_episode_counts() {
    let mut w = solo_empty(3);
    ahead(&mut w, 8.0, hay);
    assert!(step_until(&mut w, 200, |e| matches!(e, Event::Crash { feather_lost: true, .. })));
    assert!(has(&w, |e| matches!(e, Event::ProtectionStart { .. })));
    let r = w.runner(P0).unwrap();
    assert_eq!(r.feathers, 2);
    assert!(r.protected() && r.free_hit, "new episode with its free hit");
    let first_until = r.protected_until.unwrap();
    wait_controllable(&mut w);

    // Free hit: carry on through, no tumble, no knockback, bonus kept.
    w.debug_runner_mut(P0).unwrap().speed_bonus = 2.0;
    ahead(&mut w, 8.0, hay);
    assert!(step_until(&mut w, 200, |e| matches!(e, Event::Swagger { .. })));
    assert!(!has(&w, |e| matches!(e, Event::Crash { .. } | Event::Tumble { .. })));
    for _ in 0..20 {
        w.step(&[]);
        assert!(!has(&w, |e| matches!(e, Event::Crash { .. } | Event::Tumble { .. })), "one free hit, contact deduped");
    }
    let r = w.runner(P0).unwrap();
    assert_eq!(r.feathers, 2);
    assert!(matches!(r.loco, Loco::Running | Loco::WindUp { .. } | Loco::Airborne { voluntary: true, .. }), "{:?}", r.loco);
    assert!(r.knock > -0.5, "no new knockback: {}", r.knock);
    assert_eq!(r.speed_bonus, 2.0, "the free hit does not halve the coin bonus");
    assert!(r.protected() && !r.free_hit);
    assert_eq!(r.protected_until, Some(first_until), "not extended");
    assert_eq!(r.stats.swaggers, 1);

    // The next hit while still protected counts: feather, tumble, new episode.
    ahead(&mut w, 8.0, hay);
    let now_before = w.tick();
    assert!(step_until(&mut w, 200, |e| matches!(e, Event::Crash { .. })));
    assert!(w.tick() - 1 < first_until, "still inside the first episode ({} vs {first_until})", w.tick());
    assert!(has(&w, |e| matches!(e, Event::Crash { feather_lost: true, .. })));
    assert!(has(&w, |e| matches!(e, Event::Tumble { .. })));
    let start = w.events().iter().find_map(|e| if let Event::ProtectionStart { until, .. } = e { Some(*until) } else { None });
    let r = w.runner(P0).unwrap();
    assert_eq!(r.feathers, 1);
    assert!(start.is_some_and(|u| u > first_until && u > now_before), "a new episode: {start:?}");
    assert!(r.free_hit, "the new episode has its own free hit");
}

#[test]
fn second_hit_on_the_last_feather_eliminates() {
    let mut w = solo_empty(4);
    let now = w.tick();
    {
        let r = w.debug_runner_mut(P0).unwrap();
        r.feathers = 1;
        r.protected_until = Some(now + 10_000);
        r.protect_since = Some(now);
        r.free_hit = false;
    }
    ahead(&mut w, 8.0, hay);
    assert!(step_until(&mut w, 200, |e| matches!(e, Event::Eliminated { .. })));
    assert_eq!(w.phase(), Phase::Over);
}

#[test]
fn free_hit_passes_through_a_block() {
    let mut w = solo_empty(5);
    let now = w.tick();
    {
        let r = w.debug_runner_mut(P0).unwrap();
        r.protected_until = Some(now + 10_000);
        r.protect_since = Some(now);
        r.free_hit = true;
    }
    let idx = ahead(&mut w, 10.0, |c| c.with_solid(Some(SolidKind::Block)));
    let block = w.course().row_objects(idx).into_iter().find(|o| o.kind == ObjKind::Block).unwrap();
    let lane = w.runner(P0).unwrap().target_lane;
    for _ in 0..150 {
        w.step(&[]);
        assert!(!has(&w, |e| matches!(e, Event::Crash { .. })));
    }
    let r = w.runner(P0).unwrap();
    assert!(r.pos.z > block.z1() + 1.0, "through the wall: z {} vs {}", r.pos.z, block.z1());
    assert_eq!(r.target_lane, lane, "not steered aside");
    assert_eq!(r.feathers, 3);
    assert_eq!(r.swagger_obj, None, "cleared once past it");
}

// ---------------------------------------------------------------- shield and power-ups

#[test]
fn shield_takes_a_vulnerable_crash() {
    let mut w = solo_empty(6);
    let now = w.tick();
    w.debug_runner_mut(P0).unwrap().shield_until = now + 600;
    ahead(&mut w, 8.0, hay);
    assert!(step_until(&mut w, 200, |e| matches!(e, Event::Crash { .. })));
    assert!(has(&w, |e| matches!(e, Event::ShieldBroken { player } if *player == P0)));
    assert!(has(&w, |e| matches!(e, Event::Crash { feather_lost: false, .. })));
    assert!(has(&w, |e| matches!(e, Event::ProtectionStart { .. })));
    assert!(has(&w, |e| matches!(e, Event::Tumble { .. })), "still tumbles");
    let r = w.runner(P0).unwrap();
    assert_eq!(r.feathers, 3);
    assert!(!r.shield(w.tick()));
    assert!(r.protected() && r.free_hit, "a normal episode");
}

#[test]
fn free_hit_comes_before_a_held_shield() {
    let mut w = solo_empty(7);
    let now = w.tick();
    {
        let r = w.debug_runner_mut(P0).unwrap();
        r.shield_until = now + 2000;
        r.protected_until = Some(now + 600);
        r.protect_since = Some(now);
        r.free_hit = true;
    }
    ahead(&mut w, 8.0, hay);
    assert!(step_until(&mut w, 200, |e| matches!(e, Event::Swagger { .. })));
    assert!(w.runner(P0).unwrap().shield(w.tick()), "shield kept");
    // The next hit is an ordinary vulnerable crash: the shield takes it.
    ahead(&mut w, 8.0, hay);
    assert!(step_until(&mut w, 200, |e| matches!(e, Event::Crash { .. })));
    assert!(has(&w, |e| matches!(e, Event::ShieldBroken { .. })));
    assert_eq!(w.runner(P0).unwrap().feathers, 3);
}

#[test]
fn power_ups_magnet_and_doubler() {
    let mut w = solo_empty(8);
    let idx = ahead(&mut w, 10.0, |c| c.with_power(Some(PowerKind::Magnet)));
    let pid = w.course().row_objects(idx).into_iter().find(|o| matches!(o.kind, ObjKind::Power { .. })).unwrap().id;
    assert!(step_until(&mut w, 120, |e| matches!(e, Event::PowerTaken { kind: PowerKind::Magnet, by, .. } if *by == P0)));
    assert!(w.course().obj(pid).is_none(), "gone");
    assert!(w.debug_course_mut().take_power(pid).is_none(), "idempotent");
    let r = w.runner(P0).unwrap();
    assert!(r.magnet(w.tick()));
    assert_eq!(r.magnet_until, w.tick() - 1 + w.tuning().ticks(w.tuning().magnet_s));
    // Coins in the next lane are pulled in.
    let lane = r.target_lane as usize;
    let side = if lane == 0 { 1 } else { lane - 1 };
    let z = r.pos.z;
    let ci = w.course().row_at(z + 12.0) + 1;
    let mut row = Row::empty(ci);
    row.cells[side] = Cell::EMPTY.with_coins(CoinPattern::Line);
    w.debug_course_mut().insert_row(ci, row);
    for _ in 0..90 {
        w.step(&[]);
    }
    let r = w.runner(P0).unwrap();
    assert_eq!(r.target_lane as usize, lane);
    assert_eq!(r.coins, 4, "magnet collected the next lane's coins");

    // Doubler: each coin counts 2.
    ahead(&mut w, 10.0, |c| c.with_power(Some(PowerKind::Doubler)));
    assert!(step_until(&mut w, 120, |e| matches!(e, Event::PowerTaken { kind: PowerKind::Doubler, .. })));
    let before = w.runner(P0).unwrap().coins;
    ahead(&mut w, 12.0, |c| c.with_coins(CoinPattern::Line));
    for _ in 0..90 {
        w.step(&[]);
    }
    assert_eq!(w.runner(P0).unwrap().coins, before + 8);
}

#[test]
fn power_up_goes_to_the_first_runner_only() {
    let mut c = MatchConfig::multi(9, 2, None);
    c.tuning.debug_empty_course = true;
    let mut w = World::new(c);
    to_running(&mut w);
    let z = w.runner(P0).unwrap().pos.z.max(w.runner(P1).unwrap().pos.z);
    *w.debug_runner_mut(P0).unwrap() = Runner::new(P0, 1, 0.0, z, 3);
    *w.debug_runner_mut(P1).unwrap() = Runner::new(P1, 1, 0.0, z - 3.0, 3);
    let idx = w.course().row_at(z + 10.0) + 1;
    let mut row = Row::empty(idx);
    row.cells[1] = Cell::EMPTY.with_power(Some(PowerKind::Shield));
    w.debug_course_mut().insert_row(idx, row);
    let mut taken = Vec::new();
    for _ in 0..180 {
        w.step(&[]);
        for e in w.events() {
            if let Event::PowerTaken { by, kind, .. } = e {
                taken.push((*by, *kind));
            }
        }
    }
    assert_eq!(taken, vec![(P0, PowerKind::Shield)]);
    assert!(w.runner(P0).unwrap().shield(w.tick()));
    assert!(!w.runner(P1).unwrap().shield(w.tick()));
}

#[test]
fn power_ups_spawn_by_the_rules() {
    let t = Tuning::default();
    let mut kinds = [0u32; 3];
    for seed in 1..=6u64 {
        let rows = course_rows(&MatchConfig::solo(seed), 600);
        let mut last: Option<u32> = None;
        let mut n = 0;
        for r in &rows {
            let ps: Vec<_> = (0..3).filter_map(|l| r.cells[l].power().map(|k| (l, k))).collect();
            assert!(ps.len() <= 1, "one per row");
            let Some(&(l, k)) = ps.first() else { continue };
            n += 1;
            kinds[k as usize] += 1;
            assert!(r.cells[l].solid().is_none() && !r.cells[l].mud(), "seed {seed} row {}: power in a solid/mud cell", r.row_index);
            let z = r.row_index as f32 * t.row_spacing;
            assert!(z >= t.run_in, "not in the opening clear run");
            if let Some(p) = last {
                let gap = (r.row_index - p) as f32 * t.row_spacing;
                assert!(gap >= t.powerup_gap_m * (1.0 - t.powerup_jitter) - t.row_spacing, "seed {seed}: gap {gap}");
                assert!(gap <= t.powerup_gap_m * (1.0 + t.powerup_jitter) + 4.0 * t.row_spacing, "seed {seed}: gap {gap}");
            }
            last = Some(r.row_index);
        }
        assert!((9..=20).contains(&n), "seed {seed}: {n} power-ups in 3.6 km");
    }
    assert!(kinds.iter().all(|&k| k > 3), "every kind appears: {kinds:?}");
    // Disabled at 0.
    let mut c = MatchConfig::solo(1);
    c.tuning.powerup_gap_m = 0.0;
    assert!(course_rows(&c, 600).iter().all(|r| r.power().is_none()));
    // Multiplayer uses its own (shorter) gap.
    let mut w = World::new(MatchConfig::multi(3, 6, Some(BotSkill::NORMAL)));
    for _ in 0..60 * 20 {
        w.step(&[]);
    }
    assert!(w.course().rows().any(|r| r.power().is_some()));
}

// ---------------------------------------------------------------- mud

#[test]
fn no_jump_from_mud_but_lane_changes_and_ducks_work() {
    let mut w = solo_empty(10);
    ahead(&mut w, 4.0, |c| c.with_mud(true));
    for _ in 0..120 {
        w.step(&[]);
        if w.runner(P0).unwrap().in_mud {
            break;
        }
    }
    assert!(w.runner(P0).unwrap().in_mud);
    w.step(&[(P0, Command::Jump)]);
    assert!(has(&w, |e| matches!(e, Event::MudStuck { player } if *player == P0)));
    assert!(!has(&w, |e| matches!(e, Event::WindUp { .. })));
    assert_eq!(w.runner(P0).unwrap().loco, Loco::Running);
    assert!(w.runner(P0).unwrap().buffered.is_none(), "refused, not buffered");
    for _ in 0..12 {
        w.step(&[]);
        assert!(!has(&w, |e| matches!(e, Event::WindUp { .. } | Event::Jumped { .. })), "no delayed jump");
    }
    // Diagonal: the lane change still happens, the jump does not.
    let lane = w.runner(P0).unwrap().target_lane;
    w.step(&[(P0, Command::JumpLeft)]);
    assert!(has(&w, |e| matches!(e, Event::MudStuck { .. })));
    assert!(has(&w, |e| matches!(e, Event::LaneChange { .. })));
    assert_eq!(w.runner(P0).unwrap().target_lane, lane - 1);
    assert_eq!(w.runner(P0).unwrap().loco, Loco::Running);
    // Back into the mud lane and duck: allowed.
    w.step(&[(P0, Command::DiveRight)]);
    assert!(has(&w, |e| matches!(e, Event::WindUp { cmd: Command::DiveRight, .. })));
    assert_eq!(w.runner(P0).unwrap().stats.mud_stuck, 2);
}

fn empty_multi(n: usize, seed: u64) -> World {
    let mut c = MatchConfig::multi(seed, n, None);
    c.tuning.debug_empty_course = true;
    World::new(c)
}

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

fn solid_row(w: &mut World, idx: u32, cells: [Option<SolidKind>; 3]) {
    let mut r = Row::empty(idx);
    for (c, s) in r.cells.iter_mut().zip(cells) {
        *c = Cell::EMPTY.with_solid(s);
    }
    w.debug_course_mut().insert_row(idx, r);
}

#[test]
fn ghost_mud_in_front_of_a_jump_only_obstacle_is_unsafe() {
    let mut w = ghost_world(43);
    let row = w.placement_band().first;
    // Lanes 0/1 walled at row+1; lane 2 has hay: the only route jumps it.
    solid_row(&mut w, row + 1, [Some(SolidKind::Block), Some(SolidKind::Block), Some(SolidKind::Hay)]);
    assert!(w.course().is_feasible(&|z| w.plausible_speed(z)));
    let e1 = w.ghost_energy(P1).unwrap();
    assert_eq!(w.check_placement(P1, Tool::Mud, 2, row), Err(Reject::Unsafe), "mud right before the only jump");
    let r = w.place(PlaceRequest { player: P1, req: 1, tool: Tool::Mud, lane: 2, row });
    assert_eq!(r, Err(Reject::Unsafe));
    assert_eq!(w.ghost_energy(P1).unwrap(), e1, "nothing charged");
    // Mud in a lane that does not lead to the jump is fine.
    assert!(w.check_placement(P1, Tool::Mud, 0, row).is_ok());
}

#[test]
fn ghost_hay_behind_mud_as_the_only_route_is_unsafe() {
    let mut w = ghost_world(44);
    let row = w.placement_band().first;
    solid_row(&mut w, row + 1, [Some(SolidKind::Block), Some(SolidKind::Block), None]);
    // Mud in lane 2 at `row`: fine while lane 2 is free at row+1.
    w.place(PlaceRequest { player: P1, req: 1, tool: Tool::Mud, lane: 2, row }).unwrap();
    // Hay in lane 2 at row+1 would have to be jumped from the mud.
    let r = w.place(PlaceRequest { player: P2, req: 1, tool: Tool::Hay, lane: 2, row: row + 1 });
    assert_eq!(r, Err(Reject::Unsafe));
}

// ---------------------------------------------------------------- purity

const DAILY: u64 = 0x0123_4567_89ab_cdef;

/// Generated cells by row, recorded when each row first appears.
fn rows_seen_while_playing(bot: bool, n: u32) -> (BTreeMap<u32, [Cell; 3]>, RunnerStats) {
    let mut w = World::new(MatchConfig::daily(DAILY));
    let mut b = RunnerBot::new(BotSkill::MAX, 1, P0);
    let mut seen = BTreeMap::new();
    while (seen.len() as u32) < n {
        assert!(w.tick() < 60 * 60 * 20, "too slow");
        w.debug_runner_mut(P0).unwrap().feathers = 3; // never eliminated
        let cmd = if bot { b.think(&w, P0) } else { None };
        let cmds: Vec<_> = cmd.map(|c| (P0, c)).into_iter().collect();
        w.step(&cmds);
        for r in w.course().rows() {
            seen.entry(r.row_index).or_insert(r.cells);
        }
    }
    (seen, w.runner(P0).unwrap().stats)
}

#[test]
fn course_does_not_depend_on_play() {
    let n = 600u32;
    let reference: Vec<[Cell; 3]> = course_rows(&MatchConfig::daily(DAILY), n).iter().map(|r| r.cells).collect();
    assert_eq!(reference.len(), n as usize);
    let (idle, idle_stats) = rows_seen_while_playing(false, n);
    let (played, bot_stats) = rows_seen_while_playing(true, n);
    // The two runs really played differently.
    assert_eq!(idle_stats.jumps, 0);
    assert!(bot_stats.jumps > 20 && idle_stats.crashes > bot_stats.crashes, "{idle_stats:?} vs {bot_stats:?}");
    for (name, got) in [("idle", &idle), ("bot", &played)] {
        let got: Vec<[Cell; 3]> = got.values().take(n as usize).copied().collect();
        assert_eq!(got, reference, "{name}: rows differ from the pure course");
    }
    // Generating all at once or row by row gives the same rows.
    let short = course_rows(&MatchConfig::daily(DAILY), 100);
    assert_eq!(short.iter().map(|r| r.cells).collect::<Vec<_>>(), reference[..100].to_vec());
}

/// The daily course fingerprint is pinned: any change to the generator, the
/// RNG or a default tuning number that alters rows fails here. Then bump
/// `GENERATOR_VERSION` and update the hash.
#[test]
fn daily_course_golden_fingerprint() {
    let fp = det::hex(&course_fingerprint(&MatchConfig::daily(DAILY), 600));
    eprintln!("daily {DAILY:#x} v{GENERATOR_VERSION}: {fp}");
    assert_eq!(GENERATOR_VERSION, 3);
    assert_eq!(fp, "b3da39c5509b370a87836a55ba3f3e48f68bd9bed173322bdd50e69a0554f240");
}
