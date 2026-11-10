# Cluster Flock

A three-lane endless runner for Android (native Rust, no game engine, also
runnable on desktop Linux): floppy, jockey-less Nostriches charge through a
farm. Solo is a classic runner; multiplayer (2-6, phone-hosted over Fray:
online room code, Nearby LAN, Bluetooth) puts everyone on one course with
real body contact, and eliminated birds become ghost architects who drop hay,
beams, mud and coin lures ahead of the survivors. Forked from the Stampede
shell; Bluetooth and the results table descend from Twang Thang.

Build spec: [docs/SPEC.md](docs/SPEC.md). Crate guides:
`crates/flock-core/README.md`, `docs/api/flock-session.md`,
`docs/api/flock-render.md`.

Controls: swipe left/right = change lane, up = jump, down = collapse/slide,
diagonals = diagonal jump / dive. Keyboard: arrows or WASD, Space = jump.
Ghost mode: pick a tool (1-4), tap a lit square.

App shell API: [docs/api/app-shell.md](docs/api/app-shell.md).

## Source dependencies

The workspace uses sibling source checkouts as path dependencies. Before
building, provide `../forgesworn/fray`
(github.com/forgesworn/fray: the `fray-net` and `fray-relays` crates) and
`../TwangThang` (the `decented-signet` crate and its path dependencies). The
Signet crates are being moved to their own public repository; until then they
are not bundled with this repository. `Cargo.lock` pins the external registry
dependencies.

Two ForgeSworn libraries come in as git dependencies pinned by revision in
`app/Cargo.toml`, so cargo fetches them itself:
[blockseed](https://github.com/forgesworn/blockseed) (the daily run's seed) and
[nostr-release-kit](https://github.com/forgesworn/nostr-release-kit) (updates,
More games and the release publisher). Building the APK also compiles the kit's
Java, so `tools/build-apk.sh` needs a checkout of the kit at the pinned revision
(`../forgesworn/nostr-release-kit` by default, or set `KIT`), and refuses any
other revision.

## Desktop

```sh
cargo run -p gameflock                                # portrait window, starts on the splash
cargo run -p gameflock -- --screen home               # skip the splash
FLOCK_NO_VSYNC=1 cargo run -p gameflock               # if frames stall ~1 s each (X11 Fifo)
cargo test --workspace
```

Keys: Esc = Back. Mouse: left button = a finger, right button = a second finger.
Multiplayer asks for a My Signet sign-in first; solo play, screenshots and tests don't need one.

Headless screenshot of any registered screen (no window; needs Vulkan or GL):

```sh
cargo run -p gameflock -- --shot home --out docs/notes/shot-home.png --size 1080x2400 --ppp 2.625
```

## Android APK

No Gradle. Needs the Android SDK/NDK, `cargo-ndk` and a JDK.

```sh
tools/build-apk.sh
```

Output: `~/.cache/flock-android/out/gameflock_<version>_arm64-v8a.apk` and a
`gameflock.apk` symlink. `JOBS` (4), `NICE` (10), `PROFILE=debug` and
`ABI=x86_64` are optional. Release builds are signed with the project's
release key (`~/.flock-release/flock.keystore`, never committed); without it
the script falls back to a throwaway debug key, and such an APK cannot be
updated in place by a release-signed one. Application id `org.decented.gameflock`, activity
`.FlockActivity`, link scheme `gameflock://join`.

```sh
adb install -r ~/.cache/flock-android/out/gameflock.apk
adb shell am start -n org.decented.gameflock/.FlockActivity
adb logcat -s flock:V
```

## Releases

Signed releases are published on Nostr as kind 30063 events on the channel
`gameflock-android-apk`, by the release pubkey
`01b0f270e290f5cd48aa537392de04e4b22124c16d696de96158395a60343b0f`. The APK
signing certificate has SHA-256
`46fd4f7a80b700cd7d951c04e38fa3344405759d53d161de3e4dd06120fb3ace`. The app's
in-app updater trusts only that pubkey and checks the APK's size, SHA-256,
package and certificate before Android's own install prompt.

## Layout

- `app/` the `gameflock` crate (lib + desktop bin + Android cdylib).
- `crates/flock-render` the wgpu 3D renderer, `crates/flock-audio` the
  synthesised sound bank (both inherited from the fork).
- `tools/build-apk.sh` the APK pipeline; `tools/release/` the release
  config (`release.json`) and the wrapper (`publish.sh`) that runs
  nostr-release-kit's publisher.

## Assets

See `app/NOTICE` for third-party notices and the origin of the bundled art.

## Licence

MIT, see [LICENSE](LICENSE).
