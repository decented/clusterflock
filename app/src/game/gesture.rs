//! Swipe and keyboard input to [`Command`]s.
//!
//! One swipe = one command. A finger fires the moment its travel crosses a
//! threshold (no waiting for the lift), so the bird reacts at once; taps do
//! nothing. Eight directions, but ordinary swipes rarely become diagonals:
//! a diagonal needs the vector to be within [`DIAG_HALF_ANGLE`] of a 45 degree
//! axis AND real travel on both axes. A swipe that is merely heading that way
//! waits a little longer (up to [`AMBIGUOUS_DIST`]) for the second axis to
//! appear, then falls back to the dominant cardinal.

use crate::platform::{KeyCode, Keys, Touches};
use egui::{Pos2, Rect, Vec2};
use flock_core::{Command, InputFrame};
use std::collections::VecDeque;

/// Travel (UI points) at which a clear cardinal swipe fires.
pub const FIRE_DIST: f32 = 28.0;
/// A diagonal needs at least this much travel on each axis (points).
pub const DIAG_AXIS_MIN: f32 = 20.0;
/// Diagonals only within this angle (degrees) of the 45 degree axes.
pub const DIAG_HALF_ANGLE: f32 = 22.0;
/// An ambiguous (near-diagonal but short on one axis) swipe waits until this
/// much travel before it is read as a cardinal.
pub const AMBIGUOUS_DIST: f32 = 46.0;

/// Read a swipe vector (`d` in points, y down). `None` = not decided yet.
pub fn classify(d: Vec2) -> Option<Command> {
    let (ax, ay) = (d.x.abs(), d.y.abs());
    let dist = d.length();
    if dist < FIRE_DIST {
        return None;
    }
    let ang = ay.atan2(ax).to_degrees(); // 0 = horizontal, 90 = vertical
    let near_diag = (ang - 45.0).abs() <= DIAG_HALF_ANGLE;
    let right = d.x > 0.0;
    let up = d.y < 0.0;
    if near_diag {
        if ax >= DIAG_AXIS_MIN && ay >= DIAG_AXIS_MIN {
            return Some(match (up, right) {
                (true, true) => Command::JumpRight,
                (true, false) => Command::JumpLeft,
                (false, true) => Command::DiveRight,
                (false, false) => Command::DiveLeft,
            });
        }
        if dist < AMBIGUOUS_DIST {
            return None;
        }
    }
    Some(if ax > ay {
        if right { Command::Right } else { Command::Left }
    } else if up {
        Command::Jump
    } else {
        Command::Down
    })
}

/// Per-finger swipe tracking. Call [`SwipeTracker::update`] once per frame.
#[derive(Default)]
pub struct SwipeTracker {
    /// Fingers that already fired (or began on a UI control): ignored until lifted.
    spent: Vec<u64>,
}

impl SwipeTracker {
    pub fn new() -> SwipeTracker {
        SwipeTracker::default()
    }

    /// Forget every finger (focus loss).
    pub fn clear(&mut self) {
        self.spent.clear();
    }

    /// Commands fired this frame. `blocked`: rects (UI controls) a swipe may
    /// not start in.
    pub fn update(&mut self, touches: &Touches, blocked: &[Rect], out: &mut Vec<Command>) {
        self.spent.retain(|id| touches.get(*id).is_some());
        for t in touches.active() {
            if self.spent.contains(&t.id) {
                continue;
            }
            if blocked.iter().any(|r| r.contains(t.start)) {
                self.spent.push(t.id);
                continue;
            }
            if let Some(c) = classify(t.pos - t.start) {
                self.spent.push(t.id);
                out.push(c);
            }
        }
    }
}

/// Direction keys, as bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

/// Combine near-simultaneous keyboard presses into one diagonal command.
/// A lone press is held for `window` seconds (the combo window), so a second
/// key arriving inside it turns the pair into a diagonal.
#[derive(Default)]
pub struct KeyCombiner {
    pending: Option<(Dir, f32)>,
}

fn single(d: Dir) -> Command {
    match d {
        Dir::Up => Command::Jump,
        Dir::Down => Command::Down,
        Dir::Left => Command::Left,
        Dir::Right => Command::Right,
    }
}

fn combine(a: Dir, b: Dir) -> Option<Command> {
    use Dir::*;
    Some(match (a, b) {
        (Up, Left) | (Left, Up) => Command::JumpLeft,
        (Up, Right) | (Right, Up) => Command::JumpRight,
        (Down, Left) | (Left, Down) => Command::DiveLeft,
        (Down, Right) | (Right, Down) => Command::DiveRight,
        _ => return None,
    })
}

impl KeyCombiner {
    pub fn new() -> KeyCombiner {
        KeyCombiner::default()
    }

    pub fn clear(&mut self) {
        self.pending = None;
    }

    /// Feed the direction presses of one frame (edges only) and the frame
    /// time. Returns up to two commands in `out`.
    pub fn feed(&mut self, pressed: &[Dir], dt: f32, window: f32, out: &mut Vec<Command>) {
        if let Some((_, age)) = self.pending.as_mut() {
            *age += dt;
        }
        for &d in pressed {
            match self.pending.take() {
                Some((p, _)) => match combine(p, d) {
                    Some(c) => out.push(c),
                    None => {
                        out.push(single(p));
                        self.pending = Some((d, 0.0));
                    }
                },
                None => self.pending = Some((d, 0.0)),
            }
        }
        if let Some((d, age)) = self.pending {
            if age >= window {
                out.push(single(d));
                self.pending = None;
            }
        }
    }
}

/// Keyboard edges this frame: arrows/WASD (Space = jump straight away).
pub fn key_edges(keys: &Keys, dirs: &mut Vec<Dir>, jump_now: &mut bool) {
    for (codes, d) in [
        ([KeyCode::ArrowUp, KeyCode::KeyW], Dir::Up),
        ([KeyCode::ArrowDown, KeyCode::KeyS], Dir::Down),
        ([KeyCode::ArrowLeft, KeyCode::KeyA], Dir::Left),
        ([KeyCode::ArrowRight, KeyCode::KeyD], Dir::Right),
    ] {
        if codes.iter().any(|c| keys.pressed(*c)) {
            dirs.push(d);
        }
    }
    if keys.pressed(KeyCode::Space) {
        *jump_now = true;
    }
}

/// The player's command stream: edges only. Every command bumps `seq`; the
/// same frame is handed to the driver until the next command.
#[derive(Default)]
pub struct InputState {
    seq: u8,
    cmd: Option<Command>,
    queue: VecDeque<Command>,
    pub swipes: SwipeTracker,
    pub keys: KeyCombiner,
    scratch: Vec<Command>,
    dirs: Vec<Dir>,
}

impl InputState {
    pub fn new() -> InputState {
        InputState::default()
    }

    /// Drop everything held (focus loss, pause, ghost role).
    pub fn clear(&mut self) {
        self.swipes.clear();
        self.keys.clear();
        self.queue.clear();
    }

    /// Gather this frame's commands from fingers and keys and return the
    /// commands issued (for local anticipation). At most one reaches the
    /// driver per frame (the rest wait their turn), so none is lost.
    pub fn poll(&mut self, touches: &Touches, keys: &Keys, blocked: &[Rect], dt: f32, combo_window: f32, issued: &mut Vec<Command>) {
        self.scratch.clear();
        self.swipes.update(touches, blocked, &mut self.scratch);
        self.dirs.clear();
        let mut jump_now = false;
        key_edges(keys, &mut self.dirs, &mut jump_now);
        let dirs = std::mem::take(&mut self.dirs);
        self.keys.feed(&dirs, dt, combo_window, &mut self.scratch);
        self.dirs = dirs;
        if jump_now {
            self.scratch.push(Command::Jump);
        }
        for c in self.scratch.drain(..) {
            if self.queue.len() < 3 {
                self.queue.push_back(c);
            }
        }
        if let Some(c) = self.queue.pop_front() {
            self.seq = self.seq.wrapping_add(1);
            self.cmd = Some(c);
            issued.push(c);
        }
    }

    /// Queue a command from another source (a demo bot).
    pub fn push_command(&mut self, c: Command) {
        if self.queue.len() < 3 {
            self.queue.push_back(c);
        }
    }

    /// The frame to hand to `RunDriver::update`.
    pub fn frame(&self) -> InputFrame {
        InputFrame { seq: self.seq, cmd: self.cmd }
    }
}

/// Is `p` inside any of `rects`?
pub fn hits(rects: &[Rect], p: Pos2) -> bool {
    rects.iter().any(|r| r.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    fn at(deg: f32, dist: f32) -> Vec2 {
        // degrees counter-clockwise from +x (right), screen y is down
        let a = deg.to_radians();
        vec2(a.cos() * dist, -a.sin() * dist)
    }

    #[test]
    fn short_travel_is_not_a_swipe() {
        assert_eq!(classify(vec2(10.0, -10.0)), None);
        assert_eq!(classify(vec2(0.0, -FIRE_DIST - 0.1)), Some(Command::Jump));
        assert_eq!(classify(vec2(0.0, -FIRE_DIST + 1.0)), None);
    }

    #[test]
    fn cardinals() {
        assert_eq!(classify(at(0.0, 40.0)), Some(Command::Right));
        assert_eq!(classify(at(180.0, 40.0)), Some(Command::Left));
        assert_eq!(classify(at(90.0, 40.0)), Some(Command::Jump));
        assert_eq!(classify(at(270.0, 40.0)), Some(Command::Down));
    }

    #[test]
    fn ordinary_swipes_with_drift_stay_cardinal() {
        // up with up to 20 degrees of drift is still a jump
        for off in [-20.0, -10.0, 10.0, 20.0] {
            assert_eq!(classify(at(90.0 + off, 60.0)), Some(Command::Jump), "off {off}");
        }
        for off in [-20.0, 20.0] {
            assert_eq!(classify(at(off, 60.0)), Some(Command::Right));
            assert_eq!(classify(at(180.0 + off, 60.0)), Some(Command::Left));
            assert_eq!(classify(at(270.0 + off, 60.0)), Some(Command::Down));
        }
    }

    #[test]
    fn diagonals_on_the_45_axes() {
        assert_eq!(classify(at(45.0, 45.0)), Some(Command::JumpRight));
        assert_eq!(classify(at(135.0, 45.0)), Some(Command::JumpLeft));
        assert_eq!(classify(at(315.0, 45.0)), Some(Command::DiveRight));
        assert_eq!(classify(at(225.0, 45.0)), Some(Command::DiveLeft));
        // 30 degrees off the horizontal is 15 off the axis: diagonal once both axes have travel
        assert_eq!(classify(at(60.0, 50.0)), Some(Command::JumpRight));
    }

    #[test]
    fn near_diagonal_without_second_axis_waits_then_falls_back() {
        // 24 degrees: inside the angular band, but 40 pt gives only 16 pt on the short axis
        let v = at(24.0, 40.0);
        assert_eq!(classify(v), None, "waits for the second axis");
        let v = at(24.0, AMBIGUOUS_DIST + 1.0);
        assert_eq!(classify(v), Some(Command::Right), "gives up and reads the dominant axis");
    }

    #[test]
    fn diagonal_band_is_exactly_22_degrees() {
        // 23 degrees off the 45 axis (= 22 from horizontal) is NOT a diagonal at any length
        let v = at(22.0, 80.0);
        assert_eq!(classify(v), Some(Command::Right));
        let v = at(68.5, 80.0);
        assert_eq!(classify(v), Some(Command::Jump));
        // 21 degrees off the axis (= 24 or 66) is
        assert_eq!(classify(at(66.0, 80.0)), Some(Command::JumpRight));
    }

    #[test]
    fn key_combo_makes_one_diagonal() {
        let mut k = KeyCombiner::new();
        let mut out = Vec::new();
        k.feed(&[Dir::Up], 0.016, 0.08, &mut out);
        assert!(out.is_empty(), "held for the window");
        k.feed(&[Dir::Right], 0.016, 0.08, &mut out);
        assert_eq!(out, vec![Command::JumpRight]);
        // nothing left pending
        out.clear();
        for _ in 0..10 {
            k.feed(&[], 0.016, 0.08, &mut out);
        }
        assert!(out.is_empty());
    }

    #[test]
    fn lone_key_fires_after_the_window() {
        let mut k = KeyCombiner::new();
        let mut out = Vec::new();
        k.feed(&[Dir::Left], 0.016, 0.08, &mut out);
        for _ in 0..10 {
            k.feed(&[], 0.016, 0.08, &mut out);
        }
        assert_eq!(out, vec![Command::Left]);
    }

    #[test]
    fn two_horizontals_are_two_commands() {
        let mut k = KeyCombiner::new();
        let mut out = Vec::new();
        k.feed(&[Dir::Left, Dir::Right], 0.016, 0.08, &mut out);
        for _ in 0..10 {
            k.feed(&[], 0.016, 0.08, &mut out);
        }
        assert_eq!(out, vec![Command::Left, Command::Right]);
    }

    #[test]
    fn tracker_fires_once_per_finger_and_ignores_blocked_starts() {
        use crate::platform::TouchPhase;
        let mut t = Touches::new();
        let mut tr = SwipeTracker::new();
        let mut out = Vec::new();
        t.touch(1, TouchPhase::Started, egui::pos2(100.0, 400.0));
        t.touch(2, TouchPhase::Started, egui::pos2(10.0, 10.0));
        tr.update(&t, &[Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(50.0, 50.0))], &mut out);
        assert!(out.is_empty(), "taps do nothing");
        t.touch(1, TouchPhase::Moved, egui::pos2(100.0, 360.0));
        t.touch(2, TouchPhase::Moved, egui::pos2(10.0, 200.0));
        tr.update(&t, &[Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(50.0, 50.0))], &mut out);
        assert_eq!(out, vec![Command::Jump], "fires on threshold crossing, finger 2 started on a button");
        t.touch(1, TouchPhase::Moved, egui::pos2(300.0, 300.0));
        out.clear();
        tr.update(&t, &[], &mut out);
        assert!(out.is_empty(), "one swipe is one command");
        t.touch(1, TouchPhase::Ended, egui::pos2(300.0, 300.0));
        t.end_frame(0.016);
        tr.update(&t, &[], &mut out);
        t.touch(3, TouchPhase::Started, egui::pos2(100.0, 400.0));
        t.touch(3, TouchPhase::Moved, egui::pos2(60.0, 400.0));
        tr.update(&t, &[], &mut out);
        assert_eq!(out, vec![Command::Left], "a new finger swipes again");
    }

    #[test]
    fn input_state_bumps_seq_per_command() {
        let mut inp = InputState::new();
        let t = Touches::new();
        let k = Keys::default();
        let mut issued = Vec::new();
        assert_eq!(inp.frame().seq, 0);
        inp.queue.push_back(Command::Jump);
        inp.queue.push_back(Command::Left);
        inp.poll(&t, &k, &[], 0.016, 0.08, &mut issued);
        assert_eq!(inp.frame(), InputFrame { seq: 1, cmd: Some(Command::Jump) });
        inp.poll(&t, &k, &[], 0.016, 0.08, &mut issued);
        assert_eq!(inp.frame(), InputFrame { seq: 2, cmd: Some(Command::Left) });
        inp.poll(&t, &k, &[], 0.016, 0.08, &mut issued);
        assert_eq!(inp.frame().seq, 2, "no command, no new edge");
        assert_eq!(issued, vec![Command::Jump, Command::Left]);
    }
}
