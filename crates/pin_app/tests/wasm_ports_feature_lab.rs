//! 6.3: the Wasm port of the Feature Lab rules
//! (`tables/feature_lab/rules-wasm`, committed as `rules.wasm`) behaves
//! exactly as the frozen `rules.lua`.
//!
//! * A recorded played game (mode selector steps on `action` included)
//!   replays tick for tick to identical simulation output under the Lua and
//!   the Wasm bundle, with identical final scores and `state_hash`, while
//!   the two backends, fed the game's switch edges, emit byte-identical
//!   command queues every tick.
//! * Seeded random switch traffic gives byte-identical queues and identical
//!   PCG draws (the `on_init` mode-selector draw) for several seeds, and
//!   starts every one of the five modes.

mod feature_lab_support;
mod wasm_ports_support;

use feature_lab_support::{MODES, bundle, scratch};
use pin_sim::{SimConfig, Simulation};
use std::path::PathBuf;
use wasm_ports_support::{
    Lockstep, RandomEdges, key, on_big_stack, played_game_parity, wasm_bundle,
};

const TABLE: &str = "feature_lab";

fn bundles() -> (PathBuf, PathBuf) {
    let lua = bundle().to_path_buf();
    let wasm = scratch("feature_lab_wasm_bundle");
    wasm_bundle(&lua, TABLE, &wasm);
    (lua, wasm)
}

#[test]
fn a_recorded_game_matches_tick_for_tick_under_lua_and_wasm() {
    let (lua, wasm) = bundles();
    on_big_stack(move || {
        let keys = MODES.map(|m| format!("mode:{m}"));
        let keys: Vec<&str> = keys.iter().map(String::as_str).collect();
        let (r, busy, commands) = played_game_parity(TABLE, &lua, &wasm, 7000, 300_000, &keys);
        println!("{r:?}; lockstep busy ticks {busy}, commands {commands}");
        assert_eq!(r.scores.0, r.scores.1, "final scores");
        assert_eq!(r.hashes.0, r.hashes.1, "final state_hash");
        assert!(r.scores.1.is_some_and(|s| s > 0), "the Wasm game scored");
        assert!(r.max_ball >= 2, "the Wasm game ran past ball 1");
        assert!(r.ticks_with_output > 100 && busy > 100 && commands > 500);
        assert!(r.games >= 2, "several games were played");
        let started = r.modes.iter().filter(|(_, n)| *n > 0).count();
        assert!(started >= 2, "several modes started in the played games");
    });
}

#[test]
fn random_switch_traffic_gives_identical_queues_and_draws() {
    let lua = bundle().to_path_buf();
    on_big_stack(move || {
        let sim = Simulation::load(&lua, SimConfig::default()).expect("load");
        let mut sels = Vec::new();
        for seed in [0x6_3C_u64, 1, 2, 3, 4] {
            let mut ls = Lockstep::new(&sim, &lua, TABLE, seed);
            assert!(ls.rng_moves >= 1, "on_init draws the mode selector");
            sels.push(ls.view().persist[usize::from(key(&sim, "mode_sel"))]);
            let mut edges = RandomEdges::new(&sim, seed, 10, 40_000);
            let ticks = if seed == 0x6_3C { 400_000 } else { 20_000 };
            for t in 1..=ticks {
                edges.step(&mut ls, t);
            }
            if seed == 0x6_3C {
                let v = ls.view();
                let p = |n: &str| v.persist[usize::from(key(&sim, n))];
                let reached = MODES.map(|m| (m, p(&format!("mode:{m}"))));
                println!(
                    "{reached:?}; games {}; busy ticks {}, commands {}",
                    p("games_played"),
                    ls.busy_ticks,
                    ls.commands
                );
                for (m, c) in reached {
                    assert!(c > 0, "mode {m} never started: the traffic missed it");
                }
            }
        }
        println!("mode_sel drawn per seed: {sels:?}");
    });
}
