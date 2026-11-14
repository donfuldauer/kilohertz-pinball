//! Independent black-box spec tests for `pin_input::staging` (Subphase 3.1).
//!
//! Written against the fixed public API of the subphase brief, without sight
//! of the implementation.
//!
//! Semantics under test (orchestrator decision):
//! * `DedicatedButton(i)` / `ParallelLinked(i)`: active iff input `i` pressed.
//! * `DualStageThreshold`: stage `Lower` active iff axis >= lower_threshold,
//!   stage `Upper` active iff axis >= upper_threshold.
//! * `AutoSensorThingFlip`: never active through the router.

use arrayvec::ArrayVec;
use pin_core::flipper::FlipperStagingMode;
use pin_core::ids::{FlipperId, SwitchId};
use pin_core::input::{CabinetAxis, CabinetInput, InputEvent};
use pin_input::events::{CabinetFrame, IngestConfig, InputIngest};
use pin_input::staging::{
    DEFAULT_LOWER_THRESHOLD, DEFAULT_UPPER_THRESHOLD, FlipperStage, FlipperStagingRouter,
    StagingEntry,
};
use proptest::prelude::*;

type Out = ArrayVec<(FlipperId, bool), 16>;

const MS: u64 = 1_000_000;

fn axis_index(a: CabinetAxis) -> usize {
    CabinetAxis::ALL.iter().position(|x| *x == a).unwrap()
}

fn frame(tick: u64, pressed: &[CabinetInput], axes: &[(CabinetAxis, f32)]) -> CabinetFrame {
    let mut f = CabinetFrame {
        tick,
        buttons: 0,
        axes: [0.0; 5],
    };
    for i in pressed {
        f.buttons |= 1 << i.index();
    }
    for (a, v) in axes {
        f.axes[axis_index(*a)] = *v;
    }
    f
}

fn router(entries: &[StagingEntry]) -> FlipperStagingRouter {
    match FlipperStagingRouter::new(entries) {
        Ok(r) => r,
        Err(_) => panic!("router rejected valid entries {entries:?}"),
    }
}

fn step(r: &mut FlipperStagingRouter, f: &CabinetFrame) -> Vec<(FlipperId, bool)> {
    let mut out = Out::new();
    r.update(f, &mut out);
    out.to_vec()
}

fn entry(id: u16, mode: FlipperStagingMode, stage: FlipperStage) -> StagingEntry {
    StagingEntry {
        flipper: FlipperId(id),
        mode,
        stage,
    }
}

fn dedicated(id: u16, input: CabinetInput) -> StagingEntry {
    entry(
        id,
        FlipperStagingMode::DedicatedButton(input),
        FlipperStage::Lower,
    )
}

fn dual(id: u16, axis: CabinetAxis, lower: f32, upper: f32, stage: FlipperStage) -> StagingEntry {
    entry(
        id,
        FlipperStagingMode::DualStageThreshold {
            axis,
            lower_threshold: lower,
            upper_threshold: upper,
        },
        stage,
    )
}

fn auto(id: u16, stage: FlipperStage) -> StagingEntry {
    entry(
        id,
        FlipperStagingMode::AutoSensorThingFlip {
            trigger_switch: SwitchId(40),
            delay_ms: 10.0,
            hold_duration_ms: 80.0,
        },
        stage,
    )
}

/// The decision-log semantics, written independently of the implementation.
fn reference_active(e: &StagingEntry, f: &CabinetFrame) -> bool {
    match e.mode {
        FlipperStagingMode::DedicatedButton(i) | FlipperStagingMode::ParallelLinked(i) => {
            f.buttons & (1 << i.index()) != 0
        }
        FlipperStagingMode::DualStageThreshold {
            axis,
            lower_threshold,
            upper_threshold,
        } => {
            let v = f.axes[axis_index(axis)];
            match e.stage {
                FlipperStage::Lower => v >= lower_threshold,
                FlipperStage::Upper => v >= upper_threshold,
            }
        }
        FlipperStagingMode::AutoSensorThingFlip { .. } => false,
    }
}

// ------------------------------------------------------------ constants

#[test]
fn default_thresholds_are_30_and_85_percent() {
    assert_eq!(DEFAULT_LOWER_THRESHOLD, 0.30);
    assert_eq!(DEFAULT_UPPER_THRESHOLD, 0.85);
}

// --------------------------------------------------------- construction

#[test]
fn empty_router_is_valid_and_silent() {
    let mut r = router(&[]);
    assert!(step(&mut r, &frame(0, &CabinetInput::ALL, &[])).is_empty());
    assert!(!r.is_active(FlipperId(0)));
}

#[test]
fn sixteen_entries_are_accepted_seventeen_are_not() {
    let entries: Vec<StagingEntry> = (0..17)
        .map(|k| dedicated(k, CabinetInput::LeftFlipper))
        .collect();
    assert!(FlipperStagingRouter::new(&entries[..16]).is_ok());
    assert!(FlipperStagingRouter::new(&entries).is_err());
    let many: Vec<StagingEntry> = (0..200)
        .map(|k| dedicated(k, CabinetInput::LeftFlipper))
        .collect();
    assert!(FlipperStagingRouter::new(&many).is_err());
}

#[test]
fn sixteen_flippers_on_one_button_all_report_in_one_update() {
    let entries: Vec<StagingEntry> = (0..16)
        .map(|k| {
            entry(
                100 - k,
                FlipperStagingMode::ParallelLinked(CabinetInput::Action),
                FlipperStage::Lower,
            )
        })
        .collect();
    let mut r = router(&entries);
    let got = step(&mut r, &frame(0, &[CabinetInput::Action], &[]));
    let want: Vec<_> = (0..16).map(|k| (FlipperId(100 - k), true)).collect();
    assert_eq!(got, want);
    let got = step(&mut r, &frame(1, &[], &[]));
    let want: Vec<_> = (0..16).map(|k| (FlipperId(100 - k), false)).collect();
    assert_eq!(got, want);
}

#[test]
fn duplicate_flipper_ids_are_rejected() {
    let a = dedicated(3, CabinetInput::LeftFlipper);
    let b = dedicated(3, CabinetInput::RightFlipper);
    assert!(FlipperStagingRouter::new(&[a, b]).is_err());
    assert!(FlipperStagingRouter::new(&[a, a]).is_err());
    // Duplicate across different modes / stages, not adjacent.
    let c = dual(3, CabinetAxis::LeftFlipper, 0.3, 0.85, FlipperStage::Upper);
    let other = dedicated(4, CabinetInput::LeftFlipper);
    assert!(FlipperStagingRouter::new(&[a, other, c]).is_err());
    assert!(FlipperStagingRouter::new(&[a, other, auto(3, FlipperStage::Lower)]).is_err());
    // Distinct ids with identical bindings are fine.
    assert!(FlipperStagingRouter::new(&[a, dedicated(5, CabinetInput::LeftFlipper)]).is_ok());
}

#[test]
fn invalid_thresholds_are_rejected() {
    let bad: &[(f32, f32)] = &[
        (f32::NAN, 0.85),
        (0.3, f32::NAN),
        (f32::NAN, f32::NAN),
        (f32::INFINITY, f32::INFINITY),
        (0.3, f32::INFINITY),
        (f32::NEG_INFINITY, 0.85),
        (-0.01, 0.85),
        (-1.0, -0.5),
        (0.3, 1.01),
        (1.5, 2.0),
        (0.85, 0.30),
        (0.5, 0.499_999),
        (1.0, 0.0),
    ];
    for &(lo, hi) in bad {
        for stage in [FlipperStage::Lower, FlipperStage::Upper] {
            for axis in CabinetAxis::ALL {
                let e = dual(1, axis, lo, hi, stage);
                assert!(
                    FlipperStagingRouter::new(&[e]).is_err(),
                    "accepted thresholds ({lo}, {hi}) {stage:?}"
                );
                // Also when hidden behind valid entries.
                let ok = dedicated(0, CabinetInput::LeftFlipper);
                assert!(FlipperStagingRouter::new(&[ok, e]).is_err());
            }
        }
    }
}

#[test]
fn boundary_thresholds_are_accepted() {
    let good: &[(f32, f32)] = &[
        (0.0, 0.0),
        (0.0, 1.0),
        (1.0, 1.0),
        (0.5, 0.5),
        (DEFAULT_LOWER_THRESHOLD, DEFAULT_UPPER_THRESHOLD),
        (-0.0, 1.0),
    ];
    for &(lo, hi) in good {
        for stage in [FlipperStage::Lower, FlipperStage::Upper] {
            let e = dual(1, CabinetAxis::RightFlipper, lo, hi, stage);
            assert!(
                FlipperStagingRouter::new(&[e]).is_ok(),
                "rejected thresholds ({lo}, {hi}) {stage:?}"
            );
        }
    }
}

// ------------------------------------------------------ button routing

#[test]
fn nothing_is_active_before_the_first_update() {
    let r = router(&[
        dedicated(0, CabinetInput::LeftFlipper),
        dual(1, CabinetAxis::LeftFlipper, 0.0, 0.0, FlipperStage::Lower),
        auto(2, FlipperStage::Lower),
    ]);
    for id in 0..4 {
        assert!(!r.is_active(FlipperId(id)));
    }
}

#[test]
fn dedicated_button_reports_edges_only() {
    for input in CabinetInput::ALL {
        let mut r = router(&[dedicated(7, input)]);
        let id = FlipperId(7);
        assert!(step(&mut r, &frame(0, &[], &[])).is_empty());
        assert!(!r.is_active(id));
        assert_eq!(step(&mut r, &frame(1, &[input], &[])), [(id, true)]);
        assert!(r.is_active(id));
        for t in 2..10 {
            assert!(step(&mut r, &frame(t, &[input], &[])).is_empty(), "{t}");
            assert!(r.is_active(id));
        }
        assert_eq!(step(&mut r, &frame(10, &[], &[])), [(id, false)]);
        assert!(!r.is_active(id));
        assert!(step(&mut r, &frame(11, &[], &[])).is_empty());
    }
}

#[test]
fn dedicated_button_ignores_other_inputs_and_axes() {
    let mut r = router(&[dedicated(1, CabinetInput::LeftFlipper)]);
    let others: Vec<CabinetInput> = CabinetInput::ALL
        .into_iter()
        .filter(|i| *i != CabinetInput::LeftFlipper)
        .collect();
    let axes: Vec<(CabinetAxis, f32)> = CabinetAxis::ALL.into_iter().map(|a| (a, 1.0)).collect();
    assert!(step(&mut r, &frame(0, &others, &axes)).is_empty());
    assert!(!r.is_active(FlipperId(1)));
}

#[test]
fn button_pressed_on_first_frame_is_reported() {
    let mut r = router(&[dedicated(1, CabinetInput::RightFlipper)]);
    assert_eq!(
        step(&mut r, &frame(0, &[CabinetInput::RightFlipper], &[])),
        [(FlipperId(1), true)]
    );
}

#[test]
fn stage_is_irrelevant_for_button_modes() {
    for stage in [FlipperStage::Lower, FlipperStage::Upper] {
        for mode in [
            FlipperStagingMode::DedicatedButton(CabinetInput::LeftUpper),
            FlipperStagingMode::ParallelLinked(CabinetInput::LeftUpper),
        ] {
            let mut r = router(&[entry(2, mode, stage)]);
            // A fully pulled analog axis must not fire a button-mode flipper.
            assert!(step(&mut r, &frame(0, &[], &[(CabinetAxis::LeftFlipper, 1.0)])).is_empty());
            assert_eq!(
                step(&mut r, &frame(1, &[CabinetInput::LeftUpper], &[])),
                [(FlipperId(2), true)]
            );
        }
    }
}

#[test]
fn parallel_linked_flippers_fire_together_in_entry_order() {
    let l = CabinetInput::LeftFlipper;
    let rr = CabinetInput::RightFlipper;
    let mut r = router(&[
        entry(
            9,
            FlipperStagingMode::ParallelLinked(l),
            FlipperStage::Upper,
        ),
        dedicated(4, rr),
        dedicated(2, l),
        entry(
            7,
            FlipperStagingMode::ParallelLinked(rr),
            FlipperStage::Upper,
        ),
        entry(
            1,
            FlipperStagingMode::ParallelLinked(l),
            FlipperStage::Lower,
        ),
    ]);
    assert_eq!(
        step(&mut r, &frame(0, &[l], &[])),
        [
            (FlipperId(9), true),
            (FlipperId(2), true),
            (FlipperId(1), true)
        ]
    );
    assert_eq!(
        step(&mut r, &frame(1, &[l, rr], &[])),
        [(FlipperId(4), true), (FlipperId(7), true)]
    );
    // Simultaneous opposite changes, still in entry order.
    assert_eq!(
        step(&mut r, &frame(2, &[], &[])),
        [
            (FlipperId(9), false),
            (FlipperId(4), false),
            (FlipperId(2), false),
            (FlipperId(7), false),
            (FlipperId(1), false),
        ]
    );
    assert_eq!(
        step(&mut r, &frame(3, &[rr], &[])),
        [(FlipperId(4), true), (FlipperId(7), true)]
    );
    assert_eq!(
        step(&mut r, &frame(4, &[l], &[])),
        [
            (FlipperId(9), true),
            (FlipperId(4), false),
            (FlipperId(2), true),
            (FlipperId(7), false),
            (FlipperId(1), true),
        ]
    );
    for (id, want) in [(9, true), (4, false), (2, true), (7, false), (1, true)] {
        assert_eq!(r.is_active(FlipperId(id)), want);
    }
}

#[test]
fn unknown_flipper_is_never_active() {
    let mut r = router(&[dedicated(1, CabinetInput::LeftFlipper)]);
    step(&mut r, &frame(0, &CabinetInput::ALL, &[]));
    assert!(r.is_active(FlipperId(1)));
    assert!(!r.is_active(FlipperId(0)));
    assert!(!r.is_active(FlipperId(2)));
    assert!(!r.is_active(FlipperId(u16::MAX)));
}

// --------------------------------------------------- dual-stage routing

#[test]
fn dual_stage_thresholds_are_inclusive() {
    let lo = DEFAULT_LOWER_THRESHOLD;
    let hi = DEFAULT_UPPER_THRESHOLD;
    let ax = CabinetAxis::LeftFlipper;
    let below = |x: f32| f32::from_bits(x.to_bits() - 1);
    let mut r = router(&[
        dual(0, ax, lo, hi, FlipperStage::Lower),
        dual(1, ax, lo, hi, FlipperStage::Upper),
    ]);
    assert!(step(&mut r, &frame(0, &[], &[(ax, 0.0)])).is_empty());
    assert!(step(&mut r, &frame(1, &[], &[(ax, below(lo))])).is_empty());
    assert_eq!(
        step(&mut r, &frame(2, &[], &[(ax, lo)])),
        [(FlipperId(0), true)]
    );
    assert!(step(&mut r, &frame(3, &[], &[(ax, below(hi))])).is_empty());
    assert!(r.is_active(FlipperId(0)) && !r.is_active(FlipperId(1)));
    assert_eq!(
        step(&mut r, &frame(4, &[], &[(ax, hi)])),
        [(FlipperId(1), true)]
    );
    assert!(step(&mut r, &frame(5, &[], &[(ax, 1.0)])).is_empty());
    assert!(r.is_active(FlipperId(0)) && r.is_active(FlipperId(1)));
    // Release: upper drops first, then lower.
    assert_eq!(
        step(&mut r, &frame(6, &[], &[(ax, below(hi))])),
        [(FlipperId(1), false)]
    );
    assert!(step(&mut r, &frame(7, &[], &[(ax, lo)])).is_empty());
    assert_eq!(
        step(&mut r, &frame(8, &[], &[(ax, below(lo))])),
        [(FlipperId(0), false)]
    );
    assert!(!r.is_active(FlipperId(0)) && !r.is_active(FlipperId(1)));
}

#[test]
fn dual_stage_full_press_in_one_frame_fires_both_stages_in_entry_order() {
    let ax = CabinetAxis::RightFlipper;
    // Upper listed first on purpose.
    let mut r = router(&[
        dual(11, ax, 0.3, 0.85, FlipperStage::Upper),
        dual(10, ax, 0.3, 0.85, FlipperStage::Lower),
    ]);
    assert_eq!(
        step(&mut r, &frame(0, &[], &[(ax, 1.0)])),
        [(FlipperId(11), true), (FlipperId(10), true)]
    );
    assert_eq!(
        step(&mut r, &frame(1, &[], &[(ax, 0.0)])),
        [(FlipperId(11), false), (FlipperId(10), false)]
    );
}

#[test]
fn dual_stage_ignores_buttons_and_other_axes() {
    let mut r = router(&[
        dual(0, CabinetAxis::LeftFlipper, 0.3, 0.85, FlipperStage::Lower),
        dual(1, CabinetAxis::LeftFlipper, 0.3, 0.85, FlipperStage::Upper),
    ]);
    let f = frame(
        0,
        &CabinetInput::ALL,
        &[
            (CabinetAxis::RightFlipper, 1.0),
            (CabinetAxis::Plunger, 1.0),
            (CabinetAxis::NudgeX, 1.0),
            (CabinetAxis::NudgeY, 1.0),
        ],
    );
    assert!(step(&mut r, &f).is_empty());
}

#[test]
fn dual_stage_uses_each_entrys_own_axis_and_thresholds() {
    let mut r = router(&[
        dual(0, CabinetAxis::LeftFlipper, 0.2, 0.6, FlipperStage::Lower),
        dual(1, CabinetAxis::LeftFlipper, 0.4, 0.9, FlipperStage::Lower),
        dual(2, CabinetAxis::RightFlipper, 0.1, 0.5, FlipperStage::Upper),
        dual(3, CabinetAxis::Plunger, 0.7, 0.7, FlipperStage::Upper),
        dual(4, CabinetAxis::Plunger, 0.7, 0.7, FlipperStage::Lower),
    ]);
    let f = frame(
        0,
        &[],
        &[
            (CabinetAxis::LeftFlipper, 0.3),
            (CabinetAxis::RightFlipper, 0.3),
            (CabinetAxis::Plunger, 0.7),
        ],
    );
    assert_eq!(
        step(&mut r, &f),
        [
            (FlipperId(0), true),
            (FlipperId(3), true),
            (FlipperId(4), true)
        ]
    );
    let f = frame(
        1,
        &[],
        &[
            (CabinetAxis::LeftFlipper, 0.4),
            (CabinetAxis::RightFlipper, 0.5),
            (CabinetAxis::Plunger, 0.69),
        ],
    );
    assert_eq!(
        step(&mut r, &f),
        [
            (FlipperId(1), true),
            (FlipperId(2), true),
            (FlipperId(3), false),
            (FlipperId(4), false)
        ]
    );
}

#[test]
fn zero_threshold_is_active_at_rest() {
    let mut r = router(&[
        dual(0, CabinetAxis::LeftFlipper, 0.0, 1.0, FlipperStage::Lower),
        dual(1, CabinetAxis::LeftFlipper, 0.0, 1.0, FlipperStage::Upper),
    ]);
    assert_eq!(step(&mut r, &frame(0, &[], &[])), [(FlipperId(0), true)]);
    assert_eq!(
        step(&mut r, &frame(1, &[], &[(CabinetAxis::LeftFlipper, 1.0)])),
        [(FlipperId(1), true)]
    );
}

#[test]
fn equal_thresholds_fire_both_stages_together() {
    let ax = CabinetAxis::LeftFlipper;
    let mut r = router(&[
        dual(0, ax, 0.5, 0.5, FlipperStage::Lower),
        dual(1, ax, 0.5, 0.5, FlipperStage::Upper),
    ]);
    assert!(step(&mut r, &frame(0, &[], &[(ax, 0.49)])).is_empty());
    assert_eq!(
        step(&mut r, &frame(1, &[], &[(ax, 0.5)])),
        [(FlipperId(0), true), (FlipperId(1), true)]
    );
}

// ------------------------------------------------------ auto thing flip

#[test]
fn auto_sensor_thing_flip_is_never_driven_by_the_router() {
    let mut r = router(&[
        auto(0, FlipperStage::Lower),
        dedicated(1, CabinetInput::LeftUpper),
        auto(2, FlipperStage::Upper),
    ]);
    let all_axes: Vec<(CabinetAxis, f32)> =
        CabinetAxis::ALL.into_iter().map(|a| (a, 1.0)).collect();
    assert_eq!(
        step(&mut r, &frame(0, &CabinetInput::ALL, &all_axes)),
        [(FlipperId(1), true)]
    );
    assert!(!r.is_active(FlipperId(0)));
    assert!(!r.is_active(FlipperId(2)));
    assert_eq!(step(&mut r, &frame(1, &[], &[])), [(FlipperId(1), false)]);
    assert!(!r.is_active(FlipperId(0)));
    assert!(!r.is_active(FlipperId(2)));
}

// ------------------------------------- end to end: exact-tick actuation

/// Runs `ingest` + `router` for ticks `0..=last` and returns every
/// `(tick, flipper, active)` actuation.
fn run(ing: &mut InputIngest, r: &mut FlipperStagingRouter, last: u64) -> Vec<(u64, u16, bool)> {
    let mut log = Vec::new();
    let mut out = Out::new();
    for t in 0..=last {
        let f = ing.advance(t);
        assert_eq!(f.tick, t);
        out.clear();
        r.update(&f, &mut out);
        for (id, on) in &out {
            log.push((t, id.0, *on));
        }
    }
    log
}

fn ingest(debounce_ms: u16) -> InputIngest {
    InputIngest::new(IngestConfig {
        epoch_ns: 0,
        debounce_ms,
    })
}

#[test]
fn exact_tick_dedicated_buttons() {
    let mut ing = ingest(0);
    let mut r = router(&[
        dedicated(0, CabinetInput::LeftFlipper),
        dedicated(1, CabinetInput::RightFlipper),
    ]);
    let l = CabinetInput::LeftFlipper;
    let rr = CabinetInput::RightFlipper;
    for ev in [
        InputEvent::button(3 * MS + 400_000, l, true), // tick 4
        InputEvent::button(7 * MS, rr, true),          // tick 7 (on the boundary)
        InputEvent::button(12 * MS + 1, l, false),     // tick 13
        InputEvent::button(19 * MS + 999_999, rr, false), // tick 20
        InputEvent::button(25 * MS + 100, l, true),    // tap inside tick 26:
        InputEvent::button(25 * MS + 900, l, false),   // no actuation at all
    ] {
        assert!(ing.push(ev).is_ok());
    }
    assert_eq!(
        run(&mut ing, &mut r, 40),
        [(4, 0, true), (7, 1, true), (13, 0, false), (20, 1, false)]
    );
}

#[test]
fn exact_tick_parallel_linked() {
    let l = CabinetInput::LeftFlipper;
    let mut ing = ingest(0);
    let mut r = router(&[
        dedicated(0, l),
        entry(
            5,
            FlipperStagingMode::ParallelLinked(l),
            FlipperStage::Upper,
        ),
        entry(
            6,
            FlipperStagingMode::ParallelLinked(CabinetInput::RightFlipper),
            FlipperStage::Upper,
        ),
    ]);
    // Delivered out of timestamp order.
    for ev in [
        InputEvent::button(30 * MS + 5, l, false),
        InputEvent::button(9 * MS + 500_000, l, true),
        InputEvent::button(15 * MS, CabinetInput::RightFlipper, true),
    ] {
        assert!(ing.push(ev).is_ok());
    }
    assert_eq!(
        run(&mut ing, &mut r, 40),
        [
            (10, 0, true),
            (10, 5, true),
            (15, 6, true),
            (31, 0, false),
            (31, 5, false)
        ]
    );
}

#[test]
fn exact_tick_dual_stage_ramp_up_and_down() {
    let ax = CabinetAxis::LeftFlipper;
    let lo = DEFAULT_LOWER_THRESHOLD;
    let hi = DEFAULT_UPPER_THRESHOLD;
    let mut ing = ingest(0);
    let mut r = router(&[
        dual(0, ax, lo, hi, FlipperStage::Lower),
        dual(1, ax, lo, hi, FlipperStage::Upper),
    ]);
    // Up: value k/100 on tick k (k = 0..=100); down: one step per tick after.
    let value = |k: u64| k as f32 / 100.0;
    for k in 0..=100u64 {
        assert!(ing.push(InputEvent::axis(k * MS, ax, value(k))).is_ok());
    }
    for k in 1..=100u64 {
        assert!(
            ing.push(InputEvent::axis((100 + k) * MS, ax, value(100 - k)))
                .is_ok()
        );
    }
    // Expected ticks computed from the very same f32 samples.
    let up_lo = (0..=100).find(|&k| value(k) >= lo).unwrap();
    let up_hi = (0..=100).find(|&k| value(k) >= hi).unwrap();
    let down_hi = 100 + (1..=100).find(|&k| value(100 - k) < hi).unwrap();
    let down_lo = 100 + (1..=100).find(|&k| value(100 - k) < lo).unwrap();
    assert!((29..=31).contains(&up_lo) && (84..=86).contains(&up_hi));
    assert_eq!(
        run(&mut ing, &mut r, 220),
        [
            (up_lo, 0, true),
            (up_hi, 1, true),
            (down_hi, 1, false),
            (down_lo, 0, false)
        ]
    );
}

#[test]
fn exact_tick_dual_stage_sub_tick_ramp_uses_last_sample_of_each_tick() {
    // Four samples per millisecond; the frame for tick t holds the sample
    // stamped exactly t ms, i.e. sample index 4t.
    let ax = CabinetAxis::RightFlipper;
    let mut ing = ingest(0);
    let mut r = router(&[
        dual(3, ax, 0.25, 0.75, FlipperStage::Upper),
        dual(2, ax, 0.25, 0.75, FlipperStage::Lower),
    ]);
    let value = |i: u64| i as f32 / 200.0;
    for i in 0..=200u64 {
        assert!(
            ing.push(InputEvent::axis(i * 250_000, ax, value(i)))
                .is_ok()
        );
    }
    let lower_tick = (0..=50).find(|&t| value(4 * t) >= 0.25).unwrap();
    let upper_tick = (0..=50).find(|&t| value(4 * t) >= 0.75).unwrap();
    assert_eq!((lower_tick, upper_tick), (13, 38));
    assert_eq!(
        run(&mut ing, &mut r, 60),
        [(lower_tick, 2, true), (upper_tick, 3, true)]
    );
}

#[test]
fn exact_tick_with_debounce_delays_release_to_window_end() {
    let l = CabinetInput::LeftFlipper;
    let mut ing = ingest(10);
    let mut r = router(&[dedicated(0, l)]);
    for ev in [
        InputEvent::button(4 * MS + 500_000, l, true), // accepted, tick 5
        InputEvent::button(6 * MS + 500_000, l, false), // bounce
        InputEvent::button(7 * MS + 500_000, l, true),
        InputEvent::button(8 * MS + 500_000, l, false), // final state: released
    ] {
        assert!(ing.push(ev).is_ok());
    }
    assert_eq!(run(&mut ing, &mut r, 60), [(5, 0, true), (15, 0, false)]);
}

#[test]
fn clamped_and_nan_axis_input_reaches_the_router_sanely() {
    let ax = CabinetAxis::LeftFlipper;
    let mut ing = ingest(0);
    let mut r = router(&[
        dual(0, ax, 0.3, 0.85, FlipperStage::Lower),
        dual(1, ax, 0.3, 0.85, FlipperStage::Upper),
    ]);
    for ev in [
        InputEvent::axis(2 * MS, ax, 7.5), // clamps to 1.0: both fire, tick 2
        InputEvent::axis(4 * MS, ax, f32::NAN), // ignored: nothing changes
        InputEvent::axis(6 * MS, ax, -3.0), // clamps to 0.0: both drop, tick 6
    ] {
        assert!(ing.push(ev).is_ok());
    }
    assert_eq!(
        run(&mut ing, &mut r, 10),
        [(2, 0, true), (2, 1, true), (6, 0, false), (6, 1, false)]
    );
}

// --------------------------------------------------------------- proptest

fn arb_stage() -> impl Strategy<Value = FlipperStage> {
    prop_oneof![Just(FlipperStage::Lower), Just(FlipperStage::Upper)]
}

fn arb_threshold() -> impl Strategy<Value = f32> {
    prop_oneof![
        6 => 0.0f32..=1.0f32,
        1 => Just(0.0f32),
        1 => Just(1.0f32),
        1 => Just(DEFAULT_LOWER_THRESHOLD),
        1 => Just(DEFAULT_UPPER_THRESHOLD),
    ]
}

fn arb_dual() -> impl Strategy<Value = FlipperStagingMode> {
    (0usize..5, arb_threshold(), arb_threshold()).prop_map(|(a, x, y)| {
        FlipperStagingMode::DualStageThreshold {
            axis: CabinetAxis::ALL[a],
            lower_threshold: x.min(y),
            upper_threshold: x.max(y),
        }
    })
}

fn arb_mode() -> impl Strategy<Value = FlipperStagingMode> {
    prop_oneof![
        3 => (0usize..8).prop_map(|i| FlipperStagingMode::DedicatedButton(CabinetInput::ALL[i])),
        3 => (0usize..8).prop_map(|i| FlipperStagingMode::ParallelLinked(CabinetInput::ALL[i])),
        4 => arb_dual(),
        1 => (any::<u16>(), 0.0f32..100.0, 0.0f32..500.0).prop_map(|(s, d, h)| {
            FlipperStagingMode::AutoSensorThingFlip {
                trigger_switch: SwitchId(s),
                delay_ms: d,
                hold_duration_ms: h,
            }
        }),
    ]
}

/// 0..=16 entries with unique flipper ids in arbitrary (non-sorted) order.
fn arb_entries(
    mode: impl Strategy<Value = FlipperStagingMode>,
) -> impl Strategy<Value = Vec<StagingEntry>> {
    prop::collection::vec((0u16..40, mode, arb_stage()), 0..=16).prop_map(|v| {
        let mut seen = std::collections::BTreeSet::new();
        v.into_iter()
            .filter(|(id, _, _)| seen.insert(*id))
            .map(|(id, mode, stage)| entry(id, mode, stage))
            .collect()
    })
}

fn arb_frame_axis() -> impl Strategy<Value = f32> {
    prop_oneof![
        6 => -1.0f32..=1.0f32,
        1 => Just(0.0f32),
        1 => Just(1.0f32),
        1 => Just(DEFAULT_LOWER_THRESHOLD),
        1 => Just(DEFAULT_UPPER_THRESHOLD),
    ]
}

fn arb_frames() -> impl Strategy<Value = Vec<(u8, [f32; 5])>> {
    prop::collection::vec(
        (any::<u8>(), prop::array::uniform5(arb_frame_axis())),
        1..60,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// `update` emits exactly the diffs of the recomputed state, in entry
    /// order, and `is_active` always equals the recomputed state.
    #[test]
    fn prop_update_equals_reference_diffs(
        entries in arb_entries(arb_mode()),
        frames in arb_frames(),
    ) {
        let mut r = router(&entries);
        let mut prev = vec![false; entries.len()];
        for e in &entries {
            prop_assert!(!r.is_active(e.flipper));
        }
        for (t, (buttons, axes)) in frames.into_iter().enumerate() {
            let f = CabinetFrame { tick: t as u64, buttons, axes };
            let mut want = Vec::new();
            for (k, e) in entries.iter().enumerate() {
                let now = reference_active(e, &f);
                if now != prev[k] {
                    want.push((e.flipper, now));
                    prev[k] = now;
                }
            }
            let got = step(&mut r, &f);
            prop_assert_eq!(&got, &want, "tick {}", t);
            for (k, e) in entries.iter().enumerate() {
                prop_assert_eq!(r.is_active(e.flipper), prev[k]);
            }
            // Ids not in the table are never active.
            for id in 40u16..44 {
                prop_assert!(!r.is_active(FlipperId(id)));
            }
            // Feeding the same frame again yields no further changes.
            prop_assert!(step(&mut r, &f).is_empty());
        }
    }

    /// The router is a pure function of (previous frame, current frame): two
    /// routers fed different histories agree once fed the same two frames.
    #[test]
    fn prop_output_depends_only_on_previous_and_current_state(
        entries in arb_entries(arb_mode()),
        history in arb_frames(),
        a in (any::<u8>(), prop::array::uniform5(arb_frame_axis())),
        b in (any::<u8>(), prop::array::uniform5(arb_frame_axis())),
    ) {
        let mut r1 = router(&entries);
        let mut r2 = router(&entries);
        for (t, (buttons, axes)) in history.into_iter().enumerate() {
            step(&mut r1, &CabinetFrame { tick: t as u64, buttons, axes });
        }
        let fa = CabinetFrame { tick: 1000, buttons: a.0, axes: a.1 };
        let fb = CabinetFrame { tick: 1001, buttons: b.0, axes: b.1 };
        step(&mut r1, &fa);
        step(&mut r2, &fa);
        prop_assert_eq!(step(&mut r1, &fb), step(&mut r2, &fb));
    }

    /// A monotone (non-decreasing) axis ramp crosses each threshold exactly
    /// once: one `true` per flipper whose threshold is reached, on the first
    /// sample at or above it, and never a `false`.
    #[test]
    fn prop_monotone_ramp_crosses_each_threshold_exactly_once(
        entries in arb_entries(arb_dual()),
        mut samples in prop::collection::vec(prop::array::uniform5(0.0f32..=1.0f32), 1..80),
        rising in any::<bool>(),
    ) {
        // Make every axis monotone.
        for a in 0..5 {
            let mut col: Vec<f32> = samples.iter().map(|s| s[a]).collect();
            col.sort_by(|x, y| x.partial_cmp(y).unwrap());
            if !rising {
                col.reverse();
            }
            for (s, v) in samples.iter_mut().zip(col) {
                s[a] = v;
            }
        }
        let mut r = router(&entries);
        let mut log: Vec<(usize, FlipperId, bool)> = Vec::new();
        for (t, axes) in samples.iter().enumerate() {
            let f = CabinetFrame { tick: t as u64, buttons: 0, axes: *axes };
            for (id, on) in step(&mut r, &f) {
                log.push((t, id, on));
            }
        }
        for e in &entries {
            let mine: Vec<(usize, bool)> = log
                .iter()
                .filter(|(_, id, _)| *id == e.flipper)
                .map(|(t, _, on)| (*t, *on))
                .collect();
            let states: Vec<bool> = samples
                .iter()
                .enumerate()
                .map(|(t, axes)| reference_active(e, &CabinetFrame { tick: t as u64, buttons: 0, axes: *axes }))
                .collect();
            if rising {
                match states.iter().position(|s| *s) {
                    Some(t) => prop_assert_eq!(mine, vec![(t, true)]),
                    None => prop_assert!(mine.is_empty()),
                }
            } else {
                // Falling ramp: at most one activation (on the first sample)
                // followed by at most one deactivation.
                let mut want = Vec::new();
                if states[0] {
                    want.push((0, true));
                    if let Some(t) = states.iter().position(|s| !*s) {
                        want.push((t, false));
                    }
                }
                prop_assert_eq!(mine, want);
            }
        }
    }

    /// Ingest + router end to end: a monotone analog ramp delivered as
    /// timestamped events (in shuffled order) actuates each stage on exactly
    /// the tick of the first sample at or above its threshold.
    #[test]
    fn prop_ingested_ramp_actuates_on_exact_tick(
        lower in 0.01f32..=1.0f32,
        upper in 0.01f32..=1.0f32,
        mut samples in prop::collection::vec(0.0f32..=1.0f32, 1..120),
        jitter in prop::collection::vec(0u64..MS, 120),
        left in any::<bool>(),
    ) {
        let (lower, upper) = (lower.min(upper), lower.max(upper));
        let ax = if left { CabinetAxis::LeftFlipper } else { CabinetAxis::RightFlipper };
        samples.sort_by(|x, y| x.partial_cmp(y).unwrap());
        let mut ing = ingest(0);
        let mut r = router(&[
            dual(0, ax, lower, upper, FlipperStage::Lower),
            dual(1, ax, lower, upper, FlipperStage::Upper),
        ]);
        // Sample k lands somewhere inside tick k + 1; push newest first.
        for (k, v) in samples.iter().enumerate().rev() {
            let ts = (k as u64 + 1) * MS - jitter[k];
            prop_assert!(ing.push(InputEvent::axis(ts, ax, *v)).is_ok());
        }
        let mut want = Vec::new();
        if let Some(k) = samples.iter().position(|v| *v >= lower) {
            want.push((k as u64 + 1, 0u16, true));
        }
        if let Some(k) = samples.iter().position(|v| *v >= upper) {
            want.push((k as u64 + 1, 1u16, true));
        }
        prop_assert_eq!(run(&mut ing, &mut r, samples.len() as u64 + 3), want);
    }

    /// Construction accepts exactly: <= 16 entries, unique ids, and finite
    /// dual-stage thresholds with 0 <= lower <= upper <= 1.
    #[test]
    fn prop_new_validates_inputs(
        raw in prop::collection::vec(
            (0u16..24, 0usize..3, 0usize..8, 0usize..5,
             prop_oneof![4 => -0.2f32..1.2f32, 1 => Just(f32::NAN), 1 => Just(f32::INFINITY), 6 => 0.0f32..=0.5f32],
             prop_oneof![4 => -0.2f32..1.2f32, 1 => Just(f32::NAN), 1 => Just(f32::NEG_INFINITY), 6 => 0.5f32..=1.0f32],
             arb_stage()),
            0..20),
    ) {
        let entries: Vec<StagingEntry> = raw.iter().map(|&(id, kind, i, a, lo, hi, stage)| {
            let mode = match kind {
                0 => FlipperStagingMode::DedicatedButton(CabinetInput::ALL[i]),
                1 => FlipperStagingMode::ParallelLinked(CabinetInput::ALL[i]),
                _ => FlipperStagingMode::DualStageThreshold {
                    axis: CabinetAxis::ALL[a],
                    lower_threshold: lo,
                    upper_threshold: hi,
                },
            };
            entry(id, mode, stage)
        }).collect();
        let mut ids = std::collections::BTreeSet::new();
        let unique = entries.iter().all(|e| ids.insert(e.flipper));
        let thresholds_ok = entries.iter().all(|e| match e.mode {
            FlipperStagingMode::DualStageThreshold { lower_threshold: lo, upper_threshold: hi, .. } => {
                lo.is_finite() && hi.is_finite() && 0.0 <= lo && lo <= hi && hi <= 1.0
            }
            _ => true,
        });
        let valid = entries.len() <= 16 && unique && thresholds_ok;
        prop_assert_eq!(FlipperStagingRouter::new(&entries).is_ok(), valid,
            "len {} unique {} thresholds_ok {}", entries.len(), unique, thresholds_ok);
    }
}
