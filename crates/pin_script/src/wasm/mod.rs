//! WebAssembly (wasmtime) backend for table rules (PLAN.md §4 Subphase 4.5,
//! D10, D25). It mirrors the Lua backend ([`crate::lua`]): the same host
//! API, the same validation, the same fault ("tilt") and rollback semantics,
//! and bit-identical command streams for integer-only rules.
//!
//! # Guest ABI
//!
//! A rules module is a core WebAssembly module (binary or WAT text). It may
//! import only functions from the module `"pinball"`, with exactly these
//! signatures (`i32` ids; booleans are `i32` 0/1 and any other value traps):
//!
//! | import | signature |
//! |---|---|
//! | `pulse_coil(coil, ms)` | `(i32, i32)` — `ms` in `0..=65535` |
//! | `hold_coil(coil, on)` | `(i32, i32)` |
//! | `set_magnet(magnet, power)` | `(i32, f32)` — power in `[0, 1]` |
//! | `set_zipper(flipper, closed)` | `(i32, i32)` |
//! | `set_diverter(diverter, on)` | `(i32, i32)` |
//! | `spawn_ball(dispenser, profile)` | `(i32, i32)` |
//! | `set_lamp(lamp, intensity)` | `(i32, f32)` — intensity in `[0, 1]` |
//! | `set_gi_tint(r, g, b, a)` | `(f32, f32, f32, f32)` — each in `[0, 1]` |
//! | `set_score(player, score)` | `(i32, i64)` — player `1..=4` |
//! | `show_scene(scene)` | `(i32)` |
//! | `play_sound(sound, gain)` | `(i32, f32)` — gain finite and `>= 0` |
//! | `persist_set(key, value)` | `(i32, i64)` — key `< 64` |
//! | `persist_get(key) -> i64` | `(i32) -> i64` |
//! | `switch(id) -> i32` | `(i32) -> i32` (0/1) |
//! | `tick() -> i64` | `() -> i64` |
//! | `balls_in_play() -> i32` | `() -> i32` |
//! | `random(m, n) -> i64` | `(i64, i64) -> i64` — uniform in `[m, n]`, `m > n` traps |
//! | `id_of(kind, ptr, len) -> i32` | `(i32, i32, i32) -> i32` |
//!
//! `random` is Lua's `pinball.random(m, n)`: the same host PCG and the same
//! draw sequence. `id_of` looks up the UTF-8 name at `[ptr, ptr + len)` in
//! guest memory in table `kind` (0 sw, 1 coil, 2 lamp, 3 magnet, 4 diverter,
//! 5 flipper, 6 dispenser, 7 profile, 8 sound, 9 scene, 10 persist_key) and
//! returns its id or −1; a kind outside `0..=10` or an out-of-bounds name
//! traps. `id_of` costs one fuel unit per name byte plus one, never
//! allocates, and is the only import usable outside the three callbacks.
//!
//! Required exports: `on_init: () -> ()`, `on_tick: () -> ()`,
//! `on_switch_edge: (id: i32, closed: i32) -> ()` and `memory`. The entry
//! points must be functions the module defines: re-exporting an imported
//! host function under one of these names is rejected. Optional
//! exports (hot-swap state, see below): `save_state: () -> i64`,
//! `restore_state: (ptr: i32, len: i32) -> ()`, `alloc_state: (len: i32) -> i32`.
//! A missing required export, an export of the wrong type, or any other
//! import (WASI, unknown names, memories, tables, globals, wrong signatures)
//! is rejected with [`ScriptError::Load`] when the module is loaded.
//!
//! # Sandbox
//!
//! * No WASI and no other host access than the imports above.
//! * Memory cap ([`WasmConfig::memory_limit_bytes`]): a module whose declared
//!   initial memory exceeds the cap is rejected at load
//!   ([`ScriptError::MemoryLimit`]). At run time `memory.grow` past the cap
//!   **returns −1 to the guest** (it does not trap); the memory never exceeds
//!   the cap. Tables are limited to [`MAX_TABLES`] tables of at most
//!   [`MAX_TABLE_ELEMENTS`] elements; one memory, one instance per store.
//! * Fuel metering is the per-callback budget
//!   ([`WasmConfig::fuel_per_callback`]): about one unit per Wasm operator,
//!   and bulk operations are charged by size on top of that, one unit per
//!   byte (`memory.fill`, `memory.copy`, `memory.init`), per element
//!   (`table.fill`, `table.copy`, `table.init`, `table.grow`) or per page
//!   (`memory.grow`). A bulk operation's size-proportional fuel is charged
//!   when it completes, so one operation can overshoot the budget by at most
//!   its own size (bounded by the memory cap and the table limits) before
//!   the callback traps. Every callback, the start function and every state
//!   hook start with a fresh budget; exhausting it traps
//!   ([`ScriptError::BudgetExceeded`]).
//! * Module size (checked before anything is compiled, so a hostile module
//!   cannot make compilation take seconds or gigabytes): at most
//!   [`MAX_MODULE_BYTES`] of binary (WAT text: at most [`MAX_WAT_BYTES`] of
//!   text, and the binary it produces at most [`MAX_MODULE_BYTES`]), at most
//!   [`MAX_FUNCTIONS`] defined functions, no function body larger than
//!   [`MAX_FUNCTION_BODY_BYTES`], at most [`MAX_GLOBALS`] globals,
//!   [`MAX_TOTAL_TABLE_ELEMENTS`] initial table elements in total and
//!   [`MAX_SEGMENTS`] data and [`MAX_SEGMENTS`] element segments. Anything
//!   above is [`ScriptError::Load`].
//! * Stack: Wasm code may use at most [`MAX_WASM_STACK_BYTES`] of the
//!   calling thread's stack; deeper recursion traps (a fault, like any other
//!   trap). The thread that calls the backend (the sim thread) must have at
//!   least [`MIN_HOST_STACK_BYTES`] of stack.
//! * Determinism: NaN canonicalisation on, threads/shared memory off, relaxed
//!   SIMD off (and deterministic if it were on), memory64, multi-memory, GC,
//!   exceptions, stack switching and the component model off. All Wasm
//!   execution is therefore a pure function of the module, the host inputs
//!   and the host PCG.
//!
//! # Faults ("tilt")
//!
//! A trap (including fuel exhaustion and argument validation failures in the
//! imports) aborts the callback, and the commands it queued are dropped
//! (all-or-nothing; holds it dropped count as held so they get released).
//! The backend then queues `(coil, false)` for every coil the script holds
//! (continuing on later callbacks if the queue is full), stops calling the
//! guest and stays faulted until a successful reload — exactly like the Lua
//! backend.
//!
//! # Hot swap
//!
//! [`WasmScriptBackend::prepare`] compiles and validates a module on any
//! thread (the I/O worker); [`WasmScriptBackend::apply_prepared`]
//! instantiates it at a tick boundary on the sim thread:
//!
//! 1. The new module is instantiated (its start function runs under a fresh
//!    budget; it cannot use the context imports).
//! 2. If the old instance is not faulted and exports `save_state`, it is
//!    called; its `i64` result packs `(ptr << 32) | len` of a byte blob in the
//!    old guest memory. A blob longer than [`MAX_STATE_BYTES`], out of
//!    bounds, or a trapping `save_state` means "no state to carry".
//! 3. If there is a blob and the new module exports `restore_state`: an
//!    empty blob calls `restore_state(0, 0)`; otherwise the host calls the new
//!    `alloc_state(len) -> ptr`, copies the blob to `[ptr, ptr + len)` of the
//!    new memory and calls `restore_state(ptr, len)`. Without `alloc_state` a
//!    non-empty blob is not delivered (and `restore_state` is not called). A
//!    trap in `alloc_state`/`restore_state` or an out-of-bounds `ptr` fails
//!    the swap.
//! 4. On any error the old instance keeps running untouched and the error is
//!    returned; on success the faulted state is cleared (fault count, last
//!    error, overflow count and held-coil tracking carry over). `on_init` is
//!    not called again.
//!
//! # Allocation
//!
//! The callback paths (`on_tick`, `on_switch_edge`, `on_init`) do not
//! allocate on the host side: entry points are pre-resolved `TypedFunc`s,
//! the context is passed through raw pointers in the store data, name
//! lookups are binary searches over guest memory, and only error paths
//! (which fault the script) format messages. Verified by `tests/wasm_alloc.rs`.

mod host;

use host::{HostState, IdTables, V};
use pin_core::ids::SwitchId;
use pin_core::{HostCommandQueue, HostCtx, ScriptError, SwitchEdge, TableScriptBackend};
use std::borrow::Cow;
use std::fmt;
use std::sync::{Arc, OnceLock};
use wasmtime::wasmparser::{
    ElementItems, ExternalKind, FunctionBody, Operator, OperatorsReader, Parser, Payload, TypeRef,
};
use wasmtime::{
    Config, Engine, ExternType, Linker, Memory, Module, Store, StoreLimitsBuilder, Trap, TypedFunc,
    ValType,
};

use crate::NameTables;

/// Largest blob `save_state` may hand over, bytes.
pub const MAX_STATE_BYTES: usize = 64 * 1024;

/// Most tables a module may define.
pub const MAX_TABLES: usize = 4;

/// Most elements of one table (initial size and growth).
pub const MAX_TABLE_ELEMENTS: usize = 65_536;

/// Largest binary module accepted, bytes (D25, checked before compiling).
pub const MAX_MODULE_BYTES: usize = 1024 * 1024;

/// Largest WAT text module accepted, bytes. The binary it compiles to must
/// still fit [`MAX_MODULE_BYTES`].
pub const MAX_WAT_BYTES: usize = 4 * 1024 * 1024;

/// Most functions a module may define (imports not counted).
pub const MAX_FUNCTIONS: u32 = 10_000;

/// Largest single function body, bytes of its binary encoding.
pub const MAX_FUNCTION_BODY_BYTES: u64 = 256 * 1024;

/// Most globals a module may define.
pub const MAX_GLOBALS: u32 = 1_000;

/// Most table elements all of a module's tables may start with together.
pub const MAX_TOTAL_TABLE_ELEMENTS: u64 = 100_000;

/// Most data segments, and separately most element segments, of a module.
pub const MAX_SEGMENTS: u32 = 1_000;

/// Most declared locals of one function (parameters not counted).
pub const MAX_LOCALS_PER_FUNCTION: u64 = 1_024;

/// Most declared locals of all functions together.
pub const MAX_TOTAL_LOCALS: u64 = 65_536;

/// Deepest nesting of `block` / `loop` / `if` in a function.
pub const MAX_NESTING: u64 = 128;

/// Most targets (default included) of one `br_table`.
pub const MAX_BR_TABLE_TARGETS: u64 = 1_024;

/// Most `br_table` targets (defaults included) in one function.
pub const MAX_BR_TABLE_TARGETS_PER_FUNCTION: u64 = 16_384;

/// Most operators (instructions, `end` included) in one function body.
pub const MAX_OPERATORS_PER_FUNCTION: u64 = 65_536;

/// Most operators in all function bodies together.
pub const MAX_OPERATORS: u64 = 1_048_576;

/// Most entries of the type section.
pub const MAX_TYPES: u32 = 1_024;

/// Most parameters of one function type.
pub const MAX_PARAMS: u32 = 32;

/// Most results of one function type.
pub const MAX_RESULTS: u32 = 8;

/// Most items (function indices or expressions) of all element segments.
pub const MAX_ELEMENT_ITEMS: u64 = 65_536;

/// Most exports.
pub const MAX_EXPORTS: u32 = 1_024;

/// Most imports (the ABI has 18 functions).
pub const MAX_IMPORTS: u32 = 64;

/// Stack Wasm code may use before it traps, bytes.
pub const MAX_WASM_STACK_BYTES: usize = 256 * 1024;

/// Smallest stack the thread calling a [`WasmScriptBackend`] must have,
/// bytes (Wasm frames up to [`MAX_WASM_STACK_BYTES`] plus the host frames of
/// the sim loop, the backend and wasmtime's trap handling).
pub const MIN_HOST_STACK_BYTES: usize = 2 * 1024 * 1024;

/// Bytes per Wasm page.
const PAGE: u64 = 65_536;

/// Limits of one Wasm backend (§3.4, D25).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct WasmConfig {
    /// Hard cap on the guest's linear memory, bytes.
    pub memory_limit_bytes: usize,
    /// Fuel (≈ Wasm operators) allowed per callback.
    pub fuel_per_callback: u64,
}

impl WasmConfig {
    /// The configuration the engine runs table rules with, matching
    /// [`crate::lua::LuaConfig::engine`]: a 2 MiB memory cap and 100 000 fuel
    /// per callback. [`WasmConfig::default`] keeps the larger tool/test
    /// limits.
    pub const fn engine() -> Self {
        Self {
            memory_limit_bytes: 2 * 1024 * 1024,
            fuel_per_callback: 100_000,
        }
    }
}

impl Default for WasmConfig {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 16 * 1024 * 1024,
            fuel_per_callback: 1_000_000,
        }
    }
}

/// The process-wide engine (one `Config` for every backend, so modules
/// compiled on any thread can be instantiated by any backend).
fn engine() -> Result<&'static Engine, ScriptError> {
    static ENGINE: OnceLock<Result<Engine, String>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
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
                .max_wasm_stack(MAX_WASM_STACK_BYTES);
            Engine::new(&c).map_err(|e| format!("{e:#}"))
        })
        .as_ref()
        .map_err(|e| ScriptError::Load(format!("wasm engine unavailable: {e}")))
}

/// The process-wide linker holding the `pinball` imports.
fn linker() -> Result<&'static Linker<HostState>, ScriptError> {
    static LINKER: OnceLock<Result<Linker<HostState>, String>> = OnceLock::new();
    let engine = engine()?;
    LINKER
        .get_or_init(|| {
            let mut l = Linker::new(engine);
            host::define(&mut l)
                .map(|()| l)
                .map_err(|e| format!("{e:#}"))
        })
        .as_ref()
        .map_err(|e| ScriptError::Load(format!("wasm linker: {e}")))
}

fn load(msg: impl fmt::Display) -> ScriptError {
    ScriptError::Load(msg.to_string())
}

fn val(v: &ValType) -> Option<V> {
    match v {
        ValType::I32 => Some(V::I32),
        ValType::I64 => Some(V::I64),
        ValType::F32 => Some(V::F32),
        _ => None,
    }
}

/// True if the extern is a function with exactly `params -> results`.
fn func_is(ty: &ExternType, params: &[V], results: &[V]) -> bool {
    let ExternType::Func(f) = ty else {
        return false;
    };
    f.params()
        .map(|p| val(&p))
        .eq(params.iter().copied().map(Some))
        && f.results()
            .map(|r| val(&r))
            .eq(results.iter().copied().map(Some))
}

/// Exported functions: `(name, required, params, results)`.
const EXPORTS: &[(&str, bool, &[V], &[V])] = &[
    ("on_init", true, &[], &[]),
    ("on_tick", true, &[], &[]),
    ("on_switch_edge", true, &[V::I32, V::I32], &[]),
    ("save_state", false, &[], &[V::I64]),
    ("restore_state", false, &[V::I32, V::I32], &[]),
    ("alloc_state", false, &[V::I32], &[V::I32]),
];

/// Static checks against `cfg`: declared memory and table sizes.
fn check_resources(module: &Module, cfg: &WasmConfig) -> Result<(), ScriptError> {
    let r = module.resources_required();
    if let Some(pages) = r.max_initial_memory_size {
        let bytes = pages.saturating_mul(PAGE);
        if bytes > cfg.memory_limit_bytes as u64 {
            return Err(ScriptError::MemoryLimit);
        }
    }
    if r.num_tables as usize > MAX_TABLES {
        return Err(load(format_args!(
            "module defines {} tables (at most {MAX_TABLES})",
            r.num_tables
        )));
    }
    if r.max_initial_table_size.unwrap_or(0) > MAX_TABLE_ELEMENTS as u64 {
        return Err(load(format_args!(
            "a table starts with more than {MAX_TABLE_ELEMENTS} elements"
        )));
    }
    Ok(())
}

/// Longest WAT parse error kept in a [`ScriptError::Load`] message, bytes
/// (the parser quotes the offending line, which may be megabytes long).
pub const MAX_ERROR_BYTES: usize = 256;

/// `s` cut to at most `max` bytes on a character boundary, with `…` (3
/// bytes, counted) marking a cut.
fn truncated(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max - '…'.len_utf8();
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Converts WAT text to binary and checks the size limits (see the module
/// docs) without compiling anything. Returns the binary.
fn precheck(bytes: &[u8]) -> Result<Cow<'_, [u8]>, ScriptError> {
    let binary = bytes.starts_with(b"\0asm");
    let cap = if binary {
        MAX_MODULE_BYTES
    } else {
        MAX_WAT_BYTES
    };
    if bytes.len() > cap {
        return Err(load(format_args!(
            "module is {} bytes (at most {cap} bytes of {})",
            bytes.len(),
            if binary { "binary" } else { "WAT text" }
        )));
    }
    let bin = if binary {
        Cow::Borrowed(bytes)
    } else {
        Cow::Owned(
            wat::parse_bytes(bytes)
                .map_err(|e| load(truncated(&e.to_string(), MAX_ERROR_BYTES)))?
                .into_owned(),
        )
    };
    if bin.len() > MAX_MODULE_BYTES {
        return Err(load(format_args!(
            "module compiles to {} bytes of binary (at most {MAX_MODULE_BYTES})",
            bin.len()
        )));
    }
    scan(&bin)?;
    Ok(bin)
}

/// Counts what compiling `bin` would cost and rejects modules above the
/// limits, and modules exporting an imported function as an entry point.
fn scan(bin: &[u8]) -> Result<(), ScriptError> {
    let mut sc = Scan::default();
    for payload in Parser::new(0).parse_all(bin) {
        sc.payload(payload.map_err(parse_err)?)?;
    }
    Ok(())
}

fn parse_err(e: wasmtime::wasmparser::BinaryReaderError) -> ScriptError {
    load(format_args!("{e}"))
}

/// `Err(Load("<who> has <n> <what> (at most <max>)"))` when `n > max`.
fn cap(who: fmt::Arguments<'_>, what: &str, n: u64, max: u64) -> Result<(), ScriptError> {
    if n > max {
        return Err(load(format_args!("{who} has {n} {what} (at most {max})")));
    }
    Ok(())
}

/// Running totals of [`scan`].
#[derive(Default)]
struct Scan {
    imported_funcs: u32,
    /// Index (among defined functions) of the next code section entry.
    func: u32,
    locals: u64,
    operators: u64,
    element_items: u64,
    /// Reused frame stack of the operator reader.
    allocs: wasmtime::wasmparser::OperatorsReaderAllocations,
}

impl Scan {
    fn payload(&mut self, payload: Payload<'_>) -> Result<(), ScriptError> {
        let module = format_args!("module");
        match payload {
            Payload::TypeSection(r) => {
                cap(module, "types", r.count().into(), MAX_TYPES.into())?;
                for (i, ty) in r.into_iter_err_on_gc_types().enumerate() {
                    let ty = ty.map_err(parse_err)?;
                    let who = format_args!("type {i}");
                    cap(who, "params", ty.params().len() as u64, MAX_PARAMS.into())?;
                    let who = format_args!("type {i}");
                    cap(
                        who,
                        "results",
                        ty.results().len() as u64,
                        MAX_RESULTS.into(),
                    )?;
                }
            }
            Payload::ImportSection(r) => {
                let mut n = 0u64;
                for imp in r.into_imports() {
                    n += 1;
                    cap(module, "imports", n, MAX_IMPORTS.into())?;
                    if matches!(
                        imp.map_err(parse_err)?.ty,
                        TypeRef::Func(_) | TypeRef::FuncExact(_)
                    ) {
                        self.imported_funcs += 1;
                    }
                }
            }
            Payload::FunctionSection(r) => {
                cap(module, "functions", r.count().into(), MAX_FUNCTIONS.into())?;
            }
            Payload::CodeSectionStart { count, .. } => {
                cap(
                    module,
                    "function bodies",
                    count.into(),
                    MAX_FUNCTIONS.into(),
                )?;
            }
            Payload::CodeSectionEntry(body) => self.body(&body)?,
            Payload::GlobalSection(r) => {
                cap(module, "globals", r.count().into(), MAX_GLOBALS.into())?;
            }
            Payload::TableSection(r) => {
                let mut total = 0u64;
                for t in r {
                    total = total.saturating_add(t.map_err(parse_err)?.ty.initial);
                }
                cap(
                    module,
                    "initial table elements",
                    total,
                    MAX_TOTAL_TABLE_ELEMENTS,
                )?;
            }
            Payload::DataSection(r) => {
                cap(
                    module,
                    "data segments",
                    r.count().into(),
                    MAX_SEGMENTS.into(),
                )?;
            }
            Payload::ElementSection(r) => {
                cap(
                    module,
                    "element segments",
                    r.count().into(),
                    MAX_SEGMENTS.into(),
                )?;
                for e in r {
                    let n = match e.map_err(parse_err)?.items {
                        ElementItems::Functions(f) => f.count(),
                        ElementItems::Expressions(_, x) => x.count(),
                    };
                    self.element_items += u64::from(n);
                    let total = self.element_items;
                    cap(module, "element segment items", total, MAX_ELEMENT_ITEMS)?;
                }
            }
            Payload::ExportSection(r) => {
                cap(module, "exports", r.count().into(), MAX_EXPORTS.into())?;
                for e in r {
                    let e = e.map_err(parse_err)?;
                    if e.kind == ExternalKind::Func
                        && e.index < self.imported_funcs
                        && EXPORTS.iter().any(|(n, ..)| *n == e.name)
                    {
                        return Err(load(format_args!(
                            "export {} re-exports an imported host function",
                            e.name
                        )));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Checks one function body: size, declared locals and its operator
    /// stream (count, control nesting, `br_table` targets).
    fn body(&mut self, body: &FunctionBody<'_>) -> Result<(), ScriptError> {
        let f = self.func;
        self.func += 1;
        let r = body.range();
        let len = r.end - r.start;
        if len > MAX_FUNCTION_BODY_BYTES {
            return Err(load(format_args!(
                "a function body is {len} bytes (at most {MAX_FUNCTION_BODY_BYTES})"
            )));
        }
        let mut locals_reader = body.get_locals_reader().map_err(parse_err)?;
        let mut locals = 0u64;
        for _ in 0..locals_reader.get_count() {
            locals += u64::from(locals_reader.read().map_err(parse_err)?.0);
            cap(
                format_args!("function {f}"),
                "locals",
                locals,
                MAX_LOCALS_PER_FUNCTION,
            )?;
        }
        self.locals += locals;
        cap(
            format_args!("module"),
            "locals",
            self.locals,
            MAX_TOTAL_LOCALS,
        )?;

        let reader = body.get_binary_reader_for_operators().map_err(parse_err)?;
        let mut ops = OperatorsReader::new_with_allocs(reader, std::mem::take(&mut self.allocs));
        let (mut n, mut depth, mut targets) = (0u64, 0u64, 0u64);
        while !ops.eof() {
            let op = ops.read().map_err(parse_err)?;
            n += 1;
            match op {
                Operator::Block { .. }
                | Operator::Loop { .. }
                | Operator::If { .. }
                | Operator::TryTable { .. }
                | Operator::Try { .. } => {
                    depth += 1;
                    cap(
                        format_args!("function {f}"),
                        "nested blocks",
                        depth,
                        MAX_NESTING,
                    )?;
                }
                Operator::End => depth = depth.saturating_sub(1),
                Operator::BrTable { targets: t } => {
                    let k = u64::from(t.len()) + 1;
                    cap(
                        format_args!("a br_table in function {f}"),
                        "targets",
                        k,
                        MAX_BR_TABLE_TARGETS,
                    )?;
                    targets += k;
                    cap(
                        format_args!("function {f}"),
                        "br_table targets",
                        targets,
                        MAX_BR_TABLE_TARGETS_PER_FUNCTION,
                    )?;
                }
                _ => {}
            }
            if n > MAX_OPERATORS_PER_FUNCTION {
                return cap(
                    format_args!("function {f}"),
                    "operators",
                    n,
                    MAX_OPERATORS_PER_FUNCTION,
                );
            }
        }
        self.allocs = ops.into_allocations();
        self.operators += n;
        cap(
            format_args!("module"),
            "operators",
            self.operators,
            MAX_OPERATORS,
        )
    }
}

/// A module compiled and validated by [`WasmScriptBackend::prepare`], ready
/// to be instantiated by [`WasmScriptBackend::apply_prepared`]. `Send`, so it
/// can travel from the I/O worker to the sim thread.
#[derive(Clone)]
pub struct PreparedModule {
    module: Module,
}

impl fmt::Debug for PreparedModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedModule").finish_non_exhaustive()
    }
}

/// One live instance and its pre-resolved entry points.
struct Live {
    store: Store<HostState>,
    memory: Memory,
    on_init: TypedFunc<(), ()>,
    on_tick: TypedFunc<(), ()>,
    on_switch: TypedFunc<(i32, i32), ()>,
    save_state: Option<TypedFunc<(), i64>>,
    restore_state: Option<TypedFunc<(i32, i32), ()>>,
    alloc_state: Option<TypedFunc<i32, i32>>,
}

/// Maps a guest-call error to the fault it causes.
fn classify(e: &wasmtime::Error) -> ScriptError {
    match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => ScriptError::BudgetExceeded,
        Some(t) => ScriptError::Runtime(t.to_string()),
        None => ScriptError::Runtime(e.root_cause().to_string()),
    }
}

impl Live {
    /// Instantiates `m` (running its start function under a fresh budget).
    fn new(
        m: &PreparedModule,
        names: &Arc<IdTables>,
        cfg: &WasmConfig,
    ) -> Result<Self, ScriptError> {
        check_resources(&m.module, cfg)?;
        let limits = StoreLimitsBuilder::new()
            .memory_size(cfg.memory_limit_bytes)
            .table_elements(MAX_TABLE_ELEMENTS)
            .tables(MAX_TABLES)
            .memories(1)
            .instances(1)
            .trap_on_grow_failure(false)
            .build();
        let mut store = Store::new(engine()?, HostState::new(Arc::clone(names), limits));
        store.limiter(|s| &mut s.limits);
        store.set_fuel(cfg.fuel_per_callback).map_err(load)?;
        let instance =
            linker()?
                .instantiate(&mut store, &m.module)
                .map_err(|e| match classify(&e) {
                    ScriptError::Runtime(msg) => ScriptError::Load(msg),
                    other => other,
                })?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| load("module exports no memory named \"memory\""))?;
        store.data_mut().memory = Some(memory);
        let mut req = |name: &str| {
            instance
                .get_func(&mut store, name)
                .ok_or_else(|| load(format_args!("missing export {name}")))
        };
        let (init, tick, switch) = (req("on_init")?, req("on_tick")?, req("on_switch_edge")?);
        let typed = |e: wasmtime::Error| load(format_args!("{e:#}"));
        let on_init = init.typed(&store).map_err(typed)?;
        let on_tick = tick.typed(&store).map_err(typed)?;
        let on_switch = switch.typed(&store).map_err(typed)?;
        let save_state = instance.get_typed_func(&mut store, "save_state").ok();
        let restore_state = instance.get_typed_func(&mut store, "restore_state").ok();
        let alloc_state = instance.get_typed_func(&mut store, "alloc_state").ok();
        Ok(Self {
            store,
            memory,
            on_init,
            on_tick,
            on_switch,
            save_state,
            restore_state,
            alloc_state,
        })
    }

    /// Calls the `save_state` hook and copies its blob out (see module docs).
    fn save(&mut self, fuel: u64) -> Option<Vec<u8>> {
        let f = self.save_state.clone()?;
        self.store.data_mut().leave();
        self.store.set_fuel(fuel).ok()?;
        let packed = f.call(&mut self.store, ()).ok()? as u64;
        let (ptr, len) = ((packed >> 32) as usize, (packed & 0xFFFF_FFFF) as usize);
        if len > MAX_STATE_BYTES {
            return None;
        }
        let data = self.memory.data(&self.store);
        data.get(ptr..ptr.checked_add(len)?).map(<[u8]>::to_vec)
    }

    /// Hands `blob` to the `restore_state` hook (see module docs).
    fn restore(&mut self, blob: &[u8], fuel: u64) -> Result<(), ScriptError> {
        let Some(restore) = self.restore_state.clone() else {
            return Ok(());
        };
        let fail = |e: wasmtime::Error| classify(&e);
        self.store.data_mut().leave();
        // `blob.len()` ≤ MAX_STATE_BYTES, so it fits an i32.
        let len = blob.len() as i32;
        let ptr = if blob.is_empty() {
            0
        } else {
            let Some(alloc) = self.alloc_state.clone() else {
                return Ok(());
            };
            self.store.set_fuel(fuel).map_err(load)?;
            let p = alloc.call(&mut self.store, len).map_err(fail)?;
            let start = p as u32 as usize;
            let data = self.memory.data_mut(&mut self.store);
            let dst = start
                .checked_add(blob.len())
                .and_then(|end| data.get_mut(start..end))
                .ok_or_else(|| {
                    ScriptError::Runtime(format!(
                        "alloc_state({len}) returned {start}, outside guest memory"
                    ))
                })?;
            dst.copy_from_slice(blob);
            p
        };
        self.store.set_fuel(fuel).map_err(load)?;
        restore.call(&mut self.store, (ptr, len)).map_err(fail)
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
    fn new(q: &HostCommandQueue, overflow: u32) -> Self {
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
            overflow,
        }
    }

    /// Drops what the callback queued. Every coil named by a dropped hold
    /// command is marked held, so the tilt release that follows emits
    /// `(coil, false)` for it (as in the Lua backend).
    fn rollback(&self, q: &mut HostCommandQueue, book: &mut host::Book) {
        let l = &self.lens;
        for &(coil, _) in &q.coil_holds[l[1].min(q.coil_holds.len())..] {
            book.set_held(coil, true);
        }
        book.overflow = self.overflow;
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

/// Which guest entry point to call.
#[derive(Copy, Clone)]
enum Entry {
    Init,
    Tick,
    Switch(i32, i32),
}

/// The WebAssembly [`TableScriptBackend`]. See the module docs.
pub struct WasmScriptBackend {
    live: Live,
    names: Arc<IdTables>,
    cfg: WasmConfig,
    faulted: bool,
    fault_count: u32,
    last_error: Option<ScriptError>,
}

impl fmt::Debug for WasmScriptBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WasmScriptBackend")
            .field("cfg", &self.cfg)
            .field("faulted", &self.faulted)
            .field("fault_count", &self.fault_count)
            .field("overflow_count", &self.overflow_count())
            .field("last_error", &self.last_error)
            .finish_non_exhaustive()
    }
}

impl WasmScriptBackend {
    /// Compiles, validates and instantiates `module` (binary Wasm or WAT
    /// text). The start function, if any, runs under a fresh fuel budget; no
    /// callback is called yet.
    ///
    /// Errors: `Load` for invalid bytes, a module above the size limits,
    /// unknown or mistyped imports,
    /// missing or mistyped exports, too many/too large tables and a trapping
    /// start function; `MemoryLimit` for a declared initial memory above the
    /// cap; `BudgetExceeded` when the start function runs out of fuel.
    pub fn new(module: &[u8], names: &NameTables, cfg: WasmConfig) -> Result<Self, ScriptError> {
        let prepared = Self::prepare(module, cfg)?;
        let names = Arc::new(IdTables::new(names));
        let live = Live::new(&prepared, &names, &cfg)?;
        Ok(Self {
            live,
            names,
            cfg,
            faulted: false,
            fault_count: 0,
            last_error: None,
        })
    }

    /// Checks the size limits, then compiles and validates `bytes`
    /// (imports, exports, declared sizes against `cfg`) without
    /// instantiating it. May run on any thread.
    pub fn prepare(bytes: &[u8], cfg: WasmConfig) -> Result<PreparedModule, ScriptError> {
        let engine = engine()?;
        let bin = precheck(bytes)?;
        let module = Module::from_binary(engine, &bin).map_err(|e| load(format_args!("{e:#}")))?;
        for imp in module.imports() {
            let ok = imp.module() == host::MODULE
                && host::IMPORTS
                    .iter()
                    .any(|(n, p, r)| *n == imp.name() && func_is(&imp.ty(), p, r));
            if !ok {
                return Err(load(format_args!(
                    "unsupported import {}::{} (only the pinball API is available)",
                    imp.module(),
                    imp.name()
                )));
            }
        }
        for (name, required, params, results) in EXPORTS {
            match module.get_export(name) {
                None if *required => return Err(load(format_args!("missing export {name}"))),
                None => {}
                Some(ty) if func_is(&ty, params, results) => {}
                Some(_) => return Err(load(format_args!("export {name} has the wrong type"))),
            }
        }
        match module.get_export("memory") {
            Some(ExternType::Memory(_)) => {}
            Some(_) => return Err(load("export memory is not a memory")),
            None => return Err(load("missing export memory")),
        }
        check_resources(&module, &cfg)?;
        Ok(PreparedModule { module })
    }

    /// Swaps in a prepared module at a tick boundary, carrying state through
    /// `save_state` / `restore_state` (see the module docs). On error the
    /// running instance is untouched.
    pub fn apply_prepared(&mut self, m: PreparedModule) -> Result<(), ScriptError> {
        let mut new = Live::new(&m, &self.names, &self.cfg)?;
        let fuel = self.cfg.fuel_per_callback;
        let state = if self.faulted {
            None
        } else {
            self.live.save(fuel)
        };
        if let Some(blob) = state {
            new.restore(&blob, fuel)?;
        }
        new.store.data_mut().book = self.live.store.data().book.clone();
        self.live = new;
        self.faulted = false;
        Ok(())
    }

    /// True while the script is in the tilt state.
    pub fn is_faulted(&self) -> bool {
        self.faulted
    }

    /// Number of faults since construction.
    pub fn fault_count(&self) -> u32 {
        self.fault_count
    }

    /// Commands dropped because their queue was full, since construction.
    pub fn overflow_count(&self) -> u32 {
        self.live.store.data().book.overflow
    }

    /// The error behind the most recent fault, if any.
    pub fn last_error(&self) -> Option<&ScriptError> {
        self.last_error.as_ref()
    }

    /// The configuration this backend runs with.
    pub fn config(&self) -> WasmConfig {
        self.cfg
    }

    /// Bytes of guest linear memory currently allocated.
    pub fn memory_size(&self) -> usize {
        self.live.memory.data_size(&self.live.store)
    }

    fn run(&mut self, entry: Entry, ctx: &mut HostCtx) {
        let live = &mut self.live;
        if self.faulted {
            let book = &mut live.store.data_mut().book;
            if book.any_held() {
                book.release_holds(ctx.cmd);
            }
            return;
        }
        let mark = QueueMark::new(ctx.cmd, live.store.data().book.overflow);
        let r = live
            .store
            .set_fuel(self.cfg.fuel_per_callback)
            .and_then(|()| {
                live.store.data_mut().enter(ctx);
                let r = match entry {
                    Entry::Init => live.on_init.call(&mut live.store, ()),
                    Entry::Tick => live.on_tick.call(&mut live.store, ()),
                    Entry::Switch(id, closed) => live.on_switch.call(&mut live.store, (id, closed)),
                };
                live.store.data_mut().leave();
                r
            });
        if let Err(e) = r {
            let book = &mut live.store.data_mut().book;
            mark.rollback(ctx.cmd, book);
            book.release_holds(ctx.cmd);
            self.faulted = true;
            self.fault_count = self.fault_count.saturating_add(1);
            self.last_error = Some(classify(&e));
        }
    }
}

impl TableScriptBackend for WasmScriptBackend {
    fn on_init(&mut self, ctx: &mut HostCtx) {
        self.run(Entry::Init, ctx);
    }

    fn on_tick(&mut self, ctx: &mut HostCtx) {
        self.run(Entry::Tick, ctx);
    }

    fn on_switch_edge(&mut self, switch: SwitchId, edge: SwitchEdge, ctx: &mut HostCtx) {
        let closed = i32::from(edge == SwitchEdge::Closed);
        self.run(Entry::Switch(i32::from(switch.0), closed), ctx);
    }

    /// `prepare` + `apply_prepared` inline (developer-only path; may
    /// allocate and overrun the tick, PLAN.md §2.3 rule 4).
    fn apply_reload(&mut self, source: &[u8]) -> Result<(), ScriptError> {
        let m = Self::prepare(source, self.cfg)?;
        self.apply_prepared(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::{HostView, Pcg32, SeedableRng};

    const BASE: &str = r#"(module
      (import "pinball" "hold_coil" (func $hold (param i32 i32)))
      (import "pinball" "pulse_coil" (func $pulse (param i32 i32)))
      (memory (export "memory") 1)
      (global $n (mut i32) (i32.const 0))
      (func (export "on_init"))
      (func (export "on_tick")
        (global.set $n (i32.add (global.get $n) (i32.const 1)))
        (call $hold (i32.const 3) (i32.const 1))
        (if (i32.eq (global.get $n) (i32.const 2)) (then unreachable)))
      (func (export "on_switch_edge") (param i32 i32)
        (call $pulse (local.get 0) (local.get 1))))"#;

    fn tick(b: &mut WasmScriptBackend) -> HostCommandQueue {
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
    fn configs() {
        let d = WasmConfig::default();
        assert_eq!(d.memory_limit_bytes, 16 << 20);
        assert_eq!(d.fuel_per_callback, 1_000_000);
        let e = WasmConfig::engine();
        assert_eq!(e.memory_limit_bytes, 2 << 20);
        assert_eq!(e.fuel_per_callback, 100_000);
    }

    #[test]
    fn fault_releases_holds_and_stops_calls() {
        let mut b = WasmScriptBackend::new(
            BASE.as_bytes(),
            &NameTables::default(),
            WasmConfig::default(),
        )
        .unwrap();
        assert_eq!(b.memory_size(), 65_536);
        assert!(format!("{b:?}").contains("WasmScriptBackend"));
        assert_eq!(
            tick(&mut b).coil_holds.as_slice(),
            &[(pin_core::CoilId(3), true)]
        );
        let q = tick(&mut b);
        assert!(b.is_faulted());
        assert_eq!(q.coil_holds.as_slice(), &[(pin_core::CoilId(3), false)]);
        assert!(
            matches!(b.last_error(), Some(ScriptError::Runtime(m)) if m.contains("unreachable"))
        );
        assert!(tick(&mut b).is_empty());
        assert_eq!(b.fault_count(), 1);
        b.apply_reload(BASE.as_bytes()).unwrap();
        assert!(!b.is_faulted());
        assert_eq!(tick(&mut b).coil_holds.len(), 1);
        assert!(
            format!(
                "{:?}",
                WasmScriptBackend::prepare(BASE.as_bytes(), b.config()).unwrap()
            )
            .contains("PreparedModule")
        );
    }

    #[test]
    fn classify_errors() {
        assert_eq!(
            classify(&Trap::OutOfFuel.into()),
            ScriptError::BudgetExceeded
        );
        assert!(
            matches!(classify(&wasmtime::Error::msg("x")), ScriptError::Runtime(m) if m == "x")
        );
    }
}
