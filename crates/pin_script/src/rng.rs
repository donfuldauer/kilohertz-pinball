//! The random-number helpers both backends draw through (R3, SPEC §3.4).
//!
//! Lua's `math.random` / `pinball.random` and the Wasm `pinball.random`
//! import use exactly these functions over the sim-owned [`Pcg32`], so the
//! two backends produce the same draw sequence for the same seed. They
//! follow Lua 5.4's `math.random` (`project` by masked rejection, `I2d` for
//! floats) and are built under every feature combination (REQ-WEB-001).

use pin_core::{Pcg32, Rng};

/// A 64-bit draw from the host PCG (two 32-bit outputs).
pub(crate) fn next_u64(rng: &mut Pcg32) -> u64 {
    rng.next_u64()
}

/// Lua 5.4's `project`: a uniform integer in `[0, n]` by masked rejection.
///
/// `ran` is the first 64-bit draw. When `n + 1` is a power of two (or `n` is
/// `u64::MAX`) the result is `ran & n`; otherwise `ran` is masked to the
/// smallest all-ones value covering `n` and redrawn from `rng` until it is
/// at most `n`.
pub fn project(mut ran: u64, n: u64, rng: &mut Pcg32) -> u64 {
    if n & n.wrapping_add(1) == 0 {
        return ran & n;
    }
    let mut lim = n;
    lim |= lim >> 1;
    lim |= lim >> 2;
    lim |= lim >> 4;
    lim |= lim >> 8;
    lim |= lim >> 16;
    lim |= lim >> 32;
    loop {
        ran &= lim;
        if ran <= n {
            return ran;
        }
        ran = next_u64(rng);
    }
}

/// A uniform integer in `[lo, hi]` (`lo <= hi`), as Lua 5.4's
/// `math.random(lo, hi)`: one 64-bit draw, then [`project`] onto the span.
pub fn range(lo: i64, hi: i64, rng: &mut Pcg32) -> i64 {
    let r = next_u64(rng);
    let span = hi.wrapping_sub(lo) as u64;
    lo.wrapping_add(project(r, span, rng) as i64)
}

/// A float in `[0, 1)` with 53 random bits (Lua 5.4's `I2d`): the top 53
/// bits of one 64-bit draw times 2⁻⁵³.
pub fn unit_float(rng: &mut Pcg32) -> f64 {
    (next_u64(rng) >> 11) as f64 * (0.5 / (1u64 << 52) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::SeedableRng;
    use proptest::prelude::*;

    /// Reference model of Lua 5.4's `project`, written independently: the
    /// smallest `2^k - 1 >= n`, then rejection.
    fn project_model(ran: u64, n: u64, rng: &mut Pcg32) -> u64 {
        if n == u64::MAX {
            return ran;
        }
        if (n + 1).is_power_of_two() {
            return ran % (n + 1);
        }
        let mask = u64::MAX >> n.leading_zeros();
        let mut r = ran & mask;
        while r > n {
            r = rng.next_u64() & mask;
        }
        r
    }

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Pcg32::seed_from_u64(42);
        let mut b = Pcg32::seed_from_u64(42);
        for i in 0..256 {
            assert_eq!(range(-7, 1_000, &mut a), range(-7, 1_000, &mut b), "{i}");
            assert_eq!(
                unit_float(&mut a).to_bits(),
                unit_float(&mut b).to_bits(),
                "{i}"
            );
            let (ra, rb) = (next_u64(&mut a), next_u64(&mut b));
            assert_eq!(project(ra, 12, &mut a), project(rb, 12, &mut b), "{i}");
        }
        // A different seed gives a different sequence.
        let mut c = Pcg32::seed_from_u64(43);
        let mut a = Pcg32::seed_from_u64(42);
        let xs: Vec<i64> = (0..16).map(|_| range(0, i64::MAX, &mut a)).collect();
        let ys: Vec<i64> = (0..16).map(|_| range(0, i64::MAX, &mut c)).collect();
        assert_ne!(xs, ys);
    }

    #[test]
    fn next_u64_is_the_pcg_draw() {
        let mut a = Pcg32::seed_from_u64(5);
        let mut b = Pcg32::seed_from_u64(5);
        for _ in 0..16 {
            assert_eq!(next_u64(&mut a), b.next_u64());
        }
    }

    /// `range` consumes exactly one draw for a power-of-two span and
    /// `unit_float` exactly one draw (the Lua and Wasm streams stay aligned).
    #[test]
    fn draw_counts() {
        let mut a = Pcg32::seed_from_u64(77);
        let mut b = Pcg32::seed_from_u64(77);
        let r = range(10, 17, &mut a);
        assert_eq!(r, 10 + (b.next_u64() & 7) as i64);
        let f = unit_float(&mut a);
        assert_eq!(
            f.to_bits(),
            ((b.next_u64() >> 11) as f64 / 9_007_199_254_740_992.0).to_bits()
        );
        assert_eq!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn range_edges() {
        let mut rng = Pcg32::seed_from_u64(1);
        for v in [i64::MIN, -1, 0, 1, i64::MAX] {
            assert_eq!(range(v, v, &mut rng), v);
        }
        for _ in 0..64 {
            // Full range: any value, never a panic or overflow.
            let _ = range(i64::MIN, i64::MAX, &mut rng);
            let r = range(i64::MIN, i64::MIN + 2, &mut rng);
            assert!((i64::MIN..=i64::MIN + 2).contains(&r));
            let r = range(i64::MAX - 4, i64::MAX, &mut rng);
            assert!((i64::MAX - 4..=i64::MAX).contains(&r));
            let r = range(-1, 1, &mut rng);
            assert!((-1..=1).contains(&r));
        }
        // Full range is the raw draw reinterpreted (span = all ones).
        let mut a = Pcg32::seed_from_u64(3);
        let mut b = Pcg32::seed_from_u64(3);
        assert_eq!(
            range(i64::MIN, i64::MAX, &mut a),
            i64::MIN.wrapping_add(b.next_u64() as i64)
        );
    }

    #[test]
    fn project_masks_and_rejection() {
        let mut rng = Pcg32::seed_from_u64(9);
        // All-ones masks: no redraw, `ran & n`.
        assert_eq!(project(u64::MAX, u64::MAX, &mut rng), u64::MAX);
        assert_eq!(project(0b1011, 7, &mut rng), 0b011);
        assert_eq!(project(0xFFFF_FFFF_1234_5678, 0xFFFF, &mut rng), 0x5678);
        assert_eq!(project(12345, 0, &mut rng), 0);
        let before = rng.clone();
        assert_eq!(project(9, 1, &mut rng), 1);
        assert_eq!(rng, before, "an all-ones n never draws");
        // Non-masks: in range; accepted first draw does not touch the rng.
        assert_eq!(project(0b0100, 5, &mut rng), 4);
        assert_eq!(rng, before);
        // Rejected first draw (6 & 7 = 6 > 5) redraws.
        let r = project(0b0110, 5, &mut rng);
        assert!(r <= 5);
        assert_ne!(rng, before);
        for n in [2u64, 5, 6, 9, 100, 1 << 40, (1 << 63) + 1, u64::MAX - 1] {
            for _ in 0..32 {
                let ran = rng.next_u64();
                assert!(project(ran, n, &mut rng) <= n, "n = {n}");
            }
        }
    }

    #[test]
    fn unit_float_bounds() {
        let mut rng = Pcg32::seed_from_u64(11);
        for _ in 0..1024 {
            let f = unit_float(&mut rng);
            assert!((0.0..1.0).contains(&f));
            // 53 bits: f * 2^53 is an exact integer.
            let scaled = f * 9_007_199_254_740_992.0;
            assert_eq!(scaled, scaled.trunc());
        }
        // The largest possible value is just under 1.
        assert!(((u64::MAX >> 11) as f64 * (0.5 / (1u64 << 52) as f64)) < 1.0);
    }

    proptest! {
        /// `project` equals the reference model, draw for draw.
        #[test]
        fn project_matches_model(seed in any::<u64>(), ran in any::<u64>(), n in any::<u64>()) {
            let mut a = Pcg32::seed_from_u64(seed);
            let mut b = Pcg32::seed_from_u64(seed);
            let x = project(ran, n, &mut a);
            prop_assert!(x <= n);
            prop_assert_eq!(x, project_model(ran, n, &mut b));
            prop_assert_eq!(a, b);
        }

        /// `range` stays within `[lo, hi]` and is deterministic per seed.
        #[test]
        fn range_in_bounds_and_deterministic(seed in any::<u64>(), x in any::<i64>(), y in any::<i64>()) {
            let (lo, hi) = if x <= y { (x, y) } else { (y, x) };
            let mut a = Pcg32::seed_from_u64(seed);
            let mut b = Pcg32::seed_from_u64(seed);
            for _ in 0..8 {
                let r = range(lo, hi, &mut a);
                prop_assert!(lo <= r && r <= hi);
                prop_assert_eq!(r, range(lo, hi, &mut b));
            }
        }

        /// `unit_float` is in `[0, 1)` and uses the top 53 bits of one draw.
        #[test]
        fn unit_float_in_unit_interval(seed in any::<u64>()) {
            let mut a = Pcg32::seed_from_u64(seed);
            let mut b = Pcg32::seed_from_u64(seed);
            for _ in 0..8 {
                let f = unit_float(&mut a);
                prop_assert!((0.0..1.0).contains(&f));
                prop_assert_eq!(f, (b.next_u64() >> 11) as f64 / 9_007_199_254_740_992.0);
            }
        }
    }
}
