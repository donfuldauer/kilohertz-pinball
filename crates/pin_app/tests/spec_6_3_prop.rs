//! Subphase 6.3 spec tests (Independent Tester): property tests (brief
//! requirement 7). For any stream of at most 3000 ticks of cabinet inputs
//! (every cabinet button, tilt included, pressed and released at random
//! ticks, with or without a start press first) and any sim seed, the Lua
//! and the Wasm rules of every table drain identical command queues at
//! every tick and their simulations stay identical; neither faults.

mod spec_6_3_support;

use pin_core::{CabinetInput, InputEvent, SimStep, TICK_NS};
use proptest::prelude::*;
use spec_6_3_support::*;

/// A random cabinet stream: `(tick, input index, pressed)`.
#[derive(Clone, Debug)]
struct Stream {
    ticks: u64,
    seed: u64,
    start_first: bool,
    events: Vec<(u64, usize, bool)>,
}

/// The tick of the injected START press of a `start_first` stream.
const START_PRESS: u64 = 5;
/// The tick of its release.
const START_RELEASE: u64 = 30;

/// Random streams. With `start_first` the prefix through the injected
/// START release (ticks 1..=[`START_RELEASE`]) is reserved: every random
/// event comes after it, so none lands before the START press or between
/// the press and its release. Without it the events are unrestricted.
fn stream() -> impl Strategy<Value = Stream> {
    (1u64..=3000, any::<u64>(), any::<bool>()).prop_flat_map(|(ticks, seed, start_first)| {
        let first = if start_first { START_RELEASE + 1 } else { 1 };
        // No room after the reserved prefix: no random events.
        let max_events = if first <= ticks { 120 } else { 1 };
        let last = ticks.max(first);
        proptest::collection::vec(
            (first..=last, 0..CabinetInput::COUNT, any::<bool>()),
            0..max_events,
        )
        .prop_map(move |mut events| {
            events.sort_by_key(|e| e.0);
            Stream {
                ticks,
                seed,
                start_first,
                events,
            }
        })
    })
}

fn check_stream(t: Table, s: Stream) {
    on_big_stack(move || {
        let ctx = format!("{t:?} {s:?}");
        let mut lua = load(lua_bundle(t), s.seed);
        let mut wasm = load(wasm_bundle(t), s.seed);
        assert_same_tick(&lua, &wasm, &ctx);
        let mut pair = ScriptPair::new(t, &lua, s.seed, &ctx);
        let mut out_l = Box::new(*lua.snapshot());
        let mut out_w = Box::new(*wasm.snapshot());
        let mut ev: Vec<InputEvent> = Vec::new();
        if s.start_first {
            assert!(
                s.events.iter().all(|e| e.0 > START_RELEASE),
                "{ctx}: a random event inside the reserved START prefix"
            );
        }
        let mut next = 0;
        for tick in 1..=s.ticks {
            ev.clear();
            let ts = tick * TICK_NS - TICK_NS / 2;
            if s.start_first {
                match tick {
                    START_PRESS => ev.push(InputEvent::button(ts, CabinetInput::Start, true)),
                    START_RELEASE => ev.push(InputEvent::button(ts, CabinetInput::Start, false)),
                    _ => {}
                }
            }
            while next < s.events.len() && s.events[next].0 == tick {
                let (_, i, p) = s.events[next];
                ev.push(InputEvent::button(ts, CabinetInput::ALL[i], p));
                next += 1;
            }
            lua.step_1ms(tick, &ev, &mut out_l);
            wasm.step_1ms(tick, &ev, &mut out_w);
            assert_same_tick(&lua, &wasm, &ctx);
            pair.step(tick, &lua);
        }
        assert_eq!(pair.ticks, s.ticks + 1, "{ctx}");
    });
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 8, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn apex_random_cabinet_streams_keep_lua_and_wasm_identical(s in stream()) {
        check_stream(Table::Apex, s);
    }

    #[test]
    fn sky_deck_random_cabinet_streams_keep_lua_and_wasm_identical(s in stream()) {
        check_stream(Table::SkyDeck, s);
    }

    #[test]
    fn feature_lab_random_cabinet_streams_keep_lua_and_wasm_identical(s in stream()) {
        check_stream(Table::FeatureLab, s);
    }
}
