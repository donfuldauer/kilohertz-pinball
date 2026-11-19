//! Shared helpers of the Sky Deck (Subphase 6.2) tests: building the bundle
//! from `tables/sky_deck` exactly as `tables/sky_deck/build.sh` does, and
//! the small simulation helpers the suites share.
#![allow(dead_code)]

use pin_assetgen::{TableMeta, generate_kit};
use pin_core::schema::table::TableManifest;
use pin_sim::{BallHandle, SimConfig, Simulation};
use pin_tablegen::layout::parse_layout;
use pin_tablegen::{generate, write_bundle};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// `tables/sky_deck` (the sources).
pub fn sky_deck_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/sky_deck")
}

/// Files copied unchanged from the sources into the bundle besides every
/// file of `display/` (as `tables/sky_deck/build.sh` does; `pin_sim` reads
/// `wall_height` from `layout.toml` and the glass underside from
/// `glass.toml`).
pub const COPIED: [&str; 4] = ["rules.lua", "backglass.toml", "layout.toml", "glass.toml"];

fn parse_hex(c: &str) -> [u8; 3] {
    let h = c.trim_start_matches('#');
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).expect("palette colour");
    [b(0), b(2), b(4)]
}

/// Every file of `tables/sky_deck/display`, sorted by name.
pub fn display_files() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(sky_deck_src().join("display"))
        .expect("display/")
        .map(|e| e.expect("display entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    v.sort();
    v
}

/// Builds the Sky Deck bundle into `out_dir` (what `tables/sky_deck/build.sh`
/// does): `pin_tablegen` generates `playfield.glb` and the merged
/// `table.toml` from `layout.toml` + `table.toml`, the rules, backglass,
/// layout, glass and display scenes are copied, and the 4.6 placeholder kit
/// (`pin_assetgen::generate_kit`) adds the sounds, fonts and backglass art.
pub fn build_sky_deck_bundle(out_dir: &Path) {
    let src = sky_deck_src();
    let layout_text = std::fs::read_to_string(src.join("layout.toml")).expect("layout.toml");
    let table_text = std::fs::read_to_string(src.join("table.toml")).expect("table.toml");
    let layout = parse_layout(&layout_text).expect("layout.toml parses");
    let base = TableManifest::from_toml_str(&table_text).expect("table.toml parses");
    let generated = generate(&layout, &base).unwrap_or_else(|e| panic!("pin_tablegen: {e}"));
    write_bundle(&generated, out_dir).expect("write bundle");
    for f in COPIED {
        std::fs::copy(src.join(f), out_dir.join(f)).unwrap_or_else(|e| panic!("copy {f}: {e}"));
    }
    std::fs::create_dir_all(out_dir.join("display")).unwrap();
    for p in display_files() {
        let to = out_dir.join("display").join(p.file_name().unwrap());
        std::fs::copy(&p, &to).unwrap_or_else(|e| panic!("copy {}: {e}", p.display()));
    }
    let meta = TableMeta {
        title: base.meta.name.clone(),
        palette: base.meta.palette.iter().map(|c| parse_hex(c)).collect(),
        seed: 0,
    };
    generate_kit(&meta, out_dir).expect("placeholder kit");
}

/// A fresh scratch directory under the target dir (tests never share one).
pub fn scratch(tag: &str) -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let p = base.join(format!(
        "pin_app_{tag}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// The Sky Deck bundle, built once per test binary.
pub fn sky_deck_bundle() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = scratch("sky_deck_bundle");
        build_sky_deck_bundle(&d);
        d
    })
}

/// The merged manifest of the built bundle (`table.toml`).
pub fn bundle_manifest() -> TableManifest {
    let text = std::fs::read_to_string(sky_deck_bundle().join("table.toml")).expect("table.toml");
    TableManifest::from_toml_str(&text).expect("bundle table.toml parses")
}

/// Runs `f` on a thread with an 8 MiB stack.
pub fn on_big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// Persist key indices (`table.toml` `[persist] keys` order).
pub mod key {
    /// `games_played`.
    pub const GAMES: u16 = 0;
    /// `high_score`.
    pub const HIGH: u16 = 1;
    /// `ball`.
    pub const BALL: u16 = 2;
    /// `game_state` (0 attract, 1 game, 2 game over).
    pub const STATE: u16 = 3;
    /// `mode:night_overdrive`.
    pub const MODE: u16 = 4;
    /// `bridge_open`.
    pub const BRIDGE: u16 = 5;
    /// `overdrive_ready`.
    pub const READY: u16 = 6;
}

/// Loads the bundle with `seed`.
pub fn load_seeded(seed: u64) -> Simulation {
    let cfg = SimConfig {
        seed,
        ..SimConfig::default()
    };
    Simulation::load(sky_deck_bundle(), cfg).unwrap_or_else(|e| panic!("{e}"))
}

/// Loads the bundle with the default seed.
pub fn load() -> Simulation {
    Simulation::load(sky_deck_bundle(), SimConfig::default()).unwrap_or_else(|e| panic!("{e}"))
}

/// Presses cabinet switch `name` for `ticks` ticks.
pub fn press(s: &mut Simulation, name: &str, ticks: u64) {
    s.set_switch(name, true).unwrap();
    s.advance(ticks);
    s.release_switch(name).unwrap();
}

/// Ball centre at (x, y) mm on a surface at `z` mm (m).
pub fn at(x: f32, y: f32, z: f32) -> [f32; 3] {
    [x * 1.0e-3, y * 1.0e-3, (z + 13.5) * 1.0e-3]
}

/// Ball centre at (x, y) mm on the main deck (m).
pub fn mm(x: f32, y: f32) -> [f32; 3] {
    at(x, y, 0.0)
}

/// Upper-deck surface height, mm.
pub const DECK_Z_MM: f32 = 65.0;

/// No script fault, queue overflow or dropped edge; nothing escaped.
pub fn healthy(s: &Simulation) {
    assert!(!s.script_faulted(), "script fault: {:?}", s.script_error());
    assert_eq!(s.script_queue_overflows(), 0, "script queue overflow");
    let c = s.counters();
    assert_eq!(c.edges_dropped, 0, "{c:?}");
    assert_eq!(c.spawns_failed, 0, "{c:?}");
    assert_eq!(c.balls_escaped, 0, "{c:?}");
}

/// Starts a game: the first ball lands on the plunger.
pub fn start_game(s: &mut Simulation) {
    press(s, "start", 5);
    s.advance(1500);
    assert_eq!(s.persist(key::STATE), 1, "game state");
    assert_eq!(s.persist(key::BALL), 1);
    assert_eq!(s.balls_in_play(), 1);
}

/// The layer of every ball on the table, ascending slot.
pub fn layers(s: &Simulation) -> Vec<u8> {
    s.ball_handles()
        .iter()
        .filter_map(|h| s.ball(*h).map(|b| b.layer))
        .collect()
}

/// The one ball on the table (panics otherwise).
pub fn only_ball(s: &Simulation) -> BallHandle {
    let hs = s.ball_handles();
    assert_eq!(hs.len(), 1, "expected one ball");
    hs[0]
}

/// The id of a built-in ball profile (its index in
/// `BUILTIN_PROFILE_NAMES`, as the simulation numbers them).
pub fn profile_id(name: &str) -> pin_core::ProfileId {
    let i = pin_core::profiles::BUILTIN_PROFILE_NAMES
        .iter()
        .position(|&n| n == name)
        .unwrap_or_else(|| panic!("profile {name}"));
    pin_core::ProfileId(i as u16)
}

/// Handles of the balls of `profile` on the table, ascending slot.
pub fn balls_of(s: &Simulation, profile: &str) -> Vec<BallHandle> {
    let id = profile_id(profile);
    s.ball_handles()
        .into_iter()
        .filter(|h| s.ball(*h).is_some_and(|b| b.profile == id))
        .collect()
}

/// Moves ball `h` to the drain (layer 0, at rest over both trough zones);
/// it drains in the next tick.
pub fn drain(s: &mut Simulation, h: BallHandle) {
    let mut b = s.ball(h).expect("ball to drain");
    b.position_m = mm(257.0, 22.0);
    b.velocity_m_s = [0.0; 3];
    b.angular_velocity_rad_s = [0.0; 3];
    b.layer = 0;
    assert!(s.set_ball_state(h, &b));
}

/// Moves ball `h` (layer 0) onto the centre VUK's hold point at rest; the
/// scoop captures it in the next tick.
pub fn put_in_vuk(s: &mut Simulation, h: BallHandle) {
    let mut b = s.ball(h).expect("ball for the VUK");
    b.position_m = mm(257.0, 692.0);
    b.velocity_m_s = [0.0; 3];
    b.angular_velocity_rad_s = [0.0; 3];
    b.layer = 0;
    assert!(s.set_ball_state(h, &b));
}

/// Closes switch `name` for 3 ticks (a closing edge for the rules), then
/// gives it back to physics.
pub fn hit(s: &mut Simulation, name: &str) {
    s.set_switch(name, true).unwrap();
    s.advance(3);
    s.release_switch(name).unwrap();
    s.advance(1);
}

/// Whether the GI tint is a UV tint: blue above red and green, intensity
/// above 0.3.
pub fn is_uv(gi: [f32; 4]) -> bool {
    gi[2] > gi[0] && gi[2] > gi[1] && gi[3] > 0.3
}

/// The default (white) GI tint.
pub const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// How long the rules hold a ball in the centre VUK before the lift, ticks
/// (`rules.lua` `VUK_HOLD`).
pub const VUK_HOLD_TICKS: u64 = 4000;
