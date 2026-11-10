//! "Play with friends": Host, Join with code, Nearby and Bluetooth. Gated by
//! My Signet's online approval (the engine wraps a push to it in the approval
//! screen, see `engine::ONLINE_SCREENS`). Expose it as `MultiplayerScreen::new()`.

use super::bluetooth::BluetoothScreen;
use super::join::JoinScreen;
use super::lobby::LobbyScreen;
use super::nearby::NearbyScreen;
use super::online_common;
use super::online_flow;
use super::{common, Ctx, Screen, Transition};
use crate::platform::ble::BleState;
use crate::ui::art::{self, rgba};
use crate::ui::icons::{self, Icon};
use crate::ui::widgets::{self, Btn, S3};
use crate::ui::{feedback, theme};
use egui::{pos2, vec2, Align2, Color32, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Way {
    Host,
    Join,
    Nearby,
    Bluetooth,
}

impl Way {
    fn title(self) -> &'static str {
        match self {
            Way::Host => "Host a game",
            Way::Join => "Join with code",
            Way::Nearby => "Nearby",
            Way::Bluetooth => "Bluetooth",
        }
    }
    fn blurb(self) -> &'static str {
        match self {
            Way::Host => "Open a room, share the code, fill empty seats with bots.",
            Way::Join => "Type a friend's six-letter room code.",
            Way::Nearby => "Find games on the same Wi-Fi. No code needed.",
            Way::Bluetooth => "Phones close by, no Wi-Fi or internet. The host lets you in.",
        }
    }
    fn foot(self) -> &'static str {
        match self {
            Way::Host => "Up to 6 flockers",
            Way::Join => "Online",
            Way::Nearby => "Same Wi-Fi",
            Way::Bluetooth => "Close by",
        }
    }
}

pub struct MultiplayerScreen {
    t: f32,
    /// Open the join screen for this code on the first frame (a join link).
    join: Option<String>,
    /// Screenshots: show the Bluetooth card whatever the radio says.
    force_bluetooth: bool,
}

impl Default for MultiplayerScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl MultiplayerScreen {
    pub fn new() -> MultiplayerScreen {
        MultiplayerScreen { t: 0.0, join: None, force_bluetooth: false }
    }

    /// The menu with the join screen for `code` straight on top (a join link),
    /// so Back from it lands here.
    pub fn joining(code: String) -> MultiplayerScreen {
        MultiplayerScreen { join: Some(code), ..MultiplayerScreen::new() }
    }

    /// All four ways, whatever this machine's radio says (screenshots).
    pub fn sample() -> MultiplayerScreen {
        MultiplayerScreen { force_bluetooth: true, ..MultiplayerScreen::new() }
    }

    fn ways(&self) -> Vec<Way> {
        let mut v = vec![Way::Host, Way::Join, Way::Nearby];
        if self.force_bluetooth || online_flow::radio().state() != BleState::Unsupported {
            v.push(Way::Bluetooth);
        }
        v
    }
}

impl Screen for MultiplayerScreen {
    fn name(&self) -> &'static str {
        online_flow::MENU
    }

    fn on_background(&mut self, _ctx: &mut Ctx) {
        feedback::on_background();
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        common::begin(ctx, &mut self.t);
        // Back here means no room: a session left by a screen that popped (the run screen's
        // Back) is closed. Not while this screen fades out under the lobby it just opened.
        if !ctx.input_blocked && crate::online::is_some() {
            crate::online::leave();
        }
        // A join link (cold start, or while on the menu): straight into joining.
        if let Some(code) = self.join.take().or_else(online_flow::take_join) {
            log::info!("multiplayer: join link");
            return Transition::Push(Box::new(JoinScreen::with_code(&code)));
        }
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
        let ways = self.ways();
        let mut chosen: Option<Way> = None;
        let mut account = false;
        let name = ctx.profile.name.clone();
        let signed_in = ctx.account.is_some();
        common::page(ctx.viewport.screen, tm, ui, 0.18, |ui, _| {
            let hd = widgets::header(ui, safe, "Play with friends", widgets::enter(t, 0));
            if hd.back.clicked() {
                feedback::back(ctx);
                out = Transition::Pop;
            }
            let body = hd.body;
            let gap = S3;
            let foot_h = 44.0;
            let n = ways.len() as f32;
            let avail = body.height() - foot_h - gap * 2.0;
            let card_h = ((avail - gap * (n - 1.0)) / n).clamp(96.0, 124.0);
            let block = n * card_h + (n - 1.0) * gap;
            let y0 = body.top() + ((avail - block) / 2.0).max(0.0);
            for (i, way) in ways.iter().enumerate() {
                let rect = Rect::from_min_size(pos2(body.left(), y0 + i as f32 * (card_h + gap)), vec2(body.width(), card_h));
                let e = widgets::enter(t, i + 1);
                let clicked = widgets::staged(ui, e, vec2(0.0, 40.0), |ui, off| card(ui, rect.translate(off), &format!("mp-{i}"), *way, i == 0, tm + i as f32));
                if clicked {
                    feedback::select(ctx);
                    chosen = Some(*way);
                }
            }
            // ── who is playing ──
            let fy = body.bottom() - foot_h;
            let p = ui.painter().clone();
            let line = format!("Playing as {name}");
            widgets::pill(&p, pos2(body.left() + 12.0 + 4.0 + p.layout_no_wrap(line.clone(), theme::ui_bold(12.5), Color32::WHITE).size().x / 2.0 + 8.0, fy + foot_h / 2.0), &line, rgba(0x14, 0x1E, 0x46, 0.85), theme::TEXT, 12.5);
            if signed_in && widgets::button(ui, Rect::from_min_size(pos2(body.right() - 168.0, fy + 4.0), vec2(168.0, 36.0)), "mp-account", &Btn::ghost("My Signet account").size(13.5)).clicked() {
                account = true;
            }
        });
        if account {
            feedback::select(ctx);
            return Transition::Push(Box::new(super::account::AccountScreen::manage()));
        }
        if let Some(way) = chosen {
            out = open(ctx, way);
        }
        out
    }
}

/// A card in the menu: icon medallion, title, one line of help, a footnote.
fn card(ui: &mut egui::Ui, rect: Rect, salt: &str, way: Way, hero: bool, tm: f32) -> bool {
    let id = egui::Id::new(("flock-mp-card", salt));
    #[cfg(test)]
    widgets::BUTTON_RECTS.with(|v| {
        if let Some(v) = v.borrow_mut().as_mut() {
            v.push((salt.to_string(), rect, ui.clip_rect()));
        }
    });
    let resp = ui.interact(rect, id, egui::Sense::click());
    let (scale, sink) = widgets::press_fx(ui, id, &resp);
    let rect = Rect::from_center_size(rect.center() + vec2(0.0, sink * 2.0), rect.size() * scale);
    let p = ui.painter().clone();
    let r = 20.0;
    widgets::soft_shadow(&p, rect, r, 6.0, 16.0, 0.5);
    let (top, bot) = if hero { (Color32::from_rgb(0xFF, 0xB8, 0x44), Color32::from_rgb(0xD8, 0x5A, 0x16)) } else { (Color32::from_rgb(0x7E, 0x47, 0xAC), Color32::from_rgb(0x44, 0x1C, 0x6E)) };
    widgets::rrect_gradient(&p, rect, r, top, bot);
    p.rect_stroke(rect, egui::CornerRadius::same(r as u8), egui::Stroke::new(2.5_f32, theme::OUTLINE), egui::StrokeKind::Inside);
    p.hline((rect.left() + r)..=(rect.right() - r), rect.top() + 3.0, egui::Stroke::new(1.6_f32, rgba(255, 255, 255, 0.35)));
    let mc = pos2(rect.left() + 18.0 + 27.0, rect.center().y);
    p.circle_filled(mc, 27.0, rgba(10, 3, 22, 0.30));
    p.circle_stroke(mc, 27.0, egui::Stroke::new(2.0_f32, rgba(255, 255, 255, 0.22)));
    let ink = if hero { Color32::WHITE } else { theme::SAND };
    let bob = (tm * 1.8).sin() * 1.0;
    match way {
        Way::Host => icons::draw(&p, Icon::Host, mc + vec2(0.0, bob), 40.0, ink),
        Way::Join => icons::draw(&p, Icon::Join, mc + vec2(0.0, bob), 40.0, ink),
        Way::Nearby => icons::draw(&p, Icon::Nearby, mc + vec2(0.0, bob), 40.0, ink),
        Way::Bluetooth => online_common::bluetooth_glyph(&p, mc + vec2(0.0, bob), 36.0, ink),
    }
    let tx = mc.x + 27.0 + 14.0;
    let w = rect.right() - 14.0 - tx;
    let size = art::fit_size(&p, way.title(), theme::title, 24.0, w);
    art::outlined(&p, pos2(tx, rect.top() + rect.height() * 0.30), Align2::LEFT_CENTER, way.title(), theme::title(size), Color32::WHITE, theme::OUTLINE, (size * 0.085).max(1.0), 1.5);
    let sub_col = if hero { Color32::from_rgb(0x4A, 0x1A, 0x08) } else { theme::TEXT };
    let g = p.layout(way.blurb().to_string(), theme::ui_medium(12.5), sub_col, w);
    p.galley(pos2(tx, rect.top() + rect.height() * 0.46), g.clone(), sub_col);
    p.text(pos2(tx, rect.bottom() - 17.0), Align2::LEFT_CENTER, way.foot(), theme::ui_bold(10.5), if hero { Color32::from_rgb(0x4A, 0x1A, 0x08) } else { theme::SAND });
    resp.clicked()
}

/// Open the screen behind a card. Hosting opens the room first.
fn open(ctx: &mut Ctx, way: Way) -> Transition {
    if let Some(gate) = super::account::gate(ctx) {
        return gate;
    }
    match way {
        Way::Host => match online_flow::open_host(ctx) {
            Ok(()) => Transition::Push(Box::new(LobbyScreen::new())),
            Err(e) => {
                ctx.say(e);
                Transition::None
            }
        },
        Way::Join => Transition::Push(Box::new(JoinScreen::new())),
        Way::Nearby => Transition::Push(Box::new(NearbyScreen::new())),
        Way::Bluetooth => Transition::Push(Box::new(BluetoothScreen::new())),
    }
}
