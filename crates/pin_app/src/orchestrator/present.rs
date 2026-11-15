//! Presentation of the table frame from the render thread (Subphase 6.0
//! wiring).
//!
//! The 1.4 [`SurfaceSet`](pin_render::gpu::SurfaceSet) only draws debug
//! lines and has no hook for other content, and `pin_render` internals are
//! not changed in this round. So when a table is loaded, the render thread
//! presents its own frames here: each window's surface gets a
//! [`TableRenderer`] that draws the PBR playfield and the displays routed
//! to that window.
//!
//! The choice is made **per surface** ([`plan_surface`], pure and GPU-free):
//!
//! * [`SurfacePlan::Direct`]: the §2.2 present-mode policy
//!   ([`choose_present_mode`]) gives a direct mode (the playfield always;
//!   a backglass / score window offering `Mailbox` or `Immediate`).
//! * [`SurfacePlan::Secondary`]: a backglass / score window that would need
//!   a presenter thread (it offers neither `Mailbox` nor `Immediate`, D19,
//!   or `force_presenter_thread` is set). Handing table textures to the
//!   `SurfaceSet` presenter threads needs a `pin_render` API that does not
//!   exist yet, so such a surface is drawn from the render thread too, with
//!   the mode the policy picked (`Fifo`), but only every
//!   [`SECONDARY_FRAME_INTERVAL`]th render-loop iteration. Trade-off
//!   (DECISIONS "6.0 — secondary Fifo-only surfaces"): its vsync-bound
//!   acquire can still block the playfield's loop on those iterations, at
//!   most one in [`SECONDARY_FRAME_INTERVAL`], and its displays refresh at
//!   that reduced rate.
//! * [`SurfacePlan::Unsupported`]: no 8-bit colour format. A secondary
//!   surface planned so is skipped (one stderr note); only an unsupported
//!   **playfield** rejects the set ([`table_set_accepted`]), and the render
//!   thread then falls back to the `SurfaceSet` debug view (DECISIONS
//!   "6.0 — wiring completed").
//!
//! A secondary surface never turns the cabinet into the debug view, so the
//! table renderer and the sim's display commands (scenes, scores) stay on
//! whenever the playfield can be drawn.

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

/// A secondary surface ([`SurfacePlan::Secondary`]) is drawn on one
/// render-loop iteration in this many.
pub const SECONDARY_FRAME_INTERVAL: u64 = 4;

/// How one surface is presented (see the [module docs](self)).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SurfacePlan {
    /// Drawn and presented every render-loop iteration with this mode.
    Direct(wgpu::PresentMode),
    /// A backglass / score surface that wants a presenter thread, drawn
    /// from the render thread with this mode every
    /// [`SECONDARY_FRAME_INTERVAL`]th iteration.
    Secondary(wgpu::PresentMode),
    /// No 8-bit colour format: the table renderer cannot draw it.
    Unsupported,
}

/// The plan of a `role` surface offering `formats` and `present_modes`
/// under `opts` (pure; see the [module docs](self)).
pub fn plan_surface(
    role: SurfaceRole,
    formats: &[wgpu::TextureFormat],
    present_modes: &[wgpu::PresentMode],
    opts: SurfaceOptions,
) -> SurfacePlan {
    if pick_format(formats).is_none() {
        return SurfacePlan::Unsupported;
    }
    let choice = choose_present_mode(role, present_modes, opts.vrr);
    let wants_thread = role != SurfaceRole::Playfield
        && (opts.force_presenter_thread || choice.needs_presenter_thread());
    if wants_thread {
        SurfacePlan::Secondary(choice.mode())
    } else {
        SurfacePlan::Direct(choice.mode())
    }
}

/// Whether the table renderer can present a window set whose surfaces got
/// `plans`: at least one surface is drawable and no playfield surface is
/// [`SurfacePlan::Unsupported`] (unsupported secondary surfaces are
/// skipped instead).
pub fn table_set_accepted(plans: &[(SurfaceRole, SurfacePlan)]) -> bool {
    plans.iter().any(|(_, p)| *p != SurfacePlan::Unsupported)
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
    /// [`SurfacePlan::Secondary`]: drawn every
    /// [`SECONDARY_FRAME_INTERVAL`]th iteration.
    secondary: bool,
    renderer: TableRenderer,
    surface: wgpu::Surface<'static>,
    /// Dropped after `surface` (field order).
    _keep_alive: Option<WindowGuard>,
}

/// The table's surfaces, presented directly from the render thread.
pub struct AppSurfaces {
    device: wgpu::Device,
    queue: wgpu::Queue,
    slots: Vec<Slot>,
    frames: u64,
    iteration: u64,
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

    /// Configures every source planned [`SurfacePlan::Direct`] or
    /// [`SurfacePlan::Secondary`] and builds its renderer; an
    /// [`Unsupported`](SurfacePlan::Unsupported) secondary surface is
    /// skipped with a note on stderr.
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
        let mut skipped = 0;
        for src in sources {
            let role = src.role();
            let caps = src.surface.get_capabilities(ctx.adapter());
            let plan = plan_surface(role, &caps.formats, &caps.present_modes, opts);
            let (mode, secondary, format) = match (plan, pick_format(&caps.formats)) {
                (SurfacePlan::Direct(m), Some(f)) => (m, false, f),
                (SurfacePlan::Secondary(m), Some(f)) => (m, true, f),
                _ if role == SurfaceRole::Playfield => {
                    return Err(format!(
                        "window {} ({role:?}): no 8-bit surface format",
                        src.window
                    ));
                }
                _ => {
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
                secondary,
                renderer,
                surface: src.surface,
                _keep_alive: src.keep_alive,
            });
        }
        Ok(AppSurfaces {
            device,
            queue: ctx.queue().clone(),
            slots,
            frames: 0,
            iteration: 0,
            skipped,
        })
    }

    /// Surfaces drawn every iteration, secondary surfaces (reduced
    /// cadence) and skipped (unsupported) surfaces.
    pub fn plan_counts(&self) -> (usize, usize, usize) {
        let secondary = self.slots.iter().filter(|s| s.secondary).count();
        (self.slots.len() - secondary, secondary, self.skipped)
    }

    /// Number of surfaces.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// `true` without surfaces.
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Frames presented on any surface.
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Hands a display command to every window's renderer.
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

    /// Draws and presents one frame of `snap` on every surface (a
    /// secondary surface only every [`SECONDARY_FRAME_INTERVAL`]th call,
    /// [`SurfaceStatus::Unchanged`] otherwise). Returns each drawn
    /// surface's status in slot order (skipped surfaces have no slot).
    pub fn render(&mut self, snap: &FrameStateSnapshot) -> Vec<SurfaceStatus> {
        let mut out = Vec::with_capacity(self.slots.len());
        let secondary_turn = self.iteration.is_multiple_of(SECONDARY_FRAME_INTERVAL);
        self.iteration = self.iteration.wrapping_add(1);
        for s in &mut self.slots {
            if s.secondary && !secondary_turn {
                out.push(SurfaceStatus::Unchanged);
                continue;
            }
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
            // Secondary window: clear, then its displays.
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
    const MODE_SETS: [&[M]; 5] = [
        &[M::Fifo],
        &[M::Fifo, M::Mailbox],
        &[M::Fifo, M::Immediate],
        &[M::Fifo, M::FifoRelaxed],
        &[],
    ];
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
            SurfacePlan::Secondary(M::Fifo)
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
            SurfacePlan::Secondary(M::Mailbox)
        );
        assert_eq!(
            plan_surface(pf, &[F::Rgba16Float], &[M::Fifo], no),
            SurfacePlan::Unsupported
        );
        // The whole sweep: role × modes × formats × VRR × forcing.
        for role in ROLES {
            for modes in MODE_SETS {
                for formats in FORMAT_SETS {
                    for (vrr, force) in [(false, false), (false, true), (true, false), (true, true)]
                    {
                        let p = plan_surface(role, formats, modes, opts(vrr, force));
                        let choice = choose_present_mode(role, modes, vrr);
                        let expect = if !eight_bit(formats) {
                            SurfacePlan::Unsupported
                        } else if role == SurfaceRole::Playfield {
                            // The playfield never wants a presenter thread
                            // (forcing applies to secondary roles only).
                            SurfacePlan::Direct(choice.mode())
                        } else if force || choice.needs_presenter_thread() {
                            SurfacePlan::Secondary(choice.mode())
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
                            assert_eq!(p, SurfacePlan::Secondary(M::Fifo));
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
            score(P::Secondary(M::Fifo)),
            bg(P::Secondary(M::Fifo)),
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
}
