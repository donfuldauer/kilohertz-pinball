//! Convex collision primitives: signed distance and exact sphere casts.
//!
//! Every primitive is convex, so the gap `f(t) = distance(c + v t) - r` of a
//! linearly moving sphere is a convex function of `t`. Newton's method started
//! at `t = 0` with `f > 0` therefore approaches the first root monotonically
//! from the left and can never step over it: the cast cannot tunnel, needs no
//! bracketing, and converges quadratically (in one step for flat faces).

use glam::{Quat, Vec3};

/// Gap below which a sphere counts as touching (metres).
pub(crate) const TOUCH_EPS: f32 = 1.0e-6;

/// `1 - cos` of the largest angle at which two directions count as parallel
/// (about 1.4 mrad).
const PARALLEL_EPS: f32 = 1.0e-6;

/// A touching sphere counts as approaching only if its closing speed exceeds
/// this fraction of its speed. Velocities that a solver has just made
/// tangential keep a rounding residue of a few 1e-8 of the speed; without the
/// threshold that residue would be reported as an impact at time 0 forever.
pub(crate) const APPROACH_EPS: f32 = 1.0e-5;

/// Maximum Newton iterations of a cast.
const CAST_ITERATIONS: usize = 32;

/// Feature bits of a triangle: vertices a, b, c then edges ab, bc, ca.
pub(crate) const FEATURE_VERTEX: [u8; 3] = [1, 2, 4];
pub(crate) const FEATURE_EDGE: [u8; 3] = [8, 16, 32];
const FEATURE_FACE: u8 = 0;

/// Closest surface point of a primitive to a query point.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Closest {
    /// Signed distance (negative inside a solid).
    pub(crate) distance: f32,
    /// Unit normal at `point`, towards the query point.
    pub(crate) normal: Vec3,
    /// Closest point on the surface.
    pub(crate) point: Vec3,
    /// False when the closest feature is an interior (coplanar) mesh edge or
    /// vertex seen at an angle, where the neighbouring face owns the contact.
    pub(crate) valid: bool,
}

/// A convex primitive in world space.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Primitive {
    Cuboid {
        center: Vec3,
        half_extents: Vec3,
        rotation: Quat,
    },
    Cylinder {
        base: Vec3,
        /// Unit axis.
        axis: Vec3,
        height: f32,
        radius: f32,
    },
    Triangle {
        a: Vec3,
        b: Vec3,
        c: Vec3,
        /// Unit face normal.
        normal: Vec3,
        /// Feature bits whose edge/vertex contacts are real (not interior to
        /// a flat region of the mesh).
        active: u8,
    },
}

/// Any unit vector perpendicular to the unit vector `v`.
pub(crate) fn any_perpendicular(v: Vec3) -> Vec3 {
    let a = v.abs();
    let other = if a.x <= a.y && a.x <= a.z {
        Vec3::X
    } else if a.y <= a.z {
        Vec3::Y
    } else {
        Vec3::Z
    };
    let p = other - v * other.dot(v);
    let len = p.length();
    if len > 0.0 { p / len } else { Vec3::X }
}

impl Primitive {
    /// Axis-aligned bounds as (min, max).
    pub(crate) fn aabb(&self) -> (Vec3, Vec3) {
        match *self {
            Self::Cuboid {
                center,
                half_extents,
                rotation,
            } => {
                let x = (rotation * Vec3::X).abs() * half_extents.x;
                let y = (rotation * Vec3::Y).abs() * half_extents.y;
                let z = (rotation * Vec3::Z).abs() * half_extents.z;
                let e = x + y + z;
                (center - e, center + e)
            }
            Self::Cylinder {
                base,
                axis,
                height,
                radius,
            } => {
                let top = base + axis * height;
                // Conservative: the disc extent is at most `radius` per axis.
                let r = Vec3::splat(radius);
                (base.min(top) - r, base.max(top) + r)
            }
            Self::Triangle { a, b, c, .. } => (a.min(b).min(c), a.max(b).max(c)),
        }
    }

    /// Closest surface point to `p`, with signed distance and normal.
    pub(crate) fn closest(&self, p: Vec3) -> Closest {
        match *self {
            Self::Cuboid {
                center,
                half_extents,
                rotation,
            } => closest_cuboid(center, half_extents, rotation, p),
            Self::Cylinder {
                base,
                axis,
                height,
                radius,
            } => closest_cylinder(base, axis, height, radius, p),
            Self::Triangle {
                a,
                b,
                c,
                normal,
                active,
            } => closest_triangle(a, b, c, normal, active, p),
        }
    }

    /// First time in `[0, max_time]` at which a sphere of `radius` starting at
    /// `center` and moving with `velocity` touches the primitive while
    /// approaching it. Returns (time, closest-point data at that time).
    ///
    /// A sphere that already touches or overlaps at `t = 0` hits at time 0 if
    /// it is moving into the surface and is ignored otherwise (resting,
    /// sliding and separating contacts are the contact query's business).
    pub(crate) fn cast_sphere(
        &self,
        center: Vec3,
        radius: f32,
        velocity: Vec3,
        max_time: f32,
    ) -> Option<(f32, Closest)> {
        let approach = -APPROACH_EPS * velocity.length();
        let mut t = 0.0f32;
        for _ in 0..CAST_ITERATIONS {
            let cl = self.closest(center + velocity * t);
            let gap = cl.distance - radius;
            let rate = cl.normal.dot(velocity);
            if gap <= TOUCH_EPS {
                return (rate < approach && cl.valid).then_some((t, cl));
            }
            if rate >= 0.0 {
                // Convex gap that is not decreasing never reaches zero.
                return None;
            }
            // The Newton step is a lower bound of the remaining time.
            let next = t + gap / -rate;
            if next > max_time || next.is_nan() {
                return None;
            }
            if next == t {
                // Out of floating-point resolution: touching.
                return (rate < approach && cl.valid).then_some((t, cl));
            }
            t = next;
        }
        // Not converged (grazing a curved feature). `t` is still a lower bound
        // of the true impact time, so reporting it is conservative.
        let cl = self.closest(center + velocity * t);
        (cl.valid && cl.normal.dot(velocity) < approach).then_some((t, cl))
    }
}

fn closest_cuboid(center: Vec3, half: Vec3, rotation: Quat, p: Vec3) -> Closest {
    let q = rotation.conjugate() * (p - center);
    let clamped = q.clamp(-half, half);
    let diff = q - clamped;
    let d2 = diff.length_squared();
    let (distance, n_local, s_local) = if d2 > 0.0 {
        let d = d2.sqrt();
        (d, diff / d, clamped)
    } else {
        // Inside or on the surface: leave through the nearest face.
        let room = half - q.abs();
        let (axis, depth) = if room.x <= room.y && room.x <= room.z {
            (0, room.x)
        } else if room.y <= room.z {
            (1, room.y)
        } else {
            (2, room.z)
        };
        let mut n = Vec3::ZERO;
        n[axis] = if q[axis] < 0.0 { -1.0 } else { 1.0 };
        let mut s = q;
        s[axis] = n[axis] * half[axis];
        (-depth, n, s)
    };
    Closest {
        distance,
        normal: rotation * n_local,
        point: center + rotation * s_local,
        valid: true,
    }
}

fn closest_cylinder(base: Vec3, axis: Vec3, height: f32, radius: f32, p: Vec3) -> Closest {
    let w = p - base;
    let h = w.dot(axis);
    let radial = w - axis * h;
    let rho = radial.length();
    let radial_dir = if rho > 1.0e-12 {
        radial / rho
    } else {
        any_perpendicular(axis)
    };
    let d_side = rho - radius;
    let d_cap = (-h).max(h - height);
    if d_side <= 0.0 && d_cap <= 0.0 {
        // Inside or on the surface: leave through the nearest of side / caps.
        if d_side > d_cap {
            return Closest {
                distance: d_side,
                normal: radial_dir,
                point: base + axis * h + radial_dir * radius,
                valid: true,
            };
        }
        let (n, cap_h) = if h - height >= -h {
            (axis, height)
        } else {
            (-axis, 0.0)
        };
        return Closest {
            distance: d_cap,
            normal: n,
            point: base + axis * cap_h + radial,
            valid: true,
        };
    }
    let point = base + axis * h.clamp(0.0, height) + radial_dir * rho.min(radius);
    let diff = p - point;
    let d = diff.length();
    let normal = if d > 0.0 {
        diff / d
    } else if d_side >= d_cap {
        radial_dir
    } else if h > 0.5 * height {
        axis
    } else {
        -axis
    };
    Closest {
        distance: d,
        normal,
        point,
        valid: true,
    }
}

/// Closest point on a triangle (Ericson, *Real-Time Collision Detection*
/// §5.1.5) with the Voronoi feature it lies on.
fn closest_on_triangle(a: Vec3, b: Vec3, c: Vec3, p: Vec3) -> (Vec3, u8) {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, FEATURE_VERTEX[0]);
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return (b, FEATURE_VERTEX[1]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (a + ab * v, FEATURE_EDGE[0]);
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return (c, FEATURE_VERTEX[2]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (a + ac * w, FEATURE_EDGE[2]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (b + (c - b) * w, FEATURE_EDGE[1]);
    }
    let sum = va + vb + vc;
    if sum > 0.0 {
        let denom = 1.0 / sum;
        (a + ab * (vb * denom) + ac * (vc * denom), FEATURE_FACE)
    } else {
        // Numerically degenerate; any vertex is within rounding of closest.
        (a, FEATURE_VERTEX[0])
    }
}

fn closest_triangle(a: Vec3, b: Vec3, c: Vec3, face_normal: Vec3, active: u8, p: Vec3) -> Closest {
    let (point, feature) = closest_on_triangle(a, b, c, p);
    if feature == FEATURE_FACE {
        // Over the face the plane gives distance and normal exactly, without
        // the rounding of the barycentric reconstruction of `point`.
        // The difference is taken in f64: for a large triangle `p - a` would
        // otherwise round by a micrometre or more (the vertex can be tens of
        // metres away), which is the scale of the contact skin.
        let height = (p - a).dot(face_normal);
        let normal = if height < 0.0 {
            -face_normal
        } else {
            face_normal
        };
        return Closest {
            distance: height.abs(),
            normal,
            point: p - face_normal * height,
            valid: true,
        };
    }
    let diff = p - point;
    let d = diff.length();
    let normal = if d > 1.0e-12 {
        diff / d
    } else if (p - a).dot(face_normal) < 0.0 {
        -face_normal
    } else {
        face_normal
    };
    // An interior edge/vertex of a flat mesh region only counts when the
    // query point is straight above it, and then with the face normal; at an
    // angle the neighbouring face owns the contact. This prevents ghost
    // collisions on tessellated flats.
    let (normal, valid) = if active & feature != 0 {
        (normal, true)
    } else {
        let along = normal.dot(face_normal);
        if along.abs() > 1.0 - PARALLEL_EPS {
            (
                if along < 0.0 {
                    -face_normal
                } else {
                    face_normal
                },
                true,
            )
        } else {
            (normal, false)
        }
    };
    Closest {
        distance: d,
        normal,
        point,
        valid,
    }
}

/// Whether two unit normals are parallel or anti-parallel within tolerance.
pub(crate) fn normals_parallel(n1: Vec3, n2: Vec3) -> bool {
    n1.dot(n2).abs() > 1.0 - PARALLEL_EPS
}
