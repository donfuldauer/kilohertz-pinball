//! The engine threads: sim (1 kHz, real time), render (with a table: the
//! 5.1 PBR playfield and the 5.3 displays, [`TableRenderer`](super::TableRenderer);
//! without one: the 1.4 debug view), audio (sim sound triggers → 3.3 mixer,
//! haptics → SSF exciter mix and telemetry, the 7.1 mix into an
//! [`AudioSink`]: a device, an offline recording or the null sink) and I/O
//! (a placeholder that parks until shutdown). The main thread stays the SDL
//! pump (D6) and only talks to these threads through rings, the triple
//! buffer and the atomics in [`OrchestratorStats`].

use super::audio::{AUDIO_BLOCK_FRAMES, AudioEngine, AudioSink, NullSink, RegisteredSound};
use super::gpu_timer::GpuFrameTimer;
use super::hud::{DebugViewHooks, HudFeed, HudOverlay, hud_format_supported};
use super::present::{AppSurfaces, SurfacePlan, table_set_accepted};
use super::table::{DisplayCommand, FlipperOutline};
use super::trace::{FrameTraceWriter, SimTraceWriter};
use super::view::{TableRenderOptions, TableVisuals};
use super::{HybridTimer, SimClock, SimLoop};
use crate::profiler::{
    CommandQueueOverflows, FrameRateMeter, HudRates, LoopProfiler, PresentProfiler, ProfiledSim,
    Profiler, ProfilerReader, hud_rate_lines,
};
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
use pin_render::topology::{SurfaceRole, WindowEvent, WindowEventKind};
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
    /// The profiler's GPU frame-time writer (7.1 hook 3). On a device with
    /// `TIMESTAMP_QUERY`, every presented frame's GPU time is measured with
    /// timestamp queries ([`GpuFrameTimer`]) and recorded here, with the
    /// table presenter and with the debug view alike (w8g-d,
    /// [`DebugViewHooks`]); otherwise nothing is recorded (never host
    /// time). `None` (default): no GPU timing.
    pub present_profiler: Option<PresentProfiler>,
    /// The profiler HUD (`pin_app --profiler-hud`): `Some` draws
    /// [`hud_lines`](crate::profiler::hud_lines) of this reader's snapshot
    /// over the playfield (table or debug view), refreshed every
    /// [`HUD_REFRESH_NS`]. `None` (default): no HUD, frames unchanged.
    pub hud: Option<ProfilerReader>,
    /// The latency trace's frame writer (`--latency-trace`, REQ-LAT-003):
    /// every presented frame's snapshot tick and present time are recorded
    /// ([`FrameTraceWriter::presented`]). `None` (default): nothing.
    pub latency_trace: Option<FrameTraceWriter>,
}

/// How often the render thread refreshes the profiler HUD text (250 ms:
/// four layouts per second; the snapshot is cheap, the text layout is not).
pub const HUD_REFRESH_NS: u64 = 250_000_000;

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
    /// Playfield position `[x, y]` (mm, the SSF playfield frame,
    /// [`SsfConfig::playfield_mm`]) of each haptic source id, registered
    /// with the SSF mixer before the first block
    /// ([`AudioEngine::with_source_positions`];
    /// [`table_source_positions`](super::table_source_positions) for a
    /// loaded table). Sources without one play at the playfield centre.
    pub source_positions: Vec<(u16, [f32; 2])>,
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
            source_positions: Vec::new(),
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
            present_profiler: None,
            hud: None,
            latency_trace: None,
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
    /// The profiler's sim-loop writer (7.1 hook 2): the sim thread runs its
    /// ticks with [`SimLoop::run_ticks_profiled`] on
    /// [`now_ns`](Self::now_ns), recording each coil-driving input's latency
    /// from its raw SDL stamp to the start of the tick that applies it
    /// (REQ-LAT-005), and after every batch of ticks publishes
    /// [`SimLoop::input_overflow_ticks`] into it (one relaxed atomic
    /// store). Pair it with a step that does not record input latency
    /// itself ([`ProfiledSim::tick_time_only`], as
    /// [`with_profiled_sim`](Self::with_profiled_sim) does). `None`
    /// (default): not profiled.
    pub loop_profiler: Option<LoopProfiler>,
    /// The latency trace's sim writer (`--latency-trace`, REQ-LAT-003),
    /// installed on the sim loop ([`SimLoop::set_latency_trace`]).
    /// `None` (default): nothing is recorded.
    pub latency_trace: Option<SimTraceWriter>,
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
            loop_profiler: None,
            latency_trace: None,
        }
    }

    /// `sim` profiled by `profiler` (the 7.1 hooks): the step is wrapped
    /// in a tick-time-only [`ProfiledSim`] ([`ProfiledSim::tick_time_only`])
    /// timed by `now_ns`, which also becomes the sim loop's clock
    /// ([`now_ns`](Self::now_ns)), so tick times and input latencies are
    /// read on the clock the input timestamps are on (the app passes SDL's
    /// `SDL_GetTicksNS`). The loop writer goes to the sim thread
    /// ([`loop_profiler`](Self::loop_profiler)), which records each input's
    /// latency from its raw SDL stamp, also for a stamp before the loop's
    /// epoch (REQ-LAT-005, [`SimLoop::run_ticks_profiled`]), and the GPU
    /// frame-time writer goes to the render thread
    /// ([`RenderConfig::present_profiler`]). Returns the config and the
    /// profiler's reader (clone it for the HUD, [`RenderConfig::hud`]).
    pub fn with_profiled_sim<S>(
        sim: S,
        input: SpscConsumer<InputEvent>,
        now_ns: fn() -> u64,
        profiler: Profiler,
    ) -> (Self, ProfilerReader)
    where
        S: SimStep + CommandQueueOverflows + 'static,
    {
        let Profiler {
            sim: sim_profiler,
            sim_loop,
            present,
            reader,
        } = profiler;
        let profiled = ProfiledSim::new(sim, sim_profiler, now_ns).tick_time_only();
        let mut cfg = Self::with_sim(Box::new(profiled), input);
        cfg.now_ns = now_ns;
        cfg.loop_profiler = Some(sim_loop);
        cfg.render.present_profiler = Some(present);
        (cfg, reader)
    }
}

/// Sentinel of the "ms since spawn" counters: the event never happened.
const NEVER: u64 = u64::MAX;

/// Counters shared by the threads (all relaxed atomics; read with
/// [`OrchestratorStats::snapshot`]).
#[derive(Debug)]
pub struct OrchestratorStats {
    sim_ticks: AtomicU64,
    sim_dropped_ticks: AtomicU64,
    sim_ticks_due: AtomicU64,
    render_iterations: AtomicU64,
    render_frames: AtomicU64,
    render_last_tick: AtomicU64,
    render_non_monotonic: AtomicU64,
    render_presented: AtomicU64,
    render_occluded: AtomicU64,
    render_skipped: AtomicU64,
    render_hidden: AtomicU64,
    /// ms from spawn to GPU ready, [`NEVER`] until then.
    gpu_ready_ms: AtomicU64,
    /// ms from spawn to the first presented frame, [`NEVER`] until then.
    first_frame_ms: AtomicU64,
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
    /// Surface presents (one per surface and frame;
    /// [`SurfaceStatus::Presented`](pin_render::gpu::SurfaceStatus::Presented)).
    pub render_presented: u64,
    /// Surface acquires that reported the window as covered
    /// ([`SurfaceStatus::Occluded`](pin_render::gpu::SurfaceStatus::Occluded)).
    pub render_occluded: u64,
    /// Surface acquires that failed (timeout, outdated, lost;
    /// [`SurfaceStatus::Skipped`](pin_render::gpu::SurfaceStatus::Skipped)).
    pub render_skipped: u64,
    /// Surface draws skipped because the window was minimised or zero-sized
    /// ([`SurfaceStatus::Hidden`](pin_render::gpu::SurfaceStatus::Hidden)).
    pub render_hidden: u64,
    /// ms from [`spawn_threads`] until the render thread's GPU context and
    /// surfaces were ready (`None`: not (yet) ready).
    pub gpu_ready_ms: Option<u64>,
    /// ms from [`spawn_threads`] until the first presented frame (`None`:
    /// none yet).
    pub first_frame_ms: Option<u64>,
    /// A window close request matched the close policy.
    pub close_requested: bool,
    /// The render thread created its GPU context and surfaces.
    pub gpu_ready: bool,
    /// GPU setup failed (the render thread continued without a GPU).
    pub gpu_error: bool,
}

impl Default for OrchestratorStats {
    fn default() -> Self {
        Self {
            sim_ticks: AtomicU64::new(0),
            sim_dropped_ticks: AtomicU64::new(0),
            sim_ticks_due: AtomicU64::new(0),
            render_iterations: AtomicU64::new(0),
            render_frames: AtomicU64::new(0),
            render_last_tick: AtomicU64::new(0),
            render_non_monotonic: AtomicU64::new(0),
            render_presented: AtomicU64::new(0),
            render_occluded: AtomicU64::new(0),
            render_skipped: AtomicU64::new(0),
            render_hidden: AtomicU64::new(0),
            gpu_ready_ms: AtomicU64::new(NEVER),
            first_frame_ms: AtomicU64::new(NEVER),
            close_requested: AtomicBool::new(false),
            gpu_ready: AtomicBool::new(false),
            gpu_error: AtomicBool::new(false),
            audio_haptic_events: AtomicU64::new(0),
            audio_frames: AtomicU64::new(0),
            audio_datagrams: AtomicU64::new(0),
            audio_mixer_frames: AtomicU64::new(0),
            audio_mixer_voices: AtomicU64::new(0),
            audio_mixer_peak: AtomicU64::new(0),
        }
    }
}

fn ms_or_none(v: u64) -> Option<u64> {
    (v != NEVER).then_some(v)
}

fn ms_since(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_millis()).unwrap_or(NEVER - 1)
}

impl OrchestratorStats {
    /// Counts one render-loop iteration's surface statuses: presents,
    /// occluded / skipped / hidden surfaces, and (on the first frame with a
    /// present) the frame counter and the time to the first frame.
    fn record_statuses(&self, statuses: &[pin_render::gpu::SurfaceStatus], spawned: Instant) {
        use pin_render::gpu::SurfaceStatus as S;
        let mut presented = false;
        for st in statuses {
            let counter = match st {
                S::Presented => {
                    presented = true;
                    &self.render_presented
                }
                S::Occluded => &self.render_occluded,
                S::Skipped => &self.render_skipped,
                S::Hidden => &self.render_hidden,
                S::Unchanged | S::Busy => continue,
            };
            counter.fetch_add(1, Ordering::Relaxed);
        }
        if presented && self.render_frames.fetch_add(1, Ordering::Relaxed) == 0 {
            self.first_frame_ms
                .store(ms_since(spawned), Ordering::Relaxed);
        }
    }

    /// Current values.
    pub fn snapshot(&self) -> StatsSnapshot {
        StatsSnapshot {
            sim_ticks: self.sim_ticks.load(Ordering::Relaxed),
            sim_dropped_ticks: self.sim_dropped_ticks.load(Ordering::Relaxed),
            render_iterations: self.render_iterations.load(Ordering::Relaxed),
            render_frames: self.render_frames.load(Ordering::Relaxed),
            render_last_tick: self.render_last_tick.load(Ordering::Relaxed),
            render_non_monotonic: self.render_non_monotonic.load(Ordering::Relaxed),
            render_presented: self.render_presented.load(Ordering::Relaxed),
            render_occluded: self.render_occluded.load(Ordering::Relaxed),
            render_skipped: self.render_skipped.load(Ordering::Relaxed),
            render_hidden: self.render_hidden.load(Ordering::Relaxed),
            gpu_ready_ms: ms_or_none(self.gpu_ready_ms.load(Ordering::Relaxed)),
            first_frame_ms: ms_or_none(self.first_frame_ms.load(Ordering::Relaxed)),
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

impl StatsSnapshot {
    /// The render diagnostics line `pin_app` prints after its shutdown
    /// line: `pin_app: render: gpu ready after X ms, first frame after Y
    /// ms, N iterations, P presented / O occluded / S skipped / H hidden`
    /// (`-` for an event that never happened).
    pub fn render_diagnostics(&self) -> String {
        let ms = |v: Option<u64>| v.map_or_else(|| "-".to_owned(), |v| v.to_string());
        format!(
            "pin_app: render: gpu ready after {} ms, first frame after {} ms, {} iterations, \
             {} presented / {} occluded / {} skipped / {} hidden",
            ms(self.gpu_ready_ms),
            ms(self.first_frame_ms),
            self.render_iterations,
            self.render_presented,
            self.render_occluded,
            self.render_skipped,
            self.render_hidden
        )
    }
}

/// When `pin_app`'s main loop stops by itself: after `--run-frames N`
/// presented frames **and** `--run-ticks M` sim ticks (every condition
/// given must hold), with `--run-ms` as a wall-clock safety cap. With no
/// condition, `--run-ms` alone ends the run as before (a plain duration).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RunLimits {
    /// `--run-frames`: frames presented ([`StatsSnapshot::render_frames`]).
    pub frames: Option<u64>,
    /// `--run-ticks`: sim ticks computed ([`StatsSnapshot::sim_ticks`]).
    pub ticks: Option<u64>,
    /// `--run-ms`: the cap (or, without conditions, the run time).
    pub cap: Option<Duration>,
}

/// Why [`RunLimits::check`] ends the run.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RunExit {
    /// Every given condition holds (`pin_app: run condition met`).
    ConditionMet,
    /// The cap elapsed while a condition did not hold yet (a failure;
    /// `pin_app: run cap reached`).
    CapReached,
    /// The cap elapsed and no condition was given (`--run-ms` alone: the
    /// plain run time, silent as before).
    Elapsed,
}

fn flag_u64(args: &[String], flag: &str) -> Result<Option<u64>, String> {
    let Some(i) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    let v = args
        .get(i + 1)
        .ok_or_else(|| format!("{flag} needs a number"))?;
    v.parse::<u64>()
        .map(Some)
        .map_err(|e| format!("{flag} {v:?}: {e}"))
}

impl RunLimits {
    /// Reads `--run-frames N`, `--run-ticks M` and `--run-ms T` (unsigned
    /// integers).
    ///
    /// # Errors
    /// A flag without a value or with a value that is not a `u64`.
    pub fn from_args(args: &[String]) -> Result<RunLimits, String> {
        Ok(RunLimits {
            frames: flag_u64(args, "--run-frames")?,
            ticks: flag_u64(args, "--run-ticks")?,
            cap: flag_u64(args, "--run-ms")?.map(Duration::from_millis),
        })
    }

    /// `true` when `--run-frames` or `--run-ticks` was given.
    pub fn has_conditions(&self) -> bool {
        self.frames.is_some() || self.ticks.is_some()
    }

    /// `true` when there is at least one condition and every given one
    /// holds in `stats`.
    pub fn conditions_met(&self, stats: &StatsSnapshot) -> bool {
        self.has_conditions()
            && self.frames.is_none_or(|n| stats.render_frames >= n)
            && self.ticks.is_none_or(|n| stats.sim_ticks >= n)
    }

    /// Whether the run ends now, `elapsed` after it started.
    pub fn check(&self, stats: &StatsSnapshot, elapsed: Duration) -> Option<RunExit> {
        if self.conditions_met(stats) {
            return Some(RunExit::ConditionMet);
        }
        match self.cap {
            Some(cap) if elapsed >= cap => Some(if self.has_conditions() {
                RunExit::CapReached
            } else {
                RunExit::Elapsed
            }),
            _ => None,
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
    let spawned = Instant::now();
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
        loop_profiler,
        latency_trace,
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

    let mut sim_loop = SimLoop::new(sim, input, writer);
    if let Some(t) = latency_trace {
        sim_loop.set_latency_trace(t);
    }
    let (sd, st) = (Arc::clone(&shutdown), Arc::clone(&stats));
    let r = spawn(
        &mut threads,
        "sim",
        Box::new(move || {
            sim_thread(sim_loop, now_ns, timer, loop_profiler, &sd, &st);
        }),
    );
    let (sd, st) = (Arc::clone(&shutdown), Arc::clone(&stats));
    let r = r.and_then(|()| {
        spawn(
            &mut threads,
            "render",
            Box::new(move || render_thread(render, reader, &sd, &st, spawned)),
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
    pub fn shutdown(self) -> Result<StatsSnapshot, String> {
        self.shutdown_with_audio().map(|(s, _)| s)
    }

    /// [`shutdown`](Self::shutdown), also returning the audio thread's
    /// final counters, read after every thread was joined (so they are
    /// mutually consistent: e.g. `frames == mixer_frames`).
    pub fn shutdown_with_audio(mut self) -> Result<(StatsSnapshot, AudioStats), String> {
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
            Ok((self.stats.snapshot(), self.stats.audio()))
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
        source_positions,
    } = cfg;
    let mut engine =
        AudioEngine::new(ssf, mixer, sounds, &samples).with_source_positions(&source_positions);
    drop(samples);
    drop(source_positions);
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

/// Publishes the loop's input-overflow tick total into `profiler` (7.1
/// hook 2): one relaxed atomic store, no allocation, lock or I/O (D5).
pub(super) fn publish_input_overflow<S: SimStep>(
    sim_loop: &SimLoop<S>,
    profiler: Option<&mut LoopProfiler>,
) {
    if let Some(p) = profiler {
        p.set_input_overflow_ticks(sim_loop.input_overflow_ticks());
    }
}

/// The real-time sim loop: accumulator timing with the catch-up cap, dropped
/// ticks shift simulated time, hybrid sleep + spin until the next tick. No
/// allocation, lock or I/O per tick (D5). With a `loop_profiler`, the ticks
/// run with [`SimLoop::run_ticks_profiled`] on `now_ns` (input latency from
/// the raw SDL stamps, REQ-LAT-005) and the input-overflow tick total is
/// published after every batch of ticks ([`publish_input_overflow`]).
fn sim_thread<S: SimStep>(
    mut sim_loop: SimLoop<S>,
    now_ns: fn() -> u64,
    timer: HybridTimer,
    mut loop_profiler: Option<LoopProfiler>,
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
            match loop_profiler.as_mut() {
                Some(p) => sim_loop.run_ticks_profiled(due, p, now_ns),
                None => sim_loop.run_ticks(due),
            }
            publish_input_overflow(&sim_loop, loop_profiler.as_mut());
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
    /// playfield surface cannot be drawn directly).
    Debug(GpuContext, SurfaceSet),
    /// The table renderer, drawn on the render thread and presented from it
    /// or from per-surface presenter threads ([`AppSurfaces`]: per-surface
    /// plans).
    Table(GpuContext, AppSurfaces),
}

/// Which presenter the render thread builds.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum PresenterKind {
    /// [`Presenter::Debug`].
    Debug,
    /// [`Presenter::Table`].
    Table,
}

/// The presenter for a table (`has_table`) whose surfaces got `plans`:
/// the table renderer whenever the playfield can be drawn directly
/// ([`table_set_accepted`]; a backglass / score surface that offers only
/// blocking present modes is presented by its own presenter thread, and one
/// without an 8-bit format is skipped; neither turns the cabinet into the
/// debug view), else the debug view.
fn choose_presenter(has_table: bool, plans: &[(SurfaceRole, SurfacePlan)]) -> PresenterKind {
    if has_table && table_set_accepted(plans) {
        PresenterKind::Table
    } else {
        PresenterKind::Debug
    }
}

/// Something that takes the sim's display commands (the table presenter).
trait DisplayCommandSink {
    fn apply_display_command(&mut self, cmd: DisplayCommand, now_ms: u64);
}

impl DisplayCommandSink for AppSurfaces {
    fn apply_display_command(&mut self, cmd: DisplayCommand, now_ms: u64) {
        AppSurfaces::apply_display_command(self, cmd, now_ms);
    }
}

/// Drains every pending display command into `sink` (the table presenter
/// whenever the table path was chosen); without one they are discarded
/// (the debug view has no displays). Returns the commands drained.
fn drain_display_commands<T: DisplayCommandSink>(
    rx: &mut HapticsConsumer<DisplayCommand>,
    mut sink: Option<&mut T>,
    now_ms: u64,
) -> usize {
    let mut n = 0;
    while let Some(cmd) = rx.pop() {
        if let Some(s) = sink.as_deref_mut() {
            s.apply_display_command(cmd, now_ms);
        }
        n += 1;
    }
    n
}

/// Builds the render thread's presenter on `ctx` (see [`choose_presenter`]).
fn build_presenter(
    ctx: GpuContext,
    surfaces: Vec<SurfaceSource>,
    surface_options: SurfaceOptions,
    table: Option<&TableVisuals>,
    table_render: TableRenderOptions,
) -> Result<Presenter, String> {
    let plans: Vec<(SurfaceRole, SurfacePlan)> = surfaces
        .iter()
        .zip(AppSurfaces::plan(&ctx, &surfaces, surface_options))
        .map(|(s, p)| (s.role(), p))
        .collect();
    match (table, choose_presenter(table.is_some(), &plans)) {
        (Some(visuals), PresenterKind::Table) => {
            AppSurfaces::new(&ctx, surfaces, surface_options, visuals, table_render)
                .map(|s| Presenter::Table(ctx, s))
        }
        (other, _) => {
            if other.is_some() {
                eprintln!(
                    "pin_app: the playfield surface offers no 8-bit format; drawing the debug \
                     view instead of the PBR table"
                );
            }
            SurfaceSet::new(&ctx, surfaces, surface_options)
                .map(|s| Presenter::Debug(ctx, s))
                .map_err(|e| e.to_string())
        }
    }
}

/// The GPU frame timer for the render thread's presenter on `ctx`, writing
/// to `profiler`, with one stderr line saying whether frames are timed.
fn start_gpu_timer(ctx: &GpuContext, profiler: PresentProfiler) -> Option<GpuFrameTimer> {
    let timer = GpuFrameTimer::new(ctx, profiler);
    match &timer {
        Some(t) => eprintln!(
            "pin_app: gpu frame timing: timestamp queries (period {} ns)",
            t.period_ns()
        ),
        None => eprintln!(
            "pin_app: gpu frame timing: unavailable (no TIMESTAMP_QUERY on this device); the \
             profiler records no GPU frame time"
        ),
    }
    timer
}

/// The profiler hooks of a debug-view presenter (`set` on `ctx`, w8g-d):
/// a [`GpuFrameTimer`] writing to `profiler` (when given; with the same
/// `pin_app: gpu frame timing: ...` stderr line as the table presenter)
/// and, with `hud`, a [`HudOverlay`] for the playfield surface's draw
/// format. A playfield format the HUD cannot draw onto
/// ([`hud_format_supported`]), or no playfield surface, gets one stderr
/// note and no HUD; timing is unaffected. `None` when the hooks would do
/// nothing (no profiler, no usable HUD): the render thread then draws with
/// plain [`SurfaceSet::render`], so default frames are unchanged.
pub fn debug_view_hooks(
    ctx: &GpuContext,
    set: &SurfaceSet,
    profiler: Option<PresentProfiler>,
    hud: bool,
) -> Option<DebugViewHooks> {
    let timer = profiler.and_then(|p| start_gpu_timer(ctx, p));
    let overlay = if hud {
        let playfield = set
            .choices()
            .position(|(_, role, _)| role == SurfaceRole::Playfield)
            .and_then(|i| set.formats().nth(i));
        match playfield {
            Some(f) if hud_format_supported(f, ctx.adapter().get_texture_format_features(f)) => {
                Some(HudOverlay::new(ctx.device(), f))
            }
            Some(f) => {
                eprintln!(
                    "pin_app: profiler HUD: the debug view's playfield format {f:?} cannot take \
                     the overlay; no HUD (GPU frame timing is unaffected)"
                );
                None
            }
            None => {
                eprintln!("pin_app: profiler HUD: the debug view has no playfield surface; no HUD");
                None
            }
        }
    } else {
        None
    };
    let hooks = DebugViewHooks::new(timer, overlay);
    (!hooks.is_empty()).then_some(hooks)
}

/// The HUD's rate lines from the orchestrator's counters (REQ-LAT-003):
/// cumulative dropped ticks and the presented frames per second measured by
/// `fps` since its previous reading at `now_ns`.
fn hud_rates(fps: &mut FrameRateMeter, stats: &OrchestratorStats, now_ns: u64) -> HudRates {
    HudRates {
        dropped_ticks: stats.sim_dropped_ticks.load(Ordering::Relaxed),
        presented_fps_milli: fps.sample(now_ns, stats.render_frames.load(Ordering::Relaxed)),
    }
}

/// The render thread's HUD refresh: when `feed` refreshes (at most once per
/// [`HUD_REFRESH_NS`]), its profiler lines followed by the rate lines
/// ([`hud_rate_lines`]: presented fps, dropped ticks; REQ-LAT-003). `None`
/// without a feed or between refreshes (keep showing the previous lines).
fn refresh_hud(
    feed: Option<&mut HudFeed>,
    fps: &mut FrameRateMeter,
    stats: &OrchestratorStats,
) -> Option<Vec<String>> {
    let now = monotonic_now_ns();
    let mut lines = feed?.refresh(now)?;
    lines.extend(hud_rate_lines(&hud_rates(fps, stats, now)));
    Some(lines)
}

fn render_thread(
    cfg: RenderConfig,
    mut reader: TripleReader<FrameStateSnapshot>,
    shutdown: &AtomicBool,
    stats: &OrchestratorStats,
    spawned: Instant,
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
        mut present_profiler,
        hud,
        mut latency_trace,
    } = cfg;
    // The HUD is drawn by the playfield's table renderer (or, in the debug
    // view, by `DebugViewHooks`), only when a reader is configured
    // (`--profiler-hud`): default frames are unchanged.
    let table_render = TableRenderOptions {
        profiler_hud: table_render.profiler_hud || hud.is_some(),
        ..table_render
    };
    let mut hud = hud.map(|r| HudFeed::new(r, HUD_REFRESH_NS));
    // Presented fps between HUD refreshes (REQ-LAT-003).
    let mut fps = FrameRateMeter::new();
    // The debug view's profiler hooks (`None`: plain `SurfaceSet::render`).
    let mut debug_hooks: Option<DebugViewHooks> = None;
    // GPU setup on this thread (the device lives here). Without surfaces
    // there is nothing to present, so no adapter is requested.
    let mut presenter = match gpu {
        Some(g) if !g.surfaces.is_empty() => {
            let ctx =
                GpuContext::with_instance(g.instance, &g.options, Some(&g.surfaces[0].surface));
            let made = ctx.map_err(|e| e.to_string()).and_then(|ctx| {
                build_presenter(
                    ctx,
                    g.surfaces,
                    g.surface_options,
                    table.as_deref(),
                    table_render,
                )
            });
            match made {
                Ok(mut p) => {
                    // GPU frame timing and the HUD for either presenter: the
                    // table presenter times itself and draws the HUD in its
                    // renderers; the debug view gets both through hooks.
                    match &mut p {
                        Presenter::Table(ctx, s) => {
                            if let Some(timer) = present_profiler
                                .take()
                                .and_then(|pp| start_gpu_timer(ctx, pp))
                            {
                                s.set_gpu_timer(timer);
                            }
                        }
                        Presenter::Debug(ctx, set) => {
                            debug_hooks =
                                debug_view_hooks(ctx, set, present_profiler.take(), hud.is_some());
                        }
                    }
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
                        Presenter::Table(_, s) => {
                            let (drawn, threaded, skipped) = s.plan_counts();
                            eprintln!(
                                "pin_app: render on {} ({:?}); PBR table on {} surface(s): \
                                 {drawn} direct / {threaded} presenter-thread / {skipped} skipped",
                                info.name,
                                info.backend,
                                s.len()
                            );
                        }
                    }
                    stats
                        .gpu_ready_ms
                        .store(ms_since(spawned), Ordering::Relaxed);
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
            let sink = match presenter.as_mut() {
                Some(Presenter::Table(_, s)) => Some(s),
                _ => None,
            };
            drain_display_commands(rx, sink, last_tick);
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
                let st = match debug_hooks.as_mut() {
                    Some(h) => {
                        if let Some(text) = refresh_hud(hud.as_mut(), &mut fps, stats) {
                            h.set_hud_lines(&text);
                        }
                        set.render_with(&lines, &camera, h)
                    }
                    // No profiler, no HUD: the default frames, unchanged.
                    None => set.render(&lines, &camera),
                };
                if let Some(t) = latency_trace.as_mut() {
                    t.presented(&st, last_tick);
                }
                stats.record_statuses(&st, spawned);
                paced = set.frame_paced(&st);
            }
            Some(Presenter::Table(_, s)) => {
                if let Some(lines) = refresh_hud(hud.as_mut(), &mut fps, stats) {
                    s.set_hud_lines(&lines);
                }
                let st = s.render(snap);
                if let Some(t) = latency_trace.as_mut() {
                    t.presented(&st, last_tick);
                }
                stats.record_statuses(&st, spawned);
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

#[cfg(test)]
mod tests {
    use super::*;
    use pin_render::gpu::SurfaceStatus as S;
    use pin_render::gpu::wgpu::PresentMode;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn run_limits_parse_the_flags() {
        assert_eq!(RunLimits::from_args(&[]).unwrap(), RunLimits::default());
        let l = RunLimits::from_args(&args(&[
            "--table",
            "t",
            "--run-frames",
            "3",
            "--run-ticks",
            "100",
            "--run-ms",
            "120000",
        ]))
        .unwrap();
        assert_eq!(
            l,
            RunLimits {
                frames: Some(3),
                ticks: Some(100),
                cap: Some(Duration::from_millis(120_000)),
            }
        );
        assert!(l.has_conditions());
        let l = RunLimits::from_args(&args(&["--run-ms", "5"])).unwrap();
        assert!(!l.has_conditions());
        for bad in [
            &["--run-frames", "x"][..],
            &["--run-ticks", "x"],
            &["--run-ms", "x"],
            &["--run-frames"],
            &["--run-ticks", "-1"],
        ] {
            let e = RunLimits::from_args(&args(bad)).unwrap_err();
            assert!(e.starts_with(bad[0]), "{bad:?}: {e}");
        }
    }

    fn stats(frames: u64, ticks: u64) -> StatsSnapshot {
        StatsSnapshot {
            render_frames: frames,
            sim_ticks: ticks,
            ..StatsSnapshot::default()
        }
    }

    #[test]
    fn run_condition_needs_every_given_condition() {
        let both = RunLimits {
            frames: Some(3),
            ticks: Some(100),
            cap: Some(Duration::from_secs(120)),
        };
        assert!(!both.conditions_met(&stats(2, 1000)));
        assert!(!both.conditions_met(&stats(30, 99)));
        assert!(both.conditions_met(&stats(3, 100)));
        let frames = RunLimits {
            frames: Some(1),
            ..RunLimits::default()
        };
        assert!(!frames.conditions_met(&stats(0, 10_000)));
        assert!(frames.conditions_met(&stats(1, 0)));
        let ticks = RunLimits {
            ticks: Some(600),
            ..RunLimits::default()
        };
        assert!(ticks.conditions_met(&stats(0, 600)));
        assert!(!ticks.conditions_met(&stats(9, 599)));
        // No condition: never "met" (only the cap or a quit ends the run).
        assert!(!RunLimits::default().conditions_met(&stats(u64::MAX, u64::MAX)));
    }

    #[test]
    fn run_check_distinguishes_met_cap_and_plain_duration() {
        let s = Duration::from_millis;
        let l = RunLimits {
            frames: Some(3),
            ticks: Some(100),
            cap: Some(s(1000)),
        };
        assert_eq!(l.check(&stats(0, 0), s(999)), None);
        assert_eq!(l.check(&stats(3, 100), s(0)), Some(RunExit::ConditionMet));
        // Met conditions win over an elapsed cap.
        assert_eq!(
            l.check(&stats(3, 100), s(5000)),
            Some(RunExit::ConditionMet)
        );
        assert_eq!(l.check(&stats(2, 100), s(1000)), Some(RunExit::CapReached));
        let plain = RunLimits {
            cap: Some(s(50)),
            ..RunLimits::default()
        };
        assert_eq!(plain.check(&stats(0, 0), s(49)), None);
        assert_eq!(plain.check(&stats(0, 0), s(50)), Some(RunExit::Elapsed));
        // Conditions without a cap: run until they hold.
        let uncapped = RunLimits {
            ticks: Some(10),
            ..RunLimits::default()
        };
        assert_eq!(uncapped.check(&stats(0, 9), s(u64::MAX)), None);
        assert_eq!(
            uncapped.check(&stats(0, 10), s(0)),
            Some(RunExit::ConditionMet)
        );
    }

    #[test]
    fn render_diagnostics_line() {
        let s = StatsSnapshot::default();
        assert_eq!(
            s.render_diagnostics(),
            "pin_app: render: gpu ready after - ms, first frame after - ms, 0 iterations, \
             0 presented / 0 occluded / 0 skipped / 0 hidden"
        );
        let s = StatsSnapshot {
            gpu_ready_ms: Some(812),
            first_frame_ms: Some(0),
            render_iterations: 42,
            render_presented: 7,
            render_occluded: 3,
            render_skipped: 2,
            render_hidden: 1,
            ..StatsSnapshot::default()
        };
        assert_eq!(
            s.render_diagnostics(),
            "pin_app: render: gpu ready after 812 ms, first frame after 0 ms, 42 iterations, \
             7 presented / 3 occluded / 2 skipped / 1 hidden"
        );
    }

    #[test]
    fn statuses_are_counted_per_surface_and_frames_once_per_iteration() {
        let st = OrchestratorStats::default();
        let spawned = Instant::now();
        assert_eq!(st.snapshot().first_frame_ms, None);
        assert_eq!(st.snapshot().gpu_ready_ms, None);
        st.record_statuses(&[S::Occluded, S::Hidden], spawned);
        st.record_statuses(&[S::Skipped, S::Unchanged, S::Busy], spawned);
        assert_eq!(st.snapshot().render_frames, 0);
        assert_eq!(st.snapshot().first_frame_ms, None);
        st.record_statuses(&[S::Presented, S::Presented, S::Occluded], spawned);
        st.record_statuses(&[S::Presented], spawned);
        let s = st.snapshot();
        assert_eq!(s.render_frames, 2);
        assert_eq!(s.render_presented, 3);
        assert_eq!(s.render_occluded, 2);
        assert_eq!(s.render_skipped, 1);
        assert_eq!(s.render_hidden, 1);
        assert!(s.first_frame_ms.is_some());
    }

    #[derive(Default)]
    struct Recorder(Vec<(DisplayCommand, u64)>);

    impl DisplayCommandSink for Recorder {
        fn apply_display_command(&mut self, cmd: DisplayCommand, now_ms: u64) {
            self.0.push((cmd, now_ms));
        }
    }

    /// PR #21 review (threads.rs:621): a Fifo-only score window keeps the
    /// table presenter, and the display commands reach it.
    #[test]
    fn a_fifo_only_score_surface_keeps_the_table_and_its_display_commands() {
        let opts = SurfaceOptions::default();
        let formats = [wgpu::TextureFormat::Bgra8UnormSrgb];
        let plans = [
            (
                SurfaceRole::Playfield,
                super::super::present::plan_surface(
                    SurfaceRole::Playfield,
                    &formats,
                    &[PresentMode::Fifo, PresentMode::Mailbox],
                    opts,
                ),
            ),
            (
                SurfaceRole::Score,
                super::super::present::plan_surface(
                    SurfaceRole::Score,
                    &formats,
                    &[PresentMode::Fifo],
                    opts,
                ),
            ),
        ];
        assert_eq!(plans[1].1, SurfacePlan::Presenter(PresentMode::Fifo));
        assert_eq!(choose_presenter(true, &plans), PresenterKind::Table);
        // PR #21 round 4: the Fifo-only score window is retained on its own
        // presenter thread (not dropped); the playfield is drawn directly,
        // and both renderers receive every display command.
        let f = Some(wgpu::TextureFormat::Bgra8UnormSrgb);
        assert_eq!(
            super::super::present::slot_decision(plans[1].0, plans[1].1, f),
            super::super::present::SlotDecision::Present(
                PresentMode::Fifo,
                wgpu::TextureFormat::Bgra8UnormSrgb
            )
        );
        assert!(matches!(
            super::super::present::slot_decision(plans[0].0, plans[0].1, f),
            super::super::present::SlotDecision::Draw(PresentMode::Fifo, _)
        ));
        assert_eq!(choose_presenter(false, &plans), PresenterKind::Debug);
        // Playfield without an 8-bit format: the debug view.
        let bad = [(SurfaceRole::Playfield, SurfacePlan::Unsupported), plans[1]];
        assert_eq!(choose_presenter(true, &bad), PresenterKind::Debug);

        let (mut tx, mut rx) = pin_input::haptics::haptics_ring(8);
        let cmds = [
            DisplayCommand::Scene(pin_core::SceneId(2)),
            DisplayCommand::Score {
                player: 0,
                score: 1230,
            },
        ];
        for c in cmds {
            assert!(tx.push(c));
        }
        let mut table = Recorder::default();
        assert_eq!(drain_display_commands(&mut rx, Some(&mut table), 77), 2);
        assert_eq!(table.0, vec![(cmds[0], 77), (cmds[1], 77)]);
        // Without a table presenter they are drained and dropped.
        assert!(tx.push(cmds[0]));
        assert_eq!(drain_display_commands::<Recorder>(&mut rx, None, 78), 1);
        assert_eq!(drain_display_commands(&mut rx, Some(&mut table), 79), 0);
        assert_eq!(table.0.len(), 2);
    }
}
