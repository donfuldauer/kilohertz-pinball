//! Spike 0.1/Q7 support: smoke checks for the utility crates pinned in
//! `versions.toml` that no other spike exercises.

use arrayvec::ArrayVec;
use rand_core::{Rng, SeedableRng};
use rand_pcg::Pcg64Mcg;
use serde::{Deserialize, Serialize};

/// A small config-like struct for the serde/toml round trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub name: String,
    pub tilt_threshold: f32,
    pub balls: u8,
    pub switches: Vec<u16>,
}

/// First `N` outputs of a seeded PCG stream.
pub fn pcg_stream<const N: usize>(seed: u64) -> [u64; N] {
    let mut rng = Pcg64Mcg::seed_from_u64(seed);
    std::array::from_fn(|_| rng.next_u64())
}

/// Fixed-capacity collection without heap allocation.
pub fn first_four(xs: &[u32]) -> ArrayVec<u32, 4> {
    xs.iter().copied().take(4).collect()
}
