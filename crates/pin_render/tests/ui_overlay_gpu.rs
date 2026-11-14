//! Menu overlay smoke test (Subphase 4.4, gpu-tests; D30 analytic checks).
//!
//! Paints the overlay (backdrop + a few widgets) with the egui-wgpu painter
//! over a bright solid "playfield" into an offscreen target and checks
//! analytically that
//! * every pixel outside the menu panel is untouched (exactly the background),
//! * the panel region differs from the background: darkened by the
//!   semi-opaque backdrop (legibility over a bright playfield), with brighter
//!   text pixels on it,
//! * the panel scales with the DPI (`pixels_per_point` 2 doubles it in pixels).
//!
//! Without an adapter every test prints a SKIP note and passes, unless
//! `PIN_REQUIRE_GPU=1` (CI).
#![cfg(feature = "gpu-tests")]

use pin_render::gpu::{GpuContext, context_for_tests};
use pin_render::ui::{
    OverlayFrame, egui, menu_panel_rect_px, overlay_style, paint_offscreen, raw_input, show_overlay,
};
use std::sync::OnceLock;

/// Bright, warm "playfield" (written as-is into Rgba8Unorm).
const BG: [f32; 4] = [0.9, 0.85, 0.6, 1.0];

fn ctx() -> Option<&'static GpuContext> {
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    CTX.get_or_init(|| context_for_tests("ui_overlay_gpu"))
        .as_ref()
}

fn bg_u8() -> [u8; 4] {
    BG.map(|c| (c * 255.0).round() as u8)
}

fn luma(p: &[u8]) -> f64 {
    0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
}

/// Lays out a small menu at `size` px and `ppp`, returns the frame.
fn menu_frame(size: [u32; 2], ppp: f32) -> OverlayFrame {
    let ctx = egui::Context::default();
    overlay_style(&ctx);
    let out = ctx.run_ui(raw_input(size, ppp, Vec::new(), None), |ui| {
        show_overlay(ui, |ui| {
            ui.heading("Kilohertz Pinball");
            ui.label("Tables  Display  Input");
            let _ = ui.button("Play");
            let mut v = 0.5f32;
            ui.add(egui::Slider::new(&mut v, 0.0..=1.0).text("Nudge gain"));
        });
    });
    OverlayFrame::new(&ctx, out)
}

struct Stats {
    outside_changed: usize,
    inside: usize,
    inside_mean_luma: f64,
    inside_bright: usize,
}

fn analyse(px: &[u8], size: [u32; 2], panel: [u32; 4]) -> Stats {
    let bg = bg_u8();
    let [w, h] = size;
    let mut s = Stats {
        outside_changed: 0,
        inside: 0,
        inside_mean_luma: 0.0,
        inside_bright: 0,
    };
    let mut sum = 0.0;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let p = &px[i..i + 4];
            let inside = x >= panel[0] && x < panel[2] && y >= panel[1] && y < panel[3];
            if inside {
                s.inside += 1;
                let l = luma(p);
                sum += l;
                if l > 150.0 && p != bg {
                    s.inside_bright += 1;
                }
            } else if p != bg {
                s.outside_changed += 1;
            }
        }
    }
    s.inside_mean_luma = sum / s.inside.max(1) as f64;
    s
}

#[test]
fn overlay_darkens_panel_and_leaves_the_rest_untouched() {
    let Some(gpu) = ctx() else { return };
    let size = [320, 240];
    let px = paint_offscreen(gpu, size[0], size[1], BG, menu_frame(size, 1.0)).unwrap();
    assert_eq!(px.len(), (size[0] * size[1] * 4) as usize);
    let panel = menu_panel_rect_px(size, 1.0);
    assert_eq!(panel, [32, 24, 288, 216]);
    let s = analyse(&px, size, panel);
    assert_eq!(s.outside_changed, 0, "pixels outside the panel changed");
    let bg_luma = luma(&bg_u8());
    // Backdrop is ~88 % opaque near-black: the panel is far darker on average.
    assert!(
        s.inside_mean_luma < bg_luma * 0.4,
        "panel mean luma {} vs background {bg_luma}",
        s.inside_mean_luma
    );
    // Text and widgets are drawn on the backdrop.
    assert!(
        s.inside_bright > 50,
        "only {} bright pixels",
        s.inside_bright
    );
    // The panel centre row band (backdrop between widgets) is not background.
    let (cx, cy) = (size[0] / 2, panel[3] - 4);
    let i = ((cy * size[0] + cx) * 4) as usize;
    assert_ne!(&px[i..i + 4], &bg_u8(), "panel bottom centre is untouched");
}

#[test]
fn overlay_scales_with_dpi() {
    let Some(gpu) = ctx() else { return };
    let size = [640, 480];
    let px = paint_offscreen(gpu, size[0], size[1], BG, menu_frame(size, 2.0)).unwrap();
    let panel = menu_panel_rect_px(size, 2.0);
    // 640x480 px at 2x = 320x240 points: the same proportions as at 1x.
    assert_eq!(panel, [64, 48, 576, 432]);
    let s = analyse(&px, size, panel);
    assert_eq!(s.outside_changed, 0);
    assert!(s.inside_mean_luma < luma(&bg_u8()) * 0.4);
    // Text at 2x has about four times the lit area of 1x; just require plenty.
    assert!(
        s.inside_bright > 200,
        "only {} bright pixels",
        s.inside_bright
    );
}

#[test]
fn empty_frame_leaves_the_target_untouched() {
    let Some(gpu) = ctx() else { return };
    let ctx = egui::Context::default();
    let out = ctx.run_ui(raw_input([64, 48], 1.0, Vec::new(), None), |_ui| {});
    let frame = OverlayFrame::new(&ctx, out);
    let px = paint_offscreen(gpu, 64, 48, BG, frame).unwrap();
    assert!(px.chunks_exact(4).all(|p| p == bg_u8()));
}

#[test]
fn invalid_size_is_an_error() {
    let Some(gpu) = ctx() else { return };
    assert!(paint_offscreen(gpu, 0, 10, BG, menu_frame([10, 10], 1.0)).is_err());
}

/// Paints only `text` (white, `size` points) in the overlay; returns the
/// pixels and the panel rect.
fn text_image(text: &str, size: f32) -> Option<(Vec<u8>, [u32; 2], [u32; 4])> {
    let gpu = ctx()?;
    let dims = [480, 240];
    let ctx = egui::Context::default();
    overlay_style(&ctx);
    let out = ctx.run_ui(raw_input(dims, 1.0, Vec::new(), None), |ui| {
        show_overlay(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .size(size)
                    .color(egui::Color32::WHITE),
            );
        });
    });
    let px = paint_offscreen(gpu, dims[0], dims[1], BG, OverlayFrame::new(&ctx, out)).unwrap();
    Some((px, dims, menu_panel_rect_px(dims, 1.0)))
}

/// Bright (text) pixels inside the panel.
fn bright_mask(px: &[u8], dims: [u32; 2], panel: [u32; 4]) -> Vec<bool> {
    let mut m = vec![false; (dims[0] * dims[1]) as usize];
    for y in panel[1]..panel[3] {
        for x in panel[0]..panel[2] {
            let i = (y * dims[0] + x) as usize;
            // Bright and not the background (the panel's rounded corners
            // leave background pixels inside the rect).
            let p = &px[i * 4..i * 4 + 4];
            m[i] = luma(p) > 128.0 && p != bg_u8();
        }
    }
    m
}

/// 8-connected components of `mask` with at least `min` pixels.
fn blobs(mask: &[bool], w: u32, min: usize) -> usize {
    let mut seen = vec![false; mask.len()];
    let mut n = 0;
    for start in 0..mask.len() {
        if !mask[start] || seen[start] {
            continue;
        }
        let mut stack = vec![start];
        seen[start] = true;
        let mut size = 0;
        while let Some(i) = stack.pop() {
            size += 1;
            let (x, y) = ((i as u32 % w) as i64, (i as u32 / w) as i64);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (x + dx, y + dy);
                    if nx < 0 || ny < 0 || nx >= i64::from(w) {
                        continue;
                    }
                    let j = (ny * i64::from(w) + nx) as usize;
                    if j < mask.len() && mask[j] && !seen[j] {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        if size >= min {
            n += 1;
        }
    }
    n
}

/// 4.4 review B1: glyphs are real letters, not one placeholder box each.
#[test]
fn text_renders_distinct_glyphs() {
    let Some((a, dims, panel)) = text_image("oooo", 40.0) else {
        return;
    };
    let (b, _, _) = text_image("xxxx", 40.0).unwrap();
    let (ma, mb) = (bright_mask(&a, dims, panel), bright_mask(&b, dims, panel));
    let lit_a = ma.iter().filter(|v| **v).count();
    let lit_b = mb.iter().filter(|v| **v).count();
    assert!(
        lit_a > 100 && lit_b > 100,
        "no text drawn: {lit_a} / {lit_b}"
    );
    let differ = ma.iter().zip(&mb).filter(|(x, y)| x != y).count();
    assert!(
        differ * 4 > lit_a.min(lit_b),
        "same-length strings look alike ({differ} differing px of {lit_a}/{lit_b}): placeholder glyphs?"
    );
    // "i i i": each `i` is a stem and a separate dot, six blobs; a
    // placeholder box per character would give three.
    let (c, _, _) = text_image("i i i", 48.0).unwrap();
    let n = blobs(&bright_mask(&c, dims, panel), dims[0], 4);
    assert_eq!(n, 6, "expected 6 glyph blobs for \"i i i\"");
}
