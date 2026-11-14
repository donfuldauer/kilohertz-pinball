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
/// The window (and its display connection) must outlive the returned surface:
/// drop the [`SurfaceSet`] holding it (i.e. join the render thread) before
/// destroying the window.
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
/// blocking `Fifo` acquire or present) before detaching it.
pub const PRESENTER_JOIN_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

impl Drop for Presenter {
    fn drop(&mut self) {
        self.tx = None; // ends the presenter loop after the current job
        if let Some(h) = self.handle.take() {
            if join_with_timeout(&h, PRESENTER_JOIN_TIMEOUT) {
                let _ = h.join();
            } else {
                // A stalled swap chain: detach rather than hang shutdown. The
                // thread still owns the surface (see DECISIONS "1.4 — fix
                // round 1": the window may then be destroyed under it).
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
    choice: PresentChoice,
    format: wgpu::TextureFormat,
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
    // the offscreen path; otherwise the first offered format.
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

fn surface_config(
    format: wgpu::TextureFormat,
    caps: &wgpu::SurfaceCapabilities,
    (w, h): (u32, u32),
    mode: wgpu::PresentMode,
) -> wgpu::SurfaceConfiguration {
    wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
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
        view_formats: Vec::new(),
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
            let config = surface_config(format, &caps, src.pixel_size, choice.mode());
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
                        src.surface,
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
                viewports: src.viewports,
                choice,
                format: draw_format,
                logical_size: src.logical_size,
                pixel_size: src.pixel_size,
                minimized: false,
                dirty: true,
                needs_configure: false,
                configure_log,
                present,
                lines: DebugRenderer::new(&device, draw_format),
                overlay: DebugRenderer::new(&device, draw_format),
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
}

fn spawn_presenter(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    surface: wgpu::Surface<'static>,
    mut config: wgpu::SurfaceConfiguration,
    window: usize,
    gpu_lock: Arc<Mutex<()>>,
    configure_log: Arc<ConfigureLog>,
) -> Result<Presenter, GpuError> {
    let (tx, rx) = sync_channel::<PresentJob>(1);
    let busy = Arc::new(AtomicBool::new(false));
    let presented = Arc::new(AtomicU64::new(0));
    let device = device.clone();
    let queue = queue.clone();
    let retry = Arc::new(AtomicBool::new(false));
    let retry2 = Arc::clone(&retry);
    let busy2 = Arc::clone(&busy);
    let presented2 = Arc::clone(&presented);
    let handle = std::thread::Builder::new()
        .name(format!("presenter-{window}"))
        .spawn(move || {
            let blit = Blit::new(&device, config.format);
            let mut configured = true;
            for job in rx {
                if (config.width, config.height) != (job.width.max(1), job.height.max(1)) {
                    config.width = job.width.max(1);
                    config.height = job.height.max(1);
                    configured =
                        try_configure_now(&device, &surface, &config, &gpu_lock, &configure_log);
                } else if !configured {
                    configured =
                        try_configure(&device, &surface, &config, &gpu_lock, &configure_log);
                }
                let frame = if configured {
                    match surface.get_current_texture() {
                        wgpu::CurrentSurfaceTexture::Success(t)
                        | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Some(t),
                        wgpu::CurrentSurfaceTexture::Outdated
                        | wgpu::CurrentSurfaceTexture::Lost => {
                            configured = try_configure(
                                &device,
                                &surface,
                                &config,
                                &gpu_lock,
                                &configure_log,
                            );
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
                    let mut encoder =
                        device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
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
