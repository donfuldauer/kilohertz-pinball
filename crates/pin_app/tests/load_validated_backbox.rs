//! `validate_table` / `load_validated` read a bundle's `backbox_colliders.glb`
//! (fix 2.8): a generated layer-4 backbox VUK exit validates with it and is
//! refused ("no surface of layer 4 below it") without it.
//!
//! `validate_table` is tested directly: it is `load_validated`'s first step
//! (`validate_snapshot` on the captured bundle), and the apex-based bundle
//! built here has no rules script, so the simulation itself cannot load.

use pin_core::schema::LayoutManifest;
use pin_core::schema::table::{CoilDef, CoilKind, SwitchDef, SwitchKind};
use pin_tablegen::backbox::{generate_backbox, parse_backbox_layout, write_backbox};
use std::path::{Path, PathBuf};

const BACKBOX: &str = r#"
schema_version = 1

[backbox]
origin = [0.0, 1150.0, 250.0]
width = 400.0
height = 300.0
depth = 120.0

[backbox.entry]
vuk = [300.0, 400.0]
at = -100.0
width = 100.0
coil = "bb_vuk_coil"
switch = "bb_vuk_switch"

[backbox.drain]
at = 100.0
width = 100.0
"#;

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

struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn bundle(tag: &str) -> Dir {
    let dir = std::env::temp_dir().join(format!(
        "pin_app_load_validated_backbox_{}_{tag}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let guard = Dir(dir);
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../pin_assets/tests/fixtures/apex"),
        &guard.0,
    );
    let text = std::fs::read_to_string(guard.0.join("table.toml")).unwrap();
    let mut base = pin_core::schema::TableManifest::from_toml_str(&text).unwrap();
    base.balls.profiles = vec!["mini_steel".to_owned()];
    base.balls.restrict = Vec::new();
    base.flippers.clear();
    base.splines.clear();
    base.layer_transitions.clear();
    base.mechs.clear();
    base.coils.push(CoilDef {
        id: "bb_vuk_coil".into(),
        number: 90,
        kind: CoilKind::Vuk,
    });
    base.switches.push(SwitchDef {
        id: "bb_vuk_switch".into(),
        number: 180,
        kind: SwitchKind::Opto,
        layer: 0,
        position: None,
        radius: None,
    });
    let layout = LayoutManifest::from_toml_str(
        "schema_version = 1\n[playfield]\nwidth = 514.0\nlength = 1070.0\nwall_height = 28.0\n",
    )
    .unwrap();
    let main = pin_tablegen::generate(&layout, &base).unwrap_or_else(|e| panic!("{e}"));
    pin_tablegen::write_bundle(&main, &guard.0).unwrap();
    let bb = parse_backbox_layout(BACKBOX).unwrap_or_else(|e| panic!("{e}"));
    let g = generate_backbox(&bb.backbox, &main.table).unwrap_or_else(|e| panic!("{e}"));
    write_backbox(&g, &guard.0).unwrap();
    guard
}

#[test]
fn generated_backbox_vuk_exit_loads_with_colliders_and_is_refused_without() {
    let dir = bundle("with");
    assert!(dir.0.join("backbox_colliders.glb").exists());
    let ok = pin_app::orchestrator::validate_table(&dir.0).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        ok.table
            .manifest
            .mechs
            .iter()
            .any(|m| m.exit_layer == Some(4))
    );

    let without = bundle("without");
    std::fs::remove_file(without.0.join("backbox_colliders.glb")).unwrap();
    let err = match pin_app::orchestrator::validate_table(&without.0) {
        Ok(_) => panic!("a layer-4 VUK exit without backbox colliders must be refused"),
        Err(e) => e,
    };
    assert!(err.contains("no surface of layer 4"), "{err}");
}
