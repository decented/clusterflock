//! First-run gesture hints: three pills in turn (lane, jump, roll), each with an
//! arrow sliding the way to swipe. A hint goes when the player does the thing,
//! or after [`HINT_SECS`].

use crate::ui::art::{fade, phase};
use crate::ui::theme;
use crate::ui::art::rgba;
use egui::{pos2, vec2, Painter, Pos2, Rect, Stroke, Vec2};
use flock_core::Command;

pub const HINT_SECS: f32 = 5.0;
/// Seconds a hint takes to fade in and out.
const FADE: f32 = 0.25;
/// Where the pill sits, as a fraction of the screen height (spec asked for 0.62, but the bird
/// stands there; this is just below its feet).
pub const AT: f32 = 0.76;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gesture {
    Lane,
    Jump,
    Roll,
}

impl Gesture {
    pub const ALL: [Gesture; 3] = [Gesture::Lane, Gesture::Jump, Gesture::Roll];

    pub fn text(self) -> &'static str {
        match self {
            Gesture::Lane => "Swipe left or right to change lane",
            Gesture::Jump => "Swipe up to jump",
            Gesture::Roll => "Swipe down to roll under gates",
        }
    }

    /// Does `c` count as doing this (diagonals do both of their halves)?
    pub fn done_by(self, c: Command) -> bool {
        use Command::*;
        match self {
            Gesture::Lane => matches!(c, Left | Right | JumpLeft | JumpRight | DiveLeft | DiveRight),
            Gesture::Jump => matches!(c, Jump | JumpLeft | JumpRight),
            Gesture::Roll => matches!(c, Down | DiveLeft | DiveRight),
        }
    }
}

#[derive(Debug)]
pub struct Hints {
    step: usize,
    /// Seconds this hint has been up.
    age: f32,
    /// Fading out (seconds so far) once done.
    out: Option<f32>,
    /// Total seconds, for the arrow loop.
    clock: f32,
}

impl Default for Hints {
    fn default() -> Self {
        Hints::new()
    }
}

impl Hints {
    pub fn new() -> Hints {
        Hints { step: 0, age: 0.0, out: None, clock: 0.0 }
    }

    /// Start at hint `step` (staging).
    pub fn at_step(step: usize) -> Hints {
        Hints { step, ..Hints::new() }
    }

    pub fn current(&self) -> Option<Gesture> {
        Gesture::ALL.get(self.step).copied()
    }

    /// Advance by `dt` real seconds with the commands the player issued this
    /// frame. Returns true the moment the last hint is gone.
    pub fn update(&mut self, dt: f32, cmds: &[Command]) -> bool {
        let Some(g) = self.current() else { return false };
        self.clock += dt;
        match self.out.as_mut() {
            Some(o) => {
                *o += dt;
                if *o >= FADE {
                    self.step += 1;
                    self.age = 0.0;
                    self.out = None;
                    return self.step >= Gesture::ALL.len();
                }
            }
            None => {
                self.age += dt;
                if self.age >= HINT_SECS || cmds.iter().any(|c| g.done_by(*c)) {
                    self.out = Some(0.0);
                }
            }
        }
        false
    }

    fn alpha(&self) -> f32 {
        let a_in = phase(self.age, 0.0, FADE);
        let a_out = 1.0 - self.out.map_or(0.0, |o| (o / FADE).clamp(0.0, 1.0));
        a_in * a_out
    }

    /// Paint the pill at [`AT`] of the screen's height.
    pub fn draw(&self, p: &Painter, screen: Rect) {
        let Some(g) = self.current() else { return };
        let a = self.alpha();
        if a <= 0.0 {
            return;
        }
        let galley = p.layout_no_wrap(g.text().to_string(), theme::ui_bold(17.0), theme::TEXT);
        let slot = 44.0;
        let size = vec2(slot + galley.size().x + 34.0, 48.0);
        let slide = (1.0 - phase(self.age, 0.0, FADE)) * 10.0;
        let c = pos2(screen.center().x, screen.top() + screen.height() * AT + slide);
        let r = Rect::from_center_size(c, size);
        p.rect_filled(r, r.height() / 2.0, fade(rgba(14, 6, 30, 0.82), a));
        p.rect_stroke(r, r.height() / 2.0, Stroke::new(1.5_f32, fade(theme::GOLD, a * 0.7)), egui::StrokeKind::Inside);
        let arrow_c = pos2(r.left() + 16.0 + slot / 2.0, r.center().y);
        self.draw_arrow(p, g, arrow_c, a);
        p.galley_with_override_text_color(pos2(r.left() + 16.0 + slot, r.center().y - galley.size().y / 2.0), galley, fade(theme::TEXT, a));
    }

    /// An arrow that slides the swipe's way and loops (lane: left, then right).
    fn draw_arrow(&self, p: &Painter, g: Gesture, c: Pos2, a: f32) {
        let period = 1.1;
        let (dir, ph): (Vec2, f32) = match g {
            Gesture::Lane => {
                let k = self.clock / period;
                (if (k as u32) % 2 == 0 { vec2(-1.0, 0.0) } else { vec2(1.0, 0.0) }, k.fract())
            }
            Gesture::Jump => (vec2(0.0, -1.0), (self.clock / period).fract()),
            Gesture::Roll => (vec2(0.0, 1.0), (self.clock / period).fract()),
        };
        let slide = (ph * 2.0 - 1.0) * 11.0;
        let alpha = a * (3.0 * (std::f32::consts::PI * ph).sin()).min(1.0);
        let tip = c + dir * (slide + 9.0);
        let tail = c + dir * (slide - 9.0);
        let side = vec2(-dir.y, dir.x);
        let st = Stroke::new(4.0_f32, fade(theme::GOLD, alpha));
        p.line_segment([tail, tip], st);
        p.line_segment([tip, tip - dir * 8.0 + side * 7.0], st);
        p.line_segment([tip, tip - dir * 8.0 - side * 7.0], st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_hint_goes_when_the_player_does_it() {
        let mut h = Hints::new();
        assert_eq!(h.current(), Some(Gesture::Lane));
        h.update(0.1, &[Command::Jump]); // the wrong thing
        h.update(0.1, &[]);
        assert_eq!(h.current(), Some(Gesture::Lane));
        h.update(0.1, &[Command::Left]);
        h.update(0.3, &[]); // fade out
        assert_eq!(h.current(), Some(Gesture::Jump));
        h.update(0.1, &[Command::Jump]);
        h.update(0.3, &[]);
        assert_eq!(h.current(), Some(Gesture::Roll));
        h.update(0.1, &[Command::Down]);
        assert!(h.update(0.3, &[]), "finished after the third");
        assert_eq!(h.current(), None);
    }

    #[test]
    fn a_hint_times_out_after_five_seconds() {
        let mut h = Hints::new();
        for _ in 0..49 {
            h.update(0.1, &[]);
        }
        assert_eq!(h.current(), Some(Gesture::Lane));
        h.update(0.2, &[]);
        h.update(0.3, &[]);
        assert_eq!(h.current(), Some(Gesture::Jump));
    }

    #[test]
    fn walking_through_all_three_by_timeout_finishes() {
        let mut h = Hints::new();
        let mut done = false;
        for _ in 0..200 {
            done |= h.update(0.1, &[]);
        }
        assert!(done);
    }

    #[test]
    fn diagonals_count_for_both_halves() {
        assert!(Gesture::Lane.done_by(Command::JumpLeft) && Gesture::Jump.done_by(Command::JumpLeft));
        assert!(Gesture::Lane.done_by(Command::DiveRight) && Gesture::Roll.done_by(Command::DiveRight));
        assert!(!Gesture::Roll.done_by(Command::Jump));
    }
}
