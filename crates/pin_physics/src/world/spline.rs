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
    /// A guide that rises or falls has a plan view (its curve seen from
    /// above) that a ball cannot follow: it bends tighter than about 22 mm
    /// radius, comes back onto or close to itself (a helix, a loop of more
    /// than one turn, a crossing), or runs vertically. Model helical ramps
    /// as wireforms, and split other shapes into simple pieces.
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
                 tighter than 22 mm radius, no stretch coming back onto or within 44 mm \
                 of another (helix, loop, crossing), no vertical run; use a wireform \
                 for helical ramps"
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
            // Slightly padded so rounding can never make the bounds unsafe.
            bounds.push(SegmentBound {
                center,
                radius: radius * 1.0001 + 1.0e-6,
                half_interval_arc: max_speed * (0.5 / k as f32) * 1.0001 + 1.0e-6,
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
    /// table then guarantees that the global minimum is returned whatever the
    /// seed is, so seeded and unseeded queries agree. Never allocates.
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
            let lb = bound.center.distance(p) - bound.radius;
            if lb > 0.0 && lb * lb >= best_d2 {
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
