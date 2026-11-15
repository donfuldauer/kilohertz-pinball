//! Direct presentation of the table frame (Subphase 6.0 wiring).
//!
//! The 1.4 [`SurfaceSet`](pin_render::gpu::SurfaceSet) only draws debug
//! lines and has no hook for other content, and `pin_render` internals are
//! not changed in this round. So when a table is loaded, the render thread
//! presents its own frames here: each window's surface is configured with
//! the §2.2 present-mode policy
//! ([`choose_present_mode`](pin_render::gpu::choose_present_mode)) and gets
//! a [`TableRenderer`] that draws the PBR playfield and the displays
//! routed to that window. Only *direct* presentation is supported: when a
//! surface would need a presenter thread (a dedicated backglass / score
//! window offering neither `Mailbox` nor `Immediate`, D19, or
//! `force_presenter_thread`), [`AppSurfaces::supported`] is `false` and the
//! render thread falls back to the `SurfaceSet` debug view (DECISIONS
//! "6.0 — wiring completed").

use super::table::DisplayCommand;
use super::view::{TableRenderOptions, TableRenderer, TableVisuals, routes_for_window};
use pin_core::snapshot::FrameStateSnapshot;
use pin_render::displays::DisplayRoute;
use pin_render::gpu::{
    GpuContext, PresentChoice, SurfaceOptions, SurfaceSource, SurfaceStatus, WindowGuard,
    choose_present_mode, wgpu,
};
use pin_render::topology::{Rect, SurfaceRole, WindowEvent, WindowEventKind};

/// The 8-bit colour format of a surface the table renderer can draw.
fn pick_format(caps: &wgpu::SurfaceCapabilities) -> Option<wgpu::TextureFormat> {
    use wgpu::TextureFormat as F;
    caps.formats
        .iter()
        .copied()
        .find(|f| matches!(f, F::Bgra8Unorm | F::Rgba8Unorm))
        .or_else(|| {
            caps.formats
                .iter()
                .copied()
                .find(|f| matches!(f, F::Bgra8UnormSrgb | F::Rgba8UnormSrgb))
        })
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

/// The table's surfaces, presented directly from the render thread.
pub struct AppSurfaces {
    device: wgpu::Device,
    queue: wgpu::Queue,
    slots: Vec<Slot>,
    frames: u64,
}

impl AppSurfaces {
    /// Whether every source can be presented directly with the table
    /// renderer: an 8-bit colour format is offered and the present policy
    /// gives a direct mode (no presenter thread).
    pub fn supported(ctx: &GpuContext, sources: &[SurfaceSource], opts: SurfaceOptions) -> bool {
        sources.iter().all(|s| {
            let caps = s.surface.get_capabilities(ctx.adapter());
            let role = s.role();
            pick_format(&caps).is_some()
                && !(opts.force_presenter_thread && role != SurfaceRole::Playfield)
                && !choose_present_mode(role, &caps.present_modes, opts.vrr)
                    .needs_presenter_thread()
        })
    }

    /// Configures every source and builds its renderer.
    ///
    /// # Errors
    /// A surface that cannot be configured, or a renderer that cannot be
    /// built.
    pub fn new(
        ctx: &GpuContext,
        sources: Vec<SurfaceSource>,
        opts: SurfaceOptions,
        visuals: &TableVisuals,
        render: TableRenderOptions,
    ) -> Result<AppSurfaces, String> {
        let device = ctx.device().clone();
        let mut slots = Vec::with_capacity(sources.len());
        for src in sources {
            let role = src.role();
            let caps = src.surface.get_capabilities(ctx.adapter());
            let format = pick_format(&caps)
                .ok_or_else(|| format!("window {}: no 8-bit surface format", src.window))?;
            let mode = match choose_present_mode(role, &caps.present_modes, opts.vrr) {
                PresentChoice::Direct(m) => m,
                PresentChoice::PresenterThread(_) => {
                    return Err(format!(
                        "window {} ({role:?}) needs a presenter thread",
                        src.window
                    ));
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
        Ok(AppSurfaces {
            device,
            queue: ctx.queue().clone(),
            slots,
            frames: 0,
        })
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

    /// Draws and presents one frame of `snap` on every surface. Returns
    /// each surface's status in source order.
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
