//! The profiler HUD overlay (`pin_app --profiler-hud`, 7.1).
//!
//! * [`HudFeed`] (GPU-free): turns a [`ProfilerReader`] into the HUD text,
//!   [`hud_lines`] of a fresh [`ProfilerSnapshot`](crate::profiler::ProfilerSnapshot),
//!   at most once per refresh period. Render thread only (it allocates).
//! * [`HudOverlay`]: lays the lines out with egui (once per refresh, not
//!   per frame) and paints the cached meshes over the playfield with
//!   `pin_render`'s [`OverlayPainter`], loading (not clearing) the frame,
//!   top-left on a dark backdrop. Built only when the HUD is enabled
//!   ([`TableRenderOptions::profiler_hud`](super::TableRenderOptions::profiler_hud)),
//!   so frames without it are unchanged.
//! * [`DebugViewHooks`] (w8g-d): the same GPU frame timing
//!   ([`GpuFrameTimer`]) and HUD for a run that draws pin_render's
//!   `SurfaceSet` debug view (no table, or a playfield surface without an
//!   8-bit format), through `SurfaceSet::render_with`.

use super::gpu_timer::GpuFrameTimer;
use crate::profiler::{ProfilerReader, hud_lines};
use arrayvec::ArrayVec;
use pin_render::gpu::{SurfaceFrameHooks, wgpu};
use pin_render::topology::SurfaceRole;
use pin_render::ui::{OverlayFrame, OverlayPainter, egui, raw_input};

/// The HUD text of `reader`, refreshed at most once per `period_ns`.
#[derive(Debug)]
pub struct HudFeed {
    reader: ProfilerReader,
    period_ns: u64,
    last_ns: Option<u64>,
}

impl HudFeed {
    /// A feed of `reader` refreshed every `period_ns` (the render thread
    /// uses [`HUD_REFRESH_NS`](super::threads::HUD_REFRESH_NS)).
    #[must_use]
    pub fn new(reader: ProfilerReader, period_ns: u64) -> HudFeed {
        HudFeed {
            reader,
            period_ns,
            last_ns: None,
        }
    }

    /// At `now_ns` (any monotonic clock): `Some(hud_lines(&snapshot))` on
    /// the first call and whenever `period_ns` passed since the last
    /// refresh, else `None` (keep showing the previous lines).
    pub fn refresh(&mut self, now_ns: u64) -> Option<Vec<String>> {
        if let Some(last) = self.last_ns
            && now_ns.saturating_sub(last) < self.period_ns
        {
            return None;
        }
        self.last_ns = Some(now_ns);
        Some(hud_lines(&self.reader.snapshot()))
    }
}

/// HUD text size (points; the HUD lays out at 1 pixel per point).
const HUD_FONT_SIZE: f32 = 13.0;
/// Distance of the backdrop from the top-left corner and of the text from
/// the backdrop's edge, pixels.
const HUD_MARGIN: f32 = 6.0;
/// Text colour.
const HUD_TEXT: egui::Color32 = egui::Color32::from_rgb(235, 235, 210);
/// Backdrop: black at about 70 % opacity, so the text stays legible over a
/// bright playfield.
const HUD_BACKDROP: egui::Color32 = egui::Color32::from_black_alpha(180);

/// The profiler HUD painted over a playfield target of one format.
pub struct HudOverlay {
    ctx: egui::Context,
    painter: OverlayPainter,
    device: wgpu::Device,
    lines: Vec<String>,
    /// Target size the cached meshes were laid out for.
    laid_out: Option<(u32, u32)>,
    primitives: Vec<egui::ClippedPrimitive>,
    /// Texture deltas of layouts not painted yet (the font atlas first).
    textures: egui::TexturesDelta,
    rect_px: [u32; 4],
}

impl std::fmt::Debug for HudOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HudOverlay")
            .field("lines", &self.lines)
            .field("rect_px", &self.rect_px)
            .finish_non_exhaustive()
    }
}

impl HudOverlay {
    /// A HUD for targets of `format`, with no text yet (paints nothing).
    #[must_use]
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> HudOverlay {
        HudOverlay {
            ctx: egui::Context::default(),
            painter: OverlayPainter::new(device, format),
            device: device.clone(),
            lines: Vec::new(),
            laid_out: None,
            primitives: Vec::new(),
            textures: egui::TexturesDelta::default(),
            rect_px: [0; 4],
        }
    }

    /// The lines shown.
    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// The backdrop rectangle of the last layout, pixels `[x0, y0, x1,
    /// y1]` (exclusive end, rounded outwards); all zero before one. The
    /// HUD paints nothing outside it.
    #[must_use]
    pub fn rect_px(&self) -> [u32; 4] {
        self.rect_px
    }

    /// Shows `lines` from the next paint (laid out then, once).
    pub fn set_lines(&mut self, lines: &[String]) {
        if self.lines != lines {
            self.lines = lines.to_vec();
            self.laid_out = None;
        }
    }

    fn layout(&mut self, size: (u32, u32)) {
        let raw = raw_input([size.0, size.1], 1.0, Vec::new(), None);
        let lines = &self.lines;
        let mut rect = egui::Rect::NOTHING;
        let out = self.ctx.run_ui(raw, |ui| {
            let painter = ui.painter();
            let font = egui::FontId::monospace(HUD_FONT_SIZE);
            let galleys: Vec<_> = lines
                .iter()
                .map(|l| painter.layout_no_wrap(l.clone(), font.clone(), HUD_TEXT))
                .collect();
            let w = galleys.iter().map(|g| g.size().x).fold(0.0f32, f32::max);
            let h: f32 = galleys.iter().map(|g| g.size().y).sum();
            let origin = egui::pos2(HUD_MARGIN, HUD_MARGIN);
            rect = egui::Rect::from_min_size(
                origin,
                egui::vec2(w + 2.0 * HUD_MARGIN, h + 2.0 * HUD_MARGIN),
            );
            painter.rect_filled(rect, egui::CornerRadius::same(3), HUD_BACKDROP);
            let mut y = origin.y + HUD_MARGIN;
            for g in galleys {
                let dy = g.size().y;
                painter.galley(egui::pos2(origin.x + HUD_MARGIN, y), g, HUD_TEXT);
                y += dy;
            }
        });
        let frame = OverlayFrame::new(&self.ctx, out);
        let mut frame = frame;
        self.primitives = std::mem::take(&mut frame.primitives);
        self.textures
            .append(std::mem::take(&mut frame.textures_delta));
        let px = |v: f32, max: u32, up: bool| {
            let v = if up { v.ceil() } else { v.floor() };
            (v.max(0.0) as u32).min(max)
        };
        self.rect_px = [
            px(rect.min.x, size.0, false),
            px(rect.min.y, size.1, false),
            px(rect.max.x, size.0, true),
            px(rect.max.y, size.1, true),
        ];
        self.laid_out = Some(size);
    }

    /// Records the HUD over `target` (`size` pixels, this HUD's format)
    /// into `encoder`, laying the text out first when it or the size
    /// changed. Paints nothing without lines. Submit `encoder` before the
    /// next paint.
    pub fn paint(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
    ) {
        if self.lines.is_empty() || size.0 == 0 || size.1 == 0 {
            return;
        }
        if self.laid_out != Some(size) {
            self.layout(size);
        }
        let frame = OverlayFrame {
            primitives: self.primitives.clone(),
            textures_delta: std::mem::take(&mut self.textures),
            pixels_per_point: 1.0,
        };
        // Text and a filled rect use no egui paint callbacks, so there
        // are no extra command buffers to submit.
        let extra = self.painter.paint(
            &self.device,
            queue,
            encoder,
            target,
            [size.0, size.1],
            frame,
        );
        debug_assert!(extra.is_empty(), "HUD paint produced callback commands");
    }
}

impl Drop for HudOverlay {
    fn drop(&mut self) {
        // epaint panics when a TexturesDelta with pending changes is dropped.
        self.textures.clear();
    }
}

/// `true` when the HUD's [`OverlayPainter`] can draw onto a target of a
/// format with these adapter `features` (pure): its egui pipeline renders
/// into the target and alpha-blends over it, so the format must be a float
/// (normalised or floating-point) colour format allowing
/// `RENDER_ATTACHMENT` and blending. Every 8-bit colour format qualifies;
/// so do e.g. `Rgb10a2Unorm` and `Rgba16Float` where the adapter blends
/// them. Depth / stencil and integer formats never do.
#[must_use]
pub fn hud_format_supported(
    format: wgpu::TextureFormat,
    features: wgpu::TextureFormatFeatures,
) -> bool {
    !format.is_depth_stencil_format()
        && matches!(
            format.sample_type(None, None),
            Some(wgpu::TextureSampleType::Float { .. })
        )
        && features
            .allowed_usages
            .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
        && features
            .flags
            .contains(wgpu::TextureFormatFeatureFlags::BLENDABLE)
}

/// The profiler's hooks into the debug view (w8g-d): pin_render's
/// [`SurfaceSet::render_with`](pin_render::gpu::SurfaceSet::render_with)
/// calls them around every debug-view frame, exactly as
/// [`AppSurfaces::render`](super::AppSurfaces::render) times and overlays
/// the table frame:
///
/// * `begin_frame`: [`GpuFrameTimer::collect`] (finished samples, no wait),
///   then [`GpuFrameTimer::begin_frame`];
/// * `wrap_submit`: [`GpuFrameTimer::bracket`] (the surface's frame between
///   its start / end timestamps, in one submit);
/// * `overlay` (playfield only, last in its frame): [`HudOverlay::paint`];
/// * `end_frame`: [`GpuFrameTimer::end_frame`] (resolve and read back).
///
/// Without a timer the submissions go alone; without a HUD nothing is
/// painted. The render thread uses plain `SurfaceSet::render` when it has
/// neither, so default debug frames are unchanged.
#[derive(Default)]
pub struct DebugViewHooks {
    timer: Option<GpuFrameTimer>,
    hud: Option<HudOverlay>,
    hud_paints: u64,
}

impl std::fmt::Debug for DebugViewHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DebugViewHooks")
            .field("timer", &self.timer.is_some())
            .field("hud", &self.hud)
            .field("hud_paints", &self.hud_paints)
            .finish()
    }
}

impl DebugViewHooks {
    /// Hooks timing frames with `timer` (if any) and painting `hud` (if
    /// any; build it for the playfield surface's draw format).
    #[must_use]
    pub fn new(timer: Option<GpuFrameTimer>, hud: Option<HudOverlay>) -> DebugViewHooks {
        DebugViewHooks {
            timer,
            hud,
            hud_paints: 0,
        }
    }

    /// `true` with neither a timer nor a HUD (the hooks would do nothing).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.timer.is_none() && self.hud.is_none()
    }

    /// The GPU frame timer, if any.
    #[must_use]
    pub fn gpu_timer(&self) -> Option<&GpuFrameTimer> {
        self.timer.as_ref()
    }

    /// The HUD, if any.
    #[must_use]
    pub fn hud(&self) -> Option<&HudOverlay> {
        self.hud.as_ref()
    }

    /// Shows `lines` in the HUD from the next frame (ignored without one).
    pub fn set_hud_lines(&mut self, lines: &[String]) {
        if let Some(h) = &mut self.hud {
            h.set_lines(lines);
        }
    }

    /// Playfield frames the HUD was painted on (with text) so far.
    #[must_use]
    pub fn hud_paints(&self) -> u64 {
        self.hud_paints
    }
}

impl SurfaceFrameHooks for DebugViewHooks {
    fn begin_frame(&mut self) {
        if let Some(t) = &mut self.timer {
            t.collect();
            t.begin_frame();
        }
    }

    fn wrap_submit(&mut self, frame: wgpu::CommandBuffer) -> ArrayVec<wgpu::CommandBuffer, 3> {
        match &mut self.timer {
            Some(t) => t.bracket(frame),
            None => {
                let mut one = ArrayVec::new();
                one.push(frame);
                one
            }
        }
    }

    fn overlay(
        &mut self,
        role: SurfaceRole,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size_px: (u32, u32),
    ) {
        if role != SurfaceRole::Playfield {
            return;
        }
        if let Some(h) = &mut self.hud {
            if !h.lines().is_empty() && size_px.0 > 0 && size_px.1 > 0 {
                self.hud_paints += 1;
            }
            h.paint(queue, encoder, view, size_px);
        }
    }

    fn end_frame(&mut self, submit: &mut dyn FnMut(wgpu::CommandBuffer)) {
        if let Some(t) = &mut self.timer {
            t.end_frame(submit);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiler::Profiler;

    fn features(
        usages: wgpu::TextureUsages,
        flags: wgpu::TextureFormatFeatureFlags,
    ) -> wgpu::TextureFormatFeatures {
        wgpu::TextureFormatFeatures {
            allowed_usages: usages,
            flags,
        }
    }

    #[test]
    fn hud_formats_need_a_blendable_float_render_target() {
        use wgpu::TextureFormat as F;
        use wgpu::TextureFormatFeatureFlags as Fl;
        use wgpu::TextureUsages as U;
        let good = features(U::RENDER_ATTACHMENT | U::TEXTURE_BINDING, Fl::BLENDABLE);
        for f in [
            F::Rgba8Unorm,
            F::Bgra8Unorm,
            F::Rgba8UnormSrgb,
            F::Bgra8UnormSrgb,
            F::Rgb10a2Unorm,
            F::Rgba16Float,
        ] {
            assert!(hud_format_supported(f, good), "{f:?}");
            // Not blendable or not renderable on this adapter: no HUD.
            assert!(
                !hud_format_supported(f, features(U::RENDER_ATTACHMENT, Fl::empty())),
                "{f:?}"
            );
            assert!(
                !hud_format_supported(f, features(U::TEXTURE_BINDING, Fl::BLENDABLE)),
                "{f:?}"
            );
        }
        // Integer and depth formats never take the HUD.
        for f in [
            F::Rgba8Uint,
            F::R32Sint,
            F::Depth32Float,
            F::Depth24PlusStencil8,
        ] {
            assert!(!hud_format_supported(f, good), "{f:?}");
        }
    }

    #[test]
    fn empty_debug_view_hooks_pass_through() {
        let h = DebugViewHooks::new(None, None);
        assert!(h.is_empty());
        assert!(h.gpu_timer().is_none() && h.hud().is_none());
        assert_eq!(h.hud_paints(), 0);
        let mut h = DebugViewHooks::default();
        h.set_hud_lines(&["x".to_owned()]);
        h.begin_frame();
        let mut n = 0;
        h.end_frame(&mut |_| n += 1);
        assert_eq!(n, 0);
    }

    #[test]
    fn feed_shows_hud_lines_of_the_snapshot_and_throttles() {
        let mut p = Profiler::new();
        p.sim.record_tick_ns(120_000);
        p.sim.record_input(1_000, 801_000);
        p.sim.set_script_queue_overflows(2);
        p.sim_loop.set_input_overflow_ticks(3);
        p.present.record_frame_ns(4_000_000);
        let mut feed = HudFeed::new(p.reader.clone(), 100);
        let first = feed.refresh(1_000).expect("first call refreshes");
        assert_eq!(first, hud_lines(&p.reader.snapshot()));
        assert_eq!(first.len(), 4);
        assert!(first[2].ends_with("n 1"), "{}", first[2]);
        assert!(
            first[3].starts_with("cmd queue overflows 5"),
            "{}",
            first[3]
        );
        // Within the period: keep the old lines.
        p.sim.record_tick_ns(130_000);
        assert_eq!(feed.refresh(1_099), None);
        // A clock going backwards never refreshes early.
        assert_eq!(feed.refresh(10), None);
        let second = feed.refresh(1_100).expect("period passed");
        assert_eq!(second, hud_lines(&p.reader.snapshot()));
        assert!(second[0].contains("n 2"), "{}", second[0]);
    }
}
