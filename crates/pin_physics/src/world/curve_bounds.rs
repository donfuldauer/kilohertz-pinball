//! Provable bounds on the geometry of a guide's curve, by branch and bound
//! over a hierarchy of the curve itself (not over its samples): the
//! tangent-ball radii of level guides whose open ends limit the reach, and
//! the distance from an open end to whatever lies behind it (2.1 fix B7).
//!
//! # The curve and its pieces
//!
//! The curve is the polynomial of each Bézier segment (`f32` control points,
//! evaluated here in `f64`). It is cut into the 16 sample intervals per
//! segment, and those into halves as far as needed. A piece covering local
//! parameters `[s0, s1]` of one segment (`h = s1 − s0`) gets two bounds from
//! its end derivatives (`C''` is linear on a segment, so `|C''|` is largest at
//! an end; every `s` is within `h/2` of an end):
//!
//! * arc length `L ≤ h · (max(|C'(s0)|, |C'(s1)|) + h/2 · B)`, with
//!   `B = max(|C''(s0)|, |C''(s1)|)`;
//! * curvature `κ ≤ B / (min(|C'(s0)|, |C'(s1)|) − h/2 · B)²` (`κ ≤ |C''| /
//!   |C'|²`), infinite where the denominator is not positive.
//!
//! Every point `y` of a piece with end points `p0`, `p1` and arc length at
//! most `L` lies in the ball of radius `L/2` around `(p0 + p1)/2`: the sum
//! `|y − p0| + |y − p1|` is at most `L`, an ellipse with semi-major axis
//! `L/2`. Pieces are grouped in a binary tree of enclosing balls (the
//! smallest ball around two balls). Prefix sums of the arc-length bounds
//! give an upper bound of the arc length between any two points of the
//! curve.
//!
//! # The own neighbourhood
//!
//! Let `κ` bound the curvature along the curve between `a` and a point `y`
//! at arc length `ℓ ≤ 1.5/κ < π/(2κ)` from it (the largest bound of the
//! sample intervals in between, by a range maximum over the tree). With
//! `θ(u) ≤ κu` the angle of the tangent `u` along from `a`:
//! `dist(y, tangent line at a) = |∫ sin θ| ≤ (1 − cos κℓ)/κ` and, by Schur's
//! comparison theorem, `|y − a| ≥ 2 sin(κℓ/2)/κ`; so
//! `|y − a|² / (2 dist) ≥ 1/κ`. Also `(y − a) · T_a = ∫ cos θ ≥ sin(κℓ)/κ > 0`:
//! `y` is in front of `a` along its tangent. Pieces of the curve within that
//! arc length of a query are therefore not looked at: their tangent-ball
//! bound is `1/κ`, and none lies behind an end. (A local, not a global,
//! bound: a tight bend far away does not shrink the neighbourhood here.)
//!
//! # The search
//!
//! A query asks for `inf f(y)` over the curve (two quantities at once, the
//! "sides"), with a lower bound of `f` over every ball of the tree. A piece
//! whose lower bound is within the tolerance of the best value found at an
//! actual curve point is settled, and its lower bound kept; otherwise it is
//! halved, down to a fixed depth, where its lower bound is kept as it is. The
//! result is the smallest lower bound kept: every curve point is in a
//! settled, skipped or deepest piece, so the result never exceeds the
//! infimum, whatever the sampling. A query that is itself a piece of the
//! curve (a table interval) is halved as well when its own size is what
//! keeps a bound low. A search out of its budget of pieces keeps the bounds
//! of the pieces left as they are: a smaller but still proven result, and a
//! construction time bounded for any curve.
//!
//! Each step that matters is proven in the comments where it is made:
//! [`Bez::bounds`], [`CurveTree::own`], [`EndProbe`], [`TableProbe`]
//! (bounds 1 and 2), [`TangentBalls`] (the use at run time), and
//! [`Build::entries`] (tables of many intervals at once).

use glam::{DVec3, Vec3};

use super::spline::{BezierSegment, CubicSpline, SAMPLES_PER_SEGMENT};

const K: usize = SAMPLES_PER_SEGMENT;

/// Relative slack added to every bound for `f64` rounding (the quantities
/// are short sums of products, accurate to ~1e-15 relative).
const ROUND: f64 = 1.0e-9;

/// Largest tangent-ball radius the table holds (metres). Larger balls gain
/// nothing in a 1 ms step, and with the cap whole blocks of far points are
/// settled at once, which keeps the construction close to linear. (A table
/// interval is also capped at 0.9 of the smallest radius of curvature within
/// a segment of it, see [`Build::entries`].)
const TANGENT_BALL_CAP: f64 = 0.3;

/// Margins applied to every stored radius, relative and absolute (metres):
/// the `f32` rounding of the curve point at run time (~1e-7 m at table
/// coordinates of a metre) and of the parameter of the nearest point found
/// by the closest-point search.
const MARGIN: f64 = 0.9;
const MARGIN_ABS: f64 = 1.0e-6;

/// A proven radius as stored.
fn stored(rho: f64) -> f32 {
    (MARGIN * rho - MARGIN_ABS).max(0.0) as f32
}

/// Table intervals per segment along the curve, and in its two end
/// segments (where the ends make the radii change quickly).
const TABLE_COARSE: usize = 4;
const TABLE_FINE: usize = 64;

/// Tangent-ball radii below this (metres) are stored as zero: no pinball is
/// that small, and resolving them would take deep searches.
const RHO_FLOOR: f64 = 1.0e-4;

/// Relative tolerances of the searches: table entries (a piece is settled
/// once its bound is within 30 % of the best value found; the result is
/// then at least 0.7 of the infimum, and usually much closer), end terms,
/// and table entries at their cap (nothing found below it).
const TOL_TABLE: f64 = 0.3;
const TOL_END: f64 = 0.02;
const TOL_CAP: f64 = 0.05;

/// Absolute tolerance of the table searches (metres), up to half the value:
/// a tangent ball a few millimetres across is of no use to a pinball (the
/// smallest has a radius of 7.9 mm) however sharply it is known, and
/// resolving it would mean halving the table interval again and again.
const TABLE_SLACK: f64 = 2.0e-3;

/// The level at which a search settles a piece against the best value `u`
/// found: within the relative tolerance, or the absolute one up to half.
fn settle_level(u: f64, tol: f64, slack: f64) -> f64 {
    u * (1.0 - tol.max((slack / u).min(0.5)))
}

/// Depth limits: halvings of a sample interval (1/16 segment) and of a
/// table interval; with them the finest pieces are 1/4096 of a segment.
const PART_DEPTH: u8 = 8;
const COARSE_DEPTH: u8 = 10;
const FINE_DEPTH: u8 = 6;
const END_DEPTH: u8 = 20;

/// Pieces a table may look at: per slot on average and in all, and per
/// slot at most. Beyond them the searches keep the bounds they have (still
/// lower bounds; only less sharp), which bounds the construction time of
/// any curve. Shapes within the spacing rule need a few dozen per slot.
const TABLE_BUDGET_PER_SLOT: usize = 80;
const TABLE_BUDGET_BASE: usize = 1 << 17;
const SLOT_BUDGET: usize = 2048;

/// A segment's Bézier polynomial in `f64`.
#[derive(Debug, Clone, Copy)]
struct Bez([DVec3; 4]);

impl Bez {
    fn new(seg: &BezierSegment) -> Self {
        Self(seg.p.map(Vec3::as_dvec3))
    }

    fn point(&self, s: f64) -> DVec3 {
        let a = 1.0 - s;
        let [p0, p1, p2, p3] = self.0;
        p0 * (a * a * a) + p1 * (3.0 * a * a * s) + p2 * (3.0 * a * s * s) + p3 * (s * s * s)
    }

    fn d1(&self, s: f64) -> DVec3 {
        let a = 1.0 - s;
        let [p0, p1, p2, p3] = self.0;
        ((p1 - p0) * (a * a) + (p2 - p1) * (2.0 * a * s) + (p3 - p2) * (s * s)) * 3.0
    }

    fn d2(&self, s: f64) -> DVec3 {
        let [p0, p1, p2, p3] = self.0;
        ((p2 - p1 * 2.0 + p0) * (1.0 - s) + (p3 - p2 * 2.0 + p1) * s) * 6.0
    }

    /// Upper bounds of the arc length and of the curvature on `[s0, s1]`
    /// (module documentation).
    fn bounds(&self, s0: f64, s1: f64) -> (f64, f64) {
        let h = s1 - s0;
        let (v0, v1) = (self.d1(s0).length(), self.d1(s1).length());
        let bend = self.d2(s0).length().max(self.d2(s1).length());
        let length = h * (v0.max(v1) + 0.5 * h * bend) * (1.0 + ROUND);
        let slowest = v0.min(v1) - 0.5 * h * bend;
        let curvature = if slowest > 0.0 {
            bend / (slowest * slowest) * (1.0 + ROUND)
        } else {
            f64::INFINITY
        };
        (length, curvature)
    }
}

/// Prefix sums of upper bounds of the arc length over the sample intervals
/// of `spline` (one per sample, starting at 0), and an upper bound of its
/// curvature (module documentation).
pub(super) fn arc_bounds(spline: &CubicSpline) -> (Vec<f64>, f64) {
    let mut prefix = Vec::with_capacity(spline.segment_count() * K + 1);
    let mut total = 0.0f64;
    let mut kappa = 0.0f64;
    prefix.push(0.0);
    for seg in spline.segments() {
        let b = Bez::new(seg);
        for j in 0..K {
            let s0 = j as f64 / K as f64;
            let (length, curvature) = b.bounds(s0, s0 + 1.0 / K as f64);
            total += length;
            kappa = kappa.max(curvature);
            prefix.push(total * (1.0 + ROUND));
        }
    }
    (prefix, kappa)
}

/// Smallest ball around two balls.
fn enclose(a: (DVec3, f64), b: (DVec3, f64)) -> (DVec3, f64) {
    let d = a.0.distance(b.0);
    if d + b.1 <= a.1 {
        return a;
    }
    if d + a.1 <= b.1 {
        return b;
    }
    let r = 0.5 * (d + a.1 + b.1);
    let centre = a.0 + (b.0 - a.0) * ((r - a.1) / d);
    (centre, r * (1.0 + ROUND) + 1.0e-15)
}

/// A piece of the curve in a search: a node of the tree (`level ≥ 1`) or a
/// part `[s0, s1]` of the sample interval `first` (`level == 0`).
#[derive(Debug, Clone, Copy)]
struct Piece {
    centre: DVec3,
    radius: f64,
    /// Sample intervals `first..end` the piece lies in.
    first: u32,
    end: u32,
    node: u32,
    level: u8,
    depth: u8,
    /// Local parameters of a part (dyadic fractions, exact in `f32`).
    s0: f32,
    s1: f32,
}

/// The middle of a part of a sample interval: the curve point and unit
/// tangent at its mid-parameter, an upper bound `reach` of the arc length
/// from there to either end of the part, and an upper bound `turn` of the
/// angle between the tangent there and anywhere on the part.
#[derive(Debug, Clone, Copy)]
struct Mid {
    point: DVec3,
    tangent: DVec3,
    reach: f64,
    turn: f64,
}

/// What a search looks for: two infima at once over the curve points `y`.
trait Probe {
    /// Lower bounds of the two quantities over the curve points in the ball
    /// `(centre, radius)`; infinite: no point there counts. `mid`: the
    /// middle of the piece if it is a part of one sample interval, for a
    /// sharper bound.
    fn bound(&self, centre: DVec3, radius: f64, mid: Option<&Mid>) -> [f64; 2];
    /// The two quantities at the curve point `y` (infinite: not counted).
    fn value(&self, y: DVec3) -> [f64; 2];
    /// If the pieces in sample intervals `first..end` lie in the own
    /// neighbourhood of the query: a lower bound of both quantities there
    /// (infinite: nothing there counts), and they need not be looked at.
    fn skip(&self, first: usize, end: usize) -> Option<f64>;
    /// Where the query is (for the search order).
    fn at(&self) -> DVec3;
    /// Half the size of the query (zero for a point).
    fn size(&self) -> f64;
}

/// Mutable state of the searches of one query.
struct State {
    /// Smallest values found at curve points (upper bounds of the infima).
    upper: [f64; 2],
    /// The curve points where they were found (hints for the next query).
    best: [Option<DVec3>; 2],
    /// Sides already known to be below the floor.
    done: [bool; 2],
    budget: usize,
    stack: Vec<Piece>,
}

impl State {
    fn new() -> Self {
        Self {
            upper: [f64::INFINITY; 2],
            best: [None; 2],
            done: [false; 2],
            budget: 0,
            stack: Vec::new(),
        }
    }

    fn see(&mut self, y: DVec3, value: [f64; 2]) {
        for ((v, upper), best) in value.iter().zip(&mut self.upper).zip(&mut self.best) {
            if *v < *upper {
                *upper = *v;
                *best = Some(y);
            }
        }
    }
}

/// Outcome of one search.
enum Search {
    /// Lower bounds of the two infima.
    Done([f64; 2]),
    /// The query must be halved (its own size keeps a bound low).
    Split,
}

/// Settings of one search.
struct Rules {
    tol: f64,
    slack: f64,
    floor: f64,
    depth: u8,
    can_split: bool,
    init: [f64; 2],
}

/// The hierarchy of an open or closed curve.
pub(super) struct CurveTree {
    bez: Vec<Bez>,
    /// Curve points at the sample-interval boundaries.
    points: Vec<DVec3>,
    /// Curvature bound per sample interval.
    curvature: Vec<f64>,
    /// Prefix sums of the arc-length bounds of the sample intervals.
    arc: Vec<f64>,
    /// Enclosing balls: `levels[0]` per sample interval, then pairs.
    levels: Vec<Vec<(DVec3, f64)>>,
    /// Largest curvature bound per node of the tree (`bends[0]` is
    /// `curvature`), for the curvature between two points.
    bends: Vec<Vec<f64>>,
    /// The middles of whole sample intervals, computed when first needed.
    mids: Vec<core::cell::Cell<Option<Mid>>>,
}

impl CurveTree {
    pub(super) fn new(spline: &CubicSpline) -> Self {
        let bez: Vec<Bez> = spline.segments().iter().map(Bez::new).collect();
        let n = bez.len() * K;
        let local = |i: usize| (i % K) as f64 / K as f64;
        let mut points = Vec::with_capacity(n + 1);
        let mut curvature = Vec::with_capacity(n);
        let mut arc = Vec::with_capacity(n + 1);
        let mut leaves = Vec::with_capacity(n);
        let mut total = 0.0f64;
        arc.push(0.0);
        for i in 0..n {
            let b = &bez[i / K];
            let s0 = local(i);
            let s1 = s0 + 1.0 / K as f64;
            let p0 = b.point(s0);
            let p1 = b.point(s1);
            let (length, kappa) = b.bounds(s0, s1);
            points.push(p0);
            curvature.push(kappa);
            total += length;
            arc.push(total * (1.0 + ROUND));
            leaves.push((0.5 * (p0 + p1), 0.5 * length * (1.0 + ROUND) + 1.0e-15));
        }
        points.push(bez[bez.len() - 1].point(1.0));
        let mut levels = vec![leaves];
        while levels[levels.len() - 1].len() > 1 {
            let below = &levels[levels.len() - 1];
            let up: Vec<(DVec3, f64)> = below
                .chunks(2)
                .map(|pair| match pair {
                    [a, b] => enclose(*a, *b),
                    [a] => *a,
                    _ => unreachable!(),
                })
                .collect();
            levels.push(up);
        }
        let mut bends = vec![curvature.clone()];
        while bends[bends.len() - 1].len() > 1 {
            let below = &bends[bends.len() - 1];
            let up = below
                .chunks(2)
                .map(|c| c.iter().fold(0.0f64, |m, &k| m.max(k)))
                .collect();
            bends.push(up);
        }
        Self {
            bez,
            points,
            curvature,
            arc,
            levels,
            bends,
            mids: vec![core::cell::Cell::new(None); n],
        }
    }

    fn intervals(&self) -> usize {
        self.points.len() - 1
    }

    /// Largest curvature bound over sample intervals `lo..hi`.
    fn curvature_between(&self, lo: usize, hi: usize) -> f64 {
        let (mut lo, mut hi) = (lo, hi);
        let mut most = 0.0f64;
        for level in &self.bends {
            if lo >= hi {
                break;
            }
            if lo % 2 == 1 {
                most = most.max(level[lo]);
                lo += 1;
            }
            if hi % 2 == 1 {
                hi -= 1;
                most = most.max(level[hi]);
            }
            lo /= 2;
            hi /= 2;
        }
        most
    }

    /// The own neighbourhood (module documentation): if every point of
    /// sample intervals `a0..a1` is within arc length `1.5/κ` of every point
    /// of `b0..b1`, `κ` the curvature bound over everything between them,
    /// then `1/κ`. `known` is a curvature bound of part of the span, for a
    /// quick answer when it alone rules the neighbourhood out.
    fn own(&self, a0: usize, a1: usize, b0: usize, b1: usize, known: f64) -> Option<f64> {
        let (lo, hi) = (a0.min(b0), a1.max(b1));
        let span = self.arc[hi] - self.arc[lo];
        if span * known > 1.5 {
            return None;
        }
        let kappa = self.curvature_between(lo, hi);
        if span * kappa > 1.5 {
            None
        } else if kappa > 0.0 {
            Some(1.0 / kappa * (1.0 - ROUND))
        } else {
            Some(f64::INFINITY)
        }
    }

    /// The part `[s0, s1]` of sample interval `i` as a piece.
    fn part(&self, i: usize, s0: f64, s1: f64, p0: DVec3, p1: DVec3, depth: u8) -> Piece {
        let (length, _) = self.bez[i / K].bounds(s0, s1);
        Piece {
            centre: 0.5 * (p0 + p1),
            radius: 0.5 * length * (1.0 + ROUND) + 1.0e-15,
            first: i as u32,
            end: i as u32 + 1,
            node: i as u32,
            level: 0,
            depth,
            s0: s0 as f32,
            s1: s1 as f32,
        }
    }

    /// The end points of a part.
    fn part_ends(&self, piece: &Piece) -> (DVec3, DVec3) {
        let i = piece.first as usize;
        if piece.depth == 0 {
            (self.points[i], self.points[i + 1])
        } else {
            let b = &self.bez[i / K];
            (b.point(f64::from(piece.s0)), b.point(f64::from(piece.s1)))
        }
    }

    /// The middle of a part ([`Mid`]): arc lengths of its two halves, and
    /// the curvature bound of the part (or of its sample interval).
    fn mid(&self, piece: &Piece) -> Mid {
        let whole = piece.depth == 0;
        if whole && let Some(m) = self.mids[piece.first as usize].get() {
            return m;
        }
        let b = &self.bez[piece.first as usize / K];
        let m = 0.5 * (f64::from(piece.s0) + f64::from(piece.s1));
        let (left, k0) = b.bounds(f64::from(piece.s0), m);
        let (right, k1) = b.bounds(m, f64::from(piece.s1));
        let reach = left.max(right);
        let curvature = k0.max(k1).min(self.curvature[piece.first as usize]);
        let mid = Mid {
            point: b.point(m),
            tangent: b.d1(m).normalize_or_zero(),
            reach,
            turn: curvature * reach,
        };
        if whole {
            self.mids[piece.first as usize].set(Some(mid));
        }
        mid
    }

    /// Node `node` of tree level `level`.
    fn tree_piece(&self, level: usize, node: usize) -> Piece {
        let first = node << level;
        let end = ((node + 1) << level).min(self.intervals());
        let (centre, radius) = self.levels[level][node];
        // (A leaf is the ball of the whole interval, as `part` makes it.)
        let s0 = if level == 0 {
            (first % K) as f32 / K as f32
        } else {
            0.0
        };
        Piece {
            centre,
            radius,
            first: first as u32,
            end: end as u32,
            node: node as u32,
            level: level as u8,
            depth: 0,
            s0,
            s1: if level == 0 { s0 + 1.0 / K as f32 } else { 0.0 },
        }
    }

    /// Branch and bound for one query (module documentation). On `Done`,
    /// every curve point `y` has `probe.value(y)[side] ≥` the result (zero
    /// for a side found below `rules.floor`).
    fn search<P: Probe>(
        &self,
        probe: &P,
        rules: &Rules,
        state: &mut State,
        start: &[Piece],
    ) -> Search {
        let mut lower = rules.init;
        state.stack.clear();
        state.stack.extend(start.iter().rev());
        while let Some(piece) = state.stack.pop() {
            // Out of budget, the search only drains: every piece left keeps
            // its bound as it is (still a lower bound over a cover).
            let drain = state.budget == 0;
            state.budget = state.budget.saturating_sub(1);
            if let Some(own) = probe.skip(piece.first as usize, piece.end as usize) {
                lower = [lower[0].min(own), lower[1].min(own)];
                continue;
            }
            if piece.depth == 0 {
                let y = self.points[piece.first as usize];
                state.see(y, probe.value(y));
            }
            for side in 0..2 {
                if state.upper[side] < rules.floor {
                    state.done[side] = true;
                }
            }
            // Settled within the tolerance of the best value found; at the
            // cap (nothing found below it) within 2 % of the cap.
            let settled = |state: &State, side: usize, bound: f64| {
                let settle = if state.upper[side] >= rules.init[side] {
                    (1.0 - TOL_CAP) * rules.init[side]
                } else {
                    settle_level(state.upper[side], rules.tol, rules.slack)
                };
                state.done[side] || bound >= settle
            };
            let mut bound = probe.bound(piece.centre, piece.radius, None);
            let mut mid = None;
            if piece.level as usize == 0
                && !(settled(state, 0, bound[0]) && settled(state, 1, bound[1]))
            {
                // The sharper bound of a part.
                let m = self.mid(&piece);
                state.see(m.point, probe.value(m.point));
                let sharp = probe.bound(piece.centre, piece.radius, Some(&m));
                bound = [bound[0].max(sharp[0]), bound[1].max(sharp[1])];
                mid = Some(m);
            }
            let mut open = [false; 2];
            for side in 0..2 {
                if state.done[side] || bound[side] == f64::INFINITY {
                    continue;
                }
                if drain || settled(state, side, bound[side]) {
                    lower[side] = lower[side].min(bound[side]);
                } else {
                    open[side] = true;
                }
            }
            if !open[0] && !open[1] {
                continue;
            }
            if piece.level as usize > 0 {
                // The nearer child last, so that it is looked at first.
                let a = self.tree_piece(piece.level as usize - 1, 2 * piece.node as usize);
                let has_b =
                    2 * piece.node as usize + 1 < self.levels[piece.level as usize - 1].len();
                if has_b {
                    let b = self.tree_piece(piece.level as usize - 1, 2 * piece.node as usize + 1);
                    let at = probe.at();
                    if a.centre.distance_squared(at) < b.centre.distance_squared(at) {
                        state.stack.push(b);
                        state.stack.push(a);
                    } else {
                        state.stack.push(a);
                        state.stack.push(b);
                    }
                } else {
                    state.stack.push(a);
                }
                continue;
            }
            let mid_point = mid.map(|m: Mid| m.point);
            let can_halve = piece.depth < rules.depth;
            if can_halve && (piece.radius > probe.size() || !rules.can_split) {
                let mid = 0.5 * (f64::from(piece.s0) + f64::from(piece.s1));
                let pm = match mid_point {
                    Some(m) => m,
                    None => self.bez[piece.first as usize / K].point(mid),
                };
                let (p0, p1) = self.part_ends(&piece);
                let i = piece.first as usize;
                let a = self.part(i, f64::from(piece.s0), mid, p0, pm, piece.depth + 1);
                let b = self.part(i, mid, f64::from(piece.s1), pm, p1, piece.depth + 1);
                let at = probe.at();
                if a.centre.distance_squared(at) < b.centre.distance_squared(at) {
                    state.stack.push(b);
                    state.stack.push(a);
                } else {
                    state.stack.push(a);
                    state.stack.push(b);
                }
                continue;
            }
            if rules.can_split {
                return Search::Split;
            }
            // As fine as it gets: keep the lower bound as it is.
            for side in 0..2 {
                if open[side] {
                    lower[side] = lower[side].min(bound[side]);
                }
            }
        }
        for (low, done) in lower.iter_mut().zip(state.done) {
            if done {
                *low = 0.0;
            }
        }
        Search::Done(lower)
    }

    /// The whole curve as one piece.
    fn root(&self) -> Piece {
        self.tree_piece(self.levels.len() - 1, 0)
    }

    /// The group stage of the table construction: the pieces of `list`
    /// (with the pieces already settled and skipped, a cover of the curve)
    /// that a whole group of table intervals (`probe`) cannot settle against
    /// `settle`, refined while they are larger than the group, go to `out`;
    /// the lower bounds of the settled ones go to `lower`. Every curve point
    /// is still in a piece of `out`, a settled piece or a skipped one, and
    /// the bounds of the group hold for each of its intervals.
    fn filter<P: Probe>(
        &self,
        probe: &P,
        settle: [f64; 2],
        list: &[Piece],
        lower: &mut [f64; 2],
        out: &mut Vec<Piece>,
        state: &mut State,
    ) {
        state.stack.clear();
        state.stack.extend(list.iter().rev());
        while let Some(piece) = state.stack.pop() {
            if state.budget == 0 {
                // Out of budget: the rest goes down as it is.
                out.push(piece);
                continue;
            }
            state.budget -= 1;
            if let Some(own) = probe.skip(piece.first as usize, piece.end as usize) {
                *lower = [lower[0].min(own), lower[1].min(own)];
                continue;
            }
            let bound = probe.bound(piece.centre, piece.radius, None);
            let mut open = false;
            for side in 0..2 {
                if bound[side] == f64::INFINITY {
                    continue;
                }
                if bound[side] >= settle[side] {
                    lower[side] = lower[side].min(bound[side]);
                } else {
                    open = true;
                }
            }
            if !open {
                continue;
            }
            if piece.radius <= probe.size() {
                out.push(piece);
            } else if piece.level as usize > 0 {
                state
                    .stack
                    .push(self.tree_piece(piece.level as usize - 1, 2 * piece.node as usize));
                if 2 * piece.node as usize + 1 < self.levels[piece.level as usize - 1].len() {
                    state.stack.push(
                        self.tree_piece(piece.level as usize - 1, 2 * piece.node as usize + 1),
                    );
                }
            } else if piece.depth < PART_DEPTH {
                let mid = 0.5 * (f64::from(piece.s0) + f64::from(piece.s1));
                let pm = self.bez[piece.first as usize / K].point(mid);
                let (p0, p1) = self.part_ends(&piece);
                let i = piece.first as usize;
                state
                    .stack
                    .push(self.part(i, f64::from(piece.s0), mid, p0, pm, piece.depth + 1));
                state
                    .stack
                    .push(self.part(i, mid, f64::from(piece.s1), pm, p1, piece.depth + 1));
            } else {
                out.push(piece);
            }
        }
    }

    /// The two ends of an open curve: (unit tangent into the curve,
    /// `ρ_back`, `ρ_half`), where, over the curve points `y` behind
    /// the end (`b = −(y − E) · inward > 0`),
    /// `ρ_back = inf |y − E|² / (2b)` (the largest ball touching the curve
    /// at the end, straight behind it, that holds none of it) and
    /// `ρ_half = inf |y − E| / 2` (the end's term in Federer's formula with
    /// the half-line tangent cone). Both are lower bounds (no margin), at
    /// most 1e3; computed between the samples, so an end pointing at a
    /// wall a fraction of a sample interval away is seen at its true
    /// distance.
    pub(super) fn end_terms(&self) -> [EndTerms; 2] {
        let last = &self.bez[self.bez.len() - 1];
        let n = self.intervals();
        let ends = [
            (self.bez[0].point(0.0), self.bez[0].d1(0.0), 0),
            (last.point(1.0), -last.d1(1.0), n),
        ];
        let mut state = State::new();
        ends.map(|(point, d1, index)| {
            let inward = d1.normalize_or_zero();
            let probe = EndProbe {
                point,
                inward,
                index,
                tree: self,
            };
            state.upper = [1.0e3; 2];
            state.done = [false; 2];
            state.budget = usize::MAX;
            let rules = Rules {
                tol: TOL_END,
                slack: 0.0,
                floor: 0.0,
                depth: END_DEPTH,
                can_split: false,
                init: [1.0e3; 2],
            };
            let root = [self.root()];
            let [back, half] = match self.search(&probe, &rules, &mut state, &root) {
                Search::Done(lower) => lower,
                // (Unreachable: no split.)
                Search::Split => [0.0; 2],
            };
            EndTerms { inward, back, half }
        })
    }
}

/// See [`CurveTree::end_terms`].
#[derive(Debug, Clone, Copy)]
pub(super) struct EndTerms {
    pub(super) inward: DVec3,
    pub(super) back: f64,
    pub(super) half: f64,
}

/// Query of [`CurveTree::end_terms`]: end point `E`, unit `inward`.
///
/// For a ball `(c, R)`: with `v = c − E`, every point has
/// `|y − E| ≥ |v| − R` and `b ≤ −v · inward + R`; if that is not positive
/// nothing in it is behind the end, else `ρ_back ≥ (|v| − R)² / (2b)` and
/// `ρ_half ≥ (|v| − R)/2`. Pieces within the own neighbourhood of the end
/// are in front of it (module documentation).
struct EndProbe<'a> {
    point: DVec3,
    inward: DVec3,
    /// Sample-interval boundary of the end (0 or the last).
    index: usize,
    tree: &'a CurveTree,
}

impl Probe for EndProbe<'_> {
    fn bound(&self, centre: DVec3, radius: f64, _mid: Option<&Mid>) -> [f64; 2] {
        let v = centre - self.point;
        let behind = -v.dot(self.inward) + radius;
        if behind <= 0.0 {
            return [f64::INFINITY; 2];
        }
        let near = v.length() - radius;
        if near <= 0.0 {
            return [0.0; 2];
        }
        let shrink = 1.0 - ROUND;
        [near * near / (2.0 * behind) * shrink, 0.5 * near * shrink]
    }

    fn value(&self, y: DVec3) -> [f64; 2] {
        let w = y - self.point;
        let b = -w.dot(self.inward);
        let w2 = w.length_squared();
        if b > 0.0 && w2 > 0.0 {
            [w2 / (2.0 * b), 0.5 * w2.sqrt()]
        } else {
            [f64::INFINITY; 2]
        }
    }

    fn skip(&self, first: usize, end: usize) -> Option<f64> {
        let near = self.index.min(self.tree.intervals() - 1);
        self.tree
            .own(
                first,
                end,
                self.index,
                self.index,
                self.tree.curvature[near],
            )
            .map(|_| f64::INFINITY)
    }

    fn at(&self) -> DVec3 {
        self.point
    }

    fn size(&self) -> f64 {
        0.0
    }
}

/// `[ρ_+, ρ_−]` of the pair `(a, y)`: `|w|² / (2 σ w · N)`, `w = y − a`,
/// where `σ w · N > 0` (infinite elsewhere).
fn side_values(a: DVec3, normal: DVec3, y: DVec3) -> [f64; 2] {
    let w = y - a;
    let w2 = w.length_squared();
    let across = w.dot(normal);
    if w2 <= 0.0 {
        return [f64::INFINITY; 2];
    }
    [
        if across > 0.0 {
            w2 / (2.0 * across)
        } else {
            f64::INFINITY
        },
        if across < 0.0 {
            w2 / (-2.0 * across)
        } else {
            f64::INFINITY
        },
    ]
}

/// Query of a table entry: every curve point `a` of a piece `A` of the curve
/// (sample intervals `first..end`), on both sides.
///
/// For side `σ = ±1` and `N_a` the unit plan normal at `a` (the tangent
/// turned left), the quantity is `|w|² / (2 σ w · N_a)` over `y` with
/// `σ w · N_a > 0`, `w = y − a`: the radius of the largest ball touching the
/// curve at `a` from side `σ` that holds none of it. A ball of radius `ρ`
/// does that exactly if `g(a, y) = |w|² − 2ρ σ w · N_a ≥ 0` for every `y`
/// (for `σ w · N_a ≤ 0` trivially), and then so does every smaller one.
///
/// `A` has its chord midpoint `m`, half its arc-length bound `λ` (every `a`
/// is within `λ` of `m`), the curve point `a_m` and normal `N_m` at its
/// mid-parameter, the largest arc length `δ_A` from there to an end of `A`,
/// its curvature bound `κ_A`, and `θ_A = κ_A δ_A`, which bounds the angle
/// between `T_a` and `T_m`, hence `|N_a − N_m|`.
///
/// Bound 1, for any ball `(c, R)` of curve points: with `v = c − m`,
/// `|w| ≥ |v| − R − λ`, `|w| ≤ |v| + R + λ =: far` and
/// `σ w · N_a ≤ σ v · N_m + R + λ + far θ_A =: up`; the quantity is at least
/// `(|v| − R − λ)² / (2 up)`, and nothing counts where `up ≤ 0`.
///
/// Bound 2, for a part `P` of a sample interval ([`Mid`]: `y_m`, `T_ym`,
/// `δ_P`, `θ_P`), sharp where `y` nearly continues the curve through `a`
/// (the far end of a C across a narrow opening), where bound 1 needs pieces
/// far smaller than the gap. Along the curve (arc length `u` at `a`, `v` at
/// `y`; `T' = κ N`, `N' = −κ T` in the plane):
/// `∂g/∂v = 2 w · T_y − 2ρσ T_y · N_a`, `∂g/∂u = −2 w · T_a (1 − ρσκ_a)`.
/// Moving first `a` from `a_m`, then `y` from `y_m` (`w_c = y_m − a_m`),
/// `|w| ≤ |w_c| + δ_A + δ_P =: far` and
/// `|T_y · N_a| ≤ |T_ym · N_m| + θ_P + θ_A =: τ` all the way, so
/// `g ≥ |w_c|² − 2 far (δ_A + δ_P) − ρ (2σ w_c · N_m + 2 far κ_A δ_A + 2τ δ_P)`,
/// linear in `ρ`: every `ρ` up to `num / den` (all of them if `den ≤ 0`)
/// keeps it non-negative.
struct TableProbe<'a> {
    middle: DVec3,
    lambda: f64,
    point: DVec3,
    normal: DVec3,
    reach: f64,
    curvature: f64,
    turn: f64,
    first: usize,
    end: usize,
    tree: &'a CurveTree,
}

impl<'a> TableProbe<'a> {
    /// The sample intervals `first..end` (a group of table intervals).
    fn span(tree: &'a CurveTree, first: usize, end: usize) -> Self {
        let mid = (first + end) / 2;
        let d1 = tree.bez[mid / K].d1((mid % K) as f64 / K as f64);
        let normal = DVec3::new(-d1.y, d1.x, 0.0).normalize_or_zero();
        let reach = (tree.arc[mid] - tree.arc[first]).max(tree.arc[end] - tree.arc[mid]);
        let curvature = tree.curvature[first..end]
            .iter()
            .fold(0.0f64, |m, &k| m.max(k));
        Self {
            middle: 0.5 * (tree.points[first] + tree.points[end]),
            lambda: 0.5 * (tree.arc[end] - tree.arc[first]),
            point: tree.points[mid],
            normal,
            reach,
            curvature,
            turn: curvature * reach,
            first,
            end,
            tree,
        }
    }

    /// The piece `[s0, s1]` of segment `seg`.
    fn new(tree: &'a CurveTree, seg: usize, s0: f64, s1: f64) -> Self {
        let b = &tree.bez[seg];
        let mid = 0.5 * (s0 + s1);
        let d1 = b.d1(mid);
        let normal = DVec3::new(-d1.y, d1.x, 0.0).normalize_or_zero();
        let (length, _) = b.bounds(s0, s1);
        let (left, k0) = b.bounds(s0, mid);
        let (right, k1) = b.bounds(mid, s1);
        let first = seg * K + (s0 * K as f64).floor() as usize;
        let end = seg * K + (s1 * K as f64).ceil() as usize;
        let curvature = tree.curvature[first..end]
            .iter()
            .fold(0.0f64, |m, &k| m.max(k))
            .min(k0.max(k1));
        let reach = left.max(right);
        Self {
            middle: 0.5 * (b.point(s0) + b.point(s1)),
            lambda: 0.5 * length,
            point: b.point(mid),
            normal,
            reach,
            curvature,
            turn: curvature * reach,
            first,
            end,
            tree,
        }
    }
}

impl Probe for TableProbe<'_> {
    fn bound(&self, centre: DVec3, radius: f64, mid: Option<&Mid>) -> [f64; 2] {
        let shrink = 1.0 - ROUND;
        if let Some(m) = mid {
            // Bound 2.
            let w = m.point - self.point;
            let d = w.length();
            let far = d + self.reach + m.reach;
            let num = d * d - 2.0 * far * (self.reach + m.reach);
            let tau = m.tangent.dot(self.normal).abs() + m.turn + self.turn;
            let room = 2.0 * far * self.curvature * self.reach + 2.0 * tau * m.reach;
            let across = 2.0 * w.dot(self.normal);
            return [across + room, -across + room].map(|den| {
                if den <= 0.0 {
                    f64::INFINITY
                } else if num <= 0.0 || den == f64::INFINITY {
                    0.0
                } else {
                    num / den * shrink
                }
            });
        }
        // Bound 1.
        let v = centre - self.middle;
        let d = v.length();
        let slack = radius + self.lambda;
        let near = d - slack;
        let far = d + slack;
        let across = v.dot(self.normal);
        let room = slack + far * self.turn;
        [across + room, -across + room].map(|up| {
            if up <= 0.0 {
                f64::INFINITY
            } else if near <= 0.0 || up == f64::INFINITY {
                0.0
            } else {
                near * near / (2.0 * up) * shrink
            }
        })
    }

    fn value(&self, y: DVec3) -> [f64; 2] {
        side_values(self.point, self.normal, y)
    }

    fn skip(&self, first: usize, end: usize) -> Option<f64> {
        self.tree
            .own(first, end, self.first, self.end, self.curvature)
    }

    fn at(&self) -> DVec3 {
        self.middle
    }

    fn size(&self) -> f64 {
        self.lambda
    }
}

/// Distance from an end of an open curve, in units of the curve parameter
/// (1 per segment), within which the nearest point counts as the end itself
/// ([`TangentBalls::radius`]). Any width is safe: the end term only ever
/// lowers the radius, so applying it to an interior point is merely
/// conservative, and a nearest point that is the end has the parameter of
/// the end exactly (the closest-point search clamps to it). The band is
/// there for rounding of the parameter, and kept narrow so that a sphere
/// sliding past the end region is not slowed down.
const END_BAND: f32 = 1.0e-3;

/// Tangent-ball radii along a level curve `C` whose reach is limited by its
/// open ends: the pointwise, one-sided form of the reach.
///
/// For a point `a` of the curve and a unit vector `n`, the open ball of
/// radius `ρ` around `a + ρ n` misses the curve exactly if `|w|² ≥ 2 ρ w · n`
/// for every `w = y − a`, `y` on the curve. Write the plan part of `n` as
/// `α N_a + γ T_a` (`T_a` the unit tangent, `N_a` the tangent turned left).
/// If `a` is the nearest point of the curve to some point in direction `n`,
/// then `γ = 0` inside the curve, and at an end `γ ≤ 0` along the tangent
/// into the curve (`s = −γ ≥ 0` with `inward` there). Hence, with
/// `σ = sign α` and `b = −w · inward`,
///
/// `w · n ≤ |α| max(0, σ w · N_a) + s max(0, b)`,
///
/// and the ball misses the curve if
///
/// `1/ρ ≥ |α| / ρ_σ(a) + s / ρ_back`,
/// `ρ_σ(a) = inf |w|² / (2 σ w · N_a)` over `σ w · N_a > 0`,
/// `ρ_back = inf |w|² / (2 b)` over `b > 0`.
///
/// The same balls miss the wall `C × [0, h]`: for a wall point above `a`
/// nearest to `p`, the vertical part of `n` points away from the wall (or
/// is zero), so `w · n ≤ w_h · n_h`, and `|w| ≥ |w_h|`.
///
/// At run time ([`TangentBalls::radius`]) `α`, `s` and the side come from
/// the actual direction; at an end `|α|` is taken as 1 and the smaller
/// side, so that the radius only shrinks the further the direction turns
/// behind the end (still a valid, smaller, bound).
///
/// Stored, each a proven lower bound (branch and bound over the curve,
/// [`CurveTree`]) times 0.9, less 1 µm:
/// * `ρ_±` per table interval: the infimum over every point `a` of the
///   interval, not at sample points, so the table holds between its points
///   whatever the spacing: intervals a quarter of a segment long, and 1/64
///   of a segment in the first and the last segment;
/// * `ρ_back` of each end.
///
/// One side of a wall does not limit the other: a ball sliding under a
/// T-junction (the stem on the other side) keeps its full radius. Values
/// are capped at [`TANGENT_BALL_CAP`] and at 0.9 of the smallest radius of
/// curvature within a segment, and stored as zero below [`RHO_FLOOR`].
#[derive(Debug, Clone)]
pub(super) struct TangentBalls {
    /// `[ρ_+, ρ_−]` per quarter segment (unused in the end segments).
    coarse: Vec<[f32; 2]>,
    /// `[ρ_+, ρ_−]` per 1/64 of the first and of the last segment.
    fine: [Vec<[f32; 2]>; 2],
    /// Unit tangent into the curve and `ρ_back` of the first and last end.
    ends: [(Vec3, f32); 2],
    /// See [`TangentBalls::smallest`].
    smallest: f32,
}

impl TangentBalls {
    /// The table of an open level curve, from its hierarchy and end terms.
    pub(super) fn new(tree: &CurveTree, ends: &[EndTerms; 2]) -> Self {
        let segments = tree.bez.len();
        let mut build = Build {
            tree,
            state: State::new(),
            caps: Vec::new(),
            middles: Vec::new(),
            total: 0,
            smallest: f64::INFINITY,
        };
        let margin = |e: [f64; 2]| e.map(stored);
        let mut coarse = vec![[0.0f32; 2]; segments * TABLE_COARSE];
        if segments > 2 {
            // Every segment but the two end ones (the fine tables cover
            // those).
            let slots = Slots {
                first: TABLE_COARSE,
                per: TABLE_COARSE,
                depth: COARSE_DEPTH,
            };
            let count = (segments - 2) * TABLE_COARSE;
            for (k, e) in build.entries(&slots, count).into_iter().enumerate() {
                coarse[TABLE_COARSE + k] = margin(e);
            }
        }
        let fine = [0, segments - 1].map(|seg| {
            let slots = Slots {
                first: seg * TABLE_FINE,
                per: TABLE_FINE,
                depth: FINE_DEPTH,
            };
            build
                .entries(&slots, TABLE_FINE)
                .into_iter()
                .map(margin)
                .collect()
        });
        let smallest = build.smallest;
        Self {
            coarse,
            fine,
            ends: ends.map(|e| (e.inward.as_vec3(), stored(e.back))),
            smallest: smallest as f32,
        }
    }

    /// The smallest radius of the table before the margin, a proven lower
    /// bound of the reach over the directions perpendicular to the curve. (Federer's reach is the
    /// infimum of the tangent-ball radii over every point and direction of
    /// the normal cone; at an end the other directions are covered by
    /// `ρ_half`, see [`CurveTree::end_terms`].)
    pub(super) fn smallest(&self) -> f32 {
        self.smallest
    }

    /// Radius of a ball that touches the wall at the point above the curve
    /// point of parameter `t`, on the side of the unit vector `normal` (from
    /// that nearest point to the query point), and holds none of the wall.
    /// Zero where there is none to promise.
    pub(super) fn radius(&self, spline: &CubicSpline, t: f32, normal: Vec3) -> f32 {
        let max_t = spline.max_t();
        let t = t.clamp(0.0, max_t);
        let in_fine = |local: f32| ((local * TABLE_FINE as f32) as usize).min(TABLE_FINE - 1);
        let fold = |rho: [f32; 2], e: [f32; 2]| [rho[0].min(e[0]), rho[1].min(e[1])];
        let mut rho = [f32::INFINITY; 2];
        let mut in_end_segment = false;
        if t <= 1.0 {
            rho = fold(rho, self.fine[0][in_fine(t)]);
            in_end_segment = true;
        }
        if t >= max_t - 1.0 {
            rho = fold(rho, self.fine[1][in_fine(t - (max_t - 1.0))]);
            in_end_segment = true;
        }
        if !in_end_segment {
            let index = (t * TABLE_COARSE as f32) as usize;
            rho = fold(rho, self.coarse[index.min(self.coarse.len() - 1)]);
        }
        let tangent = spline.tangent(t);
        let side = Vec3::new(-tangent.y, tangent.x, 0.0).normalize_or_zero();
        if side == Vec3::ZERO {
            return 0.0;
        }
        let alpha = normal.dot(side);
        let r = if alpha > 0.0 {
            rho[0]
        } else if alpha < 0.0 {
            rho[1]
        } else {
            rho[0].min(rho[1])
        };
        let mut behind = 0.0f32;
        for (end_t, (inward, back)) in [0.0, max_t].into_iter().zip(&self.ends) {
            let s = -normal.dot(*inward);
            if (t - end_t).abs() <= END_BAND && s > 0.0 {
                if *back <= 0.0 {
                    return 0.0;
                }
                behind += s / back;
            }
        }
        let inverse = if behind > 0.0 {
            // Round an end: `|α| ≤ 1` taken as 1, and the smaller side, so
            // that the radius only shrinks the further the direction turns
            // behind the end.
            let r = rho[0].min(rho[1]);
            if r <= 0.0 {
                return 0.0;
            }
            1.0 / r + behind
        } else if alpha != 0.0 {
            if r <= 0.0 {
                return 0.0;
            }
            alpha.abs() / r
        } else {
            0.0
        };
        // (No limit at all: straight above or below the wall.)
        if inverse > 0.0 {
            (1.0 / inverse).min(1.0e3)
        } else {
            1.0e3
        }
    }
}

/// Table intervals: slot `k` of a table is the piece `[s0, s1]` of a
/// segment, `per` slots per segment, the first one at global slot `first`.
struct Slots {
    first: usize,
    per: usize,
    depth: u8,
}

impl Slots {
    /// (segment, s0, s1) of slot `k`.
    fn slot(&self, k: usize) -> (usize, f64, f64) {
        let g = self.first + k;
        let s0 = (g % self.per) as f64 / self.per as f64;
        (g / self.per, s0, s0 + 1.0 / self.per as f64)
    }
}

/// Group size (slots) up to which a group's settle threshold comes from the
/// last entry's best points rather than from the cap.
const LOCAL_GROUP: usize = 64;

/// Construction state of a table.
struct Build<'a> {
    tree: &'a CurveTree,
    state: State,
    /// Cap of every slot of the current table, and the curve point and
    /// normal at its middle (for the values at the hints).
    caps: Vec<f64>,
    middles: Vec<(DVec3, DVec3)>,
    /// Pieces the rest of the current table may still look at.
    total: usize,
    /// Smallest entry so far, before the margin.
    smallest: f64,
}

impl Build<'_> {
    /// Lower bounds of `ρ_±` (at most the cap) for slots `0..count`.
    ///
    /// The slots are the leaves of a binary tree of groups; a group filters
    /// the pieces its parent left open ([`CurveTree::filter`]) once for all
    /// its slots, so that the far parts of the curve are settled for many
    /// slots at a time. A group settles a piece if its bound is at least
    /// `(1 − tol) U`, with `U` the largest of the slots' values at the
    /// last entry's best points (each an actual pair value, so at least
    /// the infimum of its slot): every slot then keeps at least
    /// `(1 − tol)` of its infimum, as with a search of its own. Large
    /// groups settle against the cap only.
    fn entries(&mut self, slots: &Slots, count: usize) -> Vec<[f64; 2]> {
        let mut out = vec![[0.0; 2]; count];
        self.total = TABLE_BUDGET_PER_SLOT * count + TABLE_BUDGET_BASE;
        // The cap of a slot: [`TANGENT_BALL_CAP`], and 0.9 of the smallest
        // radius of curvature within a segment of it (the true radius on the
        // concave side is at most that; a larger cap only makes the search
        // work harder, and the end of a natural spline is straight).
        let intervals = self.tree.intervals();
        self.caps = (0..count)
            .map(|k| {
                let (seg, s0, s1) = slots.slot(k);
                let first = (seg * K + (s0 * K as f64).floor() as usize).saturating_sub(K);
                let end = (seg * K + (s1 * K as f64).ceil() as usize + K).min(intervals);
                let kappa = self.tree.curvature_between(first, end);
                if 0.9 > kappa * TANGENT_BALL_CAP {
                    TANGENT_BALL_CAP
                } else {
                    0.9 / kappa
                }
            })
            .collect();
        self.middles = (0..count)
            .map(|k| {
                let (seg, s0, s1) = slots.slot(k);
                let b = &self.tree.bez[seg];
                let mid = 0.5 * (s0 + s1);
                let d1 = b.d1(mid);
                (
                    b.point(mid),
                    DVec3::new(-d1.y, d1.x, 0.0).normalize_or_zero(),
                )
            })
            .collect();
        let root = [self.tree.root()];
        // The first slot on its own, from the whole curve: its best points
        // are the hints of the groups that follow (the last entry's are from
        // elsewhere on the curve).
        self.state.best = [None; 2];
        self.group(slots, 0, 1, &root, [TANGENT_BALL_CAP; 2], &mut out);
        if count > 1 {
            self.group(slots, 1, count, &root, [TANGENT_BALL_CAP; 2], &mut out);
        }
        out
    }

    fn probe<'t>(tree: &'t CurveTree, slots: &Slots, from: usize, to: usize) -> TableProbe<'t> {
        let (seg0, s0, _) = slots.slot(from);
        let (seg1, _, s1) = slots.slot(to - 1);
        if seg0 == seg1 {
            TableProbe::new(tree, seg0, s0, s1)
        } else {
            // (Coarse slots are whole sample intervals.)
            let first = seg0 * K + (s0 * K as f64) as usize;
            let end = seg1 * K + (s1 * K as f64) as usize;
            TableProbe::span(tree, first, end)
        }
    }

    /// Slots `from..to`, given the pieces left open by the enclosing groups
    /// and their lower bounds.
    fn group(
        &mut self,
        slots: &Slots,
        from: usize,
        to: usize,
        list: &[Piece],
        lower: [f64; 2],
        out: &mut [[f64; 2]],
    ) {
        if to - from == 1 {
            let (seg, s0, s1) = slots.slot(from);
            let cap = self.caps[from];
            let lower = [lower[0].min(cap), lower[1].min(cap)];
            let entry = self.entry(seg, s0, s1, slots.depth, cap, list, lower);
            self.smallest = self.smallest.min(entry[0]).min(entry[1]);
            out[from] = entry;
            return;
        }
        // The level each slot settles a piece at: `(1 − tol)` of its value
        // at the last entry's best points, or the cap less 5 %; the group
        // settles at the highest of them, so that each slot keeps what its
        // own search would have.
        let at_cap = |cap: f64| (1.0 - TOL_CAP) * cap;
        let mut settle = [0.0f64; 2];
        let local = to - from <= LOCAL_GROUP;
        for k in from..to {
            let cap = self.caps[k];
            let mut level = [at_cap(cap); 2];
            if local {
                let (point, normal) = self.middles[k];
                for y in self.state.best.into_iter().flatten() {
                    let v = side_values(point, normal, y);
                    for side in 0..2 {
                        if v[side] < cap {
                            level[side] =
                                level[side].min(settle_level(v[side], TOL_TABLE, TABLE_SLACK));
                        }
                    }
                }
            }
            settle = [settle[0].max(level[0]), settle[1].max(level[1])];
        }
        let probe = Self::probe(self.tree, slots, from, to);
        let mut lower = lower;
        let mut open = Vec::new();
        self.state.budget = self.total;
        self.tree
            .filter(&probe, settle, list, &mut lower, &mut open, &mut self.state);
        self.total = self.state.budget;
        let mid = from + (to - from) / 2;
        self.group(slots, from, mid, &open, lower, out);
        self.group(slots, mid, to, &open, lower, out);
    }

    /// One table entry: lower bounds of `ρ_±` over the piece `[s0, s1]` of
    /// segment `seg`, at most `lower` (the bounds of the enclosing groups),
    /// searching the pieces they left open; the piece is halved up to
    /// `depth` times where its own size keeps a bound low. At most
    /// [`SLOT_BUDGET`] pieces are looked at; then the bounds found so far
    /// are kept as they are.
    #[allow(clippy::too_many_arguments)]
    fn entry(
        &mut self,
        seg: usize,
        s0: f64,
        s1: f64,
        depth: u8,
        cap: f64,
        list: &[Piece],
        lower: [f64; 2],
    ) -> [f64; 2] {
        let tree = self.tree;
        let allowed = self.total.min(SLOT_BUDGET);
        let state = &mut self.state;
        state.budget = allowed;
        let hints = state.best;
        state.upper = [cap; 2];
        state.best = [None; 2];
        state.done = [false; 2];
        let mut result = lower;
        let mut pieces: Vec<(f64, f64, u8)> = vec![(s0, s1, 0)];
        while let Some((s0, s1, level)) = pieces.pop() {
            let probe = TableProbe::new(tree, seg, s0, s1);
            // The points that settled the previous entry are usually near
            // the ones that settle this one: a good value to prune with.
            for y in hints.into_iter().flatten() {
                state.see(y, probe.value(y));
            }
            let rules = Rules {
                tol: TOL_TABLE,
                slack: TABLE_SLACK,
                floor: RHO_FLOOR,
                depth: PART_DEPTH,
                can_split: level < depth,
                init: [cap; 2],
            };
            match tree.search(&probe, &rules, state, list) {
                Search::Done(found) => {
                    result = [result[0].min(found[0]), result[1].min(found[1])];
                }
                Search::Split => {
                    let mid = 0.5 * (s0 + s1);
                    pieces.push((s0, mid, level + 1));
                    pieces.push((mid, s1, level + 1));
                }
            }
        }
        for (low, done) in result.iter_mut().zip(state.done) {
            if done {
                *low = 0.0;
            }
        }
        self.total -= allowed - self.state.budget;
        result
    }
}
