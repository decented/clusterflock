//! Haptics (SPEC 12): short one-shot vibrations, best effort.
//!
//! Desktop is a no-op. Android goes through `android::AndroidHaptics`: `jni` +
//! `ndk-context` calls to `Vibrator` / `VibrationEffect.createOneShot`. Any
//! error there disables haptics silently. Use `ctx.haptics.pulse(Pulse::Bash)`.

#[cfg(target_os = "android")]
pub mod android;

/// What happened; each kind maps to a length and a strength.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pulse {
    /// A menu button press (very light).
    Tap,
    /// A bitcoin coin picked up (light).
    Coin,
    /// Landing after a jump.
    Landing,
    /// A boost starts (drift release, pad, mushroom-style item).
    Boost,
    /// You were hit by something.
    Hit,
    /// You bashed another ostrich (strongest).
    Bash,
    /// Eliminated / game over: one long strong buzz.
    Long,
}

impl Pulse {
    /// Vibration length in milliseconds.
    pub fn millis(self) -> u32 {
        match self {
            Pulse::Tap => 8,
            Pulse::Coin => 10,
            Pulse::Landing => 22,
            Pulse::Boost => 40,
            Pulse::Hit => 70,
            Pulse::Bash => 55,
            Pulse::Long => 260,
        }
    }

    /// Touch feedback (a menu button press) rather than a game event. Android files
    /// touch feedback under the system "Touch feedback" switch, which many players turn
    /// off; game events go out as media/game vibrations instead, so hits and coins still
    /// buzz with that switch off. The Settings toggle gates both.
    pub fn is_touch(self) -> bool {
        matches!(self, Pulse::Tap)
    }

    /// Strength 1..=255 (Android 8+ with amplitude control; else ignored).
    pub fn amplitude(self) -> u8 {
        match self {
            Pulse::Tap => 60,
            Pulse::Coin => 80,
            Pulse::Landing => 140,
            Pulse::Boost => 170,
            Pulse::Hit => 230,
            Pulse::Bash => 255,
            Pulse::Long => 230,
        }
    }
}

pub trait Haptics {
    /// Fire a one-shot vibration of `ms` at `amplitude`; `touch` marks touch feedback
    /// (see [`Pulse::is_touch`]). Never blocks, never panics.
    fn vibrate(&mut self, ms: u32, amplitude: u8, touch: bool);
    fn set_enabled(&mut self, on: bool);
    fn enabled(&self) -> bool;

    /// Fire the vibration for `p` (nothing when haptics are off).
    fn pulse(&mut self, p: Pulse) {
        if HAPTICS_ALLOWED && self.enabled() {
            self.vibrate(p.millis(), p.amplitude(), p.is_touch());
        }
    }
}

/// Desktop: does nothing (records the last pulse for tests/debug).
#[derive(Debug, Default)]
pub struct NoHaptics {
    pub on: bool,
    pub last_ms: Option<u32>,
    /// Whether the last pulse was touch feedback.
    pub last_touch: Option<bool>,
    pub count: u32,
}

impl Haptics for NoHaptics {
    fn vibrate(&mut self, ms: u32, _amplitude: u8, touch: bool) {
        self.last_ms = Some(ms);
        self.last_touch = Some(touch);
        self.count += 1;
    }
    fn set_enabled(&mut self, on: bool) {
        self.on = on;
    }
    fn enabled(&self) -> bool {
        self.on
    }
}

/// Master switch for haptics in this build (the Settings toggle, `Profile.haptics`, still gates every pulse).
pub const HAPTICS_ALLOWED: bool = true;

/// Seconds between two coin pulses at most (a coin chain would otherwise buzz continuously).
pub const COIN_GAP_S: f32 = 0.09;
/// Seconds between the two ticks of a power-up pickup.
pub const DOUBLE_TICK_GAP_S: f32 = 0.09;

/// The Run screen's haptic timing: the coin rate limit, the second tick of a double tick and
/// the once-only long buzz. Time is the screen's own clock in seconds.
#[derive(Debug, Default)]
pub struct RunBuzz {
    last_coin: Option<f32>,
    queued: Vec<(f32, Pulse)>,
    long_sent: bool,
}

impl RunBuzz {
    /// Fire `p` now (coins are limited to one per [`COIN_GAP_S`]).
    pub fn pulse(&mut self, h: &mut dyn Haptics, now: f32, p: Pulse) {
        if p == Pulse::Coin {
            if self.last_coin.is_some_and(|t| now - t < COIN_GAP_S && now >= t) {
                return;
            }
            self.last_coin = Some(now);
        }
        h.pulse(p);
    }

    /// Two ticks of `p`, [`DOUBLE_TICK_GAP_S`] apart.
    pub fn double_tick(&mut self, h: &mut dyn Haptics, now: f32, p: Pulse) {
        h.pulse(p);
        self.queued.push((now + DOUBLE_TICK_GAP_S, p));
    }

    /// The long strong buzz, once per run (elimination and game over arrive together).
    pub fn long_once(&mut self, h: &mut dyn Haptics) {
        if !self.long_sent {
            self.long_sent = true;
            h.pulse(Pulse::Long);
        }
    }

    /// Fire the queued second ticks that are due.
    pub fn update(&mut self, h: &mut dyn Haptics, now: f32) {
        let mut i = 0;
        while i < self.queued.len() {
            if now >= self.queued[i].0 {
                let (_, p) = self.queued.remove(i);
                h.pulse(p);
            } else {
                i += 1;
            }
        }
    }

    /// Drop anything queued (pause).
    pub fn clear(&mut self) {
        self.queued.clear();
    }
}

/// [`RunBuzz`] bound to a [`Haptics`] and a clock for one frame; `on` false (paused) makes every call a no-op.
pub struct Buzzer<'a> {
    pub buzz: &'a mut RunBuzz,
    pub h: &'a mut dyn Haptics,
    pub now: f32,
    pub on: bool,
}

impl Buzzer<'_> {
    pub fn pulse(&mut self, p: Pulse) {
        if self.on {
            self.buzz.pulse(self.h, self.now, p);
        }
    }
    pub fn double_tick(&mut self, p: Pulse) {
        if self.on {
            self.buzz.double_tick(self.h, self.now, p);
        }
    }
    pub fn long_once(&mut self) {
        if self.on {
            self.buzz.long_once(self.h);
        }
    }
}

/// The platform's haptics.

pub fn platform(enabled: bool) -> Box<dyn Haptics> {
    #[cfg(target_os = "android")]
    {
        let mut h = android::AndroidHaptics::new();
        h.set_enabled(enabled);
        Box::new(h)
    }
    #[cfg(not(target_os = "android"))]
    {
        Box::new(NoHaptics { on: enabled, ..NoHaptics::default() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_is_silent_and_bash_is_the_strongest() {
        assert!(HAPTICS_ALLOWED, "haptics are on in this build");
        let mut h = NoHaptics { on: true, ..Default::default() };
        h.pulse(Pulse::Bash);
        assert_eq!(h.last_ms, Some(55));
        h.set_enabled(false);
        h.pulse(Pulse::Coin);
        assert_eq!(h.count, 1);
        assert!([Pulse::Tap, Pulse::Coin, Pulse::Landing, Pulse::Boost, Pulse::Hit, Pulse::Long].iter().all(|p| p.amplitude() <= Pulse::Bash.amplitude()));
        assert!(Pulse::Coin.amplitude() < Pulse::Landing.amplitude() && Pulse::Landing.amplitude() < Pulse::Boost.amplitude() && Pulse::Boost.amplitude() < Pulse::Hit.amplitude());
    }

    #[test]
    fn only_menu_taps_are_touch_feedback() {
        // Game events must not be filed as touch feedback: Android drops those when the
        // system "Touch feedback" switch is off (seen on a Pixel 8 with 0.2.1).
        let mut h = NoHaptics { on: true, ..Default::default() };
        h.pulse(Pulse::Tap);
        assert_eq!(h.last_touch, Some(true));
        for p in [Pulse::Coin, Pulse::Landing, Pulse::Boost, Pulse::Hit, Pulse::Bash, Pulse::Long] {
            assert!(!p.is_touch(), "{p:?}");
            h.pulse(p);
            assert_eq!(h.last_touch, Some(false), "{p:?}");
        }
    }

    #[test]
    fn coins_are_rate_limited_double_ticks_come_twice_and_the_long_buzz_once() {
        let mut h = NoHaptics { on: true, ..Default::default() };
        let mut b = RunBuzz::default();
        for i in 0..10 {
            b.pulse(&mut h, i as f32 * 0.02, Pulse::Coin);
        }
        assert_eq!(h.count, 2, "ten coins in 0.18 s give a pulse at 0.0 s and one at 0.1 s");
        let before = h.count;
        b.double_tick(&mut h, 1.0, Pulse::Boost);
        assert_eq!(h.count, before + 1);
        b.update(&mut h, 1.05);
        assert_eq!(h.count, before + 1, "the second tick waits");
        b.update(&mut h, 1.1);
        assert_eq!(h.count, before + 2);
        b.long_once(&mut h);
        b.long_once(&mut h);
        assert_eq!(h.count, before + 3);
        assert_eq!(h.last_ms, Some(Pulse::Long.millis()));
    }
}
