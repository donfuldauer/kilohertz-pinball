//! Shared helpers for the pin_tablegen implementer tests.
#![allow(dead_code)]

use pin_assets::{GltfScene, LoadedTable, Vfs, load_gltf, load_table};
use pin_core::schema::{LayoutManifest, TableManifest};
use pin_tablegen::layout::{GeneratedTable, generate, write_bundle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The Appendix A example bundle of pin_assets (read-only).
pub fn apex_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../pin_assets/tests/fixtures/apex")
}

pub fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

pub fn apex_base() -> TableManifest {
    TableManifest::from_toml_str(&std::fs::read_to_string(apex_dir().join("table.toml")).unwrap())
        .unwrap()
}

pub fn layout(name: &str) -> LayoutManifest {
    LayoutManifest::from_toml_str(&fixture(name)).unwrap()
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

/// A fresh copy of the apex bundle (plus the `backbox.glb` its
/// backglass.toml names) in a unique temporary folder.
pub fn bundle_dir(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "pin_tablegen_{tag}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(&apex_dir(), &dir);
    std::fs::write(dir.join("backbox.glb"), b"placeholder").unwrap();
    dir
}

/// Writes `generated` into a fresh apex bundle and loads it.
pub fn load(generated: &GeneratedTable, tag: &str) -> (PathBuf, LoadedTable, GltfScene) {
    let dir = bundle_dir(tag);
    write_bundle(generated, &dir).unwrap();
    let vfs = Vfs::open(&dir).unwrap();
    let table = load_table(&vfs).unwrap_or_else(|e| panic!("{e}"));
    let scene = load_gltf(&vfs, "playfield.glb").unwrap();
    (dir, table, scene)
}

pub fn gen_apex(name: &str) -> GeneratedTable {
    generate(&layout(name), &apex_base()).unwrap_or_else(|e| panic!("{e}"))
}
