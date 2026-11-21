//! wasm-bindgen exports (wasm32-unknown-unknown).

use crate::run::{self, Design};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn perf_now() -> f64;
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn console_error(s: &str);
}

fn now_ms() -> f64 {
    perf_now()
}

fn design(s: &str) -> Result<Design, JsValue> {
    Design::parse(s).ok_or_else(|| JsValue::from_str(&format!("unknown design {s}")))
}

/// Plays a scenario; returns the summary line then the hash lines.
#[wasm_bindgen]
pub fn play(d: &str, bundle: &[u8], scn: &[u8], every: u32) -> Result<String, JsValue> {
    let o = run::play(bundle, scn, Some(design(d)?), u64::from(every))
        .map_err(|e| JsValue::from_str(&e))?;
    Ok(format!("{}\n{}", o.summary, o.lines))
}

/// Per-callback timing of a scenario; JSON.
#[wasm_bindgen]
pub fn bench(d: &str, bundle: &[u8], scn: &[u8]) -> Result<String, JsValue> {
    run::bench(bundle, scn, design(d)?, now_ms).map_err(|e| JsValue::from_str(&e))
}

/// Allocation guard over a scenario.
#[wasm_bindgen]
pub fn alloc_check(d: &str, bundle: &[u8], scn: &[u8]) -> Result<String, JsValue> {
    run::alloc_check(bundle, scn, design(d)?).map_err(|e| JsValue::from_str(&e))
}

/// The adversarial corpus on a design.
#[wasm_bindgen]
pub fn adversarial(d: &str, prod: &[u8]) -> Result<String, JsValue> {
    Ok(crate::adversarial::run_all(design(d)?, prod))
}

/// One adversarial case (and its boundary line).
#[wasm_bindgen]
pub fn adv_case(d: &str, name: &str, prod: &[u8]) -> Result<String, JsValue> {
    let d = design(d)?;
    let mut out = String::new();
    for c in crate::adversarial::corpus(prod) {
        if c.name == name {
            out.push_str(&crate::adversarial::run_case(d, &c));
            out.push('\n');
            out.push_str(&crate::adversarial::boundary(d, &c));
        }
    }
    Ok(out)
}

/// Budget-exhausting callbacks; one line per case.
#[wasm_bindgen]
pub fn worst(d: &str, reps: u32, only: &str) -> Result<String, JsValue> {
    let only = (!only.is_empty()).then_some(only);
    Ok(crate::worst::run(design(d)?, now_ms, reps as usize, only))
}

/// Load (compile + instantiate) time of a rules module.
#[wasm_bindgen]
pub fn load_time(d: &str, bytes: &[u8], reps: u32) -> Result<String, JsValue> {
    Ok(crate::worst::load_time(
        design(d)?,
        bytes,
        &Default::default(),
        now_ms,
        reps as usize,
    ))
}

/// Bundle hardening check through `Vfs::from_zip_bytes`.
#[wasm_bindgen]
pub fn zip_check(bytes: &[u8], web: bool) -> String {
    crate::zipcheck::check(bytes, web)
}

/// The JIT-mode instrumented module (async-compile path and probes).
#[wasm_bindgen]
pub fn instrument_jit(bytes: &[u8], depth: bool) -> Result<Vec<u8>, JsValue> {
    let cfg = pin_script::wasm::WasmConfig::engine();
    let (bin, _) =
        crate::abi::prepare(bytes, &cfg).map_err(|e| JsValue::from_str(&e.to_string()))?;
    let mut ic = crate::instrument::Config::engine(cfg.fuel_per_callback, cfg.memory_limit_bytes);
    ic.host_err_import = true;
    ic.nan_canon = true;
    if depth {
        ic.depth_limit = Some(run::DEPTH);
    }
    crate::instrument::instrument(&bin, &ic)
        .map(|o| o.bytes)
        .map_err(|e| JsValue::from_str(&e))
}

/// Panic messages to `console.error`.
#[wasm_bindgen]
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| console_error(&format!("s4 panic: {info}"))));
}

/// Recursion-depth limits per frame shape (bisection, `reps` repeats).
#[wasm_bindgen]
pub fn stack_probe(d: &str, reps: u32) -> Result<String, JsValue> {
    Ok(crate::adversarial::stack_probe(design(d)?, reps as usize))
}
