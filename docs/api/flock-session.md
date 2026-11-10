# flock-session and `app::online` — API for the screens

Everything a screen needs to run Solo, Flock Practice and online matches
(host, join by code, Nearby, Bluetooth), the lobby and the results. Two
layers:

* `flock_session` (crate `crates/flock-session`): the run driver, the views,
  the offline runs, and the protocol (host authority, client session).
* `crate::online` (app `app/src/online/mod.rs`): the one online session per
  process and the calls the multiplayer screens make.

Contract source: `docs/SPEC.md` §10 and §13. Sim types (`World`, `Event`,
`Tool`, `PlaceResult`, `PlayerId`, `InputFrame`, ...) are `flock_core`'s; see
`crates/flock-core/README.md`.

---

## 1. The run screen: `RunDriver`

One trait for every mode. The run screen owns nothing else.

```rust
use flock_session::{RunDriver, FlockView, ResultsView};
pub trait RunDriver {
    fn update(&mut self, now_ms: u64, input: InputFrame); // once per rendered frame
    fn place(&mut self, tool: Tool, lane: u8, row: u32) -> u32; // ghost placement; returns req id
    fn view(&self) -> Option<&FlockView>;                 // this frame
    fn results(&self) -> Option<&ResultsView>;            // Some once the match is over
    fn set_paused(&mut self, p: bool);                    // LocalRun only; no-op online
}
```

**Input.** Keep one `InputFrame { seq: u8, cmd: Option<Command> }` per run.
For each new gesture/key command: `seq = seq.wrapping_add(1); cmd = Some(c)`.
Hand the *same* frame to every `update` until the next command (repeats are
harmless: a command fires once per new `seq`). On focus loss just stop
bumping `seq`. Commands during the 3-2-1 countdown are ignored by the sim.

**`now_ms`**: any monotonic ms clock (frame time). Online it is ignored (the
session runs on its own clock).

### `FlockView` (read after `update`)

| field | meaning |
|---|---|
| `world: World` | What to draw. Offline: the real sim. Online: a display shadow built from host data — remote course (rows streamed by the host, coins/placements applied from events), your runner predicted (+ `correction`), others interpolated ~100 ms behind; roles, feathers, coins, energy from the host. `phase()`, `tick()`, `running_secs()`, `runners()`, `players()`, `course()`, `placement_band()`, `check_placement()`, `ghost_energy()` all work on it. **Never step it.** |
| `local: PlayerId` | You. |
| `events: Vec<Event>` | New since the previous `update`, each exactly once (sounds, FX, HUD flashes, `Go`, `BecameGhost`, `Placed`, `MatchOver`, ...). Online, your own motion events (wind-up, jump, land, lane change, fence bump, collapse, stand-up, mud) come from the prediction at once; everything else is the host's. |
| `place_results: Vec<(u32, PlaceResult)>` | Answers to your `place` calls since the previous `update`, `(req, result)`, each once. Show `Reject` feedback (shake + text) on `Err`. |
| `names: Vec<(PlayerId, String, u8 colour, bool bot)>` | Everyone in the match. `view.name_of(id)` looks one up; `view.hat_of(id) -> u8` gives the cosmetic hat (255 = default for the colour). |
| `phase: Phase` | `Countdown` / `Running` / `Over`. |
| `countdown: f32` | Seconds to GO during `Countdown` (drive 3-2-1-GO), else 0. |
| `round: u32` | 0 for the first match, +1 per rematch. |
| `mode: Mode` | `Solo` or `Multi` (Practice and online are `Multi`). |
| `ping_ms: Option<u32>` | Online round trip. |
| `correction: [f32; 3]` | Online visual correction already applied to your runner (debug). |
| `online: bool` | Driven by a network session. |
| `paused: bool` | Offline pause state. |
| `over: bool` | Results exist (or the link died). |

### `ResultsView`

```rust
ResultsView { rows: Vec<ResultRow>, end_reason: Option<EndReason>, round: u32, solo: Option<SoloResult> }
ResultRow { player, name, colour, hat, bot, place /*1-based, shared on ties*/, alive, survived_ticks,
            feathers, coins, placements /*architect score, unranked*/, disconnected }
row.survived_secs()   // f32 seconds
results.row(id)
```

Rows are sorted by place then id, built from `World::standings()` on the
host. Disconnected players are last and flagged. Solo: `solo = Some(SoloResult
{ distance_m, coins })` (use it for the game-over card). `end_reason`:
`FinalFeather` (solo), `NoRunnersLeft`, `TimeCap`, `NotEnoughPlayers`,
`Aborted` (host left / link lost).

---

## 2. Offline: `LocalRun` (Solo and Flock Practice)

```rust
use flock_session::LocalRun;
let mut run = LocalRun::solo(seed);                              // one runner, ends on the final feather
let mut run = LocalRun::daily(seed);                             // the daily run: solo rules, same course on every device
let mut run = LocalRun::practice(seed, bots /*1..=5*/, BotSkill::NORMAL); // you + bots, multiplayer rules, ghost mode
run.update(now_ms, input); run.view(); run.results(); run.place(tool, lane, row);
run.set_paused(true);   // freezes the sim (frame times keep flowing; auto-pause on focus loss)
run.is_paused(); run.is_over();
run.world() / run.world_mut()   // the real sim (debug tools: force_eliminate, energy, slow-mo by not updating)
LocalRun::new(cfg, local)       // any MatchConfig (debug: World::debug_* scenes are built the same way)
```

Fixed 60 Hz steps from frame times (a frame counts at most 100 ms, at most 12
ticks per update). Practice names: "You", "Bot 1".. with colours 0..n. A
command given on a frame that stepped no tick is kept for the next tick.
"Run again": make a new `LocalRun`.

---

## 3. Online: `crate::online`

### The slot

There is one session per process in a global slot. Screens never own it:

```rust
crate::online::put(session);                 // make it current (an older one leaves its room)
crate::online::with(|s| ...) -> Option<R>    // borrow it (None: no session, or re-entrant use)
crate::online::is_some()
crate::online::leave()                       // leave + empty the slot (idempotent)
```

The engine already calls `online::pump_auto()` / `online::end_frame()` every
frame and from the background tick, `online::set_away(..)` on
background/foreground, and leaves the room (with a toast) if My Signet
approval ends. A screen that shows a session should drive it itself with
`s.pump(input)` (or `s.driver().update(..)`) each frame; `pump_auto` then
does nothing that frame.

### Opening a session (each needs My Signet approval)

All return `Result<OnlineSession, String>` (`Err` = friendly text: not
signed in / not approved). Gate the screens with `account::gate` as today;
these calls re-check the persisted approval anyway and open no socket
without it.

```rust
OnlineSession::host(data_dir, name, colour)            // room code online + Nearby beacon at once
OnlineSession::join_code(data_dir, code, name, colour) // "Join with code"
online::browse_nearby(data_dir) -> LanBrowser          // Nearby list: browser.poll() -> Vec<LanRoom {name, code, player_count, max_players, addr, ..}>
OnlineSession::join_nearby(data_dir, &room, name, colour)
OnlineSession::join_bluetooth(data_dir, &seen, name, colour)  // a BleSeen from online::ble_seen()
OnlineSession::offline_host(name, colour, HostConfig::new(seed)) // no network (screenshots, tests)
```

`colour` is the preferred colour 0..6 (the host gives the first free one if
taken). Then `online::put(s)` and push the lobby/connecting screen.

Nearby on Android needs the Wi-Fi multicast lock while browsing
(`ctx.platform.set_multicast_lock(true)`), released on back/background.

### Bluetooth

Radio (no session needed), for the Nearby/Bluetooth screen:

```rust
online::ble_state() -> BleState      // Unsupported (hide all BT UI) | Off | NeedsPermission | PermissionDenied | Ready
crate::platform::ble::get().request_permission() / request_enable() / open_settings()
online::ble_scan() -> Result<(), String>  // start once while Ready (Android throttles ~5 starts / 30 s)
online::ble_seen() -> Vec<BleSeen>   // strongest first; label: online::bluetooth_room_name(&seen), colour: seen.advert.colour
online::ble_scan_error() -> Option<String>
online::ble_stop_scan()              // leaving the screen / background
```

Host side (on a hosting session; the Bluetooth door joins the *same* room as
code + Nearby):

```rust
s.start_bluetooth(colour) -> Result<(), String>   // advertise ("Bluetooth game · room 1A2B", never the host's name)
s.stop_bluetooth()                                 // seated BT players stay; waiting ones are turned away
s.bluetooth() -> BtStatus                          // Off | On { room: "1A2B" } | Failed(why)  (async radio failure shows ~1 s after start)
s.pending_joiners() -> Vec<PendingJoiner>          // BT joiners waiting for Accept
//   PendingJoiner { peer, name /*typed, self-chosen*/, persona: Option<String> /*verified npub1...*/, since }
s.accept(peer) -> bool
s.decline(peer) -> bool
```

**Accept prompt**: name the joiner by its verified persona (`persona`,
`npub1...`; show it shortened) with the typed `name` beside it. Unanswered
joiners are turned away after 60 s. Bluetooth joiners see
`ClientPhase::Waiting` ("Waiting for the host to accept") until then; after
Accept the joiner verifies the host's persona before showing the lobby
(`s.host_persona()`; failure = `Failed("Couldn't confirm the host's My Signet
approval.")`). Every path requires a verified persona from every joiner.

Desktop: `gameflock --fake-ble` uses an in-memory radio shared by all
engines in the process.

### Reading a session (`s: &OnlineSession`)

```rust
s.phase() -> &ClientPhase   // Connecting(stage text) | Waiting | Lobby | Running | Results
                            // | Failed(why) | Closed(why) | HostLeft | Disconnected(why)
s.lobby() -> Option<&LobbyView>
s.my_seat() -> Option<u8>          // your SeatView::id
s.is_host(); s.joined_by()         // Host | Code | Nearby | Bluetooth | Offline
s.code() / s.code_ready()          // show + share the code only when code_ready() (relay up 1.2 s)
s.room_ready(); s.status() -> &RoomStatus { ready, failure, relays: (up, configured), lan }
online::join_link(code)            // "gameflock://join#CODE" for Share
online::spaced_code(code)          // "KWT QPM"
s.host_persona() -> Option<&str>   // the host's verified npub (Bluetooth: after Accept)
s.rtt_ms(); online::quality(rtt) -> 0 good | 1 ok | 2 poor
s.notice() -> Option<String>       // "<name> left" (4 s)
s.results() -> Option<&ResultsView>
s.rematch_ms_left() -> Option<u64> // rematch 3-2-1 running
s.can_start()                      // host: >= 2 seats incl. bots, >= 1 human, in the lobby
s.dead_reason() -> Option<String>  // the link is gone, in words: "The host left the game.", "Room full...", ...
s.host_left() -> bool              // show "Host left" + Solo / back to the multiplayer menu
s.me() -> &ClientSession; s.authority() -> Option<&HostAuthority>   // advanced/debug
```

`LobbyView { seats: Vec<SeatView>, phase: LobbyPhase (Lobby|Running|Results), round }`,
helpers `humans() bots() seat(id) host_seat() free_colours()`.
`SeatView { id, name, colour (0..6, unique), hat (255 = default; bots 255), ready, bot, host, persona: Option<npub>, path: SeatPath (Local|Network|Bluetooth), player: Option<PlayerId> (during a match) }`.
Human seat ids are 0..100 in join order; bot seats are 100+.

### Lobby and results actions (`s: &mut OnlineSession`)

```rust
s.set_ready(bool)              // everyone ready (>= 2 seats) starts the match
s.set_colour(c)                // 0..6; another person's colour is refused, a bot's moves
s.set_hat(h) / ClientSession::with_hat(h)  // before the first update: the hat goes in the Hello (fixed for the session)
s.add_bot() -> bool            // host, lobby, while a seat is free (6 seats; a joining person displaces a bot)
s.remove_bot(Some(seat) | None) -> bool
s.set_bot_skill(BotSkill)      // host
s.start() -> bool              // host: start now
s.kick(seat) -> bool           // host
s.set_rematch_ready(bool)      // results: all ready -> 3-2-1 -> next round (round + 1 rotates the grid)
s.to_lobby() -> bool           // host: results -> lobby
s.leave()                      // or online::leave()
```

Screen flow: `Connecting/Waiting` → `Lobby` → `Running` (push the run
screen with `s.driver()`) → `Results` (results table; rematch → `Running`
again; host `to_lobby` → `Lobby`). Any dead phase → show `dead_reason()` and
go back to the multiplayer menu (offer Solo on `HostLeft`).

### The run screen online

```rust
crate::online::with(|s| {
    let d = s.driver();          // &mut dyn RunDriver
    d.update(now_ms, input);
    if let Some(v) = d.view() { /* draw */ }
    if let Some(r) = d.results() { /* go to the results table */ }
});
```

`place` online sends a reliable request; the answer arrives a round trip
later in `place_results`. The run screen should not `leave` on its own; the
results/lobby screens own that decision. There is no pause online.

---

## 4. The protocol crate (for reference)

* `HostAuthority` — lobby, seats (humans + bots), colours, the Bluetooth
  Accept gate, the match clock, input queues (`Human::feed` repeat-on-starve
  + `CmdLatch`), placements (`World::place`, idempotent per `(player, req)`),
  course row streaming (reliable, chunked <= 1024 B), events, 30 Hz
  snapshots (<= 1024 B, worst-case 6 players ≈ 600 B), results, rematch.
  Silent 6 s mid-match → `World::disconnect`. Host leaving → clients
  `HostLeft`.
* `ClientSession` — join, lobby, own-runner prediction
  (`flock_core::runner::runner_tick`, replay of unacked inputs, eased
  correction, snap only > 10 m), others interpolated with an adaptive
  100-350 ms delay. Implements `RunDriver`.
* `host_with_local_player(remote, cfg, name, colour)` — the host's own
  player on a zero-latency loopback.
* `door::HostLink` — the UDP + Bluetooth doors merged into one transport;
  the path is decided by the door, never by the joiner.
* `net` — QUIC over fray-net: `host_room`, `join_code`, `join_nearby`,
  `browse_nearby`, `host_bluetooth`, `join_bluetooth`, `join_addr`;
  `APP_TAG = "gameflock"` (= `account::APP_ID`), `PROTO_VERSION = 3`
  (0.2: hat in `Hello`/`SeatView`/`PlayerSpec`, `RunnerNet.flags`, new events).
* `flock-headless --smoke [--players N] [--bots B]` (localhost QUIC round,
  default 4 + 2 bots) and `--lan [--bots B] [--seconds S]` (a bounded LAN
  room "Flock test host" for phones on Nearby).
