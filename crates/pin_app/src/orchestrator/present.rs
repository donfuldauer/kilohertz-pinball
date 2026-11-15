//! Presentation of the table frame from the render thread (Subphase 6.0
//! wiring).
//!
//! The 1.4 [`SurfaceSet`](pin_render::gpu::SurfaceSet) only draws debug
//! lines and has no hook for other content, and `pin_render` internals are
//! not changed in this round. So when a table is loaded, the render thread
//! presents its own frames here: each drawn window's surface gets a
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
//! * [`SurfacePlan::Deferred`]: a backglass / score window whose chosen
//!   mode is blocking (it offers only `Fifo` / `FifoRelaxed`). D19 wants
//!   such a surface on its own presenter thread, and handing table frames
//!   to one needs a hand-off that does not exist yet, so the surface is
//!   **not drawn** on the table path (one stderr note; its window stays
//!   undrawn). The render thread therefore never acquires or presents a
//!   blocking-mode surface for a secondary window, and a vsync-bound
//!   secondary acquire can neither stall the playfield nor block the
//!   render-thread join (DECISIONS "6.0 — secondary Fifo-only surfaces,
//!   round 3").
//! * [`SurfacePlan::Unsupported`]: no 8-bit colour format. A secondary
//!   surface planned so is skipped (one stderr note); only an unsupported
//!   **playfield** rejects the set ([`table_set_accepted`]), and the render
//!   thread then falls back to the `SurfaceSet` debug view (DECISIONS
//!   "6.0 — wiring completed").
//!
//! A deferred or skipped secondary surface never turns the cabinet into
//! the debug view, so the table renderer and the sim's display commands
//! (scenes, scores) stay on whenever the playfield can be drawn.

use super::table::DisplayCommand;
use super::view::{TableRenderOptions, TableRenderer, TableVisuals, routes_for_window};
use pin_core::snapshot::FrameStateSnapshot;
use pin_render::displays::DisplayRoute;
use pin_render::gpu::{
    GpuContext, SurfaceOptions, SurfaceSource, SurfaceStatus, WindowGuard, choose_present_mode,
    wgpu,
};
use pin_render::topology::{Rect, SurfaceRole, WindowEvent, WindowEventKind};

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
    /// A backglass / score surface whose chosen mode (carried here) is
    /// blocking: not drawn on the table path.
    Deferred(wgpu::PresentMode),
    /// No 8-bit colour format: the table renderer cannot draw it.
    Unsupported,
}

/// The plan of a `role` surface offering `formats` and `present_modes`
/// under `opts` (pure; see the [module docs](self)).
///
/// The playfield is always [`SurfacePlan::Direct`] when it has an 8-bit
/// format. A backglass / score surface is [`SurfacePlan::Deferred`] when
/// the §2.2 policy ([`choose_present_mode`]) picks a blocking mode for it
/// and [`SurfacePlan::Direct`] otherwise. `opts.force_presenter_thread`
/// does not change the plan: a non-blocking secondary present needs no
/// presenter thread on the table path, and a blocking one is deferred
/// either way.
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
    if role != SurfaceRole::Playfield && is_blocking_mode(mode) {
        SurfacePlan::Deferred(mode)
    } else {
        SurfacePlan::Direct(mode)
    }
}

/// What [`AppSurfaces::new`] does with one surface source.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SlotDecision {
    /// Configure the surface with this mode and format and draw it every
    /// render-loop iteration.
    Draw(wgpu::PresentMode, wgpu::TextureFormat),
    /// A backglass / score surface with this blocking mode: not drawn on
    /// the table path (one stderr note).
    Defer(wgpu::PresentMode),
    /// A backglass / score surface without an 8-bit format: not drawn (one
    /// stderr note).
    Skip,
    /// A playfield surface that cannot be drawn: the whole table set is
    /// rejected.
    Reject,
}

/// The decision for a `role` surface planned `plan` whose 8-bit format is
/// `format` (pure). Only a playfield or a non-blocking secondary surface
/// is ever [`SlotDecision::Draw`]: a blocking mode on a backglass / score
/// surface is [`SlotDecision::Defer`] even if `plan` says
/// [`SurfacePlan::Direct`].
pub fn slot_decision(
    role: SurfaceRole,
    plan: SurfacePlan,
    format: Option<wgpu::TextureFormat>,
) -> SlotDecision {
    let mode = match plan {
        SurfacePlan::Direct(m) | SurfacePlan::Deferred(m) => Some(m),
        SurfacePlan::Unsupported => None,
    };
    match (role, mode, format) {
        (SurfaceRole::Playfield, Some(m), Some(f)) => SlotDecision::Draw(m, f),
        (SurfaceRole::Playfield, _, _) => SlotDecision::Reject,
        (_, Some(m), Some(_)) if is_blocking_mode(m) => SlotDecision::Defer(m),
        (_, Some(m), Some(f)) => match plan {
            SurfacePlan::Deferred(_) => SlotDecision::Defer(m),
            _ => SlotDecision::Draw(m, f),
        },
        (_, _, _) => SlotDecision::Skip,
    }
}

/// Whether the table renderer can present a window set whose surfaces got
/// `plans`: at least one surface is drawable ([`SurfacePlan::Direct`]) and
/// no playfield surface is [`SurfacePlan::Unsupported`] (unsupported
/// secondary surfaces are skipped and [`SurfacePlan::Deferred`] ones are
/// not drawn; neither rejects the set).
pub fn table_set_accepted(plans: &[(SurfaceRole, SurfacePlan)]) -> bool {
    plans
        .iter()
        .any(|(_, p)| matches!(p, SurfacePlan::Direct(_)))
        && !plans
            .iter()
            .any(|(r, p)| *r == SurfaceRole::Playfield && *p == SurfacePlan::Unsupported)
}

/// Polls a future that wgpu resolves synchronously on native backends
/// (error scopes), yielding between polls; `None` if it never resolves.
fn resolve<F: core::future::Future>(f: F) -> Option<F::Output> {
    let mut f = core::pin::pin!(f);
    let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
    for _ in 0..10_000 {
        if let core::task::Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return Some(v);
        }
        std::thread::yield_now();
    }
    None
}

/// Configures `surface`, capturing validation errors instead of letting
/// wgpu's default handler panic. `false` on failure.
fn configure(
    device: &wgpu::Device,
    surface: &wgpu::Surface<'_>,
    config: &wgpu::SurfaceConfiguration,
) -> bool {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    surface.configure(device, config);
    matches!(resolve(scope.pop()), Some(None))
}

struct Slot {
    window: usize,
    role: SurfaceRole,
    planned: Vec<(SurfaceRole, Rect)>,
    planned_size: (u32, u32),
    pixel_size: (u32, u32),
    minimized: bool,
    needs_configure: bool,
    config: wgpu::SurfaceConfiguration,
    routes: Vec<DisplayRoute>,
    renderer: TableRenderer,
    surface: wgpu::Surface<'static>,
    /// Dropped after `surface` (field order).
    _keep_alive: Option<WindowGuard>,
}

/// The table's surfaces, presented directly from the render thread (only
/// those [`slot_decision`] says to draw; see the [module docs](self)).
pub struct AppSurfaces {
    device: wgpu::Device,
    queue: wgpu::Queue,
    slots: Vec<Slot>,
    frames: u64,
    deferred: usize,
    skipped: usize,
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
    /// [`SlotDecision::Draw`] and builds its renderer; a
    /// [`SlotDecision::Defer`] (blocking-mode backglass / score) or
    /// [`SlotDecision::Skip`] (no 8-bit format) source gets one note on
    /// stderr and no slot, so it is never acquired or presented.
    ///
    /// # Errors
    /// An unsupported playfield surface (the caller falls back to the debug
    /// view, see [`table_set_accepted`]), a surface that cannot be
    /// configured, or a renderer that cannot be built.
    pub fn new(
        ctx: &GpuContext,
        sources: Vec<SurfaceSource>,
        opts: SurfaceOptions,
        visuals: &TableVisuals,
        render: TableRenderOptions,
    ) -> Result<AppSurfaces, String> {
        let device = ctx.device().clone();
        let mut slots = Vec::with_capacity(sources.len());
        let mut deferred = 0;
        let mut skipped = 0;
        for src in sources {
            let role = src.role();
            let caps = src.surface.get_capabilities(ctx.adapter());
            let plan = plan_surface(role, &caps.formats, &caps.present_modes, opts);
            let (mode, format) = match slot_decision(role, plan, pick_format(&caps.formats)) {
                SlotDecision::Draw(m, f) => (m, f),
                SlotDecision::Reject => {
                    return Err(format!(
                        "window {} ({role:?}): no 8-bit surface format",
                        src.window
                    ));
                }
                SlotDecision::Defer(m) => {
                    eprintln!(
                        "pin_app: window {} ({role:?}) offers only blocking present modes \
                         ({:?}, would use {m:?}); not drawn on the table path until a \
                         presenter-thread hand-off exists (DECISIONS \"6.0 — secondary \
                         Fifo-only surfaces, round 3\")",
                        src.window, caps.present_modes
                    );
                    deferred += 1;
                    continue;
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
            let (w, h) = (src.pixel_size.0.max(1), src.pixel_size.1.max(1));
            let config = wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                format,
                color_space: Default::default(),
                width: w,
                height: h,
                present_mode: mode,
                desired_maximum_frame_latency: 2,
                alpha_mode: caps
                    .alpha_modes
                    .first()
                    .copied()
                    .unwrap_or(wgpu::CompositeAlphaMode::Auto),
                view_formats: Vec::new(),
            };
            if !configure(&device, &src.surface, &config) {
                return Err(format!(
                    "window {} ({role:?}): configure failed",
                    src.window
                ));
            }
            let renderer = TableRenderer::new(ctx, format, (w, h), visuals, render)
                .map_err(|e| format!("window {} ({role:?}): {e}", src.window))?;
            let planned: Vec<(SurfaceRole, Rect)> = src.viewports.iter().copied().collect();
            let routes = routes_for_window(src.window, &planned, src.logical_size, (w, h));
            slots.push(Slot {
                window: src.window,
                role,
                planned,
                planned_size: src.logical_size,
                pixel_size: src.pixel_size,
                minimized: false,
                needs_configure: false,
                config,
                routes,
                renderer,
                surface: src.surface,
                _keep_alive: src.keep_alive,
            });
        }
        let out = AppSurfaces {
            device,
            queue: ctx.queue().clone(),
            slots,
            frames: 0,
            deferred,
            skipped,
        };
        debug_assert!(
            out.drawn_modes()
                .all(|(r, m)| r == SurfaceRole::Playfield || !is_blocking_mode(m)),
            "a backglass / score surface with a blocking present mode got a slot"
        );
        Ok(out)
    }

    /// `(drawn, deferred, skipped)`: surfaces drawn every iteration,
    /// blocking-mode backglass / score surfaces not drawn
    /// ([`SlotDecision::Defer`]) and surfaces without an 8-bit format
    /// ([`SlotDecision::Skip`]).
    pub fn plan_counts(&self) -> (usize, usize, usize) {
        (self.slots.len(), self.deferred, self.skipped)
    }

    /// The role and configured present mode of every drawn surface, in
    /// slot order (the surfaces the render thread acquires and presents).
    pub fn drawn_modes(&self) -> impl Iterator<Item = (SurfaceRole, wgpu::PresentMode)> + '_ {
        self.slots.iter().map(|s| (s.role, s.config.present_mode))
    }

    /// Number of drawn surfaces.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// `true` without drawn surfaces.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Frames presented on any surface.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Hands a display command to every drawn window's renderer.
    pub fn apply_display_command(&mut self, cmd: DisplayCommand, now_ms: u64) {
        for s in &mut self.slots {
            s.renderer.apply_display_command(cmd, now_ms);
        }
    }

    /// Applies a window event from the main thread (resize, minimise, ...).
    pub fn handle_window_event(&mut self, ev: &WindowEvent) {
        let Some(s) = self.slots.iter_mut().find(|s| s.window == ev.window) else {
            return;
        };
        match ev.kind {
            WindowEventKind::PixelSizeChanged { w, h } => {
                s.pixel_size = (w, h);
                if w > 0 && h > 0 {
                    s.config.width = w;
                    s.config.height = h;
                    s.needs_configure = !configure(&self.device, &s.surface, &s.config);
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

    /// Draws and presents one frame of `snap` on every drawn surface.
    /// Returns each drawn surface's status in slot order (deferred and
    /// skipped surfaces have no slot).
    pub fn render(&mut self, snap: &FrameStateSnapshot) -> Vec<SurfaceStatus> {
        let mut out = Vec::with_capacity(self.slots.len());
        for s in &mut self.slots {
            out.push(Self::render_slot(&self.device, &self.queue, s, snap));
        }
        if out.contains(&SurfaceStatus::Presented) {
            self.frames += 1;
        }
        out
    }

    /// `true` when a playfield surface presented (its `Fifo` acquire paced
    /// the frame).
    pub fn frame_paced(&self, statuses: &[SurfaceStatus]) -> bool {
        pin_render::gpu::paced_by(self.slots.iter().map(|s| s.role), statuses)
    }

    fn render_slot(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        s: &mut Slot,
        snap: &FrameStateSnapshot,
    ) -> SurfaceStatus {
        if s.minimized || s.pixel_size.0 == 0 || s.pixel_size.1 == 0 {
            return SurfaceStatus::Hidden;
        }
        if s.needs_configure {
            if !configure(device, &s.surface, &s.config) {
                return SurfaceStatus::Skipped;
            }
            s.needs_configure = false;
        }
        let (frame, suboptimal) = match s.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) => (t, false),
            wgpu::CurrentSurfaceTexture::Suboptimal(t) => (t, true),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                s.needs_configure = !configure(device, &s.surface, &s.config);
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
        let playfield = s.role == SurfaceRole::Playfield;
        if !playfield {
            // Backglass / score window: clear, then its displays.
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("secondary clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
        s.renderer.encode(
            queue,
            &mut encoder,
            &view,
            playfield,
            s.window,
            &s.routes,
            snap,
        );
        queue.submit([encoder.finish()]);
        queue.present(frame);
        if suboptimal {
            s.needs_configure = !configure(device, &s.surface, &s.config);
        }
        SurfaceStatus::Presented
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
            SurfacePlan::Deferred(M::Fifo)
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
            // Forcing a presenter thread does not apply on the table path:
            // a Mailbox present on the render thread never blocks.
            SurfacePlan::Direct(M::Mailbox)
        );
        assert_eq!(
            plan_surface(score, &[F::Bgra8Unorm], &[M::Fifo], opts(false, true)),
            SurfacePlan::Deferred(M::Fifo)
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
                        } else if is_blocking_mode(choice.mode()) {
                            SurfacePlan::Deferred(choice.mode())
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
                            assert_eq!(p, SurfacePlan::Deferred(M::Fifo));
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
            score(P::Deferred(M::Fifo)),
            bg(P::Deferred(M::FifoRelaxed)),
        ]));
        // Deferred surfaces alone are nothing to draw.
        assert!(!table_set_accepted(&[score(P::Deferred(M::Fifo))]));
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

    /// PR #21 round 3 (thread 4577325408): the render thread never gets a
    /// slot for a backglass / score surface with a blocking mode.
    #[test]
    fn only_non_blocking_secondary_surfaces_are_drawn() {
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
                                assert_eq!(Some(f), pick_format(formats), "{ctx}");
                            }
                            SlotDecision::Defer(m) => {
                                assert!(is_blocking_mode(m), "{ctx}: deferred {m:?}")
                            }
                            other => panic!("{ctx}: {other:?}"),
                        }
                        let offers_non_blocking = modes.iter().any(|m| !is_blocking_mode(*m));
                        assert_eq!(
                            matches!(d, SlotDecision::Draw(..)),
                            offers_non_blocking,
                            "{ctx}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn slot_decision_defers_blocking_secondary_whatever_the_plan_says() {
        let f = Some(F::Bgra8Unorm);
        for role in [SurfaceRole::Backglass, SurfaceRole::Score] {
            for m in [M::Fifo, M::FifoRelaxed, M::AutoVsync] {
                assert_eq!(
                    slot_decision(role, SurfacePlan::Direct(m), f),
                    SlotDecision::Defer(m)
                );
                assert_eq!(
                    slot_decision(role, SurfacePlan::Deferred(m), f),
                    SlotDecision::Defer(m)
                );
            }
            assert_eq!(
                slot_decision(role, SurfacePlan::Direct(M::Mailbox), f),
                SlotDecision::Draw(M::Mailbox, F::Bgra8Unorm)
            );
            assert_eq!(
                slot_decision(role, SurfacePlan::Direct(M::Mailbox), None),
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
        assert_eq!(
            slot_decision(pf, SurfacePlan::Unsupported, None),
            SlotDecision::Reject
        );
        assert_eq!(
            slot_decision(pf, SurfacePlan::Direct(M::Fifo), None),
            SlotDecision::Reject
        );
    }
}
