//! Lua 5.4 backend, sandbox and hot reload (PLAN.md §3.4, §4 Subphase 4.2,
//! D10, D11, D25). Also owns [`crate::load_backend`].
//!
//! # Script interface
//!
//! The table script (`rules.lua`) defines any of these global functions:
//! `on_init()`, `on_tick()`, `on_switch(id, closed)`, `on_save_state() -> value`
//! and `on_restore_state(value)`. It acts through the read-only `pinball`
//! module (see [`api`](self) docs on `LuaScriptBackend`), which is only usable
//! inside `on_init`, `on_tick` and `on_switch` (the host context exists only
//! there); calling it at top level or from the reload hooks raises an error.
//!
//! # Faults ("tilt")
//!
//! A runtime error, an exhausted instruction budget or the memory cap aborts
//! the callback, and the commands it had queued are dropped (all-or-nothing).
//! The backend then queues `(coil, false)` for every coil the
//! script holds (continuing on later callbacks if the queue is full), stops
//! calling the script and stays faulted until a successful
//! [`TableScriptBackend::apply_reload`].
//!
//! # Hot reload
//!
//! [`reload::ReloadWatcher`] (I/O worker) watches `rules.lua` and hands
//! syntax-checked source to the sim thread, which calls `apply_reload` at a
//! tick boundary. A reload builds a fresh sandboxed state; the old
//! `on_save_state` result is copied over as plain data and passed to the new
//! `on_restore_state`. `on_init` is not called again.
//!
//! # Allocation
//!
//! In steady state `on_tick`/`on_switch_edge` perform no heap allocation for a
//! script that does not itself allocate (verified by
//! `tests/lua_alloc.rs`): the host side uses only fixed-size cells, and the
//! Lua VM's own allocations (its capped arena) only happen when the script
//! creates objects. Faults, reloads and error messages may allocate.
//!
//! # Determinism
//!
//! Randomness comes only from the host `Pcg32` (`pinball.random`,
//! `math.random`), so identical seeds and inputs give identical command
//! queues, as long as rules do not depend on `pairs` order over string keys or
//! on `tostring` of tables (Lua seeds its string hash per state).

pub(crate) mod api;
mod bridge;
pub mod reload;
mod sandbox;
mod saved;

use bridge::Bridge;
use mlua::{Function, IntoLuaMulti, Value};
use pin_core::ids::SwitchId;
use pin_core::{HostCommandQueue, HostCtx, ScriptError, SwitchEdge, TableScriptBackend};
use sandbox::Vm;
use std::fmt;
use std::rc::Rc;

use crate::NameTables;

/// Instructions between two budget-hook calls; the budget's granularity.
pub const HOOK_INTERVAL: u32 = 1000;

/// Kilobytes of allocation-equivalent work in the fixed GC step run after
/// every `on_tick` (`LUA_GCSTEP`). The automatic incremental collector stays
/// on as well, so allocation-heavy scripts are still paced by allocation.
pub const GC_STEP_KBYTES: i32 = 16;

/// Limits of one Lua backend (§3.4, D25).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LuaConfig {
    /// Hard cap on the Lua heap, bytes.
    pub memory_limit_bytes: usize,
    /// VM instructions allowed per callback (granularity [`HOOK_INTERVAL`]).
    pub instruction_budget: u32,
}

impl LuaConfig {
    /// The configuration the engine runs table rules with (DECISIONS.md
    /// "Orchestrator — 4.2 re-review round 3"): 100 000 instructions per
    /// callback and a 2 MiB heap. Under it no script can make a single tick
    /// take more than 8 ms (release build; measured in DECISIONS.md "4.2 — fix
    /// round 3"). [`LuaConfig::default`] keeps the brief's larger limits for
    /// tools and tests.
    pub const fn engine() -> Self {
        Self {
            memory_limit_bytes: 2 * 1024 * 1024,
            instruction_budget: 100_000,
        }
    }
}

impl Default for LuaConfig {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 16 * 1024 * 1024,
            instruction_budget: 1_000_000,
        }
    }
}

/// The Lua 5.4 [`TableScriptBackend`]. See the module docs.
///
/// The `pinball` module (all ids are integers):
/// `pulse_coil(coil, ms)`, `hold_coil(coil, on)`, `set_magnet(magnet, power)`,
/// `set_zipper(flipper, closed)`, `set_diverter(diverter, on)`,
/// `spawn_ball(dispenser, profile)`, `set_lamp(lamp, intensity)`,
/// `set_gi_tint(r, g, b, a)`, `set_score(player 1..4, score)`,
/// `show_scene(scene)`, `play_sound(sound, gain)`, `persist_set(key, value)`,
/// `persist_get(key)`, `switch(id)`, `tick()`, `balls_in_play()`,
/// `random(n)` / `random(m, n)`, plus the name tables `pinball.sw`,
/// `pinball.coil`, … (see [`NameTables`]).
pub struct LuaScriptBackend {
    // Field order: the state drops before the bridge it references.
    vm: Vm,
    bridge: Rc<Bridge>,
    names: NameTables,
    cfg: LuaConfig,
    faulted: bool,
    fault_count: u32,
    last_error: Option<ScriptError>,
}

// SAFETY: `LuaScriptBackend` is `!Send` only because of the `Rc`s inside it
// (the `Rc<Bridge>` and mlua's non-`send` handles, whose refcounts are plain
// cells). Every clone of those `Rc`s lives inside this struct or inside the Lua
// state it owns: no method returns or accepts an mlua handle or the bridge, and
// the raw `HostCtx` pointers are cleared before each callback returns. Moving
// the backend therefore moves the whole reference graph at once, and it is only
// ever used through `&mut self` from one thread at a time. mlua (lua54, not
// luau) keeps no thread-local state.
unsafe impl Send for LuaScriptBackend {}

impl fmt::Debug for LuaScriptBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LuaScriptBackend")
            .field("cfg", &self.cfg)
            .field("faulted", &self.faulted)
            .field("fault_count", &self.fault_count)
            .field("overflow_count", &self.overflow_count())
            .field("last_error", &self.last_error)
            .finish_non_exhaustive()
    }
}

/// Lengths of every command list at the start of a callback, so a faulting
/// callback's commands can be dropped (allocation-free).
struct QueueMark {
    lens: [usize; 11],
    gi_tint: Option<[f32; 4]>,
    overflow: u32,
}

impl QueueMark {
    fn new(q: &HostCommandQueue, bridge: &Bridge) -> Self {
        Self {
            lens: [
                q.coil_pulses.len(),
                q.coil_holds.len(),
                q.magnets.len(),
                q.zippers.len(),
                q.diverters.len(),
                q.ball_spawns.len(),
                q.lamps.len(),
                q.scores.len(),
                q.display_scenes.len(),
                q.sounds.len(),
                q.persist_writes.len(),
            ],
            gi_tint: q.gi_tint,
            overflow: bridge.overflow_count(),
        }
    }

    /// Drops what the callback queued. Every coil named by a dropped hold
    /// command is marked held, so the tilt release that follows emits
    /// `(coil, false)` for it: a dropped release must never strand a coil
    /// (an extra release of a coil that was off is harmless).
    fn rollback(&self, q: &mut HostCommandQueue, bridge: &Bridge) {
        let l = &self.lens;
        for &(coil, _) in &q.coil_holds[l[1].min(q.coil_holds.len())..] {
            bridge.set_held(coil, true);
        }
        bridge.set_overflow_count(self.overflow);
        q.coil_pulses.truncate(l[0]);
        q.coil_holds.truncate(l[1]);
        q.magnets.truncate(l[2]);
        q.zippers.truncate(l[3]);
        q.diverters.truncate(l[4]);
        q.ball_spawns.truncate(l[5]);
        q.lamps.truncate(l[6]);
        q.scores.truncate(l[7]);
        q.display_scenes.truncate(l[8]);
        q.sounds.truncate(l[9]);
        q.persist_writes.truncate(l[10]);
        q.gi_tint = self.gi_tint;
    }
}

/// Which script entry point to call.
#[derive(Copy, Clone)]
enum Entry {
    Init,
    Tick,
    Switch,
}

impl LuaScriptBackend {
    /// Compiles and runs the top level of `source` in a fresh sandbox.
    ///
    /// Errors: `Load` for syntax errors, binary chunks and top-level runtime
    /// errors; `BudgetExceeded` / `MemoryLimit` when the top level exceeds a
    /// limit.
    pub fn new(source: &[u8], names: &NameTables, cfg: LuaConfig) -> Result<Self, ScriptError> {
        let interval = cfg.instruction_budget.clamp(1, HOOK_INTERVAL);
        let bridge = Rc::new(Bridge::new(interval));
        let vm = Vm::build(source, names, &cfg, &bridge)?;
        Ok(Self {
            vm,
            bridge,
            names: names.clone(),
            cfg,
            faulted: false,
            fault_count: 0,
            last_error: None,
        })
    }

    /// True while the script is in the tilt state (after a fault, until a
    /// successful reload).
    pub fn is_faulted(&self) -> bool {
        self.faulted
    }

    /// Number of faults since construction.
    pub fn fault_count(&self) -> u32 {
        self.fault_count
    }

    /// Commands dropped because their queue was full, since construction.
    pub fn overflow_count(&self) -> u32 {
        self.bridge.overflow_count()
    }

    /// The error behind the most recent fault, if any.
    pub fn last_error(&self) -> Option<&ScriptError> {
        self.last_error.as_ref()
    }

    /// The configuration this backend runs with.
    pub fn config(&self) -> LuaConfig {
        self.cfg
    }

    /// Bytes currently used by the Lua heap.
    pub fn used_memory(&self) -> usize {
        self.vm.lua.used_memory()
    }

    fn fault(&mut self, err: ScriptError, ctx: &mut HostCtx) {
        self.faulted = true;
        self.fault_count = self.fault_count.saturating_add(1);
        self.last_error = Some(err);
        self.bridge.release_holds(ctx.cmd);
    }

    /// Calls `f(args)` with `ctx` visible to the API, under a fresh budget.
    fn call_with_ctx(
        vm: &Vm,
        bridge: &Bridge,
        budget: u32,
        f: &Function,
        args: impl IntoLuaMulti,
        ctx: &mut HostCtx,
    ) -> Result<(), ScriptError> {
        vm.reset_budget(bridge, budget);
        let r = {
            let _guard = bridge.enter(ctx);
            f.call::<()>(args)
        };
        vm.check(bridge, r).map_or(Ok(()), Err)
    }

    fn run(&mut self, entry: Entry, switch: SwitchId, closed: bool, ctx: &mut HostCtx) {
        if self.faulted {
            if self.bridge.any_held() {
                self.bridge.release_holds(ctx.cmd);
            }
            return;
        }
        let keys = &self.vm.keys;
        let key = match entry {
            Entry::Init => &keys.on_init,
            Entry::Tick => &keys.on_tick,
            Entry::Switch => &keys.on_switch,
        };
        let budget = self.cfg.instruction_budget;
        let mark = QueueMark::new(ctx.cmd, &self.bridge);
        let (vm, bridge) = (&self.vm, &*self.bridge);
        let result = match vm.entry(key) {
            Err(e) => Err(ScriptError::Runtime(e.to_string())),
            Ok(None) => Ok(()),
            Ok(Some(f)) => match entry {
                Entry::Switch => {
                    Self::call_with_ctx(vm, bridge, budget, &f, (i64::from(switch.0), closed), ctx)
                }
                Entry::Init | Entry::Tick => Self::call_with_ctx(vm, bridge, budget, &f, (), ctx),
            },
        };
        if let Err(e) = result {
            // An aborted callback is all-or-nothing: drop what it queued.
            mark.rollback(ctx.cmd, &self.bridge);
            self.fault(e, ctx);
            return;
        }
        if let Entry::Tick = entry {
            // Fixed GC step under a fresh budget (scripts cannot register
            // finalizers, so no script code runs here).
            self.vm.reset_budget(&self.bridge, budget);
            let r = sandbox::gc_step(&self.vm.lua, GC_STEP_KBYTES);
            if let Some(err) = self.vm.check(&self.bridge, r) {
                self.fault(err, ctx);
            }
        }
    }

    /// Calls the old `on_save_state`, if present and not faulted. Any failure
    /// means "no state to carry" (a broken save hook must not block the fix).
    fn save_state(&self) -> Option<saved::Saved> {
        if self.faulted {
            return None;
        }
        let f = self.vm.entry(&self.vm.keys.on_save_state).ok()??;
        self.vm
            .reset_budget(&self.bridge, self.cfg.instruction_budget);
        let r = f.call::<Value>(());
        let exhausted = self.vm.exhausted(&self.bridge);
        let v = r.ok().filter(|_| !exhausted)?;
        saved::save(&v, self.cfg.memory_limit_bytes).ok().flatten()
    }
}

impl TableScriptBackend for LuaScriptBackend {
    fn on_init(&mut self, ctx: &mut HostCtx) {
        self.run(Entry::Init, SwitchId(0), false, ctx);
    }

    fn on_tick(&mut self, ctx: &mut HostCtx) {
        self.run(Entry::Tick, SwitchId(0), false, ctx);
    }

    fn on_switch_edge(&mut self, switch: SwitchId, edge: SwitchEdge, ctx: &mut HostCtx) {
        self.run(Entry::Switch, switch, edge == SwitchEdge::Closed, ctx);
    }

    /// Builds the new script in a fresh sandbox; on any error (syntax, top
    /// level, or the new `on_restore_state` failing) the old script keeps
    /// running and the error is returned. On success the faulted state is
    /// cleared (held coils stay tracked: the new script inherits them).
    fn apply_reload(&mut self, source: &[u8]) -> Result<(), ScriptError> {
        let new_vm = Vm::build(source, &self.names, &self.cfg, &self.bridge)?;
        let state = self.save_state();
        let restore = new_vm
            .entry(&new_vm.keys.on_restore_state)
            .map_err(|e| ScriptError::Runtime(e.to_string()))?;
        if let (Some(state), Some(restore)) = (state, restore) {
            new_vm.reset_budget(&self.bridge, self.cfg.instruction_budget);
            let r = saved::restore(&new_vm.lua, &state).and_then(|arg| restore.call::<()>(arg));
            if let Some(e) = new_vm.check(&self.bridge, r) {
                return Err(e);
            }
        }
        drop(std::mem::replace(&mut self.vm, new_vm));
        self.faulted = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::ids::CoilId;
    use pin_core::{HostCommandQueue, HostView, Pcg32, SeedableRng};

    fn backend(src: &str) -> LuaScriptBackend {
        LuaScriptBackend::new(src.as_bytes(), &NameTables::default(), LuaConfig::default()).unwrap()
    }

    fn tick(b: &mut LuaScriptBackend) -> HostCommandQueue {
        let view = HostView::default();
        let mut rng = Pcg32::seed_from_u64(1);
        let mut cmd = HostCommandQueue::default();
        b.on_tick(&mut HostCtx {
            view: &view,
            rng: &mut rng,
            cmd: &mut cmd,
        });
        cmd
    }

    #[test]
    fn default_config() {
        let c = LuaConfig::default();
        assert_eq!(c.memory_limit_bytes, 16 << 20);
        assert_eq!(c.instruction_budget, 1_000_000);
        let b = backend("");
        assert_eq!(b.config(), c);
        assert!(b.used_memory() > 0);
        assert!(format!("{b:?}").contains("LuaScriptBackend"));
    }

    #[test]
    fn fault_releases_holds_and_stops_calls() {
        let mut b = backend(
            "n = 0 function on_tick() n = n + 1 pinball.hold_coil(3, true) \
             if n == 2 then error('boom') end end",
        );
        assert_eq!(tick(&mut b).coil_holds.as_slice(), &[(CoilId(3), true)]);
        let q = tick(&mut b);
        assert!(b.is_faulted());
        assert_eq!(
            q.coil_holds.as_slice(),
            &[(CoilId(3), false)],
            "the faulting tick's own commands are dropped; the earlier hold is released"
        );
        assert!(matches!(b.last_error(), Some(ScriptError::Runtime(m)) if m.contains("boom")));
        assert!(tick(&mut b).is_empty());
        assert_eq!(b.fault_count(), 1);
        b.apply_reload(b"function on_tick() pinball.pulse_coil(1, 2) end")
            .unwrap();
        assert!(!b.is_faulted());
        assert_eq!(tick(&mut b).coil_pulses.len(), 1);
    }

    #[test]
    fn finalizers_are_refused() {
        let mut b = backend(
            "function on_tick() setmetatable({}, {__gc = function() while true do end end}) end",
        );
        tick(&mut b);
        assert!(b.is_faulted());
        assert!(matches!(b.last_error(), Some(ScriptError::Runtime(m)) if m.contains("__gc")));
        // Adding __gc after the fact is ignored by Lua; the GC step stays bounded.
        let mut b = backend(
            "function on_tick() local mt = {} setmetatable({}, mt) \
             mt.__gc = function() while true do end end end",
        );
        for _ in 0..50 {
            tick(&mut b);
        }
        assert!(!b.is_faulted());
    }
}
