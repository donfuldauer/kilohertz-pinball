//! Regression tests for CodeRabbit PR #3 findings on the patched vendored Lua
//! (third_party/lua-src/KHZ_PATCHES.md, patches 23-26): work done below one VM
//! instruction that used to spend no budget. Each pathological script must
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
