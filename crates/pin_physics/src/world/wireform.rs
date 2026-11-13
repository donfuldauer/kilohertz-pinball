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
/// still slides along the rest of the wall at full speed, unless the opening
/// is narrower than about 1/32 of the through-point spacing.
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
/// The same balls miss the wall `C × [0, h]`: for a wall point above `a`
/// nearest to `p`, the vertical part of `n` points away from the wall (or
/// is zero), so `w · n ≤ w_h · n_h`, and `|w| ≥ |w_h|`.
#[derive(Debug, Clone)]
struct TangentBalls {
    /// Per table interval (`per_segment` of them in every segment): the
    /// smallest `ρ_line` of its two table points and their two neighbours,
    /// with the 0.9 sampling margin.
    line: Vec<f32>,
    /// Table intervals per segment: 4, or more (up to
    /// [`TANGENT_BALL_FINEST`]) where the reach is shorter than a quarter
    /// of a segment.
    per_segment: usize,
    /// Open curves: (unit tangent into the curve, `ρ_back` with the 0.9
    /// margin) at the first and at the last point.
    ends: Option<[(Vec3, f32); 2]>,
}

/// Table intervals per segment of the tangent-ball table, coarsest and
/// finest.
const TANGENT_BALL_COARSEST: usize = 4;
const TANGENT_BALL_FINEST: usize = 64;

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

impl TangentBalls {
    /// The table for a curve of reach `reach` (no cusp, no crossing).
    ///
    /// The table is sampled, and its 0.9 margin covers what happens between
    /// two table points only if nothing comes closer to the curve than
    /// about two table intervals (a gap `D` between a wall and an end
    /// pointing at it is seen from the nearest point as
    /// `D/2 · (1 + (l/2D)²)`, `l` the interval). So the table gets as many
    /// intervals per segment (4, 8, … 64) as it takes for the longest one
    /// to be no longer than the reach, and the curve points it is compared
    /// with are at least as dense.
    ///
    /// `None` only if even 1/64 of a segment is longer than the reach: the
    /// two ends of the curve face each other across less than about 1/32 of
    /// the through-point spacing (1.6 mm at 50 mm spacing). Such a curve
    /// keeps the plain reach bound: safe, but a sphere wider than the reach
    /// slides slowly along it.
    fn new(spline: &CubicSpline, reach: f32) -> Option<Self> {
        const MAX: f64 = 1.0e3;
        let curvature = curvature_bound(spline)?;
        let samples = spline.samples();
        let segments = spline.segment_count();
        // Curve points (point, unit tangent), `dense` per segment.
        let points = |dense: usize| -> Vec<(DVec3, DVec3)> {
            if dense <= SAMPLES_PER_SEGMENT {
                samples
                    .iter()
                    .step_by(SAMPLES_PER_SEGMENT / dense)
                    .map(|sample| {
                        (
                            sample.point.as_dvec3(),
                            sample.tangent.as_dvec3().normalize(),
                        )
                    })
                    .collect()
            } else {
                (0..=segments * dense)
                    .map(|j| {
                        let i = (j / dense).min(segments - 1);
                        let (c, d1, _) = spline.eval(i, (j - i * dense) as f32 / dense as f32);
                        (c.as_dvec3(), d1.as_dvec3().normalize())
                    })
                    .collect()
            }
        };
        let mut per_segment = TANGENT_BALL_COARSEST;
        let (pts, stride) = loop {
            // The curve points are twice as dense as the coarsest table,
            // and as dense as any finer one.
            let dense = per_segment.max(2 * TANGENT_BALL_COARSEST);
            let stride = dense / per_segment;
            let pts = points(dense);
            let longest = pts
                .chunks(stride)
                .zip(pts[1..].chunks(stride))
                .map(|(from, to)| {
                    from.iter()
                        .zip(to)
                        .map(|(a, b)| a.0.distance(b.0))
                        .sum::<f64>()
                })
                .fold(0.0f64, f64::max);
            if f64::from(reach) >= longest {
                break (pts, stride);
            }
            if per_segment >= TANGENT_BALL_FINEST {
                return None;
            }
            per_segment *= 2;
        };
        let last = pts.len() - 1;
        let count = last / stride + 1;
        if count < 2 {
            return None;
        }
        let cap = if curvature * MAX > 1.0 {
            1.0 / curvature
        } else {
            MAX
        };
        let open = !spline.is_closed();
        let mut at_samples = Vec::with_capacity(count);
        let mut back = [MAX; 2];
        for a in 0..count {
            let index = (a * stride).min(last);
            let (x, tangent) = pts[index];
            let end = if open && index == 0 {
                Some((0, tangent))
            } else if open && index == last {
                Some((1, -tangent))
            } else {
                None
            };
            let mut rho = cap;
            for &(y, _) in &pts {
                let w = y - x;
                let w2 = w.length_squared();
                if w2 < 1.0e-16 {
                    continue; // the point itself, or the seam of a closed curve
                }
                let along = w.dot(tangent);
                let off = (w - tangent * along).length();
                if 2.0 * off * rho > w2 {
                    rho = w2 / (2.0 * off);
                }
                if let Some((which, inward)) = end {
                    let behind = -w.dot(inward);
                    if behind > 0.0 && 2.0 * behind * back[which] > w2 {
                        back[which] = w2 / (2.0 * behind);
                    }
                }
            }
            at_samples.push(rho);
        }
        let intervals = count - 1;
        let line = (0..intervals)
            .map(|i| {
                let mut lowest = f64::INFINITY;
                for offset in 0..4usize {
                    // Samples i − 1 ..= i + 2.
                    let j = if open {
                        (i + offset).saturating_sub(1).min(count - 1)
                    } else {
                        // Sample `intervals` is sample 0 again.
                        (i + offset + intervals - 1) % intervals
                    };
                    lowest = lowest.min(at_samples[j]);
                }
                (0.9 * lowest) as f32
            })
            .collect();
        let ends = open.then(|| {
            [
                (pts[0].1.as_vec3(), (0.9 * back[0]) as f32),
                ((-pts[last].1).as_vec3(), (0.9 * back[1]) as f32),
            ]
        });
        Some(Self {
            line,
            per_segment,
            ends,
        })
    }

    /// Radius of a ball that touches the curve at parameter `t` on the side
    /// of the unit vector `normal` (the direction from the nearest point to
    /// the query point) and contains none of it.
    fn radius(&self, t: f32, max_t: f32, normal: Vec3) -> f32 {
        let per_unit = self.per_segment as f32;
        let index = ((t.max(0.0) * per_unit) as usize).min(self.line.len() - 1);
        let mut rho = self.line[index];
        if let Some(ends) = &self.ends {
            for (end_t, (inward, back)) in [0.0, max_t].into_iter().zip(ends) {
                let s = -normal.dot(*inward);
                if (t - end_t).abs() <= END_BAND && s > 0.0 && rho > 0.0 {
                    rho = 1.0 / (1.0 / rho + s / back);
                }
            }
        }
        rho
    }
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
            let reach = curve_reach(&spline, true);
            if reach > 0.0 && reach < curve_reach(&spline, false) {
                tangent_balls = TangentBalls::new(&spline, reach);
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
                        let rho = balls.radius(cp.t, self.spline.max_t(), normal);
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
            let rho = balls.radius(t, max_t, n);
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
            // Openings narrower than a quarter segment: finer tables.
            (0.3, 0.02, 0.05),
            (0.5, 0.015, 0.05),
            (0.5, 0.02, 0.05),
            (0.5, 0.024, 0.05),
            (0.1, 0.009, 0.02),
            (0.5, 0.005, 0.05),
            (0.5, 0.003, 0.05),
            (0.1, 0.005, 0.01),
        ] {
            let g = guide(&c_ring(r, opening, spacing));
            let label = format!("C ring r {r} opening {opening}");
            // The reach is half the opening at most; along the wall the
            // balls keep (most of) the ring's radius.
            assert!(f64::from(g.reach) <= 0.5 * opening, "{label}");
            let inside = check_tangent_balls(&g, &label);
            assert!(f64::from(inside) > 0.6 * r, "{label}: {inside}");
            // Straight behind an end: half the opening at most (less the
            // margin, and less what the two terms of the bound give away).
            let balls = g.tangent_balls.as_ref().unwrap();
            let behind = balls.radius(0.0, g.spline.max_t(), -g.spline.tangent(0.0).normalize());
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
        assert!(g.reach < 1.0e-4 && g.tangent_balls.is_none(), "{}", g.reach);
        // So does a C whose opening is less than about 1/32 of its point
        // spacing: not even the finest table resolves it.
        let g = guide(&c_ring(0.5, 0.001, 0.05));
        assert!(
            g.reach <= 0.0005 && g.tangent_balls.is_none(),
            "{}",
            g.reach
        );
    }
}
