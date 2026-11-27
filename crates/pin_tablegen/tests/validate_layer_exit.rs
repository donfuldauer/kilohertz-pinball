//! KP-056 (issue #137), SPEC §4 Subphase 4.1 (validation) and 4.3 (layout
//! generator): `pin_tablegen validate` prints the generator's layer-exit
//! warning for a bundle with a target layer and no exit zone (parity with the
//! generator, `ramp_entry.rs::layer_exit_warning`), the four shipped tables
//! gain no new lint, and bounded random `layout.toml` bytes never panic.

mod support;

use pin_assets::{Vfs, load_gltf, load_table};
use pin_core::Material;
use pin_core::schema::layout::{Deck, Playfield, Ramp};
use pin_core::schema::table::{AabbDef, LayerTransitionDef};
use pin_core::schema::{LayoutManifest, TableManifest};
use pin_tablegen::clearance::{ClearanceReport, validate_bundle, validate_cli_output};
use pin_tablegen::layout::{GeneratedTable, generate, parse_layout, write_bundle};
use proptest::prelude::*;
use std::path::{Path, PathBuf};
use support::*;

const NO_EXIT: &str = "has no exit zone";
const CUSP: &str = "nearly vanishes";

const SPLINE_HEAD: &str = r##"
schema_version = 1
[meta]
id = "t"
name = "T"
author = "A"
version = "1"
script_entry = "rules.lua"
playfield_mesh = "old.glb"
players_max = 1
[balls]
profiles = ["standard_steel"]
[cabinet_switches]
left_flipper = 1
right_flipper = 2
left_upper = 3
right_upper = 4
action = 5
start = 6
launch = 7
tilt = 8
"##;

const PTS: [[f32; 3]; 3] = [
    [425.0, 595.0, -6.0],
    [425.0, 650.0, 0.0],
    [425.0, 705.0, 8.0],
];

/// A temporary bundle directory removed on drop.
struct Bundle(PathBuf);

impl Drop for Bundle {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Bundle {
    fn arg(&self) -> Vec<String> {
        vec![self.0.to_string_lossy().into_owned()]
    }
    fn report(&self) -> ClearanceReport {
        validate_bundle(&Vfs::open(&self.0).unwrap()).unwrap()
    }
    fn cli_warnings(&self) -> Vec<String> {
        validate_cli_output(&self.arg())
            .unwrap_or_else(|e| panic!("validate must succeed: {e:?}"))
            .1
    }
}

/// The apex base with a layer-2 wireform `ramp` and the given transitions
/// (the apex's own spline and transitions are replaced).
fn base(extra: &[LayerTransitionDef]) -> TableManifest {
    let pts: Vec<String> = PTS
        .iter()
        .map(|p| format!("[{:?}, {:?}, {:?}]", p[0], p[1], p[2]))
        .collect();
    let text = format!(
        "{SPLINE_HEAD}[[splines]]\nid = \"ramp\"\nkind = \"wireform\"\nlayer = 2\ngauge = 13.0\n\
         points = [{}]\n",
        pts.join(", ")
    );
    let mut t = apex_base();
    t.splines = TableManifest::from_toml_str(&text).unwrap().splines;
    t.layer_transitions = extra.to_vec();
    t
}

fn ramp_layout(decks: Vec<Deck>) -> LayoutManifest {
    LayoutManifest {
        schema_version: 1,
        playfield: Playfield {
            width: 2000.0,
            length: 2000.0,
            wall_height: 28.0,
        },
        walls: vec![],
        posts: vec![],
        lanes: vec![],
        ramps: vec![Ramp {
            spline: "ramp".into(),
            entry_width: 40.0,
        }],
        decks,
        inserts: vec![],
    }
}

fn transition(id: &str, from: u8, to: u8, min: [f32; 3], max: [f32; 3]) -> LayerTransitionDef {
    LayerTransitionDef {
        id: id.into(),
        from_layer: from,
        to_layer: to,
        zone: AabbDef { min, max },
    }
}

fn exit() -> LayerTransitionDef {
    transition("exit", 2, 0, [395.0, 875.0, 30.0], [455.0, 950.0, 140.0])
}

fn deck(layer: u8, z: f32, x1: f32) -> Deck {
    Deck {
        layer,
        z,
        outline: vec![[390.0, 870.0], [x1, 870.0], [x1, 955.0], [390.0, 955.0]],
        material: Material::Wood,
    }
}

/// Generates, writes into a fresh apex copy, checks it loads.
fn build(l: &LayoutManifest, b: &TableManifest, tag: &str) -> (GeneratedTable, Bundle) {
    let g = generate(l, b).unwrap_or_else(|e| panic!("{e}"));
    let dir = bundle_dir(tag);
    let bundle = Bundle(dir);
    write_bundle(&g, &bundle.0).unwrap();
    let vfs = Vfs::open(&bundle.0).unwrap();
    load_table(&vfs).unwrap_or_else(|e| panic!("bundle must load: {e}"));
    (g, bundle)
}

/// The layer-exit findings of a report as `(path, message)`.
fn exit_findings(r: &ClearanceReport) -> Vec<(String, String)> {
    r.warnings
        .iter()
        .filter(|w| w.message.contains(NO_EXIT))
        .map(|w| {
            assert_eq!(w.file, "table.toml");
            assert_eq!(w.profile, "");
            (w.path.clone(), w.message.clone())
        })
        .collect()
}

/// Asserts validate (report and CLI) agree with the generator's warnings.
fn assert_parity(g: &GeneratedTable, b: &Bundle, expect: usize, what: &str) {
    let gen_w: Vec<(String, String)> = g
        .warnings
        .iter()
        .filter(|w| w.message.contains(NO_EXIT))
        .map(|w| (w.path.clone(), w.message.clone()))
        .collect();
    assert_eq!(gen_w.len(), expect, "{what}: generator {gen_w:?}");
    let report = b.report();
    assert!(report.is_ok(), "{what}: {:?}", report.errors);
    assert_eq!(exit_findings(&report), gen_w, "{what}: report");
    let cli = b.cli_warnings();
    let cli_exit: Vec<&String> = cli.iter().filter(|s| s.contains(NO_EXIT)).collect();
    let want: Vec<String> = gen_w
        .iter()
        .map(|(p, m)| format!("table.toml: {p}: {m}"))
        .collect();
    assert_eq!(cli_exit.len(), want.len(), "{what}: cli {cli:?}");
    for w in &want {
        assert!(cli.contains(w), "{what}: {w:?} not in {cli:?}");
    }
}

/// SPEC 4.1 (kp056 acceptance): `validate` on a bundle with a target layer
/// and no exit zone prints the generator's warning, exactly once, and the
/// message names the transition, both layers and the advice.
#[test]
fn kp056_validate_prints_layer_exit_warning() {
    let (g, b) = build(&ramp_layout(vec![]), &base(&[]), "kp056_noexit");
    assert_parity(&g, &b, 1, "no exit");
    let (path, msg) = &exit_findings(&b.report())[0];
    assert_eq!(path, "layer_transitions[0]");
    assert!(
        msg.starts_with(
            "layer transition \"tablegen:ramps[0].entry\" (0 -> 2) leads to layer 2, which has no exit zone (a transition from layer 2) over unsupported area:"
        ),
        "{msg}"
    );
    assert!(msg.contains("add a 2 -> 0 zone where layer 2 has no floor"));
    let line = b
        .cli_warnings()
        .into_iter()
        .find(|s| s.contains(NO_EXIT))
        .unwrap();
    assert_eq!(line, format!("table.toml: layer_transitions[0]: {msg}"));
}

/// SPEC 4.1 / 4.3: parity with the generator across the exit/deck sweep of
/// `ramp_entry.rs::layer_exit_warning`.
#[test]
fn kp056_validate_layer_exit_parity_with_generator() {
    let hand = transition("hand_entry", 0, 2, [0.0; 3], [10.0, 10.0, 10.0]);
    let cases: Vec<(&str, Vec<LayerTransitionDef>, Vec<Deck>, usize)> = vec![
        ("no exit", vec![], vec![], 1),
        ("exit, no deck", vec![exit()], vec![], 0),
        (
            "exit under supporting deck",
            vec![exit()],
            vec![deck(2, 0.1, 460.0)],
            1,
        ),
        (
            "deck of another layer",
            vec![exit()],
            vec![deck(1, 0.1, 460.0)],
            0,
        ),
        (
            "deck above zone top",
            vec![exit()],
            vec![deck(2, 150.0, 460.0)],
            0,
        ),
        ("partial deck", vec![exit()], vec![deck(2, 0.1, 430.0)], 0),
        ("two entries, no exit", vec![hand.clone()], vec![], 2),
        ("two entries, exit", vec![hand, exit()], vec![], 0),
        (
            "loop and to-0 never warn",
            vec![
                transition("loop", 3, 3, [0.0; 3], [10.0, 10.0, 10.0]),
                transition("down", 3, 0, [20.0, 20.0, 0.0], [30.0, 30.0, 10.0]),
                exit(),
            ],
            vec![],
            0,
        ),
    ];
    for (i, (what, extra, decks, expect)) in cases.into_iter().enumerate() {
        let (g, b) = build(&ramp_layout(decks), &base(&extra), &format!("kp056_par{i}"));
        assert_parity(&g, &b, expect, what);
    }
}

/// SPEC 4.1: bundles the generator did not just produce: removing the exit
/// transition from, or adding an entry to, a written `table.toml` changes the
/// validate findings accordingly (one per transition leading to the layer).
#[test]
fn kp056_validate_hand_edited_table_toml() {
    let (_g, b) = build(&ramp_layout(vec![]), &base(&[exit()]), "kp056_edit");
    assert!(exit_findings(&b.report()).is_empty());
    assert!(b.cli_warnings().iter().all(|w| !w.contains(NO_EXIT)));
    let path = b.0.join("table.toml");
    let mut t = TableManifest::from_toml_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    t.layer_transitions.retain(|x| x.id != "exit");
    std::fs::write(&path, t.to_toml_string().unwrap()).unwrap();
    let found = exit_findings(&b.report());
    assert_eq!(found.len(), 1, "{found:?}");
    // A second, hand-declared entry: both are named, at their own index.
    t.layer_transitions
        .push(transition("hand_entry", 0, 2, [0.0; 3], [10.0, 10.0, 10.0]));
    std::fs::write(&path, t.to_toml_string().unwrap()).unwrap();
    let report = b.report();
    assert!(report.is_ok());
    let found = exit_findings(&report);
    let want: Vec<String> = t
        .layer_transitions
        .iter()
        .enumerate()
        .filter(|(_, x)| x.to_layer == 2 && x.from_layer != 2)
        .map(|(i, _)| format!("layer_transitions[{i}]"))
        .collect();
    assert_eq!(want.len(), 2);
    let mut got: Vec<String> = found.iter().map(|(p, _)| p.clone()).collect();
    got.sort();
    assert_eq!(got, want);
    assert!(found.iter().any(|(_, m)| m.contains("\"hand_entry\"")));
    // Putting an exit back silences it.
    t.layer_transitions.push(exit());
    std::fs::write(&path, t.to_toml_string().unwrap()).unwrap();
    assert!(exit_findings(&b.report()).is_empty());
}

const SHIPPED: [&str; 4] = ["greybox", "apex_circuit", "sky_deck", "feature_lab"];

/// A shipped table generated into a loadable bundle (as `clearance_index.rs`
/// does).
fn shipped(name: &str) -> Bundle {
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tables")
        .join(name);
    let layout_text = std::fs::read_to_string(src.join("layout.toml")).unwrap();
    let table_text = std::fs::read_to_string(src.join("table.toml")).unwrap();
    let lay = parse_layout(&layout_text).unwrap_or_else(|e| panic!("{name}: {e}"));
    let base = TableManifest::from_toml_str(&table_text).unwrap_or_else(|e| panic!("{name}: {e}"));
    let generated = generate(&lay, &base).unwrap_or_else(|e| panic!("{name}: {e}"));
    let dir = Bundle(bundle_dir(&format!("kp056_{name}")));
    write_bundle(&generated, &dir.0).unwrap();
    let written = std::fs::read_to_string(dir.0.join("table.toml")).unwrap();
    let mut manifest = TableManifest::from_toml_str(&written).unwrap();
    manifest.sounds.clear();
    std::fs::write(dir.0.join("table.toml"), manifest.to_toml_string().unwrap()).unwrap();
    for f in ["rules.lua", "backglass.toml", "glass.toml", "layout.toml"] {
        std::fs::copy(src.join(f), dir.0.join(f)).unwrap_or_else(|e| panic!("{name}: {f}: {e}"));
    }
    std::fs::create_dir_all(dir.0.join("display")).unwrap();
    let mut scenes: Vec<_> = std::fs::read_dir(src.join("display"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    scenes.sort();
    for s in scenes {
        std::fs::copy(&s, dir.0.join("display").join(s.file_name().unwrap())).unwrap();
    }
    dir
}

/// SPEC 4.1 (kp056 acceptance): the four shipped tables gain neither a
/// layer-exit nor a near-cusp warning from load, report or CLI.
#[test]
fn kp056_shipped_tables_have_no_new_lint_warnings() {
    for name in SHIPPED {
        let b = shipped(name);
        let vfs = Vfs::open(&b.0).unwrap();
        let loaded = load_table(&vfs).unwrap_or_else(|e| panic!("{name}: {e}"));
        load_gltf(&vfs, "playfield.glb").unwrap();
        for w in &loaded.warnings {
            assert!(
                !w.message.contains(CUSP) && !w.message.contains(NO_EXIT),
                "{name}: load {w:?}"
            );
        }
        for w in &b.report().warnings {
            assert!(
                !w.message.contains(CUSP) && !w.message.contains(NO_EXIT),
                "{name}: report {w:?}"
            );
        }
        for w in b.cli_warnings() {
            assert!(!w.contains(CUSP) && !w.contains(NO_EXIT), "{name}: cli {w}");
        }
    }
}

/// Runs parse, generate, write and validate; any `Ok`/`Err` is accepted.
fn pipeline(text: &str) {
    let Ok(layout) = parse_layout(text) else {
        return;
    };
    let Ok(g) = generate(&layout, &apex_base()) else {
        return;
    };
    let b = Bundle(bundle_dir("kp056_fuzz"));
    if write_bundle(&g, &b.0).is_ok() {
        let _ = validate_cli_output(&b.arg());
    }
}

fn mutate(text: &str, edits: &[(u8, usize, u8)]) -> String {
    let mut bytes = text.as_bytes().to_vec();
    for &(kind, pos, byte) in edits {
        if bytes.is_empty() {
            bytes.push(byte);
            continue;
        }
        let p = pos % bytes.len();
        match kind % 3 {
            0 => bytes[p] ^= byte | 1,
            1 => bytes.insert(p, byte),
            _ => {
                bytes.remove(p);
            }
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// SPEC 4.3 / R11 (kp056 acceptance): bounded random bytes never panic
    /// through parse, generate and validate.
    #[test]
    fn kp056_random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        pipeline(&String::from_utf8_lossy(&bytes));
    }

    /// SPEC 4.3 / R11 (kp056 acceptance): up to 8 byte edits of a valid
    /// layout never panic through parse, generate and validate.
    #[test]
    fn kp056_mutated_layouts_never_panic(
        which in 0usize..2,
        edits in proptest::collection::vec((any::<u8>(), any::<usize>(), any::<u8>()), 0..=8),
    ) {
        let name = ["apex_layout.toml", "rich_layout.toml"][which];
        pipeline(&mutate(&fixture(name), &edits));
    }
}
