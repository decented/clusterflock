//! egui theme, widgets, painted art and the screens.

pub mod art;
pub mod feedback;
pub mod icons;
pub mod screens;
pub mod theme;
pub mod widgets;

// The gameplay presentation layer lives in `app/src/game`; declared here so the
// crate root (lib.rs, owned elsewhere) does not need a line for it.
#[path = "../game/mod.rs"]
pub mod game;
