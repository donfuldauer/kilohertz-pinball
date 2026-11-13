//! Wireforms (two rails around a centerline spline) and single-curve guides.

use glam::{DVec3, Vec3};

use super::shapes::{APPROACH_EPS, any_perpendicular};
use super::spline::{
    ClosestPoint, CubicSpline, SAMPLES_PER_SEGMENT, SplineError, natural_second_derivatives,
};

/// Gap at which a guide cast reports contact (metres).
const GUIDE_SKIN: f32 = 1.0e-5;

/// Maximum steps (closest-point evaluations) of a guide cast.
const GUIDE_CAST_STEPS: u32 = 32;

/// An orthonormal, right-handed moving frame: `binormal = tangent × normal`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// Unit tangent (direction of increasing parameter).
    pub tangent: Vec3,
    /// Unit normal ("up" of the wireform cross-section).
    pub normal: Vec3,
    /// Unit binormal (`tangent × normal`); the rails lie along ± this.
    pub binormal: Vec3,
}

/// Cross-section state of a wireform at one parameter, with derivatives.
struct Section {
    center: Vec3,
    d1: Vec3,
    d2: Vec3,
    tangent: Vec3,
    speed: f32,
    /// Rotation-minimizing (unrolled) normal.
    rm_normal: Vec3,
    roll: f32,
    roll_rate: f32,
}

/// A wireform ramp: two rails a fixed gauge apart around a centerline spline.
///
/// The cross-section orientation is a rotation-minimizing frame (double
/// reflection method, Wang et al. 2008) started from `up_hint`, then rolled
/// about the tangent by the authored roll angle. Roll angles are given per
/// through-point and interpolated with a natural C² cubic spline, so the rails
/// are as smooth as the centerline.
#[derive(Debug, Clone)]
pub struct Wireform {
    centerline: CubicSpline,
    gauge: f32,
    /// Roll at the through-points (radians) and its second derivatives.
    roll: Vec<f32>,
    roll_dd: Vec<f32>,
    /// Rotation-minimizing normal at every centerline sample.
    normals: Vec<Vec3>,
    /// Its derivative with respect to the curve parameter.
    normal_rates: Vec<Vec3>,
}

impl Wireform {
    /// Creates a wireform.
    ///
    /// `roll_deg` holds one roll angle per through-point of `centerline`
    /// (degrees, positive = right-handed rotation about the tangent).
    /// `gauge_m` is the distance between the rails. `up_hint` fixes the frame
    /// at the start: the initial normal is `up_hint` made perpendicular to the
    /// tangent (if they are parallel, a deterministic fallback axis is used).
    ///
    /// # Errors
    /// Roll count mismatch, non-finite input, non-positive gauge, or a zero
    /// `up_hint`.
    pub fn new(
        centerline: CubicSpline,
        roll_deg: &[f32],
        gauge_m: f32,
        up_hint: Vec3,
    ) -> Result<Self, SplineError> {
        if roll_deg.len() != centerline.segment_count() + 1 {
            return Err(SplineError::RollCountMismatch);
        }
        if roll_deg.iter().any(|r| !r.is_finite()) || !gauge_m.is_finite() || !up_hint.is_finite() {
            return Err(SplineError::NonFinite);
        }
        if gauge_m <= 0.0 {
            return Err(SplineError::InvalidGauge);
        }
        if up_hint == Vec3::ZERO {
            return Err(SplineError::InvalidUpHint);
        }

        // Roll: natural cubic spline of the angles in radians.
        let roll_points: Vec<DVec3> = roll_deg
            .iter()
            .map(|d| DVec3::new(f64::from(*d).to_radians(), 0.0, 0.0))
            .collect();
        let roll_dd: Vec<f32> = natural_second_derivatives(&roll_points)
            .iter()
            .map(|m| m.x as f32)
            .collect();
        let roll: Vec<f32> = roll_points.iter().map(|p| p.x as f32).collect();

        // Rotation-minimizing frame along the sample table (f64).
        let samples = centerline.samples();
        let unit = |v: Vec3, fallback: DVec3| -> DVec3 {
            let d = v.as_dvec3();
            let len = d.length();
            if len > 1.0e-20 { d / len } else { fallback }
        };
        let mut tangent = unit(samples[0].tangent, DVec3::X);
        let up = up_hint.as_dvec3().normalize();
        let mut normal = up - tangent * up.dot(tangent);
        if normal.length_squared() < 1.0e-10 {
            normal = any_perpendicular(tangent.as_vec3()).as_dvec3();
            normal -= tangent * normal.dot(tangent);
        }
        normal = normal.normalize();

        let mut normals = Vec::with_capacity(samples.len());
        normals.push(normal.as_vec3());
        for k in 0..samples.len() - 1 {
            let next_tangent = unit(samples[k + 1].tangent, tangent);
            let v1 = samples[k + 1].point.as_dvec3() - samples[k].point.as_dvec3();
            let c1 = v1.length_squared();
            let (mut r, t_reflected) = if c1 > 0.0 {
                (
                    normal - v1 * (2.0 / c1 * v1.dot(normal)),
                    tangent - v1 * (2.0 / c1 * v1.dot(tangent)),
                )
            } else {
                (normal, tangent)
            };
            let v2 = next_tangent - t_reflected;
            let c2 = v2.length_squared();
            if c2 > 0.0 {
                r -= v2 * (2.0 / c2 * v2.dot(r));
            }
            // Remove accumulated rounding drift.
            r -= next_tangent * r.dot(next_tangent);
            let len = r.length();
            normal = if len > 1.0e-12 { r / len } else { normal };
            tangent = next_tangent;
            normals.push(normal.as_vec3());
        }

        // d(normal)/dt of a rotation-minimizing frame: the normal only turns
        // towards/away from the tangent.
        let k = SAMPLES_PER_SEGMENT;
        let n = centerline.segment_count();
        let mut normal_rates = Vec::with_capacity(samples.len());
        for (index, normal) in normals.iter().enumerate() {
            let i = (index / k).min(n - 1);
            let s = (index - i * k) as f32 / k as f32;
            let (_, d1, d2) = centerline.eval(i, s);
            let speed2 = d1.length_squared();
            normal_rates.push(if speed2 > 1.0e-20 {
                d1 * (-normal.dot(d2) / speed2)
            } else {
                Vec3::ZERO
            });
        }

        Ok(Self {
            centerline,
            gauge: gauge_m,
            roll,
            roll_dd,
            normals,
            normal_rates,
        })
    }

    /// The centerline spline.
    #[must_use]
    pub fn centerline(&self) -> &CubicSpline {
        &self.centerline
    }

    /// Distance between the two rails (metres).
    #[must_use]
    pub fn gauge(&self) -> f32 {
        self.gauge
    }

    /// Interpolated roll angle (radians) and its parameter derivative.
    fn roll_at(&self, i: usize, s: f32) -> (f32, f32) {
        let a = 1.0 - s;
        let (y0, y1) = (self.roll[i], self.roll[i + 1]);
        let (m0, m1) = (self.roll_dd[i], self.roll_dd[i + 1]);
        let value = y0 * a + y1 * s + ((a * a * a - a) * m0 + (s * s * s - s) * m1) / 6.0;
        let rate = (y1 - y0) + ((1.0 - 3.0 * a * a) * m0 + (3.0 * s * s - 1.0) * m1) / 6.0;
        (value, rate)
    }

    /// Interpolated roll angle at `t`, in radians.
    #[must_use]
    pub fn roll_rad(&self, t: f32) -> f32 {
        let (i, s) = self.centerline.locate(t);
        self.roll_at(i, s).0
    }

    /// Rotation-minimizing normal at (segment `i`, local `s`): cubic Hermite
    /// interpolation of the sample table, made exactly perpendicular to the
    /// tangent.
    fn rm_normal(&self, i: usize, s: f32, tangent: Vec3) -> Vec3 {
        let k = SAMPLES_PER_SEGMENT;
        let x = s * k as f32;
        let j = (x as usize).min(k - 1);
        let u = x - j as f32;
        let index = i * k + j;
        let h = 1.0 / k as f32;
        let (r0, r1) = (self.normals[index], self.normals[index + 1]);
        let (m0, m1) = (
            self.normal_rates[index] * h,
            self.normal_rates[index + 1] * h,
        );
        let u2 = u * u;
        let u3 = u2 * u;
        let r = r0 * (2.0 * u3 - 3.0 * u2 + 1.0)
            + m0 * (u3 - 2.0 * u2 + u)
            + r1 * (3.0 * u2 - 2.0 * u3)
            + m1 * (u3 - u2);
        let r = r - tangent * r.dot(tangent);
        let len = r.length();
        if len > 1.0e-6 {
            return r / len;
        }
        let r = r0 - tangent * r0.dot(tangent);
        let len = r.length();
        if len > 1.0e-6 {
            r / len
        } else {
            any_perpendicular(tangent)
        }
    }

    fn section(&self, t: f32) -> Section {
        let (i, s) = self.centerline.locate(t);
        let (center, d1, d2) = self.centerline.eval(i, s);
        let speed = d1.length();
        let tangent = if speed > 1.0e-12 { d1 / speed } else { Vec3::X };
        let rm_normal = self.rm_normal(i, s, tangent);
        let (roll, roll_rate) = self.roll_at(i, s);
        Section {
            center,
            d1,
            d2,
            tangent,
            speed,
            rm_normal,
            roll,
            roll_rate,
        }
    }

    /// The rolled frame at `t` (clamped to the centerline's range).
    #[must_use]
    pub fn frame_at(&self, t: f32) -> Frame {
        let sec = self.section(t);
        let rm_binormal = sec.tangent.cross(sec.rm_normal);
        let (sin, cos) = (libm::sinf(sec.roll), libm::cosf(sec.roll));
        let normal = (sec.rm_normal * cos + rm_binormal * sin).normalize();
        Frame {
            tangent: sec.tangent,
            normal,
            binormal: sec.tangent.cross(normal),
        }
    }

    /// The two rail points at `t`: `centerline ± (gauge / 2) · binormal`
    /// (the `+` rail first).
    #[must_use]
    pub fn rail_points(&self, t: f32) -> (Vec3, Vec3) {
        let center = self.centerline.point(t);
        let offset = self.frame_at(t).binormal * (0.5 * self.gauge);
        (center + offset, center - offset)
    }

    /// Position and parameter derivative of one rail (`side` = ±1).
    fn rail_state(&self, t: f32, side: f32) -> (Vec3, Vec3) {
        let sec = self.section(t);
        let half = 0.5 * self.gauge * side;
        let n0 = sec.rm_normal;
        let b0 = sec.tangent.cross(n0);
        let (sin, cos) = (libm::sinf(sec.roll), libm::cosf(sec.roll));
        let normal = n0 * cos + b0 * sin;
        let binormal = b0 * cos - n0 * sin;
        if sec.speed <= 1.0e-12 {
            return (sec.center + binormal * half, sec.d1);
        }
        // T' = (C'' - T (T·C'')) / |C'|;  n0' = -(n0·C'') / |C'| · T;  b0' = T' × n0.
        let tangent_rate = (sec.d2 - sec.tangent * sec.tangent.dot(sec.d2)) / sec.speed;
        let n0_rate = sec.tangent * (-n0.dot(sec.d2) / sec.speed);
        let b0_rate = tangent_rate.cross(n0);
        let binormal_rate = b0_rate * cos - n0_rate * sin - normal * sec.roll_rate;
        (
            sec.center + binormal * half,
            sec.d1 + binormal_rate * half,
            // Second derivative of the rail is approximated by the
            // centerline's in the Newton iteration below.
        )
    }

    /// Closest points of the two rails (the `+` rail first) to `p`.
    ///
    /// Starts from the centerline's closest parameter (itself seeded with
    /// `seed_t`) and polishes each rail with a Newton iteration, which is
    /// exact when the ball is near the wireform (within about the radius of
    /// curvature). Never allocates.
    #[must_use]
    pub fn closest_on_rails(&self, p: Vec3, seed_t: Option<f32>) -> [ClosestPoint; 2] {
        let t0 = self.centerline.closest_point(p, seed_t).t;
        [
            self.closest_on_rail(p, t0, 1.0),
            self.closest_on_rail(p, t0, -1.0),
        ]
    }

    fn closest_on_rail(&self, p: Vec3, t0: f32, side: f32) -> ClosestPoint {
        let max_t = self.centerline.max_t();
        let mut t = t0;
        let (mut best_t, mut best_point, mut best_d2) = (t0, Vec3::ZERO, f32::INFINITY);
        for _ in 0..12 {
            let (x, dx) = self.rail_state(t, side);
            let e = x - p;
            let d2 = e.length_squared();
            if d2 < best_d2 {
                best_d2 = d2;
                best_t = t;
                best_point = x;
            }
            let g = e.dot(dx);
            let h = dx.length_squared() + e.dot(self.centerline.second_derivative(t));
            let step = if h > 1.0e-12 {
                (-g / h).clamp(-0.25, 0.25)
            } else if g > 0.0 {
                -0.125
            } else {
                0.125
            };
            let next = (t + step).clamp(0.0, max_t);
            let moved = (next - t).abs();
            t = next;
            if moved < 1.0e-6 {
                break;
            }
        }
        let (x, _) = self.rail_state(t, side);
        let d2 = x.distance_squared(p);
        if d2 <= best_d2 {
            best_d2 = d2;
            best_t = t;
            best_point = x;
        }
        ClosestPoint {
            t: best_t,
            point: best_point,
            distance: best_d2.sqrt(),
        }
    }
}

/// A contact found by [`GuideCurve::cast_sphere`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GuideHit {
    /// Time at which the sphere reaches the guide (seconds).
    pub time: f32,
    /// Curve parameter of the contact point.
    pub t: f32,
    /// Contact point on the guide wall.
    pub point: Vec3,
    /// Unit normal from the wall toward the sphere centre.
    pub normal: Vec3,
}

/// Result of [`GuideCurve::cast_sphere`] when the sphere cannot simply move
/// for the whole time window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GuideCast {
    /// The sphere touches the wall (gap at most 10 µm) at `GuideHit::time`
    /// and is moving into it.
    Contact(GuideHit),
    /// No contact was found up to `time`, but the cast ran out of steps
    /// before the end of the window. The sphere may move to `time` freely
    /// (no impulse); cast again from there.
    Advance {
        /// Time up to which the motion is known to be free (seconds).
        time: f32,
    },
}

/// A single-curve guide (orbit wall): a spline extruded along +Z (the
/// playfield normal) by a wall height. The wall is the zero-thickness ribbon
/// `C(t) + u·Z`, `u ∈ [0, height]`, and collides from both sides.
///
/// Level curves (constant Z, the usual orbit wall) are the normal case. Any
/// level curve is accepted; one that touches or crosses itself has no reach
/// and is collided with by plain conservative advancement, which is slow for
/// a ball sliding along it. A level curve whose two open ends face each
/// other across an opening (a C) has a reach of half that opening; a ball
/// still slides along the rest of the wall at full speed, however narrow
/// the opening.
///
/// A curve that rises or falls is accepted only if a ball can slide along
/// the wall, and ride or skim its top and bottom edges, at full speed: the
/// wall (as a set in space) must have a reach of at least 20 mm. In
/// practice its plan view (the curve seen from above) may not bend tighter
/// than about 22 mm radius — about 30 mm for a plain arc, because a spline
/// bends about a quarter tighter than the arc near its free ends — nor come
/// back onto or within about 44 mm of another stretch, and the curve may not
/// bend up or down tighter than about 22 mm (the edges are copies of it).
/// Helices, loops of more than one turn, hairpins, crossings, vertical runs
/// and sharply rippled walls are rejected by [`GuideCurve::new`] with
/// [`SplineError::SlopedGuidePlanView`]; helical ramps are wireforms. The
/// rule does not depend on the wall height.
///
/// The wall of an accepted sloped guide is a strip of the vertical cylinder
/// over its plan view: beside the wall one plan-view query gives the exact
/// closest point; above or below an edge a short local search from there
/// does (a stationary point closer than the reach is the nearest point);
/// and the sphere cast uses the reach of the wall and of the plan view. Such
/// a guide costs about the same as a level one, beside the wall and at its
/// edges. A ball wider than the reach (the 79 mm XL ball on tight guides)
/// still collides exactly but slides slowly.
///
/// For every guide the through-points must be closer together than the
/// smallest radius of curvature (and roughly evenly spaced, see
/// [`CubicSpline`]); the closest-point search relies on it.
#[derive(Debug, Clone)]
pub struct GuideCurve {
    spline: CubicSpline,
    height: f32,
    /// Z of the curve if every control point has the same Z.
    level_z: Option<f32>,
    /// Conservative estimate of the wall's reach (as a set in space): within
    /// this distance of the wall the nearest wall point is unique, and any
    /// wall point closer than this at which the distance is stationary is
    /// the nearest one. For a level curve the curve's own reach (Federer's
    /// formula with the half-line tangent cone at open ends, so that two
    /// ends facing each other count); for a non-level one [`wall_reach`]
    /// (plan view and edges), at least [`MIN_SLOPED_GUIDE_REACH`]. Zero for
    /// a level curve without reach (one that touches or crosses itself).
    reach: f32,
    /// Level curves whose reach is limited by their open ends only (a ring
    /// with a narrow opening): the radius of the balls that touch the wall
    /// at a point without containing any of it ([`TangentBalls`]). It is
    /// what lets a sphere slide along such a wall as fast as along the same
    /// wall without the opening.
    tangent_balls: Option<TangentBalls>,
    /// Extra data of a non-level curve.
    sloped: Option<Sloped>,
}

/// Tangent-ball radii along a level curve `C`, the pointwise form of the
/// reach.
///
/// For a point `a` of the curve and a unit vector `n`, the open ball of
/// radius `ρ` around `a + ρ n` misses the curve exactly if `|w|² ≥ 2 ρ w · n`
/// for every `w = y − a`, `y` on the curve. If `a` is the nearest point of
/// the curve to some point in direction `n`, then `n` is perpendicular to
/// the tangent at `a`, or, at an end of an open curve, `n · inward ≤ 0`
/// (`inward` the unit tangent into the curve). Hence, with
/// `perp(w)` the distance of `w` from the tangent line at `a` and
/// `s = max(0, −n · inward)` (zero inside the curve),
///
/// `w · n ≤ perp(w) + s · max(0, −w · inward)`,
///
/// and the ball misses the curve if
///
/// `1/ρ ≥ 1/ρ_line(a) + s / ρ_back`,
/// `ρ_line(a) = inf |w|² / (2 perp(w))`,
/// `ρ_back = inf |w|² / (2 max(0, −w · inward))`.
///
/// `ρ_line` is the quantity whose infimum over the curve Federer's formula
/// takes inside the curve; `ρ_back` (half the distance to whatever lies
/// straight behind an end) is what the half-line tangent cone at an end
/// adds. Unlike the reach, the bound is local: a ball sliding along the
/// inside of a C-shaped wall keeps the radius of the C, and only a ball that
/// wraps around an end is limited by the other end facing it.
///
/// What is stored, and how it stays safe between samples and cheap to
/// build:
/// * `ρ_line` at table points, a quarter of a segment apart (`coarse`), and
///   1/64 of a segment apart in the first and the last segment (`fine`),
///   where the two ends make it change quickly. A table entry is the
///   smallest value of the two table points around it and their two
///   neighbours, **less the length of the interval**, times 0.9: next to
///   another stretch of the curve `ρ_line` falls no faster than the
///   distance along the curve (it is about the distance to where that
///   stretch ends or crosses), so the entry holds for every point of its
///   interval whatever the table spacing. There is no resolution to choose
///   and no gap too narrow for it.
/// * The one thing that falls faster is the term of an end point `E` of the
///   curve coming close to the wall, `|E − a|² / (2 perp(E − a))`. The two
///   end points are therefore not left to the table: their terms are
///   evaluated exactly, at the nearest point `a` itself, in
///   [`TangentBalls::radius`].
/// * `ρ_back` is found by a search of the curve behind each end
///   ([`back_radius`]) that subdivides between the samples wherever the
///   infimum could be, so that an end pointing at a wall a fraction of a
///   sample interval away is seen at its true distance.
/// * Values are capped at [`TANGENT_BALL_CAP`] and at 0.9 of the smallest
///   radius of curvature. Larger balls gain nothing in a 1 ms step, and
///   with the cap whole blocks of far points are ruled out by their
///   bounding spheres ([`Cloud`]), which keeps the construction close to
///   linear in the number of points.
///
/// The same balls miss the wall `C × [0, h]`: for a wall point above `a`
/// nearest to `p`, the vertical part of `n` points away from the wall (or
/// is zero), so `w · n ≤ w_h · n_h`, and `|w| ≥ |w_h|`.
#[derive(Debug, Clone)]
struct TangentBalls {
    /// Entries per quarter segment along the whole curve.
    coarse: Vec<f32>,
    /// Entries per 1/64 of the first and of the last segment.
    fine: [Vec<f32>; 2],
    /// The first and the last point of the curve: (point, unit tangent into
    /// the curve, `ρ_back` with the 0.9 margin).
    ends: [(Vec3, Vec3, f32); 2],
}

/// Largest tangent-ball radius the table holds (metres).
const TANGENT_BALL_CAP: f64 = 0.3;

/// Table intervals per segment: along the curve, and in its end segments.
const TANGENT_BALL_COARSE: usize = 4;
const TANGENT_BALL_FINE: usize = 64;

/// Points of a curve in blocks with bounding spheres, for the pair tests of
/// the reach estimates: does any point `y` change the estimate `r` of
/// `inf |w|² / (2 off)` at `x` (`w = y − x`, `off` its distance from the
/// tangent line at `x`), i.e. is `2 off r > |w|²`?
struct Cloud {
    xs: Vec<f64>,
    ys: Vec<f64>,
    zs: Vec<f64>,
    /// (centre, radius) per block of [`Cloud::BLOCK`] consecutive points.
    blocks: Vec<(DVec3, f64)>,
    /// (centre, radius) per group of [`Cloud::GROUP`] consecutive points.
    groups: Vec<(DVec3, f64)>,
}

/// Bounding sphere (centre, radius) of `points`.
fn bounding_sphere(points: &[DVec3]) -> (DVec3, f64) {
    let (mut min, mut max) = (points[0], points[0]);
    for p in points {
        min = min.min(*p);
        max = max.max(*p);
    }
    let centre = 0.5 * (min + max);
    let radius = points
        .iter()
        .map(|p| p.distance(centre))
        .fold(0.0f64, f64::max);
    (centre, radius * (1.0 + 1.0e-9))
}

/// Whether no point of a sphere can change the estimate `r` at `x`, nor
/// coincide with `x`: no point is nearer than `near`, none farther from the
/// tangent line than `off` (which is 1-Lipschitz), and `off ≤ |w|`.
#[inline(always)]
fn sphere_is_quiet(sphere: (DVec3, f64), x: DVec3, tangent: DVec3, r: f64) -> bool {
    let (centre, radius) = sphere;
    let w = centre - x;
    let near = w.length() - radius;
    let off = (w - tangent * w.dot(tangent)).length() + radius;
    near > 1.0e-6 && near * near >= 2.0 * off * r * (1.0 + 1.0e-9)
}

impl Cloud {
    const BLOCK: usize = 32;

    /// Points per group (16 blocks).
    const GROUP: usize = 16 * Self::BLOCK;

    fn new(points: &[DVec3]) -> Self {
        Self {
            xs: points.iter().map(|p| p.x).collect(),
            ys: points.iter().map(|p| p.y).collect(),
            zs: points.iter().map(|p| p.z).collect(),
            blocks: points.chunks(Self::BLOCK).map(bounding_sphere).collect(),
            groups: points.chunks(Self::GROUP).map(bounding_sphere).collect(),
        }
    }

    /// Whether no point of the group starting at `from` can change the
    /// estimate `r` at `x`, nor coincide with `x`.
    #[inline(always)]
    fn group_is_quiet(&self, from: usize, x: DVec3, tangent: DVec3, r: f64) -> bool {
        sphere_is_quiet(self.groups[from / Self::GROUP], x, tangent, r)
    }

    fn len(&self) -> usize {
        self.xs.len()
    }

    fn point(&self, index: usize) -> DVec3 {
        DVec3::new(self.xs[index], self.ys[index], self.zs[index])
    }

    /// Whether no point of the block starting at `from` can change the
    /// estimate `r` at `x`, nor coincide with `x`. Two tests, both with a
    /// 1e-9 margin (rounding is 1e-15): the bounding sphere, then pair by
    /// pair with `off² = |w|² − (w · t)²`, without a square root or a
    /// branch.
    #[inline(always)]
    fn quiet(&self, from: usize, x: DVec3, tangent: DVec3, r: f64) -> bool {
        let to = (from + Self::BLOCK).min(self.len());
        if sphere_is_quiet(self.blocks[from / Self::BLOCK], x, tangent, r) {
            return true;
        }
        let scale = 4.0 * r * r;
        let wide = scale * (1.0 + 1.0e-9);
        let tight = 1.0 - 1.0e-9;
        let (tx, ty, tz) = (tangent.x, tangent.y, tangent.z);
        // A branch-free count, so that the loop vectorises.
        let mut hits = 0u32;
        for ((&px, &py), &pz) in self.xs[from..to]
            .iter()
            .zip(&self.ys[from..to])
            .zip(&self.zs[from..to])
        {
            let (wx, wy, wz) = (px - x.x, py - x.y, pz - x.z);
            let w2 = wx * wx + wy * wy + wz * wz;
            let along = wx * tx + wy * ty + wz * tz;
            let e = w2 * wide - along * along * scale - w2 * w2 * tight;
            hits += u32::from(e > 0.0) + u32::from(w2 < 1.0e-15);
        }
        hits == 0
    }

    /// `min(cap, inf |w|² / (2 off))` at `x` over the points of the cloud.
    fn row(&self, x: DVec3, tangent: DVec3, cap: f64) -> f64 {
        let mut r = cap;
        let update = |r: &mut f64, index: usize| {
            let w = self.point(index) - x;
            let w2 = w.length_squared();
            if w2 >= 1.0e-16 {
                let off = (w - tangent * w.dot(tangent)).length();
                if 2.0 * off * *r > w2 {
                    *r = w2 / (2.0 * off);
                }
            }
        };
        // A sparse pass first: next to another stretch of the curve it
        // brings the estimate down at once, and most blocks are then ruled
        // out by their spheres.
        for index in (0..self.len()).step_by(4 * Self::BLOCK) {
            update(&mut r, index);
        }
        let mut from = 0;
        while from < self.len() {
            if from % Self::GROUP == 0 && self.group_is_quiet(from, x, tangent, r) {
                from += Self::GROUP;
                continue;
            }
            let block = from;
            from += Self::BLOCK;
            if self.quiet(block, x, tangent, r) {
                continue;
            }
            for index in block..(block + Self::BLOCK).min(self.len()) {
                update(&mut r, index);
            }
        }
        r
    }
}

/// Distance from an end of an open curve, in units of the curve parameter
/// (1 per segment, so a thousandth of the last segment, not a length),
/// within which the nearest point counts as the end itself
/// ([`TangentBalls`]). Any width is safe: the end term only ever lowers the
/// radius, so applying it to an interior point is merely conservative, and
/// a nearest point that is the end has the parameter of the end exactly
/// (the closest-point search clamps to it). The band is there for rounding
/// of the parameter, and kept narrow so that a sphere sliding past the end
/// region is not slowed down.
const END_BAND: f32 = 1.0e-3;

/// `ρ_back` of the end `e` of `spline` (`inward` the unit tangent into the
/// curve there): the infimum of `|w|² / (2 b)` over curve points `y` behind
/// the end, `w = y − e`, `b = −w · inward > 0`. At most 1e3.
///
/// Branch and bound over the sample intervals: on a piece of the curve of
/// length at most `L` between two points at distances `d0`, `d1` from `e`,
/// `|w| ≥ min(d0, d1) − L/2` and `b ≤ max(b0, b1) + L/2`, so the value is
/// at least `(min(d0, d1) − L/2)² / (2 (max(b0, b1) + L/2))`; a piece that
/// cannot hold a value more than 2 % below the best one found is dropped,
/// the others are halved (down to 1/65536 of a sample interval). One pass over the
/// samples plus a few dozen evaluations near each candidate.
fn back_radius(spline: &CubicSpline, e: DVec3, inward: DVec3) -> f64 {
    const DEPTH: u32 = 16;
    let samples = spline.samples();
    let k = SAMPLES_PER_SEGMENT as f64;
    let mut best = 1.0e3f64;
    // (parameter, distance², behind) of a curve point, and its value.
    let at = |t: f64| {
        let w = spline.point(t as f32).as_dvec3() - e;
        (t, w.length_squared(), -w.dot(inward))
    };
    let value = |p: (f64, f64, f64)| {
        if p.2 > 0.0 && p.1 > 1.0e-16 {
            p.1 / (2.0 * p.2)
        } else {
            f64::INFINITY
        }
    };
    // The samples first: a good value to prune with from the start.
    for sample in samples {
        let w = sample.point.as_dvec3() - e;
        best = best.min(value((0.0, w.length_squared(), -w.dot(inward))));
    }
    // (from, to, length, depth) of the pieces still to look at.
    type Probe = (f64, f64, f64);
    let mut stack: Vec<(Probe, Probe, f64, u32)> = Vec::new();
    for (i, pair) in samples.windows(2).enumerate() {
        let from = pair[0].point.as_dvec3();
        let to = pair[1].point.as_dvec3();
        let (w0, w1) = (from - e, to - e);
        let p0 = (i as f64 / k, w0.length_squared(), -w0.dot(inward));
        let p1 = ((i + 1) as f64 / k, w1.length_squared(), -w1.dot(inward));
        // Arc length of the interval, generously (it is nearly straight).
        stack.push((p0, p1, 1.25 * from.distance(to), 0));
        while let Some((p0, p1, length, depth)) = stack.pop() {
            best = best.min(value(p0)).min(value(p1));
            let behind = p0.2.max(p1.2) + 0.5 * length;
            if behind <= 0.0 {
                continue; // wholly in front of the end
            }
            let nearest = (p0.1.min(p1.1).sqrt() - 0.5 * length).max(0.0);
            // (A piece that could improve the value by less than 2 % is
            // not worth it: the result carries a 10 % margin.)
            if nearest * nearest >= 0.98 * 2.0 * behind * best || depth == DEPTH {
                continue;
            }
            let mid = at(0.5 * (p0.0 + p1.0));
            stack.push((p0, mid, 0.5 * length, depth + 1));
            stack.push((mid, p1, 0.5 * length, depth + 1));
        }
    }
    best
}

impl TangentBalls {
    /// The table of an open curve without cusp or crossing.
    fn new(spline: &CubicSpline) -> Option<Self> {
        let curvature = curvature_bound(spline)?;
        let cap = if 0.9 > curvature * TANGENT_BALL_CAP {
            TANGENT_BALL_CAP
        } else {
            0.9 / curvature
        };
        let samples = spline.samples();
        let last = samples.len() - 1;
        let segments = spline.segment_count();
        let fine = TANGENT_BALL_FINE;
        // (point, unit tangent) at 1/64 steps of a segment.
        let fine_points = |segment: usize| -> Vec<(DVec3, DVec3)> {
            (0..=fine)
                .map(|j| {
                    let (c, d1, _) = spline.eval(segment, j as f32 / fine as f32);
                    (c.as_dvec3(), d1.as_dvec3().normalize())
                })
                .collect()
        };
        let first = fine_points(0);
        let final_ = fine_points(segments - 1);
        // What every table point is compared with: every second sample,
        // and the end segments as finely as their tables.
        let mut cloud: Vec<DVec3> = samples
            .iter()
            .step_by(2)
            .map(|sample| sample.point.as_dvec3())
            .collect();
        cloud.extend(first.iter().map(|p| p.0));
        cloud.extend(final_.iter().map(|p| p.0));
        let cloud = Cloud::new(&cloud);
        // Along the curve.
        let stride = SAMPLES_PER_SEGMENT / TANGENT_BALL_COARSE;
        let count = last / stride + 1;
        let rows: Vec<f64> = (0..count)
            .map(|a| {
                let sample = &samples[a * stride];
                cloud.row(
                    sample.point.as_dvec3(),
                    sample.tangent.as_dvec3().normalize(),
                    cap,
                )
            })
            .collect();
        let entry = |lowest: f64, length: f64| (0.9 * (lowest - length).max(0.0)) as f32;
        let coarse = (0..count - 1)
            .map(|i| {
                let length: f32 = samples[i * stride..=(i + 1) * stride]
                    .windows(2)
                    .map(|pair| pair[0].point.distance(pair[1].point))
                    .sum();
                // Table points i − 1 ..= i + 2.
                let lowest = rows[i.saturating_sub(1)..=(i + 2).min(count - 1)]
                    .iter()
                    .fold(f64::INFINITY, |m, &r| m.min(r));
                entry(lowest, f64::from(length))
            })
            .collect();
        // In an end segment; `inner` is the table point of the coarse table
        // a quarter segment beyond its inner end, if there is one.
        let fine_table = |points: &[(DVec3, DVec3)], inner_first: bool, inner: Option<f64>| {
            let rows: Vec<f64> = points
                .iter()
                .map(|&(x, tangent)| cloud.row(x, tangent, cap))
                .collect();
            (0..fine)
                .map(|j| {
                    let mut lowest = rows[j.saturating_sub(1)..=(j + 2).min(fine)]
                        .iter()
                        .fold(f64::INFINITY, |m, &r| m.min(r));
                    let at_inner = if inner_first { j == 0 } else { j + 2 > fine };
                    if let (true, Some(beyond)) = (at_inner, inner) {
                        lowest = lowest.min(beyond);
                    }
                    entry(lowest, 1.05 * points[j].0.distance(points[j + 1].0))
                })
                .collect::<Vec<f32>>()
        };
        let per = TANGENT_BALL_COARSE;
        let fine_tables = [
            fine_table(&first, false, rows.get(per + 1).copied()),
            fine_table(
                &final_,
                true,
                (count > per + 2).then(|| rows[count - 2 - per]),
            ),
        ];
        let end = |point: DVec3, inward: DVec3| {
            let back = 0.9 * back_radius(spline, point, inward);
            (point.as_vec3(), inward.as_vec3(), back as f32)
        };
        Some(Self {
            coarse,
            fine: fine_tables,
            ends: [
                end(first[0].0, first[0].1),
                end(final_[fine].0, -final_[fine].1),
            ],
        })
    }

    /// Radius of a ball that touches the wall at `point` (above the curve
    /// point of parameter `t`) on the side of the unit vector `normal` (the
    /// direction from that nearest point to the query point) and contains
    /// none of the wall. Zero where there is none to promise.
    fn radius(&self, spline: &CubicSpline, t: f32, point: Vec3, normal: Vec3) -> f32 {
        let max_t = spline.max_t();
        let t = t.clamp(0.0, max_t);
        let fine = TANGENT_BALL_FINE;
        let in_fine = |local: f32| ((local * fine as f32) as usize).min(fine - 1);
        let mut rho = f32::INFINITY;
        if t <= 1.0 {
            rho = rho.min(self.fine[0][in_fine(t)]);
        }
        if t >= max_t - 1.0 {
            rho = rho.min(self.fine[1][in_fine(t - (max_t - 1.0))]);
        }
        if rho == f32::INFINITY {
            let index = (t * TANGENT_BALL_COARSE as f32) as usize;
            rho = self.coarse[index.min(self.coarse.len() - 1)];
        }
        if rho <= 0.0 {
            return 0.0;
        }
        let tangent = spline.tangent(t).normalize_or_zero();
        // 1/ρ_line: the table, and the two end points exactly.
        let mut inverse = 1.0 / rho;
        for (end, _, _) in &self.ends {
            // (The curve is level: only the plan position matters.)
            let w = Vec3::new(end.x - point.x, end.y - point.y, 0.0);
            let w2 = w.length_squared();
            if w2 > 1.0e-12 {
                let off = (w - tangent * w.dot(tangent)).length();
                inverse = inverse.max(2.0 * off / (0.9 * w2));
            }
        }
        for (end_t, (_, inward, back)) in [0.0, max_t].into_iter().zip(&self.ends) {
            let s = -normal.dot(*inward);
            if (t - end_t).abs() <= END_BAND && s > 0.0 {
                inverse += s / back;
            }
        }
        1.0 / inverse
    }
}

/// The reach estimates of a level curve with the half-line tangent cone at
/// open ends and with whole tangent lines: bit for bit
/// `(curve_reach(spline, true), curve_reach(spline, false))`, in one pass
/// and several times faster.
///
/// A pair `(x, y)` changes an estimate `r` only if `2 off r > |w|²`. That
/// is excluded for whole blocks of `y` at a time ([`Cloud::quiet`], against
/// the larger of the two estimates); only blocks that fail the test, the
/// points next to `x` and the few that set the estimate, go through the
/// exact update of [`curve_reach`], in the same order. Rows of the two end
/// points are always exact.
fn level_reaches(spline: &CubicSpline) -> (f32, f32) {
    const MAX_REACH: f64 = 1.0e3;
    let samples = spline.samples();
    let Some(curvature) = curvature_bound(spline) else {
        return (0.0, 0.0); // cusp
    };
    let mut initial = MAX_REACH;
    if curvature * initial > 1.0 {
        initial = 1.0 / curvature;
    }
    let stride = 4;
    let count = (samples.len() - 1) / stride + 1;
    let last = samples.len() - 1;
    let index = |j: usize| (j * stride).min(last);
    let points: Vec<DVec3> = (0..count)
        .map(|j| samples[index(j)].point.as_dvec3())
        .collect();
    let tangents: Vec<DVec3> = (0..count)
        .map(|j| samples[index(j)].tangent.as_dvec3().normalize())
        .collect();
    let cloud = Cloud::new(&points);
    let open = !spline.is_closed();
    let (mut half, mut line) = (initial, initial);
    for a in 0..count {
        let (x, tangent) = (points[a], tangents[a]);
        let end_direction = if open && a == 0 {
            Some(tangent)
        } else if open && index(a) == last {
            Some(-tangent)
        } else {
            None
        };
        let mut from = 0;
        while from < count {
            if end_direction.is_none()
                && from % Cloud::GROUP == 0
                && cloud.group_is_quiet(from, x, tangent, half.max(line))
            {
                from += Cloud::GROUP;
                continue;
            }
            let to = (from + Cloud::BLOCK).min(count);
            let exact = end_direction.is_some() || !cloud.quiet(from, x, tangent, half.max(line));
            if exact {
                for b in from..to {
                    if a == b {
                        continue;
                    }
                    let w = points[b] - x;
                    let w2 = w.length_squared();
                    if w2 < 1.0e-16 {
                        // Coincident samples: a smoothly closed loop is
                        // fine, a crossing has no reach.
                        if tangent.cross(tangents[b]).length() > 1.0e-3 {
                            return (0.0, 0.0);
                        }
                        continue;
                    }
                    let off = (w - tangent * w.dot(tangent)).length();
                    if 2.0 * off * line > w2 {
                        line = w2 / (2.0 * off);
                    }
                    let off = match end_direction {
                        Some(inward) if w.dot(inward) < 0.0 => w2.sqrt(),
                        _ => off,
                    };
                    if 2.0 * off * half > w2 {
                        half = w2 / (2.0 * off);
                    }
                }
            }
            from = to;
        }
    }
    ((0.9 * half) as f32, (0.9 * line) as f32)
}

/// What a rising or falling guide needs besides its curve.
#[derive(Debug, Clone)]
struct Sloped {
    /// Plan view (Z = 0): the spline through the projected through-points,
    /// which is the projection of the curve because the interpolation is
    /// linear in the points.
    flat: CubicSpline,
    /// Reach of the plan view (the vertical cylinder over it has the same).
    plan_reach: f32,
    /// Bounding sphere (centre, radius) of every segment of the curve.
    bounds: Vec<(Vec3, f32)>,
}

/// (squared distance, curve parameter, wall point) of a closest-point search.
type Best = (f32, f32, Vec3);

/// Smallest reach of a guide wall that rises or falls (metres, after the
/// 0.9 sampling margin of the estimate), for its plan view and for the wall
/// with its edges ([`wall_reach`]): no bend tighter than about 22 mm radius,
/// in plan or up and down, and no two stretches of the plan view closer
/// than about 44 mm unless the curve between them is that short. Below this
/// a pinball (radius up to 16 mm) could not use the reach bound of the
/// sphere cast and would crawl along the wall or its edges.
const MIN_SLOPED_GUIDE_REACH: f32 = 0.02;

/// Whether a plan-view curve comes back onto itself: two points whose
/// distance is smaller than the chord a curve of reach
/// [`MIN_SLOPED_GUIDE_REACH`] keeps between points that far apart along it
/// (Schur's comparison: `2ρ · sin(arc / 2ρ)` up to half a turn, `2ρ`
/// beyond). This is what catches a helix, whose laps lie on top of each
/// other in plan: as a point set its plan view is a plain circle, with a
/// perfectly good reach.
fn plan_view_folds(flat: &CubicSpline, closed: bool) -> bool {
    let samples = flat.samples();
    let rho = f64::from(MIN_SLOPED_GUIDE_REACH);
    // Arc length up to every second sample (chord sums).
    let stride = 2;
    let count = (samples.len() - 1) / stride + 1;
    let at = |j: usize| {
        samples[(j * stride).min(samples.len() - 1)]
            .point
            .as_dvec3()
    };
    let mut total = 0.0f64;
    let mut prev = samples[0].point.as_dvec3();
    for sample in &samples[1..] {
        let point = sample.point.as_dvec3();
        total += point.distance(prev);
        prev = point;
    }
    for a in 0..count {
        let x = at(a);
        let mut arc = 0.0f64;
        for b in a + 1..count {
            for index in (b - 1) * stride..(b * stride).min(samples.len() - 1) {
                arc += samples[index + 1]
                    .point
                    .as_dvec3()
                    .distance(samples[index].point.as_dvec3());
            }
            // Around a closed curve the shorter way counts.
            let along = if closed { arc.min(total - arc) } else { arc };
            let keep = 2.0
                * rho
                * (along / (2.0 * rho))
                    .min(core::f64::consts::FRAC_PI_2)
                    .sin();
            if at(b).distance(x) < 0.98 * keep {
                return true;
            }
        }
    }
    false
}

/// Bounding sphere of every segment of `spline`: the centre of its samples'
/// box, and the largest distance to a sample plus half a sample interval's
/// arc length (what the curve can do between two samples).
fn segment_bounds(spline: &CubicSpline) -> Vec<(Vec3, f32)> {
    let samples = spline.samples();
    let k = SAMPLES_PER_SEGMENT;
    (0..spline.segment_count())
        .map(|i| {
            let part = &samples[i * k..=(i + 1) * k];
            let (mut min, mut max) = (part[0].point, part[0].point);
            let mut speed = 0.0f32;
            for sample in part {
                min = min.min(sample.point);
                max = max.max(sample.point);
                speed = speed.max(sample.tangent.length());
            }
            let center = 0.5 * (min + max);
            let third = (spline.eval(i, 1.0).2 - spline.eval(i, 0.0).2).length();
            let step = 1.0 / k as f32;
            let between = 0.5 * step * (speed + step * step * 0.125 * third);
            let radius = part
                .iter()
                .map(|sample| sample.point.distance(center))
                .fold(0.0f32, f32::max);
            (center, radius + between)
        })
        .collect()
}

/// The plan view of `spline`, if its projected through-points are distinct.
fn plan_view(spline: &CubicSpline) -> Option<CubicSpline> {
    let segments = spline.segments();
    let mut points: Vec<Vec3> = segments.iter().map(|seg| seg.p[0]).collect();
    points.push(segments[segments.len() - 1].p[3]);
    for p in &mut points {
        p.z = 0.0;
    }
    if spline.is_closed() {
        CubicSpline::interpolate_closed(&points).ok()
    } else {
        CubicSpline::interpolate(&points).ok()
    }
}

/// Upper bound of the curvature of `spline` over every sample interval (not
/// just at the samples): `κ ≤ |C''| / |C'|²`. `C''` is linear per segment,
/// so its largest norm on an interval is at an end; `|C'|` cannot fall below
/// the smaller end value by more than half the interval times that norm.
/// `None` at a cusp (`|C'|` may vanish).
fn curvature_bound(spline: &CubicSpline) -> Option<f64> {
    let k = SAMPLES_PER_SEGMENT;
    let n = spline.segment_count();
    let h = 1.0 / k as f64;
    let derivatives = |index: usize| {
        let i = (index / k).min(n - 1);
        let s = (index - i * k) as f32 / k as f32;
        let (_, d1, d2) = spline.eval(i, s);
        (d1.as_dvec3().length(), d2.as_dvec3().length())
    };
    let mut curvature = 0.0f64;
    let mut left = derivatives(0);
    for index in 1..spline.samples().len() {
        let right = derivatives(index);
        let bend = left.1.max(right.1);
        let slowest = left.0.min(right.0) - 0.5 * h * bend;
        if slowest <= 1.0e-12 {
            return None;
        }
        curvature = curvature.max(bend / (slowest * slowest));
        left = right;
    }
    Some(curvature)
}

/// Reach of a sampled curve (Federer): the infimum over point pairs of
/// `|y - x|² / (2 · dist(y - x, tangent line at x))`, which accounts for both
/// curvature and distinct stretches of the curve approaching each other.
fn curve_reach(spline: &CubicSpline, half_line_ends: bool) -> f32 {
    const MAX_REACH: f64 = 1.0e3;
    let samples = spline.samples();
    let Some(curvature) = curvature_bound(spline) else {
        return 0.0; // cusp
    };
    let mut reach = MAX_REACH;
    if curvature * reach > 1.0 {
        reach = 1.0 / curvature;
    }
    // Pairs, on every fourth sample (plus the last).
    let stride = 4;
    let count = (samples.len() - 1) / stride + 1;
    let last = samples.len() - 1;
    let at = |j: usize| {
        let sample = &samples[(j * stride).min(last)];
        (
            sample.point.as_dvec3(),
            sample.tangent.as_dvec3().normalize(),
        )
    };
    let open = half_line_ends && !spline.is_closed();
    for a in 0..count {
        let (x, tangent) = at(a);
        // At an end of an open curve the tangent cone is the half-line into
        // the curve, not the whole tangent line: a point behind the end is
        // at its full distance from it (Federer's formula with Tan(A, x)).
        // (With the whole line the estimate is too large when something
        // lies behind an end, e.g. the two ends of a C facing each other:
        // every caller that needs a reach passes `true`.)
        let end_direction = if open && a == 0 {
            Some(tangent)
        } else if open && (a * stride).min(last) == last {
            Some(-tangent)
        } else {
            None
        };
        for b in 0..count {
            if a == b {
                continue;
            }
            let (y, other_tangent) = at(b);
            let w = y - x;
            let w2 = w.length_squared();
            if w2 < 1.0e-16 {
                // Coincident samples: a smoothly closed loop is fine, a
                // crossing has no reach.
                if tangent.cross(other_tangent).length() > 1.0e-3 {
                    return 0.0;
                }
                continue;
            }
            let off = match end_direction {
                Some(inward) if w.dot(inward) < 0.0 => w2.sqrt(),
                _ => (w - tangent * w.dot(tangent)).length(),
            };
            if 2.0 * off * reach > w2 {
                reach = w2 / (2.0 * off);
            }
        }
    }
    // Margin for what the sampling cannot see.
    (0.9 * reach) as f32
}

/// Reach of a rising or falling wall (as a set in space): at least
/// `min(ρ_P, ρ_E)`, `ρ_P` the plan view's reach ([`curve_reach`]) and `ρ_E`
/// the edge term below. The result does not depend on the wall height.
///
/// Federer 1959, Thm 4.18: for a closed set `W`, `reach(W) = inf |w|² /
/// (2 dist(w, Tan(W, x)))` over pairs `x ≠ y` of `W`, `w = y − x`. The wall
/// lies on the vertical cylinder over the plan view, so `Tan(W, x)` lies in
/// the cylinder's tangent plane, whose normal `n` is horizontal:
/// `dist(w, Tan)² = (w · n)² + dist(w', cone)²`, `w'` the in-plane part of
/// `w` and `cone` the in-plane tangent cone.
/// * `x` inside the wall or on its vertical end edges: the cone is the
///   plane, or the half-plane on the curve's side of the end; with
///   `w · n = w_h · n` and `|w| ≥ |w_h|` the pair is no worse than the plan
///   view's pair `(x_h, y_h)` with the line (half-line at an end, which
///   [`curve_reach`] uses) as tangent cone: at least `ρ_P`.
/// * `x` on the top or bottom edge: the cone is the half-plane on the
///   wall's side of the edge tangent, so `dist(w', cone) = max(0, w · m)`,
///   `m` the in-plane unit vector perpendicular to the edge pointing away
///   from the wall. At the four corners the cone is a convex wedge; a `w'`
///   on the curve's side of the end is as far from it as from the edge
///   half-plane, and any other `w` points behind the plan view's end, where
///   `dist(w, Tan) ≤ |w|` and the ratio is at least `|w_h| / 2 ≥ ρ_P`.
///
/// The edge term `1/ρ_E = sup 2 √((w·n)² + max(0, w·m)²) / |w|²` over `x` on
/// an edge has two parts, as in [`curve_reach`]:
/// * Nearby pairs: along the edge the limit is the edge's curvature, whose
///   components along `n` and `m` make up the whole curvature of `C` (the
///   edges are translates of `C`), bounded on every sample interval by
///   [`curvature_bound`]; from other directions into the wall `w · m < 0`
///   and the limit is the plan view's (`ρ_P`).
/// * Distant pairs on every fourth sample (`x`) and every second sample
///   (`y`), 0.9 margin. With `y = C(t') + λZ` relative to the edge,
///   `D = C(t') − C(t)`, the two terms are bounded separately over the
///   height (`λ ≤ 0` below a top edge, `λ ≥ 0` above a bottom edge, without
///   limit, which covers walls of every height):
///   `2|D_h · n| / (|D_h|² + (D_z + λ)²)` is largest at `λ = −D_z` or `0`,
///   and `F(λ) = (a + bλ) / (|D_h|² + (D_z + λ)²)`, `a = D · m`, `b = m · Z`,
///   at `0` or a stationary point, a root of
///   `b λ² + 2 a λ + (2 a D_z − b |D|²) = 0` (discriminant
///   `(a − b D_z)² + b² |D_h|² ≥ 0`).
///
/// Returns 0 at a cusp.
fn wall_reach(spline: &CubicSpline, plan_reach: f32) -> f32 {
    let Some(curvature) = curvature_bound(spline) else {
        return 0.0;
    };
    let samples = spline.samples();
    let last = samples.len() - 1;
    let mut far = 0.0f64;
    for xa in (0..=last).step_by(4).chain(core::iter::once(last)) {
        let x = samples[xa].point.as_dvec3();
        let tangent = samples[xa].tangent.as_dvec3().normalize();
        let flat = DVec3::new(tangent.x, tangent.y, 0.0).length();
        if flat <= 1.0e-9 {
            return 0.0;
        }
        let up = (DVec3::Z - tangent * tangent.z) / flat;
        let normal = DVec3::new(-tangent.y, tangent.x, 0.0) / flat;
        for yb in (0..=last).step_by(2) {
            let d = samples[yb].point.as_dvec3() - x;
            let horizontal = d.x * d.x + d.y * d.y;
            // The same point (or the seam of a closed curve), or a pair that
            // cannot matter: both terms are at most 2 / |w| ≤ 2 / |D_h|.
            if horizontal <= 1.0e-16
                || (far > 0.0 && 2.0 * core::f64::consts::SQRT_2 <= far * horizontal.sqrt())
            {
                continue;
            }
            let e = d.z;
            let across = d.dot(normal).abs();
            for sign in [1.0f64, -1.0] {
                let allowed = |lambda: f64| lambda * sign <= 0.0;
                let lowest = if allowed(-e) {
                    horizontal
                } else {
                    horizontal + e * e
                };
                let side = 2.0 * across / lowest;
                let m = up * sign;
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
                far = far.max((side * side + off * off).sqrt());
            }
        }
    }
    let inverse = curvature.max(far / 0.9);
    let edge = if inverse > 0.0 {
        (1.0 / inverse) as f32
    } else {
        f32::INFINITY
    };
    plan_reach.min(edge)
}

/// Earliest time at which a sphere moving with `velocity` could have its gap
/// to a set of reach at least `reach` shrink by `drop`, given its distance
/// `distance < reach` to the set and the rate `rate` (positive: away) at which
/// that distance changes now.
///
/// Proof: let `a` be the nearest point of the set, `n` the unit vector from
/// `a` to the centre `p`. For every `s < reach` the point `a + s n` has `a`
/// as its nearest point (Federer 1959, Thm 4.8 (12)), so the open ball of
/// radius `s` around it misses the set; their union is the open ball of
/// radius `reach` around `c = a + reach · n`. Hence, everywhere,
/// `distance(x) ≥ reach − |x − c|`, with equality at `p`. Along
/// `x(τ) = p + vτ`, with `R = reach − distance`, `|x − c|² = R² − 2 R rate τ +
/// |v|² τ²`; the distance stays above `distance − drop` while
/// `|x − c| ≤ R + drop`, i.e. up to the positive root of
/// `|v|² τ² − 2 R rate τ − drop (2R + drop) = 0`. No other condition: the
/// bound holds for all `τ`.
fn rolling_ball_time(distance: f32, reach: f32, rate: f32, drop: f32, speed: f32) -> f32 {
    let r = reach - distance;
    if !(r > 0.0 && speed > 0.0 && drop >= 0.0) {
        return 0.0;
    }
    let x = drop * (2.0 * r + drop);
    let disc = (r * r * rate * rate + speed * speed * x).sqrt();
    // Written so that neither sign of `rate` cancels.
    if rate < 0.0 {
        x / (disc - r * rate)
    } else {
        (r * rate + disc) / (speed * speed)
    }
}

impl GuideCurve {
    /// Wraps `spline` (the bottom edge of the wall) with a wall `height_m`.
    ///
    /// # Errors
    /// Non-finite or negative height; a closed curve that is not periodic;
    /// a rising or falling curve whose plan view a ball cannot follow
    /// ([`SplineError::SlopedGuidePlanView`], see the type documentation).
    pub fn new(spline: CubicSpline, height_m: f32) -> Result<Self, SplineError> {
        if !height_m.is_finite() {
            return Err(SplineError::NonFinite);
        }
        if height_m < 0.0 {
            return Err(SplineError::InvalidHeight);
        }
        let first = spline.segments()[0].p[0];
        let last = spline.segments()[spline.segment_count() - 1].p[3];
        if first == last && !spline.is_closed() {
            // A natural spline has a kink where it closes: no reach, and
            // therefore a contact every few micrometres of sliding.
            return Err(SplineError::ClosedGuideNotPeriodic);
        }
        let z0 = first.z;
        let level = spline
            .segments()
            .iter()
            .all(|seg| seg.p.iter().all(|p| (p.z - z0).abs() <= 1.0e-6));
        let mut tangent_balls = None;
        let (sloped, reach) = if level {
            // The true reach: at an open end the tangent cone is the
            // half-line into the curve, so whatever lies behind an end (the
            // other end of a C) limits it. Where it does, i.e. where the
            // estimate with whole tangent lines is larger, the reach says
            // little about the rest of the wall and the cast also uses the
            // tangent balls; everywhere else the reach is used alone, as
            // before.
            let (reach, line) = level_reaches(&spline);
            if reach > 0.0 && reach < line && !spline.is_closed() {
                tangent_balls = TangentBalls::new(&spline);
            }
            (None, reach)
        } else {
            // A rising or falling guide needs a plan view that a ball can
            // slide along at full speed: see `MIN_SLOPED_GUIDE_REACH`.
            let flat = plan_view(&spline).ok_or(SplineError::SlopedGuidePlanView)?;
            let plan_reach = curve_reach(&flat, true);
            if plan_reach < MIN_SLOPED_GUIDE_REACH || plan_view_folds(&flat, spline.is_closed()) {
                return Err(SplineError::SlopedGuidePlanView);
            }
            // The edges, too, must leave a ball room to ride or skim them.
            let reach = wall_reach(&spline, plan_reach);
            if reach < MIN_SLOPED_GUIDE_REACH {
                return Err(SplineError::SlopedGuidePlanView);
            }
            let bounds = segment_bounds(&spline);
            let sloped = Sloped {
                flat,
                plan_reach,
                bounds,
            };
            (Some(sloped), reach)
        };
        Ok(Self {
            spline,
            height: height_m,
            level_z: level.then_some(z0),
            reach,
            tangent_balls,
            sloped,
        })
    }

    /// The bottom-edge spline.
    #[must_use]
    pub fn spline(&self) -> &CubicSpline {
        &self.spline
    }

    /// Wall height (metres).
    #[must_use]
    pub fn height(&self) -> f32 {
        self.height
    }

    /// Whether the curve is level (constant Z).
    #[must_use]
    pub fn is_level(&self) -> bool {
        self.level_z.is_some()
    }

    /// The conservative reach estimate of the wall (metres) that the sphere
    /// cast and the closest-point query rely on: within it the nearest wall
    /// point is unique. Zero for a level curve that touches or crosses
    /// itself. For tests and diagnostics.
    #[doc(hidden)]
    #[must_use]
    pub fn reach(&self) -> f32 {
        self.reach
    }

    /// Closest point of the wall to `p`; `t` is the curve parameter below the
    /// returned point. Never allocates.
    ///
    /// Global for level curves (one spline query) and for rising or falling
    /// curves: one query of the plan view when `p` is beside the wall;
    /// otherwise (above or below the wall's edge) a local search from there,
    /// exact whenever `p` is within the wall's reach (at least 20 mm), and
    /// beyond it a search of the curve's segments, pruned by their bounding
    /// spheres. A minimum can only be missed where the curve bends on the
    /// scale of a sample interval (1/16 of a segment; 1/64 in the segment
    /// search): keep the through-point spacing below the smallest radius of
    /// curvature.
    #[must_use]
    pub fn closest_point(&self, p: Vec3, seed_t: Option<f32>) -> ClosestPoint {
        if !p.is_finite() {
            return self.spline.closest_point(p, seed_t);
        }
        if let Some(z0) = self.level_z {
            // The height on the wall does not depend on the parameter.
            let u = (p.z - z0).clamp(0.0, self.height);
            let cp = self
                .spline
                .closest_point(Vec3::new(p.x, p.y, p.z - u), seed_t);
            let point = Vec3::new(cp.point.x, cp.point.y, cp.point.z + u);
            return ClosestPoint {
                t: cp.t,
                point,
                distance: point.distance(p),
            };
        }
        match &self.sloped {
            Some(sloped) => self.closest_point_sloped(sloped, p, seed_t).0,
            // Unreachable: every non-level guide has its plan view.
            None => self.spline.closest_point(p, seed_t),
        }
    }

    /// The wall point above curve point `c` that is closest to `p` (the
    /// height on the wall is clamped to `[0, height]`), and its squared
    /// distance to `p`.
    fn wall_point(&self, c: Vec3, p: Vec3) -> (Vec3, f32) {
        let u = (p.z - c.z).clamp(0.0, self.height);
        let q = Vec3::new(c.x, c.y, c.z + u);
        (q, q.distance_squared(p))
    }

    /// Global closest point of a rising or falling wall, and the closest
    /// point of its plan view to the plan position of `p`.
    ///
    /// The wall is a strip of the vertical cylinder over the plan view, so
    /// the horizontal distance to the plan view is a lower bound of the
    /// distance to the wall. If `p` is level with the wall above the plan
    /// view's closest point (beside the wall, the usual case) that bound is
    /// attained there, and one plan-view query answers the question.
    ///
    /// Otherwise (`p` above or below the wall's edge) the nearest point is
    /// usually a few millimetres along the curve: a stationary point of the
    /// distance found from the plan view's parameter is the global nearest
    /// point if it is closer than the wall's reach ([`Self::stationary_point`]).
    /// Only beyond the reach is the curve searched:
    /// the distance from `p` to the wall's vertical line over `C(t)` is a C¹
    /// function of `t` whose derivative has the sign of `(q(t) − p) · C'(t)`,
    /// `q(t)` being the closest point of that line. Segments whose bounding
    /// sphere cannot hold a closer point than the best so far are skipped;
    /// in the others every sample interval that could hold one (the distance
    /// cannot change faster than the arc length) is searched: a sign change
    /// of the derivative is bracketed and bisected, and an interval without
    /// one is quartered first, so that a minimum is only missed if the
    /// derivative changes sign twice within 1/64 of a segment.
    fn closest_point_sloped(
        &self,
        sloped: &Sloped,
        p: Vec3,
        seed_t: Option<f32>,
    ) -> (ClosestPoint, ClosestPoint) {
        let spline = &self.spline;
        let plan = sloped.flat.closest_point(Vec3::new(p.x, p.y, 0.0), seed_t);
        let c = spline.point(plan.t);
        let (q, d2) = self.wall_point(c, p);
        // (squared distance, parameter, wall point)
        let mut best: Best = (d2, plan.t, q);
        let beside = p.z >= c.z && p.z <= c.z + self.height;
        if !beside {
            // Above or below the edge: the nearest point is usually a few
            // millimetres along the curve from the plan view's. A wall point
            // closer than the reach at which the distance is stationary is
            // the nearest one (see `stationary_point`), so one local search
            // settles it; only farther points need the global search.
            let global = match self.stationary_point(p, plan.t) {
                Some((t, q, d2)) => {
                    if d2 < best.0 {
                        best = (d2, t, q);
                    }
                    d2.sqrt() < self.reach
                }
                None => false,
            };
            if !global {
                for (i, (center, radius)) in sloped.bounds.iter().enumerate() {
                    let floor = self.wall_point(*center, p).1.sqrt() - radius;
                    if floor < best.0.sqrt() - 1.0e-7 {
                        self.search_segment(i, p, &mut best);
                    }
                }
            }
        }
        let cp = ClosestPoint {
            t: best.1,
            point: best.2,
            distance: best.0.sqrt(),
        };
        (cp, plan)
    }

    /// A wall point at which the distance to `p` is stationary, found by a
    /// safeguarded Newton iteration on the parameter from `t0`: (parameter,
    /// wall point, squared distance), or `None` if it does not converge.
    ///
    /// The distance from `p` to the wall's vertical line over `C(t)` is a C¹
    /// function `φ(t)` with `φ'(t) ∝ g(t) = (q(t) − p) · C'(t)`, `q(t)` the
    /// line's (height-clamped) closest point. At a root of `g` (or at an end
    /// of an open curve where `g` points into it) `p − q` is in the normal
    /// cone of the wall at `q`: perpendicular to the cylinder's tangent
    /// plane inside the wall, and additionally pointing away from the wall
    /// across an edge or end (first-order optimality). By Federer 1959,
    /// Thm 4.8 (12), a point `q + v` with `v` normal at `q` and `|v|` below
    /// the reach has `q` as its unique nearest point, so if the distance
    /// found is below `self.reach` it is the global minimum, whichever
    /// stationary point (even a local maximum) the iteration lands on.
    fn stationary_point(&self, p: Vec3, t0: f32) -> Option<(f32, Vec3, f32)> {
        let spline = &self.spline;
        let max_t = spline.max_t();
        let closed = spline.is_closed();
        // (parameter in range, wall point, squared distance, g, φ''/2).
        let eval = |t: f32| {
            let t = if closed {
                t.rem_euclid(max_t)
            } else {
                t.clamp(0.0, max_t)
            };
            let (i, s) = spline.locate(t);
            let (c, d1, d2) = spline.eval(i, s);
            let rise = p.z - c.z;
            let u = rise.clamp(0.0, self.height);
            let q = Vec3::new(c.x, c.y, c.z + u);
            let e = q - p;
            // Off the edges the height follows `p` and only the horizontal
            // part of the curve moves the line's closest point.
            let speed2 = if rise > 0.0 && rise < self.height {
                d1.x * d1.x + d1.y * d1.y
            } else {
                d1.length_squared()
            };
            (t, q, e.length_squared(), e.dot(d1), speed2 + e.dot(d2))
        };
        let mut t = t0;
        let mut at = eval(t);
        // Parameters seen with g < 0 and with g > 0: once both exist they
        // bracket a root, and every later iterate stays between them.
        let (mut below, mut above): (Option<f32>, Option<f32>) = (None, None);
        for _ in 0..32 {
            let (_, q, d2, g, curvature) = at;
            let done = Some((at.0, q, d2));
            if g == 0.0 {
                return done;
            }
            if !closed && ((t <= 0.0 && g > 0.0) || (t >= max_t && g < 0.0)) {
                return done; // minimum at an end of the wall
            }
            if g < 0.0 {
                below = Some(t);
            } else {
                above = Some(t);
            }
            let mut next = if curvature > 0.0 {
                t - (g / curvature).clamp(-0.5, 0.5)
            } else if g > 0.0 {
                t - 0.25
            } else {
                t + 0.25
            };
            if let (Some(a), Some(b)) = (below, above) {
                let (lo, hi) = (a.min(b), a.max(b));
                if !(next > lo && next < hi) {
                    next = 0.5 * (lo + hi);
                    if !(next > lo && next < hi) {
                        return done; // bracket at the parameter's resolution
                    }
                }
            }
            if !closed {
                next = next.clamp(0.0, max_t);
            }
            let small = (next - t).abs() <= 4.0 * f32::EPSILON * t.abs().max(1.0);
            if next == t {
                return done;
            }
            t = next;
            at = eval(t);
            if small {
                return Some((at.0, at.1, at.2));
            }
        }
        None
    }

    /// Searches segment `i` of the curve for a wall point closer to `p`
    /// than `best` (see [`closest_point_sloped`](Self::closest_point_sloped)).
    fn search_segment(&self, i: usize, p: Vec3, best: &mut Best) {
        let spline = &self.spline;
        let samples = spline.samples();
        let k = SAMPLES_PER_SEGMENT;
        let step = 1.0 / k as f32;
        // Wall point, squared distance and sign of the derivative at `t`.
        let at = |t: f32| {
            let (i, s) = spline.locate(t);
            let (c, d1, _) = spline.eval(i, s);
            let (q, d2) = self.wall_point(c, p);
            (q, d2, (q - p).dot(d1))
        };
        let consider = |best: &mut Best, t: f32, q: Vec3, d2: f32| {
            if d2 < best.0 {
                *best = (d2, t, q);
            }
        };
        // Root of the derivative in a bracket (`g(lo) < 0 < g(hi)`).
        let polish = |best: &mut Best, mut lo: f32, mut hi: f32| {
            for _ in 0..30 {
                let mid = 0.5 * (lo + hi);
                if mid <= lo || mid >= hi {
                    break;
                }
                let (q, d2, g) = at(mid);
                consider(best, mid, q, d2);
                if g < 0.0 {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
        };
        // Arc length of one sample interval of this segment, at most: the
        // speed is within h² / 8 times the third derivative of its linear
        // interpolant, and the third derivative is constant on a segment.
        let third = (spline.eval(i, 1.0).2 - spline.eval(i, 0.0).2).length();
        let slack = step * step * 0.125 * third;
        let first = &samples[i * k];
        let (q, mut d2_0) = self.wall_point(first.point, p);
        let mut g0 = (q - p).dot(first.tangent);
        let mut speed0 = first.tangent.length();
        consider(best, i as f32, q, d2_0);
        for j in 1..=k {
            let sample = &samples[i * k + j];
            let t1 = i as f32 + j as f32 * step;
            let (q1, d2_1) = self.wall_point(sample.point, p);
            let g1 = (q1 - p).dot(sample.tangent);
            let speed1 = sample.tangent.length();
            consider(best, t1, q1, d2_1);
            let arc = step * (speed0.max(speed1) + slack);
            // Smallest distance the interval can hold.
            let floor = 0.5 * (d2_0.sqrt() + d2_1.sqrt() - arc);
            if floor < best.0.sqrt() - 1.0e-7 {
                let t0 = t1 - step;
                if g0 < 0.0 && g1 > 0.0 {
                    polish(best, t0, t1);
                } else {
                    // Quarter the interval and bracket what shows up.
                    let mut left = (t0, g0);
                    for quarter in 1..=4 {
                        let (t, g) = if quarter == 4 {
                            (t1, g1)
                        } else {
                            let t = t0 + 0.25 * step * quarter as f32;
                            let (q, d2, g) = at(t);
                            consider(best, t, q, d2);
                            (t, g)
                        };
                        if left.1 < 0.0 && g > 0.0 {
                            polish(best, left.0, t);
                        }
                        left = (t, g);
                    }
                }
            }
            (d2_0, g0, speed0) = (d2_1, g1, speed1);
        }
    }

    /// Unit normal from the wall point `cp` toward `p` (falls back to the
    /// horizontal curve normal when `p` lies on the wall).
    fn normal_toward(&self, cp: &ClosestPoint, p: Vec3) -> Vec3 {
        if cp.distance > 1.0e-9 {
            return (p - cp.point) / cp.distance;
        }
        let tangent = self.spline.tangent(cp.t);
        let n = Vec3::new(-tangent.y, tangent.x, 0.0);
        let len = n.length();
        if len > 0.0 { n / len } else { Vec3::X }
    }

    /// Continuous collision detection against the wall.
    ///
    /// * `None`: the sphere does not come into contact within `max_time`.
    /// * `Some(GuideCast::Contact(hit))`: at `hit.time` the gap has closed to
    ///   at most 10 µm **and** the sphere is moving into the wall (closing
    ///   speed above 1e-5 of its speed), or it rests at the sliding limit
    ///   described below. A sphere that is not overlapping the wall and is
    ///   moving along or away from it is never reported as a contact.
    /// * `Some(GuideCast::Advance { time })`: no contact up to `time`, but the
    ///   step budget ran out; move there without an impulse and cast again.
    ///
    /// Every step is a proven lower bound of the time to contact, so the
    /// sphere can never pass through the wall, whatever its speed. Two bounds
    /// are used and the larger step taken: the gap cannot shrink faster than
    /// the speed (conservative advancement), and, within the wall's reach
    /// `ρ`, the gap's second time derivative is at least `-|v|² / (ρ - d)`,
    /// which lets a sphere running alongside the wall advance millimetres
    /// instead of micrometres per step. A level wall has a small reach when
    /// its two open ends face each other across a narrow opening (a C);
    /// there the "rolling ball" bound is used as well,
    /// with the radius of the ball that touches the wall at the nearest
    /// point without containing any of it: the radius of the C along the
    /// wall, and down to half the opening only for a sphere that wraps
    /// around an end. For a rising or falling curve the
    /// second bound is the exact "rolling ball" form of it: the wall stays
    /// outside the ball of radius `ρ` touching it at the nearest point, both
    /// beside the wall and above or below its edges (riding or skimming
    /// them); and a third bound is taken on the horizontal gap to the
    /// vertical cylinder over the plan view, which the wall gap can never
    /// undercut and whose reach is the plan view's.
    ///
    /// A sphere that is within 10 µm of the wall and not moving into it is
    /// sliding: it may sink to a gap of -10 µm, a fixed limit that does not
    /// depend on where it started, so repeated casts cannot ratchet deeper.
    /// If the wall curves into its path the contact is reported as soon as
    /// it closes in. At the limit (or below it and still sinking at any
    /// speed) the cast reports a `Contact`, possibly at time 0 and with a
    /// closing speed near zero: that is a resting contact, and the caller is
    /// expected to push the sphere out with
    /// [`closest_point`](Self::closest_point) every sub-step. A caller that
    /// only zeroes the normal velocity stays within the limit but makes less
    /// and less progress along a concave wall. A sphere that is clearly
    /// leaving (separating faster than 1e-5 of its speed) is always free.
    ///
    /// Level guides without reach (curves that touch or cross themselves)
    /// only have the first bound: next to such a wall a sliding sphere gets
    /// a contact about every 10 µm of travel, so keep them out of rolling
    /// paths or split them into simple pieces. (Rising or falling guides
    /// always have reach: [`GuideCurve::new`] rejects the others.) Never
    /// allocates.
    #[must_use]
    pub fn cast_sphere(
        &self,
        center: Vec3,
        radius: f32,
        velocity: Vec3,
        max_time: f32,
        seed_t: Option<f32>,
    ) -> Option<GuideCast> {
        self.cast_sphere_counted(center, radius, velocity, max_time, seed_t)
            .0
    }

    /// [`cast_sphere`](Self::cast_sphere) plus the number of closest-point
    /// evaluations it used (at most 32), for cost tests.
    #[doc(hidden)]
    #[must_use]
    pub fn cast_sphere_counted(
        &self,
        center: Vec3,
        radius: f32,
        velocity: Vec3,
        max_time: f32,
        seed_t: Option<f32>,
    ) -> (Option<GuideCast>, u32) {
        let speed = velocity.length();
        if !(center.is_finite() && speed.is_finite() && speed > 0.0 && max_time >= 0.0) {
            return (None, 0);
        }
        let approach = -APPROACH_EPS * speed;
        let mut time = 0.0f32;
        let mut seed = seed_t;
        let mut flat_seed = seed_t;
        let plan_speed = Vec3::new(velocity.x, velocity.y, 0.0).length();
        for step in 0..GUIDE_CAST_STEPS {
            let p = center + velocity * time;
            let (cp, plan) = match &self.sloped {
                None => (self.closest_point(p, seed), None),
                Some(sloped) => {
                    // After the first step the nearest point is a short way
                    // along the curve from the last one: a stationary point
                    // found from there that is closer than the reach is the
                    // global nearest point (see `stationary_point`), and the
                    // plan-view query (needed only for bound 3, which then
                    // adds little) is skipped.
                    let quick = seed
                        .filter(|_| step > 0)
                        .and_then(|t0| self.stationary_point(p, t0))
                        .filter(|&(_, _, d2)| d2.sqrt() < self.reach);
                    if let Some((t, point, d2)) = quick {
                        let cp = ClosestPoint {
                            t,
                            point,
                            distance: d2.sqrt(),
                        };
                        (cp, None)
                    } else {
                        let (cp, plan) = self.closest_point_sloped(sloped, p, flat_seed);
                        flat_seed = Some(plan.t);
                        (cp, Some(plan))
                    }
                }
            };
            seed = Some(cp.t);
            let gap = cp.distance - radius;
            let normal = self.normal_toward(&cp, p);
            let rate = normal.dot(velocity);
            let contact = || {
                let hit = GuideHit {
                    time,
                    t: cp.t,
                    point: cp.point,
                    normal,
                };
                (Some(GuideCast::Contact(hit)), step + 1)
            };
            // The lowest gap the next step may reach.
            let floor = if gap > GUIDE_SKIN {
                0.5 * GUIDE_SKIN
            } else if rate < approach || (gap < -GUIDE_SKIN && rate < 0.0) {
                // Touching and closing in, or sunk past the sliding limit and
                // still sinking.
                return contact();
            } else if rate > -approach {
                // Clearly leaving: free to go, wherever it starts from.
                gap - GUIDE_SKIN
            } else {
                // Sliding: may sink to the fixed limit of -skin, no further.
                gap.min(-GUIDE_SKIN)
            };
            let drop = gap - floor; // >= 0
            // Bound 1: |d(gap)/dt| <= speed.
            let mut dt = drop / speed;
            match &self.sloped {
                Some(sloped) => {
                    // Bound 2: within the wall's reach the wall stays outside
                    // the ball of radius `reach` that touches it at the
                    // nearest point on the sphere's side (see
                    // `rolling_ball_time`), which lets a sphere running
                    // alongside the wall, or riding or skimming an edge,
                    // advance millimetres instead of micrometres per step.
                    if cp.distance < self.reach {
                        dt = dt.max(rolling_ball_time(
                            cp.distance,
                            self.reach,
                            rate,
                            drop,
                            speed,
                        ));
                    }
                    // Bound 3: the wall is a strip of the vertical cylinder
                    // over its plan view, so its gap is at least the
                    // horizontal gap to that cylinder, whose reach is the
                    // plan view's (usually larger than the wall's), and
                    // which only the horizontal motion changes. Beside the
                    // wall the two gaps are equal.
                    if let Some(fc) = plan
                        && fc.distance - radius - floor > 0.0
                        && fc.distance > 1.0e-9
                    {
                        let flat_drop = fc.distance - radius - floor;
                        if plan_speed <= 0.0 {
                            // Moving straight up or down: the horizontal gap
                            // does not change at all.
                            return (None, step + 1);
                        }
                        if fc.distance < sloped.plan_reach {
                            let q = Vec3::new(p.x, p.y, 0.0);
                            let flat_rate = (q - fc.point).dot(velocity) / fc.distance;
                            dt = dt.max(rolling_ball_time(
                                fc.distance,
                                sloped.plan_reach,
                                flat_rate,
                                flat_drop,
                                plan_speed,
                            ));
                        }
                    }
                }
                None => {
                    // Level curve: gap(τ) >= gap + rate τ - ½ K τ², valid
                    // while the distance stays below `cap` < reach.
                    let distance = cp.distance;
                    // Where the open ends limit the reach (they face each
                    // other across a narrow opening) it says little about
                    // the rest of the wall: there the wall also stays
                    // outside the ball that touches it at the nearest point,
                    // on the sphere's side, with the radius that holds at
                    // that point (see `TangentBalls`, `rolling_ball_time`).
                    if let Some(balls) = &self.tangent_balls {
                        let rho = balls.radius(&self.spline, cp.t, cp.point, normal);
                        if distance < rho {
                            dt = dt.max(rolling_ball_time(distance, rho, rate, drop, speed));
                        }
                    }
                    if distance < 0.9 * self.reach {
                        let cap = 0.5 * (distance + self.reach);
                        let k = speed * speed / (self.reach - cap);
                        // Positive root of drop + rate τ - ½ K τ² = 0.
                        // (Written so that neither sign of `rate` cancels.)
                        let disc = (rate * rate + 2.0 * k * drop).sqrt();
                        let root = if rate < 0.0 {
                            2.0 * drop / (disc - rate)
                        } else {
                            (rate + disc) / k
                        };
                        let within = (cap - distance) / speed;
                        dt = dt.max(root.min(within));
                    }
                }
            }
            let next = time + dt;
            if next >= max_time || next.is_nan() {
                return (None, step + 1);
            }
            if next == time {
                // No progress possible. In contact that is a resting contact
                // at the sliding limit; otherwise the time resolution is
                // exhausted and the caller continues from here.
                if gap <= GUIDE_SKIN {
                    return contact();
                }
                return (Some(GuideCast::Advance { time }), step + 1);
            }
            time = next;
        }
        (Some(GuideCast::Advance { time }), GUIDE_CAST_STEPS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arc(radius: f64, from: f64, to: f64, spacing: f64) -> Vec<Vec3> {
        let n = (((to - from).abs() * radius / spacing).round() as usize).max(2);
        (0..=n)
            .map(|i| {
                let a = from + (to - from) * i as f64 / n as f64;
                Vec3::new((radius * a.cos()) as f32, (radius * a.sin()) as f32, 0.0)
            })
            .collect()
    }

    /// A ring of radius `r` with an opening of chord `opening` on +X.
    fn c_ring(r: f64, opening: f64, spacing: f64) -> Vec<Vec3> {
        let half = (opening / (2.0 * r)).asin();
        arc(r, half, core::f64::consts::TAU - half, spacing)
    }

    /// A ring whose second end turns inward on a small arc: that end points
    /// across the ring, with the first end behind and beside it.
    fn hook(r: f64, opening: f64, spacing: f64) -> Vec<Vec3> {
        let mut pts = c_ring(r, opening, spacing);
        let end = pts[pts.len() - 1].as_dvec3();
        let before = pts[pts.len() - 2].as_dvec3();
        let dir = (end - before).normalize();
        let left = DVec3::new(-dir.y, dir.x, 0.0);
        let bend = 3.0 * spacing;
        let centre = end + left * bend;
        let quarter = (core::f64::consts::FRAC_PI_2 * bend / spacing).round() as usize;
        for i in 1..=quarter {
            let a = core::f64::consts::FRAC_PI_2 * i as f64 / quarter as f64;
            let p = centre - left * bend * a.cos() + dir * bend * a.sin();
            pts.push(p.as_vec3());
        }
        let tip = pts[pts.len() - 1].as_dvec3();
        for i in 1..=3 {
            pts.push((tip + left * spacing * i as f64).as_vec3());
        }
        pts
    }

    fn guide(points: &[Vec3]) -> GuideCurve {
        GuideCurve::new(CubicSpline::interpolate(points).unwrap(), 0.05).unwrap()
    }

    /// Every ball `TangentBalls::radius` promises must miss the curve.
    fn check_tangent_balls(g: &GuideCurve, label: &str) -> f32 {
        let balls = g.tangent_balls.as_ref().expect(label);
        let spline = &g.spline;
        let max_t = spline.max_t();
        let dense: Vec<DVec3> = (0..=spline.segment_count() * 40)
            .map(|i| spline.point(i as f32 / 40.0).as_dvec3())
            .collect();
        let check = |t: f32, n: Vec3| -> f32 {
            let rho = balls.radius(spline, t, spline.point(t), n);
            assert!(rho > 0.0 && rho.is_finite(), "{label}: radius {rho} at {t}");
            let a = spline.point(t).as_dvec3();
            let centre = a + n.as_dvec3() * f64::from(rho);
            for y in &dense {
                let d = y.distance(centre);
                assert!(
                    d >= f64::from(rho) * (1.0 - 1.0e-4),
                    "{label}: the ball of radius {rho} at t = {t}, normal {n:?}, holds a curve \
                     point ({d} from its centre)"
                );
            }
            rho
        };
        let steps = spline.segment_count() * 8;
        let mut smallest_inside = f32::INFINITY;
        for i in 0..=steps {
            let t = max_t * i as f32 / steps as f32;
            let tangent = spline.tangent(t).normalize();
            let side = Vec3::new(-tangent.y, tangent.x, 0.0);
            for sign in [1.0f32, -1.0] {
                let rho = check(t, side * sign);
                // Leaning up or down (above or below the wall's edge).
                check(t, (side * sign + Vec3::Z * 0.7).normalize());
                if t > 1.0 && t < max_t - 1.0 {
                    smallest_inside = smallest_inside.min(rho);
                }
            }
        }
        // Around the ends.
        for (t, inward) in [
            (0.0, spline.tangent(0.0).normalize()),
            (max_t, -spline.tangent(max_t).normalize()),
        ] {
            let side = Vec3::new(-inward.y, inward.x, 0.0);
            let mut last = f32::INFINITY;
            for k in 0..=16 {
                let phi = core::f32::consts::FRAC_PI_2 * k as f32 / 16.0;
                for sign in [1.0f32, -1.0] {
                    let n = side * sign * libm::cosf(phi) - inward * libm::sinf(phi);
                    let rho = check(t, n);
                    check(t, (n + Vec3::Z * 0.5).normalize());
                    if sign > 0.0 {
                        // The further behind the end, the smaller.
                        assert!(rho <= last * (1.0 + 1.0e-5), "{label}");
                        last = rho;
                    }
                }
            }
        }
        smallest_inside
    }

    #[test]
    fn tangent_balls_of_c_rings_miss_the_wall() {
        for (r, opening, spacing) in [
            (0.04, 0.015, 0.01),
            (0.04, 0.03, 0.01),
            (0.06, 0.022, 0.01),
            (0.1, 0.029, 0.01),
            (0.1, 0.06, 0.01),
            (0.1, 0.12, 0.01),
            (0.5, 0.03, 0.05),
            (0.5, 0.16, 0.05),
            // Openings far narrower than the through-point spacing.
            (0.3, 0.02, 0.05),
            (0.5, 0.015, 0.05),
            (0.5, 0.02, 0.05),
            (0.5, 0.024, 0.05),
            (0.1, 0.009, 0.02),
            (0.5, 0.005, 0.05),
            (0.5, 0.003, 0.05),
            (0.1, 0.005, 0.01),
            (0.5, 0.001, 0.05),
            (0.5, 0.0003, 0.05),
            (0.5, 0.00005, 0.05),
            (0.1, 0.0002, 0.01),
        ] {
            let g = guide(&c_ring(r, opening, spacing));
            let label = format!("C ring r {r} opening {opening}");
            // The reach is half the opening at most; along the wall the
            // balls keep (most of) the ring's radius.
            assert!(f64::from(g.reach) <= 0.5 * opening, "{label}");
            let inside = check_tangent_balls(&g, &label);
            assert!(
                inside > 2.0 * g.reach.min(0.1) || f64::from(inside) > 0.5 * r,
                "{label}: {inside}"
            );
            // Straight behind an end: half the opening at most (less the
            // margin, and less what the two terms of the bound give away).
            let balls = g.tangent_balls.as_ref().unwrap();
            let behind = balls.radius(
                &g.spline,
                0.0,
                g.spline.point(0.0),
                -g.spline.tangent(0.0).normalize(),
            );
            assert!(
                f64::from(behind) <= 0.5 * opening && f64::from(behind) > 0.25 * opening,
                "{label}: {behind}"
            );
        }
    }

    #[test]
    fn tangent_balls_of_hooks_miss_the_wall() {
        let mut checked = 0;
        for (r, opening, spacing) in [
            (0.06, 0.03, 0.005),
            (0.1, 0.05, 0.008),
            (0.08, 0.06, 0.006),
            (0.12, 0.04, 0.008),
        ] {
            let g = guide(&hook(r, opening, spacing));
            // (A hook whose reach is set by its bend has no table.)
            if g.tangent_balls.is_some() {
                check_tangent_balls(&g, &format!("hook r {r} opening {opening}"));
                checked += 1;
            }
        }
        assert!(checked >= 2, "{checked}");
    }

    #[test]
    fn guides_without_facing_ends_keep_the_plain_reach_bound() {
        // Nothing lies behind the ends of a straight wall or of an arc of up
        // to a half turn, and a closed ring has no ends: no table.
        let straight: Vec<Vec3> = (0..=20).map(|i| Vec3::X * 0.05 * i as f32).collect();
        assert!(guide(&straight).tangent_balls.is_none());
        for (radius, turn) in [(0.15, 0.5), (0.5, 0.25), (0.03, 0.4)] {
            let g = guide(&arc(
                radius,
                0.0,
                core::f64::consts::TAU * turn,
                radius / 12.0,
            ));
            assert!(g.tangent_balls.is_none(), "arc r {radius} turn {turn}");
            assert!(f64::from(g.reach) > 0.6 * radius && f64::from(g.reach) <= radius);
        }
        let mut ring = arc(0.1, 0.0, core::f64::consts::TAU * 35.0 / 36.0, 0.1 * 0.1745);
        ring.push(ring[0]);
        let closed = GuideCurve::new(CubicSpline::interpolate_closed(&ring).unwrap(), 0.05);
        assert!(closed.unwrap().tangent_balls.is_none());
        // A crossing has no reach and no table either.
        let eight: Vec<Vec3> = (0..=40)
            .map(|i| {
                let a = core::f32::consts::TAU * i as f32 / 40.0 + 0.3;
                Vec3::new(0.1 * libm::sinf(a), 0.05 * libm::sinf(2.0 * a), 0.0)
            })
            .collect();
        let g = guide(&eight);
        assert!(g.reach < 1.0e-4, "{}", g.reach);
    }

    /// Curves of many kinds for the reach comparison.
    fn zoo() -> Vec<CubicSpline> {
        let mut curves = Vec::new();
        let open = |pts: &[Vec3]| CubicSpline::interpolate(pts).unwrap();
        curves.push(open(
            &(0..=20)
                .map(|i| Vec3::X * 0.05 * i as f32)
                .collect::<Vec<_>>(),
        ));
        for (r, opening, spacing) in [
            (0.04, 0.015, 0.01),
            (0.04, 0.03, 0.01),
            (0.1, 0.029, 0.01),
            (0.5, 0.16, 0.05),
            (0.5, 0.02, 0.05),
            (0.5, 0.0003, 0.05),
            (0.5, 0.002, 0.0104),
        ] {
            curves.push(open(&c_ring(r, opening, spacing)));
            curves.push(open(&hook(r, opening.max(0.02), spacing.min(0.01))));
        }
        for (radius, turn) in [(0.15, 0.5), (0.5, 0.25), (0.03, 0.4), (0.2, 0.9)] {
            curves.push(open(&arc(
                radius,
                0.0,
                core::f64::consts::TAU * turn,
                radius / 12.0,
            )));
        }
        // Closed rings, a figure of eight, wiggles, a kinked line (cusp-like).
        for n in [12usize, 36, 200] {
            let mut ring: Vec<Vec3> = (0..n)
                .map(|i| {
                    let a = core::f32::consts::TAU * i as f32 / n as f32;
                    Vec3::new(0.2 * libm::cosf(a), 0.15 * libm::sinf(a), 0.0)
                })
                .collect();
            ring.push(ring[0]);
            curves.push(CubicSpline::interpolate_closed(&ring).unwrap());
        }
        for (phase, amp) in [(0.3f32, 0.05f32), (0.0, 0.1), (1.0, 0.02)] {
            let pts: Vec<Vec3> = (0..=40)
                .map(|i| {
                    let a = core::f32::consts::TAU * i as f32 / 40.0 + phase;
                    Vec3::new(0.1 * libm::sinf(a), amp * libm::sinf(2.0 * a), 0.0)
                })
                .collect();
            curves.push(open(&pts));
            let pts: Vec<Vec3> = (0..=60)
                .map(|i| {
                    let u = i as f32 * 0.02;
                    Vec3::new(u, amp * libm::sinf(9.0 * u + phase), 0.0)
                })
                .collect();
            curves.push(open(&pts));
        }
        // Horseshoes with legs of unequal length.
        for extra in [0usize, 3, 9] {
            let mut pts: Vec<Vec3> = (1..=10)
                .rev()
                .map(|i| Vec3::new(0.05, -0.01 * i as f32, 0.0))
                .collect();
            pts.extend(arc(0.05, 0.0, core::f64::consts::PI, 0.01));
            pts.extend((1..=10 + extra).map(|i| Vec3::new(-0.05, -0.01 * i as f32, 0.0)));
            curves.push(open(&pts));
        }
        curves
    }

    #[test]
    fn level_reaches_are_bit_for_bit_the_pairwise_estimates() {
        let curves = zoo();
        assert!(curves.len() > 30);
        let mut limited = 0;
        for (i, spline) in curves.iter().enumerate() {
            let (half, line) = level_reaches(spline);
            assert_eq!(
                half.to_bits(),
                curve_reach(spline, true).to_bits(),
                "curve {i}"
            );
            assert_eq!(
                line.to_bits(),
                curve_reach(spline, false).to_bits(),
                "curve {i}"
            );
            limited += usize::from(half < line);
        }
        assert!(limited >= 8 && limited < curves.len(), "{limited}");
    }

    #[test]
    fn back_radius_sees_a_wall_between_two_samples() {
        // An end pointing at a straight stretch of the same curve, a
        // fraction of a sample interval away and between two samples: the
        // sampled value would be far too large.
        for gap in [2.0e-3f32, 3.0e-4, 2.0e-5] {
            // A long wall along X, a wide half turn, and a stub coming back
            // down onto the wall at x = 0.2137 (not a sample).
            let mut pts: Vec<Vec3> = (0..=8).map(|i| Vec3::X * 0.05 * i as f32).collect();
            pts.extend(
                arc(
                    0.1,
                    -core::f64::consts::FRAC_PI_2,
                    core::f64::consts::FRAC_PI_2,
                    0.05,
                )
                .iter()
                .skip(1)
                .map(|p| *p + Vec3::new(0.4, 0.1, 0.0)),
            );
            for i in 1..=3 {
                pts.push(Vec3::new(0.4 - 0.05 * i as f32, 0.2, 0.0));
            }
            // Turn down toward the wall.
            pts.push(Vec3::new(0.2137, 0.16, 0.0));
            pts.push(Vec3::new(0.2137, 0.11, 0.0));
            pts.push(Vec3::new(0.2137, 0.06, 0.0));
            pts.push(Vec3::new(0.2137, gap, 0.0));
            let spline = CubicSpline::interpolate(&pts).unwrap();
            let t = spline.max_t();
            let e = spline.point(t).as_dvec3();
            let inward = -spline.tangent(t).as_dvec3().normalize();
            let back = back_radius(&spline, e, inward);
            // Brute force over a very dense sampling of the wall below.
            let mut truth = f64::INFINITY;
            for i in 0..=400_000 {
                let w = spline.point(8.0 * i as f32 / 400_000.0).as_dvec3() - e;
                let behind = -w.dot(inward);
                if behind > 0.0 {
                    truth = truth.min(w.length_squared() / (2.0 * behind));
                }
            }
            assert!(truth < 0.75 * f64::from(gap), "gap {gap}: {truth}");
            assert!(
                back <= truth * 1.001 && back >= truth * 0.9,
                "gap {gap}: found {back}, true {truth}"
            );
        }
    }

    /// Construction cost (run with `--release -- --ignored --nocapture`):
    /// `GuideCurve::new` against the pairwise estimate that was the whole
    /// cost of a level guide before the open ends were accounted for.
    #[test]
    #[ignore = "cost report; prints, asserts nothing about time"]
    fn level_guide_construction_cost_report() {
        use std::time::Instant;
        let ring = |r: f64, opening: f64, segments: usize| {
            c_ring(r, opening, core::f64::consts::TAU * r / segments as f64)
        };
        // Uneven spacing: every other point pulled toward its neighbour.
        let uneven = |r: f64, opening: f64, segments: usize| {
            let half = (opening / (2.0 * r)).asin();
            let sweep = core::f64::consts::TAU - 2.0 * half;
            (0..=segments)
                .map(|i| {
                    let shift = if i % 2 == 1 && i < segments { 0.3 } else { 0.0 };
                    let a = half + sweep * (i as f64 + shift) / segments as f64;
                    Vec3::new((r * a.cos()) as f32, (r * a.sin()) as f32, 0.0)
                })
                .collect::<Vec<_>>()
        };
        let closed = |r: f32, n: usize| {
            let mut pts: Vec<Vec3> = (0..n)
                .map(|i| {
                    let a = core::f32::consts::TAU * i as f32 / n as f32;
                    Vec3::new(r * libm::cosf(a), r * libm::sinf(a), 0.0)
                })
                .collect();
            pts.push(pts[0]);
            CubicSpline::interpolate_closed(&pts).unwrap()
        };
        let open = |pts: &[Vec3]| CubicSpline::interpolate(pts).unwrap();
        let cases: Vec<(String, CubicSpline)> = vec![
            (
                "r500 20 mm gap, 50 mm spacing".into(),
                open(&c_ring(0.5, 0.02, 0.05)),
            ),
            (
                "r500 5 mm gap, 50 mm spacing".into(),
                open(&c_ring(0.5, 0.005, 0.05)),
            ),
            (
                "r500 2 mm gap, 50 mm spacing".into(),
                open(&c_ring(0.5, 0.002, 0.05)),
            ),
            (
                "r40 30 mm gap, 10 mm spacing".into(),
                open(&c_ring(0.04, 0.03, 0.01)),
            ),
            ("300 seg r500 30 mm gap".into(), open(&ring(0.5, 0.03, 300))),
            ("300 seg r500 2 mm gap".into(), open(&ring(0.5, 0.002, 300))),
            (
                "300 seg r500 0.6 mm gap".into(),
                open(&ring(0.5, 0.0006, 300)),
            ),
            (
                "300 seg r1000 4 mm gap".into(),
                open(&ring(1.0, 0.004, 300)),
            ),
            (
                "600 seg r1000 2 mm gap".into(),
                open(&ring(1.0, 0.002, 600)),
            ),
            ("300 seg hook".into(), open(&hook(0.5, 0.03, 0.0104))),
            (
                "2000 seg r1000 0.2 mm gap".into(),
                open(&ring(1.0, 0.0002, 2000)),
            ),
            (
                "2000 seg r1000 0.2 mm gap, uneven".into(),
                open(&uneven(1.0, 0.0002, 2000)),
            ),
            (
                "2000 seg r1000 30 mm gap".into(),
                open(&ring(1.0, 0.03, 2000)),
            ),
            (
                "2000 seg r100 0.05 mm gap".into(),
                open(&ring(0.1, 0.00005, 2000)),
            ),
            ("2000 seg closed ring r1000".into(), closed(1.0, 2000)),
            (
                "2000 seg straight wall".into(),
                open(
                    &(0..=2000)
                        .map(|i| Vec3::X * 0.01 * i as f32)
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "2000 seg S-curve".into(),
                open(
                    &(0..=2000)
                        .map(|i| {
                            let u = i as f32 * 0.005;
                            Vec3::new(u, 0.3 * libm::sinf(2.0 * u), 0.0)
                        })
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "2000 seg 350° arc r100".into(),
                open(&arc(
                    0.1,
                    0.0,
                    core::f64::consts::TAU * 350.0 / 360.0,
                    0.000_305_4,
                )),
            ),
            (
                "2000 seg spiral, turns 30 mm apart".into(),
                open(
                    &(0..=2000)
                        .map(|i| {
                            let a = 0.02 * i as f32;
                            let r = 0.1 + 0.03 * a / core::f32::consts::TAU;
                            Vec3::new(r * libm::cosf(a), r * libm::sinf(a), 0.0)
                        })
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "300 seg half turn r500 (no ends facing)".into(),
                open(&arc(0.5, 0.0, core::f64::consts::PI, 0.00524)),
            ),
        ];
        println!(
            "| guide | segments | reach mm | end-limited | pairwise estimate (329c843) ms | GuideCurve::new ms |"
        );
        for (name, spline) in cases {
            let rounds = if spline.segment_count() > 1000 { 3 } else { 10 };
            let mut old = f64::INFINITY;
            let mut new = f64::INFINITY;
            let mut built = None;
            for _ in 0..rounds {
                let begin = Instant::now();
                std::hint::black_box(curve_reach(std::hint::black_box(&spline), false));
                old = old.min(begin.elapsed().as_secs_f64() * 1.0e3);
                let copy = spline.clone();
                let begin = Instant::now();
                let g = GuideCurve::new(std::hint::black_box(copy), 0.05).unwrap();
                new = new.min(begin.elapsed().as_secs_f64() * 1.0e3);
                built = Some(g);
            }
            let g = built.unwrap();
            println!(
                "| {name} | {} | {:.3} | {} | {old:.2} | {new:.2} |",
                g.spline.segment_count(),
                g.reach * 1.0e3,
                g.tangent_balls.is_some()
            );
        }
    }
}
