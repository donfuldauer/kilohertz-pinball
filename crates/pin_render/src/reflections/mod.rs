//! Ball reflection probes (PLAN §4 Subphase 5.2).
//!
//! Chrome balls reflect their surroundings through **octahedral probes**: a
//! probe is a [`PROBE_RESOLUTION`]² map of the radiance arriving at a ball
//! centre from every direction (world z-up; [`oct_encode`] /
//! [`oct_decode`]). Probes are captured with the PBR renderer (six cube
//! faces of [`CUBE_FACE_RESOLUTION`]², then remapped to the octahedral
//! layout) under a strict budget: at most [`PROBE_BUDGET`] captures per
//! frame, round-robin across the balls ([`ProbeScheduler`]); a ball without
//! a probe of its own yet reuses the nearest captured one.
//!
//! The main PBR pass samples whatever [`ReflectionSource`] is bound: the
//! octahedral probes ([`OctahedralProbes`]), or no probes at all
//! ([`EnvironmentReflections`], the 5.1 placeholder environment). 5.4 adds a
//! ray-query implementation of the same trait.
//!
//! # Parts
//! * GPU-free: [`oct_encode`] / [`oct_decode`], [`ProbeScheduler`] and
//!   [`ProbeStats`] (the budget and age bookkeeping), [`face_basis`] (the
//!   capture cameras).
//! * GPU: [`OctahedralProbes`], [`EnvironmentReflections`].

mod probes;
mod scheduler;

pub use probes::{EnvironmentReflections, OctahedralProbes, ProbeConfig};
pub use scheduler::{ProbeScheduler, ProbeStats};

use crate::gpu::{GpuContext, GpuError};
use crate::pbr::{FrameDesc, PbrRenderer, ReflectionBinding};

/// Side of an octahedral probe map, texels.
pub const PROBE_RESOLUTION: u32 = 128;

/// Most probe captures per frame.
pub const PROBE_BUDGET: u32 = 2;

/// Most probes (one per ball; equals `pin_core::snapshot::MAX_BALLS`).
pub const MAX_PROBES: usize = pin_core::snapshot::MAX_BALLS;

/// Side of a capture cube face, pixels.
pub const CUBE_FACE_RESOLUTION: u32 = 64;

/// A ball that reflects: its centre and radius (metres, world) and the index
/// of its draw in [`FrameDesc::draws`] (that draw samples the ball's probe).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ReflectiveBall {
    /// Centre (world, m).
    pub position: [f32; 3],
    /// Radius (m).
    pub radius: f32,
    /// Index of the ball's draw in [`FrameDesc::draws`].
    pub draw: usize,
}

/// A source of mirror reflections for the PBR pass.
///
/// Per frame: [`prepare`](Self::prepare) once (it may render and submit its
/// own GPU work, e.g. probe captures), then the main pass binds
/// [`binding`](Self::binding) through
/// [`PbrRenderer::encode_reflected`].
pub trait ReflectionSource {
    /// A short name (logs, tests).
    fn name(&self) -> &'static str;

    /// Brings the reflections up to date for `frame` with `balls`.
    fn prepare(
        &mut self,
        ctx: &GpuContext,
        pbr: &mut PbrRenderer,
        frame: &FrameDesc<'_>,
        balls: &[ReflectiveBall],
    ) -> Result<(), GpuError>;

    /// What the main pass binds (group 2 and the per-draw probe layers of the
    /// last [`prepare`](Self::prepare)).
    fn binding(&self) -> ReflectionBinding<'_>;

    /// Budget counters.
    fn stats(&self) -> ProbeStats;
}

fn sign_not_zero(v: f32) -> f32 {
    if v >= 0.0 { 1.0 } else { -1.0 }
}

/// Unit direction → octahedral map coordinates in `0..=1` (world z-up: +z
/// at the centre `(0.5, 0.5)`, −z at the four corners, +x towards u = 1,
/// +y towards v = 1). Non-unit input is fine (only the direction counts); a
/// zero vector maps to the centre. Same formulas as the WGSL.
pub fn oct_encode(dir: [f32; 3]) -> [f32; 2] {
    let s = dir[0].abs() + dir[1].abs() + dir[2].abs();
    if s.is_nan() || s <= 1e-20 {
        return [0.5, 0.5];
    }
    let (mut x, mut y) = (dir[0] / s, dir[1] / s);
    if dir[2] < 0.0 {
        let (ox, oy) = (x, y);
        x = (1.0 - oy.abs()) * sign_not_zero(ox);
        y = (1.0 - ox.abs()) * sign_not_zero(oy);
    }
    [x * 0.5 + 0.5, y * 0.5 + 0.5]
}

/// Octahedral map coordinates (clamped to `0..=1`) → unit direction; the
/// inverse of [`oct_encode`].
pub fn oct_decode(uv: [f32; 2]) -> [f32; 3] {
    let u = if uv[0].is_nan() { 0.5 } else { uv[0] };
    let v = if uv[1].is_nan() { 0.5 } else { uv[1] };
    let px = u.clamp(0.0, 1.0) * 2.0 - 1.0;
    let py = v.clamp(0.0, 1.0) * 2.0 - 1.0;
    let z = 1.0 - px.abs() - py.abs();
    let (x, y) = if z < 0.0 {
        (
            (1.0 - py.abs()) * sign_not_zero(px),
            (1.0 - px.abs()) * sign_not_zero(py),
        )
    } else {
        (px, py)
    };
    let l = (x * x + y * y + z * z).sqrt();
    [x / l, y / l, z / l]
}

/// The six capture directions: `(forward, up)` per cube face.
const FACES: [([f32; 3], [f32; 3]); 6] = [
    ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
    ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
    ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
    ([0.0, -1.0, 0.0], [0.0, 0.0, 1.0]),
    ([0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
    ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0]),
];

/// `(right, up, forward)` of capture face `face` (`0..6`: +x, −x, +y, −y,
/// +z, −z), taken from the view matrix the capture camera uses (so the
/// remap and the capture can never disagree). A face pixel at `(s, t)` in
/// `0..=1` (t down) looks along `forward + (2s − 1)·right + (1 − 2t)·up`.
pub fn face_basis(face: usize) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let cam = face_camera([0.0; 3], face, 0.01, 10.0);
    let v = &cam.view;
    (
        [v[0][0], v[1][0], v[2][0]],
        [v[0][1], v[1][1], v[2][1]],
        [-v[0][2], -v[1][2], -v[2][2]],
    )
}

/// The 90° capture camera of cube face `face` (`0..6`) at `eye`.
pub(crate) fn face_camera(
    eye: [f32; 3],
    face: usize,
    near: f32,
    far: f32,
) -> crate::pbr::PbrCamera {
    let (f, up) = FACES[face % 6];
    crate::pbr::PbrCamera::look_at(
        eye,
        [eye[0] + f[0], eye[1] + f[1], eye[2] + f[2]],
        up,
        core::f32::consts::FRAC_PI_2,
        1.0,
        near,
        far,
    )
}

#[cfg(test)]
mod tests;
