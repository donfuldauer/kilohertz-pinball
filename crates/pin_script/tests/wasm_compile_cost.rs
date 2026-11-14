//! Subphase 4.5 (fix round 2, B1): the pre-compile caps bound compile cost.
//!
//! For every cap there is a module just over it (rejected, with a message
//! naming the limit) and a worst-case module at it (accepted); the worst
//! shapes are also combined into one module that sits at every module-wide
//! budget at once. `compile_cost_report` prints the release compile time and
//! peak RSS of each worst case, one process per case:
//!
//! ```sh
//! for i in $(seq 0 21); do PIN_COST_CASE=$i cargo test -p pin_script --release \
//!   --test wasm_compile_cost -- --ignored --nocapture --test-threads=1 compile_cost_report; done
//! ```

use pin_core::ScriptError;
use pin_script::wasm::{
    MAX_BLOCKS, MAX_BR_TABLE_TARGETS, MAX_BR_TABLE_TARGETS_PER_FUNCTION, MAX_BRANCH_WORK,
    MAX_ELEMENT_ITEMS, MAX_EXPORTS, MAX_FUNCTIONS, MAX_GLOBALS, MAX_IMPORTS,
    MAX_LOCALS_PER_FUNCTION, MAX_LOOP_NESTING, MAX_LOOP_WORK, MAX_MERGE_WORK, MAX_NESTING,
    MAX_OPERATOR_COST, MAX_OPERATORS, MAX_OPERATORS_PER_FUNCTION, MAX_PARAMS, MAX_PRESSURE_WORK,
    MAX_RESULTS, MAX_SEGMENTS, MAX_TOTAL_LOCALS, MAX_TOTAL_TABLE_ELEMENTS, MAX_TYPES, WasmConfig,
    WasmScriptBackend, check_limits,
};
use std::fmt::Write as _;

/// The required exports: three functions of one operator (`end`) each, two
/// types and four exports.
const HEAD: &str = r#"(memory (export "memory") 1)
  (func (export "on_init")) (func (export "on_tick"))
  (func (export "on_switch_edge") (param i32 i32))"#;
const HEAD_FUNCS: u64 = 3;
const HEAD_TYPES: u64 = 2;
const HEAD_EXPORTS: u64 = 4;

fn module(extra: &str) -> Vec<u8> {
    module_with_prefix("", extra)
}

/// `prefix` goes before the required exports (imports must come first).
fn module_with_prefix(prefix: &str, extra: &str) -> Vec<u8> {
    wat::parse_str(format!("(module {prefix} {HEAD} {extra})")).expect("generator emits valid WAT")
}

fn locals_decl(n: u64) -> String {
    let mut s = String::new();
    if n > 0 {
        s.push_str(" (local");
        for _ in 0..n {
            s.push_str(" i32");
        }
        s.push(')');
    }
    s
}

/// Fills `left` operators with local updates (4 operators each) or, without
/// locals, `i32.const`/`drop` pairs, then `nop`s.
fn fill(s: &mut String, locals: u64, mut left: u64) {
    let mut i = 0u64;
    if locals > 0 {
        while left >= 4 {
            let (a, b) = (i % locals, (i * 7 + 1) % locals);
            writeln!(s, "local.get {a} local.get {b} i32.add local.set {a}").unwrap();
            left -= 4;
            i += 1;
        }
    } else {
        while left >= 2 {
            s.push_str("i32.const 1 drop\n");
            left -= 2;
        }
    }
    for _ in 0..left {
        s.push_str("nop\n");
    }
}

fn cond(locals: u64, k: u64) -> String {
    if locals > 0 {
        format!("local.get {}", k % locals)
    } else {
        "i32.const 0".into()
    }
}

/// One function: `locals` i32 locals, `depth` nested `kind`s (`loop` or
/// `block`; one `if` around each `br_table` counts towards the depth),
/// `br_tables` `br_table`s of `targets` targets each (default included)
/// spread over every enclosing label, and local updates in the innermost
/// construct filling exactly `ops` operators (`end` included).
///
/// Cost metrics: blocks = `depth` (+ `br_tables` ifs); branches = blocks +
/// `br_if`s (one per nested construct); targets = `br_tables` × `targets`.
fn nested_fn(
    kind: &str,
    locals: u64,
    depth: u64,
    br_tables: u64,
    targets: u64,
    ops: u64,
) -> String {
    let nest = if br_tables > 0 { depth - 1 } else { depth };
    let fixed = nest * 4 + br_tables * 5 + 1;
    assert!(
        ops >= fixed,
        "{ops} operators cannot hold the shape ({fixed})"
    );
    let mut s = format!("(func{}\n", locals_decl(locals));
    for _ in 0..nest {
        writeln!(s, "{kind}").unwrap();
    }
    for j in 0..br_tables {
        let sel = cond(locals, j);
        write!(s, "{sel} if {sel} br_table").unwrap();
        for t in 0..targets {
            write!(s, " {}", (t + j) % (nest + 1)).unwrap();
        }
        s.push_str(" end\n");
    }
    fill(&mut s, locals, ops - fixed);
    for k in 0..nest {
        writeln!(s, "{} br_if 0 end", cond(locals, k)).unwrap();
    }
    s.push_str(")\n");
    s
}

/// One function of `loops` sequential loops (each conditionally looping
/// back), local updates spread over them, exactly `ops` operators.
///
/// Cost metrics: blocks = `loops`; branches = 2 × `loops`.
fn seq_fn(locals: u64, loops: u64, ops: u64) -> String {
    let fixed = loops * 4 + 1;
    assert!(ops >= fixed, "{ops} operators cannot hold {loops} loops");
    let mut s = format!("(func{}\n", locals_decl(locals));
    let per = (ops - fixed) / loops.max(1);
    for k in 0..loops {
        s.push_str("loop\n");
        fill(&mut s, locals, per);
        writeln!(s, "{} br_if 0 end", cond(locals, k)).unwrap();
    }
    fill(&mut s, locals, ops - fixed - per * loops);
    s.push_str(")\n");
    s
}

/// `n` distinct function types of `MAX_PARAMS` params (i32/i64/f32/f64
/// chosen by the bits of the index) and `MAX_RESULTS` i32 results, one
/// function `$t<i>` of each (`MAX_RESULTS` + 1 operators); the first
/// `exported` are exported.
fn typed_fns(n: u64, exported: u64) -> String {
    const TYS: [&str; 4] = ["i32", "i64", "f32", "f64"];
    let mut s = String::new();
    for i in 0..n {
        write!(s, "(func $t{i}").unwrap();
        if i < exported {
            write!(s, " (export \"t{i}\")").unwrap();
        }
        s.push_str(" (param");
        for p in 0..u64::from(MAX_PARAMS) {
            let digit = if p < 6 { (i >> (2 * p)) & 3 } else { 0 };
            write!(s, " {}", TYS[digit as usize]).unwrap();
        }
        s.push_str(") (result");
        for _ in 0..MAX_RESULTS {
            s.push_str(" i32");
        }
        s.push(')');
        for _ in 0..MAX_RESULTS {
            s.push_str(" i32.const 0");
        }
        s.push_str(")\n");
    }
    s
}

/// `items` active `ref.func` expressions over `$t0..$t<types>`, into a
/// table of their own.
fn elem_exprs(items: u64, types: u64) -> String {
    let mut s = format!("(table $tab {items} funcref) (elem (table $tab) (i32.const 0) funcref");
    for i in 0..items {
        write!(s, " (ref.func $t{})", i % types).unwrap();
    }
    s.push_str(")\n");
    s
}

fn imports_at_cap() -> String {
    "(import \"pinball\" \"tick\" (func (result i64)))\n".repeat(MAX_IMPORTS as usize)
}

fn globals_data_at_cap() -> String {
    let mut s = "(global (mut i64) (i64.const 1))".repeat(MAX_GLOBALS as usize);
    s.push_str(&"(data (i32.const 0) \"x\")".repeat(MAX_SEGMENTS as usize));
    s
}

type Build = Box<dyn Fn(u64) -> Vec<u8>>;

/// A worst case: `build(p)` for the largest `p` in `lo..=hi` that the
/// limits accept (each case's `p` is the knob that spends its budget).
struct Case {
    name: &'static str,
    lo: u64,
    hi: u64,
    build: Build,
}

fn case(name: &'static str, lo: u64, hi: u64, build: impl Fn(u64) -> Vec<u8> + 'static) -> Case {
    Case {
        name,
        lo,
        hi,
        build: Box::new(build),
    }
}

/// The largest `p` in `lo..=hi` whose module the limits accept.
fn max_param(lo: u64, hi: u64, build: &dyn Fn(u64) -> Vec<u8>) -> u64 {
    if let Err(e) = check_limits(&build(lo)) {
        panic!("the smallest module must be accepted: {e:?}");
    }
    if check_limits(&build(hi)).is_ok() {
        return hi;
    }
    let (mut lo, mut hi) = (lo, hi);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if check_limits(&build(mid)).is_ok() {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// Locals of the max-locals shapes, leaving room for the operand stack (2
/// deep in an update), which counts as live too.
const L: u64 = MAX_LOCALS_PER_FUNCTION - 3;

/// Every worst case at the caps. Shapes are fixed; the knob is pushed to
/// the limit by [`max_param`].
fn worst_cases() -> Vec<Case> {
    let types = u64::from(MAX_TYPES) - HEAD_TYPES;
    let exported = u64::from(MAX_EXPORTS) - HEAD_EXPORTS;
    let fns = u64::from(MAX_FUNCTIONS) - HEAD_FUNCS;
    let opf = MAX_OPERATORS_PER_FUNCTION;
    let tgt = MAX_BR_TABLE_TARGETS;
    vec![
        case(
            "max locals x max loop nesting; ops to the pressure cap",
            200,
            opf,
            |p| module(&nested_fn("loop", L, MAX_LOOP_NESTING, 0, 0, p)),
        ),
        case("same, a call in every update", 200, opf, |p| {
            module(
                &nested_fn("loop", L, MAX_LOOP_NESTING, 0, 0, p)
                    .replace(" i32.add", " i32.add call 0"),
            )
        }),
        case(
            "max block nesting; locals to the merge cap",
            1,
            MAX_LOCALS_PER_FUNCTION,
            |p| module(&nested_fn("block", p, MAX_NESTING, 0, 0, 30_000)),
        ),
        case(
            "max br_table targets into 15 nested loops; locals to the merge cap",
            1,
            1024,
            move |p| module(&nested_fn("loop", p, 16, 1, tgt, 20_000)),
        ),
        case(
            "max br_table targets in 32 locals' nested blocks; depth to the branch cap",
            2,
            MAX_NESTING,
            move |p| module(&nested_fn("block", 32, p, 1, tgt, opf)),
        ),
        case(
            "sequential loops in one function; loops to the branch cap",
            10,
            4000,
            |p| module(&seq_fn(30, p, 20_000)),
        ),
        case(
            "max locals, straight line; ops to the pressure cap",
            10,
            opf,
            |p| module(&nested_fn("block", L, 1, 0, 0, p)),
        ),
        case(
            "14 locals updated in one loop; ops to the loop-work cap",
            100,
            opf,
            |p| module(&nested_fn("loop", 14, 1, 0, 0, p)),
        ),
        case(
            "4 fns x 14 locals updated in one loop; ops to the loop-work cap",
            100,
            opf,
            |p| module(&nested_fn("loop", 14, 1, 0, 0, p).repeat(4)),
        ),
        case(
            "63 locals updated in one loop; ops to the loop-work cap",
            100,
            opf,
            |p| module(&nested_fn("loop", 63, 1, 0, 0, p)),
        ),
        case(
            "126 locals in 4 loops; ops to the pressure cap",
            100,
            opf,
            |p| module(&nested_fn("loop", 126, 4, 0, 0, p)),
        ),
        case(
            "max functions; loops to the block cap",
            0,
            2 * MAX_BLOCKS,
            move |p| {
                let (base, rest) = (p / fns, p % fns);
                let mut m = String::new();
                for i in 0..fns {
                    m.push_str(&seq_fn(0, base + u64::from(i < rest), 60));
                }
                module(&m)
            },
        ),
        case(
            "max functions x 8 locals; ops to the operator cap",
            10,
            1000,
            move |p| module(&nested_fn("block", 8, 1, 0, 0, p).repeat(fns as usize)),
        ),
        case(
            "max-size functions; ops to the operator cap",
            100,
            opf,
            move |p| {
                let n = MAX_OPERATORS / opf - 1;
                let mut m = nested_fn("block", 4, 8, 0, 0, opf).repeat(n as usize);
                m.push_str(&nested_fn("block", 4, 8, 0, 0, p));
                module(&m)
            },
        ),
        case("calls; to the operator-cost cap", 1, opf - 1, |p| {
            module(&format!("(func {})", "call 0 ".repeat(p as usize)))
        }),
        case(
            "call_indirect; to the operator-cost and branch caps",
            1,
            opf - 1,
            |p| {
                module(&format!(
                    "(table 1 funcref) (func {})",
                    "i32.const 0 call_indirect ".repeat(p as usize)
                ))
            },
        ),
        case(
            "max types x params x results, max exports",
            1,
            1,
            move |_| module(&typed_fns(types, exported)),
        ),
        case(
            "element items over max types; to the item cap",
            1,
            2 * MAX_ELEMENT_ITEMS,
            move |p| module(&(typed_fns(types, exported) + &elem_exprs(p, types))),
        ),
        case("imports, globals, tables, data at caps", 1, 1, |_| {
            let half = MAX_TOTAL_TABLE_ELEMENTS / 2;
            let t = format!("(table {half} funcref) (table {half} funcref)");
            module_with_prefix(&imports_at_cap(), &(t + &globals_data_at_cap()))
        }),
        case(
            "max locals spread over max functions; ops to the pressure cap",
            1,
            200,
            move |p| {
                let per = (MAX_TOTAL_LOCALS / fns).min(MAX_LOCALS_PER_FUNCTION);
                module(&nested_fn("block", per, 0, 0, 0, p).repeat(fns as usize))
            },
        ),
        case("everything at once (nested loops)", 1, 1, |_| {
            combined(false)
        }),
        case("everything at once (br_table into loops)", 1, 1, |_| {
            combined(true)
        }),
    ]
}

/// One module at every budget at once, built greedily: module-level parts
/// at their caps (imports, typed and exported functions, element items,
/// tables, globals, data), then functions that each push one budget to
/// what is left of it: the merge budget (max locals in max nested loops,
/// or a `br_table` into nested loops plus max locals in straight-line
/// code), the branch budget (sequential loops), the operator-cost budget
/// (`call_indirect`, then calls), and the function and block counts (small
/// looping functions).
fn combined(br_table: bool) -> Vec<u8> {
    // The imports add one type (`() -> i64`).
    let types = u64::from(MAX_TYPES) - HEAD_TYPES - 1;
    let exported = u64::from(MAX_EXPORTS) - HEAD_EXPORTS;
    let mut fixed = typed_fns(types, exported) + &elem_exprs(MAX_ELEMENT_ITEMS, types);
    let rest_tables = MAX_TOTAL_TABLE_ELEMENTS - MAX_ELEMENT_ITEMS;
    let big = rest_tables.min(65_536);
    fixed += &format!(
        "(table {big} funcref) (table {} funcref)",
        rest_tables - big
    );
    fixed += &globals_data_at_cap();
    let prefix = imports_at_cap();
    // `on_init`, after the imports.
    let on_init = MAX_IMPORTS;
    type Part = Box<dyn Fn(u64) -> String>;
    let mut parts: Vec<(u64, u64, Part)> = Vec::new();
    let opf = MAX_OPERATORS_PER_FUNCTION;
    if br_table {
        parts.push((
            1,
            1024,
            Box::new(|p| nested_fn("loop", p, 16, 1, MAX_BR_TABLE_TARGETS, 20_000)),
        ));
        parts.push((10, opf, Box::new(|p| nested_fn("block", L, 0, 0, 0, p))));
    } else {
        parts.push((
            200,
            opf,
            Box::new(|p| nested_fn("loop", L, MAX_LOOP_NESTING, 0, 0, p)),
        ));
    }
    parts.push((1, 4000, Box::new(|p| seq_fn(0, p, 4 * p + 1))));
    parts.push((
        0,
        opf - 1,
        Box::new(|p| {
            format!(
                "(func {})",
                "i32.const 0 call_indirect $tab ".repeat(p as usize)
            )
        }),
    ));
    parts.push((
        0,
        opf - 1,
        Box::new(move |p| format!("(func {})", format!("call {on_init} ").repeat(p as usize))),
    ));
    parts.push((
        0,
        u64::from(MAX_FUNCTIONS),
        Box::new(|p| seq_fn(0, 2, 40).repeat(p as usize)),
    ));
    let mut acc = fixed;
    for (lo, hi, part) in parts {
        let build = |p| module_with_prefix(&prefix, &(acc.clone() + &part(p)));
        let p = max_param(lo, hi, &build);
        acc += &part(p);
    }
    module_with_prefix(&prefix, &acc)
}

fn prepare(bin: &[u8]) -> Result<(), ScriptError> {
    WasmScriptBackend::prepare(bin, WasmConfig::engine()).map(drop)
}

/// Every worst case sits exactly at its limit (one step more is rejected)
/// and compiles.
#[test]
fn worst_case_modules_at_the_caps_are_accepted() {
    for c in worst_cases() {
        let p = max_param(c.lo, c.hi, &c.build);
        let bin = (c.build)(p);
        if p < c.hi {
            assert!(
                matches!(check_limits(&(c.build)(p + 1)), Err(ScriptError::Load(_))),
                "{}: one step more must be rejected",
                c.name
            );
        }
        if let Err(e) = prepare(&bin) {
            panic!("{} must be accepted: {e:?}", c.name);
        }
    }
}

fn rejected(bin: &[u8], what: &str, max: u64) {
    match prepare(bin) {
        Err(ScriptError::Load(m)) => assert!(
            m.contains(what) && m.contains(&format!("(at most {max})")),
            "expected the {what} limit ({max}), got {m}"
        ),
        other => panic!("expected the {what} limit to reject, got {other:?}"),
    }
}

#[test]
fn each_cap_rejects_one_over() {
    let l = MAX_LOCALS_PER_FUNCTION;
    rejected(
        &module(&nested_fn("block", l + 1, 1, 0, 0, 16)),
        "locals",
        l,
    );
    // Module-wide locals: many small functions (cheap on every budget).
    let n = MAX_TOTAL_LOCALS / 64 + 1;
    rejected(
        &module(&nested_fn("block", 64, 0, 0, 0, 1).repeat(n as usize)),
        "locals",
        MAX_TOTAL_LOCALS,
    );
    rejected(
        &module(&nested_fn("block", 0, MAX_NESTING + 1, 0, 0, 4096)),
        "nested blocks",
        MAX_NESTING,
    );
    rejected(
        &module(&nested_fn("loop", 0, MAX_LOOP_NESTING + 1, 0, 0, 1024)),
        "nested loops",
        MAX_LOOP_NESTING,
    );
    rejected(
        &module(&nested_fn("block", 0, 4, 1, MAX_BR_TABLE_TARGETS + 1, 4096)),
        "targets",
        MAX_BR_TABLE_TARGETS,
    );
    let n = MAX_BR_TABLE_TARGETS_PER_FUNCTION / MAX_BR_TABLE_TARGETS + 1;
    rejected(
        &module(&nested_fn("block", 0, 4, n, MAX_BR_TABLE_TARGETS, 20_000)),
        "br_table targets",
        MAX_BR_TABLE_TARGETS_PER_FUNCTION,
    );
    rejected(
        &module(&nested_fn(
            "block",
            0,
            1,
            0,
            0,
            MAX_OPERATORS_PER_FUNCTION + 1,
        )),
        "operators",
        MAX_OPERATORS_PER_FUNCTION,
    );
    // Module-wide operators: full functions, plus the head, are over.
    let full = MAX_OPERATORS / MAX_OPERATORS_PER_FUNCTION;
    let m = nested_fn("block", 0, 1, 0, 0, MAX_OPERATORS_PER_FUNCTION).repeat(full as usize);
    rejected(&module(&m), "operators", MAX_OPERATORS);
    // Module-wide blocks, in functions small enough for the branch budget.
    let per = 64;
    let mut m = seq_fn(0, per, per * 4 + 1).repeat((MAX_BLOCKS / per) as usize);
    m.push_str(&seq_fn(0, 1, 5));
    rejected(&module(&m), "blocks, loops and ifs", MAX_BLOCKS);
    // Merge work: max locals in max nested loops is exactly the cap; one
    // more branch goes over.
    let f = nested_fn("loop", l, MAX_LOOP_NESTING, 0, 0, 4096).replacen(
        "loop\n",
        "loop\nlocal.get 0 br_if 0\n",
        1,
    );
    rejected(&module(&f), "live values x branch edges", MAX_MERGE_WORK);
    // Branch work: b sequential loops are 2b branches.
    let b = (MAX_BRANCH_WORK as f64).sqrt() as u64 / 2 + 1;
    rejected(
        &module(&seq_fn(0, b, b * 4 + 1)),
        "branch edges squared",
        MAX_BRANCH_WORK,
    );
    let ops = MAX_PRESSURE_WORK / l - l + 1;
    rejected(
        &module(&nested_fn("block", l, 1, 0, 0, ops)),
        "live values x operators",
        MAX_PRESSURE_WORK,
    );
    let n = u64::from(MAX_FUNCTIONS) - HEAD_FUNCS + 1;
    rejected(
        &module(&"(func)".repeat(n as usize)),
        "functions",
        MAX_FUNCTIONS.into(),
    );
    let mut t = String::new();
    for i in 0..u64::from(MAX_TYPES) - HEAD_TYPES + 1 {
        t.push_str("(type (func (param");
        for b in 0..9 {
            t.push_str(if i >> b & 1 == 1 { " i64" } else { " i32" });
        }
        t.push_str(")))");
    }
    rejected(&module(&t), "types", MAX_TYPES.into());
    let p = " i32".repeat(MAX_PARAMS as usize + 1);
    rejected(
        &module(&format!("(type (func (param{p})))")),
        "params",
        MAX_PARAMS.into(),
    );
    let r = " i32".repeat(MAX_RESULTS as usize + 1);
    rejected(
        &module(&format!("(type (func (result{r})))")),
        "results",
        MAX_RESULTS.into(),
    );
    let items = " 0".repeat(MAX_ELEMENT_ITEMS as usize + 1);
    rejected(
        &module(&format!("(elem func{items})")),
        "element segment items",
        MAX_ELEMENT_ITEMS,
    );
    let mut ex = String::new();
    for i in 0..u64::from(MAX_EXPORTS) - HEAD_EXPORTS + 1 {
        write!(ex, "(export \"e{i}\" (func 0))").unwrap();
    }
    rejected(&module(&ex), "exports", MAX_EXPORTS.into());
    let imports =
        "(import \"pinball\" \"tick\" (func (result i64)))".repeat(MAX_IMPORTS as usize + 1);
    rejected(
        &module_with_prefix(&imports, ""),
        "imports",
        MAX_IMPORTS.into(),
    );
    let calls = MAX_OPERATOR_COST / 8 + 1;
    rejected(
        &module(&format!("(func {})", "call 0 ".repeat(calls as usize))),
        "operator cost units",
        MAX_OPERATOR_COST,
    );
}

/// Peak resident set of this process since the last reset, bytes (Linux).
fn peak_rss() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// Prints compile time and peak RSS for the worst cases (never asserts:
/// timings are machine-dependent). `PIN_COST_CASE=<i>` runs only case `i`
/// (one process per case keeps the peak-RSS figures separate).
#[test]
#[ignore = "prints release compile cost for the report"]
fn compile_cost_report() {
    let only: Option<usize> = std::env::var("PIN_COST_CASE")
        .ok()
        .and_then(|v| v.parse().ok());
    for (i, c) in worst_cases().into_iter().enumerate() {
        let name = c.name;
        if only.is_some_and(|o| o != i) {
            continue;
        }
        let p = max_param(c.lo, c.hi, &c.build);
        let bin = (c.build)(p);
        // Reset the peak-RSS counter to the current RSS.
        let _ = std::fs::write("/proc/self/clear_refs", "5");
        let start = std::time::Instant::now();
        let r = prepare(&bin);
        let t = start.elapsed();
        let rss = peak_rss().map_or("n/a".into(), |b| format!("{} MB", b >> 20));
        let verdict = match r {
            Ok(()) => String::new(),
            Err(e) => format!(" REJECTED {e:?}"),
        };
        println!(
            "| {i} | {name} | {} | {t:.3?} | {rss} |{verdict}",
            bin.len()
        );
    }
}

/// Prints how much of each limit `rules.wasm` and the modules in
/// `PIN_GUEST_WASM` (`:`-separated paths) use (N9 headroom).
#[test]
#[ignore = "prints budget usage for the report"]
fn guest_budget_usage() {
    let mut paths = vec![
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/wasm/rules.wasm"
        )
        .to_string(),
    ];
    if let Ok(more) = std::env::var("PIN_GUEST_WASM") {
        paths.extend(more.split(':').map(str::to_string));
    }
    for p in paths {
        let bin = std::fs::read(&p).unwrap();
        let u = check_limits(&bin).unwrap_or_else(|e| panic!("{p}: {e:?}"));
        let pct = |n: u64, max: u64| 100.0 * n as f64 / max as f64;
        let rows = [
            ("operators", u.operators, MAX_OPERATORS),
            ("operator cost", u.operator_cost, MAX_OPERATOR_COST),
            ("functions", u.functions, u64::from(MAX_FUNCTIONS)),
            ("blocks", u.blocks, MAX_BLOCKS),
            ("merge work", u.merge_work, MAX_MERGE_WORK),
            ("branch work", u.branch_work, MAX_BRANCH_WORK),
            ("pressure work", u.pressure_work, MAX_PRESSURE_WORK),
            ("loop work", u.loop_work, MAX_LOOP_WORK),
            ("max locals", u.max_locals, MAX_LOCALS_PER_FUNCTION),
            ("max nesting", u.max_nesting, MAX_NESTING),
            ("max loop nesting", u.max_loop_nesting, MAX_LOOP_NESTING),
            (
                "max br_table targets",
                u.max_br_table_targets,
                MAX_BR_TABLE_TARGETS_PER_FUNCTION,
            ),
            ("types", u.types, u64::from(MAX_TYPES)),
            ("imports", u.imports, u64::from(MAX_IMPORTS)),
            ("exports", u.exports, u64::from(MAX_EXPORTS)),
        ];
        println!("USAGE {p} ({} bytes)", bin.len());
        for (name, n, max) in rows {
            println!("USAGE   {name}: {n} / {max} ({:.1} %)", pct(n, max));
        }
        println!("USAGE   max operand stack: {}", u.max_stack);
    }
}

/// The round-3 review's accepted-but-slow modules (76 s, 22 s, 14 s, 5.6 s,
/// 3.9 s, 1.6 s, 1.6 s in release) are rejected without compiling.
#[test]
fn review_round_3_attacks_are_rejected() {
    let f = |n: usize, unit: &str| format!("(func {})", unit.repeat(n));
    let ci = "i32.const 0 call_indirect ";
    let deep = format!(
        "(func {} {} {})",
        "i32.const 0 ".repeat(20_000),
        ci.repeat(12_500),
        "drop ".repeat(20_000)
    );
    let attacks = [
        ("4 x 32 000 call_indirect", f(32_000, ci).repeat(4)),
        (
            "4 x 21 000 table.get",
            f(21_000, "i32.const 0 table.get 0 drop ").repeat(4),
        ),
        ("12 500 call_indirect on 20 000 stacked values", deep),
        ("16 000 call_indirect", f(16_000, ci)),
        (
            "16 000 table.grow",
            f(16_000, "ref.null func i32.const 1 table.grow 0 drop "),
        ),
        (
            "16 000 table.copy",
            f(
                16_000,
                "i32.const 0 i32.const 0 i32.const 0 table.copy 0 0 ",
            ),
        ),
        ("60 x 500 call_indirect", f(500, ci).repeat(60)),
    ];
    for (name, m) in attacks {
        let bin = module(&format!("(table 1 funcref) {m}"));
        assert!(
            matches!(check_limits(&bin), Err(ScriptError::Load(_))),
            "{name} must be rejected"
        );
    }
}
