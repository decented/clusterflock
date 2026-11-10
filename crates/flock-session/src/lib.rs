//! Cluster Flock's multiplayer layer (SPEC §10), forked from Stampede's
//! `race-session` (migration, Keeps, track transfer, the Fray `Authority`
//! adapter and checkpoints are not ported).
//!
//! - [`RunDriver`] / [`FlockView`] / [`ResultsView`]: the one run-screen API.
//! - [`LocalRun`]: Solo and Flock Practice, no network.
//! - [`HostAuthority`]: lobby, seats (humans + bots), the match clock,
//!   snapshots, course streaming, placements, results, rematch, and the
//!   Bluetooth Accept gate.
//! - [`ClientSession`]: join, lobby view, and the match with own-runner
//!   prediction and interpolation. The host's own player is one of these on a
//!   zero-latency loopback ([`host_with_local_player`]).
//! - [`transport`]: `HostTransport` / `ClientTransport`, the loopback hub
//!   (virtual clock, latency, jitter, loss) and the mux.
//! - [`net`] (feature `native`): QUIC over fray-net (room code online and
//!   Nearby at once, Bluetooth via fray virtual links) with the My Signet
//!   persona check; [`door::HostLink`] merges the UDP and Bluetooth doors.
//! - [`proto`]: the wire messages.
//!
//! Every update takes the time explicitly (`now_ms`), so whole matches run on
//! a virtual clock in tests. See `docs/api/flock-session.md`.
#![forbid(unsafe_code)]

pub mod client;
pub mod clock;
pub mod door;
pub mod host;
pub mod local;
#[cfg(feature = "native")]
pub mod net;
pub mod proto;
mod shadow;
pub mod transport;
pub mod view;

#[cfg(test)]
mod tests;

pub use client::{ClientPhase, ClientSession, ClientStats};
pub use host::{HostAuthority, HostConfig, HostPhase, HostStats, PendingJoiner};
pub use local::LocalRun;
pub use proto::{LobbyPhase, LobbyView, SeatPath, SeatView, PROTO_VERSION};
pub use transport::{
    ClientNetEvent, ClientTransport, HostIdentity, HostNetEvent, HostTransport, LinkPath, LoopClock, LoopbackConfig,
    LoopbackHub, MuxHostTransport, NullHostTransport, PeerKey,
};
pub use view::{results_from_world, FlockView, ResultRow, ResultsView, RunDriver};

/// Build the host and the host player's own session, joined by a zero-latency
/// loopback beside the (optional) remote transport (QUIC / [`door::HostLink`]).
/// Drive both every frame: `host.update(now)` then `me.update(now, input)`.
pub fn host_with_local_player(
    remote: Option<Box<dyn HostTransport>>,
    cfg: HostConfig,
    name: &str,
    colour: u8,
) -> (HostAuthority, ClientSession) {
    let hub = LoopbackHub::new(LoopClock::real(), cfg.seed);
    let local_end = hub.host();
    let mux = MuxHostTransport::new(local_end, remote.unwrap_or_else(|| Box::new(NullHostTransport)));
    let me = ClientSession::new(Box::new(hub.connect(LoopbackConfig::PERFECT)), name, colour);
    (HostAuthority::new(Box::new(mux), cfg), me)
}
