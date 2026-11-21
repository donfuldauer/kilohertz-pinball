//! Adversarial rules modules (hand-written WAT and byte-level mutations)
//! and a runner that classifies each one identically on every design:
//! load outcome, then per call: outcome, fuel used, a digest of the command
//! queue, of the host RNG state and of guest memory.

use crate::run::{Design, Rules, load};
use pin_core::ids::SwitchId;
use pin_core::{
    HostCommandQueue, HostCtx, HostView, Pcg32, Rng, ScriptError, SeedableRng, SwitchEdge,
};
use pin_script::NameTables;
use pin_script::wasm::WasmConfig;
use pin_sim::hash::fnv1a64;
use std::fmt::Write as _;

/// A call in a case script.
#[derive(Copy, Clone, Debug)]
pub enum Call {
    /// `on_init`.
    Init,
    /// `on_tick`.
    Tick,
    /// `on_switch_edge(id, closed)`.
    Switch(u16, bool),
}

use Call::{Init, Switch, Tick};

/// One adversarial case.
pub struct Case {
    /// Name.
    pub name: &'static str,
    /// Module bytes (binary, or WAT text when `wat_text`).
    pub bytes: Vec<u8>,
    /// Calls after a successful load.
    pub calls: Vec<Call>,
}

const IMPORTS: &str = r#"
  (import "pinball" "set_score" (func $set_score (param i32 i64)))
  (import "pinball" "random" (func $random (param i64 i64) (result i64)))
  (import "pinball" "tick" (func $tick (result i64)))
  (import "pinball" "pulse_coil" (func $pulse (param i32 i32)))
  (import "pinball" "hold_coil" (func $hold (param i32 i32)))
  (import "pinball" "set_lamp" (func $lamp (param i32 f32)))
  (import "pinball" "id_of" (func $id_of (param i32 i32 i32) (result i32)))
"#;

/// A module with the standard imports, `extra` items and the three
/// callbacks.
fn m(extra: &str, init: &str, tick: &str, switch: &str) -> String {
    format!(
        r#"(module {IMPORTS}
  (memory (export "memory") 1)
  {extra}
  (func (export "on_init") {init})
  (func (export "on_tick") {tick})
  (func (export "on_switch_edge") (param $id i32) (param $closed i32) {switch}))"#
    )
}

fn wat(s: &str) -> Vec<u8> {
    wat::parse_str(s).unwrap_or_else(|e| panic!("bad WAT: {e}\n{s}"))
}

/// Score := the f32 bits of `expr` (an f32-producing WAT expression).
fn nan_case(expr: &str) -> Vec<u8> {
    wat(&m(
        "",
        "",
        &format!("(call $set_score (i32.const 1) (i64.extend_i32_u (i32.reinterpret_f32 {expr})))"),
        "",
    ))
}

/// Score := the f64 bits of `expr`.
fn nan64_case(expr: &str) -> Vec<u8> {
    wat(&m(
        "",
        "",
        &format!("(call $set_score (i32.const 1) (i64.reinterpret_f64 {expr}))"),
        "",
    ))
}

const STD: [Call; 5] = [Init, Tick, Tick, Switch(3, true), Tick];

/// The corpus. `prod` = one production `rules.wasm` (for byte mutations).
#[must_use]
pub fn corpus(prod: &[u8]) -> Vec<Case> {
    let std = || STD.to_vec();
    let mut v = Vec::new();
    let mut add =
        |name: &'static str, bytes: Vec<u8>, calls: Vec<Call>| v.push(Case { name, bytes, calls });
    // --- Termination.
    add(
        "inf_loop_backedge",
        wat(&m("", "", "(loop $l (br $l))", "")),
        std(),
    );
    add(
        "inf_loop_call",
        wat(&m("(func $f)", "", "(loop $l (call $f) (br $l))", "")),
        std(),
    );
    add(
        "inf_loop_hostcall_random",
        wat(&m(
            "",
            "",
            "(loop $l (drop (call $random (i64.const 0) (i64.const 9))) (br $l))",
            "",
        )),
        std(),
    );
    add(
        "inf_loop_hold_coil",
        wat(&m(
            "",
            "",
            "(loop $l (call $hold (i32.const 7) (i32.const 1)) (br $l))",
            "",
        )),
        std(),
    );
    add(
        "start_loops",
        wat(&m("(func $s (loop $l (br $l))) (start $s)", "", "", "")),
        std(),
    );
    add(
        "start_calls_ctx_import",
        wat(&m(
            "(func $s (call $pulse (i32.const 1) (i32.const 5))) (start $s)",
            "",
            "",
            "",
        )),
        std(),
    );
    add(
        "start_ok_id_of",
        wat(&m(
            r#"(data (i32.const 16) "left_sling") (global $g (mut i32) (i32.const 0))
               (func $s (global.set $g (call $id_of (i32.const 0) (i32.const 16) (i32.const 10)))) (start $s)"#,
            "",
            "(call $set_score (i32.const 1) (i64.extend_i32_s (global.get $g)))",
            "",
        )),
        std(),
    );
    // --- Stack.
    add(
        "deep_recursion",
        wat(&m(
            "(func $r (param i32) (call $r (i32.add (local.get 0) (i32.const 1))))",
            "",
            "(call $r (i32.const 0))",
            "",
        )),
        std(),
    );
    add(
        "deep_recursion_random_each_level",
        wat(&m(
            "(func $r (drop (call $random (i64.const 0) (i64.const 9))) (call $r))",
            "",
            "(call $r)",
            "",
        )),
        std(),
    );
    add(
        "deep_recursion_bigframe",
        wat(&m(
            &format!(
                "(func $r (param i32) (local {}) (local.set 1 (local.get 0)) (call $r (i32.add (local.get 1) (i32.const 1))))",
                "i32 ".repeat(200)
            ),
            "",
            "(call $r (i32.const 0))",
            "",
        )),
        std(),
    );
    // Recursion depth n given by the switch id: finite, observable.
    add(
        "recursion_depth_n",
        wat(&m(
            "(func $r (param i32) (if (local.get 0) (then (call $r (i32.sub (local.get 0) (i32.const 1))))))",
            "",
            "",
            "(call $r (local.get $id)) (call $set_score (i32.const 1) (i64.extend_i32_u (local.get $id)))",
        )),
        vec![
            Init,
            Switch(100, true),
            Switch(1000, true),
            Switch(5000, true),
            Switch(20000, true),
            Switch(60000, true),
        ],
    );
    // --- Memory.
    add(
        "memory_grow_beyond_cap",
        wat(&m(
            "",
            "",
            "(call $set_score (i32.const 1) (i64.extend_i32_s (memory.grow (i32.const 10))))
             (call $set_score (i32.const 2) (i64.extend_i32_s (memory.grow (i32.const 21))))
             (call $set_score (i32.const 3) (i64.extend_i32_s (memory.grow (i32.const 200))))
             (call $set_score (i32.const 4) (i64.extend_i32_s (memory.size)))",
            "",
        )),
        std(),
    );
    add(
        "memory_grow_runtime_delta",
        wat(&m(
            "",
            "",
            "",
            "(call $set_score (i32.const 1) (i64.extend_i32_s (memory.grow (local.get $id))))
             (call $set_score (i32.const 2) (i64.extend_i32_s (memory.size)))",
        )),
        vec![
            Init,
            Switch(5, true),
            Switch(40, true),
            Switch(26, true),
            Switch(1, true),
        ],
    );
    add(
        "huge_initial_memory",
        wat(&format!(
            r#"(module {IMPORTS} (memory (export "memory") 33)
            (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#
        )),
        std(),
    );
    add(
        "huge_declared_max",
        wat(&m("", "", "", "").replace(
            "(memory (export \"memory\") 1)",
            "(memory (export \"memory\") 1 65536)",
        )),
        std(),
    );
    add(
        "data_segment_out_of_bounds",
        wat(&m(r#"(data (i32.const 65530) "0123456789")"#, "", "", "")),
        std(),
    );
    add(
        "big_data_segment_900k",
        wat(&m("", "", "", "").replace(
            "(memory (export \"memory\") 1)",
            &format!(
                "(memory (export \"memory\") 15) (data (i32.const 0) \"{}\")",
                "ab".repeat(450_000)
            ),
        )),
        std(),
    );
    add(
        "memory_fill_2mib_deferred",
        wat(&m(
            "",
            "",
            "(drop (memory.grow (i32.const 31))) (memory.fill (i32.const 0) (i32.const 7) (i32.const 2097152))",
            "",
        )),
        std(),
    );
    add(
        "memory_fill_small_const_loop",
        wat(&m(
            "",
            "",
            "",
            "(local $i i32) (loop $l (memory.fill (i32.const 0) (i32.const 1) (i32.const 100))
               (local.set $i (i32.add (local.get $i) (i32.const 1)))
               (br_if $l (i32.lt_u (local.get $i) (local.get $id))))
             (call $set_score (i32.const 1) (i64.extend_i32_u (local.get $i)))",
        )),
        vec![Init, Switch(900, true), Switch(1000, true)],
    );
    add(
        "memory_fill_const_via_local",
        wat(&m(
            "",
            "",
            "",
            "(local $n i32) (local $i i32) (local.set $n (i32.const 100))
             (loop $l (memory.fill (i32.const 0) (i32.const 1) (local.get $n))
               (local.set $i (i32.add (local.get $i) (i32.const 1)))
               (br_if $l (i32.lt_u (local.get $i) (local.get $id))))
             (call $set_score (i32.const 1) (i64.extend_i32_u (local.get $i)))",
        )),
        vec![Init, Switch(900, true), Switch(1000, true)],
    );
    // --- Too big / too many (pre-scan limits).
    add(
        "many_functions_2049",
        wat(&m(&"(func)".repeat(2049 - 3), "", "", "")),
        std(),
    );
    add(
        "many_locals_1025",
        wat(&m(
            &format!("(func (local {}))", "i32 ".repeat(1025)),
            "",
            "",
            "",
        )),
        std(),
    );
    add(
        "nested_loops_33",
        wat(&m(
            "",
            "",
            &format!("{}{}", "(loop ".repeat(33), ")".repeat(33)),
            "",
        )),
        std(),
    );
    // --- Tables.
    add(
        "table_grow",
        wat(&m(
            "(table $t 1 funcref)",
            "",
            "(call $set_score (i32.const 1) (i64.extend_i32_s (table.grow $t (ref.null func) (i32.const 10))))
             (call $set_score (i32.const 2) (i64.extend_i32_s (table.grow $t (ref.null func) (i32.const 70000))))
             (call $set_score (i32.const 3) (i64.extend_i32_s (table.size $t)))",
            "",
        )),
        std(),
    );
    add(
        "call_indirect_type_confusion",
        wat(&m(
            "(type $v (func)) (func $f (param i32) (result i32) (local.get 0)) (table 1 funcref) (elem (i32.const 0) $f)",
            "",
            "(call_indirect (type $v) (i32.const 0))",
            "",
        )),
        std(),
    );
    add(
        "call_indirect_null",
        wat(&m(
            "(type $v (func)) (table 4 funcref)",
            "",
            "(call_indirect (type $v) (i32.const 2))",
            "",
        )),
        std(),
    );
    add(
        "call_indirect_oob",
        wat(&m(
            "(type $v (func)) (table 4 funcref)",
            "",
            "(call_indirect (type $v) (i32.const 9))",
            "",
        )),
        std(),
    );
    add(
        "too_many_tables",
        wat(&m(
            "(table 1 funcref) (table 1 funcref) (table 1 funcref) (table 1 funcref) (table 1 funcref)",
            "",
            "",
            "",
        )),
        std(),
    );
    // --- Traps.
    add(
        "div_by_zero",
        wat(&m(
            "",
            "",
            "(drop (i32.div_s (i32.const 1) (i32.const 0)))",
            "",
        )),
        std(),
    );
    add(
        "int_overflow",
        wat(&m(
            "",
            "",
            "(drop (i32.div_s (i32.const -2147483648) (i32.const -1)))",
            "",
        )),
        std(),
    );
    add("unreachable", wat(&m("", "", "unreachable", "")), std());
    add(
        "trunc_nan",
        wat(&m(
            "",
            "",
            "(drop (i32.trunc_f32_s (f32.div (f32.const 0) (f32.const 0))))",
            "",
        )),
        std(),
    );
    add(
        "oob_load",
        wat(&m("", "", "(drop (i32.load (i32.const 65533)))", "")),
        std(),
    );
    add(
        "trap_after_partial_queue",
        wat(&m(
            "",
            "",
            "(call $pulse (i32.const 1) (i32.const 20)) (call $lamp (i32.const 2) (f32.const 0.5))
             (call $hold (i32.const 4) (i32.const 1)) (call $set_score (i32.const 1) (i64.const 77))
             (i32.store (i32.const 0) (i32.const 0x55aa55aa))
             (if (i64.gt_s (call $tick) (i64.const 1)) (then unreachable))",
            "",
        )),
        std(),
    );
    add(
        "host_arg_error",
        wat(&m(
            "",
            "",
            "(call $pulse (i32.const 70000) (i32.const 1))",
            "",
        )),
        std(),
    );
    add(
        "host_nan_gain",
        wat(&m(
            "",
            "",
            "(call $lamp (i32.const 1) (f32.div (f32.const 0) (f32.const 0)))",
            "",
        )),
        std(),
    );
    // --- Imports / exports.
    add(
        "unlisted_import",
        wat(
            r#"(module (import "pinball" "launch_missiles" (func)) (memory (export "memory") 1)
            (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#,
        ),
        std(),
    );
    add(
        "wasi_import",
        wat(
            r#"(module (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32))) (memory (export "memory") 1)
            (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#,
        ),
        std(),
    );
    add(
        "import_wrong_signature",
        wat(
            r#"(module (import "pinball" "pulse_coil" (func (param i32))) (memory (export "memory") 1)
            (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#,
        ),
        std(),
    );
    add(
        "import_memory",
        wat(
            r#"(module (import "pinball" "memory" (memory 1)) (export "memory" (memory 0))
            (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#,
        ),
        std(),
    );
    add(
        "export_wrong_signature",
        wat(r#"(module (memory (export "memory") 1)
            (func (export "on_init")) (func (export "on_tick") (param i32)) (func (export "on_switch_edge") (param i32 i32)))"#),
        std(),
    );
    add(
        "reexport_import_as_callback",
        wat(
            r#"(module (import "pinball" "tick" (func $t (result i64))) (import "pinball" "pulse_coil" (func $p (param i32 i32)))
            (memory (export "memory") 1)
            (func (export "on_init")) (export "on_switch_edge" (func $p)) (func (export "on_tick")))"#,
        ),
        std(),
    );
    add(
        "missing_memory_export",
        wat(
            r#"(module (memory 1) (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#,
        ),
        std(),
    );
    add(
        "mutable_global_export_collision",
        wat(&m(
            r#"(global $g (export "__khz_fuel") (mut i64) (i64.const 5))
               (global $w (export "__khz_why") (mut i32) (i32.const 0))
               (func (export "__khz_begin") (param f64))"#,
            "",
            "(global.set $g (i64.const -1)) (global.set $w (i32.const 1))
             (call $set_score (i32.const 1) (global.get $g))",
            "",
        )),
        std(),
    );
    add(
        "export_import_under_other_name",
        wat(&m(
            "(export \"my_tick\" (func $tick))",
            "",
            "(drop (call $tick))",
            "",
        )),
        std(),
    );
    // --- Fuel boundary (switch id = loop count; one random() per iteration).
    add(
        "fuel_boundary_random_loop",
        wat(&m(
            "",
            "",
            "",
            "(local $i i32) (loop $l (drop (call $random (i64.const 0) (i64.const 1000)))
               (local.set $i (i32.add (local.get $i) (i32.const 1)))
               (br_if $l (i32.lt_u (local.get $i) (local.get $id))))
             (call $set_score (i32.const 1) (i64.extend_i32_u (local.get $i)))",
        )),
        vec![Init, Switch(100, true)],
    );
    add(
        "id_of_long_name_fuel",
        wat(&m(
            "",
            "",
            "",
            "(local $i i32) (loop $l (drop (call $id_of (i32.const 0) (i32.const 0) (i32.const 60000)))
               (local.set $i (i32.add (local.get $i) (i32.const 1)))
               (br_if $l (i32.lt_u (local.get $i) (local.get $id))))
             (call $set_score (i32.const 1) (i64.extend_i32_u (local.get $i)))",
        )),
        vec![Init, Switch(1, true), Switch(2, true)],
    );
    add(
        "id_of_bad_kind",
        wat(&m(
            "",
            "",
            "(drop (call $id_of (i32.const 11) (i32.const 0) (i32.const 1)))",
            "",
        )),
        std(),
    );
    add(
        "id_of_oob_name",
        wat(&m(
            "",
            "",
            "(drop (call $id_of (i32.const 0) (i32.const 65530) (i32.const 10)))",
            "",
        )),
        std(),
    );
    // --- NaN payloads (score := bits).
    add(
        "nan_div0_f32",
        nan_case("(f32.div (f32.const 0) (f32.const 0))"),
        std(),
    );
    add(
        "nan_sqrt_neg_f32",
        nan_case("(f32.sqrt (f32.const -1))"),
        std(),
    );
    add(
        "nan_payload_add_f32",
        nan_case("(f32.add (f32.reinterpret_i32 (i32.const 0x7fa00001)) (f32.const 1))"),
        std(),
    );
    add(
        "nan_payload_neg_f32",
        nan_case("(f32.neg (f32.reinterpret_i32 (i32.const 0x7fa00001)))"),
        std(),
    );
    add(
        "nan_min_f32",
        nan_case("(f32.min (f32.reinterpret_i32 (i32.const 0x7fa00001)) (f32.const 1))"),
        std(),
    );
    add(
        "nan_demote",
        nan_case("(f32.demote_f64 (f64.reinterpret_i64 (i64.const 0x7ff4000000000001)))"),
        std(),
    );
    add(
        "nan_promote",
        nan64_case("(f64.promote_f32 (f32.reinterpret_i32 (i32.const 0x7fa00001)))"),
        std(),
    );
    add(
        "nan_div0_f64",
        nan64_case("(f64.div (f64.const 0) (f64.const 0))"),
        std(),
    );
    add(
        "nan_store_load_roundtrip",
        nan_case(
            "(block (result f32) (f32.store (i32.const 8) (f32.reinterpret_i32 (i32.const 0x7fa00001))) (f32.load (i32.const 8)))",
        ),
        std(),
    );
    // --- Proposals (accepted set must equal native's).
    add(
        "feat_bulk_memory",
        wat(&m(
            r#"(data $d "abc")"#,
            "",
            "(memory.init $d (i32.const 0) (i32.const 0) (i32.const 3)) (data.drop $d) (memory.copy (i32.const 4) (i32.const 0) (i32.const 3))",
            "",
        )),
        std(),
    );
    add(
        "feat_reference_types",
        wat(&m(
            "(table $t 2 funcref) (table $e 2 externref) (func $f) (elem declare func $f)",
            "",
            "(table.set $t (i32.const 1) (ref.func $f)) (drop (ref.is_null (table.get $t (i32.const 0)))) (drop (table.get $e (i32.const 1)))",
            "",
        )),
        std(),
    );
    add(
        "feat_multi_value",
        wat(&m(
            "(func $two (result i32 i64) (i32.const 1) (i64.const 2))",
            "",
            "call $two call $set_score",
            "",
        )),
        std(),
    );
    add(
        "feat_sign_ext_sat",
        wat(&m(
            "",
            "",
            "(drop (i32.extend8_s (i32.const 255))) (drop (i32.trunc_sat_f32_s (f32.const 1e20)))",
            "",
        )),
        std(),
    );
    add(
        "feat_threads_shared_memory",
        wat(r#"(module (memory (export "memory") 1 1 shared)
            (func (export "on_init")) (func (export "on_tick") (drop (i32.atomic.load (i32.const 0)))) (func (export "on_switch_edge") (param i32 i32)))"#),
        std(),
    );
    add(
        "feat_simd",
        wat(&m("", "", "(drop (i32x4.splat (i32.const 1)))", "")),
        std(),
    );
    add(
        "feat_relaxed_simd",
        wat(&m(
            "",
            "",
            "(drop (f32x4.relaxed_madd (v128.const i32x4 0 0 0 0) (v128.const i32x4 0 0 0 0) (v128.const i32x4 0 0 0 0)))",
            "",
        )),
        std(),
    );
    add(
        "feat_tail_call",
        wat(&m("(func $f)", "", "(return_call $f)", "")),
        std(),
    );
    add(
        "feat_exceptions",
        wat(&m(
            "(tag $e)",
            "",
            "(block $b (try_table (catch_all $b) (throw $e)))",
            "",
        )),
        std(),
    );
    add(
        "feat_memory64",
        wat(r#"(module (memory (export "memory") i64 1)
            (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#),
        std(),
    );
    add(
        "feat_multi_memory",
        wat(&m(
            "(memory $b 1)",
            "",
            "(drop (i32.load $b (i32.const 0)))",
            "",
        )),
        std(),
    );
    add(
        "feat_extended_const",
        wat(&m(
            "(global i32 (i32.add (i32.const 1) (i32.const 2)))",
            "",
            "",
            "",
        )),
        std(),
    );
    add(
        "feat_gc",
        wat(&m(
            "(type $s (struct (field i32)))",
            "",
            "(drop (struct.new $s (i32.const 1)))",
            "",
        )),
        std(),
    );
    // --- Malformed bytes.
    add("zero_length", Vec::new(), std());
    add("bad_magic", b"\0wasm\x01\0\0\0".to_vec(), std());
    add(
        "truncated_prod_half",
        prod[..prod.len() / 2].to_vec(),
        std(),
    );
    add(
        "truncated_prod_minus1",
        prod[..prod.len() - 1].to_vec(),
        std(),
    );
    let mut g = prod.to_vec();
    g.extend_from_slice(b"\x00\x01garbage");
    add("trailing_garbage_prod", g, std());
    let mut g = prod.to_vec();
    g.extend_from_slice(&[0, 3, 1, b'x', 0]); // a well-formed trailing custom section
    add("trailing_custom_section_prod", g, std());
    add(
        "wat_text_module",
        m("", "", "(call $set_score (i32.const 1) (i64.const 5))", "").into_bytes(),
        std(),
    );
    add("production_module", prod.to_vec(), std());
    v
}

/// Names the cases' `id_of` finds.
fn names() -> NameTables {
    NameTables {
        switches: vec![("left_sling".into(), SwitchId(5))],
        ..NameTables::default()
    }
}

/// Canonical fault classification across engines (message texts differ
/// per engine for traps; host and load-check texts must match exactly).
#[must_use]
pub fn norm(e: &ScriptError) -> String {
    let kind = |m: &str| -> String {
        let l = m.to_ascii_lowercase();
        let k = if l.contains("pinball.") {
            return format!("host:{m}");
        } else if l.contains("unreachable") {
            "unreachable"
        } else if l.contains("divide by zero")
            || l.contains("division by zero")
            || l.contains("divided by zero")
        {
            "div0"
        } else if l.contains("integer overflow") || l.contains("divide result unrepresentable") {
            "int-overflow"
        } else if l.contains("invalid conversion")
            || l.contains("float unrepresentable")
            || l.contains("conversion to integer")
        {
            "bad-conversion"
        } else if l.contains("call stack")
            || l.contains("stack overflow")
            || l.contains("maximum call stack")
            || l.contains("stack exhausted")
        {
            "stack"
        } else if l.contains("null function or function signature mismatch") {
            // V8 reports a null entry and a signature mismatch alike.
            "sig-mismatch-or-null"
        } else if l.contains("indirect call type mismatch") || l.contains("signature") {
            "sig-mismatch"
        } else if l.contains("uninitialized element") || l.contains("null") {
            "null-ref"
        } else if l.contains("table") || l.contains("element") {
            "oob-table"
        } else if l.contains("memory") || l.contains("out of bounds") {
            "oob-memory"
        } else {
            return format!("other:{m}");
        };
        k.to_owned()
    };
    match e {
        ScriptError::BudgetExceeded => "budget".into(),
        ScriptError::MemoryLimit => "memlimit".into(),
        ScriptError::Runtime(m) => format!("runtime:{}", kind(m)),
        ScriptError::Load(m) => {
            // Pre-scan and ABI-check texts are engine-neutral: keep them;
            // instantiation traps are normalised like runtime traps.
            let l = m.to_ascii_lowercase();
            let trap = [
                "unreachable",
                "out of bounds",
                "divide",
                "overflow",
                "call stack",
                "pinball.",
                "stack",
            ]
            .iter()
            .any(|k| l.contains(k))
                && !l.starts_with("module ")
                && !l.starts_with("unsupported import")
                && !l.starts_with("export ")
                && !l.starts_with("missing export");
            if trap {
                format!("load-trap:{}", kind(m))
            } else {
                format!(
                    "load:{}",
                    crate::prescan::truncated(&m.replace('\n', " "), 120)
                )
            }
        }
        other => format!("{other:?}"),
    }
}

/// Runs one case on one design; one line.
#[must_use]
pub fn run_case(design: Design, c: &Case) -> String {
    let cfg = WasmConfig::engine();
    let mut out = format!("{} {}", design.name(), c.name);
    let mut b: Box<dyn Rules> = match load(design, &c.bytes, &names(), cfg) {
        Ok(b) => b,
        Err(e) => {
            let _ = write!(out, " | load={}", norm(&e));
            return out;
        }
    };
    out.push_str(" | load=ok");
    let view = HostView::default();
    let mut rng = Pcg32::seed_from_u64(0x5eed);
    for (i, call) in c.calls.iter().enumerate() {
        let mut cmd = HostCommandQueue::default();
        let was = b.is_faulted();
        {
            let mut v = view;
            v.tick = i as u64 + 1;
            let mut ctx = HostCtx {
                view: &v,
                rng: &mut rng,
                cmd: &mut cmd,
            };
            match *call {
                Init => b.on_init(&mut ctx),
                Tick => b.on_tick(&mut ctx),
                Switch(id, cl) => b.on_switch_edge(
                    SwitchId(id),
                    if cl {
                        SwitchEdge::Closed
                    } else {
                        SwitchEdge::Opened
                    },
                    &mut ctx,
                ),
            }
        }
        let mut qb = Vec::new();
        crate::io::encode_queue(&cmd, &mut qb);
        let r = rng.clone().next_u64();
        let mem = fnv1a64(&b.memory());
        let outcome = if was {
            "skipped".to_owned()
        } else if b.is_faulted() {
            b.last_error().map_or_else(|| "fault?".into(), norm)
        } else {
            "ok".into()
        };
        // Fuel is reported for completed and budget-exhausted calls only:
        // after any other trap it is not observable (the callback faults)
        // and engines leave different partial counts behind.
        let fuel = if was || (b.is_faulted() && outcome != "budget") {
            0
        } else {
            b.last_fuel()
        };
        let scores: Vec<String> = cmd
            .scores
            .iter()
            .map(|(p, s)| format!("{p}:{s:#x}"))
            .collect();
        let _ = write!(
            out,
            " | {call:?}={outcome} fuel={fuel} q={:08x} rng={:08x} mem={:08x} scores=[{}]",
            fnv1a64(&qb) as u32,
            r as u32,
            mem as u32,
            scores.join(",")
        );
    }
    out
}

/// Largest loop count `n` (switch id) for which the case's switch callback
/// completes within the budget, by bisection on fresh instances.
#[must_use]
pub fn boundary(design: Design, c: &Case) -> String {
    let cfg = WasmConfig::engine();
    let ok = |n: u16| -> Option<bool> {
        let mut b = load(design, &c.bytes, &names(), cfg).ok()?;
        let view = HostView::default();
        let mut rng = Pcg32::seed_from_u64(1);
        let mut cmd = HostCommandQueue::default();
        b.on_switch_edge(
            SwitchId(n),
            SwitchEdge::Closed,
            &mut HostCtx {
                view: &view,
                rng: &mut rng,
                cmd: &mut cmd,
            },
        );
        Some(!b.is_faulted())
    };
    let (mut lo, mut hi) = (1u16, u16::MAX);
    if ok(hi) == Some(true) {
        return format!("{} {} boundary>=65535", design.name(), c.name);
    }
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        match ok(mid) {
            Some(true) => lo = mid,
            Some(false) => hi = mid,
            None => return format!("{} {} load-failed", design.name(), c.name),
        }
    }
    // Fuel used at the boundary and the RNG draws made by the failing run.
    let detail = |n: u16| {
        let Ok(mut b) = load(design, &c.bytes, &names(), cfg) else {
            return String::new();
        };
        let view = HostView::default();
        let mut rng = Pcg32::seed_from_u64(1);
        let mut cmd = HostCommandQueue::default();
        b.on_switch_edge(
            SwitchId(n),
            SwitchEdge::Closed,
            &mut HostCtx {
                view: &view,
                rng: &mut rng,
                cmd: &mut cmd,
            },
        );
        format!(
            "n={n}:{}:fuel={}:rng={:08x}",
            if b.is_faulted() { "fault" } else { "ok" },
            b.last_fuel(),
            rng.next_u64() as u32
        )
    };
    format!(
        "{} {} last_ok={lo} {} {}",
        design.name(),
        c.name,
        detail(lo),
        detail(hi)
    )
}

/// Every case on `design`, one line each, then the boundary lines.
#[must_use]
pub fn run_all(design: Design, prod: &[u8]) -> String {
    let mut out = String::new();
    let cases = corpus(prod);
    for c in &cases {
        out.push_str(&run_case(design, c));
        out.push('\n');
    }
    for name in [
        "fuel_boundary_random_loop",
        "memory_fill_small_const_loop",
        "memory_fill_const_via_local",
    ] {
        if let Some(c) = cases.iter().find(|c| c.name == name) {
            out.push_str(&boundary(design, c));
            out.push('\n');
        }
    }
    out
}

/// `on_switch_edge(n, _)` recurses `n` deep; each frame keeps `locals`
/// i32 locals live across the call.
#[must_use]
pub fn recursion_module(locals: u32) -> Vec<u8> {
    let mut body = String::new();
    for i in 1..=locals {
        // Loaded values cannot be rematerialised after the call: they stay
        // live in the frame (spilled) across the recursion.
        let _ = write!(
            body,
            "(local.set {i} (i32.load offset={} (i32.const 0)))",
            (i % 8000) * 4
        );
    }
    body.push_str("(if (local.get 0) (then (call $r (i32.sub (local.get 0) (i32.const 1)))))");
    let mut sum = "(local.get 0)".to_owned();
    for i in 1..=locals {
        sum = format!("(i32.add {sum} (local.get {i}))");
    }
    let _ = write!(body, "(global.set $g {sum})");
    let locals_decl = if locals > 0 {
        format!("(local {})", "i32 ".repeat(locals as usize))
    } else {
        String::new()
    };
    wat(&format!(
        r#"(module (memory (export "memory") 1) (global $g (mut i32) (i32.const 0))
  (func $r (param i32) {locals_decl} {body})
  (func (export "on_init")) (func (export "on_tick"))
  (func (export "on_switch_edge") (param i32 i32) (call $r (local.get 0))))"#
    ))
}

/// Largest recursion depth that completes, per frame shape, by bisection
/// (fuel budget raised to 10^8 so only the stack limit decides); each
/// bisection is repeated `reps` times to show (non-)determinism.
#[must_use]
pub fn stack_probe(design: Design, reps: usize) -> String {
    let cfg = WasmConfig {
        memory_limit_bytes: 2 << 20,
        fuel_per_callback: 100_000_000,
    };
    let mut out = String::new();
    for locals in [0u32, 16, 64, 200, 1000] {
        let bytes = recursion_module(locals);
        let mut found = Vec::new();
        let mut kind = String::new();
        for _ in 0..reps {
            let mut run = |n: u16| -> Option<bool> {
                let mut b = load(design, &bytes, &NameTables::default(), cfg).ok()?;
                let view = HostView::default();
                let mut rng = Pcg32::seed_from_u64(1);
                let mut cmd = HostCommandQueue::default();
                b.on_switch_edge(
                    SwitchId(n),
                    SwitchEdge::Closed,
                    &mut HostCtx {
                        view: &view,
                        rng: &mut rng,
                        cmd: &mut cmd,
                    },
                );
                if b.is_faulted() {
                    kind = b.last_error().map_or_else(String::new, norm);
                }
                Some(!b.is_faulted())
            };
            let (mut lo, mut hi) = (0u16, u16::MAX);
            if run(hi) == Some(true) {
                found.push(u32::from(hi));
                continue;
            }
            while hi - lo > 1 {
                let mid = lo + (hi - lo) / 2;
                match run(mid) {
                    Some(true) => lo = mid,
                    _ => hi = mid,
                }
            }
            found.push(u32::from(lo));
        }
        let _ = writeln!(
            out,
            "{} stack locals={locals} max_ok_depth={found:?} fault_at_limit={kind}",
            design.name()
        );
    }
    out
}
