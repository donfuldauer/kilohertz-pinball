//! Synthesised placeholder sounds (4.6 deliverables 1 and 2).
//!
//! * [`SFX_IDS`]: nine table sound effects ([`render_sfx`]); fixed recipes,
//!   independent of the table metadata.
//! * [`MOTIF_IDS`]: ten short tone motifs standing in for spoken callouts
//!   ([`render_motif`]); fixed, independent of the table metadata.
//! * [`MUSIC_ID`]: one music loop per table ([`render_music`]), seeded by the
//!   table's title, palette and seed.
//!
//! Every sound is mono, peak-normalised to [`TARGET_PEAK`] (≈ −1 dBFS), and
//! written by [`encode_wav`] as 16-bit PCM WAV at [`KIT_SAMPLE_RATE`].
//!
//! The ball-roll loop (`sfx_ball_roll`, [`BALL_ROLL_SECONDS`] long) is
//! periodic by construction: the step from its last sample back to its first
//! is no larger than its largest step between neighbouring samples and at
//! most [`BALL_ROLL_SEAM_BOUND`] at 48 kHz.

mod dsp;
mod music;
mod sfx;

pub use dsp::TARGET_PEAK;
pub use music::{MOTIF_IDS, MOTIF_STEP, MUSIC_BARS};
pub use sfx::BALL_ROLL_SECONDS;

use crate::TableMeta;

/// Sample rate of the kit's WAV files.
pub const KIT_SAMPLE_RATE: u32 = 48_000;

/// Lowest sample rate the renderers accept (pin_audio's minimum source rate).
pub const MIN_RATE: u32 = 8_000;
/// Highest sample rate the renderers accept (pin_audio's maximum source rate).
pub const MAX_RATE: u32 = 384_000;

/// Largest absolute step from the last to the first sample of the ball-roll
/// loop at 48 kHz (full scale = 1.0).
pub const BALL_ROLL_SEAM_BOUND: f32 = 0.02;

/// Stable ids of the synthesised sound effects.
pub const SFX_IDS: &[&str] = &[
    "sfx_flipper",
    "sfx_bumper",
    "sfx_sling",
    "sfx_drop_target",
    "sfx_spinner",
    "sfx_ball_roll",
    "sfx_knocker",
    "sfx_drain",
    "sfx_launch",
];

/// Stable id of the table's music loop.
pub const MUSIC_ID: &str = "music_main";

fn rate_ok(sample_rate: u32) -> bool {
    (MIN_RATE..=MAX_RATE).contains(&sample_rate)
}

/// Renders sound effect `id` (one of [`SFX_IDS`]) as mono samples at
/// `sample_rate`. `None` for an unknown id or a rate outside
/// [`MIN_RATE`]`..=`[`MAX_RATE`].
pub fn render_sfx(id: &str, sample_rate: u32) -> Option<Vec<f32>> {
    if !rate_ok(sample_rate) {
        return None;
    }
    let f: fn(u32) -> Vec<f32> = match id {
        "sfx_flipper" => sfx::flipper,
        "sfx_bumper" => sfx::bumper,
        "sfx_sling" => sfx::sling,
        "sfx_drop_target" => sfx::drop_target,
        "sfx_spinner" => sfx::spinner,
        "sfx_ball_roll" => sfx::ball_roll,
        "sfx_knocker" => sfx::knocker,
        "sfx_drain" => sfx::drain,
        "sfx_launch" => sfx::launch,
        _ => return None,
    };
    Some(f(sample_rate))
}

/// Renders callout motif `id` (one of [`MOTIF_IDS`]) as mono samples at
/// `sample_rate`; `None` as for [`render_sfx`].
pub fn render_motif(id: &str, sample_rate: u32) -> Option<Vec<f32>> {
    if !rate_ok(sample_rate) {
        return None;
    }
    music::render_motif(id, sample_rate)
}

/// Renders the table's music loop ([`MUSIC_BARS`] bars) as mono samples at
/// `sample_rate`; `None` for a rate outside [`MIN_RATE`]`..=`[`MAX_RATE`].
pub fn render_music(meta: &TableMeta, sample_rate: u32) -> Option<Vec<f32>> {
    if !rate_ok(sample_rate) {
        return None;
    }
    Some(music::render(meta, sample_rate))
}

/// Encodes mono samples as a 16-bit PCM WAV (values clamped to ±1, rounded
/// half away from zero). `None` for a sample rate of 0.
pub fn encode_wav(samples: &[f32], sample_rate: u32) -> Option<Vec<u8>> {
    if sample_rate == 0 {
        return None;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut bytes = Vec::with_capacity(44 + samples.len() * 2);
    {
        let mut w = hound::WavWriter::new(std::io::Cursor::new(&mut bytes), spec).ok()?;
        let mut w16 = w.get_i16_writer(samples.len() as u32);
        for &s in samples {
            let v = f64::from(s).clamp(-1.0, 1.0) * 32767.0;
            w16.write_sample(libm::round(v) as i16);
        }
        w16.flush().ok()?;
        w.finalize().ok()?;
    }
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms_peak(v: &[f32]) -> (f64, f64) {
        let sum: f64 = v.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
        let peak = v.iter().fold(0.0f64, |m, &x| m.max(f64::from(x).abs()));
        ((sum / v.len() as f64).sqrt(), peak)
    }

    fn db(x: f64) -> f64 {
        20.0 * x.log10()
    }

    #[test]
    fn every_sound_is_loud_enough_and_unclipped() {
        let meta = TableMeta {
            title: "Apex Circuit".into(),
            palette: vec![[11, 29, 58]],
            seed: 9,
        };
        let mut all: Vec<(String, Vec<f32>)> = Vec::new();
        for id in SFX_IDS {
            all.push((id.to_string(), render_sfx(id, KIT_SAMPLE_RATE).unwrap()));
        }
        for id in MOTIF_IDS {
            all.push((id.to_string(), render_motif(id, KIT_SAMPLE_RATE).unwrap()));
        }
        all.push((MUSIC_ID.into(), render_music(&meta, KIT_SAMPLE_RATE).unwrap()));
        for (id, v) in &all {
            let (rms, peak) = rms_peak(v);
            assert!(db(rms) > -30.0, "{id}: rms {:.1} dBFS", db(rms));
            assert!(db(peak) <= -0.9, "{id}: peak {:.2} dBFS", db(peak));
            assert!(v.iter().all(|x| x.is_finite()));
        }
        // All distinct.
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(all[i].1, all[j].1, "{} == {}", all[i].0, all[j].0);
            }
        }
    }

    #[test]
    fn rejects_unknown_ids_and_rates() {
        assert!(render_sfx("sfx_nope", 48_000).is_none());
        assert!(render_sfx("sfx_flipper", 0).is_none());
        assert!(render_sfx("sfx_flipper", 7_999).is_none());
        assert!(render_sfx("sfx_flipper", 384_001).is_none());
        assert!(render_motif("callout_nope", 48_000).is_none());
        assert!(render_motif("callout_tilt", 1).is_none());
        let meta = TableMeta {
            title: String::new(),
            palette: vec![],
            seed: 0,
        };
        assert!(render_music(&meta, u32::MAX).is_none());
        assert!(render_sfx("sfx_spinner", MIN_RATE).is_some());
    }

    #[test]
    fn ball_roll_seam_within_bound() {
        let v = render_sfx("sfx_ball_roll", KIT_SAMPLE_RATE).unwrap();
        assert_eq!(v.len(), (BALL_ROLL_SECONDS * f64::from(KIT_SAMPLE_RATE)) as usize);
        let seam = (v[v.len() - 1] - v[0]).abs();
        assert!(seam <= BALL_ROLL_SEAM_BOUND, "seam {seam}");
    }

    #[test]
    fn wav_round_trip() {
        let samples = [0.0f32, 0.5, -0.5, 1.5, -1.5];
        let bytes = encode_wav(&samples, 48_000).unwrap();
        let mut r = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(r.spec().sample_rate, 48_000);
        let got: Vec<i16> = r.samples::<i16>().map(Result::unwrap).collect();
        assert_eq!(got, [0, 16384, -16384, 32767, -32767]);
        assert!(encode_wav(&samples, 0).is_none());
    }
}
