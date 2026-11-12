//! The engine threads: sim (1 kHz, real time), render, audio and I/O
//! (placeholders that park until shutdown). The main thread stays the SDL
//! pump (D6) and only talks to these threads through rings, the triple
//! buffer and the atomics in [`OrchestratorStats`].

use super::{HybridTimer, SimClock, SimLoop};
use pin_core::spsc::SpscConsumer;
use pin_core::{FrameStateSnapshot, InputEvent, ProfileId, SimStep, TripleBuffer, TripleReader};
use pin_render::debug::{Camera, DebugLines, snapshot_lines};
use pin_render::gpu::{GpuContext, GpuOptions, SurfaceOptions, SurfaceSet, SurfaceSource, wgpu};
use pin_render::topology::{WindowEvent, WindowEventKind};
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Monotonic nanoseconds since the first call in this process (std
/// `Instant`). The default sim clock when SDL's clock is not wanted.
pub fn monotonic_now_ns() -> u64 {
    static BASE: OnceLock<Instant> = OnceLock::new();
    let base = *BASE.get_or_init(Instant::now);
    u64::try_from(base.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Radius of a built-in ball profile (index into
/// `pin_core::profiles::BUILTIN_PROFILE_NAMES`), or the standard 27 mm ball's
/// radius for unknown ids.
pub fn default_ball_radius(id: ProfileId) -> f32 {
    pin_core::profiles::builtin_profiles()
        .nth(id.index())
        .map_or(0.0135, |p| p.radius_m())
}

/// What a window's `CloseRequested` does (DECISIONS "1.4 — close policy").
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum ClosePolicy {
    /// Closing the playfield window (plan index 0) quits; closing a
    /// backglass/score window is ignored (cabinet default: a stray click on a
    /// secondary screen must not end the game).
    #[default]
    PlayfieldOnly,
    /// Closing any window quits (desktop use).
    AnyWindow,
    /// Window close requests never quit (kiosk cabinets; quit via SDL quit).
    Ignore,
}

impl ClosePolicy {
    /// Whether a `CloseRequested` on window `window` (plan index) quits.
    pub const fn quits(self, window: usize) -> bool {
        match self {
            ClosePolicy::PlayfieldOnly => window == 0,
            ClosePolicy::AnyWindow => true,
            ClosePolicy::Ignore => false,
        }
    }
}

/// GPU part of the render thread's configuration.
pub struct RenderGpu {
    /// The instance the surfaces were created from (main thread).
    pub instance: wgpu::Instance,
    /// Adapter options.
    pub options: GpuOptions,
    /// Surfaces created on the main thread (0–3).
    pub surfaces: Vec<SurfaceSource>,
    /// Present options (VRR, ...).
    pub surface_options: SurfaceOptions,
}

/// Render thread configuration.
pub struct RenderConfig {
    /// GPU and surfaces; `None` runs the render thread without a GPU (it
    /// still consumes snapshots and window events).
    pub gpu: Option<RenderGpu>,
    /// Window events from the main thread.
    pub window_events: Option<SpscConsumer<WindowEvent>>,
    /// Playfield debug camera.
    pub camera: Camera,
    /// What `CloseRequested` does.
    pub close_policy: ClosePolicy,
    /// Sleep between iterations when no playfield surface paces the loop.
    pub idle_period: Duration,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            gpu: None,
            window_events: None,
            // A standard-body playfield, 0.52 m × 1.07 m, origin bottom-left.
            camera: Camera::ortho_top_down([0.0, 0.0], [0.52, 1.07]),
            close_policy: ClosePolicy::default(),
            idle_period: Duration::from_millis(4),
        }
    }
}

/// Everything [`spawn_threads`] needs.
pub struct ThreadsConfig {
    /// The simulation (a stub until 6.0).
    pub sim: Box<dyn SimStep>,
    /// Input events from the main thread.
    pub input: SpscConsumer<InputEvent>,
    /// Clock of the input timestamps (SDL's ns clock in the app).
    pub now_ns: fn() -> u64,
    /// Sim thread wait strategy.
    pub timer: HybridTimer,
    /// Render thread configuration.
    pub render: RenderConfig,
}

impl ThreadsConfig {
    /// [`IdleSim`](super::IdleSim), the std monotonic clock, no GPU.
    pub fn idle(input: SpscConsumer<InputEvent>) -> Self {
        Self {
            sim: Box::new(super::IdleSim),
            input,
            now_ns: monotonic_now_ns,
            timer: HybridTimer::default(),
            render: RenderConfig::default(),
        }
    }
}

/// Counters shared by the threads (all relaxed atomics; read with
/// [`OrchestratorStats::snapshot`]).
#[derive(Debug, Default)]
pub struct OrchestratorStats {
    sim_ticks: AtomicU64,
    sim_dropped_ticks: AtomicU64,
    sim_ticks_due: AtomicU64,
    render_iterations: AtomicU64,
    render_frames: AtomicU64,
    render_last_tick: AtomicU64,
    render_non_monotonic: AtomicU64,
    close_requested: AtomicBool,
    gpu_ready: AtomicBool,
    gpu_error: AtomicBool,
}

/// A copy of [`OrchestratorStats`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct StatsSnapshot {
    /// Ticks computed by the sim thread.
    pub sim_ticks: u64,
    /// Ticks dropped by the catch-up cap.
    pub sim_dropped_ticks: u64,
    /// Render loop iterations.
    pub render_iterations: u64,
    /// Frames with at least one surface presented.
    pub render_frames: u64,
    /// Last snapshot tick the render thread read.
    pub render_last_tick: u64,
    /// Times the render thread read a snapshot older than the previous one.
    pub render_non_monotonic: u64,
    /// A window close request matched the close policy.
    pub close_requested: bool,
    /// The render thread created its GPU context and surfaces.
    pub gpu_ready: bool,
    /// GPU setup failed (the render thread continued without a GPU).
    pub gpu_error: bool,
}

impl OrchestratorStats {
    /// Current values.
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            sim_ticks: self.sim_ticks.load(Ordering::Relaxed),
            sim_dropped_ticks: self.sim_dropped_ticks.load(Ordering::Relaxed),
            render_iterations: self.render_iterations.load(Ordering::Relaxed),
            render_frames: self.render_frames.load(Ordering::Relaxed),
            render_last_tick: self.render_last_tick.load(Ordering::Relaxed),
            render_non_monotonic: self.render_non_monotonic.load(Ordering::Relaxed),
            close_requested: self.close_requested.load(Ordering::Relaxed),
            gpu_ready: self.gpu_ready.load(Ordering::Relaxed),
            gpu_error: self.gpu_error.load(Ordering::Relaxed),
        }
    }
}

/// Reports growth of a monotonically increasing drop counter (e.g.
/// `ForwardStats::input_dropped`) so the main thread can log it once per
/// change instead of every pump.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct DropMonitor {
    last: u64,
}

impl DropMonitor {
    /// Returns `Some(new_drops)` when `total` grew since the last call.
    pub fn check(&mut self, total: u64) -> Option<u64> {
        if total > self.last {
            let d = total - self.last;
            self.last = total;
            Some(d)
        } else {
            None
        }
    }

    /// The last total seen.
    pub fn total(&self) -> u64 {
        self.last
    }
}

/// Handles of the running threads.
pub struct Threads {
    shutdown: Arc<AtomicBool>,
    stats: Arc<OrchestratorStats>,
    handles: Vec<(&'static str, JoinHandle<()>)>,
}

/// Spawns the sim, render, audio and I/O threads.
pub fn spawn_threads(cfg: ThreadsConfig) -> Result<Threads, String> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let stats = Arc::new(OrchestratorStats::default());
    let (writer, reader) = TripleBuffer::new(FrameStateSnapshot::default());
    let mut threads = Threads {
        shutdown: Arc::clone(&shutdown),
        stats: Arc::clone(&stats),
        handles: Vec::with_capacity(4),
    };

    let ThreadsConfig {
        sim,
        input,
        now_ns,
        timer,
        render,
    } = cfg;

    let spawn = |threads: &mut Threads, name: &'static str, f: Box<dyn FnOnce() + Send>| {
        match std::thread::Builder::new().name(name.to_owned()).spawn(f) {
            Ok(h) => {
                threads.handles.push((name, h));
                Ok(())
            }
            Err(e) => Err(format!("spawning the {name} thread: {e}")),
        }
    };

    let sim_loop = SimLoop::new(sim, input, writer);
    let (sd, st) = (Arc::clone(&shutdown), Arc::clone(&stats));
    let r = spawn(
        &mut threads,
        "sim",
        Box::new(move || sim_thread(sim_loop, now_ns, timer, &sd, &st)),
    );
    let (sd, st) = (Arc::clone(&shutdown), Arc::clone(&stats));
    let r = r.and_then(|()| {
        spawn(
            &mut threads,
            "render",
            Box::new(move || render_thread(render, reader, &sd, &st)),
        )
    });
    let sd = Arc::clone(&shutdown);
    let r = r.and_then(|()| spawn(&mut threads, "audio", Box::new(move || park_until(&sd))));
    let sd = Arc::clone(&shutdown);
    let r = r.and_then(|()| spawn(&mut threads, "io", Box::new(move || park_until(&sd))));
    match r {
        Ok(()) => Ok(threads),
        Err(e) => {
            let _ = threads.shutdown();
            Err(e)
        }
    }
}

impl Threads {
    /// Live counters.
    pub fn stats(&self) -> StatsSnapshot {
        self.stats.snapshot()
    }

    /// `true` once a window close request matched the close policy; the main
    /// thread should then quit.
    pub fn close_requested(&self) -> bool {
        self.stats.close_requested.load(Ordering::Relaxed)
    }

    /// Names of the running threads (sim, render, audio, io).
    pub fn names(&self) -> Vec<&'static str> {
        self.handles.iter().map(|(n, _)| *n).collect()
    }

    /// Names of threads that have already exited (e.g. after a panic).
    pub fn finished(&self) -> Vec<&'static str> {
        self.handles
            .iter()
            .filter(|(_, h)| h.is_finished())
            .map(|(n, _)| *n)
            .collect()
    }

    /// Requests shutdown, wakes every thread and joins them all. Returns the
    /// final counters, or the names of threads that panicked.
    pub fn shutdown(mut self) -> Result<StatsSnapshot, String> {
        self.shutdown.store(true, Ordering::Release);
        for (_, h) in &self.handles {
            h.thread().unpark();
        }
        let mut panicked = Vec::new();
        for (name, h) in self.handles.drain(..) {
            if h.join().is_err() {
                panicked.push(name);
            }
        }
        if panicked.is_empty() {
            Ok(self.stats.snapshot())
        } else {
            Err(format!("thread(s) panicked: {}", panicked.join(", ")))
        }
    }
}

impl Drop for Threads {
    fn drop(&mut self) {
        // Dropped without `shutdown`: still stop and join (surfaces must be
        // released before the windows they were created from).
        self.shutdown.store(true, Ordering::Release);
        for (_, h) in &self.handles {
            h.thread().unpark();
        }
        for (_, h) in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

/// Audio / I/O placeholder: parks until shutdown.
fn park_until(shutdown: &AtomicBool) {
    while !shutdown.load(Ordering::Acquire) {
        std::thread::park_timeout(Duration::from_millis(100));
    }
}

/// The real-time sim loop: accumulator timing with the catch-up cap, dropped
/// ticks shift simulated time, hybrid sleep + spin until the next tick. No
/// allocation, lock or I/O per tick (D5).
fn sim_thread<S: SimStep>(
    mut sim_loop: SimLoop<S>,
    now_ns: fn() -> u64,
    timer: HybridTimer,
    shutdown: &AtomicBool,
    stats: &OrchestratorStats,
) {
    let start = now_ns();
    let mut clock = SimClock::new(start);
    sim_loop.set_epoch_ns(start);
    let stop = || shutdown.load(Ordering::Acquire);
    while !stop() {
        let dropped_before = clock.dropped_ticks();
        let due = clock.advance(now_ns());
        let newly_dropped = clock.dropped_ticks() - dropped_before;
        if newly_dropped > 0 {
            sim_loop.skip_ticks(newly_dropped);
            stats
                .sim_dropped_ticks
                .store(clock.dropped_ticks(), Ordering::Relaxed);
        }
        if due > 0 {
            sim_loop.run_ticks(due);
            stats.sim_ticks.store(sim_loop.tick(), Ordering::Relaxed);
            stats
                .sim_ticks_due
                .store(clock.ticks_due_total(), Ordering::Relaxed);
        }
        timer.wait_until(clock.next_due_ns(), now_ns, stop);
    }
}

fn render_thread(
    cfg: RenderConfig,
    mut reader: TripleReader<FrameStateSnapshot>,
    shutdown: &AtomicBool,
    stats: &OrchestratorStats,
) {
    let RenderConfig {
        gpu,
        mut window_events,
        camera,
        close_policy,
        idle_period,
    } = cfg;
    // GPU setup on this thread (the device lives here). Without surfaces
    // there is nothing to present, so no adapter is requested.
    let mut surfaces = match gpu {
        Some(g) if !g.surfaces.is_empty() => {
            let ctx =
                GpuContext::with_instance(g.instance, &g.options, Some(&g.surfaces[0].surface));
            match ctx.and_then(|ctx| {
                SurfaceSet::new(&ctx, g.surfaces, g.surface_options).map(|s| (ctx, s))
            }) {
                Ok((ctx, set)) => {
                    let info = ctx.adapter_info();
                    eprintln!(
                        "pin_app: render on {} ({:?}); present modes {:?}",
                        info.name,
                        info.backend,
                        set.choices().collect::<Vec<_>>()
                    );
                    stats.gpu_ready.store(true, Ordering::Relaxed);
                    Some((ctx, set))
                }
                Err(e) => {
                    eprintln!("pin_app: GPU setup failed, rendering disabled: {e}");
                    stats.gpu_error.store(true, Ordering::Relaxed);
                    None
                }
            }
        }
        _ => None,
    };
    let mut lines = DebugLines::new(4096);
    let mut last_tick = 0u64;
    while !shutdown.load(Ordering::Acquire) {
        if let Some(rx) = window_events.as_mut() {
            while let Some(ev) = rx.pop() {
                if ev.kind == WindowEventKind::CloseRequested && close_policy.quits(ev.window) {
                    stats.close_requested.store(true, Ordering::Relaxed);
                }
                if let Some((_, set)) = surfaces.as_mut() {
                    set.handle_window_event(&ev);
                }
            }
        }
        let snap = reader.read();
        if snap.tick < last_tick {
            stats.render_non_monotonic.fetch_add(1, Ordering::Relaxed);
        }
        last_tick = snap.tick;
        stats.render_last_tick.store(last_tick, Ordering::Relaxed);
        let mut paced = false;
        if let Some((_, set)) = surfaces.as_mut() {
            lines.clear();
            snapshot_lines(snap, &default_ball_radius, &mut lines);
            let st = set.render(&lines, &camera);
            if st.contains(&pin_render::gpu::SurfaceStatus::Presented) {
                stats.render_frames.fetch_add(1, Ordering::Relaxed);
            }
            paced = set.paces_frame();
        }
        stats.render_iterations.fetch_add(1, Ordering::Relaxed);
        if !paced {
            std::thread::park_timeout(idle_period);
        }
    }
    // Surfaces drop here, on the render thread, before the main thread
    // destroys the windows (it joins this thread first).
    drop(surfaces);
}
