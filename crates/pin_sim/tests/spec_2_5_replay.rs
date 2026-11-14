//! Subphase 2.5 spec tests (Independent Tester): record / replay
//! (deliverable 3, verification criterion 1).
//!
//! The recordings here are built from the pinned public fields of
//! `Recording` and compared with a simulation driven through
//! `SimStep::step_1ms` with the same per-tick events, so the tests do not
//! depend on the un-pinned parts of `Recorder`.

#[path = "fixtures/spec_2_5/support.rs"]
mod support;

use pin_core::input::{CabinetAxis, CabinetInput, InputEvent};
use pin_core::sim::tick_end_ns;
use pin_sim::replay::{Recording, replay};
use proptest::prelude::*;
use support::*;

fn fixture_table_hash() -> u64 {
    load().table_hash()
}

/// Verification criterion 1: a 10,000-tick recording (balls moving, flippers
/// firing from recorded inputs, a Lua script scoring) replays to an identical
/// state hash 10 times in a row, and that hash is the hash of the live run.
#[test]
fn ten_thousand_tick_recording_replays_identically_ten_times() {
    on_sim_thread(|| {
        const TICKS: u64 = 10_000;
        let events = input_script(TICKS, 0);
        assert!(events.len() > 60);
        let live = drive(&bundle(), DEFAULT_SEED, TICKS, &events);
        assert_eq!(live.tick(), TICKS);

        // The run is not trivial.
        let score = live.score(P_SCORE).unwrap();
        assert!(
            live.switch_hits("lane_opto").unwrap() >= 5,
            "every dispensed ball passes lane_opto"
        );
        assert!(score >= 50, "the Lua script must have scored: {score}");
        assert_eq!(live.switch_hits("start"), Some(7));
        assert!(live.switch_hits("left_flipper").unwrap() >= 10);
        assert!(live.switch_hits("right_flipper").unwrap() >= 10);
        assert!(live.score(P_RNG).unwrap() > 0);

        let rec = recording(live.table_hash(), DEFAULT_SEED, TICKS, &events);
        let want = live.state_hash();
        let mut hashes = Vec::new();
        for _ in 0..10 {
            hashes.push(replay(&bundle(), &rec).expect("replay of a valid recording"));
        }
        assert!(
            hashes.iter().all(|h| *h == want),
            "live hash {want:016x}, replays {hashes:016x?}"
        );

        // Through the file format as well.
        let bytes = rec.to_bytes();
        let back = Recording::from_bytes(&bytes).expect("round trip");
        assert_eq!(replay(&bundle(), &back).unwrap(), want);

        // The live run itself is reproducible, tick by tick.
        let again = drive(&bundle(), DEFAULT_SEED, TICKS, &events);
        assert_eq!(again.state_hash(), want);
        assert_eq!(*again.snapshot(), *live.snapshot());
    });
}

/// The recorded inputs and the seed matter: the hash is not a constant.
#[test]
fn replay_depends_on_inputs_seed_and_length() {
    on_sim_thread(|| {
        const TICKS: u64 = 2500;
        let th = fixture_table_hash();
        let events = input_script(TICKS, 0);
        let base = replay(&bundle(), &recording(th, 1, TICKS, &events)).unwrap();
        assert_eq!(
            base,
            replay(&bundle(), &recording(th, 1, TICKS, &events)).unwrap()
        );
        // Seed (the script draws from the RNG on `start`).
        assert_ne!(
            base,
            replay(&bundle(), &recording(th, 2, TICKS, &events)).unwrap()
        );
        // Length.
        let shorter: Vec<_> = events.iter().filter(|e| e.0 < TICKS).copied().collect();
        assert_ne!(
            base,
            replay(&bundle(), &recording(th, 1, TICKS - 1, &shorter)).unwrap()
        );
        // Other inputs.
        let other = input_script(TICKS, 99);
        assert_ne!(base, replay(&bundle(), &recording(th, 1, TICKS, &other)).unwrap());
        // No inputs at all = advance().
        let mut quiet = load_at(&bundle(), 1);
        quiet.advance(TICKS);
        assert_eq!(
            replay(&bundle(), &recording(th, 1, TICKS, &[])).unwrap(),
            quiet.state_hash()
        );
        assert_ne!(base, quiet.state_hash());
        // An empty recording is the freshly loaded state.
        assert_eq!(
            replay(&bundle(), &recording(th, 1, 0, &[])).unwrap(),
            load_at(&bundle(), 1).state_hash()
        );
    });
}

/// One flipper press moved by one tick is a different run: events are
/// replayed at their recorded tick.
#[test]
fn replay_applies_each_event_at_its_recorded_tick() {
    on_sim_thread(|| {
        let th = fixture_table_hash();
        let run = |press: u64| {
            let ev = vec![
                (5, button(5, CabinetInput::Start, true)),
                (press, button(press, CabinetInput::LeftFlipper, true)),
                (press + 80, button(press + 80, CabinetInput::LeftFlipper, false)),
            ];
            let live = drive(&bundle(), DEFAULT_SEED, press + 81, &ev).state_hash();
            let rep = replay(&bundle(), &recording(th, DEFAULT_SEED, press + 81, &ev)).unwrap();
            assert_eq!(live, rep, "press at {press}");
            // Stop at tick 305: the flipper is mid-stroke (a stroke takes
            // 20-35 ms), so a shifted press gives a different state.
            let short = vec![ev[0], ev[1]];
            replay(&bundle(), &recording(th, DEFAULT_SEED, 305, &short)).unwrap()
        };
        let a = run(300);
        let b = run(301);
        let c = run(303);
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    });
}

/// `replay` refuses a recording whose table hash does not match the bundle.
#[test]
fn replay_refuses_a_table_hash_mismatch() {
    on_sim_thread(|| {
        let th = fixture_table_hash();
        let events = input_script(400, 3);
        let good = recording(th, 9, 400, &events);
        let want = replay(&bundle(), &good).expect("matching table hash replays");

        for bad_hash in [th ^ 1, th.wrapping_add(1), !th, 0, th.rotate_left(32)] {
            if bad_hash == th {
                continue;
            }
            let bad = recording(bad_hash, 9, 400, &events);
            assert!(
                replay(&bundle(), &bad).is_err(),
                "table_hash {bad_hash:016x} != bundle {th:016x} must be refused"
            );
        }

        // The same recording against an edited bundle is refused ...
        let edited = bundle_copy("edited");
        let mut lua = std::fs::read_to_string(edited.join("rules.lua")).unwrap();
        lua.push_str("\n-- one more line\n");
        std::fs::write(edited.join("rules.lua"), lua).unwrap();
        let e = replay(&edited, &good);
        assert!(e.is_err(), "a recording must not replay on an edited table");
        let msg = format!("{:?}", e.unwrap_err());
        assert!(!msg.is_empty());

        // ... and accepted against a byte-identical copy elsewhere.
        let copy = bundle_copy("identical");
        assert_eq!(replay(&copy, &good).unwrap(), want);

        // A missing bundle is an error, not a panic.
        assert!(replay(&copy.join("missing"), &good).is_err());
    });
}

/// Axis events (plunger, nudge, analog flipper travel) are part of the
/// recorded stream and replay deterministically.
#[test]
fn axis_events_replay_deterministically() {
    on_sim_thread(|| {
        let th = fixture_table_hash();
        let mut ev: Vec<(u64, InputEvent)> = vec![(5, button(5, CabinetInput::Start, true))];
        for (i, t) in (100..900u64).step_by(40).enumerate() {
            let ts = tick_end_ns(0, t) - 1;
            let axis = CabinetAxis::ALL[i % CabinetAxis::ALL.len()];
            let value = ((i as f32) * 0.37).sin();
            let value = match axis {
                CabinetAxis::NudgeX | CabinetAxis::NudgeY => value,
                _ => value.abs(),
            };
            ev.push((t, InputEvent::axis(ts, axis, value)));
        }
        let live = drive(&bundle(), 3, 1000, &ev);
        let rec = recording(th, 3, 1000, &ev);
        let back = Recording::from_bytes(&rec.to_bytes()).unwrap();
        for _ in 0..3 {
            assert_eq!(replay(&bundle(), &back).unwrap(), live.state_hash());
        }
    });
}

fn arb_input() -> impl Strategy<Value = (CabinetInput, bool)> {
    (
        prop::sample::select(vec![
            CabinetInput::LeftFlipper,
            CabinetInput::RightFlipper,
            CabinetInput::Start,
            CabinetInput::Action,
            CabinetInput::Launch,
            CabinetInput::LeftUpper,
            CabinetInput::RightUpper,
        ]),
        any::<bool>(),
    )
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 12, max_shrink_iters: 8, ..ProptestConfig::default() })]

    /// Invariant: for any input stream and seed, replay(recording) equals the
    /// live run's hash, twice, also after a to_bytes/from_bytes round trip.
    #[test]
    fn any_recorded_stream_replays_to_the_live_hash(
        seed in any::<u64>(),
        ticks in 50u64..600,
        raw in prop::collection::vec((0u64..600, arb_input()), 0..40),
    ) {
        let (live, a, b) = on_sim_thread(move || {
            let mut ev: Vec<(u64, InputEvent)> = raw
                .iter()
                .map(|(t, (input, pressed))| {
                    let tick = 1 + t % ticks;
                    (tick, button(tick, *input, *pressed))
                })
                .collect();
            ev.sort_by_key(|e| e.0);
            let live = drive(&bundle(), seed, ticks, &ev);
            let rec = recording(live.table_hash(), seed, ticks, &ev);
            let a = replay(&bundle(), &rec).unwrap();
            let back = Recording::from_bytes(&rec.to_bytes()).unwrap();
            let b = replay(&bundle(), &back).unwrap();
            (live.state_hash(), a, b)
        });
        prop_assert_eq!(live, a);
        prop_assert_eq!(live, b);
    }
}
