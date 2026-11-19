//! 6.3: the Wasm port of the Apex Circuit rules
//! (`tables/apex_circuit/rules-wasm`, committed as `rules.wasm`) behaves
//! exactly as the frozen `rules.lua`.
//!
//! * A recorded played game replays tick for tick to identical simulation
//!   output under the Lua and the Wasm bundle, with identical final scores
//!   and `state_hash`, and byte-identical command queues in the two
//!   simulations after every tick (`Simulation::last_commands`).
//! * Seeded random switch traffic (every switch, both edges, games started
//!   over and over) gives byte-identical queues and PCG draws, and reaches
//!   every mode and award the rules have.

mod apex_support;
mod wasm_ports_support;

use apex_support::apex_bundle;
use pin_sim::{SimConfig, Simulation};
use std::path::PathBuf;
use wasm_ports_support::*;

const TABLE: &str = "apex_circuit";

/// The Lua bundle and a Wasm copy of it; the copy is removed when the
/// returned [`TempDir`] drops (keep it alive through the replay).
fn bundles() -> (PathBuf, TempDir) {
    let lua = apex_bundle().to_path_buf();
    let wasm = TempDir::new("apex_wasm_bundle");
    wasm_bundle(&lua, TABLE, wasm.path());
    (lua, wasm)
}

#[test]
fn a_recorded_game_matches_tick_for_tick_under_lua_and_wasm() {
    let (lua, wasm_dir) = bundles();
    on_big_stack(move || {
        let wasm = wasm_dir.path();
        let r = played_game_parity(
            &lua,
            wasm,
            9000,
            300_000,
            &["mode:combo_chain", "mode:powerball_mania"],
        );
        println!("{r:?}");
        assert_eq!(r.scores.0, r.scores.1, "final scores");
        assert_eq!(r.hashes.0, r.hashes.1, "final state_hash");
        assert!(r.scores.1.is_some_and(|s| s > 0), "the Wasm game scored");
        assert!(r.max_ball >= 2, "the Wasm game ran past ball 1");
        assert!(r.ticks_with_output > 100 && r.queue_busy_ticks > 100 && r.queue_commands > 500);
        assert!(r.games >= 2, "several games were played");
        for (m, n) in &r.modes {
            assert!(*n > 0, "{m} never started in the played games");
        }
    });
}

#[test]
fn random_switch_traffic_gives_identical_queues() {
    let lua = apex_bundle().to_path_buf();
    on_big_stack(move || {
        let sim = Simulation::load(&lua, SimConfig::default()).expect("load");
        let mut ls = Lockstep::new(&sim, &lua, TABLE, 0x6_3A);
        let mut edges = RandomEdges::new(&sim, 0x6_3A, 8, 50_000);
        for t in 1..=1_000_000 {
            edges.step(&mut ls, t);
        }
        let v = ls.view();
        let p = |n: &str| v.persist[usize::from(key(&sim, n))];
        let reached = [
            "games_played",
            "mode:combo_chain",
            "mode:powerball_mania",
            "skill_shot_count",
            "firewall_save_count",
            "apex_jackpot_count",
        ]
        .map(|n| (n, p(n)));
        println!(
            "{reached:?}; busy ticks {}, commands {}",
            ls.busy_ticks, ls.commands
        );
        for (n, c) in reached {
            assert!(c > 0, "{n} never counted up: the traffic missed a path");
        }
    });
}
