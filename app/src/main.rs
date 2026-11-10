//! Desktop launcher (Android enters through `gameflock::android_main`).

#[cfg(not(target_os = "android"))]
fn main() {
    std::process::exit(gameflock::run_desktop());
}

#[cfg(target_os = "android")]
fn main() {}
