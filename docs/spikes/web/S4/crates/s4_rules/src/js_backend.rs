//! Design B: the rules module runs on the browser's own WebAssembly engine
//! (JIT), rewritten by [`crate::instrument`] (wasmtime-exact fuel, optional
//! depth counter, capped memory, host-error wrappers).
//!
//! Wiring (single thread): engine wasm → JS `khzRules.call` → rules
//! instance export → **direct wasm→wasm** call of this module's
//! `khz_host_*` exports (no JS on the host-call path, no BigInt). A host
//! function never traps inside Rust: it records the error and returns; the
//! instrumented wrapper then calls `khz.host_err` and traps in the rules
//! instance, so no engine-module frame is ever unwound (S1: a trap through
//! Rust frames leaks shadow stack). `id_of` goes through JS (it reads the
//! rules instance's memory).

use crate::abi;
use crate::hostcore::{HostState, HostTrap, IdTables, QueueMark, Raw, id_kind};
use crate::instrument::{self, WHY_DEPTH, WHY_FUEL, WHY_HOST};
use pin_core::ids::SwitchId;
use pin_core::{HostCtx, ScriptError, SwitchEdge, TableScriptBackend};
use pin_script::NameTables;
use pin_script::wasm::WasmConfig;
use std::cell::{Cell, RefCell};
use std::sync::Arc;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = khzRules, js_name = instantiate)]
    fn js_instantiate(bytes: &[u8], names: &str, start_fuel: f64) -> i32;
    #[wasm_bindgen(js_namespace = khzRules, js_name = call)]
    fn js_call(h: i32, entry: i32, a: i32, b: i32, fuel: f64) -> i32;
    #[wasm_bindgen(js_namespace = khzRules, js_name = fuel)]
    fn js_fuel(h: i32) -> f64;
    #[wasm_bindgen(js_namespace = khzRules, js_name = lastError)]
    fn js_last_error() -> String;
    #[wasm_bindgen(js_namespace = khzRules, js_name = release)]
    fn js_release(h: i32);
    #[wasm_bindgen(js_namespace = khzRules, js_name = memory)]
    fn js_memory(h: i32) -> Vec<u8>;
}

thread_local! {
    static CUR: Cell<*mut HostState> = const { Cell::new(std::ptr::null_mut()) };
    static ERR: RefCell<Option<HostTrap>> = const { RefCell::new(None) };
    static SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static ID_FUEL: Cell<f64> = const { Cell::new(0.0) };
}

fn host_call(k: u8, a: &[Raw]) -> Raw {
    let p = CUR.with(Cell::get);
    if p.is_null() {
        ERR.with(|e| {
            *e.borrow_mut() = Some(HostTrap::Msg(
                "the pinball API is only available inside on_init, on_tick and on_switch_edge"
                    .into(),
            ));
        });
        return Raw(0);
    }
    // SAFETY: `CUR` points at the backend's boxed HostState for the duration
    // of one `js_call` made through `&mut JsBackend` (set before, cleared
    // after); single-threaded, and the backend does not touch it meanwhile.
    let h = unsafe { &mut *p };
    match h.call(k, a) {
        Ok(v) => v,
        Err(t) => {
            ERR.with(|e| *e.borrow_mut() = Some(t));
            Raw(0)
        }
    }
}

macro_rules! host_export {
    ($name:ident, $k:expr, ($($a:ident : $t:ty => $conv:expr),*)) => {
        #[doc = "JIT host import."]
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($a: $t),*) {
            host_call($k, &[$($conv),*]);
        }
    };
    ($name:ident, $k:expr, ($($a:ident : $t:ty => $conv:expr),*) -> $r:ty, $out:expr) => {
        #[doc = "JIT host import."]
        #[unsafe(no_mangle)]
        pub extern "C" fn $name($($a: $t),*) -> $r {
            $out(host_call($k, &[$($conv),*]))
        }
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

host_export!(khz_host_pulse_coil, 0, (a: i32 => i(a), b: i32 => i(b)));
host_export!(khz_host_hold_coil, 1, (a: i32 => i(a), b: i32 => i(b)));
host_export!(khz_host_set_magnet, 2, (a: i32 => i(a), b: f32 => f(b)));
host_export!(khz_host_set_zipper, 3, (a: i32 => i(a), b: i32 => i(b)));
host_export!(khz_host_set_diverter, 4, (a: i32 => i(a), b: i32 => i(b)));
host_export!(khz_host_spawn_ball, 5, (a: i32 => i(a), b: i32 => i(b)));
host_export!(khz_host_set_lamp, 6, (a: i32 => i(a), b: f32 => f(b)));
host_export!(khz_host_set_gi_tint, 7, (a: f32 => f(a), b: f32 => f(b), c: f32 => f(c), d: f32 => f(d)));
host_export!(khz_host_set_score, 8, (a: i32 => i(a), b: i64 => j(b)));
host_export!(khz_host_show_scene, 9, (a: i32 => i(a)));
host_export!(khz_host_play_sound, 10, (a: i32 => i(a), b: f32 => f(b)));
host_export!(khz_host_persist_set, 11, (a: i32 => i(a), b: i64 => j(b)));
host_export!(khz_host_persist_get, 12, (a: i32 => i(a)) -> i64, |v: Raw| v.i64());
host_export!(khz_host_switch, 13, (a: i32 => i(a)) -> i32, |v: Raw| v.i32());
host_export!(khz_host_tick, 14, () -> i64, |v: Raw| v.i64());
host_export!(khz_host_balls_in_play, 15, () -> i32, |v: Raw| v.i32());
host_export!(khz_host_random, 16, (a: i64 => j(a), b: i64 => j(b)) -> i64, |v: Raw| v.i64());

/// `khz.host_err`: 1 if the last host call failed.
#[unsafe(no_mangle)]
pub extern "C" fn khz_host_err() -> i32 {
    ERR.with(|e| i32::from(e.borrow().is_some()))
}

/// Scratch buffer of `len` bytes for an `id_of` name (JS copies into it).
#[unsafe(no_mangle)]
pub extern "C" fn khz_scratch(len: i32) -> i32 {
    SCRATCH.with(|s| {
        let mut s = s.borrow_mut();
        let n = len.max(0) as usize;
        if s.len() < n {
            s.resize(n, 0);
        }
        s.as_mut_ptr() as i32
    })
}

/// `id_of` for the JIT host: `oob` = the name is out of the rules
/// instance's memory; otherwise its `len` bytes are in the scratch buffer.
/// Returns the id or −1, or −2 with the host error recorded; the fuel left
/// afterwards is `khz_id_of_fuel()`.
#[unsafe(no_mangle)]
pub extern "C" fn khz_host_id_of(kind: i32, ptr: i32, len: i32, oob: i32, fuel_left: f64) -> i32 {
    let p = CUR.with(Cell::get);
    let fail = |t: HostTrap| {
        ERR.with(|e| *e.borrow_mut() = Some(t));
        -2
    };
    let k = match id_kind(kind) {
        Ok(k) => k,
        Err(t) => return fail(t),
    };
    if p.is_null() {
        return fail(HostTrap::Msg("pinball.id_of: no instance".into()));
    }
    // SAFETY: as `host_call`.
    let h = unsafe { &*p };
    let (start, n) = (ptr as u32 as usize, len as u32 as usize);
    let left = if fuel_left > 0.0 { fuel_left as u64 } else { 0 };
    let r = SCRATCH.with(|s| {
        let s = s.borrow();
        let name = (oob == 0).then(|| &s[..n]);
        h.id_of_parts(k, start, n, name, left)
    });
    match r {
        Ok((id, rest)) => {
            ID_FUEL.with(|c| c.set(rest as f64));
            id
        }
        Err(t) => {
            ID_FUEL.with(|c| c.set(0.0));
            fail(t)
        }
    }
}

/// Fuel left after the last `khz_host_id_of`.
#[unsafe(no_mangle)]
pub extern "C" fn khz_id_of_fuel() -> f64 {
    ID_FUEL.with(Cell::get)
}

/// The JIT rules backend.
pub struct JsBackend {
    handle: i32,
    host: Box<HostState>,
    cfg: WasmConfig,
    faulted: bool,
    fault_count: u32,
    last_error: Option<ScriptError>,
    last_fuel: u64,
}

impl Drop for JsBackend {
    fn drop(&mut self) {
        js_release(self.handle);
    }
}

fn status_error(st: i32) -> ScriptError {
    match st {
        WHY_FUEL => ScriptError::BudgetExceeded,
        WHY_DEPTH => ScriptError::Runtime("call stack exhausted (depth counter)".into()),
        WHY_HOST => match ERR.with(|e| e.borrow_mut().take()) {
            Some(HostTrap::OutOfFuel) => ScriptError::BudgetExceeded,
            Some(HostTrap::Msg(m)) => ScriptError::Runtime(m),
            None => ScriptError::Runtime("host error".into()),
        },
        5 => ScriptError::Runtime(format!(
            "call stack exhausted (RangeError: {})",
            js_last_error()
        )),
        _ => ScriptError::Runtime(js_last_error()),
    }
}

impl JsBackend {
    /// Pre-scan, checks, instrumentation, then compile + instantiate in JS
    /// (start function under a fresh budget).
    ///
    /// # Errors
    /// As `WasmScriptBackend::new`.
    pub fn new(
        src: &[u8],
        names: &NameTables,
        cfg: WasmConfig,
        depth: Option<(u32, instrument::DepthWeight)>,
    ) -> Result<Self, ScriptError> {
        let (bin, _info) = abi::prepare(src, &cfg)?;
        let mut ic = instrument::Config::engine(cfg.fuel_per_callback, cfg.memory_limit_bytes);
        ic.depth_limit = depth;
        ic.host_err_import = true;
        ic.nan_canon = true;
        let out = instrument::instrument(&bin, &ic)
            .map_err(|e| abi::load(format_args!("instrumentation: {e}")))?;
        let n = &out.names;
        let csv = format!(
            "{},{},{},{},{},{}",
            n.begin,
            n.get_fuel,
            n.set_fuel,
            n.get_why,
            n.start.as_deref().unwrap_or(""),
            n.why
        );
        let mut host = Box::new(HostState::new(Arc::new(IdTables::new(names))));
        ERR.with(|e| *e.borrow_mut() = None);
        CUR.with(|c| c.set(std::ptr::from_mut(&mut *host)));
        let h = js_instantiate(&out.bytes, &csv, cfg.fuel_per_callback as f64 - 1.0);
        CUR.with(|c| c.set(std::ptr::null_mut()));
        if h < 0 {
            let e = if h == -1 {
                abi::load(js_last_error())
            } else {
                match status_error(-h - 1) {
                    ScriptError::Runtime(m) => ScriptError::Load(m),
                    other => other,
                }
            };
            ERR.with(|e| *e.borrow_mut() = None);
            return Err(e);
        }
        Ok(Self {
            handle: h,
            host,
            cfg,
            faulted: false,
            fault_count: 0,
            last_error: None,
            last_fuel: 0,
        })
    }

    fn run(&mut self, entry: i32, a: i32, b: i32, ctx: &mut HostCtx) {
        if self.host.book.releases_pending() {
            self.host.book.drain_releases(ctx.cmd);
        }
        if self.faulted {
            return;
        }
        let mark = QueueMark::new(ctx.cmd, self.host.book.overflow);
        let budget = self.cfg.fuel_per_callback;
        self.host.enter(ctx);
        CUR.with(|c| c.set(std::ptr::from_mut(&mut *self.host)));
        let st = js_call(self.handle, entry, a, b, budget as f64);
        CUR.with(|c| c.set(std::ptr::null_mut()));
        self.host.leave();
        let left = js_fuel(self.handle);
        self.last_fuel = budget.saturating_sub(if left > 0.0 { left as u64 } else { 0 });
        if st != 0 {
            let err = status_error(st);
            ERR.with(|e| *e.borrow_mut() = None);
            mark.rollback(ctx.cmd, &mut self.host.book);
            self.host.book.release_holds(ctx.cmd);
            self.faulted = true;
            self.fault_count = self.fault_count.saturating_add(1);
            self.last_error = Some(err);
        }
    }
}

impl TableScriptBackend for JsBackend {
    fn on_init(&mut self, ctx: &mut HostCtx) {
        self.run(0, 0, 0, ctx);
    }
    fn on_tick(&mut self, ctx: &mut HostCtx) {
        self.run(1, 0, 0, ctx);
    }
    fn on_switch_edge(&mut self, switch: SwitchId, edge: SwitchEdge, ctx: &mut HostCtx) {
        self.run(
            2,
            i32::from(switch.0),
            i32::from(edge == SwitchEdge::Closed),
            ctx,
        );
    }
    fn apply_reload(&mut self, _source: &[u8]) -> Result<(), ScriptError> {
        Err(ScriptError::Load(
            "hot reload is not available on the web".into(),
        ))
    }
}

impl pin_sim::ExternalScript for JsBackend {
    fn overflow_count(&self) -> u32 {
        self.host.book.overflow
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

impl crate::run::Rules for JsBackend {
    fn last_fuel(&self) -> u64 {
        self.last_fuel
    }
    fn memory(&self) -> Vec<u8> {
        js_memory(self.handle)
    }
}
