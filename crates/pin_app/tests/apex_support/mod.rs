//! Shared helpers of the 6.1 Apex Circuit tests (`apex_*.rs`): building the
//! bundle from `tables/apex_circuit` (the logic of `tests/support/mod.rs`
//! for the grey box), name lookups and a big-stack runner.
#![allow(dead_code)]

use pin_assetgen::{TableMeta, generate_kit};
use pin_core::schema::table::TableManifest;
use pin_sim::Simulation;
use pin_tablegen::layout::parse_layout;
use pin_tablegen::{generate, write_bundle};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// `tables/apex_circuit` (the sources).
pub fn apex_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/apex_circuit")
}

/// Display scenes of the table (`display/<name>.toml`).
pub const SCENES: [&str; 11] = [
    "scene_attract",
    "scene_score",
    "scene_game_over",
    "scene_powerball_mania",
    "scene_combo_chain",
    "scene_overdrive",
    "scene_bonus",
    "scene_high_score",
    "scene_skill_shot",
    "scene_firewall_save",
    "scene_apex_jackpot",
];

/// Files copied unchanged from the sources into the bundle (as
/// `tables/apex_circuit/build.sh` does), besides the display scenes.
pub const COPIED: [&str; 4] = ["rules.lua", "backglass.toml", "layout.toml", "glass.toml"];

fn parse_hex(c: &str) -> [u8; 3] {
    let h = c.trim_start_matches('#');
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).expect("palette colour");
    [b(0), b(2), b(4)]
}

/// Builds the Apex Circuit bundle into `out_dir` (what
/// `tables/apex_circuit/build.sh` does): `pin_tablegen` generates
/// `playfield.glb` and the merged `table.toml`, the rules, backglass,
/// layout, glass and display scenes are copied, and the placeholder kit
/// (`pin_assetgen::generate_kit`) adds the sounds, fonts and backglass art.
pub fn build_apex_bundle(out_dir: &Path) {
    let src = apex_src();
    let layout_text = std::fs::read_to_string(src.join("layout.toml")).expect("layout.toml");
    let table_text = std::fs::read_to_string(src.join("table.toml")).expect("table.toml");
    let layout = parse_layout(&layout_text).expect("layout.toml parses");
    let base = TableManifest::from_toml_str(&table_text).expect("table.toml parses");
    let generated = generate(&layout, &base).unwrap_or_else(|e| panic!("pin_tablegen: {e}"));
    write_bundle(&generated, out_dir).expect("write bundle");
    let scenes = SCENES.iter().map(|s| format!("display/{s}.toml"));
    for f in COPIED.iter().map(|s| (*s).to_owned()).chain(scenes) {
        let to = out_dir.join(&f);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(src.join(&f), &to).unwrap_or_else(|e| panic!("copy {f}: {e}"));
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

/// The Apex Circuit bundle, built once per test binary.
pub fn apex_bundle() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = scratch("apex_bundle");
        build_apex_bundle(&d);
        d
    })
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

/// The `[persist]` key index of `name`.
pub fn persist_key(sim: &Simulation, name: &str) -> u16 {
    sim.ids()
        .get(pin_assets::IdKind::PersistKey, name)
        .unwrap_or_else(|| panic!("persist key {name}"))
}

/// The scene id of `name`.
pub fn scene_id(sim: &Simulation, name: &str) -> u16 {
    sim.ids()
        .get(pin_assets::IdKind::Scene, name)
        .unwrap_or_else(|| panic!("scene {name}"))
}

/// Millimetres on the playfield to a ball-centre position, m.
pub fn mm(x: f32, y: f32) -> [f32; 3] {
    [x * 1.0e-3, y * 1.0e-3, 0.0135]
}
