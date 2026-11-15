//! Shared helpers of the Subphase 5.5 tablegen tests (`backbox_*.rs`):
//! the Appendix A example bundle (`apex`) with a generated backbox.
//! Included with `#[path]`; an empty test crate on its own.
#![allow(dead_code)]

use pin_assets::{LoadedTable, Vfs, load_table};
use pin_core::schema::table::{CoilDef, CoilKind, SwitchDef, SwitchKind, TableManifest};
use pin_tablegen::backbox::{
    GeneratedBackbox, generate_backbox, parse_backbox_layout, write_backbox,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The backbox layout used by the tests: three flippers, one per staging
/// mode, a VUK-fed entry and a drain.
pub const LAYOUT: &str = r#"
schema_version = 1

[backbox]
origin = [0.0, 1150.0, 250.0]
width = 400.0
height = 300.0
depth = 35.0

[[backbox.flippers]]
side = "left"
position = [-70.0, 50.0]
coil = "bb_left_coil"
eos_switch = "bb_left_eos"
staging = { type = "DedicatedButton", input = "left_upper" }

[[backbox.flippers]]
side = "right"
position = [70.0, 50.0]
coil = "bb_right_coil"
eos_switch = "bb_right_eos"
staging = { type = "ParallelLinked", input = "right_flipper" }

[[backbox.flippers]]
side = "left"
position = [-120.0, 180.0]
length = 40.0
coil = "bb_upper_coil"
eos_switch = "bb_upper_eos"
staging = { type = "DualStageThreshold", axis = "left_flipper", lower_threshold = 0.3, upper_threshold = 0.85 }

[backbox.entry]
vuk = [120.0, 1000.0]
at = -150.0
coil = "bb_vuk_coil"
switch = "bb_vuk_switch"

[backbox.drain]
at = 160.0
"#;

/// The Appendix A example bundle of pin_assets (read-only).
pub fn apex_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../pin_assets/tests/fixtures/apex")
}

/// The apex `table.toml` plus the coils and switches [`LAYOUT`] names.
pub fn base() -> TableManifest {
    let text = std::fs::read_to_string(apex_dir().join("table.toml")).unwrap();
    let mut t = TableManifest::from_toml_str(&text).unwrap();
    for (i, (id, kind)) in [
        ("bb_left_coil", CoilKind::Flipper),
        ("bb_right_coil", CoilKind::Flipper),
        ("bb_upper_coil", CoilKind::Flipper),
        ("bb_vuk_coil", CoilKind::Vuk),
    ]
    .into_iter()
    .enumerate()
    {
        t.coils.push(CoilDef {
            id: id.into(),
            number: 90 + i as u16,
            kind,
        });
    }
    for (i, (id, kind)) in [
        ("bb_left_eos", SwitchKind::Leaf),
        ("bb_right_eos", SwitchKind::Leaf),
        ("bb_upper_eos", SwitchKind::Leaf),
        ("bb_vuk_switch", SwitchKind::Opto),
    ]
    .into_iter()
    .enumerate()
    {
        t.switches.push(SwitchDef {
            id: id.into(),
            number: 180 + i as u16,
            kind,
            layer: 0,
            position: None,
            radius: None,
        });
    }
    t
}

/// [`LAYOUT`] generated against [`base`].
pub fn generated() -> GeneratedBackbox {
    let layout = parse_backbox_layout(LAYOUT).unwrap_or_else(|e| panic!("{e}"));
    generate_backbox(&layout.backbox, &base()).unwrap_or_else(|e| panic!("{e}"))
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A fresh copy of the apex bundle in a unique temporary folder.
pub fn bundle_dir(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pin_tablegen_backbox_{tag}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&apex_dir(), &dir);
    dir
}

/// A fresh apex bundle with the generated playfield (`apex_layout.toml`)
/// and then the generated backbox ([`LAYOUT`]), loaded with the runtime
/// loader.
pub fn load(tag: &str) -> (PathBuf, LoadedTable, GeneratedBackbox) {
    let dir = bundle_dir(tag);
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/apex_layout.toml"),
    )
    .unwrap();
    let layout = pin_tablegen::layout::parse_layout(&text).unwrap();
    let main = pin_tablegen::generate(&layout, &base()).unwrap();
    pin_tablegen::write_bundle(&main, &dir).unwrap();
    let bb = parse_backbox_layout(LAYOUT).unwrap();
    let g = generate_backbox(&bb.backbox, &main.table).unwrap_or_else(|e| panic!("{e}"));
    write_backbox(&g, &dir).unwrap();
    let vfs = Vfs::open(&dir).unwrap();
    let table = load_table(&vfs).unwrap_or_else(|e| panic!("{e}"));
    (dir, table, g)
}

/// FNV-1a 64-bit hash.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}
