//! Spec: sandbox escape attempts fail (io/os/debug/package/load/dofile/
//! loadfile/require, bytecode loading, string metatable, metatables on the
//! pinball name tables) and name tables are read-only.

mod spec_support;

use pin_core::{CoilId, ScriptError, TableScriptBackend};
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use spec_support::{Host, backend, lamp, names, persisted};

const SANDBOX: &[u8] = include_bytes!("fixtures/spec_sandbox.lua");

fn probes() -> pin_core::HostCommandQueue {
    let mut b = backend(SANDBOX);
    let mut h = Host::new(1);
    h.case(&mut b, 1);
    assert!(
        !b.is_faulted(),
        "probes are pcall-protected and must not fault"
    );
    h.cmd
}

fn assert_probe(cmd: &pin_core::HostCommandQueue, id: u16, what: &str) {
    assert_eq!(
        lamp(cmd, id),
        Some(1.0),
        "sandbox probe {id} failed: {what}"
    );
}

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

#[test]
fn forbidden_libraries_and_loaders_are_absent() {
    let cmd = probes();
    for (id, name) in [
        (0, "io"),
        (1, "os"),
        (2, "debug"),
        (3, "package"),
        (4, "require"),
        (5, "load"),
        (6, "loadfile"),
        (7, "dofile"),
        (8, "loadstring"),
        (9, "coroutine"),
    ] {
        assert_probe(&cmd, id, &format!("global `{name}` must be nil"));
    }
}

#[test]
fn string_dump_cannot_be_loaded_back() {
    assert_probe(&probes(), 10, "string.dump output must not be loadable");
}

#[test]
fn allowed_libraries_are_present() {
    assert_probe(&probes(), 11, "base/string/table/math/utf8 must be usable");
}

#[test]
fn collectgarbage_is_absent_or_count_only() {
    assert_probe(
        &probes(),
        12,
        "collectgarbage(\"count\") must work if present",
    );
}

#[test]
fn writing_a_new_name_into_a_name_table_raises() {
    let cmd = probes();
    for (i, t) in TABLES.iter().enumerate() {
        assert_probe(
            &cmd,
            20 + i as u16,
            &format!("pinball.{t}.new = 1 must raise"),
        );
    }
}

#[test]
fn overwriting_an_existing_name_raises_and_keeps_the_id() {
    let cmd = probes();
    for (i, t) in TABLES.iter().enumerate() {
        assert_probe(
            &cmd,
            40 + i as u16,
            &format!("pinball.{t}.<name> = x must raise"),
        );
    }
}

#[test]
fn setmetatable_on_name_tables_fails() {
    let cmd = probes();
    for (i, t) in TABLES.iter().enumerate() {
        assert_probe(
            &cmd,
            60 + i as u16,
            &format!("setmetatable(pinball.{t}, ..) must fail"),
        );
    }
}

#[test]
fn getmetatable_tampering_does_not_unlock_name_tables() {
    let cmd = probes();
    for (i, t) in TABLES.iter().enumerate() {
        assert_probe(
            &cmd,
            80 + i as u16,
            &format!("pinball.{t} unlocked via getmetatable"),
        );
    }
}

#[test]
fn rawset_cannot_change_name_resolution() {
    let cmd = probes();
    for (i, t) in TABLES.iter().enumerate() {
        assert_probe(
            &cmd,
            100 + i as u16,
            &format!("pinball.{t} changed via rawset"),
        );
    }
}

#[test]
fn string_metatable_offers_no_escape() {
    let cmd = probes();
    assert_probe(&cmd, 120, "string metatable exposes a forbidden function");
    assert_probe(&cmd, 121, "setmetatable on a string must fail");
}

#[test]
fn nothing_reachable_from_globals_belongs_to_a_forbidden_library() {
    let mut b = backend(SANDBOX);
    let mut h = Host::new(1);
    h.case(&mut b, 2);
    assert!(
        !b.is_faulted(),
        "the reachability walk must fit in the default budget"
    );
    assert_eq!(
        persisted(&h.cmd, 0),
        Some(0),
        "forbidden keys reachable from _G"
    );
    assert!(
        persisted(&h.cmd, 1).unwrap() >= 1,
        "the walk visited nothing"
    );
}

#[test]
fn global_namespace_only_holds_sandbox_safe_names() {
    let mut b = backend(SANDBOX);
    let mut h = Host::new(1);
    h.case(&mut b, 3);
    assert!(!b.is_faulted());
    assert_eq!(
        persisted(&h.cmd, 0),
        Some(0),
        "unexpected globals in the sandbox"
    );
}

/// An unprotected use of a forbidden library is a runtime error: the script
/// faults, nothing panics.
#[test]
fn unprotected_use_of_forbidden_library_faults_the_script() {
    for body in [
        "io.open('/etc/passwd')",
        "os.execute('true')",
        "os.exit(1)",
        "debug.getregistry()",
        "require('os')",
        "package.loadlib('x', 'y')",
        "load('return 1')()",
        "dofile('/etc/passwd')",
        "loadfile('/etc/passwd')",
    ] {
        let src = format!("function on_tick() {body} pinball.pulse_coil(1, 10) end");
        let mut b = backend(src.as_bytes());
        let mut h = Host::new(1);
        h.tick(&mut b);
        assert!(b.is_faulted(), "`{body}` must fault the script");
        assert!(
            h.cmd.coil_pulses.is_empty(),
            "`{body}` must abort the callback"
        );
    }
}

#[test]
fn a_script_cannot_touch_the_file_system_or_run_processes() {
    let dir = std::env::temp_dir().join(format!("pin_script_spec_sandbox_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("escaped.txt");
    let _ = std::fs::remove_file(&target);
    let path = target.to_string_lossy().replace('\\', "/");
    let src = format!(
        r#"
        local path = "{path}"
        local function attempts()
          pcall(function() local f = io.open(path, "w"); f:write("x"); f:close() end)
          pcall(function() os.execute("touch " .. path) end)
          pcall(function() local f = io.popen("touch " .. path); f:close() end)
          pcall(function() os.rename(path .. ".nope", path) end)
          pcall(function() require("io").open(path, "w"):close() end)
          pcall(function() package.loaded.io.open(path, "w"):close() end)
          pcall(function() debug.getregistry()._LOADED.io.open(path, "w"):close() end)
          pcall(function() load("io.open(...,'w'):close()")(path) end)
          pcall(function() getmetatable("").__index.open(path, "w"):close() end)
        end
        attempts()
        function on_init() attempts() end
        function on_tick() attempts() pinball.pulse_coil(1, 10) end
        "#
    );
    let mut b = backend(src.as_bytes());
    let mut h = Host::new(1);
    h.init(&mut b);
    h.tick(&mut b);
    assert!(!b.is_faulted());
    assert_eq!(h.cmd.coil_pulses.as_slice(), &[(CoilId(1), 10)]);
    assert!(
        !target.exists(),
        "the script created a file outside the sandbox"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Precompiled bytecode must never be accepted: it bypasses the verifier-less
// Lua 5.4 loader and is the classic string.dump escape.

fn bytecode() -> Vec<u8> {
    let lua = mlua::Lua::new();
    let f = lua
        .load("function on_tick() pinball.pulse_coil(1, 10) end")
        .into_function()
        .expect("reference chunk compiles");
    let code = f.dump(false);
    assert_eq!(code.first(), Some(&0x1b), "expected a binary chunk");
    code
}

#[test]
fn binary_chunks_are_rejected_at_construction() {
    let code = bytecode();
    let r = LuaScriptBackend::new(&code, &names(), LuaConfig::default());
    assert!(r.is_err(), "a precompiled chunk must not load");
}

#[test]
fn binary_chunks_are_rejected_on_reload_and_the_old_script_survives() {
    let code = bytecode();
    let mut b = backend(b"function on_tick() pinball.pulse_coil(3, 30) end");
    let r: Result<(), ScriptError> = b.apply_reload(&code);
    assert!(r.is_err(), "a precompiled chunk must not hot-reload");
    let mut h = Host::new(1);
    h.tick(&mut b);
    assert_eq!(h.cmd.coil_pulses.as_slice(), &[(CoilId(3), 30)]);
}

#[test]
fn binary_chunks_fail_the_syntax_check() {
    assert!(pin_script::lua::reload::syntax_check(&bytecode()).is_err());
    assert!(pin_script::lua::reload::syntax_check(b"\x1bLua").is_err());
}

#[test]
fn garbage_with_a_bytecode_signature_never_panics() {
    for tail in [
        &b""[..],
        b"\x54\x00",
        b"\x54\x00\x19\x93\r\n\x1a\n\x04\x08\x08xV\0\0\0\0\0\0",
    ] {
        let mut src = b"\x1bLua".to_vec();
        src.extend_from_slice(tail);
        assert!(LuaScriptBackend::new(&src, &names(), LuaConfig::default()).is_err());
    }
}

#[test]
fn sandbox_also_applies_to_reloaded_scripts() {
    let mut b = backend(b"");
    b.apply_reload(SANDBOX).expect("fixture reloads");
    let mut h = Host::new(1);
    h.case(&mut b, 1);
    assert!(!b.is_faulted());
    for id in 0..=10 {
        assert_probe(&h.cmd, id, "forbidden global visible after reload");
    }
    for (i, t) in TABLES.iter().enumerate() {
        assert_probe(
            &h.cmd,
            20 + i as u16,
            &format!("pinball.{t} writable after reload"),
        );
    }
}
