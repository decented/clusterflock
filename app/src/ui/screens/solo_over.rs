//! Solo game over: the distance big, coins and best beneath it, a new-best
//! celebration, and one big "Run again" that restarts at once, a Share button and Home.

use super::common;
use super::run::RunScreen;
use super::{Ctx, Screen, Transition};
use crate::ui::art::{self, ease_out_back, ease_out_cubic, fade, phase, rgba};
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, Btn};
use crate::ui::{feedback, theme};
use egui::{pos2, vec2, Align2, Rect, Stroke};

/// What the game over of a daily run adds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DailyOver {
    /// The daily block.
    pub height: u32,
    /// Its seed, for "Run again".
    pub seed: u64,
    /// Today's best before this run (0 = none).
    pub prev_best: u32,
    /// Today's best now.
    pub best: u32,
    /// Attempts on this block, this one included.
    pub attempts: u32,
}

pub struct SoloOverScreen {
    t: f32,
    distance: u32,
    coins: u32,
    /// The best before this run.
    prev_best: u32,
    new_best: bool,
    daily: Option<DailyOver>,
}

impl SoloOverScreen {
    pub fn new(distance: u32, coins: u32, prev_best: u32, new_best: bool) -> SoloOverScreen {
        SoloOverScreen { t: 0.0, distance, coins, prev_best, new_best, daily: None }
    }

    /// The game over of a daily run: the best is today's, on this block.
    pub fn daily(distance: u32, coins: u32, d: DailyOver) -> SoloOverScreen {
        SoloOverScreen { t: 0.0, distance, coins, prev_best: d.prev_best, new_best: distance > d.prev_best, daily: Some(d) }
    }

    /// Staged for `--shot`: a daily run that beat today's best.
    pub fn sample_daily() -> SoloOverScreen {
        SoloOverScreen::daily(640, 31, DailyOver { height: 921_344, seed: 0x4b9c_c6b9_ba92_b55f, prev_best: 512, best: 640, attempts: 3 })
    }

    /// Staged for `--shot`: a new best.
    pub fn sample() -> SoloOverScreen {
        SoloOverScreen::new(1286, 47, 1042, true)
    }

    /// Staged: not a best.
    pub fn sample_plain() -> SoloOverScreen {
        SoloOverScreen::new(512, 9, 1042, false)
    }
}

/// The text the Share button hands to the share sheet. No link: there is no
/// public download page to point at.
pub fn share_message(distance: u32, coins: u32, new_best: bool) -> String {
    if new_best {
        format!("New best! I ran {distance} m in Cluster Flock ({coins} coins). Beat that!")
    } else {
        format!("I ran {distance} m in Cluster Flock and grabbed {coins} coins. Beat that!")
    }
}

/// The Share text of a daily run.
pub fn daily_share_message(height: u32, distance: u32) -> String {
    format!("Cluster Flock daily (block {height}): {distance} m. Same track for everyone today. Beat that!")
}

/// Where the content sits inside `safe` (pure, for tests): (card, again, share, home).
pub fn layout(safe: Rect) -> (Rect, Rect, Rect, Rect) {
    let w = (safe.width() - 40.0).min(340.0);
    let cx = safe.center().x;
    let home_h = 50.0;
    let again_h = 72.0;
    let home = Rect::from_min_size(pos2(cx - w / 2.0, safe.bottom() - 24.0 - home_h), vec2(w, home_h));
    let share = Rect::from_min_size(pos2(cx - w / 2.0, home.top() - 10.0 - home_h), vec2(w, home_h));
    let again = Rect::from_min_size(pos2(cx - w / 2.0, share.top() - 14.0 - again_h), vec2(w, again_h));
    let space = (again.top() - 20.0) - (safe.top() + 64.0);
    let h = space.min(318.0);
    let top = safe.top() + 64.0 + (space - h) * 0.2;
    let card = Rect::from_min_size(pos2(cx - w / 2.0, top), vec2(w, h));
    (card, again, share, home)
}

impl Screen for SoloOverScreen {
    fn name(&self) -> &'static str {
        "solo_over"
    }

    fn back(&mut self, ctx: &mut Ctx) -> Transition {
        feedback::back(ctx);
        Transition::PopTo("home")
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        common::begin(ctx, &mut self.t);
        Transition::None
    }

    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        let safe = ctx.viewport.safe;
        let screen = ctx.viewport.screen;
        let (t, tm) = (self.t, ctx.time as f32);
        let mut out = Transition::None;
        let (card, again, share, home) = layout(safe);
        let (dist, coins, prev, nb) = (self.distance, self.coins, self.prev_best, self.new_best);
        let daily = self.daily;
        let look = ctx.profile.look();
        common::page(screen, tm, ui, 0.45, |ui, brand| {
            let p = ui.painter().clone();
            if nb {
                widgets::confetti(&p, brand, screen, tm, 36);
            }
            // title
            let e = ease_out_back(phase(t, 0.05, 0.4));
            widgets::heading(&p, pos2(safe.center().x, safe.top() + 30.0), Align2::CENTER_CENTER, if nb { "NEW BEST!" } else { "RUN OVER" }, 34.0 * (0.7 + 0.3 * e), if nb { theme::GOLD } else { theme::TEXT });
            // card
            widgets::staged(ui, ease_out_cubic(phase(t, 0.15, 0.4)), vec2(0.0, 30.0), |ui, off| {
                let p = ui.painter().clone();
                let c = card.translate(off);
                widgets::panel(&p, c, 24.0);
                if nb {
                    p.rect_stroke(c.expand(1.5), 25.0, Stroke::new(3.0_f32, fade(theme::GOLD, 0.6 + 0.4 * (tm * 4.0).sin().abs())), egui::StrokeKind::Outside);
                }
                let caption = match daily {
                    Some(d) => format!("DAILY RUN \u{b7} block {}", crate::daily::thousands(d.height)),
                    None => "DISTANCE".to_string(),
                };
                p.text(pos2(c.center().x, c.top() + 34.0), Align2::CENTER_CENTER, caption, theme::ui_bold(14.0), if daily.is_some() { theme::GOLD } else { theme::TEXT_DIM });
                // count up
                let k = ease_out_cubic(phase(t, 0.35, 0.9));
                let shown = (dist as f32 * k).round() as u32;
                let size = art::fit_size(&p, &format!("{shown} m"), theme::title, 84.0, c.width() - 32.0);
                art::outlined(&p, pos2(c.center().x, c.top() + 104.0), Align2::CENTER_CENTER, &format!("{shown} m"), theme::title(size), if nb { theme::GOLD } else { theme::TEXT }, theme::OUTLINE, size * 0.07, size * 0.05);
                p.hline((c.left() + 28.0)..=(c.right() - 28.0), c.top() + 164.0, Stroke::new(1.0_f32, rgba(255, 255, 255, 0.12)));
                // coins and best
                let colw = c.width() / 2.0;
                let y = c.top() + 214.0;
                art::coin(&p, brand, pos2(c.left() + colw / 2.0 - 34.0, y - 4.0), 38.0, None, 1.0);
                art::outlined(&p, pos2(c.left() + colw / 2.0 - 8.0, y - 4.0), Align2::LEFT_CENTER, &format!("{}", (coins as f32 * k).round() as u32), theme::title(34.0), theme::SAND, theme::OUTLINE, 2.5, 1.5);
                p.text(pos2(c.left() + colw / 2.0, y + 30.0), Align2::CENTER_CENTER, "COINS", theme::ui_bold(12.0), theme::TEXT_DIM);
                let best = if nb { dist } else { prev };
                icons_trophy(&p, pos2(c.left() + colw * 1.5 - 40.0, y - 4.0), 34.0);
                art::outlined(&p, pos2(c.left() + colw * 1.5 - 18.0, y - 4.0), Align2::LEFT_CENTER, &format!("{best} m"), theme::title(26.0), theme::TEXT, theme::OUTLINE, 2.2, 1.5);
                p.text(pos2(c.left() + colw * 1.5, y + 30.0), Align2::CENTER_CENTER, if daily.is_some() { "BEST TODAY" } else { "BEST" }, theme::ui_bold(12.0), theme::TEXT_DIM);
                if let Some(d) = daily {
                    let mut s = format!("attempt {}", d.attempts);
                    if nb && prev > 0 {
                        s = format!("+{} m on today's best  \u{b7}  {s}", dist - prev);
                    }
                    p.text(pos2(c.center().x, c.bottom() - 26.0), Align2::CENTER_CENTER, s, theme::ui_bold(13.0), if nb && prev > 0 { theme::MINT } else { theme::TEXT_DIM });
                } else if nb && prev > 0 {
                    let s = format!("+{} m on your old best", dist - prev);
                    p.text(pos2(c.center().x, c.bottom() - 26.0), Align2::CENTER_CENTER, s, theme::ui_bold(13.0), theme::MINT);
                }
            });
            widgets::staged(ui, ease_out_cubic(phase(t, 0.5, 0.4)), vec2(0.0, 30.0), |ui, off| {
                if widgets::button(ui, again.translate(off), "over-again", &Btn::primary("Run again").icon(Icon::Restart).size(30.0)).clicked() {
                    feedback::select(ctx);
                    out = Transition::Cut(match daily {
                        Some(d) => Box::new(RunScreen::daily(d.seed, d.height, look)),
                        None => Box::new(RunScreen::solo_as(look)),
                    });
                }
                if widgets::button(ui, share.translate(off), "over-share", &Btn::ghost("Share").icon(Icon::Share).size(20.0)).clicked() {
                    feedback::tap(ctx);
                    let text = match daily {
                        Some(d) => daily_share_message(d.height, dist),
                        None => share_message(dist, coins, nb),
                    };
                    let r = ctx.platform.share_text(&text);
                    if let Some(m) = crate::platform::os::share_note(&*ctx.platform, r) {
                        ctx.say(m);
                    }
                }
                if widgets::button(ui, home.translate(off), "over-home", &Btn::ghost("Home").icon(Icon::Home).size(20.0)).clicked() {
                    feedback::back(ctx);
                    out = Transition::PopTo("home");
                }
            });
        });
        out
    }
}

fn icons_trophy(p: &egui::Painter, c: egui::Pos2, s: f32) {
    crate::ui::icons::draw(p, Icon::Trophy, c, s, theme::GOLD);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::viewport::{Insets, Viewport};

    #[test]
    fn layout_fits_phones() {
        for (w, h) in [(360.0, 800.0), (412.0, 915.0), (360.0, 640.0)] {
            let s = Viewport::new(vec2(w, h), 2.625, Insets { top: 48.0, bottom: 16.0, left: 0.0, right: 0.0 }).safe;
            let (card, again, share, home) = layout(s);
            for r in [card, again, share, home] {
                assert!(s.contains_rect(r), "{r:?} in {s:?}");
            }
            assert!(!card.intersects(again) && !again.intersects(share) && !share.intersects(home));
            assert!(card.height() >= 255.0, "card tall enough for its content at {w}x{h}: {}", card.height());
            assert!(again.height() >= 56.0);
        }
    }

    #[test]
    fn daily_share_text() {
        assert_eq!(daily_share_message(921344, 512), "Cluster Flock daily (block 921344): 512 m. Same track for everyone today. Beat that!");
    }

    #[test]
    fn share_text_is_plain_and_has_no_link() {
        assert_eq!(share_message(512, 9, false), "I ran 512 m in Cluster Flock and grabbed 9 coins. Beat that!");
        assert_eq!(share_message(1286, 47, true), "New best! I ran 1286 m in Cluster Flock (47 coins). Beat that!");
        assert!(!share_message(1, 0, true).contains("http") && !share_message(1, 0, false).contains("://"));
    }
}
