//! Sample decoding: WAV (`hound`) and Ogg Vorbis (`lewton`) byte buffers into
//! interleaved stereo `f32` at the mixer rate.
//!
//! Decoding happens once, at load time, on the I/O worker; the result is a
//! [`Sample`] the mixer plays without further conversion.
//!
//! # Conversion rules
//! * Integer PCM of `b` bits (8, 16, 24 or 32; hound reads 8-bit WAV data,
//!   which is unsigned on disk, as signed `v - 128`) maps to
//!   `v / 2^(b-1)`, computed in `f64` and rounded once to `f32`. Hence
//!   `i16::MIN` → `-1.0` and `i16::MAX` → `32767 / 32768`.
//! * 32-bit float PCM is copied as is.
//! * Vorbis is decoded to `i16` by `lewton` and converted like 16-bit PCM.
//! * Mono is copied to both channels; stereo is kept; more than two channels
//!   is [`DecodeError::TooManyChannels`].
//!
//! # Resampling (linear interpolation)
//! With `n_in` source frames at rate `src` and mixer rate `dst`:
//! * `src == dst`: the frames are copied unchanged.
//! * otherwise `n_out = ceil(n_in · dst / src)` and output frame `j` reads
//!   source position `t = j · src / dst`, split exactly in integers into
//!   `i = floor(j·src / dst)` and `f = (j·src mod dst) / dst`:
//!   `y[j] = x[i] · (1 − f) + x[i+1] · f`, where `x[i+1]` is clamped to the
//!   last source frame (`x[n_in-1]`) past the end. The arithmetic is done in
//!   `f64` and rounded once to `f32`, per channel.
//!
//! # Limits
//! Decoded audio longer than [`MAX_SECONDS`] (10 minutes, measured both at
//! the source rate and at the mixer rate) is [`DecodeError::TooLong`]. A mixer
//! rate outside `1..=`[`MAX_MIXER_RATE`] is [`DecodeError::Unsupported`].
//! `decode` never panics: in addition to explicit validation, both third-party
//! decoders run under `catch_unwind`, and a panic inside them is reported as
//! [`DecodeError::Malformed`]. Buffers are never pre-sized from header claims
//! beyond what the input bytes can hold.
//!
//! # Test fixture provenance
//! `fixtures/tone440_mono_22k.ogg` (3800 bytes, 5513 frames) was produced
//! offline with FFmpeg 6.1.1 + libvorbis 1.3.7:
//! `ffmpeg -f lavfi -i "aevalsrc=0.5*sin(2*PI*440*t):s=22050:d=0.25" -ac 1
//! -c:a libvorbis -q:a 2 -map_metadata -1 -fflags +bitexact -flags:a +bitexact
//! tone440_mono_22k.ogg`.

use std::fmt;
use std::io::Cursor;
use std::panic::{AssertUnwindSafe, catch_unwind};

/// Longest decoded sample accepted, in seconds.
pub const MAX_SECONDS: u64 = 600;

/// Highest mixer rate [`decode`] accepts.
pub const MAX_MIXER_RATE: u32 = 384_000;

/// An immutable decoded sound: interleaved stereo `f32` at the mixer rate.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Sample {
    data: Box<[f32]>,
}

impl Sample {
    /// Wraps interleaved stereo data `[l0, r0, l1, r1, ...]`. A trailing odd
    /// value (half a frame) is discarded.
    pub fn from_interleaved(mut data: Vec<f32>) -> Self {
        if data.len() % 2 == 1 {
            data.pop();
        }
        Self {
            data: data.into_boxed_slice(),
        }
    }

    /// Builds a stereo sample from one mono channel (copied to both sides).
    pub fn from_mono(mono: &[f32]) -> Self {
        let mut data = Vec::with_capacity(mono.len() * 2);
        for &v in mono {
            data.push(v);
            data.push(v);
        }
        Self::from_interleaved(data)
    }

    /// A constant ("DC") sample of `frames` frames with `left`/`right` values;
    /// handy for exact mixing tests.
    pub fn constant(frames: usize, left: f32, right: f32) -> Self {
        let mut data = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            data.push(left);
            data.push(right);
        }
        Self::from_interleaved(data)
    }

    /// Number of stereo frames.
    pub fn frames(&self) -> usize {
        self.data.len() / 2
    }

    /// Interleaved stereo samples; `len() == 2 * frames()`.
    pub fn data(&self) -> &[f32] {
        &self.data
    }
}

/// Why [`decode`] rejected a buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Not a WAV (`RIFF....WAVE`) or Ogg (`OggS`) buffer, an encoding inside
    /// one that is not supported (e.g. 64-bit float WAV, compressed WAV), or a
    /// mixer rate outside `1..=MAX_MIXER_RATE`.
    Unsupported,
    /// The container or bitstream is invalid or truncated.
    Malformed(String),
    /// More than two channels.
    TooManyChannels,
    /// Longer than [`MAX_SECONDS`].
    TooLong,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("unsupported audio format"),
            Self::Malformed(why) => write!(f, "malformed audio data: {why}"),
            Self::TooManyChannels => f.write_str("more than two audio channels"),
            Self::TooLong => write!(f, "audio longer than {MAX_SECONDS} seconds"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Decodes a WAV or Ogg Vorbis buffer into a stereo [`Sample`] at
/// `mixer_rate` (see the module docs for the exact rules). Never panics.
pub fn decode(bytes: &[u8], mixer_rate: u32) -> Result<Sample, DecodeError> {
    if mixer_rate == 0 || mixer_rate > MAX_MIXER_RATE {
        return Err(DecodeError::Unsupported);
    }
    let is_wav = bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE";
    let is_ogg = bytes.len() >= 4 && &bytes[0..4] == b"OggS";
    let decoded = if is_wav {
        guarded(|| decode_wav(bytes))?
    } else if is_ogg {
        guarded(|| decode_vorbis(bytes))?
    } else {
        return Err(DecodeError::Unsupported);
    };
    finish(decoded, mixer_rate)
}

/// Source audio before resampling: interleaved `channels` (1 or 2).
struct Decoded {
    samples: Vec<f32>,
    channels: usize,
    rate: u32,
}

fn guarded(f: impl FnOnce() -> Result<Decoded, DecodeError>) -> Result<Decoded, DecodeError> {
    catch_unwind(AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(DecodeError::Malformed("decoder panicked".into())))
}

fn malformed(e: impl fmt::Display) -> DecodeError {
    DecodeError::Malformed(e.to_string())
}

fn check_rate_and_channels(rate: u32, channels: usize) -> Result<(), DecodeError> {
    if channels == 0 {
        return Err(DecodeError::Malformed("zero channels".into()));
    }
    if channels > 2 {
        return Err(DecodeError::TooManyChannels);
    }
    if rate == 0 {
        return Err(DecodeError::Malformed("sample rate 0".into()));
    }
    Ok(())
}

fn max_frames(rate: u32) -> u64 {
    MAX_SECONDS * u64::from(rate)
}

fn map_hound(e: hound::Error) -> DecodeError {
    match e {
        hound::Error::Unsupported => DecodeError::Unsupported,
        other => malformed(other),
    }
}

fn decode_wav(bytes: &[u8]) -> Result<Decoded, DecodeError> {
    let reader = hound::WavReader::new(Cursor::new(bytes)).map_err(map_hound)?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels);
    check_rate_and_channels(spec.sample_rate, channels)?;
    if u64::from(reader.duration()) > max_frames(spec.sample_rate) {
        return Err(DecodeError::TooLong);
    }
    let bytes_per_sample = usize::from(spec.bits_per_sample.div_ceil(8)).max(1);
    let claimed = reader.len() as usize;
    let mut samples = Vec::with_capacity(claimed.min(bytes.len() / bytes_per_sample));
    match spec.sample_format {
        hound::SampleFormat::Float => {
            if spec.bits_per_sample != 32 {
                return Err(DecodeError::Unsupported);
            }
            for s in reader.into_samples::<f32>() {
                samples.push(s.map_err(map_hound)?);
            }
        }
        hound::SampleFormat::Int => {
            if spec.bits_per_sample == 0 || spec.bits_per_sample > 32 {
                return Err(DecodeError::Unsupported);
            }
            let scale = f64::from(1u32 << (spec.bits_per_sample - 1));
            for s in reader.into_samples::<i32>() {
                let v = s.map_err(map_hound)?;
                samples.push((f64::from(v) / scale) as f32);
            }
        }
    }
    Ok(Decoded {
        samples,
        channels,
        rate: spec.sample_rate,
    })
}

fn decode_vorbis(bytes: &[u8]) -> Result<Decoded, DecodeError> {
    let mut reader =
        lewton::inside_ogg::OggStreamReader::new(Cursor::new(bytes)).map_err(malformed)?;
    let channels = usize::from(reader.ident_hdr.audio_channels);
    let rate = reader.ident_hdr.audio_sample_rate;
    check_rate_and_channels(rate, channels)?;
    let limit = max_frames(rate).saturating_mul(channels as u64);
    let mut samples: Vec<f32> = Vec::new();
    while let Some(packet) = reader.read_dec_packet_itl().map_err(malformed)? {
        // A chained stream may switch parameters; only a constant layout is
        // supported.
        if usize::from(reader.ident_hdr.audio_channels) != channels
            || reader.ident_hdr.audio_sample_rate != rate
        {
            return Err(DecodeError::Unsupported);
        }
        if (samples.len() + packet.len()) as u64 > limit {
            return Err(DecodeError::TooLong);
        }
        samples.extend(packet.iter().map(|&v| (f64::from(v) / 32768.0) as f32));
    }
    let mut whole = samples.len() - samples.len() % channels;
    // The last page's granule position is the stream's length in frames
    // (Vorbis I §A.2). lewton only trims the final packet when it has seen an
    // earlier page's granule, so trim here too (single-page files).
    if let Some(total) = reader.get_last_absgp() {
        let total = usize::try_from(total).unwrap_or(usize::MAX);
        whole = whole.min(total.saturating_mul(channels));
    }
    samples.truncate(whole);
    Ok(Decoded {
        samples,
        channels,
        rate,
    })
}

/// Expands to stereo and resamples to `dst` (see the module docs).
fn finish(d: Decoded, dst: u32) -> Result<Sample, DecodeError> {
    let n_in = d.samples.len() / d.channels;
    let src = d.rate;
    let frame = |i: usize| -> (f32, f32) {
        if d.channels == 1 {
            let v = d.samples[i];
            (v, v)
        } else {
            (d.samples[2 * i], d.samples[2 * i + 1])
        }
    };
    if src == dst {
        if d.channels == 2 {
            return Ok(Sample::from_interleaved(d.samples));
        }
        return Ok(Sample::from_mono(&d.samples));
    }
    let n_out = resampled_len(n_in as u64, src, dst);
    if n_out > max_frames(dst) {
        return Err(DecodeError::TooLong);
    }
    let n_out = n_out as usize;
    let mut out = Vec::with_capacity(n_out * 2);
    if n_in == 0 {
        return Ok(Sample::from_interleaved(out));
    }
    let (src64, dst64) = (u64::from(src), u64::from(dst));
    for j in 0..n_out as u64 {
        let pos = j * src64;
        let i = (pos / dst64) as usize;
        let f = (pos % dst64) as f64 / dst64 as f64;
        let i0 = i.min(n_in - 1);
        let i1 = (i + 1).min(n_in - 1);
        let (l0, r0) = frame(i0);
        let (l1, r1) = frame(i1);
        out.push(lerp(l0, l1, f));
        out.push(lerp(r0, r1, f));
    }
    Ok(Sample::from_interleaved(out))
}

/// `ceil(n_in · dst / src)`, the resampled length in frames.
pub fn resampled_len(n_in: u64, src: u32, dst: u32) -> u64 {
    if src == dst {
        return n_in;
    }
    let num = u128::from(n_in) * u128::from(dst);
    let den = u128::from(src.max(1));
    u64::try_from(num.div_ceil(den)).unwrap_or(u64::MAX)
}

fn lerp(a: f32, b: f32, f: f64) -> f32 {
    (f64::from(a) * (1.0 - f) + f64::from(b) * f) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wav(
        spec: hound::WavSpec,
        write: impl FnOnce(&mut hound::WavWriter<Cursor<&mut Vec<u8>>>),
    ) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = hound::WavWriter::new(Cursor::new(&mut buf), spec).unwrap();
            write(&mut w);
            w.finalize().unwrap();
        }
        buf
    }

    fn spec(channels: u16, rate: u32, bits: u16, float: bool) -> hound::WavSpec {
        hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: bits,
            sample_format: if float {
                hound::SampleFormat::Float
            } else {
                hound::SampleFormat::Int
            },
        }
    }

    #[test]
    fn int_depths_scale_exactly() {
        for (bits, vals) in [
            (8u16, vec![-128i32, 0, 64, 127]),
            (16, vec![-32768, 0, 16384, 32767]),
            (24, vec![-8_388_608, 0, 4_194_304, 8_388_607]),
            (32, vec![i32::MIN, 0, 1 << 30, i32::MAX]),
        ] {
            let bytes = wav(spec(1, 48_000, bits, false), |w| {
                for &v in &vals {
                    w.write_sample(v).unwrap();
                }
            });
            let s = decode(&bytes, 48_000).unwrap();
            assert_eq!(s.frames(), 4);
            let scale = f64::from(1u32 << (bits - 1));
            for (k, &v) in vals.iter().enumerate() {
                let e = (f64::from(v) / scale) as f32;
                assert_eq!(s.data()[2 * k], e, "bits {bits}");
                assert_eq!(s.data()[2 * k + 1], e, "bits {bits}");
            }
            assert_eq!(s.data()[0], -1.0);
            assert_eq!(s.data()[4], 0.5);
        }
    }

    #[test]
    fn float_stereo_copied() {
        let bytes = wav(spec(2, 48_000, 32, true), |w| {
            for v in [0.25f32, -0.5, 1.5, 0.0] {
                w.write_sample(v).unwrap();
            }
        });
        let s = decode(&bytes, 48_000).unwrap();
        assert_eq!(s.data(), &[0.25, -0.5, 1.5, 0.0]);
    }

    #[test]
    fn too_many_channels() {
        let bytes = wav(spec(3, 48_000, 16, false), |w| {
            for _ in 0..3 {
                w.write_sample(0i16).unwrap();
            }
        });
        assert_eq!(decode(&bytes, 48_000), Err(DecodeError::TooManyChannels));
    }

    #[test]
    fn resample_linear_formula() {
        // 24 kHz → 48 kHz: every other output frame is a midpoint.
        let bytes = wav(spec(1, 24_000, 16, false), |w| {
            for v in [0i16, 16384, -16384] {
                w.write_sample(v).unwrap();
            }
        });
        let s = decode(&bytes, 48_000).unwrap();
        assert_eq!(s.frames(), 6);
        let l: Vec<f32> = s.data().iter().step_by(2).copied().collect();
        assert_eq!(l, vec![0.0, 0.25, 0.5, 0.0, -0.5, -0.5]);
        // 22.05 kHz → 48 kHz length.
        assert_eq!(resampled_len(22_050, 22_050, 48_000), 48_000);
        assert_eq!(resampled_len(441, 44_100, 48_000), 480);
        assert_eq!(resampled_len(1, 44_100, 48_000), 2);
        assert_eq!(resampled_len(0, 44_100, 48_000), 0);
    }

    #[test]
    fn downsample_and_empty() {
        let bytes = wav(spec(2, 96_000, 16, false), |w| {
            for k in 0..8i16 {
                w.write_sample(k * 1000).unwrap();
                w.write_sample(-k * 1000).unwrap();
            }
        });
        let s = decode(&bytes, 48_000).unwrap();
        assert_eq!(s.frames(), 4);
        assert_eq!(s.data()[2], (2000.0f64 / 32768.0) as f32);
        assert_eq!(s.data()[3], (-2000.0f64 / 32768.0) as f32);
        let empty = wav(spec(1, 44_100, 16, false), |_| {});
        assert_eq!(decode(&empty, 48_000).unwrap().frames(), 0);
    }

    #[test]
    fn unsupported_inputs() {
        assert_eq!(decode(b"", 48_000), Err(DecodeError::Unsupported));
        assert_eq!(
            decode(b"ID3\x04rest", 48_000),
            Err(DecodeError::Unsupported)
        );
        let ok = wav(spec(1, 48_000, 16, false), |w| {
            w.write_sample(0i16).unwrap()
        });
        assert_eq!(decode(&ok, 0), Err(DecodeError::Unsupported));
        assert_eq!(
            decode(&ok, MAX_MIXER_RATE + 1),
            Err(DecodeError::Unsupported)
        );
        assert!(matches!(
            decode(b"RIFF\0\0\0\0WAVE", 48_000),
            Err(DecodeError::Malformed(_))
        ));
        assert!(matches!(
            decode(b"OggS\0\0\0", 48_000),
            Err(DecodeError::Malformed(_))
        ));
    }

    #[test]
    fn too_long_by_header() {
        // 8-bit mono at 1 kHz: 601 s = 601_000 bytes of data.
        let bytes = wav(spec(1, 1_000, 8, false), |w| {
            for _ in 0..601_000 {
                w.write_sample(0i8).unwrap();
            }
        });
        assert_eq!(decode(&bytes, 1_000), Err(DecodeError::TooLong));
        // Header claims a huge data chunk while the bytes are short.
        let mut lie = wav(spec(1, 1_000, 8, false), |w| w.write_sample(0i8).unwrap());
        let n = lie.len();
        lie[n - 5..n - 1].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(decode(&lie, 1_000).is_err());
    }

    #[test]
    fn display_messages() {
        assert!(DecodeError::TooLong.to_string().contains("600"));
        assert!(DecodeError::Malformed("x".into()).to_string().contains('x'));
        assert!(!DecodeError::Unsupported.to_string().is_empty());
        assert!(!DecodeError::TooManyChannels.to_string().is_empty());
    }

    const OGG: &[u8] = include_bytes!("fixtures/tone440_mono_22k.ogg");

    #[test]
    fn vorbis_fixture() {
        // 0.25 s of a 440 Hz sine at amplitude 0.5, mono, 22.05 kHz (provenance
        // in the module docs): 5513 source frames.
        let s = decode(OGG, 48_000).unwrap();
        let expected = resampled_len(5_513, 22_050, 48_000) as usize;
        assert!(s.frames().abs_diff(expected) <= 3, "frames {}", s.frames());
        let peak = s.data().iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((0.4..=0.6).contains(&peak), "peak {peak}");
        // Mono → identical channels.
        assert!(s.data().chunks(2).all(|c| c[0] == c[1]));
        // Roughly 440 Hz: count positive-going zero crossings in the middle.
        let left: Vec<f32> = s.data().iter().step_by(2).copied().collect();
        let mid = &left[2_400..2_400 + 4_800]; // 0.1 s
        let ups = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((42..=46).contains(&ups), "crossings {ups}");
        // Native rate keeps the length.
        let native = decode(OGG, 22_050).unwrap();
        assert!(native.frames().abs_diff(5_513) <= 1);
    }

    #[test]
    fn vorbis_truncations_never_panic() {
        for cut in (0..OGG.len()).step_by(37) {
            let _ = decode(&OGG[..cut], 48_000);
        }
        let mut flipped = OGG.to_vec();
        for k in (60..flipped.len()).step_by(101) {
            flipped[k] ^= 0x5a;
            let _ = decode(&flipped, 48_000);
        }
    }

    #[test]
    fn sample_constructors() {
        let s = Sample::from_interleaved(vec![1.0, 2.0, 3.0]);
        assert_eq!(s.frames(), 1);
        assert_eq!(Sample::from_mono(&[0.5]).data(), &[0.5, 0.5]);
        assert_eq!(Sample::constant(2, 0.1, 0.2).data(), &[0.1, 0.2, 0.1, 0.2]);
        assert_eq!(Sample::default().frames(), 0);
    }
}
