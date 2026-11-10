//! One-shot sound effects and looped voices, synthesised from scratch. Every
//! function returns a mono buffer peak-normalised to 0.9; relative loudness is
//! set by [`crate::Sfx::base_gain`] in the mixer.

use crate::dsp::*;
use crate::{LoopId, Sfx};
use std::f32::consts::TAU;

/// Render one sound effect.
pub fn render_sfx(sfx: Sfx) -> Vec<f32> {
    let mut b = match sfx {
        Sfx::Jump => jump(),
        Sfx::Land => land(),
        Sfx::Boost => boost(),
        Sfx::SparkTier1 => spark(1),
        Sfx::SparkTier2 => spark(2),
        Sfx::SparkTier3 => spark(3),
        Sfx::LaserZap => laser_zap(),
        Sfx::TargetShatter => target_shatter(),
        Sfx::ItemTick => item_tick(),
        Sfx::ItemGet => item_get(),
        Sfx::EggThrow => egg_throw(),
        Sfx::EggHit => egg_hit(),
        Sfx::Cactus => cactus(),
        Sfx::ShieldUp => shield_up(),
        Sfx::ShieldBlock => shield_block(),
        Sfx::Lightning => lightning(),
        Sfx::MegaLaser => mega_laser(),
        Sfx::Bash => bash(),
        Sfx::Coin => coin(),
        Sfx::CountdownBeep => countdown_beep(),
        Sfx::CountdownGo => countdown_go(),
        Sfx::Lap => lap(),
        Sfx::FinalLap => final_lap(),
        Sfx::Finish => finish(),
        Sfx::MenuTap => menu_tap(),
        Sfx::MenuBack => menu_back(),
        Sfx::MenuSelect => menu_select(),
        Sfx::Laugh => laugh(),
        Sfx::Trump => trump(),
        Sfx::Flap => flap(),
        Sfx::Squawk => ostrich_squawk(),
        Sfx::Bump => bump(),
        Sfx::Crash => crash_thud(),
        Sfx::Mud => mud(),
        Sfx::GhostWhoosh => ghost_whoosh(),
        Sfx::PlaceThud => place_thud(),
        Sfx::PlaceDenied => place_denied(),
        Sfx::ProtectEnd => protect_end(),
    };
    declick(&mut b, 0.0004, 0.012);
    normalize(&mut b, 0.9);
    b
}

/// Render one looped voice. The result is exactly periodic or cross-faded, so
/// wrapping from the last sample to the first is click-free.
pub fn render_loop(id: LoopId) -> Vec<f32> {
    let mut b = match id {
        LoopId::Footsteps => loop_footsteps(),
        LoopId::Wind => loop_wind(),
        LoopId::Skid => loop_skid(),
        LoopId::LaserHum => loop_hum(),
    };
    normalize(&mut b, 0.85);
    b
}

// ---- building blocks -------------------------------------------------------

fn laugh() -> Vec<f32> {
    let mut out = vec![0.0; n(1.52)];
    // Three breathy "ha ha" voices: low frog, jockey, then a higher ostrich.
    for (voice, pitch) in [145.0f32, 245.0, 370.0].into_iter().enumerate() {
        let mut rng = Rng::new(87 + voice as u64);
        let mut phase = 0.0f32;
        let mut mouth = [Svf::new(720.0, 5.0), Svf::new(1180.0, 7.0), Svf::new(2550.0, 6.0)];
        for (i, sample) in out.iter_mut().enumerate() {
            let t = i as f32 / SR - voice as f32 * 0.055;
            if t < 0.0 { continue; }
            let beat = (t / 0.22).floor();
            let x = t - beat * 0.22;
            let env = if beat < 6.0 && x < 0.17 {
                (x / 0.17 * TAU * 0.5).sin().max(0.0).powf(1.2) * (1.0 - beat * 0.08)
            } else { 0.0 };
            let hz = pitch * (1.1 - x * 1.5 + 0.035 * (t * 35.0).fs());
            phase = (phase + hz / SR).fract();
            let breath = rng.signed() * (0.10 + 0.28 * (1.0 - x / 0.04).max(0.0));
            let source = (pulse(phase, hz / SR, 0.22) * 0.6 + breath) * env;
            let a = mouth[0].tick(source).1;
            let b = mouth[1].tick(source).1;
            let c = mouth[2].tick(source).1;
            *sample += (a * 0.65 + b * 0.32 + c * 0.12) * 0.5;
        }
    }
    out
}

/// A bright bell partial stack starting at `t0`, `f` Hz, decay rate `decay` (1/s).
fn bell(t: f32, t0: f32, f: f32, decay: f32) -> f32 {
    let t = t - t0;
    if t < 0.0 {
        return 0.0;
    }
    let w = TAU * f * t;
    let body = w.fs() + 0.35 * (2.0 * w).fs() * (-t * decay * 1.5).fe() + 0.12 * (3.01 * w).fs() * (-t * decay * 2.5).fe();
    body * (-t * decay).fe() * (1.0 - (-t * 1200.0).fe())
}

/// A phase accumulator (cycles) for frequency sweeps.
struct Ph(f32);
impl Ph {
    fn adv(&mut self, f: f32) -> f32 {
        self.0 = (self.0 + f / SR).fract();
        self.0
    }
}

/// A short noise tick/crack: white noise, exp decay.
fn noise_burst(rng: &mut Rng, t: f32, t0: f32, decay: f32) -> f32 {
    let t = t - t0;
    if t < 0.0 {
        0.0
    } else {
        rng.signed() * (-t * decay).fe()
    }
}

/// Band-limited saw-ish brass: two detuned saws through a swelling low-pass.
fn brass(freq: f32, gate: f32, release: f32, vibrato: f32) -> Vec<f32> {
    let len = n(gate + release);
    let (mut p1, mut p2) = (0.0f32, 0.3f32);
    let mut lp = 0.0f32;
    let (mut f, mut a, mut env) = (freq, 0.5f32, 0.0f32);
    (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            if i % 16 == 0 {
                // Control-rate updates: vibrato, filter opening ("blat") and envelope.
                f = freq * (1.0 + vibrato * fsin(5.5 * t) * (t * 6.0).min(1.0));
                let cut = 700.0 + 4500.0 * (1.0 - (-t * 28.0).fe());
                a = 1.0 - (-TAU * cut / SR).fe();
                env = adsr(t, gate, 0.012, 0.09, 0.78, release);
            }
            p1 += f / SR;
            p1 -= (p1 >= 1.0) as i32 as f32;
            p2 += f * 1.006 / SR;
            p2 -= (p2 >= 1.0) as i32 as f32;
            let x = 0.55 * saw(p1, f / SR) + 0.45 * saw(p2, f * 1.006 / SR);
            lp += a * (x - lp);
            lp * env
        })
        .collect()
}

fn mix_at(dst: &mut Vec<f32>, start: f32, src: &[f32], gain: f32) {
    let s = n(start).saturating_sub(1);
    if dst.len() < s + src.len() {
        dst.resize(s + src.len(), 0.0);
    }
    for (i, x) in src.iter().enumerate() {
        dst[s + i] += x * gain;
    }
}

// ---- UI --------------------------------------------------------------------

fn menu_tap() -> Vec<f32> {
    let mut ph = Ph(0.0);
    let mut rng = Rng::new(901);
    gen(0.11, |t, _| {
        let f = 640.0 + 520.0 * (1.0 - (-t * 70.0).fe());
        let p = ph.adv(f);
        let body = (TAU * p).fs() + 0.3 * (TAU * 2.0 * p).fs();
        body * (-t * 40.0).fe() * (1.0 - (-t * 1500.0).fe()) + 0.25 * noise_burst(&mut rng, t, 0.0, 600.0)
    })
}

fn menu_back() -> Vec<f32> {
    let mut ph = Ph(0.0);
    gen(0.15, |t, _| {
        let f = 700.0 - 330.0 * (1.0 - (-t * 30.0).fe());
        let p = ph.adv(f);
        ((TAU * p).fs() + 0.25 * (TAU * 2.0 * p).fs()) * (-t * 26.0).fe() * (1.0 - (-t * 1500.0).fe())
    })
}

fn menu_select() -> Vec<f32> {
    let mut rng = Rng::new(902);
    gen(0.4, |t, _| {
        let a = bell(t, 0.0, 659.3, 16.0);
        let b = bell(t, 0.085, 987.8, 11.0);
        let c = bell(t, 0.085, 1975.5, 22.0) * 0.3;
        a * 0.8 + b + c + 0.15 * noise_burst(&mut rng, t, 0.085, 700.0)
    })
}

// ---- countdown, laps, finish -----------------------------------------------

fn countdown_beep() -> Vec<f32> {
    gen(0.3, |t, _| {
        let w = TAU * 440.0 * t;
        // A soft square: odd harmonics.
        let x = w.fs() + (3.0 * w).fs() / 3.0 + (5.0 * w).fs() / 5.0 + 0.5 * (0.5 * w).fs();
        x * adsr(t, 0.2, 0.004, 0.2, 0.9, 0.08)
    })
}

fn countdown_go() -> Vec<f32> {
    let mut rng = Rng::new(903);
    let notes = [880.0f32, 1108.7, 1318.5];
    gen(0.95, |t, _| {
        let zip = 1.0 - 0.06 * (-t * 60.0).fe(); // a quick upward scoop
        let mut x = 0.0;
        for (k, &f) in notes.iter().enumerate() {
            let f = f * zip * (1.0 + 0.004 * fsin(6.0 * t));
            let w = TAU * f * t;
            x += w.fs() + 0.5 * (2.0 * w).fs() + 0.33 * (3.0 * w).fs() + 0.2 * (4.0 * w).fs() * (1.0 - 0.1 * k as f32);
        }
        let sparkle = 0.5 * noise_burst(&mut rng, t, 0.0, 45.0);
        (x + sparkle) * adsr(t, 0.55, 0.004, 0.15, 0.62, 0.4)
    })
}

fn lap() -> Vec<f32> {
    gen(0.8, |t, _| bell(t, 0.0, 784.0, 7.0) * 0.8 + bell(t, 0.13, 1046.5, 6.0))
}

fn final_lap() -> Vec<f32> {
    // "da-da-da-DAAA": a rising brass motif with a bell on top.
    let mut out = Vec::new();
    for (t0, f, gate) in [(0.0f32, 523.25f32, 0.11f32), (0.13, 659.25, 0.11), (0.26, 783.99, 0.11)] {
        mix_at(&mut out, t0, &brass(f, gate, 0.05, 0.0), 0.7);
    }
    for f in [1046.5f32, 1318.5, 1568.0] {
        mix_at(&mut out, 0.39, &brass(f, 0.5, 0.25, 0.012), 0.5);
    }
    let bells = gen(1.2, |t, _| bell(t, 0.39, 2093.0, 5.0) * 0.25);
    mix_at(&mut out, 0.0, &bells, 1.0);
    out
}

fn finish() -> Vec<f32> {
    let mut out = Vec::new();
    for (t0, f) in [(0.0f32, 523.25f32), (0.15, 659.25), (0.3, 783.99)] {
        mix_at(&mut out, t0, &brass(f, 0.12, 0.05, 0.0), 0.7);
        mix_at(&mut out, t0, &brass(f * 2.0, 0.12, 0.05, 0.0), 0.3);
    }
    // The held chord: C major with an added sixth, vibrato swelling in.
    for (f, g) in [(1046.5f32, 0.55f32), (1318.5, 0.5), (1568.0, 0.5), (2093.0, 0.3), (523.25, 0.5)] {
        mix_at(&mut out, 0.46, &brass(f, 1.1, 0.5, 0.01), g);
    }
    // Timpani + cymbal crash under the chord.
    let mut rng = Rng::new(904);
    let mut ph = Ph(0.0);
    let timp = gen(0.9, |t, _| {
        let f = 62.0 + 70.0 * (-t * 14.0).fe();
        (TAU * ph.adv(f)).fs() * (-t * 5.0).fe()
    });
    mix_at(&mut out, 0.46, &timp, 0.9);
    let mut crash = gen(1.9, |t, _| rng.signed() * (-t * 2.6).fe() * (1.0 - (-t * 400.0).fe()));
    highpass(&mut crash, 5200.0);
    mix_at(&mut out, 0.46, &crash, 0.55);
    out
}

// ---- movement ---------------------------------------------------------------

fn jump() -> Vec<f32> {
    // A soft upward whoosh: breathy noise whose band sweeps up as it fades, with a
    // faint pitched lift underneath.
    let mut rng = Rng::new(910);
    let mut bp = Svf::new(500.0, 1.6);
    let mut ph = Ph(0.0);
    let mut out = gen(0.26, |t, _| {
        bp.set(450.0 + 2200.0 * (1.0 - (-t * 11.0).fe()), 1.8);
        let (_, band, _) = bp.tick(rng.signed());
        let env = (1.0 - (-t * 45.0).fe()) * (-t * 9.5).fe();
        let tone = (TAU * ph.adv(230.0 + 380.0 * (1.0 - (-t * 14.0).fe()))).fs() * (-t * 13.0).fe() * (1.0 - (-t * 500.0).fe());
        band * env * 1.5 + tone * 0.28
    });
    // Take the fizz off the band-pass skirts: soft, not hissy.
    lowpass(&mut out, 3600.0);
    lowpass(&mut out, 4800.0);
    out
}

fn land() -> Vec<f32> {
    let mut ph = Ph(0.0);
    let mut rng = Rng::new(911);
    let mut dust: Vec<f32> = gen(0.3, |t, _| rng.signed() * (-t * 20.0).fe() * (1.0 - (-t * 600.0).fe()));
    lowpass(&mut dust, 1100.0);
    let mut i = 0;
    let mut out = gen(0.3, |t, _| {
        let f = 62.0 + 200.0 * (-t * 38.0).fe();
        let thump = (TAU * ph.adv(f)).fs() * (-t * 15.0).fe() * (1.0 - (-t * 1000.0).fe());
        let d = dust[i];
        i += 1;
        softclip(thump * 1.8) * 0.8 + d * 1.6
    });
    mid_boost(&mut out, 1.8);
    out
}

fn boost() -> Vec<f32> {
    let mut rng = Rng::new(912);
    let mut bp = Svf::new(500.0, 1.4);
    let mut ph = Ph(0.0);
    let mut sub = Ph(0.0);
    gen(0.95, |t, _| {
        let sweep = 450.0 + 3600.0 * (1.0 - (-t * 6.0).fe());
        bp.set(sweep, 1.3);
        let (_, band, _) = bp.tick(rng.signed());
        let env = (1.0 - (-t * 16.0).fe()) * (-(t - 0.05).max(0.0) * 3.2).fe();
        let f = 130.0 + 800.0 * (1.0 - (-t * 5.0).fe());
        let p = ph.adv(f);
        let saw_ish = saw(p, f / SR) * 0.22 * (-t * 3.6).fe();
        let fwoom = (TAU * sub.adv(70.0 + 25.0 * (-t * 8.0).fe())).fs() * (-t * 6.0).fe() * 0.5;
        band * env * 1.6 + saw_ish + fwoom
    })
}

fn spark(tier: u32) -> Vec<f32> {
    let freqs = [1046.5f32, 1318.5, 1568.0];
    let mut rng = Rng::new(920 + tier as u64);
    let dur = 0.22 + 0.07 * tier as f32;
    gen(dur, |t, _| {
        let mut x = 0.0;
        for (k, &f) in freqs.iter().enumerate().take(tier as usize) {
            let t0 = k as f32 * 0.045;
            x += bell(t, t0, f, 20.0 - 2.0 * tier as f32);
            x += 0.35 * bell(t, t0, f * 2.0, 28.0);
        }
        if tier == 3 {
            // The top tier crackles.
            x += 0.5 * noise_burst(&mut rng, t, 0.09, 90.0) * (TAU * 5200.0 * t).fs();
        }
        x
    })
}

// ---- weapons and hazards ------------------------------------------------------

fn laser_zap() -> Vec<f32> {
    let mut rng = Rng::new(930);
    let mut ph = Ph(0.0);
    let mut gate = 1.0;
    let mut crackle = vec![0.0f32; n(0.42)];
    for (i, c) in crackle.iter_mut().enumerate() {
        if i % 150 == 0 {
            gate = if rng.f32() < 0.55 { 1.0 } else { 0.0 };
        }
        *c = rng.signed() * gate;
    }
    highpass(&mut crackle, 1800.0);
    gen(0.42, |t, i| {
        let f = 2700.0 * (-t * 14.0).fe() + 150.0;
        let p = ph.adv(f);
        let s = saw(p, f / SR) * 0.7 + (TAU * p).fs() * 0.5;
        let buzz = 0.55 + 0.45 * fsin(70.0 * t);
        s * buzz * (-t * 7.0).fe() * (1.0 - (-t * 1200.0).fe()) + crackle[i] * 0.3 * (-t * 9.0).fe()
    })
}

fn target_shatter() -> Vec<f32> {
    let mut rng = Rng::new(931);
    let pings: Vec<(f32, f32, f32, f32)> = (0..22).map(|_| (rng.range(0.0, 0.14), rng.range(2200.0, 9500.0), rng.range(25.0, 75.0), rng.range(0.25, 0.8))).collect();
    let mut crack = gen(0.75, |t, _| rng.signed() * (-t * 16.0).fe());
    highpass(&mut crack, 2400.0);
    gen(0.75, |t, i| {
        let mut x = crack[i] * 0.8;
        for &(t0, f, d, g) in &pings {
            if t >= t0 && (t - t0) * d < 9.0 {
                x += (TAU * f * t).fs() * (-(t - t0) * d).fe() * g * 0.35;
            }
        }
        x + (TAU * 190.0 * t).fs() * (-t * 55.0).fe() * 0.8
    })
}

fn item_tick() -> Vec<f32> {
    gen(0.06, |t, _| (TAU * 1850.0 * t).fs() * (-t * 90.0).fe() * (1.0 - (-t * 2000.0).fe()))
}

fn item_get() -> Vec<f32> {
    let mut rng = Rng::new(932);
    let notes = [1046.5f32, 1318.5, 1568.0, 2093.0];
    let mut sh = gen(0.9, |t, _| rng.signed() * (-(t - 0.2).max(0.0) * 7.0).fe() * (1.0 - (-t * 30.0).fe()));
    highpass(&mut sh, 6500.0);
    gen(0.9, |t, i| {
        let mut x = 0.0;
        for (k, &f) in notes.iter().enumerate() {
            let last = k == notes.len() - 1;
            x += bell(t, 0.065 * k as f32, f, if last { 4.5 } else { 11.0 });
        }
        x * 0.8 + sh[i] * 0.2
    })
}

fn egg_throw() -> Vec<f32> {
    let mut ph = Ph(0.0);
    let mut rng = Rng::new(933);
    let mut bp = Svf::new(1500.0, 1.2);
    gen(0.34, |t, _| {
        let f = 300.0 + 420.0 * (std::f32::consts::PI * (t / 0.3).min(1.0)).fs();
        let env = (std::f32::consts::PI * (t / 0.34).min(1.0)).fs().powf(0.7);
        let (_, b, _) = bp.tick(rng.signed());
        ((TAU * ph.adv(f)).fs() + 0.3 * (TAU * 2.0 * ph.0).fs()) * env * 0.8 + b * env * 0.35
    })
}

fn egg_hit() -> Vec<f32> {
    let mut rng = Rng::new(934);
    let mut bp = Svf::new(2000.0, 0.9);
    let mut ph = Ph(0.0);
    gen(0.5, |t, _| {
        bp.set(2200.0 * (-t * 9.0).fe() + 280.0, 1.0);
        let (lo, b, _) = bp.tick(rng.signed());
        let splat = (b * 1.2 + lo * 0.4) * (-t * 11.0).fe();
        let squelch = (TAU * ph.adv(240.0 * (-t * 6.5).fe() + 60.0)).fs() * (0.6 + 0.4 * fsin(38.0 * t)) * (-t * 9.0).fe();
        let crack = noise_burst(&mut rng, t, 0.0, 280.0) * 0.8;
        splat + squelch * 0.7 + crack
    })
}

fn cactus() -> Vec<f32> {
    let mut rng = Rng::new(935);
    let mut ph = Ph(0.0);
    let mut sp = Ph(0.0);
    gen(0.6, |t, _| {
        let thud = (TAU * sp.adv(52.0 + 110.0 * (-t * 18.0).fe())).fs() * (-t * 15.0).fe();
        let prick = (TAU * 3100.0 * t).fs() * (-t * 90.0).fe() * 0.6 + noise_burst(&mut rng, t, 0.0, 400.0) * 0.4;
        // "boyoyoing": a wobbling spring that droops.
        let f = 420.0 - 160.0 * t + 90.0 * fsin(13.0 * t) * (-t * 3.0).fe();
        let boing = ((TAU * ph.adv(f)).fs() + 0.5 * (TAU * 2.0 * ph.0).fs()) * (-t * 5.5).fe() * (1.0 - (-t * 400.0).fe());
        thud * 0.9 + prick + boing * 0.55
    })
}

fn shield_up() -> Vec<f32> {
    let mut rng = Rng::new(936);
    let base = [523.25f32, 783.99, 1046.5, 1318.5];
    let mut sh = gen(0.8, |t, _| rng.signed() * (1.0 - (-t * 14.0).fe()) * (-t * 4.0).fe());
    highpass(&mut sh, 6000.0);
    let mut phs = [0.0f32; 4];
    gen(0.8, |t, i| {
        let glide = 0.62 + 0.38 * (1.0 - (-t * 10.0).fe());
        let mut x = 0.0;
        for k in 0..4 {
            phs[k] = (phs[k] + base[k] * glide / SR).fract();
            x += (TAU * phs[k]).fs() * (1.0 - 0.12 * k as f32);
        }
        let trem = 0.75 + 0.25 * fsin(17.0 * t);
        x * trem * (1.0 - (-t * 25.0).fe()) * (-t * 3.6).fe() * 0.5 + sh[i] * (0.3 + 0.3 * fsin(31.0 * t)).max(0.0) * 0.5 + bell(t, 0.16, 2093.0, 8.0) * 0.25
    })
}

fn shield_block() -> Vec<f32> {
    let mut rng = Rng::new(937);
    let ratios = [(1.0f32, 1.0f32, 5.0f32), (2.76, 0.6, 8.0), (5.4, 0.35, 12.0), (8.93, 0.2, 18.0)];
    gen(0.7, |t, _| {
        let mut x = 0.0;
        for &(r, a, d) in &ratios {
            x += (TAU * 520.0 * r * t).fs() * a * (-t * d).fe();
        }
        let thump = (TAU * (60.0 + 70.0 * (-t * 25.0).fe()) * t).fs() * (-t * 20.0).fe();
        x * (1.0 - (-t * 2500.0).fe()) + thump * 0.7 + noise_burst(&mut rng, t, 0.0, 500.0) * 0.5
    })
}

fn lightning() -> Vec<f32> {
    let mut rng = Rng::new(938);
    let mut crack = gen(1.2, |t, _| rng.signed() * (-t * 22.0).fe() * (1.0 - (-t * 3000.0).fe()));
    highpass(&mut crack, 1400.0);
    let mut rumble = gen(1.2, |t, _| rng.signed() * (1.0 - (-t * 45.0).fe()) * (-t * 3.0).fe());
    lowpass(&mut rumble, 260.0);
    let mut ph = Ph(0.0);
    let mut gate = 1.0f32;
    gen(1.2, |t, i| {
        if i % 600 == 0 {
            gate = if rng.f32() < 0.7 { 1.0 } else { 0.15 };
        }
        let f = 3800.0 * (-t * 10.0).fe() + 90.0;
        let zig = saw(ph.adv(f), f / SR) * (-t * 9.0).fe() * gate;
        crack[i] * 0.9 + zig * 0.35 + rumble[i] * 5.0 + (TAU * 55.0 * t).fs() * (-t * 6.0).fe() * 0.5
    })
}

fn mega_laser() -> Vec<f32> {
    let mut rng = Rng::new(939);
    let mut ph = Ph(0.0);
    let mut pew = Ph(0.0);
    let mut sub = Ph(0.0);
    let mut hiss = Svf::new(3000.0, 1.0);
    gen(1.7, |t, _| {
        let f = 62.0 + 52.0 * (1.0 - (-t * 7.0).fe()) + 3.0 * fsin(7.0 * t);
        let growl = 0.6 + 0.4 * fsin(33.0 * t);
        let p = ph.adv(f);
        let mut s = 0.0;
        for k in 1..=6 {
            s += (TAU * p * k as f32).fs() / k as f32 * (if k % 2 == 0 { 0.8 } else { 1.0 });
        }
        let (_, band, _) = hiss.tick(rng.signed());
        let pew_s = (TAU * pew.adv(4200.0 * (-t * 22.0).fe() + 220.0)).fs() * (-t * 12.0).fe();
        let body = (s * growl * 0.8 + band * 0.4 * growl + (TAU * sub.adv(48.0)).fs() * 0.6) * adsr(t, 1.25, 0.06, 0.2, 0.85, 0.45);
        softclip(body * 1.4) + pew_s * 0.7
    })
}

/// Laser Trump: a rude, fluttering raspberry that slows and sags, then a laser
/// zap climbing out of it.
fn trump() -> Vec<f32> {
    const RASP: f32 = 0.62;
    const ZAP: f32 = 0.50;
    let mut rng = Rng::new(940);
    let mut body = Ph(0.0);
    let mut lips = Ph(0.0);
    let mut zap = Ph(0.0);
    let mut sub = Ph(0.0);
    let mut mouth = [Svf::new(520.0, 3.0), Svf::new(1350.0, 4.0)];
    let mut hiss = Svf::new(2600.0, 1.2);
    gen(1.2, |t, _| {
        // The raspberry: buzzing lips whose flutter slows as the air runs out.
        let u = (t / RASP).min(1.0);
        let flutter = lips.adv(41.0 - 20.0 * u + 3.0 * rng.signed());
        let gate = ((TAU * flutter).fs() * 1.6 + 0.35).clamp(0.0, 1.0);
        let f = 128.0 - 48.0 * u * u + 14.0 * (-t * 30.0).fe() + 6.0 * rng.signed();
        let p = body.adv(f);
        let src = saw(p, f / SR) * 0.8 + pulse(p, f / SR, 0.3) * 0.4 + rng.signed() * 0.25;
        let lo = mouth[0].tick(src).1;
        let hi = mouth[1].tick(src).1;
        let air = (TAU * sub.adv(62.0 - 20.0 * u)).fs() * 0.4;
        let rasp_env = (1.0 - (-t * 160.0).fe()) * (1.0 - smooth_step(RASP - 0.12, RASP, t));
        let rasp = (lo * 0.9 + hi * 0.5 + air) * gate * rasp_env;
        // The zap: a saw sweeping up with a buzz, a high sine on top and crackle.
        let z = ((t - ZAP) / (1.2 - ZAP)).clamp(0.0, 1.0);
        let zf = 260.0 + 4200.0 * z * z;
        let zp = zap.adv(zf);
        let zenv = if t < ZAP {
            0.0
        } else {
            (1.0 - (-(t - ZAP) * 90.0).fe()) * (1.0 - z).powf(0.6)
        };
        let buzz = 0.6 + 0.4 * fsin(55.0 * t);
        let (_, crackle, _) = hiss.tick(rng.signed());
        let zap_s =
            (saw(zp, zf / SR) * 0.55 + (TAU * zp).fs() * 0.5) * buzz * zenv + crackle * 0.25 * zenv;
        softclip(rasp * 1.5) * 0.8 + zap_s
    })
}

/// 0 below `a`, 1 above `b`, smooth between.
fn smooth_step(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

// ---- the ostrich ---------------------------------------------------------------

/// An ostrich-ish honk: a gliding saw through two formant filters.
fn squawk(secs: f32, f_start: f32, f_peak: f32, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    let mut ph = 0.0f32;
    let mut a = Svf::new(780.0, 4.0);
    let mut b = Svf::new(1500.0, 5.0);
    gen(secs, |t, _| {
        let glide = f_start + (f_peak - f_start) * (1.0 - (-t * 26.0).fe()) - 0.3 * (f_peak - f_start) * (t / secs);
        let f = glide * (1.0 + 0.05 * fsin(11.0 * t) * (t * 9.0).min(1.0));
        ph = (ph + f / SR).fract();
        let src = saw(ph, f / SR) + 0.12 * rng.signed();
        // Formants slide up a little as the beak opens.
        let open = 1.0 + 0.18 * (1.0 - (-t * 18.0).fe());
        a.set(780.0 * open, 4.0);
        b.set(1500.0 * open, 5.0);
        let (_, ba, _) = a.tick(src);
        let (_, bb, _) = b.tick(src);
        (ba * 0.9 + bb * 0.7 + src * 0.12) * adsr(t, secs * 0.62, 0.008, 0.2, 0.7, secs * 0.38)
    })
}

fn bash() -> Vec<f32> {
    let mut rng = Rng::new(940);
    let mut ph = Ph(0.0);
    let mut lp = gen(0.3, |t, _| rng.signed() * (-t * 42.0).fe());
    lowpass(&mut lp, 1900.0);
    let thump = gen(0.35, |t, i| {
        let f = 44.0 + 120.0 * (-t * 20.0).fe();
        softclip((TAU * ph.adv(f)).fs() * 2.0) * (-t * 11.0).fe() * (1.0 - (-t * 1500.0).fe()) + lp[i.min(lp.len() - 1)] * 0.9
    });
    let mut out = Vec::new();
    mix_at(&mut out, 0.0, &thump, 1.0);
    mix_at(&mut out, 0.045, &squawk(0.42, 330.0, 640.0, 941), 0.42);
    out
}

// ---- coins -----------------------------------------------------------------------

fn coin() -> Vec<f32> {
    let mut rng = Rng::new(950);
    gen(0.6, |t, _| {
        // "ba-DING": B5 then E6, with an FM shimmer on the second.
        let a = bell(t, 0.0, 987.8, 22.0) * 0.8;
        let fm = (TAU * 1318.5 * (t - 0.065) + 1.3 * (TAU * 2637.0 * (t - 0.065)).fs() * (-(t - 0.065) * 18.0).fe()).fs();
        let b = if t >= 0.065 { fm * (-(t - 0.065) * 6.5).fe() * (1.0 - (-(t - 0.065) * 1500.0).fe()) } else { 0.0 };
        let c = bell(t, 0.065, 2637.0, 11.0) * 0.35;
        a + b + c + noise_burst(&mut rng, t, 0.0, 700.0) * 0.12
    })
}

/// Pitch ratio for the `chain`-th coin picked up in quick succession (0 = first):
/// a major-pentatonic climb that tops out an octave and a half up.
pub fn coin_pitch(chain: u32) -> f32 {
    const STEPS: [f32; 8] = [0.0, 2.0, 4.0, 7.0, 9.0, 12.0, 14.0, 16.0];
    2f32.powf(STEPS[(chain as usize).min(STEPS.len() - 1)] / 12.0)
}

// ---- loops -------------------------------------------------------------------------

fn loop_footsteps() -> Vec<f32> {
    // Four footfalls in 0.8 s (5 steps/s at pitch 1.0): the light patter of big bird
    // feet. Each is a soft pad slap, a tiny "tok" and a claw tick a few milliseconds
    // later. Rendered wrapped so the last tail rolls into the first step.
    let len = n(0.8);
    let mut out = vec![0.0f32; len];
    let mut rng = Rng::new(960);
    for k in 0..4 {
        let heavy = if k % 2 == 0 { 1.0 } else { 0.8 };
        let f0 = if k % 2 == 0 { 330.0 } else { 390.0 };
        let mut ph = Ph(0.0);
        let mut pad: Vec<f32> = gen(0.09, |t, _| rng.signed() * (-t * 75.0).fe() * (1.0 - (-t * 900.0).fe()));
        highpass(&mut pad, 650.0);
        lowpass(&mut pad, 3200.0);
        let mut claw: Vec<f32> = gen(0.04, |t, _| rng.signed() * (-t * 190.0).fe());
        highpass(&mut claw, 2800.0);
        lowpass(&mut claw, 6500.0);
        let step = gen(0.12, |t, i| {
            let tok = (TAU * ph.adv(f0 * (0.7 + 0.5 * (-t * 70.0).fe()))).fs() * (-t * 55.0).fe() * (1.0 - (-t * 1200.0).fe());
            let body = (TAU * 150.0 * t).fs() * (-t * 38.0).fe() * 0.35;
            let p = pad.get(i).copied().unwrap_or(0.0);
            // Second toe touches down about 11 ms later.
            let c = claw.get(i.wrapping_sub(n(0.011))).copied().unwrap_or(0.0);
            (tok * 0.8 + body + p * 2.0 + c * 0.35) * heavy
        });
        add_wrapped(&mut out, k * len / 4, &step, 1.0);
    }
    mid_boost(&mut out, 0.8);
    out
}

/// Add `gain` times the 220..3000 Hz band back onto the buffer (a mid shelf).
fn mid_boost(buf: &mut [f32], gain: f32) {
    let mut mid = buf.to_vec();
    lowpass(&mut mid, 3000.0);
    highpass(&mut mid, 220.0);
    for (b, m) in buf.iter_mut().zip(mid) {
        *b += m * gain;
    }
}

fn loop_wind() -> Vec<f32> {
    let len = n(2.4);
    let x = n(0.4);
    let mut rng = Rng::new(961);
    let mut raw: Vec<f32> = (0..len + x).map(|_| rng.signed()).collect();
    // Warm rush: low-passed noise with some body, then a hollow mid band.
    let mut body = raw.clone();
    lowpass(&mut body, 600.0);
    lowpass(&mut body, 1500.0);
    highpass(&mut body, 120.0);
    let mut hiss = raw.clone();
    highpass(&mut hiss, 2600.0);
    lowpass(&mut hiss, 5500.0);
    lowpass(&mut hiss, 5500.0);
    for (i, r) in raw.iter_mut().enumerate() {
        *r = body[i] * 4.0 + hiss[i] * 0.22;
    }
    let mut out = make_loop(&raw, len);
    // Gusts: whole numbers of cycles over the loop, so they wrap exactly.
    for (i, o) in out.iter_mut().enumerate() {
        let c = i as f32 / len as f32;
        *o *= 0.72 + 0.18 * fsin(c * 2.0) + 0.1 * fsin(c * 5.0 + 0.3);
    }
    out
}

fn loop_skid() -> Vec<f32> {
    let len = n(1.0);
    let x = n(0.25);
    let mut rng = Rng::new(962);
    let mut bp = Svf::new(1900.0, 1.8);
    let mut low = Svf::new(380.0, 1.0);
    let raw: Vec<f32> = (0..len + x)
        .map(|i| {
            let w = rng.signed();
            let (_, b, _) = bp.tick(w);
            let (l, _, _) = low.tick(w);
            // Gravelly crackle: sparse impulses on top of the scrape.
            let grit = if rng.f32() < 0.01 { rng.signed() * 1.1 } else { 0.0 };
            let _ = i;
            b * 0.7 + l * 1.4 + grit
        })
        .collect();
    let mut out = make_loop(&raw, len);
    lowpass(&mut out, 5000.0);
    // A faint sliding-dust squeal: integer Hz, so it wraps exactly.
    for (i, o) in out.iter_mut().enumerate() {
        let t = i as f32 / SR;
        *o += 0.12 * (TAU * (1400.0 * t + 6.0 * fsin(3.0 * t))).fs() * (0.6 + 0.4 * fsin(5.0 * t));
    }
    out
}

fn loop_hum() -> Vec<f32> {
    // 1 s of integer-Hz partials: exactly periodic. Two slightly detuned stacks beat
    // at 1 Hz; an 8 Hz tremolo and a 55 Hz ring-mod give the electrical buzz.
    // Harmonics come from the Chebyshev recurrence sin((k+1)x) = 2cos(x) sin(kx) - sin((k-1)x).
    fn stack(x: f32) -> f32 {
        let c2 = 2.0 * (x + std::f32::consts::FRAC_PI_2).fs();
        let (mut prev, mut cur) = (0.0f32, x.fs());
        let mut sum = cur;
        for k in 2..=26 {
            let next = c2 * cur - prev;
            prev = cur;
            cur = next;
            sum += cur / (k as f32).sqrt() * 0.7;
        }
        sum
    }
    let len = n(1.0);
    (0..len)
        .map(|i| {
            let t = i as f32 / len as f32; // 0..1 == seconds
            // The detuned stack's per-harmonic phase offset (0.37 rad) is dropped: it
            // only reshapes the waveform, not the sound.
            let s = stack(TAU * 110.0 * t) + 0.7 * stack(TAU * 111.0 * t + 0.37);
            let trem = 0.78 + 0.22 * (TAU * 8.0 * t).fs();
            let sq = (TAU * 55.0 * t).fs();
            let buzz = 0.65 + 0.17 * sq.signum() + 0.35 * sq;
            s * 0.35 * trem * buzz + 0.35 * (TAU * 880.0 * t + 1.5 * (TAU * 4.0 * t).fs()).fs()
        })
        .collect()
}

// ---- Cluster Flock ------------------------------------------------------------

/// Quick soft wing flutter: five airy wingbeats, each a breath of band-passed noise
/// whose band lifts on the downstroke, with a faint low "fwup" under it.
fn flap() -> Vec<f32> {
    let mut rng = Rng::new(970);
    let mut air = Svf::new(1200.0, 0.9);
    let mut beat = Ph(0.5); // starts at a trough: the first beat swells in
    let mut body = Ph(0.0);
    let mut out = gen(0.25, |t, _| {
        let b = beat.adv(21.0 - 5.0 * t / 0.25);
        let gate = (0.5 - 0.5 * fsin(b + 0.25)).max(0.0); // 0..1 per wingbeat
        let g = gate * gate;
        air.set(700.0 + 1300.0 * gate, 1.1);
        let (_, band, _) = air.tick(rng.signed());
        let env = (1.0 - (-t * 160.0).fe()) * (-t * 6.0).fe();
        let fwup = (TAU * body.adv(210.0)).fs() * g * 0.22;
        (band * 1.3 * g + fwup) * env
    });
    // Airy, not harsh: roll the band-pass skirts off and drop the rumble.
    lowpass(&mut out, 2800.0);
    lowpass(&mut out, 3600.0);
    highpass(&mut out, 220.0);
    out
}

/// A comic ostrich honk: a sawtooth that scoops up then sags, through nasal formants,
/// with a rattly shudder. Pitched low enough to stay a honk at 1.6x.
fn ostrich_squawk() -> Vec<f32> {
    let mut rng = Rng::new(971);
    let mut ph = 0.0f32;
    let mut a = Svf::new(720.0, 4.0);
    let mut b = Svf::new(1400.0, 5.0);
    let mut c = Svf::new(2500.0, 6.0);
    let secs = 0.3;
    gen(secs, |t, _| {
        let u = t / secs;
        let f = 245.0 * (1.0 + 0.6 * (1.0 - (-t * 32.0).fe()) - 0.4 * u * u) * (1.0 + 0.035 * fsin(26.0 * t) * u);
        ph = (ph + f / SR).fract();
        let src = saw(ph, f / SR) * 0.8 + pulse(ph, f / SR, 0.3) * 0.35 + 0.08 * rng.signed();
        let open = 1.0 + 0.2 * (1.0 - (-t * 20.0).fe());
        a.set(720.0 * open, 4.0);
        b.set(1400.0 * open, 5.0);
        let (_, fa, _) = a.tick(src);
        let (_, fb, _) = b.tick(src);
        let (_, fc, _) = c.tick(src);
        let rattle = 1.0 - 0.22 * (0.5 + 0.5 * fsin(44.0 * t)) * u;
        (fa * 0.9 + fb * 0.75 + fc * 0.2) * rattle * adsr(t, secs * 0.7, 0.008, 0.2, 0.78, secs * 0.3)
    })
}

/// Bump/boing: a rubbery tone that leaps up and sags, wobbling as it fades.
fn bump() -> Vec<f32> {
    let mut ph = Ph(0.0);
    let mut sub = Ph(0.0);
    let mut rng = Rng::new(972);
    gen(0.26, |t, _| {
        let f = 175.0 + 270.0 * (1.0 - (-t * 42.0).fe()) * (-t * 6.5).fe() + 30.0 * fsin(23.0 * t) * (-t * 8.0).fe();
        let p = ph.adv(f);
        let tone = (TAU * p).fs() + 0.5 * (TAU * 2.0 * p).fs() + 0.18 * (TAU * 3.0 * p).fs();
        let env = (1.0 - (-t * 400.0).fe()) * (-t * 9.0).fe();
        let thump = (TAU * sub.adv(110.0 * (0.7 + 0.6 * (-t * 40.0).fe()))).fs() * (-t * 42.0).fe() * 0.55;
        tone * env + thump + 0.08 * noise_burst(&mut rng, t, 0.0, 400.0)
    })
}

/// Comic crash: a woody thud (pitch-dropping body plus three inharmonic wood partials
/// and a knock) with a soft feather puff of noise blooming just after.
fn crash_thud() -> Vec<f32> {
    let mut rng = Rng::new(973);
    let mut knock = Svf::new(900.0, 3.0);
    let mut body = Ph(0.0);
    let mut puff: Vec<f32> = gen(0.5, |t, _| {
        let u = (t - 0.03).max(0.0);
        rng.signed() * (1.0 - (-u * 38.0).fe()) * (-u * 7.5).fe()
    });
    highpass(&mut puff, 450.0);
    lowpass(&mut puff, 3200.0);
    lowpass(&mut puff, 5000.0);
    let wood = [(1.0f32, 0.5f32, 40.0f32), (2.35, 0.35, 55.0), (3.9, 0.2, 80.0)];
    let mut out = gen(0.5, |t, i| {
        let thud = (TAU * body.adv(120.0 + 110.0 * (-t * 45.0).fe())).fs() * (-t * 14.0).fe() * (1.0 - (-t * 700.0).fe());
        let mut w = 0.0;
        for &(r, a, d) in &wood {
            w += (TAU * 310.0 * r * t).fs() * a * (-t * d).fe();
        }
        let (_, kn, _) = knock.tick(rng.signed());
        thud * 0.7 + w * (1.0 - (-t * 1500.0).fe()) * 0.65 + kn * (-t * 150.0).fe() * 0.35 + puff[i] * 2.4
    });
    mid_boost(&mut out, 0.8);
    out
}

/// Mud squelch: four rising bubbles popping over a low wet squish.
fn mud() -> Vec<f32> {
    let mut rng = Rng::new(974);
    let mut out = gen(0.3, |t, _| rng.signed() * (1.0 - (-t * 90.0).fe()) * (-t * 11.0).fe() * (0.7 + 0.3 * fsin(31.0 * t)));
    lowpass(&mut out, 900.0);
    lowpass(&mut out, 1600.0);
    for (t0, f0, g) in [(0.0f32, 190.0f32, 0.9f32), (0.06, 240.0, 0.8), (0.13, 170.0, 0.75), (0.2, 220.0, 0.5)] {
        let mut ph = Ph(0.0);
        let bub = gen(0.1, |t, _| {
            let f = f0 * (1.0 + 1.3 * (1.0 - (-t * 55.0).fe()));
            let p = ph.adv(f);
            ((TAU * p).fs() + 0.3 * (TAU * 2.0 * p).fs()) * (-t * 32.0).fe() * (1.0 - (-t * 800.0).fe())
        });
        mix_at(&mut out, t0, &bub, g);
    }
    out
}

/// A ghost rises: an airy "whoooo" sweeping upward, a breath of band-passed noise
/// following it, and a high tremolo shimmer. Swells in and dissolves to silence.
fn ghost_whoosh() -> Vec<f32> {
    let mut rng = Rng::new(975);
    let mut breath = Svf::new(500.0, 3.0);
    let mut ph = Ph(0.0);
    let mut sh = [0.0f32, 0.3, 0.7];
    let shimmer = [1900.0f32, 2470.0, 3180.0];
    let secs = 0.8;
    gen(secs, |t, _| {
        let u = t / secs;
        let env = smooth_step(0.0, 0.55, u) * (1.0 - smooth_step(0.6, 1.0, u));
        let f = 260.0 + 400.0 * u.powf(1.4) + 7.0 * fsin(5.5 * t);
        let p = ph.adv(f);
        let tone = (TAU * p).fs() + 0.35 * (TAU * 2.0 * p).fs() + 0.12 * (TAU * 3.0 * p).fs();
        breath.set(420.0 + 1500.0 * u, 3.0);
        let (_, air, _) = breath.tick(rng.signed());
        let mut tw = 0.0;
        for k in 0..3 {
            sh[k] = (sh[k] + shimmer[k] * (1.0 + 0.15 * u) / SR).fract();
            let trem = 0.5 + 0.5 * fsin(11.0 * t + 0.31 * k as f32);
            tw += fsin(sh[k]) * trem;
        }
        (tone * 0.5 + air * 0.9 + tw * 0.13) * env
    })
}

/// A ghost drops a hazard: a short falling whistle, then a soft heavy thud.
fn place_thud() -> Vec<f32> {
    const HIT: f32 = 0.15;
    let mut rng = Rng::new(976);
    let mut wh = Ph(0.0);
    let mut body = Ph(0.0);
    let mut dust: Vec<f32> = gen(0.3, |t, _| rng.signed() * (-t * 38.0).fe());
    lowpass(&mut dust, 600.0);
    let mut out = gen(0.4, |t, i| {
        let w = (TAU * wh.adv(620.0 + 900.0 * (-t * 14.0).fe())).fs() * fsin((t / (HIT + 0.02)).min(1.0) * 0.5) * 0.3;
        let tt = t - HIT;
        if tt < 0.0 {
            return w;
        }
        let thud = (TAU * body.adv(68.0 + 100.0 * (-tt * 30.0).fe())).fs() * (-tt * 10.0).fe() * (1.0 - (-tt * 500.0).fe());
        let knock = (TAU * 200.0 * tt).fs() * (-tt * 36.0).fe() * 0.8;
        w + softclip(thud * 1.6) * 0.8 + knock + dust.get(i - n(HIT).min(i)).copied().unwrap_or(0.0) * 1.2
    });
    mid_boost(&mut out, 2.0);
    out
}

/// A gentle low "nuh-uh": two soft descending blips.
fn place_denied() -> Vec<f32> {
    let mut ph = Ph(0.0);
    gen(0.15, |t, _| {
        let (f, local, len) = if t < 0.072 { (262.0, t, 0.072) } else { (196.0, t - 0.076, 0.074) };
        let x = (local / len).clamp(0.0, 1.0);
        let env = fsin(x * 0.5); // half-sine: 0 -> 1 -> 0 over the blip
        let p = ph.adv(f);
        ((TAU * p).fs() + 0.35 * (TAU * 2.0 * p).fs() + 0.15 * (TAU * 3.0 * p).fs()) * env
    })
}

/// "Safe time over": a short falling two-note tick.
fn protect_end() -> Vec<f32> {
    gen(0.2, |t, _| bell(t, 0.0, 880.0, 34.0) + bell(t, 0.095, 659.3, 34.0) * 0.9)
}
