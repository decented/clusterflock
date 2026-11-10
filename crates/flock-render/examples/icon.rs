//! Renders the Cluster Flock launcher icon: a Nostrich head and neck (googly eyes, crown) in front of a
//! sky-blue and green farm. The bird is drawn by the real renderer offscreen (the same capture path as
//! `farm_shot`) and matted out of its background by rendering the scene twice, with and without the bird;
//! the sky, sun, clouds and hills behind it are painted here, per output size.
//!
//! cargo run -p flock-render --example icon --release -- --res app/android/res [--preview DIR]
//!
//! Writes, into every `mipmap-*dpi` folder under `--res`: `ic_launcher_foreground.png`,
//! `ic_launcher_background.png`, `ic_launcher_monochrome.png` (108 dp adaptive layers), and the legacy
//! `ic_launcher.png` / `ic_launcher_round.png` (48 dp). `--preview DIR` also writes a 1024 px master and
//! 48 / 192 px renders of the legacy icon for eyeballing.
//! Env: WGPU_BACKEND=gl|vulkan.

use flock_render::capture::{Offscreen, SHOT_FORMAT};
use flock_render::*;
use glam::{Vec2, Vec3};
use std::path::{Path, PathBuf};

/// Hi-res working size of the adaptive canvas (108 dp square).
const HI: usize = 1296;
/// How much of the 108 dp canvas the bird's render frame covers. Launchers mask the adaptive icon to
/// its inner 66 dp, so the head must sit well inside; the frame's bottom edge is pinned to the canvas
/// bottom so the neck runs off the icon instead of ending in the middle of it.
const FRAME: f32 = 0.74;
/// Camera: vertical field of view, and how far above the head centre it aims (negative: the head sits high).
const FOV_DEG: f32 = 27.0;
const HEAD_LIFT: f32 = -0.3;
/// (folder, px per dp)
const DENSITIES: [(&str, f32); 5] = [("mipmap-mdpi", 1.0), ("mipmap-hdpi", 1.5), ("mipmap-xhdpi", 2.0), ("mipmap-xxhdpi", 3.0), ("mipmap-xxxhdpi", 4.0)];

fn headless() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::all()),
        ..wgpu::InstanceDescriptor::new_without_display_handle()
    });
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .expect("no adapter");
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("icon"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .expect("device")
}

fn write_png(path: &Path, w: usize, h: usize, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().unwrap();
    wr.write_image_data(rgba).unwrap();
    wr.finish().unwrap();
}

fn pose() -> OstrichPose {
    let mut p = OstrichPose::for_player(0);
    p.pos = Vec3::new(0.0, 0.0, 100.0);
    p.yaw = 0.0;
    p.speed = 0.0;
    p.neck_sway = Vec2::new(0.55, 0.35);
    p
}

/// The bird scene: the first player's Nostrich (red, crowned) seen from the front, tight on the head with
/// the neck leaving the bottom of the frame and a slight tilt for character. `bird` false renders the same
/// view without it, for the matte.
fn scene(bird: bool) -> FrameScene {
    let mut sc = FrameScene { time: 2.0, fog_scale: 0.4, ..Default::default() };
    let p = pose();
    // Aim at the head from in front of the beak, level with it, so only sky and distant hills are behind.
    let head = head_transform(&p);
    let centre = head.w_axis.truncate();
    let beak = Vec3::new(head.z_axis.x, 0.0, head.z_axis.z).normalize_or_zero();
    let beak = if beak == Vec3::ZERO { Vec3::Z } else { beak };
    if bird {
        sc.ostriches.push(p);
    }
    let eye = centre + beak * 3.2 + Vec3::Y * 0.15;
    let target = centre + Vec3::new(0.0, HEAD_LIFT, 0.0);
    sc.camera = Camera { eye, target, up: Vec3::Y, fov_y: FOV_DEG.to_radians(), near: 0.3, far: 300.0 };
    sc
}

/// Render `sc` at HI x HI and read it back.
fn render(device: &wgpu::Device, queue: &wgpu::Queue, r: &mut Renderer, off: &Offscreen, sc: &FrameScene) -> Vec<u8> {
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("icon") });
    r.render(device, queue, &mut enc, &off.view, sc, 1.0 / 60.0);
    queue.submit(std::iter::once(enc.finish()));
    off.read_rgba(device, queue).expect("read")
}

// ── painted background ──────────────────────────────────────────────────────

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn rgb(h: u32) -> [f32; 3] {
    [((h >> 16) & 255) as f32 / 255.0, ((h >> 8) & 255) as f32 / 255.0, (h & 255) as f32 / 255.0]
}

fn smooth(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Background colour at unit-square point (x, y), y down. Sky, sun, two clouds, rolling hills.
fn background(x: f32, y: f32) -> [f32; 3] {
    // sky
    let mut c = mix(rgb(0x3F8FE6), rgb(0xBFE3FA), smooth(0.0, 0.7, y));
    // sun and its halo, up on the right
    let (dx, dy) = (x - 0.72, y - 0.27);
    let d = (dx * dx + dy * dy).sqrt();
    c = mix(c, rgb(0xFFF3C4), 0.55 * (1.0 - smooth(0.04, 0.30, d)));
    c = mix(c, rgb(0xFFFBE0), 1.0 - smooth(0.085, 0.095, d));
    // clouds: a union of discs with a flat base
    for (cx, cy, s) in [(0.30, 0.30, 0.075), (0.74, 0.60, 0.055)] {
        let puffs = [(-0.9, 0.12, 0.5), (-0.35, -0.12, 0.65), (0.3, -0.05, 0.58), (0.9, 0.12, 0.45)];
        let mut inside = false;
        for (px, py, r) in puffs {
            let (ex, ey) = (x - (cx + px * s), y - (cy + py * s));
            inside |= ex * ex + ey * ey < (r * s) * (r * s);
        }
        inside |= (x - cx).abs() < 1.25 * s && y > cy + 0.1 * s && y < cy + 0.5 * s;
        if inside {
            c = mix(rgb(0xFFFFFF), rgb(0xD3E6F6), smooth(cy, cy + 0.5 * s, y) * 0.8);
        }
    }
    // far hill, middle hill, meadow
    let far = 0.66 + 0.035 * (x * 6.0 + 0.6).sin() + 0.015 * (x * 13.0).sin();
    if y > far {
        c = mix(rgb(0x8ED09A), rgb(0x6CC06C), smooth(far, 1.0, y));
    }
    let mid = 0.76 + 0.045 * (x * 4.2 + 2.4).sin() + 0.012 * (x * 11.0 + 1.0).sin();
    if y > mid {
        c = mix(rgb(0x5CB653), rgb(0x3F9A45), smooth(mid, 1.0, y));
    }
    c
}

fn paint_background(size: usize) -> Vec<u8> {
    let mut out = vec![0u8; size * size * 4];
    const SS: usize = 3;
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0.0f32; 3];
            for sy in 0..SS {
                for sx in 0..SS {
                    let u = (px as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let v = (py as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    let c = background(u, v);
                    for k in 0..3 {
                        acc[k] += c[k] / (SS * SS) as f32;
                    }
                }
            }
            let o = (py * size + px) * 4;
            for k in 0..3 {
                out[o + k] = (acc[k] * 255.0 + 0.5) as u8;
            }
            out[o + 3] = 255;
        }
    }
    out
}

// ── foreground: matte, outline, resample ────────────────────────────────────

/// Straight-alpha RGBA f32 layer.
struct Layer {
    w: usize,
    px: Vec<[f32; 4]>,
}

/// Bird colour + coverage mask from the with/without renders (sRGB bytes, HI x HI).
fn matte(with: &[u8], without: &[u8]) -> Layer {
    let px = (0..HI * HI)
        .map(|i| {
            let (a, b) = (&with[i * 4..i * 4 + 4], &without[i * 4..i * 4 + 4]);
            let d = (0..3).map(|k| (a[k] as i32 - b[k] as i32).abs()).max().unwrap();
            let m = if d > 2 { 1.0 } else { 0.0 };
            [a[0] as f32 / 255.0, a[1] as f32 / 255.0, a[2] as f32 / 255.0, m]
        })
        .collect();
    Layer { w: HI, px }
}

/// Lay a dark outline of `outline` canvas units (colour `col`) round the silhouette. The dilation runs
/// on a 1/3 scale mask (cheap) and is stamped back at full size.
fn add_outline(l: &Layer, outline: f32, col: [f32; 3]) -> Layer {
    const K: usize = 3;
    let (w, lw) = (l.w, l.w / K);
    let mut low = vec![false; lw * lw];
    for y in 0..lw * K {
        for x in 0..lw * K {
            if l.px[y * w + x][3] > 0.5 {
                low[(y / K) * lw + x / K] = true;
            }
        }
    }
    let r = ((outline * w as f32) / K as f32).round() as i32;
    let offs: Vec<(i32, i32)> = (-r..=r).flat_map(|dy| (-r..=r).map(move |dx| (dx, dy))).filter(|(dx, dy)| dx * dx + dy * dy <= r * r).collect();
    let mut grown = vec![false; lw * lw];
    for y in 0..lw as i32 {
        for x in 0..lw as i32 {
            if low[y as usize * lw + x as usize] {
                for &(dx, dy) in &offs {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx >= 0 && ny >= 0 && nx < lw as i32 && ny < lw as i32 {
                        grown[ny as usize * lw + nx as usize] = true;
                    }
                }
            }
        }
    }
    let mut out = Layer { w, px: l.px.clone() };
    for y in 0..lw * K {
        for x in 0..lw * K {
            let i = y * w + x;
            if l.px[i][3] <= 0.5 && grown[(y / K) * lw + x / K] {
                out.px[i] = [col[0], col[1], col[2], 1.0];
            }
        }
    }
    out
}

/// Box-resample the (cropped) layer to `size` px, weighting colour by coverage (so the edge carries no
/// background). `crop` = (x0, y0, side) in hi-res pixels.
fn resample(l: &Layer, crop: (f32, f32, f32), size: usize) -> Vec<[f32; 4]> {
    let (x0, y0, side) = crop;
    let step = side / size as f32;
    let mut out = vec![[0.0f32; 4]; size * size];
    for oy in 0..size {
        for ox in 0..size {
            let (xa, xb) = (x0 + ox as f32 * step, x0 + (ox + 1) as f32 * step);
            let (ya, yb) = (y0 + oy as f32 * step, y0 + (oy + 1) as f32 * step);
            let (mut c, mut wsum, mut asum) = ([0.0f32; 3], 0.0f32, 0.0f32);
            let (ix0, ix1) = (xa.floor() as i32, xb.ceil() as i32);
            let (iy0, iy1) = (ya.floor() as i32, yb.ceil() as i32);
            for iy in iy0..iy1 {
                for ix in ix0..ix1 {
                    let ov = (xb.min(ix as f32 + 1.0) - xa.max(ix as f32)).max(0.0) * (yb.min(iy as f32 + 1.0) - ya.max(iy as f32)).max(0.0);
                    wsum += ov;
                    if ix < 0 || iy < 0 || ix >= l.w as i32 || iy >= l.w as i32 {
                        continue;
                    }
                    let p = l.px[iy as usize * l.w + ix as usize];
                    asum += ov * p[3];
                    for k in 0..3 {
                        c[k] += ov * p[3] * p[k];
                    }
                }
            }
            let a = if wsum > 0.0 { asum / wsum } else { 0.0 };
            if asum > 0.0 {
                out[oy * size + ox] = [c[0] / asum, c[1] / asum, c[2] / asum, a];
            }
        }
    }
    out
}

fn to_bytes(l: &[[f32; 4]]) -> Vec<u8> {
    l.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect()
}

/// Straight-alpha layer over an opaque background, optionally masked by a rounded square or circle.
fn composite(fg: &[[f32; 4]], bg: &[u8], size: usize, shape: Shape) -> Vec<u8> {
    let mut out = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let i = y * size + x;
            let f = fg[i];
            let mut px = [0.0f32; 4];
            for k in 0..3 {
                px[k] = f[k] * f[3] + bg[i * 4 + k] as f32 / 255.0 * (1.0 - f[3]);
            }
            px[3] = shape.coverage((x as f32 + 0.5) / size as f32, (y as f32 + 0.5) / size as f32, size as f32);
            for k in 0..4 {
                out[i * 4 + k] = (px[k] * 255.0 + 0.5) as u8;
            }
        }
    }
    out
}

#[derive(Clone, Copy)]
enum Shape {
    Rounded,
    Circle,
}

impl Shape {
    fn coverage(self, u: f32, v: f32, size: f32) -> f32 {
        let (dx, dy) = ((u - 0.5).abs(), (v - 0.5).abs());
        let sd = match self {
            Shape::Circle => ((dx * dx + dy * dy).sqrt() - 0.5) * size,
            Shape::Rounded => {
                let r = 0.22;
                let (qx, qy) = (dx - (0.5 - r), dy - (0.5 - r));
                ((qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r) * size
            }
        };
        (0.5 - sd).clamp(0.0, 1.0)
    }
}

fn main() {
    let mut res = PathBuf::from("app/android/res");
    let mut preview: Option<PathBuf> = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--res" => res = PathBuf::from(args.next().expect("--res DIR")),
            "--preview" => preview = Some(PathBuf::from(args.next().expect("--preview DIR"))),
            _ => {}
        }
    }

    let (device, queue) = headless();
    let mut r = Renderer::new(&device, &queue, SHOT_FORMAT, Quality::Max);
    r.resize(HI as u32, HI as u32);
    let off = Offscreen::new(&device, HI as u32, HI as u32);
    r.load_world(&device, &queue, &WorldGeometry::farm());
    // Warm-up frame so lazily built resources do not change between the two renders.
    render(&device, &queue, &mut r, &off, &scene(false));
    let without = render(&device, &queue, &mut r, &off, &scene(false));
    let with = render(&device, &queue, &mut r, &off, &scene(true));

    let bird = matte(&with, &without);
    let fg = add_outline(&bird, 0.03, [0.106, 0.165, 0.361]);
    // Crop window (hi-res px) of the whole 108 dp canvas: the frame sits in its lower-middle.
    let side = HI as f32 / FRAME;
    let full = (-(side - HI as f32) / 2.0, HI as f32 - side, side);

    // adaptive layers (108 dp canvas)
    for (dir, dp) in DENSITIES {
        let size = (108.0 * dp) as usize;
        let layer = resample(&fg, full, size);
        let d = res.join(dir);
        write_png(&d.join("ic_launcher_foreground.png"), size, size, &to_bytes(&layer));
        write_png(&d.join("ic_launcher_background.png"), size, size, &paint_background(size));
        let mono: Vec<[f32; 4]> = layer.iter().map(|p| [0.0, 0.0, 0.0, p[3]]).collect();
        write_png(&d.join("ic_launcher_monochrome.png"), size, size, &to_bytes(&mono));
    }
    // legacy icons (48 dp): the visible 72 dp of the 108 dp canvas
    let legacy = |size: usize, shape: Shape| {
        let crop = (full.0 + full.2 / 6.0, full.1 + full.2 / 6.0, full.2 * 2.0 / 3.0);
        let layer = resample(&fg, crop, size);
        // the background crop: paint at canvas scale and cut the same window
        let big = paint_background((size * 3 / 2).max(1));
        let skip = (size * 3 / 2 - size) / 2;
        let mut bg = vec![0u8; size * size * 4];
        for y in 0..size {
            for x in 0..size {
                let s = ((y + skip) * (size * 3 / 2) + x + skip) * 4;
                bg[(y * size + x) * 4..(y * size + x) * 4 + 4].copy_from_slice(&big[s..s + 4]);
            }
        }
        composite(&layer, &bg, size, shape)
    };
    for (dir, dp) in DENSITIES {
        let size = (48.0 * dp) as usize;
        let d = res.join(dir);
        write_png(&d.join("ic_launcher.png"), size, size, &legacy(size, Shape::Rounded));
        write_png(&d.join("ic_launcher_round.png"), size, size, &legacy(size, Shape::Circle));
    }
    println!("wrote launcher icons under {}", res.display());

    if let Some(dir) = preview {
        for size in [48usize, 96, 192, 512] {
            write_png(&dir.join(format!("icon_{size}.png")), size, size, &legacy(size, Shape::Rounded));
        }
        write_png(&dir.join("icon_round_192.png"), 192, 192, &legacy(192, Shape::Circle));
        let master = resample(&fg, full, 864);
        write_png(&dir.join("icon_layers_fg.png"), 864, 864, &to_bytes(&master));
        write_png(&dir.join("with.png"), HI, HI, &with);
        println!("previews in {}", dir.display());
    }
}
