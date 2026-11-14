//! Independent spec test for Subphase 3.2, verification criterion 5: the
//! per-event paths of the profile mapper and of every calibration filter never
//! allocate. A counting global allocator (per thread) wraps the hot loops.

use pin_input::calibration::{
    DigitalNudge, NudgeConfig, NudgeFilter, PlungerConfig, PlungerFilter, thumbstick_nudge,
};
use pin_input::profiles::{
    GamepadAxis, GamepadButton, NudgeDirection, ProfileMapper, RawInput, builtin_profiles,
    parse_profile,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn bump() {
    let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
}

// SAFETY: forwards to the system allocator, only adding a thread-local counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

const EXAMPLE: &str = include_str!("spec_fixtures/profiles/example_pad.toml");

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let v = black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(v);
}

fn drive_mapper(m: &ProfileMapper) -> u64 {
    let mut seen = 0u64;
    for round in 0..20u64 {
        for sc in 0..600u32 {
            if black_box(m.key(round, sc, round % 2 == 0)).is_some() {
                seen += 1;
            }
        }
        seen += u64::from(black_box(m.key(round, u32::MAX, true)).is_some());
        for (i, b) in [
            GamepadButton::South,
            GamepadButton::East,
            GamepadButton::West,
            GamepadButton::North,
            GamepadButton::Back,
            GamepadButton::Guide,
            GamepadButton::Start,
            GamepadButton::LeftStick,
            GamepadButton::RightStick,
            GamepadButton::LeftShoulder,
            GamepadButton::RightShoulder,
            GamepadButton::DpadUp,
            GamepadButton::DpadDown,
            GamepadButton::DpadLeft,
            GamepadButton::DpadRight,
            GamepadButton::Misc1,
            GamepadButton::RightPaddle1,
            GamepadButton::LeftPaddle1,
            GamepadButton::RightPaddle2,
            GamepadButton::LeftPaddle2,
            GamepadButton::Touchpad,
        ]
        .into_iter()
        .enumerate()
        {
            seen += u64::from(black_box(m.button(round, b, i % 2 == 0)).is_some());
        }
        for raw in [i16::MIN, -20_000, -1, 0, 1, 12_345, i16::MAX] {
            for a in [
                GamepadAxis::LeftX,
                GamepadAxis::LeftY,
                GamepadAxis::RightX,
                GamepadAxis::RightY,
                GamepadAxis::LeftTrigger,
                GamepadAxis::RightTrigger,
            ] {
                seen += u64::from(black_box(m.axis(round, a, raw)).is_some());
            }
        }
        for sc in [0u32, 44, 224, 228, 500] {
            seen += u64::from(black_box(m.nudge_button(RawInput::Key(sc))).is_some());
        }
        seen += u64::from(
            m.nudge_button(RawInput::Button(GamepadButton::DpadRight))
                .is_some(),
        );
        seen += u64::from(
            m.nudge_button(RawInput::Axis(GamepadAxis::RightX))
                .is_some(),
        );
    }
    seen
}

#[test]
fn mapper_event_paths_do_not_allocate() {
    // Construction (and the lazy built-in parse) may allocate; do it first.
    let custom = match parse_profile(EXAMPLE) {
        Ok(p) => p,
        Err(e) => panic!("fixture profile must parse: {e}"),
    };
    let mut mappers: Vec<ProfileMapper> =
        builtin_profiles().iter().map(ProfileMapper::new).collect();
    mappers.push(ProfileMapper::new(&custom));

    let before = allocs();
    let mut seen = 0u64;
    for m in &mappers {
        seen += drive_mapper(m);
    }
    let after = allocs();
    assert!(seen > 0, "the loop exercised real mappings");
    assert_eq!(
        after - before,
        0,
        "ProfileMapper allocated on a per-event path"
    );
}

#[test]
fn plunger_filter_update_does_not_allocate() {
    let mut f = PlungerFilter::new(PlungerConfig {
        deadzone: 0.02,
        rest_tracking_per_s: 0.5,
        release_min_speed: 2.0,
        release_window_ms: 100,
    });
    let mut d = PlungerFilter::new(PlungerConfig::default());
    let before = allocs();
    let mut releases = 0u32;
    let mut t = 0u64;
    for cycle in 0..20u32 {
        // rest, pull, hold, snap, rest; plus the odd bad sample.
        for k in 0..600u32 {
            let raw = match k {
                0..100 => 0.004 * ((k % 5) as f32 - 2.0),
                100..400 => 0.9 * (k - 100) as f32 / 300.0,
                400..500 => 0.9,
                500..530 => 0.9 * (530 - k) as f32 / 30.0,
                _ => 0.0,
            };
            let raw = if k == 450 && cycle % 4 == 0 {
                f32::NAN
            } else {
                raw
            };
            let o = black_box(f.update(black_box(raw), t));
            releases += u32::from(o.release.is_some());
            black_box(d.update(raw, t));
            t += 1;
        }
    }
    let after = allocs();
    assert!(
        releases >= 10,
        "the loop exercised the release path ({releases})"
    );
    assert_eq!(after - before, 0, "PlungerFilter::update allocated");
}

#[test]
fn nudge_filters_do_not_allocate() {
    let mut f = NudgeFilter::new(NudgeConfig {
        high_pass_hz: 2.0,
        gain: 1.5,
        curve_exponent: 1.5,
        deadzone: 0.02,
        max: 1.0,
    });
    let mut d = NudgeFilter::new(NudgeConfig::default());
    let mut digital = DigitalNudge::new(0.7, 30);

    let before = allocs();
    let mut energy = 0.0f32;
    for k in 0..20_000u64 {
        let x = 0.8 * (k as f32 * 0.11).sin();
        let input = if k % 997 == 0 { [f32::NAN, x] } else { [x, -x] };
        let o = black_box(f.update(black_box(input), 0.001));
        energy += o[0].abs() + o[1].abs();
        black_box(d.update(input, 0.001));
        if k % 5000 == 4999 {
            f.reset();
            d.reset();
        }
        if k % 7 == 0 {
            let dir = match (k / 7) % 3 {
                0 => NudgeDirection::Left,
                1 => NudgeDirection::Right,
                _ => NudgeDirection::Forward,
            };
            digital.press(dir, k);
        }
        let s = black_box(digital.sample(k));
        energy += s[0].abs() + s[1].abs();
        let t = black_box(thumbstick_nudge(x, 0.5 - x, 0.2));
        energy += t[0].abs() + t[1].abs();
    }
    let after = allocs();
    assert!(energy > 0.0, "the loop produced output");
    assert_eq!(
        after - before,
        0,
        "a nudge filter allocated on its hot path"
    );
}
