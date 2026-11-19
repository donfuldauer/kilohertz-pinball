//! 7.1 requirement 8 and 9: determinism soak of recorded input replays on
//! Apex Circuit, Sky Deck and Feature Lab (the greybox is excluded), and the
//! profiler's neutrality (same `state_hash` with a profiler recording every
//! tick, driven by synthetic nanosecond values, and with none).
//!
//! The bounded variants (20,000 ticks) run in the normal test run. The full
//! 600,000-tick variants are `#[ignore]` (release):
//! `cargo test --release -p pin_app --test spec_7_1_soak -- --ignored --nocapture`
//! and print one `SOAK_HASH` line each.
//!
//! Multiball: the replays are driven by scripted inputs only (state injected
//! through the test API is not recorded). Feature Lab and Sky Deck reach two
//! or more balls in play with the scripted rhythm (asserted by the
//! `*_multiball_replay_*` tests); Apex Circuit never has two balls at once,
//! so it does not (stated, not asserted).

#[path = "apex_support/mod.rs"]
mod apex_support;
#[path = "feature_lab_support/mod.rs"]
mod feature_lab_support;
#[path = "sky_deck_support/mod.rs"]
mod sky_deck_support;

use pin_app::profiler::{ProfiledSim, Profiler};
use pin_core::schema::table::TableManifest;
use pin_core::{CabinetInput, FrameStateSnapshot, InputEvent, SimStep, TICK_NS};
use pin_sim::replay::{Recorder, Recording};
use pin_sim::{SimConfig, Simulation};
use std::path::{Path, PathBuf};

const BOUNDED_TICKS: u64 = 20_000;
const FULL_TICKS: u64 = 600_000;
const RUNS: usize = 5;

/// The inputs of tick `t`: start, a launch every 2.5 s, and every flipper
/// button plus the action button in a fixed rhythm of `period` ticks (five
/// presses of 80 ticks each, evenly spread over the period). The period is
/// per table: it was chosen by sweeping periods and keeping one for which the
/// scripted game reaches a second ball in play (Apex Circuit's rules never
/// serve a second ball at once, so no period does there).
fn inputs(t: u64, period: u64, out: &mut Vec<InputEvent>) {
    out.clear();
    let ts = t * TICK_NS;
    let mut b = |i: CabinetInput, p: bool| out.push(InputEvent::button(ts, i, p));
    match t {
        10 => b(CabinetInput::Start, true),
        40 => b(CabinetInput::Start, false),
        _ => {}
    }
    match t % 2500 {
        1600 => b(CabinetInput::Launch, true),
        1630 => b(CabinetInput::Launch, false),
        _ => {}
    }
    let (o, slot) = (t % period, period / 8);
    let hold = 80.min(slot - 1);
    for (k, c) in [
        CabinetInput::LeftFlipper,
        CabinetInput::RightFlipper,
        CabinetInput::LeftUpper,
        CabinetInput::RightUpper,
        CabinetInput::Action,
    ]
    .into_iter()
    .enumerate()
    {
        let on = slot * (k as u64 + 1);
        if o == on {
            b(c, true);
        }
        if o == on + hold {
            b(c, false);
        }
    }
}

/// A table under test: name, bundle and its input rhythm period.
#[derive(Clone)]
struct Table {
    name: &'static str,
    dir: PathBuf,
    period: u64,
}

fn apex() -> Table {
    Table {
        name: "apex_circuit",
        dir: apex_support::apex_bundle().to_path_buf(),
        period: 700,
    }
}
fn sky() -> Table {
    Table {
        name: "sky_deck",
        dir: sky_deck_support::sky_deck_bundle().to_path_buf(),
        period: 880,
    }
}
fn lab() -> Table {
    Table {
        name: "feature_lab",
        dir: feature_lab_support::bundle().to_path_buf(),
        period: 640,
    }
}

/// What a run produced: everything that must be identical across runs.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Outcome {
    hash: u64,
    scores: Vec<Option<i64>>,
    hits: Vec<(String, Option<u32>)>,
}

fn switch_names(dir: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join("table.toml")).expect("table.toml");
    let m = TableManifest::from_toml_str(&text).expect("manifest");
    m.switches.iter().map(|s| s.id.clone()).collect()
}

fn outcome(sim: &Simulation, names: &[String]) -> Outcome {
    Outcome {
        hash: sim.state_hash(),
        scores: (0..4u8).map(|p| sim.score(p)).collect(),
        hits: names
            .iter()
            .map(|n| (n.clone(), sim.switch_hits(n)))
            .collect(),
    }
}

struct Run {
    outcome: Outcome,
    recording: Recording,
    max_balls: usize,
}

fn record(dir: &Path, period: u64, ticks: u64) -> Run {
    let names = switch_names(dir);
    let mut rec = Recorder::new(Simulation::load(dir, SimConfig::default()).unwrap());
    let mut buf = Vec::new();
    let mut max_balls = 0;
    for t in 1..=ticks {
        inputs(t, period, &mut buf);
        rec.step(&buf);
        max_balls = max_balls.max(rec.sim().balls_in_play());
    }
    let (sim, recording) = rec.finish();
    assert!(!sim.script_faulted(), "{:?}", sim.script_error());
    assert_eq!(sim.script_queue_overflows(), 0);
    Run {
        outcome: outcome(&sim, &names),
        recording,
        max_balls,
    }
}

fn replay_outcome(dir: &Path, rec: &Recording) -> Outcome {
    let mut sim = Simulation::load(
        dir,
        SimConfig {
            seed: rec.seed,
            ..SimConfig::default()
        },
    )
    .unwrap();
    let hash = Recorder::play(&mut sim, rec);
    let o = outcome(&sim, &switch_names(dir));
    assert_eq!(o.hash, hash);
    o
}

/// Records `ticks` ticks of `t`, replays them `RUNS` times (directly and
/// through the byte format) and checks every replay against the recording
/// run: identical `state_hash`, scores and per-switch hit counts. Returns the
/// recording run's outcome and the most balls seen in play at once.
fn soak(t: &Table, ticks: u64) -> (Outcome, usize) {
    let t = t.clone();
    let (run, mb) = on_stack(move || {
        let r = record(&t.dir, t.period, ticks);
        let through_bytes = Recording::from_bytes(&r.recording.to_bytes()).unwrap();
        assert_eq!(through_bytes, r.recording, "byte format round-trips");
        for i in 0..RUNS {
            assert_eq!(
                replay_outcome(&t.dir, &r.recording),
                r.outcome,
                "{}: replay {i} diverged",
                t.name
            );
            assert_eq!(
                replay_outcome(&t.dir, &through_bytes),
                r.outcome,
                "{}: byte-format replay {i} diverged",
                t.name
            );
        }
        let again = record(&t.dir, t.period, ticks);
        assert_eq!(again.outcome, r.outcome, "{}: re-record diverged", t.name);
        (r.outcome, r.max_balls)
    });
    (run, mb)
}

fn report(t: &Table, ticks: u64, o: &Outcome, mb: usize) {
    assert!(
        o.scores[0].is_some_and(|s| s > 0),
        "{}: the game scored",
        t.name
    );
    let hit = o
        .hits
        .iter()
        .filter(|(_, h)| h.is_some_and(|h| h > 0))
        .count();
    assert!(hit > 0, "{}: switches were hit", t.name);
    eprintln!(
        "{}: {ticks} ticks, max balls in play {mb}, switches hit {hit}/{}, hash {:016x}",
        t.name,
        o.hits.len(),
        o.hash
    );
}

fn on_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    apex_support::on_big_stack(f)
}

#[test]
fn apex_circuit_bounded_soak_is_deterministic() {
    let t = apex();
    let (o, mb) = soak(&t, BOUNDED_TICKS);
    report(&t, BOUNDED_TICKS, &o, mb);
}

#[test]
fn sky_deck_bounded_soak_is_deterministic() {
    let t = sky();
    let (o, mb) = soak(&t, BOUNDED_TICKS);
    report(&t, BOUNDED_TICKS, &o, mb);
}

#[test]
fn feature_lab_bounded_soak_is_deterministic() {
    let t = lab();
    let (o, mb) = soak(&t, BOUNDED_TICKS);
    report(&t, BOUNDED_TICKS, &o, mb);
}

/// The same checks on runs long enough for the scripted game to have two or
/// more balls in play at once (Feature Lab from about tick 37,000, Sky Deck
/// from about tick 133,000; measured when the pattern was chosen). Apex
/// Circuit never serves a second ball while one is live (its rules serve one
/// ball per `serve_ball`), so it has no multiball to reach; its bounded test
/// above covers it and this limitation is stated in the decision log.
#[test]
fn feature_lab_multiball_replay_is_deterministic() {
    let t = lab();
    let (o, mb) = soak(&t, 45_000);
    report(&t, 45_000, &o, mb);
    assert!(mb >= 2, "feature_lab reached {mb} balls in play");
}

#[test]
fn sky_deck_multiball_replay_is_deterministic() {
    let t = sky();
    let (o, mb) = soak(&t, 150_000);
    report(&t, 150_000, &o, mb);
    assert!(mb >= 2, "sky_deck reached {mb} balls in play");
}

// ---- requirement 9: profiler neutrality ----

fn neutral(t: Table, ticks: u64) {
    on_stack(move || {
        let (dir, period) = (t.dir.clone(), t.period);
        let names = switch_names(&dir);
        // Without a profiler.
        let mut plain = Simulation::load(&dir, SimConfig::default()).unwrap();
        let mut out = FrameStateSnapshot::default();
        let mut buf = Vec::new();
        for k in 1..=ticks {
            inputs(k, period, &mut buf);
            plain.step_1ms(k, &buf, &mut out);
        }
        let want = outcome(&plain, &names);
        let want_snapshot_tick = out.tick;

        // With a profiler recording every tick, on a synthetic clock.
        let p = Profiler::new();
        let mut clock = 0u64;
        let mut sim = ProfiledSim::new(
            Simulation::load(&dir, SimConfig::default()).unwrap(),
            p.sim,
            move || {
                clock += 90_000;
                clock
            },
        );
        let mut out = FrameStateSnapshot::default();
        for k in 1..=ticks {
            inputs(k, period, &mut buf);
            sim.step_1ms(k, &buf, &mut out);
        }
        assert_eq!(out.tick, want_snapshot_tick);
        let got = outcome(sim.inner(), &names);
        assert_eq!(got, want, "{}: the profiler changed the simulation", t.name);
        let s = p.reader.snapshot();
        assert_eq!(s.tick.count, ticks, "{}: every tick was recorded", t.name);
        assert!(s.tick.max > 0);
        assert!(
            s.input.count > 0,
            "{}: coil-driving inputs were measured",
            t.name
        );
        assert_eq!(s.script_queue_overflows, 0);
        // Hand-feeding the writer afterwards does not touch the sim either.
        let (inner, mut writer) = sim.into_parts();
        for i in 0..1000 {
            writer.record_tick_ns(i);
        }
        assert_eq!(inner.state_hash(), want.hash);
    });
}

#[test]
fn profiler_does_not_change_the_apex_circuit_state_hash() {
    neutral(apex(), BOUNDED_TICKS);
}

#[test]
fn profiler_does_not_change_the_sky_deck_state_hash() {
    neutral(sky(), BOUNDED_TICKS);
}

#[test]
fn profiler_does_not_change_the_feature_lab_state_hash() {
    // Long enough to include the multiball phase.
    neutral(lab(), 45_000);
}

// ---- full 10-minute variants (release, ignored) ----

fn full(t: Table) {
    let t2 = t.clone();
    let (hash, mb) = on_stack(move || {
        let r = record(&t2.dir, t2.period, FULL_TICKS);
        let bytes = Recording::from_bytes(&r.recording.to_bytes()).unwrap();
        for i in 0..2 {
            assert_eq!(
                replay_outcome(&t2.dir, &bytes),
                r.outcome,
                "replay {i} diverged"
            );
        }
        assert!(
            r.outcome.scores[0].is_some_and(|s| s > 0),
            "the game scored"
        );
        (r.outcome.hash, r.max_balls)
    });
    eprintln!("{}: max balls in play {mb}", t.name);
    println!(
        "SOAK_HASH table={} os={} arch={} ticks={FULL_TICKS} hash={hash:016x}",
        t.name,
        std::env::consts::OS,
        std::env::consts::ARCH
    );
}

#[test]
#[ignore = "10 simulated minutes; run in release with --ignored"]
fn apex_circuit_full_soak() {
    full(apex());
}

#[test]
#[ignore = "10 simulated minutes; run in release with --ignored"]
fn sky_deck_full_soak() {
    full(sky());
}

#[test]
#[ignore = "10 simulated minutes; run in release with --ignored"]
fn feature_lab_full_soak() {
    full(lab());
}
