//! KP-010 (#90) acceptance B: random bytes and random TOML fragments never panic the
//! parsers or the resolvers (R11); what parses re-serialises equal; resolution is
//! deterministic and independent of the order of `[[materials]]` and `[[part_styles]]`
//! (REQ-BUILD-003, REQ-BUILD-004, R3).

mod kp010_common;

use kp010_common::{LAYOUT, PARTS_TABLE, TABLES, layout, minimal_table};
use pin_core::schema::parts::{PartKind, Placement, parse_color};
use pin_core::{LayoutManifest, PhysicsDefaults, TableManifest};
use proptest::prelude::*;

/// Runs every parser and, on success, every resolver; must not panic.
fn exercise(s: &str) {
    if let Ok(t) = TableManifest::from_toml_str(s) {
        let _ = t.effective_theme();
        if let Ok(lib) = t.part_library() {
            let _ = t.resolve_parts(&lib);
        }
        let l = layout();
        let _ = t.all_parts(&l);
        let _ = t.check_physical_pairs(&l, &PhysicsDefaults::builtin());
        let text = t.to_toml_string().expect("a parsed table serialises");
        assert_eq!(TableManifest::from_toml_str(&text).unwrap(), t);
    }
    if let Ok(l) = LayoutManifest::from_toml_str(s) {
        let t = TableManifest::from_toml_str(PARTS_TABLE).unwrap();
        let _ = l.check_parts(&t);
        let text = l.to_toml_string().expect("a parsed layout serialises");
        assert_eq!(LayoutManifest::from_toml_str(&text).unwrap(), l);
    }
}

fn fragment() -> impl Strategy<Value = String> {
    let kinds: Vec<&'static str> = PartKind::ALL.iter().map(|k| k.name()).collect();
    let ids = prop::sample::select(vec![
        "post",
        "wall",
        "post_chrome",
        "lime_ring",
        "chrome",
        "rubber",
        "gummy",
        "x",
        "",
        "Bad",
        "posts[0]",
        "posts[99]",
        "top_bumper",
        "lamps:gi_left",
        "mechs:none",
        "ring_19",
    ]);
    let num = prop::sample::select(vec![
        "0.0", "-1.0", "0.05", "1.0", "12.5", "999.0", "1001.0", "5000.0", "6000.0", "nan", "inf",
        "3", "\"x\"",
    ]);
    (
        prop::sample::select(kinds),
        ids.clone(),
        ids.clone(),
        ids,
        num.clone(),
        num.clone(),
        num,
        0usize..6,
    )
        .prop_map(|(kind, a, b, c, x, y, z, which)| match which {
            0 => format!("[[parts]]\nid = \"p{a}\"\nkind = \"{kind}\"\nstyle = \"{b}\"\nposition = [{x}, {y}, {z}]\n"),
            1 => format!("[[parts]]\nid = \"q\"\nkind = \"{kind}\"\nbind = \"{a}\"\nmaterial = \"{c}\"\nrotation_deg = [{x}, {y}, {z}]\n"),
            2 => format!("[[part_styles]]\nid = \"{a}\"\nkind = \"{kind}\"\nbase = \"{b}\"\nheight = {x}\n"),
            3 => format!("[[materials]]\nid = \"{a}\"\nbase_color = \"#123456\"\nphysical = \"{b}\"\nalpha = {x}\n"),
            4 => format!("[theme]\ntitle = \"{a}\"\nmotif = \"{b}\"\npalette = [\"#000000\", \"{c}\"]\n"),
            _ => format!("[[parts]]\nid = \"r{a}\"\nkind = \"{kind}\"\nposition = [1.0, 2.0, 3.0]\noverrides = {{ diameter = {x}, ring_size = \"{c}\", material = \"{b}\" }}\n"),
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// R11: random bytes.
    #[test]
    fn req_build_005_random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        exercise(&String::from_utf8_lossy(&bytes));
    }

    /// R11: random fragments of the new sections appended to a valid table.
    #[test]
    fn req_build_003_random_fragments_never_panic(frags in proptest::collection::vec(fragment(), 0..8)) {
        let mut src = minimal_table("");
        src.push_str("[[lamps]]\nid = \"gi_left\"\nnumber = 1\nkind = \"gi\"\n\n[[mechs]]\nid = \"top_bumper\"\nkind = \"bumper\"\nposition = [1.0, 1.0, 0.0]\n\n");
        for f in &frags {
            src.push_str(f);
            src.push('\n');
        }
        exercise(&src);
    }

    /// R11: line-level mutations of the A.5 example, the A.3 layout and the shipped tables.
    #[test]
    fn req_build_005_mutated_examples_never_panic(
        which in 0usize..6,
        line in any::<usize>(),
        junk in prop::sample::select(vec!["", "x = 1", "[[parts]]", "[theme]", "kind = \"post\"", "position = [1.0]", "= =", "id = \"post\""]),
    ) {
        let src = match which {
            0 => PARTS_TABLE,
            1 => LAYOUT,
            n => TABLES[n - 2].1,
        };
        let mut lines: Vec<&str> = src.lines().collect();
        let i = line % (lines.len() + 1);
        if i < lines.len() && junk.is_empty() {
            lines.remove(i);
        } else {
            lines.insert(i.min(lines.len()), junk);
        }
        exercise(&lines.join("\n"));
    }

    /// R3: the library and every resolved part are independent of the order of
    /// `[[materials]]` and `[[part_styles]]`, and resolving twice gives the same result.
    #[test]
    fn req_build_004_resolution_order_independent(seed in any::<u64>()) {
        let t = TableManifest::from_toml_str(PARTS_TABLE).unwrap();
        let mut u = t.clone();
        let n = u.materials.len();
        u.materials.rotate_left((seed as usize) % n);
        if seed & 1 == 1 {
            u.part_styles.reverse();
        }
        let l = layout();
        let a = t.all_parts(&l).unwrap();
        prop_assert_eq!(&a, &t.all_parts(&l).unwrap());
        prop_assert_eq!(&a, &u.all_parts(&l).unwrap());
        prop_assert_eq!(t.part_library().unwrap(), u.part_library().unwrap());
    }

    /// REQ-BUILD-002: rotation matrices are orthonormal with determinant 1 for every angle in
    /// range, and positions translate exactly.
    #[test]
    fn req_build_002_rotation_is_proper(
        rx in -360.0f32..=360.0, ry in -360.0f32..=360.0, rz in -360.0f32..=360.0,
        px in -5000.0f32..=5000.0, py in -5000.0f32..=5000.0, pz in -5000.0f32..=5000.0,
    ) {
        let p = Placement { position: [px, py, pz], rotation_deg: [rx, ry, rz] };
        let r = p.rotation_matrix();
        for i in 0..3 {
            for j in 0..3 {
                let d: f64 = (0..3).map(|k| r[i][k] * r[j][k]).sum();
                let want = if i == j { 1.0 } else { 0.0 };
                prop_assert!((d - want).abs() < 1e-12, "{} {} {}", i, j, d);
            }
        }
        let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1])
            - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
            + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
        prop_assert!((det - 1.0).abs() < 1e-12);
        prop_assert_eq!(p.to_playfield([0.0; 3]), [f64::from(px), f64::from(py), f64::from(pz)]);
    }

    /// REQ-BUILD-004: `parse_color` accepts exactly `#` plus six hex digits.
    #[test]
    fn req_build_004_parse_color_round_trips(rgb in any::<[u8; 3]>(), s in ".{0,9}") {
        let text = format!("#{:02x}{:02X}{:02x}", rgb[0], rgb[1], rgb[2]);
        prop_assert_eq!(parse_color(&text), Some(rgb));
        let ok = s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit());
        prop_assert_eq!(parse_color(&s).is_some(), ok);
    }
}
