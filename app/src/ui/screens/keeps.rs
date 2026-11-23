//! My Keeps: the Keeps (home game servers) the player has been let into, one
//! tap to rejoin with no link and no room code.
//!
//! A Keep is saved here when a join by invite link gets the player in, or when
//! the Keep hands the phone a fresh pass afterwards (see `Engine`'s pass
//! saving). The list lives in `keeps.json` ([`crate::keeps`]) and is Fray's
//! `fray-keep` list: newest play first, twenty at most. Join dials the secret
//! the file holds at that moment. Forget takes a Keep out after a second tap.
//!
//! Joining needs a My Signet persona like every online mode here: the screen is
//! in `engine::ONLINE_SCREENS` and Join goes through [`super::account::gate`].

use super::join::JoinScreen;
use super::{common, Ctx, Screen, Transition};
use crate::keeps::{self, Keep};
use crate::ui::art;
use crate::ui::widgets::{self, Btn, S2, S3};
use crate::ui::{feedback, theme};
use egui::{pos2, vec2, Align2, Color32, Rect};

/// What a tap on the page asks for (free of egui, so tests can drive it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeepsAction {
    /// Rejoin the Keep with this host key (hex).
    Join(String),
    /// First tap on Forget: ask.
    AskForget(String),
    CancelForget,
    /// Second tap: take the Keep out of the list.
    Forget(String),
}

/// How tall a Keep's row is.
const ROW_H: f32 = 108.0;
const BTN_H: f32 = 40.0;

pub const EMPTY_TEXT: &str = "No Keeps yet. Join one with an invite link and it will be saved here.";
pub const FORGET_ASK: &str = "Forget this Keep? You will need a new invite to return.";

pub struct KeepsScreen {
    t: f32,
    /// The saved Keeps, newest play first; `None` until read from `keeps.json`.
    list: Option<Vec<Keep>>,
    /// The Keep (host key) whose Forget is waiting for a second tap.
    confirm: Option<String>,
    /// Screenshots: show `list` as given and never read the file.
    sample: bool,
    /// Pending actions from the last draw.
    acts: Vec<KeepsAction>,
}

impl Default for KeepsScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl KeepsScreen {
    pub fn new() -> KeepsScreen {
        KeepsScreen { t: 0.0, list: None, confirm: None, sample: false, acts: Vec::new() }
    }

    /// Three saved Keeps (screenshots).
    pub fn sample() -> KeepsScreen {
        let now = keeps::unix_now();
        let list = vec![
            Keep::new(&[0x11; 32], &[1; 32], Some("Cousin's Keep"), now - 2 * 3600),
            Keep::new(&[0x22; 32], &[2; 32], None, now - 3 * 86_400),
            Keep::new(&[0x33; 32], &[3; 32], Some("The Friday Gang"), now - 40 * 86_400),
        ];
        KeepsScreen { list: Some(list), sample: true, confirm: Some("22".repeat(32)), ..KeepsScreen::new() }
    }

    /// The Keeps shown (tests); empty until the first frame or action reads them.
    pub fn keeps(&self) -> &[Keep] {
        self.list.as_deref().unwrap_or(&[])
    }

    /// The Keep whose Forget is waiting for its second tap (tests).
    pub fn confirming(&self) -> Option<&str> {
        self.confirm.as_deref()
    }

    fn load(&mut self, ctx: &Ctx) {
        self.list = Some(keeps::load(ctx.data_dir));
    }

    /// What a tap does.
    pub fn act(&mut self, a: KeepsAction, ctx: &mut Ctx) -> Transition {
        if self.list.is_none() && !self.sample {
            self.load(ctx);
        }
        match a {
            KeepsAction::Join(host_key) => {
                self.confirm = None;
                // The row is only for drawing: the secret to dial is read from the file at this
                // moment, so a Keep joined by a newer invite is never dialled with the old one.
                let fresh = if self.sample { self.keeps().iter().find(|k| k.host_key.eq_ignore_ascii_case(&host_key)).cloned() } else { keeps::load(ctx.data_dir).into_iter().find(|k| k.host_key.eq_ignore_ascii_case(&host_key)) };
                if !self.sample {
                    self.load(ctx);
                }
                match (super::account::gate(ctx), fresh.as_ref().and_then(Keep::keys)) {
                    (Some(gate), _) => gate,
                    (None, Some((host_key, secret))) => Transition::Push(Box::new(JoinScreen::rejoining(host_key, secret))),
                    (None, None) if fresh.is_none() => {
                        ctx.say("That Keep is no longer in My Keeps.");
                        Transition::None
                    }
                    (None, None) => {
                        ctx.say(keeps::NOT_AN_INVITE);
                        Transition::None
                    }
                }
            }
            KeepsAction::AskForget(host_key) => {
                self.confirm = Some(host_key);
                Transition::None
            }
            KeepsAction::CancelForget => {
                self.confirm = None;
                Transition::None
            }
            KeepsAction::Forget(host_key) => {
                self.confirm = None;
                let name = self.keeps().iter().find(|k| k.host_key.eq_ignore_ascii_case(&host_key)).map(|k| k.name.clone());
                if self.sample {
                    if let Some(l) = self.list.as_mut() {
                        keeps::forget(l, &host_key);
                    }
                } else {
                    match keeps::forget_saved(ctx.data_dir, &host_key) {
                        Ok(()) => ctx.say(format!("Forgot {}.", name.unwrap_or_else(|| "that Keep".into()))),
                        Err(e) => ctx.say(format!("Couldn't forget that Keep: {e}")),
                    }
                    self.load(ctx);
                }
                Transition::None
            }
        }
    }
}

impl Screen for KeepsScreen {
    fn name(&self) -> &'static str {
        "keeps"
    }

    fn on_background(&mut self, _ctx: &mut Ctx) {
        feedback::on_background();
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        common::begin(ctx, &mut self.t);
        if self.list.is_none() && !self.sample {
            self.load(ctx);
        }
        let acts = std::mem::take(&mut self.acts);
        let mut out = Transition::None;
        for a in acts {
            feedback::select(ctx);
            let t = self.act(a, ctx);
            if !t.is_none() {
                out = t;
            }
        }
        out
    }

    fn back(&mut self, ctx: &mut Ctx) -> Transition {
        feedback::back(ctx);
        Transition::Pop
    }

    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        let safe = ctx.viewport.safe;
        let (t, tm) = (self.t, ctx.time as f32);
        let mut out = Transition::None;
        let list = self.list.clone().unwrap_or_default();
        let confirm = self.confirm.clone();
        let now = keeps::unix_now();
        let mut acts: Vec<KeepsAction> = Vec::new();
        common::page(ctx.viewport.screen, tm, ui, 0.6, |ui, _| {
            let hd = widgets::header(ui, safe, "My Keeps", widgets::enter(t, 0));
            if hd.back.clicked() {
                feedback::back(ctx);
                out = Transition::Pop;
            }
            let body = hd.body;
            let p = ui.painter().clone();
            if list.is_empty() {
                let g = p.layout(EMPTY_TEXT.to_string(), theme::ui_medium(15.0), theme::TEXT_DIM, (body.width() - 24.0).min(340.0));
                p.galley(pos2(body.center().x - g.size().x / 2.0, body.top() + 40.0), g, theme::TEXT_DIM);
                return;
            }
            let step = ROW_H + S2;
            let height = list.len() as f32 * step;
            common::scroll(ui, body, "keeps-scroll", height, |ui, c| {
                let clip = ui.clip_rect();
                for (i, keep) in list.iter().enumerate() {
                    let row = Rect::from_min_size(pos2(c.left(), c.top() + i as f32 * step), vec2(c.width(), ROW_H));
                    if row.bottom() < clip.top() || row.top() > clip.bottom() {
                        continue;
                    }
                    let p = ui.painter().clone();
                    widgets::panel_solid(&p, row, 14.0);
                    let asking = confirm.as_deref() == Some(keep.host_key.as_str());
                    let by = row.bottom() - 10.0 - BTN_H;
                    let bw = (row.width() - 20.0 - S3) / 2.0;
                    let (left, right) = (Rect::from_min_size(pos2(row.left() + 10.0, by), vec2(bw, BTN_H)), Rect::from_min_size(pos2(row.left() + 10.0 + bw + S3, by), vec2(bw, BTN_H)));
                    let name_w = row.width() - 32.0;
                    let size = art::fit_size(&p, &keep.name, theme::title, 20.0, name_w);
                    p.text(row.left_top() + vec2(16.0, 20.0), Align2::LEFT_CENTER, &keep.name, theme::title(size), theme::TEXT);
                    if asking {
                        let g = p.layout(FORGET_ASK.to_string(), theme::ui_medium(11.5), Color32::from_rgb(0xFF, 0xB0, 0x8A), name_w);
                        p.galley(row.left_top() + vec2(16.0, 36.0), g, Color32::from_rgb(0xFF, 0xB0, 0x8A));
                        if widgets::button(ui, left, &format!("keep-cancel-{i}"), &Btn::new("Cancel").size(15.0)).clicked() {
                            acts.push(KeepsAction::CancelForget);
                        }
                        if widgets::button(ui, right, &format!("keep-sure-{i}"), &Btn::primary("Forget").size(15.0)).clicked() {
                            acts.push(KeepsAction::Forget(keep.host_key.clone()));
                        }
                    } else {
                        let sub = format!("last played {} \u{b7} {}", keeps::ago(keep.last_joined, now), keeps::keep_id(&keep.host_key));
                        p.text(row.left_top() + vec2(16.0, 40.0), Align2::LEFT_CENTER, sub, theme::ui_medium(11.5), theme::TEXT_DIM);
                        if widgets::button(ui, left, &format!("keep-forget-{i}"), &Btn::ghost("Forget").size(15.0)).clicked() {
                            acts.push(KeepsAction::AskForget(keep.host_key.clone()));
                        }
                        if widgets::button(ui, right, &format!("keep-join-{i}"), &Btn::primary("Join").size(16.0)).clicked() {
                            acts.push(KeepsAction::Join(keep.host_key.clone()));
                        }
                    }
                }
            });
        });
        // Acted on in `update`, so a tap that pushes a screen is applied once and in order.
        self.acts.extend(acts);
        out
    }
}
