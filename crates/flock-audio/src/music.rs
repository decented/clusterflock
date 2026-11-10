//! Cluster Flock's music, synthesised once into buffers.
//!
//! * **Menu**: a calm, playful barnyard shuffle in C major, 124 bpm, 16 bars.
//! * **Run**: a runaway-barnyard hoedown in E mixolydian, 172 bpm, 48 bars (about
//!   67 s) built as four *stems* that loop in lockstep: `core` (always on), `drive`,
//!   `frenzy` and `tension` (the [`crate::deck::Deck`] fades them in and out with the
//!   game's intensity). Every note is written with wrap-around ([`add_wrapped`]) so
//!   each stem is one period of a periodic signal and the loop point is seamless.
//!
//! Instruments are all synthesised: Karplus-Strong banjo, a bowed-saw fiddle with
//! vibrato and slides, a slapped upright bass, washboard, cowbell, hooves, slide
//! whistle, goose honks, hen clucks and a clattering pan. No kick-and-clap four on
//! the floor, no sidechain, no synth stabs: the groove is a two-feel boom-chick
//! (train beat) with a galloping off-beat, and things keep tripping over.

use crate::dsp::*;
use crate::MusicTrack;
use std::collections::HashMap;
use std::f32::consts::LN_2;
use std::sync::Arc;

pub const MENU_BPM: f32 = 124.0;
pub const RUN_BPM: f32 = 172.0;
pub const MENU_BARS: usize = 16;
pub const RUN_BARS: usize = 48;
const SPB: usize = 16;

/// Stem indices of the run track.
pub const STEM_CORE: usize = 0;
pub const STEM_DRIVE: usize = 1;
pub const STEM_FRENZY: usize = 2;
pub const STEM_TENSION: usize = 3;

/// One rendered track: stems that loop together. `len` is the length of the longest
/// stem (the loop); shorter stems must divide it.
pub struct MusicData {
    pub bar_len: usize,
    pub len: usize,
    pub stems: Vec<Arc<[f32]>>,
    /// True if the stems are layers that follow the game's mood (see `Deck`).
    pub reactive: bool,
}

impl MusicData {
    /// A single-stem, non-reactive track.
    pub fn single(buf: Vec<f32>, bar_len: usize) -> MusicData {
        MusicData { bar_len, len: buf.len(), stems: vec![buf.into()], reactive: false }
    }
}

/// Render a music track.
pub fn render_music(track: MusicTrack) -> MusicData {
    match track {
        MusicTrack::Menu => menu(),
        MusicTrack::Run => run(),
    }
}

/// Length in samples of one loop of `track`.
pub fn loop_len(track: MusicTrack) -> usize {
    match track {
        MusicTrack::Menu => Grid::new(MENU_BPM, MENU_BARS).total,
        MusicTrack::Run => Grid::new(RUN_BPM, RUN_BARS).total,
    }
}

// ---- helpers -----------------------------------------------------------------------

struct Grid {
    step: usize,
    bar: usize,
    total: usize,
}

impl Grid {
    fn new(bpm: f32, bars: usize) -> Grid {
        let step = (SR * 60.0 / bpm / 4.0).round() as usize;
        Grid { step, bar: step * SPB, total: step * SPB * bars }
    }
    /// Sample index of `s` sixteenths into `bar`.
    fn at(&self, bar: usize, s: f32) -> usize {
        bar * self.bar + (s * self.step as f32).round() as usize
    }
    fn secs(&self, steps: f32) -> f32 {
        steps * self.step as f32 / SR
    }
}

struct Bus(Vec<f32>);

impl Bus {
    fn new(len: usize) -> Bus {
        Bus(vec![0.0; len])
    }
    fn put(&mut self, at: usize, src: &[f32], gain: f32) {
        add_wrapped(&mut self.0, at, src, gain);
    }
}

type Key = (u32, u32, u32, u32);
type Cache = HashMap<Key, Vec<f32>>;

fn cached(c: &mut Cache, key: Key, make: impl FnOnce() -> Vec<f32>) -> &[f32] {
    c.entry(key).or_insert_with(make)
}

/// Deterministic jitter in -1..1 for humanising a step.
fn jitter(i: usize) -> f32 {
    let mut r = Rng::new(i as u64 + 9000);
    r.signed()
}

fn smooth_pass(buf: &mut [f32], cutoff: f32) {
    lowpass(buf, cutoff);
    lowpass(buf, cutoff * 1.4);
}

// ---- voices ----------------------------------------------------------------------

/// Soft bass-drum thump (a felt beater, not an electronic kick).
fn thump(punch: f32) -> Vec<f32> {
    let mut ph = 0.0f32;
    let mut rng = Rng::new(3);
    let mut v = gen(0.24, |t, _| {
        let f = 50.0 + 70.0 * (-t * 50.0).fe();
        ph = (ph + f / SR).fract();
        fsin(ph) * (-t * 12.0).fe() * (1.0 - (-t * 900.0).fe()) + rng.signed() * (-t * 700.0).fe() * 0.22 * punch
    });
    lowpass(&mut v, 2600.0);
    v
}

/// A brushed snare: the "chick" of the boom-chick.
fn brush(seed: u64, hard: f32) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    let decay = 18.0 + 10.0 * hard;
    let mut v = gen(0.17, |t, _| rng.signed() * (-t * decay).fe() * (1.0 - (-t * 400.0).fe()));
    highpass(&mut v, 900.0);
    lowpass(&mut v, 5200.0);
    for (i, x) in v.iter_mut().enumerate() {
        let t = i as f32 / SR;
        *x += fsin(190.0 * t) * (-t * 40.0).fe() * 0.35 * hard;
    }
    v
}

/// One stroke of a washboard: a longer down-stroke and a short up-stroke.
fn washboard(down: bool, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    let (secs, dec) = if down { (0.09, 38.0) } else { (0.05, 75.0) };
    let mut v = gen(secs, |t, _| rng.signed() * (-t * dec).fe() * (1.0 - (-t * 1500.0).fe()));
    highpass(&mut v, 2200.0);
    lowpass(&mut v, 6200.0);
    v
}

fn cowbell(f: f32) -> Vec<f32> {
    let (d1, d2) = (f / SR, f * 1.504 / SR);
    let (mut p1, mut p2) = (0.0f32, 0.3f32);
    let mut v = gen(0.22, |t, _| {
        p1 = (p1 + d1).fract();
        p2 = (p2 + d2).fract();
        (pulse(p1, d1, 0.5) + pulse(p2, d2, 0.5)) * 0.5 * ((-t * 22.0).fe() * 0.6 + (-t * 75.0).fe() * 0.4) * (1.0 - (-t * 2500.0).fe())
    });
    highpass(&mut v, 400.0);
    lowpass(&mut v, 3600.0);
    v
}

fn woodblock(f: f32) -> Vec<f32> {
    gen(0.09, |t, _| (fsin(f * t) + 0.3 * fsin(f * 2.4 * t)) * (-t * 65.0).fe() * (1.0 - (-t * 3000.0).fe()))
}

/// A hoof / tom: a sine that drops in pitch.
fn tom(f: f32) -> Vec<f32> {
    let mut ph = 0.0f32;
    let mut v = gen(0.26, |t, _| {
        ph = (ph + f * (1.0 + 0.7 * (-t * 38.0).fe()) / SR).fract();
        fsin(ph) * (-t * 15.0).fe() * (1.0 - (-t * 1000.0).fe())
    });
    lowpass(&mut v, 3000.0);
    v
}

fn clap(seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    let mut v = gen(0.2, |t, _| {
        let mut e = (-t * 25.0).fe() * 0.3;
        for k in 0..3 {
            let tt = t - k as f32 * 0.009;
            if tt >= 0.0 {
                e += (-tt * 260.0).fe() * 0.8;
            }
        }
        rng.signed() * e
    });
    highpass(&mut v, 1000.0);
    lowpass(&mut v, 4800.0);
    v
}

/// A dropped tin pan: ringing inharmonic partials over a noisy clatter.
fn clatter(seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    let parts = [(830.0f32, 9.0f32), (1190.0, 11.0), (2230.0, 15.0), (3470.0, 22.0)];
    let mut v = gen(0.55, |t, _| {
        let mut x = rng.signed() * (-t * 14.0).fe() * 0.5;
        for &(f, d) in &parts {
            x += fsin(f * t) * (-t * d).fe() * 0.25;
        }
        x * (1.0 - (-t * 900.0).fe())
    });
    highpass(&mut v, 500.0);
    smooth_pass(&mut v, 5500.0);
    v
}

/// Slapped upright bass note.
fn upright(freq: f32, gate: f32) -> Vec<f32> {
    let mut rng = Rng::new(11);
    let mut ph = 0.0f32;
    let mut v = gen(gate + 0.04, |t, _| {
        ph = (ph + freq * (1.0 + 0.03 * (-t * 60.0).fe()) / SR).fract();
        let x = fsin(ph) + 0.6 * fsin(2.0 * ph) * (-t * 6.0).fe() + 0.3 * fsin(3.0 * ph) * (-t * 9.0).fe();
        (x * (-t * 4.4).fe() + rng.signed() * (-t * 300.0).fe() * 0.16) * (1.0 - (-t * 1500.0).fe()) * (1.0 - ((t - gate) / 0.04).clamp(0.0, 1.0))
    });
    lowpass(&mut v, 1900.0);
    v
}

fn banjo_note(midi: f32, secs: f32) -> Vec<f32> {
    pluck_ks(mtof(midi), secs, 0.78, 0.9965, 0.18, midi as u64 * 7 + 1)
}

/// A strummed chop of short, damped plucks; `down` strums low to high.
fn banjo_chop(strings: &[f32], down: bool, secs: f32) -> Vec<f32> {
    let gap = n(0.005);
    let mut buf = vec![0.0f32; n(secs + 0.05) + gap * strings.len()];
    let count = strings.len();
    for (k, &m) in strings.iter().enumerate() {
        let slot = if down { k } else { count - 1 - k };
        let note = pluck_ks(mtof(m), secs, 0.5, 0.99, 0.2, m as u64 * 5 + 2);
        add_wrapped(&mut buf, slot * gap, &note, 0.55);
    }
    buf
}

/// Bowed-saw fiddle: two detuned saws, vibrato after the first moments, a scoop up
/// from `slide` semitones below, body resonances and a touch of bow noise.
fn fiddle(freq: f32, gate: f32, slide: f32, bright: f32, seed: u64) -> Vec<f32> {
    let release = 0.05;
    let len = n(gate + release);
    let mut rng = Rng::new(seed);
    let (mut p1, mut p2) = (0.0f32, 0.37f32);
    let mut b1 = Svf::new(1250.0, 4.0);
    let mut b2 = Svf::new(2600.0, 3.0);
    let cut = (2400.0 + 2000.0 * bright).min(freq * 8.0 + 900.0);
    let a = 1.0 - (-std::f32::consts::TAU * cut / SR).exp();
    let mut lp = 0.0f32;
    (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            let vib = 1.0 + 0.0075 * fsin(5.9 * t) * ((t - 0.09) * 9.0).clamp(0.0, 1.0);
            let sl = if slide != 0.0 { fexp(-(slide / 12.0) * LN_2 * (-t * 26.0).fe()) } else { 1.0 };
            let f = freq * vib * sl;
            let (d1, d2) = (f * 0.9985 / SR, f * 1.0015 / SR);
            p1 = (p1 + d1).fract();
            p2 = (p2 + d2).fract();
            let x = 0.55 * saw(p1, d1) + 0.45 * saw(p2, d2);
            lp += a * (x - lp);
            let (_, bp1, _) = b1.tick(x);
            let (_, bp2, _) = b2.tick(x);
            let y = lp + 0.3 * bp1 + 0.15 * bp2 + rng.signed() * ((-t * 45.0).fe() * 0.18 + 0.012);
            let env = (t / 0.012).min(1.0) * (1.0 + 0.3 * (-t * 14.0).fe()) * (1.0 - ((t - gate) / release).clamp(0.0, 1.0));
            y * env * 0.55
        })
        .collect()
}

/// Slide whistle: a near-pure tone gliding exponentially from `f0` to `f1`.
fn whistle(f0: f32, f1: f32, secs: f32) -> Vec<f32> {
    let mut ph = 0.0f32;
    let mut rng = Rng::new(21);
    let ratio_ln = (f1 / f0).ln();
    gen(secs, |t, _| {
        let f = f0 * fexp(ratio_ln * t / secs);
        ph = (ph + f * (1.0 + 0.012 * fsin(6.5 * t)) / SR).fract();
        let env = (t / 0.025).min(1.0) * ((secs - t) / 0.05).clamp(0.0, 1.0);
        (fsin(ph) + 0.12 * fsin(2.0 * ph) + rng.signed() * 0.03) * env
    })
}

/// A goose honk: a bent, nasal saw through two vocal formants.
fn honk(freq: f32, secs: f32) -> Vec<f32> {
    let mut ph = 0.0f32;
    let mut b1 = Svf::new(720.0, 4.0);
    let mut b2 = Svf::new(1350.0, 4.0);
    gen(secs, |t, _| {
        let f = freq * (1.0 + 0.12 * (-t * 9.0).fe() - 0.1 * (t / secs)) * (1.0 + 0.02 * fsin(11.0 * t));
        let d = f / SR;
        ph = (ph + d).fract();
        let x = saw(ph, d);
        let (_, bp1, _) = b1.tick(x);
        let (_, bp2, _) = b2.tick(x);
        (bp1 + 0.7 * bp2 + 0.15 * x) * (t / 0.01).min(1.0) * ((secs - t) / 0.04).clamp(0.0, 1.0)
    })
}

/// "buk-buk-buk": three quick hen clucks, `spacing` seconds apart.
fn cluck(spacing: f32) -> Vec<f32> {
    let burst = 0.06;
    let mut buf = vec![0.0f32; n(spacing * 2.0 + burst + 0.02)];
    for k in 0..3 {
        let mut ph = 0.0f32;
        let mut b1 = Svf::new(900.0, 5.0);
        let mut b2 = Svf::new(1700.0, 4.0);
        let top = 620.0 - 60.0 * k as f32;
        let v = gen(burst, |t, _| {
            let d = top * (1.0 - 0.45 * t / burst) / SR;
            ph = (ph + d).fract();
            let x = saw(ph, d);
            let (_, bp1, _) = b1.tick(x);
            let (_, bp2, _) = b2.tick(x);
            (bp1 + 0.6 * bp2) * (-t * 40.0).fe() * (t / 0.003).min(1.0)
        });
        add_wrapped(&mut buf, n(spacing * k as f32), &v, 1.0);
    }
    buf
}

/// Menu lead: a breathy whistled flute with a little scoop and late vibrato.
fn flute(freq: f32, gate: f32) -> Vec<f32> {
    let release = 0.09;
    let mut ph = 0.0f32;
    let mut rng = Rng::new(17);
    let mut breath = 0.0f32;
    gen(gate + release, |t, _| {
        let scoop = fexp(-0.6 / 12.0 * LN_2 * (-t * 22.0).fe());
        let vib = 1.0 + 0.006 * fsin(5.2 * t) * ((t - 0.2) * 5.0).clamp(0.0, 1.0);
        ph = (ph + freq * scoop * vib / SR).fract();
        breath += 0.3 * (rng.signed() - breath);
        (fsin(ph) + 0.18 * fsin(2.0 * ph) + 0.06 * fsin(3.0 * ph) + breath * 0.07) * adsr(t, gate, 0.035, 0.15, 0.8, release)
    })
}

// ---- the run track -------------------------------------------------------------------

const E: usize = 0;
const D: usize = 1;
const A: usize = 2;
const B7: usize = 3;
const EM: usize = 4;
const C: usize = 5;

struct Ch {
    root: f32,
    alt: f32,
    strings: [f32; 5],
}

const CHORDS: [Ch; 6] = [
    Ch { root: 40.0, alt: 47.0, strings: [52.0, 56.0, 59.0, 64.0, 68.0] },
    Ch { root: 38.0, alt: 45.0, strings: [50.0, 54.0, 57.0, 62.0, 66.0] },
    Ch { root: 45.0, alt: 40.0, strings: [57.0, 61.0, 64.0, 69.0, 73.0] },
    Ch { root: 47.0, alt: 42.0, strings: [54.0, 57.0, 59.0, 63.0, 66.0] },
    Ch { root: 40.0, alt: 47.0, strings: [52.0, 55.0, 59.0, 64.0, 67.0] },
    Ch { root: 36.0, alt: 43.0, strings: [52.0, 55.0, 60.0, 64.0, 67.0] },
];

/// Chords per bar: A | B | A again | breakdown | B again | build.
const PROG: [usize; RUN_BARS] = [
    E, D, A, E, E, D, B7, B7, //
    A, A, E, E, A, B7, E, E, //
    E, D, A, E, E, D, A, B7, //
    EM, EM, C, D, EM, EM, C, B7, //
    A, A, E, E, A, B7, E, E, //
    E, D, A, E, E, A, B7, B7,
];

/// A fiddle note: (start step, length in steps, midi, scoop from n semitones below).
type Nt = (f32, f32, f32, f32);

fn runs(start: f32, notes: &[f32]) -> Vec<Nt> {
    notes.iter().enumerate().map(|(i, &m)| (start + i as f32, 1.0, m, 0.0)).collect()
}

fn plain(v: &[(f32, f32, f32)]) -> Vec<Nt> {
    v.iter().map(|&(s, l, m)| (s, l, m, 0.0)).collect()
}

/// The hook, transposed so that its tonic is `r`.
fn hook(r: f32) -> Vec<Nt> {
    const H: [(f32, f32, f32); 11] = [(0.0, 2.0, 0.0), (2.0, 1.0, 2.0), (3.0, 1.0, 0.0), (4.0, 2.0, -5.0), (6.0, 1.0, -8.0), (7.0, 1.0, -5.0), (8.0, 2.0, 0.0), (10.0, 1.0, 4.0), (11.0, 1.0, 0.0), (12.0, 2.0, -5.0), (14.0, 2.0, 0.0)];
    let mut v: Vec<Nt> = H.iter().map(|&(s, l, o)| (s, l, r + o, 0.0)).collect();
    v[0].3 = 2.0;
    v
}

fn melody(bar: usize) -> Vec<Nt> {
    match bar {
        0 | 4 => hook(76.0),
        1 | 5 => hook(74.0),
        2 => hook(81.0),
        3 => {
            let mut v = plain(&[(0.0, 3.0, 76.0), (3.0, 1.0, 71.0), (4.0, 2.0, 68.0), (6.0, 2.0, 71.0), (8.0, 4.0, 76.0)]);
            v.extend(runs(12.0, &[71.0, 73.0, 74.0, 76.0]));
            v
        }
        6 => plain(&[(0.0, 2.0, 83.0), (2.0, 1.0, 81.0), (3.0, 1.0, 78.0), (4.0, 2.0, 75.0), (6.0, 2.0, 78.0), (8.0, 2.0, 83.0), (10.0, 1.0, 81.0), (11.0, 1.0, 78.0), (12.0, 2.0, 75.0), (14.0, 2.0, 71.0)]),
        7 => {
            let mut v = runs(0.0, &[71.0, 75.0, 78.0, 81.0]);
            v.extend(plain(&[(4.0, 2.0, 83.0), (6.0, 2.0, 81.0), (8.0, 2.0, 78.0), (10.0, 2.0, 75.0), (12.0, 1.0, 78.0), (13.0, 1.0, 75.0), (15.0, 1.0, 71.0)]));
            v
        }
        // B: the high chase.
        8 | 12 => {
            let mut v = runs(0.0, if bar == 8 { &[81.0, 83.0, 85.0, 83.0, 81.0, 83.0, 85.0, 88.0] } else { &[85.0, 88.0, 85.0, 83.0, 81.0, 83.0, 85.0, 83.0] });
            v.extend(plain(&[(8.0, 2.0, if bar == 8 { 86.0 } else { 81.0 }), (10.0, 2.0, 85.0), (12.0, 4.0, if bar == 8 { 81.0 } else { 88.0 })]));
            v
        }
        9 => {
            let mut v = runs(0.0, &[88.0, 86.0, 85.0, 83.0, 81.0, 83.0, 85.0, 81.0]);
            v.extend(plain(&[(8.0, 2.0, 80.0), (10.0, 2.0, 81.0), (12.0, 2.0, 83.0), (14.0, 2.0, 85.0)]));
            v
        }
        10 => {
            let mut v = runs(0.0, &[83.0, 80.0, 76.0, 80.0, 83.0, 80.0, 76.0, 80.0]);
            v.extend(plain(&[(8.0, 4.0, 83.0), (12.0, 2.0, 80.0), (14.0, 2.0, 76.0)]));
            v
        }
        11 => {
            let mut v = runs(0.0, &[76.0, 78.0, 80.0, 81.0, 83.0, 81.0, 80.0, 78.0]);
            v.extend(plain(&[(8.0, 2.0, 76.0), (10.0, 1.0, 78.0), (11.0, 1.0, 76.0)]));
            v.push((12.0, 4.0, 71.0, 3.0));
            v
        }
        13 => {
            let mut v = runs(0.0, &[83.0, 81.0, 78.0, 75.0, 78.0, 81.0, 83.0, 87.0]);
            v.extend(plain(&[(8.0, 2.0, 86.0), (10.0, 2.0, 83.0), (12.0, 2.0, 81.0), (14.0, 2.0, 78.0)]));
            v
        }
        14 => {
            let mut v = runs(0.0, &[76.0, 80.0, 83.0, 88.0, 83.0, 80.0, 76.0, 71.0]);
            v.push((8.0, 4.0, 76.0, 0.0));
            v.extend(runs(12.0, &[76.0, 76.0, 76.0, 76.0]));
            v
        }
        15 => {
            let mut v = runs(0.0, &[88.0, 86.0, 83.0, 81.0, 80.0, 76.0, 73.0, 71.0]);
            v.extend(plain(&[(9.0, 1.0, 76.0), (10.5, 1.0, 64.0)]));
            v
        }
        // A again: low, growly start, then the hook an octave up.
        16..=19 => {
            let r = [64.0, 62.0, 69.0, 64.0][bar - 16];
            if bar == 19 {
                let mut v = plain(&[(0.0, 3.0, 64.0), (3.0, 1.0, 59.0), (4.0, 2.0, 56.0), (6.0, 2.0, 59.0), (8.0, 4.0, 64.0)]);
                v.extend(runs(12.0, &[59.0, 61.0, 62.0, 64.0]));
                v
            } else {
                hook(r)
            }
        }
        20 => hook(76.0),
        21 => hook(74.0),
        22 => hook(81.0),
        23 => {
            let mut v = plain(&[(0.0, 2.0, 83.0), (2.0, 2.0, 81.0), (4.0, 2.0, 78.0), (6.0, 2.0, 75.0), (8.0, 2.0, 83.0), (10.0, 1.0, 78.0), (11.0, 1.0, 75.0), (12.0, 2.0, 71.0)]);
            v.push((14.5, 1.0, 66.0, 0.0));
            v
        }
        // Breakdown: stop-time and a tumble down the stairs.
        24 => {
            let mut v = plain(&[(0.0, 3.0, 76.0), (7.0, 1.0, 76.0), (10.0, 1.0, 74.0)]);
            v.push((6.0, 1.0, 79.0, 1.0));
            v.push((12.0, 4.0, 71.0, 2.0));
            v
        }
        25 => plain(&[(0.0, 2.0, 76.0), (3.0, 1.0, 79.0), (4.0, 2.0, 83.0), (8.0, 1.0, 81.0), (9.0, 1.0, 79.0), (10.0, 1.0, 76.0), (12.0, 4.0, 71.0)]),
        26 => plain(&[(0.0, 3.0, 72.0), (4.0, 1.0, 76.0), (5.0, 1.0, 79.0), (6.0, 2.0, 76.0), (8.0, 4.0, 72.0), (12.0, 2.0, 67.0)]),
        27 => plain(&[(0.0, 3.0, 74.0), (4.0, 1.0, 78.0), (5.0, 1.0, 81.0), (6.0, 2.0, 78.0), (8.0, 4.0, 74.0), (12.0, 2.0, 69.0)]),
        28 => plain(&[(0.0, 3.0, 79.0), (4.0, 1.0, 83.0), (6.0, 1.0, 79.0), (7.0, 1.0, 76.0), (10.0, 2.0, 74.0), (12.0, 4.0, 76.0)]),
        29 => {
            let ms: Vec<f32> = (0..12).map(|i| 88.0 - i as f32).collect();
            let mut v = runs(0.0, &ms);
            v.extend(plain(&[(12.0, 2.0, 76.0), (14.0, 2.0, 71.0)]));
            v
        }
        30 => plain(&[(0.0, 4.0, 84.0), (4.0, 2.0, 83.0), (6.0, 2.0, 79.0), (8.0, 4.0, 76.0), (12.0, 2.0, 72.0), (14.0, 1.0, 71.0), (15.0, 1.0, 72.0)]),
        31 => plain(&[(0.0, 2.0, 83.0), (4.0, 1.0, 83.0), (8.0, 1.0, 83.0), (10.0, 1.0, 83.0), (12.0, 1.0, 83.0), (13.0, 1.0, 83.0), (14.0, 1.0, 83.0), (15.0, 1.0, 83.0)]),
        // B again: the same chase, all in.
        32..=39 => melody(bar - 24),
        // Build: the hook at full tilt, then a climbing chase and a stop.
        40 => hook(76.0),
        41 => hook(74.0),
        42 => hook(81.0),
        43 => melody(3),
        44 => runs(0.0, &[76.0, 80.0, 83.0, 80.0, 76.0, 80.0, 83.0, 80.0, 76.0, 80.0, 83.0, 80.0, 76.0, 80.0, 83.0, 86.0]),
        45 => runs(0.0, &[81.0, 85.0, 88.0, 85.0, 81.0, 85.0, 88.0, 85.0, 81.0, 85.0, 88.0, 85.0, 81.0, 85.0, 88.0, 90.0]),
        46 => runs(0.0, &[83.0, 87.0, 90.0, 87.0, 83.0, 87.0, 90.0, 87.0, 83.0, 87.0, 90.0, 87.0, 83.0, 87.0, 90.0, 93.0]),
        _ => {
            let mut v = runs(0.0, &[90.0, 87.0, 83.0, 78.0]);
            v.extend(plain(&[(6.0, 1.0, 83.0), (7.0, 1.0, 81.0), (8.0, 1.0, 78.0), (10.0, 1.0, 75.0), (12.0, 1.0, 71.0)]));
            v
        }
    }
}

/// A third below `m`, if it fits the key (E mixolydian plus the D sharp of B7).
fn third_below(m: f32) -> f32 {
    for d in [4.0, 3.0, 5.0] {
        if matches!(((m - d) as i32).rem_euclid(12), 4 | 6 | 8 | 9 | 11 | 1 | 2 | 3) {
            return m - d;
        }
    }
    m - 12.0
}

fn put_fiddle(bus: &mut Bus, g: &Grid, cache: &mut Cache, bar: usize, notes: &[Nt], gain: f32, bright: f32) {
    for &(s, len, m, sl) in notes {
        let midi = m;
        let gate = (g.secs(len) * 0.93).max(0.05);
        let key = ((midi * 2.0) as u32, (len * 4.0) as u32, (sl * 2.0) as u32, (bright * 10.0) as u32);
        let note = cached(cache, key, || fiddle(mtof(midi), gate, sl, bright, midi as u64 + 100));
        bus.put(g.at(bar, s), note, gain);
    }
}

/// Things falling over: toms that stumble, with one late hit.
fn stumble(bus: &mut Bus, g: &Grid, bar: usize, gain: f32) {
    for (i, (s, f)) in [(12.0f32, 210.0f32), (13.0, 170.0), (14.0, 150.0), (14.5, 130.0), (15.45, 95.0)].into_iter().enumerate() {
        bus.put(g.at(bar, s), &tom(f), gain * (0.8 + 0.07 * i as f32));
    }
}

fn is_breakdown(bar: usize) -> bool {
    (24..32).contains(&bar)
}

fn run_core(g: &Grid) -> Vec<f32> {
    let mut bus = Bus::new(g.total);
    let mut cache = Cache::new();
    let (kick, chick_h, chick_s) = (thump(1.0), brush(5, 1.0), brush(6, 0.4));
    let shk_up = washboard(false, 9);
    for bar in 0..RUN_BARS {
        let ch = &CHORDS[PROG[bar]];
        let nxt = &CHORDS[PROG[(bar + 1) % RUN_BARS]];
        let bd = is_breakdown(bar);
        let stumble_bar = bar % 8 == 7;
        // Boom: bass and beater on 1 and 3. Breakdown is stop-time.
        let beats: &[f32] = if bd && bar < 31 { if bar % 2 == 1 { &[0.0, 8.0] } else { &[0.0] } } else { &[0.0, 8.0] };
        for &s in beats {
            let (m, kg) = if s == 0.0 { (ch.root, 0.9) } else { (ch.alt, 0.75) };
            if bar % 8 == 7 && s == 8.0 && bar % 16 == 15 {
                continue; // a missed beat: the whole farm trips
            }
            bus.put(g.at(bar, s), &kick, kg);
            let bass = cached(&mut cache, (m as u32, 0, 0, 1), || upright(mtof(m), g.secs(3.2)));
            bus.put(g.at(bar, s), bass, 0.55);
        }
        // A walking pickup into the next chord.
        if !bd && (nxt.root != ch.root || stumble_bar) {
            let m = if nxt.root > ch.root { nxt.root - 1.0 } else { nxt.root + 1.0 };
            let bass = cached(&mut cache, (m as u32, 1, 0, 1), || upright(mtof(m), g.secs(1.0)));
            bus.put(g.at(bar, 14.0), bass, 0.35);
        }
        // Chick: brush and banjo chop on 2 and 4, softer up-strokes between.
        if !bd {
            for s in [4.0f32, 12.0] {
                bus.put(g.at(bar, s), &chick_h, 0.34);
                let chop = cached(&mut cache, (PROG[bar] as u32, 2, 0, 2), || banjo_chop(&ch.strings, true, 0.1));
                bus.put(g.at(bar, s), chop, 0.2);
            }
            for s in [6.0f32, 14.0] {
                let late = if stumble_bar && s == 14.0 { 0.5 } else { 0.0 };
                let up = cached(&mut cache, (PROG[bar] as u32, 3, 0, 2), || banjo_chop(&ch.strings[2..], false, 0.07));
                bus.put(g.at(bar, s + late), up, 0.11);
            }
            for s in [2.0f32, 6.0, 10.0, 14.0] {
                bus.put(g.at(bar, s) + (jitter(bar * 16 + s as usize) * 120.0).max(0.0) as usize, &shk_up, 0.07);
            }
        } else if bar < 31 {
            let hit = cached(&mut cache, (PROG[bar] as u32, 4, 0, 2), || banjo_chop(&ch.strings, true, 0.28));
            bus.put(g.at(bar, 0.0), hit, 0.3);
            if bar % 2 == 1 {
                bus.put(g.at(bar, 8.0), hit, 0.22);
            }
        }
        // Build into the next section: a roll of brushes and the stumble.
        if bar == 31 {
            for s in 0..16 {
                bus.put(g.at(bar, s as f32), if s % 2 == 0 { &chick_h } else { &chick_s }, 0.1 + 0.035 * s as f32);
            }
        } else if stumble_bar || bar % 16 == 11 {
            stumble(&mut bus, g, bar, 0.42);
        }
        // Lead.
        let notes = melody(bar);
        let (gain, bright) = if bd { (0.42, 0.2) } else { (0.46, 0.5) };
        put_fiddle(&mut bus, g, &mut cache, bar, &notes, gain, bright);
        // Stingers: whistle swoops at the end of sections, clucks, a pan crash.
        match bar {
            7 | 23 | 39 => bus.put(g.at(bar, 11.0), &whistle(500.0, 1500.0, g.secs(5.0)), 0.2),
            15 => bus.put(g.at(bar, 10.5), &whistle(1700.0, 420.0, g.secs(5.0)), 0.2),
            31 => {
                bus.put(g.at(bar, 4.0), &whistle(450.0, 1250.0, g.secs(5.0)), 0.18);
                bus.put(g.at(bar, 10.0), &whistle(700.0, 1900.0, g.secs(5.0)), 0.2);
            }
            47 => {
                bus.put(g.at(bar, 10.0), &whistle(500.0, 1900.0, g.secs(5.0)), 0.22);
                bus.put(g.at(bar, 14.0), &honk(300.0, 0.2), 0.22);
            }
            _ => {}
        }
        if matches!(bar, 3 | 11 | 19 | 35 | 43) {
            bus.put(g.at(bar, 10.0), &cluck(g.secs(1.0)), 0.3);
        }
        if bar % 8 == 0 && bar > 0 || bar == 24 {
            bus.put(g.at(bar, 0.0), &clatter(40 + bar as u64), 0.14);
        }
    }
    echo_circular(&mut bus.0, g.step * 3, 0.35, 2, 0.12);
    smooth_pass(&mut bus.0, 6000.0);
    bus.0
}

fn run_drive(g: &Grid) -> Vec<f32> {
    let mut bus = Bus::new(g.total);
    let mut cache = Cache::new();
    let wash = [washboard(true, 18), washboard(false, 19), washboard(true, 20)];
    let (bell_a, bell_b) = (cowbell(560.0), cowbell(800.0));
    let clp = clap(30);
    let roll: [[usize; 8]; 2] = [[0, 2, 4, 1, 3, 4, 2, 3], [4, 2, 0, 3, 1, 2, 4, 3]];
    for bar in 0..RUN_BARS {
        if (24..28).contains(&bar) {
            continue;
        }
        let ch = &CHORDS[PROG[bar]];
        let tail = (28..32).contains(&bar);
        // Washboard sixteenths, a long scrape on every beat.
        for s in 0..16usize {
            let accent = s % 4 == 0;
            let w = if s % 2 == 0 { &wash[if accent { 0 } else { 2 }] } else { &wash[1] };
            let g_ = if accent { 0.32 } else if s % 2 == 0 { 0.22 } else { 0.14 };
            let late = (jitter(bar * 16 + s + 3) * 90.0).max(0.0) as usize;
            bus.put(g.at(bar, s as f32) + late, w, g_ * if tail { 0.7 } else { 1.0 });
        }
        if tail && bar < 31 {
            continue;
        }
        // Cowbell on the off-beats, a high-low pair.
        for (i, s) in [2.0f32, 6.0, 10.0, 14.0].into_iter().enumerate() {
            bus.put(g.at(bar, s), if i % 2 == 0 { &bell_a } else { &bell_b }, 0.2);
        }
        // Banjo roll: the sixteenth-note ripple.
        for s in 0..16usize {
            let idx = roll[s / 8][s % 8];
            let m = ch.strings[idx];
            let note = cached(&mut cache, (m as u32, 5, 0, 3), || banjo_note(m, 0.26));
            let acc = if s % 4 == 0 { 0.2 } else { 0.13 };
            bus.put(g.at(bar, s as f32), note, acc);
        }
        // Galloping bass bounce after each beat: da-DUM.
        for beat in 0..4usize {
            let (m1, m2) = (ch.root + 12.0, ch.root);
            let b1 = cached(&mut cache, (m1 as u32, 6, 0, 1), || upright(mtof(m1), g.secs(0.8)));
            bus.put(g.at(bar, (beat * 4 + 2) as f32), b1, 0.22);
            let b2 = cached(&mut cache, (m2 as u32, 6, 0, 2), || upright(mtof(m2), g.secs(0.8)));
            bus.put(g.at(bar, (beat * 4 + 3) as f32), b2, 0.2);
        }
        // Hand-claps on the second half of the form.
        if bar >= 16 && bar % 2 == 1 {
            for s in [4.0f32, 12.0] {
                bus.put(g.at(bar, s), &clp, 0.16);
            }
        }
    }
    smooth_pass(&mut bus.0, 6000.0);
    bus.0
}

fn run_frenzy(g: &Grid) -> Vec<f32> {
    let mut bus = Bus::new(g.total);
    let mut cache = Cache::new();
    let (tom_a, tom_b) = (tom(160.0), tom(120.0));
    for bar in 0..RUN_BARS {
        let quiet = (24..30).contains(&bar);
        // A second fiddle a third below the tune.
        if !quiet {
            let notes: Vec<Nt> = melody(bar).into_iter().filter(|n| n.1 >= 1.0).map(|(s, l, m, _)| (s + 0.04, l, third_below(m), 0.0)).collect();
            if bar >= 4 {
                put_fiddle(&mut bus, g, &mut cache, bar, &notes, 0.3, 0.6);
            }
            // Galloping hooves on the off-beats.
            for s in [2.0f32, 3.0, 6.0, 7.0, 10.0, 11.0, 14.0, 15.0] {
                bus.put(g.at(bar, s), if (s as usize).is_multiple_of(2) { &tom_a } else { &tom_b }, 0.2);
            }
        } else if bar >= 30 {
            for s in [8.0f32, 10.0, 12.0, 13.0, 14.0, 15.0] {
                bus.put(g.at(bar, s), &tom_a, 0.28);
            }
        }
        if !quiet && bar % 8 == 3 {
            bus.put(g.at(bar, 8.0), &honk(330.0, 0.22), 0.2);
            bus.put(g.at(bar, 10.0), &honk(262.0, 0.26), 0.2);
        }
        if !quiet && bar % 4 == 1 && bar >= 8 {
            bus.put(g.at(bar, 14.0), &cluck(g.secs(1.0)), 0.28);
        }
        if bar % 8 == 7 && !quiet {
            stumble(&mut bus, g, bar, 0.2);
        }
        if bar % 8 == 0 && bar >= 8 {
            bus.put(g.at(bar, 0.0), &clatter(70 + bar as u64), 0.12);
        }
        if bar == 47 {
            bus.put(g.at(bar, 8.0), &honk(300.0, 0.18), 0.22);
            bus.put(g.at(bar, 11.0), &honk(300.0, 0.18), 0.22);
        }
    }
    smooth_pass(&mut bus.0, 6000.0);
    bus.0
}

/// Eight bars, looped by the deck: a stomping heartbeat, a ticking clock, a
/// tremolo-bowed drone with a gulp of F against the E, a rising siren and a hen in a panic.
fn run_tension(g: &Grid) -> Vec<f32> {
    let bars = 8;
    let g8 = Grid { step: g.step, bar: g.bar, total: g.bar * bars };
    let mut bus = Bus::new(g8.total);
    let (stomp, tick_hi, tick_lo) = (thump(1.4), woodblock(1500.0), woodblock(1000.0));
    let drone: Vec<Vec<f32>> = [76.0f32, 77.0, 83.0].iter().map(|&m| fiddle(mtof(m), g8.secs(0.8), 0.0, 0.3, m as u64)).collect();
    for bar in 0..bars {
        for beat in 0..4usize {
            let s = (beat * 4) as f32;
            bus.put(g8.at(bar, s), &stomp, if beat == 0 { 0.7 } else { 0.5 });
            bus.put(g8.at(bar, s + 1.5), &stomp, 0.28);
        }
        for s in 0..16usize {
            bus.put(g8.at(bar, s as f32), if s % 4 == 0 { &tick_hi } else { &tick_lo }, if s % 4 == 0 { 0.2 } else { 0.08 });
        }
        // The drone: E5 and B5 bowed in a rapid tremolo (sixteenths re-bowed).
        let low = if bar % 4 == 3 { &drone[1] } else { &drone[0] };
        for s in 0..16usize {
            bus.put(g8.at(bar, s as f32), low, 0.1);
            bus.put(g8.at(bar, s as f32), &drone[2], 0.1);
        }
        // F against E on the way round, as a bass gulp.
        if bar % 4 == 3 {
            bus.put(g8.at(bar, 12.0), &upright(mtof(41.0), g8.secs(3.0)), 0.4);
        } else {
            bus.put(g8.at(bar, 12.0), &upright(mtof(40.0), g8.secs(3.0)), 0.3);
        }
    }
    bus.put(g8.at(0, 0.0), &whistle(420.0, 1500.0, g8.secs(31.0)), 0.12);
    bus.put(g8.at(4, 0.0), &whistle(420.0, 1500.0, g8.secs(31.0)), 0.12);
    bus.put(g8.at(6, 6.0), &cluck(g.secs(1.0)), 0.3);
    bus.put(g8.at(7, 4.0), &honk(300.0, 0.2), 0.2);
    smooth_pass(&mut bus.0, 6000.0);
    bus.0
}

fn run() -> MusicData {
    let g = Grid::new(RUN_BPM, RUN_BARS);
    let mut stems: Vec<Vec<f32>> = std::thread::scope(|sc| {
        let hs = [
            sc.spawn(|| run_core(&g)),
            sc.spawn(|| run_drive(&g)),
            sc.spawn(|| run_frenzy(&g)),
            sc.spawn(|| run_tension(&g)),
        ];
        hs.into_iter().map(|h| h.join().expect("music stem thread")).collect()
    });
    // Layer weights: the percussion and frenzy stems are all transients and quiet by
    // nature, so lift them until each layer adds about as much level as it should.
    for (s, w) in stems.iter_mut().zip([1.0f32, 2.6, 2.4, 1.2]) {
        s.iter_mut().for_each(|x| *x *= w);
    }
    // Joint level: the 99.8th percentile of the sum of every stem sits at 0.85, stems
    // keep their balance; the deck's soft clip catches the rare louder transient.
    let mut mags: Vec<f32> = (0..g.total).step_by(3).map(|i| stems.iter().map(|s| s[i % s.len()]).sum::<f32>().abs()).collect();
    let idx = (mags.len() as f32 * 0.998) as usize;
    let (_, p, _) = mags.select_nth_unstable_by(idx, |a, b| a.partial_cmp(b).unwrap());
    let k = 0.85 / (*p).max(1e-6);
    for s in stems.iter_mut() {
        s.iter_mut().for_each(|x| *x *= k);
    }
    MusicData { bar_len: g.bar, len: g.total, stems: stems.into_iter().map(Into::into).collect(), reactive: true }
}

// ---- menu --------------------------------------------------------------------------

fn menu() -> MusicData {
    let g = Grid::new(MENU_BPM, MENU_BARS);
    let swing = |s: usize| -> f32 { if s % 4 == 2 { 0.3 } else { 0.0 } };
    // C | C | F | C | G | F | C | G, then C | Am | F | G | C | F | G | C.
    struct M {
        root: f32,
        alt: f32,
        strings: [f32; 4],
    }
    let c = M { root: 36.0, alt: 43.0, strings: [52.0, 55.0, 60.0, 64.0] };
    let f = M { root: 41.0, alt: 36.0, strings: [53.0, 57.0, 60.0, 65.0] };
    let gg = M { root: 43.0, alt: 38.0, strings: [55.0, 59.0, 62.0, 67.0] };
    let am = M { root: 33.0, alt: 40.0, strings: [52.0, 57.0, 60.0, 64.0] };
    let prog = [&c, &c, &f, &c, &gg, &f, &c, &gg, &c, &am, &f, &gg, &c, &f, &gg, &c];
    let tune: [&[(f32, f32, f32)]; 16] = [
        &[(0.0, 2.0, 76.0), (2.0, 2.0, 79.0), (4.0, 2.0, 84.0), (6.0, 2.0, 79.0), (8.0, 3.0, 76.0), (11.0, 1.0, 74.0), (12.0, 4.0, 72.0)],
        &[(0.0, 2.0, 76.0), (2.0, 2.0, 79.0), (4.0, 2.0, 84.0), (6.0, 2.0, 86.0), (8.0, 4.0, 84.0), (12.0, 4.0, 79.0)],
        &[(0.0, 2.0, 77.0), (2.0, 2.0, 81.0), (4.0, 2.0, 84.0), (6.0, 2.0, 81.0), (8.0, 3.0, 77.0), (11.0, 1.0, 76.0), (12.0, 4.0, 77.0)],
        &[(0.0, 2.0, 76.0), (2.0, 1.0, 77.0), (3.0, 1.0, 76.0), (4.0, 2.0, 72.0), (6.0, 2.0, 76.0), (8.0, 8.0, 79.0)],
        &[(0.0, 2.0, 74.0), (2.0, 2.0, 79.0), (4.0, 2.0, 83.0), (6.0, 2.0, 79.0), (8.0, 3.0, 74.0), (11.0, 1.0, 71.0), (12.0, 4.0, 74.0)],
        &[(0.0, 2.0, 77.0), (2.0, 2.0, 81.0), (4.0, 2.0, 84.0), (6.0, 2.0, 81.0), (8.0, 4.0, 77.0), (12.0, 2.0, 76.0), (14.0, 2.0, 74.0)],
        &[(0.0, 2.0, 72.0), (2.0, 2.0, 76.0), (4.0, 2.0, 79.0), (6.0, 2.0, 84.0), (8.0, 4.0, 83.0), (12.0, 4.0, 79.0)],
        &[(0.0, 2.0, 74.0), (2.0, 2.0, 77.0), (4.0, 2.0, 79.0), (6.0, 1.0, 77.0), (7.0, 1.0, 76.0), (8.0, 2.0, 74.0), (10.0, 2.0, 71.0), (12.0, 4.0, 67.0)],
        &[(2.0, 2.0, 84.0), (4.0, 2.0, 84.0), (6.0, 2.0, 86.0), (8.0, 2.0, 88.0), (10.0, 2.0, 86.0), (12.0, 4.0, 84.0)],
        &[(2.0, 2.0, 81.0), (4.0, 2.0, 81.0), (6.0, 2.0, 84.0), (8.0, 2.0, 88.0), (10.0, 2.0, 84.0), (12.0, 4.0, 81.0)],
        &[(0.0, 2.0, 84.0), (2.0, 2.0, 81.0), (4.0, 2.0, 77.0), (6.0, 2.0, 81.0), (8.0, 4.0, 84.0), (12.0, 4.0, 81.0)],
        &[(0.0, 2.0, 83.0), (2.0, 2.0, 79.0), (4.0, 2.0, 74.0), (6.0, 2.0, 79.0), (8.0, 4.0, 83.0), (12.0, 2.0, 86.0), (14.0, 2.0, 83.0)],
        &[(0.0, 2.0, 84.0), (2.0, 2.0, 79.0), (4.0, 2.0, 76.0), (6.0, 2.0, 79.0), (8.0, 2.0, 84.0), (10.0, 2.0, 88.0), (12.0, 4.0, 86.0)],
        &[(0.0, 2.0, 84.0), (2.0, 2.0, 81.0), (4.0, 2.0, 77.0), (6.0, 2.0, 81.0), (8.0, 4.0, 89.0), (12.0, 4.0, 84.0)],
        &[(0.0, 2.0, 83.0), (2.0, 2.0, 79.0), (4.0, 2.0, 83.0), (6.0, 2.0, 86.0), (8.0, 2.0, 83.0), (10.0, 2.0, 79.0), (12.0, 4.0, 74.0)],
        &[(0.0, 3.0, 84.0), (4.0, 2.0, 79.0), (6.0, 2.0, 76.0), (8.0, 4.0, 72.0), (14.0, 2.0, 76.0)],
    ];
    let mut cache = Cache::new();
    let mut bus = Bus::new(g.total);
    let mut lead = Bus::new(g.total);
    let (kick, rim, hoof_hi, hoof_lo) = (thump(0.5), woodblock(1900.0), woodblock(1250.0), woodblock(820.0));
    let (wd, wu) = (washboard(true, 50), washboard(false, 51));
    for bar in 0..MENU_BARS {
        let ch = prog[bar];
        let sw = |s: usize| g.at(bar, s as f32 + swing(s));
        // Two-feel upright: root and fifth.
        for (s, m, len, gain) in [(0usize, ch.root, 5.0f32, 0.55f32), (8, ch.alt, 5.0, 0.5)] {
            let note = cached(&mut cache, (m as u32, len as u32, 0, 1), || upright(mtof(m), g.secs(len)));
            bus.put(g.at(bar, s as f32), note, gain);
        }
        bus.put(g.at(bar, 0.0), &kick, 0.35);
        bus.put(g.at(bar, 8.0), &kick, 0.28);
        // Uke-ish chops on 2 and 4, light.
        for s in [4usize, 12] {
            let chop = cached(&mut cache, (ch.root as u32, 2, 0, 2), || banjo_chop(&ch.strings, true, 0.12));
            bus.put(g.at(bar, s as f32), chop, 0.2);
        }
        // Swung washboard shuffle.
        for s in (0..16).step_by(2) {
            bus.put(sw(s), if s % 4 == 0 { &wd } else { &wu }, if s % 4 == 0 { 0.1 } else { 0.07 });
        }
        // Clip-clop hooves in the second half.
        if bar >= 8 {
            for (s, hi) in [(0usize, true), (2, false), (8, true), (10, false)] {
                bus.put(sw(s), if hi { &hoof_hi } else { &hoof_lo }, 0.12);
            }
        } else if bar % 4 == 3 {
            bus.put(g.at(bar, 15.0), &rim, 0.2);
        }
        for &(s, len, m) in tune[bar] {
            let gate = g.secs(len) * 0.94;
            let note = cached(&mut cache, ((m * 2.0) as u32, (len * 4.0) as u32, 0, 7), || flute(mtof(m), gate));
            lead.put(g.at(bar, s + swing(s as usize)), note, 0.22);
        }
        match bar {
            7 => bus.put(g.at(bar, 13.0), &cluck(g.secs(1.0)), 0.22),
            15 => bus.put(g.at(bar, 11.0), &whistle(700.0, 1700.0, g.secs(4.0)), 0.12),
            _ => {}
        }
    }
    echo_circular(&mut lead.0, g.step * 3, 0.4, 2, 0.28);
    let mut out: Vec<f32> = (0..g.total).map(|i| bus.0[i] + lead.0[i]).collect();
    smooth_pass(&mut out, 6500.0);
    out.iter_mut().for_each(|x| *x = softclip(*x * 1.1));
    normalize(&mut out, 0.7);
    MusicData::single(out, g.bar)
}
