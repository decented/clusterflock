//! ASCII top-down view of a bot match, for feel checks.
//!
//! cargo run --example ascii -- solo [skill 0..1] [seed]
//! cargo run --example ascii -- multi [n 2..6] [seed]
//! cargo run --example ascii -- sweep solo [skill] [first_seed] [count]
//! cargo run --example ascii -- sweep multi [n] [first_seed] [count]
//! Add `quiet` to print only the event log and summary.
//! `FLOCK_TUNING='{"jump_vy":9.5}'` overrides tuning fields (partial JSON).

use flock_core::{BotSkill, Event, MatchConfig, Mode, ObjKind, Phase, PlayerId, Role, World, TICK_HZ};

fn tuning_from_env() -> flock_core::Tuning {
    match std::env::var("FLOCK_TUNING") {
        Ok(j) => flock_core::Tuning::from_json(&j).expect("FLOCK_TUNING json"),
        Err(_) => flock_core::Tuning::default(),
    }
}

/// Batch runs: one line per seed (duration, end reason, bump/hop/tumble counts), then the median.
fn sweep(args: &[String]) {
    let solo = args.get(1).map(String::as_str) == Some("solo");
    let num = |i: usize, d: f64| args.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(d);
    let (first, count) = (num(3, 1.0) as u64, num(4, 12.0) as u64);
    let mut durs = Vec::new();
    let mut tot = [0u32; 6];
    for seed in first..first + count {
        let mut cfg = if solo {
            let mut c = MatchConfig::solo(seed);
            c.players[0].bot = Some(BotSkill(num(2, 1.0) as f32));
            c
        } else {
            MatchConfig::multi(seed, num(2, 6.0) as usize, Some(BotSkill::NORMAL))
        };
        cfg.tuning = tuning_from_env();
        let mut w = World::new(cfg);
        let (mut bump, mut hop, mut tumble, mut placed, mut crash, mut slam) = (0, 0, 0, 0, 0, 0);
        while w.phase() != Phase::Over && w.tick() < 60 * 60 * 40 {
            w.step(&[]);
            let evs = w.events();
            for e in evs {
                match e {
                    Event::Bump { .. } => bump += 1,
                    Event::Hop { .. } => hop += 1,
                    Event::Crash { .. } => crash += 1,
                    Event::Tumble { player } => {
                        tumble += 1;
                        if !evs.iter().any(|x| matches!(x, Event::Crash { player: p, .. } if p == player)) {
                            slam += 1;
                        }
                    }
                    Event::Placed { .. } => placed += 1,
                    _ => {}
                }
            }
        }
        let d = w.running_secs();
        durs.push(d);
        for (t, v) in tot.iter_mut().zip([bump, hop, crash, tumble, slam, placed]) {
            *t += v;
        }
        println!("seed {seed:3}: {d:7.1}s {:?} bump={bump} hop={hop} crash={crash} tumble={tumble} (slam={slam}) placed={placed} dist={:?}", w.end_reason(), w.solo_result().map(|r| r.distance_m as i32));
    }
    durs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = durs.len();
    let med = if n % 2 == 1 { durs[n / 2] } else { 0.5 * (durs[n / 2 - 1] + durs[n / 2]) };
    println!("totals: bump={} hop={} crash={} tumble={} slam={} placed={}", tot[0], tot[1], tot[2], tot[3], tot[4], tot[5]);
    println!("median {med:.1}s  min {:.1}  max {:.1}  mean {:.1}", durs[0], durs[n - 1], durs.iter().sum::<f32>() / n as f32);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("sweep") {
        sweep(&args);
        return;
    }
    let mode = args.first().map(String::as_str).unwrap_or("multi");
    let quiet = args.iter().any(|a| a == "quiet");
    let num = |i: usize, d: f64| args.get(i).and_then(|s| s.parse::<f64>().ok()).unwrap_or(d);
    let cfg = if mode == "solo" {
        let mut c = MatchConfig::solo(num(2, 1.0) as u64);
        c.players[0].bot = Some(BotSkill(num(1, 1.0) as f32));
        c
    } else {
        MatchConfig::multi(num(2, 1.0) as u64, num(1, 6.0) as usize, Some(BotSkill::NORMAL))
    };
    let mut cfg = cfg;
    cfg.tuning = tuning_from_env();
    let mut w = World::new(cfg);
    let max_ticks = 60 * 60 * 30;
    let mut counts = std::collections::BTreeMap::<&'static str, u32>::new();
    while w.phase() != Phase::Over && w.tick() < max_ticks {
        w.step(&[]);
        for e in w.events() {
            let name = match e {
                Event::Bump { .. } => "bump",
                Event::Hop { .. } => "hop",
                Event::Tumble { .. } => "tumble",
                Event::Crash { .. } => "crash",
                Event::CoinTaken { .. } => "coin",
                Event::Placed { .. } => "placed",
                Event::Jumped { .. } => "jump",
                Event::Collapsed { .. } => "collapse",
                Event::BecameGhost { .. } => "ghost",
                Event::LongProtection { .. } => "long-protection",
                _ => continue,
            };
            *counts.entry(name).or_default() += 1;
            let interesting = !matches!(e, Event::Jumped { .. } | Event::Collapsed { .. } | Event::CoinTaken { .. });
            if interesting && (!quiet || !matches!(e, Event::Bump { .. })) {
                println!("  t={:6.2}s {:?}", w.tick() as f32 / TICK_HZ as f32, e);
            }
        }
        if !quiet && w.tick().is_multiple_of(30) && w.phase() == Phase::Running {
            draw(&w);
        }
    }
    println!("\n=== over at {:.1}s ({:?}), running {:.1}s", w.tick() as f32 / 60.0, w.end_reason(), w.running_secs());
    if w.mode() == Mode::Solo {
        println!("solo result: {:?}", w.solo_result());
    }
    for s in w.standings() {
        let p = w.player(s.player).unwrap();
        println!(
            "  #{} P{} alive={} survived={:.1}s feathers={} coins={} placements={} stats={:?}",
            s.place,
            s.player.0 + 1,
            s.alive,
            s.survived_ticks as f32 / 60.0,
            s.feathers,
            s.coins,
            s.placements,
            p.runner.stats
        );
    }
    println!("event counts: {:?}", counts);
    println!("rows held: {}", w.course().len());
}

fn draw(w: &World) {
    let a = w.anchor_z();
    let lead = w.leader_z().unwrap_or(a);
    let t = w.tuning();
    println!(
        "--- t={:.1}s v={:.1} anchor={:.0} living={} ghosts={} band={:?}",
        w.running_secs(),
        w.v_course(),
        a,
        w.living_count(),
        w.players().iter().filter(|p| matches!(p.role, Role::Ghost { .. })).map(|p| format!("P{}:{:.1}", p.id().0 + 1, p.ghost_energy().unwrap_or(0.0))).collect::<Vec<_>>().join(","),
        w.placement_band()
    );
    // 2 m per line, from lead+40 down to trail-8; 7 chars per lane (x from -3.7..3.7, 21 cols).
    let cols = 21usize;
    let col = |x: f32| (((x + t.fence_x) / (2.0 * t.fence_x)) * (cols as f32 - 1.0)).round().clamp(0.0, cols as f32 - 1.0) as usize;
    let objs = w.course().objects_in(lead - 20.0, lead + 50.0);
    let mut z = lead + 40.0;
    let trail = w.trailing_z().unwrap_or(a) - 6.0;
    while z > trail {
        let mut line: Vec<char> = vec![' '; cols];
        for o in &objs {
            if o.z0() <= z + 1.0 && o.z1() >= z - 1.0 {
                let ch = match o.kind {
                    ObjKind::Hay => 'H',
                    ObjKind::Barrel => 'B',
                    ObjKind::Beam { .. } => '=',
                    ObjKind::Block => '#',
                    ObjKind::Mud => '~',
                    ObjKind::Coin => 'o',
                    ObjKind::Power { .. } => '*',
                };
                let ch = if o.is_live(w.tick()) { ch } else { ch.to_ascii_lowercase() };
                let (c0, c1) = (col(o.x - o.half_w), col(o.x + o.half_w));
                for c in line.iter_mut().take(c1 + 1).skip(c0) {
                    if *c == ' ' || *c == '~' || *c == 'o' {
                        *c = ch;
                    }
                }
            }
        }
        for p in w.players() {
            if !p.role.is_runner() {
                continue;
            }
            let r = &p.runner;
            if (r.pos.z - z).abs() <= 1.0 {
                let ch = std::char::from_digit(p.id().0 as u32 + 1, 10).unwrap_or('?');
                line[col(r.pos.x)] = ch;
            }
        }
        let s: String = line.into_iter().collect();
        println!("|{}| {:6.0}", s, z);
        z -= 2.0;
    }
    let state: Vec<String> = w
        .runners()
        .map(|r| format!("P{} f{} {:?}{}", r.id.0 + 1, r.feathers, short(&r.loco), if r.protected() { "*" } else { "" }))
        .collect();
    println!("{}", state.join(" | "));
    let _ = PlayerId(0);
}

fn short(l: &flock_core::Loco) -> &'static str {
    use flock_core::Loco::*;
    match l {
        Running => "run",
        WindUp { .. } => "wind",
        Airborne { voluntary: true, .. } => "air",
        Airborne { .. } => "hop",
        Collapsed { .. } => "duck",
        Tumbling { .. } => "TUMBLE",
        Recovering { .. } => "rec",
    }
}
