//! GPU-free planning: which atlas layer each reflective draw samples, what
//! each layer traces, and the packed scene data the trace shader reads.

use super::quality::{ReflectionChoice, ReflectionQuality, reflection_choice};
use crate::pbr::math::{Mat4, normal_matrix, transform_point};
use crate::pbr::{DrawItem, MaterialParams, MeshGeometry, NO_PROBE};
use crate::reflections::{MAX_PROBES, ReflectiveBall};

/// Atlas layer of the playfield clear-coat map (after the
/// [`MAX_PROBES`] ball layers).
pub const CLEARCOAT_LAYER: u32 = MAX_PROBES as u32;

/// Layers of the ray-query atlas: one per ball plus the clear coat.
pub const RAY_ATLAS_LAYERS: u32 = MAX_PROBES as u32 + 1;

/// Draws whose clear-coat plane is within this distance (m) of the first
/// traced clear-coat plane share its map.
pub const CLEARCOAT_PLANE_TOLERANCE_M: f32 = 1e-3;

/// … and whose plane normal is within this angle (cosine of 1°).
pub const CLEARCOAT_NORMAL_COS: f32 = 0.999_847_7;

/// What one atlas layer traces (world frame, metres).
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum RayJobKind {
    /// A smooth ball seen from the camera: texel direction `R` is traced
    /// from the sphere point whose mirror reflection of the eye ray is `R`
    /// (exact for a sphere).
    BallView {
        /// Ball centre.
        centre: [f32; 3],
        /// Ball radius.
        radius: f32,
    },
    /// A rough ball (above the cutoff) in a frame that traces other
    /// surfaces: the probe-equivalent map, rays leave the sphere radially
    /// (what a probe at the centre sees, without the capture budget).
    BallCentre {
        /// Ball centre.
        centre: [f32; 3],
        /// Ball radius.
        radius: f32,
    },
    /// A planar clear coat: texel direction `R` is traced from the plane
    /// point where the eye ray reflects into `R` (exact for a plane),
    /// computed at half resolution.
    Plane {
        /// A point on the plane.
        point: [f32; 3],
        /// Unit plane normal (the reflecting side).
        normal: [f32; 3],
    },
}

/// One traced atlas layer.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RayJob {
    /// Atlas layer written.
    pub layer: u32,
    /// What it traces.
    pub kind: RayJobKind,
    /// Traced at half resolution (2×2 texel blocks share one ray).
    pub half_res: bool,
}

/// The layer plan of one frame ([`plan_layers`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LayerPlan {
    /// Layers to trace, in layer order.
    pub jobs: Vec<RayJob>,
    /// Atlas layer per draw ([`NO_PROBE`] = the environment), one entry per
    /// draw of the frame.
    pub draw_layers: Vec<u32>,
    /// `true` when at least one surface is at or below the cutoff and so
    /// gets a ray-traced reflection; otherwise the probe result is used for
    /// the whole frame.
    pub traced: bool,
}

/// What [`plan_layers`] needs of a draw.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PlanDraw {
    /// The draw's material.
    pub material: MaterialParams,
    /// Object → world.
    pub model: Mat4,
    /// Drawn in the blended pass.
    pub transparent: bool,
    /// Largest object-space z of the draw's mesh ([`mesh_top`]); `None`
    /// when the mesh is not registered for ray queries.
    pub mesh_top: Option<f32>,
}

impl PlanDraw {
    /// The plan view of `draw` whose mesh top is `mesh_top`.
    pub fn of(draw: &DrawItem, mesh_top: Option<f32>) -> PlanDraw {
        PlanDraw {
            material: draw.material,
            model: draw.model,
            transparent: draw.transparent,
            mesh_top,
        }
    }
}

/// Plans one frame into `out` (reusing its storage):
///
/// * Ball `i < MAX_PROBES` (whose draw exists) gets layer `i`:
///   [`BallView`](RayJobKind::BallView) when its roughness is at or below
///   `cutoff` ([`reflection_choice`]), else
///   [`BallCentre`](RayJobKind::BallCentre). Further balls sample the
///   environment.
/// * With [`ReflectionQuality::BallsAndClearCoat`]: the first opaque
///   non-ball draw with a clear coat (`clearcoat > 0`) whose clear-coat
///   roughness is at or below `cutoff` and whose mesh is known
///   ([`PlanDraw::mesh_top`]) defines the
///   clear-coat plane (the mesh's top face, normal = the model's +z);
///   every such draw on that plane gets [`CLEARCOAT_LAYER`].
/// * [`LayerPlan::traced`] is `true` when any layer is a `BallView` or the
///   plane. With [`ReflectionQuality::Off`] nothing is planned.
pub fn plan_layers(
    quality: ReflectionQuality,
    cutoff: f32,
    draws: &[PlanDraw],
    balls: &[ReflectiveBall],
    out: &mut LayerPlan,
) {
    out.jobs.clear();
    out.draw_layers.clear();
    out.draw_layers.resize(draws.len(), NO_PROBE);
    out.traced = false;
    if !quality.traces_balls() {
        return;
    }
    for (i, b) in balls.iter().enumerate().take(MAX_PROBES) {
        let Some(draw) = draws.get(b.draw) else {
            continue;
        };
        let centre = b.position;
        let radius = if b.radius.is_finite() {
            b.radius.abs()
        } else {
            0.0
        };
        if !centre.iter().all(|c| c.is_finite()) {
            continue;
        }
        let kind = match reflection_choice(draw.material.roughness, cutoff) {
            ReflectionChoice::Ray => {
                out.traced = true;
                RayJobKind::BallView { centre, radius }
            }
            ReflectionChoice::Probe => RayJobKind::BallCentre { centre, radius },
        };
        out.jobs.push(RayJob {
            layer: i as u32,
            kind,
            half_res: false,
        });
        out.draw_layers[b.draw] = i as u32;
    }
    if !quality.traces_clear_coat() {
        return;
    }
    let is_ball = |d: usize| balls.iter().take(MAX_PROBES).any(|b| b.draw == d);
    let mut plane: Option<([f32; 3], [f32; 3])> = None;
    for (d, draw) in draws.iter().enumerate() {
        let coated = draw.material.clearcoat > 0.0;
        if draw.transparent
            || !coated
            || reflection_choice(draw.material.clearcoat_roughness, cutoff) != ReflectionChoice::Ray
            || is_ball(d)
        {
            continue;
        }
        let Some(top) = draw.mesh_top else {
            continue;
        };
        let Some((p, n)) = clearcoat_plane(&draw.model, top) else {
            continue;
        };
        match plane {
            None => {
                plane = Some((p, n));
                out.draw_layers[d] = CLEARCOAT_LAYER;
            }
            Some((p0, n0)) => {
                let off = dot(sub(p, p0), n0).abs();
                if dot(n, n0) >= CLEARCOAT_NORMAL_COS && off <= CLEARCOAT_PLANE_TOLERANCE_M {
                    out.draw_layers[d] = CLEARCOAT_LAYER;
                }
            }
        }
    }
    if let Some((point, normal)) = plane {
        out.traced = true;
        out.jobs.push(RayJob {
            layer: CLEARCOAT_LAYER,
            kind: RayJobKind::Plane { point, normal },
            half_res: true,
        });
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The clear-coat plane of a draw: the object-space plane `z = top`
/// transformed by `model` (point, unit normal). `None` for a degenerate or
/// non-finite transform.
pub fn clearcoat_plane(model: &Mat4, top: f32) -> Option<([f32; 3], [f32; 3])> {
    let nm = normal_matrix(model);
    let n = [nm[2][0], nm[2][1], nm[2][2]];
    let l = dot(n, n).sqrt();
    if !l.is_finite() || l < 1e-12 {
        return None;
    }
    let n = [n[0] / l, n[1] / l, n[2] / l];
    let p = transform_point(model, [0.0, 0.0, top]);
    p.iter().all(|c| c.is_finite()).then_some((p, n))
}

/// Row-major 3×4 transform of a TLAS instance from a column-major model
/// matrix (`m[column][row]`).
pub fn tlas_transform(model: &Mat4) -> [f32; 12] {
    let m = model;
    [
        m[0][0], m[1][0], m[2][0], m[3][0], //
        m[0][1], m[1][1], m[2][1], m[3][1], //
        m[0][2], m[1][2], m[2][2], m[3][2],
    ]
}

/// Static mesh data of all registered meshes, as the trace shader reads it:
/// interleaved `(position, 1)`, `(normal, 0)` pairs and mesh-local
/// triangle indices.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeometryPool {
    /// `2 · vertex` = position, `2 · vertex + 1` = normal.
    pub vertices: Vec<[f32; 4]>,
    /// Triangle indices, local to each mesh.
    pub indices: Vec<u32>,
}

/// Where a mesh lives in the [`GeometryPool`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct PoolRange {
    /// First vertex (in vertices, not `[f32; 4]` entries).
    pub first_vertex: u32,
    /// First index.
    pub first_index: u32,
    /// Triangles.
    pub triangles: u32,
}

impl GeometryPool {
    /// Appends `mesh` after checking it (non-empty triangle list, indices
    /// in range, finite positions; missing or short normals are stored as
    /// zero, which the shader replaces by the face normal).
    pub fn add(&mut self, mesh: &MeshGeometry) -> Result<PoolRange, String> {
        validate_mesh(mesh)?;
        let first_vertex = u32::try_from(self.vertices.len() / 2)
            .map_err(|_| "geometry pool exceeds u32 vertices".to_owned())?;
        let first_index = u32::try_from(self.indices.len())
            .map_err(|_| "geometry pool exceeds u32 indices".to_owned())?;
        self.vertices.reserve(mesh.positions.len() * 2);
        for (i, p) in mesh.positions.iter().enumerate() {
            let n = mesh
                .normals
                .get(i)
                .copied()
                .filter(|n| n.iter().all(|c| c.is_finite()))
                .unwrap_or([0.0; 3]);
            self.vertices.push([p[0], p[1], p[2], 1.0]);
            self.vertices.push([n[0], n[1], n[2], 0.0]);
        }
        self.indices.extend_from_slice(&mesh.indices);
        Ok(PoolRange {
            first_vertex,
            first_index,
            triangles: (mesh.indices.len() / 3) as u32,
        })
    }
}

/// Checks that `mesh` can become a bottom-level acceleration structure.
pub fn validate_mesh(mesh: &MeshGeometry) -> Result<(), String> {
    if mesh.indices.is_empty() || !mesh.indices.len().is_multiple_of(3) {
        return Err(format!(
            "a mesh needs a non-empty triangle list (got {} indices)",
            mesh.indices.len()
        ));
    }
    let n = mesh.positions.len();
    if n == 0 || u32::try_from(n).is_err() {
        return Err(format!("unsupported vertex count {n}"));
    }
    if let Some(i) = mesh.indices.iter().find(|&&i| i as usize >= n) {
        return Err(format!("index {i} out of range ({n} vertices)"));
    }
    if !mesh.positions.iter().flatten().all(|c| c.is_finite()) {
        return Err("non-finite vertex position".to_owned());
    }
    Ok(())
}

/// Largest object-space z of `mesh` (the clear-coat plane height); `None`
/// for an empty mesh.
pub fn mesh_top(mesh: &MeshGeometry) -> Option<f32> {
    mesh.positions
        .iter()
        .map(|p| p[2])
        .filter(|z| z.is_finite())
        .reduce(f32::max)
}

/// Per-instance shading data of the trace shader (80 bytes, std430).
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct InstanceRecord {
    /// Base colour (rgb), alpha 1.
    pub base_color: [f32; 4],
    /// Constant emission (rgb), metallic (w).
    pub emissive_metallic: [f32; 4],
    /// Roughness, specular, GI emissive, lamp emission (`lamp_emissive ·
    /// level`).
    pub surface: [f32; 4],
    /// Clear coat, clear-coat roughness, transmission, subsurface.
    pub coat: [f32; 4],
    /// First index, first vertex, triangle count, unused.
    pub geometry: [u32; 4],
}

const _: () = assert!(core::mem::size_of::<InstanceRecord>() == 80);

/// The record of a draw with `material` and lamp level `emission` whose
/// mesh lives at `range`. Non-finite inputs become 0 (as the PBR pass
/// sanitises its inputs).
pub fn instance_record(
    material: &MaterialParams,
    emission: f32,
    range: PoolRange,
) -> InstanceRecord {
    let f = |v: f32| if v.is_finite() { v } else { 0.0 };
    let m = material;
    let level = f(emission).clamp(0.0, 1.0);
    InstanceRecord {
        base_color: [
            f(m.base_color[0]).max(0.0),
            f(m.base_color[1]).max(0.0),
            f(m.base_color[2]).max(0.0),
            1.0,
        ],
        emissive_metallic: [
            f(m.emissive[0]).max(0.0),
            f(m.emissive[1]).max(0.0),
            f(m.emissive[2]).max(0.0),
            f(m.metallic).clamp(0.0, 1.0),
        ],
        surface: [
            f(m.roughness).clamp(0.0, 1.0),
            f(m.specular).max(0.0),
            f(m.gi_emissive).max(0.0),
            f(m.lamp_emissive).max(0.0) * level,
        ],
        coat: [
            f(m.clearcoat).clamp(0.0, 1.0),
            f(m.clearcoat_roughness).clamp(0.0, 1.0),
            f(m.transmission).clamp(0.0, 1.0),
            f(m.subsurface).clamp(0.0, 1.0),
        ],
        geometry: [range.first_index, range.first_vertex, range.triangles, 0],
    }
}
