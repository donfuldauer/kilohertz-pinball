//! Implementer integration tests for Subphase 3.1: exact-tick actuation in all
//! three staging modes, and proptests over ingest and router invariants.

use arrayvec::ArrayVec;
use pin_core::flipper::FlipperStagingMode;
use pin_core::ids::FlipperId;
use pin_core::input::{CabinetAxis, CabinetInput, InputEvent};
use pin_input::events::INGEST_CAPACITY;
use pin_input::staging::entry_active;
use pin_input::{
    CabinetFrame, DEFAULT_LOWER_THRESHOLD, DEFAULT_UPPER_THRESHOLD, FlipperStage,
    FlipperStagingRouter, IngestConfig, InputIngest, StagingEntry,
};
use proptest::prelude::*;

const MS: u64 = 1_000_000;

fn entries() -> [StagingEntry; 4] {
    let dual = FlipperStagingMode::DualStageThreshold {
        axis: CabinetAxis::RightFlipper,
        lower_threshold: DEFAULT_LOWER_THRESHOLD,
        upper_threshold: DEFAULT_UPPER_THRESHOLD,
    };
    [
        StagingEntry {
            flipper: FlipperId(0),
            mode: FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper),
            stage: FlipperStage::Lower,
        },
        StagingEntry {
            flipper: FlipperId(1),
            mode: FlipperStagingMode::ParallelLinked(CabinetInput::LeftFlipper),
            stage: FlipperStage::Upper,
        },
        StagingEntry {
            flipper: FlipperId(2),
            mode: dual,
            stage: FlipperStage::Lower,
        },
        StagingEntry {
            flipper: FlipperId(3),
            mode: dual,
            stage: FlipperStage::Upper,
        },
    ]
}

/// Runs ticks `0..ticks` and records the tick of every router change.
fn run(
    ingest: &mut InputIngest,
    router: &mut FlipperStagingRouter,
    ticks: u64,
) -> Vec<(u64, FlipperId, bool)> {
    let mut log = Vec::new();
    let mut out = ArrayVec::new();
    for t in 0..ticks {
        let f = ingest.advance(t);
        out.clear();
        router.update(&f, &mut out);
        log.extend(out.iter().map(|&(id, on)| (t, id, on)));
    }
    log
}

#[test]
fn analog_ramp_actuates_on_exact_ticks() {
    let mut ingest = InputIngest::new(IngestConfig::default());
    let mut router = FlipperStagingRouter::new(&entries()).unwrap();
    // Ramp 0 -> 1 in 0.01 steps, one per tick, timestamped mid-tick.
    for k in 0..=100u64 {
        let v = k as f32 / 100.0;
        // Event at (k - 1) ms + 0.5 ms falls in tick k (k >= 1).
        let ts = (k * MS).saturating_sub(MS / 2);
        ingest
            .push(InputEvent::axis(ts, CabinetAxis::RightFlipper, v))
            .unwrap();
    }
    let log = run(&mut ingest, &mut router, 120);
    // 0.30 reached at k = 30, 0.85 at k = 85 (f32 30/100 >= 0.30 holds exactly).
    assert_eq!(
        log,
        vec![(30, FlipperId(2), true), (85, FlipperId(3), true)]
    );
}

#[test]
fn button_sequence_actuates_on_exact_ticks() {
    let mut ingest = InputIngest::new(IngestConfig {
        epoch_ns: 7 * MS + 123,
        debounce_ms: 0,
    });
    let mut router = FlipperStagingRouter::new(&entries()).unwrap();
    let base = 7 * MS + 123;
    ingest
        .push(InputEvent::button(
            base + 10 * MS,
            CabinetInput::LeftFlipper,
            true,
        ))
        .unwrap();
    ingest
        .push(InputEvent::button(
            base + 20 * MS + 1,
            CabinetInput::LeftFlipper,
            false,
        ))
        .unwrap();
    let log = run(&mut ingest, &mut router, 30);
    assert_eq!(
        log,
        vec![
            (10, FlipperId(0), true),
            (10, FlipperId(1), true),
            (21, FlipperId(0), false),
            (21, FlipperId(1), false),
        ]
    );
}

#[test]
fn debounced_button_actuates_once() {
    let mut ingest = InputIngest::new(IngestConfig {
        epoch_ns: 0,
        debounce_ms: 4,
    });
    let mut router = FlipperStagingRouter::new(&entries()[..1]).unwrap();
    // Bouncy press at 5 ms, settles pressed; release at 40 ms with bounce.
    for (ms, p) in [
        (5, true),
        (6, false),
        (7, true),
        (40, false),
        (41, true),
        (42, false),
    ] {
        ingest
            .push(InputEvent::button(ms * MS, CabinetInput::LeftFlipper, p))
            .unwrap();
    }
    let log = run(&mut ingest, &mut router, 60);
    assert_eq!(
        log,
        vec![(5, FlipperId(0), true), (40, FlipperId(0), false)]
    );
}

fn arb_event() -> impl Strategy<Value = InputEvent> {
    let ts = 0u64..50 * MS;
    prop_oneof![
        (ts.clone(), 0usize..8, any::<bool>()).prop_map(|(t, i, p)| InputEvent::button(
            t,
            CabinetInput::ALL[i],
            p
        )),
        (
            ts,
            0usize..5,
            prop_oneof![
                -3.0f32..3.0,
                Just(f32::NAN),
                Just(f32::INFINITY),
                Just(f32::NEG_INFINITY)
            ]
        )
            .prop_map(|(t, a, v)| InputEvent::axis(t, CabinetAxis::ALL[a], v)),
    ]
}

/// Reference model for debounce 0: stable-sort by timestamp, apply each.
fn reference_frames(events: &[InputEvent], ticks: u64) -> Vec<CabinetFrame> {
    let mut sorted: Vec<InputEvent> = events.to_vec();
    sorted.sort_by_key(|e| e.timestamp_ns);
    let mut f = CabinetFrame::default();
    let mut idx = 0;
    let mut frames = Vec::new();
    for t in 0..ticks {
        while idx < sorted.len() && sorted[idx].timestamp_ns <= t * MS {
            match sorted[idx].kind {
                pin_core::input::InputEventKind::Button { input, pressed } => {
                    let bit = 1u8 << input.index();
                    if pressed {
                        f.buttons |= bit
                    } else {
                        f.buttons &= !bit
                    }
                }
                pin_core::input::InputEventKind::Axis { axis, value } => {
                    if !value.is_nan() {
                        let i = CabinetAxis::ALL.iter().position(|&a| a == axis).unwrap();
                        let lo = if i == 1 || i == 2 { -1.0 } else { 0.0 };
                        f.axes[i] = value.clamp(lo, 1.0);
                    }
                }
            }
            idx += 1;
        }
        f.tick = t;
        frames.push(f);
    }
    frames
}

proptest! {
    #[test]
    fn ingest_matches_reference(events in prop::collection::vec(arb_event(), 0..200)) {
        let mut g = InputIngest::new(IngestConfig::default());
        for e in &events {
            prop_assert!(g.push(*e).is_ok());
        }
        let want = reference_frames(&events, 55);
        for t in 0..55u64 {
            let got = g.advance(t);
            prop_assert_eq!(got.tick, t);
            prop_assert_eq!(got.buttons, want[t as usize].buttons);
            for i in 0..5 {
                prop_assert_eq!(got.axes[i].to_bits(), want[t as usize].axes[i].to_bits());
                prop_assert!(!got.axes[i].is_nan());
            }
        }
        prop_assert_eq!(g.pending_len(), 0);
    }

    #[test]
    fn ingest_never_panics(
        events in prop::collection::vec((any::<u64>(), arb_event()), 0..400),
        epoch in any::<u64>(),
        debounce in any::<u16>(),
        ticks in prop::collection::vec(any::<u64>(), 0..20),
    ) {
        let mut g = InputIngest::new(IngestConfig { epoch_ns: epoch, debounce_ms: debounce });
        let mut accepted = 0usize;
        for (ts, mut e) in events {
            e.timestamp_ns = ts;
            if g.push(e).is_ok() { accepted += 1; }
            prop_assert!(g.pending_len() <= INGEST_CAPACITY);
        }
        prop_assert!(accepted <= INGEST_CAPACITY);
        for t in ticks {
            let f = g.advance(t);
            prop_assert_eq!(f.tick, t);
        }
        let _ = g.advance(u64::MAX);
        prop_assert_eq!(g.pending_len(), 0);
    }

    #[test]
    fn debounce_edges_spaced_by_window(
        presses in prop::collection::vec((0u64..200, any::<bool>()), 0..100),
        debounce in 1u16..20,
    ) {
        let mut g = InputIngest::new(IngestConfig { epoch_ns: 0, debounce_ms: debounce });
        for (ms, p) in &presses {
            g.push(InputEvent::button(ms * MS + 17, CabinetInput::Start, *p)).unwrap();
        }
        let mut last_edge: Option<u64> = None;
        let mut prev = false;
        for t in 0..300u64 {
            let now = g.advance(t).pressed(CabinetInput::Start);
            if now != prev {
                if let Some(l) = last_edge {
                    prop_assert!(t - l >= u64::from(debounce));
                }
                last_edge = Some(t);
                prev = now;
            }
        }
        // Settles to the final raw state.
        let mut sorted = presses.clone();
        sorted.sort_by_key(|p| p.0);
        let final_raw = sorted.last().is_some_and(|p| p.1);
        prop_assert_eq!(prev, final_raw);
    }

    #[test]
    fn monotone_ramp_crosses_each_threshold_once(
        mut steps in prop::collection::vec(0.0f32..1.0, 1..200),
        lower in 0.0f32..1.0,
        span in 0.0f32..1.0,
    ) {
        let upper = (lower + span).min(1.0);
        steps.sort_by(f32::total_cmp);
        let mode = FlipperStagingMode::DualStageThreshold {
            axis: CabinetAxis::LeftFlipper,
            lower_threshold: lower,
            upper_threshold: upper,
        };
        let mut r = FlipperStagingRouter::new(&[
            StagingEntry { flipper: FlipperId(0), mode, stage: FlipperStage::Lower },
            StagingEntry { flipper: FlipperId(1), mode, stage: FlipperStage::Upper },
        ]).unwrap();
        let mut g = InputIngest::new(IngestConfig::default());
        for (k, v) in steps.iter().enumerate() {
            g.push(InputEvent::axis(k as u64 * MS, CabinetAxis::LeftFlipper, *v)).unwrap();
        }
        let mut counts = [0u32; 2];
        let mut out = ArrayVec::new();
        for t in 0..steps.len() as u64 + 2 {
            out.clear();
            r.update(&g.advance(t), &mut out);
            for (id, on) in &out {
                prop_assert!(*on, "monotone ramp deactivated a flipper");
                counts[id.index()] += 1;
                // Actuates exactly at the first step reaching the threshold.
                let thr = if id.0 == 0 { lower } else { upper };
                let first = steps.iter().position(|&s| s >= thr).unwrap() as u64;
                prop_assert_eq!(t, first);
            }
        }
        let max = *steps.last().unwrap();
        prop_assert_eq!(counts[0], u32::from(max >= lower));
        prop_assert_eq!(counts[1], u32::from(max >= upper));
    }

    #[test]
    fn router_output_equals_state_diff(
        frames in prop::collection::vec((any::<u8>(), prop::array::uniform5(-0.2f32..1.2)), 1..60),
    ) {
        let es = entries();
        let mut r = FlipperStagingRouter::new(&es).unwrap();
        let mut prev = [false; 4];
        let mut out = ArrayVec::new();
        for (i, (buttons, axes)) in frames.into_iter().enumerate() {
            let f = CabinetFrame { tick: i as u64, buttons, axes };
            out.clear();
            r.update(&f, &mut out);
            let mut want = Vec::new();
            for (k, e) in es.iter().enumerate() {
                let now = entry_active(e, &f);
                if now != prev[k] {
                    want.push((e.flipper, now));
                    prev[k] = now;
                }
                prop_assert_eq!(r.is_active(e.flipper), now);
            }
            prop_assert_eq!(out.as_slice(), want.as_slice());
        }
    }
}
