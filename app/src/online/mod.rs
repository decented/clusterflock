//! Online play glue: one [`OnlineSession`] per process, in a global slot the
//! screens borrow (`online::with(|s| ..)`). Dropping or replacing a screen
//! never ends the room; only [`leave`], [`put`] (replacing it) or the engine's
//! account upkeep do. The engine drives the session every frame and from the
//! background tick when no screen did ([`pump_auto`], [`end_frame`]).
//!
//! Forked from Stampede's `app/src/online` (no host migration, no Keeps,
//! no custom tracks), plus Twang Thang's Bluetooth door: a hosting session
//! can open a Bluetooth door into the same room ([`OnlineSession::start_bluetooth`]),
//! whose joiners wait for the host's Accept. See `docs/api/flock-session.md`.
//!
//! Every way in (host, join by code, Nearby, Bluetooth) first takes the
//! My Signet identity from [`crate::account::identity_for_online`]: with no
//! approved persona there is no identity and no socket is opened.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use flock_core::{BotSkill, InputFrame, Tool};
use flock_session::door::{BtStatus, HostLink, Radio};
use flock_session::net::{self, Identity, LanBrowser, LanRoom, NetConfig, QuicClient, QuicHost};
use flock_session::{
    host_with_local_player, ClientPhase, ClientSession, ClientTransport, FlockView, HostAuthority, HostConfig, HostPhase,
    LobbyView, LoopClock, PeerKey, PendingJoiner, ResultsView, RunDriver,
};

use crate::platform::ble::{self, Ble, BleAdvert, BleSeen, BleState};

#[cfg(test)]
mod tests;

/// Tests that put a session in the process-wide slot, or run an `Engine` frame while one may be
/// there (the engine leaves the slot's room when approval ends), take this lock.
#[cfg(test)]
pub(crate) static TEST_SLOT_LOCK: Mutex<()> = Mutex::new(());

/// `gameflock://join#CODE`, what Share link puts on the clipboard.
pub fn join_link(code: &str) -> String {
    format!("{}://join#{code}", crate::platform::LINK_SCHEME)
}

/// The code as shown: `KWT QPM`.
pub fn spaced_code(code: &str) -> String {
    let mut out = String::new();
    for (i, c) in code.chars().enumerate() {
        if i == 3 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Network settings: Flock's app tag (`flock_session::net::APP_TAG`, equal to
/// `account::APP_ID`), the app's relays, and the local-socket hook so Nearby
/// and LAN play stay on the Wi-Fi even with a VPN up (see `platform::lan`).
pub fn net_config(data_dir: &Path) -> NetConfig {
    let mut c = net::net_config(data_dir);
    c.relays = crate::platform::relays();
    c.vpn_active = crate::platform::vpn_active();
    c.lan_socket_hook = crate::platform::lan_socket_hook();
    c
}

fn identity(net: &NetConfig) -> Result<Identity, String> {
    crate::account::identity_for_online(&net.data_dir)
}

/// Listen for Nearby rooms (the caller holds the Wi-Fi multicast lock while
/// browsing, `Platform::set_multicast_lock`). `browser.poll()` returns the current list of [`LanRoom`]s.
pub fn browse_nearby(data_dir: &Path) -> LanBrowser {
    net::browse_nearby(&net_config(data_dir))
}

// ─── Bluetooth radio helpers (no session needed) ────────────────────────────

/// The radio's state (`Unsupported` hides every Bluetooth control).
pub fn ble_state() -> BleState {
    ble::get().state()
}

/// Start scanning for Bluetooth games; read them with [`ble_seen`]. Do not
/// call every frame (Android throttles scan starts).
pub fn ble_scan() -> Result<(), String> {
    ble::get().start_scan()
}

/// Stop scanning (leaving the Nearby screen, going to the background).
pub fn ble_stop_scan() {
    ble::get().stop_scan()
}

/// Bluetooth games in range, strongest first: show
/// `"Bluetooth game · " + advert.room_label()` and the host colour.
pub fn ble_seen() -> Vec<BleSeen> {
    ble::get().seen()
}

/// Why scanning finds nothing, if the radio said.
pub fn ble_scan_error() -> Option<String> {
    ble::get().scan_error()
}

/// The room name a Bluetooth joiner sees for an advert ("Bluetooth game · room 1A2B").
pub fn bluetooth_room_name(seen: &BleSeen) -> String {
    net::bluetooth_room_name(&seen.advert.room_label())
}

/// The Bluetooth door's radio as the session crate sees it.
struct BleRadio {
    ble: Arc<dyn Ble>,
    advert: BleAdvert,
}

impl Radio for BleRadio {
    fn stop_advertising(&self) {
        self.ble.stop_advertising();
    }
    fn advertising_error(&self) -> Option<String> {
        self.ble.advertising_error()
    }
    fn room_label(&self) -> String {
        self.advert.room_label()
    }
}

// ─── The session ────────────────────────────────────────────────────────────

/// A joiner's QUIC link whose blocking close (up to 300 ms) and drop run on a
/// short-lived thread, so leaving never stalls a frame.
struct DetachedClient(Option<QuicClient>);

impl ClientTransport for DetachedClient {
    fn poll(&mut self) -> Option<flock_session::ClientNetEvent> {
        self.0.as_mut()?.poll()
    }
    fn send_reliable(&mut self, bytes: Vec<u8>) {
        if let Some(q) = self.0.as_mut() {
            q.send_reliable(bytes)
        }
    }
    fn send_lossy(&mut self, bytes: Vec<u8>) {
        if let Some(q) = self.0.as_mut() {
            q.send_lossy(bytes)
        }
    }
    fn rtt_ms(&self) -> Option<u32> {
        self.0.as_ref()?.rtt_ms()
    }
    fn close(&mut self) {
        if let Some(mut q) = self.0.take() {
            let _ = std::thread::Builder::new().name("flock-client-close".into()).spawn(move || q.close());
        }
    }
    fn host_check_pending(&self) -> bool {
        self.0.as_ref().is_some_and(|q| q.host_check_pending())
    }
    fn verify_host(&mut self, attestation: &str) -> Result<String, String> {
        self.0.as_mut().ok_or("closed")?.verify_host(attestation)
    }
}

impl Drop for DetachedClient {
    fn drop(&mut self) {
        if let Some(q) = self.0.take() {
            let _ = std::thread::Builder::new().name("flock-client-drop".into()).spawn(move || drop(q));
        }
    }
}

/// The room's transport handle as the authority owns it: shutting it down
/// (which blocks up to a second flushing closes) runs off the UI thread.
struct DetachedLink(HostLink<QuicHost>);

impl flock_session::HostTransport for DetachedLink {
    fn poll(&mut self) -> Option<flock_session::HostNetEvent> {
        self.0.poll()
    }
    fn send_reliable(&mut self, peer: PeerKey, bytes: Vec<u8>) {
        self.0.send_reliable(peer, bytes)
    }
    fn send_lossy(&mut self, peer: PeerKey, bytes: Vec<u8>) {
        self.0.send_lossy(peer, bytes)
    }
    fn kick(&mut self, peer: PeerKey) {
        self.0.kick(peer)
    }
    fn disconnect(&mut self, peer: PeerKey) {
        self.0.disconnect(peer)
    }
    fn set_open(&mut self, open: bool) {
        self.0.set_open(open)
    }
    fn rtt_ms(&self, peer: PeerKey) -> Option<u32> {
        self.0.rtt_ms(peer)
    }
    fn shutdown(&mut self) {
        let mut link = self.0.clone();
        if std::thread::Builder::new().name("flock-host-shutdown".into()).spawn(move || link.shutdown()).is_err() {
            self.0.shutdown();
        }
    }
}

/// How the room is doing, for the lobby's status lines.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoomStatus {
    /// The room is listening.
    pub ready: bool,
    pub failure: Option<String>,
    /// Relays up / configured.
    pub relays: (usize, usize),
    /// Where Nearby players dial (LAN address and port).
    pub lan: Option<SocketAddr>,
}

/// How a session was opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinedBy {
    Host,
    Code,
    Nearby,
    Bluetooth,
    /// No network (tests, screenshots).
    Offline,
}

/// A game on this device.
pub struct OnlineSession {
    /// The local player (the host's own player when hosting).
    me: ClientSession,
    /// Hosting: the authority.
    host: Option<HostAuthority>,
    /// Hosting: the room's doors (LAN/WAN, and Bluetooth when on).
    link: Option<HostLink<QuicHost>>,
    /// Hosting: what opening the Bluetooth door needs.
    net: Option<(NetConfig, Identity)>,
    by: JoinedBy,
    code: Option<String>,
    clock: LoopClock,
    left: bool,
    pumped: bool,
    /// The frame the run screen last gave (repeated by `pump_auto`).
    input: InputFrame,
    status: RoomStatus,
    relay_up_since: Option<u64>,
    away_since: Option<u64>,
    last_away: Option<(u64, u64)>,
}

/// A player is "away too long" after this (ms)...
const LONG_AWAY_MS: u64 = 2500;
/// ...and a lost connection is still explained by it this long after coming back.
const AWAY_EXPLAINS_MS: u64 = 20_000;
/// A relay must have been up this long before the room code is offered.
const CODE_GRACE_MS: u64 = 1200;
/// What a player who lost the link while away is told.
pub const AWAY_TEXT: &str = "You were away too long, so the game carried on without you.";

/// The hat the local player wears in the next room (a hat id from
/// `flock_render::HATS`, 255 = the colour's own). The Hello carries it and it is
/// fixed for the session, so it is read when a session is built. Home and the
/// wardrobe keep it current.
static LOCAL_HAT: AtomicU8 = AtomicU8::new(255);

/// Set the hat the next session's Hello will carry.
pub fn set_local_hat(hat: u8) {
    LOCAL_HAT.store(hat, Ordering::Relaxed);
}

impl OnlineSession {
    fn wrap(mut me: ClientSession, host: Option<HostAuthority>, link: Option<HostLink<QuicHost>>, by: JoinedBy) -> OnlineSession {
        me.set_hat(LOCAL_HAT.load(Ordering::Relaxed));
        OnlineSession {
            me,
            host,
            link,
            net: None,
            by,
            code: None,
            clock: LoopClock::real(),
            left: false,
            pumped: false,
            input: InputFrame::default(),
            status: RoomStatus::default(),
            relay_up_since: None,
            away_since: None,
            last_away: None,
        }
    }

    /// Open a room: the code online and the beacon on the LAN at once.
    /// `colour` is the host's preferred colour (0..6).
    pub fn host(data_dir: &Path, name: &str, colour: u8) -> Result<OnlineSession, String> {
        let net = net_config(data_dir);
        let id = identity(&net)?;
        let quic = net::host_room(&net, &id, &format!("{name}'s flock"));
        Ok(Self::network_host(net, id, quic, name, colour, HostConfig::default()))
    }

    /// A room on `quic` (opened with `net` and `id`), with the host's own player.
    pub fn network_host(net: NetConfig, id: Identity, quic: QuicHost, name: &str, colour: u8, cfg: HostConfig) -> OnlineSession {
        let code = quic.code().map(str::to_string);
        let link = HostLink::new(quic);
        let (mut host, me) = host_with_local_player(Some(Box::new(DetachedLink(link.clone()))), cfg, name, colour);
        match net::own_persona(&id) {
            Some((json, npub)) => host.set_host_identity(Some(json), Some(npub)),
            None => log::warn!("online: hosting without a verifiable My Signet approval"),
        }
        log::info!("online: hosting room {}", code.as_deref().unwrap_or("?"));
        let mut s = OnlineSession::wrap(me, Some(host), Some(link), JoinedBy::Host);
        s.net = Some((net, id));
        s.code = code;
        s
    }

    /// A host with no network at all (tests, screenshots): the lobby and the
    /// match work, nobody can join.
    pub fn offline_host(name: &str, colour: u8, cfg: HostConfig) -> OnlineSession {
        let (host, me) = host_with_local_player(None, cfg, name, colour);
        OnlineSession::wrap(me, Some(host), None, JoinedBy::Offline)
    }

    /// Join by room code (online, through the relays).
    pub fn join_code(data_dir: &Path, code: &str, name: &str, colour: u8) -> Result<OnlineSession, String> {
        let net = net_config(data_dir);
        let id = identity(&net)?;
        log::info!("online: joining room {code}");
        let link = net::join_code(&net, &id, code);
        Ok(Self::network_client(link, name, colour, JoinedBy::Code))
    }

    /// Join a room heard on the LAN ([`browse_nearby`]).
    pub fn join_nearby(data_dir: &Path, room: &LanRoom, name: &str, colour: u8) -> Result<OnlineSession, String> {
        let net = net_config(data_dir);
        let id = identity(&net)?;
        log::info!("online: joining nearby room {} at {}", room.code, room.addr);
        let link = net::join_nearby(&net, &id, room);
        Ok(Self::network_client(link, name, colour, JoinedBy::Nearby))
    }

    /// Join a Bluetooth game found by [`ble_seen`]. The player waits
    /// ([`ClientPhase::Waiting`]) until the host accepts; the host's persona
    /// is verified after that ([`Self::host_persona`]).
    pub fn join_bluetooth(data_dir: &Path, seen: &BleSeen, name: &str, colour: u8) -> Result<OnlineSession, String> {
        Self::join_bluetooth_with(ble::get(), data_dir, seen, name, colour)
    }

    /// [`Self::join_bluetooth`] on a given radio (tests).
    pub fn join_bluetooth_with(radio: Arc<dyn Ble>, data_dir: &Path, seen: &BleSeen, name: &str, colour: u8) -> Result<OnlineSession, String> {
        let net = net_config(data_dir);
        let id = identity(&net)?;
        Self::bluetooth_client(radio, &net, &id, seen, name, colour)
    }

    pub(crate) fn bluetooth_client(radio: Arc<dyn Ble>, net: &NetConfig, id: &Identity, seen: &BleSeen, name: &str, colour: u8) -> Result<OnlineSession, String> {
        let link = radio.connect(&seen.device_id)?;
        log::info!("online: joining a Bluetooth game, room {}", seen.advert.room_label());
        Ok(Self::network_client(net::join_bluetooth(net, id, link), name, colour, JoinedBy::Bluetooth))
    }

    pub(crate) fn network_client(link: QuicClient, name: &str, colour: u8, by: JoinedBy) -> OnlineSession {
        let me = ClientSession::new(Box::new(DetachedClient(Some(link))), name, colour);
        OnlineSession::wrap(me, None, None, by)
    }

    // ─── Bluetooth door (hosting) ────────────────────────────────────────

    /// Open the Bluetooth door into this room: advertise and take joiners
    /// over the radio. They wait in [`Self::pending_joiners`] for
    /// [`Self::accept`]. `colour` is shown in the advert.
    pub fn start_bluetooth(&mut self, colour: u8) -> Result<(), String> {
        self.start_bluetooth_with(ble::get(), colour)
    }

    /// [`Self::start_bluetooth`] on a given radio (tests).
    pub fn start_bluetooth_with(&mut self, radio: Arc<dyn Ble>, colour: u8) -> Result<(), String> {
        let (Some(link), Some((net, id))) = (self.link.as_ref(), self.net.as_ref()) else {
            return Err("Only the phone that opened the room can open Bluetooth.".into());
        };
        match radio.state() {
            BleState::Ready => {}
            BleState::Unsupported => return Err("Bluetooth games aren't available on this device.".into()),
            BleState::Off => return Err("Bluetooth is off.".into()),
            BleState::NeedsPermission | BleState::PermissionDenied => {
                return Err("Cluster Flock needs the Bluetooth permission.".into())
            }
        }
        // Joiners verify the host's own attestation after Accept: without a
        // valid one they would be seated and then fail. Refuse up front.
        net::bluetooth_host_check(id)?;
        let rid = nostr::Keys::generate().public_key().to_bytes();
        let advert = BleAdvert::new([rid[0], rid[1], rid[2], rid[3]], colour % ble::ADVERT_COLOURS);
        let links = radio.start_advertising(advert)?;
        let bt = net::host_bluetooth(net, id, &advert.room_label(), links);
        if let Err(e) = link.start_bluetooth(Box::new(bt), Box::new(BleRadio { ble: radio.clone(), advert })) {
            radio.stop_advertising();
            return Err(e);
        }
        log::info!("online: Bluetooth door open, room {}", advert.room_label());
        Ok(())
    }

    /// Close the Bluetooth door (players already seated keep playing).
    /// Waiting joiners are turned away.
    pub fn stop_bluetooth(&mut self) {
        if let Some(h) = self.host.as_mut() {
            let waiting: Vec<PeerKey> = h.pending_joiners().iter().map(|p| p.peer).collect();
            for p in waiting {
                h.decline_pending(p);
            }
        }
        if let Some(l) = &self.link {
            l.stop_bluetooth();
        }
    }

    /// Where the Bluetooth door stands (`Off` when not hosting).
    pub fn bluetooth(&self) -> BtStatus {
        self.link.as_ref().map_or(BtStatus::Off, |l| l.bluetooth())
    }

    /// Bluetooth joiners waiting for Accept: their verified persona
    /// (`npub1...`) and the name they typed.
    pub fn pending_joiners(&self) -> Vec<PendingJoiner> {
        self.host.as_ref().map(|h| h.pending_joiners().to_vec()).unwrap_or_default()
    }

    /// Seat a waiting Bluetooth joiner.
    pub fn accept(&mut self, peer: PeerKey) -> bool {
        // The attestation sent to the joiner is re-read now: one that lapsed
        // since hosting began is not sent (the host refuses the joiner).
        if let (Some(h), Some((_, id))) = (self.host.as_mut(), self.net.as_ref()) {
            match net::own_persona(id) {
                Some((json, npub)) => h.set_host_identity(Some(json), Some(npub)),
                None => h.set_host_identity(None, None),
            }
        }
        self.host.as_mut().is_some_and(|h| h.accept_pending(peer))
    }

    /// Turn a waiting Bluetooth joiner away.
    pub fn decline(&mut self, peer: PeerKey) -> bool {
        self.host.as_mut().is_some_and(|h| h.decline_pending(peer))
    }

    // ─── The frame ───────────────────────────────────────────────────────

    /// Milliseconds on this session's clock.
    pub fn now_ms(&self) -> u64 {
        self.clock.now_ms()
    }

    /// Drive the session one frame: the host's world and network first, then
    /// your player with `input` (the run screen's current frame).
    pub fn pump(&mut self, input: InputFrame) {
        if self.left {
            return;
        }
        self.pumped = true;
        self.input = input;
        let now = self.clock.now_ms();
        if let Some(h) = self.host.as_mut() {
            h.update(now);
        }
        self.me.update(now, input);
        if let Some(link) = &self.link {
            let st = link.with_lan(|q| RoomStatus {
                ready: q.is_ready(),
                failure: q.failure().map(str::to_string),
                relays: q.relays(),
                lan: q.local_addr(),
            });
            if st.relays.0 > 0 {
                self.relay_up_since.get_or_insert(now);
            } else {
                self.relay_up_since = None;
            }
            if st != self.status {
                log::info!("online: room {:?} status {:?}", self.code, st);
                self.status = st;
            }
        }
    }

    /// The engine's catch-all: drive the session if no screen did this frame
    /// (another screen on top, the background tick). Your runner keeps the
    /// last frame (no new command).
    pub fn pump_auto(&mut self) {
        if self.pumped || self.left {
            return;
        }
        let input = self.input;
        self.pump(input);
    }

    /// Close the frame: the next `pump_auto` drives the session again.
    pub fn end_frame(&mut self) {
        self.pumped = false;
    }

    /// The app left (`true`) or came back to (`false`) the foreground. Coming
    /// back returns how long the player was away.
    pub fn set_away(&mut self, away: bool) -> Option<u64> {
        let now = self.clock.now_ms();
        match (away, self.away_since) {
            (true, None) => self.away_since = Some(now),
            (false, Some(t)) => {
                self.away_since = None;
                let len = now.saturating_sub(t);
                self.last_away = Some((now, len));
                return Some(len);
            }
            _ => {}
        }
        None
    }

    fn away_explains_loss(&self) -> bool {
        let now = self.clock.now_ms();
        self.away_since.is_some_and(|t| now.saturating_sub(t) >= LONG_AWAY_MS)
            || self.last_away.is_some_and(|(end, len)| len >= LONG_AWAY_MS && now.saturating_sub(end) <= AWAY_EXPLAINS_MS)
    }

    // ─── Reading ─────────────────────────────────────────────────────────

    /// Your player's session (lobby view, phase, results, run view).
    pub fn me(&self) -> &ClientSession {
        &self.me
    }
    /// The room's authority (hosting only).
    pub fn authority(&self) -> Option<&HostAuthority> {
        self.host.as_ref()
    }
    pub fn phase(&self) -> &ClientPhase {
        self.me.phase()
    }
    pub fn lobby(&self) -> Option<&LobbyView> {
        self.me.lobby()
    }
    pub fn my_seat(&self) -> Option<u8> {
        self.me.my_seat()
    }
    pub fn results(&self) -> Option<&ResultsView> {
        self.me.results()
    }
    pub fn is_host(&self) -> bool {
        self.host.is_some()
    }
    pub fn joined_by(&self) -> JoinedBy {
        self.by
    }
    /// The host's verified My Signet persona (`npub1...`): from the link for
    /// code/Nearby joins, after Accept for Bluetooth. `None` while unverified.
    pub fn host_persona(&self) -> Option<&str> {
        self.me.host_persona()
    }
    /// "<name> left" notices (shown ~4 s).
    pub fn notice(&self) -> Option<String> {
        self.me.notice()
    }
    /// The room code (hosting).
    pub fn code(&self) -> Option<&str> {
        self.code.as_deref()
    }
    pub fn status(&self) -> &RoomStatus {
        &self.status
    }
    /// The room is listening (or there is no network to wait for).
    pub fn room_ready(&self) -> bool {
        self.link.is_none() || (self.status.ready && self.status.failure.is_none())
    }
    /// The room code is safe to hand out (a relay has carried it a moment).
    pub fn code_ready(&self) -> bool {
        self.link.is_none()
            || (self.room_ready() && self.relay_up_since.is_some_and(|t| self.clock.now_ms().saturating_sub(t) >= CODE_GRACE_MS))
    }
    /// Can the host start now (at least two seats, one human)?
    pub fn can_start(&self) -> bool {
        self.host.as_ref().is_some_and(|h| h.can_start() && h.phase() == HostPhase::Lobby)
    }
    /// Milliseconds left of the rematch 3-2-1, if it runs.
    pub fn rematch_ms_left(&self) -> Option<u64> {
        self.me.rematch_ms_left(self.clock.now_ms())
    }
    /// The link is gone for good, in words for the player ("The host left
    /// the game."); `None` while it is up.
    pub fn dead_reason(&self) -> Option<String> {
        if matches!(self.me.phase(), ClientPhase::Disconnected(_)) && self.away_explains_loss() {
            return Some(AWAY_TEXT.into());
        }
        dead_text(self.me.phase())
    }
    /// The host left (show "Host left" and offer Solo / back to the
    /// multiplayer menu).
    pub fn host_left(&self) -> bool {
        matches!(self.me.phase(), ClientPhase::HostLeft)
    }
    /// Worst round trip to anyone (host: its slowest guest; guest: the host).
    pub fn rtt_ms(&self) -> Option<u32> {
        match &self.host {
            Some(h) => {
                let lobby = self.me.lobby()?;
                lobby.seats.iter().filter(|s| !s.bot && !s.host).filter_map(|s| h.rtt_ms(s.id)).max()
            }
            None => self.me.rtt_ms(),
        }
    }

    // ─── Lobby and results actions ───────────────────────────────────────

    pub fn set_ready(&mut self, ready: bool) {
        self.me.set_ready(ready);
    }
    /// Pick a colour 0..6 (another person's is refused; a bot's moves).
    pub fn set_colour(&mut self, colour: u8) {
        self.me.set_colour(colour);
    }
    /// Host: add a bot seat (lobby, while a seat is free).
    pub fn add_bot(&mut self) -> bool {
        self.host.as_mut().is_some_and(|h| h.add_bot())
    }
    /// Host: remove a bot seat (`None`: the last one).
    pub fn remove_bot(&mut self, seat: Option<u8>) -> bool {
        self.host.as_mut().is_some_and(|h| h.remove_bot(seat))
    }
    /// Host: the bots' skill for the next match.
    pub fn set_bot_skill(&mut self, skill: BotSkill) {
        if let Some(h) = self.host.as_mut() {
            h.set_bot_skill(skill);
        }
    }
    /// Host: start now (otherwise the match starts when everyone is ready).
    pub fn start(&mut self) -> bool {
        self.host.as_mut().is_some_and(|h| h.start())
    }
    /// Host: remove a person from the lobby.
    pub fn kick(&mut self, seat: u8) -> bool {
        self.host.as_mut().is_some_and(|h| h.kick(seat))
    }
    /// Results: ready for a rematch (all ready starts the 3-2-1; round + 1).
    pub fn set_rematch_ready(&mut self, ready: bool) {
        self.me.set_rematch_ready(ready);
    }
    /// Host: from the results back to the lobby.
    pub fn to_lobby(&mut self) -> bool {
        self.host.as_mut().is_some_and(|h| h.to_lobby())
    }

    /// The run screen's driver (the same `RunDriver` as `LocalRun`).
    pub fn driver(&mut self) -> &mut dyn RunDriver {
        self
    }

    /// Leave the room (the host tells everyone first). Idempotent.
    pub fn leave(&mut self) {
        if self.left {
            return;
        }
        self.left = true;
        self.me.leave();
        if let Some(h) = self.host.as_mut() {
            h.shutdown();
        }
    }
}

/// The run screen drives an online match like a local one. `update`'s
/// `now_ms` is ignored: the session runs on its own clock (the host's world
/// and the loopback share it).
impl RunDriver for OnlineSession {
    fn update(&mut self, _now_ms: u64, input: InputFrame) {
        self.pump(input);
    }
    fn place(&mut self, tool: Tool, lane: u8, row: u32) -> u32 {
        self.me.place(tool, lane, row)
    }
    fn view(&self) -> Option<&FlockView> {
        self.me.view()
    }
    fn results(&self) -> Option<&ResultsView> {
        self.me.results()
    }
    fn set_paused(&mut self, _p: bool) {}
}

impl Drop for OnlineSession {
    fn drop(&mut self) {
        self.leave();
    }
}

// ─── The process-wide slot ──────────────────────────────────────────────────

static SLOT: Mutex<Option<OnlineSession>> = Mutex::new(None);

/// Make `s` the app's session; a session already there leaves its room.
pub fn put(s: OnlineSession) {
    let old = lock().replace(s);
    // The old room is left outside the lock.
    drop(old);
}

fn lock() -> std::sync::MutexGuard<'static, Option<OnlineSession>> {
    SLOT.lock().unwrap_or_else(|e| e.into_inner())
}

/// Run `f` on the session, if there is one (`None` also when called from
/// inside another `with`).
pub fn with<R>(f: impl FnOnce(&mut OnlineSession) -> R) -> Option<R> {
    let mut g = match SLOT.try_lock() {
        Ok(g) => g,
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return None,
    };
    g.as_mut().map(f)
}

/// Is there a session?
pub fn is_some() -> bool {
    match SLOT.try_lock() {
        Ok(g) => g.is_some(),
        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner().is_some(),
        Err(std::sync::TryLockError::WouldBlock) => true,
    }
}

/// Leave the room and empty the slot. Idempotent.
pub fn leave() {
    let s = lock().take();
    drop(s);
}

/// Take the session out of the slot (tests).
pub fn take() -> Option<OnlineSession> {
    lock().take()
}

/// Engine: drive the session if no screen did this frame.
pub fn pump_auto() {
    with(|s| s.pump_auto());
}

/// Engine: close the frame.
pub fn end_frame() {
    with(|s| s.end_frame());
}

/// Engine: the app went to (`true`) or came back from the background.
pub fn set_away(away: bool) -> Option<u64> {
    with(|s| s.set_away(away)).flatten()
}

// ─── Words ──────────────────────────────────────────────────────────────────

/// Why the player's link is gone, in words (None while it is up).
pub fn dead_text(phase: &ClientPhase) -> Option<String> {
    Some(match phase {
        ClientPhase::Failed(why) => friendly_failure(why),
        ClientPhase::Closed(why) => closed_text(why),
        ClientPhase::HostLeft => "The host left the game.".into(),
        ClientPhase::Disconnected(_) => "Lost the connection to the host.".into(),
        _ => return None,
    })
}

/// Strip the transport's " [Stage]" suffix from a failure text.
pub fn friendly_failure(why: &str) -> String {
    let base = why.split(" [").next().unwrap_or(why).trim();
    if base.is_empty() {
        "Couldn't join that game.".into()
    } else {
        base.to_string()
    }
}

/// What the host's close reasons mean to a player.
pub fn closed_text(why: &str) -> String {
    match why {
        "lobby full" => "Room full. Ask the host to make space, or start your own game.".into(),
        "kicked" => "The host removed you from the game.".into(),
        "left" => "You left the game.".into(),
        "not admitted" => "The host couldn't let you in.".into(),
        w if w.starts_with("version mismatch") => {
            "The host is running a different version of the game. Both of you need the latest version.".into()
        }
        "match in progress" => "That game has already started. Wait for the next round.".into(),
        // Bluetooth: the host's answer to the Accept prompt (or none).
        "The host said no." | "The host didn't answer." => why.to_string(),
        "Too many people are waiting to join." => "Too many people are waiting to join. Try again in a moment.".into(),
        w => {
            let w: String = w.chars().filter(|c| !c.is_control()).take(60).collect();
            format!("The host closed the game ({w}).")
        }
    }
}

/// Ping quality for the dot: 0 good, 1 ok, 2 poor.
pub fn quality(rtt_ms: Option<u32>) -> u8 {
    match rtt_ms {
        Some(r) if r <= 70 => 0,
        Some(r) if r <= 140 => 1,
        Some(_) => 2,
        None => 0,
    }
}
