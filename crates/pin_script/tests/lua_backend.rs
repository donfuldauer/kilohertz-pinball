//! Implementer integration tests for the Lua backend (Subphase 4.2).

use pin_core::ids::{CoilId, LampId, PersistKey, SwitchId};
use pin_core::{
    HostCommandQueue, HostCtx, HostView, Pcg32, ScriptError, SeedableRng, SwitchEdge,
    TableScriptBackend,
};
use pin_script::lua::reload::{ReloadWatcher, syntax_check};
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use pin_script::{NameTables, load_backend};
use proptest::prelude::*;
use std::time::Duration;

fn names() -> NameTables {
    NameTables {
        switches: vec![
            ("left_sling".into(), SwitchId(4)),
            ("start".into(), SwitchId(9)),
        ],
        coils: vec![("sling".into(), CoilId(2)), ("trough".into(), CoilId(7))],
        lamps: vec![("shoot_again".into(), LampId(5))],
        persist: vec![("high_score".into(), PersistKey(3))],
        ..NameTables::default()
    }
}

fn new(src: &str) -> LuaScriptBackend {
    LuaScriptBackend::new(src.as_bytes(), &names(), LuaConfig::default()).unwrap()
}

fn small(src: &str, budget: u32, mem: usize) -> LuaScriptBackend {
    LuaScriptBackend::new(
        src.as_bytes(),
        &names(),
        LuaConfig {
            memory_limit_bytes: mem,
            instruction_budget: budget,
        },
    )
    .unwrap()
}

struct Host {
    view: HostView,
    rng: Pcg32,
    cmd: HostCommandQueue,
}

impl Host {
    fn new(seed: u64) -> Self {
        Self {
            view: HostView::default(),
            rng: Pcg32::seed_from_u64(seed),
            cmd: HostCommandQueue::default(),
        }
    }
    fn ctx(&mut self) -> HostCtx<'_> {
        HostCtx {
            view: &self.view,
            rng: &mut self.rng,
            cmd: &mut self.cmd,
        }
    }
    fn tick(&mut self, b: &mut dyn TableScriptBackend) -> HostCommandQueue {
        self.cmd.clear();
        b.on_tick(&mut self.ctx());
        self.view.tick += 1;
        self.cmd.clone()
    }
    fn edge(&mut self, b: &mut dyn TableScriptBackend, sw: u16, closed: bool) -> HostCommandQueue {
        self.cmd.clear();
        self.view.set_switch(SwitchId(sw), closed);
        let e = if closed {
            SwitchEdge::Closed
        } else {
            SwitchEdge::Opened
        };
        b.on_switch_edge(SwitchId(sw), e, &mut self.ctx());
        self.cmd.clone()
    }
}

const RULES: &str = r#"
local score = 0
function on_init() pinball.set_score(1, 0) pinball.set_lamp(pinball.lamp.shoot_again, 1) end
function on_switch(id, closed)
  if id == pinball.sw.left_sling and closed then
    score = score + 10
    pinball.pulse_coil(pinball.coil.sling, 20)
    pinball.set_score(1, score)
  elseif id == pinball.sw.start and closed and pinball.balls_in_play() == 0 then
    pinball.hold_coil(pinball.coil.trough, true)
    pinball.play_sound(1, 0.5)
  end
end
function on_tick()
  if pinball.tick() % 100 == 99 then pinball.show_scene(2) end
end
function on_save_state() return { score = score } end
function on_restore_state(s) score = s.score end
"#;

#[test]
fn switch_sequence_produces_expected_commands() {
    let mut b = new(RULES);
    let mut h = Host::new(1);
    b.on_init(&mut h.ctx());
    assert_eq!(h.cmd.scores.as_slice(), &[(0, 0)]);
    assert_eq!(h.cmd.lamps.as_slice(), &[(LampId(5), 1.0)]);
    let q = h.edge(&mut b, 4, true);
    assert_eq!(q.coil_pulses.as_slice(), &[(CoilId(2), 20)]);
    assert_eq!(q.scores.as_slice(), &[(0, 10)]);
    assert!(h.edge(&mut b, 4, false).is_empty());
    let q = h.edge(&mut b, 4, true);
    assert_eq!(q.scores.as_slice(), &[(0, 20)]);
    let q = h.edge(&mut b, 9, true);
    assert_eq!(q.coil_holds.as_slice(), &[(CoilId(7), true)]);
    assert_eq!(q.sounds.as_slice(), &[(pin_core::SoundId(1), 0.5)]);
    for _ in 0..99 {
        assert!(h.tick(&mut b).is_empty());
    }
    assert_eq!(h.tick(&mut b).display_scenes.len(), 1);
    assert!(!b.is_faulted());
}

#[test]
fn every_api_function_maps_to_its_queue() {
    let mut b = new(r#"
        function on_tick()
          pinball.set_magnet(1, 0.25) pinball.set_zipper(2, true) pinball.set_diverter(3, false)
          pinball.spawn_ball(4, 5) pinball.set_gi_tint(1, 0.5, 0, 1) pinball.persist_set(3, 77)
          pinball.set_score(4, 123456789012)
          assert(pinball.persist_get(pinball.persist_key.high_score) == 42)
          assert(pinball.switch(9) == true and pinball.switch(8) == false and pinball.switch(300) == false)
          assert(pinball.tick() == 5 and pinball.balls_in_play() == 2)
          local r = pinball.random(6) assert(r >= 1 and r <= 6)
          local q = pinball.random(-3, -1) assert(q >= -3 and q <= -1)
          assert(math.type(math.random(0)) == "integer")
        end"#);
    let mut h = Host::new(3);
    h.view.persist[3] = 42;
    h.view.tick = 5;
    h.view.balls_in_play = 2;
    h.view.set_switch(SwitchId(9), true);
    let q = h.tick(&mut b);
    assert!(!b.is_faulted(), "{:?}", b.last_error());
    assert_eq!(q.magnets.as_slice(), &[(pin_core::MagnetId(1), 0.25)]);
    assert_eq!(q.zippers.as_slice(), &[(pin_core::FlipperId(2), true)]);
    assert_eq!(q.diverters.as_slice(), &[(pin_core::DiverterId(3), false)]);
    assert_eq!(
        q.ball_spawns.as_slice(),
        &[(pin_core::DispenserId(4), pin_core::ProfileId(5))]
    );
    assert_eq!(q.gi_tint, Some([1.0, 0.5, 0.0, 1.0]));
    assert_eq!(q.persist_writes.as_slice(), &[(PersistKey(3), 77)]);
    assert_eq!(q.scores.as_slice(), &[(3, 123456789012)]);
}

#[test]
fn argument_errors_fault() {
    for call in [
        "pinball.pulse_coil(65536, 10)",
        "pinball.pulse_coil(1, 70000)",
        "pinball.pulse_coil(-1, 10)",
        "pinball.pulse_coil(1.5, 10)",
        "pinball.hold_coil(1, 1)",
        "pinball.set_lamp(1, 1.5)",
        "pinball.set_magnet(1, 0/0)",
        "pinball.set_score(0, 1)",
        "pinball.set_score(5, 1)",
        "pinball.set_score(1, 0.5)",
        "pinball.persist_get(64)",
        "pinball.play_sound(1, -1)",
        "pinball.set_gi_tint(1, 1, 1)",
        "pinball.random(0)",
        "pinball.random(5, 1)",
        "pinball.random()",
        "math.random(1, 2, 3)",
        "math.random('x')",
        "pinball.switch('a')",
    ] {
        let mut b = new(&format!("function on_tick() {call} end"));
        let mut h = Host::new(0);
        h.tick(&mut b);
        assert!(b.is_faulted(), "{call} should fault");
        assert!(
            matches!(b.last_error(), Some(ScriptError::Runtime(_))),
            "{call}"
        );
    }
}

#[test]
fn api_outside_callbacks_is_an_error() {
    let e = LuaScriptBackend::new(b"pinball.pulse_coil(1, 2)", &names(), LuaConfig::default())
        .unwrap_err();
    assert!(matches!(e, ScriptError::Load(m) if m.contains("only available")));
}

#[test]
fn overflow_drops_and_counts() {
    let mut b = new("function on_tick() for i = 1, 40 do pinball.pulse_coil(i, 1) end end");
    let mut h = Host::new(0);
    let q = h.tick(&mut b);
    assert_eq!(q.coil_pulses.len(), 32);
    assert_eq!(b.overflow_count(), 8);
    assert!(!b.is_faulted());
}

#[test]
fn sandbox_escapes_fail() {
    for probe in [
        "return io == nil and os == nil and debug == nil and package == nil and coroutine == nil",
        "return load == nil and dofile == nil and loadfile == nil and require == nil",
        "return string.dump == nil",
        "return not pcall(collectgarbage, 'collect') and type(collectgarbage('count')) == 'number'",
        "return getmetatable('') == 'locked'",
        "return not pcall(setmetatable, pinball, {})",
        "return not pcall(setmetatable, pinball.sw, {})",
        "return getmetatable(pinball) == 'locked'",
        "return not pcall(function() pinball.sw.x = 1 end)",
        "return not pcall(function() pinball.pulse_coil = nil end)",
        "return not pcall(rawset, pinball.coil, 'x', 1) and pinball.coil.x == nil",
        "return not pcall(rawset, pinball, 'tick', 1)",
        "return not pcall(table.insert, pinball.lamp, 1)",
        "return not pcall(setmetatable, {}, {__gc = function() end})",
        "return pinball.sw.left_sling == 4 and pinball.coil.trough == 7",
        "local n = 0 for k, v in pairs(pinball.sw) do n = n + 1 end return n == 2",
        "print('x') math.randomseed(1) return true",
    ] {
        let src = format!(
            "ok = (function() {probe} end)() \
             function on_tick() pinball.set_score(1, ok and 1 or 0) end"
        );
        let mut b = LuaScriptBackend::new(src.as_bytes(), &names(), LuaConfig::default())
            .unwrap_or_else(|e| panic!("{probe}: {e}"));
        let q = Host::new(0).tick(&mut b);
        assert_eq!(q.scores.as_slice(), &[(0, 1)], "{probe}");
    }
}

#[test]
fn bytecode_is_rejected() {
    let bytecode = b"\x1bLua\x54\x00\x19\x93\r\n\x1a\n";
    assert!(matches!(
        LuaScriptBackend::new(bytecode, &names(), LuaConfig::default()),
        Err(ScriptError::Load(_))
    ));
    assert!(syntax_check(bytecode).is_err());
}

#[test]
fn infinite_loops_are_aborted() {
    for src in [
        "function on_tick() while true do end end",
        "function on_tick() while true do pcall(function() while true do end end) end end",
        "function on_tick() xpcall(function() error('x') end, function() while true do end end) end",
        "function on_tick() local function f() return f() end f() end",
        "function on_tick() table.sort({3,2,1,4,5,6}, function(a, b) while true do end end) end",
    ] {
        let mut b = small(src, 50_000, 1 << 20);
        let mut h = Host::new(0);
        h.tick(&mut b);
        assert!(b.is_faulted(), "{src}");
        assert_eq!(b.last_error(), Some(&ScriptError::BudgetExceeded), "{src}");
    }
    // Top-level loops fail `new`.
    assert_eq!(
        LuaScriptBackend::new(b"while true do end", &names(), LuaConfig::default()).unwrap_err(),
        ScriptError::BudgetExceeded
    );
}

#[test]
fn finite_work_within_budget_passes() {
    let mut b = small(
        "function on_tick() local s = 0 for i = 1, 10000 do s = s + i end end",
        1_000_000,
        1 << 20,
    );
    let mut h = Host::new(0);
    for _ in 0..10 {
        h.tick(&mut b);
    }
    assert!(!b.is_faulted());
}

#[test]
fn memory_bombs_are_aborted() {
    for src in [
        "function on_tick() local t = {} for i = 1, 1e8 do t[i] = string.rep('x', 64) .. i end end",
        "function on_tick() local s = string.rep('x', 1 << 28) end",
        "function on_tick() while true do pcall(string.rep, 'x', 1 << 28) end end",
        "function on_tick() local s = 'x' while true do s = s .. s end end",
    ] {
        let mut b = small(src, 100_000_000, 2 << 20);
        let mut h = Host::new(0);
        h.tick(&mut b);
        assert_eq!(b.last_error(), Some(&ScriptError::MemoryLimit), "{src}");
        assert!(b.used_memory() <= 4 << 20);
    }
}

#[test]
fn reload_preserves_state_through_hooks() {
    let mut b = new(RULES);
    let mut h = Host::new(1);
    h.edge(&mut b, 4, true);
    h.edge(&mut b, 4, false);
    let v2 = RULES.replace("score + 10", "score + 100");
    b.apply_reload(v2.as_bytes()).unwrap();
    let q = h.edge(&mut b, 4, true);
    assert_eq!(q.scores.as_slice(), &[(0, 110)]);
    // A broken reload keeps the old script and its state.
    assert!(matches!(
        b.apply_reload(b"function ("),
        Err(ScriptError::Load(_))
    ));
    h.edge(&mut b, 4, false);
    assert_eq!(h.edge(&mut b, 4, true).scores.as_slice(), &[(0, 210)]);
    // A failing on_restore_state rejects the reload.
    let bad = format!("{v2} function on_restore_state(s) error('nope') end");
    assert!(matches!(
        b.apply_reload(bad.as_bytes()),
        Err(ScriptError::Runtime(_))
    ));
    h.edge(&mut b, 4, false);
    assert_eq!(h.edge(&mut b, 4, true).scores.as_slice(), &[(0, 310)]);
    // A failing on_save_state just drops the state.
    b.apply_reload(b"function on_save_state() error('x') end")
        .unwrap();
    b.apply_reload(b"function on_restore_state(s) error('not called') end")
        .unwrap();
    b.apply_reload(RULES.as_bytes()).unwrap();
}

#[test]
fn reload_clears_fault_and_new_hold_tracking_carries() {
    let mut b = new("function on_tick() pinball.hold_coil(1, true) pinball.hold_coil(2, true) end");
    let mut h = Host::new(0);
    h.tick(&mut b);
    b.apply_reload(b"function on_tick() error('tilt') end")
        .unwrap();
    let q = h.tick(&mut b);
    assert!(b.is_faulted());
    assert_eq!(
        q.coil_holds.as_slice(),
        &[(CoilId(1), false), (CoilId(2), false)]
    );
    assert!(h.tick(&mut b).is_empty());
    b.apply_reload(b"function on_tick() pinball.set_lamp(1, 1) end")
        .unwrap();
    assert!(!b.is_faulted());
    assert_eq!(h.tick(&mut b).lamps.len(), 1);
    assert_eq!(b.fault_count(), 1);
}

#[test]
fn hold_release_continues_when_queue_is_full() {
    let mut b = new(
        "n = 0 function on_tick() n = n + 1 if n == 1 then for i = 1, 32 do pinball.hold_coil(i, true) end \
         elseif n == 2 then for i = 33, 40 do pinball.hold_coil(i, true) end \
         else for i = 1, 32 do pinball.pulse_coil(i, 1) end error('x') end end",
    );
    let mut h = Host::new(0);
    h.tick(&mut b);
    h.tick(&mut b);
    let q = h.tick(&mut b);
    assert!(b.is_faulted());
    assert_eq!(q.coil_holds.len(), 32);
    let q = h.tick(&mut b);
    assert_eq!(q.coil_holds.len(), 8);
    assert!(q.coil_holds.iter().all(|&(_, on)| !on));
    assert!(h.tick(&mut b).is_empty());
}

fn run_trace(seed: u64) -> Vec<HostCommandQueue> {
    let mut b = new(
        "function on_tick() if pinball.random(10) > 7 then pinball.pulse_coil(pinball.random(1, 30), 5) end \
         if math.random() < 0.1 then pinball.set_score(1, math.random(1000)) end end \
         function on_switch(id, c) if c then pinball.set_lamp(id, pinball.random(2) == 1 and 1 or 0) end end",
    );
    let mut h = Host::new(seed);
    let mut out = Vec::new();
    for t in 0..500u16 {
        out.push(h.tick(&mut b));
        if t % 7 == 0 {
            out.push(h.edge(&mut b, t % 50, t % 14 == 0));
        }
    }
    out
}

#[test]
fn determinism_same_seed_same_queues() {
    assert_eq!(run_trace(42), run_trace(42));
    assert_ne!(run_trace(42), run_trace(43));
}

#[test]
fn load_backend_by_extension() {
    let mut b = load_backend("rules.lua", RULES.as_bytes(), &names()).unwrap();
    let mut h = Host::new(0);
    b.on_init(&mut h.ctx());
    assert!(!h.cmd.is_empty());
    assert!(load_backend("RULES.LUA", b"", &names()).is_ok());
    for entry in ["rules.wasm", "rules.py", "rules"] {
        match load_backend(entry, b"", &names()) {
            Err(ScriptError::UnsupportedBackend(m)) => assert!(m.contains(entry)),
            _ => panic!("{entry} should be unsupported"),
        }
    }
    match load_backend("x.wasm", b"", &names()) {
        Err(ScriptError::UnsupportedBackend(m)) => assert!(m.contains("4.5")),
        _ => panic!(),
    }
    assert!(matches!(
        load_backend("x.lua", b"(", &names()),
        Err(ScriptError::Load(_))
    ));
}

#[test]
fn backend_is_send() {
    fn assert_send<T: Send>(_: T) {}
    assert_send(new(""));
    let b = new("x = 1 function on_tick() x = x + 1 pinball.set_score(1, x) end");
    let q = std::thread::spawn(move || {
        let mut b = b;
        Host::new(0).tick(&mut b)
    })
    .join()
    .unwrap();
    assert_eq!(q.scores.as_slice(), &[(0, 2)]);
}

#[test]
fn watcher_delivers_checked_changes() {
    let dir = std::env::temp_dir().join(format!("pin_script_watch_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rules.lua");
    std::fs::write(&path, "x = 1").unwrap();
    let mut w = ReloadWatcher::new(&path).unwrap();
    assert_eq!(w.path(), path.as_path());
    assert!(format!("{w:?}").contains("ReloadWatcher"));
    assert_eq!(w.poll(), None);
    assert_eq!(
        w.force_check(),
        None,
        "unchanged contents are not redelivered"
    );
    std::fs::write(&path, "x = 2").unwrap();
    assert_eq!(
        w.wait(Duration::from_secs(20)).as_deref(),
        Some(&b"x = 2"[..])
    );
    std::fs::write(&path, "x = (").unwrap();
    assert_eq!(w.wait(Duration::from_secs(2)), None);
    // The bad edit may or may not have been seen yet; force a read.
    assert_eq!(w.force_check(), None);
    assert!(matches!(w.last_error(), Some(ScriptError::Load(_))));
    std::fs::write(&path, "x = 3").unwrap();
    let got = w.wait(Duration::from_secs(20)).or_else(|| w.force_check());
    assert_eq!(got.as_deref(), Some(&b"x = 3"[..]));
    assert!(w.last_error().is_none());
    drop(w);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(ReloadWatcher::new(dir.join("missing_dir/rules.lua")).is_err());
    assert!(ReloadWatcher::new("/").is_err());
}

#[test]
fn syntax_check_accepts_and_rejects() {
    assert!(syntax_check(RULES.as_bytes()).is_ok());
    assert!(matches!(
        syntax_check(b"end end"),
        Err(ScriptError::Load(_))
    ));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn random_bytes_never_panic(src in proptest::collection::vec(any::<u8>(), 0..256)) {
        let _ = syntax_check(&src);
        if let Ok(mut b) = LuaScriptBackend::new(&src, &names(), LuaConfig { memory_limit_bytes: 1 << 20, instruction_budget: 10_000 }) {
            let mut h = Host::new(0);
            h.tick(&mut b);
            let _ = b.apply_reload(&src);
        }
    }

    #[test]
    fn random_ints_stay_in_range(seed in any::<u64>(), lo in -1000i64..1000, span in 0i64..1000) {
        let src = format!("function on_tick() for i = 1, 8 do pinball.set_score(1, pinball.random({lo}, {})) end end", lo + span);
        let mut b = new(&src);
        let mut h = Host::new(seed);
        h.cmd.clear();
        b.on_tick(&mut h.ctx());
        prop_assert!(!b.is_faulted());
        for &(_, s) in &h.cmd.scores { prop_assert!(lo <= s && s <= lo + span); }
    }
}
