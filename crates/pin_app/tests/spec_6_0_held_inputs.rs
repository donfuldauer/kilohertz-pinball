//! Subphase 6.0 spec tests (Independent Tester): `pin_app::platform::HeldInputs`
//! and issue #9 item 2 (input aliases release only at the last release).
//!
//! Contract (brief 6.0 item 2): `HeldInputs::new()`;
//! `set(&mut self, source: u32, input, pressed) -> Option<bool>` is
//! `Some(true)` when `input` goes from not held to held, `Some(false)` when
//! its last holder releases it, `None` otherwise; a repeated press by the
//! same source counts once and a release by a non-holder is ignored;
//! `is_held`; `pressed_mask` (bit `CabinetInput::index`);
//! `release_source(source) -> ArrayVec<CabinetInput, 8>`; at least 256
//! distinct sources; no allocation after `new()`.
//!
//! Spec choices (DECISIONS "6.0-T — HeldInputs spec reading"): source ids
//! are arbitrary `u32` values (the capacity is a number of distinct ids, not
//! an id range); `release_source` returns exactly the inputs whose aggregate
//! state went from held to released (the release edges to forward), compared
//! as a set.

use pin_app::platform::HeldInputs;
use pin_core::input::{CabinetInput, InputEvent, InputEventKind};
use pin_input::{DeviceMatch, DeviceProfile, GamepadButton, ProfileMapper, RawInput};
use proptest::prelude::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::BTreeSet;

// ---------------------------------------------------------------------------
// Counting allocator (thread-local, so parallel tests do not interfere).
// ---------------------------------------------------------------------------

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn note() {
    let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
}

// SAFETY: forwards every call to `System` unchanged; only a thread-local
// counter is added, which neither allocates nor unwinds.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        // SAFETY: forwarded unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

const LF: CabinetInput = CabinetInput::LeftFlipper;

fn mask_of(inputs: impl IntoIterator<Item = CabinetInput>) -> u8 {
    inputs.into_iter().fold(0u8, |m, i| m | (1u8 << i.index()))
}

fn sorted(v: impl IntoIterator<Item = CabinetInput>) -> Vec<CabinetInput> {
    let mut v: Vec<_> = v.into_iter().collect();
    v.sort();
    v
}

// ---------------------------------------------------------------------------
// Scenarios (brief items 2 and 6)
// ---------------------------------------------------------------------------

#[test]
fn new_holds_nothing() {
    let h = HeldInputs::new();
    for i in CabinetInput::ALL {
        assert!(!h.is_held(i));
    }
    assert_eq!(h.pressed_mask(), 0);
}

/// Issue #9 done-when 2: two aliases held, release one → still held,
/// release the other → released.
#[test]
fn two_aliases_release_only_at_the_last_release() {
    let mut h = HeldInputs::new();
    assert_eq!(h.set(1, LF, true), Some(true));
    assert_eq!(h.set(2, LF, true), None);
    assert!(h.is_held(LF));
    assert_eq!(h.set(1, LF, false), None, "one alias still holds it");
    assert!(h.is_held(LF));
    assert_eq!(h.pressed_mask(), mask_of([LF]));
    assert_eq!(h.set(2, LF, false), Some(false), "last holder released");
    assert!(!h.is_held(LF));
    assert_eq!(h.pressed_mask(), 0);
}

#[test]
fn aliases_release_in_either_order() {
    let mut h = HeldInputs::new();
    assert_eq!(h.set(7, LF, true), Some(true));
    assert_eq!(h.set(9, LF, true), None);
    assert_eq!(h.set(9, LF, false), None);
    assert!(h.is_held(LF));
    assert_eq!(h.set(7, LF, false), Some(false));
    assert!(!h.is_held(LF));
}

#[test]
fn repeated_press_by_one_source_counts_once() {
    let mut h = HeldInputs::new();
    assert_eq!(h.set(3, LF, true), Some(true));
    assert_eq!(h.set(3, LF, true), None);
    assert_eq!(h.set(3, LF, true), None);
    assert_eq!(
        h.set(3, LF, false),
        Some(false),
        "one release undoes any number of presses by the same source"
    );
    assert!(!h.is_held(LF));
}

#[test]
fn release_by_a_non_holder_is_ignored() {
    let mut h = HeldInputs::new();
    assert_eq!(h.set(4, LF, false), None, "nothing held");
    assert!(!h.is_held(LF));
    assert_eq!(h.set(4, LF, true), Some(true));
    assert_eq!(h.set(5, LF, false), None, "source 5 never pressed");
    assert!(h.is_held(LF));
    assert_eq!(h.set(4, CabinetInput::Start, false), None, "other input");
    assert!(h.is_held(LF));
}

#[test]
fn inputs_are_independent() {
    let mut h = HeldInputs::new();
    for (k, i) in CabinetInput::ALL.into_iter().enumerate() {
        assert_eq!(h.set(1, i, true), Some(true));
        assert_eq!(
            h.pressed_mask(),
            mask_of(CabinetInput::ALL[..=k].iter().copied())
        );
    }
    assert_eq!(h.pressed_mask(), 0xFF);
    assert_eq!(h.set(1, CabinetInput::Tilt, false), Some(false));
    assert_eq!(h.pressed_mask(), 0x7F);
    assert!(h.is_held(CabinetInput::Launch));
}

#[test]
fn release_source_releases_only_what_no_one_else_holds() {
    let mut h = HeldInputs::new();
    h.set(1, LF, true);
    h.set(1, CabinetInput::Start, true);
    h.set(1, CabinetInput::Action, true);
    h.set(2, LF, true);
    let released = h.release_source(1);
    assert_eq!(
        sorted(released),
        sorted([CabinetInput::Start, CabinetInput::Action])
    );
    assert!(h.is_held(LF), "source 2 still holds the left flipper");
    assert!(!h.is_held(CabinetInput::Start));
    assert!(!h.is_held(CabinetInput::Action));
    assert!(h.release_source(1).is_empty(), "nothing left for source 1");
    assert_eq!(sorted(h.release_source(2)), vec![LF]);
    assert_eq!(h.pressed_mask(), 0);
    // A disconnected source's later release is a non-holder release.
    assert_eq!(h.set(1, LF, false), None);
}

#[test]
fn release_source_of_unknown_source_is_empty() {
    let mut h = HeldInputs::new();
    assert!(h.release_source(42).is_empty());
    h.set(1, LF, true);
    assert!(h.release_source(42).is_empty());
    assert!(h.is_held(LF));
}

/// At least 256 distinct sources, with ids spread over the whole `u32`
/// range, all hold one input; it is released only by the last of them.
#[test]
fn capacity_of_256_distinct_sources() {
    let ids: Vec<u32> = (0..256u32)
        .map(|i| i.wrapping_mul(0x0100_0193).wrapping_add(0xDEAD_0000 ^ i))
        .collect();
    assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), 256);
    let mut h = HeldInputs::new();
    for (k, &s) in ids.iter().enumerate() {
        assert_eq!(h.set(s, LF, true), if k == 0 { Some(true) } else { None });
    }
    for &s in &ids[..255] {
        assert_eq!(h.set(s, LF, false), None, "source {s:#x}");
        assert!(h.is_held(LF));
    }
    assert_eq!(h.set(ids[255], LF, false), Some(false));
    assert!(!h.is_held(LF));
    // The same 256 sources again, now each on a different input pattern,
    // released by disconnect.
    for (k, &s) in ids.iter().enumerate() {
        h.set(s, CabinetInput::ALL[k % 8], true);
    }
    assert_eq!(h.pressed_mask(), 0xFF);
    for &s in &ids {
        h.release_source(s);
    }
    assert_eq!(h.pressed_mask(), 0);
}

// ---------------------------------------------------------------------------
// Allocation (D5): nothing after `new()`
// ---------------------------------------------------------------------------

#[test]
fn no_allocation_after_new() {
    let mut h = HeldInputs::new();
    let before = allocs();
    let mut acc = 0u32;
    for round in 0..4u32 {
        for s in 0..256u32 {
            let src = s.wrapping_mul(2_654_435_761).wrapping_add(round);
            for i in CabinetInput::ALL {
                if h.set(src, i, true).is_some() {
                    acc += 1;
                }
            }
        }
        acc += u32::from(h.pressed_mask());
        acc += u32::from(h.is_held(LF));
        for s in 0..128u32 {
            let src = s.wrapping_mul(2_654_435_761).wrapping_add(round);
            for i in CabinetInput::ALL {
                if h.set(src, i, false).is_some() {
                    acc += 1;
                }
            }
        }
        for s in 128..256u32 {
            let src = s.wrapping_mul(2_654_435_761).wrapping_add(round);
            acc += h.release_source(src).len() as u32;
        }
    }
    let after = allocs();
    assert_eq!(after - before, 0, "HeldInputs allocated after new()");
    assert!(acc > 0);
    assert_eq!(h.pressed_mask(), 0);
}

// ---------------------------------------------------------------------------
// Through `pin_input::ProfileMapper`: two keys (and a gamepad button) bound
// to the left flipper.
// ---------------------------------------------------------------------------

fn alias_profile() -> DeviceProfile {
    DeviceProfile {
        id: "spec_6_0_alias".to_owned(),
        name: "Spec 6.0 alias".to_owned(),
        matches: vec![DeviceMatch::Keyboard],
        buttons: vec![
            (RawInput::Key(225), LF),
            (RawInput::Key(4), LF),
            (RawInput::Key(229), CabinetInput::RightFlipper),
        ],
        axes: Vec::new(),
        nudge_buttons: Vec::new(),
    }
}

fn pad_profile() -> DeviceProfile {
    DeviceProfile {
        id: "spec_6_0_pad".to_owned(),
        name: "Spec 6.0 pad".to_owned(),
        matches: vec![DeviceMatch::AnyGamepad],
        buttons: vec![(RawInput::Button(GamepadButton::ALL[0]), LF)],
        axes: Vec::new(),
        nudge_buttons: Vec::new(),
    }
}

/// Feeds a mapped edge into `held`; returns the forwarded button event (the
/// aggregate transition) if any.
fn feed(
    held: &mut HeldInputs,
    source: u32,
    ev: Option<InputEvent>,
) -> Option<(CabinetInput, bool)> {
    let ev = ev.expect("raw input is bound");
    let InputEventKind::Button { input, pressed } = ev.kind else {
        panic!("digital binding maps to a button event");
    };
    held.set(source, input, pressed).map(|p| (input, p))
}

#[test]
fn profile_mapper_aliases_release_at_the_last_key() {
    let keys = ProfileMapper::new(&alias_profile());
    let mut held = HeldInputs::new();
    // Source id per raw input: the scancode (one keyboard).
    let ev = keys.key(1_000, 225, true);
    assert_eq!(
        ev.map(|e| e.kind),
        Some(InputEventKind::Button {
            input: LF,
            pressed: true
        })
    );
    assert_eq!(feed(&mut held, 225, ev), Some((LF, true)));
    assert_eq!(feed(&mut held, 4, keys.key(2_000, 4, true)), None);

    // The mapper alone is last-edge-wins: releasing one alias yields a
    // release event for the left flipper ...
    let rel = keys.key(3_000, 225, false);
    assert_eq!(
        rel.map(|e| e.kind),
        Some(InputEventKind::Button {
            input: LF,
            pressed: false
        })
    );
    // ... which HeldInputs swallows while the other key still holds it.
    assert_eq!(feed(&mut held, 225, rel), None);
    assert!(held.is_held(LF));
    assert_eq!(
        feed(&mut held, 4, keys.key(4_000, 4, false)),
        Some((LF, false))
    );
    assert!(!held.is_held(LF));
}

#[test]
fn profile_mapper_keyboard_and_gamepad_alias() {
    let keys = ProfileMapper::new(&alias_profile());
    let pad = ProfileMapper::new(&pad_profile());
    let mut held = HeldInputs::new();
    const KEYBOARD_KEY_4: u32 = 0x0000_0004;
    const PAD_SOUTH: u32 = 0x8000_0000;
    assert_eq!(
        feed(&mut held, KEYBOARD_KEY_4, keys.key(10, 4, true)),
        Some((LF, true))
    );
    assert_eq!(
        feed(
            &mut held,
            PAD_SOUTH,
            pad.button(20, GamepadButton::ALL[0], true)
        ),
        None
    );
    assert_eq!(
        feed(&mut held, KEYBOARD_KEY_4, keys.key(30, 4, false)),
        None
    );
    assert!(held.is_held(LF));
    // The gamepad is unplugged: its holds go, and the flipper is released.
    assert_eq!(sorted(held.release_source(PAD_SOUTH)), vec![LF]);
    assert!(!held.is_held(LF));
    // The right flipper key is independent.
    assert_eq!(
        feed(&mut held, 229, keys.key(40, 229, true)),
        Some((CabinetInput::RightFlipper, true))
    );
    assert_eq!(held.pressed_mask(), mask_of([CabinetInput::RightFlipper]));
}

// ---------------------------------------------------------------------------
// Model-based property test
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Op {
    Set {
        src: usize,
        input: usize,
        pressed: bool,
    },
    Disconnect {
        src: usize,
    },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        8 => (0usize..80, 0usize..8, any::<bool>())
            .prop_map(|(src, input, pressed)| Op::Set { src, input, pressed }),
        1 => (0usize..80).prop_map(|src| Op::Disconnect { src }),
    ]
}

/// Pool of 80 distinct source ids over the whole `u32` range (0 and
/// `u32::MAX` included).
fn source_ids() -> Vec<u32> {
    let mut v: Vec<u32> = (0..78u32)
        .map(|i| i.wrapping_mul(0x9E37_79B9) ^ 0x5A5A_0000)
        .collect();
    v.push(0);
    v.push(u32::MAX);
    v
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    #[test]
    fn held_inputs_match_a_set_model(ops in prop::collection::vec(op(), 0..=500)) {
        let ids = source_ids();
        prop_assert_eq!(ids.iter().collect::<BTreeSet<_>>().len(), ids.len());
        let mut h = HeldInputs::new();
        let mut model: BTreeSet<(u32, CabinetInput)> = BTreeSet::new();
        let held_in = |m: &BTreeSet<(u32, CabinetInput)>, i: CabinetInput| {
            m.iter().any(|(_, x)| *x == i)
        };
        for o in ops {
            match o {
                Op::Set { src, input, pressed } => {
                    let (s, i) = (ids[src], CabinetInput::ALL[input]);
                    let was = held_in(&model, i);
                    if pressed {
                        model.insert((s, i));
                    } else {
                        model.remove(&(s, i));
                    }
                    let now = held_in(&model, i);
                    let expect = (was != now).then_some(now);
                    prop_assert_eq!(h.set(s, i, pressed), expect, "{:?}", o);
                }
                Op::Disconnect { src } => {
                    let s = ids[src];
                    let before: Vec<_> = CabinetInput::ALL
                        .into_iter()
                        .filter(|i| held_in(&model, *i))
                        .collect();
                    model.retain(|(x, _)| *x != s);
                    let expect: Vec<_> = before
                        .into_iter()
                        .filter(|i| !held_in(&model, *i))
                        .collect();
                    prop_assert_eq!(sorted(h.release_source(s)), expect, "{:?}", o);
                }
            }
            for i in CabinetInput::ALL {
                prop_assert_eq!(h.is_held(i), held_in(&model, i));
            }
            prop_assert_eq!(
                h.pressed_mask(),
                mask_of(CabinetInput::ALL.into_iter().filter(|i| held_in(&model, *i)))
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Issue #9 done-when 3: the ProfileMapper doc points at HeldInputs.
// ---------------------------------------------------------------------------

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("readable src dir")
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn profile_mapper_doc_mentions_held_inputs() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../pin_input/src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let doc = files
        .iter()
        .find_map(|f| {
            let text = std::fs::read_to_string(f).expect("readable source");
            let lines: Vec<&str> = text.lines().collect();
            let at = lines
                .iter()
                .position(|l| l.trim_start().starts_with("pub struct ProfileMapper"))?;
            let mut doc = Vec::new();
            for l in lines[..at].iter().rev() {
                let t = l.trim_start();
                if let Some(d) = t.strip_prefix("///") {
                    doc.push(d.to_owned());
                } else if t.starts_with("#[") {
                    continue;
                } else {
                    break;
                }
            }
            doc.reverse();
            Some(doc.join("\n"))
        })
        .expect("ProfileMapper is defined in pin_input/src");
    assert!(
        doc.contains("HeldInputs"),
        "the ProfileMapper doc points at HeldInputs for aliased inputs:\n{doc}"
    );
}
