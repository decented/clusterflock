//! Loopback-hub tests (Stampede's rig): lobby, whole matches under latency,
//! jitter and loss, prediction accuracy, placements over the wire,
//! disconnects, rematch, the Bluetooth Accept gate. Virtual clock
//! throughout; no sockets.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use flock_core::{
    course::Cell, Command, EndReason, Event, InputFrame, ObjId, Phase, PlayerId, Reject, Role, Row, Tool,
};

use crate::client::{ClientPhase, ClientSession};
use crate::door::{HostLink, Radio};
use crate::host::{HostAuthority, HostConfig, HostPhase, PENDING_MS, SILENCE_MS};
use crate::proto::{LobbyPhase, SeatPath, ToHost};
use crate::transport::{HostNetEvent, HostTransport, LinkPath, LoopClock, LoopbackConfig, LoopbackHub, PeerKey};
use crate::view::RunDriver;

/// Who drives a test client's runner.
#[derive(Clone)]
enum Pilot {
    Idle,
    /// `(at host-ish ms since the client entered Running, command)`, each
    /// fired once (a new `seq`).
    Script(Vec<(u64, Command)>),
    /// After `start` ms, steer toward the centre lane every second.
    Centre { start: u64 },
}

struct TClient {
    s: ClientSession,
    /// Added to the global time: every client has its own clock.
    skew: u64,
    pilot: Pilot,
    frame: InputFrame,
    started_at: Option<u64>,
    fired: usize,
    /// Stop updating (simulates a frozen/vanished phone).
    silent: bool,
    /// Every event this client's view reported, in order.
    events: Vec<Event>,
    place_results: Vec<(u32, flock_core::PlaceResult)>,
}

struct Net {
    t: Arc<AtomicU64>,
    hub: LoopbackHub,
    host: HostAuthority,
    clients: Vec<TClient>,
    /// (player, tick) -> host body z/x after every host tick.
    history: HashMap<(u8, u64), (f32, f64)>,
    last_tick: u64,
}

fn link(latency_ms: u32, jitter_ms: u32, loss: f32) -> LoopbackConfig {
    LoopbackConfig { latency_ms, jitter_ms, lossy_loss: loss }
}

fn cfg(empty: bool) -> HostConfig {
    let mut c = HostConfig::new(99);
    c.tuning.debug_empty_course = empty;
    c
}

impl Net {
    fn new(cfg: HostConfig) -> Net {
        let (clock, t) = LoopClock::manual();
        t.store(10_000, Ordering::Relaxed);
        let hub = LoopbackHub::new(clock, 42);
        let host = HostAuthority::new(Box::new(hub.host()), cfg);
        Net { t, hub, host, clients: Vec::new(), history: HashMap::new(), last_tick: 0 }
    }

    fn add(&mut self, s: ClientSession, pilot: Pilot) -> usize {
        let skew = 1_000_000 + 77_777 * self.clients.len() as u64;
        self.clients.push(TClient {
            s,
            skew,
            pilot,
            frame: InputFrame::default(),
            started_at: None,
            fired: 0,
            silent: false,
            events: Vec::new(),
            place_results: Vec::new(),
        });
        self.clients.len() - 1
    }

    fn join(&mut self, name: &str, link: LoopbackConfig, pilot: Pilot) -> usize {
        let k = self.clients.len();
        let c = self.hub.connect_as(link, LinkPath::Direct, Some(format!("npub1test{k}")));
        self.add(ClientSession::new(Box::new(c), name, k as u8), pilot)
    }

    fn now(&self) -> u64 {
        self.t.load(Ordering::Relaxed)
    }

    fn step(&mut self, ms: u64) {
        let now = self.t.fetch_add(ms, Ordering::Relaxed) + ms;
        self.host.update(now);
        if let Some(w) = self.host.world() {
            if w.tick() != self.last_tick {
                for r in w.runners() {
                    self.history.insert((r.id.0, w.tick()), (r.pos.x, r.pos.z));
                }
                self.last_tick = w.tick();
            }
        }
        for c in &mut self.clients {
            if c.silent {
                continue;
            }
            let local = now + c.skew;
            if c.started_at.is_none() && *c.s.phase() == ClientPhase::Running {
                c.started_at = Some(local);
            }
            if let (Pilot::Script(cmds), Some(t0)) = (&c.pilot, c.started_at) {
                if let Some(&(at, cmd)) = cmds.get(c.fired) {
                    if local.saturating_sub(t0) >= at {
                        c.frame = InputFrame { seq: c.frame.seq.wrapping_add(1), cmd: Some(cmd) };
                        c.fired += 1;
                    }
                }
            }
            if let (Pilot::Centre { start }, Some(t0)) = (&c.pilot, c.started_at) {
                let el = local.saturating_sub(t0);
                if el >= *start && (el - start) / 1000 >= c.fired as u64 {
                    c.fired += 1;
                    if let Some(x) = c.s.predicted().map(|r| r.pos.x) {
                        let cmd = if x < -0.5 { Some(Command::Right) } else if x > 0.5 { Some(Command::Left) } else { None };
                        if let Some(cmd) = cmd {
                            c.frame = InputFrame { seq: c.frame.seq.wrapping_add(1), cmd: Some(cmd) };
                        }
                    }
                }
            }
            c.s.update(local, c.frame);
            if let Some(v) = c.s.view() {
                c.events.extend(v.events.iter().cloned());
                c.place_results.extend(v.place_results.iter().cloned());
            }
        }
    }

    fn run_for(&mut self, ms: u64) {
        let mut left = ms;
        while left > 0 {
            let d = left.min(16);
            self.step(d);
            left -= d;
        }
    }

    fn run_until(&mut self, max_ms: u64, mut f: impl FnMut(&Net) -> bool) -> bool {
        let mut t = 0;
        while t < max_ms {
            if f(self) {
                return true;
            }
            self.step(16);
            t += 16;
        }
        f(self)
    }

    fn ready_all(&mut self) {
        for c in &mut self.clients {
            c.s.set_ready(true);
        }
    }

    fn all_running(&self) -> bool {
        self.clients.iter().all(|c| *c.s.phase() == ClientPhase::Running && c.s.view().is_some())
    }

    fn player_of(&self, k: usize) -> PlayerId {
        self.clients[k].s.view().expect("running").local
    }

    /// Lobby with `n` clients (latency `lat`), bots, all ready, into the match.
    fn started(n: usize, bots: usize, lat: LoopbackConfig, empty: bool) -> Net {
        let mut net = Net::new(cfg(empty));
        for i in 0..n {
            net.join(&format!("P{i}"), lat, Pilot::Idle);
        }
        for _ in 0..bots {
            assert!(net.host.add_bot());
        }
        assert!(net.run_until(3000, |n| n.clients.iter().all(|c| *c.s.phase() == ClientPhase::Lobby)));
        net.ready_all();
        assert!(net.run_until(3000, |n| n.all_running()), "match started");
        net
    }

    /// End the match: eliminate every runner on the host.
    fn eliminate_all(&mut self) {
        let w = self.host.world_mut().expect("world");
        let ids: Vec<PlayerId> = w.players().iter().map(|p| p.id()).collect();
        for id in ids {
            w.force_eliminate(id);
        }
    }
}

fn results_identical(net: &Net) {
    let host = net.host.results().expect("host results").clone();
    for (k, c) in net.clients.iter().enumerate() {
        assert_eq!(c.s.results(), Some(&host), "client {k} results differ");
        assert_eq!(*c.s.phase(), ClientPhase::Results);
    }
}

/// Rows `idx..idx+3` get a power-up in lanes 0, 1, 2 and row `idx+5` a hay
/// wall, just past what the host has generated (so they are streamed like
/// any row); every runner is protected with its free hit (and spare
/// feathers), so the first obstacle it meets is a `Swagger`. Returns `idx`.
fn plant_power_ups_and_a_wall(net: &mut Net) -> u32 {
    use flock_core::course::{PowerKind, SolidKind};
    let w = net.host.world_mut().unwrap();
    let idx = w.course().last_index().unwrap() + 1;
    for (k, kind) in [PowerKind::Magnet, PowerKind::Doubler, PowerKind::Shield].into_iter().enumerate() {
        let mut row = Row::empty(idx + k as u32);
        row.cells[k] = Cell::EMPTY.with_power(Some(kind));
        w.debug_course_mut().insert_row(idx + k as u32, row);
    }
    for k in 3..5 {
        w.debug_course_mut().insert_row(idx + k, Row::empty(idx + k));
    }
    let mut wall = Row::empty(idx + 5);
    for c in wall.cells.iter_mut() {
        *c = Cell::EMPTY.with_solid(Some(SolidKind::Hay));
    }
    w.debug_course_mut().insert_row(idx + 5, wall);
    let now = w.tick();
    let ids: Vec<PlayerId> = w.runners().map(|r| r.id).collect();
    for id in ids {
        let r = w.debug_runner_mut(id).unwrap();
        r.feathers = 50;
        r.protected_until = Some(now + 1_000_000);
        r.protect_since = Some(now);
        r.free_hit = true;
    }
    idx
}

/// The planted power-up rows the host saw taken (call once the pack is past
/// them, before the rows are released).
fn host_power_rows_taken(net: &Net, idx: u32) -> Vec<u32> {
    let hw = net.host.world().unwrap();
    let taken: Vec<u32> = (idx..idx + 3).filter(|i| hw.course().row(*i).is_some_and(|r| r.power_taken())).collect();
    assert!(!taken.is_empty(), "somebody took a power-up");
    assert!(hw.runners().any(|r| r.magnet_until > 0 || r.double_until > 0 || r.shield_until > 0));
    taken
}

/// Every client saw each host pickup of a planted power-up once, its remote
/// course agrees, and it saw at least one `Swagger`, none twice.
fn power_ups_and_swagger_seen_once(net: &Net, idx: u32, host_taken: &[u32]) {
    for (k, c) in net.clients.iter().enumerate() {
        let taken: Vec<ObjId> = c
            .events
            .iter()
            .filter_map(|e| if let Event::PowerTaken { obj, .. } = e { Some(*obj) } else { None })
            .filter(|o| (idx..idx + 3).contains(&(o.0 / 32)))
            .collect();
        let mut uniq = taken.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), taken.len(), "client {k}: a power-up reported twice");
        assert_eq!(taken.len(), host_taken.len(), "client {k} saw every pickup");
        let v = c.s.view().unwrap();
        for i in host_taken {
            if let Some(row) = v.world.course().row(*i) {
                assert!(row.power_taken(), "client {k}: row {i} power-up still on the remote course");
                assert!(!v.world.course().row_objects(*i).iter().any(|o| matches!(o.kind, flock_core::ObjKind::Power { .. })));
            }
        }
        let swags: Vec<(PlayerId, ObjId)> =
            c.events.iter().filter_map(|e| if let Event::Swagger { player, obj } = e { Some((*player, *obj)) } else { None }).collect();
        assert!(!swags.is_empty(), "client {k} saw a swagger");
        let mut u = swags.clone();
        u.sort();
        u.dedup();
        assert_eq!(u.len(), swags.len(), "client {k}: a swagger reported twice");
    }
}

fn full_round(n: usize, bots: usize) {
    let mut net = Net::started(n, bots, link(20, 5, 0.02), false);
    assert_eq!(net.host.world().unwrap().players().len(), n + bots);
    let idx = plant_power_ups_and_a_wall(&mut net);
    let sp = net.host.world().unwrap().course().row_spacing() as f64;
    assert!(
        net.run_until(60_000, |n| n.host.world().unwrap().trailing_z().unwrap_or(0.0) > (idx + 3) as f64 * sp + 1.0),
        "the pack passed the power-ups"
    );
    let host_taken = host_power_rows_taken(&net, idx);
    assert!(
        net.run_until(30_000, |n| n.host.world().unwrap().trailing_z().unwrap_or(0.0) > (idx + 7) as f64 * sp),
        "the pack passed the wall"
    );
    net.run_for(1000);
    assert_eq!(net.host.world().unwrap().phase(), Phase::Running);
    power_ups_and_swagger_seen_once(&net, idx, &host_taken);
    for c in &net.clients {
        let v = c.s.view().unwrap();
        assert_eq!(v.phase, Phase::Running);
        assert_eq!(v.names.len(), n + bots);
        assert!(v.world.course().len() > 10, "rows streamed");
        assert!(v.world.course().is_remote());
        // Exactly one GO, each event once.
        assert_eq!(c.events.iter().filter(|e| matches!(e, Event::Go)).count(), 1);
    }
    net.eliminate_all();
    assert!(net.run_until(3000, |n| n.clients.iter().all(|c| c.s.results().is_some())), "results reached everyone");
    results_identical(&net);
    let res = net.host.results().unwrap();
    assert_eq!(res.rows.len(), n + bots);
    assert_eq!(res.round, 0);
    // The shadow worlds agree with the host on roles at the end.
    let hw = net.host.world().unwrap();
    for c in &net.clients {
        let v = c.s.view().unwrap();
        assert!(v.over);
        for p in hw.players() {
            let mine = v.world.player(p.id()).unwrap();
            assert_eq!(mine.role.is_ghost(), p.role.is_ghost(), "role of {:?}", p.id());
        }
    }
}

#[test]
fn two_clients_play_a_round_with_identical_results() {
    full_round(2, 0);
}

#[test]
fn four_clients_and_a_bot_play_a_round_with_identical_results() {
    full_round(4, 1);
}

#[test]
fn six_clients_play_a_round_with_identical_results() {
    full_round(6, 0);
}

#[test]
fn lobby_colours_are_unique_and_bots_fill_seats() {
    let mut net = Net::new(cfg(false));
    // Both ask for colour 0... (join() picks k; force a clash)
    let a = net.hub.connect_as(LoopbackConfig::PERFECT, LinkPath::Direct, Some("npub1a".into()));
    let b = net.hub.connect_as(LoopbackConfig::PERFECT, LinkPath::Direct, Some("npub1b".into()));
    net.add(ClientSession::new(Box::new(a), "A", 3), Pilot::Idle);
    net.add(ClientSession::new(Box::new(b), "B", 3), Pilot::Idle);
    assert!(net.run_until(1000, |n| n.clients.iter().all(|c| *c.s.phase() == ClientPhase::Lobby)));
    for _ in 0..10 {
        net.host.add_bot();
    }
    net.run_for(100);
    let l = net.clients[0].s.lobby().unwrap().clone();
    assert_eq!(l.seats.len(), 6, "two humans + four bots: {l:?}");
    let mut colours: Vec<u8> = l.seats.iter().map(|s| s.colour).collect();
    colours.sort();
    colours.dedup();
    assert_eq!(colours.len(), 6, "colours unique: {:?}", l.seats.iter().map(|s| (s.id, s.colour, s.bot)).collect::<Vec<_>>());
    assert_eq!(l.seats.iter().filter(|s| !s.bot).map(|s| s.persona.clone()).collect::<Vec<_>>().len(), 2);
    assert!(l.seats.iter().filter(|s| !s.bot).all(|s| s.persona.is_some() && s.path == SeatPath::Network));
    // A human can take a bot's colour; the bot moves.
    let bot_colour = l.seats.iter().find(|s| s.bot).unwrap().colour;
    net.clients[0].s.set_colour(bot_colour);
    net.run_for(100);
    let l = net.clients[1].s.lobby().unwrap().clone();
    assert_eq!(l.seat(net.clients[0].s.my_seat().unwrap()).unwrap().colour, bot_colour);
    let mut colours: Vec<u8> = l.seats.iter().map(|s| s.colour).collect();
    colours.sort();
    colours.dedup();
    assert_eq!(colours.len(), 6);
    // ...but not another human's.
    let other = l.seat(net.clients[1].s.my_seat().unwrap()).unwrap().colour;
    net.clients[0].s.set_colour(other);
    net.run_for(100);
    let l = net.clients[0].s.lobby().unwrap();
    assert_ne!(l.seat(net.clients[0].s.my_seat().unwrap()).unwrap().colour, other);
    // A seventh person displaces a bot; remove_bot works.
    net.join("C", LoopbackConfig::PERFECT, Pilot::Idle);
    net.run_for(100);
    assert_eq!(net.host.bot_count(), 3);
    assert!(net.host.remove_bot(None));
    assert_eq!(net.host.bot_count(), 2);
}

/// All six bunch into the centre lane over the wire: the host keeps them
/// simulated, and every client's picture stays finite and close to the host.
#[test]
fn six_in_one_lane_over_the_wire_stays_coherent() {
    let mut net = Net::new(cfg(true));
    for i in 0..6 {
        net.join(&format!("P{i}"), link(30, 10, 0.03), Pilot::Idle);
    }
    assert!(net.run_until(3000, |n| n.clients.iter().all(|c| *c.s.phase() == ClientPhase::Lobby)));
    net.ready_all();
    assert!(net.run_until(3000, |n| n.all_running()));
    // Lane 0 runners go right, lane 2 runners go left, after GO.
    let go_ms = 4500;
    for c in &mut net.clients {
        c.pilot = Pilot::Centre { start: go_ms };
    }
    net.run_for(go_ms + 20_000);
    let w = net.host.world().unwrap();
    assert_eq!(w.living_count(), 6, "nobody removed");
    for r in w.runners() {
        assert!(r.pos.x.is_finite() && r.pos.z.is_finite());
        assert!(r.pos.x.abs() < 1.5, "all in the centre lane: {:?}", r.pos);
    }
    for (k, c) in net.clients.iter().enumerate() {
        let v = c.s.view().unwrap();
        assert_eq!(v.world.runners().count(), 6, "client {k} draws all six");
        for r in v.world.runners() {
            assert!(r.pos.x.is_finite() && r.pos.y.is_finite() && r.pos.z.is_finite());
            let host_z = w.runner(r.id).unwrap().pos.z;
            // Others are drawn ~100-350 ms behind (a few metres at 10 m/s).
            assert!((r.pos.z - host_z).abs() < 6.0, "client {k}: {:?} z {} vs host {host_z}", r.id, r.pos.z);
        }
        assert_eq!(c.s.stats().hard_snaps, 0, "client {k} snapped");
        assert_eq!(c.s.stats().shadow_errors, 0);
    }
}

/// A coin line in front of a bunched pack: each coin is awarded once and
/// every client sees exactly one `CoinTaken` per coin.
#[test]
fn contested_coins_are_awarded_once_and_seen_once_by_all() {
    let mut net = Net::new(cfg(true));
    for i in 0..4 {
        net.join(&format!("P{i}"), link(25, 5, 0.0), Pilot::Idle);
    }
    assert!(net.run_until(3000, |n| n.clients.iter().all(|c| *c.s.phase() == ClientPhase::Lobby)));
    net.ready_all();
    assert!(net.run_until(3000, |n| n.all_running()));
    // Everyone into the centre lane.
    for k in 0..4 {
        let p = net.player_of(k);
        let lane = net.host.world().unwrap().player(p).unwrap().runner.target_lane;
        net.clients[k].pilot = match lane {
            0 => Pilot::Script(vec![(4500, Command::Right)]),
            2 => Pilot::Script(vec![(4500, Command::Left)]),
            _ => Pilot::Idle,
        };
    }
    // A coin row just past what was generated, in the centre lane.
    let idx = {
        let w = net.host.world_mut().unwrap();
        let idx = w.course().last_index().unwrap() + 1;
        let mut row = Row::empty(idx);
        row.cells[1] = Cell::EMPTY.with_coins(flock_core::course::CoinPattern::Line);
        w.debug_course_mut().insert_row(idx, row);
        idx
    };
    let z = idx as f64 * net.host.world().unwrap().course().row_spacing() as f64;
    assert!(net.run_until(120_000, |n| n.host.world().unwrap().trailing_z().unwrap_or(0.0) > z + 5.0), "pack passed the coins");
    net.run_for(1000);
    let coin_ids: Vec<ObjId> = (2..6).map(|s| ObjId(idx * 32 + 8 + s)).collect();
    let w = net.host.world().unwrap();
    let total: u32 = w.players().iter().map(|p| p.runner.coins).sum();
    assert!((1..=4).contains(&total), "coins awarded {total}");
    for (k, c) in net.clients.iter().enumerate() {
        let taken: Vec<ObjId> =
            c.events.iter().filter_map(|e| if let Event::CoinTaken { obj, .. } = e { Some(*obj) } else { None }).collect();
        let mut uniq = taken.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(uniq.len(), taken.len(), "client {k}: a coin reported twice");
        assert_eq!(taken.len() as u32, total, "client {k} saw every award once");
        assert!(taken.iter().all(|o| coin_ids.contains(o)));
        // The remote course agrees: those coins are gone.
        let v = c.s.view().unwrap();
        if let Some(row) = v.world.course().row(idx) {
            assert_eq!(row.coins_taken.count_ones(), total, "client {k} remote course");
        }
        // And the scoreboard (via snapshots) matches the host.
        let mine: u32 = v.world.players().iter().map(|p| p.runner.coins).sum();
        assert_eq!(mine, total);
    }
}

/// A rolling barrel: the host releases it when the pack is near, the event
/// crosses the wire, and every client's remote course places it exactly where
/// the host's formula puts it at the client's own clock.
#[test]
fn barrel_position_matches_host_and_clients_over_a_run() {
    for kind in [flock_core::course::SolidKind::BarrelToward, flock_core::course::SolidKind::BarrelAway] {
        let mut net = Net::new(cfg(true));
        for i in 0..3 {
            net.join(&format!("P{i}"), link(25, 5, 0.0), Pilot::Idle);
        }
        assert!(net.run_until(3000, |n| n.clients.iter().all(|c| *c.s.phase() == ClientPhase::Lobby)));
        net.ready_all();
        assert!(net.run_until(3000, |n| n.all_running()));
        let idx = {
            let w = net.host.world_mut().unwrap();
            let idx = w.course().last_index().unwrap() + 1;
            let mut row = Row::empty(idx);
            // A lane nobody is steering out of: the barrel is the only thing on the course.
            row.cells[1] = Cell::EMPTY.with_solid(Some(kind));
            w.debug_course_mut().insert_row(idx, row);
            idx
        };
        let home = idx as f64 * net.host.world().unwrap().course().row_spacing() as f64;
        let (mut moving, mut compared) = (0, 0);
        let mut released = [false; 3];
        for _ in 0..1500 {
            net.run_for(100);
            let hc = net.host.world().unwrap().course().clone();
            let Some(host_row) = hc.row(idx) else { break };
            for (k, c) in net.clients.iter().enumerate() {
                let cc = c.s.view().unwrap().world.course();
                let Some(crow) = cc.row(idx) else { continue };
                // The client's clock follows the host's (it draws a little in the past).
                let lag = hc.clock() as i64 - cc.clock() as i64;
                assert!((-3..=60).contains(&lag), "client {k}: clock {} vs host {}", cc.clock(), hc.clock());
                // Evaluate the host's row at the client's clock: identical data, identical z.
                let mut at_client = hc.clone();
                at_client.set_clock(cc.clock());
                let want = at_client.row_objects(idx).into_iter().find(|o| o.kind == flock_core::ObjKind::Barrel).unwrap();
                let got = cc.row_objects(idx).into_iter().find(|o| o.kind == flock_core::ObjKind::Barrel).unwrap();
                if crow.release != 0 {
                    released[k] = true;
                    assert_eq!(crow.release, host_row.release, "client {k}: release tick");
                    assert_eq!(got, want, "client {k}: barrel at clock {}", cc.clock());
                    compared += 1;
                    if (got.z - home).abs() > 0.5 {
                        moving += 1;
                    }
                } else {
                    assert_eq!(got.z, home, "client {k}: unreleased barrel stays home");
                }
            }
            if net.host.world().unwrap().trailing_z().unwrap_or(0.0) > home + 60.0 {
                break;
            }
        }
        assert!(released.iter().all(|&r| r), "{kind:?}: every client learned of the release");
        assert!(compared > 30 && moving > 3, "{kind:?}: compared {compared}, moving {moving}");
    }
}

fn valid_cell(w: &flock_core::World, p: PlayerId, tool: Tool) -> (u8, u32) {
    for row in w.placement_band().iter() {
        for lane in 0..3 {
            if w.check_placement(p, tool, lane, row).is_ok() {
                return (lane, row);
            }
        }
    }
    panic!("no valid cell in {:?}", w.placement_band());
}

/// The same `Place` sent twice, the copy late: one hazard, one charge, the
/// same answer twice.
#[test]
fn duplicate_and_delayed_place_gives_one_hazard_and_one_charge() {
    let mut net = Net::started(2, 1, link(40, 30, 0.0), false);
    net.run_for(4500);
    let ghost = net.player_of(0);
    {
        let w = net.host.world_mut().unwrap();
        w.force_eliminate(ghost);
    }
    net.run_for(200);
    {
        let w = net.host.world_mut().unwrap();
        w.debug_set_energy(ghost, 10.0);
    }
    let energy0 = net.host.world().unwrap().ghost_energy(ghost).unwrap();
    let (lane, row) = valid_cell(net.host.world().unwrap(), ghost, Tool::Mud);
    let run = net.clients[0].s.run_no().unwrap();
    let m = ToHost::Place { run, req: 77, tool: Tool::Mud, lane, row };
    net.clients[0].s.send_raw(&m);
    net.run_for(500);
    net.clients[0].s.send_raw(&m);
    net.run_for(700);
    let w = net.host.world().unwrap();
    assert_eq!(w.player(ghost).unwrap().placements, 1, "one hazard");
    let cost = w.tuning().cost.mud;
    let e1 = w.ghost_energy(ghost).unwrap();
    // One charge (regeneration added meanwhile, capped).
    assert!(e1 > energy0 - 2.0 * cost + 0.01, "charged twice? {energy0} -> {e1} (cost {cost})");
    let answers: Vec<_> = net.clients[0].place_results.iter().filter(|(r, _)| *r == 77).cloned().collect();
    assert_eq!(answers.len(), 2);
    assert_eq!(answers[0], answers[1], "a retry gets the original answer");
    assert!(answers[0].1.is_ok());
    for c in &net.clients {
        let placed = c.events.iter().filter(|e| matches!(e, Event::Placed { by, .. } if *by == ghost)).count();
        assert_eq!(placed, 1, "everyone sees one placement");
        // ...and it is on their remote course.
        let v = c.s.view().unwrap();
        assert!(v.world.course().placed().iter().any(|o| o.by == Some(ghost)));
    }
    // The RunDriver path: the ghost's own placement answer arrives once.
    let req = net.clients[0].s.place(Tool::Coins, lane, row);
    net.run_for(600);
    assert_eq!(net.clients[0].place_results.iter().filter(|(r, _)| *r == req).count(), 1);
}

/// A ghost's runner commands do nothing; a runner cannot place.
#[test]
fn ghost_cannot_run_and_runner_cannot_place() {
    let mut net = Net::started(2, 1, link(20, 5, 0.0), false);
    net.run_for(4000);
    let ghost = net.player_of(0);
    net.host.world_mut().unwrap().force_eliminate(ghost);
    net.run_for(200);
    let frozen = net.host.world().unwrap().player(ghost).unwrap().runner.pos;
    net.clients[0].pilot = Pilot::Script((0..20).map(|k| (k * 100, if k % 2 == 0 { Command::Left } else { Command::Jump })).collect());
    net.clients[0].started_at = Some(net.now() + net.clients[0].skew);
    net.clients[0].fired = 0;
    net.run_for(2500);
    let w = net.host.world().unwrap();
    let p = w.player(ghost).unwrap();
    assert!(p.role.is_ghost());
    assert_eq!(p.runner.pos, frozen, "a ghost's body never moves");
    assert!(p.runner.stats.ignored_cmds > 0, "commands arrived and were ignored");
    // A runner asks to place: NotGhost, nothing placed.
    let runner = net.player_of(1);
    assert!(w.player(runner).unwrap().role.is_runner());
    let band = w.placement_band();
    let req = net.clients[1].s.place(Tool::Hay, 1, band.first);
    net.run_for(600);
    let ans: Vec<_> = net.clients[1].place_results.iter().filter(|(r, _)| *r == req).collect();
    assert_eq!(ans.len(), 1);
    assert_eq!(ans[0].1, Err(Reject::NotGhost));
    assert_eq!(net.host.world().unwrap().player(runner).unwrap().placements, 0);
}

#[test]
fn host_leaving_closes_every_client() {
    let mut net = Net::started(3, 0, link(20, 0, 0.0), false);
    net.run_for(2000);
    net.host.shutdown();
    net.run_for(300);
    for c in &net.clients {
        assert_eq!(*c.s.phase(), ClientPhase::HostLeft);
        let v = c.s.view().unwrap();
        assert!(v.over, "the run screen sees the match end");
    }
}

#[test]
fn a_client_silent_for_six_seconds_is_disconnected_and_its_body_removed() {
    let mut net = Net::started(3, 0, link(20, 0, 0.0), false);
    net.run_for(4000);
    let gone = net.player_of(2);
    net.clients[2].silent = true;
    net.run_for(SILENCE_MS + 1000);
    let w = net.host.world().unwrap();
    let p = w.player(gone).unwrap();
    assert!(matches!(p.role, Role::Disconnected));
    assert!(w.runner(gone).is_none(), "no permanent blocker");
    assert!(w.runners().all(|r| r.id != gone));
    for c in &net.clients[..2] {
        assert!(c.s.notice().is_some_and(|n| n.contains("left")), "notice shown");
        let v = c.s.view().unwrap();
        assert!(v.world.runners().all(|r| r.id != gone), "clients drop the body too");
        assert!(c.events.iter().any(|e| matches!(e, Event::Disconnected { player } if *player == gone)));
    }
}

#[test]
fn fewer_than_two_players_ends_with_not_enough_players() {
    let mut net = Net::started(2, 0, link(20, 0, 0.0), false);
    net.run_for(4000);
    net.clients[1].s.leave();
    assert!(net.run_until(2000, |n| n.clients[0].s.results().is_some()));
    let res = net.clients[0].s.results().unwrap();
    assert_eq!(res.end_reason, Some(EndReason::NotEnoughPlayers));
    let leaver = res.rows.last().unwrap();
    assert!(leaver.disconnected, "the leaver is ranked last");
}

#[test]
fn rematch_increments_the_round_and_rotates_the_grid() {
    let mut net = Net::started(2, 0, link(20, 0, 0.0), false);
    let slot = |net: &Net, p: PlayerId| {
        let r = &net.host.world().unwrap().player(p).unwrap().runner;
        (r.target_lane, r.pos.z.round() as i64)
    };
    let p0 = net.player_of(0);
    let before = slot(&net, p0);
    net.run_for(4000);
    net.eliminate_all();
    assert!(net.run_until(3000, |n| n.clients.iter().all(|c| c.s.results().is_some())));
    results_identical(&net);
    for c in &mut net.clients {
        c.s.set_rematch_ready(true);
    }
    assert!(net.run_until(1000, |n| n.clients.iter().all(|c| c.s.rematch_ms_left(n.now() + c.skew).is_some())));
    assert!(net.run_until(5000, |n| n.host.phase() == HostPhase::Running && n.host.world().unwrap().phase() != Phase::Over));
    assert!(net.run_until(2000, |n| n.all_running()));
    assert_eq!(net.host.world().unwrap().config().round, 1);
    assert_eq!(net.host.rounds(), 2);
    let p0 = net.player_of(0);
    assert_ne!(slot(&net, p0), before, "player 0 moved on the grid");
    for c in &net.clients {
        assert_eq!(c.s.view().unwrap().round, 1);
        assert!(c.s.results().is_none());
    }
    assert_eq!(net.clients[0].s.lobby().unwrap().round, 2);
    // to_lobby returns everyone to the lobby.
    net.run_for(500);
    net.host.to_lobby();
    net.run_for(300);
    for c in &net.clients {
        assert_eq!(*c.s.phase(), ClientPhase::Lobby);
        assert_eq!(c.s.lobby().unwrap().phase, LobbyPhase::Lobby);
    }
}

/// Own-runner prediction under 70 ms latency, 25 ms jitter and 5 % loss with
/// lane changes and jumps: the live prediction stays within tolerance of the
/// host's body at the same tick, and nothing hard-snaps.
#[test]
fn prediction_under_latency_jitter_and_loss_stays_within_tolerance() {
    let mut net = Net::new(cfg(true));
    let script: Vec<(u64, Command)> = (0..30)
        .map(|k| {
            let at = 3600 + k * 700;
            let c = match k % 6 {
                0 => Command::Right,
                1 => Command::Jump,
                2 => Command::Left,
                3 => Command::Down,
                4 => Command::JumpRight,
                _ => Command::DiveLeft,
            };
            (at, c)
        })
        .collect();
    net.join("P", link(70, 25, 0.05), Pilot::Script(script));
    net.join("Q", link(70, 25, 0.05), Pilot::Idle);
    assert!(net.run_until(3000, |n| n.clients.iter().all(|c| *c.s.phase() == ClientPhase::Lobby)));
    net.ready_all();
    assert!(net.run_until(3000, |n| n.all_running()));
    let me = net.player_of(0);
    // Keep Q out of P's way: P weaves 1 <-> 2, Q must be in lane 0.
    {
        let w = net.host.world_mut().unwrap();
        let q = w.players().iter().map(|p| p.id()).find(|&id| id != me).unwrap();
        let t = w.tuning().clone();
        let r = w.debug_runner_mut(q).unwrap();
        r.pos.x = t.lane_x(0);
        r.settle_lane = 0;
        r.target_lane = 0;
        r.pos.z -= 6.0;
        let r = w.debug_runner_mut(me).unwrap();
        r.pos.x = t.lane_x(1);
        r.settle_lane = 1;
        r.target_lane = 1;
    }
    net.run_for(25_000);
    let s = net.clients[0].s.stats();
    assert_eq!(s.hard_snaps, 0, "no hard snap");
    assert!(s.snapshots > 500);
    // The reconciliation error (prediction vs host after replay) stays small.
    assert!(s.max_correction_m < 1.0, "max correction {} m", s.max_correction_m);
    // And the drawn body is close to the host's at the end.
    let w = net.host.world().unwrap();
    let host = w.runner(me).unwrap();
    let mine = net.clients[0].s.view().unwrap().world.runner(me).unwrap().clone();
    assert!((mine.pos.x - host.pos.x).abs() < 1.5, "x {} vs {}", mine.pos.x, host.pos.x);
    // The client is ahead of the host by ~half the RTT + 2 ticks.
    let dz = mine.pos.z - host.pos.z;
    assert!(dz > -1.0 && dz < 3.0, "z lead {dz}");
    assert!(w.player(me).unwrap().runner.stats.jumps > 0, "jumps reached the host");
}

// ─── Admission: the Bluetooth Accept gate ──────────────────────────────────

#[test]
fn bluetooth_joiner_waits_for_accept_and_can_be_declined() {
    let mut net = Net::new(cfg(false));
    net.host.set_host_identity(Some("{host attestation}".into()), Some("npub1host".into()));
    net.join("Host", LoopbackConfig::PERFECT, Pilot::Idle);
    let bt = net.hub.connect_as(link(30, 0, 0.0), LinkPath::Bluetooth, Some("npub1bluetooth".into()));
    let k = net.add(ClientSession::new(Box::new(bt), "Radio", 2), Pilot::Idle);
    net.run_for(300);
    assert_eq!(*net.clients[k].s.phase(), ClientPhase::Waiting);
    let pend = net.host.pending_joiners().to_vec();
    assert_eq!(pend.len(), 1);
    assert_eq!(pend[0].persona.as_deref(), Some("npub1bluetooth"));
    assert_eq!(pend[0].name, "Radio");
    assert_eq!(net.host.human_count(), 1, "not seated");
    // While waiting its messages do nothing.
    net.clients[k].s.set_ready(true);
    net.clients[k].s.send_raw(&ToHost::Hello { proto: crate::PROTO_VERSION, name: "again".into(), colour: 0, hat: 255 });
    net.run_for(300);
    assert_eq!(net.host.human_count(), 1);
    assert_eq!(net.host.pending_joiners().len(), 1);
    assert!(net.clients[0].s.lobby().unwrap().seats.iter().all(|s| s.path != SeatPath::Bluetooth));
    // Accept: seated through the Bluetooth door, persona on the seat.
    assert!(net.host.accept_pending(pend[0].peer));
    net.run_for(300);
    assert_eq!(*net.clients[k].s.phase(), ClientPhase::Lobby);
    let l = net.clients[0].s.lobby().unwrap();
    let seat = l.seats.iter().find(|s| s.path == SeatPath::Bluetooth).expect("bt seat");
    assert_eq!(seat.persona.as_deref(), Some("npub1bluetooth"));
    // A second radio joiner is declined.
    let bt2 = net.hub.connect_as(link(30, 0, 0.0), LinkPath::Bluetooth, Some("npub1other".into()));
    let k2 = net.add(ClientSession::new(Box::new(bt2), "Nope", 1), Pilot::Idle);
    net.run_for(300);
    let p2 = net.host.pending_joiners()[0].peer;
    assert!(net.host.decline_pending(p2));
    net.run_for(300);
    assert!(matches!(net.clients[k2].s.phase(), ClientPhase::Closed(_)), "{:?}", net.clients[k2].s.phase());
    // A third is never answered: turned away after PENDING_MS.
    let bt3 = net.hub.connect_as(link(30, 0, 0.0), LinkPath::Bluetooth, Some("npub1late".into()));
    let k3 = net.add(ClientSession::new(Box::new(bt3), "Late", 1), Pilot::Idle);
    net.run_for(PENDING_MS + 1000);
    assert!(matches!(net.clients[k3].s.phase(), ClientPhase::Closed(_)));
    assert!(net.host.pending_joiners().is_empty());
}

/// A transport that delivers a Hello from a peer it never reported as
/// `Joined` (a door bug): the host refuses it.
struct HelloWithoutJoin(Vec<HostNetEvent>, Vec<(PeerKey, Vec<u8>)>, bool);

impl HostTransport for HelloWithoutJoin {
    fn poll(&mut self) -> Option<HostNetEvent> {
        self.0.pop()
    }
    fn send_reliable(&mut self, peer: PeerKey, bytes: Vec<u8>) {
        self.1.push((peer, bytes));
    }
    fn send_lossy(&mut self, _: PeerKey, _: Vec<u8>) {}
    fn kick(&mut self, _: PeerKey) {
        self.2 = true;
    }
    fn set_open(&mut self, _: bool) {}
    fn rtt_ms(&self, _: PeerKey) -> Option<u32> {
        None
    }
    fn shutdown(&mut self) {}
}

/// A protocol-2 client (its Hello has no hat byte, so it cannot decode as
/// v3) is still told why it was refused.
#[test]
fn an_older_protocol_hello_is_refused_with_the_reason() {
    #[derive(serde::Serialize)]
    enum OldToHost {
        Hello { proto: u32, name: String, colour: u8 },
    }
    let bytes = crate::proto::encode(&OldToHost::Hello { proto: 2, name: "old".into(), colour: 1 });
    assert!(crate::proto::decode::<ToHost>(&bytes).is_none(), "a v2 Hello does not parse as v3");
    assert_eq!(crate::proto::hello_proto(&bytes), Some(2));
    struct Recorder(Vec<HostNetEvent>, Arc<std::sync::Mutex<Vec<(PeerKey, Vec<u8>)>>>);
    impl HostTransport for Recorder {
        fn poll(&mut self) -> Option<HostNetEvent> {
            self.0.pop()
        }
        fn send_reliable(&mut self, peer: PeerKey, bytes: Vec<u8>) {
            self.1.lock().unwrap().push((peer, bytes));
        }
        fn send_lossy(&mut self, _: PeerKey, _: Vec<u8>) {}
        fn kick(&mut self, _: PeerKey) {}
        fn set_open(&mut self, _: bool) {}
        fn rtt_ms(&self, _: PeerKey) -> Option<u32> {
            None
        }
        fn shutdown(&mut self) {}
    }
    let log = Arc::new(std::sync::Mutex::new(Vec::new()));
    let t = Recorder(vec![HostNetEvent::Reliable { peer: 5, bytes }], log.clone());
    let mut h = HostAuthority::new(Box::new(t), cfg(false));
    h.update(1);
    assert_eq!(h.human_count(), 0);
    let sent = log.lock().unwrap().clone();
    let closed: Vec<String> = sent
        .iter()
        .filter(|(p, _)| *p == 5)
        .filter_map(|(_, b)| match crate::proto::decode::<crate::proto::ToClient>(b) {
            Some(crate::proto::ToClient::Closed(why)) => Some(why),
            _ => None,
        })
        .collect();
    assert_eq!(closed.len(), 1, "{closed:?}");
    assert!(closed[0].contains("version"), "{}", closed[0]);
}

#[test]
fn a_hello_without_an_admitted_link_is_refused() {
    let hello = crate::proto::encode(&ToHost::Hello { proto: crate::PROTO_VERSION, name: "x".into(), colour: 0, hat: 255 });
    let t = HelloWithoutJoin(vec![HostNetEvent::Reliable { peer: 5, bytes: hello }], Vec::new(), false);
    let mut h = HostAuthority::new(Box::new(t), cfg(false));
    h.update(1);
    assert_eq!(h.human_count(), 0);
    assert!(h.pending_joiners().is_empty());
}

struct NoRadio;
impl Radio for NoRadio {
    fn stop_advertising(&self) {}
    fn advertising_error(&self) -> Option<String> {
        None
    }
    fn room_label(&self) -> String {
        "1A2B".into()
    }
}

/// Two doors into one room: a UDP joiner that claims Bluetooth is seated as
/// network; a peer through the Bluetooth door (whatever it claims) waits for
/// Accept.
#[test]
fn host_link_doors_decide_the_path_and_the_bluetooth_door_needs_accept() {
    let (clock, t) = LoopClock::manual();
    t.store(10_000, Ordering::Relaxed);
    let lan_hub = LoopbackHub::new(clock.clone(), 1);
    let bt_hub = LoopbackHub::new(clock, 2);
    let link_ = HostLink::new(lan_hub.host());
    link_.start_bluetooth(Box::new(bt_hub.host()), Box::new(NoRadio)).unwrap();
    assert_eq!(link_.bluetooth(), crate::door::BtStatus::On { room: "1A2B".into() });
    let mut host = HostAuthority::new(Box::new(link_.clone()), cfg(false));
    host.set_host_identity(Some("{host attestation}".into()), Some("npub1host".into()));
    let liar = lan_hub.connect_as(LoopbackConfig::PERFECT, LinkPath::Bluetooth, Some("npub1liar".into()));
    let radio = bt_hub.connect_as(LoopbackConfig::PERFECT, LinkPath::Lan, Some("npub1radio".into()));
    let mut a = ClientSession::new(Box::new(liar), "Liar", 0);
    let mut b = ClientSession::new(Box::new(radio), "Radio", 1);
    for k in 1..50 {
        let now = 10_000 + k * 16;
        t.store(now, Ordering::Relaxed);
        host.update(now);
        a.update(now, InputFrame::default());
        b.update(now, InputFrame::default());
    }
    assert_eq!(*a.phase(), ClientPhase::Lobby, "the UDP door seats without Accept");
    assert_eq!(*b.phase(), ClientPhase::Waiting);
    let l = a.lobby().unwrap();
    assert_eq!(l.seats.len(), 1);
    assert_eq!(l.seats[0].path, SeatPath::Network);
    let p = host.pending_joiners()[0].clone();
    assert!(p.peer >= crate::door::BT_BASE);
    assert_eq!(p.persona.as_deref(), Some("npub1radio"));
    assert!(host.accept_pending(p.peer));
    for k in 50..80 {
        let now = 10_000 + k * 16;
        t.store(now, Ordering::Relaxed);
        host.update(now);
        a.update(now, InputFrame::default());
        b.update(now, InputFrame::default());
    }
    assert_eq!(*b.phase(), ClientPhase::Lobby);
    assert_eq!(link_.bluetooth_peers(), 1);
    // Closing the door keeps the seated radio player.
    link_.stop_bluetooth();
    host.update(10_000 + 81 * 16);
    assert_eq!(host.human_count(), 2);
}

/// LocalRun and ClientSession are both `RunDriver`s (the run screen's one API).
#[test]
fn both_drivers_are_run_drivers() {
    fn takes(_: &mut dyn RunDriver) {}
    let mut l = crate::LocalRun::solo(1);
    takes(&mut l);
    let hub = LoopbackHub::new(LoopClock::manual().0, 1);
    let mut c = ClientSession::new(Box::new(hub.connect(LoopbackConfig::PERFECT)), "x", 0);
    takes(&mut c);
}

// ─── Admission regressions (independent review: H1, M1, M2, L1, L3) ────────

mod admission {
    use std::collections::VecDeque;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;

    use flock_core::{Event, MatchConfig, ObjId, PlayerId, Tool};

    use super::*;
    use crate::client::{HOST_ACCEPT_MS, HOST_CHECK_MS, HOST_UNVERIFIED};
    use crate::proto::{encode, LobbyView, ToClient};
    use crate::transport::{ClientNetEvent, ClientTransport, HostIdentity};
    use crate::view::ResultsView;
    use crate::PROTO_VERSION;

    /// A client link to a Bluetooth host: `pending` is the transport's
    /// "host check pending" until a good attestation arrives.
    struct FakeBt {
        inq: Arc<Mutex<VecDeque<ClientNetEvent>>>,
        pending: Arc<AtomicBool>,
    }
    impl ClientTransport for FakeBt {
        fn poll(&mut self) -> Option<ClientNetEvent> {
            self.inq.lock().unwrap().pop_front()
        }
        fn send_reliable(&mut self, _: Vec<u8>) {}
        fn send_lossy(&mut self, _: Vec<u8>) {}
        fn rtt_ms(&self) -> Option<u32> {
            None
        }
        fn close(&mut self) {}
        fn host_check_pending(&self) -> bool {
            self.pending.load(Ordering::Relaxed)
        }
        fn verify_host(&mut self, a: &str) -> Result<String, String> {
            if a == "good" {
                self.pending.store(false, Ordering::Relaxed);
                Ok("npub1host".into())
            } else {
                Err("bad".into())
            }
        }
    }

    struct Rig {
        me: ClientSession,
        q: Arc<Mutex<VecDeque<ClientNetEvent>>>,
    }

    /// A Bluetooth joiner (`pending`) or a plain link, `Connected` already queued.
    fn rig(pending: bool) -> Rig {
        let q = Arc::new(Mutex::new(VecDeque::new()));
        q.lock().unwrap().push_back(ClientNetEvent::Connected {
            host: Some(HostIdentity { name: "Bluetooth game".into(), pubkey: "00".into(), npub: "npub1x".into(), persona: None }),
        });
        let t = FakeBt { inq: q.clone(), pending: Arc::new(AtomicBool::new(pending)) };
        Rig { me: ClientSession::new(Box::new(t), "Joiner", 0), q }
    }
    impl Rig {
        fn push(&self, m: &ToClient) {
            self.q.lock().unwrap().push_back(ClientNetEvent::Reliable(encode(m)));
        }
        fn up(&mut self, now: u64) {
            self.me.update(now, InputFrame::default());
        }
    }
    fn start() -> ToClient {
        ToClient::Start { config: MatchConfig::multi(1, 2, None), start_host_ms: 0, you: PlayerId(0), run: 1 }
    }
    fn welcome() -> ToClient {
        ToClient::Welcome { proto: PROTO_VERSION, seat: 0 }
    }

    /// H1: a host that never sends Welcome cannot start a match on a joiner
    /// whose host check is pending, and the joiner fails closed.
    #[test]
    fn start_without_welcome_does_not_skip_the_host_check() {
        let mut r = rig(true);
        r.up(1);
        r.push(&start());
        for t in 2..20_000u64 {
            if t % 100 == 0 {
                r.up(t);
            }
        }
        r.up(20_000);
        assert!(matches!(r.me.phase(), ClientPhase::Connecting(_)), "{:?}", r.me.phase());
        assert!(r.me.view().is_none());
        assert!(r.me.host_persona().is_none());
        // The timeout runs from connect and nothing the host sent moved it.
        r.up(HOST_ACCEPT_MS + 2);
        assert_eq!(*r.me.phase(), ClientPhase::Failed(HOST_UNVERIFIED.into()));
    }

    /// H1: even seated, Start is dropped while the check pends; after a good
    /// attestation the same host is trusted.
    #[test]
    fn start_is_dropped_while_pending_and_accepted_after_a_good_attestation() {
        let mut r = rig(true);
        r.up(1);
        r.push(&welcome());
        r.push(&start());
        r.up(2);
        assert!(matches!(r.me.phase(), ClientPhase::Connecting(_)), "{:?}", r.me.phase());
        r.push(&ToClient::HostAttestation("good".into()));
        r.push(&ToClient::Lobby(LobbyView { seats: vec![], phase: LobbyPhase::Lobby, round: 0 }));
        r.up(3);
        assert_eq!(*r.me.phase(), ClientPhase::Lobby);
        assert_eq!(r.me.host_persona(), Some("npub1host"));
        assert!(r.me.lobby().is_some());
        r.push(&start());
        r.up(4);
        assert_eq!(*r.me.phase(), ClientPhase::Running);
    }

    /// H1: a Start from a connection that was never seated is ignored.
    #[test]
    fn start_needs_a_seat() {
        let mut r = rig(false);
        r.up(1);
        r.push(&start());
        r.up(2);
        assert!(!matches!(r.me.phase(), ClientPhase::Running), "{:?}", r.me.phase());
    }

    /// M1: Results then Lobby during the check show nothing.
    #[test]
    fn results_then_lobby_show_nothing_during_the_check() {
        let mut r = rig(true);
        r.up(1);
        r.push(&welcome());
        r.up(2);
        assert!(matches!(r.me.phase(), ClientPhase::Connecting(_)));
        r.push(&ToClient::Results(ResultsView { rows: vec![], end_reason: None, round: 0, solo: None }));
        r.push(&ToClient::Lobby(LobbyView { seats: vec![], phase: LobbyPhase::Lobby, round: 0 }));
        r.up(3);
        assert!(matches!(r.me.phase(), ClientPhase::Connecting(_)), "{:?}", r.me.phase());
        assert!(r.me.lobby().is_none());
        assert!(r.me.results().is_none());
        assert!(r.me.host_persona().is_none());
        // And it still fails closed.
        r.up(2 + HOST_CHECK_MS + 1);
        assert_eq!(*r.me.phase(), ClientPhase::Failed(HOST_UNVERIFIED.into()));
    }

    /// M1: a Results message with no match under way is not a phase change
    /// (also on a link with no host check).
    #[test]
    fn results_without_a_match_are_ignored() {
        let mut r = rig(false);
        r.up(1);
        r.push(&welcome());
        r.push(&ToClient::Results(ResultsView { rows: vec![], end_reason: None, round: 0, solo: None }));
        r.up(2);
        assert_eq!(*r.me.phase(), ClientPhase::Lobby);
        assert!(r.me.results().is_none());
    }

    /// M2: re-sent Welcomes do not postpone the fail-closed timeout.
    #[test]
    fn repeated_welcome_does_not_postpone_the_host_check_timeout() {
        let mut r = rig(true);
        r.up(1);
        let mut t = 2u64;
        let mut failed_at = None;
        for _ in 0..10 {
            r.push(&welcome());
            r.up(t);
            t += 14_000;
            r.up(t);
            if r.me.phase().is_dead() && failed_at.is_none() {
                failed_at = Some(t);
            }
        }
        assert_eq!(*r.me.phase(), ClientPhase::Failed(HOST_UNVERIFIED.into()));
        assert!(failed_at.is_some_and(|f| f <= 2 + 2 * 14_000), "failed at {failed_at:?}");
    }

    /// M2: a Waiting that keeps coming does not extend the wait past the
    /// fixed window from connect either.
    #[test]
    fn repeated_waiting_does_not_extend_the_window() {
        let mut r = rig(true);
        r.up(1);
        let mut t = 2u64;
        while t < HOST_ACCEPT_MS + 2_000 {
            r.push(&ToClient::Waiting);
            r.up(t);
            t += 5_000;
        }
        assert!(r.me.phase().is_dead(), "{:?}", r.me.phase());
    }

    /// L1: placements outside the held window, with non-architect ids, or
    /// beyond the cap never grow the course without bound.
    #[test]
    fn hostile_placed_events_cannot_grow_the_course_without_bound() {
        let mut r = rig(false);
        r.up(1);
        r.push(&welcome());
        r.push(&start());
        r.up(2);
        assert_eq!(*r.me.phase(), ClientPhase::Running);
        let mut flood = |row: u32, base: u32| {
            let mut obj = base;
            for _ in 0..400 {
                let events: Vec<Event> = (0..40)
                    .map(|_| {
                        obj += 1;
                        Event::Placed { by: PlayerId(1), obj: ObjId(obj), tool: Tool::Mud, lane: 0, row, solid_at: 0 }
                    })
                    .collect();
                let m = ToClient::Events { run: 1, events };
                assert!(encode(&m).len() <= 1024);
                r.push(&m);
            }
            r.up(3);
            r.me.view().map(|v| v.world.course().placed().len()).unwrap_or(0)
        };
        assert_eq!(flood(u32::MAX, ObjId::ARCHITECT_BASE), 0, "row far outside the window");
        assert_eq!(flood(5, 1 << 30), 0, "ids that are not architect ids");
        let held = flood(5, ObjId::ARCHITECT_BASE + 1_000_000);
        assert!(held > 0 && held <= 1024, "held {held}");
        // The app-bound queue is bounded too.
        let mut r = rig(false);
        r.up(1);
        r.push(&welcome());
        r.push(&start());
        r.up(2);
        for _ in 0..200 {
            r.push(&ToClient::Events { run: 1, events: vec![Event::Go; 150] });
        }
        r.push(&ToClient::Events { run: 1, events: vec![] });
        for k in 0..300u32 {
            r.push(&ToClient::PlaceResult { run: 1, req: k, result: Err(flock_core::Reject::TooClose) });
        }
        r.up(3);
        let (e, p) = r.me.queued_for_test();
        assert!(e <= 2048 && p <= 256, "events {e}, place results {p}");
    }

    /// L3: a Bluetooth joiner is not seated by a host with no attestation
    /// to show it.
    #[test]
    fn bluetooth_accept_is_refused_without_a_host_attestation() {
        let mut net = Net::new(cfg(false));
        net.join("Host", LoopbackConfig::PERFECT, Pilot::Idle);
        let bt = net.hub.connect_as(link(30, 0, 0.0), LinkPath::Bluetooth, Some("npub1bluetooth".into()));
        let k = net.add(ClientSession::new(Box::new(bt), "Radio", 2), Pilot::Idle);
        net.run_for(300);
        let p = net.host.pending_joiners()[0].peer;
        assert!(!net.host.accept_pending(p), "refused: the host has nothing to show");
        net.run_for(300);
        assert!(matches!(net.clients[k].s.phase(), ClientPhase::Closed(_)), "{:?}", net.clients[k].s.phase());
        assert_eq!(net.host.human_count(), 1);
    }
}
