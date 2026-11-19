//! Shared helpers of the 6.3 Wasm-port parity tests (`wasm_ports_*.rs`).
//!
//! Each table is checked at two levels, both against its frozen
//! `rules.lua`:
//!
//! * **Simulation level** ([`replay_pair`]): two bundles built from the same
//!   sources, one with the Lua rules and one whose `table.toml` (in the
//!   bundle copy only) names `rules.wasm`, are stepped tick by tick with the
//!   same recorded input events. After every tick everything the
//!   simulation exposes of the drained command queue (display scenes,
//!   sounds and score reports in queue order, lamp intensities, the GI
//!   tint, every persist slot, fault and overflow state) and the full
//!   `state_hash` (balls, switches, scores, flippers, RNG) must be equal.
//! * **Queue level** ([`Lockstep`]): a Lua and a Wasm backend of the same
//!   table are fed identical switch edges and host views, each with its own
//!   copy of the seeded PCG, and the `HostCommandQueue` each produces per
//!   tick is compared byte for byte ([`queue_bytes`]), together with the
//!   PCG state (the order of `random` draws). `pin_sim` does not expose its
//!   drained queue, so this is where the exact per-tick queues are compared.
#![allow(dead_code)]

use pin_core::ids::SwitchId;
use pin_core::script::MAX_PERSIST;
use pin_core::{CabinetInput, TICK_NS};
use pin_core::{
    HostCommandQueue, HostCtx, HostView, InputEvent, Pcg32, Rng, SeedableRng, SimStep, SwitchEdge,
    TableScriptBackend,
};
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use pin_sim::replay::Recording;
use pin_sim::{SimConfig, Simulation};
use std::path::{Path, PathBuf};
use std::process::Command;

/// `tables/<table>` (the sources).
pub fn table_src(table: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tables")
        .join(table)
}

/// The committed `tables/<table>/rules.wasm`.
pub fn committed_wasm(table: &str) -> Vec<u8> {
    let p = table_src(table).join("rules.wasm");
    std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    let mut entries: Vec<_> = std::fs::read_dir(from)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        let dest = to.join(p.file_name().unwrap());
        if p.is_dir() {
            copy_dir(&p, &dest);
        } else {
            std::fs::copy(&p, &dest).unwrap();
        }
    }
}

/// Copies the Lua bundle `lua_dir` to `out`, adds the committed
/// `rules.wasm` and rewrites `script_entry` to `"rules.wasm"` in the copy's
/// `table.toml` (the sources are not touched: Lua stays the default).
pub fn wasm_bundle(lua_dir: &Path, table: &str, out: &Path) {
    copy_dir(lua_dir, out);
    std::fs::write(out.join("rules.wasm"), committed_wasm(table)).unwrap();
    let toml_path = out.join("table.toml");
    let text = std::fs::read_to_string(&toml_path).unwrap();
    let from = "script_entry = \"rules.lua\"";
    assert_eq!(
        text.matches(from).count(),
        1,
        "the bundle's table.toml names rules.lua once"
    );
    std::fs::write(
        &toml_path,
        text.replace(from, "script_entry = \"rules.wasm\""),
    )
    .unwrap();
}

/// Runs `f` on a thread with an 8 MiB stack (Wasm scripts need it).
pub fn on_big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// The bytes of a command queue: every list in field order, ids and
/// values little-endian, floats as their exact bit patterns, so two queues
/// have equal bytes exactly when they are the same commands in the same
/// order.
pub fn queue_bytes(q: &HostCommandQueue, out: &mut Vec<u8>) {
    out.clear();
    let len = |out: &mut Vec<u8>, n: usize| out.extend_from_slice(&(n as u32).to_le_bytes());
    len(out, q.coil_pulses.len());
    for (c, ms) in &q.coil_pulses {
        out.extend_from_slice(&c.0.to_le_bytes());
        out.extend_from_slice(&ms.to_le_bytes());
    }
    len(out, q.coil_holds.len());
    for (c, on) in &q.coil_holds {
        out.extend_from_slice(&c.0.to_le_bytes());
        out.push(u8::from(*on));
    }
    len(out, q.magnets.len());
    for (m, p) in &q.magnets {
        out.extend_from_slice(&m.0.to_le_bytes());
        out.extend_from_slice(&p.to_bits().to_le_bytes());
    }
    len(out, q.zippers.len());
    for (f, on) in &q.zippers {
        out.extend_from_slice(&f.0.to_le_bytes());
        out.push(u8::from(*on));
    }
    len(out, q.diverters.len());
    for (d, on) in &q.diverters {
        out.extend_from_slice(&d.0.to_le_bytes());
        out.push(u8::from(*on));
    }
    len(out, q.ball_spawns.len());
    for (d, p) in &q.ball_spawns {
        out.extend_from_slice(&d.0.to_le_bytes());
        out.extend_from_slice(&p.0.to_le_bytes());
    }
    len(out, q.lamps.len());
    for (l, i) in &q.lamps {
        out.extend_from_slice(&l.0.to_le_bytes());
        out.extend_from_slice(&i.to_bits().to_le_bytes());
    }
    match q.gi_tint {
        None => out.push(0),
        Some(t) => {
            out.push(1);
            for c in t {
                out.extend_from_slice(&c.to_bits().to_le_bytes());
            }
        }
    }
    len(out, q.scores.len());
    for (p, s) in &q.scores {
        out.push(*p);
        out.extend_from_slice(&s.to_le_bytes());
    }
    len(out, q.display_scenes.len());
    for s in &q.display_scenes {
        out.extend_from_slice(&s.0.to_le_bytes());
    }
    len(out, q.sounds.len());
    for (s, g) in &q.sounds {
        out.extend_from_slice(&s.0.to_le_bytes());
        out.extend_from_slice(&g.to_bits().to_le_bytes());
    }
    len(out, q.persist_writes.len());
    for (k, v) in &q.persist_writes {
        out.extend_from_slice(&k.0.to_le_bytes());
        out.extend_from_slice(&v.to_le_bytes());
    }
}

/// Everything a [`Simulation`] exposes after a tick that the script's
/// commands reach, as bytes: the state hash, the tick's display scenes,
/// sounds and score reports (queue order), lamps and GI tint of the
/// snapshot (float bits), every persist slot, and the script's fault /
/// overflow state.
pub fn sim_tick_bytes(sim: &Simulation, out: &mut Vec<u8>) {
    out.clear();
    out.extend_from_slice(&sim.state_hash().to_le_bytes());
    out.extend_from_slice(&(sim.display_scenes().len() as u32).to_le_bytes());
    for s in sim.display_scenes() {
        out.extend_from_slice(&s.0.to_le_bytes());
    }
    out.extend_from_slice(&(sim.sound_commands().len() as u32).to_le_bytes());
    for (s, g) in sim.sound_commands() {
        out.extend_from_slice(&s.0.to_le_bytes());
        out.extend_from_slice(&g.to_bits().to_le_bytes());
    }
    out.extend_from_slice(&(sim.score_reports().len() as u32).to_le_bytes());
    for (p, s) in sim.score_reports() {
        out.push(*p);
        out.extend_from_slice(&s.to_le_bytes());
    }
    let snap = sim.snapshot();
    for l in snap.lamps {
        out.extend_from_slice(&l.to_bits().to_le_bytes());
    }
    for g in snap.gi {
        out.extend_from_slice(&g.to_bits().to_le_bytes());
    }
    for k in 0..MAX_PERSIST as u16 {
        out.extend_from_slice(&sim.persist(k).to_le_bytes());
    }
    out.push(u8::from(sim.script_faulted()));
    out.extend_from_slice(&sim.script_fault_count().to_le_bytes());
    out.extend_from_slice(&sim.script_queue_overflows().to_le_bytes());
}

/// The `[persist]` key of `name`.
pub fn key(sim: &Simulation, name: &str) -> u16 {
    sim.ids()
        .get(pin_assets::IdKind::PersistKey, name)
        .unwrap_or_else(|| panic!("persist key {name}"))
}

/// What a [`replay_pair`] run reached.
#[derive(Debug)]
pub struct PairReport {
    /// Ticks run.
    pub ticks: u64,
    /// Final player-1 score under Lua / Wasm.
    pub scores: (Option<i64>, Option<i64>),
    /// Final `state_hash` under Lua / Wasm.
    pub hashes: (u64, u64),
    /// Highest `ball` persist value seen in the Wasm run.
    pub max_ball: i64,
    /// `games_played` at the end of the Wasm run.
    pub games: i64,
    /// Ticks on which the drained queue produced a display scene, a sound
    /// or a score report in the Wasm run (non-vacuity).
    pub ticks_with_output: u64,
    /// Highest persist value of each of `mode_keys` seen in the Wasm run.
    pub modes: Vec<(String, i64)>,
}

/// Steps the Lua and the Wasm bundle with the recorded events of
/// `recording` and asserts, after every tick, identical simulation output
/// ([`sim_tick_bytes`]); `on_tick` also sees the Lua sim after each tick
/// (for [`Lockstep::follow`]).
pub fn replay_pair(
    lua_dir: &Path,
    wasm_dir: &Path,
    recording: &Recording,
    mode_keys: &[&str],
    mut on_tick: impl FnMut(&Simulation),
) -> PairReport {
    let cfg = SimConfig {
        seed: recording.seed,
        ..SimConfig::default()
    };
    let mut lua = Simulation::load(lua_dir, cfg).expect("load the Lua bundle");
    let mut wasm = Simulation::load(wasm_dir, cfg).expect("load the Wasm bundle");
    let (mut a, mut b) = (Vec::new(), Vec::new());
    sim_tick_bytes(&lua, &mut a);
    sim_tick_bytes(&wasm, &mut b);
    assert!(a == b, "on_init output differs");
    let k_ball = key(&wasm, "ball");
    let k_games = key(&wasm, "games_played");
    let mode_ids: Vec<u16> = mode_keys.iter().map(|m| key(&wasm, m)).collect();
    let mut mode_max = vec![0i64; mode_keys.len()];
    let mut max_ball = 0;
    let mut ticks_with_output = 0;
    let mut out_l = *lua.snapshot();
    let mut out_w = *wasm.snapshot();
    let mut events: Vec<InputEvent> = Vec::new();
    for tick in 1..=recording.ticks {
        events.clear();
        events.extend(recording.events_on(tick).iter().map(|(_, e)| *e));
        lua.step_1ms(tick, &events, &mut out_l);
        wasm.step_1ms(tick, &events, &mut out_w);
        sim_tick_bytes(&lua, &mut a);
        sim_tick_bytes(&wasm, &mut b);
        if a != b {
            panic!(
                "tick {tick}: Lua and Wasm outputs differ\n lua scenes {:?} sounds {:?} scores {:?} \
                 faulted {} {:?}\nwasm scenes {:?} sounds {:?} scores {:?} faulted {} {:?}",
                lua.display_scenes(),
                lua.sound_commands(),
                lua.score_reports(),
                lua.script_faulted(),
                lua.script_error(),
                wasm.display_scenes(),
                wasm.sound_commands(),
                wasm.score_reports(),
                wasm.script_faulted(),
                wasm.script_error(),
            );
        }
        assert_eq!(out_l.tick, out_w.tick);
        if !wasm.display_scenes().is_empty()
            || !wasm.sound_commands().is_empty()
            || !wasm.score_reports().is_empty()
        {
            ticks_with_output += 1;
        }
        max_ball = max_ball.max(wasm.persist(k_ball));
        for (m, &k) in mode_max.iter_mut().zip(&mode_ids) {
            *m = (*m).max(wasm.persist(k));
        }
        on_tick(&lua);
    }
    assert!(!lua.script_faulted(), "{:?}", lua.script_error());
    assert!(!wasm.script_faulted(), "{:?}", wasm.script_error());
    assert_eq!(lua.script_queue_overflows(), 0);
    assert_eq!(wasm.script_queue_overflows(), 0);
    let modes = mode_keys
        .iter()
        .zip(mode_max)
        .map(|(m, v)| ((*m).to_owned(), v))
        .collect();
    PairReport {
        ticks: recording.ticks,
        scores: (lua.score(0), wasm.score(0)),
        hashes: (lua.state_hash(), wasm.state_hash()),
        max_ball,
        games: wasm.persist(k_games),
        ticks_with_output,
        modes,
    }
}

/// A Lua and a Wasm backend of one table driven in lockstep with identical
/// inputs; every tick's queues must be byte-identical.
pub struct Lockstep {
    lua: LuaScriptBackend,
    wasm: WasmScriptBackend,
    view: HostView,
    rng_lua: Pcg32,
    rng_wasm: Pcg32,
    q_lua: HostCommandQueue,
    q_wasm: HostCommandQueue,
    bytes: (Vec<u8>, Vec<u8>),
    prev_switches: [u64; 4],
    /// Ticks compared so far.
    pub ticks: u64,
    /// Ticks whose queue held at least one command.
    pub busy_ticks: u64,
    /// Commands compared so far (all lists).
    pub commands: u64,
    /// Callback rounds after which the PCG had moved (`random` draws).
    pub rng_moves: u64,
    /// The next `u64` the PCG copies would draw, at the last comparison.
    probe: u64,
}

fn queue_len(q: &HostCommandQueue) -> u64 {
    (q.coil_pulses.len()
        + q.coil_holds.len()
        + q.magnets.len()
        + q.zippers.len()
        + q.diverters.len()
        + q.ball_spawns.len()
        + q.lamps.len()
        + usize::from(q.gi_tint.is_some())
        + q.scores.len()
        + q.display_scenes.len()
        + q.sounds.len()
        + q.persist_writes.len()) as u64
}

impl Lockstep {
    /// Both backends of the table in bundle `lua_dir` (its `rules.lua`, and
    /// the committed `rules.wasm` of `table`), with the name tables `sim`
    /// gives the script, seeded with `seed`; runs and compares `on_init`.
    pub fn new(sim: &Simulation, lua_dir: &Path, table: &str, seed: u64) -> Self {
        let names = pin_sim::load::name_tables(sim.ids());
        let src = std::fs::read(lua_dir.join("rules.lua")).unwrap();
        let lua = LuaScriptBackend::new(&src, &names, LuaConfig::engine()).expect("Lua loads");
        let wasm = WasmScriptBackend::new(&committed_wasm(table), &names, WasmConfig::engine())
            .expect("Wasm loads");
        let mut s = Self {
            lua,
            wasm,
            view: HostView::default(),
            rng_lua: Pcg32::seed_from_u64(seed),
            rng_wasm: Pcg32::seed_from_u64(seed),
            q_lua: HostCommandQueue::default(),
            q_wasm: HostCommandQueue::default(),
            bytes: (Vec::new(), Vec::new()),
            prev_switches: [0; 4],
            ticks: 0,
            busy_ticks: 0,
            commands: 0,
            rng_moves: 0,
            probe: Pcg32::seed_from_u64(seed).next_u64(),
        };
        s.view.switches = sim.switch_bits();
        s.prev_switches = s.view.switches;
        s.view.balls_in_play = sim.balls_in_play().min(255) as u8;
        {
            let mut ctx = HostCtx {
                view: &s.view,
                rng: &mut s.rng_lua,
                cmd: &mut s.q_lua,
            };
            s.lua.on_init(&mut ctx);
        }
        {
            let mut ctx = HostCtx {
                view: &s.view,
                rng: &mut s.rng_wasm,
                cmd: &mut s.q_wasm,
            };
            s.wasm.on_init(&mut ctx);
        }
        s.compare_and_apply("on_init");
        s
    }

    /// The host view both backends see (persist slots as written so far).
    pub fn view(&self) -> &HostView {
        &self.view
    }

    /// Runs one tick on both backends: the switch `edges` in order, then
    /// `on_tick`, with the view's tick, switch bits and balls in play set
    /// first; then compares the queues and applies the persist writes.
    pub fn tick(
        &mut self,
        tick: u64,
        switches: [u64; 4],
        balls_in_play: u8,
        edges: &[(u16, bool)],
    ) {
        self.view.tick = tick;
        self.view.switches = switches;
        self.view.balls_in_play = balls_in_play;
        for (backend, rng, q) in [
            (
                &mut self.lua as &mut dyn TableScriptBackend,
                &mut self.rng_lua,
                &mut self.q_lua,
            ),
            (
                &mut self.wasm as &mut dyn TableScriptBackend,
                &mut self.rng_wasm,
                &mut self.q_wasm,
            ),
        ] {
            let mut ctx = HostCtx {
                view: &self.view,
                rng,
                cmd: q,
            };
            for &(sw, closed) in edges {
                let edge = if closed {
                    SwitchEdge::Closed
                } else {
                    SwitchEdge::Opened
                };
                backend.on_switch_edge(SwitchId(sw), edge, &mut ctx);
            }
            backend.on_tick(&mut ctx);
        }
        self.compare_and_apply(&format!("tick {tick}"));
        self.ticks += 1;
    }

    /// Follows the (Lua) simulation `sim` after one of its ticks: the
    /// switch changes since the last call are this tick's edges (ascending
    /// switch number, as the sim delivers them).
    pub fn follow(&mut self, sim: &Simulation) {
        let now = sim.switch_bits();
        let mut edges = Vec::new();
        for (w, &word) in now.iter().enumerate() {
            let mut diff = word ^ self.prev_switches[w];
            while diff != 0 {
                let b = diff.trailing_zeros() as usize;
                diff &= diff - 1;
                edges.push(((w * 64 + b) as u16, word & (1u64 << b) != 0));
            }
        }
        self.prev_switches = now;
        self.tick(sim.tick(), now, sim.balls_in_play().min(255) as u8, &edges);
    }

    fn compare_and_apply(&mut self, at: &str) {
        assert!(
            !self.lua.is_faulted(),
            "{at}: Lua faulted: {:?}",
            self.lua.last_error()
        );
        assert!(
            !self.wasm.is_faulted(),
            "{at}: Wasm faulted: {:?}",
            self.wasm.last_error()
        );
        queue_bytes(&self.q_lua, &mut self.bytes.0);
        queue_bytes(&self.q_wasm, &mut self.bytes.1);
        assert!(
            self.bytes.0 == self.bytes.1,
            "{at}: command queues differ\n lua {:?}\nwasm {:?}",
            self.q_lua,
            self.q_wasm
        );
        // The PCG draw order: both copies must stand at the same state.
        let (mut l, mut w) = (self.rng_lua.clone(), self.rng_wasm.clone());
        let (nl, nw) = (l.next_u64(), w.next_u64());
        assert_eq!(nl, nw, "{at}: the PCG draws differ");
        if nl != self.probe {
            self.rng_moves += 1;
        }
        assert_eq!(self.lua.overflow_count(), self.wasm.overflow_count());
        let n = queue_len(&self.q_lua);
        self.commands += n;
        if n > 0 {
            self.busy_ticks += 1;
        }
        for &(k, v) in &self.q_lua.persist_writes {
            if let Some(p) = self.view.persist.get_mut(k.index()) {
                *p = v;
            }
        }
        self.q_lua = HostCommandQueue::default();
        self.q_wasm = HostCommandQueue::default();
        self.probe = nl;
    }
}

/// Runs `rebuild.sh` of `table` into a scratch file and compares it with
/// the committed `rules.wasm` (as CI does). Skips with a printed note
/// where the `wasm32-unknown-unknown` target is not installed.
pub fn check_rebuild(table: &str) {
    if !wasm_target_installed() {
        println!("SKIP: wasm32-unknown-unknown is not installed; {table}/rules.wasm not rebuilt");
        return;
    }
    let script = table_src(table).join("rules-wasm/rebuild.sh");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "wasm_ports_rebuild_{table}_{}.wasm",
        std::process::id()
    ));
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("rules_wasm_{table}"));
    let status = Command::new("sh")
        .arg(&script)
        .arg(&out)
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("RUSTFLAGS")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .status()
        .expect("run rebuild.sh");
    assert!(status.success(), "{table}: rebuild.sh failed");
    let built = std::fs::read(&out).expect("read the rebuilt module");
    let _ = std::fs::remove_file(&out);
    assert!(
        built == committed_wasm(table),
        "tables/{table}/rules.wasm is stale: run tables/{table}/rules-wasm/rebuild.sh"
    );
}

/// Whether the current toolchain has the `wasm32-unknown-unknown` target.
pub fn wasm_target_installed() -> bool {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    Command::new(rustc)
        .args(["--print", "sysroot"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .is_some_and(|root| {
            Path::new(&root)
                .join("lib/rustlib/wasm32-unknown-unknown")
                .is_dir()
        })
}

/// A seeded random edge source for [`Lockstep`] fuzzing: every tick, with
/// probability `1 / every`, one random switch of `switches` toggles; the
/// `start` switch is pulsed every `start_every` ticks; balls in play drift
/// between 0 and 3.
pub struct RandomEdges {
    rng: Pcg32,
    switches: Vec<u16>,
    bits: [u64; 4],
    every: u32,
    start: u16,
    start_every: u64,
    balls: u8,
}

impl RandomEdges {
    /// A source over every switch of `sim`'s table.
    pub fn new(sim: &Simulation, seed: u64, every: u32, start_every: u64) -> Self {
        let names = pin_sim::load::name_tables(sim.ids());
        let mut switches: Vec<u16> = names.switches.iter().map(|(_, s)| s.0).collect();
        switches.sort_unstable();
        let start = sim.switch_id("start").expect("start switch").0;
        Self {
            rng: Pcg32::seed_from_u64(seed),
            switches,
            bits: sim.switch_bits(),
            every,
            start,
            start_every,
            balls: 1,
        }
    }

    fn set(&mut self, sw: u16, closed: bool, edges: &mut Vec<(u16, bool)>) {
        let (w, b) = (usize::from(sw) / 64, sw % 64);
        let was = self.bits[w] & (1u64 << b) != 0;
        if was != closed {
            if closed {
                self.bits[w] |= 1u64 << b;
            } else {
                self.bits[w] &= !(1u64 << b);
            }
            edges.push((sw, closed));
        }
    }

    /// Runs tick `tick` on `ls` with this tick's random edges.
    pub fn step(&mut self, ls: &mut Lockstep, tick: u64) {
        let mut edges = Vec::new();
        if tick % self.start_every == 1 {
            self.set(self.start, true, &mut edges);
        } else if tick % self.start_every == 30 {
            self.set(self.start, false, &mut edges);
        }
        // Up to two toggles per tick (closing and opening in one tick).
        for _ in 0..2 {
            if self.rng.next_u32().is_multiple_of(self.every) {
                let i = self.rng.next_u32() as usize % self.switches.len();
                let sw = self.switches[i];
                let closed = self.bits[usize::from(sw) / 64] & (1u64 << (sw % 64)) != 0;
                self.set(sw, !closed, &mut edges);
            }
        }
        if self.rng.next_u32().is_multiple_of(500) {
            self.balls = (self.rng.next_u32() % 4) as u8;
        }
        ls.tick(tick, self.bits, self.balls, &edges);
    }
}

/// Records `ticks` ticks of `inputs` on the Lua bundle and returns the
/// recording, through its byte format.
pub fn record(
    lua_dir: &Path,
    seed: u64,
    ticks: u64,
    mut inputs: impl FnMut(u64, &mut Vec<InputEvent>),
) -> Recording {
    let sim = Simulation::load(
        lua_dir,
        SimConfig {
            seed,
            ..SimConfig::default()
        },
    )
    .expect("load");
    let mut rec = pin_sim::replay::Recorder::new(sim);
    let mut buf = Vec::new();
    for t in 1..=ticks {
        inputs(t, &mut buf);
        rec.step(&buf);
    }
    let (_, recording) = rec.finish();
    Recording::from_bytes(&recording.to_bytes()).expect("the recording round-trips")
}

/// Played games' inputs for tick `t` (the fixed rhythm of the 6.1 / 6.4
/// replay tests): START at 10 ms and every 60 s after, a launch every 2.5 s, the lower and upper
/// flippers every 0.7 s, and the action button every `action_period` ticks.
pub fn played_inputs(t: u64, action_period: u64, out: &mut Vec<InputEvent>) {
    out.clear();
    let ts = t * TICK_NS;
    let mut b = |i: CabinetInput, p: bool| out.push(InputEvent::button(ts, i, p));
    // START every 60 s (it starts a game only from attract or game over).
    match t % 60_000 {
        10 => b(CabinetInput::Start, true),
        40 => b(CabinetInput::Start, false),
        _ => {}
    }
    match t % 2500 {
        1600 => b(CabinetInput::Launch, true),
        1630 => b(CabinetInput::Launch, false),
        _ => {}
    }
    match t % 700 {
        100 => b(CabinetInput::LeftFlipper, true),
        180 => b(CabinetInput::LeftFlipper, false),
        350 => b(CabinetInput::RightFlipper, true),
        430 => b(CabinetInput::RightFlipper, false),
        500 => b(CabinetInput::LeftUpper, true),
        560 => b(CabinetInput::LeftUpper, false),
        _ => {}
    }
    let a = t % action_period;
    if a == action_period / 2 {
        b(CabinetInput::Action, true);
    } else if a == action_period / 2 + 50 {
        b(CabinetInput::Action, false);
    }
}

/// The full check of one table: records `ticks` ticks of played games on
/// the Lua bundle ([`played_inputs`]), replays them on the Lua
/// and the Wasm bundle in lockstep (tick-for-tick equal output, see
/// [`replay_pair`]) while a [`Lockstep`] pair follows the Lua sim's switch
/// edges (byte-identical queues), and returns the report.
pub fn played_game_parity(
    table: &str,
    lua_dir: &Path,
    wasm_dir: &Path,
    action_period: u64,
    ticks: u64,
    mode_keys: &[&str],
) -> (PairReport, u64, u64) {
    let seed = SimConfig::default().seed;
    let recording = record(lua_dir, seed, ticks, |t, out| {
        played_inputs(t, action_period, out)
    });
    let names_sim = Simulation::load(lua_dir, SimConfig::default()).expect("load");
    let mut ls = Lockstep::new(&names_sim, lua_dir, table, seed);
    let report = replay_pair(lua_dir, wasm_dir, &recording, mode_keys, |s| ls.follow(s));
    assert_eq!(ls.ticks, recording.ticks);
    (report, ls.busy_ticks, ls.commands)
}
