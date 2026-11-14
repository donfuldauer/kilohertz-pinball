//! Headroom under upper decks.

use super::checks::World;
use super::geom::{CEILING_NZ, Tri, WALKABLE_NZ};
use super::{Ball, Feature, Finding, MIN_HEADROOM_GAP_MM, Sink, mm};
use glam::{DVec2, DVec3};
use std::collections::BTreeMap;

/// Largest number of plan samples per (support, ceiling) triangle pair.
const MAX_PAIR_SAMPLES: f64 = 40_000.0;
/// Coarsest sample spacing, m.
const MAX_SPACING_M: f64 = 0.01;
/// Penetration (m) of a wall that still lets a ball rest at a sample.
const WALL_TOLERANCE_M: f64 = 1.0e-4;

/// Extra plan samples (m) covering the areas a restricted ball is limited
/// to: vertices, edge points and an interior grid, at most `spacing` apart
/// and fine enough for areas much smaller than the grid, so a small area
/// under a low deck is never missed.
fn area_samples(ball: &Ball, spacing: f64) -> Vec<DVec2> {
    let mut out = Vec::new();
    let super::Allowed::Only(entries) = &ball.allowed else {
        return out;
    };
    for poly in entries.iter().filter_map(|(_, a)| a.as_ref()) {
        let pts = &poly.points;
        let mut lo = DVec2::splat(f64::INFINITY);
        let mut hi = DVec2::splat(f64::NEG_INFINITY);
        for &p in pts {
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let extent = (hi - lo).max_element();
        if !extent.is_finite() {
            continue;
        }
        let h = spacing.min(extent / 16.0).max(extent / 256.0).max(1.0e-6);
        out.extend_from_slice(pts);
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            let k = ((a.distance(b) / h).ceil() as usize).clamp(1, 4096);
            out.extend((1..k).map(|j| a.lerp(b, j as f64 / k as f64)));
        }
        let nx = ((hi.x - lo.x) / h).ceil() as usize;
        let ny = ((hi.y - lo.y) / h).ceil() as usize;
        for j in 0..=ny {
            for i in 0..=nx {
                let q = DVec2::new(
                    (lo.x + i as f64 * h).min(hi.x),
                    (lo.y + j as f64 * h).min(hi.y),
                );
                if poly.contains(q) {
                    out.push(q);
                }
            }
        }
    }
    out
}

/// The smallest headroom found under one ceiling mesh for one ball.
struct Worst {
    gap: f64,
    support_mesh: usize,
    support_layer: u8,
    ceiling_layer: u8,
}

/// Whether plan point `q` at support height `z` is in a layer transition
/// zone between layers `a` and `b` (either direction).
fn in_transition(w: &World<'_>, q: DVec2, z: f64, height: f64, a: u8, b: u8) -> bool {
    w.table.transitions.iter().any(|t| {
        let pair = (t.from_layer == a && t.to_layer == b) || (t.from_layer == b && t.to_layer == a);
        let lo = DVec3::from_array(t.zone_min_m.map(f64::from));
        let hi = DVec3::from_array(t.zone_max_m.map(f64::from));
        pair && q.x >= lo.x
            && q.x <= hi.x
            && q.y >= lo.y
            && q.y <= hi.y
            && hi.z >= z
            && lo.z <= z + height
    })
}

/// Whether a ball of radius `r` centred at `c` clears the wall triangles
/// (steeper than 60°) of `layer`.
fn laterally_free(w: &World<'_>, layer: u8, c: DVec3, r: f64, idx: &mut Vec<u32>) -> bool {
    let reach = DVec2::splat(r);
    w.grid
        .query(w.tris, c.truncate() - reach, c.truncate() + reach, idx);
    idx.iter().all(|&i| {
        let t: &Tri = &w.tris[i as usize];
        t.layer != layer
            || t.nz() >= CEILING_NZ
            || t.min.z > c.z + r
            || t.max.z < c.z - r
            || t.distance(c) >= r - WALL_TOLERANCE_M
    })
}

/// Most samples along one axis of a (support, ceiling) pair.
const MAX_AXIS_SAMPLES: f64 = 1024.0;

/// Headroom under non-steep colliders of other layers above every walkable
/// surface a ball may use: the ball resting on the support (centre at the
/// support plus `r`) must stay `r + MIN_CLEARANCE_MM` from every such
/// ceiling triangle (sphere test, so deck edges, slopes and low vertices
/// count, not only the vertical gap at the centre). The reported headroom
/// is `r + distance` (the vertical gap under a flat deck).
pub(super) fn decks(w: &World<'_>, balls: &[Ball], sink: &mut Sink) {
    let min_gap = f64::from(MIN_HEADROOM_GAP_MM) * 1.0e-3;
    let mut cands = Vec::new();
    let mut idx = Vec::new();
    for ball in balls {
        let r = ball.r();
        let need = ball.needed();
        let reach = need - r; // distance the ceiling must keep from the centre
        let spacing = (0.5 * r).min(MAX_SPACING_M);
        let mut worst: BTreeMap<usize, Worst> = BTreeMap::new();
        let extra = area_samples(ball, spacing);
        let pad = DVec2::splat(reach);
        for s in w.tris.iter() {
            if s.nz() < WALKABLE_NZ || !ball.allowed.layer(s.layer) {
                continue;
            }
            w.grid.query(
                w.tris,
                s.min.truncate() - pad,
                s.max.truncate() + pad,
                &mut cands,
            );
            for &ci in &cands {
                let c = &w.tris[ci as usize];
                if c.layer == s.layer
                    || c.nz() < CEILING_NZ
                    || c.max.z <= s.min.z + min_gap
                    || c.min.z >= s.max.z + need
                {
                    continue;
                }
                // Support points whose ball can reach the ceiling.
                let lo = s.min.truncate().max(c.min.truncate() - pad);
                let hi = s.max.truncate().min(c.max.truncate() + pad);
                if !(lo.x <= hi.x && lo.y <= hi.y) {
                    continue;
                }
                let size = hi - lo;
                let mut h = spacing
                    .max(size.x / MAX_AXIS_SAMPLES)
                    .max(size.y / MAX_AXIS_SAMPLES);
                if (size.x / h + 1.0) * (size.y / h + 1.0) > MAX_PAIR_SAMPLES {
                    h = h.max((size.x * size.y / MAX_PAIR_SAMPLES).sqrt());
                }
                if !(h.is_finite() && h > 0.0) {
                    continue;
                }
                let nx = (size.x / h).ceil().min(MAX_AXIS_SAMPLES) as usize;
                let ny = (size.y / h).ceil().min(MAX_AXIS_SAMPLES) as usize;
                let centre = (lo + hi) * 0.5;
                let grid = (0..=ny).flat_map(|j| {
                    (0..=nx).map(move |i| {
                        DVec2::new(
                            (lo.x + i as f64 * h).min(hi.x),
                            (lo.y + j as f64 * h).min(hi.y),
                        )
                    })
                });
                let in_box = |q: &DVec2| q.x >= lo.x && q.x <= hi.x && q.y >= lo.y && q.y <= hi.y;
                let from_area = extra.iter().copied().filter(in_box);
                // The ceiling's own corners (a low vertex) and, under them,
                // the support.
                let corners = c.p.iter().map(|v| v.truncate().clamp(lo, hi));
                for q in core::iter::once(centre)
                    .chain(corners)
                    .chain(grid)
                    .chain(from_area)
                {
                    if !s.contains_plan(q) {
                        continue;
                    }
                    let zs = s.z_at(q);
                    let centre3 = DVec3::new(q.x, q.y, zs + r);
                    let cp = c.closest(centre3);
                    if cp.z <= zs + min_gap {
                        continue;
                    }
                    let gap = r + cp.distance(centre3);
                    if gap.is_nan()
                        || gap >= need
                        || worst.get(&c.mesh).is_some_and(|e| e.gap <= gap)
                    {
                        continue;
                    }
                    if !ball.allowed.at(s.layer, q)
                        || in_transition(w, q, zs, ball.d, s.layer, c.layer)
                        || !laterally_free(w, s.layer, centre3, r, &mut idx)
                    {
                        continue;
                    }
                    worst.insert(
                        c.mesh,
                        Worst {
                            gap,
                            support_mesh: s.mesh,
                            support_layer: s.layer,
                            ceiling_layer: c.layer,
                        },
                    );
                }
            }
        }
        for (mesh, e) in worst {
            let node = w.scene.meshes.get(mesh).map_or("?", |m| m.name.as_str());
            let under = w
                .scene
                .meshes
                .get(e.support_mesh)
                .map_or("?", |m| m.name.as_str());
            sink.error(Finding {
                feature: Feature::DeckHeadroom,
                name: node,
                ball: &ball.name,
                file: w.glb,
                path: format!("meshes[{mesh}]"),
                required_m: need,
                actual_m: e.gap,
                message: format!(
                    "deck headroom: {node:?} (layer {}) leaves {} of headroom over {under:?} (layer {}) where ball {:?} ({}) may roll (ball-to-deck distance plus radius); it needs {}",
                    e.ceiling_layer,
                    mm(e.gap),
                    e.support_layer,
                    ball.name,
                    mm(ball.d),
                    mm(need)
                ),
            });
        }
    }
}
