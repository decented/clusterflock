//! The one shared `flock_render::Renderer` (menus and runs both draw 3D).
//! It lives in a thread-local keyed by GPU device and surface format, is
//! created lazily inside `paint_scene` (the only place a device exists) and
//! bakes the farm once, so "Run again" and Home to Run never rebuild it.

use crate::platform::data_dir::Quality;
use crate::render::ScenePass;
use flock_render::{FrameScene, Quality as RQuality, Renderer, WorldGeometry};
use std::cell::RefCell;

struct Stage {
    renderer: Renderer,
    device_id: u64,
    format: wgpu::TextureFormat,
    quality: RQuality,
    size: (u32, u32),
}

thread_local! {
    static STAGE: RefCell<Option<Stage>> = const { RefCell::new(None) };
}

/// Profile quality to the renderer's tier.
pub fn render_quality(q: Quality) -> RQuality {
    match q {
        Quality::Low => RQuality::Battery,
        Quality::Balanced => RQuality::Balanced,
        Quality::High => RQuality::Max,
    }
}

/// What to draw this frame.
pub struct Draw<'a> {
    pub scene: &'a FrameScene,
    /// Seconds of particle time to advance (0 freezes them).
    pub dt: f32,
    pub quality: Quality,
    /// The floating origin moved by this many metres (`last - new`): shifts
    /// particles in flight. 0 when it did not move.
    pub shift_z: f32,
    /// Earlier frames (shot mode only): stepped first so particles exist.
    pub replay: &'a [FrameScene],
}

/// Draw `d.scene` into the pass (clears and covers the target).
pub fn paint(pass: &mut ScenePass, d: Draw) {
    STAGE.with(|cell| {
        let mut slot = cell.borrow_mut();
        let rebuild = slot.as_ref().is_none_or(|s| s.device_id != pass.device_id || s.format != pass.format);
        if rebuild {
            let q = render_quality(d.quality);
            let mut renderer = Renderer::new(pass.device, pass.queue, pass.format, q);
            renderer.load_world(pass.device, pass.queue, &WorldGeometry::farm());
            *slot = Some(Stage { renderer, device_id: pass.device_id, format: pass.format, quality: q, size: (0, 0) });
        }
        let s = slot.as_mut().expect("stage exists");
        let q = render_quality(d.quality);
        if s.quality != q {
            s.renderer.set_quality(q);
            s.quality = q;
        }
        if s.size != pass.size_px {
            s.renderer.resize(pass.size_px.0, pass.size_px.1);
            s.size = pass.size_px;
        }
        if d.shift_z != 0.0 {
            s.renderer.shift_world_z(d.shift_z);
        }
        for sc in d.replay {
            s.renderer.step_particles(sc, 1.0 / 60.0);
        }
        s.renderer.render(pass.device, pass.queue, pass.encoder, pass.view, d.scene, d.dt);
    });
}
