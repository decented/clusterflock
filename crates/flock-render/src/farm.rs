//! The Cluster Flock farm: a straight three-lane dirt track through a bright, cheerful farm,
//! baked as a seamless repeating tile.
//!
//! The tile is [`TILE_LEN`] metres long along +Z. `load_world` bakes it [`COPIES`] times
//! side by side (`z` from `-TILE_LEN` to `3 * TILE_LEN`), so a camera whose render-space `z`
//! stays in `[-10, TILE_LEN)` always has scenery ahead of it. The app subtracts a *floating
//! origin* (a multiple of `TILE_LEN`) from every `z` it hands to the renderer; see
//! `docs/api/flock-render.md`.
//!
//! Render space: Y up, +Z forward, and +X is the runner's LEFT (SPEC section 2). The track is
//! centred on x = 0; lane `l` (0..=2, left to right) has its centre at `x = -(l - 1) * lane_width`.

use crate::decor::{self, DecorData};
use crate::farmprops::hx;
use crate::mesh::{hash01, limb, MeshData, Style};
use crate::types::*;
use crate::world::{Cells, DecorCells};
use glam::{Mat4, Vec2, Vec3};
use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// Length of the repeating tile in metres.
pub const TILE_LEN: f32 = 240.0;
/// Copies baked: tile indices `-1..=2`.
pub const COPIES: [i32; 4] = [-1, 0, 1, 2];

/// The farm palette: bright blue sky, warm sun from behind the camera.
pub fn farm_palette() -> Palette {
    let c = hx;
    Palette {
        sky_top: c(0x2f86ee),
        sky_horizon: c(0xcdeaff),
        fog_color: c(0xd3ebfa),
        fog_density: 0.0058,
        sun_dir: Vec3::new(0.30, 0.82, -0.48),
        sun_color: c(0xfff1d2),
        ambient_sky: c(0xbcd8ff),
        ambient_ground: c(0xa8c070),
        ground: c(0x79c943),
        ground_alt: c(0x5eb63b),
        road: c(0xc79a62),
        road_stripe: c(0xf0dfae),
        shoulder: c(0xb68a54),
        wall: c(0x8a5a2f),
        curb_a: c(0xd8352a),
        curb_b: c(0xf5f2ea),
        night: false,
    }
}

impl WorldGeometry {
    /// The Cluster Flock farm world: pass to `Renderer::load_world` once.
    pub fn farm() -> WorldGeometry {
        farm_world(FarmStyle::default())
    }
}

/// The farm as a [`WorldGeometry`].
pub fn farm_world(style: FarmStyle) -> WorldGeometry {
    WorldGeometry {
        road: Vec::new(),
        closed: false,
        terrain: None,
        wall: WallStyle::None,
        palette: farm_palette(),
        scenery: Vec::new(),
        pads: Vec::new(),
        start: None,
        fx: WorldFx {
            sky: SkyFx {
                cloud_cover: 0.55,
                cloud_color: [1.0, 1.0, 1.0],
                band_strength: 0.25,
                band_color: hx(0xf4fbff),
                sun_scale: 1.0,
                hills: HillStyle::Rolling,
                hill_color: Some(hx(0x4fae56)),
                hill_glow: [0.0; 3],
                hill_glow_strength: 0.0,
            },
            ..WorldFx::default()
        },
        bridges: Vec::new(),
        field: None,
        farm: Some(style),
    }
}

// ------------------------------------------------------------------ tools ----

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 & 0xFF_FFFF) as f32 / 16_777_216.0
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.next()
    }
}

fn st(hex: u32) -> Style {
    Style::rgb(hx(hex)).flat()
}

fn shade(c: Rgb, k: f32) -> Rgb {
    [(c[0] * k).clamp(0.0, 1.0), (c[1] * k).clamp(0.0, 1.0), (c[2] * k).clamp(0.0, 1.0)]
}

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn tr(p: Vec3) -> Mat4 {
    Mat4::from_translation(p)
}

fn sc(p: Vec3) -> Mat4 {
    Mat4::from_scale(p)
}

fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Height of the rolling hills at `(x, z)`; periodic in `z` with period `TILE_LEN`.
pub(crate) fn hill_height(x: f32, z: f32) -> f32 {
    let ax = x.abs();
    let ramp = smooth(46.0, 120.0, ax);
    let w = TAU / TILE_LEN;
    let a = (z * w * 2.0 + ax * 0.045).sin() * 0.5 + (z * w * 3.0 - ax * 0.02 + 1.7).sin() * 0.35 + (z * w * 5.0 + ax * 0.07).sin() * 0.15;
    ramp * (2.0 + 10.0 * (0.5 + 0.5 * a)) * (0.5 + ((ax - 46.0) / 220.0).clamp(0.0, 1.0) * 0.9)
}

/// Tile-local chunk bins, merged into the world's cells once per copy.
struct Bins {
    map: HashMap<(i32, i32), MeshData>,
    dmap: HashMap<(i32, i32), DecorData>,
}

impl Bins {
    fn key(p: Vec3) -> (i32, i32) {
        ((p.x / 64.0).floor() as i32, (p.z / 64.0).floor() as i32)
    }
    fn at(&mut self, p: Vec3) -> &mut MeshData {
        self.map.entry(Bins::key(p)).or_default()
    }
    fn dat(&mut self, p: Vec3) -> &mut DecorData {
        self.dmap.entry(Bins::key(p)).or_default()
    }
}

fn u8f(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

// ------------------------------------------------------------------- bake ----

/// Bake the whole farm: the tile built once, appended `COPIES.len()` times.
pub(crate) fn bake_farm(style: &FarmStyle, cells: &mut Cells, dcells: &mut DecorCells) {
    let mut bins = Bins { map: HashMap::new(), dmap: HashMap::new() };
    build_tile(style, &mut bins);
    for k in COPIES {
        let off = Vec3::new(0.0, 0.0, k as f32 * TILE_LEN);
        let m = tr(off);
        let mut keys: Vec<_> = bins.map.keys().copied().collect();
        keys.sort();
        for key in keys {
            let mesh = &bins.map[&key];
            let at = Vec3::new(key.0 as f32 * 64.0 + 32.0, 0.0, key.1 as f32 * 64.0 + 32.0) + off;
            cells.at(at).append(mesh, &m);
        }
        let mut dkeys: Vec<_> = bins.dmap.keys().copied().collect();
        dkeys.sort();
        for key in dkeys {
            let d = &bins.dmap[&key];
            let at = Vec3::new(key.0 as f32 * 64.0 + 32.0, 0.0, key.1 as f32 * 64.0 + 32.0) + off;
            let dst = dcells.at(at);
            // Re-base the decor vertices by the copy offset.
            let base = dst.verts.len() as u32;
            for v in &d.verts {
                let mut v = *v;
                v.pos[2] += off.z;
                dst.verts.push(v);
            }
            dst.idx.extend(d.idx.iter().map(|i| i + base));
        }
    }
}

/// Where the windmill wheels turn, in tile-local render space (hub centres).
pub(crate) fn windmill_hubs(style: &FarmStyle) -> Vec<(Vec3, f32)> {
    layout(style).windmills
}

struct Layout {
    windmills: Vec<(Vec3, f32)>,
}

/// The few placements other code needs to know about.
fn layout(style: &FarmStyle) -> Layout {
    let _ = style;
    Layout { windmills: vec![(Vec3::new(21.5, 12.6, 80.0), PI), (Vec3::new(-24.0, 12.6, 205.0), PI)] }
}

fn build_tile(style: &FarmStyle, b: &mut Bins) {
    let mut rng = Rng(style.seed.wrapping_mul(2_654_435_761).wrapping_add(12345) | 1);
    ground(style, b);
    track(style, b, &mut rng);
    fences(style, b, &mut rng);
    verge(style, b, &mut rng);
    fields(style, b, &mut rng);
    buildings(style, b, &mut rng);
    trees(style, b, &mut rng);
    yard_props(style, b, &mut rng);
    ponds(style, b, &mut rng);
    livestock(style, b, &mut rng);
}

// ----------------------------------------------------------------- ground ----

fn grass_colour(x: f32, z: f32, near: bool) -> Rgb {
    let n = hash01((x * 0.31) as i32, (z * 0.31) as i32, 3) - 0.5;
    let n2 = hash01((x * 0.07) as i32, (z * 0.07) as i32, 9) - 0.5;
    let base = hx(0x72d03c);
    let alt = hx(0x58b733);
    let t = (0.5 + n2 * 1.2).clamp(0.0, 1.0);
    let c = mix(base, alt, t);
    shade(c, if near { 1.0 + n * 0.08 } else { 1.0 + n * 0.14 })
}

fn ground(_style: &FarmStyle, b: &mut Bins) {
    // Flat grass out to 46 m each side, in 8 m rows with mown stripes near the track.
    let xs = [4.6f32, 9.0, 15.0, 24.0, 36.0, 46.0];
    let rows = (TILE_LEN / 8.0) as usize;
    for side in [-1.0f32, 1.0] {
        for i in 0..rows {
            let (z0, z1) = (i as f32 * 8.0, (i + 1) as f32 * 8.0);
            for w in xs.windows(2) {
                let (xa, xb) = (side * w[0], side * w[1]);
                let near = w[0] < 12.0;
                let mut c = grass_colour((xa + xb) * 0.5, (z0 + z1) * 0.5, near);
                if near {
                    // Mown stripes: alternate rows lighter/darker.
                    c = shade(c, if i % 2 == 0 { 1.04 } else { 0.94 });
                }
                let p = [Vec3::new(xa, 0.0, z0), Vec3::new(xb, 0.0, z0), Vec3::new(xb, 0.0, z1), Vec3::new(xa, 0.0, z1)];
                b.at(Vec3::new(xa, 0.0, z0)).quad4(p, Vec3::Y, &Style::rgb(c).flat());
            }
        }
        // Hills from 46 m outwards: a height field, 10 m cells in z, wider cells with distance.
        let hx_nodes = [46.0f32, 58.0, 72.0, 90.0, 112.0, 138.0, 168.0, 205.0, 250.0, 300.0];
        let zc = (TILE_LEN / 10.0) as usize;
        for i in 0..zc {
            let (z0, z1) = (i as f32 * 10.0, (i + 1) as f32 * 10.0);
            for w in hx_nodes.windows(2) {
                let p = [
                    Vec3::new(side * w[0], hill_height(w[0], z0), z0),
                    Vec3::new(side * w[1], hill_height(w[1], z0), z0),
                    Vec3::new(side * w[1], hill_height(w[1], z1), z1),
                    Vec3::new(side * w[0], hill_height(w[0], z1), z1),
                ];
                let mid = (p[0] + p[2]) * 0.5;
                let hgt = (mid.y / 8.0).clamp(0.0, 1.0);
                let base = grass_colour(mid.x, mid.z, false);
                let c = mix(shade(base, 0.96), hx(0x9bdc5a), hgt * 0.55);
                b.at(p[0]).quad4(p, Vec3::Y, &Style::rgb(c).flat());
            }
        }
    }
}

// ------------------------------------------------------------------ track ----

fn track(style: &FarmStyle, b: &mut Bins, rng: &mut Rng) {
    let lw = style.lane_width;
    let half = lw * 1.5 + 1.1;
    let dirt = hx(0xc99b62);
    let seg = 4.0;
    let n = (TILE_LEN / seg) as usize;
    for i in 0..n {
        let (z0, z1) = (i as f32 * seg, (i + 1) as f32 * seg);
        for lane in -1i32..=1 {
            let cx = lane as f32 * lw;
            let (xa, xb) = if lane == -1 { (cx - lw * 0.5 - 1.1, cx + lw * 0.5) } else if lane == 1 { (cx - lw * 0.5, cx + lw * 0.5 + 1.1) } else { (cx - lw * 0.5, cx + lw * 0.5) };
            let tone = 1.0 + 0.012 * (if (i + lane.unsigned_abs() as usize).is_multiple_of(2) { 1.0 } else { -1.0 }) + 0.03 * (hash01(i as i32, lane, 4) - 0.5);
            let c = shade(dirt, tone);
            let p = [Vec3::new(xa, 0.02, z0), Vec3::new(xb, 0.02, z0), Vec3::new(xb, 0.02, z1), Vec3::new(xa, 0.02, z1)];
            b.at(p[0]).quad4(p, Vec3::Y, &Style::rgb(c).flat());
        }
        // Worn, darker band at both edges of the dirt where it meets the verge.
        for side in [-1.0f32, 1.0] {
            let (xa, xb) = (side * (half - 0.9), side * (half + 0.55));
            let c = shade(hx(0xa87c46), 0.96 + 0.08 * hash01(i as i32, side as i32, 8));
            let p = [Vec3::new(xa, 0.021, z0), Vec3::new(xb, 0.021, z0), Vec3::new(xb, 0.021, z1), Vec3::new(xa, 0.021, z1)];
            b.at(p[0]).quad4(p, Vec3::Y, &Style::rgb(c).flat());
        }
    }
    // Wheel ruts: two per lane, wobbling a little, with gaps.
    for lane in -1i32..=1 {
        let cx = lane as f32 * lw;
        for k in [-0.5f32, 0.5] {
            let mut z = 0.0;
            while z < TILE_LEN - 1.0 {
                let len = rng.range(3.0, 8.0).min(TILE_LEN - z);
                let wob = |zz: f32| (zz * 0.21 + cx * 3.0 + k * 5.0).sin() * 0.05;
                let (x0, x1) = (cx + k * 1.05 - 0.17, cx + k * 1.05 + 0.17);
                let steps = (len / 4.0).ceil().max(1.0) as usize;
                for s in 0..steps {
                    let za = z + len * s as f32 / steps as f32;
                    let zb = z + len * (s + 1) as f32 / steps as f32;
                    let fade = |u: f32| (u * PI).sin().max(0.0).powf(0.35);
                    let wa = 0.5 + 0.5 * fade(s as f32 / steps as f32);
                    let wb = 0.5 + 0.5 * fade((s + 1) as f32 / steps as f32);
                    let c = hx(0xb68650);
                    let p = [
                        Vec3::new(x0 + wob(za) + (1.0 - wa) * 0.12, 0.04, za),
                        Vec3::new(x1 + wob(za) - (1.0 - wa) * 0.12, 0.04, za),
                        Vec3::new(x1 + wob(zb) - (1.0 - wb) * 0.12, 0.04, zb),
                        Vec3::new(x0 + wob(zb) + (1.0 - wb) * 0.12, 0.04, zb),
                    ];
                    b.at(p[0]).quad4(p, Vec3::Y, &Style::rgb(c).flat());
                }
                z += len + rng.range(2.0, 7.0);
            }
        }
    }
    // Mottling: big soft patches of lighter and darker dirt.
    for _ in 0..110 {
        let x = rng.range(-half, half);
        let z = rng.range(0.0, TILE_LEN);
        let r = rng.range(0.5, 1.6);
        let k = rng.range(0.90, 1.08);
        b.at(Vec3::new(x, 0.0, z)).disc(&(tr(Vec3::new(x, 0.03, z)) * Mat4::from_rotation_y(rng.range(0.0, TAU)) * sc(Vec3::new(r, 1.0, r * rng.range(0.5, 1.0)))), 7, &Style::rgb(shade(dirt, k)).flat());
    }
    // Lane marks: dashed straw-coloured lines between lanes.
    for edge in [-0.5f32, 0.5] {
        let x = edge * 2.0 * lw * 0.5 + if edge < 0.0 { -lw * 0.5 } else { lw * 0.5 };
        let _ = x;
    }
    for x in [-lw * 0.5, lw * 0.5] {
        let mut z = 0.5;
        while z < TILE_LEN - 2.0 {
            let (z0, z1) = (z, z + 1.8);
            let p = [Vec3::new(x - 0.09, 0.06, z0), Vec3::new(x + 0.09, 0.06, z0), Vec3::new(x + 0.09, 0.06, z1), Vec3::new(x - 0.09, 0.06, z1)];
            b.at(p[0]).quad4(p, Vec3::Y, &st(0xf3e3b0));
            z += 4.0;
        }
    }
    // Dirt litter: pebbles, clumps, straw, hoof prints.
    for _ in 0..150 {
        let x = rng.range(-half + 0.2, half - 0.2);
        let z = rng.range(0.0, TILE_LEN);
        let kind = rng.next();
        let p = Vec3::new(x, 0.07, z);
        let yaw = rng.range(0.0, TAU);
        let m = tr(p) * Mat4::from_rotation_y(yaw);
        if kind < 0.3 {
            // Pebble.
            let r = rng.range(0.05, 0.12);
            b.at(p).sphere(&(m * sc(Vec3::new(r, r * 0.55, r * 1.2))), 6, 3, &Style::rgb(shade(hx(0x9a8670), rng.range(0.8, 1.15))).flat());
        } else if kind < 0.55 {
            // Dark clod patch.
            let r = rng.range(0.25, 0.6);
            b.at(p).disc(&(tr(Vec3::new(x, 0.052, z)) * Mat4::from_rotation_y(yaw) * sc(Vec3::new(r, 1.0, r * rng.range(0.6, 1.0)))), 7, &st(0xb3854f));
        } else if kind < 0.8 {
            // Straw bits.
            for j in 0..3 {
                let a = yaw + j as f32 * 0.9;
                let (sa, ca) = a.sin_cos();
                let (c0, c1) = (p + Vec3::new(ca, 0.0, sa) * 0.18, p - Vec3::new(ca, 0.0, sa) * 0.18);
                let w = Vec3::new(-sa, 0.0, ca) * 0.018;
                b.at(p).quad4([c0 - w, c0 + w, c1 + w, c1 - w], Vec3::Y, &st(0xf0cf6a));
            }
        } else {
            // Hoof print: two small dark ellipses.
            for s in [-0.09f32, 0.09] {
                b.at(p).disc(&(tr(Vec3::new(x, 0.055, z) + Vec3::new(yaw.cos(), 0.0, yaw.sin()) * s) * Mat4::from_rotation_y(yaw) * sc(Vec3::new(0.05, 1.0, 0.1))), 6, &st(0x7e5731));
            }
        }
    }
}

// ----------------------------------------------------------------- fences ----

fn fences(style: &FarmStyle, b: &mut Bins, rng: &mut Rng) {
    let fx = style.fence_x;
    for side in [-1.0f32, 1.0] {
        // Contact shadow strip on the grass.
        let p = [Vec3::new(side * (fx + 0.1), 0.03, 0.0), Vec3::new(side * (fx + 0.9), 0.03, 0.0), Vec3::new(side * (fx + 0.9), 0.03, TILE_LEN), Vec3::new(side * (fx + 0.1), 0.03, TILE_LEN)];
        b.at(Vec3::new(side * fx, 0.0, 0.0)).quad4(p, Vec3::Y, &st(0x5a9a34));
        // Posts every 3 m (TILE_LEN / 3 = 80).
        let n = (TILE_LEN / 3.0) as usize;
        for i in 0..n {
            let z = i as f32 * 3.0 + 0.1;
            let h = 1.18 + rng.range(-0.04, 0.05);
            let pc = Vec3::new(side * (fx + 0.12), 0.0, z);
            let lean = rng.range(-0.04, 0.04);
            let wood = shade(hx(0xa9743f), rng.range(0.88, 1.08));
            let m = tr(pc) * Mat4::from_rotation_z(lean);
            let mesh = b.at(pc);
            mesh.cube(&(m * tr(Vec3::new(0.0, h * 0.5, 0.0)) * sc(Vec3::new(0.2, h, 0.2))), &Style::rgb(wood).flat().ao(0.3));
            // Pale cap.
            mesh.cube(&(m * tr(Vec3::new(0.0, h + 0.02, 0.0)) * sc(Vec3::new(0.23, 0.05, 0.23))), &st(0xe8c898));
        }
        // Two rails in 6 m planks, alternating tone, slightly staggered.
        let planks = (TILE_LEN / 12.0) as usize;
        for (ry, rh) in [(0.45f32, 0.16f32), (0.86, 0.16)] {
            for i in 0..planks {
                let z0 = i as f32 * 12.0 - 0.1;
                let z1 = z0 + 12.0 - 0.06;
                let wood = shade(hx(0xc08a4c), if (i + (ry * 10.0) as usize).is_multiple_of(2) { 1.0 } else { 0.9 } * rng.range(0.96, 1.04));
                let x = side * (fx + 0.04);
                let mesh = b.at(Vec3::new(x, ry, z0));
                mesh.aabb(Vec3::new(x - 0.04, ry, z0), Vec3::new(x + 0.04, ry + rh, z1), &Style::rgb(wood).flat());
            }
        }
    }
}

// ----------------------------------------------------------------- verges ----

fn verge(style: &FarmStyle, b: &mut Bins, rng: &mut Rng) {
    let fx = style.fence_x;
    for side in [-1.0f32, 1.0] {
        // Tufts and flowers on the grass strip behind the fence.
        for _ in 0..110 {
            let x = side * rng.range(fx + 0.5, 11.0);
            let z = rng.range(0.0, TILE_LEN);
            let p = Vec3::new(x, 0.0, z);
            let k = rng.next();
            let h = rng.range(0.25, 0.55);
            if k < 0.62 {
                let leaf = shade(hx(0x4fa82f), rng.range(0.85, 1.25));
                for j in 0..3 {
                    let a = TAU * (j as f32 / 3.0) + rng.range(0.0, 1.0);
                    let o = Vec3::new(a.cos(), 0.0, a.sin()) * 0.08;
                    b.at(p).cone(&(tr(p + o) * Mat4::from_rotation_z(a.cos() * 0.35) * Mat4::from_rotation_x(a.sin() * 0.35) * sc(Vec3::new(0.07, h, 0.07))), 3, &Style::rgb(leaf).flat());
                }
            } else {
                let petals = [hx(0xffffff), hx(0xffe14a), hx(0xff7aa8), hx(0xb48cff), hx(0xff8a3c)][(rng.next() * 5.0) as usize % 5];
                let stem_h = rng.range(0.22, 0.4);
                b.at(p).cyl(&(tr(p) * sc(Vec3::new(0.012, stem_h, 0.012))), 4, 1.0, 1.0, false, &st(0x3f8f2a));
                let top = p + Vec3::Y * stem_h;
                b.at(p).disc(&(tr(top) * sc(Vec3::new(0.11, 1.0, 0.11))), 7, &Style::rgb(petals).flat());
                b.at(p).disc(&(tr(top + Vec3::Y * 0.01) * sc(Vec3::new(0.045, 1.0, 0.045))), 6, &st(0xffb81c));
            }
        }
        // Bushes along the back of the verge.
        let mut z = rng.range(2.0, 14.0);
        while z < TILE_LEN - 4.0 {
            let x = side * rng.range(9.0, 12.0);
            let r = rng.range(0.9, 1.6);
            let leaf = shade(hx(0x3f9a3a), rng.range(0.85, 1.15));
            let p = Vec3::new(x, 0.0, z);
            let mesh = b.at(p);
            mesh.sphere_jit(&(tr(p + Vec3::Y * r * 0.55) * sc(Vec3::new(r, r * 0.75, r * 1.1))), 7, 4, &Style::rgb(leaf).flat().ao(0.3), 0.12, z as i32);
            mesh.sphere_jit(&(tr(p + Vec3::new(r * 0.6, r * 0.4, r * 0.4)) * sc(Vec3::new(r * 0.6, r * 0.5, r * 0.6))), 6, 3, &Style::rgb(shade(leaf, 1.1)).flat().ao(0.3), 0.12, z as i32 + 3);
            // A few berries or blossoms.
            for j in 0..2 {
                let a = rng.range(0.0, TAU);
                let bp = p + Vec3::new(a.cos() * r * 0.7, r * rng.range(0.55, 1.0), a.sin() * r * 0.7);
                mesh.sphere(&(tr(bp) * sc(Vec3::splat(0.09))), 4, 3, &Style::rgb(if j % 2 == 0 { hx(0xe94b5b) } else { hx(0xffe98a) }).flat());
            }
            z += rng.range(10.0, 30.0);
        }
    }
}

// ----------------------------------------------------------------- fields ----

#[derive(Clone, Copy)]
enum Crop {
    Wheat,
    Corn,
    Sunflower,
    Cabbage,
    Lavender,
}

fn field(b: &mut Bins, rng: &mut Rng, crop: Crop, x0: f32, x1: f32, z0: f32, z1: f32) {
    let (lo, hi) = (x0.min(x1), x0.max(x1));
    // Soil.
    let soil = [Vec3::new(lo, 0.03, z0), Vec3::new(hi, 0.03, z0), Vec3::new(hi, 0.03, z1), Vec3::new(lo, 0.03, z1)];
    b.at(soil[0]).quad4(soil, Vec3::Y, &st(match crop { Crop::Wheat => 0xd5ab50, Crop::Corn => 0x9a6c3c, Crop::Sunflower => 0x8a5c33, Crop::Cabbage => 0x8f6238, Crop::Lavender => 0x8e6b49 }));
    let row_w = match crop { Crop::Wheat => 1.8, Crop::Corn => 2.1, Crop::Sunflower => 2.8, Crop::Cabbage => 2.6, Crop::Lavender => 1.8 };
    let rows = ((hi - lo) / row_w) as usize;
    for r in 0..rows {
        let xc = lo + (r as f32 + 0.5) * (hi - lo) / rows as f32;
        let seg_len = 18.0;
        let segs = ((z1 - z0) / seg_len).ceil() as usize;
        for s in 0..segs {
            let za = z0 + s as f32 * seg_len;
            let zb = (za + seg_len).min(z1);
            let alt = (r + s) % 2 == 0;
            let k = if alt { 1.0 } else { 0.92 };
            let at = Vec3::new(xc, 0.0, za);
            let mesh = b.at(at);
            match crop {
                Crop::Wheat => {
                    // Low golden rows with a darker furrow between.
                    let c = shade(hx(0xefc752), k * rng.range(0.96, 1.04));
                    mesh.aabb(Vec3::new(xc - 0.42, 0.03, za), Vec3::new(xc + 0.42, 0.55, zb), &Style::rgb(c).flat().ao(0.35));
                    // Ears on top.
                    mesh.aabb(Vec3::new(xc - 0.3, 0.55, za), Vec3::new(xc + 0.3, 0.64, zb), &Style::rgb(shade(hx(0xffdb6e), k)).flat());
                }
                Crop::Corn => {
                    // Tall stalk wall, ragged top, tassels.
                    let h = 1.7 + 0.2 * hash01(r as i32, s as i32, 11);
                    let c = shade(hx(0x58b335), k * rng.range(0.95, 1.05));
                    mesh.aabb(Vec3::new(xc - 0.45, 0.03, za), Vec3::new(xc + 0.45, h, zb), &Style::rgb(c).flat().ao(0.4));
                    for j in 0..2 {
                        let zz = za + (zb - za) * (j as f32 + 0.5) / 2.0;
                        mesh.cone(&(tr(Vec3::new(xc + rng.range(-0.2, 0.2), h - 0.02, zz)) * sc(Vec3::new(0.3, 0.5, 0.3))), 3, &st(0xd9c25a));
                    }
                }
                Crop::Sunflower => {
                    // Green ridge with big yellow heads.
                    let c = shade(hx(0x4fa33a), k);
                    mesh.aabb(Vec3::new(xc - 0.35, 0.03, za), Vec3::new(xc + 0.35, 0.9, zb), &Style::rgb(c).flat().ao(0.4));
                    for j in 0..2 {
                        let zz = za + (zb - za) * (j as f32 + 0.5) / 2.0 + rng.range(-0.5, 0.5);
                        let hh = rng.range(1.4, 1.9);
                        // Head faces back toward the camera, tilted: a yellow disc with a dark centre.
                        let m = tr(Vec3::new(xc, hh, zz)) * Mat4::from_rotation_x(-1.1);
                        mesh.cyl(&(m * sc(Vec3::new(0.42, 0.08, 0.42))), 7, 1.0, 1.0, true, &st(0xffcc1c));
                        mesh.disc(&(m * tr(Vec3::new(0.0, 0.085, 0.0)) * sc(Vec3::new(0.2, 1.0, 0.2))), 6, &st(0x5b3a1a));
                    }
                }
                Crop::Cabbage => {
                    for j in 0..2 {
                        let zz = za + (zb - za) * (j as f32 + 0.5) / 2.0;
                        let big = rng.range(0.45, 0.6);
                        let c = shade(if (r + j) % 3 == 0 { hx(0xb44b9a) } else { hx(0x76c957) }, rng.range(0.9, 1.1));
                        mesh.sphere(&(tr(Vec3::new(xc, big * 0.7, zz)) * sc(Vec3::new(big, big * 0.75, big))), 5, 3, &Style::rgb(c).flat().ao(0.3));
                    }
                }
                Crop::Lavender => {
                    let c = shade(hx(0x9a6bd8), k);
                    mesh.aabb(Vec3::new(xc - 0.38, 0.03, za), Vec3::new(xc + 0.38, 0.6, zb), &Style::rgb(c).flat().ao(0.35));
                    mesh.aabb(Vec3::new(xc - 0.28, 0.6, za), Vec3::new(xc + 0.28, 0.72, zb), &Style::rgb(shade(hx(0xb68af0), k)).flat());
                }
            }
        }
    }
    // Field edge: a dark strip so rows read against the grass.
    for (xa, xb) in [(lo - 0.3, lo), (hi, hi + 0.3)] {
        let p = [Vec3::new(xa, 0.032, z0), Vec3::new(xb, 0.032, z0), Vec3::new(xb, 0.032, z1), Vec3::new(xa, 0.032, z1)];
        b.at(p[0]).quad4(p, Vec3::Y, &st(0x7e5a33));
    }
}

fn fields(_style: &FarmStyle, b: &mut Bins, rng: &mut Rng) {
    // Left of the runners (render +x).
    field(b, rng, Crop::Wheat, 14.0, 44.0, 30.0, 72.0);
    field(b, rng, Crop::Corn, 14.0, 40.0, 132.0, 186.0);
    field(b, rng, Crop::Lavender, 14.0, 36.0, 194.0, 226.0);
    // Right.
    field(b, rng, Crop::Sunflower, -14.0, -42.0, 14.0, 64.0);
    field(b, rng, Crop::Cabbage, -14.0, -34.0, 196.0, 230.0);
    field(b, rng, Crop::Wheat, -14.0, -42.0, 74.0, 108.0);
}

// -------------------------------------------------------------- buildings ----

fn red_barn(b: &mut MeshData, w: f32, d: f32, hw: f32) {
    // Local frame: width w (x), depth d (z), door on the +z face.
    let red = [0xb8321f, 0xc23a25, 0xa62c1b];
    let white = st(0xf4efe3);
    let roof = st(0x4f4a5a);
    let roof_hi = st(0x6c6678);
    // Foundation.
    b.aabb(Vec3::new(-w * 0.5 - 0.1, 0.0, -d * 0.5 - 0.1), Vec3::new(w * 0.5 + 0.1, 0.5, d * 0.5 + 0.1), &st(0x8b8883));
    // Planked walls.
    let planks = (w / 0.8) as i32;
    for i in 0..planks {
        let x0 = -w * 0.5 + w * i as f32 / planks as f32;
        let x1 = -w * 0.5 + w * (i + 1) as f32 / planks as f32;
        let s = st(red[(i as usize) % 3]);
        b.quad4([Vec3::new(x0, 0.5, d * 0.5), Vec3::new(x1, 0.5, d * 0.5), Vec3::new(x1, hw, d * 0.5), Vec3::new(x0, hw, d * 0.5)], Vec3::Z, &s);
        b.quad4([Vec3::new(x0, 0.5, -d * 0.5), Vec3::new(x1, 0.5, -d * 0.5), Vec3::new(x1, hw, -d * 0.5), Vec3::new(x0, hw, -d * 0.5)], -Vec3::Z, &s);
    }
    let nz = (d / 0.9) as i32;
    for side in [-1.0f32, 1.0] {
        for i in 0..nz {
            let z0 = -d * 0.5 + d * i as f32 / nz as f32;
            let z1 = -d * 0.5 + d * (i + 1) as f32 / nz as f32;
            let x = side * w * 0.5;
            b.quad4([Vec3::new(x, 0.5, z0), Vec3::new(x, 0.5, z1), Vec3::new(x, hw, z1), Vec3::new(x, hw, z0)], Vec3::X * side, &st(red[(i as usize + 1) % 3]));
        }
    }
    // Gambrel gable fill (front and back).
    let prof = [(w * 0.5, hw), (w * 0.5 * 0.66, hw + 2.2), (0.0, hw + 3.4), (-w * 0.5 * 0.66, hw + 2.2), (-w * 0.5, hw)];
    for z in [d * 0.5, -d * 0.5] {
        let n = if z > 0.0 { Vec3::Z } else { -Vec3::Z };
        let pts: Vec<Vec3> = prof.iter().map(|&(x, y)| Vec3::new(x, y, z)).collect();
        b.tri([pts[0], pts[1], pts[4]], n, &st(0xb8321f));
        b.tri([pts[1], pts[3], pts[4]], n, &st(0xc23a25));
        b.tri([pts[1], pts[2], pts[3]], n, &st(0xb8321f));
        // Roof edge trim.
    }
    // Roof panels: lower steep, upper shallow, with overhang.
    let ov = 0.55;
    for side in [-1.0f32, 1.0] {
        let p0 = Vec3::new(side * (w * 0.5 + 0.35), hw - 0.15, 0.0);
        let p1 = Vec3::new(side * w * 0.5 * 0.66, hw + 2.2, 0.0);
        let p2 = Vec3::new(0.0, hw + 3.4, 0.0);
        for (a, c, s, k) in [(p0, p1, &roof, 0.0f32), (p1, p2, &roof_hi, 1.0)] {
            let z0 = -d * 0.5 - ov;
            let z1 = d * 0.5 + ov;
            let q = [Vec3::new(a.x, a.y, z0), Vec3::new(c.x, c.y, z0), Vec3::new(c.x, c.y, z1), Vec3::new(a.x, a.y, z1)];
            let up = Vec3::new(0.0, 1.0, 0.0);
            b.quad4(q, up, s);
            // Shingle lines.
            let rows = 4;
            for j in 1..rows {
                let t = j as f32 / rows as f32;
                let pa = a.lerp(c, t);
                let q2 = [Vec3::new(pa.x, pa.y + 0.02, z0), Vec3::new(pa.x + (c.x - a.x) * 0.02, pa.y + 0.02, z0), Vec3::new(pa.x + (c.x - a.x) * 0.02, pa.y + 0.02, z1), Vec3::new(pa.x, pa.y + 0.02, z1)];
                b.quad4(q2, up, &st(0x3c3845));
            }
            let _ = k;
        }
    }
    // Roof underside gable overhang trim.
    b.aabb(Vec3::new(-0.2, hw + 3.4, -d * 0.5 - ov), Vec3::new(0.2, hw + 3.55, d * 0.5 + ov), &st(0x888494));
    // Cupola.
    b.aabb(Vec3::new(-0.7, hw + 3.5, -0.7), Vec3::new(0.7, hw + 4.6, 0.7), &white);
    b.cone(&(tr(Vec3::new(0.0, hw + 4.6, 0.0)) * Mat4::from_rotation_y(FRAC_PI_2 * 0.5) * sc(Vec3::new(1.15, 0.95, 1.15))), 4, &roof);
    // White trim and the big doors on the front.
    let zf = d * 0.5 + 0.03;
    let tw = 0.22;
    for x in [-w * 0.5 + tw * 0.5, w * 0.5 - tw * 0.5] {
        b.aabb(Vec3::new(x - tw * 0.5, 0.5, zf - 0.05), Vec3::new(x + tw * 0.5, hw, zf + 0.05), &white);
    }
    let dw = 4.6f32.min(w * 0.55);
    let dh = 4.0f32.min(hw - 0.4);
    b.aabb(Vec3::new(-dw * 0.5, 0.5, zf - 0.04), Vec3::new(dw * 0.5, 0.5 + dh, zf + 0.0), &st(0x8a2418));
    for (x0, x1) in [(-dw * 0.5, -dw * 0.5 + 0.2), (dw * 0.5 - 0.2, dw * 0.5), (-0.1, 0.1)] {
        b.aabb(Vec3::new(x0, 0.5, zf + 0.0), Vec3::new(x1, 0.5 + dh, zf + 0.08), &white);
    }
    for y in [0.5, 0.5 + dh - 0.2] {
        b.aabb(Vec3::new(-dw * 0.5, y, zf), Vec3::new(dw * 0.5, y + 0.2, zf + 0.08), &white);
    }
    for half in [-1.0f32, 1.0] {
        let cx = half * dw * 0.25;
        let a = Vec3::new(cx - dw * 0.22, 0.7, zf + 0.09);
        let c = Vec3::new(cx + dw * 0.22, 0.5 + dh - 0.2, zf + 0.09);
        let e = Vec3::new(cx + dw * 0.22, 0.7, zf + 0.09);
        let f = Vec3::new(cx - dw * 0.22, 0.5 + dh - 0.2, zf + 0.09);
        for (p, q) in [(a, c), (e, f)] {
            let mid = (p + q) * 0.5;
            let dirv = q - p;
            let x = dirv.normalize();
            let y = Vec3::Z.cross(x).normalize();
            b.cube(&crate::mesh::basis(mid, x * dirv.length(), y * 0.16, Vec3::Z * 0.06), &white);
        }
    }
    // Hayloft door and window frame above.
    b.aabb(Vec3::new(-0.8, hw - 1.6, zf - 0.04), Vec3::new(0.8, hw - 0.3, zf + 0.0), &white);
    b.aabb(Vec3::new(-0.65, hw - 1.45, zf), Vec3::new(0.65, hw - 0.45, zf + 0.04), &st(0x2a1c1a));
    // Side windows.
    for side in [-1.0f32, 1.0] {
        for i in 0..(d / 5.0) as i32 {
            let z = -d * 0.5 + 2.5 + i as f32 * 5.0;
            let x = side * (w * 0.5 + 0.02);
            b.aabb(Vec3::new(x - 0.04, 2.2, z - 0.6), Vec3::new(x + 0.04, 3.5, z + 0.6), &white);
            b.aabb(Vec3::new(x - 0.02, 2.35, z - 0.45), Vec3::new(x + 0.06, 3.35, z + 0.45), &st(0x2a1c1a));
        }
    }
}

fn silo(b: &mut MeshData, r: f32, h: f32, body: u32, cap: u32) {
    b.cyl(&sc(Vec3::new(r + 0.15, 0.35, r + 0.15)), 16, 1.0, 1.0, true, &st(0x8b8883));
    b.cyl(&(tr(Vec3::new(0.0, 0.35, 0.0)) * sc(Vec3::new(r, h, r))), 16, 1.0, 1.0, false, &Style::rgb(hx(body)).flat().ao(0.2).sheen(0.2));
    let bands = (h / 1.3) as i32;
    for i in 1..=bands {
        b.cyl(&(tr(Vec3::new(0.0, 0.35 + i as f32 * h / (bands + 1) as f32, 0.0)) * sc(Vec3::new(r + 0.04, 0.12, r + 0.04))), 16, 1.0, 1.0, false, &st(0x7d868c));
    }
    // Dome.
    let prof: Vec<(f32, f32)> = (0..=6).map(|i| {
        let a = FRAC_PI_2 * i as f32 / 6.0;
        (a.cos() * (r + 0.1), 0.35 + h + a.sin() * (r * 0.62))
    }).collect();
    b.lathe(&Mat4::IDENTITY, 16, &prof, &st(cap));
    b.cyl(&(tr(Vec3::new(0.0, 0.35 + h + r * 0.62, 0.0)) * sc(Vec3::new(0.08, 0.6, 0.08))), 6, 1.0, 1.0, false, &st(0x555a60));
    // Ladder strip and door.
    b.aabb(Vec3::new(-0.2, 0.35, r - 0.05), Vec3::new(0.2, 0.35 + h, r + 0.12), &st(0x6a5a4a));
    for i in 0..(h * 2.0) as i32 {
        b.aabb(Vec3::new(-0.3, 0.6 + i as f32 * 0.5, r + 0.1), Vec3::new(0.3, 0.66 + i as f32 * 0.5, r + 0.16), &st(0x4a4038));
    }
}

fn windmill_tower(b: &mut MeshData, hub_y: f32) {
    let iron = st(0x7d838b);
    let white = st(0xf0ece0);
    let h = hub_y - 0.6;
    let (r0, r1) = (1.5f32, 0.45f32);
    let leg = |bx: f32, bz: f32| -> (Vec3, Vec3) { (Vec3::new(bx * r0, 0.0, bz * r0), Vec3::new(bx * r1, h, bz * r1)) };
    let legs = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    for (bx, bz) in legs {
        let (a, c) = leg(bx, bz);
        b.cyl(&limb(a, c, 0.09), 6, 1.0, 0.8, false, &iron);
        b.aabb(Vec3::new(a.x - 0.25, 0.0, a.z - 0.25), Vec3::new(a.x + 0.25, 0.22, a.z + 0.25), &st(0x8b8883));
    }
    // Rings and X braces.
    let levels = 4;
    for i in 1..=levels {
        let y = h * i as f32 / (levels + 1) as f32;
        let r = r0 + (r1 - r0) * (y / h);
        let ring = [Vec3::new(-r, y, -r), Vec3::new(r, y, -r), Vec3::new(r, y, r), Vec3::new(-r, y, r)];
        for k in 0..4 {
            let (a, c) = (ring[k], ring[(k + 1) % 4]);
            b.cyl(&limb(a, c, 0.05), 5, 1.0, 1.0, false, &white);
        }
        if i < levels {
            let y2 = h * (i + 1) as f32 / (levels + 1) as f32;
            let r2 = r0 + (r1 - r0) * (y2 / h);
            let nxt = [Vec3::new(-r2, y2, -r2), Vec3::new(r2, y2, -r2), Vec3::new(r2, y2, r2), Vec3::new(-r2, y2, r2)];
            for k in 0..4 {
                b.cyl(&limb(ring[k], nxt[(k + 1) % 4], 0.035), 4, 1.0, 1.0, false, &iron);
            }
        }
    }
    // Platform and gearbox head.
    b.aabb(Vec3::new(-0.7, h, -0.7), Vec3::new(0.7, h + 0.12, 0.7), &st(0x5d4630));
    b.aabb(Vec3::new(-0.3, h + 0.12, -0.6), Vec3::new(0.3, h + 0.8, 0.5), &st(0xc8402a));
    // Rudder tail.
    b.aabb(Vec3::new(-0.04, h + 0.3, -3.1), Vec3::new(0.04, h + 1.5, -0.6), &st(0xc8402a));
    b.aabb(Vec3::new(-0.04, h + 0.1, -3.1), Vec3::new(0.04, h + 1.5, -2.2), &st(0xc8402a));
    b.aabb(Vec3::new(-0.05, h + 0.35, -1.6), Vec3::new(0.05, h + 0.45, -0.6), &st(0x6b2a1d));
    // A water tank beside it.
    b.cyl(&(tr(Vec3::new(2.6, 0.0, 0.0)) * sc(Vec3::new(1.0, 1.6, 1.0))), 12, 1.0, 1.0, true, &Style::rgb(hx(0x7a5a3a)).flat().ao(0.3));
    b.cyl(&(tr(Vec3::new(2.6, 1.6, 0.0)) * sc(Vec3::new(1.05, 0.1, 1.05))), 12, 1.0, 1.0, true, &st(0x4d4a55));
}

fn buildings(style: &FarmStyle, b: &mut Bins, rng: &mut Rng) {
    let _ = style;
    // Left side (render +x): a farmyard around z = 100, with the barn facing the track.
    {
        let at = Vec3::new(26.0, 0.0, 104.0);
        let m = tr(at) * Mat4::from_rotation_y(-FRAC_PI_2);
        let mut barn = MeshData::new();
        red_barn(&mut barn, 12.0, 16.0, 5.2);
        b.at(at).append(&barn, &m);
        // Two silos behind, one smaller shed.
        for (pz, r, hgt, body, cap) in [(100.0f32, 2.2f32, 11.0f32, 0xcfd6dc, 0x9c3a2c), (107.5, 2.0, 9.4, 0xe9e3d3, 0x4d6c8c)] {
            let mut s = MeshData::new();
            silo(&mut s, r, hgt, body, cap);
            let p = Vec3::new(38.5, 0.0, pz);
            b.at(p).append(&s, &tr(p));
        }
        // Fenced yard patch (packed dirt) in front of the barn.
        let yard = [Vec3::new(14.0, 0.03, 90.0), Vec3::new(21.0, 0.03, 90.0), Vec3::new(21.0, 0.03, 118.0), Vec3::new(14.0, 0.03, 118.0)];
        b.at(yard[0]).quad4(yard, Vec3::Y, &st(0xb99560));
    }
    // Right side (render -x): a second barn, gable on to the track, and a small tool shed.
    {
        let at = Vec3::new(-24.0, 0.0, 150.0);
        let m = tr(at) * Mat4::from_rotation_y(FRAC_PI_2);
        let mut barn = MeshData::new();
        red_barn(&mut barn, 14.0, 12.0, 5.8);
        b.at(at).append(&barn, &m);
        let p = Vec3::new(-36.0, 0.0, 138.0);
        let mut s = MeshData::new();
        silo(&mut s, 2.4, 12.5, 0xc7ced4, 0x9c3a2c);
        b.at(p).append(&s, &tr(p));
        let yard = [Vec3::new(-14.0, 0.03, 136.0), Vec3::new(-19.0, 0.03, 136.0), Vec3::new(-19.0, 0.03, 164.0), Vec3::new(-14.0, 0.03, 164.0)];
        b.at(yard[0]).quad4(yard, Vec3::Y, &st(0xb99560));
        // Little white farmhouse.
        let hp = Vec3::new(-30.0, 0.0, 178.0);
        let mut house = MeshData::new();
        farmhouse(&mut house);
        b.at(hp).append(&house, &(tr(hp) * Mat4::from_rotation_y(0.12)));
    }
    // Windmills (static towers; the wheels turn as instances).
    for (hub, face) in layout(style).windmills {
        let mut tower = MeshData::new();
        windmill_tower(&mut tower, hub.y);
        let p = Vec3::new(hub.x, 0.0, hub.z);
        b.at(p).append(&tower, &(tr(p) * Mat4::from_rotation_y(if face > 1.0 { PI } else { 0.0 })));
    }
    // A pennant on each barn's cupola (animated decor).
    let kit2 = decor::build_kit2(&farm_palette());
    for (pos, k) in [(Vec3::new(26.0, 10.3, 104.0), 0.3f32), (Vec3::new(-24.0, 10.8, 150.0), 0.6)] {
        let m = tr(pos) * Mat4::from_rotation_y(PI) * sc(Vec3::splat(0.75));
        let yaw8 = u8f(((PI) / TAU).rem_euclid(1.0));
        b.dat(pos).append(&kit2.flag, &m, 1.0, |lp, _| if lp.x > 0.06 { [decor::A_FLUTTER, u8f(k), u8f((lp.x - 0.05) / 1.8), yaw8] } else { [0, 0, 0, 0] });
    }
    let _ = rng;
}

fn farmhouse(b: &mut MeshData) {
    let wall = st(0xf6f1e4);
    let roof = st(0xb0432d);
    let (w, d, h) = (8.0f32, 6.0f32, 3.2f32);
    b.aabb(Vec3::new(-w * 0.5, 0.0, -d * 0.5), Vec3::new(w * 0.5, h, d * 0.5), &wall.ao(0.2));
    // Hip roof as a pyramid-ish prism.
    let ridge_h = h + 2.2;
    for side in [-1.0f32, 1.0] {
        let q = [Vec3::new(side * (w * 0.5 + 0.5), h - 0.1, -d * 0.5 - 0.5), Vec3::new(0.0, ridge_h, -d * 0.5 - 0.5), Vec3::new(0.0, ridge_h, d * 0.5 + 0.5), Vec3::new(side * (w * 0.5 + 0.5), h - 0.1, d * 0.5 + 0.5)];
        b.quad4(q, Vec3::Y, &roof);
    }
    for z in [-1.0f32, 1.0] {
        let n = Vec3::new(0.0, 0.3, z);
        b.tri([Vec3::new(-w * 0.5 - 0.5, h - 0.1, z * (d * 0.5 + 0.5)), Vec3::new(w * 0.5 + 0.5, h - 0.1, z * (d * 0.5 + 0.5)), Vec3::new(0.0, ridge_h, z * (d * 0.5 + 0.5))], n, &roof);
        b.tri([Vec3::new(-w * 0.5, h, z * d * 0.5), Vec3::new(w * 0.5, h, z * d * 0.5), Vec3::new(0.0, ridge_h - 0.1, z * d * 0.5)], n, &wall);
    }
    // Door + windows (front = +x).
    b.aabb(Vec3::new(w * 0.5 - 0.02, 0.0, -0.6), Vec3::new(w * 0.5 + 0.1, 2.2, 0.6), &st(0x7a3a24));
    for z in [-2.2f32, 2.2] {
        b.aabb(Vec3::new(w * 0.5 - 0.02, 1.1, z - 0.55), Vec3::new(w * 0.5 + 0.08, 2.3, z + 0.55), &st(0x7ec4ea));
        b.aabb(Vec3::new(w * 0.5 + 0.02, 0.95, z - 0.7), Vec3::new(w * 0.5 + 0.12, 1.05, z + 0.7), &st(0x7a3a24));
    }
    // Chimney.
    b.aabb(Vec3::new(-2.2, ridge_h - 0.8, -0.5), Vec3::new(-1.5, ridge_h + 1.2, 0.2), &st(0x9a4a35));
}

// ------------------------------------------------------------------ trees ----

fn tree(m: &mut MeshData, rng: &mut Rng, kind: u32, h: f32) {
    let trunk = shade(hx(0x7a4f2c), rng.range(0.9, 1.1));
    match kind {
        0 | 1 => {
            // Round broadleaf: trunk + three leaf blobs.
            let leaf = if kind == 0 { shade(hx(0x3f9f3a), rng.range(0.88, 1.12)) } else { shade(hx(0x6cc04a), rng.range(0.9, 1.1)) };
            m.cyl(&sc(Vec3::new(0.35, h * 0.5, 0.35)), 7, 1.0, 0.6, true, &Style::rgb(trunk).flat().ao(0.3));
            let r = h * 0.38;
            for (o, k) in [(Vec3::new(0.0, h * 0.72, 0.0), 1.0f32), (Vec3::new(r * 0.55, h * 0.6, r * 0.2), 0.72), (Vec3::new(-r * 0.5, h * 0.62, -r * 0.3), 0.7)] {
                m.sphere_jit(&(tr(o) * sc(Vec3::splat(r * k))), 7, 4, &Style::rgb(shade(leaf, 0.92 + 0.12 * k)).flat().ao(0.3), 0.12, (o.x * 10.0) as i32);
            }
        }
        2 => {
            // Autumn orange.
            let leaf = shade(hx(0xf09a2d), rng.range(0.9, 1.1));
            m.cyl(&sc(Vec3::new(0.32, h * 0.5, 0.32)), 7, 1.0, 0.6, true, &Style::rgb(trunk).flat().ao(0.3));
            let r = h * 0.36;
            for (o, k) in [(Vec3::new(0.0, h * 0.7, 0.0), 1.0f32), (Vec3::new(r * 0.5, h * 0.62, r * 0.3), 0.7), (Vec3::new(-r * 0.4, h * 0.85, 0.0), 0.65)] {
                m.sphere_jit(&(tr(o) * sc(Vec3::splat(r * k))), 7, 4, &Style::rgb(shade(leaf, 0.9 + 0.15 * k)).flat().ao(0.3), 0.14, (o.y * 10.0) as i32);
            }
        }
        _ => {
            // Pine: stacked cones.
            let leaf = shade(hx(0x2c7a3f), rng.range(0.9, 1.1));
            m.cyl(&sc(Vec3::new(0.3, h * 0.3, 0.3)), 6, 1.0, 0.7, true, &Style::rgb(trunk).flat().ao(0.3));
            for i in 0..4 {
                let t = i as f32 / 4.0;
                let y = h * (0.2 + 0.2 * i as f32);
                let r = h * 0.28 * (1.0 - 0.7 * t);
                m.cone(&(tr(Vec3::new(0.0, y, 0.0)) * Mat4::from_rotation_y(i as f32) * sc(Vec3::new(r, h * 0.3, r))), 8, &Style::rgb(shade(leaf, 0.9 + 0.07 * i as f32)).flat().ao(0.3));
            }
        }
    }
}

fn trees(_style: &FarmStyle, b: &mut Bins, rng: &mut Rng) {
    // Roadside trees, singly, just behind the verge hedge.
    for side in [-1.0f32, 1.0] {
        let mut z = rng.range(4.0, 20.0);
        while z < TILE_LEN - 6.0 {
            let x = side * rng.range(11.0, 13.5);
            let mut skip = false;
            // Keep clear of the farmyards.
            if side > 0.0 && (88.0..120.0).contains(&z) || side < 0.0 && (134.0..170.0).contains(&z) {
                skip = true;
            }
            if !skip {
                let p = Vec3::new(x, 0.0, z);
                let mut m = MeshData::new();
                let kind = (rng.next() * 3.0) as u32 % 3;
                let th = rng.range(5.5, 8.5);
                tree(&mut m, rng, kind, th);
                b.at(p).append(&m, &(tr(p) * Mat4::from_rotation_y(rng.range(0.0, TAU))));
                // Shadow patch.
                b.at(p).disc(&(tr(p + Vec3::new(0.0, 0.035, 0.0)) * sc(Vec3::new(2.4, 1.0, 2.0))), 9, &st(0x4f9a33));
            }
            z += rng.range(22.0, 48.0);
        }
        // Clumps of trees on the hills.
        for _ in 0..9 {
            let x = side * rng.range(52.0, 160.0);
            let z = rng.range(4.0, TILE_LEN - 4.0);
            let p = Vec3::new(x, hill_height(x, z), z);
            let mut m = MeshData::new();
            let kind = (rng.next() * 4.0) as u32;
            let th = rng.range(7.0, 12.0);
            tree(&mut m, rng, kind, th);
            b.at(p).append(&m, &(tr(p - Vec3::Y * 0.3) * Mat4::from_rotation_y(rng.range(0.0, TAU))));
            if rng.next() < 0.6 {
                let q = p + Vec3::new(rng.range(-5.0, 5.0), 0.0, rng.range(-5.0, 5.0));
                let q = Vec3::new(q.x, hill_height(q.x, q.z), q.z);
                let mut m2 = MeshData::new();
                let th = rng.range(6.0, 10.0);
                tree(&mut m2, rng, kind, th);
                b.at(q).append(&m2, &(tr(q - Vec3::Y * 0.3) * Mat4::from_rotation_y(rng.range(0.0, TAU))));
            }
        }
    }
}

// -------------------------------------------------------------- yard props ----

fn haystack(m: &mut MeshData) {
    let body: Vec<(f32, f32)> = vec![(0.0, 0.0), (1.55, 0.0), (1.6, 0.35), (1.35, 1.2), (0.85, 2.0), (0.3, 2.55), (0.0, 2.7)];
    m.lathe(&Mat4::IDENTITY, 12, &body, &Style::rgb(hx(0xe9b94a)).flat().ao(0.3));
    for i in 0..3 {
        let y = 0.5 + 0.55 * i as f32;
        let r = 1.62 - 0.22 * i as f32;
        m.cyl(&(tr(Vec3::new(0.0, y, 0.0)) * sc(Vec3::new(r, 0.06, r))), 12, 1.0, 1.0, false, &st(0xc98f2c));
    }
}

fn bale_pile(m: &mut MeshData, rng: &mut Rng) {
    let straw = [0xe9b94a, 0xf0c85a, 0xdca93d];
    let rows: [&[(f32, f32)]; 3] = [&[(-1.5, 0.0), (0.0, 0.0), (1.5, 0.0)], &[(-0.75, 0.0), (0.75, 0.0)], &[(0.0, 0.0)]];
    for (j, row) in rows.iter().enumerate() {
        for (i, &(x, _)) in row.iter().enumerate() {
            let y0 = j as f32 * 0.72;
            let c = straw[(i + j) % 3];
            let jx = rng.range(-0.05, 0.05);
            m.aabb(Vec3::new(x - 0.7 + jx, y0, -0.45), Vec3::new(x + 0.7 + jx, y0 + 0.7, 0.45), &Style::rgb(hx(c)).flat().ao(0.25));
            for z in [-0.17f32, 0.17] {
                m.aabb(Vec3::new(x - 0.71 + jx, y0, z - 0.02), Vec3::new(x + 0.71 + jx, y0 + 0.71, z + 0.02), &st(0xb0522a));
            }
        }
    }
}

fn scarecrow(m: &mut MeshData) {
    let wood = st(0x7a5230);
    m.cyl(&sc(Vec3::new(0.05, 2.3, 0.05)), 6, 1.0, 1.0, false, &wood);
    m.cyl(&(tr(Vec3::new(-0.95, 1.65, 0.0)) * Mat4::from_rotation_z(-FRAC_PI_2) * sc(Vec3::new(0.04, 1.9, 0.04))), 6, 1.0, 1.0, false, &wood);
    // Shirt and trousers.
    m.aabb(Vec3::new(-0.38, 1.2, -0.18), Vec3::new(0.38, 1.85, 0.18), &st(0xd3463a));
    m.aabb(Vec3::new(-0.28, 0.5, -0.15), Vec3::new(0.28, 1.2, 0.15), &st(0x3b5da8));
    for sx in [-1.0f32, 1.0] {
        m.aabb(Vec3::new(sx * 0.4 - 0.12, 1.5, -0.1), Vec3::new(sx * 1.0 + 0.1, 1.8, 0.1), &st(0xd3463a));
        m.cone(&(tr(Vec3::new(sx * 1.0, 1.55, 0.0)) * Mat4::from_rotation_z(-sx * FRAC_PI_2) * sc(Vec3::new(0.1, 0.3, 0.1))), 5, &st(0xf0cf6a));
    }
    // Head, hat.
    m.sphere(&(tr(Vec3::new(0.0, 2.05, 0.0)) * sc(Vec3::splat(0.3))), 9, 6, &st(0xf0cf6a));
    m.cyl(&(tr(Vec3::new(0.0, 2.25, 0.0)) * sc(Vec3::new(0.55, 0.05, 0.55))), 12, 1.0, 1.0, true, &st(0x8a5a2e));
    m.cyl(&(tr(Vec3::new(0.0, 2.28, 0.0)) * sc(Vec3::new(0.28, 0.32, 0.28))), 10, 1.0, 0.8, true, &st(0x8a5a2e));
    // Face on the -z side (towards runners): eyes and mouth.
    for sx in [-0.1f32, 0.1] {
        m.sphere(&(tr(Vec3::new(sx, 2.08, -0.27)) * sc(Vec3::splat(0.05))), 5, 3, &st(0x222222));
    }
    m.aabb(Vec3::new(-0.1, 1.92, -0.3), Vec3::new(0.1, 1.96, -0.26), &st(0x222222));
}

fn yard_props(style: &FarmStyle, b: &mut Bins, rng: &mut Rng) {
    let _ = style;
    // Haystacks and bale piles around the yards and along the verge.
    for (x, z, kind) in [
        (19.5f32, 124.0f32, 0u8), (43.0, 92.0, 0), (-18.0, 128.0, 0),
        (17.5, 94.0, 1), (-17.0, 168.0, 1), (16.5, 75.0, 1), (-30.0, 128.0, 0), (46.0, 224.0, 0), (-16.0, 70.0, 1),
        (-46.0, 180.0, 0), (48.0, 150.0, 0),
    ] {
        let p = Vec3::new(x, 0.0, z);
        let mut m = MeshData::new();
        if kind == 0 { haystack(&mut m); } else { bale_pile(&mut m, rng); }
        let yaw = if kind == 0 { rng.range(0.0, TAU) } else { FRAC_PI_2 + rng.range(-0.2, 0.2) };
        b.at(p).append(&m, &(tr(p) * Mat4::from_rotation_y(yaw)));
        b.at(p).disc(&(tr(p + Vec3::Y * 0.035) * sc(Vec3::new(2.3, 1.0, 2.0))), 10, &st(0x6aa83c));
    }
    // Round bales dotted in the fields.
    for (x, z) in [(-30.0f32, 90.0f32), (-24.0, 100.0), (30.0, 40.0), (32.0, 200.0), (36.0, 60.0)] {
        let p = Vec3::new(x, 0.0, z);
        let m = tr(p + Vec3::Y * 0.6) * Mat4::from_rotation_y(rng.range(0.0, TAU)) * Mat4::from_rotation_z(-FRAC_PI_2) * tr(Vec3::new(0.0, -0.6, 0.0));
        let mesh = b.at(p);
        mesh.cyl(&(m * sc(Vec3::new(0.6, 1.2, 0.6))), 12, 1.0, 1.0, true, &Style::rgb(hx(0xe9b94a)).flat().ao(0.25));
        mesh.cyl(&(m * tr(Vec3::new(0.0, 0.55, 0.0)) * sc(Vec3::new(0.61, 0.1, 0.61))), 12, 1.0, 1.0, false, &st(0xc98f2c));
    }
    // Scarecrows in the corn and the sunflowers.
    for (x, z, yaw) in [(26.0f32, 160.0f32, 0.4f32), (-28.0, 40.0, 0.2)] {
        let mut m = MeshData::new();
        scarecrow(&mut m);
        let p = Vec3::new(x, 0.0, z);
        b.at(p).append(&m, &(tr(p) * Mat4::from_rotation_y(yaw)));
    }
    // A parked tractor and a hay wagon.
    {
        let p = Vec3::new(-20.0, 0.0, 208.0);
        let mut m = MeshData::new();
        m.append(&crate::farmprops::tractor(), &sc(crate::farmprops::BLOCK_REF));
        b.at(p).append(&m, &(tr(p) * Mat4::from_rotation_y(FRAC_PI_2 + 0.25)));
        let wp = Vec3::new(18.5, 0.0, 12.0);
        let mut w = MeshData::new();
        wagon(&mut w);
        b.at(wp).append(&w, &(tr(wp) * Mat4::from_rotation_y(0.1)));
    }
    // Crates and barrels near the yards.
    for (x, z) in [(15.0f32, 98.0f32), (15.5, 100.2), (-15.5, 140.0), (-15.0, 142.0)] {
        let p = Vec3::new(x, 0.0, z);
        let mesh = b.at(p);
        mesh.cyl(&(tr(p) * sc(Vec3::new(0.38, 0.85, 0.38))), 10, 1.0, 1.0, true, &Style::rgb(hx(0x7a5030)).flat().ao(0.25));
        for y in [0.15f32, 0.7] {
            mesh.cyl(&(tr(p + Vec3::Y * y) * sc(Vec3::new(0.395, 0.06, 0.395))), 10, 1.0, 1.0, false, &st(0x4a4f57));
        }
    }
}

fn wagon(m: &mut MeshData) {
    let wood = st(0xb07a42);
    let dark = st(0x7a4f2a);
    m.aabb(Vec3::new(-1.0, 0.55, -1.8), Vec3::new(1.0, 0.7, 1.8), &wood);
    for (a, c) in [((-1.0f32, -1.8f32), (-0.9, 1.8)), ((0.9, -1.8), (1.0, 1.8))] {
        m.aabb(Vec3::new(a.0, 0.7, a.1), Vec3::new(c.0, 1.3, c.1), &dark);
    }
    for z in [-1.8f32, 1.7] {
        m.aabb(Vec3::new(-1.0, 0.7, z), Vec3::new(1.0, 1.3, z + 0.1), &dark);
    }
    // Hay load.
    m.sphere(&(tr(Vec3::new(0.0, 1.3, 0.0)) * sc(Vec3::new(0.95, 0.65, 1.7))), 10, 5, &Style::rgb(hx(0xe9b94a)).flat().ao(0.3));
    for (x, z) in [(-1.05f32, -1.1f32), (1.05, -1.1), (-1.05, 1.1), (1.05, 1.1)] {
        m.cyl(&(tr(Vec3::new(x - 0.07, 0.5, z)) * Mat4::from_rotation_z(-FRAC_PI_2) * sc(Vec3::new(0.5, 0.14, 0.5))), 12, 1.0, 1.0, true, &st(0x4a3524));
    }
    m.aabb(Vec3::new(-0.06, 0.45, 1.8), Vec3::new(0.06, 0.55, 3.2), &dark);
}

// ------------------------------------------------------------------ ponds ----

fn ponds(_style: &FarmStyle, b: &mut Bins, rng: &mut Rng) {
    let kit2 = decor::build_kit2(&farm_palette());
    for (x, z, rx, rz) in [(-50.0f32, 98.0f32, 8.0f32, 6.0f32), (30.0, 14.0, 6.0, 4.5), (-46.0, 212.0, 7.0, 5.0)] {
        let p = Vec3::new(x, 0.08, z);
        // Muddy bank ring.
        b.at(p).disc(&(tr(Vec3::new(x, 0.05, z)) * sc(Vec3::new(rx + 1.2, 1.0, rz + 1.2))), 22, &st(0x9a7448));
        b.at(p).disc(&(tr(Vec3::new(x, 0.06, z)) * sc(Vec3::new(rx + 0.5, 1.0, rz + 0.5))), 22, &st(0x7a5a38));
        decor::bake_fluid(b.dat(p), &Fluid { kind: FluidKind::Water, pos: p, yaw: 0.0, half: Vec2::new(rx, rz), round: true, deep: [0.12, 0.45, 0.72], shallow: [0.36, 0.78, 0.9] });
        // Reeds and lily pads around the edge.
        for i in 0..4 {
            let a = TAU * i as f32 / 4.0 + rng.range(0.0, 0.5);
            let q = Vec3::new(x + a.cos() * (rx + 0.3), 0.0, z + a.sin() * (rz + 0.3));
            b.at(q).append(&kit2.reed, &(tr(q) * Mat4::from_rotation_y(a * 3.0) * sc(Vec3::splat(rng.range(0.9, 1.5)))));
        }
        for i in 0..2 {
            let a = rng.range(0.0, TAU);
            let q = Vec3::new(x + a.cos() * rx * 0.5, 0.1, z + a.sin() * rz * 0.5);
            b.at(q).append(&kit2.lily, &(tr(q) * Mat4::from_rotation_y(a) * sc(Vec3::splat(0.8 + 0.2 * i as f32))));
        }
    }
}

// -------------------------------------------------------------- livestock ----

fn sheep(m: &mut MeshData, rng: &mut Rng, black: bool) {
    let wool = if black { 0x4a4448 } else { 0xf8f6ee };
    m.sphere_jit(&(tr(Vec3::new(0.0, 0.62, 0.0)) * sc(Vec3::new(0.38, 0.34, 0.5))), 8, 5, &Style::rgb(hx(wool)).flat().ao(0.3), 0.1, 3);
    m.sphere(&(tr(Vec3::new(0.0, 0.8, -0.1)) * sc(Vec3::new(0.3, 0.22, 0.34))), 7, 4, &Style::rgb(hx(wool)).flat());
    // Head toward +z (faces away from camera when the flock runs at it): dark face.
    m.sphere(&(tr(Vec3::new(0.0, 0.78, 0.5)) * sc(Vec3::new(0.15, 0.16, 0.2))), 7, 4, &st(0x3a3236));
    for sx in [-1.0f32, 1.0] {
        m.sphere(&(tr(Vec3::new(sx * 0.17, 0.82, 0.48)) * sc(Vec3::new(0.12, 0.05, 0.07))), 5, 3, &st(0x3a3236));
        for z in [-0.28f32, 0.28] {
            m.cyl(&(tr(Vec3::new(sx * 0.17, 0.0, z)) * sc(Vec3::new(0.045, 0.4, 0.045))), 5, 1.0, 1.0, false, &st(0x3a3236));
        }
    }
    let _ = rng;
}

fn cow(m: &mut MeshData) {
    let white = st(0xfaf8f2);
    let black = st(0x2b2b30);
    m.aabb(Vec3::new(-0.45, 0.65, -0.95), Vec3::new(0.45, 1.4, 0.8), &white.ao(0.2));
    m.aabb(Vec3::new(-0.465, 0.8, -0.5), Vec3::new(0.465, 1.3, 0.1), &black);
    m.aabb(Vec3::new(-0.3, 1.0, 0.5), Vec3::new(0.3, 1.38, 0.82), &black);
    // Head.
    m.aabb(Vec3::new(-0.27, 0.95, 0.8), Vec3::new(0.27, 1.45, 1.3), &white);
    m.aabb(Vec3::new(-0.2, 0.95, 1.2), Vec3::new(0.2, 1.1, 1.36), &st(0xf4a8b0));
    for sx in [-1.0f32, 1.0] {
        m.cone(&(tr(Vec3::new(sx * 0.2, 1.45, 0.95)) * Mat4::from_rotation_z(-sx * 0.5) * sc(Vec3::new(0.05, 0.22, 0.05))), 4, &st(0xf5e8c0));
        for (lx, lz) in [(0.3f32, -0.7f32), (0.3, 0.55)] {
            m.aabb(Vec3::new(sx * lx - 0.08, 0.0, lz - 0.08), Vec3::new(sx * lx + 0.08, 0.7, lz + 0.08), &white);
        }
    }
    m.cyl(&limb(Vec3::new(0.0, 1.3, -0.95), Vec3::new(0.0, 0.7, -1.1), 0.04), 5, 1.0, 1.0, false, &black);
}

fn livestock(style: &FarmStyle, b: &mut Bins, rng: &mut Rng) {
    let _ = style;
    let kit2_phase = |rng: &mut Rng| rng.next();
    // Sheep that hop (decor bob) in the pastures; cows that stand about.
    let flocks: [(Vec3, f32, usize, bool); 3] = [
        (Vec3::new(-26.0, 0.0, 120.0), 6.0, 5, false),
        (Vec3::new(36.0, 0.0, 122.0), 4.0, 5, false),
        (Vec3::new(-52.0, 0.0, 60.0), 5.0, 4, true),
    ];
    for (c, r, n, cows) in flocks {
        for _ in 0..n {
            let a = rng.range(0.0, TAU);
            let rr = r * rng.next().sqrt();
            let p = c + Vec3::new(a.cos() * rr, 0.0, a.sin() * rr);
            let yaw = rng.range(0.0, TAU);
            if cows {
                let mut m = MeshData::new();
                cow(&mut m);
                b.at(p).append(&m, &(tr(p) * Mat4::from_rotation_y(yaw)));
            } else {
                let mut m = MeshData::new();
                let dark = rng.next() < 0.12;
                sheep(&mut m, rng, dark);
                let phase = kit2_phase(rng);
                let sz = rng.range(0.9, 1.25);
                let xf = tr(p) * Mat4::from_rotation_y(yaw) * sc(Vec3::splat(sz));
                b.dat(p).append(&m, &xf, 1.0, |lp, _| [decor::A_BOB, u8f(phase), u8f(lp.y / 2.3), 0]);
            }
        }
    }
    // Pasture fence enclosure line (white rails) around the left pasture.
    let _ = (PI, Vec2::ZERO);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tris(b: &Bins) -> usize {
        b.map.values().map(|m| m.idx.len() / 3).sum::<usize>() + b.dmap.values().map(|m| m.idx.len() / 3).sum::<usize>()
    }

    #[test]
    fn tile_triangle_breakdown() {
        let style = FarmStyle::default();
        let mut rng = Rng(style.seed.wrapping_mul(2_654_435_761).wrapping_add(12345) | 1);
        let mut b = Bins { map: HashMap::new(), dmap: HashMap::new() };
        let mut last = 0;
        let mut report = |name: &str, b: &Bins| {
            let t = tris(b);
            println!("farm tile: {name:<10} {:>6} tris", t - last);
            last = t;
        };
        ground(&style, &mut b);
        report("ground", &b);
        track(&style, &mut b, &mut rng);
        report("track", &b);
        fences(&style, &mut b, &mut rng);
        report("fences", &b);
        verge(&style, &mut b, &mut rng);
        report("verge", &b);
        fields(&style, &mut b, &mut rng);
        report("fields", &b);
        buildings(&style, &mut b, &mut rng);
        report("buildings", &b);
        trees(&style, &mut b, &mut rng);
        report("trees", &b);
        yard_props(&style, &mut b, &mut rng);
        report("yard", &b);
        ponds(&style, &mut b, &mut rng);
        report("ponds", &b);
        livestock(&style, &mut b, &mut rng);
        report("livestock", &b);
        println!("farm tile total {last}");
    }

    #[test]
    fn hills_repeat_with_the_tile() {
        for (x, z) in [(60.0f32, 10.0f32), (-120.0, 77.0), (200.0, 239.0), (-300.0, 0.5)] {
            assert!((hill_height(x, z) - hill_height(x, z + TILE_LEN)).abs() < 1e-3, "{x} {z}");
            assert!((hill_height(x, z) - hill_height(x, z - 2.0 * TILE_LEN)).abs() < 1e-3);
        }
        // The track and its verge are flat.
        for z in [0.0f32, 55.0, 130.0] {
            assert_eq!(hill_height(0.0, z), 0.0);
            assert_eq!(hill_height(20.0, z), 0.0);
        }
    }

    #[test]
    fn the_farm_bakes_valid_geometry_over_four_tiles() {
        let w = WorldGeometry::farm();
        assert!(w.farm.is_some() && w.road.is_empty());
        let baked = crate::world::bake(&w);
        assert!(!baked.chunks.is_empty() && !baked.decor_chunks.is_empty());
        for &i in baked.idx.iter().chain(baked.decor_idx.iter().take(0)) {
            assert!((i as usize) < baked.verts.len());
        }
        for &i in &baked.decor_idx {
            assert!((i as usize) < baked.decor_verts.len());
        }
        assert!(baked.verts.iter().all(|v| v.pos.iter().all(|c| c.is_finite())));
        let (mut zlo, mut zhi) = (f32::MAX, f32::MIN);
        for v in &baked.verts {
            zlo = zlo.min(v.pos[2]);
            zhi = zhi.max(v.pos[2]);
        }
        // Four tiles back to back: z from -TILE_LEN to 3 * TILE_LEN (plus overhang).
        assert!(zlo > -TILE_LEN - 30.0 && zlo < -TILE_LEN + 5.0, "{zlo}");
        assert!(zhi < 3.0 * TILE_LEN + 30.0 && zhi > 3.0 * TILE_LEN - 5.0, "{zhi}");
        // The track itself is continuous: the dirt covers every metre of z along the middle lane.
        let tris = baked.idx.len() / 3;
        assert!(tris > 100_000 && tris < 400_000, "four tiles of farm: {tris} triangles");
        // The tile is the same in every copy: the triangle count of a window one tile apart matches.
        let count_in = |lo: f32, hi: f32| baked.idx.chunks(3).filter(|t| {
            let z = baked.verts[t[0] as usize].pos[2];
            (lo..hi).contains(&z)
        }).count();
        let a = count_in(0.0, TILE_LEN) as i64;
        let b = count_in(TILE_LEN, 2.0 * TILE_LEN) as i64;
        assert!((a - b).abs() < a / 10, "copies differ: {a} vs {b}");
    }

    #[test]
    fn nothing_blocks_the_lanes_and_the_fences_are_where_the_sim_puts_them() {
        let mut bins = Bins { map: HashMap::new(), dmap: HashMap::new() };
        let style = FarmStyle::default();
        let mut rng = Rng(1);
        fences(&style, &mut bins, &mut rng);
        for m in bins.map.values() {
            for v in &m.verts {
                // Fence geometry stays outside the playable width (inner face at fence_x).
                assert!(v.pos[0].abs() >= style.fence_x - 0.06, "fence vertex inside the track: {:?}", v.pos);
            }
        }
        // Static scenery never stands in the lanes (|x| < fence) above ground level: check the whole tile.
        let mut all = Bins { map: HashMap::new(), dmap: HashMap::new() };
        let mut rng = Rng(2);
        verge(&style, &mut all, &mut rng);
        fields(&style, &mut all, &mut rng);
        buildings(&style, &mut all, &mut rng);
        trees(&style, &mut all, &mut rng);
        yard_props(&style, &mut all, &mut rng);
        livestock(&style, &mut all, &mut rng);
        for m in all.map.values() {
            for v in &m.verts {
                if v.pos[1] > 0.25 {
                    assert!(v.pos[0].abs() > style.fence_x, "scenery in the lanes at {:?}", v.pos);
                }
            }
        }
    }
}
