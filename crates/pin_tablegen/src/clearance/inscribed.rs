//! Largest inscribed circle of a plan polygon (even-odd rule).
//!
//! Branch and bound over square cells (the "pole of inaccessibility"
//! search): the signed distance `s(q)` (distance to the region's boundary,
//! positive inside, negative outside) is 1-Lipschitz, so a cell of
//! half-side `h` centred at `c` holds no point with `s > s(c) + h·√2`.
//! Cells come out of a binary heap, largest bound first (ties by creation
//! number, so every result is deterministic), and are split in four. At
//! any moment the best centre found is a lower bound on the inscribed
//! radius and the top cell's bound an upper bound.
//!
//! * [`largest_inscribed_diameter`] stops when the two are within the
//!   precision: never above the true diameter, at most `2 · precision`
//!   below it.
//! * [`fits`] decides whether a circle of a given diameter fits: it stops
//!   as soon as the lower bound reaches the radius (fits) or the upper
//!   bound drops below it (too small), so it does not depend on a
//!   precision. The decision tolerance is a 1 µm radius bracket: once the
//!   bounds are within 1 µm of each other with the radius between them the
//!   circle counts as fitting, so a ball up to 1 µm (radius; 2 µm in
//!   diameter) wider than the inscribed circle is accepted. A narrower
//!   bracket would let a strip exactly as wide as the ball exhaust
//!   [`MAX_CELLS`] (the centre line is a plateau at the inscribed radius).
//!
//! The boundary is that of the even-odd filled region, not the outline as
//! traced: [`Boundary`] splits collinear overlapping edges at each other's
//! endpoints and drops every piece covered an even number of times (within
//! 1 nm, the tolerance of `Polygon::covers` and
//! `Polygon::has_filled_area`), since crossing it does not change parity.
//! A retraced edge or spike is therefore no wall; the outer outline and the
//! outline of a hole are (odd multiplicity).

use super::geom::{Polygon, segment_distance};
use core::cmp::Ordering;
use glam::DVec2;
use pin_core::fmath::{max_f64, min_f64};
use std::collections::BinaryHeap;

/// Most cells along the longer side of the first grid (very thin
/// polygons start from cells of `extent / MAX_START_SIDE`).
const MAX_START_SIDE: f64 = 256.0;
/// Most cells examined; past it [`largest_inscribed_diameter`] returns the
/// best centre so far (still a lower bound) and [`fits`] returns
/// [`Fit::Undecided`]. Never reached by a polygon of the table's size at the
/// validator's precision, nor by [`fits`], whose 1 µm bracket ends the search
/// on a plateau at the ball's radius after about 10^5 cells.
const MAX_CELLS: usize = 2_000_000;

/// Coincidence tolerance (m) of edge endpoints and of collinearity.
const TOL: f64 = 1.0e-9;
/// Width (m) of the radius bracket at which [`fits`] calls a circle that
/// just touches the boundary fitting (1 µm). A narrower bracket lets a
/// strip exactly as wide as the ball exhaust `MAX_CELLS`.
const TOUCH_TOLERANCE: f64 = 1.0e-6;

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
            .fold(f64::INFINITY, min_f64)
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

/// The branch and bound state over one polygon's [`Boundary`].
struct Search<'a> {
    boundary: Boundary<'a>,
    heap: BinaryHeap<Cell>,
    seq: u64,
    /// Largest signed distance at a cell centre so far (lower bound).
    best: f64,
    examined: usize,
}

impl<'a> Search<'a> {
    /// The first grid over `poly`, or `None` for a polygon with fewer than
    /// 3 points, non-finite coordinates or no interior (no boundary left
    /// once retraced edges cancel).
    fn new(poly: &'a Polygon) -> Option<Search<'a>> {
        let pts = &poly.points;
        if pts.len() < 3 || !pts.iter().all(|p| p.is_finite()) {
            return None;
        }
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        for &p in pts {
            lo = DVec2::new(min_f64(lo.x, p.x), min_f64(lo.y, p.y));
            hi = DVec2::new(max_f64(hi.x, p.x), max_f64(hi.y, p.y));
        }
        let size = hi - lo;
        let extent = max_f64(size.x, size.y);
        if !extent.is_finite() || extent <= 0.0 {
            return None;
        }
        let boundary = Boundary::new(poly);
        if boundary.segs.is_empty() {
            return None;
        }
        let side = max_f64(min_f64(size.x, size.y), extent / MAX_START_SIDE);
        let h = 0.5 * side;
        let best = boundary.signed_distance((lo + hi) * 0.5);
        let mut search = Search {
            boundary,
            heap: BinaryHeap::new(),
            seq: 0,
            best,
            examined: 0,
        };
        let nx = max_f64((size.x / side).ceil(), 1.0) as usize;
        let ny = max_f64((size.y / side).ceil(), 1.0) as usize;
        for j in 0..ny {
            for i in 0..nx {
                let c = lo + DVec2::new((i as f64 + 0.5) * side, (j as f64 + 0.5) * side);
                search.push(c, h);
            }
        }
        Some(search)
    }

    fn push(&mut self, c: DVec2, h: f64) {
        self.seq += 1;
        let cell = Cell::new(&self.boundary, c, h, self.seq);
        self.best = max_f64(self.best, cell.d);
        self.heap.push(cell);
    }

    /// Upper bound on the inscribed radius: the largest cell bound left
    /// (`-inf` when no cell is left).
    fn upper(&self) -> f64 {
        self.heap.peek().map_or(f64::NEG_INFINITY, |c| c.bound)
    }

    /// Splits the cell with the largest bound until `done(best, upper)`;
    /// false when it stopped at [`MAX_CELLS`] instead.
    fn run(&mut self, done: impl Fn(f64, f64) -> bool) -> bool {
        loop {
            if done(self.best, self.upper()) {
                return true;
            }
            if self.examined >= MAX_CELLS {
                return false;
            }
            let Some(cell) = self.heap.pop() else {
                return true;
            };
            self.examined += 1;
            let q = 0.5 * cell.h;
            for (dx, dy) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                self.push(cell.c + DVec2::new(dx * q, dy * q), q);
            }
        }
    }
}

/// Diameter (same unit as the polygon) of the largest circle inside
/// `poly` (even-odd rule), never above the true value and at most
/// `2 · precision` below it (`precision` is a radius tolerance, > 0; past
/// [`MAX_CELLS`] still a lower bound). 0 for a polygon with fewer than 3
/// points, non-finite coordinates or no interior (no boundary left once
/// retraced edges cancel).
pub(super) fn largest_inscribed_diameter(poly: &Polygon, precision: f64) -> f64 {
    if precision.is_nan() || precision <= 0.0 {
        return 0.0;
    }
    let Some(mut search) = Search::new(poly) else {
        return 0.0;
    };
    search.run(|best, upper| upper - best <= precision);
    2.0 * max_f64(search.best, 0.0)
}

/// Whether a circle fits inside a polygon ([`fits`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Fit {
    /// A circle of the diameter fits (or touches: the inscribed diameter
    /// is at most 2 µm below it, a 1 µm bracket on the radius).
    Fits,
    /// The largest circle inside is narrower (every remaining cell bound
    /// is below the radius).
    TooSmall,
    /// The search stopped at [`MAX_CELLS`] with the radius between its
    /// bounds; callers must not treat this as too small.
    Undecided,
}

/// Whether a circle of `diameter` (same unit as the polygon, > 0) fits
/// inside `poly` (even-odd rule): refines until the best centre's distance
/// reaches the radius ([`Fit::Fits`]) or the largest remaining cell bound
/// is below it ([`Fit::TooSmall`]). Decision tolerance: when the two bounds
/// are within 1 µm of each other (a radius bracket, [`TOUCH_TOLERANCE`])
/// with the radius between them, the circle counts as fitting, so a ball
/// up to 1 µm wider in radius (2 µm in diameter) than the inscribed circle
/// is accepted; a finer bracket would never stop on a strip as wide as the
/// ball. A degenerate polygon (see [`largest_inscribed_diameter`])
/// holds nothing ([`Fit::TooSmall`]); a non-positive or NaN diameter
/// fits.
pub(super) fn fits(poly: &Polygon, diameter: f64) -> Fit {
    if diameter.is_nan() || diameter <= 0.0 {
        return Fit::Fits;
    }
    let Some(mut search) = Search::new(poly) else {
        return Fit::TooSmall;
    };
    let r = 0.5 * diameter;
    let decided =
        search.run(|best, upper| best >= r || upper < r || upper - best <= TOUCH_TOLERANCE);
    if search.best >= r {
        Fit::Fits
    } else if search.upper() < r {
        Fit::TooSmall
    } else if decided {
        // Both bounds within `TOUCH_TOLERANCE` of the radius.
        Fit::Fits
    } else {
        Fit::Undecided
    }
}
