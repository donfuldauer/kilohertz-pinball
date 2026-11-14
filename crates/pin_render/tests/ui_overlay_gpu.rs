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
    OverlayFrame, OverlayPainter, egui, menu_panel_rect_px, overlay_style, paint_offscreen,
    paint_offscreen_with, raw_input, show_overlay,
};
use std::sync::OnceLock;

/// Bright, warm "playfield" (written as-is into Rgba8Unorm).
///
/// Every channel is an exact `k / 255`, so the clear colour converts to the
/// same byte on every backend. (Wave 5 gate fix: it was 0.9, 0.85, 0.6, and
/// `0.9_f32 · 255` = 229.49999 sits on a rounding boundary; Metal stored 229
/// where lavapipe stored 230, so every background pixel differed from
/// `bg_u8()` and the panel's rounded corners counted as "bright" glyph blobs.)
const BG: [f32; 4] = [230.0 / 255.0, 217.0 / 255.0, 153.0 / 255.0, 1.0];

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
            // Bright and grey: white text over the near-neutral backdrop.
            // The panel's rounded corners leave playfield pixels inside the
            // rect; those are bright too but strongly coloured (blue 77 below
            // red), so they never count, whatever byte the backend's clear
            // rounds the background to.
            let p = &px[i * 4..i * 4 + 4];
            let spread = p[..3].iter().max().unwrap() - p[..3].iter().min().unwrap();
            m[i] = luma(p) > 128.0 && spread < 32;
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

/// Later frames of one `egui::Context` do not resend the font atlas, so
/// they need the painter that saw the first frame: with a reused painter
/// the second frame still shows its text.
#[test]
fn second_frame_of_a_context_keeps_its_text() {
    let Some(gpu) = ctx() else { return };
    let dims = [480, 240];
    let panel = menu_panel_rect_px(dims, 1.0);
    let ectx = egui::Context::default();
    overlay_style(&ectx);
    let mut painter = OverlayPainter::new(
        gpu.device(),
        pin_render::gpu::wgpu::TextureFormat::Rgba8Unorm,
    );
    let mut lit = Vec::new();
    for frame in 0..3 {
        let out = ectx.run_ui(raw_input(dims, 1.0, Vec::new(), None), |ui| {
            show_overlay(ui, |ui| {
                ui.label(
                    egui::RichText::new("oooo")
                        .size(40.0)
                        .color(egui::Color32::WHITE),
                );
            });
        });
        if frame > 0 {
            assert!(
                out.textures_delta.set.is_empty(),
                "frame {frame} resent a texture: the test would not show the reuse"
            );
        }
        let px = paint_offscreen_with(
            gpu,
            &mut painter,
            dims[0],
            dims[1],
            BG,
            OverlayFrame::new(&ectx, out),
        )
        .unwrap();
        lit.push(bright_mask(&px, dims, panel).iter().filter(|v| **v).count());
    }
    assert!(lit[0] > 100, "no text in the first frame: {lit:?}");
    assert_eq!(lit[1], lit[0], "text lost in the second frame: {lit:?}");
    assert_eq!(lit[2], lit[0], "{lit:?}");
    // A painter of another format is refused.
    let mut bgra = OverlayPainter::new(
        gpu.device(),
        pin_render::gpu::wgpu::TextureFormat::Bgra8Unorm,
    );
    assert!(paint_offscreen_with(gpu, &mut bgra, 8, 8, BG, menu_frame([8, 8], 1.0)).is_err());
}

/// A rejected paint (zero size) must not lose the frame's texture deltas:
/// the painter keeps them, and the next valid paint of the same context
/// (which does not resend the font atlas) still shows its text.
#[test]
fn rejected_frame_keeps_its_textures_for_the_next_paint() {
    let Some(gpu) = ctx() else { return };
    let dims = [480, 240];
    let panel = menu_panel_rect_px(dims, 1.0);
    let ectx = egui::Context::default();
    overlay_style(&ectx);
    let mut painter = OverlayPainter::new(
        gpu.device(),
        pin_render::gpu::wgpu::TextureFormat::Rgba8Unorm,
    );
    let run = || {
        ectx.run_ui(raw_input(dims, 1.0, Vec::new(), None), |ui| {
            show_overlay(ui, |ui| {
                ui.label(
                    egui::RichText::new("oooo")
                        .size(40.0)
                        .color(egui::Color32::WHITE),
                );
            });
        })
    };
    let first = run();
    assert!(
        !first.textures_delta.set.is_empty(),
        "the first frame sent no texture: the test would not show the loss"
    );
    let rejected = paint_offscreen_with(
        gpu,
        &mut painter,
        0,
        dims[1],
        BG,
        OverlayFrame::new(&ectx, first),
    );
    assert!(rejected.is_err());
    let second = run();
    assert!(
        second.textures_delta.set.is_empty(),
        "the second frame resent a texture: the test would not show the recovery"
    );
    let px = paint_offscreen_with(
        gpu,
        &mut painter,
        dims[0],
        dims[1],
        BG,
        OverlayFrame::new(&ectx, second),
    )
    .unwrap();
    let lit = bright_mask(&px, dims, panel).iter().filter(|v| **v).count();
    assert!(lit > 100, "text lost after a rejected frame: {lit} lit px");
}

/// A paint refused for the painter's format keeps the frame's texture deltas
/// in that painter; `take_deferred` moves them to a replacement painter of
/// the right format, whose first paint (of a later frame, which does not
/// resend the font atlas) still shows its text.
#[test]
fn wrong_format_rejection_hands_its_textures_to_a_replacement_painter() {
    let Some(gpu) = ctx() else { return };
    let dims = [480, 240];
    let panel = menu_panel_rect_px(dims, 1.0);
    let ectx = egui::Context::default();
    overlay_style(&ectx);
    let run = || {
        ectx.run_ui(raw_input(dims, 1.0, Vec::new(), None), |ui| {
            show_overlay(ui, |ui| {
                ui.label(
                    egui::RichText::new("oooo")
                        .size(40.0)
                        .color(egui::Color32::WHITE),
                );
            });
        })
    };
    let mut bgra = OverlayPainter::new(
        gpu.device(),
        pin_render::gpu::wgpu::TextureFormat::Bgra8Unorm,
    );
    let first = run();
    assert!(
        !first.textures_delta.set.is_empty(),
        "the first frame sent no texture: the test would not show the loss"
    );
    let rejected = paint_offscreen_with(
        gpu,
        &mut bgra,
        dims[0],
        dims[1],
        BG,
        OverlayFrame::new(&ectx, first),
    );
    assert!(rejected.is_err());
    let mut rgba = OverlayPainter::new(
        gpu.device(),
        pin_render::gpu::wgpu::TextureFormat::Rgba8Unorm,
    );
    rgba.defer(bgra.take_deferred());
    assert!(
        bgra.take_deferred().textures_delta.set.is_empty(),
        "take_deferred left deltas behind"
    );
    let second = run();
    assert!(
        second.textures_delta.set.is_empty(),
        "the second frame resent a texture: the test would not show the recovery"
    );
    let px = paint_offscreen_with(
        gpu,
        &mut rgba,
        dims[0],
        dims[1],
        BG,
        OverlayFrame::new(&ectx, second),
    )
    .unwrap();
    let lit = bright_mask(&px, dims, panel).iter().filter(|v| **v).count();
    assert!(
        lit > 100,
        "text lost after a wrong-format rejection: {lit} lit px"
    );
}

/// A texture uploaded by a deferred (rejected) frame and drawn for the last
/// time by the next painted frame, which also frees it: the painter must
/// upload it before drawing and free it only afterwards. (Merging the two
/// texture deltas with `TexturesDelta::append` dropped the upload.)
#[test]
fn deferred_upload_freed_by_the_next_painted_frame_is_still_drawn() {
    let Some(gpu) = ctx() else { return };
    let dims = [64, 64];
    let ectx = egui::Context::default();
    let mut painter = OverlayPainter::new(
        gpu.device(),
        pin_render::gpu::wgpu::TextureFormat::Rgba8Unorm,
    );
    let rect = egui::Rect::from_min_max(egui::pos2(16.0, 16.0), egui::pos2(48.0, 48.0));
    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let mut handle: Option<egui::TextureHandle> = None;
    let first = ectx.run_ui(raw_input(dims, 1.0, Vec::new(), None), |ui| {
        let tex = ui.ctx().load_texture(
            "red",
            egui::ColorImage::filled([4, 4], egui::Color32::RED),
            egui::TextureOptions::NEAREST,
        );
        ui.painter().image(tex.id(), rect, uv, egui::Color32::WHITE);
        handle = Some(tex);
    });
    let id = handle.as_ref().unwrap().id();
    assert!(
        first.textures_delta.set.contains_key(&id),
        "the first frame did not upload the texture"
    );
    assert!(
        paint_offscreen_with(gpu, &mut painter, 0, 0, BG, OverlayFrame::new(&ectx, first)).is_err()
    );
    let second = ectx.run_ui(raw_input(dims, 1.0, Vec::new(), None), |ui| {
        ui.painter().image(id, rect, uv, egui::Color32::WHITE);
        handle = None; // last use: egui frees the texture in this frame
    });
    assert!(
        second.textures_delta.free.contains(&id),
        "the second frame did not free the texture: the test would not show the loss"
    );
    let px = paint_offscreen_with(
        gpu,
        &mut painter,
        dims[0],
        dims[1],
        BG,
        OverlayFrame::new(&ectx, second),
    )
    .unwrap();
    let at = |x: usize, y: usize| &px[(y * dims[0] as usize + x) * 4..][..4];
    let c = at(32, 32);
    assert!(
        c[0] > 200 && c[1] < 60 && c[2] < 60,
        "deferred texture not drawn: centre pixel {c:?}"
    );
    assert_eq!(at(2, 2), bg_u8(), "outside the image");
}
