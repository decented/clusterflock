# flock-audio API

Synthesised sound bank and music on rodio 0.20 (`default-features = false`, no decoders;
same device path as Twang Thang: cpal / AAudio on Android). Mono, 44.1 kHz. Nothing is
loaded from disk.

```rust
use flock_audio::{Audio, Sfx, LoopId, MusicTrack, coin_pitch};
```

## Audio

| Call | Meaning |
|---|---|
| `Audio::new() -> Audio` | Open the default output device, synthesise SFX + loops (synchronously, ~60-90 ms desktop CPU) and start rendering the three music loops (one background thread each). No device or stream error: returns a silent `Audio` (every call is a no-op). Not `Send` (holds the stream): keep it on the main thread. |
| `Audio::silent() -> Audio` | Explicit no-op instance (tests, screenshots, headless). |
| `is_live() -> bool` | A device is open. |
| `music_ready(MusicTrack) -> bool` | That loop has finished rendering (poll from the splash if you want music exactly on the first beat). |
| `play(sfx: Sfx, gain: f32, pitch: f32)` | One-shot. `gain` multiplies the effect's built-in level (1.0 = designed; clamped 0..4). `pitch` is a playback-rate ratio (1.0 = designed, 2.0 = octave up and twice as fast; clamped 0.25..4). Up to 32 voices; the oldest is dropped beyond that. |
| `set_loop(id: LoopId, gain: f32, pitch: f32)` | Live parameters of a looped voice. Call every frame; the mixer smooths (~20 ms gain, ~30 ms pitch). `gain` 0 = silent (clamped 0..2). `pitch` = playback-rate ratio (clamped 0.25..4). |
| `stop_loops()` | All loop gains to 0 (e.g. on leaving a race). |
| `set_music(Option<MusicTrack>)` | Start or switch music with a 1.2 s equal-power crossfade; `None` fades out. Setting the current track is a no-op. If the loop is still rendering, music starts when it is ready. |
| `set_volumes(master, music, sfx)` | Each 0..1, smoothed. |
| `pause_all()` / `resume_all()` | Pause: loops and gameplay one-shots go silent and freeze, music ducks by 60 %, UI sounds (`MenuTap/Back/Select`) still play. Resume undoes it. |
| `set_suspended(bool)` / `is_suspended()` | Platform lifecycle mute: immediately silences every output, including music and UI. Independent of pause and volumes; freezes music/loops and discards one-shots until foregrounded. |

`dsp`, `sfx` and `music` are public: `sfx::render_sfx(Sfx) -> Vec<f32>`,
`sfx::render_loop(LoopId) -> Vec<f32>`, `music::render_music(MusicTrack) -> Vec<f32>`
(buffers are peak-normalised below 1.0) and `coin_pitch(chain: u32) -> f32`.

## Sfx (38)

`Jump`, `Land`, `Boost`, `SparkTier1`, `SparkTier2`, `SparkTier3`, `LaserZap` (hit),
`TargetShatter`, `ItemTick`, `ItemGet`, `EggThrow`, `EggHit`, `Cactus`, `ShieldUp`,
`ShieldBlock`, `Lightning`, `MegaLaser`, `Bash` (thump + squawk), `Coin`, `CountdownBeep`,
`CountdownGo`, `Lap`, `FinalLap`, `Finish`, `MenuTap`, `MenuBack`, `MenuSelect`, `Laugh` (a short frog, jockey and ostrich chorus), `Trump` (Laser Trump: a comic raspberry rolling into a rising laser zap, about 1.2 s).
**Cluster Flock sounds** (appended after `Trump`; all synthesised, normalised like the rest):

| Sfx | Length | Use / notes |
|---|---|---|
| `Flap` | 0.25 s | Soft airy wing flutter (five wingbeats). Gain 1.0. |
| `Squawk` | 0.30 s | Comic ostrich honk. Designed for per-player pitch ratios 0.7..1.6 (0.43 s .. 0.19 s); stays a honk across that range. Gain ~1.0. |
| `Bump` | 0.26 s | Two runners bounce off each other: rubbery rising-falling boing. |
| `Crash` | 0.50 s | Obstacle hit: woody thud plus a soft feather puff. Comic, not violent. |
| `Mud` | 0.30 s | Squelch. |
| `GhostWhoosh` | 0.80 s | Elimination: airy rising "whoooo" with a shimmer; fades to silence. |
| `PlaceThud` | 0.40 s | A ghost drops a hazard: short falling whistle, then a soft heavy thud. |
| `PlaceDenied` | 0.15 s | Gentle low two-blip "nope" (kept quiet: base level 0.45). |
| `ProtectEnd` | 0.20 s | Falling two-note "safe time over" tick. |

`Jump` was retuned to a soft upward whoosh (0.26 s, API unchanged). Existing sounds still
exist (`Coin`, `CountdownBeep`, `CountdownGo`, `MenuTap/Back/Select`, `Land`, ...).
Base levels (mixer, before your `gain`): Flap 0.5, Squawk 0.7, Bump 0.7, Crash 0.8, Mud 0.65,
GhostWhoosh 0.55, PlaceThud 0.8, PlaceDenied 0.45, ProtectEnd 0.5.

Frog laughter plays once on entering the sprawled pause; nearby racers hear it
with distance attenuation. Snapshot corrections do not retrigger it.
`Sfx::ALL` lists them; `Sfx::name()` gives a file-friendly name.

**Coin chains:** `audio.play(Sfx::Coin, 1.0, coin_pitch(chain))` where `chain` is the number
of coins collected in the current quick run (0 for the first). The ratio climbs a major
pentatonic (0, 2, 4, 7, 9, 12, 14, 16 semitones) and holds at the top. Reset `chain` to 0
after about 1 s without a coin.

## LoopId (4)

`Footsteps`, `Wind`, `Skid`, `LaserHum`. `LoopId::ALL`.

## MusicTrack (3)

`Menu` (laid-back, 90 bpm, D minor, 8 bars = 21.3 s) and `Race` (driving, 150 bpm, A minor,
8 bars = 12.8 s, kick-sidechained bass and stabs) and `Farm` (Cluster Flock: light jaunty
farmyard tune, 120 bpm, G major, 8 bars = 16.0 s; oom-pah plucked bass and strummed chords,
banjo-ish lead with a hook, soft shaker, no kick; peak-normalised to 0.7, quieter than the
others). All are seamless by construction. `MusicTrack::ALL` is `[Menu, Race, Farm]`; each is
rendered on its own background thread (Farm takes ~50 ms CPU), so use
`music_ready(MusicTrack::Farm)` as for the others.

## Driving the loops from speed

Compute `s = speed / top_speed` (0..1+) once per frame and call `set_loop` for each voice.
Starting points, to be tuned by ear:

```rust
let s = (speed / top_speed).clamp(0.0, 1.2);
let grounded = !airborne;
audio.set_loop(LoopId::Footsteps, if grounded { (s * 3.0).min(1.0) } else { 0.0 }, 0.6 + 1.5 * s);
//   pitch is also the step rate: 1.0 = 5 steps/s, so 0.6 = a jog (3/s), 2.1 = a sprint (10.5/s).
audio.set_loop(LoopId::Wind, s * s * 0.9, 0.8 + 0.6 * s);
audio.set_loop(LoopId::Skid, if drifting { 0.5 + 0.5 * drift_slip } else { 0.0 }, 0.9 + 0.3 * s);
audio.set_loop(LoopId::LaserHum, if laser_charging { 0.6 + 0.4 * heat } else { 0.0 }, 0.8 + 0.8 * heat);
```

Footsteps is now a fast, light patter of big bird feet (still 4 steps per 0.8 s loop, 5 steps/s
at pitch 1.0; same API). Rate and pitch follow speed; silent in the air and when stopped.
Wind: quiet until about 40 % speed (quadratic), then rising. Skid: while the drift button is
held and slipping. LaserHum: while the laser is charging or firing; pitch climbs with heat.
Silence all four with gain 0 (or `stop_loops()`) on the results screen.

## Menus and race flow

- Splash/menus: `set_music(Some(Menu))`; tap `MenuTap`, back `MenuBack`, confirm `MenuSelect`.
- Countdown: `CountdownBeep` x3 then `CountdownGo`; start `set_music(Some(Race))` on Go (or
  at the first beep for a build-up).
- Lap line: `Lap`; the last lap line: `FinalLap`; the finish: `Finish`, then
  `set_music(Some(Menu))` for the results screen.
- Cluster Flock: `set_music(Some(Farm))` for the menu/run; `Flap`/`Jump` for the jump whoosh,
  `Squawk` with a per-player pitch ratio (0.7..1.6) when scared or bumped, `Bump` runner-vs-runner,
  `Crash`/`Mud` obstacles, `GhostWhoosh` on elimination, `PlaceThud`/`PlaceDenied` for ghost
  hazard placement, `ProtectEnd` when the protection time runs out.
- Pause menu: `pause_all()`; Resume: `resume_all()`.
- Settings sliders: `set_volumes(master, music, sfx)` (music 0 silences music only).

## Listening and timing

`cargo run -p flock-audio --example render_wav --release -- <dir>` writes every effect, each
loop (4 s), 20 s of each music loop, a `coin_chain.wav` and a `footsteps_ramp.wav`.
`--example profile_synth` prints CPU time per sound (Linux only).
