//! Native wasmtime running the *instrumented* module (design "native-instr"):
//! the same rewriting the web designs use, executed by wasmtime 50.0.1 with
//! pin_script's engine configuration (copied), so the instrumentation's
//! fuel and depth counter decide the kill point natively too. wasmtime's own
//! fuel stays on as a backstop. Generated from `wasmi_backend.rs` (spike).
//!
//! Two fuel modes:
//! * [`Mode::WasmiFuel`]: the module as it is; wasmi's own fuel metering
//!   (its units, its check points) and `StoreLimits`.
//! * [`Mode::Instrumented`]: the module rewritten by
//!   [`crate::instrument`] (wasmtime-exact fuel in a global, optional depth
//!   counter, capped memory maximum); wasmi's own fuel stays on only as a
//!   backstop (4 × the budget + 4 Mi).

use crate::abi::{self, ID_OF, IMPORTS, MAX_TABLE_ELEMENTS, MAX_TABLES, ModuleInfo, V};
use crate::hostcore::{HostState, HostTrap, IdTables, QueueMark, Raw};
use crate::instrument::{self, WHY_DEPTH, WHY_FUEL};
use pin_core::ids::SwitchId;
use pin_core::{HostCtx, ScriptError, SwitchEdge, TableScriptBackend};
use pin_script::NameTables;
use pin_script::wasm::WasmConfig;
use std::sync::Arc;
use wasmtime::{
    Caller, Config, Engine, Error, Extern, Global, Linker, Memory, Module, Store, StoreLimits,
    StoreLimitsBuilder, Trap, TypedFunc, Val,
};

pub use crate::wasmi_backend::Mode;

/// Store data.
pub struct Data {
    host: HostState,
    limits: StoreLimits,
    memory: Option<Memory>,
    fuel_global: Option<Global>,
    fuel_name: Option<String>,
    /// The host trap of the current call, if a host function trapped.
    host_trap: Option<HostTrap>,
}

/// pin_script's engine `Config` (crates/pin_script/src/wasm/mod.rs `engine()`).
fn config(_mode: Mode, _eager: bool) -> Config {
    let mut c = Config::new();
    c.consume_fuel(true)
        .cranelift_nan_canonicalization(true)
        .wasm_threads(false)
        .shared_memory(false)
        .wasm_relaxed_simd(false)
        .relaxed_simd_deterministic(true)
        .wasm_multi_memory(false)
        .wasm_memory64(false)
        .wasm_gc(false)
        .wasm_function_references(false)
        .wasm_exceptions(false)
        .wasm_stack_switching(false)
        .wasm_component_model(false)
        .wasm_custom_page_sizes(false)
        .wasm_wide_arithmetic(false)
        .parallel_compilation(false)
        .memory_init_cow(false)
        .max_wasm_stack(256 * 1024)
        .wasm_simd(false)
        .wasm_tail_call(false)
        .wasm_extended_const(false);
    c
}

fn trap(d: &mut Data, t: HostTrap) -> Error {
    let e = match &t {
        HostTrap::OutOfFuel => Error::from(Trap::OutOfFuel),
        HostTrap::Msg(m) => Error::msg(m.clone()),
    };
    d.host_trap = Some(t);
    e
}

macro_rules! plain {
    ($l:expr, $k:expr, ($($a:ident : $t:ty => $conv:expr),*) -> ()) => {
        $l.func_wrap(abi::MODULE, IMPORTS[$k].0, |mut c: Caller<'_, Data>, $($a: $t),*| -> Result<(), Error> {
            let d = c.data_mut();
            match d.host.call($k as u8, &[$($conv),*]) {
                Ok(_) => Ok(()),
                Err(t) => Err(trap(d, t)),
            }
        })
    };
    ($l:expr, $k:expr, ($($a:ident : $t:ty => $conv:expr),*) -> $r:ty, $out:expr) => {
        $l.func_wrap(abi::MODULE, IMPORTS[$k].0, |mut c: Caller<'_, Data>, $($a: $t),*| -> Result<$r, Error> {
            let d = c.data_mut();
            match d.host.call($k as u8, &[$($conv),*]) {
                Ok(v) => Ok($out(v)),
                Err(t) => Err(trap(d, t)),
            }
        })
    };
}

fn i(v: i32) -> Raw {
    Raw::from_i32(v)
}
fn j(v: i64) -> Raw {
    Raw::from_i64(v)
}
fn f(v: f32) -> Raw {
    Raw(u64::from(v.to_bits()))
}

fn define(l: &mut Linker<Data>, mode: Mode) -> Result<(), Error> {
    plain!(l, 0, (a: i32 => i(a), b: i32 => i(b)) -> ())?;
    plain!(l, 1, (a: i32 => i(a), b: i32 => i(b)) -> ())?;
    plain!(l, 2, (a: i32 => i(a), b: f32 => f(b)) -> ())?;
    plain!(l, 3, (a: i32 => i(a), b: i32 => i(b)) -> ())?;
    plain!(l, 4, (a: i32 => i(a), b: i32 => i(b)) -> ())?;
    plain!(l, 5, (a: i32 => i(a), b: i32 => i(b)) -> ())?;
    plain!(l, 6, (a: i32 => i(a), b: f32 => f(b)) -> ())?;
    plain!(l, 7, (a: f32 => f(a), b: f32 => f(b), x: f32 => f(x), y: f32 => f(y)) -> ())?;
    plain!(l, 8, (a: i32 => i(a), b: i64 => j(b)) -> ())?;
    plain!(l, 9, (a: i32 => i(a)) -> ())?;
    plain!(l, 10, (a: i32 => i(a), b: f32 => f(b)) -> ())?;
    plain!(l, 11, (a: i32 => i(a), b: i64 => j(b)) -> ())?;
    plain!(l, 12, (a: i32 => i(a)) -> i64, |v: Raw| v.i64())?;
    plain!(l, 13, (a: i32 => i(a)) -> i32, |v: Raw| v.i32())?;
    plain!(l, 14, () -> i64, |v: Raw| v.i64())?;
    plain!(l, 15, () -> i32, |v: Raw| v.i32())?;
    plain!(l, 16, (a: i64 => j(a), b: i64 => j(b)) -> i64, |v: Raw| v.i64())?;
    let instrumented = matches!(mode, Mode::Instrumented { .. });
    l.func_wrap(
        abi::MODULE,
        IMPORTS[usize::from(ID_OF)].0,
        move |mut c: Caller<'_, Data>, kind: i32, ptr: i32, len: i32| -> Result<i32, Error> {
            let mem = match c.data().memory {
                Some(m) => Some(m),
                None => match c.get_export("memory") {
                    Some(Extern::Memory(m)) => Some(m),
                    _ => None,
                },
            };
            let Some(mem) = mem else {
                let d = c.data_mut();
                return Err(trap(
                    d,
                    HostTrap::Msg("pinball.id_of: the module exports no memory".into()),
                ));
            };
            let fg = if instrumented {
                match c.data().fuel_global {
                    Some(g) => Some(g),
                    None => c
                        .data()
                        .fuel_name
                        .clone()
                        .and_then(|n| c.get_export(&n))
                        .and_then(Extern::into_global),
                }
            } else {
                None
            };
            let left = match fg {
                Some(g) => match g.get(&mut c) {
                    Val::I64(v) => v.max(0) as u64,
                    _ => 0,
                },
                None => c.get_fuel().unwrap_or(0),
            };
            let (data, d) = mem.data_and_store_mut(&mut c);
            match d.host.id_of(kind, ptr, len, data, left) {
                Ok((id, rest)) => {
                    match fg {
                        Some(g) => {
                            let _ = g.set(&mut c, Val::I64(rest as i64));
                        }
                        None => {
                            let _ = c.set_fuel(rest);
                        }
                    }
                    Ok(id)
                }
                Err(t) => {
                    if t == HostTrap::OutOfFuel {
                        match fg {
                            Some(g) => {
                                let _ = g.set(&mut c, Val::I64(0));
                            }
                            None => {
                                let _ = c.set_fuel(0);
                            }
                        }
                    }
                    Err(trap(c.data_mut(), t))
                }
            }
        },
    )?;
    let _ = (V::I32, MAX_TABLES);
    Ok(())
}

/// A compiled module ready to instantiate.
pub struct Prepared {
    engine: Engine,
    module: Module,
    mode: Mode,
    /// Bytes handed to wasmi (instrumented or original).
    pub bytes_len: usize,
    /// Info from the load-time checks.
    pub info: ModuleInfo,
    /// Export names of the instrumentation (instrumented modules).
    pub names: Option<instrument::Names>,
}

/// Pre-scan + checks (+ instrumentation) + wasmi compile.
///
/// # Errors
/// As `WasmScriptBackend::prepare`.
pub fn prepare(
    bytes: &[u8],
    cfg: &WasmConfig,
    mode: Mode,
    eager: bool,
) -> Result<Prepared, ScriptError> {
    let (bin, info) = abi::prepare(bytes, cfg)?;
    let (bin, names) = match mode {
        Mode::WasmiFuel => (bin, None),
        Mode::Instrumented { depth, profile } => {
            let mut ic = instrument::Config::engine(cfg.fuel_per_callback, cfg.memory_limit_bytes);
            ic.depth_limit = depth;
            ic.profile = profile;
            let out = instrument::instrument(&bin, &ic)
                .map_err(|e| abi::load(format_args!("instrumentation: {e}")))?;
            (out.bytes, Some(out.names))
        }
    };
    let engine = Engine::new(&config(mode, eager)).map_err(|e| abi::load(format_args!("{e:#}")))?;
    let module = Module::new(&engine, &bin[..]).map_err(|e| abi::load(format_args!("{e:#}")))?;
    Ok(Prepared {
        engine,
        module,
        mode,
        bytes_len: bin.len(),
        info,
        names,
    })
}

/// Maps a failed guest call to its fault (pin_script `classify`).
fn classify(e: &Error, d: &mut Data, why: i32) -> ScriptError {
    if let Some(t) = d.host_trap.take() {
        return match t {
            HostTrap::OutOfFuel => ScriptError::BudgetExceeded,
            HostTrap::Msg(m) => ScriptError::Runtime(m),
        };
    }
    match why {
        WHY_FUEL => return ScriptError::BudgetExceeded,
        WHY_DEPTH => return ScriptError::Runtime("call stack exhausted (depth counter)".into()),
        _ => {}
    }
    match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => ScriptError::BudgetExceeded,
        Some(t) => ScriptError::Runtime(t.to_string()),
        None => ScriptError::Runtime(e.root_cause().to_string()),
    }
}

/// The wasmi rules backend.
pub struct WasmtimeInstr {
    store: Store<Data>,
    on_init: TypedFunc<(), ()>,
    on_tick: TypedFunc<(), ()>,
    on_switch: TypedFunc<(i32, i32), ()>,
    why: Option<Global>,
    fuel: Option<Global>,
    max_depth: Option<Global>,
    cfg: WasmConfig,
    mode: Mode,
    faulted: bool,
    fault_count: u32,
    last_error: Option<ScriptError>,
    /// Fuel consumed by the last callback (in the mode's units).
    pub last_fuel: u64,
    /// Largest fuel use of a callback so far.
    pub max_fuel: u64,
}

impl WasmtimeInstr {
    /// Instantiates `p` (start function under a fresh budget).
    ///
    /// # Errors
    /// As `WasmScriptBackend::new`.
    pub fn new(p: &Prepared, names: &NameTables, cfg: WasmConfig) -> Result<Self, ScriptError> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(cfg.memory_limit_bytes)
            .table_elements(MAX_TABLE_ELEMENTS)
            .tables(MAX_TABLES)
            .memories(1)
            .instances(1)
            .trap_on_grow_failure(false)
            .build();
        let data = Data {
            host: HostState::new(Arc::new(IdTables::new(names))),
            limits,
            memory: None,
            fuel_global: None,
            fuel_name: p.names.as_ref().map(|n| n.fuel.clone()),
            host_trap: None,
        };
        let mut store = Store::new(&p.engine, data);
        store.limiter(|d| &mut d.limits);
        let budget = cfg.fuel_per_callback;
        let backstop = budget.saturating_mul(4).saturating_add(4 << 20);
        let start_fuel = match p.mode {
            Mode::WasmiFuel => budget,
            Mode::Instrumented { .. } => backstop,
        };
        store.set_fuel(start_fuel).map_err(|e| abi::load(e))?;
        let mut linker = Linker::new(&p.engine);
        define(&mut linker, p.mode).map_err(|e| abi::load(e))?;
        let inst = match linker.instantiate(&mut store, &p.module) {
            Ok(i) => i,
            Err(e) => {
                let d = store.data_mut();
                return Err(match classify(&e, d, 0) {
                    ScriptError::Runtime(m) => ScriptError::Load(m),
                    other => other,
                });
            }
        };
        // Instrumented: the start function is exported and run here, with
        // the why/fuel globals readable.
        let gname = |f: fn(&instrument::Names) -> &str| p.names.as_ref().map(|n| f(n).to_owned());
        let fuel_name = gname(|n| &n.fuel);
        let why_name = gname(|n| &n.why);
        let max_name = gname(|n| &n.max_depth);
        let get_g = |store: &mut Store<Data>, n: &Option<String>| {
            n.as_deref().and_then(|n| inst.get_global(&mut *store, n))
        };
        if let Some(name) = p.names.as_ref().and_then(|n| n.start.as_deref()) {
            if let Some(g) = get_g(&mut store, &fuel_name) {
                let _ = g.set(&mut store, Val::I64(budget as i64 - 1));
            }
            let f = inst
                .get_typed_func::<(), ()>(&mut store, name)
                .map_err(|e| abi::load(e))?;
            if let Err(e) = f.call(&mut store, ()) {
                let why = match get_g(&mut store, &why_name).map(|g| g.get(&mut store)) {
                    Some(Val::I32(v)) => v,
                    _ => 0,
                };
                return Err(match classify(&e, store.data_mut(), why) {
                    ScriptError::Runtime(m) => ScriptError::Load(m),
                    other => other,
                });
            }
        }
        let memory = inst
            .get_memory(&mut store, "memory")
            .ok_or_else(|| abi::load("module exports no memory named \"memory\""))?;
        store.data_mut().memory = Some(memory);
        let typed = |e: Error| abi::load(e);
        let on_init = inst
            .get_typed_func::<(), ()>(&mut store, "on_init")
            .map_err(typed)?;
        let on_tick = inst
            .get_typed_func::<(), ()>(&mut store, "on_tick")
            .map_err(typed)?;
        let on_switch = inst
            .get_typed_func::<(i32, i32), ()>(&mut store, "on_switch_edge")
            .map_err(typed)?;
        let (why, fuel) = (get_g(&mut store, &why_name), get_g(&mut store, &fuel_name));
        let max_depth = get_g(&mut store, &max_name);
        store.data_mut().fuel_global = fuel;
        Ok(Self {
            store,
            on_init,
            on_tick,
            on_switch,
            why,
            fuel,
            max_depth,
            cfg,
            mode: p.mode,
            faulted: false,
            fault_count: 0,
            last_error: None,
            last_fuel: 0,
            max_fuel: 0,
        })
    }

    /// Deepest weighted call depth reached (profile mode).
    #[must_use]
    pub fn max_depth(&mut self) -> Option<i32> {
        let g = self.max_depth?;
        match Some(g.get(&mut self.store)) {
            Some(Val::I32(v)) => Some(v),
            _ => None,
        }
    }

    /// True while in the tilt state.
    #[must_use]
    pub fn is_faulted(&self) -> bool {
        self.faulted
    }

    /// The error behind the last fault.
    #[must_use]
    pub fn last_error(&self) -> Option<&ScriptError> {
        self.last_error.as_ref()
    }

    /// Guest memory size, bytes.
    #[must_use]
    pub fn memory_size(&self) -> usize {
        self.store
            .data()
            .memory
            .map_or(0, |m| m.data(&self.store).len())
    }

    /// Guest memory bytes.
    #[must_use]
    pub fn memory(&self) -> &[u8] {
        match self.store.data().memory {
            Some(m) => m.data(&self.store),
            None => &[],
        }
    }

    fn run(&mut self, entry: u8, a: i32, b: i32, ctx: &mut HostCtx) {
        let d = self.store.data_mut();
        if d.host.book.releases_pending() {
            d.host.book.drain_releases(ctx.cmd);
        }
        if self.faulted {
            return;
        }
        let mark = QueueMark::new(ctx.cmd, self.store.data().host.book.overflow);
        let budget = self.cfg.fuel_per_callback;
        match self.mode {
            Mode::WasmiFuel => {
                let _ = self.store.set_fuel(budget);
            }
            Mode::Instrumented { .. } => {
                let backstop = budget.saturating_mul(4).saturating_add(4 << 20);
                let _ = self.store.set_fuel(backstop);
                if let Some(g) = self.fuel {
                    let _ = g.set(&mut self.store, Val::I64(budget as i64));
                }
                if let Some(g) = self.why {
                    let _ = g.set(&mut self.store, Val::I32(0));
                }
            }
        }
        self.store.data_mut().host.enter(ctx);
        let r = match entry {
            0 => self.on_init.call(&mut self.store, ()),
            1 => self.on_tick.call(&mut self.store, ()),
            _ => self.on_switch.call(&mut self.store, (a, b)),
        };
        self.store.data_mut().host.leave();
        let left = match (self.mode, self.fuel) {
            (Mode::Instrumented { .. }, Some(g)) => match g.get(&mut self.store) {
                Val::I64(v) => v.max(0) as u64,
                _ => 0,
            },
            _ => self.store.get_fuel().unwrap_or(0),
        };
        self.last_fuel = budget.saturating_sub(left);
        self.max_fuel = self.max_fuel.max(self.last_fuel);
        if let Err(e) = r {
            let why = match self.why {
                Some(g) => match g.get(&mut self.store) {
                    Val::I32(v) => v,
                    _ => 0,
                },
                None => 0,
            };
            let err = classify(&e, self.store.data_mut(), why);
            let book = &mut self.store.data_mut().host.book;
            mark.rollback(ctx.cmd, book);
            book.release_holds(ctx.cmd);
            self.faulted = true;
            self.fault_count = self.fault_count.saturating_add(1);
            self.last_error = Some(err);
        }
    }
}

impl TableScriptBackend for WasmtimeInstr {
    fn on_init(&mut self, ctx: &mut HostCtx) {
        self.run(0, 0, 0, ctx);
    }
    fn on_tick(&mut self, ctx: &mut HostCtx) {
        self.run(1, 0, 0, ctx);
    }
    fn on_switch_edge(&mut self, switch: SwitchId, edge: SwitchEdge, ctx: &mut HostCtx) {
        let closed = i32::from(edge == SwitchEdge::Closed);
        self.run(2, i32::from(switch.0), closed, ctx);
    }
    fn apply_reload(&mut self, _source: &[u8]) -> Result<(), ScriptError> {
        Err(ScriptError::Load(
            "hot reload is not available on the web".into(),
        ))
    }
}

impl pin_sim::ExternalScript for WasmtimeInstr {
    fn overflow_count(&self) -> u32 {
        self.store.data().host.book.overflow
    }
    fn is_faulted(&self) -> bool {
        self.faulted
    }
    fn fault_count(&self) -> u32 {
        self.fault_count
    }
    fn last_error(&self) -> Option<&ScriptError> {
        self.last_error.as_ref()
    }
}
