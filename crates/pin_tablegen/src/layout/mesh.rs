//! Triangle meshes in metres (table frame: `x` across, `y` up the table,
//! `z` the playfield normal) and the shapes the generator builds from them.

use pin_physics::world::CubicSpline;

/// Largest distance (m) between a curved surface and the flat facets that
/// approximate it (posts, circular inserts, sampled spline walls). Well
/// inside the 0.1 mm visual/collision agreement of PLAN 4.3.
pub const FACET_TOLERANCE_M: f64 = 2.0e-5;

/// An indexed triangle list with per-vertex normals.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    /// Positions, m.
    pub positions: Vec<[f32; 3]>,
    /// Unit normals.
    pub normals: Vec<[f32; 3]>,
    /// Triangle list.
    pub indices: Vec<u32>,
}

impl Mesh {
    /// Appends a vertex and returns its index.
    pub fn vertex(&mut self, p: [f32; 3], n: [f32; 3]) -> u32 {
        self.positions.push(p);
        self.normals.push(n);
        (self.positions.len() - 1) as u32
    }

    /// Appends a flat quad `a b c d` (counter-clockwise seen from the side
    /// its normal points to) as two triangles with their own vertices.
    pub fn quad(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) {
        let n = face_normal(a, b, c, d);
        let i = [a, b, c, d].map(|p| self.vertex(p, n));
        self.indices
            .extend_from_slice(&[i[0], i[1], i[2], i[0], i[2], i[3]]);
    }

    /// Appends a flat polygon at height `z` (m) triangulated by `tris`
    /// (indices into `outline`), normal +Z.
    pub fn flat(&mut self, outline: &[[f64; 2]], tris: &[[usize; 3]], z: f32) {
        let base = self.positions.len() as u32;
        for p in outline {
            self.vertex([p[0] as f32, p[1] as f32, z], [0.0, 0.0, 1.0]);
        }
        for t in tris {
            self.indices
                .extend(t.iter().map(|&k| base + k as u32));
        }
    }

    /// Number of triangles.
    #[cfg(test)]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f64; 3] {
    [
        f64::from(a[0]) - f64::from(b[0]),
        f64::from(a[1]) - f64::from(b[1]),
        f64::from(a[2]) - f64::from(b[2]),
    ]
}

fn cross(u: [f64; 3], v: [f64; 3]) -> [f64; 3] {
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}

fn unit(v: [f64; 3]) -> [f32; 3] {
    let len = libm::sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
    if len > 0.0 && len.is_finite() {
        [(v[0] / len) as f32, (v[1] / len) as f32, (v[2] / len) as f32]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// Normal of the quad `a b c d` (Newell's method over its diagonals).
fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) -> [f32; 3] {
    unit(cross(sub(c, a), sub(d, b)))
}

/// Sides of a regular polygon inscribed in a circle of radius `r` (m) whose
/// edges stay within [`FACET_TOLERANCE_M`] of the circle; a multiple of 4,
/// between 16 and 1024.
pub fn circle_sides(r: f64) -> usize {
    let c = 1.0 - FACET_TOLERANCE_M / r;
    let n = if c <= -1.0 || !(c.is_finite()) {
        16.0
    } else {
        core::f64::consts::PI / libm::acos(c.clamp(-1.0, 1.0))
    };
    let n = if n.is_finite() { n.ceil() as usize } else { 1024 };
    (n.clamp(16, 1024)).div_ceil(4) * 4
}

/// Unit vector at angle `2π k / n`.
pub fn ring(k: usize, n: usize) -> [f64; 2] {
    let a = core::f64::consts::TAU * k as f64 / n as f64;
    [libm::cos(a), libm::sin(a)]
}

/// A vertical post: an inscribed `n`-gon prism of radius `r` from `z0` to
/// `z1` (m), sides with radial normals, closed by a top cap (the bottom
/// stands on the surface below).
pub fn post(center: [f64; 2], r: f64, z0: f32, z1: f32) -> Mesh {
    let n = circle_sides(r);
    let mut m = Mesh::default();
    let rim: Vec<[f64; 2]> = (0..n)
        .map(|k| {
            let u = ring(k, n);
            [center[0] + r * u[0], center[1] + r * u[1]]
        })
        .collect();
    let normals: Vec<[f32; 3]> = (0..n)
        .map(|k| {
            let u = ring(k, n);
            [u[0] as f32, u[1] as f32, 0.0]
        })
        .collect();
    let base = m.positions.len() as u32;
    for (p, nrm) in rim.iter().zip(&normals) {
        m.vertex([p[0] as f32, p[1] as f32, z0], *nrm);
        m.vertex([p[0] as f32, p[1] as f32, z1], *nrm);
    }
    for k in 0..n as u32 {
        let j = (k + 1) % n as u32;
        let (b0, t0, b1, t1) = (base + 2 * k, base + 2 * k + 1, base + 2 * j, base + 2 * j + 1);
        m.indices.extend_from_slice(&[b0, b1, t1, b0, t1, t0]);
    }
    let fan: Vec<[usize; 3]> = (1..n - 1).map(|k| [0, k, k + 1]).collect();
    m.flat(&rim, &fan, z1);
    m
}

/// A vertical ribbon (zero-thickness wall) over the plan-view polyline
/// `points` (m) from `z0` to `z1`; `closed` adds the closing segment. Every
/// corner position is computed once, so neighbouring quads share it bit for
/// bit (the physics welds trimesh vertices by position).
pub fn ribbon(points: &[[f32; 3]], closed: bool, height: f32) -> Mesh {
    let mut m = Mesh::default();
    let tops: Vec<[f32; 3]> = points
        .iter()
        .map(|p| [p[0], p[1], p[2] + height])
        .collect();
    let n = points.len();
    let segments = if closed { n } else { n.saturating_sub(1) };
    for i in 0..segments {
        let j = (i + 1) % n;
        m.quad(points[i], points[j], tops[j], tops[i]);
    }
    m
}

/// Samples of a spline (m) such that the polyline through them stays within
/// [`FACET_TOLERANCE_M`] of the curve: per Bézier segment, `k` uniform steps
/// with `max|B''| / (8 k²) ≤ tolerance`. Includes both ends (a closed
/// spline's last sample repeats its first point exactly).
pub fn spline_samples(spline: &CubicSpline) -> Vec<[f32; 3]> {
    let mut out = Vec::new();
    let segments = spline.segments();
    for (i, seg) in segments.iter().enumerate() {
        let [p0, p1, p2, p3] = seg.p;
        let a = (p0 - p1 * 2.0 + p2).as_dvec3().length();
        let b = (p1 - p2 * 2.0 + p3).as_dvec3().length();
        let bound = 6.0 * a.max(b);
        let k = libm::sqrt(bound / (8.0 * FACET_TOLERANCE_M)).ceil();
        let k = if k.is_finite() { (k as usize).clamp(1, 4096) } else { 4096 };
        if i == 0 {
            out.push(p0.to_array());
        }
        for j in 1..k {
            out.push(seg.point(j as f32 / k as f32).to_array());
        }
        out.push(p3.to_array());
    }
    out
}

/// A tube of radius `r` (m) around a polyline with per-sample unit frame
/// vectors `(u, v)` perpendicular to it; `sides` around.
pub fn tube(center: &[[f32; 3]], frames: &[([f32; 3], [f32; 3])], r: f32, sides: usize) -> Mesh {
    let mut m = Mesh::default();
    for (c, (u, v)) in center.iter().zip(frames) {
        for k in 0..sides {
            let w = ring(k, sides);
            let (cu, sv) = (w[0] as f32, w[1] as f32);
            let n = [
                u[0] * cu + v[0] * sv,
                u[1] * cu + v[1] * sv,
                u[2] * cu + v[2] * sv,
            ];
            m.vertex([c[0] + r * n[0], c[1] + r * n[1], c[2] + r * n[2]], n);
        }
    }
    let s = sides as u32;
    for i in 0..center.len().saturating_sub(1) as u32 {
        for k in 0..s {
            let j = (k + 1) % s;
            let (a, b, c, d) = (i * s + k, i * s + j, (i + 1) * s + j, (i + 1) * s + k);
            m.indices.extend_from_slice(&[a, b, c, a, c, d]);
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    #[test]
    fn circle_sides_meet_the_tolerance() {
        for r in [1.0e-3, 5.0e-3, 0.02, 0.1, 1.0] {
            let n = circle_sides(r);
            assert_eq!(n % 4, 0);
            let sag = r * (1.0 - (core::f64::consts::PI / n as f64).cos());
            assert!(sag <= FACET_TOLERANCE_M * 1.0001 || n == 1024, "{r} {n} {sag}");
        }
        assert_eq!(circle_sides(1.0e-9), 16);
    }

    #[test]
    fn post_is_closed_on_top() {
        let m = post([0.0, 0.0], 0.005, 0.0, 0.028);
        let n = circle_sides(0.005);
        assert_eq!(m.triangle_count(), 2 * n + n - 2);
        assert!(m.positions.iter().all(|p| p[2] == 0.0 || p[2] == 0.028));
    }

    #[test]
    fn ribbon_shares_corners() {
        let pts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0]];
        let open = ribbon(&pts, false, 0.1);
        assert_eq!(open.triangle_count(), 4);
        let closed = ribbon(&pts, true, 0.1);
        assert_eq!(closed.triangle_count(), 6);
        assert_eq!(closed.normals[0], [0.0, -1.0, 0.0]);
    }

    #[test]
    fn spline_samples_follow_the_curve() {
        let pts: Vec<Vec3> = (0..9)
            .map(|i| {
                let a = i as f32 * 0.4;
                Vec3::new(0.1 * a.cos(), 0.1 * a.sin(), 0.0)
            })
            .collect();
        let s = CubicSpline::interpolate(&pts).unwrap();
        let samples = spline_samples(&s);
        assert_eq!(samples[0], pts[0].to_array());
        assert_eq!(*samples.last().unwrap(), pts[8].to_array());
        for w in samples.windows(2) {
            let mid = (Vec3::from_array(w[0]) + Vec3::from_array(w[1])) * 0.5;
            let c = s.closest_point(mid, None);
            assert!(c.distance < 3.0e-5, "{}", c.distance);
        }
    }

    #[test]
    fn tube_has_closed_rings() {
        let c = [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let f = [([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]); 2];
        let m = tube(&c, &f, 0.01, 6);
        assert_eq!(m.positions.len(), 12);
        assert_eq!(m.triangle_count(), 12);
    }
}
