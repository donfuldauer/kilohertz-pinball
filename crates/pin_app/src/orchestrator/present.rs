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
//!
//! After a GPU device loss (#133, SPEC §4 Subphase 1.4 / 6.0) the render
//! thread dissolves its `AppSurfaces` with [`AppSurfaces::into_sources`]:
//! every direct (and skipped) surface comes back as a [`SurfaceSource`]
//! with its window guard, and the display state the sim sent
//! ([`CarriedState`]) is kept, so a new `AppSurfaces` on a new device shows
//! the same scene and scores ([`AppSurfaces::restore_state`]). A surface
//! owned by a presenter thread cannot be taken back from that thread (its
//! body consumes it): the thread is stopped and joined, the surface is
//! dropped with it and the window's guard is kept in
//! [`SurfaceRecovery::unrecovered`].

use super::gpu_timer::GpuFrameTimer;
use super::handoff::{HANDOFF_SLOTS, Handoff, HandoffStats, PresenterThread, TakeFree};
use super::presenter::{
    CopyPath, PresenterTarget, configure_surface, lock_gpu, offscreen_frame_texture, run_presenter,
};
use super::table::DisplayCommand;
use super::view::{TableRenderOptions, TableRenderer, TableVisuals, routes_for_window};
use crate::ui::{MenuOverlay, MenuState};
use arrayvec::ArrayVec;
use pin_core::snapshot::FrameStateSnapshot;
use pin_core::triple_buffer::TripleReader;
use pin_render::displays::{DisplayRoute, MAX_PLAYERS};
use pin_render::gpu::{
    GpuContext, LayerCompositor, LayerTarget, PRESENTER_JOIN_TIMEOUT, SurfaceOptions,
    SurfaceSource, SurfaceStatus, WindowGuard, choose_present_mode, frame_latency_for,
    lean_quality_for, wgpu,
};
use pin_render::topology::{Rect, SurfaceRole, WindowEvent, WindowEventKind};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// A clone of the window guard the thread holds, so the window stays
    /// guarded after the thread is stopped ([`AppSurfaces::into_sources`]).
    /// Dropped after `presenter` (field order).
    keep_alive: Option<WindowGuard>,
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
    /// Sources [`slot_decision`] skipped (no 8-bit format): kept, never
    /// drawn, so [`into_sources`](Self::into_sources) can hand them back.
    parked: Vec<SurfaceSource>,
    /// The display state the sim sent, replayed after a rebuild (boxed:
    /// the presenter enum stays small).
    displays: Box<DisplayMemory>,
}

/// Why one source could not get a slot, and the source when it was not
/// consumed.
struct SlotError {
    message: String,
    source: Option<SurfaceSource>,
}

/// The display state the sim's [`DisplayCommand`]s built, in the least
/// commands that rebuild it: the last scene shown (with its start time)
/// and every player's latest score. Replaying it into a fresh renderer
/// reproduces the scene and score state of one that applied every command
/// (a renderer keeps only the last scene and the latest score per player,
/// for players below [`MAX_PLAYERS`]). Pure; carried across a device-loss
/// rebuild (#133).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayMemory {
    scene: Option<(DisplayCommand, u64)>,
    scores: [Option<(i64, u64)>; MAX_PLAYERS],
}

impl DisplayMemory {
    /// Remembers `cmd`, applied at `now_ms`.
    pub fn record(&mut self, cmd: DisplayCommand, now_ms: u64) {
        match cmd {
            DisplayCommand::Scene(_) => self.scene = Some((cmd, now_ms)),
            DisplayCommand::Score { player, score } => {
                if let Some(slot) = self.scores.get_mut(usize::from(player)) {
                    *slot = Some((score, now_ms));
                }
            }
        }
    }

    /// `true` when nothing was recorded.
    pub fn is_empty(&self) -> bool {
        self.scene.is_none() && self.scores.iter().all(Option::is_none)
    }

    /// The commands that rebuild the state, in order: every recorded
    /// player's score (player order), then the last scene.
    pub fn replay(&self) -> impl Iterator<Item = (DisplayCommand, u64)> + '_ {
        let scores = self.scores.iter().enumerate().filter_map(|(p, s)| {
            s.map(|(score, t)| {
                (
                    DisplayCommand::Score {
                        player: p as u8,
                        score,
                    },
                    t,
                )
            })
        });
        scores.chain(self.scene)
    }
}

/// What [`AppSurfaces`] carries across a rebuild besides its surfaces
/// ([`AppSurfaces::restore_state`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CarriedState {
    /// The display scenes and scores the sim sent.
    pub displays: DisplayMemory,
    /// Windows that were minimised or hidden.
    pub minimized: Vec<usize>,
}

/// A window whose surface could not be handed back (it was owned by a
/// presenter thread, which consumes it). The thread was stopped; the
/// window's guard is kept here so the window stays guarded.
pub struct UnrecoveredSurface {
    /// Window index.
    pub window: usize,
    /// The window's primary role.
    pub role: SurfaceRole,
    /// The window guard (a clone of the one the thread held).
    pub keep_alive: Option<WindowGuard>,
}

/// The surfaces of a dissolved [`AppSurfaces`]
/// ([`AppSurfaces::into_sources`]): ready for [`AppSurfaces::new`] on a new
/// device of the same instance.
pub struct SurfaceRecovery {
    /// Surfaces handed back, in slot order (then the skipped ones), with
    /// their window guards and current pixel sizes.
    pub sources: Vec<SurfaceSource>,
    /// Surfaces that could not be handed back.
    pub unrecovered: Vec<UnrecoveredSurface>,
    /// Display and window state to restore on the new surfaces.
    pub state: CarriedState,
}

impl SurfaceRecovery {
    /// Keeps the recovered sources current while no `AppSurfaces` exists:
    /// a pixel-size change updates the window's source, minimise / hide
    /// and restore / show update [`CarriedState::minimized`].
    pub fn handle_window_event(&mut self, ev: &WindowEvent) {
        match ev.kind {
            WindowEventKind::PixelSizeChanged { w, h } => {
                if let Some(src) = self.sources.iter_mut().find(|s| s.window == ev.window) {
                    src.pixel_size = (w, h);
                }
            }
            WindowEventKind::Minimized | WindowEventKind::Hidden => {
                if !self.state.minimized.contains(&ev.window) {
                    self.state.minimized.push(ev.window);
                }
            }
            WindowEventKind::Restored
            | WindowEventKind::Shown
            | WindowEventKind::Maximized
            | WindowEventKind::Exposed => self.state.minimized.retain(|&w| w != ev.window),
            _ => {}
        }
    }

    /// One line naming the windows whose surfaces could not be handed
    /// back, `None` when every surface was.
    pub fn unrecovered_note(&self) -> Option<String> {
        if self.unrecovered.is_empty() {
            return None;
        }
        let list: Vec<String> = self
            .unrecovered
            .iter()
            .map(|u| format!("window {} ({:?})", u.window, u.role))
            .collect();
        Some(format!(
            "the surface of {} was owned by a presenter thread and cannot be recovered",
            list.join(", ")
        ))
    }
}

/// [`AppSurfaces::try_new`] failed: the reason and every surface that
/// could be handed back.
pub struct AppSurfacesError {
    /// What failed (as [`AppSurfaces::new`] reports it).
    pub message: String,
    /// The sources (and unrecovered windows) for another attempt.
    pub recovery: SurfaceRecovery,
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
        Self::try_new(ctx, sources, opts, visuals, render).map_err(|e| e.message)
    }

    /// [`new`](Self::new) that hands the sources back on failure (for
    /// another attempt, #133): every source not yet consumed and every slot
    /// built so far ([`into_sources`](Self::into_sources)). A source whose
    /// presenter thread could not be spawned is lost (its surface moved
    /// into the thread body).
    ///
    /// # Errors
    /// As [`new`](Self::new), with the recovered sources.
    pub fn try_new(
        ctx: &GpuContext,
        sources: Vec<SurfaceSource>,
        opts: SurfaceOptions,
        visuals: &TableVisuals,
        render: TableRenderOptions,
    ) -> Result<AppSurfaces, Box<AppSurfacesError>> {
        // REQ-LAT-006: one warning per call; a software adapter starts lean.
        let render = render_options_for_adapter(&ctx.adapter_info(), render, &mut |m| {
            eprintln!("pin_app: warning: {m}")
        });
        let mut out = AppSurfaces {
            device: ctx.device().clone(),
            queue: ctx.queue().clone(),
            gpu_lock: Arc::new(Mutex::new(())),
            slots: Vec::with_capacity(sources.len()),
            frames: 0,
            skipped: 0,
            timer: None,
            menu: None,
            parked: Vec::new(),
            displays: Box::default(),
        };
        let mut rest = sources.into_iter();
        while let Some(src) = rest.next() {
            match out.add_slot(ctx, src, opts, visuals, render) {
                Ok(()) => {}
                Err(e) => {
                    let SlotError { message, source } = *e;
                    let mut recovery = out.into_sources();
                    recovery.sources.extend(source);
                    recovery.sources.extend(rest);
                    return Err(Box::new(AppSurfacesError { message, recovery }));
                }
            }
        }
        debug_assert!(
            out.drawn_modes()
                .all(|(r, m)| r == SurfaceRole::Playfield || !is_blocking_mode(m)),
            "the render thread presents a backglass / score surface with a blocking mode"
        );
        Ok(out)
    }

    /// Configures `src` and adds its slot (or parks a skipped source).
    /// `Err` with the reason and, when it was not consumed, the source.
    fn add_slot(
        &mut self,
        ctx: &GpuContext,
        src: SurfaceSource,
        opts: SurfaceOptions,
        visuals: &TableVisuals,
        render: TableRenderOptions,
    ) -> Result<(), Box<SlotError>> {
        let fail = |message: String, source: Option<SurfaceSource>| {
            Box::new(SlotError { message, source })
        };
        let (device, queue, gpu_lock) = (&self.device, &self.queue, &self.gpu_lock);
        {
            let role = src.role();
            let caps = src.surface.get_capabilities(ctx.adapter());
            let plan = plan_surface(role, &caps.formats, &caps.present_modes, opts);
            let (mode, format, threaded) =
                match slot_decision(role, plan, pick_format(&caps.formats)) {
                    SlotDecision::Draw(m, f) => (m, f, false),
                    SlotDecision::Present(m, f) => (m, f, true),
                    SlotDecision::Reject => {
                        return Err(fail(
                            format!("window {} ({role:?}): no 8-bit surface format", src.window),
                            Some(src),
                        ));
                    }
                    SlotDecision::Skip => {
                        eprintln!(
                            "pin_app: window {} ({role:?}) offers no 8-bit surface format; \
                             not drawing it",
                            src.window
                        );
                        self.skipped += 1;
                        self.parked.push(src);
                        return Ok(());
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
            if !configure_surface(device, &src.surface, &config, gpu_lock) {
                return Err(fail(
                    format!("window {} ({role:?}): configure failed", src.window),
                    Some(src),
                ));
            }
            // Post and debug draw in the frame's format; the displays in
            // the layer format (the twin of an `*Srgb` surface).
            let renderer = match TableRenderer::with_layer_format(
                ctx,
                draw_format,
                layers.layer_format(),
                (w, h),
                visuals,
                render,
            ) {
                Ok(r) => r,
                Err(e) => {
                    return Err(fail(
                        format!("window {} ({role:?}): {e}", src.window),
                        Some(src),
                    ));
                }
            };
            let planned: Vec<(SurfaceRole, Rect)> = src.viewports.iter().copied().collect();
            let routes = routes_for_window(src.window, &planned, src.logical_size, (w, h));
            let (window, logical_size, pixel_size) = (src.window, src.logical_size, src.pixel_size);
            let kind = if threaded {
                let handoff = Arc::new(Handoff::new(HANDOFF_SLOTS, (w, h)));
                let target = PresenterTarget {
                    device: device.clone(),
                    queue: queue.clone(),
                    surface: src.surface,
                    config,
                    src_format: draw_format,
                    path,
                    gpu_lock: Arc::clone(gpu_lock),
                };
                // The thread drops the surface before the guard; a thread
                // without a guard gets a placeholder (the caller keeps the
                // window alive, `SurfaceSource::keep_alive`). The slot keeps
                // a clone, so the window stays guarded after the thread is
                // stopped (#133).
                let keep_alive = src.keep_alive.clone();
                let guard = src.keep_alive.unwrap_or_else(|| WindowGuard::new(()));
                let presenter = PresenterThread::spawn(
                    &format!("presenter-{window}"),
                    guard,
                    handoff,
                    move |h| run_presenter(h, target),
                )
                .map_err(|e| {
                    fail(
                        format!("window {window} ({role:?}): presenter thread: {e}"),
                        None,
                    )
                })?;
                SlotKind::Threaded(Threaded {
                    mode,
                    format,
                    draw_format,
                    layers: LayerCompositor::new(layers),
                    path,
                    dead: false,
                    presenter,
                    keep_alive,
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
            self.slots.push(Slot {
                window,
                role,
                planned,
                planned_size: logical_size,
                pixel_size,
                minimized: false,
                routes,
                renderer,
                kind,
            });
        }
        Ok(())
    }

    /// Dissolves the surfaces for a rebuild on a new device (#133): every
    /// presenter hand-off is closed and each presenter thread stopped
    /// (joined, bounded by `PRESENTER_JOIN_TIMEOUT`); every renderer, the
    /// GPU frame timer and the menu overlay are dropped (nothing is drawn
    /// or submitted). Returns each direct surface (slot order, then the
    /// skipped ones) as a [`SurfaceSource`] with its window guard, its
    /// planned viewports and its current pixel size, the windows whose
    /// surface a presenter thread consumed (with their guards), and the
    /// display and window state to restore
    /// ([`restore_state`](Self::restore_state)). The surfaces stay
    /// configured on the old device until a new `AppSurfaces` configures
    /// them on the new one.
    pub fn into_sources(mut self) -> SurfaceRecovery {
        for s in &self.slots {
            if let SlotKind::Threaded(t) = &s.kind {
                t.presenter.handoff().close();
            }
        }
        self.timer = None;
        self.menu = None;
        let mut out = SurfaceRecovery {
            sources: Vec::with_capacity(self.slots.len() + self.parked.len()),
            unrecovered: Vec::new(),
            state: CarriedState {
                displays: std::mem::take(&mut *self.displays),
                minimized: Vec::new(),
            },
        };
        for slot in std::mem::take(&mut self.slots) {
            let Slot {
                window,
                role,
                planned,
                planned_size,
                pixel_size,
                minimized,
                renderer,
                kind,
                ..
            } = slot;
            drop(renderer);
            if minimized {
                out.state.minimized.push(window);
            }
            match kind {
                SlotKind::Direct(d) => {
                    let Direct {
                        surface,
                        _keep_alive: keep_alive,
                        ..
                    } = d;
                    out.sources.push(SurfaceSource {
                        window,
                        viewports: planned.iter().copied().take(3).collect::<ArrayVec<_, 3>>(),
                        surface,
                        logical_size: planned_size,
                        pixel_size,
                        keep_alive,
                    });
                }
                SlotKind::Threaded(mut t) => {
                    // The thread owns (and on exit drops) the surface; a
                    // thread that does not stop in time is detached and
                    // keeps its own guard clone.
                    let _ = t.presenter.stop(PRESENTER_JOIN_TIMEOUT);
                    let keep_alive = t.keep_alive.take();
                    drop(t);
                    out.unrecovered.push(UnrecoveredSurface {
                        window,
                        role,
                        keep_alive,
                    });
                }
            }
        }
        out.sources.append(&mut self.parked);
        out
    }

    /// Restores what [`into_sources`](Self::into_sources) carried: replays
    /// the display commands of `state.displays` into every live renderer
    /// (same scene, same scores) and marks `state.minimized` windows
    /// hidden again.
    pub fn restore_state(&mut self, state: &CarriedState) {
        for (cmd, t) in state.displays.replay() {
            self.apply_display_command(cmd, t);
        }
        for s in &mut self.slots {
            if state.minimized.contains(&s.window) {
                s.minimized = true;
            }
        }
    }

    /// The display state the sim's commands built so far.
    pub fn display_memory(&self) -> &DisplayMemory {
        &self.displays
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
        self.displays.record(cmd, now_ms);
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
        let mut fixed = SnapshotLatch::new(snap);
        self.render_latched(&mut fixed, on_playfield)
    }

    /// [`render_with_playfield_hook`](Self::render_with_playfield_hook)
    /// with a late snapshot latch (#149, REQ-LAT-001): the first playfield
    /// slot calls [`SnapshotLatch::latch`] immediately before it encodes
    /// (for a direct surface after `get_current_texture` returned, which
    /// can block; for a presenter-thread surface after a free frame slot
    /// was taken), first waiting out the latch's minimum lag after the
    /// previous playfield present ([`SnapshotLatch::late_wait`]; that
    /// present is recorded here when the slot returns
    /// [`SurfaceStatus::Presented`]), and every other slot draws
    /// [`SnapshotLatch::drawn`]. A
    /// playfield that is skipped, hidden, occluded or busy does not latch.
    /// After the call, `latch.drawn()` is the snapshot the presented
    /// playfield frame drew (the one a latency trace row must name).
    pub fn render_latched(
        &mut self,
        latch: &mut SnapshotLatch<dyn SnapshotSource + '_>,
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
        let mut playfield_latched = false;
        for s in &mut self.slots {
            let when = slot_latch(s.role, &mut playfield_latched);
            let status = if s.role == SurfaceRole::Playfield {
                let mut hook = PlayfieldHook::new(&mut *on_playfield);
                let status = Self::render_slot(
                    &self.device,
                    &self.queue,
                    &self.gpu_lock,
                    self.timer.as_mut(),
                    s,
                    &mut *latch,
                    when,
                    self.menu.as_deref_mut(),
                    &mut || hook.presented(),
                );
                hook.finish(status);
                if when == SlotLatch::Late && status == SurfaceStatus::Presented {
                    latch.presented(Instant::now());
                }
                status
            } else {
                Self::render_slot(
                    &self.device,
                    &self.queue,
                    &self.gpu_lock,
                    self.timer.as_mut(),
                    s,
                    &mut *latch,
                    when,
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
        latch: &mut SnapshotLatch<dyn SnapshotSource + '_>,
        when: SlotLatch,
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
                latch,
                when,
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
                    latch,
                    when,
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

/// Where the render thread reads the latest simulation snapshot
/// (REQ-LAT-001). The app's source is the sim's triple-buffer reader
/// (wait-free, lock-free, allocation-free: the render thread never blocks
/// the sim thread, R4); a fixed snapshot is a source that never changes.
pub trait SnapshotSource {
    /// The latest published snapshot.
    fn latest(&mut self) -> &FrameStateSnapshot;
}

impl SnapshotSource for TripleReader<FrameStateSnapshot> {
    fn latest(&mut self) -> &FrameStateSnapshot {
        self.read()
    }
}

impl SnapshotSource for &FrameStateSnapshot {
    fn latest(&mut self) -> &FrameStateSnapshot {
        self
    }
}

/// The render thread's snapshot latch (#149, REQ-LAT-001, REQ-LAT-006).
///
/// The render loop latches once at the top of each iteration (for the
/// display commands, the menu and the debug view) and the table presenter
/// latches again for the first playfield slot immediately before it
/// encodes, after the playfield surface's acquire returned
/// ([`AppSurfaces::render_latched`]). So a frame shows the newest snapshot
/// published before its encode, not the one published when the previous
/// present returned: a press that lands just before a present returns is
/// shown by the next frame once the bat has moved, instead of the frame
/// after it. The latch only moves the read later inside the frame; the
/// playfield still queues at most one frame ahead
/// (`desired_maximum_frame_latency` 1, REQ-LAT-006).
///
/// [`drawn`](Self::drawn) is the snapshot of the last latch, which is the
/// snapshot a presented playfield frame drew; [`last_tick`](Self::last_tick)
/// is its tick. A latch whose tick is lower than the previous latch's is
/// counted as non-monotonic ([`take_non_monotonic`](Self::take_non_monotonic)).
/// Each latch copies the snapshot (a few kilobytes, on the stack of the
/// render thread; no heap allocation).
///
/// **Minimum lag.** When the playfield's acquire returns right after the
/// previous present (a software adapter, whose acquire does not wait for
/// the display), a latch at that moment still misses a press that landed
/// just before that present: its bat is raised only a few ticks later. So
/// the late latch is made no earlier than [`min_lag`](Self::with_min_lag)
/// after the previous playfield present returned
/// ([`late_wait`](Self::late_wait); the render thread sleeps the rest,
/// never the sim thread). The app uses [`LATE_LATCH_MIN_LAG`]; a latch made
/// with [`new`](Self::new) has no minimum lag.
pub struct SnapshotLatch<S: ?Sized> {
    drawn: FrameStateSnapshot,
    last_tick: u64,
    non_monotonic: u64,
    latches: u64,
    min_lag: Duration,
    last_present: Option<Instant>,
    source: S,
}

/// The app's minimum lag of the late latch after the previous playfield
/// present (#149, REQ-LAT-001): the longest time from a press to the first
/// snapshot that shows its bat raised 1 degree that the windowed harness's
/// check a accepts (REQ-LAT-004's one tick plus 2 ms of forwarding and
/// wake-up, then at most 8 ticks from the applying tick to the 1 degree
/// raise): 11 ms. A press that lands just before a present returns is then
/// drawn by the next frame. Only the time from the previous present to the
/// acquire's return is topped up, so an acquire that already waited (a
/// display-paced surface) adds nothing.
pub const LATE_LATCH_MIN_LAG: Duration = Duration::from_millis(11);

impl<S: SnapshotSource> SnapshotLatch<S> {
    /// A latch over `source` that has not latched yet: [`drawn`](Self::drawn)
    /// is the default snapshot (tick 0) and [`last_tick`](Self::last_tick) 0.
    pub fn new(source: S) -> Self {
        Self {
            drawn: FrameStateSnapshot::default(),
            last_tick: 0,
            non_monotonic: 0,
            latches: 0,
            min_lag: Duration::ZERO,
            last_present: None,
            source,
        }
    }

    /// This latch with a minimum lag of its late latch after the previous
    /// playfield present ([`late_wait`](Self::late_wait)).
    pub fn with_min_lag(mut self, min_lag: Duration) -> Self {
        self.min_lag = min_lag;
        self
    }
}

impl<S: SnapshotSource + ?Sized> SnapshotLatch<S> {
    /// Reads the latest snapshot now and makes it the drawn one: updates
    /// [`last_tick`](Self::last_tick) and counts a tick lower than the
    /// previous latch's as non-monotonic.
    pub fn latch(&mut self) -> &FrameStateSnapshot {
        self.drawn = *self.source.latest();
        if self.drawn.tick < self.last_tick {
            self.non_monotonic += 1;
        }
        self.last_tick = self.drawn.tick;
        self.latches += 1;
        &self.drawn
    }

    /// The snapshot of the last [`latch`](Self::latch) (the default
    /// snapshot before the first).
    pub fn drawn(&self) -> &FrameStateSnapshot {
        &self.drawn
    }

    /// The tick of the last latch (0 before the first).
    pub fn last_tick(&self) -> u64 {
        self.last_tick
    }

    /// The number of latches since the latch was made.
    pub fn latches(&self) -> u64 {
        self.latches
    }

    /// Non-monotonic latches counted since the last call (then reset).
    pub fn take_non_monotonic(&mut self) -> u64 {
        std::mem::take(&mut self.non_monotonic)
    }

    /// Records that the playfield presented at `at` (the start of the next
    /// late latch's minimum lag).
    pub fn presented(&mut self, at: Instant) {
        self.last_present = Some(at);
    }

    /// How long a late latch at `now` waits first: the rest of the minimum
    /// lag after the previous playfield present (zero without a minimum
    /// lag, before the first present, or once the lag has passed).
    pub fn late_wait(&self, now: Instant) -> Duration {
        match self.last_present {
            Some(p) => (p + self.min_lag).saturating_duration_since(now),
            None => Duration::ZERO,
        }
    }
}

/// Which snapshot a slot draws (#149): the first playfield slot of a frame
/// latches late (immediately before its encode, after its acquire); every
/// other slot draws the snapshot already latched.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum SlotLatch {
    /// Re-read the latest snapshot just before encoding.
    Late,
    /// Draw the snapshot already latched.
    Latched,
}

impl SlotLatch {
    /// The snapshot this slot draws from `latch`.
    fn snapshot<'l>(
        self,
        latch: &'l mut SnapshotLatch<dyn SnapshotSource + '_>,
    ) -> &'l FrameStateSnapshot {
        match self {
            Self::Late => {
                // The render thread waits (the sim thread keeps ticking and
                // publishing through the triple buffer).
                let wait = latch.late_wait(Instant::now());
                if !wait.is_zero() {
                    std::thread::sleep(wait);
                }
                latch.latch()
            }
            Self::Latched => latch.drawn(),
        }
    }
}

/// The latch kind of each slot of a frame, in slot order: only the first
/// playfield slot latches late (so a frame's playfield, its trace row and
/// the slots drawn after it share one snapshot).
fn slot_latch(role: SurfaceRole, playfield_latched: &mut bool) -> SlotLatch {
    if role == SurfaceRole::Playfield && !*playfield_latched {
        *playfield_latched = true;
        SlotLatch::Late
    } else {
        SlotLatch::Latched
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
    latch: &mut SnapshotLatch<dyn SnapshotSource + '_>,
    when: SlotLatch,
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
    // The late latch (#149): the acquire has returned, so the newest
    // snapshot is read just before the frame is encoded.
    let snap = when.snapshot(latch);
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
    latch: &mut SnapshotLatch<dyn SnapshotSource + '_>,
    when: SlotLatch,
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
    let snap = when.snapshot(latch);
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

    // -----------------------------------------------------------------
    // #133 (KP-052): state carried across a device-loss rebuild
    // -----------------------------------------------------------------

    use pin_core::SceneId;

    /// SPEC §4 Subphase 6.0 / #133: replaying the memory reproduces the
    /// last scene (with its start time) and each player's latest score;
    /// players beyond `MAX_PLAYERS` are ignored, as a renderer ignores
    /// them.
    #[test]
    fn display_memory_keeps_the_last_scene_and_latest_scores() {
        let mut m = DisplayMemory::default();
        assert!(m.is_empty());
        assert_eq!(m.replay().count(), 0);
        m.record(DisplayCommand::Scene(SceneId(1)), 10);
        m.record(
            DisplayCommand::Score {
                player: 2,
                score: 7,
            },
            11,
        );
        m.record(
            DisplayCommand::Score {
                player: 0,
                score: 5,
            },
            12,
        );
        m.record(
            DisplayCommand::Score {
                player: 2,
                score: 9,
            },
            13,
        );
        m.record(DisplayCommand::Scene(SceneId(3)), 14);
        m.record(
            DisplayCommand::Score {
                player: MAX_PLAYERS as u8,
                score: 1,
            },
            15,
        );
        assert!(!m.is_empty());
        let got: Vec<_> = m.replay().collect();
        assert_eq!(
            got,
            [
                (
                    DisplayCommand::Score {
                        player: 0,
                        score: 5
                    },
                    12
                ),
                (
                    DisplayCommand::Score {
                        player: 2,
                        score: 9
                    },
                    13
                ),
                (DisplayCommand::Scene(SceneId(3)), 14),
            ]
        );
        // Replaying into a fresh memory gives the same memory.
        let mut again = DisplayMemory::default();
        for (c, t) in m.replay() {
            again.record(c, t);
        }
        assert_eq!(again, m);
    }

    /// While no `AppSurfaces` exists, minimise / restore events update the
    /// carried window state, and windows whose surface a presenter thread
    /// consumed are named (with their guards kept).
    #[test]
    fn surface_recovery_tracks_window_state_and_names_lost_surfaces() {
        let ev = |window, kind| WindowEvent {
            timestamp_ns: 0,
            window,
            kind,
        };
        let guard = WindowGuard::new(());
        let mut r = SurfaceRecovery {
            sources: Vec::new(),
            unrecovered: Vec::new(),
            state: CarriedState::default(),
        };
        assert_eq!(r.unrecovered_note(), None);
        r.handle_window_event(&ev(1, WindowEventKind::Minimized));
        r.handle_window_event(&ev(1, WindowEventKind::Hidden));
        r.handle_window_event(&ev(2, WindowEventKind::Hidden));
        assert_eq!(r.state.minimized, [1, 2]);
        r.handle_window_event(&ev(1, WindowEventKind::Restored));
        r.handle_window_event(&ev(0, WindowEventKind::PixelSizeChanged { w: 8, h: 8 }));
        assert_eq!(r.state.minimized, [2]);
        r.unrecovered.push(UnrecoveredSurface {
            window: 2,
            role: SurfaceRole::Score,
            keep_alive: Some(guard.clone()),
        });
        assert_eq!(guard.strong_count(), 2, "the window stays guarded");
        assert_eq!(
            r.unrecovered_note().as_deref(),
            Some(
                "the surface of window 2 (Score) was owned by a presenter thread and cannot be \
                 recovered"
            )
        );
        drop(r);
        assert_eq!(guard.strong_count(), 1);
    }

    /// A fake snapshot source whose tick advances between reads: read `i`
    /// returns `ticks[i]` (the last one once exhausted), counting reads.
    struct AdvancingSource {
        ticks: Vec<u64>,
        reads: usize,
        snap: FrameStateSnapshot,
    }

    impl AdvancingSource {
        fn new(ticks: &[u64]) -> Self {
            Self {
                ticks: ticks.to_vec(),
                reads: 0,
                snap: FrameStateSnapshot::default(),
            }
        }
    }

    impl SnapshotSource for AdvancingSource {
        fn latest(&mut self) -> &FrameStateSnapshot {
            let i = self.reads.min(self.ticks.len() - 1);
            self.reads += 1;
            self.snap.tick = self.ticks[i];
            &self.snap
        }
    }

    /// REQ-LAT-001 (#149): the sim publishes tick 16 between the early
    /// latch (tick 10, top of the iteration) and the playfield's late latch
    /// (after its acquire): the frame draws tick 16, and `drawn` and
    /// `last_tick` name it (the snapshot the trace row records).
    #[test]
    fn req_lat_001_late_latch_draws_the_later_snapshot() {
        let mut latch = SnapshotLatch::new(AdvancingSource::new(&[10, 16]));
        assert_eq!(latch.last_tick(), 0);
        assert_eq!(latch.drawn().tick, 0);
        assert_eq!(latch.latch().tick, 10);
        let dynl: &mut SnapshotLatch<dyn SnapshotSource> = &mut latch;
        assert_eq!(SlotLatch::Late.snapshot(dynl).tick, 16);
        assert_eq!(dynl.drawn().tick, 16);
        assert_eq!(dynl.last_tick(), 16);
        assert_eq!(dynl.latches(), 2);
        assert_eq!(dynl.take_non_monotonic(), 0);
    }

    /// REQ-LAT-001: a slot that draws the latched snapshot does not read
    /// the source again, so the slots after the playfield draw the
    /// playfield's snapshot.
    #[test]
    fn req_lat_001_latched_slot_does_not_reread() {
        let mut latch = SnapshotLatch::new(AdvancingSource::new(&[3, 7, 11]));
        latch.latch();
        let dynl: &mut SnapshotLatch<dyn SnapshotSource> = &mut latch;
        assert_eq!(SlotLatch::Late.snapshot(dynl).tick, 7);
        assert_eq!(SlotLatch::Latched.snapshot(dynl).tick, 7);
        assert_eq!(SlotLatch::Latched.snapshot(dynl).tick, 7);
        assert_eq!(latch.source.reads, 2);
    }

    /// REQ-LAT-001: a late latch whose tick is below the previous latch's
    /// is still counted as non-monotonic (once; the count resets when
    /// taken), and `last_tick` follows the snapshot actually drawn.
    #[test]
    fn req_lat_001_late_latch_counts_a_non_monotonic_read() {
        let mut latch = SnapshotLatch::new(AdvancingSource::new(&[20, 18, 25]));
        latch.latch();
        let dynl: &mut SnapshotLatch<dyn SnapshotSource> = &mut latch;
        assert_eq!(SlotLatch::Late.snapshot(dynl).tick, 18);
        assert_eq!(dynl.last_tick(), 18);
        assert_eq!(dynl.take_non_monotonic(), 1);
        assert_eq!(dynl.take_non_monotonic(), 0);
        assert_eq!(dynl.latch().tick, 25);
        assert_eq!(dynl.take_non_monotonic(), 0);
    }

    /// REQ-LAT-001: only the first playfield slot of a frame latches late;
    /// a backglass drawn before it and every slot after it draw the
    /// latched snapshot.
    #[test]
    fn req_lat_001_only_the_first_playfield_slot_latches_late() {
        let mut latched = false;
        let kinds: Vec<SlotLatch> = [
            SurfaceRole::Backglass,
            SurfaceRole::Playfield,
            SurfaceRole::Score,
            SurfaceRole::Playfield,
        ]
        .into_iter()
        .map(|r| slot_latch(r, &mut latched))
        .collect();
        assert_eq!(
            kinds,
            [
                SlotLatch::Latched,
                SlotLatch::Late,
                SlotLatch::Latched,
                SlotLatch::Latched
            ]
        );
        let mut none = false;
        assert_eq!(
            slot_latch(SurfaceRole::Score, &mut none),
            SlotLatch::Latched
        );
        assert!(!none);
    }

    /// REQ-LAT-001, REQ-LAT-006: the app's source is the sim's triple
    /// buffer (wait-free; the render thread never blocks the sim): a value
    /// published after the early latch is what the late latch draws, with
    /// no frame of queueing added (the latch only moves the read inside
    /// the frame; the playfield keeps `desired_maximum_frame_latency` 1).
    #[test]
    fn req_lat_006_late_latch_reads_the_triple_buffer_without_a_queued_frame() {
        let (mut writer, reader) = pin_core::TripleBuffer::new(FrameStateSnapshot::default());
        let mut latch = SnapshotLatch::new(reader);
        let mut s = FrameStateSnapshot {
            tick: 5,
            ..Default::default()
        };
        writer.publish(s);
        assert_eq!(latch.latch().tick, 5);
        s.tick = 9;
        writer.publish(s);
        let dynl: &mut SnapshotLatch<dyn SnapshotSource> = &mut latch;
        assert_eq!(SlotLatch::Late.snapshot(dynl).tick, 9);
        assert_eq!(dynl.drawn().tick, 9);
        assert_eq!(frame_latency_for(SurfaceRole::Playfield), 1);
    }

    /// REQ-LAT-001: a fixed snapshot is a source (the
    /// [`AppSurfaces::render_with_playfield_hook`] path): latching it
    /// draws that snapshot.
    #[test]
    fn req_lat_001_fixed_snapshot_source() {
        let s = FrameStateSnapshot {
            tick: 42,
            ..Default::default()
        };
        let mut latch = SnapshotLatch::new(&s);
        let dynl: &mut SnapshotLatch<dyn SnapshotSource + '_> = &mut latch;
        assert_eq!(SlotLatch::Late.snapshot(dynl), &s);
        assert_eq!(dynl.last_tick(), 42);
    }

    /// REQ-LAT-001 (#149): the late latch waits out the minimum lag after
    /// the previous playfield present and no more: the full lag right
    /// after the present, the rest part-way, nothing once it has passed,
    /// before the first present, or without a minimum lag (constructed
    /// instants; nothing sleeps).
    #[test]
    fn req_lat_001_late_wait_tops_up_the_minimum_lag() {
        let ms = Duration::from_millis;
        let t0 = Instant::now();
        let mut latch = SnapshotLatch::new(AdvancingSource::new(&[1])).with_min_lag(ms(11));
        assert_eq!(latch.late_wait(t0), Duration::ZERO);
        latch.presented(t0);
        assert_eq!(latch.late_wait(t0), ms(11));
        assert_eq!(latch.late_wait(t0 + ms(4)), ms(7));
        assert_eq!(latch.late_wait(t0 + ms(11)), Duration::ZERO);
        assert_eq!(latch.late_wait(t0 + ms(30)), Duration::ZERO);
        let mut none = SnapshotLatch::new(AdvancingSource::new(&[1]));
        none.presented(t0);
        assert_eq!(none.late_wait(t0), Duration::ZERO);
    }

    /// REQ-LAT-001: the app's minimum lag covers check a's longest
    /// press-to-raise time (one tick + 2 ms forwarding, then 8 ticks to
    /// the 1 degree raise) and stays well inside two 60 Hz refresh
    /// intervals.
    #[test]
    fn req_lat_001_app_minimum_lag_covers_the_press_to_raise_bound() {
        assert_eq!(
            LATE_LATCH_MIN_LAG,
            Duration::from_millis(1 + 2) + Duration::from_millis(8)
        );
        assert!(LATE_LATCH_MIN_LAG < Duration::from_micros(33_333));
    }
}
