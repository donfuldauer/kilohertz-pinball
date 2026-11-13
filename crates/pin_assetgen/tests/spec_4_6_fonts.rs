//! Spec tests, Subphase 4.6 — bitmap DMD fonts (brief: deliverable 3, FIXED
//! PUBLIC API `fonts::*`, verification 4).

mod spec_4_6_support;

use pin_assetgen::fonts::{BitmapFont, FONT_5X7, FONT_8X16, render_text};
use proptest::prelude::*;
use std::collections::BTreeSet;

fn fonts() -> [(&'static str, &'static BitmapFont, usize, usize); 2] {
    let a: &'static BitmapFont = &FONT_5X7;
    let b: &'static BitmapFont = &FONT_8X16;
    [("5x7", a, 5, 7), ("8x16", b, 8, 16)]
}

/// Row-major glyph bitmap.
fn bitmap(font: &BitmapFont, c: char) -> Option<Vec<bool>> {
    let g = font.glyph(c)?;
    let mut v = Vec::new();
    for y in 0..g.height() {
        for x in 0..g.width() {
            v.push(g.pixel(x, y));
        }
    }
    Some(v)
}

fn lit(font: &BitmapFont, c: char) -> usize {
    bitmap(font, c).unwrap().iter().filter(|&&b| b).count()
}

fn render(font: &BitmapFont, text: &str) -> (usize, usize, Vec<u8>) {
    let mut out = Vec::new();
    let (w, h) = render_text(font, text, &mut out);
    (w as usize, h as usize, out)
}

/// Lit pixels cropped to their bounding box: (w, h, row-major bits).
fn crop(w: usize, h: usize, px: &[bool]) -> (usize, usize, Vec<bool>) {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if px[y * w + x] {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    assert!(x0 != usize::MAX, "nothing lit");
    let mut out = Vec::new();
    for y in y0..=y1 {
        for x in x0..=x1 {
            out.push(px[y * w + x]);
        }
    }
    (x1 - x0 + 1, y1 - y0 + 1, out)
}

#[test]
fn every_printable_ascii_glyph_is_present_with_the_declared_size() {
    for (name, font, w, h) in fonts() {
        for code in 32u8..=126 {
            let c = code as char;
            let g = font
                .glyph(c)
                .unwrap_or_else(|| panic!("{name}: glyph {c:?} missing"));
            assert_eq!(g.width() as usize, w, "{name} {c:?} width");
            assert_eq!(g.height() as usize, h, "{name} {c:?} height");
            assert_eq!(bitmap(font, c).unwrap().len(), w * h);
        }
    }
}

#[test]
fn space_is_blank_and_every_other_glyph_is_not() {
    for (name, font, _, _) in fonts() {
        assert_eq!(lit(font, ' '), 0, "{name}: space is not blank");
        for code in 33u8..=126 {
            assert!(lit(font, code as char) > 0, "{name}: {:?} is blank", code as char);
        }
    }
}

#[test]
fn glyphs_are_legible_as_different_characters() {
    // A font, not 94 copies of a box: allow a handful of look-alikes
    // (O/0, l/I/|), nothing more.
    for (name, font, _, _) in fonts() {
        let distinct: BTreeSet<Vec<bool>> = (32u8..=126)
            .map(|c| bitmap(font, c as char).unwrap())
            .collect();
        assert!(distinct.len() >= 90, "{name}: only {} distinct glyphs", distinct.len());
        for (a, b) in [('A', 'B'), ('0', '1'), ('a', 'A'), ('.', ','), ('+', '-'), ('8', '3')] {
            assert_ne!(bitmap(font, a), bitmap(font, b), "{name}: {a:?} == {b:?}");
        }
    }
}

#[test]
fn render_text_buffer_is_w_times_h_bytes_of_0_or_255() {
    for (name, font, gw, gh) in fonts() {
        let all: String = (32u8..=126).map(|c| c as char).collect();
        for text in ["A", "HELLO", "Score 1,234,560", all.as_str()] {
            let (w, h, out) = render(font, text);
            let n = text.chars().count();
            assert_eq!(out.len(), w * h, "{name} {text:?}");
            assert!(out.iter().all(|&b| b == 0 || b == 255), "{name} {text:?}");
            assert!(h >= gh, "{name} {text:?}: height {h}");
            assert!(w >= n * gw, "{name} {text:?}: width {w} for {n} glyphs");
            let expect: usize = text.chars().map(|c| lit(font, c)).sum();
            let got = out.iter().filter(|&&b| b == 255).count();
            assert_eq!(got, expect, "{name} {text:?}: lit pixel count");
        }
    }
}

#[test]
fn render_text_draws_the_glyph_row_major() {
    for (name, font, gw, gh) in fonts() {
        for c in ['A', 'g', '#', '7', '~', 'Q'] {
            let (w, h, out) = render(font, &c.to_string());
            let px: Vec<bool> = out.iter().map(|&b| b == 255).collect();
            let glyph = bitmap(font, c).unwrap();
            assert_eq!(crop(w, h, &px), crop(gw, gh, &glyph), "{name} {c:?}");
        }
    }
}

#[test]
fn render_text_width_grows_with_the_text_and_is_deterministic() {
    for (name, font, _, _) in fonts() {
        let mut last = 0;
        for n in 1..=12 {
            let text = "W".repeat(n);
            let (w, _, out) = render(font, &text);
            assert!(w > last, "{name}: width of {n} glyphs is {w}, of {} was {last}", n - 1);
            last = w;
            assert_eq!(render(font, &text).2, out, "{name}: not deterministic");
        }
    }
}

#[test]
fn spaces_and_empty_text_render_nothing_lit() {
    for (name, font, _, _) in fonts() {
        for text in ["", " ", "     "] {
            let (w, h, out) = render(font, text);
            assert_eq!(out.len(), w * h, "{name} {text:?}");
            assert!(out.iter().all(|&b| b == 0), "{name} {text:?}: lit pixel");
        }
    }
}

#[test]
fn characters_outside_ascii_32_126_render_as_one_defined_fallback() {
    for (name, font, gw, gh) in fonts() {
        let (w0, h0, first) = render(font, "\u{e9}");
        assert!(w0 >= gw && h0 >= gh, "{name}: fallback is {w0}x{h0}");
        assert_eq!(first.len(), w0 * h0);
        assert!(first.iter().all(|&b| b == 0 || b == 255));
        // The same fallback whatever the character, every time.
        for c in ['\u{e9}', '\u{20ac}', '\u{6f22}', '\u{1f3b1}', '\u{a0}', '\u{ff}'] {
            let _ = font.glyph(c); // must not panic; Some or None are both defined
            let (w, h, out) = render(font, &c.to_string());
            assert_eq!((w, h), (w0, h0), "{name} {c:?}");
            assert_eq!(out, first, "{name}: {c:?} renders differently from U+00E9");
        }
        // A fallback takes one cell: mixed text is as wide as plain text.
        let (wa, _, _) = render(font, "AAA");
        let (wm, _, out) = render(font, "A\u{e9}A");
        assert_eq!(wm, wa, "{name}: fallback cell width");
        assert!(out.iter().filter(|&&b| b == 255).count() >= 2 * lit(font, 'A'));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn any_text_renders_without_panic(text in "\\PC{0,40}") {
        for (_, font, _, gh) in fonts() {
            let (w, h, out) = render(font, &text);
            prop_assert_eq!(out.len(), w * h);
            prop_assert!(out.iter().all(|&b| b == 0 || b == 255));
            if !text.is_empty() {
                prop_assert!(h >= gh);
            }
        }
    }

    #[test]
    fn any_char_glyph_lookup_never_panics(c in any::<char>()) {
        for (_, font, w, h) in fonts() {
            if let Some(g) = font.glyph(c) {
                prop_assert_eq!(g.width() as usize, w);
                prop_assert_eq!(g.height() as usize, h);
            } else {
                prop_assert!(!(' '..='~').contains(&c));
            }
            let _ = render(font, &c.to_string());
        }
    }

    #[test]
    fn printable_ascii_text_lights_exactly_its_glyphs(text in "[ -~]{0,32}") {
        for (_, font, gw, _) in fonts() {
            let (w, h, out) = render(font, &text);
            prop_assert_eq!(out.len(), w * h);
            prop_assert!(w >= text.len() * gw);
            let expect: usize = text.chars().map(|c| lit(font, c)).sum();
            prop_assert_eq!(out.iter().filter(|&&b| b == 255).count(), expect);
        }
    }
}
