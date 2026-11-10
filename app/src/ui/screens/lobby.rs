//! The multiplayer lobby: six seats (people and bots) with a colour and
//! accessory each, ready ticks, your bird picker, and for the host the room
//! code (Copy / Share link), the Online / Nearby / Bluetooth doors, bots and
//! their skill, Start, and the Accept prompt for Bluetooth joiners. The same
//! screen serves the host and the guests, and hands over to the match when it
//! starts (`online_flow::start_run`).
//!
//! The screen draws from a [`Snap`] taken in one `online::with` call, so the
//! staged samples (`--shot lobby_host`) need no session.

use super::online_common::{self, Door, DoorState};
use super::online_flow::{self, Next};
use super::{common, Ctx, Screen, Transition};
use crate::online::{self, JoinedBy, RoomStatus};
use crate::platform::ble::BleState;
use crate::ui::art::{self, rgba};
use crate::ui::icons::{self, Icon};
use crate::ui::widgets::{self, Btn, Seg, S2, S3};
use crate::ui::{feedback, theme};
use egui::{pos2, vec2, Align2, Color32, Rect, Stroke};
use flock_core::{BotSkill, InputFrame};
use flock_session::door::BtStatus;
use flock_session::proto::MAX_SEATS;
use flock_session::{PeerKey, SeatPath};

/// One seat as drawn.
#[derive(Clone, Debug)]
pub struct SeatRow {
    pub id: u8,
    pub name: String,
    pub colour: u8,
    /// The seat's hat (255 = its colour's own).
    pub hat: u8,
    pub ready: bool,
    pub bot: bool,
    pub host: bool,
    pub path: SeatPath,
}

/// A Bluetooth joiner waiting for Accept.
#[derive(Clone, Debug)]
pub struct PendingRow {
    pub peer: PeerKey,
    /// The name it typed (self-chosen).
    pub name: String,
    /// Its verified My Signet persona (`npub1...`).
    pub persona: Option<String>,
}

/// Everything the lobby draws, copied out of the session for the frame.
#[derive(Clone, Debug)]
pub struct Snap {
    pub is_host: bool,
    pub joined_by: JoinedBy,
    pub seats: Vec<SeatRow>,
    pub my_seat: Option<u8>,
    pub pending: Vec<PendingRow>,
    pub code: Option<String>,
    pub code_ready: bool,
    pub status: RoomStatus,
    pub bt: BtStatus,
    pub rtt: Option<u32>,
    pub can_start: bool,
    pub host_name: String,
    pub host_persona: Option<String>,
    /// Joined, but the host's lobby has not arrived yet.
    pub connecting: bool,
    pub notice: Option<String>,
}

impl Snap {
    fn me(&self) -> Option<&SeatRow> {
        self.my_seat.and_then(|id| self.seats.iter().find(|s| s.id == id))
    }
}

fn snap_of(s: &online::OnlineSession) -> Snap {
    let lobby = s.lobby();
    let seats = lobby
        .map(|l| l.seats.iter().map(|x| SeatRow { id: x.id, name: x.name.clone(), colour: x.colour, hat: x.hat, ready: x.ready, bot: x.bot, host: x.host, path: x.path }).collect())
        .unwrap_or_default();
    Snap {
        is_host: s.is_host(),
        joined_by: s.joined_by(),
        seats,
        my_seat: s.my_seat(),
        pending: s.pending_joiners().into_iter().map(|p| PendingRow { peer: p.peer, name: p.name, persona: p.persona }).collect(),
        code: s.code().map(str::to_string),
        code_ready: s.code_ready(),
        status: s.status().clone(),
        bt: s.bluetooth(),
        rtt: s.rtt_ms(),
        can_start: s.can_start(),
        host_name: lobby.and_then(|l| l.host_seat().map(|h| h.name.clone())).unwrap_or_else(|| "The host".into()),
        host_persona: s.host_persona().map(str::to_string),
        connecting: lobby.is_none(),
        notice: s.notice(),
    }
}

pub struct LobbyScreen {
    t: f32,
    /// Staged for screenshots: drawn instead of the session's lobby.
    sample: Option<Snap>,
    /// Seconds since the last Copy / Share tap (the button says "Copied").
    copied: f32,
    copied_link: bool,
    /// The slot held a session at some point (it is gone now: leave).
    had_session: bool,
    /// The host wants the Bluetooth door open.
    bt_want: bool,
    bt_err: Option<String>,
    asked_permission: bool,
    asked_enable: bool,
    /// The bot skill last chosen (the session does not report it back).
    bot_skill: BotSkill,
    last_notice: Option<String>,
}

impl Default for LobbyScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl LobbyScreen {
    /// The lobby of the session in the app's slot.
    pub fn new() -> LobbyScreen {
        LobbyScreen {
            t: 0.0,
            sample: None,
            copied: 99.0,
            copied_link: false,
            had_session: false,
            bt_want: false,
            bt_err: None,
            asked_permission: false,
            asked_enable: false,
            bot_skill: BotSkill::NORMAL,
            last_notice: None,
        }
    }

    /// A hosted room whose Bluetooth door opens as soon as the radio is ready.
    pub fn bluetooth_host() -> LobbyScreen {
        LobbyScreen { bt_want: true, ..LobbyScreen::new() }
    }

    fn staged(snap: Snap) -> LobbyScreen {
        LobbyScreen { sample: Some(snap), ..LobbyScreen::new() }
    }

    /// The host's view: four people (one over Bluetooth), a bot, a Bluetooth
    /// joiner waiting for Accept (screenshots).
    pub fn host_sample() -> LobbyScreen {
        let seat = |id: u8, name: &str, colour: u8, ready: bool, bot: bool, host: bool, path: SeatPath| SeatRow { id, name: name.into(), colour, hat: flock_render::HAT_DEFAULT, ready, bot, host, path };
        LobbyScreen::staged(Snap {
            is_host: true,
            joined_by: JoinedBy::Host,
            seats: vec![
                seat(0, "Ann", 0, false, false, true, SeatPath::Local),
                seat(1, "Mo", 1, true, false, false, SeatPath::Network),
                seat(2, "Ivy", 3, true, false, false, SeatPath::Bluetooth),
                seat(3, "Kit", 4, false, false, false, SeatPath::Network),
                seat(100, "Bot 1", 2, true, true, false, SeatPath::Local),
            ],
            my_seat: Some(0),
            pending: vec![PendingRow { peer: 7, name: "Bo".into(), persona: Some("npub1q8zv4r2wk7h9m3x5c0ydgjf6ta8ue2ls4nxp7vkc9wh3a5r0gdmzqs6yf3t".into()) }],
            code: Some("KWTQPM".into()),
            code_ready: true,
            status: RoomStatus { ready: true, failure: None, relays: (2, 3), lan: Some(std::net::SocketAddr::from(([192, 168, 1, 24], 41000))) },
            bt: BtStatus::On { room: "1A2B".into() },
            rtt: Some(38),
            can_start: true,
            host_name: "Ann".into(),
            host_persona: None,
            connecting: false,
            notice: None,
        })
    }

    /// A guest's view of the same room (screenshots).
    pub fn guest_sample() -> LobbyScreen {
        let mut s = LobbyScreen::host_sample().sample.take().expect("staged");
        s.is_host = false;
        s.joined_by = JoinedBy::Code;
        s.my_seat = Some(1);
        s.pending.clear();
        s.code = None;
        s.rtt = Some(64);
        s.host_persona = Some("npub1zf2kq9ynl4c7xd0h8rgw3s6vta5e2mj9u4pcxk8wh7qa3d5ntl0gsr6yv2e".into());
        LobbyScreen::staged(s)
    }

    /// A host who just opened the room and waits alone, Bluetooth asking for permission (screenshots).
    pub fn waiting_sample() -> LobbyScreen {
        let mut s = LobbyScreen::host_sample().sample.take().expect("staged");
        s.seats.truncate(1);
        s.pending.clear();
        s.can_start = false;
        s.bt = BtStatus::Off;
        LobbyScreen::staged(s)
    }

    fn leave(&mut self, ctx: &mut Ctx, say: Option<&str>) -> Transition {
        online_flow::leave_to_menu(ctx, say)
    }
}

/// How the Bluetooth line reads for the host.
struct BtLine {
    text: String,
    state: DoorState,
    control: BtControl,
}

enum BtControl {
    None,
    Switch(bool),
    Button(&'static str, BtAct),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum BtAct {
    On,
    Off,
    Allow,
    Settings,
    SwitchOn,
}

/// The Bluetooth line under the room code; `None` when the device has no Bluetooth games at all.
fn bt_line(door: &BtStatus, radio: BleState, want: bool, err: Option<&str>) -> Option<BtLine> {
    match door {
        BtStatus::On { room } => Some(BtLine { text: format!("Bluetooth on \u{b7} room {room}"), state: DoorState::On, control: BtControl::Switch(true) }),
        BtStatus::Failed(why) => Some(BtLine { text: why.clone(), state: DoorState::Off, control: BtControl::Button("Retry", BtAct::On) }),
        BtStatus::Off => {
            if radio == BleState::Unsupported {
                return None;
            }
            if let Some(e) = err {
                return Some(BtLine { text: e.to_string(), state: DoorState::Off, control: BtControl::Button("Retry", BtAct::On) });
            }
            if !want {
                return Some(BtLine { text: "Bluetooth \u{b7} let phones nearby join without Wi-Fi".into(), state: DoorState::Off, control: BtControl::Switch(false) });
            }
            Some(match radio {
                BleState::NeedsPermission => BtLine { text: "Bluetooth needs your permission".into(), state: DoorState::Opening, control: BtControl::Button("Allow", BtAct::Allow) },
                BleState::PermissionDenied => BtLine { text: "Bluetooth is blocked for this app".into(), state: DoorState::Off, control: BtControl::Button("Settings", BtAct::Settings) },
                BleState::Off => BtLine { text: "Bluetooth is switched off".into(), state: DoorState::Opening, control: BtControl::Button("Switch on", BtAct::SwitchOn) },
                _ => BtLine { text: "Opening Bluetooth\u{2026}".into(), state: DoorState::Opening, control: BtControl::None },
            })
        }
    }
}

/// Extra height of the host's room card for the "Share invite" row.
const SHARE_ROW: f32 = 36.0 + S2;

/// What "Share invite" sends: the room code and the same join link "Copy link" puts on the clipboard.
pub fn invite_text(code: &str) -> String {
    format!("Join my Cluster Flock room {code}: {}", online::join_link(code))
}

enum Act {
    Back,
    Ready(bool),
    Start,
    Copy,
    Link,
    Share,
    Colour(u8),
    AddBot,
    Remove(u8),
    Skill(BotSkill),
    Accept(PeerKey),
    Decline(PeerKey),
    Bt(BtAct),
}

impl Screen for LobbyScreen {
    fn name(&self) -> &'static str {
        "lobby"
    }

    fn on_background(&mut self, _ctx: &mut Ctx) {
        feedback::on_background();
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        common::begin(ctx, &mut self.t);
        self.copied += ctx.dt;
        if self.sample.is_some() {
            return Transition::None;
        }
        if !online::is_some() {
            if self.had_session {
                // The room is gone (left elsewhere): nothing to show.
                self.had_session = false;
                return Transition::PopTo(online_flow::MENU);
            }
            return Transition::None;
        }
        self.had_session = true;
        // A join link while in a room is ignored (leave first); drop it.
        let _ = online_flow::take_join();
        // The Bluetooth door, for the host.
        self.bluetooth_tick(ctx);
        let next = online_flow::pump(InputFrame::default());
        if let Some(n) = online::with(|s| s.notice()).flatten() {
            if self.last_notice.as_deref() != Some(&n) {
                ctx.say(n.clone());
                self.last_notice = Some(n);
            }
        } else {
            self.last_notice = None;
        }
        match next {
            Next::Dead(why) => self.leave(ctx, Some(&why)),
            Next::Run => online_flow::start_run(),
            Next::Results => online_flow::show_results(),
            Next::Lobby | Next::Stay => Transition::None,
        }
    }

    fn back(&mut self, ctx: &mut Ctx) -> Transition {
        feedback::back(ctx);
        let host = online::with(|s| s.is_host()).unwrap_or(false);
        self.leave(ctx, if host { Some("Room closed") } else { None })
    }

    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        let safe = ctx.viewport.safe;
        let (t, tm) = (self.t, ctx.time as f32);
        let mut acts: Vec<Act> = Vec::new();
        let sample = self.sample.is_some();
        let snap = match self.sample.clone() {
            Some(s) => Some(s),
            None => online::with(|s| snap_of(s)),
        };
        let radio = if sample { BleState::Ready } else { online_flow::radio().state() };
        let Some(snap) = snap else {
            // No session yet: just the page.
            common::page(ctx.viewport.screen, tm, ui, 0.6, |ui, _| {
                widgets::header(ui, safe, "Your room", widgets::enter(t, 0));
            });
            return Transition::None;
        };
        let copied = (self.copied < 1.6).then_some(self.copied_link);
        let bt = if snap.is_host { bt_line(&snap.bt, radio, self.bt_want, self.bt_err.as_deref()) } else { None };
        let my_ready = snap.me().is_some_and(|m| m.ready);
        let my_colour = snap.me().map(|m| m.colour);
        let my_hat = snap.me().map_or(ctx.profile.hat, |m| m.hat);
        let skill = self.bot_skill;
        let title = if snap.is_host { "Your room".to_string() } else { format!("{}'s room", snap.host_name) };
        let title = if snap.connecting && !snap.is_host { "Lobby".to_string() } else { title };
        common::page(ctx.viewport.screen, tm, ui, 1.3, |ui, _| {
            let hd = widgets::header(ui, safe, &title, widgets::enter(t, 0));
            if hd.back.clicked() {
                acts.push(Act::Back);
            }
            let body = hd.body;
            let p = ui.painter().clone();

            // ── pinned: the action bar ──
            let bar_h = 54.0;
            let bar = Rect::from_min_max(pos2(body.left(), body.bottom() - bar_h), body.max);
            widgets::staged(ui, widgets::enter(t, 5), vec2(0.0, 20.0), |ui, off| {
                let r = bar.translate(off);
                let label = if my_ready { "Ready!" } else { "Ready" };
                let b = if my_ready { Btn::new(label).icon(Icon::Check).selected(true) } else if snap.is_host { Btn::new(label) } else { Btn::primary(label) };
                let enabled = !snap.connecting;
                if snap.is_host {
                    let rw = (r.width() - S2) * 0.40;
                    if widgets::button(ui, Rect::from_min_size(r.min, vec2(rw, bar_h)), "lobby-ready", &b.enabled(enabled).size(20.0)).clicked() {
                        acts.push(Act::Ready(!my_ready));
                    }
                    let sr = Rect::from_min_max(pos2(r.left() + rw + S2, r.top()), r.max);
                    let mut start = Btn::primary("Start").enabled(snap.can_start).size(22.0);
                    if !snap.can_start {
                        start = start.sub("needs a friend or bot");
                    }
                    if widgets::button(ui, sr, "lobby-start", &start).clicked() {
                        acts.push(Act::Start);
                    }
                } else if widgets::button(ui, r, "lobby-ready", &b.enabled(enabled).size(22.0)).clicked() {
                    acts.push(Act::Ready(!my_ready));
                }
            });

            // ── pinned: the Accept prompt for a Bluetooth joiner ──
            let mut top = body.top();
            if let Some(j) = snap.pending.first() {
                let h = 108.0;
                let card = Rect::from_min_size(pos2(body.left(), top), vec2(body.width(), h));
                let pulse = 0.5 + 0.5 * (tm * 4.0).sin();
                widgets::panel_solid(&p, card, 20.0);
                p.rect_stroke(card, 20.0, Stroke::new(2.0_f32, theme::GOLD.gamma_multiply(0.55 + 0.45 * pulse)), egui::StrokeKind::Inside);
                online_common::bluetooth_glyph(&p, pos2(card.left() + 26.0, card.top() + 24.0), 24.0, theme::GOLD);
                let who = format!("{} wants to join", j.name);
                let size = art::fit_size(&p, &who, theme::title, 20.0, card.width() - 130.0);
                p.text(pos2(card.left() + 48.0, card.top() + 22.0), Align2::LEFT_CENTER, who, theme::title(size), theme::TEXT);
                let more = snap.pending.len() - 1;
                if more > 0 {
                    widgets::pill(&p, pos2(card.right() - 40.0, card.top() + 22.0), &format!("+{more} more"), rgba(0x6A, 0x36, 0x96, 0.9), theme::TEXT, 10.5);
                }
                let (persona_text, persona_col) = match &j.persona {
                    Some(n) => (format!("Verified My Signet persona {}", online_common::short_npub(n)), theme::MINT),
                    None => ("No verified My Signet persona".to_string(), Color32::from_rgb(0xFF, 0x8A, 0x7A)),
                };
                let g = p.layout(persona_text, theme::ui_medium(12.0), persona_col, card.width() - 32.0);
                p.galley(pos2(card.left() + 16.0, card.top() + 42.0), g, persona_col);
                let by = card.bottom() - 10.0 - 36.0;
                let bw = (card.width() - 32.0 - S2) / 2.0;
                if widgets::button(ui, Rect::from_min_size(pos2(card.left() + 16.0, by), vec2(bw, 36.0)), "lobby-decline", &Btn::new("Decline").size(15.0)).clicked() {
                    acts.push(Act::Decline(j.peer));
                }
                if widgets::button(ui, Rect::from_min_size(pos2(card.left() + 16.0 + bw + S2, by), vec2(bw, 36.0)), "lobby-accept", &Btn::primary("Accept").enabled(j.persona.is_some()).size(15.0)).clicked() {
                    acts.push(Act::Accept(j.peer));
                }
                top += h + S3;
            }

            // ── the scrolling middle ──
            let area = Rect::from_min_max(pos2(body.left(), top), pos2(body.right(), bar.top() - S3));
            let room_h = if snap.is_host { 122.0 + SHARE_ROW } else { 90.0 };
            let badges_h = if snap.is_host { 26.0 + 8.0 } else { 0.0 };
            let bt_h = if bt.is_some() { 42.0 + 8.0 } else { 0.0 };
            let seat_h = 44.0;
            let seat_gap = 4.0;
            let seats_h = 22.0 + MAX_SEATS as f32 * (seat_h + seat_gap);
            let skill_h = if snap.is_host { 40.0 + 8.0 } else { 0.0 };
            let pick_h = 88.0;
            // The flock, bot skill and bird picker sit together on one opaque panel,
            // so the farm scenery never shows through the text.
            let pad = 10.0;
            let flock_h = pad + seats_h + skill_h + pick_h + pad;
            // Room at the end so the picker scrolls clear of the pinned buttons.
            let content_h = room_h + S3 + badges_h + bt_h + flock_h + S3;
            common::scroll(ui, area, "lobby-scroll", content_h, |ui, c| {
                let p = ui.painter().clone();
                let mut y = c.top();
                // ── the room card ──
                let card = Rect::from_min_size(pos2(c.left(), y), vec2(c.width(), room_h));
                widgets::panel(&p, card, 22.0);
                if snap.is_host {
                    p.text(pos2(card.left() + 18.0, card.top() + 18.0), Align2::LEFT_CENTER, "ROOM CODE", theme::ui_bold(11.0), theme::TEXT_DIM);
                    match (&snap.code, snap.code_ready) {
                        (Some(code), true) => {
                            art::outlined(&p, pos2(card.center().x, card.top() + 52.0), Align2::CENTER_CENTER, &online::spaced_code(code), theme::title(48.0), theme::GOLD, theme::OUTLINE, 3.5, 2.0);
                        }
                        _ => {
                            let (msg, col) = match &snap.status.failure {
                                Some(f) => (format!("Couldn't open the room: {f}"), Color32::from_rgb(0xFF, 0x8A, 0x7A)),
                                None if snap.code.is_none() && snap.status.relays.1 == 0 && snap.status.ready => ("Nearby only: no online code".to_string(), theme::SAND),
                                None => ("Opening the room\u{2026}".to_string(), theme::SAND),
                            };
                            p.text(pos2(card.center().x, card.top() + 56.0), Align2::CENTER_CENTER, msg, theme::title(20.0), col);
                        }
                    }
                    let can = snap.code.is_some() && snap.code_ready;
                    let bw = (card.width() - 36.0 - S2) / 2.0;
                    let by = card.top() + 74.0;
                    let copy_label = if copied == Some(false) { "Copied" } else { "Copy code" };
                    if widgets::button(ui, Rect::from_min_size(pos2(card.left() + 18.0, by), vec2(bw, 36.0)), "lobby-copy", &Btn::new(copy_label).enabled(can).size(15.0)).clicked() {
                        acts.push(Act::Copy);
                    }
                    let link_label = if copied == Some(true) { "Link copied" } else { "Copy link" };
                    if widgets::button(ui, Rect::from_min_size(pos2(card.left() + 18.0 + bw + S2, by), vec2(bw, 36.0)), "lobby-link", &Btn::new(link_label).enabled(can).size(15.0)).clicked() {
                        acts.push(Act::Link);
                    }
                    let share_btn = Btn::new("Share invite").icon(Icon::Share).enabled(can).size(15.0);
                    if widgets::button(ui, Rect::from_min_size(pos2(card.left() + 18.0, by + 36.0 + S2), vec2(card.width() - 36.0, 36.0)), "lobby-share", &share_btn).clicked() {
                        acts.push(Act::Share);
                    }
                } else {
                    let door = match snap.joined_by {
                        JoinedBy::Bluetooth => Door::Bluetooth,
                        JoinedBy::Nearby => Door::Nearby,
                        _ => Door::Online,
                    };
                    p.text(pos2(card.left() + 18.0, card.top() + 20.0), Align2::LEFT_CENTER, "YOU JOINED", theme::ui_bold(11.0), theme::TEXT_DIM);
                    let hn = if snap.connecting { "Connecting\u{2026}".to_string() } else { snap.host_name.clone() };
                    let size = art::fit_size(&p, &hn, theme::title, 28.0, card.width() - 150.0);
                    p.text(pos2(card.left() + 18.0, card.top() + 46.0), Align2::LEFT_CENTER, hn, theme::title(size), theme::TEXT);
                    online_common::door_badge(&p, pos2(card.right() - 114.0, card.top() + 12.0), door, DoorState::On, tm);
                    let (persona, col) = match &snap.host_persona {
                        Some(n) => (format!("Host verified: {}", online_common::short_npub(n)), theme::MINT),
                        None => ("Waiting for the host\u{2026}".to_string(), theme::TEXT_DIM),
                    };
                    p.text(pos2(card.left() + 18.0, card.bottom() - 24.0), Align2::LEFT_CENTER, persona, theme::ui_medium(12.0), col);
                    if let Some(rtt) = snap.rtt {
                        let q = online::quality(Some(rtt));
                        let qc = [theme::MINT, theme::GOLD, theme::LASER][q as usize];
                        p.circle_filled(pos2(card.right() - 80.0, card.bottom() - 24.0), 5.0, qc);
                        p.text(pos2(card.right() - 68.0, card.bottom() - 24.0), Align2::LEFT_CENTER, format!("{rtt} ms"), theme::ui_medium(12.0), theme::TEXT_DIM);
                    }
                }
                y += room_h + S3;

                // ── the doors (host) ──
                if snap.is_host {
                    let total = snap.status.relays.1;
                    let online_state = if snap.status.failure.is_some() || total == 0 {
                        DoorState::Off
                    } else if snap.code.is_some() && snap.code_ready {
                        DoorState::On
                    } else {
                        DoorState::Opening
                    };
                    let nearby_state = if snap.status.lan.is_some() && snap.status.ready { DoorState::On } else if snap.status.failure.is_some() { DoorState::Off } else { DoorState::Opening };
                    let bt_state = match &snap.bt {
                        BtStatus::On { .. } => DoorState::On,
                        _ => DoorState::Off,
                    };
                    let mut x = c.left();
                    for (d, s) in [(Door::Online, online_state), (Door::Nearby, nearby_state), (Door::Bluetooth, bt_state)] {
                        if d == Door::Bluetooth && bt.is_none() {
                            continue;
                        }
                        let r = online_common::door_badge(&p, pos2(x, y), d, s, tm);
                        x = r.right() + S2;
                    }
                    y += badges_h;
                }

                // ── the Bluetooth line (host) ──
                if let Some(line) = &bt {
                    let r = Rect::from_min_size(pos2(c.left(), y), vec2(c.width(), 42.0));
                    widgets::panel(&p, r, 14.0);
                    let col = if line.state == DoorState::On { theme::MINT } else { theme::SAND };
                    online_common::bluetooth_glyph(&p, pos2(r.left() + 22.0, r.center().y), 22.0, col);
                    let text_w = r.width() - 44.0 - 124.0;
                    let g = p.layout(line.text.clone(), theme::ui_medium(12.5), theme::TEXT, text_w);
                    p.galley(pos2(r.left() + 42.0, r.center().y - g.size().y / 2.0), g, theme::TEXT);
                    match &line.control {
                        BtControl::None => {}
                        BtControl::Switch(on) => {
                            let mut v = *on;
                            let sw = Rect::from_min_size(pos2(r.right() - 130.0, r.top()), vec2(122.0, r.height()));
                            if widgets::toggle(ui, sw, "lobby-bt", &mut v).changed() {
                                acts.push(Act::Bt(if v { BtAct::On } else { BtAct::Off }));
                            }
                        }
                        BtControl::Button(label, act) => {
                            let b = Rect::from_center_size(pos2(r.right() - 58.0, r.center().y), vec2(104.0, 34.0));
                            if widgets::button(ui, b, "lobby-bt-btn", &Btn::new(label).size(14.0)).clicked() {
                                acts.push(Act::Bt(*act));
                            }
                        }
                    }
                    y += 42.0 + 8.0;
                }

                // ── the seats ──
                widgets::panel(&p, Rect::from_min_size(pos2(c.left(), y), vec2(c.width(), flock_h)), 18.0);
                y += pad;
                let ix = c.left() + pad;
                let iw = c.width() - 2.0 * pad;
                p.text(pos2(ix + 4.0, y + 8.0), Align2::LEFT_CENTER, format!("THE FLOCK  {}/{}", snap.seats.len(), MAX_SEATS), theme::ui_bold(11.0), theme::TEXT_DIM);
                y += 22.0;
                let mut first_open = true;
                for i in 0..MAX_SEATS {
                    let r = Rect::from_min_size(pos2(ix, y + i as f32 * (seat_h + seat_gap)), vec2(iw, seat_h));
                    let e = widgets::enter(t, 1 + i / 2);
                    let mut q = p.clone();
                    q.set_opacity(e);
                    widgets::well(&q, r, 14.0);
                    match snap.seats.get(i) {
                        Some(s) => {
                            let mine = Some(s.id) == snap.my_seat && !s.bot;
                            if mine {
                                q.rect_stroke(r, 14.0, Stroke::new(2.0_f32, theme::GOLD), egui::StrokeKind::Inside);
                            }
                            online_common::bird_chip_hat(&q, pos2(r.left() + 28.0, r.center().y), 19.0, s.colour, s.hat, 1.0);
                            if mine && !snap.connecting {
                                // Tap your own bird for the next free colour.
                                let hit = Rect::from_center_size(pos2(r.left() + 28.0, r.center().y), vec2(48.0, 46.0));
                                if ui.interact(hit, egui::Id::new("lobby-my-bird"), egui::Sense::click()).clicked() {
                                    if let Some(next) = (1..online_common::COLOURS).map(|d| (s.colour + d) % online_common::COLOURS).find(|c| !snap.seats.iter().any(|o| o.colour == *c && o.id != s.id)) {
                                        acts.push(Act::Colour(next));
                                    }
                                }
                            }
                            let right = r.right() - 10.0;
                            let removable = snap.is_host && !s.host && !snap.connecting;
                            let name_w = right - (r.left() + 58.0) - 36.0 - if removable { 40.0 } else { 0.0 };
                            let size = art::fit_size(&q, &s.name, theme::ui_bold, 16.5, name_w.max(40.0));
                            q.text(pos2(r.left() + 58.0, r.center().y - 8.0), Align2::LEFT_CENTER, &s.name, theme::ui_bold(size), theme::TEXT);
                            let mut sub: Vec<String> = Vec::new();
                            if mine {
                                sub.push("You".into());
                            }
                            if s.host {
                                sub.push("Host".into());
                            }
                            if s.bot {
                                sub.push("Bot".into());
                            }
                            sub.push(online_common::colour_name(s.colour).to_string());
                            q.text(pos2(r.left() + 58.0, r.center().y + 9.0), Align2::LEFT_CENTER, sub.join(" \u{b7} "), theme::ui_medium(11.0), theme::TEXT_DIM);
                            // right side, right to left
                            let mut x = right;
                            if removable {
                                let b = Rect::from_center_size(pos2(x - 16.0, r.center().y), vec2(38.0, 38.0));
                                let resp = ui.interact(b, egui::Id::new(("lobby-remove", s.id)), egui::Sense::click());
                                q.circle_filled(b.center(), 12.0, if resp.is_pointer_button_down_on() { rgba(255, 255, 255, 0.22) } else { rgba(255, 255, 255, 0.10) });
                                icons::draw(&q, Icon::Close, b.center(), 13.0, theme::TEXT_DIM);
                                if resp.clicked() {
                                    acts.push(Act::Remove(s.id));
                                }
                                x -= 40.0;
                            }
                            if s.bot {
                                let rr = widgets::pill(&q, pos2(x - 18.0, r.center().y), "BOT", rgba(0x6A, 0x36, 0x96, 0.9), theme::TEXT, 9.5);
                                x = rr.left() - 4.0;
                            } else if s.ready {
                                icons::draw(&q, Icon::Check, pos2(x - 12.0, r.center().y), 20.0, theme::MINT);
                                x -= 28.0;
                            } else {
                                q.circle_stroke(pos2(x - 12.0, r.center().y), 7.0, Stroke::new(1.6_f32, rgba(0xB8, 0xA6, 0xC9, 0.55)));
                                x -= 28.0;
                            }
                            if s.path == SeatPath::Bluetooth {
                                online_common::bluetooth_glyph(&q, pos2(x - 8.0, r.center().y), 16.0, theme::SAND);
                            }
                        }
                        None if snap.is_host && first_open && snap.seats.len() < MAX_SEATS => {
                            first_open = false;
                            if widgets::button(ui, r.shrink2(vec2(2.0, 2.0)), "lobby-add-bot", &Btn::ghost("Add a bot").icon(Icon::Plus).size(16.0)).clicked() {
                                acts.push(Act::AddBot);
                            }
                        }
                        None => {
                            q.text(r.center(), Align2::CENTER_CENTER, "open seat", theme::ui_medium(12.0), rgba(0xB8, 0xA6, 0xC9, 0.5));
                        }
                    }
                }
                y += MAX_SEATS as f32 * (seat_h + seat_gap);

                // ── bot skill (host) ──
                if snap.is_host {
                    let r = Rect::from_min_size(pos2(ix, y), vec2(iw, 40.0));
                    p.text(pos2(r.left() + 4.0, r.center().y), Align2::LEFT_CENTER, "Bot skill", theme::ui_bold(13.5), theme::TEXT);
                    let sel = if skill.0 < 0.4 { 0 } else if skill.0 < 0.68 { 1 } else { 2 };
                    let seg = Rect::from_min_max(pos2(r.left() + 92.0, r.top()), r.max);
                    let segs = [Seg::new("Easy"), Seg::new("Normal"), Seg::new("Hard")];
                    if let Some(i) = widgets::segmented(ui, seg, "lobby-skill", &segs, sel) {
                        acts.push(Act::Skill([BotSkill::EASY, BotSkill::NORMAL, BotSkill::HARD][i]));
                    }
                    y += 40.0 + 8.0;
                }

                // ── your bird ──
                p.text(pos2(ix + 4.0, y + 8.0), Align2::LEFT_CENTER, "YOUR BIRD", theme::ui_bold(11.0), theme::TEXT_DIM);
                if let Some(mc) = my_colour {
                    p.text(pos2(ix + iw - 4.0, y + 8.0), Align2::RIGHT_CENTER, online_common::colour_name(mc), theme::ui_bold(12.0), theme::SAND);
                }
                let strip = Rect::from_min_size(pos2(ix, y + 20.0), vec2(iw, 64.0));
                widgets::well(&p, strip, 16.0);
                let n = online_common::COLOURS as usize;
                let cell = strip.width() / n as f32;
                for k in 0..n {
                    let col = k as u8;
                    let centre = pos2(strip.left() + cell * (k as f32 + 0.5), strip.center().y);
                    let holder = snap.seats.iter().find(|s| s.colour == col && Some(s.id) != snap.my_seat);
                    let mine = my_colour == Some(col);
                    let taken = holder.is_some();
                    let hit = Rect::from_center_size(centre, vec2(cell, strip.height()));
                    let resp = ui.interact(hit, egui::Id::new(("lobby-colour", k)), egui::Sense::click());
                    let r = if mine { 25.0 } else { 22.0 };
                    if mine {
                        art::glow(&p, centre, 36.0, rgba(0xFF, 0xC8, 0x3D, 0.30));
                        p.circle_stroke(centre, r + 3.0, Stroke::new(2.5_f32, theme::GOLD));
                    }
                    online_common::bird_chip_hat(&p, centre, r, col, my_hat, if taken { 0.35 } else { 1.0 });
                    if taken {
                        p.text(centre + vec2(0.0, r + 4.0), Align2::CENTER_CENTER, "taken", theme::ui_bold(8.5), rgba(0xB8, 0xA6, 0xC9, 0.8));
                    }
                    if resp.clicked() && !taken && !mine && !snap.connecting {
                        acts.push(Act::Colour(col));
                    }
                }
            });
        });

        // ── apply ──
        let mut out = Transition::None;
        for a in acts {
            if sample {
                if matches!(a, Act::Back) {
                    return Transition::Pop;
                }
                ctx.say("This is a preview.");
                continue;
            }
            match a {
                Act::Back => {
                    feedback::back(ctx);
                    let host = snap.is_host;
                    return self.leave(ctx, if host { Some("Room closed") } else { None });
                }
                Act::Ready(r) => {
                    feedback::tap(ctx);
                    online::with(|s| s.set_ready(r));
                }
                Act::Start => {
                    feedback::select(ctx);
                    online::with(|s| s.start());
                }
                Act::Copy => {
                    feedback::tap(ctx);
                    if let Some(c) = snap.code.as_ref().filter(|_| snap.code_ready) {
                        match ctx.platform.copy_text(c) {
                            Ok(()) => ctx.say("Room code copied"),
                            Err(e) => ctx.say(format!("Couldn't copy: {e}")),
                        }
                        self.copied = 0.0;
                        self.copied_link = false;
                    }
                }
                Act::Link => {
                    feedback::tap(ctx);
                    if let Some(c) = snap.code.as_ref().filter(|_| snap.code_ready) {
                        match ctx.platform.copy_text(&online::join_link(c)) {
                            Ok(()) => ctx.say("Join link copied. Paste it in a message."),
                            Err(e) => ctx.say(format!("Couldn't copy: {e}")),
                        }
                        self.copied = 0.0;
                        self.copied_link = true;
                    }
                }
                Act::Share => {
                    feedback::tap(ctx);
                    if let Some(c) = snap.code.as_ref().filter(|_| snap.code_ready) {
                        let r = ctx.platform.share_text(&invite_text(c));
                        if let Some(m) = crate::platform::os::share_note(&*ctx.platform, r) {
                            ctx.say(m);
                        }
                    }
                }
                Act::Colour(c) => {
                    feedback::tap(ctx);
                    ctx.profile.colour = Some(c);
                    online::with(|s| s.set_colour(c));
                }
                Act::AddBot => {
                    feedback::tap(ctx);
                    online::with(|s| {
                        s.set_bot_skill(skill);
                        s.add_bot()
                    });
                }
                Act::Remove(seat) => {
                    feedback::tap(ctx);
                    online::with(|s| if seat >= 100 { s.remove_bot(Some(seat)) } else { s.kick(seat) });
                }
                Act::Skill(sk) => {
                    feedback::tap(ctx);
                    self.bot_skill = sk;
                    online::with(|s| s.set_bot_skill(sk));
                }
                Act::Accept(peer) => {
                    feedback::select(ctx);
                    online::with(|s| s.accept(peer));
                }
                Act::Decline(peer) => {
                    feedback::back(ctx);
                    online::with(|s| s.decline(peer));
                }
                Act::Bt(b) => {
                    feedback::tap(ctx);
                    out = self.bluetooth_act(b);
                }
            }
        }
        out
    }
}

impl LobbyScreen {
    /// Open the Bluetooth door while the host wants it and the radio is ready.
    fn bluetooth_tick(&mut self, ctx: &mut Ctx) {
        if !self.bt_want {
            return;
        }
        let Some((host, door)) = online::with(|s| (s.is_host(), s.bluetooth())) else { return };
        if !host || !matches!(door, BtStatus::Off) || self.bt_err.is_some() {
            return;
        }
        let radio = online_flow::radio();
        match radio.state() {
            BleState::Ready => {
                let colour = ctx.profile.preferred_colour();
                if let Some(Err(e)) = online::with(|s| online_flow::start_bluetooth(s, colour)) {
                    log::info!("lobby: Bluetooth door failed: {e}");
                    self.bt_err = Some(e);
                }
            }
            BleState::NeedsPermission if !self.asked_permission => {
                self.asked_permission = true;
                radio.request_permission();
            }
            BleState::Off if !self.asked_enable => {
                self.asked_enable = true;
                radio.request_enable();
            }
            _ => {}
        }
    }

    fn bluetooth_act(&mut self, act: BtAct) -> Transition {
        let radio = online_flow::radio();
        match act {
            BtAct::On => {
                self.bt_want = true;
                self.bt_err = None;
                self.asked_permission = false;
                self.asked_enable = false;
                // A failed door is closed before it is opened again.
                online::with(|s| {
                    if matches!(s.bluetooth(), BtStatus::Failed(_)) {
                        s.stop_bluetooth();
                    }
                });
            }
            BtAct::Off => {
                self.bt_want = false;
                self.bt_err = None;
                online::with(|s| s.stop_bluetooth());
            }
            BtAct::Allow => {
                self.asked_permission = true;
                radio.request_permission();
            }
            BtAct::SwitchOn => {
                self.asked_enable = true;
                radio.request_enable();
            }
            BtAct::Settings => radio.open_settings(),
        }
        Transition::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invite_text_carries_the_code_and_the_copy_links_url() {
        let t = invite_text("KWTQPM");
        assert_eq!(t, "Join my Cluster Flock room KWTQPM: gameflock://join#KWTQPM");
        assert!(t.ends_with(&online::join_link("KWTQPM")));
    }
}
