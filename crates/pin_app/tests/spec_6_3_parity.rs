//! Subphase 6.3 spec tests (Independent Tester): Lua / Wasm parity of the
//! table rules on recorded games (brief requirements 1, 2, 3 and 5).
//!
//! For each table (Apex Circuit, Sky Deck, Feature Lab) a Lua bundle and a
//! Wasm bundle (a copy with the committed `rules.wasm` and
//! `script_entry = "rules.wasm"`) run the same deterministic recorded game
//! (a fixed rhythm, or a seeded PCG input stream) under three sim seeds.
//! Every tick the drained command queues of both backends must be
//! identical (the command layer of `spec_6_3_support`), as must every
//! observable of the two simulations (state hash, scenes, sounds, scores,
//! haptics, lamps, persist values, ...); no script ever faults. Each game
//! must reach ball 2 and start a rule mode, issue pulses, lamps, scores,
//! sounds and scenes, and score.

mod spec_6_3_support;

use spec_6_3_support::*;

/// Ticks of every recorded parity game (long enough for ball 1 to end, a
/// second ball and a mode start on every table).
const GAME_TICKS: u64 = 120_000;

/// The command kinds every run must issue (brief requirement 2).
const REQUIRED_KINDS: [&str; 5] = ["coil_pulses", "lamps", "scores", "sounds", "display_scenes"];

fn check_run(t: Table, seed: u64, game: &Game, r: &ParityReport) {
    let ctx = format!("{t:?} seed {seed:#x} {:?}", game.inputs);
    // Requirement 1: the recorded game covers ball 1 ending, a second ball
    // and a mode start.
    assert!(r.max_ball >= 2, "{ctx}: no second ball ({r:?})");
    assert!(r.stimuli_at.is_some(), "{ctx}: ball 2 never arrived");
    assert!(r.mode_starts >= 1, "{ctx}: no rule mode started ({r:?})");
    // Requirement 2: non-vacuity.
    for k in REQUIRED_KINDS {
        assert!(r.kinds.contains(&k), "{ctx}: no {k} command ({r:?})");
    }
    assert!(
        r.kinds.len() > REQUIRED_KINDS.len(),
        "{ctx}: too few distinct command kinds: {:?}",
        r.kinds
    );
    assert!(
        r.scores[0].is_some_and(|s| s > 0),
        "{ctx}: player 1 did not score ({r:?})"
    );
    assert!(!r.persist_keys.is_empty(), "{ctx}: no persist key written");
    assert!(
        r.busy_ticks >= 100,
        "{ctx}: only {} busy ticks",
        r.busy_ticks
    );
    assert_eq!(r.compared_ticks, game.ticks + 1, "{ctx}: ticks compared");
}

/// Runs `game` under every seed of [`SEEDS`] and checks each run; the
/// final hashes of different seeds must not all coincide (the seed
/// reaches the simulation).
fn parity_all_seeds(t: Table, game: Game) {
    on_big_stack(move || {
        let mut hashes = Vec::new();
        for seed in SEEDS {
            let r = run_parity(t, seed, &game);
            check_run(t, seed, &game, &r);
            hashes.push(r.state_hash);
        }
        hashes.sort_unstable();
        hashes.dedup();
        assert!(hashes.len() > 1, "{t:?}: every seed gave the same game");
    });
}

fn rhythm() -> Game {
    Game {
        inputs: Inputs::Rhythm,
        ticks: GAME_TICKS,
        stimuli: true,
    }
}

fn random(stream: u64) -> Game {
    Game {
        inputs: Inputs::Random(stream),
        ticks: GAME_TICKS,
        stimuli: true,
    }
}

#[test]
fn apex_rhythm_game_lua_and_wasm_agree_every_tick() {
    parity_all_seeds(Table::Apex, rhythm());
}

#[test]
fn apex_random_game_lua_and_wasm_agree_every_tick() {
    parity_all_seeds(Table::Apex, random(0xa9e1));
}

#[test]
fn sky_deck_rhythm_game_lua_and_wasm_agree_every_tick() {
    parity_all_seeds(Table::SkyDeck, rhythm());
}

#[test]
fn sky_deck_random_game_lua_and_wasm_agree_every_tick() {
    parity_all_seeds(Table::SkyDeck, random(0x5d3c));
}

#[test]
fn feature_lab_rhythm_game_lua_and_wasm_agree_every_tick() {
    parity_all_seeds(Table::FeatureLab, rhythm());
}

#[test]
fn feature_lab_random_game_lua_and_wasm_agree_every_tick() {
    parity_all_seeds(Table::FeatureLab, random(0xfe1a));
}

/// The harness itself: a queue that differs in one float bit, or in the
/// order of two commands, is reported as different (the comparison is
/// strict enough to catch a port that reorders or rounds).
#[test]
fn queue_comparison_is_strict() {
    use pin_core::script::HostCommandQueue;
    use pin_core::{CoilId, LampId};
    let mut a = HostCommandQueue::default();
    a.pulse_coil(CoilId(1), 20).unwrap();
    a.pulse_coil(CoilId(2), 25).unwrap();
    a.set_lamp(LampId(3), 1.0).unwrap();
    assert!(same_queue(&a, &a.clone()));
    let mut b = HostCommandQueue::default();
    b.pulse_coil(CoilId(2), 25).unwrap();
    b.pulse_coil(CoilId(1), 20).unwrap();
    b.set_lamp(LampId(3), 1.0).unwrap();
    assert!(!same_queue(&a, &b), "order matters");
    let mut c = a.clone();
    c.lamps[0].1 = f32::from_bits(1.0f32.to_bits() + 1);
    assert!(!same_queue(&a, &c), "float bits matter");
    let mut d = a.clone();
    d.lamps[0].1 = -0.0;
    let mut e = a.clone();
    e.lamps[0].1 = 0.0;
    assert!(!same_queue(&d, &e), "signed zero matters");
    let mut f = a.clone();
    f.set_gi_tint([1.0; 4]);
    assert!(!same_queue(&a, &f), "GI tint matters");
}

/// The Wasm bundle differs from the Lua bundle only in `rules.wasm` and the
/// rewritten `script_entry`; the source `table.toml` still names
/// `rules.lua` (SPEC).
#[test]
fn wasm_bundle_is_the_lua_bundle_with_the_entry_rewritten() {
    for t in Table::ALL {
        let src = std::fs::read_to_string(t.src().join("table.toml")).unwrap();
        let m = pin_core::schema::table::TableManifest::from_toml_str(&src).unwrap();
        assert_eq!(
            m.meta.script_entry, "rules.lua",
            "{t:?}: source script_entry"
        );
        let w = wasm_bundle(t);
        let l = lua_bundle(t);
        assert_eq!(
            std::fs::read(w.join("rules.wasm")).unwrap(),
            committed_wasm(t),
            "{t:?}"
        );
        assert_eq!(
            std::fs::read(w.join("rules.lua")).unwrap(),
            std::fs::read(l.join("rules.lua")).unwrap()
        );
    }
}

#[test]
fn rewrite_script_entry_touches_one_line() {
    let s = "a = 1\nscript_entry = \"rules.lua\"\nb = \"script_entry\"\n";
    assert_eq!(
        rewrite_script_entry(s, "rules.wasm"),
        "a = 1\nscript_entry = \"rules.wasm\"\nb = \"script_entry\"\n"
    );
}
