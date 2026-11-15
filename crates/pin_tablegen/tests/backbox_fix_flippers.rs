//! Subphase 5.5, PR #20 CodeRabbit findings C, D and E: backbox flipper
//! validation. With a polygon `outline`, the whole tapered bat body (hull
//! of the pivot and tip discs) is checked over the stroke at
//! `STROKE_STEP_DEG` steps, so a notch reached only between the old three
//! sampled angles (C) or only by the shaft (D) is rejected; DualStageThreshold
//! thresholds follow the runtime staging router's rule (E). The box path is
//! unchanged.

#[path = "backbox_common.rs"]
mod common;

use pin_tablegen::backbox::{
    GeneratedBackbox, STROKE_STEP_DEG, generate_backbox, parse_backbox_layout, write_backbox,
};
use pin_tablegen::layout::GenError;
use proptest::prelude::*;
use std::fmt::Write as _;

const RECT: &[[f32; 2]] = &[[-200.0, 0.0], [200.0, 0.0], [200.0, 300.0], [-200.0, 300.0]];
/// Finding C: a notch u 80..200, v 65..75 cut in from the right edge.
const NOTCH_C: &[[f32; 2]] = &[
    [-200.0, 0.0],
    [200.0, 0.0],
    [200.0, 65.0],
    [80.0, 65.0],
    [80.0, 75.0],
    [200.0, 75.0],
    [200.0, 300.0],
    [-200.0, 300.0],
];
/// Finding D: a notch u -40..-20, v 80..300 cut down from the top.
const NOTCH_D: &[[f32; 2]] = &[
    [-200.0, 0.0],
    [200.0, 0.0],
    [200.0, 300.0],
    [-20.0, 300.0],
    [-20.0, 80.0],
    [-40.0, 80.0],
    [-40.0, 300.0],
    [-200.0, 300.0],
];
/// A convex octagon (bounding box 400 x 300).
const OCTAGON: &[[f32; 2]] = &[
    [-150.0, 0.0],
    [150.0, 0.0],
    [200.0, 50.0],
    [200.0, 250.0],
    [150.0, 300.0],
    [-150.0, 300.0],
    [-200.0, 250.0],
    [-200.0, 50.0],
];
/// Concave: the top right quarter cut away.
const L_SHAPE: &[[f32; 2]] = &[
    [-200.0, 0.0],
    [200.0, 0.0],
    [200.0, 120.0],
    [0.0, 120.0],
    [0.0, 300.0],
    [-200.0, 300.0],
];

fn mirror(pts: &[[f32; 2]]) -> Vec<[f32; 2]> {
    pts.iter().map(|p| [-p[0], p[1]]).collect()
}

fn list(pts: &[[f32; 2]]) -> String {
    let mut s = String::from("[");
    for p in pts {
        let _ = write!(s, "[{:?}, {:?}],", p[0], p[1]);
    }
    s.push(']');
    s
}

/// A 400 x 300 `backbox_layout.toml` (box default when `outline` is
/// `None`) with one flipper.
fn layout(
    outline: Option<&[[f32; 2]]>,
    side: &str,
    pos: [f32; 2],
    len: f32,
    staging: &str,
) -> String {
    // Width and height match the outline's bounding box; the holes sit at
    // 20 % and 80 % of it.
    let (lo, hi) = match outline {
        None => ([-200.0f32, 0.0f32], [200.0f32, 300.0f32]),
        Some(o) => o
            .iter()
            .fold(([f32::MAX; 2], [f32::MIN; 2]), |(lo, hi), p| {
                (
                    [lo[0].min(p[0]), lo[1].min(p[1])],
                    [hi[0].max(p[0]), hi[1].max(p[1])],
                )
            }),
    };
    let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
    let (entry, drain) = match outline {
        None => (-120.0f32, 120.0f32),
        Some(_) => (lo[0] + 0.2 * w, lo[0] + 0.8 * w),
    };
    let mut s = format!(
        "schema_version = 1\n\n[backbox]\norigin = [0.0, 1150.0, 250.0]\nwidth = {w:?}\n\
         height = {h:?}\ndepth = 35.0\n",
    );
    if let Some(o) = outline {
        let _ = writeln!(s, "outline = {}", list(o));
    }
    let _ = write!(
        s,
        "\n[backbox.entry]\nvuk = [120.0, 1000.0]\nat = {entry:?}\ncoil = \"bb_vuk_coil\"\n\
         switch = \"bb_vuk_switch\"\n\n[backbox.drain]\nat = {drain:?}\n\n\
         [[backbox.flippers]]\nside = \"{side}\"\nposition = [{:?}, {:?}]\nlength = {len:?}\n\
         coil = \"bb_left_coil\"\neos_switch = \"bb_left_eos\"\nstaging = {staging}\n",
        pos[0], pos[1]
    );
    s
}

const PARALLEL: &str = "{ type = \"ParallelLinked\", input = \"left_flipper\" }";

fn gen_text(t: &str) -> Result<GeneratedBackbox, GenError> {
    let file = parse_backbox_layout(t)?;
    generate_backbox(&file.backbox, &common::base())
}

fn flipper(
    outline: Option<&[[f32; 2]]>,
    side: &str,
    pos: [f32; 2],
    len: f32,
) -> Result<GeneratedBackbox, GenError> {
    gen_text(&layout(outline, side, pos, len, PARALLEL))
}

/// The angle and point of an "outside the backbox" error.
fn angle_and_point(e: &GenError) -> (f32, [f32; 2]) {
    let m = &e.message;
    let (head, tail) = m.split_once(" degrees (").unwrap_or_else(|| panic!("{e}"));
    let deg: f32 = head.rsplit(' ').next().unwrap().parse().unwrap();
    let (xy, _) = tail.split_once(") mm").unwrap();
    let (x, y) = xy.split_once(", ").unwrap();
    (deg, [x.parse().unwrap(), y.parse().unwrap()])
}

fn assert_outside(r: Result<GeneratedBackbox, GenError>) -> (f32, [f32; 2]) {
    let e = match r {
        Ok(_) => panic!("accepted"),
        Err(e) => e,
    };
    assert_eq!(e.element, "backbox.flippers[0]", "{e}");
    assert!(e.message.contains("outside the backbox"), "{e}");
    assert!(e.message.contains("degrees"), "{e}");
    angle_and_point(&e)
}

#[test]
fn finding_c_notch_between_the_old_samples_is_rejected() {
    let (deg, p) = assert_outside(flipper(Some(NOTCH_C), "left", [0.0, 100.0], 100.0));
    // Inside the stroke, not one of the three angles checked before.
    assert!(
        (-30.0..=20.0).contains(&deg) && ![-30.0, -5.0, 20.0].contains(&deg),
        "{deg}"
    );
    // A multiple of the step from rest.
    assert_eq!(((deg + 30.0) / STROKE_STEP_DEG).fract(), 0.0, "{deg}");
    // The offending point is at the notch (u 80..200, v 65..75), within a
    // tip radius.
    assert!(
        p[0] > 80.0 - 9.0 && p[1] > 65.0 - 9.0 && p[1] < 75.0 + 9.0,
        "{p:?}"
    );
    // The same bat in the plain rectangle is fine.
    flipper(Some(RECT), "left", [0.0, 100.0], 100.0).unwrap();
    // Right-flipper mirror.
    let m = mirror(NOTCH_C);
    let (deg, p) = assert_outside(flipper(Some(&m), "right", [0.0, 100.0], 100.0));
    assert!(
        (160.0..=210.0).contains(&deg) && ![210.0, 185.0, 160.0].contains(&deg),
        "{deg}"
    );
    assert!(p[0] < -80.0 + 9.0, "{p:?}");
    flipper(Some(&mirror(RECT)), "right", [0.0, 100.0], 100.0).unwrap();
}

#[test]
fn finding_d_shaft_crossing_a_notch_is_rejected() {
    let (_, p) = assert_outside(flipper(Some(NOTCH_D), "left", [-70.0, 100.0], 100.0));
    // The breach is on the shaft, at the notch's u range.
    assert!((-45.0..=-15.0).contains(&p[0]), "{p:?}");
    flipper(Some(RECT), "left", [-70.0, 100.0], 100.0).unwrap();
    let m = mirror(NOTCH_D);
    let (_, p) = assert_outside(flipper(Some(&m), "right", [70.0, 100.0], 100.0));
    assert!((15.0..=45.0).contains(&p[0]), "{p:?}");
    flipper(Some(&mirror(RECT)), "right", [70.0, 100.0], 100.0).unwrap();
}

#[test]
fn whole_stroke_inside_convex_and_concave_outlines_is_accepted() {
    for (o, pos) in [
        (OCTAGON, [-60.0, 80.0]),
        (L_SHAPE, [-150.0, 80.0]),
        (NOTCH_D, [-150.0, 100.0]),
        (NOTCH_C, [-100.0, 150.0]),
    ] {
        flipper(Some(o), "left", pos, 100.0).unwrap();
        let m = mirror(o);
        flipper(Some(&m), "right", [-pos[0], pos[1]], 100.0).unwrap();
    }
}

#[test]
fn box_path_keeps_its_three_angle_disc_check() {
    // The box is unchanged: a bat that clears the walls at -30, -5 and 20
    // degrees is accepted, one that does not is rejected with the old text.
    flipper(None, "left", [0.0, 100.0], 100.0).unwrap();
    let e = flipper(None, "left", [110.0, 100.0], 100.0).unwrap_err();
    assert_eq!(e.element, "backbox.flippers[0]");
    assert_eq!(
        e.message,
        "the bat reaches outside the backbox at -30 degrees (196.6, 50.0) mm"
    );
}

fn dual(lo: &str, hi: &str) -> String {
    format!(
        "{{ type = \"DualStageThreshold\", axis = \"left_flipper\", lower_threshold = {lo}, upper_threshold = {hi} }}"
    )
}

/// Generates and, only on success, writes into a fresh empty folder; returns
/// the result and whether the folder is still empty.
fn gen_and_write(t: &str) -> (Result<GeneratedBackbox, GenError>, bool) {
    let dir = std::env::temp_dir().join(format!(
        "pin_tablegen_backbox_fix_flippers_{}_{}",
        std::process::id(),
        common::fnv1a(t.as_bytes())
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let r = gen_text(t);
    if let Ok(g) = &r {
        write_backbox(g, &dir).unwrap();
    }
    let empty = std::fs::read_dir(&dir).unwrap().next().is_none();
    let _ = std::fs::remove_dir_all(&dir);
    (r, empty)
}

#[test]
fn finding_e_staging_thresholds_follow_the_runtime_router() {
    for outline in [None, Some(RECT)] {
        for (lo, hi) in [
            ("0.1", "0.9"),
            ("0.0", "1.0"),
            ("0.5", "0.5"),
            ("0.3", "0.85"),
        ] {
            let (r, empty) =
                gen_and_write(&layout(outline, "left", [-70.0, 50.0], 50.0, &dual(lo, hi)));
            r.unwrap_or_else(|e| panic!("({lo}, {hi}): {e}"));
            assert!(!empty);
        }
        for (lo, hi, shown) in [
            ("0.9", "0.1", ["0.9", "0.1"]),
            ("-0.1", "0.5", ["-0.1", "0.5"]),
            ("0.2", "1.5", ["0.2", "1.5"]),
            ("nan", "0.5", ["NaN", "0.5"]),
            ("0.1", "nan", ["0.1", "NaN"]),
            ("0.1", "inf", ["0.1", "inf"]),
            ("-inf", "0.5", ["-inf", "0.5"]),
            ("1.5", "2.0", ["1.5", "2"]),
        ] {
            let (r, empty) =
                gen_and_write(&layout(outline, "left", [-70.0, 50.0], 50.0, &dual(lo, hi)));
            let e = match r {
                Ok(_) => panic!("({lo}, {hi}) accepted"),
                Err(e) => e,
            };
            assert!(empty, "nothing is written");
            assert_eq!(e.element, "backbox.flippers[0]", "{e}");
            assert!(
                e.message
                    .contains(&format!("staging.lower_threshold {}", shown[0]))
                    && e.message
                        .contains(&format!("staging.upper_threshold {}", shown[1])),
                "{e}"
            );
        }
    }
}

#[test]
fn staging_error_names_the_offending_flipper_index() {
    let mut t = layout(None, "left", [-70.0, 50.0], 50.0, PARALLEL);
    let _ = write!(
        t,
        "\n[[backbox.flippers]]\nside = \"right\"\nposition = [70.0, 50.0]\ncoil = \"bb_right_coil\"\n\
         eos_switch = \"bb_right_eos\"\nstaging = {}\n",
        dual("0.9", "0.1")
    );
    let Err(e) = gen_text(&t) else {
        panic!("accepted")
    };
    assert_eq!(e.element, "backbox.flippers[1]", "{e}");
}

// --- Oracle -----------------------------------------------------------------

fn seg_dist(x: [f64; 2], p: [f64; 2], q: [f64; 2]) -> f64 {
    let (ex, ey) = (q[0] - p[0], q[1] - p[1]);
    let s = (((x[0] - p[0]) * ex + (x[1] - p[1]) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
    ((x[0] - p[0] - s * ex).powi(2) + (x[1] - p[1] - s * ey).powi(2)).sqrt()
}

fn point_in(poly: &[[f32; 2]], x: [f64; 2]) -> bool {
    let mut inside = false;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
        let (a, b) = (
            [f64::from(a[0]), f64::from(a[1])],
            [f64::from(b[0]), f64::from(b[1])],
        );
        if (a[1] > x[1]) != (b[1] > x[1])
            && x[0] < a[0] + (x[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0])
        {
            inside = !inside;
        }
    }
    inside
}

/// Brute-force clearance of a flipper's stroke against a polygon, mm:
/// at the given angles, discs along the bat axis (1001 per angle, radius
/// interpolated from 0.145 L to 0.085 L) against every edge; `-inf` when a
/// pivot or tip centre lies outside.
fn oracle(poly: &[[f32; 2]], pos: [f32; 2], len: f32, angles: &[f32]) -> f64 {
    let (rb, rt) = (f64::from(0.145 * len), f64::from(0.085 * len));
    let p0 = [f64::from(pos[0]), f64::from(pos[1])];
    let mut best = f64::INFINITY;
    for &deg in angles {
        let a = deg.to_radians();
        let tip = [
            f64::from(pos[0] + len * libm::cosf(a)),
            f64::from(pos[1] + len * libm::sinf(a)),
        ];
        if !point_in(poly, p0) || !point_in(poly, tip) {
            return f64::NEG_INFINITY;
        }
        let n = 1000;
        for k in 0..=n {
            let t = f64::from(k) / f64::from(n);
            let x = [p0[0] + t * (tip[0] - p0[0]), p0[1] + t * (tip[1] - p0[1])];
            let r = (1.0 - t) * rb + t * rt;
            for i in 0..poly.len() {
                let (p, q) = (poly[i], poly[(i + 1) % poly.len()]);
                let d = seg_dist(
                    x,
                    [f64::from(p[0]), f64::from(p[1])],
                    [f64::from(q[0]), f64::from(q[1])],
                );
                best = best.min(d - r);
            }
        }
    }
    best
}

fn rest_sweep(side: &str) -> (f32, f32) {
    if side == "left" {
        (-30.0, 50.0)
    } else {
        (210.0, -50.0)
    }
}

/// The outline path's angles: every STROKE_STEP_DEG from rest, both ends.
fn dense_angles(side: &str) -> Vec<f32> {
    let (rest, sweep) = rest_sweep(side);
    let steps = (sweep.abs() / STROKE_STEP_DEG).ceil() as u32;
    (0..=steps)
        .map(|k| {
            if k == steps {
                rest + sweep
            } else {
                rest + sweep / steps as f32 * k as f32
            }
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, ..ProptestConfig::default() })]

    /// With an outline, the generator accepts a flipper iff the
    /// brute-force swept-disc oracle finds it clear (outside a 1e-3 mm
    /// band), in a convex and in concave outlines, either side.
    #[test]
    fn outline_check_matches_the_disc_oracle(
        which in 0usize..4,
        left in any::<bool>(),
        u in -200.0f32..200.0,
        v in 0.0f32..300.0,
        len in 30.0f32..100.0,
    ) {
        let poly = [OCTAGON, L_SHAPE, NOTCH_C, NOTCH_D][which];
        let side = if left { "left" } else { "right" };
        let r = flipper(Some(poly), side, [u, v], len);
        let m = oracle(poly, [u, v], len, &dense_angles(side));
        if m > 1e-3 {
            prop_assert!(r.is_ok(), "clearance {} but {:?}", m, r.err());
        } else if m < -1e-3 {
            let e = r.err();
            prop_assert!(e.is_some(), "clearance {} but accepted", m);
            prop_assert_eq!(e.unwrap().element, "backbox.flippers[0]");
        }
    }

    /// The same agreement with the bat placed near the concave notches of
    /// findings C and D (either side, the outline mirrored for a right
    /// flipper), where the old three-angle end-disc check went wrong.
    #[test]
    fn notch_check_matches_the_disc_oracle(
        d_notch in any::<bool>(),
        left in any::<bool>(),
        du in 0.0f32..1.0,
        dv in 0.0f32..1.0,
        len in 30.0f32..100.0,
    ) {
        // Pivot ranges (left flipper) that put the stroke across the notch.
        let (base, u, v) = if d_notch {
            (NOTCH_D, -150.0 + 130.0 * du, 60.0 + 120.0 * dv)
        } else {
            (NOTCH_C, -40.0 + 130.0 * du, 40.0 + 100.0 * dv)
        };
        let (poly, side, u) = if left {
            (base.to_vec(), "left", u)
        } else {
            (mirror(base), "right", -u)
        };
        let r = flipper(Some(&poly), side, [u, v], len);
        let m = oracle(&poly, [u, v], len, &dense_angles(side));
        if m > 1e-3 {
            prop_assert!(r.is_ok(), "clearance {} but {:?}", m, r.err());
        } else if m < -1e-3 {
            let e = r.err();
            prop_assert!(e.is_some(), "clearance {} but accepted", m);
            prop_assert_eq!(e.unwrap().element, "backbox.flippers[0]");
        }
    }

    /// The box path (no outline): rest, mid and end of stroke, the whole
    /// body against the box walls, agrees with the generator (the box is
    /// convex, so the end discs decide).
    #[test]
    fn box_check_matches_the_disc_oracle(
        left in any::<bool>(),
        u in -200.0f32..200.0,
        v in 0.0f32..300.0,
        len in 30.0f32..100.0,
    ) {
        let side = if left { "left" } else { "right" };
        let (rest, sweep) = rest_sweep(side);
        let r = flipper(None, side, [u, v], len);
        let m = oracle(RECT, [u, v], len, &[rest, rest + 0.5 * sweep, rest + sweep]);
        if m > 1e-3 {
            prop_assert!(r.is_ok(), "clearance {} but {:?}", m, r.err());
        } else if m < -1e-3 {
            prop_assert!(r.is_err(), "clearance {} but accepted", m);
        }
    }

    /// Random star-shaped (hence valid, often concave) outlines never panic
    /// the flipper check; any rejection names the flipper or a known
    /// element.
    #[test]
    fn random_outlines_with_a_flipper_never_panic(
        radii in proptest::collection::vec(0.2f32..1.0, 5..16),
        left in any::<bool>(),
        u in -200.0f32..200.0,
        v in 0.0f32..300.0,
        len in 30.0f32..100.0,
    ) {
        let n = radii.len() as f32;
        let poly: Vec<[f32; 2]> = radii
            .iter()
            .enumerate()
            .map(|(k, r)| {
                let a = std::f32::consts::TAU * k as f32 / n;
                [200.0 * r * libm::cosf(a), 150.0 + 150.0 * r * libm::sinf(a)]
            })
            .collect();
        let side = if left { "left" } else { "right" };
        if let Err(e) = flipper(Some(&poly), side, [u, v], len) {
            prop_assert!(!e.element.is_empty() && !e.message.is_empty());
        }
    }
}
