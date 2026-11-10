//! Balance measurements (README "Feel notes"):
//!
//! cargo run -p flock-core --example measure --release -- [idle_seeds 20] [multi_seeds 24]
//!
//! 1. A no-input solo runner (nobody presses anything), seeds 1..=N: distance reached.
//! 2. Six normal-skill bots in multiplayer, seeds 1..=M: round length (`running_secs`).
//!
//! `FLOCK_TUNING='{"protect_time":4.5}'` overrides tuning fields (partial JSON).

use flock_core::{BotSkill, MatchConfig, Phase, World};

fn tuning() -> flock_core::Tuning {
    match std::env::var("FLOCK_TUNING") {
        Ok(j) => flock_core::Tuning::from_json(&j).expect("FLOCK_TUNING json"),
        Err(_) => flock_core::Tuning::default(),
    }
}

fn stats(mut v: Vec<f32>) -> (f32, f32, f32) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    let med = if n % 2 == 1 { v[n / 2] } else { 0.5 * (v[n / 2 - 1] + v[n / 2]) };
    (med, v[0], v[n - 1])
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let num = |i: usize, d: u64| args.get(i).and_then(|s| s.parse::<u64>().ok()).unwrap_or(d);
    let (idle_n, multi_n) = (num(0, 20), num(1, 24));

    let mut dists = Vec::new();
    for seed in 1..=idle_n {
        let mut cfg = MatchConfig::solo(seed);
        cfg.tuning = tuning();
        let mut w = World::new(cfg);
        let verbose = std::env::var("VERBOSE").is_ok();
        while w.phase() != Phase::Over && w.tick() < 60 * 60 * 30 {
            w.step(&[]);
            if verbose {
                let z = w.players()[0].runner.pos.z;
                for e in w.events() {
                    if matches!(e, flock_core::Event::Crash { .. } | flock_core::Event::Swagger { .. } | flock_core::Event::ShieldBroken { .. } | flock_core::Event::PowerTaken { .. } | flock_core::Event::ProtectionEnd { .. }) {
                        println!("    z {z:6.1} t {:5.1} {e:?}", w.running_secs());
                    }
                }
            }
        }
        let d = w.solo_result().map_or(0.0, |r| r.distance_m);
        println!("idle solo seed {seed:3}: {d:7.1} m  {:.1} s", w.running_secs());
        dists.push(d);
    }
    if !dists.is_empty() {
        let (m, lo, hi) = stats(dists);
        println!("IDLE SOLO distance: median {m:.1} m  min {lo:.1}  max {hi:.1}");
    }

    let mut durs = Vec::new();
    for seed in 1..=multi_n {
        let mut cfg = MatchConfig::multi(seed, 6, Some(BotSkill::NORMAL));
        cfg.tuning = tuning();
        let mut w = World::new(cfg);
        while w.phase() != Phase::Over && w.tick() < 60 * 60 * 40 {
            w.step(&[]);
        }
        println!("multi6 seed {seed:3}: {:7.1} s {:?}", w.running_secs(), w.end_reason());
        durs.push(w.running_secs());
    }
    if !durs.is_empty() {
        let (m, lo, hi) = stats(durs);
        println!("MULTI 6 normal bots: median {m:.1} s  min {lo:.1}  max {hi:.1}");
    }
}
