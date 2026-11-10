//! Bluetooth games, both ends. *Join a game* scans for hosts in range and
//! lists them ("Bluetooth game", the host's colour, room 1A2B); *Host a game*
//! opens a room with the Bluetooth door open, and its lobby asks you to Accept
//! each joiner by their verified My Signet persona. The radio's state walks
//! the player through permission and switching Bluetooth on; the whole screen
//! is hidden from the menu when the device has no Bluetooth games (Android
//! below 10).

use super::join::JoinScreen;
use super::lobby::LobbyScreen;
use super::online_common::{self, Door};
use super::online_flow;
use super::{common, Ctx, Screen, Transition};
use crate::platform::ble::{BleAdvert, BleSeen, BleState};
use crate::ui::art::{self, rgba};
use crate::ui::widgets::{self, Btn, Seg, S2, S3};
use crate::ui::{feedback, theme};
use egui::{pos2, vec2, Align2, Rect, Stroke};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Join,
    Host,
}

/// What the screen draws instead of the radio (screenshots).
struct Staged {
    state: BleState,
    seen: Vec<BleSeen>,
}

pub struct BluetoothScreen {
    t: f32,
    tab: Tab,
    state: BleState,
    seen: Vec<BleSeen>,
    scanning: bool,
    scan_error: Option<String>,
    /// Seconds since the scan started (for "nothing yet" wording).
    scan_age: f32,
    scroll: f32,
    staged: Option<Staged>,
}

impl Default for BluetoothScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl BluetoothScreen {
    pub fn new() -> BluetoothScreen {
        BluetoothScreen { t: 0.0, tab: Tab::Join, state: BleState::Unsupported, seen: Vec::new(), scanning: false, scan_error: None, scan_age: 0.0, scroll: 0.0, staged: None }
    }

    /// Scanning with three hosts in range (screenshots).
    pub fn scan_sample() -> BluetoothScreen {
        let seen = |id: &str, room: [u8; 4], colour: u8, rssi: i32| BleSeen { device_id: id.into(), advert: BleAdvert::new(room, colour), rssi, last_seen: Instant::now() };
        BluetoothScreen {
            staged: Some(Staged { state: BleState::Ready, seen: vec![seen("a", [0x1A, 0x2B, 0, 0], 1, -48), seen("b", [0x7C, 0x03, 0, 0], 4, -66), seen("c", [0xD0, 0x9E, 0, 0], 2, -82)] }),
            scan_age: 5.0,
            ..BluetoothScreen::new()
        }
    }

    /// Permission not given yet (screenshots).
    pub fn permission_sample() -> BluetoothScreen {
        BluetoothScreen { staged: Some(Staged { state: BleState::NeedsPermission, seen: Vec::new() }), ..BluetoothScreen::new() }
    }

    /// The Host tab, Bluetooth ready (screenshots).
    pub fn host_sample() -> BluetoothScreen {
        BluetoothScreen { tab: Tab::Host, staged: Some(Staged { state: BleState::Ready, seen: Vec::new() }), ..BluetoothScreen::new() }
    }

    fn stop_scan(&mut self) {
        if self.scanning && self.staged.is_none() {
            online_flow::radio().stop_scan();
        }
        self.scanning = false;
    }
}

fn bars(rssi: i32) -> u8 {
    if rssi > -60 {
        3
    } else if rssi > -75 {
        2
    } else {
        1
    }
}

impl Screen for BluetoothScreen {
    fn name(&self) -> &'static str {
        "bluetooth"
    }

    fn on_background(&mut self, _ctx: &mut Ctx) {
        feedback::on_background();
        // The scan restarts on the next frame back.
        self.stop_scan();
        self.seen.clear();
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        common::begin(ctx, &mut self.t);
        if let Some(code) = super::online_flow::take_join() {
            self.stop_scan();
            return Transition::Replace(Box::new(JoinScreen::with_code(&code)));
        }
        if let Some(s) = &self.staged {
            self.state = s.state;
            self.seen = s.seen.clone();
            return Transition::None;
        }
        let radio = online_flow::radio();
        self.state = radio.state();
        if self.state != BleState::Ready || self.tab != Tab::Join {
            if self.scanning {
                self.stop_scan();
                self.seen.clear();
            }
            return Transition::None;
        }
        // Keep scanning exactly while Ready on the Join tab. A throttled start
        // is reported and not retried every frame.
        if !self.scanning && self.scan_error.is_none() {
            match radio.start_scan() {
                Ok(()) => {
                    self.scanning = true;
                    self.scan_age = 0.0;
                }
                Err(e) => self.scan_error = Some(e),
            }
        }
        if self.scanning {
            self.scan_age += ctx.dt;
            self.seen = radio.seen();
            if let Some(e) = radio.scan_error() {
                self.scan_error = Some(e);
            }
        }
        Transition::None
    }

    fn back(&mut self, ctx: &mut Ctx) -> Transition {
        feedback::back(ctx);
        self.stop_scan();
        Transition::Pop
    }

    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        let safe = ctx.viewport.safe;
        let (t, tm) = (self.t, ctx.time as f32);
        let state = self.state;
        let tab = self.tab;
        let seen = self.seen.clone();
        let scan_error = self.scan_error.clone();
        let scan_age = self.scan_age;
        let sample = self.staged.is_some();
        let mut scroll = self.scroll;
        let (mut leave, mut permission, mut enable, mut settings, mut retry_scan, mut host) = (false, false, false, false, false, false);
        let mut pick: Option<usize> = None;
        let mut new_tab = None;
        common::page(ctx.viewport.screen, tm, ui, 0.6, |ui, _| {
            let hd = widgets::header(ui, safe, "Bluetooth games", widgets::enter(t, 0));
            if hd.back.clicked() {
                leave = true;
            }
            let body = hd.body;
            let p = ui.painter().clone();
            // ── tabs ──
            let seg = Rect::from_min_size(body.min, vec2(body.width(), 44.0));
            let segs = [Seg::new("Join a game"), Seg::new("Host a game")];
            if let Some(i) = widgets::segmented(ui, seg, "bt-tab", &segs, if tab == Tab::Join { 0 } else { 1 }) {
                new_tab = Some(if i == 0 { Tab::Join } else { Tab::Host });
            }
            let area = Rect::from_min_max(pos2(body.left(), seg.bottom() + S3), body.max);

            // ── the radio's state ──
            if state != BleState::Ready {
                let card = Rect::from_min_size(area.min, vec2(area.width(), 232.0));
                widgets::panel(&p, card, 22.0);
                let (title, text, action) = match state {
                    BleState::Unsupported => ("No Bluetooth games here", "This phone can't play over Bluetooth. Nearby and Join with code still work.", None),
                    BleState::Off => ("Bluetooth is off", "Switch Bluetooth on to find games or host one without Wi-Fi.", Some(("Switch on", "bt-enable"))),
                    BleState::NeedsPermission => ("Turn on Bluetooth play", "Cluster Flock needs permission to find and talk to phones nearby. It never uses your location.", Some(("Allow", "bt-allow"))),
                    BleState::PermissionDenied => ("Bluetooth is blocked", "Allow \"Nearby devices\" for Cluster Flock in the phone's settings, then come back.", Some(("Open settings", "bt-settings"))),
                    BleState::Ready => unreachable!(),
                };
                online_common::bluetooth_glyph(&p, pos2(card.center().x, card.top() + 38.0), 34.0, theme::SAND);
                p.text(pos2(card.center().x, card.top() + 80.0), Align2::CENTER_CENTER, title, theme::title(23.0), theme::TEXT);
                let g = p.layout(text.to_string(), theme::ui_medium(13.5), theme::TEXT_DIM, card.width() - 48.0);
                p.galley(pos2(card.center().x - g.size().x / 2.0, card.top() + 102.0), g, theme::TEXT_DIM);
                if let Some((label, salt)) = action {
                    let b = Rect::from_center_size(pos2(card.center().x, card.bottom() - 36.0), vec2(card.width() - 56.0, 48.0));
                    if widgets::button(ui, b, salt, &Btn::primary(label)).clicked() {
                        match salt {
                            "bt-enable" => enable = true,
                            "bt-allow" => permission = true,
                            _ => settings = true,
                        }
                    }
                }
                return;
            }

            match tab {
                Tab::Join => {
                    let note_h = 36.0;
                    let list = Rect::from_min_max(area.min, pos2(area.right(), area.bottom() - note_h));
                    p.text(pos2(area.center().x, area.bottom() - note_h / 2.0), Align2::CENTER_CENTER, "Stay close to the host. If a game shows under Nearby too, use that one.", theme::ui_medium(11.5), theme::TEXT_DIM);
                    if seen.is_empty() {
                        let card = Rect::from_min_size(list.min + vec2(0.0, 6.0), vec2(list.width(), 168.0));
                        widgets::panel(&p, card, 22.0);
                        let c = pos2(card.left() + 56.0, card.center().y);
                        for k in 0..3 {
                            let ph = ((tm * 0.8 + k as f32 / 3.0) % 1.0).clamp(0.0, 1.0);
                            p.circle_stroke(c, 12.0 + ph * 34.0, Stroke::new(2.0_f32, rgba(0xF7, 0x93, 0x1A, 0.7 * (1.0 - ph))));
                        }
                        online_common::bluetooth_glyph(&p, c, 32.0, theme::SAND);
                        let tx = card.left() + 112.0;
                        let (head, sub) = match &scan_error {
                            Some(e) => ("Can't scan right now", e.as_str()),
                            None if scan_age > 8.0 => ("No games in range yet", "Ask your friend to open Host a game \u{2192} Bluetooth, and stand close to them."),
                            None => ("Looking for games\u{2026}", "Ask your friend to open Host a game \u{2192} Bluetooth."),
                        };
                        let size = art::fit_size(&p, head, theme::title, 21.0, card.right() - tx - 14.0);
                        p.text(pos2(tx, card.top() + 48.0), Align2::LEFT_CENTER, head, theme::title(size), theme::TEXT);
                        let g = p.layout(sub.to_string(), theme::ui_medium(13.0), theme::TEXT_DIM, card.right() - 16.0 - tx);
                        p.galley(pos2(tx, card.top() + 72.0), g, theme::TEXT_DIM);
                        if scan_error.is_some() && widgets::button(ui, Rect::from_min_size(pos2(tx, card.bottom() - 50.0), vec2(card.right() - 16.0 - tx, 38.0)), "bt-rescan", &Btn::new("Try again").size(15.0)).clicked() {
                            retry_scan = true;
                        }
                    } else {
                        let row_h = 70.0;
                        let gap = S2;
                        let total = seen.len() as f32 * (row_h + gap);
                        let max_scroll = (total - list.height()).max(0.0);
                        let drag = ui.interact(list, egui::Id::new("bt-list"), egui::Sense::drag());
                        if drag.dragged() {
                            scroll = (scroll - drag.drag_delta().y).clamp(0.0, max_scroll);
                        }
                        let clip = p.with_clip_rect(list.expand(4.0));
                        for (i, s) in seen.iter().enumerate() {
                            let r = Rect::from_min_size(pos2(list.left(), list.top() + i as f32 * (row_h + gap) - scroll), vec2(list.width(), row_h));
                            if r.bottom() < list.top() || r.top() > list.bottom() {
                                continue;
                            }
                            let e = widgets::enter(t, 1 + i);
                            widgets::staged(ui, e, vec2(0.0, 28.0), |ui, off| {
                                let r = r.translate(off);
                                let resp = ui.interact(r.intersect(list), egui::Id::new(("bt-row", i)), egui::Sense::click());
                                let (scale, _) = widgets::press_fx(ui, egui::Id::new(("bt-row", i)), &resp);
                                let rr = Rect::from_center_size(r.center(), r.size() * scale);
                                widgets::panel(&clip, rr, 18.0);
                                online_common::bird_chip(&clip, pos2(rr.left() + 34.0, rr.center().y), 22.0, s.advert.colour, 1.0);
                                clip.text(pos2(rr.left() + 66.0, rr.center().y - 12.0), Align2::LEFT_CENTER, "Bluetooth game", theme::title(20.0), theme::TEXT);
                                clip.text(pos2(rr.left() + 66.0, rr.center().y + 12.0), Align2::LEFT_CENTER, format!("{} \u{b7} room {}", online_common::colour_name(s.advert.colour), s.advert.room_label()), theme::ui_medium(12.0), theme::TEXT_DIM);
                                // signal bars
                                let nb = bars(s.rssi);
                                for k in 0..3u8 {
                                    let h = 6.0 + 5.0 * k as f32;
                                    let x = rr.right() - 108.0 + k as f32 * 6.0;
                                    clip.rect_filled(Rect::from_min_max(pos2(x, rr.center().y + 8.0 - h), pos2(x + 4.0, rr.center().y + 8.0)), 1.5, if k < nb { theme::MINT } else { rgba(255, 255, 255, 0.16) });
                                }
                                let jb = Rect::from_center_size(pos2(rr.right() - 50.0, rr.center().y), vec2(74.0, 38.0));
                                if widgets::button(ui, jb, &format!("bt-join-{i}"), &Btn::primary("Join").size(17.0)).clicked() || resp.clicked() {
                                    pick = Some(i);
                                }
                            });
                        }
                    }
                }
                Tab::Host => {
                    let card = Rect::from_min_size(area.min, vec2(area.width(), 250.0));
                    widgets::panel(&p, card, 22.0);
                    online_common::bluetooth_glyph(&p, pos2(card.left() + 44.0, card.top() + 46.0), 40.0, theme::SAND);
                    p.text(pos2(card.left() + 84.0, card.top() + 46.0), Align2::LEFT_CENTER, "Host over Bluetooth", theme::title(22.0), theme::TEXT);
                    let g = p.layout(
                        "Open a room that phones nearby can join without Wi-Fi. Friends nearby find it under Bluetooth games; you also get the room code for everyone else.\n\nYou approve each player by their My Signet persona before they join.".to_string(),
                        theme::ui_medium(13.5),
                        theme::TEXT_DIM,
                        card.width() - 40.0,
                    );
                    p.galley(pos2(card.left() + 20.0, card.top() + 76.0), g, theme::TEXT_DIM);
                    let b = Rect::from_min_size(pos2(card.left() + 20.0, card.bottom() - 64.0), vec2(card.width() - 40.0, 48.0));
                    if widgets::button(ui, b, "bt-host", &Btn::primary("Open a Bluetooth game").size(19.0)).clicked() {
                        host = true;
                    }
                }
            }
        });
        self.scroll = scroll;
        if leave {
            return self.back(ctx);
        }
        if let Some(t) = new_tab {
            feedback::tap(ctx);
            self.tab = t;
            self.scroll = 0.0;
            if t == Tab::Host {
                self.stop_scan();
            }
        }
        let radio = online_flow::radio();
        if permission {
            feedback::select(ctx);
            radio.request_permission();
        }
        if enable {
            feedback::select(ctx);
            radio.request_enable();
        }
        if settings {
            feedback::select(ctx);
            radio.open_settings();
        }
        if retry_scan {
            feedback::tap(ctx);
            self.scan_error = None;
        }
        if let Some(i) = pick {
            feedback::select(ctx);
            if sample {
                ctx.say("This is a preview.");
            } else if let Some(s) = seen.get(i) {
                if let Some(gate) = super::account::gate(ctx) {
                    return gate;
                }
                let label = format!("Bluetooth game \u{b7} room {}", s.advert.room_label());
                match online_flow::open_bluetooth(ctx, s) {
                    Ok(()) => {
                        // Connecting stops the scan.
                        self.scanning = false;
                        self.seen.clear();
                        return Transition::Replace(Box::new(JoinScreen::connecting(Door::Bluetooth, &label)));
                    }
                    Err(e) => {
                        log::info!("bluetooth: join failed: {e}");
                        ctx.say("Couldn't reach that Bluetooth game. Stay close to the host and try again.");
                        self.scanning = false;
                    }
                }
            }
        }
        if host {
            feedback::select(ctx);
            if sample {
                ctx.say("This is a preview.");
            } else {
                if let Some(gate) = super::account::gate(ctx) {
                    return gate;
                }
                match online_flow::open_host(ctx) {
                    Ok(()) => {
                        self.stop_scan();
                        let colour = ctx.profile.preferred_colour();
                        let res = crate::online::with(|s| online_flow::start_bluetooth(s, colour));
                        if let Some(Err(e)) = res {
                            ctx.say(e);
                        }
                        return Transition::Replace(Box::new(LobbyScreen::bluetooth_host()));
                    }
                    Err(e) => ctx.say(e),
                }
            }
        }
        Transition::None
    }
}
