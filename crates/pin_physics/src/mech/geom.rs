//! Exact closest points of the mech body shapes: oriented boxes (kickers,
//! targets, plunger tip, spinner flap) and upright cylinders (bumpers).

use glam::Vec3;

use crate::ball::BodyPoint;

/// An oriented box: centre, orthonormal axes and half extents along them.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct OBox {
    pub center: Vec3,
    pub axes: [Vec3; 3],
    pub half: Vec3,
}

impl OBox {
    /// The closest surface point to `p` (signed distance, outward normal).
    pub fn closest(&self, p: Vec3) -> BodyPoint {
        let d = p - self.center;
        let local = Vec3::new(
            d.dot(self.axes[0]),
            d.dot(self.axes[1]),
            d.dot(self.axes[2]),
        );
        let h = self.half;
        let clamped = local.clamp(-h, h);
        let to_world = |v: Vec3| self.axes[0] * v.x + self.axes[1] * v.y + self.axes[2] * v.z;
        if clamped != local {
            let off = local - clamped;
            let dist = off.length();
            if dist > 0.0 {
                return BodyPoint {
                    point: self.center + to_world(clamped),
                    normal: to_world(off / dist),
                    distance: dist,
                };
            }
        }
        // Inside (or on) the box: the nearest face.
        let mut best = (f32::INFINITY, 0usize, 1.0f32);
        for axis in 0..3 {
            for sign in [-1.0f32, 1.0] {
                let depth = h[axis] - sign * local[axis];
                if depth < best.0 {
                    best = (depth, axis, sign);
                }
            }
        }
        let (depth, axis, sign) = best;
        let n = self.axes[axis] * sign;
        BodyPoint {
            point: p + n * depth,
            normal: n,
            distance: -depth,
        }
    }

    /// Axis-aligned bounds.
    pub fn aabb(&self) -> (Vec3, Vec3) {
        let e = self.axes[0].abs() * self.half.x
            + self.axes[1].abs() * self.half.y
            + self.axes[2].abs() * self.half.z;
        (self.center - e, self.center + e)
    }
}

/// An upright solid cylinder: base centre, radius, height (axis `+z`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Cylinder {
    pub base: Vec3,
    pub radius: f32,
    pub height: f32,
}

impl Cylinder {
    /// Unit horizontal direction from the axis toward `p` (`+x` on the axis).
    pub fn radial(&self, p: Vec3) -> (Vec3, f32) {
        let q = Vec3::new(p.x - self.base.x, p.y - self.base.y, 0.0);
        let rho = q.length();
        if rho > 1.0e-9 {
            (q / rho, rho)
        } else {
            (Vec3::X, 0.0)
        }
    }

    /// The closest surface point to `p`.
    pub fn closest(&self, p: Vec3) -> BodyPoint {
        let (u, rho) = self.radial(p);
        let z = p.z - self.base.z;
        let rc = rho.min(self.radius);
        let zc = z.clamp(0.0, self.height);
        if rho > self.radius || z != zc {
            let point = self.base + u * rc + Vec3::Z * zc;
            let off = p - point;
            let dist = off.length();
            if dist > 0.0 {
                return BodyPoint {
                    point,
                    normal: off / dist,
                    distance: dist,
                };
            }
        }
        let side = self.radius - rho;
        let top = self.height - z;
        let bottom = z;
        let (depth, n) = if side <= top && side <= bottom {
            (side, u)
        } else if top <= bottom {
            (top, Vec3::Z)
        } else {
            (bottom, -Vec3::Z)
        };
        BodyPoint {
            point: p + n * depth,
            normal: n,
            distance: -depth,
        }
    }

    /// Axis-aligned bounds.
    pub fn aabb(&self) -> (Vec3, Vec3) {
        let r = Vec3::new(self.radius, self.radius, 0.0);
        (self.base - r, self.base + r + Vec3::Z * self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_outside_inside_and_rotated() {
        let b = OBox {
            center: Vec3::ZERO,
            axes: [Vec3::X, Vec3::Y, Vec3::Z],
            half: Vec3::new(1.0, 2.0, 3.0),
        };
        let c = b.closest(Vec3::new(3.0, 0.0, 0.0));
        assert_eq!(c.distance, 2.0);
        assert_eq!(c.normal, Vec3::X);
        assert_eq!(c.point, Vec3::new(1.0, 0.0, 0.0));
        let c = b.closest(Vec3::new(0.5, 0.0, 0.0));
        assert_eq!(c.distance, -0.5);
        assert_eq!(c.normal, Vec3::X);
        // Rotated 90° about z: local x is world y.
        let r = OBox {
            center: Vec3::new(1.0, 1.0, 0.0),
            axes: [Vec3::Y, -Vec3::X, Vec3::Z],
            half: Vec3::new(1.0, 2.0, 3.0),
        };
        let c = r.closest(Vec3::new(1.0, 4.0, 0.0));
        assert!((c.distance - 2.0).abs() < 1e-6);
        assert!((c.normal - Vec3::Y).length() < 1e-6);
        let (lo, hi) = r.aabb();
        assert!((lo - Vec3::new(-1.0, 0.0, -3.0)).length() < 1e-6);
        assert!((hi - Vec3::new(3.0, 2.0, 3.0)).length() < 1e-6);
    }

    #[test]
    fn cylinder_side_caps_and_inside() {
        let c = Cylinder {
            base: Vec3::ZERO,
            radius: 1.0,
            height: 2.0,
        };
        let p = c.closest(Vec3::new(0.0, 3.0, 1.0));
        assert_eq!(p.distance, 2.0);
        assert_eq!(p.normal, Vec3::Y);
        let p = c.closest(Vec3::new(0.0, 0.0, 5.0));
        assert_eq!(p.distance, 3.0);
        assert_eq!(p.normal, Vec3::Z);
        let p = c.closest(Vec3::new(0.0, 0.0, -1.0));
        assert_eq!(p.normal, -Vec3::Z);
        let p = c.closest(Vec3::new(0.9, 0.0, 1.0));
        assert!((p.distance + 0.1).abs() < 1e-6);
        assert_eq!(p.normal, Vec3::X);
        let p = c.closest(Vec3::new(0.0, 0.0, 1.9));
        assert_eq!(p.normal, Vec3::Z);
        let p = c.closest(Vec3::new(0.0, 0.0, 0.1));
        assert_eq!(p.normal, -Vec3::Z);
        assert_eq!(c.radial(Vec3::ZERO), (Vec3::X, 0.0));
        let (lo, hi) = c.aabb();
        assert_eq!(
            (lo, hi),
            (Vec3::new(-1.0, -1.0, 0.0), Vec3::new(1.0, 1.0, 2.0))
        );
    }
}
