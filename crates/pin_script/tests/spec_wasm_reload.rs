//! Spec (4.5 deliverables 4 and 6, criterion 5): `prepare` compiles and
//! validates off-thread, `apply_prepared` swaps at a tick boundary and carries
//! state through `save_state` / `alloc_state` / `restore_state`; a module that
//! fails to prepare leaves the running backend untouched; `apply_reload` is
//! prepare + apply; `load_backend` / `load_backend_with` select the backend.

#[path = "spec_fixtures/wasm/support.rs"]
mod support;

use pin_core::{CoilId, SceneId, ScriptError, TableScriptBackend};
use pin_script::lua::LuaConfig;
use pin_script::wasm::{PreparedModule, WasmConfig, WasmScriptBackend};
use pin_script::{load_backend, load_backend_with};
use proptest::prelude::*;
use support::{Host, backend, cfg, guest, module, names, watchdog};

/// A counter guest. `on_tick` adds `step` and reports the counter through
/// `set_score(player, n)`; `on_init` shows scene `init_scene`.
/// State protocol (brief): `save_state() -> (ptr << 32) | len` of a blob in
/// guest memory; the host copies it into the new instance at the pointer
/// returned by `alloc_state(len)` and calls `restore_state(ptr, len)`.
/// The guest saves at 2048, receives at 4096, and honours whatever pointer
/// `restore_state` is given.
fn counter(step: i64, player: i32, init_scene: i32, hooks: &str) -> String {
    module(&format!(
        r#"(memory (export "memory") 1)
           (global $n (mut i64) (i64.const 0))
           (func (export "on_init") (call $show_scene (i32.const {init_scene})))
           (func (export "on_tick")
             (global.set $n (i64.add (global.get $n) (i64.const {step})))
             (call $set_score (i32.const {player}) (global.get $n)))
           (func (export "on_switch_edge") (param $id i32) (param $closed i32)
             (if (i32.eq (local.get $id) (i32.const 99)) (then (unreachable)))
             (call $hold_coil (local.get $id) (local.get $closed)))
           {hooks}"#
    ))
}

const SAVE: &str = r#"
  (func (export "save_state") (result i64)
    (i64.store (i32.const 2048) (global.get $n))
    (i64.or (i64.shl (i64.const 2048) (i64.const 32)) (i64.const 8)))"#;
const ALLOC: &str = r#"
  (func (export "alloc_state") (param $len i32) (result i32) (i32.const 4096))"#;
const RESTORE: &str = r#"
  (func (export "restore_state") (param $ptr i32) (param $len i32)
    (if (i32.eq (local.get $len) (i32.const 8)) (then
      (global.set $n (i64.load (local.get $ptr))))))"#;

fn full(step: i64, player: i32, init_scene: i32) -> String {
    counter(step, player, init_scene, &format!("{SAVE}{ALLOC}{RESTORE}"))
}

fn prepare(src: &str) -> Result<PreparedModule, ScriptError> {
    WasmScriptBackend::prepare(src.as_bytes(), WasmConfig::default())
}

/// The single score reported by a tick: `(player index, value)`.
fn tick(h: &mut Host, b: &mut dyn TableScriptBackend) -> (u8, i64) {
    h.tick(b);
    assert_eq!(h.cmd.scores.len(), 1, "queue: {:?}", h.cmd);
    assert_eq!(h.cmd.len(), 1, "queue: {:?}", h.cmd);
    h.cmd.scores[0]
}

#[test]
fn prepared_module_is_send_and_can_be_built_on_another_thread() {
    fn assert_send<T: Send>() {}
    assert_send::<PreparedModule>();
    let v2 = full(100, 2, 5);
    let prepared = std::thread::spawn(move || prepare(&v2))
        .join()
        .unwrap()
        .unwrap();
    let mut b = backend(&full(1, 1, 4));
    let mut h = Host::new(1);
    assert_eq!(tick(&mut h, &mut b), (0, 1));
    b.apply_prepared(prepared).unwrap();
    assert_eq!(tick(&mut h, &mut b), (1, 101));
}

#[test]
fn hot_swap_preserves_state_through_save_and_restore() {
    let mut b = backend(&full(1, 1, 4));
    let mut h = Host::new(1);
    h.init(&mut b);
    assert_eq!(h.cmd.display_scenes.as_slice(), &[SceneId(4)]);
    for i in 1..=5 {
        assert_eq!(tick(&mut h, &mut b), (0, i));
    }
    b.apply_prepared(prepare(&full(100, 2, 5)).unwrap())
        .unwrap();
    // The new rules run with the old counter; on_init is not called again
    // (same as a Lua reload), so no scene 5 appears.
    assert_eq!(tick(&mut h, &mut b), (1, 105));
    assert_eq!(tick(&mut h, &mut b), (1, 205));
    // And back, carrying the state again.
    b.apply_prepared(prepare(&full(1, 1, 4)).unwrap()).unwrap();
    assert_eq!(tick(&mut h, &mut b), (0, 206));
    assert!(!b.is_faulted());
    assert_eq!(b.fault_count(), 0);
    assert_eq!(b.config(), WasmConfig::default());
}

// Review (PR #5): `save_state` runs on the old instance and may change it.
// When the new `restore_state` then traps, the old instance resumes with its
// memory and exported mutable globals as they were before the save.
#[test]
fn failed_restore_puts_back_what_save_state_changed() {
    let old = module(
        r#"(memory (export "memory") 1)
           (global $g (export "g") (mut i64) (i64.const 0))
           (func (export "on_init"))
           (func (export "on_tick")
             (i64.store (i32.const 0) (i64.add (i64.load (i32.const 0)) (i64.const 1)))
             (global.set $g (i64.add (global.get $g) (i64.const 1000)))
             (call $set_score (i32.const 1) (i64.add (i64.load (i32.const 0)) (global.get $g))))
           (func (export "on_switch_edge") (param $id i32) (param $closed i32))
           (func (export "save_state") (result i64)
             (i64.store (i32.const 0) (i64.const 0))
             (global.set $g (i64.const 0))
             (drop (memory.grow (i32.const 1)))
             (i64.store (i32.const 65536) (i64.const 7))
             (i64.const 0))"#,
    );
    let trapping = counter(
        1,
        1,
        4,
        r#"(func (export "restore_state") (param $ptr i32) (param $len i32) (unreachable))"#,
    );
    let mut b = backend(&old);
    let mut h = Host::new(1);
    for i in 1..=3 {
        assert_eq!(tick(&mut h, &mut b), (0, i * 1001));
    }
    assert!(b.apply_prepared(prepare(&trapping).unwrap()).is_err());
    assert_eq!(tick(&mut h, &mut b), (0, 4004), "old state was put back");
    assert!(!b.is_faulted());
}

#[test]
fn state_is_only_carried_when_both_sides_have_the_hooks() {
    // Old module has no save_state: the new one starts fresh.
    let mut b = backend(&counter(1, 1, 4, ""));
    let mut h = Host::new(1);
    for _ in 0..3 {
        tick(&mut h, &mut b);
    }
    b.apply_prepared(prepare(&full(100, 2, 5)).unwrap())
        .unwrap();
    assert_eq!(tick(&mut h, &mut b), (1, 100));

    // New module has no restore_state: it starts fresh.
    let mut b = backend(&full(1, 1, 4));
    for _ in 0..3 {
        tick(&mut h, &mut b);
    }
    b.apply_prepared(prepare(&counter(100, 2, 5, "")).unwrap())
        .unwrap();
    assert_eq!(tick(&mut h, &mut b), (1, 100));
    assert!(!b.is_faulted());
}

#[test]
fn a_module_that_fails_to_prepare_leaves_the_backend_untouched() {
    let mut b = backend(&full(1, 1, 4));
    let mut h = Host::new(1);
    for _ in 0..4 {
        tick(&mut h, &mut b);
    }
    h.edge(&mut b, 6, true);
    let bad: Vec<(&str, Vec<u8>)> = vec![
        ("garbage", b"not wasm at all".to_vec()),
        ("empty", vec![]),
        ("truncated binary", support::DRIVER_WASM[..700].to_vec()),
        ("syntax error", b"(module (func".to_vec()),
        (
            "missing exports",
            b"(module (memory (export \"memory\") 1))".to_vec(),
        ),
        (
            "wasi import",
            br#"(module (import "wasi_snapshot_preview1" "proc_exit" (func (param i32)))
                 (memory (export "memory") 1)
                 (func (export "on_init")) (func (export "on_tick"))
                 (func (export "on_switch_edge") (param i32 i32)))"#
                .to_vec(),
        ),
        (
            "unknown pinball import",
            br#"(module (import "pinball" "nope" (func))
                 (memory (export "memory") 1)
                 (func (export "on_init")) (func (export "on_tick"))
                 (func (export "on_switch_edge") (param i32 i32)))"#
                .to_vec(),
        ),
    ];
    for (what, bytes) in &bad {
        let e = WasmScriptBackend::prepare(bytes, WasmConfig::default())
            .err()
            .unwrap_or_else(|| panic!("{what}: prepared"));
        assert!(!e.to_string().is_empty());
        // The trait path reports the same failure and changes nothing.
        assert!(
            b.apply_reload(bytes).is_err(),
            "{what}: apply_reload accepted"
        );
        assert!(!b.is_faulted());
    }
    assert_eq!(
        tick(&mut h, &mut b),
        (0, 5),
        "the old rules keep running with their state"
    );
    assert_eq!(b.fault_count(), 0);
    // The held coil is still tracked by the untouched backend.
    h.edge(&mut b, 99, true);
    assert!(b.is_faulted());
    assert_eq!(h.cmd.coil_holds.as_slice(), &[(CoilId(6), false)]);
}

#[test]
fn a_module_that_cannot_be_instantiated_leaves_the_backend_untouched() {
    let mut b = support::backend_with(&full(1, 1, 4), WasmConfig::engine());
    let mut h = Host::new(1);
    for _ in 0..4 {
        tick(&mut h, &mut b);
    }
    let cases = [
        // Initial memory above the backend's cap (2 MiB = 32 pages).
        full(100, 2, 5).replace(
            "(memory (export \"memory\") 1)",
            "(memory (export \"memory\") 40)",
        ),
        // Start function traps / spins.
        format!("{} ", full(100, 2, 5)).replacen(
            "(memory (export",
            "(func $boom (unreachable)) (start $boom) (memory (export",
            1,
        ),
        full(100, 2, 5).replacen(
            "(memory (export",
            "(func $spin (loop $l (br $l))) (start $spin) (memory (export",
            1,
        ),
        // restore_state traps: as in Lua, the old script keeps running.
        counter(
            100,
            2,
            5,
            &format!(r#"{ALLOC} (func (export "restore_state") (param i32 i32) (unreachable))"#),
        ),
    ];
    let (b, mut h) = watchdog("failing swap", move || {
        for (i, src) in cases.iter().enumerate() {
            // Either stage may reject; the swap as a whole must fail.
            let r = WasmScriptBackend::prepare(src.as_bytes(), WasmConfig::engine())
                .and_then(|m| b.apply_prepared(m));
            assert!(r.is_err(), "case {i}: swap succeeded");
            assert!(
                b.apply_reload(src.as_bytes()).is_err(),
                "case {i}: apply_reload succeeded"
            );
            assert!(
                !b.is_faulted(),
                "case {i}: a failed swap must not fault the old rules"
            );
        }
        (b, h)
    });
    let mut b = b;
    assert_eq!(tick(&mut h, &mut b), (0, 5));
    assert_eq!(b.fault_count(), 0);
}

#[test]
fn apply_reload_is_prepare_plus_apply() {
    let mut b = backend(&full(1, 1, 4));
    let mut h = Host::new(1);
    for _ in 0..7 {
        tick(&mut h, &mut b);
    }
    // WAT text through the trait object.
    {
        let dynb: &mut dyn TableScriptBackend = &mut b;
        dynb.apply_reload(full(100, 2, 5).as_bytes()).unwrap();
    }
    assert_eq!(tick(&mut h, &mut b), (1, 107));
    // A binary module through the trait (the driver has no state hooks).
    b.apply_reload(support::DRIVER_WASM).unwrap();
    h.tick(&mut b);
    assert_eq!(h.cmd.display_scenes.as_slice(), &[SceneId(7)]);
    assert_eq!(h.cmd.len(), 1);
}

#[test]
fn a_successful_swap_clears_the_fault_and_keeps_the_counters() {
    let mut b = backend(&full(1, 1, 4));
    let mut h = Host::new(1);
    for _ in 0..3 {
        tick(&mut h, &mut b);
    }
    h.edge(&mut b, 12, true);
    h.edge(&mut b, 99, true);
    assert!(b.is_faulted());
    assert_eq!(h.cmd.coil_holds.as_slice(), &[(CoilId(12), false)]);
    h.tick(&mut b);
    assert!(h.cmd.is_empty());
    // A failed reload keeps the tilt.
    assert!(b.apply_reload(b"garbage").is_err());
    assert!(b.is_faulted());
    h.tick(&mut b);
    assert!(h.cmd.is_empty());

    b.apply_prepared(prepare(&full(100, 2, 5)).unwrap())
        .unwrap();
    assert!(!b.is_faulted(), "a successful reload ends the tilt");
    assert_eq!(
        b.fault_count(),
        1,
        "fault_count counts faults since construction"
    );
    // As in Lua, a faulted script's state is not saved: the new rules start fresh.
    assert_eq!(tick(&mut h, &mut b), (1, 100));
    // The new rules can fault again.
    h.edge(&mut b, 99, true);
    assert!(b.is_faulted());
    assert_eq!(b.fault_count(), 2);
}

/// A guest that saves `len` pattern bytes and, on restore, verifies them.
/// `on_tick` reports `set_score(1, restored_len)` and `set_score(2, checksum)`.
fn blob_guest(len: u32) -> String {
    module(&format!(
        r#"(memory (export "memory") 4)
           (global $got (mut i64) (i64.const -1))
           (global $sum (mut i64) (i64.const 0))
           (func (export "on_init"))
           (func (export "on_tick")
             (call $set_score (i32.const 1) (global.get $got))
             (call $set_score (i32.const 2) (global.get $sum)))
           (func (export "on_switch_edge") (param i32 i32))
           (func (export "save_state") (result i64) (local $i i32)
             (block $done (loop $l
               (br_if $done (i32.ge_u (local.get $i) (i32.const {len})))
               (i32.store8 (i32.add (i32.const 65536) (local.get $i))
                 (i32.add (i32.mul (local.get $i) (i32.const 7)) (i32.const 3)))
               (local.set $i (i32.add (local.get $i) (i32.const 1)))
               (br $l)))
             (i64.or (i64.shl (i64.const 65536) (i64.const 32)) (i64.const {len})))
           (func (export "alloc_state") (param i32) (result i32) (i32.const 131072))
           (func (export "restore_state") (param $ptr i32) (param $len i32) (local $i i32)
             (global.set $got (i64.extend_i32_u (local.get $len)))
             (block $done (loop $l
               (br_if $done (i32.ge_u (local.get $i) (local.get $len)))
               (global.set $sum (i64.add (global.get $sum)
                 (i64.mul (i64.extend_i32_u (i32.load8_u (i32.add (local.get $ptr) (local.get $i))))
                          (i64.extend_i32_u (i32.add (local.get $i) (i32.const 1))))))
               (local.set $i (i32.add (local.get $i) (i32.const 1)))
               (br $l))))"#
    ))
}

fn blob_checksum(len: u32) -> i64 {
    (0..len)
        .map(|i| i64::from((i.wrapping_mul(7).wrapping_add(3)) as u8) * (i64::from(i) + 1))
        .sum()
}

#[test]
fn state_blobs_up_to_64_kib_are_copied_intact() {
    for len in [1u32, 8, 1000, 4097, 65535, 65536] {
        let src = blob_guest(len);
        // 64 KiB of byte loops needs more than the engine's fuel; use a roomy budget.
        let c = cfg(16 << 20, 50_000_000);
        let mut b = support::backend_with(&src, c);
        let m = WasmScriptBackend::prepare(src.as_bytes(), c).unwrap();
        b.apply_prepared(m)
            .unwrap_or_else(|e| panic!("len {len}: {e}"));
        let mut h = Host::new(0);
        h.tick(&mut b);
        assert!(!b.is_faulted());
        assert_eq!(
            h.cmd.scores.as_slice(),
            &[(0, i64::from(len)), (1, blob_checksum(len))],
            "blob of {len} bytes"
        );
    }
}

#[test]
fn hostile_state_hooks_do_not_break_the_host() {
    let new = full(100, 2, 5);
    let hostile_saves = [
        // ptr/len outside memory
        r#"(func (export "save_state") (result i64) (i64.const 0x7fffffff00000010))"#,
        r#"(func (export "save_state") (result i64) (i64.const -1))"#,
        r#"(func (export "save_state") (result i64) (i64.const 0x0000ffff00000010))"#,
        // far more than 64 KiB
        r#"(func (export "save_state") (result i64) (i64.const 0x00000000ffffffff))"#,
        r#"(func (export "save_state") (result i64) (i64.const 0x0000000000010001))"#,
        // traps / spins
        r#"(func (export "save_state") (result i64) (unreachable))"#,
        r#"(func (export "save_state") (result i64) (loop $l (br $l)) (i64.const 0))"#,
    ];
    for save in hostile_saves {
        let old = counter(1, 1, 4, save);
        let new = new.clone();
        let outcome = watchdog("hostile save_state", move || {
            let mut b = backend(&old);
            let mut h = Host::new(0);
            for _ in 0..3 {
                h.tick(&mut b);
            }
            let r = prepare(&new).and_then(|m| b.apply_prepared(m));
            h.tick(&mut b);
            (r.is_ok(), h.cmd.scores.first().copied(), b.is_faulted())
        });
        // Either the swap is refused (old rules continue: n = 4 for player 1)
        // or it succeeds without state (new rules: n = 100 for player 2).
        match outcome {
            (false, Some((0, 4)), false) | (true, Some((1, 100)), false) => {}
            other => panic!("{save}: unexpected outcome {other:?}"),
        }
    }

    // Hostile alloc_state in the NEW module: pointer outside its memory.
    for alloc in [
        r#"(func (export "alloc_state") (param i32) (result i32) (i32.const 65532))"#,
        r#"(func (export "alloc_state") (param i32) (result i32) (i32.const -8))"#,
        r#"(func (export "alloc_state") (param i32) (result i32) (unreachable))"#,
        r#"(func (export "alloc_state") (param i32) (result i32) (loop $l (br $l)) (i32.const 0))"#,
    ] {
        let new = counter(100, 2, 5, &format!("{alloc}{RESTORE}"));
        let outcome = watchdog("hostile alloc_state", move || {
            let mut b = backend(&full(1, 1, 4));
            let mut h = Host::new(0);
            for _ in 0..3 {
                h.tick(&mut b);
            }
            let r = prepare(&new).and_then(|m| b.apply_prepared(m));
            h.tick(&mut b);
            (r.is_ok(), h.cmd.scores.first().copied(), b.is_faulted())
        });
        match outcome {
            (false, Some((0, 4)), false) | (true, Some((1, 100)), false) => {}
            other => panic!("{alloc}: unexpected outcome {other:?}"),
        }
    }
}

#[test]
fn the_swapped_in_module_runs_under_the_backend_limits() {
    // New rules spin: after the swap the tick faults by fuel as usual.
    let spin = guest("", "(loop $l (br $l))", "");
    let b = watchdog("swap to spinning rules", move || {
        let mut b = support::backend_with(&full(1, 1, 4), WasmConfig::engine());
        let mut h = Host::new(0);
        h.tick(&mut b);
        let m = WasmScriptBackend::prepare(spin.as_bytes(), WasmConfig::engine()).unwrap();
        b.apply_prepared(m).unwrap();
        h.tick(&mut b);
        b
    });
    assert!(b.is_faulted());
    assert_eq!(b.last_error(), Some(&ScriptError::BudgetExceeded));
    assert!(b.memory_size() <= WasmConfig::engine().memory_limit_bytes);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]
    /// Model: the counter survives any sequence of swaps, failed swaps and ticks.
    #[test]
    fn counter_survives_any_swap_sequence(
        steps in proptest::collection::vec((0u8..4, 1usize..5), 1..10)
    ) {
        let v1 = full(1, 1, 4);
        let v2 = full(100, 2, 5);
        let mut b = backend(&v1);
        let mut h = Host::new(9);
        let (mut n, mut step, mut player) = (0i64, 1i64, 0u8);
        for (kind, ticks) in steps {
            match kind {
                0 => { b.apply_prepared(prepare(&v1).unwrap()).unwrap(); step = 1; player = 0; }
                1 => { b.apply_reload(v2.as_bytes()).unwrap(); step = 100; player = 1; }
                2 => { prop_assert!(b.apply_reload(b"(module").is_err()); }
                _ => {}
            }
            for _ in 0..ticks {
                n += step;
                prop_assert_eq!(tick(&mut h, &mut b), (player, n));
            }
        }
        prop_assert!(!b.is_faulted());
    }
}

// ------------------------------------------------------------ load_backend

#[test]
fn load_backend_selects_wasm_by_extension() {
    let src = full(1, 1, 4);
    for entry in ["rules.wasm", "RULES.WASM", "dir/sub/table.Wasm", "a.b.wasm"] {
        let mut b = load_backend(entry, src.as_bytes(), &names())
            .unwrap_or_else(|e| panic!("{entry}: {e}"));
        let mut h = Host::new(0);
        h.init(b.as_mut());
        assert_eq!(h.cmd.display_scenes.as_slice(), &[SceneId(4)]);
        assert_eq!(tick(&mut h, b.as_mut()), (0, 1));
        // Reload through the boxed trait object carries state.
        b.apply_reload(full(100, 2, 5).as_bytes()).unwrap();
        assert_eq!(tick(&mut h, b.as_mut()), (1, 101));
    }
    // Binary modules too.
    let mut b = load_backend("rules.wasm", support::DRIVER_WASM, &names()).unwrap();
    let mut h = Host::new(0);
    h.tick(b.as_mut());
    assert_eq!(h.cmd.display_scenes.as_slice(), &[SceneId(7)]);

    // A bad module is a load error, not "unsupported backend".
    let e = load_backend("rules.wasm", b"junk", &names()).err().unwrap();
    assert!(!matches!(e, ScriptError::UnsupportedBackend(_)), "{e:?}");
    // Lua is still selected by .lua, and unknown extensions are refused.
    let mut l = load_backend(
        "rules.lua",
        b"function on_tick() pinball.show_scene(3) end",
        &names(),
    )
    .unwrap();
    h.tick(l.as_mut());
    assert_eq!(h.cmd.display_scenes.as_slice(), &[SceneId(3)]);
    assert!(matches!(
        load_backend("rules.py", src.as_bytes(), &names()),
        Err(ScriptError::UnsupportedBackend(_))
    ));
    // Wasm bytes under a .lua name are not sniffed into the Wasm backend.
    assert!(load_backend("rules.lua", support::DRIVER_WASM, &names()).is_err());
}

#[test]
fn load_backend_with_passes_the_configs_through() {
    // 20 000 loop iterations, then a score.
    let wat = guest(
        "",
        "(local $i i32) (local.set $i (i32.const 20000))
         (loop $l (local.set $i (i32.sub (local.get $i) (i32.const 1))) (br_if $l (local.get $i)))
         (call $set_score (i32.const 1) (i64.const 7))",
        "",
    );
    let lua = b"function on_tick() for i = 1, 20000 do end pinball.set_score(1, 7) end";
    let run = |entry: &'static str, src: Vec<u8>, l: LuaConfig, w: WasmConfig| {
        watchdog("load_backend_with", move || {
            let mut b = load_backend_with(entry, &src, &names(), l, w)
                .unwrap_or_else(|e| panic!("{entry}: {e}"));
            let mut h = Host::new(0);
            h.tick(b.as_mut());
            h.cmd.scores.first().copied()
        })
    };
    let lua_small = LuaConfig {
        memory_limit_bytes: 2 << 20,
        instruction_budget: 5_000,
    };
    let wasm_small = cfg(2 << 20, 5_000);
    // Roomy configs: both run to completion.
    assert_eq!(
        run(
            "r.wasm",
            wat.clone().into_bytes(),
            lua_small,
            WasmConfig::default()
        ),
        Some((0, 7))
    );
    assert_eq!(
        run("r.lua", lua.to_vec(), LuaConfig::default(), wasm_small),
        Some((0, 7))
    );
    // Tight configs: the matching backend's budget applies.
    assert_eq!(
        run("r.wasm", wat.into_bytes(), LuaConfig::default(), wasm_small),
        None
    );
    assert_eq!(
        run("r.lua", lua.to_vec(), lua_small, WasmConfig::default()),
        None
    );
    // The memory cap is passed through as well.
    let big = guest("", "", "").replace(
        "(memory (export \"memory\") 1)",
        "(memory (export \"memory\") 40)",
    );
    assert!(
        load_backend_with(
            "r.wasm",
            big.as_bytes(),
            &names(),
            LuaConfig::default(),
            WasmConfig::engine()
        )
        .is_err()
    );
    assert!(
        load_backend_with(
            "r.wasm",
            big.as_bytes(),
            &names(),
            LuaConfig::default(),
            WasmConfig::default()
        )
        .is_ok()
    );
    assert!(matches!(
        load_backend_with(
            "r.txt",
            big.as_bytes(),
            &names(),
            LuaConfig::default(),
            WasmConfig::default()
        ),
        Err(ScriptError::UnsupportedBackend(_))
    ));
}
