//! Subphase 3.2 implementer tests: profile parser robustness (random bytes /
//! random TOML), per-mode mapping to cabinet switches, and the mapper
//! allocation guard (gate step 6).

use pin_core::input::{CabinetInput, InputEventKind};
use pin_input::profiles::{
    GamepadAxis, GamepadButton, ProfileMapper, RawInput, SdlGuid, builtin_profiles, detect,
    parse_profile,
};
use proptest::prelude::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;
thread_local! { static ALLOCS: Cell<u64> = const { Cell::new(0) }; }
// SAFETY: forwards to the system allocator, only adding a thread-local counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(p, l, n) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;
fn allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

fn pressed_input(e: Option<pin_core::input::InputEvent>) -> Option<CabinetInput> {
    match e?.kind {
        InputEventKind::Button { input, .. } => Some(input),
        InputEventKind::Axis { .. } => None,
    }
}

/// Each mode translates raw inputs to the canonical cabinet switches.
#[test]
fn every_mode_reaches_its_cabinet_switches() {
    let keyboard = detect(None, true);
    let xk = builtin_profiles()
        .iter()
        .find(|p| p.id == "xarcade_keyboard")
        .unwrap();
    let xg = detect(Some(SdlGuid::from_ids(3, 0xAA55, 0x0101)), false);
    let gp = detect(Some(SdlGuid::from_ids(3, 0x28de, 0x1102)), false);
    assert_eq!(xg.id, "xarcade_gamepad");
    assert_eq!(gp.id, "generic_gamepad");
    // Every built-in preset (generic ones too, as 1.3's maps did) reaches all 8.
    for p in [keyboard, xk, xg, gp] {
        let m = ProfileMapper::new(p);
        let mut seen = Vec::new();
        for sc in 0..512 {
            seen.extend(pressed_input(m.key(0, sc, true)));
        }
        for b in GamepadButton::ALL {
            seen.extend(pressed_input(m.button(0, b, true)));
        }
        for a in GamepadAxis::ALL {
            seen.extend(pressed_input(m.axis(0, a, i16::MAX)));
        }
        for c in &CabinetInput::ALL {
            assert!(seen.contains(c), "{} cannot reach {c:?}", p.id);
        }
    }
}

const FRAGMENTS: &[&str] = &[
    "id = \"a\"\n",
    "name = \"A\"\n",
    "match = [ { gamepad = { vendor = 0x1234, product = 0x5678 } } ]\n",
    "match = [ { keyboard = true } ]\n",
    "match = [ { any_gamepad = false } ]\n",
    "[buttons]\n",
    "left_flipper = [ \"key:225\", \"button:left_shoulder\" ]\n",
    "start = \"axis:left_trigger\"\n",
    "tilt = [ \"key:\" ]\n",
    "bogus = 1\n",
    "[[axes]]\n",
    "raw = \"left_trigger\"\n",
    "axis = \"plunger\"\n",
    "from = 0.0\n",
    "to = 1.0\n",
    "to = inf\n",
    "invert = true\n",
    "[nudge]\n",
    "left = [ \"key:224\" ]\n",
    "forward = \"button:dpad_up\"\n",
    "[match]\n",
    "x = [\n",
    "\"unterminated\n",
    "= =\n",
    "é = 1\n",
    "a.b.c = { d = [ [ ] ] }\n",
];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let text = String::from_utf8_lossy(&bytes);
        if let Err(e) = parse_profile(&text) {
            prop_assert!(!e.to_string().is_empty());
        }
    }

    #[test]
    fn random_toml_never_panics(parts in prop::collection::vec(0..FRAGMENTS.len(), 0..24)) {
        let text: String = parts.iter().map(|&i| FRAGMENTS[i]).collect();
        match parse_profile(&text) {
            Ok(p) => {
                // Anything accepted builds a mapper and maps without panicking.
                let m = ProfileMapper::new(&p);
                for a in GamepadAxis::ALL {
                    let _ = m.axis(0, a, i16::MIN);
                }
                let _ = m.nudge_button(RawInput::Key(224));
            }
            Err(e) => prop_assert!(!e.to_string().is_empty()),
        }
    }

    #[test]
    fn random_raw_specs_never_panic(s in "(key|button|axis|x)?:?[a-z0-9_+x-]{0,12}") {
        if let Some(r) = RawInput::parse(&s) {
            prop_assert_eq!(RawInput::parse(&r.to_string()), Some(r));
        }
    }

    #[test]
    fn guid_hex_round_trips(bytes in prop::array::uniform16(any::<u8>()), upper in any::<bool>()) {
        let g = SdlGuid(bytes);
        let s = if upper { g.to_string().to_uppercase() } else { g.to_string() };
        prop_assert_eq!(SdlGuid::parse_hex(&s), Some(g));
        prop_assert_eq!(g.vendor(), u16::from_le_bytes([bytes[4], bytes[5]]));
        prop_assert_eq!(g.product(), u16::from_le_bytes([bytes[8], bytes[9]]));
    }

    #[test]
    fn mapped_axes_stay_in_range(raw in any::<i16>()) {
        for p in builtin_profiles() {
            let m = ProfileMapper::new(p);
            for a in GamepadAxis::ALL {
                if let Some(e) = m.axis(0, a, raw)
                    && let InputEventKind::Axis { axis, value } = e.kind
                {
                    let (lo, hi) = pin_input::profiles::cabinet_axis_range(axis);
                    prop_assert!(value >= lo && value <= hi);
                }
            }
        }
    }
}

#[test]
fn mapper_per_event_paths_do_not_allocate() {
    let mappers: Vec<ProfileMapper> = builtin_profiles().iter().map(ProfileMapper::new).collect();
    let _ = detect(None, true); // initialise the lazily parsed built-ins first
    let mut n = 0u64;
    let before = allocs();
    for (i, m) in mappers.iter().enumerate() {
        for k in 0..20_000u32 {
            let ts = u64::from(k) * 1000 + i as u64;
            n += u64::from(m.key(ts, k % 600, k % 2 == 0).is_some());
            let b = GamepadButton::ALL[(k as usize) % GamepadButton::ALL.len()];
            n += u64::from(m.button(ts, b, k % 3 == 0).is_some());
            let a = GamepadAxis::ALL[(k as usize) % GamepadAxis::ALL.len()];
            n += u64::from(m.axis(ts, a, (k as i32 * 37 - 32768) as i16).is_some());
            n += u64::from(m.nudge_button(RawInput::Key(k % 600)).is_some());
            n += u64::from(m.nudge_button(RawInput::Button(b)).is_some());
            let _ = detect(
                Some(SdlGuid::from_ids(3, 0xAA55, (k % 4) as u16)),
                k % 5 == 0,
            );
        }
    }
    assert_eq!(allocs(), before, "mapper/detect allocated");
    assert!(n > 0);
}
