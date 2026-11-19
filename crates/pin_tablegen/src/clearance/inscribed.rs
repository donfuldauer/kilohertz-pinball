//! Largest inscribed circle of a plan polygon (even-odd rule).
//!
//! Branch and bound over square cells (the "pole of inaccessibility"
//! search): the signed distance `s(q)` (distance to the region's boundary,
//! positive inside, negative outside) is 1-Lipschitz, so a cell of
//! half-side `h` centred at `c` holds no point with `s > s(c) + h·√2`.
//!
//! The boundary is that of the even-odd filled region, not the outline as
//! traced: [`Boundary`] splits collinear overlapping edges at each other's
//! endpoints and drops every piece covered an even number of times (within
//! 1 nm, the tolerance of `Polygon::covers` and
//! `Polygon::has_filled_area`), since crossing it does not change parity.
//! A retraced edge or spike is therefore no wall; the outer outline and the
//! outline of a hole are (odd multiplicity).
//! Cells whose bound cannot beat the best centre found by more than the
//! precision are dropped; the rest are split in four. The result is the
//! best centre's distance: never above the true inscribed radius and at
//! most the precision below it. Cells are visited in a fixed order (a
//! binary heap keyed by bound, ties by creation number), so the result is
//! deterministic.

use super::geom::{Polygon, segment_distance};
use core::cmp::Ordering;
use glam::DVec2;
use std::collections::BinaryHeap;

/// Most cells along the longer side of the first grid (very thin
/// polygons start from cells of `extent / MAX_START_SIDE`).
const MAX_START_SIDE: f64 = 256.0;
/// Most cells examined; past it the best centre so far is returned (still
/// a lower bound). Never reached by a polygon of the table's size at the
/// validator's precision.
const MAX_CELLS: usize = 2_000_000;

/// Coincidence tolerance (m) of edge endpoints and of collinearity.
const TOL: f64 = 1.0e-9;

/// The boundary of a polygon's even-odd filled region: the edges, split
/// where collinear edges overlap partially, without the pieces covered an
/// even number of times. Built once per polygon.
pub(super) struct Boundary<'a> {
    poly: &'a Polygon,
    segs: Vec<(DVec2, DVec2)>,
}

impl<'a> Boundary<'a> {
    /// The boundary of `poly` (finite coordinates). Pieces keep the order
    /// of the edges they come from (deterministic).
    pub fn new(poly: &'a Polygon) -> Boundary<'a> {
        let pts = &poly.points;
        let n = pts.len();
        let edges: Vec<(DVec2, DVec2)> = (0..n)
            .map(|i| (pts[i], pts[(i + 1) % n]))
            .filter(|(a, b)| a.distance(*b) > TOL)
            .collect();
        // Split every edge at the endpoints of the edges collinear with it
        // that fall strictly inside it.
        let mut pieces: Vec<(DVec2, DVec2)> = Vec::with_capacity(edges.len());
        let mut cuts: Vec<(f64, DVec2)> = Vec::new();
        for &(a, b) in &edges {
            let len = a.distance(b);
            let dir = (b - a) / len;
            let off = |p: DVec2| dir.perp_dot(p - a).abs();
            cuts.clear();
            for &(c, d) in &edges {
                if off(c) > TOL || off(d) > TOL {
                    continue;
                }
                for p in [c, d] {
                    let t = dir.dot(p - a);
                    if t > TOL && t < len - TOL {
                        cuts.push((t, p));
                    }
                }
            }
            cuts.sort_by(|x, y| x.0.total_cmp(&y.0));
            cuts.dedup_by(|x, y| x.0 - y.0 <= TOL);
            let mut prev = a;
            for &(_, p) in &cuts {
                pieces.push((prev, p));
                prev = p;
            }
            pieces.push((prev, b));
        }
        // Keep one copy of every piece of odd multiplicity.
        let same = |p: DVec2, q: DVec2| p.distance(q) <= TOL;
        let mut used = vec![false; pieces.len()];
        let mut segs = Vec::new();
        for i in 0..pieces.len() {
            if used[i] {
                continue;
            }
            let (a, b) = pieces[i];
            let mut count = 0usize;
            for (j, &(c, d)) in pieces.iter().enumerate().skip(i) {
                if !used[j] && ((same(a, c) && same(b, d)) || (same(a, d) && same(b, c))) {
                    used[j] = true;
                    count += 1;
                }
            }
            if count % 2 == 1 {
                segs.push((a, b));
            }
        }
        Boundary { poly, segs }
    }

    /// The boundary's segments.
    #[cfg(test)]
    pub fn segments(&self) -> &[(DVec2, DVec2)] {
        &self.segs
    }

    /// Distance from `q` to the boundary (infinite when it is empty).
    fn distance(&self, q: DVec2) -> f64 {
        self.segs
            .iter()
            .map(|&(a, b)| segment_distance(q, a, b))
            .fold(f64::INFINITY, f64::min)
    }

    /// Signed distance from `q` to the boundary: positive inside
    /// (even-odd), negative outside.
    fn signed_distance(&self, q: DVec2) -> f64 {
        let d = self.distance(q);
        if self.poly.contains(q) { d } else { -d }
    }
}

/// A square cell: centre, half-side, the centre's signed distance and the
/// bound on the signed distance anywhere in it.
struct Cell {
    c: DVec2,
    h: f64,
    d: f64,
    bound: f64,
    seq: u64,
}

impl Cell {
    fn new(boundary: &Boundary<'_>, c: DVec2, h: f64, seq: u64) -> Cell {
        let d = boundary.signed_distance(c);
        Cell {
            c,
            h,
            d,
            bound: d + h * core::f64::consts::SQRT_2,
            seq,
        }
    }
}

impl PartialEq for Cell {
    fn eq(&self, o: &Cell) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}

impl Eq for Cell {}

impl PartialOrd for Cell {
    fn partial_cmp(&self, o: &Cell) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

impl Ord for Cell {
    /// Larger bound first; among equal bounds, the older cell first.
    fn cmp(&self, o: &Cell) -> Ordering {
        self.bound
            .total_cmp(&o.bound)
            .then_with(|| o.seq.cmp(&self.seq))
    }
}

/// Diameter (same unit as the polygon) of the largest circle inside
/// `poly` (even-odd rule), never above the true value and at most
/// `2 · precision` below it (`precision` is a radius tolerance, > 0).
/// 0 for a polygon with fewer than 3 points, non-finite coordinates or no
/// interior (no boundary left once retraced edges cancel).
pub(super) fn largest_inscribed_diameter(poly: &Polygon, precision: f64) -> f64 {
    let pts = &poly.points;
    if pts.len() < 3 || !pts.iter().all(|p| p.is_finite()) || precision.is_nan() || precision <= 0.0
    {
        return 0.0;
    }
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for &p in pts {
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let size = hi - lo;
    let extent = size.max_element();
    if !extent.is_finite() || extent <= 0.0 {
        return 0.0;
    }
    let boundary = Boundary::new(poly);
    if boundary.segs.is_empty() {
        return 0.0;
    }
    let side = size.min_element().max(extent / MAX_START_SIDE);
    let h = 0.5 * side;
    let mut seq = 0u64;
    let mut heap = BinaryHeap::new();
    let mut best = boundary.signed_distance((lo + hi) * 0.5);
    let nx = (size.x / side).ceil().max(1.0) as usize;
    let ny = (size.y / side).ceil().max(1.0) as usize;
    for j in 0..ny {
        for i in 0..nx {
            seq += 1;
            let c = lo + DVec2::new((i as f64 + 0.5) * side, (j as f64 + 0.5) * side);
            heap.push(Cell::new(&boundary, c, h, seq));
        }
    }
    let mut examined = 0usize;
    while let Some(cell) = heap.pop() {
        best = best.max(cell.d);
        if cell.bound - best <= precision {
            // The heap pops the largest bound first: no cell left can beat
            // the best centre by more than the precision.
            break;
        }
        examined += 1;
        if examined >= MAX_CELLS {
            break;
        }
        let q = 0.5 * cell.h;
        for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            seq += 1;
            let child = Cell::new(&boundary, cell.c + DVec2::new(dx * q, dy * q), q, seq);
            best = best.max(child.d);
            heap.push(child);
        }
    }
    2.0 * best.max(0.0)
}
