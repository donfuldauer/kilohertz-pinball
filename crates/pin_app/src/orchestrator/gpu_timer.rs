//! GPU frame time from timestamp queries (7.1 hook 3).
//!
//! The render thread brackets one frame's GPU work (every surface it draws
//! in a [`AppSurfaces::render`](super::AppSurfaces::render) call) with two
//! GPU timestamps and records their difference into the profiler's
//! [`PresentProfiler`]. Host time (`now_ns()` around acquire / present)
//! is never used: it measures presentation pacing, not GPU execution
//! (decision "7.1 — hooks requested outside owned paths").
//!
//! Mechanism (decision "w8f-q6 — profiler wiring"):
//!
//! * Each timestamp is written by an **empty compute pass** with
//!   `ComputePassTimestampWrites` (needs only
//!   [`wgpu::Features::TIMESTAMP_QUERY`], unlike
//!   `CommandEncoder::write_timestamp`, which also needs
//!   `TIMESTAMP_QUERY_INSIDE_ENCODERS`), in a command buffer of its own
//!   that travels **in the same `Queue::submit`** as the frame's work
//!   ([`GpuFrameTimer::bracket`]): the start pass before the frame's first
//!   command buffer, an end pass after each of its command buffers (the
//!   last one written wins). So the interval covers the GPU work from the
//!   frame's first submit to its last, and never the CPU's wait in
//!   `get_current_texture` (vsync pacing) before the first submit. The
//!   frame's own encoders are untouched: the rendered bytes are the same
//!   with or without timing.
//! * [`GpuFrameTimer::end_frame`] resolves the two queries into a resolve
//!   buffer and copies them into the frame's readback buffer, then
//!   requests a map (`map_async`). [`GpuFrameTimer::collect`] polls the
//!   device without waiting (`PollType::Poll`) and records every mapped
//!   result: one or more frames later, never blocking the render thread.
//! * A ring of [`GPU_TIMER_RING`] frames (query pairs and readback
//!   buffers) is allocated once in [`GpuFrameTimer::new`]; a frame whose
//!   ring entry is still in flight is simply not timed.
//! * A sample with a zero timestamp (some drivers), an end not after the
//!   start, or a non-positive tick period is discarded
//!   ([`gpu_sample_ns`]).
//!
//! Without `TIMESTAMP_QUERY` (or without compute support) on the device,
//! [`GpuFrameTimer::new`] returns `None` and nothing is recorded, so the
//! HUD's GPU line keeps its "no data" state (`--`, `n 0`).

use crate::profiler::PresentProfiler;
use arrayvec::ArrayVec;
use pin_render::gpu::{GpuContext, wgpu};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Frames whose timestamps can be in flight at once.
pub const GPU_TIMER_RING: usize = 4;

/// Bytes of one resolved query pair (two `u64` timestamps).
const PAIR_BYTES: u64 = 16;

/// The GPU time in ns between two raw timestamps `start` and `end` (ticks)
/// on a queue whose tick period is `period_ns`
/// ([`wgpu::Queue::get_timestamp_period`]), or `None` for a sample to
/// discard: a zero timestamp (unwritten or unsupported on some drivers),
/// `end <= start`, a period that is not a positive finite number, or a
/// product below 1 ns. Saturates at `u64::MAX`.
#[must_use]
pub fn gpu_sample_ns(start: u64, end: u64, period_ns: f32) -> Option<u64> {
    if start == 0 || end == 0 || end <= start || !(period_ns.is_finite() && period_ns > 0.0) {
        return None;
    }
    let ns = (end - start) as f64 * f64::from(period_ns);
    // `as` saturates; NaN cannot occur (finite positive factors).
    (ns >= 1.0).then_some(ns as u64)
}

/// Ring entry states (the map callback runs on the polling thread).
const FREE: u8 = 0;
const PENDING: u8 = 1;
const MAPPED: u8 = 2;
const FAILED: u8 = 3;

struct Entry {
    readback: wgpu::Buffer,
    state: Arc<AtomicU8>,
}

/// Times frames on the GPU with timestamp queries and records them into a
/// [`PresentProfiler`] (see the [module docs](self)). Render thread only.
pub struct GpuFrameTimer {
    device: wgpu::Device,
    query_set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    entries: Vec<Entry>,
    /// The entry the next frame uses.
    next: usize,
    /// The entry of the frame between `begin_frame` and `end_frame`.
    open: Option<usize>,
    /// The open frame's start timestamp was submitted ([`Self::bracket`]).
    begun: bool,
    period_ns: f32,
    profiler: PresentProfiler,
    recorded: u64,
    discarded: u64,
}

impl std::fmt::Debug for GpuFrameTimer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuFrameTimer")
            .field("period_ns", &self.period_ns)
            .field("recorded", &self.recorded)
            .field("discarded", &self.discarded)
            .finish_non_exhaustive()
    }
}

impl GpuFrameTimer {
    /// Whether `ctx`'s device can time frames: it enabled
    /// [`wgpu::Features::TIMESTAMP_QUERY`] (requested by
    /// [`GpuContext::with_instance`] whenever the adapter offers it) and
    /// the adapter runs compute passes.
    #[must_use]
    pub fn supported(ctx: &GpuContext) -> bool {
        ctx.device()
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            && ctx
                .adapter()
                .get_downlevel_capabilities()
                .flags
                .contains(wgpu::DownlevelFlags::COMPUTE_SHADERS)
    }

    /// A timer on `ctx` recording into `profiler`, with its ring allocated
    /// here (the only allocation besides wgpu's per-frame command
    /// buffers), or `None` when the device cannot time frames
    /// ([`supported`](Self::supported)): then nothing is ever recorded.
    #[must_use]
    pub fn new(ctx: &GpuContext, profiler: PresentProfiler) -> Option<GpuFrameTimer> {
        if !Self::supported(ctx) {
            return None;
        }
        let device = ctx.device().clone();
        let ring = GPU_TIMER_RING as u32;
        let query_set = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("gpu frame timer"),
            ty: wgpu::QueryType::Timestamp,
            count: 2 * ring,
        });
        let stride = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
        let resolve = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu frame timer resolve"),
            size: stride * u64::from(ring),
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let entries = (0..GPU_TIMER_RING)
            .map(|_| Entry {
                readback: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("gpu frame timer readback"),
                    size: PAIR_BYTES,
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                state: Arc::new(AtomicU8::new(FREE)),
            })
            .collect();
        Some(GpuFrameTimer {
            device,
            query_set,
            resolve,
            entries,
            next: 0,
            open: None,
            begun: false,
            period_ns: ctx.queue().get_timestamp_period(),
            profiler,
            recorded: 0,
            discarded: 0,
        })
    }

    /// ns per timestamp tick of the queue.
    #[must_use]
    pub fn period_ns(&self) -> f32 {
        self.period_ns
    }

    /// Samples recorded into the profiler so far.
    #[must_use]
    pub fn recorded(&self) -> u64 {
        self.recorded
    }

    /// Read-back samples discarded so far ([`gpu_sample_ns`] returned
    /// `None`, or the map failed).
    #[must_use]
    pub fn discarded(&self) -> u64 {
        self.discarded
    }

    /// Frames whose timestamps are resolved and not yet collected.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.state.load(Ordering::Acquire) != FREE)
            .count()
    }

    fn pass(&self, encoder: &mut wgpu::CommandEncoder, begin: Option<u32>, end: Option<u32>) {
        let _pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("gpu frame timestamp"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &self.query_set,
                beginning_of_pass_write_index: begin,
                end_of_pass_write_index: end,
            }),
        });
    }

    /// Starts timing a frame (no GPU work yet: the timestamps travel with
    /// the frame's submits, [`bracket`](Self::bracket)). The frame is not
    /// timed when the ring's next entry is still in flight.
    pub fn begin_frame(&mut self) {
        let i = self.next;
        self.begun = false;
        self.open = (self.entries[i].state.load(Ordering::Acquire) == FREE).then_some(i);
    }

    fn timestamp_buffer(&self, begin: Option<u32>, end: Option<u32>) -> wgpu::CommandBuffer {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu frame timestamp"),
            });
        self.pass(&mut encoder, begin, end);
        encoder.finish()
    }

    /// The command buffers to submit (in one `Queue::submit`, in order)
    /// for one of the open frame's command buffers `frame`: on the frame's
    /// first submit a start-timestamp buffer before it, and an
    /// end-timestamp buffer after it (a later submit of the frame
    /// overwrites the end). Just `frame` when no frame is being timed.
    pub fn bracket(&mut self, frame: wgpu::CommandBuffer) -> ArrayVec<wgpu::CommandBuffer, 3> {
        let mut out = ArrayVec::new();
        let Some(i) = self.open else {
            out.push(frame);
            return out;
        };
        let q = 2 * i as u32;
        if !self.begun {
            out.push(self.timestamp_buffer(Some(q), None));
            self.begun = true;
        }
        out.push(frame);
        out.push(self.timestamp_buffer(None, Some(q + 1)));
        out
    }

    /// Ends the frame begun by [`begin_frame`](Self::begin_frame). When
    /// some of its work was submitted through [`bracket`](Self::bracket),
    /// hands `submit` a command buffer that resolves the timestamp pair
    /// and copies it to the entry's readback buffer, then requests its map
    /// (read later by [`collect`](Self::collect)). Otherwise the frame is
    /// not timed and its entry is reused.
    pub fn end_frame(&mut self, submit: &mut dyn FnMut(wgpu::CommandBuffer)) {
        let Some(i) = self.open.take() else {
            return;
        };
        if !std::mem::take(&mut self.begun) {
            return;
        }
        let q = 2 * i as u32;
        let offset = i as u64 * wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu frame timer resolve"),
            });
        encoder.resolve_query_set(&self.query_set, q..q + 2, &self.resolve, offset);
        let entry = &self.entries[i];
        encoder.copy_buffer_to_buffer(&self.resolve, offset, &entry.readback, 0, PAIR_BYTES);
        submit(encoder.finish());
        entry.state.store(PENDING, Ordering::Release);
        let state = Arc::clone(&entry.state);
        entry.readback.map_async(wgpu::MapMode::Read, .., move |r| {
            state.store(if r.is_ok() { MAPPED } else { FAILED }, Ordering::Release);
        });
        self.next = (i + 1) % self.entries.len();
    }

    /// Polls the device without waiting and records every frame whose
    /// timestamps have been read back (in ring order). Returns the samples
    /// recorded by this call.
    pub fn collect(&mut self) -> usize {
        let _ = self.device.poll(wgpu::PollType::Poll);
        let mut n = 0;
        for e in &self.entries {
            match e.state.load(Ordering::Acquire) {
                MAPPED => {
                    let pair = e.readback.get_mapped_range(..).ok().map(|m| {
                        let word = |k: usize| {
                            let mut b = [0u8; 8];
                            b.copy_from_slice(&m[8 * k..8 * k + 8]);
                            u64::from_le_bytes(b)
                        };
                        (word(0), word(1))
                    });
                    e.readback.unmap();
                    match pair.and_then(|(s, t)| gpu_sample_ns(s, t, self.period_ns)) {
                        Some(ns) => {
                            self.profiler.record_frame_ns(ns);
                            self.recorded += 1;
                            n += 1;
                        }
                        None => self.discarded += 1,
                    }
                    e.state.store(FREE, Ordering::Release);
                }
                FAILED => {
                    self.discarded += 1;
                    e.state.store(FREE, Ordering::Release);
                }
                _ => {}
            }
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn zero_reversed_and_bad_period_samples_are_discarded() {
        assert_eq!(gpu_sample_ns(0, 10, 1.0), None);
        assert_eq!(gpu_sample_ns(10, 0, 1.0), None);
        assert_eq!(gpu_sample_ns(10, 10, 1.0), None);
        assert_eq!(gpu_sample_ns(11, 10, 1.0), None);
        for p in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(gpu_sample_ns(10, 20, p), None, "period {p}");
        }
        // Below 1 ns after scaling.
        assert_eq!(gpu_sample_ns(10, 11, 0.5), None);
        assert_eq!(gpu_sample_ns(10, 12, 0.5), Some(1));
        assert_eq!(gpu_sample_ns(1_000, 2_000, 1.0), Some(1_000));
        assert_eq!(gpu_sample_ns(1_000, 2_000, 41.666_668), Some(41_666));
        assert_eq!(gpu_sample_ns(1, u64::MAX, f32::MAX), Some(u64::MAX));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// A kept sample is positive and within rounding of
        /// `(end - start) · period`; it is kept exactly when both
        /// timestamps are nonzero, `end > start`, the period is positive
        /// and the product reaches 1 ns.
        #[test]
        fn samples_scale_by_the_period(
            start in 0u64..1u64 << 50,
            delta in 0u64..1u64 << 40,
            reversed in any::<bool>(),
            period in 0.01f32..100.0,
        ) {
            let (s, e) = if reversed { (start + delta, start) } else { (start, start + delta) };
            let exact = delta as f64 * f64::from(period);
            match gpu_sample_ns(s, e, period) {
                Some(ns) => {
                    prop_assert!(s != 0 && e > s);
                    prop_assert!(ns >= 1);
                    prop_assert!((ns as f64 - exact).abs() <= 1.0 + exact * 1e-12, "{ns} vs {exact}");
                }
                None => prop_assert!(s == 0 || e <= s || exact < 1.0),
            }
        }
    }
}
