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
mod surfaces;

pub use surfaces::{
    SurfaceOptions, SurfaceSet, SurfaceSource, SurfaceStatus, create_surface, paced_by,
    role_clear_color,
};

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
}

impl fmt::Display for GpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuError::NoAdapter(e) => write!(f, "no GPU adapter: {e}"),
            GpuError::RequestDevice(e) => write!(f, "GPU device request failed: {e}"),
            GpuError::Surface(e) => write!(f, "GPU surface error: {e}"),
            GpuError::Readback(e) => write!(f, "GPU readback failed: {e}"),
            GpuError::InvalidArgument(e) => write!(f, "invalid argument: {e}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// The engine's single wgpu instance, adapter, device and queue.
///
/// Creating and destroying contexts is serialised process-wide (see
/// [`lifecycle_lock`]); using them is not.
pub struct GpuContext {
    /// `None` only while dropping.
    inner: Option<Inner>,
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

impl GpuContext {
    /// A context with no surface (offscreen rendering, tests, CI).
    pub fn new_headless(opts: &GpuOptions) -> Result<GpuContext, GpuError> {
        Self::with_instance(create_instance(opts.backends), opts, None)
    }

    /// A context on an existing instance whose adapter can present to
    /// `compatible_surface` (created from the same instance).
    pub fn with_instance(
        instance: wgpu::Instance,
        opts: &GpuOptions,
        compatible_surface: Option<&wgpu::Surface<'_>>,
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
        let (device, queue) =
            match pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("pin_render"),
                required_limits:
                    wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
                ..Default::default()
            })) {
                Ok(dq) => dq,
                Err(e) => {
                    drop(adapter); // under the lock
                    drop(instance);
                    return Err(GpuError::RequestDevice(e.to_string()));
                }
            };
        Ok(GpuContext {
            inner: Some(Inner {
                queue,
                device,
                adapter,
                instance,
            }),
        })
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

#[cfg(test)]
mod tests;
