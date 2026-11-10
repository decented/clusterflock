//! Shared plumbing for the screens: the page frame (backdrop + panel), the
//! wordmark and the question card.

use crate::ui::art::{self, Brand, Scene};
use crate::ui::screens::Ctx;
use crate::ui::widgets::Btn;
use crate::ui::{theme, widgets};
use egui::{Color32, LayerId, Pos2, Rect, Ui};
use std::sync::Arc;

/// Shared choice between side-by-side landscape panels and a portrait stack.
#[derive(Clone, Copy)]
pub struct Layout {
    pub portrait: bool,
}

impl Layout {
    pub fn new(safe: Rect) -> Self { Self { portrait: safe.width() < safe.height() } }

    /// First panel on the left or above the second. Heights are points in portrait.
    pub fn split(self, area: Rect, right_width: f32, top_height: f32, gap: f32) -> (Rect, Rect) {
        if self.portrait {
            let y = area.top() + top_height.min((area.height() - gap).max(0.0));
            (Rect::from_min_max(area.min, egui::pos2(area.right(), y)),
             Rect::from_min_max(egui::pos2(area.left(), y + gap), area.max))
        } else {
            (Rect::from_min_max(area.min, egui::pos2(area.right() - right_width - gap, area.bottom())),
             Rect::from_min_max(egui::pos2(area.right() - right_width, area.top()), area.max))
        }
    }
}

/// Scroll a hand-painted page, clipping both its drawing and its input to the body.
pub fn scroll<R>(ui: &mut Ui, area: Rect, id: &str, height: f32, add: impl FnOnce(&mut Ui, Rect) -> R) -> R {
    ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
        ui.set_clip_rect(area.intersect(ui.clip_rect()));
        egui::ScrollArea::vertical().id_salt(id).auto_shrink([false, false]).show(ui, |ui| {
            let rect = Rect::from_min_size(ui.next_widget_position(), egui::vec2(ui.available_width(), height.max(area.height())));
            ui.allocate_rect(rect, egui::Sense::hover());
            add(ui, rect)
        }).inner
    }).inner
}

/// Per-screen frame prologue: audio/haptics boot and the entry clock.
pub fn begin(ctx: &mut Ctx, t: &mut f32) {
    crate::ui::feedback::boot(ctx);
    *t += ctx.dt;
}

/// Paint the backdrop for a screen and run `add` in a transparent central panel.
/// `veil` darkens the scenery (0 = full sunshine, ~0.4 = a quiet settings page).
pub fn page(screen: Rect, time: f32, ui: &egui::Context, veil: f32, add: impl FnOnce(&mut Ui, &Arc<Brand>)) {
    crate::ui::widgets::set_over_3d(false);
    let brand = art::brand(ui);
    let p = ui.layer_painter(LayerId::background());
    art::backdrop(&p, &brand, screen, Scene { t: time, sun: 0.0, veil, night: 0.0, horizon: true });
    #[allow(deprecated)]
    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| add(ui, &brand));
}

/// The "CLUSTER FLOCK" wordmark on two lines: Lilita One with a sky-blue to
/// teal gradient fill and a deep-navy outline. `size` is the cap height of
/// one line; returns the rect both lines cover.
pub fn wordmark(p: &egui::Painter, centre: Pos2, size: f32, alpha: f32) -> Rect {
    let line = |text: &str, c: Pos2| {
        art::gradient_text(p, c, text, theme::title(size), Color32::from_rgb(0xFF, 0xF1, 0xA8), Color32::from_rgb(0xFF, 0x9E, 0x3D), Color32::from_rgb(0x1B, 0x2A, 0x5C), size * 0.085, alpha)
    };
    let gap = size * 0.92;
    let top = line("CLUSTER", centre - egui::vec2(0.0, gap / 2.0));
    let bottom = line("FLOCK", centre + egui::vec2(0.0, gap / 2.0));
    top.union(bottom)
}

/// The two buttons of a [`prompt`] of height `h` on `screen`: (left, right).
pub fn prompt_buttons(screen: Rect, h: f32) -> (Rect, Rect) {
    use egui::{pos2, vec2};
    let card = Rect::from_center_size(screen.center(), vec2(screen.width().min(460.0), h));
    let bw = (card.width() - 48.0 - widgets::S3) / 2.0;
    let by = card.bottom() - 18.0 - 44.0;
    (Rect::from_min_size(pos2(card.left() + 24.0, by), vec2(bw, 44.0)), Rect::from_min_size(pos2(card.left() + 24.0 + bw + widgets::S3, by), vec2(bw, 44.0)))
}

/// A question card over a dimmed screen. `Some(true)` = the right (primary)
/// button, `Some(false)` = the left one, `None` = still asking.
#[allow(clippy::too_many_arguments)]
pub fn prompt(ui: &egui::Context, screen: Rect, id: &str, h: f32, title: &str, msg: &str, left: &str, right: &str) -> Option<bool> {
    use egui::{pos2, vec2, Align2, Color32};
    let dim = ui.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new((id, "dim"))));
    dim.rect_filled(screen, 0.0, Color32::from_black_alpha(170));
    let card = Rect::from_center_size(screen.center(), vec2(screen.width().min(460.0), h));
    let (lr, rr) = prompt_buttons(screen, h);
    let mut answer = None;
    egui::Area::new(egui::Id::new(id)).order(egui::Order::Foreground).fixed_pos(card.min).show(ui, |ui| {
        ui.set_min_size(card.size());
        let p = ui.painter().clone();
        widgets::panel_solid(&p, card, 22.0);
        p.text(pos2(card.center().x, card.top() + 34.0), Align2::CENTER_CENTER, title, theme::title(24.0), theme::TEXT);
        let g = p.layout(msg.to_string(), theme::ui_medium(14.0), theme::TEXT_DIM, card.width() - 48.0);
        p.galley(pos2(card.left() + 24.0, card.top() + 64.0), g, theme::TEXT_DIM);
        if widgets::button(ui, lr, &format!("{id}-left"), &Btn::new(left)).clicked() {
            answer = Some(false);
        }
        if widgets::button(ui, rr, &format!("{id}-right"), &Btn::primary(right)).clicked() {
            answer = Some(true);
        }
    });
    answer
}


/// A loose flock of little "V" birds drifting across `rect` (the title and
/// splash backdrop). Purely decorative; `alpha` fades it in and out.
pub fn drift_flock(p: &egui::Painter, rect: Rect, t: f32, alpha: f32) {
    use egui::{pos2, vec2, Shape, Stroke};
    for i in 0..14u32 {
        let h = |k: u32| art::hash01(i.wrapping_mul(31).wrapping_add(k));
        let speed = 14.0 + 22.0 * h(1);
        let span = rect.width() + 80.0;
        let x = rect.left() - 40.0 + (h(2) * span + t * speed) % span;
        let y = rect.top() + rect.height() * (0.10 + 0.34 * h(3)) + (t * (0.8 + h(4)) + h(5) * 6.0).sin() * 6.0;
        let size = 5.0 + 6.0 * h(6);
        let flap = (t * (5.0 + 3.0 * h(7)) + h(8) * 6.0).sin() * 0.45;
        let c = pos2(x, y);
        let col = art::fade(Color32::from_rgb(0x1B, 0x2A, 0x5C), alpha * (0.45 + 0.4 * h(9)));
        let wing = vec2(size, -size * (0.35 + flap));
        p.add(Shape::line(vec![c + vec2(-wing.x, wing.y), c, c + wing], Stroke::new(1.6_f32, col)));
    }
}
