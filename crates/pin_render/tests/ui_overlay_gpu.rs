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
    OverlayFrame, egui, menu_panel_rect_px, overlay_style, paint_offscreen, raw_input,
    show_overlay,
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
    assert!(s.inside_bright > 50, "only {} bright pixels", s.inside_bright);
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
    assert!(s.inside_bright > 200, "only {} bright pixels", s.inside_bright);
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
