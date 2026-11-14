//! Menu overlay painting (egui via egui-wgpu).
//!
//! Owner: Subphase 4.4. Stub created by Subphase 1.1.
//!
//! The menu logic and widgets live in `pin_app::ui` (GPU-free). This module
//! holds what both sides share and the GPU part:
//!
//! * the overlay layout: [`menu_panel_rect`] places the menu panel in the
//!   centre of the screen, and [`show_overlay`] draws it on a semi-opaque
//!   backdrop ([`BACKDROP`]) so it stays legible over a bright playfield, while
//!   everything outside the panel is left untouched;
//! * [`raw_input`]: an egui frame input for a surface of a given pixel size and
//!   DPI scale (`pixels_per_point` = the display's content scale), so the menu
//!   scales with display DPI;
//! * [`OverlayPainter`]: the egui-wgpu renderer that draws one tessellated
//!   frame ([`OverlayFrame`]) onto a given texture view, loading (not
//!   clearing) what is already there, so it composites over the rendered
//!   playfield;
//! * [`paint_offscreen`]: paints a frame over a solid background into an
//!   offscreen texture and reads it back (tests).
//!
//! The painter runs on the render thread (it owns wgpu, PLAN §2.2).

use crate::gpu::{GpuContext, GpuError};
pub use egui;
pub use egui_wgpu;

/// Backdrop of the menu panel: near-black, 88 % opaque (unmultiplied).
pub const BACKDROP: egui::Color32 = egui::Color32::from_rgba_premultiplied(10, 12, 18, 224);

/// Fraction of the screen width the panel uses (before the size caps).
pub const PANEL_WIDTH_FRACTION: f32 = 0.8;
/// Fraction of the screen height the panel uses (before the size caps).
pub const PANEL_HEIGHT_FRACTION: f32 = 0.8;
/// Largest panel width, points.
pub const PANEL_MAX_WIDTH: f32 = 960.0;
/// Largest panel height, points.
pub const PANEL_MAX_HEIGHT: f32 = 720.0;
/// Inner margin of the panel, points.
pub const PANEL_MARGIN: f32 = 12.0;

/// Smallest accepted `pixels_per_point`; smaller (or non-finite) values are
/// replaced by 1.
pub const MIN_PIXELS_PER_POINT: f32 = 0.25;
/// Largest accepted `pixels_per_point`.
pub const MAX_PIXELS_PER_POINT: f32 = 8.0;

/// A sane `pixels_per_point` from a display content scale (non-finite or
/// out-of-range values → 1 or the nearest bound).
pub fn sanitize_pixels_per_point(scale: f32) -> f32 {
    if scale.is_finite() && scale > 0.0 {
        scale.clamp(MIN_PIXELS_PER_POINT, MAX_PIXELS_PER_POINT)
    } else {
        1.0
    }
}

/// The menu panel: centred, [`PANEL_WIDTH_FRACTION`] × [`PANEL_HEIGHT_FRACTION`]
/// of `screen`, capped at [`PANEL_MAX_WIDTH`] × [`PANEL_MAX_HEIGHT`] points.
pub fn menu_panel_rect(screen: egui::Rect) -> egui::Rect {
    let w = (screen.width() * PANEL_WIDTH_FRACTION).min(PANEL_MAX_WIDTH);
    let h = (screen.height() * PANEL_HEIGHT_FRACTION).min(PANEL_MAX_HEIGHT);
    egui::Rect::from_center_size(screen.center(), egui::vec2(w, h))
}

/// The menu panel in physical pixels for a surface of `size_px` at
/// `pixels_per_point` (rounded outwards to whole pixels): `[x0, y0, x1, y1]`,
/// exclusive max.
pub fn menu_panel_rect_px(size_px: [u32; 2], pixels_per_point: f32) -> [u32; 4] {
    let ppp = sanitize_pixels_per_point(pixels_per_point);
    let screen = egui::Rect::from_min_size(
        egui::Pos2::ZERO,
        egui::vec2(size_px[0] as f32 / ppp, size_px[1] as f32 / ppp),
    );
    let r = menu_panel_rect(screen);
    let clamp = |v: f32, max: u32| (v.max(0.0) as u32).min(max);
    [
        clamp((r.min.x * ppp).floor(), size_px[0]),
        clamp((r.min.y * ppp).floor(), size_px[1]),
        clamp((r.max.x * ppp).ceil(), size_px[0]),
        clamp((r.max.y * ppp).ceil(), size_px[1]),
    ]
}

/// egui input for one frame on a surface of `size_px` physical pixels at
/// `pixels_per_point` (the display content scale), with `events` (pointer,
/// keys, text) and the frame time in seconds (`None` = egui's default step).
pub fn raw_input(
    size_px: [u32; 2],
    pixels_per_point: f32,
    events: Vec<egui::Event>,
    time: Option<f64>,
) -> egui::RawInput {
    let ppp = sanitize_pixels_per_point(pixels_per_point);
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(size_px[0] as f32 / ppp, size_px[1] as f32 / ppp),
        )),
        events,
        time,
        ..Default::default()
    };
    if let Some(vp) = input.viewports.get_mut(&input.viewport_id) {
        vp.native_pixels_per_point = Some(ppp);
    }
    input
}

/// Visual style of the overlay: dark theme, text a little larger than egui's
/// default for reading from a cabinet distance.
pub fn overlay_style(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::dark());
    ctx.global_style_mut(|style| {
        for font in style.text_styles.values_mut() {
            font.size *= 1.15;
        }
    });
}

/// Draws the menu panel (backdrop + contents) into `ui`, the root [`egui::Ui`]
/// of the pass (the one `Context::run_ui` hands out). Nothing is drawn outside
/// [`menu_panel_rect`] of the screen. The panel is laid out directly (no
/// `Area`), so it is visible on the first frame and never fades in: a frame is
/// a pure function of the menu state and the input.
pub fn show_overlay<R>(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let rect = menu_panel_rect(ui.ctx().content_rect());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::same(6), BACKDROP);
    let inner = rect.shrink(PANEL_MARGIN);
    ui.scope_builder(
        egui::UiBuilder::new()
            .id_salt("pin_menu_overlay")
            .max_rect(inner),
        |ui| {
            ui.set_clip_rect(inner);
            add_contents(ui)
        },
    )
    .inner
}

/// One tessellated egui frame, ready to paint.
pub struct OverlayFrame {
    /// Clipped triangle meshes.
    pub primitives: Vec<egui::ClippedPrimitive>,
    /// Texture uploads/frees to apply before painting.
    pub textures_delta: egui::TexturesDelta,
    /// Scale the frame was laid out at.
    pub pixels_per_point: f32,
}

impl OverlayFrame {
    /// Tessellates `output` (from `ctx.run_ui` / `run_pass`).
    pub fn new(ctx: &egui::Context, output: egui::FullOutput) -> OverlayFrame {
        let ppp = output.pixels_per_point;
        let primitives = ctx.tessellate(output.shapes, ppp);
        OverlayFrame {
            primitives,
            textures_delta: output.textures_delta,
            pixels_per_point: ppp,
        }
    }

    /// Whether the frame draws anything.
    pub fn is_empty(&self) -> bool {
        self.primitives.is_empty()
    }

    /// Drops the frame without painting. Its texture deltas are discarded,
    /// so a painter that never saw them lacks those textures (egui's font
    /// atlas is sent once): discard only frames of a context that is not
    /// painted by an [`OverlayPainter`] later, or tests. To skip a frame
    /// of a painted context, hand it to [`OverlayPainter::defer`] instead.
    pub fn discard(self) {}
}

impl Drop for OverlayFrame {
    fn drop(&mut self) {
        // epaint panics when a TexturesDelta with pending changes is dropped.
        self.textures_delta.clear();
    }
}

/// Draws egui frames onto a colour target of one format.
pub struct OverlayPainter {
    renderer: egui_wgpu::Renderer,
    format: wgpu::TextureFormat,
    /// Texture deltas of frames handed to [`defer`](Self::defer), applied
    /// before the next painted frame's own.
    pending: egui::TexturesDelta,
}

impl OverlayPainter {
    /// A painter for targets of `format` (preferably a non-sRGB
    /// `Rgba8Unorm` / `Bgra8Unorm`, as egui blends in gamma space).
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> OverlayPainter {
        let options = egui_wgpu::RendererOptions {
            dithering: false,
            ..Default::default()
        };
        OverlayPainter {
            renderer: egui_wgpu::Renderer::new(device, format, options),
            format,
            pending: egui::TexturesDelta::default(),
        }
    }

    /// Keeps the texture deltas of `frame`, which is not painted (its
    /// meshes are dropped), and applies them with the next painted frame,
    /// so a skipped frame never loses a texture upload such as egui's font
    /// atlas.
    pub fn defer(&mut self, frame: OverlayFrame) {
        let mut frame = frame;
        self.pending
            .append(std::mem::take(&mut frame.textures_delta));
    }

    /// The target format.
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// Records the paint of `frame` onto `target` (of `size_px`) into
    /// `encoder`: applies the texture deltas, uploads buffers, and draws in a
    /// render pass that loads the existing contents. Returns the extra
    /// command buffers egui callbacks produced (none for the menu); submit
    /// them before `encoder`.
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size_px: [u32; 2],
        frame: OverlayFrame,
    ) -> Vec<wgpu::CommandBuffer> {
        let mut frame = frame;
        let primitives = std::mem::take(&mut frame.primitives);
        // Deltas of deferred frames first, then this frame's (newer).
        let mut textures_delta = std::mem::take(&mut self.pending);
        textures_delta.append(std::mem::take(&mut frame.textures_delta));
        let pixels_per_point = frame.pixels_per_point;
        drop(frame);
        for (id, deltas) in &textures_delta.set {
            for delta in deltas {
                self.renderer.update_texture(device, queue, *id, delta);
            }
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size_px,
            pixels_per_point: sanitize_pixels_per_point(pixels_per_point),
        };
        let extra = self
            .renderer
            .update_buffers(device, queue, encoder, &primitives, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("menu overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, &primitives, &screen);
        }
        for id in &textures_delta.free {
            self.renderer.free_texture(id);
        }
        textures_delta.clear();
        extra
    }
}

impl Drop for OverlayPainter {
    fn drop(&mut self) {
        // epaint panics when a TexturesDelta with pending changes is dropped.
        self.pending.clear();
    }
}

/// Paints `frame` over a solid `background` (linear 0..1 RGBA written as-is
/// into `Rgba8Unorm`) in a `width` × `height` offscreen texture and returns
/// the tightly packed RGBA8 pixels. Used by the GPU tests.
///
/// Uses a fresh [`OverlayPainter`], so it only suits the *first* frame of
/// an `egui::Context` (later frames do not resend the font atlas); paint a
/// sequence of frames with [`paint_offscreen_with`] and one painter.
pub fn paint_offscreen(
    ctx: &GpuContext,
    width: u32,
    height: u32,
    background: [f32; 4],
    frame: OverlayFrame,
) -> Result<Vec<u8>, GpuError> {
    let mut painter = OverlayPainter::new(ctx.device(), wgpu::TextureFormat::Rgba8Unorm);
    paint_offscreen_with(ctx, &mut painter, width, height, background, frame)
}

/// [`paint_offscreen`] with a caller-kept `painter` (of format
/// `Rgba8Unorm`), which holds the textures of earlier frames: use the same
/// painter for every frame of one `egui::Context`.
///
/// A rejected request (zero or oversized `width`/`height`, a painter of
/// another format) paints nothing and returns an error, but `painter` keeps
/// the frame's texture deltas ([`OverlayPainter::defer`]), so a later valid
/// paint of the same context still has e.g. the font atlas.
pub fn paint_offscreen_with(
    ctx: &GpuContext,
    painter: &mut OverlayPainter,
    width: u32,
    height: u32,
    background: [f32; 4],
    frame: OverlayFrame,
) -> Result<Vec<u8>, GpuError> {
    let max = ctx.device().limits().max_texture_dimension_2d;
    if width == 0 || height == 0 || width > max || height > max {
        painter.defer(frame);
        return Err(GpuError::InvalidArgument(format!(
            "image size {width}x{height} (each side must be 1..={max})"
        )));
    }
    let device = ctx.device();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    if painter.format() != format {
        painter.defer(frame);
        return Err(GpuError::InvalidArgument(format!(
            "offscreen painter format {:?} (must be {format:?})",
            painter.format()
        )));
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("menu overlay offscreen"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("menu overlay offscreen"),
    });
    {
        let [r, g, b, a] = background.map(f64::from);
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("menu overlay background"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    let extra = painter.paint(
        device,
        ctx.queue(),
        &mut encoder,
        &view,
        [width, height],
        frame,
    );
    if !extra.is_empty() {
        ctx.queue().submit(extra);
    }
    read_rgba8(ctx, encoder, &texture)
}

/// Submits `encoder` after copying the RGBA8 `texture` into a buffer, and
/// returns its pixels tightly packed.
fn read_rgba8(
    ctx: &GpuContext,
    mut encoder: wgpu::CommandEncoder,
    texture: &wgpu::Texture,
) -> Result<Vec<u8>, GpuError> {
    let device = ctx.device();
    let size = texture.size();
    let (width, height) = (size.width, size.height);
    let row = width as usize * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded = row.div_ceil(align) * align;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("menu overlay readback"),
        size: crate::gpu::readback_buffer_size(device, padded, height)?,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded as u32),
                rows_per_image: Some(height),
            },
        },
        size,
    );
    ctx.queue().submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    crate::gpu::finish_map(device, &rx, crate::gpu::READBACK_WAIT_TIMEOUT)?;
    let mut out = Vec::with_capacity(row * height as usize);
    {
        let mapped = readback
            .get_mapped_range(..)
            .map_err(|e| GpuError::Readback(format!("get_mapped_range: {e:?}")))?;
        for y in 0..height as usize {
            out.extend_from_slice(&mapped[y * padded..y * padded + row]);
        }
    }
    readback.unmap();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_is_centred_and_capped() {
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 600.0));
        let r = menu_panel_rect(screen);
        assert_eq!(r.center(), screen.center());
        assert_eq!(r.width(), 640.0);
        assert_eq!(r.height(), 480.0);
        let big = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(4000.0, 3000.0));
        let r = menu_panel_rect(big);
        assert_eq!((r.width(), r.height()), (PANEL_MAX_WIDTH, PANEL_MAX_HEIGHT));
    }

    #[test]
    fn panel_pixels_scale_with_dpi() {
        // 1600x1200 px at 2x = 800x600 points: same proportions as at 1x.
        let a = menu_panel_rect_px([800, 600], 1.0);
        let b = menu_panel_rect_px([1600, 1200], 2.0);
        assert_eq!(a, [80, 60, 720, 540]);
        assert_eq!(b, [160, 120, 1440, 1080]);
        // The cap is in points, so it doubles in pixels at 2x.
        let c = menu_panel_rect_px([4000, 3000], 2.0);
        assert_eq!(c[2] - c[0], 1920);
    }

    #[test]
    fn pixels_per_point_is_sanitized() {
        assert_eq!(sanitize_pixels_per_point(f32::NAN), 1.0);
        assert_eq!(sanitize_pixels_per_point(-2.0), 1.0);
        assert_eq!(sanitize_pixels_per_point(0.01), MIN_PIXELS_PER_POINT);
        assert_eq!(sanitize_pixels_per_point(100.0), MAX_PIXELS_PER_POINT);
        assert_eq!(sanitize_pixels_per_point(1.5), 1.5);
    }

    #[test]
    fn overlay_tessellates_inside_the_panel_only() {
        let ctx = egui::Context::default();
        overlay_style(&ctx);
        let out = ctx.run_ui(raw_input([800, 600], 1.0, Vec::new(), None), |ui| {
            show_overlay(ui, |ui| ui.label("Hello"));
        });
        assert_eq!(out.pixels_per_point, 1.0);
        let frame = OverlayFrame::new(&ctx, out);
        assert!(!frame.is_empty());
        let panel = menu_panel_rect(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 600.0),
        ));
        for p in &frame.primitives {
            if let egui::epaint::Primitive::Mesh(m) = &p.primitive {
                for v in &m.vertices {
                    assert!(
                        panel.expand(1.0).contains(v.pos),
                        "vertex {:?} outside the panel {panel:?}",
                        v.pos
                    );
                }
            }
        }
        frame.discard();
    }

    #[test]
    fn raw_input_sets_dpi() {
        let ctx = egui::Context::default();
        let out = ctx.run_ui(raw_input([1600, 1200], 2.0, Vec::new(), None), |ui| {
            show_overlay(ui, |ui| ui.label("x"));
        });
        assert_eq!(out.pixels_per_point, 2.0);
        assert_eq!(ctx.content_rect().width(), 800.0);
        out.drop_without_applying_deltas();
    }
}
