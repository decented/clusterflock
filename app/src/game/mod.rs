//! Cluster Flock gameplay presentation: input gestures, the presenter (sim
//! world to 3D scene), cameras, HUD, ghost-architect UI, effects and debug
//! switches. Everything here is driven by a `flock_session::RunDriver`, so the
//! same Run screen serves Solo, Flock Practice and (later) online play.

pub mod callouts;
pub mod cameras;
pub mod debug;
pub mod fx;
pub mod ghost_ui;
pub mod gesture;
pub mod hints;
pub mod hud;
pub mod impact;
pub mod local;
pub mod mood;
pub mod presenter;
pub mod stage;
pub mod streak;
