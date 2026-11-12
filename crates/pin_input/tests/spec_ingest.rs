//! Independent black-box spec tests for `pin_input::events` (Subphase 3.1).
//!
//! Written against the fixed public API of the subphase brief, without sight
//! of the implementation.

use pin_core::input::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};
use pin_input::events::{CabinetFrame, IngestConfig, InputIngest};
use proptest::prelude::*;

const MS: u64 = 1_000_000;
const CAPACITY: usize = 256;

fn ingest(debounce_ms: u16) -> InputIngest {
    InputIngest::new(IngestConfig {
        epoch_ns: 0,
        debounce_ms,
    })
}

fn push(ing: &mut InputIngest, ev: InputEvent) {
    assert!(ing.push(ev).is_ok(), "push unexpectedly refused {ev:?}");
}

fn axis_index(a: CabinetAxis) -> usize {
    CabinetAxis::ALL.iter().position(|x| *x == a).unwrap()
}

/// First tick whose frame includes an event stamped `ts` (epoch 0).
fn tick_of(ts: u64) -> u64 {
    ts.div_ceil(MS)
}

// ---------------------------------------------------------------- basics

#[test]
fn default_config_is_epoch_zero_no_debounce() {
    let c = IngestConfig::default();
    assert_eq!(c.epoch_ns, 0);
    assert_eq!(c.debounce_ms, 0);
}

#[test]
fn initial_frame_is_neutral_and_echoes_tick() {
    let mut ing = InputIngest::new(IngestConfig::default());
    for t in [0u64, 1, 2, 10, 1000] {
        let f = ing.advance(t);
        assert_eq!(f.tick, t);
        assert_eq!(f.buttons, 0);
        assert_eq!(f.axes, [0.0; 5]);
        for i in CabinetInput::ALL {
            assert!(!f.pressed(i));
        }
        for a in CabinetAxis::ALL {
            assert_eq!(f.axis(a), 0.0);
        }
    }
}

#[test]
fn button_bits_follow_cabinet_input_index() {
    for i in CabinetInput::ALL {
        let mut ing = ingest(0);
        push(&mut ing, InputEvent::button(0, i, true));
        let f = ing.advance(0);
        assert_eq!(f.buttons, 1u8 << i.index(), "{i:?}");
        for j in CabinetInput::ALL {
            assert_eq!(f.pressed(j), i == j);
        }
        push(&mut ing, InputEvent::button(MS, i, false));
        let f = ing.advance(1);
        assert_eq!(f.buttons, 0);
    }
}

#[test]
fn all_buttons_together() {
    let mut ing = ingest(0);
    for i in CabinetInput::ALL {
        push(&mut ing, InputEvent::button(10, i, true));
    }
    let f = ing.advance(1);
    assert_eq!(f.buttons, 0xFF);
}

#[test]
fn axes_are_stored_in_cabinet_axis_all_order() {
    for (k, a) in CabinetAxis::ALL.into_iter().enumerate() {
        let mut ing = ingest(0);
        push(&mut ing, InputEvent::axis(0, a, 0.5));
        let f = ing.advance(0);
        for (j, b) in CabinetAxis::ALL.into_iter().enumerate() {
            let want = if j == k { 0.5 } else { 0.0 };
            assert_eq!(f.axes[j], want);
            assert_eq!(f.axis(b), want);
        }
    }
}

#[test]
fn frame_accessors_read_public_fields() {
    let f = CabinetFrame {
        tick: 7,
        buttons: 0b1000_0101,
        axes: [0.1, -0.2, 0.3, 0.4, 0.5],
    };
    for i in CabinetInput::ALL {
        assert_eq!(f.pressed(i), f.buttons & (1 << i.index()) != 0, "{i:?}");
    }
    assert_eq!(f.axis(CabinetAxis::Plunger), 0.1);
    assert_eq!(f.axis(CabinetAxis::NudgeX), -0.2);
    assert_eq!(f.axis(CabinetAxis::NudgeY), 0.3);
    assert_eq!(f.axis(CabinetAxis::LeftFlipper), 0.4);
    assert_eq!(f.axis(CabinetAxis::RightFlipper), 0.5);
}

#[test]
fn state_persists_across_ticks_without_events() {
    let mut ing = ingest(0);
    push(&mut ing, InputEvent::button(0, CabinetInput::Start, true));
    push(&mut ing, InputEvent::axis(0, CabinetAxis::Plunger, 0.75));
    for t in 0..50 {
        let f = ing.advance(t);
        assert!(f.pressed(CabinetInput::Start));
        assert_eq!(f.axis(CabinetAxis::Plunger), 0.75);
    }
}

// ---------------------------------------------------------------- timing

#[test]
fn event_applies_on_the_tick_its_timestamp_falls_in() {
    // ts <= tick * 1 ms  =>  included in that tick's frame.
    for (ts, want_tick) in [
        (0u64, 0u64),
        (1, 1),
        (MS - 1, 1),
        (MS, 1),
        (MS + 1, 2),
        (5 * MS, 5),
        (5 * MS + 1, 6),
        (17 * MS + 500_000, 18),
    ] {
        assert_eq!(tick_of(ts), want_tick);
        let mut ing = ingest(0);
        push(&mut ing, InputEvent::button(ts, CabinetInput::Launch, true));
        for t in 0..=want_tick + 2 {
            let f = ing.advance(t);
            assert_eq!(
                f.pressed(CabinetInput::Launch),
                t >= want_tick,
                "ts {ts} tick {t}"
            );
        }
    }
}

#[test]
fn epoch_offsets_tick_time() {
    let epoch = 1_000_000_000u64;
    let mut ing = InputIngest::new(IngestConfig {
        epoch_ns: epoch,
        debounce_ms: 0,
    });
    // Before the epoch: already due at tick 0.
    push(&mut ing, InputEvent::button(5, CabinetInput::Start, true));
    // 2.5 ms after the epoch: tick 3.
    push(
        &mut ing,
        InputEvent::button(epoch + 2 * MS + 500_000, CabinetInput::Action, true),
    );
    // Exactly on tick 4.
    push(
        &mut ing,
        InputEvent::axis(epoch + 4 * MS, CabinetAxis::Plunger, 1.0),
    );
    let want = [
        (true, false, 0.0),
        (true, false, 0.0),
        (true, false, 0.0),
        (true, true, 0.0),
        (true, true, 1.0),
        (true, true, 1.0),
    ];
    for (t, (start, action, plunger)) in want.into_iter().enumerate() {
        let f = ing.advance(t as u64);
        assert_eq!(f.tick, t as u64);
        assert_eq!(f.pressed(CabinetInput::Start), start, "tick {t}");
        assert_eq!(f.pressed(CabinetInput::Action), action, "tick {t}");
        assert_eq!(f.axis(CabinetAxis::Plunger), plunger, "tick {t}");
    }
}

#[test]
fn future_events_stay_pending() {
    let mut ing = ingest(0);
    push(
        &mut ing,
        InputEvent::button(100 * MS, CabinetInput::Tilt, true),
    );
    for t in 0..100 {
        assert!(!ing.advance(t).pressed(CabinetInput::Tilt), "tick {t}");
    }
    assert!(ing.advance(100).pressed(CabinetInput::Tilt));
}

#[test]
fn skipped_ticks_apply_everything_due() {
    let mut ing = ingest(0);
    push(
        &mut ing,
        InputEvent::button(2 * MS, CabinetInput::Start, true),
    );
    push(
        &mut ing,
        InputEvent::axis(7 * MS, CabinetAxis::NudgeX, -0.5),
    );
    push(
        &mut ing,
        InputEvent::button(9 * MS, CabinetInput::Start, false),
    );
    push(
        &mut ing,
        InputEvent::button(11 * MS, CabinetInput::Tilt, true),
    );
    let f = ing.advance(10);
    assert_eq!(f.tick, 10);
    assert!(!f.pressed(CabinetInput::Start));
    assert!(!f.pressed(CabinetInput::Tilt));
    assert_eq!(f.axis(CabinetAxis::NudgeX), -0.5);
}

// ------------------------------------------------------------- ordering

#[test]
fn out_of_order_delivery_is_applied_in_timestamp_order() {
    // Release (later timestamp) is delivered before the press.
    let mut ing = ingest(0);
    push(
        &mut ing,
        InputEvent::button(3 * MS, CabinetInput::LeftFlipper, false),
    );
    push(
        &mut ing,
        InputEvent::button(2 * MS, CabinetInput::LeftFlipper, true),
    );
    assert!(!ing.advance(5).pressed(CabinetInput::LeftFlipper));

    // Same, observed tick by tick: pressed on tick 2, released on tick 3.
    let mut ing = ingest(0);
    push(
        &mut ing,
        InputEvent::button(3 * MS, CabinetInput::LeftFlipper, false),
    );
    push(
        &mut ing,
        InputEvent::button(2 * MS, CabinetInput::LeftFlipper, true),
    );
    let got: Vec<bool> = (0..5)
        .map(|t| ing.advance(t).pressed(CabinetInput::LeftFlipper))
        .collect();
    assert_eq!(got, [false, false, true, false, false]);
}

#[test]
fn out_of_order_axis_values_end_on_latest_timestamp() {
    let mut ing = ingest(0);
    push(&mut ing, InputEvent::axis(900, CabinetAxis::Plunger, 0.9));
    push(&mut ing, InputEvent::axis(300, CabinetAxis::Plunger, 0.3));
    push(&mut ing, InputEvent::axis(600, CabinetAxis::Plunger, 0.6));
    assert_eq!(ing.advance(1).axis(CabinetAxis::Plunger), 0.9);
}

#[test]
fn equal_timestamps_keep_push_order() {
    let mut ing = ingest(0);
    push(&mut ing, InputEvent::button(MS, CabinetInput::Action, true));
    push(
        &mut ing,
        InputEvent::button(MS, CabinetInput::Action, false),
    );
    assert!(!ing.advance(1).pressed(CabinetInput::Action));

    let mut ing = ingest(0);
    push(
        &mut ing,
        InputEvent::button(MS, CabinetInput::Action, false),
    );
    push(&mut ing, InputEvent::button(MS, CabinetInput::Action, true));
    assert!(ing.advance(1).pressed(CabinetInput::Action));

    // Stability must survive interleaving with other timestamps.
    let mut ing = ingest(0);
    push(&mut ing, InputEvent::axis(MS, CabinetAxis::NudgeY, 0.1));
    push(&mut ing, InputEvent::axis(MS + 5, CabinetAxis::NudgeY, 0.9));
    push(&mut ing, InputEvent::axis(MS, CabinetAxis::NudgeY, 0.2));
    push(&mut ing, InputEvent::axis(MS - 5, CabinetAxis::NudgeY, 0.8));
    push(&mut ing, InputEvent::axis(MS, CabinetAxis::NudgeY, 0.3));
    assert_eq!(ing.advance(1).axis(CabinetAxis::NudgeY), 0.3);
    assert_eq!(ing.advance(2).axis(CabinetAxis::NudgeY), 0.9);
}

#[test]
fn press_and_release_within_one_tick_yield_final_state() {
    let mut ing = ingest(0);
    push(
        &mut ing,
        InputEvent::button(2 * MS + 100, CabinetInput::Start, true),
    );
    push(
        &mut ing,
        InputEvent::button(2 * MS + 200, CabinetInput::Start, false),
    );
    for t in 0..6 {
        assert!(!ing.advance(t).pressed(CabinetInput::Start), "tick {t}");
    }

    let mut ing = ingest(0);
    push(&mut ing, InputEvent::button(0, CabinetInput::Start, true));
    assert!(ing.advance(0).pressed(CabinetInput::Start));
    push(
        &mut ing,
        InputEvent::button(2 * MS + 100, CabinetInput::Start, false),
    );
    push(
        &mut ing,
        InputEvent::button(2 * MS + 200, CabinetInput::Start, true),
    );
    for t in 1..6 {
        assert!(ing.advance(t).pressed(CabinetInput::Start), "tick {t}");
    }
}

#[test]
fn late_events_are_applied_at_the_next_advance_never_dropped() {
    let mut ing = ingest(0);
    assert_eq!(ing.advance(10).buttons, 0);
    // Stamped well before the tick we already advanced to.
    push(
        &mut ing,
        InputEvent::button(2 * MS, CabinetInput::Start, true),
    );
    push(
        &mut ing,
        InputEvent::axis(3 * MS, CabinetAxis::Plunger, 0.4),
    );
    let f = ing.advance(11);
    assert!(f.pressed(CabinetInput::Start));
    assert_eq!(f.axis(CabinetAxis::Plunger), 0.4);

    // Late events delivered out of order still apply in timestamp order.
    push(
        &mut ing,
        InputEvent::button(5 * MS, CabinetInput::Start, true),
    );
    push(
        &mut ing,
        InputEvent::button(4 * MS, CabinetInput::Start, false),
    );
    assert!(ing.advance(12).pressed(CabinetInput::Start));
}

// ------------------------------------------------------ clamping and NaN

#[test]
fn unit_axes_clamp_to_zero_one() {
    for a in [
        CabinetAxis::Plunger,
        CabinetAxis::LeftFlipper,
        CabinetAxis::RightFlipper,
    ] {
        for (v, want) in [
            (1.5f32, 1.0f32),
            (-0.5, 0.0),
            (-1.0, 0.0),
            (1.0, 1.0),
            (0.0, 0.0),
            (0.25, 0.25),
            (f32::INFINITY, 1.0),
            (f32::NEG_INFINITY, 0.0),
            (f32::MAX, 1.0),
            (f32::MIN, 0.0),
        ] {
            let mut ing = ingest(0);
            // Start from a distinct value so "ignored" cannot pass for "clamped".
            push(&mut ing, InputEvent::axis(0, a, 0.5));
            push(&mut ing, InputEvent::axis(1, a, v));
            assert_eq!(ing.advance(1).axis(a), want, "{a:?} {v}");
        }
    }
}

#[test]
fn nudge_axes_clamp_to_minus_one_one() {
    for a in [CabinetAxis::NudgeX, CabinetAxis::NudgeY] {
        for (v, want) in [
            (3.0f32, 1.0f32),
            (-3.0, -1.0),
            (-1.0, -1.0),
            (1.0, 1.0),
            (-0.5, -0.5),
            (0.5, 0.5),
            (f32::INFINITY, 1.0),
            (f32::NEG_INFINITY, -1.0),
        ] {
            let mut ing = ingest(0);
            push(&mut ing, InputEvent::axis(0, a, 0.125));
            push(&mut ing, InputEvent::axis(1, a, v));
            assert_eq!(ing.advance(1).axis(a), want, "{a:?} {v}");
        }
    }
}

#[test]
fn nan_axis_values_are_ignored() {
    for a in CabinetAxis::ALL {
        let mut ing = ingest(0);
        push(&mut ing, InputEvent::axis(0, a, f32::NAN));
        assert_eq!(ing.advance(0).axis(a), 0.0, "{a:?}");
        push(&mut ing, InputEvent::axis(MS, a, 0.5));
        assert_eq!(ing.advance(1).axis(a), 0.5);
        push(&mut ing, InputEvent::axis(2 * MS, a, f32::NAN));
        push(&mut ing, InputEvent::axis(2 * MS, a, -f32::NAN));
        let f = ing.advance(2);
        assert_eq!(f.axis(a), 0.5, "{a:?}");
        // Only that axis moved, nothing else was disturbed.
        for b in CabinetAxis::ALL {
            if b != a {
                assert_eq!(f.axis(b), 0.0);
            }
        }
        // NaN followed by a real value in the same tick: the real value wins.
        push(&mut ing, InputEvent::axis(3 * MS, a, f32::NAN));
        push(&mut ing, InputEvent::axis(3 * MS, a, 0.25));
        assert_eq!(ing.advance(3).axis(a), 0.25);
    }
}

// -------------------------------------------------------------- capacity

#[test]
fn push_refuses_the_257th_pending_event_and_returns_it() {
    let mut ing = ingest(0);
    for k in 0..CAPACITY as u64 {
        push(
            &mut ing,
            InputEvent::axis(k, CabinetAxis::Plunger, k as f32 / 256.0),
        );
    }
    let extra = InputEvent::button(12_345, CabinetInput::Tilt, true);
    assert_eq!(ing.push(extra), Err(extra));
    // Still full, and the refused event did not sneak in.
    let extra2 = InputEvent::axis(1, CabinetAxis::NudgeX, 1.0);
    assert_eq!(ing.push(extra2), Err(extra2));

    let f = ing.advance(1);
    assert_eq!(f.axis(CabinetAxis::Plunger), 255.0 / 256.0);
    assert!(!f.pressed(CabinetInput::Tilt));
    assert_eq!(f.axis(CabinetAxis::NudgeX), 0.0);

    // Draining frees the whole capacity again.
    for k in 0..CAPACITY as u64 {
        push(
            &mut ing,
            InputEvent::button(2 * MS + k, CabinetInput::Start, k % 2 == 0),
        );
    }
    assert!(
        ing.push(InputEvent::button(0, CabinetInput::Start, true))
            .is_err()
    );
    assert!(!ing.advance(3).pressed(CabinetInput::Start));
}

#[test]
fn events_not_yet_due_keep_occupying_capacity() {
    let mut ing = ingest(0);
    for k in 0..CAPACITY as u64 {
        push(
            &mut ing,
            InputEvent::button(1000 * MS + k + 1, CabinetInput::Start, true),
        );
    }
    assert_eq!(ing.advance(5).buttons, 0);
    let ev = InputEvent::button(0, CabinetInput::Action, true);
    assert_eq!(ing.push(ev), Err(ev));
    assert!(ing.advance(1000).buttons == 0);
    assert!(ing.advance(1001).pressed(CabinetInput::Start));
    assert!(ing.push(ev).is_ok());
    assert!(ing.advance(1002).pressed(CabinetInput::Action));
}

#[test]
fn partially_drained_queue_frees_exactly_the_drained_slots() {
    let mut ing = ingest(0);
    // 100 due at tick 1, 156 due at tick 50.
    for k in 0..100u64 {
        push(&mut ing, InputEvent::axis(k, CabinetAxis::NudgeX, 0.5));
    }
    for k in 0..156u64 {
        push(
            &mut ing,
            InputEvent::axis(50 * MS - k, CabinetAxis::NudgeY, 0.5),
        );
    }
    ing.advance(1);
    for k in 0..100u64 {
        push(
            &mut ing,
            InputEvent::axis(60 * MS + k, CabinetAxis::Plunger, 1.0),
        );
    }
    let ev = InputEvent::axis(0, CabinetAxis::Plunger, 0.0);
    assert_eq!(ing.push(ev), Err(ev));
}

#[test]
fn full_queue_delivered_in_reverse_is_applied_in_order() {
    let mut ing = ingest(0);
    for k in (0..CAPACITY as u64).rev() {
        push(
            &mut ing,
            InputEvent::axis(k * 1000, CabinetAxis::LeftFlipper, k as f32 / 255.0),
        );
    }
    assert_eq!(ing.advance(1).axis(CabinetAxis::LeftFlipper), 1.0);
}

// -------------------------------------------------------------- debounce

fn trace(
    ing: &mut InputIngest,
    input: CabinetInput,
    ticks: std::ops::RangeInclusive<u64>,
) -> Vec<bool> {
    ticks.map(|t| ing.advance(t).pressed(input)).collect()
}

fn expect_pressed_iff(got: &[bool], first_tick: u64, pred: impl Fn(u64) -> bool) {
    for (k, g) in got.iter().enumerate() {
        let t = first_tick + k as u64;
        assert_eq!(*g, pred(t), "tick {t}: trace {got:?}");
    }
}

const L: CabinetInput = CabinetInput::LeftFlipper;
const R: CabinetInput = CabinetInput::RightFlipper;

#[test]
fn debounce_first_edge_is_immediate() {
    let mut ing = ingest(5);
    push(&mut ing, InputEvent::button(500_000, L, true));
    let got = trace(&mut ing, L, 0..=12);
    expect_pressed_iff(&got, 0, |t| t >= 1);
}

#[test]
fn debounce_holds_edge_until_window_end_then_applies_final_state() {
    // Press accepted at tick 1; release at tick 3 falls inside the window
    // [1, 6) and is deferred to tick 1 + 5 = 6.
    let mut ing = ingest(5);
    push(&mut ing, InputEvent::button(500_000, L, true));
    push(&mut ing, InputEvent::button(2 * MS + 500_000, L, false));
    let got = trace(&mut ing, L, 0..=14);
    expect_pressed_iff(&got, 0, |t| (1..=5).contains(&t));
}

#[test]
fn debounce_swallows_contact_bounce() {
    // press, release, press inside the window: state at window end equals the
    // accepted state, so nothing changes.
    let mut ing = ingest(5);
    push(&mut ing, InputEvent::button(500_000, L, true));
    push(&mut ing, InputEvent::button(MS + 500_000, L, false));
    push(&mut ing, InputEvent::button(2 * MS + 500_000, L, true));
    push(&mut ing, InputEvent::button(3 * MS + 100, L, false));
    push(&mut ing, InputEvent::button(3 * MS + 200, L, true));
    let got = trace(&mut ing, L, 0..=20);
    expect_pressed_iff(&got, 0, |t| t >= 1);
}

#[test]
fn debounce_edge_after_window_applies_on_its_own_tick() {
    let mut ing = ingest(5);
    push(&mut ing, InputEvent::button(500_000, L, true));
    push(&mut ing, InputEvent::button(9 * MS + 500_000, L, false));
    let got = trace(&mut ing, L, 0..=20);
    expect_pressed_iff(&got, 0, |t| (1..=9).contains(&t));
}

#[test]
fn debounce_applies_to_release_edges_too() {
    // press @1 (accepted), release @10 (accepted, window to 15), press @12
    // deferred to tick 15.
    let mut ing = ingest(5);
    push(&mut ing, InputEvent::button(500_000, L, true));
    push(&mut ing, InputEvent::button(9 * MS + 500_000, L, false));
    push(&mut ing, InputEvent::button(11 * MS + 500_000, L, true));
    let got = trace(&mut ing, L, 0..=25);
    expect_pressed_iff(&got, 0, |t| (1..=9).contains(&t) || t >= 15);
}

#[test]
fn debounce_is_per_input() {
    let mut ing = ingest(8);
    push(&mut ing, InputEvent::button(500_000, L, true));
    push(&mut ing, InputEvent::button(MS + 500_000, R, true));
    push(
        &mut ing,
        InputEvent::button(2 * MS + 500_000, CabinetInput::Start, true),
    );
    let mut frames = Vec::new();
    for t in 0..=4 {
        frames.push(ing.advance(t));
    }
    assert!(!frames[0].pressed(L));
    assert!(frames[1].pressed(L) && !frames[1].pressed(R));
    assert!(frames[2].pressed(R) && !frames[2].pressed(CabinetInput::Start));
    assert!(frames[3].pressed(CabinetInput::Start));
    assert_eq!(
        frames[4].buttons,
        (1 << L.index()) | (1 << R.index()) | (1 << CabinetInput::Start.index())
    );
}

#[test]
fn debounce_does_not_delay_axes() {
    let mut ing = ingest(20);
    for k in 0..=10u64 {
        push(
            &mut ing,
            InputEvent::axis(k * MS, CabinetAxis::LeftFlipper, k as f32 / 10.0),
        );
    }
    for k in 0..=10u64 {
        assert_eq!(
            ing.advance(k).axis(CabinetAxis::LeftFlipper),
            k as f32 / 10.0
        );
    }
}

#[test]
fn redundant_event_is_not_an_edge_and_opens_no_window() {
    // press @1 (window to 6). A repeated "pressed" report @8 changes nothing,
    // so the release @9 is the first edge after the window and is immediate.
    let mut ing = ingest(5);
    push(&mut ing, InputEvent::button(500_000, L, true));
    push(&mut ing, InputEvent::button(7 * MS + 500_000, L, true));
    push(&mut ing, InputEvent::button(8 * MS + 500_000, L, false));
    let got = trace(&mut ing, L, 0..=15);
    expect_pressed_iff(&got, 0, |t| (1..=8).contains(&t));
}

#[test]
fn debounce_of_one_ms_behaves_like_per_tick_latching() {
    let mut ing = ingest(1);
    push(&mut ing, InputEvent::button(500_000, L, true));
    push(&mut ing, InputEvent::button(MS + 500_000, L, false));
    push(&mut ing, InputEvent::button(2 * MS + 500_000, L, true));
    let got = trace(&mut ing, L, 0..=5);
    assert_eq!(got, [false, true, false, true, true, true]);
}

// ------------------------------------------------------------ robustness

#[test]
fn extreme_timestamps_and_ticks_do_not_panic() {
    for debounce_ms in [0u16, 1, u16::MAX] {
        for epoch_ns in [0u64, 1, u64::MAX / 2, u64::MAX] {
            let mut ing = InputIngest::new(IngestConfig {
                epoch_ns,
                debounce_ms,
            });
            push(&mut ing, InputEvent::button(u64::MAX, L, true));
            push(&mut ing, InputEvent::button(0, R, true));
            push(
                &mut ing,
                InputEvent::axis(u64::MAX - 1, CabinetAxis::NudgeX, -9.0),
            );
            for t in [
                0u64,
                1,
                1 << 40,
                u64::MAX / MS,
                u64::MAX / MS + 1,
                u64::MAX - 1,
                u64::MAX,
            ] {
                let f = ing.advance(t);
                assert_eq!(f.tick, t);
                assert!(f.pressed(R));
            }
        }
    }
}

// --------------------------------------------------------------- proptest

fn arb_axis_value() -> impl Strategy<Value = f32> {
    prop_oneof![
        8 => -2.0f32..2.0f32,
        1 => Just(f32::NAN),
        1 => Just(f32::INFINITY),
        1 => Just(f32::NEG_INFINITY),
        1 => any::<f32>(),
    ]
}

fn arb_kind() -> impl Strategy<Value = InputEventKind> {
    prop_oneof![
        (0usize..8, any::<bool>()).prop_map(|(i, pressed)| InputEventKind::Button {
            input: CabinetInput::ALL[i],
            pressed
        }),
        (0usize..5, arb_axis_value()).prop_map(|(a, value)| InputEventKind::Axis {
            axis: CabinetAxis::ALL[a],
            value
        }),
    ]
}

#[derive(Clone, Debug)]
enum Op {
    /// Push an event stamped `now_ns + offset` (offset may be negative = late
    /// or out of order).
    Push(i64, InputEventKind),
    /// Advance by this many ticks (>= 1).
    Advance(u64),
}

fn arb_ops() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(
        prop_oneof![
            3 => (-4_000_000i64..6_000_000i64, arb_kind()).prop_map(|(o, k)| Op::Push(o, k)),
            1 => (1u64..4).prop_map(Op::Advance),
        ],
        0..200,
    )
}

/// Reference model for `debounce_ms == 0`.
struct Model {
    epoch: u64,
    seq: u64,
    pending: Vec<(u64, u64, InputEventKind)>,
    buttons: u8,
    axes: [f32; 5],
}

impl Model {
    fn advance(&mut self, tick: u64) {
        let deadline = self.epoch + tick * MS;
        let mut due: Vec<_> = self
            .pending
            .iter()
            .copied()
            .filter(|e| e.0 <= deadline)
            .collect();
        self.pending.retain(|e| e.0 > deadline);
        due.sort_by_key(|e| (e.0, e.1));
        for (_, _, kind) in due {
            match kind {
                InputEventKind::Button { input, pressed } => {
                    if pressed {
                        self.buttons |= 1 << input.index();
                    } else {
                        self.buttons &= !(1 << input.index());
                    }
                }
                InputEventKind::Axis { axis, value } => {
                    if value.is_nan() {
                        continue;
                    }
                    let lo = match axis {
                        CabinetAxis::NudgeX | CabinetAxis::NudgeY => -1.0,
                        _ => 0.0,
                    };
                    self.axes[axis_index(axis)] = value.clamp(lo, 1.0);
                }
            }
        }
    }
}

/// At most one button report per tick (tick -> (pressed, ns before the tick
/// boundary)), so the raw per-tick state is unambiguous under debounce.
fn arb_per_tick_edges() -> impl Strategy<Value = std::collections::BTreeMap<u64, (bool, u64)>> {
    prop::collection::btree_map(1u64..60, (any::<bool>(), 0u64..MS), 0..40)
}

/// Timestamp-sorted `(timestamp_ns, pressed)` list.
fn edges_from(m: &std::collections::BTreeMap<u64, (bool, u64)>) -> Vec<(u64, bool)> {
    m.iter()
        .map(|(tick, (pressed, back))| (tick * MS - back, *pressed))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// With no debounce the ingest equals "stable sort by timestamp, apply
    /// everything due" for any interleaving of pushes and advances.
    #[test]
    fn prop_matches_reference_model_without_debounce(
        epoch in prop_oneof![Just(0u64), 0u64..10_000_000_000],
        ops in arb_ops(),
    ) {
        let mut ing = InputIngest::new(IngestConfig { epoch_ns: epoch, debounce_ms: 0 });
        let mut model = Model { epoch, seq: 0, pending: Vec::new(), buttons: 0, axes: [0.0; 5] };
        let mut tick = 0u64;
        let f = ing.advance(0);
        prop_assert_eq!((f.tick, f.buttons, f.axes), (0, 0, [0.0; 5]));
        for op in ops {
            match op {
                Op::Push(offset, kind) => {
                    let now = epoch + tick * MS;
                    let ts = now.saturating_add_signed(offset);
                    let ev = InputEvent { timestamp_ns: ts, kind };
                    if model.pending.len() < CAPACITY {
                        prop_assert!(ing.push(ev).is_ok());
                        model.pending.push((ts, model.seq, kind));
                        model.seq += 1;
                    } else {
                        prop_assert!(ing.push(ev).is_err());
                    }
                }
                Op::Advance(dt) => {
                    tick += dt;
                    let f = ing.advance(tick);
                    model.advance(tick);
                    prop_assert_eq!(f.tick, tick);
                    prop_assert_eq!(f.buttons, model.buttons, "tick {}", tick);
                    prop_assert_eq!(f.axes, model.axes, "tick {}", tick);
                    for i in CabinetInput::ALL {
                        prop_assert_eq!(f.pressed(i), model.buttons & (1 << i.index()) != 0);
                    }
                    for a in CabinetAxis::ALL {
                        prop_assert_eq!(f.axis(a), model.axes[axis_index(a)]);
                    }
                }
            }
        }
        // Flush: nothing is ever dropped.
        tick += 10;
        let f = ing.advance(tick);
        model.advance(tick);
        prop_assert!(model.pending.is_empty());
        prop_assert_eq!(f.buttons, model.buttons);
        prop_assert_eq!(f.axes, model.axes);
    }

    /// Delivery order does not matter as long as timestamps are distinct.
    #[test]
    fn prop_delivery_order_is_irrelevant_for_distinct_timestamps(
        kinds in prop::collection::vec(arb_kind(), 1..100),
        gaps in prop::collection::vec(1u64..700_000, 100),
        seed in any::<u64>(),
    ) {
        let mut ts = 0u64;
        let events: Vec<InputEvent> = kinds.iter().zip(&gaps).map(|(k, g)| {
            ts += g;
            InputEvent { timestamp_ns: ts, kind: *k }
        }).collect();
        // Deterministic shuffle (Fisher-Yates with an LCG).
        let mut shuffled = events.clone();
        let mut s = seed;
        for i in (1..shuffled.len()).rev() {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            shuffled.swap(i, (s >> 33) as usize % (i + 1));
        }
        let mut a = ingest(0);
        let mut b = ingest(0);
        for e in &events { prop_assert!(a.push(*e).is_ok()); }
        for e in &shuffled { prop_assert!(b.push(*e).is_ok()); }
        for t in 0..=tick_of(ts) + 1 {
            let fa = a.advance(t);
            let fb = b.advance(t);
            prop_assert_eq!(fa.buttons, fb.buttons, "tick {}", t);
            prop_assert_eq!(fa.axes, fb.axes, "tick {}", t);
        }
    }

    /// Never panics, never leaves the documented value ranges, for any
    /// config, any event content and any (monotone) tick sequence.
    #[test]
    fn prop_never_panics_and_axes_stay_in_range(
        epoch in any::<u64>(),
        debounce_ms in any::<u16>(),
        events in prop::collection::vec((any::<u64>(), arb_kind()), 0..400),
        mut ticks in prop::collection::vec(any::<u64>(), 1..20),
        small in any::<bool>(),
    ) {
        let mut ing = InputIngest::new(IngestConfig { epoch_ns: epoch, debounce_ms });
        if small {
            for t in &mut ticks {
                *t %= 5000;
            }
        }
        ticks.sort_unstable();
        ticks.dedup();
        let mut it = events.into_iter();
        for t in ticks {
            for (ts, kind) in it.by_ref().take(40) {
                let ts = if small { ts % (5000 * MS) } else { ts };
                let _ = ing.push(InputEvent { timestamp_ns: ts, kind });
            }
            let f = ing.advance(t);
            prop_assert_eq!(f.tick, t);
            for a in CabinetAxis::ALL {
                let v = f.axis(a);
                let lo = match a {
                    CabinetAxis::NudgeX | CabinetAxis::NudgeY => -1.0,
                    _ => 0.0,
                };
                prop_assert!((lo..=1.0).contains(&v), "{:?} = {}", a, v);
            }
        }
    }

    /// Capacity is exactly 256 pending events regardless of content.
    #[test]
    fn prop_capacity_is_256(
        events in prop::collection::vec((1_000_000_000u64..2_000_000_000, arb_kind()), 257..300),
        debounce_ms in 0u16..20,
    ) {
        let mut ing = ingest(debounce_ms);
        for (k, (ts, kind)) in events.into_iter().enumerate() {
            let ev = InputEvent { timestamp_ns: ts, kind };
            let r = ing.push(ev);
            if k < CAPACITY {
                prop_assert!(r.is_ok(), "push {} refused", k);
            } else {
                match r {
                    Err(back) => {
                        prop_assert_eq!(back.timestamp_ns, ts);
                        // Bitwise comparison so NaN payloads compare equal.
                        prop_assert_eq!(format!("{:?}", back.kind), format!("{:?}", kind));
                    }
                    Ok(()) => prop_assert!(false, "push {} accepted beyond capacity", k),
                }
            }
        }
    }

    /// Debounce invariants on one input:
    /// * the debounced output never changes twice within `debounce_ms` ticks;
    /// * the first edge is applied on the tick its timestamp falls in;
    /// * after the input goes quiet the output settles on the raw state.
    #[test]
    fn prop_debounce_spacing_and_settling(
        debounce_ms in 1u16..12,
        per_tick in arb_per_tick_edges(),
    ) {
        let mut ing = ingest(debounce_ms);
        let edges = edges_from(&per_tick);
        // Deliver newest first: delivery order must not matter.
        for (ts, pressed) in edges.iter().rev() {
            prop_assert!(ing.push(InputEvent::button(*ts, L, *pressed)).is_ok());
        }
        let raw_at = |t: u64| edges.iter().rev().find(|e| e.0 <= t * MS).map(|e| e.1).unwrap_or(false);
        let last_tick = 60 + 3 * debounce_ms as u64 + 5;
        let out: Vec<bool> = (0..=last_tick).map(|t| ing.advance(t).pressed(L)).collect();

        let changes: Vec<u64> = (0..=last_tick)
            .filter(|&t| out[t as usize] != if t == 0 { false } else { out[t as usize - 1] })
            .collect();
        for w in changes.windows(2) {
            prop_assert!(w[1] - w[0] >= debounce_ms as u64,
                "output changed at ticks {} and {} with debounce {}: {:?}", w[0], w[1], debounce_ms, out);
        }
        // First tick at which the raw end-of-tick state is "pressed".
        if let Some(first) = (0..=last_tick).find(|&t| raw_at(t)) {
            for t in 0..first {
                prop_assert!(!out[t as usize], "pressed before any raw press, tick {}", t);
            }
            prop_assert!(out[first as usize], "first press not applied on tick {}: {:?}", first, out);
        } else {
            prop_assert!(changes.is_empty());
        }
        prop_assert_eq!(out[last_tick as usize], raw_at(last_tick));
        // Other inputs untouched.
        prop_assert_eq!(ing.advance(last_tick + 1).buttons & !(1 << L.index()), 0);
    }

    /// A debounced output only ever shows a state the raw input actually had
    /// at some tick boundary within the last `debounce_ms` ticks.
    #[test]
    fn prop_debounced_output_lags_raw_by_at_most_the_window(
        debounce_ms in 1u16..12,
        per_tick in arb_per_tick_edges(),
    ) {
        let mut ing = ingest(debounce_ms);
        let edges = edges_from(&per_tick);
        for (ts, pressed) in &edges {
            prop_assert!(ing.push(InputEvent::button(*ts, R, *pressed)).is_ok());
        }
        let raw_at = |t: u64| edges.iter().rev().find(|e| e.0 <= t * MS).map(|e| e.1).unwrap_or(false);
        for t in 0..=90u64 {
            let got = ing.advance(t).pressed(R);
            let lo = t.saturating_sub(debounce_ms as u64);
            prop_assert!((lo..=t).any(|u| raw_at(u) == got),
                "tick {}: output {} matches no raw state in [{}, {}]", t, got, lo, t);
        }
    }
}
