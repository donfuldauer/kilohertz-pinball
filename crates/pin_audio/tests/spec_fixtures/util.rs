//! Shared helpers for the Subphase 3.3 spec tests (Independent Tester).
//!
//! Everything here uses only the brief's FIXED API; WAV files are written by
//! hand (no `hound`) so the fixtures are independent of the decoder under test.
#![allow(dead_code)]

use pin_audio::mixer::{
    Bus, DecodeError, Mixer, MixerConfig, Sample, SoundSender, SoundTrigger, decode, trigger_ring,
};
use pin_core::SoundId;
use std::sync::Arc;

/// Mixer rate used by most tests (the brief's default).
pub const RATE: u32 = 48_000;

/// WAVE_FORMAT_PCM.
pub const TAG_PCM: u16 = 1;
/// WAVE_FORMAT_IEEE_FLOAT.
pub const TAG_FLOAT: u16 = 3;

/// A canonical RIFF/WAVE file: 16-byte `fmt ` chunk followed by `data`.
pub fn wav_bytes(tag: u16, bits: u16, channels: u16, rate: u32, data: &[u8]) -> Vec<u8> {
    let block = channels * (bits / 8);
    let mut v = Vec::with_capacity(44 + data.len() + 1);
    v.extend_from_slice(b"RIFF");
    let pad = data.len() % 2;
    v.extend_from_slice(&((36 + data.len() + pad) as u32).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&tag.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&rate.wrapping_mul(u32::from(block)).to_le_bytes());
    v.extend_from_slice(&block.to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(data);
    if pad == 1 {
        v.push(0);
    }
    v
}

/// 32-bit float WAV from interleaved samples.
pub fn wav_f32(channels: u16, rate: u32, samples: &[f32]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    wav_bytes(TAG_FLOAT, 32, channels, rate, &data)
}

/// Unsigned 8-bit PCM WAV from interleaved samples.
pub fn wav_u8(channels: u16, rate: u32, samples: &[u8]) -> Vec<u8> {
    wav_bytes(TAG_PCM, 8, channels, rate, samples)
}

/// Signed 16-bit PCM WAV from interleaved samples.
pub fn wav_i16(channels: u16, rate: u32, samples: &[i16]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    wav_bytes(TAG_PCM, 16, channels, rate, &data)
}

/// Signed 24-bit PCM WAV from interleaved samples (low 24 bits of each i32).
pub fn wav_i24(channels: u16, rate: u32, samples: &[i32]) -> Vec<u8> {
    let data: Vec<u8> = samples
        .iter()
        .flat_map(|s| {
            let b = s.to_le_bytes();
            [b[0], b[1], b[2]]
        })
        .collect();
    wav_bytes(TAG_PCM, 24, channels, rate, &data)
}

/// Signed 32-bit PCM WAV from interleaved samples.
pub fn wav_i32(channels: u16, rate: u32, samples: &[i32]) -> Vec<u8> {
    let data: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    wav_bytes(TAG_PCM, 32, channels, rate, &data)
}

/// A `Sample` holding exactly `stereo` (interleaved L,R) at `rate`: built by
/// decoding a float WAV at its own rate, the only public way to make one.
pub fn sample_from(stereo: &[f32], rate: u32) -> Arc<Sample> {
    assert!(stereo.len().is_multiple_of(2));
    let s = decoded(decode(&wav_f32(2, rate, stereo), rate));
    assert_eq!(
        s.frames(),
        stereo.len() / 2,
        "same-rate decode keeps length"
    );
    assert_eq!(s.data(), stereo, "same-rate float decode is exact");
    Arc::new(s)
}

/// Unwraps a decode result using only `Display` (the brief does not promise
/// `Debug` on `Sample` or `DecodeError`).
#[track_caller]
pub fn decoded(r: Result<Sample, DecodeError>) -> Sample {
    match r {
        Ok(s) => s,
        Err(e) => panic!("decode failed: {e}"),
    }
}

/// A constant (DC) stereo sample of `frames` frames.
pub fn dc(left: f32, right: f32, frames: usize, rate: u32) -> Arc<Sample> {
    let mut v = Vec::with_capacity(frames * 2);
    for _ in 0..frames {
        v.push(left);
        v.push(right);
    }
    sample_from(&v, rate)
}

/// A mixer plus the sending end of its trigger ring.
pub struct Rig {
    pub mixer: Mixer,
    pub tx: SoundSender,
}

impl Rig {
    pub fn new(cfg: MixerConfig) -> Self {
        Self::with_ring(cfg, 64)
    }

    pub fn with_ring(cfg: MixerConfig, capacity: usize) -> Self {
        let (tx, rx) = trigger_ring(capacity);
        Self {
            mixer: Mixer::new(cfg, rx),
            tx,
        }
    }

    pub fn reg(&mut self, id: u16, sample: &Arc<Sample>, bus: Bus) {
        self.mixer.register(SoundId(id), Arc::clone(sample), bus);
    }

    /// Sends a trigger; panics if the ring is full.
    pub fn play(&mut self, id: u16, gain: f32) {
        assert!(
            self.tx.send(SoundTrigger {
                sound: SoundId(id),
                gain
            }),
            "trigger ring unexpectedly full"
        );
    }

    /// Renders `frames` frames in one call and returns the interleaved output.
    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        // Poisoned with a non-zero value: render must overwrite, not accumulate.
        let mut out = vec![0.777_f32; frames * 2];
        self.mixer.render(&mut out);
        out
    }

    /// Renders `frames` frames in calls of at most `chunk` frames.
    pub fn render_chunked(&mut self, frames: usize, chunk: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        let mut left = frames;
        while left > 0 {
            let n = left.min(chunk);
            out.extend_from_slice(&self.render(n));
            left -= n;
        }
        out
    }
}

/// `|a - b| <= tol`, with a readable failure message.
#[track_caller]
pub fn assert_close(a: f32, b: f32, tol: f32, what: &str) {
    assert!(
        (a - b).abs() <= tol,
        "{what}: got {a}, expected {b} (tolerance {tol})"
    );
}

/// Asserts every frame of `out` is `(left, right)` within `tol`.
#[track_caller]
pub fn assert_all_frames(out: &[f32], left: f32, right: f32, tol: f32, what: &str) {
    for (i, f) in out.chunks_exact(2).enumerate() {
        assert!(
            (f[0] - left).abs() <= tol && (f[1] - right).abs() <= tol,
            "{what}: frame {i} is ({}, {}), expected ({left}, {right})",
            f[0],
            f[1]
        );
    }
}

/// Small deterministic PRNG (xorshift64*), for tests that must not depend on
/// `proptest`.
pub struct XorShift(pub u64);

impl XorShift {
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next_u64() >> 32) as u8).collect()
    }
}

/// The committed Ogg Vorbis fixtures (see `README.md` for provenance).
pub const OGG_MONO_48K: &[u8] = include_bytes!("sine440_mono_48k.ogg");
pub const OGG_LEFT_44K1: &[u8] = include_bytes!("sine880_left_44k1.ogg");
