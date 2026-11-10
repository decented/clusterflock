use super::*;

fn peak(b: &[f32]) -> f32 {
    b.iter().fold(0.0f32, |m, x| m.max(x.abs()))
}

/// RMS of adjacent-sample differences: a yardstick for "how jumpy is this signal".
fn step_rms(b: &[f32]) -> f32 {
    let s: f32 = b.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum();
    (s / (b.len() - 1) as f32).sqrt()
}

/// The wrap from the last sample to the first is no bigger than an ordinary step.
fn assert_seamless(name: &str, b: &[f32]) {
    let seam = (b[0] - b[b.len() - 1]).abs();
    // No bigger than an ordinary step: 4x the RMS step, or (for signals with sharp
    // natural edges such as a saw wrap) the 99.9th-percentile step.
    let mut diffs: Vec<f32> = b.windows(2).map(|w| (w[1] - w[0]).abs()).collect();
    diffs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p999 = diffs[(diffs.len() as f32 * 0.999) as usize];
    let limit = (4.0 * step_rms(b)).max(1.25 * p999) + 0.02;
    assert!(seam <= limit, "{name}: seam jump {seam} exceeds {limit}");
}

fn assert_clean(name: &str, b: &[f32]) {
    assert!(!b.is_empty(), "{name}: empty");
    assert!(b.iter().all(|x| x.is_finite()), "{name}: non-finite sample");
    assert!(peak(b) <= 1.0, "{name}: peak {}", peak(b));
    assert!(peak(b) > 0.05, "{name}: silent");
}

#[test]
fn sfx_order_matches_discriminants() {
    for (i, s) in Sfx::ALL.iter().enumerate() {
        assert_eq!(*s as usize, i, "{:?}", s);
    }
    for (i, l) in LoopId::ALL.iter().enumerate() {
        assert_eq!(*l as usize, i);
    }
}

#[test]
fn every_sfx_is_clean_and_starts_and_ends_quietly() {
    for s in Sfx::ALL {
        let b = sfx::render_sfx(s);
        assert_clean(s.name(), &b);
        assert!(b[0].abs() < 0.05, "{}: starts with a click ({})", s.name(), b[0]);
        assert!(b[b.len() - 1].abs() < 0.02, "{}: ends with a click", s.name());
        assert!(b.len() < 3 * SAMPLE_RATE as usize, "{}: unreasonably long", s.name());
    }
}

#[test]
fn every_loop_is_clean_and_seamless() {
    for l in LoopId::ALL {
        let b = sfx::render_loop(l);
        assert_clean(l.name(), &b);
        assert_seamless(l.name(), &b);
    }
}

fn data(v: Vec<f32>) -> Arc<music::MusicData> {
    Arc::new(music::MusicData::single(v, 1000))
}

fn rms(b: &[f32]) -> f32 {
    (b.iter().map(|x| x * x).sum::<f32>() / b.len() as f32).sqrt()
}

#[test]
fn music_loops_are_clean_seamless_and_the_right_length() {
    for t in MusicTrack::ALL {
        let d = music::render_music(t);
        assert_eq!(d.len, music::loop_len(t), "{}", t.name());
        for (i, s) in d.stems.iter().enumerate() {
            let name = format!("{} stem {i}", t.name());
            // Run stems are levelled jointly and may exceed 1 on a rare transient (the deck
            // soft-clips), so only the menu must be a clean full-scale buffer.
            if d.reactive {
                assert!(s.iter().all(|x| x.is_finite()) && peak(s) > 0.05 && peak(s) < 2.5, "{name}: peak {}", peak(s));
            } else {
                assert_clean(&name, s);
            }
            assert_seamless(&name, s);
            assert_eq!(d.len % s.len(), 0, "{name}: stem length must divide the loop");
        }
    }
    // Menu: 16 bars at 124 bpm (a 16th is 5334.7 samples, rounded). Run: 48 bars at 172 bpm.
    assert_eq!(music::loop_len(MusicTrack::Menu), 5335 * 16 * 16);
    assert_eq!(music::loop_len(MusicTrack::Run), 3846 * 16 * 48);
    let run = music::render_music(MusicTrack::Run);
    assert_eq!(run.stems.len(), 4);
    assert!(run.reactive);
    // 48 bars at 172 bpm is about 67 s: long enough not to read as a loop.
    assert!(run.len as f32 / SAMPLE_RATE as f32 > 60.0);
}

#[test]
fn run_music_stems_balance_and_the_mix_stays_below_full_scale() {
    let d = Arc::new(music::render_music(MusicTrack::Run));
    // Joint normalisation: every layer on at once sits inside the headroom (the deck's
    // soft clip catches the rare louder transient).
    let all = Mood { intensity: 1.0, tension: 1.0, ghost: 0.0 };
    let full = (0..d.len).map(|i| d.stems.iter().map(|s| s[i % s.len()]).sum::<f32>()).fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(full > 0.3 && full < 2.5, "all stems summed peak {full}");
    let mut deck = Deck::new(Arc::clone(&d), &all);
    let top: Vec<f32> = (0..d.len).map(|_| deck.next(&all)).collect();
    assert!(peak(&top) < 1.0 && peak(&top) > 0.3, "top-speed peak {}", peak(&top));
    let mut deck = Deck::new(Arc::clone(&d), &Mood::neutral());
    let low: Vec<f32> = (0..d.len).map(|_| deck.next(&Mood::neutral())).collect();
    // Comparable loudness between the sparse and the full arrangement (within ~4 dB).
    let (rl, rt) = (rms(&low), rms(&top));
    assert!(rl > 0.06, "low-speed rms {rl}");
    assert!(rt / rl < 1.6 && rt / rl > 0.9, "rms low {rl} top {rt}");
    // The layers are different music, not copies.
    assert!(d.stems[1].iter().zip(d.stems[0].iter()).any(|(a, b)| (a - b).abs() > 0.05));
}

#[test]
fn mood_maps_to_layers_tempo_and_ghost() {
    let lo = deck::targets(&Mood::neutral());
    assert_eq!(lo.layers, [0.0, 0.0, 0.0]);
    assert_eq!(lo.rate, 1.0);
    let top = deck::targets(&Mood { intensity: 1.0, ..Mood::neutral() });
    assert_eq!(top.layers[0], 1.0);
    assert_eq!(top.layers[1], 1.0);
    assert_eq!(top.layers[2], 0.0);
    assert!((top.rate - 1.06).abs() < 1e-5);
    let tense = deck::targets(&Mood { intensity: 1.0, tension: 1.0, ghost: 0.0 });
    assert_eq!(tense.layers[2], 1.0);
    assert!(tense.rate <= 1.10);
    let ghost = deck::targets(&Mood { intensity: 0.5, tension: 0.0, ghost: 1.0 });
    assert!(ghost.rate < 1.0 && ghost.ghost == 1.0);
    // Monotone in intensity.
    let mut last = deck::targets(&Mood::neutral());
    for i in 1..=20 {
        let t = deck::targets(&Mood { intensity: i as f32 / 20.0, ..Mood::neutral() });
        assert!(t.rate >= last.rate && t.layers[0] >= last.layers[0] && t.layers[1] >= last.layers[1]);
        last = t;
    }
}

#[test]
fn deck_changes_are_smooth_when_layers_enter_and_the_ghost_arrives() {
    let d = Arc::new(music::render_music(MusicTrack::Run));
    let mut deck = Deck::new(Arc::clone(&d), &Mood::neutral());
    let sr = SAMPLE_RATE as usize;
    let calm: Vec<f32> = (0..sr * 2).map(|_| deck.next(&Mood::neutral())).collect();
    let busy = Mood { intensity: 1.0, tension: 1.0, ghost: 0.0 };
    let ramp: Vec<f32> = (0..sr * 6).map(|_| deck.next(&busy)).collect();
    // No step in the ramp is bigger than the music's own loudest steps (the drums).
    let own = calm.windows(2).chain(ramp.windows(2).skip(sr * 5)).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    let worst = ramp.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    assert!(worst <= own * 1.5 + 0.05, "ramp step {worst} vs {own}");
    assert!(deck.rate() > 1.05 && deck.layer_gains().iter().all(|g| *g > 0.9));
    let ghosted = Mood { ghost: 1.0, ..busy };
    let g: Vec<f32> = (0..sr * 6).map(|_| deck.next(&ghosted)).collect();
    assert!(g.iter().all(|x| x.is_finite() && x.abs() < 1.0));
    assert!(deck.rate() < 1.05, "the ghost slows the music ({})", deck.rate());
    // The ghost is darker: less energy up high than the same music alive.
    let alive: Vec<f32> = (0..sr * 3).map(|_| deck.next(&busy)).collect();
    let tail = &g[g.len() - sr * 3..];
    let hf = |b: &[f32]| {
        let mut h = b.to_vec();
        dsp::highpass(&mut h, 3000.0);
        rms(&h) / rms(b)
    };
    assert!(hf(tail) < hf(&alive) * 0.7, "ghost {} alive {}", hf(tail), hf(&alive));
}

#[test]
fn menu_is_calm_and_the_loop_point_is_smooth() {
    let d = music::render_music(MusicTrack::Menu);
    let b = &d.stems[0];
    assert!(!d.reactive);
    assert!(peak(b) <= 0.75, "menu peak {}", peak(b));
    assert!(rms(b) > 0.05 && rms(b) < 0.3, "menu rms {}", rms(b));
    assert_seamless("menu", b);
    let w = SAMPLE_RATE as usize / 100;
    let local = b[..w].windows(2).chain(b[b.len() - w..].windows(2)).map(|p| (p[1] - p[0]).abs()).fold(0.0f32, f32::max);
    let seam = (b[0] - b[b.len() - 1]).abs();
    assert!(seam <= local * 1.5 + 0.01, "menu loop point jumps {seam} (local max step {local})");
    assert_eq!(MusicTrack::ALL.len(), 2);
    // Every track has its own distinct slot in the bank.
    let bank = MusicBank::default();
    for t in MusicTrack::ALL {
        assert!(bank.slot(t.id()).is_some(), "{}", t.name());
        bank.slot(t.id()).unwrap().set(data(vec![t.id() as f32])).ok().unwrap();
    }
    assert_eq!(bank.get(MusicTrack::Run.id()).unwrap().stems[0][0], 2.0);
}

#[test]
fn karplus_strong_is_in_tune() {
    for m in [45.0f32, 57.0, 69.0, 81.0] {
        let f = dsp::mtof(m);
        let b = dsp::pluck_ks(f, 0.3, 0.78, 0.9965, 0.18, 4);
        assert_clean("ks", &b);
        // Autocorrelation peak near one period.
        let period = SAMPLE_RATE as f32 / f;
        let lo = (period * 0.9) as usize;
        let hi = (period * 1.1) as usize;
        let ac = |lag: usize| b[2000..8000].iter().zip(&b[2000 + lag..8000 + lag]).map(|(a, c)| a * c).sum::<f32>();
        let best = (lo..hi).max_by(|a, c| ac(*a).partial_cmp(&ac(*c)).unwrap()).unwrap();
        let cents = 1200.0 * (period / best as f32).log2().abs();
        assert!(cents < 20.0, "midi {m}: {cents} cents off (lag {best}, period {period})");
    }
}

#[test]
fn flock_sfx_have_the_designed_durations_and_stay_in_band() {
    let sr = SAMPLE_RATE as f32;
    // (sound, min seconds, max seconds)
    let cases = [
        (Sfx::Flap, 0.2, 0.3),
        (Sfx::Squawk, 0.25, 0.35),
        (Sfx::Bump, 0.2, 0.3),
        (Sfx::Crash, 0.4, 0.6),
        (Sfx::Mud, 0.25, 0.4),
        (Sfx::GhostWhoosh, 0.7, 0.9),
        (Sfx::PlaceThud, 0.3, 0.5),
        (Sfx::PlaceDenied, 0.12, 0.2),
        (Sfx::ProtectEnd, 0.15, 0.25),
        (Sfx::Jump, 0.2, 0.35),
    ];
    for (s, lo, hi) in cases {
        let b = sfx::render_sfx(s);
        assert_clean(s.name(), &b);
        let secs = b.len() as f32 / sr;
        assert!((lo..=hi).contains(&secs), "{} lasts {secs} s", s.name());
        assert!(peak(&b) < 0.95, "{} peak {}", s.name(), peak(&b));
        // Nothing should be a harsh fizz: most energy stays below 8 kHz-ish, so the
        // sample-to-sample step stays well under the white-noise level.
        assert!(step_rms(&b) < 0.6 * peak(&b), "{} is too fizzy", s.name());
    }
}

#[test]
fn squawk_stays_clean_across_the_app_pitch_range() {
    let b = sfx::render_sfx(Sfx::Squawk);
    for pitch in [0.7f32, 1.0, 1.6] {
        let r = crate::dsp::resample(&b, pitch);
        assert_clean("squawk resampled", &r);
        let secs = r.len() as f32 / SAMPLE_RATE as f32;
        assert!((0.15..0.5).contains(&secs), "squawk at {pitch}x lasts {secs} s");
    }
}

#[test]
fn footsteps_are_four_light_steps_in_point_eight_seconds() {
    let b = sfx::render_loop(LoopId::Footsteps);
    assert_eq!(b.len(), crate::dsp::n(0.8));
    // Each quarter of the loop holds one footfall: a clear peak in the first 30 ms.
    let q = b.len() / 4;
    for k in 0..4 {
        let head = peak(&b[k * q..k * q + SAMPLE_RATE as usize * 3 / 100]);
        let tail = peak(&b[k * q + q * 3 / 4..(k + 1) * q]);
        assert!(head > 0.2 && head > 4.0 * tail, "step {k}: head {head} tail {tail}");
    }
}

#[test]
fn coin_pitch_climbs_then_caps() {
    assert_eq!(coin_pitch(0), 1.0);
    let mut last = 0.0;
    for c in 0..8 {
        assert!(coin_pitch(c) > last);
        last = coin_pitch(c);
    }
    assert_eq!(coin_pitch(7), coin_pitch(100));
}

fn test_mixer() -> (Mixer, Sender<Voice>, Arc<Shared>, Arc<MusicBank>) {
    let shared = Arc::new(Shared::new());
    let music_bank = Arc::new(MusicBank::default());
    let bank = Bank {
        sfx: Sfx::ALL.iter().map(|&s| sfx::render_sfx(s).into()).collect(),
        loops: LoopId::ALL.iter().map(|&l| sfx::render_loop(l).into()).collect(),
    };
    let (tx, rx) = channel();
    let mixer = Mixer::new(rx, Arc::clone(&shared), &bank, Arc::clone(&music_bank));
    (mixer, tx, shared, music_bank)
}

#[test]
fn mixer_mixes_voices_and_stays_bounded() {
    let (mut m, tx, shared, _) = test_mixer();
    let loud: Arc<[f32]> = vec![0.9; 4000].into();
    for _ in 0..60 {
        tx.send(Voice { buf: Arc::clone(&loud), pos: 0.0, rate: 1.0, gain: 3.0, ui: false }).unwrap();
    }
    for id in LoopId::ALL {
        store(&shared.loop_gain[id as usize], 2.0);
    }
    let out: Vec<f32> = (&mut m).take(SAMPLE_RATE as usize / 2).collect();
    assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    assert!(peak(&out) > 0.3);
    assert!(m.voices.len() <= MAX_VOICES);
}

#[test]
fn loops_follow_gain_and_go_quiet() {
    let (mut m, _tx, shared, _) = test_mixer();
    let quiet: Vec<f32> = (&mut m).take(2000).collect();
    assert_eq!(peak(&quiet), 0.0);
    store(&shared.loop_gain[LoopId::Wind as usize], 1.0);
    let _ = (&mut m).take(8000).count();
    let loud: Vec<f32> = (&mut m).take(8000).collect();
    assert!(peak(&loud) > 0.1);
    store(&shared.loop_gain[LoopId::Wind as usize], 0.0);
    let _ = (&mut m).take(SAMPLE_RATE as usize).count();
    let after: Vec<f32> = (&mut m).take(4000).collect();
    assert!(peak(&after) < 0.01);
}

#[test]
fn pause_silences_gameplay_but_not_ui() {
    let (mut m, tx, shared, _) = test_mixer();
    let tone: Arc<[f32]> = (0..30000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect::<Vec<_>>().into();
    shared.paused.store(true, Ordering::Relaxed);
    let _ = (&mut m).take(SAMPLE_RATE as usize / 2).count(); // let the pause ramp settle
    tx.send(Voice { buf: Arc::clone(&tone), pos: 0.0, rate: 1.0, gain: 1.0, ui: false }).unwrap();
    let game: Vec<f32> = (&mut m).take(3000).collect();
    assert!(peak(&game) < 1e-3, "gameplay voice leaked while paused");
    tx.send(Voice { buf: Arc::clone(&tone), pos: 0.0, rate: 1.0, gain: 1.0, ui: true }).unwrap();
    let ui: Vec<f32> = (&mut m).take(3000).collect();
    assert!(peak(&ui) > 0.1, "UI voice should still play");
    // Resume: the frozen gameplay voice carries on.
    shared.paused.store(false, Ordering::Relaxed);
    let _ = (&mut m).take(SAMPLE_RATE as usize / 2).count();
    assert!(m.voices.iter().all(|v| v.pos > 0.0) || m.voices.is_empty());
}

#[test]
fn suspension_silences_music_loops_and_ui_even_if_game_pause_is_resumed() {
    let (mut mixer, tx, shared, bank) = test_mixer();
    let tone: Arc<[f32]> = vec![0.5; SAMPLE_RATE as usize * 2].into();
    bank.menu.set(data((*tone).to_vec())).ok();
    shared.track.store(MusicTrack::Menu.id(), Ordering::Relaxed);
    store(&shared.loop_gain[LoopId::Wind as usize], 1.0);
    let warm: Vec<f32> = (&mut mixer).take(8000).collect();
    assert!(peak(&warm) > 0.1);
    let music_pos = mixer.cur.deck.as_ref().unwrap().pos;
    let mut audio = Audio::silent();
    audio.shared = Arc::clone(&shared);
    audio.set_suspended(true);
    audio.pause_all();
    audio.resume_all(); // A screen callback cannot undo platform silence.
    audio.set_volumes(1.0, 1.0, 1.0);
    for ui in [false, true] {
        tx.send(Voice { buf: Arc::clone(&tone), pos: 0.0, rate: 1.0, gain: 1.0, ui }).unwrap();
    }
    let quiet: Vec<f32> = (&mut mixer).take(8000).collect();
    assert_eq!(peak(&quiet), 0.0, "all output stops without a fade or another game frame");
    assert_eq!(mixer.cur.deck.as_ref().unwrap().pos, music_pos, "music stays in place while the screen is off");
    assert!(mixer.voices.is_empty(), "background one-shots must not replay on return");
    audio.set_suspended(false);
    let resumed: Vec<f32> = (&mut mixer).take(8000).collect();
    assert!(peak(&resumed) > 0.1);
}

#[test]
fn music_crossfades_between_tracks() {
    let (mut m, _tx, shared, bank) = test_mixer();
    bank.menu.set(data(vec![0.5; 100_000])).ok();
    bank.run.set(data(vec![-0.5; 100_000])).ok();
    shared.track.store(1, Ordering::Relaxed);
    let first: Vec<f32> = (&mut m).take(SAMPLE_RATE as usize * 2).collect();
    assert!(*first.last().unwrap() > 0.1, "menu music should be playing");
    shared.track.store(2, Ordering::Relaxed);
    let fade: Vec<f32> = (&mut m).take(SAMPLE_RATE as usize * 3).collect();
    assert!(*fade.last().unwrap() < -0.1, "run music should have taken over");
    // No jump bigger than the smoothed fade allows.
    let max_step = fade.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
    assert!(max_step < 0.01, "crossfade stepped by {max_step}");
    shared.track.store(0, Ordering::Relaxed);
    let _ = (&mut m).take(SAMPLE_RATE as usize * 2).count();
    let off: Vec<f32> = (&mut m).take(2000).collect();
    assert!(peak(&off) < 1e-3, "music should fade out on None");
}

#[test]
fn music_waits_for_its_render() {
    let (mut m, _tx, shared, bank) = test_mixer();
    shared.track.store(2, Ordering::Relaxed);
    let before: Vec<f32> = (&mut m).take(5000).collect();
    assert_eq!(peak(&before), 0.0);
    bank.run.set(data(vec![0.4; 100_000])).ok();
    let after: Vec<f32> = (&mut m).take(SAMPLE_RATE as usize).collect();
    assert!(peak(&after) > 0.05);
}

#[test]
fn silent_audio_accepts_every_call() {
    let a = Audio::silent();
    assert!(!a.is_live());
    for s in Sfx::ALL {
        a.play(s, 1.0, 1.0);
    }
    for l in LoopId::ALL {
        a.set_loop(l, 1.0, 1.0);
    }
    a.set_music(Some(MusicTrack::Run));
    a.set_mood(Mood { intensity: 1.0, tension: 1.0, ghost: 1.0 });
    a.set_music(None);
    a.set_volumes(0.5, 0.5, 0.5);
    a.pause_all();
    a.resume_all();
    a.stop_loops();
    assert!(!a.music_ready(MusicTrack::Menu));
}

#[test]
fn startup_synthesis_time() {
    let t0 = std::time::Instant::now();
    let _ = Bank::synthesise();
    let sfx_ms = t0.elapsed().as_millis();
    let t1 = std::time::Instant::now();
    let _ = music::render_music(MusicTrack::Run);
    let run_ms = t1.elapsed().as_millis();
    let t2 = std::time::Instant::now();
    let _ = music::render_music(MusicTrack::Menu);
    let menu_ms = t2.elapsed().as_millis();
    println!("synthesis: sfx+loops {sfx_ms} ms, run {run_ms} ms, menu {menu_ms} ms (profile-dependent)");
}

#[test]
fn fast_exp_and_sin_are_accurate() {
    use crate::dsp::{fexp, fsin};
    let mut worst = 0.0f32;
    let mut x = -20.0f32;
    while x < 5.0 {
        worst = worst.max((fexp(x) / x.exp() - 1.0).abs());
        x += 0.0137;
    }
    assert!(worst < 3e-4, "fexp relative error {worst}");
    // Never grows for a negative argument (so envelopes cannot blow up).
    assert!(fexp(-1e-6) <= 1.0 && fexp(-3e-5) <= 1.0);
    let mut worst = 0.0f32;
    let mut c = -3.0f32;
    while c < 3.0 {
        worst = worst.max((fsin(c) - (c * std::f32::consts::TAU).sin()).abs());
        c += 0.00731;
    }
    assert!(worst < 2e-3, "fsin abs error {worst}");
}

#[test]
fn trump_is_a_raspberry_then_a_rising_zap() {
    let b = sfx::render_sfx(Sfx::Trump);
    assert_clean("trump", &b);
    let secs = b.len() as f32 / SAMPLE_RATE as f32;
    assert!((1.0..1.4).contains(&secs), "trump lasts {secs} s");
    // Both halves carry sound: the raspberry first, the zap after it.
    let sr = SAMPLE_RATE as usize;
    assert!(peak(&b[..sr / 2]) > 0.2, "raspberry too quiet");
    assert!(peak(&b[sr * 3 / 4..sr]) > 0.1, "zap too quiet");
    // The zap is brighter than the raspberry: more zero crossings per second.
    let crossings = |s: &[f32]| {
        s.windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count()
    };
    assert!(
        crossings(&b[sr * 9 / 10..sr * 11 / 10]) > 2 * crossings(&b[sr / 10..sr / 10 + sr / 5])
    );
}
