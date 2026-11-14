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

/// Headroom under non-steep colliders of other layers above every walkable
/// surface a ball may use.
pub(super) fn decks(w: &World<'_>, balls: &[Ball], sink: &mut Sink) {
    let min_gap = f64::from(MIN_HEADROOM_GAP_MM) * 1.0e-3;
    let mut cands = Vec::new();
    let mut idx = Vec::new();
    for ball in balls {
        let r = ball.r();
        let need = ball.needed();
        let spacing = (0.5 * r).min(MAX_SPACING_M);
        let mut worst: BTreeMap<usize, Worst> = BTreeMap::new();
        for s in w.tris.iter() {
            if s.nz() < WALKABLE_NZ || !ball.allowed.layer(s.layer) {
                continue;
            }
            w.grid
                .query(w.tris, s.min.truncate(), s.max.truncate(), &mut cands);
            for &ci in &cands {
                let c = &w.tris[ci as usize];
                if c.layer == s.layer
                    || c.nz() < CEILING_NZ
                    || c.max.z <= s.min.z + min_gap
                    || c.min.z >= s.max.z + need
                {
                    continue;
                }
                let lo = s.min.truncate().max(c.min.truncate());
                let hi = s.max.truncate().min(c.max.truncate());
                if lo.x > hi.x || lo.y > hi.y {
                    continue;
                }
                let size = hi - lo;
                let mut h = spacing;
                if (size.x / h + 1.0) * (size.y / h + 1.0) > MAX_PAIR_SAMPLES {
                    h = (size.x * size.y / MAX_PAIR_SAMPLES).sqrt().max(spacing);
                }
                let nx = (size.x / h).ceil() as usize;
                let ny = (size.y / h).ceil() as usize;
                let centre = (lo + hi) * 0.5;
                let grid = (0..=ny).flat_map(|j| {
                    (0..=nx).map(move |i| {
                        DVec2::new(
                            (lo.x + i as f64 * h).min(hi.x),
                            (lo.y + j as f64 * h).min(hi.y),
                        )
                    })
                });
                for q in core::iter::once(centre).chain(grid) {
                    if !(s.contains_plan(q) && c.contains_plan(q)) {
                        continue;
                    }
                    let zs = s.z_at(q);
                    let gap = c.z_at(q) - zs;
                    if !(gap > min_gap && gap < need) {
                        continue;
                    }
                    if worst.get(&c.mesh).is_some_and(|e| e.gap <= gap) {
                        continue;
                    }
                    if !ball.allowed.at(s.layer, q)
                        || in_transition(w, q, zs, ball.d, s.layer, c.layer)
                        || !laterally_free(w, s.layer, DVec3::new(q.x, q.y, zs + r), r, &mut idx)
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
                    "deck headroom: {node:?} (layer {}) is {} above {under:?} (layer {}) where ball {:?} ({}) may roll; it needs {}",
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
