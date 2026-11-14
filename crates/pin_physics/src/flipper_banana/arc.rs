//! The circular-arc tube and its closest-point query.

use crate::flipper::{BatPoint, BatShape};
use glam::Vec3;
use pin_core::{FlipperConfig, FlipperShape};

/// The bat-frame side the centreline arc curls toward: the side of its
/// centre of curvature, and of the concave face.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ConcaveSide {
    /// Centre of curvature at `(0, +arc_radius, 0)`: the concave face looks
    /// toward bat-frame `+y` (counter-clockwise about the rotation axis).
    PlusY,
    /// Centre of curvature at `(0, −arc_radius, 0)`.
    MinusY,
}

impl ConcaveSide {
    /// `+1.0` for [`ConcaveSide::PlusY`], `−1.0` for [`ConcaveSide::MinusY`].
    #[must_use]
    pub fn sign(self) -> f32 {
        match self {
            ConcaveSide::PlusY => 1.0,
            ConcaveSide::MinusY => -1.0,
        }
    }

    /// The concave side of a configured flipper: the leading face of the
    /// swing (bat-frame `+y` for `sweep_angle_deg ≥ 0`, `−y` otherwise) for
    /// `arc_sweep_deg ≥ 0`, the trailing face for `arc_sweep_deg < 0`.
    #[must_use]
    pub fn leading(sweep_angle_deg: f32, arc_sweep_deg: f32) -> ConcaveSide {
        if (sweep_angle_deg < 0.0) == (arc_sweep_deg < 0.0) {
            ConcaveSide::PlusY
        } else {
            ConcaveSide::MinusY
        }
    }
}

/// One of the two long faces of the tube.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BananaFace {
    /// The inner face, toward the centre of curvature (the pocket).
    Concave,
    /// The outer face.
    Convex,
}

/// Why a `FlipperShape` could not be turned into a [`CurvedBanana`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BananaError {
    /// The shape is not `FlipperShape::CurvedBanana`.
    NotBanana,
    /// A value is NaN or infinite.
    NonFinite {
        /// Its name in `FlipperConfig` (`"shape.length_mm"` and so on).
        field: &'static str,
    },
    /// A value is outside its valid range: a non-positive length or tube
    /// radius, an arc radius below half the length (no such arc) or not
    /// above the tube radius (no concave face), a non-positive height.
    OutOfRange {
        /// Its name in `FlipperConfig`.
        field: &'static str,
    },
}

impl core::fmt::Display for BananaError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            BananaError::NotBanana => write!(f, "the shape is not a CurvedBanana"),
            BananaError::NonFinite { field } => write!(f, "`{field}` is not a finite number"),
            BananaError::OutOfRange { field } => write!(f, "`{field}` is out of range"),
        }
    }
}

impl std::error::Error for BananaError {}

/// The curved "Banana" bat: a circular-arc tube in the mounting plane,
/// extruded along the rotation axis (see the
/// [module documentation](crate::flipper_banana) for the convention).
///
/// The geometry is kept in `f64`: with a large arc radius the distance to
/// the arc is a small difference of large numbers, and the bat must go over
/// into the straight capsule smoothly.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CurvedBanana {
    length: f32,
    arc_radius: f32,
    tube_radius: f32,
    concave: ConcaveSide,
    bottom: f32,
    top: f32,
    /// Arc radius.
    r: f64,
    /// Arc angle φ.
    phi: f64,
    /// Tip-cap centre in the mirrored plane (concave side up).
    tip: [f64; 2],
    /// Unit tangent of the centreline at the tip, same plane.
    tip_tangent: [f64; 2],
}

impl CurvedBanana {
    /// A bat with chord `length` (pivot to tip-cap centre), centreline
    /// radius `arc_radius` and `tube_radius`, all metres, curling toward
    /// `concave` and spanning `[bottom, top]` along the rotation axis.
    ///
    /// `None` unless all are finite, `length` and `tube_radius` positive,
    /// `arc_radius ≥ length / 2` (the arc is at most a half circle),
    /// `arc_radius > tube_radius` (the concave face exists) and
    /// `bottom < top`.
    #[must_use]
    pub fn new(
        length: f32,
        arc_radius: f32,
        tube_radius: f32,
        concave: ConcaveSide,
        bottom: f32,
        top: f32,
    ) -> Option<Self> {
        let finite = length.is_finite()
            && arc_radius.is_finite()
            && tube_radius.is_finite()
            && bottom.is_finite()
            && top.is_finite();
        if !finite
            || length <= 0.0
            || tube_radius <= 0.0
            || arc_radius < 0.5 * length
            || arc_radius <= tube_radius
            || bottom >= top
        {
            return None;
        }
        let r = f64::from(arc_radius);
        let half = libm::asin((0.5 * f64::from(length) / r).min(1.0));
        let phi = 2.0 * half;
        // P(a) = r·(sin a, 1 − cos a), with 1 − cos a = 2 sin²(a/2).
        let tip = [
            r * libm::sin(phi),
            2.0 * r * libm::sin(half) * libm::sin(half),
        ];
        Some(Self {
            length,
            arc_radius,
            tube_radius,
            concave,
            bottom,
            top,
            r,
            phi,
            tip,
            tip_tangent: [libm::cos(phi), libm::sin(phi)],
        })
    }

    /// The bat of a `FlipperShape::CurvedBanana` (mm → m) on a flipper that
    /// sweeps by `sweep_angle_deg`, standing `bat_height_m` high on its
    /// mounting plane. The concave side is [`ConcaveSide::leading`].
    ///
    /// # Errors
    /// [`BananaError::NotBanana`] for another shape, otherwise the first
    /// non-finite or out-of-range value by its `FlipperConfig` field name
    /// (`"shape.length_mm"`, `"shape.arc_radius_mm"`,
    /// `"shape.arc_sweep_deg"`, `"shape.tube_radius_mm"`,
    /// `"sweep_angle_deg"`, `"tuning.bat_height_m"`).
    pub fn from_shape(
        shape: &FlipperShape,
        sweep_angle_deg: f32,
        bat_height_m: f32,
    ) -> Result<Self, BananaError> {
        let FlipperShape::CurvedBanana {
            length_mm,
            arc_radius_mm,
            arc_sweep_deg,
            tube_radius_mm,
        } = *shape
        else {
            return Err(BananaError::NotBanana);
        };
        let finite = |field: &'static str, value: f32| {
            if value.is_finite() {
                Ok(value)
            } else {
                Err(BananaError::NonFinite { field })
            }
        };
        let length = finite("shape.length_mm", length_mm)? * 1.0e-3;
        let arc_radius = finite("shape.arc_radius_mm", arc_radius_mm)? * 1.0e-3;
        let arc_sweep = finite("shape.arc_sweep_deg", arc_sweep_deg)?;
        let tube = finite("shape.tube_radius_mm", tube_radius_mm)? * 1.0e-3;
        let sweep = finite("sweep_angle_deg", sweep_angle_deg)?;
        let height = finite("tuning.bat_height_m", bat_height_m)?;
        let out = |field: &'static str| Err(BananaError::OutOfRange { field });
        if length <= 0.0 {
            return out("shape.length_mm");
        }
        if tube <= 0.0 {
            return out("shape.tube_radius_mm");
        }
        if arc_radius < 0.5 * length || arc_radius <= tube {
            return out("shape.arc_radius_mm");
        }
        if height <= 0.0 {
            return out("tuning.bat_height_m");
        }
        let concave = ConcaveSide::leading(sweep, arc_sweep);
        // Everything `new` checks has been checked above, field by field.
        Self::new(length, arc_radius, tube, concave, 0.0, height).ok_or(BananaError::OutOfRange {
            field: "shape.arc_radius_mm",
        })
    }

    /// As [`CurvedBanana::from_shape`] with the shape and sweep of `config`.
    ///
    /// # Errors
    /// As [`CurvedBanana::from_shape`].
    pub fn from_config(config: &FlipperConfig, bat_height_m: f32) -> Result<Self, BananaError> {
        Self::from_shape(&config.shape, config.sweep_angle_deg, bat_height_m)
    }

    /// Chord from the pivot to the tip-cap centre, metres.
    #[must_use]
    pub fn length(&self) -> f32 {
        self.length
    }

    /// Radius of the centreline arc, metres.
    #[must_use]
    pub fn arc_radius(&self) -> f32 {
        self.arc_radius
    }

    /// Radius of the tube, metres.
    #[must_use]
    pub fn tube_radius(&self) -> f32 {
        self.tube_radius
    }

    /// Angle the centreline subtends at its centre of curvature, radians:
    /// `2·asin(length / (2·arc_radius))`, in `(0, π]`.
    #[must_use]
    pub fn arc_angle(&self) -> f32 {
        self.phi as f32
    }

    /// Length of the centreline, metres: `arc_radius · arc_angle`.
    #[must_use]
    pub fn arc_length(&self) -> f32 {
        (self.r * self.phi) as f32
    }

    /// The side the arc curls toward.
    #[must_use]
    pub fn concave_side(&self) -> ConcaveSide {
        self.concave
    }

    /// Centre of curvature of the centreline in the bat frame:
    /// `(0, ±arc_radius, 0)`.
    #[must_use]
    pub fn center_of_curvature(&self) -> Vec3 {
        Vec3::new(0.0, self.concave.sign() * self.arc_radius, 0.0)
    }

    /// Centre of the tip cap in the bat frame (`z = 0`), `length` from the
    /// pivot.
    #[must_use]
    pub fn tip_center(&self) -> Vec3 {
        self.unmirror(self.tip[0], self.tip[1], 0.0)
    }

    /// Mirrored-plane coordinates (concave side up) → bat frame.
    fn unmirror(&self, x: f64, y: f64, z: f32) -> Vec3 {
        Vec3::new(x as f32, self.concave.sign() * y as f32, z)
    }

    /// Arc parameter (angle at the centre of curvature, from the pivot) of
    /// `fraction` of the arc length, clamped to the arc.
    fn parameter(&self, fraction: f32) -> f64 {
        let f = if fraction.is_nan() {
            0.0
        } else {
            f64::from(fraction.clamp(0.0, 1.0))
        };
        f * self.phi
    }

    /// Centreline point at `fraction` of the arc length from the pivot
    /// (`0` = pivot, `1` = tip-cap centre; clamped, NaN counts as 0), in the
    /// bat frame at `z = 0`.
    #[must_use]
    pub fn centerline_point(&self, fraction: f32) -> Vec3 {
        let a = self.parameter(fraction);
        let s = libm::sin(0.5 * a);
        self.unmirror(self.r * libm::sin(a), 2.0 * self.r * s * s, 0.0)
    }

    /// Unit tangent of the centreline (pivot toward tip) at `fraction` of
    /// the arc length, in the bat frame.
    #[must_use]
    pub fn centerline_tangent(&self, fraction: f32) -> Vec3 {
        let a = self.parameter(fraction);
        self.unmirror(libm::cos(a), libm::sin(a), 0.0)
    }

    /// The point of `face` at `fraction` of the arc length from the pivot
    /// and the unit outward normal there, in the bat frame at `z = 0`: the
    /// centreline point moved by `tube_radius` along the normal. A ball
    /// "in contact at 25 % along the arc" touches this point.
    #[must_use]
    pub fn face_point(&self, fraction: f32, face: BananaFace) -> (Vec3, Vec3) {
        let a = self.parameter(fraction);
        // Toward the centre of curvature.
        let inward = self.unmirror(-libm::sin(a), libm::cos(a), 0.0);
        let normal = match face {
            BananaFace::Concave => inward,
            BananaFace::Convex => -inward,
        };
        (
            self.centerline_point(fraction) + normal * self.tube_radius,
            normal,
        )
    }

    /// In-plane signed distance and outward unit normal `(nx, ny)` of the
    /// tube's outline at `(x, y)` (bat frame).
    fn outline(&self, x: f32, y: f32) -> (f32, f32, f32) {
        let side = f64::from(self.concave.sign());
        let (px, py) = (f64::from(x), side * f64::from(y));
        let r = self.r;
        // The centre of curvature is (0, r); the arc runs from the pivot
        // (tangent +x) to the tip. A point projects radially onto the arc
        // when it lies in the wedge the arc subtends: ahead of the pivot's
        // normal line and behind the tip's.
        let ahead_of_pivot = px >= 0.0;
        let behind_tip = (self.tip[0] - px) * self.tip_tangent[0]
            + (self.tip[1] - py) * self.tip_tangent[1]
            >= 0.0;
        // (distance to the centreline, unit direction from it to the point)
        let (d, ux, uy) = if ahead_of_pivot && behind_tip {
            let (cx, cy) = (px, py - r);
            let rho = libm::sqrt(cx * cx + cy * cy);
            if rho > 0.0 {
                // ρ − r without the cancellation: (ρ² − r²) / (ρ + r).
                let signed = (px * px + py * py - 2.0 * py * r) / (rho + r);
                let sign = if signed < 0.0 { -1.0 } else { 1.0 };
                (signed.abs(), sign * cx / rho, sign * cy / rho)
            } else {
                // The centre of curvature itself: every arc point is as
                // near; take the pivot's.
                (r, 0.0, 1.0)
            }
        } else {
            let to_pivot = px * px + py * py;
            let (tx, ty) = (px - self.tip[0], py - self.tip[1]);
            let to_tip = tx * tx + ty * ty;
            let (ex, ey, e2) = if to_pivot <= to_tip {
                (px, py, to_pivot)
            } else {
                (tx, ty, to_tip)
            };
            let e = libm::sqrt(e2);
            if e > 0.0 {
                (e, ex / e, ey / e)
            } else {
                (0.0, 0.0, -1.0)
            }
        };
        (
            (d - f64::from(self.tube_radius)) as f32,
            ux as f32,
            (side * uy) as f32,
        )
    }
}

impl BatShape for CurvedBanana {
    fn closest_point(&self, local: Vec3) -> BatPoint {
        let (d2, nx, ny) = self.outline(local.x, local.y);
        // The f64 → f32 rounding of the direction leaves it a unit vector
        // to within an ulp; normalise so the contract is met exactly.
        let side_normal = Vec3::new(nx, ny, 0.0).normalize_or(Vec3::NEG_Y);
        // Above the top face or below the bottom face (positive outside).
        let (dz, cap_normal) = if local.z - self.top >= self.bottom - local.z {
            (local.z - self.top, Vec3::Z)
        } else {
            (self.bottom - local.z, Vec3::NEG_Z)
        };
        if dz <= 0.0 {
            if d2 >= dz {
                // Beside the bat, or inside nearest to the tube wall.
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
        // Nearest to the rim between the tube wall and a face.
        let offset = side_normal * d2 + cap_normal * dz;
        let distance = offset.length();
        BatPoint {
            point: local - offset,
            normal: offset / distance,
            distance,
        }
    }

    fn reach(&self) -> f32 {
        // No centreline point is farther from the pivot than the tip (the
        // chord grows along an arc of at most a half circle).
        self.length + self.tube_radius
    }

    fn height_range(&self) -> (f32, f32) {
        (self.bottom, self.top)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flipper::StraightCapsule;
    use proptest::prelude::*;

    const H: f32 = 0.03;

    fn bat() -> CurvedBanana {
        CurvedBanana::new(0.076, 0.1, 0.008, ConcaveSide::PlusY, 0.0, H).unwrap()
    }

    fn shape(length: f32, radius: f32, sweep: f32, tube: f32) -> FlipperShape {
        FlipperShape::CurvedBanana {
            length_mm: length,
            arc_radius_mm: radius,
            arc_sweep_deg: sweep,
            tube_radius_mm: tube,
        }
    }

    #[test]
    fn rejects_bad_dimensions() {
        let new = |l, r, t, b, top| CurvedBanana::new(l, r, t, ConcaveSide::PlusY, b, top);
        assert!(new(0.076, 0.1, 0.008, 0.0, H).is_some());
        assert!(new(0.0, 0.1, 0.008, 0.0, H).is_none());
        assert!(new(-0.076, 0.1, 0.008, 0.0, H).is_none());
        assert!(new(0.076, 0.1, 0.0, 0.0, H).is_none());
        assert!(new(0.076, f32::NAN, 0.008, 0.0, H).is_none());
        assert!(new(0.076, f32::INFINITY, 0.008, 0.0, H).is_none());
        assert!(new(0.076, 0.1, 0.008, H, H).is_none());
        // No arc of radius 30 mm has a 76 mm chord.
        assert!(new(0.076, 0.03, 0.008, 0.0, H).is_none());
        // A half circle is the limit.
        let half = new(0.076, 0.038, 0.008, 0.0, H).unwrap();
        assert!((half.arc_angle() - core::f32::consts::PI).abs() < 1e-3);
        // The tube must not swallow the centre of curvature.
        assert!(new(0.01, 0.008, 0.008, 0.0, H).is_none());
        assert!(new(0.01, 0.009, 0.008, 0.0, H).is_some());
    }

    #[test]
    fn accessors_and_arc_geometry() {
        let b = bat();
        assert_eq!(
            (b.length(), b.arc_radius(), b.tube_radius()),
            (0.076, 0.1, 0.008)
        );
        assert_eq!(b.concave_side(), ConcaveSide::PlusY);
        assert_eq!(b.height_range(), (0.0, H));
        assert!((b.reach() - 0.084).abs() < 1e-7);
        let phi = 2.0 * libm::asinf(0.38);
        assert!((b.arc_angle() - phi).abs() < 1e-6);
        assert!((b.arc_length() - 0.1 * phi).abs() < 1e-7);
        assert_eq!(b.center_of_curvature(), Vec3::new(0.0, 0.1, 0.0));
        // The tip is `length` from the pivot, half the arc angle off +x.
        let tip = b.tip_center();
        assert!((tip.length() - 0.076).abs() < 1e-7);
        assert!((libm::atan2f(tip.y, tip.x) - 0.5 * phi).abs() < 1e-6);
        assert_eq!(b.centerline_point(0.0), Vec3::ZERO);
        assert!((b.centerline_point(1.0) - tip).length() < 1e-7);
        assert_eq!(b.centerline_point(7.0), b.centerline_point(1.0));
        assert_eq!(b.centerline_point(f32::NAN), Vec3::ZERO);
        // Every centreline point is `arc_radius` from the centre.
        for i in 0..=10 {
            let p = b.centerline_point(i as f32 / 10.0);
            assert!(((p - b.center_of_curvature()).length() - 0.1).abs() < 1e-7);
        }
        // The arc leaves the pivot along +x and has turned by φ at the tip.
        assert_eq!(b.centerline_tangent(0.0), Vec3::X);
        let t = b.centerline_tangent(1.0);
        assert!((libm::atan2f(t.y, t.x) - phi).abs() < 1e-6);
        // Mirror image.
        let m = CurvedBanana::new(0.076, 0.1, 0.008, ConcaveSide::MinusY, 0.0, H).unwrap();
        assert_eq!(m.center_of_curvature(), Vec3::new(0.0, -0.1, 0.0));
        assert!((m.tip_center() - Vec3::new(tip.x, -tip.y, 0.0)).length() < 1e-9);
        assert_eq!(ConcaveSide::PlusY.sign(), 1.0);
        assert_eq!(ConcaveSide::MinusY.sign(), -1.0);
    }

    #[test]
    fn face_points_lie_on_the_surface_with_the_reported_normal() {
        for side in [ConcaveSide::PlusY, ConcaveSide::MinusY] {
            let b = CurvedBanana::new(0.076, 0.1, 0.008, side, 0.0, H).unwrap();
            for face in [BananaFace::Concave, BananaFace::Convex] {
                for i in 1..10 {
                    let f = i as f32 / 10.0;
                    let (p, n) = b.face_point(f, face);
                    let lift = Vec3::new(0.0, 0.0, 0.015);
                    let on = b.closest_point(p + lift);
                    assert!(on.distance.abs() < 1e-6, "{side:?} {face:?} {f}: {on:?}");
                    let off = b.closest_point(p + n * 0.01 + lift);
                    assert!((off.distance - 0.01).abs() < 1e-6);
                    assert!((off.normal - n).length() < 1e-4);
                    assert!((off.point - (p + lift)).length() < 1e-6);
                    // The concave face looks at the centre of curvature.
                    let toward = (b.center_of_curvature() - p).dot(n);
                    assert_eq!(toward > 0.0, face == BananaFace::Concave);
                }
            }
        }
    }

    #[test]
    fn regions_of_the_outline() {
        let b = bat();
        let z = 0.015;
        // Behind the pivot: the base cap.
        let p = b.closest_point(Vec3::new(-0.02, 0.0, z));
        assert!((p.distance - 0.012).abs() < 1e-6);
        assert!((p.normal - Vec3::NEG_X).length() < 1e-6);
        // Beyond the tip, along the tip's tangent: the tip cap.
        let tip = b.tip_center();
        let t = b.centerline_tangent(1.0);
        let p = b.closest_point(tip + t * 0.02 + Vec3::new(0.0, 0.0, z));
        assert!((p.distance - 0.012).abs() < 1e-6);
        assert!((p.normal - t).length() < 1e-5);
        assert!((p.point - (tip + t * 0.008 + Vec3::new(0.0, 0.0, z))).length() < 1e-6);
        // Past the tip on the far side of the centre: still the tip cap.
        let q = Vec3::new(0.09, 0.2, z);
        let p = b.closest_point(q);
        assert!((p.distance - ((q - tip).truncate().length() - 0.008)).abs() < 1e-6);
        // Beyond the centre of curvature, nearer the pivot.
        let q = Vec3::new(-0.1, 0.12, z);
        let p = b.closest_point(q);
        assert!((p.distance - (q.truncate().length() - 0.008)).abs() < 1e-6);
        // The centre of curvature: a defined answer.
        let p = b.closest_point(Vec3::new(0.0, 0.1, z));
        assert!((p.distance - 0.092).abs() < 1e-6);
        assert!(p.normal.is_normalized());
        // On the centreline, deep inside: a defined unit normal.
        let p = b.closest_point(b.centerline_point(0.5) + Vec3::new(0.0, 0.0, z));
        assert!((p.distance + 0.008).abs() < 1e-6);
        assert!(p.normal.is_normalized() && p.normal.z == 0.0);
        // Exactly on the pivot and on the tip-cap centre.
        let p = b.closest_point(Vec3::new(0.0, 0.0, z));
        assert!((p.distance + 0.008).abs() < 1e-6 && p.normal.is_normalized());
        // Just outside the wedge, behind the pivot, on its centre.
        let behind = CurvedBanana::new(0.076, 0.1, 0.008, ConcaveSide::PlusY, 0.0, H).unwrap();
        let p = behind.closest_point(Vec3::new(-0.0, -0.0, z));
        assert!(p.normal.is_normalized());
    }

    #[test]
    fn faces_and_rims() {
        let b = bat();
        let mid = b.centerline_point(0.5);
        // Above the top face.
        let p = b.closest_point(mid + Vec3::new(0.0, 0.0, 0.05));
        assert!((p.distance - 0.02).abs() < 1e-6);
        assert_eq!(p.normal, Vec3::Z);
        // Below the bottom face.
        let p = b.closest_point(mid + Vec3::new(0.0, 0.0, -0.01));
        assert!((p.distance - 0.01).abs() < 1e-6);
        assert_eq!(p.normal, Vec3::NEG_Z);
        // Off the top rim behind the pivot: the diagonal.
        let p = b.closest_point(Vec3::new(-0.011, 0.0, 0.034));
        assert!((p.distance - 0.005).abs() < 1e-6);
        assert!(p.normal.z > 0.0 && p.normal.x < 0.0);
        // Inside, just under the top face.
        let p = b.closest_point(mid + Vec3::new(0.0, 0.0, 0.029));
        assert!((p.distance + 0.001).abs() < 1e-6);
        assert_eq!(p.normal, Vec3::Z);
        // Inside, near the tube wall.
        let (face, n) = b.face_point(0.5, BananaFace::Concave);
        let p = b.closest_point(face - n * 0.0005 + Vec3::new(0.0, 0.0, 0.015));
        assert!((p.distance + 0.0005).abs() < 1e-6);
        assert!(p.normal.z == 0.0 && (p.normal - n).length() < 1e-4);
    }

    #[test]
    fn surface_velocity_is_rigid_rotation() {
        let b = bat();
        let (p, _) = b.face_point(0.85, BananaFace::Concave);
        let v = b.surface_velocity(p, 100.0);
        // ω ẑ × r.
        assert!((v - Vec3::Z.cross(p) * 100.0).length() < 1e-5);
        assert!(v.length() <= 100.0 * b.reach());
    }

    #[test]
    fn from_shape_reads_the_contract() {
        let b = CurvedBanana::from_shape(&shape(80.0, 100.0, 30.0, 8.0), 50.0, H).unwrap();
        assert!((b.length() - 0.08).abs() < 1e-8);
        assert!((b.arc_radius() - 0.1).abs() < 1e-8);
        assert!((b.tube_radius() - 0.008).abs() < 1e-9);
        assert_eq!(b.height_range(), (0.0, H));
        // Only the sign of arc_sweep_deg is read; the arc angle follows
        // from chord and radius.
        assert!((b.arc_angle() - 2.0 * libm::asinf(0.4)).abs() < 1e-6);
        let other = CurvedBanana::from_shape(&shape(80.0, 100.0, 5.0, 8.0), 50.0, H).unwrap();
        assert_eq!(b, other);
        // The concave face leads the swing unless arc_sweep_deg is negative.
        let side = |sweep: f32, arc: f32| {
            CurvedBanana::from_shape(&shape(80.0, 100.0, arc, 8.0), sweep, H)
                .unwrap()
                .concave_side()
        };
        assert_eq!(side(50.0, 30.0), ConcaveSide::PlusY);
        assert_eq!(side(-50.0, 30.0), ConcaveSide::MinusY);
        assert_eq!(side(50.0, -30.0), ConcaveSide::MinusY);
        assert_eq!(side(-50.0, -30.0), ConcaveSide::PlusY);
        assert_eq!(side(0.0, 0.0), ConcaveSide::PlusY);
    }

    #[test]
    fn from_shape_names_the_bad_field() {
        let err = |s: FlipperShape, sweep: f32, h: f32| {
            CurvedBanana::from_shape(&s, sweep, h).unwrap_err()
        };
        let straight = FlipperShape::StraightCapsule {
            length_mm: 76.0,
            base_radius_mm: 11.0,
            tip_radius_mm: 6.5,
        };
        assert_eq!(err(straight, 50.0, H), BananaError::NotBanana);
        let nf = |field| BananaError::NonFinite { field };
        let oor = |field| BananaError::OutOfRange { field };
        let nan = f32::NAN;
        assert_eq!(
            err(shape(nan, 100.0, 30.0, 8.0), 50.0, H),
            nf("shape.length_mm")
        );
        assert_eq!(
            err(shape(80.0, f32::INFINITY, 30.0, 8.0), 50.0, H),
            nf("shape.arc_radius_mm")
        );
        assert_eq!(
            err(shape(80.0, 100.0, nan, 8.0), 50.0, H),
            nf("shape.arc_sweep_deg")
        );
        assert_eq!(
            err(shape(80.0, 100.0, 30.0, nan), 50.0, H),
            nf("shape.tube_radius_mm")
        );
        assert_eq!(
            err(shape(80.0, 100.0, 30.0, 8.0), nan, H),
            nf("sweep_angle_deg")
        );
        assert_eq!(
            err(shape(80.0, 100.0, 30.0, 8.0), 50.0, nan),
            nf("tuning.bat_height_m")
        );
        assert_eq!(
            err(shape(0.0, 100.0, 30.0, 8.0), 50.0, H),
            oor("shape.length_mm")
        );
        assert_eq!(
            err(shape(80.0, 100.0, 30.0, 0.0), 50.0, H),
            oor("shape.tube_radius_mm")
        );
        assert_eq!(
            err(shape(80.0, 39.0, 30.0, 8.0), 50.0, H),
            oor("shape.arc_radius_mm")
        );
        assert_eq!(
            err(shape(10.0, 8.0, 30.0, 8.0), 50.0, H),
            oor("shape.arc_radius_mm")
        );
        assert_eq!(
            err(shape(80.0, 100.0, 30.0, 8.0), 50.0, 0.0),
            oor("tuning.bat_height_m")
        );
        for e in [
            BananaError::NotBanana,
            nf("shape.length_mm"),
            oor("shape.arc_radius_mm"),
        ] {
            assert!(!e.to_string().is_empty());
        }
        assert!(
            nf("shape.length_mm")
                .to_string()
                .contains("shape.length_mm")
        );
    }

    #[test]
    fn a_huge_radius_is_the_straight_capsule() {
        let straight = StraightCapsule::new(0.076, 0.008, 0.008, 0.0, H).unwrap();
        for radius in [1.0e3f32, 1.0e6, 1.0e12, 1.0e30, f32::MAX] {
            let b = CurvedBanana::new(0.076, radius, 0.008, ConcaveSide::PlusY, 0.0, H).unwrap();
            for (x, y) in [
                (0.03, 0.02),
                (0.03, -0.02),
                (-0.02, 0.01),
                (0.1, 0.01),
                (0.05, 0.001),
                (0.076, -0.03),
            ] {
                let p = Vec3::new(x, y, 0.015);
                let (a, s) = (b.closest_point(p), straight.closest_point(p));
                let tol = 0.08 * 0.08 / radius + 1e-7;
                assert!((a.distance - s.distance).abs() <= tol, "{radius} {p}");
                assert!((a.normal - s.normal).length() <= 2.0 * 0.08 / radius + 1e-5);
            }
        }
    }

    /// Distance from `(x, y)` to the nearest of many points of the
    /// centreline, minus the tube radius.
    fn brute_force(b: &CurvedBanana, x: f32, y: f32, samples: u32) -> f64 {
        let s = f64::from(b.concave.sign());
        let mut best = f64::INFINITY;
        for i in 0..=samples {
            let a = b.phi * f64::from(i) / f64::from(samples);
            let (cx, cy) = (b.r * libm::sin(a), s * b.r * (1.0 - libm::cos(a)));
            let (dx, dy) = (f64::from(x) - cx, f64::from(y) - cy);
            best = best.min(libm::sqrt(dx * dx + dy * dy));
        }
        best - f64::from(b.tube_radius)
    }

    proptest! {
        /// The analytic distance equals the brute-force one (never above
        /// it; below it by at most the sampling error), for both bends and
        /// arcs from nearly straight to a half circle.
        #[test]
        fn closest_point_matches_brute_force(
            x in -0.06f32..0.14, y in -0.12f32..0.12,
            length in 0.03f32..0.1, bend in 0.0f32..1.0, tube in 0.003f32..0.012,
            plus in any::<bool>(),
        ) {
            // bend 0 → half circle, bend 1 → 2 m radius.
            let radius = (0.5 * length).max(tube * 1.01) + bend * bend * 2.0;
            let side = if plus { ConcaveSide::PlusY } else { ConcaveSide::MinusY };
            let b = CurvedBanana::new(length, radius, tube, side, 0.0, H).unwrap();
            let full = f64::from(b.closest_point(Vec3::new(x, y, 0.015)).distance);
            // Inside, the top and bottom faces can be nearer than the wall.
            let got = f64::from(b.outline(x, y).0);
            prop_assert!(full == got || (got < 0.0 && full >= got));
            let samples = 4000;
            let want = brute_force(&b, x, y, samples);
            // A sampled polyline is never nearer than the arc, and at most
            // half a sample spacing farther.
            let spacing = f64::from(b.arc_length()) / f64::from(samples);
            prop_assert!(got <= want + 2e-7, "{got} > {want}");
            prop_assert!(got >= want - 0.5 * spacing - 2e-7, "{got} < {want}");
        }

        /// The signed distance is 1-Lipschitz, the closest point lies on the
        /// surface along the reported normal, and no surface point is beyond
        /// `reach` (the facts the swept test relies on).
        #[test]
        fn closest_point_invariants(
            x in -0.05f32..0.14, y in -0.1f32..0.1, z in -0.03f32..0.06,
            dx in -0.01f32..0.01, dy in -0.01f32..0.01, dz in -0.01f32..0.01,
            radius in 0.04f32..1.0, tube in 0.003f32..0.012, plus in any::<bool>(),
        ) {
            let side = if plus { ConcaveSide::PlusY } else { ConcaveSide::MinusY };
            let b = CurvedBanana::new(0.076, radius, tube, side, 0.0, H).unwrap();
            let p = Vec3::new(x, y, z);
            let q = p + Vec3::new(dx, dy, dz);
            let (a, c) = (b.closest_point(p), b.closest_point(q));
            prop_assert!((a.distance - c.distance).abs() <= (p - q).length() + 1e-6);
            prop_assert!((a.normal.length() - 1.0).abs() < 1e-4);
            prop_assert!((a.point + a.normal * a.distance - p).length() < 1e-5);
            // Outside, the surface point is on the surface. (Deep inside a
            // strongly curved tube the nearest wall along the normal can be
            // the far one; the swept test never asks there.)
            if a.distance >= 0.0 {
                prop_assert!(b.closest_point(a.point).distance.abs() < 1e-5);
            }
            let rho = libm::sqrtf(a.point.x * a.point.x + a.point.y * a.point.y);
            if a.distance >= 0.0 {
                prop_assert!(rho <= b.reach() + 1e-5);
            }
        }

        /// Mirror symmetry: the two bends are mirror images in y.
        #[test]
        fn bends_are_mirror_images(x in -0.05f32..0.14, y in -0.1f32..0.1, radius in 0.04f32..1.0) {
            let up = CurvedBanana::new(0.076, radius, 0.008, ConcaveSide::PlusY, 0.0, H).unwrap();
            let down = CurvedBanana::new(0.076, radius, 0.008, ConcaveSide::MinusY, 0.0, H).unwrap();
            let (a, c) = (
                up.closest_point(Vec3::new(x, y, 0.015)),
                down.closest_point(Vec3::new(x, -y, 0.015)),
            );
            prop_assert_eq!(a.distance, c.distance);
            prop_assert_eq!(a.normal.x, c.normal.x);
            prop_assert_eq!(a.normal.y, -c.normal.y);
        }
    }
}
