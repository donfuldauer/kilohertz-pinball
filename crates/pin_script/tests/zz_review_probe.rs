//! TEMPORARY reviewer probe (4.5 round 2). Delete before finishing.
use pin_core::{HostCommandQueue, HostCtx, HostView, Pcg32, SeedableRng, TableScriptBackend};
use pin_script::NameTables;
use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use std::time::{Duration, Instant};
use std::fmt::Write;

fn base(extra: &str) -> String {
    format!(r#"(module
      (import "pinball" "tick" (func $tick (result i64)))
      (import "pinball" "set_score" (func $set_score (param i32 i64)))
      (memory (export "memory") 1)
      {extra}
      (func (export "on_init"))
      (func (export "on_tick"))
      (func (export "on_switch_edge") (param i32 i32)))"#)
}

fn time(name: &str, src: &[u8]) {
    let t = Instant::now();
    let r = WasmScriptBackend::prepare(src, WasmConfig::engine());
    let d = t.elapsed();
    let msg = match &r { Ok(_) => "OK".to_string(), Err(e) => format!("{e:?}").chars().take(160).collect() };
    println!("PROBE compile {name}: src {} bytes -> {msg} in {d:?}", src.len());
    if r.is_ok() {
        let t = Instant::now();
        let b = WasmScriptBackend::new(src, &NameTables::default(), WasmConfig::engine());
        println!("PROBE   new(): ok={} in {:?} err={:?}", b.is_ok(), t.elapsed(), b.err().map(|e| format!("{e:?}").chars().take(120).collect::<String>()));
    }
}

fn loops_locals(locals: usize, depth: usize, funcs: usize) -> String {
    let mut s = String::new();
    for _ in 0..funcs {
        s.push_str("(func (result i64) (local ");
        for _ in 0..locals { s.push_str("i64 "); }
        s.push_str(") ");
        for _ in 0..depth { s.push_str("(loop "); }
        for i in 0..locals { write!(s, "(local.set {i} (i64.add (local.get {i}) (i64.const 1)))").unwrap(); }
        s.push_str("(br_if 0 (i32.const 0))");
        for _ in 0..depth { s.push_str(")"); }
        s.push_str("(i64.const 0)");
        for i in 0..locals { write!(s, "(i64.add (local.get {i}))").unwrap(); }
        s.push_str(")\n");
    }
    s
}

#[test]
fn probe_case() {
    let case = std::env::var("PROBE_CASE").unwrap_or_default();
    let mut s = String::new();
    match case.as_str() {
        "b1_1m_funcs" => {
            for _ in 0..999_990 { s.push_str("(func (param i32) (result i32) (i32.add (local.get 0) (i32.const 1)))\n"); }
            time(&case, base(&s).as_bytes());
        }
        "b1_300k_stmt" => {
            s.push_str("(func (result i64) (local $a i64) ");
            for i in 0..300_000 { write!(s, "(local.set $a (i64.mul (i64.add (local.get $a) (i64.const {i})) (i64.const 31)))\n").unwrap(); }
            s.push_str("(local.get $a))");
            time(&case, base(&s).as_bytes());
        }
        "b1_200k_funcs" => {
            for _ in 0..200_000 { s.push_str("(func (param i32) (result i32) (i32.add (local.get 0) (i32.const 1)))\n"); }
            time(&case, base(&s).as_bytes());
        }
        "b1_8mb_binary" => {
            // valid header + one 8 MB custom section
            let mut b = b"\0asm\x01\0\0\0".to_vec();
            b.push(0); b.extend_from_slice(&[0x80, 0x80, 0x80, 0x04]); b.push(1); b.push(b'x');
            b.resize(8 + 5 + 8 * 1024 * 1024, 0);
            time(&case, &b);
        }
        "cap_10k_funcs" => {
            // 10 000 functions filling ~1 MiB: ~95 bytes each
            for _ in 0..9_990 {
                s.push_str("(func (param i64) (result i64) (local.get 0)");
                for k in 0..22 { write!(s, "(i64.mul (i64.const {}))(i64.add (i64.const 77))", 1000 + k).unwrap(); }
                s.push_str(")\n");
            }
            time(&case, base(&s).as_bytes());
        }
        "cap_10k_funcs_exported" => {
            for i in 0..9_990 {
                write!(s, "(func (export \"f{i}\") (param i64 i32 f32 f64) (result i64) (local.get 0)").unwrap();
                for k in 0..18 { write!(s, "(i64.mul (i64.const {}))(i64.add (i64.const 77))", 1000 + k).unwrap(); }
                s.push_str(")\n");
            }
            time(&case, base(&s).as_bytes());
        }
        "cap_4_big_funcs" => {
            for _ in 0..4 {
                s.push_str("(func (result i64) (local $a i64) ");
                for i in 0..36_000 { write!(s, "(local.set $a (i64.mul (i64.add (local.get $a) (i64.const {i})) (i64.const 31)))\n").unwrap(); }
                s.push_str("(local.get $a))");
            }
            time(&case, base(&s).as_bytes());
        }
        "cap_wat_4mb_comments" => {
            s = base("");
            s.push_str(&";; padding padding padding padding\n".repeat(110_000));
            time(&case, s.as_bytes());
        }
        "many_tables" => {
            for _ in 0..100_000 { s.push_str("(table 0 funcref)"); }
            time(&case, base(&s).as_bytes());
        }
        "many_types" => {
            for i in 0..150_000 { write!(s, "(type (func (param {})))", if i % 2 == 0 { "i32 i64" } else { "f32 i32 i32" }).unwrap(); }
            time(&case, base(&s).as_bytes());
        }
        "many_distinct_types" => {
            // distinct signatures: binary expansions of i
            for i in 0..40_000u32 { s.push_str("(type (func (param"); for b in 0..16 { s.push_str(if i >> b & 1 == 1 { " i32" } else { " i64" }); } s.push_str(")))"); }
            time(&case, base(&s).as_bytes());
        }
        "many_imports" => {
            let mut m = String::from("(module ");
            for _ in 0..50_000 { m.push_str("(import \"pinball\" \"tick\" (func (result i64)))"); }
            m.push_str("(memory (export \"memory\") 1)(func (export \"on_init\"))(func (export \"on_tick\"))(func (export \"on_switch_edge\") (param i32 i32)))");
            time(&case, m.as_bytes());
        }
        "many_exports" => {
            s.push_str("(func $f)");
            for i in 0..100_000 { write!(s, "(export \"e{i}\" (func $f))").unwrap(); }
            time(&case, base(&s).as_bytes());
        }
        "elem_big" => {
            s.push_str("(func $f) (table 65536 funcref)");
            for _ in 0..10 { s.push_str("(elem func"); for _ in 0..90_000 { s.push_str(" $f"); } s.push_str(")"); }
            time(&case, base(&s).as_bytes());
        }
        "elem_expr_active" => {
            s.push_str("(func $f) (table 65536 funcref) (table 30000 funcref)");
            for _ in 0..4 { s.push_str("(elem (table 0) (i32.const 0) funcref"); for _ in 0..65_000 { s.push_str(" (ref.func $f)"); } s.push_str(")"); }
            time(&case, base(&s).as_bytes());
        }
        "elem_1000_active" => {
            s.push_str("(func $f) (table 65536 funcref)");
            for _ in 0..1000 { s.push_str("(elem (i32.const 0) func"); for _ in 0..900 { s.push_str(" $f"); } s.push_str(")"); }
            time(&case, base(&s).as_bytes());
        }
        "data_1000" => {
            let chunk = "a".repeat(1000);
            for _ in 0..1000 { write!(s, "(data (i32.const 0) \"{chunk}\")").unwrap(); }
            time(&case, base(&s).as_bytes());
        }
        "custom_sections" => {
            for i in 0..3 { write!(s, "(@custom \"c{i}\" \"{}\")", "z".repeat(300_000)).unwrap(); }
            time(&case, base(&s).as_bytes());
        }
        "names_big" => {
            for i in 0..9_000 { write!(s, "(func $averyveryveryveryveryveryveryveryveryveryveryveryveryveryveryverylongname{i})").unwrap(); }
            time(&case, base(&s).as_bytes());
        }
        "locals_loops" => {
            let l: usize = std::env::var("PL").unwrap().parse().unwrap();
            let d: usize = std::env::var("PD").unwrap().parse().unwrap();
            let f: usize = std::env::var("PF").unwrap().parse().unwrap();
            time(&format!("{case} L={l} D={d} F={f}"), base(&loops_locals(l, d, f)).as_bytes());
        }
        "brtable" => {
            // 400 nested blocks, 500 br_tables of 400 targets each
            s.push_str("(func (param i32) ");
            for _ in 0..400 { s.push_str("(block "); }
            for _ in 0..500 { s.push_str("(block (br_table "); for i in 0..400 { write!(s, "{i} ").unwrap(); } s.push_str("0 (local.get 0)))"); }
            for _ in 0..400 { s.push_str(")"); }
            s.push_str(")");
            time(&case, base(&s).as_bytes());
        }
        "many_params" => {
            s.push_str("(type $t (func (param"); for _ in 0..1000 { s.push_str(" i64"); } s.push_str(")))");
            s.push_str("(func $g (type $t))");
            s.push_str("(func ");
            for _ in 0..180 { for _ in 0..1000 { s.push_str("(i64.const 1)"); } s.push_str("(call $g)"); }
            s.push_str(")");
            time(&case, base(&s).as_bytes());
        }
        "simd_blocks" => {
            s.push_str("(func (result v128) (local $a v128) ");
            for i in 0..12_000 { write!(s, "(local.set $a (f32x4.div (f32x4.sqrt (local.get $a)) (v128.const i32x4 {i} 2 3 4)))").unwrap(); }
            s.push_str("(local.get $a))");
            time(&case, base(&s).as_bytes());
        }
        other => println!("PROBE unknown case {other}"),
    }
}

struct H { view: HostView, rng: Pcg32, cmd: HostCommandQueue }
impl H {
    fn new() -> Self { Self { view: HostView::default(), rng: Pcg32::seed_from_u64(1), cmd: HostCommandQueue::default() } }
    fn tick(&mut self, b: &mut WasmScriptBackend) -> Duration {
        self.cmd.clear();
        let t = Instant::now();
        b.on_tick(&mut HostCtx { view: &self.view, rng: &mut self.rng, cmd: &mut self.cmd });
        t.elapsed()
    }
}

/// `runs` fresh backends, `ticks` ticks each; prints worst/median of the per-backend worst tick.
fn stall(name: &str, src: &str, cfg: WasmConfig, runs: usize, ticks: usize) {
    let mut worsts = Vec::new();
    let mut faulted = 0;
    let mut err = None;
    for _ in 0..runs {
        let mut b = match WasmScriptBackend::new(src.as_bytes(), &NameTables::default(), cfg) {
            Ok(b) => b, Err(e) => { println!("PROBE stall {name}: load failed {e:?}"); return; } };
        let mut h = H::new();
        let mut w = Duration::ZERO;
        for _ in 0..ticks { w = w.max(h.tick(&mut b)); }
        if b.is_faulted() { faulted += 1; err = b.last_error().cloned(); }
        worsts.push(w);
    }
    worsts.sort();
    println!("PROBE stall {name}: worst {:?} median {:?} faulted {faulted}/{runs} err={err:?}", worsts[runs - 1], worsts[runs / 2]);
}

fn tickmod(pages: u32, extra: &str, body: &str) -> String {
    format!(r#"(module
      (memory (export "memory") {pages})
      {extra}
      (func (export "on_init"))
      (func (export "on_tick") (local $i i32) {body})
      (func (export "on_switch_edge") (param i32 i32)))"#)
}
fn lp(n: u32, op: &str) -> String {
    format!("(local.set $i (i32.const {n})) (loop $l {op} (local.set $i (i32.sub (local.get $i) (i32.const 1))) (br_if $l (local.get $i)))")
}

#[test]
fn probe_stall() {
    let e = WasmConfig::engine();
    let full = 2 * 1024 * 1024;
    stall("single fill 2MiB", &tickmod(32, "", &format!("(memory.fill (i32.const 0) (i32.const 1) (i32.const {full}))")), e, 50, 3);
    stall("single copy 2MiB-1", &tickmod(32, "", &format!("(memory.copy (i32.const 1) (i32.const 0) (i32.const {}))", full - 1)), e, 50, 3);
    stall("2 fills 2MiB (99999 then full)", &tickmod(32, "", &format!("(memory.fill (i32.const 0) (i32.const 1) (i32.const 99000)) (memory.fill (i32.const 0) (i32.const 1) (i32.const {full}))")), e, 50, 3);
    stall("fill loop 2MiB", &tickmod(32, "", &lp(1000, &format!("(memory.fill (i32.const 0) (i32.const 1) (i32.const {full}))"))), e, 50, 3);
    stall("copy loop 2MiB", &tickmod(32, "", &lp(1000, &format!("(memory.copy (i32.const 1) (i32.const 0) (i32.const {}))", full - 1))), e, 50, 3);
    stall("copy loop 9000B x10 nonfault", &tickmod(32, "", &lp(10, "(memory.copy (i32.const 1) (i32.const 0) (i32.const 9000))")), e, 50, 200);
    stall("fill loop 90000B x1 nonfault", &tickmod(32, "", "(memory.fill (i32.const 0) (i32.const 1) (i32.const 90000))"), e, 50, 200);
    stall("fill 0 bytes loop", &tickmod(32, "", &lp(100000, "(memory.fill (i32.const 0) (i32.const 1) (i32.const 0))")), e, 50, 3);
    stall("fill 1 byte loop", &tickmod(32, "", &lp(100000, "(memory.fill (i32.const 0) (i32.const 1) (i32.const 1))")), e, 50, 3);
    stall("grow loop", &tickmod(1, "", &lp(100000, "(drop (memory.grow (i32.const 1)))")), e, 50, 3);
    stall("grow 0 loop", &tickmod(1, "", &lp(100000, "(drop (memory.grow (i32.const 0)))")), e, 50, 3);
    stall("grow 31 single + fill", &tickmod(1, "", &format!("(drop (memory.grow (i32.const 31))) (memory.fill (i32.const 0) (i32.const 1) (i32.const {full}))")), e, 50, 3);
    stall("grow too-big loop (fails)", &tickmod(1, "", &lp(100000, "(drop (memory.grow (i32.const 60000)))")), e, 50, 3);
    stall("table.fill single 65536", &tickmod(1, "(table $t 65536 funcref)", "(table.fill $t (i32.const 0) (ref.null func) (i32.const 65536))"), e, 50, 3);
    stall("table.copy loop", &tickmod(1, "(table $t 65536 funcref)(table $u 34000 funcref)", &lp(1000, "(table.copy $t $t (i32.const 1) (i32.const 0) (i32.const 65535))")), e, 50, 3);
    stall("table.grow loop fail", &tickmod(1, "(table $t 0 funcref)", &lp(100000, "(drop (table.grow $t (ref.null func) (i32.const 70000)))")), e, 50, 3);
    stall("table.grow single 65536", &tickmod(1, "(table $t 0 funcref)", "(drop (table.grow $t (ref.null func) (i32.const 65536)))"), e, 50, 3);
    stall("memory.init loop", &tickmod(32, &format!("(data $d \"{}\")", "a".repeat(900_000)), &lp(1000, "(memory.init $d (i32.const 0) (i32.const 0) (i32.const 900000))")), e, 50, 3);
    stall("table.init loop", &{
        let mut x = String::from("(func $f)(table $t 65536 funcref)(elem $e func"); for _ in 0..65_000 { x.push_str(" $f"); } x.push_str(")");
        tickmod(1, &x, &lp(1000, "(table.init $t $e (i32.const 0) (i32.const 0) (i32.const 65000))")) }, e, 50, 3);
    stall("spin", &tickmod(1, "", &lp(1_000_000_000, "")), e, 50, 3);
    let d = WasmConfig::default();
    stall("default: single fill 16MiB", &tickmod(256, "", "(memory.fill (i32.const 0) (i32.const 1) (i32.const 16777216))"), d, 20, 3);
}

#[test]
fn probe_stack() {
    let src = r#"(module
      (memory (export "memory") 1)
      (func $r (param i64) (result i64) (i64.add (call $r (i64.add (local.get 0) (i64.const 1))) (i64.const 1)))
      (func (export "on_init"))
      (func (export "on_tick") (drop (call $r (i64.const 0))))
      (func (export "on_switch_edge") (param i32 i32)))"#;
    let kib: usize = std::env::var("PROBE_STACK_KIB").unwrap().parse().unwrap();
    std::thread::Builder::new().stack_size(kib * 1024).spawn(move || {
        stall(&format!("recursion on {kib} KiB thread"), src, WasmConfig { memory_limit_bytes: 1 << 20, fuel_per_callback: u64::MAX }, 3, 2);
    }).unwrap().join().unwrap();
}
