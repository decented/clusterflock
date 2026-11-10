# flock-core

Pure gameplay simulation for **Cluster Flock**: a three-lane endless runner
with floppy Nostriches, solo, and 2–6 player host-authoritative multiplayer
where eliminated runners become obstacle-placing ghosts. No I/O, no
rendering, no threads, no `unsafe`. Everything that is state is
`Serialize + Deserialize` (the whole `World` included).

Contract: `docs/SPEC.md` §0–9. This README is the API guide for the
session, render and app crates.

Build (workspace member; run from the repo root):

```
cargo test -p flock-core
cargo run -p flock-core --example ascii --release -- multi 6 [seed] [quiet]
cargo run -p flock-core --example ascii --release -- solo [skill 0..1] [seed] [quiet]
cargo run -p flock-core --example ascii --release -- sweep multi 6 [first_seed] [count]
cargo run -p flock-core --example ascii --release -- sweep solo [skill] [first_seed] [count]
cargo run -p flock-core --example measure --release -- [idle_seeds 20] [multi_seeds 24]
```

`measure` prints the two balance numbers: the distance a **no-input** solo
runner reaches (seeds 1..N) and the round length of six normal-skill bots
(seeds 1..M). `VERBOSE=1` logs the idle runner's crashes.

## Coordinates and clock

- Metres. `x` lateral (+ = runner's right), `y` up (0 = ground), `z` forward
  (`f64`; everything else `f32`). **The renderer's right for yaw 0 is −X, so
  the presenter maps sim `x` to render `-x`** (do it in one place).
- Lanes 0, 1, 2 (left → right), centre `x = (lane − 1) · lane_width`.
  Fences at `±fence_x`.
- `TICK_HZ = 60`, `DT = 1/60`. All timers are ticks (`Tick = u64`). Pausing =
  not calling `step`. `MAX_PLAYERS = 6`, `PlayerId(u8)` 0..=5.

## World lifecycle

```rust
let mut w = World::new(MatchConfig::solo(seed));          // or MatchConfig::multi(seed, n, bot_skill)
loop {
    w.step(&[(PlayerId(0), Command::Jump)]);              // one 60 Hz tick
    for e in w.events() { /* this tick's events */ }
    if w.phase() == Phase::Over { break; }
}
```

- `MatchConfig { seed, mode: Mode::{Solo, Multi}, players: Vec<PlayerSpec { id, name, colour, bot: Option<BotSkill>, hat }>, tuning, round }`.
  Invalid/duplicate ids are dropped, at most 6 kept; solo keeps the first.
  `PlayerSpec::new(id, name, colour, bot)` gives `hat = PlayerSpec::DEFAULT_HAT` (255 = the
  default hat for the colour; cosmetic only). `MatchConfig::daily(seed)` = solo rules on a
  published seed (see "Determinism").
- Phases `Countdown` (`countdown` s, nobody moves, commands ignored) → `Running` → `Over`.
  `step` after `Over` is a no-op (tick does not advance).
- Ends: solo — final feather (`EndReason::FinalFeather`). Multi — no runners
  left (`NoRunnersLeft`), `mp_cap` elapsed (`TimeCap`), fewer than 2
  non-disconnected players (`NotEnoughPlayers`). `abort()` → `Aborted`
  (use for "host left"); `disconnect` in solo also aborts.
- Start grid: front row z 0 (lanes 1, 0, 2), back row z −3; slot order
  rotated by `(round + seed) % n`.
- **Event timing rule:** `events()` holds the events of the last `step`.
  Anything done between steps (`place`, `disconnect`, `force_eliminate`,
  `abort`) takes effect immediately but its events appear in the *next*
  step's list.
- Bots: players with `bot: Some(skill)` are driven inside `step` (runner bot
  while running, ghost bot after elimination), seeded from the match seed —
  solo/practice need nothing else.

Accessors: `tick() phase() end_reason() mode() config() tuning()
anchor_z() v_course() running_ticks() running_secs() players() player(id)
runners() runner(id) living_count() ghost_count() leader_z() trailing_z()
ghost_energy(id) course() events() standings() solo_result()
placement_band() plausible_speed(z) next_ctx()`.

- `players() -> &[Player { spec, role, runner, placements, survived_ticks, eliminated_at, disconnected }]`.
  `runner` is kept (frozen) after elimination so results can show it.
- `runners()` iterates living runner bodies only; `runner(id)` is `None` for ghosts.
- `Role::{Runner, Ghost { energy, placed }, Disconnected}`.
- `solo_result()` is `Some(SoloResult { distance_m, coins })` in solo at any
  phase (live HUD values; final once `Over`), `None` in multi.
- `standings() -> Vec<Standing { player, place, alive, survived_ticks, feathers, coins, placements, disconnected }>`
  sorted by place then id. Alive above eliminated; alive by feathers then
  coins; eliminated by survival ticks then coins; disconnected last (same
  keys among themselves). Ties share a place (competition ranking 1, 1, 3).

## Runners

`Runner { id, pos: Pos{x,y,z}, vel: Vel{x, y, z_off}, loco, feathers, protected_until, coins,
settle_lane, target_lane, buffered, contacts_prev (bitset by PlayerId), touched_prev,
alive_ticks, stats, boost, knock, in_mud, incidents, protect_since, long_logged, speed_bonus,
free_hit, swagger_obj, magnet_until, double_until, shield_until }`

- `free_hit`: the current protection episode's one free obstacle hit is still
  available (HUD). `swagger_obj`: the obstacle being passed through on it (host-only).
- Power-up timers (ticks, `0` = never): active while `tick < *_until`; helpers
  `magnet(now) doubler(now) shield(now)`. `stats` gained `swaggers` and `mud_stuck`.

- `vel.z_off` is forward speed minus `v_course`; `forward_speed(v_course)` gives the total.
- `speed_bonus` (m/s): personal coin speed. Each collected coin adds
  `coin_speed` up to `coin_speed_cap`; a crash that costs a feather halves it.
  It is added to `vel.z_off` by the shared integration (so `predict_runner`
  includes it bit for bit), in solo and multi alike (solo: the runner outruns
  `v_course`, the camera follows; multi: you edge ahead of the pack within the
  band, the pack pull working against it). `plausible_speed` and the feasibility
  DP include the cap.
- `Loco::{Running, WindUp{kind: Command, until}, Airborne{voluntary, can_act: false},
  Collapsed{until}, Tumbling{until, spin_dir}, Recovering{until}}`. `loco.is_low()`
  for Collapsed/Tumbling (collision height `height_low`).
- Only `Running` is controllable; other commands are buffered (one, newest
  wins, `input_buffer` s) and fire on the first controllable tick.
- **Protection (0.2 rule: "allow one free hit, the next one counts"):**
  `protected_until: Option<Tick>`. A feather-losing crash (or a shield break) starts an
  episode of `protect_time` (4 s) with `free_hit = true`. The first damaging-obstacle
  hit of the episode is free: no feather, no tumble, knockback, speed or control loss
  (coin bonus kept, block walls are passed through) — `Event::Swagger { player, obj }`.
  The next damaging hit in the same episode ends it at once and is an ordinary
  vulnerable crash (shield check, feather, tumble, a new episode with a new free hit).
  Nothing extends an episode (runner bumps never touch protection or the free hit); it
  ends when the timer has passed **and** the runner is `Running` again (`ProtectionEnd`).
  `vulnerable() == protected_until.is_none()`. `Event::ProtectionRefresh` is gone.
- **Mud:** a runner `in_mud` cannot start a jump: `Jump` is refused and dropped
  (`Event::MudStuck`), `JumpLeft/Right` only change lane. Ducks and lane changes work.
  Part of the shared integration, so `predict_runner` refuses it the same way.
- **Power-ups** (`ObjKind::Power { kind: PowerKind::{Magnet, Doubler, Shield} }`):
  collected like a coin, first runner takes it, gone for everyone
  (`Event::PowerTaken { obj, by, kind }`). Magnet `magnet_s` 8 s: also collects coins
  within `magnet_reach_lanes` (1.15) lane widths across and `magnet_reach_z` (2.5 m)
  along, any height. Doubler `doubler_s` 10 s: each coin counts 2 (speed bonus per coin
  unchanged). Shield, held up to `shield_s` 20 s: the next crash that would cost a
  feather costs none but still tumbles and starts a normal episode
  (`Event::ShieldBroken`). While protected with the free hit available, the free hit
  comes first; a second hit in an episode is a vulnerable crash, so a held shield takes it.
- Helpers: `height(t) controllable() protected() vulnerable() grounded() forward_speed(v)`.

## Course (streamable)

- `Row { row_index: u32, cells: [Cell; 3], coins_taken: u16, release: u32 }` — 13 bytes of
  data, `Copy`, serde (`release` defaults to 0). `z = row_index · row_spacing`.
  `release` is the tick the row's barrels started rolling (0 = not yet).
  `coins_taken` bit 15 = the row's power-up was collected (`power_taken()`);
  `power()` gives `(lane, kind)` of the row's power-up (at most one per row).
- `Cell(u8)`: bits 0-2 solid (0 none, 1 hay, 2 beam, 3 beam-all, 4 block,
  5 barrel rolling toward the runners, 6 barrel rolling away),
  bit 3 mud, bits 4-5 coins (0 none, 1 line of 4, 2 arc of 5 over hay),
  bits 6-7 power-up (0 none, 1 magnet, 2 doubler, 3 shield).
  Use `solid() mud() coins() power()` / `with_*`. A beam-all sets `BeamAll` in all
  three cells and is one object.
- `ObjId(u32)`: generated `row·32 + lane·8 + slot` (slot 0 solid, 1 mud,
  2..=6 coins, 7 power-up); ghost placements sequential from `0x8000_0000`
  (`is_architect()`); a ghost coin line uses five consecutive ids.
- Objects are derived views: `Obj { id, kind: ObjKind::{Hay, Beam{span}, Block, Mud, Coin, Barrel, Power{kind}},
  lane, row, x, z, y (bottom / coin centre), top, half_w, half_d, by, solid_at, dir, vz, roll, released }`
  with `damage()` (`Crash`/`Slow`/`None`), `is_live(now)` (pending
  placements are not), `z0() z1() lanes() covers_lane()`.
- Queries: `rows() row(i) rows_in(z0, z1) first_index() last_index() len()
  objects_in(z0, z1) -> Vec<Obj> row_objects(i) objects() obj(id) placed()
  row_z(i) row_at(z) lane_x(l) is_feasible(speed) frontier()`.
- **Beams** (renderer: draw exactly what collides): `Obj.y` is the underside
  (`hazards.beam_underside` 1.15 m) and `Obj.top` the top (`hazards.beam_top()` =
  underside + `hazards.beam_thickness` 0.8 = 1.95 m); `half_w` / `half_d` the
  footprint (an all-lanes beam spans fence to fence). The jump apex is ~1.45 m,
  so no jump (any speed, any coin bonus) clears the top, and a crouched /
  rolling body (0.55 m) passes under with a >= 0.43 s window. Hay, block and
  barrel: `Obj.y = 0`, `Obj.top` the height.
- **Barrels** (`ObjKind::Barrel`, `Damage::Crash`, one lane, jumpable like hay):
  a lying cylinder (`barrel_radius` 0.35 = top height / 2, `barrel_width` 1.2
  across the lane) that **rolls along its lane** while hay stays still.
  `Obj.dir` is -1 (rolls toward the runners, `barrel_speed_toward` 4 m/s) or +1
  (away, `barrel_speed_away` 3 m/s); `Obj.vz` the current z velocity (0 before
  release and after the roll ends); `Obj.roll` the roll angle in radians about
  the x axis (`travelled / radius`, positive = rolled toward +z: rotate the mesh
  by it); `Obj.released` whether it has started. `Obj.z` is already the current z.
  - **z(tick)** is a pure function of the row and the course clock:
    `z = home + dir · speed · min((clock − row.release)/60, barrel_travel_s)`
    (`home = row_z`, before `release` it sits at `home`; it stops and rests after
    `barrel_travel_s` = 6 s, i.e. 24 m / 18 m). `Course::barrel_state(row, dir)`
    gives `(z, vz, roll, released)`.
  - **Release:** the host starts a row's barrels when the *leading* living runner
    is within `barrel_trigger_m` (24 m) of its home z (`World::step`, first thing),
    sets `Row.release` and emits `Event::BarrelReleased { row, tick }`. A remote
    client calls `course.release_barrel(row, tick)` on that event (idempotent) and
    **`course.set_clock(tick)` every tick** (the world does it itself; `objects_in`
    and friends then return barrels at the right z). Test
    `host_releases_barrels_when_...` checks host and remote objects are equal on
    every tick; `barrel_position_is_a_pure_function_...` checks the formula.
  - A crash does not stop the barrel (heavy: it keeps rolling) and costs each
    runner one incident per barrel. Trip + somersault like hay.
  - **Feasibility:** `Course::barrel_extent(home, dir, v)` turns a barrel into the
    stationary-equivalent hay the DP uses: the runner meets it after closing the
    trigger distance, closing speed `v − vz`, centre shifted by `vz·trigger/(v−vz)`,
    half-depth scaled by `v/(v−vz)` plus a 1 m margin. Toward barrels get a
    *wider* time window (they are narrower in time), away barrels a shorter one
    (3 m/s keeps >= 0.18 s). Measured real jump windows: 0.45 s toward and 0.47 s
    away at 13 m/s and at 22 m/s + 3. The runner bot times its jump from the
    barrel's current z and velocity.
  - Generation: patterns 8 (toward) and 9 (away), min difficulty 0.35, phased in
    by `q` like the other non-single patterns (so never during the gentle
    multiplayer opening), at least two rows after the previous solid. Not a
    ghost tool.
- **Remote/client course:** `Course::new_remote(&tuning)` never generates.
  Rebuild it from host data: `insert_row(index, row)` for streamed rows,
  `remove_coin(id)` on `Event::CoinTaken { obj, by }`, `take_power(id)` on
  `Event::PowerTaken { obj, .. }`,
  `release_barrel(row, tick)` on `Event::BarrelReleased { row, tick }` and
  `set_clock(tick)` every tick (barrel positions; `World::step` sets the clock to the
  tick it simulates, i.e. `world.tick() - 1` after a step; flock-session's
  `shadow::build` and client do both),
  `add_placement(by, obj, tool, lane, row, solid_at)` on
  `Event::Placed { by, obj, tool, lane, row, solid_at }` (idempotent),
  `release_before(first_index)` to drop old rows. `Event::Materialised { obj }`
  marks the tick a placement became solid (also derivable from `solid_at`);
  one event per object, so a coin lure emits five.
  Clients must not regenerate rows from the seed in multiplayer: generation
  there depends on host state (ghost count, time, placements). Test `remote_course_follows_host_events`
  checks the remote copy matches the host exactly.

## Ghost placement

- `PlaceRequest { player, req (per-player sequence), tool: Tool::{Coins, Mud, Hay, Beam}, lane, row }`.
- `World::place(req) -> PlaceResult = Result<Accepted { obj, cost }, Reject>`,
  atomic, validated in order: `NotGhost` (also unknown id, not running),
  `Duplicate`, `OutsideBand` (also lane > 2, row not generated), `TooClose`,
  `Occupied`, `Density`, `Unsafe`, `NoEnergy`. Rejections spend nothing.
- Retries: a repeated `(player, req)` still in the 64-entry log returns the
  **original result verbatim** (Ok or Err) with no second hazard/charge;
  a `req` older than the whole log returns `Err(Duplicate)`.
- `check_placement(player, tool, lane, row) -> Result<cost, Reject>` — same
  checks, no side effects (UI preview). `placement_band() -> RowRange { first, last }`
  (inclusive; `is_empty()`, `contains()`, `iter()`).
- Accepted objects exist at once as reservations (`solid_at = now +
  materialise_s`), collide only once live. Coins = a line of 5 lures, never
  blocking. Density counts architect solids only (1 per row, 3 per 4 rows).
- `Unsafe` = the feasibility DP over the whole held window (including
  pending placements) fails with the candidate added. Hay, beam **and mud** run it
  (mud right before a jump-only obstacle can kill a lane).

## Client prediction and input

- `predict_runner(&mut runner, &course, &tuning, ctx: PredictCtx { tick, v_course, anchor_z, pack }, cmd)`
  runs exactly the per-runner code `World::step` uses (command + buffer/wind-up
  rules, loco timers, lateral spring, forward speed incl. pack correction,
  gravity, fences, mud). Contacts, damage, coins and protection are
  host-only. `runner::runner_tick(.., &[Command], &mut events)` is the same with
  several commands and the events. `World::next_ctx()` returns the ctx the
  next `step` will use (send it, or `v_course`/`anchor_z`, in snapshots).
  Test `predict_runner_matches_world_step_bit_for_bit` holds this equality.
- `InputFrame { seq: u8, cmd: Option<Command> }`, `pack() -> [u8; 2]`
  (`[seq, code]`, code 0 = none, 1..=8 = `Left, Right, Jump, Down, JumpLeft,
  JumpRight, DiveLeft, DiveRight`), `unpack([u8; 2]) -> Option<InputFrame>`
  (`None` for a bad code). Clients bump `seq` per new command and resend the
  frame every tick.
- `CmdLatch::feed(player, frame) -> Option<Command>` applies a frame only when
  its `seq` differs from the last one seen for that player; `feed_all(&[(id, frame)])`
  gives the `(PlayerId, Command)` list for `step`; `reset(player)` on rejoin.

## Flop (visual only, not networked)

`flop::Flop::new()`, then `flop.step(&runner, dt)` per rendered frame with the
(interpolated) body. Output fields: `neck_sway`, `neck_pitch`,
`neck_stretch` (≈0.8–1.25), `body_roll`, `body_pitch`, `squash` (y-scale;
scale x/z by `1/sqrt(squash)`), `wing_open` (0–1), `wing_flap` (angle),
`leg_scramble` (0–1), `leg_phase` (rad), `tumble_angle` (rad, roll about the
forward axis), `head_bob` (m). Signs: + sway/roll toward sim +x (flip with x
in the presenter), + pitch forward. Wind-up squash, landing thump, lurches
throwing the neck, frantic wings in the air (faster on involuntary hops),
somersault spin while tumbling, stand-up wobble. Deterministic.

## Bots

- `bot::RunnerBot::new(skill, seed, id)`, `think(&World, id) -> Option<Command>`:
  plans lanes with the same feasibility transition over 16 rows (Pareto
  search, robust model first), times jumps/ducks from the same windows,
  with skill-dependent reaction delay, timing noise and mistakes (wrong
  action or badly timed, 22 % → 0.4 % of obstacles). It mildly avoids a
  runner just ahead at high skill, otherwise ignores other runners, so
  contact happens.
- `bot::GhostBot::new(skill, seed, id)`, `think(&World, id) -> Option<PlaceRequest>`:
  every 2–4.5 s a valid placement; lures early, hay/beam later; prefers
  lanes runners are heading for.

## Determinism (daily runs)

- In solo, row `i` is a pure function of `(seed, i, Tuning, GENERATOR_VERSION)`:
  per-row difficulty `1 − e^(−max(row_z − run_in, solo_difficulty_floor_m) / solo_difficulty_ramp_m)`
  (the floor, 175 m, reproduces 0.1.x's opening, which evaluated its whole first
  batch at that distance), the speed model is a function of z only, generation is
  sequential in row order, so it does not matter when or in what batches rows are
  generated or what the runner does. (Multiplayer rows depend on host time, ghosts and
  placements, by SPEC, and are streamed.)
- RNG: `det::SplitMix64` (pinned; reference vector test) with pinned draws
  (`unit()` 24-bit, `below(n)` multiply-shift). The `rand` crate is no longer in the
  generation path (bots still use `rand_pcg`, host-side only).
- No libm in anything that feeds the sim or generation: `det::exp`, `det::sin` are
  built from IEEE `+ − × ÷`, `round` and bit casts (solo speed curve, difficulty,
  boost/knock decay, mud wobble). Found and replaced: `exp` in `Tuning::solo_speed`,
  the DP speed model, the difficulty formula and the boost/knock decay; `sin` in the
  mud wobble; `powi(2)` in the coin pickup distance. `sqrt`, `floor`, `round` are
  exactly specified and stay. `flop.rs` (visual only) keeps libm.
- `course_rows(cfg, n)` / `course_fingerprint(cfg, n) -> [u8; 32]` (SHA-256 of
  `row_index` u32 LE + the three cell bytes per row; `det::hex` for logging).
  `tests/rules_02.rs` pins the hash of `MatchConfig::daily(0x0123456789abcdef)`, 600
  rows, and checks an idle runner and a playing bot see exactly those rows. Bump
  `GENERATOR_VERSION` (now 3) with any change that alters rows.

## Debug hooks

`World::debug_six_in_one_lane(seed)` (6 non-bot runners queued in lane 1,
empty course), `World::debug_one_runner_five_ghosts(seed)` (P0 human vs
five ghost bots), `force_eliminate(id)`, `debug_runner_mut(id)`,
`debug_course_mut()`, `debug_set_energy(id, e)`, `Tuning.debug_empty_course`.
Slow motion is app-side (step less often).

## Tuning

`Tuning` holds every SPEC §3 number, `#[serde(default)]`
so a JSON file only needs overrides (`Tuning::from_json`). Additions:
`lane_change_s` 0.32 (DP lane-change time; the spring measures 0.27 s to
within 0.3 m), `feas_reaction_s` 0.15, `boost_tau` 0.6, `knock_tau` 0.35,
`long_protection_log_s` 8, `solo_difficulty_ramp_m` 1500 / `solo_difficulty_floor_m` 175,
power-ups `powerup_gap_m` 240 / `mp_powerup_gap_m` 160 / `powerup_jitter` 0.35 /
`powerup_weights` [40, 35, 25] / `magnet_s` 8 / `magnet_reach_lanes` 1.15 /
`magnet_reach_z` 2.5 / `doubler_s` 10 / `shield_s` 20, hazard geometry incl. `beam_width`, `block_width`,
`mud_width`, `lure_spacing`, and `debug_empty_course`. Play-test additions:
`torso_height` 1.1, `coin_speed` 0.05, `coin_speed_cap` 3.0, `plausible_boost_frac` 0.36 (+ `plausible_margin()`) and, in
`hazards`, `beam_thickness` 0.8 (+ `beam_top()`), `barrel_radius` 0.35,
`barrel_width` 1.2, `barrel_speed_toward` 4, `barrel_speed_away` 3,
`barrel_trigger_m` 24, `barrel_travel_s` 6.

**Defaults retuned for human play** (these differ from SPEC §3; everything
else is the SPEC value). Measured results are under "Feel notes".

| field | SPEC | now | why |
|---|---|---|---|
| `hazards.beam_underside` | 1.0 | 1.15 | beams read as impassable by jumping; beam collision top = underside + `beam_thickness` 0.8 = 1.95 m (apex 1.45 m) |
| `jump_fwd_boost` | 2 | 6 | a jump carries you ~3.6 m forward (boost decays with `boost_tau` 0.5 -> 0.6): through the pack |
| `collapse_fwd_loss` | 3 | 6 | a crouch drops you ~3.6 m back; `diag_dive_back_bias` -1.5 -> -3 |
| `pack_k` | 0.6 | 0.5 | slightly gentler pull to the pack anchor: ground won/lost by jumps/crouches persists ~2 s (0.45 let the six-in-one-lane session test lose a bird to the side lane) |
| `course_v0` | 10 | 13 | faster from the start |
| `hazards.hay_top` | 0.85 | 0.70 | shallower bale: with the jump below: window 0.147 s -> 0.247 s at 12 m/s (DP figure) |
| `gravity` | 30 | 28 | floatier, comic jump (airtime 0.64 s, apex 1.45 m < beam top 1.95 m) |
| `course_vmax` | 18 | 22 | faster solo top speed (+ up to 3 m/s coin bonus) |
| `course_ramp_m` | 1800 | 2500 | solo speed ramp |
| `mp_v_end` | 15 | 16 | multi speed ramps 13 -> 16 over `mp_cap` (18 made 6-bot rounds ~85 s) |
| `tumble_impulse` | 4.5 | 6.0 | a side bump staggers; only hard slams tumble |
| `protect_time` | 3.0 | 4.0 | one episode per feather, one free hit inside it (0.2; was 6.0 with refresh on every impact) |
| `ghost_energy_start` | 5 | 3 | ghosts start with less to throw |
| `ghost_energy_regen` | 0.9 | 0.3 | about one hay per ghost per 10 s |
| `density_per_4_rows` | 3 | 2 | fewer architect solids per stretch |

`jump_vy` stays 9.0 (the effective lift of the discrete integrator is
~8.77, so the apex is ~1.4 m). `bump_restitution` 0.35, `lateral_amax` 60,
`ghost_energy_max` 10 and the tool costs are unchanged.

## Provisional choices (SPEC silent)

- **Feasibility DP** (`feas`): state = earliest actionable z per lane, timing
  at max plausible speed. A lane move for row r starts after the previous
  row's centre and solids in crossed lanes and must end before row r's solids
  and before `row.z + row_spacing − max_half_depth − radius` (so it never
  silently crosses the next row). Jump/duck windows come from the exact
  discrete trajectory (hay clearance margin 0.05 m). `feas_reaction_s` is
  added to every action and lane-change duration so chained manoeuvres never
  need frame-perfect play.
- **Generator** keeps at least two live lanes in the DP frontier after each
  row (falls back to emptier candidates), so routes never funnel into one
  lane far ahead. Patterns unlock with difficulty: single hay/beam → beam
  across all lanes, two hay, mud+hay → block+hay → hay+beam → two blocks;
  coin trails zig-zag, coin arcs over hay. Solo difficulty
  `1 − e^(−d/1500)`; multi `0.2 + 0.7·t/cap`, density `1/(1 + 0.25·ghosts)`.
  The first `run_in` metres are empty. Multiplayer opens gently: `mp_run_in` (75 m) empty, then
  `mp_gentle_m` (230 m) of single-lane hay/beams at least 5 rows apart, then the full pool is phased
  in over `mp_ramp_m` (`GenCtx::gentle_until` / `ramp_m`).
- **Crash reactions:** hay → knockback + tumble + a somersault hop over the
  bale; beam → knockback + tumble (low, slides under); block → solid wall:
  the body is resolved out of it and steered (`target_lane`) to a free side
  lane (anti-stuck). One incident per (runner, object), max one feather per
  tick. Beam collision top is underside + `beam_thickness` (0.8) so it cannot be jumped. Barrel: like hay (knockback, tumble, somersault hop).
- **Runner contacts:** 3 positional iterations; impulse on the first.
  "Receiver" tumbles when its own impulse is lateral > `tumble_impulse` or
  pushes it forward from behind > `tumble_impulse`. Ramp hop needs the
  standing runner on the ground and not ahead of the low body (+0.15 m).
- Coins: collected if horizontal distance ≤ `body_radius + collect_radius`
  and the coin sphere overlaps the body's vertical extent.
- Pack correction (`pack_k` 0.35, band 9 m) clamp ±6 m/s, ×3 stronger beyond `pack_band`. Fence lurch
  at an edge = 5 m/s sideways kick. Mud wobble = 2.4 Hz lateral jiggle.
- Ghosts regenerate in the same tick they are created; new ghost bots wait
  0.5–1.5 s before their first placement.

## Feel notes (ascii runs)

Tuned for humans on a phone (see "Tuning" for the numbers). Measured with
`ascii -- sweep solo|multi ...` (one line per seed, median at the end;
`FLOCK_TUNING='{"jump_vy":9.5}'` overrides defaults without rebuilding).
Duration = `running_secs()` at the end (multi: time of the last elimination;
180 = reached the `mp_cap` cap).

| config | seeds | median | min | max |
|---|---|---|---|---|
| solo, max-skill bot | 1-60 | 309 s | 56 s | 1754 s |
| solo, normal-skill (0.55) bot | 1-60 | 70 s | 47 s | 215 s |
| multi, 6 normal bots | 1-24 | 103 s | 74 s | 175 s |
| multi, 6 normal bots (held out) | 100-123 | 114 s | 69 s | 139 s |

Multi contact per 6-bot round (seeds 1-24 / 100-123): bumps 10.8 / 8.9, hops
19.8 / 16.8, tumbles 37.8 / 35.8 (slams ~1.7 / 1.9). (Before the play-test
pass: max-skill solo 280 s, normal 94 s, 6-bot multi 124 / 129 s, 13 bumps.)

- **Max plausible speed (feasibility DP).** Solo: `v_course + coin_speed_cap +
  plausible_boost_frac * jump_fwd_boost` (+5.2 m/s at the defaults). A full
  jump boost on top (+9 m/s) made the generator too sparse (max-skill bot
  1057 s). Multi: only `plausible_boost_frac * jump_fwd_boost`, because the
  pull toward the pack anchor cancels a coin bonus at equilibrium. The result
  is very sensitive to `plausible_boost_frac` (0.25: 150 s, 0.36: 309 s,
  0.4: 620 s median for the max-skill bot), and so is contact: complex
  multi-lane patterns only survive the DP when it is not over-padded, and
  they are what force bots to share lanes. `bot` lane planning also has a
  small flock instinct (`HERD_PULL`) toward lanes with nearby runners.

- **Jump = forward, crouch = back.** A jump adds `jump_fwd_boost` 6 m/s
  decaying with `boost_tau` 0.6 s (~3.6 m); a crouch (Down) subtracts
  `collapse_fwd_loss` 6 m/s (~3.6 m back, `tests/spec.rs`
  `crouch_drops_a_bird_back_...`). Runner-runner contacts use the torso band
  `[y, y + torso_height]` (1.1 m, capped by the body height): an airborne bird
  whose feet are above another's torso passes over it. A bird that has just
  jumped and is still rising vaults through any non-collapsed body (no impulse,
  no damage; otherwise the jump's own impulse is handed to the bird in front
  and nobody moves), and once it comes down it is a plain contact again, so
  landing on a standing bird's back is a bump without damage. A jump from
  directly behind (contact distance .. 1.4 m) in the same lane ends ahead of
  the bird, and clears two stacked birds (`jump_from_behind_...`,
  `jump_clears_two_...`); from 2.5 m back you come down on its back instead.
  A crouched body is passed over by the follower via the ramp hop
  (`crouch_lets_the_bird_behind_pass_...`). `pack_k` 0.35 / `pack_band` 9 let
  that ground persist for several seconds.
- **Coins build speed**: see `Runner.speed_bonus`; equilibrium lead over the
  pack in multi is `bonus / pack_k` ~ 8.6 m at the cap (inside the band).
- Hay jump window (jump command to clear the bale; `tests/jump_window.rs`):
  0.258 s at 13 m/s and 0.32 s at 27.2 m/s (solo max plausible speed incl. coin cap and
  boost share) by the feasibility DP's conservative figure, and 0.433 s at 13 m/s
  / 0.450 s at 22 m/s + 3 m/s bonus measured in the real world (one jump
  command on every tick against a lone bale; the forward boost helps). Barrels:
  0.45 / 0.47 s toward, 0.47 / 0.45 s away at the same two speeds. A duck under
  a beam has a 0.43-0.57 s window (13 m/s ... 22 m/s + 3); a jump can never
  clear a beam (apex 1.45 m < beam collision top 1.95 m).
- Multi: crashes ~36 per round (6 bots); ghosts place ~60 items per round, so
  ghosts still matter but a round is not over by 90 s.
- Solo: a human beginner dies in about a minute (normal bot ~70 s). Speed
  starts at 13 m/s, reaches ~19 m/s at 3 min and approaches 22 m/s (+ up to
  3 m/s coin bonus).
- Eliminations still come mostly from hay/beam/barrel/block crashes. 0.2: a crash
  chain is no longer harmless (one free hit per episode, the next one counts).
  If rounds feel too short for humans, `ghost_energy_regen` and
  `protect_time` are the first knobs, `tumble_impulse` the next.
- 0.2 measurements (`--example measure`, seeds 1-20 / 1-24): a **no-input** solo
  runner reaches median 287 m (min 179, max 414; 0.1.x: 6120 m, half the seeds ran
  the 30-minute cap); six normal bots: median 90.4 s (min 52, max 180; 0.1.x: 102.6 s).
  `protect_time` barely moves the idle number (3 s: 287 m, 5 s: 326 m: the idle
  runner's hits are mostly > 4 s apart). `solo_difficulty_floor_m` 400 gives 221 m
  (difficulty 0.23 from the start: beam-all and two-hay patterns in the opening).

## Known gaps

- `Row` byte size not measured with bincode here (no dependency); the data is
  4 + 3 + 2 + 4 bytes (`release`).
- The bot is competent, not perfect; it ignores diagonal jumps.
- `predict_runner` is bit-exact with the host in the same build, and since 0.2 uses no
  libm, so arm64 and x86 should agree too (not yet verified on a device; snapshots
  correct any drift anyway). The golden course hash is only checked on x86 so far.
- The mud rule in the DP checks the jump window at the row's max speed and the
  "take off before / after the mud" conditions over `[v − plausible_margin, v]`;
  barrels use `barrel_extent` at the max speed only (bug-review M3 still open).
- With default `band_near_s` (3 s) > `min_reaction_s` (1.6 s), `TooClose`
  cannot fire; it matters only if the band is tuned closer.
- The hay jump window is generous (see "Feel notes"); if hay top or
  gravity are retuned again, `tests/jump_window.rs` guards >= 0.22 s at
  13 m/s and at top speed (22 m/s + coin bonus), >= 0.18 s for barrels.
- The multi ramp (`0.2 + 0.7 t/cap`, density `1/(1 + 0.25 ghosts)`) is constant in
  `world.rs`; solo difficulty is `solo_difficulty_ramp_m` / `solo_difficulty_floor_m`;
  speed (`course_*`) is the other solo knob.
