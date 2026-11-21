//! Worst-case cost of one callback a hostile module can force: each module
//! loops in `on_tick` until the engine budget (100 000 fuel) runs out. Also
//! module compile/instantiate times of the production modules.

use crate::run::{Design, load};
use pin_core::{HostCommandQueue, HostCtx, HostView, Pcg32, SeedableRng};
use pin_script::NameTables;
use pin_script::wasm::WasmConfig;
use std::fmt::Write as _;

const IMPORTS: &str = r#"
  (import "pinball" "tick" (func $tick (result i64)))
  (import "pinball" "random" (func $random (param i64 i64) (result i64)))
  (import "pinball" "id_of" (func $id_of (param i32 i32 i32) (result i32)))
  (import "pinball" "hold_coil" (func $hold (param i32 i32)))
  (import "pinball" "set_lamp" (func $lamp (param i32 f32)))
"#;

fn module(extra: &str, body: &str) -> Vec<u8> {
    let w = format!(
        r#"(module {IMPORTS} (memory (export "memory") 32) {extra}
  (func (export "on_init")) (func (export "on_switch_edge") (param i32 i32))
  (func (export "on_tick") (local $i i32) (loop $l {body} (br $l))))"#
    );
    wat::parse_str(&w).unwrap_or_else(|e| panic!("{e}"))
}

/// The hostile loop bodies.
#[must_use]
pub fn cases() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("loop_br_only", module("", "")),
        (
            "loop_arith",
            module("", "(local.set $i (i32.add (local.get $i) (i32.const 1)))"),
        ),
        ("loop_call_empty", module("(func $f)", "(call $f)")),
        (
            "loop_call_indirect",
            module(
                "(type $v (func)) (func $f) (table 1 funcref) (elem (i32.const 0) $f)",
                "(call_indirect (type $v) (i32.const 0))",
            ),
        ),
        ("loop_host_tick", module("", "(drop (call $tick))")),
        (
            "loop_host_random",
            module(
                "",
                "(drop (call $random (i64.const 0) (i64.const 1099511627776)))",
            ),
        ),
        (
            "loop_host_id_of_len0",
            module(
                "",
                "(drop (call $id_of (i32.const 0) (i32.const 0) (i32.const 0)))",
            ),
        ),
        (
            "loop_host_hold_coil_overflow",
            module("", "(call $hold (i32.const 7) (i32.const 1))"),
        ),
        (
            "loop_host_set_lamp_overflow",
            module("", "(call $lamp (i32.const 3) (f32.const 0.5))"),
        ),
        (
            "loop_memory_grow0",
            module("", "(drop (memory.grow (i32.const 0)))"),
        ),
        ("loop_memory_size", module("", "(drop (memory.size))")),
        (
            "loop_memory_fill_128",
            module(
                "",
                "(memory.fill (i32.const 0) (i32.const 1) (i32.const 128))",
            ),
        ),
        (
            "loop_memory_copy_64k",
            module(
                "",
                "(memory.copy (i32.const 0) (i32.const 65536) (i32.const 65536))",
            ),
        ),
        // Zero-fuel operators (wasmtime charges nop/block/end nothing): the
        // largest floods the D25 pre-scan still accepts in one loop body
        // (60 000 nops / 4 000 blocks are refused by its loop-work and
        // branch-work budgets).
        ("loop_nop_flood_60k", module("", &"nop ".repeat(60_000))),
        ("loop_nop_flood_23k", module("", &"nop ".repeat(23_000))),
        (
            "loop_block_flood_4000",
            module("", &"(block) ".repeat(4_000)),
        ),
        (
            "loop_block_flood_1400",
            module("", &"(block) ".repeat(1_400)),
        ),
        (
            "loop_load_store",
            module(
                "",
                "(i32.store (local.get $i) (i32.add (i32.load (local.get $i)) (i32.const 1)))",
            ),
        ),
    ]
}

fn stats(v: &mut [f64]) -> (f64, f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[0], v[v.len() / 2], v[v.len() - 1])
}

/// Times `reps` budget-exhausting `on_tick` calls per case on `design`
/// (fresh instance each); one line per case: min/median/max µs, fuel and
/// the fault.
#[must_use]
pub fn run(design: Design, clock: fn() -> f64, reps: usize, only: Option<&str>) -> String {
    let cfg = WasmConfig::engine();
    let mut out = String::new();
    for (name, bytes) in cases() {
        if only.is_some_and(|o| o != name) {
            continue;
        }
        let mut times = Vec::new();
        let mut info = String::new();
        for _ in 0..reps {
            let mut b = match load(design, &bytes, &NameTables::default(), cfg) {
                Ok(b) => b,
                Err(e) => {
                    info = format!("load error {e}");
                    break;
                }
            };
            let view = HostView::default();
            let mut rng = Pcg32::seed_from_u64(1);
            let mut cmd = HostCommandQueue::default();
            let t0 = clock();
            b.on_tick(&mut HostCtx {
                view: &view,
                rng: &mut rng,
                cmd: &mut cmd,
            });
            let t1 = clock();
            times.push((t1 - t0) * 1e3);
            info = format!(
                "fault={} fuel={}",
                b.last_error()
                    .map_or_else(|| "none".into(), crate::adversarial::norm),
                b.last_fuel()
            );
        }
        if times.is_empty() {
            let _ = writeln!(out, "{} {name} {info}", design.name());
            continue;
        }
        let (mn, md, mx) = stats(&mut times);
        let _ = writeln!(
            out,
            "{} {name} min_us={mn:.1} median_us={md:.1} max_us={mx:.1} {info}",
            design.name()
        );
    }
    out
}

/// Compile + instantiate time of `src` on `design`, `reps` times (ms).
#[must_use]
pub fn load_time(
    design: Design,
    src: &[u8],
    names: &NameTables,
    clock: fn() -> f64,
    reps: usize,
) -> String {
    let cfg = WasmConfig::engine();
    let mut v = Vec::new();
    for _ in 0..reps {
        let t0 = clock();
        let r = load(design, src, names, cfg);
        let t1 = clock();
        if let Err(e) = r {
            return format!("{} load error {e}", design.name());
        }
        v.push(t1 - t0);
    }
    let (mn, md, mx) = stats(&mut v);
    format!(
        "{} bytes={} load_ms min={mn:.3} median={md:.3} max={mx:.3}",
        design.name(),
        src.len()
    )
}
