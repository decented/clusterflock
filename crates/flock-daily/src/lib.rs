//! The Cluster Flock daily run: one track for everybody, seeded by Bitcoin.
//!
//! There is no server. The "daily block" of a UTC day is read from a public
//! block explorer, but the explorer is only a transport: every header is parsed
//! and checked here (proof of work, difficulty floor, hash linkage) before the
//! day rule picks a block. See `docs/SPEC.md`, "Daily run".
//!
//! * [`Header`], [`Block`], [`verify_chain`]: parsing and verification.
//! * [`find_daily`]: the day rule on a verified window.
//! * [`seed_from_hash`]: the 64-bit seed of the sim.
//! * [`resolve`]: the whole procedure over a [`resolve::Source`].
//! * `http` (feature `fetch`): the blocking HTTPS source.

use sha2::{Digest, Sha256};
use std::fmt;

pub mod resolve;
#[cfg(feature = "fetch")]
pub mod http;

/// Seconds in a UTC day.
pub const DAY_SECS: u64 = 86_400;
/// How many predecessors of the daily block must be older than midnight.
pub const PREDECESSORS: usize = 6;
/// Headers fetched around the midnight hint: heights `hint-7 ..= hint+7`.
pub const WINDOW: usize = 15;
/// Domain separator of the seed.
pub const SEED_DOMAIN: &[u8] = b"clusterflock-daily-v1";

/// The easiest target a header may claim, as compact `bits`.
///
/// It is the target of difficulty 1.0e13 (`0xffff * 2^208 / 1e13`, rounded down
/// by the compact encoding, so the real difficulty is a hair above 1e13). Real
/// mainnet difficulty since 2025 is above 1e14; forging one header at 1e13
/// costs roughly 4e22 hashes. The floor stops an explorer from inventing
/// headers with an easy target; it is not a proof that the chain is the
/// heaviest one.
pub const FLOOR_BITS: u32 = 0x171c_25a6;

/// A descriptive failure. Nothing here panics on bad network input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A header or hash was not valid hex of the right length.
    BadEncoding(String),
    /// `bits` is zero, negative or overflows 256 bits.
    BadBits(u32),
    /// The header hash is above its own target.
    PowFailed { height: u32 },
    /// The target is easier than [`FLOOR_BITS`].
    TargetTooEasy { height: u32 },
    /// Header `height` does not point at the hash of header `height - 1`.
    BrokenLink { height: u32 },
    /// Heights in a window are not consecutive, or the window is empty or short.
    BadWindow(String),
    /// The explorer returned a header whose hash is not the hash it was asked for.
    WrongHeader,
    /// The window cannot decide the day: the first six blocks are not all before midnight.
    Inconclusive,
    /// No block of this day was found, today or yesterday.
    NoDailyBlock,
    /// The two explorers disagree about the hash at the chosen height.
    CrossCheckMismatch { height: u32 },
    /// The request budget (about 40) ran out.
    TooManyRequests,
    /// A network or HTTP failure of one source.
    Network { source: String, what: String },
    /// An explorer answered, but not in the documented format.
    BadResponse { source: String, what: String },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadEncoding(s) => write!(f, "bad encoding: {s}"),
            Error::BadBits(b) => write!(f, "invalid difficulty bits {b:#010x}"),
            Error::PowFailed { height } => write!(f, "block {height}: hash is above its target"),
            Error::TargetTooEasy { height } => write!(f, "block {height}: target is easier than the floor"),
            Error::BrokenLink { height } => write!(f, "block {height}: does not follow the previous header"),
            Error::BadWindow(s) => write!(f, "bad header window: {s}"),
            Error::WrongHeader => write!(f, "explorer returned a header that does not match its hash"),
            Error::Inconclusive => write!(f, "header window does not reach back before midnight"),
            Error::NoDailyBlock => write!(f, "no daily block found"),
            Error::CrossCheckMismatch { height } => write!(f, "explorers disagree about block {height}"),
            Error::TooManyRequests => write!(f, "request budget exhausted"),
            Error::Network { source, what } => write!(f, "{source}: {what}"),
            Error::BadResponse { source, what } => write!(f, "{source}: unexpected response ({what})"),
        }
    }
}

impl std::error::Error for Error {}

fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// Lower-case hex of `bytes`.
pub fn to_hex(bytes: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(D[(b >> 4) as usize] as char);
        s.push(D[(b & 15) as usize] as char);
    }
    s
}

/// Hex to bytes. Accepts upper and lower case, rejects odd lengths and junk.
pub fn from_hex(s: &str) -> Result<Vec<u8>, Error> {
    let s = s.trim();
    let bad = || Error::BadEncoding(format!("not hex ({} chars)", s.len()));
    if s.len() % 2 != 0 || !s.is_ascii() {
        return Err(bad());
    }
    let nib = |c: u8| match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    };
    s.as_bytes()
        .chunks(2)
        .map(|p| Some(nib(p[0])? << 4 | nib(p[1])?))
        .collect::<Option<Vec<u8>>>()
        .ok_or_else(bad)
}

/// A block hash as explorers display it (byte-reversed) to the 32 bytes in
/// header order (the output of double SHA-256).
pub fn hash_from_display(s: &str) -> Result<[u8; 32], Error> {
    let mut v = from_hex(s)?;
    if v.len() != 32 {
        return Err(Error::BadEncoding(format!("hash is {} bytes, not 32", v.len())));
    }
    v.reverse();
    Ok(v.try_into().unwrap())
}

/// A hash in header order to the displayed (byte-reversed) hex.
pub fn hash_to_display(h: &[u8; 32]) -> String {
    let mut r = *h;
    r.reverse();
    to_hex(&r)
}

/// An 80-byte Bitcoin block header.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Header(pub [u8; 80]);

impl Header {
    pub fn from_hex(s: &str) -> Result<Header, Error> {
        let v = from_hex(s)?;
        let a: [u8; 80] = v
            .try_into()
            .map_err(|v: Vec<u8>| Error::BadEncoding(format!("header is {} bytes, not 80", v.len())))?;
        Ok(Header(a))
    }

    /// The previous block's hash, in header order.
    pub fn prev(&self) -> [u8; 32] {
        self.0[4..36].try_into().unwrap()
    }

    /// The header timestamp, seconds since the Unix epoch.
    pub fn time(&self) -> u32 {
        u32::from_le_bytes(self.0[68..72].try_into().unwrap())
    }

    /// The compact difficulty target.
    pub fn bits(&self) -> u32 {
        u32::from_le_bytes(self.0[72..76].try_into().unwrap())
    }

    /// The block hash, double SHA-256, in header order.
    pub fn hash(&self) -> [u8; 32] {
        sha256(&sha256(&self.0))
    }

    /// The block hash as explorers display it.
    pub fn hash_display(&self) -> String {
        hash_to_display(&self.hash())
    }
}

/// The 256-bit big-endian target of compact `bits`.
pub fn compact_to_target(bits: u32) -> Result<[u8; 32], Error> {
    let exp = (bits >> 24) as usize;
    let mant = bits & 0x007f_ffff;
    if bits & 0x0080_0000 != 0 || mant == 0 {
        return Err(Error::BadBits(bits));
    }
    // Value = mant * 256^(exp-3). Byte i of the mantissa (0 = least significant)
    // lands at little-endian position (exp - 3 + i).
    let mut le = [0u8; 32];
    for i in 0..3usize {
        let byte = ((mant >> (8 * i)) & 0xff) as u8;
        if byte == 0 {
            continue;
        }
        let pos = (exp as isize) - 3 + i as isize;
        if pos < 0 {
            continue; // shifted out below the units digit
        }
        if pos >= 32 {
            return Err(Error::BadBits(bits));
        }
        le[pos as usize] = byte;
    }
    le.reverse();
    Ok(le)
}

/// The floor target ([`FLOOR_BITS`]).
pub fn floor_target() -> [u8; 32] {
    compact_to_target(FLOOR_BITS).expect("floor bits are valid")
}

/// A header with its height.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Block {
    pub height: u32,
    pub header: Header,
}

/// Check one header on its own: valid bits, target at or below the floor,
/// hash at or below the target.
pub fn check_header(height: u32, h: &Header) -> Result<(), Error> {
    let target = compact_to_target(h.bits())?;
    if target > floor_target() {
        return Err(Error::TargetTooEasy { height });
    }
    let mut hash = h.hash();
    hash.reverse(); // big-endian number
    if hash > target {
        return Err(Error::PowFailed { height });
    }
    Ok(())
}

/// Verify a window of consecutive blocks: heights step by one, every header
/// passes [`check_header`], and every header points at the hash of the one
/// before it.
pub fn verify_chain(blocks: &[Block]) -> Result<(), Error> {
    if blocks.is_empty() {
        return Err(Error::BadWindow("empty".into()));
    }
    for (i, b) in blocks.iter().enumerate() {
        check_header(b.height, &b.header)?;
        if i > 0 {
            let p = &blocks[i - 1];
            if b.height != p.height.wrapping_add(1) {
                return Err(Error::BadWindow(format!("heights {} then {}", p.height, b.height)));
            }
            if b.header.prev() != p.header.hash() {
                return Err(Error::BrokenLink { height: b.height });
            }
        }
    }
    Ok(())
}

/// The UTC day number of a Unix time.
pub fn day_of(t: u64) -> u64 {
    t / DAY_SECS
}

/// The day rule on a window. The window is verified first ([`verify_chain`]).
///
/// The daily block of `day` is the lowest-height block whose timestamp is
/// `>= day * 86400` and whose six predecessors all have timestamps below it.
/// Returns the index in `blocks`, or `Ok(None)` when no block of the window
/// reaches midnight. Because the window cannot say what lies before it, the
/// first six blocks must all be before midnight, else [`Error::Inconclusive`].
pub fn find_daily(blocks: &[Block], day: u64) -> Result<Option<usize>, Error> {
    verify_chain(blocks)?;
    let mid = day * DAY_SECS;
    let before = |b: &Block| (b.header.time() as u64) < mid;
    if blocks.len() <= PREDECESSORS || !blocks[..PREDECESSORS].iter().all(before) {
        return Err(Error::Inconclusive);
    }
    for i in PREDECESSORS..blocks.len() {
        if !before(&blocks[i]) && blocks[i - PREDECESSORS..i].iter().all(before) {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

/// The sim seed: the first 8 bytes of
/// `SHA-256("clusterflock-daily-v1" || hash)`, big-endian, where `hash` is the
/// 32-byte block hash in header order (not the displayed, reversed form).
pub fn seed_from_hash(hash: &[u8; 32]) -> u64 {
    let mut h = Sha256::new();
    h.update(SEED_DOMAIN);
    h.update(hash);
    let d = h.finalize();
    u64::from_be_bytes(d[..8].try_into().unwrap())
}

/// A resolved daily.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Daily {
    /// UTC day number (`floor(t / 86400)`).
    pub day: u64,
    /// The block height, the daily's public number.
    pub height: u32,
    /// The block hash as explorers display it.
    pub hash_hex: String,
    /// The sim seed.
    pub seed: u64,
}

impl Daily {
    pub fn from_block(day: u64, b: &Block) -> Daily {
        let h = b.header.hash();
        Daily { day, height: b.height, hash_hex: hash_to_display(&h), seed: seed_from_hash(&h) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_examples() {
        // Genesis: 0x1d00ffff = 0xffff * 2^208.
        let t = compact_to_target(0x1d00ffff).unwrap();
        assert_eq!(to_hex(&t), "00000000ffff0000000000000000000000000000000000000000000000000000");
        assert!(compact_to_target(0).is_err());
        assert!(compact_to_target(0x0480_0001).is_err(), "negative");
        assert!(compact_to_target(0x2301_0000).is_err(), "overflow");
        assert_eq!(compact_to_target(0x0100_0100).unwrap()[31], 0, "shifted out");
    }

    #[test]
    fn floor_is_difficulty_1e13_or_harder() {
        // diff1 / 1e13 as hex, from an independent calculation.
        let f = floor_target();
        assert!(to_hex(&f).starts_with("0000000000000000001c25a6"), "{}", to_hex(&f));
        // Mainnet difficulty in 2026 (bits 0x1701e5ac) is far harder than the floor.
        assert!(compact_to_target(0x1701e5ac).unwrap() < f);
    }

    #[test]
    fn seed_vector() {
        // Block 974869, displayed hash.
        let h = hash_from_display("0000000000000000000123e050878a33503aca65df86ca668fca2ec493b46f79").unwrap();
        assert_eq!(h[0], 0x79, "header order is the reverse of the display");
        assert_eq!(seed_from_hash(&h), 0x4b9c_c6b9_ba92_b55f);
        assert_eq!(seed_from_hash(&[0u8; 32]), 0x7b1e_2b4f_90aa_346b);
    }

    #[test]
    fn hex_round_trip_and_junk() {
        assert_eq!(from_hex("00ffAb").unwrap(), vec![0, 255, 0xab]);
        assert!(from_hex("abc").is_err());
        assert!(from_hex("zz").is_err());
        assert!(Header::from_hex("00").is_err());
        assert!(hash_from_display("00").is_err());
    }
}
