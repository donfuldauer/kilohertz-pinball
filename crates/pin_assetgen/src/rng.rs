//! The kit's own seeded PCG and the hashes that derive seeds from table
//! metadata. Nothing here reads the clock, a thread RNG or hash-map order.

/// PCG-XSH-RR 32 (O'Neill), 64-bit state. Same seed and stream give the same
/// sequence on every platform.
#[derive(Clone, Debug)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

const PCG_MULT: u64 = 6_364_136_223_846_793_005;

impl Pcg32 {
    /// Seeds a generator; `stream` selects one of 2^63 independent sequences.
    pub fn new(seed: u64, stream: u64) -> Self {
        let mut rng = Pcg32 {
            state: 0,
            inc: (stream << 1) | 1,
        };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    /// Next 32 random bits.
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(PCG_MULT).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform integer in `0..n` (`0` when `n == 0`), by multiply-shift.
    pub fn below(&mut self, n: u32) -> u32 {
        ((u64::from(self.next_u32()) * u64::from(n)) >> 32) as u32
    }

    /// Uniform `f64` in `[0, 1)` with 32 bits of resolution.
    pub fn unit(&mut self) -> f64 {
        f64::from(self.next_u32()) / 4_294_967_296.0
    }

    /// Uniform `f64` in `[-1, 1)`.
    pub fn signed(&mut self) -> f64 {
        self.unit() * 2.0 - 1.0
    }
}

/// FNV-1a 64 over `bytes`, continuing from `h`.
pub fn fnv1a(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// FNV-1a 64 offset basis.
pub const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;

/// SplitMix64 finaliser: spreads every input bit over the output.
pub fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcg_reference_sequence() {
        // Reference values of pcg32_srandom_r(42, 54) from the PCG C demo.
        let mut r = Pcg32::new(42, 54);
        let got: Vec<u32> = (0..6).map(|_| r.next_u32()).collect();
        assert_eq!(
            got,
            [
                0xa15c02b7, 0x7b47f409, 0xba1d3330, 0x83d2f293, 0xbfa4784b, 0xcbed606e
            ]
        );
    }

    #[test]
    fn below_and_unit_ranges() {
        let mut r = Pcg32::new(1, 2);
        for _ in 0..1000 {
            assert!(r.below(7) < 7);
            let u = r.unit();
            assert!((0.0..1.0).contains(&u));
            let s = r.signed();
            assert!((-1.0..1.0).contains(&s));
        }
        assert_eq!(r.below(0), 0);
    }

    #[test]
    fn hashes_spread() {
        assert_ne!(fnv1a(FNV_OFFSET, b"a"), fnv1a(FNV_OFFSET, b"b"));
        assert_eq!(fnv1a(FNV_OFFSET, b""), FNV_OFFSET);
        assert_ne!(mix64(0), mix64(1));
    }
}
