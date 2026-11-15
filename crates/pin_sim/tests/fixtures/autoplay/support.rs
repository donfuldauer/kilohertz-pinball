//! Shared helpers of the `autoplay_*` tests (included with `#[path]`).
#![allow(dead_code)]

use pin_sim::autoplay::{SkillLevel, SoakConfig};
use pin_sim::cli::SIM_THREAD_STACK_BYTES;
use std::path::{Path, PathBuf};

/// The fixture bundle `name` under `tests/fixtures/autoplay/`.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/autoplay")
        .join(name)
}

/// Runs `f` on a thread with the simulation stack size.
pub fn on_sim_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(SIM_THREAD_STACK_BYTES)
        .spawn(f)
        .expect("spawn")
        .join()
        .expect("sim thread panicked")
}

/// A soak of `games` games with `seed`, Standard skill, one thread, a
/// tick limit of `max_ticks` and no wall-clock budget check.
pub fn cfg(games: u32, seed: u64, max_ticks: u64) -> SoakConfig {
    SoakConfig {
        games,
        seed,
        skill: SkillLevel::Standard,
        threads: 1,
        max_game_ticks: max_ticks,
        check_tick_budget: false,
        ..SoakConfig::default()
    }
}

/// A fresh scratch folder under the target dir.
pub fn scratch(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("autoplay_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}
