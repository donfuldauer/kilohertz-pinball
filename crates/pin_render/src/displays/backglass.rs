//! Backglass: layered translite art with lamp-driven masks and flashers.

use super::quad::{Blend, QuadDraw, QuadInstance, QuadRenderer, WHITE, render_offscreen};
use super::routing::fit_aspect;
use super::{DisplayError, RgbaImage};
use crate::gpu::{GpuContext, GpuError};
use crate::topology::Rect;
use pin_core::LampId;
use pin_core::schema::backglass::BackglassManifest;
use pin_core::schema::table::LampKind;

/// A referenced lamp must move at least this much since the last draw to
/// need a redraw (below half an 8-bit step of any layer).
pub const LAMP_REDRAW_EPSILON: f32 = 1.0 / 1024.0;

/// How a lamp-driven layer combines with the layers below it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LayerBlend {
    /// Alpha-over weighted by mask · lamp: `out = dst + (rgb − dst)·k`.
    Over,
    /// Added on top (flashers): `out = dst + rgb·k`.
    Additive,
}

/// The lamp that lights a layer, through a mask.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerLamp {
    /// Lamp (index into the snapshot's `lamps`).
    pub lamp: LampId,
    /// Mask; its red channel is the mask value (a grey PNG decodes to
    /// equal channels). Stretched over the translite like the layer.
    pub mask: RgbaImage,
    /// Blend mode.
    pub blend: LayerBlend,
}

/// One translite layer.
#[derive(Clone, Debug, PartialEq)]
pub struct BackglassLayer {
    /// Art (straight alpha).
    pub image: RgbaImage,
    /// Lamp and mask, for lit layers.
    pub lamp: Option<LayerLamp>,
}

/// The whole backglass, back to front.
#[derive(Clone, Debug, PartialEq)]
pub struct BackglassDesc {
    /// Layers, back to front. The first layer's size sets the aspect.
    pub layers: Vec<BackglassLayer>,
}

impl BackglassDesc {
    /// Builds the backglass from `backglass.toml`: layers sorted by `depth`
    /// (stable), each `lamp_mask` resolved to its `[[lamp_masks]]` entry and
    /// lamp. A lamp of kind `flasher` makes the layer [`LayerBlend::Additive`],
    /// others [`LayerBlend::Over`]. `load_image` decodes a bundle image.
    pub fn from_manifest(
        m: &BackglassManifest,
        resolve_lamp: impl Fn(&str) -> Option<(LampId, LampKind)>,
        mut load_image: impl FnMut(&str) -> Result<RgbaImage, String>,
    ) -> Result<BackglassDesc, DisplayError> {
        let mut order: Vec<_> = m.translite.layers.iter().collect();
        order.sort_by_key(|l| l.depth);
        let mut layers = Vec::with_capacity(order.len());
        for l in order {
            let image = load_image(&l.file).map_err(|e| DisplayError::Image(l.file.clone(), e))?;
            let lamp = match &l.lamp_mask {
                None => None,
                Some(id) => {
                    let mask = m.lamp_masks.iter().find(|k| &k.id == id).ok_or_else(|| {
                        DisplayError::Reference(format!("lamp mask {id:?} of layer {}", l.file))
                    })?;
                    let (lamp, kind) = resolve_lamp(&mask.lamp).ok_or_else(|| {
                        DisplayError::Reference(format!("lamp {:?} of mask {id:?}", mask.lamp))
                    })?;
                    let img = load_image(&mask.file)
                        .map_err(|e| DisplayError::Image(mask.file.clone(), e))?;
                    Some(LayerLamp {
                        lamp,
                        mask: img,
                        blend: if kind == LampKind::Flasher {
                            LayerBlend::Additive
                        } else {
                            LayerBlend::Over
                        },
                    })
                }
            };
            layers.push(BackglassLayer { image, lamp });
        }
        if layers.is_empty() {
            return Err(DisplayError::Reference("translite has no layers".into()));
        }
        Ok(BackglassDesc { layers })
    }

    /// Aspect `(w, h)` of the translite (first layer; 4:3 when empty).
    pub fn aspect(&self) -> (u32, u32) {
        match self.layers.first() {
            Some(l) if l.image.width > 0 && l.image.height > 0 => (l.image.width, l.image.height),
            _ => (4, 3),
        }
    }
}

fn lamp_level(lamps: &[f32], id: LampId) -> f32 {
    let v = lamps.get(usize::from(id.0)).copied().unwrap_or(0.0);
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

struct GpuLayer {
    tex: (usize, usize),
    lamp: Option<(LampId, LayerBlend)>,
}

/// Draws a [`BackglassDesc`] with the current lamp levels.
pub struct BackglassRenderer {
    quads: QuadRenderer,
    layers: Vec<GpuLayer>,
    aspect: (u32, u32),
    drawn: Option<Vec<f32>>,
}

impl BackglassRenderer {
    /// Uploads the layers of `desc` for targets of `format`.
    pub fn new(
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        desc: &BackglassDesc,
    ) -> Result<BackglassRenderer, GpuError> {
        let max = ctx.device().limits().max_texture_dimension_2d;
        let mut quads = QuadRenderer::new(ctx, format);
        let mut layers = Vec::with_capacity(desc.layers.len());
        let check = |img: &RgbaImage| {
            if img.width == 0 || img.height == 0 || img.width > max || img.height > max {
                Err(GpuError::InvalidArgument(format!(
                    "backglass image {}x{} (each side 1..={max})",
                    img.width, img.height
                )))
            } else {
                Ok(())
            }
        };
        for l in &desc.layers {
            check(&l.image)?;
            let image = quads.add_image(ctx.queue(), &l.image);
            let (mask, lamp) = match &l.lamp {
                None => (WHITE, None),
                Some(ll) => {
                    check(&ll.mask)?;
                    (
                        quads.add_image(ctx.queue(), &ll.mask),
                        Some((ll.lamp, ll.blend)),
                    )
                }
            };
            layers.push(GpuLayer {
                tex: (image, mask),
                lamp,
            });
        }
        Ok(BackglassRenderer {
            quads,
            layers,
            aspect: desc.aspect(),
            drawn: None,
        })
    }

    /// Aspect of the translite.
    pub fn aspect(&self) -> (u32, u32) {
        self.aspect
    }

    /// The lamps the layers depend on, in layer order (repeats kept).
    pub fn lamps(&self) -> Vec<LampId> {
        self.layers
            .iter()
            .filter_map(|l| l.lamp.map(|(id, _)| id))
            .collect()
    }

    /// Whether the backglass must be redrawn for `lamps`: never drawn, marked
    /// dirty, or a referenced lamp moved by at least [`LAMP_REDRAW_EPSILON`]
    /// (or reached 0 / 1 exactly) since the last [`encode`](Self::encode).
    pub fn needs_redraw(&self, lamps: &[f32]) -> bool {
        let Some(drawn) = &self.drawn else {
            return true;
        };
        self.lamps().iter().zip(drawn).any(|(&id, &old)| {
            let new = lamp_level(lamps, id);
            (new - old).abs() >= LAMP_REDRAW_EPSILON
                || (new != old && (new == 0.0 || new == 1.0))
        })
    }

    /// Forces the next [`needs_redraw`](Self::needs_redraw) to be `true`
    /// (resize, expose).
    pub fn mark_dirty(&mut self) {
        self.drawn = None;
    }

    /// Records the backglass into `viewport` of `view` (a texture of
    /// `target_size`) with `lamps` (indexed by `LampId`; missing = 0, values
    /// clamped to 0..=1): letterbox black, then the layers back to front.
    pub fn encode(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        target_size: (u32, u32),
        viewport: Rect,
        lamps: &[f32],
    ) {
        let art = fit_aspect(viewport, self.aspect.0, self.aspect.1);
        let rect = [
            art.x as f32,
            art.y as f32,
            art.right() as f32,
            art.bottom() as f32,
        ];
        let vp = [
            viewport.x as f32,
            viewport.y as f32,
            viewport.right() as f32,
            viewport.bottom() as f32,
        ];
        let mut draws = vec![QuadDraw::solid(vp, [0.0, 0.0, 0.0, 1.0])];
        for l in &self.layers {
            let (k, mode, blend) = match l.lamp {
                None => (1.0, 1.0, Blend::Over),
                Some((id, b)) => (
                    lamp_level(lamps, id),
                    2.0,
                    match b {
                        LayerBlend::Over => Blend::Over,
                        LayerBlend::Additive => Blend::Add,
                    },
                ),
            };
            if k == 0.0 {
                continue;
            }
            draws.push(QuadDraw {
                inst: QuadInstance {
                    rect,
                    uv: [0.0, 0.0, 1.0, 1.0],
                    color: [k; 4],
                    params: [mode, 0.0, 0.0, 0.0],
                },
                tex: l.tex,
                blend,
            });
        }
        self.quads
            .encode(encoder, view, target_size, viewport, &draws);
        self.drawn = Some(
            self.lamps()
                .into_iter()
                .map(|id| lamp_level(lamps, id))
                .collect(),
        );
    }
}

/// Renders `desc` with `lamps` filling a `width × height` image
/// (`Rgba8Unorm`, viewport = whole image) and reads it back.
pub fn render_backglass_to_image(
    ctx: &GpuContext,
    desc: &BackglassDesc,
    lamps: &[f32],
    width: u32,
    height: u32,
) -> Result<RgbaImage, GpuError> {
    let mut r = BackglassRenderer::new(ctx, wgpu::TextureFormat::Rgba8Unorm, desc)?;
    render_offscreen(ctx, width, height, |encoder, view| {
        r.encode(
            encoder,
            view,
            (width, height),
            Rect::new(0, 0, width, height),
            lamps,
        );
    })
}
