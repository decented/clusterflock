//! Meshes of the farm obstacles and small Cluster Flock extras (stars, eyes, ghost wisps,
//! placement cells). Obstacles are built in a unit box (x and z in -0.5..0.5, y in 0..1,
//! designed for the sim's reference size) and stretched to the sim's real size by the instance
//! matrix, so the app only supplies the collision dimensions.

use crate::mesh::{basis, hash01, limb, MeshData, Style};
use crate::types::*;
use glam::{Mat4, Vec2, Vec3};
use std::f32::consts::{FRAC_PI_2, TAU};

/// How one [`FarmPropInst`] is drawn this frame: which mesh, where, how translucent.
pub(crate) struct PropDraw {
    pub mesh: crate::meshes::MeshId,
    pub model: Mat4,
    pub alpha: f32,
}

/// Mesh, matrix and alpha of a prop, including the architect drop-in: it falls from
/// [`DROP_HEIGHT`] with an ease-in (so `drop` = 1 is the instant of impact, whatever time the fall
/// was spread over), then squashes flat and springs back over `settle` 0..1, and fades in over
/// the first sixth of the fall.
pub(crate) fn prop_draw(fp: &FarmPropInst) -> PropDraw {
    use crate::meshes::MeshId;
    let mesh = match fp.kind {
        FarmPropKind::HayBale => MeshId::HayBale,
        FarmPropKind::Beam { span_lanes } => {
            if span_lanes >= 3 {
                MeshId::Beam3
            } else {
                MeshId::Beam1
            }
        }
        FarmPropKind::Block { variant } => match variant {
            BlockVariant::BarnWall => MeshId::BlockBarn,
            BlockVariant::Tractor => MeshId::Tractor,
            BlockVariant::Trough => MeshId::Trough,
        },
        FarmPropKind::Mud => MeshId::Mud,
        FarmPropKind::Barrel => MeshId::Barrel,
    };
    let (base, alpha) = drop_base(fp);
    let model = match fp.kind {
        // A mud decal keeps its own thickness: its mesh is in metres, only x and z stretch.
        FarmPropKind::Mud => base * Mat4::from_scale(Vec3::new(fp.size.x, 1.0, fp.size.z)),
        // The gate frame is in metres in y; it only grows if the bar is raised.
        FarmPropKind::Beam { .. } => base * Mat4::from_scale(Vec3::new(fp.size.x, frame_scale(fp), fp.size.z)),
        // The barrel is centred: lift it by its radius and roll it about its axis (x).
        FarmPropKind::Barrel => {
            let c = Vec3::Y * (fp.size.y * 0.5);
            base * Mat4::from_translation(c) * Mat4::from_rotation_x(fp.roll) * Mat4::from_scale(fp.size)
        }
        _ => base * Mat4::from_scale(fp.size),
    };
    PropDraw { mesh, model, alpha }
}

/// The other draws of a gate (the frame is [`prop_draw`]): the striped crossbar and the bunting
/// and sacks hanging under it. `[None, None]` for every other prop.
pub(crate) fn prop_extras(fp: &FarmPropInst) -> [Option<PropDraw>; 2] {
    use crate::meshes::MeshId;
    let FarmPropKind::Beam { span_lanes } = fp.kind else { return [None, None] };
    let (base, alpha) = drop_base(fp);
    let under = fp.size.y;
    let thick = (bar_top(fp) - under).max(0.2);
    let at = base * Mat4::from_translation(Vec3::Y * under);
    let wide = span_lanes >= 3;
    [
        Some(PropDraw { mesh: if wide { MeshId::BeamBar3 } else { MeshId::BeamBar1 }, model: at * Mat4::from_scale(Vec3::new(fp.size.x, thick, fp.size.z)), alpha }),
        Some(PropDraw { mesh: if wide { MeshId::BeamHang3 } else { MeshId::BeamHang1 }, model: at * Mat4::from_scale(Vec3::new(fp.size.x, 1.0, fp.size.z)), alpha }),
    ]
}

/// Top of a beam's collision bar: the sim's value, or the underside plus 0.6.
pub(crate) fn bar_top(fp: &FarmPropInst) -> f32 {
    if fp.bar_top > fp.size.y + 0.05 {
        fp.bar_top
    } else {
        fp.size.y + 0.6
    }
}

/// Stretch of the gate frame in y, so the posts and the top rail stay above a raised bar.
fn frame_scale(fp: &FarmPropInst) -> f32 {
    ((bar_top(fp) + (GATE_RAIL_LO - 1.75)) / GATE_RAIL_LO).max(1.0)
}

/// Squash on impact: 0 outside the settle, else a damped spring that starts fully squashed
/// (positive = flatter and wider) and overshoots into a stretch before it rests.
pub(crate) fn settle_squash(settle: f32) -> f32 {
    let s = settle.clamp(0.0, 1.0);
    if s >= 1.0 {
        0.0
    } else {
        0.30 * (-4.5 * s).exp() * (11.0 * s).cos()
    }
}

/// Translation and drop-in squash shared by every draw of a prop, and its alpha.
fn drop_base(fp: &FarmPropInst) -> (Mat4, f32) {
    let d = fp.drop.clamp(0.0, 1.0);
    let lift = if d < 1.0 { DROP_HEIGHT * (1.0 - d * d) } else { 0.0 };
    let mud = matches!(fp.kind, FarmPropKind::Mud);
    let scale = if d < 1.0 {
        if mud {
            // a falling blob: narrow and tall, it splats flat on impact
            let w = 0.45 + 0.55 * d;
            Vec3::new(w, 1.0 + 5.0 * (1.0 - d), w)
        } else {
            let st = 0.07 * d * d * d;
            Vec3::new(1.0 - 0.5 * st, 1.0 + st, 1.0 - 0.5 * st)
        }
    } else {
        let sq = settle_squash(fp.settle);
        if mud {
            Vec3::new(1.0 + 1.0 * sq, 1.0 - 0.7 * sq, 1.0 + 1.0 * sq)
        } else {
            Vec3::new(1.0 + 0.6 * sq, 1.0 - sq, 1.0 + 0.6 * sq)
        }
    };
    let base = Mat4::from_translation(fp.pos + Vec3::Y * lift) * Mat4::from_scale(scale);
    (base, fp.alpha.clamp(0.0, 1.0) * (d * 6.0).min(1.0))
}

/// Where a gate's name banner hangs, relative to the prop: width, height and the y of its
/// bottom edge (between the bar's top and the frame's top rail), both faces.
pub(crate) fn banner_dims(fp: &FarmPropInst) -> (f32, f32, f32) {
    let pw = if matches!(fp.kind, FarmPropKind::Beam { span_lanes } if span_lanes >= 3) { 0.42 } else { 0.30 };
    let y0 = bar_top(fp) + 0.07;
    let rail = GATE_RAIL_LO * frame_scale(fp);
    let h = (rail - y0 - 0.06).clamp(0.2, 0.5);
    let w = (fp.size.x - 2.0 * pw - 0.2).clamp(0.6, 2.6);
    (w, h, y0)
}

/// The mark of the architect who dropped this prop: straps on a hay bale, the banner panel on a
/// gate. Mesh, model matrix and sRGB tint; `None` for unmarked props and every other kind.
pub(crate) fn placer_mark(fp: &FarmPropInst) -> Option<(crate::meshes::MeshId, Mat4, Rgb)> {
    use crate::meshes::MeshId;
    let colour = fp.placer?;
    let (base, _) = drop_base(fp);
    match fp.kind {
        FarmPropKind::HayBale => Some((MeshId::PlacerStraps, base * Mat4::from_scale(fp.size), colour)),
        FarmPropKind::Beam { .. } => {
            let (w, h, y0) = banner_dims(fp);
            Some((MeshId::PlacerBanner, base * Mat4::from_translation(Vec3::Y * y0) * Mat4::from_scale(Vec3::new(w, h, BANNER_DEPTH)), colour))
        }
        _ => None,
    }
}

/// World position of the middle of a gate's banner (render space, with the fall applied), and
/// its size in metres: what an overlay needs to put the placer's name on it.
pub fn banner_anchor(fp: &FarmPropInst) -> Option<(Vec3, Vec2)> {
    if !matches!(fp.kind, FarmPropKind::Beam { .. }) || fp.placer.is_none() {
        return None;
    }
    let (w, h, y0) = banner_dims(fp);
    let (base, _) = drop_base(fp);
    Some((base.transform_point3(Vec3::Y * (y0 + h * 0.5)), Vec2::new(w, h)))
}

/// Straps of the placer's colour round a hay bale (a unit box like the bale itself): two bands
/// a little proud of the twine, and a short flag-tab on the top so it reads from above.
pub(crate) fn placer_straps() -> MeshData {
    let mut m = MeshData::new();
    let (w, h, d) = (HAY_REF.x, HAY_REF.y, HAY_REF.z);
    let st = Style::tint0(1.0).emit(0.12).flat();
    for x in [-0.42f32, 0.42] {
        m.aabb(Vec3::new(x - 0.085, -0.001, -d * 0.5 - 0.035), Vec3::new(x + 0.085, h + 0.03, d * 0.5 + 0.035), &st);
    }
    // a tab on top between the straps
    m.aabb(Vec3::new(-0.25, h + 0.02, -0.28), Vec3::new(0.25, h + 0.05, 0.28), &st);
    let mut b = MeshData::new();
    b.append(&m, &s(Vec3::new(1.0 / w, 1.0 / h, 1.0 / d)));
    b
}

/// A gate's name panel: a unit box (x -0.5..0.5, y 0..1, z -0.5..0.5) in the placer's colour with
/// a cream border on both faces; the instance scales it to the banner size.
pub(crate) fn placer_banner() -> MeshData {
    let mut m = MeshData::new();
    m.aabb(Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5), &Style::tint0(0.95).emit(0.1));
    let trim = flat(0xfff4d8);
    let e = 0.5 + 0.004;
    for side in [-1.0f32, 1.0] {
        let z = side * e;
        let n = Vec3::Z * side;
        let (bx, by) = (0.025f32, 0.07f32);
        for (a, c) in [((-0.5, 0.0), (0.5, by)), ((-0.5, 1.0 - by), (0.5, 1.0)), ((-0.5, by), (-0.5 + bx, 1.0 - by)), ((0.5 - bx, by), (0.5, 1.0 - by))] {
            m.quad4([Vec3::new(a.0, a.1, z), Vec3::new(c.0, a.1, z), Vec3::new(c.0, c.1, z), Vec3::new(a.0, c.1, z)], n, &trim);
        }
    }
    m
}

/// Hex colour to sRGB floats.
pub(crate) fn hx(hex: u32) -> Rgb {
    [((hex >> 16) & 255) as f32 / 255.0, ((hex >> 8) & 255) as f32 / 255.0, (hex & 255) as f32 / 255.0]
}

fn flat(hex: u32) -> Style {
    Style::rgb(hx(hex)).flat()
}

fn t(p: Vec3) -> Mat4 {
    Mat4::from_translation(p)
}

fn s(p: Vec3) -> Mat4 {
    Mat4::from_scale(p)
}

/// Reference sizes the unit meshes are drawn for (the sim's numbers).
pub(crate) const BEAM1_REF: Vec3 = Vec3::new(2.0, 1.0, 0.4);
pub(crate) const BEAM3_REF: Vec3 = Vec3::new(7.4, 1.0, 0.4);
pub(crate) const BLOCK_REF: Vec3 = Vec3::new(2.0, 2.5, 3.0);
pub(crate) const HAY_REF: Vec3 = Vec3::new(1.6, 0.7, 1.0);
/// Gate frame heights (m): tall posts, a top rail, and the striped bar's reference thickness.
const GATE_POST_TOP: f32 = 2.62;
const GATE_RAIL_LO: f32 = 2.28;
const GATE_RAIL_HI: f32 = 2.52;
const BAR_THICK: f32 = 0.6;
/// Horizontal run of one hazard stripe of the bar (m).
const BAR_SLANT: f32 = 0.22;
const BEAM_DEPTH: f32 = 0.4;
/// Depth (m) of a gate's name banner: inside the top rail, proud of the spindles.
const BANNER_DEPTH: f32 = 0.30;
/// Distance between lane centres, for the arrow panels of the three-lane gate.
const LANE_PITCH: f32 = 2.2;

/// A stationary rectangular bale: bevelled straw block, a seam that suggests two stacked
/// bales, straw stripes, two bands of red twine and a few loose stalks. Built in metres at
/// `HAY_REF` and normalised to the unit box. It never rolls: every edge is square.
pub(crate) fn hay_bale() -> MeshData {
    let mut m = MeshData::new();
    let (w, h, d) = (HAY_REF.x, HAY_REF.y, HAY_REF.z);
    let straw = flat(0xe9b94a);
    let straw_dark = flat(0xc78f2e);
    let straw_light = flat(0xf7d577);
    let twine = flat(0xc2432a);
    let seam = flat(0x8d5f1d);
    // Two bales stacked (lower slightly wider), each a bevelled block.
    let half = h * 0.5;
    for k in 0..2 {
        let y0 = half * k as f32;
        let inset = 0.02 * k as f32;
        let c = Vec3::new(0.0, y0 + half * 0.5, 0.0);
        let sz = Vec3::new(w - 2.0 * inset, half - 0.012, d - 2.0 * inset);
        let st = if k == 0 { straw.ao(0.3) } else { straw.ao(0.1) };
        m.bevel_box(&(t(c) * s(sz)), 0.07 / sz.min_element().max(0.2), &st);
    }
    // Straw stripes on the front (-z) face: vertical, and on the top face: along z.
    let ns = 15;
    for i in 0..ns {
        let x0 = -w * 0.5 + 0.05 + (w - 0.1) * i as f32 / ns as f32;
        let x1 = x0 + (w - 0.1) / ns as f32 * 0.55;
        let st = if i % 3 == 0 { &straw_dark } else if i % 3 == 1 { &straw_light } else { &straw };
        for k in 0..2 {
            let y0 = half * k as f32 + 0.05;
            let y1 = half * (k + 1) as f32 - 0.06;
            let zf = -d * 0.5 + 0.02 * k as f32 - 0.012;
            m.quad4([Vec3::new(x0, y0, zf), Vec3::new(x1, y0, zf), Vec3::new(x1, y1, zf), Vec3::new(x0, y1, zf)], -Vec3::Z, st);
        }
        let yt = h + 0.003;
        m.quad4([Vec3::new(x0, yt, -d * 0.5 + 0.06), Vec3::new(x1, yt, -d * 0.5 + 0.06), Vec3::new(x1, yt, d * 0.5 - 0.06), Vec3::new(x0, yt, d * 0.5 - 0.06)], Vec3::Y, st);
    }
    let ni = 7;
    for i in 0..ni {
        let z0 = -d * 0.5 + 0.08 + (d - 0.16) * i as f32 / ni as f32;
        let z1 = z0 + (d - 0.16) / ni as f32 * 0.5;
        for side in [-1.0f32, 1.0] {
            let x = side * (w * 0.5 + 0.004);
            let st = if i % 2 == 0 { &straw_dark } else { &straw_light };
            m.quad4([Vec3::new(x, 0.06, z0), Vec3::new(x, 0.06, z1), Vec3::new(x, h - 0.07, z1), Vec3::new(x, h - 0.07, z0)], Vec3::X * side, st);
        }
    }
    // The seam between the two bales: a dark groove all round the front and sides.
    m.aabb(Vec3::new(-w * 0.5 - 0.006, half - 0.014, -d * 0.5 - 0.016), Vec3::new(w * 0.5 + 0.006, half + 0.014, d * 0.5 + 0.006), &seam);
    // Two bands of twine all the way round (front, top, back).
    for x in [-0.42f32, 0.42] {
        m.aabb(Vec3::new(x - 0.03, -0.001, -d * 0.5 - 0.022), Vec3::new(x + 0.03, h + 0.014, d * 0.5 + 0.022), &twine);
    }
    // Loose stalks poking out of the top and the front.
    for i in 0..14 {
        let hx_ = hash01(i, 3, 5);
        let hz_ = hash01(i, 7, 9);
        let base = if i % 2 == 0 { Vec3::new(-w * 0.5 + 0.1 + (w - 0.2) * hx_, h, -d * 0.5 + 0.1 + (d - 0.2) * hz_) } else { Vec3::new(-w * 0.5 + 0.1 + (w - 0.2) * hx_, 0.06 + (h - 0.12) * hz_, -d * 0.5) };
        let out = if i % 2 == 0 { Vec3::new(hash01(i, 1, 1) - 0.5, 1.0, hash01(i, 2, 2) - 0.5) } else { Vec3::new(hash01(i, 1, 1) - 0.5, hash01(i, 2, 2) * 0.4 - 0.2, -1.0) };
        m.cone(&limb(base, base + out.normalize() * 0.13, 0.014), 4, &straw_light);
    }
    let mut b = MeshData::new();
    b.append(&m, &s(Vec3::new(1.0 / w, 1.0 / h, 1.0 / d)));
    b
}

fn timber(hex: u32) -> Style {
    flat(hex)
}

/// Where the parts of a gate sit, shared by the frame and bar meshes.
struct GateDims {
    span: f32,
    post_w: f32,
}

fn gate_dims(span_lanes: u8) -> GateDims {
    if span_lanes >= 3 {
        GateDims { span: BEAM3_REF.x, post_w: 0.42 }
    } else {
        GateDims { span: BEAM1_REF.x, post_w: 0.30 }
    }
}

/// Overhead gate, part 1: the frame. Tall posts on stone footings, a top rail and a row of
/// spindles, in metres with the ground at y = 0 and normalised in x (by the span) and z (by the
/// depth): the instance scales x by the sim width and z by the sim depth, y by `frame_scale`.
pub(crate) fn beam_frame(span_lanes: u8) -> MeshData {
    let g = gate_dims(span_lanes);
    let (span, pw) = (g.span, g.post_w);
    let wood = timber(0x8f6236);
    let wood_dark = timber(0x654223);
    let wood_light = timber(0xb98250);
    let mut m = MeshData::new();
    let pd = 0.30; // half depth of a post
    let px = span * 0.5 - pw * 0.5;
    for side in [-1.0f32, 1.0] {
        let x = side * px;
        m.aabb(Vec3::new(x - pw * 0.5, 0.0, -pd), Vec3::new(x + pw * 0.5, GATE_POST_TOP, pd), &wood.ao(0.3));
        // Cap, footing and a knot of light grain on the front.
        m.aabb(Vec3::new(x - pw * 0.62, GATE_POST_TOP, -pd * 1.1), Vec3::new(x + pw * 0.62, GATE_POST_TOP + 0.12, pd * 1.1), &wood_dark);
        m.aabb(Vec3::new(x - pw * 0.75, 0.0, -pd * 1.25), Vec3::new(x + pw * 0.75, 0.2, pd * 1.25), &flat(0x8d8a84));
        m.aabb(Vec3::new(x - pw * 0.12, 0.25, -pd - 0.012), Vec3::new(x + pw * 0.12, GATE_POST_TOP - 0.1, -pd - 0.004), &wood_light);
        // Corner brace from the post up to the top rail.
        let inner = x - side * pw * 0.5;
        m.cyl(&limb(Vec3::new(inner, GATE_RAIL_LO - 0.5, -0.05), Vec3::new(inner - side * 0.5, GATE_RAIL_LO, -0.05), 0.07), 6, 1.0, 1.0, false, &wood_dark);
    }
    // Top rail.
    m.aabb(Vec3::new(-span * 0.5 - 0.08, GATE_RAIL_LO, -0.22), Vec3::new(span * 0.5 + 0.08, GATE_RAIL_HI, 0.22), &wood.ao(0.2));
    m.aabb(Vec3::new(-span * 0.5 - 0.08, GATE_RAIL_HI - 0.03, -0.23), Vec3::new(span * 0.5 + 0.08, GATE_RAIL_HI + 0.03, 0.23), &wood_light);
    // Spindles between the bar and the rail: it is a gate, a solid header, not an open hurdle.
    let n = (span / 0.42).round() as i32;
    for i in 1..n {
        let x = -span * 0.5 + span * i as f32 / n as f32;
        if x.abs() > px - pw * 0.5 - 0.1 {
            continue;
        }
        m.aabb(Vec3::new(x - 0.045, 1.6, -0.12), Vec3::new(x + 0.045, GATE_RAIL_LO, 0.12), &wood_dark);
    }
    let mut b = MeshData::new();
    b.append(&m, &s(Vec3::new(1.0 / span, 1.0, 1.0 / BEAM_DEPTH)));
    b
}

/// Overhead gate, part 2: the striped crossbar you duck under. Built with its underside at
/// y = 0 and `BAR_THICK` tall (the instance scales y by the sim's real thickness), with bunting
/// and tassels dangling below it, so the open gap under it reads at a glance.
pub(crate) fn beam_bar(span_lanes: u8) -> MeshData {
    let g = gate_dims(span_lanes);
    let (span, pw) = (g.span, g.post_w);
    let yellow = flat(0xffc61a);
    let black = flat(0x26201c);
    let mut m = MeshData::new();
    let half = span * 0.5 - pw * 0.4;
    // The bar: dark core, yellow diagonal stripes on both faces (the runners see the -z one, an
    // architect looking back down the course sees the +z one).
    m.aabb(Vec3::new(-half, 0.0, -BEAM_DEPTH * 0.5), Vec3::new(half, BAR_THICK, BEAM_DEPTH * 0.5), &black.ao(0.15));
    let len = half * 2.0;
    let n = (len / 0.36).round() as i32;
    let cw = (len - BAR_SLANT) / n as f32;
    for side in [-1.0f32, 1.0] {
        let zf = side * (BEAM_DEPTH * 0.5 + 0.012);
        for i in 0..n {
            let xa = -half + cw * i as f32;
            if i % 2 == 0 {
                m.quad4(
                    [Vec3::new(xa, 0.0, zf), Vec3::new(xa + cw, 0.0, zf), Vec3::new(xa + cw + BAR_SLANT, BAR_THICK, zf), Vec3::new(xa + BAR_SLANT, BAR_THICK, zf)],
                    Vec3::Z * side,
                    &yellow,
                );
            }
        }
    }
    // The underside is dark too, and the lid slightly lighter so the top edge reads.
    m.aabb(Vec3::new(-half, BAR_THICK - 0.03, -BEAM_DEPTH * 0.5 - 0.01), Vec3::new(half, BAR_THICK, BEAM_DEPTH * 0.5 + 0.01), &flat(0x5a4a3a));
    // "Duck" panels: a blue board with a white down arrow, one per lane the bar covers.
    let lanes = if span_lanes >= 3 { 3 } else { 1 };
    for side in [-1.0f32, 1.0] {
        let pz = side * (BEAM_DEPTH * 0.5 + 0.024);
        let toward = Vec3::Z * side;
        for l in 0..lanes {
            let cx = if lanes == 1 { 0.0 } else { (l as f32 - 1.0) * LANE_PITCH };
            let ph = BAR_THICK * 0.9;
            let pwid = 0.56;
            m.aabb(Vec3::new(cx - pwid * 0.5, (BAR_THICK - ph) * 0.5, pz - 0.012), Vec3::new(cx + pwid * 0.5, (BAR_THICK + ph) * 0.5, pz + 0.012), &flat(0x1c63b0));
            let zz = pz + side * 0.016;
            let white = flat(0xffffff);
            let (y0, y1) = ((BAR_THICK - ph) * 0.5 + 0.06, (BAR_THICK + ph) * 0.5 - 0.05);
            let shaft = 0.075;
            m.quad4([Vec3::new(cx - shaft, y1, zz), Vec3::new(cx + shaft, y1, zz), Vec3::new(cx + shaft, y0 + 0.2, zz), Vec3::new(cx - shaft, y0 + 0.2, zz)], toward, &white);
            m.tri([Vec3::new(cx - 0.2, y0 + 0.21, zz), Vec3::new(cx + 0.2, y0 + 0.21, zz), Vec3::new(cx, y0, zz)], toward, &white);
        }
    }
    let mut b = MeshData::new();
    b.append(&m, &s(Vec3::new(1.0 / span, 1.0 / BAR_THICK, 1.0 / BEAM_DEPTH)));
    b
}

/// The bunting and feed sacks dangling under a gate's bar, with the underside at y = 0 (unscaled in y:
/// they keep their size however thick the bar is). They are short, so the open gap stays obvious.
pub(crate) fn beam_hang(span_lanes: u8) -> MeshData {
    let g = gate_dims(span_lanes);
    let (span, pw) = (g.span, g.post_w);
    let mut m = MeshData::new();
    let half = span * 0.5 - pw * 0.4;
    let len = half * 2.0;
    // Bunting dangling from the underside: a string and pennants in red, white and yellow.
    let cols = [0xe2402a, 0xfff4d8, 0xffc61a, 0x2d9bd6];
    let zb = -BEAM_DEPTH * 0.5 + 0.03;
    let np = (len / 0.3).round() as i32;
    let step = len / np as f32;
    m.cyl(&(t(Vec3::new(-half, -0.015, zb)) * Mat4::from_rotation_z(-FRAC_PI_2) * s(Vec3::new(0.012, len, 0.012))), 4, 1.0, 1.0, false, &flat(0x3a2a1e));
    for i in 0..np {
        let x = -half + step * (i as f32 + 0.5);
        let len_p = 0.24 + 0.04 * hash01(i, 4, 2);
        let wid = step * 0.42;
        m.tri([Vec3::new(x - wid, -0.015, zb), Vec3::new(x + wid, -0.015, zb), Vec3::new(x, -0.015 - len_p, zb)], -Vec3::Z, &flat(cols[(i as usize) % cols.len()]));
    }
    // Feed sacks hanging at both ends by short ropes, so it reads as a heavy overhead load.
    for side in [-1.0f32, 1.0] {
        let x = side * (half - 0.2);
        m.cyl(&limb(Vec3::new(x, 0.0, zb), Vec3::new(x, -0.1, zb), 0.02), 4, 1.0, 1.0, false, &flat(0x3a2a1e));
        m.sphere(&(t(Vec3::new(x, -0.28, zb)) * s(Vec3::new(0.15, 0.2, 0.13))), 8, 5, &flat(0xd9c28f));
        m.aabb(Vec3::new(x - 0.07, -0.13, zb - 0.02), Vec3::new(x + 0.07, -0.09, zb + 0.02), &flat(0xa84a2a));
    }
    let mut b = MeshData::new();
    b.append(&m, &s(Vec3::new(1.0 / span, 1.0, 1.0 / BEAM_DEPTH)));
    b
}

/// A wooden barrel on its side, axis along x, centred on the origin with radius 0.5 and length 1
/// (the instance scales it to the sim's size and rolls it). Bulged staves in two tones with a
/// pair of red staves, dark iron hoops and lids, a bung, so the spin shows.
pub(crate) fn barrel() -> MeshData {
    let mut b = MeshData::new();
    let wood_a = flat(0xb27a42);
    let wood_b = flat(0x94622f);
    let red = flat(0xc4432f);
    let iron = Style::rgb(hx(0x3b4048)).flat().sheen(0.5);
    let staves = 14;
    let slices = 8;
    let rad = |u: f32| 0.5 * (1.0 - 0.09 * (2.0 * u - 1.0).powi(2) * 1.0) / 1.0; // u 0..1 along the axis
    let at = |a: f32, u: f32, grow: f32| {
        let r = rad(u) + grow;
        Vec3::new(u - 0.5, a.cos() * r, a.sin() * r)
    };
    for k in 0..staves {
        let a0 = TAU * k as f32 / staves as f32;
        let a1 = TAU * (k as f32 + 0.92) / staves as f32;
        let col = if k == 0 || k == staves / 2 { &red } else if k % 2 == 0 { &wood_a } else { &wood_b };
        for i in 0..slices {
            let u0 = 0.04 + 0.92 * i as f32 / slices as f32;
            let u1 = 0.04 + 0.92 * (i + 1) as f32 / slices as f32;
            let mid = (a0 + a1) * 0.5;
            b.quad4([at(a0, u0, 0.0), at(a0, u1, 0.0), at(a1, u1, 0.0), at(a1, u0, 0.0)], Vec3::new(0.0, mid.cos(), mid.sin()), col);
        }
    }
    // Lids: dark discs with a lighter plank cross, set a little in from the ends.
    for side in [-1.0f32, 1.0] {
        let x = side * 0.5;
        let m = t(Vec3::new(x, 0.0, 0.0)) * Mat4::from_rotation_z(-side * FRAC_PI_2) * s(Vec3::new(rad(0.0) * 0.98, 1.0, rad(0.0) * 0.98));
        b.disc(&m, 14, &flat(0x6d4a2b));
        let m2 = t(Vec3::new(x + side * 0.004, 0.0, 0.0)) * Mat4::from_rotation_z(-side * FRAC_PI_2) * s(Vec3::new(rad(0.0) * 0.6, 1.0, rad(0.0) * 0.6));
        b.disc(&m2, 14, &flat(0x815632));
    }
    // Hoops: near each end and either side of the belly.
    for u in [0.07f32, 0.27, 0.73, 0.93] {
        let w = 0.065;
        let r = rad(u) + 0.014;
        let x0 = u - 0.5 - w * 0.5;
        b.cyl(&(t(Vec3::new(x0, 0.0, 0.0)) * Mat4::from_rotation_z(-FRAC_PI_2) * s(Vec3::new(r, w, r))), 16, 1.0, 1.0, false, &iron);
    }
    // A bung and a painted ring on the belly make the roll obvious.
    b.sphere(&(t(Vec3::new(0.0, 0.0, 0.5)) * s(Vec3::new(0.045, 0.045, 0.03))), 6, 4, &flat(0x2a1a10));
    b
}

/// Barn wall section (red planks, white cross-brace), in the unit box.
pub(crate) fn block_barn() -> MeshData {
    let mut m = MeshData::new();
    let (w, h, d) = (BLOCK_REF.x, BLOCK_REF.y, BLOCK_REF.z);
    let red = [0xb8321f, 0xa62c1b, 0xc23a25];
    let white = flat(0xf4efe3);
    // Stone footing and the planked box.
    m.aabb(Vec3::new(-w * 0.5, 0.0, -d * 0.5), Vec3::new(w * 0.5, 0.45, d * 0.5), &flat(0x8a8782));
    // Vertical planks on the three visible faces (front -z, both sides).
    let n = 10;
    for i in 0..n {
        let x0 = -w * 0.5 + w * i as f32 / n as f32;
        let x1 = x0 + w / n as f32;
        m.quad4([Vec3::new(x0, 0.45, -d * 0.5), Vec3::new(x1, 0.45, -d * 0.5), Vec3::new(x1, h - 0.3, -d * 0.5), Vec3::new(x0, h - 0.3, -d * 0.5)], -Vec3::Z, &flat(red[i % 3]));
    }
    let nz = 12;
    for side in [-1.0f32, 1.0] {
        for i in 0..nz {
            let z0 = -d * 0.5 + d * i as f32 / nz as f32;
            let z1 = z0 + d / nz as f32;
            let x = side * w * 0.5;
            m.quad4([Vec3::new(x, 0.45, z0), Vec3::new(x, 0.45, z1), Vec3::new(x, h - 0.3, z1), Vec3::new(x, h - 0.3, z0)], Vec3::X * side, &flat(red[(i + 1) % 3]));
        }
    }
    m.aabb(Vec3::new(-w * 0.5, 0.45, -d * 0.5 + 0.02), Vec3::new(w * 0.5, h - 0.3, d * 0.5), &flat(0xa62c1b));
    // Roof cap: dark slab with a lighter ridge.
    m.aabb(Vec3::new(-w * 0.5 - 0.12, h - 0.3, -d * 0.5 - 0.12), Vec3::new(w * 0.5 + 0.12, h - 0.12, d * 0.5 + 0.12), &flat(0x4d4a55));
    m.aabb(Vec3::new(-w * 0.5 - 0.06, h - 0.12, -d * 0.5 - 0.06), Vec3::new(w * 0.5 + 0.06, h, d * 0.5 + 0.06), &flat(0x6a6672));
    // White trim: posts, rails and the big X.
    let zf = -d * 0.5 - 0.02;
    let tw = 0.14;
    for x in [-w * 0.5 + tw * 0.5, w * 0.5 - tw * 0.5] {
        m.aabb(Vec3::new(x - tw * 0.5, 0.45, zf - 0.04), Vec3::new(x + tw * 0.5, h - 0.3, zf + 0.04), &white);
    }
    m.aabb(Vec3::new(-w * 0.5, 0.45, zf - 0.04), Vec3::new(w * 0.5, 0.45 + tw, zf + 0.04), &white);
    m.aabb(Vec3::new(-w * 0.5, h - 0.3 - tw, zf - 0.04), Vec3::new(w * 0.5, h - 0.3, zf + 0.04), &white);
    let (a, c, e, f) = (Vec3::new(-w * 0.5 + tw, 0.45 + tw, zf - 0.05), Vec3::new(w * 0.5 - tw, h - 0.3 - tw, zf - 0.05), Vec3::new(w * 0.5 - tw, 0.45 + tw, zf - 0.05), Vec3::new(-w * 0.5 + tw, h - 0.3 - tw, zf - 0.05));
    for (p, q) in [(a, c), (e, f)] {
        let mid = (p + q) * 0.5;
        let dirv = q - p;
        let len = dirv.length();
        let x = dirv.normalize();
        let y = Vec3::Z.cross(x).normalize();
        m.cube(&basis(mid, x * len, y * tw, Vec3::Z * 0.06), &white);
    }
    // Side trim bands.
    for side in [-1.0f32, 1.0] {
        let x = side * (w * 0.5 + 0.02);
        m.aabb(Vec3::new(x - 0.03, 0.45, -d * 0.5), Vec3::new(x + 0.03, 0.45 + tw, d * 0.5), &white);
        m.aabb(Vec3::new(x - 0.03, h - 0.3 - tw, -d * 0.5), Vec3::new(x + 0.03, h - 0.3, d * 0.5), &white);
    }
    let mut b = MeshData::new();
    b.append(&m, &s(Vec3::new(1.0 / w, 1.0 / h, 1.0 / d)));
    b
}

/// A green tractor seen from behind (+z is the front): big rear wheels, cab with glass,
/// roof, exhaust, hood and small front wheels.
pub(crate) fn tractor() -> MeshData {
    let mut m = MeshData::new();
    let (w, h, d) = (BLOCK_REF.x, BLOCK_REF.y, BLOCK_REF.z);
    let green = flat(0x3fa23a);
    let green_dark = flat(0x2c7a2c);
    let yellow = flat(0xf7c52b);
    let tyre = flat(0x2a2a2e);
    let glass = Style::rgb(hx(0x8fd3f0)).flat().sheen(0.8);
    let wheel = |m: &mut MeshData, x: f32, z: f32, r: f32, wd: f32, treads: bool| {
        // Axis along x.
        let r0 = r;
        let r = r + 0.05;
        let base = t(Vec3::new(x - wd * 0.5, r, z)) * Mat4::from_rotation_z(-FRAC_PI_2);
        m.cyl(&(base * s(Vec3::new(r0, wd, r0))), 14, 1.0, 1.0, true, &tyre);
        // Tread blocks.
        for i in 0..if treads { 8 } else { 0 } {
            let a = TAU * i as f32 / 8.0;
            let c = Vec3::new(x, r + a.sin() * r0, z + a.cos() * r0);
            m.cube(&basis(c, Vec3::X * wd * 1.02, Vec3::new(0.0, a.sin(), a.cos()) * 0.1, Vec3::new(0.0, -a.cos(), a.sin()) * r0 * 0.5), &flat(0x1a1a1d));
        }
        // Rim and hub on the outside face.
        let side = x.signum();
        let hubx = x + side * (wd * 0.5 + 0.005);
        m.cyl(&(t(Vec3::new(hubx, r, z)) * Mat4::from_rotation_z(-side * FRAC_PI_2) * s(Vec3::new(r0 * 0.55, 0.02, r0 * 0.55))), 14, 1.0, 1.0, true, &yellow);
        m.cyl(&(t(Vec3::new(hubx + side * 0.02, r, z)) * Mat4::from_rotation_z(-side * FRAC_PI_2) * s(Vec3::new(r0 * 0.18, 0.02, r0 * 0.18))), 8, 1.0, 1.0, true, &tyre);
    };
    // Rear wheels: big.
    wheel(&mut m, -0.78, -0.55, 0.92, 0.42, true);
    wheel(&mut m, 0.78, -0.55, 0.92, 0.42, true);
    // Front wheels: small.
    wheel(&mut m, -0.72, 1.15, 0.5, 0.26, false);
    wheel(&mut m, 0.72, 1.15, 0.5, 0.26, false);
    // Chassis and hood.
    m.aabb(Vec3::new(-0.55, 0.5, -1.0), Vec3::new(0.55, 1.05, 0.55), &green.ao(0.25));
    m.aabb(Vec3::new(-0.42, 0.62, 0.35), Vec3::new(0.42, 1.28, 1.5), &green.ao(0.2));
    m.aabb(Vec3::new(-0.36, 1.28, 0.55), Vec3::new(0.36, 1.34, 1.45), &green_dark);
    // Grille and lights.
    m.aabb(Vec3::new(-0.3, 0.72, 1.5), Vec3::new(0.3, 1.1, 1.53), &flat(0x1f3d1f));
    for x in [-0.32f32, 0.32] {
        m.sphere(&(t(Vec3::new(x, 1.12, 1.52)) * s(Vec3::splat(0.09))), 6, 4, &Style::rgb(hx(0xfff3b0)).emit(0.8));
    }
    // Cab: frame, glass, white roof.
    let (cx0, cx1, cz0, cz1) = (-0.55, 0.55, -0.95, 0.25);
    for (x, z) in [(cx0, cz0), (cx1, cz0), (cx0, cz1), (cx1, cz1)] {
        m.aabb(Vec3::new(x - 0.05, 1.05, z - 0.05), Vec3::new(x + 0.05, 2.2, z + 0.05), &green_dark);
    }
    m.aabb(Vec3::new(cx0 + 0.05, 1.12, cz0 - 0.01), Vec3::new(cx1 - 0.05, 2.15, cz0 + 0.01), &glass);
    for sx in [cx0, cx1] {
        m.aabb(Vec3::new(sx - 0.01, 1.12, cz0 + 0.05), Vec3::new(sx + 0.01, 2.15, cz1 - 0.05), &glass);
    }
    m.aabb(Vec3::new(cx0 - 0.12, 2.2, cz0 - 0.12), Vec3::new(cx1 + 0.12, 2.38, cz1 + 0.12), &flat(0xf3f0e6));
    m.aabb(Vec3::new(cx0 - 0.12, 2.38, cz0 - 0.12), Vec3::new(cx1 + 0.12, 2.42, cz1 + 0.12), &flat(0xd8d4c6));
    // Seat and wheel glimpsed through the glass.
    m.aabb(Vec3::new(-0.2, 1.05, -0.6), Vec3::new(0.2, 1.5, -0.3), &flat(0x3a2a22));
    // Exhaust pipe.
    m.cyl(&(t(Vec3::new(0.25, 1.3, 0.85)) * s(Vec3::new(0.05, 1.0, 0.05))), 8, 1.0, 1.0, true, &flat(0x2a2a2a));
    m.cyl(&(t(Vec3::new(0.25, 2.2, 0.85)) * s(Vec3::new(0.07, 0.1, 0.07))), 8, 1.0, 0.9, true, &flat(0x888888));
    // Tow hitch at the back.
    m.aabb(Vec3::new(-0.15, 0.45, -1.35), Vec3::new(0.15, 0.6, -1.0), &flat(0x555555));
    m.cyl(&(t(Vec3::new(0.0, 0.6, -1.3)) * s(Vec3::new(0.09, 0.2, 0.09))), 8, 1.0, 1.0, true, &flat(0xc9c9c9));
    let _ = (w, h, d);
    let mut b = MeshData::new();
    b.append(&m, &s(Vec3::new(1.0 / w, 1.0 / h, 1.0 / d)));
    b
}

/// A big wooden water trough, a hand pump and stacked straw filling the block.
pub(crate) fn trough() -> MeshData {
    let mut m = MeshData::new();
    let (w, h, d) = (BLOCK_REF.x, BLOCK_REF.y, BLOCK_REF.z);
    let wood = flat(0xa87140);
    let wood_dark = flat(0x7a4f2a);
    let iron = flat(0x4a4f57);
    // The trough: a long open box along z with planked walls.
    let (x0, x1, z0, z1, y1) = (-0.9, 0.9, -0.85, 1.45, 1.15);
    m.aabb(Vec3::new(x0, 0.2, z0), Vec3::new(x1, y1, z1), &wood_dark.ao(0.2));
    // Inner water.
    m.quad4([Vec3::new(x0 + 0.14, y1 - 0.12, z0 + 0.14), Vec3::new(x1 - 0.14, y1 - 0.12, z0 + 0.14), Vec3::new(x1 - 0.14, y1 - 0.12, z1 - 0.14), Vec3::new(x0 + 0.14, y1 - 0.12, z1 - 0.14)], Vec3::Y, &Style::rgb(hx(0x4fb3e8)).flat().sheen(0.9));
    // Rim planks (top frame) leaving the water visible.
    for (a, b2) in [((x0, z0), (x1, z0 + 0.14)), ((x0, z1 - 0.14), (x1, z1)), ((x0, z0), (x0 + 0.14, z1)), ((x1 - 0.14, z0), (x1, z1))] {
        m.aabb(Vec3::new(a.0, y1 - 0.1, a.1), Vec3::new(b2.0, y1 + 0.04, b2.1), &wood);
    }
    // Plank lines and iron bands.
    for z in [-0.55f32, 0.3, 1.1] {
        m.aabb(Vec3::new(x0 - 0.03, 0.2, z - 0.07), Vec3::new(x1 + 0.03, y1 + 0.02, z + 0.07), &iron);
    }
    // Legs.
    for (x, z) in [(x0 + 0.1, z0 + 0.1), (x1 - 0.1, z0 + 0.1), (x0 + 0.1, z1 - 0.1), (x1 - 0.1, z1 - 0.1)] {
        m.aabb(Vec3::new(x - 0.1, 0.0, z - 0.1), Vec3::new(x + 0.1, 0.25, z + 0.1), &wood_dark);
    }
    // Pump tower rising to the full height at the front end.
    m.cyl(&(t(Vec3::new(0.0, y1, 1.15)) * s(Vec3::new(0.14, h - y1 - 0.2, 0.14))), 10, 1.0, 0.85, true, &flat(0x2e7fb8));
    m.sphere(&(t(Vec3::new(0.0, h - 0.18, 1.15)) * s(Vec3::new(0.22, 0.2, 0.22))), 10, 6, &flat(0xd33b2b));
    m.cyl(&limb(Vec3::new(0.0, h - 0.5, 1.15), Vec3::new(-0.55, h - 0.8, 1.15), 0.06), 8, 1.0, 1.0, true, &iron);
    m.sphere(&(t(Vec3::new(-0.6, h - 0.84, 1.15)) * s(Vec3::splat(0.1))), 8, 5, &iron);
    // A stack of hay on the near end, a bucket and a rubber duck.
    for (i, (x, y, z)) in [(-0.45f32, 0.0f32, -1.2f32), (0.45, 0.0, -1.2), (0.0, 0.62, -1.2)].into_iter().enumerate() {
        m.aabb(Vec3::new(x - 0.4, y, z - 0.3), Vec3::new(x + 0.4, y + 0.6, z + 0.3), &flat(if i == 2 { 0xf0c85a } else { 0xe3b449 }));
        m.aabb(Vec3::new(x - 0.41, y + 0.25, z - 0.31), Vec3::new(x + 0.41, y + 0.31, z + 0.31), &flat(0xa84a2a));
    }
    m.cyl(&(t(Vec3::new(0.35, 1.2, 0.3)) * s(Vec3::new(0.14, 0.2, 0.14))), 10, 0.8, 1.0, false, &flat(0xb8bcc2));
    let mut b = MeshData::new();
    b.append(&m, &s(Vec3::new(1.0 / w, 1.0 / h, 1.0 / d)));
    b
}

/// A mud puddle decal in the unit square (y a hair above 0): irregular outline, wet sheen,
/// a raised dark rim and bubbles. `size.y` is ignored by the instance matrix.
pub(crate) fn mud() -> MeshData {
    let mut b = MeshData::new();
    let n = 28;
    let outline = |scale: f32, jit: f32| -> Vec<Vec3> {
        (0..n)
            .map(|i| {
                let a = TAU * i as f32 / n as f32;
                let wob = 0.82 + 0.18 * (a * 3.0 + 0.6).sin() + 0.08 * (a * 7.0 + 1.3).sin() + jit * (crate::mesh::hash01(i, 5, 31) - 0.5);
                Vec3::new(a.cos() * 0.5 * wob * scale, 0.0, a.sin() * 0.5 * wob * scale)
            })
            .collect()
    };
    let fan = |b: &mut MeshData, pts: &[Vec3], y: f32, st: &Style| {
        let c = Vec3::new(0.0, y, 0.0);
        for i in 0..pts.len() {
            let a = pts[i] + Vec3::Y * y;
            let c2 = pts[(i + 1) % pts.len()] + Vec3::Y * y;
            b.tri([c, c2, a], Vec3::Y, st);
        }
    };
    // Dirty rim, wet body, glossy middle.
    fan(&mut b, &outline(1.06, 0.2), 0.10, &flat(0x7a5a38));
    fan(&mut b, &outline(1.0, 0.12), 0.108, &flat(0x4a301c));
    fan(&mut b, &outline(0.78, 0.2), 0.114, &Style::rgb(hx(0x5d3b22)).flat().sheen(0.9));
    fan(&mut b, &outline(0.45, 0.3), 0.12, &Style::rgb(hx(0x7a5030)).flat().sheen(1.0));
    // Bubbles and a footprint or two.
    for i in 0..6 {
        let a = i as f32 * 1.7 + 0.5;
        let r = 0.12 + 0.18 * crate::mesh::hash01(i, 2, 40);
        let p = Vec3::new(a.cos() * r, 0.125, a.sin() * r * 0.9);
        b.sphere(&(t(p) * s(Vec3::new(0.025, 0.012, 0.025))), 7, 3, &Style::rgb(hx(0x9a6e45)).sheen(1.0));
    }
    b
}

/// A 5-pointed star facing +z, emissive yellow, for the protection halo. Radius 1 (the pose
/// scales it).
pub(crate) fn star() -> MeshData {
    let mut b = MeshData::new();
    b.star(&Mat4::IDENTITY, 5, 0.46, 0.45, &Style::rgb(hx(0xffd92b)).emit(0.85));
    b.star(&(t(Vec3::new(0.0, 0.0, 0.0)) * s(Vec3::new(0.55, 0.55, 1.3))), 5, 0.46, 0.45, &Style::rgb(hx(0xfff6a8)).emit(0.95));
    b
}

/// One googly eye, looking along +z, radius 1: a white ball and a dark pupil with a highlight.
pub(crate) fn eye() -> MeshData {
    let mut b = MeshData::new();
    b.sphere(&Mat4::IDENTITY, 10, 6, &Style::col(0.98, 0.98, 0.96).sheen(0.5));
    b.sphere(&(t(Vec3::new(0.0, 0.0, 0.58)) * s(Vec3::new(0.48, 0.48, 0.5))), 8, 5, &Style::col(0.05, 0.04, 0.07).sheen(0.7));
    b.sphere(&(t(Vec3::new(0.16, 0.2, 0.95)) * s(Vec3::splat(0.13))), 4, 3, &Style::col(1.0, 1.0, 1.0).emit(0.5));
    b
}

/// An eyelid: a hemisphere shell just over the eye, in tint 0 (the head colour). Closed when
/// it covers the whole eye; the instance rotates it up and down about x.
pub(crate) fn eyelid() -> MeshData {
    let mut b = MeshData::new();
    // Upper half of a slightly bigger sphere: lathe from the side.
    let profile: Vec<(f32, f32)> = (0..=4).map(|i| {
        let a = FRAC_PI_2 * (i as f32 / 4.0);
        (a.cos() * 1.1, a.sin() * 1.1)
    }).collect();
    // Swing the hemisphere so its pole points up (+y) and its rim lies in the y=0 plane.
    b.lathe(&Mat4::IDENTITY, 10, &profile, &Style::tint0(1.0).ao(0.1));
    b
}

/// A cell of the placement grid: a flat unit square (fill, emissive tint 0).
pub(crate) fn cell_fill() -> MeshData {
    let mut b = MeshData::new();
    b.quad4([Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 0.0, -0.5), Vec3::new(0.5, 0.0, 0.5), Vec3::new(-0.5, 0.0, 0.5)], Vec3::Y, &Style::tint0(1.0).emit(0.75));
    b
}

/// The border of a grid cell: a thin frame around the unit square (emissive tint 0).
pub(crate) fn cell_frame() -> MeshData {
    let mut b = MeshData::new();
    let w = 0.06;
    let st = Style::tint0(1.0).emit(0.9).flat();
    for (a, c) in [
        ((-0.5, -0.5), (0.5, -0.5 + w)),
        ((-0.5, 0.5 - w), (0.5, 0.5)),
        ((-0.5, -0.5 + w), (-0.5 + w, 0.5 - w)),
        ((0.5 - w, -0.5 + w), (0.5, 0.5 - w)),
    ] {
        b.quad4([Vec3::new(a.0, 0.0, a.1), Vec3::new(c.0, 0.0, a.1), Vec3::new(c.0, 0.0, c.1), Vec3::new(a.0, 0.0, c.1)], Vec3::Y, &st);
    }
    b
}

/// Windmill wheel facing +z (spin about z), hub at the origin, radius ~2.4 m.
pub(crate) fn windmill_wheel() -> MeshData {
    let mut b = MeshData::new();
    let wood = flat(0xf1ede0);
    let red = flat(0xc8402a);
    let iron = flat(0x5a5e66);
    let blades = 18;
    for i in 0..blades {
        let a = TAU * i as f32 / blades as f32;
        let (sa, ca) = a.sin_cos();
        let dir = Vec3::new(ca, sa, 0.0);
        let side = Vec3::new(-sa, ca, 0.0);
        let r0 = 0.45;
        let r1 = 2.4;
        // A blade: a flat paddle angled like a vane.
        let tilt = Vec3::Z * 0.22;
        let p = [dir * r0 - side * 0.11, dir * r0 + side * 0.11 + tilt * 0.4, dir * r1 + side * 0.17 + tilt, dir * r1 - side * 0.17 - tilt * 0.2];
        b.quad4(p, Vec3::Z, if i % 2 == 0 { &wood } else { &red });
        b.quad4([p[0], p[1], p[2], p[3]], -Vec3::Z, if i % 2 == 0 { &wood } else { &red });
    }
    for r in [1.35f32, 2.4] {
        b.torus(&(Mat4::from_rotation_x(FRAC_PI_2) * t(Vec3::ZERO)), 24, 4, r, 0.035, &iron);
    }
    b.sphere(&s(Vec3::splat(0.3)), 10, 6, &red);
    b.cyl(&(Mat4::from_rotation_x(FRAC_PI_2) * s(Vec3::new(0.12, 0.5, 0.12))), 8, 1.0, 1.0, true, &iron);
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(m: &MeshData) -> (Vec3, Vec3) {
        m.bounds()
    }

    #[test]
    fn unit_meshes_fit_the_unit_box() {
        for (name, m, ylim) in [
            ("hay", hay_bale(), 1.13),
            ("barn", block_barn(), 1.02),
            ("tractor", tractor(), 1.02),
            ("trough", trough(), 1.02),
        ] {
            let (lo, hi) = bounds(&m);
            assert!(lo.y >= -0.02, "{name} below ground: {lo:?}");
            assert!(hi.y <= ylim + 0.05, "{name} too tall: {hi:?}");
            assert!(lo.x >= -0.62 && hi.x <= 0.62, "{name} x: {lo:?} {hi:?}");
            assert!(lo.z >= -0.72 && hi.z <= 0.72, "{name} z: {lo:?} {hi:?}");
            assert!(m.tri_count() < 1800, "{name}: {} tris", m.tri_count());
        }
    }

    /// World-space bounds of every vertex of a prop's draws.
    fn world_bounds(fp: &FarmPropInst) -> Vec<(crate::meshes::MeshId, Vec3, Vec3)> {
        let mut out = Vec::new();
        for d in std::iter::once(prop_draw(fp)).chain(prop_extras(fp).into_iter().flatten()) {
            let m = crate::meshes::build_mesh(d.mesh);
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for v in &m.verts {
                let p = d.model.transform_point3(Vec3::from_array(v.pos));
                lo = lo.min(p);
                hi = hi.max(p);
            }
            out.push((d.mesh, lo, hi));
        }
        out
    }

    #[test]
    fn gate_posts_are_tall_and_the_bar_sits_between_the_sim_underside_and_top() {
        for lanes in [1u8, 3] {
            for (under, top) in [(1.0f32, 1.6f32), (1.15, 1.75), (1.15, 1.95)] {
                let mut fp = FarmPropInst::new(FarmPropKind::Beam { span_lanes: lanes }, Vec3::new(0.0, 0.0, 20.0), Vec3::new(if lanes == 3 { 7.4 } else { 2.0 }, under, 0.4));
                fp.bar_top = top;
                let parts = world_bounds(&fp);
                let (_, flo, fhi) = parts[0];
                let (_, blo, bhi) = parts[2];
                let (_, _, bar_hi) = parts[1];
                assert!(flo.y.abs() < 0.001, "posts reach the ground");
                assert!(fhi.y > 2.4 && fhi.y < 3.4, "tall posts: {}", fhi.y);
                assert!(fhi.y > top + 0.4, "posts stand clear above the bar top {top}: {}", fhi.y);
                // The bar's body is exactly underside..top; only bunting and sacks hang below.
                assert!((bar_hi.y - top).abs() < 0.01, "bar top {} vs sim top {top}", bar_hi.y);
                assert!(blo.y > under - 0.5 && blo.y < under - 0.2, "bunting hangs {}", under - blo.y);
                let _ = bhi;
                // Nothing of the frame in the lane gap below the underside except the posts.
                let m = crate::meshes::build_mesh(parts[0].0);
                let d = prop_draw(&fp);
                for v in &m.verts {
                    let p = d.model.transform_point3(Vec3::from_array(v.pos));
                    if p.y < under - 0.001 {
                        assert!(p.x.abs() > fp.size.x * 0.5 - 0.6, "frame geometry in the gap at {p:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_barrel_sits_on_the_ground_and_rolls_about_its_axis() {
        let mut fp = FarmPropInst::new(FarmPropKind::Barrel, Vec3::new(1.0, 0.0, 20.0), Vec3::new(0.9, 1.0, 1.0));
        for roll in [0.0f32, 0.7, 2.0, -1.3] {
            fp.roll = roll;
            let (_, lo, hi) = world_bounds(&fp)[0];
            assert!(lo.y > -0.06 && lo.y < 0.06, "roll {roll}: base {}", lo.y);
            assert!(hi.y < 1.06, "roll {roll}: top {}", hi.y);
            assert!(((lo.x + hi.x) * 0.5 - 1.0).abs() < 0.02, "rolls in place across x");
        }
    }

    fn inst(kind: FarmPropKind, size: Vec3) -> FarmPropInst {
        FarmPropInst::new(kind, Vec3::new(1.0, 0.0, 20.0), size)
    }

    #[test]
    fn landed_props_sit_on_the_ground_at_the_sim_size() {
        for (kind, size) in [
            (FarmPropKind::HayBale, Vec3::new(1.6, 0.7, 1.0)),
            (FarmPropKind::Beam { span_lanes: 3 }, Vec3::new(7.4, 1.15, 0.4)),
            (FarmPropKind::Barrel, Vec3::new(0.9, 1.0, 1.0)),
            (FarmPropKind::Block { variant: BlockVariant::Tractor }, Vec3::new(2.0, 2.5, 3.0)),
        ] {
            let fp = inst(kind, size);
            let d = prop_draw(&fp);
            let m = crate::meshes::build_mesh(d.mesh);
            let mut lo = Vec3::splat(f32::MAX);
            let mut hi = Vec3::splat(f32::MIN);
            for v in &m.verts {
                let p = d.model.transform_point3(Vec3::from_array(v.pos));
                lo = lo.min(p);
                hi = hi.max(p);
            }
            assert!(lo.y.abs() < 0.03, "{kind:?} base {}", lo.y);
            assert!((lo.x + hi.x) * 0.5 - fp.pos.x < 0.05, "{kind:?} centred in x");
            assert!(hi.x - lo.x < size.x * 1.1 + 0.05, "{kind:?} width {}", hi.x - lo.x);
            assert!(d.alpha > 0.999);
        }
    }

    #[test]
    fn the_architect_drop_falls_from_twelve_metres_and_slams_down_at_impact() {
        let mut fp = inst(FarmPropKind::Block { variant: BlockVariant::BarnWall }, BLOCK_REF);
        let base_y = |fp: &FarmPropInst| prop_draw(fp).model.transform_point3(Vec3::ZERO).y;
        fp.drop = 0.0;
        assert!((base_y(&fp) - DROP_HEIGHT).abs() < 1e-3, "starts {DROP_HEIGHT} m up: {}", base_y(&fp));
        assert!(prop_draw(&fp).alpha < 0.01, "and invisible");
        fp.drop = 0.5;
        let mid = base_y(&fp);
        assert!(mid > 3.0 && mid < 11.0, "falling: {mid}");
        fp.drop = 0.99;
        let late = base_y(&fp);
        assert!(late < 0.3 * DROP_HEIGHT / 10.0 * 1.0 + 0.1, "slamming in fast at the end: {late}");
        // impact: fully squashed (shorter and wider), on the ground, then it springs back
        fp.drop = 1.0;
        fp.settle = 0.0;
        let sy = prop_draw(&fp).model.y_axis.length();
        let sx = prop_draw(&fp).model.x_axis.length();
        assert!(sy < BLOCK_REF.y * 0.75 && sx > BLOCK_REF.x * 1.15, "squash {sx} {sy}");
        assert!(base_y(&fp).abs() < 1e-4, "stays on the ground while squashed");
        fp.settle = 0.3;
        assert!(prop_draw(&fp).model.y_axis.length() > BLOCK_REF.y, "overshoots into a stretch");
        fp.settle = 1.0;
        assert!((prop_draw(&fp).model.y_axis.length() - BLOCK_REF.y).abs() < 1e-5);
    }

    #[test]
    fn placer_marks_exist_for_hay_and_gates_only_and_the_banner_sits_between_bar_and_rail() {
        let tint = Some([0.9, 0.3, 0.4]);
        let mut hay = inst(FarmPropKind::HayBale, HAY_REF);
        assert!(placer_mark(&hay).is_none(), "unmarked without a placer");
        hay.placer = tint;
        assert!(placer_mark(&hay).is_some());
        let mut mud = inst(FarmPropKind::Mud, Vec3::new(2.0, 0.1, 6.0));
        mud.placer = tint;
        assert!(placer_mark(&mud).is_none());
        for lanes in [1u8, 3] {
            let mut gate = inst(FarmPropKind::Beam { span_lanes: lanes }, Vec3::new(if lanes == 3 { 7.4 } else { 2.0 }, 1.15, 0.4));
            gate.bar_top = 1.75;
            gate.placer = tint;
            let (w, h, y0) = banner_dims(&gate);
            assert!(y0 >= bar_top(&gate) && y0 + h <= GATE_RAIL_LO + 0.001, "banner {y0}..{} between the bar top and the rail", y0 + h);
            assert!(w > 0.8 && w < gate.size.x, "banner width {w}");
            let (c, sz) = banner_anchor(&gate).unwrap();
            assert!((c.y - (y0 + h * 0.5)).abs() < 1e-4 && (c.x - gate.pos.x).abs() < 1e-4 && sz.x == w);
        }
    }

    #[test]
    fn mud_keeps_its_own_thickness() {
        let fp = inst(FarmPropKind::Mud, Vec3::new(2.0, 50.0, 6.0));
        let d = prop_draw(&fp);
        assert!((d.model.y_axis.length() - 1.0).abs() < 1e-5, "size.y is ignored for mud");
        let m = crate::meshes::build_mesh(d.mesh);
        let hi = m.verts.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
        assert!(hi < 0.2 && hi > 0.05, "a thin decal: {hi}");
    }
}
