//! Building one sandboxed Lua 5.4 state (§3.4, D25).
//!
//! * Libraries: `base`, `string`, `table`, `math`, `utf8` only (no `io`, `os`,
//!   `debug`, `package`, `coroutine`).
//! * Removed from `base`: `dofile`, `loadfile`, `load`, `require`, `warn`;
//!   `print` is a no-op; `collectgarbage` only answers `"count"`.
//!   `string.dump` is removed and source is loaded in text mode only, so no
//!   bytecode can ever be loaded. `math.randomseed` is a no-op and
//!   `math.random` draws from the host PCG.
//! * The string metatable and every `pinball` table are protected
//!   (`__metatable`), `pinball` tables are read-only proxies (also against
//!   `rawset`).
//! * `setmetatable` refuses metatables with `__gc` (Lua runs finalizers with
//!   hooks disabled) or `__mode` (ephemeron convergence is quadratic inside the
//!   GC's atomic step); neither could be bounded by the budget.
//! * Lua itself is patched (third_party/lua-src/KHZ_PATCHES.md): fixed hash
//!   seed and sort pivot, 64 KiB string cap, pattern step limit, no `<close>`,
//!   no addresses in `tostring`/`%p`, and library loops charged against the same
//!   budget as the instruction hook (`lua_khzspend`/`lua_khzcharge`).
//! * `pcall`/`xpcall` re-raise memory errors, and once the instruction budget
//!   is exhausted the hook fires on every instruction, so neither limit can be
//!   swallowed by a protected call.

use super::LuaConfig;
use super::api;
use super::bridge::Bridge;
use crate::NameTables;
use mlua::chunk::ChunkMode;
use mlua::{Function, HookTriggers, Lua, LuaOptions, StdLib, Table, Value, VmState};
use pin_core::ScriptError;
use std::rc::Rc;

/// Error message raised when the instruction budget runs out.
pub(crate) const BUDGET_MSG: &str = "instruction budget exceeded";

/// Chunk name used in error messages.
const CHUNK_NAME: &str = "=rules.lua";

/// Trusted prelude, run once per state before the table script. Arguments:
/// the private API table, the private name tables, the memory-abort function.
const PRELUDE: &str = r##"
local api, names, memabort, hostrandom = ...
local G = _G
local error, rawset, rawget, setmetatable, next, getmetatable, type, select =
  error, rawset, rawget, setmetatable, next, getmetatable, type, select
local _pcall = pcall
local protected = setmetatable({}, { __mode = "k" })
local function readonly(t, what)
  local proxy = {}
  -- Iterate the hidden table without handing it out (next(t) would).
  local function iter(_, k) return next(t, k) end
  setmetatable(proxy, {
    __index = t,
    __newindex = function() error(what .. " is read-only", 2) end,
    __pairs = function() return iter, proxy, nil end,
    __len = function() return #t end,
    __metatable = "locked",
  })
  protected[proxy] = true
  return proxy
end
for k, v in next, names do api[k] = readonly(v, "pinball." .. k) end
G.pinball = readonly(api, "pinball")
G.rawset = function(t, k, v)
  if protected[t] then error("cannot rawset a read-only pinball table", 2) end
  return rawset(t, k, v)
end
-- Finalizers run with hooks disabled (no instruction budget), so scripts may
-- not register any. A __gc added to a metatable later is ignored by Lua 5.4.
G.setmetatable = function(t, mt)
  if type(mt) == "table" then
    if rawget(mt, "__gc") ~= nil then
      error("__gc metamethods are not allowed in table scripts", 2)
    end
    -- Weak tables: ephemeron convergence is quadratic in the GC's atomic
    -- step, outside the budget.
    if rawget(mt, "__mode") ~= nil then
      error("weak tables (__mode) are not allowed in table scripts", 2)
    end
  end
  return setmetatable(t, mt)
end
local MEM = "not enough memory"
local function chk(ok, ...)
  if not ok and (...) == MEM then memabort() end
  return ok, ...
end
G.pcall = function(...) return chk(_pcall(...)) end
-- A real xpcall runs the message handler at the error point, where an error
-- raised by the budget hook leaves hooks disabled: a looping handler would
-- escape the budget. Run the handler after unwinding instead, as plain code.
local function xret(h, ok, ...)
  if ok then return true, ... end
  local e = ...
  if e == MEM then memabort() end
  return false, select(2, chk(_pcall(h, e)))
end
G.xpcall = function(f, h, ...)
  if type(h) ~= "function" then error("bad argument #2 to 'xpcall' (function expected)", 2) end
  return xret(h, _pcall(f, ...))
end
G.math.random = hostrandom
getmetatable("").__metatable = "locked"
"##;

/// Entry-point names, interned once per state.
pub(crate) struct Keys {
    pub(crate) on_init: mlua::LuaString,
    pub(crate) on_tick: mlua::LuaString,
    pub(crate) on_switch: mlua::LuaString,
    pub(crate) on_save_state: mlua::LuaString,
    pub(crate) on_restore_state: mlua::LuaString,
}

/// One sandboxed Lua state with the table script loaded.
pub(crate) struct Vm {
    // Field order matters for drop: handles before the state.
    pub(crate) globals: Table,
    pub(crate) keys: Keys,
    /// The main `lua_State` (valid while `lua` lives), for the KHZ budget.
    raw: *mut mlua::ffi::lua_State,
    pub(crate) lua: Lua,
}

fn load_err(e: &mlua::Error) -> ScriptError {
    ScriptError::Load(e.to_string())
}

/// True if `e` (or its cause chain) is a Lua memory error.
pub(crate) fn is_memory_error(e: &mlua::Error) -> bool {
    match e {
        mlua::Error::MemoryError(_) => true,
        mlua::Error::CallbackError { cause, .. } => is_memory_error(cause),
        mlua::Error::WithContext { cause, .. } => is_memory_error(cause),
        _ => false,
    }
}

// KHZ cost budget of the patched Lua (third_party/lua-src/KHZ_PATCHES.md).
unsafe extern "C-unwind" {
    fn lua_khzsetbudget(state: *mut mlua::ffi::lua_State, n: i64);
    fn lua_khzspend(state: *mut mlua::ffi::lua_State, n: i64) -> i64;
}

/// Maps an mlua error raised while the budget/memory flags were live.
fn classify(bridge: &Bridge, exhausted: bool, e: &mlua::Error) -> ScriptError {
    if bridge.budget_hit() || exhausted {
        ScriptError::BudgetExceeded
    } else if bridge.memory_hit() || is_memory_error(e) {
        ScriptError::MemoryLimit
    } else {
        ScriptError::Runtime(e.to_string())
    }
}

/// Changes the instruction-hook interval of the main thread, keeping mlua's
/// hook function. Allocation-free on the Rust side.
fn set_hook_count(lua: &Lua, count: u32) -> mlua::Result<()> {
    let count = i32::try_from(count).unwrap_or(i32::MAX);
    // SAFETY: only reads and re-sets the hook of the state mlua hands us; the
    // hook function pointer and mask are kept, so mlua's bookkeeping holds.
    unsafe {
        lua.exec_raw::<()>((), |state| {
            let hook = mlua::ffi::lua_gethook(state);
            let mask = mlua::ffi::lua_gethookmask(state);
            mlua::ffi::lua_sethook(state, hook, mask, count);
        })
    }
}

/// Performs one fixed-budget incremental GC step (`LUA_GCSTEP`, `kbytes`).
pub(crate) fn gc_step(lua: &Lua, kbytes: i32) -> mlua::Result<()> {
    // SAFETY: `lua_gc(LUA_GCSTEP, n)` is a plain API call on the running state.
    unsafe {
        lua.exec_raw::<()>((), |state| {
            mlua::ffi::lua_gc(state, mlua::ffi::LUA_GCSTEP, kbytes);
        })
    }
}

/// Compile-only check of `source` in text mode (no state is kept).
///
/// Compilation runs under the engine's heap cap
/// ([`LuaConfig::engine`]`.memory_limit_bytes`), so the parser's allocations
/// are bounded no matter how large the file is; a source whose compiled form
/// does not fit is a [`ScriptError::MemoryLimit`].
pub(crate) fn syntax_check(source: &[u8]) -> Result<(), ScriptError> {
    let lua = Lua::new_with(StdLib::NONE, LuaOptions::new()).map_err(|e| load_err(&e))?;
    lua.set_memory_limit(LuaConfig::engine().memory_limit_bytes)
        .map_err(|e| load_err(&e))?;
    lua.load(source)
        .set_name(CHUNK_NAME)
        .set_mode(ChunkMode::Text)
        .into_function()
        .map(drop)
        .map_err(|e| {
            if is_memory_error(&e) {
                ScriptError::MemoryLimit
            } else {
                load_err(&e)
            }
        })
}

impl Vm {
    /// Builds a fresh sandboxed state, installs the API and runs the top-level
    /// chunk of `source` under the budget. `bridge` is shared across reloads.
    pub(crate) fn build(
        source: &[u8],
        names: &NameTables,
        cfg: &LuaConfig,
        bridge: &Rc<Bridge>,
    ) -> Result<Vm, ScriptError> {
        let libs = StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8;
        let lua = Lua::new_with(libs, LuaOptions::new()).map_err(|e| load_err(&e))?;
        let vm = Self::install(lua, names, bridge).map_err(|e| load_err(&e))?;

        // The cap applies from compilation on: the compiled chunk's constants
        // live in the heap too, so a source whose compiled form does not fit
        // is a `MemoryLimit`, never a reason to raise the cap.
        vm.lua
            .set_memory_limit(cfg.memory_limit_bytes)
            .map_err(|e| load_err(&e))?;
        let chunk = vm
            .lua
            .load(source)
            .set_name(CHUNK_NAME)
            .set_mode(ChunkMode::Text)
            .into_function()
            .map_err(|e| {
                if is_memory_error(&e) {
                    ScriptError::MemoryLimit
                } else {
                    load_err(&e)
                }
            })?;

        let hook_bridge = Rc::clone(bridge);
        let raw = vm.raw as usize;
        let interval = i64::from(bridge.hook_interval);
        vm.lua
            .set_hook(
                HookTriggers::new().every_nth_instruction(bridge.hook_interval),
                move |lua, _| {
                    // SAFETY: `raw` is this state's main thread; the hook only
                    // runs while the state is alive.
                    let left = unsafe { lua_khzspend(raw as *mut mlua::ffi::lua_State, interval) };
                    if left < 0 || hook_bridge.budget_hit() {
                        hook_bridge.mark_budget_hit();
                        // From now on fail on every instruction so no `pcall`
                        // can keep the script running.
                        let _ = set_hook_count(lua, 1);
                        Err(mlua::Error::runtime(BUDGET_MSG))
                    } else {
                        Ok(VmState::Continue)
                    }
                },
            )
            .map_err(|e| load_err(&e))?;

        vm.reset_budget(bridge, cfg.instruction_budget);
        let r = chunk.call::<()>(());
        if let Some(e) = vm.check(bridge, r) {
            return Err(match e {
                ScriptError::Runtime(m) => ScriptError::Load(m),
                other => other,
            });
        }
        Ok(vm)
    }

    /// Starts a fresh budget of `instructions` for the next call into Lua.
    ///
    /// Also restores the hook interval: a budget hit lowers it to every
    /// instruction (see the hook in [`Vm::build`]), and a state that is kept
    /// after that (e.g. the old state after a failed reload whose save hook ran
    /// out of budget) must not keep paying `hook_interval` per instruction.
    pub(crate) fn reset_budget(&self, bridge: &Bridge, instructions: u32) {
        // Cannot fail: no arguments are pushed and the closure does not raise.
        let _ = set_hook_count(&self.lua, bridge.hook_interval);
        bridge.reset_flags();
        // SAFETY: `raw` is the live main state of `self.lua`.
        unsafe { lua_khzsetbudget(self.raw, i64::from(instructions)) };
    }

    /// True once the budget of the current call is used up.
    pub(crate) fn exhausted(&self, bridge: &Bridge) -> bool {
        // SAFETY: as above; spending 0 only reads the counter.
        bridge.budget_hit() || unsafe { lua_khzspend(self.raw, 0) } < 0
    }

    /// The fault behind `r`, if any: an error, or a budget or memory abort
    /// recorded even though the call returned (an abort swallowed at the very
    /// end, or a `memabort` error caught by an outer `pcall`).
    pub(crate) fn check<T>(&self, bridge: &Bridge, r: mlua::Result<T>) -> Option<ScriptError> {
        let exhausted = self.exhausted(bridge);
        match r {
            Err(e) => Some(classify(bridge, exhausted, &e)),
            Ok(_) if exhausted => Some(ScriptError::BudgetExceeded),
            Ok(_) if bridge.memory_hit() => Some(ScriptError::MemoryLimit),
            Ok(_) => None,
        }
    }

    fn install(lua: Lua, names: &NameTables, bridge: &Rc<Bridge>) -> mlua::Result<Vm> {
        // SAFETY: only reads the state pointer mlua runs on (the main thread
        // outside callbacks); it stays valid for the life of `lua`.
        let raw = unsafe {
            let mut p: *mut mlua::ffi::lua_State = std::ptr::null_mut();
            lua.exec_raw::<()>((), |state| p = state)?;
            p
        };
        let g = lua.globals();
        for k in [
            "dofile",
            "loadfile",
            "load",
            "loadstring",
            "require",
            "warn",
            "module",
        ] {
            g.raw_set(k, Value::Nil)?;
        }
        for k in ["io", "os", "debug", "package", "coroutine", "jit", "ffi"] {
            g.raw_set(k, Value::Nil)?;
        }
        g.raw_set(
            "print",
            lua.create_function(|_, _: mlua::MultiValue| Ok(()))?,
        )?;
        g.raw_set(
            "collectgarbage",
            lua.create_function(
                |lua, (opt, _): (Option<mlua::LuaString>, Value)| match opt {
                    Some(s) if s.as_bytes().as_ref() == b"count" => {
                        Ok(lua.used_memory() as f64 / 1024.0)
                    }
                    _ => Err(mlua::Error::runtime(
                        "collectgarbage: only collectgarbage(\"count\") is available",
                    )),
                },
            )?,
        )?;
        let string: Table = g.raw_get("string")?;
        // No bytecode; no binary packing (its per-option loops are not
        // charged, and rules have no use for it).
        for k in ["dump", "pack", "unpack", "packsize"] {
            string.raw_set(k, Value::Nil)?;
        }
        let math: Table = g.raw_get("math")?;
        math.raw_set(
            "randomseed",
            lua.create_function(|_, _: mlua::MultiValue| Ok(()))?,
        )?;

        let api_table = api::build(&lua, bridge)?;
        let name_tables = lua.create_table()?;
        for (field, entries) in names.lua_tables() {
            let t = lua.create_table_with_capacity(0, entries.len())?;
            for (name, id) in entries {
                t.raw_set(name, i64::from(id))?;
            }
            name_tables.raw_set(field, t)?;
        }
        let mem_bridge = Rc::clone(bridge);
        let memabort = lua.create_function(move |_, ()| -> mlua::Result<()> {
            mem_bridge.mark_memory_hit();
            Err(mlua::Error::MemoryError("not enough memory".into()))
        })?;
        lua.load(PRELUDE)
            .set_name("=prelude")
            .set_mode(ChunkMode::Text)
            .call::<()>((
                api_table,
                name_tables,
                memabort,
                api::math_random_fn(&lua, bridge)?,
            ))?;

        let key = |s: &str| lua.create_string(s);
        let keys = Keys {
            on_init: key("on_init")?,
            on_tick: key("on_tick")?,
            on_switch: key("on_switch")?,
            on_save_state: key("on_save_state")?,
            on_restore_state: key("on_restore_state")?,
        };
        Ok(Vm {
            globals: g,
            keys,
            raw,
            lua,
        })
    }

    /// The global function named by `key`, if the script defines one
    /// (`Ok(None)` for nil or a non-function value).
    pub(crate) fn entry(&self, key: &mlua::LuaString) -> mlua::Result<Option<Function>> {
        match self.globals.raw_get::<Value>(key)? {
            Value::Function(f) => Ok(Some(f)),
            _ => Ok(None),
        }
    }
}
