//! Offline drivers: Solo, Flock Practice and the debug test runs, all a
//! [`LocalRun`] wrapped so the debug switches (force-eliminate) reach the
//! simulation without the Run screen knowing which driver it has.

use super::debug::{self, DebugAction};
use flock_core::{BotSkill, InputFrame, PlayerId, Tool, World};
use flock_session::{FlockView, LocalRun, ResultsView, RunDriver};

pub struct LocalDriver {
    run: LocalRun,
}

impl LocalDriver {
    pub fn new(run: LocalRun) -> LocalDriver {
        LocalDriver { run }
    }
}

impl RunDriver for LocalDriver {
    fn update(&mut self, now_ms: u64, input: InputFrame) {
        let actions = debug::with(|d| std::mem::take(&mut d.pending));
        for a in actions {
            match a {
                DebugAction::ForceEliminateMe => {
                    let me = self.run.view().map_or(PlayerId(0), |v| v.local);
                    self.run.world_mut().force_eliminate(me);
                }
            }
        }
        self.run.update(now_ms, input);
    }
    fn place(&mut self, tool: Tool, lane: u8, row: u32) -> u32 {
        self.run.place(tool, lane, row)
    }
    fn view(&self) -> Option<&FlockView> {
        self.run.view()
    }
    fn results(&self) -> Option<&ResultsView> {
        self.run.results()
    }
    fn set_paused(&mut self, p: bool) {
        self.run.set_paused(p)
    }
}

/// A solo run (seed fixed by the debug switch, else fresh).
pub fn solo() -> Box<dyn RunDriver> {
    Box::new(LocalDriver::new(LocalRun::solo(debug::seed_or(debug::fresh_seed()))))
}

/// Flock Practice: you plus `bots` bots (1..=5) of `skill`.
pub fn practice(bots: u8, skill: BotSkill) -> Box<dyn RunDriver> {
    Box::new(LocalDriver::new(LocalRun::practice(debug::seed_or(debug::fresh_seed()), bots, skill)))
}

/// A run on a ready-made world (`LocalRun` has no constructor for one, so this
/// builds it from the world's own config and swaps the world in).
fn from_world(world: World) -> Box<dyn RunDriver> {
    let mut run = LocalRun::new(world.config().clone(), PlayerId(0));
    *run.world_mut() = world;
    Box::new(LocalDriver::new(run))
}

/// Debug: six runners queued in one lane on an empty course.
pub fn debug_six_in_one_lane() -> Box<dyn RunDriver> {
    from_world(World::debug_six_in_one_lane(debug::seed_or(debug::fresh_seed())))
}

/// Debug: you against five ghost bots.
pub fn debug_one_runner_five_ghosts() -> Box<dyn RunDriver> {
    from_world(World::debug_one_runner_five_ghosts(debug::seed_or(debug::fresh_seed())))
}

/// Bot skill from the Practice setup (0 easy, 1 normal, 2 hard).
pub fn skill(level: usize) -> BotSkill {
    [BotSkill::EASY, BotSkill::NORMAL, BotSkill::HARD][level.min(2)]
}
