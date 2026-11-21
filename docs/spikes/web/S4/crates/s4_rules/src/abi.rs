//! The rules-module ABI (`pinball` imports, required exports) and the
//! load-time checks `pin_script::wasm::WasmScriptBackend::prepare` makes
//! after its pre-scan, re-implemented over wasmparser so every web design
//! runs the same checks in the same order (imports, exports, `memory`,
//! declared resources).
//!
//! Source of truth: `crates/pin_script/src/wasm/{mod.rs,host.rs}` at
//! `995935f` (`IMPORTS`, `EXPORTS`, `check_resources`, `prepare`).

use pin_core::ScriptError;
use pin_script::wasm::WasmConfig;
use std::fmt;
use wasmparser::{ExternalKind, Parser, Payload, TypeRef, ValType};

/// Value types used by the ABI.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum V {
    /// `i32`.
    I32,
    /// `i64`.
    I64,
    /// `f32`.
    F32,
}

use V::{F32, I32, I64};

/// Import module name.
pub const MODULE: &str = "pinball";

/// The complete import ABI: `(name, params, results)` (pin_script host.rs).
pub const IMPORTS: &[(&str, &[V], &[V])] = &[
    ("pulse_coil", &[I32, I32], &[]),
    ("hold_coil", &[I32, I32], &[]),
    ("set_magnet", &[I32, F32], &[]),
    ("set_zipper", &[I32, I32], &[]),
    ("set_diverter", &[I32, I32], &[]),
    ("spawn_ball", &[I32, I32], &[]),
    ("set_lamp", &[I32, F32], &[]),
    ("set_gi_tint", &[F32, F32, F32, F32], &[]),
    ("set_score", &[I32, I64], &[]),
    ("show_scene", &[I32], &[]),
    ("play_sound", &[I32, F32], &[]),
    ("persist_set", &[I32, I64], &[]),
    ("persist_get", &[I32], &[I64]),
    ("switch", &[I32], &[I32]),
    ("tick", &[], &[I64]),
    ("balls_in_play", &[], &[I32]),
    ("random", &[I64, I64], &[I64]),
    ("id_of", &[I32, I32, I32], &[I32]),
];

/// Index of `id_of` in [`IMPORTS`].
pub const ID_OF: u8 = 17;

/// Exported functions: `(name, required, params, results)` (pin_script mod.rs).
pub const EXPORTS: &[(&str, bool, &[V], &[V])] = &[
    ("on_init", true, &[], &[]),
    ("on_tick", true, &[], &[]),
    ("on_switch_edge", true, &[V::I32, V::I32], &[]),
    ("save_state", false, &[], &[V::I64]),
    ("restore_state", false, &[V::I32, V::I32], &[]),
    ("alloc_state", false, &[V::I32], &[V::I32]),
];

/// Most tables a module may define.
pub const MAX_TABLES: usize = 4;
/// Most elements of one table (initial size and growth).
pub const MAX_TABLE_ELEMENTS: usize = 65_536;
/// Bytes per Wasm page.
pub const PAGE: u64 = 65_536;

/// `ScriptError::Load(msg)`.
pub fn load(msg: impl fmt::Display) -> ScriptError {
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

fn sig_is(ty: &wasmparser::FuncType, params: &[V], results: &[V]) -> bool {
    ty.params()
        .iter()
        .map(val)
        .eq(params.iter().copied().map(Some))
        && ty
            .results()
            .iter()
            .map(val)
            .eq(results.iter().copied().map(Some))
}

/// What the web backends need to know about an accepted module.
#[derive(Clone, Debug, Default)]
pub struct ModuleInfo {
    /// For every imported function (in import order), its index in [`IMPORTS`].
    pub imports: Vec<u8>,
    /// Declared initial pages of the (single) memory.
    pub mem_min: u64,
    /// Declared maximum pages, if any.
    pub mem_max: Option<u64>,
    /// Whether the module has a start function.
    pub has_start: bool,
    /// Indices of the optional exports present (`save_state`, `restore_state`, `alloc_state`).
    pub optional_exports: [bool; 3],
}

/// The checks `prepare` makes after the pre-scan, in its order: every
/// import is a `pinball` function of the ABI with its exact signature;
/// required exports exist with their exact types (optional ones, when
/// present, too); `memory` is an exported memory; declared initial memory
/// within the cap (`MemoryLimit`), at most [`MAX_TABLES`] tables, none
/// starting above [`MAX_TABLE_ELEMENTS`]. `bin` has passed
/// [`crate::prescan::precheck`] (so it is valid under the engine features).
pub fn check_module(bin: &[u8], cfg: &WasmConfig) -> Result<ModuleInfo, ScriptError> {
    let mut types: Vec<wasmparser::FuncType> = Vec::new();
    let mut func_types: Vec<u32> = Vec::new(); // imported then defined
    let mut imports: Vec<(String, String, TypeRef)> = Vec::new();
    let mut exports: Vec<(String, ExternalKind, u32)> = Vec::new();
    let mut memories: Vec<wasmparser::MemoryType> = Vec::new();
    let mut tables: Vec<wasmparser::TableType> = Vec::new();
    let mut has_start = false;
    for payload in Parser::new(0).parse_all(bin) {
        match payload.map_err(|e| load(e))? {
            Payload::TypeSection(r) => {
                for t in r.into_iter_err_on_gc_types() {
                    types.push(t.map_err(|e| load(e))?);
                }
            }
            Payload::ImportSection(r) => {
                for imp in r.into_imports() {
                    let imp = imp.map_err(|e| load(e))?;
                    if let TypeRef::Func(t) | TypeRef::FuncExact(t) = imp.ty {
                        func_types.push(t);
                    }
                    if let TypeRef::Memory(m) = imp.ty {
                        memories.push(m);
                    }
                    imports.push((imp.module.to_owned(), imp.name.to_owned(), imp.ty));
                }
            }
            Payload::FunctionSection(r) => {
                for t in r {
                    func_types.push(t.map_err(|e| load(e))?);
                }
            }
            Payload::MemorySection(r) => {
                for m in r {
                    memories.push(m.map_err(|e| load(e))?);
                }
            }
            Payload::TableSection(r) => {
                for t in r {
                    tables.push(t.map_err(|e| load(e))?.ty);
                }
            }
            Payload::ExportSection(r) => {
                for e in r {
                    let e = e.map_err(|e| load(e))?;
                    exports.push((e.name.to_owned(), e.kind, e.index));
                }
            }
            Payload::StartSection { .. } => has_start = true,
            _ => {}
        }
    }
    let fty = |func: u32| -> Option<&wasmparser::FuncType> {
        types.get(*func_types.get(func as usize)? as usize)
    };
    // Imports (wasmtime's `module.imports()` order).
    let mut info = ModuleInfo {
        has_start,
        ..ModuleInfo::default()
    };
    for (m, n, ty) in &imports {
        let found = match ty {
            TypeRef::Func(t) | TypeRef::FuncExact(t) if m == MODULE => {
                types.get(*t as usize).and_then(|ft| {
                    IMPORTS
                        .iter()
                        .position(|(name, p, r)| *name == n.as_str() && sig_is(ft, p, r))
                })
            }
            _ => None,
        };
        match found {
            Some(i) => info.imports.push(i as u8),
            None => {
                return Err(load(format_args!(
                    "unsupported import {m}::{n} (only the pinball API is available)"
                )));
            }
        }
    }
    let export = |name: &str| exports.iter().find(|(n, ..)| n == name);
    for (k, (name, required, params, results)) in EXPORTS.iter().enumerate() {
        match export(name) {
            None if *required => return Err(load(format_args!("missing export {name}"))),
            None => {}
            Some((_, ExternalKind::Func, idx))
                if fty(*idx).is_some_and(|t| sig_is(t, params, results)) =>
            {
                if k >= 3 {
                    info.optional_exports[k - 3] = true;
                }
            }
            Some(_) => return Err(load(format_args!("export {name} has the wrong type"))),
        }
    }
    match export("memory") {
        Some((_, ExternalKind::Memory, _)) => {}
        Some(_) => return Err(load("export memory is not a memory")),
        None => return Err(load("missing export memory")),
    }
    // check_resources.
    let max_initial = memories.iter().map(|m| m.initial).max();
    if let Some(pages) = max_initial
        && pages.saturating_mul(PAGE) > cfg.memory_limit_bytes as u64
    {
        return Err(ScriptError::MemoryLimit);
    }
    if tables.len() > MAX_TABLES {
        return Err(load(format_args!(
            "module defines {} tables (at most {MAX_TABLES})",
            tables.len()
        )));
    }
    if tables.iter().map(|t| t.initial).max().unwrap_or(0) > MAX_TABLE_ELEMENTS as u64 {
        return Err(load(format_args!(
            "a table starts with more than {MAX_TABLE_ELEMENTS} elements"
        )));
    }
    if let Some(m) = memories.first() {
        info.mem_min = m.initial;
        info.mem_max = m.maximum;
    }
    Ok(info)
}

/// The full load-time check: pre-scan (limits + validation, WAT accepted)
/// then [`check_module`]. Returns the binary.
pub fn prepare(bytes: &[u8], cfg: &WasmConfig) -> Result<(Vec<u8>, ModuleInfo), ScriptError> {
    let (bin, _usage) = crate::prescan::precheck(bytes)?;
    let info = check_module(&bin, cfg)?;
    Ok((bin.into_owned(), info))
}
