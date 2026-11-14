//! Spec (4.5 criteria 2, 3, 6): fuel exhaustion is contained, the memory cap
//! holds at load and at `memory.grow`, hostile modules fail cleanly at load,
//! and random bytes never panic the loader.

#[path = "spec_fixtures/wasm/support.rs"]
mod support;

use pin_core::{CoilId, SceneId, ScriptError};
use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use proptest::prelude::*;
use support::{Driver, Host, IMPORTS, cfg, guest, module, names, op, watchdog};

const PAGE: usize = 65536;
const MIB: usize = 1024 * 1024;

fn load(src: &str, cfg: WasmConfig) -> Result<WasmScriptBackend, ScriptError> {
    WasmScriptBackend::new(src.as_bytes(), &names(), cfg)
}

/// `new` on a helper thread: a looping start function must not hang the suite.
fn load_guarded(what: &str, src: String, cfg: WasmConfig) -> Result<(), ScriptError> {
    watchdog(what, move || {
        WasmScriptBackend::new(src.as_bytes(), &names(), cfg).map(drop)
    })
}

fn assert_rejected(what: &str, src: &str) {
    for c in [WasmConfig::default(), WasmConfig::engine()] {
        match load_guarded(what, src.to_owned(), c) {
            Ok(()) => panic!("{what}: a hostile module was accepted"),
            Err(e) => assert!(!e.to_string().is_empty()),
        }
    }
}

const CALLBACKS: &str = r#"
  (func (export "on_init"))
  (func (export "on_tick"))
  (func (export "on_switch_edge") (param i32 i32))
"#;

// ------------------------------------------------------------------ fuel

/// Runs `op` on a driver under `cfg` behind the hang guard.
fn spin(cfg: WasmConfig, opcode: u16) -> (Driver, pin_core::HostCommandQueue) {
    watchdog("fuel exhaustion", move || {
        let mut d = Driver::with_cfg(cfg);
        // Tick 1: hold two coils and behave.
        d.call(op::HOLD_COIL, &[21, 1]);
        d.call(op::HOLD_COIL, &[22, 1]);
        d.call(op::HOLD_COIL, &[22, 0]);
        d.call(op::HOLD_COIL, &[23, 1]);
        // Tick 2: an earlier callback queues a pulse, then the guest spins
        // after queueing the marker commands.
        d.h.cmd.clear();
        d.call_keep(op::PULSE_COIL, &[1, 10]);
        d.h.view.persist[0] = 0;
        d.h.edge_keep(&mut d.b, opcode, false);
        let q = d.h.cmd.clone();
        (d, q)
    })
}

#[test]
fn an_infinite_loop_is_contained_by_fuel() {
    for c in [
        WasmConfig::engine(),
        WasmConfig::default(),
        cfg(16 * MIB, 5000),
    ] {
        for opcode in [op::SPIN, op::SPIN_CALLING_HOST] {
            let (mut d, q) = spin(c, opcode);
            assert!(d.b.is_faulted(), "{c:?} op {opcode}");
            assert_eq!(d.b.fault_count(), 1);
            assert_eq!(
                d.b.last_error(),
                Some(&ScriptError::BudgetExceeded),
                "fuel exhaustion reports BudgetExceeded ({c:?}, op {opcode})"
            );
            // The earlier callback's pulse stays; the spinning callback's
            // markers and host calls are rolled back; held coils are released
            // (21, 23, and 8 from the rolled-back marker hold; not 22).
            assert_eq!(q.coil_pulses.as_slice(), &[(CoilId(1), 10)], "op {opcode}");
            assert!(q.lamps.is_empty() && q.scores.is_empty() && q.persist_writes.is_empty());
            assert_eq!(q.gi_tint, None);
            let mut rel: Vec<_> = q.coil_holds.iter().copied().collect();
            rel.sort_by_key(|&(c, _)| c.0);
            assert_eq!(
                rel,
                vec![(CoilId(8), false), (CoilId(21), false), (CoilId(23), false)]
            );
            assert_eq!(q.len(), 4);
            assert_eq!(
                d.b.overflow_count(),
                0,
                "rolled-back overflows are not counted"
            );
            // Later ticks and edges are no-ops.
            for _ in 0..20 {
                assert!(d.tick().is_empty());
                assert!(d.call(op::SPIN, &[]).is_empty());
                assert!(d.call(op::PULSE_COIL, &[1, 1]).is_empty());
            }
            assert_eq!(d.b.fault_count(), 1);
        }
    }
}

#[test]
fn an_infinite_loop_in_on_tick_and_on_init_is_contained() {
    let tick_spin = guest(
        "(call $hold_coil (i32.const 3) (i32.const 1))",
        "(call $pulse_coil (i32.const 1) (i32.const 1)) (loop $l (br $l))",
        "(call $show_scene (i32.const 2))",
    );
    let init_spin = guest(
        "(call $pulse_coil (i32.const 1) (i32.const 1)) (loop $l (br $l))",
        "(call $show_scene (i32.const 7))",
        "(call $show_scene (i32.const 2))",
    );
    let (b, q1, q2, q3) = watchdog("on_tick spin", move || {
        let mut b = support::backend_with(&tick_spin, WasmConfig::engine());
        let mut h = Host::new(1);
        h.init(&mut b);
        assert_eq!(h.cmd.coil_holds.as_slice(), &[(CoilId(3), true)]);
        h.edge(&mut b, 4, true);
        assert_eq!(h.cmd.display_scenes.as_slice(), &[SceneId(2)]);
        h.tick(&mut b);
        let q1 = h.cmd.clone();
        h.tick(&mut b);
        let q2 = h.cmd.clone();
        h.edge(&mut b, 4, true);
        let q3 = h.cmd.clone();
        (b, q1, q2, q3)
    });
    assert!(b.is_faulted());
    assert_eq!(b.last_error(), Some(&ScriptError::BudgetExceeded));
    assert_eq!(q1.coil_holds.as_slice(), &[(CoilId(3), false)]);
    assert_eq!(
        q1.len(),
        1,
        "the faulting tick's pulse is rolled back: {q1:?}"
    );
    assert!(q2.is_empty() && q3.is_empty());
    assert_eq!(b.fault_count(), 1);

    let (b, q1, q2) = watchdog("on_init spin", move || {
        let mut b = support::backend_with(&init_spin, WasmConfig::engine());
        let mut h = Host::new(1);
        h.init(&mut b);
        let q1 = h.cmd.clone();
        h.tick(&mut b);
        (b, q1, h.cmd.clone())
    });
    assert!(b.is_faulted());
    assert_eq!(b.last_error(), Some(&ScriptError::BudgetExceeded));
    assert!(q1.is_empty() && q2.is_empty());
}

#[test]
fn fuel_is_a_per_callback_budget_not_a_lifetime_budget() {
    let (d, ok) = watchdog("per-callback fuel", || {
        let mut d = Driver::with_cfg(WasmConfig::engine());
        let mut ok = 0u32;
        // 1000 loop iterations per callback are far below 100 000 fuel, but
        // 600 such callbacks are far above it in total.
        for _ in 0..600 {
            if d.value(op::COUNT_LOOP, &[1000]) == Some(1) {
                ok += 1;
            }
            d.tick();
        }
        (d, ok)
    });
    assert!(!d.b.is_faulted(), "{:?}", d.b.last_error());
    assert_eq!(ok, 600);

    // The same guest with a loop far above the budget faults.
    let d = watchdog("over budget", || {
        let mut d = Driver::with_cfg(WasmConfig::engine());
        assert_eq!(d.value(op::COUNT_LOOP, &[1_000_000]), None);
        d
    });
    assert!(d.b.is_faulted());
    assert_eq!(d.b.last_error(), Some(&ScriptError::BudgetExceeded));
}

#[test]
fn the_budget_scales_with_the_configured_fuel() {
    // 20 000 iterations (>= 3 instructions each) cannot fit in 10 000 fuel but
    // fit easily in 10 000 000.
    let (small, big) = watchdog("fuel scaling", || {
        let mut s = Driver::with_cfg(cfg(16 * MIB, 10_000));
        let a = s.value(op::COUNT_LOOP, &[20_000]);
        let mut b = Driver::with_cfg(cfg(16 * MIB, 10_000_000));
        let v = b.value(op::COUNT_LOOP, &[20_000]);
        ((s.b.is_faulted(), a), (b.b.is_faulted(), v))
    });
    assert_eq!(small, (true, None));
    assert_eq!(big, (false, Some(1)));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]
    /// Whatever the budget, a spinning guest is contained and tilts.
    #[test]
    fn any_fuel_budget_contains_a_spinning_guest(fuel in 1u64..300_000, host_calls in any::<bool>()) {
        let opcode = if host_calls { op::SPIN_CALLING_HOST } else { op::SPIN };
        let (faulted, q, err) = watchdog("spin proptest", move || {
            let mut d = Driver::with_cfg(cfg(16 * MIB, fuel));
            let q = d.call(opcode, &[]).clone();
            (d.b.is_faulted(), q, d.b.last_error().cloned())
        });
        prop_assert!(faulted);
        prop_assert!(q.is_empty());
        prop_assert_eq!(err, Some(ScriptError::BudgetExceeded));
    }
}

/// Criterion 2 timing ("the faulting tick takes <= 8 ms with
/// WasmConfig::engine() in release"). Rule 5 forbids wall-clock assertions in
/// the normal suite, so this is ignored by default and only prints; run with
/// `cargo test -p pin_script --release --test spec_wasm_sandbox -- --ignored --nocapture`.
#[test]
#[ignore = "prints wall-clock numbers for the report; run in release"]
fn report_faulting_tick_duration() {
    let mut worst = std::time::Duration::ZERO;
    for opcode in [op::SPIN, op::SPIN_CALLING_HOST, op::RECURSE] {
        for _ in 0..50 {
            let mut d = Driver::with_cfg(WasmConfig::engine());
            d.call(op::HOLD_RANGE, &[100, 32]);
            d.h.cmd.clear();
            let t = std::time::Instant::now();
            d.h.edge_keep(&mut d.b, opcode, false);
            let dt = t.elapsed();
            assert!(d.b.is_faulted());
            worst = worst.max(dt);
        }
    }
    println!("worst faulting callback with WasmConfig::engine(): {worst:?}");
    if !cfg!(debug_assertions) {
        assert!(
            worst <= std::time::Duration::from_millis(8),
            "faulting tick took {worst:?} (> 8 ms) in release"
        );
    }
}

// ---------------------------------------------------------------- memory

/// A guest with the given memory declaration whose `on_tick` reports
/// `memory.grow(persist[0])` through `set_score(1, _)` and whose
/// `on_switch_edge` reports `memory.size`.
fn grow_guest(mem: &str) -> String {
    module(&format!(
        r#"{mem}
        (func (export "on_init"))
        (func (export "on_tick")
          (call $set_score (i32.const 1)
            (i64.extend_i32_s (memory.grow (i32.wrap_i64 (call $persist_get (i32.const 0)))))))
        (func (export "on_switch_edge") (param i32 i32)
          (call $set_score (i32.const 1) (i64.extend_i32_s (memory.size))))"#
    ))
}

struct Grower {
    b: WasmScriptBackend,
    h: Host,
}

impl Grower {
    fn new(mem: &str, c: WasmConfig) -> Self {
        Self {
            b: support::backend_with(&grow_guest(mem), c),
            h: Host::new(0),
        }
    }
    /// `memory.grow(delta)`: Some(result) or None when the call faulted.
    fn grow(&mut self, delta: i32) -> Option<i64> {
        self.h.view.persist[0] = i64::from(delta);
        self.h.tick(&mut self.b);
        self.h.cmd.scores.first().map(|&(_, v)| v)
    }
    fn pages(&mut self) -> Option<i64> {
        self.h.edge(&mut self.b, 0, true);
        self.h.cmd.scores.first().map(|&(_, v)| v)
    }
}

const MEM1: &str = r#"(memory (export "memory") 1)"#;

/// A grow past the cap must fail: -1 to the guest, or a trap that faults.
fn assert_grow_refused(g: &mut Grower, delta: i32, pages_before: i64) {
    let cap = g.b.config().memory_limit_bytes;
    let r = g.grow(delta);
    match r {
        Some(-1) => {
            assert!(!g.b.is_faulted());
            assert_eq!(
                g.pages(),
                Some(pages_before),
                "a refused grow must not change the size"
            );
        }
        None => assert!(g.b.is_faulted(), "no result but not faulted"),
        Some(v) => panic!("memory.grow({delta}) past the {cap}-byte cap returned {v}"),
    }
    assert!(
        g.b.memory_size() <= cap,
        "memory_size {} > cap {cap}",
        g.b.memory_size()
    );
}

#[test]
fn an_initial_memory_above_the_cap_is_rejected_at_load() {
    let e = WasmConfig::engine(); // 2 MiB = 32 pages
    assert!(load(&grow_guest(r#"(memory (export "memory") 33)"#), e).is_err());
    assert!(load(&grow_guest(r#"(memory (export "memory") 33 64)"#), e).is_err());
    assert!(load(&grow_guest(r#"(memory (export "memory") 1000)"#), e).is_err());
    assert!(load(&grow_guest(r#"(memory (export "memory") 65536)"#), e).is_err());
    assert!(
        load(
            &grow_guest(r#"(memory (export "memory") 257)"#),
            WasmConfig::default()
        )
        .is_err()
    );
    // A cap that is not a whole number of pages.
    assert!(
        load(
            &grow_guest(r#"(memory (export "memory") 2)"#),
            cfg(100_000, 1_000_000)
        )
        .is_err()
    );
    assert!(load(&grow_guest(MEM1), cfg(PAGE - 1, 1_000_000)).is_err());
    assert!(load(&grow_guest(MEM1), cfg(0, 1_000_000)).is_err());

    // At or below the cap loads.
    let b = load(&grow_guest(r#"(memory (export "memory") 32)"#), e).unwrap();
    assert_eq!(b.memory_size(), 2 * MIB);
    let b = load(
        &grow_guest(r#"(memory (export "memory") 256)"#),
        WasmConfig::default(),
    )
    .unwrap();
    assert_eq!(b.memory_size(), 16 * MIB);
    let b = load(&grow_guest(MEM1), cfg(100_000, 1_000_000)).unwrap();
    assert_eq!(b.memory_size(), PAGE);
    let b = load(&grow_guest(MEM1), cfg(PAGE, 1_000_000)).unwrap();
    assert_eq!(b.memory_size(), PAGE);
    // A declared maximum above the cap is fine: only growth is limited.
    let b = load(&grow_guest(r#"(memory (export "memory") 1 65536)"#), e).unwrap();
    assert_eq!(b.memory_size(), PAGE);
}

#[test]
fn memory_grow_works_up_to_the_cap_and_fails_past_it() {
    let mut g = Grower::new(MEM1, WasmConfig::engine());
    assert_eq!(g.b.memory_size(), PAGE);
    assert_eq!(g.pages(), Some(1));
    assert_eq!(g.grow(0), Some(1));
    assert_eq!(g.grow(1), Some(1));
    assert_eq!(g.b.memory_size(), 2 * PAGE);
    assert_eq!(g.grow(30), Some(2));
    assert_eq!(g.b.memory_size(), 32 * PAGE);
    assert_eq!(g.pages(), Some(32));
    assert!(!g.b.is_faulted());
    assert_grow_refused(&mut g, 1, 32);

    for delta in [32, 1000, 65535, 65536, i32::MAX, -1, i32::MIN] {
        let mut g = Grower::new(MEM1, WasmConfig::engine());
        assert_grow_refused(&mut g, delta, 1);
    }

    // A cap between page multiples.
    let mut g = Grower::new(MEM1, cfg(3 * PAGE + 5, 1_000_000));
    assert_eq!(g.grow(2), Some(1));
    assert_grow_refused(&mut g, 1, 3);

    // The module's own maximum still applies below the cap (plain Wasm: -1).
    let mut g = Grower::new(r#"(memory (export "memory") 1 2)"#, WasmConfig::default());
    assert_eq!(g.grow(1), Some(1));
    assert_grow_refused(&mut g, 1, 2);
}

#[test]
fn grown_memory_is_usable_and_the_old_bound_moves() {
    let mut d = Driver::with_cfg(WasmConfig::engine());
    d.call(op::OOB_STORE, &[65528]);
    assert_eq!(d.value(op::MEMORY_SIZE, &[]), Some(1));
    assert_eq!(d.value(op::MEMORY_GROW, &[1]), Some(1));
    d.call(op::OOB_STORE, &[65536]);
    assert_eq!(d.value(op::OOB_LOAD, &[65536]), Some(-1));
    assert_eq!(d.value(op::OOB_LOAD, &[2 * 65536 - 8]), Some(0));
    assert!(!d.b.is_faulted());
    assert_eq!(d.b.memory_size(), 2 * PAGE);
    d.call(op::OOB_LOAD, &[2 * 65536 - 7]);
    assert!(d.b.is_faulted());
}

#[test]
fn a_guest_that_grows_forever_never_exceeds_the_cap() {
    let src = guest(
        "",
        "(block $out (loop $l
           (br_if $out (i32.eq (memory.grow (i32.const 1)) (i32.const -1)))
           (br $l)))
         (call $set_score (i32.const 1) (i64.extend_i32_s (memory.size)))",
        "",
    );
    for c in [
        WasmConfig::engine(),
        cfg(5 * PAGE + 1, 1_000_000),
        WasmConfig::default(),
    ] {
        let cap = c.memory_limit_bytes;
        let s = src.clone();
        let (b, scores) = watchdog("grow forever", move || {
            let mut b = support::backend_with(&s, c);
            let mut h = Host::new(0);
            let mut scores = Vec::new();
            for _ in 0..5 {
                h.tick(&mut b);
                scores.push(h.cmd.scores.first().map(|&(_, v)| v));
                assert!(b.memory_size() <= cap);
            }
            (b, scores)
        });
        assert!(b.memory_size() <= cap, "{} > {cap}", b.memory_size());
        if !b.is_faulted() {
            // grow returned -1 at the cap: the guest saw exactly cap/64KiB pages.
            let want = Some((cap / PAGE) as i64);
            assert_eq!(scores, vec![want; 5]);
            assert_eq!(b.memory_size(), (cap / PAGE) * PAGE);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    /// Model check: a grow succeeds iff the result fits the cap; the size never
    /// exceeds the cap.
    #[test]
    fn memory_grow_follows_the_cap_model(
        cap_pages in 1usize..40, slack in 0usize..PAGE,
        deltas in proptest::collection::vec(prop_oneof![0i32..4, 0i32..50, any::<i32>()], 1..20)
    ) {
        let cap = cap_pages * PAGE + slack;
        let mut g = Grower::new(MEM1, cfg(cap, 1_000_000));
        let mut pages = 1i64;
        for d in deltas {
            let r = g.grow(d);
            prop_assert!(g.b.memory_size() <= cap);
            if g.b.is_faulted() {
                // "traps -> fault" flavour: only legal for a grow past the cap.
                prop_assert!(d < 0 || pages + i64::from(d) > cap_pages as i64);
                break;
            }
            if d >= 0 && pages + i64::from(d) <= cap_pages as i64 {
                prop_assert_eq!(r, Some(pages));
                pages += i64::from(d);
            } else {
                prop_assert_eq!(r, Some(-1));
            }
            prop_assert_eq!(g.b.memory_size(), pages as usize * PAGE);
            prop_assert_eq!(g.pages(), Some(pages));
        }
    }
}

#[test]
fn tables_cannot_be_grown_without_bound() {
    // 50 million funcref slots would be hundreds of MiB of host memory outside
    // the guest's memory cap.
    let src = module(
        r#"(memory (export "memory") 1)
           (table $t 1 funcref)
           (func (export "on_init"))
           (func (export "on_tick")
             (call $set_score (i32.const 1)
               (i64.extend_i32_s (table.grow $t (ref.null func) (i32.const 50000000)))))
           (func (export "on_switch_edge") (param i32 i32))"#,
    );
    for c in [WasmConfig::engine(), WasmConfig::default()] {
        let mut b = match load(&src, c) {
            Ok(b) => b,
            Err(_) => continue, // rejecting growable tables outright is also clean
        };
        let mut h = Host::new(0);
        h.tick(&mut b);
        let r = h.cmd.scores.first().map(|&(_, v)| v);
        assert!(
            b.is_faulted() || r == Some(-1),
            "table.grow by 50M entries returned {r:?}"
        );
    }
}

// -------------------------------------------------------------- hostile

#[test]
fn wasi_and_unknown_imports_are_rejected() {
    let mem = r#"(memory (export "memory") 1)"#;
    for (what, import) in [
        (
            "WASI fd_write",
            r#"(import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))"#,
        ),
        (
            "WASI proc_exit",
            r#"(import "wasi_snapshot_preview1" "proc_exit" (func (param i32)))"#,
        ),
        (
            "WASI clock",
            r#"(import "wasi_snapshot_preview1" "clock_time_get" (func (param i32 i64 i32) (result i32)))"#,
        ),
        (
            "WASI unstable",
            r#"(import "wasi_unstable" "random_get" (func (param i32 i32) (result i32)))"#,
        ),
        ("env import", r#"(import "env" "abort" (func))"#),
        (
            "unknown pinball function",
            r#"(import "pinball" "format_disk" (func))"#,
        ),
        (
            "case-changed name",
            r#"(import "pinball" "Pulse_Coil" (func (param i32 i32)))"#,
        ),
        (
            "empty module name",
            r#"(import "" "pulse_coil" (func (param i32 i32)))"#,
        ),
        (
            "wrong arity",
            r#"(import "pinball" "pulse_coil" (func (param i32)))"#,
        ),
        (
            "wrong param type",
            r#"(import "pinball" "pulse_coil" (func (param i64 i32)))"#,
        ),
        (
            "wrong f32/f64",
            r#"(import "pinball" "set_lamp" (func (param i32 f64)))"#,
        ),
        (
            "wrong result",
            r#"(import "pinball" "tick" (func (result i32)))"#,
        ),
        (
            "extra result",
            r#"(import "pinball" "show_scene" (func (param i32) (result i32)))"#,
        ),
        (
            "missing result",
            r#"(import "pinball" "persist_get" (func (param i32)))"#,
        ),
        (
            "imported memory",
            r#"(import "pinball" "memory" (memory 1))"#,
        ),
        (
            "imported global",
            r#"(import "pinball" "tick" (global i64))"#,
        ),
        (
            "imported table",
            r#"(import "pinball" "table" (table 1 funcref))"#,
        ),
        (
            "imported env memory",
            r#"(import "env" "memory" (memory 1))"#,
        ),
    ] {
        let mem = if import.contains("(memory 1))") {
            ""
        } else {
            mem
        };
        let src = format!("(module {import} {mem} {CALLBACKS})");
        assert_rejected(what, &src);
    }
    // Control: the same skeleton without the bad import loads.
    load(
        &format!("(module {mem} {CALLBACKS})"),
        WasmConfig::default(),
    )
    .unwrap();
    load(
        &format!("(module {IMPORTS} {mem} {CALLBACKS})"),
        WasmConfig::engine(),
    )
    .unwrap();
}

#[test]
fn missing_or_mistyped_exports_are_rejected() {
    let mem = r#"(memory (export "memory") 1)"#;
    let init = r#"(func (export "on_init"))"#;
    let tick = r#"(func (export "on_tick"))"#;
    let edge = r#"(func (export "on_switch_edge") (param i32 i32))"#;
    load(
        &format!("(module {mem} {init} {tick} {edge})"),
        WasmConfig::default(),
    )
    .unwrap();
    for (what, body) in [
        ("empty module", String::new()),
        ("no on_init", format!("{mem} {tick} {edge}")),
        ("no on_tick", format!("{mem} {init} {edge}")),
        ("no on_switch_edge", format!("{mem} {init} {tick}")),
        (
            "no memory export",
            format!("(memory 1) {init} {tick} {edge}"),
        ),
        ("no memory at all", format!("{init} {tick} {edge}")),
        (
            "lua-style on_switch name",
            format!(r#"{mem} {init} {tick} (func (export "on_switch") (param i32 i32))"#),
        ),
        (
            "on_tick takes a param",
            format!(r#"{mem} {init} {edge} (func (export "on_tick") (param i32))"#),
        ),
        (
            "on_tick returns a value",
            format!(r#"{mem} {init} {edge} (func (export "on_tick") (result i32) (i32.const 0))"#),
        ),
        (
            "on_init takes a param",
            format!(r#"{mem} {tick} {edge} (func (export "on_init") (param i64))"#),
        ),
        (
            "on_switch_edge with one param",
            format!(r#"{mem} {init} {tick} (func (export "on_switch_edge") (param i32))"#),
        ),
        (
            "on_switch_edge with i64 id",
            format!(r#"{mem} {init} {tick} (func (export "on_switch_edge") (param i64 i32))"#),
        ),
        (
            "on_switch_edge with a result",
            format!(
                r#"{mem} {init} {tick} (func (export "on_switch_edge") (param i32 i32) (result i32) (i32.const 0))"#
            ),
        ),
        (
            "on_tick is a global",
            format!(r#"{mem} {init} {edge} (global (export "on_tick") i32 (i32.const 0))"#),
        ),
        (
            "memory is a function",
            format!(r#"(memory 1) {init} {tick} {edge} (func (export "memory"))"#),
        ),
        (
            "memory is a global",
            format!(r#"{init} {tick} {edge} (global (export "memory") i32 (i32.const 0))"#),
        ),
    ] {
        assert_rejected(what, &format!("(module {body})"));
    }
}

#[test]
fn start_functions_cannot_run_away() {
    let mem = r#"(memory (export "memory") 1)"#;
    for (what, start) in [
        (
            "start loops forever",
            "(func $s (loop $l (br $l))) (start $s)".to_owned(),
        ),
        (
            "start traps",
            "(func $s (unreachable)) (start $s)".to_owned(),
        ),
        (
            "start grows memory forever",
            "(func $s (loop $l (drop (memory.grow (i32.const 1))) (br $l))) (start $s)".to_owned(),
        ),
        (
            "start recurses forever",
            "(func $s (call $s)) (start $s)".to_owned(),
        ),
    ] {
        assert_rejected(what, &format!("(module {mem} {CALLBACKS} {start})"));
    }
    // The host API has no context at load time: a start function using it
    // must not reach a queue.
    assert_rejected(
        "start calls the host API",
        &format!(
            r#"(module (import "pinball" "pulse_coil" (func $p (param i32 i32)))
                 (import "pinball" "random" (func $r (param i64 i64) (result i64)))
                 {mem} {CALLBACKS}
                 (func $s (call $p (i32.const 1) (i32.const 1))
                          (drop (call $r (i64.const 1) (i64.const 6))))
                 (start $s))"#
        ),
    );
}

#[test]
fn oversized_or_exotic_resources_are_rejected() {
    let cb = CALLBACKS;
    for (what, src) in [
        (
            "second, huge, unexported memory",
            format!(r#"(module (memory (export "memory") 1) (memory $big 60000) {cb})"#),
        ),
        (
            "shared memory (threads)",
            format!(r#"(module (memory (export "memory") 1 1 shared) {cb})"#),
        ),
        (
            "huge table",
            format!(r#"(module (memory (export "memory") 1) (table 10000000 funcref) {cb})"#),
        ),
        (
            "many huge tables",
            format!(
                r#"(module (memory (export "memory") 1) {} {cb})"#,
                "(table 10000000 funcref)".repeat(8)
            ),
        ),
        (
            "data segment out of bounds",
            format!(
                r#"(module (memory (export "memory") 1) (data (i32.const 65530) "0123456789") {cb})"#
            ),
        ),
        (
            "element segment out of bounds",
            format!(
                r#"(module (memory (export "memory") 1) (table 1 funcref) (func $f) (elem (i32.const 5) $f) {cb})"#
            ),
        ),
    ] {
        assert_rejected(what, &src);
    }
}

/// LEB128 for the hand-assembled binaries below.
fn leb(mut v: u32, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn section(id: u8, body: &[u8], out: &mut Vec<u8>) {
    out.push(id);
    leb(body.len() as u32, out);
    out.extend_from_slice(body);
}

/// A minimal valid guest in binary form with `globals` mutable i64 globals:
/// memory, three empty callbacks, nothing else.
fn binary_guest(globals: u32) -> Vec<u8> {
    let mut m = b"\0asm\x01\0\0\0".to_vec();
    // types: 0 = () -> (), 1 = (i32, i32) -> ()
    section(1, &[2, 0x60, 0, 0, 0x60, 2, 0x7f, 0x7f, 0], &mut m);
    // functions: three, types 0, 0, 1
    section(3, &[3, 0, 0, 1], &mut m);
    // memory: one, min 1, no max
    section(5, &[1, 0, 1], &mut m);
    if globals > 0 {
        let mut g = Vec::new();
        leb(globals, &mut g);
        for _ in 0..globals {
            g.extend_from_slice(&[0x7e, 1, 0x42, 0, 0x0b]); // (mut i64) = i64.const 0
        }
        section(6, &g, &mut m);
    }
    // exports
    let mut e = vec![4u8];
    for (name, kind, idx) in [
        ("on_init", 0u8, 0u8),
        ("on_tick", 0, 1),
        ("on_switch_edge", 0, 2),
        ("memory", 2, 0),
    ] {
        e.push(name.len() as u8);
        e.extend_from_slice(name.as_bytes());
        e.push(kind);
        e.push(idx);
    }
    section(7, &e, &mut m);
    // code: three empty bodies
    section(10, &[3, 2, 0, 0x0b, 2, 0, 0x0b, 2, 0, 0x0b], &mut m);
    m
}

#[test]
fn a_hand_assembled_binary_guest_loads_and_runs() {
    let bin = binary_guest(3);
    let mut b = WasmScriptBackend::new(&bin, &names(), WasmConfig::engine())
        .unwrap_or_else(|e| panic!("minimal binary guest rejected: {e}"));
    let mut h = Host::new(0);
    h.init(&mut b);
    h.tick(&mut b);
    h.edge(&mut b, 3, true);
    assert!(h.cmd.is_empty());
    assert!(!b.is_faulted());
    assert_eq!(b.memory_size(), PAGE);
}

#[test]
fn huge_global_sections_fail_cleanly() {
    // More globals than any engine accepts.
    let bin = binary_guest(1_000_001);
    assert!(WasmScriptBackend::new(&bin, &names(), WasmConfig::engine()).is_err());
    // A large but representable count: accepted or rejected, never a panic,
    // and a loaded module still runs.
    let bin = binary_guest(100_000);
    if let Ok(mut b) = WasmScriptBackend::new(&bin, &names(), WasmConfig::engine()) {
        let mut h = Host::new(0);
        h.tick(&mut b);
        assert!(!b.is_faulted());
    }
}

#[test]
fn invalid_bytes_are_rejected() {
    let names = names();
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", vec![]),
        ("magic only", b"\0asm".to_vec()),
        (
            "header only (valid empty module)",
            b"\0asm\x01\0\0\0".to_vec(),
        ),
        ("wrong version", b"\0asm\x02\0\0\0".to_vec()),
        ("component header", b"\0asm\x0d\0\x01\0".to_vec()),
        ("garbage text", b"this is not wasm".to_vec()),
        ("lua source", b"function on_tick() end".to_vec()),
        ("unbalanced wat", b"(module (func".to_vec()),
        (
            "wat with unknown instruction",
            b"(module (func (export \"on_tick\") frobnicate))".to_vec(),
        ),
        ("empty wat module", b"(module)".to_vec()),
        ("wat component", b"(component)".to_vec()),
        ("nul bytes", vec![0; 4096]),
        ("0xff bytes", vec![0xff; 4096]),
        ("invalid utf-8 text", b"(module \xff\xfe)".to_vec()),
        (
            "elf header",
            b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0".to_vec(),
        ),
    ];
    for (what, bytes) in cases {
        for c in [WasmConfig::default(), WasmConfig::engine()] {
            let r = WasmScriptBackend::new(&bytes, &names, c);
            let e = r.err().unwrap_or_else(|| panic!("{what}: accepted"));
            assert!(!e.to_string().is_empty());
            assert!(
                !matches!(e, ScriptError::UnsupportedBackend(_)),
                "{what}: a bad module is a load failure, got {e:?}"
            );
        }
    }
}

/// Offsets at which a top-level section of `bin` ends, with the section id.
fn section_ends(bin: &[u8]) -> Vec<(u8, usize)> {
    let mut out = Vec::new();
    let mut i = 8;
    while i < bin.len() {
        let id = bin[i];
        i += 1;
        let (mut size, mut shift) = (0usize, 0);
        loop {
            let b = bin[i];
            i += 1;
            size |= usize::from(b & 0x7f) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                break;
            }
        }
        i += size;
        out.push((id, i));
    }
    assert_eq!(i, bin.len());
    out
}

#[test]
fn truncations_of_a_valid_binary_are_rejected() {
    let names = names();
    let full = support::DRIVER_WASM;
    WasmScriptBackend::new(full, &names, WasmConfig::engine()).unwrap();
    // A truncation at a section boundary after the code section is itself a
    // complete, valid module (it only drops the data and/or custom name
    // sections; the driver has no data-count section). Every other prefix is
    // malformed.
    let ends = section_ends(full);
    let code_end = ends
        .iter()
        .find(|&&(id, _)| id == 10)
        .map(|&(_, e)| e)
        .unwrap();
    assert!(code_end < full.len());
    let valid_cut = |len: usize| len >= code_end && ends.iter().any(|&(_, e)| e == len);
    for len in 0..full.len() {
        let r = WasmScriptBackend::new(&full[..len], &names, WasmConfig::engine());
        if valid_cut(len) {
            continue; // a valid module: accepted or not, but no panic
        }
        assert!(
            r.is_err(),
            "the first {len} of {} bytes were accepted",
            full.len()
        );
    }
    // Trailing garbage is not a module either.
    let mut long = full.to_vec();
    long.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    assert!(WasmScriptBackend::new(&long, &names, WasmConfig::engine()).is_err());
}

#[test]
fn a_host_import_re_exported_as_a_callback_is_harmless() {
    // on_switch_edge is the pulse_coil import itself: (i32, i32) -> ().
    let src = r#"(module (import "pinball" "pulse_coil" (func $p (param i32 i32)))
             (memory (export "memory") 1)
             (func (export "on_init")) (func (export "on_tick"))
             (export "on_switch_edge" (func $p)))"#;
    if let Ok(mut b) = load(src, WasmConfig::engine()) {
        let mut h = Host::new(0);
        h.edge(&mut b, 3, true);
        if !b.is_faulted() {
            assert_eq!(h.cmd.coil_pulses.as_slice(), &[(CoilId(3), 1)]);
        }
        h.edge(&mut b, 65535, false);
        h.tick(&mut b);
    }
}

#[test]
fn extra_exports_and_unused_imports_are_fine() {
    let src = module(
        r#"(memory (export "memory") 1)
           (global (export "version") i32 (i32.const 3))
           (func (export "helper") (param i32) (result i32) (local.get 0))
           (func (export "on_init")) (func (export "on_tick"))
           (func (export "on_switch_edge") (param i32 i32))"#,
    );
    let mut b = load(&src, WasmConfig::engine()).unwrap();
    let mut h = Host::new(0);
    h.init(&mut b);
    h.tick(&mut b);
    assert!(!b.is_faulted() && h.cmd.is_empty());
}

fn try_run(bytes: Vec<u8>) -> bool {
    watchdog("fuzzed module", move || {
        match WasmScriptBackend::new(&bytes, &names(), WasmConfig::engine()) {
            Err(e) => {
                assert!(!e.to_string().is_empty());
                false
            }
            Ok(mut b) => {
                // A module that loads must stay contained when run.
                let mut h = Host::new(1);
                h.init(&mut b);
                for i in 0..4u16 {
                    h.tick(&mut b);
                    h.edge(&mut b, i * 5, i % 2 == 0);
                    h.edge(&mut b, 100 + i, true);
                }
                assert!(b.memory_size() <= WasmConfig::engine().memory_limit_bytes);
                true
            }
        }
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn random_bytes_never_panic_the_loader(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let r = WasmScriptBackend::new(&bytes, &names(), WasmConfig::default());
        prop_assert!(r.is_err());
    }

    #[test]
    fn random_bytes_after_a_valid_header_never_panic(
        tail in proptest::collection::vec(any::<u8>(), 0..1024)
    ) {
        let mut bytes = b"\0asm\x01\0\0\0".to_vec();
        bytes.extend_from_slice(&tail);
        try_run(bytes);
    }

    #[test]
    fn random_text_never_panics_the_loader(text in "[ -~\\n()]{0,400}") {
        let _ = WasmScriptBackend::new(text.as_bytes(), &names(), WasmConfig::default()).map(drop);
        let wrapped = format!("(module {text})");
        let _ = WasmScriptBackend::new(wrapped.as_bytes(), &names(), WasmConfig::default()).map(drop);
    }

    /// Byte flips, deletions and insertions on a valid module: never a panic,
    /// and a mutant that still loads stays contained when run.
    #[test]
    fn mutated_valid_modules_never_panic(
        edits in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>(), 0u8..3), 1..6)
    ) {
        let mut bytes = support::DRIVER_WASM.to_vec();
        for (at, byte, kind) in edits {
            let i = at.index(bytes.len());
            match kind {
                0 => bytes[i] = byte,
                1 => { bytes.remove(i); }
                _ => bytes.insert(i, byte),
            }
        }
        try_run(bytes);
    }
}
