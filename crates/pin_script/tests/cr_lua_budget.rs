//! Regression tests for CodeRabbit PR #3 findings on the patched vendored Lua
//! (third_party/lua-src/KHZ_PATCHES.md, patches 23-26 and 28): work done below
//! one VM instruction that used to spend no budget, deferred cost charged to
//! the wrong state, and `xpcall` message handlers run outside the budget. Each pathological script must
//! fault on the budget (or the pattern step limit) instead of stalling the
//! tick. As in `spec_stall.rs`, a regression shows up as a long-running test
//! rather than a failed assertion, so there are no wall-clock assertions; the
//! controls show the same shape faults only because of the new charges.

mod spec_support;

use spec_support::{Host, backend_with, cfg};

const MIB: usize = 1024 * 1024;

/// Runs `body` once as an unprotected `on_tick`; returns whether the script
/// faulted (and checks that a fault aborted the callback).
fn faults(setup: &str, body: &str, budget: u32) -> bool {
    let src = format!("{setup}\nfunction on_tick() {body} pinball.show_scene(1) end");
    let mut b = backend_with(src.as_bytes(), cfg(16 * MIB, budget));
    let mut h = Host::new(1);
    h.tick(&mut b);
    if b.is_faulted() {
        assert!(
            h.cmd.display_scenes.is_empty(),
            "`{body}` faulted without aborting the callback"
        );
        assert_eq!(b.fault_count(), 1);
    } else {
        assert_eq!(h.cmd.display_scenes.len(), 1);
    }
    b.is_faulted()
}

/// The same body retried forever under `pcall` must also end in a fault.
fn faults_under_pcall(setup: &str, body: &str) {
    let src =
        format!("{setup}\nfunction on_tick() while true do pcall(function() {body} end) end end");
    let mut b = backend_with(src.as_bytes(), cfg(16 * MIB, 50_000));
    let mut h = Host::new(1);
    h.tick(&mut b);
    assert!(b.is_faulted(), "`{body}` retried under pcall must fault");
}

// ldo.c: a cyclic `__call` chain retries `tryfuncTM` without running an
// instruction, shifting one more stack slot each time (quadratic).

const CYCLIC_CALL: &str = "local mt = {}
local cyc = setmetatable({}, mt)
mt.__call = cyc
local mt2 = {}
local one = setmetatable({}, mt2)
mt2.__call = function(self, a) return a end";

#[test]
fn cyclic_call_metamethod_faults() {
    assert!(faults(CYCLIC_CALL, "cyc(1, 2, 3)", 50_000));
    faults_under_pcall(CYCLIC_CALL, "cyc()");
}

#[test]
fn ordinary_call_metamethod_still_works() {
    assert!(!faults(
        CYCLIC_CALL,
        "for i = 1, 1000 do assert(one(i) == i) end",
        50_000
    ));
}

// lstrlib.c: `%b` and bracket-class scans did variable-length work below one
// matching step.

#[test]
fn balanced_match_over_64k_open_brackets_faults() {
    assert!(faults(
        "local s = string.rep('(', 65536)",
        "local r = string.match(s, '%b()')",
        1_000_000
    ));
    faults_under_pcall(
        "local s = string.rep('(', 65536)",
        "local r = string.find(s, '%b()')",
    );
}

#[test]
fn long_bracket_class_scans_fault() {
    let setup = "local s = string.rep('b', 60000)
local p = '[' .. string.rep('a', 60000) .. ']'";
    assert!(faults(setup, "local r = string.find(s, p)", 1_000_000));
    assert!(faults(
        setup,
        "local r = string.find(s, '%f' .. p)",
        1_000_000
    ));
}

#[test]
fn ordinary_patterns_still_work() {
    assert!(!faults(
        "local s = string.rep('(x)', 1000)",
        "assert(string.match(s, '%b()') == '(x)')
         assert(select(2, string.gsub(s, '%b()', '')) == 1000)
         assert(string.find(string.rep('b', 1000) .. 'a', '[%w_]*a') == 1)
         assert(string.match('abcabc', '(abc)%1') == 'abc')",
        1_000_000
    ));
}

// ltable.c: integer keys hashed with a plain modulo can be chosen to share one
// chain; every lookup of an absent key then walks the whole chain.

/// 1024 keys `i * 1023`: with 1024 hash nodes (modulus 1023) all of them are
/// in bucket 0. `plain` holds the same number of non-colliding keys.
const COLLIDING: &str = "local t = {}
for i = 1, 1024 do t[i * 1023] = true end
local plain = {}
for i = 1, 1024 do plain[i * 1024 + 7] = true end";

#[test]
fn colliding_hash_chain_lookups_fault() {
    // ~3 instructions per iteration: well inside the instruction budget, but
    // 200 000 walks of a 1024-node chain.
    assert!(faults(
        COLLIDING,
        "for i = 1, 200000 do local x = t[0] end",
        1_000_000
    ));
}

#[test]
fn ordinary_hash_lookups_do_not_fault() {
    assert!(!faults(
        COLLIDING,
        "for i = 1, 200000 do local x = plain[0] end",
        1_000_000
    ));
}

// xpcall: stock Lua runs the message handler at the error point, and for the
// budget error that is inside the count hook (hooks disabled), so a looping
// handler would never return. The sandbox's `xpcall` runs the handler after
// unwinding, under the budget. A hang here fails through the watchdog.

/// Runs `body` as `on_tick` on its own thread; returns whether it faulted,
/// failing if the tick has not returned within 10 s.
fn faults_with_watchdog(body: &'static str) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(faults("", body, 50_000));
    });
    match rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(faulted) => faulted,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            panic!("on_tick did not return within 10 s: {body}")
        }
        Err(e) => panic!("on_tick panicked: {body}: {e}"),
    }
}

#[test]
fn looping_xpcall_message_handler_faults() {
    for body in [
        "xpcall(function() while true do end end, function() while true do end end)",
        "xpcall(function() while true do end end, function(e)
           if tostring(e):find('budget', 1, true) then while true do end end
           return e
         end)",
        "while true do
           xpcall(function() while true do end end, function() while true do end end)
         end",
    ] {
        assert!(faults_with_watchdog(body), "`{body}` must fault");
    }
}

#[test]
fn ordinary_xpcall_still_works() {
    assert!(!faults_with_watchdog(
        "local ok, e = xpcall(function() error('boom', 0) end, function(e) return 'h:' .. e end)
         assert(not ok and e == 'h:boom')
         local ok2, a, b = xpcall(function(x, y) return x + y, x * y end, print, 3, 4)
         assert(ok2 and a == 7 and b == 12)"
    ));
}

// lstate.c/lstring.c (patch 28): the thread-local pending cost (long-string
// compares in table lookups, which have no lua_State) is kept per state: a
// state's pending cost is charged to that state only, whichever states set or
// spend their budgets in between.

unsafe extern "C-unwind" {
    fn lua_khzsetbudget(state: *mut mlua::ffi::lua_State, n: i64);
    fn lua_khzspend(state: *mut mlua::ffi::lua_State, n: i64) -> i64;
}

const BUDGET: i64 = 1_000_000_000;
/// 2000 lookups of a 60 000-byte key equal to (but not the same string as)
/// the stored one: 2000 * (60000 / 64) = 1 875 000 pending cost.
const LOOKUPS_COST: i64 = 2000 * (60000 / 64);

struct State {
    lua: mlua::Lua,
    raw: *mut mlua::ffi::lua_State,
    /// Runs the lookups without calling any C function (so nothing drains
    /// the pending cost before the next spend).
    lookups: mlua::Function,
}

impl State {
    fn new() -> State {
        let lua = mlua::Lua::new();
        // SAFETY: only reads the state pointer mlua runs on.
        let raw = unsafe {
            let mut p = std::ptr::null_mut();
            lua.exec_raw::<()>((), |state| p = state).unwrap();
            p
        };
        lua.load("k1 = string.rep('x', 60000) k2 = string.rep('x', 59999) .. 'x' t = { [k1] = 1 }")
            .exec()
            .unwrap();
        let lookups = lua
            .load("local t, k = t, k2 for i = 1, 2000 do local x = t[k] end")
            .into_function()
            .unwrap();
        State { lua, raw, lookups }
    }

    fn set_budget(&self) {
        // SAFETY: `raw` is the live main state of `self.lua`.
        unsafe { lua_khzsetbudget(self.raw, BUDGET) }
    }

    /// Cost charged since `set_budget` (draining this state's pending cost).
    fn charged(&self) -> i64 {
        // SAFETY: as above.
        BUDGET - unsafe { lua_khzspend(self.raw, 0) }
    }
}

/// `worker` accrues pending cost, `other` spends, then `worker` spends.
/// `worker_owns_first`: the worker set its budget last before running (so
/// it already owned the pending cost; the old code discarded it at the
/// other state's spend), otherwise the other state did (the old code charged
/// the worker's cost to it).
fn pending_cost_is_charged_to_its_state(worker: &State, other: &State, worker_owns_first: bool) {
    if worker_owns_first {
        other.set_budget();
        worker.set_budget();
    } else {
        worker.set_budget();
        other.set_budget();
    }
    worker.lookups.call::<()>(()).unwrap();
    assert_eq!(other.charged(), 0, "the other state was charged");
    let w = worker.charged();
    assert!(
        (LOOKUPS_COST..LOOKUPS_COST + 1000).contains(&w),
        "worker charged {w}, expected about {LOOKUPS_COST}"
    );
    assert_eq!(other.charged(), 0);
}

#[test]
fn pending_cost_stays_with_the_state_that_produced_it() {
    let a = State::new();
    let b = State::new();
    for owns_first in [true, false] {
        pending_cost_is_charged_to_its_state(&a, &b, owns_first);
        pending_cost_is_charged_to_its_state(&b, &a, owns_first);
    }
}

#[test]
fn pending_cost_of_a_nested_state_is_kept_apart() {
    // A host callback of A runs B's lookups between two runs of A's own.
    let a = State::new();
    let b = std::rc::Rc::new(State::new());
    let b2 = std::rc::Rc::clone(&b);
    let run_b = a
        .lua
        .create_function(move |_, ()| b2.lookups.call::<()>(()))
        .unwrap();
    a.lua.globals().set("run_b", run_b).unwrap();
    let nested = a
        .lua
        .load(
            "local t, k, run_b = t, k2, run_b
               for i = 1, 2000 do local x = t[k] end
               run_b()
               for i = 1, 2000 do local x = t[k] end",
        )
        .into_function()
        .unwrap();
    b.set_budget();
    a.set_budget();
    nested.call::<()>(()).unwrap();
    let cb = b.charged();
    let ca = a.charged();
    assert!(
        (LOOKUPS_COST..LOOKUPS_COST + 1000).contains(&cb),
        "B charged {cb}, expected about {LOOKUPS_COST}"
    );
    assert!(
        (2 * LOOKUPS_COST..2 * LOOKUPS_COST + 1000).contains(&ca),
        "A charged {ca}, expected about {}",
        2 * LOOKUPS_COST
    );
}

#[test]
fn c_api_lookups_after_a_nested_state_call_are_charged_to_the_caller() {
    // A host callback of A runs B's lookups, then does A's own lookups
    // through the C API (lua_rawget) before returning to A's VM. Without a
    // claim on every API entry, B stayed the owner and was charged for A.
    let a = State::new();
    let b = std::rc::Rc::new(State::new());
    let b2 = std::rc::Rc::clone(&b);
    let f = a
        .lua
        .create_function(move |lua, ()| {
            b2.lookups.call::<()>(())?;
            let t: mlua::Table = lua.globals().get("t")?;
            let k: mlua::LuaString = lua.globals().get("k2")?;
            for _ in 0..2000 {
                let _: mlua::Value = t.raw_get(k.clone())?;
            }
            Ok(())
        })
        .unwrap();
    b.set_budget();
    a.set_budget();
    f.call::<()>(()).unwrap();
    let cb = b.charged();
    let ca = a.charged();
    assert!(
        (LOOKUPS_COST..LOOKUPS_COST + 1000).contains(&cb),
        "B charged {cb}, expected about {LOOKUPS_COST}"
    );
    assert!(
        (LOOKUPS_COST..LOOKUPS_COST + 1000).contains(&ca),
        "A charged {ca}, expected about {LOOKUPS_COST}"
    );
}
