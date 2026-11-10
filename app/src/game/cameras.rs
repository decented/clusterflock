//! Cameras: the runner camera follows the pack anchor with easing only (no
//! input); the ghost (architect) camera sits far beyond the placement band and
//! looks BACK down the course at the flock running toward it, so the cells you
//! can place on fill the lower part of the screen and the survivors are visible
//! beyond them; becoming a ghost swings the view over the pack and round to
//! that side in [`GLIDE_SECS`]. Also the screen shake (scaled down by the
//! reduced-shake setting) and the projection helpers the HUD and ghost picking use.

use egui::{pos2, Pos2, Vec2};
use flock_render::Camera;
use glam::{Mat4, Quat, Vec3, Vec4};

/// Seconds the camera glides from the runner view to the ghost view.
pub const GLIDE_SECS: f32 = 0.8;
/// Time constant of the anchor-following ease.
const FOLLOW_TAU: f32 = 0.16;

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Default)]
pub struct CameraRig {
    run_z: f64,
    ghost_z: f64,
    run_x: f32,
    started: bool,
    /// 0 = runner camera, 1 = ghost camera.
    blend: f32,
    /// Remaining shake energy 0..1.
    shake: f32,
    /// The tool column is on the left (the ghost view keeps its cells clear of it).
    pub left_handed: bool,
    pub camera: Camera,
}

/// Screen aspect (width / height) the ghost view is tuned for: a 412 x 915 dp phone.
pub const GHOST_REF_ASPECT: f32 = 412.0 / 915.0;
/// Ghost camera: height above the ground, distance beyond the placement band's far edge, how far
/// down the course it looks, vertical field of view (degrees, at the reference aspect), and the
/// sideways offsets of the eye and the look-at point (render x, for a right-handed tool column).
/// Found by searching the 412 x 915 dp layout so that every legal cell stays at least 44 x 30 dp
/// and clear of the HUD while the whole pack stays on screen: a long lens a long way back.
const GHOST_HEIGHT: f32 = 32.97;
const GHOST_BEHIND: f32 = 90.73;
const GHOST_LOOK: f32 = 132.31;
const GHOST_FOV_DEG: f32 = 15.65;
const GHOST_EYE_X: f32 = 2.49;
const GHOST_TARGET_X: f32 = -1.15;

/// The ghost camera. `far_z` is the render-space z of the placement band's far edge (the camera
/// sits beyond it and looks back toward the pack, so render +x is on the screen's right).
pub fn ghost_camera(far_z: f32, aspect: f32, left_handed: bool) -> Camera {
    let side = if left_handed { -1.0 } else { 1.0 };
    let eye = Vec3::new(GHOST_EYE_X * side, GHOST_HEIGHT, far_z + GHOST_BEHIND);
    let target = Vec3::new(GHOST_TARGET_X * side, 0.0, far_z + GHOST_BEHIND - GHOST_LOOK);
    // Wider than the reference aspect: same vertical view. Narrower: same horizontal view.
    let tan_y = (GHOST_FOV_DEG.to_radians() * 0.5).tan() * (GHOST_REF_ASPECT / aspect.max(0.2)).max(1.0);
    Camera { eye, target, up: Vec3::Y, fov_y: 2.0 * tan_y.atan(), near: 2.0, far: 560.0 }
}

/// Blend two cameras: eye and field of view linearly, the look direction round the shortest arc,
/// so the swing from the runner view to the ghost view goes over the top of the pack.
fn blend_cameras(a: &Camera, b: &Camera, t: f32) -> Camera {
    let (da, db) = ((a.target - a.eye), (b.target - b.eye));
    let (la, lb) = (da.length().max(1e-3), db.length().max(1e-3));
    let q = Quat::from_rotation_arc(da / la, db / lb);
    let dir = Quat::IDENTITY.slerp(q, t) * (da / la);
    let eye = a.eye.lerp(b.eye, t);
    Camera { eye, target: eye + dir * (la + (lb - la) * t), up: Vec3::Y, fov_y: a.fov_y + (b.fov_y - a.fov_y) * t, near: a.near + (b.near - a.near) * t, far: a.far + (b.far - a.far) * t }
}

impl CameraRig {
    pub fn new() -> CameraRig {
        CameraRig::default()
    }

    /// Jump straight to the poses for these values (first frame).
    pub fn snap(&mut self) {
        self.started = false;
    }

    /// Smoothly return toward the runner view (new round).
    pub fn is_ghost_view(&self) -> bool {
        self.blend > 0.5
    }

    /// A bump or crash: add shake.
    pub fn kick(&mut self, amount: f32) {
        self.shake = (self.shake + amount).min(1.0);
    }

    /// Advance by `dt`. `anchor` and `focus` are sim z (f64) of the pack
    /// anchor and of the placement band's far edge (what the ghost view hangs from); `origin` is
    /// the floating origin; `local_x` the local runner's sim x (a gentle
    /// sideways follow).
    pub fn update(&mut self, dt: f32, anchor: f64, focus: f64, origin: f64, local_x: f32, ghost: bool, aspect: f32, fov_extra: f32) {
        if !self.started {
            self.run_z = anchor;
            self.ghost_z = focus;
            self.run_x = 0.0;
            self.blend = if ghost { 1.0 } else { 0.0 };
            self.started = true;
        }
        let k = 1.0 - (-dt / FOLLOW_TAU).exp();
        self.run_z += (anchor - self.run_z) * k as f64;
        self.ghost_z += (focus - self.ghost_z) * k as f64;
        self.run_x += (local_x * 0.22 - self.run_x) * (1.0 - (-dt / 0.35).exp());
        let target = if ghost { 1.0 } else { 0.0 };
        let step = dt / GLIDE_SECS;
        self.blend = if self.blend < target { (self.blend + step).min(target) } else { (self.blend - step).max(target) };
        self.shake = (self.shake - dt * 2.4).max(0.0);

        let mut run = Camera::runner((self.run_z - origin) as f32, aspect);
        // speed feel: the runner view opens up a little (radians; the ghost view is untouched)
        run.fov_y += fov_extra;
        // gentle sideways follow (render x is mirrored)
        run.eye.x -= self.run_x;
        run.target.x -= self.run_x * 0.5;
        let gh = ghost_camera((self.ghost_z - origin) as f32, aspect, self.left_handed);
        let b = smooth(self.blend);
        self.camera = if b <= 0.0 {
            run
        } else if b >= 1.0 {
            gh
        } else {
            blend_cameras(&run, &gh, b)
        };
    }

    /// World-space shake offset for `FrameScene::shake`. `scale` 1 normal,
    /// about 0.2 for reduced shake, 0 off.
    pub fn shake_offset(&self, time: f32, scale: f32) -> Vec3 {
        // the ghost view is a long way off through a long lens: a knock has to be bigger to show
        let a = self.shake * self.shake * 0.34 * scale * (1.0 + 5.0 * self.glide());
        if a <= 0.001 {
            return Vec3::ZERO;
        }
        Vec3::new((time * 53.0).sin(), (time * 61.0 + 1.3).sin() * 0.7, 0.0) * a
    }

    /// How far into the glide (0..1, smoothed): the instruction banner uses it.
    pub fn glide(&self) -> f32 {
        smooth(self.blend)
    }
}

/// Projects render-space points to screen points and back.
pub struct Projector {
    vp: Mat4,
    inv: Mat4,
    size: Vec2,
}

impl Projector {
    /// `size` in UI points (the full screen).
    pub fn new(cam: &Camera, size: Vec2) -> Projector {
        let vp = cam.view_proj(size.x / size.y.max(1.0));
        Projector { vp, inv: vp.inverse(), size }
    }

    /// Screen point of `p`, or `None` behind the camera.
    pub fn project(&self, p: Vec3) -> Option<Pos2> {
        let c = self.vp * Vec4::new(p.x, p.y, p.z, 1.0);
        if c.w <= 0.05 {
            return None;
        }
        let n = c.truncate() / c.w;
        Some(pos2((n.x * 0.5 + 0.5) * self.size.x, (0.5 - n.y * 0.5) * self.size.y))
    }

    /// Screen scale at `p`: points per metre (horizontal), for sizing markers.
    pub fn scale_at(&self, p: Vec3) -> f32 {
        match (self.project(p), self.project(p + Vec3::X)) {
            (Some(a), Some(b)) => (a - b).length(),
            _ => 0.0,
        }
    }

    /// Where the finger's ray meets the horizontal plane at height `y`.
    pub fn ground(&self, s: Pos2, y: f32) -> Option<Vec3> {
        let nx = s.x / self.size.x * 2.0 - 1.0;
        let ny = 1.0 - s.y / self.size.y * 2.0;
        let a = self.inv * Vec4::new(nx, ny, 0.0, 1.0);
        let b = self.inv * Vec4::new(nx, ny, 1.0, 1.0);
        let (a, b) = (a.truncate() / a.w, b.truncate() / b.w);
        let d = b - a;
        if d.y.abs() < 1e-5 {
            return None;
        }
        let t = (y - a.y) / d.y;
        (t > 0.0).then(|| a + d * t)
    }
}

/// Sim to render space. The ONLY place sim x is mirrored (the renderer's
/// right for yaw 0 is -x).
#[inline]
pub fn to_render(sim_x: f32, sim_y: f32, sim_z: f64, origin: f64) -> Vec3 {
    Vec3::new(-sim_x, sim_y, (sim_z - origin) as f32)
}

/// Render x back to sim x.
#[inline]
pub fn to_sim_x(render_x: f32) -> f32 {
    -render_x
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    #[test]
    fn project_and_pick_round_trip() {
        let cam = Camera::runner(0.0, 0.45);
        let pr = Projector::new(&cam, vec2(412.0, 915.0));
        let p = Vec3::new(-2.2, 0.0, 10.0);
        let s = pr.project(p).unwrap();
        let g = pr.ground(s, 0.0).unwrap();
        assert!((g - p).length() < 0.05, "{g:?} vs {p:?}");
    }

    /// The architect view at 412 x 915 dp over the multiplayer speed range and every phase of the
    /// row grid: legal cells are big enough and clear of the HUD, the pack is on screen. Prints the
    /// worst cases (run with `--nocapture`).
    #[test]
    fn architect_view_numbers_at_412_by_915() {
        use crate::ui::game::hud;
        use egui::{pos2, vec2, Rect};
        let size = vec2(412.0, 915.0);
        let screen = Rect::from_min_size(pos2(0.0, 0.0), size);
        let aspect = size.x / size.y;
        let (lane_w, sp) = (2.2f64, 6.0f64);
        let (cell_w, cell_d) = (2.0f32, 5.6f32);
        let (mut min_cw, mut min_ch) = (f32::MAX, f32::MAX);
        let (mut min_rh, mut min_rtop, mut max_rbot) = (f32::MAX, f32::MAX, 0.0f32);
        let mut min_cell_top = f32::MAX;
        let mut cells_seen = 0;
        for left in [false, true] {
            let rc = hud::layout(screen, hud::Mode { solo: false, pause: true, ghost: true, left_handed: left });
            let pill = crate::ui::game::ghost_ui::living_pill(rc.banner, left);
            let hidden: Vec<Rect> = [rc.energy, rc.banner, pill, rc.info].into_iter().chain(rc.tools).chain(rc.pause).collect();
            let mut v = 13.0f64;
            while v <= 16.001 {
                let mut ph = 0.0f64;
                while ph < 6.0 {
                    let lead = 600.0 + ph; // render z = sim z here (origin 0), any whole-row offset is the same
                    let reach = v + 6.0;
                    let (near, far) = (lead + 3.0 * reach, lead + 5.0 * reach);
                    let cam = ghost_camera(far as f32, aspect, left);
                    let pr = Projector::new(&cam, size);
                    for row in (near / sp).ceil() as i64..=(far / sp).floor() as i64 {
                        for lane in 0..3 {
                            let (cx, cz) = (-((lane as f32 - 1.0) * lane_w as f32), (row as f64 * sp) as f32);
                            let p = |dx: f32, dz: f32| pr.project(Vec3::new(cx + dx, 0.0, cz + dz)).expect("in front of the camera");
                            let (nl, nr, fl, fr) = (p(-cell_w / 2.0, cell_d / 2.0), p(cell_w / 2.0, cell_d / 2.0), p(-cell_w / 2.0, -cell_d / 2.0), p(cell_w / 2.0, -cell_d / 2.0));
                            let c = p(0.0, 0.0);
                            // z - is further from the camera: that edge is the narrow one
                            min_cw = min_cw.min((fr.x - fl.x).abs()).min((nr.x - nl.x).abs());
                            min_ch = min_ch.min(((nl.y + nr.y) / 2.0 - (fl.y + fr.y) / 2.0).abs());
                            for q in [nl, nr, fl, fr, c] {
                                assert!(screen.contains(q), "cell corner off screen: {q:?} (v {v}, phase {ph}, row {row})");
                                assert!(!hidden.iter().any(|r| r.contains(q)), "cell point {q:?} under the HUD (v {v}, phase {ph}, row {row}, left {left})");
                                min_cell_top = min_cell_top.min(q.y);
                            }
                            cells_seen += 1;
                        }
                    }
                    // the pack: lanes and fence, on the ground and mid-jump, from the leader back 14 m
                    for back in [0.0f64, 5.0, 10.0, 14.0] {
                        for rx in [-3.3f32, -2.2, 0.0, 2.2, 3.3] {
                            for jump in [0.0f32, 1.5] {
                                let z = (lead - back) as f32;
                                let (feet, head) = (pr.project(Vec3::new(rx, jump, z)).unwrap(), pr.project(Vec3::new(rx, jump + 2.2, z)).unwrap());
                                for q in [feet, head] {
                                    assert!(screen.contains(q) && !hidden.iter().any(|r| r.contains(q)), "runner point {q:?} off screen or under the HUD (v {v}, phase {ph}, back {back})");
                                    min_rtop = min_rtop.min(q.y);
                                    max_rbot = max_rbot.max(q.y);
                                }
                                min_rh = min_rh.min((feet.y - head.y).abs());
                            }
                        }
                    }
                    ph += 0.25;
                }
                v += 0.25;
            }
        }
        println!("architect view 412x915 dp, v 13..16 m/s, all row phases, both handednesses, {cells_seen} cells:");
        println!("  (b) legal cell: min width {min_cw:.1} dp, min height {min_ch:.1} dp, top-most cell point y {min_cell_top:.0} dp (HUD ends y 122)");
        println!("  (a) runner (2.2 m, on the ground or 1.5 m up, leader back 14 m, lanes and fence): min height {min_rh:.1} dp, y range {min_rtop:.0}..{max_rbot:.0} dp");
        assert!(min_cw >= 44.0, "cells at least 44 dp wide: {min_cw}");
        assert!(min_ch >= 30.0, "cells at least 30 dp tall: {min_ch}");
        assert!(min_rh >= 18.0, "runners at least 18 dp tall: {min_rh}");
    }

    #[test]
    fn the_ghost_view_faces_back_and_the_glide_swings_over_the_pack() {
        let run = Camera::runner(0.0, 0.45);
        let gh = ghost_camera(100.0, GHOST_REF_ASPECT, false);
        let look = |c: &Camera| (c.target - c.eye).normalize();
        assert!(look(&run).z > 0.5, "the runner looks down the course");
        assert!(look(&gh).z < -0.9 && look(&gh).y < 0.0, "the architect looks back toward the flock, downwards: {:?}", look(&gh));
        assert!(gh.eye.z > 100.0 + 50.0 && gh.eye.y > 20.0, "camera beyond the band, raised: {:?}", gh.eye);
        // half way round the look direction is steeply down: over the top, not through the ground
        let mid = blend_cameras(&run, &gh, 0.5);
        assert!(look(&mid).y < -0.5, "over the top of the pack at the half way point: {:?}", look(&mid));
        let mut min_y = f32::MAX;
        for k in 0..=20 {
            let c = blend_cameras(&run, &gh, k as f32 / 20.0);
            min_y = min_y.min(c.eye.y);
            assert!(c.eye.is_finite() && c.target.is_finite() && c.fov_y > 0.1);
        }
        assert!(min_y >= run.eye.y - 0.01, "never dips below the runner camera: {min_y}");
        // a left-handed layout mirrors the view; a wider screen keeps the same vertical view
        let l = ghost_camera(100.0, GHOST_REF_ASPECT, true);
        assert!((l.eye.x + gh.eye.x).abs() < 1e-4 && (l.target.x + gh.target.x).abs() < 1e-4);
        assert!((ghost_camera(100.0, 1.7, false).fov_y - gh.fov_y).abs() < 1e-5);
        assert!(ghost_camera(100.0, 0.36, false).fov_y > gh.fov_y, "a narrower screen keeps the same width: taller view");
    }

    #[test]
    fn mirror_is_one_place() {
        assert_eq!(to_render(2.2, 0.0, 5.0, 0.0).x, -2.2);
        assert_eq!(to_sim_x(to_render(1.5, 0.0, 0.0, 0.0).x), 1.5);
    }

    #[test]
    fn glide_takes_point_eight_seconds() {
        let mut r = CameraRig::new();
        r.update(1.0 / 60.0, 0.0, 0.0, 0.0, 0.0, false, 0.45, 0.0);
        for _ in 0..48 {
            r.update(1.0 / 60.0, 0.0, 0.0, 0.0, 0.0, true, 0.45, 0.0);
        }
        assert!(r.glide() > 0.99, "{}", r.glide());
        let mut r = CameraRig::new();
        r.update(1.0 / 60.0, 0.0, 0.0, 0.0, 0.0, false, 0.45, 0.0);
        for _ in 0..24 {
            r.update(1.0 / 60.0, 0.0, 0.0, 0.0, 0.0, true, 0.45, 0.0);
        }
        assert!(r.glide() > 0.3 && r.glide() < 0.7, "halfway at 0.4 s, got {}", r.glide());
    }
}
