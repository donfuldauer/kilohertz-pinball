//! Spec tests (Independent Tester, Subphase 3.3): `pin_audio::mixer::decode`.
//!
//! Verification criteria 4 and 5: WAV fixtures generated in-test (8/16/24/32-bit
//! int, float, mono/stereo, 22.05/44.1/48 kHz), the committed Ogg Vorbis
//! fixtures, error classification, and "never panics" on hostile input.

#[path = "spec_fixtures/util.rs"]
mod util;

use pin_audio::mixer::{DecodeError, Sample, decode};
use util::*;

/// Tolerance for an integer format of `bits`: 1.5 LSB, which accepts both
/// the `/2^(bits-1)` and the `/(2^(bits-1) - 1)` scaling conventions but not
/// a wrong byte order, sign or width.
fn int_tol(bits: u32) -> f32 {
    1.5 / (1u64 << (bits - 1)) as f32 + 1e-6
}

#[track_caller]
fn assert_samples(s: &Sample, expected: &[f32], tol: f32, what: &str) {
    assert_eq!(
        s.data().len(),
        2 * s.frames(),
        "{what}: data len = 2·frames"
    );
    assert_eq!(s.data().len(), expected.len(), "{what}: length");
    for (i, (a, b)) in s.data().iter().zip(expected).enumerate() {
        assert!(
            (a - b).abs() <= tol,
            "{what}: sample {i} is {a}, expected {b} (tol {tol})"
        );
    }
}

/// Mono → both channels.
fn dup(mono: &[f32]) -> Vec<f32> {
    mono.iter().flat_map(|&x| [x, x]).collect()
}

fn err_of(r: Result<Sample, DecodeError>) -> DecodeError {
    match r {
        Ok(s) => panic!("expected an error, decoded {} frames", s.frames()),
        Err(e) => e,
    }
}

// ---------------------------------------------------------------- WAV, same rate

#[test]
fn wav_float_stereo_is_exact() {
    let src = [0.0, 1.0, -1.0, 0.5, 0.25, -0.125, 1e-3, -0.999];
    for rate in [22_050, 44_100, 48_000] {
        let s = decoded(decode(&wav_f32(2, rate, &src), rate));
        assert_eq!(s.frames(), 4);
        assert_eq!(s.data(), &src, "float stereo at {rate} Hz is bit-exact");
    }
}

#[test]
fn wav_float_mono_goes_to_both_channels() {
    let src = [0.5, -0.25, 0.125, 0.0, 1.0];
    let s = decoded(decode(&wav_f32(1, RATE, &src), RATE));
    assert_eq!(s.frames(), 5);
    assert_eq!(s.data(), dup(&src).as_slice());
}

#[test]
fn wav_u8_mono_and_stereo() {
    let raw = [128u8, 255, 0, 192, 64, 129, 127, 1];
    let exp: Vec<f32> = raw
        .iter()
        .map(|&b| (f32::from(b) - 128.0) / 128.0)
        .collect();
    let s = decoded(decode(&wav_u8(2, RATE, &raw), RATE));
    assert_eq!(s.frames(), 4);
    assert_samples(&s, &exp, int_tol(8), "u8 stereo");
    let s = decoded(decode(&wav_u8(1, RATE, &raw), RATE));
    assert_eq!(s.frames(), 8);
    assert_samples(&s, &dup(&exp), int_tol(8), "u8 mono");
}

#[test]
fn wav_i16_mono_and_stereo() {
    let raw = [0i16, 16_384, -16_384, 32_767, -32_768, 1, -1, 8_192];
    let exp: Vec<f32> = raw.iter().map(|&v| f32::from(v) / 32_768.0).collect();
    for rate in [22_050, 44_100, 48_000] {
        let s = decoded(decode(&wav_i16(2, rate, &raw), rate));
        assert_eq!(s.frames(), 4);
        assert_samples(&s, &exp, int_tol(16), "i16 stereo");
        let s = decoded(decode(&wav_i16(1, rate, &raw), rate));
        assert_eq!(s.frames(), 8);
        assert_samples(&s, &dup(&exp), int_tol(16), "i16 mono");
    }
}

#[test]
fn wav_i24_mono_and_stereo() {
    let raw = [
        0i32,
        4_194_304,
        -4_194_304,
        8_388_607,
        -8_388_608,
        1,
        -1,
        0x0012_3456,
    ];
    let exp: Vec<f32> = raw.iter().map(|&v| v as f32 / 8_388_608.0).collect();
    let s = decoded(decode(&wav_i24(2, RATE, &raw), RATE));
    assert_eq!(s.frames(), 4);
    assert_samples(&s, &exp, int_tol(24), "i24 stereo");
    let s = decoded(decode(&wav_i24(1, RATE, &raw), RATE));
    assert_eq!(s.frames(), 8);
    assert_samples(&s, &dup(&exp), int_tol(24), "i24 mono");
}

#[test]
fn wav_i32_mono_and_stereo() {
    let raw = [
        0i32,
        1 << 30,
        -(1 << 30),
        i32::MAX,
        i32::MIN,
        1 << 29,
        -(1 << 29),
        0x1234_5678,
    ];
    let exp: Vec<f32> = raw
        .iter()
        .map(|&v| (f64::from(v) / 2_147_483_648.0) as f32)
        .collect();
    let s = decoded(decode(&wav_i32(2, RATE, &raw), RATE));
    assert_eq!(s.frames(), 4);
    assert_samples(&s, &exp, 1e-6, "i32 stereo");
    let s = decoded(decode(&wav_i32(1, RATE, &raw), RATE));
    assert_eq!(s.frames(), 8);
    assert_samples(&s, &dup(&exp), 1e-6, "i32 mono");
}

#[test]
fn wav_with_zero_frames_decodes_to_an_empty_sample_or_errors_cleanly() {
    // Not specified by the brief beyond "never panics"; if it decodes, it is empty.
    if let Ok(s) = decode(&wav_i16(2, RATE, &[]), RATE) {
        assert_eq!(s.frames(), 0);
        assert!(s.data().is_empty());
    }
}

#[test]
fn wav_with_extra_chunks_before_data_is_decoded() {
    // A LIST chunk between `fmt ` and `data` is common in real files.
    let plain = wav_i16(1, RATE, &[1000, -2000, 3000]);
    let mut v = plain[..36].to_vec();
    v.extend_from_slice(b"LIST");
    v.extend_from_slice(&4u32.to_le_bytes());
    v.extend_from_slice(b"INFO");
    v.extend_from_slice(&plain[36..]);
    let riff_len = (v.len() - 8) as u32;
    v[4..8].copy_from_slice(&riff_len.to_le_bytes());
    let s = decoded(decode(&v, RATE));
    let exp: Vec<f32> = [1000i16, -2000, 3000]
        .iter()
        .map(|&x| f32::from(x) / 32_768.0)
        .collect();
    assert_samples(&s, &dup(&exp), int_tol(16), "LIST chunk skipped");
}

// ---------------------------------------------------------------- WAV, resampled

#[track_caller]
fn assert_len_about(frames: usize, expected: usize, what: &str) {
    assert!(
        frames.abs_diff(expected) <= 1,
        "{what}: {frames} frames, expected {expected} ± 1"
    );
}

#[test]
fn resample_2x_up_is_linear_interpolation() {
    // 22.05 kHz file into a 44.1 kHz mixer: output frame i sits at source
    // position i/2, so even frames are source frames and odd frames are the
    // midpoint of their neighbours (the definition of linear resampling).
    let src = [0.0f32, 0.5, -0.5, 1.0, 0.25, 0.25, -1.0, 0.0];
    let s = decoded(decode(&wav_f32(1, 22_050, &src), 44_100));
    assert_len_about(s.frames(), 16, "2x up");
    let d = s.data();
    for k in 0..src.len() {
        assert_close(d[2 * (2 * k)], src[k], 1e-6, "even frame, left");
        assert_close(d[2 * (2 * k) + 1], src[k], 1e-6, "even frame, right");
        if k + 1 < src.len() {
            let mid = 0.5 * (src[k] + src[k + 1]);
            assert_close(d[2 * (2 * k + 1)], mid, 1e-6, "odd frame, left");
            assert_close(d[2 * (2 * k + 1) + 1], mid, 1e-6, "odd frame, right");
        }
    }
}

#[test]
fn resample_2x_down_picks_every_other_frame() {
    // 48 kHz file into a 24 kHz mixer: output frame i sits exactly on source
    // frame 2i.
    let src: Vec<f32> = (0..64)
        .map(|i| ((i * 37) % 41) as f32 / 41.0 - 0.5)
        .collect();
    let s = decoded(decode(&wav_f32(1, 48_000, &src), 24_000));
    assert_len_about(s.frames(), 32, "2x down");
    let d = s.data();
    for i in 0..s.frames().min(32) {
        assert_close(d[2 * i], src[2 * i], 1e-6, "left");
        assert_close(d[2 * i + 1], src[2 * i], 1e-6, "right");
    }
}

#[test]
fn resample_keeps_dc_and_scales_length() {
    for (file_rate, mixer_rate) in [
        (22_050u32, 48_000u32),
        (44_100, 48_000),
        (48_000, 44_100),
        (22_050, 44_100),
        (44_100, 22_050),
    ] {
        let frames = file_rate as usize / 10; // 0.1 s
        let src: Vec<i16> = (0..frames).flat_map(|_| [8_192i16, -16_384]).collect();
        let s = decoded(decode(&wav_i16(2, file_rate, &src), mixer_rate));
        let what = format!("{file_rate} -> {mixer_rate}");
        assert_len_about(s.frames(), mixer_rate as usize / 10, &what);
        assert_eq!(s.data().len(), 2 * s.frames());
        assert_all_frames(s.data(), 0.25, -0.5, 1e-4, &what);
    }
}

#[test]
fn resample_of_a_ramp_stays_on_the_ramp_with_independent_channels() {
    // Left: a straight line in time; right: a different straight line. Any
    // linear interpolation at source position p = i·file_rate/mixer_rate
    // reproduces the line exactly, whatever the fractional phase.
    for (file_rate, mixer_rate) in [(44_100u32, 48_000u32), (22_050, 48_000), (48_000, 44_100)] {
        let n = file_rate as usize / 10;
        let line_l = |p: f64| (p / n as f64) as f32 * 0.8 - 0.4;
        let line_r = |p: f64| 0.3 - (p / n as f64) as f32 * 0.6;
        let src: Vec<f32> = (0..n)
            .flat_map(|i| [line_l(i as f64), line_r(i as f64)])
            .collect();
        let s = decoded(decode(&wav_f32(2, file_rate, &src), mixer_rate));
        let what = format!("{file_rate} -> {mixer_rate}");
        assert_len_about(s.frames(), mixer_rate as usize / 10, &what);
        let ratio = f64::from(file_rate) / f64::from(mixer_rate);
        for (i, f) in s.data().chunks_exact(2).enumerate() {
            let p = i as f64 * ratio;
            if p > (n - 1) as f64 {
                break; // past the last source frame: edge handling is the implementer's
            }
            assert_close(f[0], line_l(p), 5e-4, &format!("{what} left frame {i}"));
            assert_close(f[1], line_r(p), 5e-4, &format!("{what} right frame {i}"));
        }
    }
}

// ---------------------------------------------------------------- Ogg Vorbis

fn stats(s: &Sample, n: usize, freq: f64, rate: f64) -> (f64, f64, f64, usize) {
    let d = s.data();
    let (mut sxy, mut sxx, mut syy, mut peak) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut crossings = 0;
    for i in 0..n {
        let x = f64::from(d[2 * i]);
        let y = 0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / rate).sin();
        sxy += x * y;
        sxx += x * x;
        syy += y * y;
        peak = peak.max(x.abs());
        if i > 0 && (d[2 * (i - 1)] < 0.0) != (d[2 * i] < 0.0) {
            crossings += 1;
        }
    }
    (
        sxy / (sxx * syy).sqrt(),
        (sxx / n as f64).sqrt(),
        peak,
        crossings,
    )
}

#[test]
fn ogg_mono_fixture_decodes_to_a_plausible_sine() {
    let s = decoded(decode(OGG_MONO_48K, 48_000));
    assert_eq!(s.data().len(), 2 * s.frames());
    // Lenient length here (the strict one is its own test below).
    assert!(
        (12_000..=12_000 + 4_096).contains(&s.frames()),
        "0.25 s at 48 kHz, got {} frames",
        s.frames()
    );
    for f in s.data().chunks_exact(2) {
        assert_eq!(f[0], f[1], "mono → both channels");
        assert!(f[0].is_finite() && f[0].abs() <= 1.0);
    }
    let (corr, rms, peak, crossings) = stats(&s, 12_000, 440.0, 48_000.0);
    assert!(corr > 0.99, "correlation with a 440 Hz sine: {corr}");
    assert!((0.33..0.38).contains(&rms), "rms {rms} (expected 0.5/√2)");
    assert!((0.45..0.56).contains(&peak), "peak {peak} (expected 0.5)");
    assert!(
        (216..=224).contains(&crossings),
        "{crossings} zero crossings (110 cycles expected)"
    );
}

#[test]
fn ogg_stereo_fixture_keeps_channels_apart() {
    let s = decoded(decode(OGG_LEFT_44K1, 44_100));
    assert!(
        (8_820..=8_820 + 4_096).contains(&s.frames()),
        "0.2 s at 44.1 kHz, got {} frames",
        s.frames()
    );
    let (corr, rms, peak, _) = stats(&s, 8_820, 880.0, 44_100.0);
    assert!(corr > 0.99, "left correlates with an 880 Hz sine: {corr}");
    assert!((0.33..0.38).contains(&rms), "left rms {rms}");
    assert!((0.45..0.58).contains(&peak), "left peak {peak}");
    let right_peak = s
        .data()
        .chunks_exact(2)
        .map(|f| f[1].abs())
        .fold(0.0f32, f32::max);
    assert!(
        right_peak < 0.01,
        "right channel is silent, peak {right_peak}"
    );
}

#[test]
fn ogg_length_is_the_streams_final_granule_position() {
    // The stream's own length (final granule position) is 12 000 / 8 820
    // frames; the last Vorbis packet decodes past it and the excess (which is
    // not silence in these files: peak ≈ 0.3) must be trimmed.
    let s = decoded(decode(OGG_MONO_48K, 48_000));
    assert_eq!(s.frames(), 12_000, "mono fixture: 0.25 s at 48 kHz");
    let s = decoded(decode(OGG_LEFT_44K1, 44_100));
    assert_eq!(s.frames(), 8_820, "stereo fixture: 0.2 s at 44.1 kHz");
}

#[test]
fn ogg_is_resampled_to_the_mixer_rate() {
    let s = decoded(decode(OGG_LEFT_44K1, 48_000));
    assert!(
        (9_599..=9_601 + 4_460).contains(&s.frames()),
        "0.2 s at 48 kHz, got {} frames",
        s.frames()
    );
    // Still an 880 Hz tone on the left after resampling.
    let (corr, rms, _, crossings) = stats(&s, 9_599, 880.0, 48_000.0);
    assert!(corr > 0.98, "correlation {corr}");
    assert!((0.32..0.38).contains(&rms), "rms {rms}");
    assert!((348..=356).contains(&crossings), "{crossings} crossings");
}

// ---------------------------------------------------------------- errors

#[test]
fn unknown_magic_is_unsupported() {
    let inputs: [&[u8]; 8] = [
        b"",
        b"R",
        b"abc",
        b"ID3\x03\x00\x00\x00\x00\x00\x00 not audio at all",
        b"fLaC\x00\x00\x00\x22",
        b"FORM\x00\x00\x00\x10AIFFCOMM",
        b"RIFF\x24\x00\x00\x00AVI LIST", // RIFF but not WAVE
        &[0u8; 64],
    ];
    for bytes in inputs {
        let e = err_of(decode(bytes, RATE));
        assert!(
            matches!(e, DecodeError::Unsupported),
            "{bytes:?} should be Unsupported, got: {e}"
        );
    }
}

#[test]
fn more_than_two_channels_is_rejected() {
    for channels in [3u16, 4, 6, 8] {
        let src = vec![0i16; usize::from(channels) * 10];
        let e = err_of(decode(&wav_i16(channels, RATE, &src), RATE));
        assert!(
            matches!(e, DecodeError::TooManyChannels),
            "{channels} channels: got {e}"
        );
    }
}

#[test]
fn garbage_after_a_valid_magic_is_malformed() {
    let mut riff = b"RIFF\xff\xff\x00\x00WAVE".to_vec();
    riff.extend_from_slice(&XorShift(7).bytes(200));
    let e = err_of(decode(&riff, RATE));
    assert!(matches!(e, DecodeError::Malformed(_)), "RIFF garbage: {e}");

    let mut ogg = b"OggS".to_vec();
    ogg.extend_from_slice(&XorShift(9).bytes(200));
    let e = err_of(decode(&ogg, RATE));
    assert!(matches!(e, DecodeError::Malformed(_)), "OggS garbage: {e}");

    // Header cut inside the fmt chunk.
    let good = wav_i16(2, RATE, &[1, 2, 3, 4]);
    for cut in 12..36 {
        let e = err_of(decode(&good[..cut], RATE));
        assert!(
            matches!(e, DecodeError::Malformed(_)),
            "WAV cut at {cut}: {e}"
        );
    }
    // Ogg cut inside the first (identification) page.
    let e = err_of(decode(&OGG_MONO_48K[..40], RATE));
    assert!(matches!(e, DecodeError::Malformed(_)), "Ogg cut at 40: {e}");
}

#[test]
fn unsupported_wav_encodings_are_errors_not_panics() {
    // 64-bit float, 12-bit and 0-bit PCM, A-law, zero channels, zero rate.
    let data = [0u8; 64];
    let cases = [
        wav_bytes(TAG_FLOAT, 64, 1, RATE, &data),
        wav_bytes(TAG_PCM, 12, 1, RATE, &data),
        wav_bytes(TAG_PCM, 0, 1, RATE, &data),
        wav_bytes(6, 8, 1, RATE, &data),
        wav_bytes(TAG_PCM, 16, 0, RATE, &data),
        wav_bytes(TAG_PCM, 16, 2, 0, &data),
    ];
    for (i, bytes) in cases.iter().enumerate() {
        assert!(decode(bytes, RATE).is_err(), "case {i} must be an error");
    }
}

#[test]
fn errors_have_a_display_message() {
    let errs = [
        err_of(decode(b"nope", RATE)),
        err_of(decode(&wav_i16(3, RATE, &[0; 30]), RATE)),
        err_of(decode(b"OggS but not really an ogg stream", RATE)),
        DecodeError::Unsupported,
        DecodeError::Malformed("x".to_string()),
        DecodeError::TooManyChannels,
        DecodeError::TooLong,
    ];
    for e in &errs {
        assert!(!e.to_string().is_empty());
    }
    assert!(
        DecodeError::Malformed("zzyzx".to_string())
            .to_string()
            .contains("zzyzx")
    );
}

#[test]
fn longer_than_ten_minutes_is_too_long() {
    // 601 s of 8-bit mono at 8 kHz, decoded for an 8 kHz mixer.
    let over = vec![128u8; 8_000 * 601];
    let e = err_of(decode(&wav_u8(1, 8_000, &over), 8_000));
    assert!(matches!(e, DecodeError::TooLong), "601 s: got {e}");
    drop(over);

    // 599 s is fine.
    let under = vec![128u8; 8_000 * 599];
    let s = decoded(decode(&wav_u8(1, 8_000, &under), 8_000));
    assert_eq!(s.frames(), 8_000 * 599);
}

#[test]
fn the_ten_minute_cap_applies_to_the_decoded_length() {
    // 301 s at 8 kHz is under ten minutes at any mixer rate: accepted.
    // (Checks the cap is on duration, not on a frame count at one rate.)
    let bytes = wav_u8(1, 8_000, &vec![128u8; 8_000 * 301]);
    let s = decoded(decode(&bytes, 16_000));
    assert!(s.frames().abs_diff(16_000 * 301) <= 1);
}

// ---------------------------------------------------------------- never panics

#[test]
fn truncated_valid_wavs_never_panic() {
    let files = [
        wav_u8(1, 22_050, &XorShift(1).bytes(301)),
        wav_i16(2, 44_100, &[1234; 300]),
        wav_i24(2, 48_000, &[-5_000_000; 150]),
        wav_i32(1, 22_050, &[i32::MAX; 99]),
        wav_f32(2, 44_100, &[0.5; 200]),
    ];
    for bytes in &files {
        for cut in 0..bytes.len() {
            if let Ok(s) = decode(&bytes[..cut], RATE) {
                assert_eq!(s.data().len(), 2 * s.frames());
                assert!(s.frames() <= 2 * 48_000);
            }
        }
    }
}

#[test]
fn truncated_valid_oggs_never_panic() {
    for bytes in [OGG_MONO_48K, OGG_LEFT_44K1] {
        let cuts = (0..256).chain((256..bytes.len()).step_by(7));
        for cut in cuts {
            if let Ok(s) = decode(&bytes[..cut], RATE) {
                assert_eq!(s.data().len(), 2 * s.frames());
                assert!(s.frames() <= 48_000);
            }
        }
    }
}

#[test]
fn lying_headers_never_panic_or_over_allocate() {
    // data chunk claims 4 GiB / RIFF claims 4 GiB; fmt chunk claims a huge size;
    // absurd sample rates and block sizes.
    let mut a = wav_i16(2, RATE, &[1, 2, 3, 4]);
    a[40..44].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut b = a.clone();
    b[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    let mut c = wav_i16(2, RATE, &[1, 2, 3, 4]);
    c[16..20].copy_from_slice(&0xffff_fff0u32.to_le_bytes());
    let d = wav_i16(2, u32::MAX, &[1, 2, 3, 4]);
    let e = wav_i16(2, 1, &[1; 64]); // 1 Hz file: 32 s of audio from 64 frames
    let f = wav_bytes(TAG_PCM, 16, 2, RATE, &[0; 7]); // not a whole frame
    for (i, bytes) in [a, b, c, d, e, f].iter().enumerate() {
        if let Ok(s) = decode(bytes, RATE) {
            assert_eq!(s.data().len(), 2 * s.frames(), "case {i}");
            assert!(s.frames() <= 600 * 48_000, "case {i}: ten-minute cap");
        }
    }
}

#[test]
fn degenerate_mixer_rates_never_panic() {
    let ok = wav_i16(2, RATE, &[1, 2, 3, 4]);
    for mixer_rate in [0u32, 1] {
        let _ = decode(&ok, mixer_rate);
        let _ = decode(OGG_MONO_48K, mixer_rate);
    }
}

#[test]
fn seeded_random_bytes_never_panic() {
    let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
    for round in 0..3_000 {
        let len = (rng.next_u64() % 300) as usize;
        let mut bytes = rng.bytes(len);
        // Two thirds of the inputs get a valid magic so the parsers are reached.
        match round % 3 {
            0 if len >= 12 => {
                bytes[..4].copy_from_slice(b"RIFF");
                bytes[8..12].copy_from_slice(b"WAVE");
            }
            1 if len >= 4 => bytes[..4].copy_from_slice(b"OggS"),
            _ => {}
        }
        let _ = decode(&bytes, RATE);
    }
}

#[test]
fn bit_flipped_valid_files_never_panic() {
    let mut rng = XorShift(42);
    let wav = wav_i16(2, 44_100, &[321; 400]);
    for base in [wav.as_slice(), OGG_MONO_48K, OGG_LEFT_44K1] {
        for _ in 0..400 {
            let mut bytes = base.to_vec();
            for _ in 0..=(rng.next_u64() % 4) {
                let at = (rng.next_u64() % bytes.len() as u64) as usize;
                bytes[at] ^= 1 << (rng.next_u64() % 8);
            }
            if let Ok(s) = decode(&bytes, RATE) {
                assert_eq!(s.data().len(), 2 * s.frames());
            }
        }
    }
}
