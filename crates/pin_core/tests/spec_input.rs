//! Black-box spec tests for `pin_core::input` (Subphase 1.1 brief, PLAN.md §3.4).

use pin_core::input::{CabinetAxis, CabinetInput, HapticsTelemetryEvent, InputEvent};

/// Compile-time shape check: `InputEvent` carries SDL's nanosecond timestamp.
#[allow(dead_code)]
fn timestamp_of(event: &InputEvent) -> u64 {
    event.timestamp_ns
}

#[allow(dead_code)]
fn types_exist(_: &CabinetAxis, _: &HapticsTelemetryEvent) {}

fn assert_clone_debug<T: Clone + std::fmt::Debug>() {}

#[test]
fn cabinet_types_are_clone_and_debug() {
    // Required by the derives on FlipperStagingMode in §3.3.
    assert_clone_debug::<CabinetInput>();
    assert_clone_debug::<CabinetAxis>();
}

#[test]
fn the_eight_reserved_cabinet_inputs_exist_and_are_distinct() {
    let all = [
        CabinetInput::LeftFlipper,
        CabinetInput::RightFlipper,
        CabinetInput::LeftUpper,
        CabinetInput::RightUpper,
        CabinetInput::Action,
        CabinetInput::Start,
        CabinetInput::Launch,
        CabinetInput::Tilt,
    ];
    let names: Vec<String> = all.iter().map(|c| format!("{c:?}")).collect();
    for (i, a) in names.iter().enumerate() {
        for b in &names[i + 1..] {
            assert_ne!(a, b, "cabinet inputs must be distinguishable");
        }
    }
    assert_eq!(names.len(), 8);
}

#[test]
fn cabinet_input_has_exactly_the_eight_reserved_variants() {
    // An exhaustive match: fails to compile if a variant is added or missing.
    fn index(c: &CabinetInput) -> usize {
        match c {
            CabinetInput::LeftFlipper => 0,
            CabinetInput::RightFlipper => 1,
            CabinetInput::LeftUpper => 2,
            CabinetInput::RightUpper => 3,
            CabinetInput::Action => 4,
            CabinetInput::Start => 5,
            CabinetInput::Launch => 6,
            CabinetInput::Tilt => 7,
        }
    }
    assert_eq!(index(&CabinetInput::LeftFlipper), 0);
    assert_eq!(index(&CabinetInput::Tilt), 7);
}
