//! Offscreen verification shots of the Cluster Flock look: a portrait runner-camera scene with six
//! Nostriches in different states, hay, a three-lane beam, a tractor, mud and coins, and a ghost
//! camera scene with the placement grid.
//!
//! cargo run -p flock-render --example farm_shot --release -- --out docs/notes/shots
//! Env: SHOT_W / SHOT_H (default 1080x2400), SHOT_QUALITY=battery|balanced|max (default max), WGPU_BACKEND=gl|vulkan.

use flock_render::capture::{Offscreen, SHOT_FORMAT};
use flock_render::*;
use glam::{Vec2, Vec3};
use std::path::{Path, PathBuf};

const LANE_W: f32 = 2.2;
/// Render-space x of a sim lane centre: lane 0 (left) is at +x, because the renderer's right is -x.
fn lane_x(lane: f32) -> f32 {
    -(lane - 1.0) * LANE_W
}

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
    let info = adapter.get_info();
    eprintln!("adapter: {} ({:?})", info.name, info.backend);
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("farm_shot"),
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        ..Default::default()
    }))
    .expect("device")
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header().unwrap();
    wr.write_image_data(rgba).unwrap();
    wr.finish().unwrap();
}

fn bird(player: u8, lane: f32, z: f32) -> OstrichPose {
    OstrichPose { pos: Vec3::new(lane_x(lane), 0.0, z), speed: 14.0, ..OstrichPose::for_player(player) }
}

/// A gate as the sim describes it today: underside 1.15, collision top 1.95, 0.4 deep.
fn gate(lanes: u8, x: f32, z: f32) -> FarmPropInst {
    let mut g = FarmPropInst::new(FarmPropKind::Beam { span_lanes: lanes }, Vec3::new(x, 0.0, z), Vec3::new(if lanes == 3 { 7.4 } else { 2.0 }, 1.15, 0.4));
    g.bar_top = 1.95;
    g
}

/// A barrel as the sim has it (1.2 wide, radius 0.35) lying across the lane, rolling: `roll = z / radius`.
fn barrel(x: f32, z: f32) -> FarmPropInst {
    let mut b = FarmPropInst::new(FarmPropKind::Barrel, Vec3::new(x, 0.0, z), Vec3::new(1.2, 0.7, 0.7));
    b.roll = z / 0.35;
    b
}

fn coin(x: f32, y: f32, z: f32, t: f32) -> CoinInst {
    CoinInst { pos: Vec3::new(x, y, z), spin: t * 3.0 + z * 0.4, visible: true }
}

/// Representative runner scene at `time` (birds animated from the clock).
fn scene(anchor: f32, time: f32, aspect: f32) -> FrameScene {
    let mut sc = FrameScene { camera: Camera::runner(anchor, aspect), time, ..Default::default() };
    let ph = time * 2.3;
    // 0: the local player, running in the middle lane, outlined.
    let mut p0 = bird(0, 1.0, anchor + 0.5);
    p0.gait_phase = ph;
    p0.speed = 16.0;
    p0.outline = Some([1.0, 0.98, 0.55]);
    p0.neck_sway = Vec2::new(0.25 * (time * 3.0).sin(), 0.2);
    p0.body_roll = 0.05 * (time * 3.0).sin();
    // 1: wind-up squash before a jump, left lane.
    let mut p1 = bird(1, 0.0, anchor + 4.0);
    p1.squash = 0.9;
    p1.speed = 6.0;
    p1.gait_phase = ph + 0.3;
    p1.neck_sway = Vec2::new(0.0, -0.5);
    // 2: airborne with flapping wings and scrambling legs, right lane.
    let mut p2 = bird(2, 2.0, anchor + 8.0);
    p2.pos.y = 1.2;
    p2.airborne = true;
    p2.tuck = 0.5;
    p2.wing_flap = 1.0;
    p2.scramble = 0.6;
    p2.neck_sway = Vec2::new(-0.3, 0.4);
    p2.body_pitch = -0.15;
    // 3: tuck-and-roll in the middle lane, further on (ball radius BALL_RADIUS, rolling 8 m/s).
    let mut p3 = bird(3, 1.0, anchor + 12.0);
    p3.ball = 1.0;
    p3.roll = (time * 8.0) / BALL_RADIUS;
    p3.speed = 8.0;
    // 4: tumbling after a crash, left lane.
    let mut p4 = bird(4, 0.0, anchor + 15.0);
    p4.tumble = 1.7;
    p4.scramble = 1.0;
    p4.speed = 4.0;
    p4.pos.y = 0.5;
    p4.body_pitch = 0.2;
    p4.neck_sway = Vec2::new(0.6, 0.1);
    // 5: protected with stars, right lane, just behind the pack.
    let mut p5 = bird(5, 2.0, anchor + 1.0);
    p5.stars = 1.0;
    p5.speed = 15.0;
    p5.gait_phase = ph + 0.5;
    // A ghost floating above the field.
    let mut g = bird(2, 0.0, anchor + 10.5);
    g.ghost = 1.0;
    g.speed = 0.0;
    g.neck_sway = Vec2::new(0.3, 0.2);
    sc.ostriches = vec![p0, p1, p2, p3, p4, p5, g];
    for (i, o) in sc.ostriches.iter().enumerate() {
        if !o.airborne && o.collapsed < 0.5 && o.ghost < 0.5 && o.ball < 0.5 {
            sc.emitters.push(Emitter {
                racer: i,
                dust: 0.7,
                dust_color: [0.78, 0.6, 0.38],
                drift_tier: 0,
                drifting: false,
                wheel: 0.0,
                offroad: false,
                boost: false,
                burning: false,
            });
        }
    }
    // Obstacles ahead.
    sc.farm_props.push(FarmPropInst::new(FarmPropKind::Mud, Vec3::new(lane_x(1.0), 0.0, anchor + 27.0), Vec3::new(2.0, 1.0, 6.0)));
    sc.farm_props.push(FarmPropInst::new(FarmPropKind::HayBale, Vec3::new(lane_x(0.0), 0.0, anchor + 24.0), Vec3::new(1.6, 0.7, 1.0)));
    sc.farm_props.push(barrel(lane_x(2.0), anchor + 31.0 - (time - 3.0) * 6.0));
    sc.farm_props.push(gate(3, 0.0, anchor + 38.0));
    sc.farm_props.push(FarmPropInst::new(FarmPropKind::Block { variant: BlockVariant::Tractor }, Vec3::new(lane_x(2.0), 0.0, anchor + 46.0), Vec3::new(2.0, 2.5, 3.0)));
    sc.farm_props.push(FarmPropInst::new(FarmPropKind::Block { variant: BlockVariant::BarnWall }, Vec3::new(lane_x(0.0), 0.0, anchor + 50.0), Vec3::new(2.0, 2.5, 3.0)));
    sc.farm_props.push(gate(1, lane_x(1.0), anchor + 58.0));
    sc.farm_props.push(gate(1, lane_x(0.0), anchor + 17.0));
    sc.farm_props.push(barrel(lane_x(0.0), anchor + 41.0 + (time - 3.0) * 4.0));
    sc.farm_props.push(FarmPropInst::new(FarmPropKind::Block { variant: BlockVariant::Trough }, Vec3::new(lane_x(1.0), 0.0, anchor + 66.0), Vec3::new(2.0, 2.5, 3.0)));
    // Coin lines.
    for i in 0..6 {
        sc.coins.push(coin(lane_x(1.0), 0.8, anchor + 19.0 + i as f32 * 1.4, time));
    }
    for i in 0..7 {
        let t = i as f32 / 6.0;
        sc.coins.push(coin(lane_x(2.0), 0.8 + 1.4 * (t * std::f32::consts::PI).sin(), anchor + 26.0 + i as f32 * 1.1, time));
    }
    for i in 0..5 {
        sc.coins.push(coin(lane_x(0.0), 0.8, anchor + 33.0 + i as f32 * 1.4, time));
    }
    sc
}

struct Shooter {
    device: wgpu::Device,
    queue: wgpu::Queue,
    r: Renderer,
    off: Offscreen,
    out: PathBuf,
}

impl Shooter {
    fn frame(&mut self, sc: &FrameScene, dt: f32) {
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("shot") });
        self.r.render(&self.device, &self.queue, &mut enc, &self.off.view, sc, dt);
        self.queue.submit(std::iter::once(enc.finish()));
    }

    fn save(&mut self, name: &str, sc: &FrameScene) {
        self.frame(sc, 1.0 / 60.0);
        let st = self.r.stats();
        let rgba = self.off.read_rgba(&self.device, &self.queue).expect("read");
        write_png(&self.out.join(format!("{name}.png")), self.off.size.0, self.off.size.1, &rgba);
        println!("{name:<14} draw_calls={:<4} triangles={:<7} particles={}", st.draw_calls, st.triangles, st.particles);
    }
}

fn main() {
    let mut out = PathBuf::from("docs/notes/shots");
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--out" {
            out = PathBuf::from(args.next().expect("--out DIR"));
        }
    }
    let (w, h) = (
        std::env::var("SHOT_W").ok().and_then(|v| v.parse().ok()).unwrap_or(1080u32),
        std::env::var("SHOT_H").ok().and_then(|v| v.parse().ok()).unwrap_or(2400u32),
    );
    let (device, queue) = headless();
    let quality = match std::env::var("SHOT_QUALITY").as_deref() {
        Ok("battery") => Quality::Battery,
        Ok("balanced") => Quality::Balanced,
        _ => Quality::Max,
    };
    let mut r = Renderer::new(&device, &queue, SHOT_FORMAT, quality);
    r.resize(w, h);
    let off = Offscreen::new(&device, w, h);
    let t_load = std::time::Instant::now();
    r.load_world(&device, &queue, &WorldGeometry::farm());
    println!("load_world (4 farm tiles): {:.0} ms", t_load.elapsed().as_secs_f64() * 1000.0);
    let mut sh = Shooter { device, queue, r, off, out };

    // Floating origin: the sim distance is far past one tile; the app subtracts a multiple of TILE_LEN.
    let sim_anchor = 5000.5f32;
    let origin = (sim_anchor / TILE_LEN).floor() * TILE_LEN;
    let anchor = sim_anchor - origin;
    println!("sim anchor {sim_anchor} -> origin {origin}, render anchor {anchor}");

    // Run particles forward with bursts at chosen moments so they are mid-flight at the shot.
    let t0 = 3.0f32;
    let frames = 100;
    let aspect = w as f32 / h as f32;
    let mut sc = scene(anchor, t0, aspect);
    let mut dust_cell: Vec<i64> = Vec::new();
    for f in 0..frames {
        let t = t0 + f as f32 / 60.0;
        sc = scene(anchor, t, aspect);
        // A puff of dust behind every rolling barrel each 1.1 m it travels.
        let barrels: Vec<(i64, Vec3, f32)> = sc.farm_props.iter().filter(|p| p.kind == FarmPropKind::Barrel).enumerate().map(|(i, p)| (i as i64, p.pos, if i == 0 { 1.0 } else { -1.0 })).collect();
        dust_cell.resize(barrels.len(), i64::MIN);
        for (i, pos, dir) in barrels {
            let cell = (pos.z / 1.1).floor() as i64;
            if dust_cell[i as usize] != cell && dust_cell[i as usize] != i64::MIN {
                sc.bursts.push(Burst::new(BurstKind::DustTrail, pos + Vec3::Z * dir * 0.35, 4));
            }
            dust_cell[i as usize] = cell;
        }
        if f == 55 {
            let p4 = sc.ostriches[4].pos;
            sc.bursts.push(Burst::new(BurstKind::Feathers, p4, 18).with_color(PlayerStyle::of(4).body));
            sc.bursts.push(Burst::new(BurstKind::HitPuff, p4 + Vec3::Y * 0.8, 10));
            // A crash right beside the lens (a bird behind the camera's runner): must stay small.
            let near = Vec3::new(lane_x(2.0), 0.9, anchor - 4.5);
            sc.bursts.push(Burst::new(BurstKind::Feathers, near, 18).with_color(PlayerStyle::of(1).body));
            sc.bursts.push(Burst::new(BurstKind::HitPuff, near, 10));
            sc.bursts.push(Burst::new(BurstKind::MudSplat, Vec3::new(lane_x(1.0), 0.0, anchor + 26.0), 22));
            sc.bursts.push(Burst::new(BurstKind::GhostPoof, sc.ostriches[6].pos + Vec3::Y * 0.6, 14).with_color(PlayerStyle::of(2).body));
        }
        if f == 80 {
            sc.bursts.push(Burst::new(BurstKind::CoinSparkle, Vec3::new(lane_x(1.0), 0.9, anchor + 20.0), 12));
            sc.bursts.push(Burst::new(BurstKind::LandingDust, Vec3::new(lane_x(2.0), 0.0, anchor + 8.0), 24));
        }
        sh.r.step_particles(&sc, 1.0 / 60.0);
    }
    sc.bursts.clear();
    sh.save("farm_runner", &sc);
    // Where do the triangles go? World only / dynamic meshes only.
    for (name, d) in [("  world only", Diag { meshes: false, ..Diag::default() }), ("  meshes only", Diag { world: false, ..Diag::default() })] {
        sh.r.set_diag(d);
        sh.frame(&sc, 0.0);
        let st = sh.r.stats();
        println!("{name:<14} draw_calls={:<4} triangles={}", st.draw_calls, st.triangles);
    }
    sh.r.set_diag(Diag::default());

    // Ghost camera with the placement grid over the band ahead of the pack.
    let mut gs = sc.clone();
    gs.camera = Camera::ghost(anchor);
    gs.fog_scale = 0.7;
    gs.farm_props.clear();
    gs.coins.truncate(6);
    gs.placement.clear();
    let first_row = ((anchor + 30.0) / 6.0).floor() as i32;
    for row in 0..8 {
        for lane in 0..3 {
            let z = (first_row + row) as f32 * 6.0 + 3.0;
            let state = match (row, lane) {
                (0, _) => CellState::Invalid,
                (3, 1) => CellState::Selected,
                (2, 2) | (5, 0) => CellState::Reserved,
                _ => CellState::Valid,
            };
            gs.placement.push(PlacementCell { pos: Vec3::new(lane_x(lane as f32), 0.0, z), size: Vec2::new(LANE_W - 0.2, 5.6), state });
        }
    }
    let z_of = |row: i32| (first_row + row) as f32 * 6.0 + 3.0;
    let mut reserve = FarmPropInst::new(FarmPropKind::HayBale, Vec3::new(lane_x(2.0), 0.0, z_of(2)), Vec3::new(1.6, 0.85, 1.0));
    reserve.alpha = 0.55;
    gs.farm_props.push(reserve);
    let mut beam_res = FarmPropInst::new(FarmPropKind::Beam { span_lanes: 1 }, Vec3::new(lane_x(0.0), 0.0, z_of(5)), Vec3::new(2.2, 1.0, 0.4));
    beam_res.alpha = 0.55;
    gs.farm_props.push(beam_res);
    let mut bad = FarmPropInst::new(FarmPropKind::HayBale, Vec3::new(lane_x(1.0), 0.0, z_of(0)), Vec3::new(1.6, 0.85, 1.0));
    bad.alpha = 0.6;
    bad.danger = true;
    gs.farm_props.push(bad);
    let mut dropping = FarmPropInst::new(FarmPropKind::Block { variant: BlockVariant::Tractor }, Vec3::new(lane_x(1.0), 0.0, z_of(3)), Vec3::new(2.0, 2.5, 3.0));
    dropping.drop = 0.7;
    dropping.alpha = 0.7;
    gs.farm_props.push(dropping);
    gs.farm_props.push(FarmPropInst::new(FarmPropKind::Block { variant: BlockVariant::BarnWall }, Vec3::new(lane_x(0.0), 0.0, z_of(7)), Vec3::new(2.0, 2.5, 3.0)));
    gs.farm_props.push(FarmPropInst::new(FarmPropKind::Mud, Vec3::new(lane_x(2.0), 0.0, z_of(6)), Vec3::new(2.0, 1.0, 6.0)));
    sh.save("farm_ghost", &gs);

    // Gates close up with a rolling bird under one and a standing bird for scale.
    let mut gt = FrameScene { camera: Camera::runner(anchor, aspect), time: 3.0, ..Default::default() };
    let mut roller = bird(0, 1.0, anchor + 4.5);
    roller.ball = 1.0;
    roller.roll = 1.0;
    roller.outline = Some([1.0, 0.98, 0.55]);
    roller.speed = 14.0;
    let mut stander = bird(3, 0.0, anchor + 9.0);
    stander.gait_phase = 0.3;
    let mut stander2 = bird(2, 2.0, anchor + 14.0);
    stander2.gait_phase = 0.6;
    gt.ostriches = vec![roller, stander, stander2];
    gt.farm_props.push(gate(1, lane_x(1.0), anchor + 9.0));
    gt.farm_props.push(gate(1, lane_x(0.0), anchor + 20.0));
    gt.farm_props.push(gate(3, 0.0, anchor + 30.0));
    gt.farm_props.push(FarmPropInst::new(FarmPropKind::HayBale, Vec3::new(lane_x(2.0), 0.0, anchor + 9.0), Vec3::new(1.6, 0.7, 1.0)));
    gt.farm_props.push(barrel(lane_x(2.0), anchor + 24.0));
    sh.save("farm_gate", &gt);

    // A lineup seen from the front and above (faces, accessories, outline), for checking the art.
    let mut lu = FrameScene { time: 2.0, ..Default::default() };
    for i in 0..6u8 {
        let mut p = OstrichPose::for_player(i);
        p.pos = Vec3::new(-6.25 + 2.5 * i as f32, 0.0, 100.0);
        p.yaw = 0.0;
        p.speed = 0.0;
        p.neck_sway = Vec2::new(0.2, 0.0);
        match i {
            0 => p.outline = Some([1.0, 0.98, 0.55]),
            1 => p.squash = 0.8,
            2 => {
                p.airborne = true;
                p.pos.y = 0.8;
                p.wing_flap = 1.0;
                p.scramble = 0.8;
            }
            3 => p.collapsed = 1.0,
            4 => {
                p.stars = 1.0;
            }
            _ => p.ghost = 1.0,
        }
        lu.ostriches.push(p);
    }
    lu.camera = Camera { eye: Vec3::new(0.0, 3.4, 110.0), target: Vec3::new(0.0, 1.3, 100.0), up: Vec3::Y, fov_y: 60f32.to_radians(), near: 0.4, far: 300.0 };
    let mut sh2 = sh;
    sh2.r.resize(1600, 900);
    sh2.off = Offscreen::new(&sh2.device, 1600, 900);
    sh2.save("farm_lineup", &lu);

    // The tuck-and-roll ball over a full turn, from the side and slightly above.
    let mut bl = FrameScene { time: 2.0, ..Default::default() };
    for i in 0..8 {
        let mut p = OstrichPose::for_player((i % 6) as u8);
        p.pos = Vec3::new(0.0, 0.0, 100.0 + 1.55 * (i as f32 - 3.5));
        p.ball = if i == 0 { 0.0 } else { 1.0 };
        p.roll = std::f32::consts::TAU * (i as f32 - 1.0).max(0.0) / 7.0;
        p.speed = 8.0;
        bl.ostriches.push(p);
    }
    bl.camera = Camera { eye: Vec3::new(1.0, 3.0, 106.0), target: Vec3::new(0.0, 0.8, 100.0), up: Vec3::Y, fov_y: 50f32.to_radians(), near: 0.4, far: 300.0 };
    bl.camera.eye = Vec3::new(-5.5, 3.6, 100.0);
    bl.camera.target = Vec3::new(0.0, 0.5, 100.0);
    bl.camera.fov_y = 62f32.to_radians();
    sh2.save("farm_ball", &bl);

    // Props close up: barrels at three roll angles, bales, a gate.
    let mut pr = FrameScene { time: 2.0, ..Default::default() };
    for (k, x) in [-2.2f32, 0.0, 2.2].into_iter().enumerate() {
        let mut b = barrel(x, 92.0);
        b.roll = 0.9 * k as f32;
        pr.farm_props.push(b);
        pr.farm_props.push(FarmPropInst::new(FarmPropKind::HayBale, Vec3::new(x, 0.0, 95.0), Vec3::new(1.6, 0.7, 1.0)));
    }
    pr.farm_props.push(gate(1, 0.0, 99.0));
    pr.camera = Camera { eye: Vec3::new(0.0, 2.6, 86.0), target: Vec3::new(0.0, 0.7, 96.0), up: Vec3::Y, fov_y: 60f32.to_radians(), near: 0.4, far: 300.0 };
    sh2.save("farm_props", &pr);
}
