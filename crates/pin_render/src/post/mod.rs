//! HDR post-processing (PLAN §4 Subphase 5.2).
//!
//! The linear HDR frame of the PBR pass (`pbr::HDR_FORMAT`) goes through a
//! physically based chain to the display:
//!
//! 1. **Exposure and motion blur** — the frame is scaled by
//!    [`PostConfig::exposure`]; pixels a moving ball sweeps during the
//!    shutter ([`MotionBlurConfig`]) show the mean over the shutter of the
//!    ball, translated along its screen velocity, over the background. A
//!    still ball leaves the image unchanged.
//! 2. **Bloom** — energy conserving: the above-threshold part of each pixel
//!    ([`bloom_prefilter`]: soft-knee threshold on luminance, firefly clamp)
//!    is scattered by a mip-chain point spread function; a fraction
//!    [`BloomConfig::intensity`] of it moves, the rest stays. The total
//!    luminance is unchanged up to border losses.
//! 3. **Tone mapping** — ACES filmic fit ([`aces_film`]) or a linear clamp,
//!    then the sRGB curve unless the output format encodes it.
//!
//! Anti-aliasing is MSAA 4× in the PBR pass ([`AaMode`]), applied by
//! [`FrameRenderer`], which also runs the reflection probes.
//!
//! # Parts
//! * GPU-free: [`PostConfig`] and its parts, [`aces_film`],
//!   [`bloom_prefilter`], [`luminance`], [`screen_motion`], [`LdrImage`].
//! * GPU: [`PostChain`], [`HdrInput`], [`FrameRenderer`].

mod chain;
mod frame;
pub(crate) mod readback;

pub use chain::{HdrInput, MAX_BLUR_BALLS, PostChain, PostStats, shader_sources};
pub use frame::{FrameRenderer, RenderedFrame, SceneBall};

use crate::pbr::PbrCamera;

/// Bloom settings (see the [module docs](self)).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BloomConfig {
    /// Bloom on (off = the image passes unchanged).
    pub enabled: bool,
    /// Luminance (after exposure) where the bloom starts.
    pub threshold: f32,
    /// Soft-knee half width around the threshold (clamped to
    /// `0..=threshold`).
    pub knee: f32,
    /// Fraction of the above-threshold light that is scattered (`0..=1`).
    pub intensity: f32,
    /// Weight of the wider levels (`0..=1`): level `i` of `L` gets
    /// `(1 − radius)·radiusⁱ`, the last `radius^(L−1)`.
    pub radius: f32,
    /// Largest luminance a pixel feeds into the bloom (firefly clamp;
    /// `≤ 0` = off).
    pub firefly_clamp: f32,
    /// Most mip levels (each half the previous size, ≥ 1 pixel).
    pub max_levels: u32,
}

impl Default for BloomConfig {
    fn default() -> Self {
        BloomConfig {
            enabled: true,
            threshold: 1.0,
            knee: 0.5,
            intensity: 0.15,
            radius: 0.65,
            firefly_clamp: 64.0,
            max_levels: 6,
        }
    }
}

/// Tone-mapping curve.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ToneMap {
    /// ACES filmic fit (Narkowicz 2015), per channel: [`aces_film`].
    #[default]
    AcesFilm,
    /// Clamp to `0..=1` (tests).
    Linear,
}

/// Velocity-based motion blur of the balls.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MotionBlurConfig {
    /// Blur on.
    pub enabled: bool,
    /// Shutter time (s): a ball's streak is its motion over this time.
    pub shutter_s: f32,
    /// Samples along the streak (1..=64).
    pub samples: u32,
    /// Longest streak (pixels); longer ones are shortened.
    pub max_length_px: f32,
}

impl Default for MotionBlurConfig {
    fn default() -> Self {
        MotionBlurConfig {
            enabled: true,
            shutter_s: 1.0 / 120.0,
            samples: 16,
            max_length_px: 64.0,
        }
    }
}

/// Anti-aliasing mode.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum AaMode {
    /// None.
    Off,
    /// MSAA 4× in the PBR pass, hardware-resolved.
    #[default]
    Msaa4,
}

impl AaMode {
    /// PBR sample count of this mode.
    pub const fn samples(self) -> u32 {
        match self {
            AaMode::Off => 1,
            AaMode::Msaa4 => 4,
        }
    }
}

/// The whole post chain configuration.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PostConfig {
    /// Linear scale applied to the scene radiance first.
    pub exposure: f32,
    /// Bloom.
    pub bloom: BloomConfig,
    /// Tone-mapping curve.
    pub tone_map: ToneMap,
    /// Motion blur.
    pub motion_blur: MotionBlurConfig,
    /// Anti-aliasing.
    pub aa: AaMode,
}

impl Default for PostConfig {
    fn default() -> Self {
        PostConfig {
            exposure: 1.0,
            bloom: BloomConfig::default(),
            tone_map: ToneMap::AcesFilm,
            motion_blur: MotionBlurConfig::default(),
            aa: AaMode::Msaa4,
        }
    }
}

impl PostConfig {
    /// Everything off: exposure 1, no bloom, no blur, linear clamp, no AA
    /// (the post chain is then the identity up to 8-bit encoding).
    pub fn neutral() -> PostConfig {
        PostConfig {
            exposure: 1.0,
            bloom: BloomConfig {
                enabled: false,
                ..BloomConfig::default()
            },
            tone_map: ToneMap::Linear,
            motion_blur: MotionBlurConfig {
                enabled: false,
                ..MotionBlurConfig::default()
            },
            aa: AaMode::Off,
        }
    }
}

/// Non-finite and negative values → 0.
fn finite_non_negative(x: f32) -> f32 {
    if x.is_finite() { x.max(0.0) } else { 0.0 }
}

/// Rec. 709 luminance of linear RGB.
pub fn luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

/// ACES filmic tone-mapping fit (Krzysztof Narkowicz, 2015), one channel:
/// `clamp(x(2.51x + 0.03) / (x(2.43x + 0.59) + 0.14), 0, 1)`, no implicit
/// exposure scale. NaN and negative input → 0. Same as the shader.
pub fn aces_film(x: f32) -> f32 {
    let x = if x.is_nan() { 0.0 } else { x.max(0.0) };
    if x.is_infinite() {
        return 1.0;
    }
    ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0)
}

/// The part of a pixel (linear, after exposure) that feeds the bloom: a
/// soft-knee threshold on luminance (`max(soft, L − t) / L` of the colour,
/// with `soft = clamp(L − t + k, 0, 2k)² / 4k`), then scaled so its
/// luminance is at most [`BloomConfig::firefly_clamp`]. Every channel of
/// the result lies in `0..=` the input's. Same as the shader.
pub fn bloom_prefilter(rgb: [f32; 3], cfg: &BloomConfig) -> [f32; 3] {
    let c = rgb.map(finite_non_negative);
    let l = luminance(c);
    if l <= 0.0 {
        return [0.0; 3];
    }
    let t = finite_non_negative(cfg.threshold);
    let knee = finite_non_negative(cfg.knee).min(t);
    let soft = (l - t + knee).clamp(0.0, 2.0 * knee);
    let soft = soft * soft / (4.0 * knee + 1e-5);
    let contrib = (soft.max(l - t) / l).clamp(0.0, 1.0);
    let mut scale = contrib;
    let fc = cfg.firefly_clamp;
    if fc.is_finite() && fc > 0.0 && l * contrib > fc {
        scale = fc / l;
    }
    c.map(|v| v * scale)
}

/// A ball for motion blur: centre and velocity (world, SI) and radius (m).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MotionBall {
    /// Centre (m).
    pub position: [f32; 3],
    /// Velocity (m/s).
    pub velocity: [f32; 3],
    /// Radius (m).
    pub radius: f32,
}

impl MotionBall {
    /// From a snapshot ball state.
    pub fn from_state(state: &pin_core::snapshot::BallState, radius: f32) -> MotionBall {
        MotionBall {
            position: state.position_m,
            velocity: state.velocity_m_s,
            radius,
        }
    }
}

/// A ball's footprint and streak on screen (pixel units, origin top-left,
/// pixel centres at +½).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ScreenMotion {
    /// Projected centre.
    pub center_px: [f32; 2],
    /// Projected radius (at the centre's depth).
    pub radius_px: f32,
    /// Screen displacement over the shutter (end minus start).
    pub displacement_px: [f32; 2],
}

/// Projects `p` (world) to pixel coordinates and view depth, or `None`
/// behind the near plane.
fn project_px(camera: &PbrCamera, p: [f32; 3], width: u32, height: u32) -> Option<([f32; 2], f32)> {
    let m = camera.view_proj();
    let c: [f32; 4] =
        core::array::from_fn(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r]);
    if !(c[3] > camera.near * 0.5) || !c.iter().all(|v| v.is_finite()) {
        return None;
    }
    let (nx, ny) = (c[0] / c[3], c[1] / c[3]);
    Some((
        [
            (nx + 1.0) * 0.5 * width as f32,
            (1.0 - ny) * 0.5 * height as f32,
        ],
        c[3],
    ))
}

/// Where `ball` is on a `width`×`height` image from `camera`, and how far
/// it moves over `shutter_s` (from `position − ½·v·shutter` to
/// `position + ½·v·shutter`). `None` when the ball centre or either end of
/// its motion is behind the camera, or an input is not finite.
pub fn screen_motion(
    camera: &PbrCamera,
    ball: &MotionBall,
    shutter_s: f32,
    width: u32,
    height: u32,
) -> Option<ScreenMotion> {
    let shutter = finite_non_negative(shutter_s);
    let half = |s: f32| {
        core::array::from_fn::<f32, 3, _>(|k| ball.position[k] + s * 0.5 * shutter * ball.velocity[k])
    };
    let (center, depth) = project_px(camera, ball.position, width, height)?;
    let (a, _) = project_px(camera, half(-1.0), width, height)?;
    let (b, _) = project_px(camera, half(1.0), width, height)?;
    // Projected radius: r · (focal length in pixels) / depth.
    let focal_px = camera.proj[1][1] * 0.5 * height as f32;
    let radius_px = finite_non_negative(ball.radius) * focal_px / depth;
    let out = ScreenMotion {
        center_px: center,
        radius_px,
        displacement_px: [b[0] - a[0], b[1] - a[1]],
    };
    let finite = out.center_px.iter().all(|v| v.is_finite())
        && out.radius_px.is_finite()
        && out.displacement_px.iter().all(|v| v.is_finite());
    finite.then_some(out)
}

/// What the post chain needs per frame besides the image.
#[derive(Copy, Clone, Debug)]
pub struct PostFrame<'a> {
    /// The camera the image was rendered with.
    pub camera: &'a PbrCamera,
    /// Balls to motion-blur (at most [`MAX_BLUR_BALLS`] are used).
    pub balls: &'a [MotionBall],
}

/// An 8-bit display image read back from the GPU: RGBA, `pixels[y · width
/// + x]`, row 0 at the top (BGRA outputs are swizzled to RGBA).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LdrImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// RGBA pixels.
    pub pixels: Vec<[u8; 4]>,
}

impl LdrImage {
    /// Pixel at column `x`, row `y`.
    pub fn at(&self, x: u32, y: u32) -> [u8; 4] {
        self.pixels[(y * self.width + x) as usize]
    }

    /// Mean RGBA (0..=255) over columns `x0..x1`, rows `y0..y1` (clamped;
    /// zero for an empty region).
    pub fn region_mean(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> [f32; 4] {
        let (x1, y1) = (x1.min(self.width), y1.min(self.height));
        let mut sum = [0u64; 4];
        let mut n = 0u64;
        for y in y0..y1 {
            for x in x0..x1 {
                for (s, v) in sum.iter_mut().zip(self.at(x, y)) {
                    *s += u64::from(v);
                }
                n += 1;
            }
        }
        if n == 0 {
            return [0.0; 4];
        }
        sum.map(|s| (s as f64 / n as f64) as f32)
    }

    /// Tightly packed RGBA8.
    pub fn to_rgba8(&self) -> Vec<u8> {
        self.pixels.iter().flatten().copied().collect()
    }
}

#[cfg(test)]
mod tests;
