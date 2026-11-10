//! [`ClientSession`]: one player's side of an online room (SPEC §10): join,
//! lobby, and the match with own-runner prediction, reconciliation and
//! interpolation of everyone else. The host's own player is one of these on
//! a zero-latency loopback ([`crate::host_with_local_player`]).
//!
//! Prediction: the client runs ahead of the host by half the RTT plus a
//! 2-tick jitter buffer. Each tick it records the current `InputFrame`
//! (input number `n` drives host tick `n - 1`), steps its own runner with
//! `flock_core::runner::runner_tick` (the same code `World::step` runs) and
//! sends the five newest frames. On each snapshot it resets to the host's
//! body, replays the unacked frames and eases the position difference out
//! over 150-250 ms; only an error over [`SNAP_DISTANCE`] snaps. Contacts,
//! damage, coins and protection are host-only (bumps show as eased
//! corrections). Everyone else is drawn ~100 ms behind the host timeline
//! with Stampede's adaptive delay.

use std::collections::VecDeque;

use flock_core::runner::runner_tick;
use flock_core::{
    Command, Course, EndReason, Event, InputFrame, MatchConfig, Phase, PlaceResult, Player, PlayerId, PredictCtx,
    Role, Runner, Tick, Tool, Tuning, World, DT, MAX_PLAYERS, TICK_HZ,
};

use crate::clock::ClockSync;
use crate::proto::{
    self, decode, encode, LobbyPhase, LobbyView, LossyToClient, LossyToHost, PlayerNet, RematchMsg, Snapshot, ToClient,
    ToHost, COLOURS, INPUTS_PER_PACKET, PROTO_VERSION,
};
use crate::shadow;
use crate::transport::{ClientNetEvent, ClientTransport, HostIdentity};
use crate::view::{FlockView, ResultsView, RunDriver};

/// Others are drawn at least this far behind the host timeline (ms).
pub const INTERP_DELAY_MS: f64 = 100.0;
/// ... and at most this far.
pub const INTERP_DELAY_MAX_MS: f64 = 350.0;
const SNAPSHOT_GAP_MS: f64 = proto::SNAPSHOT_EVERY_TICKS as f64 * 1000.0 / TICK_HZ as f64;
const INTERP_MARGIN_MS: f64 = 8.0;
const LATENESS_WINDOW_MS: u64 = 4000;
const LATENESS_QUANTILE: f64 = 0.97;
const DELAY_FALL_PER_S: f64 = 25.0;
const RENDER_RATE_MIN: f64 = 0.8;
const RENDER_RATE_MAX: f64 = 1.08;
const RENDER_STEER_MS: f64 = 250.0;
const RENDER_RATE_CHANGE_PER_S: f64 = 3.0;
const RENDER_RATE_STARVED: f64 = 0.5;
const RENDER_HEADROOM_MS: f64 = 50.0;
const RENDER_SNAP_MS: f64 = 500.0;
const RENDER_MAX_FRAME_MS: f64 = 250.0;
/// A correction up to 1 m eases out over this long (ms).
pub const CORRECTION_MS: f32 = 150.0;
pub const CORRECTION_MAX_MS: f32 = 250.0;
/// A prediction error beyond this (metres) is shown as a jump, not eased.
pub const SNAP_DISTANCE: f32 = 10.0;
/// Inputs are produced this many ticks ahead of the host's need.
pub const JITTER_TICKS: u64 = 2;
const INPUT_RING: usize = 480;
const SNAP_BUFFER: usize = 30;
/// Others are extrapolated at most this many ticks past the newest snapshot.
const EXTRAPOLATE_TICKS: f64 = 6.0;
/// A jump between two snapshots longer than this is not interpolated.
const TELEPORT_M: f64 = 20.0;
const MAX_FRAME_S: f32 = 0.1;
const MAX_CATCH_UP_TICKS: u32 = 120;
/// How long a "<name> left" notice shows (local ms).
pub const NOTICE_MS: u64 = 4000;
const MAX_NOTICES: usize = 3;
/// A Bluetooth joiner that cannot verify the host this long after being
/// seated gives up.
pub const HOST_CHECK_MS: u64 = 15_000;
/// A Bluetooth joiner waits at most this long (from connecting) for the
/// host's Accept and then the host check: the host's own answer window
/// ([`crate::host::PENDING_MS`]) plus [`HOST_CHECK_MS`]. Fixed at connect;
/// nothing the host sends moves it.
pub const HOST_ACCEPT_MS: u64 = crate::host::PENDING_MS + HOST_CHECK_MS;
/// Rows a client holds at most (a host that never releases cannot grow it).
const MAX_ROWS: usize = 4096;
/// Placed objects a client holds at most (a host with a valid persona still
/// cannot grow the course without bound; a real match places far fewer).
const MAX_PLACED: usize = 1024;
/// A `Placed` event may name a row this far past the newest row held.
const PLACE_AHEAD_ROWS: u32 = 256;
/// Events queued for the app between frames, at most.
const MAX_QUEUED_EVENTS: usize = 2048;
/// Place answers queued for the app between frames, at most.
const MAX_PLACE_RESULTS: usize = 256;
/// Text of a failed host check.
pub const HOST_UNVERIFIED: &str = "Couldn't confirm the host's My Signet approval.";

/// Where this player is in the flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientPhase {
    /// Joining; the text is the current stage.
    Connecting(String),
    /// Bluetooth: connected, waiting for the host to accept.
    Waiting,
    Lobby,
    /// From `Start`: the countdown, then the match.
    Running,
    Results,
    /// The join failed (friendly text).
    Failed(String),
    /// The host refused or removed us (full, version, declined, kicked...).
    Closed(String),
    /// The host left (screens say "Host left").
    HostLeft,
    Disconnected(String),
}

impl ClientPhase {
    /// The link is gone for good.
    pub fn is_dead(&self) -> bool {
        matches!(
            self,
            ClientPhase::Failed(_) | ClientPhase::Closed(_) | ClientPhase::HostLeft | ClientPhase::Disconnected(_)
        )
    }
}

/// Counters for tests and a debug overlay.
#[derive(Clone, Debug, Default)]
pub struct ClientStats {
    pub snapshots: u64,
    pub inputs_sent: u64,
    /// Size of the latest reconciliation error (before easing), metres.
    pub last_correction_m: f32,
    pub max_correction_m: f32,
    /// Corrections over [`SNAP_DISTANCE`] (shown as a jump, not eased).
    pub hard_snaps: u64,
    pub interp_frames: u64,
    pub interp_starved_frames: u64,
    pub interp_delay_ms: f32,
    /// Shadow worlds that could not be built (flock-core layout drift).
    pub shadow_errors: u64,
}

/// How long a visual correction of `metres` eases out.
pub fn correction_ms(metres: f32) -> f32 {
    (CORRECTION_MS + (metres - 1.0).max(0.0) * 20.0).min(CORRECTION_MAX_MS)
}

/// How far in the past to draw the others (host ms), given how late recent
/// snapshots were.
pub fn interp_target_ms(lateness_ms: &[f64]) -> f64 {
    if lateness_ms.is_empty() {
        return INTERP_DELAY_MS;
    }
    let mut v = lateness_ms.to_vec();
    v.sort_by(f64::total_cmp);
    let q = v[((v.len() - 1) as f64 * LATENESS_QUANTILE).round() as usize];
    (q + SNAPSHOT_GAP_MS + INTERP_MARGIN_MS).clamp(INTERP_DELAY_MS, INTERP_DELAY_MAX_MS)
}

/// The share of a correction still shown `age_ms` into an ease of `duration_ms`.
pub fn correction_left(age_ms: f32, duration_ms: f32) -> f32 {
    let t = (age_ms / duration_ms.max(1.0)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

fn input_tick_ms(t: i64) -> i64 {
    t * 1000 / TICK_HZ as i64
}

/// The pack context the host will integrate a tick with, extrapolated from
/// the newest snapshot (exact for the snapshot's own tick).
#[derive(Clone, Copy, Debug)]
struct CtxState {
    /// The tick the next `advance` predicts.
    next_tick: Tick,
    /// `World::anchor_z()` after tick `next_tick - 1`.
    anchor: f64,
    /// Mean z of the living runners after tick `next_tick - 1`.
    mean: f64,
}

impl CtxState {
    /// The ctx for `next_tick` (`None` during the countdown), then move on.
    fn advance(&mut self, t: &Tuning, go: Tick) -> Option<PredictCtx> {
        let tick = self.next_tick;
        self.next_tick += 1;
        if tick < go {
            return None;
        }
        let v = t.multi_speed((tick - go) as f32 * DT);
        let anchor = self.anchor + (v * DT) as f64 + (t.pack_anchor_follow * DT) as f64 * (self.mean - self.anchor);
        self.anchor = anchor;
        self.mean += (v * DT) as f64;
        Some(PredictCtx { tick, v_course: v, anchor_z: anchor, pack: true })
    }
}

/// Motion events `runner_tick` raises: the own player's come from the
/// prediction, so the host's copies are dropped.
fn is_own_motion_event(e: &Event, you: PlayerId) -> bool {
    match e {
        Event::LaneChange { player, .. }
        | Event::FenceBump { player }
        | Event::WindUp { player, .. }
        | Event::Jumped { player }
        | Event::Collapsed { player }
        | Event::StoodUp { player }
        | Event::MudEnter { player }
        | Event::MudStuck { player }
        | Event::Landed { player } => *player == you,
        _ => false,
    }
}

/// Every player id an event names is in the match (ids come from the host:
/// never hand the app one it cannot look up).
fn event_ok(e: &Event, n: usize) -> bool {
    let ok = |p: &PlayerId| (p.0 as usize) < n;
    match e {
        Event::Go | Event::Materialised { .. } | Event::MatchOver { .. } => true,
        Event::LaneChange { player, to } => ok(player) && *to < flock_core::LANES,
        Event::FenceBump { player }
        | Event::WindUp { player, .. }
        | Event::Jumped { player }
        | Event::Collapsed { player }
        | Event::Landed { player }
        | Event::StoodUp { player }
        | Event::Tumble { player }
        | Event::Crash { player, .. }
        | Event::MudEnter { player }
        | Event::ProtectionStart { player, .. }
        | Event::ProtectionEnd { player }
        | Event::Swagger { player, .. }
        | Event::MudStuck { player }
        | Event::ShieldBroken { player }
        | Event::LongProtection { player, .. }
        | Event::Eliminated { player }
        | Event::BecameGhost { player }
        | Event::Disconnected { player } => ok(player),
        Event::Hop { player, over } => ok(player) && ok(over),
        Event::Disrupted { player, by } => ok(player) && ok(by),
        Event::Bump { a, b, .. } => ok(a) && ok(b),
        Event::CoinTaken { by, .. } | Event::PowerTaken { by, .. } => ok(by),
        Event::Placed { by, lane, .. } => ok(by) && *lane < flock_core::LANES,
        // Bounded by the course itself: an unknown row is ignored.
        Event::BarrelReleased { .. } => true,
    }
}

/// The host-only, unbounded parts of a body are never trusted from the wire.
fn sanitize_own(r: &mut Runner) {
    r.touched_prev.clear();
    r.incidents.clear();
    if !(r.pos.x.is_finite() && r.pos.y.is_finite() && r.pos.z.is_finite()) {
        r.pos.x = 0.0;
        r.pos.y = 0.0;
        r.pos.z = 0.0;
    }
}

struct Run {
    run_no: u8,
    you: PlayerId,
    start_ms: u64,
    go_tick: Tick,
    cfg: MatchConfig,
    tuning: Tuning,
    /// Bodies, roles and scores as last known (the shadow's players).
    players: Vec<Player>,
    /// Placeholder course for the shadow image.
    placeholder: Course,
    pred: Runner,
    predicting: bool,
    over: bool,
    end: Option<EndReason>,
    /// Next input number to produce (1-based).
    next_seq: u32,
    /// `(input number, frame, command the latch fired)`.
    ring: VecDeque<(u32, [u8; 2], Option<Command>)>,
    /// The latch mirror: seq of the last frame seen.
    latch_seq: Option<u8>,
    ctx: CtxState,
    snaps: VecDeque<Snapshot>,
    newest_tick: u32,
    released: u32,
    corr_from: [f32; 3],
    corr_age: f32,
    corr_ms: f32,
    last_now: u64,
    events: Vec<Event>,
    place_results: Vec<(u32, PlaceResult)>,
    render_tick: f64,
    lateness: VecDeque<(u64, f64)>,
    delay_ms: f64,
    render_ms: Option<f64>,
    render_rate: f64,
}

/// One player's session.
pub struct ClientSession {
    transport: Box<dyn ClientTransport>,
    name: String,
    colour: u8,
    hat: u8,
    phase: ClientPhase,
    host: Option<HostIdentity>,
    /// The host's verified persona (`npub1...`), once checked.
    host_persona: Option<String>,
    /// Bluetooth: `Some(local ms)` from the moment the transport connected
    /// while the host check was pending. Set once, never reset.
    host_check_since: Option<u64>,
    /// Bluetooth: local ms of the first `Welcome` (the seat); set once.
    seated_at: Option<u64>,
    /// `Connected` has been seen: nothing the link says counts before it.
    connected: bool,
    my_seat: Option<u8>,
    lobby: Option<LobbyView>,
    results: Option<ResultsView>,
    rematch_ends: Option<u64>,
    clock: ClockSync,
    run: Option<Run>,
    view: Option<FlockView>,
    stats: ClientStats,
    now: u64,
    next_req: u32,
    notices: VecDeque<(String, u64)>,
}

impl ClientSession {
    /// The cosmetic hat sent in the Hello (255 = default for the colour).
    /// Call before the first `update` (the Hello goes out on connect); the
    /// hat is fixed for the session.
    pub fn set_hat(&mut self, hat: u8) {
        self.hat = hat;
    }
    /// Builder form of [`ClientSession::set_hat`].
    pub fn with_hat(mut self, hat: u8) -> ClientSession {
        self.hat = hat;
        self
    }

    /// `colour` is the preferred lobby colour (0..6).
    pub fn new(transport: Box<dyn ClientTransport>, name: &str, colour: u8) -> ClientSession {
        ClientSession {
            transport,
            name: proto::sanitize_name(name, "Player"),
            colour: colour.min(COLOURS - 1),
            hat: flock_core::PlayerSpec::DEFAULT_HAT,
            phase: ClientPhase::Connecting("Connecting".into()),
            host: None,
            host_persona: None,
            host_check_since: None,
            seated_at: None,
            connected: false,
            my_seat: None,
            lobby: None,
            results: None,
            rematch_ends: None,
            clock: ClockSync::default(),
            run: None,
            view: None,
            stats: ClientStats::default(),
            now: 0,
            next_req: 0,
            notices: VecDeque::new(),
        }
    }

    // ─── Queries ─────────────────────────────────────────────────────────

    pub fn phase(&self) -> &ClientPhase {
        &self.phase
    }
    /// The host's runtime key as the link proved it (QUIC).
    pub fn host_identity(&self) -> Option<&HostIdentity> {
        self.host.as_ref()
    }
    /// The host's verified My Signet persona (`npub1...`): from the link for
    /// online/Nearby joins, from the post-Accept check for Bluetooth.
    pub fn host_persona(&self) -> Option<&str> {
        self.host_persona.as_deref()
    }
    /// The host check (Bluetooth) is still pending: nothing the host sent is
    /// trusted yet. The transport is the single source of truth for this.
    fn checking(&self) -> bool {
        self.transport.host_check_pending()
    }
    /// The lobby, once the host is trusted (never during the host check).
    pub fn lobby(&self) -> Option<&LobbyView> {
        if self.checking() {
            return None;
        }
        self.lobby.as_ref()
    }
    pub fn my_seat(&self) -> Option<u8> {
        self.my_seat
    }
    pub fn my_lobby_seat(&self) -> Option<&proto::SeatView> {
        let id = self.my_seat?;
        self.lobby()?.seat(id)
    }
    pub fn results(&self) -> Option<&ResultsView> {
        if self.checking() {
            return None;
        }
        self.results.as_ref()
    }
    pub fn view(&self) -> Option<&FlockView> {
        if self.checking() {
            return None;
        }
        self.view.as_ref()
    }
    pub fn stats(&self) -> &ClientStats {
        &self.stats
    }
    pub fn rtt_ms(&self) -> Option<u32> {
        self.transport.rtt_ms().or_else(|| self.clock.rtt_ms().map(|r| r as u32))
    }
    pub fn clock_sync(&self) -> &ClockSync {
        &self.clock
    }
    /// Estimated host clock.
    pub fn host_now(&self, local_now: u64) -> Option<u64> {
        self.clock.host_now(local_now)
    }
    /// Milliseconds left of the rematch 3-2-1, if it runs.
    pub fn rematch_ms_left(&self, local_now: u64) -> Option<u64> {
        let ends = self.rematch_ends?;
        let host = self.clock.host_now(local_now)?;
        Some(ends.saturating_sub(host))
    }
    /// "<name> left" for each player who left in the last [`NOTICE_MS`].
    pub fn notice(&self) -> Option<String> {
        let shown: Vec<&str> = self
            .notices
            .iter()
            .filter(|(_, at)| self.now < at.saturating_add(NOTICE_MS))
            .map(|(t, _)| t.as_str())
            .collect();
        (!shown.is_empty()).then(|| shown.join(" \u{b7} "))
    }
    /// Your own predicted body (tests, debug), before the visual correction.
    pub fn predicted(&self) -> Option<&Runner> {
        self.run.as_ref().filter(|r| r.predicting).map(|r| &r.pred)
    }
    /// The latest input number the host has applied.
    pub fn last_ack(&self) -> Option<u32> {
        self.run.as_ref()?.snaps.back().map(|s| s.ack_seq)
    }

    // ─── Commands ────────────────────────────────────────────────────────

    /// Lobby: pick a colour (the host refuses one another person holds).
    pub fn set_colour(&mut self, c: u8) {
        if c < COLOURS {
            self.colour = c;
            self.send(&ToHost::SetColour(c));
        }
    }
    pub fn set_ready(&mut self, ready: bool) {
        self.send(&ToHost::SetReady(ready));
    }
    /// Results screen: ready for a rematch.
    pub fn set_rematch_ready(&mut self, ready: bool) {
        self.send(&ToHost::RematchReady(ready));
    }
    /// Leave the room.
    pub fn leave(&mut self) {
        self.send(&ToHost::Leave);
        self.transport.close();
        if !self.phase.is_dead() {
            self.phase = ClientPhase::Closed("left".into());
        }
    }
    /// Ask to place a ghost tool; returns the request id (answered in
    /// `view().place_results`). Outside a running match nothing is sent and
    /// the answer is `Err(NotGhost)`.
    pub fn place(&mut self, tool: Tool, lane: u8, row: u32) -> u32 {
        self.next_req = self.next_req.wrapping_add(1);
        let req = self.next_req;
        let run_no = match (&self.phase, self.run.as_ref()) {
            (ClientPhase::Running, Some(r)) if !r.over => Some(r.run_no),
            _ => None,
        };
        match run_no {
            Some(run) => self.send(&ToHost::Place { run, req, tool, lane, row }),
            None => {
                if let Some(r) = self.run.as_mut() {
                    r.place_results.push((req, Err(flock_core::Reject::NotGhost)));
                }
            }
        }
        req
    }

    /// Tests: send any message as this client.
    #[cfg(test)]
    pub(crate) fn send_raw(&mut self, m: &ToHost) {
        self.send(m);
    }

    /// Tests: `(queued events, queued place results)` waiting for the app.
    #[cfg(test)]
    pub(crate) fn queued_for_test(&self) -> (usize, usize) {
        self.run.as_ref().map_or((0, 0), |r| (r.events.len(), r.place_results.len()))
    }

    /// Tests: the current round number on the wire.
    #[cfg(test)]
    pub(crate) fn run_no(&self) -> Option<u8> {
        self.run.as_ref().map(|r| r.run_no)
    }

    fn send(&mut self, m: &ToHost) {
        let b = encode(m);
        if !b.is_empty() && b.len() <= proto::MAX_RELIABLE_LEN {
            self.transport.send_reliable(b);
        }
    }

    // ─── The frame ───────────────────────────────────────────────────────

    /// One frame. `input` is the player's current frame (ignored outside a match).
    pub fn update(&mut self, now_ms: u64, input: InputFrame) {
        self.now = now_ms;
        while let Some(ev) = self.transport.poll() {
            self.on_net(ev, now_ms);
        }
        if self.host_check_since.is_some() && !self.phase.is_dead() {
            if !self.checking() {
                self.host_check_since = None; // verified
            } else if self.host_check_expired(now_ms) {
                self.fail_host_check();
            }
        }
        if matches!(self.phase, ClientPhase::Lobby | ClientPhase::Running | ClientPhase::Results) {
            for p in self.clock.tick(now_ms) {
                self.send(&p);
            }
        }
        if matches!(self.phase, ClientPhase::Running | ClientPhase::Results) {
            self.run_frame(now_ms, input);
        }
        while self.notices.front().is_some_and(|(_, at)| now_ms >= at.saturating_add(NOTICE_MS)) {
            self.notices.pop_front();
        }
    }

    /// Fail closed: the host check has run out of time. The window opens at
    /// connect ([`HOST_ACCEPT_MS`], to give the host time to Accept) and
    /// closes [`HOST_CHECK_MS`] after the seat if that is sooner. Both
    /// instants are set once; no message moves them.
    fn host_check_expired(&self, now: u64) -> bool {
        let Some(since) = self.host_check_since else { return false };
        let mut deadline = since.saturating_add(HOST_ACCEPT_MS);
        if let Some(seated) = self.seated_at {
            deadline = deadline.min(seated.saturating_add(HOST_CHECK_MS));
        }
        now > deadline
    }

    fn fail_host_check(&mut self) {
        self.host_check_since = None;
        self.send(&ToHost::Leave);
        self.transport.close();
        self.phase = ClientPhase::Failed(HOST_UNVERIFIED.into());
    }

    fn on_net(&mut self, ev: ClientNetEvent, now: u64) {
        match ev {
            ClientNetEvent::Progress(s) => {
                if matches!(self.phase, ClientPhase::Connecting(_)) {
                    self.phase = ClientPhase::Connecting(s);
                }
            }
            ClientNetEvent::Connected { host } => {
                if self.connected {
                    return; // one connection per session
                }
                self.connected = true;
                // The check is pending from the moment the link is up (not
                // from Welcome): the timeout starts here.
                if self.checking() {
                    self.host_check_since = Some(now);
                }
                self.host_persona = host.as_ref().and_then(|h| h.persona.clone());
                self.host = host;
                self.phase = ClientPhase::Connecting("Joining".into());
                let hello = ToHost::Hello { proto: PROTO_VERSION, name: self.name.clone(), colour: self.colour, hat: self.hat };
                self.send(&hello);
            }
            ClientNetEvent::Failed(s) => {
                if !self.phase.is_dead() {
                    self.phase = ClientPhase::Failed(s)
                }
            }
            ClientNetEvent::Reliable(b) => {
                if !self.connected || b.len() > proto::MAX_RELIABLE_LEN {
                    return;
                }
                if let Some(m) = decode::<ToClient>(&b) {
                    self.on_reliable(m, now);
                }
            }
            ClientNetEvent::Lossy(b) => {
                if !self.connected || self.checking() {
                    return; // snapshots of an unchecked host are dropped
                }
                if let Some(LossyToClient::Snapshot(s)) = decode::<LossyToClient>(&b) {
                    self.on_snapshot(s, now);
                }
            }
            ClientNetEvent::Disconnected { host_left, reason } => {
                if self.phase.is_dead() {
                    return; // keep the more specific reason
                }
                self.phase = if host_left { ClientPhase::HostLeft } else { ClientPhase::Disconnected(reason) };
                self.end_run_on_loss();
            }
        }
    }

    /// The link is gone: an unfinished match is over for this player.
    fn end_run_on_loss(&mut self) {
        if let Some(r) = self.run.as_mut() {
            r.over = true;
            r.predicting = false;
            if r.end.is_none() {
                r.end = Some(EndReason::Aborted);
            }
        }
        // No more frames are built for a dead link: mark the view over now.
        if let Some(v) = self.view.as_mut() {
            v.over = true;
            v.phase = Phase::Over;
        }
    }

    fn on_reliable(&mut self, m: ToClient, now: u64) {
        // A closed client stays closed.
        if self.phase.is_dead() {
            return;
        }
        // Bluetooth: from the moment the link is up until the host's
        // attestation verifies, only Welcome (the seat), Waiting,
        // HostAttestation, Pong and Closed are processed. Everything else the
        // host says is dropped.
        let checking = self.checking();
        if checking
            && !matches!(
                m,
                ToClient::Welcome { .. }
                    | ToClient::Waiting
                    | ToClient::HostAttestation(_)
                    | ToClient::Pong { .. }
                    | ToClient::Closed(_)
            )
        {
            return;
        }
        match m {
            ToClient::Welcome { proto, seat } => {
                if self.my_seat.is_some() {
                    return; // seated once; a second Welcome changes nothing
                }
                if proto != PROTO_VERSION {
                    self.phase = ClientPhase::Closed("version mismatch".into());
                    self.transport.close();
                    return;
                }
                self.my_seat = Some(seat);
                if checking {
                    // Bluetooth: the host must prove its persona before the
                    // lobby is shown (`HostAttestation` follows).
                    self.seated_at.get_or_insert(now);
                    self.phase = ClientPhase::Connecting("Checking the host".into());
                } else if matches!(self.phase, ClientPhase::Connecting(_) | ClientPhase::Waiting) {
                    self.phase = ClientPhase::Lobby;
                }
            }
            ToClient::Waiting => {
                if self.my_seat.is_none() && matches!(self.phase, ClientPhase::Connecting(_)) {
                    self.phase = ClientPhase::Waiting;
                }
            }
            ToClient::HostAttestation(json) => {
                if !checking {
                    return;
                }
                match self.transport.verify_host(&json) {
                    Ok(persona) => {
                        self.host_persona = Some(persona);
                        self.host_check_since = None;
                        self.seated_at = None;
                        if self.my_seat.is_some() && matches!(self.phase, ClientPhase::Connecting(_)) {
                            self.phase = ClientPhase::Lobby;
                        }
                    }
                    Err(e) => {
                        log::warn!("[client] host attestation refused: {e}");
                        self.fail_host_check();
                    }
                }
            }
            ToClient::Lobby(v) => {
                if v.seats.len() > proto::MAX_SEATS * 2 {
                    return;
                }
                let back_to_lobby = v.phase == LobbyPhase::Lobby;
                self.lobby = Some(v);
                if back_to_lobby && matches!(self.phase, ClientPhase::Results | ClientPhase::Running) {
                    self.phase = ClientPhase::Lobby;
                    self.run = None;
                    self.view = None;
                    self.results = None;
                    self.rematch_ends = None;
                }
            }
            ToClient::Start { config, start_host_ms, you, run } => {
                // A match is joined only from a seat (the host check, if any,
                // already passed: it is dropped above while pending).
                if self.my_seat.is_none() {
                    return;
                }
                self.on_start(config, start_host_ms, you, run);
            }
            ToClient::Rows { run, rows } => {
                let Some(view) = self.view.as_mut() else { return };
                if self.run.as_ref().is_none_or(|r| r.run_no != run) {
                    return;
                }
                let course = view.world.debug_course_mut();
                for row in rows {
                    course.insert_row(row.row_index, row);
                }
                while course.len() > MAX_ROWS {
                    let Some(first) = course.first_index() else { break };
                    course.release_before(first + 1);
                }
            }
            ToClient::Events { run, events } => {
                let (Some(r), Some(view)) = (self.run.as_mut(), self.view.as_mut()) else { return };
                if r.run_no != run {
                    return;
                }
                let n = r.players.len();
                for e in events {
                    if !event_ok(&e, n) {
                        continue;
                    }
                    let course = view.world.debug_course_mut();
                    match &e {
                        Event::CoinTaken { obj, .. } => {
                            course.remove_coin(*obj);
                        }
                        Event::PowerTaken { obj, .. } => {
                            course.take_power(*obj);
                        }
                        // The host starts a row's barrels rolling (the rows
                        // streamed earlier carry no release tick).
                        Event::BarrelReleased { row, tick } => {
                            course.release_barrel(*row, *tick);
                        }
                        Event::Placed { by, obj, tool, lane, row, solid_at } => {
                            // Bounded: an architect id, a row inside the window
                            // of rows held (or just ahead), and a capped count.
                            let low = course.first_index().unwrap_or(0).max(r.released);
                            let high = course.last_index().unwrap_or(r.released).saturating_add(PLACE_AHEAD_ROWS);
                            let fits = obj.is_architect()
                                && (low..=high).contains(row)
                                && course.placed().len() < MAX_PLACED;
                            if fits {
                                course.add_placement(*by, *obj, *tool, *lane, *row, *solid_at);
                            } else {
                                log::warn!("[client] implausible placement ignored");
                                continue;
                            }
                        }
                        Event::MatchOver { reason } => r.end = Some(*reason),
                        // Roles change by reliable event at once (snapshots
                        // confirm them, and carry the energy).
                        Event::BecameGhost { player } => {
                            if let Some(p) = r.players.iter_mut().find(|p| p.id() == *player) {
                                if p.role.is_runner() {
                                    p.role = Role::Ghost { energy: r.tuning.ghost_energy_start, placed: 0 };
                                }
                            }
                        }
                        Event::Disconnected { player } => {
                            if let Some(p) = r.players.iter_mut().find(|p| p.id() == *player) {
                                p.role = Role::Disconnected;
                                p.disconnected = true;
                            }
                        }
                        _ => {}
                    }
                    if !is_own_motion_event(&e, r.you) && r.events.len() < MAX_QUEUED_EVENTS {
                        r.events.push(e);
                    }
                }
            }
            ToClient::PlaceResult { run, req, result } => {
                if let Some(r) = self.run.as_mut().filter(|r| r.run_no == run) {
                    if r.place_results.len() < MAX_PLACE_RESULTS {
                        r.place_results.push((req, result));
                    }
                }
            }
            ToClient::Results(res) => {
                // Results end a match this client is in.
                if self.run.is_none() {
                    return;
                }
                let n = self.run.as_ref().map_or(MAX_PLAYERS, |r| r.players.len());
                let sane = res.rows.len() <= n
                    && res.rows.iter().all(|row| (row.player.0 as usize) < n && (1..=n).contains(&(row.place as usize)));
                if !sane {
                    log::warn!("[client] malformed results ignored");
                    return;
                }
                if let Some(r) = self.run.as_mut() {
                    r.over = true;
                    r.predicting = false;
                    r.end = r.end.or(res.end_reason);
                }
                self.results = Some(res);
                self.phase = ClientPhase::Results;
                self.rematch_ends = None;
            }
            ToClient::Rematch(RematchMsg::Countdown { ends_at_host_ms }) => self.rematch_ends = Some(ends_at_host_ms),
            ToClient::Rematch(RematchMsg::Cancelled) => self.rematch_ends = None,
            ToClient::Pong { id, client_t, host_t } => self.clock.on_pong(id, client_t, host_t, now),
            ToClient::Closed(why) => {
                self.phase = if why == crate::host::HOST_LEFT { ClientPhase::HostLeft } else { ClientPhase::Closed(why) };
                self.host_check_since = None;
                self.end_run_on_loss();
            }
            ToClient::Left { player } => {
                let Some(r) = self.run.as_ref() else { return };
                if player == r.you {
                    return;
                }
                let Some(p) = r.players.iter().find(|p| p.id() == player) else { return };
                self.notices.push_back((format!("{} left", p.spec.name), now));
                while self.notices.len() > MAX_NOTICES {
                    self.notices.pop_front();
                }
            }
        }
    }

    fn on_start(&mut self, config: MatchConfig, start_host_ms: u64, you: PlayerId, run_no: u8) {
        let world = World::new(config);
        let Some(me) = world.player(you) else { return };
        if world.mode() != flock_core::Mode::Multi {
            return;
        }
        let tuning = world.tuning().clone();
        let go_tick = tuning.ticks(tuning.countdown);
        let n = world.living_count().max(1);
        let mean = world.runners().map(|r| r.pos.z).sum::<f64>() / n as f64;
        let run = Run {
            run_no,
            you,
            start_ms: start_host_ms,
            go_tick,
            cfg: world.config().clone(),
            players: world.players().to_vec(),
            placeholder: Course::new_remote(&tuning),
            pred: me.runner.clone(),
            predicting: true,
            over: false,
            end: None,
            next_seq: 1,
            ring: VecDeque::new(),
            latch_seq: None,
            ctx: CtxState { next_tick: 0, anchor: world.anchor_z(), mean },
            snaps: VecDeque::new(),
            newest_tick: 0,
            released: 0,
            corr_from: [0.0; 3],
            corr_age: 1.0,
            corr_ms: CORRECTION_MS,
            last_now: self.now,
            events: Vec::new(),
            place_results: Vec::new(),
            render_tick: 0.0,
            lateness: VecDeque::new(),
            delay_ms: INTERP_DELAY_MS,
            render_ms: None,
            render_rate: 1.0,
            tuning,
        };
        // The display world: this config's players on a remote course that
        // only the host's rows and events fill.
        let course = Course::new_remote(&run.tuning);
        let parts = shadow::Parts {
            cfg: &run.cfg,
            tick: 0,
            phase: Phase::Countdown,
            end: None,
            running_since: None,
            players: &run.players,
            anchor: world.anchor_z(),
            v_course: 0.0,
        };
        let shadow_world = match shadow::build(&parts, course, &run.placeholder) {
            Ok(w) => w,
            Err((e, course)) => {
                log::error!("[client] {e}");
                self.stats.shadow_errors += 1;
                // Fall back to a fresh world holding the remote course.
                let mut w = world.clone();
                *w.debug_course_mut() = course;
                w
            }
        };
        self.view = Some(FlockView::new(shadow_world, you, true));
        self.run = Some(run);
        self.results = None;
        self.rematch_ends = None;
        self.notices.clear();
        self.phase = ClientPhase::Running;
    }

    fn on_snapshot(&mut self, s: Snapshot, now: u64) {
        let Some(run) = self.run.as_mut() else { return };
        if s.run != run.run_no || s.players.len() > MAX_PLAYERS || s.bodies.len() > MAX_PLAYERS {
            return;
        }
        self.stats.snapshots += 1;
        if let Some(host_now) = self.clock.host_now(now) {
            run.note_lateness(s.tick, host_now, now);
        }
        if s.first_row > run.released {
            run.released = s.first_row;
            if let Some(v) = self.view.as_mut() {
                v.world.debug_course_mut().release_before(s.first_row);
            }
        }
        let course = self.view.as_ref().map(|v| v.world.course());
        if let Some((d, snapped)) = course.and_then(|c| run.on_snapshot(s, c)) {
            self.stats.last_correction_m = d;
            self.stats.max_correction_m = self.stats.max_correction_m.max(d);
            if snapped {
                self.stats.hard_snaps += 1;
            }
        }
    }

    fn lead_ms(&self) -> i64 {
        let one_way = self.clock.rtt_ms().unwrap_or(0).div_ceil(2) as i64;
        one_way + input_tick_ms(JITTER_TICKS as i64) + 1
    }

    /// Match frame: input production and prediction, then the view.
    fn run_frame(&mut self, now: u64, input: InputFrame) {
        let Some(host_now) = self.clock.host_now(now) else { return };
        let lead = self.lead_ms();
        let ping = self.rtt_ms();
        let (Some(run), Some(view)) = (self.run.as_mut(), self.view.as_mut()) else { return };
        let frame_ms = now.saturating_sub(run.last_now) as f64;
        let dt = (frame_ms as f32 / 1000.0).min(MAX_FRAME_S);
        run.last_now = now;
        run.advance_render(host_now, frame_ms);
        run.corr_age += dt;
        if !run.over {
            // Input n is needed by the host at start + (n-1)/60 s; we are
            // `lead` early. i128 and a cap: `start_ms` is the host's to choose.
            let t_ms = host_now as i128 + lead as i128 - run.start_ms as i128;
            let due: u32 =
                if t_ms < 0 { 0 } else { (t_ms * TICK_HZ as i128 / 1000 + 1).min(u32::MAX as i128 / 2) as u32 };
            if due.saturating_sub(run.next_seq) > MAX_CATCH_UP_TICKS {
                // Far behind (a stall): skip ahead; the prediction is reset
                // by the next snapshot.
                let skip_to = due - 2;
                run.ctx.next_tick += (skip_to - run.next_seq) as u64;
                run.next_seq = skip_to;
            }
            let mut sent_any = false;
            while run.next_seq <= due {
                run.produce(input, view.world.course());
                sent_any = true;
            }
            if sent_any {
                let seq = run.next_seq - 1;
                let n = run
                    .ring
                    .iter()
                    .rev()
                    .take(INPUTS_PER_PACKET)
                    .enumerate()
                    .take_while(|(k, (s, _, _))| *s == seq - *k as u32)
                    .count();
                let inputs: Vec<[u8; 2]> = run.ring.iter().skip(run.ring.len() - n).map(|(_, p, _)| *p).collect();
                let bytes = encode(&LossyToHost::Inputs { run: run.run_no, seq, inputs });
                self.stats.inputs_sent += 1;
                self.transport.send_lossy(bytes);
            }
        }
        match run.build_view(view, host_now, ping) {
            Ok(()) => {}
            Err(e) => {
                if self.stats.shadow_errors == 0 {
                    log::error!("[client] {e}");
                }
                self.stats.shadow_errors += 1;
            }
        }
        self.stats.interp_delay_ms = run.render_ms.map_or(run.delay_ms, |r| host_now as f64 - r) as f32;
        if !run.over && !run.snaps.is_empty() {
            self.stats.interp_frames += 1;
            if run.render_tick > run.newest_tick as f64 {
                self.stats.interp_starved_frames += 1;
            }
        }
        if self.results.is_some() {
            view.over = true;
        }
    }
}

impl Run {
    /// Take the next input number: record it and step the prediction.
    fn produce(&mut self, input: InputFrame, course: &Course) {
        let seq = self.next_seq;
        self.next_seq += 1;
        // The host's CmdLatch, mirrored: a command fires on a new seq.
        let cmd = if self.latch_seq != Some(input.seq) {
            self.latch_seq = Some(input.seq);
            input.cmd
        } else {
            None
        };
        self.ring.push_back((seq, input.pack(), cmd));
        while self.ring.len() > INPUT_RING {
            self.ring.pop_front();
        }
        let ctx = self.ctx.advance(&self.tuning, self.go_tick);
        if self.predicting {
            if let Some(ctx) = ctx {
                let cmds: Vec<Command> = cmd.into_iter().collect();
                let mut evs = Vec::new();
                runner_tick(&mut self.pred, course, &self.tuning, ctx, &cmds, &mut evs);
                self.events.extend(evs.into_iter().filter(|e| is_own_motion_event(e, self.you)));
            }
        }
    }

    /// A snapshot arrived. Returns the reconciliation error `(metres,
    /// snapped)` when it was the newest and the runner is predicted.
    fn on_snapshot(&mut self, s: Snapshot, course: &Course) -> Option<(f32, bool)> {
        if self.snaps.iter().any(|x| x.tick == s.tick) {
            return None;
        }
        let newest = s.tick > self.newest_tick || self.snaps.is_empty();
        let pos = self.snaps.iter().position(|x| x.tick > s.tick).unwrap_or(self.snaps.len());
        let own = s.own.clone();
        let ack = s.ack_seq;
        let (anchor, mean) = (s.anchor, s.mean_z);
        let over = s.phase == Phase::Over;
        if newest {
            if let Some(e) = s.end {
                self.end = Some(e);
            }
            self.apply_scores(&s.players);
        }
        self.snaps.insert(pos, s);
        while self.snaps.len() > SNAP_BUFFER {
            self.snaps.pop_front();
        }
        if !newest {
            return None;
        }
        self.newest_tick = self.snaps.back().map_or(0, |x| x.tick);
        let Some(mut own) = own else {
            // Not a runner any more (ghost, disconnected): nothing to predict.
            self.predicting = false;
            self.corr_from = [0.0; 3];
            return None;
        };
        sanitize_own(&mut own);
        if over || self.over {
            self.predicting = false;
        }
        if !self.predicting {
            self.pred = own;
            self.corr_from = [0.0; 3];
            return None;
        }
        let old = self.pred.pos;
        self.pred = own;
        while self.ring.front().is_some_and(|(n, _, _)| *n <= ack) {
            self.ring.pop_front();
        }
        // Input n drives tick n - 1: replay from tick `ack`.
        self.ctx = CtxState { next_tick: ack as Tick, anchor, mean };
        let replay: Vec<Option<Command>> = self.ring.iter().map(|(_, _, c)| *c).collect();
        for cmd in replay {
            if let Some(ctx) = self.ctx.advance(&self.tuning, self.go_tick) {
                let cmds: Vec<Command> = cmd.into_iter().collect();
                // Replays raise no events: they were raised when first predicted.
                let mut sink = Vec::new();
                runner_tick(&mut self.pred, course, &self.tuning, ctx, &cmds, &mut sink);
            }
        }
        let delta = [old.x - self.pred.pos.x, old.y - self.pred.pos.y, (old.z - self.pred.pos.z) as f32];
        let cur = self.current_correction();
        let total = [cur[0] + delta[0], cur[1] + delta[1], cur[2] + delta[2]];
        let len = (total[0] * total[0] + total[1] * total[1] + total[2] * total[2]).sqrt();
        let snapped = !len.is_finite() || len > SNAP_DISTANCE;
        self.corr_from = if snapped { [0.0; 3] } else { total };
        self.corr_ms = correction_ms(if snapped { 0.0 } else { len });
        self.corr_age = 0.0;
        let d = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
        Some((d, snapped))
    }

    /// Roles and scores from the newest snapshot.
    fn apply_scores(&mut self, nets: &[PlayerNet]) {
        for n in nets {
            if let Some(p) = self.players.iter_mut().find(|p| p.id() == n.id) {
                p.role = n.role;
                p.placements = n.placements;
                p.survived_ticks = n.survived_ticks;
                p.disconnected = matches!(n.role, Role::Disconnected);
            }
        }
    }

    fn current_correction(&self) -> [f32; 3] {
        let k = correction_left(self.corr_age * 1000.0, self.corr_ms);
        [self.corr_from[0] * k, self.corr_from[1] * k, self.corr_from[2] * k]
    }

    fn note_lateness(&mut self, tick: u32, host_now: u64, local: u64) {
        let sent = self.start_ms as f64 + tick as f64 * 1000.0 / TICK_HZ as f64;
        self.lateness.push_back((local, host_now as f64 - sent));
        while self.lateness.front().is_some_and(|(t, _)| t + LATENESS_WINDOW_MS < local) {
            self.lateness.pop_front();
        }
        while self.lateness.len() > 4 * SNAP_BUFFER * LATENESS_WINDOW_MS as usize / 1000 {
            self.lateness.pop_front();
        }
    }

    /// Move the others' clock on by a frame: toward `host_now - delay`, at
    /// between [`RENDER_RATE_MIN`] and [`RENDER_RATE_MAX`] of real time.
    fn advance_render(&mut self, host_now: u64, frame_ms: f64) {
        let dt_ms = frame_ms.clamp(0.0, RENDER_MAX_FRAME_MS);
        let late: Vec<f64> = self.lateness.iter().map(|(_, l)| *l).collect();
        let target = interp_target_ms(&late);
        self.delay_ms = target.max(self.delay_ms - DELAY_FALL_PER_S * dt_ms / 1000.0);
        let goal = host_now as f64 - self.delay_ms;
        self.render_ms = Some(match self.render_ms {
            Some(r) if (goal - r).abs() <= RENDER_SNAP_MS => {
                let r = r + dt_ms;
                let mut rate = (1.0 + (goal - r) / RENDER_STEER_MS).clamp(RENDER_RATE_MIN, RENDER_RATE_MAX);
                if let Some(newest) = self.snaps.back() {
                    let newest_ms = self.start_ms as f64 + newest.tick as f64 * 1000.0 / TICK_HZ as f64;
                    let room = ((newest_ms - r) / RENDER_HEADROOM_MS).clamp(0.0, 1.0);
                    if room < 1.0 {
                        rate = rate.min(RENDER_RATE_STARVED + (1.0 - RENDER_RATE_STARVED) * room);
                    }
                }
                let step = RENDER_RATE_CHANGE_PER_S * dt_ms / 1000.0;
                let rate = rate.clamp(self.render_rate - step, self.render_rate + step);
                self.render_rate = rate;
                r - dt_ms * (1.0 - rate)
            }
            _ => {
                self.render_rate = 1.0;
                goal
            }
        });
        let r = self.render_ms.unwrap_or(goal);
        self.render_tick = (r - self.start_ms as f64) * TICK_HZ as f64 / 1000.0;
    }

    /// The two snapshots around `render_tick` and the fraction between them.
    fn bracket(&self, render_tick: f64) -> Option<(&Snapshot, &Snapshot, f64)> {
        let first = self.snaps.front()?;
        let last = self.snaps.back()?;
        if render_tick <= first.tick as f64 {
            return Some((first, first, 0.0));
        }
        if render_tick >= last.tick as f64 {
            return Some((last, last, 0.0));
        }
        let j = self.snaps.iter().position(|s| s.tick as f64 > render_tick)?;
        let (a, b) = (&self.snaps[j - 1], &self.snaps[j]);
        let t = (render_tick - a.tick as f64) / (b.tick - a.tick).max(1) as f64;
        Some((a, b, t))
    }

    /// Another player's body at `render_tick`, if it is a runner there.
    fn interpolate(&self, id: PlayerId, render_tick: f64, base: &Runner) -> Option<Runner> {
        let (a, b, t) = self.bracket(render_tick)?;
        let na = a.bodies.iter().find(|n| n.id == id)?;
        let nb = b.bodies.iter().find(|n| n.id == id).unwrap_or(na);
        let mut r = base.clone();
        if std::ptr::eq(a, b) {
            let ahead = (render_tick - a.tick as f64).clamp(0.0, EXTRAPOLATE_TICKS) as f32 * DT;
            na.apply(&mut r);
            let v_course = a.v_course;
            r.pos.x += na.vel[0] * ahead;
            r.pos.y = (r.pos.y + na.vel[1] * ahead).max(0.0);
            r.pos.z += ((v_course + na.vel[2]) * ahead) as f64;
            return Some(r);
        }
        let near = if t < 0.5 { na } else { nb };
        near.apply(&mut r);
        if (na.z - nb.z).abs() <= TELEPORT_M {
            let tf = t as f32;
            let lerp = |x: f32, y: f32| x + (y - x) * tf;
            r.pos.x = lerp(na.x, nb.x);
            r.pos.y = lerp(na.y, nb.y);
            r.pos.z = na.z + (nb.z - na.z) * t;
            r.vel.x = lerp(na.vel[0], nb.vel[0]);
            r.vel.y = lerp(na.vel[1], nb.vel[1]);
            r.vel.z_off = lerp(na.vel[2], nb.vel[2]);
        }
        Some(r)
    }

    /// Rebuild the display world for this frame.
    fn build_view(&mut self, view: &mut FlockView, host_now: u64, ping: Option<u32>) -> Result<(), String> {
        let tick_f = (host_now as f64 - self.start_ms as f64) * TICK_HZ as f64 / 1000.0;
        let newest = self.snaps.back();
        let mut tick = tick_f.floor().max(0.0) as Tick;
        let over = self.over || newest.is_some_and(|s| s.phase == Phase::Over);
        if over {
            // Freeze at the host's final tick.
            if let Some(s) = newest {
                tick = s.tick as Tick;
            }
        }
        let phase = if over {
            Phase::Over
        } else if tick >= self.go_tick {
            Phase::Running
        } else {
            Phase::Countdown
        };
        // Bodies: own predicted + correction; others interpolated.
        let render_tick = self.render_tick;
        let corr = self.current_correction();
        let mut players = std::mem::take(&mut self.players);
        for p in players.iter_mut() {
            if !p.role.is_runner() {
                continue;
            }
            if p.id() == self.you {
                if self.predicting || newest.is_some() {
                    p.runner = self.pred.clone();
                    p.runner.pos.x += corr[0];
                    p.runner.pos.y = (p.runner.pos.y + corr[1]).max(0.0);
                    p.runner.pos.z += corr[2] as f64;
                }
            } else if let Some(r) = self.interpolate(p.id(), render_tick, &p.runner) {
                p.runner = r;
            }
        }
        self.players = players;
        let (anchor, v_course) = match self.bracket(render_tick) {
            Some((a, b, t)) => (a.anchor + (b.anchor - a.anchor) * t, a.v_course + (b.v_course - a.v_course) * t as f32),
            None => (view.world.anchor_z(), view.world.v_course()),
        };
        let running_since = (tick >= self.go_tick).then_some(self.go_tick);
        let parts = shadow::Parts {
            cfg: &self.cfg,
            tick,
            phase,
            end: if over { self.end.or(Some(EndReason::Aborted)) } else { None },
            running_since,
            players: &self.players,
            anchor,
            v_course,
        };
        let course = std::mem::replace(view.world.debug_course_mut(), self.placeholder.clone());
        let result = match shadow::build(&parts, course, &self.placeholder) {
            Ok(w) => {
                view.world = w;
                Ok(())
            }
            Err((e, course)) => {
                *view.world.debug_course_mut() = course;
                // Still move the bodies on the stale shadow.
                for p in &self.players {
                    if let Some(r) = view.world.debug_runner_mut(p.id()) {
                        *r = p.runner.clone();
                    }
                }
                Err(e)
            }
        };
        view.events = std::mem::take(&mut self.events);
        view.place_results = std::mem::take(&mut self.place_results);
        view.correction = corr;
        view.ping_ms = ping;
        view.refresh();
        view.over = over;
        result
    }
}

impl RunDriver for ClientSession {
    fn update(&mut self, now_ms: u64, input: InputFrame) {
        ClientSession::update(self, now_ms, input);
    }
    fn place(&mut self, tool: Tool, lane: u8, row: u32) -> u32 {
        ClientSession::place(self, tool, lane, row)
    }
    fn view(&self) -> Option<&FlockView> {
        self.view.as_ref()
    }
    fn results(&self) -> Option<&ResultsView> {
        self.results.as_ref()
    }
    fn set_paused(&mut self, _p: bool) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctx_mirror_matches_world_next_ctx_on_a_snapshot_tick() {
        let mut w = World::new(MatchConfig::multi(4, 3, Some(flock_core::BotSkill::NORMAL)));
        for _ in 0..400 {
            w.step(&[]);
        }
        let n = w.living_count();
        let mean = w.runners().map(|r| r.pos.z).sum::<f64>() / n as f64;
        let t = w.tuning().clone();
        let go = t.ticks(t.countdown);
        let mut c = CtxState { next_tick: w.tick(), anchor: w.anchor_z(), mean };
        let mine = c.advance(&t, go).unwrap();
        let host = w.next_ctx();
        assert_eq!(mine.tick, host.tick);
        assert_eq!(mine.v_course, host.v_course);
        assert_eq!(mine.anchor_z, host.anchor_z);
        assert!(mine.pack && host.pack);
    }

    #[test]
    fn host_event_ids_are_range_checked() {
        assert!(event_ok(&Event::Jumped { player: PlayerId(2) }, 3));
        assert!(!event_ok(&Event::Jumped { player: PlayerId(3) }, 3));
        assert!(!event_ok(&Event::Bump { a: PlayerId(0), b: PlayerId(9), speed: 1.0 }, 6));
        assert!(!event_ok(&Event::LaneChange { player: PlayerId(0), to: 7 }, 6));
    }
}
