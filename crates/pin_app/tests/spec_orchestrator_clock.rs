//! Independent spec tests for `pin_app::orchestrator::SimClock` (Subphase 1.4,
//! verification criterion 3).
//!
//! `SimClock` is a pure accumulator: these tests feed it synthetic timestamps only (D8: no
//! sleeping, no wall clock). The randomised sweeps use a local SplitMix64 so this file needs
//! nothing beyond the crate's fixed API.

use pin_app::orchestrator::SimClock;
use pin_core::sim::{MAX_CATCH_UP_TICKS, TICK_NS};

struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_1EB5);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[test]
fn fresh_clock_has_nothing_due() {
    for start in [0u64, 1, 999_999, 1_000_000, 123_456_789_012] {
        let mut c = SimClock::new(start);
        assert_eq!(c.ticks_due_total(), 0);
        assert_eq!(c.dropped_ticks(), 0);
        assert_eq!(c.advance(start), 0, "no time has passed at start {start}");
        assert_eq!(c.ticks_due_total(), 0);
        assert_eq!(c.dropped_ticks(), 0);
    }
}

#[test]
fn one_tick_per_millisecond() {
    let start = 5_000_000_123;
    let mut c = SimClock::new(start);
    for k in 1..=5_000u64 {
        assert_eq!(c.advance(start + k * TICK_NS), 1, "tick {k}");
    }
    assert_eq!(c.ticks_due_total(), 5_000);
    assert_eq!(c.dropped_ticks(), 0);
}

#[test]
fn sub_tick_remainder_is_kept() {
    let mut c = SimClock::new(0);
    assert_eq!(c.advance(TICK_NS - 1), 0);
    assert_eq!(c.advance(TICK_NS), 1);
    // 1.5 ms later: one tick, half a tick stays in the accumulator.
    assert_eq!(c.advance(2 * TICK_NS + TICK_NS / 2), 1);
    // Another 0.5 ms completes the next tick.
    assert_eq!(c.advance(3 * TICK_NS), 1);
    assert_eq!(c.ticks_due_total(), 3);
    assert_eq!(c.dropped_ticks(), 0);

    // 0.4 ms steps: 5 steps = 2 ms = 2 ticks, in every window of 5 steps.
    let mut c = SimClock::new(0);
    let mut total = 0u64;
    for i in 1..=10_000u64 {
        let r = c.advance(i * 400_000);
        assert!(r <= 1);
        total += u64::from(r);
        assert_eq!(total, i * 400_000 / TICK_NS);
    }
    assert_eq!(c.dropped_ticks(), 0);
}

#[test]
fn catch_up_is_capped_at_eight_and_the_rest_is_dropped() {
    let cap = u64::from(MAX_CATCH_UP_TICKS);
    assert_eq!(cap, 8);

    // Exactly the cap: nothing dropped.
    let mut c = SimClock::new(0);
    assert_eq!(c.advance(8 * TICK_NS), 8);
    assert_eq!(c.dropped_ticks(), 0);

    // One over the cap: one dropped.
    let mut c = SimClock::new(0);
    assert_eq!(c.advance(9 * TICK_NS), 8);
    assert_eq!(c.dropped_ticks(), 1);
    assert_eq!(c.ticks_due_total(), 8);
    // Dropped time does not come back.
    assert_eq!(c.advance(9 * TICK_NS), 0);
    assert_eq!(c.advance(9 * TICK_NS + 1), 0);
    assert_eq!(c.advance(10 * TICK_NS), 1);
    assert_eq!(c.dropped_ticks(), 1);

    // A long stall: 20.7 ms → 8 ticks, 12 dropped, the 0.7 ms remainder is kept.
    let mut c = SimClock::new(1_000);
    assert_eq!(c.advance(1_000 + 20 * TICK_NS + 700_000), 8);
    assert_eq!(c.dropped_ticks(), 12);
    assert_eq!(c.advance(1_000 + 20 * TICK_NS + 999_999), 0);
    assert_eq!(c.advance(1_000 + 21 * TICK_NS), 1, "0.7 ms remainder + 0.3 ms");
    assert_eq!(c.ticks_due_total(), 9);
    assert_eq!(c.dropped_ticks(), 12);
}

#[test]
fn time_going_backwards_returns_zero_and_changes_nothing() {
    let mut c = SimClock::new(10 * TICK_NS);
    assert_eq!(c.advance(13 * TICK_NS + 500_000), 3);
    let (total, dropped) = (c.ticks_due_total(), c.dropped_ticks());
    for earlier in [0u64, 1, 10 * TICK_NS, 13 * TICK_NS, 13 * TICK_NS + 499_999] {
        assert_eq!(c.advance(earlier), 0, "now {earlier} is in the past");
        assert_eq!(c.ticks_due_total(), total);
        assert_eq!(c.dropped_ticks(), dropped);
    }
    // The reference point did not move back: 0.5 ms more is exactly one tick.
    assert_eq!(c.advance(14 * TICK_NS - 1), 0);
    assert_eq!(c.advance(14 * TICK_NS), 1);

    // A timestamp before the start is "earlier than the last call" too.
    let mut c = SimClock::new(1_000_000_000);
    assert_eq!(c.advance(0), 0);
    assert_eq!(c.advance(999_999_999), 0);
    assert_eq!(c.ticks_due_total(), 0);
    assert_eq!(c.dropped_ticks(), 0);
    assert_eq!(c.advance(1_000_000_000 + TICK_NS), 1);
}

#[test]
fn extreme_timestamps_do_not_overflow() {
    let mut c = SimClock::new(0);
    assert_eq!(c.advance(u64::MAX), 8);
    assert_eq!(c.ticks_due_total(), 8);
    assert_eq!(c.dropped_ticks(), u64::MAX / TICK_NS - 8);
    assert_eq!(c.advance(u64::MAX), 0);

    let mut c = SimClock::new(u64::MAX - 3 * TICK_NS);
    assert_eq!(c.advance(u64::MAX), 3);
    assert_eq!(c.dropped_ticks(), 0);

    let mut c = SimClock::new(u64::MAX);
    assert_eq!(c.advance(u64::MAX), 0);
    assert_eq!(c.advance(0), 0);
}

/// Criterion 3, as a randomised sweep over arbitrary timestamp sequences (forward steps of
/// every scale, repeats and backward jumps): never more than 8 per call; `ticks_due_total`
/// is the sum of the returns; `ticks_due_total + dropped == floor((now − start) / TICK_NS)`
/// for the latest time seen; both counters are monotone.
#[test]
fn property_accounting_invariant_holds_for_random_timelines() {
    let mut rng = SplitMix(0x1_4_C10C);
    for case in 0..400u64 {
        let start = match case % 4 {
            0 => 0,
            1 => rng.below(TICK_NS),
            2 => rng.below(1 << 40),
            _ => rng.next() >> 2,
        };
        let mut c = SimClock::new(start);
        let mut latest = start;
        let mut sum = 0u64;
        let mut prev_total = 0u64;
        let mut prev_dropped = 0u64;
        for _ in 0..300 {
            let now = match rng.below(10) {
                0 => latest,                                   // repeat
                1 => latest.saturating_sub(rng.below(5 * TICK_NS)), // backwards
                2 => rng.below(latest.max(1)),                 // far backwards
                3 => latest + rng.below(100),                  // nanoseconds
                4 | 5 => latest + rng.below(2 * TICK_NS),      // around one tick
                6 | 7 => latest + rng.below(12 * TICK_NS),     // around the cap
                8 => latest + rng.below(8) * TICK_NS,          // whole ticks
                _ => latest + rng.below(5_000 * TICK_NS),      // long stall
            };
            let before = (c.ticks_due_total(), c.dropped_ticks());
            let r = c.advance(now);
            assert!(r <= MAX_CATCH_UP_TICKS, "case {case}: returned {r}");
            if now < latest {
                assert_eq!(r, 0, "case {case}: backwards step must return 0");
                assert_eq!((c.ticks_due_total(), c.dropped_ticks()), before);
            }
            latest = latest.max(now);
            sum += u64::from(r);
            assert_eq!(c.ticks_due_total(), sum, "case {case}");
            assert_eq!(
                c.ticks_due_total() + c.dropped_ticks(),
                (latest - start) / TICK_NS,
                "case {case}: total + dropped must equal elapsed whole ticks \
                 (start {start}, now {latest})"
            );
            assert!(c.ticks_due_total() >= prev_total);
            assert!(c.dropped_ticks() >= prev_dropped);
            // Ticks are dropped only when the cap was hit.
            if c.dropped_ticks() > prev_dropped {
                assert_eq!(r, MAX_CATCH_UP_TICKS, "case {case}: dropped below the cap");
            }
            prev_total = c.ticks_due_total();
            prev_dropped = c.dropped_ticks();
        }
    }
}

/// With steps that never exceed the cap, nothing is ever dropped, whatever the phase.
#[test]
fn property_no_drops_when_polled_at_least_every_eight_ticks() {
    let mut rng = SplitMix(77);
    for _ in 0..200 {
        let start = rng.below(1 << 50);
        let mut c = SimClock::new(start);
        let mut now = start;
        for _ in 0..500 {
            now += rng.below(8 * TICK_NS + 1);
            let r = c.advance(now);
            assert!(r <= 8);
            assert_eq!(c.dropped_ticks(), 0);
            assert_eq!(c.ticks_due_total(), (now - start) / TICK_NS);
        }
    }
}

/// Two clocks fed the same timeline agree (pure function of its inputs).
#[test]
fn property_clock_is_deterministic() {
    let mut rng = SplitMix(4242);
    let start = rng.below(1 << 30);
    let mut a = SimClock::new(start);
    let mut b = SimClock::new(start);
    let mut now = start;
    for _ in 0..5_000 {
        now += rng.below(20 * TICK_NS);
        assert_eq!(a.advance(now), b.advance(now));
        assert_eq!(a.ticks_due_total(), b.ticks_due_total());
        assert_eq!(a.dropped_ticks(), b.dropped_ticks());
    }
}
