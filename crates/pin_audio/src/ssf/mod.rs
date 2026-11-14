//! Surround-sound feedback (SSF, Subphase 3.4): a four-corner panner over the
//! 7.1 exciter channels and an offline voice mixer that turns haptic events
//! into 8-channel audio for bass shakers / exciters mounted under the
//! playfield.
//!
//! # Channel map (Orchestrator decision, DECISIONS "Wave 4 brief decisions")
//! SMPTE 7.1 order: [`CH_FL`] 0, [`CH_FR`] 1, [`CH_FC`] 2, [`CH_LFE`] 3,
//! [`CH_BL`] 4, [`CH_BR`] 5, [`CH_SL`] 6, [`CH_SR`] 7. The playfield exciters
//! are the front corners (low y, near the flippers) on SL (left) / SR (right)
//! and the back corners (high y) on BL / BR. FL, FR, FC and LFE are never fed
//! by the panner; they are left to the music/backbox mix.
//!
//! # Panning law
//! [`SsfPanner`] is equal-power bilinear over the playfield rectangle given
//! at construction. With `u = clamp((x − x_min)/(x_max − x_min), 0, 1)` and
//! `w = clamp((y − y_min)/(y_max − y_min), 0, 1)`:
//!
//! ```text
//! SL = cos(u·π/2)·cos(w·π/2)    SR = sin(u·π/2)·cos(w·π/2)
//! BL = cos(u·π/2)·sin(w·π/2)    BR = sin(u·π/2)·sin(w·π/2)
//! ```
//!
//! so `SL² + SR² + BL² + BR² = 1` everywhere. The gains are computed in `f64`
//! with `libm` and rounded once to `f32`. A degenerate axis (`max ≤ min`, or a
//! non-finite bound) and a NaN coordinate both map to the middle of that axis
//! (`u` or `w` = 0.5); ±∞ coordinates clamp like any other.
//!
//! # Voice mixer
//! [`SsfMixer`] renders haptic events into interleaved 8-channel `f32`
//! buffers; see its docs for the voice model. It is pure Rust, so tests run
//! without an audio device.
//!
//! # Interface with the 3.3 mixer
//! The 3.3 [`Mixer`](crate::Mixer) renders interleaved **stereo** (music,
//! callouts, sfx). On a 7.1 device the audio callback renders the 3.3 mixer
//! into a stereo scratch buffer and calls [`SsfMixer::render_with_stereo`],
//! which renders the exciter channels and adds the stereo mix onto FL/FR
//! ([`mix_stereo_front`]); the result is hard-clipped to `[−1, 1]` like the
//! 3.3 master stage. On a stereo device the SSF output is simply not
//! rendered. Both mixers work in frames of the same sample rate.

mod voices;

pub use voices::{MAX_IMPULSE_VOICES, MAX_ROLL_VOICES, SsfConfig, SsfMixer};

/// Front left (SMPTE 7.1 index 0).
pub const CH_FL: usize = 0;
/// Front right (1).
pub const CH_FR: usize = 1;
/// Front centre (2).
pub const CH_FC: usize = 2;
/// Low-frequency effects (3).
pub const CH_LFE: usize = 3;
/// Back left (4): back-left playfield exciter (high y, low x).
pub const CH_BL: usize = 4;
/// Back right (5): back-right playfield exciter (high y, high x).
pub const CH_BR: usize = 5;
/// Side left (6): front-left playfield exciter (low y, low x).
pub const CH_SL: usize = 6;
/// Side right (7): front-right playfield exciter (low y, high x).
pub const CH_SR: usize = 7;
/// Channels in a 7.1 frame.
pub const CHANNELS: usize = 8;

const HALF_PI: f64 = core::f64::consts::FRAC_PI_2;

/// Equal-power four-corner panner (module docs for the law).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SsfPanner {
    x_min: f32,
    x_max: f32,
    y_min: f32,
    y_max: f32,
}

/// Normalised position on one axis (module docs).
fn axis(v: f32, lo: f32, hi: f32) -> f64 {
    let (v, lo, hi) = (f64::from(v), f64::from(lo), f64::from(hi));
    let span = hi - lo;
    if v.is_nan() || !lo.is_finite() || !hi.is_finite() || span <= 0.0 {
        return 0.5;
    }
    ((v - lo) / span).clamp(0.0, 1.0)
}

impl SsfPanner {
    /// A panner over the playfield rectangle `[x_min, x_max] × [y_min, y_max]`
    /// (mm, playfield frame: y grows from the flippers towards the back).
    pub fn new(x_min_mm: f32, x_max_mm: f32, y_min_mm: f32, y_max_mm: f32) -> SsfPanner {
        SsfPanner {
            x_min: x_min_mm,
            x_max: x_max_mm,
            y_min: y_min_mm,
            y_max: y_max_mm,
        }
    }

    /// The rectangle `[x_min, x_max, y_min, y_max]`, mm.
    pub fn bounds(&self) -> [f32; 4] {
        [self.x_min, self.x_max, self.y_min, self.y_max]
    }

    /// The four exciter gains `[SL, SR, BL, BR]` at `(x, y)` mm.
    pub fn corner_gains(&self, x_mm: f32, y_mm: f32) -> [f32; 4] {
        let u = axis(x_mm, self.x_min, self.x_max) * HALF_PI;
        let w = axis(y_mm, self.y_min, self.y_max) * HALF_PI;
        let (cu, su) = (libm::cos(u), libm::sin(u));
        let (cw, sw) = (libm::cos(w), libm::sin(w));
        [
            (cu * cw) as f32,
            (su * cw) as f32,
            (cu * sw) as f32,
            (su * sw) as f32,
        ]
    }

    /// The 7.1 gains at `(x, y)` mm in SMPTE order (FL, FR, FC, LFE = 0).
    pub fn gains(&self, x_mm: f32, y_mm: f32) -> [f32; 8] {
        let [sl, sr, bl, br] = self.corner_gains(x_mm, y_mm);
        let mut g = [0.0; CHANNELS];
        g[CH_SL] = sl;
        g[CH_SR] = sr;
        g[CH_BL] = bl;
        g[CH_BR] = br;
        g
    }
}

/// Adds interleaved stereo `stereo` (the 3.3 mixer's output) onto FL/FR of
/// the interleaved 7.1 buffer `out71`, frame by frame, over the frames both
/// buffers hold. Never allocates.
pub fn mix_stereo_front(stereo: &[f32], out71: &mut [f32]) {
    for (lr, f) in stereo.chunks_exact(2).zip(out71.chunks_exact_mut(CHANNELS)) {
        f[CH_FL] += lr[0];
        f[CH_FR] += lr[1];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corners_and_degenerate_axes() {
        let p = SsfPanner::new(0.0, 100.0, 0.0, 200.0);
        assert_eq!(p.bounds(), [0.0, 100.0, 0.0, 200.0]);
        let g = p.gains(0.0, 0.0);
        assert_eq!(g[CH_SL], 1.0);
        assert!(g[CH_SR].abs() < 1e-7 && g[CH_BL].abs() < 1e-7);
        let g = p.gains(1e9, 1e9);
        assert!((g[CH_BR] - 1.0).abs() < 1e-7);
        let g = p.gains(f32::NAN, -f32::INFINITY);
        assert!((g[CH_SL] - g[CH_SR]).abs() < 1e-7);
        assert!((g[CH_SL] - core::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6);
        for g in [
            SsfPanner::new(5.0, 5.0, 0.0, 1.0).gains(3.0, 0.0),
            SsfPanner::new(f32::NAN, 5.0, 0.0, 1.0).gains(3.0, 0.0),
        ] {
            assert!((g[CH_SL] - g[CH_SR]).abs() < 1e-7);
        }
        for ch in [CH_FL, CH_FR, CH_FC, CH_LFE] {
            assert_eq!(g[ch], 0.0);
        }
    }

    #[test]
    fn stereo_front_mix() {
        let stereo = [0.5, -0.25, 1.0, 0.0];
        let mut out = [0.0f32; 16];
        out[CH_SL] = 0.1;
        mix_stereo_front(&stereo, &mut out);
        assert_eq!(&out[..2], &[0.5, -0.25]);
        assert_eq!(out[CH_SL], 0.1);
        assert_eq!(&out[8..10], &[1.0, 0.0]);
    }
}
