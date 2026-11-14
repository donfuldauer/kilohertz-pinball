//! C² interpolating cubic splines stored as cubic Bézier segments (D13).
//!
//! A spline is authored as through-points `P_0 … P_n`. The global parameter
//! `t ∈ [0, n]` puts `P_i` at `t = i`; segment `i` covers `[i, i + 1]`.
//! Natural end conditions (zero second derivative at both ends) and a
//! tridiagonal solve give second-derivative continuity at interior knots.

use glam::{DVec3, Vec3};

/// Sample-table resolution: samples per segment (a power of two, so that
/// sample parameters are exact).
pub(crate) const SAMPLES_PER_SEGMENT: usize = 16;

/// Parameter-space convergence threshold of the Newton iterations.
const NEWTON_EPS: f32 = 1.0e-6;

/// Bisection depth of a sample interval that may hold several roots of
/// `g = (C - p) · C'` in [`CubicSpline::closest_point`]: the finest
/// sub-interval is `w = 1 / (SAMPLES_PER_SEGMENT * 2^5) = 1/512` of a
/// segment.
///
/// The depth limit is not a proof that a piece holds no closer point: at
/// this depth a piece with `g' <= 0` somewhere is dropped if its ends do not
/// bracket a root, and only one root is refined if they do. What is missed
/// there is bounded by `|g'_min| w²` in squared distance (see
/// `CubicSpline::search_interval`), which on the spline sizes the solver
/// meets is below f32 rounding (docs/DECISIONS.md, "closest_point
/// depth-limit bound").
const MULTI_ROOT_DEPTH: u32 = 5;

/// Errors from spline, wireform and guide construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplineError {
    /// Fewer than two through-points.
    TooFewPoints,
    /// A coordinate, roll angle or other input is NaN or infinite.
    NonFinite,
    /// Two consecutive through-points are identical.
    DuplicatePoints,
    /// The number of roll angles differs from the number of through-points.
    RollCountMismatch,
    /// The rail gauge is not strictly positive.
    InvalidGauge,
    /// The up hint is the zero vector.
    InvalidUpHint,
    /// The guide wall height is negative.
    InvalidHeight,
    /// A closed spline was requested but the last point differs from the first.
    NotClosed,
    /// A guide's first and last through-points coincide but the spline is not
    /// periodic (build it with [`CubicSpline::interpolate_closed`]).
    ClosedGuideNotPeriodic,
    /// A guide that rises or falls has a shape a ball cannot follow: its
    /// plan view (the curve seen from above) bends tighter than about 22 mm
    /// radius (in practice about 30 mm for a plain arc: a spline bends about
    /// a quarter tighter than the arc near its free ends), comes back onto
    /// or within about 44 mm of itself (a helix, a loop of more than one
    /// turn, a crossing), or runs vertically; or the curve bends up or down
    /// tighter than about 22 mm (the wall's top and bottom edges follow it).
    /// Model helical ramps as wireforms, and split or smooth other shapes.
    SlopedGuidePlanView,
}

impl core::fmt::Display for SplineError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::TooFewPoints => "a spline needs at least two through-points",
            Self::NonFinite => "spline input is NaN or infinite",
            Self::DuplicatePoints => "two consecutive through-points are identical",
            Self::RollCountMismatch => "one roll angle per through-point is required",
            Self::InvalidGauge => "rail gauge must be positive",
            Self::InvalidUpHint => "up hint must be a non-zero vector",
            Self::InvalidHeight => "guide height must not be negative",
            Self::NotClosed => "a closed spline must end on its first point",
            Self::ClosedGuideNotPeriodic => {
                "a closed guide needs a periodic spline (interpolate_closed)"
            }
            Self::SlopedGuidePlanView => {
                "a guide that rises or falls must have a simple plan view: no bend \
                 tighter than about 22 mm radius (about 30 mm for a plain arc, whose \
                 spline bends tighter near its ends), no stretch coming back onto or \
                 within about 44 mm of another (helix, loop, crossing), no vertical \
                 run; and the curve may not bend up or down tighter than about 22 mm; \
                 use a wireform for helical ramps"
            }
        })
    }
}

impl std::error::Error for SplineError {}

/// One cubic Bézier segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BezierSegment {
    /// Control points; `p[0]` and `p[3]` are consecutive through-points.
    pub p: [Vec3; 4],
}

impl BezierSegment {
    /// Position at local parameter `s ∈ [0, 1]` (exact at both ends).
    #[must_use]
    pub fn point(&self, s: f32) -> Vec3 {
        let a = 1.0 - s;
        let [p0, p1, p2, p3] = self.p;
        p0 * (a * a * a) + p1 * (3.0 * a * a * s) + p2 * (3.0 * a * s * s) + p3 * (s * s * s)
    }

    /// First derivative with respect to `s`.
    #[must_use]
    pub fn tangent(&self, s: f32) -> Vec3 {
        let a = 1.0 - s;
        let [p0, p1, p2, p3] = self.p;
        ((p1 - p0) * (a * a) + (p2 - p1) * (2.0 * a * s) + (p3 - p2) * (s * s)) * 3.0
    }
}

/// Result of a closest-point query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClosestPoint {
    /// Global curve parameter of the closest point.
    pub t: f32,
    /// The closest point.
    pub point: Vec3,
    /// Distance from the query point to `point` (metres).
    pub distance: f32,
}

/// Conservative bounds of one segment, for pruning closest-point queries.
#[derive(Debug, Clone, Copy)]
struct SegmentBound {
    center: Vec3,
    radius: f32,
    /// Arc-length bound of half a sample interval.
    half_interval_arc: f32,
    /// Upper bound of `|C'(s)|` on the segment (padded).
    max_speed: f32,
    /// Upper bound of `|C''(s)|` on the segment (padded).
    max_second: f32,
    /// `|C'''|` (constant on a cubic segment, padded).
    max_third: f32,
    /// Lower bound of `|C'(s)|²` on the segment (0 if it may vanish).
    min_speed_sq: f32,
}

/// A sample-table entry: position and first derivative.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Sample {
    pub(crate) point: Vec3,
    pub(crate) tangent: Vec3,
}

/// A C² interpolating cubic spline.
///
/// The parameter is uniform per through-point (`P_i` at `t = i`), so
/// through-points should be roughly evenly spaced: where the spacing changes
/// abruptly (say 50 mm on a straight, 13 mm on the following arc) the curve
/// overshoots and wiggles, as any uniformly parametrized spline does.
#[derive(Debug, Clone)]
pub struct CubicSpline {
    segments: Vec<BezierSegment>,
    /// Second derivative at every through-point (`segments.len() + 1`).
    knot_dd: Vec<Vec3>,
    bounds: Vec<SegmentBound>,
    /// `segments.len() * SAMPLES_PER_SEGMENT + 1` uniformly spaced samples.
    samples: Vec<Sample>,
    /// Built with periodic end conditions.
    closed: bool,
}

/// Second derivatives at the knots of the periodic cubic spline through the
/// closed polygon `points` (`points[0] == points[n]`), unit knot spacing.
/// The cyclic system `M[i-1] + 4 M[i] + M[i+1] = rhs[i]` is solved by
/// Gauss–Seidel sweeps, which contract by a factor of four per sweep here.
fn periodic_second_derivatives(points: &[DVec3]) -> Vec<DVec3> {
    let n = points.len() - 1; // distinct knots
    let mut m = vec![DVec3::ZERO; n + 1];
    let rhs: Vec<DVec3> = (0..n)
        .map(|i| (points[(i + n - 1) % n] - points[i] * 2.0 + points[(i + 1) % n]) * 6.0)
        .collect();
    for _ in 0..64 {
        for i in 0..n {
            m[i] = (rhs[i] - m[(i + n - 1) % n] - m[(i + 1) % n]) / 4.0;
        }
    }
    m[n] = m[0];
    m
}

/// Second derivatives at the knots of the natural cubic spline through
/// `points` with unit knot spacing (Thomas algorithm; the system is strictly
/// diagonally dominant, so the solve is unconditionally stable).
pub(crate) fn natural_second_derivatives(points: &[DVec3]) -> Vec<DVec3> {
    let n = points.len();
    let mut m = vec![DVec3::ZERO; n];
    if n < 3 {
        return m;
    }
    // Row i: M[i-1] + 4 M[i] + M[i+1] = 6 (P[i-1] - 2 P[i] + P[i+1]).
    let mut c = vec![0.0f64; n];
    for i in 1..n - 1 {
        let rhs = (points[i - 1] - points[i] * 2.0 + points[i + 1]) * 6.0;
        let denom = 4.0 - c[i - 1];
        c[i] = 1.0 / denom;
        m[i] = (rhs - m[i - 1]) / denom;
    }
    for i in (1..n - 1).rev() {
        let next = m[i + 1];
        m[i] -= next * c[i];
    }
    m
}

impl CubicSpline {
    /// Builds the natural C² cubic spline through `points`.
    ///
    /// # Errors
    /// Fewer than two points, a non-finite coordinate, or two consecutive
    /// identical points.
    pub fn interpolate(points: &[Vec3]) -> Result<Self, SplineError> {
        if points.len() < 2 {
            return Err(SplineError::TooFewPoints);
        }
        if points.iter().any(|p| !p.is_finite()) {
            return Err(SplineError::NonFinite);
        }
        if points.windows(2).any(|w| w[0] == w[1]) {
            return Err(SplineError::DuplicatePoints);
        }
        Self::build(points, false)
    }

    /// Builds the periodic C² cubic spline through a closed polygon: the last
    /// point must equal the first, and the curve is C² there too (no natural
    /// end conditions). Use it for closed guides and loops.
    ///
    /// # Errors
    /// Fewer than four points (three distinct), last point different from the
    /// first, a non-finite coordinate, or two consecutive identical points.
    pub fn interpolate_closed(points: &[Vec3]) -> Result<Self, SplineError> {
        if points.len() < 4 {
            return Err(SplineError::TooFewPoints);
        }
        if points.iter().any(|p| !p.is_finite()) {
            return Err(SplineError::NonFinite);
        }
        if points[0] != points[points.len() - 1] {
            return Err(SplineError::NotClosed);
        }
        if points.windows(2).any(|w| w[0] == w[1]) {
            return Err(SplineError::DuplicatePoints);
        }
        Self::build(points, true)
    }

    fn build(points: &[Vec3], closed: bool) -> Result<Self, SplineError> {
        // Solve in f64, store in f32.
        let pd: Vec<DVec3> = points.iter().map(|p| p.as_dvec3()).collect();
        let md = if closed {
            periodic_second_derivatives(&pd)
        } else {
            natural_second_derivatives(&pd)
        };
        if md.iter().any(|m| !m.as_vec3().is_finite()) {
            // Coordinates so large that differences overflow f32.
            return Err(SplineError::NonFinite);
        }
        let n = points.len() - 1;
        let mut segments = Vec::with_capacity(n);
        for i in 0..n {
            let delta = pd[i + 1] - pd[i];
            let d0 = delta - (md[i] * 2.0 + md[i + 1]) / 6.0;
            let d1 = delta + (md[i] + md[i + 1] * 2.0) / 6.0;
            let seg = BezierSegment {
                p: [
                    points[i],
                    (pd[i] + d0 / 3.0).as_vec3(),
                    (pd[i + 1] - d1 / 3.0).as_vec3(),
                    points[i + 1],
                ],
            };
            if seg.p.iter().any(|p| !p.is_finite()) {
                return Err(SplineError::NonFinite);
            }
            segments.push(seg);
        }
        let knot_dd: Vec<Vec3> = md.iter().map(|m| m.as_vec3()).collect();

        let k = SAMPLES_PER_SEGMENT;
        let mut samples = Vec::with_capacity(n * k + 1);
        let mut bounds = Vec::with_capacity(n);
        for seg in &segments {
            for j in 0..k {
                let s = j as f32 / k as f32;
                samples.push(Sample {
                    point: seg.point(s),
                    tangent: seg.tangent(s),
                });
            }
            let center = (seg.p[0] + seg.p[1] + seg.p[2] + seg.p[3]) * 0.25;
            let mut radius = 0.0f32;
            for p in seg.p {
                radius = radius.max(center.distance(p));
            }
            // |C'(s)| <= 3 max |p[j+1] - p[j]| (convex hull of the hodograph).
            let mut max_speed = 0.0f32;
            for j in 0..3 {
                max_speed = max_speed.max(seg.p[j + 1].distance(seg.p[j]));
            }
            max_speed *= 3.0;
            // C'' of a cubic is linear in s: its largest norm is at an end.
            let max_second = 6.0
                * (seg.p[2] - seg.p[1] * 2.0 + seg.p[0])
                    .length()
                    .max((seg.p[3] - seg.p[2] * 2.0 + seg.p[1]).length());
            let max_second = max_second * 1.0001 + 1.0e-6;
            let max_third =
                6.0 * (seg.p[3] - seg.p[2] * 3.0 + seg.p[1] * 3.0 - seg.p[0]).length() * 1.0001
                    + 1.0e-6;
            // Every s is within half a sample interval of a sample, and C'
            // changes by at most `max_second` per unit s.
            let mut min_speed = f32::INFINITY;
            for j in 0..=k {
                min_speed = min_speed.min(seg.tangent(j as f32 / k as f32).length());
            }
            let min_speed = (min_speed - max_second * (0.5 / k as f32)) * 0.9999;
            // Slightly padded so rounding can never make the bounds unsafe.
            bounds.push(SegmentBound {
                center,
                radius: radius * 1.0001 + 1.0e-6,
                half_interval_arc: max_speed * (0.5 / k as f32) * 1.0001 + 1.0e-6,
                max_speed: max_speed * 1.0001 + 1.0e-6,
                max_second,
                max_third,
                min_speed_sq: if min_speed > 0.0 {
                    min_speed * min_speed
                } else {
                    0.0
                },
            });
        }
        let last = &segments[n - 1];
        samples.push(Sample {
            point: last.point(1.0),
            tangent: last.tangent(1.0),
        });

        Ok(Self {
            segments,
            knot_dd,
            bounds,
            samples,
            closed,
        })
    }

    /// Number of Bézier segments (through-points minus one).
    #[must_use]
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    /// The Bézier segments, in order.
    #[must_use]
    pub fn segments(&self) -> &[BezierSegment] {
        &self.segments
    }

    /// Whether the spline was built with
    /// [`interpolate_closed`](Self::interpolate_closed) (periodic, smooth
    /// where its end meets its start). The parameter is still clamped to
    /// `[0, segment_count()]`, not wrapped.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Largest valid parameter (`segment_count()` as `f32`).
    #[must_use]
    pub fn max_t(&self) -> f32 {
        self.segments.len() as f32
    }

    /// The uniformly spaced sample table (`SAMPLES_PER_SEGMENT` per segment).
    pub(crate) fn samples(&self) -> &[Sample] {
        &self.samples
    }

    /// Splits a global parameter into (segment index, local parameter),
    /// clamping to the valid range. NaN maps to the start.
    pub(crate) fn locate(&self, t: f32) -> (usize, f32) {
        let n = self.segments.len();
        let t = if t.is_nan() {
            0.0
        } else {
            t.clamp(0.0, n as f32)
        };
        let i = (t as usize).min(n - 1);
        (i, t - i as f32)
    }

    /// Second derivative of segment `i` at local parameter `s`. It is the
    /// linear blend of the knot values, hence exactly continuous at knots.
    fn second_at(&self, i: usize, s: f32) -> Vec3 {
        self.knot_dd[i] * (1.0 - s) + self.knot_dd[i + 1] * s
    }

    /// Position, first and second derivative of segment `i` at `s`.
    pub(crate) fn eval(&self, i: usize, s: f32) -> (Vec3, Vec3, Vec3) {
        let seg = &self.segments[i];
        (seg.point(s), seg.tangent(s), self.second_at(i, s))
    }

    /// Position at `t` (clamped to `[0, segment_count()]`).
    #[must_use]
    pub fn point(&self, t: f32) -> Vec3 {
        let (i, s) = self.locate(t);
        self.segments[i].point(s)
    }

    /// First derivative with respect to `t` (clamped).
    #[must_use]
    pub fn tangent(&self, t: f32) -> Vec3 {
        let (i, s) = self.locate(t);
        self.segments[i].tangent(s)
    }

    /// Second derivative with respect to `t` (clamped).
    #[must_use]
    pub fn second_derivative(&self, t: f32) -> Vec3 {
        let (i, s) = self.locate(t);
        self.second_at(i, s)
    }

    /// Closest point of the curve to `p`.
    ///
    /// `seed_t` (typically the ball's parameter from the previous tick) starts
    /// a Newton iteration whose result prunes the search; the coarse sample
    /// table then searches the whole curve whatever the seed is. The seed
    /// affects only the pruning and the starting point, not which answer is
    /// correct. Never allocates.
    ///
    /// The returned point is the global minimum of the distance up to a
    /// bounded approximation error, not exactly, and seeded and unseeded
    /// queries agree to within that bound, not bit-for-bit. Cause:
    /// `search_interval` bisects a 1/16-segment sample interval at most 5
    /// times (`MULTI_ROOT_DEPTH`), so the finest piece is `w = 1/512` of a
    /// segment.
    /// At that limit a piece that is not provably monotone can be dropped when
    /// its ends do not bracket a root, or only one bracketed root is refined.
    /// The analytic bound on what is missed is `|gp_min| w²` in squared
    /// distance, where `gp_min` is the minimum of `g'` over the piece; on the
    /// spline sizes the solver meets it is below f32 rounding (docs/DECISIONS.md,
    /// "closest_point depth-limit bound"). The 1 µm tolerance of the sweeps in
    /// tests/spline_closest_point_depth_sweep.rs is empirical, not a per-query
    /// guarantee: they found a worst error of 0.46 µm (unseeded vs brute
    /// force) and 0.48 µm (seeded vs unseeded).
    ///
    /// Every local minimum of `|C - p|²` is a root of `g = (C - p) · C'`.
    /// Where `g' = |C'|² + (C - p) · C''` is provably positive on a whole
    /// segment (`min |C'|² > max |C - p| max |C''|` from per-segment bounds,
    /// roughly: `p` is closer than one radius of curvature), `g` increases,
    /// each sample interval holds at most one root, and it is refined when
    /// the end samples bracket it: the fast path, which is nearly every
    /// query. Elsewhere an interval may hold several roots (minima and
    /// maxima, even with both end signs alike), so it is bisected (bounded
    /// depth, fixed-size stack) until each piece is provably monotone,
    /// root-free or no closer than the best candidate, and every bracketed
    /// root is refined (issue #8); pieces that reach the depth limit are
    /// settled without that proof, within the bound stated on
    /// `search_interval` (well below f32 rounding at pinball scale).
    ///
    /// A non-finite `p` yields the curve start with an infinite distance.
    #[must_use]
    pub fn closest_point(&self, p: Vec3, seed_t: Option<f32>) -> ClosestPoint {
        if !p.is_finite() {
            return ClosestPoint {
                t: 0.0,
                point: self.segments[0].p[0],
                distance: f32::INFINITY,
            };
        }
        let k = SAMPLES_PER_SEGMENT;
        let n = self.segments.len();

        // Initial candidate: the nearest through-point.
        let mut best_t = 0.0f32;
        let mut best_point = self.samples[0].point;
        let mut best_d2 = f32::INFINITY;
        for i in 0..=n {
            let q = self.samples[i * k].point;
            let d2 = q.distance_squared(p);
            if d2 < best_d2 {
                best_d2 = d2;
                best_t = i as f32;
                best_point = q;
            }
        }

        // Newton from the seed: a cheap, usually exact, upper bound.
        if let Some(seed) = seed_t.filter(|s| s.is_finite()) {
            let (t, q) = self.newton_from(p, seed);
            let d2 = q.distance_squared(p);
            if d2 < best_d2 {
                best_d2 = d2;
                best_t = t;
                best_point = q;
            }
        }

        // Global pass over the sample table, pruned by conservative bounds.
        for i in 0..n {
            let bound = &self.bounds[i];
            let center_distance = bound.center.distance(p);
            let lb = center_distance - bound.radius;
            if lb > 0.0 && lb * lb >= best_d2 {
                continue;
            }
            // Fast path: `g` provably increases on the whole segment
            // (`min |C'|² > max |C - p| max |C''|`), so each sample interval
            // holds at most one root, refined when its end samples bracket it.
            if bound.min_speed_sq <= (center_distance + bound.radius) * bound.max_second {
                let mut best = (best_d2, best_t, best_point);
                self.search_segment(i, p, &mut best);
                (best_d2, best_t, best_point) = best;
                continue;
            }
            let base = i * k;
            let e0 = self.samples[base].point - p;
            let mut d0 = e0.length_squared();
            let mut g0 = e0.dot(self.samples[base].tangent);
            for j in 0..k {
                let s1 = &self.samples[base + j + 1];
                let e1 = s1.point - p;
                let d1 = e1.length_squared();
                let g1 = e1.dot(s1.tangent);
                if d1 < best_d2 {
                    best_d2 = d1;
                    best_point = s1.point;
                    best_t = i as f32 + (j + 1) as f32 / k as f32;
                }
                // Every point of this interval is within `half_interval_arc`
                // of one of its two end samples.
                let lbi = d0.min(d1).sqrt() - bound.half_interval_arc;
                if (lbi <= 0.0 || lbi * lbi < best_d2) && g0 < 0.0 && g1 > 0.0 {
                    let lo = j as f32 / k as f32;
                    let hi = (j + 1) as f32 / k as f32;
                    let (s, q) = self.refine(i, p, lo, hi, g0, g1);
                    let d2 = q.distance_squared(p);
                    if d2 < best_d2 {
                        best_d2 = d2;
                        best_point = q;
                        best_t = i as f32 + s;
                    }
                }
                d0 = d1;
                g0 = g1;
            }
        }

        ClosestPoint {
            t: best_t,
            point: best_point,
            distance: best_d2.sqrt(),
        }
    }

    /// The global pass of [`Self::closest_point`] over segment `i` when `g`
    /// may have several roots in a sample interval: like the fast path, but
    /// each interval that may hold a closer point goes to
    /// [`Self::search_interval`] with `g'` at its end samples. `best` is
    /// (squared distance, global parameter, point).
    #[inline(never)]
    fn search_segment(&self, i: usize, p: Vec3, best: &mut (f32, f32, Vec3)) {
        let k = SAMPLES_PER_SEGMENT;
        let bound = &self.bounds[i];
        let dd = second_differences(&self.segments[i]);
        let base = i * k;
        let s0 = &self.samples[base];
        let e0 = s0.point - p;
        let mut d0 = e0.length_squared();
        let mut g0 = e0.dot(s0.tangent);
        let mut gp0 = slope(e0, s0.tangent, dd, 0.0);
        for j in 0..k {
            let s1 = &self.samples[base + j + 1];
            let e1 = s1.point - p;
            let d1 = e1.length_squared();
            let g1 = e1.dot(s1.tangent);
            let hi = (j + 1) as f32 / k as f32;
            let gp1 = slope(e1, s1.tangent, dd, hi);
            if d1 < best.0 {
                *best = (d1, i as f32 + hi, s1.point);
            }
            let lbi = d0.min(d1).sqrt() - bound.half_interval_arc;
            if lbi <= 0.0 || lbi * lbi < best.0 {
                let root = Node {
                    lo: j as f32 / k as f32,
                    hi,
                    g: [g0, g1],
                    d2: [d0, d1],
                    slope: [gp0, gp1],
                    depth: 0,
                };
                self.search_interval(i, p, root, best);
            }
            d0 = d1;
            g0 = g1;
            gp0 = gp1;
        }
    }

    /// Search of one sample interval of segment `i` where `g` may have
    /// several roots (see [`Self::closest_point`]): depth-first bisection
    /// on a fixed-size stack (no allocation). On a piece of width `w`,
    /// `g'` lies within `G2 w / 2` of its value at the nearer end, where
    /// `G2 >= |g''| = |3 C'·C'' + (C - p)·C'''|`. A piece is
    /// - dropped when a second-order lower bound of `|C - p|²` (from either
    ///   end, with `g'` at its lower bound) cannot beat `best` by more than
    ///   rounding, or when `g` provably keeps one sign or decreases (no
    ///   interior minimum; the ends are candidates already);
    /// - refined when `g` provably increases (one root), or at
    ///   [`MULTI_ROOT_DEPTH`], if its ends bracket a root;
    /// - otherwise split at its midpoint, which is a candidate too.
    ///
    /// Guarantee. Every dropped piece other than at the depth limit provably
    /// holds no point closer than `best` by more than the prune margin of
    /// 4 ulps of the squared distance (about 2 ulps of the distance). At the
    /// depth limit a piece is not proven: one whose ends do not bracket a
    /// root is dropped (an interior minimum and maximum may sit between
    /// same-sign ends) and one whose ends bracket a root has only one root
    /// refined (there may be three). With `c = gp_min <= 0` there (else it
    /// would be the provably increasing case), the second-order bound from
    /// a candidate already taken bounds the miss: with both `g > 0`,
    /// `|C - p|²` on the piece is at least `d_lo + c w²` (from the lower
    /// end; with both `g < 0`, `d_hi + c w²` from the upper end), and around
    /// a refined root `s*` it is at least `d(s*) + c w²`. So the result
    /// exceeds the true minimum by at most `|gp_min| w²` in squared
    /// distance, plus the 4-ulp margin and f32 rounding, with `w = 1/512` of
    /// a segment. Release sweeps of about 3.7 · 10^5 queries on tight
    /// splines near centres of curvature (the generator of
    /// tests/spline_closest_point_depth_sweep.rs, which keeps 10^4 of them)
    /// found no miss above f32 rounding: at most 31 nm lost in a
    /// depth-limit piece, at 0.27 m coordinates.
    ///
    /// `best` is (squared distance, global parameter, point).
    fn search_interval(&self, i: usize, p: Vec3, root: Node, best: &mut (f32, f32, Vec3)) {
        let bound = &self.bounds[i];
        let seg = &self.segments[i];
        let dd = second_differences(seg);
        let mut stack = [root; MULTI_ROOT_DEPTH as usize + 1];
        let mut len = 1;
        while len > 0 {
            len -= 1;
            let node = stack[len];
            let w = node.hi - node.lo;
            let [g_lo, g_hi] = node.g;
            let [d_lo, d_hi] = node.d2;
            let [gp_lo, gp_hi] = node.slope;
            let reach = d_lo.max(d_hi).sqrt() + bound.max_speed * 0.5 * w;
            let g2 = 3.0 * bound.max_speed * bound.max_second + reach * bound.max_third;
            let slack = g2 * 0.5 * w;
            let gp_min = gp_lo.min(gp_hi) - slack;
            let gp_max = gp_lo.max(gp_hi) + slack;
            // |C - p|² at lo + x is at least d_lo + 2 g_lo x + gp_min x², and
            // likewise from hi; either bound's minimum bounds the piece.
            let floor =
                taylor_floor(d_lo, g_lo, gp_min, w).max(taylor_floor(d_hi, -g_hi, gp_min, w));
            if floor >= best.0 * (1.0 - 4.0 * f32::EPSILON) {
                continue;
            }
            // `g` decreasing: no interior minimum.
            if gp_max < 0.0 {
                continue;
            }
            let one_sign = g_lo > 0.0 && g_hi > 0.0 || g_lo < 0.0 && g_hi < 0.0;
            if one_sign && g_lo.abs() + g_hi.abs() > gp_min.abs().max(gp_max.abs()) * w {
                continue;
            }
            if gp_min > 0.0 || node.depth >= MULTI_ROOT_DEPTH {
                if g_lo < 0.0 && g_hi > 0.0 {
                    let (s, q) = self.refine(i, p, node.lo, node.hi, g_lo, g_hi);
                    let d2 = q.distance_squared(p);
                    if d2 < best.0 {
                        *best = (d2, i as f32 + s, q);
                    }
                }
                continue;
            }
            let mid = 0.5 * (node.lo + node.hi);
            let q = seg.point(mid);
            let tan = seg.tangent(mid);
            let e = q - p;
            let (d_mid, g_mid) = (e.length_squared(), e.dot(tan));
            let gp_mid = slope(e, tan, dd, mid);
            if d_mid < best.0 {
                *best = (d_mid, i as f32 + mid, q);
            }
            let depth = node.depth + 1;
            // Right half below the left one: the left half is searched first.
            stack[len] = Node {
                lo: mid,
                hi: node.hi,
                g: [g_mid, g_hi],
                d2: [d_mid, d_hi],
                slope: [gp_mid, gp_hi],
                depth,
            };
            stack[len + 1] = Node {
                lo: node.lo,
                hi: mid,
                g: [g_lo, g_mid],
                d2: [d_lo, d_mid],
                slope: [gp_lo, gp_mid],
                depth,
            };
            len += 2;
        }
    }

    /// Unbracketed, step-limited Newton iteration on
    /// `g(t) = (C(t) - p) · C'(t)` from `seed`. Returns the final parameter
    /// and point; the caller only uses it if it improves on its best.
    fn newton_from(&self, p: Vec3, seed: f32) -> (f32, Vec3) {
        let max_t = self.max_t();
        let mut t = seed.clamp(0.0, max_t);
        for _ in 0..8 {
            let (i, s) = self.locate(t);
            let (c, d1, d2) = self.eval(i, s);
            let e = c - p;
            let g = e.dot(d1);
            let h = d1.length_squared() + e.dot(d2);
            let step = if h > 1.0e-12 {
                (-g / h).clamp(-0.5, 0.5)
            } else if g > 0.0 {
                // Not locally convex: walk downhill.
                -0.25
            } else {
                0.25
            };
            let next = (t + step).clamp(0.0, max_t);
            let moved = (next - t).abs();
            t = next;
            if moved < NEWTON_EPS {
                break;
            }
        }
        (t, self.point(t))
    }

    /// Root of `g(s) = (C(s) - p) · C'(s)` in the bracket `[lo, hi]` of
    /// segment `i`, where `g(lo) = g_lo < 0 < g(hi) = g_hi`: Newton steps
    /// safeguarded by bisection, so it always converges inside the bracket.
    fn refine(&self, i: usize, p: Vec3, lo: f32, hi: f32, g_lo: f32, g_hi: f32) -> (f32, Vec3) {
        let (mut lo, mut hi) = (lo, hi);
        // Start at the secant root.
        let mut s = lo + (hi - lo) * (g_lo / (g_lo - g_hi));
        if !(s > lo && s < hi) {
            s = 0.5 * (lo + hi);
        }
        for _ in 0..24 {
            let (c, d1, d2) = self.eval(i, s);
            let e = c - p;
            let g = e.dot(d1);
            if g > 0.0 {
                hi = s;
            } else if g < 0.0 {
                lo = s;
            } else {
                break;
            }
            let h = d1.length_squared() + e.dot(d2);
            let mut next = if h > 0.0 { s - g / h } else { f32::NAN };
            if !(next > lo && next < hi) {
                next = 0.5 * (lo + hi);
            }
            let moved = (next - s).abs();
            s = next;
            if moved < NEWTON_EPS * 0.25 || hi - lo < NEWTON_EPS * 0.25 {
                break;
            }
        }
        (s, self.segments[i].point(s))
    }
}

/// Second differences `(A, B)` of a Bézier segment: `C''(s) = (1 - s) A + s B`.
fn second_differences(seg: &BezierSegment) -> (Vec3, Vec3) {
    let [p0, p1, p2, p3] = seg.p;
    ((p2 - p1 * 2.0 + p0) * 6.0, (p3 - p2 * 2.0 + p1) * 6.0)
}

/// `g'(s) = |C'|² + (C - p) · C''` from `e = C(s) - p` and `C'(s)`.
fn slope(e: Vec3, tangent: Vec3, dd: (Vec3, Vec3), s: f32) -> f32 {
    tangent.length_squared() + e.dot(dd.0 * (1.0 - s) + dd.1 * s)
}

/// Lower bound over `x` in `[0, w]` of `d + 2 g x + c x²`.
fn taylor_floor(d: f32, g: f32, c: f32, w: f32) -> f32 {
    let end = d + (2.0 * g + c * w) * w;
    let mut floor = d.min(end);
    if c > 0.0 {
        let x = -g / c;
        if x > 0.0 && x < w {
            floor = floor.min(d - g * g / c);
        }
    }
    floor
}

/// A piece `[lo, hi]` of a sample interval in
/// [`CubicSpline::search_interval`], with `g`, squared distance and
/// `g' = |C'|² + (C - p) · C''` at both ends.
#[derive(Debug, Clone, Copy)]
struct Node {
    lo: f32,
    hi: f32,
    g: [f32; 2],
    d2: [f32; 2],
    slope: [f32; 2],
    depth: u32,
}
