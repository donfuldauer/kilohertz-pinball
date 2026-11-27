//! Headroom under upper decks.

use super::checks::World;
use super::geom::{CEILING_NZ, Filter, LayerSel, Slope, Tri, WALKABLE_NZ};
use super::{Ball, Feature, Finding, MIN_HEADROOM_GAP_MM, Sink, mm};
use glam::{DVec2, DVec3};
use pin_core::fmath::{max_f64, min_f64};
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
            lo = DVec2::new(min_f64(lo.x, p.x), min_f64(lo.y, p.y));
            hi = DVec2::new(max_f64(hi.x, p.x), max_f64(hi.y, p.y));
        }
        let span = hi - lo;
        let extent = max_f64(span.x, span.y);
        if !extent.is_finite() {
            continue;
        }
        let h = max_f64(
            max_f64(min_f64(spacing, extent / 16.0), extent / 256.0),
            1.0e-6,
        );
        // Vertices and edge points always (a needle-thin spike has no
        // interior grid point), at most `spacing` apart along each edge.
        out.extend_from_slice(pts);
        let edge_step = min_f64(h, spacing);
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            let k = ((a.distance(b) / edge_step).ceil() as usize).clamp(1, 4096);
            out.extend((1..k).map(|j| a.lerp(b, j as f64 / k as f64)));
        }
        let nx = ((hi.x - lo.x) / h).ceil() as usize;
        let ny = ((hi.y - lo.y) / h).ceil() as usize;
        for j in 0..=ny {
            for i in 0..=nx {
                let q = DVec2::new(
                    min_f64(lo.x + i as f64 * h, hi.x),
                    min_f64(lo.y + j as f64 * h, hi.y),
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
    let walls = Filter {
        layer: LayerSel::Only(layer),
        slopes: Slope::Wall.bit(),
        z_lo: c.z - r,
        z_hi: c.z + r,
    };
    w.index.query(
        w.tris,
        c.truncate() - reach,
        c.truncate() + reach,
        &walls,
        idx,
    );
    idx.iter().all(|&i| {
        w.count(1);
        let t: &Tri = &w.tris[i as usize];
        t.layer != layer
            || t.nz() >= CEILING_NZ
            || t.min.z > c.z + r
            || t.max.z < c.z - r
            || t.distance(c) >= r - WALL_TOLERANCE_M
    })
}

/// Centre of a ball of radius `r` resting on support `s` above plan point
/// `q`: `r / |n·z|` above the support plane, so the sphere is tangent to an
/// inclined support instead of sinking into it.
pub(super) fn resting_centre(s: &Tri, q: DVec2, r: f64) -> DVec3 {
    DVec3::new(q.x, q.y, s.z_at(q) + r / s.nz())
}

/// Most samples along one axis of a (support, ceiling) pair.
const MAX_AXIS_SAMPLES: f64 = 1024.0;

/// Lower bound on the headroom `r + cp.distance(centre)` that the sample
/// loop of [`worst_under`] computes (f64, rounding included) for any
/// sample of support `s` under ceiling `c` at a plan point of `lo..=hi`,
/// given `centre_z`, the bounds on the computed resting-centre height over
/// `s` (from [`Tri::z_at_bounds`] plus `r / s.nz()`, added with the same
/// operation as [`resting_centre`]). NaN samples are outside the claim
/// (they never update a result).
///
/// Only a level ceiling ([`Tri::level_z`]) gives more than `r`: every
/// closest point on it has height `h` exactly, so the computed vertical
/// offset `|centre.z - h|` is at least `D = h - centre_z.1` (ceiling above
/// every centre) or `centre_z.0 - h` (below), both rounded the same way as
/// the sample's subtraction and monotone in their operands; the plan
/// offsets are at least [`Tri::level_plan_clearance`]. The sample's
/// squared distance sums the same three squares in the same order, each
/// at least the bound's, so it is at least the bound's sum; the square
/// root and the final `r +` are the very operations of the sample (glam's
/// `length`), so the bound is exact when the sample's plan offset
/// vanishes, which makes exact ties prunable.
///
/// With `plan` `None`, the plan offsets are taken as zero: a weaker bound,
/// cheaper to compute, never above the full one.
fn headroom_floor(r: f64, centre_z: (f64, f64), c: &Tri, plan: Option<(DVec2, DVec2)>) -> f64 {
    let Some(h) = c.level_z() else {
        return r;
    };
    let d = if h >= centre_z.1 {
        h - centre_z.1
    } else if h <= centre_z.0 {
        centre_z.0 - h
    } else {
        0.0
    };
    let (gx, gy) = plan.map_or((0.0, 0.0), |(lo, hi)| c.level_plan_clearance(lo, hi));
    r + DVec3::new(gx, gy, d).length()
}

/// Called for each support a ball may use with its index in `w.tris`, the
/// bounds on its computed resting-centre height, the ceiling height limit
/// `top` and its ceiling candidates.
type PerSupport<'a> = dyn FnMut(usize, (f64, f64), f64, &[u32]) + 'a;

/// Everything the per-pair sample scan needs about one ball.
struct Scan<'a> {
    ball: &'a Ball,
    r: f64,
    spacing: f64,
    min_gap: f64,
    /// [`area_samples`] of the ball.
    extra: Vec<DVec2>,
}

/// The plan box of support points of `s` whose ball can reach ceiling `c`
/// (`s`'s plan box against `c`'s grown by `pad`), or `None` when empty.
fn reach_box(s: &Tri, c: &Tri, pad: DVec2) -> Option<(DVec2, DVec2)> {
    let (s_lo, c_lo) = (s.min.truncate(), c.min.truncate() - pad);
    let lo = DVec2::new(max_f64(s_lo.x, c_lo.x), max_f64(s_lo.y, c_lo.y));
    let (s_hi, c_hi) = (s.max.truncate(), c.max.truncate() + pad);
    let hi = DVec2::new(min_f64(s_hi.x, c_hi.x), min_f64(s_hi.y, c_hi.y));
    (lo.x <= hi.x && lo.y <= hi.y).then_some((lo, hi))
}

/// Whether the narrow phase skips ceiling `c` for support `s` at once
/// (same layer, steep, or outside `s.min.z + min_gap ..= top`).
fn not_a_ceiling(s: &Tri, c: &Tri, min_gap: f64, top: f64) -> bool {
    c.layer == s.layer || c.nz() < CEILING_NZ || c.max.z <= s.min.z + min_gap || c.min.z >= top
}

/// Samples support `s` under ceiling `c` over the plan box `lo..=hi` in
/// order and returns the last (so the lowest) headroom that went below
/// `limit`, lowering `limit` to each such headroom as it goes, among the
/// samples where the ball may rest (allowed, not in a transition zone,
/// laterally free). `None` when no sample went below `limit`.
fn scan_pair(
    w: &World<'_>,
    k: &Scan<'_>,
    (s, c): (&Tri, &Tri),
    (lo, hi): (DVec2, DVec2),
    mut limit: f64,
    idx: &mut Vec<u32>,
) -> Option<f64> {
    let r = k.r;
    let size = hi - lo;
    let mut h = max_f64(
        max_f64(k.spacing, size.x / MAX_AXIS_SAMPLES),
        size.y / MAX_AXIS_SAMPLES,
    );
    if (size.x / h + 1.0) * (size.y / h + 1.0) > MAX_PAIR_SAMPLES {
        h = max_f64(h, (size.x * size.y / MAX_PAIR_SAMPLES).sqrt());
    }
    if !(h.is_finite() && h > 0.0) {
        return None;
    }
    let nx = min_f64((size.x / h).ceil(), MAX_AXIS_SAMPLES) as usize;
    let ny = min_f64((size.y / h).ceil(), MAX_AXIS_SAMPLES) as usize;
    let centre = (lo + hi) * 0.5;
    let grid = (0..=ny).flat_map(|j| {
        (0..=nx).map(move |i| {
            DVec2::new(
                min_f64(lo.x + i as f64 * h, hi.x),
                min_f64(lo.y + j as f64 * h, hi.y),
            )
        })
    });
    let in_box = |q: &DVec2| q.x >= lo.x && q.x <= hi.x && q.y >= lo.y && q.y <= hi.y;
    let from_area = k.extra.iter().copied().filter(in_box);
    // The ceiling's own corners (a low vertex) and, under them, the
    // support.
    let corners = c.p.iter().map(|v| {
        // glam's clamp order: max with `lo`, then min with `hi`.
        DVec2::new(
            min_f64(max_f64(v.x, lo.x), hi.x),
            min_f64(max_f64(v.y, lo.y), hi.y),
        )
    });
    let mut found = None;
    for q in core::iter::once(centre)
        .chain(corners)
        .chain(grid)
        .chain(from_area)
    {
        if !s.contains_plan(q) {
            continue;
        }
        let zs = s.z_at(q);
        let centre3 = resting_centre(s, q, r);
        let cp = c.closest(centre3);
        if cp.z <= zs + k.min_gap {
            continue;
        }
        let gap = r + cp.distance(centre3);
        // `!(gap < limit)`: NaN, at or above `need`, or not below the
        // entry already kept.
        if gap.is_nan() || gap >= limit {
            continue;
        }
        if !k.ball.allowed.at(s.layer, q)
            || in_transition(w, q, zs, k.ball.d, s.layer, c.layer)
            || !laterally_free(w, s.layer, centre3, r, idx)
        {
            continue;
        }
        limit = gap;
        found = Some(gap);
    }
    found
}

/// The smallest headroom under each ceiling mesh for `ball`: for each
/// ceiling mesh, the first sample (supports in `w.tris` order, then the
/// index's candidates ascending, then the samples in order) that reaches
/// the minimum headroom below `ball.needed()` among the samples where the
/// ball may rest.
///
/// With `prune` (#231), a (support, ceiling) pair whose [`headroom_floor`]
/// shows that no sample of it can reach that minimum is skipped and not
/// counted in `IndexStats::pairs_examined`:
///
/// * the floor is at or above the entry already kept for the ceiling's
///   mesh: every sample of the pair would fail the `gap < kept` test (a
///   no-op);
/// * the floor is strictly above `seed`, a headroom some valid sample of
///   the mesh really has (so the minimum is at most `seed`): no sample of
///   the pair reaches the minimum, and the samples that do are kept by
///   the same `gap < kept` sequence with or without the pair.
///
/// The seed of a ceiling mesh is the lowest valid headroom of its first
/// pair with the lowest floor below `need` (a bound-only pass over the
/// pairs first, counting nothing; then that pair is sampled, and counted).
/// Without `prune`, this is the original loop. Also gives the part of
/// the counter the seed step added (zero without `prune`).
fn worst_under(
    w: &World<'_>,
    ball: &Ball,
    prune: bool,
    cands: &mut Vec<u32>,
    idx: &mut Vec<u32>,
) -> (BTreeMap<usize, Worst>, u64) {
    let r = ball.r();
    let need = ball.needed();
    let reach = need - r; // distance the ceiling must keep from the centre
    let spacing = min_f64(0.5 * r, MAX_SPACING_M);
    let k = Scan {
        ball,
        r,
        spacing,
        min_gap: f64::from(MIN_HEADROOM_GAP_MM) * 1.0e-3,
        extra: area_samples(ball, spacing),
    };
    let pad = DVec2::splat(reach);
    // Each walkable support the ball may use, its resting-centre height
    // bounds, the ceiling height bound `top` and its ceiling candidates.
    let supports = |cands: &mut Vec<u32>, f: &mut PerSupport<'_>| {
        for (si, s) in w.tris.iter().enumerate() {
            if s.nz() < WALKABLE_NZ || !ball.allowed.layer(s.layer) {
                continue;
            }
            // Highest resting centre on `s` ([`resting_centre`]) plus the
            // distance the ceiling must keep from it: a ceiling wholly
            // above this cannot be too low anywhere on `s`.
            let top = s.max.z + r / s.nz() + reach;
            // The ceilings the loop below does not skip at once: other
            // layers, not steep, overlapping `s.min.z + min_gap ..= top`.
            let ceilings = Filter {
                layer: LayerSel::Except(s.layer),
                slopes: Slope::CEILINGS,
                z_lo: s.min.z + k.min_gap,
                z_hi: top,
            };
            w.index.query(
                w.tris,
                s.min.truncate() - pad,
                s.max.truncate() + pad,
                &ceilings,
                cands,
            );
            let (lo, hi) = s.z_at_bounds();
            f(si, (lo + r / s.nz(), hi + r / s.nz()), top, cands);
        }
    };
    // Seeds: per ceiling mesh, the first pair with the lowest floor below
    // `need`, then its lowest valid headroom.
    let mut seeds: BTreeMap<usize, f64> = BTreeMap::new();
    let before = w.pairs.get();
    if prune {
        let mut first: BTreeMap<usize, (f64, usize, u32)> = BTreeMap::new();
        supports(cands, &mut |si, centre_z, top, cands| {
            let s = &w.tris[si];
            for &ci in cands {
                let c = &w.tris[ci as usize];
                if not_a_ceiling(s, c, k.min_gap, top) {
                    continue;
                }
                let Some((lo, hi)) = reach_box(s, c, pad) else {
                    continue;
                };
                let best = first.get(&c.mesh).map_or(need, |e| min_f64(e.0, need));
                // The vertical part alone first (never above the full floor).
                if headroom_floor(r, centre_z, c, None) >= best {
                    continue;
                }
                let floor = headroom_floor(r, centre_z, c, Some((lo, hi)));
                if floor < best {
                    first.insert(c.mesh, (floor, si, ci));
                }
            }
        });
        for (mesh, (_, si, ci)) in first {
            let (s, c) = (&w.tris[si], &w.tris[ci as usize]);
            w.count(1);
            let seed = reach_box(s, c, pad).and_then(|b| scan_pair(w, &k, (s, c), b, need, idx));
            if let Some(seed) = seed {
                seeds.insert(mesh, seed);
            }
        }
    }
    let seed_cost = w.pairs.get() - before;
    let mut worst: BTreeMap<usize, Worst> = BTreeMap::new();
    supports(cands, &mut |si, centre_z, top, cands| {
        let s = &w.tris[si];
        let mut pruned = 0usize;
        for &ci in cands {
            let c = &w.tris[ci as usize];
            if not_a_ceiling(s, c, k.min_gap, top) {
                continue;
            }
            let Some((lo, hi)) = reach_box(s, c, pad) else {
                continue;
            };
            let kept = worst.get(&c.mesh).map(|e| e.gap);
            if prune {
                let seed = seeds.get(&c.mesh).copied();
                let skip = |floor: f64| {
                    kept.is_some_and(|g| floor >= g) || seed.is_some_and(|g| floor > g)
                };
                // The vertical part alone first (never above the full floor).
                if (kept.is_some() || seed.is_some())
                    && (skip(headroom_floor(r, centre_z, c, None))
                        || skip(headroom_floor(r, centre_z, c, Some((lo, hi)))))
                {
                    pruned += 1;
                    continue;
                }
            }
            let limit = kept.map_or(need, |g| min_f64(g, need));
            if let Some(gap) = scan_pair(w, &k, (s, c), (lo, hi), limit, idx) {
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
        // Every candidate the query returned except the pruned pairs.
        w.count(cands.len() - pruned);
    });
    (worst, seed_cost)
}

/// Headroom under non-steep colliders of other layers above every walkable
/// surface a ball may use: the ball resting on the support (centre
/// [`resting_centre`]) must stay `r + MIN_CLEARANCE_MM` from every such
/// ceiling triangle (sphere test, so deck edges, slopes and low vertices
/// count, not only the vertical gap at the centre). The reported headroom
/// is `r + distance` (the vertical gap under a flat deck).
///
/// Pairs that provably cannot change the reported minimum (or the support
/// it names) are pruned ([`worst_under`], #231).
pub(super) fn decks(w: &World<'_>, balls: &[Ball], sink: &mut Sink) {
    let mut cands = Vec::new();
    let mut idx = Vec::new();
    for ball in balls {
        let need = ball.needed();
        let (worst, _) = worst_under(w, ball, true, &mut cands, &mut idx);
        for (mesh, e) in worst {
            let (file, local, node) = w.mesh_ref(mesh);
            let (_, _, under) = w.mesh_ref(e.support_mesh);
            sink.error(Finding {
                feature: Feature::DeckHeadroom,
                name: node,
                ball: &ball.name,
                file,
                path: format!("meshes[{local}]"),
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

#[cfg(test)]
mod tests {
    use super::super::{Allowed, SpatialIndex};
    use super::*;
    use crate::clearance::geom::{Polygon, TriIndex, collider_triangles};
    use pin_assets::{GltfScene, LoadedTable, MeshData};
    use proptest::prelude::*;
    use std::path::Path;
    use std::sync::OnceLock;

    /// The apex fixture table (loaded once from a copy with placeholder
    /// meshes) plus a layer 0 / 1 transition zone over part of the random
    /// scenes, so that `in_transition` takes part in the check.
    fn table() -> LoadedTable {
        static TABLE: OnceLock<LoadedTable> = OnceLock::new();
        TABLE
            .get_or_init(|| {
                let src =
                    Path::new(env!("CARGO_MANIFEST_DIR")).join("../pin_assets/tests/fixtures/apex");
                let dir = std::env::temp_dir()
                    .join(format!("pin_tablegen_headroom_unit_{}", std::process::id()));
                let _ = std::fs::remove_dir_all(&dir);
                copy_dir(&src, &dir);
                for f in ["playfield.glb", "backbox.glb"] {
                    std::fs::write(dir.join(f), b"placeholder").unwrap();
                }
                let vfs = pin_assets::Vfs::open(&dir).unwrap();
                let mut table = pin_assets::load_table(&vfs).unwrap_or_else(|e| panic!("{e}"));
                drop(vfs);
                let _ = std::fs::remove_dir_all(&dir);
                table.transitions.push(pin_assets::TransitionSi {
                    name: "unit".to_owned(),
                    from_layer: 0,
                    to_layer: 1,
                    zone_min_m: [0.0, 0.0, -0.1],
                    zone_max_m: [0.04, 0.05, 0.1],
                });
                table
            })
            .clone()
    }

    fn copy_dir(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// A collider sheet of `nx × ny` quads (two triangles each) over
    /// `x0..x0 + w`, `y0..y0 + h` at height `z + sx·x + sy·y`, on `layer`.
    fn sheet(layer: u8, [x0, y0, w, h, z]: [f32; 5], [sx, sy]: [f32; 2], n: [u32; 2]) -> MeshData {
        let [nx, ny] = n;
        let mut positions = Vec::new();
        for j in 0..=ny {
            for i in 0..=nx {
                let (u, v) = (i as f32 / nx as f32, j as f32 / ny as f32);
                let (x, y) = (x0 + u * w, y0 + v * h);
                positions.push([x, y, z + sx * x + sy * y]);
            }
        }
        let mut indices = Vec::new();
        let at = |i: u32, j: u32| j * (nx + 1) + i;
        for j in 0..ny {
            for i in 0..nx {
                let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
                indices.extend_from_slice(&[a, b, c, a, c, d]);
            }
        }
        MeshData {
            name: format!("sheet{layer}_{z}"),
            positions,
            indices,
            transform: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
            extras: pin_assets::NodeExtras {
                collision_layer: Some(layer),
                collider: true,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// A vertical wall on `layer` along `x = x0`, `y0..y1`, `0..0.2` m.
    fn wall(layer: u8, x0: f32, y0: f32, y1: f32) -> MeshData {
        let mut m = sheet(layer, [x0, y0, 0.0, y1 - y0, 0.0], [0.0, 0.0], [1, 1]);
        m.positions = vec![[x0, y0, 0.0], [x0, y1, 0.0], [x0, y1, 0.2], [x0, y0, 0.2]];
        m.indices = vec![0, 1, 2, 0, 2, 3];
        m
    }

    /// One `worst` entry as comparable bits.
    type Entry = (usize, u64, usize, u8, u8);

    /// `worst_under` with and without the prune, plus both counters.
    fn run(
        table: &LoadedTable,
        scene: &GltfScene,
        ball: &Ball,
        index: SpatialIndex,
        prune: bool,
    ) -> (Vec<Entry>, u64, u64) {
        let tris = collider_triangles(scene);
        let index = TriIndex::new(&tris, index);
        let w = World {
            table,
            scene,
            tris: &tris,
            index: &index,
            pairs: core::cell::Cell::new(0),
            glb: "playfield.glb",
            backbox: None,
        };
        let (worst, seed_cost) = worst_under(&w, ball, prune, &mut Vec::new(), &mut Vec::new());
        let entries = worst
            .into_iter()
            .map(|(m, e)| {
                (
                    m,
                    e.gap.to_bits(),
                    e.support_mesh,
                    e.support_layer,
                    e.ceiling_layer,
                )
            })
            .collect();
        (entries, w.pairs.get(), seed_cost)
    }

    fn ball(d: f64, allowed: Allowed) -> Ball {
        Ball {
            name: format!("{d}"),
            d,
            allowed,
        }
    }

    /// A random small scene: overlapping floors and decks on three layers,
    /// heights from a short list so that exact ties are common (several
    /// supports and ceilings at the same heights), some tilted.
    fn scenes() -> impl Strategy<Value = GltfScene> {
        let mesh = (
            0u8..3,
            0usize..4,
            0usize..4,
            prop_oneof![
                3 => Just([0.0f32, 0.0]),
                1 => (-0.05f32..0.05, -0.05f32..0.05).prop_map(|(a, b)| [a, b]),
                1 => Just([1.0e-6f32, 0.0]),
            ],
            1u32..5,
            1u32..5,
            0usize..3,
        );
        (proptest::collection::vec(mesh, 1..6), any::<bool>()).prop_map(|(ms, with_wall)| {
            const Z: [f32; 4] = [0.0, 0.06, 0.065, 0.03];
            const AT: [[f32; 2]; 4] = [[0.0, 0.0], [0.05, 0.02], [0.0, 0.1], [0.11, 0.07]];
            const SIZE: [[f32; 2]; 3] = [[0.12, 0.12], [0.2, 0.05], [0.07, 0.15]];
            let mut meshes: Vec<MeshData> = ms
                .into_iter()
                .map(|(layer, zi, ai, tilt, nx, ny, si)| {
                    let [x0, y0] = AT[ai];
                    let [w, h] = SIZE[si];
                    sheet(layer, [x0, y0, w, h, Z[zi]], tilt, [nx, ny])
                })
                .collect();
            if with_wall {
                meshes.push(wall(0, 0.06, 0.0, 0.2));
            }
            GltfScene {
                meshes,
                ..Default::default()
            }
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(192))]

        /// SPEC 2.8, 6.0 (#231): the pruned headroom check keeps exactly
        /// the entries of the unpruned loop (ceiling mesh, gap bits, named
        /// support) on random scenes full of exact ties, for unrestricted
        /// and area-restricted balls and either index; it never examines
        /// more pairs, and the fine index never more than the legacy one.
        #[test]
        fn pruned_headroom_equals_unpruned(
            scene in scenes(),
            d in prop_oneof![Just(0.027f64), Just(0.0794), Just(0.05)],
            restricted in any::<bool>(),
        ) {
            let table = table();
            let allowed = if restricted {
                let pts = [(0.02, 0.01), (0.15, 0.03), (0.12, 0.18), (0.03, 0.15)];
                Allowed::Only(vec![(
                    vec![0, 1],
                    Some(Polygon {
                        points: pts.iter().map(|&(x, y)| DVec2::new(x, y)).collect(),
                    }),
                )])
            } else {
                Allowed::Anywhere
            };
            let b = ball(d, allowed);
            let (reference, ref_pairs, _) = run(&table, &scene, &b, SpatialIndex::Legacy, false);
            for index in [SpatialIndex::Legacy, SpatialIndex::Fine] {
                let (entries, pairs, seed_cost) = run(&table, &scene, &b, index, true);
                prop_assert_eq!(&entries, &reference, "{:?}", index);
                // Legacy reference: the fine index examines no more.
                prop_assert!(pairs <= ref_pairs + seed_cost, "{} {} {}", pairs, ref_pairs, seed_cost);
                let (unpruned, _, none) = run(&table, &scene, &b, index, false);
                prop_assert_eq!(none, 0);
                prop_assert_eq!(&unpruned, &reference);
            }
            let fine = run(&table, &scene, &b, SpatialIndex::Fine, true).1;
            let legacy = run(&table, &scene, &b, SpatialIndex::Legacy, true).1;
            prop_assert!(fine <= legacy, "{} {}", fine, legacy);
        }
    }

    /// SPEC 2.8, 6.0 (#231): exact ties. Three identical level floors
    /// (meshes 0..3) under two identical level decks: every sample has the
    /// same headroom, and the first support mesh in triangle order is the
    /// one named, with and without the prune, for both decks.
    #[test]
    fn ties_name_the_first_support() {
        let table = table();
        let floor = |n| sheet(0, [0.0, 0.0, 0.3, 0.3, 0.0], [0.0, 0.0], [n, n]);
        let deck = |n| sheet(1, [0.05, 0.05, 0.2, 0.2, 0.06], [0.0, 0.0], [n, n]);
        let scene = GltfScene {
            meshes: vec![floor(7), floor(5), floor(9), deck(6), deck(4)],
            ..Default::default()
        };
        let b = ball(0.0794, Allowed::Anywhere);
        let (reference, ref_pairs, _) = run(&table, &scene, &b, SpatialIndex::Fine, false);
        let (pruned, pairs, _) = run(&table, &scene, &b, SpatialIndex::Fine, true);
        assert_eq!(pruned, reference);
        assert_eq!(reference.len(), 2, "{reference:?}");
        for e in &reference {
            assert_eq!(e.2, 0, "{reference:?}");
            assert!((f64::from_bits(e.1) - 0.06).abs() < 1.0e-6);
        }
        assert!(pairs * 4 < ref_pairs, "{pairs} vs {ref_pairs}");
    }

    /// The `5 k²`-triangle synthetic scene of #135: a `4 k²`-triangle level
    /// floor (layer 0, 514 × 1070 mm) under a `k²`-triangle level deck
    /// (layer 1, at `deck_z` m) over its upper-right quarter.
    fn dense(k: u32, deck_z: f32) -> GltfScene {
        GltfScene {
            meshes: vec![
                sheet(0, [0.0, 0.0, 0.514, 1.07, 0.0], [0.0, 0.0], [k, 2 * k]),
                sheet(1, [0.26, 0.7, 0.254, 0.37, deck_z], [0.0, 0.0], [k / 2, k]),
            ],
            ..Default::default()
        }
    }

    /// SPEC 2.8, 6.0 (#231): on the too-low deck (60 and 65 mm decks, a
    /// 79.4 mm ball), nine times the triangles (8 k to 75 k) costs the
    /// pruned check at most nine times the examined pairs (linear), where
    /// the unpruned loop grows with the product of the densities; the
    /// result is the unpruned one, and counts are deterministic.
    #[test]
    fn too_low_deck_pairs_grow_linearly() {
        let table = table();
        let b = ball(0.0794, Allowed::Anywhere);
        for deck in [0.06f32, 0.065] {
            let small = dense(41, deck);
            let (reference, unpruned, _) = run(&table, &small, &b, SpatialIndex::Fine, false);
            let (entries, pruned, _) = run(&table, &small, &b, SpatialIndex::Fine, true);
            assert_eq!(entries, reference);
            assert_eq!(entries.len(), 1);
            assert!(
                pruned > 0 && pruned * 20 < unpruned,
                "{pruned} vs {unpruned}"
            );
            let big = dense(123, deck);
            let (_, big_pruned, _) = run(&table, &big, &b, SpatialIndex::Fine, true);
            assert!(big_pruned <= 9 * pruned, "{pruned} -> {big_pruned}");
            assert_eq!(
                run(&table, &big, &b, SpatialIndex::Fine, true).1,
                big_pruned
            );
        }
    }
}
