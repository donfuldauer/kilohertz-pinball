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

mod temp_dir_support;
use temp_dir_support::TempDir;

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
        // Finite as f64, infinite once narrowed to f32.
        "pinball.play_sound(1, 1e300)",
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
        "function on_tick() local t = {} for i = 1, 1e8 do t[i] = string.rep('x', 60000) .. i end end",
        "function on_tick() local t = {} while true do pcall(function() for i = 1, 1e9 do t[#t + 1] = string.rep('x', 60000) .. i end end) end end",
    ] {
        let mut b = small(src, 100_000_000, 2 << 20);
        let mut h = Host::new(0);
        h.tick(&mut b);
        assert_eq!(b.last_error(), Some(&ScriptError::MemoryLimit), "{src}");
        assert!(b.used_memory() <= 4 << 20);
    }
}

// Review (PR #5): the outer `pcall` of `pcall(pcall, f)` catches the error
// `memabort` raises for the inner one, so the callback returns normally. The
// recorded memory hit must still fault it and drop what it queued.
#[test]
fn memory_error_swallowed_by_nested_pcall_still_faults() {
    let src = "function on_tick()
      pinball.pulse_coil(1, 10)
      pcall(pcall, function()
        local t = {} for i = 1, 1e9 do t[i] = string.rep('x', 60000) .. i end
      end)
    end";
    let mut b = small(src, 100_000_000, 2 << 20);
    let mut h = Host::new(0);
    let q = h.tick(&mut b);
    assert_eq!(b.last_error(), Some(&ScriptError::MemoryLimit));
    assert!(
        q.coil_pulses.is_empty(),
        "the faulted callback's commands are rolled back"
    );
}

// Review (PR #5): the heap cap also bounds compilation. Distinct long
// literals (each under the 64 KiB string cap) whose constants together
// exceed the cap must be rejected, not raise the cap.
#[test]
fn compiled_source_over_the_memory_cap_is_rejected() {
    let mut src = String::from("local t = {");
    for i in 0..64 {
        src.push_str(&format!("\"{i:04}{}\",", "x".repeat(50_000)));
    }
    src.push_str("}\nfunction on_tick() end");
    let cfg = LuaConfig {
        memory_limit_bytes: 2 << 20,
        instruction_budget: 1_000_000,
    };
    let e = LuaScriptBackend::new(src.as_bytes(), &names(), cfg).unwrap_err();
    assert_eq!(e, ScriptError::MemoryLimit);
    // The same script fits a cap that covers it.
    let cfg = LuaConfig {
        memory_limit_bytes: 16 << 20,
        ..cfg
    };
    assert!(LuaScriptBackend::new(src.as_bytes(), &names(), cfg).is_ok());
}

/// Faults with a runtime error whose message contains `needle`.
fn assert_runtime_fault(src: &str, needle: &str) {
    let mut b = new(src);
    Host::new(0).tick(&mut b);
    assert!(b.is_faulted(), "{src}");
    match b.last_error() {
        Some(ScriptError::Runtime(m)) => assert!(m.contains(needle), "{src}: {m}"),
        other => panic!("{src}: {other:?}"),
    }
}

/// Faults with `BudgetExceeded`.
fn assert_budget_fault(src: &str) {
    let mut b = new(src);
    Host::new(0).tick(&mut b);
    assert_eq!(b.last_error(), Some(&ScriptError::BudgetExceeded), "{src}");
}

#[test]
fn string_length_is_capped_at_64_kib() {
    for src in [
        "function on_tick() local s = string.rep('x', 65537) end",
        "function on_tick() local s = 'x' while true do s = s .. s end end",
        "function on_tick() local s = string.rep('x', 40000) local t = s .. s end",
    ] {
        assert_runtime_fault(src, "64 KiB");
    }
    let mut b =
        new("function on_tick() local s = string.rep('x', 65536) pinball.set_score(1, #s) end");
    let q = Host::new(0).tick(&mut b);
    assert_eq!(q.scores.as_slice(), &[(0, 65536)]);
}

// Review B1: C loops the instruction hook never sees are charged by size.
#[test]
fn library_loops_are_charged() {
    for src in [
        "function on_tick() local s = string.rep('', math.maxinteger) end",
        "function on_tick() table.move({}, 1, 1e15, 2) end",
        "function on_tick() table.insert(setmetatable({}, {__len = function() return math.maxinteger - 1 end}), 1, 1) end",
        "function on_tick() table.remove(setmetatable({}, {__len = function() return math.maxinteger end}), 1) end",
        "function on_tick() table.concat({}, '', 1, math.maxinteger) end",
        "function on_tick() return table.unpack({}, 1, 1e7) end",
        "function on_tick() while true do pcall(table.move, {}, 1, 1e15, 2) end end",
        "local t = {} for i = 1, 200000 do t[i] = (i * 7919) % 1000003 end \
         function on_tick() while true do table.sort(t) end end",
        "local t = {} for i = 1, 100000 do t[i] = i end \
         local function g(...) while true do local n = select('#', ...) end end \
         function on_tick() g(table.unpack(t, 1, 100000)) end",
    ] {
        assert_budget_fault(src);
    }
    assert_runtime_fault(
        "function on_tick() local s = string.rep('x', 1 << 28) end",
        "64 KiB",
    );
    // Backtracking patterns: the budget stops them first; with a huge budget
    // the per-call step limit does.
    let pat = "function on_tick() string.find(string.rep('a', 30000), '.-.-.-.-b') end";
    assert_budget_fault(pat);
    let mut b = small(pat, u32::MAX, 16 << 20);
    Host::new(0).tick(&mut b);
    assert!(
        matches!(b.last_error(), Some(ScriptError::Runtime(m)) if m.contains("pattern too complex")),
        "{:?}",
        b.last_error()
    );
}

// Review B4: per-byte work on long strings is charged.
#[test]
fn long_string_work_is_charged() {
    let p = "local s = string.rep('a', 65536) local s2 = string.rep('a', 65535) .. 'a' \
             local s3 = string.rep('a', 65535)";
    for body in [
        "while true do if s == s2 then end end",
        "local t = {[s] = 1} while true do local x = t[s2] end",
        "while true do if s3 < s2 then end end",
        "while true do s:find('b', 1, true) end",
        "while true do local x = s:upper() end",
        "while true do local x = s3 .. 'b' end",
        "while true do utf8.len(s) end",
        "while true do local x = s:byte(1, 60000) end",
        "while true do pcall(error, 'x') end",
        "while true do pcall(function() return s3 + 0 end) end",
    ] {
        assert_budget_fault(&format!("{p} function on_tick() {body} end"));
    }
    // Long numerals are not converted (bounded scan).
    let mut b = new(&format!(
        "{p} function on_tick() pinball.set_score(1, tonumber(string.rep(' ', 300) .. '5') or -1) end"
    ));
    assert_eq!(Host::new(0).tick(&mut b).scores.as_slice(), &[(0, -1)]);
}

// Review B2: to-be-closed variables are rejected.
#[test]
fn to_be_closed_is_rejected() {
    let src = b"function on_tick() local x <close> = nil end";
    assert!(matches!(syntax_check(src), Err(ScriptError::Load(m)) if m.contains("to-be-closed")));
    assert!(matches!(
        LuaScriptBackend::new(src, &names(), LuaConfig::default()),
        Err(ScriptError::Load(_))
    ));
    let mut b = new("");
    assert!(b.apply_reload(src).is_err());
    // The generic-for closing value is refused at run time.
    assert_runtime_fault(
        "function on_tick() for k in next, {}, nil, setmetatable({}, {}) do end end",
        "to-be-closed",
    );
    let mut ok = new("function on_tick() local x <const> = 1 for k, v in pairs({1}) do end end");
    Host::new(0).tick(&mut ok);
    assert!(!ok.is_faulted());
}

// Review B3: iterating a read-only table never hands out the hidden table.
#[test]
fn pairs_does_not_leak_hidden_tables() {
    let mut b = new("function on_tick() \
           local f, t = pairs(pinball.sw) assert(t == pinball.sw) \
           assert(not pcall(rawset, t, 'a', 999)) \
           local _, api = pairs(pinball) assert(api == pinball) \
           local n = 0 for k, v in pairs(pinball.sw) do n = n + 1 assert(pinball.sw[k] == v) end \
           pinball.set_score(1, n) end");
    let q = Host::new(0).tick(&mut b);
    assert!(!b.is_faulted(), "{:?}", b.last_error());
    assert_eq!(q.scores.as_slice(), &[(0, 2)]);
}

// Review B5: the saved state is bounded by the memory cap.
#[test]
fn save_state_bomb_is_dropped_not_allocated() {
    let mut b = new("local s = string.rep('x', 65536) \
         function on_save_state() local t = {} for i = 1, 20000 do t[i] = s end return t end \
         function on_restore_state(t) error('must not be called') end");
    b.apply_reload(b"function on_restore_state(t) error('must not be called') end")
        .unwrap();
}

/// The attack-corpus `savebomb.lua` loads under the engine config (its
/// string is exactly at the 64 KiB cap) and its `on_save_state` result is
/// dropped by the save byte budget on reload, not at load.
#[test]
fn corpus_savebomb_loads_and_its_state_is_dropped_on_reload() {
    let src = include_str!("attack_corpus/savebomb.lua");
    let restore = "saved_n = 7 \
         function on_restore_state(t) saved_n = #t end \
         function on_tick() pinball.set_score(1, saved_n) end";
    for cfg in [LuaConfig::engine(), LuaConfig::default()] {
        let mut b = LuaScriptBackend::new(src.as_bytes(), &names(), cfg)
            .unwrap_or_else(|e| panic!("savebomb must load under {cfg:?}: {e}"));
        let mut h = Host::new(0);
        h.tick(&mut b);
        assert!(!b.is_faulted(), "{:?}", b.last_error());
        b.apply_reload(restore.as_bytes()).unwrap();
        assert_eq!(
            h.tick(&mut b).scores.as_slice(),
            &[(0, 7)],
            "the over-budget state must not reach on_restore_state ({cfg:?})"
        );
    }
    // Control: with 1-byte strings the same 20 000-iteration save loop fits
    // the engine's instruction budget and the copy fits the byte budget, so
    // the state is carried to the new script (whose restore of 20 000
    // entries then runs out of budget and fails the swap). Above, the save
    // loop is identical, so it is the byte budget that drops the state.
    let small_save = src.replacen("string.rep(\"x\", 65536)", "\"x\"", 1);
    assert_ne!(small_save, src);
    let mut b =
        LuaScriptBackend::new(small_save.as_bytes(), &names(), LuaConfig::engine()).unwrap();
    assert_eq!(
        b.apply_reload(restore.as_bytes()),
        Err(ScriptError::BudgetExceeded)
    );
}

// Review B6 helpers: no addresses leak.
#[test]
fn addresses_do_not_leak() {
    assert_runtime_fault(
        "function on_tick() string.format('%p', {}) end",
        "'%p' is not allowed",
    );
    let mut b = new(
        "function on_tick() assert(tostring({}) == 'table' and tostring(print) == 'function') \
         pinball.set_score(1, 1) end",
    );
    assert_eq!(
        Host::new(0).tick(&mut b).scores.len(),
        1,
        "{:?}",
        b.last_error()
    );
}

#[test]
fn weak_tables_are_refused() {
    assert_runtime_fault(
        "function on_tick() setmetatable({}, {__mode = 'k'}) end",
        "weak tables",
    );
}

const PAIRS_SCRIPT: &str = r#"
function on_tick()
  for name, id in pairs(pinball.coil) do pinball.pulse_coil(id, pinball.random(1, 50)) end
  local lamps = {gi = 1, shoot_again = 2, jackpot = 3, left_lane = 4, right_lane = 5, bonus2x = 6}
  for k, v in pairs(lamps) do pinball.set_lamp(v, 1) end
  local t = {} for i = 1, 50 do t["k" .. i] = i end
  local order = 0 for k, v in pairs(t) do order = order * 31 + v order = order % 1000000007 end
  pinball.set_score(1, order)
  local arr = {} for i = 1, 300 do arr[i] = (i * 7919) % 101 end
  table.sort(arr, function(a, b) return a % 7 < b % 7 end)
  pinball.set_score(2, arr[1] * 1000 + arr[150])
end
"#;

fn pairs_trace() -> String {
    let mut b = new(PAIRS_SCRIPT);
    let mut h = Host::new(5);
    let mut out = String::new();
    for _ in 0..20 {
        out.push_str(&format!("{:?}\n", h.tick(&mut b)));
    }
    assert!(!b.is_faulted(), "{:?}", b.last_error());
    out
}

/// Child half of `pairs_order_is_identical_across_processes`.
#[test]
fn pairs_order_child() {
    if std::env::var_os("KHZ_PAIRS_CHILD").is_some() {
        println!("BEGIN\n{}END", pairs_trace());
    }
}

// Review B6: string hashing is seeded with a constant, so `pairs` order (and
// with it the command queues) is the same in every process.
#[test]
fn pairs_order_is_identical_across_processes() {
    let run = || {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "pairs_order_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("KHZ_PAIRS_CHILD", "1")
            .output()
            .unwrap();
        let s = String::from_utf8(out.stdout).unwrap();
        let a = s.find("BEGIN").expect("child output");
        let e = s.find("END").unwrap();
        s[a..e].to_string()
    };
    let first = run();
    assert!(first.len() > 100);
    assert_eq!(first, run());
    assert_eq!(first, format!("BEGIN\n{}", pairs_trace()));
}

#[test]
fn math_random_rejects_surplus_arguments() {
    assert_runtime_fault(
        "function on_tick() math.random(1, 2, nil, 4) end",
        "wrong number",
    );
    assert_runtime_fault(
        "function on_tick() math.random(1, 2, nil) end",
        "wrong number",
    );
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

/// A reload between two batches of tilt releases must not strand the coils
/// still waiting for queue room: the drain continues under the new script.
#[test]
fn tilt_releases_keep_draining_after_a_reload_mid_drain() {
    let mut b = new(
        "n = 0 function on_tick() n = n + 1 if n == 1 then for i = 1, 32 do pinball.hold_coil(i, true) end \
         elseif n == 2 then for i = 33, 40 do pinball.hold_coil(i, true) end \
         else error('x') end end",
    );
    let mut h = Host::new(0);
    assert_eq!(h.tick(&mut b).coil_holds.len(), 32);
    assert_eq!(h.tick(&mut b).coil_holds.len(), 8);
    let mut released = Vec::new();
    let q = h.tick(&mut b);
    assert!(b.is_faulted());
    assert_eq!(q.coil_holds.len(), 32);
    released.extend(q.coil_holds.iter().copied());
    b.apply_reload(b"function on_tick() pinball.set_lamp(1, 1) end")
        .unwrap();
    assert!(!b.is_faulted());
    for _ in 0..3 {
        let q = h.tick(&mut b);
        assert_eq!(
            q.lamps.len(),
            1,
            "the new script runs while the drain goes on"
        );
        released.extend(q.coil_holds.iter().copied());
    }
    assert!(released.iter().all(|&(_, on)| !on), "{released:?}");
    let mut ids: Vec<u16> = released.iter().map(|&(c, _)| c.0).collect();
    ids.sort_unstable();
    let want: Vec<u16> = (1..=40).collect();
    assert_eq!(ids, want, "every coil held at the fault is released once");
}

/// A healthy reload (no fault) still hands the held coils to the new script:
/// nothing is released behind its back.
#[test]
fn healthy_reload_keeps_holds() {
    let mut b = new("function on_tick() for i = 1, 40 do pinball.hold_coil(i, true) end end");
    let mut h = Host::new(0);
    h.tick(&mut b);
    b.apply_reload(b"function on_tick() end").unwrap();
    for _ in 0..3 {
        assert!(h.tick(&mut b).is_empty());
    }
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
    for entry in ["rules.py", "rules"] {
        match load_backend(entry, b"", &names()) {
            Err(ScriptError::UnsupportedBackend(m)) => assert!(m.contains(entry)),
            _ => panic!("{entry} should be unsupported"),
        }
    }
    assert!(matches!(
        load_backend("x.wasm", b"", &names()),
        Err(ScriptError::Load(_))
    ));
    let minimal = br#"(module
  (memory (export "memory") 1)
  (func (export "on_init"))
  (func (export "on_tick"))
  (func (export "on_switch_edge") (param i32 i32)))"#;
    assert!(load_backend("x.wasm", minimal, &names()).is_ok());
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
    let tmp = TempDir::new("watch");
    let dir = tmp.path();
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
    // The watcher goes before the directory it watches.
    drop(w);
    assert!(ReloadWatcher::new(dir.join("missing_dir/rules.lua")).is_err());
    assert!(ReloadWatcher::new("/").is_err());
    drop(tmp);
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

// Review B8: a release dropped by the rollback of a faulting callback is
// still emitted by the tilt release.
#[test]
fn rollback_never_strands_a_held_coil() {
    let mut b = new("n = 0 function on_tick() n = n + 1 \
         if n == 1 then pinball.hold_coil(3, true) \
         else pinball.hold_coil(3, false) pinball.hold_coil(4, true) error('x') end end");
    let mut h = Host::new(0);
    assert_eq!(h.tick(&mut b).coil_holds.as_slice(), &[(CoilId(3), true)]);
    let q = h.tick(&mut b);
    assert!(b.is_faulted());
    assert!(
        q.coil_holds.contains(&(CoilId(3), false)),
        "{:?}",
        q.coil_holds
    );
    assert!(
        q.coil_holds.iter().all(|&(_, on)| !on),
        "{:?}",
        q.coil_holds
    );
    assert!(h.tick(&mut b).is_empty());
}

// Review N6: overflow counts of a rolled-back callback are rolled back too.
#[test]
fn rollback_restores_overflow_count() {
    let mut b = new(
        "n = 0 function on_tick() n = n + 1 for i = 1, 40 do pinball.pulse_coil(i, 1) end \
         if n == 2 then error('x') end end",
    );
    let mut h = Host::new(0);
    h.tick(&mut b);
    assert_eq!(b.overflow_count(), 8);
    let q = h.tick(&mut b);
    assert!(b.is_faulted());
    assert!(q.coil_pulses.is_empty());
    assert_eq!(b.overflow_count(), 8);
}

// Review B7: binary packing is not available.
#[test]
fn string_pack_is_removed() {
    let mut b = new(
        "function on_tick() assert(string.pack == nil and string.unpack == nil \
         and string.packsize == nil and ('').pack == nil) pinball.set_score(1, 1) end",
    );
    assert_eq!(
        Host::new(0).tick(&mut b).scores.len(),
        1,
        "{:?}",
        b.last_error()
    );
}

// Review N7: reload end to end with many references to a string just under
// 64 KiB: over the cap the state is dropped, under it the state is restored.
#[test]
fn reload_with_large_saved_state() {
    let src = |n: usize| {
        format!(
            "local s = string.rep('x', 65535) \
             function on_save_state() local t = {{}} for i = 1, {n} do t[i] = s end return t end \
             function on_restore_state(t) got = #t end \
             function on_tick() pinball.set_score(1, got or -1) end"
        )
    };
    // 20 000 × 64 KiB ≫ 16 MiB cap: no state, restore not called.
    let mut b = new(&src(20_000));
    b.apply_reload(src(20_000).as_bytes()).unwrap();
    assert_eq!(Host::new(0).tick(&mut b).scores.as_slice(), &[(0, -1)]);
    // 100 × 64 KiB = 6.4 MiB < 16 MiB: restored (each copy is its own string).
    let mut b = new(&src(100));
    b.apply_reload(src(100).as_bytes()).unwrap();
    assert_eq!(Host::new(0).tick(&mut b).scores.as_slice(), &[(0, 100)]);
}

// Review B9: expensive single operations carry extra cost, so a small budget
// stops a loop of them after few iterations.
#[test]
fn expensive_operations_are_charged() {
    for body in [
        "local x = string.format('%99.99f', 1e300)",
        "local a, b = 1e308, 3e-308 local x = a % b",
        "local x = math.fmod(1e308, 1e-308)",
        "local x = tostring(1e300)",
        "local x = tonumber(string.rep('9', 200))",
        "pcall(pinball.pulse_coil)",
        "pcall(collectgarbage, 'collect')",
        "local x = string.format('%q', string.rep('\\0', 1000))",
    ] {
        // Each iteration costs ≥ 256 units, so 40 of them exceed 10 000,
        // although the loop itself is only a few hundred instructions.
        let src = format!("function on_tick() for i = 1, 40 do {body} end end");
        let mut b = small(&src, 10_000, 4 << 20);
        Host::new(0).tick(&mut b);
        assert_eq!(b.last_error(), Some(&ScriptError::BudgetExceeded), "{body}");
        let mut ok = small(&src, 100_000, 4 << 20);
        Host::new(0).tick(&mut ok);
        assert!(!ok.is_faulted(), "{body}: {:?}", ok.last_error());
    }
}

/// Runs `setup` (spread over ticks, each under the engine budget) and then a
/// bounded `attack` that is cheap in VM instructions; returns the fault, if
/// any, of the attack tick. Used for the round-3 charging tests.
fn engine_attack(setup: &str, setup_ticks: usize, attack: &str) -> Option<ScriptError> {
    let src = format!(
        "phase = 0 ticks = 0 \
         function on_tick() if phase == 0 then {setup} ticks = ticks + 1 \
           if ticks >= {setup_ticks} then phase = 1 end \
         else {attack} end end"
    );
    let mut b = LuaScriptBackend::new(src.as_bytes(), &names(), LuaConfig::engine()).unwrap();
    let mut h = Host::new(0);
    for _ in 0..setup_ticks {
        h.tick(&mut b);
        assert!(!b.is_faulted(), "set-up faulted: {:?}", b.last_error());
    }
    h.tick(&mut b);
    b.last_error().cloned()
}

#[test]
fn engine_config_values() {
    let c = LuaConfig::engine();
    assert_eq!(c.instruction_budget, 100_000);
    assert_eq!(c.memory_limit_bytes, 2 << 20);
    assert_ne!(c, LuaConfig::default());
}

// Review B10: `next` over an emptied array part charges the slots it skips.
#[test]
fn next_charges_skipped_slots() {
    let setup = "t = t or {} n = n or 0 m = m or 0 \
                 if n < 40000 then for i = 1, 8000 do n = n + 1 t[n] = true end \
                 else for i = 1, 8000 do m = m + 1 t[m] = nil end end";
    let attack = "local nx = next for i = 1, 40 do nx(t) end";
    assert_eq!(
        engine_attack(setup, 10, attack),
        Some(ScriptError::BudgetExceeded)
    );
    assert_eq!(engine_attack(setup, 10, "local nx = next nx(t)"), None);
}

// Review B11: emergency full collections near the memory cap are charged.
#[test]
fn emergency_gc_is_charged() {
    let setup = "keep = keep or {} for i = 1, 3000 do keep[#keep + 1] = {i} end";
    // Live data near the 2 MiB cap; each 60 KB string then forces collections.
    let mut b = LuaScriptBackend::new(
        format!(
            "phase = 0 function on_tick() if phase == 0 then {setup} \
             if collectgarbage('count') > 1700 then phase = 1 end \
             else for i = 1, 30 do local s = string.rep('x', 60000) end end end"
        )
        .as_bytes(),
        &names(),
        LuaConfig::engine(),
    )
    .unwrap();
    let mut h = Host::new(0);
    for _ in 0..200 {
        h.tick(&mut b);
        if b.is_faulted() {
            break;
        }
    }
    assert_eq!(b.last_error(), Some(&ScriptError::BudgetExceeded));
}

// Review B12: `string.rep` is charged for the bytes it copies.
#[test]
fn string_rep_is_charged_by_size() {
    let mut b = small(
        "function on_tick() for i = 1, 6 do local s = string.rep('x', 65000) end end",
        10_000,
        4 << 20,
    );
    Host::new(0).tick(&mut b);
    assert_eq!(b.last_error(), Some(&ScriptError::BudgetExceeded));
    // The doubling copy is exact, with and without separators.
    let mut b = new("function on_tick() \
           assert(string.rep('ab', 5, ',') == 'ab,ab,ab,ab,ab') \
           assert(string.rep('abc', 7) == 'abcabcabcabcabcabcabc') \
           assert(string.rep('x', 1) == 'x' and string.rep('', 3, '-') == '--') \
           assert(#string.rep('xyz', 21845) == 65535) \
           assert(string.rep('a', 3, '') == 'aaa') \
           pinball.set_score(1, 1) end");
    assert_eq!(
        Host::new(0).tick(&mut b).scores.len(),
        1,
        "{:?}",
        b.last_error()
    );
}

// Review B13: buffers cannot grow past the string cap.
#[test]
fn buffers_stop_at_the_string_cap() {
    let mut b = new(
        "local s = string.rep('x', 1000) local t = {} for i = 1, 80 do t[i] = s end \
         function on_tick() \
           local ok, e = pcall(table.concat, t) assert(not ok and e:find('64 KiB'), e) \
           ok, e = pcall(string.gsub, string.rep('a', 80), 'a', s) assert(not ok and e:find('64 KiB'), e) \
           ok, e = pcall(string.gsub, string.rep('a', 80), 'a', {a = s}) assert(not ok and e:find('64 KiB'), e) \
           ok, e = pcall(string.format, string.rep('%s', 80), table.unpack(t)) assert(not ok and e:find('64 KiB'), e) \
           assert(#table.concat(t, '', 1, 65) == 65000) \
           pinball.set_score(1, 1) end",
    );
    assert_eq!(
        Host::new(0).tick(&mut b).scores.len(),
        1,
        "{:?}",
        b.last_error()
    );
}

// Review B14: table resizes are charged by size. A hash part of exactly
// 32 768 keys is built under a 4 000-instruction budget; one more key then
// rehashes everything, costing (32 768 + 65 536) / 16 = 6 144 > 4 000.
#[test]
fn table_resize_is_charged() {
    let run = |attack: &str| {
        let src = format!(
            "t = {{}} n = 0 function on_tick() \
             if n < 32768 then for i = 1, 64 do n = n + 1 t[-n] = true end \
             else {attack} end end"
        );
        let mut b = small(&src, 4_000, 16 << 20);
        let mut h = Host::new(0);
        for _ in 0..512 {
            h.tick(&mut b);
            assert!(!b.is_faulted(), "set-up: {:?}", b.last_error());
        }
        h.tick(&mut b);
        b.last_error().cloned()
    };
    assert_eq!(run("t[0.5] = true"), Some(ScriptError::BudgetExceeded));
    assert_eq!(run("t[-1] = false"), None);
}
