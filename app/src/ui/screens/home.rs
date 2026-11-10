//! Title and main menu in one: the Cluster Flock wordmark over the 3D farm
//! with a Nostrich idling on the track, and the menu buttons below it.

use super::common::{self, wordmark};
use super::practice::PracticeScreen;
use super::run::RunScreen;
use super::wardrobe::WardrobeScreen;
use super::{Ctx, Screen, Transition};
use crate::daily::DailyService;
use crate::ui::game::stage::{self, Draw};
use crate::render::ScenePass;
use crate::ui::art::{ease_out_back, ease_out_cubic, fade, phase, rgba};
use crate::ui::widgets::{self, Btn, S3, S4};
use crate::ui::{feedback, theme};
use egui::{pos2, vec2, Align2, Color32, Rect, Stroke};
use flock_render::{Accessory, Camera, CoinInst, FarmPropInst, FarmPropKind, FrameScene, OstrichPose};
use glam::{Vec2, Vec3};

pub struct HomeScreen {
    t: f32,
    scene: FrameScene,
    quality: crate::platform::data_dir::Quality,
    colour: u8,
    hat: u8,
    /// The fetch of today's Bitcoin block (see `crate::daily`).
    daily: DailyService,
    /// `--shot` staging: fake a cached daily, a wallet and a best on the first frame.
    sample: bool,
}

impl Default for HomeScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl HomeScreen {
    pub fn new() -> HomeScreen {
        HomeScreen { t: 0.0, scene: FrameScene::default(), quality: Default::default(), colour: 0, hat: flock_render::HAT_DEFAULT, daily: DailyService::new(), sample: false }
    }

    /// Staged for `--shot`: today's block cached, a best on it and coins in the wallet.
    pub fn sample_daily() -> HomeScreen {
        let mut s = HomeScreen::new();
        s.sample = true;
        s
    }
}

/// Button heights, top to bottom (Play solo, Daily run, Practice, Friends, the
/// Settings / More games row) and the gap between them, shrunk to fit `space`.
pub fn button_heights(space: f32) -> ([f32; 5], f32) {
    let base = [60.0, 54.0, 48.0, 48.0, 42.0];
    let gap = S3 - 2.0;
    let total: f32 = base.iter().sum::<f32>() + gap * 4.0;
    let k = (space / total).min(1.0);
    let mut h = base;
    for x in &mut h {
        *x *= k;
    }
    (h, gap * k.max(0.8))
}

/// The idle scene: a bird on the track facing the lens, a bale and a few coins.
pub fn idle_scene(t: f32, aspect: f32, colour: u8, hat: u8) -> FrameScene {
    let z0 = 30.0;
    let mut sc = FrameScene { time: t, ..Default::default() };
    let mut b = OstrichPose::for_player_hat(colour, hat);
    b.pos = Vec3::new(0.0, 0.0, z0);
    b.yaw = std::f32::consts::PI + 0.18 * (t * 0.45).sin();
    // a slow look around and an occasional flap
    b.neck_sway = Vec2::new(0.35 * (t * 0.9).sin(), 0.1 + 0.1 * (t * 0.7).sin());
    b.body_roll = 0.03 * (t * 0.9).sin();
    b.squash = (0.5 + 0.5 * (t * 2.2).sin()).powi(8) * 0.25;
    let flap = ((t * 0.31).fract() < 0.07) as u8 as f32;
    b.wing_flap = flap;
    b.outline = None;
    sc.ostriches.push(b);
    let _ = Accessory::None;
    sc.farm_props.push(FarmPropInst::new(FarmPropKind::HayBale, Vec3::new(2.2, 0.0, z0 + 5.5), Vec3::new(1.6, 0.7, 1.0)));
    for k in 0..5 {
        let a = k as f32 * 0.5;
        sc.coins.push(CoinInst { pos: Vec3::new(-2.2 + 0.1 * k as f32, 0.8 + 0.5 * (a).sin(), z0 + 3.0 + k as f32 * 1.1), spin: t * 3.0 + k as f32, visible: true });
    }
    let eye = Vec3::new(1.9, 2.6, z0 - 14.0);
    let target = Vec3::new(0.1, 2.1, z0);
    sc.camera = Camera { eye, target, up: Vec3::Y, fov_y: (41.0f32 + (0.45 - aspect).max(0.0) * 30.0).to_radians(), near: 0.4, far: 230.0 };
    sc.fog_scale = 0.8;
    sc
}

impl Screen for HomeScreen {
    fn name(&self) -> &'static str {
        "home"
    }

    fn on_background(&mut self, _ctx: &mut Ctx) {
        feedback::on_background();
    }

    fn clear_color(&self, _ctx: &Ctx) -> [f32; 4] {
        [0.55, 0.75, 0.95, 1.0]
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        common::begin(ctx, &mut self.t);
        if self.sample {
            self.sample = false;
            let p = &mut *ctx.profile;
            p.name_seed = 5;
            p.best_distance_m = 1286;
            p.best_coins = 47;
            p.wallet = 1250;
            p.wardrobe_ready = true;
            p.colour = Some(1);
            p.owned_hats = vec![0, 2, 8];
            p.hat = 8;
            let now = crate::daily::now_unix();
            p.daily = Some(crate::platform::data_dir::DailyCache { day: 20766, height: 921_344, hash_hex: "00".repeat(32), seed: 0x4b9c_c6b9_ba92_b55f, fetched_unix: now });
            p.daily_best = Some(crate::platform::data_dir::DailyBest { height: 921_344, metres: 512, coins: 21, attempts: 3 });
        }
        // today's block: no network in --shot, none while a run is on top (Home is not updated then)
        if self.daily.poll(ctx.profile, ctx.opts.shot.is_none()) {
            ctx.save_profile();
        }
        crate::online::set_local_hat(ctx.profile.hat);
        self.quality = ctx.profile.quality;
        (self.colour, self.hat) = ctx.profile.look();
        let s = ctx.viewport.screen;
        self.scene = idle_scene(self.t + ctx.time as f32 * 0.0, s.width() / s.height().max(1.0), self.colour, self.hat);
        Transition::None
    }

    fn paint_scene(&mut self, pass: &mut ScenePass) {
        stage::paint(pass, Draw { scene: &self.scene, dt: 1.0 / 60.0, quality: self.quality, shift_z: 0.0, replay: &[] });
    }

    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        widgets::set_over_3d(true);
        let safe = ctx.viewport.safe;
        let screen = ctx.viewport.screen;
        let t = self.t;
        let tm = ctx.time as f32;
        let mut out = Transition::None;
        let (best_m, best_coins) = (ctx.profile.best_distance_m, ctx.profile.best_coins);
        let name = ctx.profile.name.clone();
        let wallet = ctx.profile.wallet;
        let line = self.daily.line(ctx.profile);
        let daily_play = ctx.profile.daily.as_ref().map(|d| (d.seed, d.height));
        let look = ctx.profile.look();
        let brand = crate::ui::art::brand(ui);
        let p = ui.layer_painter(egui::LayerId::background());
        // legibility veils: the sky behind the title, the ground behind the buttons
        let top_v = Rect::from_min_max(screen.min, pos2(screen.right(), safe.top() + safe.height() * 0.34));
        crate::ui::art::vgradient(&p, top_v, &[(0.0, rgba(20, 28, 70, 0.55)), (1.0, rgba(20, 28, 70, 0.0))]);
        let low = Rect::from_min_max(pos2(screen.left(), safe.top() + safe.height() * 0.50), screen.max);
        crate::ui::art::vgradient(&p, low, &[(0.0, rgba(18, 10, 31, 0.0)), (0.35, rgba(18, 10, 31, 0.78)), (1.0, rgba(18, 10, 31, 0.94))]);
        #[allow(deprecated)]
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let p = ui.painter().clone();
            // wordmark: drops in, then floats
            let size = (safe.width() * 0.22).min(safe.height() * 0.105);
            let e = ease_out_back(phase(t, 0.10, 0.55));
            let bob = (tm * 1.6).sin() * 2.0;
            // below the name line and the wallet pill (60 pt), whatever the screen height
            let centre = pos2(safe.center().x, (safe.top() + safe.height() * 0.16 + 14.0).max(safe.top() + 60.0 + size + 10.0) + bob);
            let rect = wordmark(&p, centre - vec2(0.0, (1.0 - e) * 60.0), size * (0.8 + 0.2 * e), phase(t, 0.1, 0.25));
            p.text(pos2(centre.x, rect.bottom() + size * 0.16), Align2::CENTER_CENTER, "stick together, fly far", theme::ui_bold(14.0), fade(Color32::WHITE, phase(t, 0.55, 0.45)));

            // who is playing, and the best run so far
            let line_txt = if best_m > 0 || best_coins > 0 { format!("{name}  \u{b7}  best {best_m} m  \u{b7}  {best_coins} coins") } else { name };
            widgets::pill(&p, pos2(safe.center().x, safe.top() + 18.0), &line_txt, rgba(0x14, 0x1E, 0x46, 0.7), theme::TEXT, 12.0);

            // the wallet: tap it (or the bird) for the wardrobe
            let wtxt = crate::daily::thousands(wallet.min(u32::MAX as u64) as u32);
            let wg = p.layout_no_wrap(wtxt.clone(), theme::ui_bold(14.0), theme::SAND);
            let wr = Rect::from_center_size(pos2(safe.center().x, safe.top() + 18.0 + 28.0), vec2(wg.size().x + 48.0, 28.0));
            let wresp = ui.interact(wr.expand2(vec2(6.0, 8.0)), egui::Id::new("home-wallet"), egui::Sense::click());
            p.rect_filled(wr, 14.0, rgba(0x14, 0x1E, 0x46, if wresp.is_pointer_button_down_on() { 0.95 } else { 0.7 }));
            p.rect_stroke(wr, 14.0, Stroke::new(1.2_f32, rgba(0xE8, 0xC6, 0x8A, 0.45)), egui::StrokeKind::Inside);
            crate::ui::art::coin(&p, &brand, wr.left_center() + vec2(16.0, 0.0), 20.0, None, 1.0);
            p.text(pos2(wr.left() + 30.0, wr.center().y), Align2::LEFT_CENTER, wtxt, theme::ui_bold(14.0), theme::SAND);
            p.text(pos2(wr.right() - 10.0, wr.center().y), Align2::CENTER_CENTER, "\u{203a}", theme::ui_bold(16.0), theme::TEXT_DIM);
            if wresp.clicked() {
                feedback::select(ctx);
                out = Transition::Push(Box::new(WardrobeScreen::new()));
            }

            // the buttons, stacked in the lower part
            let w = (safe.width() - 2.0 * S4 * 2.0).min(320.0);
            let bottom = safe.bottom() - 30.0;
            let (heights, gap) = button_heights(bottom - (safe.top() + safe.height() * 0.34));
            let total: f32 = heights.iter().sum::<f32>() + gap * 4.0;
            let top = bottom - total;
            // tap the bird for the wardrobe
            let bird = Rect::from_min_max(pos2(safe.center().x - safe.width() * 0.26, safe.top() + safe.height() * 0.38), pos2(safe.center().x + safe.width() * 0.26, top - 4.0));
            if bird.height() > 30.0 && ui.interact(bird, egui::Id::new("home-bird"), egui::Sense::click()).clicked() {
                feedback::select(ctx);
                out = Transition::Push(Box::new(WardrobeScreen::new()));
            }
            let k = ease_out_cubic(phase(t, 0.45, 0.45));
            widgets::staged(ui, k, vec2(0.0, 30.0), |ui, off| {
                let mut y = top;
                let mut at = |i: usize| {
                    let r = Rect::from_min_size(pos2(safe.center().x - w / 2.0, y), vec2(w, heights[i])).translate(off);
                    y += heights[i] + gap;
                    r
                };
                if widgets::button(ui, at(0), "home-solo", &Btn::primary("Play solo").size(24.0)).clicked() {
                    feedback::select(ctx);
                    out = Transition::Push(Box::new(RunScreen::solo_as(look)));
                }
                let sub = line.text();
                let dbtn = Btn::new("Daily run").sub(&sub).size(20.0).enabled(line.playable());
                if widgets::button(ui, at(1), "home-daily", &dbtn).clicked() {
                    if let (true, Some((seed, height))) = (line.playable(), daily_play) {
                        feedback::select(ctx);
                        out = Transition::Push(Box::new(RunScreen::daily(seed, height, look)));
                    }
                }
                if widgets::button(ui, at(2), "home-practice", &Btn::new("Flock practice").size(20.0)).clicked() {
                    feedback::select(ctx);
                    out = Transition::Push(Box::new(PracticeScreen::new()));
                }
                if widgets::button(ui, at(3), "home-friends", &Btn::new("Play with friends").size(20.0)).clicked() {
                    feedback::select(ctx);
                    out = Transition::Push(Box::new(super::multiplayer::MultiplayerScreen::new()));
                }
                // the last row: Settings and More games side by side
                let last = at(4);
                let half = (last.width() - gap) / 2.0;
                let (left, right) = (Rect::from_min_size(last.min, vec2(half, last.height())), Rect::from_min_size(pos2(last.min.x + half + gap, last.min.y), vec2(half, last.height())));
                if widgets::button(ui, left, "home-settings", &Btn::ghost("Settings").size(17.0)).clicked() {
                    feedback::select(ctx);
                    out = Transition::Push(Box::new(super::settings::SettingsScreen::new()));
                }
                if widgets::button(ui, right, "home-more-games", &Btn::ghost("More games").size(17.0)).clicked() {
                    feedback::select(ctx);
                    out = Transition::Push(Box::new(super::games::GamesScreen::new()));
                }
            });

            // build and credit line
            p.text(pos2(safe.center().x, safe.bottom() - 8.0), Align2::CENTER_BOTTOM, format!("a game by decented  {}", crate::version_label()), theme::ui_medium(10.0), fade(Color32::WHITE, 0.55));
        });
        out
    }
}

impl Drop for HomeScreen {
    fn drop(&mut self) {
        widgets::set_over_3d(false);
    }
}
