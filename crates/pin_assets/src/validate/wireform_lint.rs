//! Wireform near-cusp lint: a warning, never an error (#137, KP-056).
//!
//! A wireform's rails follow a rotation-minimizing frame along its
//! centerline, a uniformly parametrized natural cubic spline (SPEC §2.1;
//! DECISIONS.md "2.1 — fix task W"). Where the centerline's speed
//! `|dP/dt|` nearly vanishes (a near-cusp: the tangent shrinks and may
//! reverse within one sample step) the frame is ill-defined and the twist
//! the rails get there is only a convention. This lint warns about such
//! centerlines.
//!
//! Rule: for each segment `i` (between through-points `i` and `i + 1`),
//! sample the speed at [`SPEED_SAMPLES`] + 1 evenly spaced parameters (both
//! ends included) and take the minimum; divide it by the segment's chord
//! (the distance between its two through-points, the speed of a uniform
//! straight parametrization). A reversal (the tangent turning by more than
//! 90° between neighbouring samples, as `guide_lint::segment_bends` sees
//! it) counts as ratio 0. The worst segment (smallest ratio; the first on
//! a tie) warns when its ratio is below [`NEAR_CUSP_SPEED_RATIO`].
//!
//! Deterministic, `f64` internally, no allocation: `O(65 × segments)`.

use glam::DVec3;
use pin_core::fmath::min_f64;
use pin_physics::world::CubicSpline;

/// Speed samples per segment (plus one: both ends are sampled), as the
/// guide curvature lint samples (`guide_lint::CURVATURE_SAMPLES`).
pub(super) const SPEED_SAMPLES: usize = 64;

/// A wireform segment whose smallest speed is below this fraction of its
/// chord warns. Calibrated on the shipped tables (DECISIONS.md "#137
/// KP-056 — validation lints"): their smallest ratio is far above it, and a
/// collinear 61 mm then 300 mm pair of steps (start speed about 2 % of the
/// chord) is below it.
pub(super) const NEAR_CUSP_SPEED_RATIO: f64 = 0.05;

/// The slowest segment of a centerline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SlowSegment {
    /// Segment index: it runs from `points[segment]` to
    /// `points[segment + 1]`.
    pub segment: usize,
    /// Smallest sampled speed over the chord (0 for a reversal); finite and
    /// not negative.
    pub ratio: f64,
}

/// The segment of `spline` with the smallest speed-to-chord ratio (see the
/// [module docs](self)), or `None` when no segment has a positive, finite
/// chord.
pub(super) fn slowest_segment(spline: &CubicSpline) -> Option<SlowSegment> {
    let h = 1.0 / SPEED_SAMPLES as f64;
    let mut worst: Option<SlowSegment> = None;
    for (i, seg) in spline.segments().iter().enumerate() {
        let [p0, p1, p2, p3] = seg.p.map(|q| q.as_dvec3());
        let chord = p0.distance(p3);
        if !(chord.is_finite() && chord > 0.0) {
            continue;
        }
        let tangent = |s: f64| {
            let a = 1.0 - s;
            ((p1 - p0) * (a * a) + (p2 - p1) * (2.0 * a * s) + (p3 - p2) * (s * s)) * 3.0
        };
        let mut min_speed = f64::INFINITY;
        let mut previous: Option<DVec3> = None;
        for j in 0..=SPEED_SAMPLES {
            let t = tangent(j as f64 * h);
            if previous.is_some_and(|p| p.dot(t) < 0.0) {
                min_speed = 0.0;
            }
            previous = Some(t);
            min_speed = min_f64(min_speed, t.length());
        }
        let ratio = min_speed / chord;
        // `!(>= 0)` also maps a NaN (no finite speed) to 0: no worse case.
        let ratio = if ratio >= 0.0 && ratio.is_finite() {
            ratio
        } else {
            0.0
        };
        if worst.is_none_or(|w| ratio < w.ratio) {
            worst = Some(SlowSegment { segment: i, ratio });
        }
    }
    worst
}

/// The slowest segment of `spline` when it is a near-cusp (its ratio is
/// below [`NEAR_CUSP_SPEED_RATIO`]).
pub(super) fn near_cusp(spline: &CubicSpline) -> Option<SlowSegment> {
    slowest_segment(spline).filter(|s| s.ratio < NEAR_CUSP_SPEED_RATIO)
}

/// The warning text for a near-cusp segment.
pub(super) fn message(c: SlowSegment) -> String {
    format!(
        "wireform centerline speed nearly vanishes between points[{}] and points[{}]: its \
         smallest speed is {:.1} % of the segment's chord (below {:.0} %); at a near-cusp the \
         rails' twist is only a convention, so the rails may turn unexpectedly there; add \
         through-points or move the points apart",
        c.segment,
        c.segment + 1,
        c.ratio * 100.0,
        NEAR_CUSP_SPEED_RATIO * 100.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use proptest::prelude::*;

    fn spline(mm: &[[f32; 3]]) -> CubicSpline {
        let pts: Vec<Vec3> = mm.iter().map(|p| Vec3::from_array(*p) * 1.0e-3).collect();
        CubicSpline::interpolate(&pts).unwrap()
    }

    /// SPEC §2.1 (#137): two equal collinear steps are a uniform straight
    /// parametrization: ratio 1, no warning.
    #[test]
    fn equal_steps_have_ratio_one() {
        let s = spline(&[[0.0, 0.0, 0.0], [0.0, 100.0, 0.0], [0.0, 200.0, 0.0]]);
        let w = slowest_segment(&s).unwrap();
        assert!((w.ratio - 1.0).abs() < 1.0e-4, "{w:?}");
        assert_eq!(near_cusp(&s), None);
    }

    /// SPEC §2.1 (#137): collinear steps of 61 mm then 300 mm start at about
    /// 1.25 mm per unit t (natural spline: 61 - 358.5 / 6): ratio about 0.02
    /// on the first segment, a near-cusp.
    #[test]
    fn short_then_long_collinear_step_is_a_near_cusp() {
        let s = spline(&[[0.0, 0.0, 0.0], [0.0, 61.0, 0.0], [0.0, 361.0, 0.0]]);
        let w = near_cusp(&s).unwrap();
        assert_eq!(w.segment, 0);
        assert!((w.ratio - 1.25 / 61.0).abs() < 1.0e-3, "{w:?}");
        let m = message(w);
        for part in [
            "wireform",
            "nearly vanishes",
            "points[0]",
            "points[1]",
            "2.0 %",
            "convention",
            "add through-points",
        ] {
            assert!(m.contains(part), "{part:?} not in {m}");
        }
    }

    /// SPEC §2.1 (#137): 40 mm forward then 300 mm back reverses within the
    /// first segment (start speed 125, end speed -130 mm per unit t):
    /// ratio 0.
    #[test]
    fn reversal_has_ratio_zero() {
        let s = spline(&[[0.0, 0.0, 0.0], [0.0, 40.0, 0.0], [0.0, -260.0, 0.0]]);
        let w = near_cusp(&s).unwrap();
        assert_eq!(
            w,
            SlowSegment {
                segment: 0,
                ratio: 0.0
            }
        );
    }

    /// SPEC §2.1 (#137): the same near-cusp in a tilted plane (direction
    /// (1, 2, 2) / 3, so all three coordinates move) and after a curved
    /// lead-in is found on its own segment.
    #[test]
    fn planar_3d_near_cusp() {
        let d = [1.0f32 / 3.0, 2.0 / 3.0, 2.0 / 3.0];
        let at = |k: f32| [100.0 + d[0] * k, 50.0 + d[1] * k, 20.0 + d[2] * k];
        let s = spline(&[at(0.0), at(61.0), at(361.0)]);
        let w = near_cusp(&s).unwrap();
        assert_eq!(w.segment, 0);
        assert!((w.ratio - 1.25 / 61.0).abs() < 2.0e-3, "{w:?}");
        // A smooth curved ramp: no warning.
        let ramp = spline(&[
            [60.0, 900.0, 40.0],
            [40.0, 600.0, 55.0],
            [150.0, 260.0, 20.0],
        ]);
        assert!(slowest_segment(&ramp).unwrap().ratio > NEAR_CUSP_SPEED_RATIO);
        assert_eq!(near_cusp(&ramp), None);
    }

    /// SPEC §2.1 (#137): a switchback at the end of a longer climbing
    /// path (40 mm on, then 300 mm back along the same line): the worst
    /// segment is the one that turns back, at ratio 0.
    #[test]
    fn switchback_warns_on_its_segment() {
        let d = [0.0f32, 0.8, 0.6];
        let at = |k: f32| [50.0, 100.0 + d[1] * k, 10.0 + d[2] * k];
        let s = spline(&[at(0.0), at(200.0), at(400.0), at(440.0), at(140.0)]);
        let w = near_cusp(&s).unwrap();
        assert!(w.segment == 2 || w.segment == 3, "{w:?}");
        assert_eq!(w.ratio, 0.0);
        assert!(message(w).contains(&format!("points[{}]", w.segment + 1)));
    }

    /// SPEC §2.1 (#137): no segment with a chord gives no verdict.
    #[test]
    fn degenerate_spline_has_no_segment() {
        let s = CubicSpline::interpolate(&[Vec3::ZERO, Vec3::ZERO]);
        if let Ok(s) = s {
            assert_eq!(slowest_segment(&s), None);
        }
    }

    /// SPEC §2.1 (#137): deterministic: identical output on repeated calls.
    #[test]
    fn deterministic() {
        let s = spline(&[[0.0, 0.0, 0.0], [0.0, 61.0, 3.0], [10.0, 361.0, 9.0]]);
        let a = slowest_segment(&s);
        for _ in 0..5 {
            let b = slowest_segment(&s);
            assert_eq!(
                a.map(|w| (w.segment, w.ratio.to_bits())),
                b.map(|w| (w.segment, w.ratio.to_bits()))
            );
            assert_eq!(a.map(message), b.map(message));
        }
    }

    /// SPEC §2.1 (#137): calibration: every wireform of the shipped tables
    /// and of the apex fixture is far from a near-cusp (prints the ratios:
    /// `cargo test -p pin_assets shipped_wireforms -- --nocapture`).
    #[test]
    fn shipped_wireforms_are_not_near_cusps() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut seen = 0;
        for file in [
            "../../tables/greybox/table.toml",
            "../../tables/apex_circuit/table.toml",
            "../../tables/sky_deck/table.toml",
            "../../tables/feature_lab/table.toml",
            "tests/fixtures/apex/table.toml",
        ] {
            let text = std::fs::read_to_string(root.join(file)).unwrap();
            let t = pin_core::schema::TableManifest::from_toml_str(&text).unwrap();
            for s in t
                .splines
                .iter()
                .filter(|s| s.kind == pin_core::schema::table::SplineKind::Wireform)
            {
                let w = slowest_segment(&spline(&s.points)).unwrap();
                println!(
                    "{file} {}: segment {} ratio {:.4}",
                    s.id, w.segment, w.ratio
                );
                assert!(w.ratio >= NEAR_CUSP_SPEED_RATIO, "{file} {}: {w:?}", s.id);
                seen += 1;
            }
        }
        assert_eq!(
            seen, 4,
            "greybox, apex_circuit, sky_deck and the apex fixture"
        );
    }

    fn points() -> impl Strategy<Value = Vec<[f32; 3]>> {
        prop::collection::vec(prop::array::uniform3(0.0f32..1.5), 3..12)
            .prop_filter("no repeated consecutive points", |v| {
                v.windows(2).all(|w| w[0] != w[1])
            })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        /// SPEC §2.1 (#137): random finite centerlines (3..12 points within
        /// 0..1.5 m) never panic, and the ratio is finite and not negative.
        #[test]
        fn ratio_is_finite_and_not_negative(pts in points()) {
            let through: Vec<Vec3> = pts.iter().map(|p| Vec3::from_array(*p)).collect();
            if let Ok(s) = CubicSpline::interpolate(&through) {
                let w = slowest_segment(&s);
                prop_assert!(w.is_some());
                let w = w.unwrap();
                prop_assert!(w.ratio.is_finite() && w.ratio >= 0.0, "{:?}", w);
                prop_assert!(w.segment + 1 < pts.len());
                prop_assert_eq!(near_cusp(&s).is_some(), w.ratio < NEAR_CUSP_SPEED_RATIO);
                let again = slowest_segment(&s).unwrap();
                prop_assert_eq!(again.ratio.to_bits(), w.ratio.to_bits());
            }
        }
    }
}
