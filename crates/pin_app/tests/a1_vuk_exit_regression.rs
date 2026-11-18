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
//! * Eject scatter (w7pre-vuk-scatter, DECISIONS.md "w7pre-vuk-scatter —
//!   deterministic VUK eject scatter"): every eject by the VUK is scattered
//!   from a per-mech PCG derived from the sim seed. The sweep is repeated
//!   over [`SCATTER_SEEDS`] with the old 270 mm offset added (the
//!   scatter-free loop configuration): every case drains within 120 s and
//!   re-enters the VUK at most [`MAX_SCATTERED_REENTRIES`] times. A game
//!   in which the VUK ejects at least 5 times replays identically (two
//!   runs, and a replay of its recording through the file format), and
//!   other seeds scatter its ejects differently.
//!
//! Simulated time only (rule 5): fixed tick counts, state injection through
//! the public `Simulation` API.

mod support;

use pin_core::TICK_NS;
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::schema::table::{MechKind, TableManifest};
use pin_core::sim::SimStep;
use pin_sim::replay::Recording;
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

/// The pre-fix exit x offset (mm), which looped for ever without scatter.
const OLD_X_MM: f32 = 270.0;

/// Sim seeds of the scattered sweep.
const SCATTER_SEEDS: [u64; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

/// Most VUK re-entries allowed in one scattered sweep run. Measured: at
/// most 2 (270 mm; 0 at 263..=267 mm) over seeds 1..=8; the bound is 3x
/// that (DECISIONS.md "w7pre-vuk-scatter"). Without scatter, 270 mm
/// re-entered 51 times in 120 s.
const MAX_SCATTERED_REENTRIES: u32 = 6;

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
    /// LAUNCH presses the rig made for a ball parked at rest (see
    /// [`PARKED_TICKS`]).
    launches: u32,
}

/// A ball at rest (speed below [`PARKED_SPEED`]) for this many ticks is
/// parked (e.g. in the shooter lane behind the served ball): the rig
/// presses LAUNCH, as a player would. Not a VUK loop.
const PARKED_TICKS: u64 = 2_000;

/// Speed (m/s) under which the ball counts as at rest.
const PARKED_SPEED: f32 = 0.01;

fn press(s: &mut Simulation, name: &str, ticks: u64) {
    s.set_switch(name, true).unwrap();
    s.advance(ticks);
    s.release_switch(name).unwrap();
}

/// Loads `bundle`, starts a game (its first ball waits on the plunger; no
/// launch, no flipper input follows) and puts a steel ball at the VUK's
/// eject state; runs until it drains or [`LIMIT_TICKS`]. Sim seed [`SEED`].
fn run_from_eject(bundle: &Path) -> Run {
    run_from_eject_seeded(bundle, SEED)
}

/// [`run_from_eject`] with sim seed `seed`.
fn run_from_eject_seeded(bundle: &Path, seed: u64) -> Run {
    let table_text = std::fs::read_to_string(bundle.join("table.toml")).expect("bundle table");
    let table = TableManifest::from_toml_str(&table_text).expect("bundle table parses");
    let (pos, off, layer) = vuk_eject(&table);
    let cfg = SimConfig {
        seed,
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
    let mut launches = 0;
    let mut still = 0u64;
    let mut t = 0u64;
    while t < LIMIT_TICKS {
        t += 1;
        s.advance(1);
        if let Some(b) = s.ball(h) {
            let v = b.velocity_m_s;
            let speed2 = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
            still = if speed2 < PARKED_SPEED * PARKED_SPEED {
                still + 1
            } else {
                0
            };
            if still >= PARKED_TICKS && t + 50 <= LIMIT_TICKS {
                press(&mut s, "launch", 50);
                t += 50;
                launches += 1;
                still = 0;
            }
        }
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
        launches,
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

/// The scatter-free loop configuration (the old 270 mm) and the 263..=267 mm
/// sweep, each over [`SCATTER_SEEDS`]: with eject scatter every case drains
/// within 120 s and re-enters the VUK at most [`MAX_SCATTERED_REENTRIES`]
/// times (without scatter, 270 mm re-entered 51 times and never drained).
#[test]
fn scattered_ejects_break_the_vuk_loop_at_every_offset_and_seed() {
    on_big_stack(|| {
        let mut failed = Vec::new();
        let mut worst = 0u32;
        let mut xs = SWEEP_X_MM.to_vec();
        xs.push(OLD_X_MM);
        for x in xs {
            let dir = bundle_with_exit_x(x);
            for seed in SCATTER_SEEDS {
                let run = run_from_eject_seeded(&dir, seed);
                println!("exit x {x} mm, seed {seed}: {run:?}");
                worst = worst.max(run.vuk_entries);
                if run.drained_at.is_none() || run.vuk_entries > MAX_SCATTERED_REENTRIES {
                    failed.push((x, seed, run));
                }
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
        println!("most VUK re-entries in one run: {worst}");
        assert!(failed.is_empty(), "runs that did not drain: {failed:?}");
    });
}

/// An operator action between ticks, applied before tick `t` of a run.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    /// A steel ball spawned on the VUK's hold point.
    Spawn,
    /// The `n`-th spawned ball removed.
    Remove(usize),
}

/// What an operated run observed.
#[derive(Debug, Clone, PartialEq)]
struct Trace {
    /// `state_hash` every 50 ticks.
    hashes: Vec<u64>,
    /// Ball state bits (position, velocity, spin, layer) on the eject tick.
    ejects: Vec<Vec<u32>>,
    vuk_hits: u32,
    switch_bits: [u64; 4],
    score: Option<i64>,
    /// Final state of every ball on the table.
    balls: Vec<Vec<u32>>,
}

fn state_bits(b: &pin_core::snapshot::BallState) -> Vec<u32> {
    b.position_m
        .iter()
        .chain(&b.velocity_m_s)
        .chain(&b.angular_velocity_rad_s)
        .chain(&b.orientation)
        .map(|c| c.to_bits())
        .chain([u32::from(b.layer)])
        .collect()
}

/// Ticks of an operated run.
const OPERATED_TICKS: u64 = 9_000;

/// Ejects the operated run asks for.
const OPERATED_EJECTS: usize = 6;

/// Plays the grey-box with `seed`: START pressed at tick 100 (a recorded
/// input), then, as the operator, a steel ball put on the VUK's hold point
/// whenever the scoop is empty (from tick 2000, 200 ticks after the last
/// removal), and each ejected ball removed 300 ticks after its eject (so
/// no drain ends the game), [`OPERATED_EJECTS`] times. With `replay`, the
/// inputs come from the recording and the operator actions from the log
/// instead. Returns the trace, the recording and the operator log.
fn operated_run(
    bundle: &Path,
    seed: u64,
    replay: Option<(&Recording, &[(u64, Op)])>,
) -> (Trace, Recording, Vec<(u64, Op)>) {
    let table_text = std::fs::read_to_string(bundle.join("table.toml")).unwrap();
    let table = TableManifest::from_toml_str(&table_text).unwrap();
    let (pos, _, _) = vuk_eject(&table);
    let hold = [pos[0] * 1.0e-3, pos[1] * 1.0e-3, pos[2] * 1.0e-3 + 0.014];
    let cfg = SimConfig {
        seed,
        ..SimConfig::default()
    };
    let mut s = Simulation::load(bundle, cfg).unwrap();
    let vi = (0..s.mech_set().len())
        .find(|&i| s.mech_set().kind(i) == Some(MechKind::Vuk))
        .expect("a VUK mech");
    let mut rec = Recording::new(seed, s.table_hash());
    let mut log: Vec<(u64, Op)> = Vec::new();
    let mut spawned = Vec::new();
    let mut trace = Trace {
        hashes: Vec::new(),
        ejects: Vec::new(),
        vuk_hits: 0,
        switch_bits: [0; 4],
        score: None,
        balls: Vec::new(),
    };
    let mut out = *s.snapshot();
    let mut inputs: Vec<InputEvent> = Vec::new();
    let mut next_spawn = 2_000u64;
    let mut ejected_at: Option<u64> = None;
    let mut held = false;
    let mut next_op = 0usize;
    for tick in 1..=OPERATED_TICKS {
        // Operator actions before the tick.
        let mut ops: Vec<Op> = Vec::new();
        match replay {
            Some((_, l)) => {
                while next_op < l.len() && l[next_op].0 == tick {
                    ops.push(l[next_op].1);
                    next_op += 1;
                }
            }
            None => {
                let current = spawned.len().checked_sub(1);
                if let Some(at) = ejected_at
                    && tick >= at + 300
                {
                    ops.push(Op::Remove(current.unwrap()));
                    ejected_at = None;
                    next_spawn = tick + 200;
                } else if ejected_at.is_none()
                    && !held
                    && spawned.len() < OPERATED_EJECTS
                    && tick >= next_spawn
                    && current.is_none_or(|c: usize| s.ball(spawned[c]).is_none())
                {
                    ops.push(Op::Spawn);
                    next_spawn = u64::MAX;
                }
            }
        }
        for op in ops {
            match op {
                Op::Spawn => {
                    spawned.push(s.spawn_ball("standard_steel", hold, [0.0; 3]).unwrap());
                }
                Op::Remove(n) => {
                    s.remove_ball(spawned[n]);
                }
            }
            log.push((tick, op));
        }
        // Inputs of the tick.
        inputs.clear();
        match replay {
            Some((r, _)) => inputs.extend(r.events_on(tick).iter().map(|e| e.1)),
            None => {
                if tick == 100 || tick == 180 {
                    inputs.push(InputEvent::button(
                        tick * TICK_NS - TICK_NS / 2,
                        CabinetInput::Start,
                        tick == 100,
                    ));
                }
            }
        }
        for e in &inputs {
            rec.events.push((tick, *e));
        }
        rec.ticks = tick;
        s.step_1ms(tick, &inputs, &mut out);
        // Eject detection: the VUK let go of the operator's ball.
        let now_held = s.mech_set().vuk_ball(vi).is_some();
        if held
            && !now_held
            && let Some(&h) = spawned.last()
            && let Some(b) = s.ball(h)
        {
            trace.ejects.push(state_bits(&b));
            ejected_at = Some(tick);
        }
        held = now_held;
        if tick % 50 == 0 {
            trace.hashes.push(s.state_hash());
        }
    }
    assert!(!s.script_faulted(), "script fault: {:?}", s.script_error());
    trace.vuk_hits = s.switch_hits("vuk_sw").unwrap();
    trace.switch_bits = s.switch_bits();
    trace.score = s.score(0);
    trace.balls = s
        .ball_handles()
        .iter()
        .filter_map(|&h| s.ball(h))
        .map(|b| state_bits(&b))
        .collect();
    (trace, rec, log)
}

/// Same seed and inputs: two runs and a replay of the recording (through
/// the file format) and the operator log give identical ball states,
/// switch counts, scores and state hashes over a game with at least 5
/// scattered ejects; another seed scatters the same ejects differently.
#[test]
fn scattered_vuk_ejects_replay_identically() {
    on_big_stack(|| {
        let bundle = greybox_bundle();
        let (a, rec, log) = operated_run(bundle, SEED, None);
        println!(
            "ejects {}, vuk hits {}, score {:?}, ops {log:?}",
            a.ejects.len(),
            a.vuk_hits,
            a.score
        );
        assert!(a.ejects.len() >= 5, "only {} ejects", a.ejects.len());
        assert!(a.vuk_hits >= 5);
        // Consecutive ejects from the same hold point differ (scatter).
        for w in a.ejects.windows(2) {
            assert_ne!(w[0], w[1], "two ejects were identical");
        }
        let (b, rec_b, log_b) = operated_run(bundle, SEED, None);
        assert_eq!(a, b, "a second run diverged");
        assert_eq!(log, log_b);
        assert_eq!(rec.to_bytes(), rec_b.to_bytes());
        let rec = Recording::from_bytes(&rec.to_bytes()).unwrap();
        let (c, _, log_c) = operated_run(bundle, rec.seed, Some((&rec, &log)));
        assert_eq!(a, c, "the replay diverged");
        assert_eq!(log, log_c);
        for other in [SEED + 1, SEED + 2] {
            let (d, _, _) = operated_run(bundle, other, None);
            assert!(
                d.ejects.len() >= 5,
                "seed {other}: {} ejects",
                d.ejects.len()
            );
            assert_ne!(
                a.ejects[0], d.ejects[0],
                "seed {other} gave the same first eject"
            );
        }
    });
}
