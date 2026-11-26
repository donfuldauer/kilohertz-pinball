//! Ramp entry zones and the layer-exit lint (w7pre-ramp-entry).
//!
//! # Entry zone
//!
//! A ramp whose spline is on layer `N > 0` gets a `0 -> N` transition
//! `tablegen:ramps[i].entry`. Its zone is the tight axis-aligned box of an
//! oriented box that starts where the rails start (the spline's first
//! through-point: a wireform's rails at `t = 0` are centred on it) and lies
//! along the spline's start tangent `u`:
//!
//! * length `L = entry_width` along `u` (from the start `p0` to
//!   `p1 = p0 + L u`): a square plan for a level start, so a ball travels a
//!   full entry width inside the zone whatever its heading;
//! * `entry_width` across, centred on the spline, horizontally
//!   perpendicular to `u`'s plan direction;
//! * in height from `min(p0.z, p1.z, 0)` to `max(p0.z, p1.z, 0) +
//!   entry_width / 2`: from the playfield (or the rails, if lower) to half
//!   an entry width above the higher of the playfield and the rails, so a
//!   ball rolling on the playfield (centre one radius up) is inside it.
//!
//! The zone never reaches behind the start by more than `entry_width / 2`
//! (in plan, along `-u`): the box itself does not, and its axis-aligned
//! bounds add corner triangles that reach `(entry_width / 2)·|sin 2θ|`
//! behind the start for a plan heading `θ` from an axis (0 for an
//! axis-aligned start, `entry_width / 2` at 45°). The generator refuses a
//! zone that overlaps any `N -> 0` transition of the table
//! ([`overlaps`]), so the overshoot can never reach a fallback zone.
//!
//! # Layer-exit lint
//!
//! [`layer_exit_warnings`]: a layer `N > 0` that some transition leads to
//! must have an *exit zone over unsupported area*: a transition from layer
//! `N` (to another layer) whose plan footprint has at least one sample
//! point (the 3 × 3 grid of its corners, edge midpoints and centre) with no
//! layer-`N` support under it. Support is a collider triangle on
//! layer `N` (a deck; walls and posts have no plan area) whose plan
//! projection contains the point and whose lowest vertex is not above the
//! zone's top. Triangles under 1 mm² of plan area are ignored. Otherwise a
//! ball that leaves layer `N`'s support falls through the playfield: every
//! transition leading to `N` gets a warning at its `layer_transitions[i]`
//! path. Loops (`from == to`) and transitions to layer 0 never warn.
//! Warnings never fail generation or validation.
//!
//! The rule has one implementation, [`layer_exit_findings`], which takes
//! the support triangles from its caller. It runs in two places with the
//! same message text: the generator ([`layer_exit_warnings`], over the
//! generated collider nodes at their glb positions), and
//! `pin_tablegen validate <bundle>` (`clearance::checks::layer_exits`, a
//! `Feature::LayerExit` warning, over the bundle's node-transformed
//! collider triangles, backbox colliders included), so hand-edited and
//! older bundles are covered too (#137). Generated nodes have identity
//! transforms, so both give the same verdict on a generated bundle.

use super::glb::Node;
use glam::{DVec2, DVec3, Vec3};
use pin_assets::ValidationError;
use pin_assets::validate::TABLE_FILE;
use pin_core::fmath::{max_f64, min_f64};
use pin_core::schema::table::{AabbDef, LayerTransitionDef};
use pin_core::units::mm_to_m;

/// Smallest plan component of the unit start tangent: below it the spline
/// starts vertically and has no plan direction to lay the zone along.
const MIN_PLAN_TANGENT: f64 = 1.0e-6;

/// The entry zone (mm) of a ramp whose spline starts at `p0` (mm) with
/// start tangent `tangent` (any length), for `width` mm (see the
/// [module docs](self)). The caller checks it is not empty after `f32`.
///
/// # Errors
/// A message when the start tangent has no plan direction (or is not
/// finite).
pub(super) fn entry_zone(p0: [f32; 3], tangent: Vec3, width: f32) -> Result<AabbDef, String> {
    let t = tangent.as_dvec3();
    let len = t.length();
    if !(len.is_finite() && len > 0.0) {
        return Err(format!("has no start direction (tangent {tangent:?})"));
    }
    let u = t / len;
    let plan = DVec2::new(u.x, u.y);
    if plan.length() <= MIN_PLAN_TANGENT {
        return Err(format!(
            "starts vertically (tangent {tangent:?}); the entry zone lies along the start \
             tangent's plan direction, so the spline must start with a horizontal component"
        ));
    }
    let d = plan.normalize();
    let across = DVec2::new(-d.y, d.x);
    let w = f64::from(width);
    let half = 0.5 * w;
    let s = DVec3::new(f64::from(p0[0]), f64::from(p0[1]), f64::from(p0[2]));
    let e = s + u * w;
    let mut lo = DVec2::splat(f64::INFINITY);
    let mut hi = DVec2::splat(f64::NEG_INFINITY);
    for c in [s, e] {
        for k in [-half, half] {
            let q = DVec2::new(c.x, c.y) + across * k;
            lo = DVec2::new(min_f64(lo.x, q.x), min_f64(lo.y, q.y));
            hi = DVec2::new(max_f64(hi.x, q.x), max_f64(hi.y, q.y));
        }
    }
    let z_lo = min_f64(min_f64(s.z, e.z), 0.0);
    let z_hi = max_f64(max_f64(s.z, e.z), 0.0) + half;
    Ok(AabbDef {
        min: [lo.x as f32, lo.y as f32, z_lo as f32],
        max: [hi.x as f32, hi.y as f32, z_hi as f32],
    })
}

/// Whether two zones share a point (bounds inclusive, as
/// `LayerTransition::contains`).
pub(super) fn overlaps(a: &AabbDef, b: &AabbDef) -> bool {
    (0..3).all(|k| a.min[k] <= b.max[k] && b.min[k] <= a.max[k])
}

/// One plan triangle of a collider, m, with its lowest vertex height: the
/// unit of support the layer-exit lint reads (see the [module docs](self)).
pub(crate) struct Support {
    tri: [DVec2; 3],
    z_min: f64,
}

impl Support {
    /// The support a collider triangle with vertices `v` (m, `[x, y, z]`)
    /// gives, or `None` when its plan area is below 1 mm² (a vertical wall,
    /// a post side or a sliver supports nothing).
    pub(crate) fn new(v: [[f64; 3]; 3]) -> Option<Support> {
        let tri = v.map(|q| DVec2::new(q[0], q[1]));
        let area2 = (tri[1] - tri[0]).perp_dot(tri[2] - tri[0]).abs();
        // A NaN area (a non-finite vertex) supports nothing either.
        if area2.is_nan() || area2 < MIN_SUPPORT_AREA2_M2 {
            return None;
        }
        let z_min = v.iter().map(|q| q[2]).fold(f64::INFINITY, min_f64);
        Some(Support { tri, z_min })
    }

    /// Whether the triangle's plan projection contains `q` (edges
    /// included).
    fn covers(&self, q: DVec2) -> bool {
        let [a, b, c] = self.tri;
        let d1 = (b - a).perp_dot(q - a);
        let d2 = (c - b).perp_dot(q - b);
        let d3 = (a - c).perp_dot(q - c);
        (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
    }
}

/// Plan area (m², doubled) below which a collider triangle supports
/// nothing (a vertical wall or a sliver): 1 mm².
const MIN_SUPPORT_AREA2_M2: f64 = 2.0e-6;

/// Layer-`layer` collider triangles of the generated `nodes` with a plan
/// area, read at their glb positions (generated nodes have identity
/// transforms).
fn supports(nodes: &[Node], layer: u8) -> Vec<Support> {
    let mut out = Vec::new();
    for n in nodes
        .iter()
        .filter(|n| n.extras.collider && n.extras.collision_layer == layer)
    {
        let p = &n.mesh.positions;
        for t in n.mesh.indices.chunks_exact(3) {
            let v = [p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]];
            if let Some(s) = Support::new(v.map(|q| q.map(f64::from))) {
                out.push(s);
            }
        }
    }
    out
}

/// Whether some point of `zone`'s plan 3 × 3 sample grid (mm) has no
/// support in `sup` at or below the zone's top.
fn over_unsupported(zone: &AabbDef, sup: &[Support]) -> bool {
    let top = f64::from(mm_to_m(zone.max[2]));
    let at = |k: usize, f: f64| {
        let (a, b) = (f64::from(zone.min[k]), f64::from(zone.max[k]));
        (a + (b - a) * f) * 1.0e-3
    };
    let fractions = [0.0, 0.5, 1.0];
    fractions.iter().any(|&fx| {
        fractions.iter().any(|&fy| {
            let q = DVec2::new(at(0, fx), at(1, fy));
            !sup.iter().any(|s| s.z_min <= top && s.covers(q))
        })
    })
}

/// The layer-exit lint's message for transition `t` leading to layer `n`
/// (`t.to_layer`), the same text from the generator and from
/// `pin_tablegen validate`.
fn layer_exit_message(t: &LayerTransitionDef) -> String {
    let n = t.to_layer;
    format!(
        "layer transition {:?} ({} -> {n}) leads to layer {n}, which has no exit zone \
         (a transition from layer {n}) over unsupported area: a ball that leaves \
         layer {n}'s decks or rails is never returned to another layer and falls \
         through the playfield; add a {n} -> 0 zone where layer {n} has no floor",
        t.id, t.from_layer
    )
}

/// The layer-exit lint's core (see the [module docs](self)), shared by
/// the generator ([`layer_exit_warnings`]) and `pin_tablegen validate`
/// (`clearance::checks::layer_exits`): `(i, message)` per transition `i`
/// of `transitions` leading to a layer without an exit zone over
/// unsupported area, in transition order. `supports_of(n)` gives layer
/// `n`'s support triangles ([`Support::new`]); it is called at most once
/// per layer, and only for layers some transition leads to.
pub(crate) fn layer_exit_findings(
    transitions: &[LayerTransitionDef],
    mut supports_of: impl FnMut(u8) -> Vec<Support>,
) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    // Each layer's verdict is computed once.
    let mut verdict: [Option<bool>; 256] = [None; 256];
    for (i, t) in transitions.iter().enumerate() {
        let n = t.to_layer;
        if n == 0 || t.from_layer == n {
            continue;
        }
        let has_exit = *verdict[usize::from(n)].get_or_insert_with(|| {
            let sup = supports_of(n);
            transitions
                .iter()
                .filter(|x| x.from_layer == n && x.to_layer != n)
                .any(|x| over_unsupported(&x.zone, &sup))
        });
        if !has_exit {
            out.push((i, layer_exit_message(t)));
        }
    }
    out
}

/// The layer-exit lint over the table's `transitions` and the generated
/// collider `nodes` (see the [module docs](self)): one warning at
/// `layer_transitions[i]` per transition `i` leading to a layer without an
/// exit zone over unsupported area, in transition order.
pub(super) fn layer_exit_warnings(
    transitions: &[LayerTransitionDef],
    nodes: &[Node],
) -> Vec<ValidationError> {
    layer_exit_findings(transitions, |n| supports(nodes, n))
        .into_iter()
        .map(|(i, message)| {
            ValidationError::new(TABLE_FILE, format!("layer_transitions[{i}]"), message)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC §4 4.3 (#137): triangles under 1 mm² of plan area (doubled
    /// area below 2e-6 m²), vertical ones and non-finite ones support
    /// nothing; the lowest vertex is the support height.
    #[test]
    fn support_needs_one_square_millimetre_of_plan() {
        let tri = |w: f64, h: f64| [[0.0, 0.0, 0.3], [w, 0.0, 0.1], [0.0, h, 0.2]];
        // 1.0 mm² exactly (2e-6 doubled) supports; just below does not.
        let s = Support::new(tri(2.0e-3, 1.0e-3)).unwrap();
        assert_eq!(s.z_min, 0.1);
        assert!(s.covers(DVec2::new(1.0e-4, 1.0e-4)));
        assert!(s.covers(DVec2::ZERO), "edges included");
        assert!(!s.covers(DVec2::new(2.0e-3, 1.0e-3)));
        assert!(Support::new(tri(1.99e-3, 1.0e-3)).is_none());
        // A vertical wall triangle has no plan area.
        assert!(Support::new([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.5, 0.0, 1.0]]).is_none());
        assert!(Support::new(tri(f64::NAN, 1.0)).is_none());
    }

    fn t(id: &str, from: u8, to: u8, lo: f32, hi: f32) -> LayerTransitionDef {
        LayerTransitionDef {
            id: id.to_owned(),
            from_layer: from,
            to_layer: to,
            zone: AabbDef {
                min: [lo, lo, 0.0],
                max: [hi, hi, 50.0],
            },
        }
    }

    /// SPEC §4 4.3 (#137): the shared core asks for each target layer's
    /// support once, never for layer 0 or a loop's layer alone, and
    /// reports in transition order.
    #[test]
    fn core_reads_each_layer_once() {
        let ts = [
            t("a", 0, 2, 0.0, 10.0),
            t("loop", 4, 4, 0.0, 10.0),
            t("b", 1, 2, 20.0, 30.0),
            t("down", 3, 0, 0.0, 10.0),
            t("c", 0, 3, 40.0, 50.0),
        ];
        let mut asked = Vec::new();
        let found = layer_exit_findings(&ts, |n| {
            asked.push(n);
            Vec::new()
        });
        assert_eq!(asked, [2, 3]);
        // Layer 2 has no exit; layer 3's exit is over nothing (no support).
        let paths: Vec<usize> = found.iter().map(|(i, _)| *i).collect();
        assert_eq!(paths, [0, 2]);
        assert!(
            found[1]
                .1
                .starts_with("layer transition \"b\" (1 -> 2) leads to layer 2")
        );
        assert!(
            found[1]
                .1
                .ends_with("add a 2 -> 0 zone where layer 2 has no floor")
        );
    }
}
