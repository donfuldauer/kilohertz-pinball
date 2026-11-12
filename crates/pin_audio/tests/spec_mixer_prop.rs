//! Spec property tests (Independent Tester, Subphase 3.3).
//!
//! Bounded `proptest` runs: `decode` never panics on hostile bytes
//! (criterion 5); decode values and resampled lengths; and the mixer's
//! invariants (plain sum, voice limiting, render-split invariance, ducking
//! bounds) over random trigger schedules.

#[path = "spec_fixtures/util.rs"]
mod util;

use pin_audio::mixer::{Bus, MixerConfig, decode};
use proptest::prelude::*;
use util::*;

fn bus(i: usize) -> Bus {
    match i {
        0 => Bus::Music,
        1 => Bus::Callout,
        _ => Bus::Sfx,
    }
}

/// Random split of `frames` into render-call sizes, driven by `seed`.
fn render_split(rig: &mut Rig, frames: usize, rng: &mut XorShift) -> Vec<f32> {
    let mut out = Vec::with_capacity(frames * 2);
    let mut left = frames;
    while left > 0 {
        let n = 1 + (rng.next_u64() as usize % 300).min(left - 1);
        out.extend_from_slice(&rig.render(n));
        left -= n;
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn decode_never_panics_on_random_bytes(bytes in prop::collection::vec(any::<u8>(), 0..600)) {
        let _ = decode(&bytes, RATE);
    }

    #[test]
    fn decode_never_panics_on_random_bytes_behind_a_valid_magic(
        kind in 0..3u8,
        tail in prop::collection::vec(any::<u8>(), 0..600),
        rate in prop::sample::select(vec![8_000u32, 22_050, 44_100, 48_000, 96_000]),
    ) {
        let mut bytes = match kind {
            0 => b"RIFF\x00\x01\x00\x00WAVE".to_vec(),
            1 => b"RIFF\x00\x01\x00\x00WAVEfmt \x10\x00\x00\x00".to_vec(),
            _ => b"OggS".to_vec(),
        };
        bytes.extend_from_slice(&tail);
        if let Ok(s) = decode(&bytes, rate) {
            prop_assert_eq!(s.data().len(), 2 * s.frames());
        }
    }

    #[test]
    fn decode_never_panics_on_a_random_wav_header(
        tag in prop_oneof![Just(1u16), Just(3u16), Just(0xFFFEu16), any::<u16>()],
        bits in prop_oneof![Just(8u16), Just(16u16), Just(24u16), Just(32u16), 0..80u16],
        channels in 0..5u16,
        rate in prop_oneof![Just(0u32), 1_000..200_000u32],
        data in prop::collection::vec(any::<u8>(), 0..200),
    ) {
        if let Ok(s) = decode(&wav_bytes(tag, bits, channels, rate, &data), RATE) {
            prop_assert_eq!(s.data().len(), 2 * s.frames());
            prop_assert!(channels == 1 || channels == 2);
            prop_assert!(s.data().iter().all(|x| !x.is_nan()) || tag == 3);
        }
    }

    #[test]
    fn decode_never_panics_on_damaged_valid_files(
        which in 0..3usize,
        cut in any::<prop::sample::Index>(),
        edits in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..6),
    ) {
        let wav = wav_i16(2, 44_100, &[1_000; 256]);
        let base: &[u8] = [wav.as_slice(), OGG_MONO_48K, OGG_LEFT_44K1][which];
        let mut bytes = base.to_vec();
        for (at, value) in edits {
            let i = at.index(bytes.len());
            // Keep the WAV sample-rate field intact so a damaged rate cannot
            // legitimately turn the fixture into minutes of audio.
            if which == 0 && (24..28).contains(&i) {
                continue;
            }
            bytes[i] = value;
        }
        bytes.truncate(cut.index(bytes.len() + 1));
        if let Ok(s) = decode(&bytes, RATE) {
            prop_assert_eq!(s.data().len(), 2 * s.frames());
        }
    }

    #[test]
    fn wav_i16_values_survive_decoding(
        stereo in any::<bool>(),
        samples in prop::collection::vec(any::<i16>(), 0..200),
        rate in prop::sample::select(vec![22_050u32, 44_100, 48_000]),
    ) {
        let channels = if stereo { 2 } else { 1 };
        let samples = &samples[..samples.len() - samples.len() % channels];
        prop_assume!(!samples.is_empty());
        let s = decoded(decode(&wav_i16(channels as u16, rate, samples), rate));
        prop_assert_eq!(s.frames(), samples.len() / channels);
        for (i, f) in s.data().chunks_exact(2).enumerate() {
            let (l, r) = if stereo {
                (samples[2 * i], samples[2 * i + 1])
            } else {
                (samples[i], samples[i])
            };
            prop_assert!((f[0] - f32::from(l) / 32_768.0).abs() <= 5e-5);
            prop_assert!((f[1] - f32::from(r) / 32_768.0).abs() <= 5e-5);
        }
    }

    #[test]
    fn wav_float_is_passed_through_bit_exact(
        samples in prop::collection::vec(-1.0f32..=1.0, 2..200),
    ) {
        let samples = &samples[..samples.len() & !1];
        let s = decoded(decode(&wav_f32(2, RATE, samples), RATE));
        prop_assert_eq!(s.data(), samples);
    }

    #[test]
    fn resampling_scales_length_bounds_values_and_keeps_dc(
        frames in 2..1_500usize,
        level in -32_768..=32_767i32,
        noise in prop::collection::vec(any::<i16>(), 64),
        src in prop::sample::select(vec![8_000u32, 11_025, 22_050, 32_000, 44_100, 48_000, 96_000]),
        dst in prop::sample::select(vec![22_050u32, 44_100, 48_000]),
    ) {
        let level = level as i16;
        let exact = frames as f64 * f64::from(dst) / f64::from(src);
        let lo = (exact.floor() as usize).saturating_sub(1);
        let hi = exact.ceil() as usize + 1;

        // DC stays DC.
        let s = decoded(decode(&wav_i16(1, src, &vec![level; frames]), dst));
        prop_assert!((lo..=hi).contains(&s.frames()), "{} frames, expected ≈ {}", s.frames(), exact);
        let v = f32::from(level) / 32_768.0;
        prop_assert!(s.data().iter().all(|x| (x - v).abs() <= 1e-4));

        // Linear interpolation never leaves the range of the source.
        let src_samples: Vec<i16> = (0..frames).map(|i| noise[i % 64]).collect();
        let (min, max) = src_samples.iter().fold((i16::MAX, i16::MIN), |(a, b), &x| (a.min(x), b.max(x)));
        let s = decoded(decode(&wav_i16(1, src, &src_samples), dst));
        prop_assert!((lo..=hi).contains(&s.frames()));
        let (min, max) = (f32::from(min) / 32_768.0 - 1e-4, f32::from(max) / 32_768.0 + 1e-4);
        prop_assert!(s.data().iter().all(|&x| x >= min && x <= max));
        // The first output frame is the first source frame.
        prop_assert!((s.data()[0] - f32::from(src_samples[0]) / 32_768.0).abs() <= 1e-4);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// Criterion 3: output = clip(master · Σ value·gain) for music/sfx voices
    /// with no callout playing; channels independent.
    #[test]
    fn mix_is_the_clipped_sum(
        voices in prop::collection::vec((-64..=64i32, -64..=64i32, 0..=32u32, any::<bool>()), 0..12),
        master in prop::sample::select(vec![0.0f32, 0.25, 0.5, 1.0, 1.5, 2.0]),
        frames in 1..400usize,
    ) {
        let mut rig = Rig::new(MixerConfig {
            master_gain: master,
            max_voices: [12, 4, 12],
            ..MixerConfig::default()
        });
        let (mut sum_l, mut sum_r) = (0.0f64, 0.0f64);
        for (i, (l, r, g, music)) in voices.iter().enumerate() {
            let (l, r, g) = (*l as f32 / 256.0, *r as f32 / 256.0, *g as f32 / 16.0);
            rig.reg(i as u16, &dc(l, r, 512, RATE), bus(if *music { 0 } else { 2 }));
            rig.play(i as u16, g);
            sum_l += f64::from(l) * f64::from(g);
            sum_r += f64::from(r) * f64::from(g);
        }
        let expect_l = (sum_l * f64::from(master)).clamp(-1.0, 1.0) as f32;
        let expect_r = (sum_r * f64::from(master)).clamp(-1.0, 1.0) as f32;
        let out = rig.render(frames);
        for f in out.chunks_exact(2) {
            prop_assert!((f[0] - expect_l).abs() <= 1e-5, "left {} vs {}", f[0], expect_l);
            prop_assert!((f[1] - expect_r).abs() <= 1e-5, "right {} vs {}", f[1], expect_r);
        }
        prop_assert_eq!(rig.mixer.duck_level(), 1.0);
    }

    /// Criterion 2: on a full bus the oldest voice is stolen; counts match.
    #[test]
    fn voice_limit_keeps_the_newest_voices(
        limit in 1..7usize,
        bus_index in 0..3usize,
        schedule in prop::collection::vec((0..8u16, 0..64usize), 0..40),
    ) {
        let mut max_voices = [1, 1, 1];
        max_voices[bus_index] = limit;
        let mut rig = Rig::new(MixerConfig { max_voices, ..MixerConfig::default() });
        // Sound k has DC value 2^k / 4096 so a sum identifies the playing set
        // when every sound plays at most once... sounds may repeat, so compare sums.
        for k in 0..8u16 {
            let v = f32::from(1u16 << k) / 4_096.0;
            rig.reg(k, &dc(v, -v, 4_096, RATE), bus(bus_index));
        }
        let mut played: Vec<u16> = Vec::new();
        for (id, gap) in schedule {
            rig.play(id, 1.0);
            played.push(id);
            let out = rig.render(gap + 1);
            let alive = &played[played.len().saturating_sub(limit)..];
            let expect: f32 = alive.iter().map(|&k| f32::from(1u16 << k) / 4_096.0).sum();
            // A callout bus is never ducked; music/sfx see no callout here.
            for f in out.chunks_exact(2) {
                prop_assert!((f[0] - expect).abs() <= 1e-5, "left {} vs {}", f[0], expect);
                prop_assert!((f[1] + expect).abs() <= 1e-5, "right {} vs {}", f[1], -expect);
            }
            prop_assert_eq!(rig.mixer.active_voices(bus(bus_index)), played.len().min(limit));
            prop_assert_eq!(
                rig.mixer.stolen_voices(),
                played.len().saturating_sub(limit) as u64
            );
        }
        prop_assert_eq!(rig.mixer.unknown_sounds(), 0);
    }

    /// Triggers land at render-call boundaries, so how the frames between two
    /// triggers are split into calls must not change the output (the duck
    /// ramp is per frame, not per buffer).
    #[test]
    fn output_does_not_depend_on_how_renders_are_split(
        schedule in prop::collection::vec((0..6u16, 0..=4u32, 1..1_500usize), 1..10),
        seed in 1..u64::MAX,
    ) {
        let build = || {
            let mut rig = Rig::new(MixerConfig {
                max_voices: [1, 2, 3],
                duck_attack_ms: 5.0,
                duck_release_ms: 20.0,
                ..MixerConfig::default()
            });
            rig.reg(0, &dc(0.25, -0.125, 3_000, RATE), bus(0));
            rig.reg(1, &dc(0.0625, 0.0625, 700, RATE), bus(1));
            rig.reg(2, &dc(-0.03125, 0.0, 150, RATE), bus(1));
            rig.reg(3, &dc(0.125, 0.25, 900, RATE), bus(2));
            rig.reg(4, &dc(-0.25, 0.125, 90, RATE), bus(2));
            rig // sound 5 stays unregistered
        };
        let (mut a, mut b) = (build(), build());
        let mut rng = XorShift(seed);
        for (id, gain, gap) in schedule {
            a.play(id, gain as f32 / 4.0);
            b.play(id, gain as f32 / 4.0);
            let whole = a.render(gap);
            let split = render_split(&mut b, gap, &mut rng);
            for (i, (x, y)) in whole.iter().zip(&split).enumerate() {
                prop_assert!((x - y).abs() <= 2e-5, "value {}: {} vs {}", i, x, y);
            }
            prop_assert!((a.mixer.duck_level() - b.mixer.duck_level()).abs() <= 2e-5);
            for i in 0..3 {
                prop_assert_eq!(a.mixer.active_voices(bus(i)), b.mixer.active_voices(bus(i)));
            }
            prop_assert_eq!(a.mixer.stolen_voices(), b.mixer.stolen_voices());
            prop_assert_eq!(a.mixer.unknown_sounds(), b.mixer.unknown_sounds());
        }
    }

    /// Ducking invariants over random schedules: the level stays inside
    /// [duck_gain, 1], moves by at most one ramp step per frame, only falls
    /// while a callout is active and only rises while none is; the output is
    /// finite and inside [−1, 1].
    #[test]
    fn duck_level_and_output_stay_in_bounds(
        schedule in prop::collection::vec((0..4u16, 0..=8u32, 1..600usize), 1..14),
        duck_gain in prop::sample::select(vec![0.0f32, 0.2, 0.35, 0.8, 1.0]),
    ) {
        let mut rig = Rig::new(MixerConfig {
            duck_gain,
            duck_attack_ms: 10.0,  // 480 frames
            duck_release_ms: 40.0, // 1920 frames
            ..MixerConfig::default()
        });
        rig.reg(0, &dc(0.5, -0.5, 2_000, RATE), bus(0));
        rig.reg(1, &dc(0.25, 0.25, 300, RATE), bus(1));
        rig.reg(2, &dc(0.75, -0.75, 1_000, RATE), bus(2));
        rig.reg(3, &dc(0.0, 0.0, 40, RATE), bus(1));
        let attack_step = (1.0 - duck_gain) / 480.0;
        let release_step = (1.0 - duck_gain) / 1_920.0;
        for (id, gain, gap) in schedule {
            rig.play(id, gain as f32 / 4.0);
            let mut left = gap;
            while left > 0 {
                let before = rig.mixer.duck_level();
                let out = rig.render(1);
                let after = rig.mixer.duck_level();
                prop_assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
                prop_assert!(after >= duck_gain - 1e-5 && after <= 1.0, "level {}", after);
                let delta = after - before;
                prop_assert!(delta <= release_step + 1e-6, "rose by {}", delta);
                prop_assert!(delta >= -attack_step - 1e-6, "fell by {}", delta);
                left -= 1;
            }
        }
        // With nothing new, everything ends and the level returns to exactly 1.
        let _ = rig.render_chunked(2_000 + 1_920 + 2, 256);
        prop_assert_eq!(rig.mixer.duck_level(), 1.0);
        prop_assert!(rig.render(16).iter().all(|&x| x == 0.0));
    }
}
