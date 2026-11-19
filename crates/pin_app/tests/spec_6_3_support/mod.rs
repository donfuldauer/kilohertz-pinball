//! Shared helpers of the Subphase 6.3 spec tests (Independent Tester):
//! Lua and Wasm bundles of the three tables, deterministic recorded games
//! (a fixed rhythm and a seeded PCG stream), and the lock-step parity
//! harness that runs one game on both backends.
//!
//! Two layers are compared at every tick:
//!
//! * **Simulation layer.** A [`Simulation`] of the Lua bundle and one of the
//!   Wasm bundle receive the same input events and the same switch stimuli;
//!   their state hash, display scenes, sounds, score reports, haptics,
//!   lamps, GI, flippers, mech transforms, persist values, counters and
//!   script health must agree after every tick, and so must the command
//!   queue each simulation's script filled and applied in the tick
//!   (`Simulation::last_commands`, after load `on_init`'s): same commands,
//!   same order, float payloads bit-for-bit ([`same_queue`]). Each script
//!   is called by its own simulation with that simulation's pre-tick view,
//!   so this is the per-tick queue comparison.
//! * **Command layer (synthetic stream).** A [`ScriptPair`] drives a
//!   standalone `LuaScriptBackend` and `WasmScriptBackend` (built from the
//!   same bundle files and the simulation's own name tables) with one
//!   shared synthetic stream: the switch bits and ball count of the Lua
//!   simulation *after* each tick, the switch edges between consecutive
//!   post-tick states, the persist values their own commands wrote, and one
//!   seeded `Pcg32` each. That is not the view the simulation's script saw
//!   (ball counts after spawns and drains, physics edges a tick early,
//!   edges merged in ascending order), so it checks that both backends
//!   answer one shifted stream identically, not the sim's own queues.
#![allow(dead_code)]

/// The Feature Lab build (`crates/pin_app/examples/build_feature_lab.rs`,
/// the implementation `tables/feature_lab/build.sh` runs).
#[path = "../../examples/build_feature_lab.rs"]
mod feature_lab_build;

use pin_assetgen::{TableMeta, generate_kit};
use pin_assets::IdKind;
use pin_core::schema::table::TableManifest;
use pin_core::script::{HostCommandQueue, HostCtx, HostView, MAX_PERSIST, SwitchEdge};
use pin_core::{
    CabinetInput, FrameStateSnapshot, InputEvent, Pcg32, SeedableRng, SimStep, TICK_NS,
    TableScriptBackend,
};
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use pin_sim::{MAX_PLAYERS, SimConfig, Simulation};
use pin_tablegen::layout::parse_layout;
use pin_tablegen::{generate, write_bundle};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Tables and bundles
// ---------------------------------------------------------------------------

/// The three tables with Wasm ports (PLAN 6.3).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Table {
    /// `tables/apex_circuit`.
    Apex,
    /// `tables/sky_deck`.
    SkyDeck,
    /// `tables/feature_lab`.
    FeatureLab,
}

impl Table {
    /// Every table, in a fixed order.
    pub const ALL: [Table; 3] = [Table::Apex, Table::SkyDeck, Table::FeatureLab];

    /// The directory name under `tables/`.
    pub fn dir_name(self) -> &'static str {
        match self {
            Table::Apex => "apex_circuit",
            Table::SkyDeck => "sky_deck",
            Table::FeatureLab => "feature_lab",
        }
    }

    fn index(self) -> usize {
        match self {
            Table::Apex => 0,
            Table::SkyDeck => 1,
            Table::FeatureLab => 2,
        }
    }

    /// `tables/<table>` (the sources).
    pub fn src(self) -> PathBuf {
        repo_root().join("tables").join(self.dir_name())
    }

    /// Playfield switches closed (3 ticks each, 60 ticks apart) once ball 2
    /// of a game is on the table, chosen from the rules' documented mode
    /// qualifications so that a mode starts within the recorded game:
    ///
    /// * Apex Circuit: a left-orbit feed (`left_spinner`) then two shot
    ///   switches within 4 s raise the combo to 2x: `combo_chain` starts.
    /// * Sky Deck: the Newton ball and all three drop targets arm Night
    ///   Overdrive, then ACTION starts it.
    /// * Feature Lab: every qualifying shot sequence in turn (pops,
    ///   pops_down, banana_vuk, backbox_run, magnet_lane), so the selected
    ///   mode (drawn from the sim RNG) starts whichever it is.
    ///
    /// Each list ends with a sling (a sound and a coil pulse).
    pub fn mode_stimuli(self) -> &'static [&'static str] {
        match self {
            Table::Apex => &[
                "left_spinner",
                "orbit_top",
                "orbit_top",
                "ramp_made",
                "sling_left_sw",
            ],
            Table::SkyDeck => &[
                "newton_sw",
                "drop_1_sw",
                "drop_2_sw",
                "drop_3_sw",
                "action",
                "sling_left_sw",
            ],
            Table::FeatureLab => &[
                "bumper_2_ring",
                "bumper_3_ring",
                "bumper_2_ring",
                "vuk_lane",
                "bb_vuk_sw",
                "bb_drain",
                "magnet_opto",
                "vuk_lane",
                "sling_left_sw",
            ],
        }
    }
}

/// The repository root.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Removes the scratch directories of earlier test processes that have
/// exited (`pin_app_spec63_<tag>_<pid>_<n>` with no `/proc/<pid>`). The
/// bundles are cached in statics for the whole process, so they cannot be
/// removed on drop; this keeps at most the live processes' copies. Does
/// nothing where `/proc` is not available.
fn prune_dead_scratch(base: &Path) {
    if !Path::new("/proc/self").is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix("pin_app_spec63_") else {
            continue;
        };
        let mut parts = rest.rsplitn(3, '_');
        let (Some(_n), Some(pid), Some(_tag)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        if pid != std::process::id() && !Path::new(&format!("/proc/{pid}")).exists() {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// A fresh scratch directory under the target dir (tests never share one).
pub fn scratch(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    static PRUNE: std::sync::Once = std::sync::Once::new();
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    PRUNE.call_once(|| prune_dead_scratch(&base));
    let p = base.join(format!(
        "pin_app_spec63_{tag}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn parse_hex(c: &str) -> [u8; 3] {
    let h = c.trim_start_matches('#');
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).expect("palette colour");
    [b(0), b(2), b(4)]
}

/// Builds the Apex Circuit or Sky Deck bundle into `out` the way their
/// `build.sh` does: `pin_tablegen` generates `playfield.glb` and the merged
/// `table.toml`, the rules, backglass, layout, glass and every display
/// scene are copied, and the placeholder kit is written.
fn build_generated(src: &Path, out: &Path) {
    let layout_text = std::fs::read_to_string(src.join("layout.toml")).expect("layout.toml");
    let table_text = std::fs::read_to_string(src.join("table.toml")).expect("table.toml");
    let layout = parse_layout(&layout_text).expect("layout.toml parses");
    let base = TableManifest::from_toml_str(&table_text).expect("table.toml parses");
    let generated = generate(&layout, &base).unwrap_or_else(|e| panic!("pin_tablegen: {e}"));
    write_bundle(&generated, out).expect("write bundle");
    for f in ["rules.lua", "backglass.toml", "layout.toml", "glass.toml"] {
        std::fs::copy(src.join(f), out.join(f)).unwrap_or_else(|e| panic!("copy {f}: {e}"));
    }
    std::fs::create_dir_all(out.join("display")).unwrap();
    let mut scenes: Vec<PathBuf> = std::fs::read_dir(src.join("display"))
        .expect("display/")
        .map(|e| e.expect("display entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    scenes.sort();
    for p in scenes {
        let to = out.join("display").join(p.file_name().unwrap());
        std::fs::copy(&p, &to).unwrap_or_else(|e| panic!("copy {}: {e}", p.display()));
    }
    let meta = TableMeta {
        title: base.meta.name.clone(),
        palette: base.meta.palette.iter().map(|c| parse_hex(c)).collect(),
        seed: 0,
    };
    generate_kit(&meta, out).expect("placeholder kit");
}

fn build_lua_bundle(t: Table, out: &Path) {
    match t {
        Table::Apex | Table::SkyDeck => build_generated(&t.src(), out),
        Table::FeatureLab => {
            feature_lab_build::build_feature_lab_bundle(out)
                .unwrap_or_else(|e| panic!("feature_lab build: {e}"));
        }
    }
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

/// The committed `tables/<t>/rules.wasm` (SPEC: committed beside
/// `rules.lua`).
pub fn committed_wasm_path(t: Table) -> PathBuf {
    t.src().join("rules.wasm")
}

/// Reads the committed `rules.wasm`, failing with a SPEC-mismatch message
/// when it is missing.
pub fn committed_wasm(t: Table) -> Vec<u8> {
    let p = committed_wasm_path(t);
    std::fs::read(&p).unwrap_or_else(|e| {
        panic!(
            "SPEC mismatch: {} must be committed beside rules.lua ({e})",
            p.display()
        )
    })
}

/// Rewrites the `script_entry` line of a bundle's `table.toml` to
/// `entry`; exactly one such line must exist.
pub fn rewrite_script_entry(table_toml: &str, entry: &str) -> String {
    let mut n = 0;
    let mut out = String::with_capacity(table_toml.len() + 8);
    for line in table_toml.split_inclusive('\n') {
        let key = line.trim_start();
        if key.starts_with("script_entry") && key[12..].trim_start().starts_with('=') {
            n += 1;
            out.push_str(&format!("script_entry = \"{entry}\""));
            if line.ends_with('\n') {
                out.push('\n');
            }
        } else {
            out.push_str(line);
        }
    }
    assert_eq!(n, 1, "table.toml must have exactly one script_entry line");
    out
}

/// The Lua bundle of `t` (its `build.sh` steps), built once per test
/// binary.
pub fn lua_bundle(t: Table) -> &'static Path {
    static DIRS: [OnceLock<PathBuf>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    DIRS[t.index()].get_or_init(|| {
        let d = scratch(&format!("{}_lua", t.dir_name()));
        build_lua_bundle(t, &d);
        let toml = std::fs::read_to_string(d.join("table.toml")).unwrap();
        let m = TableManifest::from_toml_str(&toml).expect("bundle table.toml parses");
        assert_eq!(
            m.meta.script_entry, "rules.lua",
            "the Lua bundle runs rules.lua"
        );
        d
    })
}

/// The Wasm bundle of `t`: a copy of the Lua bundle with the committed
/// `tables/<t>/rules.wasm` and `script_entry = "rules.wasm"` in the copy's
/// `table.toml` only (the backend is chosen by the extension).
pub fn wasm_bundle(t: Table) -> &'static Path {
    static DIRS: [OnceLock<PathBuf>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    DIRS[t.index()].get_or_init(|| {
        let lua = lua_bundle(t);
        let d = scratch(&format!("{}_wasm", t.dir_name()));
        copy_dir(lua, &d);
        std::fs::write(d.join("rules.wasm"), committed_wasm(t)).unwrap();
        let toml = std::fs::read_to_string(d.join("table.toml")).unwrap();
        let toml = rewrite_script_entry(&toml, "rules.wasm");
        let m = TableManifest::from_toml_str(&toml).expect("rewritten table.toml parses");
        assert_eq!(m.meta.script_entry, "rules.wasm");
        std::fs::write(d.join("table.toml"), toml).unwrap();
        d
    })
}

/// Loads `dir` with sim seed `seed` and the default script limits.
pub fn load(dir: &Path, seed: u64) -> Simulation {
    Simulation::load(
        dir,
        SimConfig {
            seed,
            ..SimConfig::default()
        },
    )
    .unwrap_or_else(|e| panic!("load {}: {e}", dir.display()))
}

/// Runs `f` on a thread with an 8 MiB stack (wasmtime needs 2 MiB).
pub fn on_big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// The persist key index of `name`.
pub fn persist_key(s: &Simulation, name: &str) -> u16 {
    s.ids()
        .get(IdKind::PersistKey, name)
        .unwrap_or_else(|| panic!("persist key {name}"))
}

/// The sum of every `mode:<id>` persist value (mode start counts).
pub fn mode_starts(s: &Simulation) -> i64 {
    pin_sim::load::name_tables(s.ids())
        .persist
        .iter()
        .filter(|(n, _)| n.starts_with("mode:"))
        .map(|(_, k)| s.persist(k.0))
        .sum()
}

// ---------------------------------------------------------------------------
// Recorded games
// ---------------------------------------------------------------------------

/// A small PCG32 (XSH RR, 64-bit state) owned by the test: the source of
/// the pseudo-random input streams (independent of the sim's RNG).
#[derive(Clone, Debug)]
pub struct TestPcg {
    state: u64,
    inc: u64,
}

impl TestPcg {
    /// Seeds the generator (stream 0xda3e39cb94b95bdb).
    pub fn new(seed: u64) -> Self {
        let mut p = Self {
            state: 0,
            inc: (0xda3e_39cb_94b9_5bdb << 1) | 1,
        };
        p.next_u32();
        p.state = p.state.wrapping_add(seed);
        p.next_u32();
        p
    }

    /// The next 32 random bits.
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform in `0..n` (n > 0; the modulo bias is irrelevant here).
    pub fn below(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }
}

/// The cabinet input stream of a recorded game.
#[derive(Clone, Debug)]
pub enum Inputs {
    /// Start, a launch every 2.5 s, the flippers in a fixed rhythm and the
    /// action button now and then (as `apex_replay.rs`).
    Rhythm,
    /// A seeded pseudo-random stream from [`TestPcg`]: flipper buttons
    /// toggled at random, random launches and action presses, and a start
    /// press every 30 s (a new game after game over).
    Random(u64),
}

/// A deterministic recorded game.
#[derive(Clone, Debug)]
pub struct Game {
    /// The cabinet input stream.
    pub inputs: Inputs,
    /// Ticks to run.
    pub ticks: u64,
    /// Whether the table's [`Table::mode_stimuli`] are applied once ball 2
    /// is on the table (switch injection, not part of a byte recording).
    pub stimuli: bool,
}

/// Generates a game's cabinet events tick by tick.
pub struct InputGen {
    inputs: Inputs,
    pcg: TestPcg,
    held: [bool; CabinetInput::COUNT],
    release_at: [u64; CabinetInput::COUNT],
}

impl InputGen {
    /// A generator for `inputs`.
    pub fn new(inputs: &Inputs) -> Self {
        let seed = match inputs {
            Inputs::Rhythm => 0,
            Inputs::Random(s) => *s,
        };
        Self {
            inputs: inputs.clone(),
            pcg: TestPcg::new(seed),
            held: [false; CabinetInput::COUNT],
            release_at: [0; CabinetInput::COUNT],
        }
    }

    /// The events of tick `t` (≥ 1) into `out` (cleared first).
    pub fn events(&mut self, t: u64, out: &mut Vec<InputEvent>) {
        out.clear();
        match self.inputs {
            Inputs::Rhythm => rhythm(t, out),
            Inputs::Random(_) => self.random(t, out),
        }
    }

    fn random(&mut self, t: u64, out: &mut Vec<InputEvent>) {
        let ts = t * TICK_NS - TICK_NS / 2;
        let push = |out: &mut Vec<InputEvent>, i: CabinetInput, p: bool| {
            out.push(InputEvent::button(ts, i, p));
        };
        if t % 30_000 == 10 {
            push(out, CabinetInput::Start, true);
        }
        if t % 30_000 == 45 {
            push(out, CabinetInput::Start, false);
        }
        for i in [
            CabinetInput::LeftFlipper,
            CabinetInput::RightFlipper,
            CabinetInput::LeftUpper,
            CabinetInput::RightUpper,
        ] {
            if self.pcg.below(150) == 0 {
                let k = i.index();
                self.held[k] = !self.held[k];
                push(out, i, self.held[k]);
            }
        }
        for (i, odds, hold) in [
            (CabinetInput::Launch, 1_500, 40),
            (CabinetInput::Action, 4_000, 30),
        ] {
            let k = i.index();
            if self.held[k] {
                if t >= self.release_at[k] {
                    self.held[k] = false;
                    push(out, i, false);
                }
            } else if self.pcg.below(odds) == 0 {
                self.held[k] = true;
                self.release_at[k] = t + hold;
                push(out, i, true);
            }
        }
    }
}

/// The fixed-rhythm stream (tick `t`).
fn rhythm(t: u64, out: &mut Vec<InputEvent>) {
    let ts = t * TICK_NS;
    let mut b = |i: CabinetInput, p: bool| out.push(InputEvent::button(ts, i, p));
    match t % 30_000 {
        10 => b(CabinetInput::Start, true),
        40 => b(CabinetInput::Start, false),
        _ => {}
    }
    if t % 2500 == 1600 {
        b(CabinetInput::Launch, true);
    }
    if t % 2500 == 1630 {
        b(CabinetInput::Launch, false);
    }
    match t % 700 {
        100 => b(CabinetInput::LeftFlipper, true),
        180 => b(CabinetInput::LeftFlipper, false),
        350 => b(CabinetInput::RightFlipper, true),
        430 => b(CabinetInput::RightFlipper, false),
        500 => b(CabinetInput::LeftUpper, true),
        560 => b(CabinetInput::LeftUpper, false),
        620 => b(CabinetInput::RightUpper, true),
        660 => b(CabinetInput::RightUpper, false),
        _ => {}
    }
    match t % 9000 {
        4000 => b(CabinetInput::Action, true),
        4050 => b(CabinetInput::Action, false),
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Command layer: two standalone backends in lock step
// ---------------------------------------------------------------------------

/// The command kinds of a `HostCommandQueue`, in field order.
pub const KINDS: [&str; 12] = [
    "coil_pulses",
    "coil_holds",
    "magnets",
    "zippers",
    "diverters",
    "ball_spawns",
    "lamps",
    "gi_tint",
    "scores",
    "display_scenes",
    "sounds",
    "persist_writes",
];

/// The kinds present in `q`.
pub fn kinds_of(q: &HostCommandQueue) -> [bool; 12] {
    [
        !q.coil_pulses.is_empty(),
        !q.coil_holds.is_empty(),
        !q.magnets.is_empty(),
        !q.zippers.is_empty(),
        !q.diverters.is_empty(),
        !q.ball_spawns.is_empty(),
        !q.lamps.is_empty(),
        q.gi_tint.is_some(),
        !q.scores.is_empty(),
        !q.display_scenes.is_empty(),
        !q.sounds.is_empty(),
        !q.persist_writes.is_empty(),
    ]
}

/// Whether two queues hold the same commands in the same order, float
/// payloads compared bit for bit.
pub fn same_queue(a: &HostCommandQueue, b: &HostCommandQueue) -> bool {
    fn f<T: PartialEq + Copy>(x: &[(T, f32)], y: &[(T, f32)]) -> bool {
        x.len() == y.len()
            && x.iter()
                .zip(y)
                .all(|(p, q)| p.0 == q.0 && p.1.to_bits() == q.1.to_bits())
    }
    a.coil_pulses == b.coil_pulses
        && a.coil_holds == b.coil_holds
        && f(&a.magnets, &b.magnets)
        && a.zippers == b.zippers
        && a.diverters == b.diverters
        && a.ball_spawns == b.ball_spawns
        && f(&a.lamps, &b.lamps)
        && a.gi_tint.map(|g| g.map(f32::to_bits)) == b.gi_tint.map(|g| g.map(f32::to_bits))
        && a.scores == b.scores
        && a.display_scenes == b.display_scenes
        && f(&a.sounds, &b.sounds)
        && a.persist_writes == b.persist_writes
}

/// A standalone Lua and Wasm backend of one table fed one shifted synthetic
/// stream (post-tick views of the Lua simulation); see the module docs.
pub struct ScriptPair {
    lua: LuaScriptBackend,
    wasm: WasmScriptBackend,
    rng_lua: Pcg32,
    rng_wasm: Pcg32,
    view: HostView,
    cmd_lua: HostCommandQueue,
    cmd_wasm: HostCommandQueue,
    prev_bits: [u64; 4],
    /// Kinds seen in any drained queue.
    pub kinds: [bool; 12],
    /// Persist keys written by any command.
    pub persist_keys: BTreeSet<u16>,
    /// Ticks (including `on_init`) whose drained queues were non-empty.
    pub busy_ticks: u64,
    /// Ticks compared.
    pub ticks: u64,
    context: String,
}

impl ScriptPair {
    /// Builds both backends from `lua_dir`/`rules.lua` and the committed
    /// `rules.wasm` of `t` with the name tables of `world` (a simulation of
    /// `t`), seeds both RNGs with `seed` and runs and compares `on_init`.
    pub fn new(t: Table, world: &Simulation, seed: u64, context: &str) -> Self {
        let names = pin_sim::load::name_tables(world.ids());
        let lua_src = std::fs::read(lua_bundle(t).join("rules.lua")).expect("rules.lua");
        let lua = LuaScriptBackend::new(&lua_src, &names, LuaConfig::engine())
            .unwrap_or_else(|e| panic!("{context}: Lua backend: {e}"));
        let wasm = WasmScriptBackend::new(&committed_wasm(t), &names, WasmConfig::engine())
            .unwrap_or_else(|e| panic!("{context}: Wasm backend: {e}"));
        let view = HostView {
            switches: world.switch_bits(),
            balls_in_play: world.balls_in_play().min(255) as u8,
            ..HostView::default()
        };
        let mut p = Self {
            lua,
            wasm,
            rng_lua: Pcg32::seed_from_u64(seed),
            rng_wasm: Pcg32::seed_from_u64(seed),
            view,
            cmd_lua: HostCommandQueue::default(),
            cmd_wasm: HostCommandQueue::default(),
            prev_bits: world.switch_bits(),
            kinds: [false; 12],
            persist_keys: BTreeSet::new(),
            busy_ticks: 0,
            ticks: 0,
            context: context.to_owned(),
        };
        {
            let mut c = HostCtx {
                view: &p.view,
                rng: &mut p.rng_lua,
                cmd: &mut p.cmd_lua,
            };
            p.lua.on_init(&mut c);
            let mut c = HostCtx {
                view: &p.view,
                rng: &mut p.rng_wasm,
                cmd: &mut p.cmd_wasm,
            };
            p.wasm.on_init(&mut c);
        }
        p.drain_and_compare(0);
        p
    }

    /// Delivers tick `t`: the switch edges between the previous and the
    /// current switch bits of `world` (ascending switch id), then
    /// `on_tick`; then drains and compares both queues.
    pub fn step(&mut self, t: u64, world: &Simulation) {
        let bits = world.switch_bits();
        self.view.tick = t;
        self.view.switches = bits;
        self.view.balls_in_play = world.balls_in_play().min(255) as u8;
        for (w, (&now, &before)) in bits.iter().zip(&self.prev_bits).enumerate() {
            let mut diff = before ^ now;
            while diff != 0 {
                let b = diff.trailing_zeros() as usize;
                diff &= diff - 1;
                let id = pin_core::SwitchId((w * 64 + b) as u16);
                let edge = if now & (1u64 << b) != 0 {
                    SwitchEdge::Closed
                } else {
                    SwitchEdge::Opened
                };
                let mut c = HostCtx {
                    view: &self.view,
                    rng: &mut self.rng_lua,
                    cmd: &mut self.cmd_lua,
                };
                self.lua.on_switch_edge(id, edge, &mut c);
                let mut c = HostCtx {
                    view: &self.view,
                    rng: &mut self.rng_wasm,
                    cmd: &mut self.cmd_wasm,
                };
                self.wasm.on_switch_edge(id, edge, &mut c);
            }
        }
        self.prev_bits = bits;
        let mut c = HostCtx {
            view: &self.view,
            rng: &mut self.rng_lua,
            cmd: &mut self.cmd_lua,
        };
        self.lua.on_tick(&mut c);
        let mut c = HostCtx {
            view: &self.view,
            rng: &mut self.rng_wasm,
            cmd: &mut self.cmd_wasm,
        };
        self.wasm.on_tick(&mut c);
        self.drain_and_compare(t);
    }

    fn drain_and_compare(&mut self, t: u64) {
        let ctx = &self.context;
        assert!(
            same_queue(&self.cmd_lua, &self.cmd_wasm),
            "{ctx}: command queues differ at tick {t}\n lua:  {:?}\n wasm: {:?}",
            self.cmd_lua,
            self.cmd_wasm
        );
        assert!(
            self.rng_lua == self.rng_wasm,
            "{ctx}: RNG draw order differs at tick {t}"
        );
        self.check_health(t);
        let k = kinds_of(&self.cmd_lua);
        for (seen, now) in self.kinds.iter_mut().zip(k) {
            *seen |= now;
        }
        if !self.cmd_lua.is_empty() {
            self.busy_ticks += 1;
        }
        for &(key, v) in &self.cmd_lua.persist_writes {
            self.persist_keys.insert(key.0);
            if key.index() < MAX_PERSIST {
                self.view.persist[key.index()] = v;
            }
        }
        self.cmd_lua.clear();
        self.cmd_wasm.clear();
        self.ticks += 1;
    }

    fn check_health(&self, t: u64) {
        let ctx = &self.context;
        assert!(
            !self.lua.is_faulted() && self.lua.fault_count() == 0,
            "{ctx}: standalone Lua faulted at tick {t}: {:?}",
            self.lua.last_error()
        );
        assert!(
            !self.wasm.is_faulted() && self.wasm.fault_count() == 0,
            "{ctx}: standalone Wasm faulted at tick {t}: {:?}",
            self.wasm.last_error()
        );
        assert!(self.wasm.last_error().is_none(), "{ctx}: Wasm error");
        assert_eq!(self.lua.overflow_count(), 0, "{ctx}: Lua queue overflow");
        assert_eq!(self.wasm.overflow_count(), 0, "{ctx}: Wasm queue overflow");
    }

    /// The names of the kinds seen.
    pub fn kinds_seen(&self) -> Vec<&'static str> {
        KINDS
            .iter()
            .zip(self.kinds)
            .filter(|(_, s)| *s)
            .map(|(n, _)| *n)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Simulation layer
// ---------------------------------------------------------------------------

/// Asserts the script of `s` is healthy: never faulted, no error, no
/// queue overflow.
pub fn assert_script_healthy(s: &Simulation, ctx: &str) {
    assert!(
        !s.script_faulted(),
        "{ctx}: script tilted: {:?}",
        s.script_error()
    );
    assert_eq!(s.script_fault_count(), 0, "{ctx}: script faults");
    assert!(s.script_error().is_none(), "{ctx}: script error");
    assert_eq!(s.script_queue_overflows(), 0, "{ctx}: queue overflow");
}

fn bits(v: &[f32]) -> impl Iterator<Item = u32> + '_ {
    v.iter().map(|x| x.to_bits())
}

/// Asserts that the Lua and the Wasm simulation published the same tick,
/// starting with the command queue each script filled and the simulation
/// applied (`Simulation::last_commands`).
pub fn assert_same_tick(l: &Simulation, w: &Simulation, ctx: &str) {
    let t = l.tick();
    assert_eq!(t, w.tick(), "{ctx}: tick");
    assert_script_healthy(l, &format!("{ctx} lua tick {t}"));
    assert_script_healthy(w, &format!("{ctx} wasm tick {t}"));
    assert!(
        same_queue(l.last_commands(), w.last_commands()),
        "{ctx}: the simulations' command queues differ at tick {t}\n lua:  {:?}\n wasm: {:?}",
        l.last_commands(),
        w.last_commands()
    );
    assert_eq!(
        l.state_hash(),
        w.state_hash(),
        "{ctx}: state_hash differs at tick {t}"
    );
    assert_eq!(
        l.display_scenes(),
        w.display_scenes(),
        "{ctx}: display scenes at tick {t}"
    );
    let (ls, ws) = (l.sound_commands(), w.sound_commands());
    assert!(
        ls.len() == ws.len()
            && ls
                .iter()
                .zip(ws)
                .all(|(a, b)| a.0 == b.0 && a.1.to_bits() == b.1.to_bits()),
        "{ctx}: sounds at tick {t}: {ls:?} vs {ws:?}"
    );
    assert_eq!(
        l.score_reports(),
        w.score_reports(),
        "{ctx}: score reports at tick {t}"
    );
    assert_eq!(
        l.haptic_events(),
        w.haptic_events(),
        "{ctx}: haptics at tick {t}"
    );
    let (a, b): (&FrameStateSnapshot, &FrameStateSnapshot) = (l.snapshot(), w.snapshot());
    assert!(
        bits(&a.lamps).eq(bits(&b.lamps)),
        "{ctx}: lamps at tick {t}"
    );
    assert!(bits(&a.gi).eq(bits(&b.gi)), "{ctx}: GI at tick {t}");
    assert_eq!(
        format!("{:?}{:?}", a.flippers(), a.dynamic_meshes()),
        format!("{:?}{:?}", b.flippers(), b.dynamic_meshes()),
        "{ctx}: flippers / mechs at tick {t}"
    );
    for k in 0..MAX_PERSIST as u16 {
        assert_eq!(l.persist(k), w.persist(k), "{ctx}: persist {k} at tick {t}");
    }
    assert_eq!(
        l.balls_in_play(),
        w.balls_in_play(),
        "{ctx}: balls at tick {t}"
    );
    assert_eq!(l.counters(), w.counters(), "{ctx}: counters at tick {t}");
}

// ---------------------------------------------------------------------------
// The parity run
// ---------------------------------------------------------------------------

/// What a parity run observed (identical on both backends).
#[derive(Debug)]
pub struct ParityReport {
    /// Final state hash (both simulations).
    pub state_hash: u64,
    /// Final scores of players 0..MAX_PLAYERS.
    pub scores: Vec<Option<i64>>,
    /// Highest `ball` persist value seen.
    pub max_ball: i64,
    /// Final `ball` persist value.
    pub final_ball: i64,
    /// Final sum of the `mode:<id>` persist values.
    pub mode_starts: i64,
    /// Tick the mode stimuli started, if they ran.
    pub stimuli_at: Option<u64>,
    /// Command kinds seen (command layer).
    pub kinds: Vec<&'static str>,
    /// Persist keys written (command layer).
    pub persist_keys: BTreeSet<u16>,
    /// Ticks with a non-empty queue (command layer).
    pub busy_ticks: u64,
    /// Ticks compared (command layer, including `on_init`).
    pub compared_ticks: u64,
}

/// Runs `game` on the Lua and the Wasm bundle of `t` with sim seed `seed`
/// in lock step, comparing both layers after every tick; panics at the
/// first difference. Call it on a big stack.
pub fn run_parity(t: Table, seed: u64, game: &Game) -> ParityReport {
    let ctx = format!("{t:?} seed {seed:#x} {:?}", game.inputs);
    let mut lua = load(lua_bundle(t), seed);
    let mut wasm = load(wasm_bundle(t), seed);
    assert_ne!(
        lua.table_hash(),
        wasm.table_hash(),
        "{ctx}: the two bundles must differ (script_entry)"
    );
    assert_same_tick(&lua, &wasm, &ctx);
    let mut pair = ScriptPair::new(t, &lua, seed, &ctx);
    let k_ball = persist_key(&lua, "ball");
    let k_state = persist_key(&lua, "game_state");
    let mut igen = InputGen::new(&game.inputs);
    let mut ev = Vec::new();
    let mut out_l = Box::new(*lua.snapshot());
    let mut out_w = Box::new(*wasm.snapshot());
    let mut max_ball = 0;
    let mut last_ball = lua.persist(k_ball);
    let mut armed: Option<u64> = None;
    let mut stimuli_at = None;
    // (tick, switch, close)
    let mut pending: Vec<(u64, &'static str, bool)> = Vec::new();
    let mut lua_scores = vec![None; usize::from(MAX_PLAYERS)];
    for tick in 1..=game.ticks {
        // Stimuli, decided from the Lua world before the tick.
        let ball = lua.persist(k_ball);
        if game.stimuli && stimuli_at.is_none() {
            if ball != last_ball && ball >= 2 && lua.persist(k_state) == 1 {
                armed = Some(tick + 300);
            }
            if armed == Some(tick) {
                stimuli_at = Some(tick);
                for (i, name) in t.mode_stimuli().iter().enumerate() {
                    let at = tick + 60 * i as u64;
                    pending.push((at, name, true));
                    pending.push((at + 3, name, false));
                }
            }
        }
        last_ball = ball;
        for &(at, name, close) in &pending {
            if at == tick {
                for s in [&mut lua, &mut wasm] {
                    if close {
                        s.set_switch(name, true)
                    } else {
                        s.release_switch(name)
                    }
                    .unwrap_or_else(|e| panic!("{ctx}: switch {name}: {e}"));
                }
            }
        }
        igen.events(tick, &mut ev);
        lua.step_1ms(tick, &ev, &mut out_l);
        wasm.step_1ms(tick, &ev, &mut out_w);
        assert_same_tick(&lua, &wasm, &ctx);
        pair.step(tick, &lua);
        max_ball = max_ball.max(lua.persist(k_ball));
        for (p, s) in lua_scores.iter_mut().enumerate() {
            if let Some(v) = lua.score(p as u8) {
                *s = Some(v);
            }
        }
    }
    let scores: Vec<Option<i64>> = (0..MAX_PLAYERS).map(|p| lua.score(p)).collect();
    let wscores: Vec<Option<i64>> = (0..MAX_PLAYERS).map(|p| wasm.score(p)).collect();
    assert_eq!(scores, wscores, "{ctx}: final scores");
    assert_eq!(scores, lua_scores, "{ctx}: score() is the last report");
    assert_eq!(lua.persist(k_ball), wasm.persist(k_ball), "{ctx}: ball");
    assert_eq!(
        lua.state_hash(),
        wasm.state_hash(),
        "{ctx}: final state_hash"
    );
    assert_eq!(mode_starts(&lua), mode_starts(&wasm), "{ctx}: mode starts");
    ParityReport {
        state_hash: lua.state_hash(),
        scores,
        max_ball,
        final_ball: lua.persist(k_ball),
        mode_starts: mode_starts(&lua),
        stimuli_at,
        kinds: pair.kinds_seen(),
        persist_keys: pair.persist_keys.clone(),
        busy_ticks: pair.busy_ticks,
        compared_ticks: pair.ticks,
    }
}

/// The sim seeds every parity game runs with (≥ 3, PLAN 6.3 brief item 3).
pub const SEEDS: [u64; 3] = [pin_sim::DEFAULT_SEED, 1, 0x5eed_63c0_ffee];
