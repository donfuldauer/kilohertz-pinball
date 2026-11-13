//! The nine table sound effects. Each is a fixed recipe of tones and noise
//! bursts with its own constant PCG seed, so the SFX set does not depend on
//! the table metadata.

use super::dsp::{Filter, Synth, TAU, normalize, one_pole};
use crate::rng::Pcg32;

/// Ball-roll loop length in seconds.
pub const BALL_ROLL_SECONDS: f64 = 2.0;

fn rng_for(id: &str) -> Pcg32 {
    Pcg32::new(
        crate::rng::fnv1a(crate::rng::FNV_OFFSET, id.as_bytes()),
        0x5f78,
    )
}

const THUD: [f64; 3] = [1.0, 0.35, 0.12];
const PURE: [f64; 1] = [1.0];

pub(super) fn flipper(sr: u32) -> Vec<f32> {
    let mut rng = rng_for("sfx_flipper");
    let mut s = Synth::new(sr, 0.14);
    // Coil thump, plunger clack, rubber slap.
    s.tone(0.0, 0.14, 190.0, 70.0, 0.02, 0.0005, 0.035, 0.9, &THUD);
    s.noise(
        &mut rng,
        0.0,
        0.03,
        Filter::Band(2600.0, 1.4),
        0.0,
        0.006,
        1.4,
    );
    s.noise(&mut rng, 0.004, 0.08, Filter::Low(900.0), 0.0, 0.018, 0.9);
    s.finish()
}

pub(super) fn bumper(sr: u32) -> Vec<f32> {
    let mut rng = rng_for("sfx_bumper");
    let mut s = Synth::new(sr, 0.28);
    s.tone(0.0, 0.25, 140.0, 85.0, 0.03, 0.0005, 0.07, 1.0, &THUD);
    s.noise(
        &mut rng,
        0.0,
        0.05,
        Filter::Band(1800.0, 1.0),
        0.0,
        0.01,
        1.3,
    );
    // Metallic skirt ring: two inharmonic partials.
    s.tone(0.002, 0.27, 1240.0, 1240.0, 1.0, 0.001, 0.08, 0.22, &PURE);
    s.tone(0.002, 0.27, 1863.0, 1863.0, 1.0, 0.001, 0.06, 0.15, &PURE);
    s.finish()
}

pub(super) fn sling(sr: u32) -> Vec<f32> {
    let mut rng = rng_for("sfx_sling");
    let mut s = Synth::new(sr, 0.16);
    s.tone(0.0, 0.15, 260.0, 120.0, 0.015, 0.0005, 0.04, 0.8, &THUD);
    s.noise(
        &mut rng,
        0.0,
        0.06,
        Filter::Band(1200.0, 0.8),
        0.0,
        0.012,
        1.4,
    );
    s.noise(&mut rng, 0.0, 0.015, Filter::High(3000.0), 0.0, 0.003, 0.8);
    s.finish()
}

pub(super) fn drop_target(sr: u32) -> Vec<f32> {
    let mut rng = rng_for("sfx_drop_target");
    let mut s = Synth::new(sr, 0.12);
    s.noise(
        &mut rng,
        0.0,
        0.02,
        Filter::Band(3200.0, 1.5),
        0.0,
        0.004,
        1.2,
    );
    s.tone(0.0, 0.11, 520.0, 480.0, 0.02, 0.0005, 0.025, 0.5, &THUD);
    s.tone(0.0, 0.11, 1130.0, 1130.0, 1.0, 0.0005, 0.02, 0.25, &PURE);
    // The target dropping into its slot.
    s.noise(&mut rng, 0.045, 0.06, Filter::Low(700.0), 0.0, 0.015, 0.8);
    s.finish()
}

pub(super) fn spinner(sr: u32) -> Vec<f32> {
    let mut rng = rng_for("sfx_spinner");
    let mut s = Synth::new(sr, 0.035);
    s.noise(
        &mut rng,
        0.0,
        0.035,
        Filter::Band(4200.0, 2.0),
        0.0,
        0.006,
        1.0,
    );
    s.tone(0.0, 0.035, 2480.0, 2480.0, 1.0, 0.0003, 0.008, 0.5, &PURE);
    s.finish()
}

pub(super) fn knocker(sr: u32) -> Vec<f32> {
    let mut rng = rng_for("sfx_knocker");
    let mut s = Synth::new(sr, 0.35);
    s.tone(0.0, 0.35, 110.0, 62.0, 0.02, 0.0005, 0.09, 1.0, &THUD);
    // Cabinet wood resonance.
    s.tone(0.0, 0.3, 218.0, 218.0, 1.0, 0.001, 0.06, 0.35, &[1.0, 0.2]);
    s.noise(
        &mut rng,
        0.0,
        0.03,
        Filter::Band(1500.0, 0.7),
        0.0,
        0.006,
        1.6,
    );
    s.finish()
}

pub(super) fn drain(sr: u32) -> Vec<f32> {
    let mut rng = rng_for("sfx_drain");
    let mut s = Synth::new(sr, 0.6);
    // Falling tone and the ball rolling into the trough.
    s.tone(
        0.0,
        0.6,
        420.0,
        90.0,
        0.18,
        0.01,
        0.35,
        0.6,
        &[1.0, 0.5, 0.25, 0.12],
    );
    s.noise(&mut rng, 0.0, 0.5, Filter::Low(350.0), 0.02, 0.2, 1.2);
    s.noise(
        &mut rng,
        0.42,
        0.08,
        Filter::Band(900.0, 1.0),
        0.0,
        0.02,
        0.7,
    );
    s.finish()
}

pub(super) fn launch(sr: u32) -> Vec<f32> {
    let mut rng = rng_for("sfx_launch");
    let mut s = Synth::new(sr, 0.4);
    // Plunger release thump, spring twang, whoosh up the shooter lane.
    s.tone(0.0, 0.2, 160.0, 70.0, 0.03, 0.0005, 0.05, 0.9, &THUD);
    s.tone(
        0.0,
        0.3,
        310.0,
        330.0,
        0.1,
        0.001,
        0.07,
        0.3,
        &[1.0, 0.6, 0.3],
    );
    s.noise(
        &mut rng,
        0.02,
        0.38,
        Filter::Band(1600.0, 0.9),
        0.05,
        0.15,
        0.9,
    );
    s.finish()
}

/// The seamless ball-roll loop: every component is periodic over the loop.
///
/// * Rumble: partials with a whole number of cycles per loop, phases from the
///   PCG, amplitude-modulated by a whole-cycle LFO, so sample `L` equals
///   sample `0` exactly.
/// * Texture: white noise low-passed circularly (the one-pole filter runs
///   over the buffer twice and keeps the second pass, so its state at the
///   wrap matches; the residual transient is below `f64` precision for any
///   rate ≥ 8 kHz).
pub(super) fn ball_roll(sr: u32) -> Vec<f32> {
    let mut rng = rng_for("sfx_ball_roll");
    let sr_f = f64::from(sr);
    let n = libm::round(BALL_ROLL_SECONDS * sr_f) as usize;
    let nn = n as u64;
    let mut out = vec![0.0f64; n];
    // Rumble partials: k cycles per loop, k / BALL_ROLL_SECONDS Hz (35–190 Hz).
    let partials: Vec<(u64, f64, f64)> = (0..10)
        .map(|j| {
            let k = 70 + u64::from(rng.below(310));
            let amp = 1.0 / (1.0 + j as f64 * 0.4);
            (k, amp, rng.unit())
        })
        .collect();
    for (i, o) in out.iter_mut().enumerate() {
        let i = i as u64;
        let mut v = 0.0;
        for &(k, amp, ph) in &partials {
            let frac = ((k * i) % nn) as f64 / n as f64;
            v += amp * libm::sin(TAU * (frac + ph));
        }
        // Wobble: 3 cycles per loop.
        let lfo = ((3 * i) % nn) as f64 / n as f64;
        *o = v * (1.0 + 0.3 * libm::sin(TAU * lfo));
    }
    // Circular low-passed noise.
    let noise: Vec<f64> = (0..n).map(|_| rng.signed()).collect();
    let a = one_pole(260.0, sr_f);
    let mut y = 0.0f64;
    let mut y2 = 0.0f64;
    for pass in 0..2 {
        for (i, &x) in noise.iter().enumerate() {
            y += a * (x - y);
            y2 += a * (y - y2);
            if pass == 1 {
                out[i] += 6.0 * y2;
            }
        }
    }
    normalize(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ball_roll_wraps_like_any_step() {
        for sr in [8_000, 44_100, 48_000, 96_000] {
            let v = ball_roll(sr);
            let seam = (v[v.len() - 1] - v[0]).abs();
            let max_step = v
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0f32, f32::max);
            assert!(seam <= max_step, "{sr}: seam {seam} > max step {max_step}");
        }
    }
}
