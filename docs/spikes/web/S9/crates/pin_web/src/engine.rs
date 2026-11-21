//! The engine core of the S9 module (target independent): bundle bytes →
//! `Vfs::from_zip_bytes` (web limits) → `Simulation` with the real Lua
//! backend (`.lua` entry) or the wasmi backend running the instrumented
//! module (`.wasm` entry, S4 design A), fixed-step stepping of a recorded
//! scenario (replay) or of live input, and a per-tick parity check against
//! native reference hashes (`state_hash`, S1's `ext_hash`, S4's
//! command-queue hash).

use arrayvec::ArrayVec;
use pin_assets::{Vfs, ZipLimits};
use pin_core::ids::SwitchId;
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::sim::SimStep;
use pin_core::snapshot::FrameStateSnapshot;
use pin_core::{HostCtx, ScriptError, SwitchEdge, TICK_NS, TableScriptBackend};
use pin_script::lua::LuaScriptBackend;
use pin_sim::hash::fnv1a64;
use pin_sim::{ExternalScript, SimConfig, Simulation};
use s1_harness::driver::Driver;
use s1_harness::hashes::ext_hash;
use s1_harness::io::{Scenario, encode_queue};
use s4_rules::run::{Design, Probe, Recorder};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

/// Largest bundle file the page copies into linear memory (S4 §8: checked
/// on `File.size` in JS before any copy, and again here).
pub const WEB_MAX_BUNDLE_BYTES: usize = 32 << 20;

/// Live-play seed (the soak seed of S1's recordings).
pub const LIVE_SEED: u64 = 0x004B_485A;

/// Most live input events queued for one tick.
pub const MAX_LIVE_INPUTS: usize = 64;

/// The web zip limits (S4 §8): 64 MiB total, 32 MiB per file.
#[must_use]
pub fn web_limits() -> ZipLimits {
    s4_rules::zipcheck::web_limits()
}

/// The engine's Lua backend as an [`ExternalScript`] (live play): the real
/// `LuaScriptBackend`, its VM allocations exempt from the D5 guard like
/// pin_sim's native `Script::Lua`.
pub struct LuaRules(pub LuaScriptBackend);

impl TableScriptBackend for LuaRules {
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

impl ExternalScript for LuaRules {
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
    fn alloc_exempt(&self) -> bool {
        true
    }
}

/// Native reference hashes: one row per tick from tick 0, `cols` u64 each
/// (`state ext` or `state ext cmd`). File: `S9REF\0\0\0`, u32 cols, u32 0,
/// u64 rows, rows (little endian).
pub struct Refs {
    cols: usize,
    rows: Vec<u64>,
}

impl Refs {
    /// Parses the binary reference file.
    ///
    /// # Errors
    /// A malformed file.
    pub fn from_bytes(b: &[u8]) -> Result<Refs, String> {
        if b.len() < 24 || &b[..8] != b"S9REF\0\0\0" {
            return Err("not an S9REF file".into());
        }
        let cols = u32::from_le_bytes(b[8..12].try_into().map_err(|_| "cols")?) as usize;
        let n = u64::from_le_bytes(b[16..24].try_into().map_err(|_| "rows")?) as usize;
        if !(2..=3).contains(&cols) || b.len() != 24 + n * cols * 8 {
            return Err(format!(
                "bad S9REF size (cols {cols}, rows {n}, {} B)",
                b.len()
            ));
        }
        let rows = b[24..]
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap_or_default()))
            .collect();
        Ok(Refs { cols, rows })
    }

    fn row(&self, tick: u64) -> Option<&[u64]> {
        let i = usize::try_from(tick).ok()? * self.cols;
        self.rows.get(i..i + self.cols)
    }

    fn len(&self) -> usize {
        self.rows.len() / self.cols
    }
}

/// Per-tick comparison counters.
#[derive(Debug, Default, Clone)]
pub struct Compare {
    /// Ticks compared (incl. tick 0).
    pub compared: u64,
    /// Ticks whose `state_hash` differed.
    pub state_bad: u64,
    /// Ticks whose `ext_hash` differed.
    pub ext_bad: u64,
    /// Ticks whose command-queue hash differed (wasm references only).
    pub cmd_bad: u64,
    /// First tick with any difference.
    pub first_bad: Option<u64>,
    /// Ticks beyond the reference.
    pub beyond: u64,
}

/// A replay in progress.
struct Replay {
    drv: Driver,
    refs: Option<Refs>,
    cmp: Compare,
    scn_ticks: u64,
}

/// Live input state.
struct Live {
    pending: ArrayVec<InputEvent, MAX_LIVE_INPUTS>,
    out: FrameStateSnapshot,
    events: u64,
}

/// A loaded table.
pub struct Engine {
    /// The simulation (boxed: 61.5 KB, S1 §11 (iii) 6).
    pub sim: Box<Simulation>,
    /// The bundle's VFS (the renderer reads its visuals).
    pub vfs: Vfs,
    /// `lua`, `wasm` or `none`.
    pub rules: &'static str,
    /// The manifest's script entry.
    pub entry: String,
    #[cfg(feature = "harness")]
    lua_check: Option<Arc<Mutex<s2_harness::parity::Check>>>,
    probe: Option<Arc<Mutex<Probe>>>,
    replay: Option<Replay>,
    live: Live,
    cmd_buf: Vec<u8>,
    stream: u64,
    /// Bytes of the bundle file.
    pub bundle_bytes: usize,
    /// Most balls in play seen.
    pub max_balls: usize,
}

/// FNV over the encoded last command queue (S4's `cmd_hash`).
fn cmd_hash(sim: &Simulation, buf: &mut Vec<u8>) -> u64 {
    buf.clear();
    encode_queue(sim.last_commands(), buf);
    fnv1a64(buf)
}

/// Load options.
pub struct LoadOpts {
    /// Sim seed.
    pub seed: u64,
    /// S1's recorded script-output stream: wraps the Lua backend in S2's
    /// per-step checker.
    pub scr: Option<Vec<u8>>,
    /// The wasm rules design (S4); `WasmiInstrDepth` is the recommended one.
    pub design: Design,
}

impl Engine {
    /// Opens `bytes` with the web limits and loads the table with its
    /// manifest's script entry. The error text is the loader's own
    /// (identical to native for the same bytes).
    ///
    /// # Errors
    /// Bundle, validation and script-load failures.
    pub fn load(bytes: Vec<u8>, opts: LoadOpts) -> Result<Engine, String> {
        let bundle_bytes = bytes.len();
        if bundle_bytes > WEB_MAX_BUNDLE_BYTES {
            return Err(format!(
                "bundle file is too large ({bundle_bytes} bytes; the web limit is {WEB_MAX_BUNDLE_BYTES})"
            ));
        }
        let vfs = Vfs::from_zip_bytes(bytes, web_limits()).map_err(|e| e.to_string())?;
        if let Some(s) = &opts.scr {
            s1_harness::io::check_script_stream(s)?;
        }
        let cfg = SimConfig {
            seed: opts.seed,
            ..SimConfig::default()
        };
        let (lua_cfg, wasm_cfg) = (cfg.lua, cfg.wasm);
        let mut rules = "none";
        let mut entry_name = String::new();
        #[cfg(feature = "harness")]
        let mut lua_check = None;
        let mut probe = None;
        #[cfg_attr(not(feature = "harness"), allow(unused_mut, unused_variables))]
        let mut scr = opts.scr;
        let design = opts.design;
        let sim = Simulation::load_from_vfs_with_script(&vfs, cfg, &mut |entry, src, names| {
            entry_name = entry.to_owned();
            if entry.ends_with(".lua") {
                rules = "lua";
                let b = LuaScriptBackend::new(src, names, lua_cfg)?;
                #[cfg(feature = "harness")]
                if let Some(s) = scr.take() {
                    let (c, h) =
                        s2_harness::parity::CheckingScript::new(b, s).map_err(ScriptError::Load)?;
                    lua_check = Some(h);
                    return Ok(Box::new(c) as Box<dyn ExternalScript>);
                }
                Ok(Box::new(LuaRules(b)) as Box<dyn ExternalScript>)
            } else if entry.ends_with(".wasm") {
                rules = "wasm";
                let b = s4_rules::run::load(design, src, names, wasm_cfg)?;
                let (r, p) = Recorder::new(b, None, 0);
                probe = Some(p);
                Ok(Box::new(r) as Box<dyn ExternalScript>)
            } else {
                Err(ScriptError::UnsupportedBackend(entry.to_owned()))
            }
        })
        .map_err(|e| e.to_string())?;
        Ok(Engine {
            sim: Box::new(sim),
            vfs,
            rules,
            entry: entry_name,
            #[cfg(feature = "harness")]
            lua_check,
            probe,
            replay: None,
            live: Live {
                pending: ArrayVec::new(),
                out: FrameStateSnapshot::default(),
                events: 0,
            },
            cmd_buf: Vec::with_capacity(1 << 16),
            stream: 0xcbf2_9ce4_8422_2325,
            bundle_bytes,
            max_balls: 0,
        })
    }

    /// Attaches a recorded scenario (and optional native references) to a
    /// freshly loaded table and compares tick 0.
    ///
    /// # Errors
    /// Malformed scenario or reference files.
    pub fn attach_replay(&mut self, scn: &[u8], refs: Option<&[u8]>) -> Result<(), String> {
        let scn = Scenario::from_bytes(scn)?;
        let refs = refs.map(Refs::from_bytes).transpose()?;
        self.replay = Some(Replay {
            drv: Driver::new(&scn),
            refs,
            cmp: Compare::default(),
            scn_ticks: scn.ticks,
        });
        self.check_tick();
        Ok(())
    }

    /// The scenario seed of `scn` (the table must be loaded with it).
    ///
    /// # Errors
    /// Malformed scenario.
    pub fn scenario_seed(scn: &[u8]) -> Result<u64, String> {
        Ok(Scenario::from_bytes(scn)?.seed)
    }

    fn check_tick(&mut self) {
        let c = cmd_hash(&self.sim, &mut self.cmd_buf);
        let st = self.sim.state_hash();
        let ex = ext_hash(&self.sim);
        for v in [st, ex, c] {
            for b in v.to_le_bytes() {
                self.stream = (self.stream ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        let tick = self.sim.tick();
        let Some(r) = &mut self.replay else { return };
        let Some(refs) = &r.refs else { return };
        let Some(row) = refs.row(tick) else {
            r.cmp.beyond += 1;
            return;
        };
        r.cmp.compared += 1;
        let mut bad = false;
        if row[0] != st {
            r.cmp.state_bad += 1;
            bad = true;
        }
        if row[1] != ex {
            r.cmp.ext_bad += 1;
            bad = true;
        }
        if refs.cols == 3 && row[2] != c {
            r.cmp.cmd_bad += 1;
            bad = true;
        }
        if bad && r.cmp.first_bad.is_none() {
            r.cmp.first_bad = Some(tick);
        }
    }

    /// Queues a live input for the next tick.
    pub fn push_input(&mut self, input: CabinetInput, pressed: bool) {
        let ts = self.sim.tick() * TICK_NS + TICK_NS / 2;
        if self
            .live
            .pending
            .try_push(InputEvent::button(ts, input, pressed))
            .is_ok()
        {
            self.live.events += 1;
        }
    }

    /// Runs up to `n` ticks (replay: until the scenario ends); returns the
    /// ticks run.
    ///
    /// # Errors
    /// A replay operation failed.
    pub fn step(&mut self, n: u32) -> Result<u32, String> {
        let mut done = 0;
        for _ in 0..n {
            if let Some(r) = &mut self.replay {
                if self.sim.tick() >= r.scn_ticks {
                    break;
                }
                r.drv.advance(&mut self.sim)?;
            } else {
                let tick = self.sim.tick() + 1;
                self.sim
                    .step_1ms(tick, &self.live.pending, &mut self.live.out);
                self.live.pending.clear();
            }
            self.max_balls = self.max_balls.max(self.sim.balls_in_play());
            self.check_tick();
            done += 1;
        }
        Ok(done)
    }

    /// Whether a replay is attached and finished.
    #[must_use]
    pub fn replay_done(&self) -> bool {
        self.replay
            .as_ref()
            .is_some_and(|r| self.sim.tick() >= r.scn_ticks)
    }

    /// The snapshot of the last tick (the renderer's input).
    #[must_use]
    pub fn snapshot(&self) -> &FrameStateSnapshot {
        match &self.replay {
            Some(r) => r.drv.snapshot(),
            None => &self.live.out,
        }
    }

    /// Status as JSON text.
    #[must_use]
    pub fn status(&self) -> String {
        let mut s = String::with_capacity(512);
        let _ = write!(
            s,
            "{{\"tick\":{},\"rules\":\"{}\",\"entry\":\"{}\",\"state_hash\":\"{:016x}\",\"ext_hash\":\"{:016x}\",\"stream\":\"{:016x}\",\"table_hash\":\"{:016x}\",\"balls\":{},\"max_balls\":{},\"score\":{},\"faulted\":{},\"fault_count\":{},\"error\":{},\"overflow\":{},\"live_events\":{}",
            self.sim.tick(),
            self.rules,
            self.entry,
            self.sim.state_hash(),
            ext_hash(&self.sim),
            self.stream,
            self.sim.table_hash(),
            self.sim.balls_in_play(),
            self.max_balls,
            self.sim
                .score(0)
                .map_or("null".to_string(), |v| v.to_string()),
            self.sim.script_faulted(),
            self.sim.script_fault_count(),
            serde_json::to_string(&self.sim.script_error().map(ToString::to_string))
                .unwrap_or_default(),
            self.sim.script_queue_overflows(),
            self.live.events,
        );
        let fl: Vec<String> = self
            .snapshot()
            .flippers()
            .iter()
            .map(|f| format!("{:.4}", f.angle_rad))
            .collect();
        let _ = write!(s, ",\"flippers\":[{}]", fl.join(","));
        if let Some(r) = &self.replay {
            let c = &r.cmp;
            let _ = write!(
                s,
                ",\"replay\":{{\"ticks\":{},\"done\":{},\"refs\":{},\"ref_cols\":{},\"compared\":{},\"state_bad\":{},\"ext_bad\":{},\"cmd_bad\":{},\"first_bad\":{},\"beyond\":{},\"ops_left\":{}}}",
                r.scn_ticks,
                self.sim.tick() >= r.scn_ticks,
                r.refs.as_ref().map_or(0, Refs::len),
                r.refs.as_ref().map_or(0, |x| x.cols),
                c.compared,
                c.state_bad,
                c.ext_bad,
                c.cmd_bad,
                c.first_bad.map_or("null".to_string(), |t| t.to_string()),
                c.beyond,
                r.drv.ops_left()
            );
        }
        #[cfg(feature = "harness")]
        if let Some(h) = &self.lua_check
            && let Ok(c) = h.lock()
        {
            let _ = write!(
                s,
                ",\"lua_stream\":{{\"steps\":{},\"tick_mismatch\":{},\"draw_mismatch\":{},\"digest_mismatch\":{},\"queue_mismatch\":{},\"first_mismatch\":{},\"exhausted\":{},\"queue_bytes\":{}}}",
                c.steps,
                c.tick_mismatch,
                c.draw_mismatch,
                c.digest_mismatch,
                c.queue_mismatch,
                c.first_mismatch
                    .map_or("null".to_string(), |t| t.to_string()),
                c.exhausted,
                c.queue_bytes
            );
        }
        if let Some(p) = &self.probe
            && let Ok(p) = p.lock()
        {
            let _ = write!(
                s,
                ",\"wasm_rules\":{{\"callbacks\":{},\"max_fuel\":{},\"total_fuel\":{},\"fuel_digest\":\"{:016x}\"}}",
                p.callbacks, p.max_fuel, p.total_fuel, p.fuel_digest
            );
        }
        s.push('}');
        s
    }
}

/// Parses a cabinet input name.
#[must_use]
pub fn cabinet_input(name: &str) -> Option<CabinetInput> {
    Some(match name {
        "left" => CabinetInput::LeftFlipper,
        "right" => CabinetInput::RightFlipper,
        "left_upper" => CabinetInput::LeftUpper,
        "right_upper" => CabinetInput::RightUpper,
        "action" => CabinetInput::Action,
        "start" => CabinetInput::Start,
        "launch" => CabinetInput::Launch,
        "tilt" => CabinetInput::Tilt,
        _ => return None,
    })
}
