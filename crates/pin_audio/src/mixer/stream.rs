//! Streamed music: a track decoded from its compressed bytes by a loader
//! thread into a bounded ring that the audio thread only pops from (module
//! docs of [`super`], "Streaming").
//!
//! The loader runs the same decode and resampling code as
//! [`decode_with_budget`](super::decode_with_budget), so a streamed track
//! produces bit-identical frames. The audio thread uses only atomics and the
//! lock-free [`pin_core::spsc`] ring: it never allocates, locks, blocks,
//! signals the loader or reads files (D5). The loader polls
//! (`park_timeout`) for free space, a rewind or a stop request.
//!
//! # Passes and rewinds
//! The track plays its source in *passes*, numbered by an epoch the audio
//! thread owns. When a voice ends (end of source, failure, or stolen) the
//! audio thread bumps the epoch and drops what is buffered; the loader sees
//! the new epoch, restarts the decode from source frame 0 and records where
//! in the ring the new pass begins, so values of an older pass that were
//! still pushed after the bump are discarded unplayed.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::thread::{self, JoinHandle, Thread};
use std::time::Duration;

use pin_core::spsc::{SpscConsumer, SpscProducer, channel};

use super::decode::{DecodeBudget, DecodeError, FrameSink, Halt, max_frames, run_decode};

/// Default ring size of a streamed track, in frames (1.37 s at 48 kHz;
/// 256 KiB mono, 512 KiB stereo).
pub const STREAM_RING_FRAMES: usize = 1 << 16;

/// How long the loader parks between polls of the ring, the epoch and the
/// stop flag.
const LOADER_PARK: Duration = Duration::from_millis(2);

/// Flag bit of `Shared::pushed`: the pass `loader_epoch` has ended.
const END: u64 = 1 << 63;

/// Upper bound of one [`StreamHandle::wait_filled`] wait between re-checks.
const WAIT_SLICE: Duration = Duration::from_millis(10);

/// State shared by the loader, the audio thread and the handles. Counters
/// are in ring *values* (`channels` per frame).
struct Shared {
    channels: usize,
    capacity_values: u64,
    ring_bytes: u64,
    /// Set by the track's `Drop`.
    stop: AtomicBool,
    /// Cleared when the loader thread exits (even by unwinding).
    alive: AtomicBool,
    /// Pass the audio thread wants (starts at 1; bumped on rewind).
    epoch: AtomicU64,
    /// Pass the loader is producing (0 before the first one).
    loader_epoch: AtomicU64,
    /// Value of `pushed` when `loader_epoch` began.
    pass_start: AtomicU64,
    /// Values published in total (whole frames only), with [`END`] set once
    /// the current pass has ended (end of source or failure). The loader
    /// publishes a frame only when it knows another frame follows, or
    /// together with [`END`] in the same store, so the audio thread never
    /// sees "ring empty, pass not ended" after the last real frame.
    pushed: AtomicU64,
    /// Values popped (or discarded) in total.
    popped: AtomicU64,
    /// Last pass that reached the end of the source or failed.
    finished_epoch: AtomicU64,
    /// The loader hit a decode error in some pass.
    failed: AtomicBool,
    /// Frames the audio thread rendered as silence for lack of data.
    underruns: AtomicU64,
    /// The loader thread, for `wait_filled` to wake it (never the audio
    /// thread).
    loader: OnceLock<Thread>,
    /// Loader → waiters only; the audio thread never touches these.
    lock: Mutex<()>,
    cv: Condvar,
}

impl Shared {
    fn notify(&self) {
        let _g = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        self.cv.notify_all();
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::Acquire)
    }

    /// `wait_filled`'s condition: stopped, loader gone, current pass ended,
    /// or the loader is on the current pass and no frame fits in the ring.
    fn settled(&self) -> bool {
        if self.stopping() || !self.alive.load(Ordering::Acquire) {
            return true;
        }
        let e = self.epoch.load(Ordering::Acquire);
        if self.finished_epoch.load(Ordering::Acquire) == e {
            return true;
        }
        let used = (self.pushed.load(Ordering::Acquire) & !END)
            .saturating_sub(self.popped.load(Ordering::Acquire));
        self.loader_epoch.load(Ordering::Acquire) == e
            && used + self.channels as u64 > self.capacity_values
    }
}

/// A music track streamed from its compressed bytes (see [`open_stream`]).
///
/// Owned by the [`Mixer`](super::Mixer) once registered
/// ([`Mixer::register_stream`](super::Mixer::register_stream)). It is
/// `Send`. Dropping it stops and joins the loader thread and returns
/// exactly [`ring_bytes`](Self::ring_bytes) to the budget it was opened
/// with.
pub struct StreamedTrack {
    shared: Arc<Shared>,
    ring: SpscConsumer<f32>,
    loader: Option<JoinHandle<()>>,
    budget: Arc<DecodeBudget>,
    ring_frames: usize,
    /// Upper bound of the output length the header announced.
    max_out_frames: Option<u64>,
    /// Audio-thread copies of `Shared::epoch` / `Shared::popped`.
    epoch: u64,
    popped: u64,
}

/// A cloneable view of a [`StreamedTrack`]'s state for load and test
/// threads (`Send + Sync`). It stays valid after the track is dropped (it
/// then reports a stopped track).
#[derive(Clone)]
pub struct StreamHandle {
    shared: Arc<Shared>,
}

/// Opens `bytes` (a WAV or Ogg Vorbis file, owned by the track) as a
/// streamed track at `mixer_rate`, with a ring of `ring_frames` frames.
///
/// The header is validated synchronously with the same errors as
/// [`decode_with_budget`](super::decode_with_budget) (mixer rate, file
/// rate, channels, container, chained Ogg, and the 10-minute cap from the
/// WAV data-chunk size or last Ogg granule). The ring holds
/// `ring_frames` clamped to at least 1, to at most the 10-minute cap at
/// `mixer_rate` and, when the header gives the output length, to at most
/// that length; its bytes (`ring_frames × 4 × channels`) are charged to
/// `budget` before it is allocated, all or nothing
/// ([`DecodeError::BudgetExhausted`], nothing charged). The decoded length
/// is never charged. On success the `khz-music-loader` thread starts
/// filling the ring. Never panics.
pub fn open_stream(
    bytes: Vec<u8>,
    mixer_rate: u32,
    ring_frames: usize,
    budget: &Arc<DecodeBudget>,
) -> Result<StreamedTrack, DecodeError> {
    let mut probed = None;
    match run_decode::<Probe>(&bytes, mixer_rate, |layout| {
        probed = Some(*layout);
        Err(Halt::Stopped)
    }) {
        Ok(never) => match never {},
        Err(Halt::Failed(e)) => return Err(e),
        Err(Halt::Stopped) => {}
    }
    let Some(layout) = probed else {
        return Err(DecodeError::Malformed("no header".into()));
    };
    let channels = layout.channels;
    let mut frames = (ring_frames as u64).min(max_frames(mixer_rate));
    if let Some(len) = layout.max_out_frames {
        frames = frames.min(len);
    }
    let frames = frames.max(1);
    // ≤ the 10-minute cap (≤ 2^28 frames): fits in usize on every target.
    let ring_frames = frames as usize;
    let ring_bytes = frames * 4 * channels as u64;
    if !budget.try_charge_exact(ring_bytes) {
        return Err(DecodeError::BudgetExhausted);
    }
    let (tx, rx) = channel::<f32>(ring_frames * channels);
    let shared = Arc::new(Shared {
        channels,
        capacity_values: (ring_frames * channels) as u64,
        ring_bytes,
        stop: AtomicBool::new(false),
        alive: AtomicBool::new(true),
        epoch: AtomicU64::new(1),
        loader_epoch: AtomicU64::new(0),
        pass_start: AtomicU64::new(0),
        pushed: AtomicU64::new(0),
        popped: AtomicU64::new(0),
        finished_epoch: AtomicU64::new(0),
        failed: AtomicBool::new(false),
        underruns: AtomicU64::new(0),
        loader: OnceLock::new(),
        lock: Mutex::new(()),
        cv: Condvar::new(),
    });
    let loader_shared = Arc::clone(&shared);
    let spawned = thread::Builder::new()
        .name("khz-music-loader".into())
        .spawn(move || loader_main(&loader_shared, &bytes, mixer_rate, tx));
    let loader = match spawned {
        Ok(h) => h,
        Err(e) => {
            budget.refund_bytes(ring_bytes);
            return Err(DecodeError::Malformed(format!(
                "cannot start the music loader thread: {e}"
            )));
        }
    };
    Ok(StreamedTrack {
        shared,
        ring: rx,
        loader: Some(loader),
        budget: Arc::clone(budget),
        ring_frames,
        max_out_frames: layout.max_out_frames,
        epoch: 1,
        popped: 0,
    })
}

/// The header probe's sink: never built (the probe stops in `open`).
enum Probe {}

impl FrameSink for Probe {
    type Error = Halt;

    fn put(&mut self, _: f32, _: f32) -> Result<(), Halt> {
        match *self {}
    }
}

/// Clears `Shared::alive` when the loader exits, however it exits.
struct AliveGuard<'a>(&'a Shared);

impl Drop for AliveGuard<'_> {
    fn drop(&mut self) {
        self.0.alive.store(false, Ordering::Release);
        self.0.notify();
    }
}

fn loader_main(shared: &Shared, bytes: &[u8], mixer_rate: u32, mut tx: SpscProducer<f32>) {
    let _alive = AliveGuard(shared);
    let _ = shared.loader.set(thread::current());
    let mut pushed: u64 = 0;
    while !shared.stopping() {
        let epoch = shared.epoch.load(Ordering::Acquire);
        // Publishes every value of older passes (an unpublished frame of an
        // abandoned pass included) and clears `END` before the new pass
        // becomes visible through `loader_epoch`.
        shared.pushed.store(pushed, Ordering::Release);
        shared.pass_start.store(pushed, Ordering::Release);
        shared.loader_epoch.store(epoch, Ordering::Release);
        shared.notify();
        let result = run_decode(bytes, mixer_rate, |_| {
            Ok(RingSink {
                tx: &mut tx,
                shared,
                epoch,
                pushed: &mut pushed,
            })
        });
        match result {
            Ok(_) => {}
            Err(Halt::Stopped) => continue,
            Err(Halt::Failed(_)) => shared.failed.store(true, Ordering::Release),
        }
        // The held-back last frame and the end of the pass, in one store.
        shared.pushed.store(pushed | END, Ordering::Release);
        shared.finished_epoch.store(epoch, Ordering::Release);
        shared.notify();
        while !shared.stopping() && shared.epoch.load(Ordering::Acquire) == epoch {
            thread::park_timeout(LOADER_PARK);
        }
    }
}

/// The loader's sink: pushes whole frames into the ring, parking while it
/// is full; stops the decode when the track is dropped or rewound.
struct RingSink<'a> {
    tx: &'a mut SpscProducer<f32>,
    shared: &'a Shared,
    epoch: u64,
    pushed: &'a mut u64,
}

impl FrameSink for RingSink<'_> {
    type Error = Halt;

    fn put(&mut self, l: f32, r: f32) -> Result<(), Halt> {
        let ch = self.shared.channels;
        // Another frame follows the ones pushed so far: publish them.
        self.shared.pushed.store(*self.pushed, Ordering::Release);
        loop {
            if self.shared.stopping() || self.shared.epoch.load(Ordering::Acquire) != self.epoch {
                return Err(Halt::Stopped);
            }
            if self.tx.capacity() - self.tx.len() >= ch {
                // Room was checked; only this thread pushes.
                let _ = self.tx.push(l);
                if ch == 2 {
                    let _ = self.tx.push(r);
                }
                // Published by the next `put` or with the end of the pass.
                *self.pushed += ch as u64;
                return Ok(());
            }
            self.shared.notify();
            thread::park_timeout(LOADER_PARK);
        }
    }
}

impl StreamedTrack {
    /// Stored channels: 1 (mono) or 2 (stereo).
    pub fn channels(&self) -> usize {
        self.shared.channels
    }

    /// Ring size in frames (see [`open_stream`] for the clamping).
    pub fn ring_frames(&self) -> usize {
        self.ring_frames
    }

    /// Bytes charged to the budget: `ring_frames() × 4 × channels()`.
    pub fn ring_bytes(&self) -> u64 {
        self.shared.ring_bytes
    }

    /// A handle to observe (and wait for) the track from other threads.
    pub fn handle(&self) -> StreamHandle {
        StreamHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Underrun frames so far (audio thread or load time; atomics only).
    pub(super) fn underruns(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }

    /// The header announced zero output frames: a trigger starts no voice.
    pub(super) fn is_empty_source(&self) -> bool {
        self.max_out_frames == Some(0)
    }

    /// Pops and drops values until `popped == upto` (audio thread; bounded
    /// by the ring capacity).
    fn discard_until(&mut self, upto: u64) {
        while self.popped < upto {
            if self.ring.pop().is_none() {
                break;
            }
            self.popped += 1;
        }
        self.shared.popped.store(self.popped, Ordering::Release);
    }

    /// Audio thread: fills `buf` with up to `frames` frames of the current
    /// pass (`channels()` values each) and returns `(frames to mix, ended)`.
    /// Missing frames while the pass has not ended are silence (counted as
    /// underruns); once the pass has ended and the ring is drained, the
    /// voice ends after the frames returned. Never allocates, locks or
    /// blocks.
    pub(super) fn read(&mut self, buf: &mut [f32], frames: usize) -> (usize, bool) {
        let ch = self.shared.channels;
        let e = self.epoch;
        // The loader publishes the last frame of a pass and `END` in one
        // store, so an empty ring without `END` is a real underrun.
        let mut fin = false;
        let mut left = 0u64;
        if self.shared.loader_epoch.load(Ordering::Acquire) == e {
            let start = self.shared.pass_start.load(Ordering::Acquire);
            self.discard_until(start);
            let published = self.shared.pushed.load(Ordering::Acquire);
            fin = published & END != 0;
            left = (published & !END).saturating_sub(self.popped);
        }
        let frames = frames.min(buf.len() / ch);
        let avail = usize::try_from(left / ch as u64)
            .unwrap_or(usize::MAX)
            .min(frames);
        for slot in &mut buf[..avail * ch] {
            *slot = self.ring.pop().unwrap_or(0.0);
        }
        self.popped += (avail * ch) as u64;
        self.shared.popped.store(self.popped, Ordering::Release);
        if avail < frames {
            if fin {
                return (avail, true);
            }
            buf[avail * ch..frames * ch].fill(0.0);
            self.shared
                .underruns
                .fetch_add((frames - avail) as u64, Ordering::Relaxed);
            return (frames, false);
        }
        let drained = left - ((avail * ch) as u64) < ch as u64;
        (frames, fin && drained)
    }

    /// Audio thread: the voice ended (or was stolen); the next trigger
    /// plays from source frame 0. Bumps the epoch (so
    /// [`StreamHandle::is_finished`] reads `false` at once) and drops what
    /// is buffered; the loader refills in the background.
    pub(super) fn rewind(&mut self) {
        // Everything pushed so far belongs to older passes; read it before
        // publishing the new epoch so no value of the new pass is dropped.
        let stale = self.shared.pushed.load(Ordering::Acquire) & !END;
        self.epoch += 1;
        self.shared.epoch.store(self.epoch, Ordering::Release);
        self.discard_until(stale);
    }
}

impl Drop for StreamedTrack {
    /// Stops and joins the loader, then returns the ring's bytes.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        if let Some(h) = self.loader.take() {
            h.thread().unpark();
            let _ = h.join();
        }
        self.shared.notify();
        self.budget.refund_bytes(self.shared.ring_bytes);
    }
}

impl StreamHandle {
    /// Blocks the calling (load or test) thread until the ring is full for
    /// the current pass, the current pass has ended or failed, or the track
    /// was dropped. Wakes the loader to speed it up. Never call it on the
    /// audio thread.
    pub fn wait_filled(&self) {
        let sh = &*self.shared;
        loop {
            if sh.settled() {
                return;
            }
            if let Some(t) = sh.loader.get() {
                t.unpark();
            }
            let g = sh.lock.lock().unwrap_or_else(PoisonError::into_inner);
            if sh.settled() {
                return;
            }
            // Bounded wait: `settled` is re-checked after every slice.
            let _ = sh
                .cv
                .wait_timeout(g, WAIT_SLICE)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Frames of the current pass waiting in the ring.
    pub fn buffered_frames(&self) -> usize {
        let sh = &*self.shared;
        if sh.loader_epoch.load(Ordering::Acquire) != sh.epoch.load(Ordering::Acquire) {
            return 0;
        }
        let start = sh.pass_start.load(Ordering::Acquire);
        let popped = sh.popped.load(Ordering::Acquire).max(start);
        let values = (sh.pushed.load(Ordering::Acquire) & !END).saturating_sub(popped);
        usize::try_from(values / sh.channels as u64).unwrap_or(usize::MAX)
    }

    /// Frames rendered as silence because the ring was empty before the
    /// pass ended (one per missing frame, never reset).
    pub fn underrun_frames(&self) -> u64 {
        self.shared.underruns.load(Ordering::Relaxed)
    }

    /// The loader reached the end of the source (or failed) in the current
    /// pass. Reads `false` from the moment a voice ends until the next pass
    /// ends.
    pub fn is_finished(&self) -> bool {
        let sh = &*self.shared;
        sh.finished_epoch.load(Ordering::Acquire) == sh.epoch.load(Ordering::Acquire)
    }

    /// The loader met a decode error (corrupt data after a valid header) in
    /// some pass; the frames before it still play.
    pub fn failed(&self) -> bool {
        self.shared.failed.load(Ordering::Acquire)
    }

    /// Bytes the track's ring holds charged to its budget.
    pub fn ring_bytes(&self) -> u64 {
        self.shared.ring_bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::{
        Bus, Mixer, MixerConfig, Sample, SoundSender, SoundTrigger, decode, trigger_ring,
    };
    use pin_core::SoundId;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    use std::io::Cursor;

    // Allocation guard (D5): a counting global allocator, per thread, as
    // in `tests/mixer_alloc.rs`. Only the thread calling `render` is
    // counted; the loader thread allocates freely.
    struct Counting;

    thread_local! {
        static COUNT: Cell<u64> = const { Cell::new(0) };
    }

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
            unsafe { System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
            unsafe { System.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            let _ = COUNT.try_with(|c| c.set(c.get() + 1));
            unsafe { System.realloc(p, l, n) }
        }
    }

    #[global_allocator]
    static A: Counting = Counting;

    fn allocs<R>(f: impl FnOnce() -> R) -> (R, u64) {
        let before = COUNT.with(Cell::get);
        let r = f();
        (r, COUNT.with(Cell::get) - before)
    }

    const OGG: &[u8] = include_bytes!("fixtures/tone440_mono_22k.ogg");

    /// A 16-bit WAV of interleaved `vals`.
    fn wav(channels: u16, rate: u32, vals: &[i16]) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut buf = Vec::new();
        {
            let mut w = hound::WavWriter::new(Cursor::new(&mut buf), spec).unwrap();
            for &v in vals {
                w.write_sample(v).unwrap();
            }
            w.finalize().unwrap();
        }
        buf
    }

    /// Deterministic pseudo-random 16-bit values.
    fn noise(n: usize, seed: u32) -> Vec<i16> {
        let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 16) as u16 as i16
            })
            .collect()
    }

    /// `n` distinct small values `10·(k+1)` (exact in f32 after /32768).
    fn ramp(n: usize) -> Vec<i16> {
        (0..n).map(|k| (10 * (k + 1)) as i16).collect()
    }

    fn val(v: i16) -> f32 {
        (f64::from(v) / 32768.0) as f32
    }

    fn unlimited() -> Arc<DecodeBudget> {
        Arc::new(DecodeBudget::unlimited())
    }

    fn mixer(cfg: MixerConfig) -> (SoundSender, Mixer) {
        let (tx, rx) = trigger_ring(64);
        (tx, Mixer::new(cfg, rx))
    }

    fn trig(tx: &mut SoundSender, id: u16, gain: f32) {
        assert!(tx.send(SoundTrigger {
            sound: SoundId(id),
            gain,
        }));
    }

    fn left(out: &[f32]) -> Vec<f32> {
        out.iter().step_by(2).copied().collect()
    }

    /// SPEC §4 3.3 (#130): frames leave the ring in source order, the ring
    /// is sized as requested, and its bytes follow `Sample::bytes`' rule.
    #[test]
    fn ring_fill_and_drain_in_order() {
        let x = ramp(1_000);
        let budget = unlimited();
        let t = open_stream(wav(1, 48_000, &x), 48_000, 64, &budget).unwrap();
        assert_eq!(
            (t.channels(), t.ring_frames(), t.ring_bytes()),
            (1, 64, 256)
        );
        let h = t.handle();
        h.wait_filled();
        assert_eq!(h.buffered_frames(), 64);
        assert!(!h.is_finished() && !h.failed());
        assert_eq!(h.ring_bytes(), 256);
        assert_eq!(budget.used_bytes(), 256);
        let (mut tx, mut m) = mixer(MixerConfig::default());
        m.register_stream(SoundId(0), t, Bus::Music)
            .map_err(drop)
            .unwrap();
        assert!(m.is_registered(SoundId(0)));
        trig(&mut tx, 0, 1.0);
        let mut got = Vec::new();
        let mut out = vec![0.0f32; 2 * 50];
        for _ in 0..20 {
            h.wait_filled();
            m.render(&mut out);
            assert!(out.chunks(2).all(|c| c[0] == c[1]), "mono panned");
            got.extend(left(&out));
        }
        let want: Vec<f32> = x.iter().map(|&v| val(v)).collect();
        assert_eq!(&got[..1_000], &want[..]);
        assert!(got[1_000..].iter().all(|&v| v == 0.0));
        assert_eq!(m.active_voices(Bus::Music), 0);
        assert_eq!(m.stream_underruns(), 0);
        drop(m);
        assert_eq!(budget.used_bytes(), 0);
        // The handle outlives the track: it reports a stopped track.
        h.wait_filled();
    }

    /// Renders the same trigger script through a resident decode and a
    /// streamed track of `bytes` (sound 0, music) next to a resident
    /// callout (1, ducks the music) and sfx (2); `wait_filled` before each
    /// render. Returns both outputs as bit patterns.
    fn compare(bytes: &[u8], ring: usize, block: usize, blocks: usize) -> (Vec<u32>, Vec<u32>) {
        let cfg = MixerConfig {
            duck_attack_ms: 2.0,
            duck_release_ms: 5.0,
            master_gain: 1.3,
            ..MixerConfig::default()
        };
        let rate = cfg.sample_rate;
        let callout = Arc::new(Sample::from_mono(&[0.3; 700]));
        let sfx = Arc::new(Sample::constant(900, 0.2, -0.4));
        let script = [
            (0usize, 0u16, 0.8f32),
            (2, 1, 1.0),
            (3, 2, 0.7),
            (6, 1, 0.9),
        ];
        let run = |streamed: bool| {
            let budget = unlimited();
            let (mut tx, mut m) = mixer(cfg);
            let handle = if streamed {
                let t = open_stream(bytes.to_vec(), rate, ring, &budget).unwrap();
                let h = t.handle();
                m.register_stream(SoundId(0), t, Bus::Music)
                    .map_err(drop)
                    .unwrap();
                Some(h)
            } else {
                m.register(
                    SoundId(0),
                    Arc::new(decode(bytes, rate).unwrap()),
                    Bus::Music,
                );
                None
            };
            m.register(SoundId(1), Arc::clone(&callout), Bus::Callout);
            m.register(SoundId(2), Arc::clone(&sfx), Bus::Sfx);
            let mut out = vec![0.0f32; 2 * block];
            let mut all = Vec::new();
            let mut ended_at = None;
            for b in 0..blocks {
                for &(at, id, g) in &script {
                    if at == b {
                        trig(&mut tx, id, g);
                    }
                }
                // Retrigger the music once it has ended (plays from 0).
                if ended_at.is_some_and(|e| b == e + 2) {
                    trig(&mut tx, 0, 0.6);
                }
                if let Some(h) = &handle {
                    h.wait_filled();
                }
                m.render(&mut out);
                if b > 0 && ended_at.is_none() && m.active_voices(Bus::Music) == 0 {
                    ended_at = Some(b);
                }
                all.extend(out.iter().map(|v| v.to_bits()));
            }
            assert!(ended_at.is_some(), "music ended within the run");
            assert_eq!(m.stream_underruns(), 0);
            all
        };
        (run(false), run(true))
    }

    /// SPEC §4 3.3 (#130): a streamed mono WAV that needs resampling mixes
    /// bit-identically to the resident decode (ring smaller than the
    /// track, so it wraps; ducking, master gain, clipping, a rewind).
    #[test]
    fn streamed_mono_resampled_wav_matches_resident() {
        let bytes = wav(1, 22_050, &noise(3_000, 7));
        let (a, b) = compare(&bytes, 1_000, 300, 60);
        assert_eq!(a, b);
        assert!(
            a.iter().any(|&v| f32::from_bits(v).abs() == 1.0),
            "clip reached"
        );
    }

    /// SPEC §4 3.3 (#130): a streamed stereo WAV (44.1 kHz) mixes
    /// bit-identically to the resident decode.
    #[test]
    fn streamed_stereo_wav_matches_resident() {
        let bytes = wav(2, 44_100, &noise(2 * 4_000, 8));
        let (a, b) = compare(&bytes, 777, 256, 60);
        assert_eq!(a, b);
    }

    /// SPEC §4 3.3 (#130): the Ogg Vorbis fixture streams bit-identically
    /// to its resident decode.
    #[test]
    fn streamed_ogg_matches_resident() {
        let (a, b) = compare(OGG, 2_048, 512, 60);
        assert_eq!(a, b);
        assert!(a.iter().any(|&v| v != 0));
    }

    /// SPEC §4 3.3 (#130 a): an underrun renders silence, counts one per
    /// missing frame, and does not skip: the next real frame follows.
    #[test]
    fn underrun_is_silence_without_skipping() {
        let x = ramp(1_000);
        let t = open_stream(wav(1, 48_000, &x), 48_000, 10, &unlimited()).unwrap();
        let h = t.handle();
        let (mut tx, mut m) = mixer(MixerConfig::default());
        m.register_stream(SoundId(4), t, Bus::Music)
            .map_err(drop)
            .unwrap();
        trig(&mut tx, 4, 1.0);
        h.wait_filled();
        let mut out = vec![0.0f32; 2 * 256];
        m.render(&mut out);
        let l = left(&out);
        let want: Vec<f32> = x[..10].iter().map(|&v| val(v)).collect();
        assert_eq!(&l[..10], &want[..]);
        assert!(l[10..].iter().all(|&v| v == 0.0));
        assert_eq!(h.underrun_frames(), 246);
        assert_eq!(m.stream_underruns(), 246);
        assert_eq!(m.active_voices(Bus::Music), 1);
        h.wait_filled();
        let mut out = vec![0.0f32; 2 * 5];
        m.render(&mut out);
        let want: Vec<f32> = x[10..15].iter().map(|&v| val(v)).collect();
        assert_eq!(left(&out), want);
        assert_eq!(m.stream_underruns(), 246);
    }

    /// SPEC §4 3.3 (#130 c, ring sizing): the ring is clamped to the known
    /// length; after the voice ends the track rewinds (`is_finished` false
    /// at once) and the next trigger plays from frame 0 again.
    #[test]
    fn rewind_after_end() {
        let x = ramp(300);
        let t = open_stream(wav(1, 48_000, &x), 48_000, 1_000, &unlimited()).unwrap();
        assert_eq!(t.ring_frames(), 300);
        let h = t.handle();
        let (mut tx, mut m) = mixer(MixerConfig::default());
        m.register_stream(SoundId(0), t, Bus::Music)
            .map_err(drop)
            .unwrap();
        let mut passes = Vec::new();
        for _ in 0..3 {
            h.wait_filled();
            assert!(h.is_finished(), "the whole track fits in the ring");
            assert_eq!(h.buffered_frames(), 300);
            trig(&mut tx, 0, 1.0);
            let mut out = vec![0.0f32; 2 * 512];
            m.render(&mut out);
            assert_eq!(m.active_voices(Bus::Music), 0);
            assert!(!h.is_finished(), "a new pass starts when the voice ends");
            passes.push(left(&out));
        }
        let want: Vec<f32> = x.iter().map(|&v| val(v)).collect();
        assert_eq!(&passes[0][..300], &want[..]);
        assert!(passes.iter().all(|p| *p == passes[0]));
        assert_eq!(h.underrun_frames(), 0);
    }

    /// SPEC §4 3.3 (#130 c): a stolen streamed voice rewinds its track;
    /// stale frames never play.
    #[test]
    fn rewind_after_steal() {
        let x = ramp(2_000);
        let t = open_stream(wav(1, 48_000, &x), 48_000, 500, &unlimited()).unwrap();
        let h = t.handle();
        let cfg = MixerConfig {
            max_voices: [1, 4, 24],
            ..MixerConfig::default()
        };
        let (mut tx, mut m) = mixer(cfg);
        m.register_stream(SoundId(0), t, Bus::Music)
            .map_err(drop)
            .unwrap();
        m.register(
            SoundId(1),
            Arc::new(Sample::constant(1_000, 0.0, 0.0)),
            Bus::Music,
        );
        let mut out = vec![0.0f32; 2 * 100];
        h.wait_filled();
        trig(&mut tx, 0, 1.0);
        m.render(&mut out);
        let first = left(&out);
        trig(&mut tx, 1, 1.0); // steals the stream
        m.render(&mut out);
        assert_eq!(m.stolen_voices(), 1);
        assert!(!h.is_finished());
        h.wait_filled();
        assert_eq!(h.buffered_frames(), 500);
        trig(&mut tx, 0, 1.0); // steals the resident voice back
        m.render(&mut out);
        assert_eq!(left(&out), first);
        assert_eq!(m.stolen_voices(), 2);
        assert_eq!(m.stream_underruns(), 0);
    }

    /// SPEC §4 3.3 (#130 b): one voice per streamed track; a trigger while
    /// it plays is ignored (no voice, no steal). A zero-length track starts
    /// nothing.
    #[test]
    fn one_voice_per_track_and_empty_track() {
        let x = ramp(1_000);
        let t = open_stream(wav(1, 48_000, &x), 48_000, 1_000, &unlimited()).unwrap();
        let h = t.handle();
        let (mut tx, mut m) = mixer(MixerConfig::default());
        m.register_stream(SoundId(0), t, Bus::Music)
            .map_err(drop)
            .unwrap();
        h.wait_filled();
        trig(&mut tx, 0, 1.0);
        trig(&mut tx, 0, 1.0);
        let mut out = vec![0.0f32; 2 * 100];
        m.render(&mut out);
        assert_eq!(m.active_voices(Bus::Music), 1);
        trig(&mut tx, 0, 2.0);
        m.render(&mut out);
        assert_eq!(m.active_voices(Bus::Music), 1);
        assert_eq!(m.stolen_voices(), 0);
        let want: Vec<f32> = x[100..200].iter().map(|&v| val(v)).collect();
        assert_eq!(left(&out), want, "gain 1, not doubled, not restarted");

        let empty = open_stream(wav(2, 48_000, &[]), 48_000, 1_000, &unlimited()).unwrap();
        assert_eq!((empty.ring_frames(), empty.ring_bytes()), (1, 8));
        let eh = empty.handle();
        m.register_stream(SoundId(5), empty, Bus::Music)
            .map_err(drop)
            .unwrap();
        eh.wait_filled();
        assert!(eh.is_finished());
        trig(&mut tx, 5, 1.0);
        m.render(&mut out);
        // Only sound 0's voice: the empty track started none.
        assert_eq!(m.active_voices(Bus::Music), 1);
        assert_eq!(m.stream_underruns(), 0);
    }

    /// SPEC §4 3.3 (#130): only the music bus takes a streamed track; any
    /// other bus hands it back unchanged.
    #[test]
    fn non_music_bus_is_rejected() {
        let budget = unlimited();
        let (_tx, mut m) = mixer(MixerConfig::default());
        for bus in [Bus::Sfx, Bus::Callout] {
            let t = open_stream(wav(2, 48_000, &ramp(200)), 48_000, 100, &budget).unwrap();
            let back = m.register_stream(SoundId(3), t, bus).err().unwrap();
            assert_eq!((back.ring_frames(), back.ring_bytes()), (100, 800));
            assert!(!m.is_registered(SoundId(3)));
            assert_eq!(budget.used_bytes(), 800);
            drop(back);
            assert_eq!(budget.used_bytes(), 0);
        }
    }

    /// D25, #129 (#130 budget): the ring is charged at open, all or
    /// nothing; dropping, replacing or dropping the mixer returns it.
    #[test]
    fn budget_charge_and_release() {
        let bytes = wav(2, 48_000, &noise(2 * 5_000, 3));
        let budget = Arc::new(DecodeBudget::new(10_000));
        let a = open_stream(bytes.clone(), 48_000, 1_000, &budget).unwrap();
        assert_eq!(budget.used_bytes(), 8_000);
        assert_eq!(
            open_stream(bytes.clone(), 48_000, 1_000, &budget).err(),
            Some(DecodeError::BudgetExhausted)
        );
        assert_eq!(budget.used_bytes(), 8_000);
        let b = open_stream(bytes.clone(), 48_000, 250, &budget).unwrap();
        assert_eq!(budget.used_bytes(), 10_000);
        drop(a);
        assert_eq!(budget.used_bytes(), 2_000);
        let (mut tx, mut m) = mixer(MixerConfig::default());
        m.register_stream(SoundId(0), b, Bus::Music)
            .map_err(drop)
            .unwrap();
        trig(&mut tx, 0, 1.0);
        let mut out = vec![0.0f32; 2 * 64];
        m.render(&mut out);
        // Replace a playing stream by a stream: the old voice stops and
        // the old ring returns.
        let c = open_stream(bytes.clone(), 48_000, 500, &budget).unwrap();
        assert_eq!(budget.used_bytes(), 6_000);
        m.register_stream(SoundId(0), c, Bus::Music)
            .map_err(drop)
            .unwrap();
        assert_eq!(m.active_voices(Bus::Music), 0);
        assert_eq!(budget.used_bytes(), 4_000);
        // Replace by a resident sample.
        m.register(
            SoundId(0),
            Arc::new(Sample::constant(10, 0.0, 0.0)),
            Bus::Music,
        );
        assert_eq!(budget.used_bytes(), 0);
        let d = open_stream(bytes, 48_000, 1_250, &budget).unwrap();
        assert_eq!(budget.used_bytes(), 10_000);
        m.register_stream(SoundId(9), d, Bus::Music)
            .map_err(drop)
            .unwrap();
        drop(m);
        assert_eq!(budget.used_bytes(), 0);
    }

    /// Sets a WAV's data-chunk size field.
    fn claim_data_len(bytes: &mut [u8], len: u32) {
        let at = (0..bytes.len() - 4)
            .rev()
            .find(|&i| &bytes[i..i + 4] == b"data")
            .unwrap();
        bytes[at + 4..at + 8].copy_from_slice(&len.to_le_bytes());
    }

    /// D25 (#130 header): `open_stream` validates the header synchronously
    /// with `decode_with_budget`'s errors and charges nothing on error;
    /// exactly 600 s is accepted, one frame more is `TooLong`.
    #[test]
    fn header_validation_matches_decode() {
        let budget = Arc::new(DecodeBudget::new(1 << 20));
        let ok = wav(1, 48_000, &ramp(10));
        let open = |b: &[u8], rate: u32| open_stream(b.to_vec(), rate, 64, &budget).err();
        assert_eq!(open(&ok, 0), Some(DecodeError::Unsupported));
        assert_eq!(
            open(&ok, MAX_TEST_MIXER + 1),
            Some(DecodeError::Unsupported)
        );
        assert_eq!(open(b"", 48_000), Some(DecodeError::Unsupported));
        assert_eq!(
            open(b"ID3\x04 rest", 48_000),
            Some(DecodeError::Unsupported)
        );
        assert_eq!(
            open(&wav(1, 7_999, &[0]), 48_000),
            Some(DecodeError::Unsupported)
        );
        let three = {
            let spec = hound::WavSpec {
                channels: 3,
                sample_rate: 48_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut buf = Vec::new();
            let mut w = hound::WavWriter::new(Cursor::new(&mut buf), spec).unwrap();
            for _ in 0..3 {
                w.write_sample(0i16).unwrap();
            }
            w.finalize().unwrap();
            buf
        };
        assert_eq!(open(&three, 48_000), Some(DecodeError::TooManyChannels));
        let mut long = wav(1, 8_000, &[0; 4]);
        claim_data_len(&mut long, 600 * 8_000 * 2);
        assert_eq!(open(&long, 8_000), None, "exactly 600 s");
        claim_data_len(&mut long, 600 * 8_000 * 2 + 2);
        assert_eq!(open(&long, 8_000), Some(DecodeError::TooLong));
        assert_eq!(decode(&long, 8_000), Err(DecodeError::TooLong));
        // Chained Ogg: the fixture twice, the second with another serial.
        let mut chained = OGG.to_vec();
        let mut second = OGG.to_vec();
        let mut p = 0;
        while p + 27 <= second.len() && &second[p..p + 4] == b"OggS" {
            second[p + 14] ^= 0xff;
            let segs = usize::from(second[p + 26]);
            let body: usize = second[p + 27..p + 27 + segs]
                .iter()
                .map(|&x| usize::from(x))
                .sum();
            p += 27 + segs + body;
        }
        chained.extend_from_slice(&second);
        assert_eq!(open(&chained, 48_000), Some(DecodeError::Unsupported));
        assert_eq!(decode(&chained, 48_000), Err(DecodeError::Unsupported));
        assert!(matches!(
            open(b"OggS\0\0\0", 48_000),
            Some(DecodeError::Malformed(_))
        ));
        assert_eq!(budget.used_bytes(), 0);
        // A huge request is clamped to the 10-minute cap (here the known
        // length, 4 frames) and charged accordingly.
        let t = open_stream(long_ok(), 8_000, usize::MAX, &budget).unwrap();
        assert_eq!(t.ring_frames(), 4);
    }

    const MAX_TEST_MIXER: u32 = crate::mixer::MAX_MIXER_RATE;

    fn long_ok() -> Vec<u8> {
        wav(1, 8_000, &[0; 4])
    }

    /// D25 (#130 d): corrupt data after a valid header: the buffered frames
    /// play, `failed()` is set, then the voice ends (not an underrun).
    #[test]
    fn loader_failure_mid_stream_ends_the_voice() {
        let x = ramp(1_000);
        let full = wav(1, 48_000, &x);
        // Keep the header, 500 frames and one byte of the 501st.
        let cut = full.len() - 2 * 500 + 1;
        let bytes = full[..cut].to_vec();
        assert!(matches!(
            decode(&bytes, 48_000),
            Err(DecodeError::Malformed(_))
        ));
        let t = open_stream(bytes, 48_000, 4_096, &unlimited()).unwrap();
        let h = t.handle();
        let (mut tx, mut m) = mixer(MixerConfig::default());
        m.register_stream(SoundId(0), t, Bus::Music)
            .map_err(drop)
            .unwrap();
        trig(&mut tx, 0, 1.0);
        let mut got = Vec::new();
        let mut out = vec![0.0f32; 2 * 128];
        for _ in 0..10 {
            h.wait_filled();
            m.render(&mut out);
            got.extend(left(&out));
            if m.active_voices(Bus::Music) == 0 {
                break;
            }
        }
        assert_eq!(m.active_voices(Bus::Music), 0);
        assert!(h.failed());
        let want: Vec<f32> = x[..500].iter().map(|&v| val(v)).collect();
        assert_eq!(&got[..500], &want[..]);
        assert!(got[500..].iter().all(|&v| v == 0.0));
        assert_eq!(m.stream_underruns(), 0);
    }

    /// D5 (#130): `render` with a streamed voice never allocates: playing,
    /// underrunning, ending, rewinding, being stolen and retriggering.
    #[test]
    fn render_with_streams_does_not_allocate() {
        let budget = unlimited();
        let short = open_stream(wav(1, 22_050, &ramp(300)), 48_000, 1_000, &budget).unwrap();
        let long = open_stream(wav(2, 48_000, &noise(2 * 3_000, 5)), 48_000, 100, &budget).unwrap();
        let (hs, hl) = (short.handle(), long.handle());
        let cfg = MixerConfig {
            max_voices: [1, 4, 24],
            ..MixerConfig::default()
        };
        let (mut tx, mut m) = mixer(cfg);
        m.register_stream(SoundId(0), short, Bus::Music)
            .map_err(drop)
            .unwrap();
        m.register_stream(SoundId(1), long, Bus::Music)
            .map_err(drop)
            .unwrap();
        m.register(
            SoundId(2),
            Arc::new(Sample::constant(400, 0.1, 0.1)),
            Bus::Callout,
        );
        let mut out = vec![0.0f32; 2 * 256];
        let mut total = 0;
        for round in 0..30u16 {
            hs.wait_filled();
            hl.wait_filled();
            let ((), a) = allocs(|| {
                for k in 0..3u16 {
                    tx.send(SoundTrigger {
                        sound: SoundId((round + k) % 3),
                        gain: 0.7,
                    });
                }
                m.render(&mut out);
                m.render(&mut out[..2 * 100 + 1]);
                let _ = m.stream_underruns();
            });
            total += a;
        }
        assert!(m.stolen_voices() > 0, "steals exercised");
        assert!(m.stream_underruns() > 0, "underruns exercised");
        assert_eq!(total, 0, "allocations in render");
    }

    /// Acceptance (#130): a 10-minute music track (exactly 600 s) plays
    /// with memory bounded by the ring (the budget holds one ring, not the
    /// decoded length) and output equal to the full decode, block by block.
    #[test]
    fn ten_minute_track_streams_like_a_full_decode() {
        let src = 8_000;
        let rate = 16_000;
        let x: Vec<i16> = (0..600 * src as usize)
            .map(|k| ((k as u32).wrapping_mul(2_654_435_761) >> 16) as u16 as i16)
            .collect();
        let bytes = wav(1, src, &x);
        drop(x);
        let resident = decode(&bytes, rate).unwrap();
        assert_eq!(resident.frames(), 600 * rate as usize);
        let budget = unlimited();
        let t = open_stream(bytes, rate, STREAM_RING_FRAMES, &budget).unwrap();
        assert_eq!(t.ring_bytes(), (STREAM_RING_FRAMES * 4) as u64);
        assert_eq!(budget.used_bytes(), t.ring_bytes());
        assert!(budget.used_bytes() * 100 < resident.bytes());
        let h = t.handle();
        let cfg = MixerConfig {
            sample_rate: rate,
            ..MixerConfig::default()
        };
        let (mut ta, mut a) = mixer(cfg);
        let (mut tb, mut b) = mixer(cfg);
        a.register(SoundId(0), Arc::new(resident), Bus::Music);
        b.register_stream(SoundId(0), t, Bus::Music)
            .map_err(drop)
            .unwrap();
        trig(&mut ta, 0, 0.9);
        trig(&mut tb, 0, 0.9);
        let mut oa = vec![0.0f32; 2 * 256];
        let mut ob = vec![0.0f32; 2 * 256];
        let mut blocks = 0usize;
        let mut alloc_total = 0;
        loop {
            if h.buffered_frames() < 256 {
                h.wait_filled();
            }
            a.render(&mut oa);
            let ((), n) = allocs(|| b.render(&mut ob));
            alloc_total += n;
            blocks += 1;
            assert!(
                oa.iter().zip(&ob).all(|(p, q)| p.to_bits() == q.to_bits()),
                "block {blocks}"
            );
            assert_eq!(a.active_voices(Bus::Music), b.active_voices(Bus::Music));
            if a.active_voices(Bus::Music) == 0 {
                break;
            }
            assert_eq!(budget.used_bytes(), (STREAM_RING_FRAMES * 4) as u64);
        }
        assert_eq!(blocks, 600 * rate as usize / 256);
        assert_eq!(b.stream_underruns(), 0);
        assert_eq!(alloc_total, 0);
        drop(b);
        assert_eq!(budget.used_bytes(), 0);
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig { cases: 64, ..Default::default() })]

        /// R11, D25 (#130): bounded random bytes (raw, or behind a WAV or
        /// Ogg magic) never panic `open_stream`; an error leaves nothing
        /// charged and a dropped track returns its ring.
        #[test]
        fn random_bytes_never_panic_or_leak(
            body in proptest::collection::vec(proptest::num::u8::ANY, 0..600),
            prefix in 0usize..3,
            limit in proptest::sample::select(vec![0u64, 64, 4_096, u64::MAX]),
        ) {
            let mut bytes = match prefix {
                0 => Vec::new(),
                1 => b"RIFF\x24\x08\0\0WAVEfmt ".to_vec(),
                _ => b"OggS\0\x02".to_vec(),
            };
            bytes.extend_from_slice(&body);
            let budget = Arc::new(DecodeBudget::new(limit));
            match open_stream(bytes, 48_000, 256, &budget) {
                Err(_) => proptest::prop_assert_eq!(budget.used_bytes(), 0),
                Ok(t) => {
                    proptest::prop_assert_eq!(budget.used_bytes(), t.ring_bytes());
                    t.handle().wait_filled();
                    drop(t);
                    proptest::prop_assert_eq!(budget.used_bytes(), 0);
                }
            }
        }

        /// R11, D25 (#130 d): a valid WAV or Ogg whose tail is truncated or
        /// corrupted never hangs the mixer: the voice ends within a bounded
        /// number of renders and nothing panics.
        #[test]
        fn corrupt_tails_end_the_voice(
            ogg in proptest::bool::ANY,
            cut in 0.0f64..1.0,
            flips in proptest::collection::vec((0.0f64..1.0, 1u8..=255), 0..8),
        ) {
            let (mut bytes, keep) = if ogg {
                (OGG.to_vec(), 3_000usize)
            } else {
                (wav(1, 8_000, &noise(2_000, 11)), 44usize)
            };
            let len = keep + ((bytes.len() - keep) as f64 * cut) as usize;
            bytes.truncate(len.max(keep));
            let span = bytes.len() - keep;
            for (at, x) in flips {
                if span > 0 {
                    let i = keep + ((span as f64 * at) as usize).min(span - 1);
                    bytes[i] ^= x;
                }
            }
            let budget = unlimited();
            let rate = 22_050;
            let Ok(t) = open_stream(bytes, rate, 1_024, &budget) else {
                proptest::prop_assert_eq!(budget.used_bytes(), 0);
                return Ok(());
            };
            let h = t.handle();
            let (mut tx, mut m) = mixer(MixerConfig { sample_rate: rate, ..MixerConfig::default() });
            m.register_stream(SoundId(0), t, Bus::Music).map_err(drop).unwrap();
            trig(&mut tx, 0, 1.0);
            let mut out = vec![0.0f32; 2 * 256];
            let mut ended = false;
            for _ in 0..4_000 {
                h.wait_filled();
                m.render(&mut out);
                if m.active_voices(Bus::Music) == 0 {
                    ended = true;
                    break;
                }
            }
            proptest::prop_assert!(ended);
            proptest::prop_assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
        }
    }
}
