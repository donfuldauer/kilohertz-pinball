//! Bitmap DMD fonts defined in code: 5×7 and 8×16, ASCII 32–126 (4.6
//! deliverable 3), plus a text renderer for the DMD scene layer (5.3).
//!
//! * [`FONT_5X7`]: the classic 5×7 dot-matrix forms ([`data`]).
//! * [`FONT_8X16`]: derived from the 5×7 forms at compile time: every row is
//!   doubled (14 rows, one blank row above and below) and every column is
//!   drawn two pixels wide (a bold 6-pixel body centred in 8 columns). This is
//!   the usual "large" DMD style and keeps both sizes visually consistent.
//!
//! Characters outside 32–126 have no glyph ([`BitmapFont::glyph`] returns
//! `None`); [`render_text`] draws them as the font's fallback glyph, a hollow
//! box filling the glyph cell ([`BitmapFont::fallback`]).

mod data;

use data::GLYPHS_5X7;

/// First code point with a glyph (space).
pub const FIRST_CHAR: u32 = 32;
/// Last code point with a glyph (`~`).
pub const LAST_CHAR: u32 = 126;
/// Columns of blank spacing [`render_text`] puts between two glyphs.
pub const GLYPH_SPACING: u32 = 1;

/// Rows of every glyph cell; unused rows are zero. Bit `width - 1 - x` of a
/// row is column `x`.
type Rows = [u8; 16];

/// A fixed-cell bitmap font.
#[derive(Debug)]
pub struct BitmapFont {
    name: &'static str,
    width: u32,
    height: u32,
    glyphs: [Rows; 95],
    fallback: Rows,
}

/// One glyph cell of a [`BitmapFont`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glyph {
    width: u32,
    height: u32,
    rows: Rows,
}

impl Glyph {
    /// Cell width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Cell height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Whether pixel `(x, y)` (origin top left) is lit; `false` outside the cell.
    pub fn pixel(&self, x: u32, y: u32) -> bool {
        if x >= self.width || y >= self.height {
            return false;
        }
        (self.rows[y as usize] >> (self.width - 1 - x)) & 1 == 1
    }

    /// Whether any pixel is lit.
    pub fn is_blank(&self) -> bool {
        self.rows.iter().all(|&r| r == 0)
    }
}

impl BitmapFont {
    /// Short name (`"5x7"`, `"8x16"`).
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// Cell width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Cell height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The glyph of `c`, or `None` outside ASCII 32–126.
    pub fn glyph(&self, c: char) -> Option<Glyph> {
        let code = c as u32;
        if !(FIRST_CHAR..=LAST_CHAR).contains(&code) {
            return None;
        }
        Some(self.make(self.glyphs[(code - FIRST_CHAR) as usize]))
    }

    /// The glyph drawn for characters without one: a hollow box.
    pub fn fallback(&self) -> Glyph {
        self.make(self.fallback)
    }

    /// [`glyph`](Self::glyph), or the [`fallback`](Self::fallback).
    pub fn glyph_or_fallback(&self, c: char) -> Glyph {
        self.glyph(c).unwrap_or_else(|| self.fallback())
    }

    /// Width in pixels of `text` as [`render_text`] lays it out.
    pub fn text_width(&self, text: &str) -> u32 {
        let n = text.chars().count() as u64;
        if n == 0 {
            return 0;
        }
        let w = n * u64::from(self.width) + (n - 1) * u64::from(GLYPH_SPACING);
        u32::try_from(w).unwrap_or(u32::MAX)
    }

    fn make(&self, rows: Rows) -> Glyph {
        Glyph {
            width: self.width,
            height: self.height,
            rows,
        }
    }
}

const BOX_5X7: [u8; 7] = [
    0b11111, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11111,
];

const fn pad_5x7(src: [u8; 7]) -> Rows {
    let mut out = [0u8; 16];
    let mut y = 0;
    while y < 7 {
        out[y] = src[y];
        y += 1;
    }
    out
}

/// 5×7 → 8×16: rows doubled into rows 1..=14, each source column `x` drawn at
/// columns `x + 1` and `x + 2`.
const fn big_from_5x7(src: [u8; 7]) -> Rows {
    let mut out = [0u8; 16];
    let mut y = 0;
    while y < 7 {
        let mut row = 0u8;
        let mut x = 0;
        while x < 5 {
            if (src[y] >> (4 - x)) & 1 == 1 {
                // Column c is bit 7 - c.
                row |= 1 << (7 - (x + 1));
                row |= 1 << (7 - (x + 2));
            }
            x += 1;
        }
        out[1 + 2 * y] = row;
        out[2 + 2 * y] = row;
        y += 1;
    }
    out
}

const fn all_5x7() -> [Rows; 95] {
    let mut out = [[0u8; 16]; 95];
    let mut i = 0;
    while i < 95 {
        out[i] = pad_5x7(GLYPHS_5X7[i]);
        i += 1;
    }
    out
}

const fn all_8x16() -> [Rows; 95] {
    let mut out = [[0u8; 16]; 95];
    let mut i = 0;
    while i < 95 {
        out[i] = big_from_5x7(GLYPHS_5X7[i]);
        i += 1;
    }
    out
}

/// A hollow box filling the 8×16 cell.
const fn box_8x16() -> Rows {
    let mut out = [0b1000_0001u8; 16];
    out[0] = 0xff;
    out[15] = 0xff;
    out
}

static FONT_5X7_DATA: BitmapFont = BitmapFont {
    name: "5x7",
    width: 5,
    height: 7,
    glyphs: all_5x7(),
    fallback: pad_5x7(BOX_5X7),
};

static FONT_8X16_DATA: BitmapFont = BitmapFont {
    name: "8x16",
    width: 8,
    height: 16,
    glyphs: all_8x16(),
    fallback: box_8x16(),
};

/// The 5×7 DMD font.
pub static FONT_5X7: &BitmapFont = &FONT_5X7_DATA;
/// The 8×16 DMD font.
pub static FONT_8X16: &BitmapFont = &FONT_8X16_DATA;

/// Renders `text` on one line into `out` (cleared first), one byte per pixel,
/// `0` (off) or `255` (on), row-major, and returns `(width, height)`.
///
/// Glyphs are [`GLYPH_SPACING`] columns apart; the height is the font height
/// (also for empty text, whose width is 0). Every character, including control
/// characters and `'\n'`, takes one cell; characters without a glyph draw the
/// fallback box. Text wider than `u32::MAX` pixels is cut at the last whole
/// glyph that fits.
pub fn render_text(font: &BitmapFont, text: &str, out: &mut Vec<u8>) -> (u32, u32) {
    out.clear();
    let w = font.text_width(text);
    let h = font.height;
    let cell = font.width + GLYPH_SPACING;
    // Whole glyphs that fit into `w` (only less than all for absurdly long text).
    let fits = (u64::from(w) + u64::from(GLYPH_SPACING)) / u64::from(cell);
    let w = if fits == 0 {
        0
    } else {
        (fits * u64::from(cell) - u64::from(GLYPH_SPACING)) as u32
    };
    out.resize(w as usize * h as usize, 0);
    for (i, c) in text.chars().take(fits as usize).enumerate() {
        let g = font.glyph_or_fallback(c);
        let x0 = i as u32 * cell;
        for y in 0..h {
            let row = y as usize * w as usize;
            for x in 0..font.width {
                if g.pixel(x, y) {
                    out[row + (x0 + x) as usize] = 255;
                }
            }
        }
    }
    (w, h)
}

/// Draws `text` into a `buf_w`×`buf_h` single-byte buffer at `(x, y)` (top
/// left of the first glyph; may be negative or off the buffer: pixels outside
/// are clipped), scaled by `scale` (≥ 1; 0 draws nothing), setting lit pixels
/// to `value`. For compositing DMD frames and the kit's generated art.
#[allow(clippy::too_many_arguments)]
pub fn blit_text(
    font: &BitmapFont,
    text: &str,
    buf: &mut [u8],
    buf_w: u32,
    buf_h: u32,
    x: i64,
    y: i64,
    scale: u32,
    value: u8,
) {
    if scale == 0 || buf.len() < buf_w as usize * buf_h as usize {
        return;
    }
    let s = i64::from(scale);
    let cell = i64::from(font.width + GLYPH_SPACING) * s;
    for (i, c) in text.chars().enumerate() {
        let gx = x + i as i64 * cell;
        if gx >= i64::from(buf_w) {
            break;
        }
        if gx + i64::from(font.width) * s <= 0 {
            continue;
        }
        let g = font.glyph_or_fallback(c);
        for py in 0..font.height {
            for px in 0..font.width {
                if !g.pixel(px, py) {
                    continue;
                }
                for dy in 0..s {
                    let yy = y + i64::from(py) * s + dy;
                    if yy < 0 || yy >= i64::from(buf_h) {
                        continue;
                    }
                    for dx in 0..s {
                        let xx = gx + i64::from(px) * s + dx;
                        if xx >= 0 && xx < i64::from(buf_w) {
                            buf[(yy * i64::from(buf_w) + xx) as usize] = value;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printable() -> impl Iterator<Item = char> {
        (FIRST_CHAR..=LAST_CHAR).map(|c| char::from_u32(c).unwrap())
    }

    #[test]
    fn full_range_both_sizes() {
        for (font, w, h) in [(FONT_5X7, 5, 7), (FONT_8X16, 8, 16)] {
            assert_eq!((font.width(), font.height()), (w, h));
            for c in printable() {
                let g = font.glyph(c).unwrap();
                assert_eq!((g.width(), g.height()), (w, h));
                assert_eq!(g.is_blank(), c == ' ', "{c:?} in {}", font.name());
            }
            assert!(font.glyph('\u{1f}').is_none());
            assert!(font.glyph('\u{7f}').is_none());
            assert!(font.glyph('é').is_none());
            assert!(!font.fallback().is_blank());
            assert_eq!(font.glyph_or_fallback('\n'), font.fallback());
        }
    }

    #[test]
    fn glyphs_are_distinct() {
        for font in [FONT_5X7, FONT_8X16] {
            let mut seen = std::collections::BTreeMap::new();
            for c in printable() {
                if let Some(prev) = seen.insert(font.glyph(c).unwrap().rows, c) {
                    panic!("{c:?} and {prev:?} share a glyph in {}", font.name());
                }
            }
            // The fallback is not any printable glyph.
            assert!(!seen.contains_key(&font.fallback().rows));
        }
    }

    #[test]
    fn glyph_rows_stay_in_cell() {
        for font in [FONT_5X7, FONT_8X16] {
            for c in printable() {
                let g = font.glyph(c).unwrap();
                for (y, &r) in g.rows.iter().enumerate() {
                    if y as u32 >= g.height() {
                        assert_eq!(r, 0);
                    }
                    if g.width() < 8 {
                        assert_eq!(r >> g.width(), 0);
                    }
                }
                assert!(!g.pixel(g.width(), 0));
                assert!(!g.pixel(0, g.height()));
            }
        }
    }

    #[test]
    fn big_font_matches_small_font() {
        for c in printable() {
            let s = FONT_5X7.glyph(c).unwrap();
            let b = FONT_8X16.glyph(c).unwrap();
            for y in 0..16 {
                for x in 0..8 {
                    let expect = (1..=14).contains(&y)
                        && ((1..=5).contains(&x) && s.pixel(x - 1, (y - 1) / 2)
                            || (2..=6).contains(&x) && s.pixel(x - 2, (y - 1) / 2));
                    assert_eq!(b.pixel(x, y), expect, "{c:?} ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn render_layout() {
        let mut out = vec![7; 3];
        assert_eq!(render_text(FONT_5X7, "", &mut out), (0, 7));
        assert!(out.is_empty());
        assert_eq!(render_text(FONT_5X7, "I", &mut out), (5, 7));
        assert_eq!(out.len(), 35);
        // 'I' is a centre column with serifs.
        assert_eq!(out[2], 255);
        assert_eq!(out[0], 0);
        assert_eq!(render_text(FONT_8X16, "AB", &mut out), (17, 16));
        assert!(out.iter().all(|&v| v == 0 || v == 255));
        // Spacing column is blank.
        assert!((0..16).all(|y| out[y * 17 + 8] == 0));
        // Fallback for non-ASCII.
        assert_eq!(render_text(FONT_5X7, "é", &mut out), (5, 7));
        assert_eq!(&out[0..5], &[255; 5]);
    }

    #[test]
    fn blit_clips_and_scales() {
        let mut buf = vec![0u8; 20 * 10];
        blit_text(FONT_5X7, "HI", &mut buf, 20, 10, -3, -2, 2, 9);
        assert!(buf.contains(&9));
        assert!(buf.iter().all(|&v| v == 0 || v == 9));
        let mut buf2 = vec![0u8; 20 * 10];
        blit_text(FONT_5X7, "HI", &mut buf2, 20, 10, 0, 0, 0, 9);
        blit_text(FONT_5X7, "HI", &mut buf2, 20, 10, 100, 100, 1, 9);
        blit_text(FONT_5X7, "HI", &mut buf2, 20, 10, -100, 0, 1, 9);
        blit_text(FONT_5X7, "HI", &mut buf2[..10], 20, 10, 0, 0, 1, 9);
        assert!(buf2.iter().all(|&v| v == 0));
        // Unscaled blit equals render_text.
        let mut r = Vec::new();
        let (w, h) = render_text(FONT_8X16, "Kz~", &mut r);
        let mut b = vec![0u8; (w * h) as usize];
        blit_text(FONT_8X16, "Kz~", &mut b, w, h, 0, 0, 1, 255);
        assert_eq!(r, b);
    }
}
