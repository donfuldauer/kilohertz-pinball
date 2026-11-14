//! Pre-compile limits (D25): everything [`super::WasmScriptBackend::prepare`]
//! checks before wasmtime compiles a module, so that any module that passes
//! compiles in well under a second (Subphase 4.5 fix rounds 1–3).
//!
//! The module is validated with wasmparser under exactly the proposals the
//! engine enables ([`FEATURES`]), function by function, without compiling.
//! Besides plain counts, Cranelift's compile cost is bounded by three
//! module-wide budgets over per-function products, and every operator is
//! charged by its cost class ([`OpCost`]): a linear weight (relative to a
//! plain arithmetic operator) and the number of blocks Cranelift expands it
//! into. Operators not listed in [`op_cost`] fall into the most expensive
//! class.

use super::{EXPORTS, load};
use pin_core::ScriptError;
use std::borrow::Cow;
use std::fmt;
use wasmtime::wasmparser::{
    DataKind, ElementItems, ElementKind, ExternalKind, FuncToValidate, FuncValidatorAllocations,
    FunctionBody, Operator, OperatorsReader, OperatorsReaderAllocations, Parser, Payload, TypeRef,
    ValidPayload, Validator, ValidatorResources, WasmFeatures,
};

/// The proposals the engine enables (and the only ones a module may use):
/// Wasm 1.0 plus mutable globals, sign extension, saturating float-to-int,
/// multi-value, bulk memory and reference types. rustc's `wasm32` target
/// emits all of these by default (bulk memory for `memset`/`memcpy`,
/// reference types for the `call_indirect` encoding). SIMD, tail calls,
/// extended constant expressions and every later proposal are off.
pub(super) const FEATURES: WasmFeatures = WasmFeatures::WASM1
    .union(WasmFeatures::SATURATING_FLOAT_TO_INT)
    .union(WasmFeatures::SIGN_EXTENSION)
    .union(WasmFeatures::MULTI_VALUE)
    .union(WasmFeatures::BULK_MEMORY)
    .union(WasmFeatures::REFERENCE_TYPES);

/// Largest binary module accepted, bytes.
pub const MAX_MODULE_BYTES: usize = 1024 * 1024;

/// Largest WAT text module accepted, bytes. The binary it compiles to must
/// still fit [`MAX_MODULE_BYTES`].
pub const MAX_WAT_BYTES: usize = 4 * 1024 * 1024;

/// Longest WAT parse error kept in a [`ScriptError::Load`] message, bytes
/// (the parser quotes the offending line, which may be megabytes long).
pub const MAX_ERROR_BYTES: usize = 256;

/// Most functions a module may define (imports not counted).
pub const MAX_FUNCTIONS: u32 = 2_048;

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

/// Deepest nesting of `block` / `loop` / `if` in a function. A Rust `match`
/// over n values compiles to n nested blocks and one `br_table`, so this
/// allows matches over a few hundred switch ids (nested blocks are cheap
/// for Cranelift; nested loops are capped separately).
pub const MAX_NESTING: u64 = 512;

/// Deepest nesting of `loop`s in a function. Cranelift's compile time
/// explodes with loop depth when many values are live (1 024 locals: 96
/// nested loops ≈ 0.9 s, 128 ≈ 54 s).
pub const MAX_LOOP_NESTING: u64 = 32;

/// Most targets (default included) of one `br_table`.
pub const MAX_BR_TABLE_TARGETS: u64 = 1_024;

/// Most `br_table` targets (defaults included) in one function.
pub const MAX_BR_TABLE_TARGETS_PER_FUNCTION: u64 = 1_024;

/// Most operators (instructions, `end` included) in one function body.
pub const MAX_OPERATORS_PER_FUNCTION: u64 = 65_536;

/// Most operators in all function bodies together.
pub const MAX_OPERATORS: u64 = 131_072;

/// Module-wide budget of operator cost units: Σ of every operator's
/// [`OpCost::weight`] (1 for a plain arithmetic operator).
pub const MAX_OPERATOR_COST: u64 = 131_072;

/// Most control constructs (`block`, `loop`, `if`) in all functions
/// together (each loop also gets a fuel check; ≈ 35 µs of compile time).
pub const MAX_BLOCKS: u64 = 4_096;

/// Most entries of the type section.
pub const MAX_TYPES: u32 = 256;

/// Most parameters of one function type.
pub const MAX_PARAMS: u32 = 32;

/// Most results of one function type.
pub const MAX_RESULTS: u32 = 8;

/// Most items (function indices or expressions) of all element segments.
pub const MAX_ELEMENT_ITEMS: u64 = 1_024;

/// Most exports.
pub const MAX_EXPORTS: u32 = 256;

/// Most imports (the ABI has 18 functions).
pub const MAX_IMPORTS: u32 = 64;

/// Module-wide budget for Σ over functions of live values × edges, where
/// live values = params + locals + the deepest operand stack, and edges =
/// control constructs + branches + `br_table` targets + the blocks
/// operators expand into ([`OpCost::edges`]): an upper bound on the SSA
/// block arguments Cranelift creates and allocates registers for.
pub const MAX_MERGE_WORK: u64 = 65_536;

/// Module-wide budget for Σ over functions of edges²: Cranelift's cost
/// grows quadratically with the number of blocks in one function.
pub const MAX_BRANCH_WORK: u64 = 2 * 1024 * 1024;

/// Module-wide budget for Σ over functions of live values × (operator cost
/// units + params + locals): register-allocation cost when many values are
/// live (every declared local is also zero-initialised).
pub const MAX_PRESSURE_WORK: u64 = 8 * 1024 * 1024;

/// Module-wide budget for Σ over functions of (operator cost units inside
/// loops)²: register allocation of a long loop body grows quadratically
/// with its length for some local-update patterns (65 000 updates of 14
/// locals in one loop ≈ 1.4 s; 23 000 ≈ 0.15 s), whatever the number of
/// locals. 2²⁹ allows ≈ 23 000 units in the loops of one function.
pub const MAX_LOOP_WORK: u64 = 1 << 29;

/// Compile-cost class of one operator.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct OpCost {
    /// Linear compile cost, in plain-operator units.
    pub weight: u64,
    /// Blocks (edges) Cranelift creates for it, beyond the structured
    /// control flow counted separately.
    pub edges: u64,
}

/// Arithmetic, constants, locals, globals, loads, stores, conversions,
/// `select`, `drop`, `nop` and the structured control operators (whose
/// blocks are counted by the control accounting).
pub const PLAIN: OpCost = OpCost {
    weight: 1,
    edges: 0,
};

/// `call`: a call site (all registers clobbered, fuel saved and reloaded).
pub const CALL: OpCost = OpCost {
    weight: 8,
    edges: 0,
};

/// `call_indirect`, `table.get`, `table.set`: bounds check, lazy
/// table-element initialisation (a branch to a libcall) and signature check.
pub const TABLE_ACCESS: OpCost = OpCost {
    weight: 64,
    edges: 4,
};

/// Libcall-backed operators with size-proportional fuel (`memory.grow`,
/// `memory.fill/copy/init`, `table.grow/fill/copy/init/size`, `ref.func`,
/// `data.drop`, `elem.drop`): a libcall plus a deferred fuel check.
pub const LIBCALL: OpCost = OpCost {
    weight: 64,
    edges: 4,
};

/// Every operator not classified above (none the engine accepts today):
/// the most expensive class, so a new operator is never free.
pub const MOST_EXPENSIVE: OpCost = OpCost {
    weight: 64,
    edges: 4,
};

/// The cost class of `op`. Unlisted operators are [`MOST_EXPENSIVE`].
pub fn op_cost(op: &Operator<'_>) -> OpCost {
    use Operator::*;
    match op {
        // Control (blocks and branches are counted structurally).
        Unreachable
        | Nop
        | Block { .. }
        | Loop { .. }
        | If { .. }
        | Else
        | End
        | Br { .. }
        | BrIf { .. }
        | BrTable { .. }
        | Return => PLAIN,
        // Parametric, variables, constants.
        Drop
        | Select
        | TypedSelect { .. }
        | LocalGet { .. }
        | LocalSet { .. }
        | LocalTee { .. }
        | GlobalGet { .. }
        | GlobalSet { .. }
        | I32Const { .. }
        | I64Const { .. }
        | F32Const { .. }
        | F64Const { .. }
        | RefNull { .. }
        | RefIsNull
        | MemorySize { .. } => PLAIN,
        // Memory access.
        I32Load { .. }
        | I64Load { .. }
        | F32Load { .. }
        | F64Load { .. }
        | I32Load8S { .. }
        | I32Load8U { .. }
        | I32Load16S { .. }
        | I32Load16U { .. }
        | I64Load8S { .. }
        | I64Load8U { .. }
        | I64Load16S { .. }
        | I64Load16U { .. }
        | I64Load32S { .. }
        | I64Load32U { .. }
        | I32Store { .. }
        | I64Store { .. }
        | F32Store { .. }
        | F64Store { .. }
        | I32Store8 { .. }
        | I32Store16 { .. }
        | I64Store8 { .. }
        | I64Store16 { .. }
        | I64Store32 { .. } => PLAIN,
        // Integer comparison and arithmetic.
        I32Eqz | I32Eq | I32Ne | I32LtS | I32LtU | I32GtS | I32GtU | I32LeS | I32LeU | I32GeS
        | I32GeU | I64Eqz | I64Eq | I64Ne | I64LtS | I64LtU | I64GtS | I64GtU | I64LeS | I64LeU
        | I64GeS | I64GeU | I32Clz | I32Ctz | I32Popcnt | I32Add | I32Sub | I32Mul | I32DivS
        | I32DivU | I32RemS | I32RemU | I32And | I32Or | I32Xor | I32Shl | I32ShrS | I32ShrU
        | I32Rotl | I32Rotr | I64Clz | I64Ctz | I64Popcnt | I64Add | I64Sub | I64Mul | I64DivS
        | I64DivU | I64RemS | I64RemU | I64And | I64Or | I64Xor | I64Shl | I64ShrS | I64ShrU
        | I64Rotl | I64Rotr | I32Extend8S | I32Extend16S | I64Extend8S | I64Extend16S
        | I64Extend32S => PLAIN,
        // Float comparison and arithmetic.
        F32Eq | F32Ne | F32Lt | F32Gt | F32Le | F32Ge | F64Eq | F64Ne | F64Lt | F64Gt | F64Le
        | F64Ge | F32Abs | F32Neg | F32Ceil | F32Floor | F32Trunc | F32Nearest | F32Sqrt
        | F32Add | F32Sub | F32Mul | F32Div | F32Min | F32Max | F32Copysign | F64Abs | F64Neg
        | F64Ceil | F64Floor | F64Trunc | F64Nearest | F64Sqrt | F64Add | F64Sub | F64Mul
        | F64Div | F64Min | F64Max | F64Copysign => PLAIN,
        // Conversions.
        I32WrapI64 | I32TruncF32S | I32TruncF32U | I32TruncF64S | I32TruncF64U | I64ExtendI32S
        | I64ExtendI32U | I64TruncF32S | I64TruncF32U | I64TruncF64S | I64TruncF64U
        | F32ConvertI32S | F32ConvertI32U | F32ConvertI64S | F32ConvertI64U | F32DemoteF64
        | F64ConvertI32S | F64ConvertI32U | F64ConvertI64S | F64ConvertI64U | F64PromoteF32
        | I32ReinterpretF32 | I64ReinterpretF64 | F32ReinterpretI32 | F64ReinterpretI64
        | I32TruncSatF32S | I32TruncSatF32U | I32TruncSatF64S | I32TruncSatF64U
        | I64TruncSatF32S | I64TruncSatF32U | I64TruncSatF64S | I64TruncSatF64U => PLAIN,
        Call { .. } => CALL,
        CallIndirect { .. } | TableGet { .. } | TableSet { .. } => TABLE_ACCESS,
        MemoryGrow { .. }
        | MemoryInit { .. }
        | MemoryCopy { .. }
        | MemoryFill { .. }
        | DataDrop { .. }
        | TableInit { .. }
        | TableCopy { .. }
        | ElemDrop { .. }
        | TableFill { .. }
        | TableGrow { .. }
        | TableSize { .. }
        | RefFunc { .. } => LIBCALL,
        _ => MOST_EXPENSIVE,
    }
}

/// How much of each limit a module uses (from [`check_limits`]).
#[doc(hidden)]
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub functions: u64,
    pub operators: u64,
    pub operator_cost: u64,
    pub blocks: u64,
    pub merge_work: u64,
    pub branch_work: u64,
    pub pressure_work: u64,
    pub loop_work: u64,
    pub locals: u64,
    pub max_locals: u64,
    pub max_operators: u64,
    pub max_nesting: u64,
    pub max_loop_nesting: u64,
    pub max_stack: u64,
    pub max_br_table_targets: u64,
    pub types: u64,
    pub imports: u64,
    pub exports: u64,
    pub element_items: u64,
    /// Fuel wasmtime's synthesized start-up code charges to initialise the
    /// instance before the module's own start function runs: one unit per
    /// byte of every active data segment and per item of every active
    /// element segment (wasmtime's `memory_init_per_byte` and
    /// `table_init_per_element` costs). [`super::Live::new`] adds it to the
    /// start budget, so instantiation costs the guest nothing on any
    /// platform.
    pub init_fuel: u64,
}

/// `s` cut to at most `max` bytes on a character boundary, with `…` (3
/// bytes, counted) marking a cut.
pub(super) fn truncated(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max - '…'.len_utf8();
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Converts WAT text to binary and checks every limit (see the module
/// docs) without compiling anything. Returns the binary and its usage.
pub(super) fn precheck(bytes: &[u8]) -> Result<(Cow<'_, [u8]>, Usage), ScriptError> {
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
    let usage = scan(&bin)?;
    Ok((bin, usage))
}

/// Checks `bytes` (binary or WAT) against every pre-compile limit without
/// compiling, and reports how much of each it uses. For tests and tools.
#[doc(hidden)]
pub fn check_limits(bytes: &[u8]) -> Result<Usage, ScriptError> {
    // Exactly the checks `prepare` runs before compiling (including the WAT
    // text cap), so the two never disagree.
    precheck(bytes).map(|(_, usage)| usage)
}

fn scan(bin: &[u8]) -> Result<Usage, ScriptError> {
    let mut sc = Scan {
        validator: Validator::new_with_features(FEATURES),
        imported_funcs: 0,
        func_allocs: FuncValidatorAllocations::default(),
        op_allocs: OperatorsReaderAllocations::default(),
        u: Usage::default(),
        startup: false,
        tables: Vec::new(),
        precomputed: true,
    };
    for payload in Parser::new(0).parse_all(bin) {
        sc.payload(payload.map_err(parse_err)?)?;
    }
    // The start-up function's own exit charge.
    sc.u.init_fuel += u64::from(sc.startup);
    Ok(sc.u)
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
struct Scan {
    validator: Validator,
    imported_funcs: u32,
    func_allocs: FuncValidatorAllocations,
    op_allocs: OperatorsReaderAllocations,
    u: Usage,
    /// Whether wasmtime will run its start-up code (see [`Usage::init_fuel`]).
    startup: bool,
    /// Defined tables: initial size and whether they hold funcrefs.
    tables: Vec<(u64, bool)>,
    /// Whether every active element segment so far is precomputed.
    precomputed: bool,
}

impl Scan {
    fn payload(&mut self, payload: Payload<'_>) -> Result<(), ScriptError> {
        // Cheap count checks first, then full validation of the section.
        self.counts(&payload)?;
        match self.validator.payload(&payload).map_err(parse_err)? {
            ValidPayload::Func(func, body) => self.body(func, &body),
            _ => Ok(()),
        }
    }

    fn counts(&mut self, payload: &Payload<'_>) -> Result<(), ScriptError> {
        let module = format_args!("module");
        match payload {
            Payload::TypeSection(r) => {
                cap(module, "types", r.count().into(), MAX_TYPES.into())?;
                self.u.types = r.count().into();
                for (i, ty) in r.clone().into_iter_err_on_gc_types().enumerate() {
                    let ty = ty.map_err(parse_err)?;
                    let params = ty.params().len() as u64;
                    cap(
                        format_args!("type {i}"),
                        "params",
                        params,
                        MAX_PARAMS.into(),
                    )?;
                    let results = ty.results().len() as u64;
                    cap(
                        format_args!("type {i}"),
                        "results",
                        results,
                        MAX_RESULTS.into(),
                    )?;
                }
            }
            Payload::ImportSection(r) => {
                let mut n = 0u64;
                for imp in r.clone().into_imports() {
                    n += 1;
                    cap(module, "imports", n, MAX_IMPORTS.into())?;
                    if matches!(
                        imp.map_err(parse_err)?.ty,
                        TypeRef::Func(_) | TypeRef::FuncExact(_)
                    ) {
                        self.imported_funcs += 1;
                    }
                }
                self.u.imports = n;
            }
            Payload::FunctionSection(r) => {
                cap(module, "functions", r.count().into(), MAX_FUNCTIONS.into())?;
                self.u.functions = r.count().into();
            }
            Payload::CodeSectionStart { count, .. } => {
                cap(
                    module,
                    "function bodies",
                    (*count).into(),
                    MAX_FUNCTIONS.into(),
                )?;
            }
            Payload::GlobalSection(r) => {
                cap(module, "globals", r.count().into(), MAX_GLOBALS.into())?;
            }
            Payload::TableSection(r) => {
                let mut total = 0u64;
                for t in r.clone() {
                    let ty = t.map_err(parse_err)?.ty;
                    total = total.saturating_add(ty.initial);
                    self.tables
                        .push((ty.initial, ty.element_type.is_func_ref()));
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
                for d in r.clone() {
                    let d = d.map_err(parse_err)?;
                    if matches!(d.kind, DataKind::Active { .. }) {
                        // The offset expression plus one unit per byte.
                        self.u.init_fuel += 1 + d.data.len() as u64;
                        self.startup = true;
                    }
                }
            }
            Payload::ElementSection(r) => {
                cap(
                    module,
                    "element segments",
                    r.count().into(),
                    MAX_SEGMENTS.into(),
                )?;
                for e in r.clone() {
                    let e = e.map_err(parse_err)?;
                    let (n, exprs) = match e.items {
                        ElementItems::Functions(f) => (u64::from(f.count()), 0),
                        ElementItems::Expressions(_, x) => {
                            (u64::from(x.count()), u64::from(x.count()))
                        }
                    };
                    self.u.element_items += n;
                    self.element_init_fuel(&e.kind, n, exprs)?;
                    let total = self.u.element_items;
                    cap(module, "element segment items", total, MAX_ELEMENT_ITEMS)?;
                }
            }
            Payload::StartSection { .. } => self.startup = true,
            Payload::ExportSection(r) => {
                cap(module, "exports", r.count().into(), MAX_EXPORTS.into())?;
                self.u.exports = r.count().into();
                for e in r.clone() {
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

    /// Adds what wasmtime's start-up code charges for one element segment
    /// of `n` items (`exprs` of them expressions) to [`Usage::init_fuel`]:
    /// one unit per item expression and, for an active segment, its offset
    /// expression plus one unit per item written. Like wasmtime, a leading
    /// run of active segments of plain function indices at constant
    /// offsets inside a funcref table's initial size is precomputed at
    /// compile time and costs nothing.
    fn element_init_fuel(
        &mut self,
        kind: &ElementKind<'_>,
        n: u64,
        exprs: u64,
    ) -> Result<(), ScriptError> {
        match kind {
            ElementKind::Declared => return Ok(()),
            ElementKind::Passive => self.u.init_fuel += exprs,
            ElementKind::Active {
                table_index,
                offset_expr,
            } => {
                let mut ops = offset_expr.get_operators_reader();
                let offset = match (ops.read(), ops.read()) {
                    (Ok(Operator::I32Const { value }), Ok(Operator::End)) => {
                        Some(u64::from(value as u32))
                    }
                    _ => None,
                };
                let table = self.tables.get(table_index.unwrap_or(0) as usize);
                self.precomputed &= exprs == 0
                    && matches!((offset, table), (Some(o), Some(&(min, true))) if o + n <= min);
                if self.precomputed {
                    return Ok(());
                }
                self.u.init_fuel += exprs + 1 + n;
            }
        }
        self.startup = true;
        Ok(())
    }

    /// Validates one function body operator by operator, checks its
    /// limits and adds its compile-cost estimates to the module totals.
    fn body(
        &mut self,
        func: FuncToValidate<ValidatorResources>,
        body: &FunctionBody<'_>,
    ) -> Result<(), ScriptError> {
        let r = body.range();
        let len = r.end - r.start;
        if len > MAX_FUNCTION_BODY_BYTES {
            return Err(load(format_args!(
                "a function body is {len} bytes (at most {MAX_FUNCTION_BODY_BYTES})"
            )));
        }
        let f = func.index;
        let fname = format!("function {f}");
        // Declared locals, counted before the validator allocates them.
        let mut locals_reader = body.get_locals_reader().map_err(parse_err)?;
        let mut locals = 0u64;
        for _ in 0..locals_reader.get_count() {
            locals += u64::from(locals_reader.read().map_err(parse_err)?.0);
            cap(
                format_args!("{fname}"),
                "locals",
                locals,
                MAX_LOCALS_PER_FUNCTION,
            )?;
        }
        self.u.locals += locals;
        cap(
            format_args!("module"),
            "locals",
            self.u.locals,
            MAX_TOTAL_LOCALS,
        )?;
        self.u.max_locals = self.u.max_locals.max(locals);

        let mut fv = func.into_validator(std::mem::take(&mut self.func_allocs));
        fv.read_locals(&mut body.get_binary_reader())
            .map_err(parse_err)?;
        // Params + declared locals.
        let vars = u64::from(fv.len_locals());
        let reader = body.get_binary_reader_for_operators().map_err(parse_err)?;
        let mut ops = OperatorsReader::new_with_allocs(reader, std::mem::take(&mut self.op_allocs));
        let (mut n, mut weight, mut stack, mut in_loops) = (0u64, 0u64, 0u64, 0u64);
        // Open constructs, and which of them are loops (bit i of `loop_bits`
        // for depth i < MAX_NESTING = 512).
        let (mut depth, mut loop_bits, mut loop_depth) = (0u64, [0u64; 8], 0u64);
        let (mut blocks, mut branches, mut targets, mut op_edges) = (0u64, 0u64, 0u64, 0u64);
        while !ops.eof() {
            let offset = ops.original_position();
            let op = ops.read().map_err(parse_err)?;
            fv.op(offset, &op).map_err(parse_err)?;
            n += 1;
            if n > MAX_OPERATORS_PER_FUNCTION {
                return cap(
                    format_args!("{fname}"),
                    "operators",
                    n,
                    MAX_OPERATORS_PER_FUNCTION,
                );
            }
            stack = stack.max(u64::from(fv.operand_stack_height()));
            let c = op_cost(&op);
            weight += c.weight;
            op_edges += c.edges;
            if loop_depth > 0 {
                in_loops += c.weight;
            }
            match op {
                Operator::Block { .. } | Operator::Loop { .. } | Operator::If { .. } => {
                    cap(
                        format_args!("{fname}"),
                        "nested blocks",
                        depth + 1,
                        MAX_NESTING,
                    )?;
                    if matches!(op, Operator::Loop { .. }) {
                        loop_bits[(depth / 64) as usize] |= 1 << (depth % 64);
                        loop_depth += 1;
                        cap(
                            format_args!("{fname}"),
                            "nested loops",
                            loop_depth,
                            MAX_LOOP_NESTING,
                        )?;
                        self.u.max_loop_nesting = self.u.max_loop_nesting.max(loop_depth);
                    }
                    depth += 1;
                    self.u.max_nesting = self.u.max_nesting.max(depth);
                    blocks += 1;
                }
                Operator::End if depth > 0 => {
                    depth -= 1;
                    let (w, b) = ((depth / 64) as usize, 1u64 << (depth % 64));
                    if loop_bits[w] & b != 0 {
                        loop_bits[w] &= !b;
                        loop_depth -= 1;
                    }
                }
                Operator::Else | Operator::Br { .. } | Operator::BrIf { .. } | Operator::Return => {
                    branches += 1;
                }
                Operator::BrTable { targets: t } => {
                    let k = u64::from(t.len()) + 1;
                    let who_t = format_args!("a br_table in {fname}");
                    cap(who_t, "targets", k, MAX_BR_TABLE_TARGETS)?;
                    targets += k;
                    let what = "br_table targets";
                    cap(
                        format_args!("{fname}"),
                        what,
                        targets,
                        MAX_BR_TABLE_TARGETS_PER_FUNCTION,
                    )?;
                    self.u.max_br_table_targets = self.u.max_br_table_targets.max(targets);
                }
                _ => {}
            }
        }
        ops.finish().map_err(parse_err)?;
        self.op_allocs = ops.into_allocations();
        self.func_allocs = fv.into_allocations();

        let u = &mut self.u;
        u.max_operators = u.max_operators.max(n);
        u.max_stack = u.max_stack.max(stack);
        let module = || format_args!("module");
        u.blocks += blocks;
        cap(module(), "blocks, loops and ifs", u.blocks, MAX_BLOCKS)?;
        u.operators += n;
        cap(module(), "operators", u.operators, MAX_OPERATORS)?;
        u.operator_cost += weight;
        cap(
            module(),
            "operator cost units",
            u.operator_cost,
            MAX_OPERATOR_COST,
        )?;
        // Compile-cost estimates (see MAX_MERGE_WORK and friends). Values
        // on the operand stack are live like locals.
        let live = vars + stack;
        let edges = blocks + branches + targets + op_edges;
        u.merge_work += live * edges;
        let what = "compile-cost units of live values x branch edges";
        cap(module(), what, u.merge_work, MAX_MERGE_WORK)?;
        u.branch_work += edges * edges;
        let what = "compile-cost units of branch edges squared";
        cap(module(), what, u.branch_work, MAX_BRANCH_WORK)?;
        u.pressure_work += live * (weight + vars);
        let what = "compile-cost units of live values x operators";
        cap(module(), what, u.pressure_work, MAX_PRESSURE_WORK)?;
        u.loop_work += in_loops * in_loops;
        let what = "compile-cost units of operators in loops squared";
        cap(module(), what, u.loop_work, MAX_LOOP_WORK)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_char_boundaries() {
        assert_eq!(truncated("short", 256), "short");
        // 'é' is 2 bytes and '…' 3: 6 bytes leave room for one 'é'.
        assert_eq!(truncated("éééé", 6), "é…");
        assert_eq!(truncated("éééé", 7), "éé…");
        assert!(truncated(&"x".repeat(10_000), MAX_ERROR_BYTES).len() <= MAX_ERROR_BYTES);
    }
}
