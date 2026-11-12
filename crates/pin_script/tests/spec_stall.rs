//! Spec: "Faults never panic or stall the tick". These scenarios hide script
//! code in places a naive per-callback budget does not reach (message
//! handlers, finalizers). They live in their own binary because a regression
//! here shows up as a hang rather than a failed assertion.

mod spec_support;

use pin_core::TableScriptBackend;
use spec_support::{Host, backend_with, cfg};

const FAULTS: &[u8] = include_bytes!("fixtures/spec_faults.lua");
const MIB: usize = 1024 * 1024;

/// `while true do xpcall(function() error("x") end, <handler that loops>) end`
#[test]
fn xpcall_with_a_looping_message_handler_is_aborted() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 50_000));
    let mut h = Host::new(1);
    h.case(&mut b, 25);
    assert!(b.is_faulted());
    assert_eq!(b.fault_count(), 1);
    h.tick(&mut b);
    assert!(h.cmd.is_empty());
}

/// A single xpcall whose handler loops, without the outer retry loop.
#[test]
fn a_single_looping_message_handler_is_aborted() {
    let src = b"function on_tick()
                  xpcall(function() error('x') end, function() while true do end end)
                  while true do end
                end";
    let mut b = backend_with(src, cfg(16 * MIB, 50_000));
    let mut h = Host::new(1);
    h.tick(&mut b);
    assert!(b.is_faulted());
}

/// 200 objects whose `__gc` never returns become garbage; later ticks (and the
/// per-tick GC step) must still return.
#[test]
fn looping_finalizers_do_not_stall_later_ticks() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 16);
    for _ in 0..300 {
        h.tick(&mut b);
        h.case(&mut b, 27);
    }
}

/// Garbage with looping finalizers created while memory pressure forces
/// collections inside the callback itself.
#[test]
fn looping_finalizers_under_memory_pressure_do_not_stall() {
    let src = b"function on_tick()
                  for i = 1, 50 do
                    setmetatable({}, { __gc = function() while true do end end })
                  end
                  local t = {}
                  for i = 1, 64 do t[i] = string.rep('p', 32768) .. i end
                end";
    let mut b = backend_with(src, cfg(4 * MIB, 50_000));
    let mut h = Host::new(1);
    for _ in 0..200 {
        h.tick(&mut b);
    }
}

#[test]
fn looping_finalizers_do_not_stall_reload() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 16);
    let _ = b.apply_reload(b"function on_tick() pinball.show_scene(1) end");
    for _ in 0..50 {
        h.tick(&mut b);
    }
}

/// Closing a Lua state runs every pending finalizer.
#[test]
fn looping_finalizers_do_not_stall_drop() {
    let src = b"keep = {}
                function on_tick()
                  for i = 1, 20 do
                    keep[#keep + 1] = setmetatable({}, { __gc = function() while true do end end })
                  end
                end";
    let mut b = backend_with(src, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.tick(&mut b);
    drop(b);
}

/// A finalizer that raises instead of looping must not take the host down.
#[test]
fn erroring_finalizers_do_not_panic() {
    let src = b"function on_tick()
                  for i = 1, 100 do
                    setmetatable({}, { __gc = function() error('in gc') end })
                  end
                  pinball.show_scene(1)
                end";
    let mut b = backend_with(src, cfg(16 * MIB, 50_000));
    let mut h = Host::new(1);
    for _ in 0..300 {
        h.tick(&mut b);
    }
}

// ---------------------------------------------------------------------------
// Library loops the instruction hook cannot see (Orchestrator decision
// "4.2 review: patched vendored Lua"). Every case must fault, not hang.

/// Runs `body` as an unprotected `on_tick` under a 50 000 instruction budget;
/// it must fault before reaching the marker command.
fn assert_faults(setup: &str, body: &str) {
    let src = format!("{setup}\nfunction on_tick() {body} pinball.show_scene(1) end");
    let mut b = backend_with(src.as_bytes(), cfg(16 * MIB, 50_000));
    let mut h = Host::new(1);
    h.tick(&mut b);
    assert!(b.is_faulted(), "`{body}` must fault the script");
    assert!(
        h.cmd.display_scenes.is_empty(),
        "`{body}` did not abort the callback"
    );
    assert_eq!(b.fault_count(), 1);
    // The backend recovers afterwards.
    b.apply_reload(b"function on_tick() pinball.show_scene(2) end")
        .expect("recovery reload");
    h.tick(&mut b);
    assert_eq!(h.cmd.display_scenes.len(), 1);
}

/// The same call retried forever under pcall must also end in a fault.
fn assert_faults_under_pcall(setup: &str, body: &str) {
    let src =
        format!("{setup}\nfunction on_tick() while true do pcall(function() {body} end) end end");
    let mut b = backend_with(src.as_bytes(), cfg(16 * MIB, 50_000));
    let mut h = Host::new(1);
    h.tick(&mut b);
    assert!(
        b.is_faulted(),
        "`{body}` retried under pcall must fault the script"
    );
}

const HUGE_LEN: &str = "
local function huge(n)
  return setmetatable({}, { __len = function() return n end })
end
local function bottomless(n)
  return setmetatable({}, {
    __len = function() return n end,
    __index = function() return '' end,
    __newindex = function() end,
  })
end";

macro_rules! must_fault {
    ($($name:ident => $body:expr;)*) => {$(
        #[test]
        fn $name() {
            assert_faults(HUGE_LEN, $body);
        }
    )*};
}

must_fault! {
    rep_of_empty_string_maxinteger_times => "local s = string.rep('', math.maxinteger)";
    rep_of_empty_string_with_separator => "local s = string.rep('', math.maxinteger, '')";
    rep_one_billion => "local s = string.rep('x', 1e9)";
    table_move_huge_range => "table.move({}, 1, 1e15, 2)";
    table_move_huge_range_between_tables => "table.move({}, 1, 1e15, 1, {})";
    table_move_huge_range_backwards => "table.move({}, 2, 1e15, 1)";
    table_remove_with_len_maxinteger => "table.remove(huge(math.maxinteger), 1)";
    table_remove_with_len_maxinteger_minus_one => "table.remove(huge(math.maxinteger - 1), 1)";
    table_remove_bottomless_maxinteger_minus_one =>
        "table.remove(bottomless(math.maxinteger - 1), 1)";
    table_insert_with_len_maxinteger => "table.insert(huge(math.maxinteger), 1, 'v')";
    table_insert_with_len_maxinteger_minus_one =>
        "table.insert(huge(math.maxinteger - 1), 1, 'v')";
    table_insert_bottomless_maxinteger_minus_one =>
        "table.insert(bottomless(math.maxinteger - 1), 1, 'v')";
    table_concat_huge_range => "local s = table.concat({}, '', 1, 1e15)";
    table_concat_huge_range_of_empty_strings =>
        "local s = table.concat(bottomless(0), '', 1, math.maxinteger)";
    table_concat_huge_len_of_empty_strings =>
        "local s = table.concat(bottomless(math.maxinteger))";
    table_unpack_huge_range => "local a = table.unpack({}, 1, 1e15)";
    table_unpack_ten_million => "local a = table.unpack({}, 1, 1e7)";
    table_unpack_huge_len => "local a = table.unpack(huge(math.maxinteger))";
    table_sort_huge_len => "table.sort(bottomless(1e15))";
    find_backtracking_30000 => "local r = string.find(string.rep('a', 30000), '.-.-.-.-b')";
    find_backtracking_60000 => "local r = string.find(string.rep('a', 60000), '.-.-.-.-b')";
    match_backtracking_60000 => "local r = string.match(string.rep('a', 60000), '.-.-.-.-b')";
    gsub_backtracking_60000 => "local r = string.gsub(string.rep('a', 60000), '.-.-.-.-b', '')";
    gmatch_backtracking_60000 =>
        "for m in string.gmatch(string.rep('a', 60000), '.-.-.-.-b') do end";
    find_method_backtracking_60000 => "local r = (('a'):rep(60000)):find('a*a*a*a*a*b')";
}

#[test]
fn library_loops_retried_under_pcall_still_fault() {
    for body in [
        "string.rep('', math.maxinteger)",
        "table.move({}, 1, 1e15, 2)",
        "table.remove(huge(math.maxinteger - 1), 1)",
        "table.insert(huge(math.maxinteger - 1), 1, 'v')",
        "table.concat(bottomless(0), '', 1, math.maxinteger)",
        "string.find(string.rep('a', 60000), '.-.-.-.-b')",
    ] {
        assert_faults_under_pcall(HUGE_LEN, body);
    }
}

#[test]
fn library_loops_at_load_time_do_not_hang_construction() {
    for src in [
        &b"string.rep('', math.maxinteger)"[..],
        b"table.move({}, 1, 1e15, 2)",
        b"string.find(string.rep('a', 60000), '.-.-.-.-b')",
    ] {
        match pin_script::lua::LuaScriptBackend::new(
            src,
            &spec_support::names(),
            cfg(16 * MIB, 50_000),
        ) {
            Err(_) => {}
            Ok(b) => assert!(b.is_faulted()),
        }
    }
}

/// Ordinary, small uses of the same library functions keep working.
#[test]
fn bounded_library_calls_still_work() {
    let src = b"function on_tick()
                  local t = { 5, 3, 1, 4, 2 }
                  table.sort(t)
                  table.insert(t, 6)
                  table.insert(t, 1, 0)
                  local last = table.remove(t)
                  local first = table.remove(t, 1)
                  local moved = table.move(t, 1, #t, 2, {})
                  local s = table.concat(t, ',')
                  local a, b = table.unpack(t, 1, 2)
                  local i, j = string.find(string.rep('a', 1000) .. 'b', '.-b')
                  local ok = (last == 6 and first == 0 and s == '1,2,3,4,5' and a == 1 and b == 2
                    and moved[2] == 1 and moved[6] == 5 and i == 1 and j == 1001
                    and string.rep('ab', 3, '-') == 'ab-ab-ab'
                    and ('x'):rep(0) == '' and string.rep('', 1000) == '')
                  pinball.persist_set(0, ok and 1 or 0)
                end";
    let mut b = backend_with(src, cfg(16 * MIB, 50_000));
    let mut h = Host::new(1);
    for _ in 0..50 {
        h.tick(&mut b);
        assert!(!b.is_faulted());
        assert_eq!(spec_support::persisted(&h.cmd, 0), Some(1));
    }
}

// ---------------------------------------------------------------------------
// on_save_state returning pathological state: the host never aborts and
// apply_reload never panics; reload proceeds without state or errors cleanly.

const NEW_SCRIPT: &[u8] = b"local restored = 0
function on_restore_state(s) restored = restored + 1 end
function on_tick() pinball.set_score(2, 7) pinball.persist_set(0, restored) end";

/// Runs `old` for `ticks` ticks, reloads into `NEW_SCRIPT` and checks the
/// backend is in one of the two clean outcomes.
fn reload_with_pathological_state(old: &str, ticks: u32) {
    let mut b = backend_with(old.as_bytes(), cfg(16 * MIB, 1_000_000));
    let mut h = Host::new(1);
    for _ in 0..ticks {
        h.tick(&mut b);
        assert!(
            !b.is_faulted(),
            "the old script must stay healthy while building its state"
        );
    }
    match b.apply_reload(NEW_SCRIPT) {
        Ok(()) => {
            assert!(
                !b.is_faulted(),
                "a successful reload leaves a healthy backend"
            );
            h.tick(&mut b);
            assert!(!b.is_faulted());
            assert_eq!(
                spec_support::score(&h.cmd, 1),
                Some(7),
                "the new script runs"
            );
        }
        Err(_) => {
            // Clean error: the old script is still installed and ticking.
            h.tick(&mut b);
            if !b.is_faulted() {
                assert_eq!(
                    spec_support::score(&h.cmd, 0),
                    Some(1),
                    "the old script still runs"
                );
            }
        }
    }
    // Either way a plain reload brings the backend to a known-good script.
    let _ = b.apply_reload(b"function on_tick() pinball.show_scene(3) end");
    h.tick(&mut b);
}

#[test]
fn save_state_with_many_references_to_one_large_string() {
    reload_with_pathological_state(
        "local big = string.rep('s', 60000)
         function on_tick() pinball.set_score(1, 1) end
         function on_save_state()
           local t = {}
           for i = 1, 5000 do t[i] = big end
           return t
         end",
        2,
    );
}

#[test]
fn save_state_with_many_references_to_one_large_string_as_keys_and_nested() {
    reload_with_pathological_state(
        "local big = string.rep('s', 60000)
         function on_tick() pinball.set_score(1, 1) end
         function on_save_state()
           local t = {}
           for i = 1, 2000 do t[i] = { name = big, [big] = i } end
           return t
         end",
        2,
    );
}

#[test]
fn save_state_with_a_huge_flat_table() {
    reload_with_pathological_state(
        "local state = {}
         function on_tick()
           for i = 1, 10000 do state[#state + 1] = i end
           pinball.set_score(1, 1)
         end
         function on_save_state() return state end",
        20,
    );
}

#[test]
fn save_state_with_a_huge_string_keyed_table() {
    reload_with_pathological_state(
        "local state, n = {}, 0
         function on_tick()
           for i = 1, 2000 do n = n + 1 state['key_' .. n] = { n, 'v' .. n } end
           pinball.set_score(1, 1)
         end
         function on_save_state() return state end",
        10,
    );
}

#[test]
fn save_state_with_a_very_deep_table() {
    reload_with_pathological_state(
        "local root = {}
         local tip = root
         function on_tick()
           for i = 1, 5000 do local n = {} tip.next = n tip = n end
           pinball.set_score(1, 1)
         end
         function on_save_state() return root end",
        20,
    );
}

#[test]
fn save_state_with_a_wide_shared_dag() {
    // 2^40 paths through 40 shared levels: exponential for a naive deep copy.
    reload_with_pathological_state(
        "function on_tick() pinball.set_score(1, 1) end
         function on_save_state()
           local level = { leaf = true }
           for i = 1, 40 do level = { a = level, b = level } end
           return level
         end",
        2,
    );
}

#[test]
fn save_state_built_inside_the_hook_past_the_memory_cap() {
    reload_with_pathological_state(
        "function on_tick() pinball.set_score(1, 1) end
         function on_save_state()
           local t = {}
           while true do t[#t + 1] = string.rep('m', 60000) .. #t end
         end",
        2,
    );
}
