//! Guide authoring lints: warnings only, never errors (human ruling, end of
//! Wave 3, item 4; DECISIONS.md "4.1 — fix task H").
//!
//! * (b) a bend whose radius of curvature is not above the through-point
//!   spacings next to it (the guide's closest-point sampling bounds rely on
//!   spacing below the radius);
//! * (c) a ball profile wider than the tightest bend of a sloped guide (it
//!   slides slowly there, in single steps);
//! * (d) a level guide whose open ends nearly meet (a hairline gap), or one
//!   of whose open ends comes within that distance of the rest of the curve;
//!   and, separately, a level guide that doubles back or touches itself
//!   (no proven reach) for another reason.
//!
//! (a), uneven spacing, stays in `rules.rs` next to the spline build.

use glam::Vec3;
use pin_physics::world::{CubicSpline, GuideCurve};

/// Curvature samples per spline segment (one segment per through-point
/// spacing; the parameter is uniform per segment).
pub(super) const CURVATURE_SAMPLES: usize = 64;
/// Golden-section steps that refine the largest sample of a segment.
const REFINE_STEPS: usize = 40;
/// Open ends of a level guide closer than this (m) to each other, or an
/// open end closer than this to the rest of its own curve, are a gap.
pub(super) const HAIRLINE_GAP_M: f32 = 1.0e-3;
/// Along-curve distance (m) from an open end beyond which the curve counts
/// as "the rest of the curve" for the end-near-wall check: a curve can only
/// come back within [`HAIRLINE_GAP_M`] of an end after this much length by
/// turning back on itself.
pub(super) const END_EXCLUSION_M: f32 = 4.0 * HAIRLINE_GAP_M;
/// Target length (m) of one polyline piece for the end-near-wall and
/// self-touching distance checks (the sagitta of a 0.5 mm chord on a 5 mm
/// radius is 6 µm).
const PIECE_M: f32 = 0.5e-3;
/// Polyline pieces per spline segment at least (so short segments keep
/// their shape) and in total at most (the piece length grows instead).
const MIN_PIECES: usize = 4;
const MAX_PIECES: usize = 1 << 17;
/// Sub-chords per segment used to estimate its length.
const LENGTH_PROBES: usize = 8;
/// A reach below this (m) counts as none: `GuideCurve::reach()` is exactly 0
/// for an end within ~0.1 mm of its own wall, but a curve crossing itself
/// between samples can come out at a few nanometres.
pub(super) const NO_REACH_M: f32 = 1.0e-6;

/// The tightest bend of a curve or of one segment.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct Bend {
    /// Smallest radius of curvature, metres (0 at a cusp or reversal,
    /// infinite on a straight).
    pub radius_m: f64,
    /// Curve parameter where it occurs (`P_i` at `t = i`).
    pub t: f64,
}

/// Curvature at `t`: the larger of the space curve's and the plan view's
/// (`x`, `y`), so that a sloped guide is judged by the bend its wall (a
/// vertical cylinder over the plan view) really has. Infinite where the
/// relevant speed vanishes (a cusp, or a vertical run).
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
        f64::INFINITY
    };
    space.max(plan)
}

/// The tightest bend of every segment (`segment_count()` entries).
///
/// Sampling: [`CURVATURE_SAMPLES`] + 1 evenly spaced parameters per segment
/// (both ends included, so each through-point spacing is probed at 1/64 of
/// its length), then a golden-section refinement of the segment's largest
/// sample over its two neighbouring sample intervals. A tangent that turns
/// by more than 90° between two neighbouring samples is a cusp or a
/// reversal (a collinear spline that runs back over itself has zero
/// curvature everywhere but turns by 180°): radius 0 there. A bend tight
/// enough to matter (radius ≤ spacing) turns the tangent by a sizeable angle
/// over a parameter interval much wider than 1/64, so the samples see it.
pub(super) fn segment_bends(spline: &CubicSpline) -> Vec<Bend> {
    let h = 1.0 / CURVATURE_SAMPLES as f64;
    (0..spline.segment_count())
        .map(|i| {
            let mut best = (0.0f64, i as f64);
            let mut previous: Option<Vec3> = None;
            for j in 0..=CURVATURE_SAMPLES {
                let t = i as f64 + j as f64 * h;
                let tangent = spline.tangent(t as f32);
                if let Some(p) = previous
                    && p.dot(tangent) < 0.0
                {
                    best = (f64::INFINITY, t - 0.5 * h);
                }
                previous = Some(tangent);
                let k = curvature(spline, t);
                if k > best.0 {
                    best = (k, t);
                }
            }
            if best.0.is_finite() && best.0 > 0.0 {
                best = refine(spline, best, h, i);
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
        })
        .collect()
}

/// Golden-section search for the curvature maximum on `[t - h, t + h]`
/// within segment `i`.
fn refine(spline: &CubicSpline, best: (f64, f64), h: f64, i: usize) -> (f64, f64) {
    let (lo, hi) = (i as f64, i as f64 + 1.0);
    let (mut a, mut b) = ((best.1 - h).max(lo), (best.1 + h).min(hi));
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
    let mut out = best;
    for (k, t) in [(kc, c), (kd, d)] {
        if k > out.0 {
            out = (k, t);
        }
    }
    out
}

/// The tightest bend of the whole curve.
#[cfg(test)]
fn tightest_bend(spline: &CubicSpline) -> Bend {
    tightest_of(&segment_bends(spline))
}

/// The tightest of `bends` (radius infinite when there are none).
pub(super) fn tightest_of(bends: &[Bend]) -> Bend {
    bends.iter().copied().fold(
        Bend {
            radius_m: f64::INFINITY,
            t: 0.0,
        },
        |a, b| if b.radius_m < a.radius_m { b } else { a },
    )
}

/// Lint (b): the segment whose bend is tightest relative to the spacings
/// next to it (its own and its neighbours'), when that spacing is at or
/// above the bend's radius. Returns (bend, segment of the offending
/// spacing, that spacing in metres).
pub(super) fn spacing_lint(
    bends: &[Bend],
    spacing: &[f32],
    closed: bool,
) -> Option<(Bend, usize, f32)> {
    let n = bends.len();
    let mut worst: Option<(f64, Bend, usize, f32)> = None;
    for (i, bend) in bends.iter().enumerate() {
        let neighbours = [i.checked_sub(1).or(closed.then(|| n - 1)), Some(i), {
            let j = i + 1;
            if j < n { Some(j) } else { closed.then_some(0) }
        }];
        for j in neighbours.into_iter().flatten() {
            let s = spacing[j];
            if f64::from(s) < bend.radius_m {
                continue;
            }
            let ratio = f64::from(s) / bend.radius_m; // inf at a cusp
            if worst.is_none_or(|w| ratio > w.0) {
                worst = Some((ratio, *bend, j, s));
            }
        }
    }
    worst.map(|(_, b, j, s)| (b, j, s))
}

/// What lint (d) found on a level guide.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) enum GapLint {
    /// The open ends are this far apart (m), under [`HAIRLINE_GAP_M`].
    Hairline(f32),
    /// An open end (`at_start`) is this far (m) from the rest of the curve,
    /// under [`HAIRLINE_GAP_M`], near parameter `t`.
    EndNearWall { distance: f32, t: f32 },
    /// No proven reach otherwise: the curve doubles back or touches itself
    /// near parameter `t`.
    DoublesBack { t: f32 },
}

/// Polyline samples of the curve: (point, along-curve distance, parameter).
///
/// Pieces are about [`PIECE_M`] long (at least [`MIN_PIECES`] per segment;
/// if the curve is so long that this would exceed [`MAX_PIECES`] pieces,
/// longer pieces), so the number of samples is linear in the number of
/// through-points plus the curve's length in half-millimetres, and pieces
/// have comparable lengths, which the grid in [`closest_far_pair`] relies on.
fn polyline(spline: &CubicSpline) -> Vec<(Vec3, f32, f32)> {
    let n = spline.segment_count();
    let lengths: Vec<f32> = (0..n)
        .map(|i| {
            (0..LENGTH_PROBES)
                .map(|k| {
                    let t = |k: usize| i as f32 + k as f32 / LENGTH_PROBES as f32;
                    spline.point(t(k)).distance(spline.point(t(k + 1)))
                })
                .sum()
        })
        .collect();
    let total: f32 = lengths.iter().sum();
    let piece = PIECE_M.max(total / MAX_PIECES as f32);
    let mut out = Vec::new();
    let mut along = 0.0f32;
    let mut previous = spline.point(0.0);
    out.push((previous, 0.0, 0.0));
    for (i, &len) in lengths.iter().enumerate() {
        let k = ((len / piece).ceil() as usize).clamp(MIN_PIECES, MAX_PIECES);
        for j in 1..=k {
            let t = i as f32 + j as f32 / k as f32;
            let p = spline.point(t);
            along += p.distance(previous);
            previous = p;
            out.push((p, along, t));
        }
    }
    out
}

fn point_segment_distance(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let s = if len2 > 0.0 {
        ((p - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * s)
}

/// Distance from the end at along-curve distance `end_along` (position `e`)
/// to the part of the polyline at least [`END_EXCLUSION_M`] along the curve
/// away from it, and the parameter of the nearest such sample.
fn end_clearance(line: &[(Vec3, f32, f32)], e: Vec3, end_along: f32) -> (f32, f32) {
    let mut best = (f32::INFINITY, 0.0);
    for w in line.windows(2) {
        let (a, b) = (w[0], w[1]);
        if (a.1 - end_along).abs() < END_EXCLUSION_M || (b.1 - end_along).abs() < END_EXCLUSION_M {
            continue;
        }
        let d = point_segment_distance(e, a.0, b.0);
        if d < best.0 {
            best = (d, a.2);
        }
    }
    best
}

/// Lint (d) for a built guide (`closed` = snapped closed by the loader;
/// `bends` from [`segment_bends`]). Level guides only.
pub(super) fn gap_lint(guide: &GuideCurve, closed: bool, bends: &[Bend]) -> Option<GapLint> {
    if !guide.is_level() {
        return None;
    }
    let spline = guide.spline();
    let line = polyline(spline);
    if !closed {
        let (first, last) = (line[0], line[line.len() - 1]);
        let gap = first.0.distance(last.0);
        if gap < HAIRLINE_GAP_M {
            return Some(GapLint::Hairline(gap));
        }
        // The nearest stretch of the curve to each open end; it only counts
        // as the end nearing the wall when the curve gets there without a
        // cusp or sub-millimetre bend (otherwise it simply doubles back).
        let doubled = |from: f32, to: f32| {
            let (lo, hi) = (f64::from(from.min(to)), f64::from(from.max(to)));
            bends
                .iter()
                .any(|b| b.radius_m < f64::from(HAIRLINE_GAP_M) && b.t >= lo && b.t <= hi)
        };
        let near = [first, last]
            .map(|end| (end.2, end_clearance(&line, end.0, end.1)))
            .into_iter()
            .filter(|&(t_end, (d, t))| d < HAIRLINE_GAP_M && !doubled(t_end, t))
            .map(|(_, near)| near)
            .fold((f32::INFINITY, 0.0), |a, b| if b.0 < a.0 { b } else { a });
        if near.0 < HAIRLINE_GAP_M {
            return Some(GapLint::EndNearWall {
                distance: near.0,
                t: near.1,
            });
        }
    }
    // Any two stretches of the curve, far apart along it, that come within
    // HAIRLINE_GAP_M of each other (a crossing, a fold, a reversal); and any
    // guide whose wall has no proven reach.
    let near = closest_far_pair(&line, closed).found;
    let tight = tightest_of(bends);
    if near.is_some() || guide.reach() < NO_REACH_M {
        // Where: a cusp or sub-millimetre bend, else the pair found.
        let t = if tight.radius_m < f64::from(HAIRLINE_GAP_M) {
            tight.t as f32
        } else {
            near.unwrap_or(0.0)
        };
        return Some(GapLint::DoublesBack { t });
    }
    None
}

/// Plan-view distance between segments `a0 a1` and `b0 b1`.
fn segment_distance(a0: Vec3, a1: Vec3, b0: Vec3, b1: Vec3) -> f32 {
    let flat = |v: Vec3| glam::Vec2::new(v.x, v.y);
    let (p, q, r, s) = (flat(a0), flat(a1), flat(b0), flat(b1));
    let cross = |u: glam::Vec2, v: glam::Vec2| u.perp_dot(v);
    let (d1, d2) = (q - p, s - r);
    let denom = cross(d1, d2);
    if denom != 0.0 {
        let t = cross(r - p, d2) / denom;
        let u = cross(r - p, d1) / denom;
        if (0.0..=1.0).contains(&t) && (0.0..=1.0).contains(&u) {
            return 0.0; // they cross
        }
    }
    let point = |x: glam::Vec2, a: glam::Vec2, b: glam::Vec2| {
        let ab = b - a;
        let len2 = ab.length_squared();
        let h = if len2 > 0.0 {
            ((x - a).dot(ab) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        x.distance(a + ab * h)
    };
    point(p, r, s)
        .min(point(q, r, s))
        .min(point(r, p, q))
        .min(point(s, p, q))
}

/// Result of [`closest_far_pair`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub(super) struct FarPair {
    /// Parameter of the earlier piece of a pair closer than
    /// [`HAIRLINE_GAP_M`] in plan view and at least [`END_EXCLUSION_M`]
    /// apart along the curve (around a closed one); `None` if there is none.
    pub found: Option<f32>,
    /// Piece pairs measured (a deterministic work count, for tests).
    pub measured: usize,
}

/// Finds two polyline pieces at least [`END_EXCLUSION_M`] apart along the
/// curve that come within [`HAIRLINE_GAP_M`] of each other in plan view,
/// stopping at the first.
///
/// Uniform grid, cell side = the longest piece + [`HAIRLINE_GAP_M`]: each
/// piece is entered in the cells its bounding box, grown by the gap,
/// touches (at most 2 × 2), so two pieces within the gap share a cell.
/// Within a cell the pieces are ordered by their along-curve position and
/// each is measured only against pieces at least [`END_EXCLUSION_M`]
/// further along (a binary search), so stretches of the same passage are
/// never compared. Pieces of different passages measured without a hit are
/// at least the gap apart, and only a bounded number of passages that far
/// apart fit through one cell, so the work is linear in the number of pieces
/// whatever the orientation or clustering (a crowded cell ends at its first
/// hit).
pub(super) fn closest_far_pair(line: &[(Vec3, f32, f32)], closed: bool) -> FarPair {
    let total = line[line.len() - 1].1;
    let pieces = line.len() - 1;
    let longest = line
        .windows(2)
        .map(|w| (w[1].0.x - w[0].0.x).abs().max((w[1].0.y - w[0].0.y).abs()))
        .fold(0.0f32, f32::max);
    let cell = longest + HAIRLINE_GAP_M;
    let (min_x, min_y) = line
        .iter()
        .fold((f32::INFINITY, f32::INFINITY), |(x, y), p| {
            (x.min(p.0.x), y.min(p.0.y))
        });
    let index = |v: f32, lo: f32| (((v - lo) / cell).floor().max(0.0)) as i64;
    let mut grid: std::collections::HashMap<(i64, i64), Vec<usize>> =
        std::collections::HashMap::with_capacity(pieces);
    for (i, w) in line.windows(2).enumerate() {
        let g = HAIRLINE_GAP_M;
        let (x0, x1) = (w[0].0.x.min(w[1].0.x) - g, w[0].0.x.max(w[1].0.x) + g);
        let (y0, y1) = (w[0].0.y.min(w[1].0.y) - g, w[0].0.y.max(w[1].0.y) + g);
        for cx in index(x0, min_x)..=index(x1, min_x) {
            for cy in index(y0, min_y)..=index(y1, min_y) {
                grid.entry((cx, cy)).or_default().push(i);
            }
        }
    }
    let mut measured = 0;
    // Deterministic order: cells by key (HashMap order is not).
    let mut cells: Vec<_> = grid.into_iter().collect();
    cells.sort_unstable_by_key(|(k, _)| *k);
    for (_, members) in cells {
        // Entered in polyline order, so already sorted along the curve.
        for a in 0..members.len() {
            let i = members[a];
            let start = line[i + 1].1;
            // Pieces at least END_EXCLUSION_M further along (and, around a
            // closed curve, not within it the other way round).
            let first = members.partition_point(|&j| line[j].1 < start + END_EXCLUSION_M);
            for &j in &members[first.max(a + 1)..] {
                if closed && total - (line[j + 1].1 - line[i].1) < END_EXCLUSION_M {
                    break; // the rest are closer the other way round
                }
                measured += 1;
                let d = segment_distance(line[i].0, line[i + 1].0, line[j].0, line[j + 1].0);
                if d < HAIRLINE_GAP_M {
                    return FarPair {
                        found: Some(line[i].2),
                        measured,
                    };
                }
            }
        }
    }
    FarPair {
        found: None,
        measured,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn open(pts: &[[f32; 3]]) -> CubicSpline {
        let pts: Vec<Vec3> = pts.iter().map(|&p| Vec3::from_array(p)).collect();
        CubicSpline::interpolate(&pts).unwrap()
    }

    #[test]
    fn circle_radius_is_recovered() {
        let bend = tightest_bend(&circle(0.1, 32));
        assert!((bend.radius_m - 0.1).abs() < 1.0e-3, "{bend:?}");
    }

    #[test]
    fn straight_line_has_no_bend() {
        let s = open(&[[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]]);
        assert_eq!(tightest_bend(&s).radius_m, f64::INFINITY);
    }

    #[test]
    fn collinear_reversal_is_a_cusp() {
        // Runs to 100 mm, back to 50 mm, on to 150 mm: zero curvature, but
        // the tangent flips twice.
        let s = open(&[
            [0.0; 3],
            [0.1, 0.0, 0.0],
            [0.05, 0.0, 0.0],
            [0.15, 0.0, 0.0],
        ]);
        let bends = segment_bends(&s);
        let bend = tightest_of(&bends);
        assert_eq!(bend.radius_m, 0.0, "{bends:?}");
        assert!((0.5..2.5).contains(&bend.t), "{bend:?}");
        let spacing = [0.1, 0.05, 0.1];
        let (b, _, _) = spacing_lint(&bends, &spacing, false).unwrap();
        assert_eq!(b.radius_m, 0.0);
    }

    #[test]
    fn plan_view_counts_for_a_helix_like_curve() {
        let pts: Vec<Vec3> = (0..=8)
            .map(|i| {
                let a = i as f32 / 8.0 * core::f32::consts::FRAC_PI_2;
                Vec3::new(0.1 * a.cos(), 0.1 * a.sin(), 0.02 * i as f32)
            })
            .collect();
        let s = CubicSpline::interpolate(&pts).unwrap();
        let bend = tightest_bend(&s);
        assert!(bend.radius_m < 0.1 + 1.0e-3, "{bend:?}");
        assert_eq!(
            curvature(&open(&[[0.0; 3], [0.0, 0.0, 1.0]]), 0.5),
            f64::INFINITY
        );
    }

    #[test]
    fn spacing_lint_is_local() {
        // A tight bend between fine spacings, far from a coarse straight:
        // compared with its own neighbours only.
        let bends = [
            Bend {
                radius_m: f64::INFINITY,
                t: 0.5,
            },
            Bend {
                radius_m: f64::INFINITY,
                t: 1.5,
            },
            Bend {
                radius_m: 0.02,
                t: 2.5,
            },
            Bend {
                radius_m: f64::INFINITY,
                t: 3.5,
            },
        ];
        assert_eq!(spacing_lint(&bends, &[0.1, 0.01, 0.01, 0.01], false), None);
        let (b, j, s) = spacing_lint(&bends, &[0.1, 0.03, 0.01, 0.01], false).unwrap();
        assert_eq!((b.t, j, s), (2.5, 1, 0.03));
        // Closed: the first segment's neighbour wraps around.
        let bends = [
            Bend {
                radius_m: 0.02,
                t: 0.5,
            },
            Bend {
                radius_m: f64::INFINITY,
                t: 1.5,
            },
            Bend {
                radius_m: f64::INFINITY,
                t: 2.5,
            },
        ];
        assert_eq!(spacing_lint(&bends, &[0.01, 0.01, 0.05], false), None);
        assert_eq!(
            spacing_lint(&bends, &[0.01, 0.01, 0.05], true).map(|x| x.1),
            Some(2)
        );
    }

    #[test]
    fn gap_lints() {
        let guide = |pts: &[[f32; 3]]| GuideCurve::new(open(pts), 0.0).unwrap();
        let square = |end: [f32; 3]| {
            guide(&[
                [0.0; 3],
                [0.1, 0.0, 0.0],
                [0.1, 0.1, 0.0],
                [0.0, 0.1, 0.0],
                end,
            ])
        };
        let check = |g: &GuideCurve, closed: bool| gap_lint(g, closed, &segment_bends(g.spline()));
        let g = square([0.0, 0.0005, 0.0]);
        assert!(
            matches!(check(&g, false), Some(GapLint::Hairline(d)) if (d - 0.0005).abs() < 1e-6)
        );
        assert_eq!(check(&square([0.0, 0.02, 0.0]), false), None);
        let closed = GuideCurve::new(circle(0.1, 16), 0.0).unwrap();
        assert_eq!(check(&closed, true), None);
        let sloped = guide(&[[0.0; 3], [0.1, 0.0, 0.01]]);
        assert_eq!(check(&sloped, false), None);
        // A closed figure eight crosses itself: doubles back, near the crossing.
        let mut eight: Vec<Vec3> = (0..16)
            .map(|i| {
                let a = i as f32 / 16.0 * core::f32::consts::TAU;
                Vec3::new(0.1 * a.sin(), 0.05 * (2.0 * a).sin(), 0.0)
            })
            .collect();
        eight.push(eight[0]);
        let crossing =
            GuideCurve::new(CubicSpline::interpolate_closed(&eight).unwrap(), 0.0).unwrap();
        match check(&crossing, true) {
            Some(GapLint::DoublesBack { t }) => {
                let p = crossing.spline().point(t);
                assert!(p.length() < 0.01, "{t} {p}");
            }
            other => panic!("{other:?}"),
        }
        // An open Z whose diagonals cross (its reach is not 0): doubles back
        // near the crossing at (50, 50) mm.
        let z = guide(&[[0.0; 3], [0.1, 0.1, 0.0], [0.1, 0.0, 0.0], [0.0, 0.1, 0.0]]);
        match check(&z, false) {
            Some(GapLint::DoublesBack { t }) => {
                let p = z.spline().point(t);
                assert!(p.distance(Vec3::new(0.05, 0.05, 0.0)) < 0.02, "{t} {p}");
            }
            other => panic!("{other:?}"),
        }
        // Collinear reversal: doubles back at the cusp, no end near a wall
        // (both ends are 150 mm apart and away from the overlap's ends).
        let back = guide(&[
            [0.0; 3],
            [0.1, 0.0, 0.0],
            [0.05, 0.0, 0.0],
            [0.15, 0.0, 0.0],
        ]);
        assert!(back.reach() < NO_REACH_M);
        assert!(
            matches!(check(&back, false), Some(GapLint::DoublesBack { .. })),
            "{:?}",
            check(&back, false)
        );
    }

    fn spline_mm(pts: &[[f32; 2]]) -> CubicSpline {
        let pts: Vec<Vec3> = pts
            .iter()
            .map(|p| Vec3::new(p[0] * 1e-3, p[1] * 1e-3, 0.0))
            .collect();
        CubicSpline::interpolate(&pts).unwrap()
    }

    /// The four shapes of review round 2 (B5), 4096 points each.
    fn b5_shapes() -> Vec<(&'static str, CubicSpline)> {
        let n = 4096;
        let mut seed = 0x2545_F491u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        let blob: Vec<[f32; 2]> = (0..n).map(|_| [100.0 + rnd(), 100.0 + rnd()]).collect();
        vec![
            (
                "vertical",
                spline_mm(&(0..n).map(|i| [100.0, i as f32]).collect::<Vec<_>>()),
            ),
            (
                "diagonal",
                spline_mm(
                    &(0..n)
                        .map(|i| [i as f32 * 0.7, i as f32 * 0.7])
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "zigzag",
                spline_mm(
                    &(0..n)
                        .map(|i| [100.0 + (i % 2) as f32 * 0.5, i as f32])
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "blob",
                CubicSpline::interpolate(
                    &blob
                        .iter()
                        .map(|p| Vec3::new(p[0] * 1e-3, p[1] * 1e-3, 0.0))
                        .collect::<Vec<_>>(),
                )
                .unwrap_or_else(|_| spline_mm(&[[0.0, 0.0], [1.0, 0.0]])),
            ),
        ]
    }

    #[test]
    fn far_pair_search_is_linear_whatever_the_orientation() {
        // Deterministic work measure (review round 2, B5): piece pairs
        // measured. Straight runs and a zig-zag along any axis measure
        // at most one pair per piece; a blob stops at its first hit.
        for (name, spline) in b5_shapes() {
            let line = polyline(&spline);
            let pieces = line.len() - 1;
            assert!(pieces <= 4096 * 64, "{name}: {pieces}");
            let r = closest_far_pair(&line, false);
            match name {
                "blob" => {
                    assert!(r.found.is_some(), "{name}");
                    assert!(r.measured <= 64, "{name}: {r:?}");
                }
                _ => {
                    assert_eq!(r.found, None, "{name}");
                    // A cell can hold up to ~5 mm of one straight run, so a
                    // few far-along pieces of the same run meet: linear.
                    assert!(r.measured <= pieces, "{name}: {r:?} for {pieces} pieces");
                }
            }
        }
        // Two long parallel legs 1.5 mm apart joined by a turn: each piece
        // meets the other leg's few pieces in its cells, not all of them.
        let mut u: Vec<[f32; 2]> = (0..2000).map(|i| [0.0, 2000.0 - i as f32]).collect();
        u.extend((0..2000).map(|i| [1.5, i as f32]));
        let line = polyline(&spline_mm(&u));
        let r = closest_far_pair(&line, false);
        assert!(
            r.measured <= 32 * line.len(),
            "{r:?} for {} pieces",
            line.len()
        );
    }

    /// Release-build timing report for the B5 shapes (all lints); run with
    /// `cargo test --release -p pin_assets -- --ignored lint_timing`.
    #[test]
    #[ignore]
    fn lint_timing_report() {
        for (name, spline) in b5_shapes() {
            let Ok(guide) = GuideCurve::new(spline, 0.0) else {
                continue;
            };
            let start = std::time::Instant::now();
            let bends = segment_bends(guide.spline());
            let spacing = vec![1.0e-3f32; bends.len()];
            let _ = spacing_lint(&bends, &spacing, false);
            let _ = gap_lint(&guide, false, &bends);
            let took = start.elapsed();
            eprintln!("{name}: all lints {took:?}");
            assert!(
                took < std::time::Duration::from_millis(50),
                "{name}: {took:?}"
            );
        }
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

        /// Any spline: radii are non-negative (never NaN), the locations lie
        /// in their segments, and the lints never panic.
        #[test]
        fn random_splines(pts in proptest::collection::vec(
            (-1.0f32..1.0, -1.0f32..1.0, -0.1f32..0.1), 2..10), level in proptest::bool::ANY
        ) {
            let pts: Vec<Vec3> = pts
                .into_iter()
                .map(|(x, y, z)| Vec3::new(x, y, if level { 0.0 } else { z }))
                .collect();
            if let Ok(s) = CubicSpline::interpolate(&pts) {
                let bends = segment_bends(&s);
                for (i, b) in bends.iter().enumerate() {
                    proptest::prop_assert!(b.radius_m >= 0.0);
                    proptest::prop_assert!(b.t >= i as f64 && b.t <= i as f64 + 1.0, "{b:?}");
                }
                let spacing: Vec<f32> = pts.windows(2).map(|w| w[0].distance(w[1])).collect();
                let _ = spacing_lint(&bends, &spacing, false);
                if let Ok(g) = GuideCurve::new(s, 0.0) {
                    let _ = gap_lint(&g, false, &bends);
                }
            }
        }
    }
}
