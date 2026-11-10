//! Flock Practice setup: how many bots (1-5) and how good they are, then the
//! run (the multiplayer rules offline: knocked out, you become a ghost and
//! build against the others).

use super::common;
use super::run::RunScreen;
use super::{Ctx, Screen, Transition};
use crate::ui::art::{self, ease_out_cubic, phase, rgba};
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, Btn, Seg, S3, S4};
use crate::ui::{feedback, theme};
use egui::{pos2, vec2, Align2, Rect, Stroke};
use flock_render::PlayerStyle;

pub struct PracticeScreen {
    t: f32,
    bots: u8,
    skill: usize,
}

impl Default for PracticeScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl PracticeScreen {
    pub fn new() -> PracticeScreen {
        PracticeScreen { t: 0.0, bots: 3, skill: 1 }
    }
}

/// The colour of seat `i` in a practice flock: you wear yours, the bots the
/// other colours in order (the same rule as `LocalRun::practice_styled`).
pub fn flock_colour(i: usize, mine: u8) -> u8 {
    if i == 0 {
        mine % 6
    } else {
        (0..6u8).filter(|c| *c != mine % 6).nth(i - 1).unwrap_or(0)
    }
}

const SKILLS: [&str; 3] = ["Easy", "Normal", "Hard"];
const BLURB: [&str; 3] = ["Bots stumble a lot. Good for learning the ropes.", "Bots run well but still bump into each other.", "Bots rarely crash. Outlast them if you can."];

/// Layout (pure, for tests): (card, bots control, skill control, start button).
pub fn layout(body: Rect) -> (Rect, Rect, Rect, Rect, Rect) {
    let w = body.width().min(380.0);
    let x = body.center().x - w / 2.0;
    let start = Rect::from_min_size(pos2(x, body.bottom() - 72.0 - 6.0), vec2(w, 72.0));
    let top = body.top() + 4.0;
    let flock = Rect::from_min_size(pos2(x, top), vec2(w, 132.0));
    let bots = Rect::from_min_size(pos2(x + S4, flock.bottom() + 46.0), vec2(w - 2.0 * S4, 48.0));
    let skill = Rect::from_min_size(pos2(x + S4, bots.bottom() + 54.0), vec2(w - 2.0 * S4, 48.0));
    (flock, bots, skill, start, Rect::from_min_max(pos2(x, top), pos2(x + w, skill.bottom() + 52.0)))
}

impl Screen for PracticeScreen {
    fn name(&self) -> &'static str {
        "practice"
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        common::begin(ctx, &mut self.t);
        Transition::None
    }

    fn back(&mut self, ctx: &mut Ctx) -> Transition {
        feedback::back(ctx);
        Transition::Pop
    }

    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        let safe = ctx.viewport.safe;
        let (t, tm) = (self.t, ctx.time as f32);
        let mut out = Transition::None;
        let (bots, skill) = (self.bots, self.skill);
        let mine = ctx.profile.preferred_colour();
        let look = ctx.profile.look();
        let (mut new_bots, mut new_skill) = (bots, skill);
        common::page(ctx.viewport.screen, tm, ui, 0.5, |ui, _| {
            let hd = widgets::header(ui, safe, "Flock practice", widgets::enter(t, 0));
            if hd.back.clicked() {
                feedback::back(ctx);
                out = Transition::Pop;
            }
            let (flock, bots_r, skill_r, start, card) = layout(hd.body);
            let free = (start.top() - 16.0 - card.bottom()).max(0.0);
            let shift = vec2(0.0, free * 0.4);
            widgets::staged(ui, widgets::enter(t, 1), vec2(0.0, 24.0), |ui, off| {
                let off = off + shift;
                let p = ui.painter().clone();
                widgets::panel(&p, card.translate(off), 22.0);
                // the flock: you plus the bots, as colour discs with their accessory initial
                let fl = flock.translate(off);
                p.text(pos2(fl.center().x, fl.top() + 22.0), Align2::CENTER_CENTER, "THE FLOCK", theme::ui_bold(12.0), theme::TEXT_DIM);
                let n = bots as usize + 1;
                let step = ((fl.width() - 32.0) / 6.0).min(52.0);
                let x0 = fl.center().x - step * (n as f32 - 1.0) / 2.0;
                for i in 0..n {
                    let c = pos2(x0 + i as f32 * step, fl.top() + 68.0);
                    let s = PlayerStyle::of(flock_colour(i, mine));
                    let col = art::from_f3(s.body);
                    p.circle_filled(c + vec2(0.0, 3.0), step * 0.42, rgba(0, 0, 0, 0.3));
                    p.circle_filled(c, step * 0.42, col);
                    p.circle_stroke(c, step * 0.42, Stroke::new(if i == 0 { 3.0_f32 } else { 1.5_f32 }, if i == 0 { theme::GOLD } else { rgba(255, 255, 255, 0.5) }));
                    crate::ui::icons::draw(&p, Icon::Ostrich, c, step * 0.56, rgba(255, 255, 255, 0.92));
                }
                p.text(pos2(fl.center().x, fl.top() + 108.0), Align2::CENTER_CENTER, format!("You + {} bot{}", bots, if bots == 1 { "" } else { "s" }), theme::ui_bold(14.0), theme::TEXT);

                let br = bots_r.translate(off);
                p.text(pos2(br.left(), br.top() - 14.0), Align2::LEFT_CENTER, "Bots", theme::ui_bold(13.0), theme::TEXT_DIM);
                let labels = ["1", "2", "3", "4", "5"];
                let segs: Vec<Seg> = labels.iter().map(|l| Seg::new(l)).collect();
                if let Some(i) = widgets::segmented(ui, br, "prac-bots", &segs, bots as usize - 1) {
                    feedback::tap(ctx);
                    new_bots = i as u8 + 1;
                }
                let sr = skill_r.translate(off);
                p.text(pos2(sr.left(), sr.top() - 14.0), Align2::LEFT_CENTER, "Bot skill", theme::ui_bold(13.0), theme::TEXT_DIM);
                let segs: Vec<Seg> = SKILLS.iter().map(|l| Seg::new(l)).collect();
                if let Some(i) = widgets::segmented(ui, sr, "prac-skill", &segs, skill) {
                    feedback::tap(ctx);
                    new_skill = i;
                }
                p.text(pos2(sr.center().x, sr.bottom() + 26.0), Align2::CENTER_CENTER, BLURB[skill.min(2)], theme::ui_medium(12.5), theme::TEXT_DIM);
                let _ = (S3, ease_out_cubic(phase(t, 0.0, 0.1)));
            });
            widgets::staged(ui, widgets::enter(t, 3), vec2(0.0, 30.0), |ui, off| {
                if widgets::button(ui, start.translate(off), "prac-go", &Btn::primary("Start").icon(Icon::Play).size(30.0)).clicked() {
                    feedback::select(ctx);
                    out = Transition::Replace(Box::new(RunScreen::practice_as(bots, skill, look)));
                }
            });
        });
        self.bots = new_bots;
        self.skill = new_skill;
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::viewport::{Insets, Viewport};

    #[test]
    fn the_flock_has_each_colour_once() {
        for mine in 0..6u8 {
            let mut c: Vec<u8> = (0..6).map(|i| flock_colour(i, mine)).collect();
            assert_eq!(c[0], mine);
            c.sort();
            assert_eq!(c, vec![0, 1, 2, 3, 4, 5]);
        }
    }

    #[test]
    fn layout_fits_phones() {
        for (w, h) in [(360.0, 800.0), (412.0, 915.0), (360.0, 640.0)] {
            let s = Viewport::new(vec2(w, h), 2.625, Insets { top: 48.0, bottom: 16.0, left: 0.0, right: 0.0 }).safe;
            let body = Rect::from_min_max(pos2(s.left() + 16.0, s.top() + 8.0 + 2.0 + 44.0 + 12.0), pos2(s.right() - 16.0, s.bottom() - 16.0));
            let (_f, b, k, start, card) = layout(body);
            for r in [b, k, start, card] {
                assert!(body.contains_rect(r), "{r:?} in {body:?}");
            }
            assert!(card.bottom() < start.top() + 1.0, "card above the start button at {w}x{h}");
            assert!(start.height() >= 56.0);
        }
    }
}
