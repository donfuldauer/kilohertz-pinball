//! Largest inscribed circle of a plan polygon (even-odd rule).
//!
//! Branch and bound over square cells (the "pole of inaccessibility"
//! search): the signed distance `s(q)` (distance to the nearest edge,
//! positive inside, negative outside) is 1-Lipschitz, so a cell of
//! half-side `h` centred at `c` holds no point with `s > s(c) + h·√2`.
//! Cells whose bound cannot beat the best centre found by more than the
//! precision are dropped; the rest are split in four. The result is the
//! best centre's distance: never above the true inscribed radius and at
//! most the precision below it. Cells are visited in a fixed order (a
//! binary heap keyed by bound, ties by creation number), so the result is
//! deterministic.

use super::geom::Polygon;
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

/// Signed distance from `q` to the polygon's edges: positive inside
/// (even-odd), negative outside.
fn signed_distance(poly: &Polygon, q: DVec2) -> f64 {
    let d = poly.boundary_distance(q);
    if poly.contains(q) { d } else { -d }
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
    fn new(poly: &Polygon, c: DVec2, h: f64, seq: u64) -> Cell {
        let d = signed_distance(poly, c);
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
/// interior.
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
    let side = size.min_element().max(extent / MAX_START_SIDE);
    let h = 0.5 * side;
    let mut seq = 0u64;
    let mut heap = BinaryHeap::new();
    let mut best = signed_distance(poly, (lo + hi) * 0.5);
    let nx = (size.x / side).ceil().max(1.0) as usize;
    let ny = (size.y / side).ceil().max(1.0) as usize;
    for j in 0..ny {
        for i in 0..nx {
            seq += 1;
            let c = lo + DVec2::new((i as f64 + 0.5) * side, (j as f64 + 0.5) * side);
            heap.push(Cell::new(poly, c, h, seq));
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
            let child = Cell::new(poly, cell.c + DVec2::new(dx * q, dy * q), q, seq);
            best = best.max(child.d);
            heap.push(child);
        }
    }
    2.0 * best.max(0.0)
}
