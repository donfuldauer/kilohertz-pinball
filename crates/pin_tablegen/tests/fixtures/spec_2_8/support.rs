//! Shared helpers for the Subphase 2.8 spec tests (Independent Tester).
//!
//! Black box: bundles are produced with the merged 4.3 generator
//! (`pin_tablegen::layout::{generate, write_bundle}`) from text fixtures and
//! checked through the published 2.8 contract (DECISIONS.md "2.8 — Clearance
//! validator public API").

#![allow(dead_code)]

use pin_assets::{GltfScene, LoadedTable, Vfs, load_gltf, load_table};
use pin_core::schema::layout::LayoutManifest;
use pin_core::schema::table::TableManifest;
use pin_tablegen::clearance::{ClearanceError, ClearanceReport, Feature, validate_clearance};
use pin_tablegen::layout::{generate, write_bundle};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

pub const XL: &str = "jumbo_xl_resin";
pub const STD: &str = "standard_steel";
pub const MINI: &str = "mini_steel";
pub const JUMBO: &str = "jumbo_steel";

/// Ball diameter of a built-in profile, mm.
pub fn diameter_mm(profile: &str) -> f32 {
    pin_core::builtin_profile(profile)
        .unwrap_or_else(|| panic!("unknown built-in profile {profile}"))
        .diameter_m
        * 1000.0
}

pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spec_2_8")
}

fn read_fixture(name: &str) -> String {
    std::fs::read_to_string(fixture_dir().join(name)).unwrap()
}

/// `table_base.toml` with `balls` (the body of the `[balls]` section) and
/// `extra` (further sections) inserted.
pub fn table_toml(balls: &str, extra: &str) -> String {
    let base = read_fixture("table_base.toml");
    let marker = base
        .lines()
        .find(|l| l.contains("@BALLS@"))
        .expect("marker line");
    let mut out = base.replace(marker, &format!("[balls]\n{balls}\n"));
    out.push('\n');
    out.push_str(extra);
    out.push('\n');
    out
}

/// `[balls]` body declaring `profiles` and no restriction.
pub fn balls(profiles: &[&str]) -> String {
    format!(
        "profiles = [{}]",
        profiles
            .iter()
            .map(|p| format!("{p:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// `[balls]` body declaring `profiles` and one restriction of `profile`.
pub fn balls_restricted(
    profiles: &[&str],
    profile: &str,
    layers: &[u8],
    area: Option<&[[f32; 2]]>,
) -> String {
    let area = area.map_or(String::new(), |a| {
        format!(
            ", area = [{}]",
            a.iter()
                .map(|p| format!("[{:?}, {:?}]", p[0], p[1]))
                .collect::<Vec<_>>()
                .join(", ")
        )
    });
    format!(
        "{}\nrestrict = [ {{ profile = {profile:?}, layers = {layers:?}{area} }} ]",
        balls(profiles)
    )
}

/// `open_layout.toml` plus `extra` sections.
pub fn layout_toml(extra: &str) -> String {
    let mut s = read_fixture("open_layout.toml");
    s.push('\n');
    s.push_str(extra);
    s.push('\n');
    s
}

/// A fresh, empty directory under the cargo test tmpdir.
pub fn temp_dir(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("spec_2_8-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_tree(from: &Path, to: &Path, skip_root: &[&str]) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if skip_root.iter().any(|s| name.to_str() == Some(*s)) {
            continue;
        }
        let dst = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dst, &[]);
        } else {
            std::fs::copy(entry.path(), dst).unwrap();
        }
    }
}

/// Everything a bundle needs besides `table.toml` and `playfield.glb`
/// (backglass.toml, rules.lua, audio, display): the 4.3 spec base bundle.
pub fn copy_base_extras(dir: &Path) {
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spec_4_3/base"),
        dir,
        &["table.toml", "playfield.glb"],
    );
}

/// Generates a bundle directory from layout and table text. `Err` carries
/// the generator's or the parser's message.
pub fn try_bundle(tag: &str, layout: &str, table: &str) -> Result<PathBuf, String> {
    let layout = LayoutManifest::from_toml_str(layout).map_err(|e| format!("layout: {e}"))?;
    let table = TableManifest::from_toml_str(table).map_err(|e| format!("table: {e}"))?;
    let generated = generate(&layout, &table).map_err(|e| format!("generate: {e}"))?;
    let dir = temp_dir(tag);
    copy_base_extras(&dir);
    write_bundle(&generated, &dir).map_err(|e| format!("write_bundle: {e}"))?;
    Ok(dir)
}

pub fn bundle(tag: &str, layout: &str, table: &str) -> PathBuf {
    try_bundle(tag, layout, table).unwrap_or_else(|e| panic!("{tag}: fixture does not build: {e}"))
}

pub struct Loaded {
    pub table: LoadedTable,
    pub scene: GltfScene,
}

pub fn try_load(dir: &Path) -> Result<Loaded, String> {
    let vfs = Vfs::open(dir).map_err(|e| format!("Vfs::open: {e}"))?;
    let table = load_table(&vfs).map_err(|e| format!("load_table: {e}"))?;
    let scene = load_gltf(&vfs, &table.manifest.meta.playfield_mesh)
        .map_err(|e| format!("load_gltf: {e}"))?;
    Ok(Loaded { table, scene })
}

/// Loads a bundle that must pass the 4.1 validation.
pub fn load(dir: &Path) -> Loaded {
    try_load(dir).unwrap_or_else(|e| panic!("fixture bundle does not load: {e}"))
}

/// Builds, loads and validates in one go.
pub fn validate(tag: &str, layout: &str, table: &str) -> ClearanceReport {
    let dir = bundle(tag, layout, table);
    let l = load(&dir);
    let report = validate_clearance(&l.table, &l.scene);
    let _ = std::fs::remove_dir_all(&dir);
    report
}

pub fn describe(report: &ClearanceReport) -> String {
    let mut s = String::new();
    for e in &report.errors {
        s.push_str(&format!(
            "  error   [{:?} {:?} {}] {e}\n",
            e.feature, e.name, e.profile
        ));
    }
    for w in &report.warnings {
        s.push_str(&format!(
            "  warning [{:?} {:?} {}] {w}\n",
            w.feature, w.name, w.profile
        ));
    }
    if s.is_empty() {
        s.push_str("  (no errors, no warnings)\n");
    }
    s
}

pub fn assert_accepts(report: &ClearanceReport, what: &str) {
    assert!(
        report.is_ok() && report.errors.is_empty(),
        "{what}: must be accepted, got\n{}",
        describe(report)
    );
}

/// The errors of `feature` for `profile`.
pub fn errors_of<'a>(
    report: &'a ClearanceReport,
    feature: Feature,
    profile: &str,
) -> Vec<&'a ClearanceError> {
    report
        .errors
        .iter()
        .filter(|e| e.feature == feature && e.profile == profile)
        .collect()
}

/// Asserts that the report rejects with exactly errors of `feature` for
/// `profile` (and nothing else), returns them.
pub fn assert_rejects_only<'a>(
    report: &'a ClearanceReport,
    feature: Feature,
    profile: &str,
    what: &str,
) -> Vec<&'a ClearanceError> {
    assert!(
        !report.is_ok(),
        "{what}: must be rejected, got\n{}",
        describe(report)
    );
    let hits = errors_of(report, feature, profile);
    assert!(
        !hits.is_empty(),
        "{what}: no {feature:?} error for {profile}, got\n{}",
        describe(report)
    );
    assert!(
        hits.len() == report.errors.len(),
        "{what}: unexpected other errors:\n{}",
        describe(report)
    );
    hits
}

/// The parts of the contract every error must satisfy: file, key path,
/// message naming the feature and the profile, Display and the
/// `pin_assets::ValidationError` form.
pub fn check_error_shape(e: &ClearanceError, loaded: Option<&Loaded>) {
    assert!(!e.name.is_empty(), "error without a feature name: {e:?}");
    assert!(!e.path.is_empty(), "error without a key path: {e:?}");
    assert!(!e.profile.is_empty(), "error without a profile: {e:?}");
    assert!(
        e.message.contains(&e.profile),
        "message does not name the profile {:?}: {:?}",
        e.profile,
        e.message
    );
    // (A restriction finding is located by its key path; its name is that
    // path.)
    assert!(
        e.feature == Feature::Restriction || e.message.contains(&e.name),
        "message does not name the offending feature {:?}: {:?}",
        e.name,
        e.message
    );
    assert_eq!(
        e.to_string(),
        format!("{}: {}: {}", e.file, e.path, e.message)
    );
    let v = e.to_validation_error();
    assert_eq!(
        (&v.file, &v.path, &v.message),
        (&e.file, &e.path, &e.message)
    );
    if let Some(l) = loaded {
        let mesh = &l.table.manifest.meta.playfield_mesh;
        assert!(
            e.file == "table.toml" || &e.file == mesh,
            "error file {:?} is neither table.toml nor {mesh:?}",
            e.file
        );
    }
}

/// `is_sorted` by the contract's key (file, path, profile, message).
pub fn assert_sorted(list: &[ClearanceError], what: &str) {
    for w in list.windows(2) {
        let a = (&w[0].file, &w[0].path, &w[0].profile, &w[0].message);
        let b = (&w[1].file, &w[1].path, &w[1].profile, &w[1].message);
        assert!(
            a <= b,
            "{what} not sorted by (file, path, profile, message): {a:?} > {b:?}"
        );
    }
}

/// Index of the spline `id` in the loaded manifest.
pub fn spline_index(l: &Loaded, id: &str) -> usize {
    l.table
        .manifest
        .splines
        .iter()
        .position(|s| s.id == id)
        .unwrap_or_else(|| panic!("no spline {id:?} in the generated table"))
}

pub fn mech_index(l: &Loaded, id: &str) -> usize {
    l.table
        .manifest
        .mechs
        .iter()
        .position(|m| m.id == id)
        .unwrap_or_else(|| panic!("no mech {id:?}"))
}

pub fn mesh_index(l: &Loaded, name: &str) -> usize {
    l.scene
        .meshes
        .iter()
        .position(|m| m.name == name)
        .unwrap_or_else(|| {
            panic!(
                "no mesh {name:?}; meshes: {:?}",
                l.scene.meshes.iter().map(|m| &m.name).collect::<Vec<_>>()
            )
        })
}
