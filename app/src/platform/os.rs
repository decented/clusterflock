//! OS services the game needs beyond a window: the clipboard (room codes),
//! launch links (`gameflock://join#CODE`), the screen wake lock, the Wi-Fi
//! multicast lock (Nearby play), the room service and leaving the app. Best effort everywhere: a failure is a message
//! or a no-op, never a crash.

use nostr_release_kit::testing::RecordingInstaller;
use nostr_release_kit::ApkInstaller;
use std::collections::VecDeque;

/// The orientations allowed by the current screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    /// Portrait only: the whole app, by default.
    Portrait,
    /// Follow the user's rotation setting.
    Free,
    /// Either landscape orientation (reserved; no screen asks for it yet).
    Landscape,
}

/// What to tell the player after `share_text` (`None`: the share sheet itself
/// is the feedback). Only Android has a sheet; elsewhere the text was copied.
pub fn share_note(platform: &dyn Platform, result: Result<(), String>) -> Option<String> {
    match result {
        Err(e) => Some(format!("Couldn't share: {e}")),
        Ok(()) if platform.name() == "android" => None,
        Ok(()) => Some("Copied to the clipboard".into()),
    }
}

/// OS services. One instance, owned by the engine (`Ctx::platform`). It is also the
/// app's [`ApkInstaller`] (nostr-release-kit): downloading, verifying and installing an
/// APK. Every installer method has a "cannot" default, so a platform with no installer
/// (desktop) implements nothing; Android delegates to the kit's `AndroidInstaller`.
pub trait Platform: ApkInstaller {
    fn name(&self) -> &'static str;

    fn set_orientation(&mut self, _orientation: Orientation) {}

    /// Open a link in the system browser.
    fn open_url(&mut self, url: &str) -> Result<(), String>;

    /// Put `text` on the clipboard (a room code). Android: `setPrimaryClip`.
    fn copy_text(&mut self, text: &str) -> Result<(), String>;

    /// Hand `text` to the system share sheet. Android: a chooser over
    /// `ACTION_SEND` (the player picks the app). Desktop: the clipboard.
    fn share_text(&mut self, text: &str) -> Result<(), String>;

    /// The clipboard's text, if any (paste a room code). Android 10+ answers
    /// nothing unless the app has input focus.
    fn read_clipboard(&mut self) -> Option<String>;

    /// A launch link the OS handed us (a `gameflock:` URL), at most one per call.
    fn take_launch_link(&mut self) -> Option<String>;

    /// Keep the screen on. The engine drives this from `Screen::keep_awake`.
    fn set_keep_screen_on(&mut self, on: bool);

    /// Keep the process in use while the player is in an online room (Android:
    /// a silent foreground service, so the network survives a switch to another
    /// app). The engine turns it on when a session exists and off when it ends.
    fn set_room_service(&mut self, _on: bool) {}

    /// Hold the Wi-Fi multicast lock while Nearby play listens (Android only).
    fn set_multicast_lock(&mut self, on: bool);

    /// Back on the first screen: leave the app the platform's own way.
    /// True if the app was sent away and the loop should go on (Android:
    /// to the background); false if the loop should end (desktop).
    fn leave_app(&mut self) -> bool {
        false
    }

    /// Records calls (tests and `--shot`); `None` on real platforms.
    fn recorded(&self) -> Option<&RecordingPlatform> {
        None
    }
}

/// The platform for this build. `launch_link` is the desktop stand-in for an
/// Android intent (`--join CODE`).
pub fn platform(launch_link: Option<String>) -> Box<dyn Platform> {
    #[cfg(target_os = "android")]
    {
        let _ = launch_link;
        Box::new(super::android::AndroidPlatform::default())
    }
    #[cfg(not(target_os = "android"))]
    {
        Box::new(DesktopPlatform { launch_link })
    }
}

// ─── Desktop ────────────────────────────────────────────────────────────────

/// Linux desktop: external tools, never a hard dependency.
#[cfg(not(target_os = "android"))]
pub struct DesktopPlatform {
    launch_link: Option<String>,
}

#[cfg(not(target_os = "android"))]
fn pipe_to(cmd: &str, args: &[&str], text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(cmd).args(args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| format!("{cmd}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes()).map_err(|e| format!("{cmd}: {e}"))?;
    }
    // wl-copy / xclip fork to keep serving the selection; don't wait on them.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

// Desktop cannot install an APK: the kit's defaults.
#[cfg(not(target_os = "android"))]
impl ApkInstaller for DesktopPlatform {}

#[cfg(not(target_os = "android"))]
impl Platform for DesktopPlatform {
    fn name(&self) -> &'static str {
        "desktop"
    }

    fn open_url(&mut self, url: &str) -> Result<(), String> {
        let mut child = std::process::Command::new("xdg-open")
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't open a browser ({e})"))?;
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    fn copy_text(&mut self, text: &str) -> Result<(), String> {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let tries: [(&str, &[&str]); 3] = if wayland {
            [("wl-copy", &[]), ("xclip", &["-selection", "clipboard"]), ("xsel", &["--clipboard", "--input"])]
        } else {
            [("xclip", &["-selection", "clipboard"]), ("xsel", &["--clipboard", "--input"]), ("wl-copy", &[])]
        };
        let mut last = String::from("no clipboard tool");
        for (cmd, args) in tries {
            match pipe_to(cmd, args, text) {
                Ok(()) => return Ok(()),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn share_text(&mut self, text: &str) -> Result<(), String> {
        // No share sheet on desktop: the clipboard is the nearest thing.
        log::info!("desktop: no share sheet, copying the text instead");
        self.copy_text(text)
    }

    fn read_clipboard(&mut self) -> Option<String> {
        let tries: [(&str, &[&str]); 3] = [("wl-paste", &["--no-newline"]), ("xclip", &["-selection", "clipboard", "-o"]), ("xsel", &["--clipboard", "--output"])];
        for (cmd, args) in tries {
            if let Ok(out) = std::process::Command::new(cmd).args(args).stderr(std::process::Stdio::null()).output() {
                if out.status.success() {
                    return String::from_utf8(out.stdout).ok().filter(|s| !s.trim().is_empty());
                }
            }
        }
        None
    }

    fn take_launch_link(&mut self) -> Option<String> {
        self.launch_link.take()
    }

    fn set_keep_screen_on(&mut self, _on: bool) {}

    fn set_multicast_lock(&mut self, _on: bool) {}
}

// ─── Tests and screenshots ──────────────────────────────────────────────────

/// Records every call (tests, headless screenshots). Never touches the OS.
#[derive(Debug)]
pub struct RecordingPlatform {
    pub orientations: Vec<Orientation>,
    pub opened: Vec<String>,
    pub copied: Vec<String>,
    /// Every text handed to the share sheet.
    pub shared: Vec<String>,
    pub clipboard: Option<String>,
    pub launch: VecDeque<String>,
    pub keep_on: bool,
    /// Whether the Wi-Fi multicast lock is held.
    pub multicast: bool,
    /// How many times the app was asked to leave (Back on the first screen).
    pub left: u32,
    /// What `leave_app` answers.
    pub leave_goes_on: bool,
    /// The installer double (nostr-release-kit's): the certificate it answers with (`cert`),
    /// the jobs it was asked to start (`apk_started`), the status it answers (`apk`), the
    /// installed packages, and so on. Its own package is Cluster Flock's.
    pub installer: RecordingInstaller,
}

impl Default for RecordingPlatform {
    fn default() -> Self {
        RecordingPlatform {
            orientations: Vec::new(),
            opened: Vec::new(),
            copied: Vec::new(),
            shared: Vec::new(),
            clipboard: None,
            launch: VecDeque::new(),
            keep_on: false,
            multicast: false,
            left: 0,
            leave_goes_on: false,
            // An install job that names no package is an update of this app.
            installer: RecordingInstaller { own_package: crate::release::OWN_PACKAGE.to_string(), ..RecordingInstaller::default() },
        }
    }
}

impl Platform for RecordingPlatform {
    fn set_orientation(&mut self, orientation: Orientation) { self.orientations.push(orientation); }
    fn name(&self) -> &'static str {
        "recording"
    }

    fn open_url(&mut self, url: &str) -> Result<(), String> {
        self.opened.push(url.to_string());
        Ok(())
    }

    fn copy_text(&mut self, text: &str) -> Result<(), String> {
        self.copied.push(text.to_string());
        self.clipboard = Some(text.to_string());
        Ok(())
    }

    fn share_text(&mut self, text: &str) -> Result<(), String> {
        self.shared.push(text.to_string());
        Ok(())
    }

    fn read_clipboard(&mut self) -> Option<String> {
        self.clipboard.clone()
    }

    fn take_launch_link(&mut self) -> Option<String> {
        self.launch.pop_front()
    }

    fn set_keep_screen_on(&mut self, on: bool) {
        self.keep_on = on;
    }

    fn set_multicast_lock(&mut self, on: bool) {
        self.multicast = on;
    }

    fn leave_app(&mut self) -> bool {
        self.left += 1;
        self.leave_goes_on
    }

    fn recorded(&self) -> Option<&RecordingPlatform> {
        Some(self)
    }
}

impl ApkInstaller for RecordingPlatform {
    fn can_install_apk(&self) -> bool {
        self.installer.can_install_apk()
    }

    fn own_cert_sha256(&mut self) -> Option<String> {
        self.installer.own_cert_sha256()
    }

    fn apk_start(&mut self, job: &nostr_release_kit::ApkJob) -> Result<(), String> {
        self.installer.apk_start(job)
    }

    fn apk_status(&mut self) -> nostr_release_kit::ApkStatus {
        self.installer.apk_status()
    }

    fn apk_cancel(&mut self) {
        self.installer.apk_cancel();
    }

    fn apk_active_package(&mut self) -> Option<String> {
        self.installer.apk_active_package()
    }

    fn apk_open_install_settings(&mut self) {
        self.installer.apk_open_install_settings();
    }

    fn installed_version_code(&mut self, package: &str) -> Option<u32> {
        self.installer.installed_version_code(package)
    }

    fn open_package(&mut self, package: &str) -> bool {
        self.installer.open_package(package)
    }
}
