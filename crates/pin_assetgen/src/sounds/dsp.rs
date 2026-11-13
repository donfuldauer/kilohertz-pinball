//! Small, deterministic synthesis helpers. Plain `f64` arithmetic in a fixed
//! order, `libm` for every transcendental function, no `mul_add`.

use crate::rng::Pcg32;

pub(crate) const TAU: f64 = core::f64::consts::TAU;

/// Peak level every rendered sound is normalised to: 0.89 ≈ −1.0 dBFS, well
/// under the −0.1 dBFS ceiling even after 16-bit rounding.
pub const TARGET_PEAK: f64 = 0.89;

/// Shortest attack ramp of any tone or noise burst (0.5 ms), so no sound
/// starts with a step.
pub(crate) const MIN_ATTACK: f64 = 0.0005;

/// A noise colouring filter.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Filter {
    /// One-pole low-pass at the given cut-off (Hz).
    Low(f64),
    /// One-pole high-pass at the given cut-off (Hz).
    High(f64),
    /// Two-pole band-pass (RBJ, 0 dB peak) at the given centre (Hz) and Q.
    Band(f64, f64),
}

/// One-pole smoothing coefficient for cut-off `fc` at rate `sr`.
pub(crate) fn one_pole(fc: f64, sr: f64) -> f64 {
    1.0 - libm::exp(-TAU * fc.min(0.45 * sr) / sr)
}

/// RBJ band-pass biquad.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Biquad {
    b0: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    pub(crate) fn band(fc: f64, q: f64, sr: f64) -> Biquad {
        let w0 = TAU * fc.min(0.45 * sr) / sr;
        let alpha = libm::sin(w0) / (2.0 * q.max(0.1));
        let a0 = 1.0 + alpha;
        Biquad {
            b0: alpha / a0,
            b2: -alpha / a0,
            a1: -2.0 * libm::cos(w0) / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    pub(crate) fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Stateful filter applying a [`Filter`] sample by sample.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FilterState {
    Low { a: f64, y: f64 },
    High { a: f64, y: f64 },
    Band(Biquad),
}

impl FilterState {
    pub(crate) fn new(f: Filter, sr: f64) -> FilterState {
        match f {
            Filter::Low(fc) => FilterState::Low {
                a: one_pole(fc, sr),
                y: 0.0,
            },
            Filter::High(fc) => FilterState::High {
                a: one_pole(fc, sr),
                y: 0.0,
            },
            Filter::Band(fc, q) => FilterState::Band(Biquad::band(fc, q, sr)),
        }
    }

    pub(crate) fn run(&mut self, x: f64) -> f64 {
        match self {
            FilterState::Low { a, y } => {
                *y += *a * (x - *y);
                *y
            }
            FilterState::High { a, y } => {
                *y += *a * (x - *y);
                x - *y
            }
            FilterState::Band(b) => b.run(x),
        }
    }
}

/// MIDI note number to frequency (A4 = 440 Hz).
pub(crate) fn midi_hz(note: f64) -> f64 {
    440.0 * libm::pow(2.0, (note - 69.0) / 12.0)
}

/// A mono buffer being synthesised at a fixed rate.
pub(crate) struct Synth {
    pub(crate) sr: f64,
    pub(crate) out: Vec<f64>,
}

impl Synth {
    /// `seconds` of silence at `sr`.
    pub(crate) fn new(sr: u32, seconds: f64) -> Synth {
        let sr = f64::from(sr);
        let n = libm::round(seconds * sr) as usize;
        Synth {
            sr,
            out: vec![0.0; n.max(1)],
        }
    }

    /// Sample index of time `t` seconds.
    pub(crate) fn at(&self, t: f64) -> usize {
        libm::round(t * self.sr) as usize
    }

    /// A pitched tone starting at `start` s: frequency glides exponentially
    /// from `f0` to `f1` with time constant `glide` s, decays with time
    /// constant `tau` s after a `attack` s linear attack, lasts `dur` s, and
    /// has harmonics `h` (1-based) with amplitudes `amps[h - 1]`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn tone(
        &mut self,
        start: f64,
        dur: f64,
        f0: f64,
        f1: f64,
        glide: f64,
        attack: f64,
        tau: f64,
        amp: f64,
        amps: &[f64],
    ) {
        let s0 = self.at(start);
        let n = self.at(dur);
        let nyq = 0.45 * self.sr;
        let mut phase = 0.0f64;
        let fade = self.at(0.004).max(1);
        for i in 0..n {
            let idx = s0 + i;
            if idx >= self.out.len() {
                break;
            }
            let t = i as f64 / self.sr;
            let f = f1 + (f0 - f1) * libm::exp(-t / glide.max(1e-6));
            let att = if attack > 0.0 {
                (t / attack.max(MIN_ATTACK)).min(1.0)
            } else {
                1.0
            };
            let tail = ((n - i) as f64 / fade as f64).min(1.0);
            let env = amp * att * tail * libm::exp(-t / tau.max(1e-6));
            let mut v = 0.0;
            for (h, &a) in amps.iter().enumerate() {
                let hf = f * (h + 1) as f64;
                if hf < nyq {
                    v += a * libm::sin(TAU * phase * (h + 1) as f64);
                }
            }
            self.out[idx] += env * v;
            phase += f / self.sr;
            phase -= libm::floor(phase);
        }
    }

    /// A noise burst starting at `start` s, `dur` s long, coloured by
    /// `filter`, decaying with time constant `tau` s.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn noise(
        &mut self,
        rng: &mut Pcg32,
        start: f64,
        dur: f64,
        filter: Filter,
        attack: f64,
        tau: f64,
        amp: f64,
    ) {
        let s0 = self.at(start);
        let n = self.at(dur);
        let mut f = FilterState::new(filter, self.sr);
        let fade = self.at(0.004).max(1);
        for i in 0..n {
            let x = rng.signed();
            let idx = s0 + i;
            if idx >= self.out.len() {
                break;
            }
            let t = i as f64 / self.sr;
            let att = if attack > 0.0 {
                (t / attack.max(MIN_ATTACK)).min(1.0)
            } else {
                1.0
            };
            let tail = ((n - i) as f64 / fade as f64).min(1.0);
            let env = amp * att * tail * libm::exp(-t / tau.max(1e-6));
            self.out[idx] += env * f.run(x);
        }
    }

    /// Scales to [`TARGET_PEAK`] and converts to `f32`.
    pub(crate) fn finish(self) -> Vec<f32> {
        normalize(self.out)
    }
}

/// Scales `buf` so its absolute peak is [`TARGET_PEAK`] (silence stays
/// silent) and converts to `f32`.
pub(crate) fn normalize(buf: Vec<f64>) -> Vec<f32> {
    let peak = buf.iter().fold(0.0f64, |m, &v| m.max(v.abs()));
    let g = if peak > 0.0 { TARGET_PEAK / peak } else { 0.0 };
    buf.into_iter().map(|v| (v * g) as f32).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_behave() {
        let sr = 48_000.0;
        // Low-pass passes DC, high-pass removes it.
        let mut lp = FilterState::new(Filter::Low(100.0), sr);
        let mut hp = FilterState::new(Filter::High(100.0), sr);
        let (mut l, mut h) = (0.0, 0.0);
        for _ in 0..48_000 {
            l = lp.run(1.0);
            h = hp.run(1.0);
        }
        assert!((l - 1.0).abs() < 1e-6);
        assert!(h.abs() < 1e-6);
        // Band-pass: unit gain at the centre, small far away.
        let gain = |f: f64| {
            let mut b = Biquad::band(1000.0, 2.0, sr);
            let mut peak = 0.0f64;
            for i in 0..48_000 {
                let y = b.run(libm::sin(TAU * f * i as f64 / sr));
                if i > 24_000 {
                    peak = peak.max(y.abs());
                }
            }
            peak
        };
        assert!((gain(1000.0) - 1.0).abs() < 0.01);
        assert!(gain(50.0) < 0.05);
    }

    #[test]
    fn midi_reference() {
        assert_eq!(midi_hz(69.0), 440.0);
        assert!((midi_hz(81.0) - 880.0).abs() < 1e-9);
    }

    #[test]
    fn normalize_silence_and_peak() {
        assert_eq!(normalize(vec![0.0; 4]), vec![0.0f32; 4]);
        let v = normalize(vec![0.5, -2.0, 1.0]);
        assert_eq!(v[1], -(TARGET_PEAK as f32));
    }
}
