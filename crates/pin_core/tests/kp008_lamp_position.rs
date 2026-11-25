//! KP-008 (#88): optional `[[lamps]]` `position` / `radius` (REQ-VIS-004) and the
//! `dynamic_meshes` slot convention (REQ-VIS-002).

use core::mem::{offset_of, size_of};
use pin_core::snapshot::{
    FrameStateSnapshot, MAX_DYNAMIC_MESHES, MAX_LAMPS, MeshTransform, NEWTON_SWING_RANGE_RAD,
};
use pin_core::{
    TableManifest,
    schema::table::{LampDef, LampKind, MechKind},
};
use proptest::prelude::*;

const TABLE: &str = include_str!("../fixtures/table.toml");

/// The fixture with `[[lamps]]` and/or `[[mechs]]` replaced by the given TOML snippets.
fn fixture_with(lamps: Option<&str>, mechs: Option<&str>) -> String {
    let mut root: toml::Table = toml::from_str(TABLE).unwrap();
    for (key, text) in [("lamps", lamps), ("mechs", mechs)] {
        if let Some(text) = text {
            let t: toml::Table = toml::from_str(text).unwrap();
            root.insert(key.into(), t[key].clone());
        }
    }
    toml::to_string(&root).unwrap()
}

const LAMPS: &str = r##"
[[lamps]]
id = "ramp_flasher"
number = 41
kind = "flasher"
position = [410.0, 720.0, 0.0]
radius = 60.0

[[lamps]]
id = "pos_only"
number = 42
kind = "insert"
color = "#ff7a00"
position = [1.5, 2.5, 3.5]

[[lamps]]
id = "neither"
number = 43
kind = "gi"
"##;

#[test]
fn req_vis_004_lamp_position_radius_round_trip() {
    let a = TableManifest::from_toml_str(&fixture_with(Some(LAMPS), None)).unwrap();
    assert_eq!(a.lamps[0].kind, LampKind::Flasher);
    assert_eq!(a.lamps[0].position, Some([410.0, 720.0, 0.0]));
    assert_eq!(a.lamps[0].radius, Some(60.0));
    assert_eq!(a.lamps[1].position, Some([1.5, 2.5, 3.5]));
    assert_eq!(a.lamps[1].radius, None);
    assert_eq!(a.lamps[2].position, None);
    assert_eq!(a.lamps[2].radius, None);
    let text = a.to_toml_string().unwrap();
    let b = TableManifest::from_toml_str(&text).unwrap();
    assert_eq!(a, b);
}

#[test]
fn req_vis_004_absent_lamp_fields_are_not_serialised() {
    let l = LampDef {
        id: "x".into(),
        number: 1,
        kind: LampKind::Insert,
        color: None,
        response: Default::default(),
        position: None,
        radius: None,
    };
    let s = toml::to_string(&l).unwrap();
    assert!(!s.contains("position") && !s.contains("radius"), "{s}");
    let l2 = LampDef {
        position: Some([1.0, 2.0, 3.0]),
        radius: Some(5.0),
        ..l
    };
    let s = toml::to_string(&l2).unwrap();
    assert!(s.contains("position") && s.contains("radius"), "{s}");
    assert_eq!(toml::from_str::<LampDef>(&s).unwrap(), l2);
}

fn collect_tables(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect_tables(&p, out);
        } else if p.file_name().is_some_and(|n| n == "table.toml") {
            out.push(p);
        }
    }
}

/// `table.toml` fixtures that are intentionally not valid manifests, as repo-relative paths,
/// each with a one-line reason. Every other `table.toml` under `tables/` and `crates/` must
/// parse, serialize and round-trip. Checked on 2026-11-25: all 23 files parse (the "gap",
/// "overflow", "trap" and "script_error" fixtures are valid manifests whose scripts or
/// geometry misbehave at run time), so the list is empty; add a path with a reason here
/// only for a fixture that is deliberately not a manifest.
const EXCLUDED: &[&str] = &[];

#[test]
fn req_vis_004_existing_table_files_parse_unchanged() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files = Vec::new();
    collect_tables(&root.join("tables"), &mut files);
    collect_tables(&root.join("crates"), &mut files);
    let rel = |f: &std::path::Path| -> String {
        f.strip_prefix(&root)
            .unwrap()
            .components()
            .map(|c| c.as_os_str().to_str().unwrap())
            .collect::<Vec<_>>()
            .join("/")
    };
    let rels: Vec<String> = files.iter().map(|f| rel(f)).collect();
    // A rename or deletion of an excluded fixture is noticed.
    for e in EXCLUDED {
        assert!(
            rels.iter().any(|r| r == e) && root.join(e).is_file(),
            "excluded fixture {e} does not exist"
        );
    }
    // The production tables are never excluded.
    let production: Vec<&String> = rels.iter().filter(|r| r.starts_with("tables/")).collect();
    assert!(production.len() >= 4, "production tables: {production:?}");
    assert!(
        production.iter().all(|r| !EXCLUDED.contains(&r.as_str())),
        "a production table is excluded"
    );
    let mut must_parse = 0;
    let mut excluded = 0;
    for (f, r) in files.iter().zip(&rels) {
        if EXCLUDED.contains(&r.as_str()) {
            excluded += 1;
            continue;
        }
        must_parse += 1;
        let text = std::fs::read_to_string(f).unwrap_or_else(|e| panic!("{r}: read: {e}"));
        let m = TableManifest::from_toml_str(&text)
            .unwrap_or_else(|e| panic!("{r}: must parse (or be listed in EXCLUDED): {e:?}"));
        assert!(
            m.lamps
                .iter()
                .all(|l| l.position.is_none() && l.radius.is_none()),
            "{r}: existing lamps gained position/radius"
        );
        let s = m
            .to_toml_string()
            .unwrap_or_else(|e| panic!("{r}: must serialize: {e:?}"));
        let back = TableManifest::from_toml_str(&s)
            .unwrap_or_else(|e| panic!("{r}: serialized text must re-parse: {e:?}"));
        assert_eq!(back, m, "{r}: round trip");
    }
    // Every file is exactly one of must-parse or excluded.
    assert_eq!(must_parse + excluded, files.len());
    assert_eq!(excluded, EXCLUDED.len());
    let fx = TableManifest::from_toml_str(TABLE).unwrap();
    assert!(!fx.lamps.is_empty());
    assert!(
        fx.lamps
            .iter()
            .all(|l| l.position.is_none() && l.radius.is_none())
    );
    let again = TableManifest::from_toml_str(&fx.to_toml_string().unwrap()).unwrap();
    assert_eq!(fx, again);
}

#[test]
fn req_vis_004_unknown_lamp_key_still_rejected() {
    let bad = LAMPS.replace("position = [410.0", "posiiton = [410.0");
    assert!(TableManifest::from_toml_str(&fixture_with(Some(&bad), None)).is_err());
    let bad = LAMPS.replace("radius = 60.0", "radius = 60.0\nglow = 1.0");
    assert!(TableManifest::from_toml_str(&fixture_with(Some(&bad), None)).is_err());
}

proptest! {
    #[test]
    fn req_vis_004_lamp_random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..600)) {
        let s = String::from_utf8_lossy(&bytes);
        let _ = TableManifest::from_toml_str(&s);
        let _ = toml::from_str::<LampDef>(&s);
    }

    #[test]
    fn req_vis_004_lamp_text_with_new_keys_never_panics(
        p in "\\PC{0,30}", r in "\\PC{0,30}",
    ) {
        let t = format!("id=\"a\"\nnumber=1\nkind=\"flasher\"\nposition={p}\nradius={r}\n");
        let _ = toml::from_str::<LampDef>(&t);
        let snippet = format!("[[lamps]]\n{t}");
        if toml::from_str::<toml::Table>(&snippet).is_ok() {
            let m = fixture_with(Some(&snippet), None);
            let _ = TableManifest::from_toml_str(&m);
        }
    }

    #[test]
    fn req_vis_004_lamp_float_round_trip_bit_exact(
        x in any::<f32>().prop_filter("finite", |v| v.is_finite()),
        y in any::<f32>().prop_filter("finite", |v| v.is_finite()),
        z in any::<f32>().prop_filter("finite", |v| v.is_finite()),
        r in any::<f32>().prop_filter("finite", |v| v.is_finite()),
    ) {
        let l = LampDef {
            id: "p".into(), number: 7, kind: LampKind::Flasher, color: None,
            response: Default::default(), position: Some([x, y, z]), radius: Some(r),
        };
        let back: LampDef = toml::from_str(&toml::to_string(&l).unwrap()).unwrap();
        let (a, b) = (l.position.unwrap(), back.position.unwrap());
        for i in 0..3 { prop_assert_eq!(a[i].to_bits(), b[i].to_bits()); }
        prop_assert_eq!(l.radius.unwrap().to_bits(), back.radius.unwrap().to_bits());
    }
}

#[test]
fn req_vis_002_snapshot_contract_unchanged_by_kp008() {
    fn assert_traits<T: Copy + Default + Send + 'static>() {}
    assert_traits::<FrameStateSnapshot>();
    // Measured on origin/main (612573b) before KP-008: 4000 bytes.
    assert!(
        size_of::<FrameStateSnapshot>() <= 4000,
        "{}",
        size_of::<FrameStateSnapshot>()
    );
    assert_eq!(MAX_DYNAMIC_MESHES, 64);
    assert_eq!(MAX_LAMPS, 256);
    // The struct is repr(Rust), so the compiler picks the memory order; the declared field
    // set and order is pinned by this exhaustive destructuring (a field added, removed or
    // renamed fails to compile) and by the field-by-field offsets being distinct.
    let offs = [
        offset_of!(FrameStateSnapshot, tick),
        offset_of!(FrameStateSnapshot, ball_count),
        offset_of!(FrameStateSnapshot, balls),
        offset_of!(FrameStateSnapshot, flipper_count),
        offset_of!(FrameStateSnapshot, flippers),
        offset_of!(FrameStateSnapshot, dynamic_mesh_count),
        offset_of!(FrameStateSnapshot, dynamic_meshes),
        offset_of!(FrameStateSnapshot, lamps),
        offset_of!(FrameStateSnapshot, gi),
    ];
    let mut sorted = offs;
    sorted.sort_unstable();
    assert!(sorted.windows(2).all(|w| w[0] < w[1]), "{offs:?}");
    let FrameStateSnapshot {
        tick: _,
        ball_count: _,
        balls: _,
        flipper_count: _,
        flippers: _,
        dynamic_mesh_count: _,
        dynamic_meshes: _,
        lamps: _,
        gi: _,
    } = FrameStateSnapshot::default();
}

const MECHS: &str = r#"
[[mechs]]
id = "m0"
kind = "bumper"
[[mechs]]
id = "m1"
kind = "sling"
[[mechs]]
id = "m2"
kind = "drop_target"
[[mechs]]
id = "m3"
kind = "spinner"
[[mechs]]
id = "m4"
kind = "vuk"
[[mechs]]
id = "m5"
kind = "plunger"
[[mechs]]
id = "m6"
kind = "kickback"
[[mechs]]
id = "m7"
kind = "diverter"
[[mechs]]
id = "m8"
kind = "newton_ball"
[[mechs]]
id = "m9"
kind = "gate"
"#;

#[test]
fn req_vis_002_slot_i_belongs_to_ith_mech() {
    let m = TableManifest::from_toml_str(&fixture_with(None, Some(MECHS))).unwrap();
    let kinds: Vec<MechKind> = m.mechs.iter().map(|d| d.kind).collect();
    assert_eq!(kinds.len(), 10);
    let mut snap = FrameStateSnapshot::default();
    for (i, _) in m.mechs.iter().enumerate() {
        assert!(snap.push_dynamic_mesh(MeshTransform {
            position_m: [i as f32, 0.0, 0.0],
            ..MeshTransform::default()
        }));
    }
    assert_eq!(snap.dynamic_meshes().len(), m.mechs.len());
    for (i, def) in m.mechs.iter().enumerate() {
        assert_eq!(def.id, format!("m{i}"));
        assert_eq!(snap.dynamic_meshes()[i].position_m[0], i as f32);
    }
    // Mechs beyond the capacity are unpublished.
    for _ in 0..MAX_DYNAMIC_MESHES {
        let _ = snap.push_dynamic_mesh(MeshTransform::default());
    }
    assert_eq!(snap.dynamic_meshes().len(), MAX_DYNAMIC_MESHES);
}

#[test]
fn req_vis_002_newton_swing_range_is_half_pi() {
    assert_eq!(NEWTON_SWING_RANGE_RAD, core::f32::consts::FRAC_PI_2);
}
