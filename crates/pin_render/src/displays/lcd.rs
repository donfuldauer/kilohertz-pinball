//! LCD mode: a 16:9 compositor of rectangles, sprites and vector text.

use super::quad::{
    QuadDraw, QuadInstance, QuadRenderer, STROKE_HALF_WIDTH, WHITE, glyph_slot, render_offscreen,
};
use super::routing::fit_aspect;
use super::scene::{LayerKind, Scene};
use super::score::{ScoreState, expand_text, score_view_layout};
use super::{DisplayError, ImageSet, RgbaImage};
use crate::gpu::{GpuContext, GpuError};
use crate::topology::Rect;
use std::collections::BTreeMap;

/// Glyph advance of the vector font, glyph units (5 wide + 1 space).
pub const GLYPH_ADVANCE_UNITS: f32 = 6.0;
/// Stroke half-width of the vector font, glyph units (a glyph cell is 5 × 7
/// units; one unit = cell height / 7).
pub const GLYPH_STROKE_HALF_WIDTH: f32 = STROKE_HALF_WIDTH;

/// Width in pixels of `text` in the vector font with glyph cells `cell`
/// pixels tall: `(6n − 1) · cell / 7` (0 for empty text).
pub fn vector_text_width(text: &str, cell: f32) -> f32 {
    let n = text.chars().count();
    if n == 0 {
        return 0.0;
    }
    (GLYPH_ADVANCE_UNITS * n as f32 - 1.0) * cell / 7.0
}

/// LCD canvas size.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct LcdSpec {
    /// Canvas width, pixels.
    pub width: u32,
    /// Canvas height, pixels.
    pub height: u32,
}

impl Default for LcdSpec {
    fn default() -> Self {
        LcdSpec {
            width: 1920,
            height: 1080,
        }
    }
}

impl LcdSpec {
    /// A canvas of `width × height` (16..=8192 each).
    pub fn new(width: u32, height: u32) -> Result<LcdSpec, DisplayError> {
        if !(16..=8192).contains(&width) || !(16..=8192).contains(&height) {
            return Err(DisplayError::BadSize(format!(
                "LCD {width}x{height} (16..=8192 each)"
            )));
        }
        Ok(LcdSpec { width, height })
    }
}

/// One LCD draw, canvas pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum LcdItem {
    /// A filled rectangle `[x0, y0, x1, y1]`, straight-alpha colour.
    Rect {
        /// Rectangle.
        rect: [f32; 4],
        /// Colour and opacity.
        color: [u8; 4],
    },
    /// An image stretched over `rect`, multiplied by `color` (alpha =
    /// opacity).
    Image {
        /// Bundle path (an [`ImageSet`] key).
        file: String,
        /// Rectangle.
        rect: [f32; 4],
        /// Tint and opacity.
        color: [u8; 4],
    },
    /// One line of vector text.
    Text {
        /// Text.
        text: String,
        /// Top-left corner of the first glyph cell.
        origin: [f32; 2],
        /// Glyph cell height, pixels.
        cell: f32,
        /// Colour and opacity.
        color: [u8; 4],
    },
}

/// What the LCD shows: a background and items back to front.
#[derive(Clone, Debug, PartialEq)]
pub struct LcdDrawList {
    /// Canvas width.
    pub width: u32,
    /// Canvas height.
    pub height: u32,
    /// Background colour.
    pub background: [u8; 3],
    /// Items, back to front.
    pub items: Vec<LcdItem>,
}

fn rgba(c: [u8; 3], opacity: f32) -> [u8; 4] {
    [
        c[0],
        c[1],
        c[2],
        (opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

fn text_item(
    text: String,
    anchor: [f32; 2],
    cell: f32,
    align: super::Align,
    valign: super::VAlign,
    color: [u8; 4],
    max_w: f32,
) -> LcdItem {
    let mut cell = cell;
    let w = vector_text_width(&text, cell);
    if w > max_w && w > 0.0 {
        cell *= max_w / w;
    }
    let w = vector_text_width(&text, cell);
    LcdItem::Text {
        origin: [align.left_of(anchor[0], w), valign.top_of(anchor[1], cell)],
        text,
        cell,
        color,
    }
}

impl LcdDrawList {
    /// Composes `scene` at `elapsed_ms` on a `spec` canvas. Text is shrunk to
    /// fit the canvas width (score text: its box width). Images with `h = 0`
    /// keep their aspect (from `images`; skipped when missing).
    pub fn compose(
        spec: &LcdSpec,
        scene: &Scene,
        elapsed_ms: u64,
        scores: &ScoreState,
        images: &ImageSet,
    ) -> LcdDrawList {
        let (fw, fh) = (spec.width as f32, spec.height as f32);
        let mut items = Vec::new();
        for layer in &scene.layers {
            let Some(st) = layer.state_at(elapsed_ms, scene.duration_ms) else {
                continue;
            };
            let bx = |bw: f32, bh: f32| {
                let x0 = layer.align.left_of(st.x * fw, bw);
                let y0 = layer.valign.top_of(st.y * fh, bh);
                [x0, y0, x0 + bw, y0 + bh]
            };
            let color = rgba(layer.color, st.opacity);
            match &layer.kind {
                LayerKind::Text(t) => items.push(text_item(
                    expand_text(t, scores),
                    [st.x * fw, st.y * fh],
                    layer.size * fh,
                    layer.align,
                    layer.valign,
                    color,
                    fw,
                )),
                LayerKind::Rect => items.push(LcdItem::Rect {
                    rect: bx(layer.w * fw, layer.h * fh),
                    color,
                }),
                LayerKind::Image(file) => {
                    let bw = layer.w * fw;
                    let bh = if layer.h > 0.0 {
                        layer.h * fh
                    } else {
                        match images.get(file) {
                            Some(img) => bw * img.height as f32 / img.width.max(1) as f32,
                            None => continue,
                        }
                    };
                    items.push(LcdItem::Image {
                        file: file.clone(),
                        rect: bx(bw, bh),
                        color,
                    });
                }
                LayerKind::Score => {
                    let b = bx(layer.w * fw, layer.h * fh);
                    let (bw, bh) = (b[2] - b[0], b[3] - b[1]);
                    for t in score_view_layout(scores) {
                        let c = layer.color.map(|v| (f32::from(v) * t.emphasis).round() as u8);
                        items.push(text_item(
                            t.text,
                            [b[0] + t.x * bw, b[1] + t.y * bh],
                            t.size * bh,
                            t.align,
                            t.valign,
                            rgba(c, st.opacity),
                            bw,
                        ));
                    }
                }
            }
        }
        LcdDrawList {
            width: spec.width,
            height: spec.height,
            background: scene.background,
            items,
        }
    }
}

fn premul(c: [u8; 4]) -> [f32; 4] {
    let a = f32::from(c[3]) / 255.0;
    [
        f32::from(c[0]) / 255.0 * a,
        f32::from(c[1]) / 255.0 * a,
        f32::from(c[2]) / 255.0 * a,
        a,
    ]
}

/// Draws [`LcdDrawList`]s.
pub struct LcdRenderer {
    quads: QuadRenderer,
    spec: LcdSpec,
    images: BTreeMap<String, usize>,
    list: Option<LcdDrawList>,
}

impl LcdRenderer {
    /// A renderer of a `spec` canvas for targets of `format`.
    pub fn new(ctx: &GpuContext, format: wgpu::TextureFormat, spec: LcdSpec) -> LcdRenderer {
        LcdRenderer {
            quads: QuadRenderer::new(ctx, format),
            spec,
            images: BTreeMap::new(),
            list: None,
        }
    }

    /// The canvas spec.
    pub fn spec(&self) -> LcdSpec {
        self.spec
    }

    /// Uploads the image used by scene layers with `file = <file>`. Marks
    /// the display changed.
    pub fn set_image(&mut self, ctx: &GpuContext, file: &str, image: &RgbaImage) {
        let i = self.quads.add_image(ctx.queue(), image);
        self.images.insert(file.to_owned(), i);
        self.list = None;
    }

    /// Takes `list` as the content; returns whether it differs from the
    /// current content (the display then needs a redraw).
    pub fn set_draw_list(&mut self, _queue: &wgpu::Queue, list: &LcdDrawList) -> bool {
        if self.list.as_ref() == Some(list) {
            return false;
        }
        self.list = Some(list.clone());
        true
    }

    /// Records the current content into `viewport` of `view` (a texture of
    /// `target_size`): letterbox black, the canvas fitted to its aspect.
    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target_size: (u32, u32),
        viewport: Rect,
    ) {
        let canvas = fit_aspect(viewport, self.spec.width, self.spec.height);
        let s = canvas.w as f32 / self.spec.width as f32;
        let (ox, oy) = (canvas.x as f32, canvas.y as f32);
        let map = |r: [f32; 4]| [ox + r[0] * s, oy + r[1] * s, ox + r[2] * s, oy + r[3] * s];
        let vp = [
            viewport.x as f32,
            viewport.y as f32,
            viewport.right() as f32,
            viewport.bottom() as f32,
        ];
        let mut draws = vec![QuadDraw::solid(vp, [0.0, 0.0, 0.0, 1.0])];
        if let Some(list) = &self.list {
            let full = [0.0, 0.0, self.spec.width as f32, self.spec.height as f32];
            draws.push(QuadDraw::solid(map(full), premul(rgba(list.background, 1.0))));
            for item in &list.items {
                match item {
                    LcdItem::Rect { rect, color } => {
                        draws.push(QuadDraw::solid(map(*rect), premul(*color)));
                    }
                    LcdItem::Image { file, rect, color } => {
                        let Some(&tex) = self.images.get(file) else {
                            continue;
                        };
                        draws.push(QuadDraw {
                            inst: QuadInstance {
                                rect: map(*rect),
                                uv: [0.0, 0.0, 1.0, 1.0],
                                color: premul(*color),
                                params: [1.0, 0.0, 0.0, 0.0],
                            },
                            tex: (tex, WHITE),
                            blend: super::quad::Blend::Over,
                        });
                    }
                    LcdItem::Text {
                        text,
                        origin,
                        cell,
                        color,
                    } => {
                        let unit = cell / 7.0;
                        for (i, ch) in text.chars().enumerate() {
                            if ch == ' ' {
                                continue;
                            }
                            let x = origin[0] + i as f32 * GLYPH_ADVANCE_UNITS * unit;
                            let r = [x, origin[1], x + 5.0 * unit, origin[1] + 7.0 * unit];
                            draws.push(QuadDraw {
                                inst: QuadInstance {
                                    rect: map(r),
                                    uv: [0.0, 0.0, 5.0, 7.0],
                                    color: premul(*color),
                                    params: [3.0, glyph_slot(ch) as f32, 1.0 / (unit * s), 0.0],
                                },
                                tex: (WHITE, WHITE),
                                blend: super::quad::Blend::Over,
                            });
                        }
                    }
                }
            }
        }
        self.quads
            .encode(encoder, view, target_size, viewport, &draws);
    }
}

/// Renders `list` on a `spec` LCD filling a `width × height` image
/// (`Rgba8Unorm`, viewport = whole image) and reads it back. `images`
/// supplies the image items.
pub fn render_lcd_to_image(
    ctx: &GpuContext,
    spec: LcdSpec,
    list: &LcdDrawList,
    images: &ImageSet,
    width: u32,
    height: u32,
) -> Result<RgbaImage, GpuError> {
    let mut r = LcdRenderer::new(ctx, wgpu::TextureFormat::Rgba8Unorm, spec);
    for (file, img) in images.iter() {
        r.set_image(ctx, file, img);
    }
    r.set_draw_list(ctx.queue(), list);
    render_offscreen(ctx, width, height, |encoder, view| {
        r.encode(encoder, view, (width, height), Rect::new(0, 0, width, height));
    })
}
