//! Spec tests (Independent Tester, #130 / KP-049): music streamed from the
//! compressed file through a bounded ring.
//!
//! Black-box tests written from the brief's PINNED PUBLIC API only:
//! `open_stream`, `StreamedTrack`, `StreamHandle`, `Mixer::register_stream`
//! and `Mixer::stream_underruns`, compared against the merged resident path
//! (`decode` + `Mixer::register`).
//!
//! Traceability:
//! * SPEC 3.3 (mixer) and #130 AC1: [`ten_minute_stream_equals_full_decode`],
//!   [`stereo_streams_equal_full_decode`], [`ogg_streams_equal_full_decode`],
//!   [`boundary_lengths_equal_full_decode`], [`rewind_after_end_and_after_steal`],
//!   [`second_trigger_is_ignored_while_playing`], [`zero_length_track`],
//!   [`underrun_invariants`].
//! * Budget (D25, "Human ruling - decoded-audio budget"): [`budget_charge_and_release`],
//!   [`header_errors_match_decode`], the 1 MiB part of the AC1 test.
//! * D5 (no allocation on the audio render path): [`render_does_not_allocate`]
//!   and the per-render count in the AC1 test.
//! * D25 / R11 (hostile input): [`hostile_bytes_and_corruption`].
//!
//! The global allocator keeps a per-thread allocation counter (D5: only the
//! test thread, which plays the audio thread, is counted; the loader thread
//! is not) and a global live-bytes / peak tracker (S5 memory bound, all
//! threads). Every test holds one static lock so the tracker is not polluted
//! by tests running in parallel. Simulated time only (R6): the tests never
//! sleep; they synchronise with `StreamHandle::wait_filled`.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use pin_audio::mixer::{
    Bus, DecodeBudget, DecodeError, Mixer, MixerConfig, STREAM_RING_FRAMES, Sample, SoundSender,
    SoundTrigger, StreamHandle, StreamedTrack, decode, decode_with_budget, open_stream,
    trigger_ring,
};
use pin_core::SoundId;
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};

// =============================================================== allocator

/// Counting + live-bytes tracking allocator.
struct Tracking;

thread_local! {
    /// Allocation calls (alloc, alloc_zeroed, realloc, dealloc) on this thread.
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

/// Live heap bytes over all threads.
static LIVE: AtomicUsize = AtomicUsize::new(0);
/// Peak of [`LIVE`] since the last [`reset_peak`].
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn bump() {
    let _ = COUNT.try_with(|c| c.set(c.get() + 1));
}

fn grow(n: usize) {
    let now = LIVE.fetch_add(n, Ordering::SeqCst) + n;
    PEAK.fetch_max(now, Ordering::SeqCst);
}

fn shrink(n: usize) {
    LIVE.fetch_sub(n, Ordering::SeqCst);
}

unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        bump();
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            grow(l.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        bump();
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            grow(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        bump();
        unsafe { System.dealloc(p, l) };
        shrink(l.size());
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        bump();
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() {
            // Conservative: old and new block are both counted for an instant.
            grow(n);
            shrink(l.size());
        }
        q
    }
}

#[global_allocator]
static A: Tracking = Tracking;

/// Allocation calls made by this thread so far.
fn allocs_here() -> u64 {
    COUNT.with(Cell::get)
}

/// Sets the peak to the current live bytes and returns that baseline.
fn reset_peak() -> usize {
    let live = LIVE.load(Ordering::SeqCst);
    PEAK.store(live, Ordering::SeqCst);
    live
}

fn peak() -> usize {
    PEAK.load(Ordering::SeqCst)
}

// =============================================================== serialisation

static SERIAL: Mutex<()> = Mutex::new(());

/// One test at a time in this binary (poisoning ignored), so the global
/// live-bytes tracker only sees the running test.
fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

// =============================================================== fixtures

/// Mixer rate used by most tests.
const RATE: u32 = 48_000;
const MUSIC: u16 = 1;
const CALLOUT: u16 = 2;
const OTHER: u16 = 3;

static OGG_440_48K: &[u8] = include_bytes!("spec_fixtures/sine440_mono_48k.ogg");
static OGG_880_44K1: &[u8] = include_bytes!("spec_fixtures/sine880_left_44k1.ogg");
static OGG_440_22K: &[u8] = include_bytes!("../src/mixer/fixtures/tone440_mono_22k.ogg");

/// A canonical 44-byte RIFF/WAVE header (integer PCM) with room for `data_len`
/// bytes of data; the caller appends the data and then calls [`wav_finish`].
fn wav_header(bits: u16, channels: u16, rate: u32, data_len: usize) -> Vec<u8> {
    let block = channels * (bits / 8);
    let mut v = Vec::with_capacity(44 + data_len + 1);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&((36 + data_len + data_len % 2) as u32).to_le_bytes());
    v.extend_from_slice(b"WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&rate.wrapping_mul(u32::from(block)).to_le_bytes());
    v.extend_from_slice(&block.to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data_len as u32).to_le_bytes());
    v
}

fn wav_finish(mut v: Vec<u8>) -> Vec<u8> {
    if !(v.len() - 44).is_multiple_of(2) {
        v.push(0);
    }
    v
}

/// 16-bit PCM WAV of `frames` frames; `f(frame, channel)` gives each sample.
fn wav_i16(channels: u16, rate: u32, frames: usize, f: impl Fn(usize, usize) -> i16) -> Vec<u8> {
    let mut v = wav_header(16, channels, rate, frames * 2 * channels as usize);
    for n in 0..frames {
        for c in 0..channels as usize {
            v.extend_from_slice(&f(n, c).to_le_bytes());
        }
    }
    wav_finish(v)
}

/// 8-bit (unsigned) PCM WAV of `frames` frames.
fn wav_u8(channels: u16, rate: u32, frames: usize, f: impl Fn(usize, usize) -> u8) -> Vec<u8> {
    let mut v = wav_header(8, channels, rate, frames * channels as usize);
    for n in 0..frames {
        for c in 0..channels as usize {
            v.push(f(n, c));
        }
    }
    wav_finish(v)
}

/// A deterministic, non-periodic-looking test signal for short fixtures.
fn varied(n: usize, c: usize) -> i16 {
    let x = (n.wrapping_mul(7919) + c * 4513 + (n / 97) * 31) % 20_011;
    (x as i32 - 10_005) as i16
}

/// LONG fixture sample: square (period 240) + triangle (period 1001) + a
/// per-second offset that differs for every one of the 600 seconds.
fn long_signal(n: usize) -> i16 {
    let sq: i32 = if (n / 120).is_multiple_of(2) {
        5_000
    } else {
        -5_000
    };
    let tri = ((n % 1001) as i32 - 500).abs() * 16 - 4_000;
    let drift = ((n / 24_000) as i32 * 37) % 2_000 - 1_000;
    (sq + tri + drift) as i16
}

/// 10 minutes (exactly 600 s) of mono 16-bit PCM at 24 kHz.
const LONG_SRC_RATE: u32 = 24_000;
const LONG_SRC_FRAMES: usize = 14_400_000;

fn long_wav() -> Vec<u8> {
    wav_i16(1, LONG_SRC_RATE, LONG_SRC_FRAMES, |n, _| long_signal(n))
}

fn resident(bytes: &[u8], rate: u32) -> Arc<Sample> {
    match decode(bytes, rate) {
        Ok(s) => Arc::new(s),
        Err(e) => panic!("reference decode failed: {e:?}"),
    }
}

/// A 1200-frame square-wave callout (ducks music while it plays).
fn callout(rate: u32) -> Arc<Sample> {
    resident(
        &wav_i16(1, RATE, 1_200, |n, _| {
            if (n / 50).is_multiple_of(2) {
                12_000
            } else {
                -12_000
            }
        }),
        rate,
    )
}

/// A short constant resident music sound used to steal the stream's voice.
fn other_music(rate: u32) -> Arc<Sample> {
    resident(&wav_i16(1, RATE, 800, |_, _| -3_000), rate)
}

fn bus(i: u8) -> Bus {
    match i {
        0 => Bus::Music,
        1 => Bus::Callout,
        _ => Bus::Sfx,
    }
}

fn cfg(rate: u32, max_music: usize) -> MixerConfig {
    let d = MixerConfig::default();
    let sfx = d.max_voices[2];
    let co = d.max_voices[1];
    MixerConfig {
        sample_rate: rate,
        max_voices: [max_music, co, sfx],
        ..d
    }
}

fn open(bytes: Vec<u8>, rate: u32, ring: usize, budget: &Arc<DecodeBudget>) -> StreamedTrack {
    match open_stream(bytes, rate, ring, budget) {
        Ok(t) => t,
        Err(e) => panic!("open_stream failed: {e:?}"),
    }
}

/// Render-chunk patterns: short tracks use 256, 1000 and odd sizes.
const SHORT_CHUNKS: [usize; 7] = [256, 1000, 37, 509, 1, 777, 2048];
/// Long tracks: 16 384-frame blocks.
const LONG_CHUNK: usize = 16_384;

// =============================================================== the pair rig

/// A streamed mixer and a resident reference mixer driven in lock step.
///
/// Every render waits for the stream ring first and never asks for more than
/// `ring_frames()` frames (S3), then compares both outputs bit for bit.
struct Pair {
    s: Mixer,
    stx: SoundSender,
    r: Mixer,
    rtx: SoundSender,
    h: Option<StreamHandle>,
    ring: usize,
    out_s: Vec<f32>,
    out_r: Vec<f32>,
    /// Allocation calls on this thread inside the streamed mixer's `render`.
    allocs: u64,
    /// Output frames rendered so far.
    frame: u64,
    /// Whether any streamed output sample was hard-clipped to +-1.
    clipped: bool,
}

impl Pair {
    fn new(rate: u32, max_music: usize, max_chunk: usize) -> Pair {
        let (stx, srx) = trigger_ring(64);
        let (rtx, rrx) = trigger_ring(64);
        Pair {
            s: Mixer::new(cfg(rate, max_music), srx),
            stx,
            r: Mixer::new(cfg(rate, max_music), rrx),
            rtx,
            h: None,
            ring: 0,
            out_s: vec![0.0; 2 * max_chunk],
            out_r: vec![0.0; 2 * max_chunk],
            allocs: 0,
            frame: 0,
            clipped: false,
        }
    }

    /// Registers a resident sound on both mixers.
    fn add(&mut self, id: u16, sample: &Arc<Sample>, b: u8) {
        self.s.register(SoundId(id), Arc::clone(sample), bus(b));
        self.r.register(SoundId(id), Arc::clone(sample), bus(b));
    }

    /// Registers the stream (as `MUSIC`) and its resident reference.
    fn attach(&mut self, track: StreamedTrack, reference: Arc<Sample>) {
        self.ring = track.ring_frames();
        assert!(self.ring >= 1, "a non-empty track needs a ring");
        self.h = Some(track.handle());
        if self
            .s
            .register_stream(SoundId(MUSIC), track, Bus::Music)
            .is_err()
        {
            panic!("register_stream refused Bus::Music");
        }
        self.r.register(SoundId(MUSIC), reference, Bus::Music);
    }

    fn handle(&self) -> &StreamHandle {
        self.h.as_ref().expect("attached")
    }

    fn trigger(&mut self, id: u16, gain: f32) {
        let t = SoundTrigger {
            sound: SoundId(id),
            gain,
        };
        assert!(self.stx.send(t) && self.rtx.send(t), "trigger ring full");
    }

    fn trigger_stream_only(&mut self, id: u16, gain: f32) {
        assert!(self.stx.send(SoundTrigger {
            sound: SoundId(id),
            gain,
        }));
    }

    /// One lock-step render of `frames` frames; returns the streamed output.
    fn step(&mut self, frames: usize) -> &[f32] {
        assert!(frames >= 1 && frames <= self.ring, "chunk {frames} > ring");
        self.handle().wait_filled();
        let Pair {
            s, r, out_s, out_r, ..
        } = self;
        let os = &mut out_s[..2 * frames];
        let or = &mut out_r[..2 * frames];
        let before = allocs_here();
        s.render(os);
        let a = allocs_here() - before;
        r.render(or);
        for (i, (a, b)) in os.iter().zip(or.iter()).enumerate() {
            if a.to_bits() != b.to_bits() {
                panic!(
                    "stream differs from resident decode at output frame {} ch {}: {a} vs {b}",
                    self.frame + (i / 2) as u64,
                    i % 2,
                );
            }
        }
        self.clipped |= os.iter().any(|v| v.abs() == 1.0);
        assert_eq!(
            self.s.active_voices(Bus::Music),
            self.r.active_voices(Bus::Music),
            "music voice count differs at frame {}",
            self.frame
        );
        self.allocs += a;
        self.frame += frames as u64;
        &self.out_s[..2 * frames]
    }

    /// Steps with `pattern` (clipped to the ring) until the reference music
    /// voice has ended; returns the streamed output when `keep` is set.
    fn play_to_end(&mut self, pattern: &[usize], keep: bool) -> Vec<f32> {
        let mut kept = Vec::new();
        for i in 0..1_000_000usize {
            let n = pattern[i % pattern.len()].min(self.ring);
            let out = self.step(n);
            if keep {
                kept.extend_from_slice(out);
            }
            if self.r.active_voices(Bus::Music) == 0 {
                assert_eq!(self.s.active_voices(Bus::Music), 0);
                return kept;
            }
        }
        panic!("music voice never ended");
    }

    fn assert_no_underrun(&self) {
        assert_eq!(self.handle().underrun_frames(), 0, "S3: no underrun");
        assert_eq!(self.s.stream_underruns(), 0, "S3: no underrun");
    }
}

// =============================================================== AC1

/// #130 AC1, SPEC 3.3, D5, D25: a 10-minute mono 24 kHz music fixture streams
/// at 48 kHz (resampled) through a 1 MiB budget, bit-identical to the full
/// decode block by block, including a callout (ducking), an ignored mid-play
/// retrigger and a rewind after the end; the live-heap peak stays within
/// `ring_bytes() + 4 MiB` of the baseline; zero allocations per render on the
/// audio thread.
#[test]
fn ten_minute_stream_equals_full_decode() {
    let _g = serial();
    let bytes = long_wav();
    // S5: a resident decode does not fit a 1 MiB budget ...
    let small = DecodeBudget::new(1 << 20);
    assert_eq!(
        decode_with_budget(&bytes, RATE, &small),
        Err(DecodeError::BudgetExhausted)
    );
    assert_eq!(small.used_bytes(), 0);
    let reference = resident(&bytes, RATE);
    assert_eq!(
        reference.frames(),
        2 * LONG_SRC_FRAMES,
        "ceil(n * 48k / 24k)"
    );
    assert_eq!(reference.channels(), 1);

    let mut p = Pair::new(RATE, 2, LONG_CHUNK);
    p.add(CALLOUT, &callout(RATE), 1);
    let budget = Arc::new(DecodeBudget::new(1 << 20));

    // S5 baseline: input bytes, reference and buffers already exist.
    let baseline = reset_peak();
    // ... but the stream does (S1: charged by the ring only).
    let track = open(bytes, RATE, STREAM_RING_FRAMES, &budget);
    assert_eq!(track.channels(), 1);
    assert_eq!(track.ring_frames(), STREAM_RING_FRAMES);
    assert_eq!(track.ring_bytes(), (STREAM_RING_FRAMES * 4) as u64);
    assert_eq!(track.ring_bytes(), 256 * 1024);
    assert_eq!(budget.used_bytes(), track.ring_bytes());
    let ring_bytes = track.ring_bytes() as usize;
    p.attach(track, Arc::clone(&reference));
    assert_eq!(budget.used_bytes(), ring_bytes as u64);

    p.trigger(MUSIC, 0.8);
    let mut block = 0usize;
    loop {
        match block {
            100 => p.trigger(CALLOUT, 1.0),
            // S7: a retrigger while playing is ignored by the stream; the
            // reference gets no trigger, so equality proves it was ignored.
            300 => p.trigger_stream_only(MUSIC, 0.8),
            _ => {}
        }
        p.step(LONG_CHUNK);
        if block >= 300 {
            assert!(p.s.active_voices(Bus::Music) <= 1);
        }
        block += 1;
        if p.r.active_voices(Bus::Music) == 0 {
            break;
        }
        assert!(block < 2_000, "LONG track did not end");
    }
    assert_eq!(block, (2 * LONG_SRC_FRAMES).div_ceil(LONG_CHUNK));
    // Rewind after the end: plays from source frame 0 again.
    p.trigger(MUSIC, 0.8);
    for _ in 0..8 {
        p.step(LONG_CHUNK);
    }
    let rise = peak().saturating_sub(baseline);
    p.assert_no_underrun();
    assert_eq!(p.allocs, 0, "D5: allocations in streamed render");
    assert!(
        rise <= ring_bytes + (4 << 20),
        "S5: peak rose {rise} bytes above baseline (ring {ring_bytes})"
    );
    eprintln!("AC1 memory: peak rise {rise} bytes, ring {ring_bytes} bytes");
    drop(p);
    assert_eq!(budget.used_bytes(), 0, "S1: dropping the mixer releases");
}

// =============================================================== S3 / S11

/// SPEC 3.3 (S3): 20 s stereo tracks at 48 kHz and 44.1 kHz with different
/// left and right content, gains 0.5 and 1.7 (clipping), with a callout.
#[test]
fn stereo_streams_equal_full_decode() {
    let _g = serial();
    for src_rate in [48_000u32, 44_100] {
        let frames = 20 * src_rate as usize;
        let bytes = wav_i16(2, src_rate, frames, |n, c| {
            if c == 0 {
                let sq: i32 = if (n / 48).is_multiple_of(2) {
                    15_000
                } else {
                    -15_000
                };
                (sq + varied(n, 0) as i32 / 2) as i16
            } else {
                let p = (n % 331) as i32;
                ((p - 165).abs() * 300 - 24_750) as i16
            }
        });
        let reference = resident(&bytes, RATE);
        assert_eq!(reference.channels(), 2);
        for gain in [0.5f32, 1.7] {
            let budget = Arc::new(DecodeBudget::new(64 << 20));
            let track = open(bytes.clone(), RATE, STREAM_RING_FRAMES, &budget);
            assert_eq!(track.channels(), 2);
            assert_eq!(track.ring_frames(), STREAM_RING_FRAMES);
            assert_eq!(track.ring_bytes(), (STREAM_RING_FRAMES * 8) as u64);
            let mut p = Pair::new(RATE, 2, LONG_CHUNK);
            p.add(CALLOUT, &callout(RATE), 1);
            p.attach(track, Arc::clone(&reference));
            p.trigger(MUSIC, gain);
            let pattern = [LONG_CHUNK, 1000, 4097, 256, 333];
            for i in 0..12 {
                if i == 10 {
                    p.trigger(CALLOUT, 0.9);
                }
                p.step(pattern[i % pattern.len()]);
            }
            p.play_to_end(&pattern, false);
            assert_eq!(p.clipped, gain > 1.0, "gain {gain}: clipping exercised");
            p.assert_no_underrun();
            assert_eq!(p.allocs, 0, "D5");
            drop(p);
            assert_eq!(budget.used_bytes(), 0);
        }
    }
}

/// SPEC 3.3 (S11): the Ogg Vorbis fixtures stream bit-identically to their
/// resident decode at mixer rates 48 kHz and 44.1 kHz, twice (rewind).
#[test]
fn ogg_streams_equal_full_decode() {
    let _g = serial();
    for ogg in [OGG_440_48K, OGG_880_44K1, OGG_440_22K] {
        for rate in [48_000u32, 44_100] {
            let reference = resident(ogg, rate);
            let budget = Arc::new(DecodeBudget::new(16 << 20));
            let track = open(ogg.to_vec(), rate, STREAM_RING_FRAMES, &budget);
            assert!(track.ring_frames() <= STREAM_RING_FRAMES);
            assert_eq!(track.channels(), reference.channels());
            assert_eq!(
                track.ring_bytes(),
                (track.ring_frames() * 4 * track.channels()) as u64
            );
            assert_eq!(budget.used_bytes(), track.ring_bytes());
            let mut p = Pair::new(rate, 2, 2048);
            p.add(CALLOUT, &callout(rate), 1);
            p.attach(track, Arc::clone(&reference));
            p.trigger(MUSIC, 0.9);
            p.step(256.min(p.ring));
            p.trigger(CALLOUT, 1.0);
            let first = p.play_to_end(&SHORT_CHUNKS, true);
            // Rewind: the second play is compared with the resident
            // reference in `step` as well.
            p.trigger(MUSIC, 0.9);
            let second = p.play_to_end(&SHORT_CHUNKS, true);
            assert!(first.iter().any(|v| *v != 0.0), "first play audible");
            assert!(second.iter().any(|v| *v != 0.0), "rewound play audible");
            p.assert_no_underrun();
            assert_eq!(p.allocs, 0, "D5");
            drop(p);
            assert_eq!(budget.used_bytes(), 0);
        }
    }
}

/// SPEC 3.3 (S3, SHORT): tracks shorter than, equal to, one longer than and
/// twice the ring (ring 1000 frames), at the mixer rate and resampled from
/// 44.1 kHz, mono and stereo; played twice (rewind) with a callout.
#[test]
fn boundary_lengths_equal_full_decode() {
    let _g = serial();
    const R: usize = 1000;
    // (source rate, channels, source frames); 918 / 919 frames at 44.1 kHz
    // resample to exactly 1000 / 1001 frames at 48 kHz.
    let cases: [(u32, u16, usize); 11] = [
        (48_000, 1, 1),
        (48_000, 1, 100),
        (48_000, 1, 999),
        (48_000, 1, R),
        (48_000, 1, R + 1),
        (48_000, 1, 2 * R),
        (48_000, 1, 2 * R + 333),
        (48_000, 2, R),
        (48_000, 2, R + 1),
        (44_100, 1, 918),
        (44_100, 2, 919),
    ];
    for (src_rate, ch, frames) in cases {
        let bytes = wav_i16(ch, src_rate, frames, varied);
        let reference = resident(&bytes, RATE);
        let len = reference.frames();
        let budget = Arc::new(DecodeBudget::new(1 << 20));
        let track = open(bytes, RATE, R, &budget);
        let ring = track.ring_frames();
        let ctx = format!("{src_rate} Hz x{ch}, {frames} frames ({len} out), ring {ring}");
        assert!(ring <= R, "{ctx}");
        if len > R {
            assert_eq!(ring, R, "{ctx}: S1 ring equals request");
        }
        assert_eq!(track.channels(), ch as usize, "{ctx}");
        assert_eq!(track.ring_bytes(), (ring * 4 * ch as usize) as u64, "{ctx}");
        assert_eq!(budget.used_bytes(), track.ring_bytes(), "{ctx}");
        let h = track.handle();
        h.wait_filled();
        assert_eq!(h.buffered_frames(), len.min(ring), "{ctx}: prefill");
        if len > ring {
            assert!(!h.is_finished(), "{ctx}: ring full before the end");
        } else if len < ring {
            assert!(h.is_finished(), "{ctx}: whole track buffered");
        }
        assert!(!h.failed(), "{ctx}");
        let mut p = Pair::new(RATE, 2, 2048);
        p.add(CALLOUT, &callout(RATE), 1);
        p.attach(track, Arc::clone(&reference));
        p.trigger(MUSIC, 0.9);
        p.step(37.min(p.ring));
        p.trigger(CALLOUT, 1.0);
        p.play_to_end(&SHORT_CHUNKS, false);
        p.step(256.min(p.ring));
        // Rewind: same output as the resident reference again.
        p.trigger(MUSIC, 1.3);
        p.play_to_end(&SHORT_CHUNKS[1..], false);
        p.assert_no_underrun();
        assert_eq!(p.allocs, 0, "{ctx}: D5");
        drop(p);
        assert_eq!(budget.used_bytes(), 0, "{ctx}");
    }
}

// =============================================================== S1 / S9

fn mono_track_bytes(frames: usize) -> Vec<u8> {
    wav_i16(1, RATE, frames, varied)
}

/// S1 / S9 (budget, D25): the ring is charged (4 bytes per mono frame, 8 per
/// stereo frame), over-budget opens fail without a charge, and dropping the
/// track, its owning mixer or replacing its registration releases exactly;
/// only `Bus::Music` accepts a stream.
#[test]
fn budget_charge_and_release() {
    let _g = serial();
    let mono = mono_track_bytes(5_000);
    let stereo = wav_i16(2, RATE, 5_000, varied);
    for r in [1usize, 7, 512, 4096] {
        for (bytes, ch) in [(&mono, 1usize), (&stereo, 2)] {
            let budget = Arc::new(DecodeBudget::new(1 << 20));
            let t = open(bytes.clone(), RATE, r, &budget);
            assert_eq!(t.ring_frames(), r, "S1: track longer than the ring");
            assert_eq!(t.channels(), ch);
            assert_eq!(t.ring_bytes(), (r * 4 * ch) as u64);
            assert_eq!(t.handle().ring_bytes(), t.ring_bytes());
            assert_eq!(budget.used_bytes(), t.ring_bytes());
            assert_eq!(
                budget.remaining_bytes(),
                budget.limit_bytes() - t.ring_bytes()
            );
            drop(t);
            assert_eq!(budget.used_bytes(), 0);
        }
    }
    // A track shorter than the ring: charged by its (possibly clamped) ring.
    {
        let budget = Arc::new(DecodeBudget::new(1 << 20));
        let t = open(mono_track_bytes(100), RATE, 4096, &budget);
        assert!(t.ring_frames() >= 1 && t.ring_frames() <= 4096);
        assert_eq!(budget.used_bytes(), (t.ring_frames() * 4) as u64);
    }

    // Pre-existing charge from a resident decode.
    let small = mono_track_bytes(300);
    let probe = DecodeBudget::new(1 << 20);
    decode_with_budget(&small, RATE, &probe).unwrap();
    let c = probe.used_bytes();
    assert!(c > 0);
    let fresh = |extra: u64| {
        let b = Arc::new(DecodeBudget::new(c + extra));
        decode_with_budget(&small, RATE, &b).unwrap();
        assert_eq!(b.used_bytes(), c);
        b
    };
    // Over budget by one byte: BudgetExhausted, nothing charged.
    for (bytes, ch) in [(&mono, 1u64), (&stereo, 2)] {
        let b = fresh(512 * 4 * ch - 1);
        assert_eq!(
            open_stream(bytes.clone(), RATE, 512, &b).err(),
            Some(DecodeError::BudgetExhausted)
        );
        assert_eq!(b.used_bytes(), c, "S1: failed open leaves no charge");
        let b = fresh(512 * 4 * ch);
        let t = open(bytes.clone(), RATE, 512, &b);
        assert_eq!(b.remaining_bytes(), 0, "exact fit");
        drop(t);
        assert_eq!(b.used_bytes(), c, "never below the prior value");
    }

    // Owned by a mixer: dropping the mixer releases (also mid-play).
    for play in [false, true] {
        let b = fresh(1 << 20);
        let (mut tx, rx) = trigger_ring(8);
        let mut m = Mixer::new(cfg(RATE, 2), rx);
        let t = open(mono.clone(), RATE, 512, &b);
        let h = t.handle();
        assert!(m.register_stream(SoundId(MUSIC), t, Bus::Music).is_ok());
        assert_eq!(b.used_bytes(), c + 2048);
        if play {
            tx.send(SoundTrigger {
                sound: SoundId(MUSIC),
                gain: 1.0,
            });
            let mut out = vec![0.0f32; 2 * 256];
            h.wait_filled();
            m.render(&mut out);
            assert_eq!(m.active_voices(Bus::Music), 1);
        }
        drop(m);
        assert_eq!(b.used_bytes(), c, "S1: dropping the mixer releases");
        // The handle outlives the track without keeping the charge.
        assert_eq!(h.ring_bytes(), 2048);
        drop(h);
        assert_eq!(b.used_bytes(), c);
    }

    // Replacing the registration releases (with a resident sound, and with
    // another stream that is then charged instead).
    {
        let b = fresh(1 << 20);
        let (mut tx, rx) = trigger_ring(8);
        let mut m = Mixer::new(cfg(RATE, 2), rx);
        let t = open(mono.clone(), RATE, 512, &b);
        let h = t.handle();
        assert!(m.register_stream(SoundId(MUSIC), t, Bus::Music).is_ok());
        tx.send(SoundTrigger {
            sound: SoundId(MUSIC),
            gain: 1.0,
        });
        let mut out = vec![0.0f32; 2 * 256];
        h.wait_filled();
        m.render(&mut out);
        drop(h);
        let t2 = open(stereo.clone(), RATE, 100, &b);
        assert_eq!(b.used_bytes(), c + 2048 + 800);
        assert!(m.register_stream(SoundId(MUSIC), t2, Bus::Music).is_ok());
        m.render(&mut out);
        assert_eq!(b.used_bytes(), c + 800, "replaced stream released");
        m.register(SoundId(MUSIC), resident(&small, RATE), Bus::Music);
        m.render(&mut out);
        assert_eq!(b.used_bytes(), c, "replaced by a resident sound");
        drop(m);
        assert_eq!(b.used_bytes(), c);
    }

    // S9: only Bus::Music accepts a stream.
    for b_idx in [1u8, 2] {
        let b = fresh(1 << 20);
        let (mut tx, rx) = trigger_ring(8);
        let mut m = Mixer::new(cfg(RATE, 2), rx);
        let t = open(mono.clone(), RATE, 512, &b);
        let back = match m.register_stream(SoundId(MUSIC), t, bus(b_idx)) {
            Ok(()) => panic!("S9: register_stream accepted a non-music bus"),
            Err(t) => t,
        };
        assert_eq!(back.ring_frames(), 512);
        assert_eq!(b.used_bytes(), c + 2048, "S9: still charged");
        let unknown = m.unknown_sounds();
        tx.send(SoundTrigger {
            sound: SoundId(MUSIC),
            gain: 1.0,
        });
        let mut out = vec![1.0f32; 2 * 256];
        m.render(&mut out);
        assert_eq!(m.unknown_sounds(), unknown + 1, "S9: nothing registered");
        for bb in 0..3 {
            assert_eq!(m.active_voices(bus(bb)), 0);
        }
        assert!(out.iter().all(|v| *v == 0.0));
        assert_eq!(b.used_bytes(), c + 2048);
        drop(back);
        assert_eq!(b.used_bytes(), c, "S9: released when the track drops");
        drop(m);
        assert_eq!(b.used_bytes(), c);
    }
}

// =============================================================== S2

/// S2 (decode rules, D25): header errors are synchronous and equal to
/// `decode_with_budget`'s; nothing is charged on error; never panics.
#[test]
fn header_errors_match_decode() {
    let _g = serial();
    let ok = mono_track_bytes(200);
    let mut cases: Vec<(&str, Vec<u8>, u32, Option<DecodeError>)> = vec![
        ("empty", Vec::new(), RATE, Some(DecodeError::Unsupported)),
        (
            "text",
            b"nope".to_vec(),
            RATE,
            Some(DecodeError::Unsupported),
        ),
        ("junk", vec![0xAB; 64], RATE, Some(DecodeError::Unsupported)),
        ("rate 0", ok.clone(), 0, Some(DecodeError::Unsupported)),
        (
            "rate 384001",
            ok.clone(),
            384_001,
            Some(DecodeError::Unsupported),
        ),
        ("rate 384000", ok.clone(), 384_000, None),
        ("rate 8000", ok.clone(), 8_000, None),
        (
            "src 7999 Hz",
            wav_i16(1, 7_999, 100, varied),
            RATE,
            Some(DecodeError::Unsupported),
        ),
        ("src 8000 Hz", wav_i16(1, 8_000, 100, varied), RATE, None),
        (
            "3 channels",
            wav_i16(3, RATE, 100, varied),
            RATE,
            Some(DecodeError::TooManyChannels),
        ),
        (
            "ogg rate 0",
            OGG_440_48K.to_vec(),
            0,
            Some(DecodeError::Unsupported),
        ),
    ];
    // Exactly 600 s accepted, one frame more is TooLong (8-bit 8 kHz keeps
    // the fixture at 4.8 MB; mixer rate 8 kHz avoids resampling).
    cases.push((
        "600 s",
        wav_u8(1, 8_000, 4_800_000, |n, _| (n % 251) as u8),
        8_000,
        None,
    ));
    cases.push((
        "600 s + 1 frame",
        wav_u8(1, 8_000, 4_800_001, |n, _| (n % 251) as u8),
        8_000,
        Some(DecodeError::TooLong),
    ));
    for (name, bytes, rate, want) in cases {
        let db = DecodeBudget::new(512 << 20);
        let d = decode_with_budget(&bytes, rate, &db).map(|_| ());
        let sb = Arc::new(DecodeBudget::new(512 << 20));
        let o = open_stream(bytes, rate, 4096, &sb);
        match want {
            Some(e) => {
                assert_eq!(d.as_ref().err(), Some(&e), "{name}: decode");
                assert_eq!(o.as_ref().err(), Some(&e), "{name}: open_stream");
                assert_eq!(sb.used_bytes(), 0, "{name}: nothing charged");
            }
            None => {
                assert_eq!(d, Ok(()), "{name}: decode accepts");
                let t = match o {
                    Ok(t) => t,
                    Err(e) => panic!("{name}: open_stream rejected: {e:?}"),
                };
                assert_eq!(sb.used_bytes(), t.ring_bytes(), "{name}");
                drop(t);
                assert_eq!(sb.used_bytes(), 0, "{name}");
            }
        }
    }
}

// =============================================================== S6

/// SPEC 3.3 (S6): after a streamed voice ends the next trigger plays from
/// source frame 0 again, bit-identical to the first play and to the resident
/// reference; `is_finished` reads false for the new pass; and after the
/// stream's voice is stolen (one music voice) a later trigger starts at
/// frame 0 with no stale frames.
#[test]
fn rewind_after_end_and_after_steal() {
    let _g = serial();
    const R: usize = 1000;
    // Rewind after the end, track longer than the ring.
    for (src_rate, frames) in [(48_000u32, 3_000usize), (44_100, 2_500), (48_000, 2 * R)] {
        let bytes = wav_i16(1, src_rate, frames, varied);
        let reference = resident(&bytes, RATE);
        let budget = Arc::new(DecodeBudget::new(1 << 20));
        let mut p = Pair::new(RATE, 2, R);
        p.attach(open(bytes, RATE, R, &budget), reference);
        assert_eq!(p.ring, R);
        p.trigger(MUSIC, 0.7);
        let first = p.play_to_end(&[500], true);
        // The pass after the end is a new pass: the track is longer than the
        // ring, so the loader cannot have reached its end yet.
        assert!(!p.handle().is_finished(), "S6: new pass not finished");
        p.handle().wait_filled();
        assert!(!p.handle().is_finished(), "S6: ring full, pass not done");
        assert_eq!(p.handle().buffered_frames(), R, "S6: refilled from 0");
        p.trigger(MUSIC, 0.7);
        let second = p.play_to_end(&[500], true);
        assert_eq!(first.len(), second.len());
        assert!(
            first
                .iter()
                .zip(&second)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "S6: second play equals the first"
        );
        assert!(first.iter().any(|v| *v != 0.0));
        p.assert_no_underrun();
        assert_eq!(p.allocs, 0);
        drop(p);
        assert_eq!(budget.used_bytes(), 0);
    }

    // Stolen voice (max one music voice), at several points: while the
    // loader is mid-pass and after it already reached the end of source.
    for (frames, steal_after) in [(3_000usize, 2usize), (3_000, 4), (1_500, 2), (1_200, 1)] {
        let bytes = wav_i16(1, RATE, frames, varied);
        let reference = resident(&bytes, RATE);
        let budget = Arc::new(DecodeBudget::new(1 << 20));
        let mut p = Pair::new(RATE, 1, R);
        p.add(OTHER, &other_music(RATE), 0);
        p.attach(open(bytes, RATE, R, &budget), reference);
        p.trigger(MUSIC, 1.0);
        for _ in 0..steal_after {
            p.step(500);
        }
        p.trigger(OTHER, 1.0);
        p.step(300);
        assert_eq!(p.s.stolen_voices(), p.r.stolen_voices());
        assert!(p.s.stolen_voices() >= 1, "the stream's voice was stolen");
        p.step(300);
        p.trigger(MUSIC, 1.0);
        let replay = p.play_to_end(&[500, 333], true);
        assert!(replay.iter().any(|v| *v != 0.0));
        p.assert_no_underrun();
        assert_eq!(p.allocs, 0);
        drop(p);
        assert_eq!(budget.used_bytes(), 0);
    }
}

// =============================================================== S7

/// SPEC 3.3 (S7): one voice per streamed track; a trigger while its voice is
/// active is ignored (the output equals a single play).
#[test]
fn second_trigger_is_ignored_while_playing() {
    let _g = serial();
    const R: usize = 1000;
    let bytes = wav_i16(2, RATE, 3_000, varied);
    let reference = resident(&bytes, RATE);
    let budget = Arc::new(DecodeBudget::new(1 << 20));
    let mut p = Pair::new(RATE, 2, R);
    p.attach(open(bytes, RATE, R, &budget), reference);
    // Two triggers in the same batch on the stream: still one voice.
    p.trigger(MUSIC, 0.6);
    p.trigger_stream_only(MUSIC, 0.6);
    p.step(400);
    assert_eq!(p.s.active_voices(Bus::Music), 1);
    for k in 0..8u32 {
        if k.is_multiple_of(2) {
            p.trigger_stream_only(MUSIC, 1.5);
        }
        p.step(301);
        assert!(p.s.active_voices(Bus::Music) <= 1);
    }
    p.play_to_end(&[256], false);
    p.assert_no_underrun();
    drop(p);
    assert_eq!(budget.used_bytes(), 0);
}

/// SPEC 3.3 (S7): a zero-length track (valid WAV, no frames) opens, charges
/// at most its ring, and a trigger starts no voice (exact silence).
#[test]
fn zero_length_track() {
    let _g = serial();
    for ch in [1u16, 2] {
        let bytes = wav_i16(ch, RATE, 0, varied);
        let budget = Arc::new(DecodeBudget::new(1 << 20));
        let t = open(bytes, RATE, 4096, &budget);
        assert!(t.ring_frames() <= 4096);
        assert_eq!(t.channels(), ch as usize);
        assert_eq!(t.ring_bytes(), (t.ring_frames() * 4 * ch as usize) as u64);
        assert_eq!(budget.used_bytes(), t.ring_bytes());
        let h = t.handle();
        h.wait_filled();
        assert_eq!(h.buffered_frames(), 0);
        let (mut tx, rx) = trigger_ring(8);
        let mut m = Mixer::new(cfg(RATE, 2), rx);
        assert!(m.register_stream(SoundId(MUSIC), t, Bus::Music).is_ok());
        let mut out = vec![1.0f32; 2 * 256];
        for _ in 0..3 {
            assert!(tx.send(SoundTrigger {
                sound: SoundId(MUSIC),
                gain: 1.0,
            }));
            h.wait_filled();
            m.render(&mut out);
            assert_eq!(m.active_voices(Bus::Music), 0, "S7: no voice");
            assert!(out.iter().all(|v| *v == 0.0), "exact silence");
        }
        assert_eq!(h.underrun_frames(), 0);
        assert_eq!(m.stream_underruns(), 0);
        drop(m);
        drop(h);
        assert_eq!(budget.used_bytes(), 0);
    }
}

// =============================================================== S8

/// SPEC 3.3 (S8): frames the ring cannot supply render as silence and are
/// counted one per missing frame; the voice never skips: exactly N frames of
/// the DC track (0.25) play, and the zero frames between the trigger render
/// and the last real frame equal the underrun count. Only invariants are
/// asserted (underruns depend on thread timing).
#[test]
fn underrun_invariants() {
    let _g = serial();
    const N: usize = 20_000;
    const CHUNK: usize = 300;
    const MAX_FRAMES: u64 = 50_000_000;
    let bytes = wav_i16(1, RATE, N, |_, _| 8_192);
    for run in 0..3 {
        let budget = Arc::new(DecodeBudget::new(1 << 20));
        let t = open(bytes.clone(), RATE, 512, &budget);
        assert_eq!(t.ring_frames(), 512);
        let h = t.handle();
        let (mut tx, rx) = trigger_ring(8);
        let mut m = Mixer::new(cfg(RATE, 2), rx);
        assert!(m.register_stream(SoundId(MUSIC), t, Bus::Music).is_ok());
        assert!(tx.send(SoundTrigger {
            sound: SoundId(MUSIC),
            gain: 1.0,
        }));
        let mut out = vec![0.0f32; 2 * CHUNK];
        let (mut real, mut zeros_pending, mut gaps) = (0usize, 0u64, 0u64);
        let mut rendered = 0u64;
        loop {
            m.render(&mut out);
            rendered += CHUNK as u64;
            for f in out.chunks_exact(2) {
                if f[0] == 0.25 && f[1] == 0.25 {
                    real += 1;
                    gaps += zeros_pending;
                    zeros_pending = 0;
                } else if f[0] == 0.0 && f[1] == 0.0 {
                    zeros_pending += 1;
                } else {
                    panic!("run {run}: unexpected frame {f:?}");
                }
            }
            if m.active_voices(Bus::Music) == 0 {
                break;
            }
            assert!(rendered < MAX_FRAMES, "run {run}: voice never ended");
            std::thread::yield_now();
        }
        assert_eq!(real, N, "run {run}: S8 every source frame played once");
        assert_eq!(
            h.underrun_frames(),
            gaps,
            "run {run}: S8 underrun count ({zeros_pending} zero frames after the last real \
             frame, rendered {rendered} frames)"
        );
        assert_eq!(m.stream_underruns(), gaps, "run {run}: mixer total");
        eprintln!("underrun_invariants run {run}: {gaps} underrun frames");
        drop(m);
        drop(h);
        assert_eq!(budget.used_bytes(), 0);
    }
}

// =============================================================== S10

/// Opens `bytes` as a stream and, if it opens, plays it to the end; checks
/// the budget is clean afterwards. Never hangs: everything is dropped (and
/// the loader joined) before returning.
fn exercise(bytes: Vec<u8>, out: &mut [f32]) -> Result<(), TestCaseError> {
    const MAX_ITERS: usize = 1_000_000;
    let budget = Arc::new(DecodeBudget::new(16 << 20));
    let t = match open_stream(bytes, RATE, STREAM_RING_FRAMES, &budget) {
        Ok(t) => t,
        Err(_) => {
            prop_assert_eq!(budget.used_bytes(), 0, "failed open charged");
            return Ok(());
        }
    };
    prop_assert_eq!(budget.used_bytes(), t.ring_bytes());
    prop_assert!(t.channels() == 1 || t.channels() == 2);
    let chunk = t.ring_frames().clamp(1, 4096);
    let h = t.handle();
    let (mut tx, rx) = trigger_ring(8);
    let mut m = Mixer::new(cfg(RATE, 2), rx);
    prop_assert!(m.register_stream(SoundId(MUSIC), t, Bus::Music).is_ok());
    let sent = tx.send(SoundTrigger {
        sound: SoundId(MUSIC),
        gain: 1.0,
    });
    prop_assert!(sent);
    let mut ended = false;
    for _ in 0..MAX_ITERS {
        h.wait_filled();
        m.render(&mut out[..2 * chunk]);
        prop_assert!(out[..2 * chunk].iter().all(|v| v.abs() <= 1.0));
        if m.active_voices(Bus::Music) == 0 {
            ended = true;
            break;
        }
    }
    prop_assert!(ended, "voice did not end (failed = {})", h.failed());
    drop(m);
    drop(h);
    prop_assert_eq!(budget.used_bytes(), 0, "released after drop");
    Ok(())
}

/// D25 / R11 (S10): arbitrary bytes and corrupted WAV / Ogg files never
/// panic or hang `open_stream`, the loader or `render`; errors leave nothing
/// charged; opened tracks play to the end and release the budget.
/// Deterministic (fixed ChaCha seed, R3), 64 cases per strategy.
#[test]
fn hostile_bytes_and_corruption() {
    let _g = serial();
    let out = RefCell::new(vec![0.0f32; 2 * 4096]);
    let config = || Config {
        cases: 64,
        failure_persistence: None,
        ..Config::default()
    };

    // (a) Arbitrary bytes up to 4 KiB, sometimes behind a valid magic.
    let mut runner = TestRunner::new_with_rng(
        config(),
        TestRng::from_seed(RngAlgorithm::ChaCha, &[0x13; 32]),
    );
    runner
        .run(
            &(0u8..3, prop::collection::vec(any::<u8>(), 0..4096)),
            |(prefix, tail)| {
                let mut bytes = match prefix {
                    0 => Vec::new(),
                    1 => b"RIFF\0\0\0\0WAVE".to_vec(),
                    _ => b"OggS".to_vec(),
                };
                bytes.extend_from_slice(&tail);
                bytes.truncate(4096);
                exercise(bytes, &mut out.borrow_mut())
            },
        )
        .unwrap();

    // (b) Corruptions of valid files: flip bytes, truncate, zero a range.
    let bases: Vec<Vec<u8>> = vec![
        wav_i16(1, RATE, 2_000, varied),
        wav_u8(2, 22_050, 1_000, |n, c| (n * 3 + c * 77) as u8),
        OGG_440_48K.to_vec(),
        OGG_880_44K1.to_vec(),
        OGG_440_22K.to_vec(),
    ];
    let mut runner = TestRunner::new_with_rng(
        config(),
        TestRng::from_seed(RngAlgorithm::ChaCha, &[0x31; 32]),
    );
    runner
        .run(
            &(
                0usize..bases.len(),
                0u8..3,
                any::<u32>(),
                1usize..512,
                prop::collection::vec((any::<u32>(), any::<u8>()), 1..8),
            ),
            |(which, op, pos, len, flips)| {
                let mut b = bases[which].clone();
                let at = pos as usize % b.len();
                match op {
                    0 => {
                        for (i, x) in &flips {
                            let k = *i as usize % b.len();
                            b[k] ^= x | 1;
                        }
                    }
                    1 => b.truncate(at),
                    _ => {
                        let end = (at + len).min(b.len());
                        b[at..end].fill(0);
                    }
                }
                exercise(b, &mut out.borrow_mut())
            },
        )
        .unwrap();
}

// =============================================================== S4

/// D5 (S4): `render` of a streamed voice never allocates on the audio
/// thread: start, middle, end, after the end, retrigger while playing,
/// rewind, steal, replay after the steal, and a render without waiting for
/// the ring (underrun path), with a callout ducking the music.
#[test]
fn render_does_not_allocate() {
    let _g = serial();
    const R: usize = 1000;
    let budget = Arc::new(DecodeBudget::new(1 << 20));
    let (mut tx, rx) = trigger_ring(32);
    let mut m = Mixer::new(cfg(RATE, 1), rx);
    let t = open(wav_i16(1, 44_100, 3_000, varied), RATE, R, &budget);
    let h = t.handle();
    assert!(m.register_stream(SoundId(MUSIC), t, Bus::Music).is_ok());
    m.register(SoundId(OTHER), other_music(RATE), Bus::Music);
    m.register(SoundId(CALLOUT), callout(RATE), Bus::Callout);
    let mut out = vec![0.0f32; 2 * R];
    let mut report: Vec<(&str, u64)> = Vec::with_capacity(256);

    let send = |tx: &mut SoundSender, id: u16| {
        assert!(tx.send(SoundTrigger {
            sound: SoundId(id),
            gain: 1.0,
        }));
    };
    let render = |m: &mut Mixer, out: &mut [f32], wait: bool| -> u64 {
        if wait {
            h.wait_filled();
        }
        let before = allocs_here();
        m.render(out);
        allocs_here() - before
    };

    send(&mut tx, MUSIC);
    report.push(("start", render(&mut m, &mut out[..2 * 300], true)));
    assert_eq!(m.active_voices(Bus::Music), 1);
    send(&mut tx, CALLOUT);
    report.push(("middle+callout", render(&mut m, &mut out[..2 * 1000], true)));
    send(&mut tx, MUSIC);
    report.push(("retrigger", render(&mut m, &mut out[..2 * 777], true)));
    for _ in 0..10 {
        let a = render(&mut m, &mut out[..2 * 999], true);
        report.push(("to end", a));
        if m.active_voices(Bus::Music) == 0 {
            break;
        }
    }
    assert_eq!(m.active_voices(Bus::Music), 0, "voice ended");
    report.push(("after end", render(&mut m, &mut out[..2 * 256], true)));
    send(&mut tx, MUSIC);
    report.push(("rewind", render(&mut m, &mut out[..2 * 500], true)));
    assert_eq!(m.active_voices(Bus::Music), 1);
    report.push(("middle", render(&mut m, &mut out[..2 * 500], true)));
    send(&mut tx, OTHER);
    report.push(("steal", render(&mut m, &mut out[..2 * 300], true)));
    report.push(("other", render(&mut m, &mut out[..2 * 300], true)));
    send(&mut tx, MUSIC);
    report.push(("after steal", render(&mut m, &mut out[..2 * 300], true)));
    for _ in 0..20 {
        report.push(("replay", render(&mut m, &mut out[..2 * 1000], true)));
        if m.active_voices(Bus::Music) == 0 {
            break;
        }
    }
    // Underrun path: trigger after the end and render without waiting.
    send(&mut tx, MUSIC);
    for _ in 0..40 {
        report.push(("no wait", render(&mut m, &mut out[..2 * 300], false)));
    }
    let bad: Vec<_> = report.iter().filter(|(_, a)| *a != 0).collect();
    assert!(bad.is_empty(), "D5: allocating renders: {bad:?}");
    drop(m);
    assert_eq!(budget.used_bytes(), 0);
}
