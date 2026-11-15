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
//! wgpu 30.0.1 documents the feature as Vulkan-only. So lavapipe on Linux
//! CI validates the ray path, and on Windows / macOS (and GL)
//! [`select_reflection_source`] always takes the probe path.
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
//! * **Balls (full resolution).** Every texel direction `R` of a ball's
//!   [`PROBE_RESOLUTION`](crate::reflections::PROBE_RESOLUTION)² map is
//!   traced from the sphere point whose mirror reflection of the eye ray is
//!   `R`, so a neighbouring ball appears at its true size and place (a
//!   probe captured at the centre has no parallax and shows it smaller).
//! * **Clear coat (half resolution).** With
//!   [`ReflectionQuality::BallsAndClearCoat`], the playfield clear coat
//!   (the first opaque clear-coated draw, taken as the plane of its mesh's
//!   top face) gets one extra map ([`CLEARCOAT_LAYER`]) traced from the eye
//!   mirrored in that plane: exact planar reflections, one ray per 2×2
//!   texels.
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
//! the back end wgpu uses for ray queries (the feature is Vulkan-only).
//! Recorded, not asserted (`tests/raytrace_naga.rs`): naga's MSL back end
//! rejects it at the default language version ("ray tracing is not
//! supported prior to MSL 2.4") and accepts it at MSL 2.4; the HLSL back
//! end accepts it with default options.
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
}

impl Default for RayQueryConfig {
    fn default() -> Self {
        RayQueryConfig {
            ray_roughness_cutoff: DEFAULT_RAY_ROUGHNESS_CUTOFF,
            probes: ProbeConfig::default(),
            max_distance: 20.0,
            origin_offset: 1e-3,
            plane_offset: 1e-4,
        }
    }
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
