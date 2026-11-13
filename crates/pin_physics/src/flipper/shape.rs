//! Bat shapes in the bat's own frame.
//!
//! The **bat frame** has its origin at the pivot, `x` along the bat (pivot
//! toward the tip), `z` along the mounting plane's normal (the rotation
//! axis) and `y = z × x`. A [`BatShape`] is a rigid surface in that frame;
//! [`BatMotion`] carries it through the tick (rotation about `z`, zipper
//! slide of the pivot), and the flipper solver only ever talks to the shape
//! through this trait. Subphase 2.6 adds the curved "Banana" bat by
//! implementing the trait in [`crate::flipper_banana`].

use crate::ball::{BODY_REST_SPEED, BODY_SKIN, BodyCast, BodyPoint, MAX_CAST_ITERATIONS};
use glam::Vec3;

/// Angle (radians) within which a bat counts as being at a stop.
pub const STOP_ANGLE_TOLERANCE: f32 = 1.0e-5;

/// Closest point of a bat surface, in the bat frame (metres).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BatPoint {
    /// Closest surface point.
    pub point: Vec3,
    /// Unit outward normal at `point`.
    pub normal: Vec3,
    /// Signed distance of the query point from the surface (negative inside).
    pub distance: f32,
}

/// Pose and velocity of a bat during one piece of a tick: the pivot slides
/// at constant velocity and the bat turns at constant angular velocity about
/// `axis`, with the angle limited to `[angle_min, angle_max]` (end stops).
///
/// `time` arguments are seconds since the start of the tick; the motion is
/// valid from `start` on.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BatMotion {
    /// Time the piece starts.
    pub start: f32,
    /// Pivot at `start`, world space, metres.
    pub pivot: Vec3,
    /// Pivot velocity, m/s.
    pub pivot_velocity: Vec3,
    /// Time after which the pivot no longer moves (slide end reached).
    pub pivot_stop_time: f32,
    /// Unit rotation axis (the mounting plane's normal).
    pub axis: Vec3,
    /// In-plane unit vector of angle 0.
    pub zero: Vec3,
    /// `axis × zero`: in-plane unit vector of angle +90°.
    pub quarter: Vec3,
    /// Angle about `axis` at `start`, radians.
    pub angle: f32,
    /// Angular velocity about `axis`, rad/s.
    pub angular_velocity: f32,
    /// Lowest angle the stops allow, radians.
    pub angle_min: f32,
    /// Highest angle the stops allow, radians.
    pub angle_max: f32,
}

impl BatMotion {
    /// Bat angle at `time`.
    #[must_use]
    pub fn angle_at(&self, time: f32) -> f32 {
        let a = self.angle + self.angular_velocity * (time - self.start).max(0.0);
        a.clamp(self.angle_min, self.angle_max)
    }

    /// Angular velocity at `time`: zero once a stop holds the bat.
    #[must_use]
    pub fn angular_velocity_at(&self, time: f32) -> f32 {
        let a = self.angle + self.angular_velocity * (time - self.start).max(0.0);
        let w = self.angular_velocity;
        if (w > 0.0 && self.angle_max - a <= STOP_ANGLE_TOLERANCE)
            || (w < 0.0 && a - self.angle_min <= STOP_ANGLE_TOLERANCE)
        {
            0.0
        } else {
            w
        }
    }

    /// Pivot position at `time`.
    #[must_use]
    pub fn pivot_at(&self, time: f32) -> Vec3 {
        let s = (time.min(self.pivot_stop_time) - self.start).max(0.0);
        self.pivot + self.pivot_velocity * s
    }

    /// Pivot velocity at `time`.
    #[must_use]
    pub fn pivot_velocity_at(&self, time: f32) -> Vec3 {
        if time < self.pivot_stop_time {
            self.pivot_velocity
        } else {
            Vec3::ZERO
        }
    }

    /// Unit vector along the bat (bat-frame `x`) at `time`.
    #[must_use]
    pub fn direction_at(&self, time: f32) -> Vec3 {
        let a = self.angle_at(time);
        self.zero * libm::cosf(a) + self.quarter * libm::sinf(a)
    }

    /// World point → bat frame at `time`.
    #[must_use]
    pub fn to_local(&self, time: f32, world: Vec3) -> Vec3 {
        let x = self.direction_at(time);
        let y = self.axis.cross(x);
        let d = world - self.pivot_at(time);
        Vec3::new(d.dot(x), d.dot(y), d.dot(self.axis))
    }

    /// Bat-frame point → world at `time`.
    #[must_use]
    pub fn to_world(&self, time: f32, local: Vec3) -> Vec3 {
        self.pivot_at(time) + self.vector_to_world(time, local)
    }

    /// Bat-frame vector → world at `time`.
    #[must_use]
    pub fn vector_to_world(&self, time: f32, local: Vec3) -> Vec3 {
        let x = self.direction_at(time);
        let y = self.axis.cross(x);
        x * local.x + y * local.y + self.axis * local.z
    }

    /// World velocity at `time` of the bat's material point at world
    /// position `point`.
    #[must_use]
    pub fn velocity_at(&self, time: f32, point: Vec3) -> Vec3 {
        let r = point - self.pivot_at(time);
        self.axis.cross(r) * self.angular_velocity_at(time) + self.pivot_velocity_at(time)
    }
}

/// A rigid bat surface in the bat frame (see the [module docs](self)).
///
/// Implementations must be exact in [`closest_point`](BatShape::closest_point)
/// (the continuous collision test relies on the distance never being
/// over-estimated) and must not allocate.
pub trait BatShape {
    /// Closest surface point to `local` (bat frame), with the outward normal
    /// and the signed distance.
    fn closest_point(&self, local: Vec3) -> BatPoint;

    /// The largest distance of any surface point from the rotation axis
    /// (bounds the surface speed, `|ω|·reach`).
    fn reach(&self) -> f32;

    /// The extent of the surface along the rotation axis, `(low, high)` in
    /// the bat frame's `z`.
    fn height_range(&self) -> (f32, f32);

    /// Velocity, in the bat frame, of the material point at `local` when the
    /// bat turns at `angular_velocity` (rad/s about `z`): `ω ẑ × p`.
    fn surface_velocity(&self, local: Vec3, angular_velocity: f32) -> Vec3 {
        Vec3::new(-local.y, local.x, 0.0) * angular_velocity
    }

    /// Swept test: the first time within `window` seconds after `time` at
    /// which a sphere (`center + velocity·s`, world space) touches the bat
    /// while closing in, with the bat following `motion`.
    ///
    /// The default is conservative advancement evaluated in the bat frame:
    /// the gap cannot close faster than the sphere's speed plus the pivot's
    /// speed plus `|ω|·reach`, so stepping by gap / that bound never steps
    /// over a contact, whatever the speeds. Same reporting rules as
    /// [`conservative_cast`](crate::ball::conservative_cast): a sphere that
    /// touches at `s = 0` without closing faster than
    /// [`BODY_REST_SPEED`] is not a hit.
    fn cast_sphere(
        &self,
        motion: &BatMotion,
        time: f32,
        center: Vec3,
        radius: f32,
        velocity: Vec3,
        window: f32,
    ) -> Option<BodyCast> {
        if window.is_nan() || window <= 0.0 {
            return None;
        }
        let bound = velocity.length()
            + motion.pivot_velocity.length()
            + motion.angular_velocity.abs() * self.reach();
        if !bound.is_finite() {
            return None;
        }
        let mut s = 0.0f32;
        for _ in 0..MAX_CAST_ITERATIONS {
            let t = time + s;
            let p = center + velocity * s;
            let cp = world_closest_point(self, motion, t, p);
            let gap = cp.distance - radius;
            if gap.is_nan() {
                return None;
            }
            let step = if gap <= BODY_SKIN {
                let closing = (velocity - motion.velocity_at(t, cp.point)).dot(cp.normal);
                if closing < -BODY_REST_SPEED {
                    return Some(BodyCast::Hit {
                        time: s,
                        point: cp.point,
                        normal: cp.normal,
                    });
                }
                if s == 0.0 {
                    return None;
                }
                BODY_SKIN / bound
            } else {
                (gap - 0.5 * BODY_SKIN) / bound
            };
            if step.is_nan() || step <= 0.0 || !step.is_finite() {
                return None;
            }
            s += step;
            if s > window {
                return None;
            }
        }
        Some(BodyCast::Advance { time: s })
    }
}

/// Closest point of `shape`, carried by `motion`, to world point `point` at
/// `time`.
pub fn world_closest_point<S: BatShape + ?Sized>(
    shape: &S,
    motion: &BatMotion,
    time: f32,
    point: Vec3,
) -> BodyPoint {
    let cp = shape.closest_point(motion.to_local(time, point));
    BodyPoint {
        point: motion.to_world(time, cp.point),
        normal: motion.vector_to_world(time, cp.normal),
        distance: cp.distance,
    }
}

/// The standard bat: a tapered capsule (two circles and their outer
/// tangents) in the mounting plane, extruded along the rotation axis.
///
/// The base circle (radius `base_radius`) is centred on the pivot, the tip
/// circle (radius `tip_radius`) on the bat axis at `length` from the pivot,
/// so the bat reaches `length + tip_radius` from the pivot. Along the
/// rotation axis the bat spans `[bottom, top]` (the mounting plane is
/// `z = 0`); its top and bottom faces are flat.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct StraightCapsule {
    length: f32,
    base_radius: f32,
    tip_radius: f32,
    bottom: f32,
    top: f32,
    /// `(base_radius − tip_radius) / length`: sine of the taper half-angle.
    taper_sin: f32,
    /// Cosine of the taper half-angle.
    taper_cos: f32,
}

impl StraightCapsule {
    /// A bat of `length` (pivot to tip-circle centre), `base_radius` and
    /// `tip_radius`, all metres, spanning `[bottom, top]` along the rotation
    /// axis. `None` unless all are finite, the radii positive, `length`
    /// longer than the radii differ (neither circle inside the other) and
    /// `bottom < top`.
    #[must_use]
    pub fn new(
        length: f32,
        base_radius: f32,
        tip_radius: f32,
        bottom: f32,
        top: f32,
    ) -> Option<Self> {
        let finite = length.is_finite()
            && base_radius.is_finite()
            && tip_radius.is_finite()
            && bottom.is_finite()
            && top.is_finite();
        if !finite || base_radius <= 0.0 || tip_radius <= 0.0 || length <= 0.0 || bottom >= top {
            return None;
        }
        let taper_sin = (base_radius - tip_radius) / length;
        if taper_sin.abs() >= 1.0 {
            return None;
        }
        Some(Self {
            length,
            base_radius,
            tip_radius,
            bottom,
            top,
            taper_sin,
            taper_cos: libm::sqrtf(1.0 - taper_sin * taper_sin),
        })
    }

    /// Pivot to tip-circle centre, metres.
    #[must_use]
    pub fn length(&self) -> f32 {
        self.length
    }

    /// Radius at the pivot, metres.
    #[must_use]
    pub fn base_radius(&self) -> f32 {
        self.base_radius
    }

    /// Radius at the tip, metres.
    #[must_use]
    pub fn tip_radius(&self) -> f32 {
        self.tip_radius
    }

    /// In-plane signed distance and outward unit normal `(nx, ny)` of the
    /// tapered capsule's outline at `(x, y)`.
    fn outline(&self, x: f32, y: f32) -> (f32, f32, f32) {
        let side = if y < 0.0 { -1.0 } else { 1.0 };
        let lateral = y.abs();
        // Position along the slanted side, measured from the base circle's
        // tangent point.
        let along = x * self.taper_cos - lateral * self.taper_sin;
        let round = |cx: f32, radius: f32| {
            let (dx, dy) = (x - cx, y);
            let d = libm::sqrtf(dx * dx + dy * dy);
            if d > 1.0e-9 {
                (d - radius, dx / d, dy / d)
            } else {
                (-radius, 0.0, 1.0)
            }
        };
        if along <= 0.0 {
            round(0.0, self.base_radius)
        } else if along >= self.taper_cos * self.length {
            round(self.length, self.tip_radius)
        } else {
            (
                x * self.taper_sin + lateral * self.taper_cos - self.base_radius,
                self.taper_sin,
                self.taper_cos * side,
            )
        }
    }
}

impl BatShape for StraightCapsule {
    fn closest_point(&self, local: Vec3) -> BatPoint {
        let (d2, nx, ny) = self.outline(local.x, local.y);
        let side_normal = Vec3::new(nx, ny, 0.0);
        // Above the top face or below the bottom face (positive outside).
        let (dz, cap_normal) = if local.z - self.top >= self.bottom - local.z {
            (local.z - self.top, Vec3::Z)
        } else {
            (self.bottom - local.z, Vec3::NEG_Z)
        };
        if dz <= 0.0 {
            if d2 >= dz {
                // Beside the bat, or inside nearest to the side wall.
                return BatPoint {
                    point: local - side_normal * d2,
                    normal: side_normal,
                    distance: d2,
                };
            }
            return BatPoint {
                point: local - cap_normal * dz,
                normal: cap_normal,
                distance: dz,
            };
        }
        if d2 <= 0.0 {
            // Over a flat face.
            return BatPoint {
                point: local - cap_normal * dz,
                normal: cap_normal,
                distance: dz,
            };
        }
        // Nearest to the rim between the side wall and a face.
        let offset = side_normal * d2 + cap_normal * dz;
        let distance = offset.length();
        BatPoint {
            point: local - offset,
            normal: offset / distance,
            distance,
        }
    }

    fn reach(&self) -> f32 {
        self.base_radius.max(self.length + self.tip_radius)
    }

    fn height_range(&self) -> (f32, f32) {
        (self.bottom, self.top)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn bat() -> StraightCapsule {
        StraightCapsule::new(0.076, 0.011, 0.0065, 0.0, 0.03).unwrap()
    }

    #[test]
    fn rejects_bad_dimensions() {
        assert!(StraightCapsule::new(0.076, 0.011, 0.0065, 0.0, 0.03).is_some());
        assert!(StraightCapsule::new(0.0, 0.011, 0.0065, 0.0, 0.03).is_none());
        assert!(StraightCapsule::new(0.076, 0.0, 0.0065, 0.0, 0.03).is_none());
        assert!(StraightCapsule::new(0.076, 0.011, -1.0, 0.0, 0.03).is_none());
        assert!(StraightCapsule::new(f32::NAN, 0.011, 0.0065, 0.0, 0.03).is_none());
        assert!(StraightCapsule::new(0.076, 0.011, 0.0065, 0.03, 0.03).is_none());
        // One circle inside the other.
        assert!(StraightCapsule::new(0.004, 0.011, 0.0065, 0.0, 0.03).is_none());
        let b = bat();
        assert_eq!(
            (b.length(), b.base_radius(), b.tip_radius()),
            (0.076, 0.011, 0.0065)
        );
        assert_eq!(b.height_range(), (0.0, 0.03));
        assert!((b.reach() - 0.0825).abs() < 1e-7);
    }

    #[test]
    fn regions_of_the_outline() {
        let b = bat();
        // Behind the pivot: base circle.
        let p = b.closest_point(Vec3::new(-0.02, 0.0, 0.01));
        assert!((p.distance - 0.009).abs() < 1e-6);
        assert!((p.normal - Vec3::NEG_X).length() < 1e-6);
        // Beyond the tip: tip circle.
        let p = b.closest_point(Vec3::new(0.1, 0.0, 0.01));
        assert!((p.distance - (0.1 - 0.0825)).abs() < 1e-6);
        assert!((p.point - Vec3::new(0.0825, 0.0, 0.01)).length() < 1e-6);
        // Beside the middle: the slanted side, radius between base and tip.
        let p = b.closest_point(Vec3::new(0.038, 0.05, 0.01));
        assert!(p.normal.y > 0.99 && p.normal.x > 0.0);
        let radius_here = 0.05 - p.distance;
        assert!(radius_here > 0.0065 && radius_here < 0.011);
        // Mirror image below the axis.
        let q = b.closest_point(Vec3::new(0.038, -0.05, 0.01));
        assert!((q.distance - p.distance).abs() < 1e-7);
        assert!((q.normal.y + p.normal.y).abs() < 1e-7);
        // Dead centre of the base circle: a defined normal.
        let c = b.closest_point(Vec3::new(0.0, 0.0, 0.015));
        assert!(c.distance < 0.0 && c.normal.is_normalized());
    }

    #[test]
    fn faces_and_rims() {
        let b = bat();
        // Above the top face.
        let p = b.closest_point(Vec3::new(0.03, 0.0, 0.05));
        assert!((p.distance - 0.02).abs() < 1e-6);
        assert_eq!(p.normal, Vec3::Z);
        // Below the bottom face.
        let p = b.closest_point(Vec3::new(0.03, 0.0, -0.01));
        assert!((p.distance - 0.01).abs() < 1e-6);
        assert_eq!(p.normal, Vec3::NEG_Z);
        // Off the top rim: distance is the diagonal.
        let p = b.closest_point(Vec3::new(-0.014, 0.0, 0.034));
        assert!((p.distance - libm::sqrtf(0.003 * 0.003 + 0.004 * 0.004)).abs() < 1e-6);
        assert!(p.normal.z > 0.0 && p.normal.x < 0.0);
        // Inside, just under the top face.
        let p = b.closest_point(Vec3::new(0.03, 0.0, 0.029));
        assert!((p.distance + 0.001).abs() < 1e-6);
        assert_eq!(p.normal, Vec3::Z);
        // Inside, near the side wall.
        let p = b.closest_point(Vec3::new(0.0, 0.0105, 0.015));
        assert!(p.distance < 0.0 && p.distance > -0.001);
        assert!(p.normal.z == 0.0);
    }

    #[test]
    fn surface_velocity_is_rigid_rotation() {
        let b = bat();
        let v = b.surface_velocity(Vec3::new(0.05, 0.0, 0.01), 10.0);
        assert!((v - Vec3::new(0.0, 0.5, 0.0)).length() < 1e-7);
    }

    fn still(angle: f32) -> BatMotion {
        BatMotion {
            start: 0.0,
            pivot: Vec3::new(0.1, 0.2, 0.0),
            pivot_velocity: Vec3::ZERO,
            pivot_stop_time: 0.0,
            axis: Vec3::Z,
            zero: Vec3::X,
            quarter: Vec3::Y,
            angle,
            angular_velocity: 0.0,
            angle_min: -10.0,
            angle_max: 10.0,
        }
    }

    #[test]
    fn motion_limits_and_slide() {
        let mut m = still(0.0);
        m.angular_velocity = 100.0;
        m.angle_max = 0.05;
        m.pivot_velocity = Vec3::new(1.0, 0.0, 0.0);
        m.pivot_stop_time = 0.4e-3;
        assert!((m.angle_at(0.2e-3) - 0.02).abs() < 1e-6);
        assert_eq!(m.angle_at(1.0e-3), 0.05);
        assert_eq!(m.angular_velocity_at(0.2e-3), 100.0);
        assert_eq!(m.angular_velocity_at(1.0e-3), 0.0);
        assert!((m.pivot_at(0.2e-3).x - 0.1002).abs() < 1e-7);
        assert!((m.pivot_at(1.0e-3).x - 0.1004).abs() < 1e-7);
        assert_eq!(m.pivot_velocity_at(0.2e-3), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(m.pivot_velocity_at(0.5e-3), Vec3::ZERO);
        // A point on the bat moves with rotation plus slide.
        let p = m.pivot_at(0.2e-3) + m.direction_at(0.2e-3) * 0.05;
        let v = m.velocity_at(0.2e-3, p);
        assert!((v.y - 5.0).abs() < 0.01 && (v.x - 0.9).abs() < 0.01, "{v}");
        m.angular_velocity = -100.0;
        m.angle_min = -0.05;
        assert_eq!(m.angular_velocity_at(1.0e-3), 0.0);
    }

    #[test]
    fn cast_hits_a_still_bat_and_skips_resting_contact() {
        let b = bat();
        let m = still(0.0);
        let r = 0.0135;
        // Falling onto the middle of the bat from +y.
        let start = m.pivot + Vec3::new(0.038, 0.05, 0.0135);
        let hit = b.cast_sphere(&m, 0.0, start, r, Vec3::new(0.0, -4.0, 0.0), 0.05);
        let Some(BodyCast::Hit { time, normal, .. }) = hit else {
            panic!("expected a hit, got {hit:?}");
        };
        let cp = world_closest_point(&b, &m, time, start + Vec3::new(0.0, -4.0, 0.0) * time);
        assert!((cp.distance - r).abs() <= 2.0 * BODY_SKIN);
        assert!(normal.y > 0.99);
        // Touching without closing in: not a hit.
        let rest = start + Vec3::new(0.0, -4.0, 0.0) * time;
        assert_eq!(b.cast_sphere(&m, 0.0, rest, r, Vec3::ZERO, 0.05), None);
        // Moving away, a miss, bad windows.
        assert_eq!(
            b.cast_sphere(&m, 0.0, start, r, Vec3::new(0.0, 4.0, 0.0), 0.05),
            None
        );
        assert_eq!(
            b.cast_sphere(&m, 0.0, start, r, Vec3::new(0.0, -4.0, 0.0), 0.0),
            None
        );
        assert_eq!(
            b.cast_sphere(&m, 0.0, start, r, Vec3::new(0.0, -4.0, 0.0), f32::NAN),
            None
        );
        assert_eq!(
            b.cast_sphere(&m, 0.0, start, r, Vec3::new(0.0, f32::INFINITY, 0.0), 0.05),
            None
        );
        assert_eq!(
            b.cast_sphere(&m, 0.0, Vec3::splat(f32::NAN), r, Vec3::Y, 0.05),
            None
        );
    }

    #[test]
    fn cast_gives_up_after_the_iteration_budget() {
        // Grazing along the bat inside the skin without closing in creeps
        // forward and finally reports how far it got.
        let b = bat();
        let m = still(0.0);
        let r = 0.0135;
        let start = m.pivot + Vec3::new(0.02, 0.05, 0.0135);
        let Some(BodyCast::Hit { time, .. }) =
            b.cast_sphere(&m, 0.0, start, r, Vec3::new(0.0, -1.0, 0.0), 0.1)
        else {
            panic!("no hit");
        };
        let touching = start + Vec3::new(0.0, -1.0, 0.0) * (time - 20.0e-6);
        // Parallel to the slanted side: never closes.
        let along = Vec3::new(b.taper_cos, -b.taper_sin, 0.0) * 0.01;
        let out = b.cast_sphere(&m, 0.0, touching, r, along, 1.0);
        assert!(
            matches!(out, Some(BodyCast::Advance { time }) if time > 0.0) || out.is_none(),
            "{out:?}"
        );
    }

    proptest! {
        /// The signed distance is 1-Lipschitz and the closest point lies on
        /// the surface along the reported normal (the two facts the swept
        /// test relies on).
        #[test]
        fn closest_point_invariants(
            x in -0.05f32..0.14, y in -0.06f32..0.06, z in -0.03f32..0.06,
            dx in -0.01f32..0.01, dy in -0.01f32..0.01, dz in -0.01f32..0.01,
            base in 0.004f32..0.015, tip in 0.003f32..0.015,
        ) {
            let b = StraightCapsule::new(0.076, base, tip, 0.0, 0.03).unwrap();
            let p = Vec3::new(x, y, z);
            let q = p + Vec3::new(dx, dy, dz);
            let (a, c) = (b.closest_point(p), b.closest_point(q));
            prop_assert!((a.distance - c.distance).abs() <= (p - q).length() + 1e-6);
            prop_assert!((a.normal.length() - 1.0).abs() < 1e-4);
            // Reconstruct the query point from the surface point.
            prop_assert!((a.point + a.normal * a.distance - p).length() < 1e-5);
            // The surface point is on the surface.
            prop_assert!(b.closest_point(a.point).distance.abs() < 1e-5);
            // No surface point is farther from the axis than `reach`.
            let rho = libm::sqrtf(a.point.x * a.point.x + a.point.y * a.point.y);
            prop_assert!(rho <= b.reach() + 1e-5);
        }

        /// World ↔ bat frame round trip for any pose.
        #[test]
        fn frame_round_trip(angle in -3.0f32..3.0, x in -0.1f32..0.1, y in -0.1f32..0.1, z in -0.1f32..0.1) {
            let m = still(angle);
            let p = Vec3::new(x, y, z);
            prop_assert!((m.to_world(0.0, m.to_local(0.0, p)) - p).length() < 1e-6);
        }
    }
}
