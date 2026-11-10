//! Clear streaks and close dodges, derived each frame from the display world.
//! Pure: the caller hands in the local runner's position and the damaging
//! obstacles near it; nothing here touches the simulation.

use std::collections::HashSet;

/// Seconds of travel ahead of an obstacle that count as "close".
pub const CLOSE_SECS: f32 = 0.30;

/// A damaging obstacle: its lane footprint (sim x range) and z extent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Obs {
    pub id: u32,
    pub x0: f32,
    pub x1: f32,
    /// Near edge (first z the runner meets).
    pub z0: f64,
    /// Far edge: the runner has passed the obstacle once its z reaches this.
    pub z1: f64,
}

/// A crash event against the local runner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crash {
    pub obj: u32,
    pub feather_lost: bool,
}

/// One frame of input.
pub struct Frame<'a> {
    pub x: f32,
    pub z: f64,
    /// Forward speed, m/s.
    pub speed: f32,
    pub obstacles: &'a [Obs],
    pub crashes: &'a [Crash],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Out {
    /// An obstacle was passed cleanly; the streak is now `streak`.
    Clear { streak: u32 },
    /// A feather-losing crash ended a streak of `lost`.
    Reset { lost: u32 },
    /// Left an obstacle's lane at the last moment, without a crash.
    Close,
}

#[derive(Default)]
pub struct ClearTracker {
    /// Half a body: how far outside the lane the runner must be to be clear of it.
    margin: f32,
    streak: u32,
    seen: HashSet<u32>,
    passed: HashSet<u32>,
    hit: HashSet<u32>,
    armed: HashSet<u32>,
    closed: HashSet<u32>,
}

impl ClearTracker {
    pub fn new(margin: f32) -> ClearTracker {
        ClearTracker { margin, ..ClearTracker::default() }
    }

    pub fn streak(&self) -> u32 {
        self.streak
    }

    /// Advance one frame; appends what happened to `out`.
    pub fn update(&mut self, f: &Frame, out: &mut Vec<Out>) {
        for c in f.crashes {
            self.hit.insert(c.obj);
            self.armed.remove(&c.obj);
            if c.feather_lost {
                if self.streak > 0 {
                    out.push(Out::Reset { lost: self.streak });
                }
                self.streak = 0;
            }
        }
        let reach = CLOSE_SECS * f.speed.max(0.0);
        for o in f.obstacles {
            let inside = f.x >= o.x0 && f.x <= o.x1;
            if f.z >= o.z1 {
                // passed (an obstacle first seen already behind us counts for nothing)
                if self.seen.contains(&o.id) && self.passed.insert(o.id) {
                    self.armed.remove(&o.id);
                    if inside && !self.hit.contains(&o.id) {
                        self.streak += 1;
                        out.push(Out::Clear { streak: self.streak });
                    }
                }
                continue;
            }
            self.seen.insert(o.id);
            if self.hit.contains(&o.id) || self.closed.contains(&o.id) {
                continue;
            }
            let gap = o.z0 - f.z;
            if gap < 0.0 {
                continue;
            }
            if inside {
                if gap <= reach as f64 {
                    self.armed.insert(o.id);
                }
            } else if self.armed.contains(&o.id) && (f.x < o.x0 - self.margin || f.x > o.x1 + self.margin) {
                self.armed.remove(&o.id);
                self.closed.insert(o.id);
                out.push(Out::Close);
            }
        }
        // forget what has left the window (it never comes back)
        let live: HashSet<u32> = f.obstacles.iter().map(|o| o.id).collect();
        for s in [&mut self.seen, &mut self.passed, &mut self.hit, &mut self.armed, &mut self.closed] {
            s.retain(|id| live.contains(id));
        }
    }
}

/// The word for a streak count, if it is a milestone: 3, 5, 10, 15, 20, then every 10.
pub fn streak_word(n: u32) -> Option<String> {
    Some(match n {
        3 => "NICE!".into(),
        5 => "SMOOTH!".into(),
        10 => "ON FIRE!".into(),
        15 => "FLOCKING GOOD!".into(),
        20 => "UNSTOPPABLE!".into(),
        n if n > 20 && n % 10 == 0 => format!("x{n}"),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(id: u32, l: i32, z: f64) -> Obs {
        let c = l as f32 * 2.2;
        Obs { id, x0: c - 1.1, x1: c + 1.1, z0: z - 0.5, z1: z + 0.5 }
    }

    fn run(t: &mut ClearTracker, x: f32, z: f64, obs: &[Obs], crashes: &[Crash]) -> Vec<Out> {
        let mut out = Vec::new();
        t.update(&Frame { x, z, speed: 14.0, obstacles: obs, crashes }, &mut out);
        out
    }

    /// Drive past `obs` in a straight line at sim x.
    fn drive(t: &mut ClearTracker, x: f32, obs: &[Obs], z_from: f64, z_to: f64) -> Vec<Out> {
        let mut all = Vec::new();
        let mut z = z_from;
        while z <= z_to {
            all.extend(run(t, x, z, obs, &[]));
            z += 0.25;
        }
        all
    }

    #[test]
    fn a_clear_counts_once() {
        let mut t = ClearTracker::new(0.42);
        let o = [lane(1, 0, 30.0)];
        let out = drive(&mut t, 0.0, &o, 0.0, 40.0);
        assert_eq!(out, vec![Out::Clear { streak: 1 }]);
        assert_eq!(t.streak(), 1);
    }

    #[test]
    fn each_obstacle_adds_one() {
        let mut t = ClearTracker::new(0.42);
        let o = [lane(1, 0, 30.0), lane(2, 0, 50.0), lane(3, 0, 70.0)];
        let out = drive(&mut t, 0.0, &o, 0.0, 80.0);
        let streaks: Vec<u32> = out.iter().filter_map(|o| if let Out::Clear { streak } = o { Some(*streak) } else { None }).collect();
        assert_eq!(streaks, vec![1, 2, 3]);
    }

    #[test]
    fn other_lanes_do_not_count() {
        let mut t = ClearTracker::new(0.42);
        let o = [lane(1, 1, 30.0), lane(2, -1, 50.0)];
        let out = drive(&mut t, 0.0, &o, 0.0, 60.0);
        assert!(out.is_empty());
        assert_eq!(t.streak(), 0);
    }

    #[test]
    fn a_feather_losing_crash_resets_and_does_not_count() {
        let mut t = ClearTracker::new(0.42);
        let o = [lane(1, 0, 30.0), lane(2, 0, 50.0)];
        let mut out = drive(&mut t, 0.0, &o, 0.0, 40.0);
        assert_eq!(t.streak(), 1);
        // run into the second one
        out.clear();
        out.extend(drive(&mut t, 0.0, &o, 40.0, 49.0));
        out.extend(run(&mut t, 0.0, 49.5, &o, &[Crash { obj: 2, feather_lost: true }]));
        out.extend(drive(&mut t, 0.0, &o, 50.0, 60.0));
        assert_eq!(out, vec![Out::Reset { lost: 1 }]);
        assert_eq!(t.streak(), 0);
    }

    #[test]
    fn a_protected_crash_neither_counts_nor_resets() {
        let mut t = ClearTracker::new(0.42);
        let o = [lane(1, 0, 30.0), lane(2, 0, 50.0)];
        drive(&mut t, 0.0, &o, 0.0, 40.0);
        let mut out = drive(&mut t, 0.0, &o, 40.0, 49.0);
        out.extend(run(&mut t, 0.0, 49.5, &o, &[Crash { obj: 2, feather_lost: false }]));
        out.extend(drive(&mut t, 0.0, &o, 50.0, 60.0));
        assert!(out.is_empty());
        assert_eq!(t.streak(), 1);
    }

    #[test]
    fn a_rolling_barrel_counts_once() {
        let mut t = ClearTracker::new(0.42);
        let mut out = Vec::new();
        // runner moves 0.25 m a frame, barrel rolls toward it 0.07 m a frame
        let (mut z, mut bz) = (0.0f64, 40.0f64);
        while z < 60.0 {
            let b = [Obs { id: 9, x0: -1.1, x1: 1.1, z0: bz - 0.5, z1: bz + 0.5 }];
            out.extend(run(&mut t, 0.0, z, &b, &[]));
            z += 0.25;
            bz -= 0.07;
        }
        assert_eq!(out, vec![Out::Clear { streak: 1 }]);
    }

    #[test]
    fn an_obstacle_first_seen_behind_does_not_count() {
        let mut t = ClearTracker::new(0.42);
        let o = [lane(1, 0, 10.0)];
        assert!(run(&mut t, 0.0, 20.0, &o, &[]).is_empty());
        assert_eq!(t.streak(), 0);
    }

    #[test]
    fn leaving_the_lane_at_the_last_moment_is_close() {
        let mut t = ClearTracker::new(0.42);
        let o = [lane(1, 0, 30.0)];
        let mut out = drive(&mut t, 0.0, &o, 0.0, 25.0); // z0 = 29.5; 0.3 s at 14 m/s = 4.2 m
        assert!(out.is_empty());
        // slides right, out of the lane (and a body clear of it) a little later
        let mut x = 0.0;
        let mut z = 25.5;
        while z < 40.0 {
            x += 0.3;
            out.extend(run(&mut t, x, z, &o, &[]));
            z += 0.25;
        }
        assert_eq!(out, vec![Out::Close], "close once, and no clear for the lane you left");
        assert_eq!(t.streak(), 0);
    }

    #[test]
    fn an_early_dodge_is_not_close() {
        let mut t = ClearTracker::new(0.42);
        let o = [lane(1, 0, 40.0)];
        let mut out = drive(&mut t, 0.0, &o, 0.0, 10.0);
        out.extend(drive(&mut t, 2.2, &o, 10.5, 50.0));
        assert!(out.is_empty());
    }

    #[test]
    fn a_close_one_that_crashes_is_not_close() {
        let mut t = ClearTracker::new(0.42);
        let o = [lane(1, 0, 30.0)];
        let mut out = drive(&mut t, 0.0, &o, 0.0, 28.0);
        out.extend(run(&mut t, 0.0, 28.5, &o, &[Crash { obj: 1, feather_lost: true }]));
        out.extend(drive(&mut t, 1.5, &o, 28.75, 40.0));
        assert_eq!(out, vec![]);
    }

    #[test]
    fn close_fires_at_most_once_per_obstacle() {
        let mut t = ClearTracker::new(0.42);
        let o = [lane(1, 0, 30.0)];
        drive(&mut t, 0.0, &o, 0.0, 26.0);
        let a = run(&mut t, 2.0, 26.25, &o, &[]);
        let b = run(&mut t, 0.0, 26.5, &o, &[]);
        let c = run(&mut t, 2.0, 26.75, &o, &[]);
        assert_eq!(a, vec![Out::Close]);
        assert!(b.is_empty() && c.is_empty());
    }

    #[test]
    fn the_words() {
        assert_eq!(streak_word(3).as_deref(), Some("NICE!"));
        assert_eq!(streak_word(4), None);
        assert_eq!(streak_word(20).as_deref(), Some("UNSTOPPABLE!"));
        assert_eq!(streak_word(25), None);
        assert_eq!(streak_word(30).as_deref(), Some("x30"));
        assert_eq!(streak_word(40).as_deref(), Some("x40"));
    }
}
