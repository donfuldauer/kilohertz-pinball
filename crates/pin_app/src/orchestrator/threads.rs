//! The engine threads: sim (1 kHz, real time), render (with a table: the
//! 5.1 PBR playfield and the 5.3 displays, [`TableRenderer`](super::TableRenderer);
//! without one: the 1.4 debug view), audio (sim sound triggers → 3.3 mixer,
//! haptics → SSF exciter mix and telemetry, the 7.1 mix into an
//! [`AudioSink`]: a device, an offline recording or the null sink) and I/O
//! (a placeholder that parks until shutdown). The main thread stays the SDL
//! pump (D6) and only talks to these threads through rings, the triple
//! buffer and the atomics in [`OrchestratorStats`].

use super::audio::{AUDIO_BLOCK_FRAMES, AudioEngine, AudioSink, NullSink, RegisteredSound};
use super::present::AppSurfaces;
use super::table::{DisplayCommand, FlipperOutline};
use super::view::{TableRenderOptions, TableVisuals};
use super::{HybridTimer, SimClock, SimLoop};
use arrayvec::ArrayVec;
use pin_audio::mixer::{MixerConfig, SoundReceiver};
use pin_audio::ssf::{CHANNELS, SsfConfig};
use pin_audio::telemetry::{DatagramBatch, TelemetryEncoder, TelemetryState};
use pin_core::haptics::HapticEvent;
use pin_core::input::HapticsTelemetryEvent;
use pin_core::spsc::SpscConsumer;
use pin_core::{FrameStateSnapshot, InputEvent, ProfileId, SimStep, TripleBuffer, TripleReader};
use pin_input::haptics::HapticsConsumer;
use pin_render::debug::{Camera, DebugLine, DebugLines, snapshot_lines};
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
    /// Static table lines drawn every frame under the balls (the loaded
    /// table's collider outlines, [`table_lines`](super::table_lines)).
    pub table_lines: Vec<DebugLine>,
    /// Flipper outlines, drawn at each snapshot's flipper angles.
    pub flippers: Vec<FlipperOutline>,
    /// The loaded table ([`TableVisuals::of`](super::TableVisuals::of)).
    /// `Some`: the surfaces show the PBR table and the displays
    /// ([`TableRenderer`](super::TableRenderer)); `None`: the debug view
    /// (`table_lines`, `flippers`, balls).
    pub table: Option<std::sync::Arc<TableVisuals>>,
    /// PBR / post settings and the debug overlay switch (default: post
    /// defaults, overlay off).
    pub table_render: TableRenderOptions,
    /// Display commands (scenes, scores) from the sim thread
    /// ([`TableSim::with_displays`](super::TableSim::with_displays)).
    pub displays: Option<HapticsConsumer<DisplayCommand>>,
}

/// Colour of flipper bats in the debug view.
pub const FLIPPER_LINE_COLOR: [f32; 4] = [0.95, 0.95, 0.3, 1.0];

/// Audio thread configuration.
pub struct AudioConfig {
    /// Haptic impulses from the sim thread ([`TableSim`](super::TableSim)).
    pub haptics: Option<HapticsConsumer<HapticsTelemetryEvent>>,
    /// SSF exciter mixer settings.
    pub ssf: SsfConfig,
    /// Sleep between blocks (unpaced sinks render one block per period).
    pub period: Duration,
    /// Sound triggers from the sim thread
    /// ([`TableSim::with_sounds`](super::TableSim::with_sounds)), drained
    /// by the 3.3 mixer.
    pub sounds: Option<SoundReceiver>,
    /// 3.3 mixer settings.
    pub mixer: MixerConfig,
    /// The table's decoded sounds ([`table_samples`](super::table_samples)).
    pub samples: Vec<RegisteredSound>,
    /// Where the 7.1 blocks go; `None` = [`NullSink`].
    pub sink: Option<Box<dyn AudioSink>>,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            haptics: None,
            ssf: SsfConfig::default(),
            period: Duration::from_millis(5),
            sounds: None,
            mixer: MixerConfig::default(),
            samples: Vec::new(),
            sink: None,
        }
    }
}

/// What the audio thread did (read with [`Threads::audio_stats`]).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct AudioStats {
    /// Haptic impulses received from the sim.
    pub haptic_events: u64,
    /// 7.1 frames (3.3 mixer + SSF) rendered into the sink.
    pub frames: u64,
    /// Telemetry datagrams encoded (haptic impulses, kind 5).
    pub telemetry_datagrams: u64,
    /// Frames the 3.3 mixer rendered.
    pub mixer_frames: u64,
    /// Mixer voices still playing at the end of the last block.
    pub mixer_voices: u64,
    /// Most mixer voices playing at the end of a block so far.
    pub mixer_peak_voices: u64,
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            gpu: None,
            window_events: None,
            // A standard-body playfield, 0.52 m × 1.07 m, origin bottom-left,
            // with a 1 cm margin so geometry on the table bounds stays visible.
            camera: Camera::ortho_top_down_with_margin([0.0, 0.0], [0.52, 1.07], 0.01),
            close_policy: ClosePolicy::default(),
            idle_period: Duration::from_millis(4),
            table_lines: Vec::new(),
            flippers: Vec::new(),
            table: None,
            table_render: TableRenderOptions::default(),
            displays: None,
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
    /// Audio thread configuration.
    pub audio: AudioConfig,
}

impl ThreadsConfig {
    /// [`IdleSim`](super::IdleSim), the std monotonic clock, no GPU, no
    /// haptics.
    pub fn idle(input: SpscConsumer<InputEvent>) -> Self {
        Self::with_sim(Box::new(super::IdleSim), input)
    }

    /// `sim` (e.g. a [`TableSim`](super::TableSim)), the std monotonic
    /// clock, no GPU, no haptics.
    pub fn with_sim(sim: Box<dyn SimStep>, input: SpscConsumer<InputEvent>) -> Self {
        Self {
            sim,
            input,
            now_ns: monotonic_now_ns,
            timer: HybridTimer::default(),
            render: RenderConfig::default(),
            audio: AudioConfig::default(),
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
    audio_haptic_events: AtomicU64,
    audio_frames: AtomicU64,
    audio_datagrams: AtomicU64,
    audio_mixer_frames: AtomicU64,
    audio_mixer_voices: AtomicU64,
    audio_mixer_peak: AtomicU64,
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

    /// The audio thread's counters.
    pub fn audio(&self) -> AudioStats {
        AudioStats {
            haptic_events: self.audio_haptic_events.load(Ordering::Relaxed),
            frames: self.audio_frames.load(Ordering::Relaxed),
            telemetry_datagrams: self.audio_datagrams.load(Ordering::Relaxed),
            mixer_frames: self.audio_mixer_frames.load(Ordering::Relaxed),
            mixer_voices: self.audio_mixer_voices.load(Ordering::Relaxed),
            mixer_peak_voices: self.audio_mixer_peak.load(Ordering::Relaxed),
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
        audio,
    } = cfg;

    let spawn = |threads: &mut Threads, name: &'static str, f: Box<dyn FnOnce() + Send>| {
        let mut b = std::thread::Builder::new().name(name.to_owned());
        if name == "sim" {
            // A Wasm rules script needs MIN_HOST_STACK_BYTES on the thread
            // that steps the simulation (see `Simulation`).
            b = b.stack_size(super::table::SIM_STACK_BYTES);
        }
        match b.spawn(f) {
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
    let (sd, st) = (Arc::clone(&shutdown), Arc::clone(&stats));
    let r = r.and_then(|()| {
        spawn(
            &mut threads,
            "audio",
            Box::new(move || audio_thread(audio, &sd, &st)),
        )
    });
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

    /// The audio thread's counters.
    pub fn audio_stats(&self) -> AudioStats {
        self.stats.audio()
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

/// The audio thread: drains haptic impulses into the SSF exciter mixer and
/// the telemetry encoder; the 3.3 mixer drains the sim's sound triggers;
/// each block (the mixer's stereo on FL/FR plus the SSF channels) goes to
/// the sink. Unpaced sinks get one block per `period`; a device sink is
/// kept a few blocks ahead. Allocates its buffers once, before the loop.
fn audio_thread(cfg: AudioConfig, shutdown: &AtomicBool, stats: &OrchestratorStats) {
    let AudioConfig {
        mut haptics,
        ssf,
        period,
        sounds,
        mixer,
        samples,
        sink,
    } = cfg;
    let mut engine = AudioEngine::new(ssf, mixer, sounds, &samples);
    drop(samples);
    let mut sink: Box<dyn AudioSink> = sink.unwrap_or_else(|| Box::new(NullSink::default()));
    let mut encoder = TelemetryEncoder::new();
    let mut batch = DatagramBatch::new();
    let state = TelemetryState::new();
    let mut impulses: ArrayVec<HapticsTelemetryEvent, 64> = ArrayVec::new();
    /// Most blocks a paced sink gets per wake-up (catch-up bound).
    const MAX_BLOCKS_PER_WAKE: usize = 8;
    while !shutdown.load(Ordering::Acquire) {
        if let Some(rx) = haptics.as_mut() {
            loop {
                impulses.clear();
                while !impulses.is_full() {
                    match rx.pop() {
                        Some(e) => {
                            engine.feed_haptic(HapticEvent::from(e));
                            impulses.push(e);
                        }
                        None => break,
                    }
                }
                if impulses.is_empty() {
                    break;
                }
                stats
                    .audio_haptic_events
                    .fetch_add(impulses.len() as u64, Ordering::Relaxed);
                let tick = impulses.last().map_or(0, |e| e.tick);
                encoder.encode_tick(tick, &state, &impulses, &mut batch);
                stats
                    .audio_datagrams
                    .fetch_add(batch.len() as u64, Ordering::Relaxed);
            }
        }
        let blocks = if sink.paced() { MAX_BLOCKS_PER_WAKE } else { 1 };
        for _ in 0..blocks {
            if sink.paced() && !sink.wants_block() {
                break;
            }
            let block = engine.render_block();
            sink.write(block, CHANNELS);
            stats
                .audio_frames
                .fetch_add(AUDIO_BLOCK_FRAMES as u64, Ordering::Relaxed);
            stats
                .audio_mixer_frames
                .store(engine.mixer().frames_rendered(), Ordering::Relaxed);
            stats
                .audio_mixer_voices
                .store(engine.active_voices() as u64, Ordering::Relaxed);
            stats
                .audio_mixer_peak
                .store(engine.peak_voices() as u64, Ordering::Relaxed);
        }
        std::thread::park_timeout(period);
    }
}

/// I/O placeholder: parks until shutdown.
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

/// What the render thread presents with.
enum Presenter {
    /// The 1.4 surfaces with the debug view (no table, or a table whose
    /// surfaces need presenter threads).
    Debug(GpuContext, SurfaceSet),
    /// The table renderer, presented directly.
    Table(GpuContext, AppSurfaces),
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
        table_lines,
        flippers,
        table,
        table_render,
        mut displays,
    } = cfg;
    // GPU setup on this thread (the device lives here). Without surfaces
    // there is nothing to present, so no adapter is requested.
    let mut presenter = match gpu {
        Some(g) if !g.surfaces.is_empty() => {
            let ctx =
                GpuContext::with_instance(g.instance, &g.options, Some(&g.surfaces[0].surface));
            let made = ctx.map_err(|e| e.to_string()).and_then(|ctx| match &table {
                Some(visuals) if AppSurfaces::supported(&ctx, &g.surfaces, g.surface_options) => {
                    AppSurfaces::new(&ctx, g.surfaces, g.surface_options, visuals, table_render)
                        .map(|s| Presenter::Table(ctx, s))
                }
                other => {
                    if other.is_some() {
                        eprintln!(
                            "pin_app: a surface needs a presenter thread or an unsupported \
                             format; drawing the debug view instead of the PBR table"
                        );
                    }
                    SurfaceSet::new(&ctx, g.surfaces, g.surface_options)
                        .map(|s| Presenter::Debug(ctx, s))
                        .map_err(|e| e.to_string())
                }
            });
            match made {
                Ok(p) => {
                    let ctx = match &p {
                        Presenter::Debug(c, _) | Presenter::Table(c, _) => c,
                    };
                    let info = ctx.adapter_info();
                    match &p {
                        Presenter::Debug(_, set) => eprintln!(
                            "pin_app: render on {} ({:?}); debug view; present modes {:?}",
                            info.name,
                            info.backend,
                            set.choices().collect::<Vec<_>>()
                        ),
                        Presenter::Table(_, s) => eprintln!(
                            "pin_app: render on {} ({:?}); PBR table on {} surface(s)",
                            info.name,
                            info.backend,
                            s.len()
                        ),
                    }
                    stats.gpu_ready.store(true, Ordering::Relaxed);
                    Some(p)
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
    let mut lines = DebugLines::new(4096 + table_lines.len() + flippers.len());
    let mut last_tick = 0u64;
    while !shutdown.load(Ordering::Acquire) {
        if let Some(rx) = window_events.as_mut() {
            while let Some(ev) = rx.pop() {
                if ev.kind == WindowEventKind::CloseRequested && close_policy.quits(ev.window) {
                    stats.close_requested.store(true, Ordering::Relaxed);
                }
                match presenter.as_mut() {
                    Some(Presenter::Debug(_, set)) => set.handle_window_event(&ev),
                    Some(Presenter::Table(_, s)) => s.handle_window_event(&ev),
                    None => {}
                }
            }
        }
        let snap = reader.read();
        if snap.tick < last_tick {
            stats.render_non_monotonic.fetch_add(1, Ordering::Relaxed);
        }
        last_tick = snap.tick;
        stats.render_last_tick.store(last_tick, Ordering::Relaxed);
        if let Some(rx) = displays.as_mut() {
            while let Some(cmd) = rx.pop() {
                if let Some(Presenter::Table(_, s)) = presenter.as_mut() {
                    s.apply_display_command(cmd, last_tick);
                }
            }
        }
        let mut paced = false;
        match presenter.as_mut() {
            Some(Presenter::Debug(_, set)) => {
                lines.clear();
                for l in &table_lines {
                    lines.push(*l);
                }
                for st in snap.flippers() {
                    if let Some(f) = flippers.get(usize::from(st.id.0)) {
                        lines.push(f.line(st.angle_rad, FLIPPER_LINE_COLOR));
                    }
                }
                snapshot_lines(snap, &default_ball_radius, &mut lines);
                let st = set.render(&lines, &camera);
                if st.contains(&pin_render::gpu::SurfaceStatus::Presented) {
                    stats.render_frames.fetch_add(1, Ordering::Relaxed);
                }
                paced = set.frame_paced(&st);
            }
            Some(Presenter::Table(_, s)) => {
                let st = s.render(snap);
                if st.contains(&pin_render::gpu::SurfaceStatus::Presented) {
                    stats.render_frames.fetch_add(1, Ordering::Relaxed);
                }
                paced = s.frame_paced(&st);
            }
            None => {}
        }
        stats.render_iterations.fetch_add(1, Ordering::Relaxed);
        if !paced {
            std::thread::park_timeout(idle_period);
        }
    }
    // Surfaces drop here, on the render thread, before the main thread
    // destroys the windows (it joins this thread first).
    drop(presenter);
}
