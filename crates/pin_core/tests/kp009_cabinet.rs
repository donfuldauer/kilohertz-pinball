//! KP-009 (#89) acceptance tests: `cabinet.toml` schema, presets (SPEC Appendix E),
//! playfield attitude and validation (SPEC Appendix A.4).
//!
//! REQ-BUILD-002, REQ-BUILD-003, REQ-BUILD-005, REQ-CAB-003, REQ-CAB-004, REQ-CAB-005,
//! REQ-CAB-007, REQ-CAB-008.

use pin_core::schema::SchemaError;
use pin_core::schema::cabinet::{
    Attitude, CABINET_PRESETS_TOML, CabinetGeometry, CabinetManifest, CrossSectionShape,
    DEFAULT_PRESET, Finish, LegSurface, ScoreDisplayKind, TableEnvelope, playfield_attitude,
    preset_ids,
};
use proptest::prelude::*;

const FIXTURE: &str = include_str!("fixtures/cabinet.toml");
const PRESETS: [&str; 3] = ["classic_standard", "classic_widebody", "modern_standard"];
/// Appendix E playfield sizes (width, length), mm, per preset.
const PLAYFIELDS: [(f32, f32); 3] = [(514.0, 1070.0), (584.0, 1070.0), (514.0, 1070.0)];

fn preset(id: &str) -> CabinetGeometry {
    CabinetGeometry::preset(id).unwrap()
}

fn envelope(i: usize, pitch: Option<f32>) -> TableEnvelope {
    TableEnvelope {
        playfield_width_mm: PLAYFIELDS[i].0,
        playfield_length_mm: PLAYFIELDS[i].1,
        glass_height_mm: 112.0,
        max_part_height_mm: 65.0,
        pitch_deg: pitch,
    }
}

/// Front-rear spacing `Sy` from the leg corner insets (mean centrelines), mm,
/// computed independently of the attitude function.
fn sy(g: &CabinetGeometry) -> f64 {
    let l = &g.legs;
    let len = f64::from(g.body.length);
    let front =
        (f64::from(l.front_left.corner_inset[1]) + f64::from(l.front_right.corner_inset[1])) / 2.0;
    let rear = len
        - (f64::from(l.rear_left.corner_inset[1]) + f64::from(l.rear_right.corner_inset[1])) / 2.0;
    rear - front
}

/// Left-right spacing `Sx`, mm.
fn sx(g: &CabinetGeometry) -> f64 {
    let l = &g.legs;
    let w = f64::from(g.body.width);
    let left =
        (f64::from(l.front_left.corner_inset[0]) + f64::from(l.rear_left.corner_inset[0])) / 2.0;
    let right =
        (f64::from(l.front_right.corner_inset[0]) + f64::from(l.rear_right.corner_inset[0])) / 2.0;
    w - left - right
}

fn invalid_msg(r: Result<(), SchemaError>) -> String {
    match r {
        Err(SchemaError::Invalid(m)) => m,
        other => panic!("expected SchemaError::Invalid, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// A. Each preset with default levelers: 6.5° and level.
// ---------------------------------------------------------------------------

/// REQ-CAB-008, Appendix E: default levelers give 6.5° ± 0.01° and 0 roll.
#[test]
fn req_cab_008_presets_give_6_5_deg_and_level_appendix_e() {
    for id in PRESETS {
        let a = playfield_attitude(&preset(id));
        assert!(
            (a.pitch_deg() - 6.5).abs() <= 0.01,
            "{id}: pitch {}",
            a.pitch_deg()
        );
        assert!(a.roll_rad.abs() < 1e-12, "{id}: roll {}", a.roll_rad);
    }
}

// ---------------------------------------------------------------------------
// B. Levelers steepen and roll analytically.
// ---------------------------------------------------------------------------

/// REQ-CAB-008, REQ-CAB-007: +10 mm on both rear levelers adds atan(10/Sy); +10 mm on one
/// side rolls by ∓atan(10/Sx) with the pitch unchanged; one leg gives the half-angle
/// (mean model).
#[test]
fn req_cab_007_req_cab_008_levelers_give_analytic_pitch_and_roll() {
    for id in PRESETS {
        let g = preset(id);
        let base = playfield_attitude(&g);
        let (sy, sx) = (sy(&g), sx(&g));

        let mut rear = g.clone();
        rear.legs.rear_left.leveler.extension += 10.0;
        rear.legs.rear_right.leveler.extension += 10.0;
        let a = playfield_attitude(&rear);
        let want = libm::atan2(10.0, sy);
        assert!(
            (a.pitch_rad - base.pitch_rad - want).abs() < 1e-4,
            "{id} rear"
        );
        assert!(a.roll_rad.abs() < 1e-12);

        let mut left = g.clone();
        left.legs.front_left.leveler.extension += 10.0;
        left.legs.rear_left.leveler.extension += 10.0;
        let a = playfield_attitude(&left);
        assert!(
            (a.roll_rad + libm::atan2(10.0, sx)).abs() < 1e-4,
            "{id} left"
        );
        assert!(
            (a.pitch_rad - base.pitch_rad).abs() < 1e-12,
            "{id} left pitch"
        );

        let mut right = g.clone();
        right.legs.front_right.leveler.extension += 10.0;
        right.legs.rear_right.leveler.extension += 10.0;
        let a = playfield_attitude(&right);
        assert!(
            (a.roll_rad - libm::atan2(10.0, sx)).abs() < 1e-4,
            "{id} right"
        );
        assert!(
            (a.pitch_rad - base.pitch_rad).abs() < 1e-12,
            "{id} right pitch"
        );

        let mut one = g.clone();
        one.legs.rear_right.leveler.extension += 10.0;
        let a = playfield_attitude(&one);
        assert!((a.pitch_rad - base.pitch_rad - libm::atan2(5.0, sy)).abs() < 1e-4);
        assert!((a.roll_rad - libm::atan2(5.0, sx)).abs() < 1e-4);

        // The mount angle adds directly (REQ-BUILD-002).
        let mut m = g.clone();
        m.mount.pitch_deg += 1.0;
        m.mount.roll_deg = -0.5;
        let a = playfield_attitude(&m);
        assert!((a.pitch_deg() - 7.5).abs() < 1e-9);
        assert!((a.roll_deg() + 0.5).abs() < 1e-9);
    }
}

/// Measured numbers for the report (REQ-CAB-008): printed with `--nocapture`.
#[test]
fn req_cab_008_report_numbers() {
    for id in PRESETS {
        let g = preset(id);
        let base = playfield_attitude(&g);
        let mut rear = g.clone();
        rear.legs.rear_left.leveler.extension += 10.0;
        rear.legs.rear_right.leveler.extension += 10.0;
        let d = playfield_attitude(&rear).pitch_rad - base.pitch_rad;
        println!(
            "{id}: pitch {:.6} deg, roll {:e} rad, Sy {} mm, Sx {} mm, +10 mm rear delta {:.9} rad, atan(10/Sy) {:.9} rad",
            base.pitch_deg(),
            base.roll_rad,
            sy(&g),
            sx(&g),
            d,
            libm::atan2(10.0, sy(&g))
        );
    }
}

// ---------------------------------------------------------------------------
// C. Validation: rejections name the field; the accept side.
// ---------------------------------------------------------------------------

/// REQ-CAB-005: a cabinet narrower than the playfield is rejected naming `body.width`.
#[test]
fn req_cab_005_narrow_cabinet_names_body_width() {
    let g = preset("classic_standard");
    let mut t = envelope(0, None);
    t.playfield_width_mm = 584.0; // a widebody playfield
    let m = invalid_msg(g.validate(&t));
    assert!(m.contains("body.width"), "{m}");
    println!("{m}");
}

/// REQ-CAB-005: glass below the tallest ramp is rejected naming `glass.height`.
#[test]
fn req_cab_005_low_glass_names_glass_height() {
    let g = preset("classic_standard");
    let mut t = envelope(0, None);
    t.glass_height_mm = 60.0;
    t.max_part_height_mm = 65.0;
    let m = invalid_msg(g.validate(&t));
    assert!(m.contains("glass.height"), "{m}");
    println!("{m}");
}

/// REQ-CAB-008: `meta.pitch_deg` 0.1° off is rejected naming it; 0.04° off, exactly the
/// derived value and `None` are accepted.
#[test]
fn req_cab_008_pitch_deg_agreement_0_05() {
    for (i, id) in PRESETS.iter().enumerate() {
        let mut g = preset(id);
        g.legs.rear_left.leveler.extension = 7.0 + g.legs.rear_left.leveler.extension_min;
        let derived = playfield_attitude(&g).pitch_deg();
        for off in [0.1, -0.1] {
            let m = invalid_msg(g.validate(&envelope(i, Some((derived + off) as f32))));
            assert!(m.contains("meta.pitch_deg"), "{m}");
            assert!(m.contains(&format!("{:.4}", derived)), "shows derived: {m}");
            if i == 0 && off > 0.0 {
                println!("{m}");
            }
        }
        for off in [0.04, -0.04, 0.0] {
            assert_eq!(
                g.validate(&envelope(i, Some((derived + off) as f32))),
                Ok(())
            );
        }
        assert_eq!(g.validate(&envelope(i, None)), Ok(()));
    }
}

/// REQ-CAB-005: a backbox overlapping the playfield is rejected naming its field.
#[test]
fn req_cab_005_backbox_overlap_names_backbox_front_y() {
    // A body whose side walls fall toward the rear, so the glass can rise above the
    // backbox bottom while staying under the walls.
    let mut g = preset("classic_standard");
    g.body.front_height = 900.0;
    g.body.rear_height = 450.0;
    g.backbox.bottom_z = 450.0;
    g.backbox.front_y = 900.0;
    g.mount.height = 283.0;
    // Glass top at the rear edge ≈ 283 + 1070 sin 6.5° + 116.8 cos 6.5° ≈ 520 mm > 450.
    let m = invalid_msg(g.validate(&envelope(0, None)));
    assert!(
        m.contains("backbox.front_y") && m.contains("backbox.bottom_z"),
        "{m}"
    );
    println!("{m}");
    // Behind the playfield's rear edge (30 + 1070 cos 6.5° ≈ 1093 mm) it is clear.
    g.backbox.front_y = 1100.0;
    assert_eq!(g.validate(&envelope(0, None)), Ok(()));
    // Above the glass it is clear too.
    g.backbox.front_y = 900.0;
    g.backbox.bottom_z = 530.0;
    assert_eq!(g.validate(&envelope(0, None)), Ok(()));
}

/// The reported rolled configuration (#163 gate round 9): classic_standard with
/// `mount.pitch_deg` 0, `mount.roll_deg` `roll` and `mount.height` 80.
fn rolled(roll: f32) -> CabinetGeometry {
    let mut g = preset("classic_standard");
    g.mount.pitch_deg = 0.0;
    g.mount.roll_deg = roll;
    g.mount.height = 80.0;
    g
}

/// REQ-CAB-005, REQ-CAB-008, REQ-BUILD-002: roll turns the playfield about its front-left
/// origin, so at 45° the right surface corner alone reaches 80 + 514 sin 45° ≈ 443.5 mm,
/// above the 404.7 mm front wall top: rejected naming `mount.height` and the right side.
#[test]
fn req_cab_005_req_cab_008_rolled_right_corner_rejected() {
    let m = invalid_msg(rolled(45.0).validate(&envelope(0, None)));
    assert!(m.contains("mount.height"), "{m}");
    assert!(
        m.contains("front edge") && m.contains("right corner"),
        "{m}"
    );
    println!("{m}");
}

/// REQ-CAB-005, REQ-CAB-008: the same mount at roll 0 fits; a small roll that still clears
/// fits; a negative roll lowers the right side and is not wrongly rejected.
#[test]
fn req_cab_005_req_cab_008_roll_zero_small_and_negative_pass() {
    for roll in [0.0, 2.0, -2.0, -45.0] {
        assert_eq!(
            rolled(roll).validate(&envelope(0, None)),
            Ok(()),
            "roll {roll}"
        );
    }
}

/// REQ-CAB-005: a rolled playfield's raised right corner is the highest glass top for
/// the backbox check, so a backbox that clears the level playfield overlaps the rolled one.
#[test]
fn req_cab_005_rolled_backbox_overlap_rejected() {
    let mut g = preset("classic_standard");
    g.body.front_height = 900.0;
    // Side wall top at the rear edge ≈ 550 mm, above every glass corner below.
    g.body.rear_height = 480.0;
    g.backbox.front_y = 900.0;
    g.backbox.bottom_z = 530.0;
    g.mount.height = 283.0;
    // Level: rear glass top ≈ 520 mm < 530 mm, clear.
    assert_eq!(g.validate(&envelope(0, None)), Ok(()));
    // Rolled 2°: the rear-right glass top rises by 514 sin 2° ≈ 17.9 mm to ≈ 538 mm.
    g.mount.roll_deg = 2.0;
    let m = invalid_msg(g.validate(&envelope(0, None)));
    assert!(
        m.contains("backbox.front_y") && m.contains("backbox.bottom_z"),
        "{m}"
    );
    println!("{m}");
    // Negative roll lowers the right side: the left corners govern, clear again.
    g.mount.roll_deg = -2.0;
    assert_eq!(g.validate(&envelope(0, None)), Ok(()));
}

/// REQ-CAB-005: other containment rules name their fields.
#[test]
fn req_cab_005_other_containment_fields() {
    let g = preset("classic_standard");
    let mut t = envelope(0, None);
    t.playfield_length_mm = 1300.0;
    assert!(invalid_msg(g.validate(&t)).contains("body.length"));

    let mut off = g.clone();
    off.mount.offset_x = 10.0;
    assert!(invalid_msg(off.validate(&envelope(0, None))).contains("mount.offset_x"));

    let mut t = envelope(0, None);
    t.playfield_width_mm = 520.0;
    let mut small_glass = g.clone();
    small_glass.glass_panel.width = 515.0;
    assert!(invalid_msg(small_glass.validate(&t)).contains("glass_panel.width"));
    let mut short_glass = g.clone();
    short_glass.glass_panel.length = 1000.0;
    assert!(invalid_msg(short_glass.validate(&envelope(0, None))).contains("glass_panel.length"));

    let mut high = g.clone();
    high.mount.height = 300.0;
    assert!(invalid_msg(high.validate(&envelope(0, None))).contains("mount.height"));
    let mut t = envelope(0, None);
    t.glass_height_mm = 200.0;
    t.max_part_height_mm = 65.0;
    let mut steep_rear = g.clone();
    steep_rear.body.rear_height = 410.0;
    steep_rear.backbox.bottom_z = 800.0;
    assert!(invalid_msg(steep_rear.validate(&t)).contains("mount.height"));

    for (bad, field) in [
        (
            TableEnvelope {
                playfield_width_mm: 0.0,
                ..envelope(0, None)
            },
            "table.playfield_width_mm",
        ),
        (
            TableEnvelope {
                playfield_length_mm: f32::NAN,
                ..envelope(0, None)
            },
            "table.playfield_length_mm",
        ),
        (
            TableEnvelope {
                glass_height_mm: -1.0,
                ..envelope(0, None)
            },
            "glass.height",
        ),
        (
            TableEnvelope {
                max_part_height_mm: -1.0,
                ..envelope(0, None)
            },
            "table.max_part_height_mm",
        ),
        (
            TableEnvelope {
                pitch_deg: Some(f32::INFINITY),
                ..envelope(0, None)
            },
            "meta.pitch_deg",
        ),
    ] {
        assert!(invalid_msg(g.validate(&bad)).contains(field), "{field}");
    }
    // An inconsistent cabinet fails validation before containment.
    let mut bad = g.clone();
    bad.body.width = -1.0;
    assert!(invalid_msg(bad.validate(&envelope(0, None))).contains("body.width"));
}

/// REQ-CAB-007, REQ-CAB-005: an out-of-range leveler extension names
/// `legs.<leg>.leveler.extension`, in the file and on the resolved geometry.
#[test]
fn req_cab_007_leveler_extension_out_of_range_named() {
    let src = "schema_version = 1\npreset = \"modern_standard\"\n[legs.front_right.leveler]\nextension = 10.0\n";
    match CabinetManifest::from_toml_str(src) {
        Err(SchemaError::Invalid(m)) => {
            assert!(m.contains("legs.front_right.leveler.extension"), "{m}")
        }
        other => panic!("{other:?}"),
    }
    let mut g = preset("classic_standard");
    g.legs.rear_left.leveler.extension = 44.5;
    let m = invalid_msg(g.validate(&envelope(0, None)));
    assert!(m.contains("legs.rear_left.leveler.extension"), "{m}");
}

// ---------------------------------------------------------------------------
// D. Property tests.
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// REQ-CAB-008: attitude finite, in band, deterministic and Lipschitz over every
    /// leg's leveler range, on each preset.
    #[test]
    fn req_cab_008_attitude_finite_continuous_deterministic(
        which in 0usize..3,
        t in proptest::array::uniform4(0.0f32..=1.0),
        leg in 0usize..4,
        delta in -5.0f32..5.0,
    ) {
        let mut g = preset(PRESETS[which]);
        {
            let legs = [
                &mut g.legs.front_left, &mut g.legs.front_right,
                &mut g.legs.rear_left, &mut g.legs.rear_right,
            ];
            for (l, t) in legs.into_iter().zip(t) {
                let lv = &mut l.leveler;
                lv.extension = lv.extension_min + (lv.extension_max - lv.extension_min) * t;
            }
        }
        let a = playfield_attitude(&g);
        let b = playfield_attitude(&g);
        prop_assert_eq!(a.pitch_rad.to_bits(), b.pitch_rad.to_bits());
        prop_assert_eq!(a.roll_rad.to_bits(), b.roll_rad.to_bits());
        prop_assert!(a.pitch_rad.is_finite() && a.roll_rad.is_finite());
        prop_assert!(a.pitch_deg() > 4.0 && a.pitch_deg() < 9.0, "{}", a.pitch_deg());
        prop_assert!(a.roll_deg().abs() < 6.0, "{}", a.roll_deg());

        let mut p = g.clone();
        let target = match leg {
            0 => &mut p.legs.front_left,
            1 => &mut p.legs.front_right,
            2 => &mut p.legs.rear_left,
            _ => &mut p.legs.rear_right,
        };
        let before = target.leveler.extension;
        target.leveler.extension = before + delta;
        let real_delta = f64::from(target.leveler.extension) - f64::from(before);
        let c = playfield_attitude(&p);
        let min_spacing = if sx(&g) < sy(&g) { sx(&g) } else { sy(&g) };
        let bound = real_delta.abs() / min_spacing * 1.01 + 1e-15;
        prop_assert!((c.pitch_rad - a.pitch_rad).abs() <= bound);
        prop_assert!((c.roll_rad - a.roll_rad).abs() <= bound);
    }

    /// REQ-CAB-003 (R11): random bytes never panic the parser or the resolver.
    #[test]
    fn req_cab_003_random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let s = String::from_utf8_lossy(&bytes);
        if let Ok(m) = CabinetManifest::from_toml_str(&s) {
            let _ = m.resolve();
        }
    }

    /// REQ-CAB-003 (R11): mutated and truncated fixtures never panic.
    #[test]
    fn req_cab_003_mutated_fixture_never_panics(
        pos in any::<prop::sample::Index>(),
        len in 0usize..40,
        insert in "\\PC{0,20}",
        flips in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..8),
        cut in any::<prop::sample::Index>(),
    ) {
        let mut chars: Vec<char> = FIXTURE.chars().collect();
        let at = pos.index(chars.len());
        let end = (at + len).min(chars.len());
        chars.splice(at..end, insert.chars());
        let s: String = chars.into_iter().collect();
        let mut bytes = s.into_bytes();
        for (i, b) in flips {
            if !bytes.is_empty() {
                let k = i.index(bytes.len());
                bytes[k] ^= b;
            }
        }
        let k = cut.index(bytes.len() + 1);
        let s = String::from_utf8_lossy(&bytes[..k]);
        if let Ok(m) = CabinetManifest::from_toml_str(&s) {
            let _ = m.resolve();
        }
        let t = &FIXTURE[..cut.index(FIXTURE.len() + 1)];
        if let Ok(m) = CabinetManifest::from_toml_str(t) {
            let _ = m.resolve();
        }
    }
}

// ---------------------------------------------------------------------------
// E. Schema tests.
// ---------------------------------------------------------------------------

fn parse_err(src: &str) -> SchemaError {
    CabinetManifest::from_toml_str(src).unwrap_err()
}

/// REQ-BUILD-005: unknown keys are errors at the top level, in a nested table and in a
/// per-leg table.
#[test]
fn req_build_005_unknown_keys_rejected() {
    for src in [
        "schema_version = 1\npreset = \"classic_standard\"\ncolour = 1\n",
        "schema_version = 1\npreset = \"classic_standard\"\n[backbox.score_display]\nfoo = 1\n",
        "schema_version = 1\npreset = \"classic_standard\"\n[legs.rear_left]\nwheels = 4\n",
        "schema_version = 1\npreset = \"classic_standard\"\n[legs.rear_left.leveler]\nspin = 1.0\n",
        "schema_version = 1\npreset = \"classic_standard\"\n[legs.middle]\nlength = 1.0\n",
    ] {
        assert!(matches!(parse_err(src), SchemaError::Parse(_)), "{src}");
    }
}

/// REQ-BUILD-005: wrong schema_version; non-finite floats named; unknown preset named.
#[test]
fn req_build_005_version_finite_and_preset_errors() {
    assert_eq!(
        parse_err("schema_version = 2\npreset = \"classic_standard\"\n"),
        SchemaError::UnsupportedVersion(2)
    );
    assert!(matches!(
        parse_err("preset = \"classic_standard\"\n"),
        SchemaError::Parse(_)
    ));
    match parse_err("schema_version = 1\npreset = \"classic_standard\"\n[mount]\npitch_deg = nan\n")
    {
        SchemaError::Invalid(m) => assert!(m.contains("mount.pitch_deg"), "{m}"),
        e => panic!("{e:?}"),
    }
    match parse_err(
        "schema_version = 1\npreset = \"classic_standard\"\n[legs.rear_right.leveler]\nextension = inf\n",
    ) {
        SchemaError::Invalid(m) => assert!(m.contains("legs.rear_right.leveler.extension"), "{m}"),
        e => panic!("{e:?}"),
    }
    match parse_err("schema_version = 1\npreset = \"tiny_cabinet\"\n") {
        SchemaError::Invalid(m) => assert!(m.contains("`preset`"), "{m}"),
        e => panic!("{e:?}"),
    }
    assert!(matches!(
        CabinetGeometry::preset("nope"),
        Err(SchemaError::Invalid(m)) if m.contains("preset")
    ));
}

/// REQ-CAB-004: `preset` alone resolves to exactly the preset; the default preset is
/// `classic_standard`.
#[test]
fn req_cab_004_preset_only_resolves_to_preset() {
    for id in PRESETS {
        let m = CabinetManifest::from_toml_str(&format!("schema_version = 1\npreset = \"{id}\"\n"))
            .unwrap();
        assert_eq!(m.resolve().unwrap(), preset(id));
    }
    assert_eq!(DEFAULT_PRESET, "classic_standard");
    assert_eq!(
        CabinetGeometry::default_preset().unwrap(),
        preset("classic_standard")
    );
}

/// REQ-CAB-004, REQ-BUILD-003: a single-field override changes that field only.
#[test]
fn req_cab_004_single_field_override() {
    let m = CabinetManifest::from_toml_str(
        "schema_version = 1\npreset = \"classic_standard\"\n[legs.rear_left.leveler]\nextension = 10.0\n",
    )
    .unwrap();
    let got = m.resolve().unwrap();
    let mut want = preset("classic_standard");
    assert_ne!(got, want);
    want.legs.rear_left.leveler.extension = 10.0;
    assert_eq!(got, want);
}

/// REQ-CAB-007, REQ-BUILD-003: `[legs.default]` sets all four legs, a per-leg table beats it.
#[test]
fn req_cab_007_leg_default_and_per_leg() {
    let m = CabinetManifest::from_toml_str(
        "schema_version = 1\npreset = \"classic_widebody\"\n\
         [legs.default]\nlength = 730.0\n[legs.default.section]\nshape = \"round\"\n\
         [legs.front_left]\nlength = 740.0\n",
    )
    .unwrap();
    let g = m.resolve().unwrap();
    assert_eq!(g.legs.front_left.length, 740.0);
    for (name, l) in g.legs.all() {
        assert_eq!(l.section.shape, CrossSectionShape::Round, "{name}");
        assert_eq!(l.section.top, 76.0, "{name}");
        if name != "front_left" {
            assert_eq!(l.length, 730.0, "{name}");
        }
    }
}

/// REQ-CAB-004: without `preset` every field is needed; the first missing one is named.
/// The fully stated form of a preset resolves to that preset.
#[test]
fn req_cab_004_no_preset_needs_every_field() {
    match parse_err("schema_version = 1\n[body]\nwidth = 560.0\n") {
        SchemaError::Invalid(m) => assert!(m.contains("`body.length`"), "{m}"),
        e => panic!("{e:?}"),
    }
    // A complete file without `preset`: the preset's own table, re-rooted.
    let table: toml::Table = toml::from_str(CABINET_PRESETS_TOML).unwrap();
    let modern = table["presets"]["modern_standard"]
        .as_table()
        .unwrap()
        .clone();
    let mut file = toml::Table::new();
    file.insert("schema_version".into(), toml::Value::Integer(1));
    file.extend(modern);
    let m = CabinetManifest::from_toml_str(&toml::to_string(&file).unwrap()).unwrap();
    assert_eq!(m.preset, None);
    assert_eq!(m.resolve().unwrap(), preset("modern_standard"));
}

/// REQ-BUILD-005: write then read gives an equal manifest; the writer refuses what the
/// reader would refuse.
#[test]
fn req_build_005_round_trip() {
    let m = CabinetManifest::from_toml_str(FIXTURE).unwrap();
    let text = m.to_toml_string().unwrap();
    assert_eq!(CabinetManifest::from_toml_str(&text).unwrap(), m);
    let full = m.resolve().unwrap();
    assert!(!m.overrides().art.unwrap().is_empty());
    assert_eq!(full.art["cabinet.side_left"], "art/side_left.png");
    let mut bad = m.clone();
    bad.preset = Some("nope".into());
    assert!(bad.to_toml_string().is_err());
    let mut bad = m;
    bad.schema_version = 3;
    assert_eq!(
        bad.to_toml_string(),
        Err(SchemaError::UnsupportedVersion(3))
    );
}

// ---------------------------------------------------------------------------
// F. Preset data against Appendix E.
// ---------------------------------------------------------------------------

struct Row {
    width: f32,
    length: f32,
    heights: (f32, f32),
    leg: f32,
    finish: Finish,
    surface: LegSurface,
    black: bool,
    ext: (f32, f32, f32),
    lockdown: f64,
    button: f64,
    backbox: (f32, f32, f32),
    backbox_top: f64,
    backglass: (f32, f32),
    translite: (f32, f32),
    display: (ScoreDisplayKind, f32, f32),
    glass: (f32, f32, f32),
}

/// Appendix E, verbatim (inches converted: 3 in = 76 mm).
fn appendix_e() -> [Row; 3] {
    [
        Row {
            width: 560.0,
            length: 1315.0,
            heights: (400.0, 605.0),
            leg: 724.0,
            finish: Finish::Chrome,
            surface: LegSurface::Ribbed,
            black: false,
            ext: (0.0, 0.0, 44.0),
            lockdown: 910.0,
            button: 860.0,
            backbox: (730.0, 730.0, 250.0),
            backbox_top: 1930.0,
            backglass: (686.0, 479.0),
            translite: (656.0, 449.0),
            display: (ScoreDisplayKind::Dmd, 320.0, 80.0),
            glass: (533.0, 1092.0, 4.8),
        },
        Row {
            width: 630.0,
            length: 1315.0,
            heights: (400.0, 605.0),
            leg: 724.0,
            finish: Finish::PowderCoat,
            surface: LegSurface::Plain,
            black: true,
            ext: (0.0, 0.0, 44.0),
            lockdown: 910.0,
            button: 860.0,
            backbox: (730.0, 730.0, 250.0),
            backbox_top: 1930.0,
            backglass: (686.0, 479.0),
            translite: (656.0, 449.0),
            display: (ScoreDisplayKind::Dmd, 320.0, 80.0),
            glass: (603.0, 1092.0, 4.8),
        },
        Row {
            width: 559.0,
            length: 1308.0,
            heights: (413.0, 603.0),
            leg: 778.0,
            finish: Finish::Wrinkle,
            surface: LegSurface::Plain,
            black: true,
            ext: (25.0, 25.0, 69.0),
            lockdown: 914.0,
            button: 864.0,
            backbox: (705.0, 711.0, 250.0),
            backbox_top: 1911.0,
            backglass: (658.0, 487.0),
            translite: (628.0, 457.0),
            display: (ScoreDisplayKind::Lcd, 345.0, 194.0),
            glass: (533.0, 1092.0, 4.8),
        },
    ]
}

/// Appendix E, REQ-CAB-004, REQ-CAB-007: every preset value against the table.
#[test]
fn appendix_e_preset_values_req_cab_004() {
    assert_eq!(preset_ids(), PRESETS);
    for (i, (id, e)) in PRESETS.iter().zip(appendix_e()).enumerate() {
        let g = preset(id);
        let b = &g.body;
        assert_eq!((b.width, b.length), (e.width, e.length), "{id}");
        assert_eq!((b.front_height, b.rear_height), e.heights, "{id}");
        for (name, l) in g.legs.all() {
            assert_eq!(l.length, e.leg, "{id} {name}");
            assert_eq!(l.finish, e.finish, "{id} {name}");
            assert_eq!(l.surface, e.surface, "{id} {name}");
            assert_eq!(l.colour == "#141414", e.black, "{id} {name}");
            let lv = &l.leveler;
            assert_eq!(
                (lv.extension, lv.extension_min, lv.extension_max),
                e.ext,
                "{id} {name}"
            );
            assert_eq!(lv.thread_length, 76.0, "{id}: 3 in bolt");
        }
        assert_eq!(g.lockdown_bar_top_height_mm(), e.lockdown, "{id}");
        let c = &g.controls;
        for btn in [c.left_flipper_button, c.right_flipper_button] {
            let h = g.floor_height_mm(f64::from(btn[1]), f64::from(btn[2]));
            assert_eq!(h, e.button, "{id}");
        }
        let bb = &g.backbox;
        assert_eq!((bb.width, bb.height, bb.depth), e.backbox, "{id}");
        assert_eq!(g.backbox_top_height_mm(), e.backbox_top, "{id}");
        assert_eq!(
            (bb.backglass.width, bb.backglass.height),
            e.backglass,
            "{id}"
        );
        assert_eq!(
            (bb.translite_opening.width, bb.translite_opening.height),
            e.translite,
            "{id}"
        );
        let s = &bb.score_display;
        assert_eq!((s.kind, s.width, s.height), e.display, "{id}");
        if s.kind == ScoreDisplayKind::Dmd {
            assert_eq!(s.pixels, [128, 32], "{id}");
        } else {
            // 16:9 at 15.6 in diagonal.
            assert_eq!(s.pixels[0] * 9, s.pixels[1] * 16);
            let diag = libm::hypot(f64::from(s.width), f64::from(s.height)) / 25.4;
            assert!((diag - 15.6).abs() < 0.05, "{diag}");
        }
        let gp = &g.glass_panel;
        assert_eq!((gp.width, gp.length, gp.thickness), e.glass, "{id}");
        if *id == "modern_standard" {
            assert_eq!(bb.backglass.thickness, 3.2);
        }

        // 6.5° and level; contains its own playfield.
        let a = playfield_attitude(&g);
        assert!(
            (a.pitch_deg() - 6.5).abs() < 1e-9 && a.roll_rad == 0.0,
            "{id}"
        );
        assert_eq!(g.validate(&envelope(i, Some(6.5))), Ok(()), "{id}");
        // Default levelers are the same on all four legs.
        let lv = &g.legs.front_left.leveler;
        assert!(g.legs.all().iter().all(|(_, l)| l.leveler == *lv), "{id}");
    }
}

// ---------------------------------------------------------------------------
// G. Fixture (Appendix A.4).
// ---------------------------------------------------------------------------

/// Appendix A.4, REQ-BUILD-005: the fixture is the SPEC example verbatim, parses,
/// resolves and validates; its non-default rear legs give the analytic pitch.
#[test]
fn appendix_a4_fixture_is_the_spec_example_and_validates() {
    let spec = include_str!("../../../docs/SPEC.md");
    assert!(spec.contains("### A.4 `cabinet.toml`"));
    assert!(
        spec.contains(FIXTURE),
        "fixture differs from SPEC.md Appendix A.4"
    );
    let m = CabinetManifest::from_toml_str(FIXTURE).unwrap();
    assert_eq!(m.preset.as_deref(), Some("classic_standard"));
    let g = m.resolve().unwrap();
    assert_eq!(g.body.front_height, 405.0);
    assert_eq!(g.legs.rear_left.leveler.extension, 12.0);
    assert_eq!(g.legs.rear_right.length, 736.0);
    assert_eq!(g.legs.front_left.finish, Finish::PowderCoat);
    assert_eq!(g.trim.rail_colour, "#101010");
    let a: Attitude = playfield_attitude(&g);
    let want = 6.0 + libm::atan2(12.0, sy(&g)).to_degrees();
    assert!((a.pitch_deg() - want).abs() < 1e-9);
    assert!(a.roll_rad.abs() < 1e-12);
    assert_eq!(g.validate(&envelope(0, Some(a.pitch_deg() as f32))), Ok(()));
    assert!(g.validate(&envelope(0, Some(6.5))).is_err());
}
