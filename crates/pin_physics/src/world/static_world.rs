//! Static collision world: convex primitives in a `parry3d` BVH, with an
//! 8-bit layer mask and an enable flag per collider.
//!
//! The BVH (broad phase) is `parry3d::partitioning::Bvh` over one leaf per
//! primitive: a cuboid, a cylinder, or a single triangle of a mesh. The narrow
//! phase is the exact convex sphere cast / signed distance of
//! [`super::shapes`], which is allocation-free and deterministic.

use std::collections::BTreeMap;

use arrayvec::ArrayVec;
use glam::{Quat, Vec3};
use parry3d::bounding_volume::Aabb;
use parry3d::partitioning::{Bvh, BvhBuildStrategy};

use super::shapes::{FEATURE_EDGE, FEATURE_VERTEX, Primitive, normals_parallel};

/// Margin added to every BVH leaf so that touching queries are never missed
/// through rounding (metres).
const LEAF_MARGIN: f32 = 1.0e-5;

/// Maximum number of contacts returned by [`StaticWorld::sphere_contacts`].
pub const MAX_SPHERE_CONTACTS: usize = 16;

/// Shape of a static collider (world space, metres).
#[derive(Debug, Clone, PartialEq)]
pub enum StaticShape {
    /// Oriented box.
    Cuboid {
        /// Centre.
        center: Vec3,
        /// Half extents along the local axes.
        half_extents: Vec3,
        /// Local-to-world rotation.
        rotation: Quat,
    },
    /// Solid cylinder (posts, pegs).
    Cylinder {
        /// Centre of the bottom cap.
        base: Vec3,
        /// Vector from the bottom-cap centre to the top-cap centre: its
        /// direction is the cylinder axis and its length the height (a unit
        /// vector gives a 1 m tall cylinder).
        axis: Vec3,
        /// Radius.
        radius: f32,
    },
    /// Two-sided triangle mesh (toys, ramps, playfield cut-outs).
    TriMesh {
        /// Vertex positions.
        vertices: Vec<Vec3>,
        /// Vertex indices of each triangle.
        indices: Vec<[u32; 3]>,
    },
}

/// Identifies a collider of a [`StaticWorld`], in insertion order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ColliderHandle(u32);

impl ColliderHandle {
    /// Insertion index of the collider.
    #[must_use]
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// Result of [`StaticWorld::cast_sphere`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SphereHit {
    /// Time of impact in `[0, max_time]` (seconds).
    pub time: f32,
    /// Contact point on the collider surface.
    pub point: Vec3,
    /// Unit normal pointing from the surface toward the sphere centre.
    pub normal: Vec3,
    /// The collider that was hit.
    pub collider: ColliderHandle,
}

/// One overlap reported by [`StaticWorld::sphere_contacts`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SphereContact {
    /// Closest point on the collider surface.
    pub point: Vec3,
    /// Unit normal pointing from the surface toward the sphere centre.
    pub normal: Vec3,
    /// Penetration depth (`radius` minus signed distance), `>= 0`.
    pub depth: f32,
    /// The overlapping collider.
    pub collider: ColliderHandle,
}

/// Collects colliders for a [`StaticWorld`].
#[derive(Debug, Clone, Default)]
pub struct StaticWorldBuilder {
    shapes: Vec<(StaticShape, u8, bool)>,
}

impl StaticWorldBuilder {
    /// An empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a collider on the layers in the `layers` mask
    /// ([`super::layers`]). Disabled colliders can be enabled later with
    /// [`StaticWorld::set_enabled`].
    ///
    /// Geometry that cannot collide (non-finite coordinates, zero-height
    /// cylinders, zero-area triangles, out-of-range indices) is skipped when
    /// the world is built; the handle stays valid.
    pub fn add_collider(&mut self, shape: StaticShape, layers: u8, enabled: bool) -> ColliderHandle {
        let handle = ColliderHandle(self.shapes.len() as u32);
        self.shapes.push((shape, layers, enabled));
        handle
    }

    /// Builds the BVH.
    #[must_use]
    pub fn build(self) -> StaticWorld {
        let mut prims: Vec<(Primitive, u32)> = Vec::new();
        let mut layers = Vec::with_capacity(self.shapes.len());
        let mut enabled = Vec::with_capacity(self.shapes.len());
        for (index, (shape, layer_mask, on)) in self.shapes.into_iter().enumerate() {
            let collider = index as u32;
            layers.push(layer_mask);
            enabled.push(on);
            match shape {
                StaticShape::Cuboid {
                    center,
                    half_extents,
                    rotation,
                } => {
                    let len2 = rotation.length_squared();
                    let finite = center.is_finite() && half_extents.is_finite();
                    if finite && len2.is_finite() && len2 > 0.0 {
                        prims.push((
                            Primitive::Cuboid {
                                center,
                                half_extents: half_extents.abs(),
                                rotation: rotation.normalize(),
                            },
                            collider,
                        ));
                    }
                }
                StaticShape::Cylinder { base, axis, radius } => {
                    let height = axis.length();
                    if base.is_finite() && height.is_finite() && height > 0.0 && radius.is_finite()
                    {
                        prims.push((
                            Primitive::Cylinder {
                                base,
                                axis: axis / height,
                                height,
                                radius: radius.abs(),
                            },
                            collider,
                        ));
                    }
                }
                StaticShape::TriMesh { vertices, indices } => {
                    mesh_primitives(&vertices, &indices, collider, &mut prims);
                }
            }
        }

        let aabbs: Vec<Aabb> = prims
            .iter()
            .map(|(prim, _)| {
                let (min, max) = prim.aabb();
                Aabb::new(min - Vec3::splat(LEAF_MARGIN), max + Vec3::splat(LEAF_MARGIN))
            })
            .collect();
        let bvh = if aabbs.is_empty() {
            Bvh::new()
        } else {
            Bvh::from_leaves(BvhBuildStrategy::Binned, &aabbs)
        };
        StaticWorld {
            bvh,
            prims,
            layers,
            enabled,
        }
    }
}

/// Bit pattern of a position, with -0.0 folded onto +0.0, for welding.
fn weld_key(v: Vec3) -> [u32; 3] {
    [
        (v.x + 0.0).to_bits(),
        (v.y + 0.0).to_bits(),
        (v.z + 0.0).to_bits(),
    ]
}

/// Converts a mesh into triangle primitives and flags the edges and vertices
/// that are interior to a flat region (both adjacent triangles coplanar), so
/// that a sphere rolling across a tessellated flat never collides with them.
fn mesh_primitives(
    vertices: &[Vec3],
    indices: &[[u32; 3]],
    collider: u32,
    prims: &mut Vec<(Primitive, u32)>,
) {
    struct Tri {
        /// Welded vertex ids.
        ids: [u32; 3],
        pos: [Vec3; 3],
        normal: Vec3,
    }
    // Weld vertices by position so that unindexed meshes still share edges.
    let mut welded: BTreeMap<[u32; 3], u32> = BTreeMap::new();
    let mut tris: Vec<Tri> = Vec::with_capacity(indices.len());
    for tri in indices {
        let mut pos = [Vec3::ZERO; 3];
        let mut ok = true;
        for (slot, &vi) in pos.iter_mut().zip(tri) {
            match vertices.get(vi as usize) {
                Some(v) if v.is_finite() => *slot = *v,
                _ => ok = false,
            }
        }
        if !ok {
            continue;
        }
        let cross = (pos[1] - pos[0]).cross(pos[2] - pos[0]);
        let area2 = cross.length();
        let longest = (pos[1] - pos[0])
            .length_squared()
            .max((pos[2] - pos[1]).length_squared())
            .max((pos[0] - pos[2]).length_squared());
        // Reject zero-area and numerically degenerate (needle) triangles.
        if !(area2 > 0.0 && area2 > 1.0e-6 * longest) {
            continue;
        }
        let mut ids = [0u32; 3];
        for (id, p) in ids.iter_mut().zip(pos) {
            let next = welded.len() as u32;
            *id = *welded.entry(weld_key(p)).or_insert(next);
        }
        tris.push(Tri {
            ids,
            pos,
            normal: cross / area2,
        });
    }

    // Edge -> (count, first triangle, index of its opposite vertex).
    let mut edges: BTreeMap<(u32, u32), (u32, usize, usize)> = BTreeMap::new();
    let edge_key = |a: u32, b: u32| if a < b { (a, b) } else { (b, a) };
    // Local edge e joins corners e and (e + 1) % 3; its opposite corner is (e + 2) % 3.
    for (ti, tri) in tris.iter().enumerate() {
        for e in 0..3 {
            let key = edge_key(tri.ids[e], tri.ids[(e + 1) % 3]);
            edges
                .entry(key)
                .and_modify(|entry| entry.0 += 1)
                .or_insert((1, ti, (e + 2) % 3));
        }
    }
    // An edge is flat (inactive) when exactly two coplanar triangles share it
    // and lie on opposite sides of it.
    let is_flat = |ti: usize, e: usize| -> bool {
        let tri = &tris[ti];
        let key = edge_key(tri.ids[e], tri.ids[(e + 1) % 3]);
        let Some(&(count, first, first_opp)) = edges.get(&key) else {
            return false;
        };
        if count != 2 {
            return false;
        }
        if first == ti {
            // The partner is found from its own side; flatness is symmetric,
            // so look it up by scanning the partner's entry instead.
            return false;
        }
        let other = &tris[first];
        if !normals_parallel(tri.normal, other.normal) {
            return false;
        }
        let e0 = tri.pos[e];
        let edge = tri.pos[(e + 1) % 3] - e0;
        let side = |p: Vec3| edge.cross(p - e0).dot(tri.normal);
        side(tri.pos[(e + 2) % 3]) * side(other.pos[first_opp]) < 0.0
    };
    // `edges` only remembers the first triangle of each edge, so evaluate
    // flatness from the second triangle and share the result with the first.
    let mut flat_edges: BTreeMap<(u32, u32), bool> = BTreeMap::new();
    for (ti, tri) in tris.iter().enumerate() {
        for e in 0..3 {
            if is_flat(ti, e) {
                flat_edges.insert(edge_key(tri.ids[e], tri.ids[(e + 1) % 3]), true);
            }
        }
    }
    // A welded vertex is active when any edge that touches it is active.
    let mut vertex_active = vec![false; welded.len()];
    for key in edges.keys() {
        if !flat_edges.contains_key(key) {
            vertex_active[key.0 as usize] = true;
            vertex_active[key.1 as usize] = true;
        }
    }
    for tri in &tris {
        let mut active = 0u8;
        for e in 0..3 {
            if !flat_edges.contains_key(&edge_key(tri.ids[e], tri.ids[(e + 1) % 3])) {
                active |= FEATURE_EDGE[e];
            }
            if vertex_active[tri.ids[e] as usize] {
                active |= FEATURE_VERTEX[e];
            }
        }
        prims.push((
            Primitive::Triangle {
                a: tri.pos[0],
                b: tri.pos[1],
                c: tri.pos[2],
                normal: tri.normal,
                active,
            },
            collider,
        ));
    }
}

/// The static colliders of a table.
#[derive(Debug, Clone)]
pub struct StaticWorld {
    bvh: Bvh,
    /// Primitive and the index of the collider it belongs to; BVH leaf `i`
    /// is `prims[i]`.
    prims: Vec<(Primitive, u32)>,
    layers: Vec<u8>,
    enabled: Vec<bool>,
}

impl StaticWorld {
    /// Number of colliders (enabled or not).
    #[must_use]
    pub fn collider_count(&self) -> usize {
        self.layers.len()
    }

    /// Enables or disables a collider (drop targets, gates, diverters).
    /// Unknown handles are ignored.
    pub fn set_enabled(&mut self, h: ColliderHandle, enabled: bool) {
        if let Some(flag) = self.enabled.get_mut(h.index()) {
            *flag = enabled;
        }
    }

    /// Whether a collider is enabled; false for an unknown handle.
    #[must_use]
    pub fn is_enabled(&self, h: ColliderHandle) -> bool {
        self.enabled.get(h.index()).copied().unwrap_or(false)
    }

    /// Layer mask of a collider; 0 for an unknown handle.
    #[must_use]
    pub fn collider_layers(&self, h: ColliderHandle) -> u8 {
        self.layers.get(h.index()).copied().unwrap_or(0)
    }

    fn active(&self, collider: u32, layer_mask: u8) -> bool {
        self.enabled[collider as usize] && self.layers[collider as usize] & layer_mask != 0
    }

    /// Continuous collision detection for a ball: the first time in
    /// `[0, max_time]` at which a sphere of `radius` starting at `center` and
    /// moving with constant `velocity` touches an enabled collider that shares
    /// a layer with `layer_mask`.
    ///
    /// The cast is exact for every primitive and cannot tunnel, whatever the
    /// speed. A sphere that already touches or overlaps a collider at time 0
    /// hits it at time 0 only if it moves into the surface; resting, sliding
    /// and separating overlaps are reported by
    /// [`sphere_contacts`](Self::sphere_contacts) instead. Simultaneous hits
    /// resolve to the collider added first. Never allocates.
    #[must_use]
    pub fn cast_sphere(
        &self,
        center: Vec3,
        radius: f32,
        velocity: Vec3,
        max_time: f32,
        layer_mask: u8,
    ) -> Option<SphereHit> {
        if self.prims.is_empty()
            || !(center.is_finite() && velocity.is_finite() && radius.is_finite())
            || !(max_time >= 0.0)
            || !(radius >= 0.0)
        {
            return None;
        }
        let end = center + velocity * max_time;
        if !end.is_finite() {
            return None;
        }
        let pad = Vec3::splat(radius + LEAF_MARGIN);
        let query = Aabb::new(center.min(end) - pad, center.max(end) + pad);
        let mut best: Option<(f32, u32, super::shapes::Closest)> = None;
        for leaf in self.bvh.intersect_aabb(&query) {
            let (prim, collider) = &self.prims[leaf as usize];
            if !self.active(*collider, layer_mask) {
                continue;
            }
            // Later hits than the best so far cannot win.
            let limit = best.map_or(max_time, |(t, _, _)| t);
            if let Some((t, cl)) = prim.cast_sphere(center, radius, velocity, limit) {
                let better = match best {
                    None => true,
                    Some((bt, bleaf, _)) => t < bt || (t == bt && leaf < bleaf),
                };
                if better {
                    best = Some((t, leaf, cl));
                }
            }
        }
        best.map(|(time, leaf, cl)| SphereHit {
            time,
            point: cl.point,
            normal: cl.normal,
            collider: ColliderHandle(self.prims[leaf as usize].1),
        })
    }

    /// Collects the overlaps of a sphere with the enabled colliders that
    /// share a layer with `layer_mask`. `out` is cleared first.
    ///
    /// A sphere touching a surface (depth 0) counts. Coincident contacts on
    /// the same collider (the shared edge of two mesh triangles) are reported
    /// once. If more than 16 distinct contacts exist, the deepest are kept.
    /// Contacts come out in collider insertion order. Never allocates.
    pub fn sphere_contacts(
        &self,
        center: Vec3,
        radius: f32,
        layer_mask: u8,
        out: &mut ArrayVec<SphereContact, MAX_SPHERE_CONTACTS>,
    ) {
        out.clear();
        if self.prims.is_empty() || !(center.is_finite() && radius >= 0.0 && radius.is_finite()) {
            return;
        }
        let pad = Vec3::splat(radius + LEAF_MARGIN);
        let query = Aabb::new(center - pad, center + pad);
        let mut found: ArrayVec<(u32, SphereContact), MAX_SPHERE_CONTACTS> = ArrayVec::new();
        for leaf in self.bvh.intersect_aabb(&query) {
            let (prim, collider) = &self.prims[leaf as usize];
            if !self.active(*collider, layer_mask) {
                continue;
            }
            let cl = prim.closest(center);
            let depth = radius - cl.distance;
            if !(depth >= 0.0) || !cl.valid {
                continue;
            }
            let contact = SphereContact {
                point: cl.point,
                normal: cl.normal,
                depth,
                collider: ColliderHandle(*collider),
            };
            // Merge with a coincident contact of the same collider.
            let tol = 1.0e-5 + 1.0e-4 * radius;
            if let Some(existing) = found.iter_mut().find(|(_, c)| {
                c.collider == contact.collider
                    && c.point.distance_squared(contact.point) <= tol * tol
                    && c.normal.dot(contact.normal) > 0.999
            }) {
                if leaf < existing.0 {
                    *existing = (leaf, contact);
                }
                continue;
            }
            if !found.is_full() {
                found.push((leaf, contact));
                continue;
            }
            // Full: replace the shallowest if this one is deeper.
            let mut shallowest = 0;
            for (i, (_, c)) in found.iter().enumerate() {
                if c.depth < found[shallowest].1.depth {
                    shallowest = i;
                }
            }
            if contact.depth > found[shallowest].1.depth {
                found[shallowest] = (leaf, contact);
            }
        }
        // BVH traversal order is an implementation detail; sort by primitive.
        found.sort_unstable_by_key(|(leaf, _)| *leaf);
        for (_, contact) in found {
            out.push(contact);
        }
    }
}
