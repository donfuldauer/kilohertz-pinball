//! Sample decoding: WAV (`hound`) and Ogg Vorbis (`lewton`) byte buffers into
//! mono or interleaved stereo `f32` at the mixer rate.
//!
//! A resident sound is decoded once, at load time, on the I/O worker; the
//! result is a [`Sample`] the mixer plays without further conversion. A
//! streamed music track ([`open_stream`](super::open_stream)) runs the same
//! code on its loader thread, pass after pass: one decode path (header
//! validation, the WAV and Vorbis decoders and the linear resampler) feeds
//! either the budget-charged output buffer of [`decode_with_budget`] or
//! the track's ring, so both produce bit-identical frames and enforce the
//! same limits.
//!
//! # Conversion rules
//! * Integer PCM of `b` bits (8, 16, 24 or 32; hound reads 8-bit WAV data,
//!   which is unsigned on disk, as signed `v - 128`) maps to
//!   `v / 2^(b-1)`, computed in `f64` and rounded once to `f32`. Hence
//!   `i16::MIN` → `-1.0` and `i16::MAX` → `32767 / 32768`.
//! * 32-bit float PCM is copied as is.
//! * Vorbis is decoded to `i16` by `lewton` and converted like 16-bit PCM.
//! * Mono stays mono (one `f32` per frame, [`Sample::channels`] `== 1`; the
//!   mixer pans it to both sides at render time, bit for bit what a
//!   duplicated stereo copy would mix); stereo is kept; more than two
//!   channels is [`DecodeError::TooManyChannels`].
//!
//! # Resampling (linear interpolation)
//! With `n_in` source frames at rate `src` and mixer rate `dst`:
//! * `src == dst`: the frames are copied unchanged.
//! * otherwise `n_out = ceil(n_in · dst / src)` and output frame `j` reads
//!   source position `t = j · src / dst`, split exactly in integers into
//!   `i = floor(j·src / dst)` and `f = (j·src mod dst) / dst`:
//!   `y[j] = x[i] · (1 − f) + x[i+1] · f`, where `x[i+1]` is clamped to the
//!   last source frame (`x[n_in-1]`) past the end. The arithmetic is done in
//!   `f64` and rounded once to `f32`, per stored channel (a mono file is
//!   resampled once; the result equals either side of the duplicated pair).
//!
//! # Limits (tables are untrusted downloads, D25)
//! * Mixer rate outside `1..=`[`MAX_MIXER_RATE`], or a file rate outside
//!   [`MIN_SOURCE_RATE`]`..=`[`MAX_SOURCE_RATE`]: [`DecodeError::Unsupported`].
//! * Longer than [`MAX_SECONDS`] (10 minutes) at the file rate or, after
//!   resampling, at the mixer rate: [`DecodeError::TooLong`]. The length is
//!   checked up front from the WAV data-chunk size or the last Ogg page's
//!   granule position, and again while decoding: resampling is streamed
//!   (source frames are never buffered), so no more than 10 minutes of
//!   output is ever allocated; the up-front reservation is capped at
//!   [`INITIAL_RESERVE_FRAMES`] whatever the header claims (see "Memory").
//! * Ogg buffers with more than one logical stream (chained or multiplexed,
//!   seen as a serial-number change): [`DecodeError::Unsupported`].
//! * The Vorbis comment header is skipped unparsed; the identification and
//!   setup headers go through lewton's low-level parsers, vendored with
//!   allocation bounds (`third_party/lewton/KHZ_PATCHES.md`), because an
//!   allocation failure aborts the process and cannot be caught.
//! * Vorbis output is cut at the last page's granule position (encoder
//!   padding; Vorbis I §A.2). Samples a stream asks to discard at its start
//!   (a first audio page whose granule is lower than the samples it
//!   completes) are not dropped there: the cut happens at the end instead, so
//!   the length is right but the start is not skipped.
//! * Edge results: an Ogg stream with valid headers and no audio packets
//!   decodes to `Ok` with 0 frames, and so does a WAV whose data chunk is
//!   empty; bytes after the last complete Ogg page or after the WAV data
//!   chunk are ignored.
//!
//! # Memory (tables are untrusted downloads, D25)
//! No size read from a header or a granule position is trusted for
//! allocation. The output buffer starts at no more than
//! [`INITIAL_RESERVE_FRAMES`] frames (less when the header announces fewer)
//! and grows on demand, doubling, clamped to the 10-minute cap, so its
//! capacity never exceeds `max(INITIAL_RESERVE_FRAMES, 2 × frames produced)`
//! and is trimmed to the exact length when decoding ends.
//!
//! Decoded data of a whole bundle is bounded by a shared [`DecodeBudget`]
//! ([`decode_with_budget`]): every frame of output capacity is charged to the
//! budget before it is allocated, the unused part is refunded when the
//! decode ends, and a failed decode refunds everything it charged. A frame
//! costs [`BYTES_PER_MONO_FRAME`] (4) bytes for a mono file and
//! [`BYTES_PER_FRAME`] (8) for a stereo one. So after each call the budget's
//! `used_bytes` is exactly the sum of [`Sample::bytes`]
//! (`4 × channels() × frames()`) of the samples it returned (minus what was
//! [released](DecodeBudget::release)), and a decode succeeds if and only if
//! its output fits into what is left (for decodes that do not overlap in
//! time; while decodes run concurrently, each one's spare capacity is
//! charged, so a decode can fail early because of another one in flight).
//! The `used` counter never wraps or saturates: a release larger than what
//! is charged is refused with [`OverRelease`] and changes nothing, so after
//! every returned sample has been released exactly once `used_bytes` is 0.
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
use std::sync::atomic::{AtomicU64, Ordering};

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

/// Bytes of decoded data per stereo frame (two `f32`), what
/// [`DecodeBudget`] charges per frame of a stereo sample.
pub const BYTES_PER_FRAME: u64 = 8;

/// Bytes of decoded data per mono frame (one `f32`), what [`DecodeBudget`]
/// charges per frame of a mono sample.
pub const BYTES_PER_MONO_FRAME: u64 = 4;

/// Bytes of one stored `f32` value.
const BYTES_PER_VALUE: u64 = 4;

/// Most frames an output buffer reserves before any audio is decoded,
/// whatever the header or granule position announces (512 KiB).
pub const INITIAL_RESERVE_FRAMES: u64 = 1 << 16;

/// Smallest growth step of an output buffer, in frames.
const MIN_GROW_FRAMES: u64 = 1 << 10;

/// Aggregate budget of decoded audio for one bundle, in bytes of decoded
/// `f32` data ([`BYTES_PER_MONO_FRAME`] per mono frame, [`BYTES_PER_FRAME`]
/// per stereo frame), shared by every
/// [`decode_with_budget`] call for that bundle.
///
/// Charges are made as output is produced (in growth steps of the output
/// buffer, never from header sizes), are atomic and never exceed the limit;
/// the type is `Sync`, so decodes on several threads can share one budget
/// (e.g. behind an `Arc`). See the module docs for the exact accounting.
/// The per-file 10-minute cap still applies: a 48 kHz sample is at most
/// ~230 MB, so the [default](DecodeBudget::DEFAULT_BYTES) of 512 MiB holds
/// about 23 minutes of 48 kHz stereo per bundle.
#[derive(Debug)]
pub struct DecodeBudget {
    limit: u64,
    used: AtomicU64,
}

impl DecodeBudget {
    /// Default budget per bundle: 512 MiB of decoded audio.
    pub const DEFAULT_BYTES: u64 = 512 * 1024 * 1024;

    /// A budget of `limit_bytes` bytes of decoded data, none used.
    pub fn new(limit_bytes: u64) -> Self {
        Self {
            limit: limit_bytes,
            used: AtomicU64::new(0),
        }
    }

    /// A budget with no aggregate limit (the per-file cap still applies).
    pub fn unlimited() -> Self {
        Self::new(u64::MAX)
    }

    /// The limit in bytes.
    pub fn limit_bytes(&self) -> u64 {
        self.limit
    }

    /// Bytes currently charged.
    pub fn used_bytes(&self) -> u64 {
        self.used.load(Ordering::Acquire)
    }

    /// Bytes still available.
    pub fn remaining_bytes(&self) -> u64 {
        // `used <= limit` is an invariant (charges never exceed the limit and
        // releases never go below zero), so this cannot actually saturate.
        self.limit.saturating_sub(self.used_bytes())
    }

    /// Returns a sample's bytes ([`Sample::bytes`]) to the budget, e.g. when
    /// the loader drops it.
    ///
    /// The subtraction is checked and atomic: when the sample's bytes exceed
    /// what is currently charged (a sample released twice, or one that was
    /// never charged to this budget), nothing changes and the attempt is
    /// reported as [`OverRelease`].
    pub fn release(&self, sample: &Sample) -> Result<(), OverRelease> {
        let attempted_bytes = sample.bytes();
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_sub(attempted_bytes)
            })
            .map(|_| ())
            .map_err(|used_bytes| OverRelease {
                attempted_bytes,
                used_bytes,
            })
    }

    /// Charges up to `want` frames of `bytes_per_frame` bytes each; returns
    /// how many were granted (0 when not even one frame fits, or when
    /// `bytes_per_frame` is 0).
    fn try_charge_frames(&self, want: u64, bytes_per_frame: u64) -> u64 {
        if bytes_per_frame == 0 {
            return 0;
        }
        let mut granted = 0;
        let _ = self
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                let frames = (self.limit.saturating_sub(used) / bytes_per_frame).min(want);
                granted = frames;
                // `frames * bytes_per_frame <= limit - used`: neither the
                // product nor the sum can overflow; checked anyway (R11).
                if frames == 0 {
                    return None;
                }
                frames
                    .checked_mul(bytes_per_frame)
                    .and_then(|b| used.checked_add(b))
            });
        granted
    }

    /// Charges exactly `bytes`, all or nothing (a streamed track's ring,
    /// [`super::stream`]); returns whether the charge was made. Never
    /// exceeds the limit.
    pub(super) fn try_charge_exact(&self, bytes: u64) -> bool {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes).filter(|&total| total <= self.limit)
            })
            .is_ok()
    }

    /// Returns `bytes` charged by [`try_charge_exact`](Self::try_charge_exact).
    pub(super) fn refund_bytes(&self, bytes: u64) {
        self.refund_frames(bytes, 1);
    }

    /// Refunds frames of `bytes_per_frame` bytes a decode charged itself
    /// (unused capacity, or all of it on failure). They are always part of
    /// `used`, so the checked subtraction cannot fail; if it ever did, `used`
    /// is left unchanged.
    fn refund_frames(&self, frames: u64, bytes_per_frame: u64) {
        let bytes = frames.saturating_mul(bytes_per_frame);
        let result = self
            .used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_sub(bytes)
            });
        debug_assert!(result.is_ok(), "decode budget refund underflow");
    }
}

/// A [`DecodeBudget::release`] that asked to return more bytes than the
/// budget had charged; the budget was left unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverRelease {
    /// Bytes the release tried to return ([`Sample::bytes`]).
    pub attempted_bytes: u64,
    /// Bytes charged to the budget at the moment of the attempt.
    pub used_bytes: u64,
}

impl fmt::Display for OverRelease {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "decode budget over-release: tried to return {} bytes but only {} are charged",
            self.attempted_bytes, self.used_bytes
        )
    }
}

impl std::error::Error for OverRelease {}

impl Default for DecodeBudget {
    /// [`DecodeBudget::DEFAULT_BYTES`].
    fn default() -> Self {
        Self::new(Self::DEFAULT_BYTES)
    }
}

/// An immutable decoded sound at the mixer rate: one channel (mono, one
/// `f32` per frame) or two (interleaved stereo `[l0, r0, l1, r1, ...]`).
///
/// Mono is stored once and panned to both sides by the mixer, so it takes
/// half the memory (and half the [`DecodeBudget`] charge) of the same frames
/// in stereo; [`frame`](Self::frame) and [`to_stereo_vec`](Self::to_stereo_vec)
/// give the stereo view. Two samples are equal when their channel count and
/// data are equal (a mono sample differs from its duplicated stereo copy).
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    /// 1 or 2.
    channels: u8,
    /// `channels × frames` values, interleaved when stereo.
    data: Box<[f32]>,
}

impl Default for Sample {
    /// An empty stereo sample (0 frames, `channels() == 2`).
    fn default() -> Self {
        Self {
            channels: 2,
            data: Box::default(),
        }
    }
}

impl Sample {
    /// Wraps interleaved stereo data `[l0, r0, l1, r1, ...]`. A trailing odd
    /// value (half a frame) is discarded.
    pub fn from_interleaved(mut data: Vec<f32>) -> Self {
        if data.len() % 2 == 1 {
            data.pop();
        }
        Self {
            channels: 2,
            data: data.into_boxed_slice(),
        }
    }

    /// Builds a mono sample (one `f32` per frame) by copying `mono`.
    pub fn from_mono(mono: &[f32]) -> Self {
        Self::from_mono_vec(mono.to_vec())
    }

    /// Builds a mono sample (one `f32` per frame), taking ownership of
    /// `mono` without copying it.
    pub fn from_mono_vec(mono: Vec<f32>) -> Self {
        Self {
            channels: 1,
            data: mono.into_boxed_slice(),
        }
    }

    /// A constant ("DC") stereo sample of `frames` frames with `left`/`right`
    /// values; handy for exact mixing tests.
    pub fn constant(frames: usize, left: f32, right: f32) -> Self {
        let mut data = Vec::with_capacity(frames * 2);
        for _ in 0..frames {
            data.push(left);
            data.push(right);
        }
        Self::from_interleaved(data)
    }

    /// Stored channels: 1 (mono) or 2 (stereo). An empty
    /// [`Default`] sample reports 2.
    pub fn channels(&self) -> usize {
        usize::from(self.channels)
    }

    /// Number of frames (`data().len() / channels()`).
    pub fn frames(&self) -> usize {
        self.data.len() / self.channels()
    }

    /// The raw stored values, interleaved by [`channels`](Self::channels):
    /// `len() == channels() * frames()`. Mono data is one value per frame;
    /// use [`frame`](Self::frame) or [`to_stereo_vec`](Self::to_stereo_vec)
    /// for a stereo view.
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// Bytes of decoded data held (`4 × channels() × frames()`), what a
    /// [`DecodeBudget`] charges for this sample.
    pub fn bytes(&self) -> u64 {
        (self.data.len() as u64).saturating_mul(BYTES_PER_VALUE)
    }

    /// The stereo view `(left, right)` of frame `i`; a mono frame is
    /// `(v, v)`.
    ///
    /// # Panics
    /// When `i >= frames()`.
    pub fn frame(&self, i: usize) -> (f32, f32) {
        assert!(i < self.frames(), "frame {i} out of range");
        if self.channels == 1 {
            let v = self.data[i];
            (v, v)
        } else {
            (self.data[2 * i], self.data[2 * i + 1])
        }
    }

    /// An interleaved stereo copy `[l0, r0, l1, r1, ...]` (mono values are
    /// duplicated). Allocates: for tests and load time, never the audio
    /// thread.
    pub fn to_stereo_vec(&self) -> Vec<f32> {
        if self.channels == 1 {
            self.data.iter().flat_map(|&v| [v, v]).collect()
        } else {
            self.data.to_vec()
        }
    }
}

/// Why [`decode`] rejected a buffer. Non-exhaustive: match it with a
/// wildcard arm, more reasons may be added.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Not a WAV (`RIFF....WAVE`) or Ogg (`OggS`) buffer; an encoding inside
    /// one that is not supported (e.g. 64-bit float WAV, compressed WAV); a
    /// file sample rate outside [`MIN_SOURCE_RATE`]`..=`[`MAX_SOURCE_RATE`];
    /// an Ogg buffer with more than one logical stream (chained or
    /// multiplexed); or a mixer rate outside `1..=`[`MAX_MIXER_RATE`].
    Unsupported,
    /// The container or bitstream is invalid or truncated.
    Malformed(String),
    /// More than two channels.
    TooManyChannels,
    /// Longer than [`MAX_SECONDS`].
    TooLong,
    /// The bundle's [`DecodeBudget`] cannot hold the decoded output
    /// ([`decode_with_budget`]); nothing stays charged for this decode.
    BudgetExhausted,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported => f.write_str("unsupported audio format"),
            Self::Malformed(why) => write!(f, "malformed audio data: {why}"),
            Self::TooManyChannels => f.write_str("more than two audio channels"),
            Self::TooLong => write!(f, "audio longer than {MAX_SECONDS} seconds"),
            Self::BudgetExhausted => f.write_str("decoded-audio budget of the bundle exhausted"),
        }
    }
}

impl std::error::Error for DecodeError {}
/// Decodes a WAV or Ogg Vorbis buffer into a [`Sample`] at `mixer_rate`
/// (mono files stay mono, stereo stays stereo; see the module docs for the
/// exact rules). Never panics.
///
/// Only the per-file limits apply; a bundle loader decoding many files uses
/// [`decode_with_budget`].
pub fn decode(bytes: &[u8], mixer_rate: u32) -> Result<Sample, DecodeError> {
    decode_with_budget(bytes, mixer_rate, &DecodeBudget::unlimited())
}

/// [`decode`], charging the output to a bundle's shared [`budget`]
/// (module docs, "Memory"). Fails with [`DecodeError::BudgetExhausted`] when
/// the output does not fit; on any error nothing stays charged. Results are
/// identical to [`decode`] whenever the budget suffices.
///
/// [`budget`]: DecodeBudget
pub fn decode_with_budget(
    bytes: &[u8],
    mixer_rate: u32,
    budget: &DecodeBudget,
) -> Result<Sample, DecodeError> {
    run_decode(bytes, mixer_rate, |layout| {
        Ok(VecSink::new(
            layout.channels,
            layout.src,
            mixer_rate,
            layout.expected_in,
            budget,
        ))
    })
    .map(VecSink::into_sample)
}

/// Why the shared decode path stopped early, for sinks that can stop it on
/// their own (the streaming loader, [`super::stream`]).
#[derive(Debug)]
pub(super) enum Halt {
    /// The source is invalid or over a limit (same values as
    /// [`decode_with_budget`]).
    Failed(DecodeError),
    /// The sink asked to stop (track dropped or rewound, or a header probe).
    Stopped,
}

impl From<DecodeError> for Halt {
    fn from(e: DecodeError) -> Self {
        Halt::Failed(e)
    }
}

/// Where the shared decode path delivers output frames at the mixer rate.
pub(super) trait FrameSink {
    /// Error the sink can stop the decode with; decoder errors convert into
    /// it.
    type Error: From<DecodeError>;

    /// Takes the next output frame. A mono sink stores only `l` (`r == l`
    /// for mono sources).
    fn put(&mut self, l: f32, r: f32) -> Result<(), Self::Error>;
}

/// What a header announced, validated (channels 1 or 2, source rate in
/// range, length within the cap), handed to the sink factory of
/// [`run_decode`] before any audio is decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Layout {
    /// Stored channels, 1 or 2.
    pub channels: usize,
    /// Source (file) rate in Hz.
    pub src: u32,
    /// Source frames the header claims, bounded by what the bytes can hold
    /// for WAV (sizes the first reservation only; never trusted for more).
    pub expected_in: u64,
    /// Upper bound of the output length at the mixer rate when the header
    /// gives one (WAV data-chunk size, last Ogg granule position).
    pub max_out_frames: Option<u64>,
}

/// The one decode path: validates the mixer rate and the container, parses
/// and validates the header, asks `open` for a sink, then streams every
/// decoded source frame through the linear [`Resampling`] into that sink.
/// Returns the sink once the source has ended. Decoders run under
/// `catch_unwind`; a panic becomes [`DecodeError::Malformed`].
pub(super) fn run_decode<S: FrameSink>(
    bytes: &[u8],
    mixer_rate: u32,
    open: impl FnOnce(&Layout) -> Result<S, S::Error>,
) -> Result<S, S::Error> {
    if mixer_rate == 0 || mixer_rate > MAX_MIXER_RATE {
        return Err(DecodeError::Unsupported.into());
    }
    let is_wav = bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE";
    let is_ogg = bytes.len() >= 4 && &bytes[0..4] == b"OggS";
    if is_wav {
        guarded(|| decode_wav(bytes, mixer_rate, open))
    } else if is_ogg {
        guarded(|| decode_vorbis(bytes, mixer_rate, open))
    } else {
        Err(DecodeError::Unsupported.into())
    }
}

fn guarded<T, E: From<DecodeError>>(f: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
    catch_unwind(AssertUnwindSafe(f))
        .unwrap_or_else(|_| Err(DecodeError::Malformed("decoder panicked".into()).into()))
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

pub(super) fn max_frames(rate: u32) -> u64 {
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

/// Streaming linear resampler for 1 or 2 channels (module docs), shared by
/// the full decode and the streaming loader. Source frames are pushed one
/// at a time (mono pushes `(v, v)`; only the left value is used and
/// stored); output frame `j` is emitted to the sink as soon as both of its
/// source frames are known, so source frames are never buffered and the
/// length caps are enforced while decoding.
pub(super) struct Resampling<S> {
    /// Stored channels, 1 or 2.
    channels: usize,
    src: u64,
    dst: u64,
    /// Source frames pushed.
    n_in: u64,
    /// Next output frame.
    j: u64,
    prev: (f32, f32),
    last: (f32, f32),
    max_in: u64,
    max_out: u64,
    sink: S,
}

/// The full decode's resampler: output into a budget-charged [`VecSink`]
/// (built directly by the unit tests; [`run_decode`] builds it from the
/// header's [`Layout`]).
#[cfg(test)]
type Resampler<'b> = Resampling<VecSink<'b>>;

/// Budget bytes of one output frame with `channels` stored channels.
fn bytes_per_frame(channels: usize) -> u64 {
    if channels == 1 {
        BYTES_PER_MONO_FRAME
    } else {
        BYTES_PER_FRAME
    }
}

impl<S: FrameSink> Resampling<S> {
    /// A resampler from `src` to `dst` Hz feeding `sink`. `channels` is 1
    /// (mono output) or anything else (stereo output).
    fn with_sink(channels: usize, src: u32, dst: u32, sink: S) -> Self {
        Self {
            channels: if channels == 1 { 1 } else { 2 },
            src: u64::from(src),
            dst: u64::from(dst),
            n_in: 0,
            j: 0,
            prev: (0.0, 0.0),
            last: (0.0, 0.0),
            max_in: max_frames(src),
            max_out: max_frames(dst),
            sink,
        }
    }

    fn emit(&mut self, a: (f32, f32), b: (f32, f32), f: f64) -> Result<(), S::Error> {
        if self.j >= self.max_out {
            return Err(DecodeError::TooLong.into());
        }
        let l = lerp(a.0, b.0, f);
        let r = if self.channels == 2 {
            lerp(a.1, b.1, f)
        } else {
            l
        };
        self.sink.put(l, r)?;
        self.j += 1;
        Ok(())
    }

    fn push(&mut self, l: f32, r: f32) -> Result<(), S::Error> {
        if self.n_in >= self.max_in {
            return Err(DecodeError::TooLong.into());
        }
        self.n_in += 1;
        if self.src == self.dst {
            if self.j >= self.max_out {
                return Err(DecodeError::TooLong.into());
            }
            self.sink.put(l, if self.channels == 2 { r } else { l })?;
            self.j += 1;
            return Ok(());
        }
        self.prev = self.last;
        self.last = (l, r);
        // Source frame k = n_in - 1 just arrived: every pending output frame
        // with i = floor(j·src/dst) = k - 1 can now be interpolated (none
        // when k = 0).
        let k = self.n_in - 1;
        loop {
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

    /// Emits the tail (output frames past the last source frame) and returns
    /// the sink.
    fn flush(mut self) -> Result<S, S::Error> {
        if self.src != self.dst && self.n_in > 0 {
            // Remaining frames have i ≥ n_in − 1: both neighbours clamp to the
            // last source frame.
            let n_out = resampled_len(self.n_in, self.src as u32, self.dst as u32);
            while self.j < n_out {
                let f = ((self.j * self.src) % self.dst) as f64 / self.dst as f64;
                self.emit(self.last, self.last, f)?;
            }
        }
        Ok(self.sink)
    }
}

#[cfg(test)]
impl<'b> Resampler<'b> {
    /// A full-decode resampler whose output is charged to `budget`
    /// ([`VecSink`]). `expected_in` (a header claim) only sizes the first
    /// reservation, which never exceeds [`INITIAL_RESERVE_FRAMES`].
    fn new(
        channels: usize,
        src: u32,
        dst: u32,
        expected_in: u64,
        budget: &'b DecodeBudget,
    ) -> Self {
        Self::with_sink(
            channels,
            src,
            dst,
            VecSink::new(channels, src, dst, expected_in, budget),
        )
    }

    fn finish(self) -> Result<Sample, DecodeError> {
        self.flush().map(VecSink::into_sample)
    }
}

/// The full decode's output buffer. It grows on demand and every frame of
/// its capacity is charged to the budget first; dropping it without
/// [`VecSink::into_sample`] refunds the charge.
pub(super) struct VecSink<'b> {
    /// Stored channels, 1 or 2.
    channels: usize,
    out: Vec<f32>,
    /// Frames stored.
    frames: u64,
    max_out: u64,
    budget: &'b DecodeBudget,
    /// Frames charged to `budget` (= output capacity in frames).
    charged: u64,
}

impl<'b> VecSink<'b> {
    fn new(
        channels: usize,
        src: u32,
        dst: u32,
        expected_in: u64,
        budget: &'b DecodeBudget,
    ) -> Self {
        let mut sink = Self {
            channels: if channels == 1 { 1 } else { 2 },
            out: Vec::new(),
            frames: 0,
            max_out: max_frames(dst),
            budget,
            charged: 0,
        };
        let first = resampled_len(expected_in, src, dst)
            .min(sink.max_out)
            .min(INITIAL_RESERVE_FRAMES);
        sink.reserve(first);
        sink
    }

    /// Charges up to `frames` more frames and reserves exactly what was
    /// granted; returns the number granted.
    fn reserve(&mut self, frames: u64) -> u64 {
        if frames == 0 {
            return 0;
        }
        let granted = self
            .budget
            .try_charge_frames(frames, bytes_per_frame(self.channels));
        if granted > 0 {
            self.charged += granted;
            // `granted` ≤ the 10-minute cap (≤ 2^28 frames): fits in usize.
            self.out.reserve_exact(self.channels * granted as usize);
        }
        granted
    }

    /// Makes room for one more output frame (called after the length caps).
    fn room(&mut self) -> Result<(), DecodeError> {
        if self.frames < self.charged {
            return Ok(());
        }
        let want = self
            .frames
            .max(MIN_GROW_FRAMES)
            .min(self.max_out.saturating_sub(self.frames));
        if self.reserve(want) == 0 {
            return Err(DecodeError::BudgetExhausted);
        }
        Ok(())
    }

    /// The stored frames as a [`Sample`], keeping exactly them charged.
    fn into_sample(mut self) -> Sample {
        self.budget
            .refund_frames(self.charged - self.frames, bytes_per_frame(self.channels));
        self.charged = 0;
        let out = std::mem::take(&mut self.out);
        if self.channels == 1 {
            Sample::from_mono_vec(out)
        } else {
            Sample::from_interleaved(out)
        }
    }
}

impl FrameSink for VecSink<'_> {
    type Error = DecodeError;

    fn put(&mut self, l: f32, r: f32) -> Result<(), DecodeError> {
        self.room()?;
        self.out.push(l);
        if self.channels == 2 {
            self.out.push(r);
        }
        self.frames += 1;
        Ok(())
    }
}

impl Drop for VecSink<'_> {
    /// An unfinished decode (error or panic) leaves nothing charged.
    fn drop(&mut self) {
        self.budget
            .refund_frames(self.charged, bytes_per_frame(self.channels));
    }
}

fn map_hound(e: hound::Error) -> DecodeError {
    match e {
        hound::Error::Unsupported => DecodeError::Unsupported,
        other => malformed(other),
    }
}

fn decode_wav<S: FrameSink>(
    bytes: &[u8],
    dst: u32,
    open: impl FnOnce(&Layout) -> Result<S, S::Error>,
) -> Result<S, S::Error> {
    let reader = hound::WavReader::new(Cursor::new(bytes)).map_err(map_hound)?;
    let spec = reader.spec();
    let channels = usize::from(spec.channels);
    check_rate_and_channels(spec.sample_rate, channels)?;
    let frames = u64::from(reader.duration());
    check_length(frames, spec.sample_rate, dst)?;
    match spec.sample_format {
        hound::SampleFormat::Float if spec.bits_per_sample != 32 => {
            return Err(DecodeError::Unsupported.into());
        }
        hound::SampleFormat::Int if spec.bits_per_sample == 0 || spec.bits_per_sample > 32 => {
            return Err(DecodeError::Unsupported.into());
        }
        _ => {}
    }
    // Never trust the header beyond what the bytes can hold (and the
    // resampler caps the first reservation anyway).
    let bytes_per_frame = (usize::from(spec.bits_per_sample.div_ceil(8)).max(1) * channels) as u64;
    let expected = frames.min(bytes.len() as u64 / bytes_per_frame);
    let layout = Layout {
        channels,
        src: spec.sample_rate,
        expected_in: expected,
        max_out_frames: Some(resampled_len(expected, spec.sample_rate, dst)),
    };
    let sink = open(&layout)?;
    let mut rs = Resampling::with_sink(channels, spec.sample_rate, dst, sink);
    let mut pending: Option<f32> = None;
    let mut feed = |v: f32| -> Result<(), S::Error> {
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
            for s in reader.into_samples::<f32>() {
                feed(s.map_err(map_hound)?)?;
            }
        }
        hound::SampleFormat::Int => {
            let scale = f64::from(1u32 << (spec.bits_per_sample - 1));
            for s in reader.into_samples::<i32>() {
                let v = s.map_err(map_hound)?;
                feed((f64::from(v) / scale) as f32)?;
            }
        }
    }
    rs.flush()
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
        let body: usize = b[p + 27..p + 27 + segs]
            .iter()
            .map(|&x| usize::from(x))
            .sum();
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

fn decode_vorbis<S: FrameSink>(
    bytes: &[u8],
    dst: u32,
    open: impl FnOnce(&Layout) -> Result<S, S::Error>,
) -> Result<S, S::Error> {
    use lewton::audio::{PreviousWindowRight, read_audio_packet_generic};
    use lewton::header::{read_header_ident, read_header_setup};
    use lewton::samples::InterleavedSamples;

    let scan = scan_ogg_pages(bytes);
    if scan.other_serial {
        // Chained or multiplexed: each link restarts its granule positions,
        // so the length cannot be bounded up front.
        return Err(DecodeError::Unsupported.into());
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
    // The granule position is a claim: it only sizes the first (capped)
    // reservation and bounds the output length.
    let layout = Layout {
        channels,
        src,
        expected_in: scan.last_granule.unwrap_or(0),
        max_out_frames: scan.last_granule.map(|g| resampled_len(g, src, dst)),
    };
    let sink = open(&layout)?;
    let mut rs = Resampling::with_sink(channels, src, dst, sink);
    let mut pwr = PreviousWindowRight::new();
    'packets: while let Some(packet) = rdr.read_packet().map_err(malformed)? {
        if packet.stream_serial() != scan.serial {
            return Err(DecodeError::Unsupported.into());
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
    rs.flush()
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
            assert_eq!(s.channels(), 1);
            let d = s.to_stereo_vec();
            let scale = f64::from(1u32 << (bits - 1));
            for (k, &v) in vals.iter().enumerate() {
                let e = (f64::from(v) / scale) as f32;
                assert_eq!(d[2 * k], e, "bits {bits}");
                assert_eq!(d[2 * k + 1], e, "bits {bits}");
            }
            assert_eq!(d[0], -1.0);
            assert_eq!(d[4], 0.5);
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
        let l: Vec<f32> = s.to_stereo_vec().iter().step_by(2).copied().collect();
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
        assert!(matches!(
            decode(&bytes, 8_000),
            Err(DecodeError::Malformed(_))
        ));
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

    static UNLIMITED: DecodeBudget = DecodeBudget {
        limit: u64::MAX,
        used: AtomicU64::new(0),
    };

    fn stream(x: &[(f32, f32)], src: u32, dst: u32) -> Vec<f32> {
        let mut rs = Resampler::new(2, src, dst, x.len() as u64, &UNLIMITED);
        for &(l, r) in x {
            rs.push(l, r).unwrap();
        }
        rs.finish().unwrap().data().to_vec()
    }

    /// The mono resampler on `x`, returned as its stereo view.
    fn stream_mono(x: &[f32], src: u32, dst: u32) -> Sample {
        let mut rs = Resampler::new(1, src, dst, x.len() as u64, &UNLIMITED);
        for &v in x {
            rs.push(v, v).unwrap();
        }
        rs.finish().unwrap()
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

        /// SPEC §4 Subphase 3.3: resampling a mono channel once gives, bit
        /// for bit, either side of the duplicated-stereo resample, stored as
        /// one value per frame.
        #[test]
        fn mono_resampler_matches_duplicated_stereo(
            x in proptest::collection::vec(-1.0f32..1.0, 0..300),
            src in proptest::sample::select(vec![8_000u32, 11_025, 22_050, 44_100, 48_000, 96_000, 384_000]),
            dst in proptest::sample::select(vec![8_000u32, 22_050, 44_100, 48_000, 192_000]),
        ) {
            let dup: Vec<(f32, f32)> = x.iter().map(|&v| (v, v)).collect();
            let m = stream_mono(&x, src, dst);
            proptest::prop_assert_eq!(m.channels(), 1);
            proptest::prop_assert_eq!(m.data().len(), m.frames());
            let want = reference(&dup, src, dst);
            let got = m.to_stereo_vec();
            proptest::prop_assert_eq!(got.len(), want.len());
            for (a, b) in got.iter().zip(&want) {
                proptest::prop_assert_eq!(a.to_bits(), b.to_bits());
            }
        }

        /// D25: the private charge helper never grants more than fits and
        /// never overflows, whatever the limit, use and frame size.
        #[test]
        fn charge_helper_is_bounded(
            limit in proptest::num::u64::ANY,
            pre in proptest::num::u64::ANY,
            want in proptest::num::u64::ANY,
            bpf in proptest::sample::select(vec![0u64, 1, BYTES_PER_MONO_FRAME, BYTES_PER_FRAME, u64::MAX]),
        ) {
            let b = DecodeBudget::new(limit);
            b.used.store(pre.min(limit), Ordering::Release);
            let before = b.used_bytes();
            let g = b.try_charge_frames(want, bpf);
            proptest::prop_assert!(g <= want);
            proptest::prop_assert!(b.used_bytes() <= limit);
            if bpf == 0 {
                proptest::prop_assert_eq!(g, 0);
            } else {
                proptest::prop_assert_eq!(b.used_bytes() - before, g * bpf);
                proptest::prop_assert!(g == want || (limit - b.used_bytes()) < bpf);
            }
            b.refund_frames(g, bpf);
            proptest::prop_assert_eq!(b.used_bytes(), before);
        }
    }

    #[test]
    fn resampler_caps_while_streaming() {
        let unlimited = DecodeBudget::unlimited();
        let mut rs = Resampler::new(2, 8_000, 48_000, 10, &unlimited);
        rs.max_out = 11;
        assert_eq!(rs.push(0.0, 0.0), Ok(()));
        assert_eq!(rs.push(0.0, 0.0), Ok(())); // 6 out
        assert_eq!(rs.push(0.0, 0.0), Err(DecodeError::TooLong)); // 12th
        let mut rs = Resampler::new(2, 8_000, 8_000, 0, &unlimited);
        rs.max_in = 2;
        rs.push(0.0, 0.0).unwrap();
        rs.push(0.0, 0.0).unwrap();
        assert_eq!(rs.push(0.0, 0.0), Err(DecodeError::TooLong));
        let mut rs = Resampler::new(2, 8_000, 8_000, 0, &unlimited);
        rs.max_out = 1;
        rs.push(0.0, 0.0).unwrap();
        assert_eq!(rs.push(0.0, 0.0), Err(DecodeError::TooLong));
        let mut rs = Resampler::new(2, 8_000, 16_000, 0, &unlimited);
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
        // Mono → stored as one channel; the stereo view has identical sides.
        assert_eq!(s.channels(), 1);
        let stereo = s.to_stereo_vec();
        let peak = stereo.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!((0.4..=0.6).contains(&peak), "peak {peak}");
        assert!(stereo.chunks(2).all(|c| c[0] == c[1]));
        // Roughly 440 Hz: count positive-going zero crossings in the middle.
        let left: Vec<f32> = stereo.iter().step_by(2).copied().collect();
        let mid = &left[2_400..2_400 + 4_800]; // 0.1 s
        let ups = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((42..=46).contains(&ups), "crossings {ups}");
        // Native rate keeps the length.
        let native = decode(OGG, 22_050).unwrap();
        assert!(native.frames().abs_diff(5_513) <= 1);
        assert_eq!(native.channels(), 1);
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

    /// D25: a checked release succeeds exactly, refuses an over-release and
    /// leaves `used` unchanged; a second release of the same sample fails.
    #[test]
    fn release_is_checked() {
        let budget = DecodeBudget::new(1 << 20);
        let wav_bytes = wav(spec(1, 48_000, 16, false), |w| {
            for _ in 0..100 {
                w.write_sample(0i16).unwrap();
            }
        });
        // Mono WAV: 4 bytes per frame (#227).
        let a = decode_with_budget(&wav_bytes, 48_000, &budget).unwrap();
        let b = decode_with_budget(&wav_bytes, 48_000, &budget).unwrap();
        assert_eq!(budget.used_bytes(), 2 * 100 * BYTES_PER_MONO_FRAME);
        assert_eq!(budget.release(&a), Ok(()));
        assert_eq!(budget.used_bytes(), 100 * BYTES_PER_MONO_FRAME);
        // Over-release: a sample larger than what is charged.
        let big = Sample::from_mono(&[0.0; 101]);
        assert_eq!(
            budget.release(&big),
            Err(OverRelease {
                attempted_bytes: 101 * BYTES_PER_MONO_FRAME,
                used_bytes: 100 * BYTES_PER_MONO_FRAME,
            })
        );
        assert_eq!(budget.used_bytes(), 100 * BYTES_PER_MONO_FRAME);
        assert_eq!(budget.release(&b), Ok(()));
        assert_eq!(budget.used_bytes(), 0);
        // Second release of the same sample.
        let err = budget.release(&b).unwrap_err();
        assert_eq!(err.attempted_bytes, 100 * BYTES_PER_MONO_FRAME);
        assert_eq!(err.used_bytes, 0);
        assert_eq!(budget.used_bytes(), 0);
        assert!(err.to_string().contains("400"));
        // An empty sample always releases.
        assert_eq!(budget.release(&Sample::default()), Ok(()));
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig { cases: 64, ..Default::default() })]
        /// D25: after every returned sample is released exactly once, the
        /// budget is back to 0, whatever the order and the failures.
        #[test]
        fn used_returns_to_zero(
            lens in proptest::collection::vec(0usize..3000, 1..8),
            limit_frames in 0u64..12_000,
            order_seed in 0u64..1000,
        ) {
            let budget = DecodeBudget::new(limit_frames * BYTES_PER_FRAME);
            let mut got = Vec::new();
            for n in lens {
                let bytes = wav(spec(1, 48_000, 16, false), |w| {
                    for _ in 0..n {
                        w.write_sample(0i16).unwrap();
                    }
                });
                match decode_with_budget(&bytes, 48_000, &budget) {
                    Ok(s) => got.push(s),
                    Err(e) => proptest::prop_assert_eq!(e, DecodeError::BudgetExhausted),
                }
            }
            let sum: u64 = got.iter().map(Sample::bytes).sum();
            proptest::prop_assert_eq!(budget.used_bytes(), sum);
            let k = got.len();
            if k > 0 {
                got.rotate_left((order_seed as usize) % k);
            }
            for s in &got {
                proptest::prop_assert_eq!(budget.release(s), Ok(()));
            }
            proptest::prop_assert_eq!(budget.used_bytes(), 0);
        }
    }

    #[test]
    fn sample_constructors() {
        let s = Sample::from_interleaved(vec![1.0, 2.0, 3.0]);
        assert_eq!(s.frames(), 1);
        assert_eq!(Sample::from_mono(&[0.5]).data(), &[0.5]);
        assert_eq!(Sample::from_mono(&[0.5]).to_stereo_vec(), vec![0.5, 0.5]);
        assert_eq!(Sample::constant(2, 0.1, 0.2).data(), &[0.1, 0.2, 0.1, 0.2]);
        assert_eq!(Sample::default().frames(), 0);
    }

    fn dup(x: &[f32]) -> Vec<f32> {
        x.iter().flat_map(|&v| [v, v]).collect()
    }

    /// SPEC §4 Subphase 3.3 (#227): a mono sample stores one value per frame,
    /// half the memory (and budget bytes) of the same frames in stereo.
    #[test]
    fn mono_sample_is_half_the_memory() {
        for n in [0usize, 1, 2, 7, 256, 1000] {
            let x: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin()).collect();
            let m = Sample::from_mono(&x);
            let s = Sample::from_interleaved(dup(&x));
            assert_eq!(m.frames(), n);
            assert_eq!(s.frames(), n);
            assert_eq!(m.channels(), 1);
            assert_eq!(s.channels(), 2);
            assert_eq!(m.bytes() * 2, s.bytes(), "n {n}");
            assert_eq!(m.data().len() * 2, s.data().len(), "n {n}");
            assert_eq!(m.bytes(), n as u64 * BYTES_PER_MONO_FRAME);
            assert_eq!(s.bytes(), n as u64 * BYTES_PER_FRAME);
            assert_eq!(m.to_stereo_vec(), s.to_stereo_vec());
            assert_eq!(s.to_stereo_vec(), s.data());
            for (i, &v) in x.iter().enumerate() {
                assert_eq!(m.frame(i), s.frame(i));
                assert_eq!(m.frame(i), (v, v));
            }
            assert_eq!(Sample::from_mono_vec(x.clone()), m);
            assert_ne!(m, s, "mono and stereo storage differ");
        }
        let d = Sample::default();
        assert_eq!((d.channels(), d.frames(), d.bytes()), (2, 0, 0));
        let c = Sample::constant(3, 0.25, -0.5);
        assert_eq!(c.frame(2), (0.25, -0.5));
        assert_eq!(c.bytes(), 3 * BYTES_PER_FRAME);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn frame_out_of_range_panics_mono() {
        let _ = Sample::from_mono(&[1.0, 2.0]).frame(2);
    }

    #[test]
    #[should_panic(expected = "out of range")]
    fn frame_out_of_range_panics_stereo() {
        let _ = Sample::constant(2, 1.0, 2.0).frame(2);
    }

    /// The pre-#227 decode result of a mono WAV: every value duplicated.
    fn mono_wav(rate: u32, vals: &[i16]) -> Vec<u8> {
        wav(spec(1, rate, 16, false), |w| {
            for &v in vals {
                w.write_sample(v).unwrap();
            }
        })
    }

    /// SPEC §4 Subphase 3.3, D25 (#227): a mono WAV decodes to one channel
    /// whose stereo view equals the old duplicated-stereo decode, on the
    /// same-rate and the resampled path; a stereo WAV stays stereo.
    #[test]
    fn mono_wav_stays_mono() {
        let vals: Vec<i16> = (0..500)
            .map(|i| ((i * 977) % 65_536 - 32_768) as i16)
            .collect();
        let x: Vec<(f32, f32)> = vals
            .iter()
            .map(|&v| {
                let f = (f64::from(v) / 32768.0) as f32;
                (f, f)
            })
            .collect();
        for (src, dst) in [(48_000u32, 48_000u32), (22_050, 48_000), (96_000, 44_100)] {
            let s = decode(&mono_wav(src, &vals), dst).unwrap();
            assert_eq!(s.channels(), 1, "{src}->{dst}");
            assert_eq!(s.data().len(), s.frames());
            let want = reference(&x, src, dst);
            let got = s.to_stereo_vec();
            assert_eq!(got.len(), want.len());
            assert!(
                got.iter()
                    .zip(&want)
                    .all(|(a, b)| a.to_bits() == b.to_bits()),
                "{src}->{dst}"
            );
        }
        let st = wav(spec(2, 44_100, 16, false), |w| {
            for v in [1i16, -1, 2, -2] {
                w.write_sample(v).unwrap();
            }
        });
        let s = decode(&st, 48_000).unwrap();
        assert_eq!(s.channels(), 2);
        assert_eq!(s.data().len(), 2 * s.frames());
    }

    /// D25 (#227): mono output is charged 4 bytes per frame and stereo 8;
    /// a budget that fits N mono frames refuses N + 1 and keeps nothing
    /// charged on failure.
    #[test]
    fn budget_charges_per_channel() {
        let budget = DecodeBudget::new(1 << 20);
        let m = decode_with_budget(&mono_wav(48_000, &[1; 300]), 48_000, &budget).unwrap();
        assert_eq!(m.channels(), 1);
        assert_eq!(budget.used_bytes(), 300 * BYTES_PER_MONO_FRAME);
        assert_eq!(budget.used_bytes(), m.bytes());
        let st = wav(spec(2, 48_000, 16, false), |w| {
            for _ in 0..300 {
                w.write_sample(1i16).unwrap();
                w.write_sample(-1i16).unwrap();
            }
        });
        let s = decode_with_budget(&st, 48_000, &budget).unwrap();
        assert_eq!(s.channels(), 2);
        assert_eq!(
            budget.used_bytes(),
            300 * BYTES_PER_MONO_FRAME + 300 * BYTES_PER_FRAME
        );
        budget.release(&m).unwrap();
        budget.release(&s).unwrap();
        assert_eq!(budget.used_bytes(), 0);
        assert!(budget.release(&m).is_err());

        // Exactly N mono frames fit (resampled path too); N + 1 do not.
        for (src, n_in) in [(48_000u32, 2_000usize), (24_000, 1_000)] {
            let n = resampled_len(n_in as u64, src, 48_000);
            let fits = DecodeBudget::new(n * BYTES_PER_MONO_FRAME);
            let ok = decode_with_budget(&mono_wav(src, &vec![7; n_in]), 48_000, &fits).unwrap();
            assert_eq!(ok.frames() as u64, n);
            assert_eq!(fits.used_bytes(), n * BYTES_PER_MONO_FRAME);
            assert_eq!(fits.remaining_bytes(), 0);
            let short = DecodeBudget::new(n * BYTES_PER_MONO_FRAME);
            assert_eq!(
                decode_with_budget(&mono_wav(src, &vec![7; n_in + 1]), 48_000, &short),
                Err(DecodeError::BudgetExhausted)
            );
            assert_eq!(short.used_bytes(), 0);
        }
        // A failing (truncated) mono decode leaves nothing charged.
        let mut cut = mono_wav(48_000, &[3; 5_000]);
        cut.truncate(cut.len() - 1_001);
        let b = DecodeBudget::new(1 << 20);
        assert!(matches!(
            decode_with_budget(&cut, 48_000, &b),
            Err(DecodeError::Malformed(_))
        ));
        assert_eq!(b.used_bytes(), 0);
    }

    /// SPEC §4 Subphase 3.3, D25 (#227): the mono Vorbis fixture decodes to
    /// one channel, its stereo view equals the duplicated decode, and the
    /// budget holds exactly its bytes.
    #[test]
    fn mono_vorbis_stays_mono() {
        for dst in [22_050u32, 48_000] {
            let budget = DecodeBudget::new(1 << 20);
            let s = decode_with_budget(OGG, dst, &budget).unwrap();
            assert_eq!(s.channels(), 1);
            assert_eq!(
                budget.used_bytes(),
                s.frames() as u64 * BYTES_PER_MONO_FRAME
            );
            let st = s.to_stereo_vec();
            assert_eq!(st.len(), 2 * s.frames());
            for i in 0..s.frames() {
                assert_eq!(s.frame(i), (st[2 * i], st[2 * i + 1]));
                assert_eq!(s.data()[i].to_bits(), st[2 * i + 1].to_bits());
            }
        }
    }
}
