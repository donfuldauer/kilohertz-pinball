//! Backbox render pass (PLAN §4 Subphase 5.5).
//!
//! Owner: Subphase 5.5.
//!
//! A table may declare `[backbox]` in `backglass.toml`: a separate 3D scene
//! (`scene`, a glTF) seen through the translite by its own camera
//! (`camera = { position, look_at, fov_deg }`, mm and degrees, in the
//! scene's frame: z up). This module draws it and composites it into the
//! backglass.
//!
//! # Parts
//! * GPU-free: [`BackboxScene`] (named meshes with colours and transforms;
//!   [`BackboxScene::from_gltf`] from a loaded glTF), [`BackboxView`] (the
//!   camera; [`BackboxView::project`] for analytic tests),
//!   [`layers_behind`] / [`manifest_layers_behind`] (the depth split),
//!   [`reel_scene`] (score-reel drums with 5 × 7 digits) and the toys
//!   ([`ToyRig`]: score reels and eased mechanism channels).
//! * GPU: [`BackboxRenderer`] (offscreen colour + depth target sized from the
//!   backglass art rectangle, cleared transparent, depth-tested, two-sided
//!   Lambert with an ambient floor) and [`encode_backglass_with_backbox`] /
//!   [`render_backglass_with_backbox_to_image`].
//!
//! # Depth compositing (DECISIONS.md "5.5 — Backbox depth compositing")
//! The translite layers are drawn back to front by `depth` (0 = back). The
//! backbox sits at a plane depth (default [`DEFAULT_PLANE_DEPTH`] = 0.5):
//! layers with `depth < plane` are drawn first (behind it), then the backbox
//! image is composited with premultiplied "over" (opaque where the scene
//! drew, transparent elsewhere, so the background shows around it), then the
//! layers with `depth ≥ plane` over it (frame, logo, lamp-lit art). Inside
//! the backbox image, depth is resolved per pixel by its own depth buffer;
//! between the backbox and the translite there is no depth test (the layers
//! are flat art at fixed depths), so they cannot z-fight. A backglass
//! without `[backbox]` keeps using [`crate::displays::BackglassRenderer`]
//! unchanged (byte-identical output).
//!
//! # Toys and simulation state
//! [`BackboxRenderer::apply_toys`] sets the dynamic transforms of the meshes
//! a [`ToyRig`] names; a dynamic transform is applied on top of the mesh's
//! own (`dynamic · base`). Flipper bats and balls on the backbox are drawn
//! from simulation state the same way
//! ([`BackboxRenderer::set_dynamic_by_name`], [`BackboxRenderer::set_balls`]).

mod renderer;
mod scene;
#[cfg(test)]
mod tests;
mod toys;

pub use renderer::{
    BACKBOX_COLOR_FORMAT, BACKBOX_DEPTH_FORMAT, BALL_COLOR, BALL_RADIUS_M, BackboxLighting,
    BackboxRenderer, MAX_BACKBOX_BALLS, encode_backglass_with_backbox,
    render_backglass_with_backbox_to_image, shader_sources,
};
pub use scene::{
    BACKBOX_FAR_M, BACKBOX_NEAR_M, BackboxMesh, BackboxScene, BackboxView, DEFAULT_PLANE_DEPTH,
    REEL_DIGIT_COLOR, REEL_DRUM_COLOR, ReelSpec, compose, layers_behind, linear_to_srgb,
    manifest_layers_behind, reel_scene,
};
pub use toys::{
    DIGIT_DEG, MechChannel, MechMotion, REEL_DIGITS, ScoreReel, ToyRig, digit_at, reel_angle_deg,
    rotation_about, score_digit, shortest_delta_deg, wrap_deg,
};
