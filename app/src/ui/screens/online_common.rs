//! Pieces the multiplayer screens share: the bird chip (colour + accessory),
//! the Bluetooth glyph, transport badges, the connecting spinner, and the
//! persona text. Kept apart from `ui/icons.rs` / `ui/widgets.rs`, which belong
//! to the gameplay screens.

use crate::ui::art::{self, from_f3, rgba};
use crate::ui::{theme, widgets};
use egui::{pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Shape, Stroke};
use flock_render::{Accessory, PlayerStyle};

/// Number of player colours (and accessories).
pub const COLOURS: u8 = 6;

/// `PlayerStyle::of` for a lobby colour.
pub fn style(colour: u8) -> PlayerStyle {
    PlayerStyle::of(colour % COLOURS)
}

/// The colour as a UI swatch.
pub fn swatch(colour: u8) -> Color32 {
    from_f3(style(colour).body)
}

/// The colour's name ("Tomato"), for the picker.
pub fn colour_name(colour: u8) -> &'static str {
    style(colour).name
}

/// A bird's face in its colour with its head accessory, in a chip of radius
/// `r` around `c`. Colour-blind readable: every colour has its own accessory.
/// `dim` (0..1) greys it for a colour somebody else holds.
pub fn bird_chip(p: &Painter, c: Pos2, r: f32, colour: u8, dim: f32) {
    bird_chip_hat(p, c, r, colour, flock_render::HAT_DEFAULT, dim);
}

/// [`bird_chip`] wearing hat `hat` (a `flock_render::HATS` id; 255 = the colour's own).
pub fn bird_chip_hat(p: &Painter, c: Pos2, r: f32, colour: u8, hat: u8, dim: f32) {
    let s = PlayerStyle::with_hat(colour % COLOURS, hat);
    let a = |col: Color32| art::fade(col, dim);
    let out = a(theme::OUTLINE);
    // the disc behind the bird
    p.circle_filled(c + vec2(0.0, 1.5), r + 1.0, rgba(0, 0, 0, 0.30 * dim));
    p.circle_filled(c, r, a(art::rgba(0x2A, 0x14, 0x48, 1.0)));
    p.circle_stroke(c, r, Stroke::new(1.5_f32, a(rgba(255, 244, 224, 0.55))));
    // head
    let hr = r * 0.62;
    let hc = c + vec2(0.0, r * 0.16);
    let pt = |x: f32, y: f32| hc + vec2(x, y) * hr;
    let body = a(from_f3(s.body));
    let neck = a(from_f3(s.neck));
    let accent = a(from_f3(s.accent));
    p.circle_filled(hc, hr + 1.2, out);
    p.circle_filled(hc, hr, body);
    p.circle_filled(pt(-0.25, 0.35), hr * 0.5, neck);
    // beak, eyes
    let beak = a(from_f3(s.beak));
    p.add(Shape::convex_polygon(vec![pt(-0.2, 0.18), pt(0.2, 0.18), pt(0.0, 0.62)], beak, Stroke::new(1.0_f32, out)));
    for sx in [-1.0_f32, 1.0] {
        p.circle_filled(pt(sx * 0.36, -0.12), hr * 0.2, a(Color32::WHITE));
        p.circle_filled(pt(sx * 0.36, -0.1), hr * 0.1, a(Color32::from_rgb(0x20, 0x10, 0x30)));
    }
    let poly = |pts: Vec<Pos2>, fill: Color32| {
        p.add(Shape::convex_polygon(pts, fill, Stroke::new(1.0_f32, out)));
    };
    match s.accessory {
        Accessory::Crown => poly(
            vec![pt(-0.55, -0.9), pt(-0.62, -1.5), pt(-0.28, -1.2), pt(0.0, -1.62), pt(0.28, -1.2), pt(0.62, -1.5), pt(0.55, -0.9)],
            accent,
        ),
        Accessory::Cap => {
            let dome: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let t = std::f32::consts::PI * (1.0 + i as f32 / 10.0);
                    pt(t.cos() * 0.98, -0.3 + t.sin() * 0.95)
                })
                .collect();
            poly(dome, accent);
            poly(vec![pt(0.0, -0.32), pt(1.25, -0.22), pt(1.22, -0.06), pt(0.0, -0.12)], accent);
        }
        Accessory::VikingHelmet => {
            let grey = a(Color32::from_rgb(0xB8, 0xBE, 0xC8));
            let horn = a(Color32::from_rgb(0xF4, 0xE6, 0xC0));
            poly(vec![pt(-0.85, -0.5), pt(-1.4, -1.2), pt(-0.5, -0.85)], horn);
            poly(vec![pt(0.85, -0.5), pt(1.4, -1.2), pt(0.5, -0.85)], horn);
            let dome: Vec<Pos2> = (0..=10)
                .map(|i| {
                    let t = std::f32::consts::PI * (1.0 + i as f32 / 10.0);
                    pt(t.cos() * 0.95, -0.28 + t.sin() * 0.85)
                })
                .collect();
            poly(dome, grey);
            poly(vec![pt(-0.95, -0.4), pt(0.95, -0.4), pt(0.95, -0.22), pt(-0.95, -0.22)], accent);
        }
        Accessory::Mohawk => {
            for i in 0..5 {
                let x = -0.5 + 0.25 * i as f32;
                let h = if i == 2 { 1.65 } else if i % 2 == 0 { 1.5 } else { 1.38 };
                poly(vec![pt(x - 0.15, -0.86), pt(x, -h), pt(x + 0.15, -0.86)], accent);
            }
        }
        Accessory::StarHeadband => {
            poly(vec![pt(-0.98, -0.52), pt(0.98, -0.52), pt(0.98, -0.3), pt(-0.98, -0.3)], accent);
            crate::ui::icons::star(p, pt(0.0, -0.88), hr * 0.5, a(theme::GOLD), Stroke::new(1.0_f32, out));
        }
        Accessory::Scarf => {
            poly(vec![pt(-0.95, 0.55), pt(0.95, 0.55), pt(1.0, 0.92), pt(-1.0, 0.92)], accent);
            poly(vec![pt(0.45, 0.85), pt(0.9, 0.85), pt(0.95, 1.5), pt(0.55, 1.4)], accent);
        }
        Accessory::Goggles | Accessory::ChainShades => {
            for sx in [-1.0_f32, 1.0] {
                p.circle_stroke(pt(sx * 0.36, -0.12), hr * 0.3, Stroke::new(2.0_f32, accent));
            }
        }
        Accessory::None => {}
    }
}

/// The Bluetooth rune, centred on `c` in a box of side `s`.
pub fn bluetooth_glyph(p: &Painter, c: Pos2, s: f32, col: Color32) {
    let u = |x: f32, y: f32| c + vec2(x, y) * s;
    let pts = vec![u(-0.27, -0.2), u(0.27, 0.24), u(0.0, 0.48), u(0.0, -0.48), u(0.27, -0.24), u(-0.27, 0.2)];
    p.add(Shape::line(pts, Stroke::new((s * 0.11).max(1.6), col)));
}

/// Which way in a transport badge reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Door {
    Online,
    Nearby,
    Bluetooth,
}

impl Door {
    pub fn label(self) -> &'static str {
        match self {
            Door::Online => "Online",
            Door::Nearby => "Nearby",
            Door::Bluetooth => "Bluetooth",
        }
    }
}

/// How a door is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DoorState {
    /// Open and working.
    On,
    /// Coming up.
    Opening,
    /// Not on (or failed).
    Off,
}

/// A small badge: a status dot (or the Bluetooth glyph) and the door's name.
/// Returns its rect; `left_top` is where it starts.
pub fn door_badge(p: &Painter, left_top: Pos2, door: Door, state: DoorState, tm: f32) -> Rect {
    let size = 12.0;
    let g = p.layout_no_wrap(door.label().to_string(), theme::ui_bold(size), if state == DoorState::Off { theme::TEXT_DIM } else { theme::TEXT });
    let w = g.size().x + 40.0;
    let rect = Rect::from_min_size(left_top, vec2(w, 26.0));
    let (bg, edge) = match state {
        DoorState::On => (rgba(0x14, 0x40, 0x34, 0.85), rgba(0x6B, 0xE0, 0xA8, 0.8)),
        DoorState::Opening => (rgba(0x40, 0x2C, 0x10, 0.85), rgba(0xFF, 0xC8, 0x3D, 0.8)),
        DoorState::Off => (rgba(10, 4, 24, 0.5), rgba(255, 255, 255, 0.14)),
    };
    p.rect_filled(rect, 13.0, bg);
    p.rect_stroke(rect, 13.0, Stroke::new(1.2_f32, edge), egui::StrokeKind::Inside);
    let dot = pos2(rect.left() + 14.0, rect.center().y);
    let col = match state {
        DoorState::On => theme::MINT,
        DoorState::Opening => theme::GOLD.gamma_multiply(0.55 + 0.45 * (tm * 5.0).sin().abs()),
        DoorState::Off => rgba(0xB8, 0xA6, 0xC9, 0.5),
    };
    if door == Door::Bluetooth {
        bluetooth_glyph(p, dot, 14.0, col);
    } else {
        p.circle_filled(dot, 4.5, col);
    }
    p.galley(pos2(rect.left() + 28.0, rect.center().y - g.size().y / 2.0), g, theme::TEXT);
    rect
}

/// A spinning arc (connecting).
pub fn spinner(p: &Painter, c: Pos2, r: f32, tm: f32) {
    p.circle_stroke(c, r, Stroke::new(r * 0.17, rgba(255, 255, 255, 0.12)));
    let a0 = tm * 5.0;
    let pts: Vec<Pos2> = (0..=18).map(|i| c + vec2((a0 + i as f32 * 0.07).cos(), (a0 + i as f32 * 0.07).sin()) * r).collect();
    p.add(Shape::line(pts, Stroke::new(r * 0.17, theme::ACCENT)));
}

/// `npub1abcdefgh...stuvwxyz`, short enough for a row.
pub fn short_npub(npub: &str) -> String {
    let chars: Vec<char> = npub.chars().collect();
    if chars.len() <= 20 {
        return npub.to_string();
    }
    let head: String = chars[..10].iter().collect();
    let tail: String = chars[chars.len() - 6..].iter().collect();
    format!("{head}\u{2026}{tail}")
}

/// A centred card for a message with a title, drawing its wrapped text.
/// Returns the rect of the card.
pub fn message_card(p: &Painter, card: Rect, title: &str, title_col: Color32, text: &str) {
    widgets::panel(p, card, 22.0);
    p.text(pos2(card.center().x, card.top() + 34.0), Align2::CENTER_CENTER, title, theme::title(24.0), title_col);
    let g = p.layout(text.to_string(), theme::ui_medium(14.0), theme::TEXT, card.width() - 48.0);
    p.galley(pos2(card.left() + 24.0, card.top() + 66.0), g, theme::TEXT);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_six_colours_are_distinct_and_each_has_its_own_accessory() {
        let mut acc = Vec::new();
        for c in 0..COLOURS {
            let s = style(c);
            assert!(!acc.contains(&s.accessory), "accessory repeated at {c}");
            acc.push(s.accessory);
        }
        assert_eq!(acc.len(), 6);
    }

    #[test]
    fn personas_are_shortened_for_rows() {
        let npub = "npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqzzzzzz";
        let s = short_npub(npub);
        assert!(s.chars().count() < 20 && s.starts_with("npub1qqqqq") && s.ends_with("zzzzzz"));
        assert_eq!(short_npub("npub1abc"), "npub1abc");
    }
}
