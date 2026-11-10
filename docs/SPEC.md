# Cluster Flock — build spec

Display name **Cluster Flock** (provisional). Code names use
`flock` / `gameflock`. This spec records
the decisions taken for the Android build. Numbers are tunables (`Tuning`), not settled balance.

## 0. Decisions

| Topic | Decision |
|---|---|
| Platform | Android APK only (arm64), plus the desktop Linux build for development and `--shot`. No browser build, no embedding. |
| Base | Forked from Stampede's Rust workspace (wgpu 29, winit, egui 0.34, rodio, no Gradle). Bluetooth ported from Twang Thang. Results table descends from Twang Thang's. |
| Package | `org.decented.gameflock`, activity `FlockActivity`, links `gameflock://join#CODE`. (`cc.trotters.flock` is the ForgeSworn location app — do not collide.) |
| Orientation | Portrait everywhere. |
| Authority | The phone hosting the room is the authority (Fray host). No dedicated server needed. Solo runs the same sim locally with no network code at all. |
| Host loss | No host migration in v1. Host leaving ends the round cleanly for everyone with "Host left" and offers Solo / back to lobby list. Known gap. |
| Transports | Online (room code over Nostr rendezvous + QUIC), Nearby (LAN), Bluetooth (Twang's BLE bridge as a Fray virtual link). Same lobby for all. |
| Identity | My Signet sign-in is mandatory for any multiplayer entry (host, join, nearby, bluetooth). Solo and Flock Practice need no account and no network. |
| Updater | In-app updater pinned to the project's release key (see `README.md`, Releases). |
| Prediction | Own runner predicted with `flock_core::predict_runner` and replay of unacked inputs (Stampede pattern); others interpolated with adaptive delay; contacts, damage and coins are host-only. Plus immediate cosmetic anticipation of the local command. |
| Bots | Core ships runner bots and ghost bots. Used for: Flock Practice (you + up to 5 bots, offline), host-added bots in a lobby, and the debug congestion tests. Solo has none. |
| Haptics | On by default; the player can switch them off in Settings. |

## 1. Workspace

```
crates/flock-core     pure simulation, no I/O, no rendering, no glam  (this spec §2-§9)
crates/flock-session  match protocol over Fray: LocalRun, host authority, ClientSession, lobby (§10)
crates/flock-render   race-render fork: farm course, floppy Nostrich, props, ghosts (§11)
crates/flock-audio    race-audio fork: synthesised sounds (§12)
blockseed (external)  the daily run's Bitcoin-block seed: verification and fetch (§14)
app                   package `gameflock`: shell, screens, platform, BLE, account (§13)
```

Git deps pinned by revision: `decented-signet` from forgesworn/signet-play, and `fray-net` and
`fray-relays` from forgesworn/fray at the revision signet-play pins, so cargo builds one copy.

## 2. flock-core: coordinates, clock, ids

- Metres. `x` lateral (+ = runner's right), `y` up (0 = ground), `z` forward along the course.
  Matches the renderer: Y up, yaw 0 faces +Z, `right = (-1, 0, 0)` when yaw 0 — **note**: the
  renderer's `right` for yaw 0 is −X, so the app maps sim `x` to render `-x` (one place:
  the presenter). Document this at the mapping site.
- Course progress `z` is `f64`; everything else `f32`. Rendering subtracts a floating origin.
- Fixed tick `TICK_HZ = 60`. All gameplay timers count ticks (`u32`/`u64`), never wall time.
  Pausing = not calling `step`.
- `PlayerId(u8)` 0..=5. `MAX_PLAYERS = 6`. Data structures sized for 6 from the start.
- Lanes `0,1,2` (left→right), centres `x = (lane - 1) * lane_width`. Fences at `±fence_x`.

## 3. flock-core: Tuning (serde, `Default`, JSON-loadable)

All of these live in `Tuning` (nested structs fine). Defaults:

| Name | Default | Meaning |
|---|---|---|
| lane_width | 2.2 | m |
| fence_x | 3.7 | inner face of edge fences |
| body_radius | 0.42 | runner footprint circle (x-z) |
| height_stand / height_low | 1.5 / 0.55 | collision top when standing / collapsed or tumbling |
| gravity | 30.0 | m/s² |
| jump_vy | 9.0 | airtime ≈0.6 s, apex ≈1.35 m |
| jump_windup | 0.09 s | consistent, learnable |
| jump_fwd_boost | 2.0 m/s | decays with `boost_tau` 0.5 s |
| down_windup | 0.05 s | |
| collapse_time | 0.70 s | low profile duration, then stand-up |
| collapse_fwd_loss | 3.0 m/s | |
| standup_time | 0.30 s | Recovering, not controllable |
| lateral_k / lateral_zeta / lateral_amax | 140 / 0.5 / 60 | underdamped lane spring: lurch, overshoot, wobble |
| diag_jump_fwd_bias / diag_dive_back_bias | +1.0 / −1.5 m/s | |
| input_buffer | 0.15 s | one buffered command |
| combo_window (keyboard diagonal) | 0.08 s | app-side |
| course_v0 / course_vmax / course_ramp_m | 10 / 18 / 1800 | solo: `v = v0 + (vmax-v0)(1-e^(-d/ramp))` |
| mp_v_end | 15 | multiplayer: linear by time to this at the cap |
| pack_k | 0.6 /s | gentle pull of each runner to the pack anchor |
| pack_band | 7.0 m | soft wall at ±band from anchor (stronger pull outside) |
| pack_anchor_follow | 0.5 /s | anchor drifts toward mean living z |
| bump_restitution | 0.35 | runner-runner |
| significant_impact | 2.5 m/s | closing speed for a "significant" new contact |
| tumble_impulse | 4.5 m/s | lateral/rear impulse over this → tumble |
| tumble_time | 1.2 s | at the mercy of physics, then standup |
| ramp_hop_vy | 6.0 | involuntary hop off a collapsed body |
| feathers | 3 | |
| protect_time | 3.0 s (default now 4.0 s, see §6) | |
| crash_knockback | 6.0 m/s | rearward on a damaging crash |
| mud_speed_mult / mud_wobble | 0.7 / 1.0 | |
| countdown | 3.0 s | |
| run_in | 45 m | clear course at the start (solo) |
| mp_run_in | 75 m | clear course at the start (multiplayer: a longer run so the start grid spreads out) |
| mp_gentle_m / mp_ramp_m | 230 / 200 m | multiplayer opening after the run-in: only single-lane hay/beams, at least 5 rows apart; then the full pool is phased in linearly over `mp_ramp_m` |
| mp_cap | 180 s | multiplayer only |
| row_spacing | 6.0 m | discrete course/placement rows |
| gen_ahead | 220 m | course generated beyond the lead runner |
| keep_behind | 30 m | release rows this far behind the trailing runner |
| band_near_s / band_far_s | 3.0 / 5.0 | ghost placement band, seconds ahead of the leader at `v_course + jump_fwd_boost` |
| min_reaction_s | 1.6 | never place closer than this to any living runner |
| materialise_s | 0.5 | drop-in animation from acceptance to solid |
| ghost_energy_max / start / regen | 10 / 5 / 0.9 per s | |
| cost: coins / mud / hay / beam | 1 / 2 / 3 / 4 | |
| density: max architect solids per row / per 4 rows | 1 / 3 | |
| solo_difficulty_ramp_m / solo_difficulty_floor_m | 1500 / 175 m | solo row difficulty `1 − e^(−max(row_z − run_in, floor)/ramp)` (a pure function of the row, §4) |
| powerup_gap_m / mp_powerup_gap_m | 240 / 160 m | mean distance between power-ups, solo / multi (0 = none) |
| powerup_jitter / powerup_weights | 0.35 / [40, 35, 25] | ±35 % seeded jitter per gap; Magnet / Doubler / Shield |
| magnet_s / magnet_reach_lanes / magnet_reach_z | 8 s / 1.15 / 2.5 m | magnet: also collects coins within 1.15 lane widths across and 2.5 m along, any height |
| doubler_s / shield_s | 10 s / 20 s | doubler: each coin counts 2; shield: held up to 20 s |

Hazard geometry (also `Tuning`): hay bale top 0.85, depth 1.0, lane-wide 1.6;
beam underside 1.0, depth 0.4, spans one lane or all three (`Beam{span}`);
block (barn wall / tractor / trough) height 2.5, depth 3.0, one lane; mud
depth 6.0 one lane; coin radius 0.35, collect radius 0.6, heights 0.8 (ground)
or along a jump arc.

## 4. Course

- The course is a sequence of **rows** at `z = row_index * row_spacing`. Each row holds, per
  lane, at most one *solid* (`Hay`, `Beam`, `Block`, `Barrel`) plus optional `Mud`, coins and
  (at most one per row) a power-up. Cell byte: bits 0-2 solid, bit 3 mud, bits 4-5 coins,
  bits 6-7 power-up (0 none, 1 Magnet, 2 Doubler, 3 Shield). `Row.coins_taken` bit 15 =
  the row's power-up was collected; power-up ids use slot 7.
  Rows are generated from a **SplitMix64** seeded by `(match_seed, row_index)` with pinned
  draw functions (no `rand` in generation) — generated rows are stored once and never re-rolled.
- **Purity (daily runs).** In solo every generated row is a pure function of
  `(seed, row_index, Tuning, GENERATOR_VERSION)`: difficulty is computed per row from its z,
  the speed model is a function of z, and nothing that feeds generation or the simulation
  uses libm (`exp`, `sin` and friends are replaced by `flock_core::det`, built from
  IEEE `+ − × ÷` only). `flock_core::course_fingerprint(cfg, rows)` (SHA-256 of
  `row_index` LE + 3 cell bytes per row) lets a device prove its daily course; a golden
  hash for `MatchConfig::daily(0x0123456789abcdef)` is pinned by a test. Multiplayer rows
  still depend on host state (time, ghost count, placements) by design and are streamed.
- Every object has a stable `ObjId(u32)` (generator: derived from row+lane+kind; architect:
  sequential from a separate range) and an explicit `Damage` class:
  `Crash` (hay frontal hit, beam hit while tall, block hit), `Slow` (mud), `None` (coins).
  Damage comes from the classification and the body/obstacle geometry test in §6, never
  from "whichever mesh touched first".
- **Generator**: difficulty `0..1` from distance (solo) or time + architect count (multi:
  procedural density scales down by `1 / (1 + 0.25 * ghosts)` so procedural and architect
  hazards are not both maximised). Pattern pool grows with difficulty: single hay, single
  beam, beam across all three lanes (duck), hay in two lanes, block + hay, mud lane,
  zig-zag coin trails, coin arcs over hay. First `run_in` metres empty (multiplayer: `mp_run_in`, then a gentle stretch, see tuning).
- **Feasibility** (shared by generator and architect validation): a DP over rows. State =
  (lane, earliest tick-distance at which the runner is actionable again). A transition
  into lane `b` at row `r` from lane `a` needs: lane-change time `|a-b| * lane_change_s`
  (0.32 s per lane at the speed of that row) fits in the gap; the solid at `(r,b)` is
  passable by an action (`Hay`→jump, `Beam`→collapse, `Block`→never, empty→run) and that
  action can start (windup) after the previous action finished. Use the *maximum*
  plausible speed (`v_course + jump_fwd_boost`) for timing. The course is valid while at
  least one lane is reachable at every row. Coins and power-ups never block. **Mud** never
  blocks running, but no jump can start in it (§5): a jump counts only if its command and
  wind-up can lie outside every mud span of that lane at every speed from the slowest
  plausible (`v − plausible_margin`) to the max plausible. So a hay bale / barrel that can
  only be cleared by taking off from mud is rejected by the generator and by ghost
  placement validation (ghost **mud** now runs the DP too).
- Power-ups: one every `powerup_gap_m` (± `powerup_jitter`) after the run-in, in a random lane
  of the due row without a solid or mud (else the next row that has one), kind by
  `powerup_weights`; drawn from a separate per-row stream after the row is accepted, so
  they never affect feasibility.
  The generator rejects a candidate row that would break feasibility and falls back to an
  emptier pattern; it never emits an impossible course.
- Rows are released `keep_behind` behind the trailing living runner (including a
  recovering one). Long runs keep only O(window) rows; `z` stays `f64`.

## 5. Runners

`Runner { id, pos: (f32 x, f32 y, f64 z), vel: (vx, vy, vz_offset), loco: Loco, feathers,
protected_until: Option<Tick>, coins: u32, settle_lane: u8, target_lane: u8, buffered:
Option<(Command, Tick)>, contacts_prev: bitset of other runners + obj ids touched last tick,
alive_ticks, stats }`

`Loco` (locomotion is separate from protection):
`Running`, `WindUp { kind, until }`, `Airborne { voluntary: bool, can_act: false }`,
`Collapsed { until }`, `Tumbling { until, spin_dir }`, `Recovering { until }`.
Controllable = `Running` (commands start immediately) — commands in any other state are
buffered for `input_buffer`; a buffered command fires on the first controllable tick if
still fresh. Only one is held; a newer one replaces it. Held keys never repeat (app sends
edges only).

**Mud:** a runner whose body is in mud (`in_mud`) cannot start a jump. A `Jump` (or a
buffered one firing) is refused and dropped (event `MudStuck`); `JumpLeft/Right` still
change lane but do not jump. `Down`, dives and lane changes work.

Runner additions (0.2): `free_hit: bool` (the protection episode's free hit is still
available; HUD), `swagger_obj`, and power-up timers `magnet_until`, `double_until`,
`shield_until` (ticks; active while `tick < until`).

`Command`: `Left, Right, Jump, Down, JumpLeft, JumpRight, DiveLeft, DiveRight`.
- Left/Right: `target_lane ±1` (at an edge: lurch into the fence, bounce, wobble, no damage).
- Jump: WindUp → Airborne(voluntary) with `vy = jump_vy`, forward boost.
- Down: WindUp(short) → Collapsed: low profile, forward loss. Voluntary; never costs a
  feather and never starts protection.
- JumpLeft/Right: jump + target lane change with forward bias. DiveLeft/Right: collapse
  + lane change with rearward bias.

Lateral motion is a force toward `target_x` (spring `lateral_k`, `lateral_zeta`, accel
clamp) — so the bird lurches, overshoots and wobbles, and its body pushes into whoever is
there. After a tumble or a major collision `target_lane` is reset to the lane the body is
actually in (`settle_lane` = nearest lane), not the one originally requested.

Forward: `vz = v_course + boost + pack_correction + mud + knockback` where
`pack_correction = -pack_k * (z - anchor)` plus a stronger soft wall beyond `pack_band`.
The anchor advances at `v_course` and drifts toward the mean living z at
`pack_anchor_follow`. Solo: the anchor is the runner (correction 0).

Floppiness that matters for play: the windups, the lateral overshoot, knockback and
tumbles all change the authoritative body. Purely visual wobble (neck, legs, wings) is
computed in the presenter from the body's accelerations (`flock_core::flop::Flop`, a
deterministic spring set the app steps per runner; not networked).

## 6. Contacts and damage

Per tick, after integration:

1. **Runner–runner**: circles in x-z, only if their vertical extents overlap. Push apart
   by mass-equal positional correction + impulse with `bump_restitution`. A *new* contact
   (pair not in `contacts_prev`) with closing speed > `significant_impact` is a
   significant impact for both. Impulse > `tumble_impulse` → the receiver tumbles. Six
   runners in one lane must form a queue/pile-up with no teleporting and no deletion.
   An incoming lane-changer is pushed back out, squeezes in, or shoves the other — purely
   from the impulses; no random outcome selector.
2. **Collapsed ramp**: a grounded, standing runner whose footprint overlaps a `Collapsed`
   or `Tumbling` runner (behind→over contact, actual overlap only) gets an involuntary
   hop `Airborne{voluntary:false}` with `ramp_hop_vy`. It cannot jump until it lands
   (that uses its jump opportunity). An already-airborne runner that contacts a low body
   has its trajectory disrupted (vz −2 m/s, vy clamped to ≤ 2). Ducking affects only
   bodies that touch.
3. **Obstacles**: test the body (footprint circle × [y, y+height]) against each solid's
   box. Hay: frontal overlap with body bottom below hay top → Crash. Beam: body top above
   beam underside → Crash. Block: any overlap → Crash. Mud: overlap → speed multiplier
   and wobble, no damage. One **incident** per (runner, obstacle id); at most one feather
   lost per runner per tick.
4. **Crash** (vulnerable): knockback rearward, body tumbles; `target_lane` resets to actual
   lane. A held **shield** takes it (no feather, `ShieldBroken`); otherwise lose a feather
   (coin speed bonus halved). Final feather → eliminated (multi: becomes Ghost this tick;
   solo: GameOver). Otherwise a protection **episode** starts:
   `protected_until = now + protect_time`, with exactly **one free hit** (`free_hit`).
5. **Protection (0.2 decision, replaces the old refresh rule)**: "hitting things
   keeps you invincible" is gone. During an episode the first damaging-obstacle hit is
   free: no feather, no tumble, no knockback, no speed or control loss, coin bonus kept;
   the runner carries on through the obstacle (event `Swagger { player, obj }`, cosmetic).
   Any further damaging hit in the same episode — even with time left — ends protection at
   once and is exactly a vulnerable crash (shield check, feather, tumble, a new episode with
   its own free hit; on the last feather: eliminated). Protection is **never extended**:
   runner–runner contacts keep their physical effect but never refresh it nor use the free
   hit. It ends when the timer has passed and the runner is controllable again
   (`ProtectionEnd`). Several contacts with one obstacle are one hit; at most one feather
   per runner per tick. Voluntary actions never start protection. Log (event) any episode
   over 8 s.
6. **Coins**: a coin is collected by a runner whose body overlaps its sphere (height
   matters: jumping over a low coin does not collect it). If several overlap in the same
   tick, the nearest centre wins, then lower `PlayerId`. Exactly one award; coin removed
   for everyone. **Power-ups** are collected the same way (event `PowerTaken { obj, by,
   kind }`, gone for everyone): Magnet (`magnet_s`: also collects coins within
   `magnet_reach_lanes` lane widths across and `magnet_reach_z` along, any height),
   Doubler (`doubler_s`: each coin counts 2; the speed bonus still grows by one coin's
   worth), Shield (held up to `shield_s`; see 4). Picking up a kind already active restarts
   its timer.
7. Edge fences: solid walls at `±fence_x`; hitting one bounces, never damages.

## 7. Roles, ghosts and placement

`Role::Runner | Ghost { energy: f32, placed: u32 } | Disconnected`.

- On elimination in multi: role becomes Ghost the same tick, with `ghost_energy_start`
  energy, body removed from collisions, event `BecameGhost`. No spectator wait.
- Ghosts send `PlaceRequest { player, req: u32 (per-player sequence), tool: Tool, lane, row }`.
  `Tool = Coins | Mud | Hay | Beam`. `World::place` validates atomically, in this order,
  each failure a `Reject` reason with no energy spent:
  `NotGhost`, `Duplicate` (returns the original result for a repeated `(player, req)`),
  `OutsideBand`, `TooClose` (within `min_reaction_s` of any living runner at max
  plausible speed), `Occupied` (cell has a solid, or a pending accepted placement),
  `Density`, `Unsafe` (feasibility DP fails with this added, including pending
  placements), `NoEnergy`.
- Accepted: energy charged, object stored as pending with `solid_at = now + materialise`;
  visible to everyone at once as a reservation (translucent) and solid after
  materialisation. Event `Placed { by, obj, tool, lane, row }`.
- `World::check_placement(player, tool, lane, row) -> Result<cost, Reject>` is the same
  logic without side effects (UI preview). `World::placement_band() -> RowRange`.
- Coins placed by ghosts are lures: a short line of 5 coins; never counted as blocking.
- Ghosts regenerate energy every tick; they cannot issue runner commands (ignored + counted).

## 8. Match flow and results

`MatchConfig { seed: u64, mode: Mode::{Solo, Multi}, players: Vec<PlayerSpec { id, name,
colour: u8, bot: Option<BotSkill>, hat: u8 /* 255 = default hat for the colour; cosmetic */ }>,
tuning: Tuning, round: u32 }`. `MatchConfig::daily(seed)` = solo rules on a published seed.

Phases: `Countdown` → `Running` → `Over`. Start grid: up to 6 in two rows of three
lanes (row gap 3 m), assignment rotated by `round` (and seed) so nobody is always at the
back. Clear `run_in`.

Ends: solo — final feather. Multi — no runners left, or `mp_cap` elapsed, or fewer than
2 connected humans+bots remain (`EndReason::NotEnoughPlayers`).

`World::standings() -> Vec<Standing { player, place: u8 (shared on ties), alive,
survived_ticks, feathers, coins, placements, disconnected }>`:
runners alive at the end rank above eliminated; alive ones by feathers then coins;
eliminated by survival time then coins; equal → shared place. Disconnected players rank
last, flagged. Message order never breaks ties. Solo result: `distance_m`, `coins`.

`World::disconnect(id)`: body removed at once (no permanent blocker), role
`Disconnected`, `survived_ticks` frozen.

## 9. Bots and debug

- `bot::RunnerBot { skill }` reads the same feasibility model and the visible rows, picks a
  lane, and issues commands with skill-dependent reaction delay and error rate (seeded).
  It must also exhibit contact (it does not avoid other runners perfectly).
- `bot::GhostBot` places a valid item every few seconds, preferring lures early and hay/beam
  later.
- Debug hooks on `World`: `force_eliminate(id)`, `set_slow_motion` is app-side,
  `debug_six_in_one_lane(seed)` and `debug_one_runner_five_ghosts(seed)` constructors.

### Required core tests (the ones that are core-level)

solo runs with no network; final feather ends solo; 2/4/6-player worlds share one course;
all six in one lane stay simulated (no NaN, no teleport, none removed) for 30 s; entering a
crowded lane displaces without a front-of-queue teleport; collapse in front of a follower
forces a hop and blocks the follower's next jump until landing, while a non-contacting
runner in another lane is unaffected; a body hitting a damaging obstacle across many ticks
loses one feather; crash then significant impact 2 s later → protection unchanged (0.2);
resting contact does not refresh; the first obstacle hit while protected passes through
with no feather loss and the second costs one (0.2); final feather in multi → Ghost in the same tick; last runner keeps
running against ghosts; two ghosts targeting one cell → one accepted, one `Occupied`, only
the accepted charged; wall attempt → `Unsafe`/`Density`, no energy spent; contested coin →
one award; duplicate `PlaceRequest` → no second hazard or charge; a 30-minute solo
simulation (headless, bot-driven) keeps rows bounded, the course feasible and positions
finite.

## 10. flock-session

Background: Stampede's multiplayer notes and Twang Thang's Bluetooth notes. **Fork Stampede's `race-session`**; keep its
class-(a) files near verbatim, rewrite class (c). Do not port: migration (all of it),
Keep invites, track transfer, `authority.rs`, `checkpoint.rs`, race-authority.

- `APP_TAG = "gameflock"` (= `account::APP_ID`; namespaces room codes, ALPN, LAN beacon,
  attestation `d` tag). `PROTO_VERSION = 3` (0.2: `Hello`/`SeatView` carry `hat`,
  `RunnerNet` a power-up/free-hit `flags` byte, new events, `Runner` fields). A Hello of
  another version is refused with the reason even when it cannot be decoded. Seats: 6
  (`MAX_SEATS`), humans + host-added bots.
- Bots are `PlayerSpec.bot` seats simulated inside `World` on the host; lobby shows them
  as seats with a bot badge; host can add/remove bots in the lobby.
- **Driver trait** (one run screen for all modes):
  ```rust
  trait RunDriver {
      fn update(&mut self, now_ms: u64, input: InputFrame);
      fn place(&mut self, tool: Tool, lane: u8, row: u32) -> u32;   // returns req id
      fn view(&self) -> Option<&FlockView>;
      fn results(&self) -> Option<&ResultsView>;
      fn set_paused(&mut self, p: bool);                          // LocalRun only; no-op online
  }
  ```
  `FlockView { world: World /* display shadow, never stepped by the app */, local: PlayerId,
  events: Vec<Event> /* this frame */, place_results: Vec<(u32, PlaceResult)>, phase, hud
  info }`. `LocalRun` (solo and Flock Practice; fixed step from frame times, 100 ms / 12-tick
  cap, same as `LocalRace`) and `ClientSession` (online, host included via loopback seat).
- Input: `InputFrame` (2 bytes, command latch by `cmd_seq`) per tick, five newest per
  datagram, Stampede's `Human::feed` repeat-on-starve pattern (harmless with the latch).
- Placements: reliable `ToHost::Place { req, tool, lane, row }`; host calls
  `World::place`, replies reliable `ToClient::PlaceResult { req, result }`; dedupe
  `(player, req)` (core already idempotent). Accepted placements reach everyone as events.
- Course: host streams generated rows reliably (`ToClient::Rows`, chunked ≤1024 B) ahead of
  need; clients build a remote `Course` from rows + events, never run the generator.
- Snapshot (lossy, 30 Hz, `budget`ed ≤1024 B with a worst-case 6-player test): tick, ack
  seq, own full runner, others compact (pos, vel, loco tag + timer, feathers, protected,
  role, energy for ghosts), anchor, v_course. Coins taken / placements / feathers lost /
  role changes are reliable events.
- Prediction: own runner via `flock_core::predict_runner` with Stampede's replay of unacked
  inputs and eased correction; contacts and damage are host-only (expect visible
  corrections on bumps — ease them; never snap under 10 m). Others interpolated ~100 ms
  behind (Stampede's adaptive delay). Ghost UI is not predicted except the local preview.
- Disconnect: `Leave` or 6 s silence → `World::disconnect`. Host leaves → clients get
  `Closed` → "Host left" → offer back to multiplayer menu / solo. `NotEnoughPlayers` ends.
- Results: `ResultsView` rows from `World::standings()` + names/colours; rematch via
  Stampede's `RematchReady`, `round + 1` rotates the grid.
- Transports: QUIC online (room code), Nearby LAN, and **Bluetooth** as a Fray virtual
  link (Twang's `HostLink` two-door merge). For a BLE (`PathKind::Virtual`) host, the client
  skips the pre-join host attestation check and verifies the host's persona from its lobby
  seat after acceptance; BLE joiners need the host's **Accept** (lobby prompt naming the
  verified persona). Signet persona attestation is required from every joiner on every
  path. This admission code gets an independent review.
- Keep `flock-headless` (`--smoke` loopback/localhost N-client check, `--lan` bounded LAN
  host with bots) for device testing from the laptop.

## 11. flock-render

Fork of race-render (same API style: plain data in, pixels out, no dependency on
flock-core). Changes:

- **Nostrich without jockey**: drop rider, saddle, laptop, cake, rugs, printer, laser
  extras from the default draw (code may stay if unused). Add pose inputs for floppiness:
  `squash` (wind-up), `neck_sway` (x, z), `body_roll`, `body_pitch`, `tumble` (full-body roll
  angle), `collapsed` 0..1 (belly-flop slide pose, legs out behind), `wing_flap` 0..1,
  `scramble` 0..1 (leg flailing), `stars` 0..1 (protection: 4–5 stars circling the head),
  `ghost` 0..1 (translucent, pale, floating, wavy tail instead of legs), `outline`
  (local-player rim/outline colour), `colour` per player (6 distinct body/accent sets that
  also differ by an accessory shape for colour-blind readability).
- **Farm course**: straight three-lane dirt track with lane ruts, wooden edge fences,
  grass verges, barns, silos, windmill, haystacks, trees, crops in the distance, sky.
  The world is a seamless repeating tile (e.g. 240 m) baked once; the app draws dynamic
  things relative to a floating origin and wraps the camera within the tile.
- **Props** (instanced each frame, like `drill_props`): `HayBale`, `Beam` (one-lane gate
  frame or three-lane overhead barn beam), `Block` (barn wall section / tractor / water
  trough variants), `Mud` (decal with sheen), coins (existing), plus `alpha` for
  reservations/previews and `drop` 0..1 for the architect drop-in animation, and a
  `danger` tint for invalid previews.
- **Ghost layer**: placement grid overlay (lane × row cells inside the band) with valid /
  invalid / reserved states; drawn in 3D on the ground.
- Particles: dust from feet, feather puffs on a crash (feathers fly off), mud splats,
  coin sparkles, landing dust, ghost "poof" on elimination.
- Cameras are the app's: runner camera behind and above the pack anchor (portrait,
  fov ~62°, eye ~(0, 7.6, anchor−9.0), target ~(0, 0, anchor+10.5)), easing only; ghost
  camera higher and diagonal (~(−5, 15, anchor−3) → (0, 0, anchor+40)) showing the band.

## 12. flock-audio

race-audio fork, all synthesised: footsteps (fast patter), jump whoosh, flap, squawk
(several pitches, per player), bump/boing, crash + feather puff, coin chime, mud squelch,
ghost whoosh, placement drop thud, countdown beeps, a light farmyard-jaunty loop. Volumes
from Profile.

## 13. App

Screens (portrait, Stampede shell style, Twang-quality polish):

- **Splash** → **Home**: big title art, the Nostrich idling, buttons: *Play solo* (straight
  into a run), *Flock practice* (you + 1–5 bots, offline, the multiplayer rules incl. ghost
  mode), *Play with friends* (Signet gate first), *Settings*. Shows local best.
- **Run screen** (all modes): `RunDriver`, presenter (World → FrameScene with `Flop` per
  runner), runner HUD (feathers, coins, distance or time left, living count in multi,
  protection ring), swipe input (one gesture = one command; 8-way with angle/distance
  thresholds, diagonals only within ±25° of the 45° axes), keyboard (arrows/WASD, combo
  window for diagonals, Space=jump), pause (solo/practice only; auto on focus loss), held
  input cleared on focus loss. Countdown 3-2-1-GO.
- **Ghost mode**: camera transition, one-line instruction, toolbar (4 large tool buttons
  with cost, energy bar), tap a lit cell → preview → tap again (or tap directly on a valid
  cell) to place; reject feedback text + shake; reservation shown; surviving runners
  shown. Keyboard: 1–4 tools, arrows move cursor, Enter places.
- **Solo game over**: distance (primary), coins, best (new-best celebration), *Run again*
  (instant), *Home*.
- **Multiplayer**: *Host*, *Join with code*, *Nearby*, *Bluetooth* cards → **Lobby** (seat
  list with colour + accessory, name, ready, bots add/remove for host, start when ≥2) →
  run → **Results table** descended from Twang Thang's:
  place, player, survived time, feathers, coins, architect placements (unranked) →
  *Rematch* / *Leave*.
- **Settings**: name, music/effects, quality, reduced shake, left-handed toolbar, account
  (sign out), debug section (hidden behind 7 taps on version: seed, slow-mo, collision
  boxes, protection status, pack bounds, placement cells, force-eliminate, six-in-one-lane
  and one-runner/five-ghosts tests).
- Profile adds: best distance, best coins, colour choice, reduced_shake.

## 14. Daily run

One track per UTC day for every player, with no server of ours. The seed comes from the
Bitcoin blockchain. A public block explorer is only the transport: the phone checks
everything itself. Code: the `blockseed` crate (github.com/forgesworn/blockseed), with the domain
`clusterflock-daily-v1` and User-Agent `clusterflock` set in `app/src/daily.rs`. The run itself is
`MatchConfig::daily(seed)`
(solo rules, generator version `GENERATOR_VERSION`).

### 14.1 The rule

The rule is blockseed's (`docs/RULE.md` in that repository is normative); in short:

- Day `D = floor(t / 86400)` in UTC, with `t` a Unix time; midnight `M = D * 86400`.
- `MTP(B)` is the median of the timestamps of the 11 blocks before `B`.
- The **anchor** `A` is the lowest-height block with `MTP(A) >= M - 3 h`. The **daily block**
  of day `D` is the lowest-height block at or above `A` whose own timestamp is `>= M`. On every
  ordinary day this is simply the first block stamped at or after midnight; the anchor makes the
  answer the same whenever it is asked, even when a block near midnight is stamped out of order.
- The **current daily** is the one for the latest day that has such a block with at least three
  linked blocks on top of it. Until then yesterday's stays live. The daily's public number is
  that block's **height**.
- "Now" is the timestamp of the tip block, never the phone clock.

### 14.2 Verification (the explorer is untrusted)

For every header used: parse the 80 bytes; block hash = double SHA-256 of the header; require
`hash <=` the target from `bits` and the target `<=` a difficulty floor; require each header's
previous-hash to equal the hash of the header before it across the whole verified segment, and
`bits` to equal the predecessor's except at a retarget height (multiple of 2016), where it may
change by at most a factor of 4. The three confirmations are linked, checked descendants, not a
height claim. The chosen block's hash and the tip height are cross-checked on a second explorer;
an unreachable second explorer is tolerated and recorded (see blockseed's README for the trust
model).

### 14.3 Seed

`seed = u64::from_be_bytes(SHA-256(b"clusterflock-daily-v1" || block_hash)[0..8])`, where
`block_hash` is the 32 bytes in header order (the double-SHA-256 output, not the reversed
display form). Block 974869 (`0000000000000000000123e050878a33503aca65df86ca668fca2ec493b46f79`
displayed) gives `0x4b9cc6b9ba92b55f`.

### 14.4 Fetch

Blocking, on a background thread, rustls with the bundled webpki roots (ureq 2.x), a plain
`User-Agent: clusterflock`, 8 s per request, a bounded request budget (blockseed's `MAX_REQUESTS`). Esplora REST, primary
`https://mempool.space/api`, cross-check `https://blockstream.info/api`:
`GET /blocks/tip/height`, `GET /block-height/:height` (hash, plain text),
`GET /block/:hash/header` (80-byte header, hex), and on mempool.space only
`GET /v1/mining/blocks/timestamp/:ts` (JSON `{height, hash, timestamp}`, the block nearest
at or before `ts`; used as a hint only). Procedure: tip height, tip hash, tip header; `D`
from its timestamp; hint for `D * 86400`; the window ends at `min(hint + 7, tip)` and goes
back 15 headers by following each header's previous hash; verify; apply the rule; if no
block of `D` qualifies, repeat for `D - 1`. A disagreement between the explorers about the
chosen height's hash is an error; an unreachable second explorer is tolerated and flagged.

### 14.5 App

- Profile caches `{ day, height, hash_hex, seed, fetched_unix }`. Home refetches when there
  is no cache or it is older than 20 minutes; never during a run, never on the UI thread.
  A cached older daily stays playable and is labelled with its block.
- Unlimited attempts. `daily_best { height, metres, coins, attempts }` is kept for the
  current block and reset when the block changes. Daily runs never touch `best_distance_m`.
- At run start the app logs the block height and the hex `course_fingerprint` of the first
  600 rows, so two phones can be compared in logcat.
- **Wallet**: `Profile.wallet` is every coin ever banked. A run banks its coins once: when
  it finishes and when the player quits from the pause menu.
- **Wardrobe**: six free colours, hats from `flock_render::HATS` with prices
  (150, 300, 500, 800, 1200, 2000, 3000 coins in ascending flashiness). Everyone owns "no
  hat" and the hat their first-launch colour wore. `Profile.hat` 255 is the colour's default.
  The equipped hat goes in the online `Hello` and is fixed for that session.
