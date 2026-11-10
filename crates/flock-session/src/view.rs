//! The render-ready run view and the one trait the run screen drives: solo,
//! Flock Practice ([`crate::LocalRun`]) and online ([`crate::ClientSession`])
//! look the same to it.

use flock_core::{
    EndReason, Event, InputFrame, Mode, Phase, PlaceResult, PlayerId, PlayerSpec, SoloResult, Tool, World, DT,
};
use serde::{Deserialize, Serialize};

/// One frame of a run, ready to draw. Rebuilt by [`RunDriver::update`];
/// borrow it with [`RunDriver::view`].
#[derive(Clone, Debug)]
pub struct FlockView {
    /// The world to draw. Offline it is the real simulation. Online it is a
    /// display shadow: a remote-course `World` (rows streamed by the host,
    /// coins and placements applied from host events) whose runner bodies are
    /// your predicted runner (plus `correction`) and the others interpolated
    /// from snapshots; roles, feathers, coins and energy come from the host.
    /// Never step it.
    pub world: World,
    /// Your player.
    pub local: PlayerId,
    /// Events since the previous `update`, each exactly once (sounds, FX,
    /// HUD flashes). Online, motion events of your own runner (wind-up,
    /// jump, land, lane change) come from the prediction; everything else is
    /// the host's.
    pub events: Vec<Event>,
    /// Answers to your [`RunDriver::place`] calls since the previous
    /// `update`: `(req, result)`.
    pub place_results: Vec<(u32, PlaceResult)>,
    /// `(id, name, colour 0..=5, bot)` for every player in the match.
    pub names: Vec<(PlayerId, String, u8, bool)>,
    /// Match phase (also `world.phase()`; online the host's).
    pub phase: Phase,
    /// Seconds to GO while the countdown runs, else 0.
    pub countdown: f32,
    /// Round number (rematches count up from 0).
    pub round: u32,
    /// Solo or Multi (Flock Practice is Multi).
    pub mode: Mode,
    /// Round trip to the host (online only).
    pub ping_ms: Option<u32>,
    /// Visual correction currently added to your runner `[x, y, z]` metres
    /// (online; zero offline). Already applied in `world`.
    pub correction: [f32; 3],
    /// Online: the run is being driven by this device's network session.
    pub online: bool,
    /// Offline: the sim is paused.
    pub paused: bool,
    /// The match is over (results exist).
    pub over: bool,
}

impl FlockView {
    pub fn new(world: World, local: PlayerId, online: bool) -> FlockView {
        let mut v = FlockView {
            names: names_of(&world),
            phase: world.phase(),
            countdown: 0.0,
            round: world.config().round,
            mode: world.mode(),
            world,
            local,
            events: Vec::new(),
            place_results: Vec::new(),
            ping_ms: None,
            correction: [0.0; 3],
            online,
            paused: false,
            over: false,
        };
        v.refresh();
        v
    }

    /// Name, colour and bot flag of a player.
    pub fn name_of(&self, id: PlayerId) -> Option<(&str, u8, bool)> {
        self.names.iter().find(|n| n.0 == id).map(|n| (n.1.as_str(), n.2, n.3))
    }

    /// Cosmetic hat of a player (`PlayerSpec::DEFAULT_HAT` = 255: the
    /// default hat for its colour; also for unknown ids).
    pub fn hat_of(&self, id: PlayerId) -> u8 {
        self.world.config().players.iter().find(|p| p.id == id).map_or(PlayerSpec::DEFAULT_HAT, |p| p.hat)
    }

    /// Recompute the derived fields (`phase`, `countdown`) from `world`.
    pub(crate) fn refresh(&mut self) {
        self.phase = self.world.phase();
        self.countdown = countdown_left(&self.world);
    }
}

/// Seconds until GO while the world is in `Countdown`, else 0.
pub fn countdown_left(w: &World) -> f32 {
    if w.phase() != Phase::Countdown {
        return 0.0;
    }
    (w.tuning().countdown - w.tick() as f32 * DT).max(0.0)
}

/// `(id, name, colour, bot)` of every player in the world's config.
pub fn names_of(w: &World) -> Vec<(PlayerId, String, u8, bool)> {
    w.config().players.iter().map(|p| (p.id, p.name.clone(), p.colour, p.bot.is_some())).collect()
}

/// One results-table row (SPEC §8 standings + lobby names).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResultRow {
    pub player: PlayerId,
    pub name: String,
    pub colour: u8,
    /// Cosmetic hat (255 = default for the colour).
    pub hat: u8,
    pub bot: bool,
    /// 1-based, shared on ties.
    pub place: u8,
    /// Still a runner at the end.
    pub alive: bool,
    pub survived_ticks: u64,
    pub feathers: u8,
    pub coins: u32,
    /// Accepted ghost placements (architect score, unranked).
    pub placements: u32,
    /// Left the match (ranked last).
    pub disconnected: bool,
}

impl ResultRow {
    /// Survival time in seconds.
    pub fn survived_secs(&self) -> f32 {
        self.survived_ticks as f32 * DT
    }
}

/// The final table of a match, sorted by place then player id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResultsView {
    pub rows: Vec<ResultRow>,
    /// Why the match ended (`None` only if built before the end).
    pub end_reason: Option<EndReason>,
    pub round: u32,
    /// Solo only: distance and coins.
    pub solo: Option<SoloResult>,
}

impl ResultsView {
    pub fn row(&self, id: PlayerId) -> Option<&ResultRow> {
        self.rows.iter().find(|r| r.player == id)
    }
}

/// Results of a world from `World::standings()`, shared by the host and the
/// local runs.
pub fn results_from_world(w: &World) -> ResultsView {
    let rows = w
        .standings()
        .into_iter()
        .map(|s| {
            let spec = w.config().players.iter().find(|p| p.id == s.player);
            ResultRow {
                player: s.player,
                name: spec.map_or_else(String::new, |p| p.name.clone()),
                colour: spec.map_or(0, |p| p.colour),
                hat: spec.map_or(PlayerSpec::DEFAULT_HAT, |p| p.hat),
                bot: spec.is_some_and(|p| p.bot.is_some()),
                place: s.place,
                alive: s.alive,
                survived_ticks: s.survived_ticks,
                feathers: s.feathers,
                coins: s.coins,
                placements: s.placements,
                disconnected: s.disconnected,
            }
        })
        .collect();
    ResultsView { rows, end_reason: w.end_reason(), round: w.config().round, solo: w.solo_result() }
}

/// What the run screen drives, for every mode.
///
/// Call `update` once per rendered frame with the frame time in milliseconds
/// (any monotonic clock) and the player's current [`InputFrame`] (bump `seq`
/// once per new command and keep handing the same frame until the next
/// command), then read `view()`.
pub trait RunDriver {
    fn update(&mut self, now_ms: u64, input: InputFrame);
    /// Ask to place a ghost tool; returns the request id. The answer arrives
    /// in a later `view().place_results` as `(req, result)`.
    fn place(&mut self, tool: Tool, lane: u8, row: u32) -> u32;
    /// The current frame; `None` before a match exists.
    fn view(&self) -> Option<&FlockView>;
    /// Set once the match is over.
    fn results(&self) -> Option<&ResultsView>;
    /// Pause/resume (offline runs only; a no-op online).
    fn set_paused(&mut self, p: bool);
}
