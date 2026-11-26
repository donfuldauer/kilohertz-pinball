//! Mesh building for the cabinet model: points in the cabinet frame in
//! millimetres (`f64`), closed pieces with outward, counter-clockwise faces
//! and flat per-face normals. [`MeshMm::to_glb`] converts to metres (`f32`)
//! and adds the per-face box-projected UVs.

use super::glb::Mesh;
use pin_core::fmath::{max_f64, min_f64};

/// A point, mm.
pub(crate) type P3 = [f64; 3];

/// One of the six cabinet-frame axis directions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Axis {
    /// `+x`.
    PosX,
    /// `-x`.
    NegX,
    /// `+y`.
    PosY,
    /// `-y`.
    NegY,
    /// `+z`.
    PosZ,
}

impl Axis {
    /// Unit direction.
    pub(crate) fn dir(self) -> P3 {
        match self {
            Axis::PosX => [1.0, 0.0, 0.0],
            Axis::NegX => [-1.0, 0.0, 0.0],
            Axis::PosY => [0.0, 1.0, 0.0],
            Axis::NegY => [0.0, -1.0, 0.0],
            Axis::PosZ => [0.0, 0.0, 1.0],
        }
    }

    /// Two unit vectors `(u, v)` across the axis with `u x v = dir`, so a
    /// ring `cos(t) u + sin(t) v` runs counter-clockwise about the axis.
    pub(crate) fn basis(self) -> (P3, P3) {
        match self {
            Axis::PosX => ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            Axis::NegX => ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
            Axis::PosY => ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            Axis::NegY => ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            Axis::PosZ => ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
        }
    }

    /// The glTF extras label (`"+x"`, `"-y"`, ...).
    pub(crate) fn label(self) -> &'static str {
        match self {
            Axis::PosX => "+x",
            Axis::NegX => "-x",
            Axis::PosY => "+y",
            Axis::NegY => "-y",
            Axis::PosZ => "+z",
        }
    }

    /// The opposite direction (`+z` has no use for one here and maps to
    /// itself).
    pub(crate) fn opposite(self) -> Axis {
        match self {
            Axis::PosX => Axis::NegX,
            Axis::NegX => Axis::PosX,
            Axis::PosY => Axis::NegY,
            Axis::NegY => Axis::PosY,
            Axis::PosZ => Axis::PosZ,
        }
    }
}

#[cfg(test)]
fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `origin + s * a + t * b + w * c`, evaluated term by term (no FMA).
pub(crate) fn offset(origin: P3, terms: &[(f64, P3)]) -> P3 {
    let mut p = origin;
    for &(s, d) in terms {
        for k in 0..3 {
            p[k] += s * d[k];
        }
    }
    p
}

/// Newell normal of a polygon, unit length (`+z` for a degenerate one).
fn newell(points: &[P3]) -> P3 {
    let mut n = [0.0f64; 3];
    for (i, a) in points.iter().enumerate() {
        let b = points[(i + 1) % points.len()];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let len = libm::sqrt(n[0] * n[0] + n[1] * n[1] + n[2] * n[2]);
    if len > 1e-12 && len.is_finite() {
        [n[0] / len, n[1] / len, n[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Ring `k` of `n` points on a circle of radius `r` about `centre` in the
/// plane across `axis`, counter-clockwise about the axis.
pub(crate) fn circle(centre: P3, axis: Axis, r: f64, n: usize) -> Vec<P3> {
    let (u, v) = axis.basis();
    (0..n)
        .map(|k| {
            let t = 2.0 * core::f64::consts::PI * k as f64 / n as f64;
            offset(centre, &[(r * libm::cos(t), u), (r * libm::sin(t), v)])
        })
        .collect()
}

/// Removes consecutive duplicate points of a closed 2-D profile and makes it
/// counter-clockwise (positive signed area).
pub(crate) fn ccw_profile(profile: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(profile.len());
    for &p in profile {
        let dup = out
            .last()
            .is_some_and(|q| (q[0] - p[0]).abs() < 1e-9 && (q[1] - p[1]).abs() < 1e-9);
        if !dup {
            out.push(p);
        }
    }
    while out.len() > 1 {
        let (f, l) = (out[0], out[out.len() - 1]);
        if (f[0] - l[0]).abs() < 1e-9 && (f[1] - l[1]).abs() < 1e-9 {
            out.pop();
        } else {
            break;
        }
    }
    let mut area = 0.0;
    for (i, a) in out.iter().enumerate() {
        let b = out[(i + 1) % out.len()];
        area += a[0] * b[1] - b[0] * a[1];
    }
    if area < 0.0 {
        out.reverse();
    }
    out
}

/// An indexed triangle mesh in millimetres with flat per-face normals.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct MeshMm {
    /// Positions, mm.
    pub positions: Vec<P3>,
    /// Unit normals, one per position.
    pub normals: Vec<P3>,
    /// Triangle list, counter-clockwise front faces.
    pub indices: Vec<u32>,
    /// Explicit UVs, one per position (empty: the box projection of
    /// [`MeshMm::to_glb`]). Set by [`MeshMm::set_uvs`].
    pub uvs: Vec<[f64; 2]>,
}

impl MeshMm {
    /// Adds a polygon (counter-clockwise seen from its front). Up to four
    /// points are fanned from the first one; larger (star-shaped) polygons
    /// from their centroid.
    pub(crate) fn polygon(&mut self, points: &[P3]) {
        if points.len() < 3 {
            return;
        }
        let n = newell(points);
        let base = self.positions.len() as u32;
        if points.len() <= 4 {
            for &p in points {
                self.positions.push(p);
                self.normals.push(n);
            }
            for i in 1..points.len() as u32 - 1 {
                self.indices
                    .extend_from_slice(&[base, base + i, base + i + 1]);
            }
        } else {
            let k = points.len() as f64;
            let mut c = [0.0; 3];
            for p in points {
                for j in 0..3 {
                    c[j] += p[j] / k;
                }
            }
            self.positions.push(c);
            self.normals.push(n);
            for &p in points {
                self.positions.push(p);
                self.normals.push(n);
            }
            let m = points.len() as u32;
            for i in 0..m {
                let a = base + 1 + i;
                let b = base + 1 + (i + 1) % m;
                self.indices.extend_from_slice(&[base, a, b]);
            }
        }
    }

    /// A closed loft through `rings` (all the same length, each
    /// counter-clockwise about the direction from the first ring to the
    /// last), capped at both ends.
    pub(crate) fn loft(&mut self, rings: &[Vec<P3>]) {
        let Some(first) = rings.first() else {
            return;
        };
        let n = first.len();
        if n < 3 || rings.iter().any(|r| r.len() != n) {
            return;
        }
        for w in rings.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            for k in 0..n {
                let k1 = (k + 1) % n;
                self.polygon(&[a[k], a[k1], b[k1], b[k]]);
            }
        }
        let mut start = first.clone();
        start.reverse();
        self.polygon(&start);
        if let Some(last) = rings.last() {
            self.polygon(last);
        }
    }

    /// A solid of revolution about `axis` through `origin`: `profile` is
    /// `(s along the axis, radius)` pairs, `segments` points per ring.
    pub(crate) fn lathe(
        &mut self,
        origin: P3,
        axis: Axis,
        profile: &[(f64, f64)],
        segments: usize,
    ) {
        let d = axis.dir();
        let rings: Vec<Vec<P3>> = profile
            .iter()
            .map(|&(s, r)| circle(offset(origin, &[(s, d)]), axis, r, segments))
            .collect();
        self.loft(&rings);
    }

    /// A six-faced solid between the quads `near` and `far`, each given
    /// counter-clockwise about the direction from `near` to `far`.
    pub(crate) fn hexa(&mut self, near: [P3; 4], far: [P3; 4]) {
        self.loft(&[near.to_vec(), far.to_vec()]);
    }

    /// An axis-aligned box `lo..hi`.
    pub(crate) fn aabb(&mut self, lo: P3, hi: P3) {
        let near = [
            [lo[0], lo[1], lo[2]],
            [hi[0], lo[1], lo[2]],
            [hi[0], hi[1], lo[2]],
            [lo[0], hi[1], lo[2]],
        ];
        let far = near.map(|p| [p[0], p[1], hi[2]]);
        self.hexa(near, far);
    }

    /// A prism along `+y` from `y0` to `y1` of the (x, z) profile, with the
    /// profile's z measured from `base(y)` (a sheared extrusion that follows
    /// a sloping line, such as the side wall tops).
    pub(crate) fn extrude_y(
        &mut self,
        profile_xz: &[[f64; 2]],
        y0: f64,
        y1: f64,
        base: impl Fn(f64) -> f64,
    ) {
        // CCW in (x, z) is clockwise about +y (x cross z = -y): reverse.
        let mut p = ccw_profile(profile_xz);
        p.reverse();
        let ring = |y: f64| -> Vec<P3> { p.iter().map(|q| [q[0], y, base(y) + q[1]]).collect() };
        self.loft(&[ring(y0), ring(y1)]);
    }

    /// A prism along `+x` from `x0` to `x1` of the (y, z) profile.
    pub(crate) fn extrude_x(&mut self, profile_yz: &[[f64; 2]], x0: f64, x1: f64) {
        let p = ccw_profile(profile_yz);
        let ring = |x: f64| -> Vec<P3> { p.iter().map(|q| [x, q[0], q[1]]).collect() };
        self.loft(&[ring(x0), ring(x1)]);
    }

    /// Replaces the UVs of every vertex with `f(position)`, clamped to
    /// `[0, 1]` (a texture-mapped quad, such as the translite or the score
    /// display, maps its image exactly instead of by the box projection).
    pub(crate) fn set_uvs(&mut self, f: impl Fn(P3) -> [f64; 2]) {
        self.uvs = self
            .positions
            .iter()
            .map(|&p| f(p).map(|c| max_f64(0.0, min_f64(1.0, c))))
            .collect();
    }

    /// `true` when the mesh has no triangle.
    pub(crate) fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// Converts to metres (`f32`) with UVs: the explicit UVs when set
    /// ([`MeshMm::set_uvs`]), otherwise each vertex is projected on the
    /// node's bounding box along its face normal's dominant axis, so every
    /// face maps into `[0, 1]` and a box face covers it whole; `u` runs to
    /// the viewer's right seen from outside the face, `v` runs down (glTF
    /// image rows; the top of an image is at the top of a side face).
    pub(crate) fn to_glb(&self) -> Mesh {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for p in &self.positions {
            for k in 0..3 {
                lo[k] = min_f64(lo[k], p[k]);
                hi[k] = max_f64(hi[k], p[k]);
            }
        }
        let explicit = self.uvs.len() == self.positions.len() && !self.uvs.is_empty();
        let uvs = if explicit {
            self.uvs.iter().map(|uv| uv.map(|c| c as f32)).collect()
        } else {
            self.box_uvs(lo, hi)
        };
        Mesh {
            positions: self
                .positions
                .iter()
                .map(|p| p.map(|c| (c / 1000.0) as f32))
                .collect(),
            normals: self.normals.iter().map(|n| n.map(|c| c as f32)).collect(),
            uvs,
            indices: self.indices.clone(),
        }
    }

    /// The box-projected UVs on the bounds `lo..hi` (see [`MeshMm::to_glb`]).
    fn box_uvs(&self, lo: P3, hi: P3) -> Vec<[f32; 2]> {
        let frac = |p: &P3, k: usize| -> f64 {
            let ext = hi[k] - lo[k];
            if ext > 1e-9 {
                max_f64(0.0, min_f64(1.0, (p[k] - lo[k]) / ext))
            } else {
                0.5
            }
        };
        self.positions
            .iter()
            .zip(&self.normals)
            .map(|(p, n)| {
                let (ax, ay, az) = (n[0].abs(), n[1].abs(), n[2].abs());
                let (u, v) = if ax >= ay && ax >= az {
                    let fy = frac(p, 1);
                    (if n[0] >= 0.0 { fy } else { 1.0 - fy }, 1.0 - frac(p, 2))
                } else if ay >= az {
                    let fx = frac(p, 0);
                    (if n[1] >= 0.0 { 1.0 - fx } else { fx }, 1.0 - frac(p, 2))
                } else {
                    let fy = frac(p, 1);
                    (frac(p, 0), if n[2] >= 0.0 { 1.0 - fy } else { fy })
                };
                [u as f32, v as f32]
            })
            .collect()
    }

    /// Signed enclosed volume, mm^3 (positive when every closed piece faces
    /// outward).
    #[cfg(test)]
    pub(crate) fn signed_volume(&self) -> f64 {
        let mut v = 0.0;
        for t in self.indices.chunks_exact(3) {
            let a = self.positions[t[0] as usize];
            let b = self.positions[t[1] as usize];
            let c = self.positions[t[2] as usize];
            v += a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]);
        }
        v / 6.0
    }

    /// Checks that each triangle's winding agrees with its stored normal.
    #[cfg(test)]
    pub(crate) fn windings_match_normals(&self) -> bool {
        self.indices.chunks_exact(3).all(|t| {
            let a = self.positions[t[0] as usize];
            let b = self.positions[t[1] as usize];
            let c = self.positions[t[2] as usize];
            let (e1, e2) = (sub(b, a), sub(c, a));
            let cr = [
                e1[1] * e2[2] - e1[2] * e2[1],
                e1[2] * e2[0] - e1[0] * e2[2],
                e1[0] * e2[1] - e1[1] * e2[0],
            ];
            let n = self.normals[t[0] as usize];
            let area2 = cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2];
            area2 < 1e-12 || cr[0] * n[0] + cr[1] * n[1] + cr[2] * n[2] > 0.0
        })
    }
}
