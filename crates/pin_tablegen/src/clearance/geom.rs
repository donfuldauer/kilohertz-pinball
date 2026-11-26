//! Geometry helpers of the clearance validator (setup-time, `f64`).

use glam::{DVec2, DVec3, Vec3};
use pin_assets::{GltfScene, MeshData};
use pin_core::fmath::{max_f64, min_f64};
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
            min: DVec3::new(
                min_f64(min_f64(p[0].x, p[1].x), p[2].x),
                min_f64(min_f64(p[0].y, p[1].y), p[2].y),
                min_f64(min_f64(p[0].z, p[1].z), p[2].z),
            ),
            max: DVec3::new(
                max_f64(max_f64(p[0].x, p[1].x), p[2].x),
                max_f64(max_f64(p[0].y, p[1].y), p[2].y),
                max_f64(max_f64(p[0].z, p[1].z), p[2].z),
            ),
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
        let scale = max_f64((b - a).length_squared(), (c - a).length_squared());
        let eps = -1.0e-9 * scale;
        (d1 >= eps && d2 >= eps && d3 >= eps) || (d1 <= -eps && d2 <= -eps && d3 <= -eps)
    }

    /// Closest point to plan point `q` of the triangle's plan projection
    /// (`q` itself when the projection contains it).
    pub fn plan_closest(&self, q: DVec2) -> DVec2 {
        if self.contains_plan(q) {
            return q;
        }
        let [a, b, c] = self.p.map(|v| v.truncate());
        [(a, b), (b, c), (c, a)]
            .into_iter()
            .map(|(u, v)| segment_closest(q, u, v))
            .min_by(|x, y| x.distance(q).total_cmp(&y.distance(q)))
            .unwrap_or(q)
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
    append_collider_triangles(
        scene,
        |m| {
            m.extras
                .collider
                .then(|| m.extras.collision_layer.unwrap_or(0))
        },
        0,
        &mut out,
    );
    out
}

/// Appends the triangles of every mesh of `scene` to which `layer_of`
/// gives a layer (`None` skips the mesh), built as [`collider_triangles`]
/// builds them, with `mesh_offset` added to every [`Tri::mesh`] (so the
/// triangles of a second scene keep distinct mesh indices).
pub(super) fn append_collider_triangles(
    scene: &GltfScene,
    layer_of: impl Fn(&MeshData) -> Option<u8>,
    mesh_offset: usize,
    out: &mut Vec<Tri>,
) {
    for (mi, mesh) in scene.meshes.iter().enumerate() {
        let Some(layer) = layer_of(mesh) else {
            continue;
        };
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
        for t in mesh.indices.chunks_exact(3) {
            let get = |i: u32| mesh.positions.get(i as usize).copied();
            let (Some(a), Some(b), Some(c)) = (get(t[0]), get(t[1]), get(t[2])) else {
                continue;
            };
            let p = [tf(a), tf(b), tf(c)];
            if p.iter().all(|v| v.is_finite())
                && let Some(tri) = Tri::new(p, layer, mesh_offset + mi)
            {
                out.push(tri);
            }
        }
    }
}

/// Uniform plan-view grid over triangle bounding boxes (cells of at
/// least 50 mm): the reference index ([`super::SpatialIndex::Legacy`]).
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
            let (t_lo, t_hi) = (t.min.truncate(), t.max.truncate());
            lo = DVec2::new(min_f64(lo.x, t_lo.x), min_f64(lo.y, t_lo.y));
            hi = DVec2::new(max_f64(hi.x, t_hi.x), max_f64(hi.y, t_hi.y));
        }
        if tris.is_empty() {
            lo = DVec2::ZERO;
            hi = DVec2::ZERO;
        }
        let span = hi - lo;
        let extent = max_f64(span.x, span.y);
        let cell = max_f64(extent / MAX_GRID_SIDE, 0.05);
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
        out.retain(|&i| plan_overlaps(&tris[i as usize], lo, hi));
    }
}

/// Which triangles a filtered index query asks for. Every field only
/// narrows the plan-box query; the callers' narrow phases apply the same
/// conditions again (exactly), so a filter is a verdict-preserving early
/// reject and [`TriGrid`] (the reference) may ignore it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Filter {
    /// Layer selection.
    pub layer: LayerSel,
    /// Accepted [`Slope`] classes ([`Slope::bit`] mask).
    pub slopes: u8,
    /// Lowest `Tri::max.z` accepted (inclusive).
    pub z_lo: f64,
    /// Highest `Tri::min.z` accepted (inclusive).
    pub z_hi: f64,
}

/// Layer part of a [`Filter`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LayerSel {
    /// Every layer.
    Any,
    /// Only this layer.
    Only(u8),
    /// Every layer but this one.
    Except(u8),
}

impl LayerSel {
    fn accepts(self, layer: u8) -> bool {
        match self {
            LayerSel::Any => true,
            LayerSel::Only(l) => layer == l,
            LayerSel::Except(l) => layer != l,
        }
    }
}

/// Slope class of a triangle by `|n·z|` ([`Tri::nz`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Slope {
    /// Steeper than 60° (`|n·z| <` [`CEILING_NZ`]): a wall.
    Wall,
    /// A ceiling that is not walkable (`CEILING_NZ <= |n·z| <`
    /// [`WALKABLE_NZ`]).
    Ceiling,
    /// Walkable (and a ceiling): `|n·z| >=` [`WALKABLE_NZ`].
    Walkable,
}

impl Slope {
    /// Every class.
    pub const ALL: u8 = 0b111;
    /// Ceilings (walkable ones included): `|n·z| >=` [`CEILING_NZ`].
    pub const CEILINGS: u8 = 0b110;

    /// The class of `t`.
    pub fn of(t: &Tri) -> Slope {
        let nz = t.nz();
        if nz >= WALKABLE_NZ {
            Slope::Walkable
        } else if nz >= CEILING_NZ {
            Slope::Ceiling
        } else {
            Slope::Wall
        }
    }

    /// This class's bit in [`Filter::slopes`].
    pub const fn bit(self) -> u8 {
        match self {
            Slope::Wall => 0b001,
            Slope::Ceiling => 0b010,
            Slope::Walkable => 0b100,
        }
    }
}

impl Filter {
    /// No narrowing: the plain plan-box query.
    pub const ANY: Filter = Filter {
        layer: LayerSel::Any,
        slopes: Slope::ALL,
        z_lo: f64::NEG_INFINITY,
        z_hi: f64::INFINITY,
    };

    /// Whether `t` passes (layer, slope and `z_lo..=z_hi` overlap).
    pub fn accepts(&self, t: &Tri) -> bool {
        self.layer.accepts(t.layer)
            && self.slopes & Slope::of(t).bit() != 0
            && t.max.z >= self.z_lo
            && t.min.z <= self.z_hi
    }
}

/// Whether the plan bounding box of `t` overlaps the box `lo..hi` (the
/// exact test of [`TriGrid::query`]).
fn plan_overlaps(t: &Tri, lo: DVec2, hi: DVec2) -> bool {
    t.min.x <= hi.x && t.max.x >= lo.x && t.min.y <= hi.y && t.max.y >= lo.y
}

/// Most triangles in a [`TriBvh`] leaf.
const BVH_LEAF: usize = 4;

/// One node of a [`Bvh`]: the bounding box of its triangles; a leaf holds
/// `items[a..b]`, an inner node has children `a` and `b`.
#[derive(Clone, Debug)]
struct BvhNode {
    min: DVec3,
    max: DVec3,
    a: u32,
    b: u32,
    leaf: bool,
}

/// A bounding-volume hierarchy over the 3D bounding boxes of some
/// triangles: median splits along the longest axis of the centroid
/// bounds, at most [`BVH_LEAF`] triangles per leaf (so `O(n log n)` to
/// build, fewer than `n` nodes, depth at most `log2 n + 1`).
#[derive(Clone, Debug)]
struct Bvh {
    nodes: Vec<BvhNode>,
    items: Vec<u32>,
}

impl Bvh {
    fn new(tris: &[Tri], mut items: Vec<u32>) -> Bvh {
        let mut nodes = Vec::with_capacity(items.len());
        if !items.is_empty() {
            Self::build(tris, &mut items, 0, &mut nodes);
        }
        Bvh { nodes, items }
    }

    /// Builds the subtree over `items` (at `offset` in the item list);
    /// returns its node index.
    fn build(tris: &[Tri], items: &mut [u32], offset: usize, nodes: &mut Vec<BvhNode>) -> u32 {
        let mut min = DVec3::splat(f64::INFINITY);
        let mut max = DVec3::splat(f64::NEG_INFINITY);
        let mut c_min = DVec3::splat(f64::INFINITY);
        let mut c_max = DVec3::splat(f64::NEG_INFINITY);
        for &i in items.iter() {
            let t = &tris[i as usize];
            min = vmin(min, t.min);
            max = vmax(max, t.max);
            let c = t.min + t.max;
            c_min = vmin(c_min, c);
            c_max = vmax(c_max, c);
        }
        let me = nodes.len() as u32;
        nodes.push(BvhNode {
            min,
            max,
            a: offset as u32,
            b: (offset + items.len()) as u32,
            leaf: true,
        });
        if items.len() <= BVH_LEAF {
            return me;
        }
        let ext = c_max - c_min;
        let axis = if ext.x >= ext.y && ext.x >= ext.z {
            0
        } else if ext.y >= ext.z {
            1
        } else {
            2
        };
        let key = |i: u32| {
            let t = &tris[i as usize];
            t.min[axis] + t.max[axis]
        };
        let mid = items.len() / 2;
        items.select_nth_unstable_by(mid, |&p, &q| key(p).total_cmp(&key(q)).then(p.cmp(&q)));
        let (left, right) = items.split_at_mut(mid);
        let a = Self::build(tris, left, offset, nodes);
        let b = Self::build(tris, right, offset + mid, nodes);
        let node = &mut nodes[me as usize];
        node.a = a;
        node.b = b;
        node.leaf = false;
        me
    }

    /// Appends (unordered) the triangles whose plan boxes overlap
    /// `lo..hi` and which `f` accepts.
    fn query(&self, tris: &[Tri], lo: DVec2, hi: DVec2, f: &Filter, out: &mut Vec<u32>) {
        if !self.nodes.is_empty() {
            self.visit(0, tris, lo, hi, f, out);
        }
    }

    fn visit(&self, n: u32, tris: &[Tri], lo: DVec2, hi: DVec2, f: &Filter, out: &mut Vec<u32>) {
        let node = &self.nodes[n as usize];
        let hit = node.min.x <= hi.x
            && node.max.x >= lo.x
            && node.min.y <= hi.y
            && node.max.y >= lo.y
            && node.max.z >= f.z_lo
            && node.min.z <= f.z_hi;
        if !hit {
            return;
        }
        if node.leaf {
            for &i in &self.items[node.a as usize..node.b as usize] {
                let t = &tris[i as usize];
                if plan_overlaps(t, lo, hi) && f.accepts(t) {
                    out.push(i);
                }
            }
        } else {
            self.visit(node.a, tris, lo, hi, f, out);
            self.visit(node.b, tris, lo, hi, f, out);
        }
    }
}

fn vmin(a: DVec3, b: DVec3) -> DVec3 {
    DVec3::new(min_f64(a.x, b.x), min_f64(a.y, b.y), min_f64(a.z, b.z))
}

fn vmax(a: DVec3, b: DVec3) -> DVec3 {
    DVec3::new(max_f64(a.x, b.x), max_f64(a.y, b.y), max_f64(a.z, b.z))
}

/// The finer spatial index ([`super::SpatialIndex::Fine`]): one [`Bvh`]
/// per (layer, [`Slope`]) bucket, so a filtered query only walks the
/// buckets its [`Filter`] selects and prunes by height in the tree.
#[derive(Clone, Debug)]
pub(super) struct TriBvh {
    /// Non-empty buckets in (layer, slope) order.
    buckets: Vec<(u8, Slope, Bvh)>,
}

impl TriBvh {
    pub fn new(tris: &[Tri]) -> TriBvh {
        let mut order: Vec<(u8, Slope, u32)> = tris
            .iter()
            .enumerate()
            .map(|(i, t)| (t.layer, Slope::of(t), i as u32))
            .collect();
        order.sort_unstable();
        let mut buckets = Vec::new();
        for group in order.chunk_by(|p, q| p.0 == q.0 && p.1 == q.1) {
            let items = group.iter().map(|g| g.2).collect();
            buckets.push((group[0].0, group[0].1, Bvh::new(tris, items)));
        }
        TriBvh { buckets }
    }

    /// Indices of the triangles whose plan bounding boxes overlap the box
    /// `lo..hi` (`lo <= hi`) and which `f` accepts, ascending and without
    /// duplicates (into `out`).
    pub fn query(&self, tris: &[Tri], lo: DVec2, hi: DVec2, f: &Filter, out: &mut Vec<u32>) {
        out.clear();
        for (layer, slope, bvh) in &self.buckets {
            if f.layer.accepts(*layer) && f.slopes & slope.bit() != 0 {
                bvh.query(tris, lo, hi, f, out);
            }
        }
        // Each triangle is in exactly one leaf of one bucket: no
        // duplicates to remove.
        out.sort_unstable();
    }
}

/// The spatial index a validation run uses ([`super::SpatialIndex`]).
pub(super) enum TriIndex {
    /// The reference uniform grid: ignores [`Filter`]s.
    Legacy(TriGrid),
    /// The finer index: applies [`Filter`]s.
    Fine(TriBvh),
}

impl TriIndex {
    pub fn new(tris: &[Tri], kind: super::SpatialIndex) -> TriIndex {
        match kind {
            super::SpatialIndex::Legacy => TriIndex::Legacy(TriGrid::new(tris)),
            super::SpatialIndex::Fine => TriIndex::Fine(TriBvh::new(tris)),
        }
    }

    /// Indices, ascending and without duplicates, of the triangles whose
    /// plan bounding boxes overlap `lo..hi`; the fine index also drops
    /// those `f` rejects (the legacy grid returns them all).
    pub fn query(&self, tris: &[Tri], lo: DVec2, hi: DVec2, f: &Filter, out: &mut Vec<u32>) {
        match self {
            TriIndex::Legacy(g) => g.query(tris, lo, hi, out),
            TriIndex::Fine(b) => b.query(tris, lo, hi, f, out),
        }
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

    /// Whether `q` is inside or on the boundary (within 1 nm): the area's
    /// own vertices and edge points count as inside.
    pub fn covers(&self, q: DVec2) -> bool {
        self.contains(q) || self.boundary_distance(q) <= 1.0e-9
    }

    /// Distance (m) from `q` to the polygon's boundary.
    pub fn boundary_distance(&self, q: DVec2) -> f64 {
        let n = self.points.len();
        (0..n)
            .map(|i| segment_distance(q, self.points[i], self.points[(i + 1) % n]))
            .fold(f64::INFINITY, min_f64)
    }

    /// Whether the even-odd filled region has positive area (a
    /// self-intersecting outline with filled lobes counts even when its
    /// signed area cancels; one traced twice over does not). Sweeps the
    /// horizontal slabs between vertex heights: a slab is empty exactly when
    /// the edges spanning it pair up into coincident lines (within 1 nm).
    pub fn has_filled_area(&self) -> bool {
        const TOL: f64 = 1.0e-9;
        let pts = &self.points;
        let n = pts.len();
        if n < 3 {
            return false;
        }
        let mut ys: Vec<f64> = pts.iter().map(|p| p.y).collect();
        ys.sort_by(f64::total_cmp);
        ys.dedup();
        let mut edges: Vec<(DVec2, DVec2)> = (0..n)
            .filter_map(|i| {
                let (a, b) = (pts[i], pts[(i + 1) % n]);
                match a.y.total_cmp(&b.y) {
                    core::cmp::Ordering::Equal => None,
                    core::cmp::Ordering::Less => Some((a, b)),
                    core::cmp::Ordering::Greater => Some((b, a)),
                }
            })
            .collect();
        edges.sort_by(|e, f| e.0.y.total_cmp(&f.0.y));
        let mut next = 0;
        let mut active: Vec<(DVec2, DVec2)> = Vec::new();
        let mut xs: Vec<(f64, f64)> = Vec::new();
        for w in ys.windows(2) {
            let (y0, y1) = (w[0], w[1]);
            while next < edges.len() && edges[next].0.y <= y0 {
                active.push(edges[next]);
                next += 1;
            }
            // Every vertex height is a slab boundary, so an edge still
            // above `y0` spans the whole slab.
            active.retain(|e| e.1.y > y0);
            xs.clear();
            for &(a, b) in &active {
                let at = |y: f64| a.x + (y - a.y) * (b.x - a.x) / (b.y - a.y);
                xs.push((at(y0), at(y1)));
            }
            xs.sort_by(|p, q| p.0.total_cmp(&q.0).then(p.1.total_cmp(&q.1)));
            let unpaired = |c: &[(f64, f64)]| {
                c.len() == 1 || (c[1].0 - c[0].0).abs() > TOL || (c[1].1 - c[0].1).abs() > TOL
            };
            if xs.chunks(2).any(unpaired) {
                return true;
            }
        }
        false
    }

    /// Whether segment `a`–`b` meets the polygon's boundary: crosses one of
    /// its edges, or touches it (within 1 nm, as [`Polygon::covers`]) at a
    /// vertex or along a collinear stretch of an edge.
    pub fn meets_segment(&self, a: DVec2, b: DVec2) -> bool {
        const TOL: f64 = 1.0e-9;
        let n = self.points.len();
        (0..n).any(|i| segments_cross(a, b, self.points[i], self.points[(i + 1) % n]))
            // Two segments that touch without crossing properly (collinear
            // overlap included) have an endpoint of one on the other.
            || self.points.iter().any(|&p| segment_distance(p, a, b) <= TOL)
            || self.boundary_distance(a) <= TOL
            || self.boundary_distance(b) <= TOL
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

/// Closest point to `q` of the segment `a`–`b`.
fn segment_closest(q: DVec2, a: DVec2, b: DVec2) -> DVec2 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 0.0 {
        ((q - a).dot(ab) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    a + ab * t
}

/// Distance from `q` to the segment `a`–`b`.
pub(super) fn segment_distance(q: DVec2, a: DVec2, b: DVec2) -> f64 {
    q.distance(segment_closest(q, a, b))
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
    use proptest::prelude::*;

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
            assert!(
                closest_on_triangle(p, a, b, c).distance(want) < 1e-12,
                "{p}"
            );
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

    /// Every triangle whose plan box overlaps `lo..hi` and which `f`
    /// accepts, ascending: the brute-force reference.
    fn brute(tris: &[Tri], lo: DVec2, hi: DVec2, f: &Filter) -> Vec<u32> {
        (0..tris.len() as u32)
            .filter(|&i| {
                let t = &tris[i as usize];
                plan_overlaps(t, lo, hi) && f.accepts(t)
            })
            .collect()
    }

    /// A small random triangle near `(x, y, z)` (sizes from `s`), or `None`
    /// when degenerate.
    fn tri_at(x: f64, y: f64, z: f64, s: [f64; 6], layer: u8, steep: bool) -> Option<Tri> {
        let a = v(x, y, z);
        let b = v(x + s[0], y + s[1], z + s[2] * 0.1);
        let c = if steep {
            v(x + s[3] * 0.1, y + s[4] * 0.1, z + s[5].abs() + 0.01)
        } else {
            v(x + s[3], y + s[4], z + s[5] * 0.1)
        };
        Tri::new([a, b, c], layer, usize::from(layer))
    }

    fn filters() -> impl Strategy<Value = Filter> {
        (0u8..5, 0u8..3, 1u8..8, -0.2f64..0.3, 0.0f64..0.4).prop_map(|(l, sel, slopes, z, dz)| {
            Filter {
                layer: match sel {
                    0 => LayerSel::Any,
                    1 => LayerSel::Only(l),
                    _ => LayerSel::Except(l),
                },
                slopes,
                z_lo: z,
                z_hi: z + dz,
            }
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        /// SPEC 2.8 (#135): the fine index answers every plan-box query
        /// exactly as the legacy grid and a brute-force scan (ascending, no
        /// duplicates), and a filtered query exactly as the filtered scan.
        #[test]
        fn fine_index_matches_legacy_and_brute_force(
            raw in prop::collection::vec(
                (0.0f64..1.0, 0.0f64..2.0, -0.1f64..0.2, prop::array::uniform6(-0.08f64..0.08), 0u8..4, any::<bool>()),
                0..400,
            ),
            boxes in prop::collection::vec((-0.2f64..1.2, -0.2f64..2.2, 0.0f64..0.3, 0.0f64..0.3), 1..24),
            f in filters(),
        ) {
            let tris: Vec<Tri> = raw
                .iter()
                .filter_map(|&(x, y, z, s, l, steep)| tri_at(x, y, z, s, l, steep))
                .collect();
            let grid = TriGrid::new(&tris);
            let fine = TriBvh::new(&tris);
            let (mut a, mut b, mut c) = (Vec::new(), Vec::new(), Vec::new());
            for &(x, y, w, h) in &boxes {
                let (lo, hi) = (DVec2::new(x, y), DVec2::new(x + w, y + h));
                grid.query(&tris, lo, hi, &mut a);
                fine.query(&tris, lo, hi, &Filter::ANY, &mut b);
                prop_assert_eq!(&a, &b);
                prop_assert_eq!(&b, &brute(&tris, lo, hi, &Filter::ANY));
                prop_assert!(b.windows(2).all(|w| w[0] < w[1]));
                fine.query(&tris, lo, hi, &f, &mut c);
                prop_assert_eq!(&c, &brute(&tris, lo, hi, &f));
                // The legacy grid ignores filters (it is the reference).
                TriIndex::Legacy(TriGrid::new(&tris)).query(&tris, lo, hi, &f, &mut b);
                prop_assert_eq!(&a, &b);
            }
        }
    }

    #[test]
    fn index_handles_empty_huge_and_coincident_scenes() {
        let mut out = vec![7];
        let empty = TriBvh::new(&[]);
        empty.query(&[], DVec2::splat(-1.0), DVec2::ONE, &Filter::ANY, &mut out);
        assert!(out.is_empty());
        let none = TriIndex::new(&[], super::super::SpatialIndex::Fine);
        none.query(&[], DVec2::ZERO, DVec2::ONE, &Filter::ANY, &mut out);
        assert!(out.is_empty());

        // Huge plan extent: tiny triangles at the origin and a few far out.
        let mut tris = Vec::new();
        for i in 0..200 {
            let x = f64::from(i) * 1.0e-3;
            tris.push(
                Tri::new(
                    [v(x, 0.0, 0.0), v(x + 1e-3, 0.0, 0.0), v(x, 1e-3, 0.0)],
                    0,
                    0,
                )
                .unwrap(),
            );
        }
        for k in [-1.0e30, 1.0e30] {
            tris.push(
                Tri::new([v(k, k, 0.0), v(k * 0.5, k, 0.0), v(k, k * 0.5, 1.0)], 1, 1).unwrap(),
            );
        }
        tris.push(
            Tri::new(
                [v(-1e30, -1e30, 0.0), v(1e30, -1e30, 0.0), v(0.0, 1e30, 0.0)],
                2,
                2,
            )
            .unwrap(),
        );
        let fine = TriBvh::new(&tris);
        let grid = TriGrid::new(&tris);
        let mut b = Vec::new();
        for (lo, hi) in [
            (DVec2::new(0.05, 0.0), DVec2::new(0.0505, 0.0)),
            (DVec2::splat(-2e30), DVec2::splat(2e30)),
            (DVec2::splat(f64::NEG_INFINITY), DVec2::splat(f64::INFINITY)),
            (DVec2::splat(f64::NAN), DVec2::splat(f64::NAN)),
            (DVec2::splat(5.0e29), DVec2::splat(1.0e30)),
        ] {
            fine.query(&tris, lo, hi, &Filter::ANY, &mut out);
            grid.query(&tris, lo, hi, &mut b);
            assert_eq!(out, b, "{lo} {hi}");
            assert_eq!(out, brute(&tris, lo, hi, &Filter::ANY), "{lo} {hi}");
        }

        // 50 000 coincident triangles: a balanced tree, every one found.
        let t = Tri::new([v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)], 0, 0).unwrap();
        let same = vec![t; 50_000];
        let fine = TriBvh::new(&same);
        let bvh = &fine.buckets[0].2;
        assert!(bvh.nodes.len() < same.len(), "{}", bvh.nodes.len());
        fine.query(
            &same,
            DVec2::splat(0.5),
            DVec2::splat(0.5),
            &Filter::ANY,
            &mut out,
        );
        assert_eq!(out, (0..50_000).collect::<Vec<u32>>());
        fine.query(
            &same,
            DVec2::splat(2.0),
            DVec2::splat(3.0),
            &Filter::ANY,
            &mut out,
        );
        assert!(out.is_empty());
    }

    #[test]
    fn filters_select_layers_slopes_and_heights() {
        let flat = Tri::new([v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 1.0, 0.0)], 1, 0).unwrap();
        let wall = Tri::new([v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, 0.0, 1.0)], 1, 0).unwrap();
        // 55° from level: a ceiling, not walkable.
        let (s, c) = (libm::sin(55f64.to_radians()), libm::cos(55f64.to_radians()));
        let ceil = Tri::new([v(0.0, 0.0, 0.0), v(1.0, 0.0, 0.0), v(0.0, c, s)], 1, 0).unwrap();
        assert_eq!(Slope::of(&flat), Slope::Walkable);
        assert_eq!(Slope::of(&wall), Slope::Wall);
        assert_eq!(Slope::of(&ceil), Slope::Ceiling);
        let f = Filter {
            layer: LayerSel::Except(1),
            ..Filter::ANY
        };
        assert!(!f.accepts(&flat));
        let f = Filter {
            layer: LayerSel::Only(1),
            slopes: Slope::CEILINGS,
            z_lo: 0.5,
            z_hi: 2.0,
        };
        assert!(!f.accepts(&flat) && !f.accepts(&wall) && f.accepts(&ceil));
        assert!(Filter::ANY.accepts(&wall));
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
        assert!(sq.has_filled_area());
        assert!((sq.boundary_distance(DVec2::new(0.5, 0.25)) - 0.25).abs() < 1e-12);
        assert!((sq.boundary_distance(DVec2::new(3.0, 0.5)) - 2.0).abs() < 1e-12);
        assert!(sq.overlaps_box(DVec2::new(0.5, 0.5), DVec2::new(3.0, 3.0)));
        assert!(sq.overlaps_box(DVec2::new(-1.0, -1.0), DVec2::new(3.0, 3.0)));
        assert!(sq.overlaps_box(DVec2::new(-1.0, 0.4), DVec2::new(3.0, 0.6)));
        assert!(!sq.overlaps_box(DVec2::new(2.0, 2.0), DVec2::new(3.0, 3.0)));
    }

    /// Even-odd emptiness, not signed area: a bow tie (signed area 0) has
    /// two filled lobes; a square traced twice (signed area 4) has none.
    #[test]
    fn filled_area_follows_the_even_odd_rule() {
        let poly = |p: &[[f64; 2]]| Polygon {
            points: p.iter().map(|&[x, y]| DVec2::new(x, y)).collect(),
        };
        let bow_tie = poly(&[[0.0, 0.0], [1.0, 1.0], [1.0, 0.0], [0.0, 1.0]]);
        assert!(bow_tie.contains(DVec2::new(0.1, 0.5)));
        assert!(bow_tie.has_filled_area());
        let sq = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        assert!(poly(&sq).has_filled_area());
        let twice: Vec<[f64; 2]> = sq.iter().chain(&sq).copied().collect();
        assert!(!poly(&twice).contains(DVec2::new(0.5, 0.5)));
        assert!(!poly(&twice).has_filled_area());
        for empty in [
            &[[0.0, 0.0], [1.0, 1.0], [2.0, 2.0]][..],
            &[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0]],
            &[[0.0, 0.0], [2.0, 2.0], [1.0, 1.0], [3.0, 3.0]],
            &[[0.0, 0.0], [1.0, 0.0]],
        ] {
            assert!(!poly(empty).has_filled_area(), "{empty:?}");
        }
        // A thin but real sliver still counts.
        assert!(poly(&[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0e-6]]).has_filled_area());
    }
}
