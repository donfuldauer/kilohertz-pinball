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
//! # Limits (tables are untrusted downloads, D25)
//! * Mixer rate outside `1..=`[`MAX_MIXER_RATE`], or a file rate outside
//!   [`MIN_SOURCE_RATE`]`..=`[`MAX_SOURCE_RATE`]: [`DecodeError::Unsupported`].
//! * Longer than [`MAX_SECONDS`] (10 minutes) at the file rate or, after
//!   resampling, at the mixer rate: [`DecodeError::TooLong`]. The length is
//!   checked up front from the WAV data-chunk size or the last Ogg page's
//!   granule position, and again while decoding: resampling is streamed
//!   (source frames are never buffered), so no more than 10 minutes of
//!   output is ever allocated, and an up-front reservation never exceeds what
//!   the input bytes (WAV) or the granule (Vorbis) can account for.
//! * Ogg buffers with more than one logical stream (chained or multiplexed,
//!   seen as a serial-number change): [`DecodeError::Unsupported`].
//! * The Vorbis comment header is skipped unparsed; the identification and
//!   setup headers go through lewton's low-level parsers, vendored with
//!   allocation bounds (`third_party/lewton/KHZ_PATCHES.md`), because an
//!   allocation failure aborts the process and cannot be caught.
//! * Vorbis output is cut at the last page's granule position (encoder
//!   padding; Vorbis I §A.2).
//!
//! `decode` never panics: in addition to explicit validation, both decoders
//! run under `catch_unwind`, and a panic inside them is reported as
//! [`DecodeError::Malformed`].
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

/// Lowest source (file) sample rate accepted; lower rates are
/// [`DecodeError::Unsupported`]. Bounds the output/input size ratio.
pub const MIN_SOURCE_RATE: u32 = 8_000;

/// Highest source (file) sample rate accepted; higher rates are
/// [`DecodeError::Unsupported`]. Bounds the work and memory per input byte.
pub const MAX_SOURCE_RATE: u32 = 384_000;

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
    if is_wav {
        guarded(|| decode_wav(bytes, mixer_rate))
    } else if is_ogg {
        guarded(|| decode_vorbis(bytes, mixer_rate))
    } else {
        Err(DecodeError::Unsupported)
    }
}

fn guarded(f: impl FnOnce() -> Result<Sample, DecodeError>) -> Result<Sample, DecodeError> {
    catch_unwind(AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(DecodeError::Malformed("decoder panicked".into())))
}

fn malformed(e: impl fmt::Display) -> DecodeError {
    DecodeError::Malformed(e.to_string())
}

/// Validates the source layout: 1 or 2 channels at a rate in
/// `MIN_SOURCE_RATE..=MAX_SOURCE_RATE`.
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
    if !(MIN_SOURCE_RATE..=MAX_SOURCE_RATE).contains(&rate) {
        return Err(DecodeError::Unsupported);
    }
    Ok(())
}

fn max_frames(rate: u32) -> u64 {
    MAX_SECONDS * u64::from(rate)
}

/// Rejects a source of `n_in` frames whose decoded form would exceed the cap
/// at either rate.
fn check_length(n_in: u64, src: u32, dst: u32) -> Result<(), DecodeError> {
    if n_in > max_frames(src) || resampled_len(n_in, src, dst) > max_frames(dst) {
        Err(DecodeError::TooLong)
    } else {
        Ok(())
    }
}

/// Streaming stereo expansion + linear resampler (module docs). Source frames
/// are pushed one at a time; output frame `j` is emitted as soon as both of
/// its source frames are known, so only the output is buffered and the
/// length caps are enforced while decoding.
struct Resampler {
    src: u64,
    dst: u64,
    /// Source frames pushed.
    n_in: u64,
    /// Next output frame.
    j: u64,
    prev: (f32, f32),
    last: (f32, f32),
    out: Vec<f32>,
    max_in: u64,
    max_out: u64,
}

impl Resampler {
    fn new(src: u32, dst: u32, expected_in: u64) -> Self {
        let expected_out = resampled_len(expected_in, src, dst).min(max_frames(dst));
        Self {
            src: u64::from(src),
            dst: u64::from(dst),
            n_in: 0,
            j: 0,
            prev: (0.0, 0.0),
            last: (0.0, 0.0),
            out: Vec::with_capacity(2 * expected_out as usize),
            max_in: max_frames(src),
            max_out: max_frames(dst),
        }
    }

    fn emit(&mut self, a: (f32, f32), b: (f32, f32), f: f64) -> Result<(), DecodeError> {
        if self.j >= self.max_out {
            return Err(DecodeError::TooLong);
        }
        self.out.push(lerp(a.0, b.0, f));
        self.out.push(lerp(a.1, b.1, f));
        self.j += 1;
        Ok(())
    }

    fn push(&mut self, l: f32, r: f32) -> Result<(), DecodeError> {
        if self.n_in >= self.max_in {
            return Err(DecodeError::TooLong);
        }
        self.n_in += 1;
        if self.src == self.dst {
            if self.j >= self.max_out {
                return Err(DecodeError::TooLong);
            }
            self.out.push(l);
            self.out.push(r);
            self.j += 1;
            return Ok(());
        }
        self.prev = self.last;
        self.last = (l, r);
        // Source frame k = n_in - 1 just arrived: every pending output frame
        // with i = floor(j·src/dst) = k - 1 can now be interpolated.
        let k = self.n_in - 1;
        while k >= 1 {
            let pos = self.j * self.src;
            let i = pos / self.dst;
            if i + 1 > k {
                break;
            }
            let f = (pos % self.dst) as f64 / self.dst as f64;
            self.emit(self.prev, self.last, f)?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<Sample, DecodeError> {
        if self.src != self.dst && self.n_in > 0 {
            // Remaining frames have i ≥ n_in − 1: both neighbours clamp to the
            // last source frame.
            let n_out = resampled_len(self.n_in, self.src as u32, self.dst as u32);
            while self.j < n_out {
                let f = ((self.j * self.src) % self.dst) as f64 / self.dst as f64;
                self.emit(self.last, self.last, f)?;
            }
        }
        Ok(Sample::from_interleaved(self.out))
    }
}

fn map_hound(e: hound::Error) -> DecodeError {
    match e {
        hound::Error::Unsupported => DecodeError::Unsupported,
        other => malformed(other),
    }
}

fn decode_wav(bytes: &[u8], dst: u32) -> Result<Sample, DecodeError> {
    let reader = hound::WavReader::new(Cursor::new(bytes)).map_err(map_hound)?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels);
    check_rate_and_channels(spec.sample_rate, channels)?;
    let frames = u64::from(reader.duration());
    check_length(frames, spec.sample_rate, dst)?;
    // Never trust the header beyond what the bytes can hold.
    let bytes_per_frame = (usize::from(spec.bits_per_sample.div_ceil(8)).max(1) * channels) as u64;
    let expected = frames.min(bytes.len() as u64 / bytes_per_frame);
    let mut rs = Resampler::new(spec.sample_rate, dst, expected);
    let mut pending: Option<f32> = None;
    let mut feed = |v: f32| -> Result<(), DecodeError> {
        if channels == 1 {
            rs.push(v, v)
        } else if let Some(l) = pending.take() {
            rs.push(l, v)
        } else {
            pending = Some(v);
            Ok(())
        }
    };
    match spec.sample_format {
        hound::SampleFormat::Float => {
            if spec.bits_per_sample != 32 {
                return Err(DecodeError::Unsupported);
            }
            for s in reader.into_samples::<f32>() {
                feed(s.map_err(map_hound)?)?;
            }
        }
        hound::SampleFormat::Int => {
            if spec.bits_per_sample == 0 || spec.bits_per_sample > 32 {
                return Err(DecodeError::Unsupported);
            }
            let scale = f64::from(1u32 << (spec.bits_per_sample - 1));
            for s in reader.into_samples::<i32>() {
                let v = s.map_err(map_hound)?;
                feed((f64::from(v) / scale) as f32)?;
            }
        }
    }
    rs.finish()
}

/// Page-level scan of an Ogg buffer: the serial of the first page, whether
/// any page has another serial (chained or multiplexed streams), and the
/// granule position of the last page that has one (the stream length in
/// frames for a stream starting at granule 0). Stops at the first byte
/// sequence that is not a complete page; the packet reader reports those.
struct OggScan {
    serial: u32,
    other_serial: bool,
    last_granule: Option<u64>,
}

fn scan_ogg_pages(b: &[u8]) -> OggScan {
    let mut scan = OggScan {
        serial: 0,
        other_serial: false,
        last_granule: None,
    };
    let mut p = 0usize;
    let mut first = true;
    while b.len() >= p + 27 && &b[p..p + 4] == b"OggS" {
        let segs = usize::from(b[p + 26]);
        if b.len() < p + 27 + segs {
            break;
        }
        let body: usize = b[p + 27..p + 27 + segs].iter().map(|&x| usize::from(x)).sum();
        let mut word = [0u8; 8];
        word.copy_from_slice(&b[p + 6..p + 14]);
        let granule = u64::from_le_bytes(word);
        let mut s4 = [0u8; 4];
        s4.copy_from_slice(&b[p + 14..p + 18]);
        let serial = u32::from_le_bytes(s4);
        if first {
            scan.serial = serial;
            first = false;
        } else if serial != scan.serial {
            scan.other_serial = true;
        }
        if granule != u64::MAX {
            scan.last_granule = Some(granule);
        }
        p += 27 + segs + body;
    }
    scan
}

fn decode_vorbis(bytes: &[u8], dst: u32) -> Result<Sample, DecodeError> {
    use lewton::audio::{PreviousWindowRight, read_audio_packet_generic};
    use lewton::header::{read_header_ident, read_header_setup};
    use lewton::samples::InterleavedSamples;

    let scan = scan_ogg_pages(bytes);
    if scan.other_serial {
        // Chained or multiplexed: each link restarts its granule positions,
        // so the length cannot be bounded up front.
        return Err(DecodeError::Unsupported);
    }
    let mut rdr = ogg::PacketReader::new(Cursor::new(bytes));
    let mut next = |what: &str| -> Result<ogg::Packet, DecodeError> {
        let p = rdr
            .read_packet()
            .map_err(malformed)?
            .ok_or_else(|| DecodeError::Malformed(format!("missing {what}")))?;
        if p.stream_serial() != scan.serial {
            return Err(DecodeError::Unsupported);
        }
        Ok(p)
    };
    let ident = read_header_ident(&next("identification header")?.data).map_err(malformed)?;
    let channels = usize::from(ident.audio_channels);
    let src = ident.audio_sample_rate;
    check_rate_and_channels(src, channels)?;
    if let Some(total) = scan.last_granule {
        check_length(total, src, dst)?;
    }
    // The comment header is skipped unparsed: nothing in it is needed, and
    // its counts are attacker-controlled.
    let _comment = next("comment header")?;
    let setup = read_header_setup(
        &next("setup header")?.data,
        ident.audio_channels,
        (ident.blocksize_0, ident.blocksize_1),
    )
    .map_err(malformed)?;

    // Frames past the last granule position are encoder padding (Vorbis I
    // §A.2); without a granule the decoded length stands.
    let limit = scan.last_granule.unwrap_or(u64::MAX);
    let mut rs = Resampler::new(src, dst, scan.last_granule.unwrap_or(0));
    let mut pwr = PreviousWindowRight::new();
    'packets: loop {
        let packet = match rdr.read_packet().map_err(malformed)? {
            Some(p) => p,
            None => break,
        };
        if packet.stream_serial() != scan.serial {
            return Err(DecodeError::Unsupported);
        }
        let decoded: InterleavedSamples<i16> =
            read_audio_packet_generic(&ident, &setup, &packet.data, &mut pwr).map_err(malformed)?;
        for frame in decoded.samples.chunks_exact(channels) {
            if rs.n_in >= limit {
                break 'packets;
            }
            let l = (f64::from(frame[0]) / 32768.0) as f32;
            let r = (f64::from(frame[channels - 1]) / 32768.0) as f32;
            rs.push(l, r)?;
        }
    }
    rs.finish()
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

    /// Sets a WAV's data-chunk size field (the header claims, not the bytes).
    fn claim_data_len(bytes: &mut [u8], len: u32) {
        let at = (0..bytes.len() - 4)
            .rev()
            .find(|&i| &bytes[i..i + 4] == b"data")
            .unwrap();
        bytes[at + 4..at + 8].copy_from_slice(&len.to_le_bytes());
    }

    #[test]
    fn too_long_by_header() {
        // 8-bit mono at 8 kHz claiming 601 s: rejected before reading data.
        let mut bytes = wav(spec(1, 8_000, 8, false), |w| w.write_sample(0i8).unwrap());
        claim_data_len(&mut bytes, 601 * 8_000);
        assert_eq!(decode(&bytes, 8_000), Err(DecodeError::TooLong));
        // Exactly 600 s claimed is within the cap (then fails as truncated).
        claim_data_len(&mut bytes, 600 * 8_000);
        assert!(matches!(decode(&bytes, 8_000), Err(DecodeError::Malformed(_))));
        // 600 s at 8 kHz upsampled to 48 kHz is still 600 s: fine; a claim
        // of u32::MAX bytes is too long.
        claim_data_len(&mut bytes, u32::MAX);
        assert_eq!(decode(&bytes, 48_000), Err(DecodeError::TooLong));
    }

    #[test]
    fn source_rate_bounds() {
        for (rate, ok) in [
            (1u32, false),
            (7_999, false),
            (8_000, true),
            (384_000, true),
            (384_001, false),
            (1_000_000_000, false),
        ] {
            let bytes = wav(spec(1, rate, 16, false), |w| {
                for _ in 0..600 {
                    w.write_sample(0i16).unwrap();
                }
            });
            let r = decode(&bytes, 48_000);
            if ok {
                assert!(r.is_ok(), "rate {rate}");
            } else {
                assert_eq!(r, Err(DecodeError::Unsupported), "rate {rate}");
            }
        }
    }

    /// The batch formula from the module docs, as a reference.
    fn reference(x: &[(f32, f32)], src: u32, dst: u32) -> Vec<f32> {
        if src == dst {
            return x.iter().flat_map(|&(l, r)| [l, r]).collect();
        }
        let n_out = resampled_len(x.len() as u64, src, dst);
        let mut out = Vec::new();
        if x.is_empty() {
            return out;
        }
        for j in 0..n_out {
            let pos = j * u64::from(src);
            let i = (pos / u64::from(dst)) as usize;
            let f = (pos % u64::from(dst)) as f64 / f64::from(dst);
            let a = x[i.min(x.len() - 1)];
            let b = x[(i + 1).min(x.len() - 1)];
            out.push(lerp(a.0, b.0, f));
            out.push(lerp(a.1, b.1, f));
        }
        out
    }

    fn stream(x: &[(f32, f32)], src: u32, dst: u32) -> Vec<f32> {
        let mut rs = Resampler::new(src, dst, x.len() as u64);
        for &(l, r) in x {
            rs.push(l, r).unwrap();
        }
        rs.finish().unwrap().data().to_vec()
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig { cases: 200, ..Default::default() })]
        #[test]
        fn streaming_resampler_matches_formula(
            x in proptest::collection::vec((-1.0f32..1.0, -1.0f32..1.0), 0..300),
            src in proptest::sample::select(vec![8_000u32, 11_025, 22_050, 44_100, 48_000, 96_000, 384_000]),
            dst in proptest::sample::select(vec![8_000u32, 22_050, 44_100, 48_000, 192_000]),
        ) {
            proptest::prop_assert_eq!(stream(&x, src, dst), reference(&x, src, dst));
        }
    }

    #[test]
    fn resampler_caps_while_streaming() {
        let mut rs = Resampler::new(8_000, 48_000, 10);
        rs.max_out = 11;
        assert_eq!(rs.push(0.0, 0.0), Ok(()));
        assert_eq!(rs.push(0.0, 0.0), Ok(())); // 6 out
        assert_eq!(rs.push(0.0, 0.0), Err(DecodeError::TooLong)); // 12th
        let mut rs = Resampler::new(8_000, 8_000, 0);
        rs.max_in = 2;
        rs.push(0.0, 0.0).unwrap();
        rs.push(0.0, 0.0).unwrap();
        assert_eq!(rs.push(0.0, 0.0), Err(DecodeError::TooLong));
        let mut rs = Resampler::new(8_000, 8_000, 0);
        rs.max_out = 1;
        rs.push(0.0, 0.0).unwrap();
        assert_eq!(rs.push(0.0, 0.0), Err(DecodeError::TooLong));
        let mut rs = Resampler::new(8_000, 16_000, 0);
        rs.max_out = 1;
        rs.push(0.5, 0.5).unwrap();
        assert_eq!(rs.finish(), Err(DecodeError::TooLong));
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
