//! Designs, the timing/fuel recorder, scenario playback with per-tick
//! hashes (state, supplementary, rules command queue) and per-callback
//! statistics. Shared by the native binary and the wasm-bindgen exports.

use crate::driver::Driver;
use crate::hashes::ext_hash;
use crate::io::{Scenario, encode_queue};
use crate::wasmi_backend::{self, Mode, WasmiBackend};
use pin_assets::{Vfs, ZipLimits};
use pin_core::ids::SwitchId;
use pin_core::{HostCtx, ScriptError, SwitchEdge, TableScriptBackend};
use pin_script::NameTables;
use pin_script::wasm::WasmConfig;
use pin_sim::hash::fnv1a64;
use pin_sim::{ExternalScript, SimConfig, Simulation};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

/// A rules backend the harness can probe.
pub trait Rules: ExternalScript {
    /// Fuel the last (non-skipped) callback used, in the backend's units.
    fn last_fuel(&self) -> u64;
    /// A copy of guest linear memory (adversarial checks).
    fn memory(&self) -> Vec<u8>;
}

impl Rules for WasmiBackend {
    fn last_fuel(&self) -> u64 {
        self.last_fuel
    }
    fn memory(&self) -> Vec<u8> {
        WasmiBackend::memory(self).to_vec()
    }
}

/// A rules engine design.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Design {
    /// The repo's wasmtime backend (native only).
    Native,
    /// A: wasmi with its own fuel.
    WasmiFuel,
    /// A: wasmi running the instrumented module (wasmtime-exact fuel).
    WasmiInstr,
    /// A: as `WasmiInstr` with the call-depth counter.
    WasmiInstrDepth,
    /// B: browser JIT running the instrumented module (wasm32 only).
    Jit,
    /// B with the call-depth counter.
    JitDepth,
    /// Native wasmtime running the instrumented module with the depth
    /// counter (what native would run if it adopted the counter).
    NativeInstrDepth,
    /// C: JIT for the three hash-pinned production modules, wasmi
    /// (instrumented, depth counter) for everything else.
    Hybrid,
}

impl Design {
    /// Parses a design name.
    #[must_use]
    pub fn parse(s: &str) -> Option<Design> {
        Some(match s {
            "native" => Design::Native,
            "wasmi-fuel" => Design::WasmiFuel,
            "wasmi-instr" => Design::WasmiInstr,
            "wasmi-instr-depth" => Design::WasmiInstrDepth,
            "jit" => Design::Jit,
            "jit-depth" => Design::JitDepth,
            "hybrid" => Design::Hybrid,
            "native-instr-depth" => Design::NativeInstrDepth,
            _ => return None,
        })
    }

    /// Its name.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Design::Native => "native",
            Design::WasmiFuel => "wasmi-fuel",
            Design::WasmiInstr => "wasmi-instr",
            Design::WasmiInstrDepth => "wasmi-instr-depth",
            Design::Jit => "jit",
            Design::JitDepth => "jit-depth",
            Design::Hybrid => "hybrid",
            Design::NativeInstrDepth => "native-instr-depth",
        }
    }
}

/// The weighted call-depth limit used by the `*-depth` designs and the
/// hybrid's interpreter path (see RESULTS §stack).
pub const DEPTH: (u32, crate::instrument::DepthWeight) =
    (256 * 1024, crate::instrument::DepthWeight::FrameBytes);

/// SHA-256 digests of the three committed production `rules.wasm` (the
/// hybrid's JIT allow-list).
pub const PINNED: [&str; 3] = [
    // apex_circuit, sky_deck, feature_lab (sha256sum tables/*/rules.wasm)
    "c7fb4b8ddc7bf19b",
    "a3fb121ea7f7b1c6",
    "40abca342ab1ebb3",
];

/// Loads `src` with `design`.
///
/// # Errors
/// The design's load error.
pub fn load(
    design: Design,
    src: &[u8],
    names: &NameTables,
    cfg: WasmConfig,
) -> Result<Box<dyn Rules>, ScriptError> {
    match design {
        #[cfg(feature = "native")]
        Design::Native => Ok(Box::new(crate::native::NativeRules::new(src, names, cfg)?)),
        #[cfg(not(feature = "native"))]
        Design::Native => Err(ScriptError::UnsupportedBackend(
            "native wasmtime not built".into(),
        )),
        Design::WasmiFuel | Design::WasmiInstr | Design::WasmiInstrDepth => {
            let mode = match design {
                Design::WasmiFuel => Mode::WasmiFuel,
                Design::WasmiInstr => Mode::Instrumented {
                    depth: None,
                    profile: false,
                },
                _ => Mode::Instrumented {
                    depth: Some(DEPTH),
                    profile: false,
                },
            };
            let p = wasmi_backend::prepare(src, &cfg, mode, true)?;
            Ok(Box::new(WasmiBackend::new(&p, names, cfg)?))
        }
        Design::Jit | Design::JitDepth => {
            #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
            {
                let depth = (design == Design::JitDepth).then_some(DEPTH);
                Ok(Box::new(crate::js_backend::JsBackend::new(
                    src, names, cfg, depth,
                )?))
            }
            #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
            {
                let _ = (src, names, cfg);
                Err(ScriptError::UnsupportedBackend(
                    "jit needs a browser engine".into(),
                ))
            }
        }
        Design::NativeInstrDepth => {
            #[cfg(feature = "native")]
            {
                let mode = Mode::Instrumented {
                    depth: Some(DEPTH),
                    profile: false,
                };
                let p = crate::wasmtime_instr::prepare(src, &cfg, mode, true)?;
                Ok(Box::new(crate::wasmtime_instr::WasmtimeInstr::new(
                    &p, names, cfg,
                )?))
            }
            #[cfg(not(feature = "native"))]
            Err(ScriptError::UnsupportedBackend(
                "native wasmtime not built".into(),
            ))
        }
        Design::Hybrid => {
            let h = crate::sha256::hex(src);
            if PINNED.iter().any(|p| h.starts_with(p)) {
                load(Design::JitDepth, src, names, cfg)
            } else {
                load(Design::WasmiInstrDepth, src, names, cfg)
            }
        }
    }
}

/// One callback sample.
#[derive(Copy, Clone, Debug)]
pub struct Sample {
    /// 0 init, 1 tick, 2 switch edge.
    pub kind: u8,
    /// Wall time, ns.
    pub ns: f32,
    /// Fuel used.
    pub fuel: u32,
}

/// What the recorder collected.
#[derive(Debug, Default)]
pub struct Probe {
    /// Per-callback samples (when timing).
    pub samples: Vec<Sample>,
    /// Largest fuel of one callback.
    pub max_fuel: u64,
    /// Sum of fuel.
    pub total_fuel: u64,
    /// Callbacks seen.
    pub callbacks: u64,
    /// FNV of every callback's fuel in order (fuel-accounting parity).
    pub fuel_digest: u64,
}

/// Wraps a backend: times each callback (if a clock is given) and records
/// the fuel it used.
pub struct Recorder {
    inner: Box<dyn Rules>,
    clock: Option<fn() -> f64>,
    probe: Arc<Mutex<Probe>>,
}

impl Recorder {
    /// Wraps `inner`.
    #[must_use]
    pub fn new(
        inner: Box<dyn Rules>,
        clock: Option<fn() -> f64>,
        cap: usize,
    ) -> (Self, Arc<Mutex<Probe>>) {
        let probe = Arc::new(Mutex::new(Probe {
            samples: Vec::with_capacity(cap),
            fuel_digest: 0xcbf2_9ce4_8422_2325,
            ..Probe::default()
        }));
        (
            Self {
                inner,
                clock,
                probe: Arc::clone(&probe),
            },
            probe,
        )
    }

    fn wrap(&mut self, kind: u8, f: impl FnOnce(&mut dyn Rules)) {
        let skipped = self.inner.is_faulted();
        let t0 = self.clock.map(|c| c());
        f(self.inner.as_mut());
        let t1 = self.clock.map(|c| c());
        if skipped {
            return;
        }
        let fuel = self.inner.last_fuel();
        if let Ok(mut p) = self.probe.lock() {
            p.callbacks += 1;
            p.total_fuel += fuel;
            p.max_fuel = p.max_fuel.max(fuel);
            for b in fuel.to_le_bytes() {
                p.fuel_digest = (p.fuel_digest ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
            }
            if let (Some(a), Some(b)) = (t0, t1)
                && p.samples.len() < p.samples.capacity()
            {
                p.samples.push(Sample {
                    kind,
                    ns: ((b - a) * 1e6) as f32,
                    fuel: fuel.min(u64::from(u32::MAX)) as u32,
                });
            }
        }
    }
}

impl TableScriptBackend for Recorder {
    fn on_init(&mut self, ctx: &mut HostCtx) {
        self.wrap(0, |b| b.on_init(ctx));
    }
    fn on_tick(&mut self, ctx: &mut HostCtx) {
        self.wrap(1, |b| b.on_tick(ctx));
    }
    fn on_switch_edge(&mut self, s: SwitchId, e: SwitchEdge, ctx: &mut HostCtx) {
        self.wrap(2, |b| b.on_switch_edge(s, e, ctx));
    }
    fn apply_reload(&mut self, src: &[u8]) -> Result<(), ScriptError> {
        self.inner.apply_reload(src)
    }
}

impl ExternalScript for Recorder {
    fn overflow_count(&self) -> u32 {
        self.inner.overflow_count()
    }
    fn is_faulted(&self) -> bool {
        self.inner.is_faulted()
    }
    fn fault_count(&self) -> u32 {
        self.inner.fault_count()
    }
    fn last_error(&self) -> Option<&ScriptError> {
        self.inner.last_error()
    }
}

/// Opens a zipped bundle from memory.
///
/// # Errors
/// The asset error text.
pub fn open_bundle(zip: &[u8]) -> Result<Vfs, String> {
    Vfs::from_zip_bytes(zip.to_vec(), ZipLimits::default()).map_err(|e| e.to_string())
}

/// Loads a bundle with a design (`Native` uses the engine's own path,
/// `Simulation::load_from_vfs`, i.e. `Script::Wasm`).
///
/// # Errors
/// Load failures.
pub fn load_sim(
    zip: &[u8],
    seed: u64,
    design: Option<Design>,
    clock: Option<fn() -> f64>,
    cap: usize,
) -> Result<(Simulation, Option<Arc<Mutex<Probe>>>), String> {
    let vfs = open_bundle(zip)?;
    let cfg = SimConfig {
        seed,
        ..SimConfig::default()
    };
    match design {
        None => {
            #[cfg(feature = "native")]
            {
                let sim = Simulation::load_from_vfs(&vfs, cfg).map_err(|e| e.to_string())?;
                Ok((sim, None))
            }
            #[cfg(not(feature = "native"))]
            Err("engine path needs the native feature".into())
        }
        Some(d) => {
            let mut probe = None;
            let wasm = cfg.wasm;
            let sim = Simulation::load_from_vfs_with_script(&vfs, cfg, &mut |_e, src, names| {
                let b = load(d, src, names, wasm)?;
                let (r, p) = Recorder::new(b, clock, cap);
                probe = Some(p);
                Ok(Box::new(r) as Box<dyn ExternalScript>)
            })
            .map_err(|e| e.to_string())?;
            Ok((sim, probe))
        }
    }
}

/// Per-tick hash lines and a summary.
#[derive(Debug, Default)]
pub struct PlayOut {
    /// `tick state_hash ext_hash cmd_hash` lines.
    pub lines: String,
    /// Summary line.
    pub summary: String,
}

/// FNV over the encoded last command queue.
fn cmd_hash(sim: &Simulation, buf: &mut Vec<u8>) -> u64 {
    buf.clear();
    encode_queue(sim.last_commands(), buf);
    fnv1a64(buf)
}

/// Plays `scn` on `zip` with `design` (`None` = engine path), every tick a
/// line.
///
/// # Errors
/// Load and scenario failures.
pub fn play(
    zip: &[u8],
    scn_bytes: &[u8],
    design: Option<Design>,
    every: u64,
) -> Result<PlayOut, String> {
    let scn = Scenario::from_bytes(scn_bytes)?;
    let (mut sim, probe) = load_sim(zip, scn.seed, design, None, 0)?;
    let mut drv = Driver::new(&scn);
    let mut out = String::with_capacity(64 * 64 * 1024);
    let mut buf = Vec::with_capacity(4096);
    let mut cmd_all: u64 = 0xcbf2_9ce4_8422_2325;
    let line =
        |sim: &Simulation, buf: &mut Vec<u8>, cmd_all: &mut u64, out: &mut String, write: bool| {
            let c = cmd_hash(sim, buf);
            for b in c.to_le_bytes() {
                *cmd_all = (*cmd_all ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
            }
            if write {
                let _ = writeln!(
                    out,
                    "{} {:016x} {:016x} {:016x}",
                    sim.tick(),
                    sim.state_hash(),
                    ext_hash(sim),
                    c
                );
            }
        };
    line(&sim, &mut buf, &mut cmd_all, &mut out, true);
    let mut max_balls = 0;
    while sim.tick() < scn.ticks {
        let t = drv.advance(&mut sim)?;
        max_balls = max_balls.max(sim.balls_in_play());
        line(
            &sim,
            &mut buf,
            &mut cmd_all,
            &mut out,
            every <= 1 || t % every == 0 || t == scn.ticks,
        );
    }
    let (mf, cb, fd) = match &probe {
        Some(p) => {
            let p = p.lock().map_err(|_| "probe")?;
            (p.max_fuel, p.callbacks, p.fuel_digest)
        }
        None => (0, 0, 0),
    };
    let summary = format!(
        "design={} ticks={} state_hash={:016x} ext_hash={:016x} cmd_stream={:016x} table_hash={:016x} faulted={} fault={:?} overflow={} max_balls={} score={:?} callbacks={} max_fuel={} fuel_digest={:016x}",
        design.map_or("engine", Design::name),
        sim.tick(),
        sim.state_hash(),
        ext_hash(&sim),
        cmd_all,
        sim.table_hash(),
        sim.script_faulted(),
        sim.script_error().map(ToString::to_string),
        sim.script_queue_overflows(),
        max_balls,
        sim.score(0),
        cb,
        mf,
        fd
    );
    Ok(PlayOut {
        lines: out,
        summary,
    })
}

/// Summary statistics, µs.
#[must_use]
pub fn stats(v: &mut [f64]) -> String {
    if v.is_empty() {
        return "{\"n\":0}".into();
    }
    v.sort_by(f64::total_cmp);
    let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    format!(
        "{{\"n\":{},\"mean_us\":{:.3},\"p50_us\":{:.3},\"p99_us\":{:.3},\"p999_us\":{:.3},\"max_us\":{:.3}}}",
        v.len(),
        mean / 1e3,
        q(0.5) / 1e3,
        q(0.99) / 1e3,
        q(0.999) / 1e3,
        v[v.len() - 1] / 1e3
    )
}

/// Times every callback of a scenario run; returns JSON with per-kind
/// statistics, fuel figures, load time and the whole-run time.
///
/// # Errors
/// Load and scenario failures.
pub fn bench(
    zip: &[u8],
    scn_bytes: &[u8],
    design: Design,
    clock: fn() -> f64,
) -> Result<String, String> {
    let scn = Scenario::from_bytes(scn_bytes)?;
    let t0 = clock();
    let (mut sim, probe) = load_sim(
        zip,
        scn.seed,
        Some(design),
        Some(clock),
        3 * scn.ticks as usize + 10_000,
    )?;
    let load_ms = clock() - t0;
    let probe = probe.ok_or("no probe")?;
    let mut drv = Driver::new(&scn);
    let t1 = clock();
    while sim.tick() < scn.ticks {
        drv.advance(&mut sim)?;
    }
    let run_ms = clock() - t1;
    let p = probe.lock().map_err(|_| "probe")?;
    let mut by = [Vec::new(), Vec::new(), Vec::new()];
    let mut fuel_by = [0u64; 3];
    for s in &p.samples {
        by[usize::from(s.kind)].push(f64::from(s.ns));
        fuel_by[usize::from(s.kind)] = fuel_by[usize::from(s.kind)].max(u64::from(s.fuel));
    }
    let mut all: Vec<f64> = p.samples.iter().map(|s| f64::from(s.ns)).collect();
    let sum_rules_ms = all.iter().sum::<f64>() / 1e6;
    Ok(format!(
        "{{\"design\":\"{}\",\"ticks\":{},\"load_ms\":{:.3},\"run_ms\":{:.3},\"rules_ms\":{:.3},\"callbacks\":{},\"max_fuel\":{},\"mean_fuel\":{:.1},\"max_fuel_init\":{},\"max_fuel_tick\":{},\"max_fuel_switch\":{},\"all\":{},\"init\":{},\"tick\":{},\"switch\":{},\"state_hash\":\"{:016x}\"}}",
        design.name(),
        scn.ticks,
        load_ms,
        run_ms,
        sum_rules_ms,
        p.callbacks,
        p.max_fuel,
        p.total_fuel as f64 / p.callbacks.max(1) as f64,
        fuel_by[0],
        fuel_by[1],
        fuel_by[2],
        stats(&mut all),
        stats(&mut by[0]),
        stats(&mut by[1]),
        stats(&mut by[2]),
        sim.state_hash()
    ))
}

/// Allocation guard over a scenario with `design` (D5 check of the
/// backend's host side; the wasmi interpreter is not exempt).
///
/// # Errors
/// Load and scenario failures.
pub fn alloc_check(zip: &[u8], scn_bytes: &[u8], design: Design) -> Result<String, String> {
    let scn = Scenario::from_bytes(scn_bytes)?;
    let vfs = open_bundle(zip)?;
    let cfg = SimConfig {
        seed: scn.seed,
        ..SimConfig::default()
    };
    let wasm = cfg.wasm;
    let mut sim = Simulation::load_from_vfs_with_script(&vfs, cfg, &mut |_e, src, names| {
        let b = load(design, src, names, wasm)?;
        Ok(Box::new(Bare(b)) as Box<dyn ExternalScript>)
    })
    .map_err(|e| e.to_string())?;
    let mut drv = Driver::new(&scn);
    let (mut allocs, mut first) = (0u64, None);
    while sim.tick() < scn.ticks {
        let t = drv.prepare(&mut sim)?;
        let ((), a, _) = pin_sim::alloc_guard::count_allocs_split(|| drv.step(&mut sim, t));
        if a > 0 && first.is_none() {
            first = Some(t);
        }
        allocs += a;
    }
    Ok(format!(
        "design={} alloc_guard installed={} ticks={} allocs_in_step={allocs} first_alloc_tick={first:?} state_hash={:016x}",
        design.name(),
        pin_sim::alloc_guard::is_installed(),
        sim.tick(),
        sim.state_hash()
    ))
}

/// A `Rules` box as an `ExternalScript` without recording.
struct Bare(Box<dyn Rules>);

impl TableScriptBackend for Bare {
    fn on_init(&mut self, ctx: &mut HostCtx) {
        self.0.on_init(ctx);
    }
    fn on_tick(&mut self, ctx: &mut HostCtx) {
        self.0.on_tick(ctx);
    }
    fn on_switch_edge(&mut self, s: SwitchId, e: SwitchEdge, ctx: &mut HostCtx) {
        self.0.on_switch_edge(s, e, ctx);
    }
    fn apply_reload(&mut self, src: &[u8]) -> Result<(), ScriptError> {
        self.0.apply_reload(src)
    }
}

impl ExternalScript for Bare {
    fn overflow_count(&self) -> u32 {
        self.0.overflow_count()
    }
    fn is_faulted(&self) -> bool {
        self.0.is_faulted()
    }
    fn fault_count(&self) -> u32 {
        self.0.fault_count()
    }
    fn last_error(&self) -> Option<&ScriptError> {
        self.0.last_error()
    }
}

/// Deepest call depth the production rules reach over a scenario (wasmi,
/// instrumented with an unlimited counter), in calls and in estimated
/// frame bytes.
///
/// # Errors
/// Load and scenario failures.
pub fn max_depth(zip: &[u8], scn_bytes: &[u8]) -> Result<String, String> {
    use crate::instrument::DepthWeight;
    use std::sync::atomic::{AtomicI32, Ordering};
    struct DepthProbe(WasmiBackend, Arc<AtomicI32>);
    impl TableScriptBackend for DepthProbe {
        fn on_init(&mut self, ctx: &mut HostCtx) {
            self.0.on_init(ctx);
            self.1
                .fetch_max(self.0.max_depth().unwrap_or(0), Ordering::Relaxed);
        }
        fn on_tick(&mut self, ctx: &mut HostCtx) {
            self.0.on_tick(ctx);
            self.1
                .fetch_max(self.0.max_depth().unwrap_or(0), Ordering::Relaxed);
        }
        fn on_switch_edge(&mut self, s: SwitchId, e: SwitchEdge, ctx: &mut HostCtx) {
            self.0.on_switch_edge(s, e, ctx);
            self.1
                .fetch_max(self.0.max_depth().unwrap_or(0), Ordering::Relaxed);
        }
        fn apply_reload(&mut self, src: &[u8]) -> Result<(), ScriptError> {
            self.0.apply_reload(src)
        }
    }
    impl ExternalScript for DepthProbe {}
    let scn = Scenario::from_bytes(scn_bytes)?;
    let mut out = String::new();
    for w in [DepthWeight::Calls, DepthWeight::FrameBytes] {
        let vfs = open_bundle(zip)?;
        let cfg = SimConfig {
            seed: scn.seed,
            ..SimConfig::default()
        };
        let wasm = cfg.wasm;
        let seen = Arc::new(AtomicI32::new(0));
        let s2 = Arc::clone(&seen);
        let mut sim = Simulation::load_from_vfs_with_script(&vfs, cfg, &mut |_e, src, names| {
            let p = wasmi_backend::prepare(
                src,
                &wasm,
                Mode::Instrumented {
                    depth: Some((i32::MAX as u32, w)),
                    profile: true,
                },
                true,
            )?;
            Ok(Box::new(DepthProbe(
                WasmiBackend::new(&p, names, wasm)?,
                Arc::clone(&s2),
            )) as Box<dyn ExternalScript>)
        })
        .map_err(|e| e.to_string())?;
        let mut drv = Driver::new(&scn);
        while sim.tick() < scn.ticks {
            drv.advance(&mut sim)?;
        }
        let _ = writeln!(
            out,
            "weight={w:?} max_depth={} state_hash={:016x}",
            seen.load(Ordering::Relaxed),
            sim.state_hash()
        );
    }
    Ok(out)
}

#[cfg(feature = "native")]
impl Rules for crate::wasmtime_instr::WasmtimeInstr {
    fn last_fuel(&self) -> u64 {
        self.last_fuel
    }
    fn memory(&self) -> Vec<u8> {
        crate::wasmtime_instr::WasmtimeInstr::memory(self).to_vec()
    }
}
