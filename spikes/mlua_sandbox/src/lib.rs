//! Spike 0.1/Q6a: `mlua` (lua54 + vendored) memory limit and instruction-count hook.
//!
//! The helpers here are the minimal sandbox recipe that Subphase 1.x scripting
//! is expected to reuse: a hard allocation cap and an instruction budget that
//! aborts a runaway script deterministically (counted in VM instructions, never
//! wall-clock time).

use std::cell::Cell;
use std::rc::Rc;

use mlua::debug::DebugEvent;
use mlua::{HookTriggers, Lua, VmState};

/// Instructions between hook invocations. Budget granularity is this value.
pub const HOOK_INTERVAL: u32 = 1_000;

/// Error message raised by the instruction-budget hook.
pub const BUDGET_EXCEEDED: &str = "instruction budget exceeded";

/// Replacement for the global `xpcall` installed by
/// [`install_instruction_budget`]: the message handler runs after the
/// protected call has unwound, as ordinary (hooked, budgeted) code.
pub const BOUNDED_XPCALL: &str = r#"
local pcall, type, error = pcall, type, error
local function xret(h, ok, ...)
  if ok then return true, ... end
  local _, r = pcall(h, (...))
  return false, r
end
xpcall = function(f, h, ...)
  if type(h) ~= "function" then
    error("bad argument #2 to 'xpcall' (function expected, got " .. type(h) .. ")", 2)
  end
  return xret(h, pcall(f, ...))
end
"#;

/// Create a Lua 5.4 state with a hard memory cap of `limit_bytes`.
pub fn lua_with_memory_limit(limit_bytes: usize) -> mlua::Result<Lua> {
    let lua = Lua::new();
    lua.set_memory_limit(limit_bytes)?;
    Ok(lua)
}

/// Install an instruction-count hook that aborts execution once more than
/// `budget` instructions (rounded up to [`HOOK_INTERVAL`]) have executed.
/// Returns a shared counter of hook ticks for inspection.
///
/// The budget error is an ordinary Lua error, so `pcall`/`xpcall` can catch
/// it. To keep a script such as
/// `while true do pcall(function() while true do end end) end` from
/// restarting the work forever, the hook also fires on every call and
/// return, and once the budget is spent it fails on those events too: the
/// call into `pcall` and the return from it both run outside the protected
/// region, so the error reaches the host. Only count events advance the tick
/// counter, so the budget arithmetic is unchanged.
///
/// Stock `xpcall` runs its message handler at the error point, before
/// unwinding; for an error raised by this hook that is still inside the hook
/// (`allowhook` is 0), so no count, call or return event fires in the
/// handler and `xpcall(f, function() while true do end end)` never returns.
/// The global `xpcall` is therefore replaced by [`BOUNDED_XPCALL`], which
/// runs the handler after unwinding, with hooks active and the budget still
/// enforced. Differences from stock `xpcall`: the handler sees the unwound
/// stack (a `debug.traceback` there would not show the error point), and an
/// error inside the handler is returned as the error value instead of
/// re-entering the handler.
///
/// Not covered: coroutines created from Lua (`coroutine.create`) do not run
/// this per-thread hook under mlua 0.12, so code inside them is not counted.
/// `__gc` finalizers also run with hooks disabled, so a looping finalizer is
/// not bounded either; a production sandbox must refuse `__gc` metatables
/// (as `pin_script` does).
pub fn install_instruction_budget(lua: &Lua, budget: u64) -> mlua::Result<Rc<Cell<u64>>> {
    // Before the hook: this chunk's instructions are not charged.
    lua.load(BOUNDED_XPCALL)
        .set_name("=bounded_xpcall")
        .exec()?;
    let ticks = Rc::new(Cell::new(0u64));
    let ticks_in_hook = Rc::clone(&ticks);
    let mut triggers = HookTriggers::new().every_nth_instruction(HOOK_INTERVAL);
    triggers.on_calls = true;
    triggers.on_returns = true;
    lua.set_hook(triggers, move |_lua, debug| {
        let mut n = ticks_in_hook.get();
        if matches!(debug.event(), DebugEvent::Count) {
            n += 1;
            ticks_in_hook.set(n);
        }
        if n * u64::from(HOOK_INTERVAL) > budget {
            Err(mlua::Error::runtime(BUDGET_EXCEEDED))
        } else {
            Ok(VmState::Continue)
        }
    })?;
    Ok(ticks)
}
