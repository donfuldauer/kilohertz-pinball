//! Spec: hot reload (`apply_reload`, the save/restore hooks), the I/O-side
//! `reload::syntax_check` / `ReloadWatcher`, and `load_backend` extension
//! handling.

mod spec_support;
mod temp_dir_support;

use pin_core::{CoilId, ScriptError, TableScriptBackend};
use pin_script::lua::reload::{ReloadWatcher, syntax_check};
use pin_script::{NameTables, load_backend};
use proptest::prelude::*;
use spec_support::{Host, backend, backend_with, cfg, names, persisted, score};
use temp_dir_support::TempDir;

const V1: &[u8] = include_bytes!("fixtures/spec_reload_v1.lua");
const V2: &[u8] = include_bytes!("fixtures/spec_reload_v2.lua");
const NO_HOOKS: &[u8] = b"local n = 0
function on_tick() n = n + 1 pinball.set_score(3, n) end";

/// Runs v1 for 5 ticks with two closed switches (7 then 20).
fn v1_with_state() -> (pin_script::lua::LuaScriptBackend, Host) {
    let mut b = backend(V1);
    let mut h = Host::new(3);
    for _ in 0..5 {
        h.tick(&mut b);
    }
    assert_eq!(score(&h.cmd, 0), Some(5));
    h.edge(&mut b, 7, true);
    h.edge(&mut b, 7, false);
    h.edge(&mut b, 20, true);
    assert_eq!(h.cmd.coil_pulses.as_slice(), &[(CoilId(31), 2)]);
    (b, h)
}

#[test]
fn reload_carries_state_through_the_hooks() {
    let (mut b, mut h) = v1_with_state();
    b.apply_reload(V2).expect("v2 compiles");
    assert!(!b.is_faulted());

    h.tick(&mut b);
    assert!(!b.is_faulted());
    assert_eq!(
        persisted(&h.cmd, 0),
        Some(1),
        "on_restore_state called exactly once"
    );
    assert_eq!(persisted(&h.cmd, 1), Some(1), "saved table arrived intact");
    assert_eq!(
        persisted(&h.cmd, 2),
        Some(2),
        "array part of the saved state"
    );
    assert_eq!(
        persisted(&h.cmd, 3),
        Some(1),
        "string field of the saved state"
    );
    assert_eq!(
        h.cmd.scores.as_slice(),
        &[(1u8, 6i64)],
        "the tick counter continued from 5"
    );
    assert_eq!(
        h.cmd.coil_pulses.as_slice(),
        &[(CoilId(7), 1), (CoilId(20), 2)],
        "switch history survived in order"
    );
}

#[test]
fn the_new_script_replaces_every_entry_point() {
    let (mut b, mut h) = v1_with_state();
    b.apply_reload(V2).unwrap();
    // v1's on_switch pulsed the knocker; v2's does not.
    h.edge(&mut b, 8, true);
    assert!(
        h.cmd.is_empty(),
        "old on_switch still installed: {:?}",
        h.cmd
    );
    h.tick(&mut b);
    assert!(score(&h.cmd, 0).is_none(), "old on_tick still installed");
    assert_eq!(persisted(&h.cmd, 2), Some(3));
}

#[test]
fn state_survives_a_chain_of_reloads() {
    let (mut b, mut h) = v1_with_state();
    b.apply_reload(V2).unwrap();
    h.tick(&mut b); // n = 6
    h.edge(&mut b, 63, true);
    b.apply_reload(V2).unwrap();
    h.tick(&mut b); // n = 7
    assert_eq!(h.cmd.scores.as_slice(), &[(1u8, 7i64)]);
    assert_eq!(
        persisted(&h.cmd, 0),
        Some(1),
        "each new instance is restored once"
    );
    assert_eq!(persisted(&h.cmd, 1), Some(1));
    assert_eq!(
        persisted(&h.cmd, 3),
        Some(2),
        "state came from v2 this time"
    );
    assert_eq!(
        h.cmd.coil_pulses.as_slice(),
        &[(CoilId(7), 1), (CoilId(20), 2), (CoilId(63), 3)]
    );
    for expected in 8..=20 {
        b.apply_reload(V2).unwrap();
        h.tick(&mut b);
        assert_eq!(h.cmd.scores.as_slice(), &[(1u8, expected)]);
    }
    assert!(!b.is_faulted());
}

#[test]
fn reloading_identical_source_is_seamless() {
    let mut b = backend(V2);
    let mut h = Host::new(1);
    for _ in 0..3 {
        h.tick(&mut b);
    }
    b.apply_reload(V2).unwrap();
    h.tick(&mut b);
    assert_eq!(h.cmd.scores.as_slice(), &[(1u8, 4i64)]);
}

#[test]
fn new_script_without_restore_hook_starts_fresh() {
    let (mut b, mut h) = v1_with_state();
    b.apply_reload(NO_HOOKS).expect("reload without hooks");
    assert!(!b.is_faulted());
    h.tick(&mut b);
    assert_eq!(h.cmd.scores.as_slice(), &[(2u8, 1i64)]);
    assert_eq!(h.cmd.len(), 1);
}

#[test]
fn restore_hook_is_not_called_when_the_old_script_saved_nothing() {
    let mut b = backend(NO_HOOKS);
    let mut h = Host::new(1);
    for _ in 0..4 {
        h.tick(&mut b);
    }
    b.apply_reload(V2).unwrap();
    h.tick(&mut b);
    assert!(!b.is_faulted());
    assert_eq!(
        persisted(&h.cmd, 0),
        Some(0),
        "on_restore_state needs both hooks"
    );
    assert_eq!(h.cmd.scores.as_slice(), &[(1u8, 1i64)]);
}

#[test]
fn failed_reload_returns_err_and_keeps_the_old_script_and_its_state() {
    let (mut b, mut h) = v1_with_state();
    for bad in [
        &b"function on_tick( end"[..],
        b"this is not lua",
        b"x = = 1",
        b"function on_tick() pinball.set_score(1, 1) ",
        b"\xff\xfe\x00\x01 garbage",
    ] {
        let r: Result<(), ScriptError> = b.apply_reload(bad);
        let err = r.expect_err("broken source must be rejected");
        assert!(
            matches!(err, ScriptError::Load(_)),
            "a compile failure is a load error, got {err:?}"
        );
        assert!(!b.is_faulted(), "a rejected reload is not a script fault");
    }
    h.tick(&mut b);
    assert_eq!(
        h.cmd.scores.as_slice(),
        &[(0u8, 6i64)],
        "old script and its counter keep running"
    );
    h.edge(&mut b, 8, true);
    assert_eq!(h.cmd.coil_pulses.as_slice(), &[(CoilId(31), 3)]);

    // And a later good reload still sees the full state.
    b.apply_reload(V2).unwrap();
    h.tick(&mut b);
    assert_eq!(h.cmd.scores.as_slice(), &[(1u8, 7i64)]);
    assert_eq!(persisted(&h.cmd, 2), Some(3));
}

#[test]
fn reload_recovers_a_faulted_script() {
    let src = b"function on_tick() while true do end end";
    let mut b = backend_with(src, cfg(16 * 1024 * 1024, 20_000));
    let mut h = Host::new(1);
    h.tick(&mut b);
    assert!(b.is_faulted());
    b.apply_reload(V2).expect("reload of a faulted backend");
    assert!(!b.is_faulted());
    h.tick(&mut b);
    assert!(!b.is_faulted());
    assert_eq!(score(&h.cmd, 1), Some(1));
}

#[test]
fn save_hook_that_never_returns_cannot_stall_the_reload() {
    let old = b"function on_save_state() while true do end end
                function on_tick() pinball.set_score(1, 1) end";
    let mut b = backend_with(old, cfg(16 * 1024 * 1024, 20_000));
    let mut h = Host::new(1);
    h.tick(&mut b);
    // The outcome (reload without state, or an error) is the implementer's
    // choice; hanging or leaving the backend half-swapped is not.
    match b.apply_reload(V2) {
        Ok(()) => {
            assert!(!b.is_faulted());
            h.tick(&mut b);
            assert_eq!(
                score(&h.cmd, 1),
                Some(1),
                "new script runs from fresh state"
            );
            assert_eq!(persisted(&h.cmd, 1), Some(0), "no bogus state was restored");
        }
        Err(_) => {
            h.tick(&mut b);
            if !b.is_faulted() {
                assert_eq!(score(&h.cmd, 0), Some(1), "old script still installed");
            }
        }
    }
}

#[test]
fn odd_save_and_restore_hooks_never_panic() {
    let olds: [&[u8]; 5] = [
        b"function on_save_state() return nil end",
        b"function on_save_state() return 42 end",
        b"function on_save_state() error('no state for you') end",
        b"function on_save_state() return { f = function() end, [{}] = 1 } end",
        b"function on_save_state() local t = {} t.self = t return t end",
    ];
    let news: [&[u8]; 3] = [
        V2,
        b"function on_restore_state(s) error('cannot restore') end
          function on_tick() pinball.set_score(1, 7) end",
        b"function on_restore_state(s) while true do end end
          function on_tick() pinball.set_score(1, 7) end",
    ];
    for old in olds {
        for new in news {
            let mut b = backend_with(old, cfg(16 * 1024 * 1024, 20_000));
            let mut h = Host::new(1);
            h.tick(&mut b);
            let _ = b.apply_reload(new);
            h.tick(&mut b);
            h.edge(&mut b, 1, true);
            // Still recoverable afterwards.
            b.apply_reload(NO_HOOKS)
                .expect("plain script always reloads");
            assert!(!b.is_faulted());
            h.tick(&mut b);
            assert_eq!(score(&h.cmd, 2), Some(1));
        }
    }
}

#[test]
fn name_tables_remain_available_after_reload() {
    let mut b = backend(b"");
    b.apply_reload(
        b"function on_tick() pinball.pulse_coil(pinball.coil.knocker, pinball.sw.spinner) end",
    )
    .unwrap();
    let mut h = Host::new(1);
    h.tick(&mut b);
    assert_eq!(h.cmd.coil_pulses.as_slice(), &[(CoilId(31), 63)]);
}

#[test]
fn reload_through_the_trait_object_works() {
    let mut b: Box<dyn TableScriptBackend> = Box::new(backend(V1));
    let mut h = Host::new(1);
    h.tick(b.as_mut());
    h.tick(b.as_mut());
    assert!(b.apply_reload(b"nope(").is_err());
    b.apply_reload(V2).unwrap();
    h.tick(b.as_mut());
    assert_eq!(h.cmd.scores.as_slice(), &[(1u8, 3i64)]);
}

// ---------------------------------------------------------------------------
// reload::syntax_check

#[test]
fn syntax_check_accepts_valid_sources() {
    for src in [
        V1,
        V2,
        NO_HOOKS,
        b"",
        b"-- only a comment",
        b"return 1",
        include_bytes!("fixtures/spec_api.lua"),
        include_bytes!("fixtures/spec_sandbox.lua"),
        include_bytes!("fixtures/spec_faults.lua"),
    ] {
        assert_eq!(syntax_check(src), Ok(()));
    }
}

#[test]
fn syntax_check_rejects_invalid_sources() {
    for src in [
        &b"function on_tick( end"[..],
        b"x = = 1",
        b"if true then",
        b"local 1 = 2",
        b"return return",
        b"\"unterminated",
        b"goto nowhere",
        b"\x00\x01\x02 binary junk",
    ] {
        let r = syntax_check(src);
        assert!(
            r.is_err(),
            "{:?} must be rejected",
            String::from_utf8_lossy(src)
        );
    }
}

#[test]
fn syntax_check_only_compiles_and_never_runs_the_chunk() {
    // Each of these would hang, fault or escape if executed.
    for src in [
        &b"while true do end"[..],
        b"error('must not run')",
        b"local t = {} while true do t[#t + 1] = string.rep('x', 1 << 20) end",
        b"os.exit(3)",
        b"io.open('/nonexistent/pin_script_spec', 'w')",
        b"undefined_function()",
    ] {
        assert_eq!(syntax_check(src), Ok(()));
    }
}

#[test]
fn syntax_check_compiles_under_the_engine_heap_cap() {
    // Many distinct string constants: each is interned in the compiling
    // state's heap, so together they need more than the engine's 2 MiB cap.
    let cap = pin_script::lua::LuaConfig::engine().memory_limit_bytes;
    // Each literal stays under the 64 KiB per-string cap.
    let per = 60 * 1024;
    let mut src = String::from("local t = {\n");
    for i in 0..(cap / per + 8) {
        src.push('"');
        src.push_str(&format!("{i:08}"));
        src.push_str(&"x".repeat(per));
        src.push_str("\",\n");
    }
    src.push_str("}\n");
    assert!(src.len() > cap);
    assert_eq!(syntax_check(src.as_bytes()), Err(ScriptError::MemoryLimit));
    // A small source with the same shape still compiles.
    assert_eq!(syntax_check(b"local t = { \"a\", \"b\" }"), Ok(()));
}

// ---------------------------------------------------------------------------
// reload::ReloadWatcher (I/O worker side). No sleeping: bounded yield loops.

const A: &[u8] = b"function on_tick() pinball.set_score(1, 1) end\n";
const B: &[u8] = b"function on_tick() pinball.set_score(1, 2) end -- edited\n";
const C: &[u8] = b"function on_tick() pinball.set_score(1, 3) end -- edited again\n";
const BROKEN: &[u8] = b"function on_tick( pinball.set_score(1, 4) end -- broken\n";

/// Polls until `want` is delivered, rewriting the file now and then so a lost
/// file-system event cannot wedge the test. Every delivered source must be
/// syntactically valid and must never be `BROKEN`.
fn poll_until(w: &mut ReloadWatcher, path: &std::path::Path, want: &[u8]) -> bool {
    const ROUNDS: u32 = 400;
    const POLLS_PER_ROUND: u32 = 50_000;
    for _ in 0..ROUNDS {
        std::fs::write(path, want).unwrap();
        for _ in 0..POLLS_PER_ROUND {
            if let Some(src) = w.poll() {
                assert_ne!(
                    src.as_slice(),
                    BROKEN,
                    "a broken file was handed to the sim"
                );
                assert!(syntax_check(&src).is_ok(), "poll returned unchecked source");
                if src == want {
                    return true;
                }
            }
            std::thread::yield_now();
        }
    }
    false
}

#[test]
fn watcher_delivers_changed_valid_source() {
    let dir = TempDir::new("watch_ok");
    let path = dir.path().join("rules.lua");
    std::fs::write(&path, A).unwrap();
    let mut w = ReloadWatcher::new(&path).expect("watch an existing file");
    assert!(poll_until(&mut w, &path, B), "the edit was never delivered");
    assert!(
        poll_until(&mut w, &path, C),
        "the second edit was never delivered"
    );
}

#[test]
fn watcher_never_delivers_a_file_that_fails_the_syntax_check() {
    let dir = TempDir::new("watch_broken");
    let path = dir.path().join("rules.lua");
    std::fs::write(&path, A).unwrap();
    let mut w = ReloadWatcher::new(&path).expect("watch an existing file");
    assert!(poll_until(&mut w, &path, B));

    std::fs::write(&path, BROKEN).unwrap();
    for _ in 0..200_000 {
        if let Some(src) = w.poll() {
            assert_ne!(
                src.as_slice(),
                BROKEN,
                "a broken file was handed to the sim"
            );
            assert!(syntax_check(&src).is_ok());
        }
        std::thread::yield_now();
    }
    // Fixing the file is picked up again.
    assert!(poll_until(&mut w, &path, C), "the fix was never delivered");
}

#[test]
fn watcher_output_feeds_apply_reload() {
    let dir = TempDir::new("watch_apply");
    let path = dir.path().join("rules.lua");
    std::fs::write(&path, A).unwrap();
    let mut w = ReloadWatcher::new(&path).unwrap();
    let mut b = backend(A);
    let mut h = Host::new(1);
    h.tick(&mut b);
    assert_eq!(score(&h.cmd, 0), Some(1));

    let mut delivered = None;
    'outer: for _ in 0..400 {
        std::fs::write(&path, B).unwrap();
        for _ in 0..50_000 {
            if let Some(src) = w.poll()
                && src == B
            {
                delivered = Some(src);
                break 'outer;
            }
            std::thread::yield_now();
        }
    }
    let src = delivered.expect("the edit was never delivered");
    b.apply_reload(&src).unwrap();
    h.tick(&mut b);
    assert_eq!(score(&h.cmd, 0), Some(2));
}

#[test]
fn poll_without_changes_is_cheap_and_returns_valid_source_or_nothing() {
    let dir = TempDir::new("watch_idle");
    let path = dir.path().join("rules.lua");
    std::fs::write(&path, A).unwrap();
    let mut w = ReloadWatcher::new(&path).unwrap();
    for _ in 0..10_000 {
        if let Some(src) = w.poll() {
            // Some implementations may report the initial contents once.
            assert_eq!(src.as_slice(), A);
        }
    }
}

// ---------------------------------------------------------------------------
// load_backend

fn load(entry: &str, source: &[u8]) -> Result<Box<dyn TableScriptBackend>, ScriptError> {
    load_backend(entry, source, &names())
}

fn expect_err(entry: &str, source: &[u8]) -> ScriptError {
    match load(entry, source) {
        Ok(_) => panic!("load_backend({entry:?}) must fail"),
        Err(e) => e,
    }
}

#[test]
fn lua_extension_selects_the_lua_backend() {
    for entry in [
        "rules.lua",
        "scripts/rules.lua",
        "./a/b/c.lua",
        "my.table.rules.lua",
    ] {
        let mut b = match load(entry, V2) {
            Ok(b) => b,
            Err(e) => panic!("{entry}: {e}"),
        };
        let mut h = Host::new(1);
        h.init(b.as_mut());
        h.tick(b.as_mut());
        assert_eq!(score(&h.cmd, 1), Some(1), "{entry}");
        b.apply_reload(V1).unwrap();
        h.tick(b.as_mut());
        assert_eq!(score(&h.cmd, 0), Some(1), "{entry}");
    }
}

#[test]
fn lua_backend_from_load_backend_uses_the_name_tables_and_default_limits() {
    let src = b"function on_tick() pinball.pulse_coil(pinball.coil.knocker, 5) end
                function on_switch(id, closed) while true do end end";
    let mut b = load("rules.lua", src).unwrap_or_else(|e| panic!("{e}"));
    let mut h = Host::new(1);
    h.tick(b.as_mut());
    assert_eq!(h.cmd.coil_pulses.as_slice(), &[(CoilId(31), 5)]);
    // Default budget: the loop is aborted and the script goes silent.
    h.edge(b.as_mut(), 1, true);
    h.tick(b.as_mut());
    assert!(h.cmd.is_empty());
}

#[test]
fn lua_source_errors_are_reported() {
    let e = expect_err("rules.lua", b"function (");
    assert!(matches!(e, ScriptError::Load(_)), "got {e:?}");
}

#[test]
fn wasm_extension_selects_the_wasm_backend() {
    for source in [&b"\0asm\x01\0\0\0"[..], b"", V1] {
        let e = expect_err("rules.wasm", source);
        assert!(matches!(e, ScriptError::Load(_)), "got {e:?}");
    }
    let minimal = br#"(module
  (memory (export "memory") 1)
  (func (export "on_init"))
  (func (export "on_tick"))
  (func (export "on_switch_edge") (param i32 i32)))"#;
    assert!(load("rules.wasm", minimal).is_ok());
}

#[test]
fn unknown_extensions_are_rejected() {
    for entry in [
        "rules.py",
        "rules.js",
        "rules.txt",
        "rules",
        "",
        "lua",
        "rules.lua.bak",
        "rules.luac",
        "dir.lua/rules",
    ] {
        let e = expect_err(entry, V1);
        assert!(
            !matches!(e, ScriptError::BudgetExceeded | ScriptError::MemoryLimit),
            "{entry:?}: {e:?}"
        );
    }
}

#[test]
fn load_backend_with_empty_name_tables_works() {
    let mut b = match load_backend(
        "rules.lua",
        b"function on_tick() pinball.set_lamp(1, 1) end",
        &NameTables::default(),
    ) {
        Ok(b) => b,
        Err(e) => panic!("{e}"),
    };
    let mut h = Host::new(1);
    h.tick(b.as_mut());
    assert_eq!(h.cmd.lamps.len(), 1);
}

// ---------------------------------------------------------------------------
// Parsers survive bounded random bytes.

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn syntax_check_survives_random_bytes(src in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = syntax_check(&src);
    }

    #[test]
    fn syntax_check_survives_random_lua_like_text(
        src in "[ a-z0-9=(){}\\[\\]\"'.,;:+*/%<>~#\\n-]{0,200}"
    ) {
        let _ = syntax_check(src.as_bytes());
    }

    #[test]
    fn syntax_check_agrees_with_backend_construction_on_token_soup(
        words in proptest::collection::vec(
            prop_oneof![
                Just("local"), Just("x"), Just("="), Just("1"), Just("function"), Just("end"),
                Just("("), Just(")"), Just("if"), Just("then"), Just("return"), Just("y"),
                Just("+"), Just("do"), Just("{"), Just("}"), Just(","), Just("f"),
            ],
            0..12,
        )
    ) {
        // Wrapped in a never-called function so construction cannot fail at
        // run time. An `end` with no open `function`/`if`/`do` before it
        // would close the wrapper and leave the rest as top-level code that
        // runs (and can fail) at construction, so such `end`s are dropped.
        let mut open = 0usize;
        let words: Vec<&str> = words
            .into_iter()
            .filter(|w| match *w {
                "function" | "if" | "do" => {
                    open += 1;
                    true
                }
                "end" if open == 0 => false,
                "end" => {
                    open -= 1;
                    true
                }
                _ => true,
            })
            .collect();
        let src = format!("local function spec_wrapper() {} end", words.join(" "));
        let checked = syntax_check(src.as_bytes()).is_ok();
        let built = pin_script::lua::LuaScriptBackend::new(
            src.as_bytes(), &names(), pin_script::lua::LuaConfig::default()).is_ok();
        prop_assert_eq!(checked, built, "syntax_check and new() disagree on {:?}", src);
    }

    #[test]
    fn backend_construction_and_reload_survive_random_bytes(
        src in proptest::collection::vec(any::<u8>(), 0..256)
    ) {
        let _ = pin_script::lua::LuaScriptBackend::new(
            &src, &names(), cfg(4 * 1024 * 1024, 20_000));
        let mut b = backend_with(V1, cfg(4 * 1024 * 1024, 20_000));
        let _ = b.apply_reload(&src);
        let mut h = Host::new(1);
        h.tick(&mut b);
    }

    #[test]
    fn load_backend_survives_arbitrary_entries(
        entry in "[a-zA-Z0-9_./\\\\ -]{0,24}",
        src in proptest::collection::vec(any::<u8>(), 0..64),
    ) {
        let lower = entry.to_lowercase();
        let r = load_backend(&entry, &src, &NameTables::default());
        // Entries that merely contain ".lua" (e.g. "a.lua/") are left to the implementation.
        if !lower.contains(".lua") {
            prop_assert!(r.is_err(), "{:?} is not a Lua entry", entry);
        }
    }
}
