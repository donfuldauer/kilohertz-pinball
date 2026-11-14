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
