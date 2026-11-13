//! Procedural placeholder art derived from table metadata (4.6 deliverable 4):
//! the backglass translite (base, lit title layer and its lamp mask, matching
//! the Appendix A.2 layer layout), playfield art, insert labels and the DMD
//! font sheets. Everything is integer drawing in a fixed order from the kit's
//! own PCG; images are written by the deterministic [`png`] encoder.

pub mod png;

use crate::TableMeta;
use crate::fonts::{BitmapFont, FONT_5X7, FONT_8X16, GLYPH_SPACING, blit_text};
use crate::rng::Pcg32;
pub use png::{ColorType, encode_png};

/// Palette used when the table gives none (the Appendix A example colours).
pub const DEFAULT_PALETTE: [[u8; 3]; 3] = [[0x0b, 0x1d, 0x3a], [0xf2, 0xa9, 0x00], [0xe6, 0x39, 0x46]];

/// Translite (backglass) size in pixels.
pub const TRANSLITE_SIZE: (u32, u32) = (512, 384);
/// Playfield art size in pixels: 1 px per mm of the Appendix A.3 playfield.
pub const PLAYFIELD_SIZE: (u32, u32) = (514, 1070);
/// Insert label size in pixels.
pub const INSERT_LABEL_SIZE: (u32, u32) = (96, 48);

/// Insert labels the kit draws: `(name, text)`. The stable id is
/// `img_insert_<name>`, the file `art/inserts/<name>.png`.
pub const INSERT_LABELS: &[(&str, &str)] = &[
    ("extra_ball", "EXTRA BALL"),
    ("shoot_again", "SHOOT AGAIN"),
    ("jackpot", "JACKPOT"),
    ("super_jackpot", "SUPER JACKPOT"),
    ("multiball", "MULTIBALL"),
    ("lock", "LOCK"),
    ("special", "SPECIAL"),
    ("x2", "2X"),
    ("x3", "3X"),
    ("x5", "5X"),
];

// PCG streams, one per image family.
const STREAM_TRANSLITE: u64 = 0x7472_616e;
const STREAM_PLAYFIELD: u64 = 0x706c_6179;

/// An 8-bit image in memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel layout.
    pub color: ColorType,
    /// Row-major pixels, `color.channels()` bytes each.
    pub data: Vec<u8>,
}

impl Image {
    /// A zero-filled image (black; transparent for RGBA).
    pub fn new(width: u32, height: u32, color: ColorType) -> Image {
        Image {
            width,
            height,
            color,
            data: vec![0; width as usize * height as usize * color.channels()],
        }
    }

    /// PNG bytes of the image (`None` only for a zero-sized image).
    pub fn to_png(&self) -> Option<Vec<u8>> {
        encode_png(self.width, self.height, self.color, &self.data)
    }

    fn offset(&self, x: i64, y: i64) -> Option<usize> {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return None;
        }
        Some((y as usize * self.width as usize + x as usize) * self.color.channels())
    }

    /// Blends `rgb` over pixel `(x, y)` with coverage `alpha` (0–255);
    /// RGBA pixels also gain opacity. Pixels off the image are ignored.
    pub fn blend(&mut self, x: i64, y: i64, rgb: [u8; 3], alpha: u8) {
        let Some(o) = self.offset(x, y) else {
            return;
        };
        let a = u32::from(alpha);
        let mix = |d: u8, s: u8| ((u32::from(d) * (255 - a) + u32::from(s) * a + 127) / 255) as u8;
        match self.color {
            ColorType::Gray => {
                self.data[o] = mix(self.data[o], luma(rgb));
            }
            ColorType::Rgb => {
                for c in 0..3 {
                    self.data[o + c] = mix(self.data[o + c], rgb[c]);
                }
            }
            ColorType::Rgba => {
                let da = u32::from(self.data[o + 3]);
                let out_a = a + da * (255 - a) / 255;
                if out_a == 0 {
                    return;
                }
                for c in 0..3 {
                    let num = u32::from(rgb[c]) * a * 255
                        + u32::from(self.data[o + c]) * da * (255 - a);
                    self.data[o + c] = ((num + out_a * 255 / 2) / (out_a * 255)) as u8;
                }
                self.data[o + 3] = out_a as u8;
            }
        }
    }

    /// Fills the rectangle `[x0, x1) × [y0, y1)` (clipped) by blending.
    pub fn fill_rect(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, rgb: [u8; 3], alpha: u8) {
        let (x0, x1) = (x0.max(0), x1.min(i64::from(self.width)));
        let (y0, y1) = (y0.max(0), y1.min(i64::from(self.height)));
        for y in y0..y1 {
            for x in x0..x1 {
                self.blend(x, y, rgb, alpha);
            }
        }
    }

    /// Fills (or, with `ring > 0`, outlines `ring` pixels wide) a circle.
    pub fn circle(&mut self, cx: i64, cy: i64, r: i64, ring: i64, rgb: [u8; 3], alpha: u8) {
        let inner = if ring > 0 { (r - ring).max(0) } else { -1 };
        for y in cy - r..=cy + r {
            for x in cx - r..=cx + r {
                let d2 = (x - cx) * (x - cx) + (y - cy) * (y - cy);
                if d2 <= r * r && (inner < 0 || d2 > inner * inner) {
                    self.blend(x, y, rgb, alpha);
                }
            }
        }
    }

    /// An upward chevron with its tip at `(cx, top)`, `half` wide each side
    /// and `thick` pixels thick.
    pub fn chevron(&mut self, cx: i64, top: i64, half: i64, thick: i64, rgb: [u8; 3], alpha: u8) {
        for dx in -half..=half {
            let y0 = top + dx.abs();
            for y in y0..y0 + thick {
                self.blend(cx + dx, y, rgb, alpha);
            }
        }
    }

    /// A line `thick` pixels wide (integer DDA, fixed step count).
    #[allow(clippy::too_many_arguments)]
    pub fn line(&mut self, x0: i64, y0: i64, x1: i64, y1: i64, thick: i64, rgb: [u8; 3], alpha: u8) {
        let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
        let t0 = -(thick / 2);
        for i in 0..=steps {
            let x = x0 + (x1 - x0) * i / steps;
            let y = y0 + (y1 - y0) * i / steps;
            for dy in t0..t0 + thick.max(1) {
                for dx in t0..t0 + thick.max(1) {
                    self.blend(x + dx, y + dy, rgb, alpha);
                }
            }
        }
    }

    /// Draws text with a one-pixel outline: lit pixels in `fg`, their
    /// 8-neighbours that are not lit in `outline`.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        font: &BitmapFont,
        text: &str,
        x: i64,
        y: i64,
        scale: u32,
        fg: [u8; 3],
        outline: [u8; 3],
    ) {
        let mask = text_mask(self.width, self.height, font, text, x, y, scale);
        paint_mask(self, &mask, fg, outline);
    }
}

/// Lit pixels (255) and their outline (128) of `text` on a `w`×`h` mask.
fn text_mask(w: u32, h: u32, font: &BitmapFont, text: &str, x: i64, y: i64, scale: u32) -> Vec<u8> {
    let mut mask = vec![0u8; w as usize * h as usize];
    blit_text(font, text, &mut mask, w, h, x, y, scale, 255);
    let (wi, hi) = (i64::from(w), i64::from(h));
    let lit = mask.clone();
    for yy in 0..hi {
        for xx in 0..wi {
            if lit[(yy * wi + xx) as usize] != 0 {
                continue;
            }
            let near = (-1..=1).any(|dy: i64| {
                (-1..=1).any(|dx: i64| {
                    let (nx, ny) = (xx + dx, yy + dy);
                    nx >= 0 && ny >= 0 && nx < wi && ny < hi && lit[(ny * wi + nx) as usize] != 0
                })
            });
            if near {
                mask[(yy * wi + xx) as usize] = 128;
            }
        }
    }
    mask
}

fn paint_mask(img: &mut Image, mask: &[u8], fg: [u8; 3], outline: [u8; 3]) {
    let w = i64::from(img.width);
    for (i, &m) in mask.iter().enumerate() {
        let (x, y) = (i as i64 % w, i as i64 / w);
        match m {
            255 => img.blend(x, y, fg, 255),
            128 => img.blend(x, y, outline, 255),
            _ => {}
        }
    }
}

/// Rec. 601 luma, integer.
pub fn luma(c: [u8; 3]) -> u8 {
    ((299 * u32::from(c[0]) + 587 * u32::from(c[1]) + 114 * u32::from(c[2]) + 500) / 1000) as u8
}

/// `a` mixed towards `b` by `t`/255.
pub fn mix(a: [u8; 3], b: [u8; 3], t: u8) -> [u8; 3] {
    let t = u32::from(t);
    let m = |x: u8, y: u8| ((u32::from(x) * (255 - t) + u32::from(y) * t + 127) / 255) as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

const WHITE: [u8; 3] = [255, 255, 255];
const BLACK: [u8; 3] = [0, 0, 0];

/// Palette entry `i` of the table, cycling; [`DEFAULT_PALETTE`] when empty.
pub fn palette_color(meta: &TableMeta, i: usize) -> [u8; 3] {
    if meta.palette.is_empty() {
        DEFAULT_PALETTE[i % DEFAULT_PALETTE.len()]
    } else {
        meta.palette[i % meta.palette.len()]
    }
}

/// `fg`, or black/white when `fg` would not read on `bg`.
fn readable(fg: [u8; 3], bg: [u8; 3]) -> [u8; 3] {
    let (lf, lb) = (i32::from(luma(fg)), i32::from(luma(bg)));
    if (lf - lb).abs() >= 96 {
        fg
    } else if lb < 128 {
        WHITE
    } else {
        BLACK
    }
}

/// Largest integer scale in `1..=max` at which `text` fits in `avail` pixels
/// (1 when even scale 1 does not fit; the text is then clipped).
fn fit_scale(font: &BitmapFont, text: &str, avail: u32, max: u32) -> u32 {
    let w = u64::from(font.text_width(text));
    (1..=max).rev().find(|&s| w * u64::from(s) <= u64::from(avail)).unwrap_or(1)
}

/// The title as drawn on the art: upper-cased ASCII letters, other
/// characters kept (non-ASCII draws the font's fallback box), at most 48
/// characters.
fn display_title(meta: &TableMeta) -> String {
    meta.title.chars().take(48).map(|c| c.to_ascii_uppercase()).collect()
}

/// Where the translite title goes: `(x, y, scale)`.
fn translite_title_place(meta: &TableMeta) -> (String, i64, i64, u32) {
    let (w, h) = TRANSLITE_SIZE;
    let t = display_title(meta);
    let scale = fit_scale(FONT_8X16, &t, w - 32, 4);
    let tw = i64::from(FONT_8X16.text_width(&t)) * i64::from(scale);
    let x = (i64::from(w) - tw) / 2;
    let y = i64::from(h) / 2 - 8 * i64::from(scale);
    (t, x.max(0), y, scale)
}

/// The backglass translite base layer (RGB).
pub fn translite_base(meta: &TableMeta) -> Image {
    let (w, h) = TRANSLITE_SIZE;
    let (c0, c1, c2) = (palette_color(meta, 0), palette_color(meta, 1), palette_color(meta, 2));
    let mut rng = Pcg32::new(crate::meta_seed(meta), STREAM_TRANSLITE);
    let mut img = Image::new(w, h, ColorType::Rgb);
    // Vertical gradient.
    let top = mix(c0, BLACK, 90);
    let bottom = mix(c0, c2, 70);
    for y in 0..h {
        let c = mix(top, bottom, (y * 255 / (h - 1)) as u8);
        img.fill_rect(0, i64::from(y), i64::from(w), i64::from(y) + 1, c, 255);
    }
    // Sunburst from below the bottom edge.
    let cx = 192.0 + f64::from(rng.below(129));
    let cy = f64::from(h) + 40.0;
    let rays = 10 + rng.below(13);
    let twist = rng.unit();
    for y in 0..h {
        for x in 0..w {
            let a = libm::atan2(f64::from(y) - cy, f64::from(x) - cx);
            let t = (a + core::f64::consts::PI) / core::f64::consts::TAU;
            let sector = libm::floor((t * f64::from(rays * 2)) + twist) as i64;
            if sector % 2 == 0 {
                img.blend(i64::from(x), i64::from(y), c1, 34);
            }
        }
    }
    // Rings.
    for i in 0..6 + rng.below(6) {
        let x = i64::from(rng.below(w));
        let y = i64::from(rng.below(h));
        let r = 10 + i64::from(rng.below(50));
        let col = if i % 2 == 0 { c1 } else { c2 };
        img.circle(x, y, r, 3, col, 110);
    }
    // Title band and title.
    let (t, x, y, scale) = translite_title_place(meta);
    if !t.is_empty() {
        let s = i64::from(scale);
        img.fill_rect(0, y - 2 * s - 4, i64::from(w), y + 18 * s + 4, BLACK, 150);
        img.text(FONT_8X16, &t, x, y, scale, readable(c1, BLACK), BLACK);
    }
    // Frame.
    for (i, col) in [(0, c2), (4, c1)] {
        let (x0, y0, x1, y1) = (i, i, i64::from(w) - i, i64::from(h) - i);
        img.fill_rect(x0, y0, x1, y0 + 3, col, 255);
        img.fill_rect(x0, y1 - 3, x1, y1, col, 255);
        img.fill_rect(x0, y0, x0 + 3, y1, col, 255);
        img.fill_rect(x1 - 3, y0, x1, y1, col, 255);
    }
    img
}

/// The lit title layer of the translite (RGBA; transparent off the title).
pub fn translite_logo(meta: &TableMeta) -> Image {
    let (w, h) = TRANSLITE_SIZE;
    let mut img = Image::new(w, h, ColorType::Rgba);
    let (t, x, y, scale) = translite_title_place(meta);
    let glow = mix(readable(palette_color(meta, 1), BLACK), WHITE, 96);
    img.text(FONT_8X16, &t, x, y, scale, glow, palette_color(meta, 2));
    img
}

/// The lamp mask of the title layer (greyscale; 255 under the title).
pub fn translite_logo_mask(meta: &TableMeta) -> Image {
    let (w, h) = TRANSLITE_SIZE;
    let (t, x, y, scale) = translite_title_place(meta);
    let mask = text_mask(w, h, FONT_8X16, &t, x, y, scale);
    Image {
        width: w,
        height: h,
        color: ColorType::Gray,
        data: mask.into_iter().map(|m| if m == 0 { 0 } else { 255 }).collect(),
    }
}

/// Playfield art (RGB, 1 px per mm, y down from the back of the playfield).
pub fn playfield(meta: &TableMeta) -> Image {
    let (w, h) = PLAYFIELD_SIZE;
    let (wi, hi) = (i64::from(w), i64::from(h));
    let (c0, c1, c2) = (palette_color(meta, 0), palette_color(meta, 1), palette_color(meta, 2));
    let c3 = palette_color(meta, 3);
    let mut rng = Pcg32::new(crate::meta_seed(meta), STREAM_PLAYFIELD);
    let mut img = Image::new(w, h, ColorType::Rgb);
    let back = mix(c0, c2, 60);
    let front = mix(c0, BLACK, 60);
    for y in 0..h {
        let c = mix(back, front, (y * 255 / (h - 1)) as u8);
        img.fill_rect(0, i64::from(y), wi, i64::from(y) + 1, c, 255);
    }
    // Faint grid.
    let step = 40 + i64::from(rng.below(25));
    let mut g = step / 2;
    while g < wi.max(hi) {
        img.fill_rect(g, 0, g + 1, hi, c1, 22);
        img.fill_rect(0, g, wi, g + 1, c1, 22);
        g += step;
    }
    // Pop-bumper rings at the back.
    for i in 0..3 {
        let x = 150 + i64::from(i) * 107 + i64::from(rng.below(30));
        let y = 170 + i64::from(rng.below(120));
        img.circle(x, y, 34, 0, c2, 120);
        img.circle(x, y, 34, 4, c1, 255);
    }
    // Lane chevrons.
    for (k, lx) in [56i64, 136, 378, 458].into_iter().enumerate() {
        let col = palette_color(meta, k + 1);
        let start = 360 + i64::from(rng.below(160));
        for j in 0..3 {
            img.chevron(lx, start + j * 34, 16, 7, col, 230);
        }
    }
    // Centre insert ladder.
    let n = 4 + rng.below(3);
    for i in 0..n {
        let y = 540 + i64::from(i) * 46;
        let col = if i % 2 == 0 { c1 } else { c3 };
        img.circle(wi / 2, y, 15, 0, col, 200);
        img.circle(wi / 2, y, 15, 2, WHITE, 160);
    }
    // Slingshot outlines and flipper zone.
    img.line(70, 780, 150, 900, 4, c2, 255);
    img.line(70, 780, 70, 880, 4, c2, 255);
    img.line(wi - 70, 780, wi - 150, 900, 4, c2, 255);
    img.line(wi - 70, 780, wi - 70, 880, 4, c2, 255);
    img.fill_rect(0, hi - 60, wi, hi, BLACK, 140);
    // Title.
    let t = display_title(meta);
    if !t.is_empty() {
        let scale = fit_scale(FONT_8X16, &t, w - 40, 3);
        let tw = i64::from(FONT_8X16.text_width(&t)) * i64::from(scale);
        let x = ((wi - tw) / 2).max(0);
        img.text(FONT_8X16, &t, x, 440, scale, readable(c1, back), BLACK);
    }
    img
}

/// An insert label (RGBA, transparent background): the text tinted from the
/// palette with a black outline, on one or two lines at the largest scale
/// that fits.
pub fn insert_label(meta: &TableMeta, text: &str) -> Image {
    let (w, h) = INSERT_LABEL_SIZE;
    let mut img = Image::new(w, h, ColorType::Rgba);
    let font = FONT_5X7;
    let one_line = vec![text.to_owned()];
    let two_lines: Vec<String> = match text.split_once(' ') {
        Some((a, b)) => vec![a.to_owned(), b.to_owned()],
        None => one_line.clone(),
    };
    let avail_w = w - 4;
    let avail_h = h - 4;
    let fits = |lines: &[String], s: u32| {
        let lw = lines.iter().map(|l| font.text_width(l)).max().unwrap_or(0);
        let lh = lines.len() as u32 * font.height() + (lines.len() as u32 - 1) * 2;
        u64::from(lw) * u64::from(s) <= u64::from(avail_w) && lh * s <= avail_h
    };
    let mut choice = (one_line.clone(), 1);
    'outer: for s in (1..=3).rev() {
        for lines in [&one_line, &two_lines] {
            if fits(lines, s) {
                choice = (lines.clone(), s);
                break 'outer;
            }
        }
    }
    let (lines, s) = choice;
    let si = i64::from(s);
    let line_h = (i64::from(font.height()) + 2) * si;
    let total = line_h * lines.len() as i64 - 2 * si;
    let mut y = (i64::from(h) - total) / 2;
    let fg = mix(palette_color(meta, 1), WHITE, 80);
    for l in &lines {
        let lw = i64::from(font.text_width(l)) * si;
        img.text(font, l, ((i64::from(w) - lw) / 2).max(0), y, s, fg, BLACK);
        y += line_h;
    }
    img
}

/// A sheet of every glyph of `font` (greyscale): 16 columns × 6 rows of
/// cells for code points 32–126 then the fallback glyph, one blank pixel of
/// grid gap.
pub fn font_sheet(font: &BitmapFont) -> Image {
    let (cw, ch) = (font.width() + GLYPH_SPACING, font.height() + 1);
    let (w, h) = (16 * cw + 1, 6 * ch + 1);
    let mut img = Image::new(w, h, ColorType::Gray);
    let chars = (crate::fonts::FIRST_CHAR..=crate::fonts::LAST_CHAR)
        .filter_map(char::from_u32)
        .map(|c| font.glyph_or_fallback(c))
        .chain(core::iter::once(font.fallback()));
    for (i, g) in chars.enumerate() {
        let (x0, y0) = (1 + (i as u32 % 16) * cw, 1 + (i as u32 / 16) * ch);
        for y in 0..g.height() {
            for x in 0..g.width() {
                if g.pixel(x, y) {
                    let o = ((y0 + y) * w + x0 + x) as usize;
                    img.data[o] = 255;
                }
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(title: &str, palette: Vec<[u8; 3]>, seed: u64) -> TableMeta {
        TableMeta {
            title: title.into(),
            palette,
            seed,
        }
    }

    #[test]
    fn blend_rules() {
        let mut g = Image::new(2, 1, ColorType::Gray);
        g.blend(0, 0, WHITE, 255);
        g.blend(5, 5, WHITE, 255);
        assert_eq!(g.data, [255, 0]);
        let mut a = Image::new(1, 1, ColorType::Rgba);
        a.blend(0, 0, [10, 20, 30], 0);
        assert_eq!(a.data, [0, 0, 0, 0]);
        a.blend(0, 0, [10, 20, 30], 255);
        assert_eq!(a.data, [10, 20, 30, 255]);
        a.blend(0, 0, [210, 220, 230], 128);
        assert_eq!(a.data[3], 255);
        assert!(a.data[0] > 100 && a.data[0] < 120);
    }

    #[test]
    fn helpers() {
        assert_eq!(luma(WHITE), 255);
        assert_eq!(luma(BLACK), 0);
        assert_eq!(mix(BLACK, WHITE, 255), WHITE);
        assert_eq!(readable(WHITE, BLACK), WHITE);
        assert_eq!(readable([20, 20, 20], BLACK), WHITE);
        assert_eq!(readable([230, 230, 230], WHITE), BLACK);
        let m = meta("", vec![], 0);
        assert_eq!(palette_color(&m, 4), DEFAULT_PALETTE[1]);
        let m = meta("", vec![[1, 2, 3]], 0);
        assert_eq!(palette_color(&m, 4), [1, 2, 3]);
    }

    #[test]
    fn art_depends_on_meta() {
        let a = meta("Apex Circuit", DEFAULT_PALETTE.to_vec(), 1);
        let b = meta("Apex Circuit", DEFAULT_PALETTE.to_vec(), 2);
        let c = meta("Other", DEFAULT_PALETTE.to_vec(), 1);
        assert_eq!(translite_base(&a), translite_base(&a));
        assert_ne!(translite_base(&a), translite_base(&b));
        assert_ne!(translite_base(&a), translite_base(&c));
        assert_ne!(playfield(&a), playfield(&b));
        assert_ne!(translite_logo(&a), translite_logo(&c));
        // The mask covers the lit layer exactly.
        let logo = translite_logo(&a);
        let mask = translite_logo_mask(&a);
        for (i, &m) in mask.data.iter().enumerate() {
            assert_eq!(m != 0, logo.data[i * 4 + 3] != 0);
        }
        assert!(mask.data.iter().any(|&m| m == 255));
    }

    #[test]
    fn empty_and_long_titles() {
        let e = meta("", vec![], u64::MAX);
        assert!(translite_logo_mask(&e).data.iter().all(|&m| m == 0));
        let long = meta(&"Ünïcødé ".repeat(40), vec![[255, 255, 255]], 3);
        assert!(translite_base(&long).to_png().is_some());
        assert!(playfield(&long).to_png().is_some());
    }

    #[test]
    fn labels_fit_and_are_visible() {
        let m = meta("x", vec![], 0);
        for (_, text) in INSERT_LABELS {
            let img = insert_label(&m, text);
            let lit = img.data.chunks_exact(4).filter(|p| p[3] != 0).count();
            assert!(lit > 20, "{text}");
            // Nothing touches the 1-pixel border (text fits).
            for x in 0..img.width as usize {
                assert_eq!(img.data[x * 4 + 3], 0, "{text}");
            }
        }
        // Over-long text clips instead of failing.
        let img = insert_label(&m, "A VERY LONG LABEL THAT CANNOT FIT");
        assert!(img.data.chunks_exact(4).any(|p| p[3] != 0));
    }

    #[test]
    fn font_sheets() {
        let s = font_sheet(FONT_5X7);
        assert_eq!((s.width, s.height), (97, 49));
        let b = font_sheet(FONT_8X16);
        assert_eq!((b.width, b.height), (145, 103));
        assert!(b.to_png().is_some());
    }
}
