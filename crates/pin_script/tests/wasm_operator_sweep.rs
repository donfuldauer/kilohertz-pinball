//! Subphase 4.5 (fix round 3, B1): operator sweep.
//!
//! For every operator wasmparser knows (`for_each_operator!`, not a
//! hand-written list), this builds a small "unit" that runs the operator once
//! (operands found by searching value types, results dropped), then the
//! worst module that sits exactly at the pre-compile limits for it: the unit
//! repeated as often as `check_limits` accepts, in one function, in the
//! maximum number of functions, and in one function on top of a deep operand
//! stack. One more unit must be rejected. Operators of proposals the engine
//! disables must be rejected outright.
//!
//! `operator_sweep_report` (ignored) compiles every accepted at-limit module
//! in release and prints the time and peak RSS, one process per case:
//!
//! ```sh
//! n=$(PIN_SWEEP_COUNT=1 cargo test -p pin_script --release --test wasm_operator_sweep \
//!       -- --ignored --nocapture operator_sweep_report | grep -o 'CASES [0-9]*' | cut -d' ' -f2)
//! for i in $(seq 0 $((n - 1))); do PIN_SWEEP_CASE=$i cargo test -p pin_script --release \
//!   --test wasm_operator_sweep -- --ignored --nocapture operator_sweep_report; done
//! ```

use pin_core::ScriptError;
use pin_script::wasm::{
    MAX_FUNCTIONS, MAX_OPERATORS_PER_FUNCTION, WasmConfig, WasmScriptBackend, check_limits,
};
use wasmtime::wasmparser::{Parser, Payload, Validator, WasmFeatures};

/// One operator from wasmparser's list.
#[derive(Clone, Debug)]
struct OpInfo {
    proposal: &'static str,
    name: &'static str,
    visit: &'static str,
    args: &'static [&'static str],
    arity: &'static str,
}

macro_rules! collect_ops {
    ($( @$proposal:ident $op:ident $({ $($arg:ident: $argty:ty),* })? => $visit:ident ($($ann:tt)*) )*) => {
        vec![$(
            OpInfo {
                proposal: stringify!($proposal),
                name: stringify!($op),
                visit: stringify!($visit),
                args: &[$($(stringify!($arg)),*)?],
                arity: stringify!($($ann)*),
            }
        ),*]
    };
}

fn all_ops() -> Vec<OpInfo> {
    wasmtime::wasmparser::for_each_operator!(collect_ops)
}

/// Proposals the engine enables (`pin_script::wasm` FEATURES).
const ENABLED: &[&str] = &[
    "mvp",
    "sign_extension",
    "saturating_float_to_int",
    "bulk_memory",
    "reference_types",
];

/// Index of the unit function in [`template`] (after the three required
/// exports and `$f0`).
const SWEEP_FUNC: usize = 4;
/// Functions [`template`] defines besides the unit function.
const OTHER_FUNCS: u64 = 4;
/// Depth of the operand stack in the "deep" variant.
/// (Deeper stacks are rejected for every operator: the stacked constants
/// alone exceed the pressure budget at ≈ 2 000.)
const DEEP: u64 = 1_024;

fn template(unit: &str) -> String {
    format!(
        r#"(module
  (type $ty (func))
  (memory (export "memory") 1)
  (table $t 16 funcref)
  (global $g (mut i32) (i32.const 0))
  (data $d "abcd")
  (elem $e func $f0)
  (func (export "on_init")) (func (export "on_tick"))
  (func (export "on_switch_edge") (param i32 i32))
  (func $f0)
  (func $sweep (local i32 i64 f32 f64 funcref externref)
{unit}
  ))"#
    )
}

const TYPES: [(&str, &str); 7] = [
    ("i32", "i32.const 0"),
    ("i64", "i64.const 0"),
    ("f32", "f32.const 0"),
    ("f64", "f64.const 0"),
    ("funcref", "ref.null func"),
    ("externref", "ref.null extern"),
    ("v128", "v128.const i64x2 0 0"),
];

/// WAT mnemonic of a `visit_*` name.
fn mnemonic(visit: &str) -> String {
    let s = visit.strip_prefix("visit_").unwrap_or(visit);
    if s == "typed_select" {
        return "select".into();
    }
    if let Some(rest) = s.strip_prefix("atomic_") {
        return format!("atomic.{rest}");
    }
    for p in [
        "i32", "i64", "f32", "f64", "v128", "i8x16", "i16x8", "i32x4", "i64x2", "f32x4", "f64x2",
        "local", "global", "memory", "table", "ref", "data", "elem",
    ] {
        if let Some(rest) = s.strip_prefix(p).and_then(|r| r.strip_prefix('_')) {
            let rest = match rest.strip_prefix("atomic_") {
                Some(r) => format!(
                    "atomic.{}",
                    r.replacen("rmw8_", "rmw8.", 1)
                        .replacen("rmw16_", "rmw16.", 1)
                        .replacen("rmw32_", "rmw32.", 1)
                        .replacen("rmw_", "rmw.", 1)
                ),
                None => rest.to_string(),
            };
            return format!("{p}.{rest}");
        }
    }
    s.to_string()
}

/// WAT immediates for the operator's fields, by field name. `None` for a
/// field this generator does not know (the operator is then reported).
fn immediates(op: &OpInfo, result_ty: &str) -> Option<String> {
    let mut s = String::new();
    for a in op.args {
        let imm = match *a {
            "memarg" | "mem" | "dst_mem" | "src_mem" => "",
            "local_index" => "0",
            "global_index" => "$g",
            "value" if op.name == "V128Const" => "i64x2 0 0",
            "value" => "0",
            "lane" => "0",
            "lanes" => "0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0",
            "ordering" => "",
            "data_index" => "$d",
            "elem_index" => "$e",
            "table" | "dst_table" | "src_table" | "table_index" => "$t",
            "function_index" => "$f0",
            "hty" => "func",
            "type_index" => "(type $ty)",
            "ty" => {
                s.push_str(&format!(" (result {result_ty})"));
                continue;
            }
            _ => return None,
        };
        if !imm.is_empty() {
            s.push(' ');
            s.push_str(imm);
        }
    }
    // `table.copy` takes two tables, `call_indirect` the table first.
    if op.visit == "visit_table_copy" {
        s = " $t $t".into();
    }
    if op.visit == "visit_table_init" {
        s = " $t $e".into();
    }
    if op.visit == "visit_call_indirect" {
        s = " $t (type $ty)".into();
    }
    Some(s)
}

/// Units for operators with custom arity (structured control and calls).
fn custom_unit(op: &OpInfo) -> Option<String> {
    Some(
        match op.visit {
            "visit_unreachable" => "i32.const 0 if unreachable end",
            "visit_block" | "visit_end" => "block end",
            "visit_loop" => "loop end",
            "visit_if" => "i32.const 0 if end",
            "visit_else" => "i32.const 0 if else end",
            "visit_br" => "block br 0 end",
            "visit_br_if" => "block i32.const 0 br_if 0 end",
            "visit_br_table" => "block i32.const 0 br_table 0 0 end",
            "visit_return" => "i32.const 0 if return end",
            "visit_call" => "call $f0",
            "visit_call_indirect" => "i32.const 0 call_indirect $t (type $ty)",
            "visit_return_call" => "i32.const 0 if return_call $f0 end",
            "visit_return_call_indirect" => {
                "i32.const 0 if i32.const 0 return_call_indirect $t (type $ty) end"
            }
            "visit_typed_select_multi" => {
                "i32.const 0 i64.const 0 i32.const 0 i64.const 0 i32.const 0 \
                 select (result i32 i64) drop drop"
            }
            _ => return None,
        }
        .to_string(),
    )
}

fn validates(wat: &str, features: WasmFeatures) -> Option<Vec<u8>> {
    let bin = wat::parse_str(wat).ok()?;
    Validator::new_with_features(features)
        .validate_all(&bin)
        .ok()?;
    Some(bin)
}

/// A unit for `op` that validates under `features`, as WAT, by searching
/// operand types (all equal first, then every combination).
fn unit_for(op: &OpInfo, features: WasmFeatures) -> Option<String> {
    if let Some(u) = custom_unit(op) {
        return validates(&template(&u), features).map(|_| u);
    }
    let (params, results) = parse_arity(op.arity)?;
    let m = mnemonic(op.visit);
    let n = TYPES.len();
    let mut combos: Vec<Vec<usize>> = (0..n).map(|t| vec![t; params]).collect();
    let total = n.pow(params as u32);
    for mut k in 0..total {
        let mut c = Vec::with_capacity(params);
        for _ in 0..params {
            c.push(k % n);
            k /= n;
        }
        combos.push(c);
    }
    for combo in combos {
        for (rty, _) in TYPES {
            let imm = immediates(op, rty)?;
            let mut u = String::new();
            for &t in &combo {
                u.push_str(TYPES[t].1);
                u.push(' ');
            }
            u.push_str(&m);
            u.push_str(&imm);
            for _ in 0..results {
                u.push_str(" drop");
            }
            if validates(&template(&u), features).is_some() {
                return Some(u);
            }
            if !op.args.contains(&"ty") {
                break;
            }
        }
    }
    None
}

fn parse_arity(a: &str) -> Option<(usize, usize)> {
    // "arity 2 -> 1"
    let mut it = a.split_whitespace();
    (it.next()? == "arity").then_some(())?;
    let p = it.next()?.parse().ok()?;
    (it.next()? == "->").then_some(())?;
    Some((p, it.next()?.parse().ok()?))
}

fn leb(mut v: u64, out: &mut Vec<u8>) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn read_leb(b: &[u8], pos: &mut usize) -> u64 {
    let (mut v, mut shift) = (0u64, 0);
    loop {
        let x = b[*pos];
        *pos += 1;
        v |= u64::from(x & 0x7f) << shift;
        if x & 0x80 == 0 {
            return v;
        }
        shift += 7;
    }
}

/// The compiled template split around the unit function's body.
struct Template {
    bin: Vec<u8>,
    /// Locals declaration of the unit function (count and entries).
    locals: Vec<u8>,
    /// The unit's operator bytes (without the final `end`).
    unit: Vec<u8>,
    /// Number of operators in the unit.
    unit_ops: u64,
}

impl Template {
    fn new(unit_wat: &str) -> Self {
        let bin = wat::parse_str(template(unit_wat)).unwrap();
        let mut i = 0;
        let mut found = None;
        for p in Parser::new(0).parse_all(&bin) {
            if let Payload::CodeSectionEntry(body) = p.unwrap() {
                if i == SWEEP_FUNC {
                    found = Some(body);
                }
                i += 1;
            }
        }
        let body = found.unwrap();
        let r = body.range();
        let ops = body.get_binary_reader_for_operators().unwrap();
        let ops_start = ops.original_position();
        let mut reader = body.get_operators_reader().unwrap();
        let mut unit_ops = 0;
        while !reader.eof() {
            reader.read().unwrap();
            unit_ops += 1;
        }
        Self {
            locals: bin[r.start as usize..ops_start as usize].to_vec(),
            unit: bin[ops_start as usize..r.end as usize - 1].to_vec(),
            unit_ops: unit_ops - 1,
            bin,
        }
    }

    /// The module with `funcs` copies of the unit function, each holding
    /// `k` units on top of `deep` stacked `i32.const 0`.
    /// With `looped`, the units sit in a `loop` that branches back.
    fn build(&self, k: u64, funcs: u64, deep: u64, looped: bool) -> Vec<u8> {
        let mut body = self.locals.clone();
        for _ in 0..deep {
            body.extend_from_slice(&[0x41, 0x00]);
        }
        if looped {
            // loop (empty block type)
            body.extend_from_slice(&[0x03, 0x40]);
        }
        for _ in 0..k {
            body.extend_from_slice(&self.unit);
        }
        if looped {
            // i32.const 0, br_if 0, end
            body.extend_from_slice(&[0x41, 0x00, 0x0d, 0x00, 0x0b]);
        }
        body.extend(std::iter::repeat_n(0x1a, deep as usize));
        body.push(0x0b);
        let b = &self.bin;
        let mut out = b[..8].to_vec();
        let mut pos = 8;
        while pos < b.len() {
            let id = b[pos];
            pos += 1;
            let size = read_leb(b, &mut pos) as usize;
            let payload = &b[pos..pos + size];
            pos += size;
            let new = match id {
                3 => {
                    // Function section: append copies of the unit function's type.
                    let mut p = 0;
                    let count = read_leb(payload, &mut p);
                    let mut types = Vec::new();
                    for _ in 0..count {
                        types.push(read_leb(payload, &mut p));
                    }
                    let ty = types[SWEEP_FUNC];
                    let mut n = Vec::new();
                    leb(count + funcs - 1, &mut n);
                    for t in types
                        .iter()
                        .chain(std::iter::repeat_n(&ty, funcs as usize - 1))
                    {
                        leb(*t, &mut n);
                    }
                    n
                }
                10 => {
                    // Code section: replace the unit body, append copies.
                    let mut p = 0;
                    let count = read_leb(payload, &mut p);
                    let mut n = Vec::new();
                    leb(count + funcs - 1, &mut n);
                    for i in 0..count as usize {
                        let len = read_leb(payload, &mut p) as usize;
                        if i == SWEEP_FUNC {
                            for _ in 0..funcs {
                                leb(body.len() as u64, &mut n);
                                n.extend_from_slice(&body);
                            }
                        } else {
                            leb(len as u64, &mut n);
                            n.extend_from_slice(&payload[p..p + len]);
                        }
                        p += len;
                    }
                    n
                }
                _ => payload.to_vec(),
            };
            out.push(id);
            leb(new.len() as u64, &mut out);
            out.extend_from_slice(&new);
        }
        out
    }

    fn build_v(&self, k: u64, v: &Variant) -> Vec<u8> {
        self.build(k, v.funcs, v.deep, v.looped)
    }

    /// The largest `k` whose module `check_limits` accepts.
    fn max_units(&self, v: &Variant) -> u64 {
        let ok = |k| check_limits(&self.build_v(k, v)).is_ok();
        if let Err(e) = check_limits(&self.build_v(0, v)) {
            panic!("the empty module must be accepted: {e:?}");
        }
        let mut hi = (MAX_OPERATORS_PER_FUNCTION / self.unit_ops.max(1)) + 1;
        if ok(hi) {
            return hi;
        }
        let mut lo = 0;
        while hi - lo > 1 {
            let mid = (lo + hi) / 2;
            if ok(mid) { lo = mid } else { hi = mid }
        }
        lo
    }
}

/// One sweep case: an operator and a variant.
struct Case {
    op: OpInfo,
    unit: String,
    v: Variant,
}

/// Where the units go: how many copies of the unit function, how deep a
/// stack they run on, and whether they sit in a loop.
#[derive(Clone, Copy)]
struct Variant {
    name: &'static str,
    funcs: u64,
    deep: u64,
    looped: bool,
}

fn enabled_features() -> WasmFeatures {
    WasmFeatures::WASM1
        | WasmFeatures::SATURATING_FLOAT_TO_INT
        | WasmFeatures::SIGN_EXTENSION
        | WasmFeatures::MULTI_VALUE
        | WasmFeatures::BULK_MEMORY
        | WasmFeatures::REFERENCE_TYPES
}

/// Every operator of an enabled proposal with a unit that validates, and
/// the operators that have none (with the reason).
fn units() -> (Vec<(OpInfo, String)>, Vec<(OpInfo, &'static str)>) {
    let (mut ok, mut none) = (Vec::new(), Vec::new());
    for op in all_ops() {
        if !ENABLED.contains(&op.proposal) {
            continue;
        }
        match unit_for(&op, enabled_features()) {
            Some(u) => ok.push((op, u)),
            None => none.push((op, "no valid use under the enabled proposals")),
        }
    }
    (ok, none)
}

fn cases() -> Vec<Case> {
    let max_funcs = u64::from(MAX_FUNCTIONS) - OTHER_FUNCS;
    let mut out = Vec::new();
    for (op, unit) in units().0 {
        let v = |name, funcs, deep, looped| Variant {
            name,
            funcs,
            deep,
            looped,
        };
        for v in [
            v("1 fn", 1, 0, false),
            v("1 fn, in a loop", 1, 0, true),
            v("max fns", max_funcs, 0, false),
            v("1 fn, deep stack", 1, DEEP, false),
        ] {
            out.push(Case {
                op: op.clone(),
                unit: unit.clone(),
                v,
            });
        }
    }
    out
}

#[test]
fn every_enabled_operator_is_covered() {
    let (ok, none) = units();
    // Only the multi-type `select` encoding has no valid use without GC.
    let names: Vec<_> = none.iter().map(|(o, _)| o.name).collect();
    assert_eq!(
        names,
        ["TypedSelectMulti"],
        "operators without a unit: {none:?}"
    );
    assert!(ok.len() > 190, "{} operators", ok.len());
}

#[test]
fn disabled_proposals_are_rejected() {
    let (mut checked, mut skipped) = (0, 0);
    for op in all_ops() {
        if ENABLED.contains(&op.proposal) {
            continue;
        }
        let Some(u) = unit_for(&op, WasmFeatures::all()) else {
            skipped += 1;
            continue;
        };
        let bin = wat::parse_str(template(&u)).unwrap();
        let r = check_limits(&bin);
        assert!(
            matches!(r, Err(ScriptError::Load(_))),
            "{} ({}) must be rejected: {r:?}",
            op.name,
            op.proposal
        );
        checked += 1;
    }
    // TypedSelectMulti (enabled proposal, no valid use) too.
    let multi = template(
        &custom_unit(
            &all_ops()
                .into_iter()
                .find(|o| o.name == "TypedSelectMulti")
                .unwrap(),
        )
        .unwrap(),
    );
    assert!(check_limits(&wat::parse_str(multi).unwrap()).is_err());
    println!("disabled operators rejected: {checked}, not encodable by the generator: {skipped}");
    // Best effort: the generator cannot encode every disabled operator
    // (GC, exceptions, stack switching need types it does not declare),
    // but the bulk of them (SIMD, threads, tail calls, wide arithmetic).
    assert!(checked > 250, "only {checked} disabled operators checked");
}

/// Every at-limit sweep module is accepted by `check_limits`, and one more
/// unit is rejected (unless the operator cap per function is reached
/// first). Compiling them is `operator_sweep_report`'s job (release).
#[test]
fn every_operator_sits_exactly_at_the_limits() {
    let cases = cases();
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    std::thread::scope(|scope| {
        for chunk in cases.chunks(cases.len().div_ceil(threads)) {
            scope.spawn(move || {
                for c in chunk {
                    let t = Template::new(&c.unit);
                    let k = t.max_units(&c.v);
                    let at = t.build_v(k, &c.v);
                    assert!(check_limits(&at).is_ok(), "{} {}", c.op.name, c.v.name);
                    let over = t.build_v(k + 1, &c.v);
                    assert!(
                        matches!(check_limits(&over), Err(ScriptError::Load(_))),
                        "{} {}: one more unit must be rejected",
                        c.op.name,
                        c.v.name
                    );
                }
            });
        }
    });
}

/// Peak resident set of this process since the last reset, bytes (Linux).
fn peak_rss() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// Compiles the at-limit module of case `PIN_SWEEP_CASE` (all cases if
/// unset) with `prepare` and prints time and peak RSS (never asserts on
/// time: machine-dependent). `PIN_SWEEP_COUNT=1` only prints the count.
#[test]
#[ignore = "prints release compile cost for the report"]
fn operator_sweep_report() {
    let cases = cases();
    if std::env::var_os("PIN_SWEEP_COUNT").is_some() {
        println!("CASES {}", cases.len());
        return;
    }
    let only: Option<usize> = std::env::var("PIN_SWEEP_CASE")
        .ok()
        .and_then(|v| v.parse().ok());
    for (i, c) in cases.iter().enumerate() {
        if only.is_some_and(|o| o != i) {
            continue;
        }
        let t = Template::new(&c.unit);
        let k = t.max_units(&c.v);
        let bin = t.build_v(k, &c.v);
        let _ = std::fs::write("/proc/self/clear_refs", "5");
        let start = std::time::Instant::now();
        let r = WasmScriptBackend::prepare(&bin, WasmConfig::engine());
        let el = start.elapsed();
        let rss = peak_rss().map_or("n/a".into(), |b| format!("{} MB", b >> 20));
        assert!(r.is_ok(), "{} {}: {:?}", c.op.name, c.v.name, r.err());
        println!(
            "SWEEP | {i} | {} | {} | {k} x {} ops | {} | {:.3?} | {rss} |",
            c.op.name,
            c.v.name,
            t.unit_ops,
            bin.len(),
            el
        );
    }
}
