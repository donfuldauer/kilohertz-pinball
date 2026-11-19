//! A fixed-capacity, allocation-free log-bucket histogram of `u64`
//! nanosecond samples (HDR-histogram style).
//!
//! # Bucket layout
//!
//! With [`SUB_BUCKET_BITS`] = 7 (`S`), values below `2^(S+1)` = 256 have a
//! bucket each (exact). A larger value `v` with highest set bit `e` lands in
//! bucket `shift · 2^S + (v >> shift)` with `shift = e − S`: every power of
//! two is split into `2^S` = 128 equal sub-buckets of width `2^shift`.
//! Values up to [`MAX_TRACKED_NS`] (`2^36 − 1` ns, about 68.7 s) are
//! bucketed; larger ones are counted in the top bucket and in
//! [`HistogramSummary::saturated`]. That is [`BUCKETS`] = 3840 counters,
//! allocated once by [`LatencyHistogram::new`].
//!
//! # Percentiles and the error bound
//!
//! A quantile `q` (given in parts per million, [`P50_PPM`] ...) uses the
//! nearest-rank rule on the sorted samples: rank `max(1, ⌈q · n⌉)`
//! (1-based), the same rule as `pin_sim::autoplay::report::nearest_rank`.
//! The histogram reports the inclusive upper edge of the bucket holding that
//! rank, clamped to the largest recorded sample (the exact maximum is kept).
//! So, with `x` the exact nearest-rank value:
//!
//! * the reported value is never below `x` (a pass against a target is
//!   never a false pass), and
//! * it is at most `x + ⌊x / 2^S⌋`, a relative error below
//!   [`RELATIVE_ERROR_BOUND`] = `2^-7` (0.78125 %); values below 256 ns are
//!   exact.
//!
//! When the rank falls in the saturated top bucket the reported value is the
//! exact maximum (no bound beyond `x ≤ reported ≤ max`).
//!
//! # Concurrency
//!
//! Every counter is an `AtomicU64` updated with a relaxed read-modify-write
//! (`fetch_add`, `fetch_min`, `fetch_max`): [`LatencyHistogram::record`]
//! takes `&self`, is O(1), never locks and never allocates, so the sim
//! thread can record while the HUD thread reads. A reader sees each counter
//! untorn; a summary taken while a writer runs may mix samples from
//! adjacent records (the HUD tolerates that; see the `profiler` module docs).

use core::sync::atomic::{AtomicU64, Ordering};

/// Sub-bucket bits `S`: each power of two is split into `2^S` buckets.
pub const SUB_BUCKET_BITS: u32 = 7;
/// Sub-buckets per power of two (`2^S`).
pub const SUB_BUCKETS: u64 = 1 << SUB_BUCKET_BITS;
/// Bit width of the tracked range: values below `2^MAX_TRACKED_BITS` are
/// bucketed with the error bound.
pub const MAX_TRACKED_BITS: u32 = 36;
/// The largest value bucketed with the error bound, ns (`2^36 − 1`, about
/// 68.7 s); larger samples saturate the top bucket.
pub const MAX_TRACKED_NS: u64 = (1 << MAX_TRACKED_BITS) - 1;
/// Number of buckets (3840).
pub const BUCKETS: usize = bucket_index_unchecked(MAX_TRACKED_NS) + 1;
/// Upper bound of the relative error of a reported percentile (`2^-S`).
pub const RELATIVE_ERROR_BOUND: f64 = 1.0 / SUB_BUCKETS as f64;

/// The median, in parts per million.
pub const P50_PPM: u32 = 500_000;
/// The 99th percentile, in parts per million.
pub const P99_PPM: u32 = 990_000;
/// The 99.9th percentile, in parts per million.
pub const P99_9_PPM: u32 = 999_000;
/// One million parts per million (the maximum quantile).
pub const PPM: u32 = 1_000_000;

const fn bucket_index_unchecked(v: u64) -> usize {
    if v < 2 * SUB_BUCKETS {
        v as usize
    } else {
        let e = 63 - v.leading_zeros();
        let shift = e - SUB_BUCKET_BITS;
        (shift as u64 * SUB_BUCKETS + (v >> shift)) as usize
    }
}

/// The bucket of sample `v` (values above [`MAX_TRACKED_NS`] map to the top
/// bucket).
#[must_use]
pub const fn bucket_index(v: u64) -> usize {
    let v = if v > MAX_TRACKED_NS {
        MAX_TRACKED_NS
    } else {
        v
    };
    bucket_index_unchecked(v)
}

/// The smallest value of bucket `i` (`i < BUCKETS`).
#[must_use]
pub const fn bucket_low(i: usize) -> u64 {
    let i = i as u64;
    if i < 2 * SUB_BUCKETS {
        i
    } else {
        let shift = i / SUB_BUCKETS - 1;
        (i - shift * SUB_BUCKETS) << shift
    }
}

/// The largest value of bucket `i` (`i < BUCKETS`), inclusive.
#[must_use]
pub const fn bucket_high(i: usize) -> u64 {
    let iu = i as u64;
    if iu < 2 * SUB_BUCKETS {
        iu
    } else {
        let shift = iu / SUB_BUCKETS - 1;
        bucket_low(i) + (1 << shift) - 1
    }
}

/// The nearest-rank of quantile `ppm` (parts per million, clamped to
/// [`PPM`]) among `n` samples: `max(1, ⌈ppm · n / 10^6⌉)`, 1-based; 0 when
/// `n` is 0.
#[must_use]
pub const fn nearest_rank(n: u64, ppm: u32) -> u64 {
    if n == 0 {
        return 0;
    }
    let ppm = if ppm > PPM { PPM } else { ppm };
    let num = n as u128 * ppm as u128;
    let r = num.div_ceil(PPM as u128) as u64;
    if r == 0 { 1 } else { r }
}

/// Count, extremes, mean and percentiles of a histogram (all ns; zero when
/// `count` is 0).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct HistogramSummary {
    /// Number of samples.
    pub count: u64,
    /// Smallest sample (exact).
    pub min: u64,
    /// Largest sample (exact).
    pub max: u64,
    /// Mean sample, rounded down (the sum saturates at `u64::MAX`).
    pub mean: u64,
    /// Median (nearest rank, see the module docs for the error bound).
    pub p50: u64,
    /// 99th percentile.
    pub p99: u64,
    /// 99.9th percentile.
    pub p99_9: u64,
    /// Samples above [`MAX_TRACKED_NS`] (counted in the top bucket).
    pub saturated: u64,
}

impl HistogramSummary {
    /// Whether at least one sample was recorded.
    #[must_use]
    pub const fn has_samples(&self) -> bool {
        self.count > 0
    }
}

/// The percentile of quantile `ppm` from a bucket-count array (`counts`
/// totalling `total`, largest sample `max`), as described in the module
/// docs; `None` when `total` is 0.
#[must_use]
pub fn percentile_from_counts(counts: &[u64], total: u64, max: u64, ppm: u32) -> Option<u64> {
    if total == 0 {
        return None;
    }
    let rank = nearest_rank(total, ppm);
    let mut seen = 0u64;
    for (i, &c) in counts.iter().enumerate() {
        seen = seen.saturating_add(c);
        if seen >= rank {
            return Some(if i + 1 == BUCKETS {
                max
            } else {
                bucket_high(i).min(max)
            });
        }
    }
    // A concurrent writer added samples between the count and the walk:
    // the rank lies beyond what was read; the maximum is the safe answer.
    Some(max)
}

/// The fixed-capacity log-bucket histogram (see the module docs).
pub struct LatencyHistogram {
    buckets: Box<[AtomicU64]>,
    min: AtomicU64,
    max: AtomicU64,
    sum: AtomicU64,
    saturated: AtomicU64,
}

impl Default for LatencyHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for LatencyHistogram {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LatencyHistogram")
            .field("summary", &self.summary())
            .finish()
    }
}

impl LatencyHistogram {
    /// An empty histogram; allocates its [`BUCKETS`] counters once, here.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buckets: (0..BUCKETS).map(|_| AtomicU64::new(0)).collect(),
            min: AtomicU64::new(u64::MAX),
            max: AtomicU64::new(0),
            sum: AtomicU64::new(0),
            saturated: AtomicU64::new(0),
        }
    }

    /// Records one sample, ns. O(1), lock-free, allocation-free.
    #[inline]
    pub fn record(&self, ns: u64) {
        self.buckets[bucket_index(ns)].fetch_add(1, Ordering::Relaxed);
        self.min.fetch_min(ns, Ordering::Relaxed);
        self.max.fetch_max(ns, Ordering::Relaxed);
        // Saturating sum: one writer per histogram (the profiler's
        // single-writer handles), so load + store cannot lose an update
        // from another writer; a second writer could only make the mean
        // approximate, never unsound.
        let s = self.sum.load(Ordering::Relaxed);
        self.sum.store(s.saturating_add(ns), Ordering::Relaxed);
        if ns > MAX_TRACKED_NS {
            self.saturated.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Number of samples (the sum of the bucket counters).
    #[must_use]
    pub fn count(&self) -> u64 {
        self.buckets
            .iter()
            .map(|b| b.load(Ordering::Relaxed))
            .fold(0u64, u64::saturating_add)
    }

    /// The value at quantile `ppm` (parts per million; see the module docs),
    /// or `None` without samples.
    #[must_use]
    pub fn value_at_ppm(&self, ppm: u32) -> Option<u64> {
        let mut counts = [0u64; BUCKETS];
        let total = self.load_counts(&mut counts);
        percentile_from_counts(&counts, total, self.max.load(Ordering::Relaxed), ppm)
    }

    fn load_counts(&self, counts: &mut [u64; BUCKETS]) -> u64 {
        let mut total = 0u64;
        for (c, b) in counts.iter_mut().zip(self.buckets.iter()) {
            *c = b.load(Ordering::Relaxed);
            total = total.saturating_add(*c);
        }
        total
    }

    /// Count, extremes, mean, p50 / p99 / p99.9 from one read of the
    /// counters.
    #[must_use]
    pub fn summary(&self) -> HistogramSummary {
        let mut counts = [0u64; BUCKETS];
        let total = self.load_counts(&mut counts);
        if total == 0 {
            return HistogramSummary::default();
        }
        let max = self.max.load(Ordering::Relaxed);
        let min = self.min.load(Ordering::Relaxed).min(max);
        let p = |ppm| percentile_from_counts(&counts, total, max, ppm).unwrap_or(max);
        HistogramSummary {
            count: total,
            min,
            max,
            mean: self.sum.load(Ordering::Relaxed) / total,
            p50: p(P50_PPM),
            p99: p(P99_PPM),
            p99_9: p(P99_9_PPM),
            saturated: self.saturated.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn exact(sorted: &[u64], ppm: u32) -> u64 {
        sorted[(nearest_rank(sorted.len() as u64, ppm) - 1) as usize]
    }

    fn within_bound(exact: u64, got: u64) -> bool {
        got >= exact && got <= exact + (exact >> SUB_BUCKET_BITS)
    }

    #[test]
    fn layout_constants() {
        assert_eq!(BUCKETS, 3840);
        assert_eq!(bucket_index(MAX_TRACKED_NS), BUCKETS - 1);
        assert_eq!(bucket_index(u64::MAX), BUCKETS - 1);
        assert_eq!(bucket_high(BUCKETS - 1), MAX_TRACKED_NS);
        assert_eq!(bucket_low(0), 0);
        assert_eq!(bucket_high(255), 255);
        assert_eq!(bucket_low(256), 256);
        assert_eq!(bucket_high(256), 257);
        assert!((RELATIVE_ERROR_BOUND - 0.0078125).abs() < 1e-12);
    }

    #[test]
    fn buckets_tile_the_range_without_gaps() {
        for i in 0..BUCKETS - 1 {
            assert_eq!(bucket_high(i) + 1, bucket_low(i + 1), "bucket {i}");
            assert_eq!(bucket_index(bucket_low(i)), i);
            assert_eq!(bucket_index(bucket_high(i)), i);
        }
    }

    #[test]
    fn nearest_rank_rule() {
        assert_eq!(nearest_rank(0, P50_PPM), 0);
        assert_eq!(nearest_rank(1, 0), 1);
        assert_eq!(nearest_rank(1, P99_9_PPM), 1);
        assert_eq!(nearest_rank(10, P50_PPM), 5);
        assert_eq!(nearest_rank(1000, P99_9_PPM), 999);
        assert_eq!(nearest_rank(1001, P99_9_PPM), 1000);
        assert_eq!(nearest_rank(7, PPM + 5), 7);
    }

    #[test]
    fn empty_histogram() {
        let h = LatencyHistogram::new();
        assert_eq!(h.count(), 0);
        assert_eq!(h.value_at_ppm(P50_PPM), None);
        assert_eq!(h.summary(), HistogramSummary::default());
        assert!(!h.summary().has_samples());
        assert!(format!("{h:?}").contains("count: 0"));
    }

    #[test]
    fn single_sample_is_every_percentile() {
        for v in [
            0u64,
            1,
            255,
            256,
            349_999,
            350_000,
            1_500_000,
            MAX_TRACKED_NS,
        ] {
            let h = LatencyHistogram::default();
            h.record(v);
            let s = h.summary();
            assert_eq!(s.count, 1);
            assert_eq!((s.min, s.max, s.mean), (v, v, v));
            assert_eq!((s.p50, s.p99, s.p99_9), (v, v, v), "{v}");
        }
    }

    #[test]
    fn top_bucket_saturates_and_reports_the_exact_max() {
        let h = LatencyHistogram::new();
        h.record(10);
        h.record(MAX_TRACKED_NS + 1);
        h.record(u64::MAX);
        let s = h.summary();
        assert_eq!(s.count, 3);
        assert_eq!(s.saturated, 2);
        assert_eq!(s.max, u64::MAX);
        assert_eq!(s.p99_9, u64::MAX);
        assert_eq!(s.p50, u64::MAX, "rank 2 of 3 is in the top bucket");
        assert_eq!(s.min, 10);
        assert_eq!(s.mean, u64::MAX / 3, "the sum saturates");
    }

    #[test]
    fn small_values_are_exact() {
        let h = LatencyHistogram::new();
        for v in 0..256u64 {
            h.record(v);
        }
        assert_eq!(h.value_at_ppm(P50_PPM), Some(127));
        assert_eq!(h.value_at_ppm(PPM), Some(255));
        assert_eq!(h.value_at_ppm(0), Some(0));
    }

    #[test]
    fn concurrent_writer_overrun_answers_max() {
        // Counts that do not reach the rank (as when a writer races the
        // walk) answer the maximum.
        let counts = [0u64; BUCKETS];
        assert_eq!(percentile_from_counts(&counts, 5, 42, P50_PPM), Some(42));
        assert_eq!(percentile_from_counts(&counts, 0, 42, P50_PPM), None);
    }

    proptest! {
        #[test]
        fn index_low_high_roundtrip(v in any::<u64>()) {
            let i = bucket_index(v);
            prop_assert!(i < BUCKETS);
            if v <= MAX_TRACKED_NS {
                prop_assert!(bucket_low(i) <= v && v <= bucket_high(i));
                // The width bound behind the relative-error bound.
                let w = bucket_high(i) - bucket_low(i);
                prop_assert!(w <= bucket_low(i) >> SUB_BUCKET_BITS);
            }
        }

        #[test]
        fn percentiles_within_bound_of_exact(
            mut v in prop::collection::vec(0u64..=MAX_TRACKED_NS, 1..400),
            scale in 0u32..36,
        ) {
            for x in &mut v {
                *x >>= scale;
            }
            let h = LatencyHistogram::new();
            for &x in &v {
                h.record(x);
            }
            let mut sorted = v.clone();
            sorted.sort_unstable();
            for ppm in [0, 1, P50_PPM, 900_000, P99_PPM, P99_9_PPM, PPM] {
                let got = h.value_at_ppm(ppm).unwrap();
                let want = exact(&sorted, ppm);
                prop_assert!(within_bound(want, got), "ppm {} exact {} got {}", ppm, want, got);
            }
            let s = h.summary();
            prop_assert_eq!(s.count, v.len() as u64);
            prop_assert_eq!(s.min, sorted[0]);
            prop_assert_eq!(s.max, *sorted.last().unwrap());
            let sum: u128 = v.iter().map(|&x| u128::from(x)).sum();
            prop_assert_eq!(u128::from(s.mean), sum / v.len() as u128);
            prop_assert!(s.min <= s.p50 && s.p50 <= s.p99 && s.p99 <= s.p99_9 && s.p99_9 <= s.max);
        }

        #[test]
        fn order_of_records_does_not_matter(
            v in prop::collection::vec(any::<u64>(), 1..200),
            seed in any::<u64>(),
        ) {
            let a = LatencyHistogram::new();
            for &x in &v {
                a.record(x);
            }
            // A seeded permutation (Fisher-Yates on a small LCG).
            let mut p = v.clone();
            let mut s = seed | 1;
            for i in (1..p.len()).rev() {
                s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
                p.swap(i, (s >> 33) as usize % (i + 1));
            }
            let b = LatencyHistogram::new();
            for &x in &p {
                b.record(x);
            }
            prop_assert_eq!(a.summary(), b.summary());
        }

        #[test]
        fn percentiles_are_monotonic_in_q(
            v in prop::collection::vec(any::<u64>(), 1..300),
            q in prop::collection::vec(0u32..=PPM, 2..20),
        ) {
            let h = LatencyHistogram::new();
            for &x in &v {
                h.record(x);
            }
            let mut q = q;
            q.sort_unstable();
            let vals: Vec<u64> = q.iter().map(|&p| h.value_at_ppm(p).unwrap()).collect();
            prop_assert!(vals.windows(2).all(|w| w[0] <= w[1]), "{:?} -> {:?}", q, vals);
            prop_assert!(*vals.last().unwrap() <= h.summary().max);
        }

        #[test]
        fn nearest_rank_is_in_range(n in 1u64..1_000_000_000, ppm in 0u32..=PPM) {
            let r = nearest_rank(n, ppm);
            prop_assert!(r >= 1 && r <= n);
            // ⌈q·n⌉: r − 1 < q·n ≤ r.
            let qn = u128::from(n) * u128::from(ppm);
            prop_assert!(u128::from(r) * u128::from(PPM) >= qn);
            prop_assert!(r == 1 || u128::from(r - 1) * u128::from(PPM) < qn);
        }
    }
}
