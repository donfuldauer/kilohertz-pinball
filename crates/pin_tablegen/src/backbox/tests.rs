//! Unit tests of the backbox generator (frames, validation, determinism,
//! parser robustness).

use super::*;
use pin_core::schema::table::{CoilDef, CoilKind, SwitchDef, SwitchKind};
use proptest::prelude::*;

/// The example of the module docs.
pub(super) const EXAMPLE: &str = r#"
schema_version = 1

[backbox]
origin = [0.0, 1150.0, 250.0]
width = 400.0
height = 300.0
depth = 35.0

[[backbox.flippers]]
side = "left"
position = [-60.0, 40.0]
coil = "bb_left_coil"
eos_switch = "bb_left_eos"
staging = { type = "ParallelLinked", input = "left_flipper" }

[[backbox.flippers]]
side = "right"
position = [60.0, 40.0]
coil = "bb_right_coil"
eos_switch = "bb_right_eos"
staging = { type = "DualStageThreshold", axis = "right_flipper", lower_threshold = 0.3, upper_threshold = 0.85 }

[backbox.entry]
vuk = [120.0, 1000.0]
at = -120.0
coil = "bb_vuk_coil"
switch = "bb_vuk_switch"

[backbox.drain]
at = 150.0
"#;

/// A minimal base table with the names the example uses.
pub(super) fn base() -> TableManifest {
    let mut t = TableManifest::from_toml_str(include_str!("../../../pin_core/fixtures/table.toml"))
        .expect("pin_core fixture");
    for (i, (id, kind)) in [
        ("bb_left_coil", CoilKind::Flipper),
        ("bb_right_coil", CoilKind::Flipper),
        ("bb_vuk_coil", CoilKind::Vuk),
    ]
    .into_iter()
    .enumerate()
    {
        t.coils.push(CoilDef {
            id: id.into(),
            number: 60 + i as u16,
            kind,
        });
    }
    for (i, id) in ["bb_left_eos", "bb_right_eos", "bb_vuk_switch"]
        .into_iter()
        .enumerate()
    {
        t.switches.push(SwitchDef {
            id: id.into(),
            number: 200 + i as u16,
            kind: SwitchKind::Opto,
            layer: 0,
            position: None,
            radius: None,
        });
    }
    t
}

fn example() -> BackboxLayout {
    parse_backbox_layout(EXAMPLE)
        .expect("example parses")
        .backbox
}

fn err_of(layout: &BackboxLayout) -> GenError {
    generate_backbox(layout, &base()).expect_err("invalid layout")
}

#[test]
fn frame_axes_follow_the_pitch() {
    let f = BackboxFrame::new([0.0, 1000.0, 200.0], 6.5);
    let p = 6.5f32.to_radians();
    assert!((f.normal() - Vec3::new(0.0, -p.cos(), p.sin())).length() < 1e-6);
    assert!((f.up() - Vec3::new(0.0, p.sin(), p.cos())).length() < 1e-6);
    assert!((f.to_table(Vec3::ZERO) - Vec3::new(0.0, 1.0, 0.2)).length() < 1e-6);
    // World-up along the plane: gravity in the table frame is
    // (0, −g sin p, −g cos p) = −g · up, all of it along the plane.
    let g = Vec3::new(0.0, -9.81 * p.sin(), -9.81 * p.cos());
    assert!(g.dot(f.normal()).abs() < 1e-5);
    assert!((g.dot(f.up()) + 9.81).abs() < 1e-5);
}

#[test]
fn example_generates_every_element() {
    let g = generate_backbox(&example(), &base()).expect("generates");
    let names: Vec<&str> = g.colliders.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "backbox.back",
            "backbox.glass",
            "backbox.walls[0]",
            "backbox.walls[1]",
            "backbox.roof",
            "backbox.shelf[0]",
            "backbox.shelf[1]",
            "backbox.shelf[2]",
        ]
    );
    assert!(g.colliders.iter().all(|c| c.layer_mask == 1 << 4));
    assert_eq!(g.flippers.len(), 2);
    for f in &g.flippers {
        assert_eq!(f.collision_layer, 4);
        let n = Vec3::from(f.surface_normal);
        assert!((n - g.frame.normal()).length() < 1e-6);
        assert!(n.length() > 0.999 && n.length() < 1.001);
    }
    assert_eq!(g.flippers[0].rest_angle_deg, -30.0);
    assert_eq!(g.flippers[1].sweep_angle_deg, -50.0);
    assert_eq!(g.mechs.len(), 1);
    assert_eq!(g.mechs[0].exit_layer, Some(4));
    assert_eq!(g.mechs[0].layer, Some(0));
    let t = &g.layer_transitions;
    assert_eq!((t[0].from_layer, t[0].to_layer), (0, 4));
    assert_eq!((t[1].from_layer, t[1].to_layer), (4, 0));
    for z in t {
        assert!((0..3).all(|k| z.zone.min[k] < z.zone.max[k]));
    }
    // Merged and round-tripped.
    assert!(g.table_toml.starts_with(crate::layout::GENERATED_HEADER));
    assert_eq!(
        g.table
            .flippers
            .iter()
            .filter(|f| f.id.starts_with(BACKBOX_PREFIX))
            .count(),
        2
    );
}

#[test]
fn rerun_replaces_generated_entries() {
    let g = generate_backbox(&example(), &base()).unwrap();
    let again = generate_backbox(&example(), &g.table).unwrap();
    assert_eq!(again.table, g.table);
    assert_eq!(again.table_toml, g.table_toml);
}

#[test]
fn output_is_byte_identical_across_runs() {
    let a = generate_backbox(&example(), &base()).unwrap();
    let b = generate_backbox(&example(), &base()).unwrap();
    assert_eq!(a.scene_glb, b.scene_glb);
    assert_eq!(a.colliders_glb, b.colliders_glb);
    assert_eq!(a.table_toml, b.table_toml);
}

#[test]
fn the_ball_fits_the_slot_and_the_holes() {
    let g = generate_backbox(&example(), &base()).unwrap();
    // The VUK exit point is inside the slot: between glass and back with
    // room for the ball.
    let m = &g.mechs[0];
    let exit = (Vec3::from(m.position.unwrap()) + Vec3::from(m.exit_offset.unwrap())) * 1e-3;
    let local = g.frame.rotation.inverse() * (exit - g.frame.origin_m);
    assert!(
        local.y > 0.0135 && local.y < 0.035 - 0.0135 + 1e-6,
        "{local:?}"
    );
    assert!(local.z > 0.0135, "above the shelf: {local:?}");
}

#[test]
fn errors_name_the_element() {
    type Edit = Box<dyn Fn(&mut BackboxLayout)>;
    let cases: Vec<(Edit, &str)> = vec![
        (Box::new(|l| l.origin[1] = f32::NAN), "backbox.origin"),
        (Box::new(|l| l.width = 20.0), "backbox"),
        (Box::new(|l| l.depth = 20.0), "backbox"),
        (Box::new(|l| l.height = f32::INFINITY), "backbox"),
        (
            Box::new(|l| l.collision_layer = 0),
            "backbox.collision_layer",
        ),
        (
            Box::new(|l| l.collision_layer = 1),
            "backbox.collision_layer",
        ),
        (
            Box::new(|l| l.collision_layer = 2),
            "backbox.collision_layer",
        ),
        (
            Box::new(|l| l.collision_layer = 3),
            "backbox.collision_layer",
        ),
        (
            Box::new(|l| l.collision_layer = 9),
            "backbox.collision_layer",
        ),
        (Box::new(|l| l.entry.at = 190.0), "backbox.entry"),
        (Box::new(|l| l.entry.width = 10.0), "backbox.entry"),
        (Box::new(|l| l.entry.vuk[0] = 1.0e9), "backbox.entry"),
        (Box::new(|l| l.entry.coil = "nope".into()), "backbox.entry"),
        (
            Box::new(|l| l.entry.switch = "nope".into()),
            "backbox.entry",
        ),
        (Box::new(|l| l.drain.at = -110.0), "backbox.drain"),
        (Box::new(|l| l.drain.width = 1.0e6), "backbox.drain"),
        (
            Box::new(|l| l.flippers[1].length = 5.0),
            "backbox.flippers[1]",
        ),
        (
            Box::new(|l| l.flippers[0].position = [-195.0, 40.0]),
            "backbox.flippers[0]",
        ),
        (
            Box::new(|l| l.flippers[0].position = [0.0, 5.0]),
            "backbox.flippers[0]",
        ),
        (
            Box::new(|l| l.flippers[1].coil = "x".into()),
            "backbox.flippers[1]",
        ),
        (
            Box::new(|l| l.flippers[1].eos_switch = "x".into()),
            "backbox.flippers[1]",
        ),
        (
            Box::new(|l| {
                l.flippers[0].staging = FlipperStagingDef::AutoSensorThingFlip {
                    trigger_switch: "x".into(),
                    delay_ms: 10.0,
                    hold_duration_ms: 50.0,
                }
            }),
            "backbox.flippers[0]",
        ),
        (
            Box::new(|l| {
                let f = l.flippers[0].clone();
                l.flippers = vec![f; MAX_BACKBOX_FLIPPERS + 1];
            }),
            "backbox.flippers",
        ),
    ];
    for (i, (edit, element)) in cases.iter().enumerate() {
        let mut l = example();
        edit(&mut l);
        let e = err_of(&l);
        assert_eq!(e.element, *element, "case {i}: {e}");
        assert!(!e.message.is_empty());
    }
    // A bad pitch in the base table names it.
    let mut b = base();
    b.meta.pitch_deg = f32::NAN;
    let e = generate_backbox(&example(), &b).unwrap_err();
    assert_eq!(e.element, "table.toml meta.pitch_deg");
}

#[test]
fn thing_flip_staging_with_a_known_switch_is_accepted() {
    let mut l = example();
    l.flippers[0].staging = FlipperStagingDef::AutoSensorThingFlip {
        trigger_switch: "bb_vuk_switch".into(),
        delay_ms: 10.0,
        hold_duration_ms: 50.0,
    };
    assert!(generate_backbox(&l, &base()).is_ok());
}

#[test]
fn parse_errors_name_the_file() {
    for text in [
        "schema_version = 2\n[backbox]\n",
        "schema_version = 1\n",
        "schema_version = 1\n[backbox]\nbogus = 1\n",
        "not toml ===",
    ] {
        let e = parse_backbox_layout(text).unwrap_err();
        assert_eq!(e.element, BACKBOX_LAYOUT_FILE, "{text}");
    }
    let v2 = EXAMPLE.replace("schema_version = 1", "schema_version = 2");
    assert!(
        parse_backbox_layout(&v2)
            .unwrap_err()
            .message
            .contains("schema_version")
    );
    let big = "#".repeat(MAX_LAYOUT_BYTES + 1);
    assert!(parse_backbox_layout(&big).is_err());
    assert!(parse_backbox_layout_bytes(&[0xff, 0xfe]).is_err());
    assert!(parse_backbox_layout_bytes(EXAMPLE.as_bytes()).is_ok());
}

#[test]
fn write_backbox_writes_the_three_files() {
    let g = generate_backbox(&example(), &base()).unwrap();
    let dir =
        std::env::temp_dir().join(format!("pin_tablegen_backbox_unit_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    write_backbox(&g, &dir).unwrap();
    assert_eq!(
        std::fs::read(dir.join(BACKBOX_SCENE_FILE)).unwrap(),
        g.scene_glb
    );
    assert_eq!(
        std::fs::read(dir.join(BACKBOX_COLLIDERS_FILE)).unwrap(),
        g.colliders_glb
    );
    assert_eq!(
        std::fs::read_to_string(dir.join(crate::layout::TABLE_FILE)).unwrap(),
        g.table_toml
    );
    // A file where the folder should be is an error, not a panic.
    let blocked = dir.join("blocked");
    std::fs::write(&blocked, b"x").unwrap();
    assert!(write_backbox(&g, &blocked).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bat_mesh_faces_the_glass() {
    let shape = BatShape {
        pivot: [0.0, 0.05],
        angle: -0.5,
        length: 0.05,
        rb: 0.007,
        rt: 0.004,
    };
    let m = bat_mesh(&shape, 0.035, 0.02);
    assert_eq!(m.positions.len(), m.normals.len());
    assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
    // Every triangle's winding agrees with its stored normal.
    for t in m.indices.chunks(3) {
        let p = |k: usize| Vec3::from(m.positions[t[k] as usize]);
        let n = (p(1) - p(0)).cross(p(2) - p(0));
        if n.length() < 1e-12 {
            continue;
        }
        let stored = Vec3::from(m.normals[t[0] as usize]);
        assert!(n.normalize().dot(stored) > 0.5, "{t:?}");
    }
    let front = m
        .positions
        .iter()
        .map(|p| p[1])
        .fold(f32::INFINITY, f32::min);
    assert!((front - 0.015).abs() < 1e-6);
}

#[test]
fn box_mesh_faces_point_outward() {
    let m = box_mesh(Vec3::ZERO, Vec3::new(1.0, 2.0, 3.0));
    let c = Vec3::new(0.5, 1.0, 1.5);
    for t in m.indices.chunks(3) {
        let p = |k: usize| Vec3::from(m.positions[t[k] as usize]);
        let n = (p(1) - p(0)).cross(p(2) - p(0));
        assert!(n.dot(p(0) - c) > 0.0);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Bounded random bytes never panic the parser.
    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = parse_backbox_layout_bytes(&bytes);
    }

    /// Mutated example text never panics the parser or the generator.
    #[test]
    fn mutated_layouts_never_panic(pos in 0usize..EXAMPLE.len(), byte in any::<u8>()) {
        let mut bytes = EXAMPLE.as_bytes().to_vec();
        bytes[pos] = byte;
        if let Ok(file) = parse_backbox_layout_bytes(&bytes) {
            let _ = generate_backbox(&file.backbox, &base());
        }
    }

    /// Any valid box: every generated number is finite, the colliders are
    /// on the backbox layer only, and the frame is orthonormal.
    #[test]
    fn valid_boxes_generate_finite_geometry(
        width in 400.0f32..1000.0,
        height in 150.0f32..1000.0,
        depth in 30.0f32..200.0,
        pitch in 0.0f32..15.0,
        layer in 4u8..8,
    ) {
        let mut l = example();
        l.width = width;
        l.height = height;
        l.depth = depth;
        l.collision_layer = layer;
        let mut b = base();
        b.meta.pitch_deg = pitch;
        let g = generate_backbox(&l, &b).expect("valid");
        for c in &g.colliders {
            prop_assert_eq!(c.layer_mask, 1u8 << layer);
            if let StaticShape::Cuboid { center, half_extents, rotation } = &c.shape {
                prop_assert!(center.is_finite() && half_extents.is_finite());
                prop_assert!(half_extents.min_element() > 0.0);
                prop_assert!((rotation.length() - 1.0).abs() < 1e-5);
            }
        }
        prop_assert!(g.frame.normal().dot(g.frame.up()).abs() < 1e-5);
        for f in &g.flippers {
            prop_assert!(f.base_position.iter().all(|v| v.is_finite()));
        }
    }
}

mod outline_geometry {
    //! The planar helpers behind the optional `outline` (5.5-FIX1b).

    use super::super::outline::{Outline, check_outline, segments_touch};
    use proptest::prelude::*;

    fn rect() -> Outline {
        check_outline(&[[-200.0, 0.0], [200.0, 0.0], [200.0, 300.0], [-200.0, 300.0]]).unwrap()
    }

    #[test]
    fn winding_and_bounds() {
        let o = rect();
        assert!(o.ccw);
        assert_eq!((o.lo, o.hi), ([-200.0, 0.0], [200.0, 300.0]));
        let cw =
            check_outline(&[[-200.0, 0.0], [-200.0, 300.0], [200.0, 300.0], [200.0, 0.0]]).unwrap();
        assert!(!cw.ccw);
        assert!(o.check_fits(400.0, 300.0).is_ok());
        assert!(o.check_fits(400.005, 299.995).is_ok());
        assert_eq!(
            o.check_fits(f32::NAN, 300.0).unwrap_err().element,
            "backbox"
        );
    }

    #[test]
    fn holes_cut_only_downward_edges() {
        let o = rect();
        let holes = [(-140.0, -100.0), (130.0, 170.0)];
        // Bottom edge: three pieces, in order, end points exact.
        let p = o.edge_pieces(0, &holes);
        assert_eq!(
            p,
            vec![
                ([-200.0, 0.0], [-140.0, 0.0]),
                ([-100.0, 0.0], [130.0, 0.0]),
                ([170.0, 0.0], [200.0, 0.0]),
            ]
        );
        // The top edge faces up: whole.
        assert_eq!(
            o.edge_pieces(2, &holes),
            vec![([200.0, 300.0], [-200.0, 300.0])]
        );
        // A hole covering the whole edge leaves nothing; one off the edge
        // leaves it whole; one over the edge's end trims one side.
        assert!(o.edge_pieces(0, &[(-300.0, 300.0)]).is_empty());
        assert_eq!(o.edge_pieces(0, &[(250.0, 260.0)]).len(), 1);
        assert_eq!(
            o.edge_pieces(0, &[(150.0, 260.0)]),
            vec![([-200.0, 0.0], [150.0, 0.0])]
        );
    }

    #[test]
    fn floors_follow_the_bottom() {
        // A V-shaped bottom: lowest at u = 0.
        let o = check_outline(&[
            [-200.0, 100.0],
            [0.0, 0.0],
            [200.0, 100.0],
            [200.0, 300.0],
            [-200.0, 300.0],
        ])
        .unwrap();
        assert!(o.floor_at(0.0).abs() < 1e-6);
        assert!((o.floor_at(100.0) - 50.0).abs() < 1e-4);
        assert!((o.lowest_floor((50.0, 150.0)) - 25.0).abs() < 1e-4);
        assert!(o.lowest_floor((-20.0, 20.0)).abs() < 1e-6);
        // Off the outline: the bounding-box bottom.
        assert_eq!(o.floor_at(500.0), 0.0);
        assert_eq!(o.lowest_floor((300.0, 400.0)), 0.0);
    }

    #[test]
    fn discs_inside_the_outline() {
        let o = rect();
        assert!(o.contains_disc([0.0, 150.0], 100.0));
        assert!(!o.contains_disc([0.0, 150.0], 160.0));
        assert!(!o.contains_disc([195.0, 150.0], 10.0));
        assert!(!o.contains_disc([300.0, 150.0], 1.0));
    }

    /// The concave outline of CodeRabbit finding D: a notch u -40..-20,
    /// v 80..300 cut down from the top.
    fn notch() -> Outline {
        check_outline(&[
            [-200.0, 0.0],
            [200.0, 0.0],
            [200.0, 300.0],
            [-20.0, 300.0],
            [-20.0, 80.0],
            [-40.0, 80.0],
            [-40.0, 300.0],
            [-200.0, 300.0],
        ])
        .unwrap()
    }

    #[test]
    fn tapered_capsule_inside_and_crossing_an_edge() {
        let o = rect();
        assert!(o.contains_tapered_capsule([0.0, 150.0], 20.0, [100.0, 150.0], 10.0));
        assert_eq!(
            o.tapered_capsule_breach([0.0, 150.0], 20.0, [100.0, 150.0], 10.0),
            None
        );
        // The tip disc crosses the right edge (u = 200).
        assert!(!o.contains_tapered_capsule([0.0, 150.0], 20.0, [195.0, 150.0], 10.0));
        // An end centre outside: that centre is the breach.
        assert_eq!(
            o.tapered_capsule_breach([150.0, 150.0], 5.0, [250.0, 150.0], 5.0),
            Some([250.0, 150.0])
        );
        assert_eq!(
            o.tapered_capsule_breach([-250.0, 150.0], 5.0, [0.0, 150.0], 5.0),
            Some([-250.0, 150.0])
        );
        // The axis crosses an edge and comes back (a concave outline):
        // both centres inside.
        let n = notch();
        assert!(n.contains_disc([-70.0, 100.0], 14.5));
        assert!(n.contains_disc([30.0, 100.0], 8.5));
        let q = n
            .tapered_capsule_breach([-70.0, 100.0], 14.5, [30.0, 100.0], 8.5)
            .expect("the shaft crosses the notch");
        assert!((-45.0..=-15.0).contains(&q[0]) && q[1] == 100.0, "{q:?}");
        assert!(!n.contains_tapered_capsule([-70.0, 100.0], 14.5, [30.0, 100.0], 8.5));
    }

    #[test]
    fn tapered_capsule_touching_is_inside() {
        let o = rect();
        // 10 mm from the bottom edge (v = 0) with radius 10: tangent.
        assert!(o.contains_tapered_capsule([-100.0, 10.0], 10.0, [100.0, 10.0], 10.0));
        assert!(!o.contains_tapered_capsule([-100.0, 10.0], 10.01, [100.0, 10.0], 10.0));
        assert!(!o.contains_tapered_capsule([-100.0, 10.0], 10.0, [100.0, 10.0], 10.01));
        // Tangent to the notch bottom (v = 80) from below.
        let n = notch();
        assert!(n.contains_tapered_capsule([-100.0, 70.0], 10.0, [100.0, 70.0], 10.0));
    }

    #[test]
    fn tapered_capsule_hull_crossing_a_notch() {
        // The axis (v = 70) stays 10 mm below the notch; the hull reaches
        // it when the radius there exceeds 10 mm. Both end discs are well
        // inside.
        let n = notch();
        for (a, b) in [([-70.0, 70.0], [30.0, 70.0]), ([30.0, 70.0], [-70.0, 70.0])] {
            assert!(n.contains_disc(a, 12.0) && n.contains_disc(b, 12.0));
            assert!(!n.contains_tapered_capsule(a, 12.0, b, 12.0));
            assert!(n.contains_tapered_capsule(a, 9.0, b, 9.0));
            let q = n.tapered_capsule_breach(a, 12.0, b, 12.0).unwrap();
            assert!((-40.0..=-20.0).contains(&q[0]) && q[1] == 70.0, "{q:?}");
        }
    }

    #[test]
    fn tapered_capsule_zero_length_axis_is_the_larger_disc() {
        let o = rect();
        for (ra, rb) in [(10.0, 140.0), (140.0, 10.0)] {
            assert!(o.contains_tapered_capsule([0.0, 150.0], ra, [0.0, 150.0], rb));
        }
        for (ra, rb) in [(10.0, 160.0), (160.0, 10.0)] {
            assert!(!o.contains_tapered_capsule([0.0, 150.0], ra, [0.0, 150.0], rb));
            assert_eq!(
                o.contains_tapered_capsule([0.0, 150.0], ra, [0.0, 150.0], rb),
                o.contains_disc([0.0, 150.0], 160.0)
            );
        }
        // Zero radius, zero length: the point test.
        assert!(o.contains_tapered_capsule([0.0, 0.5], 0.0, [0.0, 0.5], 0.0));
        assert!(!o.contains_tapered_capsule([0.0, -0.5], 0.0, [0.0, -0.5], 0.0));
    }

    #[test]
    fn tapered_capsule_radii_differ() {
        let n = notch();
        // Thin near the notch (u -40..-20 at t ≈ 0.33..0.4): the radius
        // there is below the 10 mm gap.
        assert!(n.contains_tapered_capsule([-140.0, 70.0], 2.0, [160.0, 70.0], 20.0));
        // Thick near the notch: 12.8 mm at u = -20.
        assert!(!n.contains_tapered_capsule([-140.0, 70.0], 20.0, [160.0, 70.0], 2.0));
        // One end disc contains the other (|rb - ra| >= length).
        let o = rect();
        assert!(o.contains_tapered_capsule([0.0, 150.0], 100.0, [10.0, 150.0], 5.0));
        assert!(!o.contains_tapered_capsule([0.0, 150.0], 155.0, [10.0, 150.0], 5.0));
        // Left edge 50 mm away from the thick end.
        assert!(o.contains_tapered_capsule([-150.0, 150.0], 40.0, [150.0, 150.0], 5.0));
        assert!(!o.contains_tapered_capsule([-150.0, 150.0], 55.0, [150.0, 150.0], 5.0));
    }

    #[test]
    fn touching_cases() {
        let p = |x: f64, y: f64| [x, y];
        // Proper crossing, T touch, end-to-end touch, collinear overlap.
        assert!(segments_touch(
            p(0.0, 0.0),
            p(2.0, 2.0),
            p(0.0, 2.0),
            p(2.0, 0.0)
        ));
        assert!(segments_touch(
            p(0.0, 0.0),
            p(2.0, 0.0),
            p(1.0, 0.0),
            p(1.0, 5.0)
        ));
        assert!(segments_touch(
            p(0.0, 0.0),
            p(2.0, 0.0),
            p(2.0, 0.0),
            p(3.0, 1.0)
        ));
        assert!(segments_touch(
            p(0.0, 0.0),
            p(2.0, 0.0),
            p(1.0, 0.0),
            p(3.0, 0.0)
        ));
        // Collinear disjoint, parallel, far apart, straddling one line only.
        assert!(!segments_touch(
            p(0.0, 0.0),
            p(1.0, 0.0),
            p(1.5, 0.0),
            p(3.0, 0.0)
        ));
        assert!(!segments_touch(
            p(0.0, 0.0),
            p(2.0, 0.0),
            p(0.0, 1.0),
            p(2.0, 1.0)
        ));
        assert!(!segments_touch(
            p(0.0, 0.0),
            p(1.0, 1.0),
            p(5.0, 5.0),
            p(6.0, 7.0)
        ));
        assert!(!segments_touch(
            p(0.0, 0.0),
            p(2.0, 2.0),
            p(1.0, 0.0),
            p(3.0, 1.0)
        ));
    }

    /// Oracle for the tapered capsule: the smallest `dist(disc centre,
    /// edge) - radius` over discs along the axis (2001 samples per edge,
    /// then a ternary search around the best sample: the clearance is
    /// convex along the axis), mm.
    fn oracle_clearance(o: &Outline, a: [f32; 2], ra: f32, b: [f32; 2], rb: f32) -> f64 {
        let f = |p: [f32; 2]| [f64::from(p[0]), f64::from(p[1])];
        let (a, b, ra, rb) = (f(a), f(b), f64::from(ra), f64::from(rb));
        let mut best = f64::INFINITY;
        for i in 0..o.len() {
            let (p, q) = o.edge(i);
            let (p, q) = (f(p), f(q));
            let g = |t: f64| {
                let x = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
                let (ex, ey) = (q[0] - p[0], q[1] - p[1]);
                let s = (((x[0] - p[0]) * ex + (x[1] - p[1]) * ey) / (ex * ex + ey * ey))
                    .clamp(0.0, 1.0);
                let d = ((x[0] - p[0] - s * ex).powi(2) + (x[1] - p[1] - s * ey).powi(2)).sqrt();
                d - ((1.0 - t) * ra + t * rb)
            };
            let n: u32 = 2000;
            let k = (0..=n)
                .min_by(|&i, &j| {
                    g(f64::from(i) / f64::from(n)).total_cmp(&g(f64::from(j) / f64::from(n)))
                })
                .unwrap();
            let (mut lo, mut hi) = (
                f64::from(k.saturating_sub(1)) / f64::from(n),
                f64::from((k + 1).min(n)) / f64::from(n),
            );
            for _ in 0..100 {
                let m1 = lo + (hi - lo) / 3.0;
                let m2 = hi - (hi - lo) / 3.0;
                if g(m1) < g(m2) {
                    hi = m2;
                } else {
                    lo = m1;
                }
            }
            best = best
                .min(g(0.5 * (lo + hi)))
                .min(g(f64::from(k) / f64::from(n)));
        }
        best
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

        /// The touch test is symmetric in the two segments and in each
        /// segment's direction.
        #[test]
        fn touch_is_symmetric(v in proptest::array::uniform8(-20i32..20)) {
            let q = |i: usize| [f64::from(v[i]), f64::from(v[i + 1])];
            let (a, b, c, d) = (q(0), q(2), q(4), q(6));
            let t = segments_touch(a, b, c, d);
            prop_assert_eq!(t, segments_touch(c, d, a, b));
            prop_assert_eq!(t, segments_touch(b, a, c, d));
            prop_assert_eq!(t, segments_touch(a, b, d, c));
        }

        /// `contains_tapered_capsule` agrees with an independent oracle (the
        /// minimum over every edge of the distance from the swept discs
        /// to the edge minus their radius, by dense sampling of the axis
        /// refined by a ternary search on the convex clearance) whenever
        /// the oracle is more than 1e-3 mm from the boundary, and never
        /// panics on any outline `check_outline` accepts.
        #[test]
        fn tapered_capsule_matches_the_disc_oracle(
            pts in proptest::collection::vec((-200.0f32..200.0, -200.0f32..200.0), 3..9),
            a in (-220.0f32..220.0, -220.0f32..220.0),
            b in (-220.0f32..220.0, -220.0f32..220.0),
            ra in 0.0f32..40.0,
            rb in 0.0f32..40.0,
        ) {
            let pts: Vec<[f32; 2]> = pts.iter().map(|&(x, y)| [x, y]).collect();
            let Ok(o) = check_outline(&pts) else { return Ok(()); };
            let (a, b) = ([a.0, a.1], [b.0, b.1]);
            let got = o.contains_tapered_capsule(a, ra, b, rb);
            prop_assert_eq!(got, o.tapered_capsule_breach(a, ra, b, rb).is_none());
            let centres_in = o.contains_disc(a, 0.0) && o.contains_disc(b, 0.0);
            if !centres_in {
                prop_assert!(!got);
                return Ok(());
            }
            let m = oracle_clearance(&o, a, ra, b, rb);
            if m > 1e-3 {
                prop_assert!(got, "clearance {} but rejected", m);
            } else if m < -1e-3 {
                prop_assert!(!got, "clearance {} but accepted", m);
            }
        }

        /// The same agreement on the fixed concave notch outline, with the
        /// capsule mostly inside it (both outcomes common).
        #[test]
        fn tapered_capsule_matches_the_oracle_in_a_notch(
            a in (-200.0f32..200.0, 0.0f32..300.0),
            b in (-200.0f32..200.0, 0.0f32..300.0),
            ra in 0.0f32..30.0,
            rb in 0.0f32..30.0,
        ) {
            let o = notch();
            let (a, b) = ([a.0, a.1], [b.0, b.1]);
            let got = o.contains_tapered_capsule(a, ra, b, rb);
            if !(o.contains_disc(a, 0.0) && o.contains_disc(b, 0.0)) {
                prop_assert!(!got);
                return Ok(());
            }
            let m = oracle_clearance(&o, a, ra, b, rb);
            if m > 1e-3 {
                prop_assert!(got, "clearance {} but rejected", m);
            } else if m < -1e-3 {
                prop_assert!(!got, "clearance {} but accepted", m);
            }
        }

        /// Cutting holes out of an edge keeps exactly the edge length outside
        /// the holes, in order, inside the edge.
        #[test]
        fn cut_pieces_conserve_length(
            x0 in -300.0f32..-50.0,
            x1 in 50.0f32..300.0,
            h0 in -320.0f32..320.0,
            hw0 in 1.0f32..100.0,
            h1 in -320.0f32..320.0,
            hw1 in 1.0f32..100.0,
        ) {
            let o = check_outline(&[[x0, 0.0], [x1, 0.0], [x1, 200.0], [x0, 200.0]]).unwrap();
            let holes = [(h0, h0 + hw0), (h1, h1 + hw1)];
            let pieces = o.edge_pieces(0, &holes);
            let mut covered = 0.0f64;
            let mut last = x0;
            for (a, b) in &pieces {
                prop_assert!(a[0] >= last && b[0] > a[0] && b[0] <= x1);
                prop_assert!(a[1] == 0.0 && b[1] == 0.0);
                for (ha, hb) in holes {
                    prop_assert!(b[0] <= ha + 1e-3 || a[0] >= hb - 1e-3);
                }
                covered += f64::from(b[0] - a[0]);
                last = b[0];
            }
            // Length outside the holes, sampled.
            let n = 4000;
            let step = f64::from(x1 - x0) / f64::from(n);
            let outside = (0..n)
                .filter(|k| {
                    let u = f64::from(x0) + (f64::from(*k) + 0.5) * step;
                    !holes.iter().any(|(a, b)| u > f64::from(*a) && u < f64::from(*b))
                })
                .count() as f64
                * step;
            prop_assert!((covered - outside).abs() < 2.0 * step + 1e-3, "{} vs {}", covered, outside);
        }
    }
}
