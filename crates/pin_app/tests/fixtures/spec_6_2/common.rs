//! Shared helpers of the Subphase 6.2 spec tests (Independent Tester): the
//! "Sky Deck" table (`tables/sky_deck`) built in-process into a bundle the
//! way its documented `build.sh` does (brief C1), plus the black-box driving
//! helpers every `spec_6_2_*` binary uses (cabinet inputs through the real
//! `SimStep::step_1ms` path, persist keys by their position in `[persist]
//! keys`, drains, health checks).
//!
//! Only `tables/sky_deck/table.toml` and `layout.toml` are read, and only by
//! the generators (`pin_tablegen::parse_layout` / `generate`); every other
//! source file is copied unchanged, `display/` by listing the directory.

#![allow(dead_code)]

use pin_assetgen::{TableMeta, generate_kit};
use pin_assets::{LoadedTable, Vfs, load_table};
use pin_core::TableManifest;
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::schema::layout::LayoutManifest;
use pin_core::schema::table::SwitchKind;
use pin_core::sim::{SimStep, TICK_NS};
use pin_core::snapshot::BallState;
use pin_sim::{BallHandle, SimConfig, Simulation};
use pin_tablegen::layout::parse_layout;
use pin_tablegen::{generate, write_bundle};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Steel ball profile.
pub const STEEL: &str = "standard_steel";
/// Glo-Ball profile.
pub const GLO: &str = "uv_globall";
/// `ProfileId` of `standard_steel` (index in `BUILTIN_PROFILE_NAMES`).
pub const STEEL_ID: u16 = 0;
/// `ProfileId` of `uv_globall` (index in `BUILTIN_PROFILE_NAMES`).
pub const GLO_ID: u16 = 3;
/// Upper-deck height (brief C2), m.
pub const DECK_Z_M: f32 = 0.065;

/// Radius of `standard_steel`, m.
pub fn steel_radius_m() -> f32 {
    pin_core::builtin_profile(STEEL)
        .expect("built-in profile")
        .radius_m()
}

/// Radius of `uv_globall`, m.
pub fn glo_radius_m() -> f32 {
    pin_core::builtin_profile(GLO)
        .expect("built-in profile")
        .radius_m()
}

/// `tables/sky_deck` (the sources).
pub fn sky_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/sky_deck")
}

/// Scratch directory under the test target dir, unique to this process.
pub fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("spec_6_2_{name}_{}", std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clear scratch dir");
    }
    std::fs::create_dir_all(&dir).expect("create scratch dir");
    dir
}

fn parse_hex(c: &str) -> [u8; 3] {
    let h = c.trim_start_matches('#');
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).expect("palette colour");
    [b(0), b(2), b(4)]
}

/// Copies every file under `from` (recursively, sorted) to `to`.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create display dir");
    let mut entries: Vec<_> = std::fs::read_dir(from)
        .unwrap_or_else(|e| panic!("list {}: {e}", from.display()))
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for p in entries {
        let dest = to.join(p.file_name().expect("file name"));
        if p.is_dir() {
            copy_tree(&p, &dest);
        } else {
            std::fs::copy(&p, &dest).unwrap_or_else(|e| panic!("copy {}: {e}", p.display()));
        }
    }
}

/// Builds the Sky Deck bundle into `out_dir` (brief C1): `pin_tablegen`
/// generates `playfield.glb` and the merged `table.toml` from `layout.toml`
/// and `table.toml`; `rules.lua`, `backglass.toml`, `layout.toml`,
/// `glass.toml` and every file under `display/` are copied unchanged; the
/// placeholder kit (`pin_assetgen::generate_kit`, title "Sky Deck") adds the
/// sounds, fonts and backglass art.
pub fn build_sky_deck_bundle(out_dir: &Path) {
    let src = sky_src();
    let layout_text = std::fs::read_to_string(src.join("layout.toml"))
        .expect("tables/sky_deck/layout.toml exists");
    let table_text =
        std::fs::read_to_string(src.join("table.toml")).expect("tables/sky_deck/table.toml exists");
    let layout = parse_layout(&layout_text).expect("layout.toml parses");
    let base = TableManifest::from_toml_str(&table_text).expect("table.toml parses");
    let generated = generate(&layout, &base).unwrap_or_else(|e| panic!("pin_tablegen: {e}"));
    write_bundle(&generated, out_dir).expect("write bundle");
    for f in ["rules.lua", "backglass.toml", "layout.toml", "glass.toml"] {
        std::fs::copy(src.join(f), out_dir.join(f))
            .unwrap_or_else(|e| panic!("copy tables/sky_deck/{f}: {e}"));
    }
    copy_tree(&src.join("display"), &out_dir.join("display"));
    let meta = TableMeta {
        title: "Sky Deck".to_owned(),
        palette: base.meta.palette.iter().map(|c| parse_hex(c)).collect(),
        seed: 0,
    };
    generate_kit(&meta, out_dir).expect("placeholder kit");
}

/// The Sky Deck bundle, built once per test binary.
pub fn bundle() -> &'static Path {
    static BUNDLE: OnceLock<PathBuf> = OnceLock::new();
    BUNDLE.get_or_init(|| {
        let out = scratch("sky_deck");
        build_sky_deck_bundle(&out);
        out
    })
}

/// `pin_assets::load_table` on the bundle.
pub fn table() -> &'static LoadedTable {
    static TABLE: OnceLock<LoadedTable> = OnceLock::new();
    TABLE.get_or_init(|| {
        let vfs = Vfs::open(bundle()).expect("open Sky Deck bundle");
        load_table(&vfs).expect("Sky Deck bundle passes load_table")
    })
}

/// The bundle's `layout.toml` (as copied by the build), parsed.
pub fn layout() -> &'static LayoutManifest {
    static LAYOUT: OnceLock<LayoutManifest> = OnceLock::new();
    LAYOUT.get_or_init(|| {
        let text = std::fs::read_to_string(bundle().join("layout.toml")).expect("bundle layout");
        parse_layout(&text).expect("bundle layout.toml parses")
    })
}

/// A fresh simulation of the bundle with simulation seed `seed`.
pub fn load_seeded(seed: u64) -> Simulation {
    Simulation::load(
        bundle(),
        SimConfig {
            seed,
            ..SimConfig::default()
        },
    )
    .expect("Sky Deck loads")
}

/// A fresh simulation of the bundle (default config).
pub fn load() -> Simulation {
    Simulation::load(bundle(), SimConfig::default()).expect("Sky Deck loads")
}

/// Runs `f` on a thread with a 64 MiB stack (script backends recurse).
pub fn big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .name("spec-6.2-sim".into())
        .stack_size(64 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

// ---------------------------------------------------------------------------
// Persist keys
// ---------------------------------------------------------------------------

/// Index of `key` in the bundle's `[persist] keys` list (brief C5).
pub fn persist_index(key: &str) -> u16 {
    let keys = &table()
        .manifest
        .persist
        .as_ref()
        .expect("[persist] section")
        .keys;
    let i = keys
        .iter()
        .position(|k| k == key)
        .unwrap_or_else(|| panic!("[persist] keys has {key}"));
    u16::try_from(i).expect("key index fits u16")
}

/// The persistent value of `key`.
pub fn persist(sim: &Simulation, key: &str) -> i64 {
    sim.persist(persist_index(key))
}

// ---------------------------------------------------------------------------
// Inputs and time
// ---------------------------------------------------------------------------

/// One tick with cabinet input `input` pressed or released, through the
/// real input path (`SimStep::step_1ms`).
pub fn cabinet(sim: &mut Simulation, input: CabinetInput, pressed: bool) {
    let mut out = *sim.snapshot();
    let t = sim.tick() + 1;
    let ev = InputEvent::button(t * TICK_NS - TICK_NS / 2, input, pressed);
    sim.step_1ms(t, &[ev], &mut out);
}

/// Presses `input` (one tick), holds it `hold` ticks and releases it (one
/// tick): `hold + 2` ticks in all.
pub fn press(sim: &mut Simulation, input: CabinetInput, hold: u64) {
    cabinet(sim, input, true);
    sim.advance(hold);
    cabinet(sim, input, false);
}

/// Closing edges of switch `name` since load.
pub fn hits(sim: &Simulation, name: &str) -> u32 {
    sim.switch_hits(name)
        .unwrap_or_else(|| panic!("known switch {name}"))
}

/// Closes switch `name` for `hold` ticks, opens it and hands it back to
/// physics (a forced rule event, not a physics hit).
pub fn pulse(sim: &mut Simulation, name: &str, hold: u64) {
    sim.set_switch(name, true).expect("known switch");
    sim.advance(hold);
    sim.set_switch(name, false).expect("known switch");
    sim.advance(5);
    sim.release_switch(name).expect("known switch");
}

/// Steps one tick at a time until a ball is on the table; `false` after
/// `max` ticks.
pub fn wait_for_ball(sim: &mut Simulation, max: u64) -> bool {
    for _ in 0..max {
        if sim.balls_in_play() > 0 {
            return true;
        }
        sim.advance(1);
    }
    sim.balls_in_play() > 0
}

/// Steps until `cond` holds (checked after each tick); `false` after `max`.
pub fn run_until(
    sim: &mut Simulation,
    max: u64,
    mut cond: impl FnMut(&Simulation) -> bool,
) -> bool {
    for _ in 0..max {
        sim.advance(1);
        if cond(sim) {
            return true;
        }
    }
    false
}

/// Removes every ball (no drain: the rules see nothing).
pub fn clear_table(sim: &mut Simulation) {
    let handles: Vec<BallHandle> = sim.ball_handles().iter().copied().collect();
    for h in handles {
        assert!(sim.remove_ball(h));
    }
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

/// Whether every component of `b` is finite.
pub fn finite(b: &BallState) -> bool {
    b.position_m.iter().all(|v| v.is_finite())
        && b.velocity_m_s.iter().all(|v| v.is_finite())
        && b.angular_velocity_rad_s.iter().all(|v| v.is_finite())
}

/// No script fault, no queue overflow, no escaped or non-finite ball, every
/// ball inside the escape box.
pub fn assert_healthy(sim: &Simulation, what: &str) {
    assert!(
        !sim.script_faulted(),
        "{what}: script faulted: {:?}",
        sim.script_error()
    );
    assert_eq!(
        sim.script_queue_overflows(),
        0,
        "{what}: script queue overflow"
    );
    assert_eq!(sim.counters().balls_escaped, 0, "{what}: a ball escaped");
    let (lo, hi) = sim.escape_bounds();
    for h in sim.ball_handles() {
        let b = sim.ball(h).expect("live handle");
        assert!(finite(&b), "{what}: non-finite ball {b:?}");
        for k in 0..3 {
            assert!(
                (lo[k]..=hi[k]).contains(&b.position_m[k]),
                "{what}: ball outside the table box {b:?} ({lo:?}..{hi:?})"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Game helpers
// ---------------------------------------------------------------------------

/// The trough switch used for drains: the first `trough`-kind switch with a
/// position (else the first `trough`-kind switch).
pub fn trough() -> pin_assets::SwitchSi {
    let t = table();
    t.switches
        .iter()
        .find(|s| s.kind == SwitchKind::Trough && s.position_m.is_some())
        .or_else(|| t.switches.iter().find(|s| s.kind == SwitchKind::Trough))
        .expect("trough switch")
        .clone()
}

/// Drains ball `h` through the trough: moved onto the trough switch (when
/// it has a position; layer set to the switch's layer) or removed with the
/// switch pulsed. Returns after the drain registered.
pub fn drain_ball(sim: &mut Simulation, h: BallHandle) {
    let tr = trough();
    let Some(mut b) = sim.ball(h) else {
        panic!("drain_ball: ball already gone")
    };
    match tr.position_m {
        Some(p) => {
            let r = pin_core::builtin_profile(
                pin_core::profiles::BUILTIN_PROFILE_NAMES[usize::from(b.profile.0)],
            )
            .expect("profile")
            .radius_m();
            b.position_m = [p[0], p[1], p[2] + r];
            b.velocity_m_s = [0.0; 3];
            b.angular_velocity_rad_s = [0.0; 3];
            b.layer = tr.layer;
            assert!(sim.set_ball_state(h, &b), "move the ball onto the trough");
            let ok = run_until(sim, 50, |s| s.ball(h).is_none());
            assert!(ok, "the ball drained into the trough");
        }
        None => {
            assert!(sim.remove_ball(h));
            pulse(sim, &tr.name, 5);
        }
    }
}

/// Starts a game with the cabinet `start` button and waits (<= 10 s) for
/// the served ball. Asserts `game_state` 1 and `ball` 1.
pub fn start_game(sim: &mut Simulation) {
    press(sim, CabinetInput::Start, 50);
    assert!(wait_for_ball(sim, 10_000), "start serves a ball");
    assert_eq!(persist(sim, "game_state"), 1, "game_state after start");
    assert_eq!(persist(sim, "ball"), 1, "ball after start");
}

/// A simulation `warm` ticks after load with a game started (the served
/// ball is on the table).
pub fn game_seeded(seed: u64) -> Simulation {
    let mut sim = load_seeded(seed);
    sim.advance(200);
    start_game(&mut sim);
    sim
}

/// The handle of the only steel ball on layer 0 (panics otherwise).
pub fn steel_ball(sim: &Simulation) -> BallHandle {
    let v: Vec<BallHandle> = sim
        .ball_handles()
        .iter()
        .copied()
        .filter(|&h| {
            sim.ball(h)
                .is_some_and(|b| b.profile.0 == STEEL_ID && b.layer == 0)
        })
        .collect();
    assert_eq!(v.len(), 1, "exactly one steel ball on layer 0");
    v[0]
}

/// Handles of the `uv_globall` balls on the table.
pub fn glo_balls(sim: &Simulation) -> Vec<BallHandle> {
    sim.ball_handles()
        .iter()
        .copied()
        .filter(|&h| sim.ball(h).is_some_and(|b| b.profile.0 == GLO_ID))
        .collect()
}

// ---------------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------------

/// Dot product.
pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Length.
pub fn len(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

/// `a * k`.
pub fn scale(a: [f32; 3], k: f32) -> [f32; 3] {
    [a[0] * k, a[1] * k, a[2] * k]
}

/// `a + b`.
pub fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Unit vector at `angle_deg` in the playfield plane (90 deg = up the table).
pub fn dir(angle_deg: f32) -> [f32; 3] {
    let a = angle_deg.to_radians();
    [a.cos(), a.sin(), 0.0]
}

/// Plan-view distance of two points, m.
pub fn plan_dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// Even-odd point-in-polygon (mm).
pub fn inside(poly: &[[f32; 2]], p: [f32; 2]) -> bool {
    let mut c = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            c = !c;
        }
        j = i;
    }
    c
}

/// Distance (mm) of `p` from the polygon's boundary.
pub fn edge_distance(poly: &[[f32; 2]], p: [f32; 2]) -> f32 {
    let n = poly.len();
    let mut best = f32::INFINITY;
    for i in 0..n {
        let a = poly[i];
        let b = poly[(i + 1) % n];
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l2 = dx * dx + dy * dy;
        let t = if l2 > 0.0 {
            (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let q = [a[0] + t * dx, a[1] + t * dy];
        best = best.min((p[0] - q[0]).hypot(p[1] - q[1]));
    }
    best
}

// ---------------------------------------------------------------------------
// Physics-contact helpers (shared by the mechanism, rule and alloc tests)
// ---------------------------------------------------------------------------

/// The `[[mechs]]` entry `id`.
pub fn mech(id: &str) -> &'static pin_core::schema::table::MechDef {
    table()
        .manifest
        .mechs
        .iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("mech {id}"))
}

/// Index of mech `id` in the loaded table's mech list (the `MechSet` order).
pub fn mech_index(id: &str) -> usize {
    table()
        .mechs
        .iter()
        .position(|m| m.name == id)
        .unwrap_or_else(|| panic!("mech {id} loaded"))
}

/// Centre of a steel ball resting on the surface at mech point `p_mm`,
/// offset `d` m along `u`.
pub fn at(p_mm: [f32; 3], u: [f32; 3], d: f32) -> [f32; 3] {
    [
        p_mm[0] * 1e-3 + u[0] * d,
        p_mm[1] * 1e-3 + u[1] * d,
        p_mm[2] * 1e-3 + steel_radius_m(),
    ]
}

/// Steps until the physics closes switch `sw` (hit count rises) or `max`
/// ticks pass, asserting every ball stays finite; the tick of the hit.
pub fn wait_hit(sim: &mut Simulation, sw: &str, max: u64) -> Option<u64> {
    let before = hits(sim, sw);
    for _ in 0..max {
        sim.advance(1);
        for h in sim.ball_handles() {
            assert!(finite(&sim.ball(h).expect("live")), "finite balls");
        }
        if hits(sim, sw) > before {
            return Some(sim.tick());
        }
    }
    None
}

/// Rolls a deck ball into drop target `id`'s face (`angle_deg` = face
/// normal) at `speed` from 40 mm; returns the hit tick. The ball is removed
/// 5 ticks after the hit (or after 400 ticks), before it could drain.
pub fn strike_drop(sim: &mut Simulation, id: &str, speed: f32) -> Option<u64> {
    let def = mech(id);
    let p = def.position.expect("drop position");
    let u = dir(def.angle_deg.unwrap_or(90.0));
    let h = sim
        .spawn_ball_on_layer(
            STEEL,
            def.layer.unwrap_or(0),
            at(p, u, 0.04),
            scale(u, -speed),
        )
        .expect("spawn");
    let sw = def.switch.as_deref().expect("drop switch");
    let hit = wait_hit(sim, sw, 400);
    sim.advance(5);
    let _ = sim.remove_ball(h);
    hit
}

/// Unit plan-view tangent of the first Newton chain's path at its first
/// captive ball, and that ball's centre.
pub fn newton_aim(sim: &Simulation) -> ([f32; 3], [f32; 3]) {
    let chain = sim.solver().newton_chain(0);
    let c0 = chain.ball_position(0).to_array();
    let t = chain.path().tangent(chain.ball_t(0)).to_array();
    let t = [t[0], t[1], 0.0];
    (scale(t, 1.0 / len(t).max(1e-9)), c0)
}

/// Rolls a ball along the captive path into the Newton ball from 45 mm
/// at `speed`; returns the `newton_sw` hit tick. The striker is removed 5
/// ticks after the hit (or after 300 ticks).
pub fn strike_newton(sim: &mut Simulation, speed: f32) -> Option<u64> {
    let def = mech("newton");
    let (d, c0) = newton_aim(sim);
    let pos = [c0[0] - d[0] * 0.045, c0[1] - d[1] * 0.045, c0[2]];
    let h = sim
        .spawn_ball_on_layer(STEEL, def.layer.unwrap_or(0), pos, scale(d, speed))
        .expect("spawn");
    let hit = wait_hit(sim, "newton_sw", 300);
    sim.advance(5);
    let _ = sim.remove_ball(h);
    hit
}

/// Spawns a ball at rest on the VUK (layer of the mech); its handle.
pub fn ball_into_vuk(sim: &mut Simulation) -> BallHandle {
    let def = mech("vuk");
    let p = def.position.expect("vuk position");
    sim.spawn_ball_on_layer(
        STEEL,
        def.layer.unwrap_or(0),
        at(p, [0.0; 3], 0.0),
        [0.0; 3],
    )
    .expect("spawn into the VUK")
}
