//! wgpu multi-surface context (PLAN §2.2, §4 Subphase 1.4).
//!
//! * [`GpuContext`]: the engine's single wgpu instance, adapter, device and
//!   queue. It lives on the render thread.
//! * [`choose_present_mode`]: the §2.2 / D19 present-mode policy per
//!   [`SurfaceRole`].
//! * [`SurfaceSet`] (in `surfaces`): one to three configured surfaces created on
//!   the main thread from SDL windows and presented from the render thread; a
//!   secondary surface that is only offered `Fifo` gets its own presenter
//!   thread so it cannot stall the playfield.
//!
//! Adapter selection follows the 0.1 spike (DECISIONS "0.1 — Dependency spike
//! results", Q5): [`GpuOptions::from_env`] honours `WGPU_BACKEND` and forces
//! the fallback (software) adapter in CI, so Linux CI gets lavapipe and
//! Windows CI gets WARP; macOS uses Metal.

mod blit;
mod quality;
mod surfaces;

pub use quality::{
    LeanQuality, PLAYFIELD_FRAME_LATENCY, SECONDARY_FRAME_LATENCY, frame_latency_for,
    is_cpu_adapter, lean_quality_for,
};

pub use surfaces::{
    SurfaceOptions, SurfaceSet, SurfaceSource, SurfaceStatus, create_surface, paced_by,
    role_clear_color,
};
// 6.0 (issue #9a): window lifetime guard for surfaces and presenter threads.
pub use surfaces::{PRESENTER_JOIN_TIMEOUT, WindowGuard, spawn_guarded};
// Issue #10: display layers composite in display-encoded space on `*Srgb`
// (fallback) surfaces too.
pub use surfaces::{LayerCompositor, LayerTarget};
// w8g-d: per-frame hooks (GPU timing, overlay) into `SurfaceSet::render_with`
// and the debug-view frame recorder they wrap.
pub use surfaces::{DebugFrame, NoFrameHooks, SurfaceFrameHooks, encode_debug_frame};

/// The wgpu crate as used by `pin_render` (so dependants need no own pin).
pub use wgpu;

use crate::topology::SurfaceRole;
use core::fmt;
use wgpu::{Backends, PresentMode};

/// Environment variable that turns "no adapter" into a failure (set in CI).
pub const REQUIRE_GPU_ENV: &str = "PIN_REQUIRE_GPU";

/// Optional override for [`GpuOptions::force_fallback_adapter`] (`0` or `1`).
pub const FORCE_FALLBACK_ENV: &str = "PIN_FORCE_FALLBACK_ADAPTER";

/// `true` when `PIN_REQUIRE_GPU=1`: GPU tests must fail instead of skipping
/// when no adapter is found.
pub fn require_gpu() -> bool {
    std::env::var(REQUIRE_GPU_ENV).is_ok_and(|v| v.trim() == "1")
}

/// How a surface presents.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum PresentChoice {
    /// Presented from the render thread with this mode.
    Direct(PresentMode),
    /// Presented from a dedicated presenter thread with this mode, so its
    /// blocking acquire cannot stall the playfield (D19).
    PresenterThread(PresentMode),
}

impl PresentChoice {
    /// The present mode to configure.
    pub const fn mode(self) -> PresentMode {
        match self {
            PresentChoice::Direct(m) | PresentChoice::PresenterThread(m) => m,
        }
    }

    /// `true` for [`PresentChoice::PresenterThread`].
    pub const fn needs_presenter_thread(self) -> bool {
        matches!(self, PresentChoice::PresenterThread(_))
    }
}

/// The §2.2 present-mode policy.
///
/// * Playfield: `Direct(FifoRelaxed)` if `vrr` and `offered` contains it, else
///   `Direct(Fifo)` (the playfield paces the frame, D19).
/// * Backglass / Score (redraw only on change): `Direct(Mailbox)` if offered,
///   else `Direct(Immediate)` if offered, else `PresenterThread(Fifo)`.
///
/// An empty `offered` list is treated as `[Fifo]` (the only guaranteed mode).
/// `AutoVsync` / `AutoNoVsync` in `offered` are ignored: they are wgpu
/// fallback aliases, not modes a surface reports.
pub fn choose_present_mode(role: SurfaceRole, offered: &[PresentMode], vrr: bool) -> PresentChoice {
    let has = |m: PresentMode| offered.contains(&m);
    match role {
        SurfaceRole::Playfield => {
            if vrr && has(PresentMode::FifoRelaxed) {
                PresentChoice::Direct(PresentMode::FifoRelaxed)
            } else {
                PresentChoice::Direct(PresentMode::Fifo)
            }
        }
        SurfaceRole::Backglass | SurfaceRole::Score => {
            if has(PresentMode::Mailbox) {
                PresentChoice::Direct(PresentMode::Mailbox)
            } else if has(PresentMode::Immediate) {
                PresentChoice::Direct(PresentMode::Immediate)
            } else {
                PresentChoice::PresenterThread(PresentMode::Fifo)
            }
        }
    }
}

/// Adapter selection options.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GpuOptions {
    /// Request the fallback (software) adapter: lavapipe on Linux, WARP on
    /// Windows. Forced in CI by [`GpuOptions::from_env`].
    pub force_fallback_adapter: bool,
    /// Backends to enable.
    pub backends: Backends,
}

impl Default for GpuOptions {
    /// The platform's primary backend (Vulkan, Metal or DX12), any adapter.
    fn default() -> Self {
        Self {
            force_fallback_adapter: false,
            backends: Backends::PRIMARY,
        }
    }
}

impl GpuOptions {
    /// Options for tests and CI from the process environment, see
    /// [`GpuOptions::from_vars`]. The application uses
    /// [`GpuOptions::for_app`] instead, so a `CI=1` in a user's shell never
    /// selects the software adapter.
    pub fn from_env() -> Self {
        Self::from_vars(|k| std::env::var(k).ok())
    }

    /// Options for the application from the process environment, see
    /// [`GpuOptions::app_from_vars`].
    pub fn for_app() -> Self {
        Self::app_from_vars(|k| std::env::var(k).ok())
    }

    /// Application options: `WGPU_BACKEND` as in [`from_vars`](Self::from_vars);
    /// the fallback adapter only with an explicit
    /// `PIN_FORCE_FALLBACK_ADAPTER=1` (CI variables are ignored).
    pub fn app_from_vars(get: impl Fn(&str) -> Option<String>) -> Self {
        Self {
            force_fallback_adapter: get(FORCE_FALLBACK_ENV).is_some_and(|v| v.trim() == "1"),
            backends: backends_from(&get),
        }
    }

    /// Test/CI options from an environment lookup (on this OS):
    ///
    /// * `WGPU_BACKEND` — comma-separated `vulkan|dx12|metal|gl` (case
    ///   insensitive; `vk`, `d3d12`, `mtl`, `gles`, `opengl` accepted).
    ///   Unknown names are ignored; nothing valid → the default backends.
    /// * The fallback adapter is forced in CI (`CI` = `true`/`1`, or
    ///   `PIN_REQUIRE_GPU=1`), as the 0.1 spike did, except on macOS or when
    ///   only Metal is enabled (Metal has no fallback adapter).
    ///   `PIN_FORCE_FALLBACK_ADAPTER` = `0`/`1` overrides that choice.
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> Self {
        Self::from_vars_on(cfg!(target_os = "macos"), get)
    }

    /// [`from_vars`](Self::from_vars) as it behaves on macOS (`macos`) or
    /// elsewhere.
    pub fn from_vars_on(macos: bool, get: impl Fn(&str) -> Option<String>) -> Self {
        let backends = backends_from(&get);
        let truthy = |k: &str| {
            get(k).is_some_and(|v| {
                let v = v.trim();
                v == "1" || v.eq_ignore_ascii_case("true")
            })
        };
        let in_ci = truthy("CI") || get(REQUIRE_GPU_ENV).is_some_and(|v| v.trim() == "1");
        let no_fallback = macos || backends == Backends::METAL;
        let mut force_fallback_adapter = in_ci && !no_fallback;
        match get(FORCE_FALLBACK_ENV).as_deref().map(str::trim) {
            Some("1") => force_fallback_adapter = true,
            Some("0") => force_fallback_adapter = false,
            _ => {}
        }
        Self {
            force_fallback_adapter,
            backends,
        }
    }
}

fn backends_from(get: &impl Fn(&str) -> Option<String>) -> Backends {
    get("WGPU_BACKEND")
        .and_then(|v| parse_backends(&v))
        .unwrap_or(GpuOptions::default().backends)
}

/// Parses a `WGPU_BACKEND` value; `None` when no name is valid.
pub fn parse_backends(value: &str) -> Option<Backends> {
    let mut out = Backends::empty();
    for name in value.split(',') {
        let name = name.trim().to_ascii_lowercase();
        out |= match name.as_str() {
            "vulkan" | "vk" => Backends::VULKAN,
            "dx12" | "d3d12" => Backends::DX12,
            "metal" | "mtl" => Backends::METAL,
            "gl" | "gles" | "opengl" => Backends::GL,
            _ => Backends::empty(),
        };
    }
    (!out.is_empty()).then_some(out)
}

/// GPU errors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuError {
    /// No adapter matched the options.
    NoAdapter(String),
    /// The adapter refused to create a device.
    RequestDevice(String),
    /// A surface could not be created or configured.
    Surface(String),
    /// Reading back a rendered image failed.
    Readback(String),
    /// Invalid arguments (e.g. a zero-sized image).
    InvalidArgument(String),
    /// The device was lost (driver reset, `Device::destroy`, ...): the
    /// payload is the loss reason only ([`GpuContext::device_lost_reason`],
    /// e.g. `"Destroyed"` or `"Unknown: <message>"`). Nothing created on the
    /// device can be used again; the application rebuilds the context and
    /// its renderers (SPEC §4 Subphase 5.4 item 4).
    DeviceLost(String),
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuError::NoAdapter(e) => write!(f, "no GPU adapter: {e}"),
            GpuError::RequestDevice(e) => write!(f, "GPU device request failed: {e}"),
            GpuError::Surface(e) => write!(f, "GPU surface error: {e}"),
            GpuError::Readback(e) => write!(f, "GPU readback failed: {e}"),
            GpuError::InvalidArgument(e) => write!(f, "invalid argument: {e}"),
            GpuError::DeviceLost(e) => write!(f, "GPU device lost: {e}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// The engine's single wgpu instance, adapter, device and queue.
///
/// Creating and destroying contexts is serialised process-wide (see
/// [`lifecycle_lock`]); using them is not.
///
/// # Device loss
///
/// Every constructor registers the device's one device-lost callback
/// (`wgpu::Device::set_device_lost_callback`) before the context is
/// returned; it records the loss in a flag shared with everything built on
/// the context ([`device_lost`](Self::device_lost),
/// [`device_lost_reason`](Self::device_lost_reason); the ray-query
/// reflection source reads the same flag). wgpu 30 calls it for a driver
/// loss (`Unknown`) and after `Device::destroy` (`Destroyed`, delivered by
/// the next device maintenance, e.g. a poll), never for a plain drop, so
/// normal teardown does not set it. Do not register another device-lost
/// callback on [`device`](Self::device): wgpu keeps one per device and a
/// later one would replace the context's.
pub struct GpuContext {
    /// `None` only while dropping.
    inner: Option<Inner>,
    /// Set by the device-lost callback (first reason wins).
    loss: std::sync::Arc<LossFlag>,
}

/// Device-loss state shared between a [`GpuContext`], the wgpu device-lost
/// callback its constructor registers (called from wgpu threads) and the
/// objects built on the context (SPEC §4 Subphase 5.4 item 4).
#[derive(Debug, Default)]
pub(crate) struct LossFlag {
    lost: std::sync::atomic::AtomicBool,
    reason: std::sync::OnceLock<String>,
}

impl LossFlag {
    /// Records the loss: the first reason wins, the flag is set after the
    /// reason so a reader that sees the flag also sees a reason.
    pub(crate) fn set(&self, reason: &str) {
        let _ = self.reason.set(reason.to_owned());
        self.lost.store(true, std::sync::atomic::Ordering::Release);
    }

    /// `true` once the device is known to be lost.
    pub(crate) fn is_set(&self) -> bool {
        self.lost.load(std::sync::atomic::Ordering::Acquire)
    }

    /// The first recorded reason (empty before a loss).
    pub(crate) fn reason(&self) -> &str {
        self.reason.get().map_or("", String::as_str)
    }

    /// The reason once the device is lost, `None` before.
    pub(crate) fn lost_reason(&self) -> Option<String> {
        self.is_set().then(|| self.reason().to_owned())
    }
}

/// The reason string recorded for a wgpu device-lost callback:
/// `"<reason>"`, or `"<reason>: <message>"` with a message.
pub(crate) fn device_lost_reason(reason: wgpu::DeviceLostReason, message: &str) -> String {
    if message.is_empty() {
        format!("{reason:?}")
    } else {
        format!("{reason:?}: {message}")
    }
}

/// Fields in teardown order (queue first, instance last).
struct Inner {
    queue: wgpu::Queue,
    device: wgpu::Device,
    adapter: wgpu::Adapter,
    instance: wgpu::Instance,
}

/// Serialises driver-level setup and teardown (instance creation, adapter
/// and device requests, device destruction) across threads.
///
/// The engine has one context, but test harnesses run many tests on
/// parallel threads, each making and dropping its own context. Software
/// drivers are not reliably safe under concurrent instance/device creation
/// and destruction: lavapipe crashed with SIGSEGV in CI under exactly that
/// load (DECISIONS "1.4 — CI fix: lavapipe SIGSEGV and macOS occlusion").
/// Rendering on several live contexts at once stays concurrent.
fn lifecycle_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Drop for GpuContext {
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else {
            return;
        };
        let _g = lifecycle_lock();
        // Let submitted work finish before the device (if this was its last
        // handle) and the instance are destroyed. The wait is bounded: a hung
        // device must not hold the process-wide lock (and so every other
        // context's setup and teardown) forever (1.4 gate-fix N5).
        if let Err(e) = wait_for_device(&inner.device, TEARDOWN_WAIT_TIMEOUT) {
            eprintln!("pin_render: device did not go idle before teardown: {e}");
        }
        drop(inner);
    }
}

/// Longest wait for the device to go idle before a context is torn down
/// (under the process-wide lifecycle lock).
pub const TEARDOWN_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Longest wait for submitted work in a readback (offscreen renders on a
/// software adapter under load can take seconds).
pub const READBACK_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Waits until the device finished its submitted work, at most `timeout`
/// (`wgpu::PollError::Timeout` after that). Every device wait in
/// `pin_render` is bounded; none waits indefinitely.
pub fn wait_for_device(
    device: &wgpu::Device,
    timeout: std::time::Duration,
) -> Result<(), wgpu::PollError> {
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(timeout),
        })
        .map(|_| ())
}

/// Byte size of a readback buffer of `height` rows of `padded_row` bytes on
/// `device`, or [`GpuError::Readback`] when it exceeds the device's
/// `max_buffer_size` (a texture the device accepts can still be too large
/// to read back in one buffer; creating that buffer would be a validation
/// error).
pub(crate) fn readback_buffer_size(
    device: &wgpu::Device,
    padded_row: usize,
    height: u32,
) -> Result<u64, GpuError> {
    checked_readback_size(device.limits().max_buffer_size, padded_row, height)
}

/// [`readback_buffer_size`] for a given `max_buffer_size`.
pub(crate) fn checked_readback_size(
    max_buffer_size: u64,
    padded_row: usize,
    height: u32,
) -> Result<u64, GpuError> {
    (padded_row as u64)
        .checked_mul(u64::from(height))
        .filter(|&n| n <= max_buffer_size)
        .ok_or_else(|| {
            GpuError::Readback(format!(
                "a {height}-row image of {padded_row} bytes per row needs a readback buffer \
                 larger than the device's max_buffer_size ({max_buffer_size} bytes)"
            ))
        })
}

/// Result channel of a `Buffer::map_async` callback.
pub(crate) type MapReceiver = std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>;

/// Waits (bounded by `timeout` in total) for the device to finish and for
/// the `map_async` callback to report on `rx` (5.1 review N6: a poll that
/// returns without the callback having run must not block forever).
pub(crate) fn finish_map(
    device: &wgpu::Device,
    rx: &MapReceiver,
    timeout: std::time::Duration,
) -> Result<(), GpuError> {
    let start = std::time::Instant::now();
    wait_for_device(device, timeout).map_err(|e| GpuError::Readback(format!("poll: {e}")))?;
    // The callback normally ran inside the poll; allow a short grace period
    // even when the poll used up the whole budget.
    let left = timeout
        .saturating_sub(start.elapsed())
        .max(std::time::Duration::from_millis(100));
    recv_map(rx, left)
}

/// The `map_async` result from `rx`, waiting at most `timeout`.
pub(crate) fn recv_map(rx: &MapReceiver, timeout: std::time::Duration) -> Result<(), GpuError> {
    use std::sync::mpsc::RecvTimeoutError;
    match rx.recv_timeout(timeout) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(GpuError::Readback(format!("map_async: {e}"))),
        Err(RecvTimeoutError::Timeout) => Err(GpuError::Readback(format!(
            "map_async callback did not run within {timeout:?}"
        ))),
        Err(RecvTimeoutError::Disconnected) => Err(GpuError::Readback(
            "map_async callback dropped without a result".into(),
        )),
    }
}

/// Creates a wgpu instance over `backends` (instance flags honour the
/// `WGPU_*` debug environment variables).
pub fn create_instance(backends: Backends) -> wgpu::Instance {
    let _g = lifecycle_lock();
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = backends;
    desc.flags = desc.flags.with_env();
    wgpu::Instance::new(desc)
}

/// Features every context enables whenever the adapter offers them
/// (timestamp queries for GPU frame timing); never required.
fn optional_timestamp_features() -> wgpu::Features {
    wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
}

/// The features a context's device requests on an adapter offering
/// `adapter`, when the caller wants `wanted`: `(wanted | timestamp
/// queries) & adapter`. Features the adapter lacks are silently left out
/// (check [`wgpu::Device::features`]); the timestamp queries
/// (`TIMESTAMP_QUERY`, `TIMESTAMP_QUERY_INSIDE_ENCODERS`) are always added
/// when offered. Pure; every [`GpuContext`] constructor requests exactly
/// this (SPEC §4 Subphase 5.4 item 4, §4 Subphase 1.4).
pub fn device_features(adapter: wgpu::Features, wanted: wgpu::Features) -> wgpu::Features {
    (wanted | optional_timestamp_features()) & adapter
}

/// What the shared build path asks the adapter for (pure, so both
/// constructors' inputs are testable without a GPU).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DeviceRequest {
    /// [`device_features`] of the adapter's features and `wanted`.
    pub(crate) features: wgpu::Features,
    /// Downlevel defaults at the adapter's resolution limits, plus the
    /// adapter's acceleration-structure limits with ray queries.
    pub(crate) limits: wgpu::Limits,
    /// The experimental-features opt-in is needed (an experimental feature
    /// such as [`wgpu::Features::EXPERIMENTAL_RAY_QUERY`] is requested).
    pub(crate) experimental: bool,
}

/// The device request for an adapter with `adapter_features` and
/// `adapter_limits` when `wanted` is asked for (the one request path of
/// every [`GpuContext`] constructor).
pub(crate) fn device_request(
    adapter_features: wgpu::Features,
    adapter_limits: &wgpu::Limits,
    wanted: wgpu::Features,
) -> DeviceRequest {
    let features = device_features(adapter_features, wanted);
    let mut limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter_limits.clone());
    if features.contains(wgpu::Features::EXPERIMENTAL_RAY_QUERY) {
        limits = limits.using_acceleration_structure_values(adapter_limits.clone());
    }
    DeviceRequest {
        features,
        limits,
        experimental: features.intersects(wgpu::Features::all_experimental_mask()),
    }
}

impl GpuContext {
    /// A context with no surface (offscreen rendering, tests, CI).
    pub fn new_headless(opts: &GpuOptions) -> Result<GpuContext, GpuError> {
        Self::with_instance(create_instance(opts.backends), opts, None)
    }

    /// A context on an existing instance whose adapter can present to
    /// `compatible_surface` (created from the same instance). The device
    /// enables `TIMESTAMP_QUERY` and `TIMESTAMP_QUERY_INSIDE_ENCODERS` when
    /// the adapter offers them (check [`wgpu::Device::features`]); nothing
    /// else beyond the defaults. Same as
    /// [`with_instance_features`](Self::with_instance_features) with
    /// `wgpu::Features::empty()`.
    pub fn with_instance(
        instance: wgpu::Instance,
        opts: &GpuOptions,
        compatible_surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<GpuContext, GpuError> {
        Self::build(instance, opts, compatible_surface, wgpu::Features::empty())
    }

    /// [`with_instance`](Self::with_instance) whose device additionally
    /// enables every feature of `wanted` the adapter reports
    /// ([`device_features`]; features the adapter lacks are silently left
    /// out: check [`wgpu::Device::features`]). Experimental features (e.g.
    /// [`wgpu::Features::EXPERIMENTAL_RAY_QUERY`], SPEC §4 Subphase 5.4)
    /// are opted into only when requested and supported; with ray queries
    /// the adapter's acceleration-structure limits are requested as well.
    /// The windowed counterpart of
    /// [`new_headless_with_features`](Self::new_headless_with_features):
    /// both go through the same request path.
    pub fn with_instance_features(
        instance: wgpu::Instance,
        opts: &GpuOptions,
        compatible_surface: Option<&wgpu::Surface<'_>>,
        wanted: wgpu::Features,
    ) -> Result<GpuContext, GpuError> {
        Self::build(instance, opts, compatible_surface, wanted)
    }

    /// A headless context like [`new_headless`](Self::new_headless) whose
    /// device additionally enables every feature of `wanted` that the
    /// adapter reports (5.4): [`create_instance`] followed by
    /// [`with_instance_features`](Self::with_instance_features) without a
    /// surface. Features the adapter lacks are silently left out: check
    /// [`wgpu::Device::features`] on the result. Timestamp queries are
    /// enabled when offered, as for every context.
    pub fn new_headless_with_features(
        opts: &GpuOptions,
        wanted: wgpu::Features,
    ) -> Result<GpuContext, GpuError> {
        Self::build(create_instance(opts.backends), opts, None, wanted)
    }

    /// The one adapter / device request path of every constructor: the
    /// adapter for `compatible_surface`, then a device with
    /// [`device_request`], then the device-lost callback that records a
    /// loss in the context's flag. Runs under the [`lifecycle_lock`].
    fn build(
        instance: wgpu::Instance,
        opts: &GpuOptions,
        compatible_surface: Option<&wgpu::Surface<'_>>,
        wanted: wgpu::Features,
    ) -> Result<GpuContext, GpuError> {
        let _g = lifecycle_lock();
        let adapter =
            match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: opts.force_fallback_adapter,
                compatible_surface,
                apply_limit_buckets: false,
            })) {
                Ok(a) => a,
                Err(e) => {
                    drop(instance); // under the lock
                    return Err(GpuError::NoAdapter(format!(
                        "{e} (backends {:?}, force_fallback_adapter {})",
                        opts.backends, opts.force_fallback_adapter
                    )));
                }
            };
        let request = device_request(adapter.features(), &adapter.limits(), wanted);
        let experimental = if request.experimental {
            // SAFETY: the experimental features are requested explicitly by
            // the caller (the optional ray-query path of 5.4, which falls
            // back to probes on any runtime error); see
            // `wgpu::ExperimentalFeatures::enabled`.
            unsafe { wgpu::ExperimentalFeatures::enabled() }
        } else {
            wgpu::ExperimentalFeatures::disabled()
        };
        let (device, queue) =
            match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("pin_render"),
                // GPU timestamp queries (the app's GPU frame-time profiler)
                // only where the adapter offers them, plus what the caller
                // wanted and the adapter has: check `device.features()`.
                required_features: request.features,
                required_limits: request.limits,
                experimental_features: experimental,
                ..Default::default()
            })) {
                Ok(dq) => dq,
                Err(e) => {
                    drop(adapter); // under the lock
                    drop(instance);
                    return Err(GpuError::RequestDevice(e.to_string()));
                }
            };
        let loss = std::sync::Arc::new(LossFlag::default());
        let cb_loss = std::sync::Arc::clone(&loss);
        device.set_device_lost_callback(move |reason, message| {
            cb_loss.set(&device_lost_reason(reason, &message));
        });
        Ok(GpuContext {
            inner: Some(Inner {
                queue,
                device,
                adapter,
                instance,
            }),
            loss,
        })
    }

    /// `true` once the device is lost (its device-lost callback ran: a
    /// driver loss, or `Device::destroy` followed by a device poll).
    /// Dropping a context never sets it. Thread-safe.
    pub fn device_lost(&self) -> bool {
        self.loss.is_set()
    }

    /// The loss reason once [`device_lost`](Self::device_lost) is true
    /// (`"Destroyed"`, `"Unknown: <message>"`, ...; the first reason
    /// wins), `None` before.
    pub fn device_lost_reason(&self) -> Option<String> {
        self.loss.lost_reason()
    }

    /// The loss state shared with objects built on this context.
    pub(crate) fn loss_flag(&self) -> &std::sync::Arc<LossFlag> {
        &self.loss
    }

    fn inner(&self) -> &Inner {
        self.inner.as_ref().expect("GpuContext used while dropping")
    }

    /// The instance (create surfaces from it).
    pub fn instance(&self) -> &wgpu::Instance {
        &self.inner().instance
    }

    /// The adapter.
    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.inner().adapter
    }

    /// The device.
    pub fn device(&self) -> &wgpu::Device {
        &self.inner().device
    }

    /// The queue.
    pub fn queue(&self) -> &wgpu::Queue {
        &self.inner().queue
    }

    /// Adapter name, backend, type and driver.
    pub fn adapter_info(&self) -> wgpu::AdapterInfo {
        self.inner().adapter.get_info()
    }
}

/// Test helper (not part of the engine API): a headless context for GPU tests, or `None` (with a printed note) when no
/// adapter is available. Panics instead when [`require_gpu`] is set.
#[doc(hidden)]
pub fn context_for_tests(test: &str) -> Option<GpuContext> {
    match GpuContext::new_headless(&GpuOptions::from_env()) {
        Ok(ctx) => {
            let info = ctx.adapter_info();
            println!(
                "[{test}] adapter: {} ({:?}, {:?})",
                info.name, info.backend, info.device_type
            );
            Some(ctx)
        }
        Err(e) if require_gpu() => panic!("[{test}] {REQUIRE_GPU_ENV}=1 but {e}"),
        Err(e) => {
            println!("[{test}] SKIP: {e}; set {REQUIRE_GPU_ENV}=1 to fail instead");
            None
        }
    }
}

/// Test helper for the `pin_render --lib` unit tests: one process-wide headless context, created on first use and never
/// dropped. Two lavapipe devices alive at once (one being created or destroyed while the other renders) crash the
/// driver, so the lib tests must not each build their own; see DECISIONS "Wave 5 gate fix", round 3. `None` when no
/// adapter is available (the PIN_REQUIRE_GPU panic and SKIP note come from [`context_for_tests`]).
#[cfg(all(test, feature = "gpu-tests"))]
pub(crate) fn shared_context_for_tests(test: &str) -> Option<&'static GpuContext> {
    use std::sync::OnceLock;
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    println!("[{test}] using the shared lib-test context");
    CTX.get_or_init(|| context_for_tests("lib-shared")).as_ref()
}

#[cfg(test)]
mod tests;
