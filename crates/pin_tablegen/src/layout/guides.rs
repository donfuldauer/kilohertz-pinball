//! Lane rails: two level guide splines either side of a lane's centre line,
//! with through-points chosen so that `pin_assets` accepts them without an
//! error or an authoring warning.

use glam::Vec3;
use pin_core::schema::table::{SplineDef, SplineKind};
use pin_core::units::mm3_to_m;
use pin_physics::world::{CubicSpline, GuideCurve};

/// Most through-points per spline (the loader's limit).
pub const MAX_SPLINE_POINTS: usize = pin_assets::validate::MAX_SPLINE_POINTS;
/// Longest spline the loader accepts, mm.
const MAX_SPLINE_LENGTH_MM: f64 = pin_assets::validate::MAX_SPLINE_LENGTH_M as f64 * 1000.0;
/// Longest through-point spacing on a rail, mm.
pub const MAX_RAIL_SPACING_MM: f64 = 20.0;
/// Rail spacing as a fraction of the rail's tightest radius (the loader
/// warns when the spacing is not below the radius).
const SPACING_PER_RADIUS: f64 = 0.4;
/// Shortest rail spacing, mm: a rail that would need less bends too
/// tightly to be a guide.
pub const MIN_RAIL_SPACING_MM: f64 = 0.5;
/// A rail may not bend tighter than this fraction of the centre line's
/// radius would allow (`1 − (w/2)·κ` on the inside of a bend; at 0 the
/// inner rail has a cusp).
const MIN_OFFSET_FACTOR: f64 = 0.2;
/// Centre-line samples per spline segment.
const DENSE: usize = 256;
/// Curvature samples per segment when checking a built rail (the loader's
/// lint uses 64 plus a refinement; 128 here, with 10 % margin).
const CHECK_SAMPLES: usize = 128;
/// Open ends closer than this (mm) are a hairline gap (loader lint).
pub const HAIRLINE_GAP_MM: f64 = 1.0;
/// Ends closer than this (mm) close the lane.
pub const CLOSE_SNAP_MM: f64 = 1.0e-3;
/// The loader's reach floor (m): below it a guide has no proven clearance.
const NO_REACH_M: f32 = 1.0e-6;

/// One generated rail.
#[derive(Clone, Debug)]
pub struct Rail {
    /// Through-points, mm (`z` = 0); a closed rail ends on its first point.
    pub points_mm: Vec<[f32; 3]>,
    /// The spline exactly as the loader builds it (m).
    pub spline_m: CubicSpline,
}

/// Plan-view sample of the centre line.
struct Sample {
    c: [f64; 2],
    n: [f64; 2],
    kappa: f64,
    t: f64,
}

fn centre_samples(spline: &CubicSpline) -> Result<Vec<Sample>, String> {
    let segs = spline.segment_count();
    let mut out = Vec::with_capacity(segs * DENSE + 1);
    let mut prev_tangent: Option<[f64; 2]> = None;
    for k in 0..=segs * DENSE {
        let t = k as f64 / DENSE as f64;
        let tf = t as f32;
        let p = spline.point(tf).as_dvec3();
        let d1 = spline.tangent(tf).as_dvec3();
        let d2 = spline.second_derivative(tf).as_dvec3();
        let speed = libm::sqrt(d1.x * d1.x + d1.y * d1.y);
        let at = || format!("near points[{}]", libm::round(t) as usize);
        if speed.is_nan() || speed <= 1.0e-9 {
            return Err(format!("the centre line stops and turns back {}", at()));
        }
        let tan = [d1.x / speed, d1.y / speed];
        if let Some(pt) = prev_tangent
            && pt[0] * tan[0] + pt[1] * tan[1] < 0.0
        {
            return Err(format!("the centre line turns back on itself {}", at()));
        }
        prev_tangent = Some(tan);
        out.push(Sample {
            c: [p.x, p.y],
            n: [-tan[1], tan[0]],
            kappa: (d1.x * d2.y - d1.y * d2.x) / (speed * speed * speed),
            t,
        });
    }
    Ok(out)
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    let (dx, dy) = (a[0] - b[0], a[1] - b[1]);
    libm::sqrt(dx * dx + dy * dy)
}

/// Points at equal arc length along the polyline `line` (`segments` + 1 of
/// them, both ends included).
fn resample(line: &[[f64; 2]], segments: usize) -> Vec<[f64; 2]> {
    let mut along = Vec::with_capacity(line.len());
    let mut total = 0.0;
    along.push(0.0);
    for w in line.windows(2) {
        total += dist(w[0], w[1]);
        along.push(total);
    }
    let mut out = Vec::with_capacity(segments + 1);
    let mut j = 0usize;
    for k in 0..=segments {
        if k == segments {
            out.push(line[line.len() - 1]);
            break;
        }
        let target = total * k as f64 / segments as f64;
        while j + 2 < along.len() && along[j + 1] < target {
            j += 1;
        }
        let span = along[j + 1] - along[j];
        let f = if span > 0.0 {
            ((target - along[j]) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (a, b) = (line[j], line[j + 1]);
        out.push([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f]);
    }
    out
}

/// Builds the spline from mm through-points exactly as the loader does.
pub fn loader_spline(points_mm: &[[f32; 3]], closed: bool) -> Result<CubicSpline, String> {
    let through: Vec<Vec3> = points_mm
        .iter()
        .map(|&p| Vec3::from_array(mm3_to_m(p)))
        .collect();
    let built = if closed {
        CubicSpline::interpolate_closed(&through)
    } else {
        CubicSpline::interpolate(&through)
    };
    built.map_err(|e| format!("the rail does not construct: {e}"))
}

/// The first error, else the first warning, `pin_assets` reports for a
/// level guide through `points_mm`, without its `splines[0]` path.
pub fn loader_problem(points_mm: &[[f32; 3]]) -> Option<String> {
    let def = SplineDef {
        id: "rail".into(),
        kind: SplineKind::Guide,
        layer: 0,
        gauge: None,
        points: points_mm.to_vec(),
        roll_deg: None,
    };
    let (errors, warnings) = pin_assets::validate::check_spline(0, &def);
    errors.into_iter().chain(warnings).next().map(|e| e.message)
}

/// Smallest plan-view radius of curvature of `spline`, m (infinite on a
/// straight; 0 where the curve turns back).
pub fn min_radius(spline: &CubicSpline) -> f64 {
    let mut best = f64::INFINITY;
    let mut prev: Option<Vec3> = None;
    for k in 0..=spline.segment_count() * CHECK_SAMPLES {
        let t = (k as f64 / CHECK_SAMPLES as f64) as f32;
        let d1 = spline.tangent(t);
        if let Some(p) = prev
            && p.dot(d1) < 0.0
        {
            return 0.0;
        }
        prev = Some(d1);
        let (d1, d2) = (d1.as_dvec3(), spline.second_derivative(t).as_dvec3());
        let speed = libm::sqrt(d1.x * d1.x + d1.y * d1.y);
        if speed <= 1.0e-12 {
            return 0.0;
        }
        let k = (d1.x * d2.y - d1.y * d2.x).abs() / (speed * speed * speed);
        if k > 0.0 {
            best = best.min(1.0 / k);
        }
    }
    best
}

/// Builds one rail at `side` (+1 left of the direction of travel, −1
/// right), `half` mm from the centre line, with through-point spacing at
/// most `max_spacing` mm.
fn rail(
    samples: &[Sample],
    side: f64,
    half: f64,
    closed: bool,
    max_spacing: f64,
) -> Result<Rail, String> {
    let name = if side > 0.0 { "left" } else { "right" };
    let mut line = Vec::with_capacity(samples.len());
    let mut r_min = f64::INFINITY;
    for s in samples {
        let f = 1.0 - side * half * s.kappa;
        if f < MIN_OFFSET_FACTOR {
            return Err(format!(
                "the centre line bends with radius {:.1} mm near points[{}], too tight for \
                 the lane's half-width of {half:.1} mm (the {name} rail would fold); \
                 soften the bend or narrow the lane",
                1.0 / s.kappa.abs(),
                libm::round(s.t) as usize
            ));
        }
        if s.kappa != 0.0 {
            r_min = r_min.min(f / s.kappa.abs());
        }
        line.push([s.c[0] + side * half * s.n[0], s.c[1] + side * half * s.n[1]]);
    }
    if closed {
        // The periodic centre line ends exactly where it starts.
        let first = line[0];
        let last = line.len() - 1;
        line[last] = first;
    }
    let length: f64 = line.windows(2).map(|w| dist(w[0], w[1])).sum();
    if length > MAX_SPLINE_LENGTH_MM {
        return Err(format!(
            "the {name} rail is {:.1} m long; a spline may be at most {} m long: split the \
             lane into several lanes",
            length / 1000.0,
            MAX_SPLINE_LENGTH_MM / 1000.0
        ));
    }
    let mut spacing = max_spacing.min(SPACING_PER_RADIUS * r_min);
    if spacing < MIN_RAIL_SPACING_MM {
        return Err(format!(
            "the {name} rail bends with radius {r_min:.2} mm, too tight for a guide"
        ));
    }
    for _attempt in 0..4 {
        let min_segments = if closed { 3 } else { 1 };
        let segments = ((length / spacing).ceil() as usize).max(min_segments);
        if segments + 1 > MAX_SPLINE_POINTS {
            return Err(format!(
                "the {name} rail ({:.0} mm) needs {} through-points at {spacing:.1} mm spacing, \
                 more than the {MAX_SPLINE_POINTS} a spline may have; shorten the lane or \
                 soften its bends",
                length,
                segments + 1
            ));
        }
        let pts = resample(&line, segments);
        let mut points_mm: Vec<[f32; 3]> = pts
            .iter()
            .map(|p| [p[0] as f32, p[1] as f32, 0.0])
            .collect();
        if closed {
            let last = points_mm.len() - 1;
            points_mm[last] = points_mm[0];
        }
        if points_mm.windows(2).any(|w| w[0] == w[1]) {
            return Err(format!("the {name} rail repeats a through-point"));
        }
        let spline_m = loader_spline(&points_mm, closed)?;
        let guide = GuideCurve::new(spline_m.clone(), 0.0)
            .map_err(|e| format!("the {name} rail does not construct: {e}"))?;
        if guide.reach() < NO_REACH_M {
            return Err(format!(
                "the {name} rail touches or crosses itself (no clearance can be proven); \
                 move the lane's stretches apart"
            ));
        }
        // The loader's spacing lint: spacing below the tightest bend.
        let chords: Vec<f64> = points_mm
            .windows(2)
            .map(|w| f64::from(Vec3::from_array(w[0]).distance(Vec3::from_array(w[1]))))
            .collect();
        let widest = chords.iter().fold(0.0f64, |a, &b| a.max(b));
        let narrowest = chords.iter().fold(f64::INFINITY, |a, &b| a.min(b));
        let radius_mm = min_radius(&spline_m) * 1000.0;
        if widest < 0.9 * radius_mm && widest <= 2.5 * narrowest {
            // The loader's own check, errors and authoring warnings alike
            // (4.3 fix round 1: length, self-crossing, hairline gaps).
            if let Some(problem) = loader_problem(&points_mm) {
                return Err(format!("the {name} rail would not load cleanly: {problem}"));
            }
            return Ok(Rail {
                points_mm,
                spline_m,
            });
        }
        spacing *= 0.5;
        if spacing < MIN_RAIL_SPACING_MM {
            break;
        }
    }
    Err(format!(
        "the {name} rail bends too tightly for evenly spaced through-points; soften the bend"
    ))
}

/// Both rails of a lane through centre points `points` (mm), `width` mm
/// apart. The caller has checked the inputs are finite, in range and free
/// of repeated points, and that a closed lane ends exactly on its start.
pub fn lane_rails(points: &[[f32; 2]], width: f32, closed: bool) -> Result<[Rail; 2], String> {
    let through: Vec<Vec3> = points.iter().map(|p| Vec3::new(p[0], p[1], 0.0)).collect();
    let centre = if closed {
        CubicSpline::interpolate_closed(&through)
    } else {
        CubicSpline::interpolate(&through)
    }
    .map_err(|e| format!("the centre line does not construct: {e}"))?;
    let samples = centre_samples(&centre)?;
    let half = f64::from(width) * 0.5;
    let left = rail(&samples, 1.0, half, closed, MAX_RAIL_SPACING_MM)?;
    let right = rail(&samples, -1.0, half, closed, MAX_RAIL_SPACING_MM)?;
    Ok([left, right])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_lane() {
        let [l, r] = lane_rails(&[[0.0, 0.0], [0.0, 100.0]], 30.0, false).unwrap();
        assert_eq!(l.points_mm[0], [-15.0, 0.0, 0.0]);
        assert_eq!(r.points_mm[0], [15.0, 0.0, 0.0]);
        assert!(l.points_mm.len() >= 6);
        assert!(!l.spline_m.is_closed());
    }

    #[test]
    fn closed_ring_lane() {
        let n = 12;
        let mut pts: Vec<[f32; 2]> = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * core::f32::consts::TAU;
                [200.0 + 100.0 * a.cos(), 300.0 + 100.0 * a.sin()]
            })
            .collect();
        pts.push(pts[0]);
        let [l, r] = lane_rails(&pts, 40.0, true).unwrap();
        for rail in [&l, &r] {
            assert!(rail.spline_m.is_closed());
            assert_eq!(rail.points_mm[0], *rail.points_mm.last().unwrap());
        }
        let inner = min_radius(&l.spline_m) * 1000.0;
        assert!((inner - 80.0).abs() < 4.0, "{inner}");
    }

    #[test]
    fn too_tight_for_width() {
        let e = lane_rails(&[[0.0, 0.0], [50.0, 10.0], [0.0, 20.0]], 40.0, false).unwrap_err();
        assert!(e.contains("too tight") || e.contains("turns back"), "{e}");
    }

    #[test]
    fn too_long() {
        // A long sinuous lane: tight bends force a short spacing.
        let pts: Vec<[f32; 2]> = (0..300)
            .map(|i| [i as f32 * 30.0, if i % 2 == 0 { 0.0 } else { 30.0 }])
            .collect();
        let e = lane_rails(&pts, 2.0, false).unwrap_err();
        assert!(e.contains("4096"), "{e}");
    }

    #[test]
    fn resample_is_even() {
        let line = [[0.0, 0.0], [1.0, 0.0], [3.0, 0.0]];
        let r = resample(&line, 3);
        assert_eq!(r, vec![[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [3.0, 0.0]]);
    }

    #[test]
    fn min_radius_of_line_is_infinite() {
        let s = loader_spline(&[[0.0; 3], [100.0, 0.0, 0.0]], false).unwrap();
        assert_eq!(min_radius(&s), f64::INFINITY);
    }
}
