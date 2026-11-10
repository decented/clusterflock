//! `flock-headless`: a host with no screen, for checks from the laptop.
//!
//! * `--smoke [--players N] [--bots B]` — host a room on localhost QUIC, join
//!   it with N real QUIC clients (synthetic signed My Signet identities, the
//!   unchanged persona check), ready up, play a full round with B host bots,
//!   and check every client got the host's results. Exit 0 on success; one
//!   JSON line on stdout. Default 4 players + 2 bots (6 seats).
//! * `--lan [--bots B] [--seconds S] [--name NAME]` — a bounded LAN room
//!   ("Flock test host") that phones join from Nearby, with B bots; it starts
//!   when every joined phone is ready and rematches when all are ready again.
//!   Ends after S seconds (default 900). Synthetic signed test identity: the
//!   phones verify it like any host.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use flock_core::{Command, InputFrame, Phase};
use flock_session::net::{self, QuicHost};
use flock_session::{ClientPhase, ClientSession, HostAuthority, HostConfig, HostPhase, ResultsView};

#[derive(Debug)]
struct Args {
    smoke: bool,
    lan: bool,
    players: usize,
    bots: usize,
    seconds: u64,
    name: String,
}

const USAGE: &str = "flock-headless --smoke [--players N] [--bots B]\n       flock-headless --lan [--bots B] [--seconds S] [--name NAME]";

fn parse() -> Result<Args, String> {
    let mut a = Args { smoke: false, lan: false, players: 4, bots: 2, seconds: 900, name: "Flock test host".into() };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut num = |name: &str| -> Result<u64, String> {
            it.next().and_then(|v| v.parse().ok()).ok_or_else(|| format!("{name} needs a number\n{USAGE}"))
        };
        match arg.as_str() {
            "--smoke" => a.smoke = true,
            "--lan" => a.lan = true,
            "--players" => a.players = num("--players")? as usize,
            "--bots" => a.bots = num("--bots")? as usize,
            "--seconds" => a.seconds = num("--seconds")?,
            "--name" => a.name = it.next().ok_or("--name needs a value")?,
            "-h" | "--help" => return Err(USAGE.into()),
            other => return Err(format!("unknown argument {other:?}\n{USAGE}")),
        }
    }
    if a.smoke == a.lan {
        return Err(USAGE.into());
    }
    let seats = flock_session::proto::MAX_SEATS;
    if a.smoke && (a.players == 0 || a.players + a.bots > seats || a.players + a.bots < 2) {
        return Err(format!("players + bots must be 2..={seats} (players >= 1)"));
    }
    if a.lan && a.bots >= seats {
        return Err(format!("at most {} bots (leave a seat for a phone)", seats - 1));
    }
    Ok(a)
}

fn main() {
    let _ = env_log();
    let args = match parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let r = if args.smoke { smoke(&args) } else { lan(&args) };
    if let Err(e) = r {
        eprintln!("flock-headless: {e}");
        std::process::exit(1);
    }
}

/// Minimal stderr logging (RUST_LOG-free: warnings and errors only).
fn env_log() -> Result<(), log::SetLoggerError> {
    struct L;
    impl log::Log for L {
        fn enabled(&self, m: &log::Metadata) -> bool {
            m.level() <= log::Level::Warn
        }
        fn log(&self, r: &log::Record) {
            if self.enabled(r.metadata()) {
                eprintln!("[{}] {}", r.level(), r.args());
            }
        }
        fn flush(&self) {}
    }
    static LOGGER: L = L;
    log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Warn))
}

fn data_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("flock-headless-{tag}-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    d
}

/// A headless host on `quic` (no player seat of its own) with `bots` bots.
fn authority(quic: QuicHost, id: &net::Identity, bots: usize) -> (HostAuthority, flock_session::door::HostLink<QuicHost>) {
    let link = flock_session::door::HostLink::new(quic);
    let mut host = HostAuthority::new(Box::new(link.clone()), HostConfig::default());
    if let Some((json, npub)) = net::own_persona(id) {
        host.set_host_identity(Some(json), Some(npub));
    }
    for _ in 0..bots {
        host.add_bot();
    }
    (host, link)
}

fn wait_ready(host: &mut HostAuthority, link: &flock_session::door::HostLink<QuicHost>, start: Instant) -> Result<(u16, String, String), String> {
    loop {
        host.update(start.elapsed().as_millis() as u64);
        let st = link.with_lan(|q| {
            if let Some(f) = q.failure() {
                return Some(Err(f.to_string()));
            }
            if q.is_ready() {
                let port = q.local_addr()?.port();
                return Some(Ok((port, q.lan_token()?, q.code()?.to_string())));
            }
            None
        });
        match st {
            Some(r) => return r,
            None if start.elapsed() > Duration::from_secs(10) => return Err("the room did not open in 10 s".into()),
            None => std::thread::sleep(Duration::from_millis(4)),
        }
    }
}

fn smoke(args: &Args) -> Result<(), String> {
    let dir = data_dir("smoke");
    let mut cfg = net::net_config(&dir);
    cfg.relays.clear();
    cfg.stun.clear();
    cfg.lan_beacon = false;
    cfg.host_has_player = false;
    let host_id = net::test_identity();
    let quic = net::host_room(&cfg, &host_id, "Flock smoke");
    let (mut host, link) = authority(quic, &host_id, args.bots);
    let start = Instant::now();
    let (port, token, _code) = wait_ready(&mut host, &link, start)?;
    let addr = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), port);
    let mut clients: Vec<ClientSession> = (0..args.players)
        .map(|k| {
            let id = net::test_identity();
            ClientSession::new(Box::new(net::join_addr(&cfg, &id, addr, &token)), &format!("Smoke {}", k + 1), k as u8)
        })
        .collect();
    let mut frames = vec![InputFrame::default(); args.players];
    let mut readied = false;
    let mut ended_at: Option<u64> = None;
    let mut running_since: Option<u64> = None;
    loop {
        let now = start.elapsed().as_millis() as u64;
        host.update(now);
        for (k, c) in clients.iter_mut().enumerate() {
            // A little steering so the round has commands in it.
            if *c.phase() == ClientPhase::Running && now % 1500 < 5 {
                let cmd = if (now / 1500 + k as u64).is_multiple_of(2) { Command::Left } else { Command::Right };
                frames[k] = InputFrame { seq: frames[k].seq.wrapping_add(1), cmd: Some(cmd) };
            }
            c.update(now, frames[k]);
            if let ClientPhase::Failed(e) | ClientPhase::Closed(e) = c.phase() {
                return Err(format!("smoke client {k}: {e}"));
            }
        }
        if !readied && host.human_count() == args.players && clients.iter().all(|c| *c.phase() == ClientPhase::Lobby) {
            for c in &mut clients {
                c.set_ready(true);
            }
            readied = true;
        }
        if host.phase() == HostPhase::Running && running_since.is_none() {
            running_since = Some(now);
        }
        // Six seconds after GO, end the round (everyone becomes a ghost).
        if let (Some(t), None) = (running_since, ended_at) {
            let go = host.world().is_some_and(|w| w.phase() == Phase::Running);
            if go && now > t + 3_700 + 6_000 {
                let w = host.world_mut().expect("world");
                let ids: Vec<_> = w.players().iter().map(|p| p.id()).collect();
                for id in ids {
                    w.force_eliminate(id);
                }
                ended_at = Some(now);
            }
        }
        if let Some(res) = host.results().cloned() {
            if clients.iter().all(|c| c.results().is_some()) {
                return finish(args, &mut host, &mut clients, &res, start, &link);
            }
        }
        if start.elapsed() > Duration::from_secs(60) {
            return Err(format!(
                "smoke timed out: host {:?}, {} seated, clients {:?}",
                host.phase(),
                host.human_count(),
                clients.iter().map(|c| c.phase().clone()).collect::<Vec<_>>()
            ));
        }
        std::thread::sleep(Duration::from_millis(4));
    }
}

fn finish(
    args: &Args,
    host: &mut HostAuthority,
    clients: &mut [ClientSession],
    res: &ResultsView,
    start: Instant,
    link: &flock_session::door::HostLink<QuicHost>,
) -> Result<(), String> {
    for (k, c) in clients.iter().enumerate() {
        if c.results() != Some(res) {
            return Err(format!("client {k} results differ from the host's"));
        }
        if c.host_persona().is_none() {
            return Err(format!("client {k} has no verified host persona"));
        }
    }
    let seats = res.rows.len();
    let stats = host.stats().clone();
    let snaps: u64 = clients.iter().map(|c| c.stats().snapshots).sum();
    let snaps_max = clients.iter().map(|c| c.stats().max_correction_m).fold(0.0f32, f32::max);
    println!(
        "{{\"event\":\"smoke\",\"completed\":true,\"seats\":{seats},\"players\":{},\"bots\":{},\"transport\":\"localhost QUIC\",\"synthetic_identities\":true,\"end\":\"{:?}\",\"snapshot_bytes_max\":{},\"snapshots_received\":{snaps},\"max_correction_m\":{snaps_max:.3},\"rows_sent\":{},\"wall_seconds\":{:.1}}}",
        args.players,
        args.bots,
        res.end_reason,
        stats.snapshot_bytes_max,
        stats.rows_sent,
        start.elapsed().as_secs_f64()
    );
    for c in clients.iter_mut() {
        c.leave();
    }
    let _ = link;
    host.shutdown();
    Ok(())
}

fn lan(args: &Args) -> Result<(), String> {
    let dir = data_dir("lan");
    let mut cfg = net::net_config(&dir);
    cfg.relays.clear();
    cfg.host_has_player = false;
    let host_id = net::test_identity();
    let quic = net::host_room(&cfg, &host_id, &args.name);
    let (mut host, link) = authority(quic, &host_id, args.bots);
    let start = Instant::now();
    let (port, _token, code) = wait_ready(&mut host, &link, start)?;
    println!(
        "{{\"event\":\"lan_ready\",\"name\":{:?},\"code\":{code:?},\"port\":{port},\"bots\":{},\"seconds\":{}}}",
        args.name, args.bots, args.seconds
    );
    let mut last_phase = host.phase();
    let mut last_humans = 0;
    while start.elapsed() < Duration::from_secs(args.seconds) {
        host.update(start.elapsed().as_millis() as u64);
        if host.phase() != last_phase || host.human_count() != last_humans {
            last_phase = host.phase();
            last_humans = host.human_count();
            let names: Vec<String> = host.lobby_view().seats.iter().map(|s| format!("{}{}", s.name, if s.bot { " (bot)" } else { "" })).collect();
            println!("{{\"event\":\"room\",\"phase\":\"{last_phase:?}\",\"seats\":{names:?}}}");
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    let _ = link;
    host.shutdown();
    println!("{{\"event\":\"lan_done\"}}");
    Ok(())
}
