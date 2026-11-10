# App shell (`app/`, crate `gameflock`)

The platform layer: window, GPU surface, egui, input, lifecycle, Android glue,
the screen router and headless screenshots. New screens and gameplay should not
need to touch `platform/`, `app.rs` or `render/`.

## Module map (`app/src`)

| Module | What |
|---|---|
| `lib.rs` | `run_desktop`, `android_main` (logger tag `flock`, panic hook, one game loop per process), `version_label()` |
| `cli.rs` | desktop options (`--screen`, `--shot`, `--out`, `--size`, `--ppp`, `--t`, `--insets`, `--join`, `--data-dir`, `--quit-after`) |
| `app.rs` | winit `ApplicationHandler`: surface create/drop/rebuild, lifecycle, input routing, redraw, frame-time log |
| `engine.rs` | `Engine::frame(dt, size_px, native_ppp, raw)`: input, router, egui, toast, fade, wake lock. Same code for window and `--shot` |
| `shot.rs` | headless screenshot mode |
| `render/` | `Gpu`, `EguiPainter`, `capture` (offscreen + PNG), `perf::FrameStats`, `ScenePass`, `encode_frame` |
| `platform/` | `lifecycle`, `viewport` (UI scale, `Insets`, `Viewport`), `touch` (`Touches`, `Keys`), `os` (`Platform` trait), `data_dir` (`Profile`), `android` (JNI) |
| `haptics/` | `Haptics`, `Pulse`; Android vibrator over JNI |
| `ui/theme.rs` | palette, fonts, `install` |
| `account.rs` | My Signet sign-in and online approval (`Account`, over `decented-signet`); `Account::identity_for_online` gates real network use |
| `ui/screens/` | `Screen`, `Transition`, `Router`, `Ctx`, registry `SCREENS`; `splash`, `home` (placeholder buttons), `settings` (+ `settings_cards`: UPDATES and MORE GAMES), `games` (More games), `account`, `common` |
| `update.rs` | the in-app updater (`Updater`, on `Ctx::updates`): checks one signed kind-30063 release event per launch, downloads and verifies the APK, hands it to Android's installer. `RELEASE_PUBKEY_HEX` pins the release key (were it `None`, Settings would say "Updates aren't set up for this build yet" and nothing is fetched) |
| `games.rs`, `nostr_fetch.rs` | More games: the built-in `CATALOG` (Twang Thang, Stampede; pins copied from their own catalogs) and the one-shot relay reader both features use |
| `platform/updater_android.rs` | Android half of installs: calls `Updater.java` (`Platform::apk_*`, `installed_version_code`, `open_package`) |
| `sound.rs` | the app's one `flock_audio::Audio` |

Frame: `engine.frame` -> `render::encode_frame` = clear to `Screen::clear_color`,
`Screen::paint_scene` (your 3D hook), then egui on top.

Screen-off, suspension and focus loss mute the shared audio immediately from
`Engine::on_background`, independently of screen hooks and further frames.
Foreground focus restores audio at the saved volumes. Surface recreation alone does not unmute an unfocused window.

## The Screen trait (`ui/screens/mod.rs`)

```rust
pub trait Screen {
    fn name(&self) -> &'static str;                                   // required; key for --shot/--screen/PopTo
    fn update(&mut self, ctx: &mut Ctx) -> Transition;                // logic, once per frame
    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition; // egui UI
    fn back(&mut self, ctx: &mut Ctx) -> Transition;                  // system Back / Esc; default Pop
    fn clear_color(&self, ctx: &Ctx) -> [f32; 4];                     // sRGB rgba; default theme::BG
    fn paint_scene(&mut self, pass: &mut ScenePass);                  // 3D renderer hook (device, queue, encoder, view, size_px, format)
    fn orientation(&self) -> platform::os::Orientation;              // Portrait by default (the whole app is portrait)
    fn keep_awake(&self) -> bool;                                     // wake lock while true (a run in progress)
    fn on_background(&mut self, ctx: &mut Ctx);                       // app backgrounded / unfocused
}
pub enum Transition { None, Push(Box<dyn Screen>), Replace(Box<dyn Screen>), Cut(Box<dyn Screen>), Pop, PopTo(&'static str), Quit }
```

Push/Replace/Pop/PopTo fade through the background colour (0.14 s out, 0.22 s
in); input is held back (`ctx.touches` empty) during the fade-out. `Cut` swaps
at once. `Pop` on the first screen quits: on Android that is `moveTaskToBack`
(the loop keeps running), on desktop the window closes.

`Ctx` (built by the engine each call): `dt`, `time`, `viewport`
(`screen`, `safe`, `insets`, `ppp` in UI points), `touches`, `keys`, `haptics`,
`platform`, `profile`, `data_dir`, `account`, `muted`, `opts`, plus
`ctx.say("toast")`, `ctx.save_profile()`.

### Add a screen

1. New file `ui/screens/run.rs` with `pub struct RunScreen` implementing `Screen`.
2. `pub mod run;` in `ui/screens/mod.rs` and one line in `SCREENS`:
   `("run", || Box::new(run::RunScreen::new()))`.
3. Open it from another screen: `Transition::Push(Box::new(RunScreen::new()))`.
4. Check it without a display: `cargo run -p gameflock -- --shot home --out /tmp/h.png`.

Lay out inside `ctx.viewport.safe` (the camera cutout is at the top). Design
for a width of 360-412 points; the screen is about 800-915 points tall.

### Online-gated screens

`engine::ONLINE_SCREENS` lists screen names (`multiplayer`, `join`, `nearby`,
`bluetooth`, `lobby`, ...) that need My Signet's online approval. A
Push/Cut/Replace to one of them is wrapped in `AccountScreen::online(Some(next))`
until `Account::online_allowed()`. A new multiplayer screen's `name()` goes there
(a test checks the existing ones). The multiplayer screens are
`ui/screens/{multiplayer,join,nearby,bluetooth,lobby}.rs`; the hand-off to the
match is `ui/screens/online_flow.rs` (`start_run`, `show_results`).

## UI scale

The UI uses a short side of 360-412 points in either orientation
(`viewport::ui_ppp`); the app is portrait, so that is the width. On phones that
is the native scale (Pixel 8: 411x914 pt at 2.625). A desktop window or tablet
is scaled so its short side is 412 points (540x1170 window -> 412x893 pt). All
positions in `Ctx` are UI points. egui gets the same ppp and screen rect; egui's
pointer is fed by the engine, not egui-winit, so hit-testing matches.

The whole app is portrait: `Screen::orientation` defaults to
`Orientation::Portrait` (Android `SCREEN_ORIENTATION_PORTRAIT`), and the manifest
is `screenOrientation="portrait"`. `Free` and `Landscape` still exist in the
`Orientation` enum for a screen that must differ. The engine changes the
request only when the top screen's policy changes; a surface resize clears held
touches and asks for fresh cutout insets immediately.

`screens::common::Layout::new(safe).portrait` is true here; `common::scroll`
provides a clipped body for tall painted pages.

## Raw touch API (`ctx.touches: &Touches`)

egui sees one pointer; gameplay may need every thumb. Each finger:

```rust
Touch { id: u64, pos, start: Pos2, start_time: f64, vel: Vec2 /* pt/s, smoothed */, order: u64 }
// .travel(), .delta(), .age(now), .is_mouse()
touches.active() -> &[Touch]      // down now, oldest first
touches.began()   -> &[Touch]     // went down since last frame (a tap lands in began AND ended)
touches.ended()   -> &[Touch]     // lifted/cancelled since last frame, at last position
touches.get(id), touches.count(), touches.in_rect(rect)
```

Desktop: left mouse = touch `MOUSE_LEFT`, right mouse = a second touch
`MOUSE_RIGHT` (press both to try two thumbs). Ids are stable for a finger's
life; lifting one never moves another. `ctx.keys`: `held(KeyCode)`,
`pressed`, `released`, `any_held(&[..])` (physical keys, so WASD is layout
independent). Esc and Android Back call `Screen::back` (not in `keys`).
Raw touches are not gated by egui: if a screen has buttons and a thumb pad,
test `ctx.touches` against your own rects.

## Haptics

`ctx.haptics.pulse(Pulse::Tap)`; kinds `Tap, Coin, Landing, Boost, Hit, Bash`
(length + amplitude each, `Pulse::millis/amplitude`). Haptics are disabled in
code for now (`HAPTICS_ALLOWED=false` in `haptics/mod.rs`). No-op on desktop and
when `profile.haptics` is off (set via `set_enabled`; the engine does not sync
it, call `ctx.haptics.set_enabled(ctx.profile.haptics)` after Settings changes).

## Wake lock, room service, clipboard, links

- Wake lock: return `true` from `Screen::keep_awake` while a run is in progress; the
  engine toggles `FLAG_KEEP_SCREEN_ON` (via the UI thread, never
  `AndroidApp::set_window_flags`, which deadlocks) and drops it on background.
- Room service: the engine starts `RoomService` (a silent foreground service)
  while a My Signet request is waiting, so Android keeps the network up.
- Clipboard: `ctx.platform.copy_text(code)`, `ctx.platform.read_clipboard()`
  (desktop needs xclip, wl-copy or xsel).
- Join links: `gameflock://join#CODE` (adb: `am start -a android.intent.action.VIEW
  -d "gameflock://join#KWTQPM" org.decented.gameflock`; desktop: `--join KWTQPM`)
  are read by the platform (`Platform::take_launch_link`); the engine parses the
  code and, from Home or the multiplayer menu family, opens the join screen on it
  (My Signet's gate first). In a room the link is refused with a toast.
- `ctx.muted` is true while backgrounded or unfocused (flock-audio should read it).

## More games and updates

`Home` (the small "More games" button beside Settings) and Settings (MORE GAMES
card) open `ui/screens/games.rs`; Settings also has the UPDATES card.
Both are ported from Stampede and kept close to it. The catalog is app code,
one entry per game (`games.rs`): name, blurb (under 60 bytes), package, release
channel, the release key and APK signing certificate pins, and a PNG in
`app/assets/games/`. Keep the manifest `<queries>` in step with it. Tapping Get
downloads the APK named by that game's signed release event, checks SHA-256,
size, package and certificate, then Android shows its own install prompt (the
app needs the "Install unknown apps" switch; nothing is silent).

Release identity (Cluster Flock's own): package `org.decented.gameflock`,
channel `gameflock-android-apk`. The release pubkey (`update.rs`
`RELEASE_PUBKEY_HEX`) and signing cert
(`tools/release/release-helpers.mjs` `EXPECTED_CERT_SHA256`) are pinned (see
`README.md`, Releases). Publishing is `tools/release/publish-release.mjs`
(Blossom upload + one signed event; see its header). Screenshots:
`--shot more_games`, `more_games_none_sample`, `more_games_downloading_sample`,
`more_games_open_sample`, `settings_cards`, `settings_updates_sample`,
`settings_downloading_sample`.

## Data dir and profile

Android: the activity's internal data path. Desktop: `--data-dir`, else
`$XDG_DATA_HOME/gameflock` or `~/.local/share/gameflock`. One file,
`profile.json` (`Profile`: name, music/effects volume, haptics, quality,
left_handed, seen_intro, `best_distance_m`, `best_coins`; every field defaulted,
so old files load and unknown keys are ignored; corrupt file ->
`profile.json.bad` + defaults). Mutate `ctx.profile`; it is saved when the app
is backgrounded and on exit (`ctx.save_profile()` forces it). Put new settings
in `Profile`.

## Shot mode

```
cargo run -p gameflock -- --shot home --out docs/notes/shot-home.png
    [--size 1080x2400] [--ppp 2.625] [--t 1.5] [--insets 0,48,0,0] [--join CODE]
```

No window: the screen runs on a virtual 60 Hz clock for `--t` seconds (default
1) with no input, the last frame is rendered offscreen (Vulkan or GL, lavapipe
is fine) and written as PNG. `--size` is output pixels (default 1080x2400);
`--size 1080x2400 --ppp 2.625` reproduces a phone (insets are `left,top,right,bottom`). Unknown names list the
registered screens. A screen that needs fingers/keys for its state needs a
constructor that stages it (add a registry entry with that state).

## Frame-time log

Every 5 s one line (logcat tag `flock`, desktop stderr):
`frames: 301 in 5.0s = 60.1 fps | interval avg 16.63 ms p95 18.86 ms | cpu avg 16.19 ms p95 18.42 ms`.
`interval` = time between presented frames; `cpu` = start of redraw to present
returned (includes waiting in Fifo `present`, so it tracks the interval).
`FLOCK_NO_VSYNC=1` selects Immediate on desktop: on the dev laptop (Intel
Xe, X11) Fifo surface acquire blocks ~1 s per frame (Twang Thang does the
same), so use it for desktop runs.

## Lifecycle notes (ported, do not undo)

- Surface dropped in `suspended`, rebuilt in `resumed`; the GPU device and
  egui's textures (`TextureMirror`) survive. `Lost`/`Outdated` reconfigure.
- Unfocused on Android: no drawing, `ControlFlow::Wait`.
- One `EventLoop` per process: the loop ends when the activity is destroyed
  and the process exits with it (`platform/lifecycle.rs`).
- egui's clock is the engine clock (`raw.time`), not egui-winit's.
- Insets are re-read every second (a 180-degree flip moves the cutout).
