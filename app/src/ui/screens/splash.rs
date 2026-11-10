//! Splash (~2.4 s, tap to skip): the sky brightens, a flock drifts across, the
//! two-line wordmark pops in with a shock ring, then a fade to the title.

use super::common::{self, wordmark};
use super::{home::HomeScreen, Ctx, Screen, Transition};
use crate::ui::art::{self, ease_out_back, ease_out_cubic, fade, phase, rgba, Scene};
use crate::ui::{feedback, theme};
use egui::{pos2, Align2, Stroke};

pub const SPLASH_SECS: f32 = 2.4;

/// When the wordmark lands.
const POP: f32 = 0.85;

pub struct SplashScreen {
    t: f32,
}

impl Default for SplashScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl SplashScreen {
    pub fn new() -> SplashScreen {
        SplashScreen { t: 0.0 }
    }
}

impl Screen for SplashScreen {
    fn name(&self) -> &'static str {
        "splash"
    }

    fn on_background(&mut self, _ctx: &mut Ctx) {
        feedback::on_background();
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        common::begin(ctx, &mut self.t);
        let skip = !ctx.touches.began().is_empty() || ctx.keys.any_held(&[crate::platform::KeyCode::Space, crate::platform::KeyCode::Enter]);
        if self.t >= SPLASH_SECS || (skip && self.t > 0.3) {
            return Transition::Replace(Box::new(HomeScreen::new()));
        }
        Transition::None
    }

    /// Back skips to the title (it must not quit the app).
    fn back(&mut self, _ctx: &mut Ctx) -> Transition {
        Transition::Replace(Box::new(HomeScreen::new()))
    }

    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        let brand = art::brand(ui);
        let screen = ctx.viewport.screen;
        let safe = ctx.viewport.safe;
        let t = self.t;
        let p = ui.layer_painter(egui::LayerId::background());

        let rise = ease_out_cubic(phase(t, 0.0, 1.6));
        art::backdrop(&p, &brand, screen, Scene { t: ctx.time as f32, sun: -0.65 + 0.65 * rise, veil: 0.0, night: 0.0, horizon: true });
        common::drift_flock(&p, screen, t * 2.0, phase(t, 0.1, 0.6));

        // the wordmark pops in with a shock ring
        let k = t - POP;
        let size = (safe.width() * 0.24).min(safe.height() * 0.12);
        let centre = pos2(safe.center().x, safe.top() + safe.height() * 0.38);
        if k >= 0.0 {
            let e = ease_out_back(phase(t, POP, 0.30));
            let sc = art::lerp(2.4, 1.0, e);
            let rect = wordmark(&p, centre, size * sc, phase(t, POP, 0.08));
            if k < 0.6 {
                let rk = k / 0.6;
                p.circle_stroke(centre, size * (0.6 + 2.6 * ease_out_cubic(rk)), Stroke::new(5.0 * (1.0 - rk) + 0.5, rgba(255, 236, 190, 0.55 * (1.0 - rk))));
            }
            let tk = phase(t, POP + 0.5, 0.35);
            if tk > 0.0 {
                p.text(pos2(centre.x, rect.bottom() + size * 0.32 + (1.0 - ease_out_cubic(tk)) * 8.0), Align2::CENTER_CENTER, "stick together, fly far", theme::ui_bold(15.0), fade(theme::OUTLINE, tk));
            }
        }

        // credit
        let ck = phase(t, 1.4, 0.45);
        if ck > 0.0 {
            p.text(pos2(safe.center().x, safe.bottom() - 18.0), Align2::CENTER_CENTER, "a game by decented", theme::ui_bold(13.0), fade(theme::TEXT_DIM, ck * 0.9));
        }

        // up from black at the start, and out at the end
        let black = (1.0 - phase(t, 0.0, 0.4)).max(phase(t, SPLASH_SECS - 0.25, 0.25));
        if black > 0.0 {
            p.rect_filled(screen, 0.0, theme::BG.gamma_multiply(black));
        }
        Transition::None
    }
}
