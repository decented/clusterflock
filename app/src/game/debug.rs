//! Session-only debug switches (Settings, behind 7 taps on the version).
//! Not persisted: a fresh launch is always clean. Read by the Run screen and
//! the local driver every frame through [`with`].

use std::cell::RefCell;

#[derive(Clone, Debug, Default)]
pub struct DebugState {
    /// Use `seed` for every local run (reproducible course).
    pub fixed_seed: bool,
    pub seed: u64,
    /// Run the sim at a quarter speed.
    pub slow_mo: bool,
    /// Draw collision volumes of runners and obstacles.
    pub boxes: bool,
    /// Draw protection status above every runner.
    pub protection: bool,
    /// Draw the pack anchor and its soft-wall band.
    pub pack: bool,
    /// Draw the placement band with row numbers and reject reasons.
    pub placement: bool,
    /// Actions for the local driver to apply on its next update.
    pub pending: Vec<DebugAction>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DebugAction {
    /// Eliminate the local player now (turns them into a ghost in multi).
    ForceEliminateMe,
}

thread_local! {
    static DEBUG: RefCell<DebugState> = RefCell::new(DebugState { seed: 1234, ..DebugState::default() });
}

/// Read or change the debug state.
pub fn with<R>(f: impl FnOnce(&mut DebugState) -> R) -> R {
    DEBUG.with(|d| f(&mut d.borrow_mut()))
}

/// Any overlay on (cheap check for the hot path).
pub fn any_overlay() -> bool {
    with(|d| d.boxes || d.protection || d.pack || d.placement)
}

/// The seed for a new local run: the fixed one when set, else `fresh`.
pub fn seed_or(fresh: u64) -> u64 {
    with(|d| if d.fixed_seed { d.seed } else { fresh })
}

/// A fresh random-ish seed (clock and process id; not security).
pub fn fresh_seed() -> u64 {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7);
    (n ^ (n >> 29) ^ ((std::process::id() as u64) << 17)).max(1)
}
