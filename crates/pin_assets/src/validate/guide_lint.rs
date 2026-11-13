//! Guide authoring lints: warnings only, never errors (human ruling, end of
//! Wave 3, item 4; DECISIONS.md "4.1 — fix task H").
//!
//! * (b) through-point spacing at or above the smallest radius of curvature
//!   (the guide's closest-point sampling bounds rely on the opposite);
//! * (c) a ball profile wider than the tightest bend of a sloped guide (it
//!   slides slowly there, in single steps);
//! * (d) a level guide whose open ends nearly meet (a hairline gap), or whose
//!   wall has no proven reach (an open end touching its own wall).
//!
//! (a), uneven spacing, stays in `rules.rs` next to the spline build.

use pin_physics::world::{CubicSpline, GuideCurve};

/// Curvature samples per spline segment (one segment per through-point
/// spacing; the parameter is uniform per segment).
pub(super) const CURVATURE_SAMPLES: usize = 64;
/// Golden-section steps that refine the largest sample.
const REFINE_STEPS: usize = 40;
/// Open ends of a level guide closer than this (m), but not snapped closed,
/// are a hairline gap.
pub(super) const HAIRLINE_GAP_M: f32 = 1.0e-3;

/// A reach below this (m) counts as none: `GuideCurve::reach()` is exactly 0
/// for an open end within ~0.1 mm of its own wall, but a curve crossing
/// itself between samples can come out at a few nanometres.
pub(super) const NO_REACH_M: f32 = 1.0e-6;

/// The tightest bend of a curve.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct Bend {
    /// Smallest radius of curvature, metres (0 at a cusp).
    pub radius_m: f64,
    /// Curve parameter where it occurs (`P_i` at `t = i`).
    pub t: f64,
}

/// Curvature at `t`: the larger of the space curve's and the plan view's
/// (`x`, `y`), so that a sloped guide is judged by the bend its wall (a
/// vertical cylinder over the plan view) really has. Infinite where the
/// relevant speed vanishes.
fn curvature(spline: &CubicSpline, t: f64) -> f64 {
    let d1 = spline.tangent(t as f32).as_dvec3();
    let d2 = spline.second_derivative(t as f32).as_dvec3();
    let speed = d1.length();
    let plan_speed = d1.x.hypot(d1.y);
    let space = if speed > 1.0e-12 {
        d1.cross(d2).length() / (speed * speed * speed)
    } else {
        f64::INFINITY
    };
    let plan = if plan_speed > 1.0e-12 {
        (d1.x * d2.y - d1.y * d2.x).abs() / (plan_speed * plan_speed * plan_speed)
    } else {
        // A cusp, or a vertical run: no plan-view bend a ball can follow.
        f64::INFINITY
    };
    space.max(plan)
}

/// Smallest radius of curvature of `spline`.
///
/// Sampling: [`CURVATURE_SAMPLES`] evenly spaced parameters per segment
/// (so every through-point spacing is probed at 1/64 of its length, and both
/// ends of every segment are included), then a golden-section refinement of
/// the largest sample over the two neighbouring sample intervals. A bend
/// tight enough to matter for (b) (radius ≤ spacing) turns the tangent by a
/// sizeable angle over a parameter interval much wider than 1/64, so the
/// samples see it; the refinement recovers the peak itself to within float
/// precision when the peak is smooth.
pub(super) fn tightest_bend(spline: &CubicSpline) -> Bend {
    let n = spline.segment_count();
    let h = 1.0 / CURVATURE_SAMPLES as f64;
    let mut best = (0.0f64, 0.0f64);
    for i in 0..n {
        for j in 0..=CURVATURE_SAMPLES {
            let t = i as f64 + j as f64 * h;
            let k = curvature(spline, t);
            if k > best.0 {
                best = (k, t);
            }
        }
    }
    if best.0.is_finite() && best.0 > 0.0 {
        // Golden-section search for the maximum on [t - h, t + h].
        let (mut a, mut b) = ((best.1 - h).max(0.0), (best.1 + h).min(n as f64));
        let g = (5.0f64.sqrt() - 1.0) / 2.0;
        let mut c = b - g * (b - a);
        let mut d = a + g * (b - a);
        let (mut kc, mut kd) = (curvature(spline, c), curvature(spline, d));
        for _ in 0..REFINE_STEPS {
            if kc > kd {
                b = d;
                d = c;
                kd = kc;
                c = b - g * (b - a);
                kc = curvature(spline, c);
            } else {
                a = c;
                c = d;
                kc = kd;
                d = a + g * (b - a);
                kd = curvature(spline, d);
            }
        }
        for (k, t) in [(kc, c), (kd, d)] {
            if k > best.0 {
                best = (k, t);
            }
        }
    }
    let radius_m = if best.0 > 0.0 {
        1.0 / best.0
    } else {
        f64::INFINITY
    };
    Bend {
        radius_m,
        t: best.1,
    }
}

/// What the level-guide lint (d) found.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum GapLint {
    /// The open ends are this far apart (m), under [`HAIRLINE_GAP_M`].
    Hairline(f32),
    /// No proven reach although the guide is open: an end touches or nearly
    /// touches its own wall.
    EndTouchesWall,
    /// No proven reach on a closed guide: the curve touches or crosses itself.
    SelfTouching,
}

/// Lint (d) for a built guide (`closed` = snapped closed by the loader).
pub(super) fn gap_lint(guide: &GuideCurve, closed: bool) -> Option<GapLint> {
    if !guide.is_level() {
        return None;
    }
    let spline = guide.spline();
    if !closed {
        let first = spline.point(0.0);
        let last = spline.point(spline.max_t());
        let gap = first.distance(last);
        if gap < HAIRLINE_GAP_M {
            return Some(GapLint::Hairline(gap));
        }
    }
    (guide.reach() < NO_REACH_M).then_some(if closed {
        GapLint::SelfTouching
    } else {
        GapLint::EndTouchesWall
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn circle(r: f32, n: usize) -> CubicSpline {
        let mut pts: Vec<Vec3> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * core::f32::consts::TAU;
                Vec3::new(r * a.cos(), r * a.sin(), 0.0)
            })
            .collect();
        pts.push(pts[0]);
        CubicSpline::interpolate_closed(&pts).unwrap()
    }

    #[test]
    fn circle_radius_is_recovered() {
        let bend = tightest_bend(&circle(0.1, 32));
        assert!((bend.radius_m - 0.1).abs() < 1.0e-3, "{bend:?}");
    }

    #[test]
    fn straight_line_has_no_bend() {
        let s = CubicSpline::interpolate(&[Vec3::ZERO, Vec3::X, Vec3::X * 2.0]).unwrap();
        assert_eq!(tightest_bend(&s).radius_m, f64::INFINITY);
    }

    #[test]
    fn plan_view_counts_for_a_helix_like_curve() {
        // A rising quarter turn: the plan view bends tighter than the space curve.
        let pts: Vec<Vec3> = (0..=8)
            .map(|i| {
                let a = i as f32 / 8.0 * core::f32::consts::FRAC_PI_2;
                Vec3::new(0.1 * a.cos(), 0.1 * a.sin(), 0.02 * i as f32)
            })
            .collect();
        let s = CubicSpline::interpolate(&pts).unwrap();
        let bend = tightest_bend(&s);
        assert!(bend.radius_m < 0.1 + 1.0e-3, "{bend:?}");
        // A vertical run has no plan-view bend a ball can follow.
        assert_eq!(
            curvature(
                &CubicSpline::interpolate(&[Vec3::ZERO, Vec3::Z]).unwrap(),
                0.5
            ),
            f64::INFINITY
        );
    }

    #[test]
    fn gap_lints() {
        let open = |end: Vec3| {
            let pts = [
                Vec3::ZERO,
                Vec3::new(0.1, 0.0, 0.0),
                Vec3::new(0.1, 0.1, 0.0),
                Vec3::new(0.0, 0.1, 0.0),
                end,
            ];
            GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.0).unwrap()
        };
        let g = open(Vec3::new(0.0, 0.0005, 0.0));
        assert!(
            matches!(gap_lint(&g, false), Some(GapLint::Hairline(d)) if (d - 0.0005).abs() < 1e-6)
        );
        assert_eq!(gap_lint(&open(Vec3::new(0.0, 0.02, 0.0)), false), None);
        let closed = GuideCurve::new(circle(0.1, 16), 0.0).unwrap();
        assert_eq!(gap_lint(&closed, true), None);
        let sloped = GuideCurve::new(
            CubicSpline::interpolate(&[Vec3::ZERO, Vec3::new(0.1, 0.0, 0.01)]).unwrap(),
            0.0,
        )
        .unwrap();
        assert_eq!(gap_lint(&sloped, false), None);
        // A closed figure eight crosses itself: no reach.
        let mut eight: Vec<Vec3> = (0..16)
            .map(|i| {
                let a = i as f32 / 16.0 * core::f32::consts::TAU;
                Vec3::new(0.1 * a.sin(), 0.05 * (2.0 * a).sin(), 0.0)
            })
            .collect();
        eight.push(eight[0]);
        let crossing =
            GuideCurve::new(CubicSpline::interpolate_closed(&eight).unwrap(), 0.0).unwrap();
        assert_eq!(gap_lint(&crossing, true), Some(GapLint::SelfTouching));
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig {
            cases: 64,
            failure_persistence: None,
            ..proptest::prelude::ProptestConfig::default()
        })]

        /// A circle through n ≥ 16 points: the smallest radius is the circle's
        /// within 2 %.
        #[test]
        fn circles(r in 0.01f32..2.0, n in 16usize..40) {
            let bend = tightest_bend(&circle(r, n));
            proptest::prop_assert!((bend.radius_m / f64::from(r) - 1.0).abs() < 0.02, "{bend:?}");
        }

        /// Any spline: the radius is non-negative (never NaN) and the
        /// location is inside the parameter range.
        #[test]
        fn random_splines(pts in proptest::collection::vec(
            (-1.0f32..1.0, -1.0f32..1.0, -0.1f32..0.1), 2..10)
        ) {
            let pts: Vec<Vec3> = pts.into_iter().map(|(x, y, z)| Vec3::new(x, y, z)).collect();
            if let Ok(s) = CubicSpline::interpolate(&pts) {
                let bend = tightest_bend(&s);
                proptest::prop_assert!(bend.radius_m >= 0.0);
                proptest::prop_assert!(bend.t >= 0.0 && bend.t <= f64::from(s.max_t()));
            }
        }
    }
}
