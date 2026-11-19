//! 6.3: the Wasm port of the Sky Deck rules (`tables/sky_deck/rules-wasm`,
//! committed as `rules.wasm`) behaves exactly as the frozen `rules.lua`.
//!
//! * A recorded played game (Sky Bridge presses on `action` included)
//!   replays tick for tick to identical simulation output under the Lua and
//!   the Wasm bundle, with identical final scores and `state_hash`, and
//!   byte-identical command queues in the two simulations after every tick
//!   (`Simulation::last_commands`).
//! * Seeded random switch traffic gives byte-identical queues and reaches
//!   Night Overdrive (the UV GI tint, the Glo-Ball) and the Sky Bridge.

mod sky_deck_support;
mod wasm_ports_support;

use pin_sim::{SimConfig, Simulation};
use sky_deck_support::sky_deck_bundle;
use std::path::PathBuf;
use wasm_ports_support::{
    Lockstep, RandomEdges, TempDir, key, on_big_stack, played_game_parity, wasm_bundle,
};

const TABLE: &str = "sky_deck";

/// The Lua bundle and a Wasm copy of it; the copy is removed when the
/// returned [`TempDir`] drops (keep it alive through the replay).
fn bundles() -> (PathBuf, TempDir) {
    let lua = sky_deck_bundle().to_path_buf();
    let wasm = TempDir::new("sky_deck_wasm_bundle");
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
            6000,
            300_000,
            &["mode:night_overdrive", "bridge_open"],
        );
        println!("{r:?}");
        assert_eq!(r.scores.0, r.scores.1, "final scores");
        assert_eq!(r.hashes.0, r.hashes.1, "final state_hash");
        assert!(r.scores.1.is_some_and(|s| s > 0), "the Wasm game scored");
        assert!(r.max_ball >= 2, "the Wasm game ran past ball 1");
        assert!(r.ticks_with_output > 100 && r.queue_busy_ticks > 100 && r.queue_commands > 500);
        assert!(r.games >= 2, "several games were played");
        assert!(
            r.modes.iter().any(|(m, n)| m == "bridge_open" && *n == 1),
            "the Sky Bridge opened in the played games"
        );
    });
}

#[test]
fn random_switch_traffic_gives_identical_queues() {
    let lua = sky_deck_bundle().to_path_buf();
    on_big_stack(move || {
        let sim = Simulation::load(&lua, SimConfig::default()).expect("load");
        let mut ls = Lockstep::new(&sim, &lua, TABLE, 0x6_3B);
        let mut edges = RandomEdges::new(&sim, 0x6_3B, 12, 50_000);
        let k_bridge = usize::from(key(&sim, "bridge_open"));
        let mut bridge_opened = false;
        for t in 1..=300_000 {
            edges.step(&mut ls, t);
            bridge_opened |= ls.view().persist[k_bridge] == 1;
        }
        let v = ls.view();
        let p = |n: &str| v.persist[usize::from(key(&sim, n))];
        let reached = ["games_played", "mode:night_overdrive", "high_score"].map(|n| (n, p(n)));
        println!(
            "{reached:?}; bridge opened {bridge_opened}; busy ticks {}, commands {}",
            ls.busy_ticks, ls.commands
        );
        for (n, c) in reached {
            assert!(c > 0, "{n} never counted up: the traffic missed a path");
        }
        assert!(bridge_opened, "the Sky Bridge never opened");
    });
}
