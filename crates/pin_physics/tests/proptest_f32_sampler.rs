//! Regression guard for proptest 1.12.0's faulty `debug_assert!` in
//! `FloatUniform::sample` (`self.low - result < initial.step` is wrong when the
//! split-interval step is negative). The workspace `Cargo.toml` turns debug assertions
//! off for the `proptest` package only (DECISIONS "w7pre-proptest-f32"); this test fails
//! with a panic if that override is removed while the upstream defect is still present.
//!
//! The seeds were found by an offline scan (one sample per ChaCha seed whose first 8
//! bytes are the little-endian counter): both ranges panic at these seeds in a build
//! with proptest debug assertions on. The clamp after the assert keeps every value in
//! `[low, high)`, which is what the test asserts.

use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

const TRIGGER_SEEDS: [u64; 2] = [25_354_464, 28_756_204];

fn sample_at(seed: u64, range: std::ops::Range<f32>) -> f32 {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&seed.to_le_bytes());
    let rng = TestRng::from_seed(RngAlgorithm::ChaCha, &bytes);
    let mut runner = TestRunner::new_with_rng(Config::default(), rng);
    range.new_tree(&mut runner).unwrap().current()
}

#[test]
fn f32_range_sampler_survives_known_trigger_seeds() {
    for &seed in &TRIGGER_SEEDS {
        for (low, high) in [(0.001f32, 0.02f32), (0.05, 0.3)] {
            let v = sample_at(seed, low..high);
            assert!(
                (low..high).contains(&v),
                "seed {seed}: {v} outside [{low}, {high})"
            );
        }
    }
}
