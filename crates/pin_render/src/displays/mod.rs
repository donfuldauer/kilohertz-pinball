//! Backglass and dual-mode score display (PLAN §4 Subphase 5.3).
//!
//! Owner: Subphase 5.3. Stub created by Subphase 1.1.
//!
//! * **Backglass** ([`BackglassRenderer`]): layered translite art; a layer
//!   tied to a lamp is blended through its lamp mask in proportion to the
//!   lamp level (alpha-over), or added on top for flashers.
//! * **Score display**, one of two modes (`backglass.toml`
//!   `[score_display] mode`):
//!   * **DMD** ([`DmdRenderer`]): a procedural dot grid (128 × 32 or 192 × 64
//!     dots; orange, red or RGB). The content is a CPU [`DmdFrame`] of dot
//!     drive levels; the shader draws round dots, unlit dots faintly visible.
//!   * **LCD** ([`LcdRenderer`]): a 16:9 compositor of rectangles, sprites and
//!     scalable "vector" text from an [`LcdDrawList`].
//! * **Scenes** ([`Scene`], [`SceneLibrary`], [`ScenePlayer`]): the display
//!   scene format (TOML; DECISIONS "5.3 — Display scene format") shown by the
//!   rules' `display_scenes` commands, plus placeholder scenes
//!   ([`PLACEHOLDER_SCENES`]). Without an active scene the display shows the
//!   score view (formatted by [`format_score`] / [`score_lines`]).
//! * **Routing** ([`display_routes`], [`DisplaySet`]): each display is drawn
//!   into its viewport in the topology plan: a PiP rectangle on the playfield
//!   window or a dedicated window.
//! * **Change detection**: displays redraw only on change
//!   ([`DmdRenderer::set_frame`], [`LcdRenderer::set_draw_list`],
//!   [`BackglassRenderer::needs_redraw`], [`ChangeTracker`]).
//!
//! Colour: images and compositing are in display-encoded (sRGB-valued)
//! space, as 2D art tools blend; the render targets of 1.4 are non-sRGB
//! formats, so shader output is written unchanged. On an `*Srgb` target the
//! shaders decode their output first, so stored values still match.
//!
//! Everything except the renderers is GPU-free.

mod backglass;
mod dmd;
mod lcd;
mod quad;
mod routing;
mod scene;
mod score;

#[cfg(test)]
mod tests;

pub use backglass::{
    BackglassDesc, BackglassLayer, BackglassRenderer, LAMP_REDRAW_EPSILON, LayerBlend, LayerLamp,
    render_backglass_to_image,
};
pub use dmd::{
    DMD_128X32, DMD_192X64, DMD_BACKGROUND, DMD_DOT_RADIUS, DMD_UNLIT_LEVEL, DmdFont, DmdFrame,
    DmdGeometry, DmdRenderer, DmdSpec, dmd_dot_color, dmd_geometry, dmd_text_dots, dmd_tint,
    render_dmd_to_image,
};
pub use lcd::{
    GLYPH_ADVANCE_UNITS, GLYPH_STROKE_HALF_WIDTH, LcdDrawList, LcdItem, LcdRenderer, LcdSpec,
    TEXT_FIT, render_lcd_to_image, vector_text_width,
};
pub use routing::{
    ChangeTracker, DisplayContent, DisplayDirty, DisplayRoute, DisplaySet, ScoreDisplayConfig,
    ScoreDisplayModel, ScoreRenderer, display_routes, fit_aspect,
};
pub use scene::{
    Align, FontChoice, LayerKind, LayerState, MAX_SCENE_BYTES, MAX_SCENE_LAYERS, MAX_TEXT_CHARS,
    PLACEHOLDER_SCENES, SCORE_SCENE, Scene, SceneError, SceneLayer, SceneLibrary, ScenePlayer,
    VAlign, parse_color, placeholder_scene,
};
pub use score::{
    MAX_PLAYERS, PlayerRow, ScoreLines, ScoreState, ScoreText, expand_text, format_score,
    format_score_with, score_lines, score_view_layout,
};

use core::fmt;

/// WGSL sources of the display shaders `(name, source)` (for naga
/// validation in tests): the quad compositor and the DMD dot grid.
pub fn shader_sources() -> [(&'static str, String); 2] {
    [
        ("display_quads", quad::quad_shader_source()),
        ("dmd", dmd::dmd_shader_source()),
    ]
}
use std::collections::BTreeMap;

/// Errors from building displays from manifests or images.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplayError {
    /// A size outside the supported range.
    BadSize(String),
    /// An image could not be loaded (path, reason).
    Image(String, String),
    /// A lamp mask, lamp or layer reference is unknown.
    Reference(String),
}

impl fmt::Display for DisplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DisplayError::BadSize(e) => write!(f, "bad display size: {e}"),
            DisplayError::Image(p, e) => write!(f, "image {p}: {e}"),
            DisplayError::Reference(e) => write!(f, "unknown reference: {e}"),
        }
    }
}

impl std::error::Error for DisplayError {}

/// An 8-bit straight-alpha RGBA image, display-encoded (the sRGB values a
/// PNG stores), row 0 at the top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width · height · 4` bytes, row-major.
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    /// A transparent black image.
    pub fn new(width: u32, height: u32) -> RgbaImage {
        RgbaImage {
            width,
            height,
            pixels: vec![0; width as usize * height as usize * 4],
        }
    }

    fn checked(width: u32, height: u32, data: &[u8], channels: usize) -> bool {
        width > 0
            && height > 0
            && (width as usize)
                .checked_mul(height as usize)
                .and_then(|n| n.checked_mul(channels))
                == Some(data.len())
    }

    /// An opaque image from one grey byte per pixel; `None` on a size mismatch
    /// or a zero size.
    pub fn from_gray(width: u32, height: u32, data: Vec<u8>) -> Option<RgbaImage> {
        if !Self::checked(width, height, &data, 1) {
            return None;
        }
        let pixels = data.iter().flat_map(|&g| [g, g, g, 255]).collect();
        Some(RgbaImage {
            width,
            height,
            pixels,
        })
    }

    /// An opaque image from RGB bytes; `None` on a size mismatch or a zero size.
    pub fn from_rgb(width: u32, height: u32, data: Vec<u8>) -> Option<RgbaImage> {
        if !Self::checked(width, height, &data, 3) {
            return None;
        }
        let pixels = data
            .chunks_exact(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect();
        Some(RgbaImage {
            width,
            height,
            pixels,
        })
    }

    /// An image from RGBA bytes; `None` on a size mismatch or a zero size.
    pub fn from_rgba(width: u32, height: u32, data: Vec<u8>) -> Option<RgbaImage> {
        if !Self::checked(width, height, &data, 4) {
            return None;
        }
        Some(RgbaImage {
            width,
            height,
            pixels: data,
        })
    }

    /// Pixel `(x, y)`; transparent black outside the image.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0; 4];
        }
        let o = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.pixels[o],
            self.pixels[o + 1],
            self.pixels[o + 2],
            self.pixels[o + 3],
        ]
    }

    /// The pixels with colour multiplied by alpha (for GPU upload).
    pub(crate) fn premultiplied(&self) -> Vec<u8> {
        self.pixels
            .chunks_exact(4)
            .flat_map(|p| {
                let a = u32::from(p[3]);
                let m = |c: u8| ((u32::from(c) * a + 127) / 255) as u8;
                [m(p[0]), m(p[1]), m(p[2]), p[3]]
            })
            .collect()
    }
}

/// Images referenced by scenes (`kind = "image"` layers), by bundle path.
/// The caller decodes them (through `pin_assets`' VFS); the displays only
/// read pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageSet {
    images: BTreeMap<String, RgbaImage>,
}

impl ImageSet {
    /// An empty set.
    pub fn new() -> ImageSet {
        ImageSet::default()
    }

    /// Adds or replaces the image of `file`.
    pub fn insert(&mut self, file: &str, image: RgbaImage) {
        self.images.insert(file.to_owned(), image);
    }

    /// The image of `file`.
    pub fn get(&self, file: &str) -> Option<&RgbaImage> {
        self.images.get(file)
    }

    /// `(file, image)` pairs in path order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &RgbaImage)> {
        self.images.iter().map(|(k, v)| (k.as_str(), v))
    }
}
