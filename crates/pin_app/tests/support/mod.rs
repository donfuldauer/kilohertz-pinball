//! Shared helpers of the 6.0 integration and greybox tests: building the
//! grey-box bundle from `tables/greybox` and running a simulation on a
//! thread with a big stack.
#![allow(dead_code)]

use pin_assetgen::{TableMeta, generate_kit};
use pin_core::schema::table::TableManifest;
use pin_tablegen::layout::parse_layout;
use pin_tablegen::{generate, write_bundle};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

/// `tables/greybox` (the sources).
pub fn greybox_src() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/greybox")
}

/// Files copied unchanged from the sources into the bundle (as
/// `tables/greybox/build.sh` does; `pin_sim` reads `wall_height` from
/// `layout.toml` and the glass underside from `glass.toml`).
pub const COPIED: [&str; 8] = [
    "rules.lua",
    "backglass.toml",
    "layout.toml",
    "glass.toml",
    "display/scene_attract.toml",
    "display/scene_score.toml",
    "display/scene_game_over.toml",
    "display/scene_powerball_mania.toml",
];

fn parse_hex(c: &str) -> [u8; 3] {
    let h = c.trim_start_matches('#');
    let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).expect("palette colour");
    [b(0), b(2), b(4)]
}

/// Builds the grey-box bundle into `out_dir` (what `tables/greybox/build.sh`
/// does): `pin_tablegen` generates `playfield.glb` and the merged
/// `table.toml` from `layout.toml` + `table.toml`, the rules, backglass,
/// layout, glass and display scenes are copied, and the 4.6 placeholder kit
/// (`pin_assetgen::generate_kit`) adds the sounds, fonts and backglass art.
pub fn build_greybox_bundle(out_dir: &Path) {
    build_greybox_bundle_with_layout_extra(out_dir, "");
}

/// [`build_greybox_bundle`] with `layout_extra` appended to the layout text
/// `pin_tablegen` generates the glTF from (the copied `layout.toml` stays the
/// grey box's: the extra only adds visual elements such as `[[inserts]]`).
pub fn build_greybox_bundle_with_layout_extra(out_dir: &Path, layout_extra: &str) {
    let src = greybox_src();
    let mut layout_text = std::fs::read_to_string(src.join("layout.toml")).expect("layout.toml");
    layout_text.push('\n');
    layout_text.push_str(layout_extra);
    let table_text = std::fs::read_to_string(src.join("table.toml")).expect("table.toml");
    let layout = parse_layout(&layout_text).expect("layout.toml parses");
    let base = TableManifest::from_toml_str(&table_text).expect("table.toml parses");
    let generated = generate(&layout, &base).unwrap_or_else(|e| panic!("pin_tablegen: {e}"));
    write_bundle(&generated, out_dir).expect("write bundle");
    for f in COPIED {
        let to = out_dir.join(f);
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(src.join(f), &to).unwrap_or_else(|e| panic!("copy {f}: {e}"));
    }
    let meta = TableMeta {
        title: base.meta.name.clone(),
        palette: base.meta.palette.iter().map(|c| parse_hex(c)).collect(),
        seed: 0,
    };
    generate_kit(&meta, out_dir).expect("placeholder kit");
}

/// A fresh scratch directory under the target dir (removed by the OS / CI
/// cleanup; tests never share one).
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

/// The grey-box bundle, built once per test binary.
pub fn greybox_bundle() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = scratch("greybox_bundle");
        build_greybox_bundle(&d);
        d
    })
}

/// Runs `f` on a thread with an 8 MiB stack (Wasm needs 2 MiB; the Lua
/// tables run anywhere, but the tests keep one rule).
pub fn on_big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

/// Two `[[inserts]]` for the grey box (lamps `powerball_lamp` and
/// `backglass_logo`): `inserts[0]` on the bare lower playfield, `inserts[1]`
/// under the acrylic upper deck (x 300..460, y 860..1040 mm).
pub const GREYBOX_INSERTS: &str = "\
[[inserts]]
lamp = \"powerball_lamp\"
position = [200.0, 300.0]
shape = \"circle\"
size = 60.0

[[inserts]]
lamp = \"backglass_logo\"
position = [380.0, 950.0]
shape = \"circle\"
size = 60.0
";

/// The grey-box bundle with [`GREYBOX_INSERTS`], built once per test binary.
pub fn greybox_inserts_bundle() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let d = scratch("greybox_inserts_bundle");
        build_greybox_bundle_with_layout_extra(&d, GREYBOX_INSERTS);
        d
    })
}
