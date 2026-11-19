//! Unit tests of the clearance validator's helpers (the table-level
//! behaviour is tested in `tests/clearance_*.rs`).

use super::geom::Polygon;
use super::*;

#[test]
fn feature_labels_are_distinct() {
    let all = [
        Feature::Lane,
        Feature::FlipperGap,
        Feature::Wireform,
        Feature::Scoop,
        Feature::VukExit,
        Feature::DeckHeadroom,
        Feature::Restriction,
        Feature::BackboxCollider,
    ];
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            assert_ne!(a.label(), b.label());
        }
    }
}

#[test]
fn allowed_regions() {
    let square = Polygon {
        points: vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 0.0),
            DVec2::new(1.0, 1.0),
            DVec2::new(0.0, 1.0),
        ],
    };
    let a = Allowed::Only(vec![(vec![0], Some(square)), (vec![2], None)]);
    assert!(a.restricted() && !Allowed::Anywhere.restricted());
    assert!(a.layer(0) && a.layer(2) && !a.layer(1));
    assert!(a.at(0, DVec2::new(0.5, 0.5)) && !a.at(0, DVec2::new(1.5, 0.5)));
    assert!(a.at(2, DVec2::new(9.0, 9.0)) && !a.at(1, DVec2::new(0.5, 0.5)));
    assert!(a.in_box(0, DVec2::new(0.9, 0.9), DVec2::new(2.0, 2.0)));
    assert!(!a.in_box(0, DVec2::new(1.5, 1.5), DVec2::new(2.0, 2.0)));
    assert!(
        Allowed::Anywhere.at(5, DVec2::ZERO)
            && Allowed::Anywhere.in_box(5, DVec2::ZERO, DVec2::ONE)
    );
    // A path whose vertices straddle the area without landing in it still
    // passes over it (a wireform sampled every 10 mm over a 1 mm area).
    let path = [DVec2::new(0.495, -1.0), DVec2::new(0.505, 2.0)];
    assert!(!path.iter().any(|&q| a.at(0, q)));
    assert!(a.on_path(0, &path));
    assert!(!a.on_path(1, &path));
    assert!(a.on_path(2, &path) && Allowed::Anywhere.on_path(1, &path));
    let beside = [DVec2::new(1.5, -1.0), DVec2::new(1.5, 2.0)];
    assert!(!a.on_path(0, &beside));
    assert!(a.on_path(0, &[DVec2::new(0.5, 0.5)]));
    // A path running along an area edge (collinear overlap, both ends
    // outside) passes over the area's boundary, which counts as inside
    // (`covers`), in either direction.
    let along = [DVec2::new(-1.0, 1.0), DVec2::new(2.0, 1.0)];
    assert!(a.on_path(0, &along));
    assert!(a.on_path(0, &[along[1], along[0]]));
    // Touching only a corner counts too; passing just beside it does not.
    assert!(a.on_path(0, &[DVec2::new(2.0, 0.0), DVec2::new(0.0, 2.0)]));
    assert!(!a.on_path(0, &[DVec2::new(2.0, 0.0), DVec2::new(0.0, 2.001)]));
    assert!(!a.on_path(0, &[DVec2::new(-1.0, 1.001), DVec2::new(2.0, 1.001)]));
}

#[test]
fn reports_convert_and_display() {
    let e = ClearanceError {
        feature: Feature::Lane,
        name: "l".into(),
        profile: "jumbo_xl_resin".into(),
        file: "table.toml".into(),
        path: "splines[0]".into(),
        required_mm: 80.4,
        actual_mm: 30.0,
        message: "too narrow".into(),
    };
    assert_eq!(e.to_string(), "table.toml: splines[0]: too narrow");
    let ok = ClearanceReport {
        errors: vec![],
        warnings: vec![e.clone()],
    };
    assert!(ok.is_ok());
    assert_eq!(ok.into_result().unwrap().len(), 1);
    let bad = ClearanceReport {
        errors: vec![e],
        warnings: vec![],
    };
    assert!(!bad.is_ok());
    assert_eq!(bad.into_result().unwrap_err()[0].path, "splines[0]");
    assert_eq!(mm(0.0794), "79.4 mm");
}

/// A ball resting on an inclined support is tangent to it, so under a
/// parallel inclined ceiling its headroom (`r + distance`) is the normal
/// separation; centring it `r` straight above the support overstated it by
/// `r (1 - n·z)` and let a too-low ceiling pass.
#[test]
fn resting_centre_on_an_inclined_support() {
    use super::geom::Tri;
    use glam::DVec3;
    let tri = |p: [DVec3; 3]| {
        let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize();
        Tri {
            p,
            n,
            layer: 0,
            mesh: 0,
            min: p[0].min(p[1]).min(p[2]),
            max: p[0].max(p[1]).max(p[2]),
        }
    };
    // 30° slope rising along +x; ceiling parallel, `gap` above along the normal.
    let slope = 30f64.to_radians().tan();
    let plane = |dz: f64| {
        tri([
            DVec3::new(-1.0, -1.0, -slope + dz),
            DVec3::new(1.0, -1.0, slope + dz),
            DVec3::new(0.0, 1.0, dz),
        ])
    };
    let support = plane(0.0);
    let (r, gap) = (0.0135, 0.0275);
    let ceiling = plane(gap / support.nz());
    let q = DVec2::new(0.1, 0.0);
    let c = super::headroom::resting_centre(&support, q, r);
    assert!((support.distance(c) - r).abs() < 1e-12);
    let headroom = r + ceiling.distance(c);
    assert!((headroom - gap).abs() < 1e-12, "{headroom}");
    // A 27 mm ball needs 28 mm with the clearance margin: the 27.5 mm
    // separation is too low, which the old vertical offset hid.
    let need = 2.0 * r + f64::from(MIN_CLEARANCE_MM) * 1.0e-3;
    assert!(headroom < need, "{headroom} vs {need}");
    let old = DVec3::new(q.x, q.y, support.z_at(q) + r);
    assert!(r + ceiling.distance(old) > need);
}

/// The VUK exit support test measures to the interpolated spline the
/// physics uses, not to the chords between its control points.
#[test]
fn exit_support_measures_to_the_spline_not_its_chords() {
    use glam::{DVec3, Vec3};
    let pts = [[0.0f32, 0.0, 0.0], [0.3, 0.3, 0.0], [0.6, 0.0, 0.0]];
    let v: Vec<Vec3> = pts.iter().map(|&p| Vec3::from_array(p)).collect();
    let sp = super::geom::build_spline(&v, false).unwrap();
    let on_curve = sp.point(0.5).as_dvec3();
    let chord_mid = DVec3::new(0.15, 0.15, 0.0);
    // Distance from a point to the chord (0,0)-(0.3,0.3) (the nearer one).
    let to_chord = |q: DVec3| (q.x - q.y).abs() / 2f64.sqrt();
    assert!(to_chord(on_curve) > 5.0e-3, "{on_curve}");
    assert!(sp.closest_point(chord_mid.as_vec3(), None).distance > 5.0e-3);
    let reach = 1.0e-3;
    assert!(super::checks::near_spline(&pts, false, on_curve, reach));
    assert!(!super::checks::near_spline(&pts, false, chord_mid, reach));
    // A spline that does not construct falls back to the chords.
    let bad = [[0.0f32, 0.0, 0.0], [0.0, 0.0, 0.0]];
    let bad_v: Vec<Vec3> = bad.iter().map(|&p| Vec3::from_array(p)).collect();
    assert!(super::geom::build_spline(&bad_v, false).is_none());
    assert!(super::checks::near_spline(&bad, false, DVec3::ZERO, reach));
    assert!(!super::checks::near_spline(&bad, false, DVec3::X, reach));
}

// ------------------------------------------- fix-2.8-backbox-clearance

mod backbox_bundle {
    use super::super::*;
    use crate::backbox::{generate_backbox, parse_backbox_layout, write_backbox};
    use pin_core::schema::LayoutManifest;
    use pin_core::schema::table::{BallRestriction, CoilDef, CoilKind, SwitchDef, SwitchKind};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub const MINI: &str = "mini_steel";
    pub const XL: &str = "jumbo_xl_resin";

    /// A backbox deep and wide enough for a 79.4 mm ball, no flippers, its
    /// VUK at (300, 400) on the playfield.
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

    fn apex_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../pin_assets/tests/fixtures/apex")
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

    /// A bundle folder: the apex fixture (no flippers, splines,
    /// transitions or mechs) with `profiles` and `restrict`, a bare
    /// 514 x 1070 mm generated playfield and, with `backbox`, the generated
    /// [`BACKBOX`]. Deleted on drop.
    pub struct Bundle(pub PathBuf);

    impl Drop for Bundle {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl Bundle {
        pub fn vfs(&self) -> Vfs {
            Vfs::open(&self.0).unwrap()
        }
    }

    pub fn bundle(profiles: &[&str], restrict: Vec<BallRestriction>, backbox: bool) -> Bundle {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "pin_tablegen_clearance_unit_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        copy_dir(&apex_dir(), &dir);
        let guard = Bundle(dir);
        let text = std::fs::read_to_string(guard.0.join("table.toml")).unwrap();
        let mut base = pin_core::schema::TableManifest::from_toml_str(&text).unwrap();
        base.balls.profiles = profiles.iter().map(|s| (*s).to_owned()).collect();
        base.balls.restrict = restrict;
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
        let main = crate::generate(&layout, &base).unwrap_or_else(|e| panic!("{e}"));
        crate::write_bundle(&main, &guard.0).unwrap();
        if backbox {
            let bb = parse_backbox_layout(BACKBOX).unwrap_or_else(|e| panic!("{e}"));
            let g = generate_backbox(&bb.backbox, &main.table).unwrap_or_else(|e| panic!("{e}"));
            write_backbox(&g, &guard.0).unwrap();
        } else {
            // The apex backglass names a backbox scene; give it one.
            std::fs::write(guard.0.join("backbox.glb"), b"placeholder").unwrap();
        }
        guard
    }

    pub fn square(x: f32, y: f32, side: f32) -> Vec<[f32; 2]> {
        vec![[x, y], [x + side, y], [x + side, y + side], [x, y + side]]
    }

    pub fn restrict(profile: &str, layers: &[u8], area: Vec<[f32; 2]>) -> BallRestriction {
        BallRestriction {
            profile: profile.to_owned(),
            layers: layers.to_vec(),
            area,
        }
    }
}

/// (a) A generated backbox: its VUK ejects onto the backbox layer (4)
/// over the entry hole; the box's colliders (read from
/// `backbox_colliders.glb`) support it. The 15.88 mm ball (and the
/// apex dispenser's 27 mm balls) validate. The 79.4 mm ball's only finding
/// is a true overlap: the generator puts the exit 13.5 mm (a standard
/// ball's radius) from the back panel whatever the ball, so a 39.7 mm
/// radius overlaps it; it is never refused for want of a surface (see
/// docs/decisions/2026-11-18-fix-2.8-backbox-clearance.md).
#[test]
fn generated_backbox_vuk_exit_validates() {
    use backbox_bundle::*;
    let b = bundle(&[MINI], vec![], true);
    let vfs = b.vfs();
    let table = load_table(&vfs).unwrap();
    let vuk = table
        .manifest
        .mechs
        .iter()
        .find(|m| m.id == "tablegen:backbox.vuk")
        .unwrap();
    assert_eq!(vuk.exit_layer, Some(4));
    let r = validate_bundle(&vfs).unwrap();
    assert!(r.is_ok(), "{:#?}", r.errors);
    let profiles: Vec<&str> = table
        .manifest
        .balls
        .profiles
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(profiles, [MINI]);
    // The same through the diameters entry point, 15.88 and 79.4 mm.
    let scene = load_gltf(&vfs, "playfield.glb").unwrap();
    let bb = load_backbox_colliders(&vfs).unwrap().unwrap();
    // The generator names the backbox layer on every node.
    assert!(
        bb.meshes
            .iter()
            .all(|m| m.extras.collider && m.extras.collision_layer == Some(4))
    );
    let r = validate_clearance_for(&table, &scene, Some(&bb), Some(&[0.01588]));
    assert!(r.is_ok(), "{:#?}", r.errors);
    let r = validate_clearance_for(&table, &scene, Some(&bb), Some(&[0.0794]));
    assert_eq!(r.errors.len(), 1, "{:#?}", r.errors);
    let e = &r.errors[0];
    assert_eq!(e.feature, Feature::VukExit);
    assert!(e.message.contains("overlaps it by"), "{}", e.message);
    assert!((e.actual_mm - 13.5).abs() < 0.1, "{}", e.actual_mm);

    let b = bundle(&[MINI, XL], vec![], true);
    let r = validate_bundle(&b.vfs()).unwrap();
    assert!(
        r.errors.iter().all(|e| !e.message.contains("no surface")),
        "{:#?}",
        r.errors
    );
    assert!(
        r.errors
            .iter()
            .all(|e| e.profile == XL && e.feature == Feature::VukExit),
        "{:#?}",
        r.errors
    );
}

/// (b) The same bundle without `backbox_colliders.glb`: nothing of layer 4
/// is under the exit, refused with the old message, and exactly as
/// [`validate_clearance`] (no backbox) reports it.
#[test]
fn backbox_vuk_exit_without_colliders_is_refused() {
    use backbox_bundle::*;
    let b = bundle(&[MINI], vec![], true);
    std::fs::remove_file(b.0.join(pin_assets::BACKBOX_COLLIDERS_FILE)).unwrap();
    let vfs = b.vfs();
    assert!(load_backbox_colliders(&vfs).unwrap().is_none());
    let r = validate_bundle(&vfs).unwrap();
    assert!(!r.is_ok());
    let msgs: Vec<&str> = r
        .errors
        .iter()
        .filter(|e| e.profile == MINI)
        .map(|e| e.message.as_str())
        .collect();
    assert_eq!(msgs.len(), 1, "{:#?}", r.errors);
    assert!(
        msgs[0].starts_with("vuk \"tablegen:backbox.vuk\" ejects ball \"mini_steel\" (15.9 mm) to a point with no surface of layer 4 below it"),
        "{}",
        msgs[0]
    );
    let table = load_table(&vfs).unwrap();
    let scene = load_gltf(&vfs, "playfield.glb").unwrap();
    assert_eq!(r, validate_clearance(&table, &scene));
    assert_eq!(r, validate_clearance_with_backbox(&table, &scene, None));
}

/// Backbox colliders stay on their own layer: a layer-0 VUK exit off the
/// playfield is not supported by them, and they are no layer-0 walls.
#[test]
fn backbox_colliders_are_not_playfield_geometry() {
    use backbox_bundle::*;
    let b = bundle(&[MINI], vec![], true);
    let vfs = b.vfs();
    let mut table = load_table(&vfs).unwrap();
    let scene = load_gltf(&vfs, "playfield.glb").unwrap();
    let bb = load_backbox_colliders(&vfs).unwrap().unwrap();
    let bb_tris = {
        let mut t = Vec::new();
        geom::append_collider_triangles(&bb, |m| backbox_node_layer(m).ok(), 0, &mut t);
        t
    };
    assert!(!bb_tris.is_empty() && bb_tris.iter().all(|t| t.layer == 4));
    // Retarget the generated VUK's exit to layer 0, same point (inside the
    // box, far off the playfield): no layer-0 surface below it.
    let i = table
        .manifest
        .mechs
        .iter()
        .position(|m| m.id == "tablegen:backbox.vuk")
        .unwrap();
    table.manifest.mechs[i].exit_layer = Some(0);
    let r = validate_clearance_with_backbox(&table, &scene, Some(&bb));
    assert!(
        r.errors
            .iter()
            .any(|e| e.message.contains("no surface of layer 0")),
        "{:#?}",
        r.errors
    );
    // Each node keeps its own backbox layer, mesh indices offset; a node
    // naming no layer adds no geometry (no default layer).
    let mut mixed = bb.clone();
    for (k, m) in mixed.meshes.iter_mut().enumerate() {
        m.extras.collision_layer = [Some(6), None][k % 2];
    }
    let mut t = Vec::new();
    geom::append_collider_triangles(&mixed, |m| backbox_node_layer(m).ok(), 3, &mut t);
    assert!(!t.is_empty());
    assert!(
        t.iter()
            .all(|t| t.layer == 6 && t.mesh >= 3 && (t.mesh - 3) % 2 == 0)
    );
}

/// The backbox support reach is plan distance to a backbox floor below
/// the ball: none within reach, or only above it, is no support.
#[test]
fn backbox_floor_support_needs_reach_and_height() {
    use backbox_bundle::*;
    let b = bundle(&[MINI], vec![], true);
    let vfs = b.vfs();
    let mut table = load_table(&vfs).unwrap();
    let scene = load_gltf(&vfs, "playfield.glb").unwrap();
    let bb = load_backbox_colliders(&vfs).unwrap().unwrap();
    let i = table
        .manifest
        .mechs
        .iter()
        .position(|m| m.id == "tablegen:backbox.vuk")
        .unwrap();
    let base = table.manifest.mechs[i].exit_offset.unwrap();
    let no_surface = |table: &LoadedTable| {
        validate_clearance_with_backbox(table, &scene, Some(&bb))
            .errors
            .iter()
            .any(|e| e.message.contains("no surface of layer 4"))
    };
    assert!(!no_surface(&table));
    // 400 mm below the box: the shelf is above, not below.
    table.manifest.mechs[i].exit_offset = Some([base[0], base[1], base[2] - 400.0]);
    assert!(no_surface(&table));
    // 300 mm beside the box (x): out of reach of any shelf.
    table.manifest.mechs[i].exit_offset = Some([base[0] - 300.0, base[1], base[2]]);
    assert!(no_surface(&table));
}

fn restriction_errors(r: &ClearanceReport) -> Vec<&ClearanceError> {
    r.errors
        .iter()
        .filter(|e| e.feature == Feature::Restriction)
        .collect()
}

/// (c) A 40 mm square cannot hold the 79.4 mm ball: an error at
/// `balls.restrict[i].area` naming the profile, its diameter and the
/// largest circle that fits. (d) A 100 mm square can.
#[test]
fn restrict_area_must_hold_the_ball() {
    use backbox_bundle::*;
    let b = bundle(
        &[MINI],
        vec![
            restrict(MINI, &[0], vec![]),
            restrict(XL, &[0], square(200.0, 300.0, 40.0)),
        ],
        false,
    );
    let r = validate_bundle(&b.vfs()).unwrap();
    let e = restriction_errors(&r);
    assert_eq!(e.len(), 1, "{:#?}", r.errors);
    let e = e[0];
    assert_eq!(e.path, "balls.restrict[1].area");
    assert_eq!(e.name, "balls.restrict[1]");
    assert_eq!(e.file, "table.toml");
    assert_eq!(e.profile, XL);
    assert!(e.message.contains(XL), "{}", e.message);
    assert!(e.message.contains("79.4 mm"), "{}", e.message);
    assert!(e.message.contains("40.0 mm"), "{}", e.message);
    assert!((e.required_mm - 79.4).abs() < 1.0e-3);
    assert!(
        e.actual_mm <= 40.0 && e.actual_mm >= 39.5,
        "{}",
        e.actual_mm
    );
    assert!(!r.is_ok());

    let b = bundle(
        &[MINI],
        vec![restrict(XL, &[0], square(200.0, 300.0, 100.0))],
        false,
    );
    let r = validate_bundle(&b.vfs()).unwrap();
    assert!(restriction_errors(&r).is_empty(), "{:#?}", r.errors);
    assert!(r.is_ok(), "{:#?}", r.errors);
}

/// (e) A thin 30 x 400 mm strip holds the 15.88 mm ball, not the 79.4 mm
/// one; an entry without an area is never checked.
#[test]
fn thin_strip_holds_only_the_small_ball() {
    use backbox_bundle::*;
    let strip = vec![
        [200.0, 300.0],
        [230.0, 300.0],
        [230.0, 700.0],
        [200.0, 700.0],
    ];
    let b = bundle(&[MINI], vec![restrict(XL, &[0], strip.clone())], false);
    let r = validate_bundle(&b.vfs()).unwrap();
    let e = restriction_errors(&r);
    assert_eq!(e.len(), 1, "{:#?}", r.errors);
    assert_eq!(e[0].path, "balls.restrict[0].area");
    assert!(e[0].message.contains("30.0 mm"), "{}", e[0].message);

    let b = bundle(
        &[MINI],
        vec![restrict(MINI, &[0], strip), restrict(XL, &[0], vec![])],
        false,
    );
    let r = validate_bundle(&b.vfs()).unwrap();
    assert!(restriction_errors(&r).is_empty(), "{:#?}", r.errors);
    assert!(r.is_ok(), "{:#?}", r.errors);
}

/// (f) Squares of 70..=90 mm for the 79.4 mm ball: refused below, accepted
/// above, with exactly one transition within 0.5 mm of 79.4 mm.
#[test]
fn square_sweep_has_one_transition_at_the_ball_diameter() {
    use backbox_bundle::*;
    let b = bundle(&[MINI], vec![], false);
    let vfs = b.vfs();
    let mut table = load_table(&vfs).unwrap();
    let scene = load_gltf(&vfs, "playfield.glb").unwrap();
    let mut verdicts = Vec::new();
    for side in 70..=90u16 {
        let side = f32::from(side);
        table.manifest.balls.restrict = vec![restrict(XL, &[0], square(200.0, 300.0, side))];
        let r = validate_clearance(&table, &scene);
        let refused = restriction_errors(&r)
            .iter()
            .any(|e| e.path == "balls.restrict[0].area");
        verdicts.push((side, refused));
    }
    let flips: Vec<f32> = verdicts
        .windows(2)
        .filter(|w| w[0].1 != w[1].1)
        .map(|w| 0.5 * (w[0].0 + w[1].0))
        .collect();
    assert_eq!(flips.len(), 1, "{verdicts:?}");
    assert!((flips[0] - 79.4).abs() <= 0.5, "{verdicts:?}");
    assert!(verdicts[0].1 && !verdicts[20].1, "{verdicts:?}");
}

mod inscribed_circle {
    use super::super::geom::Polygon;
    use super::super::inscribed::largest_inscribed_diameter;
    use glam::DVec2;
    use proptest::prelude::*;

    /// Radius precision used by the validator: a quarter of the 0.5 mm
    /// diameter tolerance.
    const PREC: f64 = 0.125e-3;
    const TOL: f64 = 0.5e-3;

    fn poly(p: &[[f64; 2]]) -> Polygon {
        Polygon {
            points: p.iter().map(|&[x, y]| DVec2::new(x, y)).collect(),
        }
    }

    fn rect(x: f64, y: f64, w: f64, h: f64) -> Polygon {
        poly(&[[x, y], [x + w, y], [x + w, y + h], [x, y + h]])
    }

    #[test]
    fn degenerate_inputs_give_zero() {
        assert_eq!(
            largest_inscribed_diameter(&poly(&[[0.0, 0.0], [1.0, 0.0]]), PREC),
            0.0
        );
        let line = poly(&[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]]);
        assert_eq!(largest_inscribed_diameter(&line, PREC), 0.0);
        let dot = poly(&[[1.0, 1.0], [1.0, 1.0], [1.0, 1.0]]);
        assert_eq!(largest_inscribed_diameter(&dot, PREC), 0.0);
        let nan = poly(&[[0.0, 0.0], [1.0, 0.0], [f64::NAN, 1.0]]);
        assert_eq!(largest_inscribed_diameter(&nan, PREC), 0.0);
        assert_eq!(
            largest_inscribed_diameter(&rect(0.0, 0.0, 1.0, 1.0), 0.0),
            0.0
        );
        // Traced twice: no even-odd interior.
        let sq = [[0.0, 0.0], [0.1, 0.0], [0.1, 0.1], [0.0, 0.1]];
        let twice: Vec<[f64; 2]> = sq.iter().chain(&sq).copied().collect();
        assert_eq!(largest_inscribed_diameter(&poly(&twice), PREC), 0.0);
    }

    #[test]
    fn known_shapes() {
        let d = largest_inscribed_diameter(&rect(0.2, 0.3, 0.03, 0.4), PREC);
        assert!((0.03 - TOL..=0.03).contains(&d), "{d}");
        // An L with 40 mm arms: the circle touches both outer edges and the
        // reflex corner (0.04, 0.04): r = 0.04·√2 / (1 + √2).
        let l = poly(&[
            [0.0, 0.0],
            [0.2, 0.0],
            [0.2, 0.04],
            [0.04, 0.04],
            [0.04, 0.2],
            [0.0, 0.2],
        ]);
        let want = 2.0 * 0.04 * core::f64::consts::SQRT_2 / (1.0 + core::f64::consts::SQRT_2);
        let d = largest_inscribed_diameter(&l, PREC);
        assert!(d <= want + 1e-12 && d >= want - TOL, "{d} vs {want}");
        // A square with a square hole (even-odd): a 100 mm frame 20 mm wide;
        // its corners hold the same circle as an L with 20 mm arms.
        let frame = poly(&[
            [0.0, 0.0],
            [0.1, 0.0],
            [0.1, 0.1],
            [0.0, 0.1],
            [0.0, 0.0],
            [0.02, 0.02],
            [0.02, 0.08],
            [0.08, 0.08],
            [0.08, 0.02],
            [0.02, 0.02],
        ]);
        let want = 0.5 * want;
        let d = largest_inscribed_diameter(&frame, PREC);
        assert!(d <= want + 1e-12 && d >= want - TOL, "{d} vs {want}");
        // A needle 1 µm thin and a metre long starts from bounded cells.
        let d = largest_inscribed_diameter(&rect(0.0, 0.0, 1.0, 1.0e-6), PREC);
        assert!((0.0..=1.0e-6).contains(&d), "{d}");
    }

    proptest! {
        /// Axis-aligned rectangles: the shorter side, never above it and at
        /// most the 0.5 mm tolerance below it; deterministic.
        #[test]
        fn rectangles(x in -1.0f64..1.0, y in -1.0f64..1.0, w in 0.001f64..1.0, h in 0.001f64..1.0) {
            let p = rect(x, y, w, h);
            let d = largest_inscribed_diameter(&p, PREC);
            let want = w.min(h);
            prop_assert!(d <= want + 1e-12 && d >= want - TOL, "{d} vs {want}");
            prop_assert_eq!(d.to_bits(), largest_inscribed_diameter(&p, PREC).to_bits());
        }

        /// Triangles (either winding): the incircle diameter `4A / P`.
        #[test]
        fn triangles(p in prop::array::uniform3((-0.5f64..0.5, -0.5f64..0.5))) {
            let pts = p.map(|(x, y)| DVec2::new(x, y));
            let area = 0.5 * ((pts[1] - pts[0]).perp_dot(pts[2] - pts[0])).abs();
            prop_assume!(area > 1.0e-4);
            let per = pts[0].distance(pts[1]) + pts[1].distance(pts[2]) + pts[2].distance(pts[0]);
            let want = 4.0 * area / per;
            let d = largest_inscribed_diameter(&Polygon { points: pts.to_vec() }, PREC);
            prop_assert!(d <= want + 1e-9 && d >= want - TOL, "{d} vs {want}");
        }

        /// Rotated squares: the side, whatever the rotation.
        #[test]
        fn rotated_squares(a in 0.0f64..core::f64::consts::TAU, s in 0.01f64..0.5) {
            let (sn, cs) = a.sin_cos();
            let pts: Vec<DVec2> = [[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]]
                .iter()
                .map(|&[x, y]| {
                    let (x, y) = (0.5 * s * x, 0.5 * s * y);
                    DVec2::new(x * cs - y * sn, x * sn + y * cs)
                })
                .collect();
            let d = largest_inscribed_diameter(&Polygon { points: pts }, PREC);
            prop_assert!(d <= s + 1e-9 && d >= s - TOL, "{d} vs {s}");
        }

        /// Any polygon, self-intersecting ones included: finite, at least 0,
        /// at most the bounding box's shorter side, and the circle exists:
        /// a dense grid never finds a point deeper than the result plus the
        /// tolerance and the grid spacing.
        #[test]
        fn any_polygon(p in prop::collection::vec((-0.3f64..0.3, -0.3f64..0.3), 3..9)) {
            let poly = Polygon { points: p.iter().map(|&(x, y)| DVec2::new(x, y)).collect() };
            let d = largest_inscribed_diameter(&poly, PREC);
            prop_assert!(d.is_finite() && d >= 0.0);
            let mut lo = DVec2::splat(f64::INFINITY);
            let mut hi = DVec2::splat(f64::NEG_INFINITY);
            for &q in &poly.points {
                lo = lo.min(q);
                hi = hi.max(q);
            }
            prop_assert!(d <= (hi - lo).min_element() + 1e-12);
            let step = 0.005;
            let mut deepest: f64 = 0.0;
            for j in 0..=120 {
                for i in 0..=120 {
                    let q = DVec2::new(-0.3 + f64::from(i) * step, -0.3 + f64::from(j) * step);
                    if poly.contains(q) {
                        deepest = deepest.max(poly.boundary_distance(q));
                    }
                }
            }
            prop_assert!(2.0 * deepest <= d + TOL + 1e-12, "grid {} vs {d}", 2.0 * deepest);
        }
    }
}

/// CodeRabbit 4622606031: a backbox collider node without
/// `extras.collision_layer`, or with one outside 4..=7, is an error naming
/// `backbox_colliders.glb`, the node and the loader's condition (as
/// `pin_sim::load::backbox_colliders` refuses it), never default-layer
/// geometry.
#[test]
fn backbox_node_without_a_backbox_layer_is_an_error() {
    use backbox_bundle::*;
    let b = bundle(&[MINI], vec![], true);
    let vfs = b.vfs();
    let table = load_table(&vfs).unwrap();
    let scene = load_gltf(&vfs, "playfield.glb").unwrap();
    let bb = load_backbox_colliders(&vfs).unwrap().unwrap();
    assert!(validate_clearance_with_backbox(&table, &scene, Some(&bb)).is_ok());
    for (k, bad) in [None, Some(3u8), Some(8)].into_iter().enumerate() {
        let mut broken = bb.clone();
        let i = k.min(broken.meshes.len() - 1);
        broken.meshes[i].extras.collision_layer = bad;
        let node = broken.meshes[i].name.clone();
        let r = validate_clearance_with_backbox(&table, &scene, Some(&broken));
        let want = format!(
            "node {node:?}: extras.collision_layer must be a backbox layer 4..=7, got {bad:?}"
        );
        let found: Vec<&ClearanceError> = r
            .errors
            .iter()
            .filter(|e| e.file == pin_assets::BACKBOX_COLLIDERS_FILE && e.message == want)
            .collect();
        assert_eq!(found.len(), 1, "{bad:?}: {:#?}", r.errors);
        let e = found[0];
        assert_eq!(e.path, format!("meshes[{i}]"));
        assert_eq!(e.name, node);
        assert!(e.to_string().starts_with("backbox_colliders.glb: "), "{e}");
        assert_eq!(e.feature, Feature::BackboxCollider);
        assert!(!r.is_ok());
    }
    // As in the loader, a node that is not a collider is refused too.
    let mut broken = bb.clone();
    broken.meshes[0].extras.collider = false;
    let node = broken.meshes[0].name.clone();
    let r = validate_clearance_with_backbox(&table, &scene, Some(&broken));
    let want = format!("node {node:?} is not a collider (extras.collider must be true)");
    assert!(
        r.errors
            .iter()
            .any(|e| e.file == pin_assets::BACKBOX_COLLIDERS_FILE
                && e.path == "meshes[0]"
                && e.message == want),
        "{:#?}",
        r.errors
    );
}
