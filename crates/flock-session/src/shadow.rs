//! The client's display `World`: built from host data, never stepped.
//!
//! flock-core has no public setters for a world's tick, phase, roles or
//! course speed (only `debug_*` hooks for bodies and the course), so the
//! shadow is made through serde: [`build`] serialises an image with exactly
//! `World`'s field layout and deserialises it as a `World`. The image never
//! carries bots, the placement log or events, so the shadow cannot act on
//! its own. The course is swapped in afterwards with `debug_course_mut`, so
//! the (large) remote course is never copied per frame.
//!
//! The test `image_layout_matches_world` fails the moment `World`'s fields
//! change; then update [`Image`] (or replace this module with a flock-core
//! shadow constructor, which is the requested long-term fix).

use flock_core::{Course, EndReason, Event, MatchConfig, Phase, PlaceResult, Player, Tick, World, MAX_PLAYERS};
use serde::Serialize;

/// `World`'s fields, in declaration order (see flock-core `world.rs`).
#[derive(Serialize)]
struct Image<'a> {
    cfg: &'a MatchConfig,
    tick: Tick,
    phase: Phase,
    end: Option<EndReason>,
    running_since: Option<Tick>,
    players: &'a [Player],
    course: &'a Course,
    anchor: f64,
    v_course: f32,
    events: &'a [Event],
    queued: &'a [Event],
    /// `Vec<BotSlot>`: always empty (a sequence of length 0).
    bots: &'a [()],
    place_log: Vec<Vec<(u32, PlaceResult)>>,
}

/// The display state of a world.
pub struct Parts<'a> {
    pub cfg: &'a MatchConfig,
    pub tick: Tick,
    pub phase: Phase,
    pub end: Option<EndReason>,
    pub running_since: Option<Tick>,
    pub players: &'a [Player],
    pub anchor: f64,
    pub v_course: f32,
}

/// Build a display world from `parts` holding `course`. `placeholder` is any
/// small course (it is serialised in place of the real one). On a layout
/// mismatch the error says so and the course is handed back.
#[allow(clippy::result_large_err)]
pub fn build(parts: &Parts<'_>, course: Course, placeholder: &Course) -> Result<World, (String, Course)> {
    let img = Image {
        cfg: parts.cfg,
        tick: parts.tick,
        phase: parts.phase,
        end: parts.end,
        running_since: parts.running_since,
        players: parts.players,
        course: placeholder,
        anchor: parts.anchor,
        v_course: parts.v_course,
        events: &[],
        queued: &[],
        bots: &[],
        place_log: vec![Vec::new(); MAX_PLAYERS],
    };
    let bytes = match bincode::serialize(&img) {
        Ok(b) => b,
        Err(e) => return Err((format!("shadow image: {e}"), course)),
    };
    match bincode::deserialize::<World>(&bytes) {
        Ok(mut w) => {
            *w.debug_course_mut() = course;
            // Barrels roll as a function of the tick the world last stepped
            // (`World::step` sets the clock to the tick it simulates).
            w.debug_course_mut().set_clock(parts.tick.saturating_sub(1));
            Ok(w)
        }
        Err(e) => Err((format!("shadow world layout mismatch: {e}"), course)),
    }
}

/// The tick `Go` happened on, if the world is past its countdown.
#[cfg(test)]
pub fn running_since(w: &World) -> Option<Tick> {
    (w.phase() != Phase::Countdown).then(|| w.tick().saturating_sub(w.running_ticks()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flock_core::{Command, PlayerId};

    fn parts_of(w: &World) -> Parts<'_> {
        Parts {
            cfg: w.config(),
            tick: w.tick(),
            phase: w.phase(),
            end: w.end_reason(),
            running_since: running_since(w),
            players: w.players(),
            anchor: w.anchor_z(),
            v_course: w.v_course(),
        }
    }

    /// The image serialises to exactly the bytes of a real world (no bots,
    /// no placements, no pending events): any change to `World`'s fields
    /// breaks this before it breaks a client.
    #[test]
    fn image_layout_matches_world() {
        let mut w = World::new(MatchConfig::multi(9, 4, None));
        for k in 0..400 {
            let c = if k % 50 == 0 { vec![(PlayerId(1), Command::Left)] } else { Vec::new() };
            w.step(&c);
        }
        assert_eq!(w.phase(), Phase::Running);
        let real = bincode::serialize(&w).unwrap();
        let p = parts_of(&w);
        let img = Image {
            cfg: p.cfg,
            tick: p.tick,
            phase: p.phase,
            end: p.end,
            running_since: p.running_since,
            players: p.players,
            course: w.course(),
            anchor: p.anchor,
            v_course: p.v_course,
            events: w.events(),
            queued: &[],
            bots: &[],
            place_log: vec![Vec::new(); MAX_PLAYERS],
        };
        assert_eq!(bincode::serialize(&img).unwrap(), real, "World's field layout changed: update shadow::Image");
    }

    #[test]
    fn built_world_reads_like_the_original() {
        let mut w = World::new(MatchConfig::multi(3, 6, Some(flock_core::BotSkill::NORMAL)));
        for _ in 0..600 {
            w.step(&[]);
        }
        let course = Course::new_remote(w.tuning());
        let ph = Course::new_remote(w.tuning());
        let s = build(&parts_of(&w), course, &ph).map_err(|e| e.0).unwrap();
        assert_eq!(s.tick(), w.tick());
        assert_eq!(s.phase(), w.phase());
        assert_eq!(s.running_ticks(), w.running_ticks());
        assert_eq!(s.players(), w.players());
        assert_eq!(s.anchor_z(), w.anchor_z());
        assert!(s.course().is_remote());
        // Stepping a shadow never runs bots (it has none) or generates rows.
        let mut s2 = s.clone();
        s2.step(&[]);
        assert!(s2.course().is_empty());
    }
}
