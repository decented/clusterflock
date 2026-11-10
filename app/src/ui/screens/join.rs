//! Join by room code: six slots, an in-game keypad of the room-code letters
//! (24, no I or O), Paste, and honest connecting stages. The same page is the
//! "connecting" card for a Nearby or Bluetooth room that is already dialling.
//! Porting note: Stampede's `join.rs` without Keeps; Bluetooth adds the
//! "Waiting for the host to let you in…" stage.

use super::lobby::LobbyScreen;
use super::online_common::{self, Door};
use super::online_flow;
use super::{common, Ctx, Screen, Transition};
use crate::online;
use crate::ui::art::{self, rgba};
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, Btn, S2, S3};
use crate::ui::{feedback, theme};
use decented_net::room::{self, TypedChar, ALPHABET, CODE_LEN};
use egui::{pos2, vec2, Align2, Color32, Rect, Stroke};
use flock_core::InputFrame;
use flock_session::ClientPhase;

/// How long one attempt may take before we say so.
const GIVE_UP_SECS: f32 = 40.0;
/// A join by code that fails for a reason that may pass (the host's relays are
/// still coming up, a lost offer) is tried again until this many seconds have
/// gone by in all, then reported.
pub const RETRY_BUDGET_SECS: f32 = 35.0;

/// What Paste says when the clipboard holds no room code.
pub const NO_CODE_ON_CLIPBOARD: &str = "No room code on the clipboard.";

/// Would trying again soon plausibly work? (Not for a full or started room, a
/// wrong version, or an unverified host.)
pub fn is_transient(why: &str) -> bool {
    let base = online::friendly_failure(why);
    ["Code not found", "Couldn't reach the internet meeting points", "Found the host but couldn't connect", "didn't answer over the link", "Network problem"]
        .iter()
        .any(|m| base.contains(m))
}

/// The room code in clipboard text: a bare code, `KWT QPM`, or a `gameflock://join#CODE` link.
pub fn code_in(text: &str) -> Option<String> {
    let t = text.trim();
    if let Some(c) = room::normalize_code(t) {
        return Some(c);
    }
    if let Some((_, tail)) = t.rsplit_once('#') {
        if let Some(c) = room::normalize_code(tail) {
            return Some(c);
        }
    }
    t.split(|c: char| !c.is_ascii_alphabetic()).find_map(|w| (w.len() == CODE_LEN).then(|| room::normalize_code(w)).flatten())
}

enum Stage {
    Entry,
    /// The session being joined is in the app's online slot.
    Connecting { age: f32 },
    Failed(String),
}

pub struct JoinScreen {
    code: String,
    stage: Stage,
    /// What we are joining, for the connecting page.
    label: String,
    door: Door,
    t: f32,
    shake: f32,
    hint: Option<(String, f32)>,
    /// Join this on the first frame (a link opened the app).
    auto: Option<String>,
    /// Attempt number (a join by code is retried while the host's relays come up).
    attempt: u32,
    /// Seconds since the first attempt of this join.
    total: f32,
    retry: bool,
    /// Screenshots: the connecting page with no session behind it.
    staged: bool,
}

impl Default for JoinScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl JoinScreen {
    pub fn new() -> JoinScreen {
        JoinScreen { code: String::new(), stage: Stage::Entry, label: String::new(), door: Door::Online, t: 0.0, shake: 0.0, hint: None, auto: None, attempt: 0, total: 0.0, retry: true, staged: false }
    }

    /// Straight into joining (a `gameflock://join#CODE` link). A code that
    /// isn't one opens the keypad instead.
    pub fn with_code(code: &str) -> JoinScreen {
        match room::normalize_code(code) {
            Some(c) => JoinScreen { auto: Some(c), ..JoinScreen::new() },
            None => JoinScreen::new(),
        }
    }

    /// The connecting page for a session already in the app's slot (Nearby,
    /// Bluetooth): `label` is the room's name.
    pub fn connecting(door: Door, label: &str) -> JoinScreen {
        JoinScreen { label: label.to_string(), door, retry: false, stage: Stage::Connecting { age: 0.0 }, attempt: 1, ..JoinScreen::new() }
    }

    /// Four letters typed (screenshots).
    pub fn sample() -> JoinScreen {
        JoinScreen { code: "KWTQ".into(), ..JoinScreen::new() }
    }

    /// A Bluetooth join waiting for the host (screenshots).
    pub fn waiting_sample() -> JoinScreen {
        JoinScreen { stage: Stage::Connecting { age: 3.0 }, label: "Bluetooth game \u{b7} room 1A2B".into(), door: Door::Bluetooth, retry: false, attempt: 1, staged: true, ..JoinScreen::new() }
    }

    /// A failed attempt (screenshots).
    pub fn failed_sample() -> JoinScreen {
        JoinScreen { code: "KWTQPM".into(), stage: Stage::Failed("Code not found (check the code, or the host may have started).".into()), ..JoinScreen::new() }
    }

    fn say(&mut self, text: impl Into<String>) {
        self.hint = Some((text.into(), 2.8));
    }

    /// Start joining `code` from scratch (a new attempt count).
    fn begin(&mut self, ctx: &mut Ctx, code: &str) {
        self.attempt = 0;
        self.total = 0.0;
        self.retry = true;
        self.door = Door::Online;
        self.code = code.to_string();
        self.attempt_join(ctx, code);
    }

    fn attempt_join(&mut self, ctx: &mut Ctx, code: &str) {
        if let Some(gate) = super::account::gate(ctx) {
            // Approval was withdrawn while here: ask again, keep the keypad.
            self.stage = Stage::Entry;
            self.auto = None;
            let _ = gate;
            ctx.say("My Signet approval is needed to play online.");
            return;
        }
        self.attempt += 1;
        match online_flow::open_join_code(ctx, code) {
            Ok(()) => {
                self.label = format!("Room {}", online::spaced_code(code));
                self.stage = Stage::Connecting { age: 0.0 };
            }
            Err(e) => self.stage = Stage::Failed(e),
        }
    }

    fn fail(&mut self, why: String) {
        online::leave();
        self.stage = Stage::Failed(why);
    }

    fn push_letter(&mut self, c: char) {
        if self.code.chars().count() < CODE_LEN {
            self.code.push(c);
        }
    }
}

enum Step {
    Seated,
    Dead(String),
    Waiting,
    Gone,
}

/// Letters pressed on a keyboard (desktop).
fn typed_letters(ctx: &Ctx) -> Vec<char> {
    use winit::keyboard::KeyCode as K;
    const KEYS: [(K, char); 26] = [
        (K::KeyA, 'A'), (K::KeyB, 'B'), (K::KeyC, 'C'), (K::KeyD, 'D'), (K::KeyE, 'E'), (K::KeyF, 'F'), (K::KeyG, 'G'),
        (K::KeyH, 'H'), (K::KeyI, 'I'), (K::KeyJ, 'J'), (K::KeyK, 'K'), (K::KeyL, 'L'), (K::KeyM, 'M'), (K::KeyN, 'N'),
        (K::KeyO, 'O'), (K::KeyP, 'P'), (K::KeyQ, 'Q'), (K::KeyR, 'R'), (K::KeyS, 'S'), (K::KeyT, 'T'), (K::KeyU, 'U'),
        (K::KeyV, 'V'), (K::KeyW, 'W'), (K::KeyX, 'X'), (K::KeyY, 'Y'), (K::KeyZ, 'Z'),
    ];
    KEYS.iter().filter(|(k, _)| ctx.keys.pressed(*k)).map(|(_, c)| *c).collect()
}

impl Screen for JoinScreen {
    fn name(&self) -> &'static str {
        "join"
    }

    fn on_background(&mut self, _ctx: &mut Ctx) {
        feedback::on_background();
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        common::begin(ctx, &mut self.t);
        self.shake = (self.shake - ctx.dt * 3.0).max(0.0);
        if let Some((_, left)) = self.hint.as_mut() {
            *left -= ctx.dt;
            if *left <= 0.0 {
                self.hint = None;
            }
        }
        // A join link while here: join that room instead.
        if let Some(c) = online_flow::take_join().and_then(|c| room::normalize_code(&c)) {
            let same = self.code == c && matches!(self.stage, Stage::Connecting { .. });
            if !same {
                online::leave();
                self.auto = Some(c);
            }
        }
        if let Some(code) = self.auto.take() {
            self.begin(ctx, &code);
        }
        if self.staged {
            return Transition::None;
        }
        match &mut self.stage {
            Stage::Entry => {
                for c in typed_letters(ctx) {
                    match room::typed_char(c) {
                        TypedChar::Letter(l) => self.push_letter(l),
                        TypedChar::Ignored => {}
                        TypedChar::Rejected => {
                            self.shake = 1.0;
                            self.say(room::rejected_hint(c));
                        }
                    }
                }
                if ctx.keys.pressed(winit::keyboard::KeyCode::Backspace) {
                    self.code.pop();
                }
                if ctx.keys.pressed(winit::keyboard::KeyCode::Enter) && self.code.chars().count() == CODE_LEN {
                    let c = self.code.clone();
                    self.begin(ctx, &c);
                }
            }
            Stage::Connecting { age } => {
                *age += ctx.dt;
                let age = *age;
                self.total += ctx.dt;
                let step = online::with(|s| {
                    s.pump(InputFrame::default());
                    if let Some(why) = s.dead_reason() {
                        return Step::Dead(why);
                    }
                    let seated = match s.phase() {
                        ClientPhase::Lobby => s.lobby().is_some(),
                        ClientPhase::Running => s.me().view().is_some(),
                        ClientPhase::Results => true,
                        _ => false,
                    };
                    if seated {
                        Step::Seated
                    } else {
                        Step::Waiting
                    }
                })
                .unwrap_or(Step::Gone);
                match step {
                    Step::Seated => return Transition::Replace(Box::new(LobbyScreen::new())),
                    Step::Dead(why) => {
                        log::info!("join: attempt {} failed after {:.0}s: {why}", self.attempt, self.total);
                        if self.retry && self.code.chars().count() == CODE_LEN && is_transient(&why) && self.total < RETRY_BUDGET_SECS {
                            // The host's relays may still be coming up (or our offer was lost): ask again.
                            online::leave();
                            let c = self.code.clone();
                            self.attempt_join(ctx, &c);
                        } else {
                            self.fail(why);
                        }
                    }
                    Step::Gone => self.stage = Stage::Failed("The connection was cancelled.".into()),
                    Step::Waiting if age > GIVE_UP_SECS && self.door != Door::Bluetooth => {
                        log::info!("join: gave up after {age:.0}s");
                        self.fail("That's taking too long. Check you are online and the host is still waiting, then try again.".into());
                    }
                    Step::Waiting => {}
                }
            }
            Stage::Failed(_) => {}
        }
        Transition::None
    }

    fn back(&mut self, ctx: &mut Ctx) -> Transition {
        feedback::back(ctx);
        // Cancelling a typed-code attempt returns to the keypad; anything else leaves.
        if matches!(self.stage, Stage::Connecting { .. }) {
            online::leave();
            if self.retry && self.code.chars().count() == CODE_LEN {
                self.stage = Stage::Entry;
                return Transition::None;
            }
        }
        Transition::Pop
    }

    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        let safe = ctx.viewport.safe;
        let (t, tm) = (self.t, ctx.time as f32);
        let mut out = Transition::None;
        let entry = matches!(self.stage, Stage::Entry);
        let mut typed: Vec<char> = Vec::new();
        let (mut backspace, mut paste, mut go, mut retry, mut leave) = (false, false, false, false, false);
        let code = self.code.clone();
        let shake = self.shake;
        let hint = self.hint.clone();
        let label = self.label.clone();
        let door = self.door;
        let staged_waiting = self.staged;
        let (stage_text, waiting) = match &self.stage {
            Stage::Connecting { .. } if staged_waiting => ("Waiting for the host to let you in\u{2026}".to_string(), true),
            Stage::Connecting { .. } => online::with(|s| match s.phase() {
                ClientPhase::Connecting(text) => (text.clone(), false),
                ClientPhase::Waiting => ("Waiting for the host to let you in\u{2026}".to_string(), true),
                _ => ("Joining\u{2026}".to_string(), false),
            })
            .unwrap_or_else(|| ("Joining\u{2026}".to_string(), false)),
            _ => (String::new(), false),
        };
        let progress = if waiting || door == Door::Bluetooth {
            "Stay close to the host and keep Bluetooth on.".to_string()
        } else if self.attempt > 1 {
            format!("Still looking\u{2026} the host may still be getting ready (try {})", self.attempt)
        } else {
            "This can take a few seconds.".to_string()
        };
        let failed = match &self.stage {
            Stage::Failed(m) => Some(m.clone()),
            _ => None,
        };
        common::page(ctx.viewport.screen, tm, ui, 0.6, |ui, _brand| {
            let title = if entry { "Join with code" } else if failed.is_some() { "Couldn't join" } else { "Joining" };
            let hd = widgets::header(ui, safe, title, widgets::enter(t, 0));
            if hd.back.clicked() {
                leave = true;
            }
            let body = hd.body;
            let p = ui.painter().clone();
            if entry {
                let n = code.chars().count();
                p.text(pos2(body.center().x, body.top() + 12.0), Align2::CENTER_CENTER, "Ask the host for their six-letter room code.", theme::ui_medium(14.0), theme::TEXT_DIM);
                // ── the slots ──
                let gap = 7.0;
                let mid = 12.0;
                let slot_w = ((body.width() - mid - gap * 4.0) / CODE_LEN as f32).min(52.0);
                let slots_w = CODE_LEN as f32 * slot_w + gap * 4.0 + mid;
                let slot_h = (slot_w * 1.28).min(66.0);
                let top = body.top() + 34.0;
                let x0 = body.center().x - slots_w / 2.0;
                let sx = (shake * 40.0).sin() * shake * 6.0;
                for i in 0..CODE_LEN {
                    let extra = if i >= 3 { mid } else { 0.0 };
                    let r = Rect::from_min_size(pos2(x0 + sx + i as f32 * (slot_w + gap) + extra, top), vec2(slot_w, slot_h));
                    let e = widgets::enter(t, 1 + i / 3);
                    let mut q = p.clone();
                    q.set_opacity(e);
                    widgets::well(&q, r, 12.0);
                    if i == n {
                        q.rect_stroke(r, 12.0, Stroke::new(2.0_f32, rgba(0xFF, 0xC8, 0x3D, 0.6 + 0.4 * (tm * 6.0).sin().abs())), egui::StrokeKind::Inside);
                    }
                    if let Some(c) = code.chars().nth(i) {
                        art::outlined(&q, r.center(), Align2::CENTER_CENTER, &c.to_string(), theme::title(slot_w * 0.78), theme::TEXT, theme::OUTLINE, 2.5, 0.0);
                    }
                }
                // ── paste / backspace / join ──
                let by = top + slot_h + S3;
                let bh = 48.0;
                let paste_w = body.width() * 0.30;
                let bs_w = body.width() * 0.18;
                let join_w = body.width() - paste_w - bs_w - 2.0 * S2;
                if widgets::button(ui, Rect::from_min_size(pos2(body.left(), by), vec2(paste_w, bh)), "join-paste", &Btn::new("Paste").size(16.0)).clicked() {
                    paste = true;
                }
                if widgets::button(ui, Rect::from_min_size(pos2(body.left() + paste_w + S2, by), vec2(bs_w, bh)), "join-bs", &Btn::new("").icon(Icon::Backspace)).clicked() {
                    backspace = true;
                }
                let ready = n == CODE_LEN;
                if widgets::button(ui, Rect::from_min_size(pos2(body.right() - join_w, by), vec2(join_w, bh)), "join-go", &Btn::primary("Join").enabled(ready).size(20.0)).clicked() {
                    go = true;
                }
                // the line under the buttons
                let hint_y = by + bh + 16.0;
                match &hint {
                    Some((h, _)) => {
                        p.text(pos2(body.center().x, hint_y), Align2::CENTER_CENTER, h, theme::ui_bold(13.0), Color32::from_rgb(0xFF, 0xB0, 0x8A));
                    }
                    None => {
                        p.text(pos2(body.center().x, hint_y), Align2::CENTER_CENTER, "No I or O in a code.", theme::ui_medium(12.0), theme::TEXT_DIM);
                    }
                }
                // ── the keypad: the 24 code letters, six to a row ──
                let rows: Vec<Vec<char>> = ALPHABET.chars().collect::<Vec<_>>().chunks(6).map(|c| c.to_vec()).collect();
                let kgap = 8.0;
                let kw = ((body.width() - kgap * 5.0) / 6.0).min(70.0);
                let avail = body.bottom() - (hint_y + 16.0);
                let kh = ((avail - kgap * 3.0) / 4.0).clamp(40.0, 64.0);
                let ky = body.bottom() - (kh * 4.0 + kgap * 3.0);
                let kx = body.center().x - (kw * 6.0 + kgap * 5.0) / 2.0;
                for (ri, keys) in rows.iter().enumerate() {
                    for (ci, c) in keys.iter().enumerate() {
                        let r = Rect::from_min_size(pos2(kx + ci as f32 * (kw + kgap), ky + ri as f32 * (kh + kgap)), vec2(kw, kh));
                        let e = widgets::enter(t, 2 + ri);
                        let clicked = widgets::staged(ui, e, vec2(0.0, 20.0), |ui, off| widgets::button(ui, r.translate(off), &format!("join-key-{c}"), &Btn::new(&c.to_string()).size(24.0)).clicked());
                        if clicked {
                            typed.push(*c);
                        }
                    }
                }
            } else if let Some(msg) = &failed {
                let card = Rect::from_center_size(body.center() - vec2(0.0, 40.0), vec2(body.width().min(560.0), 250.0));
                widgets::panel(&p, card, 24.0);
                p.text(pos2(card.center().x, card.top() + 38.0), Align2::CENTER_CENTER, "Couldn't join", theme::title(26.0), Color32::from_rgb(0xFF, 0x8A, 0x7A));
                let g = p.layout(msg.clone(), theme::ui_medium(14.5), theme::TEXT, card.width() - 56.0);
                p.galley(pos2(card.left() + 28.0, card.top() + 66.0), g, theme::TEXT);
                let can_retry = code.chars().count() == CODE_LEN && self.retry;
                let buttons = 1 + usize::from(can_retry);
                let bw = (card.width() - 56.0 - S3 * (buttons as f32 - 1.0)) / buttons as f32;
                let by = card.bottom() - 22.0 - 46.0;
                let mut bx = card.left() + 28.0;
                if widgets::button(ui, Rect::from_min_size(pos2(bx, by), vec2(bw, 46.0)), "join-fail-back", &Btn::new("Back")).clicked() {
                    leave = true;
                }
                bx += bw + S3;
                if can_retry && widgets::button(ui, Rect::from_min_size(pos2(bx, by), vec2(bw, 46.0)), "join-retry", &Btn::primary("Try again")).clicked() {
                    retry = true;
                }
            } else {
                // connecting
                let card = Rect::from_center_size(body.center() - vec2(0.0, 40.0), vec2(body.width().min(520.0), 262.0));
                widgets::panel(&p, card, 24.0);
                let c = pos2(card.center().x, card.top() + 56.0);
                if door == Door::Bluetooth && waiting {
                    online_common::bluetooth_glyph(&p, c, 38.0, theme::SAND);
                    for k in 0..3 {
                        let ph = ((tm * 0.8 + k as f32 / 3.0) % 1.0).clamp(0.0, 1.0);
                        p.circle_stroke(c, 16.0 + ph * 26.0, Stroke::new(2.0_f32, rgba(0xF7, 0x93, 0x1A, 0.7 * (1.0 - ph))));
                    }
                } else {
                    online_common::spinner(&p, c, 26.0, tm);
                }
                let size = art::fit_size(&p, &stage_text, theme::title, 23.0, card.width() - 40.0);
                p.text(pos2(card.center().x, card.top() + 112.0), Align2::CENTER_CENTER, &stage_text, theme::title(size), theme::TEXT);
                p.text(pos2(card.center().x, card.top() + 140.0), Align2::CENTER_CENTER, &label, theme::ui_bold(14.0), theme::SAND);
                let g = p.layout(progress.clone(), theme::ui_medium(12.5), theme::TEXT_DIM, card.width() - 40.0);
                p.galley(pos2(card.center().x - g.size().x / 2.0, card.top() + 156.0), g, theme::TEXT_DIM);
                if widgets::button(ui, Rect::from_min_size(pos2(card.left() + 28.0, card.bottom() - 22.0 - 42.0), vec2(card.width() - 56.0, 42.0)), "join-cancel", &Btn::new("Cancel")).clicked() {
                    leave = true;
                }
            }
        });
        // apply
        for c in typed {
            feedback::tap(ctx);
            self.push_letter(c);
        }
        if backspace {
            feedback::tap(ctx);
            self.code.pop();
        }
        if paste {
            feedback::tap(ctx);
            match ctx.platform.read_clipboard().as_deref().and_then(code_in) {
                Some(c) => {
                    self.code = c;
                    self.hint = None;
                }
                None => self.say(NO_CODE_ON_CLIPBOARD),
            }
        }
        if go && self.code.chars().count() == CODE_LEN {
            feedback::select(ctx);
            let c = self.code.clone();
            self.begin(ctx, &c);
        }
        if retry {
            feedback::select(ctx);
            let c = self.code.clone();
            self.begin(ctx, &c);
        }
        if leave {
            out = self.back(ctx);
            if matches!(out, Transition::None) {
                return out;
            }
            online::leave();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_found_in_clipboard_text() {
        assert_eq!(code_in("kwtqpm").as_deref(), Some("KWTQPM"));
        assert_eq!(code_in("KWT QPM").as_deref(), Some("KWTQPM"));
        assert_eq!(code_in("gameflock://join#KWTQPM").as_deref(), Some("KWTQPM"));
        assert_eq!(code_in("Join my game: KWTQPM!").as_deref(), Some("KWTQPM"));
        assert_eq!(code_in("hello world"), None);
        assert_eq!(code_in("IOIOIO"), None, "I and O are never in a code");
    }

    #[test]
    fn only_passing_trouble_is_retried() {
        assert!(is_transient("Code not found (check the code). [Looking up the room]"));
        assert!(!is_transient("Room full."));
        assert!(!is_transient("The host is running a different version of the game."));
    }
}
