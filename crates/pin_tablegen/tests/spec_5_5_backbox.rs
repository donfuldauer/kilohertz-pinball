//! Subphase 5.5 spec tests (Independent Tester): `pin_tablegen::backbox`
//! generates deterministic backbox geometry from an optional backbox layout
//! block; errors name the offending element; nothing panics.
//!
//! * Determinism: the same block gives byte-identical `.glb` output twice
//!   (and from re-parsed text).
//! * Errors name the offending element for a self-intersecting outline, a
//!   zero-size outline, an outline of more than 1024 points and NaN
//!   coordinates (outline and wall).
//! * Random bytes and mutated valid blocks never panic the parser or the
//!   generator.
//! * The generated geometry loads through `pin_assets::load_gltf` and every
//!   backbox collider is on collision layer 4 only.
//!
//! The backbox API is reached only through `spec_5_5_api.rs`.

#[path = "spec_5_5_api.rs"]
mod api;

use pin_assets::{Vfs, load_gltf};
use pin_core::fmath::{max_f32, min_f32};
use proptest::prelude::*;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A mini-playfield 400 × 300 mm with two walls.
const VALID: &str = r#"
[backbox]
outline = [[0.0, 0.0], [400.0, 0.0], [400.0, 300.0], [0.0, 300.0]]

[[backbox.walls]]
points = [[40.0, 40.0], [40.0, 260.0]]

[[backbox.walls]]
points = [[360.0, 40.0], [360.0, 260.0], [200.0, 280.0]]
"#;

fn block(outline: &str) -> String {
    format!("[backbox]\noutline = {outline}\n")
}

fn generate_text(text: &str) -> Result<Vec<u8>, api::GenError> {
    let l = api::parse(text)?;
    api::generate(&l)
}

#[test]
fn valid_block_generates_a_glb() {
    let glb = generate_text(VALID).unwrap_or_else(|e| panic!("{}", api::message(&e)));
    assert!(glb.len() > 20);
    assert_eq!(&glb[0..4], b"glTF", "not a binary glTF");
}

#[test]
fn generation_is_byte_identical_twice() {
    let l = api::parse(VALID).unwrap();
    let a = api::generate(&l).unwrap();
    let b = api::generate(&l).unwrap();
    assert!(a == b, "two generations of the same block differ");
    // Re-parsed text, separately.
    let c = generate_text(VALID).unwrap();
    assert!(a == c, "generation from re-parsed text differs");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// Determinism over random rectangles and wall positions.
    #[test]
    fn determinism_over_random_valid_blocks(
        w in 100.0f32..800.0,
        h in 100.0f32..600.0,
        fx in 0.1f32..0.9,
    ) {
        let x = w * fx;
        let text = format!(
            "[backbox]\noutline = [[0.0, 0.0], [{w}, 0.0], [{w}, {h}], [0.0, {h}]]\n\n\
             [[backbox.walls]]\npoints = [[{x}, {}], [{x}, {}]]\n",
            h * 0.2,
            h * 0.8
        );
        let a = generate_text(&text);
        let b = generate_text(&text);
        prop_assert!(a.is_ok(), "{}: {:?}", text, a.as_ref().err().map(api::message));
        prop_assert!(a == b);
    }
}

fn expect_error(text: &str, what: &str, element_has: &[&str]) {
    let r = generate_text(text);
    let e = match r {
        Ok(_) => panic!("{what}: accepted:\n{text}"),
        Err(e) => e,
    };
    let el = api::element(&e).to_string();
    for part in element_has {
        assert!(
            el.contains(part),
            "{what}: error element {el:?} does not name {part:?} ({})",
            api::message(&e)
        );
    }
    assert!(!api::message(&e).is_empty());
}

#[test]
fn self_intersecting_outline_is_named() {
    // A bow-tie.
    expect_error(
        &block("[[0.0, 0.0], [400.0, 300.0], [400.0, 0.0], [0.0, 300.0]]"),
        "self-intersecting outline",
        &["backbox", "outline"],
    );
}

#[test]
fn zero_size_outlines_are_named() {
    for (what, outline) in [
        (
            "all points equal",
            "[[10.0, 10.0], [10.0, 10.0], [10.0, 10.0], [10.0, 10.0]]",
        ),
        ("collinear", "[[0.0, 0.0], [100.0, 0.0], [200.0, 0.0]]"),
        (
            "zero height",
            "[[0.0, 5.0], [400.0, 5.0], [400.0, 5.0], [0.0, 5.0]]",
        ),
    ] {
        expect_error(&block(outline), what, &["backbox", "outline"]);
    }
}

#[test]
fn more_than_1024_outline_points_are_named() {
    // A convex 1025-gon (valid apart from its size).
    let n = 1025;
    let mut s = String::from("[");
    for i in 0..n {
        let a = i as f64 * std::f64::consts::TAU / n as f64;
        let _ = write!(
            s,
            "[{:.4}, {:.4}],",
            200.0 + 150.0 * libm::cos(a),
            150.0 + 140.0 * libm::sin(a)
        );
    }
    s.push(']');
    expect_error(&block(&s), "1025 points", &["backbox", "outline"]);
    // 1024 points is still fine.
    let n = 1024;
    let mut s = String::from("[");
    for i in 0..n {
        let a = i as f64 * std::f64::consts::TAU / n as f64;
        let _ = write!(
            s,
            "[{:.4}, {:.4}],",
            200.0 + 150.0 * libm::cos(a),
            150.0 + 140.0 * libm::sin(a)
        );
    }
    s.push(']');
    generate_text(&block(&s))
        .unwrap_or_else(|e| panic!("1024 points refused: {}", api::message(&e)));
}

#[test]
fn nan_coordinates_are_named() {
    expect_error(
        &block("[[0.0, 0.0], [400.0, nan], [400.0, 300.0], [0.0, 300.0]]"),
        "NaN in the outline",
        &["backbox", "outline"],
    );
    expect_error(
        &block("[[0.0, 0.0], [400.0, inf], [400.0, 300.0], [0.0, 300.0]]"),
        "infinity in the outline",
        &["backbox", "outline"],
    );
    let wall = format!(
        "{}\n[[backbox.walls]]\npoints = [[40.0, 40.0], [nan, 260.0]]\n",
        block("[[0.0, 0.0], [400.0, 0.0], [400.0, 300.0], [0.0, 300.0]]")
    );
    expect_error(&wall, "NaN in a wall", &["backbox", "walls[0]"]);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// Random bytes: parse (and generate when the parse succeeds) never
    /// panics.
    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        if let Ok(l) = api::parse_bytes(&bytes) {
            let _ = api::generate(&l);
        }
    }

    /// A valid block with random byte edits: never panics; an error names
    /// something.
    #[test]
    fn mutated_blocks_never_panic(
        edits in proptest::collection::vec((0usize..400, any::<u8>()), 1..12),
    ) {
        let mut bytes = VALID.as_bytes().to_vec();
        for (at, b) in edits {
            let i = at % bytes.len();
            bytes[i] = b;
        }
        match api::parse_bytes(&bytes) {
            Ok(l) => {
                if let Err(e) = api::generate(&l) {
                    prop_assert!(!api::element(&e).is_empty());
                }
            }
            Err(e) => prop_assert!(!api::message(&e).is_empty()),
        }
    }

    /// Random outlines (any numbers, any point count up to 40): never panic;
    /// accepted outlines generate deterministically.
    #[test]
    fn random_outlines_never_panic(
        pts in proptest::collection::vec(
            (prop_oneof![Just(f32::NAN), Just(f32::INFINITY), -1.0e7f32..1.0e7],
             prop_oneof![Just(f32::NAN), -1.0e7f32..1.0e7]),
            0..40,
        ),
    ) {
        let mut s = String::from("[");
        for (x, y) in &pts {
            let f = |v: f32| if v.is_nan() { "nan".to_string() } else if v.is_infinite() { "inf".to_string() } else { format!("{v:?}") };
            let _ = write!(s, "[{}, {}],", f(*x), f(*y));
        }
        s.push(']');
        let text = block(&s);
        let a = generate_text(&text);
        if a.is_ok() {
            prop_assert!(a == generate_text(&text));
        }
    }
}

fn temp_dir(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "spec_5_5_tablegen_{tag}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn generated_geometry_loads_with_every_collider_on_layer_4_only() {
    let glb = generate_text(VALID).unwrap();
    let dir = temp_dir("load");
    std::fs::write(dir.join("backbox.glb"), &glb).unwrap();
    let vfs = Vfs::open(&dir).unwrap();
    let scene = load_gltf(&vfs, "backbox.glb").unwrap_or_else(|e| panic!("{e}"));
    let colliders: Vec<_> = scene.meshes.iter().filter(|m| m.extras.collider).collect();
    assert!(
        !colliders.is_empty(),
        "no collider nodes in the backbox geometry"
    );
    for m in &colliders {
        assert_eq!(
            m.extras.collision_layer,
            Some(4),
            "collider {} is not on the backbox layer",
            m.name
        );
    }
    // Every node that names a collision layer names layer 4.
    for m in &scene.meshes {
        if let Some(l) = m.extras.collision_layer {
            assert_eq!(l, 4, "node {} on layer {l}", m.name);
        }
        assert!(
            m.positions.iter().flatten().all(|v| v.is_finite()),
            "{}",
            m.name
        );
        assert!(
            !m.indices.is_empty() && m.indices.len() % 3 == 0,
            "{}",
            m.name
        );
    }
    // The walls (two) and the mini-playfield surface are colliders: at
    // least three collider nodes.
    assert!(colliders.len() >= 3, "{} collider nodes", colliders.len());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn generated_geometry_spans_the_outline() {
    // The collider geometry covers the outline's extent (metres in the
    // file, mm in the block): 400 × 300 mm in the plane, so the bounding
    // box of all collider vertices has two sides of about 0.4 m and 0.3 m
    // (in whatever orientation the backbox plane takes), and the
    // generated plane is not degenerate.
    let glb = generate_text(VALID).unwrap();
    let dir = temp_dir("extent");
    std::fs::write(dir.join("backbox.glb"), &glb).unwrap();
    let vfs = Vfs::open(&dir).unwrap();
    let scene = load_gltf(&vfs, "backbox.glb").unwrap();
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for m in scene.meshes.iter().filter(|m| m.extras.collider) {
        let t = m.transform;
        for p in &m.positions {
            // Column-major node transform.
            let w =
                [0usize, 1, 2].map(|r| t[0][r] * p[0] + t[1][r] * p[1] + t[2][r] * p[2] + t[3][r]);
            for k in 0..3 {
                lo[k] = min_f32(lo[k], w[k]);
                hi[k] = max_f32(hi[k], w[k]);
            }
        }
    }
    let mut ext: Vec<f32> = (0..3).map(|k| hi[k] - lo[k]).collect();
    ext.sort_by(f32::total_cmp);
    assert!((ext[2] - 0.4).abs() < 0.02, "extents {ext:?}");
    assert!((ext[1] - 0.3).abs() < 0.02, "extents {ext:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
