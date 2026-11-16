//! Gate A1 endless-ball fix, loop 1 (docs/DECISIONS.md "a1-endless-table —
//! loop 1: VUK exit offset"): a steel ball ejected by the VUK must not ride
//! the deck, the deck-exit guide, bumpers 2/3, the right wall and the
//! top-right corner straight back into the scoop for ever.
//!
//! * From the VUK's eject state (the scoop position plus its `exit_offset`,
//!   on its exit layer, zero velocity and spin, read from the built
//!   bundle's `table.toml`), with a game running, no flipper input and a
//!   fixed sim seed, the ball drains within 120 simulated seconds and enters
//!   the VUK at most once more on the way (a re-entry is ejected by the
//!   rules as in play).
//! * Sweep: the same run with the exit x offset at 263..=267 mm (a modified
//!   copy of the table) prints how many drain. The fix is a knife-edge
//!   relief (DECISIONS); the brief lets the sweep gate only if all five
//!   drain, which they did when it was written (5 of 5; at 270 mm, the old
//!   offset, the ball re-entered the VUK 51 times in 120 s and never
//!   drained), so it requires every one to drain.
//!
//! Simulated time only (rule 5): fixed tick counts, state injection through
//! the public `Simulation` API.

mod support;

use pin_core::schema::table::{MechKind, TableManifest};
use pin_sim::{SimConfig, Simulation};
use pin_tablegen::generate;
use pin_tablegen::layout::parse_layout;
use std::path::Path;
use support::{build_greybox_bundle, greybox_bundle, greybox_src, on_big_stack, scratch};

/// 120 simulated seconds at 1 kHz.
const LIMIT_TICKS: u64 = 120_000;

/// Sim seed of every run (the script's PCG; the physics is deterministic).
const SEED: u64 = 1;

/// Exit x offsets of the informational sweep, mm.
const SWEEP_X_MM: [f32; 5] = [263.0, 264.0, 265.0, 266.0, 267.0];

/// The VUK's scoop position, exit offset (mm) and exit layer in a manifest.
fn vuk_eject(table: &TableManifest) -> ([f32; 3], [f32; 3], u8) {
    let m = table
        .mechs
        .iter()
        .find(|m| matches!(m.kind, MechKind::Vuk))
        .expect("the grey-box has a VUK");
    (
        m.position.expect("VUK position"),
        m.exit_offset.expect("VUK exit_offset"),
        m.exit_layer.expect("VUK exit_layer"),
    )
}

/// How the ejected ball's run ended.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Run {
    /// Tick (after the spawn) at which it drained, if it did.
    drained_at: Option<u64>,
    /// Closing edges of the VUK switch after the spawn.
    vuk_entries: u32,
    /// Where the ball was at the end of the run (mm), if still on the table.
    last_mm: Option<[f32; 3]>,
}

fn press(s: &mut Simulation, name: &str, ticks: u64) {
    s.set_switch(name, true).unwrap();
    s.advance(ticks);
    s.release_switch(name).unwrap();
}

/// Loads `bundle`, starts a game (its first ball waits on the plunger; no
/// launch, no flipper input follows) and puts a steel ball at the VUK's
/// eject state; runs until it drains or [`LIMIT_TICKS`].
fn run_from_eject(bundle: &Path) -> Run {
    let table_text = std::fs::read_to_string(bundle.join("table.toml")).expect("bundle table");
    let table = TableManifest::from_toml_str(&table_text).expect("bundle table parses");
    let (pos, off, layer) = vuk_eject(&table);
    let cfg = SimConfig {
        seed: SEED,
        ..SimConfig::default()
    };
    let mut s = Simulation::load(bundle, cfg).expect("bundle loads");
    s.advance(10);
    press(&mut s, "start", 5);
    s.advance(1500);
    assert_eq!(
        s.switch_closed("plunger_tip"),
        Some(true),
        "the served ball waits on the plunger"
    );
    let vuk0 = s.switch_hits("vuk_sw").unwrap();
    let drained0 = s.counters().balls_drained;
    let escaped0 = s.counters().balls_escaped;
    let p = [
        (pos[0] + off[0]) * 1.0e-3,
        (pos[1] + off[1]) * 1.0e-3,
        (pos[2] + off[2]) * 1.0e-3,
    ];
    let h = s
        .spawn_ball_on_layer("standard_steel", layer, p, [0.0; 3])
        .expect("spawn at the eject point");
    let mut drained_at = None;
    for t in 1..=LIMIT_TICKS {
        s.advance(1);
        if s.ball(h).is_none() {
            assert_eq!(
                s.counters().balls_escaped,
                escaped0,
                "the ball escaped instead of draining"
            );
            assert!(
                s.counters().balls_drained > drained0,
                "the ball left the table without draining"
            );
            drained_at = Some(t);
            break;
        }
    }
    assert!(!s.script_faulted(), "script fault: {:?}", s.script_error());
    Run {
        drained_at,
        vuk_entries: s.switch_hits("vuk_sw").unwrap() - vuk0,
        last_mm: s.ball(h).map(|b| b.position_m.map(|c| c * 1.0e3)),
    }
}

#[test]
fn steel_ball_ejected_by_the_vuk_drains_within_120_s() {
    on_big_stack(|| {
        let run = run_from_eject(greybox_bundle());
        println!("eject at the table's exit offset: {run:?}");
        assert!(
            run.drained_at.is_some(),
            "the ejected ball did not drain within {LIMIT_TICKS} ticks: {run:?}"
        );
        assert!(
            run.vuk_entries <= 1,
            "the ejected ball entered the VUK {} times (at most once more allowed): {run:?}",
            run.vuk_entries
        );
    });
}

/// Builds the grey-box bundle into a scratch folder with the VUK's exit x
/// offset replaced by `x_mm`.
fn bundle_with_exit_x(x_mm: f32) -> std::path::PathBuf {
    let dir = scratch("vuk_exit_x");
    build_greybox_bundle(&dir);
    let src = greybox_src();
    let layout_text = std::fs::read_to_string(src.join("layout.toml")).unwrap();
    let table_text = std::fs::read_to_string(src.join("table.toml")).unwrap();
    let layout = parse_layout(&layout_text).unwrap();
    let mut base = TableManifest::from_toml_str(&table_text).unwrap();
    let vuk = base
        .mechs
        .iter_mut()
        .find(|m| matches!(m.kind, MechKind::Vuk))
        .unwrap();
    let mut off = vuk.exit_offset.unwrap();
    off[0] = x_mm;
    vuk.exit_offset = Some(off);
    let generated = generate(&layout, &base).unwrap_or_else(|e| panic!("pin_tablegen: {e}"));
    pin_tablegen::write_bundle(&generated, &dir).unwrap();
    dir
}

/// How many exit x offsets in [`SWEEP_X_MM`] let the ejected ball drain
/// within 120 s, entering the VUK at most once more: all of them.
#[test]
fn exit_x_sweep_263_to_267_mm_drains_at_every_offset() {
    on_big_stack(|| {
        let mut drained = 0;
        let mut failed = Vec::new();
        for x in SWEEP_X_MM {
            let dir = bundle_with_exit_x(x);
            let run = run_from_eject(&dir);
            println!("exit x {x} mm: {run:?}");
            if run.drained_at.is_some() && run.vuk_entries <= 1 {
                drained += 1;
            } else {
                failed.push((x, run));
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
        println!(
            "VUK exit x sweep: {drained} of {} drain within 120 s",
            SWEEP_X_MM.len()
        );
        assert!(failed.is_empty(), "exit x offsets that loop: {failed:?}");
    });
}
