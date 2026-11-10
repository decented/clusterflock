//! Render the music to WAV files so you can listen, with the levels and synthesis times.
//!
//!     cargo run -p flock-audio --release --example render -- [--out DIR]
//!
//! The default directory is `docs/notes/audio/` under the repo root (gitignored). Every file is
//! played through a real `Deck`, exactly as the game does, with a fixed (or, for the
//! `ramp` files, a moving) `Mood`:
//!
//!   menu              the menu loop, 1.1 loops
//!   run_low           run music at low speed (core layer only)
//!   run_top           top speed (all speed layers, fastest tempo)
//!   run_tension       top speed plus the final-runner tension layer
//!   run_ghost         low-mid speed; the player becomes a ghost after 10 s
//!   run_ramp          intensity climbs 0 -> 1 over the loop, tension joins in the last 25 s
//!   run_stem_<name>   each stem on its own (at full gain), one loop
use flock_audio::dsp::SAMPLE_RATE;
use flock_audio::music::{self, MusicData};
use flock_audio::{Deck, Mood, MusicTrack};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

fn write_wav(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data_len).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?;
    f.write_all(&SAMPLE_RATE.to_le_bytes())?;
    f.write_all(&(SAMPLE_RATE * 2).to_le_bytes())?;
    f.write_all(&2u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data_len.to_le_bytes())?;
    for s in samples {
        f.write_all(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes())?;
    }
    f.flush()
}

/// CPU time of the whole process so far (all threads), from /proc, in ms (10 ms ticks).
fn cpu_ms() -> f32 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let rest = stat.rsplit(')').next().unwrap_or("");
    let f: Vec<&str> = rest.split_whitespace().collect();
    let tick = |i: usize| f.get(i).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
    (tick(11) + tick(12)) * 10.0
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

/// Share of the energy above ~4 kHz (a one-pole high-pass is enough to compare takes).
fn hf_share(b: &[f32]) -> f32 {
    let mut h = b.to_vec();
    flock_audio::dsp::highpass(&mut h, 4000.0);
    let e: f32 = b.iter().map(|x| x * x).sum();
    let eh: f32 = h.iter().map(|x| x * x).sum();
    eh / e.max(1e-9)
}

fn report(name: &str, secs: f32, b: &[f32]) {
    let peak = b.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let rms = (b.iter().map(|x| x * x).sum::<f32>() / b.len() as f32).sqrt();
    let step = b.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    println!("{name:18} {secs:6.1} s   peak {:6.1} dBFS   rms {:6.1} dBFS   >4k {:4.1} %   max step {step:.2}", db(peak), db(rms), 100.0 * hf_share(b));
}

fn play(data: &Arc<MusicData>, secs: f32, mood: impl Fn(f32) -> Mood) -> Vec<f32> {
    let n = (secs * SAMPLE_RATE as f32) as usize;
    let mut deck = Deck::new(Arc::clone(data), &mood(0.0));
    (0..n).map(|i| deck.next(&mood(i as f32 / SAMPLE_RATE as f32))).collect()
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir: PathBuf = match args.iter().position(|a| a == "--out") {
        Some(i) => PathBuf::from(&args[i + 1]),
        None => PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/notes/audio"),
    };
    std::fs::create_dir_all(&dir)?;

    let mut datas = Vec::new();
    for t in MusicTrack::ALL {
        let (t0, c0) = (Instant::now(), cpu_ms());
        let d = music::render_music(t);
        let cpu = cpu_ms() - c0;
        let ms = t0.elapsed().as_secs_f32() * 1000.0;
        let mb: f32 = d.stems.iter().map(|s| s.len() * 4).sum::<usize>() as f32 / 1e6;
        println!("synth {:5} {ms:6.1} ms wall, {cpu:5.0} ms cpu (all threads), loop {:.1} s, {} stem(s), {mb:.1} MB", t.name(), d.len as f32 / SAMPLE_RATE as f32, d.stems.len());
        datas.push((t, Arc::new(d)));
    }
    let menu = &datas[0].1;
    let run = &datas[1].1;
    let loop_s = run.len as f32 / SAMPLE_RATE as f32;

    let mut out: Vec<(String, Vec<f32>)> = vec![
        ("menu".into(), play(menu, menu.len as f32 / SAMPLE_RATE as f32 * 1.1, |_| Mood::neutral())),
        ("run_low".into(), play(run, loop_s + 3.0, |_| Mood::neutral())),
        ("run_top".into(), play(run, loop_s + 3.0, |_| Mood { intensity: 1.0, ..Mood::neutral() })),
        ("run_tension".into(), play(run, loop_s + 3.0, |_| Mood { intensity: 1.0, tension: 1.0, ghost: 0.0 })),
        ("run_ghost".into(), play(run, 40.0, |t| Mood { intensity: 0.55, tension: 0.0, ghost: if t > 10.0 { 1.0 } else { 0.0 } })),
        (
            "run_ramp".into(),
            play(run, loop_s, |t| Mood { intensity: (t / (loop_s - 10.0)).clamp(0.0, 1.0), tension: ((t - (loop_s - 25.0)) / 4.0).clamp(0.0, 1.0), ghost: 0.0 }),
        ),
    ];
    for (i, name) in ["core", "drive", "frenzy", "tension"].iter().enumerate() {
        let stem = &run.stems[i];
        let n = run.len.max(stem.len());
        out.push((format!("run_stem_{name}"), (0..n).map(|k| stem[k % stem.len()]).collect()));
    }
    for (name, b) in &out {
        write_wav(&dir.join(format!("{name}.wav")), b)?;
        report(name, b.len() as f32 / SAMPLE_RATE as f32, b);
    }
    let t = flock_audio::deck::targets(&Mood { intensity: 1.0, tension: 1.0, ghost: 0.0 });
    println!("top-speed rate {:.3} -> {:.0} bpm; with tension {:.3}", flock_audio::deck::targets(&Mood { intensity: 1.0, ..Mood::neutral() }).rate, 172.0 * flock_audio::deck::targets(&Mood { intensity: 1.0, ..Mood::neutral() }).rate, t.rate);
    println!("wrote {} files to {}", out.len(), dir.display());
    Ok(())
}
