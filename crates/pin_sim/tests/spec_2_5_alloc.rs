//! Subphase 2.5 spec tests (Independent Tester): the allocation guard
//! (verification criterion 2, RULES 4 / D5) and the allocation bound of
//! `Recording::from_bytes`.
//!
//! A counting global allocator with thread-local counters: every heap
//! allocation made on the measuring thread inside `step_1ms` is counted. The
//! fixture scripts create no Lua / Wasm objects in their callbacks, so the
//! expected count is exactly zero (the Lua VM routes its allocations through
//! the global allocator too, which makes this the stricter check).

#[path = "fixtures/spec_2_5/support.rs"]
mod support;

use pin_core::input::InputEvent;
use pin_core::sim::SimStep;
use pin_core::snapshot::FrameStateSnapshot;
use pin_sim::Simulation;
use pin_sim::replay::Recording;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::Path;
use support::*;

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static BYTES: Cell<u64> = const { Cell::new(0) };
}

fn bump(size: usize) {
    let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
    let _ = BYTES.try_with(|c| c.set(c.get() + size as u64));
}

// SAFETY: forwards to the system allocator, only adding thread-local counters.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump(new_size);
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

fn bytes() -> u64 {
    BYTES.with(Cell::get)
}

#[test]
fn counter_detects_allocation() {
    let (a, b) = (allocs(), bytes());
    let v = std::hint::black_box(vec![1u8; 64]);
    assert!(allocs() > a);
    assert!(bytes() >= b + 64);
    drop(v);
}

/// Steps `ticks` ticks with the scripted inputs and returns
/// (allocations inside step_1ms, first tick that allocated).
/// Balls are re-injected through the test API between ticks (outside the
/// guarded region) so that there is always something to simulate.
fn guarded_run(
    sim: &mut Simulation,
    events: &[(u64, InputEvent)],
    ticks: u64,
    inject: bool,
) -> (u64, Option<u64>) {
    let mut out = Box::new(FrameStateSnapshot::default());
    let mut buf: [InputEvent; 8] = [button(1, pin_core::input::CabinetInput::Tilt, false); 8];
    let first_tick = sim.tick() + 1;
    let mut i = events.partition_point(|e| e.0 < first_tick);
    let mut total = 0u64;
    let mut first_bad = None;
    for tick in first_tick..first_tick + ticks {
        let mut n = 0;
        while i < events.len() && events[i].0 == tick {
            buf[n] = events[i].1;
            n += 1;
            i += 1;
        }
        if inject && tick % 2000 == 1 && sim.balls_in_play() < 3 {
            let k = (tick / 2000) as f32;
            let x = 0.08 + 0.35 * (k * 0.618).fract();
            sim.spawn_ball("standard_steel", [x, 0.98, R], [0.3 - 0.6 * (k * 0.37).fract(), -0.8, 0.0])
                .expect("inject ball");
        }
        let before = allocs();
        sim.step_1ms(tick, &buf[..n], &mut out);
        let d = allocs() - before;
        if d != 0 {
            total += d;
            first_bad.get_or_insert(tick);
        }
    }
    (total, first_bad)
}

fn guard_bundle(bundle: &Path, what: &str, warm_up: u64) {
    const TICKS: u64 = 60_000;
    let events = input_script(warm_up + TICKS, 11);
    let mut sim = load_at(bundle, DEFAULT_SEED);
    let (warm, _) = guarded_run(&mut sim, &events, warm_up, true);
    println!("{what}: {warm} allocations in step_1ms during the {warm_up}-tick warm-up");
    let b0 = bytes();
    let (n, first) = guarded_run(&mut sim, &events, TICKS, true);
    assert_eq!(sim.tick(), warm_up + TICKS);
    // The run did real work.
    assert!(sim.switch_hits("left_flipper").unwrap() > 100);
    assert!(sim.switch_hits("start").unwrap() >= 40);
    assert_eq!(
        n,
        0,
        "{what}: {n} heap allocations ({} bytes) inside step_1ms over {TICKS} ticks, first at tick {first:?}",
        bytes() - b0
    );
}

/// Verification criterion 2 (Lua): no allocation in `step_1ms` over 60,000
/// ticks with the scoring script loaded, balls in play, flippers firing and
/// script-spawned balls. A 3,000-tick warm-up covers every script path once
/// (the Lua VM may size its stack on first use).
#[test]
fn allocation_guard_60000_ticks_with_lua_script() {
    on_sim_thread(|| {
        guard_bundle(&bundle(), "lua", 3000);
        // The fixture script really ran and scored during such a run.
        let events = input_script(4000, 11);
        let sim = drive(&bundle(), DEFAULT_SEED, 4000, &events);
        assert!(sim.score(P_SCORE).unwrap() > 0);
    });
}

/// Rust engine code may not allocate at all in `step_1ms` (D5): with a rules
/// file that defines no callbacks the count is zero from the very first tick
/// (no warm-up), with balls, flippers and dispenser-less play.
#[test]
fn allocation_guard_from_tick_one_without_script_callbacks() {
    on_sim_thread(|| {
        let dir = bundle_with_script("nocallbacks", "-- no callbacks\n");
        let events = input_script(8000, 5);
        let mut sim = load_at(&dir, DEFAULT_SEED);
        sim.spawn_ball("standard_steel", [0.259, 0.9, R], [0.0, -0.5, 0.0])
            .unwrap();
        sim.spawn_ball("ceramic_powerball", [0.20, 0.25, R], [0.4, 0.0, 0.0])
            .unwrap();
        let (n, first) = guarded_run(&mut sim, &events, 8000, true);
        assert_eq!(
            n, 0,
            "{n} heap allocations inside step_1ms with no script callbacks, first at tick {first:?}"
        );
        assert!(sim.switch_hits("left_flipper").unwrap() > 5);
    });
}

const WASM_RULES: &str = r#"
;; Spec 2.5 Wasm rules: counts ticks and closed edges, spawns a ball on `start`.
(module
  (import "pinball" "set_score" (func $set_score (param i32 i64)))
  (import "pinball" "spawn_ball" (func $spawn_ball (param i32 i32)))
  (import "pinball" "pulse_coil" (func $pulse_coil (param i32 i32)))
  (import "pinball" "random" (func $random (param i64 i64) (result i64)))
  (import "pinball" "id_of" (func $id_of (param i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  (data (i32.const 16) "start")           ;; 5
  (data (i32.const 32) "feed")            ;; 4
  (data (i32.const 48) "standard_steel")  ;; 14
  (data (i32.const 64) "bumper_1_ring")   ;; 13
  (data (i32.const 80) "bumper_1_coil")   ;; 13
  (global $sw_start (mut i32) (i32.const -1))
  (global $sw_ring (mut i32) (i32.const -1))
  (global $coil (mut i32) (i32.const -1))
  (global $disp (mut i32) (i32.const -1))
  (global $prof (mut i32) (i32.const -1))
  (global $ticks (mut i64) (i64.const 0))
  (global $edges (mut i64) (i64.const 0))
  (global $rng (mut i64) (i64.const 0))
  (func (export "on_init")
    (global.set $sw_start (call $id_of (i32.const 0) (i32.const 16) (i32.const 5)))
    (global.set $sw_ring (call $id_of (i32.const 0) (i32.const 64) (i32.const 13)))
    (global.set $coil (call $id_of (i32.const 1) (i32.const 80) (i32.const 13)))
    (global.set $disp (call $id_of (i32.const 6) (i32.const 32) (i32.const 4)))
    (global.set $prof (call $id_of (i32.const 7) (i32.const 48) (i32.const 14))))
  (func (export "on_tick")
    (global.set $ticks (i64.add (global.get $ticks) (i64.const 1)))
    (call $set_score (i32.const 1) (global.get $ticks))
    (call $set_score (i32.const 2) (global.get $edges))
    (call $set_score (i32.const 3) (global.get $rng)))
  (func (export "on_switch_edge") (param $id i32) (param $closed i32)
    (if (i32.eqz (local.get $closed)) (then (return)))
    (global.set $edges (i64.add (global.get $edges) (i64.const 1)))
    (if (i32.eq (local.get $id) (global.get $sw_start))
      (then
        (call $spawn_ball (global.get $disp) (global.get $prof))
        (global.set $rng (i64.add (global.get $rng) (call $random (i64.const 1) (i64.const 1000000))))))
    (if (i32.eq (local.get $id) (global.get $sw_ring))
      (then (call $pulse_coil (global.get $coil) (i32.const 20)))))
)
"#;

/// Verification criterion 2 (Wasm): the same guard with a Wasm rules module.
/// Also checks that a `.wasm` script_entry runs at all (same host API).
#[test]
fn allocation_guard_60000_ticks_with_wasm_script() {
    on_sim_thread(|| {
        let dir = bundle_with_wasm("wasm_guard", WASM_RULES);
        // The Wasm rules run: tick counter, edges, RNG, script-spawned ball.
        let mut sim = load_at(&dir, DEFAULT_SEED);
        sim.advance(10);
        assert_eq!(sim.score(1), Some(10), "on_tick ran once per tick");
        sim.set_switch("start", true).unwrap();
        sim.advance(2);
        assert_eq!(sim.score(2), Some(1));
        assert!(sim.score(3).unwrap() >= 1);
        assert_eq!(sim.balls_in_play(), 1, "the Wasm rules spawned a ball");
        drop(sim);
        guard_bundle(&dir, "wasm", 3000);
    });
}

/// Lua and Wasm rules with the same logic drive the same simulation
/// (replay hash stability holds for both backends).
#[test]
fn wasm_run_is_deterministic() {
    on_sim_thread(|| {
        let dir = bundle_with_wasm("wasm_det", WASM_RULES);
        let events = input_script(3000, 2);
        let a = drive(&dir, 42, 3000, &events);
        let b = drive(&dir, 42, 3000, &events);
        assert_eq!(a.state_hash(), b.state_hash());
        assert_eq!(a.score(1), Some(3000));
        let rec = recording(a.table_hash(), 42, 3000, &events);
        assert_eq!(pin_sim::replay::replay(&dir, &rec).unwrap(), a.state_hash());
    });
}

/// `from_bytes` must bound its allocation by the input length: a header that
/// claims 2^64 events in a 200-byte file must not reserve for them.
#[test]
fn from_bytes_allocation_is_bounded_by_the_input_length() {
    let events = input_script(600, 1);
    let good = Recording {
        seed: 1,
        table_hash: 2,
        ticks: 600,
        events: events.into_iter().take(12).collect(),
    }
    .to_bytes();
    assert!(good.len() < 4096, "sample recording is {} bytes", good.len());
    let limit = |len: usize| 64 * len as u64 + 4096;
    let mut worst = 0u64;
    let mut check = |data: &[u8], what: &str| {
        let before = bytes();
        let r = Recording::from_bytes(data);
        let used = bytes() - before;
        drop(r);
        worst = worst.max(used);
        assert!(
            used <= limit(data.len()),
            "from_bytes allocated {used} bytes for a {}-byte input ({what})",
            data.len()
        );
    };
    check(&good, "valid file");
    for width in [1usize, 2, 4, 8] {
        for fill in [0xFFu8, 0x7F, 0x80, 0x10] {
            for i in 0..good.len() {
                let mut bad = good.clone();
                for b in bad.iter_mut().skip(i).take(width) {
                    *b = fill;
                }
                check(&bad, "overwritten field");
                // ... and the same cut right after the overwritten field.
                let cut = (i + width).min(bad.len());
                check(&bad[..cut], "overwritten field, truncated");
            }
        }
    }
    println!("from_bytes: worst case {worst} bytes allocated for a {}-byte input", good.len());
}
