//! Recorded mainnet headers: blocks 974861..=974875 (the UTC midnight of day
//! 20764, 2026-11-07) and 974725..=974739 (day 20763). Lines are
//! `height displayed-hash header-hex`.

use flock_daily::resolve::{resolve, Source};
use flock_daily::*;
use std::collections::HashMap;

const D64: &str = include_str!("data/day_20764.txt");
const D63: &str = include_str!("data/day_20763.txt");

fn load(s: &str) -> Vec<Block> {
    s.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let mut p = l.split_whitespace();
            let height: u32 = p.next().unwrap().parse().unwrap();
            let hash = p.next().unwrap();
            let header = Header::from_hex(p.next().unwrap()).unwrap();
            assert_eq!(header.hash_display(), hash, "vector {height}");
            Block { height, header }
        })
        .collect()
}

const MID_64: u64 = 20764 * DAY_SECS;

#[test]
fn hash_is_double_sha256_displayed_reversed() {
    let b = load(D64);
    assert_eq!(b.len(), 15);
    let daily = b.iter().find(|b| b.height == 974869).unwrap();
    assert_eq!(daily.header.hash_display(), "0000000000000000000123e050878a33503aca65df86ca668fca2ec493b46f79");
    assert_eq!(daily.header.time() as u64 - MID_64, 637);
    assert_eq!(daily.header.bits(), 0x1701e5ac);
}

#[test]
fn real_windows_verify() {
    verify_chain(&load(D64)).unwrap();
    verify_chain(&load(D63)).unwrap();
}

#[test]
fn day_rule_picks_the_first_block_after_midnight() {
    let b = load(D64);
    let i = find_daily(&b, 20764).unwrap().unwrap();
    assert_eq!(b[i].height, 974869);
    let d = Daily::from_block(20764, &b[i]);
    assert_eq!(d.height, 974869);
    assert_eq!(d.seed, 0x4b9c_c6b9_ba92_b55f);

    let b = load(D63);
    let i = find_daily(&b, 20763).unwrap().unwrap();
    assert_eq!(b[i].height, 974733);
}

#[test]
fn no_block_qualifies_in_a_window_before_midnight() {
    // The first eight blocks of day 20764's window all precede its midnight.
    let b = load(D64);
    assert_eq!(find_daily(&b[..8], 20764).unwrap(), None);
    // The same window read for the previous day has a block after that midnight
    // but its first six are also after: inconclusive, not a wrong answer.
    assert_eq!(find_daily(&b, 20763), Err(Error::Inconclusive));
}

#[test]
fn a_window_must_reach_back_before_midnight() {
    let b = load(D64);
    // Starting just before the daily block leaves fewer than six predecessors.
    assert_eq!(find_daily(&b[8..], 20764), Err(Error::Inconclusive));
}

#[test]
fn tampered_header_fails() {
    let mut b = load(D64);
    b[5].header.0[70] ^= 0x01; // a timestamp byte: the hash no longer meets the target
    let e = verify_chain(&b).unwrap_err();
    assert!(matches!(e, Error::PowFailed { .. } | Error::BrokenLink { .. }), "{e}");
}

#[test]
fn too_easy_target_fails() {
    let mut b = load(D64);
    // Difficulty 1 target with a hash that cannot meet it as tampered, but the
    // floor check must fire first, whatever the hash is.
    b[3].header.0[72..76].copy_from_slice(&0x1d00_ffffu32.to_le_bytes());
    assert_eq!(verify_chain(&b), Err(Error::TargetTooEasy { height: b[3].height }));
    // And one step above the floor is refused, one at the floor is not a TargetTooEasy.
    let mut h = b[3].header;
    h.0[72..76].copy_from_slice(&(FLOOR_BITS + 1).to_le_bytes());
    assert_eq!(check_header(1, &h), Err(Error::TargetTooEasy { height: 1 }));
    h.0[72..76].copy_from_slice(&FLOOR_BITS.to_le_bytes());
    assert_eq!(check_header(1, &h), Err(Error::PowFailed { height: 1 }));
}

#[test]
fn broken_link_fails() {
    let mut b = load(D64);
    b.remove(7); // heights jump
    assert!(matches!(verify_chain(&b), Err(Error::BadWindow(_))));
    let mut b = load(D64);
    b[7].header = load(D63)[7].header; // valid header, wrong place, renumbered
    assert_eq!(verify_chain(&b), Err(Error::BrokenLink { height: b[7].height }));
}

#[test]
fn seed_changes_with_the_block() {
    let b = load(D64);
    let a = seed_from_hash(&b[8].header.hash());
    let c = seed_from_hash(&b[9].header.hash());
    assert_ne!(a, c);
}

/// An explorer over recorded headers.
struct Mock {
    name: &'static str,
    blocks: HashMap<u32, Block>,
    tip: u32,
    hint_offset: i64,
    hint_fixed: Option<u32>,
    down: bool,
    lie_at: Option<u32>,
    calls: u32,
}

impl Mock {
    fn new(name: &'static str, sets: &[&str]) -> Mock {
        let mut blocks = HashMap::new();
        for s in sets {
            for b in load(s) {
                blocks.insert(b.height, b);
            }
        }
        Mock { name, blocks, tip: 0, hint_offset: 0, hint_fixed: None, down: false, lie_at: None, calls: 0 }
    }
    fn nope(&self) -> Error {
        Error::Network { source: self.name.into(), what: "down".into() }
    }
}

impl Source for Mock {
    fn name(&self) -> &str {
        self.name
    }
    fn tip_height(&mut self) -> Result<u32, Error> {
        self.calls += 1;
        if self.down { Err(self.nope()) } else { Ok(self.tip) }
    }
    fn hash_at(&mut self, h: u32) -> Result<[u8; 32], Error> {
        self.calls += 1;
        if self.down {
            return Err(self.nope());
        }
        if self.lie_at == Some(h) {
            return Ok([7; 32]);
        }
        self.blocks.get(&h).map(|b| b.header.hash()).ok_or_else(|| self.nope())
    }
    fn header(&mut self, hash: &[u8; 32]) -> Result<Header, Error> {
        self.calls += 1;
        if self.down {
            return Err(self.nope());
        }
        self.blocks.values().find(|b| &b.header.hash() == hash).map(|b| b.header).ok_or_else(|| self.nope())
    }
    fn height_hint(&mut self, ts: u64) -> Result<u32, Error> {
        self.calls += 1;
        if let Some(h) = self.hint_fixed {
            return Ok(h);
        }
        // The last block at or before ts, like mempool.space, plus an offset.
        let h = self.blocks.values().filter(|b| b.header.time() as u64 <= ts).map(|b| b.height).max().ok_or_else(|| self.nope())?;
        Ok((h as i64 + self.hint_offset) as u32)
    }
}

#[test]
fn resolve_over_a_mock_chain() {
    let mut a = Mock::new("a", &[D64, D63]);
    a.tip = 974875;
    let mut b = Mock::new("b", &[D64]);
    let o = resolve(&mut a, Some(&mut b)).unwrap();
    assert_eq!(o.daily.day, 20764);
    assert_eq!(o.daily.height, 974869);
    assert_eq!(o.daily.seed, 0x4b9c_c6b9_ba92_b55f);
    assert!(o.cross_checked);
    assert!(o.requests <= 40);
}

#[test]
fn resolve_with_a_clamped_window_and_a_wrong_hint() {
    // The hint is three blocks too high, so the window is clamped at the tip.
    let mut a = Mock::new("a", &[D64, D63]);
    a.tip = 974875;
    a.hint_offset = 3;
    let o = resolve(&mut a, None).unwrap();
    assert_eq!(o.daily.height, 974869);
    assert!(!o.cross_checked);
}

#[test]
fn unreachable_second_source_is_tolerated_and_flagged() {
    let mut a = Mock::new("a", &[D64, D63]);
    a.tip = 974875;
    let mut b = Mock::new("b", &[]);
    b.down = true;
    let o = resolve(&mut a, Some(&mut b)).unwrap();
    assert!(!o.cross_checked);
    assert_eq!(o.daily.height, 974869);
}

#[test]
fn cross_check_mismatch_is_an_error() {
    let mut a = Mock::new("a", &[D64, D63]);
    a.tip = 974875;
    let mut b = Mock::new("b", &[D64]);
    b.lie_at = Some(974869);
    assert_eq!(resolve(&mut a, Some(&mut b)), Err(Error::CrossCheckMismatch { height: 974869 }));
}

#[test]
fn yesterday_stays_live_when_todays_window_has_no_block() {
    // The tip says it is day 20764, but the (wrong) hint points into day 20763:
    // the window holds no block of 20764, so the previous day is used.
    let mut a = Mock::new("a", &[D64, D63]);
    a.tip = 974875;
    a.hint_fixed = Some(974732);
    let o = resolve(&mut a, None).unwrap();
    assert_eq!(o.daily.day, 20763);
    assert_eq!(o.daily.height, 974733);
}

#[test]
fn an_explorer_that_answers_with_the_wrong_header_is_refused() {
    struct Liar(Mock);
    impl Source for Liar {
        fn name(&self) -> &str { "liar" }
        fn tip_height(&mut self) -> Result<u32, Error> { self.0.tip_height() }
        fn hash_at(&mut self, h: u32) -> Result<[u8; 32], Error> { self.0.hash_at(h) }
        fn height_hint(&mut self, t: u64) -> Result<u32, Error> { self.0.height_hint(t) }
        fn header(&mut self, _hash: &[u8; 32]) -> Result<Header, Error> {
            Ok(load(D64)[0].header)
        }
    }
    let mut m = Mock::new("a", &[D64]);
    m.tip = 974875;
    assert_eq!(resolve(&mut Liar(m), None), Err(Error::WrongHeader));
}

#[test]
fn request_budget_is_enforced() {
    let mut a = Mock::new("a", &[D64, D63]);
    a.tip = 974875;
    let mut b = Mock::new("b", &[D64]);
    let o = resolve(&mut a, Some(&mut b)).unwrap();
    assert!(o.requests <= resolve::MAX_REQUESTS);
}

/// Live check against both explorers. Run with
/// `cargo test -p flock-daily --features fetch -- --ignored`.
#[cfg(feature = "fetch")]
#[test]
#[ignore = "needs the network"]
fn live_today() {
    let o = flock_daily::http::fetch_today().expect("live daily");
    assert!(o.requests <= 40);
    assert_eq!(o.daily.hash_hex.len(), 64);
    assert!(o.daily.hash_hex.starts_with("0000000"));
    assert!(o.daily.height > 975_000);
    println!("{:?}", o);
}
