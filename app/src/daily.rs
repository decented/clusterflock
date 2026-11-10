//! The daily run's service: when to fetch the day's Bitcoin block, the
//! background thread that does it, and the daily's bookkeeping in the profile.
//!
//! The rule, the verification and the fetch are in the `blockseed` crate; see
//! `docs/SPEC.md` section 14. Nothing here blocks the UI thread: the fetch runs
//! on its own thread and the screen polls a channel once per frame.

use crate::platform::data_dir::{DailyBest, DailyCache, Profile};
use flock_core::{course_fingerprint, det, MatchConfig};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{SystemTime, UNIX_EPOCH};

/// The seed domain of the daily run. Changing it changes every daily seed.
pub const SEED_DOMAIN: &[u8] = b"clusterflock-daily-v1";
/// The User-Agent sent to the block explorers: plain, no contact details.
pub const USER_AGENT: &str = "clusterflock";

/// A cached daily younger than this is not fetched again.
pub const FRESH_SECS: u64 = 20 * 60;
/// After a failed fetch, wait this long before trying again.
pub const RETRY_SECS: u64 = 90;
/// Rows hashed for the fingerprint logged at the start of a daily run.
pub const FINGERPRINT_ROWS: u32 = 600;

/// Unix time now, from the phone clock. It only spaces the refetches; the
/// daily itself comes from block timestamps.
pub fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Should a fetch start now? True with no cache, or a cache older than 20
/// minutes. A phone clock set back counts as old too.
pub fn is_stale(cache: Option<&DailyCache>, now: u64) -> bool {
    match cache {
        None => true,
        Some(c) => c.fetched_unix.abs_diff(now) > FRESH_SECS,
    }
}

/// `921344` as `921,344`.
pub fn thousands(n: u32) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// Store a fetched daily in the profile. It never moves back to an older day (a
/// stale explorer tip, or a tip timestamp that steps back across midnight), and a
/// result the second source did not confirm never replaces one already cached for
/// that day. A different block (by hash) resets the day's best.
pub fn apply(profile: &mut Profile, d: &blockseed::Daily, cross_checked: bool, now: u64) {
    if let Some(c) = &profile.daily {
        if d.day < c.day || (!cross_checked && d.day == c.day && d.hash_hex != c.hash_hex) {
            return;
        }
        if c.hash_hex != d.hash_hex {
            profile.daily_best = None;
        }
    }
    if profile.daily_best.as_ref().is_some_and(|b| b.height != d.height) {
        profile.daily_best = None;
    }
    profile.daily = Some(DailyCache { day: d.day, height: d.height, hash_hex: d.hash_hex.clone(), seed: d.seed, fetched_unix: now });
}

/// Count one attempt on the cached daily and keep the best metres (with the
/// coins of that run). `metres` is where the run ended, also when the player quit.
pub fn record_attempt(profile: &mut Profile, height: u32, metres: u32, coins: u32) {
    let b = profile.daily_best.get_or_insert_with(DailyBest::default);
    if b.height != height {
        *b = DailyBest { height, ..DailyBest::default() };
    }
    b.attempts = b.attempts.saturating_add(1);
    if metres > b.metres || b.attempts == 1 {
        b.metres = metres;
        b.coins = coins;
    }
}

/// The line logged when a daily run starts, so that two phones can be compared
/// in logcat. The hash of 600 generated rows is computed on a short-lived
/// thread, not in the frame that starts the run.
pub fn log_start(height: u32, seed: u64) {
    log::info!("daily run: block {height} seed {seed:#018x}");
    let _ = std::thread::Builder::new().name("daily-fingerprint".into()).spawn(move || {
        let fp = det::hex(&course_fingerprint(&MatchConfig::daily(seed), FINGERPRINT_ROWS));
        log::info!("daily run: block {height} course fingerprint({FINGERPRINT_ROWS} rows) {fp}");
    });
}

/// What the Home button should say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Line {
    /// A fetch is running and there is nothing cached to play.
    Fetching,
    /// No cached daily and no way to get one right now.
    NeedsInternet,
    /// A daily can be played.
    Ready { height: u32, best_m: Option<u32> },
}

impl Line {
    /// The sub-line under "Daily run".
    pub fn text(&self) -> String {
        match self {
            Line::Fetching => "Fetching today's track\u{2026}".into(),
            Line::NeedsInternet => "Needs internet once today".into(),
            Line::Ready { height, best_m: Some(m) } => format!("block {} \u{b7} best {} m today", thousands(*height), m),
            Line::Ready { height, best_m: None } => format!("block {} \u{b7} not run yet", thousands(*height)),
        }
    }

    pub fn playable(&self) -> bool {
        matches!(self, Line::Ready { .. })
    }
}

type Reply = Result<blockseed::resolve::Outcome, blockseed::Error>;

/// The background fetch, owned by the Home screen.
#[derive(Default)]
pub struct DailyService {
    rx: Option<Receiver<Reply>>,
    /// When the last attempt failed (unix), for the retry pause.
    failed_at: Option<u64>,
    /// The last error, for the log and tests.
    pub last_error: Option<String>,
}

impl DailyService {
    pub fn new() -> DailyService {
        DailyService::default()
    }

    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Call once per frame while Home is on top: start a fetch if the cache is
    /// stale, and take the result of a finished one. `allow` is false in
    /// `--shot` mode (no network) and anywhere else a fetch is unwelcome.
    pub fn poll(&mut self, profile: &mut Profile, allow: bool) -> bool {
        let now = now_unix();
        let mut changed = false;
        if let Some(rx) = &self.rx {
            match rx.try_recv() {
                Ok(Ok(o)) => {
                    log::info!("daily: block {} seed {:#x} cross-check {:?} ({} requests)", o.daily.height, o.daily.seed, o.cross_check, o.requests);
                    apply(profile, &o.daily, o.cross_checked(), now);
                    self.rx = None;
                    self.failed_at = None;
                    changed = true;
                }
                Ok(Err(e)) => {
                    log::warn!("daily: fetch failed: {e}");
                    self.last_error = Some(e.to_string());
                    self.failed_at = Some(now);
                    self.rx = None;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.failed_at = Some(now);
                    self.rx = None;
                }
            }
        }
        let paused = self.failed_at.is_some_and(|f| now.abs_diff(f) < RETRY_SECS);
        if allow && self.rx.is_none() && !paused && is_stale(profile.daily.as_ref(), now) {
            // The cached daily, if any. blockseed uses it only when it is the daily
            // of the day before the tip's and today's is not confirmed yet, which
            // saves walking yesterday's blocks again after midnight.
            let known = profile.daily.as_ref().map(|c| blockseed::Daily {
                day: c.day,
                height: c.height,
                hash_hex: c.hash_hex.clone(),
                seed: c.seed,
            });
            self.start(known);
        }
        changed
    }

    fn start(&mut self, known: Option<blockseed::Daily>) {
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new().name("daily-fetch".into()).spawn(move || {
            let _ = tx.send(blockseed::http::fetch_today_with(SEED_DOMAIN, USER_AGENT, known.as_ref()));
        });
        match spawned {
            Ok(_) => self.rx = Some(rx),
            Err(e) => {
                log::warn!("daily: could not start the fetch thread: {e}");
                self.failed_at = Some(now_unix());
            }
        }
    }

    /// What Home should show for `profile`.
    pub fn line(&self, profile: &Profile) -> Line {
        match &profile.daily {
            Some(c) => Line::Ready { height: c.height, best_m: profile.daily_best_on(c.height).map(|b| b.metres).filter(|m| *m > 0) },
            None if self.busy() => Line::Fetching,
            None => Line::NeedsInternet,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use blockseed::Daily;

    fn daily(height: u32) -> Daily {
        Daily { day: 20766, height, hash_hex: "00".repeat(32), seed: 42 }
    }

    #[test]
    fn staleness_is_twenty_minutes() {
        let c = DailyCache { fetched_unix: 10_000, ..DailyCache::default() };
        assert!(is_stale(None, 0));
        assert!(!is_stale(Some(&c), 10_000 + 20 * 60));
        assert!(is_stale(Some(&c), 10_000 + 20 * 60 + 1));
        assert!(is_stale(Some(&c), 10_000 - 20 * 60 - 1), "clock set back");
    }

    #[test]
    fn thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(921_344), "921,344");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn a_new_block_resets_the_best_and_the_same_block_keeps_it() {
        let mut p = Profile::default();
        apply(&mut p, &daily(100), true, 5);
        record_attempt(&mut p, 100, 512, 9);
        apply(&mut p, &daily(100), true, 6);
        assert_eq!(p.daily_best, Some(DailyBest { height: 100, metres: 512, coins: 9, attempts: 1 }));
        assert_eq!(p.daily.as_ref().unwrap().fetched_unix, 6);
        apply(&mut p, &daily(101), true, 7);
        assert_eq!(p.daily_best, None);
    }

    #[test]
    fn attempts_count_and_the_best_keeps_its_coins() {
        let mut p = Profile::default();
        record_attempt(&mut p, 7, 300, 12);
        record_attempt(&mut p, 7, 100, 40);
        record_attempt(&mut p, 7, 450, 3);
        assert_eq!(p.daily_best, Some(DailyBest { height: 7, metres: 450, coins: 3, attempts: 3 }));
        record_attempt(&mut p, 8, 20, 1);
        assert_eq!(p.daily_best, Some(DailyBest { height: 8, metres: 20, coins: 1, attempts: 1 }));
    }

    #[test]
    fn home_line_texts() {
        let mut p = Profile::default();
        let s = DailyService::new();
        assert_eq!(s.line(&p).text(), "Needs internet once today");
        assert!(!s.line(&p).playable());
        apply(&mut p, &daily(921_344), true, 1);
        assert_eq!(s.line(&p).text(), "block 921,344 \u{b7} not run yet");
        record_attempt(&mut p, 921_344, 512, 3);
        assert_eq!(s.line(&p).text(), "block 921,344 \u{b7} best 512 m today");
        assert!(s.line(&p).playable());
        let busy = DailyService { rx: Some(channel().1), ..DailyService::default() };
        assert_eq!(busy.line(&Profile::default()).text(), "Fetching today's track\u{2026}");
    }
}
