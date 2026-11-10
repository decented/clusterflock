//! The wardrobe: the player's bird in 3D, the six colours (free) and the hats
//! (bought with the wallet). Opened from Home by tapping the wallet or the bird.

use super::home;
use super::online_common;
use super::{Ctx, Screen, Transition};
use crate::render::ScenePass;
use crate::ui::art::{self, fade, rgba};
use crate::ui::game::stage::{self, Draw};
use crate::ui::widgets::{self, S2, S3, S4};
use crate::ui::{feedback, theme};
use egui::{pos2, vec2, Align2, Color32, Rect, Sense, Stroke};
use flock_render::{FrameScene, PlayerStyle, HATS};

/// The hats in the order the grid shows them: free first, then by price.
pub const ORDER: [u8; 9] = [0, 2, 6, 7, 3, 4, 5, 1, 8];

/// What a hat costs, in coins. The most ordinary hats are cheap, the crown is
/// 2000 and the chain shades 3000. Hat 0 ("no hat") is free to everybody, and so
/// is the hat the player's colour wore at first launch (it is owned from the start).
pub fn price(hat: u8) -> u32 {
    match hat {
        0 => 0,
        2 => 150,  // cap
        6 => 300,  // scarf
        7 => 500,  // goggles
        3 => 800,  // viking helmet
        4 => 1200, // mohawk
        5 => 1600, // star headband
        1 => 2000, // crown
        8 => 3000, // chain shades
        _ => 3000,
    }
}

/// The name of a hat id.
pub fn hat_name(id: u8) -> &'static str {
    HATS.iter().find(|h| h.id == id).map_or("Hat", |h| h.name)
}

pub struct WardrobeScreen {
    t: f32,
    scene: FrameScene,
    quality: crate::platform::data_dir::Quality,
    /// `--shot` staging: fake a profile with some hats on the first frame.
    sample: bool,
}

impl Default for WardrobeScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl WardrobeScreen {
    pub fn new() -> WardrobeScreen {
        WardrobeScreen { t: 0.0, scene: FrameScene::default(), quality: Default::default(), sample: false }
    }

    /// Staged for `--shot`: a few hats owned, one worn, 640 coins.
    pub fn sample() -> WardrobeScreen {
        let mut s = WardrobeScreen::new();
        s.sample = true;
        s
    }
}

/// The preview: the bird close up, turning slowly.
pub fn preview_scene(t: f32, aspect: f32, colour: u8, hat: u8) -> FrameScene {
    let mut sc = home::idle_scene(t, aspect, colour, hat);
    sc.farm_props.clear();
    sc.coins.clear();
    let z0 = 30.0;
    if let Some(b) = sc.ostriches.first_mut() {
        b.yaw = std::f32::consts::PI + 0.55 * (t * 0.55).sin();
    }
    sc.camera.eye = glam::Vec3::new(0.8, 2.5, z0 - 10.5);
    sc.camera.target = glam::Vec3::new(0.0, 0.85, z0);
    sc
}

/// The panel and its rows, from the bottom up (pure, for tests): (panel, colour row, grid cells in `ORDER`).
pub fn layout(safe: Rect) -> (Rect, Rect, [Rect; 9]) {
    let w = (safe.width() - 2.0 * S4).min(420.0);
    let x = safe.center().x - w / 2.0;
    let gap = 8.0;
    let pad = S3;
    let header = 60.0;
    let colour_h = 62.0;
    let min_bird = 150.0;
    let avail = safe.height() - header - colour_h - 2.0 * pad - min_bird - 2.0 * gap - S4;
    let cell_h = (avail / 3.0).clamp(48.0, 76.0);
    let grid_h = 3.0 * cell_h + 2.0 * gap;
    let panel_h = pad + colour_h + grid_h + pad;
    let panel = Rect::from_min_size(pos2(x, safe.bottom() - S4 - panel_h), vec2(w, panel_h));
    let colours = Rect::from_min_size(panel.min + vec2(pad, pad), vec2(w - 2.0 * pad, colour_h));
    let cell_w = (w - 2.0 * pad - 2.0 * gap) / 3.0;
    let mut cells = [Rect::NOTHING; 9];
    for (i, c) in cells.iter_mut().enumerate() {
        let (col, row) = ((i % 3) as f32, (i / 3) as f32);
        *c = Rect::from_min_size(pos2(panel.left() + pad + col * (cell_w + gap), colours.bottom() + row * (cell_h + gap)), vec2(cell_w, cell_h));
    }
    (panel, colours, cells)
}

impl Screen for WardrobeScreen {
    fn name(&self) -> &'static str {
        "wardrobe"
    }

    fn back(&mut self, ctx: &mut Ctx) -> Transition {
        feedback::back(ctx);
        Transition::Pop
    }

    fn clear_color(&self, _ctx: &Ctx) -> [f32; 4] {
        [0.55, 0.75, 0.95, 1.0]
    }

    fn update(&mut self, ctx: &mut Ctx) -> Transition {
        super::common::begin(ctx, &mut self.t);
        if self.sample {
            self.sample = false;
            let p = &mut *ctx.profile;
            p.name_seed = 2;
            p.colour = Some(2);
            p.wardrobe_ready = true;
            p.owned_hats = vec![0, 3, 2, 6];
            p.hat = 2;
            p.wallet = 640;
        }
        crate::online::set_local_hat(ctx.profile.hat);
        self.quality = ctx.profile.quality;
        let s = ctx.viewport.screen;
        let (colour, hat) = ctx.profile.look();
        self.scene = preview_scene(self.t, s.width() / s.height().max(1.0), colour, hat);
        Transition::None
    }

    fn paint_scene(&mut self, pass: &mut ScenePass) {
        stage::paint(pass, Draw { scene: &self.scene, dt: 1.0 / 60.0, quality: self.quality, shift_z: 0.0, replay: &[] });
    }

    fn draw(&mut self, ctx: &mut Ctx, ui: &egui::Context) -> Transition {
        widgets::set_over_3d(true);
        let safe = ctx.viewport.safe;
        let screen = ctx.viewport.screen;
        let (t, tm) = (self.t, ctx.time as f32);
        let mut out = Transition::None;
        let (colour, hat) = ctx.profile.look();
        let wallet = ctx.profile.wallet;
        let brand = art::brand(ui);
        let (panel, colour_row, cells) = layout(safe);
        let veil = ui.layer_painter(egui::LayerId::background());
        art::vgradient(&veil, Rect::from_min_max(screen.min, pos2(screen.right(), safe.top() + 120.0)), &[(0.0, rgba(20, 28, 70, 0.55)), (1.0, rgba(20, 28, 70, 0.0))]);
        art::vgradient(&veil, Rect::from_min_max(pos2(screen.left(), panel.top() - 40.0), screen.max), &[(0.0, rgba(18, 10, 31, 0.0)), (0.3, rgba(18, 10, 31, 0.8)), (1.0, rgba(18, 10, 31, 0.95))]);
        #[allow(deprecated)]
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let hd = widgets::header(ui, safe, "Wardrobe", widgets::enter(t, 0));
            if hd.back.clicked() {
                feedback::back(ctx);
                out = Transition::Pop;
            }
            let p = ui.painter().clone();
            // the wallet, top right
            let wc = pos2(safe.right() - S4 - 4.0, safe.top() + S2 + 2.0 + 22.0);
            let label = crate::daily::thousands(wallet.min(u32::MAX as u64) as u32);
            let g = p.layout_no_wrap(label.clone(), theme::ui_bold(15.0), theme::SAND);
            let pw = g.size().x + 44.0;
            let pill = Rect::from_center_size(wc - vec2(pw / 2.0, 0.0), vec2(pw, 30.0));
            p.rect_filled(pill, 15.0, rgba(0x14, 0x1E, 0x46, 0.75));
            art::coin(&p, &brand, pill.left_center() + vec2(17.0, 0.0), 22.0, None, 1.0);
            p.text(pos2(pill.left() + 32.0, pill.center().y), Align2::LEFT_CENTER, label, theme::ui_bold(15.0), theme::SAND);

            widgets::staged(ui, widgets::enter(t, 1), vec2(0.0, 30.0), |ui, off| {
                let p = ui.painter().clone();
                widgets::panel(&p, panel.translate(off), 22.0);
                // ---- colours
                let cr = colour_row.translate(off);
                p.text(pos2(cr.left(), cr.top() + 8.0), Align2::LEFT_CENTER, "COLOUR", theme::ui_bold(11.0), theme::TEXT_DIM);
                p.text(pos2(cr.right(), cr.top() + 8.0), Align2::RIGHT_CENTER, online_common::colour_name(colour), theme::ui_bold(11.0), theme::TEXT);
                let step = cr.width() / 6.0;
                let r = (step * 0.36).min(19.0);
                for c in 0..6u8 {
                    let centre = pos2(cr.left() + step * (c as f32 + 0.5), cr.top() + 18.0 + (cr.height() - 18.0) / 2.0);
                    let hit = Rect::from_center_size(centre, vec2(step, cr.height() - 14.0));
                    let resp = ui.interact(hit, egui::Id::new(("wardrobe-colour", c)), Sense::click());
                    let mine = c == colour;
                    if mine {
                        art::glow(&p, centre, r * 1.9, rgba(0xFF, 0xC8, 0x3D, 0.30));
                    }
                    p.circle_filled(centre + vec2(0.0, 2.0), r, rgba(0, 0, 0, 0.35));
                    p.circle_filled(centre, r, art::from_f3(PlayerStyle::of(c).body));
                    p.circle_stroke(centre, r, Stroke::new(if mine { 3.0_f32 } else { 1.5_f32 }, if mine { theme::GOLD } else { rgba(255, 255, 255, 0.5) }));
                    if resp.clicked() && !mine {
                        feedback::select(ctx);
                        ctx.profile.colour = Some(c);
                        ctx.save_profile();
                    }
                }
                // ---- hats
                for (i, id) in ORDER.iter().enumerate() {
                    let rect = cells[i].translate(off);
                    let owned = ctx.profile.owns_hat(*id);
                    let worn = hat == *id;
                    let cost = price(*id);
                    let affordable = owned || wallet >= cost as u64;
                    let resp = ui.interact(rect, egui::Id::new(("wardrobe-hat", *id)), Sense::click());
                    let pressed = resp.is_pointer_button_down_on();
                    widgets::well(&p, if pressed { rect.shrink(1.5) } else { rect }, 14.0);
                    if worn {
                        p.rect_stroke(rect, 14.0, Stroke::new(2.5_f32, theme::GOLD), egui::StrokeKind::Inside);
                    }
                    let dim = if affordable { 1.0 } else { 0.55 };
                    let rr = (rect.height() * 0.30).min(20.0);
                    let chip = pos2(rect.left() + 8.0 + rr, rect.center().y);
                    online_common::bird_chip_hat(&p, chip, rr, colour, *id, dim);
                    let tx = chip.x + rr + 6.0;
                    let avail = rect.right() - 6.0 - tx;
                    let name = hat_name(*id);
                    let ns = art::fit_size(&p, name, theme::ui_bold, 12.0, avail);
                    p.text(pos2(tx, rect.center().y - 8.0), Align2::LEFT_CENTER, name, theme::ui_bold(ns), fade(theme::TEXT, dim));
                    let sub_y = rect.center().y + 9.0;
                    if worn {
                        p.text(pos2(tx, sub_y), Align2::LEFT_CENTER, "WORN", theme::ui_bold(11.0), theme::GOLD);
                    } else if owned {
                        p.text(pos2(tx, sub_y), Align2::LEFT_CENTER, "OWNED", theme::ui_bold(11.0), theme::MINT);
                    } else {
                        art::coin(&p, &brand, pos2(tx + 6.0, sub_y), 13.0, None, dim);
                        p.text(pos2(tx + 15.0, sub_y), Align2::LEFT_CENTER, crate::daily::thousands(cost), theme::ui_bold(12.0), fade(theme::SAND, dim));
                    }
                    if resp.clicked() {
                        if owned {
                            if !worn {
                                feedback::select(ctx);
                                ctx.profile.hat = *id;
                                ctx.save_profile();
                            }
                        } else {
                            match ctx.profile.buy_hat(*id, cost) {
                                Ok(()) => {
                                    feedback::bought(ctx);
                                    ctx.save_profile();
                                    ctx.say(format!("{} is yours", hat_name(*id)));
                                }
                                Err(crate::platform::data_dir::BuyError::Short(n)) => {
                                    feedback::tap(ctx);
                                    ctx.say(format!("Not enough coins \u{2014} {n} to go"));
                                }
                            }
                        }
                    }
                }
            });
            let _ = (Color32::WHITE, tm);
        });
        out
    }
}

impl Drop for WardrobeScreen {
    fn drop(&mut self) {
        widgets::set_over_3d(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::viewport::{Insets, Viewport};

    #[test]
    fn every_hat_is_in_the_grid_once_with_a_price() {
        let mut ids: Vec<u8> = ORDER.to_vec();
        ids.sort();
        let mut all: Vec<u8> = HATS.iter().map(|h| h.id).collect();
        all.sort();
        assert_eq!(ids, all);
        assert_eq!(price(0), 0);
        let prices: Vec<u32> = ORDER.iter().map(|h| price(*h)).collect();
        assert!(prices.windows(2).all(|w| w[0] < w[1]), "ascending: {prices:?}");
        assert_eq!(price(1), 2000, "crown");
        assert_eq!(price(8), 3000, "chain shades");
        // The seven listed prices are all there.
        for p in [150, 300, 500, 800, 1200, 2000, 3000] {
            assert!(prices.contains(&p), "{p}");
        }
    }

    #[test]
    fn a_new_player_can_afford_nothing_but_owns_two_hats() {
        let mut p = crate::platform::data_dir::Profile { name_seed: 4, ..Default::default() };
        p.ensure_wardrobe();
        let owned: Vec<u8> = ORDER.iter().copied().filter(|h| p.owns_hat(*h)).collect();
        assert_eq!(owned.len(), 2, "no hat and the first-launch colour's hat: {owned:?}");
    }

    #[test]
    fn layout_fits_phones() {
        for (w, h) in [(320.0, 480.0), (360.0, 640.0), (360.0, 800.0), (412.0, 915.0)] {
            let s = Viewport::new(vec2(w, h), 2.0, Insets { top: 24.0, bottom: 16.0, left: 0.0, right: 0.0 }).safe;
            let (panel, colours, cells) = layout(s);
            assert!(s.contains_rect(panel), "{panel:?} in {s:?} at {w}x{h}");
            assert!(panel.contains_rect(colours));
            for c in cells {
                assert!(panel.contains_rect(c), "{c:?}");
                assert!(c.height() >= 48.0 && c.width() >= 80.0, "{c:?}");
            }
            assert!(colours.bottom() <= cells[0].top());
            // room for the bird above the panel
            assert!(panel.top() - (s.top() + 60.0) >= 100.0, "bird room {} at {w}x{h}", panel.top() - s.top() - 60.0);
        }
    }
}
