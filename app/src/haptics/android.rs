//! Android haptics through JNI (`jni` + `ndk-context`, see
//! `platform::android::with_env`):
//!
//! 1. The Application context's `getSystemService("vibrator")` →
//!    `android.os.Vibrator` (cached as a global ref; `hasVibrator()` checked).
//! 2. API ≥ 26: `VibrationEffect.createOneShot(ms, amplitude)` +
//!    `vibrator.vibrate(effect)`; older: `vibrator.vibrate(ms)`.
//!    Game events (not touch feedback, see `Pulse::is_touch`) carry attributes, because
//!    Android files a short vibration without attributes as touch feedback and drops it
//!    when the system "Touch feedback" switch is off: API ≥ 33 `VibrationAttributes`
//!    usage MEDIA, API 26..=32 `AudioAttributes` usage GAME (which maps to the same).
//!    If the attributes can't be built, the plain call is used.
//! 3. On any JNI error `failed` is set and haptics stay off for the session
//!    (silently). Needs the `VIBRATE` permission (manifest).

use super::Haptics;
use crate::platform::android::{system_service, with_env};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;

#[derive(Debug, Default)]
pub struct AndroidHaptics {
    on: bool,
    failed: bool,
    vibrator: Option<GlobalRef>,
    /// The attributes of a game-event vibration (built once; `None` until first needed).
    game_attrs: Option<GlobalRef>,
    /// Building `game_attrs` failed: game events use the plain call.
    game_attrs_failed: bool,
}

impl AndroidHaptics {
    pub fn new() -> Self {
        Self::default()
    }

    fn vibrator(&mut self) -> Option<GlobalRef> {
        if self.vibrator.is_none() {
            let v = with_env("get vibrator", |env| {
                let v = system_service(env, "vibrator")?;
                if !env.call_method(&v, "hasVibrator", "()Z", &[])?.z()? {
                    return Ok(None);
                }
                env.new_global_ref(v).map(Some)
            })
            .flatten();
            if v.is_none() {
                log::info!("haptics: no vibrator; haptics off");
                self.failed = true;
            }
            self.vibrator = v;
        }
        self.vibrator.clone()
    }

    /// API ≥ 33: `VibrationAttributes.createForUsage(USAGE_MEDIA)`; API 26..=32:
    /// `new AudioAttributes.Builder().setUsage(USAGE_GAME).build()`.
    fn game_attrs(&mut self, sdk: i32) -> Option<GlobalRef> {
        if self.game_attrs.is_none() && !self.game_attrs_failed {
            let a = with_env("vibration attributes", |env| {
                let attrs = if sdk >= 33 {
                    let usage = env.get_static_field("android/os/VibrationAttributes", "USAGE_MEDIA", "I")?.i()?;
                    env.call_static_method("android/os/VibrationAttributes", "createForUsage", "(I)Landroid/os/VibrationAttributes;", &[JValue::Int(usage)])?.l()?
                } else {
                    let usage = env.get_static_field("android/media/AudioAttributes", "USAGE_GAME", "I")?.i()?;
                    let b = env.new_object("android/media/AudioAttributes$Builder", "()V", &[])?;
                    let b = env.call_method(&b, "setUsage", "(I)Landroid/media/AudioAttributes$Builder;", &[JValue::Int(usage)])?.l()?;
                    env.call_method(&b, "build", "()Landroid/media/AudioAttributes;", &[])?.l()?
                };
                env.new_global_ref(attrs)
            });
            if a.is_none() {
                log::info!("haptics: no vibration attributes; game events use the plain call");
                self.game_attrs_failed = true;
            }
            self.game_attrs = a;
        }
        self.game_attrs.clone()
    }
}

impl Haptics for AndroidHaptics {
    fn vibrate(&mut self, ms: u32, amplitude: u8, touch: bool) {
        if self.failed || ms == 0 {
            return;
        }
        let Some(vib) = self.vibrator() else { return };
        let sdk = android_activity::AndroidApp::sdk_version();
        let modern = sdk >= 26;
        // 1..=255; 0 would mean "default amplitude" (-1), which we never want.
        let amp = i32::from(amplitude.max(1));
        if modern && !touch {
            if let Some(a) = self.game_attrs(sdk) {
                let ok = with_env("vibrate (game)", |env| {
                    let effect = one_shot(env, ms, amp)?;
                    if sdk >= 33 {
                        env.call_method(vib.as_obj(), "vibrate", "(Landroid/os/VibrationEffect;Landroid/os/VibrationAttributes;)V", &[JValue::Object(&effect), JValue::Object(a.as_obj())])?;
                    } else {
                        env.call_method(vib.as_obj(), "vibrate", "(Landroid/os/VibrationEffect;Landroid/media/AudioAttributes;)V", &[JValue::Object(&effect), JValue::Object(a.as_obj())])?;
                    }
                    Ok(())
                });
                if ok.is_some() {
                    return;
                }
                // Never lose haptics over the attributes: drop them and use the plain call.
                log::info!("haptics: vibrate with attributes failed; game events use the plain call");
                self.game_attrs = None;
                self.game_attrs_failed = true;
            }
        }
        let ok = with_env("vibrate", |env| {
            if modern {
                let effect = one_shot(env, ms, amp)?;
                env.call_method(vib.as_obj(), "vibrate", "(Landroid/os/VibrationEffect;)V", &[JValue::Object(&effect)])?;
            } else {
                env.call_method(vib.as_obj(), "vibrate", "(J)V", &[JValue::Long(ms as i64)])?;
            }
            Ok(())
        });
        if ok.is_none() {
            log::info!("haptics: vibrate failed; haptics off");
            self.failed = true;
        }
    }
    fn set_enabled(&mut self, on: bool) {
        self.on = on;
    }
    fn enabled(&self) -> bool {
        self.on && !self.failed
    }
}

/// `VibrationEffect.createOneShot(ms, amplitude)` (API 26+).
fn one_shot<'a>(env: &mut JNIEnv<'a>, ms: u32, amp: i32) -> jni::errors::Result<JObject<'a>> {
    env.call_static_method("android/os/VibrationEffect", "createOneShot", "(JI)Landroid/os/VibrationEffect;", &[JValue::Long(i64::from(ms)), JValue::Int(amp)])?.l()
}
