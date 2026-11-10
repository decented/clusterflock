//! What the run tells the music: how fast and how desperate things are.

use flock_audio::Mood;

/// Everything the mood is worked out from (plain numbers, so it is easy to test).
pub struct Inputs {
    pub solo: bool,
    pub running: bool,
    /// `World::v_course()` and the tuning's speed range for this mode.
    pub v_course: f32,
    pub v_start: f32,
    pub v_top: f32,
    pub living: usize,
    pub total: usize,
    /// Multiplayer seconds left on the clock.
    pub time_left: Option<f32>,
    pub feathers: u8,
    pub max_feathers: u8,
    pub ghost: bool,
}

/// Intensity follows course speed (and, in a crowd, how many have fallen); tension is
/// the last 30 s, the last two runners, or the last feather. A ghost gets the ghost mood.
pub fn music_mood(i: &Inputs) -> Mood {
    if !i.running {
        return Mood { ghost: if i.ghost { 1.0 } else { 0.0 }, ..Mood::neutral() };
    }
    let span = (i.v_top - i.v_start).max(0.1);
    let speed = ((i.v_course - i.v_start) / span).clamp(0.0, 1.0);
    let crowd = i.total >= 3;
    let fallen = if crowd { 1.0 - (i.living.saturating_sub(1)) as f32 / (i.total - 1) as f32 } else { 0.0 };
    let intensity = speed.max(fallen).clamp(0.0, 1.0);
    let mut tension = 0.0f32;
    if let Some(t) = i.time_left {
        tension = tension.max((1.0 - t / 30.0).clamp(0.0, 1.0));
    }
    if crowd && !i.ghost {
        tension = tension.max(match i.living {
            0 | 1 => 1.0,
            2 => 0.5,
            _ => 0.0,
        });
    }
    if i.solo && i.max_feathers > 1 && i.feathers <= 1 {
        tension = tension.max(0.6);
    }
    Mood { intensity, tension, ghost: if i.ghost { 1.0 } else { 0.0 } }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Inputs {
        Inputs { solo: false, running: true, v_course: 13.0, v_start: 13.0, v_top: 16.0, living: 6, total: 6, time_left: Some(180.0), feathers: 3, max_feathers: 3, ghost: false }
    }

    #[test]
    fn calm_at_the_start_and_busy_at_top_speed() {
        assert_eq!(music_mood(&base()), Mood::neutral());
        let m = music_mood(&Inputs { v_course: 16.0, ..base() });
        assert_eq!(m.intensity, 1.0);
        assert_eq!(m.tension, 0.0);
    }

    #[test]
    fn fewer_runners_and_the_last_seconds_raise_it() {
        let m = music_mood(&Inputs { living: 2, ..base() });
        assert!(m.intensity > 0.7 && m.tension == 0.5);
        let m = music_mood(&Inputs { living: 1, ..base() });
        assert!(m.intensity == 1.0 && m.tension == 1.0);
        let m = music_mood(&Inputs { time_left: Some(15.0), ..base() });
        assert!((m.tension - 0.5).abs() < 1e-6);
    }

    #[test]
    fn solo_follows_speed_and_the_last_feather() {
        let s = Inputs { solo: true, total: 1, living: 1, time_left: None, v_top: 22.0, ..base() };
        assert_eq!(music_mood(&s), Mood::neutral());
        let m = music_mood(&Inputs { v_course: 22.0, feathers: 1, ..s });
        assert_eq!((m.intensity, m.tension), (1.0, 0.6));
    }

    #[test]
    fn ghosts_get_the_ghost_mood_without_the_dread() {
        let m = music_mood(&Inputs { ghost: true, living: 1, ..base() });
        assert_eq!(m.ghost, 1.0);
        assert_eq!(m.tension, 0.0);
        let m = music_mood(&Inputs { running: false, ..base() });
        assert_eq!(m, Mood::neutral());
    }
}
