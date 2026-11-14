//! Geometry helpers of the clearance validator (setup-time, `f64`).

use glam::{DVec2, DVec3, Vec3};
use pin_assets::GltfScene;
use pin_physics::world::CubicSpline;

/// `|n·z|` at and above which a triangle is walkable (within 45° of level).
pub(super) const WALKABLE_NZ: f64 = core::f64::consts::FRAC_1_SQRT_2;
/// `|n·z|` at and above which a triangle can be a ceiling (within 60° of level).
pub(super) const CEILING_NZ: f64 = 0.5;

/// One collider triangle of the playfield scene, in metres.
#[derive(Clone, Debug)]
pub(super) struct Tri {
    pub p: [DVec3; 3],
    /// Unit normal (sign arbitrary: meshes are two-sided).
    pub n: DVec3,
    pub layer: u8,
    /// Index into `GltfScene::meshes`.
    pub mesh: usize,
    pub min: DVec3,
    pub max: DVec3,
}

impl Tri {
    fn new(p: [DVec3; 3], layer: u8, mesh: usize) -> Option<Tri> {
        let n = (p[1] - p[0]).cross(p[2] - p[0]);
        let len = n.length();
        if !(len.is_finite() && len > 1.0e-14) {
            return None;
        }
        Some(Tri {
            p,
            n: n / len,
            layer,
            mesh,
            min: p[0].min(p[1]).min(p[2]),
            max: p[0].max(p[1]).max(p[2]),
        })
    }

    /// `|n·z|`.
    pub fn nz(&self) -> f64 {
        self.n.z.abs()
    }

    /// Height of the triangle's plane above plan point `q` (non-steep
    /// triangles only).
    pub fn z_at(&self, q: DVec2) -> f64 {
        let a = self.p[0];
        a.z - (self.n.x * (q.x - a.x) + self.n.y * (q.y - a.y)) / self.n.z
    }

    /// Whether plan point `q` lies in the triangle's plan projection
    /// (inclusive, with a small tolerance).
    pub fn contains_plan(&self, q: DVec2) -> bool {
        let [a, b, c] = self.p.map(|v| v.truncate());
        let d1 = cross2(b - a, q - a);
        let d2 = cross2(c - b, q - b);
        let d3 = cross2(a - c, q - c);
        let scale = (b - a).length_squared().max((c - a).length_squared());
        let eps = -1.0e-9 * scale;
        (d1 >= eps && d2 >= eps && d3 >= eps) || (d1 <= -eps && d2 <= -eps && d3 <= -eps)
    }

    /// Distance from `q` to the triangle.
    pub fn distance(&self, q: DVec3) -> f64 {
        q.distance(closest_on_triangle(q, self.p[0], self.p[1], self.p[2]))
    }

    /// Closest point of the triangle to `q`.
    pub fn closest(&self, q: DVec3) -> DVec3 {
        closest_on_triangle(q, self.p[0], self.p[1], self.p[2])
    }
}

fn cross2(a: DVec2, b: DVec2) -> f64 {
    a.x * b.y - a.y * b.x
}

/// Closest point of triangle `abc` to `p` (Ericson, *Real-Time Collision
/// Detection* 5.1.5).
pub(super) fn closest_on_triangle(p: DVec3, a: DVec3, b: DVec3, c: DVec3) -> DVec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

/// Every collider triangle of `scene` in world space (node transform
/// applied; default layer 0, as the engine builds them).
pub(super) fn collider_triangles(scene: &GltfScene) -> Vec<Tri> {
    let mut out = Vec::new();
    for (mi, mesh) in scene.meshes.iter().enumerate() {
        if !mesh.extras.collider {
            continue;
        }
        let m = &mesh.transform;
        let tf = |p: [f32; 3]| -> DVec3 {
            let mut o = [0.0f64; 3];
            for (row, out) in o.iter_mut().enumerate() {
                *out = f64::from(m[0][row]) * f64::from(p[0])
                    + f64::from(m[1][row]) * f64::from(p[1])
                    + f64::from(m[2][row]) * f64::from(p[2])
                    + f64::from(m[3][row]);
            }
            DVec3::from_array(o)
        };
        let layer = mesh.extras.collision_layer.unwrap_or(0);
        for t in mesh.indices.chunks_exact(3) {
            let get = |i: u32| mesh.positions.get(i as usize).copied();
            let (Some(a), Some(b), Some(c)) = (get(t[0]), get(t[1]), get(t[2])) else {
                continue;
            };
            let p = [tf(a), tf(b), tf(c)];
            if p.iter().all(|v| v.is_finite())
                && let Some(tri) = Tri::new(p, layer, mi)
            {
                out.push(tri);
            }
        }
    }
    out
}

/// Uniform plan-view grid over triangle bounding boxes.
pub(super) struct TriGrid {
    origin: DVec2,
    cell: f64,
    nx: usize,
    ny: usize,
    cells: Vec<Vec<u32>>,
}

/// Most cells along one axis of a [`TriGrid`].
const MAX_GRID_SIDE: f64 = 256.0;

impl TriGrid {
    pub fn new(tris: &[Tri]) -> TriGrid {
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        for t in tris {
            lo = lo.min(t.min.truncate());
            hi = hi.max(t.max.truncate());
        }
        if tris.is_empty() {
            lo = DVec2::ZERO;
            hi = DVec2::ZERO;
        }
        let extent = (hi - lo).max_element();
        let cell = (extent / MAX_GRID_SIDE).max(0.05);
        let nx = (((hi.x - lo.x) / cell).floor() as usize + 1).max(1);
        let ny = (((hi.y - lo.y) / cell).floor() as usize + 1).max(1);
        let mut grid = TriGrid {
            origin: lo,
            cell,
            nx,
            ny,
            cells: vec![Vec::new(); nx * ny],
        };
        for (i, t) in tris.iter().enumerate() {
            let (x0, y0, x1, y1) = grid.range(t.min.truncate(), t.max.truncate());
            for y in y0..=y1 {
                for x in x0..=x1 {
                    grid.cells[y * nx + x].push(i as u32);
                }
            }
        }
        grid
    }

    fn range(&self, lo: DVec2, hi: DVec2) -> (usize, usize, usize, usize) {
        let idx = |v: f64, n: usize| -> usize {
            let k = (v / self.cell).floor();
            if k.is_nan() || k < 0.0 {
                0
            } else {
                (k as usize).min(n - 1)
            }
        };
        let a = lo - self.origin;
        let b = hi - self.origin;
        (
            idx(a.x, self.nx),
            idx(a.y, self.ny),
            idx(b.x, self.nx),
            idx(b.y, self.ny),
        )
    }

    /// Indices of the triangles whose plan bounding boxes may overlap the
    /// box `lo..hi`, ascending and without duplicates (into `out`).
    pub fn query(&self, tris: &[Tri], lo: DVec2, hi: DVec2, out: &mut Vec<u32>) {
        out.clear();
        let (x0, y0, x1, y1) = self.range(lo, hi);
        for y in y0..=y1 {
            for x in x0..=x1 {
                out.extend_from_slice(&self.cells[y * self.nx + x]);
            }
        }
        out.sort_unstable();
        out.dedup();
        out.retain(|&i| {
            let t = &tris[i as usize];
            t.min.x <= hi.x && t.max.x >= lo.x && t.min.y <= hi.y && t.max.y >= lo.y
        });
    }
}

/// A plan-view polygon (metres), even-odd rule.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Polygon {
    pub points: Vec<DVec2>,
}

impl Polygon {
    pub fn contains(&self, q: DVec2) -> bool {
        let pts = &self.points;
        let mut inside = false;
        let mut j = pts.len() - 1;
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[j]);
            if (a.y > q.y) != (b.y > q.y) {
                let x = a.x + (q.y - a.y) * (b.x - a.x) / (b.y - a.y);
                if q.x < x {
                    inside = !inside;
                }
            }
            j = i;
        }
        inside
    }

    /// Twice the signed area.
    pub fn area2(&self) -> f64 {
        let pts = &self.points;
        let mut s = 0.0;
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            s += cross2(a, b);
        }
        s
    }

    /// Whether the polygon and the axis-aligned box `lo..hi` may overlap
    /// (any vertex of one inside the other, or crossing edges).
    pub fn overlaps_box(&self, lo: DVec2, hi: DVec2) -> bool {
        let inside_box = |p: DVec2| p.x >= lo.x && p.x <= hi.x && p.y >= lo.y && p.y <= hi.y;
        if self.points.iter().any(|&p| inside_box(p)) {
            return true;
        }
        let corners = [lo, DVec2::new(hi.x, lo.y), hi, DVec2::new(lo.x, hi.y)];
        if corners.iter().any(|&c| self.contains(c)) {
            return true;
        }
        let n = self.points.len();
        (0..n).any(|i| {
            let (a, b) = (self.points[i], self.points[(i + 1) % n]);
            (0..4).any(|k| segments_cross(a, b, corners[k], corners[(k + 1) % 4]))
        })
    }
}

fn segments_cross(a: DVec2, b: DVec2, c: DVec2, d: DVec2) -> bool {
    let o = |p: DVec2, q: DVec2, r: DVec2| cross2(q - p, r - p);
    let (d1, d2, d3, d4) = (o(c, d, a), o(c, d, b), o(a, b, c), o(a, b, d));
    ((d1 > 0.0) != (d2 > 0.0)) && ((d3 > 0.0) != (d4 > 0.0))
}

/// The table's spline through `points` (open or closed), as the loader
/// builds it.
pub(super) fn build_spline(points: &[Vec3], closed: bool) -> Option<CubicSpline> {
    if closed {
        CubicSpline::interpolate_closed(points).ok()
    } else {
        CubicSpline::interpolate(points).ok()
    }
}

/// Parameter values along `s`, at most `step` metres apart (estimated
/// from each segment's control polygon), including both ends.
pub(super) fn sample_params(s: &CubicSpline, step: f64) -> Vec<f32> {
    let mut out = Vec::new();
    for (i, seg) in s.segments().iter().enumerate() {
        let len: f64 = seg
            .p
            .windows(2)
            .map(|w| f64::from(w[0].distance(w[1])))
            .sum();
        let k = ((len / step).ceil() as usize).clamp(1, 4096);
        for j in 0..k {
            out.push(i as f32 + j as f32 / k as f32);
        }
    }
    out.push(s.max_t());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> DVec3 {
        DVec3::new(x, y, z)
    }

    #[test]
    fn closest_point_regions() {
        let (a, b, c) = (v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0));
        let cases = [
            (v(-1.0, -1.0, 0.0), a),
            (v(2.0, -0.5, 0.0), b),
            (v(-0.5, 2.0, 0.0), c),
            (v(0.5, -1.0, 0.0), v(0.5, 0.0, 0.0)),
            (v(-1.0, 0.5, 0.0), v(0.0, 0.5, 0.0)),
            (v(1.0, 1.0, 0.0), v(0.5, 0.5, 0.0)),
            (v(0.2, 0.2, 3.0), v(0.2, 0.2, 0.0)),
        ];
        for (p, want) in cases {
            assert!(closest_on_triangle(p, a, b, c).distance(want) < 1e-12, "{p}");
        }
    }

    #[test]
    fn triangle_plan_queries() {
        let t = Tri::new([v(0.0, 0.0, 1.0), v(2.0, 0.0, 1.0), v(0.0, 2.0, 3.0)], 2, 7).unwrap();
        assert!(t.contains_plan(DVec2::new(0.5, 0.5)));
        assert!(!t.contains_plan(DVec2::new(1.5, 1.5)));
        assert!((t.z_at(DVec2::new(0.0, 1.0)) - 2.0).abs() < 1e-12);
        assert!(Tri::new([v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(2.0, 0.0, 0.0)], 0, 0).is_none());
        let grid = TriGrid::new(std::slice::from_ref(&t));
        let mut out = Vec::new();
        grid.query(
            std::slice::from_ref(&t),
            DVec2::new(1.0, 1.0),
            DVec2::new(5.0, 5.0),
            &mut out,
        );
        assert_eq!(out, vec![0]);
        grid.query(
            std::slice::from_ref(&t),
            DVec2::new(3.0, 3.0),
            DVec2::new(5.0, 5.0),
            &mut out,
        );
        assert!(out.is_empty());
        let empty = TriGrid::new(&[]);
        empty.query(&[], DVec2::ZERO, DVec2::ONE, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn polygons() {
        let sq = Polygon {
            points: vec![
                DVec2::new(0.0, 0.0),
                DVec2::new(1.0, 0.0),
                DVec2::new(1.0, 1.0),
                DVec2::new(0.0, 1.0),
            ],
        };
        assert!(sq.contains(DVec2::new(0.5, 0.5)));
        assert!(!sq.contains(DVec2::new(1.5, 0.5)));
        assert!((sq.area2() - 2.0).abs() < 1e-12);
        assert!(sq.overlaps_box(DVec2::new(0.5, 0.5), DVec2::new(3.0, 3.0)));
        assert!(sq.overlaps_box(DVec2::new(-1.0, -1.0), DVec2::new(3.0, 3.0)));
        assert!(sq.overlaps_box(DVec2::new(-1.0, 0.4), DVec2::new(3.0, 0.6)));
        assert!(!sq.overlaps_box(DVec2::new(2.0, 2.0), DVec2::new(3.0, 3.0)));
    }
}
