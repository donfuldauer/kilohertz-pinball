//! Near-linear forms of the two pairwise tests of a rising or falling
//! guide's construction: whether its plan view folds back onto itself, and
//! the edge term of its wall's reach. Both look at the same sample pairs as
//! the pairwise originals (kept in the tests as references), but skip whole
//! blocks of pairs that provably cannot decide anything, so that a sloped
//! guide of 2 000 segments builds in milliseconds instead of seconds.

use glam::DVec3;

use super::curve_bounds::arc_bounds;
use super::spline::CubicSpline;
use super::wireform::{MIN_SLOPED_GUIDE_REACH, curvature_bound, segment_distance};

/// Points per leaf of the trees below.
const LEAF: usize = 8;

/// A binary tree of point blocks: per node the index range and a capsule
/// (segment from its first to its last point, and the largest distance of
/// any of its points from that segment), which holds every point exactly.
struct Capsules {
    points: Vec<DVec3>,
    /// `levels[0]` per leaf of [`LEAF`] points, then pairs:
    /// (first index, end index, capsule radius).
    levels: Vec<Vec<(usize, usize, f64)>>,
}

impl Capsules {
    fn new(points: Vec<DVec3>) -> Self {
        let n = points.len();
        let capsule = |first: usize, end: usize| {
            let (p, q) = (points[first], points[end - 1]);
            let radius = points[first..end]
                .iter()
                .map(|&y| segment_distance(y, p, q))
                .fold(0.0f64, f64::max);
            (first, end, radius * (1.0 + 1.0e-9) + 1.0e-12)
        };
        let mut levels = vec![
            (0..n)
                .step_by(LEAF)
                .map(|first| capsule(first, (first + LEAF).min(n)))
                .collect::<Vec<_>>(),
        ];
        while levels[levels.len() - 1].len() > 1 {
            let below = &levels[levels.len() - 1];
            let up = below
                .chunks(2)
                .map(|pair| capsule(pair[0].0, pair[pair.len() - 1].1))
                .collect();
            levels.push(up);
        }
        Self { points, levels }
    }

    fn top(&self) -> (usize, usize) {
        (self.levels.len() - 1, 0)
    }

    /// Distance between the first and last point of a node.
    fn length(&self, level: usize, index: usize) -> f64 {
        let (first, end, _) = self.levels[level][index];
        self.points[first].distance(self.points[end - 1])
    }

    fn node(&self, level: usize, index: usize) -> (usize, usize, f64) {
        self.levels[level][index]
    }

    fn children(&self, level: usize, index: usize) -> impl Iterator<Item = (usize, usize)> {
        let below = level.wrapping_sub(1);
        let count = if level == 0 {
            0
        } else {
            self.levels[below].len()
        };
        [2 * index, 2 * index + 1]
            .into_iter()
            .filter(move |&j| j < count)
            .map(move |j| (below, j))
    }
}

/// [`plan_view_folds_pairwise`](super::wireform) in near-linear time: the
/// same pairs of every second sample, the same arc lengths (chord sums of
/// the samples) and the same test, `|y − x| < 0.98 · keep(along)`.
///
/// Pairs that cannot fold are skipped in blocks:
/// * farther apart than `0.98 · 2ρ` (`keep ≤ 2ρ`), by the capsules;
/// * at most `π/κ` apart along the curve (by an upper bound of the arc
///   length, either way round a closed curve; `κ` a bound of the
///   curvature, `ρ ≤ 1/κ`): by Schur's comparison theorem their chord is at
///   least `2 sin(κℓ/2)/κ` for the true arc length `ℓ ≥ along`, which is at
///   least `2ρ sin(min(along/2ρ, π/2))` (the function `2r sin(ℓ/2r)` grows
///   with `r` and with `ℓ ≤ πr`, and `2R sin(πρ/2R) ≥ 2ρ` for `R ≥ ρ`).
///
/// A curvature bound above `1/ρ` (a bend tighter than the reach the guide
/// needs, which the plan view's reach estimate already rejects) is
/// reported as a fold.
pub(super) fn plan_view_folds(flat: &CubicSpline, closed: bool) -> bool {
    let rho = f64::from(MIN_SLOPED_GUIDE_REACH);
    let (upper_arc, kappa) = arc_bounds(flat);
    // (A NaN bound counts as too tight, too.)
    if kappa * rho > 1.0 || kappa.is_nan() {
        return true;
    }
    let schur = if kappa > 0.0 {
        core::f64::consts::PI / kappa
    } else {
        f64::INFINITY
    };
    let samples = flat.samples();
    let last = samples.len() - 1;
    // Chord sums up to every sample, in the original order of summation.
    let mut chord = Vec::with_capacity(samples.len());
    let mut total = 0.0f64;
    chord.push(0.0);
    for pair in samples.windows(2) {
        total += pair[1].point.as_dvec3().distance(pair[0].point.as_dvec3());
        chord.push(total);
    }
    let stride = 2;
    let count = last / stride + 1;
    let index = |j: usize| (j * stride).min(last);
    let tree = Capsules::new(
        (0..count)
            .map(|j| samples[index(j)].point.as_dvec3())
            .collect(),
    );
    let reach = 0.98 * 2.0 * rho;
    let u_total = upper_arc[upper_arc.len() - 1];
    // Whether every pair (a, b) with b in first..end (all after a) is too
    // short along the curve to fold.
    let short = |a: usize, first: usize, end: usize| {
        let (ia, i_lo, i_hi) = (index(a), index(first), index(end - 1));
        let direct = upper_arc[i_hi] - upper_arc[ia];
        if direct <= schur {
            return true;
        }
        // The other way round: at most the total less the chord sum.
        closed && u_total - (chord[i_lo] - chord[ia]) <= schur
    };
    let mut stack = Vec::new();
    for a in 0..count {
        let x = tree.points[a];
        stack.clear();
        stack.push(tree.top());
        while let Some((level, node)) = stack.pop() {
            let (start, end, radius) = tree.node(level, node);
            if end <= a + 1 {
                continue; // only pairs with b > a, as the original
            }
            let first = start.max(a + 1);
            let (p, q) = (tree.points[start], tree.points[end - 1]);
            if segment_distance(x, p, q) - radius >= reach || short(a, first, end) {
                continue;
            }
            if level > 0 {
                stack.extend(tree.children(level, node));
                continue;
            }
            for b in first..end {
                let arc = chord[index(b)] - chord[index(a)];
                let along = if closed { arc.min(total - arc) } else { arc };
                let keep = 2.0
                    * rho
                    * (along / (2.0 * rho))
                        .min(core::f64::consts::FRAC_PI_2)
                        .sin();
                if tree.points[b].distance(x) < 0.98 * keep {
                    return true;
                }
            }
        }
    }
    false
}

/// The frame of the wall at an edge point: horizontal unit normal `n` and
/// the in-plane unit vector `up` perpendicular to the edge (see the
/// reference `wall_reach_pairwise`).
struct EdgeFrame {
    x: DVec3,
    up: DVec3,
    normal: DVec3,
    /// Horizontal part of `up`, and `up · Z` (the horizontal length of the
    /// unit tangent).
    up_h: DVec3,
    flat: f64,
}

/// Upper bound of both edge terms of the pairs `(x, y)`, `d = y − x`, with
/// `|d_h|² ≥ h`, `|d · n| ≤ across` and, per sign `s` of the reference,
/// `s d · up ≤ a[s]`, `s d · up_h ≤ c[s]` and the reference's denominator
/// `lowest_s ≥ low[s] ≥ h`.
///
/// The side term is `2 |d · n| / lowest_s`. The other is `2 max(0, F)`,
/// `F = (a + bλ) / (|d_h|² + (e + λ)²)` over the reference's allowed `λ`
/// (`a = s d · up`, `b = s · flat`, `e = d_z`), where `bλ ≤ 0`; so
/// `F ≤ max(0, a) / h`; and, with `μ = e + λ` and `a − b e = s d · up_h`
/// (as `up_z = flat`), `F = (s d · up_h + bμ) / (|d_h|² + μ²)
/// ≤ max(0, s d · up_h) / h + flat / (2 √h)`.
fn term_bound(flat: f64, h: f64, across: f64, a: [f64; 2], c: [f64; 2], low: [f64; 2]) -> f64 {
    let mut worst = 0.0f64;
    for s in 0..2 {
        let side = 2.0 * across / low[s];
        let lift = (a[s].max(0.0) / h).min(c[s].max(0.0) / h + 0.5 * flat / h.sqrt());
        let off = 2.0 * lift;
        worst = worst.max(side * side + off * off);
    }
    worst.sqrt() * (1.0 + 1.0e-9)
}

/// The edge term of the pair `(x, y)`, `d = y − x`, exactly as in the
/// pairwise reference (`wall_reach_pairwise`); `None` for the same point.
fn edge_term(frame: &EdgeFrame, d: DVec3) -> Option<f64> {
    let horizontal = d.x * d.x + d.y * d.y;
    if horizontal <= 1.0e-16 {
        return None;
    }
    let e = d.z;
    let across = d.dot(frame.normal).abs();
    let mut term = 0.0f64;
    for sign in [1.0f64, -1.0] {
        let allowed = |lambda: f64| lambda * sign <= 0.0;
        let lowest = if allowed(-e) {
            horizontal
        } else {
            horizontal + e * e
        };
        let side = 2.0 * across / lowest;
        let m = frame.up * sign;
        let (a, b) = (d.dot(m), m.z);
        let value = |lambda: f64| {
            let rise = e + lambda;
            (a + b * lambda) / (horizontal + rise * rise)
        };
        let mut out = value(0.0);
        let disc = ((a - b * e) * (a - b * e) + b * b * horizontal).sqrt();
        if b.abs() > 1.0e-12 {
            for root in [(-a + disc) / b, (-a - disc) / b] {
                if root.is_finite() && allowed(root) {
                    out = out.max(value(root));
                }
            }
        }
        let off = 2.0 * out.max(0.0);
        term = term.max((side * side + off * off).sqrt());
    }
    Some(term)
}

/// Groups of consecutive frames (a binary tree over them): a frame in the
/// middle, and how far any frame of the group is from it, in position and
/// in its three vectors.
struct FrameGroup {
    first: usize,
    frame: EdgeFrame,
    centre: DVec3,
    lambda: f64,
    normal: f64,
    up: f64,
    up_h: f64,
    /// Largest `flat` of the group.
    flat: f64,
}

struct FrameGroups {
    levels: Vec<Vec<FrameGroup>>,
}

impl FrameGroups {
    fn new(frames: &[EdgeFrame]) -> Self {
        let group = |first: usize, end: usize| {
            let m = &frames[(first + end) / 2];
            let mut g = FrameGroup {
                first,
                frame: EdgeFrame {
                    x: m.x,
                    up: m.up,
                    normal: m.normal,
                    up_h: m.up_h,
                    flat: m.flat,
                },
                centre: m.x,
                lambda: 0.0,
                normal: 0.0,
                up: 0.0,
                up_h: 0.0,
                flat: 0.0,
            };
            for f in &frames[first..end] {
                g.lambda = g.lambda.max(f.x.distance(m.x));
                g.normal = g.normal.max(f.normal.distance(m.normal));
                g.up = g.up.max(f.up.distance(m.up));
                g.up_h = g.up_h.max(f.up_h.distance(m.up_h));
                g.flat = g.flat.max(f.flat);
            }
            let pad = |v: f64| v * (1.0 + 1.0e-9) + 1.0e-15;
            g.lambda = pad(g.lambda);
            g.normal = pad(g.normal);
            g.up = pad(g.up);
            g.up_h = pad(g.up_h);
            g
        };
        let n = frames.len();
        let mut levels = vec![(0..n).map(|i| group(i, i + 1)).collect::<Vec<_>>()];
        let mut width = 1;
        while levels[levels.len() - 1].len() > 1 {
            width *= 2;
            levels.push(
                (0..n)
                    .step_by(width)
                    .map(|f| group(f, (f + width).min(n)))
                    .collect(),
            );
        }
        Self { levels }
    }
}

/// Relative tolerance of the edge-term search ([`wall_reach`]).
const EDGE_TOL: f64 = 0.01;

/// Reach of a rising or falling wall: `wall_reach_pairwise` (see there for
/// the bound) over the same sample pairs, without visiting most of them.
///
/// Per pair, both edge terms are at most
/// `2 √((w · n)² + max(0, ±w · up)²) / |w_h|²` (the `λ` part of the
/// numerator never helps, see the reference; the denominators are at least
/// `|w_h|²`). Over a capsule of points (segment `p q`, radius `R`) the
/// linear parts are at most their larger end value plus `R`, and `|w_h|`
/// at least the horizontal distance to the segment less `R`.
///
/// Decision and value:
/// * The guide is rejected exactly when the reference rejects it: a
///   capsule is skipped only if its bound is at most `0.9/ρ_min` (no pair
///   in it can push the reach below the minimum), and any pair above that
///   rejects at once.
/// * Values at most `τ = 0.9 · max(κ, 1/plan_reach)` cannot change the
///   result. Above `τ`, a capsule is also skipped when its bound is within
///   1 % of the largest term found (`far`); if that ever happened, the
///   term used is `1.01 · far`, which is at least the reference's, so the
///   reach is at most the reference's and at least 1/1.01 of it; and as
///   the reference's is then known to be at least `ρ_min`, it is raised to
///   `ρ_min` if below.
///
/// Returns 0 where the reference does (a cusp, a vertical tangent) and for
/// a rejected guide.
pub(super) fn wall_reach(spline: &CubicSpline, plan_reach: f32) -> f32 {
    let Some(curvature) = curvature_bound(spline) else {
        return 0.0;
    };
    let min_reach = f64::from(MIN_SLOPED_GUIDE_REACH);
    let plan = f64::from(plan_reach);
    if curvature * min_reach > 1.0 || plan < min_reach {
        return 0.0; // the reference's reach is below the minimum too
    }
    let reject = 0.9 / min_reach;
    let floor = 0.9 * curvature.max(1.0 / plan);
    let samples = spline.samples();
    let last = samples.len() - 1;
    let mut frames = Vec::with_capacity(last / 4 + 2);
    for xa in (0..=last).step_by(4).chain(core::iter::once(last)) {
        let x = samples[xa].point.as_dvec3();
        let tangent = samples[xa].tangent.as_dvec3().normalize();
        let flat = DVec3::new(tangent.x, tangent.y, 0.0).length();
        if flat <= 1.0e-9 {
            return 0.0;
        }
        let up = (DVec3::Z - tangent * tangent.z) / flat;
        frames.push(EdgeFrame {
            x,
            up,
            normal: DVec3::new(-tangent.y, tangent.x, 0.0) / flat,
            up_h: DVec3::new(up.x, up.y, 0.0),
            flat: up.z,
        });
    }
    let tree = Capsules::new(
        (0..=last)
            .step_by(2)
            .map(|yb| samples[yb].point.as_dvec3())
            .collect(),
    );
    let groups = FrameGroups::new(&frames);
    let mut far = 0.0f64;
    let mut lossy = false;
    // Pairs (group of frames, node of points), as a dual tree: a pair is
    // dropped when its bound rules out every pair of its frames and points.
    let mut stack = vec![(groups.levels.len() - 1, 0, tree.levels.len() - 1, 0)];
    while let Some((xl, xi, yl, yi)) = stack.pop() {
        let group = &groups.levels[xl][xi];
        let (first, end, radius) = tree.node(yl, yi);
        let (p, q) = (
            tree.points[first] - group.centre,
            tree.points[end - 1] - group.centre,
        );
        let flat = |v: DVec3| DVec3::new(v.x, v.y, 0.0);
        let slack = radius + group.lambda;
        let near = segment_distance(DVec3::ZERO, flat(p), flat(q)) - slack;
        if near > 0.0 {
            let reach = p.length().max(q.length()) + slack;
            let frame = &group.frame;
            let across = p.dot(frame.normal).abs().max(q.dot(frame.normal).abs())
                + slack
                + reach * group.normal;
            let (pu, qu) = (p.dot(frame.up), q.dot(frame.up));
            let (ph, qh) = (p.dot(frame.up_h), q.dot(frame.up_h));
            let up = slack + reach * group.up;
            let uh = slack + reach * group.up_h;
            let a = [pu.max(qu) + up, -pu.min(qu) + up];
            let c = [ph.max(qh) + uh, -ph.min(qh) + uh];
            let h = near * near;
            let bound = term_bound(group.flat, h, across, a, c, [h; 2]);
            let threshold = floor.max(((1.0 + EDGE_TOL) * far).min(reject));
            if bound <= threshold {
                lossy |= bound > floor.max(far);
                continue;
            }
        }
        if xl > 0 && (yl == 0 || group.lambda >= 0.5 * tree.length(yl, yi)) {
            for j in [2 * xi, 2 * xi + 1] {
                if j < groups.levels[xl - 1].len() {
                    stack.push((xl - 1, j, yl, yi));
                }
            }
            continue;
        }
        if yl > 0 {
            stack.extend(tree.children(yl, yi).map(|(l, j)| (xl, xi, l, j)));
            continue;
        }
        // One frame and one leaf of points: pair by pair.
        let frame = &frames[group.first];
        for y in &tree.points[first..end] {
            let d = *y - frame.x;
            // The same bound pair by pair first: the exact term only where
            // it could matter.
            let h = d.x * d.x + d.y * d.y;
            if h > 1.0e-16 {
                let (u, uh) = (d.dot(frame.up), d.dot(frame.up_h));
                let e = d.z;
                let low = [
                    if e >= 0.0 { h } else { h + e * e },
                    if e <= 0.0 { h } else { h + e * e },
                ];
                let across = d.dot(frame.normal).abs();
                let bound = term_bound(frame.flat, h, across, [u, -u], [uh, -uh], low);
                let threshold = floor.max(((1.0 + EDGE_TOL) * far).min(reject));
                if bound <= threshold {
                    lossy |= bound > floor.max(far);
                    continue;
                }
            }
            if let Some(term) = edge_term(frame, d) {
                far = far.max(term);
            }
        }
        if far > reject {
            return 0.0;
        }
    }
    let used = if lossy { (1.0 + EDGE_TOL) * far } else { far };
    let inverse = curvature.max(used / 0.9);
    let edge = if inverse > 0.0 {
        (1.0 / inverse) as f32
    } else {
        f32::INFINITY
    };
    let reach = plan_reach.min(edge);
    if lossy {
        reach.max(MIN_SLOPED_GUIDE_REACH)
    } else {
        reach
    }
}
