//! `flock-core`: the pure gameplay simulation for Cluster Flock.
//!
//! No I/O, no rendering, no threads. One [`World`] is stepped at a fixed
//! [`TICK_HZ`]; solo runs it locally, multiplayer runs it on the host and
//! ships snapshots/events. See the crate README for the full contract.
//!
//! Coordinates (SPEC §2): metres, `x` lateral (+ = runner's right), `y` up
//! (0 = ground), `z` forward along the course (`f64`). The renderer's `right`
//! for yaw 0 is −X, so the presenter maps sim `x` to render `-x`.
#![forbid(unsafe_code)]

pub mod bot;
pub mod course;
pub mod det;
pub mod feas;
pub mod flop;
pub mod input;
mod place;
pub mod runner;
pub mod tuning;
mod world;

use serde::{Deserialize, Serialize};

pub use course::{BeamSpan, Course, Damage, Obj, ObjId, ObjKind, PowerKind, Row, GENERATOR_VERSION};
pub use input::{CmdLatch, InputFrame};
pub use runner::{predict_runner, Loco, Pos, PredictCtx, Runner, RunnerStats, Vel};
pub use tuning::{HazardGeom, ToolCosts, Tuning};
pub use world::{course_fingerprint, course_rows, Player, World};

/// Simulation rate.
pub const TICK_HZ: u32 = 60;
/// Seconds per tick.
pub const DT: f32 = 1.0 / TICK_HZ as f32;
/// Room size; every per-player structure is sized for this.
pub const MAX_PLAYERS: usize = 6;
/// Lanes 0, 1, 2 (left → right).
pub const LANES: u8 = 3;

/// Simulation tick counter (ticks since `World::new`).
pub type Tick = u64;

/// Player slot 0..=5.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub u8);

/// Runner input. The app sends edges only (held keys never repeat).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Command {
    Left,
    Right,
    Jump,
    Down,
    JumpLeft,
    JumpRight,
    DiveLeft,
    DiveRight,
}

impl Command {
    /// Lateral lane delta implied by the command.
    pub fn lane_delta(self) -> i8 {
        match self {
            Command::Left | Command::JumpLeft | Command::DiveLeft => -1,
            Command::Right | Command::JumpRight | Command::DiveRight => 1,
            Command::Jump | Command::Down => 0,
        }
    }
    pub fn is_jump(self) -> bool {
        matches!(self, Command::Jump | Command::JumpLeft | Command::JumpRight)
    }
    pub fn is_down(self) -> bool {
        matches!(self, Command::Down | Command::DiveLeft | Command::DiveRight)
    }
}

/// Ghost architect tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tool {
    Coins,
    Mud,
    Hay,
    Beam,
}

impl Tool {
    pub const ALL: [Tool; 4] = [Tool::Coins, Tool::Mud, Tool::Hay, Tool::Beam];
    /// Solid tools count toward density and feasibility.
    pub fn is_solid(self) -> bool {
        matches!(self, Tool::Hay | Tool::Beam)
    }
}

/// A ghost's placement request. `req` is a per-player sequence number used
/// for idempotent retries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlaceRequest {
    pub player: PlayerId,
    pub req: u32,
    pub tool: Tool,
    pub lane: u8,
    pub row: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Accepted {
    /// The placed object (for `Tool::Coins`, the first of five consecutive ids).
    pub obj: ObjId,
    /// Energy charged.
    pub cost: f32,
}

/// Placement rejection, in validation order (SPEC §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Reject {
    /// Sender is not a ghost (runner, disconnected, unknown id, or match not running).
    NotGhost,
    /// A stale `req` older than the stored retry window. (A repeated `req`
    /// still inside the window returns the original result instead.)
    Duplicate,
    /// Row outside the placement band, not generated, or lane out of range.
    OutsideBand,
    /// Within `min_reaction_s` of a living runner.
    TooClose,
    /// Cell already has a solid / same-kind object / pending placement.
    Occupied,
    /// Shared architect density limit.
    Density,
    /// Would leave no feasible route.
    Unsafe,
    NoEnergy,
}

pub type PlaceResult = Result<Accepted, Reject>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    Solo,
    Multi,
}

/// Bot skill 0.0 (clumsy) ..= 1.0 (best).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BotSkill(pub f32);

impl BotSkill {
    pub const EASY: BotSkill = BotSkill(0.25);
    pub const NORMAL: BotSkill = BotSkill(0.55);
    pub const HARD: BotSkill = BotSkill(0.8);
    pub const MAX: BotSkill = BotSkill(1.0);
    pub fn level(self) -> f32 {
        if self.0.is_finite() {
            self.0.clamp(0.0, 1.0)
        } else {
            0.5
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerSpec {
    pub id: PlayerId,
    pub name: String,
    pub colour: u8,
    /// `Some` → the world drives this player itself (runner bot, then ghost bot).
    pub bot: Option<BotSkill>,
    /// Cosmetic hat id; [`PlayerSpec::DEFAULT_HAT`] (255) = "the default hat
    /// for my colour". No effect on the simulation.
    #[serde(default = "default_hat")]
    pub hat: u8,
}

fn default_hat() -> u8 {
    PlayerSpec::DEFAULT_HAT
}

impl PlayerSpec {
    /// `hat` value meaning "the default hat for my colour".
    pub const DEFAULT_HAT: u8 = 255;
    /// A player with the default hat.
    pub fn new(id: PlayerId, name: impl Into<String>, colour: u8, bot: Option<BotSkill>) -> PlayerSpec {
        PlayerSpec { id, name: name.into(), colour, bot, hat: Self::DEFAULT_HAT }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatchConfig {
    pub seed: u64,
    pub mode: Mode,
    pub players: Vec<PlayerSpec>,
    pub tuning: Tuning,
    /// Round number; rotates the start grid.
    pub round: u32,
}

impl MatchConfig {
    /// Solo, one human player 0.
    pub fn solo(seed: u64) -> Self {
        Self {
            seed,
            mode: Mode::Solo,
            players: vec![PlayerSpec::new(PlayerId(0), "You", 0, None)],
            tuning: Tuning::default(),
            round: 0,
        }
    }
    /// The daily run: solo rules on a published seed. Its course is a pure
    /// function of `(seed, row, tuning, GENERATOR_VERSION)`, identical on
    /// every device (`course_fingerprint` proves it).
    pub fn daily(seed: u64) -> Self {
        Self::solo(seed)
    }
    /// Multi with `n` players (clamped 1..=6); `bots` gives each a skill or None.
    pub fn multi(seed: u64, n: usize, bot: Option<BotSkill>) -> Self {
        let n = n.clamp(1, MAX_PLAYERS);
        Self {
            seed,
            mode: Mode::Multi,
            players: (0..n)
                .map(|i| PlayerSpec::new(PlayerId(i as u8), format!("P{}", i + 1), i as u8, bot))
                .collect(),
            tuning: Tuning::default(),
            round: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Countdown,
    Running,
    Over,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum EndReason {
    /// Solo: the runner lost the final feather.
    FinalFeather,
    /// Multi: every runner has been eliminated.
    NoRunnersLeft,
    /// Multi: `mp_cap` elapsed.
    TimeCap,
    /// Multi: fewer than two connected players remain.
    NotEnoughPlayers,
    /// Ended from outside (`World::abort`), e.g. host left.
    Aborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Role {
    Runner,
    Ghost { energy: f32, placed: u32 },
    Disconnected,
}

impl Role {
    pub fn is_runner(&self) -> bool {
        matches!(self, Role::Runner)
    }
    pub fn is_ghost(&self) -> bool {
        matches!(self, Role::Ghost { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Standing {
    pub player: PlayerId,
    /// 1-based, shared on ties (competition ranking: 1, 1, 3).
    pub place: u8,
    pub alive: bool,
    pub survived_ticks: u64,
    pub feathers: u8,
    pub coins: u32,
    pub placements: u32,
    pub disconnected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SoloResult {
    pub distance_m: f32,
    pub coins: u32,
}

/// Inclusive row index range; empty when `first > last`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RowRange {
    pub first: u32,
    pub last: u32,
}

impl RowRange {
    pub const EMPTY: RowRange = RowRange { first: 1, last: 0 };
    pub fn is_empty(&self) -> bool {
        self.first > self.last
    }
    pub fn contains(&self, row: u32) -> bool {
        row >= self.first && row <= self.last
    }
    pub fn iter(&self) -> std::ops::RangeInclusive<u32> {
        self.first..=self.last
    }
}

/// Everything that happened in one tick (`World::events`). Events caused
/// between steps (`place`, `disconnect`, `force_eliminate`, `abort`) appear in
/// the next step's list.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// Countdown finished; running starts this tick.
    Go,
    LaneChange { player: PlayerId, to: u8 },
    /// Lane command at an edge: lurched into the fence (no damage).
    FenceBump { player: PlayerId },
    WindUp { player: PlayerId, cmd: Command },
    Jumped { player: PlayerId },
    Collapsed { player: PlayerId },
    Landed { player: PlayerId },
    StoodUp { player: PlayerId },
    /// Involuntary hop off a collapsed/tumbling body.
    Hop { player: PlayerId, over: PlayerId },
    /// Airborne runner clipped a low body: trajectory disrupted.
    Disrupted { player: PlayerId, by: PlayerId },
    /// New runner–runner contact above `significant_impact`.
    Bump { a: PlayerId, b: PlayerId, speed: f32 },
    Tumble { player: PlayerId },
    /// A damaging obstacle hit: tumble + knockback. `feather_lost` false when
    /// a shield took it (`ShieldBroken` in the same tick) or a further contact
    /// in a tick that already cost a feather.
    Crash { player: PlayerId, obj: ObjId, kind: ObjKind, feather_lost: bool },
    MudEnter { player: PlayerId },
    /// A protection episode started (feather lost or shield broken); its one
    /// free hit is available (`Runner::free_hit`).
    ProtectionStart { player: PlayerId, until: Tick },
    /// Protection ran out (a second hit ends it with a new `ProtectionStart` instead).
    ProtectionEnd { player: PlayerId },
    /// A protection episode passed `long_protection_log_s` (logged once).
    LongProtection { player: PlayerId, ticks: u64 },
    /// A coin was collected (remove it from a remote course).
    CoinTaken { obj: ObjId, by: PlayerId },
    Eliminated { player: PlayerId },
    BecameGhost { player: PlayerId },
    /// Accepted placement; apply remotely with `Course::add_placement`.
    Placed { by: PlayerId, obj: ObjId, tool: Tool, lane: u8, row: u32, solid_at: Tick },
    /// A ghost-placed object became solid/live.
    Materialised { obj: ObjId },
    Disconnected { player: PlayerId },
    MatchOver { reason: EndReason },
    /// The barrels of course row `row` started rolling at `tick` (apply on a
    /// remote course with `Course::release_barrel(row, tick)`).
    BarrelReleased { row: u32, tick: Tick },
    /// The protection episode's free hit: the runner carried on through
    /// `obj` untouched (cosmetic; the next hit in the episode counts).
    Swagger { player: PlayerId, obj: ObjId },
    /// A jump was refused: the runner stands in mud.
    MudStuck { player: PlayerId },
    /// A power-up was collected (apply remotely with `Course::take_power(obj)`).
    PowerTaken { obj: ObjId, by: PlayerId, kind: PowerKind },
    /// A held shield absorbed a crash (no feather lost) and is gone.
    ShieldBroken { player: PlayerId },
}
