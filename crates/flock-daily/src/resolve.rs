//! The procedure: tip, day, hint, window, verify, rule, cross-check.
//!
//! It runs over a [`Source`] so that it can be tested against recorded
//! mainnet headers; [`crate::http`] is the real one.

use crate::{check_header, day_of, find_daily, Block, Daily, Error, Header, DAY_SECS, WINDOW};

/// Most requests one resolution may make, both sources together.
pub const MAX_REQUESTS: u32 = 40;

/// The four Esplora calls the procedure needs. Everything a source returns is
/// untrusted and is checked by the caller.
pub trait Source {
    /// A short name for error messages.
    fn name(&self) -> &str;
    /// `GET /blocks/tip/height`.
    fn tip_height(&mut self) -> Result<u32, Error>;
    /// `GET /block-height/:height`: the hash in header order.
    fn hash_at(&mut self, height: u32) -> Result<[u8; 32], Error>;
    /// `GET /block/:hash/header`.
    fn header(&mut self, hash: &[u8; 32]) -> Result<Header, Error>;
    /// `GET /v1/mining/blocks/timestamp/:ts`: the height of the block nearest to `ts`.
    /// Only a hint; the window is verified anyway.
    fn height_hint(&mut self, ts: u64) -> Result<u32, Error>;
}

/// A resolved daily and how sure we are of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub daily: Daily,
    /// The second source agreed on the hash at the chosen height.
    pub cross_checked: bool,
    /// Requests made.
    pub requests: u32,
}

struct Budget(u32);
impl Budget {
    fn spend(&mut self) -> Result<(), Error> {
        if self.0 >= MAX_REQUESTS {
            return Err(Error::TooManyRequests);
        }
        self.0 += 1;
        Ok(())
    }
}

/// A header fetched by hash, checked to hash to that hash and to pass the
/// proof-of-work checks.
fn fetch_header(src: &mut dyn Source, b: &mut Budget, hash: &[u8; 32], height: u32) -> Result<Header, Error> {
    b.spend()?;
    let h = src.header(hash)?;
    if &h.hash() != hash {
        return Err(Error::WrongHeader);
    }
    check_header(height, &h)?;
    Ok(h)
}

/// Blocks that must sit on top of a daily block before it is adopted.
pub const CONFIRMATIONS: u64 = 3;

/// Resolve the current daily.
///
/// "Now" is the timestamp of the tip block, never the phone clock. The window
/// ends at `min(hint + 7, tip)` and goes back [`WINDOW`] headers by following
/// each header's previous hash, so linkage holds by construction and is
/// re-verified by [`find_daily`]. A second source, if given, must agree on the
/// hash at the chosen height; being unreachable only clears `cross_checked`.
pub fn resolve(primary: &mut dyn Source, secondary: Option<&mut dyn Source>) -> Result<Outcome, Error> {
    let mut b = Budget(0);
    b.spend()?;
    let tip = primary.tip_height()?;
    b.spend()?;
    let tip_hash = primary.hash_at(tip)?;
    let tip_header = fetch_header(primary, &mut b, &tip_hash, tip)?;
    let now = tip_header.time() as u64;
    let mut day = day_of(now);

    let mut found: Option<(u64, Block)> = None;
    for _ in 0..2 {
        b.spend()?;
        let hint = primary.height_hint(day * DAY_SECS)?;
        let top = hint.saturating_add(7).min(tip);
        if (top as usize) < WINDOW {
            return Err(Error::BadWindow("height too small".into()));
        }
        let mut blocks: Vec<Block> = Vec::with_capacity(WINDOW);
        let (mut hash, mut header) = if top == tip {
            (tip_hash, tip_header)
        } else {
            b.spend()?;
            let h = primary.hash_at(top)?;
            (h, fetch_header(primary, &mut b, &h, top)?)
        };
        let mut height = top;
        loop {
            blocks.push(Block { height, header });
            if blocks.len() == WINDOW {
                break;
            }
            hash = header.prev();
            height -= 1;
            header = fetch_header(primary, &mut b, &hash, height)?;
        }
        let _ = hash;
        blocks.reverse();
        if let Some(i) = find_daily(&blocks, day)? {
            // A block at or near the tip can still be orphaned, which would leave
            // phones on different seeds: wait for CONFIRMATIONS blocks on top.
            if (tip.saturating_sub(blocks[i].height) as u64) >= CONFIRMATIONS {
                found = Some((day, blocks[i]));
                break;
            }
        }
        // Today's block does not exist yet (or is not buried yet): yesterday's stays live.
        day -= 1;
    }
    let (day, block) = found.ok_or(Error::NoDailyBlock)?;
    let daily = Daily::from_block(day, &block);

    let mut cross_checked = false;
    if let Some(sec) = secondary {
        if b.spend().is_ok() {
            match sec.hash_at(block.height) {
                Ok(h) if h == block.header.hash() => cross_checked = true,
                Ok(_) => return Err(Error::CrossCheckMismatch { height: block.height }),
                Err(_) => {} // unreachable second source: tolerated and flagged
            }
        }
    }
    Ok(Outcome { daily, cross_checked, requests: b.0 })
}
