use mlua_sandbox::{
    BUDGET_EXCEEDED, HOOK_INTERVAL, install_instruction_budget, lua_with_memory_limit,
};

const MIB: usize = 1024 * 1024;

#[test]
fn reports_lua_version() {
    let lua = lua_with_memory_limit(4 * MIB).unwrap();
    let v: String = lua.load("return _VERSION").eval().unwrap();
    println!("[mlua] _VERSION = {v}");
    assert_eq!(v, "Lua 5.4");
}

#[test]
fn memory_limit_raises_memory_error() {
    let lua = lua_with_memory_limit(4 * MIB).unwrap();
    // Grow a table of distinct strings until the cap is hit.
    let err = lua
        .load(
            r#"
            local t = {}
            for i = 1, 10000000 do t[i] = string.rep("x", 64) .. i end
            "#,
        )
        .exec()
        .expect_err("allocation beyond the cap must fail");
    println!("[mlua] memory-limit error: {err}");
    assert!(
        matches!(err, mlua::Error::MemoryError(_)),
        "expected MemoryError, got {err:?}"
    );
    // The state stays usable after the error and the cap is still enforced.
    lua.gc_collect().unwrap();
    assert!(lua.used_memory() <= 4 * MIB);
    let ok: i64 = lua.load("return 1 + 2").eval().unwrap();
    assert_eq!(ok, 3);
}

#[test]
fn instruction_hook_aborts_infinite_loop() {
    let lua = lua_with_memory_limit(4 * MIB).unwrap();
    let budget = 1_000_000;
    let ticks = install_instruction_budget(&lua, budget).unwrap();
    let err = lua
        .load("while true do end")
        .exec()
        .expect_err("runaway loop must be aborted by the hook");
    println!("[mlua] hook error after {} ticks: {err}", ticks.get());
    assert!(err.to_string().contains(BUDGET_EXCEEDED), "{err:?}");
    assert_eq!(ticks.get(), budget / u64::from(HOOK_INTERVAL) + 1);
}

#[test]
fn instruction_hook_cannot_be_swallowed_by_pcall() {
    // The budget error is catchable; a loop that restarts the work after
    // each catch must still return control to the host.
    for script in [
        "while true do pcall(function() while true do end end) end",
        "while true do xpcall(function() while true do end end, function(e) return e end) end",
    ] {
        let lua = lua_with_memory_limit(4 * MIB).unwrap();
        let budget = 100_000;
        let ticks = install_instruction_budget(&lua, budget).unwrap();
        let err = lua
            .load(script)
            .exec()
            .expect_err("a pcall loop must not outlive the budget");
        assert!(
            err.to_string().contains(BUDGET_EXCEEDED),
            "{script}: {err:?}"
        );
        assert_eq!(
            ticks.get(),
            budget / u64::from(HOOK_INTERVAL) + 1,
            "{script}"
        );
    }
}

/// Runs `script` under a 100 000-instruction budget on its own thread and
/// returns the error, failing (instead of hanging the test run) if `exec()`
/// has not returned within 10 s.
fn exec_with_watchdog(script: &'static str) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let lua = lua_with_memory_limit(4 * MIB).unwrap();
        install_instruction_budget(&lua, 100_000).unwrap();
        let r = lua.load(script).exec().map_err(|e| e.to_string());
        let _ = tx.send(r);
    });
    match rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(r) => r.expect_err("the script must not finish"),
        Err(_) => panic!("exec() did not return within 10 s: {script}"),
    }
}

#[test]
fn xpcall_message_handler_cannot_escape_the_budget() {
    // Stock xpcall runs the handler inside the hook that raised the budget
    // error (hooks disabled), so a looping handler never returned.
    for script in [
        "xpcall(function() while true do end end, function() while true do end end)",
        "xpcall(function() while true do end end, function(e)
           if tostring(e):find('budget', 1, true) then while true do end end
           return e
         end)",
        "while true do xpcall(function() while true do end end, function() while true do end end) end",
    ] {
        let err = exec_with_watchdog(script);
        assert!(err.contains(BUDGET_EXCEEDED), "{script}: {err}");
    }
}

#[test]
fn bounded_xpcall_keeps_xpcall_results() {
    let lua = lua_with_memory_limit(4 * MIB).unwrap();
    install_instruction_budget(&lua, 1_000_000).unwrap();
    let (ok, a, b): (bool, i64, i64) = lua
        .load("return xpcall(function(x, y) return x + y, x * y end, error, 3, 4)")
        .eval()
        .unwrap();
    assert_eq!((ok, a, b), (true, 7, 12));
    let (ok, msg): (bool, String) = lua
        .load("return xpcall(function() error('boom', 0) end, function(e) return 'handled ' .. e end)")
        .eval()
        .unwrap();
    assert_eq!((ok, msg.as_str()), (false, "handled boom"));
    let err = lua
        .load("xpcall(function() end, 42)")
        .exec()
        .expect_err("a non-function handler is an argument error");
    assert!(
        err.to_string().contains("bad argument #2 to 'xpcall'"),
        "{err}"
    );
}

#[test]
fn instruction_hook_is_deterministic_for_finite_work() {
    // Same script, same tick count: the budget is counted in VM instructions.
    let run = || {
        let lua = lua_with_memory_limit(4 * MIB).unwrap();
        let ticks = install_instruction_budget(&lua, u64::MAX / 2).unwrap();
        let s: i64 = lua
            .load("local s = 0 for i = 1, 200000 do s = s + i end return s")
            .eval()
            .unwrap();
        (s, ticks.get())
    };
    let a = run();
    let b = run();
    println!("[mlua] finite run: sum={} ticks={}", a.0, a.1);
    assert_eq!(a, b);
    assert_eq!(a.0, 200000 * 200001 / 2);
    assert!(a.1 > 0);
}
