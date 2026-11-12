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
/// Guides are meant for level curves (constant Z, the usual orbit wall). A
/// curve that rises or falls is supported, but its sphere cast falls back to
/// plain conservative advancement, which needs many more steps when the ball
/// runs alongside the wall.
#[derive(Debug, Clone)]
pub struct GuideCurve {
    spline: CubicSpline,
    height: f32,
    /// Z of the curve if every control point has the same Z.
    level_z: Option<f32>,
    /// Conservative estimate of the wall's reach: within this distance of the
    /// wall the nearest wall point is unique, so the distance function is
    /// smooth with second derivatives bounded below by `-1 / (reach - d)`.
    /// Zero when unknown (non-level or self-touching curves).
    reach: f32,
}

/// Reach of a sampled curve (Federer): the infimum over point pairs of
/// `|y - x|² / (2 · dist(y - x, tangent line at x))`, which accounts for both
/// curvature and distinct stretches of the curve approaching each other.
fn curve_reach(spline: &CubicSpline) -> f32 {
    const MAX_REACH: f64 = 1.0e3;
    let samples = spline.samples();
    // Curvature at every sample.
    let k = SAMPLES_PER_SEGMENT;
    let n = spline.segment_count();
    let mut reach = MAX_REACH;
    for index in 0..samples.len() {
        let i = (index / k).min(n - 1);
        let s = (index - i * k) as f32 / k as f32;
        let (_, d1, d2) = spline.eval(i, s);
        let (d1, d2) = (d1.as_dvec3(), d2.as_dvec3());
        let speed = d1.length();
        if speed <= 1.0e-12 {
            return 0.0; // cusp
        }
        let curvature = d1.cross(d2).length() / (speed * speed * speed);
        if curvature * reach > 1.0 {
            reach = 1.0 / curvature;
        }
    }
    // Pairs, on every fourth sample (plus the last).
    let stride = 4;
    let count = (samples.len() - 1) / stride + 1;
    let at = |j: usize| {
        let sample = &samples[(j * stride).min(samples.len() - 1)];
        (
            sample.point.as_dvec3(),
            sample.tangent.as_dvec3().normalize(),
        )
    };
    for a in 0..count {
        let (x, tangent) = at(a);
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
            let off = (w - tangent * w.dot(tangent)).length();
            if 2.0 * off * reach > w2 {
                reach = w2 / (2.0 * off);
            }
        }
    }
    // Margin for what the sampling cannot see.
    (0.9 * reach) as f32
}

impl GuideCurve {
    /// Wraps `spline` (the bottom edge of the wall) with a wall `height_m`.
    ///
    /// # Errors
    /// Non-finite or negative height.
    pub fn new(spline: CubicSpline, height_m: f32) -> Result<Self, SplineError> {
        if !height_m.is_finite() {
            return Err(SplineError::NonFinite);
        }
        if height_m < 0.0 {
            return Err(SplineError::InvalidHeight);
        }
        let z0 = spline.segments()[0].p[0].z;
        let level = spline
            .segments()
            .iter()
            .all(|seg| seg.p.iter().all(|p| (p.z - z0).abs() <= 1.0e-6));
        let reach = if level { curve_reach(&spline) } else { 0.0 };
        Ok(Self {
            spline,
            height: height_m,
            level_z: level.then_some(z0),
            reach,
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

    /// Closest point of the wall to `p`; `t` is the curve parameter below the
    /// returned point. Never allocates.
    ///
    /// Exact (global) for level curves. For a rising or falling curve the
    /// height along the wall and the parameter along the curve are optimised
    /// alternately until they stop moving, which finds the closest point
    /// near the curve's own closest point.
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
        let mut cp = self.spline.closest_point(p, seed_t);
        let mut u = (p.z - cp.point.z).clamp(0.0, self.height);
        // The alternation contracts by about slope² per round.
        for _ in 0..24 {
            let q = Vec3::new(p.x, p.y, p.z - u);
            let next = self.spline.closest_point(q, Some(cp.t));
            let moved = (next.t - cp.t).abs();
            cp = next;
            let next_u = (p.z - cp.point.z).clamp(0.0, self.height);
            let u_moved = (next_u - u).abs();
            u = next_u;
            if moved < 1.0e-6 && u_moved < 1.0e-7 {
                break;
            }
        }
        let point = Vec3::new(cp.point.x, cp.point.y, cp.point.z + u);
        ClosestPoint {
            t: cp.t,
            point,
            distance: point.distance(p),
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
    ///   speed above 1e-5 of its speed). A sphere moving along or away from
    ///   the wall is never reported as a contact.
    /// * `Some(GuideCast::Advance { time })`: no contact up to `time`, but the
    ///   step budget ran out; move there without an impulse and cast again.
    ///
    /// Every step is a proven lower bound of the time to contact, so the
    /// sphere can never pass through the wall, whatever its speed. Two bounds
    /// are used and the larger step taken: the gap cannot shrink faster than
    /// the speed (conservative advancement), and, for level curves within the
    /// wall's reach `ρ`, the gap's second time derivative is at least
    /// `-|v|² / (ρ - d)`, which lets a sphere running alongside the wall
    /// advance millimetres instead of micrometres per step.
    ///
    /// A sphere that is within 10 µm of the wall (or overlapping it) and not
    /// moving into it is sliding: it is allowed to sink at most a further
    /// 10 µm (never below -10 µm from a non-overlapping start) while it
    /// slides; if the wall curves into its path the resulting contact is
    /// reported as soon as it closes in. Resolve the remaining overlap with
    /// [`closest_point`](Self::closest_point). Never allocates.
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
        // Lowest gap a run of sliding steps may reach.
        let mut slide_floor: Option<f32> = None;
        for step in 0..GUIDE_CAST_STEPS {
            let p = center + velocity * time;
            let cp = self.closest_point(p, seed);
            seed = Some(cp.t);
            let gap = cp.distance - radius;
            let normal = self.normal_toward(&cp, p);
            let rate = normal.dot(velocity);
            // The lowest gap the next step may reach.
            let floor = if gap <= GUIDE_SKIN {
                if rate < approach {
                    let hit = GuideHit {
                        time,
                        t: cp.t,
                        point: cp.point,
                        normal,
                    };
                    return (Some(GuideCast::Contact(hit)), step + 1);
                }
                // Sliding or leaving: bounded sink, fixed when sliding starts.
                *slide_floor.get_or_insert(gap.min(0.0) - GUIDE_SKIN)
            } else {
                slide_floor = None;
                0.5 * GUIDE_SKIN
            };
            let drop = gap - floor;
            if drop <= 0.0 {
                // Sunk to the sliding limit without ever closing in faster
                // than the approach threshold: hand back to the caller.
                return (Some(GuideCast::Advance { time }), step + 1);
            }
            // Bound 1: |d(gap)/dt| <= speed.
            let mut dt = drop / speed;
            // Bound 2: gap(τ) >= gap + rate τ - ½ K τ², valid while the
            // distance stays below `cap` < reach.
            if cp.distance < 0.9 * self.reach {
                let cap = 0.5 * (cp.distance + self.reach);
                let k = speed * speed / (self.reach - cap);
                // Positive root of drop + rate τ - ½ K τ² = 0.
                // (Written so that neither sign of `rate` cancels.)
                let disc = (rate * rate + 2.0 * k * drop).sqrt();
                let root = if rate < 0.0 {
                    2.0 * drop / (disc - rate)
                } else {
                    (rate + disc) / k
                };
                let within = (cap - cp.distance) / speed;
                let quadratic = root.min(within);
                if quadratic > dt {
                    dt = quadratic;
                }
            }
            let next = time + dt;
            if next >= max_time || next.is_nan() {
                return (None, step + 1);
            }
            if next == time {
                // Below floating-point resolution.
                return (Some(GuideCast::Advance { time }), step + 1);
            }
            time = next;
        }
        (Some(GuideCast::Advance { time }), GUIDE_CAST_STEPS)
    }
}
