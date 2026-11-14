//! Subphase 2.5 spec tests (Independent Tester): the allocation guard
//! (verification criterion 2, RULES 4 / D5) and the allocation bound of
//! `Recording::from_bytes`.
//!
//! A counting global allocator with thread-local counters: every heap
//! allocation made on the measuring thread inside `step_1ms` is counted and
//! classified by its call stack:
//!
//! * **engine** — no Lua VM allocator frame on the stack. RULES 4 / D5 allow
//!   none, from the very first tick.
//! * **Lua VM** — requested by the Lua VM through its capped allocator
//!   (`luaM_*` frames). RULES 4 exempts these; the fixture script creates no
//!   Lua objects in its callbacks, so after a warm-up (the VM sizes its stack
//!   on first use) the count is expected to be zero as well.

#[path = "fixtures/spec_2_5/support.rs"]
mod support;

use pin_core::input::InputEvent;
use pin_core::sim::SimStep;
use pin_core::snapshot::FrameStateSnapshot;
use pin_sim::Simulation;
use pin_sim::hash::fnv1a64;
use pin_sim::replay::{EVENT_BYTES, HEADER_BYTES, Recording, TRAILER_BYTES};
use std::alloc::{GlobalAlloc, Layout, System};
use std::backtrace::Backtrace;
use std::cell::{Cell, RefCell};
use std::path::Path;
use support::*;

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static BYTES: Cell<u64> = const { Cell::new(0) };
    /// True while the measuring thread is inside a guarded `step_1ms`.
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static ENGINE: Cell<u64> = const { Cell::new(0) };
    static LUA_VM: Cell<u64> = const { Cell::new(0) };
    static FIRST_ENGINE_TRACE: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn bump(size: usize) {
    let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
    let _ = BYTES.try_with(|c| c.set(c.get() + size as u64));
    // Classify allocations made inside a guarded tick. Disarmed while the
    // backtrace is captured (capturing allocates).
    if ARMED.try_with(|c| c.replace(false)).unwrap_or(false) {
        let trace = Backtrace::force_capture().to_string();
        if trace.contains("luaM_") {
            LUA_VM.with(|c| c.set(c.get() + 1));
        } else {
            ENGINE.with(|c| c.set(c.get() + 1));
            FIRST_ENGINE_TRACE.with(|t| {
                let mut t = t.borrow_mut();
                if t.is_none() {
                    *t = Some(format!("{size} bytes\n{trace}"));
                }
            });
        }
        ARMED.with(|c| c.set(true));
    }
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
    // The classifier: a plain Rust allocation inside an armed region is an
    // engine allocation.
    std::thread::spawn(|| {
        ARMED.with(|c| c.set(true));
        let v = std::hint::black_box(vec![2u8; 32]);
        ARMED.with(|c| c.set(false));
        drop(v);
        assert_eq!((ENGINE.with(Cell::get), LUA_VM.with(Cell::get)), (1, 0));
        assert!(FIRST_ENGINE_TRACE.with(|t| t.borrow().is_some()));
    })
    .join()
    .unwrap();
}

/// What a guarded run saw inside `step_1ms`.
#[derive(Debug, Default)]
struct Guarded {
    /// Allocations not made by the Lua VM (must be zero).
    engine: u64,
    /// Allocations made by the Lua VM allocator.
    lua_vm: u64,
    /// First tick with any allocation.
    first_tick: Option<u64>,
}

/// Steps `ticks` ticks with the scripted inputs.
/// Balls are re-injected through the test API between ticks (outside the
/// guarded region) so that there is always something to simulate.
fn guarded_run(
    sim: &mut Simulation,
    events: &[(u64, InputEvent)],
    ticks: u64,
    inject: bool,
) -> Guarded {
    let mut out = Box::new(FrameStateSnapshot::default());
    let mut buf: [InputEvent; 8] = [button(1, pin_core::input::CabinetInput::Tilt, false); 8];
    let first_tick = sim.tick() + 1;
    let mut i = events.partition_point(|e| e.0 < first_tick);
    let mut g = Guarded::default();
    let (e0, l0) = (ENGINE.with(Cell::get), LUA_VM.with(Cell::get));
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
            sim.spawn_ball(
                "standard_steel",
                [x, 0.98, R],
                [0.3 - 0.6 * (k * 0.37).fract(), -0.8, 0.0],
            )
            .expect("inject ball");
        }
        let before = allocs();
        ARMED.with(|c| c.set(true));
        sim.step_1ms(tick, &buf[..n], &mut out);
        ARMED.with(|c| c.set(false));
        if allocs() != before {
            g.first_tick.get_or_insert(tick);
        }
    }
    g.engine = ENGINE.with(Cell::get) - e0;
    g.lua_vm = LUA_VM.with(Cell::get) - l0;
    g
}

fn engine_trace() -> String {
    FIRST_ENGINE_TRACE.with(|t| t.borrow().clone().unwrap_or_default())
}

fn guard_bundle(bundle: &Path, what: &str, warm_up: u64) {
    const TICKS: u64 = 60_000;
    let events = input_script(warm_up + TICKS, 11);
    let mut sim = load_at(bundle, DEFAULT_SEED);
    let warm = guarded_run(&mut sim, &events, warm_up, true);
    println!("{what}: step_1ms during the {warm_up}-tick warm-up: {warm:?}");
    assert!(
        !sim.script_faulted(),
        "{what}: the script faulted during the warm-up"
    );
    assert_eq!(
        warm.engine,
        0,
        "{what}: {} engine (non-Lua-VM) heap allocations inside step_1ms during the first \
         {warm_up} ticks, first allocating tick {:?}; first one: {}",
        warm.engine,
        warm.first_tick,
        engine_trace()
    );
    let run = guarded_run(&mut sim, &events, TICKS, true);
    println!("{what}: step_1ms over the next {TICKS} ticks: {run:?}");
    assert!(
        !sim.script_faulted(),
        "{what}: the script faulted during the guarded run"
    );
    assert_eq!(sim.tick(), warm_up + TICKS);
    // The run did real work.
    assert!(sim.switch_hits("left_flipper").unwrap() > 100);
    assert!(sim.switch_hits("start").unwrap() >= 40);
    assert_eq!(
        run.engine,
        0,
        "{what}: {} engine heap allocations inside step_1ms over {TICKS} ticks, first \
         allocating tick {:?}; first one: {}",
        run.engine,
        run.first_tick,
        engine_trace()
    );
    assert_eq!(
        run.lua_vm, 0,
        "{what}: the Lua VM allocated {} times over {TICKS} steady-state ticks although the \
         script creates no objects (first allocating tick {:?})",
        run.lua_vm, run.first_tick
    );
}

/// Verification criterion 2 (Lua): no engine allocation in `step_1ms` from
/// tick 1, and no allocation at all over 60,000 ticks after a 3,000-tick
/// warm-up, with the scoring script loaded, balls in play, flippers firing
/// and script-spawned balls.
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
/// file that defines no callbacks the engine count is zero from the very
/// first tick (no warm-up), with balls and flippers in play. (The Lua VM's
/// own collector may still resize its stack once; that is the exemption.)
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
        let g = guarded_run(&mut sim, &events, 8000, true);
        assert_eq!(
            g.engine,
            0,
            "{} engine heap allocations inside step_1ms with no script callbacks, first \
             allocating tick {:?}; first one: {}",
            g.engine,
            g.first_tick,
            engine_trace()
        );
        assert!(g.lua_vm <= 4, "an idle Lua VM kept allocating: {g:?}");
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
        assert_eq!(sim.score(0), Some(10), "on_tick ran once per tick");
        let edges = sim.score(1).unwrap();
        sim.set_switch("start", true).unwrap();
        sim.advance(2);
        assert_eq!(
            sim.score(1),
            Some(edges + 1),
            "one more closed edge delivered"
        );
        assert!(sim.score(2).unwrap() >= 1);
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
        assert_eq!(a.score(0), Some(3000));
        let rec = recording(a.table_hash(), 42, 3000, &events);
        assert_eq!(pin_sim::replay::replay(&dir, &rec).unwrap(), a.state_hash());
    });
}

/// Recomputes the checksum trailer after editing the body (as in
/// `replay_format.rs`).
fn reseal(bytes: &mut [u8]) {
    if bytes.len() < TRAILER_BYTES {
        return;
    }
    let body = bytes.len() - TRAILER_BYTES;
    let sum = fnv1a64(&bytes[..body]);
    bytes[body..].copy_from_slice(&sum.to_le_bytes());
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
    assert!(
        good.len() < 4096,
        "sample recording is {} bytes",
        good.len()
    );
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
                // ... and resealed, so the edit gets past the checksum.
                reseal(&mut bad);
                check(&bad, "overwritten field, resealed");
            }
        }
    }
    // Declared event count overwritten, the body resized to match it and the
    // checksum recomputed: these pass the length and checksum checks and
    // reach event validation, which runs after the event vector is reserved.
    let n = (good.len() - HEADER_BYTES - TRAILER_BYTES) / EVENT_BYTES;
    for count in [0usize, 1, n - 1, n + 1, 2 * n, 64, 1000] {
        for pad in [0x00u8, 0xFF, 0x01] {
            let mut bad = good[..HEADER_BYTES].to_vec();
            bad[36..44].copy_from_slice(&(count as u64).to_le_bytes());
            let keep = count.min(n) * EVENT_BYTES;
            bad.extend_from_slice(&good[HEADER_BYTES..HEADER_BYTES + keep]);
            bad.resize(HEADER_BYTES + count * EVENT_BYTES, pad);
            bad.extend_from_slice(&[0; TRAILER_BYTES]);
            reseal(&mut bad);
            check(&bad, "event count overwritten, resized and resealed");
        }
        // Count overwritten and resealed without resizing the body.
        let mut bad = good.clone();
        bad[36..44].copy_from_slice(&(count as u64).to_le_bytes());
        reseal(&mut bad);
        check(&bad, "event count overwritten, resealed");
    }
    for count in [u64::MAX, u64::MAX / EVENT_BYTES as u64, 1 << 40] {
        let mut bad = good.clone();
        bad[36..44].copy_from_slice(&count.to_le_bytes());
        reseal(&mut bad);
        check(&bad, "huge event count, resealed");
    }
    println!(
        "from_bytes: worst case {worst} bytes allocated for a {}-byte input",
        good.len()
    );
}
