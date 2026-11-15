//! Hardware ray-query reflections (optional, runtime-detected; PLAN §4
//! Subphase 5.4).
//!
//! With [`RAY_QUERY_FEATURE`] on the device, chrome balls reflect each
//! other and the table geometry exactly, and (optionally) so does the
//! playfield clear coat; without it, or after any runtime error, rendering
//! uses the 5.2 reflection probes with identical output. The baseline
//! renderer never depends on this module (PLAN §6), and it ships off by
//! default ([`ReflectionQuality::Off`], D33). The code compiles on every
//! platform; only the runtime decides.
//!
//! # Backend survey (wgpu 30.0.1)
//!
//! Which adapters report `Features::EXPERIMENTAL_RAY_QUERY` on the pinned
//! wgpu 30.0.1 (`tests/raytrace_survey.rs` enumerates the adapters of every
//! backend and prints the bit; DECISIONS "5.4 — ray-query support on wgpu
//! 30.0.1"):
//!
//! | backend | adapter seen | `EXPERIMENTAL_RAY_QUERY` |
//! |---|---|---|
//! | Vulkan | `llvmpipe (LLVM 20.1.2, 256 bits)` — lavapipe, Mesa 25.2.8 (Ubuntu 24.04 `mesa-vulkan-drivers`), CPU | **yes** |
//! | Vulkan | lavapipe of the CI image (Mesa 26.0.8; DECISIONS "Q3" spike) | yes |
//! | GL | `llvmpipe (LLVM 20.1.2, 256 bits)` — OpenGL 4.5 core, Mesa 25.2.8 | no |
//! | DX12 | none on Linux; WARP on the hosted Windows runner (Q3 spike) | no |
//! | Metal | none on Linux; the hosted macOS runner (Q3 spike) | no |
//!
//! Observed: Vulkan (lavapipe) reports the bit; GL does not; the hosted
//! Windows (DX12/WARP) and macOS (Metal) runners did not in the Q3 spike.
//! The ray path is taken only when the device reports and enables
//! `RAY_QUERY_FEATURE` ([`select_reflection_source`] follows the device's
//! feature bit); DX12 and Metal coverage on real hardware is open (PLAN
//! risk row 5).
//!
//! # How it works
//!
//! The PBR pass (5.1/5.2, unchanged) samples one octahedral map per draw
//! from the bound [`ReflectionSource`](crate::reflections::ReflectionSource).
//! [`RayQueryReflections`] fills its own atlas of such maps with a compute
//! shader that issues WGSL ray queries (`shaders/trace.wgsl`) against a
//! top-level acceleration structure (TLAS) of the frame's draws:
//!
//! * **Acceleration structures.** One bottom-level structure (BLAS) per
//!   static mesh, built once when the mesh is registered; one TLAS refitted
//!   every frame from the draws' transforms (balls, flippers and any other
//!   moving mesh). [`RayStats`] counts BLAS builds and TLAS refits.
//! * **Map resolution.** Every atlas layer is a
//!   [`RayQueryConfig::map_resolution`]² octahedral map (default
//!   [`DEFAULT_MAP_RESOLUTION`], 4× the probes' 128 per side), and each ray
//!   cell averages [`RayQueryConfig::supersample`] rays (default 4, a
//!   rotated grid), so reflected hard edges land at their sub-pixel place
//!   on screen instead of in texel-sized steps (DECISIONS "5.4 — ray atlas
//!   map resolution (visual-checklist fix)").
//! * **Balls.** Every texel direction `R` of a ball's map is
//!   traced from the sphere point whose mirror reflection of the eye ray is
//!   `R`, so a neighbouring ball appears at its true size and place (a
//!   probe captured at the centre has no parallax and shows it smaller).
//! * **Clear coat.** With
//!   [`ReflectionQuality::BallsAndClearCoat`], the playfield clear coat
//!   (the first opaque clear-coated draw, taken as the plane of its mesh's
//!   top face) gets one extra map ([`CLEARCOAT_LAYER`]) traced from the eye
//!   mirrored in that plane: exact planar reflections, one ray cell per
//!   [`RayQueryConfig::clearcoat_block`]² texels (default 1: every texel).
//! * **Roughness cutoff.** Surfaces rougher than
//!   [`RayQueryConfig::ray_roughness_cutoff`] (default 0.3) use the probe
//!   result ([`reflection_choice`]). A frame with nothing at or below the
//!   cutoff runs and binds the inner probes: output identical to the probe
//!   path.
//! * **Misses** return the frame's environment (as a probe does), so a map
//!   never goes black. Hits are shaded with the PBR pass's model (same
//!   BRDF library, unshadowed point lights). A mirror-like hit (roughness,
//!   or clear-coat roughness with a clear coat, at or below the cutoff)
//!   takes one further traced ray as its mirror term, whose own hit uses
//!   the environment: two bounces at most, so chrome balls show each
//!   other's reflections. Rougher hits use the environment (one bounce).
//!
//! # Shader translation (naga 30.0.1)
//!
//! The trace module validates with the ray-query capability (and fails
//! without it) and translates to SPIR-V with `OpCapability RayQueryKHR`,
//! the back end wgpu uses for ray queries. Vulkan is confirmed at runtime
//! (lavapipe, the only back end observed to report the feature); the DX12
//! and Metal runtime paths on real hardware are unsurveyed. Asserted in
//! `tests/raytrace_naga.rs`: naga's MSL back end accepts every module at
//! MSL 2.4, the HLSL back end accepts it with default options, and the
//! default MSL language version is rejected (an error containing "MSL
//! 2.4": ray tracing is not supported prior to MSL 2.4).
//!
//! # Fallback
//!
//! [`select_reflection_source`] picks the path ([`choose_reflection_path`]):
//! quality off → probes; feature absent or device lost → probes (logged
//! once, never panics); otherwise [`RayQueryReflections`], which on any
//! GPU error in a frame switches permanently to its inner probes and keeps
//! rendering. The device must have been created with the feature:
//! [`GpuContext::new_headless_with_features`](crate::gpu::GpuContext::new_headless_with_features).

mod plan;
mod quality;
mod select;
mod source;

pub use plan::{
    CLEARCOAT_LAYER, CLEARCOAT_NORMAL_COS, CLEARCOAT_PLANE_TOLERANCE_M, GeometryPool,
    InstanceRecord, LayerPlan, PlanDraw, PoolRange, RAY_ATLAS_LAYERS, RayJob, RayJobKind,
    clearcoat_plane, instance_record, mesh_top, plan_layers, tlas_transform, validate_mesh,
};
pub use quality::{
    DEFAULT_RAY_ROUGHNESS_CUTOFF, ParseReflectionQualityError, ReflectionChoice, ReflectionQuality,
    reflection_choice,
};
pub use select::{
    FallbackReason, RayCapability, RayCaps, ReflectionPath, choose_reflection_path,
    select_reflection_source, select_reflection_source_with,
};
pub use source::{RayQueryError, RayQueryReflections, RayStats};

use crate::reflections::ProbeConfig;

/// The device feature the ray path needs.
pub const RAY_QUERY_FEATURE: wgpu::Features = wgpu::Features::EXPERIMENTAL_RAY_QUERY;

/// The wgpu version the backend survey was taken on.
pub const WGPU_VERSION: &str = "30.0.1";

/// `true` when `adapter` reports [`RAY_QUERY_FEATURE`].
pub fn adapter_supports_ray_query(adapter: &wgpu::Adapter) -> bool {
    adapter.features().contains(RAY_QUERY_FEATURE)
}

/// Ray-query reflection settings.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RayQueryConfig {
    /// Surfaces with a (clear-coat) roughness above this use the probe
    /// result; at or below it they are traced ([`reflection_choice`]).
    pub ray_roughness_cutoff: f32,
    /// The probe path's settings (fallback and rough surfaces).
    pub probes: ProbeConfig,
    /// Longest reflection ray (m); beyond it the environment shows.
    pub max_distance: f32,
    /// Ball ray origins sit this fraction of the radius outside the sphere.
    pub origin_offset: f32,
    /// Clear-coat ray origins sit this far (m) above the plane.
    pub plane_offset: f32,
    /// Side (texels) of every ray-atlas layer: the octahedral map each
    /// traced surface samples (default [`DEFAULT_MAP_RESOLUTION`]). A power
    /// of two or a multiple of 8, at most the device's
    /// `max_texture_dimension_2d`; [`RayQueryReflections::new`] declines
    /// anything else ([`RayQueryError::InvalidConfig`]). The atlas holds
    /// [`RAY_ATLAS_LAYERS`] layers of `map_resolution`² Rgba16Float texels
    /// ([`ray_atlas_bytes`]). 128 with `clearcoat_block` 2 and
    /// `supersample` 1 reproduces the first 5.4 atlas.
    pub map_resolution: u32,
    /// Side (texels) of the square block of the clear-coat layer that
    /// shares one ray cell: 1 (every texel traced) or 2 (2×2 texels share
    /// one result). Ball layers always trace every texel.
    pub clearcoat_block: u32,
    /// Rays averaged per ray cell: 1 (the cell centre) or 4 (a rotated
    /// grid, [`supersample_offsets`]): a reflected hard edge becomes a
    /// coverage gradient across the texel it crosses, so bilinear sampling
    /// puts its 50 % level at the true sub-texel position instead of a
    /// texel-sized step.
    pub supersample: u32,
}

impl Default for RayQueryConfig {
    fn default() -> Self {
        RayQueryConfig {
            ray_roughness_cutoff: DEFAULT_RAY_ROUGHNESS_CUTOFF,
            probes: ProbeConfig::default(),
            max_distance: 20.0,
            origin_offset: 1e-3,
            plane_offset: 1e-4,
            map_resolution: DEFAULT_MAP_RESOLUTION,
            clearcoat_block: DEFAULT_CLEARCOAT_BLOCK,
            supersample: DEFAULT_SUPERSAMPLE,
        }
    }
}

/// Default [`RayQueryConfig::map_resolution`] (DECISIONS "5.4 — ray atlas
/// map resolution (visual-checklist fix)").
pub const DEFAULT_MAP_RESOLUTION: u32 = 512;

/// Default [`RayQueryConfig::clearcoat_block`].
pub const DEFAULT_CLEARCOAT_BLOCK: u32 = 1;

/// Default [`RayQueryConfig::supersample`].
pub const DEFAULT_SUPERSAMPLE: u32 = 4;

/// Sub-cell ray positions (fractions of the cell side, origin at the
/// cell's low corner) for `samples` rays per cell: the centre for 1, the
/// 4-ray rotated grid (every sample on its own row and column, offsets
/// 1/8, 3/8, 5/8, 7/8 on each axis) for 4; `None` for any other count.
/// Fixed, so a frame never depends on an earlier one; the WGSL holds the
/// same table (`SS_ROTATED_GRID` in `shaders/trace.wgsl`).
pub const fn supersample_offsets(samples: u32) -> Option<&'static [[f32; 2]]> {
    match samples {
        1 => Some(&[[0.5, 0.5]]),
        4 => Some(&SS_ROTATED_GRID),
        _ => None,
    }
}

const SS_ROTATED_GRID: [[f32; 2]; 4] = [
    [0.375, 0.125],
    [0.875, 0.375],
    [0.625, 0.875],
    [0.125, 0.625],
];

/// Bytes of the ray atlas at map side `map_resolution`
/// ([`RAY_ATLAS_LAYERS`] layers, 8 bytes per Rgba16Float texel).
pub const fn ray_atlas_bytes(map_resolution: u32) -> u64 {
    RAY_ATLAS_LAYERS as u64 * map_resolution as u64 * map_resolution as u64 * 8
}

/// Checks the resolution fields of `config` against a device whose
/// `max_texture_dimension_2d` is `max_texture_dimension`: `Err` with the
/// reason when [`RayQueryConfig::map_resolution`] is 0, neither a power of
/// two nor a multiple of 8, or above the limit, or when
/// [`RayQueryConfig::clearcoat_block`] is not 1 or 2, or
/// [`RayQueryConfig::supersample`] has no offset table
/// ([`supersample_offsets`]).
pub fn validate_config(config: &RayQueryConfig, max_texture_dimension: u32) -> Result<(), String> {
    let n = config.map_resolution;
    if n == 0 || !(n.is_power_of_two() || n.is_multiple_of(8)) {
        return Err(format!(
            "map_resolution {n} must be a power of two or a multiple of 8 (and not 0)"
        ));
    }
    if n > max_texture_dimension {
        return Err(format!(
            "map_resolution {n} exceeds the device's max_texture_dimension_2d \
             {max_texture_dimension}"
        ));
    }
    if !matches!(config.clearcoat_block, 1 | 2) {
        return Err(format!(
            "clearcoat_block {} must be 1 or 2",
            config.clearcoat_block
        ));
    }
    if supersample_offsets(config.supersample).is_none() {
        return Err(format!("supersample {} must be 1 or 4", config.supersample));
    }
    Ok(())
}

/// The trace module: the ray-query enable directive, the PBR BRDF library
/// and the octahedral helpers of the PBR reflection include (shared with
/// the forward pass), then the trace entry point `cs_trace`.
pub(crate) const TRACE_SOURCE: &str = concat!(
    "enable wgpu_ray_query;\n",
    include_str!("../pbr/shaders/brdf.wgsl"),
    "\n",
    include_str!("../pbr/shaders/reflection.wgsl"),
    "\n",
    include_str!("shaders/trace.wgsl")
);

/// `(name, WGSL source)` of every shader module of this module (complete
/// on its own; needs the ray-query capability to validate).
pub fn shader_sources() -> &'static [(&'static str, &'static str)] {
    static SOURCES: [(&str, &str); 1] = [("raytrace_trace", TRACE_SOURCE)];
    &SOURCES
}

#[cfg(test)]
mod tests;
