//! [`LocalRun`]: the offline path (Solo and Flock Practice) with the same
//! [`RunDriver`] API as an online [`crate::ClientSession`], so the app has one
//! run screen. No network code at all.

use flock_core::{
    BotSkill, CmdLatch, Command, InputFrame, MatchConfig, Phase, PlaceRequest, PlaceResult, PlayerId, Tool, World,
    MAX_PLAYERS, TICK_HZ,
};

use crate::view::{results_from_world, FlockView, ResultsView, RunDriver};

/// Most world ticks stepped in one `update` (a stalled frame must not spiral).
pub const MAX_CATCH_UP: u32 = 12;
/// Longest frame step counted, ms.
pub const MAX_FRAME_MS: u64 = 100;

/// `cfg` with player 0 (the local player) wearing `colour` and `hat`.
fn styled(mut cfg: MatchConfig, colour: u8, hat: u8) -> MatchConfig {
    if let Some(p) = cfg.players.first_mut() {
        p.colour = colour % crate::proto::COLOURS;
        p.hat = hat;
    }
    cfg
}

/// A match with no network: wraps `World` and steps it at 60 Hz from the
/// frame times.
pub struct LocalRun {
    view: FlockView,
    latch: CmdLatch,
    /// A command latched on a frame that stepped no tick waits here.
    pending_cmd: Option<Command>,
    pending_places: Vec<(u32, PlaceResult)>,
    next_req: u32,
    sim_ms: u64,
    last_now: Option<u64>,
    paused: bool,
    results: Option<ResultsView>,
}

impl LocalRun {
    /// Solo: one runner, no bots, the run ends on the final feather.
    pub fn solo(seed: u64) -> LocalRun {
        LocalRun::new(MatchConfig::solo(seed), PlayerId(0))
    }

    /// The daily run: solo rules on a published seed (same course on every
    /// device; see `flock_core::course_fingerprint`).
    pub fn daily(seed: u64) -> LocalRun {
        LocalRun::new(MatchConfig::daily(seed), PlayerId(0))
    }

    /// Flock Practice: you (player 0) plus `bots` (clamped 1..=5) runner/ghost
    /// bots under the multiplayer rules (ghost mode included).
    pub fn practice(seed: u64, bots: u8, skill: BotSkill) -> LocalRun {
        let bots = (bots as usize).clamp(1, MAX_PLAYERS - 1);
        let mut cfg = MatchConfig::multi(seed, bots + 1, Some(skill));
        for (i, p) in cfg.players.iter_mut().enumerate() {
            if i == 0 {
                p.bot = None;
                p.name = "You".into();
            } else {
                p.name = format!("Bot {i}");
            }
        }
        LocalRun::new(cfg, PlayerId(0))
    }

    /// [`LocalRun::solo`] with the local player wearing `colour` and `hat`
    /// (presentation only; 255 = the colour's default hat).
    pub fn solo_styled(seed: u64, colour: u8, hat: u8) -> LocalRun {
        LocalRun::new(styled(MatchConfig::solo(seed), colour, hat), PlayerId(0))
    }

    /// [`LocalRun::daily`] with the local player's look.
    pub fn daily_styled(seed: u64, colour: u8, hat: u8) -> LocalRun {
        LocalRun::new(styled(MatchConfig::daily(seed), colour, hat), PlayerId(0))
    }

    /// [`LocalRun::practice`] with the local player's look. The bots take the
    /// other colours in order, so no bot shares the player's colour.
    pub fn practice_styled(seed: u64, bots: u8, skill: BotSkill, colour: u8, hat: u8) -> LocalRun {
        let mut cfg = styled(LocalRun::practice(seed, bots, skill).world().config().clone(), colour, hat);
        let mut others = (0..crate::proto::COLOURS).filter(|c| *c != colour % crate::proto::COLOURS);
        for p in cfg.players.iter_mut().skip(1) {
            if let Some(c) = others.next() {
                p.colour = c;
            }
        }
        LocalRun::new(cfg, PlayerId(0))
    }

    /// Any config; `local` is the player the controls drive. The countdown
    /// starts on the first `update`.
    pub fn new(cfg: MatchConfig, local: PlayerId) -> LocalRun {
        let world = World::new(cfg);
        let local = if world.player(local).is_some() { local } else { world.players()[0].id() };
        LocalRun {
            view: FlockView::new(world, local, false),
            latch: CmdLatch::new(),
            pending_cmd: None,
            pending_places: Vec::new(),
            next_req: 0,
            sim_ms: 0,
            last_now: None,
            paused: false,
            results: None,
        }
    }

    /// The real simulation (debug tools may poke it between updates).
    pub fn world(&self) -> &World {
        &self.view.world
    }

    /// Debug tools (force-eliminate, energy): mutable sim.
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.view.world
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn is_over(&self) -> bool {
        self.results.is_some()
    }
}

impl RunDriver for LocalRun {
    fn update(&mut self, now_ms: u64, input: InputFrame) {
        let dt = self.last_now.map_or(0, |l| now_ms.saturating_sub(l)).min(MAX_FRAME_MS);
        self.last_now = Some(now_ms);
        let local = self.view.local;
        // Feed the latch every frame so a command given while paused is
        // consumed (and dropped) rather than fired on resume.
        let cmd = self.latch.feed(local, input);
        if !self.paused {
            self.sim_ms += dt;
            if cmd.is_some() {
                self.pending_cmd = cmd;
            }
        }
        let mut events = Vec::new();
        let mut steps = 0;
        let w = &mut self.view.world;
        while steps < MAX_CATCH_UP && !self.paused && w.phase() != Phase::Over && w.tick() * 1000 / TICK_HZ as u64 <= self.sim_ms {
            let cmds: Vec<(PlayerId, Command)> = self.pending_cmd.take().map(|c| (local, c)).into_iter().collect();
            w.step(&cmds);
            events.extend_from_slice(w.events());
            steps += 1;
        }
        // Keep the sim clock from running ahead of a capped catch-up.
        let max_ms = (w.tick() + 1) * 1000 / TICK_HZ as u64;
        if self.sim_ms > max_ms {
            self.sim_ms = max_ms;
        }
        self.view.events = events;
        self.view.place_results = std::mem::take(&mut self.pending_places);
        self.view.paused = self.paused;
        self.view.refresh();
        if self.results.is_none() && self.view.world.phase() == Phase::Over {
            self.results = Some(results_from_world(&self.view.world));
        }
        self.view.over = self.results.is_some();
    }

    fn place(&mut self, tool: Tool, lane: u8, row: u32) -> u32 {
        self.next_req = self.next_req.wrapping_add(1);
        let req = self.next_req;
        let r = self.view.world.place(PlaceRequest { player: self.view.local, req, tool, lane, row });
        self.pending_places.push((req, r));
        req
    }

    fn view(&self) -> Option<&FlockView> {
        Some(&self.view)
    }

    fn results(&self) -> Option<&ResultsView> {
        self.results.as_ref()
    }

    fn set_paused(&mut self, p: bool) {
        self.paused = p;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flock_core::{Event, Mode, Reject};

    fn run_until(r: &mut LocalRun, ms: u64, mut f: impl FnMut(u64, &FlockView)) -> u64 {
        let mut now = 0;
        while now <= ms {
            r.update(now, InputFrame::default());
            f(now, r.view().unwrap());
            if r.results().is_some() {
                break;
            }
            now += 16;
        }
        now
    }

    #[test]
    fn styled_runs_give_the_local_player_a_colour_and_hat() {
        let r = LocalRun::solo_styled(7, 3, 8);
        let p = &r.world().config().players[0];
        assert_eq!((p.colour, p.hat), (3, 8));
        let r = LocalRun::daily_styled(7, 5, 255);
        assert_eq!(r.world().config().players[0].colour, 5);
        let r = LocalRun::practice_styled(7, 5, BotSkill::NORMAL, 2, 0);
        let cols: Vec<u8> = r.world().config().players.iter().map(|p| p.colour).collect();
        assert_eq!(cols[0], 2);
        let mut sorted = cols.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), 6, "all six colours once: {cols:?}");
    }

    #[test]
    fn solo_steps_at_60hz_and_counts_down() {
        let mut r = LocalRun::solo(7);
        assert_eq!(r.view().unwrap().mode, Mode::Solo);
        r.update(0, InputFrame::default());
        r.update(1000, InputFrame::default());
        // 100 ms frame cap: one long frame counts as 100 ms.
        assert!(r.world().tick() <= 7, "tick {}", r.world().tick());
        let mut go = false;
        run_until(&mut r, 6000, |_, v| go |= v.events.iter().any(|e| matches!(e, Event::Go)));
        assert!(go, "GO event seen exactly through events");
        assert_eq!(r.view().unwrap().phase, Phase::Running);
    }

    #[test]
    fn pause_freezes_the_sim() {
        let mut r = LocalRun::solo(1);
        run_until(&mut r, 500, |_, _| {});
        let t = r.world().tick();
        r.set_paused(true);
        for i in 0..50 {
            r.update(600 + i * 16, InputFrame::default());
        }
        assert_eq!(r.world().tick(), t);
        assert!(r.view().unwrap().paused);
        r.set_paused(false);
        r.update(600 + 50 * 16, InputFrame::default());
        r.update(600 + 51 * 16, InputFrame::default());
        assert!(r.world().tick() > t && r.world().tick() <= t + 3);
    }

    #[test]
    fn command_on_a_zero_step_frame_is_not_lost() {
        let mut r = LocalRun::solo(3);
        // Get past the countdown.
        let mut now = 0;
        while r.world().phase() != Phase::Running {
            now += 16;
            r.update(now, InputFrame::default());
        }
        // A frame with dt 0 steps nothing but latches the command.
        let f = InputFrame { seq: 1, cmd: Some(Command::Left) };
        r.update(now, f);
        let mut changed = false;
        for _ in 0..40 {
            now += 16;
            r.update(now, f);
            changed |= r.view().unwrap().events.iter().any(|e| {
                matches!(e, Event::LaneChange { .. } | Event::WindUp { .. } | Event::FenceBump { .. })
            });
        }
        assert!(changed, "the left command fired once");
    }

    #[test]
    fn practice_has_bots_and_place_answers_once() {
        let mut r = LocalRun::practice(5, 5, BotSkill::NORMAL);
        let v = r.view().unwrap();
        assert_eq!(v.names.len(), 6);
        assert!(!v.names[0].3 && v.names[1..].iter().all(|n| n.3));
        // A runner cannot place.
        let req = r.place(Tool::Hay, 1, 40);
        r.update(0, InputFrame::default());
        assert_eq!(r.view().unwrap().place_results, vec![(req, Err(Reject::NotGhost))]);
        r.update(16, InputFrame::default());
        assert!(r.view().unwrap().place_results.is_empty(), "each result exactly once");
    }

    #[test]
    fn practice_ends_with_results() {
        let mut r = LocalRun::practice(11, 3, BotSkill::NORMAL);
        r.world_mut().force_eliminate(PlayerId(0));
        run_until(&mut r, 400_000, |_, _| {});
        let res = r.results().expect("results");
        assert_eq!(res.rows.len(), 4);
        assert!(res.end_reason.is_some());
        assert!(r.view().unwrap().over);
    }
}
