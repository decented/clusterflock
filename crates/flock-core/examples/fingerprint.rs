//! Print the course fingerprint of a daily seed, to compare with the
//! `daily run: ... course fingerprint` line a phone logs.
//!
//!     cargo run -p flock-core --example fingerprint -- 0xfd088d4dfa815887 [rows]

use flock_core::{course_fingerprint, det, MatchConfig};

fn main() {
    let mut args = std::env::args().skip(1);
    let seed = args.next().expect("usage: fingerprint <seed, hex with 0x or decimal> [rows]");
    let seed = match seed.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16).expect("hex seed"),
        None => seed.parse().expect("decimal seed"),
    };
    let rows = args.next().map(|r| r.parse().expect("rows")).unwrap_or(600);
    println!("{}", det::hex(&course_fingerprint(&MatchConfig::daily(seed), rows)));
}
