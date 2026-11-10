//! The music deck: plays one track's stems in lockstep and follows the game's mood.
//!
//! The mixer owns two decks (the current track and the one fading out); the WAV
//! renderer example drives a deck directly, so what it writes is what the game plays.

use crate::dsp::SR;
use crate::music::MusicData;
use std::f32::consts::TAU;
use std::sync::Arc;

/// What the game tells the music, every frame. All fields 0..1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Mood {
    /// How hard the run is going: course speed, or fewer runners left. Brings in the
    /// percussion layer, then the frenzy layer, and nudges the tempo up (to ~ +6 %).
    pub intensity: f32,
    /// Final-runner / last-seconds dread: the tension layer and a touch more tempo.
    pub tension: f32,
    /// The local player is a ghost: the music goes dark, woozy and echoing.
    pub ghost: f32,
}

impl Mood {
    pub fn neutral() -> Mood {
        Mood::default()
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What a mood asks of a reactive track: layer gains (drive, frenzy, tension),
/// playback rate and ghost amount.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Targets {
    pub layers: [f32; 3],
    pub rate: f32,
    pub ghost: f32,
}

pub fn targets(m: &Mood) -> Targets {
    let i = m.intensity.clamp(0.0, 1.0);
    let t = m.tension.clamp(0.0, 1.0);
    let gh = m.ghost.clamp(0.0, 1.0);
    // The tension layer needs the groove under it; as a ghost the dread is not for you.
    let layers = [smoothstep(0.12, 0.45, i).max(0.6 * t), smoothstep(0.55, 0.85, i), t * (1.0 - 0.8 * gh)];
    let rate = (1.0 + 0.06 * smoothstep(0.1, 1.0, i) + 0.03 * t) * (1.0 - 0.07 * gh);
    Targets { layers, rate, ghost: gh }
}

/// Level compensation: fewer layers means fewer voices, so lift the sum a little.
fn comp(layers: &[f32; 3]) -> f32 {
    1.0 + 0.3 * (1.0 - 0.5 * (layers[0] + layers[1]))
}

const GHOST_DELAY: usize = 12_800;

pub struct Deck {
    data: Arc<MusicData>,
    pub pos: f64,
    gains: [f32; 3],
    rate: f32,
    ghost: f32,
    tg: Targets,
    tick: u32,
    // ghost colouring
    lp: f32,
    lp2: f32,
    /// The ghost echo line, allocated once in `new` (never on the audio thread).
    dly: Vec<f32>,
    /// The echo line holds sound (and `dw` is live); it is zeroed when the ghost fades.
    dly_dirty: bool,
    dw: usize,
    dlp: f32,
    lfo: f32,
}

impl Deck {
    /// A deck at the loop start, already at the mood's settings (no ramp-in).
    pub fn new(data: Arc<MusicData>, mood: &Mood) -> Deck {
        let tg = targets(mood);
        let reactive = data.reactive;
        Deck {
            data,
            pos: 0.0,
            gains: if reactive { tg.layers } else { [0.0; 3] },
            rate: if reactive { tg.rate } else { 1.0 },
            ghost: if reactive { tg.ghost } else { 0.0 },
            tg,
            tick: 0,
            lp: 0.0,
            lp2: 0.0,
            dly: if reactive { vec![0.0; GHOST_DELAY] } else { Vec::new() },
            dly_dirty: false,
            dw: 0,
            dlp: 0.0,
            lfo: 0.0,
        }
    }

    pub fn rate(&self) -> f32 {
        self.rate
    }

    pub fn layer_gains(&self) -> [f32; 3] {
        self.gains
    }

    fn read(stem: &[f32], pos: f64, loop_len: usize) -> f32 {
        let len = stem.len();
        let p = if len == loop_len { pos } else { pos % len as f64 };
        let i = p as usize;
        let f = (p - i as f64) as f32;
        let a = stem[i];
        let b = stem[if i + 1 == len { 0 } else { i + 1 }];
        a + (b - a) * f
    }

    /// The next output sample.
    pub fn next(&mut self, mood: &Mood) -> f32 {
        let d = Arc::clone(&self.data);
        let len = d.len;
        let mut x;
        if !d.reactive {
            x = Self::read(&d.stems[0], self.pos, len);
            self.pos += 1.0;
        } else {
            if self.tick & 31 == 0 {
                self.tg = targets(mood);
            }
            self.tick = self.tick.wrapping_add(1);
            for i in 0..3 {
                // Layers come in quickly (a few hundred ms) and leave a little slower.
                let k = if self.tg.layers[i] > self.gains[i] { 0.00014 } else { 0.00008 };
                self.gains[i] += (self.tg.layers[i] - self.gains[i]) * k;
            }
            self.rate += (self.tg.rate - self.rate) * 0.00002;
            self.ghost += (self.tg.ghost - self.ghost) * 0.00004;
            x = Self::read(&d.stems[0], self.pos, len);
            for i in 0..3 {
                if self.gains[i] > 1e-4 {
                    if let Some(s) = d.stems.get(i + 1) {
                        x += self.gains[i] * Self::read(s, self.pos, len);
                    }
                }
            }
            x = crate::dsp::softclip(x * comp(&self.gains));
            self.pos += self.rate as f64;
            x = self.ghostly(x);
        }
        if self.pos >= len as f64 {
            self.pos -= len as f64;
        }
        x
    }

    /// Dark, wobbling and echoing, in proportion to `ghost`.
    fn ghostly(&mut self, x: f32) -> f32 {
        let g = self.ghost;
        if g < 0.002 {
            self.lp = x;
            self.lp2 = x;
            if self.dly_dirty {
                // Back to silence, as a fresh line would be, without freeing it.
                self.dly.fill(0.0);
                self.dly_dirty = false;
            }
            return x;
        }
        if !self.dly_dirty {
            self.dw = 0;
            self.dly_dirty = true;
        }
        // Cutoff glides from open to ~900 Hz.
        let fc = 14000.0 * (900.0f32 / 14000.0).powf(g.min(1.0));
        let a = 1.0 - (-TAU * fc / SR).exp();
        self.lp += a * (x - self.lp);
        self.lp2 += a * (self.lp - self.lp2);
        let dry = self.lp2;
        let rd = self.dly[self.dw];
        self.dlp += 0.25 * (rd - self.dlp);
        self.dly[self.dw] = dry + 0.5 * self.dlp;
        self.dw = (self.dw + 1) % GHOST_DELAY;
        self.lfo = (self.lfo + 0.3 / SR).fract();
        let trem = 1.0 - 0.2 * g * (0.5 + 0.5 * (TAU * self.lfo).sin());
        (dry + 0.45 * g * self.dlp) * trem * (1.0 - 0.15 * g)
    }
}
