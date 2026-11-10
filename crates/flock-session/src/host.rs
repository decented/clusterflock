//! [`HostAuthority`]: the lobby and the match, host side (SPEC §10).
//!
//! The host runs `flock_core::World`; host-added bots are `PlayerSpec.bot`
//! seats simulated inside it. The host's own player is an ordinary
//! [`crate::ClientSession`] on a zero-latency loopback link (see
//! [`crate::MuxHostTransport`]). Driven as `update(now_ms)` with any monotonic
//! millisecond clock; the clients sync to it.
//!
//! Timeline: world tick `t` happens at host time `start_host_ms + t * 1000 / 60`
//! and consumes input number `t + 1` of every human seat. A tick whose input
//! never arrived repeats the seat's last frame (harmless: the `CmdLatch` only
//! fires on a new `seq`, so the real frame fires one tick late instead).
//! Late copies for ticks already stepped are dropped.
//!
//! ## Admission (reviewed code)
//!
//! * A peer's path and verified persona come only from the transport's
//!   `HostNetEvent::Joined` (the door it came through), never from its Hello.
//!   A Hello from a remote peer with no `Joined` record is refused.
//! * A Bluetooth joiner is not seated by its Hello: it waits in
//!   [`HostAuthority::pending_joiners`] (told `Waiting`) until the host calls
//!   [`HostAuthority::accept_pending`]; [`HostAuthority::decline_pending`] or
//!   [`PENDING_MS`] without an answer disconnects it. A waiting peer's other
//!   messages are ignored.
//! * After acceptance a Bluetooth joiner is sent the host's attestation
//!   (`ToClient::HostAttestation`) to verify the host it could not check
//!   before joining.

use std::collections::{BTreeMap, HashMap};

use flock_core::{
    BotSkill, CmdLatch, Event, InputFrame, MatchConfig, Mode, Phase, PlaceRequest, PlayerId, PlayerSpec, Tuning, World,
    TICK_HZ,
};
use serde::{Deserialize, Serialize};

use crate::proto::{
    chunk, decode, encode, sanitize_name, LobbyPhase, LobbyView, LossyToHost, PlayerNet, RematchMsg, RunnerNet,
    SeatPath, SeatView, Snapshot, ToClient, ToHost, COLOURS, INPUTS_PER_PACKET, MAX_LOSSY_LEN, MAX_RELIABLE_LEN,
    MAX_SEATS, PROTO_VERSION, SNAPSHOT_EVERY_TICKS,
};
use crate::transport::{HostNetEvent, HostTransport, LinkPath, PeerKey, LOCAL_PEER_BASE};
use crate::view::{results_from_world, ResultsView};

/// Host-clock lead between `Start` and tick 0, so every client has the
/// message before the countdown begins.
pub const START_LEAD_MS: u64 = 700;
/// The rematch 3-2-1.
pub const REMATCH_MS: u64 = 3000;
/// A seat that sends nothing for this long mid-match is disconnected.
pub const SILENCE_MS: u64 = 6000;
/// A Bluetooth joiner the host has not accepted by then is turned away.
pub const PENDING_MS: u64 = 60_000;
/// Joiners waiting for Accept at once (more are turned away).
pub const MAX_PENDING: usize = 6;
/// Most world ticks stepped in one `update` (a stalled app catches up gradually).
const MAX_CATCH_UP: u32 = 12;
/// Inputs queued per seat at most.
const MAX_QUEUED: usize = 120;
/// Human seat ids are below this; bot seats are `BOT_ID_BASE + n`.
pub const BOT_ID_BASE: u8 = 100;
/// The reason a client is given when the host leaves (`ClientPhase::HostLeft`).
pub const HOST_LEFT: &str = "host left";
/// A Bluetooth joiner is turned away when the host has no My Signet
/// attestation to show it.
pub const HOST_NOT_SIGNED_IN: &str = "host not signed in";

const BOT_NAMES: [&str; 6] = ["Pip", "Clover", "Nugget", "Bramble", "Maple", "Dot"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostPhase {
    Lobby,
    /// From `Start` to the results: the countdown and the match.
    Running,
    Results,
}

/// Lobby settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HostConfig {
    /// Base seed; each round derives its own.
    pub seed: u64,
    /// Skill of host-added bots.
    pub bot_skill: BotSkill,
    /// Start when every human is ready (and again for rematches).
    pub auto_start: bool,
    pub tuning: Tuning,
}

impl HostConfig {
    pub fn new(seed: u64) -> HostConfig {
        HostConfig { seed, bot_skill: BotSkill::NORMAL, auto_start: true, tuning: Tuning::default() }
    }
}

impl Default for HostConfig {
    /// A seed from the wall clock (rounds differ between rooms).
    fn default() -> Self {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        HostConfig::new(mix64(t as u64 ^ (t >> 64) as u64))
    }
}

/// Counters for tests and debug overlays.
#[derive(Clone, Debug, Default)]
pub struct HostStats {
    pub snapshots_sent: u64,
    pub snapshot_bytes_max: usize,
    /// Bodies dropped to keep a snapshot inside the budget (0 in practice).
    pub bodies_dropped: u64,
    /// Ticks where a seat's input had not arrived (its last frame was repeated).
    pub starved_ticks: u64,
    pub ticks: u64,
    pub rows_sent: u64,
    pub places: u64,
}

/// A Bluetooth joiner waiting for the host's Accept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingJoiner {
    pub peer: PeerKey,
    /// The name it typed (self-chosen: show it beside the persona).
    pub name: String,
    /// Its verified My Signet persona (`npub1...`), from the transport.
    pub persona: Option<String>,
    /// Host time it knocked (ms).
    pub since: u64,
    colour: u8,
    hat: u8,
}

/// What the transport said about a peer when it joined.
#[derive(Clone, Debug)]
struct Link {
    path: LinkPath,
    persona: Option<String>,
}

struct Human {
    id: u8,
    peer: PeerKey,
    name: String,
    colour: u8,
    hat: u8,
    ready: bool,
    host: bool,
    path: SeatPath,
    persona: Option<String>,
    last_heard: u64,
    /// Player in the current match.
    player: Option<PlayerId>,
    /// Input number -> packed frame.
    inq: BTreeMap<u32, [u8; 2]>,
    last_fed: [u8; 2],
    applied_seq: u32,
    /// Next course row to stream to this seat.
    next_row: u32,
}

impl Human {
    fn reset_run_input(&mut self) {
        self.inq.clear();
        self.applied_seq = 0;
        self.last_fed = InputFrame::default().pack();
        self.next_row = 0;
    }

    /// The frame for input number `want` (world tick `want - 1`) and whether
    /// it was missing (then the last frame repeats).
    fn feed(&mut self, want: u32) -> ([u8; 2], bool) {
        let (fed, starved) = match self.inq.remove(&want) {
            Some(p) => (p, false),
            None => (self.last_fed, true),
        };
        while let Some(&k) = self.inq.keys().next() {
            if k <= want {
                self.inq.remove(&k);
            } else {
                break;
            }
        }
        self.applied_seq = want;
        self.last_fed = fed;
        (fed, starved)
    }
}

struct Bot {
    id: u8,
    name: String,
    colour: u8,
    player: Option<PlayerId>,
}

struct Run {
    world: World,
    start_ms: u64,
    run_no: u8,
    events: Vec<Event>,
    results: Option<ResultsView>,
    results_pending: bool,
    last_snap_tick: u64,
    /// Answers to `Place`, sent with the next flush.
    answers: Vec<(PeerKey, ToClient)>,
}

pub struct HostAuthority {
    transport: Box<dyn HostTransport>,
    cfg: HostConfig,
    phase: HostPhase,
    humans: Vec<Human>,
    bots: Vec<Bot>,
    links: HashMap<PeerKey, Link>,
    pending: Vec<PendingJoiner>,
    by_peer: HashMap<PeerKey, u8>,
    next_id: u8,
    next_bot: u8,
    now: u64,
    rounds: u32,
    run: Option<Run>,
    latch: CmdLatch,
    rematch_at: Option<u64>,
    lobby_dirty: bool,
    rematch_dirty: bool,
    open: Option<bool>,
    stats: HostStats,
    host_attestation: Option<String>,
    host_persona: Option<String>,
}

impl HostAuthority {
    pub fn new(transport: Box<dyn HostTransport>, cfg: HostConfig) -> HostAuthority {
        HostAuthority {
            transport,
            cfg,
            phase: HostPhase::Lobby,
            humans: Vec::new(),
            bots: Vec::new(),
            links: HashMap::new(),
            pending: Vec::new(),
            by_peer: HashMap::new(),
            next_id: 0,
            next_bot: 0,
            now: 0,
            rounds: 0,
            run: None,
            latch: CmdLatch::new(),
            rematch_at: None,
            lobby_dirty: false,
            rematch_dirty: false,
            open: None,
            stats: HostStats::default(),
            host_attestation: None,
            host_persona: None,
        }
    }

    /// The host's own My Signet attestation (sent to Bluetooth joiners after
    /// Accept so they can verify the host) and its persona (`npub1...`, shown
    /// on the host's own seat).
    pub fn set_host_identity(&mut self, attestation: Option<String>, persona: Option<String>) {
        self.host_attestation = attestation;
        self.host_persona = persona;
        for h in self.humans.iter_mut().filter(|h| h.host) {
            h.persona = self.host_persona.clone();
        }
        self.lobby_dirty = true;
    }

    // ─── Queries ─────────────────────────────────────────────────────────

    pub fn phase(&self) -> HostPhase {
        self.phase
    }
    pub fn config(&self) -> &HostConfig {
        &self.cfg
    }
    pub fn stats(&self) -> &HostStats {
        &self.stats
    }
    /// The authoritative world while a match (or its results) exists.
    pub fn world(&self) -> Option<&World> {
        self.run.as_ref().map(|r| &r.world)
    }
    /// Debug/test hook: the authoritative world.
    pub fn world_mut(&mut self) -> Option<&mut World> {
        self.run.as_mut().map(|r| &mut r.world)
    }
    pub fn results(&self) -> Option<&ResultsView> {
        self.run.as_ref().and_then(|r| r.results.as_ref())
    }
    pub fn human_count(&self) -> usize {
        self.humans.len()
    }
    pub fn bot_count(&self) -> usize {
        self.bots.len()
    }
    /// Rounds started so far.
    pub fn rounds(&self) -> u32 {
        self.rounds
    }
    /// Bluetooth joiners waiting for Accept, oldest first.
    pub fn pending_joiners(&self) -> &[PendingJoiner] {
        &self.pending
    }
    /// The player a human seat plays in the current match.
    pub fn player_of_seat(&self, seat: u8) -> Option<PlayerId> {
        self.humans.iter().find(|h| h.id == seat).and_then(|h| h.player)
    }
    pub fn peer_of_seat(&self, seat: u8) -> Option<PeerKey> {
        self.humans.iter().find(|h| h.id == seat).map(|h| h.peer)
    }
    /// Host-clock time the current match's tick 0 happens.
    pub fn start_host_ms(&self) -> Option<u64> {
        self.run.as_ref().map(|r| r.start_ms)
    }
    pub fn rtt_ms(&self, seat: u8) -> Option<u32> {
        let h = self.humans.iter().find(|h| h.id == seat)?;
        self.transport.rtt_ms(h.peer)
    }
    /// When the rematch 3-2-1 ends (host clock), while one runs.
    pub fn rematch_ends_at(&self) -> Option<u64> {
        self.rematch_at
    }
    /// A match can start: at least two seats (humans + bots), one human.
    pub fn can_start(&self) -> bool {
        !self.humans.is_empty() && self.humans.len() + self.bots.len() >= 2
    }

    pub fn lobby_view(&self) -> LobbyView {
        let mut seats: Vec<SeatView> = self
            .humans
            .iter()
            .map(|h| SeatView {
                id: h.id,
                name: h.name.clone(),
                colour: h.colour,
                hat: h.hat,
                ready: h.ready,
                bot: false,
                host: h.host,
                persona: h.persona.clone(),
                path: h.path,
                player: if self.phase == HostPhase::Lobby { None } else { h.player },
            })
            .collect();
        seats.extend(self.bots.iter().map(|b| SeatView {
            id: b.id,
            name: b.name.clone(),
            colour: b.colour,
            hat: PlayerSpec::DEFAULT_HAT,
            ready: true,
            bot: true,
            host: false,
            persona: None,
            path: SeatPath::Local,
            player: if self.phase == HostPhase::Lobby { None } else { b.player },
        }));
        LobbyView {
            seats,
            phase: match self.phase {
                HostPhase::Lobby => LobbyPhase::Lobby,
                HostPhase::Running => LobbyPhase::Running,
                HostPhase::Results => LobbyPhase::Results,
            },
            round: self.rounds,
        }
    }

    fn colour_free(&self, c: u8, except_human: Option<u8>) -> bool {
        !self.humans.iter().any(|h| h.colour == c && Some(h.id) != except_human)
            && !self.bots.iter().any(|b| b.colour == c)
    }

    fn first_free_colour(&self) -> u8 {
        (0..COLOURS).find(|&c| self.colour_free(c, None)).unwrap_or(0)
    }

    /// Give `c` to a human: a bot holding it moves to a free colour.
    fn take_colour_from_bot(&mut self, c: u8) {
        if let Some(i) = self.bots.iter().position(|b| b.colour == c) {
            self.bots[i].colour = u8::MAX;
            let free = (0..COLOURS)
                .find(|&k| k != c && !self.humans.iter().any(|h| h.colour == k) && !self.bots.iter().any(|b| b.colour == k))
                .unwrap_or(0);
            self.bots[i].colour = free;
        }
    }

    // ─── Lobby controls (the host's UI) ──────────────────────────────────

    /// Add a bot seat (lobby only, while a seat is free).
    pub fn add_bot(&mut self) -> bool {
        if self.phase != HostPhase::Lobby || self.humans.len() + self.bots.len() >= MAX_SEATS {
            return false;
        }
        let Some(id) = (0..BOT_ID_BASE)
            .map(|k| BOT_ID_BASE + (self.next_bot.wrapping_add(k) % (255 - BOT_ID_BASE)))
            .find(|id| !self.bots.iter().any(|b| b.id == *id))
        else {
            return false;
        };
        self.next_bot = self.next_bot.wrapping_add(1);
        let used: Vec<&str> = self.bots.iter().map(|b| b.name.as_str()).collect();
        let name = BOT_NAMES.iter().find(|n| !used.contains(n)).unwrap_or(&BOT_NAMES[0]).to_string();
        let colour = self.first_free_colour();
        self.bots.push(Bot { id, name, colour, player: None });
        self.lobby_dirty = true;
        true
    }

    /// Remove a bot seat by seat id (`None`: the last one).
    pub fn remove_bot(&mut self, seat: Option<u8>) -> bool {
        if self.phase != HostPhase::Lobby || self.bots.is_empty() {
            return false;
        }
        let i = match seat {
            Some(id) => match self.bots.iter().position(|b| b.id == id) {
                Some(i) => i,
                None => return false,
            },
            None => self.bots.len() - 1,
        };
        self.bots.remove(i);
        self.lobby_dirty = true;
        true
    }

    pub fn set_bot_skill(&mut self, skill: BotSkill) {
        self.cfg.bot_skill = skill;
    }

    pub fn set_auto_start(&mut self, on: bool) {
        self.cfg.auto_start = on;
    }

    /// Remove a human seat (not the host's own) from the lobby.
    pub fn kick(&mut self, seat: u8) -> bool {
        let Some(h) = self.humans.iter().find(|h| h.id == seat && !h.host) else { return false };
        let peer = h.peer;
        self.send(peer, &ToClient::Closed("kicked".into()));
        self.on_left(peer);
        self.transport.kick(peer);
        true
    }

    /// Seat a waiting Bluetooth joiner. `false` if it is gone, or the lobby
    /// is not open (then it is turned away).
    pub fn accept_pending(&mut self, peer: PeerKey) -> bool {
        let Some(i) = self.pending.iter().position(|p| p.peer == peer) else { return false };
        let p = self.pending.remove(i);
        let link = self.links.get(&peer).cloned();
        let Some(link) = link.filter(|l| l.path == LinkPath::Bluetooth) else {
            self.refuse(peer, "not admitted");
            return false;
        };
        self.seat(peer, &p.name, p.colour, p.hat, SeatPath::Bluetooth, link.persona)
    }

    /// Turn a waiting Bluetooth joiner away.
    pub fn decline_pending(&mut self, peer: PeerKey) -> bool {
        let Some(i) = self.pending.iter().position(|p| p.peer == peer) else { return false };
        self.pending.remove(i);
        self.refuse(peer, "The host said no.");
        true
    }

    /// Start now (host's Start button). `false` if not possible.
    pub fn start(&mut self) -> bool {
        if self.phase != HostPhase::Lobby || !self.can_start() {
            return false;
        }
        self.begin_round();
        true
    }

    /// From the results, back to the lobby; every ready flag is cleared.
    pub fn to_lobby(&mut self) -> bool {
        if self.phase == HostPhase::Lobby {
            return false;
        }
        self.phase = HostPhase::Lobby;
        self.run = None;
        self.rematch_at = None;
        for h in &mut self.humans {
            h.ready = false;
            h.player = None;
            h.reset_run_input();
        }
        for b in &mut self.bots {
            b.player = None;
        }
        self.lobby_dirty = true;
        true
    }

    /// Tell everyone the host is leaving and drop the links.
    pub fn shutdown(&mut self) {
        let peers: Vec<PeerKey> = self.humans.iter().map(|h| h.peer).chain(self.pending.iter().map(|p| p.peer)).collect();
        for p in peers {
            self.send(p, &ToClient::Closed(HOST_LEFT.into()));
        }
        if let Some(run) = self.run.as_mut() {
            run.world.abort();
        }
        self.transport.shutdown();
        self.humans.clear();
        self.pending.clear();
        self.by_peer.clear();
        self.links.clear();
    }

    // ─── The frame ───────────────────────────────────────────────────────

    /// One host frame at host-clock `now_ms`: read the network, step the
    /// world for every tick that is due, and send what changed.
    pub fn update(&mut self, now_ms: u64) {
        self.now = self.now.max(now_ms);
        let now = self.now;
        while let Some(ev) = self.transport.poll() {
            self.on_net_event(ev);
        }
        self.expire_pending(now);
        match self.phase {
            HostPhase::Lobby => {
                if self.cfg.auto_start && self.can_start() && self.humans.iter().all(|h| h.ready) {
                    self.begin_round();
                }
            }
            HostPhase::Running | HostPhase::Results => {
                self.step_world(now);
                self.drop_silent(now);
            }
        }
        if self.phase == HostPhase::Results {
            self.eval_rematch();
            if let Some(t) = self.rematch_at {
                if now >= t {
                    self.rematch_at = None;
                    if self.can_start() {
                        self.begin_round();
                    }
                }
            }
        }
        self.flush();
    }

    fn on_net_event(&mut self, ev: HostNetEvent) {
        match ev {
            HostNetEvent::Joined { peer, path, persona } => {
                log::debug!("[host] peer {peer} linked via {path:?}");
                self.links.insert(peer, Link { path, persona });
            }
            HostNetEvent::Left { peer } => {
                self.links.remove(&peer);
                self.pending.retain(|p| p.peer != peer);
                self.on_left(peer);
            }
            HostNetEvent::Reliable { peer, bytes } => self.on_reliable(peer, &bytes),
            HostNetEvent::Lossy { peer, bytes } => self.on_lossy(peer, &bytes),
        }
    }

    fn expire_pending(&mut self, now: u64) {
        let late: Vec<PeerKey> =
            self.pending.iter().filter(|p| now.saturating_sub(p.since) > PENDING_MS).map(|p| p.peer).collect();
        for peer in late {
            self.pending.retain(|p| p.peer != peer);
            self.refuse(peer, "The host didn't answer.");
        }
    }

    // ─── Messages ────────────────────────────────────────────────────────

    fn send(&mut self, peer: PeerKey, m: &ToClient) {
        let bytes = encode(m);
        if bytes.is_empty() || bytes.len() > MAX_RELIABLE_LEN {
            log::error!("[host] reliable message of {} bytes not sent", bytes.len());
            return;
        }
        self.transport.send_reliable(peer, bytes);
    }

    fn broadcast_bytes(&mut self, bytes: &[u8]) {
        if bytes.is_empty() || bytes.len() > MAX_RELIABLE_LEN {
            log::error!("[host] broadcast of {} bytes not sent", bytes.len());
            return;
        }
        let peers: Vec<PeerKey> = self.humans.iter().map(|h| h.peer).collect();
        for p in peers {
            self.transport.send_reliable(p, bytes.to_vec());
        }
    }

    fn broadcast(&mut self, m: &ToClient) {
        let bytes = encode(m);
        self.broadcast_bytes(&bytes);
    }

    /// Tell a peer why and drop its link (no ban).
    fn refuse(&mut self, peer: PeerKey, why: &str) {
        self.send(peer, &ToClient::Closed(why.into()));
        self.transport.disconnect(peer);
    }

    fn on_reliable(&mut self, peer: PeerKey, bytes: &[u8]) {
        if bytes.len() > MAX_RELIABLE_LEN {
            return;
        }
        let Some(msg) = decode::<ToHost>(bytes) else {
            // Another protocol version's Hello: refuse it with the reason.
            if let Some(proto) = crate::proto::hello_proto(bytes).filter(|p| *p != PROTO_VERSION) {
                if !self.by_peer.contains_key(&peer) && !self.pending.iter().any(|p| p.peer == peer) {
                    self.refuse(peer, &format!("version mismatch (host {PROTO_VERSION}, you {proto})"));
                }
                return;
            }
            log::debug!("[host] undecodable reliable message from {peer}");
            return;
        };
        let now = self.now;
        if let ToHost::Hello { proto, name, colour, hat } = msg {
            if !self.by_peer.contains_key(&peer) && !self.pending.iter().any(|p| p.peer == peer) {
                self.on_hello(peer, proto, &name, colour, hat);
            }
            return;
        }
        // Waiting (pending) and unknown peers get nothing else.
        let Some(&id) = self.by_peer.get(&peer) else { return };
        let Some(hi) = self.humans.iter().position(|h| h.id == id) else { return };
        self.humans[hi].last_heard = now;
        match msg {
            ToHost::Hello { .. } => {}
            ToHost::SetColour(c) => {
                if self.phase == HostPhase::Lobby
                    && c < COLOURS
                    && self.humans[hi].colour != c
                    && !self.humans.iter().any(|h| h.id != id && h.colour == c)
                {
                    self.humans[hi].colour = c;
                    self.take_colour_from_bot(c);
                    self.lobby_dirty = true;
                }
            }
            ToHost::SetReady(r) => {
                if self.phase == HostPhase::Lobby && self.humans[hi].ready != r {
                    self.humans[hi].ready = r;
                    self.lobby_dirty = true;
                }
            }
            ToHost::RematchReady(r) => {
                if self.phase == HostPhase::Results && self.humans[hi].ready != r {
                    self.humans[hi].ready = r;
                    self.lobby_dirty = true;
                    self.rematch_dirty = true;
                }
            }
            ToHost::Leave => {
                self.on_left(peer);
                self.transport.disconnect(peer);
            }
            ToHost::Ping { id, client_t } => {
                self.send(peer, &ToClient::Pong { id, client_t, host_t: now });
            }
            ToHost::Place { run, req, tool, lane, row } => {
                let player = self.humans[hi].player;
                let Some(r) = self.run.as_mut() else { return };
                if run != r.run_no || self.phase != HostPhase::Running {
                    return;
                }
                let Some(player) = player else { return };
                // The world validates role, band, energy, density and safety,
                // and answers a repeated (player, req) with its first result.
                let result = r.world.place(PlaceRequest { player, req, tool, lane, row });
                self.stats.places += 1;
                r.answers.push((peer, ToClient::PlaceResult { run, req, result }));
            }
        }
    }

    fn on_hello(&mut self, peer: PeerKey, proto: u32, name: &str, colour: u8, hat: u8) {
        if proto != PROTO_VERSION {
            self.refuse(peer, &format!("version mismatch (host {PROTO_VERSION}, you {proto})"));
            return;
        }
        let host = peer >= LOCAL_PEER_BASE;
        // The path is the door's word (Joined), never the joiner's.
        let link = self.links.get(&peer).cloned();
        let (path, persona) = match (host, link) {
            (true, _) => (SeatPath::Local, self.host_persona.clone()),
            (false, Some(Link { path: LinkPath::Bluetooth, persona })) => (SeatPath::Bluetooth, persona),
            (false, Some(Link { path: LinkPath::Local, persona })) => (SeatPath::Local, persona),
            (false, Some(Link { persona, .. })) => (SeatPath::Network, persona),
            (false, None) => {
                log::warn!("[host] hello from {peer} with no admitted link refused");
                self.refuse(peer, "not admitted");
                return;
            }
        };
        if self.phase != HostPhase::Lobby {
            self.refuse(peer, "match in progress");
            return;
        }
        if self.humans.len() >= MAX_SEATS {
            self.refuse(peer, "lobby full");
            return;
        }
        let colour = colour.min(COLOURS - 1);
        if path == SeatPath::Bluetooth {
            if self.pending.len() >= MAX_PENDING {
                self.refuse(peer, "Too many people are waiting to join.");
                return;
            }
            self.pending.push(PendingJoiner {
                peer,
                name: sanitize_name(name, "Player"),
                persona,
                since: self.now,
                colour,
                hat,
            });
            self.send(peer, &ToClient::Waiting);
            return;
        }
        self.seat(peer, name, colour, hat, path, persona);
    }

    /// Give a peer a seat (lobby only). Returns whether it was seated.
    fn seat(&mut self, peer: PeerKey, name: &str, colour: u8, hat: u8, path: SeatPath, persona: Option<String>) -> bool {
        if self.phase != HostPhase::Lobby {
            self.refuse(peer, "match in progress");
            return false;
        }
        if path == SeatPath::Bluetooth && self.host_attestation.is_none() {
            // The joiner would verify the host after Accept and fail: refuse
            // now, with the reason, rather than seat someone who cannot stay.
            self.refuse(peer, HOST_NOT_SIGNED_IN);
            return false;
        }
        if self.humans.len() >= MAX_SEATS {
            self.refuse(peer, "lobby full");
            return false;
        }
        if self.humans.len() + self.bots.len() >= MAX_SEATS {
            // People come first: a bot gives up its seat.
            self.bots.pop();
        }
        // The next id below BOT_ID_BASE not in use; ids are not reused at
        // once, so a UI keyed on them does not mistake a newcomer for who left.
        let Some(id) = (0..BOT_ID_BASE)
            .map(|k| (self.next_id + k) % BOT_ID_BASE)
            .find(|id| !self.humans.iter().any(|h| h.id == *id))
        else {
            self.refuse(peer, "lobby full");
            return false;
        };
        self.next_id = (id + 1) % BOT_ID_BASE;
        let colour = if !self.humans.iter().any(|h| h.colour == colour) { colour } else { self.first_free_colour() };
        let host = peer >= LOCAL_PEER_BASE;
        self.humans.push(Human {
            id,
            peer,
            name: sanitize_name(name, &format!("Player {}", id as u32 + 1)),
            colour,
            hat,
            ready: false,
            host,
            path,
            persona,
            last_heard: self.now,
            player: None,
            inq: BTreeMap::new(),
            last_fed: InputFrame::default().pack(),
            applied_seq: 0,
            next_row: 0,
        });
        // A bot holding the colour moves to a free one.
        self.take_colour_from_bot(colour);
        self.by_peer.insert(peer, id);
        self.send(peer, &ToClient::Welcome { proto: PROTO_VERSION, seat: id });
        if path == SeatPath::Bluetooth {
            if let Some(a) = self.host_attestation.clone() {
                self.send(peer, &ToClient::HostAttestation(a));
            }
        }
        self.send(peer, &ToClient::Lobby(self.lobby_view()));
        self.lobby_dirty = true;
        true
    }

    fn on_lossy(&mut self, peer: PeerKey, bytes: &[u8]) {
        let Some(LossyToHost::Inputs { run, seq, inputs }) = decode::<LossyToHost>(bytes) else { return };
        let now = self.now;
        let Some(&id) = self.by_peer.get(&peer) else { return };
        let Some(h) = self.humans.iter_mut().find(|h| h.id == id) else { return };
        h.last_heard = now;
        let run_no = self.run.as_ref().map(|r| r.run_no);
        if self.phase != HostPhase::Running || Some(run) != run_no || inputs.is_empty() || inputs.len() > INPUTS_PER_PACKET {
            return;
        }
        // `seq` numbers the last input; input numbers start at 1.
        let Some(first) = seq.checked_sub(inputs.len() as u32 - 1).filter(|&f| f >= 1) else { return };
        for (i, p) in inputs.iter().enumerate() {
            let n = first + i as u32;
            // Bad command codes are dropped here, never fed.
            if n > h.applied_seq && InputFrame::unpack(*p).is_some() {
                h.inq.entry(n).or_insert(*p);
            }
        }
        while h.inq.len() > MAX_QUEUED {
            let last = *h.inq.keys().next_back().expect("non-empty");
            h.inq.remove(&last);
        }
    }

    fn on_left(&mut self, peer: PeerKey) {
        let Some(id) = self.by_peer.remove(&peer) else { return };
        let Some(i) = self.humans.iter().position(|h| h.id == id) else { return };
        let h = self.humans.remove(i);
        self.lobby_dirty = true;
        self.rematch_dirty = true;
        if let (Some(p), Some(run)) = (h.player, self.run.as_mut()) {
            if self.phase == HostPhase::Running {
                log::info!("[host] {} (player {}) left mid-match", h.name, p.0);
                // Body removed at once, role Disconnected, ranked last.
                run.world.disconnect(p);
                self.latch.reset(p);
                self.broadcast(&ToClient::Left { player: p });
            }
        }
        if self.phase != HostPhase::Lobby && self.humans.is_empty() {
            self.to_lobby();
        }
    }

    fn drop_silent(&mut self, now: u64) {
        if self.phase != HostPhase::Running {
            return;
        }
        let start = self.run.as_ref().map_or(0, |r| r.start_ms);
        if now < start + SILENCE_MS {
            return;
        }
        let silent: Vec<PeerKey> = self
            .humans
            .iter()
            .filter(|h| !h.host && now.saturating_sub(h.last_heard) > SILENCE_MS)
            .map(|h| h.peer)
            .collect();
        for p in silent {
            log::info!("[host] peer {p} silent for {SILENCE_MS} ms: disconnected");
            self.on_left(p);
            self.transport.disconnect(p);
        }
    }

    // ─── Starting a round ────────────────────────────────────────────────

    fn begin_round(&mut self) {
        let round = self.rounds;
        self.rounds += 1;
        let run_no = (self.rounds & 0xFF) as u8;
        let mut players = Vec::new();
        let now = self.now;
        for (i, h) in self.humans.iter_mut().enumerate() {
            let id = PlayerId(i as u8);
            players.push(PlayerSpec { id, name: h.name.clone(), colour: h.colour, bot: None, hat: h.hat });
            h.player = Some(id);
            h.ready = false;
            h.last_heard = now;
            h.reset_run_input();
        }
        let first_bot = players.len();
        let skill = self.cfg.bot_skill;
        for (k, b) in self.bots.iter_mut().enumerate() {
            let id = PlayerId((first_bot + k) as u8);
            players.push(PlayerSpec::new(id, b.name.clone(), b.colour, Some(skill)));
            b.player = Some(id);
        }
        // A multiple of the seat count, so the core's grid rotation
        // `(round + seed) % n` is `round % n`: each rematch moves everyone
        // one slot on (nobody is always at the back).
        let n = players.len().max(1) as u64;
        let seed = mix64(self.cfg.seed ^ mix64(round as u64 + 1)) >> 8;
        let seed = seed - seed % n;
        let config = MatchConfig { seed, mode: Mode::Multi, players, tuning: self.cfg.tuning.clone(), round };
        let start_ms = now + START_LEAD_MS;
        self.latch = CmdLatch::new();
        self.run = Some(Run {
            world: World::new(config.clone()),
            start_ms,
            run_no,
            events: Vec::new(),
            results: None,
            results_pending: false,
            last_snap_tick: 0,
            answers: Vec::new(),
        });
        self.phase = HostPhase::Running;
        self.rematch_at = None;
        let starts: Vec<(PeerKey, PlayerId)> =
            self.humans.iter().map(|h| (h.peer, h.player.expect("just seated"))).collect();
        for (peer, you) in starts {
            self.send(peer, &ToClient::Start { config: config.clone(), start_host_ms: start_ms, you, run: run_no });
        }
        self.lobby_dirty = true;
    }

    // ─── The world ──────────────────────────────────────────────────────

    fn step_world(&mut self, now: u64) {
        let Some(run) = self.run.as_mut() else { return };
        let mut steps = 0;
        while steps < MAX_CATCH_UP && run.world.phase() != Phase::Over {
            let t = run.world.tick();
            if run.start_ms + t * 1000 / TICK_HZ as u64 > now {
                break;
            }
            let want = t as u32 + 1;
            let mut frames = Vec::with_capacity(self.humans.len());
            for h in &mut self.humans {
                let Some(p) = h.player else { continue };
                let (fed, starved) = h.feed(want);
                if starved && run.world.phase() == Phase::Running {
                    self.stats.starved_ticks += 1;
                }
                if let Some(f) = InputFrame::unpack(fed) {
                    frames.push((p, f));
                }
            }
            let cmds = self.latch.feed_all(&frames);
            run.world.step(&cmds);
            self.stats.ticks += 1;
            steps += 1;
            run.events.extend_from_slice(run.world.events());
            if run.world.phase() == Phase::Over && run.results.is_none() {
                run.results = Some(results_from_world(&run.world));
                run.results_pending = true;
                self.phase = HostPhase::Results;
                self.lobby_dirty = true;
                self.rematch_dirty = true;
                for h in &mut self.humans {
                    h.ready = false;
                }
            }
        }
    }

    // ─── Rematch ────────────────────────────────────────────────────────

    fn eval_rematch(&mut self) {
        if !std::mem::take(&mut self.rematch_dirty) {
            return;
        }
        let all = self.phase == HostPhase::Results && self.can_start() && self.humans.iter().all(|h| h.ready);
        match (all, self.rematch_at) {
            (true, None) => {
                let at = self.now + REMATCH_MS;
                self.rematch_at = Some(at);
                self.broadcast(&ToClient::Rematch(RematchMsg::Countdown { ends_at_host_ms: at }));
            }
            (false, Some(_)) => {
                self.rematch_at = None;
                self.broadcast(&ToClient::Rematch(RematchMsg::Cancelled));
            }
            _ => {}
        }
    }

    // ─── Output ─────────────────────────────────────────────────────────

    fn flush(&mut self) {
        if std::mem::take(&mut self.lobby_dirty) {
            let v = self.lobby_view();
            self.broadcast(&ToClient::Lobby(v));
        }
        let mut rows_out: Vec<(PeerKey, Vec<u8>)> = Vec::new();
        let mut answers: Vec<(PeerKey, Vec<u8>)> = Vec::new();
        let mut shared: Vec<Vec<u8>> = Vec::new();
        if let Some(run) = self.run.as_mut() {
            let run_no = run.run_no;
            // Rows first: events (coins taken, placements) refer to them.
            let course = run.world.course();
            if let Some(last) = course.last_index() {
                let first = course.first_index().unwrap_or(0);
                for h in self.humans.iter_mut().filter(|h| h.player.is_some()) {
                    let from = h.next_row.max(first);
                    if from > last {
                        continue;
                    }
                    let rows: Vec<flock_core::Row> = course.rows().filter(|r| r.row_index >= from).copied().collect();
                    self.stats.rows_sent += rows.len() as u64;
                    for m in chunk(&rows, |rows| ToClient::Rows { run: run_no, rows }) {
                        rows_out.push((h.peer, m));
                    }
                    h.next_row = last + 1;
                }
            }
            if !run.events.is_empty() {
                let evs = std::mem::take(&mut run.events);
                shared.extend(chunk(&evs, |events| ToClient::Events { run: run_no, events }));
            }
            for (peer, m) in std::mem::take(&mut run.answers) {
                answers.push((peer, encode(&m)));
            }
            if std::mem::take(&mut run.results_pending) {
                if let Some(r) = run.results.clone() {
                    shared.push(encode(&ToClient::Results(r)));
                }
            }
        }
        // Per-peer rows, then the shared events and results, in that order on
        // every peer's reliable stream.
        for (peer, bytes) in rows_out {
            self.transport.send_reliable(peer, bytes);
        }
        for bytes in &shared {
            self.broadcast_bytes(bytes);
        }
        for (peer, bytes) in answers {
            if bytes.len() <= MAX_RELIABLE_LEN {
                self.transport.send_reliable(peer, bytes);
            }
        }
        self.send_snapshots();
        let open = self.phase == HostPhase::Lobby && self.humans.len() < MAX_SEATS;
        if self.open != Some(open) {
            self.open = Some(open);
            self.transport.set_open(open);
        }
    }

    fn send_snapshots(&mut self) {
        let Some(run) = self.run.as_mut() else { return };
        let w = &run.world;
        // Every other tick, and always the final state once the match is over.
        let due = w.tick() / SNAPSHOT_EVERY_TICKS != run.last_snap_tick / SNAPSHOT_EVERY_TICKS;
        let last = w.phase() == Phase::Over && w.tick() != run.last_snap_tick;
        if w.tick() == 0 || !(due || last) {
            return;
        }
        run.last_snap_tick = w.tick();
        let n = w.living_count();
        let mean_z = if n > 0 { w.runners().map(|r| r.pos.z).sum::<f64>() / n as f64 } else { w.anchor_z() };
        let players: Vec<PlayerNet> = w
            .players()
            .iter()
            .map(|p| PlayerNet { id: p.id(), role: p.role, placements: p.placements, survived_ticks: p.survived_ticks })
            .collect();
        let bodies: Vec<RunnerNet> = w.runners().map(|r| RunnerNet::from_runner(r, w.tick())).collect();
        let first_row = w.course().first_index().unwrap_or(0);
        let mut out: Vec<(PeerKey, Vec<u8>)> = Vec::new();
        for h in &self.humans {
            let Some(p) = h.player else { continue };
            let own = w.runner(p).cloned();
            let from = own.as_ref().map_or(w.anchor_z(), |r| r.pos.z);
            let mut snap = Snapshot {
                run: run.run_no,
                tick: w.tick() as u32,
                ack_seq: h.applied_seq,
                phase: w.phase(),
                end: w.end_reason(),
                v_course: w.v_course(),
                anchor: w.anchor_z(),
                mean_z,
                first_row,
                own,
                bodies: bodies.iter().filter(|b| b.id != p).copied().collect(),
                players: players.clone(),
            };
            self.stats.bodies_dropped += snap.budget(from) as u64;
            let bytes = snap.encode();
            if bytes.len() > MAX_LOSSY_LEN {
                log::error!("[host] snapshot of {} bytes over budget, not sent", bytes.len());
                continue;
            }
            self.stats.snapshot_bytes_max = self.stats.snapshot_bytes_max.max(bytes.len());
            self.stats.snapshots_sent += 1;
            out.push((h.peer, bytes));
        }
        for (peer, bytes) in out {
            self.transport.send_lossy(peer, bytes);
        }
    }
}

/// SplitMix64 finaliser.
pub fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}
