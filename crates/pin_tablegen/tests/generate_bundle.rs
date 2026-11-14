//! PLAN 4.3 verification on the Appendix A.3 layout and a fuller fixture:
//! the bundle validates, extras and lamp bindings, visual/collision
//! agreement, determinism, re-runs, invalid layouts, and the CLI.

mod support;

use glam::Vec3;
use pin_assets::{GltfScene, LoadedTable, MeshData, validate_gltf_bindings};
use pin_core::schema::LayoutManifest;
use pin_core::schema::table::SplineKind;
use pin_physics::world::CubicSpline;
use pin_tablegen::layout::{GENERATED_PREFIX, generate};
use support::*;

/// PLAN 4.3: visual mesh and collision geometry agree within 0.1 mm.
const AGREE_M: f32 = 1.0e-4;

fn plan_dist_to_segment(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let (abx, aby) = (b[0] - a[0], b[1] - a[1]);
    let len2 = abx * abx + aby * aby;
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * abx + (p[1] - a[1]) * aby) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (dx, dy) = (p[0] - (a[0] + abx * t), p[1] - (a[1] + aby * t));
    (dx * dx + dy * dy).sqrt()
}

fn triangles(m: &MeshData) -> impl Iterator<Item = [[f32; 3]; 3]> + '_ {
    m.indices.chunks_exact(3).map(|t| {
        [
            m.positions[t[0] as usize],
            m.positions[t[1] as usize],
            m.positions[t[2] as usize],
        ]
    })
}

/// Points on the mesh surface: vertices, edge midpoints, centroids.
fn surface_samples(m: &MeshData) -> Vec<[f32; 3]> {
    let mut out = m.positions.clone();
    for [a, b, c] in triangles(m) {
        for (p, q) in [(a, b), (b, c), (c, a)] {
            out.push([
                (p[0] + q[0]) / 2.0,
                (p[1] + q[1]) / 2.0,
                (p[2] + q[2]) / 2.0,
            ]);
        }
        out.push([
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ]);
    }
    out
}

fn area(m: &MeshData) -> f64 {
    triangles(m)
        .map(|[a, b, c]| {
            let u = Vec3::from_array(b) - Vec3::from_array(a);
            let v = Vec3::from_array(c) - Vec3::from_array(a);
            0.5 * f64::from(u.cross(v).length())
        })
        .sum()
}

fn mm(p: [f32; 2]) -> [f32; 2] {
    [p[0] / 1000.0, p[1] / 1000.0]
}

fn spline_of(table: &LoadedTable, name: &str) -> CubicSpline {
    let s = table
        .splines
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no spline {name}"));
    let pts: Vec<Vec3> = s.points_m.iter().map(|&p| Vec3::from_array(p)).collect();
    if s.closed {
        CubicSpline::interpolate_closed(&pts).unwrap()
    } else {
        CubicSpline::interpolate(&pts).unwrap()
    }
}

/// Every collider node against the layout's ideal geometry, every guide
/// node against the loader's spline (both directions).
fn check_agreement(layout: &LayoutManifest, table: &LoadedTable, scene: &GltfScene) {
    let h = layout.playfield.wall_height / 1000.0;
    let mut seen = 0;
    for m in &scene.meshes {
        let name = m.name.as_str();
        let index = |prefix: &str| -> Option<usize> {
            name.strip_prefix(prefix)?.strip_suffix(']')?.parse().ok()
        };
        if m.extras.collider {
            assert!(m.extras.collision_layer.is_some(), "{name}");
            assert!(m.material.is_some(), "{name}");
        }
        // Solid geometry collides, on the main layer unless it is a deck.
        let solid = |layer: u8| {
            assert!(m.extras.collider, "{name}: solid geometry must collide");
            assert_eq!(m.extras.collision_layer, Some(layer), "{name}");
        };
        if name == "floor" {
            solid(0);
            let (w, l) = (
                layout.playfield.width / 1000.0,
                layout.playfield.length / 1000.0,
            );
            for p in &m.positions {
                assert!(p[2] == 0.0 && (0.0..=w).contains(&p[0]) && (0.0..=l).contains(&p[1]));
            }
            assert!((area(m) - f64::from(w * l)).abs() < 1e-9, "{name}");
            seen += 1;
        } else if let Some(i) = index("walls[") {
            solid(0);
            let wall = &layout.walls[i];
            let mut pts: Vec<[f32; 2]> = wall.points.iter().map(|&p| mm(p)).collect();
            if wall.closed {
                pts.push(pts[0]);
            }
            for p in surface_samples(m) {
                let d = pts
                    .windows(2)
                    .map(|w| plan_dist_to_segment([p[0], p[1]], w[0], w[1]))
                    .fold(f32::INFINITY, f32::min);
                assert!(d <= AGREE_M, "{name}: {p:?} is {d} m from the wall");
                assert!((-1e-7..=h + 1e-6).contains(&p[2]), "{name}: {p:?}");
            }
            // Vice versa: every wall point is a mesh vertex at both heights.
            for q in &pts {
                for z in [0.0, h] {
                    assert!(
                        m.positions.iter().any(|p| (Vec3::from_array(*p)
                            - Vec3::new(q[0], q[1], z))
                        .length()
                            < 1e-6),
                        "{name}: {q:?} at z {z} not covered"
                    );
                }
            }
            seen += 1;
        } else if let Some(i) = index("posts[") {
            solid(0);
            let post = layout.posts[i];
            let (c, r) = (mm(post.position), post.radius / 1000.0);
            for p in surface_samples(m) {
                let d = ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2)).sqrt();
                let on_cap = (p[2] - h).abs() < 1e-7 && d <= r + 1e-6;
                let on_side = (d - r).abs() <= AGREE_M && (-1e-7..=h + 1e-6).contains(&p[2]);
                assert!(on_cap || on_side, "{name}: {p:?} (d {d}, r {r})");
            }
            seen += 1;
        } else if let Some(i) = index("decks[") {
            let deck = &layout.decks[i];
            solid(deck.layer);
            let z = deck.z / 1000.0;
            let outline: Vec<[f32; 2]> = deck.outline.iter().map(|&p| mm(p)).collect();
            for p in &m.positions {
                assert!((p[2] - z).abs() < 1e-7, "{name}");
                assert!(
                    outline
                        .iter()
                        .any(|q| (q[0] - p[0]).abs() < 1e-6 && (q[1] - p[1]).abs() < 1e-6)
                );
            }
            let mut a2 = 0.0f64;
            for k in 0..outline.len() {
                let (a, b) = (outline[k], outline[(k + 1) % outline.len()]);
                a2 += f64::from(a[0]) * f64::from(b[1]) - f64::from(b[0]) * f64::from(a[1]);
            }
            assert!((area(m) - a2.abs() / 2.0).abs() < 1e-8, "{name}");
            seen += 1;
        } else if name.starts_with(GENERATED_PREFIX) {
            assert!(
                !m.extras.collider,
                "{name}: guides collide through their spline"
            );
            let spline = spline_of(table, name);
            for p in surface_samples(m) {
                let c = spline.closest_point(Vec3::new(p[0], p[1], 0.0), None);
                assert!(
                    c.distance <= AGREE_M,
                    "{name}: {p:?} is {} m off",
                    c.distance
                );
                assert!((-1e-7..=h + 1e-6).contains(&p[2]));
            }
            let bottom: Vec<[f32; 2]> = m
                .positions
                .iter()
                .filter(|p| p[2] == 0.0)
                .map(|p| [p[0], p[1]])
                .collect();
            let n = spline.segment_count() * 50;
            for k in 0..=n {
                let q = spline.point(k as f32 / 50.0);
                let d = bottom
                    .windows(2)
                    .map(|w| plan_dist_to_segment([q.x, q.y], w[0], w[1]))
                    .fold(f32::INFINITY, f32::min);
                assert!(
                    d <= AGREE_M,
                    "{name}: spline point {q} is {d} m from the mesh"
                );
            }
            seen += 1;
        }
    }
    let lanes = layout.lanes.len() * 2;
    assert_eq!(
        seen,
        1 + layout.walls.len() + layout.posts.len() + layout.decks.len() + lanes
    );
}

fn check_bundle(layout_name: &str) {
    let layout = layout(layout_name);
    let generated = gen_apex(layout_name);
    let (dir, table, scene) = load(&generated, layout_name.trim_end_matches(".toml"));
    // 4.1 validation passed (load_table returned Ok). The generated splines
    // get no warnings either.
    for (k, s) in table.manifest.splines.iter().enumerate() {
        if s.id.starts_with(GENERATED_PREFIX) {
            let path = format!("splines[{k}]");
            let w: Vec<_> = table
                .warnings
                .iter()
                .filter(|w| w.path == path || w.path.starts_with(&format!("{path}.")))
                .collect();
            assert!(w.is_empty(), "{layout_name}: {w:#?}");
            assert_eq!(s.kind, SplineKind::Guide);
        }
    }
    assert_eq!(generated.table, table.manifest);
    // Lamp bindings.
    assert!(validate_gltf_bindings(&scene, &table.ids).is_empty());
    let lamps: Vec<&str> = scene
        .meshes
        .iter()
        .filter_map(|m| m.extras.lamp.as_deref())
        .collect();
    let want: Vec<&str> = layout.inserts.iter().map(|i| i.lamp.as_str()).collect();
    assert_eq!(lamps, want);
    for m in scene.meshes.iter().filter(|m| m.extras.lamp.is_some()) {
        assert!(!m.extras.collider);
        assert!(m.positions.iter().all(|p| (p[2] - 1.0e-4).abs() < 1e-9));
    }
    check_agreement(&layout, &table, &scene);
    // Ramps: a visual node on the spline's layer, and the entry transition.
    for (i, r) in layout.ramps.iter().enumerate() {
        let node = scene
            .meshes
            .iter()
            .find(|m| m.name == format!("ramps[{i}]"))
            .unwrap();
        let s = table.splines.iter().find(|s| s.name == r.spline).unwrap();
        assert_eq!(node.extras.collision_layer, Some(s.layer));
        assert!(!node.extras.collider);
        assert!(
            table
                .transitions
                .iter()
                .any(|t| t.name == format!("{GENERATED_PREFIX}ramps[{i}].entry")
                    && t.to_layer == s.layer)
        );
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn appendix_a3_layout_validates() {
    check_bundle("apex_layout.toml");
}

#[test]
fn rich_layout_validates() {
    check_bundle("rich_layout.toml");
}

#[test]
fn materials_are_flat_and_named() {
    let generated = gen_apex("rich_layout.toml");
    let (dir, _, scene) = load(&generated, "materials");
    let names: Vec<&str> = scene.materials.iter().map(|m| m.name.as_str()).collect();
    for want in [
        "wood",
        "rubber",
        "steel",
        "plastic",
        "acrylic",
        "insert #ff7a00",
    ] {
        assert!(names.contains(&want), "{names:?}");
    }
    assert!(scene.materials.iter().all(|m| m.base_color_image.is_none()));
    for m in scene.meshes.iter().filter(|m| m.extras.collider) {
        let mat = &scene.materials[m.material.unwrap()];
        assert!(
            pin_core::Material::from_name(&mat.name).is_some(),
            "{}",
            m.name
        );
    }
    let post = scene.meshes.iter().find(|m| m.name == "posts[0]").unwrap();
    assert_eq!(scene.materials[post.material.unwrap()].name, "rubber");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn deterministic_over_ten_runs() {
    let first = gen_apex("rich_layout.toml");
    for _ in 0..9 {
        let again = gen_apex("rich_layout.toml");
        assert!(again.glb == first.glb);
        assert_eq!(again.table_toml, first.table_toml);
    }
    // Files on disk too.
    let a = bundle_dir("det_a");
    let b = bundle_dir("det_b");
    pin_tablegen::write_bundle(&first, &a).unwrap();
    pin_tablegen::write_bundle(&gen_apex("rich_layout.toml"), &b).unwrap();
    for f in ["playfield.glb", "table.toml"] {
        assert!(std::fs::read(a.join(f)).unwrap() == std::fs::read(b.join(f)).unwrap());
    }
    std::fs::remove_dir_all(a).unwrap();
    std::fs::remove_dir_all(b).unwrap();
}

/// CodeRabbit (write_bundle): concurrent writes into one folder are
/// serialised, so staging, replacement, rollback and cleanup of one call
/// never interleave with another's. Two threads repeatedly writing
/// different bundles always succeed and leave a matching pair and no
/// staging or backup files.
#[test]
fn concurrent_write_bundles_into_one_folder_leave_a_consistent_pair() {
    let a = gen_apex("apex_layout.toml");
    let b = gen_apex("rich_layout.toml");
    assert!(a.glb != b.glb && a.table_toml != b.table_toml);
    let dir = bundle_dir("concurrent");
    let mut expected = names(&dir);
    expected.extend([
        "playfield.glb".to_owned(),
        "table.toml".to_owned(),
        pin_tablegen::LOCK_FILE.to_owned(),
    ]);
    expected.sort();
    expected.dedup();
    std::thread::scope(|s| {
        for g in [&a, &b] {
            let dir = &dir;
            s.spawn(move || {
                for _ in 0..200 {
                    pin_tablegen::write_bundle(g, dir).unwrap();
                }
            });
        }
    });
    let glb = std::fs::read(dir.join("playfield.glb")).unwrap();
    let toml = std::fs::read_to_string(dir.join("table.toml")).unwrap();
    let pair_of = |g: &pin_tablegen::GeneratedTable| glb == g.glb && toml == g.table_toml;
    assert!(pair_of(&a) || pair_of(&b), "mixed or damaged pair");
    assert_eq!(names(&dir), expected);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A writable output folder under a read-only parent: the lock lives inside
/// the folder, so writing needs no access to the parent. (Skipped when the
/// process can write the read-only parent anyway, e.g. as root.)
#[cfg(unix)]
#[test]
fn write_bundle_needs_no_write_access_to_the_parent() {
    use std::os::unix::fs::PermissionsExt as _;
    let generated = gen_apex("apex_layout.toml");
    let parent = bundle_dir("readonly_parent");
    let dir = parent.join("out");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o555)).unwrap();
    let probe = parent.join("probe");
    let privileged = std::fs::write(&probe, b"").is_ok();
    let result = if privileged {
        let _ = std::fs::remove_file(&probe);
        None
    } else {
        Some(pin_tablegen::write_bundle(&generated, &dir))
    };
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
    match result {
        None => eprintln!("skipped: the read-only parent is writable by this process"),
        Some(r) => {
            r.unwrap();
            assert_eq!(
                std::fs::read(dir.join("playfield.glb")).unwrap(),
                generated.glb
            );
        }
    }
    std::fs::remove_dir_all(parent).unwrap();
}

fn names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn rerun_on_generated_table_never_duplicates() {
    let layout = layout("rich_layout.toml");
    let first = generate(&layout, &apex_base()).unwrap();
    let second = generate(&layout, &first.table).unwrap();
    assert_eq!(first, second);
    let generated = second
        .table
        .splines
        .iter()
        .filter(|s| s.id.starts_with(GENERATED_PREFIX))
        .count();
    assert_eq!(generated, 2 * layout.lanes.len());
    // Hand-written entries survive in order.
    let base = apex_base();
    assert_eq!(second.table.splines[0], base.splines[0]);
    assert_eq!(second.table.layer_transitions[0], base.layer_transitions[0]);
    assert_eq!(second.table.lamps, base.lamps);
}

fn with(edit: impl FnOnce(&mut String)) -> Result<(), String> {
    let mut text = fixture("apex_layout.toml");
    edit(&mut text);
    let layout = LayoutManifest::from_toml_str(&text).unwrap();
    generate(&layout, &apex_base())
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[test]
fn invalid_layouts_name_the_element() {
    let e = with(|t| *t = t.replace("lamp = \"overdrive_ready\"", "lamp = \"overdrive_redy\""))
        .unwrap_err();
    assert!(
        e.starts_with("inserts[0]: ") && e.contains("overdrive_redy"),
        "{e}"
    );
    let e = with(|t| *t = t.replace("\"return_wireform\"", "\"no_such_spline\"")).unwrap_err();
    assert!(
        e.starts_with("ramps[0]: ") && e.contains("no_such_spline"),
        "{e}"
    );
    let e = with(|t| {
        t.push_str(
            "\n[[walls]]\npoints = [[100.0, 100.0], [200.0, 200.0], [200.0, 100.0], [100.0, 200.0]]\n\
             closed = true\nmaterial = \"wood\"\n",
        )
    })
    .unwrap_err();
    assert!(e.starts_with("walls[1]: ") && e.contains("crosses"), "{e}");
    let e = with(|t| *t = t.replace("width = 32.0", "width = 0.0")).unwrap_err();
    assert!(e.starts_with("lanes[0]: "), "{e}");
}

#[test]
fn cli_end_to_end() {
    let exe = env!("CARGO_BIN_EXE_pin_tablegen");
    let dir = bundle_dir("cli");
    let layout = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/apex_layout.toml");
    let base = apex_dir().join("table.toml");
    let out = std::process::Command::new(exe)
        .arg(&layout)
        .arg("--table")
        .arg(&base)
        .arg("--out")
        .arg(&dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let vfs = pin_assets::Vfs::open(&dir).unwrap();
    pin_assets::load_table(&vfs).unwrap();
    let expected = gen_apex("apex_layout.toml");
    assert!(std::fs::read(dir.join("playfield.glb")).unwrap() == expected.glb);
    assert_eq!(
        std::fs::read_to_string(dir.join("table.toml")).unwrap(),
        expected.table_toml
    );

    // An invalid layout: exit code 1, the element named, nothing written.
    let bad = dir.join("bad_layout.toml");
    std::fs::write(
        &bad,
        fixture("apex_layout.toml").replace("overdrive_ready", "nope"),
    )
    .unwrap();
    let empty = dir.join("empty_out");
    let out = std::process::Command::new(exe)
        .arg(&bad)
        .arg("--table")
        .arg(&base)
        .arg("--out")
        .arg(&empty)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("inserts[0]") && err.contains("\"nope\""),
        "{err}"
    );
    assert!(!empty.exists());
    // Usage errors: exit code 2.
    let out = std::process::Command::new(exe).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    std::fs::remove_dir_all(dir).unwrap();
}

use std::path::Path;
