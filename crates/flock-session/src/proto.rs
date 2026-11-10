//! The wire protocol (SPEC §10): reliable messages both ways, 60 Hz lossy
//! `Inputs` up, 30 Hz lossy `Snapshot`s down. bincode (varint) throughout.
//!
//! Limits (fray-net): every reliable message <= [`MAX_RELIABLE_LEN`] bytes,
//! every lossy one <= [`MAX_LOSSY_LEN`]. [`encode`] never checks; the senders
//! do (events and rows are chunked, snapshots are budgeted with
//! [`Snapshot::budget`]).
//!
//! Enum variants are tag-indexed: only append new variants **last**, never
//! reorder; an incompatible change bumps [`PROTO_VERSION`].

use bincode::Options;
use flock_core::{
    EndReason, Event, Loco, MatchConfig, Phase, PlaceResult, PlayerId, Role, Row, Runner, Tick, Tool,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::view::ResultsView;

/// fray-net's reliable message cap (both ends must agree; it is part of the
/// app tag's contract).
pub const MAX_RELIABLE_LEN: usize = 1024;
pub const MAX_LOSSY_LEN: usize = MAX_RELIABLE_LEN;

/// Bumped on any incompatible wire change. A `Hello` with another number is
/// refused.
pub const PROTO_VERSION: u32 = 3;
/// Seats in a match: humans plus host-added bots (= `flock_core::MAX_PLAYERS`).
pub const MAX_SEATS: usize = flock_core::MAX_PLAYERS;
/// Player colours (each seat holds one, unique in the lobby).
pub const COLOURS: u8 = 6;
/// Longest display name, in characters.
pub const NAME_CHARS: usize = 12;
/// Sim ticks between snapshots (60 Hz / 2 = 30 Hz).
pub const SNAPSHOT_EVERY_TICKS: u64 = 2;
/// Inputs sent per `Inputs` datagram (the newest, redundantly).
pub const INPUTS_PER_PACKET: usize = 5;
/// Hard cap on a decoded message (alloc guard; real ones are <= 1024).
const DECODE_LIMIT: u64 = 4096;

fn opts() -> impl Options {
    bincode::DefaultOptions::new().with_limit(DECODE_LIMIT)
}

/// Serialise a message.
pub fn encode<T: Serialize>(v: &T) -> Vec<u8> {
    opts().serialize(v).unwrap_or_default()
}

/// Serialised size without allocating.
pub fn encoded_len<T: Serialize>(v: &T) -> usize {
    opts().serialized_size(v).unwrap_or(u64::MAX) as usize
}

/// Parse a message; `None` if it is malformed (never panics).
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    opts().deserialize(bytes).ok()
}

/// The `proto` of a `ToHost::Hello` from any protocol version (the variant
/// tag and `proto` lead every version's Hello), so a client too old or new to
/// decode can still be refused with the reason.
pub fn hello_proto(bytes: &[u8]) -> Option<u32> {
    let (tag, proto): (u32, u32) = opts().allow_trailing_bytes().deserialize(bytes).ok()?;
    (tag == 0).then_some(proto)
}

// ─── Reliable: client -> host ───────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ToHost {
    /// First message. `colour` is the preferred colour (0..6); the host gives
    /// the first free one if it is taken. `hat`: cosmetic hat id, 255 = the
    /// default hat for my colour. (`proto` stays the first field: an older
    /// client's Hello is recognised by [`hello_proto`] and refused by version.)
    Hello { proto: u32, name: String, colour: u8, hat: u8 },
    /// Lobby: pick a colour (refused if another human holds it).
    SetColour(u8),
    /// Lobby ready flag.
    SetReady(bool),
    /// Results-screen ready flag (everyone ready starts the rematch 3-2-1).
    RematchReady(bool),
    Leave,
    /// Host clock sync.
    Ping { id: u32, client_t: u64 },
    /// Ghost placement. `run` is `Start::run` (stragglers from an earlier
    /// round are dropped); `req` is the per-player request id the answer
    /// echoes. The host's world is idempotent per `(player, req)`.
    Place { run: u8, req: u32, tool: Tool, lane: u8, row: u32 },
}

// ─── Reliable: host -> client ───────────────────────────────────────────────

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ToClient {
    /// You are in. `seat` is your stable lobby seat id (not your `PlayerId`,
    /// which `Start` gives).
    Welcome { proto: u32, seat: u8 },
    /// The roster (lobby, and during results where `ready` means rematch-ready).
    Lobby(LobbyView),
    /// Build the match from `config`; world tick 0 is host-clock
    /// `start_host_ms`. `you` is your player. `run` numbers the round (wraps)
    /// so stragglers from the previous one are dropped after a rematch.
    Start { config: MatchConfig, start_host_ms: u64, you: PlayerId, run: u8 },
    /// Generated course rows, in order (a chunk of at most [`MAX_RELIABLE_LEN`]).
    Rows { run: u8, rows: Vec<Row> },
    /// Sim events for sound, FX, HUD and the remote course.
    Events { run: u8, events: Vec<Event> },
    /// The answer to your `ToHost::Place`.
    PlaceResult { run: u8, req: u32, result: PlaceResult },
    Results(ResultsView),
    Rematch(RematchMsg),
    Pong { id: u32, client_t: u64, host_t: u64 },
    /// The host ended your link (full, version, declined, host leaving).
    Closed(String),
    /// A player left the match (their body is removed; the results list them last).
    Left { player: PlayerId },
    /// Bluetooth: you are connected and waiting for the host to accept you.
    Waiting,
    /// The host's own My Signet attestation (JSON). Sent to Bluetooth joiners
    /// after acceptance: their transport could not check it before joining
    /// (fray withholds it from the radio-range welcome), so the client
    /// verifies it now against the host key QUIC proved.
    HostAttestation(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RematchMsg {
    /// Everyone is ready: the rematch starts at this host-clock time.
    Countdown { ends_at_host_ms: u64 },
    /// Someone un-readied or left.
    Cancelled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LobbyPhase {
    Lobby,
    Running,
    Results,
}

/// How a seat reached the host (decided by the host's own door, never by the
/// joiner's claim).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SeatPath {
    /// The host's own player, or a bot.
    Local,
    /// Online (room code) or Nearby (LAN).
    Network,
    Bluetooth,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeatView {
    /// Seat id. Humans: 0..100 in join order, kept while seated. Bots:
    /// `100 + n` (stable while the bot sits).
    pub id: u8,
    pub name: String,
    /// 0..6, unique in the lobby.
    pub colour: u8,
    /// Cosmetic hat id (255 = default for the colour; bots always 255).
    pub hat: u8,
    pub ready: bool,
    pub bot: bool,
    /// The host's own player.
    pub host: bool,
    /// The seat's verified My Signet persona (`npub1...`), when the host
    /// verified one (every network joiner; the host's own seat when it has an
    /// attestation; never bots).
    pub persona: Option<String>,
    pub path: SeatPath,
    /// `PlayerId` in the current match (from `Start` on), `None` in the lobby.
    pub player: Option<PlayerId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyView {
    pub seats: Vec<SeatView>,
    pub phase: LobbyPhase,
    /// Rounds started so far (the next match is round `round`).
    pub round: u32,
}

impl LobbyView {
    pub fn humans(&self) -> usize {
        self.seats.iter().filter(|s| !s.bot).count()
    }
    pub fn bots(&self) -> usize {
        self.seats.iter().filter(|s| s.bot).count()
    }
    pub fn seat(&self, id: u8) -> Option<&SeatView> {
        self.seats.iter().find(|s| s.id == id)
    }
    /// The host's own seat.
    pub fn host_seat(&self) -> Option<&SeatView> {
        self.seats.iter().find(|s| s.host)
    }
    /// Colours nobody holds.
    pub fn free_colours(&self) -> Vec<u8> {
        (0..COLOURS).filter(|c| !self.seats.iter().any(|s| s.colour == *c)).collect()
    }
}

// ─── Lossy ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LossyToHost {
    /// `inputs` are the newest packed `InputFrame`s, oldest first; the last
    /// one has number `seq`. Input number `n` drives world tick `n - 1`.
    Inputs { run: u8, seq: u32, inputs: Vec<[u8; 2]> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum LossyToClient {
    Snapshot(Snapshot),
}

/// Another player's body, compact.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunnerNet {
    pub id: PlayerId,
    pub x: f32,
    pub y: f32,
    pub z: f64,
    /// `[x, y, z_off]`.
    pub vel: [f32; 3],
    pub loco: Loco,
    pub feathers: u8,
    pub protected_until: Option<Tick>,
    pub coins: u32,
    pub target_lane: u8,
    pub in_mud: bool,
    /// [`RunnerNet::FREE_HIT`] | [`RunnerNet::MAGNET`] | [`RunnerNet::DOUBLER`]
    /// | [`RunnerNet::SHIELD`], as of the snapshot's tick.
    pub flags: u8,
}

impl RunnerNet {
    /// The protection episode's free hit is still available.
    pub const FREE_HIT: u8 = 1;
    pub const MAGNET: u8 = 2;
    pub const DOUBLER: u8 = 4;
    pub const SHIELD: u8 = 8;

    /// `now`: the tick the power-up timers are judged at (the world's tick).
    pub fn from_runner(r: &Runner, now: Tick) -> RunnerNet {
        let mut flags = 0;
        for (on, f) in [(r.free_hit, Self::FREE_HIT), (r.magnet(now), Self::MAGNET), (r.doubler(now), Self::DOUBLER), (r.shield(now), Self::SHIELD)] {
            if on {
                flags |= f;
            }
        }
        RunnerNet {
            id: r.id,
            x: r.pos.x,
            y: r.pos.y,
            z: r.pos.z,
            vel: [r.vel.x, r.vel.y, r.vel.z_off],
            loco: r.loco,
            feathers: r.feathers,
            protected_until: r.protected_until,
            coins: r.coins,
            target_lane: r.target_lane,
            in_mud: r.in_mud,
            flags,
        }
    }

    /// Write this state onto a body (keeps the fields the net form lacks).
    pub fn apply(&self, r: &mut Runner) {
        r.pos.x = self.x;
        r.pos.y = self.y;
        r.pos.z = self.z;
        r.vel.x = self.vel[0];
        r.vel.y = self.vel[1];
        r.vel.z_off = self.vel[2];
        r.loco = self.loco;
        r.feathers = self.feathers;
        r.protected_until = self.protected_until;
        r.coins = self.coins;
        r.target_lane = self.target_lane;
        r.in_mud = self.in_mud;
        // Timers are not sent for others: an active effect reads as active
        // until the next snapshot says otherwise.
        let until = |f: u8| if self.flags & f != 0 { Tick::MAX } else { 0 };
        r.free_hit = self.flags & Self::FREE_HIT != 0;
        r.magnet_until = until(Self::MAGNET);
        r.double_until = until(Self::DOUBLER);
        r.shield_until = until(Self::SHIELD);
    }
}

/// Per-player scoreboard state.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerNet {
    pub id: PlayerId,
    pub role: Role,
    pub placements: u32,
    pub survived_ticks: u64,
}

/// One recipient's view of the world, 30 Hz. At most [`MAX_LOSSY_LEN`] bytes
/// after [`Snapshot::budget`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Which round (`Start::run`).
    pub run: u8,
    /// World ticks stepped when this was taken: the state after tick
    /// `tick - 1`, at host time `start_host_ms + tick * 1000 / 60`.
    pub tick: u32,
    /// The recipient's last applied input number (real or repeated).
    pub ack_seq: u32,
    pub phase: Phase,
    pub end: Option<EndReason>,
    /// `World::v_course()` / `anchor_z()` after tick `tick - 1`.
    pub v_course: f32,
    pub anchor: f64,
    /// Mean z of the living runners (the pack anchor follows it).
    pub mean_z: f64,
    /// The host course's first held row (release older rows).
    pub first_row: u32,
    /// The recipient's own body in full, while it is a runner
    /// (`touched_prev` and `incidents` are host-only and cleared).
    pub own: Option<Runner>,
    /// Every other living runner.
    pub bodies: Vec<RunnerNet>,
    /// Everyone's role and score.
    pub players: Vec<PlayerNet>,
}

impl Snapshot {
    /// Serialised size of the datagram.
    pub fn wire_len(&self) -> usize {
        1 + encoded_len(self)
    }

    /// Trim to the lossy budget: the own body's host-only lists go first,
    /// then the bodies farthest from `from` (z). Returns how many bodies
    /// were dropped (0 for any real 6-player match; see the worst-case test).
    pub fn budget(&mut self, from: f64) -> usize {
        if let Some(own) = self.own.as_mut() {
            own.touched_prev.clear();
            own.incidents.clear();
        }
        self.bodies.sort_by(|a, b| (a.z - from).abs().total_cmp(&(b.z - from).abs()));
        let mut dropped = 0;
        while self.wire_len() > MAX_LOSSY_LEN && self.bodies.pop().is_some() {
            dropped += 1;
        }
        dropped
    }

    /// Encode as a lossy datagram (same bytes as
    /// `encode(&LossyToClient::Snapshot(..))` without the clone).
    pub fn encode(&self) -> Vec<u8> {
        let mut v = vec![0u8];
        v.extend(encode(self));
        v
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Split `items` into messages built by `wrap` that each fit
/// [`MAX_RELIABLE_LEN`]. An item too big on its own is dropped (logged).
pub fn chunk<T: Serialize + Clone>(items: &[T], wrap: impl Fn(Vec<T>) -> ToClient) -> Vec<Vec<u8>> {
    // Tag (1) + run (1) + length varint (<= 3) + slack.
    const FRAME: usize = 8;
    let mut out = Vec::new();
    let mut cur: Vec<T> = Vec::new();
    let mut cur_len = FRAME;
    for it in items {
        let l = encoded_len(it);
        if l + FRAME > MAX_RELIABLE_LEN {
            log::error!("[proto] item of {l} bytes cannot be sent");
            continue;
        }
        if !cur.is_empty() && cur_len + l > MAX_RELIABLE_LEN {
            out.push(encode(&wrap(std::mem::take(&mut cur))));
            cur_len = FRAME;
        }
        cur.push(it.clone());
        cur_len += l;
    }
    if !cur.is_empty() {
        out.push(encode(&wrap(cur)));
    }
    out
}

/// Clean a display name: control characters out, trimmed, at most
/// [`NAME_CHARS`] characters; empty becomes `fallback`.
pub fn sanitize_name(raw: &str, fallback: &str) -> String {
    let s: String = raw.chars().filter(|c| !c.is_control()).collect();
    let s: String = s.trim().chars().take(NAME_CHARS).collect();
    if s.is_empty() {
        fallback.to_string()
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flock_core::{BotSkill, Command, PlayerSpec, World, MAX_PLAYERS};

    fn long_name(i: usize) -> String {
        // 12 four-byte characters: the longest name in bytes.
        std::iter::repeat_n('\u{1F426}', NAME_CHARS).collect::<String>() + &i.to_string()
    }

    /// A six-seat config with the longest names.
    pub(crate) fn big_config() -> MatchConfig {
        let mut cfg = MatchConfig::multi(u64::MAX, MAX_PLAYERS, Some(BotSkill::NORMAL));
        for (i, p) in cfg.players.iter_mut().enumerate() {
            *p = PlayerSpec { id: p.id, name: sanitize_name(&long_name(i), "x"), colour: i as u8, bot: Some(BotSkill(0.123_456)), hat: 254 };
        }
        cfg.round = u32::MAX;
        cfg
    }

    #[test]
    fn start_fits_one_reliable_message() {
        let m = ToClient::Start { config: big_config(), start_host_ms: u64::MAX, you: PlayerId(5), run: 255 };
        let n = encoded_len(&m);
        assert!(n <= MAX_RELIABLE_LEN, "Start is {n} bytes");
    }

    /// The fullest snapshot: six players, own body with every optional field
    /// and long host-only lists (which the budget clears), others at the
    /// widest varints.
    #[test]
    fn worst_case_six_player_snapshot_fits_the_budget() {
        let mut w = World::new(big_config());
        for _ in 0..600 {
            w.step(&[(PlayerId(0), Command::JumpLeft)]);
        }
        let mut own = w.players()[0].runner.clone();
        own.buffered = Some((Command::DiveRight, u64::MAX));
        own.protected_until = Some(u64::MAX);
        own.protect_since = Some(u64::MAX);
        own.coins = u32::MAX;
        own.alive_ticks = u64::MAX;
        own.loco = Loco::Tumbling { until: u64::MAX, spin_dir: -1 };
        own.free_hit = true;
        own.swagger_obj = Some(flock_core::ObjId(u32::MAX));
        own.magnet_until = u64::MAX;
        own.double_until = u64::MAX;
        own.shield_until = u64::MAX;
        own.speed_bonus = f32::MAX;
        own.stats.swaggers = u32::MAX;
        own.stats.mud_stuck = u32::MAX;
        own.stats.crashes = u32::MAX;
        own.stats.longest_protection_ticks = u64::MAX;
        own.touched_prev = (0..200).map(|i| flock_core::ObjId(0x8000_0000 + i)).collect();
        own.incidents = (0..200).map(|i| flock_core::ObjId(0x8000_0000 + i)).collect();
        let body = |i: u8| RunnerNet {
            id: PlayerId(i),
            x: f32::MAX,
            y: f32::MAX,
            z: f64::MAX,
            vel: [f32::MAX; 3],
            loco: Loco::Tumbling { until: u64::MAX, spin_dir: -1 },
            feathers: 255,
            protected_until: Some(u64::MAX),
            coins: u32::MAX,
            target_lane: 2,
            in_mud: true,
            flags: 0xFF,
        };
        let mut s = Snapshot {
            run: 255,
            tick: u32::MAX,
            ack_seq: u32::MAX,
            phase: Phase::Running,
            end: Some(EndReason::NotEnoughPlayers),
            v_course: f32::MAX,
            anchor: f64::MAX,
            mean_z: f64::MAX,
            first_row: u32::MAX,
            own: Some(own),
            bodies: (1..MAX_PLAYERS as u8).map(body).collect(),
            players: (0..MAX_PLAYERS as u8)
                .map(|i| PlayerNet {
                    id: PlayerId(i),
                    role: Role::Ghost { energy: f32::MAX, placed: u32::MAX },
                    placements: u32::MAX,
                    survived_ticks: u64::MAX,
                })
                .collect(),
        };
        assert!(s.wire_len() > MAX_LOSSY_LEN, "the host-only lists alone overflow ({} B)", s.wire_len());
        let dropped = s.budget(0.0);
        assert_eq!(dropped, 0, "no body is ever dropped for six players");
        let n = s.encode().len();
        assert!(n <= MAX_LOSSY_LEN, "snapshot is {n} bytes");
        eprintln!("worst-case 6-player snapshot: {n} of {MAX_LOSSY_LEN} bytes");
        let back: LossyToClient = decode(&s.encode()).expect("decodes");
        assert_eq!(back, LossyToClient::Snapshot(s));
    }

    #[test]
    fn rows_and_events_chunk_under_the_limit() {
        let w = World::new(big_config());
        let rows: Vec<Row> = w.course().rows().copied().collect();
        assert!(!rows.is_empty());
        let msgs = chunk(&rows, |rows| ToClient::Rows { run: 1, rows });
        assert!(msgs.iter().all(|m| m.len() <= MAX_RELIABLE_LEN));
        let mut back = Vec::new();
        for m in &msgs {
            let Some(ToClient::Rows { rows, .. }) = decode(m) else { panic!("decode") };
            back.extend(rows);
        }
        assert_eq!(back, rows);
        let evs: Vec<Event> = (0..500).map(|i| Event::CoinTaken { obj: flock_core::ObjId(i * 1000), by: PlayerId(3) }).collect();
        let msgs = chunk(&evs, |events| ToClient::Events { run: 1, events });
        assert!(msgs.len() > 1 && msgs.iter().all(|m| m.len() <= MAX_RELIABLE_LEN));
    }

    #[test]
    fn decode_never_panics_on_garbage() {
        for n in 0..300u32 {
            let bytes: Vec<u8> = (0..(n % 40)).map(|k| (k * 37 + n * 11) as u8).collect();
            let _ = decode::<ToHost>(&bytes);
            let _ = decode::<ToClient>(&bytes);
            let _ = decode::<LossyToHost>(&bytes);
            let _ = decode::<LossyToClient>(&bytes);
        }
        // A huge declared length is refused by the limit, not allocated.
        assert!(decode::<ToClient>(&[2, 0xFC, 0xFF, 0xFF, 0xFF, 0x7F]).is_none());
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(sanitize_name("  a\u{7}b  ", "x"), "ab");
        assert_eq!(sanitize_name("", "x"), "x");
        assert_eq!(sanitize_name("abcdefghijklmnopq", "x").chars().count(), NAME_CHARS);
    }
}
