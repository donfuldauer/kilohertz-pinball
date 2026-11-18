//! Presenting to one to three window surfaces (PLAN §2.2, D19).
//!
//! Surfaces are created on the main thread from SDL windows
//! ([`create_surface`]) and handed to the render thread inside
//! [`SurfaceSource`]s. [`SurfaceSet`] configures each with the present mode
//! from [`choose_present_mode`](super::choose_present_mode):
//!
//! * the playfield surface is drawn every frame and paces the render loop
//!   (`Fifo`, or `FifoRelaxed` with VRR);
//! * backglass and score surfaces are redrawn only when something changed
//!   (first frame, resize, expose, [`SurfaceSet::request_redraw`]) with
//!   `Mailbox` or `Immediate`;
//! * a secondary surface offered only `Fifo` gets a presenter thread: the
//!   render thread draws its content into an offscreen texture and hands it
//!   over; the presenter thread blocks on the swap chain instead of the render
//!   thread. While a hand-off is in flight, further redraws of that surface
//!   wait (they stay pending), so the render thread never blocks on it.

use super::blit::Blit;
use super::{GpuContext, GpuError, PresentChoice, choose_present_mode};
use crate::debug::{Camera, DebugLine, DebugLines, DebugRenderer};
use crate::topology::{Rect, SurfaceRole, WindowEvent, WindowEventKind};
use arrayvec::ArrayVec;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use wgpu::rwh::{HasDisplayHandle, HasWindowHandle};

/// Creates a surface for `window` on `instance`.
///
/// # Safety
/// The window (and its display connection) must outlive the returned surface.
///
/// How the engine guarantees it (DECISIONS "6.0 — window lifetime", issue
/// #9a): the caller wraps the window, or a token that owns it, in a
/// [`WindowGuard`] and passes a clone as [`SurfaceSource::keep_alive`]. The
/// [`SurfaceSet`] slot keeps that clone until its surface is dropped, and a
/// presenter thread is spawned with [`spawn_guarded`], which drops the
/// surface before the guard. So even a presenter thread that is detached
/// after [`PRESENTER_JOIN_TIMEOUT`] keeps the window alive until it exits.
///
/// What remains the caller's duty:
/// * pass a `keep_alive` guard whose last drop is what destroys the window
///   (with `keep_alive: None` the window must outlive every thread that can
///   hold the surface, which a detached presenter breaks);
/// * destroy the window only when the guard says so: for windows that must
///   be destroyed on one thread (SDL on Cocoa), the guard's value must defer
///   the destruction to that thread rather than destroy it in `Drop`;
/// * keep the display connection (the windowing system) open as long as
///   the window.
pub unsafe fn create_surface<W: HasDisplayHandle + HasWindowHandle>(
    instance: &wgpu::Instance,
    window: &W,
) -> Result<wgpu::Surface<'static>, GpuError> {
    // SAFETY: the caller keeps `window` alive for the surface's lifetime.
    let target = unsafe { wgpu::SurfaceTargetUnsafe::from_display_and_window(window, window) }
        .map_err(|e| GpuError::Surface(format!("window handle: {e}")))?;
    // SAFETY: as above.
    unsafe { instance.create_surface_unsafe(target) }
        .map_err(|e| GpuError::Surface(format!("create_surface: {e}")))
}

/// A shared, type-erased handle that keeps a window alive (issue #9a).
///
/// Clones share one value; the value is dropped when the last clone is.
/// Surfaces created with [`create_surface`] hold a clone (through
/// [`SurfaceSource::keep_alive`]) for as long as they exist, including on a
/// presenter thread started with [`spawn_guarded`], so the window cannot be
/// destroyed under a surface. The value must be `Send + Sync`, because the
/// last clone may be dropped on any thread: windows that may only be
/// destroyed on one thread (SDL windows on the main thread) are wrapped in a
/// token whose `Drop` hands the destruction to that thread.
#[derive(Clone)]
pub struct WindowGuard {
    inner: Arc<dyn Send + Sync>,
}

impl WindowGuard {
    /// Wraps `window` (or a token owning it).
    pub fn new<T: Send + Sync + 'static>(window: T) -> WindowGuard {
        WindowGuard {
            inner: Arc::new(window),
        }
    }

    /// Live clones of this guard (including `self`).
    pub fn strong_count(&self) -> usize {
        Arc::strong_count(&self.inner)
    }
}

impl std::fmt::Debug for WindowGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowGuard")
            .field("strong_count", &self.strong_count())
            .finish()
    }
}

/// Spawns a thread named `name` that runs `body` and drops `guard` only
/// after `body` has returned, so everything `body` owns (a surface) is
/// dropped before the window the guard keeps alive. Used for presenter
/// threads; a detached thread therefore still keeps its window.
///
/// # Errors
/// The OS error when the thread cannot be spawned (`guard` is then dropped
/// on the calling thread).
pub fn spawn_guarded(
    name: &str,
    guard: WindowGuard,
    body: impl FnOnce() + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name(name.to_owned())
        .spawn(move || {
            let guard = guard;
            body();
            drop(guard);
        })
}

/// The planned viewports of a `from`-sized window for the same window at
/// size `to` (both logical). The window's own viewport (the first) scales
/// with the window. A PiP keeps its width fraction and aspect (shrunk to fit
/// a window that became too flat) and the window edges it touched, i.e. its
/// corner anchor; other positions scale with the window. With a zero size on
/// either side the planned rectangles are returned unchanged.
pub(super) fn rescale_viewports(
    planned: &[(SurfaceRole, Rect)],
    from: (u32, u32),
    to: (u32, u32),
) -> ArrayVec<(SurfaceRole, Rect), 3> {
    let scale = |v: u64, num: u32, den: u32| (v * u64::from(num) / u64::from(den.max(1))) as u32;
    let mut out = ArrayVec::new();
    for (i, &(role, r)) in planned.iter().take(3).enumerate() {
        if from == to
            || from.0 == 0
            || from.1 == 0
            || to.0 == 0
            || to.1 == 0
            || r.w == 0
            || r.h == 0
        {
            out.push((role, r));
            continue;
        }
        let (w, h) = if i == 0 {
            (
                scale(u64::from(r.w), to.0, from.0),
                scale(u64::from(r.h), to.1, from.1),
            )
        } else {
            let w = scale(u64::from(r.w), to.0, from.0).min(to.0);
            let h = scale(u64::from(w), r.h, r.w);
            if h > to.1 {
                (scale(u64::from(to.1), r.w, r.h).min(to.0), to.1)
            } else {
                (w, h)
            }
        };
        // Position along one axis: keep a touched window edge, else scale.
        let place = |pos: i32, len: u32, new_len: u32, from: u32, to: u32| -> i32 {
            if pos == 0 {
                0
            } else if i64::from(pos) + i64::from(len) == i64::from(from) {
                (i64::from(to) - i64::from(new_len)) as i32
            } else {
                (i64::from(pos) * i64::from(to) / i64::from(from)) as i32
            }
        };
        out.push((
            role,
            Rect::new(
                place(r.x, r.w, w, from.0, to.0),
                place(r.y, r.h, h, from.1, to.1),
                w,
                h,
            ),
        ));
    }
    out
}

/// One window's surface as handed from the main thread to the render thread.
pub struct SurfaceSource {
    /// Index of the window in `TopologyPlan::windows` (as in [`WindowEvent`]).
    pub window: usize,
    /// Viewports in window-local logical coordinates; the first is the
    /// window's own role, PiPs follow (`WindowPlan::viewports`).
    pub viewports: ArrayVec<(SurfaceRole, Rect), 3>,
    /// The surface (created with [`create_surface`]).
    pub surface: wgpu::Surface<'static>,
    /// Logical window size (the plan's bounds).
    pub logical_size: (u32, u32),
    /// Drawable size in physical pixels.
    pub pixel_size: (u32, u32),
    /// Keeps the window alive while anything (the [`SurfaceSet`] slot or a
    /// presenter thread, even a detached one) still holds this surface; see
    /// the `# Safety` section of [`create_surface`]. `None` leaves the
    /// lifetime entirely to the caller.
    pub keep_alive: Option<WindowGuard>,
}

impl SurfaceSource {
    /// The window's primary role (the playfield if no viewport is listed).
    pub fn role(&self) -> SurfaceRole {
        self.viewports
            .first()
            .map_or(SurfaceRole::Playfield, |(r, _)| *r)
    }
}

/// Surface options.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct SurfaceOptions {
    /// Variable refresh rate requested (playfield uses `FifoRelaxed` when
    /// offered).
    pub vrr: bool,
    /// Always give secondary surfaces a presenter thread (`Fifo`), whatever
    /// they offer. For tests of the presenter path on adapters offering
    /// `Mailbox`.
    pub force_presenter_thread: bool,
}

/// What happened to one surface in [`SurfaceSet::render`].
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SurfaceStatus {
    /// Drawn and presented (or handed to its presenter thread).
    Presented,
    /// Nothing changed (secondary surfaces redraw only on change).
    Unchanged,
    /// The presenter thread is still busy with the previous hand-off; the
    /// redraw stays pending.
    Busy,
    /// The window is minimised or zero-sized.
    Hidden,
    /// The swap chain reported the window as fully covered (macOS checks
    /// the window's occlusion state before every acquire, and a window that
    /// was never shown on screen counts as covered). Nothing was drawn; the
    /// redraw stays pending and the frame is not paced, so the caller sleeps.
    Occluded,
    /// Acquiring the swap-chain image failed (timeout, occluded, outdated,
    /// lost); the surface was reconfigured where that helps and the redraw
    /// stays pending.
    Skipped,
}

/// `true` when some surface with role `Playfield` (roles in source order,
/// paired with `statuses`) was [`SurfaceStatus::Presented`].
pub fn paced_by(roles: impl IntoIterator<Item = SurfaceRole>, statuses: &[SurfaceStatus]) -> bool {
    roles
        .into_iter()
        .zip(statuses)
        .any(|(r, s)| r == SurfaceRole::Playfield && *s == SurfaceStatus::Presented)
}

/// Placeholder background of each role until real content exists (linear
/// RGBA): the playfield is black, backglass dark blue, score dark amber.
pub const fn role_clear_color(role: SurfaceRole) -> [f32; 4] {
    match role {
        SurfaceRole::Playfield => [0.0, 0.0, 0.0, 1.0],
        SurfaceRole::Backglass => [0.02, 0.02, 0.12, 1.0],
        SurfaceRole::Score => [0.12, 0.06, 0.0, 1.0],
    }
}

/// Outline colour of PiP placeholders.
const PIP_OUTLINE: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

struct Presenter {
    tx: Option<SyncSender<PresentJob>>,
    busy: Arc<AtomicBool>,
    /// Set by the presenter thread when it could not present a hand-off.
    retry: Arc<AtomicBool>,
    presented: Arc<AtomicU64>,
    handle: Option<JoinHandle<()>>,
    /// Offscreen source texture (`Rgba8Unorm`), recreated on resize.
    source: Option<wgpu::Texture>,
}

struct PresentJob {
    source: wgpu::Texture,
    width: u32,
    height: u32,
}

/// How long dropping a presenter waits for its thread (which may be inside a
/// blocking `Fifo` acquire or present) before detaching it. A detached
/// presenter keeps its window alive through its [`WindowGuard`] (given as
/// [`SurfaceSource::keep_alive`]) until it exits.
pub const PRESENTER_JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

impl Drop for Presenter {
    fn drop(&mut self) {
        self.tx = None; // ends the presenter loop after the current job
        if let Some(h) = self.handle.take() {
            if join_with_timeout(&h, PRESENTER_JOIN_TIMEOUT) {
                let _ = h.join();
            } else {
                // A stalled swap chain: detach rather than hang shutdown. The
                // thread still owns the surface, and its `WindowGuard` keeps
                // the window alive until it exits (DECISIONS "6.0 — window
                // lifetime", issue #9a).
                eprintln!(
                    "pin_render: presenter thread did not stop within {PRESENTER_JOIN_TIMEOUT:?}; detaching it"
                );
            }
        }
    }
}

/// Waits (polling every millisecond, render side only) until `h` finished or
/// `timeout` passed; `true` when it finished.
fn join_with_timeout<T>(h: &JoinHandle<T>, timeout: std::time::Duration) -> bool {
    let start = std::time::Instant::now();
    while !h.is_finished() {
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    true
}

enum Present {
    /// Presented from the render thread.
    Direct {
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
    },
    /// Presented from a presenter thread (which owns the surface).
    Thread(Presenter),
}

struct Slot {
    window: usize,
    role: SurfaceRole,
    viewports: ArrayVec<(SurfaceRole, Rect), 3>,
    /// Viewports and logical size as planned; resizes rescale from these
    /// (not from the current rectangles, so rounding does not accumulate).
    planned: (ArrayVec<(SurfaceRole, Rect), 3>, (u32, u32)),
    choice: PresentChoice,
    format: wgpu::TextureFormat,
    /// How display layers reach this surface's frames.
    layer: LayerTarget,
    logical_size: (u32, u32),
    pixel_size: (u32, u32),
    minimized: bool,
    dirty: bool,
    /// The last (re)configuration failed; retried before the next draw.
    needs_configure: bool,
    /// Failed configurations of this surface (shared with its presenter).
    configure_log: Arc<ConfigureLog>,
    present: Present,
    /// Renderers for this slot's format (lines, PiP outlines).
    lines: DebugRenderer,
    overlay: DebugRenderer,
    /// Keeps the window alive until `present` (the surface, or the
    /// presenter holding it) is dropped: fields drop in declaration order.
    _keep_alive: Option<WindowGuard>,
}

/// One to three configured surfaces on the render thread.
pub struct SurfaceSet {
    device: wgpu::Device,
    queue: wgpu::Queue,
    slots: Vec<Slot>,
    overlay_lines: DebugLines,
    /// Serialises queue submissions and surface (re)configuration across the
    /// render thread and presenter threads: wgpu's `Surface::configure` waits
    /// for the device to go idle and fails if another thread submits
    /// meanwhile.
    gpu_lock: Arc<Mutex<()>>,
}

/// Failed (re)configurations of one surface (1.4 review N-a): every failed
/// [`try_configure`] call is counted; the first error is logged once.
///
/// It also holds the retry back-off (5.1 review N5): after a failure the next
/// `skip` unforced attempts return `false` at once without touching the
/// device or the submit lock, with `skip` doubling from 1 up to
/// [`CONFIGURE_BACKOFF_MAX`] per consecutive failure and resetting on
/// success. A failing call holds the submit lock for up to
/// `CONFIGURE_ATTEMPTS ×` the bounded device wait, so retrying it before
/// every draw on a persistently busy device would stall every frame. Forced
/// attempts (a new size: initial setup, a pixel-size change, a presenter
/// hand-off of another size) ignore the back-off.
pub(crate) struct ConfigureLog {
    /// Label for the log line (window index and role).
    label: String,
    failures: AtomicU64,
    logged: AtomicBool,
    /// Unforced attempts still to skip.
    skip: AtomicU32,
    /// Skips to apply after the next failure.
    next_skip: AtomicU32,
}

/// Longest run of skipped configuration retries after repeated failures.
pub(crate) const CONFIGURE_BACKOFF_MAX: u32 = 64;

impl ConfigureLog {
    pub(crate) fn new(window: usize, role: SurfaceRole) -> Arc<Self> {
        Arc::new(Self {
            label: format!("window {window} ({role:?})"),
            failures: AtomicU64::new(0),
            logged: AtomicBool::new(false),
            skip: AtomicU32::new(0),
            next_skip: AtomicU32::new(1),
        })
    }

    /// `true` when an unforced attempt may run now; otherwise uses up one
    /// skip of the back-off.
    pub(crate) fn may_attempt(&self) -> bool {
        let left = self.skip.load(Ordering::Acquire);
        if left == 0 {
            return true;
        }
        self.skip.store(left - 1, Ordering::Release);
        false
    }

    /// Resets the back-off after a successful configuration.
    pub(crate) fn succeeded(&self) {
        self.skip.store(0, Ordering::Release);
        self.next_skip.store(1, Ordering::Release);
    }

    /// Starts (or lengthens) the back-off after a failed configuration.
    fn backoff(&self) {
        let n = self.next_skip.load(Ordering::Acquire);
        self.skip.store(n, Ordering::Release);
        self.next_skip
            .store((n * 2).min(CONFIGURE_BACKOFF_MAX), Ordering::Release);
    }

    /// Counts a failed configuration and logs `error` if it is the first.
    pub(crate) fn record(&self, error: &dyn std::fmt::Display) {
        self.failures.fetch_add(1, Ordering::AcqRel);
        self.backoff();
        if !self.logged.swap(true, Ordering::AcqRel) {
            eprintln!(
                "pin_render: surface configuration failed for {} (retried with back-off; logged once): {error}",
                self.label
            );
        }
    }

    /// Failed configurations so far.
    pub(crate) fn failures(&self) -> u64 {
        self.failures.load(Ordering::Acquire)
    }
}

/// Longest wait for the device to go idle between configuration attempts.
const CONFIGURE_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

/// Configures `surface`, serialised with submissions by `gpu_lock`.
///
/// `Surface::configure` first waits for the device to go idle and reports a
/// validation error when the queue is not empty afterwards (it can be, e.g.
/// with a presenter thread presenting on the same device). That error is
/// captured with an error scope instead of reaching the default handler
/// (which panics); the call is retried a few times after a bounded wait for
/// the device. Returns `false` if it still failed — the caller retries later;
/// the failure is counted in `log` and its error logged once per surface.
///
/// While `log`'s back-off is running this returns `false` at once (see
/// [`ConfigureLog`]); [`try_configure_now`] ignores the back-off.
pub(crate) fn try_configure(
    device: &wgpu::Device,
    surface: &wgpu::Surface<'_>,
    config: &wgpu::SurfaceConfiguration,
    gpu_lock: &Mutex<()>,
    log: &ConfigureLog,
) -> bool {
    log.may_attempt() && try_configure_now(device, surface, config, gpu_lock, log)
}

/// [`try_configure`] without the back-off (a new configuration, e.g. a new
/// size, is always attempted).
pub(crate) fn try_configure_now(
    device: &wgpu::Device,
    surface: &wgpu::Surface<'_>,
    config: &wgpu::SurfaceConfiguration,
    gpu_lock: &Mutex<()>,
    log: &ConfigureLog,
) -> bool {
    let _g = lock(gpu_lock);
    let mut last = None;
    for _ in 0..CONFIGURE_ATTEMPTS {
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        surface.configure(device, config);
        match pollster::block_on(scope.pop()) {
            None => {
                log.succeeded();
                return true;
            }
            Some(e) => last = Some(e),
        }
        let _ = super::wait_for_device(device, CONFIGURE_WAIT_TIMEOUT);
    }
    if let Some(e) = last {
        log.record(&e);
    }
    false
}

/// Attempts per [`try_configure`] call.
const CONFIGURE_ATTEMPTS: u32 = 4;

/// Locks the submit/configure lock (a poisoned lock is still usable: it
/// guards no data).
fn lock(m: &Mutex<()>) -> MutexGuard<'_, ()> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn pick_format(caps: &wgpu::SurfaceCapabilities) -> Option<wgpu::TextureFormat> {
    // Prefer a non-sRGB 8-bit format so debug colours land unchanged, like
    // the offscreen path; otherwise the first offered format (the fallback
    // path; usually an `*Srgb` one). On that fallback the debug renderers
    // still draw through the `*Srgb` view, but display layers are composited
    // in display-encoded space through the non-sRGB twin
    // (`LayerTarget::for_target`: a twin view via `view_formats`, or a
    // twin-format scratch texture), exactly as on the preferred path.
    caps.formats
        .iter()
        .copied()
        .find(|f| {
            matches!(
                f,
                wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
            )
        })
        .or_else(|| caps.formats.first().copied())
}

/// How display layers (DMD, LCD, backglass: [`crate::displays`]) are
/// composited onto a target texture of a given format (issue #10).
///
/// The displays blend in display-encoded (sRGB-valued) space, as 2D art
/// tools do. On a non-sRGB target that is what fixed-function blending does
/// already. On an `*Srgb` target the blend unit decodes the destination to
/// linear, blends and re-encodes, so a half-transparent layer would blend in
/// linear space; the layers are therefore drawn through the target's
/// non-sRGB twin format (`Rgba8UnormSrgb` → `Rgba8Unorm`, `Bgra8UnormSrgb`
/// → `Bgra8Unorm`), which stores the same bytes without conversion.
///
/// Build the display renderers for [`layer_format`](Self::layer_format) and
/// draw them through a [`LayerCompositor`] built from this target.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LayerTarget {
    /// Layers are drawn through the target's own view and the renderers are
    /// built for this format. Every non-sRGB target; also an `*Srgb` target
    /// that allows neither a twin view nor copies (then the display shaders
    /// decode their output and blending happens in linear space: the
    /// degraded last resort).
    Direct(wgpu::TextureFormat),
    /// An `*Srgb` target whose texture lists its non-sRGB twin in
    /// `view_formats` (surfaces need
    /// [`wgpu::DownlevelFlags::SURFACE_VIEW_FORMATS`]): layers are drawn
    /// through a twin-format view of the same texture. No extra pass.
    TwinView {
        /// The target (surface) format.
        target: wgpu::TextureFormat,
        /// Its non-sRGB twin, the layer format.
        twin: wgpu::TextureFormat,
    },
    /// An `*Srgb` target without twin views but with `COPY_SRC | COPY_DST`:
    /// the target is copied into a twin-format scratch texture (a raw byte
    /// copy: the formats differ only in sRGB-ness), the layers are drawn
    /// there and the result is copied back.
    Intermediate {
        /// The target (surface) format.
        target: wgpu::TextureFormat,
        /// Its non-sRGB twin, the layer and scratch format.
        twin: wgpu::TextureFormat,
    },
}

impl LayerTarget {
    /// The layer target for a target texture of `format` on a device with
    /// `downlevel` flags whose target texture allows `usages`.
    ///
    /// A non-sRGB format gives [`Direct`](Self::Direct). An `*Srgb` format
    /// gives [`TwinView`](Self::TwinView) when `downlevel` has
    /// [`SURFACE_VIEW_FORMATS`](wgpu::DownlevelFlags::SURFACE_VIEW_FORMATS)
    /// (DX12, Metal, Vulkan with `VK_KHR_swapchain_mutable_format`), else
    /// [`Intermediate`](Self::Intermediate) when `usages` has `COPY_SRC` and
    /// `COPY_DST`, else `Direct` (degraded: linear-space blending).
    pub fn for_target(
        format: wgpu::TextureFormat,
        downlevel: wgpu::DownlevelFlags,
        usages: wgpu::TextureUsages,
    ) -> LayerTarget {
        let twin = format.remove_srgb_suffix();
        if !format.is_srgb() || twin == format {
            return LayerTarget::Direct(format);
        }
        if downlevel.contains(wgpu::DownlevelFlags::SURFACE_VIEW_FORMATS) {
            LayerTarget::TwinView {
                target: format,
                twin,
            }
        } else if usages.contains(wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST) {
            LayerTarget::Intermediate {
                target: format,
                twin,
            }
        } else {
            LayerTarget::Direct(format)
        }
    }

    /// The format of the target texture.
    pub fn target_format(&self) -> wgpu::TextureFormat {
        match *self {
            LayerTarget::Direct(f) => f,
            LayerTarget::TwinView { target, .. } | LayerTarget::Intermediate { target, .. } => {
                target
            }
        }
    }

    /// The format to build the display renderers for (the twin, except for
    /// [`Direct`](Self::Direct)).
    pub fn layer_format(&self) -> wgpu::TextureFormat {
        match *self {
            LayerTarget::Direct(f) => f,
            LayerTarget::TwinView { twin, .. } | LayerTarget::Intermediate { twin, .. } => twin,
        }
    }

    /// The `view_formats` the target texture (or surface configuration)
    /// must list: the twin for [`TwinView`](Self::TwinView), else none.
    pub fn view_formats(&self) -> Vec<wgpu::TextureFormat> {
        match *self {
            LayerTarget::TwinView { twin, .. } => vec![twin],
            LayerTarget::Direct(_) | LayerTarget::Intermediate { .. } => Vec::new(),
        }
    }

    /// Usages the target texture needs on top of `RENDER_ATTACHMENT`:
    /// `COPY_SRC | COPY_DST` for [`Intermediate`](Self::Intermediate), else
    /// none.
    pub fn extra_usages(&self) -> wgpu::TextureUsages {
        match self {
            LayerTarget::Intermediate { .. } => {
                wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST
            }
            LayerTarget::Direct(_) | LayerTarget::TwinView { .. } => wgpu::TextureUsages::empty(),
        }
    }
}

/// Composites display layers onto frames of a [`LayerTarget`].
///
/// Per frame: [`begin`](Self::begin) gives the view to draw the layers
/// into (`None`: the caller's own view of the frame), the layers are
/// recorded, then [`end`](Self::end). On a [`LayerTarget::Direct`] target
/// both calls record and allocate nothing, so the non-sRGB path is
/// unchanged. The [`LayerTarget::Intermediate`] scratch texture is created
/// on first use and again only when the frame size changes.
#[derive(Debug)]
pub struct LayerCompositor {
    target: LayerTarget,
    scratch: Option<wgpu::Texture>,
}

impl LayerCompositor {
    /// A compositor for frames of `target`.
    pub fn new(target: LayerTarget) -> LayerCompositor {
        LayerCompositor {
            target,
            scratch: None,
        }
    }

    /// The layer target.
    pub fn target(&self) -> LayerTarget {
        self.target
    }

    /// Prepares `frame` (a texture of the target format, created with the
    /// target's [`view_formats`](LayerTarget::view_formats) and
    /// [`extra_usages`](LayerTarget::extra_usages)) for the layers and
    /// returns the view to draw them into, of format
    /// [`layer_format`](LayerTarget::layer_format); `None` means "draw
    /// through your own view of `frame`" ([`LayerTarget::Direct`], or a
    /// frame whose format is not this target's). Content already recorded
    /// into `frame` is kept under the layers.
    pub fn begin(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::Texture,
    ) -> Option<wgpu::TextureView> {
        if frame.format() != self.target.target_format() {
            return None;
        }
        match self.target {
            LayerTarget::Direct(_) => None,
            LayerTarget::TwinView { twin, .. } => {
                Some(frame.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("display layers (twin view)"),
                    format: Some(twin),
                    ..Default::default()
                }))
            }
            LayerTarget::Intermediate { twin, .. } => {
                let size = frame.size();
                let stale = self.scratch.as_ref().is_none_or(|t| t.size() != size);
                if stale {
                    self.scratch = Some(device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("display layers (scratch)"),
                        size,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: twin,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::COPY_SRC
                            | wgpu::TextureUsages::COPY_DST,
                        view_formats: &[],
                    }));
                }
                let scratch = self.scratch.as_ref()?;
                encoder.copy_texture_to_texture(
                    frame.as_image_copy(),
                    scratch.as_image_copy(),
                    size,
                );
                Some(scratch.create_view(&wgpu::TextureViewDescriptor::default()))
            }
        }
    }

    /// Finishes the layers of `frame` (the texture given to
    /// [`begin`](Self::begin)): copies the scratch texture back for
    /// [`LayerTarget::Intermediate`]; nothing otherwise.
    pub fn end(&self, encoder: &mut wgpu::CommandEncoder, frame: &wgpu::Texture) {
        if frame.format() != self.target.target_format() {
            return;
        }
        if let (LayerTarget::Intermediate { .. }, Some(scratch)) = (self.target, &self.scratch)
            && scratch.size() == frame.size()
        {
            encoder.copy_texture_to_texture(
                scratch.as_image_copy(),
                frame.as_image_copy(),
                frame.size(),
            );
        }
    }
}

fn surface_config(
    format: wgpu::TextureFormat,
    caps: &wgpu::SurfaceCapabilities,
    (w, h): (u32, u32),
    mode: wgpu::PresentMode,
    layer: LayerTarget,
) -> wgpu::SurfaceConfiguration {
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | layer.extra_usages(),
        format,
        color_space: Default::default(),
        width: w.max(1),
        height: h.max(1),
        present_mode: mode,
        desired_maximum_frame_latency: 2,
        alpha_mode: caps
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto),
        view_formats: layer.view_formats(),
    }
}

fn rgba(c: [f32; 4]) -> wgpu::Color {
    wgpu::Color {
        r: f64::from(c[0]),
        g: f64::from(c[1]),
        b: f64::from(c[2]),
        a: f64::from(c[3]),
    }
}

impl SurfaceSet {
    /// Configures every source on `ctx`'s device. Fails if a surface is not
    /// supported by the adapter.
    pub fn new(
        ctx: &GpuContext,
        sources: Vec<SurfaceSource>,
        opts: SurfaceOptions,
    ) -> Result<SurfaceSet, GpuError> {
        let device = ctx.device().clone();
        let queue = ctx.queue().clone();
        let gpu_lock = Arc::new(Mutex::new(()));
        let mut slots = Vec::with_capacity(sources.len());
        for src in sources {
            let role = src.role();
            let caps = src.surface.get_capabilities(ctx.adapter());
            let format = pick_format(&caps).ok_or_else(|| {
                GpuError::Surface(format!(
                    "window {} ({role:?}): surface not supported by the adapter",
                    src.window
                ))
            })?;
            let mut choice = choose_present_mode(role, &caps.present_modes, opts.vrr);
            if opts.force_presenter_thread && role != SurfaceRole::Playfield {
                choice = PresentChoice::PresenterThread(wgpu::PresentMode::Fifo);
            }
            // Display layers on a direct surface follow the surface format
            // (a twin view or scratch texture on the `*Srgb` fallback); a
            // presenter-thread surface is drawn into an `Rgba8Unorm`
            // offscreen texture, so its layers are direct.
            let layer = match choice {
                PresentChoice::Direct(_) => LayerTarget::for_target(
                    format,
                    ctx.adapter().get_downlevel_capabilities().flags,
                    caps.usages,
                ),
                PresentChoice::PresenterThread(_) => {
                    LayerTarget::Direct(wgpu::TextureFormat::Rgba8Unorm)
                }
            };
            let config = surface_config(format, &caps, src.pixel_size, choice.mode(), layer);
            let configure_log = ConfigureLog::new(src.window, role);
            if !try_configure_now(&device, &src.surface, &config, &gpu_lock, &configure_log) {
                return Err(GpuError::Surface(format!(
                    "window {} ({role:?}): surface configuration failed",
                    src.window
                )));
            }
            let (present, draw_format) = match choice {
                PresentChoice::Direct(_) => (
                    Present::Direct {
                        surface: src.surface,
                        config,
                    },
                    format,
                ),
                PresentChoice::PresenterThread(_) => (
                    Present::Thread(spawn_presenter(
                        &device,
                        &queue,
                        GuardedSurface {
                            surface: src.surface,
                            keep_alive: src.keep_alive.clone(),
                        },
                        config,
                        src.window,
                        Arc::clone(&gpu_lock),
                        Arc::clone(&configure_log),
                    )?),
                    wgpu::TextureFormat::Rgba8Unorm,
                ),
            };
            slots.push(Slot {
                window: src.window,
                role,
                planned: (src.viewports.clone(), src.logical_size),
                viewports: src.viewports,
                choice,
                format: draw_format,
                layer,
                logical_size: src.logical_size,
                pixel_size: src.pixel_size,
                minimized: false,
                dirty: true,
                needs_configure: false,
                configure_log,
                present,
                lines: DebugRenderer::new(&device, draw_format),
                overlay: DebugRenderer::new(&device, draw_format),
                _keep_alive: src.keep_alive,
            });
        }
        Ok(SurfaceSet {
            device,
            queue,
            slots,
            overlay_lines: DebugLines::new(64),
            gpu_lock,
        })
    }

    /// Number of surfaces.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// `true` when there are no surfaces.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// The present choice of each surface, in source order, with its window
    /// index and role.
    pub fn choices(&self) -> impl Iterator<Item = (usize, SurfaceRole, PresentChoice)> + '_ {
        self.slots.iter().map(|s| (s.window, s.role, s.choice))
    }

    /// `true` when `statuses` (the result of the last [`render`](Self::render))
    /// shows a presented playfield surface: its blocking `Fifo` acquire paced
    /// the frame, so the render loop needs no own sleep. When the playfield
    /// could not present (occluded, timeout, outdated, hidden, zero-sized)
    /// the caller must sleep instead of spinning.
    pub fn frame_paced(&self, statuses: &[SurfaceStatus]) -> bool {
        paced_by(self.slots.iter().map(|s| s.role), statuses)
    }

    /// Frames handed to presenter threads and presented by them, in total.
    pub fn presenter_thread_frames(&self) -> u64 {
        self.slots
            .iter()
            .filter_map(|s| match &s.present {
                Present::Thread(p) => Some(p.presented.load(Ordering::Acquire)),
                Present::Direct { .. } => None,
            })
            .sum()
    }

    /// Failed surface (re)configurations so far, over all surfaces
    /// (including presenter threads). A surface that cannot be configured is
    /// retried before later draws with an exponential back-off (1, 2, 4, …
    /// up to 64 skipped draws between attempts); each failed attempt is
    /// counted here and the first error of each surface is logged once (to
    /// stderr).
    pub fn configure_failures(&self) -> u64 {
        self.slots.iter().map(|s| s.configure_log.failures()).sum()
    }

    /// [`configure_failures`](Self::configure_failures) per surface, in
    /// source order, with its window index.
    pub fn configure_failures_by_surface(&self) -> impl Iterator<Item = (usize, u64)> + '_ {
        self.slots
            .iter()
            .map(|s| (s.window, s.configure_log.failures()))
    }

    /// Marks every secondary surface showing `role` for redraw.
    pub fn request_redraw(&mut self, role: SurfaceRole) {
        for s in &mut self.slots {
            if s.role == role {
                s.dirty = true;
            }
        }
    }

    /// Applies a window event from the main thread.
    pub fn handle_window_event(&mut self, ev: &WindowEvent) {
        let Some(s) = self.slots.iter_mut().find(|s| s.window == ev.window) else {
            return;
        };
        match ev.kind {
            WindowEventKind::PixelSizeChanged { w, h } => {
                s.pixel_size = (w, h);
                if let Present::Direct { surface, config } = &mut s.present
                    && w > 0
                    && h > 0
                {
                    config.width = w;
                    config.height = h;
                    let ok = try_configure_now(
                        &self.device,
                        surface,
                        config,
                        &self.gpu_lock,
                        &s.configure_log,
                    );
                    s.needs_configure = !ok;
                }
                s.dirty = true;
            }
            WindowEventKind::Resized { w, h } => {
                s.logical_size = (w, h);
                s.viewports = rescale_viewports(&s.planned.0, s.planned.1, (w, h));
                s.dirty = true;
            }
            WindowEventKind::Minimized | WindowEventKind::Hidden => s.minimized = true,
            WindowEventKind::Restored
            | WindowEventKind::Shown
            | WindowEventKind::Maximized
            | WindowEventKind::Exposed => {
                s.minimized = false;
                s.dirty = true;
            }
            _ => {}
        }
    }

    /// Draws one frame: the playfield surface(s) every call with `lines`
    /// seen through `camera`; secondary surfaces only when marked dirty. PiP
    /// viewports get a placeholder outline. Returns each surface's status in
    /// source order.
    pub fn render(&mut self, lines: &DebugLines, camera: &Camera) -> ArrayVec<SurfaceStatus, 3> {
        let mut out = ArrayVec::new();
        for i in 0..self.slots.len() {
            let st = self.render_slot(i, lines, camera);
            if !out.is_full() {
                out.push(st);
            }
        }
        out
    }

    fn pip_overlay(&mut self, i: usize) {
        let s = &self.slots[i];
        self.overlay_lines.clear();
        let (lw, lh) = s.logical_size;
        let (pw, ph) = s.pixel_size;
        let sx = if lw > 0 { pw as f32 / lw as f32 } else { 1.0 };
        let sy = if lh > 0 { ph as f32 / lh as f32 } else { 1.0 };
        for (_, r) in s.viewports.iter().skip(1) {
            // Pixel-centre outline in physical pixels (y down).
            let x0 = r.x as f32 * sx + 0.5;
            let y0 = r.y as f32 * sy + 0.5;
            let x1 = (r.x as f32 + r.w as f32) * sx - 0.5;
            let y1 = (r.y as f32 + r.h as f32) * sy - 0.5;
            let pts = [[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]];
            for w in pts.windows(2) {
                self.overlay_lines.push(DebugLine::new(
                    [w[0][0], w[0][1], 0.0],
                    [w[1][0], w[1][1], 0.0],
                    PIP_OUTLINE,
                ));
            }
        }
    }

    fn render_slot(&mut self, i: usize, lines: &DebugLines, camera: &Camera) -> SurfaceStatus {
        {
            let s = &mut self.slots[i];
            if let Present::Thread(p) = &s.present
                && p.retry.swap(false, Ordering::AcqRel)
            {
                s.dirty = true;
            }
            if s.minimized || s.pixel_size.0 == 0 || s.pixel_size.1 == 0 {
                return SurfaceStatus::Hidden;
            }
            if s.role != SurfaceRole::Playfield && !s.dirty {
                return SurfaceStatus::Unchanged;
            }
            if let Present::Thread(p) = &s.present
                && p.busy.load(Ordering::Acquire)
            {
                return SurfaceStatus::Busy;
            }
        }
        self.pip_overlay(i);
        let device = self.device.clone();
        let queue = self.queue.clone();
        let gpu_lock = Arc::clone(&self.gpu_lock);
        let overlay_lines = &self.overlay_lines;
        let s = &mut self.slots[i];
        let (pw, ph) = s.pixel_size;
        let pixel_camera = Camera::ortho_top_down([0.0, ph as f32], [pw as f32, 0.0]);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("surface frame"),
        });
        let clear = role_clear_color(s.role);
        let draw = |s: &mut Slot, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView| {
            if s.role == SurfaceRole::Playfield {
                s.lines
                    .draw_with(encoder, &queue, view, camera, lines, Some(clear), None);
            } else {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("secondary clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(rgba(clear)),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            }
            if !overlay_lines.is_empty() {
                s.overlay
                    .draw(encoder, &queue, view, &pixel_camera, overlay_lines);
            }
        };
        if matches!(s.present, Present::Direct { .. }) {
            if s.needs_configure {
                let Present::Direct { surface, config } = &s.present else {
                    unreachable!()
                };
                if !try_configure(&device, surface, config, &gpu_lock, &s.configure_log) {
                    return SurfaceStatus::Skipped;
                }
                s.needs_configure = false;
            }
            let Present::Direct { surface, config } = &s.present else {
                unreachable!()
            };
            let (frame, suboptimal) = match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(t) => (t, false),
                wgpu::CurrentSurfaceTexture::Suboptimal(t) => (t, true),
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    let ok = try_configure(&device, surface, config, &gpu_lock, &s.configure_log);
                    s.needs_configure = !ok;
                    return SurfaceStatus::Skipped;
                }
                wgpu::CurrentSurfaceTexture::Occluded => return SurfaceStatus::Occluded,
                _ => return SurfaceStatus::Skipped,
            };
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            draw(s, &mut encoder, &view);
            {
                let _g = lock(&gpu_lock);
                queue.submit([encoder.finish()]);
            }
            queue.present(frame);
            if suboptimal && let Present::Direct { surface, config } = &s.present {
                let ok = try_configure(&device, surface, config, &gpu_lock, &s.configure_log);
                s.needs_configure = !ok;
            }
        } else {
            let source = {
                let Present::Thread(p) = &mut s.present else {
                    unreachable!()
                };
                let stale = p
                    .source
                    .as_ref()
                    .is_none_or(|t| (t.width(), t.height()) != (pw, ph));
                if stale {
                    p.source = Some(device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("presenter source"),
                        size: wgpu::Extent3d {
                            width: pw,
                            height: ph,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    }));
                }
                p.source.clone().expect("source texture")
            };
            let view = source.create_view(&wgpu::TextureViewDescriptor::default());
            draw(s, &mut encoder, &view);
            {
                let _g = lock(&gpu_lock);
                queue.submit([encoder.finish()]);
            }
            let Present::Thread(p) = &mut s.present else {
                unreachable!()
            };
            p.busy.store(true, Ordering::Release);
            let job = PresentJob {
                source,
                width: pw,
                height: ph,
            };
            match p.tx.as_ref().map(|tx| tx.try_send(job)) {
                Some(Ok(())) => {}
                Some(Err(TrySendError::Full(_))) => return SurfaceStatus::Busy,
                Some(Err(TrySendError::Disconnected(_))) | None => {
                    p.busy.store(false, Ordering::Release);
                    return SurfaceStatus::Skipped;
                }
            }
        }
        s.dirty = false;
        SurfaceStatus::Presented
    }

    /// The draw format of each surface (the swap-chain format for direct
    /// surfaces, the offscreen format for presenter-thread surfaces).
    pub fn formats(&self) -> impl Iterator<Item = wgpu::TextureFormat> + '_ {
        self.slots.iter().map(|s| s.format)
    }

    /// The display-layer target of each surface, in source order: its
    /// surface is configured with the target's
    /// [`view_formats`](LayerTarget::view_formats) and
    /// [`extra_usages`](LayerTarget::extra_usages); display renderers for
    /// it are built for [`layer_format`](LayerTarget::layer_format).
    pub fn layer_targets(&self) -> impl Iterator<Item = LayerTarget> + '_ {
        self.slots.iter().map(|s| s.layer)
    }
}

/// A surface handed to a presenter thread with the guard of its window.
struct GuardedSurface {
    surface: wgpu::Surface<'static>,
    keep_alive: Option<WindowGuard>,
}

fn spawn_presenter(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: GuardedSurface,
    mut config: wgpu::SurfaceConfiguration,
    window: usize,
    gpu_lock: Arc<Mutex<()>>,
    configure_log: Arc<ConfigureLog>,
) -> Result<Presenter, GpuError> {
    let GuardedSurface {
        surface,
        keep_alive,
    } = target;
    let (tx, rx) = sync_channel::<PresentJob>(1);
    let busy = Arc::new(AtomicBool::new(false));
    let presented = Arc::new(AtomicU64::new(0));
    let device = device.clone();
    let queue = queue.clone();
    let retry = Arc::new(AtomicBool::new(false));
    let retry2 = Arc::clone(&retry);
    let busy2 = Arc::clone(&busy);
    let presented2 = Arc::clone(&presented);
    // The thread holds the window guard (if any) and drops it only after the
    // closure below, which owns the surface, has returned (issue #9a).
    let guard = keep_alive.unwrap_or_else(|| WindowGuard::new(()));
    let handle = spawn_guarded(&format!("presenter-{window}"), guard, move || {
        let blit = Blit::new(&device, config.format);
        let mut configured = true;
        for job in rx {
            if (config.width, config.height) != (job.width.max(1), job.height.max(1)) {
                config.width = job.width.max(1);
                config.height = job.height.max(1);
                configured =
                    try_configure_now(&device, &surface, &config, &gpu_lock, &configure_log);
            } else if !configured {
                configured = try_configure(&device, &surface, &config, &gpu_lock, &configure_log);
            }
            let frame = if configured {
                match surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(t)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Some(t),
                    wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                        configured =
                            try_configure(&device, &surface, &config, &gpu_lock, &configure_log);
                        None
                    }
                    _ => None,
                }
            } else {
                None
            };
            if let Some(frame) = frame {
                let dst = frame
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let src = job
                    .source
                    .create_view(&wgpu::TextureViewDescriptor::default());
                let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("presenter blit"),
                });
                blit.encode(&device, &mut encoder, &src, &dst);
                {
                    let _g = lock(&gpu_lock);
                    queue.submit([encoder.finish()]);
                }
                queue.present(frame);
                presented2.fetch_add(1, Ordering::AcqRel);
            } else {
                retry2.store(true, Ordering::Release);
            }
            busy2.store(false, Ordering::Release);
        }
    })
    .map_err(|e| GpuError::Surface(format!("presenter thread: {e}")))?;
    Ok(Presenter {
        tx: Some(tx),
        busy,
        retry,
        presented,
        handle: Some(handle),
        source: None,
    })
}

#[cfg(test)]
mod configure_log_tests {
    use super::*;

    #[test]
    fn failures_are_counted_and_logged_once() {
        let log = ConfigureLog::new(2, SurfaceRole::Backglass);
        assert_eq!(log.failures(), 0);
        assert!(log.label.contains("window 2") && log.label.contains("Backglass"));
        log.record(&"first");
        log.record(&"second");
        log.record(&"third");
        assert_eq!(log.failures(), 3);
        assert!(log.logged.load(Ordering::Acquire));
    }

    /// 5.1 review N5: after each failure the unforced retries back off
    /// 1, 2, 4, … up to CONFIGURE_BACKOFF_MAX skipped attempts; a success
    /// resets it.
    #[test]
    fn retries_back_off_exponentially_and_reset_on_success() {
        let log = ConfigureLog::new(0, SurfaceRole::Playfield);
        assert!(log.may_attempt(), "no back-off before a failure");
        let mut skips = Vec::new();
        for _ in 0..9 {
            log.record(&"busy");
            let mut n = 0;
            while !log.may_attempt() {
                n += 1;
            }
            skips.push(n);
        }
        assert_eq!(skips, [1, 2, 4, 8, 16, 32, 64, 64, 64]);
        assert_eq!(log.failures(), 9, "skipped attempts are not failures");
        log.record(&"busy");
        log.succeeded();
        assert!(log.may_attempt(), "success clears the pending skips");
        log.record(&"busy");
        assert!(
            !log.may_attempt() && log.may_attempt(),
            "back-off restarts at 1"
        );
    }
}

#[cfg(test)]
mod join_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn bounded_join_finishes_or_times_out() {
        let quick = std::thread::spawn(|| ());
        assert!(join_with_timeout(&quick, Duration::from_secs(10)));
        quick.join().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let stuck = std::thread::spawn(move || {
            let _ = rx.recv();
        });
        assert!(!join_with_timeout(&stuck, Duration::from_millis(5)));
        drop(tx);
        stuck.join().unwrap();
    }
}

#[cfg(test)]
mod layer_target_tests {
    use super::*;
    use wgpu::DownlevelFlags as D;
    use wgpu::TextureFormat as F;
    use wgpu::TextureUsages as U;

    const COPY: U = U::COPY_SRC.union(U::COPY_DST);

    #[test]
    fn non_srgb_targets_are_direct_whatever_the_device() {
        for f in [
            F::Rgba8Unorm,
            F::Bgra8Unorm,
            F::Rgba16Float,
            F::Rgb10a2Unorm,
        ] {
            for d in [D::empty(), D::all()] {
                for u in [U::RENDER_ATTACHMENT, U::RENDER_ATTACHMENT | COPY] {
                    let t = LayerTarget::for_target(f, d, u);
                    assert_eq!(t, LayerTarget::Direct(f));
                    assert_eq!((t.target_format(), t.layer_format()), (f, f));
                    assert!(t.view_formats().is_empty());
                    assert!(t.extra_usages().is_empty());
                }
            }
        }
    }

    #[test]
    fn srgb_targets_draw_layers_in_the_twin_format() {
        for (f, twin) in [
            (F::Rgba8UnormSrgb, F::Rgba8Unorm),
            (F::Bgra8UnormSrgb, F::Bgra8Unorm),
        ] {
            let all = D::SURFACE_VIEW_FORMATS | D::VIEW_FORMATS;
            for u in [U::RENDER_ATTACHMENT, U::RENDER_ATTACHMENT | COPY] {
                let t = LayerTarget::for_target(f, all, u);
                assert_eq!(t, LayerTarget::TwinView { target: f, twin });
                assert_eq!((t.target_format(), t.layer_format()), (f, twin));
                assert_eq!(t.view_formats(), vec![twin]);
                assert!(t.extra_usages().is_empty());
                assert!(!t.layer_format().is_srgb());
            }
            let t = LayerTarget::for_target(f, D::VIEW_FORMATS, U::RENDER_ATTACHMENT | COPY);
            assert_eq!(t, LayerTarget::Intermediate { target: f, twin });
            assert_eq!((t.target_format(), t.layer_format()), (f, twin));
            assert!(t.view_formats().is_empty());
            assert_eq!(t.extra_usages(), COPY);
            // Neither a twin view nor both copy usages: the degraded direct
            // path.
            for u in [
                U::RENDER_ATTACHMENT,
                U::RENDER_ATTACHMENT | U::COPY_DST,
                U::RENDER_ATTACHMENT | U::COPY_SRC,
            ] {
                assert_eq!(
                    LayerTarget::for_target(f, D::VIEW_FORMATS, u),
                    LayerTarget::Direct(f)
                );
            }
        }
    }

    #[test]
    fn surface_config_lists_the_twin_view_and_copy_usages() {
        let caps = wgpu::SurfaceCapabilities {
            formats: vec![F::Bgra8UnormSrgb],
            present_modes: vec![wgpu::PresentMode::Fifo],
            alpha_modes: vec![wgpu::CompositeAlphaMode::Opaque],
            usages: U::RENDER_ATTACHMENT | COPY,
            ..Default::default()
        };
        let f = pick_format(&caps).unwrap();
        assert_eq!(f, F::Bgra8UnormSrgb, "the fallback: first offered");
        let mode = wgpu::PresentMode::Fifo;
        let twin = LayerTarget::for_target(f, D::SURFACE_VIEW_FORMATS, caps.usages);
        let c = surface_config(f, &caps, (8, 4), mode, twin);
        assert_eq!((c.format, c.usage), (f, U::RENDER_ATTACHMENT));
        assert_eq!(c.view_formats, vec![F::Bgra8Unorm]);
        let inter = LayerTarget::for_target(f, D::empty(), caps.usages);
        let c = surface_config(f, &caps, (8, 4), mode, inter);
        assert_eq!(c.usage, U::RENDER_ATTACHMENT | COPY);
        assert!(c.view_formats.is_empty());
        // The preferred (non-sRGB) path is configured exactly as before.
        let caps = wgpu::SurfaceCapabilities {
            formats: vec![F::Bgra8UnormSrgb, F::Bgra8Unorm],
            ..caps
        };
        let f = pick_format(&caps).unwrap();
        assert_eq!(f, F::Bgra8Unorm);
        let direct = LayerTarget::for_target(f, D::all(), caps.usages);
        let c = surface_config(f, &caps, (8, 4), mode, direct);
        assert_eq!(
            (c.usage, c.view_formats),
            (U::RENDER_ATTACHMENT, Vec::new())
        );
    }
}
