//! Load-time rewriting of a (pre-scanned, checked) rules module so that any
//! engine can enforce the native sandbox:
//!
//! * **Fuel, wasmtime-exact.** A mutable `i64` global holds the fuel left.
//!   The rewriter replays wasmtime 50.0.1's Cranelift fuel accounting
//!   (`wasmtime-internal-cranelift-50.0.1/src/func_environ.rs`
//!   `fuel_before_op`, `fuel_check`, `pre/post_translate_bulk_op`) over the
//!   original operators: the same per-operator costs
//!   (`wasmtime-environ` `default_operator_cost`: 0 for nop, drop, block,
//!   loop, unreachable, return, else, end; 1 otherwise), the same flush
//!   points (call, call_indirect, return, unreachable, loop, if, br*, end,
//!   else), the same check points (function entry, loop headers, after a
//!   deferred bulk operation) and the same check (trap when the fuel left
//!   is `<= 0`), the same size-proportional bulk charges (1 per byte,
//!   element or page; constants up to 128 charged up front, larger or
//!   run-time sizes charged only after the operation succeeded, then
//!   checked), and the same notion of reachability (Cranelift skips
//!   unreachable code). Whether a size operand is a compile-time constant
//!   is decided like Cranelift's SSA builder would (constants flow through
//!   locals; loop-header parameters are unknown inside the loop body).
//! * **Call depth.** Optional weighted depth counter: every function adds
//!   its weight on entry and subtracts it on every exit; above the limit it
//!   traps. Deterministic on every engine (the browser's own stack limit
//!   is not).
//! * **Memory and table caps.** The memory maximum becomes
//!   `min(declared, cap)`, table maxima `min(declared, 65 536)`; the spec
//!   makes `grow` past a maximum return −1 (wasmtime's `StoreLimits` with
//!   `trap_on_grow_failure(false)` behaves the same).
//! * **Host errors (JIT mode).** Imports are wrapped: after the host call a
//!   second import (`khz.host_err`) reports whether the host rejected the
//!   call, and the wrapper traps; every reference to an import is redirected
//!   to its wrapper.
//!
//! A trap raised by the instrumentation sets the `why` global first:
//! 1 = out of fuel, 2 = call depth, 3 = host error.

use crate::prescan::FEATURES;
use std::collections::HashMap;
use wasm_encoder::reencode::{Reencode, RoundtripReencoder};
use wasm_encoder::{
    BlockType as EBlockType, CodeSection, ConstExpr, DataCountSection, DataSection, ElementSection,
    EntityType, ExportKind, ExportSection, Function, FunctionSection, GlobalSection, GlobalType,
    ImportSection, Instruction as I, MemorySection, MemoryType, Module, TableSection, TypeSection,
    ValType as EVal,
};
use wasmparser::{
    BlockType, FuncToValidate, FunctionBody, Operator, Parser, Payload, ValidPayload, Validator,
    ValidatorResources,
};

/// Why the instrumentation trapped (the `why` global).
pub const WHY_FUEL: i32 = 1;
/// Call-depth limit.
pub const WHY_DEPTH: i32 = 2;
/// The host rejected a call (JIT mode).
pub const WHY_HOST: i32 = 3;

/// How the call-depth counter weighs a function.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DepthWeight {
    /// One unit per active call.
    Calls,
    /// An estimate of the function's frame in bytes:
    /// `16 * (params + locals + max operand stack) + 64`.
    FrameBytes,
}

/// What to insert.
#[derive(Clone, Debug)]
pub struct Config {
    /// wasmtime-exact fuel accounting.
    pub fuel: bool,
    /// The fuel global's initial value: the budget the start function runs
    /// with (wasmtime's start-up code charges 1 for its `call` of the start
    /// function on top of the segment costs pin_script adds back).
    pub start_fuel: i64,
    /// Weighted call-depth limit.
    pub depth_limit: Option<(u32, DepthWeight)>,
    /// Memory maximum, pages.
    pub mem_cap_pages: u64,
    /// Wrap imports and check `khz.host_err` after each host call.
    pub host_err_import: bool,
    /// Keep a `max_depth` global (profiling).
    pub profile: bool,
    /// Canonicalise NaN results of float arithmetic (wasmtime's
    /// `cranelift_nan_canonicalization`: the same operators, the same
    /// canonical bits 0x7fc00000 / 0x7ff8000000000000).
    pub nan_canon: bool,
}

impl Config {
    /// Fuel and caps as the native engine config, no depth counter.
    #[must_use]
    pub fn engine(fuel_per_callback: u64, memory_limit_bytes: usize) -> Self {
        Self {
            fuel: true,
            start_fuel: fuel_per_callback as i64 - 1,
            depth_limit: None,
            mem_cap_pages: memory_limit_bytes as u64 / 65_536,
            host_err_import: false,
            profile: false,
            nan_canon: false,
        }
    }
}

/// Export names the host uses (chosen not to collide with the module's).
#[derive(Clone, Debug, Default)]
pub struct Names {
    /// `i64` fuel-left global.
    pub fuel: String,
    /// `i32` why global.
    pub why: String,
    /// `i32` depth global.
    pub depth: String,
    /// `i32` max-depth global (profile).
    pub max_depth: String,
    /// `begin(fuel: f64)`: sets fuel, clears why and depth.
    pub begin: String,
    /// `fuel() -> f64`.
    pub get_fuel: String,
    /// `set_fuel(f64)`.
    pub set_fuel: String,
    /// `why() -> i32`.
    pub get_why: String,
    /// The module's start function, exported instead of run at
    /// instantiation (the host calls it right after instantiating, with
    /// the fuel and why globals readable).
    pub start: Option<String>,
}

/// The rewritten module.
#[derive(Clone, Debug)]
pub struct Instrumented {
    /// Module bytes.
    pub bytes: Vec<u8>,
    /// Export names of the control globals and helpers.
    pub names: Names,
    /// Fuel checks inserted (function entries + loop headers + bulk).
    pub checks: u32,
    /// Fuel flushes inserted.
    pub flushes: u32,
}

/// A value's identity for Cranelift-style constant detection: two tags
/// are the same SSA value iff their ids are equal.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct Tag {
    id: u32,
    konst: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    Func,
    Block,
    Loop,
    If,
}

#[derive(Clone, Debug)]
struct Frame {
    kind: Kind,
    /// Operand-stack height below the frame's params.
    base: usize,
    params: usize,
    results: usize,
    head_reachable: bool,
    consequent_ends_reachable: Option<bool>,
    exit_branched: bool,
    /// Locals arriving at the frame's exit (branches and fall-through).
    incoming: Vec<Vec<Tag>>,
    /// If: locals and params at the head.
    head_locals: Vec<Tag>,
    head_params: Vec<Tag>,
    /// Loop: locals before, header params, back edges.
    pre: Vec<Tag>,
    loop_params: Vec<Tag>,
    back: Vec<Vec<Tag>>,
}

impl Frame {
    fn new(kind: Kind, base: usize, params: usize, results: usize, reachable: bool) -> Self {
        Self {
            kind,
            base,
            params,
            results,
            head_reachable: reachable,
            consequent_ends_reachable: None,
            exit_branched: false,
            incoming: Vec::new(),
            head_locals: Vec::new(),
            head_params: Vec::new(),
            pre: Vec::new(),
            loop_params: Vec::new(),
            back: Vec::new(),
        }
    }
}

/// Function-index remapping (an extra import shifts defined functions;
/// references to imports go to their wrappers).
struct Remap {
    n_imp: u32,
    extra: u32,
    wrap_base: Option<u32>,
}

impl Reencode for Remap {
    type Error = std::convert::Infallible;
    fn function_index(
        &mut self,
        func: u32,
    ) -> Result<u32, wasm_encoder::reencode::Error<Self::Error>> {
        Ok(if func < self.n_imp {
            match self.wrap_base {
                Some(b) => b + func,
                None => func,
            }
        } else {
            func + self.extra
        })
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn eval(v: wasmparser::ValType) -> EVal {
    RoundtripReencoder.val_type(v).expect("core value type")
}

/// The globals the rewriter adds (indices).
#[derive(Copy, Clone)]
struct G {
    fuel: u32,
    why: u32,
    depth: u32,
    max_depth: u32,
}

/// Rewrites `bin` (a module that passed `abi::prepare`). See the module docs.
///
/// # Errors
/// Parse or encode failures (never for a module that passed the pre-scan).
pub fn instrument(bin: &[u8], cfg: &Config) -> Result<Instrumented, String> {
    let mut validator = Validator::new_with_features(FEATURES);
    let mut types: Vec<wasmparser::FuncType> = Vec::new();
    let mut imports: Vec<(String, String, u32)> = Vec::new();
    let mut func_types: Vec<u32> = Vec::new();
    let mut defined_types: Vec<u32> = Vec::new();
    let mut tables: Vec<wasmparser::Table> = Vec::new();
    let mut memories: Vec<wasmparser::MemoryType> = Vec::new();
    let mut globals = None;
    let mut exports: Vec<(String, wasmparser::ExternalKind, u32)> = Vec::new();
    let mut start = None;
    let mut elems = None;
    let mut datas = None;
    let mut data_count = None;
    let mut bodies: Vec<(FuncToValidate<ValidatorResources>, FunctionBody<'_>)> = Vec::new();
    let mut n_globals = 0u32;
    for payload in Parser::new(0).parse_all(bin) {
        let payload = payload.map_err(err)?;
        if let ValidPayload::Func(f, b) = validator.payload(&payload).map_err(err)? {
            bodies.push((f, b));
        }
        match payload {
            Payload::TypeSection(r) => {
                for t in r.into_iter_err_on_gc_types() {
                    types.push(t.map_err(err)?);
                }
            }
            Payload::ImportSection(r) => {
                for imp in r.into_imports() {
                    let imp = imp.map_err(err)?;
                    match imp.ty {
                        wasmparser::TypeRef::Func(t) | wasmparser::TypeRef::FuncExact(t) => {
                            imports.push((imp.module.to_owned(), imp.name.to_owned(), t));
                            func_types.push(t);
                        }
                        _ => return Err("non-function import".into()),
                    }
                }
            }
            Payload::FunctionSection(r) => {
                for t in r {
                    let t = t.map_err(err)?;
                    func_types.push(t);
                    defined_types.push(t);
                }
            }
            Payload::TableSection(r) => {
                for t in r {
                    tables.push(t.map_err(err)?);
                }
            }
            Payload::MemorySection(r) => {
                for m in r {
                    memories.push(m.map_err(err)?);
                }
            }
            Payload::GlobalSection(r) => {
                n_globals = r.count();
                globals = Some(r);
            }
            Payload::ExportSection(r) => {
                for e in r {
                    let e = e.map_err(err)?;
                    exports.push((e.name.to_owned(), e.kind, e.index));
                }
            }
            Payload::StartSection { func, .. } => start = Some(func),
            Payload::ElementSection(r) => elems = Some(r),
            Payload::DataSection(r) => datas = Some(r),
            Payload::DataCountSection { count, .. } => data_count = Some(count),
            _ => {}
        }
    }
    let n_imp = imports.len() as u32;
    let n_def = defined_types.len() as u32;
    let extra = u32::from(cfg.host_err_import);
    let wrap_base = cfg.host_err_import.then_some(n_imp + extra + n_def);
    let helper_base = n_imp + extra + n_def + if cfg.host_err_import { n_imp } else { 0 };
    let mut remap = Remap {
        n_imp,
        extra,
        wrap_base,
    };

    // Types: originals, then helper and block types.
    let mut ty = TypeSection::new();
    for t in &types {
        ty.ty().function(
            t.params().iter().map(|v| eval(*v)),
            t.results().iter().map(|v| eval(*v)),
        );
    }
    let mut next_ty = types.len() as u32;
    let mut add_ty = |ty: &mut TypeSection, p: &[EVal], r: &[EVal]| {
        ty.ty().function(p.iter().copied(), r.iter().copied());
        next_ty += 1;
        next_ty - 1
    };
    let t_ret_i32 = add_ty(&mut ty, &[], &[EVal::I32]);
    let t_ret_f64 = add_ty(&mut ty, &[], &[EVal::F64]);
    let t_f64 = add_ty(&mut ty, &[EVal::F64], &[]);
    // Result-only block types for multi-result functions.
    let mut result_types: HashMap<Vec<EVal>, u32> = HashMap::new();
    for &t in &defined_types {
        let res: Vec<EVal> = types[t as usize]
            .results()
            .iter()
            .map(|v| eval(*v))
            .collect();
        if res.len() > 1 && !result_types.contains_key(&res) {
            let id = add_ty(&mut ty, &[], &res);
            result_types.insert(res, id);
        }
    }

    // Imports.
    let mut imp = ImportSection::new();
    for (m, n, t) in &imports {
        imp.import(m, n, EntityType::Function(*t));
    }
    if cfg.host_err_import {
        imp.import("khz", "host_err", EntityType::Function(t_ret_i32));
    }

    // Functions: originals, wrappers, helpers (begin, fuel, set_fuel, why).
    let mut funcs = FunctionSection::new();
    for &t in &defined_types {
        funcs.function(t);
    }
    if cfg.host_err_import {
        for (_, _, t) in &imports {
            funcs.function(*t);
        }
    }
    funcs.function(t_f64); // begin
    funcs.function(t_ret_f64); // fuel
    funcs.function(t_f64); // set_fuel
    funcs.function(t_ret_i32); // why

    // Tables (maxima capped at 65 536 elements).
    let mut tab = TableSection::new();
    for t in &tables {
        let mut tt = RoundtripReencoder.table_type(t.ty).map_err(err)?;
        let cap = crate::abi::MAX_TABLE_ELEMENTS as u64;
        tt.maximum = Some(tt.maximum.map_or(cap, |m| m.min(cap)));
        match &t.init {
            wasmparser::TableInit::RefNull => {
                tab.table(tt);
            }
            wasmparser::TableInit::Expr(e) => {
                let e = remap.const_expr(e.clone()).map_err(err)?;
                tab.table_with_init(tt, &e);
            }
        }
    }
    // Memory (maximum capped).
    let mut mem = MemorySection::new();
    for m in &memories {
        let max = m
            .maximum
            .map_or(cfg.mem_cap_pages, |x| x.min(cfg.mem_cap_pages));
        mem.memory(MemoryType {
            minimum: m.initial,
            maximum: Some(max.max(m.initial)),
            memory64: m.memory64,
            shared: m.shared,
            page_size_log2: m.page_size_log2,
        });
    }
    // Globals: originals then ours.
    let mut glob = GlobalSection::new();
    if let Some(r) = globals {
        remap.parse_global_section(&mut glob, r).map_err(err)?;
    }
    let g = G {
        fuel: n_globals,
        why: n_globals + 1,
        depth: n_globals + 2,
        max_depth: n_globals + 3,
    };
    let mutable = |t| GlobalType {
        val_type: t,
        mutable: true,
        shared: false,
    };
    glob.global(mutable(EVal::I64), &ConstExpr::i64_const(cfg.start_fuel));
    glob.global(mutable(EVal::I32), &ConstExpr::i32_const(0));
    glob.global(mutable(EVal::I32), &ConstExpr::i32_const(0));
    glob.global(mutable(EVal::I32), &ConstExpr::i32_const(0));

    // Exports: originals (remapped) then ours under unused names.
    let mut exp = ExportSection::new();
    for (n, k, i) in &exports {
        let (kind, idx) = match k {
            wasmparser::ExternalKind::Func | wasmparser::ExternalKind::FuncExact => {
                (ExportKind::Func, remap.function_index(*i).map_err(err)?)
            }
            wasmparser::ExternalKind::Table => (ExportKind::Table, *i),
            wasmparser::ExternalKind::Memory => (ExportKind::Memory, *i),
            wasmparser::ExternalKind::Global => (ExportKind::Global, *i),
            wasmparser::ExternalKind::Tag => (ExportKind::Tag, *i),
        };
        exp.export(n, kind, idx);
    }
    let taken: std::collections::HashSet<&str> = exports.iter().map(|(n, ..)| n.as_str()).collect();
    let pick = |base: &str| {
        let mut name = base.to_owned();
        let mut k = 0;
        while taken.contains(name.as_str()) {
            k += 1;
            name = format!("{base}_{k}");
        }
        name
    };
    let names = Names {
        fuel: pick("__khz_fuel"),
        why: pick("__khz_why"),
        depth: pick("__khz_depth"),
        max_depth: pick("__khz_max_depth"),
        begin: pick("__khz_begin"),
        get_fuel: pick("__khz_get_fuel"),
        set_fuel: pick("__khz_set_fuel"),
        get_why: pick("__khz_get_why"),
        start: start.map(|_| pick("__khz_start")),
    };
    exp.export(&names.fuel, ExportKind::Global, g.fuel);
    exp.export(&names.why, ExportKind::Global, g.why);
    exp.export(&names.depth, ExportKind::Global, g.depth);
    exp.export(&names.max_depth, ExportKind::Global, g.max_depth);
    exp.export(&names.begin, ExportKind::Func, helper_base);
    exp.export(&names.get_fuel, ExportKind::Func, helper_base + 1);
    exp.export(&names.set_fuel, ExportKind::Func, helper_base + 2);
    exp.export(&names.get_why, ExportKind::Func, helper_base + 3);
    if let (Some(s), Some(n)) = (start, &names.start) {
        exp.export(n, ExportKind::Func, remap.function_index(s).map_err(err)?);
    }

    let mut elem = ElementSection::new();
    if let Some(r) = elems {
        remap.parse_element_section(&mut elem, r).map_err(err)?;
    }
    let mut data = DataSection::new();
    if let Some(r) = datas {
        remap.parse_data_section(&mut data, r).map_err(err)?;
    }

    // Code.
    let mut code = CodeSection::new();
    let mut stats = (0u32, 0u32);
    for (i, (f, body)) in bodies.into_iter().enumerate() {
        let t = &types[defined_types[i] as usize];
        let func = rewrite_body(
            f,
            &body,
            t,
            &types,
            cfg,
            g,
            &mut remap,
            &result_types,
            &mut stats,
        )?;
        code.function(&func);
    }
    if cfg.host_err_import {
        for (k, (_, _, t)) in imports.iter().enumerate() {
            let ft = &types[*t as usize];
            let mut f = Function::new([]);
            for p in 0..ft.params().len() as u32 {
                f.instruction(&I::LocalGet(p));
            }
            f.instruction(&I::Call(k as u32));
            f.instruction(&I::Call(n_imp));
            f.instruction(&I::If(EBlockType::Empty));
            f.instruction(&I::I32Const(WHY_HOST));
            f.instruction(&I::GlobalSet(g.why));
            f.instruction(&I::Unreachable);
            f.instruction(&I::End);
            f.instruction(&I::End);
            code.function(&f);
        }
    }
    // begin(fuel)
    let mut f = Function::new([]);
    f.instruction(&I::LocalGet(0));
    f.instruction(&I::I64TruncSatF64S);
    f.instruction(&I::GlobalSet(g.fuel));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(g.why));
    f.instruction(&I::I32Const(0));
    f.instruction(&I::GlobalSet(g.depth));
    f.instruction(&I::End);
    code.function(&f);
    let mut f = Function::new([]);
    f.instruction(&I::GlobalGet(g.fuel));
    f.instruction(&I::F64ConvertI64S);
    f.instruction(&I::End);
    code.function(&f);
    let mut f = Function::new([]);
    f.instruction(&I::LocalGet(0));
    f.instruction(&I::I64TruncSatF64S);
    f.instruction(&I::GlobalSet(g.fuel));
    f.instruction(&I::End);
    code.function(&f);
    let mut f = Function::new([]);
    f.instruction(&I::GlobalGet(g.why));
    f.instruction(&I::End);
    code.function(&f);

    let mut module = Module::new();
    module.section(&ty);
    module.section(&imp);
    module.section(&funcs);
    if !tables.is_empty() {
        module.section(&tab);
    }
    if !memories.is_empty() {
        module.section(&mem);
    }
    module.section(&glob);
    module.section(&exp);
    if !elem.is_empty() {
        module.section(&elem);
    }
    if let Some(c) = data_count {
        module.section(&DataCountSection { count: c });
    }
    module.section(&code);
    if !data.is_empty() {
        module.section(&data);
    }
    Ok(Instrumented {
        bytes: module.finish(),
        names,
        checks: stats.0,
        flushes: stats.1,
    })
}

/// Per-function rewriting state.
struct Body<'c> {
    cfg: &'c Config,
    g: G,
    out: Function,
    pending: i64,
    reachable: bool,
    locals: Vec<Tag>,
    stack: Vec<Tag>,
    frames: Vec<Frame>,
    next_id: u32,
    alias: HashMap<u32, Tag>,
    t_len: u32,
    t_res: u32,
    t_f32: u32,
    t_f64: u32,
    checks: u32,
    flushes: u32,
}

impl Body<'_> {
    fn fresh(&mut self, konst: Option<u64>) -> Tag {
        self.next_id += 1;
        Tag {
            id: self.next_id,
            konst,
        }
    }

    fn resolve(&self, mut t: Tag) -> Tag {
        while let Some(a) = self.alias.get(&t.id) {
            t = *a;
        }
        t
    }

    fn emit(&mut self, i: &I<'_>) {
        self.out.instruction(i);
    }

    /// `fuel -= pending` (wasmtime `fuel_increment_var`).
    fn flush(&mut self) {
        if !self.cfg.fuel {
            self.pending = 0;
            return;
        }
        if self.pending != 0 {
            let p = self.pending;
            self.emit(&I::GlobalGet(self.g.fuel));
            self.emit(&I::I64Const(p));
            self.emit(&I::I64Sub);
            self.emit(&I::GlobalSet(self.g.fuel));
            self.flushes += 1;
        }
        self.pending = 0;
    }

    /// wasmtime `fuel_check`: flush, then trap when the fuel left is `<= 0`.
    fn check(&mut self) {
        self.flush();
        if !self.cfg.fuel {
            return;
        }
        self.emit(&I::GlobalGet(self.g.fuel));
        self.emit(&I::I64Const(0));
        self.emit(&I::I64LeS);
        self.emit(&I::If(EBlockType::Empty));
        self.emit(&I::I32Const(WHY_FUEL));
        self.emit(&I::GlobalSet(self.g.why));
        self.emit(&I::Unreachable);
        self.emit(&I::End);
        self.checks += 1;
    }

    /// `fuel -= local * 1` (a run-time size), no flush.
    fn charge_local(&mut self, local: u32) {
        if !self.cfg.fuel {
            return;
        }
        self.emit(&I::GlobalGet(self.g.fuel));
        self.emit(&I::LocalGet(local));
        self.emit(&I::I64ExtendI32U);
        self.emit(&I::I64Sub);
        self.emit(&I::GlobalSet(self.g.fuel));
    }

    fn locals_snapshot(&self) -> Vec<Tag> {
        self.locals.iter().map(|t| self.resolve(*t)).collect()
    }

    /// Cranelift's lookup at a sealed block with these predecessors: the
    /// same value from every predecessor, or a (non-constant) parameter.
    fn merge(&mut self, inc: &[Vec<Tag>]) -> Vec<Tag> {
        let n = self.locals.len();
        (0..n)
            .map(|i| {
                let first = self.resolve(inc[0][i]);
                if inc.iter().all(|v| self.resolve(v[i]) == first) {
                    first
                } else {
                    self.fresh(None)
                }
            })
            .collect()
    }

    fn record_branch(&mut self, depth: u32) {
        let snap = self.locals_snapshot();
        let idx = self.frames.len() - 1 - depth as usize;
        let f = &mut self.frames[idx];
        match f.kind {
            Kind::Loop => f.back.push(snap),
            Kind::Func => {}
            _ => {
                f.exit_branched = true;
                f.incoming.push(snap);
            }
        }
    }
}

fn block_arity(types: &[wasmparser::FuncType], b: BlockType) -> (usize, usize) {
    match b {
        BlockType::Empty => (0, 0),
        BlockType::Type(_) => (0, 1),
        BlockType::FuncType(t) => {
            let t = &types[t as usize];
            (t.params().len(), t.results().len())
        }
    }
}

/// Size operand of a bulk operation, if any (wasmtime charges 1 per unit).
fn bulk(op: &Operator<'_>) -> Option<Bulk> {
    Some(match op {
        Operator::MemoryFill { .. }
        | Operator::MemoryCopy { .. }
        | Operator::MemoryInit { .. }
        | Operator::TableFill { .. }
        | Operator::TableCopy { .. }
        | Operator::TableInit { .. } => Bulk::Plain,
        Operator::MemoryGrow { .. } => Bulk::MemoryGrow,
        Operator::TableGrow { .. } => Bulk::TableGrow,
        _ => return None,
    })
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum Bulk {
    Plain,
    MemoryGrow,
    TableGrow,
}

/// wasmtime's `SMALL_BULK_OP_COST`.
const SMALL_BULK_OP_COST: u64 = 128;

#[allow(clippy::too_many_arguments)]
fn rewrite_body(
    f: FuncToValidate<ValidatorResources>,
    body: &FunctionBody<'_>,
    fty: &wasmparser::FuncType,
    types: &[wasmparser::FuncType],
    cfg: &Config,
    g: G,
    remap: &mut Remap,
    result_types: &HashMap<Vec<EVal>, u32>,
    stats: &mut (u32, u32),
) -> Result<Function, String> {
    let mut fv = f.into_validator(Default::default());
    let mut locals_decl: Vec<(u32, EVal)> = Vec::new();
    let mut n_locals = fty.params().len() as u32;
    for l in body.get_locals_reader().map_err(err)? {
        let (n, t) = l.map_err(err)?;
        locals_decl.push((n, eval(t)));
        n_locals += n;
    }
    let t_len = n_locals;
    let t_res = n_locals + 1;
    locals_decl.push((2, EVal::I32));
    let t_f32 = n_locals + 2;
    let t_f64 = n_locals + 3;
    locals_decl.push((1, EVal::F32));
    locals_decl.push((1, EVal::F64));
    fv.read_locals(&mut body.get_binary_reader()).map_err(err)?;

    let mut b = Body {
        cfg,
        g,
        out: Function::new(locals_decl),
        pending: 1,
        reachable: true,
        locals: Vec::new(),
        stack: Vec::new(),
        frames: Vec::new(),
        next_id: 0,
        alias: HashMap::new(),
        t_len,
        t_res,
        t_f32,
        t_f64,
        checks: 0,
        flushes: 0,
    };
    // Params are block parameters of the entry block; declared locals start
    // as one `iconst 0` each (Cranelift `declare_locals`).
    for _ in 0..fty.params().len() {
        let t = b.fresh(None);
        b.locals.push(t);
    }
    for _ in fty.params().len() as u32..n_locals {
        let t = b.fresh(Some(0));
        b.locals.push(t);
    }
    let results: Vec<EVal> = fty.results().iter().map(|v| eval(*v)).collect();
    b.frames
        .push(Frame::new(Kind::Func, 0, 0, results.len(), true));

    // Prologue: depth (Cranelift's stack check runs first), then fuel.
    let weight = match cfg.depth_limit {
        Some((_, DepthWeight::Calls)) => 1,
        Some((_, DepthWeight::FrameBytes)) => {
            // Max operand stack is unknown before the scan; use the
            // validator's bound after a dry run.
            let mut probe = body.get_operators_reader().map_err(err)?;
            let mut fv2 = fv.clone();
            let mut max_h = 0u32;
            while !probe.eof() {
                let off = probe.original_position();
                let op = probe.read().map_err(err)?;
                fv2.op(off, &op).map_err(err)?;
                max_h = max_h.max(fv2.operand_stack_height());
            }
            16 * (n_locals + max_h) + 64
        }
        None => 0,
    };
    if let Some((limit, _)) = cfg.depth_limit {
        b.emit(&I::GlobalGet(g.depth));
        b.emit(&I::I32Const(weight as i32));
        b.emit(&I::I32Add);
        b.emit(&I::GlobalSet(g.depth));
        b.emit(&I::GlobalGet(g.depth));
        b.emit(&I::I32Const(limit as i32));
        b.emit(&I::I32GtU);
        b.emit(&I::If(EBlockType::Empty));
        b.emit(&I::I32Const(WHY_DEPTH));
        b.emit(&I::GlobalSet(g.why));
        b.emit(&I::Unreachable);
        b.emit(&I::End);
        if cfg.profile {
            b.emit(&I::GlobalGet(g.depth));
            b.emit(&I::GlobalGet(g.max_depth));
            b.emit(&I::I32GtU);
            b.emit(&I::If(EBlockType::Empty));
            b.emit(&I::GlobalGet(g.depth));
            b.emit(&I::GlobalSet(g.max_depth));
            b.emit(&I::End);
        }
    }
    b.check(); // function entry: pending = 1
    // Wrapper block so every exit passes the depth epilogue.
    let wrap = cfg.depth_limit.is_some();
    if wrap {
        let bt = match results.len() {
            0 => EBlockType::Empty,
            1 => EBlockType::Result(results[0]),
            _ => EBlockType::FunctionType(result_types[&results]),
        };
        b.emit(&I::Block(bt));
    }

    let mut reader = body.get_operators_reader().map_err(err)?;
    while !reader.eof() {
        let offset = reader.original_position();
        let op = reader.read().map_err(err)?;
        let arity = op.operator_arity(&fv);
        fv.op(offset, &op).map_err(err)?;
        let reachable = b.reachable;
        // Structural bookkeeping first (frames, reachability, locals).
        match &op {
            Operator::Block { blockty } | Operator::Loop { blockty } | Operator::If { blockty } => {
                let (p, r) = block_arity(types, *blockty);
                let is_if = matches!(op, Operator::If { .. });
                if reachable && is_if {
                    b.stack.pop();
                }
                let base = b.stack.len().saturating_sub(p);
                let kind = match op {
                    Operator::Block { .. } => Kind::Block,
                    Operator::Loop { .. } => Kind::Loop,
                    _ => Kind::If,
                };
                let mut fr = Frame::new(kind.clone(), base, p, r, reachable);
                if reachable {
                    match kind {
                        Kind::Loop => {
                            fr.pre = b.locals_snapshot();
                            let params: Vec<Tag> =
                                (0..b.locals.len()).map(|_| b.fresh(None)).collect();
                            fr.loop_params = params.clone();
                            b.locals = params;
                            // Loop params on the operand stack become header params.
                            for i in base..b.stack.len() {
                                b.stack[i] = b.fresh(None);
                            }
                        }
                        Kind::If => {
                            fr.head_locals = b.locals_snapshot();
                            fr.head_params = b.stack[base..].to_vec();
                        }
                        _ => {}
                    }
                }
                // Fuel: loop and if flush before; block is free.
                if reachable && !matches!(op, Operator::Block { .. }) {
                    if is_if {
                        b.pending += 1;
                    }
                    b.flush();
                }
                b.frames.push(fr);
                let i = remap.instruction(op.clone()).map_err(err)?;
                b.emit(&i);
                if reachable && matches!(op, Operator::Loop { .. }) {
                    b.check();
                }
                continue;
            }
            Operator::Else => {
                if reachable {
                    b.flush();
                }
                let snap = b.locals_snapshot();
                let fr = b.frames.last_mut().ok_or("else without if")?;
                fr.consequent_ends_reachable = Some(reachable);
                if reachable {
                    fr.incoming.push(snap);
                }
                let head = fr.head_reachable;
                let base = fr.base;
                let hp = fr.head_params.clone();
                let hl = fr.head_locals.clone();
                b.emit(&I::Else);
                b.reachable = head;
                if head {
                    b.stack.truncate(base);
                    b.stack.extend(hp);
                    b.locals = hl;
                }
                continue;
            }
            Operator::End => {
                if reachable {
                    b.flush();
                }
                let snap = b.locals_snapshot();
                let mut fr = b.frames.pop().ok_or("unbalanced end")?;
                if fr.kind == Kind::Func {
                    // Function end.
                    if wrap {
                        b.emit(&I::End);
                        if let Some((_, _)) = cfg.depth_limit {
                            b.emit(&I::GlobalGet(g.depth));
                            b.emit(&I::I32Const(weight as i32));
                            b.emit(&I::I32Sub);
                            b.emit(&I::GlobalSet(g.depth));
                        }
                    }
                    b.emit(&I::End);
                    continue;
                }
                let reachable_after = if reachable {
                    true
                } else {
                    let anyway = match (&fr.kind, fr.consequent_ends_reachable) {
                        (Kind::If, None) => fr.head_reachable,
                        (Kind::If, Some(c)) => fr.head_reachable && c,
                        _ => false,
                    };
                    fr.exit_branched || anyway
                };
                if fr.kind == Kind::Loop && fr.head_reachable {
                    // Seal the header: a parameter whose every back edge
                    // carries itself or the entry value is trivial.
                    for i in 0..fr.pre.len() {
                        let p = fr.loop_params[i];
                        let pre = fr.pre[i];
                        let trivial = fr.back.iter().all(|v| {
                            let x = b.resolve(v[i]);
                            x == p || x == pre
                        });
                        if trivial && p != pre {
                            b.alias.insert(p.id, pre);
                        }
                    }
                }
                if reachable_after {
                    let new_locals = match fr.kind {
                        Kind::Loop => snap,
                        _ => {
                            if reachable {
                                fr.incoming.push(snap);
                            }
                            if fr.kind == Kind::If
                                && fr.consequent_ends_reachable.is_none()
                                && fr.head_reachable
                            {
                                fr.incoming.push(fr.head_locals.clone());
                            }
                            if fr.incoming.is_empty() {
                                b.locals_snapshot()
                            } else {
                                b.merge(&fr.incoming)
                            }
                        }
                    };
                    b.locals = new_locals;
                    b.stack.truncate(fr.base);
                    for _ in 0..fr.results {
                        let t = b.fresh(None);
                        b.stack.push(t);
                    }
                } else {
                    b.stack.truncate(fr.base);
                }
                let _ = fr.params;
                b.reachable = reachable_after;
                b.emit(&I::End);
                continue;
            }
            _ => {}
        }
        if !reachable {
            // Unreachable code: emitted (remapped) but never charged.
            match op {
                Operator::Return if wrap => {
                    // Branch to the wrapper block (label = open original blocks).
                    let d = (b.frames.len() - 1) as u32;
                    b.emit(&I::Br(d));
                }
                _ => {
                    let i = remap.instruction(op.clone()).map_err(err)?;
                    b.emit(&i);
                }
            }
            continue;
        }
        let cost: i64 = match op {
            Operator::Nop
            | Operator::Drop
            | Operator::Block { .. }
            | Operator::Loop { .. }
            | Operator::Unreachable
            | Operator::Return
            | Operator::Else
            | Operator::End => 0,
            _ => 1,
        };
        b.pending += cost;
        match &op {
            Operator::Unreachable => {
                b.flush();
                b.emit(&I::Unreachable);
                b.reachable = false;
            }
            Operator::Return => {
                b.flush();
                if wrap {
                    // Branch to the wrapper block (one below the function label).
                    let d = (b.frames.len() - 1) as u32;
                    b.emit(&I::Br(d));
                } else {
                    b.emit(&I::Return);
                }
                b.reachable = false;
            }
            Operator::Call { .. } | Operator::CallIndirect { .. } => {
                b.flush();
                let i = remap.instruction(op.clone()).map_err(err)?;
                b.emit(&i);
                apply_arity(&mut b, arity);
            }
            Operator::Br { relative_depth } => {
                b.flush();
                b.record_branch(*relative_depth);
                b.emit(&I::Br(*relative_depth));
                b.reachable = false;
            }
            Operator::BrIf { relative_depth } => {
                b.flush();
                b.stack.pop();
                b.record_branch(*relative_depth);
                b.emit(&I::BrIf(*relative_depth));
            }
            Operator::BrTable { targets } => {
                b.flush();
                b.stack.pop();
                let mut seen = Vec::new();
                for t in targets.targets() {
                    let t = t.map_err(err)?;
                    if !seen.contains(&t) {
                        seen.push(t);
                    }
                }
                if !seen.contains(&targets.default()) {
                    seen.push(targets.default());
                }
                for t in seen {
                    b.record_branch(t);
                }
                let i = remap.instruction(op.clone()).map_err(err)?;
                b.emit(&i);
                b.reachable = false;
            }
            Operator::I32Const { value } => {
                let t = b.fresh(Some(u64::from(*value as u32)));
                b.stack.push(t);
                b.emit(&I::I32Const(*value));
            }
            Operator::I64Const { value } => {
                let t = b.fresh(Some(*value as u64));
                b.stack.push(t);
                b.emit(&I::I64Const(*value));
            }
            Operator::LocalGet { local_index } => {
                let t = b.resolve(b.locals[*local_index as usize]);
                b.stack.push(t);
                b.emit(&I::LocalGet(*local_index));
            }
            Operator::LocalSet { local_index } => {
                let t = b.stack.pop().unwrap_or(Tag { id: 0, konst: None });
                b.locals[*local_index as usize] = t;
                b.emit(&I::LocalSet(*local_index));
            }
            Operator::LocalTee { local_index } => {
                let t = *b.stack.last().unwrap_or(&Tag { id: 0, konst: None });
                b.locals[*local_index as usize] = t;
                b.emit(&I::LocalTee(*local_index));
            }
            _ => {
                if let Some(kind) = bulk(&op) {
                    let units = b.stack.last().and_then(|t| b.resolve(*t).konst);
                    let small =
                        units.filter(|u| u.checked_mul(1).is_some_and(|c| c <= SMALL_BULK_OP_COST));
                    let i = remap.instruction(op.clone()).map_err(err)?;
                    match (kind, small, units) {
                        // Small constant: charged up front, no check.
                        (Bulk::TableGrow, Some(u), _) => {
                            b.pending += u as i64;
                            b.emit(&i);
                            b.flush();
                        }
                        (_, Some(u), _) => {
                            b.pending += u as i64;
                            b.emit(&i);
                        }
                        // Deferred: charged after success, then checked.
                        (Bulk::Plain, None, Some(u)) => {
                            b.emit(&i);
                            b.pending += u.min(i64::MAX as u64) as i64;
                            b.check();
                        }
                        (Bulk::Plain, None, None) => {
                            b.emit(&I::LocalTee(b.t_len));
                            b.emit(&i);
                            b.flush();
                            let l = b.t_len;
                            b.charge_local(l);
                            b.check();
                        }
                        (Bulk::MemoryGrow | Bulk::TableGrow, None, u) => {
                            // delta on top: save, grow, flush, charge on success.
                            if u.is_none() {
                                b.emit(&I::LocalTee(b.t_len));
                            }
                            b.emit(&i);
                            b.flush();
                            let res = b.t_res;
                            b.emit(&I::LocalTee(res));
                            b.emit(&I::I32Const(-1));
                            b.emit(&I::I32Ne);
                            b.emit(&I::If(EBlockType::Empty));
                            match u {
                                Some(u) => {
                                    b.pending += u.min(i64::MAX as u64) as i64;
                                }
                                None => {
                                    let l = b.t_len;
                                    b.charge_local(l);
                                }
                            }
                            b.check();
                            b.emit(&I::End);
                            b.emit(&I::LocalGet(res));
                        }
                    }
                    apply_arity(&mut b, arity);
                } else {
                    let i = remap.instruction(op.clone()).map_err(err)?;
                    b.emit(&i);
                    if cfg.nan_canon {
                        canonicalise(&mut b, &op);
                    }
                    if !matches!(op, Operator::Nop) {
                        apply_arity(&mut b, arity);
                    }
                }
            }
        }
    }
    reader.finish().map_err(err)?;
    stats.0 += b.checks;
    stats.1 += b.flushes;
    Ok(b.out)
}

/// After a float operator whose NaN result Cranelift canonicalises
/// (`cranelift-codegen` `nan_canonicalization::is_fp_arith`: add, sub, mul,
/// div, min, max, sqrt, ceil, floor, trunc, nearest, demote, promote),
/// replace a NaN result by the canonical NaN: `x == x ? x : canonical`.
fn canonicalise(b: &mut Body<'_>, op: &Operator<'_>) {
    use Operator as O;
    let f32_op = matches!(
        op,
        O::F32Add
            | O::F32Sub
            | O::F32Mul
            | O::F32Div
            | O::F32Min
            | O::F32Max
            | O::F32Sqrt
            | O::F32Ceil
            | O::F32Floor
            | O::F32Trunc
            | O::F32Nearest
            | O::F32DemoteF64
    );
    let f64_op = matches!(
        op,
        O::F64Add
            | O::F64Sub
            | O::F64Mul
            | O::F64Div
            | O::F64Min
            | O::F64Max
            | O::F64Sqrt
            | O::F64Ceil
            | O::F64Floor
            | O::F64Trunc
            | O::F64Nearest
            | O::F64PromoteF32
    );
    if f32_op {
        let t = b.t_f32;
        b.emit(&I::LocalTee(t));
        b.emit(&I::F32Const(wasm_encoder::Ieee32::new(0x7fc0_0000)));
        b.emit(&I::LocalGet(t));
        b.emit(&I::LocalGet(t));
        b.emit(&I::F32Eq);
        b.emit(&I::Select);
    } else if f64_op {
        let t = b.t_f64;
        b.emit(&I::LocalTee(t));
        b.emit(&I::F64Const(wasm_encoder::Ieee64::new(
            0x7ff8_0000_0000_0000,
        )));
        b.emit(&I::LocalGet(t));
        b.emit(&I::LocalGet(t));
        b.emit(&I::F64Eq);
        b.emit(&I::Select);
    }
}

/// Pops the operator's inputs and pushes unknown outputs.
fn apply_arity(b: &mut Body<'_>, arity: Option<(u32, u32)>) {
    let (p, r) = arity.unwrap_or((0, 0));
    for _ in 0..p {
        b.stack.pop();
    }
    for _ in 0..r {
        let t = b.fresh(None);
        b.stack.push(t);
    }
}
