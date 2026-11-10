//! Crash impact, as numbers: the white flash and the time scale (hit-stop and
//! final-feather slow motion). Real time in, presentation out; the Run screen
//! only applies the time scale to a LOCAL sim (solo), never to a networked one.

use crate::ui::art;

/// Flash peak alpha and length.
pub const FLASH_ALPHA: f32 = 0.30;
pub const FLASH_SECS: f32 = 0.20;
/// Hit-stop: this slow for `HIT_HOLD` seconds, then eased back over `HIT_EASE`.
pub const HIT_SCALE: f32 = 0.15;
pub const HIT_HOLD: f32 = 0.10;
pub const HIT_EASE: f32 = 0.20;
/// Final feather: slow motion for this long.
pub const SLOWMO_SCALE: f32 = 0.3;
pub const SLOWMO_SECS: f32 = 0.9;

#[derive(Default)]
pub struct Impact {
    /// Real seconds since the last feather-losing crash (flash).
    flash: Option<f32>,
    /// Real seconds since the hit-stop began.
    hit: Option<f32>,
    /// Real seconds of slow motion left.
    slow: f32,
}

impl Impact {
    pub fn new() -> Impact {
        Impact::default()
    }

    /// A feather-losing crash on the local runner.
    pub fn flash(&mut self) {
        self.flash = Some(0.0);
    }

    pub fn hit_stop(&mut self) {
        self.hit = Some(0.0);
    }

    pub fn slow_mo(&mut self) {
        self.slow = SLOWMO_SECS;
        self.hit = None;
    }

    /// Advance by real seconds (0 while paused).
    pub fn advance(&mut self, dt: f32) {
        if let Some(t) = self.flash.as_mut() {
            *t += dt;
            if *t >= FLASH_SECS {
                self.flash = None;
            }
        }
        if let Some(t) = self.hit.as_mut() {
            *t += dt;
            if *t >= HIT_HOLD + HIT_EASE {
                self.hit = None;
            }
        }
        self.slow = (self.slow - dt).max(0.0);
    }

    /// What to multiply the sim's time by.
    pub fn time_scale(&self) -> f32 {
        if self.slow > 0.0 {
            return SLOWMO_SCALE;
        }
        match self.hit {
            Some(t) if t < HIT_HOLD => HIT_SCALE,
            Some(t) => HIT_SCALE + (1.0 - HIT_SCALE) * art::smooth(((t - HIT_HOLD) / HIT_EASE).clamp(0.0, 1.0)),
            None => 1.0,
        }
    }

    /// White flash alpha 0..1; `shake_scale` is the reduced-shake factor (1 normal, 0 off).
    pub fn flash_alpha(&self, shake_scale: f32) -> f32 {
        match self.flash {
            Some(t) => FLASH_ALPHA * (1.0 - t / FLASH_SECS).clamp(0.0, 1.0) * shake_scale.clamp(0.0, 1.0),
            None => 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_is_neutral() {
        let i = Impact::new();
        assert_eq!(i.time_scale(), 1.0);
        assert_eq!(i.flash_alpha(1.0), 0.0);
    }

    #[test]
    fn flash_fades_in_a_fifth_of_a_second_and_respects_reduced_shake() {
        let mut i = Impact::new();
        i.flash();
        assert!((i.flash_alpha(1.0) - 0.30).abs() < 1e-6);
        assert!((i.flash_alpha(0.18) - 0.054).abs() < 1e-4);
        assert_eq!(i.flash_alpha(0.0), 0.0);
        i.advance(0.1);
        assert!((i.flash_alpha(1.0) - 0.15).abs() < 1e-4);
        i.advance(0.11);
        assert_eq!(i.flash_alpha(1.0), 0.0);
    }

    #[test]
    fn hit_stop_holds_then_eases_back() {
        let mut i = Impact::new();
        i.hit_stop();
        assert_eq!(i.time_scale(), HIT_SCALE);
        i.advance(0.09);
        assert_eq!(i.time_scale(), HIT_SCALE);
        i.advance(0.11); // 0.20 s in: halfway through the ease
        let s = i.time_scale();
        assert!(s > 0.4 && s < 0.7, "{s}");
        i.advance(0.2);
        assert_eq!(i.time_scale(), 1.0);
    }

    #[test]
    fn slow_motion_lasts_about_nine_tenths() {
        let mut i = Impact::new();
        i.hit_stop();
        i.slow_mo();
        assert_eq!(i.time_scale(), SLOWMO_SCALE);
        i.advance(0.85);
        assert_eq!(i.time_scale(), SLOWMO_SCALE);
        i.advance(0.1);
        assert_eq!(i.time_scale(), 1.0);
    }

    #[test]
    fn a_paused_frame_changes_nothing() {
        let mut i = Impact::new();
        i.hit_stop();
        i.advance(0.0);
        assert_eq!(i.time_scale(), HIT_SCALE);
    }
}
