//! Subphase 4.5 (fix round 2, B1): the pre-compile caps bound compile cost.
//!
//! For every cap there is a module just over it (rejected, with a message
//! naming the limit) and a worst-case module exactly at it (accepted). The
//! worst shapes are also combined. `compile_cost_report` prints the release
//! compile time and peak RSS of each worst case:
//! `cargo test -p pin_script --release --test wasm_compile_cost -- --ignored --nocapture --test-threads=1`.

use pin_core::ScriptError;
use pin_script::NameTables;
use pin_script::wasm::{
    MAX_BR_TABLE_TARGETS, MAX_BR_TABLE_TARGETS_PER_FUNCTION, MAX_ELEMENT_ITEMS, MAX_EXPORTS,
    MAX_FUNCTIONS, MAX_GLOBALS, MAX_IMPORTS, MAX_LOCALS_PER_FUNCTION, MAX_MODULE_BYTES,
    MAX_NESTING, MAX_OPERATORS, MAX_OPERATORS_PER_FUNCTION, MAX_PARAMS, MAX_RESULTS, MAX_SEGMENTS,
    MAX_TOTAL_LOCALS, MAX_TOTAL_TABLE_ELEMENTS, MAX_TYPES, WasmConfig, WasmScriptBackend,
};
use std::fmt::Write as _;

/// The required exports: three functions (one operator each, `end`), two
/// types and four exports.
const HEAD: &str = r#"(memory (export "memory") 1)
  (func (export "on_init")) (func (export "on_tick"))
  (func (export "on_switch_edge") (param i32 i32))"#;
const HEAD_FUNCS: u64 = 3;
const HEAD_TYPES: u64 = 2;
const HEAD_EXPORTS: u64 = 4;

fn module(extra: &str) -> Vec<u8> {
    wat::parse_str(format!("(module {HEAD} {extra})")).expect("generator emits valid WAT")
}

/// One function: `locals` i32 locals, `depth` levels of nesting (loops,
/// plus one `if` around each `br_table` when there are any), `br_tables`
/// `br_table`s of `targets` targets each (default included) spread over
/// every enclosing label, and local updates in the innermost loop filling
/// exactly `ops` operators (`end` included).
fn heavy_fn(locals: u64, depth: u64, br_tables: u64, targets: u64, ops: u64) -> String {
    let loops = if br_tables > 0 { depth - 1 } else { depth };
    let fixed = loops * 4 + br_tables * 5 + 1;
    assert!(
        ops >= fixed,
        "{ops} operators cannot hold the shape ({fixed})"
    );
    let mut s = String::from("(func");
    if locals > 0 {
        s.push_str(" (local");
        for _ in 0..locals {
            s.push_str(" i32");
        }
        s.push(')');
    }
    s.push('\n');
    for _ in 0..loops {
        s.push_str("loop\n");
    }
    for j in 0..br_tables {
        let sel = j % locals.max(1);
        let sel = if locals > 0 {
            format!("local.get {sel}")
        } else {
            "i32.const 0".into()
        };
        write!(s, "{sel} if {sel} br_table").unwrap();
        for t in 0..targets {
            write!(s, " {}", (t + j) % (loops + 1)).unwrap();
        }
        s.push_str(" end\n");
    }
    let mut left = ops - fixed;
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
    for k in 0..loops {
        let cond = if locals > 0 {
            format!("local.get {}", k % locals)
        } else {
            "i32.const 0".into()
        };
        writeln!(s, "{cond} br_if 0 end").unwrap();
    }
    s.push_str(")\n");
    s
}

/// The largest `p` in `1..=hi` for which `build(p)` fits the binary cap.
fn fit(hi: u64, build: impl Fn(u64) -> Vec<u8>) -> (u64, Vec<u8>) {
    let full = build(hi);
    if full.len() <= MAX_MODULE_BYTES {
        return (hi, full);
    }
    // Sizes are close to linear in `p`: extrapolate, then step down.
    let small = build(1).len() as f64;
    let per = (full.len() as f64 - small) / (hi - 1) as f64;
    let mut p = (((MAX_MODULE_BYTES as f64 - small) / per) as u64 + 1).min(hi);
    loop {
        let b = build(p);
        if b.len() <= MAX_MODULE_BYTES {
            return (p, b);
        }
        p -= (p / 200).max(1);
    }
}

/// `n` distinct function types of `MAX_PARAMS` params (i32/i64/f32/f64
/// chosen by the bits of the index) and `MAX_RESULTS` i32 results, with a
/// function of each type; the first `exported` are exported.
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

fn many(n: u64, item: &str) -> String {
    item.repeat(n as usize)
}

/// Every worst case at the caps, by name.
fn worst_cases() -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let opf = MAX_OPERATORS_PER_FUNCTION;
    let (l, d) = (MAX_LOCALS_PER_FUNCTION, MAX_NESTING);

    // One function at max locals x max nesting x max operators.
    out.push((
        "1 fn: locals x nesting x ops".into(),
        module(&heavy_fn(l, d, 0, 0, opf)),
    ));
    // ... plus every br_table target the function may have.
    let tables = MAX_BR_TABLE_TARGETS_PER_FUNCTION / MAX_BR_TABLE_TARGETS;
    out.push((
        "1 fn: + max br_table targets".into(),
        module(&heavy_fn(l, d, tables, MAX_BR_TABLE_TARGETS, opf)),
    ));
    // As many functions at max locals as the module-wide locals cap allows,
    // each at max nesting and br_table targets, operators filling the rest.
    let heavy = MAX_TOTAL_LOCALS / l;
    let (ops, bin) = fit(opf.min(MAX_OPERATORS / heavy), |ops| {
        module(&many(1, &heavy_fn(l, d, tables, MAX_BR_TABLE_TARGETS, ops)).repeat(heavy as usize))
    });
    out.push((
        format!("{heavy} fns: locals x nesting x br_table, {ops} ops each"),
        bin,
    ));
    // Max functions, all at max nesting, operators shared out to the
    // module-wide cap (or the byte cap).
    let fns = u64::from(MAX_FUNCTIONS) - HEAD_FUNCS;
    let (ops, bin) = fit(MAX_OPERATORS / fns, |ops| {
        module(&heavy_fn(0, d.min(ops / 4 - 1), 0, 0, ops).repeat(fns as usize))
    });
    out.push((format!("{fns} fns: nesting, {ops} ops each"), bin));
    // Max functions each with an equal share of the module-wide locals.
    let (ops, bin) = fit(MAX_OPERATORS / fns, |ops| {
        module(&heavy_fn(MAX_TOTAL_LOCALS / fns, 1, 0, 0, ops.max(9)).repeat(fns as usize))
    });
    out.push((format!("{fns} fns: shared locals, {ops} ops each"), bin));
    // Densest operator stream the byte cap allows, in max-size functions.
    let per = opf;
    let (n, bin) = fit(MAX_OPERATORS / per, |n| {
        module(&heavy_fn(4, d, 0, 0, per).repeat(n as usize))
    });
    out.push((format!("{n} fns x {per} ops (dense)"), bin));
    // Max types, each at max params and results, each with an exported
    // function (the escaping signatures need trampolines).
    let types = u64::from(MAX_TYPES) - HEAD_TYPES;
    let exported = u64::from(MAX_EXPORTS) - HEAD_EXPORTS;
    out.push((
        "types x params x results, max exports".into(),
        module(&typed_fns(types, exported)),
    ));
    // Max element items as expressions over those functions, active, in one
    // table, plus the same again as function indices (passive) is over the
    // cap, so: half and half.
    let half = MAX_ELEMENT_ITEMS / 2;
    let mut elems =
        String::from("(table $tab 65536 funcref)\n(elem (table $tab) (i32.const 0) funcref");
    for i in 0..half {
        write!(elems, " (ref.func $t{})", i % types).unwrap();
    }
    elems.push_str(")\n(elem func");
    for i in 0..MAX_ELEMENT_ITEMS - half {
        write!(elems, " $t{}", (i * 7) % types).unwrap();
    }
    elems.push_str(")\n");
    out.push((
        "max element items over max types".into(),
        module(&format!("{}{elems}", typed_fns(types, exported))),
    ));
    // Everything else at its cap: imports, globals, tables, data segments.
    let mut rest = String::new();
    for i in 0..MAX_IMPORTS {
        write!(
            rest,
            "(import \"pinball\" \"tick\" (func $imp{i} (result i64)))"
        )
        .unwrap();
    }
    rest.push_str(&many(
        u64::from(MAX_GLOBALS),
        "(global (mut i64) (i64.const 1))",
    ));
    let per_table = MAX_TOTAL_TABLE_ELEMENTS / 2;
    write!(
        rest,
        "(table {per_table} funcref) (table {per_table} funcref)"
    )
    .unwrap();
    rest.push_str(&many(u64::from(MAX_SEGMENTS), "(data (i32.const 0) \"x\")"));
    // Imports must come first in WAT text.
    out.push((
        "imports, globals, tables, data at caps".into(),
        wat::parse_str(format!("(module {rest} {HEAD})")).unwrap(),
    ));
    out
}

fn prepare(bin: &[u8]) -> Result<(), ScriptError> {
    WasmScriptBackend::prepare(bin, WasmConfig::engine()).map(drop)
}

#[test]
fn worst_case_modules_at_the_caps_are_accepted() {
    for (name, bin) in worst_cases() {
        assert!(bin.len() <= MAX_MODULE_BYTES, "{name}: {} bytes", bin.len());
        if let Err(e) = prepare(&bin) {
            panic!("{name} must be accepted: {e:?}");
        }
        // And it instantiates.
        let r = WasmScriptBackend::new(&bin, &NameTables::default(), WasmConfig::default());
        assert!(r.is_ok(), "{name}: {:?}", r.err());
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
    rejected(&module(&heavy_fn(l + 1, 1, 0, 0, 16)), "locals", l);
    let per = MAX_TOTAL_LOCALS / l + 1;
    rejected(
        &module(&heavy_fn(l, 1, 0, 0, 16).repeat(per as usize)),
        "locals",
        MAX_TOTAL_LOCALS,
    );
    rejected(
        &module(&heavy_fn(0, MAX_NESTING + 1, 0, 0, 1024)),
        "nested blocks",
        MAX_NESTING,
    );
    rejected(
        &module(&heavy_fn(1, 4, 1, MAX_BR_TABLE_TARGETS + 1, 4096)),
        "targets",
        MAX_BR_TABLE_TARGETS,
    );
    let n = MAX_BR_TABLE_TARGETS_PER_FUNCTION / MAX_BR_TABLE_TARGETS + 1;
    rejected(
        &module(&heavy_fn(1, 4, n, MAX_BR_TABLE_TARGETS, 4096)),
        "br_table targets",
        MAX_BR_TABLE_TARGETS_PER_FUNCTION,
    );
    rejected(
        &module(&heavy_fn(0, 1, 0, 0, MAX_OPERATORS_PER_FUNCTION + 1)),
        "operators",
        MAX_OPERATORS_PER_FUNCTION,
    );
    // Module-wide operators: full functions, then one more operator.
    let full = MAX_OPERATORS / MAX_OPERATORS_PER_FUNCTION - 1;
    let rest = MAX_OPERATORS - full * MAX_OPERATORS_PER_FUNCTION - HEAD_FUNCS + 1;
    let body = format!(
        "{}{}",
        heavy_fn(0, 1, 0, 0, MAX_OPERATORS_PER_FUNCTION).repeat(full as usize),
        heavy_fn(0, 1, 0, 0, rest)
    );
    rejected(&module(&body), "operators", MAX_OPERATORS);
    let types = u64::from(MAX_TYPES) - HEAD_TYPES + 1;
    let mut t = String::new();
    for i in 0..types {
        write!(t, "(type (func (param").unwrap();
        for b in 0..11 {
            t.push_str(if i >> b & 1 == 1 { " i64" } else { " i32" });
        }
        t.push_str(")))");
    }
    rejected(&module(&t), "types", MAX_TYPES.into());
    let p = many(u64::from(MAX_PARAMS) + 1, " i32");
    rejected(
        &module(&format!("(type (func (param{p})))")),
        "params",
        MAX_PARAMS.into(),
    );
    let r = many(u64::from(MAX_RESULTS) + 1, " i32");
    rejected(
        &module(&format!("(type (func (result{r})))")),
        "results",
        MAX_RESULTS.into(),
    );
    let items = many(MAX_ELEMENT_ITEMS + 1, " 0");
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
    let imports = many(
        u64::from(MAX_IMPORTS) + 1,
        "(import \"pinball\" \"tick\" (func (result i64)))",
    );
    rejected(
        &wat::parse_str(format!("(module {imports} {HEAD})")).unwrap(),
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

/// Prints compile time and peak RSS for every worst case (never asserts:
/// timings are machine-dependent). Run single-threaded in release.
#[test]
#[ignore = "prints release compile cost for the report"]
fn compile_cost_report() {
    let cases = worst_cases();
    println!("| worst case | binary bytes | compile (prepare) | peak RSS |");
    println!("|---|---|---|---|");
    for (name, bin) in cases {
        // Reset the peak-RSS counter to the current RSS.
        let _ = std::fs::write("/proc/self/clear_refs", "5");
        let start = std::time::Instant::now();
        let r = prepare(&bin);
        let t = start.elapsed();
        let rss = peak_rss().map_or("n/a".into(), |b| format!("{} MB", b >> 20));
        println!(
            "| {name} | {} | {t:.3?} | {rss} |{}",
            bin.len(),
            if r.is_ok() { "" } else { " REJECTED" }
        );
    }
}
