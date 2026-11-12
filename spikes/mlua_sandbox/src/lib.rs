//! Spike 0.1/Q6a: `mlua` (lua54 + vendored) memory limit and instruction-count hook.
//!
//! The helpers here are the minimal sandbox recipe that Subphase 1.x scripting
//! is expected to reuse: a hard allocation cap and an instruction budget that
//! aborts a runaway script deterministically (counted in VM instructions, never
//! wall-clock time).

use std::cell::Cell;
use std::rc::Rc;

use mlua::{HookTriggers, Lua, VmState};

/// Instructions between hook invocations. Budget granularity is this value.
pub const HOOK_INTERVAL: u32 = 1_000;

/// Error message raised by the instruction-budget hook.
pub const BUDGET_EXCEEDED: &str = "instruction budget exceeded";

/// Create a Lua 5.4 state with a hard memory cap of `limit_bytes`.
pub fn lua_with_memory_limit(limit_bytes: usize) -> mlua::Result<Lua> {
    let lua = Lua::new();
    lua.set_memory_limit(limit_bytes)?;
    Ok(lua)
}

/// Install an instruction-count hook that aborts execution once more than
/// `budget` instructions (rounded up to [`HOOK_INTERVAL`]) have executed.
/// Returns a shared counter of hook ticks for inspection.
pub fn install_instruction_budget(lua: &Lua, budget: u64) -> mlua::Result<Rc<Cell<u64>>> {
    let ticks = Rc::new(Cell::new(0u64));
    let ticks_in_hook = Rc::clone(&ticks);
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_INTERVAL),
        move |_lua, _debug| {
            let n = ticks_in_hook.get() + 1;
            ticks_in_hook.set(n);
            if n * u64::from(HOOK_INTERVAL) > budget {
                Err(mlua::Error::runtime(BUDGET_EXCEEDED))
            } else {
                Ok(VmState::Continue)
            }
        },
    )?;
    Ok(ticks)
}
