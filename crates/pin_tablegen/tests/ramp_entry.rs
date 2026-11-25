//! w7pre-ramp-entry: the generated `0 -> N` ramp entry zone starts where the
//! rails start and lies along the spline's start tangent; it never overlaps
//! an `N -> 0` transition (a generation error naming the ramp otherwise);
//! and the layer-exit lint warns about a layer that has no exit zone over
//! unsupported area, without failing generation.

use glam::Vec3;
use pin_core::Material;
use pin_core::fmath::{max_f32, min_f32, min_f64};
use pin_core::schema::layout::{Deck, Playfield, Ramp};
use pin_core::schema::table::{AabbDef, LayerTransitionDef};
use pin_core::schema::{LayoutManifest, TableManifest};
use pin_physics::world::CubicSpline;
use pin_tablegen::layout::{GENERATED_PREFIX, GeneratedTable, generate, parse_layout};
use proptest::prelude::*;
use std::path::Path;

const BASE_HEAD: &str = r##"
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

/// A base table with a layer-2 wireform `ramp` through `points` (mm) and
/// the given extra transitions.
fn base(points: &[[f32; 3]], extra: &[LayerTransitionDef]) -> TableManifest {
    let pts: Vec<String> = points
        .iter()
        .map(|p| format!("[{:?}, {:?}, {:?}]", p[0], p[1], p[2]))
        .collect();
    let text = format!(
        "{BASE_HEAD}[[splines]]\nid = \"ramp\"\nkind = \"wireform\"\nlayer = 2\ngauge = 13.0\n\
         points = [{}]\n",
        pts.join(", ")
    );
    let mut t = TableManifest::from_toml_str(&text).unwrap();
    t.layer_transitions.extend_from_slice(extra);
    t
}

fn layout(width: f32) -> LayoutManifest {
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
            entry_width: width,
        }],
        decks: vec![],
        inserts: vec![],
    }
}

fn zone(min: [f32; 3], max: [f32; 3]) -> AabbDef {
    AabbDef { min, max }
}

fn transition(id: &str, from: u8, to: u8, z: AabbDef) -> LayerTransitionDef {
    LayerTransitionDef {
        id: id.into(),
        from_layer: from,
        to_layer: to,
        zone: z,
    }
}

fn entry(g: &GeneratedTable) -> &LayerTransitionDef {
    let id = format!("{GENERATED_PREFIX}ramps[0].entry");
    g.table
        .layer_transitions
        .iter()
        .find(|t| t.id == id)
        .expect("entry transition")
}

fn contains(z: &AabbDef, p: [f32; 3]) -> bool {
    (0..3).all(|k| z.min[k] <= p[k] && p[k] <= z.max[k])
}

fn overlap(a: &AabbDef, b: &AabbDef) -> bool {
    (0..3).all(|k| a.min[k] <= b.max[k] && b.min[k] <= a.max[k])
}

/// Unit plan direction of the spline's start tangent as the loader builds
/// the spline (metres, natural cubic).
fn plan_dir(points: &[[f32; 3]]) -> [f64; 2] {
    let pts: Vec<Vec3> = points
        .iter()
        .map(|p| Vec3::new(p[0], p[1], p[2]) * 1.0e-3)
        .collect();
    let t = CubicSpline::interpolate(&pts).unwrap().tangent(0.0);
    let (x, y) = (f64::from(t.x), f64::from(t.y));
    let l = (x * x + y * y).sqrt();
    [x / l, y / l]
}

/// A 2 -> 0 fallback cube (half size `width / 4`, z −50..150) whose every
/// plan point lies `width / 2 + 1` mm or more behind the spline's start
/// along the start tangent: the most the entry zone's bounds may reach
/// back is `width / 2`, so the two never overlap.
fn tight_fallback(points: &[[f32; 3]], width: f32) -> LayerTransitionDef {
    let [dx, dy] = plan_dir(points);
    let h = f64::from(width) / 4.0;
    let back = f64::from(width) / 2.0 + 1.0 + h * (dx.abs() + dy.abs());
    let c = [
        f64::from(points[0][0]) - back * dx,
        f64::from(points[0][1]) - back * dy,
    ];
    transition(
        "fallback",
        2,
        0,
        zone(
            [(c[0] - h) as f32, (c[1] - h) as f32, -50.0],
            [(c[0] + h) as f32, (c[1] + h) as f32, 150.0],
        ),
    )
}

/// Requirement 4 (a)–(d) for one ramp spline and width.
fn check_case(points: &[[f32; 3]], width: f32) -> Result<(), String> {
    let fb = tight_fallback(points, width);
    let b = base(points, std::slice::from_ref(&fb));
    let l = layout(width);
    let g = generate(&l, &b).map_err(|e| format!("{points:?} w {width}: {e}"))?;
    let z = entry(&g).zone;
    let what = format!("{points:?} w {width}: zone {z:?}");
    if !(0..3).all(|k| z.min[k] < z.max[k]) {
        return Err(format!("(a) empty: {what}"));
    }
    if !contains(&z, points[0]) {
        return Err(format!("(b) misses the rail start: {what}"));
    }
    if overlap(&z, &fb.zone) {
        return Err(format!("(c) overlaps {:?}: {what}", fb.zone));
    }
    // The zone lies ahead of the start: its centre is ahead along the
    // tangent, and it reaches back no more than width / 2.
    let [dx, dy] = plan_dir(points);
    let along = |x: f32, y: f32| {
        (f64::from(x) - f64::from(points[0][0])) * dx
            + (f64::from(y) - f64::from(points[0][1])) * dy
    };
    let centre = along(0.5 * (z.min[0] + z.max[0]), 0.5 * (z.min[1] + z.max[1]));
    if centre <= 0.0 {
        return Err(format!("centre not ahead of the start ({centre}): {what}"));
    }
    let back = [
        along(z.min[0], z.min[1]),
        along(z.min[0], z.max[1]),
        along(z.max[0], z.min[1]),
        along(z.max[0], z.max[1]),
    ]
    .into_iter()
    .fold(f64::INFINITY, min_f64);
    if back < -(f64::from(width) / 2.0 + 1.0e-3) {
        return Err(format!("reaches {back} mm behind the start: {what}"));
    }
    let again = generate(&l, &b).unwrap();
    if again != g {
        return Err(format!("(d) not deterministic: {what}"));
    }
    Ok(())
}

/// Straight ramps along a plan heading `deg`, rising by `rise` mm over
/// 200 mm, from (500, 500, z0).
fn straight(deg: f64, z0: f32, rise: f32) -> Vec<[f32; 3]> {
    let (s, c) = libm::sincos(deg.to_radians());
    let p = |d: f64, z: f32| [(500.0 + d * c) as f32, (500.0 + d * s) as f32, z];
    vec![p(0.0, z0), p(100.0, z0 + 0.5 * rise), p(200.0, z0 + rise)]
}

/// Requirement 4: straight (axis-aligned and not), curving left and right,
/// rising, starting below and above the playfield; entry_width 20..60 mm.
#[test]
fn entry_zone_sweep() {
    let mut splines: Vec<Vec<[f32; 3]>> = Vec::new();
    for deg in [
        0.0, 30.0, 45.0, 60.0, 90.0, 135.0, 180.0, 225.0, 270.0, 300.0, 17.3,
    ] {
        splines.push(straight(deg, 0.0, 0.0));
        splines.push(straight(deg, -6.0, 40.0));
    }
    // Rising steeply from above the playfield.
    splines.push(straight(70.0, 40.0, 120.0));
    // Curving left and right (heading up the table first).
    splines.push(vec![
        [300.0, 300.0, 0.0],
        [310.0, 420.0, 15.0],
        [240.0, 520.0, 30.0],
    ]);
    splines.push(vec![
        [300.0, 300.0, 0.0],
        [290.0, 420.0, 15.0],
        [360.0, 520.0, 30.0],
    ]);
    // The grey box's wire and the unit tests' wire (a start tangent not
    // axis-aligned, falling then rising).
    splines.push(vec![
        [425.0, 595.0, -6.0],
        [425.0, 650.0, 0.0],
        [425.0, 705.0, 8.0],
    ]);
    splines.push(vec![
        [60.0, 900.0, 40.0],
        [40.0, 600.0, 55.0],
        [150.0, 260.0, 20.0],
    ]);
    let mut failures = Vec::new();
    for s in &splines {
        for w in [20.0, 27.0, 33.3, 40.0, 50.0, 60.0] {
            if let Err(e) = check_case(s, w) {
                failures.push(e);
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// Requirement 4 over random headings, pitches, starts and widths.
    #[test]
    fn entry_zone_props(
        deg in 0.0f64..360.0,
        z0 in -10.0f32..60.0,
        rise in -20.0f32..120.0,
        bend in -60.0f32..60.0,
        width in 20.0f32..60.0,
    ) {
        let mut s = straight(deg, z0, rise);
        // Bend the end sideways.
        let (sn, c) = libm::sincos(deg.to_radians());
        s[2][0] += (-sn * f64::from(bend)) as f32;
        s[2][1] += (c * f64::from(bend)) as f32;
        prop_assert!(check_case(&s, width).is_ok(), "{:?}", check_case(&s, width));
    }
}

/// The greybox pattern: heading +y, a fallback ending 1 mm behind the rail
/// start and catch zones beside the lane are all clear of the zone; the
/// zone is the lane (x 405..445) from the rails' start.
#[test]
fn axis_aligned_start_has_no_overshoot() {
    let pts = [
        [425.0, 595.0, -6.0],
        [425.0, 650.0, 0.0],
        [425.0, 705.0, 8.0],
    ];
    let extra = [
        transition(
            "fb",
            2,
            0,
            zone([375.0, 350.0, -50.0], [465.0, 594.0, 150.0]),
        ),
        transition(
            "left",
            2,
            0,
            zone([325.0, 575.0, -50.0], [404.9, 890.0, 200.0]),
        ),
        transition(
            "right",
            2,
            0,
            zone([445.1, 575.0, -50.0], [525.0, 890.0, 200.0]),
        ),
    ];
    let g = generate(&layout(40.0), &base(&pts, &extra)).unwrap();
    let z = &entry(&g).zone;
    assert_eq!(z.min[0], 405.0);
    assert_eq!(z.max[0], 445.0);
    assert_eq!(z.min[1], 595.0);
    assert!(z.max[1] > 634.0 && z.max[1] <= 635.0, "{z:?}");
    assert_eq!(z.min[2], -6.0);
    assert_eq!(z.max[2], 20.0);
}

/// Requirement 2: an overlapping 2 -> 0 zone is a generation error naming
/// the ramp and both zones; other directions may overlap.
#[test]
fn overlapping_fallback_is_an_error() {
    let pts = [
        [425.0, 595.0, -6.0],
        [425.0, 650.0, 0.0],
        [425.0, 705.0, 8.0],
    ];
    // The grey box's old data: the fallback reached y 574 and a below-floor
    // catch zone covered the lane from y 575: both overlap the new zone.
    for (id, z) in [
        (
            "over_start",
            zone([375.0, 350.0, -50.0], [465.0, 595.0, 150.0]),
        ),
        (
            "catch_below",
            zone([403.0, 575.0, -50.0], [447.0, 890.0, 9.0]),
        ),
        ("inside", zone([420.0, 610.0, 0.0], [430.0, 620.0, 10.0])),
    ] {
        let e = generate(&layout(40.0), &base(&pts, &[transition(id, 2, 0, z)])).unwrap_err();
        assert_eq!(e.element, "ramps[0]", "{e}");
        assert!(e.message.contains("entry zone"), "{e}");
        assert!(e.message.contains("tablegen:ramps[0].entry"), "{e}");
        assert!(e.message.contains(&format!("{id:?}")), "{e}");
    }
    // A zone on another layer pair is not a fallback of this ramp.
    let over = zone([375.0, 350.0, -50.0], [465.0, 700.0, 150.0]);
    for (from, to) in [(2, 1), (3, 0), (0, 2), (1, 0)] {
        generate(
            &layout(40.0),
            &base(&pts, &[transition("other", from, to, over)]),
        )
        .unwrap_or_else(|e| panic!("{from} -> {to}: {e}"));
    }
    // A stale generated zone in the base is dropped, not checked.
    let stale = transition("tablegen:ramps[0].entry", 2, 0, over);
    generate(&layout(40.0), &base(&pts, &[stale])).unwrap();
}

/// Requirement 3: degenerate widths keep their error.
#[test]
fn degenerate_widths_keep_their_error() {
    let pts = [[425.0, 595.0, -6.0], [425.0, 650.0, 0.0]];
    for w in [f32::NAN, f32::INFINITY, -1.0, 0.0, 0.05, 1.0e-9] {
        let e = generate(&layout(w), &base(&pts, &[])).unwrap_err();
        assert_eq!(e.element, "ramps[0]", "{w}: {e}");
        assert!(e.message.contains("entry_width"), "{w}: {e}");
    }
    // A start far from the origin, where 0.1 mm collapses in f32.
    let far = [[1.0e7, 1.0e7, 0.0], [1.0e7, 1.0e7 + 100.0, 0.0]];
    let e = generate(&layout(0.1), &base(&far, &[])).unwrap_err();
    assert_eq!(e.element, "ramps[0]");
    assert!(
        e.message
            .starts_with("entry_width 0.1 mm gives an empty entry zone at the spline's start"),
        "{e}"
    );
}

/// A spline that starts straight up has no plan direction for the zone.
#[test]
fn vertical_start_is_an_error() {
    let pts = [[300.0, 300.0, 0.0], [300.0, 300.0, 100.0]];
    let e = generate(&layout(40.0), &base(&pts, &[])).unwrap_err();
    assert_eq!(e.element, "ramps[0]");
    assert!(e.message.contains("starts vertically"), "{e}");
}

fn deck(layer: u8, outline: &[[f32; 2]]) -> Deck {
    Deck {
        layer,
        z: 0.1,
        outline: outline.to_vec(),
        material: Material::Wood,
    }
}

/// Requirement 5: the layer-exit lint.
#[test]
fn layer_exit_warning() {
    let pts = [
        [425.0, 595.0, -6.0],
        [425.0, 650.0, 0.0],
        [425.0, 705.0, 8.0],
    ];
    // No exit from layer 2 at all: a warning at the entry's path naming it
    // and the layer; generation succeeds.
    let g = generate(&layout(40.0), &base(&pts, &[])).unwrap();
    assert_eq!(g.warnings.len(), 1, "{:?}", g.warnings);
    let w = &g.warnings[0];
    assert_eq!(w.file, "table.toml");
    assert_eq!(w.path, "layer_transitions[0]");
    assert!(w.message.contains("\"tablegen:ramps[0].entry\""), "{w}");
    assert!(w.message.contains("layer 2"), "{w}");
    // An exit (to layer 0) over area without a layer-2 deck: no
    // warning.
    let exit = transition(
        "exit",
        2,
        0,
        zone([395.0, 875.0, 30.0], [455.0, 950.0, 140.0]),
    );
    let g = generate(&layout(40.0), &base(&pts, std::slice::from_ref(&exit))).unwrap();
    assert!(g.warnings.is_empty(), "{:?}", g.warnings);
    // The same exit entirely over a layer-2 deck: supported, so it is no
    // exit for a falling ball; the warning is back.
    let mut l = layout(40.0);
    l.decks = vec![deck(
        2,
        &[
            [390.0, 870.0],
            [460.0, 870.0],
            [460.0, 955.0],
            [390.0, 955.0],
        ],
    )];
    let g = generate(&l, &base(&pts, std::slice::from_ref(&exit))).unwrap();
    assert_eq!(g.warnings.len(), 1, "{:?}", g.warnings);
    // A deck of another layer, or one above the zone's top, is no support.
    for d in [
        deck(
            1,
            &[
                [390.0, 870.0],
                [460.0, 870.0],
                [460.0, 955.0],
                [390.0, 955.0],
            ],
        ),
        Deck {
            z: 150.0,
            ..deck(
                2,
                &[
                    [390.0, 870.0],
                    [460.0, 870.0],
                    [460.0, 955.0],
                    [390.0, 955.0],
                ],
            )
        },
    ] {
        l.decks = vec![d];
        let g = generate(&l, &base(&pts, std::slice::from_ref(&exit))).unwrap();
        assert!(g.warnings.is_empty(), "{:?}", g.warnings);
    }
    // A deck covering only part of the exit leaves unsupported area.
    l.decks = vec![deck(
        2,
        &[
            [390.0, 870.0],
            [430.0, 870.0],
            [430.0, 955.0],
            [390.0, 955.0],
        ],
    )];
    let g = generate(&l, &base(&pts, std::slice::from_ref(&exit))).unwrap();
    assert!(g.warnings.is_empty(), "{:?}", g.warnings);
    // Every transition into the layer is named; a hand-declared one too.
    let hand = transition(
        "hand_entry",
        0,
        2,
        zone([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]),
    );
    let g = generate(&layout(40.0), &base(&pts, std::slice::from_ref(&hand))).unwrap();
    let paths: Vec<&str> = g.warnings.iter().map(|w| w.path.as_str()).collect();
    assert_eq!(paths, ["layer_transitions[0]", "layer_transitions[1]"]);
    assert!(g.warnings[0].message.contains("\"hand_entry\""));
    // A loop on the layer itself and transitions to layer 0 never warn.
    let only = [
        transition("loop", 3, 3, zone([0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
        transition("down", 1, 0, zone([0.0, 0.0, 0.0], [10.0, 10.0, 10.0])),
    ];
    let mut l0 = layout(40.0);
    l0.ramps.clear();
    let g = generate(&l0, &base(&pts, &only)).unwrap();
    assert!(g.warnings.is_empty(), "{:?}", g.warnings);
}

/// A deck exactly at the exit zone's top (in mm) is support: the zone top
/// uses the same f32 conversion as the deck's mesh height. 100.2 mm is a
/// value whose f32 metres exceed its f64 metres.
#[test]
fn layer_exit_deck_at_zone_top_supports() {
    let pts = [
        [425.0, 595.0, -6.0],
        [425.0, 650.0, 0.0],
        [425.0, 705.0, 8.0],
    ];
    let exit = transition(
        "exit",
        2,
        0,
        zone([395.0, 875.0, 30.0], [455.0, 950.0, 100.2]),
    );
    let mut l = layout(40.0);
    l.decks = vec![Deck {
        z: 100.2,
        ..deck(
            2,
            &[
                [390.0, 870.0],
                [460.0, 870.0],
                [460.0, 955.0],
                [390.0, 955.0],
            ],
        )
    }];
    let g = generate(&l, &base(&pts, std::slice::from_ref(&exit))).unwrap();
    assert_eq!(g.warnings.len(), 1, "{:?}", g.warnings);
}

/// Requirement 5: warnings never fail the command line; it prints them.
#[test]
fn cli_prints_warnings_and_succeeds() {
    let dir = std::env::temp_dir().join(format!("pin_tablegen_ramp_entry_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let pts = [
        [425.0, 595.0, -6.0],
        [425.0, 650.0, 0.0],
        [425.0, 705.0, 8.0],
    ];
    let table = dir.join("base.toml");
    std::fs::write(&table, base(&pts, &[]).to_toml_string().unwrap()).unwrap();
    let lay = dir.join("layout.toml");
    std::fs::write(
        &lay,
        "schema_version = 1\n[playfield]\nwidth = 600.0\nlength = 1000.0\nwall_height = 28.0\n\
         [[ramps]]\nspline = \"ramp\"\nentry_width = 40.0\n",
    )
    .unwrap();
    let out = dir.join("out");
    let args: Vec<String> = [
        lay.to_str().unwrap(),
        "--table",
        table.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    let summary = pin_tablegen::layout::run_cli(&args).unwrap();
    assert!(summary.starts_with("wrote "), "{summary}");
    assert!(
        summary.contains("\nwarning: table.toml: layer_transitions[0]: "),
        "{summary}"
    );
    assert!(out.join("table.toml").is_file());
    // The binary too: exit 0.
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_pin_tablegen"))
        .args(&args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{o:?}");
    assert!(String::from_utf8_lossy(&o.stdout).contains("warning: "));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Requirement 5 and 7: the grey box generates with the new zone (the lane
/// from the rails' start), no overlap and no layer-exit warning.
#[test]
fn greybox_generates_without_warnings() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/greybox");
    let layout = parse_layout(&std::fs::read_to_string(src.join("layout.toml")).unwrap()).unwrap();
    let base =
        TableManifest::from_toml_str(&std::fs::read_to_string(src.join("table.toml")).unwrap())
            .unwrap();
    let g = generate(&layout, &base).unwrap();
    assert!(g.warnings.is_empty(), "{:?}", g.warnings);
    let z = &entry(&g).zone;
    assert_eq!((z.min[0], z.max[0]), (405.0, 445.0), "{z:?}");
    assert_eq!(z.min[1], 595.0, "{z:?}");
    assert!(z.max[1] > 634.0 && z.max[1] <= 635.0, "{z:?}");
    assert_eq!((z.min[2], z.max[2]), (-6.0, 20.0), "{z:?}");
    for t in g
        .table
        .layer_transitions
        .iter()
        .filter(|t| t.from_layer == 2 && t.to_layer == 0)
    {
        assert!(!overlap(z, &t.zone), "{}", t.id);
    }
}

/// The grey box generated from `tables/greybox` (no warnings checked here).
fn greybox() -> GeneratedTable {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/greybox");
    let layout = parse_layout(&std::fs::read_to_string(src.join("layout.toml")).unwrap()).unwrap();
    let base =
        TableManifest::from_toml_str(&std::fs::read_to_string(src.join("table.toml")).unwrap())
            .unwrap();
    generate(&layout, &base).unwrap()
}

/// The old below-floor catch volume under the wire (mm): x 403..447,
/// y 575..890, z -50..9.
const OLD_CATCH: AabbDef = AabbDef {
    min: [403.0, 575.0, -50.0],
    max: [447.0, 890.0, 9.0],
};

/// Number of f32 steps from `a` up to `b` (`a <= b`, same sign or zero).
fn ulps(a: f32, b: f32) -> i64 {
    let key = |v: f32| {
        let bits = i64::from(v.to_bits() as i32);
        if bits < 0 {
            i64::from(i32::MIN) - bits
        } else {
            bits
        }
    };
    key(b) - key(a)
}

/// The sample values on one axis: a regular grid over the old volume
/// (`step` mm), the entry zone's bounds and the f32 values on and around
/// them, values in the strips the old two-part split left open, and the
/// old volume's own bounds; all clipped to the old volume.
fn axis_samples(k: usize, entry: &AabbDef, step: f32, extra: &[f32]) -> Vec<f32> {
    let (lo, hi) = (OLD_CATCH.min[k], OLD_CATCH.max[k]);
    let mut v = Vec::new();
    let mut i = 0u32;
    loop {
        let x = lo + step * i as f32;
        if x > hi {
            break;
        }
        v.push(x);
        i += 1;
    }
    v.extend([lo, hi, lo.next_up(), hi.next_down()]);
    for f in [entry.min[k], entry.max[k]] {
        let mut d = f;
        let mut u = f;
        v.push(f);
        for _ in 0..3 {
            d = d.next_down();
            u = u.next_up();
            v.extend([d, u]);
        }
        v.extend([f - 0.5, f + 0.5, f - 1.0, f + 1.0, f - 0.001, f + 0.001]);
    }
    v.extend_from_slice(extra);
    v.retain(|x| (lo..=hi).contains(x));
    v.sort_by(f32::total_cmp);
    v.dedup();
    v
}

/// w7pre-catch-gaps: every point of the old below-floor catch volume is in
/// the generated entry zone or in a 2 -> 0 zone, no 2 -> 0 zone overlaps
/// (or touches) the entry zone, and each face of the entry zone inside the
/// old volume has a catch exactly one f32 step outside it (in mm; in
/// metres, after the loader's mm -> m conversion, still disjoint and at
/// most two steps).
#[test]
fn greybox_below_deck_catch_has_no_gaps() {
    let g = greybox();
    let e = entry(&g).zone;
    let catches: Vec<&LayerTransitionDef> = g
        .table
        .layer_transitions
        .iter()
        .filter(|t| t.from_layer == 2 && t.to_layer == 0)
        .collect();
    for t in &catches {
        assert!(!overlap(&e, &t.zone), "{} overlaps the entry {e:?}", t.id);
    }

    // Sampling: a deterministic grid plus the special values on each axis.
    let xs = axis_samples(0, &e, 1.0, &[403.5, 404.0, 404.5, 445.5, 446.0, 446.5]);
    let ys = axis_samples(
        1,
        &e,
        2.5,
        &[
            594.0, 594.5, 594.9, 634.79, 634.8, 635.0, 635.5, 636.0, 610.0,
        ],
    );
    let zs = axis_samples(2, &e, 0.5, &[-6.5, -5.5, 0.0, 0.1, 8.9]);
    let m = pin_core::units::mm3_to_m;
    let zones_m: Vec<pin_physics::world::LayerTransition> = catches
        .iter()
        .map(|t| pin_physics::world::LayerTransition {
            from_layer: 2,
            to_layer: 0,
            min: Vec3::from_array(m(t.zone.min)),
            max: Vec3::from_array(m(t.zone.max)),
        })
        .collect();
    let entry_m = pin_physics::world::LayerTransition {
        from_layer: 0,
        to_layer: 2,
        min: Vec3::from_array(m(e.min)),
        max: Vec3::from_array(m(e.max)),
    };
    let mut gaps = Vec::new();
    let mut n = 0usize;
    for &x in &xs {
        for &y in &ys {
            for &z in &zs {
                let p = [x, y, z];
                n += 1;
                if !(contains(&e, p) || catches.iter().any(|t| contains(&t.zone, p))) {
                    gaps.push(p);
                    continue;
                }
                // The same point as the simulation sees it (metres).
                let pm = Vec3::from_array(m(p));
                if !(entry_m.contains(pm) || zones_m.iter().any(|t| t.contains(pm))) {
                    gaps.push(p);
                }
            }
        }
    }
    assert!(n > 100_000, "only {n} samples");
    // The spec example in the old side strip.
    assert!(xs.contains(&404.0) && ys.contains(&610.0) && zs.contains(&0.0));
    assert!(
        gaps.is_empty(),
        "{} of {n} sampled points uncovered, e.g. {:?}",
        gaps.len(),
        &gaps[..gaps.len().min(8)]
    );

    // The slivers: each entry face inside the old volume has a catch whose
    // facing bound is the adjacent f32 (mm) and stays disjoint in metres.
    // In metres the step can be two ulps: the mm f32 grid divided by 1000
    // is coarser than the metre grid at some values (595 mm).
    for k in 0..3 {
        let (lo, hi) = (OLD_CATCH.min[k], OLD_CATCH.max[k]);
        if lo < e.min[k] && e.min[k] <= hi {
            let near = catches
                .iter()
                .map(|t| t.zone.max[k])
                .filter(|&b| b < e.min[k])
                .fold(f32::NEG_INFINITY, max_f32);
            assert_eq!(near, e.min[k].next_down(), "axis {k} min face");
            let w = ulps(
                pin_core::units::mm_to_m(near),
                pin_core::units::mm_to_m(e.min[k]),
            );
            assert!(
                (1..=2).contains(&w),
                "axis {k} min face: {w} ulps in metres"
            );
        }
        if lo <= e.max[k] && e.max[k] < hi {
            let near = catches
                .iter()
                .map(|t| t.zone.min[k])
                .filter(|&b| b > e.max[k])
                .fold(f32::INFINITY, min_f32);
            assert_eq!(near, e.max[k].next_up(), "axis {k} max face");
            let w = ulps(
                pin_core::units::mm_to_m(e.max[k]),
                pin_core::units::mm_to_m(near),
            );
            assert!(
                (1..=2).contains(&w),
                "axis {k} max face: {w} ulps in metres"
            );
        }
    }
}
