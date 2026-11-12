//! TEMPORARY reviewer probe (4.5). Delete before finishing.
use pin_core::{HostCommandQueue, HostCtx, HostView, Pcg32, ScriptError, SeedableRng, SwitchEdge, TableScriptBackend};
use pin_core::ids::SwitchId;
use pin_script::NameTables;
use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use std::time::{Duration, Instant};

const IMPORTS: &str = r#"
  (import "pinball" "pulse_coil" (func $pulse_coil (param i32 i32)))
  (import "pinball" "hold_coil" (func $hold_coil (param i32 i32)))
  (import "pinball" "tick" (func $tick (result i64)))
  (import "pinball" "switch" (func $switch (param i32) (result i32)))
  (import "pinball" "set_score" (func $set_score (param i32 i64)))
  (import "pinball" "id_of" (func $id_of (param i32 i32 i32) (result i32)))
"#;

fn module(body: &str) -> String {
    format!("(module {IMPORTS} {body})")
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
    fn edge(&mut self, b: &mut WasmScriptBackend, id: u16) -> Duration {
        let t = Instant::now();
        b.on_switch_edge(SwitchId(id), SwitchEdge::Closed, &mut HostCtx { view: &self.view, rng: &mut self.rng, cmd: &mut self.cmd });
        t.elapsed()
    }
}

fn worst(name: &str, src: &str, cfg: WasmConfig, names: &NameTables, ticks: usize) {
    let t0 = Instant::now();
    let b = WasmScriptBackend::new(src.as_bytes(), names, cfg);
    let load = t0.elapsed();
    let mut b = match b {
        Ok(b) => b,
        Err(e) => { println!("PROBE {name}: load failed in {load:?}: {e:?}"); return; }
    };
    let mut h = H::new();
    let mut w = Duration::ZERO;
    let mut first_fault = None;
    for i in 0..ticks {
        let d = h.tick(&mut b);
        w = w.max(d);
        if b.is_faulted() && first_fault.is_none() { first_fault = Some(i); }
    }
    println!("PROBE {name}: load {load:?} worst tick {w:?} faulted={} at {:?} err={:?} score={:?} mem={}",
        b.is_faulted(), first_fault, b.last_error(), h.cmd.scores.last(), b.memory_size());
}

fn bulk(op: &str, pages: u32, iters: u32) -> String {
    module(&format!(r#"
      (memory (export "memory") {pages})
      (global $n (mut i64) (i64.const 0))
      (func (export "on_init"))
      (func (export "on_tick") (local $i i32)
        (local.set $i (i32.const {iters}))
        (loop $l
          {op}
          (local.set $i (i32.sub (local.get $i) (i32.const 1)))
          (br_if $l (local.get $i)))
        (global.set $n (i64.add (global.get $n) (i64.const 1)))
        (call $set_score (i32.const 1) (global.get $n)))
      (func (export "on_switch_edge") (param i32 i32))"#))
}

#[test]
fn probe_bulk_memory_stall() {
    let n = NameTables::default();
    let e = WasmConfig::engine();
    // 2 MiB = 32 pages; 31 pages source/dst span
    let fill = "(memory.fill (i32.const 0) (i32.const 0) (i32.const 2097152))";
    let copy = "(memory.copy (i32.const 1) (i32.const 0) (i32.const 2097151))";
    for iters in [10u32, 1000, 12000, 1000000] {
        worst(&format!("fill x{iters} engine"), &bulk(fill, 32, iters), e, &n, 3);
    }
    worst("copy x12000 engine", &bulk(copy, 32, 12000), e, &n, 3);
    let d = WasmConfig::default();
    let fill16 = "(memory.fill (i32.const 0) (i32.const 0) (i32.const 16777216))";
    worst("fill16M x300 default", &bulk(fill16, 256, 300), d, &n, 2);
    // plain loops
    worst("spin engine", &bulk("", 1, 1_000_000_000), e, &n, 3);
    worst("hostcall tick engine nonfault", &bulk("(drop (call $tick))", 1, 16000), e, &n, 5);
    worst("hostcall tick engine fault", &bulk("(drop (call $tick))", 1, 100000000), e, &n, 3);
    worst("id_of len0", &bulk("(drop (call $id_of (i32.const 0) (i32.const 0) (i32.const 0)))", 1, 100000000), e, &n, 3);
    worst("div loop", &bulk("(global.set $n (i64.div_s (i64.const 0x7fffffffffffffff) (i64.add (global.get $n) (i64.const 3))))", 1, 100000000), e, &n, 3);
}

#[test]
fn probe_table_and_grow() {
    let n = NameTables::default();
    let e = WasmConfig::engine();
    let src = module(r#"
      (memory (export "memory") 1)
      (table $t 65536 65536 funcref)
      (table $u 65536 65536 funcref)
      (func $f)
      (elem declare func $f)
      (func (export "on_init"))
      (func (export "on_tick") (local $i i32)
        (local.set $i (i32.const 12000))
        (loop $l
          (table.fill $t (i32.const 0) (ref.func $f) (i32.const 65536))
          (local.set $i (i32.sub (local.get $i) (i32.const 1)))
          (br_if $l (local.get $i))))
      (func (export "on_switch_edge") (param i32 i32))"#);
    worst("table.fill x12000", &src, e, &n, 3);
    let src = module(r#"
      (memory (export "memory") 1)
      (table $t 65536 65536 funcref)
      (table $u 65536 65536 funcref)
      (func (export "on_init"))
      (func (export "on_tick") (local $i i32)
        (local.set $i (i32.const 12000))
        (loop $l
          (table.copy $t $u (i32.const 0) (i32.const 0) (i32.const 65536))
          (local.set $i (i32.sub (local.get $i) (i32.const 1)))
          (br_if $l (local.get $i))))
      (func (export "on_switch_edge") (param i32 i32))"#);
    worst("table.copy x12000", &src, e, &n, 3);
    let src = module(r#"
      (memory (export "memory") 1)
      (func (export "on_init"))
      (func (export "on_tick") (local $i i32)
        (local.set $i (i32.const 18000))
        (loop $l
          (drop (memory.grow (i32.const 1)))
          (local.set $i (i32.sub (local.get $i) (i32.const 1)))
          (br_if $l (local.get $i))))
      (func (export "on_switch_edge") (param i32 i32))"#);
    worst("memory.grow x18000", &src, e, &n, 5);
    let src = module(r#"
      (memory (export "memory") 1)
      (table $t 0 funcref)
      (func (export "on_init"))
      (func (export "on_tick") (local $i i32)
        (local.set $i (i32.const 12000))
        (loop $l
          (drop (table.grow $t (ref.null func) (i32.const 70000)))
          (drop (table.grow $t (ref.null func) (i32.const 1)))
          (local.set $i (i32.sub (local.get $i) (i32.const 1)))
          (br_if $l (local.get $i))))
      (func (export "on_switch_edge") (param i32 i32))"#);
    worst("table.grow x12000", &src, e, &n, 8);
}

#[test]
fn probe_recursion() {
    let n = NameTables::default();
    let src = module(r#"
      (memory (export "memory") 1)
      (func $r (param i64) (result i64) (i64.add (call $r (i64.add (local.get 0) (i64.const 1))) (i64.const 1)))
      (func (export "on_init"))
      (func (export "on_tick") (drop (call $r (i64.const 0))))
      (func (export "on_switch_edge") (param i32 i32))"#);
    worst("recursion default", &src, WasmConfig::default(), &n, 3);
    worst("recursion engine", &src, WasmConfig::engine(), &n, 3);
    worst("recursion huge fuel", &src, WasmConfig { memory_limit_bytes: 1 << 20, fuel_per_callback: u64::MAX }, &n, 3);
    // run on a small thread stack
    let s2 = src.clone();
    std::thread::Builder::new().stack_size(256 * 1024).spawn(move || {
        worst("recursion huge fuel on 256K thread", &s2, WasmConfig { memory_limit_bytes: 1 << 20, fuel_per_callback: u64::MAX }, &NameTables::default(), 2);
    }).unwrap().join().unwrap();
}

#[test]
fn probe_host_func_as_export() {
    let n = NameTables::default();
    let src = r#"(module
      (import "pinball" "pulse_coil" (func $pulse_coil (param i32 i32)))
      (import "pinball" "hold_coil" (func $hold_coil (param i32 i32)))
      (import "pinball" "switch" (func $switch (param i32) (result i32)))
      (memory (export "memory") 1)
      (func (export "on_init"))
      (func (export "on_tick"))
      (export "on_switch_edge" (func $pulse_coil))
      (func (export "save_state") (result i64) (i64.const 0x0000000100000004))
      (export "alloc_state" (func $switch))
      (export "restore_state" (func $hold_coil)))"#;
    let mut b = WasmScriptBackend::new(src.as_bytes(), &n, WasmConfig::engine()).unwrap();
    let mut h = H::new();
    h.edge(&mut b, 7);
    println!("PROBE hostexport: faulted={} pulses={:?} err={:?}", b.is_faulted(), h.cmd.coil_pulses, b.last_error());
    let r = b.apply_reload(src.as_bytes());
    println!("PROBE hostexport reload: {r:?} faulted={}", b.is_faulted());
    h.edge(&mut b, 70000u32 as u16);
}

#[test]
fn probe_state_edge_cases() {
    let n = NameTables::default();
    let mk = |save: &str, alloc: &str| format!(r#"(module
      (memory (export "memory") 1)
      (func (export "on_init"))
      (func (export "on_tick"))
      (func (export "on_switch_edge") (param i32 i32))
      (func (export "save_state") (result i64) (i64.const {save}))
      (func (export "alloc_state") (param i32) (result i32) (i32.const {alloc}))
      (func (export "restore_state") (param i32 i32)))"#);
    for (save, alloc) in [("-1", "0"), ("0x7fffffffffffffff", "0"), ("0xffffffff00000001", "0"), ("0x0000ffff00000002", "0"),
        ("16", "-1"), ("16", "65535"), ("16", "65520"), ("65536", "0"), ("65537", "0"), ("0x8000000000000010", "0")] {
        let src = mk(save, alloc);
        let mut b = WasmScriptBackend::new(src.as_bytes(), &n, WasmConfig::engine()).unwrap();
        let r = b.apply_reload(src.as_bytes());
        println!("PROBE state save={save} alloc={alloc}: {r:?}");
    }
    // zero fuel / zero memory configs
    let src = mk("0", "0");
    let mut b = WasmScriptBackend::new(src.as_bytes(), &n, WasmConfig { memory_limit_bytes: 65536, fuel_per_callback: 0 }).unwrap();
    let mut h = H::new();
    h.tick(&mut b);
    println!("PROBE zero fuel: faulted={} {:?}", b.is_faulted(), b.last_error());
    let r = WasmScriptBackend::new(src.as_bytes(), &n, WasmConfig { memory_limit_bytes: 0, fuel_per_callback: 10 });
    println!("PROBE zero mem: {:?}", r.err());
    let r = WasmScriptBackend::new(src.as_bytes(), &n, WasmConfig { memory_limit_bytes: usize::MAX, fuel_per_callback: u64::MAX });
    println!("PROBE max cfg: ok={}", r.is_ok());
}

#[test]
fn probe_compile_bombs() {
    let cfg = WasmConfig::engine();
    let time = |name: &str, src: &[u8]| {
        let t = Instant::now();
        let r = WasmScriptBackend::prepare(src, cfg);
        println!("PROBE compile {name}: {} bytes -> {:?} in {:?}", src.len(), r.as_ref().map(|_| ()).map_err(|e| match e { ScriptError::Load(m) => m.chars().take(120).collect::<String>(), o => format!("{o:?}") }), t.elapsed());
    };
    let base = |extra: &str| format!(r#"(module
      (memory (export "memory") 1)
      {extra}
      (func (export "on_init"))
      (func (export "on_tick"))
      (func (export "on_switch_edge") (param i32 i32)))"#);
    // many small functions
    let mut s = String::new();
    for _ in 0..200_000 { s.push_str("(func (param i32) (result i32) (i32.add (local.get 0) (i32.const 1)))\n"); }
    time("200k funcs", base(&s).as_bytes());
    // one huge straight-line function
    let mut s = String::from("(func (result i64) (local $a i64) ");
    for i in 0..300_000 { s.push_str(&format!("(local.set $a (i64.mul (i64.add (local.get $a) (i64.const {i})) (i64.const 31)))\n")); }
    s.push_str("(local.get $a))");
    time("300k-stmt func", base(&s).as_bytes());
    // many locals + big br_table
    let mut s = String::from("(func (param i32) ");
    for _ in 0..2000 { s.push_str("(block "); }
    s.push_str("(br_table ");
    for i in 0..2000 { s.push_str(&format!("{i} ")); }
    s.push_str("0 (local.get 0))");
    for _ in 0..2000 { s.push_str(")"); }
    s.push_str(")");
    time("nested blocks", base(&s).as_bytes());
    // many globals
    let mut s = String::new();
    for _ in 0..500_000 { s.push_str("(global (mut i64) (i64.const 1))"); }
    time("500k globals", base(&s).as_bytes());
    // big data segment over cap? 2 MiB of data in 32 pages
    let s = format!("(data (i32.const 0) \"{}\")", "a".repeat(60000));
    time("data", base(&s).as_bytes());
    // 50k locals function with all used
    let mut s = String::from("(func (result i64) (local ");
    for _ in 0..49_000 { s.push_str("i64 "); }
    s.push_str(") ");
    for i in 0..49_000 { s.push_str(&format!("(local.set {i} (i64.const {i}))")); }
    s.push_str("(i64.const 0)");
    for i in 0..49_000 { s.push_str(&format!("(i64.add (local.get {i}))")); }
    s.push_str(")");
    time("49k locals", base(&s).as_bytes());
    // deep paren nesting in WAT
    let s = format!("(func (result i32) {} (i32.const 1) {})", "(i32.eqz ".repeat(100_000), ")".repeat(100_000));
    time("deep wat", base(&s).as_bytes());
}

#[test]
fn probe_many_backends() {
    let n = NameTables::default();
    let src = module(r#"(memory (export "memory") 1)
      (func (export "on_init")) (func (export "on_tick")) (func (export "on_switch_edge") (param i32 i32))"#);
    let mut v = Vec::new();
    let t = Instant::now();
    for i in 0..3000 {
        match WasmScriptBackend::new(src.as_bytes(), &n, WasmConfig::engine()) {
            Ok(b) => v.push(b),
            Err(e) => { println!("PROBE many: failed at {i}: {e:?}"); break; }
        }
    }
    println!("PROBE many: {} live backends in {:?}", v.len(), t.elapsed());
}
