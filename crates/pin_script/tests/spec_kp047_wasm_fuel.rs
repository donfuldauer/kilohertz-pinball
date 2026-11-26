//! KP-047 (#128) independent spec tests, Wasm side (SPEC 4.5, D9, D25, R11).
//!
//! Instantiation fuel must be exact: whatever a module's element segments,
//! data segments and global initialisers are, its start function receives
//! exactly `fuel_per_callback` (never fewer: no under-count of the start-up
//! code's charge; never more: no over-grant), so whether a module loads is a
//! pure function of the module and the config on every platform (D9;
//! DECISIONS "4.5 — fix: platform-independent instantiation fuel (macOS CI)").
//!
//! Black-box: the minimum `fuel_per_callback` under which a module with a
//! 300-iteration start function loads is the same with and without any
//! segments or globals, and 1 without a start function.

use pin_core::ScriptError;
use pin_script::NameTables;
use pin_script::wasm::{WasmConfig, WasmScriptBackend, check_limits};
use proptest::prelude::*;

/// The KP-047 module template; `start` is `(start $s)` or empty.
fn module(start: &str, rest: &str) -> String {
    format!(
        r#"(module (memory (export "memory") 4) (table 64 funcref)
  (func $f) (elem declare func $f)
  (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32))
  (func $s (local $i i32) (loop $l (local.set $i (i32.add (local.get $i) (i32.const 1))) (br_if $l (i32.lt_u (local.get $i) (i32.const 300)))))
  {start} {rest})"#
    )
}

fn config(fuel: u64) -> WasmConfig {
    WasmConfig {
        memory_limit_bytes: 16 << 20,
        fuel_per_callback: fuel,
    }
}

/// `Ok(true)` loads, `Ok(false)` ran out of fuel, `Err` any other error.
fn loads(src: &str, fuel: u64) -> Result<bool, ScriptError> {
    match WasmScriptBackend::new(src.as_bytes(), &NameTables::default(), config(fuel)) {
        Ok(_) => Ok(true),
        Err(ScriptError::BudgetExceeded) => Ok(false),
        Err(e) => Err(e),
    }
}

/// The smallest budget under which `src` loads (copied from
/// tests/wasm_backend.rs, as the brief asks).
fn min_load_fuel(src: &str) -> u64 {
    let loads = |fuel| match loads(src, fuel) {
        Ok(ok) => ok,
        Err(e) => panic!("unexpected load error {e}"),
    };
    if loads(1) {
        return 1;
    }
    let (mut lo, mut hi) = (1u64, 1u64 << 20);
    assert!(loads(hi));
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if loads(mid) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

fn base() -> u64 {
    let base = min_load_fuel(&module("(start $s)", ""));
    assert!(base > 300, "{base}");
    base
}

/// B2 case list: `(fragment, whether wasmtime must run start-up code for it)`
/// where `Some(true)` means not precomputed (init fuel >= 1), `Some(false)`
/// means free, `None` means engine-defined (only B1 equality is asserted).
const CASES: &[(&str, Option<bool>)] = &[
    // Empty expression-form active element segments.
    ("(elem (i32.const 0) funcref)", Some(true)),
    ("(elem (table 0) (i32.const 3) funcref)", Some(true)),
    ("(elem (i32.const 63) funcref)", Some(true)),
    ("(elem (i32.const 64) funcref)", Some(true)),
    // Ordering around the precomputed run.
    (
        "(elem (i32.const 0) func $f) (elem (i32.const 1) funcref) (elem (i32.const 2) func $f $f)",
        Some(true),
    ),
    (
        "(elem (i32.const 0) funcref) (elem (i32.const 0) func $f $f $f)",
        Some(true),
    ),
    ("(elem (i32.const 0) func $f $f $f)", Some(false)),
    // Passive and declared empties.
    ("(elem funcref)", None),
    ("(elem declare funcref)", None),
    // Non-numeric global initialisers.
    ("(global funcref (ref.func $f))", Some(true)),
    ("(global funcref (ref.null func))", Some(true)),
    ("(global (mut funcref) (ref.null func))", Some(true)),
    ("(global externref (ref.null extern))", Some(true)),
    ("(global (mut externref) (ref.null extern))", Some(true)),
    (
        "(global funcref (ref.func $f)) (global externref (ref.null extern)) (global (mut funcref) (ref.null func)) (global (mut funcref) (ref.func $f))",
        Some(true),
    ),
    // Numeric controls.
    ("(global i32 (i32.const 7))", None),
    ("(global (mut i64) (i64.const 1))", None),
    // Globals with element and data segments in one module.
    (
        r#"(global funcref (ref.func $f)) (elem (i32.const 5) funcref) (data (i32.const 1024) "left_sling")"#,
        Some(true),
    ),
    (
        r#"(global (mut externref) (ref.null extern)) (global i32 (i32.const 7)) (elem (i32.const 0) func $f $f) (elem (i32.const 9) funcref (ref.func $f) (ref.null func)) (data (i32.const 65531) "start")"#,
        Some(true),
    ),
    (
        r#"(elem (i32.const 64) funcref) (global funcref (ref.null func)) (data (i32.const 3) "xyxyxy") (data (i32.const 131000) "z")"#,
        Some(true),
    ),
];

/// Data segments each case is also combined with.
const DATA: &[&str] = &[
    r#"(data (i32.const 1024) "left_sling") (data (i32.const 65531) "start")"#,
    r#"(data (i32.const 0) "")"#,
];

/// Every B2 case alone and combined with each data segment set.
fn all_cases() -> Vec<String> {
    let mut v = Vec::new();
    for &(c, _) in CASES {
        v.push(c.to_owned());
        for d in DATA {
            v.push(format!("{d} {c}"));
            v.push(format!("{c} {d}"));
        }
    }
    v
}

/// KP-047 B1/B2, SPEC 4.5, D9: for every case the minimum loading budget
/// with a start function equals the baseline's (the start function gets
/// exactly `fuel_per_callback`: no under-count, no over-grant), and without a
/// start function every budget loads.
#[test]
fn kp047_spec_4_5_start_function_gets_exactly_fuel_per_callback() {
    let base = base();
    let mut failures = Vec::new();
    for case in all_cases() {
        let with = min_load_fuel(&module("(start $s)", &case));
        let without = min_load_fuel(&module("", &case));
        if with != base || without != 1 {
            failures.push(format!(
                "{case:.80}: min fuel with start {with} (want {base}), without start {without} (want 1)"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// KP-047 B4, SPEC 4.5, D9: with `fuel_per_callback = base - 1` every case
/// fails to load with `BudgetExceeded` (the start function is never given
/// more than its budget), and with `base` every case loads.
#[test]
fn kp047_spec_4_5_start_function_is_never_over_granted() {
    let base = base();
    let mut failures = Vec::new();
    for case in std::iter::once(String::new()).chain(all_cases()) {
        let src = module("(start $s)", &case);
        match (loads(&src, base - 1), loads(&src, base)) {
            (Ok(false), Ok(true)) => {}
            other => failures.push(format!(
                "{case:.80}: (base - 1, base) gave {other:?}, want (BudgetExceeded, loads)"
            )),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// KP-047 B2, SPEC 4.5, D9: load success does not depend on the budget for
/// modules without a start function, across a sweep of budgets.
#[test]
fn kp047_spec_4_5_modules_without_start_load_under_any_budget() {
    for case in all_cases() {
        let src = module("", &case);
        for fuel in [1u64, 2, 3, 7, 100, 301, 10_000] {
            assert_eq!(loads(&src, fuel), Ok(true), "{case:.80} fuel {fuel}");
        }
    }
}

/// KP-047 (optional), SPEC 4.5: `Usage::init_fuel` of a module without a
/// start function is >= 1 for the cases that need start-up code and 0 for
/// the bare template and the precomputed-only control (a free module runs no
/// start-up code); with a start function, the precomputed-only control costs
/// what the bare template costs (the start call's own start-up charge).
#[test]
fn kp047_spec_4_5_init_fuel_is_reported_for_start_up_code() {
    let init = |start: &str, case: &str| {
        check_limits(module(start, case).as_bytes())
            .unwrap_or_else(|e| panic!("{case:.80}: {e:?}"))
            .init_fuel
    };
    assert_eq!(
        init("", ""),
        0,
        "a module without segments needs no start-up code"
    );
    let bare_with_start = init("(start $s)", "");
    let mut failures = Vec::new();
    for &(case, nonzero) in CASES {
        let without = init("", case);
        match nonzero {
            Some(true) if without < 1 => {
                failures.push(format!("{case:.80}: init_fuel {without} (want >= 1)"))
            }
            Some(false) if without != 0 || init("(start $s)", case) != bare_with_start => failures
                .push(format!(
                    "{case:.80}: init_fuel {without} / {} with start (want 0 / {bare_with_start})",
                    init("(start $s)", case)
                )),
            _ => {}
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// KP-047 B1, SPEC 4.5: the template's table and declared segment cost the
/// start function nothing: its minimum budget equals that of a module with
/// only memory, exports and the start function.
#[test]
fn kp047_spec_4_5_template_baseline_matches_a_minimal_module() {
    let minimal = r#"(module (memory (export "memory") 4)
  (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32))
  (func $s (local $i i32) (loop $l (local.set $i (i32.add (local.get $i) (i32.const 1))) (br_if $l (i32.lt_u (local.get $i) (i32.const 300)))))
  (start $s))"#;
    assert_eq!(min_load_fuel(minimal), base());
}

// ---------------------------------------------------------------------------
// B3: random fragment lists.

fn fragment() -> impl Strategy<Value = String> {
    prop_oneof![
        (0u32..72).prop_map(|o| format!("(elem (i32.const {o}) funcref)")),
        (0u32..72).prop_map(|o| format!("(elem (table 0) (i32.const {o}) funcref)")),
        (0u32..72, 1usize..4)
            .prop_map(|(o, n)| format!("(elem (i32.const {o}) func{})", " $f".repeat(n))),
        (0u32..72, 1usize..3).prop_map(|(o, n)| format!(
            "(elem (i32.const {o}) funcref{})",
            " (ref.func $f) (ref.null func)".repeat(n)
        )),
        Just("(elem funcref)".to_owned()),
        Just("(elem funcref (ref.func $f))".to_owned()),
        Just("(elem declare funcref)".to_owned()),
        Just("(global funcref (ref.func $f))".to_owned()),
        Just("(global funcref (ref.null func))".to_owned()),
        Just("(global (mut funcref) (ref.null func))".to_owned()),
        Just("(global externref (ref.null extern))".to_owned()),
        Just("(global (mut externref) (ref.null extern))".to_owned()),
        Just("(global i32 (i32.const 7))".to_owned()),
        Just("(global (mut i64) (i64.const 1))".to_owned()),
        (0u32..262_150, 0usize..40)
            .prop_map(|(o, n)| format!(r#"(data (i32.const {o}) "{}")"#, "d".repeat(n))),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// KP-047 B3, SPEC 4.5, D9: any short list of fragments (any order,
    /// offsets inside and beyond the table) satisfies B1 when the module is
    /// valid; invalid ones are discarded, never counted as passes.
    #[test]
    fn kp047_spec_4_5_random_fragment_lists_never_change_the_start_budget(
        frags in proptest::collection::vec(fragment(), 1..=6),
    ) {
        let rest = frags.join(" ");
        let with = module("(start $s)", &rest);
        let without = module("", &rest);
        // Valid and instantiable under a generous budget, or discarded.
        prop_assume!(loads(&with, 1 << 20).is_ok() && loads(&without, 1 << 20).is_ok());
        prop_assume!(loads(&without, 1 << 20) == Ok(true));
        let base = base();
        let got = min_load_fuel(&with);
        prop_assert_eq!(got, base, "{:.80}", rest);
        prop_assert_eq!(min_load_fuel(&without), 1, "{:.80}", rest);
        prop_assert_eq!(loads(&with, base - 1), Ok(false), "{:.80}", rest);
    }
}

// ---------------------------------------------------------------------------
// B5: parse safety (R11).

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// KP-047 B5, R11: `check_limits` never panics on bounded random bytes.
    #[test]
    fn kp047_r11_check_limits_survives_random_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..512),
    ) {
        let _ = check_limits(&bytes);
        let mut framed = b"\0asm\x01\0\0\0".to_vec();
        framed.extend_from_slice(&bytes);
        let _ = check_limits(&framed);
    }

    /// KP-047 B5, R11: `check_limits` never panics on valid KP-047 module
    /// binaries with random bytes flipped.
    #[test]
    fn kp047_r11_check_limits_survives_mutated_module_binaries(
        case in 0..CASES.len(),
        flips in proptest::collection::vec((any::<prop::sample::Index>(), 1u8..=255), 1..8),
    ) {
        let bin = wat::parse_str(module("(start $s)", CASES[case].0)).expect("case is valid WAT");
        prop_assert!(check_limits(&bin).is_ok());
        let mut m = bin.clone();
        for (at, x) in flips {
            let i = at.index(m.len());
            m[i] ^= x;
        }
        let _ = check_limits(&m);
        // Truncations of the mutated binary as well.
        for cut in [m.len() / 3, m.len() / 2, m.len() - 1] {
            let _ = check_limits(&m[..cut]);
        }
    }
}
