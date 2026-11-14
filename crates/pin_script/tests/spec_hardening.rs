//! Spec: sandbox hardening from the Orchestrator decision "4.2 review: patched
//! vendored Lua" — 64 KiB string cap, `<close>` rejected, `__pairs` on the
//! read-only tables, address-free `tostring`, `math.random` arity.
//! (Scenarios that could hang live in `spec_stall.rs`.)

mod spec_support;

use pin_core::{CoilId, TableScriptBackend};
use pin_script::lua::reload::syntax_check;
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use spec_support::{Host, backend, lamp, names};

const HARDENING: &[u8] = include_bytes!("fixtures/spec_hardening.lua");

const TABLES: [&str; 11] = [
    "sw",
    "coil",
    "lamp",
    "magnet",
    "diverter",
    "flipper",
    "dispenser",
    "profile",
    "sound",
    "scene",
    "persist_key",
];

fn run_case(case: u16) -> pin_core::HostCommandQueue {
    let mut b = backend(HARDENING);
    let mut h = Host::new(1);
    h.case(&mut b, case);
    assert!(
        !b.is_faulted(),
        "case {case}: probes are pcall-protected and must not fault"
    );
    h.cmd
}

fn assert_probe(cmd: &pin_core::HostCommandQueue, id: u16, what: &str) {
    assert_eq!(lamp(cmd, id), Some(1.0), "probe {id} failed: {what}");
}

/// Runs `body` as an unprotected `on_tick`; it must fault before reaching the marker.
fn assert_faults(body: &str) {
    let src = format!("function on_tick() {body} pinball.show_scene(1) end");
    let mut b = backend(src.as_bytes());
    let mut h = Host::new(1);
    h.tick(&mut b);
    assert!(b.is_faulted(), "`{body}` must fault the script");
    assert!(
        h.cmd.display_scenes.is_empty(),
        "`{body}` did not abort the callback"
    );
}

// ---------------------------------------------------------------------------
// 64 KiB string cap.

#[test]
fn strings_up_to_64_kib_work_and_longer_ones_cannot_be_created() {
    let cmd = run_case(1);
    for (id, what) in [
        (0, "string.rep('x', 65536) works"),
        (1, "string.rep('x', 65537) raises"),
        (2, "string.rep('x', 1e9) raises"),
        (3, "32768 .. 32768 bytes works"),
        (4, "40000 .. 40000 bytes raises"),
        (5, "65536 bytes .. '!' raises"),
        (6, "string.format producing 80000 bytes raises"),
        (7, "string.format producing 60000 bytes works"),
        (8, "table.concat producing 90000 bytes raises"),
        (9, "table.concat producing 60000 bytes works"),
        (10, "string.rep with separator past the cap raises"),
        (11, "table.concat with separator past the cap raises"),
        (12, "s:rep(2) of a 40000 byte string raises"),
        (13, "string.gsub growing past the cap raises"),
        (14, "small string.format padding still works"),
        (15, "upper/reverse of a 40000 byte string work"),
        (16, "repeated doubling is stopped at the cap"),
    ] {
        assert_probe(&cmd, id, what);
    }
}

#[test]
fn unprotected_oversized_strings_fault_the_script() {
    for body in [
        "local s = string.rep('x', 65537)",
        "local s = string.rep('x', 1e9)",
        "local a = string.rep('x', 40000) local s = a .. a",
        "local a = string.rep('x', 40000) local s = table.concat({a, a})",
        "local a = string.rep('x', 40000) local s = ('%s%s'):format(a, a)",
        "local s = 'x' for i = 1, 20 do s = s .. s end",
    ] {
        assert_faults(body);
    }
}

#[test]
fn a_string_of_exactly_64_kib_is_usable_in_a_callback() {
    let src = b"function on_tick()
                  local s = string.rep('x', 65536)
                  pinball.persist_set(0, #s)
                end";
    let mut b = backend(src);
    let mut h = Host::new(1);
    h.tick(&mut b);
    assert!(!b.is_faulted());
    assert_eq!(spec_support::persisted(&h.cmd, 0), Some(65536));
}

// ---------------------------------------------------------------------------
// `<close>` variables are rejected everywhere source is accepted.

const CLOSE_SOURCES: [&[u8]; 4] = [
    b"local x <close> = nil",
    b"function on_tick()
        local guard <close> = setmetatable({}, { __close = function() while true do end end })
      end",
    b"do local a <const>, b <close> = 1, nil end",
    b"local x<close> = setmetatable({}, { __close = function() end })",
];

#[test]
fn close_variables_are_rejected_by_new() {
    for src in CLOSE_SOURCES {
        let r = LuaScriptBackend::new(src, &names(), LuaConfig::default());
        assert!(
            r.is_err(),
            "{:?} must not load",
            String::from_utf8_lossy(src)
        );
    }
}

#[test]
fn close_variables_are_rejected_by_syntax_check() {
    for src in CLOSE_SOURCES {
        assert!(
            syntax_check(src).is_err(),
            "{:?} must fail the syntax check",
            String::from_utf8_lossy(src)
        );
    }
}

#[test]
fn close_variables_are_rejected_by_apply_reload_and_the_old_script_survives() {
    let mut b = backend(b"function on_tick() pinball.pulse_coil(3, 30) end");
    let mut h = Host::new(1);
    for src in CLOSE_SOURCES {
        assert!(
            b.apply_reload(src).is_err(),
            "{:?}",
            String::from_utf8_lossy(src)
        );
        assert!(!b.is_faulted());
        h.tick(&mut b);
        assert_eq!(h.cmd.coil_pulses.as_slice(), &[(CoilId(3), 30)]);
    }
}

#[test]
fn const_variables_and_the_name_close_are_still_accepted() {
    for src in [
        &b"local x <const> = 1 function on_tick() pinball.persist_set(0, x) end"[..],
        b"local close = 5 function on_tick() pinball.persist_set(0, close) end",
    ] {
        assert_eq!(syntax_check(src), Ok(()));
        let mut b = backend(src);
        let mut h = Host::new(1);
        h.tick(&mut b);
        assert!(!b.is_faulted());
        assert_eq!(h.cmd.persist_writes.len(), 1);
    }
}

// ---------------------------------------------------------------------------
// __pairs on the read-only tables.

#[test]
fn pairs_over_a_name_table_yields_every_name_and_id() {
    let cmd = run_case(2);
    for (i, t) in TABLES.iter().enumerate() {
        assert_probe(
            &cmd,
            i as u16,
            &format!("pairs(pinball.{t}) must yield every name -> id"),
        );
    }
}

#[test]
fn pairs_hands_out_nothing_writable() {
    let cmd = run_case(3);
    for (i, t) in TABLES.iter().enumerate() {
        assert_probe(
            &cmd,
            i as u16,
            &format!("pinball.{t} was changed through an object returned by pairs()"),
        );
    }
}

#[test]
fn pairs_over_pinball_cannot_replace_api_functions() {
    let cmd = run_case(4);
    assert_probe(
        &cmd,
        0,
        "pinball.pulse_coil was replaced by a script function",
    );
    assert_probe(&cmd, 1, "pinball.sw was replaced");
    assert_probe(&cmd, 2, "pinball.tick was replaced");
    assert_eq!(
        cmd.coil_pulses.as_slice(),
        &[(CoilId(3), 30)],
        "the real pulse_coil must run"
    );
}

#[test]
fn tampering_in_one_callback_does_not_affect_later_callbacks() {
    let mut b = backend(HARDENING);
    let mut h = Host::new(1);
    h.case(&mut b, 3);
    h.case(&mut b, 4);
    h.case(&mut b, 2);
    assert!(!b.is_faulted());
    for (i, t) in TABLES.iter().enumerate() {
        assert_probe(
            &h.cmd,
            i as u16,
            &format!("pinball.{t} damaged by earlier tampering"),
        );
    }
}

// ---------------------------------------------------------------------------
// No addresses.

#[test]
fn tostring_and_format_do_not_leak_addresses() {
    let cmd = run_case(5);
    for (id, what) in [
        (0, "tostring({}) contains an address"),
        (1, "tostring(print) contains an address"),
        (2, "tostring(function() end) contains an address"),
        (3, "tostring of pinball objects contains an address"),
        (4, "string.format('%p', {}) must raise"),
        (5, "string.format('%p', print) must raise"),
        (6, "string.format('%s', {}) contains an address"),
        (7, "error messages contain an address"),
        (8, "tostring of library objects contains an address"),
    ] {
        assert_probe(&cmd, id, what);
    }
}

#[test]
fn unprotected_percent_p_faults() {
    assert_faults("local s = string.format('%p', {})");
}

// ---------------------------------------------------------------------------
// math.random arity.

#[test]
fn math_random_with_too_many_arguments_raises() {
    for call in [
        "math.random(1, 2, nil, 4)",
        "math.random(1, 2, 3)",
        "math.random(1, 2, nil)",
    ] {
        assert_faults(&format!("local r = {call}"));
    }
}

#[test]
fn math_random_with_valid_arity_still_works() {
    let src = b"function on_tick()
                  local a, b, c = math.random(), math.random(6), math.random(2, 3)
                  pinball.persist_set(0, b)
                  pinball.persist_set(1, c)
                end";
    let mut b = backend(src);
    let mut h = Host::new(1);
    h.tick(&mut b);
    assert!(!b.is_faulted());
    assert_eq!(h.cmd.persist_writes.len(), 2);
}

// ---------------------------------------------------------------------------
// DECISIONS "4.2 re-review round 2": binary packing is removed.

#[test]
fn string_pack_unpack_and_packsize_are_absent() {
    let src = b"function on_tick()
                  local s = ''
                  pinball.set_lamp(0, string.pack == nil and 1 or 0)
                  pinball.set_lamp(1, string.unpack == nil and 1 or 0)
                  pinball.set_lamp(2, string.packsize == nil and 1 or 0)
                  pinball.set_lamp(3, (s.pack == nil and s.unpack == nil and s.packsize == nil) and 1 or 0)
                  local mt = getmetatable('')
                  local idx = type(mt) == 'table' and mt.__index or nil
                  pinball.set_lamp(4, (type(idx) ~= 'table'
                    or (idx.pack == nil and idx.unpack == nil and idx.packsize == nil)) and 1 or 0)
                end";
    let mut b = backend(src);
    let mut h = Host::new(1);
    h.tick(&mut b);
    assert!(!b.is_faulted());
    assert_probe(&h.cmd, 0, "string.pack must be nil");
    assert_probe(&h.cmd, 1, "string.unpack must be nil");
    assert_probe(&h.cmd, 2, "string.packsize must be nil");
    assert_probe(&h.cmd, 3, "pack functions reachable as string methods");
    assert_probe(
        &h.cmd,
        4,
        "pack functions reachable through the string metatable",
    );
}

#[test]
fn unprotected_use_of_string_pack_faults() {
    assert_faults("local s = string.pack('i4', 1)");
    assert_faults("local n = string.packsize('i4')");
    assert_faults("local v = string.unpack('i4', 'abcd')");
    assert_faults("local s = ('i4'):pack(1)");
}
