//! Nearby games: rooms heard on this Wi-Fi's beacon, live. Tap one to join it
//! directly (no internet needed). Bluetooth games have their own page
//! (`bluetooth.rs`), one tap away at the foot.

use super::bluetooth::BluetoothScreen;
use super::join::JoinScreen;
use super::online_common::{self, Door};
use super::online_flow;
use super::{common, Ctx, Screen, Transition};
use crate::online;
use crate::ui::art::{self, rgba};
use crate::ui::icons::{self, Icon};
use crate::ui::widgets::{self, Btn, S2, S3};
use crate::ui::{feedback, theme};
use egui::{pos2, vec2, Align2, Rect, Stroke};
use flock_session::net::{LanBrowser, LanRoom};

pub struct NearbyScreen {
    browser: Option<LanBrowser>,
    rooms: Vec<LanRoom>,
    started: bool,
    listening: bool,
    /// Holding the Wi-Fi multicast lock.
    locked: bool,
    /// Screenshots: show `rooms` and never open a browser.
    sample: bool,
    t: f32,
    scroll: f32,
}

impl Default for NearbyScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl NearbyScreen {
    pub fn new() -> NearbyScreen {
        NearbyScreen { browser: None, rooms: Vec::new(), started: false, listening: true, locked: false, sample: false, t: 0.0, scroll: 0.0 }
    }

    /// A fixed list (screenshots).
    pub fn sample() -> NearbyScreen {
        let room = |name: &str, code: &str, n: u8, last: u8| LanRoom {
            name: name.into(),
            code: code.into(),
            player_count: n,
            max_players: 6,
            addr: std::net::SocketAddr::from(([192, 168, 1, last], 41000 + last as u16)),
            token: "00".into(),
        };
        NearbyScreen {
            rooms: vec![room("Mo's flock", "KWTQPM", 2, 24), room("Friday flock", "HDXNZB", 5, 31), room("Rider's flock", "PCRVAS", 6, 40)],
            started: true,
            sample: true,
            ..NearbyScreen::new()
        }
    }

    /// Nothing nearby yet (screenshots).
    pub fn empty_sample() -> NearbyScreen {
        NearbyScreen { started: true, sample: true, ..NearbyScreen::new() }
    }

    fn stop(&mut self, ctx: &mut Ctx) {
        self.browser = None;
        if self.locked {
            self.locked = false;
            ctx.platform.set_multicast_lock(false);
        }
    }
}

impl Screen for NearbyScreen {
    fn name(&self) -> &'static str {
        "nearby"
    }

    fn on_background(&mut self, ctx: &mut Ctx) {
        feedback::on_background();
        // Release the LAN port while away; the list starts again on the next frame back.
        if !self.sample {
            self.stop(ctx);
            self.started = false;
        }
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        common::begin(ctx, &mut self.t);
        if !self.started {
            self.started = true;
            // Android drops multicast packets without the lock.
            ctx.platform.set_multicast_lock(true);
            self.locked = true;
            let b = online::browse_nearby(ctx.data_dir);
            self.listening = b.is_listening();
            log::info!("nearby: browsing (listening {})", self.listening);
            self.browser = Some(b);
        }
        if let Some(b) = self.browser.as_mut() {
            let rooms = b.poll();
            if rooms.len() != self.rooms.len() {
                log::info!("nearby: {} room(s) in range", rooms.len());
            }
            self.rooms = rooms;
        }
        // A join link while here: that room instead.
        if let Some(code) = online_flow::take_join() {
            self.stop(ctx);
            return Transition::Replace(Box::new(JoinScreen::with_code(&code)));
        }
        Transition::None
    }

    fn back(&mut self, ctx: &mut Ctx) -> Transition {
        feedback::back(ctx);
        self.stop(ctx);
        Transition::Pop
    }

    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        let safe = ctx.viewport.safe;
        let (t, tm) = (self.t, ctx.time as f32);
        let rooms = self.rooms.clone();
        let listening = self.listening;
        let bt_here = !matches!(online_flow::radio().state(), crate::platform::ble::BleState::Unsupported) || self.sample;
        let mut join: Option<usize> = None;
        let (mut leave, mut by_code, mut bluetooth) = (false, false, false);
        let mut scroll = self.scroll;
        common::page(ctx.viewport.screen, tm, ui, 0.6, |ui, _| {
            let hd = widgets::header(ui, safe, "Nearby games", widgets::enter(t, 0));
            if hd.back.clicked() {
                leave = true;
            }
            let body = hd.body;
            let p = ui.painter().clone();
            let foot_h = 50.0;
            let foot_rows = if bt_here { 2.0 } else { 1.0 };
            let foot_total = foot_h * foot_rows + S2 * (foot_rows - 1.0);
            let list = Rect::from_min_max(body.min, pos2(body.right(), body.bottom() - foot_total - S3));
            let row_h = 66.0;
            let gap = S2;
            if rooms.is_empty() {
                let card = Rect::from_min_size(list.min + vec2(0.0, 6.0), vec2(list.width(), 168.0));
                widgets::panel(&p, card, 22.0);
                // radar sweep
                let c = pos2(card.left() + 56.0, card.center().y);
                for k in 0..3 {
                    let ph = ((tm * 0.8 + k as f32 / 3.0) % 1.0).clamp(0.0, 1.0);
                    p.circle_stroke(c, 12.0 + ph * 34.0, Stroke::new(2.0_f32, rgba(0xF7, 0x93, 0x1A, 0.7 * (1.0 - ph))));
                }
                icons::draw(&p, Icon::Nearby, c, 34.0, theme::SAND);
                let tx = card.left() + 112.0;
                let (head, sub) = if listening {
                    ("Looking for games\u{2026}", "Both phones need to be on the same Wi-Fi. Ask a friend to open Host a game.")
                } else {
                    ("Can't listen on this network", "Something else is using the Nearby port, or Wi-Fi is off. Try Join with code instead.")
                };
                let size = art::fit_size(&p, head, theme::title, 22.0, card.right() - tx - 14.0);
                p.text(pos2(tx, card.top() + 48.0), Align2::LEFT_CENTER, head, theme::title(size), theme::TEXT);
                let g = p.layout(sub.to_string(), theme::ui_medium(13.0), theme::TEXT_DIM, card.right() - 16.0 - tx);
                p.galley(pos2(tx, card.top() + 72.0), g, theme::TEXT_DIM);
            } else {
                let total = rooms.len() as f32 * (row_h + gap);
                let max_scroll = (total - list.height()).max(0.0);
                let drag = ui.interact(list, egui::Id::new("nearby-list"), egui::Sense::drag());
                if drag.dragged() {
                    scroll = (scroll - drag.drag_delta().y).clamp(0.0, max_scroll);
                }
                let clip = p.with_clip_rect(list.expand(4.0));
                for (i, room) in rooms.iter().enumerate() {
                    let r = Rect::from_min_size(pos2(list.left(), list.top() + i as f32 * (row_h + gap) - scroll), vec2(list.width(), row_h));
                    if r.bottom() < list.top() || r.top() > list.bottom() {
                        continue;
                    }
                    let e = widgets::enter(t, 1 + i);
                    widgets::staged(ui, e, vec2(0.0, 28.0), |ui, off| {
                        let r = r.translate(off);
                        let full = room.player_count >= room.max_players;
                        let resp = ui.interact(r.intersect(list), egui::Id::new(("nearby-row", i)), egui::Sense::click());
                        let (scale, _) = widgets::press_fx(ui, egui::Id::new(("nearby-row", i)), &resp);
                        let rr = Rect::from_center_size(r.center(), r.size() * scale);
                        widgets::panel(&clip, rr, 18.0);
                        icons::draw(&clip, Icon::Nearby, pos2(rr.left() + 30.0, rr.center().y), 28.0, theme::SAND);
                        let name_size = art::fit_size(&clip, &room.name, theme::title, 20.0, rr.width() - 200.0);
                        clip.text(pos2(rr.left() + 58.0, rr.center().y - 11.0), Align2::LEFT_CENTER, &room.name, theme::title(name_size), theme::TEXT);
                        clip.text(pos2(rr.left() + 58.0, rr.center().y + 13.0), Align2::LEFT_CENTER, format!("{}  \u{b7}  {}", online::spaced_code(&room.code), room.addr.ip()), theme::ui_medium(11.5), theme::TEXT_DIM);
                        let count = format!("{}/{}", room.player_count, room.max_players);
                        clip.text(pos2(rr.right() - 96.0, rr.center().y - 6.0), Align2::RIGHT_CENTER, count, theme::numbers(21.0), if full { theme::LASER } else { theme::GOLD });
                        clip.text(pos2(rr.right() - 96.0, rr.center().y + 14.0), Align2::RIGHT_CENTER, "flockers", theme::ui_medium(9.5), theme::TEXT_DIM);
                        let jb = Rect::from_center_size(pos2(rr.right() - 50.0, rr.center().y), vec2(74.0, 38.0));
                        let b = if full { Btn::new("Full").enabled(false) } else { Btn::primary("Join") };
                        if widgets::button(ui, jb, &format!("nearby-join-{i}"), &b.size(17.0)).clicked() || (resp.clicked() && !full) {
                            join = Some(i);
                        }
                    });
                }
            }
            // ── the foot ──
            let fy = body.bottom() - foot_total;
            if widgets::button(ui, Rect::from_min_size(pos2(body.left(), fy), vec2(body.width(), foot_h)), "nearby-code", &Btn::new("Join with code").icon(Icon::Join).size(17.0)).clicked() {
                by_code = true;
            }
            if bt_here && widgets::button(ui, Rect::from_min_size(pos2(body.left(), fy + foot_h + S2), vec2(body.width(), foot_h)), "nearby-bluetooth", &Btn::ghost("Bluetooth games").size(16.0)).clicked() {
                bluetooth = true;
            }
            if bt_here {
                online_common::bluetooth_glyph(&p, pos2(body.left() + 28.0, fy + foot_h + S2 + foot_h / 2.0), 22.0, theme::SAND);
            }
        });
        self.scroll = scroll;
        if leave {
            return self.back(ctx);
        }
        if by_code {
            feedback::select(ctx);
            self.stop(ctx);
            return Transition::Replace(Box::new(JoinScreen::new()));
        }
        if bluetooth {
            feedback::select(ctx);
            self.stop(ctx);
            return Transition::Replace(Box::new(BluetoothScreen::new()));
        }
        if let Some(i) = join {
            if let Some(room) = rooms.get(i) {
                feedback::select(ctx);
                if let Some(gate) = super::account::gate(ctx) {
                    return gate;
                }
                match online_flow::open_nearby(ctx, room) {
                    Ok(()) => {
                        self.stop(ctx);
                        return Transition::Replace(Box::new(JoinScreen::connecting(Door::Nearby, &room.name)));
                    }
                    Err(e) => ctx.say(e),
                }
            }
        }
        Transition::None
    }
}
