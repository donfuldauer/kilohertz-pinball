//! Presentation of the table frame from the render thread (Subphase 6.0
//! wiring).
//!
//! The 1.4 [`SurfaceSet`](pin_render::gpu::SurfaceSet) only draws debug
//! lines; its only hooks (w8g-d,
//! [`SurfaceFrameHooks`](pin_render::gpu::SurfaceFrameHooks) through
//! `SurfaceSet::render_with`) time its frames' submissions and paint an
//! overlay over the playfield, which is how the debug view gets the GPU
//! frame timer and the profiler HUD ([`DebugViewHooks`](super::DebugViewHooks)).
//! It cannot draw the table. So when a table is loaded, the render thread
//! draws its own frames here: each window's surface gets a
//! [`TableRenderer`] that draws the PBR playfield and the displays routed
//! to that window.
//!
//! The choice is made **per surface** ([`plan_surface`] and
//! [`slot_decision`], both pure and GPU-free):
//!
//! * [`SurfacePlan::Direct`]: drawn and presented by the render thread on
//!   every render-loop iteration. This is the playfield (always; its `Fifo`
//!   or, with VRR, `FifoRelaxed` acquire paces the loop by design, D19) and
//!   a backglass / score window whose chosen mode is non-blocking
//!   (`Mailbox`, `Immediate`; see [`is_blocking_mode`]).
//! * [`SurfacePlan::Presenter`]: a backglass / score window whose chosen
//!   mode is blocking (it offers only `Fifo` / `FifoRelaxed`), or any
//!   backglass / score window with `SurfaceOptions::force_presenter_thread`.
//!   D19: "a separate presenter if only FIFO is available". The render
//!   thread still draws the window's frame (playfield-free: a clear, then
//!   every display routed to it) into an offscreen texture and hands it to
//!   a presenter thread that owns the `wgpu::Surface` through a
//!   latest-frame mailbox (`handoff`, `presenter` modules): the render
//!   thread never acquires or presents that surface, so a vsync-bound
//!   secondary acquire can neither stall the playfield nor block the
//!   render-thread join (DECISIONS "6.0 — secondary Fifo-only surfaces,
//!   round 4").
//! * [`SurfacePlan::Unsupported`]: no 8-bit colour format. A secondary
//!   surface planned so is skipped (one stderr note); only an unsupported
//!   **playfield** rejects the set ([`table_set_accepted`]), and the render
//!   thread then falls back to the `SurfaceSet` debug view (DECISIONS
//!   "6.0 — wiring completed"), which is profiled like the table (GPU frame
//!   time, `--profiler-hud`) through [`DebugViewHooks`](super::DebugViewHooks)
//!   (w8g-d).
//!
//! Display layers (score display, backglass) blend in display-encoded
//! space on every surface (issue #10). Each slot gets a
//! [`LayerTarget`] at configure time ([`slot_layers`]): a direct `*Srgb`
//! (fallback) surface lists its non-sRGB twin in `view_formats` or gets
//! `COPY_SRC | COPY_DST`, its display renderers are built for the twin and
//! drawn through a [`LayerCompositor`]; a presenter-thread surface is drawn
//! into an offscreen texture of the twin format ([`offscreen_format`]),
//! which the presenter copies (or blits, decoding first) into the `*Srgb`
//! image byte for byte. Non-sRGB surfaces are unchanged.
//!
//! A presenter-thread or skipped secondary surface never turns the cabinet
//! into the debug view, so the table renderer and the sim's display
//! commands (scenes, scores) stay on whenever the playfield can be drawn;
//! display commands reach the renderer of every retained surface, direct
//! or presenter-thread.
//!
//! The in-game menu (issue #84, REQ-VIEW-006) is painted last into the
//! playfield slot's frame, over the table, the displays and the HUD, before
//! the frame is submitted and presented ([`AppSurfaces::set_menu_overlay`]);
//! while the menu is closed no pass is recorded and frames are unchanged.

use super::gpu_timer::GpuFrameTimer;
use super::handoff::{HANDOFF_SLOTS, Handoff, HandoffStats, PresenterThread, TakeFree};
use super::presenter::{
    CopyPath, PresenterTarget, configure_surface, lock_gpu, offscreen_frame_texture, run_presenter,
};
use super::table::DisplayCommand;
use super::view::{TableRenderOptions, TableRenderer, TableVisuals, routes_for_window};
use crate::ui::{MenuOverlay, MenuState};
use pin_core::snapshot::FrameStateSnapshot;
use pin_render::displays::DisplayRoute;
use pin_render::gpu::{
    GpuContext, LayerCompositor, LayerTarget, SurfaceOptions, SurfaceSource, SurfaceStatus,
    WindowGuard, choose_present_mode, frame_latency_for, lean_quality_for, wgpu,
};
use pin_render::topology::{Rect, SurfaceRole, WindowEvent, WindowEventKind};
use std::sync::{Arc, Mutex};

/// The 8-bit colour format of a surface the table renderer can draw.
fn pick_format(formats: &[wgpu::TextureFormat]) -> Option<wgpu::TextureFormat> {
    use wgpu::TextureFormat as F;
    formats
        .iter()
        .copied()
        .find(|f| matches!(f, F::Bgra8Unorm | F::Rgba8Unorm))
        .or_else(|| {
            formats
                .iter()
                .copied()
                .find(|f| matches!(f, F::Bgra8UnormSrgb | F::Rgba8UnormSrgb))
        })
}

/// The format the render thread draws a presenter-thread surface's frames
/// in (the offscreen pool textures): the surface `format` without its sRGB
/// suffix, so the frame's display layers blend in display-encoded space
/// with no compositing step (issue #10). The presenter's copy into the
/// surface moves the bytes unchanged (`copy_texture_to_texture` allows
/// formats that differ only in sRGB-ness; the blit decodes first,
/// [`FrameCopy::with_source`](super::presenter::FrameCopy::with_source)).
/// A non-sRGB `format` is returned unchanged.
pub(crate) fn offscreen_format(format: wgpu::TextureFormat) -> wgpu::TextureFormat {
    format.remove_srgb_suffix()
}

/// How one slot's frames are drawn, from its surface `format`, whether it
/// is `threaded` (presenter thread), its presenter copy `path`, the
/// adapter's `downlevel` flags and the surface's allowed `usages` (pure).
///
/// Returns `(draw_format, layers, usage)`: the format of the texture the
/// render thread draws into (the surface for a direct slot,
/// [`offscreen_format`] for a threaded one), the [`LayerTarget`] of that
/// texture (`LayerTarget::for_target(draw_format, downlevel, usages)`;
/// always [`LayerTarget::Direct`] for a threaded slot, whose offscreen
/// format is never `*Srgb`), and the surface's configured usage
/// (`RENDER_ATTACHMENT`, or the copy path's usages, plus
/// [`LayerTarget::extra_usages`]). The surface's `view_formats` are
/// [`LayerTarget::view_formats`].
/// The surface configuration of one slot; the frame latency follows the
/// role ([`frame_latency_for`], REQ-LAT-006), everything else is as given.
fn slot_surface_config(
    role: SurfaceRole,
    usage: wgpu::TextureUsages,
    format: wgpu::TextureFormat,
    (w, h): (u32, u32),
    present_mode: wgpu::PresentMode,
    alpha_mode: wgpu::CompositeAlphaMode,
    view_formats: Vec<wgpu::TextureFormat>,
) -> wgpu::SurfaceConfiguration {
    wgpu::SurfaceConfiguration {
        usage,
        format,
        color_space: Default::default(),
        width: w,
        height: h,
        present_mode,
        desired_maximum_frame_latency: frame_latency_for(role),
        alpha_mode,
        view_formats,
    }
}

/// `render` as the app starts it on `info`: lean on a CPU adapter (one call
/// of `warn`), unchanged otherwise (REQ-LAT-006).
fn render_options_for_adapter(
    info: &wgpu::AdapterInfo,
    render: TableRenderOptions,
    warn: &mut dyn FnMut(&str),
) -> TableRenderOptions {
    match lean_quality_for(info, warn) {
        Some(q) => TableRenderOptions {
            post: q.apply_to_post(render.post),
            ..render
        },
        None => render,
    }
}

pub(crate) fn slot_layers(
    format: wgpu::TextureFormat,
    threaded: bool,
    path: CopyPath,
    downlevel: wgpu::DownlevelFlags,
    usages: wgpu::TextureUsages,
) -> (wgpu::TextureFormat, LayerTarget, wgpu::TextureUsages) {
    let draw_format = if threaded {
        offscreen_format(format)
    } else {
        format
    };
    let layers = LayerTarget::for_target(draw_format, downlevel, usages);
    let base = if threaded {
        path.target_usages()
    } else {
        wgpu::TextureUsages::RENDER_ATTACHMENT
    };
    (draw_format, layers, base | layers.extra_usages())
}

/// `true` for a present mode whose `get_current_texture` / `present` can
/// wait for vertical blank (`Fifo`, `FifoRelaxed`, `AutoVsync`); `false`
/// for the non-blocking modes (`Mailbox`, `Immediate`, `AutoNoVsync`).
pub fn is_blocking_mode(mode: wgpu::PresentMode) -> bool {
    use wgpu::PresentMode as M;
    match mode {
        M::Fifo | M::FifoRelaxed | M::AutoVsync => true,
        M::Mailbox | M::Immediate | M::AutoNoVsync => false,
    }
}

/// How one surface is presented (see the [module docs](self)).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SurfacePlan {
    /// Drawn and presented by the render thread every render-loop
    /// iteration with this mode (the playfield, or a backglass / score
    /// surface with a non-blocking mode).
    Direct(wgpu::PresentMode),
    /// A backglass / score surface drawn by the render thread into an
    /// offscreen frame and presented with this mode by its own presenter
    /// thread (a blocking chosen mode, or a forced presenter thread).
    Presenter(wgpu::PresentMode),
    /// No 8-bit colour format: the table renderer cannot draw it.
    Unsupported,
}

/// The plan of a `role` surface offering `formats` and `present_modes`
/// under `opts` (pure; see the [module docs](self)).
///
/// The playfield is always [`SurfacePlan::Direct`] when it has an 8-bit
/// format (it paces the loop; forcing never applies to it). A backglass /
/// score surface is [`SurfacePlan::Presenter`] when the §2.2 policy
/// ([`choose_present_mode`]) picks a blocking mode for it or when
/// `opts.force_presenter_thread` is set (then with the mode the policy
/// picked, e.g. `Mailbox`), and [`SurfacePlan::Direct`] otherwise.
pub fn plan_surface(
    role: SurfaceRole,
    formats: &[wgpu::TextureFormat],
    present_modes: &[wgpu::PresentMode],
    opts: SurfaceOptions,
) -> SurfacePlan {
    if pick_format(formats).is_none() {
        return SurfacePlan::Unsupported;
    }
    let mode = choose_present_mode(role, present_modes, opts.vrr).mode();
    if role != SurfaceRole::Playfield && (is_blocking_mode(mode) || opts.force_presenter_thread) {
        SurfacePlan::Presenter(mode)
    } else {
        SurfacePlan::Direct(mode)
    }
}

/// What [`AppSurfaces::new`] does with one surface source.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SlotDecision {
    /// Configure the surface with this mode and format and draw and present
    /// it from the render thread every render-loop iteration.
    Draw(wgpu::PresentMode, wgpu::TextureFormat),
    /// A backglass / score surface: configure it with this mode and format,
    /// hand it to its own presenter thread, and draw its frames offscreen
    /// on the render thread.
    Present(wgpu::PresentMode, wgpu::TextureFormat),
    /// A backglass / score surface without an 8-bit format: not drawn (one
    /// stderr note).
    Skip,
    /// A playfield surface that cannot be drawn: the whole table set is
    /// rejected.
    Reject,
}

/// The decision for a `role` surface planned `plan` whose 8-bit format is
/// `format` (pure). A playfield with a format is always
/// [`SlotDecision::Draw`] (never a presenter thread: it paces the loop). A
/// backglass / score surface is [`SlotDecision::Present`] when `plan` says
/// [`SurfacePlan::Presenter`] or its mode is blocking (even if `plan` says
/// [`SurfacePlan::Direct`]), so the render thread never acquires a
/// blocking secondary surface; otherwise [`SlotDecision::Draw`].
pub fn slot_decision(
    role: SurfaceRole,
    plan: SurfacePlan,
    format: Option<wgpu::TextureFormat>,
) -> SlotDecision {
    match (role, plan, format) {
        (SurfaceRole::Playfield, SurfacePlan::Direct(m) | SurfacePlan::Presenter(m), Some(f)) => {
            SlotDecision::Draw(m, f)
        }
        (SurfaceRole::Playfield, _, _) => SlotDecision::Reject,
        (_, SurfacePlan::Presenter(m), Some(f)) => SlotDecision::Present(m, f),
        (_, SurfacePlan::Direct(m), Some(f)) if is_blocking_mode(m) => SlotDecision::Present(m, f),
        (_, SurfacePlan::Direct(m), Some(f)) => SlotDecision::Draw(m, f),
        (_, _, _) => SlotDecision::Skip,
    }
}

/// Whether the table renderer can present a window set whose surfaces got
/// `plans`: at least one surface is drawn directly ([`SurfacePlan::Direct`])
/// and no playfield surface is [`SurfacePlan::Unsupported`] (unsupported
/// secondary surfaces are skipped and [`SurfacePlan::Presenter`] ones are
/// presented by their own threads; neither rejects the set, but neither
/// counts as the directly drawn surface either).
pub fn table_set_accepted(plans: &[(SurfaceRole, SurfacePlan)]) -> bool {
    plans
        .iter()
        .any(|(_, p)| matches!(p, SurfacePlan::Direct(_)))
        && !plans
            .iter()
            .any(|(r, p)| *r == SurfaceRole::Playfield && *p == SurfacePlan::Unsupported)
}

/// How a retained surface is presented ([`AppSurfaces::slots`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SlotPath {
    /// Acquired and presented by the render thread.
    Direct,
    /// Drawn offscreen by the render thread, presented by its own presenter
    /// thread along this copy path.
    PresenterThread(CopyPath),
}

/// One retained surface as seen from outside ([`AppSurfaces::slots`]).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SlotInfo {
    /// Window index (as in [`WindowEvent`]).
    pub window: usize,
    /// The window's primary role.
    pub role: SurfaceRole,
    /// Direct or presenter thread.
    pub path: SlotPath,
    /// The configured present mode.
    pub mode: wgpu::PresentMode,
    /// The surface format (a presenter-thread surface's frames are drawn
    /// in its non-sRGB twin, see the [module docs](self)).
    pub format: wgpu::TextureFormat,
    /// Display commands its renderer applied so far.
    pub display_commands: u64,
    /// The hand-off counters of a presenter-thread surface.
    pub presenter: Option<HandoffStats>,
    /// A presenter-thread surface whose thread ended (closed hand-off):
    /// no longer drawn.
    pub dead: bool,
}

/// A surface the render thread acquires and presents itself.
struct Direct {
    needs_configure: bool,
    /// Its surface was configured with this target's view formats and
    /// usages (format and capabilities never change on resize, so it is
    /// kept for the slot's lifetime).
    layers: LayerCompositor,
    config: wgpu::SurfaceConfiguration,
    surface: wgpu::Surface<'static>,
    /// Dropped after `surface` (field order).
    _keep_alive: Option<WindowGuard>,
}

/// A surface owned by a presenter thread; the render thread keeps the
/// frame pool's textures (inside the hand-off) and the thread handle.
struct Threaded {
    mode: wgpu::PresentMode,
    /// The surface format.
    format: wgpu::TextureFormat,
    /// The offscreen frame format ([`offscreen_format`]).
    draw_format: wgpu::TextureFormat,
    /// Layers of the offscreen frames (always direct).
    layers: LayerCompositor,
    path: CopyPath,
    /// The presenter thread ended: stop drawing (one stderr note).
    dead: bool,
    presenter: PresenterThread<wgpu::Texture>,
}

enum SlotKind {
    Direct(Direct),
    Threaded(Threaded),
}

struct Slot {
    window: usize,
    role: SurfaceRole,
    planned: Vec<(SurfaceRole, Rect)>,
    planned_size: (u32, u32),
    pixel_size: (u32, u32),
    minimized: bool,
    routes: Vec<DisplayRoute>,
    renderer: TableRenderer,
    kind: SlotKind,
}

impl Slot {
    /// Whether display commands still reach this slot's renderer.
    fn live(&self) -> bool {
        !matches!(&self.kind, SlotKind::Threaded(t) if t.dead)
    }
}

/// The table's surfaces: the playfield and non-blocking secondary surfaces
/// presented directly from the render thread, blocking (or forced)
/// backglass / score surfaces through their own presenter threads (see the
/// [module docs](self)).
pub struct AppSurfaces {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Serialises `queue.submit` and `Surface::configure` across the render
    /// thread and the presenter threads.
    gpu_lock: Arc<Mutex<()>>,
    slots: Vec<Slot>,
    frames: u64,
    skipped: usize,
    /// GPU frame timing (7.1 hook 3), when the device supports it.
    timer: Option<GpuFrameTimer>,
    /// The in-game menu, painted over the playfield (issue #84).
    menu: Option<Box<MenuOverlay>>,
}

impl AppSurfaces {
    /// The plan of every source, in source order ([`plan_surface`] on the
    /// surface's capabilities).
    pub fn plan(
        ctx: &GpuContext,
        sources: &[SurfaceSource],
        opts: SurfaceOptions,
    ) -> Vec<SurfacePlan> {
        sources
            .iter()
            .map(|s| {
                let caps = s.surface.get_capabilities(ctx.adapter());
                plan_surface(s.role(), &caps.formats, &caps.present_modes, opts)
            })
            .collect()
    }

    /// Configures every source [`slot_decision`] marks
    /// [`SlotDecision::Draw`] or [`SlotDecision::Present`] and builds its
    /// renderer; a [`SlotDecision::Present`] source's surface moves to a new
    /// presenter thread (with its window guard). A [`SlotDecision::Skip`]
    /// (no 8-bit format) source gets one note on stderr and no slot.
    ///
    /// # Errors
    /// An unsupported playfield surface (the caller falls back to the debug
    /// view, see [`table_set_accepted`]), a surface that cannot be
    /// configured, a renderer that cannot be built, or a presenter thread
    /// that cannot be spawned.
    pub fn new(
        ctx: &GpuContext,
        sources: Vec<SurfaceSource>,
        opts: SurfaceOptions,
        visuals: &TableVisuals,
        render: TableRenderOptions,
    ) -> Result<AppSurfaces, String> {
        // REQ-LAT-006: one warning per call; a software adapter starts lean.
        let render = render_options_for_adapter(&ctx.adapter_info(), render, &mut |m| {
            eprintln!("pin_app: warning: {m}")
        });
        let device = ctx.device().clone();
        let queue = ctx.queue().clone();
        let gpu_lock = Arc::new(Mutex::new(()));
        let mut slots = Vec::with_capacity(sources.len());
        let mut skipped = 0;
        for src in sources {
            let role = src.role();
            let caps = src.surface.get_capabilities(ctx.adapter());
            let plan = plan_surface(role, &caps.formats, &caps.present_modes, opts);
            let (mode, format, threaded) =
                match slot_decision(role, plan, pick_format(&caps.formats)) {
                    SlotDecision::Draw(m, f) => (m, f, false),
                    SlotDecision::Present(m, f) => (m, f, true),
                    SlotDecision::Reject => {
                        return Err(format!(
                            "window {} ({role:?}): no 8-bit surface format",
                            src.window
                        ));
                    }
                    SlotDecision::Skip => {
                        eprintln!(
                            "pin_app: window {} ({role:?}) offers no 8-bit surface format; \
                             not drawing it",
                            src.window
                        );
                        skipped += 1;
                        continue;
                    }
                };
            let path = CopyPath::for_usages(caps.usages);
            let (draw_format, layers, usage) = slot_layers(
                format,
                threaded,
                path,
                ctx.adapter().get_downlevel_capabilities().flags,
                caps.usages,
            );
            let (w, h) = (src.pixel_size.0.max(1), src.pixel_size.1.max(1));
            let config = slot_surface_config(
                role,
                usage,
                format,
                (w, h),
                mode,
                caps.alpha_modes
                    .first()
                    .copied()
                    .unwrap_or(wgpu::CompositeAlphaMode::Auto),
                layers.view_formats(),
            );
            if !configure_surface(&device, &src.surface, &config, &gpu_lock) {
                return Err(format!(
                    "window {} ({role:?}): configure failed",
                    src.window
                ));
            }
            // Post and debug draw in the frame's format; the displays in
            // the layer format (the twin of an `*Srgb` surface).
            let renderer = TableRenderer::with_layer_format(
                ctx,
                draw_format,
                layers.layer_format(),
                (w, h),
                visuals,
                render,
            )
            .map_err(|e| format!("window {} ({role:?}): {e}", src.window))?;
            let planned: Vec<(SurfaceRole, Rect)> = src.viewports.iter().copied().collect();
            let routes = routes_for_window(src.window, &planned, src.logical_size, (w, h));
            let kind = if threaded {
                let handoff = Arc::new(Handoff::new(HANDOFF_SLOTS, (w, h)));
                let target = PresenterTarget {
                    device: device.clone(),
                    queue: queue.clone(),
                    surface: src.surface,
                    config,
                    src_format: draw_format,
                    path,
                    gpu_lock: Arc::clone(&gpu_lock),
                };
                // The thread drops the surface before the guard; a thread
                // without a guard gets a placeholder (the caller keeps the
                // window alive, `SurfaceSource::keep_alive`).
                let guard = src.keep_alive.unwrap_or_else(|| WindowGuard::new(()));
                let presenter = PresenterThread::spawn(
                    &format!("presenter-{}", src.window),
                    guard,
                    handoff,
                    move |h| run_presenter(h, target),
                )
                .map_err(|e| format!("window {} ({role:?}): presenter thread: {e}", src.window))?;
                SlotKind::Threaded(Threaded {
                    mode,
                    format,
                    draw_format,
                    layers: LayerCompositor::new(layers),
                    path,
                    dead: false,
                    presenter,
                })
            } else {
                SlotKind::Direct(Direct {
                    needs_configure: false,
                    layers: LayerCompositor::new(layers),
                    config,
                    surface: src.surface,
                    _keep_alive: src.keep_alive,
                })
            };
            slots.push(Slot {
                window: src.window,
                role,
                planned,
                planned_size: src.logical_size,
                pixel_size: src.pixel_size,
                minimized: false,
                routes,
                renderer,
                kind,
            });
        }
        let out = AppSurfaces {
            device,
            queue,
            gpu_lock,
            slots,
            frames: 0,
            skipped,
            timer: None,
            menu: None,
        };
        debug_assert!(
            out.drawn_modes()
                .all(|(r, m)| r == SurfaceRole::Playfield || !is_blocking_mode(m)),
            "the render thread presents a backglass / score surface with a blocking mode"
        );
        Ok(out)
    }

    /// `(direct, threaded, skipped)`: surfaces presented by the render
    /// thread, surfaces presented by their own presenter threads
    /// ([`SlotDecision::Present`]) and surfaces without an 8-bit format
    /// ([`SlotDecision::Skip`]).
    pub fn plan_counts(&self) -> (usize, usize, usize) {
        let threaded = self
            .slots
            .iter()
            .filter(|s| matches!(s.kind, SlotKind::Threaded(_)))
            .count();
        (self.slots.len() - threaded, threaded, self.skipped)
    }

    /// The role and configured present mode of every surface the render
    /// thread acquires and presents itself, in slot order.
    pub fn drawn_modes(&self) -> impl Iterator<Item = (SurfaceRole, wgpu::PresentMode)> + '_ {
        self.slots.iter().filter_map(|s| match &s.kind {
            SlotKind::Direct(d) => Some((s.role, d.config.present_mode)),
            SlotKind::Threaded(_) => None,
        })
    }

    /// Every retained surface, in slot order (the order of
    /// [`render`](Self::render)'s statuses).
    pub fn slots(&self) -> impl Iterator<Item = SlotInfo> + '_ {
        self.slots.iter().map(|s| {
            let (path, mode, format, presenter, dead) = match &s.kind {
                SlotKind::Direct(d) => (
                    SlotPath::Direct,
                    d.config.present_mode,
                    d.config.format,
                    None,
                    false,
                ),
                SlotKind::Threaded(t) => (
                    SlotPath::PresenterThread(t.path),
                    t.mode,
                    t.format,
                    Some(t.presenter.handoff().stats()),
                    t.dead,
                ),
            };
            SlotInfo {
                window: s.window,
                role: s.role,
                path,
                mode,
                format,
                display_commands: s.renderer.display_commands(),
                presenter,
                dead,
            }
        })
    }

    /// The hand-off counters of every presenter-thread surface: `(window,
    /// role, stats)` in slot order.
    pub fn presenter_stats(&self) -> Vec<(usize, SurfaceRole, HandoffStats)> {
        self.slots
            .iter()
            .filter_map(|s| match &s.kind {
                SlotKind::Threaded(t) => Some((s.window, s.role, t.presenter.handoff().stats())),
                SlotKind::Direct(_) => None,
            })
            .collect()
    }

    /// Number of retained surfaces (direct and presenter-thread).
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// `true` without retained surfaces.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Render-loop iterations in which some surface was presented or handed
    /// to its presenter thread.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Hands a display command to the renderer of every retained window
    /// (direct and presenter-thread; a presenter-thread window whose thread
    /// ended is no longer drawn and ignores it).
    pub fn apply_display_command(&mut self, cmd: DisplayCommand, now_ms: u64) {
        for s in &mut self.slots {
            if s.live() {
                s.renderer.apply_display_command(cmd, now_ms);
            }
        }
    }

    /// Applies a window event from the main thread (resize, minimise, ...).
    /// A presenter-thread surface is reconfigured by its thread on the next
    /// frame of the new size (the render thread does not hold it).
    pub fn handle_window_event(&mut self, ev: &WindowEvent) {
        let Some(s) = self.slots.iter_mut().find(|s| s.window == ev.window) else {
            return;
        };
        match ev.kind {
            WindowEventKind::PixelSizeChanged { w, h } => {
                s.pixel_size = (w, h);
                if w > 0 && h > 0 {
                    match &mut s.kind {
                        SlotKind::Direct(d) => {
                            d.config.width = w;
                            d.config.height = h;
                            d.needs_configure = !configure_surface(
                                &self.device,
                                &d.surface,
                                &d.config,
                                &self.gpu_lock,
                            );
                        }
                        SlotKind::Threaded(t) => {
                            // Frames of the old size are discarded, never
                            // presented; free slots get new textures when
                            // next taken.
                            t.presenter.handoff().set_size((w, h));
                        }
                    }
                    if let Err(e) = s.renderer.resize(w, h) {
                        eprintln!("pin_app: resize window {}: {e}", s.window);
                    }
                    s.routes = routes_for_window(s.window, &s.planned, s.planned_size, (w, h));
                }
            }
            WindowEventKind::Minimized | WindowEventKind::Hidden => s.minimized = true,
            WindowEventKind::Restored
            | WindowEventKind::Shown
            | WindowEventKind::Maximized
            | WindowEventKind::Exposed => s.minimized = false,
            _ => {}
        }
    }

    /// Times every frame's GPU work with `timer` from now on (7.1 hook 3,
    /// see [`GpuFrameTimer`]): [`render`](Self::render) brackets each
    /// frame's submits with its timestamps and collects finished samples
    /// without waiting.
    pub fn set_gpu_timer(&mut self, timer: GpuFrameTimer) {
        self.timer = Some(timer);
    }

    /// The GPU frame timer, if one was set.
    pub fn gpu_timer(&self) -> Option<&GpuFrameTimer> {
        self.timer.as_ref()
    }

    /// The format the menu overlay must be built for to paint over the
    /// playfield slot's frames ([`menu_overlay_format`]): `None` without a
    /// playfield slot.
    pub fn playfield_format(&self) -> Option<wgpu::TextureFormat> {
        self.slots
            .iter()
            .find(|s| s.role == SurfaceRole::Playfield)
            .map(|s| match &s.kind {
                SlotKind::Direct(d) => menu_overlay_format(d.layers.target()),
                SlotKind::Threaded(t) => t.draw_format,
            })
    }

    /// Paints `menu` over every playfield frame from now on while its state
    /// is an open menu (build it for [`playfield_format`](Self::playfield_format)).
    pub fn set_menu_overlay(&mut self, menu: MenuOverlay) {
        self.menu = Some(Box::new(menu));
    }

    /// The menu overlay, if one was set.
    pub fn menu_overlay(&self) -> Option<&MenuOverlay> {
        self.menu.as_deref()
    }

    /// Shows `state` in the menu overlay (no effect without one).
    pub fn set_menu_state(&mut self, state: MenuState) {
        if let Some(m) = &mut self.menu {
            m.set_state(state);
        }
    }

    /// Hands the profiler HUD text to every playfield surface's renderer
    /// (drawn only by renderers built with
    /// [`TableRenderOptions::profiler_hud`]).
    pub fn set_hud_lines(&mut self, lines: &[String]) {
        for s in &mut self.slots {
            if s.role == SurfaceRole::Playfield {
                s.renderer.set_hud_lines(lines);
            }
        }
    }

    /// Draws one frame of `snap` for every retained surface: presents it
    /// (direct) or hands it to the presenter thread (never blocking on it).
    /// Returns each surface's status in slot order: for a presenter-thread
    /// surface [`SurfaceStatus::Presented`] means handed off and
    /// [`SurfaceStatus::Busy`] that no frame slot was free. With a
    /// [`GpuFrameTimer`], the frame's submits are bracketed by its
    /// timestamps and earlier frames' samples are collected (no wait).
    pub fn render(&mut self, snap: &FrameStateSnapshot) -> Vec<SurfaceStatus> {
        self.render_with_playfield_hook(snap, &mut |_| {})
    }

    /// [`render`](Self::render) with a hook for the latency trace
    /// (REQ-LAT-003): `on_playfield` is called exactly once per playfield
    /// slot with that slot's status, before any other slot's work. For a
    /// direct playfield surface that presents, it is called with
    /// [`SurfaceStatus::Presented`] immediately after `queue.present`
    /// returned and before the surface is reconfigured (a suboptimal
    /// surface is reconfigured after the present), so a clock read in the
    /// hook is the playfield's present time, not the end of a reconfigure
    /// or of the whole frame (backglass and score surfaces are drawn after
    /// it). For every other outcome (skipped, hidden, occluded, busy, or a
    /// presenter-thread playfield, which only hands the frame off and has
    /// no present of its own to stamp) it is called with the returned
    /// status after the slot's work. Returns exactly what
    /// [`render`](Self::render) returns.
    pub fn render_with_playfield_hook(
        &mut self,
        snap: &FrameStateSnapshot,
        on_playfield: &mut dyn FnMut(SurfaceStatus),
    ) -> Vec<SurfaceStatus> {
        let (queue, gpu_lock) = (&self.queue, &self.gpu_lock);
        let mut submit = |cb: wgpu::CommandBuffer| {
            let _g = lock_gpu(gpu_lock);
            queue.submit([cb]);
        };
        if let Some(t) = &mut self.timer {
            t.collect();
            t.begin_frame();
        }
        let mut out = Vec::with_capacity(self.slots.len());
        for s in &mut self.slots {
            let status = if s.role == SurfaceRole::Playfield {
                let mut hook = PlayfieldHook::new(&mut *on_playfield);
                let status = Self::render_slot(
                    &self.device,
                    &self.queue,
                    &self.gpu_lock,
                    self.timer.as_mut(),
                    s,
                    snap,
                    self.menu.as_deref_mut(),
                    &mut || hook.presented(),
                );
                hook.finish(status);
                status
            } else {
                Self::render_slot(
                    &self.device,
                    &self.queue,
                    &self.gpu_lock,
                    self.timer.as_mut(),
                    s,
                    snap,
                    None,
                    &mut || {},
                )
            };
            out.push(status);
        }
        let drew = out.contains(&SurfaceStatus::Presented);
        if let Some(t) = &mut self.timer {
            t.end_frame(&mut submit);
        }
        if drew {
            self.frames += 1;
        }
        out
    }

    /// `true` when a playfield surface presented (its `Fifo` acquire paced
    /// the frame); a presenter-thread hand-off never paces.
    pub fn frame_paced(&self, statuses: &[SurfaceStatus]) -> bool {
        pin_render::gpu::paced_by(self.slots.iter().map(|s| s.role), statuses)
    }

    /// Draws one slot, with `menu` painted last into its frame (the
    /// playfield slot). `on_present` is called by a direct surface
    /// immediately after its `queue.present` returned (before any
    /// reconfigure); it is not called on any other path.
    #[allow(clippy::too_many_arguments)]
    fn render_slot(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        gpu_lock: &Mutex<()>,
        timer: Option<&mut GpuFrameTimer>,
        s: &mut Slot,
        snap: &FrameStateSnapshot,
        menu: Option<&mut MenuOverlay>,
        on_present: &mut dyn FnMut(),
    ) -> SurfaceStatus {
        if !s.live() {
            return SurfaceStatus::Skipped;
        }
        if s.minimized || s.pixel_size.0 == 0 || s.pixel_size.1 == 0 {
            return SurfaceStatus::Hidden;
        }
        let target = FrameTarget {
            role: s.role,
            window: s.window,
            routes: &s.routes,
        };
        match &mut s.kind {
            SlotKind::Direct(d) => render_direct(
                device,
                queue,
                gpu_lock,
                timer,
                d,
                &mut s.renderer,
                target,
                snap,
                menu,
                on_present,
            ),
            SlotKind::Threaded(t) => {
                let st = render_threaded(
                    device,
                    queue,
                    gpu_lock,
                    timer,
                    t,
                    &mut s.renderer,
                    target,
                    snap,
                    menu,
                );
                if t.dead {
                    eprintln!(
                        "pin_app: the presenter thread of window {} ({:?}) ended; the window is \
                         no longer drawn",
                        s.window, s.role
                    );
                }
                st
            }
        }
    }
}

impl Drop for AppSurfaces {
    /// Closes every presenter hand-off first, so all presenter threads stop
    /// together; each slot's `PresenterThread` then joins its thread with
    /// the bounded `PRESENTER_JOIN_TIMEOUT` (detaching a stuck one, which
    /// keeps its window through its guard) when the slots drop. No lock is
    /// held while joining.
    fn drop(&mut self) {
        for s in &self.slots {
            if let SlotKind::Threaded(t) = &s.kind {
                t.presenter.handoff().close();
            }
        }
    }
}

/// What a frame is drawn for (disjoint from the slot's renderer and kind).
#[derive(Copy, Clone)]
struct FrameTarget<'a> {
    role: SurfaceRole,
    window: usize,
    routes: &'a [DisplayRoute],
}

/// Records the table frame for `target` into `frame` (default view
/// `view`): for a backglass / score window a clear first, then the
/// renderer's frame, its displays through `layers`
/// ([`TableRenderer::encode_composited`]: `LayerCompositor::begin` after
/// the playfield, post and debug, `end` after the displays, both before the
/// caller's submit).
#[allow(clippy::too_many_arguments)]
fn encode_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    frame: &wgpu::Texture,
    view: &wgpu::TextureView,
    layers: &mut LayerCompositor,
    renderer: &mut TableRenderer,
    target: FrameTarget<'_>,
    snap: &FrameStateSnapshot,
) {
    let playfield = target.role == SurfaceRole::Playfield;
    if !playfield {
        // Backglass / score window: clear, then its displays.
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("secondary clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    renderer.encode_composited(
        device,
        queue,
        encoder,
        frame,
        view,
        layers,
        playfield,
        target.window,
        target.routes,
        snap,
    );
}

/// The format a menu overlay paints a direct frame of `layers` in: the
/// non-sRGB twin when the frame has a twin view ([`LayerTarget::TwinView`];
/// egui blends in display-encoded space, like the display layers), else
/// the frame's own format.
pub fn menu_overlay_format(layers: LayerTarget) -> wgpu::TextureFormat {
    match layers {
        LayerTarget::TwinView { twin, .. } => twin,
        other => other.target_format(),
    }
}

/// Paints `menu` (when open) last into `frame` (`view`: its default view;
/// a twin view is made for a [`LayerTarget::TwinView`] frame, see
/// [`menu_overlay_format`]). A closed menu records nothing.
fn paint_menu(
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    frame: &wgpu::Texture,
    view: &wgpu::TextureView,
    layers: LayerTarget,
    menu: Option<&mut MenuOverlay>,
) {
    let Some(m) = menu.filter(|m| m.is_open()) else {
        return;
    };
    let size = (frame.width(), frame.height());
    if m.format() == frame.format() {
        m.paint(queue, encoder, view, size);
    } else if let LayerTarget::TwinView { twin, .. } = layers
        && m.format() == twin
    {
        let twin_view = frame.create_view(&wgpu::TextureViewDescriptor {
            label: Some("menu overlay (twin view)"),
            format: Some(twin),
            ..Default::default()
        });
        m.paint(queue, encoder, &twin_view, size);
    }
}

/// Submits one frame command buffer, between the GPU frame timer's
/// timestamps when one is timing the frame ([`GpuFrameTimer::bracket`]).
fn submit_frame(
    queue: &wgpu::Queue,
    gpu_lock: &Mutex<()>,
    timer: Option<&mut GpuFrameTimer>,
    frame: wgpu::CommandBuffer,
) {
    let cbs = match timer {
        Some(t) => t.bracket(frame),
        None => {
            let mut one = arrayvec::ArrayVec::<wgpu::CommandBuffer, 3>::new();
            one.push(frame);
            one
        }
    };
    let _g = lock_gpu(gpu_lock);
    queue.submit(cbs);
}

/// Once-only bookkeeping of the playfield hook for one playfield slot
/// (REQ-LAT-003): [`presented`](Self::presented) fires the hook with
/// [`SurfaceStatus::Presented`] at the direct surface's present;
/// [`finish`](Self::finish) fires it with the slot's returned status only
/// if it has not fired yet. So the hook runs exactly once per slot.
struct PlayfieldHook<'h> {
    hook: &'h mut dyn FnMut(SurfaceStatus),
    fired: bool,
}

impl<'h> PlayfieldHook<'h> {
    fn new(hook: &'h mut dyn FnMut(SurfaceStatus)) -> Self {
        Self { hook, fired: false }
    }

    /// The direct playfield surface's `queue.present` just returned.
    fn presented(&mut self) {
        if !self.fired {
            self.fired = true;
            (self.hook)(SurfaceStatus::Presented);
        }
    }

    /// The slot's work ended with `status`: fires the hook unless the
    /// present already did.
    fn finish(mut self, status: SurfaceStatus) {
        if !self.fired {
            self.fired = true;
            (self.hook)(status);
        }
    }
}

/// Presents, then calls `on_present`, then (on a suboptimal surface)
/// reconfigures: the present-time hook (REQ-LAT-003) is never delayed by a
/// reconfigure, which can block.
fn present_then_reconfigure(
    present: impl FnOnce(),
    on_present: &mut dyn FnMut(),
    suboptimal: bool,
    reconfigure: impl FnOnce(),
) {
    present();
    on_present();
    if suboptimal {
        reconfigure();
    }
}

/// Acquires, draws and presents a direct surface; `on_present` is called
/// immediately after the present returned, before a suboptimal surface is
/// reconfigured ([`present_then_reconfigure`]).
#[allow(clippy::too_many_arguments)]
fn render_direct(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    gpu_lock: &Mutex<()>,
    timer: Option<&mut GpuFrameTimer>,
    d: &mut Direct,
    renderer: &mut TableRenderer,
    target: FrameTarget<'_>,
    snap: &FrameStateSnapshot,
    menu: Option<&mut MenuOverlay>,
    on_present: &mut dyn FnMut(),
) -> SurfaceStatus {
    if d.needs_configure {
        if !configure_surface(device, &d.surface, &d.config, gpu_lock) {
            return SurfaceStatus::Skipped;
        }
        d.needs_configure = false;
    }
    let (frame, suboptimal) = match d.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(t) => (t, false),
        wgpu::CurrentSurfaceTexture::Suboptimal(t) => (t, true),
        wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
            d.needs_configure = !configure_surface(device, &d.surface, &d.config, gpu_lock);
            return SurfaceStatus::Skipped;
        }
        wgpu::CurrentSurfaceTexture::Occluded => return SurfaceStatus::Occluded,
        _ => return SurfaceStatus::Skipped,
    };
    let view = frame
        .texture
        .create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("table frame"),
    });
    encode_frame(
        device,
        queue,
        &mut encoder,
        &frame.texture,
        &view,
        &mut d.layers,
        renderer,
        target,
        snap,
    );
    paint_menu(
        queue,
        &mut encoder,
        &frame.texture,
        &view,
        d.layers.target(),
        menu,
    );
    submit_frame(queue, gpu_lock, timer, encoder.finish());
    present_then_reconfigure(
        || queue.present(frame),
        on_present,
        suboptimal,
        || d.needs_configure = !configure_surface(device, &d.surface, &d.config, gpu_lock),
    );
    SurfaceStatus::Presented
}

/// Draws a presenter-thread surface's frame into a free pool texture,
/// submits it and posts it to the presenter thread. Never touches the
/// surface and never waits for the presenter: [`SurfaceStatus::Busy`] when
/// no slot is free; [`SurfaceStatus::Skipped`] (and `t.dead`) when the
/// presenter thread ended.
#[allow(clippy::too_many_arguments)]
fn render_threaded(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    gpu_lock: &Mutex<()>,
    timer: Option<&mut GpuFrameTimer>,
    t: &mut Threaded,
    renderer: &mut TableRenderer,
    target: FrameTarget<'_>,
    snap: &FrameStateSnapshot,
    menu: Option<&mut MenuOverlay>,
) -> SurfaceStatus {
    let handoff = t.presenter.handoff();
    let mut frame = match handoff.try_take_free() {
        TakeFree::Frame(f) => f,
        TakeFree::Busy => return SurfaceStatus::Busy,
        TakeFree::Closed => {
            t.dead = true;
            return SurfaceStatus::Skipped;
        }
    };
    let size = renderer.size();
    if frame.size() != size || frame.payload().is_none() {
        // A stale-size (or new) slot: a texture of the current size.
        frame.set_payload(offscreen_frame_texture(device, t.draw_format, size), size);
    }
    let Some(tex) = frame.payload() else {
        handoff.give_back(frame);
        return SurfaceStatus::Skipped;
    };
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("presenter-thread frame"),
    });
    encode_frame(
        device,
        queue,
        &mut encoder,
        tex,
        &view,
        &mut t.layers,
        renderer,
        target,
        snap,
    );
    paint_menu(queue, &mut encoder, tex, &view, t.layers.target(), menu);
    // Submitted before the post: the presenter's copy, submitted later on
    // the same queue, sees the finished frame (no CPU wait).
    submit_frame(queue, gpu_lock, timer, encoder.finish());
    if handoff.post(frame) {
        SurfaceStatus::Presented
    } else {
        t.dead = true;
        SurfaceStatus::Skipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::PresentMode as M;
    use wgpu::TextureFormat as F;

    const ROLES: [SurfaceRole; 3] = [
        SurfaceRole::Playfield,
        SurfaceRole::Backglass,
        SurfaceRole::Score,
    ];
    const MODE_SETS: [&[M]; 9] = [
        &[M::Fifo],
        &[M::Fifo, M::Mailbox],
        &[M::Fifo, M::Immediate],
        &[M::Fifo, M::FifoRelaxed],
        &[M::FifoRelaxed],
        &[M::Mailbox],
        &[M::Immediate],
        &[M::Fifo, M::FifoRelaxed, M::Mailbox, M::Immediate],
        &[],
    ];
    const VRR_FORCE: [(bool, bool); 4] =
        [(false, false), (false, true), (true, false), (true, true)];
    const FORMAT_SETS: [&[F]; 4] = [
        &[F::Bgra8UnormSrgb],
        &[F::Rgba8Unorm, F::Rgba16Float],
        &[F::Rgba16Float, F::Rgb10a2Unorm],
        &[],
    ];

    fn opts(vrr: bool, force: bool) -> SurfaceOptions {
        SurfaceOptions {
            vrr,
            force_presenter_thread: force,
        }
    }

    fn eight_bit(formats: &[F]) -> bool {
        formats.iter().any(|f| {
            matches!(
                f,
                F::Bgra8Unorm | F::Rgba8Unorm | F::Bgra8UnormSrgb | F::Rgba8UnormSrgb
            )
        })
    }

    /// REQ-LAT-003: the present-time hook runs after the present and
    /// before a suboptimal surface's reconfigure.
    #[test]
    fn req_lat_003_hook_runs_after_present_before_reconfigure() {
        for (suboptimal, want) in [
            (true, &["present", "hook", "reconfigure"][..]),
            (false, &["present", "hook"][..]),
        ] {
            let log = std::cell::RefCell::new(Vec::new());
            present_then_reconfigure(
                || log.borrow_mut().push("present"),
                &mut || log.borrow_mut().push("hook"),
                suboptimal,
                || log.borrow_mut().push("reconfigure"),
            );
            assert_eq!(log.into_inner(), want, "suboptimal = {suboptimal}");
        }
    }

    /// REQ-LAT-003: the playfield hook fires exactly once per slot: at the
    /// present for a presented direct slot (not again after the slot
    /// returned), otherwise once with the returned status.
    #[test]
    fn req_lat_003_playfield_hook_fires_once_per_slot() {
        let mut calls = Vec::new();
        {
            let mut rec = |st: SurfaceStatus| calls.push(st);
            let mut h = PlayfieldHook::new(&mut rec);
            h.presented();
            h.presented();
            h.finish(SurfaceStatus::Presented);
        }
        assert_eq!(calls, [SurfaceStatus::Presented]);
        for st in [
            SurfaceStatus::Skipped,
            SurfaceStatus::Hidden,
            SurfaceStatus::Occluded,
            SurfaceStatus::Busy,
            SurfaceStatus::Presented,
        ] {
            let mut calls = Vec::new();
            {
                let mut rec = |s: SurfaceStatus| calls.push(s);
                PlayfieldHook::new(&mut rec).finish(st);
            }
            assert_eq!(calls, [st]);
        }
    }

    #[test]
    fn plans_follow_role_mode_format_and_forcing() {
        let no = opts(false, false);
        let pf = SurfaceRole::Playfield;
        let score = SurfaceRole::Score;
        let bg = SurfaceRole::Backglass;
        assert_eq!(
            plan_surface(pf, &[F::Bgra8UnormSrgb], &[M::Fifo, M::Mailbox], no),
            SurfacePlan::Direct(M::Fifo)
        );
        assert_eq!(
            plan_surface(score, &[F::Bgra8Unorm], &[M::Fifo], no),
            SurfacePlan::Presenter(M::Fifo)
        );
        assert_eq!(
            plan_surface(bg, &[F::Bgra8Unorm], &[M::Fifo, M::Mailbox], no),
            SurfacePlan::Direct(M::Mailbox)
        );
        assert_eq!(
            plan_surface(
                bg,
                &[F::Bgra8Unorm],
                &[M::Fifo, M::Mailbox],
                opts(false, true)
            ),
            // Forcing a presenter thread keeps the mode the policy picked.
            SurfacePlan::Presenter(M::Mailbox)
        );
        assert_eq!(
            plan_surface(
                pf,
                &[F::Bgra8Unorm],
                &[M::Fifo, M::Mailbox],
                opts(false, true)
            ),
            SurfacePlan::Direct(M::Fifo)
        );
        assert_eq!(
            plan_surface(score, &[F::Bgra8Unorm], &[M::Fifo], opts(false, true)),
            SurfacePlan::Presenter(M::Fifo)
        );
        assert_eq!(
            plan_surface(pf, &[F::Rgba16Float], &[M::Fifo], no),
            SurfacePlan::Unsupported
        );
        // The whole sweep: role × modes × formats × VRR × forcing.
        for role in ROLES {
            for modes in MODE_SETS {
                for formats in FORMAT_SETS {
                    for (vrr, force) in VRR_FORCE {
                        let p = plan_surface(role, formats, modes, opts(vrr, force));
                        let choice = choose_present_mode(role, modes, vrr);
                        let expect = if !eight_bit(formats) {
                            SurfacePlan::Unsupported
                        } else if role == SurfaceRole::Playfield {
                            // The playfield always paces directly (forcing
                            // never applies to it).
                            SurfacePlan::Direct(choice.mode())
                        } else if force || is_blocking_mode(choice.mode()) {
                            SurfacePlan::Presenter(choice.mode())
                        } else {
                            SurfacePlan::Direct(choice.mode())
                        };
                        assert_eq!(
                            p, expect,
                            "{role:?} {modes:?} {formats:?} vrr {vrr} force {force}"
                        );
                        if role != SurfaceRole::Playfield
                            && eight_bit(formats)
                            && !modes.contains(&M::Mailbox)
                            && !modes.contains(&M::Immediate)
                        {
                            assert_eq!(p, SurfacePlan::Presenter(M::Fifo));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn only_an_unsupported_playfield_rejects_the_set() {
        use SurfacePlan as P;
        let pf = |p| (SurfaceRole::Playfield, p);
        let score = |p| (SurfaceRole::Score, p);
        let bg = |p| (SurfaceRole::Backglass, p);
        // A Fifo-only score window does not reject the set.
        let fifo_only = plan_surface(
            SurfaceRole::Score,
            &[F::Bgra8Unorm],
            &[M::Fifo],
            opts(false, false),
        );
        assert!(table_set_accepted(&[
            pf(P::Direct(M::Fifo)),
            score(fifo_only)
        ]));
        assert!(table_set_accepted(&[
            pf(P::Direct(M::Fifo)),
            score(P::Presenter(M::Fifo)),
            bg(P::Presenter(M::FifoRelaxed)),
        ]));
        // Presenter-thread surfaces alone have no direct (pacing) surface.
        assert!(!table_set_accepted(&[score(P::Presenter(M::Fifo))]));
        assert!(!table_set_accepted(&[
            score(P::Presenter(M::Fifo)),
            bg(P::Presenter(M::Mailbox)),
        ]));
        assert!(table_set_accepted(&[
            pf(P::Direct(M::Fifo)),
            score(P::Unsupported)
        ]));
        let pf_unsupported = plan_surface(
            SurfaceRole::Playfield,
            &[F::Rgba16Float],
            &[M::Fifo],
            opts(false, false),
        );
        assert!(!table_set_accepted(&[
            pf(pf_unsupported),
            score(P::Direct(M::Mailbox)),
        ]));
        assert!(!table_set_accepted(&[score(P::Unsupported)]));
        assert!(!table_set_accepted(&[]));
    }

    #[test]
    fn blocking_modes_are_the_vsync_bound_ones() {
        for m in [M::Fifo, M::FifoRelaxed, M::AutoVsync] {
            assert!(is_blocking_mode(m), "{m:?}");
        }
        for m in [M::Mailbox, M::Immediate, M::AutoNoVsync] {
            assert!(!is_blocking_mode(m), "{m:?}");
        }
    }

    /// PR #21 rounds 3 and 4 (threads 4577325408, 4577423193): the render
    /// thread never presents a backglass / score surface with a blocking
    /// mode itself, and such a surface is retained on a presenter thread
    /// instead of being dropped.
    #[test]
    fn blocking_secondary_surfaces_get_a_presenter_thread() {
        for role in ROLES {
            for modes in MODE_SETS {
                for formats in FORMAT_SETS {
                    for (vrr, force) in VRR_FORCE {
                        let plan = plan_surface(role, formats, modes, opts(vrr, force));
                        let d = slot_decision(role, plan, pick_format(formats));
                        let ctx = format!("{role:?} {modes:?} {formats:?} vrr {vrr} force {force}");
                        if role == SurfaceRole::Playfield {
                            if eight_bit(formats) {
                                assert!(matches!(d, SlotDecision::Draw(..)), "{ctx}: {d:?}");
                            } else {
                                assert_eq!(d, SlotDecision::Reject, "{ctx}");
                            }
                            continue;
                        }
                        if !eight_bit(formats) {
                            assert_eq!(d, SlotDecision::Skip, "{ctx}");
                            continue;
                        }
                        match d {
                            SlotDecision::Draw(m, f) => {
                                assert!(!is_blocking_mode(m), "{ctx}: drawn with {m:?}");
                                assert!(!force, "{ctx}: forced but drawn directly");
                                assert_eq!(Some(f), pick_format(formats), "{ctx}");
                            }
                            SlotDecision::Present(m, f) => {
                                assert!(force || is_blocking_mode(m), "{ctx}: {m:?}");
                                assert_eq!(Some(f), pick_format(formats), "{ctx}");
                            }
                            other => panic!("{ctx}: {other:?}"),
                        }
                        let offers_non_blocking = modes.iter().any(|m| !is_blocking_mode(*m));
                        assert_eq!(
                            matches!(d, SlotDecision::Draw(..)),
                            offers_non_blocking && !force,
                            "{ctx}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn slot_decision_sends_blocking_secondary_to_a_presenter_whatever_the_plan_says() {
        let f = Some(F::Bgra8Unorm);
        for role in [SurfaceRole::Backglass, SurfaceRole::Score] {
            for m in [M::Fifo, M::FifoRelaxed, M::AutoVsync] {
                assert_eq!(
                    slot_decision(role, SurfacePlan::Direct(m), f),
                    SlotDecision::Present(m, F::Bgra8Unorm)
                );
                assert_eq!(
                    slot_decision(role, SurfacePlan::Presenter(m), f),
                    SlotDecision::Present(m, F::Bgra8Unorm)
                );
            }
            assert_eq!(
                slot_decision(role, SurfacePlan::Presenter(M::Mailbox), f),
                SlotDecision::Present(M::Mailbox, F::Bgra8Unorm)
            );
            assert_eq!(
                slot_decision(role, SurfacePlan::Direct(M::Mailbox), f),
                SlotDecision::Draw(M::Mailbox, F::Bgra8Unorm)
            );
            assert_eq!(
                slot_decision(role, SurfacePlan::Direct(M::Mailbox), None),
                SlotDecision::Skip
            );
            assert_eq!(
                slot_decision(role, SurfacePlan::Presenter(M::Fifo), None),
                SlotDecision::Skip
            );
            assert_eq!(
                slot_decision(role, SurfacePlan::Unsupported, f),
                SlotDecision::Skip
            );
        }
        let pf = SurfaceRole::Playfield;
        assert_eq!(
            slot_decision(pf, SurfacePlan::Direct(M::Fifo), f),
            SlotDecision::Draw(M::Fifo, F::Bgra8Unorm)
        );
        // The playfield is never handed to a presenter thread.
        assert_eq!(
            slot_decision(pf, SurfacePlan::Presenter(M::Fifo), f),
            SlotDecision::Draw(M::Fifo, F::Bgra8Unorm)
        );
        assert_eq!(
            slot_decision(pf, SurfacePlan::Unsupported, None),
            SlotDecision::Reject
        );
        assert_eq!(
            slot_decision(pf, SurfacePlan::Direct(M::Fifo), None),
            SlotDecision::Reject
        );
    }
    /// Issue #10: the layer target, usage and view formats of every slot
    /// kind, over formats × downlevel flags × surface usages × copy paths.
    #[test]
    fn slot_layers_route_srgb_display_layers_through_the_twin() {
        use wgpu::DownlevelFlags as D;
        use wgpu::TextureUsages as U;
        const COPY: U = U::COPY_SRC.union(U::COPY_DST);
        let flag_sets = [
            D::empty(),
            D::VIEW_FORMATS,
            D::SURFACE_VIEW_FORMATS,
            D::all(),
        ];
        let usage_sets = [
            U::RENDER_ATTACHMENT,
            U::RENDER_ATTACHMENT | U::COPY_DST,
            U::RENDER_ATTACHMENT | COPY,
        ];
        for f in [
            F::Bgra8Unorm,
            F::Rgba8Unorm,
            F::Bgra8UnormSrgb,
            F::Rgba8UnormSrgb,
        ] {
            let twin = f.remove_srgb_suffix();
            for d in flag_sets {
                for u in usage_sets {
                    let path = CopyPath::for_usages(u);
                    let ctx = format!("{f:?} {d:?} {u:?}");
                    // Direct slot: the surface itself is the target.
                    let (draw, lt, usage) = slot_layers(f, false, path, d, u);
                    assert_eq!(draw, f, "{ctx}");
                    assert_eq!(lt, LayerTarget::for_target(f, d, u), "{ctx}");
                    assert_eq!(lt.target_format(), f, "{ctx}");
                    assert_eq!(usage, U::RENDER_ATTACHMENT | lt.extra_usages(), "{ctx}");
                    if f.is_srgb() {
                        let want = if d.contains(D::SURFACE_VIEW_FORMATS) {
                            LayerTarget::TwinView { target: f, twin }
                        } else if u.contains(COPY) {
                            LayerTarget::Intermediate { target: f, twin }
                        } else {
                            // Degraded last resort (pin_render part 1).
                            LayerTarget::Direct(f)
                        };
                        if want != LayerTarget::Direct(f) {
                            assert_eq!(lt.layer_format(), twin, "{ctx}");
                            assert!(!lt.layer_format().is_srgb(), "{ctx}");
                        }
                        assert_eq!(lt, want, "{ctx}");
                        match want {
                            LayerTarget::TwinView { .. } => {
                                assert_eq!(lt.view_formats(), vec![twin], "{ctx}");
                                assert_eq!(usage, U::RENDER_ATTACHMENT, "{ctx}");
                            }
                            LayerTarget::Intermediate { .. } => {
                                assert!(lt.view_formats().is_empty(), "{ctx}");
                                assert_eq!(usage, U::RENDER_ATTACHMENT | COPY, "{ctx}");
                            }
                            LayerTarget::Direct(_) => {}
                        }
                    } else {
                        // Non-sRGB surfaces are configured exactly as before.
                        assert_eq!(lt, LayerTarget::Direct(f), "{ctx}");
                        assert!(lt.view_formats().is_empty(), "{ctx}");
                        assert!(lt.extra_usages().is_empty(), "{ctx}");
                        assert_eq!(usage, U::RENDER_ATTACHMENT, "{ctx}");
                    }
                    // Presenter-thread slot: drawn offscreen in the twin,
                    // surface configured with the copy path's usages only.
                    let (draw, lt, usage) = slot_layers(f, true, path, d, u);
                    assert_eq!(draw, twin, "{ctx}");
                    assert_eq!(draw, offscreen_format(f), "{ctx}");
                    assert_eq!(lt, LayerTarget::Direct(twin), "{ctx}");
                    assert!(lt.view_formats().is_empty(), "{ctx}");
                    assert_eq!(usage, path.target_usages(), "{ctx}");
                }
            }
        }
    }

    #[test]
    fn offscreen_frames_are_never_srgb_and_keep_plain_formats() {
        for f in [F::Bgra8Unorm, F::Rgba8Unorm] {
            assert_eq!(offscreen_format(f), f);
        }
        assert_eq!(offscreen_format(F::Bgra8UnormSrgb), F::Bgra8Unorm);
        assert_eq!(offscreen_format(F::Rgba8UnormSrgb), F::Rgba8Unorm);
        // Every format `pick_format` can return gets a non-sRGB offscreen
        // twin that `copy_texture_to_texture` accepts against the surface.
        for formats in FORMAT_SETS {
            if let Some(f) = pick_format(formats) {
                let o = offscreen_format(f);
                assert!(!o.is_srgb(), "{f:?}");
                assert_eq!(o.remove_srgb_suffix(), f.remove_srgb_suffix());
            }
        }
    }

    fn test_info(device_type: wgpu::DeviceType) -> wgpu::AdapterInfo {
        wgpu::AdapterInfo {
            name: "test-adapter".to_string(),
            vendor: 0,
            device: 0,
            device_type,
            device_pci_bus_id: String::new(),
            driver: String::new(),
            driver_info: String::new(),
            backend: wgpu::Backend::Vulkan,
            subgroup_min_size: 0,
            subgroup_max_size: 0,
            transient_saves_memory: None,
            limit_bucket: None,
        }
    }

    /// REQ-LAT-006: the playfield slot config has frame latency 1, the
    /// secondary slots 2, and nothing else depends on the role.
    #[test]
    fn req_lat_006_slot_surface_config_frame_latency() {
        let mk = |role| {
            slot_surface_config(
                role,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
                F::Bgra8Unorm,
                (16, 9),
                M::Fifo,
                wgpu::CompositeAlphaMode::Opaque,
                vec![F::Bgra8UnormSrgb],
            )
        };
        let p = mk(SurfaceRole::Playfield);
        assert_eq!(p.desired_maximum_frame_latency, 1);
        assert_eq!((p.width, p.height, p.present_mode), (16, 9, M::Fifo));
        assert_eq!(p.view_formats, vec![F::Bgra8UnormSrgb]);
        for role in [SurfaceRole::Backglass, SurfaceRole::Score] {
            let c = mk(role);
            assert_eq!(c.desired_maximum_frame_latency, 2);
            let mut q = p.clone();
            q.desired_maximum_frame_latency = 2;
            assert_eq!(c, q);
        }
    }

    /// REQ-LAT-006: a CPU adapter yields lean options and one warning.
    #[test]
    fn req_lat_006_cpu_adapter_starts_lean_with_one_warning() {
        let mut input = TableRenderOptions {
            debug_overlay: true,
            profiler_hud: true,
            ..TableRenderOptions::default()
        };
        input.post.bloom.enabled = true;
        input.post.motion_blur.enabled = true;
        let mut n = 0;
        let out =
            render_options_for_adapter(&test_info(wgpu::DeviceType::Cpu), input, &mut |_| n += 1);
        assert_eq!(n, 1);
        assert_eq!(out.post.aa, pin_render::post::AaMode::Off);
        assert!(!out.post.bloom.enabled && !out.post.motion_blur.enabled);
        assert!(out.debug_overlay && out.profiler_hud);
        assert_eq!(out.post.exposure, input.post.exposure);
    }

    /// REQ-LAT-006: a non-CPU adapter leaves the options as they were.
    #[test]
    fn req_lat_006_other_adapters_keep_options() {
        let mut input = TableRenderOptions::default();
        input.post.bloom.enabled = true;
        for t in [
            wgpu::DeviceType::Other,
            wgpu::DeviceType::IntegratedGpu,
            wgpu::DeviceType::DiscreteGpu,
            wgpu::DeviceType::VirtualGpu,
        ] {
            let mut n = 0;
            let out = render_options_for_adapter(&test_info(t), input, &mut |_| n += 1);
            assert_eq!(n, 0);
            assert_eq!(out.post, input.post);
            assert_eq!(
                (out.debug_overlay, out.profiler_hud),
                (input.debug_overlay, input.profiler_hud)
            );
        }
    }

    #[test]
    fn menu_overlay_paints_through_the_twin_of_a_twin_view_frame() {
        assert_eq!(
            menu_overlay_format(LayerTarget::TwinView {
                target: F::Bgra8UnormSrgb,
                twin: F::Bgra8Unorm
            }),
            F::Bgra8Unorm
        );
        assert_eq!(
            menu_overlay_format(LayerTarget::Intermediate {
                target: F::Rgba8UnormSrgb,
                twin: F::Rgba8Unorm
            }),
            F::Rgba8UnormSrgb
        );
        assert_eq!(
            menu_overlay_format(LayerTarget::Direct(F::Rgba8Unorm)),
            F::Rgba8Unorm
        );
    }
}
