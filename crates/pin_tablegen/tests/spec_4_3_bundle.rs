//! Subphase 4.3 spec tests (Independent Tester, black box): generated bundles
//! pass the 4.1 validation (criterion 1), are deterministic (criterion 3),
//! bind every insert to its lamp (criterion 4) and merge into the base
//! manifest without duplicating on a re-run (deliverable 1).

#[path = "fixtures/spec_4_3/support.rs"]
mod support;

use pin_core::schema::table::TableManifest;
use pin_tablegen::layout::{generate, write_bundle};
use proptest::prelude::*;
use std::collections::BTreeSet;
use support::*;

#[test]
fn appendix_a3_example_passes_bundle_validation() {
    let (layout, base) = (a3_layout(), base_table());
    let generated = gen_ok(&layout, &base);
    let loaded = check_valid_bundle(&generated, &base, "a3").unwrap_or_else(|e| panic!("{e}"));
    assert!(!loaded.scene.meshes.is_empty());
    // The ramp's spline is still declared.
    assert!(
        generated
            .table
            .splines
            .iter()
            .any(|s| s.id == "return_wireform")
    );
    // Floor and deck: one triangle mesh per layer surface.
    let meshes = world_meshes(&loaded.scene, &layout, &generated.table);
    check_surface(
        &meshes,
        0,
        0.0,
        &playfield_outline(&layout.playfield),
        "playfield floor",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let deck = &layout.decks[0];
    check_surface(
        &meshes,
        deck.layer,
        f64::from(deck.z),
        &to_f64(&deck.outline),
        "decks[0]",
    )
    .unwrap_or_else(|e| panic!("{e}"));
    // The one insert yields one lamp-bound node.
    let lamp_nodes: BTreeSet<&str> = loaded
        .scene
        .meshes
        .iter()
        .filter(|m| m.extras.lamp.is_some())
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(lamp_nodes.len(), 1, "{lamp_nodes:?}");
    assert!(
        loaded
            .scene
            .meshes
            .iter()
            .any(|m| m.extras.lamp.as_deref() == Some("overdrive_ready"))
    );
}

#[test]
fn every_element_kind_layout_passes_bundle_validation() {
    let (layout, base) = (geo_layout(), base_table());
    let generated = gen_ok(&layout, &base);
    check_valid_bundle(&generated, &base, "geo").unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn empty_layout_generates_a_floor() {
    let base = base_table();
    let layout = build_layout(450.0, 1000.0, 25.0, false, &[], &[]);
    let generated = gen_ok(&layout, &base);
    let loaded = check_valid_bundle(&generated, &base, "empty").unwrap_or_else(|e| panic!("{e}"));
    let meshes = world_meshes(&loaded.scene, &layout, &generated.table);
    check_surface(
        &meshes,
        0,
        0.0,
        &playfield_outline(&layout.playfield),
        "playfield floor",
    )
    .unwrap_or_else(|e| panic!("{e}"));
}

#[test]
fn table_toml_is_the_serialised_manifest() {
    let base = base_table();
    let generated = gen_ok(&a3_layout(), &base);
    let parsed = TableManifest::from_toml_str(&generated.table_toml).unwrap();
    assert_eq!(parsed, generated.table);
    assert_eq!(
        &generated.glb[..4],
        b"glTF",
        "playfield.glb is a binary glTF"
    );
}

#[test]
fn write_bundle_writes_exactly_the_two_outputs() {
    let generated = gen_ok(&a3_layout(), &base_table());
    let dir = temp_dir("write");
    write_bundle(&generated, &dir).unwrap();
    assert_eq!(
        std::fs::read(dir.join("playfield.glb")).unwrap(),
        generated.glb
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("table.toml")).unwrap(),
        generated.table_toml
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn base_manifest_is_preserved_by_the_merge() {
    let base = base_table();
    for layout in [a3_layout(), geo_layout()] {
        let t = gen_ok(&layout, &base).table;
        assert_eq!(t.schema_version, base.schema_version);
        assert_eq!(t.meta, base.meta);
        assert_eq!(t.balls, base.balls);
        assert_eq!(t.cabinet_switches, base.cabinet_switches);
        assert_eq!(t.switches, base.switches);
        assert_eq!(t.coils, base.coils);
        assert_eq!(t.lamps, base.lamps);
        assert_eq!(t.magnets, base.magnets);
        assert_eq!(t.flippers, base.flippers);
        assert_eq!(t.dispensers, base.dispensers);
        assert_eq!(t.mechs, base.mechs);
        assert_eq!(t.shots, base.shots);
        assert_eq!(t.modes, base.modes);
        assert_eq!(t.sounds, base.sounds);
        assert_eq!(t.scenes, base.scenes);
        assert_eq!(t.persist, base.persist);
        assert_eq!(t.dof, base.dof);
        for s in &base.splines {
            assert!(t.splines.contains(s), "base spline {:?} was lost", s.id);
        }
        for tr in &base.layer_transitions {
            assert!(
                t.layer_transitions.contains(tr),
                "base transition {:?} was lost",
                tr.id
            );
        }
        let ids: BTreeSet<&str> = t.splines.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), t.splines.len(), "duplicate spline ids");
        let ids: BTreeSet<&str> = t.layer_transitions.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids.len(), t.layer_transitions.len());
    }
}

/// Deliverable 1: a re-run on the merged manifest replaces the generated
/// entries and never duplicates them.
#[test]
fn rerun_on_generated_manifest_replaces_and_never_duplicates() {
    let base = base_table();
    for layout in [a3_layout(), geo_layout()] {
        let first = gen_ok(&layout, &base);
        let second = gen_ok(&layout, &first.table);
        assert_eq!(second.table, first.table);
        assert_eq!(second.table_toml, first.table_toml);
        assert_eq!(second.glb, first.glb);
        let third = gen_ok(&layout, &second.table);
        assert_eq!(third.table_toml, first.table_toml);
    }
    // Re-running with a different layout drops what the first run generated.
    let from_geo = gen_ok(&geo_layout(), &base);
    let empty = build_layout(500.0, 1000.0, 25.0, false, &[], &[]);
    let direct = gen_ok(&empty, &base);
    let rerun = gen_ok(&empty, &from_geo.table);
    assert_eq!(rerun.table, direct.table);
    assert_eq!(rerun.glb, direct.glb);
}

/// Criterion 3: byte-identical glb and table.toml across 10 runs.
#[test]
fn ten_runs_are_byte_identical() {
    let base = base_table();
    for layout in [a3_layout(), geo_layout()] {
        let first = gen_ok(&layout, &base);
        for run in 1..10 {
            // Fresh inputs each time: nothing may depend on addresses.
            let (l, b) = (layout.clone(), base.clone());
            let again = gen_ok(&l, &b);
            assert!(again.glb == first.glb, "glb differs on run {run}");
            assert_eq!(again.table_toml, first.table_toml, "run {run}");
            assert_eq!(again.table, first.table, "run {run}");
        }
    }
}

#[test]
fn determinism_holds_across_threads() {
    let base = base_table();
    let layout = geo_layout();
    let first = gen_ok(&layout, &base);
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let (l, b) = (layout.clone(), base.clone());
            std::thread::spawn(move || {
                let g = gen_ok(&l, &b);
                (g.glb, g.table_toml)
            })
        })
        .collect();
    for h in handles {
        let (glb, toml) = h.join().unwrap();
        assert!(glb == first.glb);
        assert_eq!(toml, first.table_toml);
    }
}

/// Criterion 4: every insert yields one lamp-bound node, bound to its lamp.
#[test]
fn every_insert_yields_one_lamp_bound_node() {
    let (layout, base) = (geo_layout(), base_table());
    let generated = gen_ok(&layout, &base);
    let loaded = check_valid_bundle(&generated, &base, "lamps").unwrap_or_else(|e| panic!("{e}"));
    let mut all_nodes = BTreeSet::new();
    for (i, insert) in layout.inserts.iter().enumerate() {
        let nodes: BTreeSet<&str> = loaded
            .scene
            .meshes
            .iter()
            .filter(|m| m.extras.lamp.as_deref() == Some(insert.lamp.as_str()))
            .map(|m| m.name.as_str())
            .collect();
        assert_eq!(
            nodes.len(),
            1,
            "inserts[{i}] ({}) must yield exactly one lamp-bound node, got {nodes:?}",
            insert.lamp
        );
        all_nodes.extend(nodes);
    }
    let lamp_bound: BTreeSet<&str> = loaded
        .scene
        .meshes
        .iter()
        .filter(|m| m.extras.lamp.is_some())
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(lamp_bound, all_nodes, "lamp-bound nodes without an insert");
    assert_eq!(lamp_bound.len(), layout.inserts.len());
}

/// A long, finely authored lane still respects the 4 096 through-point cap
/// and the lints (deliverable 3).
#[test]
fn long_lane_respects_the_point_cap() {
    let base = base_table();
    let mut layout = build_layout(520.0, 1100.0, 26.0, true, &[], &[]);
    // A gentle S along the table, one point per millimetre.
    let points: Vec<[f32; 2]> = (0..=900)
        .map(|i| {
            let y = 100.0 + i as f32;
            let x = 260.0 + 60.0 * (i as f32 / 900.0 * std::f32::consts::TAU).sin();
            [x, y]
        })
        .collect();
    layout.lanes.push(pin_core::schema::layout::Lane {
        kind: pin_core::schema::layout::LaneKind::Orbit,
        points,
        width: 40.0,
    });
    match generate(&layout, &base) {
        Ok(generated) => {
            check_valid_bundle(&generated, &base, "long").unwrap_or_else(|e| panic!("{e}"));
        }
        // Refusing is allowed, emitting an invalid bundle is not; the refusal
        // must name the lane.
        Err(e) => assert!(e.to_string().contains("lanes[0]"), "{e}"),
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, failure_persistence: None, ..ProptestConfig::default() })]

    /// Criterion 1: at least 50 random valid layouts generate bundles that
    /// pass `load_table`, with valid bindings and one node per insert.
    #[test]
    fn random_valid_layouts_pass_bundle_validation(layout in valid_layout()) {
        let base = base_table();
        let generated = match generate(&layout, &base) {
            Ok(g) => g,
            Err(e) => return Err(TestCaseError::fail(format!("valid layout refused: {e}"))),
        };
        let loaded = check_valid_bundle(&generated, &base, "random")
            .map_err(TestCaseError::fail)?;
        for insert in &layout.inserts {
            let nodes: BTreeSet<&str> = loaded
                .scene
                .meshes
                .iter()
                .filter(|m| m.extras.lamp.as_deref() == Some(insert.lamp.as_str()))
                .map(|m| m.name.as_str())
                .collect();
            prop_assert_eq!(nodes.len(), 1, "insert {} -> {:?}", &insert.lamp, nodes);
        }
        let lamp_bound: BTreeSet<&str> = loaded
            .scene
            .meshes
            .iter()
            .filter(|m| m.extras.lamp.is_some())
            .map(|m| m.name.as_str())
            .collect();
        prop_assert_eq!(lamp_bound.len(), layout.inserts.len());
        // Determinism and idempotent re-run on random layouts too.
        let again = generate(&layout, &base).ok().unwrap();
        prop_assert!(again.glb == generated.glb);
        prop_assert_eq!(&again.table_toml, &generated.table_toml);
        let rerun = generate(&layout, &generated.table).ok().unwrap();
        prop_assert_eq!(&rerun.table_toml, &generated.table_toml);
    }
}
