//! Subphase 4.5 (fix round 2, B1): the pre-compile caps bound compile cost.
//!
//! For every cap there is a module just over it (rejected, with a message
//! naming the limit) and a worst-case module at it (accepted); the worst
//! shapes are also combined into one module that sits at every module-wide
//! budget at once. `compile_cost_report` prints the release compile time and
//! peak RSS of each worst case, one process per case:
//!
//! ```sh
//! for i in $(seq 0 16); do PIN_COST_CASE=$i cargo test -p pin_script --release \
//!   --test wasm_compile_cost -- --ignored --nocapture --test-threads=1 compile_cost_report; done
//! ```

use pin_core::ScriptError;
use pin_script::wasm::{
    MAX_BLOCKS, MAX_BR_TABLE_TARGETS, MAX_BR_TABLE_TARGETS_PER_FUNCTION, MAX_BRANCH_WORK,
    MAX_ELEMENT_ITEMS, MAX_EXPORTS, MAX_FUNCTIONS, MAX_GLOBALS, MAX_IMPORTS,
    MAX_LOCALS_PER_FUNCTION, MAX_LOOP_NESTING, MAX_MERGE_WORK, MAX_MODULE_BYTES, MAX_NESTING,
    MAX_OPERATORS, MAX_OPERATORS_PER_FUNCTION, MAX_PARAMS, MAX_PRESSURE_WORK, MAX_RESULTS,
    MAX_SEGMENTS, MAX_TOTAL_LOCALS, MAX_TOTAL_TABLE_ELEMENTS, MAX_TYPES, WasmConfig,
    WasmScriptBackend,
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
/// Pressure work of the head: `on_switch_edge`, 2 params × (1 op + 2).
const HEAD_PRESSURE: u64 = 6;

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

type Build = Box<dyn Fn() -> Vec<u8>>;

/// Every worst case at the caps, by name, built on demand.
fn worst_cases() -> Vec<(&'static str, Build)> {
    let l = MAX_LOCALS_PER_FUNCTION;
    let types = u64::from(MAX_TYPES) - HEAD_TYPES;
    let exported = u64::from(MAX_EXPORTS) - HEAD_EXPORTS;
    let fns = u64::from(MAX_FUNCTIONS) - HEAD_FUNCS;
    // Max locals in max nested loops spend the whole merge budget
    // (l × 2 × MAX_LOOP_NESTING); these operators the whole pressure budget.
    let deep_ops = (MAX_PRESSURE_WORK - HEAD_PRESSURE) / l - l;
    // b sequential loops: (2b)² = the whole branch budget.
    let seq_loops = (MAX_BRANCH_WORK as f64).sqrt() as u64 / 2;
    let seq_locals = MAX_MERGE_WORK / (2 * seq_loops);
    vec![
        (
            "max locals x max loop nesting (merge + pressure caps)",
            Box::new(move || module(&nested_fn("loop", l, MAX_LOOP_NESTING, 0, 0, deep_ops))),
        ),
        (
            "max locals x max loop nesting, a call in every update",
            Box::new(move || {
                // 5 operators per update instead of 4: same operator count.
                let f = nested_fn("loop", l, MAX_LOOP_NESTING, 0, 0, deep_ops * 4 / 5)
                    .replace(" i32.add", " i32.add call 0");
                module(&f)
            }),
        ),
        (
            "256 locals x max block nesting (merge + pressure caps)",
            Box::new(|| {
                // merge = 256 × (128 blocks + 128 br_ifs) = the cap.
                let ops = (MAX_PRESSURE_WORK - HEAD_PRESSURE) / 256 - 256;
                module(&nested_fn("block", 256, MAX_NESTING, 0, 0, ops))
            }),
        ),
        (
            "max br_table targets into 15 nested loops (targets + merge caps)",
            Box::new(move || {
                // merge = 61 × (16 blocks + 15 br_ifs + 1024 targets) ≤ cap.
                module(&nested_fn("loop", 61, 16, 1, MAX_BR_TABLE_TARGETS, 32_768))
            }),
        ),
        (
            "max br_table targets into max nested blocks (targets + merge caps)",
            Box::new(move || {
                // merge = 51 × (128 blocks + 127 br_ifs + 1024 targets) ≤ cap.
                module(&nested_fn(
                    "block",
                    51,
                    MAX_NESTING,
                    1,
                    MAX_BR_TABLE_TARGETS,
                    MAX_OPERATORS_PER_FUNCTION,
                ))
            }),
        ),
        (
            "sequential loops in one function (branch + merge caps)",
            Box::new(move || module(&seq_fn(seq_locals, seq_loops, 60_000))),
        ),
        (
            "max locals, straight line (pressure cap)",
            Box::new(move || module(&nested_fn("block", l, 1, 0, 0, deep_ops))),
        ),
        (
            "128 locals x max operators in one function (pressure cap)",
            Box::new(|| {
                let ops = ((MAX_PRESSURE_WORK - HEAD_PRESSURE) / 128 - 128)
                    .min(MAX_OPERATORS_PER_FUNCTION);
                module(&nested_fn("loop", 128, 4, 0, 0, ops))
            }),
        ),
        (
            "max blocks as loops over max functions",
            Box::new(move || {
                let ops = MAX_OPERATORS / fns - 1;
                let (base, rest) = (MAX_BLOCKS / fns, MAX_BLOCKS % fns);
                let mut m = String::new();
                for i in 0..fns {
                    m.push_str(&seq_fn(0, base + u64::from(i < rest), ops));
                }
                module(&m)
            }),
        ),
        (
            "max functions x max operators",
            Box::new(move || {
                let ops = (MAX_OPERATORS - HEAD_FUNCS) / fns;
                module(&nested_fn("block", 8, 1, 0, 0, ops).repeat(fns as usize))
            }),
        ),
        (
            "max operators in max-size functions",
            Box::new(|| {
                let n = MAX_OPERATORS / MAX_OPERATORS_PER_FUNCTION;
                let last = MAX_OPERATORS - (n - 1) * MAX_OPERATORS_PER_FUNCTION - HEAD_FUNCS;
                let mut m = nested_fn("loop", 4, 8, 0, 0, MAX_OPERATORS_PER_FUNCTION)
                    .repeat(n as usize - 1);
                m.push_str(&nested_fn("loop", 4, 8, 0, 0, last));
                module(&m)
            }),
        ),
        (
            "max types x params x results, max exports",
            Box::new(move || module(&typed_fns(types, exported))),
        ),
        (
            "max element items (ref.func expressions) over max types",
            Box::new(move || {
                let m = typed_fns(types, exported) + &elem_exprs(MAX_ELEMENT_ITEMS, types);
                module(&m)
            }),
        ),
        (
            "imports, globals, tables, data at caps",
            Box::new(|| {
                let half = MAX_TOTAL_TABLE_ELEMENTS / 2;
                let t = format!("(table {half} funcref) (table {half} funcref)");
                module_with_prefix(&imports_at_cap(), &(t + &globals_data_at_cap()))
            }),
        ),
        (
            "max locals spread over max functions",
            Box::new(move || {
                // No branches (merge 0); operators up to the pressure cap.
                let per = (MAX_TOTAL_LOCALS / fns).min(l);
                let ops = (MAX_PRESSURE_WORK - HEAD_PRESSURE) / fns / per - per;
                module(&nested_fn("block", per, 0, 0, 0, ops).repeat(fns as usize))
            }),
        ),
        (
            "everything at once (nested loops)",
            Box::new(|| combined(false)),
        ),
        (
            "everything at once (br_table into loops)",
            Box::new(|| combined(true)),
        ),
    ]
}

/// One module at every module-wide budget at once. The merge, pressure and
/// branch budgets are spent either on max locals in max nested loops plus
/// sequential loops (`br_table == false`), or on a `br_table` into nested
/// loops plus max locals in straight-line code plus sequential loops. Both
/// also have max typed and exported functions, max element items, imports,
/// globals, tables and data at their caps, and small looping functions
/// taking the function, block and operator counts to their caps.
fn combined(br_table: bool) -> Vec<u8> {
    let l = MAX_LOCALS_PER_FUNCTION;
    // The imports add one type (`() -> i64`).
    let types = u64::from(MAX_TYPES) - HEAD_TYPES - 1;
    let exported = u64::from(MAX_EXPORTS) - HEAD_EXPORTS;
    let p = u64::from(MAX_PARAMS);
    let typed_ops = types * (u64::from(MAX_RESULTS) + 1);
    let mut pressure = HEAD_PRESSURE + types * p * (u64::from(MAX_RESULTS) + 1 + p);
    let (mut ops, mut blocks, mut branch, mut fns) =
        (HEAD_FUNCS + typed_ops, 0, 0, HEAD_FUNCS + types);
    let mut m = String::new();
    if br_table {
        // merge = 61 × (16 blocks + 15 br_ifs + 1024 targets) ≤ cap.
        let (v, f_ops) = (61, 32_768);
        m.push_str(&nested_fn("loop", v, 16, 1, MAX_BR_TABLE_TARGETS, f_ops));
        pressure += v * (f_ops + v);
        ops += f_ops;
        blocks += 16;
        branch += (31 + MAX_BR_TABLE_TARGETS).pow(2);
        // The rest of the pressure budget: max locals, no branches.
        let s_ops = (MAX_PRESSURE_WORK - pressure) / l - l;
        m.push_str(&nested_fn("block", l, 0, 0, 0, s_ops));
        ops += s_ops;
        fns += 2;
    } else {
        // merge = l × 2 × MAX_LOOP_NESTING = the cap.
        let d_ops = (MAX_PRESSURE_WORK - pressure) / l - l;
        m.push_str(&nested_fn("loop", l, MAX_LOOP_NESTING, 0, 0, d_ops));
        ops += d_ops;
        blocks += MAX_LOOP_NESTING;
        branch += (2 * MAX_LOOP_NESTING).pow(2);
        fns += 1;
    }
    // Small functions: the rest of the functions, with up to 2 loops each
    // (≤ 16 branch work each), budgeted before the sequential function.
    let small = u64::from(MAX_FUNCTIONS) - fns - 1;
    branch += small * 16;
    // Sequential loops (no locals) with the rest of the branch budget.
    let seq_loops = ((MAX_BRANCH_WORK - branch) as f64).sqrt() as u64 / 2;
    let seq_ops = seq_loops * 4 + 1;
    m.push_str(&seq_fn(0, seq_loops, seq_ops));
    ops += seq_ops;
    blocks += seq_loops;
    m.push_str(&typed_fns(types, exported));
    m.push_str(&elem_exprs(MAX_ELEMENT_ITEMS, types));
    let rest_tables = MAX_TOTAL_TABLE_ELEMENTS - MAX_ELEMENT_ITEMS;
    let big = rest_tables.min(65_536);
    write!(
        m,
        "(table {big} funcref) (table {} funcref)",
        rest_tables - big
    )
    .unwrap();
    m.push_str(&globals_data_at_cap());
    let left = (MAX_BLOCKS - blocks).min(2 * small);
    let (base, rest) = (left / small, left % small);
    let ops_each = (MAX_OPERATORS - ops) / small;
    for i in 0..small {
        m.push_str(&seq_fn(0, base + u64::from(i < rest), ops_each));
    }
    module_with_prefix(&imports_at_cap(), &m)
}

fn prepare(bin: &[u8]) -> Result<(), ScriptError> {
    WasmScriptBackend::prepare(bin, WasmConfig::engine()).map(drop)
}

#[test]
fn worst_case_modules_at_the_caps_are_accepted() {
    for (name, build) in worst_cases() {
        let bin = build();
        assert!(bin.len() <= MAX_MODULE_BYTES, "{name}: {} bytes", bin.len());
        if let Err(e) = prepare(&bin) {
            panic!("{name} must be accepted: {e:?}");
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
        &module(&nested_fn("block", 0, MAX_NESTING + 1, 0, 0, 1024)),
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
        &module(&nested_fn("block", 0, 4, n, MAX_BR_TABLE_TARGETS, 32_768)),
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
    rejected(&module(&f), "locals x branch edges", MAX_MERGE_WORK);
    // Branch work: b sequential loops are 2b branches.
    let b = (MAX_BRANCH_WORK as f64).sqrt() as u64 / 2 + 1;
    rejected(
        &module(&seq_fn(0, b, b * 4 + 1)),
        "branches squared",
        MAX_BRANCH_WORK,
    );
    let ops = MAX_PRESSURE_WORK / l - l + 1;
    rejected(
        &module(&nested_fn("block", l, 1, 0, 0, ops)),
        "locals x operators",
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
    for (i, (name, build)) in worst_cases().into_iter().enumerate() {
        if only.is_some_and(|o| o != i) {
            continue;
        }
        let bin = build();
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
