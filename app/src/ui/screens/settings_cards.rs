//! The Settings cards below the panel: UPDATES (in-app updates) and MORE GAMES
//! (the entry to the catalog). Ported from Stampede's `settings_cards.rs` (which
//! also draws a RELAYS card; Cluster Flock has no relay settings, so that card
//! is not ported). Each card is drawn into a rectangle the screen lays out, and
//! reports what was tapped as a [`CardAction`]; the screen acts on it.
//!
//! The screen scrolls, so a row that does not lie wholly inside the visible
//! `view` is neither drawn nor tappable (no half rows under the header).

use crate::ui::theme;
use crate::ui::widgets::{self, Btn};
use nostr_release_kit::{UpdateTone, UpdateView};
use egui::{pos2, vec2, Align2, Color32, Rect, Stroke, Ui};

/// Height of a button row in a card.
const BTN_H: f32 = 42.0;
/// Card padding.
const PAD: f32 = 14.0;
/// The label line ("UPDATES") at the top of a card.
const LABEL_H: f32 = 22.0;

/// What a card asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum CardAction {
    CheckUpdates,
    InstallUpdate,
    CancelUpdate,
    MoreGames,
}

/// Is `r` wholly inside the visible area?
fn shown(view: Rect, r: Rect) -> bool {
    view.expand(0.5).contains_rect(r)
}

fn card_frame(ui: &Ui, card: Rect, label: &str, view: Rect, hot: bool) {
    let p = ui.painter();
    widgets::panel(p, card, 20.0);
    if hot {
        p.rect_stroke(card, egui::CornerRadius::same(20), Stroke::new(2.0_f32, theme::ACCENT), egui::StrokeKind::Inside);
    }
    let lr = Rect::from_min_size(card.min + vec2(PAD + 2.0, PAD - 2.0), vec2(card.width() - 2.0 * PAD, LABEL_H - 4.0));
    if shown(view, lr) {
        p.text(pos2(lr.left(), lr.center().y), Align2::LEFT_CENTER, label, theme::ui_bold(11.5), theme::SAND);
    }
}

fn tone_color(t: UpdateTone) -> Color32 {
    match t {
        UpdateTone::Dim => theme::TEXT_DIM,
        UpdateTone::Accent => theme::ACCENT,
        UpdateTone::Gold => theme::GOLD,
        UpdateTone::Danger => theme::LASER,
    }
}

/// The UPDATES card's height (the status and notes wrap, so it depends on `v`).
pub fn updates_height(p: &egui::Painter, width: f32, v: &UpdateView) -> f32 {
    let inner = width - 2.0 * PAD - 4.0;
    let status = p.layout(v.status.clone(), theme::ui_medium(13.0), Color32::WHITE, inner).size().y;
    let notes = v.offer.as_ref().filter(|o| !o.notes.is_empty()).map_or(0.0, |o| p.layout(o.notes.clone(), theme::ui_medium(12.0), Color32::WHITE, inner).size().y + 6.0);
    PAD + LABEL_H + 22.0 + 4.0 + status + 8.0 + notes + if v.progress.is_some() { 14.0 } else { 0.0 } + if v.offer.is_some() { BTN_H + 8.0 } else { 0.0 } + BTN_H + PAD
}

/// The UPDATES card: what is installed, where the check stands, the release
/// notes when one is on offer, download progress, and the buttons.
pub fn draw_updates(ui: &mut Ui, view: Rect, card: Rect, v: &UpdateView, t: f32, act: &mut Option<CardAction>) {
    card_frame(ui, card, "UPDATES", view, v.offer.is_some());
    let p = ui.painter().clone();
    let tone = tone_color(v.tone);
    let x = card.min.x + PAD + 2.0;
    let inner = card.width() - 2.0 * PAD - 4.0;
    let mut y = card.min.y + PAD + LABEL_H;
    let title = Rect::from_min_size(pos2(x, y), vec2(inner, 22.0));
    if shown(view, title) {
        p.text(pos2(x, y + 11.0), Align2::LEFT_CENTER, format!("Installed {}", v.installed), theme::ui_bold(15.0), theme::TEXT);
        if v.checking {
            widgets::dots(&p, pos2(card.max.x - 36.0, y + 11.0), 3, t * 3.0, theme::ACCENT);
        }
    }
    y += 22.0 + 4.0;
    let sg = p.layout(v.status.clone(), theme::ui_medium(13.0), tone, inner);
    let sh = sg.size().y;
    if shown(view, Rect::from_min_size(pos2(x, y), vec2(inner, sh))) {
        p.galley(pos2(x, y), sg, tone);
    }
    y += sh + 8.0;
    if let Some(o) = v.offer.as_ref().filter(|o| !o.notes.is_empty()) {
        let ng = p.layout(o.notes.clone(), theme::ui_medium(12.0), theme::TEXT_DIM, inner);
        let nh = ng.size().y;
        if shown(view, Rect::from_min_size(pos2(x, y - 6.0), vec2(inner, nh))) {
            p.galley(pos2(x, y - 6.0), ng, theme::TEXT_DIM);
        }
        y += nh;
    }
    if let Some(pr) = v.progress {
        let track = Rect::from_min_size(pos2(x, y - 2.0), vec2(inner, 6.0));
        if shown(view, track) {
            p.rect_filled(track, 3.0, Color32::from_white_alpha(28));
            p.rect_filled(Rect::from_min_size(track.min, vec2(track.width() * pr, 6.0)), 3.0, theme::ACCENT);
        }
        y += 14.0;
    }
    if let Some(o) = &v.offer {
        let b = Rect::from_min_size(pos2(x, y), vec2(inner, BTN_H));
        if shown(view, b) {
            if v.downloading {
                if widgets::button(ui, b, "update-cancel", &Btn::ghost("Cancel download").size(16.0)).clicked() {
                    *act = Some(CardAction::CancelUpdate);
                }
            } else if widgets::button(ui, b, "update-install", &Btn::primary(&format!("Update to v{}", o.version)).size(17.0).enabled(v.can_install)).clicked() {
                *act = Some(CardAction::InstallUpdate);
            }
        }
        y += BTN_H + 8.0;
    }
    let b = Rect::from_min_size(pos2(x, y), vec2(inner, BTN_H));
    if shown(view, b) && widgets::button(ui, b, "update-check", &Btn::ghost("Check for updates").size(15.0).enabled(v.can_check)).clicked() {
        *act = Some(CardAction::CheckUpdates);
    }
}

/// The MORE GAMES card's height.
pub fn more_games_height() -> f32 {
    PAD + LABEL_H + BTN_H + PAD
}

/// The MORE GAMES card: one button to the catalog.
pub fn draw_more_games(ui: &mut Ui, view: Rect, card: Rect, act: &mut Option<CardAction>) {
    card_frame(ui, card, "MORE GAMES", view, false);
    let b = Rect::from_min_size(pos2(card.min.x + PAD + 2.0, card.min.y + PAD + LABEL_H), vec2(card.width() - 2.0 * PAD - 4.0, BTN_H));
    if shown(view, b) && widgets::button(ui, b, "more-games", &Btn::new("More games").sub("Other games by the same people").size(16.0)).clicked() {
        *act = Some(CardAction::MoreGames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Pos2;

    /// Draw `f` for two frames with a click (press, then release) at `pos`; what the second frame reported.
    fn click_at(pos: Pos2, f: impl Fn(&mut Ui, &mut Option<CardAction>)) -> Option<CardAction> {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let raw = |events: Vec<egui::Event>, time: f64| egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            events,
            time: Some(time),
            ..Default::default()
        };
        let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let mut out = None;
        for (i, events) in [vec![egui::Event::PointerMoved(pos)], vec![button(true)], vec![button(false)], vec![]].into_iter().enumerate() {
            let mut act = None;
            #[allow(deprecated)]
            let _ = ctx.run(raw(events, i as f64 * 0.05), |c| {
                #[allow(deprecated)]
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(c, |ui| f(ui, &mut act));
            });
            if act.is_some() {
                out = act;
            }
        }
        out
    }

    const CARD_W: f32 = 400.0;

    #[test]
    fn the_updates_card_offers_what_the_view_allows() {
        use nostr_release_kit::OfferView;
        let c = Rect::from_min_size(pos2(20.0, 20.0), vec2(CARD_W, 260.0));
        let mut v = UpdateView { installed: "v0.2.0".into(), status: "Version 0.3.0 is available (9.8 MB)".into(), tone: UpdateTone::Accent, offer: Some(OfferView { version: "0.3.0".into(), size: "9.8 MB".into(), notes: String::new() }), checking: false, progress: None, can_check: true, can_install: true, downloading: false };
        let (x, inner) = (c.min.x + PAD + 2.0, c.width() - 2.0 * PAD - 4.0);
        let _ = inner;
        // Layout: label, installed line, status (one line here), then the buttons.
        let probe = |v: &UpdateView, y: f32| {
            let v = v.clone();
            click_at(pos2(x + 60.0, y), move |ui, act| draw_updates(ui, c.expand(300.0), c, &v, 0.0, act))
        };
        let top = c.min.y + PAD + LABEL_H + 22.0 + 4.0;
        let status_h = 17.0; // one line of 13 pt text
        let by = top + status_h + 8.0;
        let install_y = by + BTN_H / 2.0;
        let got = probe(&v, install_y);
        assert!(matches!(got, Some(CardAction::InstallUpdate) | Some(CardAction::CheckUpdates)), "{got:?}");
        // While bytes arrive the same button cancels.
        v.downloading = true;
        v.can_install = false;
        v.progress = Some(0.3);
        let got = probe(&v, install_y + 14.0);
        assert_eq!(got, Some(CardAction::CancelUpdate));
    }

    #[test]
    fn the_more_games_card_is_tall_enough_for_its_button() {
        assert!(more_games_height() > 60.0);
    }
}
