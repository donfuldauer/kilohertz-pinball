//! PBR pipeline and pinball materials (PLAN §4 Subphase 5.1).
//!
//! A clustered forward renderer with up to [`MAX_LIGHTS`] dynamic point
//! lights, drawing linear HDR radiance into an `Rgba16Float` target that the
//! HDR post chain (5.2) reads.
//!
//! # Frames and units
//! * **World**: the table frame, z up, metres (as `playfield.glb` from
//!   `pin_tablegen` and the physics). Meshes come in object space with a
//!   column-major model matrix ([`math::Mat4`], `m[column][row]`).
//! * **View**: right-handed, the camera looks down −Z with +Y up and +X
//!   right ([`math::look_at`]); clip space is wgpu's (depth 0..1).
//! * **Light**: a [`PointLight`] of intensity I gives irradiance `I / d²`
//!   (times colour, windowed to 0 at its range); outputs are radiances in the
//!   same units (a white Lambert surface reflects at most `I / (π d²)`).
//!
//! # Parts
//! * GPU-free (testable without an adapter): [`ClusterGrid`] /
//!   [`assign_lights`] (conservative view-space clustering), [`LightList`]
//!   and [`build_light_list`] (per-frame list from snapshot lamp levels),
//!   [`LampWarmup`] / [`LampBank`] (first-order warm-up from simulated time),
//!   [`MaterialKind`] / [`MaterialParams`] (the material table),
//!   [`MeshGeometry`] (test meshes) and [`shader_sources`] (the WGSL, for
//!   naga validation).
//! * GPU: [`PbrRenderer`] — [`PbrRenderer::new`]`(&GpuContext, width,
//!   height)`, [`PbrRenderer::upload_mesh`], [`PbrRenderer::encode`] (into a
//!   command encoder) and [`PbrRenderer::render_to_image`] (render, submit
//!   and read the HDR target back as linear `f32` RGBA, for tests).
//!
//! # Shading model
//! One model for every material (no per-material branches in the shader):
//! metallic-roughness Cook-Torrance (GGX, height-correlated Smith, Schlick)
//! plus Lambert diffuse; a clear-coat lobe (F0 0.04, Kelemen visibility)
//! that attenuates the base layer; wrapped diffuse for thin translucent
//! plastic (subsurface); blended transmission (premultiplied, no depth
//! write); a placeholder environment ([`Environment`]: sky / horizon / ground
//! radiance) for ambient and mirror terms; and three emission terms
//! (constant, GI tint · `gi_emissive`, lamp level · `lamp_emissive`).

mod cluster;
mod light;
mod material;
pub mod math;
mod mesh;
mod renderer;
mod warmup;

pub use cluster::{
    ClusterAssignment, ClusterGrid, DEFAULT_CLUSTER_DIMS, MAX_CLUSTER_DIM, assign_lights,
};
pub use light::{LIGHT_OFF_LEVEL, LampLight, LightList, PointLight, build_light_list};
pub use material::{MaterialKind, MaterialParams};
pub use math::PbrCamera;
pub use mesh::MeshGeometry;
pub use renderer::{
    DEPTH_FORMAT, DrawItem, Environment, FrameDesc, HDR_FORMAT, HdrImage, MeshId, PbrRenderer,
    PbrStats, model_view_proj,
};
pub use warmup::{LampBank, LampKind, LampWarmup};

/// Most dynamic lights per frame; further lights are dropped and counted.
pub const MAX_LIGHTS: usize = 128;

/// The BRDF library (functions only).
const BRDF_SOURCE: &str = include_str!("shaders/brdf.wgsl");

/// The forward shading module (the BRDF library followed by the bindings
/// and entry points `vs_main`, `fs_opaque`, `fs_blended`).
pub(crate) const FORWARD_SOURCE: &str = concat!(
    include_str!("shaders/brdf.wgsl"),
    "\n",
    include_str!("shaders/forward.wgsl")
);

/// `(name, WGSL source)` of every shader module, each complete on its own
/// (for naga validation and translation).
pub fn shader_sources() -> &'static [(&'static str, &'static str)] {
    static SOURCES: [(&str, &str); 2] = [("pbr_brdf", BRDF_SOURCE), ("pbr_forward", FORWARD_SOURCE)];
    &SOURCES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wgsl_light_capacity_matches() {
        let forward = shader_sources()
            .iter()
            .find(|(n, _)| *n == "pbr_forward")
            .unwrap()
            .1;
        assert!(forward.contains(&format!("array<Light, {MAX_LIGHTS}>")));
        assert!(forward.contains(&format!("MAX_LIGHTS: u32 = {MAX_LIGHTS}u")));
        assert!(forward.starts_with(BRDF_SOURCE));
    }
}
