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
//! * `setmetatable` refuses metatables with `__gc`: Lua runs finalizers with
//!   hooks disabled, so a finalizer could otherwise loop outside the budget.
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
  setmetatable(proxy, {
    __index = t,
    __newindex = function() error(what .. " is read-only", 2) end,
    __pairs = function() return next, t, nil end,
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
  if type(mt) == "table" and rawget(mt, "__gc") ~= nil then
    error("__gc metamethods are not allowed in table scripts", 2)
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
G.math.random = function(...)
  local n = select("#", ...)
  if n > 2 then error("wrong number of arguments", 2) end
  return hostrandom(n, ...)
end
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

/// Maps an mlua error raised while the budget/memory flags were live.
pub(crate) fn classify(bridge: &Bridge, e: &mlua::Error) -> ScriptError {
    if bridge.budget_hit() {
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
pub(crate) fn syntax_check(source: &[u8]) -> Result<(), ScriptError> {
    let lua = Lua::new_with(StdLib::NONE, LuaOptions::new()).map_err(|e| load_err(&e))?;
    lua.load(source)
        .set_name(CHUNK_NAME)
        .set_mode(ChunkMode::Text)
        .into_function()
        .map(drop)
        .map_err(|e| load_err(&e))
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

        // Compile before applying the cap so syntax errors are always `Load`.
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
        let used = vm.lua.used_memory();
        vm.lua
            .set_memory_limit(cfg.memory_limit_bytes.max(used))
            .map_err(|e| load_err(&e))?;

        let hook_bridge = Rc::clone(bridge);
        vm.lua
            .set_hook(
                HookTriggers::new().every_nth_instruction(bridge.hook_interval),
                move |lua, _| {
                    if hook_bridge.charge() {
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

        bridge.reset_budget(cfg.instruction_budget);
        chunk.call::<()>(()).map_err(|e| match classify(bridge, &e) {
            ScriptError::Runtime(m) => ScriptError::Load(m),
            other => other,
        })?;
        Ok(vm)
    }

    fn install(lua: Lua, names: &NameTables, bridge: &Rc<Bridge>) -> mlua::Result<Vm> {
        let g = lua.globals();
        for k in ["dofile", "loadfile", "load", "loadstring", "require", "warn", "module"] {
            g.raw_set(k, Value::Nil)?;
        }
        for k in ["io", "os", "debug", "package", "coroutine", "jit", "ffi"] {
            g.raw_set(k, Value::Nil)?;
        }
        g.raw_set("print", lua.create_function(|_, _: mlua::MultiValue| Ok(()))?)?;
        g.raw_set(
            "collectgarbage",
            lua.create_function(|lua, (opt, _): (Option<String>, Value)| {
                match opt.as_deref() {
                    Some("count") => Ok(lua.used_memory() as f64 / 1024.0),
                    _ => Err(mlua::Error::runtime(
                        "collectgarbage: only collectgarbage(\"count\") is available",
                    )),
                }
            })?,
        )?;
        let string: Table = g.raw_get("string")?;
        string.raw_set("dump", Value::Nil)?;
        let math: Table = g.raw_get("math")?;
        math.raw_set("randomseed", lua.create_function(|_, _: mlua::MultiValue| Ok(()))?)?;

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
            lua,
        })
    }

    /// The global function named by `key`, if the script defines one.
    pub(crate) fn entry(&self, key: &mlua::LuaString) -> Option<Function> {
        match self.globals.raw_get::<Value>(key) {
            Ok(Value::Function(f)) => Some(f),
            _ => None,
        }
    }
}
