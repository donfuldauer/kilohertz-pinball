//! Subphase 2.5 spec tests (Independent Tester): edge cases of the injection
//! API, hostile recordings handed to `replay`, layers, handle reuse and the
//! Wasm budget.

#[path = "fixtures/spec_2_5/support.rs"]
mod support;

use pin_core::input::{CabinetInput, InputEvent};
use pin_sim::replay::{Recording, replay};
use proptest::prelude::*;
use support::*;

/// A drained ball's handle stays dead when its arena slot is reused.
#[test]
fn stale_handle_stays_dead_when_the_slot_is_reused() {
    on_sim_thread(|| {
        let mut sim = load();
        let first = sim
            .spawn_ball("standard_steel", [0.257, 0.12, R], [0.0, -0.5, 0.0])
            .unwrap();
        sim.advance(600);
        assert!(sim.ball(first).is_none(), "drained through trough_1");
        assert_eq!(sim.balls_in_play(), 0);
        let mut later = Vec::new();
        for i in 0..4 {
            let h = sim
                .spawn_ball(
                    "standard_steel",
                    [0.08 + 0.05 * i as f32, 0.95, R],
                    [0.0; 3],
                )
                .unwrap();
            assert_ne!(h, first, "a new ball gets a new handle");
            later.push(h);
        }
        assert!(
            sim.ball(first).is_none(),
            "the old handle must not alias a new ball"
        );
        sim.advance(50);
        assert!(sim.ball(first).is_none());
        assert_eq!(sim.balls_in_play(), 4);
        assert!(later.iter().all(|h| sim.ball(*h).is_some()));
    });
}

/// Many balls through the drain: the arena never leaks slots.
#[test]
fn arena_slots_are_reusable_after_drains() {
    on_sim_thread(|| {
        let mut sim = load();
        for round in 0..40 {
            let h = sim
                .spawn_ball("standard_steel", [0.257, 0.10, R], [0.0, -0.8, 0.0])
                .unwrap_or_else(|e| panic!("round {round}: {e}"));
            sim.advance(300);
            assert!(sim.ball(h).is_none(), "round {round}: not drained");
        }
        assert_eq!(sim.balls_in_play(), 0);
        assert_eq!(sim.switch_hits("trough_1"), Some(40));
        assert_eq!(sim.score(P_SCORE), Some(40_000));
    });
}

/// A layer-0 opto is not closed by a ball on another layer.
#[test]
fn switches_only_see_balls_on_their_layer() {
    on_sim_thread(|| {
        let mut sim = load();
        // Dead centre of lane_opto, but on layer 1.
        sim.spawn_ball_on_layer("standard_steel", 1, [0.257, 0.850, R], [0.0; 3])
            .unwrap();
        sim.advance(3);
        assert_eq!(sim.switch_closed("lane_opto"), Some(false));
        assert_eq!(sim.switch_hits("lane_opto"), Some(0));
        // The same on layer 0 closes it.
        let mut sim = load();
        sim.spawn_ball_on_layer("standard_steel", 0, [0.257, 0.850, R], [0.0; 3])
            .unwrap();
        sim.advance(3);
        assert_eq!(sim.switch_hits("lane_opto"), Some(1));
    });
}

/// Non-finite or absurd injected state is refused or removed; it never
/// panics, hangs, or leaves NaN in the published state.
#[test]
fn non_finite_injection_is_contained() {
    on_sim_thread(|| {
        let bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0e30, -1.0e30];
        for &b in &bad {
            for axis in 0..3 {
                for in_velocity in [false, true] {
                    let mut sim = load();
                    let ok = sim
                        .spawn_ball("standard_steel", [0.06, 0.95, R], [0.0; 3])
                        .unwrap();
                    let mut p = [0.3, 0.6, R];
                    let mut v = [0.0f32; 3];
                    if in_velocity {
                        v[axis] = b;
                    } else {
                        p[axis] = b;
                    }
                    let h = sim.spawn_ball("standard_steel", p, v).ok();
                    sim.advance(200);
                    assert_eq!(sim.tick(), 200);
                    for ball in sim.snapshot().balls() {
                        let all = ball
                            .position_m
                            .iter()
                            .chain(&ball.velocity_m_s)
                            .chain(&ball.angular_velocity_rad_s)
                            .chain(&ball.orientation);
                        for c in all {
                            assert!(
                                c.is_finite(),
                                "value {b} on axis {axis} (velocity: {in_velocity}) left {ball:?}"
                            );
                        }
                    }
                    if let Some(h) = h
                        && !b.is_finite()
                    {
                        assert!(sim.ball(h).is_none(), "a non-finite ball must be removed");
                    }
                    let good = sim.ball(ok).expect("the healthy ball is unaffected");
                    assert!(good.position_m[1] < 0.95);
                }
            }
        }
    });
}

/// Very fast balls (CCD sub-steps inside the tick) stay inside the perimeter
/// wall or are removed; never tunnel to a position outside while still alive.
#[test]
fn fast_balls_do_not_tunnel_through_the_perimeter() {
    on_sim_thread(|| {
        for (i, speed) in [5.0f32, 12.0, 25.0, 40.0].into_iter().enumerate() {
            for k in 0..8 {
                let a = (k as f32) * std::f32::consts::FRAC_PI_4 + 0.2 + i as f32 * 0.1;
                let mut sim = load();
                let h = sim
                    .spawn_ball(
                        "standard_steel",
                        [0.10, 0.80, R],
                        [speed * a.cos(), speed * a.sin(), 0.0],
                    )
                    .unwrap();
                for t in 0..400 {
                    sim.advance(1);
                    let Some(b) = sim.ball(h) else { break };
                    let p = b.position_m;
                    assert!(
                        (-0.001..=0.515).contains(&p[0]) && (-0.001..=1.071).contains(&p[1]),
                        "{speed} m/s at angle {a}: outside the walls at tick {t}: {p:?}"
                    );
                }
            }
        }
    });
}

/// A Wasm rules module that spins forever in a callback is cut off by the
/// engine budget; the tick carries on.
#[test]
fn runaway_wasm_callback_does_not_stall_the_tick() {
    on_sim_thread(|| {
        let wat = r#"
(module
  (import "pinball" "set_score" (func $set_score (param i32 i64)))
  (import "pinball" "id_of" (func $id_of (param i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 16) "action")
  (global $sw (mut i32) (i32.const -1))
  (global $ticks (mut i64) (i64.const 0))
  (func (export "on_init")
    (global.set $sw (call $id_of (i32.const 0) (i32.const 16) (i32.const 6))))
  (func (export "on_tick")
    (global.set $ticks (i64.add (global.get $ticks) (i64.const 1)))
    (call $set_score (i32.const 1) (global.get $ticks)))
  (func (export "on_switch_edge") (param $id i32) (param $closed i32)
    (if (i32.and (local.get $closed) (i32.eq (local.get $id) (global.get $sw)))
      (then (loop $spin (br $spin)))))
)
"#;
        let dir = bundle_with_wasm("wasm_spin", wat);
        let mut sim = load_at(&dir, DEFAULT_SEED);
        let h = sim
            .spawn_ball("standard_steel", [0.06, 0.95, R], [0.0; 3])
            .unwrap();
        sim.advance(20);
        assert_eq!(sim.score(1), Some(20));
        sim.set_switch("action", true).unwrap();
        sim.advance(500);
        assert_eq!(sim.tick(), 520);
        let frozen = sim.score(1).unwrap();
        assert!(
            frozen == 20 || frozen == 21,
            "a faulted module is not called: {frozen}"
        );
        assert!(
            sim.ball(h).unwrap().position_m[1] < 0.93,
            "physics carries on"
        );
    });
}

fn ev(tick: u64, stamp_tick: u64, input: CabinetInput, pressed: bool) -> (u64, InputEvent) {
    (tick, button(stamp_tick.max(1), input, pressed))
}

/// `replay` takes a `Recording` value whose public fields can hold anything:
/// events out of order, at tick 0, beyond `ticks`, with timestamps of other
/// ticks. It must return (Ok or Err) without panicking, and deterministically.
#[test]
fn replay_survives_inconsistent_recordings() {
    on_sim_thread(|| {
        let th = load().table_hash();
        type Case = (&'static str, u64, Vec<(u64, InputEvent)>);
        let cases: Vec<Case> = vec![
            (
                "event at tick 0",
                50,
                vec![ev(0, 1, CabinetInput::Start, true)],
            ),
            (
                "event after the end",
                50,
                vec![ev(51, 51, CabinetInput::Start, true)],
            ),
            (
                "event far after the end",
                50,
                vec![ev(u64::MAX, 7, CabinetInput::Start, true)],
            ),
            (
                "events out of order",
                200,
                vec![
                    ev(150, 150, CabinetInput::LeftFlipper, true),
                    ev(10, 10, CabinetInput::Start, true),
                    ev(100, 100, CabinetInput::LeftFlipper, false),
                ],
            ),
            (
                "timestamp of another tick",
                200,
                vec![
                    ev(10, 190, CabinetInput::Start, true),
                    ev(150, 3, CabinetInput::LeftFlipper, true),
                ],
            ),
            (
                "timestamp u64::MAX",
                100,
                vec![(20, InputEvent::button(u64::MAX, CabinetInput::Start, true))],
            ),
            (
                "timestamp 0",
                100,
                vec![(20, InputEvent::button(0, CabinetInput::Start, true))],
            ),
            (
                "many events in one tick",
                100,
                (0..500)
                    .map(|i| {
                        ev(
                            30,
                            30,
                            CabinetInput::ALL[i % CabinetInput::ALL.len()],
                            i % 3 == 0,
                        )
                    })
                    .collect(),
            ),
            (
                "events but zero ticks",
                0,
                vec![ev(1, 1, CabinetInput::Start, true)],
            ),
        ];
        for (what, ticks, events) in cases {
            let rec = Recording {
                seed: 1,
                table_hash: th,
                ticks,
                events,
            };
            let a = replay(&bundle(), &rec).ok();
            let b = replay(&bundle(), &rec).ok();
            assert_eq!(a, b, "{what}: replay is not deterministic");
            // Whatever the verdict, the file form must not panic either.
            let bytes = rec.to_bytes();
            let _ = Recording::from_bytes(&bytes);
        }
    });
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, max_shrink_iters: 16, ..ProptestConfig::default() })]

    /// Arbitrary (tick, timestamp, input) triples in a Recording never make
    /// `replay` panic, and two replays agree.
    #[test]
    fn replay_never_panics_on_arbitrary_event_lists(
        ticks in 0u64..300,
        seed in any::<u64>(),
        raw in prop::collection::vec(
            (
                prop_oneof![4 => 0u64..320, 1 => any::<u64>()],
                prop_oneof![4 => 0u64..320_000_000, 1 => any::<u64>()],
                0usize..8,
                any::<bool>(),
            ),
            0..24,
        ),
    ) {
        let (a, b) = on_sim_thread(move || {
            let events: Vec<(u64, InputEvent)> = raw
                .iter()
                .map(|&(tick, ts, i, pressed)| {
                    (tick, InputEvent::button(ts, CabinetInput::ALL[i % CabinetInput::ALL.len()], pressed))
                })
                .collect();
            let rec = Recording { seed, table_hash: load().table_hash(), ticks, events };
            (replay(&bundle(), &rec).ok(), replay(&bundle(), &rec).ok())
        });
        prop_assert_eq!(a, b);
    }

    /// Any ball rolling on the floor inside the table stays finite and inside
    /// the perimeter wall (or drains) for 300 ticks, for both profiles. (A
    /// ball launched upwards may legitimately clear the 28 mm wall, so the
    /// property is stated for balls that stay below the wall top.)
    #[test]
    fn injected_balls_stay_finite_and_on_the_table(
        ceramic in any::<bool>(),
        x in 0.02f32..0.49,
        y in 0.02f32..1.05,
        vx in -6.0f32..6.0,
        vy in -6.0f32..6.0,
    ) {
        let r: Result<(), TestCaseError> = on_sim_thread(move || {
            let mut sim = load();
            let profile = if ceramic { "ceramic_powerball" } else { "standard_steel" };
            let h = sim.spawn_ball(profile, [x, y, R], [vx, vy, 0.0]).unwrap();
            let mut airborne = false;
            for t in 0..300 {
                sim.advance(1);
                let Some(b) = sim.ball(h) else { break };
                let p = b.position_m;
                prop_assert!(p.iter().chain(&b.velocity_m_s).all(|c| c.is_finite()), "tick {}", t);
                // Centre above the wall top: it may leave over the wall.
                airborne |= p[2] > 0.028;
                prop_assert!(
                    airborne ||
                    (-0.002..=0.516).contains(&p[0]) && (-0.002..=1.072).contains(&p[1]),
                    "tick {}: left the table at {:?}", t, p
                );
            }
            Ok(())
        });
        r?;
    }
}
