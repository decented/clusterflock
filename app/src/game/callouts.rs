//! Floating words over the run (streaks, milestones, near misses): one at a
//! time, in the same display type as the countdown. A word pops in with a quick
//! overshoot, holds, then rises and fades. Presentation only.

use crate::ui::art::{self, ease_out_back, fade, phase};
use crate::ui::theme;
use egui::{pos2, Align2, Painter, Rect};

/// Seconds the pop-in overshoot takes.
pub const POP: f32 = 0.12;
/// The word holds until here, then rises and fades.
pub const HOLD_END: f32 = 0.50;
/// Total life.
pub const TOTAL: f32 = 0.90;
/// Points the word rises while fading.
pub const RISE: f32 = 40.0;
/// Where the word sits, as a fraction of the screen height (clear of the bird and the lane near it).
pub const AT: f32 = 0.30;

const BIG: f32 = 64.0;
const SMALL: f32 = 36.0;

struct Callout {
    text: String,
    big: bool,
    age: f32,
}

#[derive(Default)]
pub struct Callouts {
    cur: Option<Callout>,
}

/// (scale, alpha, rise in points) at `age` seconds.
pub fn look(age: f32) -> (f32, f32, f32) {
    let scale = 0.6 + 0.4 * ease_out_back(phase(age, 0.0, POP));
    let k = art::smooth(phase(age, HOLD_END, TOTAL - HOLD_END));
    (scale, 1.0 - k, RISE * k)
}

impl Callouts {
    pub fn new() -> Callouts {
        Callouts::default()
    }

    /// Show `text`. A new word replaces the old one, except that a small word
    /// never cuts short a big one that is still holding.
    pub fn push(&mut self, text: impl Into<String>, big: bool) {
        if let Some(c) = &self.cur {
            if c.big && !big && c.age < HOLD_END {
                return;
            }
        }
        self.cur = Some(Callout { text: text.into(), big, age: 0.0 });
    }

    pub fn update(&mut self, dt: f32) {
        if let Some(c) = self.cur.as_mut() {
            c.age += dt;
            if c.age >= TOTAL {
                self.cur = None;
            }
        }
    }

    pub fn text(&self) -> Option<&str> {
        self.cur.as_ref().map(|c| c.text.as_str())
    }

    /// Paint the word centred at [`AT`] of the screen's height.
    pub fn draw(&self, p: &Painter, screen: Rect) {
        let Some(c) = &self.cur else { return };
        let (scale, a, rise) = look(c.age);
        let base = if c.big { BIG } else { SMALL };
        // long words shrink to fit the width, overshoot included
        let w = p.layout_no_wrap(c.text.clone(), theme::title(base), egui::Color32::WHITE).size().x.max(1.0);
        let fit = (screen.width() * 0.9 / (w * 1.15)).min(1.0);
        let size = base * fit * scale;
        let at = pos2(screen.center().x, screen.top() + screen.height() * AT - rise);
        let (fill, line) = (if c.big { theme::GOLD } else { theme::TEXT }, theme::OUTLINE);
        art::outlined(p, at, Align2::CENTER_CENTER, &c.text, theme::title(size), fade(fill, a), fade(line, a), (size * 0.09).max(2.0), (size * 0.05).max(1.5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pops_holds_rises_and_fades() {
        let (s0, a0, r0) = look(0.0);
        assert!(s0 < 0.7 && a0 == 1.0 && r0 == 0.0);
        let (s, _, _) = look(POP * 0.7);
        assert!(s > 1.0, "overshoots on the way in, got {s}");
        let (s, a, r) = look(0.35);
        assert!((s - 1.0).abs() < 1e-3 && a == 1.0 && r == 0.0, "holds");
        let (_, a, r) = look(0.7);
        assert!(a < 1.0 && a > 0.0 && r > 0.0 && r < RISE, "rising and fading");
        let (_, a, r) = look(TOTAL);
        assert!(a < 0.01 && (r - RISE).abs() < 0.1);
    }

    #[test]
    fn expires() {
        let mut c = Callouts::new();
        c.push("NICE!", true);
        c.update(TOTAL - 0.01);
        assert!(c.text().is_some());
        c.update(0.02);
        assert!(c.text().is_none());
    }

    #[test]
    fn a_new_word_replaces_the_old_but_small_never_cuts_a_holding_big() {
        let mut c = Callouts::new();
        c.push("250 m", false);
        c.push("NICE!", true);
        assert_eq!(c.text(), Some("NICE!"));
        c.update(0.2);
        c.push("CLOSE!", false);
        assert_eq!(c.text(), Some("NICE!"));
        c.push("SMOOTH!", true);
        assert_eq!(c.text(), Some("SMOOTH!"));
        c.update(0.6);
        c.push("CLOSE!", false);
        assert_eq!(c.text(), Some("CLOSE!"));
    }
}
