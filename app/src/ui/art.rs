//! Brand art and painted scenery: the four brand textures, gradients, the
//! sunny-farm backdrop every menu sits on, chunky outlined display text and
//! the gradient wordmark. Everything is a pure function of time, so a `--shot`
//! frame is exactly what a phone shows at that moment.

use crate::ui::theme;
use egui::epaint::{Mesh, Vertex};
use egui::{pos2, vec2, Align2, Color32, ColorImage, Context, FontId, Id, Painter, Pos2, Rect, Shape, Stroke, TextureHandle, TextureId, TextureOptions};
use std::sync::Arc;

// ── brand textures ──────────────────────────────────────────────────────────

static DUST_PUFF: &[u8] = include_bytes!("../../assets/brand/dust_puff.png");
static BTC_COIN: &[u8] = include_bytes!("../../assets/brand/btc_coin.png");

/// The brand images as egui textures (loaded once per egui context).
pub struct Brand {
    /// Soft dust cloud (tint it), 512x512.
    pub dust: TextureHandle,
    /// Bitcoin coin, 256x256.
    pub coin: TextureHandle,
}

fn decode(bytes: &[u8]) -> ColorImage {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::normalize_to_color8());
    let blank = || ColorImage::filled([4, 4], Color32::TRANSPARENT);
    let Ok(mut r) = dec.read_info() else { return blank() };
    let Some(n) = r.output_buffer_size() else { return blank() };
    let mut buf = vec![0u8; n];
    let Ok(info) = r.next_frame(&mut buf) else { return blank() };
    let (w, h) = (info.width as usize, info.height as usize);
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buf[..w * h * 4].to_vec(),
        png::ColorType::Rgb => buf[..w * h * 3].chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => buf[..w * h * 2].chunks_exact(2).flat_map(|c| [c[0], c[0], c[0], c[1]]).collect(),
        _ => buf[..w * h].iter().flat_map(|&g| [g, g, g, 255]).collect(),
    };
    ColorImage::from_rgba_unmultiplied([w, h], &rgba)
}

/// A game's icon from the catalog (a PNG bundled in the app) as a texture, loaded once per key.
pub fn game_icon(ctx: &Context, key: &str, png: &'static [u8]) -> TextureHandle {
    let id = Id::new(("flock-game-icon", key));
    if let Some(t) = ctx.data(|d| d.get_temp::<TextureHandle>(id)) {
        return t;
    }
    let t = ctx.load_texture(format!("game-icon-{key}"), decode(png), TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    t
}

/// The brand textures, loading them on first use. Cheap afterwards.
pub fn brand(ctx: &Context) -> Arc<Brand> {
    let id = Id::new("flock-brand-textures");
    if let Some(b) = ctx.data(|d| d.get_temp::<Arc<Brand>>(id)) {
        return b;
    }
    let opts = TextureOptions::LINEAR;
    let b = Arc::new(Brand {
        dust: ctx.load_texture("brand-dust", decode(DUST_PUFF), opts),
        coin: ctx.load_texture("brand-coin", decode(BTC_COIN), opts),
    });
    ctx.data_mut(|d| d.insert_temp(id, b.clone()));
    b
}

// ── small helpers ───────────────────────────────────────────────────────────

/// 0..1 hash of an integer (stable, no state).
pub fn hash01(n: u32) -> f32 {
    let mut x = n.wrapping_mul(0x9E37_79B1) ^ 0x85EB_CA6B;
    x ^= x >> 15;
    x = x.wrapping_mul(0x2C1B_3C6D);
    x ^= x >> 12;
    x = x.wrapping_mul(0x297A_2D39);
    x ^= x >> 15;
    (x & 0xFFFF) as f32 / 65535.0
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = 1.0 - t.clamp(0.0, 1.0);
    1.0 - t * t * t
}

pub fn ease_out_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0) - 1.0;
    1.0 + 2.70158 * t * t * t + 1.70158 * t * t
}

/// Progress of an animation that starts at `delay` and runs `dur` seconds.
pub fn phase(t: f32, delay: f32, dur: f32) -> f32 {
    ((t - delay) / dur).clamp(0.0, 1.0)
}

/// A colour with its alpha scaled (premultiplied-safe).
pub fn fade(c: Color32, a: f32) -> Color32 {
    c.gamma_multiply(a.clamp(0.0, 1.0))
}

pub fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Opaque colour of a 0..1 float triple (race-core palettes).
pub fn from_f3(c: [f32; 3]) -> Color32 {
    Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8)
}

fn push_quad(m: &mut Mesh, corners: [Pos2; 4], uvs: [Pos2; 4], cols: [Color32; 4]) {
    let i = m.vertices.len() as u32;
    for k in 0..4 {
        m.vertices.push(Vertex { pos: corners[k], uv: uvs[k], color: cols[k] });
    }
    m.indices.extend([i, i + 1, i + 2, i, i + 2, i + 3]);
}

/// A vertical multi-stop gradient filling `r`. Stops are `(0..1 down the rect, colour)`.
pub fn vgradient(p: &Painter, r: Rect, stops: &[(f32, Color32)]) {
    if stops.len() < 2 || r.height() <= 0.0 {
        return;
    }
    let mut m = Mesh::default();
    let uv = egui::epaint::WHITE_UV;
    for w in stops.windows(2) {
        let (y0, y1) = (r.top() + w[0].0 * r.height(), r.top() + w[1].0 * r.height());
        push_quad(&mut m, [pos2(r.left(), y0), pos2(r.right(), y0), pos2(r.right(), y1), pos2(r.left(), y1)], [pos2(uv.x, uv.y); 4], [w[0].1, w[0].1, w[1].1, w[1].1]);
    }
    p.add(Shape::mesh(m));
}

/// A horizontal gradient filling `r`.
pub fn hgradient(p: &Painter, r: Rect, left: Color32, right: Color32) {
    let mut m = Mesh::default();
    let uv = egui::epaint::WHITE_UV;
    push_quad(&mut m, [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom()], [pos2(uv.x, uv.y); 4], [left, right, right, left]);
    p.add(Shape::mesh(m));
}

/// A soft round glow: `color` at the centre fading to nothing at `radius`.
pub fn glow(p: &Painter, c: Pos2, radius: f32, color: Color32) {
    let mut m = Mesh::default();
    let uv = egui::epaint::WHITE_UV;
    m.vertices.push(Vertex { pos: c, uv, color });
    let n = 40;
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        m.vertices.push(Vertex { pos: c + vec2(a.cos(), a.sin()) * radius, uv, color: Color32::TRANSPARENT });
    }
    for i in 0..n as u32 {
        m.indices.extend([0, 1 + i, 1 + (i + 1) % n as u32]);
    }
    p.add(Shape::mesh(m));
}

/// A glow squashed to an ellipse (horizon haze).
pub fn glow_ellipse(p: &Painter, c: Pos2, rx: f32, ry: f32, color: Color32) {
    let mut m = Mesh::default();
    let uv = egui::epaint::WHITE_UV;
    m.vertices.push(Vertex { pos: c, uv, color });
    let n = 40;
    for i in 0..n {
        let a = i as f32 / n as f32 * std::f32::consts::TAU;
        m.vertices.push(Vertex { pos: c + vec2(a.cos() * rx, a.sin() * ry), uv, color: Color32::TRANSPARENT });
    }
    for i in 0..n as u32 {
        m.indices.extend([0, 1 + i, 1 + (i + 1) % n as u32]);
    }
    p.add(Shape::mesh(m));
}

/// A textured quad centred on `c`, rotated by `angle` radians, optionally mirrored.
pub fn sprite(p: &Painter, tex: TextureId, c: Pos2, size: egui::Vec2, angle: f32, flip_x: bool, tint: Color32) {
    let (s, co) = angle.sin_cos();
    let rot = |v: egui::Vec2| vec2(v.x * co - v.y * s, v.x * s + v.y * co);
    let h = size / 2.0;
    let corners = [c + rot(vec2(-h.x, -h.y)), c + rot(vec2(h.x, -h.y)), c + rot(vec2(h.x, h.y)), c + rot(vec2(-h.x, h.y))];
    let (u0, u1) = if flip_x { (1.0, 0.0) } else { (0.0, 1.0) };
    let uvs = [pos2(u0, 0.0), pos2(u1, 0.0), pos2(u1, 1.0), pos2(u0, 1.0)];
    let mut m = Mesh::with_texture(tex);
    push_quad(&mut m, corners, uvs, [tint; 4]);
    p.add(Shape::mesh(m));
}

/// A bitcoin coin of height `h` (a spinning coin when `spin` is 0..1).
pub fn coin(p: &Painter, b: &Brand, c: Pos2, h: f32, spin: Option<f32>, alpha: f32) {
    let w = match spin {
        Some(s) => (s * std::f32::consts::TAU).cos().abs().max(0.12) * h,
        None => h,
    };
    sprite(p, b.coin.id(), c, vec2(w, h), 0.0, false, fade(Color32::WHITE, alpha));
}

// ── text treatments ─────────────────────────────────────────────────────────

/// Text with a coloured outline and an optional soft drop shadow, centred (or
/// aligned) on `pos`. Returns the text rect.
#[allow(clippy::too_many_arguments)]
pub fn outlined(p: &Painter, pos: Pos2, anchor: Align2, text: &str, font: FontId, fill: Color32, outline: Color32, width: f32, shadow: f32) -> Rect {
    let galley = p.layout_no_wrap(text.to_string(), font, Color32::WHITE);
    let rect = anchor.anchor_size(pos, galley.size());
    let a = fill.a() as f32 / 255.0;
    if shadow > 0.0 && a > 0.0 {
        let sc = rgba(0, 0, 0, 0.35 * a);
        p.galley_with_override_text_color(rect.min + vec2(0.0, shadow), galley.clone(), sc);
    }
    if width > 0.0 {
        let n = if width > 2.5 { 16 } else { 10 };
        let oc = fade(outline, a);
        for i in 0..n {
            let ang = i as f32 / n as f32 * std::f32::consts::TAU;
            p.galley_with_override_text_color(rect.min + vec2(ang.cos(), ang.sin()) * width, galley.clone(), oc);
        }
    }
    p.galley_with_override_text_color(rect.min, galley, fill);
    rect
}

/// Display text with a vertical gradient fill, a thick plum outline and a drop
/// shadow: the wordmark treatment. `pos` is the centre.
#[allow(clippy::too_many_arguments)]
pub fn gradient_text(p: &Painter, pos: Pos2, text: &str, font: FontId, top: Color32, bottom: Color32, outline: Color32, width: f32, alpha: f32) -> Rect {
    let galley = p.layout_no_wrap(text.to_string(), font, Color32::WHITE);
    let rect = Align2::CENTER_CENTER.anchor_size(pos, galley.size());
    // soft shadow
    p.galley_with_override_text_color(rect.min + vec2(0.0, width * 1.6), galley.clone(), rgba(0, 0, 0, 0.30 * alpha));
    p.galley_with_override_text_color(rect.min + vec2(0.0, width * 1.1), galley.clone(), rgba(0, 0, 0, 0.30 * alpha));
    let n = 18;
    for i in 0..n {
        let ang = i as f32 / n as f32 * std::f32::consts::TAU;
        p.galley_with_override_text_color(rect.min + vec2(ang.cos(), ang.sin()) * width, galley.clone(), fade(outline, alpha));
    }
    // gradient fill: recolour the glyph mesh by height
    let (y0, y1) = (rect.top() + galley.size().y * 0.12, rect.bottom() - galley.size().y * 0.14);
    let [tw, th] = p.fonts(|f| f.font_image_size());
    for row in &galley.rows {
        let mut m = row.row.visuals.mesh.clone();
        let off = rect.min.to_vec2() + row.pos.to_vec2();
        for v in &mut m.vertices {
            v.uv = pos2(v.uv.x / tw as f32, v.uv.y / th as f32);
            v.pos += off;
            let k = ((v.pos.y - y0) / (y1 - y0)).clamp(0.0, 1.0);
            v.color = fade(theme::mix(top, bottom, k), alpha);
        }
        p.add(Shape::mesh(m));
    }
    // a thin highlight along the top edge of the letters
    let hi = galley.clone();
    p.with_clip_rect(Rect::from_min_max(rect.min - vec2(8.0, 8.0), pos2(rect.max.x + 8.0, rect.top() + galley.size().y * 0.30)))
        .galley_with_override_text_color(rect.min + vec2(0.0, -width * 0.18), hi, rgba(255, 244, 200, 0.16 * alpha));
    rect
}

/// The largest font size (at most `size`) at which `text` fits `max_w` in `family`.
pub fn fit_size(p: &Painter, text: &str, mk: fn(f32) -> FontId, size: f32, max_w: f32) -> f32 {
    let mut s = size;
    while s > 8.0 {
        let w = p.layout_no_wrap(text.to_string(), mk(s), Color32::WHITE).size().x;
        if w <= max_w {
            break;
        }
        s -= 0.5;
    }
    s
}

// ── the backdrop ────────────────────────────────────────────────────────────

/// How the painted farm is set (a sunny morning by default).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scene {
    /// Seconds (drives the drift of clouds, birds and the windmill).
    pub t: f32,
    /// How high the sun sits: 0 = its place in the sky, negative = still down behind the hills.
    pub sun: f32,
    /// 0..1 dark veil over everything but the sky, for quiet screens.
    pub veil: f32,
    /// Shift the palette towards dusk (cooler, darker sky).
    pub night: f32,
    /// Draw the farm (false: just the sky and clouds, for modal backdrops).
    pub horizon: bool,
}

impl Scene {
    pub fn dusk(t: f32) -> Scene {
        Scene { t, sun: 0.0, veil: 0.0, night: 0.0, horizon: true }
    }
    pub fn quiet(t: f32) -> Scene {
        Scene { t, sun: 0.0, veil: 0.42, night: 0.0, horizon: true }
    }
}

const SKY_TOP: Color32 = Color32::from_rgb(0x4F, 0x9B, 0xE6);
const SKY_MID: Color32 = Color32::from_rgb(0x86, 0xC3, 0xF2);
const SKY_LOW: Color32 = Color32::from_rgb(0xC4, 0xE6, 0xFA);
const SKY_HORIZON: Color32 = Color32::from_rgb(0xEA, 0xF6, 0xEE);
const HILL_FAR: Color32 = Color32::from_rgb(0x9F, 0xCF, 0xB0);
const HILL_MID: Color32 = Color32::from_rgb(0x72, 0xBE, 0x6C);
const HILL_NEAR: Color32 = Color32::from_rgb(0x56, 0xAC, 0x4C);
const MEADOW: Color32 = Color32::from_rgb(0x4A, 0x9E, 0x45);
const BARN_RED: Color32 = Color32::from_rgb(0xC2, 0x3B, 0x2E);
const BARN_DARK: Color32 = Color32::from_rgb(0x8E, 0x26, 0x20);
const TRIM: Color32 = Color32::from_rgb(0xFA, 0xF4, 0xE4);
const FENCE: Color32 = Color32::from_rgb(0x9A, 0x66, 0x35);
const FENCE_LIGHT: Color32 = Color32::from_rgb(0xC9, 0x93, 0x58);
/// Deep teal-navy used for veils and the vignette (the panels' companion colour).
const SHADE: (u8, u8, u8) = (0x0A, 0x22, 0x36);

fn sky_stops(night: f32) -> [(f32, Color32); 4] {
    let n = |c: Color32| theme::mix(c, Color32::from_rgb(0x1B, 0x2A, 0x5C), night * 0.6);
    [(0.0, n(SKY_TOP)), (0.38, n(SKY_MID)), (0.7, n(SKY_LOW)), (1.0, n(SKY_HORIZON))]
}

/// Height of a hill silhouette at `x`: a periodic field of three sines.
fn ridge_y(x: f32, base_y: f32, amp: f32, period: f32, seed: f32) -> f32 {
    let u = x / period * std::f32::consts::TAU;
    let h = (u + seed).sin() * 0.55 + (u * 2.3 + seed * 1.7).sin() * 0.30 + (u * 5.1 + seed * 3.1).sin() * 0.15;
    base_y - amp * (0.5 + 0.5 * h)
}

/// A filled hill from its silhouette down to the bottom of `rect`.
fn ridge(p: &Painter, rect: Rect, base_y: f32, amp: f32, period: f32, seed: f32, color: Color32) {
    let mut m = Mesh::default();
    let uv = egui::epaint::WHITE_UV;
    let n = ((rect.width() / 14.0).ceil() as usize).max(8);
    for i in 0..=n {
        let x = rect.left() + rect.width() * i as f32 / n as f32;
        let y = ridge_y(x, base_y, amp, period, seed);
        let i0 = m.vertices.len() as u32;
        m.vertices.push(Vertex { pos: pos2(x, y), uv, color });
        m.vertices.push(Vertex { pos: pos2(x, rect.bottom()), uv, color });
        if i > 0 {
            m.indices.extend([i0 - 2, i0 - 1, i0, i0 - 1, i0 + 1, i0]);
        }
    }
    p.add(Shape::mesh(m));
}

/// A puffy cloud of width about `s * 2.4`, with a faint blue underside.
fn cloud(p: &Painter, c: Pos2, s: f32, alpha: f32) {
    let puffs = [(-0.95, 0.10, 0.46), (-0.38, -0.14, 0.62), (0.30, -0.06, 0.56), (0.92, 0.12, 0.42)];
    let shade = rgba(0xB4, 0xD2, 0xEE, alpha * 0.8);
    let white = rgba(0xFF, 0xFF, 0xFF, alpha * 0.95);
    for (col, dy) in [(shade, 0.09 * s), (white, 0.0)] {
        for (x, y, r) in puffs {
            p.circle_filled(c + vec2(x * s, y * s + dy), r * s, col);
        }
        p.rect_filled(Rect::from_min_max(c + vec2(-1.25 * s, 0.1 * s + dy), c + vec2(1.25 * s, 0.52 * s + dy)), 0.4 * s, col);
    }
}

/// A red barn standing on `base` (bottom centre), `u` pixels per barn-unit (the barn is 4u wide).
fn barn(p: &Painter, base: Pos2, u: f32) {
    let at = |x: f32, y: f32| base + vec2(x * u, -y * u);
    // soft ground shadow
    p.add(Shape::ellipse_filled(at(0.9, -0.05), vec2(3.6 * u, 0.28 * u), rgba(0x20, 0x60, 0x30, 0.35)));
    // silo
    let silo = Color32::from_rgb(0xC7, 0xD2, 0xDA);
    let silo_d = Color32::from_rgb(0x9C, 0xAA, 0xB6);
    p.rect_filled(Rect::from_min_max(at(2.2, 4.1), at(3.7, 0.0)), 0.0, silo);
    p.rect_filled(Rect::from_min_max(at(3.15, 4.1), at(3.7, 0.0)), 0.0, silo_d);
    p.circle_filled(at(2.95, 4.1), 0.75 * u, silo);
    p.add(Shape::convex_polygon(vec![at(2.2, 4.1), at(2.95, 4.95), at(3.7, 4.1)], silo_d, Stroke::NONE));
    for k in 1..=4 {
        p.line_segment([at(2.2, k as f32 * 0.9), at(3.7, k as f32 * 0.9)], Stroke::new((0.07 * u).max(1.0), silo_d));
    }
    // body: a gambrel gable
    let body = vec![at(-2.0, 0.0), at(-2.0, 2.5), at(-1.55, 3.4), at(0.0, 4.1), at(1.55, 3.4), at(2.0, 2.5), at(2.0, 0.0)];
    p.add(Shape::convex_polygon(body.clone(), BARN_RED, Stroke::NONE));
    // darker lower planks
    p.rect_filled(Rect::from_min_max(at(-2.0, 0.55), at(2.0, 0.0)), 0.0, BARN_DARK);
    // roof edge and white trim
    let roof = vec![at(-2.0, 2.5), at(-1.55, 3.4), at(0.0, 4.1), at(1.55, 3.4), at(2.0, 2.5)];
    p.add(Shape::line(roof, Stroke::new(0.34 * u, BARN_DARK)));
    let mut outline = body;
    outline.push(outline[0]);
    p.add(Shape::line(outline, Stroke::new((0.09 * u).max(1.0), TRIM)));
    // double door with the white cross, and a hayloft window
    let door = Rect::from_min_max(at(-0.8, 1.9), at(0.8, 0.0));
    p.rect_filled(door, 0.0, BARN_RED);
    let st = Stroke::new((0.12 * u).max(1.2), TRIM);
    p.rect_stroke(door, 0.0, st, egui::StrokeKind::Inside);
    p.line_segment([door.left_top(), door.right_bottom()], st);
    p.line_segment([door.right_top(), door.left_bottom()], st);
    p.line_segment([door.center_top(), door.center_bottom()], st);
    let win = Rect::from_min_max(at(-0.42, 3.35), at(0.42, 2.55));
    p.rect_filled(win, 0.05 * u, Color32::from_rgb(0x3A, 0x1A, 0x18));
    p.rect_stroke(win, 0.05 * u, st, egui::StrokeKind::Inside);
}

/// A small windmill: tower on `base`, four blades turning at `spin` radians.
fn windmill(p: &Painter, base: Pos2, u: f32, spin: f32, tone: f32) {
    let col = |c: Color32| theme::mix(c, HILL_FAR, tone);
    let at = |x: f32, y: f32| base + vec2(x * u, -y * u);
    let tower = col(Color32::from_rgb(0xF2, 0xEC, 0xE0));
    let dark = col(Color32::from_rgb(0xC9, 0xBE, 0xAA));
    p.add(Shape::convex_polygon(vec![at(-0.55, 0.0), at(0.55, 0.0), at(0.22, 4.2), at(-0.22, 4.2)], tower, Stroke::NONE));
    p.add(Shape::convex_polygon(vec![at(0.0, 0.0), at(0.55, 0.0), at(0.22, 4.2), at(0.0, 4.2)], dark, Stroke::NONE));
    let hub = at(0.0, 4.4);
    for k in 0..4 {
        let a = spin + k as f32 * std::f32::consts::FRAC_PI_2;
        let (sn, cs) = a.sin_cos();
        let r = |x: f32, y: f32| hub + vec2((x * cs - y * sn) * u, (x * sn + y * cs) * u);
        p.add(Shape::convex_polygon(vec![r(0.15, -0.09), r(2.6, -0.3), r(2.6, 0.3), r(0.15, 0.12)], tower, Stroke::new(0.8_f32, dark)));
    }
    p.circle_filled(hub, 0.2 * u, dark);
}

/// The sunny-farm scene filling `rect`: sky, sun, clouds, rolling hills with a
/// windmill, the barn and silo, a wooden fence and the meadow.
pub fn backdrop(p: &Painter, _b: &Brand, rect: Rect, sc: Scene) {
    let t = sc.t;
    let p = &p.with_clip_rect(rect);
    let (w, h) = (rect.width(), rect.height());
    let horizon_y = rect.top() + h * 0.60;
    let sky_rect = Rect::from_min_max(rect.min, pos2(rect.right(), horizon_y + 0.12 * h));
    vgradient(p, sky_rect, &sky_stops(sc.night));

    // sun: a halo and a pale disc, sinking behind the hills when `sun` < 0
    let sun_c = pos2(rect.left() + w * 0.80, rect.top() + h * 0.16 - sc.sun * h * 0.55);
    let sun_r = (w * 0.075).min(h * 0.05);
    glow(p, sun_c, sun_r * 5.0, rgba(0xFF, 0xF4, 0xC8, 0.55 * (1.0 - sc.night)));
    p.circle_filled(sun_c, sun_r, rgba(0xFF, 0xF8, 0xDA, 1.0 - sc.night * 0.7));

    if sc.horizon {
        let u = (w * 0.040).min(h * 0.021);
        // far hills, hazed, with the windmill
        let far_base = horizon_y;
        let far_amp = h * 0.075;
        ridge(p, rect, far_base, far_amp, w * 1.5, 0.9, HILL_FAR);
        let wx = rect.left() + w * 0.76;
        windmill(p, pos2(wx, ridge_y(wx, far_base, far_amp, w * 1.5, 0.9) + 0.4 * u), u * 0.55, t * 0.55, 0.25);
        // middle hills with the barn and silo
        let mid_base = horizon_y + h * 0.045;
        let mid_amp = h * 0.05;
        ridge(p, rect, mid_base, mid_amp, w * 1.9, 3.9, HILL_MID);
        let bx = rect.left() + w * 0.24;
        barn(p, pos2(bx, ridge_y(bx, mid_base, mid_amp, w * 1.9, 3.9) + 0.5 * u), u);
        // near hill, with the fence along its crest
        let near_base = horizon_y + h * 0.11;
        let near_amp = h * 0.045;
        ridge(p, rect, near_base, near_amp, w * 1.3, 1.7, HILL_NEAR);
        let post_gap = w / 9.0;
        let k = (h * 0.0105).max(2.0);
        let rail = |dy: f32| -> Vec<Pos2> {
            (0..=24)
                .map(|i| {
                    let x = rect.left() + w * i as f32 / 24.0;
                    pos2(x, ridge_y(x, near_base, near_amp, w * 1.3, 1.7) + 1.6 * k - dy)
                })
                .collect()
        };
        for (dy, th) in [(2.3 * k, 0.55 * k), (4.4 * k, 0.55 * k)] {
            p.add(Shape::line(rail(dy - 0.2 * k), Stroke::new(th, FENCE_LIGHT)));
            p.add(Shape::line(rail(dy), Stroke::new(th * 0.8, FENCE)));
        }
        let mut x = rect.left() - post_gap * 0.3;
        while x < rect.right() + post_gap {
            let y = ridge_y(x, near_base, near_amp, w * 1.3, 1.7) + 1.6 * k;
            let post = Rect::from_min_max(pos2(x - 0.55 * k, y - 5.6 * k), pos2(x + 0.55 * k, y + 0.5 * k));
            p.rect_filled(post, 0.3 * k, FENCE);
            p.rect_filled(Rect::from_min_max(post.min, pos2(post.min.x + 0.35 * k, post.max.y)), 0.2 * k, FENCE_LIGHT);
            x += post_gap;
        }
        // the meadow in front: mown stripes, then shade towards the bottom
        let meadow = Rect::from_min_max(pos2(rect.left(), near_base + near_amp), rect.max);
        vgradient(p, meadow, &[(0.0, HILL_NEAR), (0.18, MEADOW), (1.0, theme::mix(MEADOW, Color32::from_rgb(0x1E, 0x5A, 0x34), 0.8))]);
        for i in 0..6 {
            let k0 = (i as f32 / 6.0).powf(1.6);
            let k1 = ((i as f32 + 0.5) / 6.0).powf(1.6);
            p.rect_filled(Rect::from_min_max(pos2(rect.left(), meadow.top() + meadow.height() * k0), pos2(rect.right(), meadow.top() + meadow.height() * k1)), 0.0, rgba(255, 255, 255, 0.05));
        }
        // a few daisies and tufts
        // (not on the dense list pages, where a daisy under a label reads as a stray dot)
        for i in 0..if sc.veil > 0.5 { 0 } else { 26u32 } {
            let fx = rect.left() + hash01(i * 3 + 90) * w;
            let fy = meadow.top() + meadow.height() * (0.08 + 0.9 * hash01(i * 3 + 91).powf(0.8));
            let r = k * (0.18 + 0.4 * (fy - meadow.top()) / meadow.height());
            if i % 3 == 0 {
                let c = if i % 2 == 0 { rgba(255, 255, 255, 0.9) } else { rgba(0xFF, 0xE0, 0x6A, 0.9) };
                p.circle_filled(pos2(fx, fy), r, c);
                p.circle_filled(pos2(fx, fy), r * 0.42, rgba(0xF2, 0xA0, 0x1E, 0.95));
            } else {
                p.line_segment([pos2(fx, fy), pos2(fx - r, fy - 1.8 * r)], Stroke::new(r * 0.5, rgba(0x7A, 0xD0, 0x6A, 0.5)));
                p.line_segment([pos2(fx, fy), pos2(fx + r, fy - 1.6 * r)], Stroke::new(r * 0.5, rgba(0x7A, 0xD0, 0x6A, 0.5)));
            }
        }
    } else {
        // no farm: green down to the bottom
        vgradient(p, Rect::from_min_max(pos2(rect.left(), horizon_y), rect.max), &[(0.0, HILL_FAR), (1.0, MEADOW)]);
    }

    // a light scrim at the top keeps titles legible on the pale sky; the veil quietens busy pages
    vgradient(p, Rect::from_min_size(rect.min, vec2(w, h * 0.34)), &[(0.0, rgba(SHADE.0, SHADE.1, SHADE.2, 0.14)), (1.0, Color32::TRANSPARENT)]);
    if sc.veil > 0.0 {
        p.rect_filled(rect, 0.0, rgba(SHADE.0, SHADE.1, SHADE.2, sc.veil * 0.5));
    }
    // clouds drifting right, wrapping around the screen
    for i in 0..7u32 {
        let s = (w * 0.07 + hash01(i * 7 + 1) * w * 0.07).min(h * 0.07);
        let span = w + s * 6.0;
        let speed = 3.0 + hash01(i * 5 + 3) * 6.0;
        let x = rect.left() - s * 3.0 + (hash01(i * 3 + 2) * span + t * speed) % span;
        let y = rect.top() + h * (0.11 + 0.28 * hash01(i * 11 + 4));
        cloud(p, pos2(x, y), s, (0.92 + 0.08 * hash01(i)) * (1.0 - sc.night * 0.6) * (1.0 - sc.veil * 0.2));
    }

    vignette(p, rect, 0.10);
}

/// Darkened edges.
pub fn vignette(p: &Painter, rect: Rect, strength: f32) {
    let d = Color32::from_rgba_unmultiplied(SHADE.0 / 2, SHADE.1 / 2, SHADE.2 / 2, (strength * 255.0) as u8);
    let w = rect.width() * 0.22;
    let hh = rect.height() * 0.30;
    hgradient(p, Rect::from_min_size(rect.min, vec2(w, rect.height())), d, Color32::TRANSPARENT);
    hgradient(p, Rect::from_min_max(pos2(rect.right() - w, rect.top()), rect.max), Color32::TRANSPARENT, d);
    vgradient(p, Rect::from_min_size(rect.min, vec2(rect.width(), hh)), &[(0.0, d), (1.0, Color32::TRANSPARENT)]);
    vgradient(p, Rect::from_min_max(pos2(rect.left(), rect.bottom() - hh), rect.max), &[(0.0, Color32::TRANSPARENT), (1.0, d)]);
}

/// Over a live 3D backdrop: only a light vignette and a bottom shade so the
/// controls stay readable.
pub fn backdrop_over_3d(p: &Painter, rect: Rect, left_shade: f32) {
    vignette(p, rect, 0.45);
    if left_shade > 0.0 {
        hgradient(p, Rect::from_min_size(rect.min, vec2(rect.width() * 0.6, rect.height())), rgba(0x0C, 0x05, 0x16, left_shade), Color32::TRANSPARENT);
    }
}

/// Laser-eye beams from `from` going right, length `len`, with a hot core.
pub fn laser_beam(p: &Painter, from: Pos2, dir: egui::Vec2, len: f32, width: f32, alpha: f32) {
    let d = dir.normalized();
    let n = vec2(-d.y, d.x);
    let end = from + d * len;
    for (w, a, col) in [(width * 4.0, 0.16, theme::LASER), (width * 2.2, 0.35, theme::LASER), (width, 0.95, Color32::from_rgb(255, 220, 215))] {
        let mut m = Mesh::default();
        let uv = egui::epaint::WHITE_UV;
        let c0 = fade(col, a * alpha);
        // tapered: thick at the eye, thin at the end
        push_quad(&mut m, [from - n * w * 0.5, end - n * w * 0.12, end + n * w * 0.12, from + n * w * 0.5], [pos2(uv.x, uv.y); 4], [c0, fade(col, 0.0), fade(col, 0.0), c0]);
        p.add(Shape::mesh(m));
    }
    glow(p, from, width * 5.0, fade(theme::LASER, 0.7 * alpha));
}

/// A big shaded trophy cup centred on `c`, `s` points across. `base` is the metal colour.
pub fn trophy(p: &Painter, c: Pos2, s: f32, base: Color32) {
    let u = |x: f32, y: f32| c + vec2(x, y) * s;
    let dark = theme::mix(base, Color32::from_rgb(0x5A, 0x24, 0x08), 0.45);
    let mid = theme::mix(base, Color32::from_rgb(0x5A, 0x24, 0x08), 0.20);
    let light = theme::mix(base, Color32::WHITE, 0.55);
    let outline = theme::OUTLINE;
    let w = s * 0.075;
    // handles first, behind the cup
    for side in [-1.0_f32, 1.0] {
        let (a0, a1) = if side < 0.0 { (std::f32::consts::FRAC_PI_2, std::f32::consts::PI * 1.5) } else { (-std::f32::consts::FRAC_PI_2, std::f32::consts::FRAC_PI_2) };
        let pts: Vec<Pos2> = (0..=14).map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / 14.0;
            u(side * 0.31, -0.22) + vec2(a.cos() * 0.15 * s, a.sin() * 0.16 * s)
        }).collect();
        p.add(Shape::line(pts.clone(), Stroke::new(w + 4.0, outline)));
        p.add(Shape::line(pts, Stroke::new(w, mid)));
    }
    let cup = [(0.30, -0.40), (0.27, -0.12), (0.17, 0.06), (0.07, 0.12), (-0.07, 0.12), (-0.17, 0.06), (-0.27, -0.12), (-0.30, -0.40)];
    let pts: Vec<Pos2> = cup.iter().map(|&(x, y)| u(x, y)).collect();
    p.add(Shape::closed_line(pts.clone(), Stroke::new(5.0_f32, outline)));
    crate::ui::icons::fan(p, u(0.0, -0.14), &pts, base);
    let shade: Vec<Pos2> = [(0.12, -0.40), (0.30, -0.40), (0.27, -0.12), (0.17, 0.06), (0.07, 0.12), (0.11, -0.10)].iter().map(|&(x, y)| u(x, y)).collect();
    crate::ui::icons::fan(p, u(0.2, -0.16), &shade, dark);
    let hi: Vec<Pos2> = [(-0.23, -0.36), (-0.14, -0.36), (-0.17, -0.14), (-0.12, 0.0), (-0.19, -0.08)].iter().map(|&(x, y)| u(x, y)).collect();
    crate::ui::icons::fan(p, u(-0.17, -0.2), &hi, light);
    crate::ui::icons::star(p, u(0.0, -0.20), 0.12 * s, dark, Stroke::new(1.5_f32, outline));
    // rim, stem and base
    p.rect_filled(Rect::from_center_size(u(0.0, -0.40), vec2(0.62 * s, 0.05 * s)), s * 0.02, light);
    for (rect, col) in [
        (Rect::from_center_size(u(0.0, 0.19), vec2(0.12 * s, 0.14 * s)), mid),
        (Rect::from_center_size(u(0.0, 0.30), vec2(0.46 * s, 0.07 * s)), base),
        (Rect::from_center_size(u(0.0, 0.375), vec2(0.62 * s, 0.09 * s)), dark),
    ] {
        p.rect_stroke(rect.expand(1.5), egui::CornerRadius::same(3), Stroke::new(2.5_f32, outline), egui::StrokeKind::Outside);
        p.rect_filled(rect, 3.0, col);
    }
}
