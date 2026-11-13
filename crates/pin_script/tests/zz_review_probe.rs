//! TEMPORARY reviewer probe (4.5 round 3). Delete before finishing.
use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use std::time::Instant;
use wasmtime::wasmparser::{Operator, Parser, Payload, ElementItems};

fn uleb(mut v: u64, out: &mut Vec<u8>) { loop { let b = (v & 0x7f) as u8; v >>= 7; if v == 0 { out.push(b); break; } out.push(b | 0x80); } }
fn sleb(mut v: i64, out: &mut Vec<u8>) { loop { let b = (v & 0x7f) as u8; v >>= 7; let done = (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0); if done { out.push(b); break; } out.push(b | 0x80); } }
fn section(id: u8, body: &[u8], out: &mut Vec<u8>) { out.push(id); uleb(body.len() as u64, out); out.extend_from_slice(body); }
const I32: u8 = 0x7f; const I64: u8 = 0x7e; const F64: u8 = 0x7c; const V128: u8 = 0x7b;
#[derive(Default, Clone)]
struct F { ty: u32, locals: Vec<(u32, u8)>, code: Vec<u8>, export: bool }
#[derive(Default)]
struct M { types: Vec<(Vec<u8>, Vec<u8>)>, funcs: Vec<F>, elem_items: Option<(usize, u32)>, table: Option<u32>, global: bool, mem_pages: u32 }
fn build(mut m: M) -> Vec<u8> {
    let t_void = m.types.len() as u32; m.types.push((vec![], vec![]));
    let t_sw = m.types.len() as u32; m.types.push((vec![I32, I32], vec![]));
    let first_entry = m.funcs.len() as u32;
    for ty in [t_void, t_void, t_sw] { m.funcs.push(F { ty, ..F::default() }); }
    let mut out = b"\0asm\x01\0\0\0".to_vec();
    let mut b = Vec::new(); uleb(m.types.len() as u64, &mut b);
    for (p, r) in &m.types { b.push(0x60); uleb(p.len() as u64, &mut b); b.extend_from_slice(p); uleb(r.len() as u64, &mut b); b.extend_from_slice(r); }
    section(1, &b, &mut out);
    let mut b = Vec::new(); uleb(m.funcs.len() as u64, &mut b); for f in &m.funcs { uleb(f.ty as u64, &mut b); }
    section(3, &b, &mut out);
    if let Some(n) = m.table { let mut b = vec![1, 0x70, 0]; uleb(n as u64, &mut b); section(4, &b, &mut out); }
    let mut b = vec![1, 0]; uleb(m.mem_pages.max(1) as u64, &mut b); section(5, &b, &mut out);
    if m.global { section(6, &[2, I64, 1, 0x42, 7, 0x0b, F64, 1, 0x44, 0,0,0,0,0,0,0xf0,0x3f, 0x0b], &mut out); }
    let mut b = Vec::new();
    let nexp = 4 + m.funcs.iter().filter(|f| f.export).count();
    uleb(nexp as u64, &mut b);
    for (name, idx) in [("on_init", first_entry), ("on_tick", first_entry + 1), ("on_switch_edge", first_entry + 2)] { uleb(name.len() as u64, &mut b); b.extend_from_slice(name.as_bytes()); b.push(0); uleb(idx as u64, &mut b); }
    b.push(6); b.extend_from_slice(b"memory"); b.push(2); b.push(0);
    for (i, f) in m.funcs.iter().enumerate() { if f.export { let n = format!("{i:x}"); uleb(n.len() as u64, &mut b); b.extend_from_slice(n.as_bytes()); b.push(0); uleb(i as u64, &mut b); } }
    section(7, &b, &mut out);
    if let Some((n, modulo)) = m.elem_items {
        let mut b = vec![1, 1, 0]; uleb(n as u64, &mut b);
        for i in 0..n { uleb((i as u32 % modulo) as u64, &mut b); }
        section(9, &b, &mut out);
    }
    let mut b = Vec::new(); uleb(m.funcs.len() as u64, &mut b);
    for f in &m.funcs {
        let mut body = Vec::new(); uleb(f.locals.len() as u64, &mut body);
        for (n, t) in &f.locals { uleb(*n as u64, &mut body); body.push(*t); }
        body.extend_from_slice(&f.code); body.push(0x0b);
        uleb(body.len() as u64, &mut b); b.extend_from_slice(&body);
    }
    section(10, &b, &mut out);
    out
}
fn envn(k: &str, d: usize) -> usize { std::env::var(k).ok().and_then(|v| v.parse().ok()).unwrap_or(d) }

fn peak_rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/status").ok().and_then(|s| s.lines().find(|l| l.starts_with("VmHWM")).and_then(|l| l.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()))).unwrap_or(0) / 1024
}

#[derive(Default, Debug)]
struct Metrics { bytes: usize, types: u32, imports: u32, funcs: u32, exports: u32, globals: u32, elem_items: u64, ops: u64, max_ops: u64, blocks: u64, max_nest: u64, max_loop_nest: u64, max_locals: u64, locals: u64, merge: u64, branch: u64, pressure: u64, max_tgt_fn: u64, max_params: u64 }
fn metrics(bin: &[u8]) -> Result<Metrics, String> {
    let mut m = Metrics { bytes: bin.len(), ..Default::default() };
    let mut type_params: Vec<u64> = vec![]; let mut func_types: Vec<u32> = vec![]; let mut f = 0usize;
    for p in Parser::new(0).parse_all(bin) {
        match p.map_err(|e| e.to_string())? {
            Payload::TypeSection(r) => { m.types = r.count(); for t in r.into_iter_err_on_gc_types() { let t = t.map_err(|e| e.to_string())?; m.max_params = m.max_params.max(t.params().len() as u64); type_params.push(t.params().len() as u64); } }
            Payload::ImportSection(r) => { m.imports = r.count(); }
            Payload::FunctionSection(r) => { m.funcs = r.count(); for t in r { func_types.push(t.map_err(|e| e.to_string())?); } }
            Payload::GlobalSection(r) => m.globals = r.count(),
            Payload::ExportSection(r) => m.exports = r.count(),
            Payload::ElementSection(r) => { for e in r { m.elem_items += match e.map_err(|e| e.to_string())?.items { ElementItems::Functions(f) => f.count() as u64, ElementItems::Expressions(_, x) => x.count() as u64 }; } }
            Payload::CodeSectionEntry(body) => {
                let mut lr = body.get_locals_reader().map_err(|e| e.to_string())?;
                let mut locals = 0u64;
                for _ in 0..lr.get_count() { locals += lr.read().map_err(|e| e.to_string())?.0 as u64; }
                let params = func_types.get(f).and_then(|&t| type_params.get(t as usize)).copied().unwrap_or(0);
                f += 1;
                let vars = params + locals;
                let mut ops = body.get_operators_reader().map_err(|e| e.to_string())?;
                let (mut n, mut depth, mut loops, mut branches, mut targets, mut blocks) = (0u64, 0u64, Vec::<bool>::new(), 0u64, 0u64, 0u64);
                while !ops.eof() {
                    let op = ops.read().map_err(|e| e.to_string())?; n += 1;
                    match op {
                        Operator::Block { .. } | Operator::Loop { .. } | Operator::If { .. } => { let l = matches!(op, Operator::Loop { .. }); loops.push(l); depth += 1; m.max_nest = m.max_nest.max(depth); m.max_loop_nest = m.max_loop_nest.max(loops.iter().filter(|x| **x).count() as u64); branches += 1; blocks += 1; }
                        Operator::End if depth > 0 => { depth -= 1; loops.pop(); }
                        Operator::Else | Operator::Br { .. } | Operator::BrIf { .. } | Operator::Return => branches += 1,
                        Operator::BrTable { targets: t } => targets += t.len() as u64 + 1,
                        _ => {}
                    }
                }
                m.ops += n; m.max_ops = m.max_ops.max(n); m.blocks += blocks; m.locals += locals; m.max_locals = m.max_locals.max(locals); m.max_tgt_fn = m.max_tgt_fn.max(targets);
                m.merge += vars * (branches + targets); let e = branches + targets; m.branch += e * e; m.pressure += vars * (n + vars);
            }
            _ => {}
        }
    }
    Ok(m)
}

fn run(name: &str, bin: &[u8]) {
    let t = Instant::now();
    let r = WasmScriptBackend::prepare(bin, WasmConfig::engine());
    let d = t.elapsed();
    let msg = match &r { Ok(_) => "ACCEPTED".to_string(), Err(e) => format!("{e:?}").chars().take(170).collect() };
    println!("PROBE {name}: {} bytes -> {msg} in {d:?}, peak RSS {} MB", bin.len(), peak_rss_mb());
    if bin.starts_with(b"\0asm") { if let Ok(m) = metrics(bin) {
        println!("PROBE   usage: ops {}/262144 maxops/fn {}/65536 funcs {}/2048 blocks {}/4096 merge {}/65536 branch {}/2097152 pressure {}/8388608 locals max {}/1024 nest {}/128 loopnest {}/32 types {}/256 exports {}/256 imports {}/64 elem {}/1024 brtgt/fn {}/1024", m.ops, m.max_ops, m.funcs, m.blocks, m.merge, m.branch, m.pressure, m.max_locals, m.max_nest, m.max_loop_nest, m.types, m.exports, m.imports, m.elem_items, m.max_tgt_fn);
    } }
}

#[test]
fn probe_file() {
    let p = std::env::var("PROBE_WASM").unwrap();
    let bin = std::fs::read(&p).unwrap();
    run(&p, &bin);
}

// stack-building helpers
fn push_vals(c: &mut Vec<u8>, n: usize) { for _ in 0..n { c.extend_from_slice(&[0x23, 0]); } } // global.get 0 (i64)
fn fold_vals(c: &mut Vec<u8>, n: usize) { for _ in 1..n { c.push(0x7c); } c.push(0x1a); } // i64.add..., drop

#[test]
fn probe_bin() {
    let case = std::env::var("PROBE_CASE").unwrap_or_default();
    let n = envn("PN", 0); let k = envn("PK", 0); let nf = envn("PF", 1);
    let mut m = M { global: true, ..M::default() };
    match case.as_str() {
        // ---- round 2 hostile modules
        "r2_locals_loops" => {
            let (l, d) = (n, k);
            let mut c = Vec::new();
            for _ in 0..d { c.extend_from_slice(&[0x03, 0x40]); }
            for i in 0..l { c.push(0x20); uleb(i as u64, &mut c); c.extend_from_slice(&[0x42, 1, 0x7c, 0x21]); uleb(i as u64, &mut c); }
            c.extend_from_slice(&[0x41, 0, 0x0d, 0]);
            for _ in 0..d { c.push(0x0b); }
            c.extend_from_slice(&[0x42, 0]);
            for i in 0..l { c.push(0x20); uleb(i as u64, &mut c); c.push(0x7c); }
            m.types.push((vec![], vec![I64]));
            for _ in 0..nf { m.funcs.push(F { ty: 0, locals: vec![(l as u32, I64)], code: c.clone(), export: false }); }
        }
        "r2_brtable" => {
            let mut c = Vec::new();
            for _ in 0..n { c.extend_from_slice(&[0x02, 0x40]); }
            for _ in 0..k { c.extend_from_slice(&[0x02, 0x40, 0x20, 0, 0x0e]); uleb(n as u64, &mut c); for i in 0..n { uleb(i as u64, &mut c); } c.push(0); c.push(0x0b); }
            for _ in 0..n { c.push(0x0b); }
            m.types.push((vec![I32], vec![]));
            m.funcs.push(F { ty: 0, code: c, ..F::default() });
        }
        "r2_locals_only" => { m.types.push((vec![], vec![])); for _ in 0..n { m.funcs.push(F { ty: 0, locals: vec![(k as u32, I64)], ..F::default() }); } }
        "r2_types" => { for i in 0..n { m.types.push(((0..k).map(|b| if (i >> (b % 20)) & 1 == 1 { I32 } else { I64 }).collect(), vec![])); } }
        "r2_sigs_escaping" => { for i in 0..n { m.types.push(((0..k).map(|b| if (i >> b) & 1 == 1 { I32 } else { I64 }).collect(), vec![])); m.funcs.push(F { ty: i as u32, export: true, ..F::default() }); } }
        "r2_elem" => { m.elem_items = Some((n, 3)); m.table = Some(0); }
        "r2_10k_funcs" => {
            m.types.push((vec![I64], vec![I64]));
            for _ in 0..9_990 { let mut c = vec![0x20, 0]; for j in 0..11 { c.push(0x42); sleb(1000 + j as i64, &mut c); c.push(0x7e); c.push(0x42); sleb(77, &mut c); c.push(0x7c); } m.funcs.push(F { ty: 0, code: c, export: k == 1, ..F::default() }); }
        }
        // ---- round 3 attack shapes (must pass limits)
        "straight" => { // baseline: nf funcs of n statements (6 ops each) on one local
            m.types.push((vec![I64], vec![I64]));
            for _ in 0..nf { let mut c = Vec::new(); for i in 0..n { c.extend_from_slice(&[0x20, 0, 0x42]); sleb(i as i64, &mut c); c.extend_from_slice(&[0x7c, 0x42, 31, 0x7e, 0x21, 0]); } c.extend_from_slice(&[0x20, 0]); m.funcs.push(F { ty: 0, code: c, ..F::default() }); }
        }
        "deep_stack" => { // n live stack values, k mid ops selected by PM
            let mode = envn("PM", 0);
            m.types.push((vec![], vec![]));
            m.table = Some(1);
            m.funcs.push(F { ty: 0, ..F::default() }); // func 0: void callee
            for _ in 0..nf {
                let mut c = Vec::new();
                push_vals(&mut c, n);
                for i in 0..k { match mode {
                    0 => {}
                    1 => c.extend_from_slice(&[0x10, 0]), // call 0
                    2 => c.extend_from_slice(&[0x41, 0, 0x11, 0, 0]), // call_indirect type 0 table 0
                    3 => c.extend_from_slice(&[0x41, 0, 0x25, 0, 0x1a]), // table.get, drop
                    4 => c.extend_from_slice(&[0x41, 0, 0x40, 0, 0x1a]), // memory.grow, drop
                    5 => c.extend_from_slice(&[0x23, 0, 0x42, 3, 0x7f, 0x1a]), // global.get, const, i64.div_s, drop
                    6 => c.extend_from_slice(&[0x23, 1, 0xb0, 0x1a]), // global.get f64, i64.trunc_f64_s, drop
                    7 => c.extend_from_slice(&[0x23, 1, 0x9f, 0x23, 1, 0xa3, 0xb0, 0x1a]), // f64 sqrt, div, trunc, drop
                    8 => { c.extend_from_slice(&[0x41]); sleb((i % 60000) as i64, &mut c); c.extend_from_slice(&[0x29, 3]); uleb((i * 8) as u64 % 60000, &mut c); c.push(0x1a); } // i64.load distinct offsets
                    9 => c.extend_from_slice(&[0x23, 0, 0x23, 0, 0x41, 1, 0x1b, 0x1a]), // select, drop
                    10 => c.extend_from_slice(&[0x41, 0, 0x41, 0, 0x41, 0, 0xfc, 0x0b, 0]), // memory.fill
                    11 => c.extend_from_slice(&[0x23, 0, 0x24, 0]), // global get/set traffic
                    12 => c.extend_from_slice(&[0x3f, 0, 0x1a]), // memory.size drop
                    _ => {}
                } }
                fold_vals(&mut c, n);
                m.funcs.push(F { ty: 0, code: c, ..F::default() });
            }
        }
        "deep_stack_brif" => { // n live values, then one block with k br_if 0
            m.types.push((vec![], vec![]));
            for _ in 0..nf { let mut c = Vec::new(); push_vals(&mut c, n); c.extend_from_slice(&[0x02, 0x40]); for _ in 0..k { c.extend_from_slice(&[0x41, 0, 0x0d, 0]); } c.push(0x0b); fold_vals(&mut c, n); m.funcs.push(F { ty: 0, code: c, ..F::default() }); }
        }
        "deep_stack_loops" => { // n live values, k sequential loops each with a fuel check; PM calls inside
            m.types.push((vec![], vec![]));
            m.funcs.push(F { ty: 0, ..F::default() });
            for _ in 0..nf { let mut c = Vec::new(); push_vals(&mut c, n); for _ in 0..k { c.extend_from_slice(&[0x03, 0x40, 0x10, 0, 0x0b]); } fold_vals(&mut c, n); m.funcs.push(F { ty: 0, code: c, ..F::default() }); }
        }
        "calls32" => { // calls of a 32-param function with n live stack values
            m.types.push((vec![I64; 32], vec![I64; 8]));
            m.types.push((vec![], vec![]));
            let mut callee = Vec::new(); for _ in 0..8 { callee.extend_from_slice(&[0x42, 0]); }
            m.funcs.push(F { ty: 0, code: callee, ..F::default() });
            for _ in 0..nf { let mut c = Vec::new(); push_vals(&mut c, n); for _ in 0..k { for p in 0..32 { c.extend_from_slice(&[0x23, 0]); let _ = p; } c.extend_from_slice(&[0x10, 0]); for _ in 0..8 { c.push(0x1a); } } fold_vals(&mut c, n); m.funcs.push(F { ty: 1, code: c, ..F::default() }); }
        }
        "v128_stack" => { // n live v128 values + k swizzles
            m.types.push((vec![], vec![]));
            for _ in 0..nf { let mut c = Vec::new();
                for _ in 0..n { c.extend_from_slice(&[0x23, 0, 0xfd, 0x12]); } // i64x2.splat
                for _ in 0..k { c.extend_from_slice(&[0x23, 0, 0xfd, 0x12, 0x23, 0, 0xfd, 0x12, 0xfd, 0x0e, 0x1a]); } // swizzle, drop
                for _ in 1..n { c.extend_from_slice(&[0xfd, 0xce, 0x01]); } // i64x2.add
                c.push(0x1a);
                m.funcs.push(F { ty: 0, code: c, ..F::default() }); }
            let _ = V128;
        }
        "multivalue_ifs" => { // k if/else each producing 8 results from 8 params, chained; n extra live values
            m.types.push((vec![I64; 8], vec![I64; 8]));
            m.types.push((vec![], vec![]));
            for _ in 0..nf { let mut c = Vec::new(); push_vals(&mut c, n + 8);
                for _ in 0..k { c.extend_from_slice(&[0x41, 1, 0x04, 0, 0x05, 0x0b]); } // i32.const 1; if (type 0) else end
                fold_vals(&mut c, n + 8);
                m.funcs.push(F { ty: 1, code: c, ..F::default() }); }
        }
        "many_small" => { // n functions each with k locals-free ops and one call to next
            m.types.push((vec![I64, I64, I64, I64], vec![I64]));
            for i in 0..n { let mut c = Vec::new(); for j in 0..k { c.extend_from_slice(&[0x20, (j % 4) as u8, 0x20, ((j + 1) % 4) as u8, 0x7e, 0x21, (j % 4) as u8]); }
                c.extend_from_slice(&[0x20, 0, 0x20, 1, 0x20, 2, 0x20, 3, 0x10]); uleb(((i + 1) % n) as u64, &mut c);
                m.funcs.push(F { ty: 0, code: c, export: i < 250, ..F::default() }); }
        }
        "locals_pressure" => { // nf funcs with n locals all live, straight-line; k ops
            m.types.push((vec![], vec![I64]));
            for _ in 0..nf { let mut c = Vec::new();
                for i in 0..n { c.extend_from_slice(&[0x23, 0, 0x21]); uleb(i as u64, &mut c); }
                for j in 0..k { let a = j % n; let b = (j * 7 + 1) % n; c.push(0x20); uleb(a as u64, &mut c); c.push(0x20); uleb(b as u64, &mut c); c.extend_from_slice(&[0x7c, 0x21]); uleb(a as u64, &mut c); }
                c.extend_from_slice(&[0x42, 0]); for i in 0..n { c.push(0x20); uleb(i as u64, &mut c); c.push(0x7c); }
                m.funcs.push(F { ty: 0, locals: vec![(n as u32, I64)], code: c, ..F::default() }); }
        }
        "locals_calls" => { // n locals live across k calls
            m.types.push((vec![], vec![I64]));
            m.types.push((vec![], vec![]));
            m.funcs.push(F { ty: 1, ..F::default() });
            for _ in 0..nf { let mut c = Vec::new();
                for i in 0..n { c.extend_from_slice(&[0x23, 0, 0x21]); uleb(i as u64, &mut c); }
                for _ in 0..k { c.extend_from_slice(&[0x10, 0]); }
                c.extend_from_slice(&[0x42, 0]); for i in 0..n { c.push(0x20); uleb(i as u64, &mut c); c.push(0x7c); }
                m.funcs.push(F { ty: 0, locals: vec![(n as u32, I64)], code: c, ..F::default() }); }
        }
        other => { println!("PROBE unknown {other}"); return; }
    }
    let bin = build(m);
    run(&format!("{case} N={n} K={k} F={nf} M={}", envn("PM", 0)), &bin);
}

#[test]
fn probe_wat() {
    let case = std::env::var("PROBE_CASE").unwrap_or_default();
    let head = r#"(memory (export "memory") 1) (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32))"#;
    let mut s = String::new();
    match case.as_str() {
        "r2_elem_expr_active" => { s.push_str("(func $f) (table 65536 funcref) (table 30000 funcref)"); for _ in 0..4 { s.push_str("(elem (table 0) (i32.const 0) funcref"); for _ in 0..65_000 { s.push_str(" (ref.func $f)"); } s.push_str(")"); } }
        "r2_distinct_types" => { for i in 0..40_000u32 { s.push_str("(type (func (param"); for b in 0..16 { s.push_str(if i >> b & 1 == 1 { " i32" } else { " i64" }); } s.push_str(")))"); } }
        "r2_elem_big" => { s.push_str("(func $f) (table 65536 funcref)"); for _ in 0..10 { s.push_str("(elem func"); for _ in 0..90_000 { s.push_str(" $f"); } s.push_str(")"); } }
        "r2_wat_error" => { s.push_str(&"(".repeat(3_000_000)); }
        _ => {}
    }
    let src = format!("(module {head} {s})");
    let t = Instant::now();
    let r = WasmScriptBackend::prepare(src.as_bytes(), WasmConfig::engine());
    let msg = match &r { Ok(_) => "ACCEPTED".to_string(), Err(e) => { let m = format!("{e:?}"); format!("(err len {}) {}", m.len(), m.chars().take(150).collect::<String>()) } };
    println!("PROBE {case}: {} bytes WAT -> {msg} in {:?}, peak RSS {} MB", src.len(), t.elapsed(), peak_rss_mb());
}
