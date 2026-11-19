//! 7.1 determinism soak (D9): on each shipped table (apex_circuit,
//! sky_deck, feature_lab; greybox is the Gate A1 physics fixture and out
//! of scope), a recorded input stream of a closed-loop scripted player is
//! replayed several times to the same `state_hash`, score and per-switch
//! hit counts, through the recording byte format, and once more with the
//! 7.1 profiler wrapped around every tick (the profiler must not change the
//! hash).
//!
//! * `full_soak_*` (ignored; run in release: `cargo test --release -p
//!   pin_app --test profiler_soak -- --ignored --nocapture`): 600,000 ticks
//!   (10 simulated minutes) reaching multiball (at least two balls in play
//!   at some tick). Apex Circuit's rules have no multiball, so the harness
//!   injects a second ball at scheduled ticks there (decision
//!   `7.1 — Apex multiball`).
//! * `bounded_soak_*` (default suite, debug CI): the same harness for
//!   10,000 ticks (10 simulated seconds).
//!
//! Every run prints `SOAK_HASH table=<name> os=<os> arch=<arch>
//! ticks=<ticks> hash=<16 hex>`; the soak workflow collects these lines.
//! The profiled run reports the tick time p99.9 and mean measured on the
//! wall clock: printed, never asserted (rule 5, shared machines).

mod profiler_support;

use pin_app::profiler::{TICK_P99_9_TARGET_NS, format_ms};
use pin_sim::replay::Recording;
use profiler_support::*;

/// Ticks of the full soak (10 simulated minutes).
const FULL_TICKS: u64 = 600_000;
/// Ticks of the bounded soak (10 simulated seconds).
const BOUNDED_TICKS: u64 = 10_000;
/// Plain replays per table (besides the recording run and the profiled
/// replay; with the `pin_sim::replay::replay` cross-check and the
/// profiled replay, 10 replays on tables without injections).
const REPLAYS: usize = 8;

fn soak(table: Table, ticks: u64, need_multiball: bool) {
    on_big_stack(move || {
        let name = table.name();
        let rec = record(table, ticks);
        println!(
            "soak {name}: {ticks} ticks, games {}, max balls {}, first multiball tick {:?}, \
             multiball ticks {}, injections {}, score {:?}",
            rec.games,
            rec.max_balls,
            rec.first_multiball_tick,
            rec.multiball_ticks,
            rec.stream.injections.len(),
            rec.result.score
        );
        assert!(rec.games >= 1, "{name}: a game was started");
        assert!(
            rec.result.switch_hits.iter().any(|(_, n)| *n > 0),
            "{name}: switches were hit"
        );
        if need_multiball {
            assert!(
                rec.max_balls >= 2,
                "{name}: the stream reached multiball (max balls {})",
                rec.max_balls
            );
        }
        let want = rec.result.clone();
        println!("{}", soak_hash_line(table, ticks, want.hash));

        // Through the recording byte format.
        let bytes = rec.stream.recording.to_bytes();
        let decoded = Recording::from_bytes(&bytes).expect("recording decodes");
        assert_eq!(decoded, rec.stream.recording, "{name}: byte round trip");
        let stream = Stream {
            recording: decoded,
            injections: rec.stream.injections.clone(),
        };

        for run in 1..=REPLAYS {
            let got = replay_plain(table, &stream);
            println!("{}", soak_hash_line(table, ticks, got.hash));
            assert_eq!(got.hash, want.hash, "{name}: replay {run} state_hash");
            assert_eq!(got.score, want.score, "{name}: replay {run} score");
            assert_eq!(
                got.switch_hits, want.switch_hits,
                "{name}: replay {run} switch hits"
            );
        }
        if stream.injections.is_empty() {
            // The pin_sim replay entry point agrees (no harness schedule).
            let h = replay_via_pin_sim(table, &stream);
            println!("{}", soak_hash_line(table, ticks, h));
            assert_eq!(h, want.hash, "{name}: pin_sim::replay::replay");
        }

        // Profiler on: same hash, score and hits; timing reported only.
        let (got, snap) = replay_profiled(table, &stream, wall_ns);
        println!("{}", soak_hash_line(table, ticks, got.hash));
        assert_eq!(got, want, "{name}: profiled replay");
        assert_eq!(snap.tick.count, ticks, "{name}: one tick sample per tick");
        assert_eq!(snap.command_queue_overflows(), 0, "{name}: no overflow");
        let verdict = if snap.tick.p99_9 < TICK_P99_9_TARGET_NS {
            "under"
        } else {
            "OVER"
        };
        println!(
            "SOAK_TICK table={name} profile={} ticks={ticks} p99_9_ms={} mean_ms={} max_ms={} \
             target_ms={} {verdict}",
            if cfg!(debug_assertions) {
                "debug"
            } else {
                "release"
            },
            format_ms(snap.tick.p99_9),
            format_ms(snap.tick.mean),
            format_ms(snap.tick.max),
            format_ms(TICK_P99_9_TARGET_NS)
        );
        println!(
            "SOAK_INPUT table={name} events={} p99_ms={} clamped={}",
            snap.input.count,
            format_ms(snap.input.p99),
            snap.input_clamped
        );
    });
}

#[test]
fn bounded_soak_apex_circuit() {
    soak(Table::Apex, BOUNDED_TICKS, false);
}

#[test]
fn bounded_soak_sky_deck() {
    soak(Table::SkyDeck, BOUNDED_TICKS, false);
}

#[test]
fn bounded_soak_feature_lab() {
    soak(Table::FeatureLab, BOUNDED_TICKS, false);
}

#[test]
#[ignore = "10 simulated minutes per run; run in release with --ignored --nocapture"]
fn full_soak_apex_circuit() {
    soak(Table::Apex, FULL_TICKS, true);
}

#[test]
#[ignore = "10 simulated minutes per run; run in release with --ignored --nocapture"]
fn full_soak_sky_deck() {
    soak(Table::SkyDeck, FULL_TICKS, true);
}

#[test]
#[ignore = "10 simulated minutes per run; run in release with --ignored --nocapture"]
fn full_soak_feature_lab() {
    soak(Table::FeatureLab, FULL_TICKS, true);
}
