//! Subphase 4.5 implementer tests: sandbox limits, faults and rollback,
//! argument validation, hot swap and hostile modules for the Wasm backend.

use pin_core::ids::{CoilId, SwitchId};
use pin_core::{
    HostCommandQueue, HostCtx, HostView, Pcg32, ScriptError, SeedableRng, SwitchEdge,
    TableScriptBackend,
};
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use pin_script::wasm::{MAX_STATE_BYTES, PreparedModule, WasmConfig, WasmScriptBackend};
use pin_script::{NameTables, load_backend, load_backend_with};
use proptest::prelude::*;

/// Every import, named `$<import>`.
const IMPORTS: &str = r#"
  (import "pinball" "pulse_coil" (func $pulse_coil (param i32 i32)))
  (import "pinball" "hold_coil" (func $hold_coil (param i32 i32)))
  (import "pinball" "set_magnet" (func $set_magnet (param i32 f32)))
  (import "pinball" "set_zipper" (func $set_zipper (param i32 i32)))
  (import "pinball" "set_diverter" (func $set_diverter (param i32 i32)))
  (import "pinball" "spawn_ball" (func $spawn_ball (param i32 i32)))
  (import "pinball" "set_lamp" (func $set_lamp (param i32 f32)))
  (import "pinball" "set_gi_tint" (func $set_gi_tint (param f32 f32 f32 f32)))
  (import "pinball" "set_score" (func $set_score (param i32 i64)))
  (import "pinball" "show_scene" (func $show_scene (param i32)))
  (import "pinball" "play_sound" (func $play_sound (param i32 f32)))
  (import "pinball" "persist_set" (func $persist_set (param i32 i64)))
  (import "pinball" "persist_get" (func $persist_get (param i32) (result i64)))
  (import "pinball" "switch" (func $switch (param i32) (result i32)))
  (import "pinball" "tick" (func $tick (result i64)))
  (import "pinball" "balls_in_play" (func $balls_in_play (result i32)))
  (import "pinball" "random" (func $random (param i64 i64) (result i64)))
  (import "pinball" "id_of" (func $id_of (param i32 i32 i32) (result i32)))
"#;

/// A module with every import, one page of memory, `extra` top-level
/// fields, and the given callback bodies.
fn wat(extra: &str, init: &str, tick: &str, switch: &str) -> String {
    format!(
        r#"(module {IMPORTS}
  (memory (export "memory") 1)
  (global $n (mut i32) (i32.const 0))
  {extra}
  (func (export "on_init") {init})
  (func (export "on_tick") {tick})
  (func (export "on_switch_edge") (param $id i32) (param $closed i32) {switch}))"#
    )
}

fn tick_module(tick: &str) -> String {
    wat("", "", tick, "")
}

fn new(src: &str, cfg: WasmConfig) -> WasmScriptBackend {
    WasmScriptBackend::new(src.as_bytes(), &NameTables::default(), cfg).unwrap()
}

fn load_err(src: &str) -> ScriptError {
    WasmScriptBackend::new(
        src.as_bytes(),
        &NameTables::default(),
        WasmConfig::default(),
    )
    .expect_err("module must be rejected")
}

struct Host {
    view: HostView,
    rng: Pcg32,
}

impl Host {
    fn new() -> Self {
        Self {
            view: HostView::default(),
            rng: Pcg32::seed_from_u64(42),
        }
    }

    fn with(&mut self, f: impl FnOnce(&mut HostCtx)) -> HostCommandQueue {
        let mut cmd = HostCommandQueue::default();
        f(&mut HostCtx {
            view: &self.view,
            rng: &mut self.rng,
            cmd: &mut cmd,
        });
        cmd
    }

    fn tick(&mut self, b: &mut dyn TableScriptBackend) -> HostCommandQueue {
        self.view.tick += 1;
        self.with(|c| b.on_tick(c))
    }
}

// ---------------------------------------------------------------- faults ----

#[test]
fn fuel_exhaustion_rolls_back_releases_holds_and_tilts() {
    let src = tick_module(
        r#"
        (global.set $n (i32.add (global.get $n) (i32.const 1)))
        (call $hold_coil (i32.const 4) (i32.const 1))
        (call $pulse_coil (i32.const 1) (i32.const 30))
        (if (i32.eq (global.get $n) (i32.const 3)) (then (loop $l (br $l))))"#,
    );
    let mut b = new(&src, WasmConfig::engine());
    let mut h = Host::new();
    for _ in 0..2 {
        let q = h.tick(&mut b);
        assert_eq!(q.coil_holds.as_slice(), &[(CoilId(4), true)]);
        assert_eq!(q.coil_pulses.len(), 1);
    }
    let start = std::time::Instant::now();
    let q = h.tick(&mut b);
    let took = start.elapsed();
    println!("fuel-exhaustion tick with WasmConfig::engine(): {took:?}");
    assert!(b.is_faulted());
    assert_eq!(b.last_error(), Some(&ScriptError::BudgetExceeded));
    assert!(
        q.coil_pulses.is_empty(),
        "the faulting tick's commands are dropped"
    );
    assert_eq!(q.coil_holds.as_slice(), &[(CoilId(4), false)]);
    for _ in 0..5 {
        assert!(h.tick(&mut b).is_empty());
        assert!(
            h.with(|c| b.on_switch_edge(SwitchId(1), SwitchEdge::Closed, c))
                .is_empty()
        );
        assert!(h.with(|c| b.on_init(c)).is_empty());
    }
    assert_eq!(b.fault_count(), 1);
}

#[test]
fn same_fault_semantics_as_lua() {
    // The Lua twin of the module above, run side by side.
    let lua_src = r#"
        local n = 0
        function on_tick()
          n = n + 1
          pinball.hold_coil(4, true)
          pinball.pulse_coil(1, 30)
          if n == 3 then while true do end end
        end"#;
    let wasm_src = tick_module(
        r#"
        (global.set $n (i32.add (global.get $n) (i32.const 1)))
        (call $hold_coil (i32.const 4) (i32.const 1))
        (call $pulse_coil (i32.const 1) (i32.const 30))
        (if (i32.eq (global.get $n) (i32.const 3)) (then (loop $l (br $l))))"#,
    );
    let mut l = LuaScriptBackend::new(
        lua_src.as_bytes(),
        &NameTables::default(),
        LuaConfig::engine(),
    )
    .unwrap();
    let mut w = new(&wasm_src, WasmConfig::engine());
    let (mut hl, mut hw) = (Host::new(), Host::new());
    for _ in 0..6 {
        assert_eq!(hl.tick(&mut l), hw.tick(&mut w));
        assert_eq!(l.is_faulted(), w.is_faulted());
        assert_eq!(l.fault_count(), w.fault_count());
        assert_eq!(l.last_error(), w.last_error());
    }
}

#[test]
fn traps_in_every_callback_fault() {
    let src = wat("", "unreachable", "", "");
    let mut b = new(&src, WasmConfig::default());
    let mut h = Host::new();
    assert!(h.with(|c| b.on_init(c)).is_empty());
    assert!(b.is_faulted());
    assert!(matches!(b.last_error(), Some(ScriptError::Runtime(m)) if m.contains("unreachable")));

    let src = wat(
        "",
        "",
        "",
        "(call $pulse_coil (local.get $id) (i32.const 5)) unreachable",
    );
    let mut b = new(&src, WasmConfig::default());
    let q = h.with(|c| b.on_switch_edge(SwitchId(9), SwitchEdge::Closed, c));
    assert!(q.is_empty());
    assert!(b.is_faulted());
    // Integer division by zero and stack overflow are traps too.
    let src = tick_module("(drop (i32.div_s (i32.const 1) (global.get $n)))");
    let mut b = new(&src, WasmConfig::default());
    h.tick(&mut b);
    assert!(matches!(b.last_error(), Some(ScriptError::Runtime(_))));
    let src = wat("(func $r (call $r))", "", "(call $r)", "");
    let mut b = new(&src, WasmConfig::default());
    h.tick(&mut b);
    assert!(b.is_faulted());
}

#[test]
fn queue_overflow_is_counted_and_rolled_back_with_a_fault() {
    // 40 pulses: 32 fit, 8 overflow.
    let body = "(call $pulse_coil (i32.const 1) (i32.const 1))".repeat(40);
    let mut b = new(&tick_module(&body), WasmConfig::default());
    let mut h = Host::new();
    assert_eq!(h.tick(&mut b).coil_pulses.len(), 32);
    assert_eq!(b.overflow_count(), 8);
    assert!(!b.is_faulted());
    let body = format!("{body} unreachable");
    let mut b = new(&tick_module(&body), WasmConfig::default());
    assert!(h.tick(&mut b).is_empty());
    assert_eq!(
        b.overflow_count(),
        0,
        "a faulted callback's overflows are rolled back"
    );
}

#[test]
fn dropped_hold_release_is_still_released_on_fault() {
    // Tick 1 holds coil 6; tick 2 releases it and then faults: the release is
    // rolled back, so the tilt release must emit it.
    let src = tick_module(
        r#"
        (global.set $n (i32.add (global.get $n) (i32.const 1)))
        (if (i32.eq (global.get $n) (i32.const 1))
          (then (call $hold_coil (i32.const 6) (i32.const 1)))
          (else (call $hold_coil (i32.const 6) (i32.const 0)) unreachable))"#,
    );
    let mut b = new(&src, WasmConfig::default());
    let mut h = Host::new();
    h.tick(&mut b);
    assert_eq!(h.tick(&mut b).coil_holds.as_slice(), &[(CoilId(6), false)]);
}

// ----------------------------------------------------- argument checks ----

#[test]
fn invalid_arguments_trap_with_the_lua_messages() {
    let cases: &[(&str, &str)] = &[
        (
            "(call $pulse_coil (i32.const -1) (i32.const 1))",
            "pinball.pulse_coil: coil id -1",
        ),
        (
            "(call $pulse_coil (i32.const 65536) (i32.const 1))",
            "pinball.pulse_coil: coil id",
        ),
        (
            "(call $pulse_coil (i32.const 1) (i32.const 65536))",
            "pinball.pulse_coil: duration",
        ),
        (
            "(call $pulse_coil (i32.const 1) (i32.const -1))",
            "pinball.pulse_coil: duration",
        ),
        (
            "(call $hold_coil (i32.const 1) (i32.const 2))",
            "pinball.hold_coil: on must be a boolean",
        ),
        (
            "(call $set_magnet (i32.const 1) (f32.const 1.5))",
            "pinball.set_magnet: power",
        ),
        (
            "(call $set_magnet (i32.const 1) (f32.const nan))",
            "pinball.set_magnet: power",
        ),
        (
            "(call $set_zipper (i32.const 1) (i32.const -1))",
            "pinball.set_zipper: closed",
        ),
        (
            "(call $set_diverter (i32.const 1) (i32.const 7))",
            "pinball.set_diverter: on",
        ),
        (
            "(call $spawn_ball (i32.const 1) (i32.const 70000))",
            "pinball.spawn_ball: profile",
        ),
        (
            "(call $spawn_ball (i32.const -5) (i32.const 0))",
            "pinball.spawn_ball: dispenser",
        ),
        (
            "(call $set_lamp (i32.const 1) (f32.const -0.5))",
            "pinball.set_lamp: intensity",
        ),
        (
            "(call $set_lamp (i32.const 1) (f32.const inf))",
            "pinball.set_lamp: intensity",
        ),
        (
            "(call $set_gi_tint (f32.const 1) (f32.const 1) (f32.const 2) (f32.const 1))",
            "pinball.set_gi_tint: b",
        ),
        (
            "(call $set_gi_tint (f32.const 1) (f32.const 1) (f32.const 1) (f32.const -1))",
            "pinball.set_gi_tint: intensity",
        ),
        (
            "(call $set_score (i32.const 0) (i64.const 1))",
            "pinball.set_score: player 0",
        ),
        (
            "(call $set_score (i32.const 5) (i64.const 1))",
            "pinball.set_score: player 5",
        ),
        (
            "(call $show_scene (i32.const -1))",
            "pinball.show_scene: scene",
        ),
        (
            "(call $play_sound (i32.const 1) (f32.const -1))",
            "pinball.play_sound: gain -1 is negative",
        ),
        (
            "(call $play_sound (i32.const 1) (f32.const inf))",
            "pinball.play_sound: gain must be a finite number",
        ),
        (
            "(call $play_sound (i32.const 99999) (f32.const 1))",
            "pinball.play_sound: sound",
        ),
        (
            "(call $persist_set (i32.const 64) (i64.const 1))",
            "pinball.persist_set: persist key 64",
        ),
        (
            "(drop (call $persist_get (i32.const -1)))",
            "pinball.persist_get: persist key id -1",
        ),
        (
            "(drop (call $switch (i32.const 65536)))",
            "pinball.switch: switch id",
        ),
        (
            "(drop (call $random (i64.const 5) (i64.const 4)))",
            "pinball.random: interval is empty",
        ),
        (
            "(drop (call $id_of (i32.const 11) (i32.const 0) (i32.const 1)))",
            "pinball.id_of: kind 11",
        ),
        (
            "(drop (call $id_of (i32.const -1) (i32.const 0) (i32.const 1)))",
            "pinball.id_of: kind -1",
        ),
        (
            "(drop (call $id_of (i32.const 0) (i32.const 65535) (i32.const 2)))",
            "pinball.id_of: name at 65535+2 is out of bounds",
        ),
        (
            "(drop (call $id_of (i32.const 0) (i32.const 1) (i32.const -1)))",
            "pinball.id_of: name",
        ),
    ];
    let mut h = Host::new();
    for (call, msg) in cases {
        let mut b = new(
            &tick_module(&format!("(call $show_scene (i32.const 1)) {call}")),
            WasmConfig::default(),
        );
        let q = h.tick(&mut b);
        assert!(q.is_empty(), "{call}: commands not rolled back");
        assert!(b.is_faulted(), "{call} must fault");
        match b.last_error() {
            Some(ScriptError::Runtime(m)) => {
                assert!(m.contains(msg), "{call}: {m:?} lacks {msg:?}")
            }
            other => panic!("{call}: unexpected {other:?}"),
        }
    }
}

#[test]
fn boundary_arguments_are_accepted() {
    let body = r#"
      (call $pulse_coil (i32.const 65535) (i32.const 65535))
      (call $pulse_coil (i32.const 0) (i32.const 0))
      (call $hold_coil (i32.const 65535) (i32.const 0))
      (call $set_magnet (i32.const 0) (f32.const 0))
      (call $set_magnet (i32.const 0) (f32.const 1))
      (call $set_lamp (i32.const 255) (f32.const -0))
      (call $set_score (i32.const 4) (i64.const -9223372036854775808))
      (call $set_score (i32.const 1) (i64.const 9223372036854775807))
      (call $play_sound (i32.const 0) (f32.const -0))
      (call $play_sound (i32.const 0) (f32.const 1000))
      (call $persist_set (i32.const 63) (i64.const -1))
      (call $set_gi_tint (f32.const 0) (f32.const 0.25) (f32.const 0.5) (f32.const 1))
      (drop (call $random (i64.const -9223372036854775808) (i64.const 9223372036854775807)))
      (drop (call $random (i64.const 3) (i64.const 3)))
      (drop (call $switch (i32.const 65535)))
      (drop (call $id_of (i32.const 10) (i32.const 0) (i32.const 0)))
      (drop (call $id_of (i32.const 0) (i32.const 65536) (i32.const 0)))"#;
    let mut b = new(&tick_module(body), WasmConfig::default());
    let mut h = Host::new();
    let q = h.tick(&mut b);
    assert!(!b.is_faulted(), "{:?}", b.last_error());
    assert_eq!(
        q.coil_pulses.as_slice(),
        &[(CoilId(65535), 65535), (CoilId(0), 0)]
    );
    assert_eq!(q.scores.as_slice(), &[(3, i64::MIN), (0, i64::MAX)]);
    assert_eq!(q.gi_tint, Some([0.0, 0.25, 0.5, 1.0]));
    assert_eq!(q.sounds[1].1, 1000.0);
}

#[test]
fn reads_see_the_host_view() {
    let src = tick_module(
        r#"
        (call $set_score (i32.const 1) (call $tick))
        (call $set_score (i32.const 2) (i64.extend_i32_u (call $balls_in_play)))
        (call $set_score (i32.const 3) (call $persist_get (i32.const 63)))
        (call $set_score (i32.const 4) (i64.extend_i32_u (call $switch (i32.const 200))))
        (drop (call $switch (i32.const 300)))"#,
    );
    let mut b = new(&src, WasmConfig::default());
    let mut h = Host::new();
    h.view.tick = 1233;
    h.view.balls_in_play = 3;
    h.view.persist[63] = -77;
    h.view.set_switch(SwitchId(200), true);
    let q = h.tick(&mut b);
    assert_eq!(q.scores.as_slice(), &[(0, 1234), (1, 3), (2, -77), (3, 1)]);
}

#[test]
fn id_of_resolves_names_from_the_tables() {
    let names = NameTables {
        switches: vec![("start".into(), SwitchId(12))],
        coils: vec![("kick".into(), CoilId(5))],
        persist: vec![("hs".into(), pin_core::PersistKey(7))],
        ..NameTables::default()
    };
    let src = wat(
        r#"(data (i32.const 100) "startkickhsnope")"#,
        "",
        r#"
        (call $set_score (i32.const 1) (i64.extend_i32_s (call $id_of (i32.const 0) (i32.const 100) (i32.const 5))))
        (call $set_score (i32.const 2) (i64.extend_i32_s (call $id_of (i32.const 1) (i32.const 105) (i32.const 4))))
        (call $set_score (i32.const 3) (i64.extend_i32_s (call $id_of (i32.const 10) (i32.const 109) (i32.const 2))))
        (call $set_score (i32.const 4) (i64.extend_i32_s (call $id_of (i32.const 1) (i32.const 111) (i32.const 4))))"#,
        "",
    );
    let mut b = WasmScriptBackend::new(src.as_bytes(), &names, WasmConfig::default()).unwrap();
    let q = Host::new().tick(&mut b);
    assert_eq!(q.scores.as_slice(), &[(0, 12), (1, 5), (2, 7), (3, -1)]);
}

#[test]
fn id_of_costs_fuel_per_byte() {
    // 100 fuel cannot pay for a 60 000-byte name.
    let src = tick_module("(drop (call $id_of (i32.const 0) (i32.const 0) (i32.const 60000)))");
    let cfg = WasmConfig {
        fuel_per_callback: 100,
        ..WasmConfig::default()
    };
    let mut b = new(&src, cfg);
    Host::new().tick(&mut b);
    assert_eq!(b.last_error(), Some(&ScriptError::BudgetExceeded));
}

#[test]
fn context_imports_trap_outside_callbacks_but_id_of_works() {
    let start = |body: &str| wat(&format!("(func $s {body}) (start $s)"), "", "", "");
    let e = load_err(&start("(call $pulse_coil (i32.const 1) (i32.const 1))"));
    assert!(
        matches!(&e, ScriptError::Load(m) if m.contains("only available inside")),
        "{e:?}"
    );
    let e = load_err(&start("(drop (call $tick))"));
    assert!(matches!(e, ScriptError::Load(_)));
    new(
        &start("(global.set $n (call $id_of (i32.const 0) (i32.const 0) (i32.const 3)))"),
        WasmConfig::default(),
    );
    // State hooks cannot reach the context either: a save_state that calls
    // tick() traps, so no state is carried (and the swap still succeeds).
    let old = wat(
        r#"(func (export "save_state") (result i64) (drop (call $tick)) (i64.const 0))"#,
        "",
        "",
        "",
    );
    let new_src = wat(
        r#"(func (export "restore_state") (param i32 i32) (global.set $n (i32.const 99)))"#,
        "",
        "(call $set_score (i32.const 1) (i64.extend_i32_u (global.get $n)))",
        "",
    );
    let mut b = new(&old, WasmConfig::default());
    b.apply_reload(new_src.as_bytes()).unwrap();
    assert_eq!(Host::new().tick(&mut b).scores.as_slice(), &[(0, 0)]);
}

#[test]
fn random_matches_lua_draw_for_draw() {
    let lua = r#"function on_tick()
        pinball.set_score(1, pinball.random(1, 6))
        pinball.set_score(2, pinball.random(-1000000, 1000000))
        pinball.set_score(3, pinball.random(100))
        pinball.set_score(4, pinball.random(math.mininteger, math.maxinteger))
      end"#;
    let w = tick_module(
        r#"
        (call $set_score (i32.const 1) (call $random (i64.const 1) (i64.const 6)))
        (call $set_score (i32.const 2) (call $random (i64.const -1000000) (i64.const 1000000)))
        (call $set_score (i32.const 3) (call $random (i64.const 1) (i64.const 100)))
        (call $set_score (i32.const 4) (call $random (i64.const -9223372036854775808) (i64.const 9223372036854775807)))"#,
    );
    let mut l = LuaScriptBackend::new(lua.as_bytes(), &NameTables::default(), LuaConfig::default())
        .unwrap();
    let mut b = new(&w, WasmConfig::default());
    let (mut hl, mut hw) = (Host::new(), Host::new());
    for _ in 0..500 {
        assert_eq!(hl.tick(&mut l), hw.tick(&mut b));
    }
    assert_eq!(hl.rng, hw.rng);
}

// --------------------------------------------------------------- memory ----

#[test]
fn memory_grow_past_the_cap_returns_minus_one() {
    // Cap = 4 pages. Growing by 2 works (1 -> 3), by 1 more works (-> 4),
    // then every further grow returns -1 and the memory stays at the cap.
    let cfg = WasmConfig {
        memory_limit_bytes: 4 * 65_536,
        ..WasmConfig::default()
    };
    let src = tick_module(
        r#"
        (call $set_score (i32.const 1) (i64.extend_i32_s (memory.grow (i32.const 2))))
        (call $set_score (i32.const 2) (i64.extend_i32_s (memory.grow (i32.const 1))))
        (call $set_score (i32.const 3) (i64.extend_i32_s (memory.grow (i32.const 1))))
        (call $set_score (i32.const 4) (i64.extend_i32_s (memory.grow (i32.const 65535))))"#,
    );
    let mut b = new(&src, cfg);
    assert_eq!(b.memory_size(), 65_536);
    let q = Host::new().tick(&mut b);
    assert!(!b.is_faulted());
    assert_eq!(q.scores.as_slice(), &[(0, 1), (1, 3), (2, -1), (3, -1)]);
    assert_eq!(b.memory_size(), 4 * 65_536);
}

#[test]
fn memory_never_exceeds_the_cap_with_engine_limits() {
    let src =
        tick_module("(loop $l (br_if $l (i32.ne (memory.grow (i32.const 1)) (i32.const -1))))");
    let mut b = new(&src, WasmConfig::engine());
    let mut h = Host::new();
    for _ in 0..3 {
        h.tick(&mut b);
        assert!(!b.is_faulted());
        assert!(b.memory_size() <= WasmConfig::engine().memory_limit_bytes);
    }
    assert_eq!(b.memory_size(), 2 * 1024 * 1024);
}

#[test]
fn initial_memory_above_the_cap_is_rejected_at_load() {
    let module = |pages: u32| {
        format!(
            r#"(module (memory (export "memory") {pages})
               (func (export "on_init")) (func (export "on_tick"))
               (func (export "on_switch_edge") (param i32 i32)))"#
        )
    };
    let cfg = WasmConfig::engine();
    // 32 pages = exactly 2 MiB: accepted; 33: rejected.
    WasmScriptBackend::new(module(32).as_bytes(), &NameTables::default(), cfg).unwrap();
    let e = WasmScriptBackend::new(module(33).as_bytes(), &NameTables::default(), cfg).unwrap_err();
    assert_eq!(e, ScriptError::MemoryLimit);
    assert_eq!(
        WasmScriptBackend::prepare(module(65536).as_bytes(), cfg).unwrap_err(),
        ScriptError::MemoryLimit
    );
    // Prepared under a larger cap, it is still refused by a smaller backend.
    let p = WasmScriptBackend::prepare(module(40).as_bytes(), WasmConfig::default()).unwrap();
    let mut b = WasmScriptBackend::new(module(1).as_bytes(), &NameTables::default(), cfg).unwrap();
    assert_eq!(b.apply_prepared(p).unwrap_err(), ScriptError::MemoryLimit);
}

// ------------------------------------------------------------- hot swap ----

/// A counter rules module whose state (the counter, 8 bytes at address 64)
/// survives hot swaps via save/alloc/restore. `step` is added per tick.
fn counter(step: i64) -> String {
    wat(
        r#"
        (global $count (mut i64) (i64.const 0))
        (func (export "save_state") (result i64)
          (i64.store (i32.const 64) (global.get $count))
          (i64.or (i64.shl (i64.const 64) (i64.const 32)) (i64.const 8)))
        (func (export "alloc_state") (param $len i32) (result i32) (i32.const 1024))
        (func (export "restore_state") (param $p i32) (param $len i32)
          (if (i32.eq (local.get $len) (i32.const 8))
            (then (global.set $count (i64.load (local.get $p))))))"#,
        "",
        &format!(
            "(global.set $count (i64.add (global.get $count) (i64.const {step})))
             (call $set_score (i32.const 1) (global.get $count))"
        ),
        "",
    )
}

fn score(q: &HostCommandQueue) -> i64 {
    q.scores[0].1
}

#[test]
fn hot_swap_carries_state() {
    let mut b = new(&counter(1), WasmConfig::default());
    let mut h = Host::new();
    for _ in 0..5 {
        h.tick(&mut b);
    }
    let p = WasmScriptBackend::prepare(counter(100).as_bytes(), b.config()).unwrap();
    b.apply_prepared(p).unwrap();
    assert_eq!(score(&h.tick(&mut b)), 105);
    b.apply_reload(counter(1000).as_bytes()).unwrap();
    assert_eq!(score(&h.tick(&mut b)), 1105);
}

#[test]
fn prepare_runs_on_another_thread() {
    fn assert_send<T: Send>() {}
    assert_send::<PreparedModule>();
    assert_send::<WasmScriptBackend>();
    let p = std::thread::spawn(|| {
        WasmScriptBackend::prepare(counter(7).as_bytes(), WasmConfig::default())
    })
    .join()
    .unwrap()
    .unwrap();
    let mut b = new(&counter(1), WasmConfig::default());
    let mut h = Host::new();
    h.tick(&mut b);
    b.apply_prepared(p.clone()).unwrap();
    assert_eq!(score(&h.tick(&mut b)), 8);
    // The same prepared module can be instantiated again.
    b.apply_prepared(p).unwrap();
    assert_eq!(score(&h.tick(&mut b)), 15);
}

#[test]
fn failed_swaps_leave_the_running_rules_untouched() {
    let mut b = new(&counter(1), WasmConfig::default());
    let mut h = Host::new();
    h.tick(&mut b);
    // Prepare failures.
    assert!(matches!(
        b.apply_reload(b"not wasm"),
        Err(ScriptError::Load(_))
    ));
    assert!(WasmScriptBackend::prepare(b"\0asm\x01\0\0\0\x05", b.config()).is_err());
    // Instantiation failure (start traps).
    let bad_start = wat("(func $s unreachable) (start $s)", "", "", "");
    assert!(b.apply_reload(bad_start.as_bytes()).is_err());
    // restore_state traps.
    let bad_restore = wat(
        r#"(func (export "alloc_state") (param i32) (result i32) (i32.const 0))
           (func (export "restore_state") (param i32 i32) unreachable)"#,
        "",
        "",
        "",
    );
    assert!(matches!(
        b.apply_reload(bad_restore.as_bytes()),
        Err(ScriptError::Runtime(_))
    ));
    // alloc_state returns an out-of-bounds pointer.
    let bad_alloc = wat(
        r#"(func (export "alloc_state") (param i32) (result i32) (i32.const 65532))
           (func (export "restore_state") (param i32 i32))"#,
        "",
        "",
        "",
    );
    assert!(
        matches!(b.apply_reload(bad_alloc.as_bytes()), Err(ScriptError::Runtime(m)) if m.contains("alloc_state"))
    );
    // restore_state runs out of fuel.
    let slow_restore = wat(
        r#"(func (export "alloc_state") (param i32) (result i32) (i32.const 0))
           (func (export "restore_state") (param i32 i32) (loop $l (br $l)))"#,
        "",
        "",
        "",
    );
    assert_eq!(
        b.apply_reload(slow_restore.as_bytes()),
        Err(ScriptError::BudgetExceeded)
    );
    assert!(!b.is_faulted());
    assert_eq!(
        score(&h.tick(&mut b)),
        2,
        "the old instance kept running with its state"
    );
}

#[test]
fn state_blob_edge_cases() {
    let saver = |packed: &str| {
        wat(
            &format!(r#"(func (export "save_state") (result i64) {packed})"#),
            "",
            "",
            "",
        )
    };
    let receiver = wat(
        r#"(global $got (mut i64) (i64.const -1))
           (func (export "alloc_state") (param $len i32) (result i32) (i32.const 0))
           (func (export "restore_state") (param $p i32) (param $len i32)
             (global.set $got (i64.extend_i32_u (local.get $len))))"#,
        "",
        "(call $set_score (i32.const 1) (global.get $got))",
        "",
    );
    let mut h = Host::new();
    let mut carried = |packed: &str| {
        let mut b = new(&saver(packed), WasmConfig::default());
        b.apply_reload(receiver.as_bytes()).unwrap();
        score(&h.tick(&mut b))
    };
    // Empty blob: restore_state(0, 0).
    assert_eq!(carried("(i64.const 0)"), 0);
    // A whole 64 KiB page: carried.
    assert_eq!(
        carried(&format!("(i64.const {MAX_STATE_BYTES})")),
        MAX_STATE_BYTES as i64
    );
    // Too long, out of bounds, or trapping: nothing carried.
    assert_eq!(carried(&format!("(i64.const {})", MAX_STATE_BYTES + 1)), -1);
    assert_eq!(carried("(i64.const 0x0000FFFF00000002)"), -1);
    assert_eq!(carried("unreachable"), -1);
    // No alloc_state: a non-empty blob is not delivered.
    let no_alloc = wat(
        r#"(func (export "restore_state") (param i32 i32) unreachable)"#,
        "",
        "",
        "",
    );
    let mut b = new(&saver("(i64.const 4)"), WasmConfig::default());
    b.apply_reload(no_alloc.as_bytes()).unwrap();
}

#[test]
fn reload_after_fault_clears_tilt_but_carries_no_state() {
    let faulty = wat(
        r#"(func (export "save_state") (result i64) unreachable)"#,
        "",
        "(call $hold_coil (i32.const 9) (i32.const 1)) (if (global.get $n) (then unreachable)) (global.set $n (i32.const 1))",
        "",
    );
    let mut b = new(&faulty, WasmConfig::default());
    let mut h = Host::new();
    h.tick(&mut b);
    assert_eq!(h.tick(&mut b).coil_holds.as_slice(), &[(CoilId(9), false)]);
    assert!(b.is_faulted());
    b.apply_reload(counter(3).as_bytes()).unwrap();
    assert!(!b.is_faulted());
    assert_eq!(b.fault_count(), 1);
    assert!(b.last_error().is_some());
    assert_eq!(score(&h.tick(&mut b)), 3);
}

#[test]
fn held_coils_and_overflow_carry_across_swaps() {
    let holder = tick_module(&format!(
        "(call $hold_coil (i32.const 11) (i32.const 1)) {}",
        "(call $show_scene (i32.const 1))".repeat(9)
    ));
    let mut b = new(&holder, WasmConfig::default());
    let mut h = Host::new();
    h.tick(&mut b);
    assert_eq!(b.overflow_count(), 1);
    b.apply_reload(tick_module("unreachable").as_bytes())
        .unwrap();
    assert_eq!(b.overflow_count(), 1);
    // The new rules fault: the coil the old rules held is released.
    assert_eq!(h.tick(&mut b).coil_holds.as_slice(), &[(CoilId(11), false)]);
}

// ------------------------------------------------------- hostile modules ----

const EXPORTS: &str = r#"(memory (export "memory") 1)
  (func (export "on_init")) (func (export "on_tick"))
  (func (export "on_switch_edge") (param i32 i32))"#;

fn is_load(e: &ScriptError) -> bool {
    matches!(e, ScriptError::Load(_))
}

#[test]
fn hostile_modules_fail_cleanly() {
    let cases = [
        // WASI and unknown imports.
        format!(r#"(module (import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32))) {EXPORTS})"#),
        format!(r#"(module (import "pinball" "launch_missiles" (func)) {EXPORTS})"#),
        format!(r#"(module (import "env" "pulse_coil" (func (param i32 i32))) {EXPORTS})"#),
        // Known import, wrong signature.
        format!(r#"(module (import "pinball" "pulse_coil" (func (param i64 i32))) {EXPORTS})"#),
        format!(r#"(module (import "pinball" "tick" (func (result i32))) {EXPORTS})"#),
        // Non-function imports.
        r#"(module (import "pinball" "memory" (memory 1)) (export "memory" (memory 0))
             (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#.into(),
        format!(r#"(module (import "pinball" "g" (global i32)) {EXPORTS})"#),
        format!(r#"(module (import "pinball" "t" (table 1 funcref)) {EXPORTS})"#),
        // Missing / mistyped exports.
        r#"(module (memory (export "memory") 1) (func (export "on_init")) (func (export "on_tick")))"#.into(),
        r#"(module (memory (export "memory") 1) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#.into(),
        r#"(module (memory (export "memory") 1) (func (export "on_init")) (func (export "on_switch_edge") (param i32 i32)))"#.into(),
        r#"(module (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#.into(),
        r#"(module (memory 1) (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#.into(),
        r#"(module (memory (export "memory") 1) (func (export "on_init")) (func (export "on_tick") (param i32)) (func (export "on_switch_edge") (param i32 i32)))"#.into(),
        r#"(module (memory (export "memory") 1) (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32)))"#.into(),
        r#"(module (memory 1) (func (export "memory")) (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#.into(),
        format!(r#"(module {EXPORTS} (func (export "save_state") (result i32) (i32.const 0)))"#),
        format!(r#"(module {EXPORTS} (func (export "restore_state") (param i64)))"#),
        format!(r#"(module {EXPORTS} (func (export "alloc_state") (param i32)))"#),
        format!(r#"(module {EXPORTS} (global (export "on_tick2") i32 (i32.const 0)) (global (export "save_state") i32 (i32.const 0)))"#),
        // Huge or many tables.
        format!(r#"(module {EXPORTS} (table 10000000 funcref))"#),
        format!(r#"(module {EXPORTS} (table 1 funcref) (table 1 funcref) (table 1 funcref) (table 1 funcref) (table 1 funcref))"#),
        // Start function that traps.
        format!(r#"(module {EXPORTS} (func $s unreachable) (start $s))"#),
        // Threads / shared memory, memory64, multi-memory, SIMD-relaxed.
        r#"(module (memory (export "memory") 1 1 shared) (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#.into(),
        r#"(module (memory (export "memory") i64 1) (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32)))"#.into(),
        format!(r#"(module {EXPORTS} (memory 1))"#),
        format!(r#"(module {EXPORTS} (func (result v128) (i32x4.relaxed_trunc_f32x4_s (v128.const i32x4 0 0 0 0))))"#),
        // Invalid bytes.
        "".into(),
        "\0asm".into(),
        "\0asm\x01\0\0\0\x01\x05".into(),
        "(module".into(),
    ];
    for (i, src) in cases.iter().enumerate() {
        let e =
            WasmScriptBackend::new(src.as_bytes(), &NameTables::default(), WasmConfig::engine())
                .err()
                .unwrap_or_else(|| panic!("case {i} was accepted: {src}"));
        assert!(is_load(&e), "case {i}: {e:?}");
    }
}

#[test]
fn looping_start_function_runs_out_of_fuel() {
    let src = format!(r#"(module {EXPORTS} (func $s (loop $l (br $l))) (start $s))"#);
    let e = WasmScriptBackend::new(src.as_bytes(), &NameTables::default(), WasmConfig::engine())
        .unwrap_err();
    assert_eq!(e, ScriptError::BudgetExceeded);
}

#[test]
fn many_globals_and_big_functions_do_not_panic() {
    let globals = "(global i64 (i64.const 1))".repeat(50_000);
    let src = format!("(module {EXPORTS} {globals})");
    let _ = WasmScriptBackend::new(src.as_bytes(), &NameTables::default(), WasmConfig::engine());
    let body = "(drop (i32.const 1))".repeat(20_000);
    let src = format!("(module {EXPORTS} (func {body}))");
    let _ = WasmScriptBackend::new(src.as_bytes(), &NameTables::default(), WasmConfig::engine());
}

#[test]
fn minimal_module_without_imports_runs() {
    let mut b = WasmScriptBackend::new(
        format!("(module {EXPORTS})").as_bytes(),
        &NameTables::default(),
        WasmConfig::engine(),
    )
    .unwrap();
    let mut h = Host::new();
    assert!(h.with(|c| b.on_init(c)).is_empty());
    assert!(h.tick(&mut b).is_empty());
    assert!(
        h.with(|c| b.on_switch_edge(SwitchId(1), SwitchEdge::Opened, c))
            .is_empty()
    );
    assert_eq!(b.config(), WasmConfig::engine());
    assert!(!b.is_faulted());
    assert_eq!(b.fault_count(), 0);
    assert_eq!(b.overflow_count(), 0);
    assert!(b.last_error().is_none());
}

#[test]
fn switch_edges_arrive_with_id_and_flag() {
    let src = wat(
        "",
        "",
        "",
        "(call $set_score (i32.const 1) (i64.extend_i32_u (i32.add (i32.mul (local.get $id) (i32.const 10)) (local.get $closed))))",
    );
    let mut b = new(&src, WasmConfig::default());
    let mut h = Host::new();
    let q = h.with(|c| b.on_switch_edge(SwitchId(255), SwitchEdge::Closed, c));
    assert_eq!(q.scores.as_slice(), &[(0, 2551)]);
    let q = h.with(|c| b.on_switch_edge(SwitchId(65535), SwitchEdge::Opened, c));
    assert_eq!(q.scores.as_slice(), &[(0, 655_350)]);
}

// --------------------------------------------------------- load_backend ----

#[test]
fn load_backend_picks_wasm_by_extension() {
    let src = tick_module("(call $pulse_coil (i32.const 2) (i32.const 3))");
    let mut b = load_backend("rules/RULES.WASM", src.as_bytes(), &NameTables::default()).unwrap();
    assert_eq!(
        Host::new().tick(b.as_mut()).coil_pulses.as_slice(),
        &[(CoilId(2), 3)]
    );
    let mut b = load_backend_with(
        "rules.wasm",
        src.as_bytes(),
        &NameTables::default(),
        LuaConfig::engine(),
        WasmConfig::engine(),
    )
    .unwrap();
    assert_eq!(Host::new().tick(b.as_mut()).coil_pulses.len(), 1);
    assert!(load_backend("rules.wasm", b"garbage", &NameTables::default()).is_err());
    let mut l = load_backend_with(
        "rules.lua",
        b"function on_tick() pinball.pulse_coil(2, 3) end",
        &NameTables::default(),
        LuaConfig::engine(),
        WasmConfig::engine(),
    )
    .unwrap();
    assert_eq!(Host::new().tick(l.as_mut()).coil_pulses.len(), 1);
    for bad in ["rules.js", "rules"] {
        assert!(matches!(
            load_backend_with(
                bad,
                b"",
                &NameTables::default(),
                LuaConfig::default(),
                WasmConfig::default()
            ),
            Err(ScriptError::UnsupportedBackend(_))
        ));
    }
    // apply_reload through the trait object.
    b.apply_reload(tick_module("").as_bytes()).unwrap();
    assert!(Host::new().tick(b.as_mut()).is_empty());
}

// ------------------------------------------------------------ proptests ----

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn new_never_panics_on_random_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = WasmScriptBackend::new(&bytes, &NameTables::default(), WasmConfig::engine());
    }

    #[test]
    fn new_never_panics_on_wasm_magic_plus_random_bytes(tail in proptest::collection::vec(any::<u8>(), 0..256)) {
        let mut bytes = b"\0asm\x01\0\0\0".to_vec();
        bytes.extend(tail);
        let _ = WasmScriptBackend::new(&bytes, &NameTables::default(), WasmConfig::engine());
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn mutated_rules_wasm_never_panics(flips in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8)) {
        let mut bytes = include_bytes!("fixtures/wasm/rules.wasm").to_vec();
        for (i, x) in flips {
            let at = i.index(bytes.len());
            bytes[at] ^= x;
        }
        if let Ok(mut b) = WasmScriptBackend::new(&bytes, &NameTables::default(), WasmConfig::engine()) {
            let mut h = Host::new();
            h.with(|c| b.on_init(c));
            for t in 0..20u16 {
                h.tick(&mut b);
                h.with(|c| b.on_switch_edge(SwitchId(t % 8), SwitchEdge::Closed, c));
            }
        }
    }

    #[test]
    fn wasm_random_equals_lua_random(seed in any::<u64>(), a in any::<i64>(), b in any::<i64>()) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let lua = format!("function on_tick() pinball.set_score(1, pinball.random({lo}, {hi})) end");
        let w = tick_module(&format!(
            "(call $set_score (i32.const 1) (call $random (i64.const {lo}) (i64.const {hi})))"
        ));
        let mut l = LuaScriptBackend::new(lua.as_bytes(), &NameTables::default(), LuaConfig::default()).unwrap();
        let mut x = new(&w, WasmConfig::default());
        let (mut hl, mut hw) = (Host::new(), Host::new());
        hl.rng = Pcg32::seed_from_u64(seed);
        hw.rng = Pcg32::seed_from_u64(seed);
        for _ in 0..4 {
            let (ql, qw) = (hl.tick(&mut l), hw.tick(&mut x));
            prop_assert_eq!(&ql, &qw);
            prop_assert!((lo..=hi).contains(&qw.scores[0].1));
        }
    }

    #[test]
    fn invalid_flags_and_ids_always_fault(v in any::<i32>()) {
        let src = tick_module(&format!("(call $hold_coil (i32.const 1) (i32.const {v}))"));
        let mut b = new(&src, WasmConfig::default());
        Host::new().tick(&mut b);
        prop_assert_eq!(b.is_faulted(), !(v == 0 || v == 1));
        let src = tick_module(&format!("(call $show_scene (i32.const {v}))"));
        let mut b = new(&src, WasmConfig::default());
        Host::new().tick(&mut b);
        prop_assert_eq!(b.is_faulted(), !(0..=65535).contains(&v));
    }

    #[test]
    fn unit_values_fault_exactly_outside_0_1(bits in any::<u32>()) {
        let f = f32::from_bits(bits);
        let src = tick_module(&format!("(call $set_lamp (i32.const 1) (f32.const {}))", wat_f32(f)));
        let mut b = new(&src, WasmConfig::default());
        let q = Host::new().tick(&mut b);
        let ok = (0.0..=1.0).contains(&f);
        prop_assert_eq!(b.is_faulted(), !ok);
        if ok {
            prop_assert_eq!(q.lamps[0].1.to_bits(), f.to_bits());
        }
    }
}

/// `f` as an exact WAT `f32` literal.
fn wat_f32(f: f32) -> String {
    if f.is_nan() {
        format!("nan:0x{:x}", f.to_bits() & 0x7f_ffff)
            .replace("nan:0x0", "nan")
            .replace("nan", if f.is_sign_negative() { "-nan" } else { "nan" })
    } else if f.is_infinite() {
        if f > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        format!("{f:e}")
    }
}

/// Reports (never asserts: rule 5) the cost of a faulting tick that spends
/// its whole engine budget on host calls, the most expensive per fuel unit.
#[test]
fn host_call_loop_tick_cost_report() {
    let src = tick_module(
        r#"(loop $l
             (call $pulse_coil (i32.const 1) (i32.const 1))
             (drop (call $random (i64.const 1) (i64.const 1000)))
             (drop (call $id_of (i32.const 1) (i32.const 0) (i32.const 0)))
             (br $l))"#,
    );
    let mut b = new(&src, WasmConfig::engine());
    let mut h = Host::new();
    let start = std::time::Instant::now();
    h.tick(&mut b);
    println!(
        "host-call-loop faulting tick with WasmConfig::engine(): {:?}",
        start.elapsed()
    );
    assert_eq!(b.last_error(), Some(&ScriptError::BudgetExceeded));
}

// ------------------------------------------- pre-compile size limits ----

fn load_msg(src: &[u8]) -> String {
    match WasmScriptBackend::new(src, &NameTables::default(), WasmConfig::engine()) {
        Err(ScriptError::Load(m)) => m,
        Err(e) => panic!("expected a Load error, got {e:?}"),
        Ok(_) => panic!("module must be rejected"),
    }
}

/// `check_limits` applies the same WAT text cap as `prepare`: valid WAT
/// padded past the cap with whitespace is rejected by both.
#[test]
fn check_limits_applies_the_wat_text_cap() {
    use pin_script::wasm::{MAX_WAT_BYTES, check_limits};
    let module = br#"(module
  (memory (export "memory") 1)
  (func (export "on_init"))
  (func (export "on_tick"))
  (func (export "on_switch_edge") (param i32 i32)))"#;
    let mut text = module.to_vec();
    text.resize(MAX_WAT_BYTES, b' ');
    assert!(check_limits(&text).is_ok());
    assert!(WasmScriptBackend::prepare(&text, WasmConfig::default()).is_ok());
    text.push(b' ');
    let want = format!("at most {MAX_WAT_BYTES} bytes of WAT text");
    match check_limits(&text) {
        Err(ScriptError::Load(m)) => assert!(m.contains(&want), "{m}"),
        other => panic!("check_limits accepted over-cap WAT: {other:?}"),
    }
    assert!(WasmScriptBackend::prepare(&text, WasmConfig::default()).is_err());
}

/// Each limit is enforced by the pre-scan: the messages name the limit,
/// which wasmtime's compiler would never produce.
#[test]
fn oversize_modules_are_rejected_before_compiling() {
    use pin_script::wasm::{
        MAX_FUNCTION_BODY_BYTES, MAX_FUNCTIONS, MAX_GLOBALS, MAX_MODULE_BYTES, MAX_SEGMENTS,
        MAX_TOTAL_TABLE_ELEMENTS, MAX_WAT_BYTES,
    };
    // Binary bigger than the cap (rejected on length alone).
    let mut bin = b"\0asm\x01\0\0\0".to_vec();
    bin.resize(MAX_MODULE_BYTES + 1, 0);
    let m = load_msg(&bin);
    assert!(
        m.contains(&format!("at most {MAX_MODULE_BYTES} bytes of binary")),
        "{m}"
    );
    // WAT text bigger than the text cap.
    let mut text = b"(module ".to_vec();
    text.resize(MAX_WAT_BYTES + 1, b' ');
    let m = load_msg(&text);
    assert!(
        m.contains(&format!("at most {MAX_WAT_BYTES} bytes of WAT text")),
        "{m}"
    );
    // WAT text under its cap that compiles to an oversize binary.
    let data = "a".repeat(MAX_MODULE_BYTES + 1);
    let m = load_msg(format!(r#"(module {EXPORTS} (data (i32.const 0) "{data}"))"#).as_bytes());
    assert!(
        m.contains("compiles to") && m.contains(&MAX_MODULE_BYTES.to_string()),
        "{m}"
    );

    let cases: [(String, &str, String); 6] = [
        (
            "(func)".repeat(MAX_FUNCTIONS as usize - 2),
            "functions",
            MAX_FUNCTIONS.to_string(),
        ),
        (
            format!(
                "(func {})",
                "(drop (i32.const 1))".repeat(MAX_FUNCTION_BODY_BYTES as usize / 3 + 1)
            ),
            "function body",
            MAX_FUNCTION_BODY_BYTES.to_string(),
        ),
        (
            "(global i32 (i32.const 0))".repeat(MAX_GLOBALS as usize + 1),
            "globals",
            MAX_GLOBALS.to_string(),
        ),
        (
            "(table 60000 funcref) (table 40001 funcref)".into(),
            "table elements",
            MAX_TOTAL_TABLE_ELEMENTS.to_string(),
        ),
        (
            r#"(data (i32.const 0) "")"#.repeat(MAX_SEGMENTS as usize + 1),
            "data segments",
            MAX_SEGMENTS.to_string(),
        ),
        (
            format!(
                "(table 1 funcref) {}",
                "(elem (i32.const 0) func)".repeat(MAX_SEGMENTS as usize + 1)
            ),
            "element segments",
            MAX_SEGMENTS.to_string(),
        ),
    ];
    for (extra, what, max) in cases {
        let m = load_msg(format!("(module {EXPORTS} {extra})").as_bytes());
        assert!(
            m.contains(what) && m.contains(&format!("at most {max}")),
            "{what}: {m}"
        );
    }
}

#[test]
fn modules_at_the_limits_still_load() {
    use pin_script::wasm::{MAX_FUNCTIONS, MAX_GLOBALS, MAX_SEGMENTS};
    // EXPORTS defines three functions.
    let funcs = "(func)".repeat(MAX_FUNCTIONS as usize - 3);
    let globals = "(global i32 (i32.const 0))".repeat(MAX_GLOBALS as usize);
    let data = r#"(data (i32.const 0) "")"#.repeat(MAX_SEGMENTS as usize);
    let src = format!(
        "(module {EXPORTS} {funcs} {globals} {data} (table 60000 funcref) (table 40000 funcref))"
    );
    let cfg = WasmConfig::default();
    assert!(WasmScriptBackend::new(src.as_bytes(), &NameTables::default(), cfg).is_ok());
    let bin = include_bytes!("fixtures/wasm/rules.wasm");
    assert!(WasmScriptBackend::prepare(bin, WasmConfig::engine()).is_ok());
}

#[test]
fn reexported_host_functions_are_not_entry_points() {
    let imports = r#"(import "pinball" "pulse_coil" (func $p (param i32 i32)))
      (import "pinball" "tick" (func $t (result i64)))
      (import "pinball" "switch" (func $s (param i32) (result i32)))"#;
    let base =
        r#"(memory (export "memory") 1) (func (export "on_init")) (func (export "on_tick"))"#;
    let cases = [
        format!(r#"(module {imports} {base} (export "on_switch_edge" (func $p)))"#),
        format!(
            r#"(module {imports} {base} (func (export "on_switch_edge") (param i32 i32)) (export "save_state" (func $t)))"#
        ),
        format!(
            r#"(module {imports} {base} (func (export "on_switch_edge") (param i32 i32)) (export "restore_state" (func $p)))"#
        ),
        format!(
            r#"(module {imports} {base} (func (export "on_switch_edge") (param i32 i32)) (export "alloc_state" (func $s)))"#
        ),
    ];
    for src in cases {
        let m = load_msg(src.as_bytes());
        assert!(m.contains("re-exports an imported host function"), "{m}");
    }
    // Re-exporting an import under any other name is harmless.
    let ok = format!(
        r#"(module {imports} {base} (func (export "on_switch_edge") (param i32 i32)) (export "pulse" (func $p)))"#
    );
    assert!(
        WasmScriptBackend::new(ok.as_bytes(), &NameTables::default(), WasmConfig::engine()).is_ok()
    );
}

// ------------------------------------------------------ stack and bulk ----

/// Deep recursion traps at the Wasm stack limit instead of overflowing a
/// host thread that has only 1 MiB of stack.
#[test]
fn deep_recursion_traps_cleanly_on_a_small_host_stack() {
    let src = wat(
        "(func $r (param i64) (result i64) (i64.add (local.get 0) (call $r (local.get 0))))",
        "",
        "(drop (call $r (i64.const 1)))",
        "",
    );
    let err = std::thread::Builder::new()
        .stack_size(1024 * 1024)
        .spawn(move || {
            let mut b = new(&src, WasmConfig::default());
            let mut h = Host::new();
            h.tick(&mut b);
            assert!(b.is_faulted());
            assert!(h.tick(&mut b).is_empty());
            b.last_error().cloned()
        })
        .unwrap()
        .join()
        .expect("the host thread must not overflow");
    match err {
        Some(ScriptError::Runtime(m)) => assert!(m.contains("stack"), "{m}"),
        e => panic!("expected a stack-exhaustion trap, got {e:?}"),
    }
}

/// Bulk operations are charged by size: a handful of whole-memory or
/// whole-table fills/copies, far below the engine budget in instructions,
/// exceeds it in bytes or elements and faults like any other budget overrun
/// (finite workloads, so plain instruction fuel cannot be what stops them).
#[test]
fn bulk_memory_and_table_operations_exhaust_the_budget() {
    let fill = "(memory.fill (i32.const 0) (i32.const 1) (i32.const 65536))";
    let copy = "(memory.copy (i32.const 0) (i32.const 32768) (i32.const 32768))";
    let tfill = "(table.fill $t (i32.const 0) (ref.null func) (i32.const 65536))";
    let budget = WasmConfig::engine().fuel_per_callback;
    for (op, size) in [(fill, 65_536), (copy, 32_768), (tfill, 65_536)] {
        // One operation fits the budget...
        let src = wat("(table $t 65536 funcref)", "", op, "");
        let mut b = new(&src, WasmConfig::engine());
        Host::new().tick(&mut b);
        assert!(!b.is_faulted(), "{op}: {:?}", b.last_error());
        // ...enough of them to exceed it by size do not.
        let n = budget / size + 1;
        let body = op.repeat(n as usize);
        let src = wat("(table $t 65536 funcref)", "", &body, "");
        let mut b = new(&src, WasmConfig::engine());
        Host::new().tick(&mut b);
        assert_eq!(
            b.last_error(),
            Some(&ScriptError::BudgetExceeded),
            "{n} x {op}"
        );
    }
}

/// Reports (never asserts: rule 5) the wall time of faulting bulk-op ticks.
/// `cargo test -p pin_script --release --test wasm_backend bulk_op_tick_cost_report -- --ignored --nocapture`.
#[test]
#[ignore = "prints wall-clock numbers for the report; run in release"]
fn bulk_op_tick_cost_report() {
    for body in [
        "(loop $l (memory.fill (i32.const 0) (i32.const 7) (i32.const 65536)) (br $l))",
        "(loop $l (memory.copy (i32.const 0) (i32.const 32768) (i32.const 32768)) (br $l))",
        "(loop $l (table.fill $t (i32.const 0) (ref.null func) (i32.const 65536)) (br $l))",
    ] {
        let src = wat("(table $t 65536 funcref)", "", body, "");
        let mut worst = std::time::Duration::ZERO;
        for _ in 0..20 {
            let mut b = new(&src, WasmConfig::engine());
            let mut h = Host::new();
            let start = std::time::Instant::now();
            h.tick(&mut b);
            worst = worst.max(start.elapsed());
        }
        println!("worst faulting bulk tick with WasmConfig::engine(): {worst:?}  {body}");
    }
}

/// A WAT syntax error on a multi-megabyte line keeps the error short.
#[test]
fn wat_parse_errors_are_truncated() {
    use pin_script::wasm::{MAX_ERROR_BYTES, MAX_WAT_BYTES};
    // An unknown multi-megabyte token: the parser's error quotes the line,
    // and the 'é' near the cut checks the cut is on a character boundary.
    let mut text = format!("(module {EXPORTS} (func é").into_bytes();
    text.resize(MAX_WAT_BYTES - 4, b'x');
    text.extend_from_slice(b"))");
    let m = load_msg(&text);
    assert!(m.len() <= MAX_ERROR_BYTES, "{} bytes", m.len());
    assert!(m.ends_with('…'), "{m}");
    // Short errors are kept whole.
    let m = load_msg(b"(module (func $x))) junk");
    assert!(m.len() < MAX_ERROR_BYTES && !m.ends_with('…'), "{m}");
}

/// The smallest budget under which `src` loads.
fn min_load_fuel(src: &str) -> u64 {
    let loads = |fuel| {
        let cfg = WasmConfig {
            memory_limit_bytes: 16 << 20,
            fuel_per_callback: fuel,
        };
        match WasmScriptBackend::new(src.as_bytes(), &NameTables::default(), cfg) {
            Ok(_) => true,
            Err(ScriptError::BudgetExceeded) => false,
            Err(e) => panic!("unexpected load error {e}"),
        }
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

/// D9 (macOS CI regression): whether a module loads depends only on its
/// start function's cost, never on its data or element segments. wasmtime
/// fills linear memory from a copy-on-write image where the host supports
/// one (Linux memfd) and with metered start-up code elsewhere (macOS,
/// Windows); that copy used to come out of the start budget, so a guest
/// with data at both ends of a page (a 64 KiB image) failed to load under
/// a budget below about 65 536 on macOS only.
#[test]
fn segments_never_change_whether_a_module_loads() {
    let module = |start: &str, segs: &str| {
        format!(
            r#"(module (memory (export "memory") 4) (table 64 funcref)
              (func (export "on_init")) (func (export "on_tick"))
              (func (export "on_switch_edge") (param i32 i32))
              (func $s (local $i i32)
                (loop $l (local.set $i (i32.add (local.get $i) (i32.const 1)))
                  (br_if $l (i32.lt_u (local.get $i) (i32.const 300)))))
              {start} {segs})"#
        )
    };
    let big = "xy".repeat(60_000);
    let segments = [
        String::new(),
        r#"(data (i32.const 1024) "left_sling") (data (i32.const 65531) "start")"#.to_owned(),
        format!(r#"(data (i32.const 3) "{big}") (data (i32.const 131000) "z")"#),
        "(elem (i32.const 0) func $s $s $s) (elem (i32.const 9) funcref (ref.func $s) (ref.null func))"
            .to_owned(),
    ];
    let base = min_load_fuel(&module("(start $s)", ""));
    assert!(base > 300, "{base}");
    for segs in &segments {
        assert_eq!(
            min_load_fuel(&module("(start $s)", segs)),
            base,
            "{segs:.70}"
        );
        // Without a start function, any budget loads them.
        assert_eq!(min_load_fuel(&module("", segs)), 1, "{segs:.70}");
    }
}
