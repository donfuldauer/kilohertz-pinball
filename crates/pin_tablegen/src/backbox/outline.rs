//! The optional polygon `outline` and the `[[backbox.walls]]` polylines of
//! `backbox_layout.toml`: validation (errors name `backbox.outline`,
//! `backbox.outline[i]` or `backbox.walls[i]`) and the planar geometry the
//! generator needs (edge orientation, hole cuts, floor heights, disc
//! containment).
//!
//! All coordinates are plane coordinates `[u, v]` in mm from the layout's
//! `origin` (`u` across = backbox `x`, `v` up = backbox `z`). Predicates run
//! in `f64` on the `f32` inputs (differences and products of `f32` values are
//! exact or nearly so in `f64`), in a fixed iteration order.

use super::{BackboxWall, MIN_EXTENT_MM, check_range, gen_err};
use crate::layout::{GenError, MAX_COORD_MM, MAX_SIZE_MM};

/// Most points in an `outline` or in one wall.
pub const MAX_OUTLINE_POINTS: usize = 1024;
/// An outline whose points all lie within this distance of one line is
/// zero-size, mm.
pub const ZERO_SIZE_MM: f64 = 1.0e-3;
/// Smallest accepted outline area, mm² (a smaller one is zero-size).
pub const MIN_OUTLINE_AREA_MM2: f64 = 1.0e-3;
/// Largest difference between `width` / `height` and the outline's
/// bounding box, mm.
pub const OUTLINE_FIT_TOLERANCE_MM: f32 = 0.01;

const OUTLINE: &str = "backbox.outline";

/// A validated outline.
#[derive(Clone, Debug)]
pub(super) struct Outline {
    /// The points, as given.
    pub pts: Vec<[f32; 2]>,
    /// Counter-clockwise in `(u, v)` (positive signed area).
    pub ccw: bool,
    /// Bounding box low corner, mm.
    pub lo: [f32; 2],
    /// Bounding box high corner, mm.
    pub hi: [f32; 2],
}

fn d2(p: [f32; 2]) -> [f64; 2] {
    [f64::from(p[0]), f64::from(p[1])]
}

/// Twice the signed area of the triangle `a b c` (positive when
/// counter-clockwise).
fn orient(a: [f64; 2], b: [f64; 2], c: [f64; 2]) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

/// `p`, collinear with `a b`, lies on the closed segment `a b`.
fn on_segment(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> bool {
    p[0] >= a[0].min(b[0])
        && p[0] <= a[0].max(b[0])
        && p[1] >= a[1].min(b[1])
        && p[1] <= a[1].max(b[1])
}

/// The closed segments `p1 p2` and `p3 p4` share at least one point
/// (proper crossing, touching or collinear overlap).
pub(super) fn segments_touch(p1: [f64; 2], p2: [f64; 2], p3: [f64; 2], p4: [f64; 2]) -> bool {
    if p1[0].max(p2[0]) < p3[0].min(p4[0])
        || p3[0].max(p4[0]) < p1[0].min(p2[0])
        || p1[1].max(p2[1]) < p3[1].min(p4[1])
        || p3[1].max(p4[1]) < p1[1].min(p2[1])
    {
        return false;
    }
    let d1 = orient(p3, p4, p1);
    let d2 = orient(p3, p4, p2);
    let d3 = orient(p1, p2, p3);
    let d4 = orient(p1, p2, p4);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0))
        && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0))
    {
        return true;
    }
    (d1 == 0.0 && on_segment(p3, p4, p1))
        || (d2 == 0.0 && on_segment(p3, p4, p2))
        || (d3 == 0.0 && on_segment(p1, p2, p3))
        || (d4 == 0.0 && on_segment(p1, p2, p4))
}

/// Every coordinate of `p` finite and within ±[`MAX_COORD_MM`].
fn check_point(element: &str, what: &str, p: [f32; 2]) -> Result<(), GenError> {
    for (k, &c) in p.iter().enumerate() {
        check_range(
            element,
            &format!("{what}[{k}]"),
            c,
            -MAX_COORD_MM,
            MAX_COORD_MM,
        )?;
    }
    Ok(())
}

/// Validates an outline, in this order: 3..=[`MAX_OUTLINE_POINTS`] points,
/// all finite (and within ±[`MAX_COORD_MM`]), not all on one line
/// (zero-size), no repeated consecutive points, no edge folding back on its
/// neighbour, no two non-adjacent edges sharing a point (self-intersecting,
/// a bow-tie included), an area above [`MIN_OUTLINE_AREA_MM2`], and a bounding box
/// of [`MIN_EXTENT_MM`]..=[`MAX_SIZE_MM`] each way.
pub(super) fn check_outline(pts: &[[f32; 2]]) -> Result<Outline, GenError> {
    let n = pts.len();
    if n < 3 {
        return Err(gen_err(
            OUTLINE,
            format!("{n} points: a polygon needs at least 3"),
        ));
    }
    if n > MAX_OUTLINE_POINTS {
        return Err(gen_err(
            OUTLINE,
            format!("{n} points, more than {MAX_OUTLINE_POINTS}"),
        ));
    }
    for (i, &p) in pts.iter().enumerate() {
        check_point(&format!("{OUTLINE}[{i}]"), "point", p)?;
    }
    let q: Vec<[f64; 2]> = pts.iter().map(|&p| d2(p)).collect();
    // Zero size: every point within ZERO_SIZE_MM of the line through the
    // first point and the point farthest from it (or all points equal).
    let far = (1..n).fold(0, |best, i| {
        let d = |k: usize| (q[k][0] - q[0][0]).powi(2) + (q[k][1] - q[0][1]).powi(2);
        if d(i) > d(best) { i } else { best }
    });
    let span = libm::hypot(q[far][0] - q[0][0], q[far][1] - q[0][1]);
    let thickness = if span > 0.0 {
        (0..n)
            .map(|i| (orient(q[0], q[far], q[i]) / span).abs())
            .fold(0.0, f64::max)
    } else {
        0.0
    };
    if thickness <= ZERO_SIZE_MM {
        return Err(gen_err(
            OUTLINE,
            format!(
                "zero-size outline: the points are coincident or collinear (spread {thickness} mm across, {span} mm along)"
            ),
        ));
    }
    for i in 0..n {
        let j = (i + 1) % n;
        if q[i] == q[j] {
            return Err(gen_err(
                format!("{OUTLINE}[{j}]"),
                format!("point {j} repeats point {i}: zero-length edge"),
            ));
        }
    }
    // An edge running back over its neighbour (a spike of zero width).
    for i in 0..n {
        let (a, b, c) = (q[(i + n - 1) % n], q[i], q[(i + 1) % n]);
        let back = (b[0] - a[0]) * (c[0] - b[0]) + (b[1] - a[1]) * (c[1] - b[1]);
        if orient(a, b, c) == 0.0 && back < 0.0 {
            return Err(gen_err(
                OUTLINE,
                format!("self-intersecting: the edges at point {i} fold back over each other"),
            ));
        }
    }
    // Non-adjacent edges i (q[i] q[i+1]) and j must not share a point.
    for i in 0..n {
        for j in i + 2..n {
            if i == 0 && j == n - 1 {
                continue; // adjacent through the closing point
            }
            if segments_touch(q[i], q[(i + 1) % n], q[j], q[(j + 1) % n]) {
                return Err(gen_err(
                    OUTLINE,
                    format!(
                        "self-intersecting: edge {i} (points {i}..{}) and edge {j} (points {j}..{}) cross or touch",
                        (i + 1) % n,
                        (j + 1) % n
                    ),
                ));
            }
        }
    }
    // Shoelace about the first point (less cancellation far from 0).
    let mut area2 = 0.0f64;
    for i in 1..n - 1 {
        area2 += orient(q[0], q[i], q[i + 1]);
    }
    let area = 0.5 * area2;
    if area.abs() <= MIN_OUTLINE_AREA_MM2 {
        return Err(gen_err(
            OUTLINE,
            format!(
                "zero-size outline: area {area} mm², at least {MIN_OUTLINE_AREA_MM2} mm² needed"
            ),
        ));
    }
    let mut lo = pts[0];
    let mut hi = pts[0];
    for p in pts {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    check_range(
        OUTLINE,
        "bounding-box width",
        hi[0] - lo[0],
        MIN_EXTENT_MM,
        MAX_SIZE_MM,
    )?;
    check_range(
        OUTLINE,
        "bounding-box height",
        hi[1] - lo[1],
        MIN_EXTENT_MM,
        MAX_SIZE_MM,
    )?;
    Ok(Outline {
        pts: pts.to_vec(),
        ccw: area > 0.0,
        lo,
        hi,
    })
}

/// Validates the walls: each has 2..=[`MAX_OUTLINE_POINTS`] finite points
/// that are not all the same.
pub(super) fn check_walls(walls: &[BackboxWall]) -> Result<(), GenError> {
    for (i, w) in walls.iter().enumerate() {
        let e = format!("backbox.walls[{i}]");
        let n = w.points.len();
        if n < 2 {
            return Err(gen_err(&e, format!("{n} points: a wall needs at least 2")));
        }
        if n > MAX_OUTLINE_POINTS {
            return Err(gen_err(
                &e,
                format!("{n} points, more than {MAX_OUTLINE_POINTS}"),
            ));
        }
        for (j, &p) in w.points.iter().enumerate() {
            check_point(&e, &format!("points[{j}]"), p)?;
        }
        if w.points.iter().all(|&p| p == w.points[0]) {
            return Err(gen_err(
                &e,
                "zero length: every point is the same".to_string(),
            ));
        }
    }
    Ok(())
}

impl Outline {
    /// Number of edges (= points).
    pub fn len(&self) -> usize {
        self.pts.len()
    }

    /// Edge `i`: point `i` to point `i + 1` (wrapping).
    pub fn edge(&self, i: usize) -> ([f32; 2], [f32; 2]) {
        (self.pts[i], self.pts[(i + 1) % self.pts.len()])
    }

    /// The interior is above edge `i` (its outward normal points down), so
    /// a hole below cuts it.
    fn faces_down(&self, i: usize) -> bool {
        let (a, b) = self.edge(i);
        let du = b[0] - a[0];
        if self.ccw { du > 0.0 } else { du < 0.0 }
    }

    /// `width` and `height` equal the bounding box (within
    /// [`OUTLINE_FIT_TOLERANCE_MM`]).
    pub fn check_fits(&self, width: f32, height: f32) -> Result<(), GenError> {
        for (what, given, derived) in [
            ("width", width, self.hi[0] - self.lo[0]),
            ("height", height, self.hi[1] - self.lo[1]),
        ] {
            let fits = (given - derived).abs() <= OUTLINE_FIT_TOLERANCE_MM;
            if !fits {
                return Err(gen_err(
                    "backbox",
                    format!(
                        "{what} {given} mm does not match the outline's bounding-box {what} {derived} mm"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// The parts of edge `i` left after cutting the holes (`u` intervals,
    /// mm) out of it, in order along the edge. Only downward-facing edges
    /// are cut; pieces shorter than 1 µm are dropped.
    pub fn edge_pieces(&self, i: usize, holes: &[(f32, f32)]) -> Vec<([f32; 2], [f32; 2])> {
        let (a, b) = self.edge(i);
        if !self.faces_down(i) {
            return vec![(a, b)];
        }
        let du = f64::from(b[0]) - f64::from(a[0]);
        let mut keep: Vec<(f64, f64)> = vec![(0.0, 1.0)];
        for &(ha, hb) in holes {
            let s0 = (f64::from(ha) - f64::from(a[0])) / du;
            let s1 = (f64::from(hb) - f64::from(a[0])) / du;
            let (c0, c1) = (s0.min(s1), s0.max(s1));
            let mut next = Vec::with_capacity(keep.len() + 1);
            for (k0, k1) in keep {
                if c1 <= k0 || c0 >= k1 {
                    next.push((k0, k1));
                    continue;
                }
                if c0 > k0 {
                    next.push((k0, c0));
                }
                if c1 < k1 {
                    next.push((c1, k1));
                }
            }
            keep = next;
        }
        let at = |s: f64| -> [f32; 2] {
            if s <= 0.0 {
                a
            } else if s >= 1.0 {
                b
            } else {
                [
                    (f64::from(a[0]) + s * du) as f32,
                    (f64::from(a[1]) + s * (f64::from(b[1]) - f64::from(a[1]))) as f32,
                ]
            }
        };
        let len = libm::hypot(du, f64::from(b[1]) - f64::from(a[1]));
        keep.into_iter()
            .filter(|(k0, k1)| (k1 - k0) * len > 1.0e-3)
            .map(|(k0, k1)| (at(k0), at(k1)))
            .collect()
    }

    /// `v` of downward-facing edge `i` at `u`, if `u` is within its span.
    fn v_at(&self, i: usize, u: f64) -> Option<f64> {
        let (a, b) = self.edge(i);
        let (a, b) = (d2(a), d2(b));
        if u < a[0].min(b[0]) || u > a[0].max(b[0]) {
            return None;
        }
        Some(a[1] + (u - a[0]) / (b[0] - a[0]) * (b[1] - a[1]))
    }

    /// The bottom of the outline at `u`: the lowest downward-facing edge
    /// over `u`, mm (the bounding-box bottom if none).
    pub fn floor_at(&self, u: f32) -> f32 {
        let u = f64::from(u);
        let mut best = f64::INFINITY;
        for i in 0..self.len() {
            if self.faces_down(i)
                && let Some(v) = self.v_at(i, u)
            {
                best = best.min(v);
            }
        }
        if best.is_finite() {
            best as f32
        } else {
            self.lo[1]
        }
    }

    /// The lowest point of the outline's bottom over `u` in `hole`, mm (the
    /// bounding-box bottom if no edge lies over it).
    pub fn lowest_floor(&self, hole: (f32, f32)) -> f32 {
        let (h0, h1) = (f64::from(hole.0), f64::from(hole.1));
        let mut best = f64::INFINITY;
        for i in 0..self.len() {
            if !self.faces_down(i) {
                continue;
            }
            let (a, b) = self.edge(i);
            let (u0, u1) = (
                f64::from(a[0].min(b[0])).max(h0),
                f64::from(a[0].max(b[0])).min(h1),
            );
            if u0 > u1 {
                continue;
            }
            for u in [u0, u1] {
                if let Some(v) = self.v_at(i, u) {
                    best = best.min(v);
                }
            }
        }
        if best.is_finite() {
            best as f32
        } else {
            self.lo[1]
        }
    }

    /// The disc of radius `r` at `c` lies inside the outline (centre inside,
    /// at least `r` from every edge).
    pub fn contains_disc(&self, c: [f32; 2], r: f32) -> bool {
        let p = d2(c);
        let r = f64::from(r);
        let n = self.len();
        let mut inside = false;
        for i in 0..n {
            let (a, b) = self.edge(i);
            let (a, b) = (d2(a), d2(b));
            if (a[1] > p[1]) != (b[1] > p[1]) {
                let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                if p[0] < x {
                    inside = !inside;
                }
            }
            let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
            let t =
                (((p[0] - a[0]) * ex + (p[1] - a[1]) * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
            let (dx, dy) = (p[0] - (a[0] + t * ex), p[1] - (a[1] + t * ey));
            if dx * dx + dy * dy < r * r {
                return false;
            }
        }
        inside
    }

    /// The convex hull of the discs `(a, ra)` and `(b, rb)` (a tapered
    /// capsule, e.g. a flipper bat) lies inside the outline: both end
    /// centres inside (even-odd, as [`Outline::contains_disc`]) and no edge
    /// closer to the axis than the linearly interpolated radius (strict
    /// rejection, so a tangent edge is accepted).
    pub fn contains_tapered_capsule(&self, a: [f32; 2], ra: f32, b: [f32; 2], rb: f32) -> bool {
        self.tapered_capsule_breach(a, ra, b, rb).is_none()
    }

    /// Where the hull of [`Outline::contains_tapered_capsule`] leaves the
    /// outline: `None` when it is inside, else an end centre outside the
    /// polygon or the axis point of the deepest edge intrusion, mm.
    ///
    /// Exact up to rounding: the hull is the union of the discs centred at
    /// `A(t) = a + t (b - a)` with radius `r(t) = (1 - t) ra + t rb`,
    /// `t` in `0..=1`, and per edge `g(t) = dist(A(t), edge) - r(t)` is
    /// convex in `t`. Its minimum is at `t = 0`, `t = 1`, or the minimum
    /// of one of the pieces of the point-to-segment distance: the
    /// stationary point of the distance to either edge end, the ends of the
    /// range where `A(t)` projects inside the edge, or the axis crossing
    /// the edge's line. `g` is evaluated at each of those candidates.
    pub fn tapered_capsule_breach(
        &self,
        a: [f32; 2],
        ra: f32,
        b: [f32; 2],
        rb: f32,
    ) -> Option<[f32; 2]> {
        if !self.contains_disc(a, 0.0) {
            return Some(a);
        }
        if !self.contains_disc(b, 0.0) {
            return Some(b);
        }
        let (pa, pb) = (d2(a), d2(b));
        let (ra, rb) = (f64::from(ra), f64::from(rb));
        let dv = [pb[0] - pa[0], pb[1] - pa[1]];
        let len = libm::hypot(dv[0], dv[1]);
        let at = |t: f64| [pa[0] + t * dv[0], pa[1] + t * dv[1]];
        let radius = |t: f64| (1.0 - t) * ra + t * rb;
        let mut worst: Option<(f64, f64)> = None;
        for i in 0..self.len() {
            let (p, q) = self.edge(i);
            let (p, q) = (d2(p), d2(q));
            let mut cand = [f64::NAN; 7];
            cand[0] = 0.0;
            cand[1] = 1.0;
            if len > 0.0 {
                let u = [dv[0] / len, dv[1] / len];
                // Radius change per mm along the axis; |k| >= 1 means one
                // end disc contains the other (no interior stationary point).
                let k = (rb - ra) / len;
                if k.abs() < 1.0 {
                    for (slot, e) in [(2, p), (3, q)] {
                        let w0 = [pa[0] - e[0], pa[1] - e[1]];
                        let s0 = w0[0] * u[0] + w0[1] * u[1];
                        let h = (w0[0] * u[1] - w0[1] * u[0]).abs();
                        // d/ds (sqrt(s² + h²) - k s) = 0.
                        let s = k * h / libm::sqrt(1.0 - k * k);
                        cand[slot] = (s - s0) / len;
                    }
                }
                let ee = [q[0] - p[0], q[1] - p[1]];
                let l2 = ee[0] * ee[0] + ee[1] * ee[1];
                if l2 > 0.0 {
                    // Projection parameter of A(t) on the edge: c0 + t c1.
                    let c0 = ((pa[0] - p[0]) * ee[0] + (pa[1] - p[1]) * ee[1]) / l2;
                    let c1 = (dv[0] * ee[0] + dv[1] * ee[1]) / l2;
                    if c1 != 0.0 {
                        cand[4] = -c0 / c1;
                        cand[5] = (1.0 - c0) / c1;
                    }
                    // Signed distance to the edge's line: o0 + t o1.
                    let o0 = ee[0] * (pa[1] - p[1]) - ee[1] * (pa[0] - p[0]);
                    let o1 = ee[0] * dv[1] - ee[1] * dv[0];
                    if o1 != 0.0 {
                        cand[6] = -o0 / o1;
                    }
                }
            }
            for t in cand {
                if !t.is_finite() {
                    continue;
                }
                let t = t.clamp(0.0, 1.0);
                let g = dist_to_segment(at(t), p, q) - radius(t);
                if g < 0.0 && worst.is_none_or(|(wg, _)| g < wg) {
                    worst = Some((g, t));
                }
            }
        }
        worst.map(|(_, t)| {
            let x = at(t);
            [x[0] as f32, x[1] as f32]
        })
    }
}

/// Distance from `x` to the closed segment `p q`, mm.
fn dist_to_segment(x: [f64; 2], p: [f64; 2], q: [f64; 2]) -> f64 {
    let (ex, ey) = (q[0] - p[0], q[1] - p[1]);
    let l2 = ex * ex + ey * ey;
    let t = if l2 > 0.0 {
        (((x[0] - p[0]) * ex + (x[1] - p[1]) * ey) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    libm::hypot(x[0] - (p[0] + t * ex), x[1] - (p[1] + t * ey))
}
