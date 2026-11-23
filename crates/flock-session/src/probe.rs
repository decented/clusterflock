//! Synthetic clients that join a room and play until a round completes or a
//! number of host ticks have arrived: what `flock-headless --join-invite`
//! runs, and what the tests run against an in-process host. The caller says
//! how each client dials ([`crate::net::join_invite`] for a Keep's invite link,
//! a fake relay in tests), so the same code is exercised both ways.
//!
//! The clients are real [`ClientSession`]s over real QUIC links with
//! synthetic signed My Signet identities ([`net::test_identity`]): the host's
//! persona check and admission gate see them as any other joiner.

use std::time::{Duration, Instant};

use flock_core::{Command, InputFrame};

use crate::client::{ClientPhase, ClientSession};
use crate::net::{self, Identity, QuicClient};

/// What to play and for how long.
#[derive(Clone, Debug)]
pub struct ProbeConfig {
    /// Synthetic identities to join with, one client each (at least 1).
    pub players: usize,
    /// Stop early, as a success, once a client has seen this many host world
    /// ticks (0: only a completed round ends the run in success).
    pub ticks: u32,
    /// Give up after this long.
    pub timeout: Duration,
}

/// How a successful run ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeEnd {
    /// A client received the match's results.
    RoundCompleted,
    /// A client saw [`ProbeConfig::ticks`] host ticks.
    TicksReceived,
}

/// The one-line result.
#[derive(Clone, Debug)]
pub struct ProbeReport {
    pub end: ProbeEnd,
    pub players: usize,
    /// Clients that were seated in a lobby at the end.
    pub seated: usize,
    /// Clients that held the match's results at the end.
    pub with_results: usize,
    /// The newest host tick any client saw.
    pub ticks: u32,
    pub snapshots: u64,
    pub wall_seconds: f64,
}

impl ProbeReport {
    /// The report as one JSON line, as `--smoke` prints its own.
    pub fn json_line(&self) -> String {
        format!(
            "{{\"event\":\"join\",\"completed\":true,\"end\":\"{}\",\"players\":{},\"seated\":{},\"with_results\":{},\"ticks\":{},\"snapshots\":{},\"synthetic_identities\":true,\"wall_seconds\":{:.1}}}",
            match self.end {
                ProbeEnd::RoundCompleted => "round",
                ProbeEnd::TicksReceived => "ticks",
            },
            self.players,
            self.seated,
            self.with_results,
            self.ticks,
            self.snapshots,
            self.wall_seconds
        )
    }
}

/// Why a run did not succeed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeError {
    /// The room (or its Keep) turned a client away or ended its link.
    Refused(String),
    /// Neither a round nor the ticks arrived in time.
    TimedOut(String),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::Refused(why) | ProbeError::TimedOut(why) => f.write_str(why),
        }
    }
}

/// Join with `cfg.players` fresh synthetic identities (`dial` opens each
/// client's link) and play. `pump` runs once per frame with the elapsed
/// milliseconds: a test passes its in-process host's `update` there; a run
/// against a remote room passes a no-op.
///
/// Every client readies up in the lobby and steers a little. Returns on the
/// first of: a client holds the results, a client has seen `cfg.ticks` host
/// ticks, a client is refused (`Err(Refused)`), or the timeout
/// (`Err(TimedOut)`). The clients leave before it returns.
pub fn run_clients(
    cfg: &ProbeConfig,
    mut dial: impl FnMut(&Identity, usize) -> QuicClient,
    mut pump: impl FnMut(u64),
) -> Result<ProbeReport, ProbeError> {
    let players = cfg.players.max(1);
    let mut clients: Vec<ClientSession> = (0..players)
        .map(|k| {
            let id = net::test_identity();
            ClientSession::new(Box::new(dial(&id, k)), &format!("Probe {}", k + 1), k as u8)
        })
        .collect();
    let mut frames = vec![InputFrame::default(); players];
    let mut readied = vec![false; players];
    let start = Instant::now();
    let outcome = loop {
        let now = start.elapsed().as_millis() as u64;
        pump(now);
        for (k, c) in clients.iter_mut().enumerate() {
            // A little steering so the round has commands in it.
            if *c.phase() == ClientPhase::Running && now % 1500 < 5 {
                let cmd = if (now / 1500 + k as u64).is_multiple_of(2) { Command::Left } else { Command::Right };
                frames[k] = InputFrame { seq: frames[k].seq.wrapping_add(1), cmd: Some(cmd) };
            }
            c.update(now, frames[k]);
            if !readied[k] && *c.phase() == ClientPhase::Lobby {
                c.set_ready(true);
                readied[k] = true;
            }
        }
        if let Some((k, why)) = clients.iter().enumerate().find_map(|(k, c)| match c.phase() {
            ClientPhase::Failed(e) | ClientPhase::Closed(e) | ClientPhase::Disconnected(e) => Some((k, e.clone())),
            ClientPhase::HostLeft => Some((k, "the host left".to_string())),
            _ => None,
        }) {
            break Err(ProbeError::Refused(format!("client {k}: {why}")));
        }
        let with_results = clients.iter().filter(|c| c.results().is_some()).count();
        let ticks = clients.iter().filter_map(|c| c.newest_host_tick()).max().unwrap_or(0);
        let end = if with_results > 0 {
            Some(ProbeEnd::RoundCompleted)
        } else if cfg.ticks > 0 && ticks >= cfg.ticks {
            Some(ProbeEnd::TicksReceived)
        } else {
            None
        };
        if let Some(end) = end {
            break Ok(ProbeReport {
                end,
                players,
                seated: clients.iter().filter(|c| c.my_seat().is_some()).count(),
                with_results,
                ticks,
                snapshots: clients.iter().map(|c| c.stats().snapshots).sum(),
                wall_seconds: start.elapsed().as_secs_f64(),
            });
        }
        if start.elapsed() > cfg.timeout {
            break Err(ProbeError::TimedOut(format!(
                "no round or ticks within {} s: clients {:?}",
                cfg.timeout.as_secs(),
                clients.iter().map(|c| c.phase().clone()).collect::<Vec<_>>()
            )));
        }
        std::thread::sleep(Duration::from_millis(4));
    };
    for c in &mut clients {
        c.leave();
    }
    outcome
}
