//! Exactly specified arithmetic for everything that feeds the simulation or
//! the course generator.
//!
//! `f32::exp`, `sin`, `powf`, `ln` and friends call the platform's libm,
//! whose results may differ by an ulp between an arm64 phone and an x86
//! host. Everything here is built from IEEE-754 `+ - * /`, `round` and bit
//! casts only, which are correctly rounded on every platform Rust supports
//! (Rust never contracts `a * b + c` into an FMA on its own), so the results
//! are bit-identical everywhere.
//!
//! [`SplitMix64`] is the course RNG: a pinned, documented algorithm with
//! pinned draw functions (no `rand` crate in the generation path, whose
//! `gen_range` / float conversion are allowed to change between versions).

use serde::{Deserialize, Serialize};

// fdlibm's split of ln 2 (high part has trailing zero bits, so `k * LN2_HI` is exact).
const LN2_HI: f64 = f64::from_bits(0x3fe6_2e42_fee0_0000);
const LN2_LO: f64 = f64::from_bits(0x3dea_39ef_3579_3c76);
const LOG2_E: f64 = std::f64::consts::LOG2_E;

/// `2^k` for `-1022 <= k <= 1023` (exact).
fn pow2(k: i32) -> f64 {
    f64::from_bits(((k + 1023) as u64) << 52)
}

/// `e^x` from basic operations only (relative error < 1e-15).
pub fn exp(x: f64) -> f64 {
    if x.is_nan() {
        return x;
    }
    if x > 709.0 {
        return f64::INFINITY;
    }
    if x < -708.0 {
        return 0.0;
    }
    // x = k ln2 + r, |r| <= ln2 / 2.
    let k = (x * LOG2_E).round();
    let r = (x - k * LN2_HI) - k * LN2_LO;
    // Taylor series, Horner form, degree 14 (|r|^15 / 15! < 1e-17).
    let mut s = 1.0f64;
    let mut i = 14;
    while i >= 1 {
        s = 1.0 + s * r / i as f64;
        i -= 1;
    }
    s * pow2(k as i32)
}

/// `e^x` for `f32`, evaluated in `f64` (deterministic).
pub fn exp_f32(x: f32) -> f32 {
    exp(x as f64) as f32
}

/// `sin(x)` from basic operations only (absolute error < 1e-15 for
/// `|x| < 1e6`).
pub fn sin(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    use std::f64::consts::{FRAC_PI_2, PI, TAU};
    // Reduce to [-pi, pi], then to [-pi/2, pi/2] by sin(pi - r) = sin(r).
    let k = (x / TAU).round();
    let mut r = x - k * TAU;
    if r > FRAC_PI_2 {
        r = PI - r;
    } else if r < -FRAC_PI_2 {
        r = -PI - r;
    }
    // Taylor series to r^19 (|r| <= pi/2: term < 1e-17).
    let r2 = r * r;
    let mut s = 1.0f64;
    let mut n = 19u32;
    while n >= 3 {
        s = 1.0 - s * r2 / ((n * (n - 1)) as f64);
        n -= 2;
    }
    r * s
}

/// `sin` for `f32`, evaluated in `f64` (deterministic).
pub fn sin_f32(x: f32) -> f32 {
    sin(x as f64) as f32
}

/// SplitMix64 (Steele, Lea, Flood 2014; the reference constants of
/// `java.util.SplittableRandom`). The course RNG: one generator per row,
/// seeded from `(match seed, row index)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SplitMix64 {
    state: u64,
}

/// The SplitMix64 output function (also used to derive seeds).
pub fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    pub fn next_u64(&mut self) -> u64 {
        let z = self.state;
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        // Same mixing as `mix64(z)`.
        mix64(z)
    }
    /// High 32 bits of the next output.
    pub fn next_u32(&mut self) -> u32 {
        (self.next_u64() >> 32) as u32
    }
    /// Uniform in `[0, 1)` with 24 bits (exact `f32`).
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / 16_777_216.0)
    }
    /// Uniform integer in `0..n` (`n > 0`; multiply-shift, no rejection).
    pub fn below(&mut self, n: u32) -> u32 {
        ((self.next_u32() as u64 * n as u64) >> 32) as u32
    }
    /// Uniform integer in `lo..hi` (`hi > lo`).
    pub fn range_u8(&mut self, lo: u8, hi: u8) -> u8 {
        lo + self.below((hi - lo) as u32) as u8
    }
    /// A fair coin.
    pub fn flip(&mut self) -> bool {
        self.next_u64() >> 63 == 1
    }
}

const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01,
    0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
    0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
    0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116, 0x1e376c08,
    0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 (FIPS 180-4) of `data` (small, dependency-free; for course
/// fingerprints).
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for block in msg.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, c) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([c[0], c[1], c[2], c[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K256[i]).wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// Lower-case hex of a digest.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_test_vectors() {
        assert_eq!(hex(&sha256(b"")), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(hex(&sha256(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(
            hex(&sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn exp_matches_libm_closely() {
        for i in -400..=400 {
            let x = i as f64 * 0.137;
            let (a, b) = (exp(x), x.exp());
            assert!(((a - b) / b).abs() < 1e-14, "exp({x}) = {a} vs {b}");
        }
        assert_eq!(exp(0.0), 1.0);
        assert_eq!(exp(-1000.0), 0.0);
    }

    #[test]
    fn sin_matches_libm_closely() {
        for i in -2000..=2000 {
            let x = i as f64 * 0.0731;
            assert!((sin(x) - x.sin()).abs() < 1e-13, "sin({x})");
        }
        assert!((sin(250_000.3) - 250_000.3f64.sin()).abs() < 1e-9);
    }

    #[test]
    fn splitmix_reference_vector() {
        // First outputs of SplitMix64 seeded with 1234567 (published reference).
        let mut r = SplitMix64::new(1_234_567);
        assert_eq!(r.next_u64(), 6_457_827_717_110_365_317);
        assert_eq!(r.next_u64(), 3_203_168_211_198_807_973);
        assert_eq!(r.next_u64(), 9_817_491_932_198_370_423);
        let mut r = SplitMix64::new(7);
        for _ in 0..1000 {
            let u = r.unit();
            assert!((0.0..1.0).contains(&u));
            assert!(r.below(3) < 3);
        }
    }
}
