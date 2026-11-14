//! Independent GPU spec tests for Subphase 5.3 (backglass and dual-mode score display), under
//! the image test policy D30: analytic pixel assertions first; a regression snapshot is
//! compared (and can only be stored) after the analytic assertions of the same image pass.
//!
//! Written from the brief (`docs/briefs/wave5/5.3.md`) and the published contract (DECISIONS
//! "5.3 — Displays public API") without reading the implementation.
//!
//! Snapshots: `tests/fixtures/spec_5_3/<name>_<w>x<h>.rgba8` (raw display RGBA8). Set
//! `SPEC_5_3_BLESS=1` to (re)write them; they are written only after the analytic checks.
#![cfg(feature = "gpu-tests")]
#![allow(clippy::needless_range_loop)] // channel indices read clearer as ranges

use pin_assetgen::fonts::{FONT_5X7, FONT_8X16, blit_text};
use pin_core::LampId;
use pin_core::schema::backglass::DmdColor;
use pin_render::displays::{
    BackglassDesc, BackglassLayer, BackglassRenderer, DMD_BACKGROUND, DMD_DOT_RADIUS,
    DMD_UNLIT_LEVEL, DisplayContent, DisplayRoute, DisplaySet, DmdFrame, DmdRenderer, DmdSpec,
    ImageSet, LAMP_REDRAW_EPSILON, LayerBlend, LayerLamp, LcdDrawList, LcdItem, LcdRenderer,
    LcdSpec, RgbaImage, SceneLibrary, ScoreDisplayConfig, ScoreDisplayModel, ScoreRenderer,
    display_routes, dmd_dot_color, dmd_geometry, dmd_tint, fit_aspect, render_backglass_to_image,
    render_dmd_to_image, render_lcd_to_image,
};
use pin_render::gpu::{GpuContext, context_for_tests, wgpu};
use pin_render::topology::{
    Corner, DisplayInfo, PipPlacement, Rect, SurfaceRole, TopologyMode, plan_topology,
};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

// ---------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------

fn px(img: &RgbaImage, x: u32, y: u32) -> [f32; 3] {
    let p = img.pixel(x, y);
    [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
}

/// Pixel whose centre is nearest to the continuous target position `p`.
fn px_at(img: &RgbaImage, p: [f32; 2]) -> [f32; 3] {
    let x = (p[0].floor().max(0.0) as u32).min(img.width - 1);
    let y = (p[1].floor().max(0.0) as u32).min(img.height - 1);
    px(img, x, y)
}

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn mean(img: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32) -> [f32; 3] {
    let mut s = [0f64; 3];
    let mut n = 0f64;
    for y in y0..y1.min(img.height) {
        for x in x0..x1.min(img.width) {
            let p = px(img, x, y);
            for k in 0..3 {
                s[k] += f64::from(p[k]);
            }
            n += 1.0;
        }
    }
    assert!(n > 0.0, "empty region {x0},{y0}..{x1},{y1}");
    [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32]
}

fn assert_close(got: [f32; 3], want: [f32; 3], tol: f32, what: &str) {
    for k in 0..3 {
        assert!(
            (got[k] - want[k]).abs() <= tol,
            "{what}: got {got:?}, want {want:?} (±{tol})"
        );
    }
}

fn to255(c: [f32; 3]) -> [f32; 3] {
    [c[0] * 255.0, c[1] * 255.0, c[2] * 255.0]
}

fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for _ in 0..w * h {
        v.extend_from_slice(&c);
    }
    RgbaImage::from_rgba(w, h, v).expect("valid image")
}

/// An image built from a per-pixel function.
fn image_fn(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> RgbaImage {
    let mut v = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            v.extend_from_slice(&f(x, y));
        }
    }
    RgbaImage::from_rgba(w, h, v).expect("valid image")
}

fn fixture_path(name: &str, w: u32, h: u32) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/spec_5_3")
        .join(format!("{name}_{w}x{h}.rgba8"))
}

/// Secondary evidence (D30 b): compare with the stored snapshot, or store it under
/// `SPEC_5_3_BLESS=1`. Call only after the analytic assertions on `img` have passed.
/// Tolerance (lavapipe / WARP / Metal rasterise edges differently): at most 1 % of the
/// channels may differ by more than 6 levels, none by more than 96.
fn snapshot(name: &str, img: &RgbaImage) {
    let path = fixture_path(name, img.width, img.height);
    if std::env::var_os("SPEC_5_3_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &img.pixels).unwrap();
        println!("[{name}] snapshot written to {}", path.display());
        return;
    }
    let want = std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "snapshot {} missing ({e}); run once with SPEC_5_3_BLESS=1",
            path.display()
        )
    });
    assert_eq!(want.len(), img.pixels.len(), "[{name}] snapshot size");
    let mut off = 0usize;
    let mut worst = 0u8;
    for (a, b) in want.iter().zip(&img.pixels) {
        let d = a.abs_diff(*b);
        worst = worst.max(d);
        if d > 6 {
            off += 1;
        }
    }
    let frac = off as f64 / want.len() as f64;
    println!("[{name}] snapshot: {off} channels off by > 6 ({frac:.5}), worst {worst}");
    assert!(
        frac <= 0.01 && worst <= 96,
        "[{name}] differs from the snapshot: {frac:.4} of channels off, worst {worst}"
    );
}

/// An offscreen `Rgba8Unorm` target pre-filled with one colour.
struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    w: u32,
    h: u32,
}

impl Target {
    /// `w` must be a multiple of 64 (row pitch alignment of the readback).
    fn new(ctx: &GpuContext, w: u32, h: u32, fill: [u8; 4]) -> Target {
        assert_eq!(w % 64, 0);
        let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("spec_5_3 target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for _ in 0..w * h {
            data.extend_from_slice(&fill);
        }
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            texture.size(),
        );
        let view = texture.create_view(&Default::default());
        Target {
            texture,
            view,
            w,
            h,
        }
    }

    fn read(&self, ctx: &GpuContext) -> RgbaImage {
        let device = ctx.device();
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(self.w * self.h * 4),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(self.w * 4),
                    rows_per_image: Some(self.h),
                },
            },
            self.texture.size(),
        );
        ctx.queue().submit([enc.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buf.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        pin_render::gpu::wait_for_device(device, std::time::Duration::from_secs(60)).unwrap();
        rx.recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let bytes = buf.get_mapped_range(..).unwrap().to_vec();
        RgbaImage::from_rgba(self.w, self.h, bytes).unwrap()
    }
}

const MAGENTA: [u8; 4] = [255, 0, 255, 255];

fn is_magenta(c: [f32; 3]) -> bool {
    c[0] > 250.0 && c[1] < 5.0 && c[2] > 250.0
}

/// A DMD frame with `text` (kit font, integer scale) at dot `(x, y)`, plus the lit mask.
fn text_frame(
    spec: &DmdSpec,
    text: &str,
    big: bool,
    scale: u32,
    x: i64,
    y: i64,
    drive: [u8; 3],
) -> (DmdFrame, Vec<bool>) {
    let (w, h) = (spec.width, spec.height);
    let mut buf = vec![0u8; (w * h) as usize];
    let font = if big { FONT_8X16 } else { FONT_5X7 };
    blit_text(font, text, &mut buf, w, h, x, y, scale, 255);
    let mut frame = DmdFrame::new(w, h);
    let lit: Vec<bool> = buf.iter().map(|&b| b != 0).collect();
    for yy in 0..h {
        for xx in 0..w {
            if lit[(yy * w + xx) as usize] {
                frame.set(xx, yy, drive);
            }
        }
    }
    (frame, lit)
}

fn color_name(c: DmdColor) -> &'static str {
    match c {
        DmdColor::Orange => "orange",
        DmdColor::Red => "red",
        DmdColor::Rgb => "rgb",
    }
}

// ---------------------------------------------------------------------------------------
// DMD: dot pitch and colour at known dot positions (text, both sizes, three colour modes)
// ---------------------------------------------------------------------------------------

/// Checks every dot of `frame` in `img`: the dot centre has the reference colour of its
/// drive level, and the four cell corners around it are background.
fn check_dmd_dots(
    img: &RgbaImage,
    spec: &DmdSpec,
    frame: &DmdFrame,
    viewport: Rect,
    what: &str,
) -> (f32, f32) {
    let geo = dmd_geometry(&spec, viewport);
    let mut lit_sum = 0.0f64;
    let mut lit_n = 0u32;
    let mut unlit_sum = 0.0f64;
    let mut unlit_n = 0u32;
    for y in 0..spec.height {
        for x in 0..spec.width {
            let drive = frame.get(x, y);
            let c = geo.dot_center(x, y);
            let got = px_at(img, c);
            let want = to255(dmd_dot_color(spec.color, drive));
            assert_close(got, want, 3.0, &format!("{what}: dot ({x},{y}) centre"));
            if drive == [0, 0, 0] {
                unlit_sum += f64::from(luma(got));
                unlit_n += 1;
            } else if drive == [255, 255, 255] {
                lit_sum += f64::from(luma(got));
                lit_n += 1;
            }
            // Cell corner (half a pitch diagonally away: 0.707 pitch from every centre,
            // well outside the 0.40-pitch dot radius): background.
            let corner = [c[0] + geo.pitch * 0.5, c[1] + geo.pitch * 0.5];
            if corner[0] < (viewport.x as f32 + viewport.w as f32) - 1.0
                && corner[1] < (viewport.y as f32 + viewport.h as f32) - 1.0
            {
                // Both pixels around the corner point.
                for d in [-0.5f32, 0.5] {
                    let got = px_at(img, [corner[0] + d, corner[1] + d]);
                    assert_close(
                        got,
                        to255(DMD_BACKGROUND),
                        3.0,
                        &format!("{what}: gap after dot ({x},{y})"),
                    );
                }
            }
        }
    }
    (
        (lit_sum / f64::from(lit_n.max(1))) as f32,
        (unlit_sum / f64::from(unlit_n.max(1))) as f32,
    )
}

#[test]
fn dmd_text_dot_positions_and_colours_both_sizes_three_colour_modes() {
    let Some(ctx) = context_for_tests("dmd_text_dot_positions") else {
        return;
    };
    // (dots, pitch in px, big font, scale, text)
    let cases: [(u32, u32, u32, bool, u32, &str); 2] = [
        (128, 32, 8, false, 2, "1,234,567"),
        (192, 64, 6, true, 2, "PB 9,870"),
    ];
    for (w, h, pitch, big, scale, text) in cases {
        for color in [DmdColor::Orange, DmdColor::Red, DmdColor::Rgb] {
            let spec = DmdSpec::new(w, h, color).unwrap();
            let (frame, lit) = text_frame(&spec, text, big, scale, 3, 2, [255, 255, 255]);
            let n_lit = lit.iter().filter(|b| **b).count();
            assert!(
                n_lit > 100 && n_lit < (w * h / 2) as usize,
                "sanity: {n_lit} lit dots"
            );
            assert_eq!(frame.lit_count(), n_lit, "DmdFrame::lit_count");
            let (iw, ih) = (w * pitch, h * pitch);
            let img = render_dmd_to_image(&ctx, spec, &frame, iw, ih).unwrap();
            assert_eq!((img.width, img.height), (iw, ih));
            let what = format!("{w}x{h} {}", color_name(color));
            let viewport = Rect::new(0, 0, iw, ih);

            // Geometry contract: the pitch is the viewport over the dot count.
            let geo = dmd_geometry(&spec, viewport);
            assert!((geo.pitch - pitch as f32).abs() < 1e-3, "{what}: pitch");
            assert!((geo.radius - DMD_DOT_RADIUS * geo.pitch).abs() < 1e-3);
            let c00 = geo.dot_center(0, 0);
            assert!(
                (c00[0] - pitch as f32 * 0.5).abs() < 1e-3
                    && (c00[1] - pitch as f32 * 0.5).abs() < 1e-3,
                "{what}: first dot centre {c00:?}"
            );

            let (lit_l, unlit_l) = check_dmd_dots(&img, &spec, &frame, viewport, &what);

            // Lit vs unlit luminance ratio = 1 / DMD_UNLIT_LEVEL (8-bit rounding of the
            // unlit dot moves it by a few percent).
            let ratio = lit_l / unlit_l;
            let want = 1.0 / DMD_UNLIT_LEVEL;
            println!("[{what}] lit {lit_l:.1} unlit {unlit_l:.1} ratio {ratio:.2}");
            assert!(
                (ratio - want).abs() <= 0.12 * want,
                "{what}: lit/unlit luminance ratio {ratio} (want {want})"
            );
            // Unlit dots are faintly visible: clearly above the background, far below lit.
            assert!(unlit_l >= 4.0, "{what}: unlit dots invisible ({unlit_l})");
            assert!(unlit_l < 0.25 * lit_l);

            // Colour mode, independent of the reference function: sample one lit dot.
            let first_lit = lit.iter().position(|b| *b).unwrap() as u32;
            let c = px_at(&img, geo.dot_center(first_lit % w, first_lit / w));
            match color {
                DmdColor::Orange => {
                    assert!(c[0] > 245.0, "{what}: {c:?}");
                    assert!(c[1] > 0.3 * c[0] && c[1] < 0.6 * c[0], "{what}: {c:?}");
                    assert!(c[2] < 0.15 * c[0], "{what}: {c:?}");
                }
                DmdColor::Red => {
                    assert!(c[0] > 245.0, "{what}: {c:?}");
                    assert!(c[1] < 0.2 * c[0] && c[2] < 0.2 * c[0], "{what}: {c:?}");
                }
                DmdColor::Rgb => {
                    assert!(c.iter().all(|v| *v > 245.0), "{what}: white drive {c:?}");
                }
            }
            if (w, color) == (128, DmdColor::Orange) {
                // Secondary evidence at a small size.
                let small = render_dmd_to_image(&ctx, spec, &frame, 384, 96).unwrap();
                check_dmd_dots(&small, &spec, &frame, Rect::new(0, 0, 384, 96), "golden");
                snapshot("dmd_text_orange", &small);
            }
        }
    }
}

#[test]
fn dmd_dots_are_round_and_evenly_pitched() {
    let Some(ctx) = context_for_tests("dmd_dots_round") else {
        return;
    };
    for (w, h, pitch) in [(128u32, 32u32, 10u32), (192, 64, 8)] {
        let spec = DmdSpec::new(w, h, DmdColor::Orange).unwrap();
        let mut frame = DmdFrame::new(w, h);
        for y in 0..h {
            for x in 0..w {
                frame.set(x, y, [255, 255, 255]);
            }
        }
        let (iw, ih) = (w * pitch, h * pitch);
        let img = render_dmd_to_image(&ctx, spec, &frame, iw, ih).unwrap();
        let geo = dmd_geometry(&spec, Rect::new(0, 0, iw, ih));
        let peak = px_at(&img, geo.dot_center(0, 0))[0];
        assert!(peak > 245.0);
        let half = peak * 0.5;

        // Runs above half-maximum along a scan line through dot centres.
        let runs = |horizontal: bool, line: u32| -> Vec<(f32, f32)> {
            let n = if horizontal { iw } else { ih };
            let mut out = Vec::new();
            let mut start: Option<u32> = None;
            for i in 0..=n {
                let on = i < n && {
                    let p = if horizontal {
                        px(&img, i, line)
                    } else {
                        px(&img, line, i)
                    };
                    p[0] > half
                };
                match (on, start) {
                    (true, None) => start = Some(i),
                    (false, Some(s)) => {
                        out.push(((s + i) as f32 * 0.5, (i - s) as f32));
                        start = None;
                    }
                    _ => {}
                }
            }
            out
        };
        for row in [0, h / 2, h - 1] {
            let line = geo.dot_center(0, row)[1].floor() as u32;
            let r = runs(true, line);
            assert_eq!(r.len(), w as usize, "{w}x{h}: one run per dot in row {row}");
            for (i, (centre, width)) in r.iter().enumerate() {
                let want = geo.dot_center(i as u32, row)[0];
                assert!(
                    (centre - want).abs() <= 0.75,
                    "{w}x{h}: dot {i} centre {centre} vs {want}"
                );
                assert!(
                    (width - 2.0 * geo.radius).abs() <= 1.5,
                    "{w}x{h}: dot {i} width {width} vs diameter {}",
                    2.0 * geo.radius
                );
            }
            // Even spacing: every centre-to-centre distance is the pitch.
            for pair in r.windows(2) {
                assert!((pair[1].0 - pair[0].0 - geo.pitch).abs() <= 1.0);
            }
        }
        for col in [0, w / 2, w - 1] {
            let line = geo.dot_center(col, 0)[0].floor() as u32;
            let r = runs(false, line);
            assert_eq!(
                r.len(),
                h as usize,
                "{w}x{h}: one run per dot in column {col}"
            );
            for (i, (centre, width)) in r.iter().enumerate() {
                let want = geo.dot_center(col, i as u32)[1];
                assert!((centre - want).abs() <= 0.75);
                assert!((width - 2.0 * geo.radius).abs() <= 1.5);
            }
        }
        // Round, not square: inside the radius on the diagonal is lit, the square's corner
        // (0.9 r, 0.9 r) → 1.27 r from the centre is dark.
        for (x, y) in [(0u32, 0u32), (w / 2, h / 2), (w - 1, h - 1), (7, 3)] {
            let c = geo.dot_center(x, y);
            for s in [[1.0f32, 1.0], [1.0, -1.0], [-1.0, 1.0], [-1.0, -1.0]] {
                let k_in = 0.5 * geo.radius; // 0.707 r from the centre
                let k_out = 0.9 * geo.radius;
                let inside = px_at(&img, [c[0] + s[0] * k_in, c[1] + s[1] * k_in]);
                let outside = px_at(&img, [c[0] + s[0] * k_out, c[1] + s[1] * k_out]);
                assert!(inside[0] > 0.9 * peak, "dot ({x},{y}) diagonal inside");
                assert!(
                    outside[0] < 0.1 * peak,
                    "dot ({x},{y}) is not round: {outside:?} at 1.27 r"
                );
            }
        }
        // Covered area fraction of a full-on display ≈ π r² / pitch² = π · 0.16.
        let m = mean(&img, 0, 0, iw, ih);
        let want = std::f32::consts::PI * DMD_DOT_RADIUS * DMD_DOT_RADIUS * 255.0;
        assert!(
            (m[0] - want).abs() <= 0.06 * want,
            "{w}x{h}: mean red {} vs disc coverage {want}",
            m[0]
        );
    }
}

#[test]
fn dmd_drive_levels_are_linear_and_rgb_dots_take_their_colour() {
    let Some(ctx) = context_for_tests("dmd_drive_levels") else {
        return;
    };
    // Mono: 16 drive levels in one row, one dot each.
    for color in [DmdColor::Orange, DmdColor::Red] {
        let spec = DmdSpec::new(128, 32, color).unwrap();
        let mut frame = DmdFrame::new(128, 32);
        for i in 0..16u32 {
            let v = (i * 17) as u8;
            frame.set(4 + i * 3, 10, [v, v, v]);
        }
        let img = render_dmd_to_image(&ctx, spec, &frame, 1024, 256).unwrap();
        let geo = dmd_geometry(&spec, Rect::new(0, 0, 1024, 256));
        let tint = dmd_tint(color);
        let mut prev = -1.0f32;
        for i in 0..16u32 {
            let got = px_at(&img, geo.dot_center(4 + i * 3, 10));
            let b = DMD_UNLIT_LEVEL + (1.0 - DMD_UNLIT_LEVEL) * (i as f32 / 15.0);
            let want = [tint[0] * b * 255.0, tint[1] * b * 255.0, tint[2] * b * 255.0];
            assert_close(got, want, 3.0, &format!("{color:?} level {i}"));
            assert!(got[0] > prev, "{color:?}: level {i} not brighter");
            prev = got[0];
        }
    }
    // RGB: primaries keep their hue; an unlit dot is neutral.
    let spec = DmdSpec::new(128, 32, DmdColor::Rgb).unwrap();
    let mut frame = DmdFrame::new(128, 32);
    let drives: [[u8; 3]; 5] = [
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
        [255, 255, 0],
        [64, 128, 255],
    ];
    for (i, d) in drives.iter().enumerate() {
        frame.set(10 + 5 * i as u32, 16, *d);
    }
    let img = render_dmd_to_image(&ctx, spec, &frame, 1024, 256).unwrap();
    let geo = dmd_geometry(&spec, Rect::new(0, 0, 1024, 256));
    for (i, d) in drives.iter().enumerate() {
        let got = px_at(&img, geo.dot_center(10 + 5 * i as u32, 16));
        assert_close(
            got,
            to255(dmd_dot_color(DmdColor::Rgb, *d)),
            3.0,
            &format!("rgb drive {d:?}"),
        );
        for k in 0..3 {
            if d[k] == 255 {
                assert!(got[k] > 245.0, "rgb drive {d:?}: {got:?}");
            }
            if d[k] == 0 {
                assert!(got[k] < 60.0, "rgb drive {d:?} leaks into channel {k}: {got:?}");
            }
        }
    }
    let unlit = px_at(&img, geo.dot_center(100, 5));
    assert!(
        (unlit[0] - unlit[1]).abs() <= 2.0 && (unlit[1] - unlit[2]).abs() <= 2.0,
        "rgb unlit dot is neutral: {unlit:?}"
    );
    assert!(unlit[0] >= 4.0);
}

#[test]
fn dmd_fractional_pitch_is_centred_and_letterboxed() {
    let Some(ctx) = context_for_tests("dmd_fractional_pitch") else {
        return;
    };
    let spec = DmdSpec::new(128, 32, DmdColor::Red).unwrap();
    let (frame, _) = text_frame(&spec, "BALL 2", false, 3, 5, 4, [255, 255, 255]);
    // 1000 × 300: pitch = min(1000/128, 300/32) = 7.8125; the grid is 250 px high, centred.
    let (iw, ih) = (1000u32, 300u32);
    let viewport = Rect::new(0, 0, iw, ih);
    let geo = dmd_geometry(&spec, viewport);
    assert!((geo.pitch - 7.8125).abs() < 1e-4);
    let img = render_dmd_to_image(&ctx, spec, &frame, iw, ih).unwrap();
    check_dmd_dots(&img, &spec, &frame, viewport, "fractional pitch");
    // Letterbox bands (25 px above and below the grid) are background.
    let top = geo.dot_center(0, 0)[1] - geo.pitch * 0.5;
    assert!((top - 25.0).abs() < 0.01, "grid top {top}");
    for (y0, y1) in [(0u32, 23u32), (277, 300)] {
        assert_close(
            mean(&img, 0, y0, iw, y1),
            to255(DMD_BACKGROUND),
            0.5,
            "letterbox band",
        );
    }
    // The same in a tall viewport: bands left and right.
    let (iw, ih) = (640u32, 400u32); // pitch = min(5, 12.5) = 5 → grid 640 × 160, centred
    let img = render_dmd_to_image(&ctx, spec, &frame, iw, ih).unwrap();
    check_dmd_dots(&img, &spec, &frame, Rect::new(0, 0, iw, ih), "tall viewport");
    assert_close(mean(&img, 0, 0, iw, 118), [0.0; 3], 0.5, "top band");
    assert_close(mean(&img, 0, 282, iw, 400), [0.0; 3], 0.5, "bottom band");
}

// ---------------------------------------------------------------------------------------
// Score display model → DMD (rendered score text) and placeholder scenes
// ---------------------------------------------------------------------------------------

/// Finds `text` in `frame` as an exact glyph pattern in either kit font at scales 1..=4.
fn find_text(frame: &DmdFrame, text: &str) -> Option<(bool, u32, u32, u32)> {
    let (w, h) = (frame.width, frame.height);
    let lit = |x: u32, y: u32| frame.get(x, y) != [0, 0, 0];
    for big in [true, false] {
        let font = if big { FONT_8X16 } else { FONT_5X7 };
        for scale in (1..=4u32).rev() {
            let tw = font.text_width(text) * scale;
            let th = font.height() * scale;
            if tw > w || th > h {
                continue;
            }
            let mut pat = vec![0u8; (tw * th) as usize];
            blit_text(font, text, &mut pat, tw, th, 0, 0, scale, 1);
            let ink: Vec<(u32, u32)> = (0..th)
                .flat_map(|y| (0..tw).map(move |x| (x, y)))
                .filter(|&(x, y)| pat[(y * tw + x) as usize] != 0)
                .collect();
            for oy in 0..=(h - th) {
                'pos: for ox in 0..=(w - tw) {
                    if !ink.iter().all(|&(x, y)| lit(ox + x, oy + y)) {
                        continue;
                    }
                    for y in 0..th {
                        for x in 0..tw {
                            if (pat[(y * tw + x) as usize] != 0) != lit(ox + x, oy + y) {
                                continue 'pos;
                            }
                        }
                    }
                    return Some((big, scale, ox, oy));
                }
            }
        }
    }
    None
}

#[test]
fn dmd_score_view_renders_the_formatted_score_as_font_dots() {
    let Some(ctx) = context_for_tests("dmd_score_view") else {
        return;
    };
    for (w, h, pitch) in [(128u32, 32u32, 6u32), (192, 64, 4)] {
        for color in [DmdColor::Orange, DmdColor::Red, DmdColor::Rgb] {
            let spec = DmdSpec::new(w, h, color).unwrap();
            let mut model = ScoreDisplayModel::new(
                ScoreDisplayConfig::Dmd(spec),
                SceneLibrary::placeholders(),
            );
            model.scores_mut().scores[0] = 1_234_560;
            model.scores_mut().ball = 1;
            let DisplayContent::Dmd(frame) = model.compose(0) else {
                panic!("a DMD config must compose a DMD frame");
            };
            assert_eq!((frame.width, frame.height), (w, h));
            // Score formatting on the display: the formatted text is there, dot for dot.
            let found = find_text(&frame, "1,234,560");
            let Some((big, scale, ox, oy)) = found else {
                panic!("{w}x{h} {color:?}: \"1,234,560\" not found as kit-font dots");
            };
            println!(
                "[{w}x{h} {color:?}] score text: font {} scale {scale} at ({ox},{oy})",
                if big { "8x16" } else { "5x7" }
            );
            // And it is rendered where the geometry says, in the mode's colour.
            let (iw, ih) = (w * pitch, h * pitch);
            let img = render_dmd_to_image(&ctx, spec, &frame, iw, ih).unwrap();
            check_dmd_dots(
                &img,
                &spec,
                &frame,
                Rect::new(0, 0, iw, ih),
                &format!("score view {w}x{h} {color:?}"),
            );
            // A different score gives a different frame, the same score the same frame.
            let DisplayContent::Dmd(again) = model.compose(0) else {
                unreachable!()
            };
            assert_eq!(again, frame, "compose is deterministic");
            model.scores_mut().scores[0] = 1_234_570;
            let DisplayContent::Dmd(next) = model.compose(0) else {
                unreachable!()
            };
            assert_ne!(next, frame);
            assert!(find_text(&next, "1,234,570").is_some());
        }
    }
}

#[test]
fn placeholder_scenes_render_on_dmd_and_lcd() {
    let Some(ctx) = context_for_tests("placeholder_scenes_render") else {
        return;
    };
    let ids = [
        "scene_attract",
        "scene_score",
        "scene_game_over",
        "scene_powerball_mania",
    ];
    for id in ids {
        for (w, h) in [(128u32, 32u32), (192, 64)] {
            let spec = DmdSpec::new(w, h, DmdColor::Orange).unwrap();
            let lib = SceneLibrary::placeholders();
            let sid = lib
                .id_of(id)
                .unwrap_or_else(|| panic!("placeholder {id} missing"));
            let mut model = ScoreDisplayModel::new(ScoreDisplayConfig::Dmd(spec), lib);
            model.scenes_mut().show(sid, 0);
            // Sample a few instants (a scene may blink): at least one shows content.
            let mut best = 0usize;
            let mut best_frame = None;
            for t in [0u64, 100, 250, 400, 700, 1000] {
                let DisplayContent::Dmd(frame) = model.compose(t) else {
                    panic!("DMD config composes a DMD frame");
                };
                if frame.lit_count() > best {
                    best = frame.lit_count();
                    best_frame = Some(frame);
                }
            }
            assert!(best >= 20, "{id} at {w}x{h}: only {best} lit dots");
            let frame = best_frame.unwrap();
            let img = render_dmd_to_image(&ctx, spec, &frame, w * 4, h * 4).unwrap();
            let lit_px = (0..img.height)
                .flat_map(|y| (0..img.width).map(move |x| (x, y)))
                .filter(|&(x, y)| px(&img, x, y)[0] > 200.0)
                .count();
            // Each lit dot covers about π·(0.4·4)² ≈ 8 px.
            assert!(
                lit_px >= best * 3,
                "{id} at {w}x{h}: {lit_px} bright pixels for {best} lit dots"
            );
        }
        // LCD at 16:9.
        let spec = LcdSpec::new(1920, 1080).unwrap();
        let lib = SceneLibrary::placeholders();
        let sid = lib.id_of(id).unwrap();
        let mut model = ScoreDisplayModel::new(ScoreDisplayConfig::Lcd(spec), lib);
        model.scores_mut().scores[0] = 42_000;
        model.scenes_mut().show(sid, 0);
        let mut seen = false;
        for t in [0u64, 100, 250, 400, 700, 1000] {
            let DisplayContent::Lcd(list) = model.compose(t) else {
                panic!("LCD config composes an LCD draw list");
            };
            assert_eq!((list.width, list.height), (1920, 1080));
            if list.items.is_empty() {
                continue;
            }
            let img = render_lcd_to_image(&ctx, spec, &list, &ImageSet::new(), 480, 270).unwrap();
            let bg = [
                f32::from(list.background[0]),
                f32::from(list.background[1]),
                f32::from(list.background[2]),
            ];
            let differing = (0..img.height)
                .flat_map(|y| (0..img.width).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let p = px(&img, x, y);
                    (0..3).any(|k| (p[k] - bg[k]).abs() > 24.0)
                })
                .count();
            if differing >= 200 {
                seen = true;
                break;
            }
        }
        assert!(seen, "{id}: nothing visible on the LCD");
    }
}

// ---------------------------------------------------------------------------------------
// Backglass: lamp-mask blending, flashers, layer order
// ---------------------------------------------------------------------------------------

const BASE: [u8; 4] = [40, 80, 120, 255];
const LIT: [u8; 4] = [250, 200, 50, 255];

/// Base + one lamp-driven layer whose mask is: left third full (red 255), middle third
/// half (red 128), right third zero — with green/blue set where red is zero, to prove only
/// the red channel is the mask.
fn lamp_desc(blend: LayerBlend, layer_rgba: [u8; 4], base: [u8; 4]) -> BackglassDesc {
    let (w, h) = (96u32, 72u32);
    let mask = image_fn(w, h, |x, _| {
        if x < 32 {
            [255, 0, 0, 255]
        } else if x < 64 {
            [128, 128, 128, 255]
        } else {
            [0, 255, 255, 255]
        }
    });
    BackglassDesc {
        layers: vec![
            BackglassLayer {
                image: solid(w, h, base),
                lamp: None,
            },
            BackglassLayer {
                image: solid(w, h, layer_rgba),
                lamp: Some(LayerLamp {
                    lamp: LampId(5),
                    mask,
                    blend,
                }),
            },
        ],
    }
}

fn lamps_with(id: usize, v: f32) -> Vec<f32> {
    let mut l = vec![0.0f32; 16];
    // Unreferenced lamps at full: they must not light anything.
    l[0] = 1.0;
    l[4] = 1.0;
    l[6] = 1.0;
    l[id] = v;
    l
}

fn f3(c: [u8; 4]) -> [f32; 3] {
    [f32::from(c[0]), f32::from(c[1]), f32::from(c[2])]
}

fn lerp3(a: [f32; 3], b: [f32; 3], k: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * k,
        a[1] + (b[1] - a[1]) * k,
        a[2] + (b[2] - a[2]) * k,
    ]
}

// Regions of a 384 × 288 render of the 96 × 72 description (scale 4), kept 12 px away from
// the mask boundaries at x = 128 and x = 256 (texture filtering).
const R_FULL: (u32, u32, u32, u32) = (8, 8, 116, 280);
const R_HALF: (u32, u32, u32, u32) = (140, 8, 244, 280);
const R_OUT: (u32, u32, u32, u32) = (268, 8, 376, 280);

fn region(img: &RgbaImage, r: (u32, u32, u32, u32)) -> [f32; 3] {
    mean(img, r.0, r.1, r.2, r.3)
}

/// Largest per-channel deviation from the region mean (flatness).
fn spread(img: &RgbaImage, r: (u32, u32, u32, u32)) -> f32 {
    let m = region(img, r);
    let mut worst = 0f32;
    for y in r.1..r.3 {
        for x in r.0..r.2 {
            let p = px(img, x, y);
            for k in 0..3 {
                worst = worst.max((p[k] - m[k]).abs());
            }
        }
    }
    worst
}

#[test]
fn backglass_lamp_mask_blend_is_linear_inside_and_zero_outside() {
    let Some(ctx) = context_for_tests("backglass_lamp_mask_linear") else {
        return;
    };
    let desc = lamp_desc(LayerBlend::Over, LIT, BASE);
    let render = |v: f32| render_backglass_to_image(&ctx, &desc, &lamps_with(5, v), 384, 288).unwrap();
    let levels = [0.0f32, 0.25, 0.5, 0.75, 1.0];
    let imgs: Vec<RgbaImage> = levels.iter().map(|&v| render(v)).collect();
    let (base, lit) = (f3(BASE), f3(LIT));
    for (img, &v) in imgs.iter().zip(&levels) {
        // Inside the full mask: base + (rgb − base)·lamp.
        assert_close(
            region(img, R_FULL),
            lerp3(base, lit, v),
            2.0,
            &format!("full mask, lamp {v}"),
        );
        // Half mask (128/255): k = 0.502·lamp.
        assert_close(
            region(img, R_HALF),
            lerp3(base, lit, v * 128.0 / 255.0),
            2.0,
            &format!("half mask, lamp {v}"),
        );
        // Outside the mask (red = 0, green/blue = 255): exactly the base.
        assert_close(
            region(img, R_OUT),
            base,
            1.0,
            &format!("outside mask, lamp {v}"),
        );
        for r in [R_FULL, R_HALF, R_OUT] {
            assert!(spread(img, r) <= 2.0, "region not flat at lamp {v}");
        }
    }
    // Zero change outside the mask: byte-identical for every lamp level.
    for (img, &v) in imgs.iter().zip(&levels) {
        for y in R_OUT.1..R_OUT.3 {
            for x in R_OUT.0..R_OUT.2 {
                assert_eq!(
                    img.pixel(x, y),
                    imgs[0].pixel(x, y),
                    "pixel ({x},{y}) outside the mask changed at lamp {v}"
                );
            }
        }
    }
    // Linearity in the lamp level (0, 0.5, 1): the midpoint image is the mean of the ends,
    // per pixel, everywhere (including the filtered mask edges).
    let (lo, mid, hi) = (&imgs[0], &imgs[2], &imgs[4]);
    let mut worst = 0f32;
    for y in 0..288 {
        for x in 0..384 {
            let (a, m, b) = (px(lo, x, y), px(mid, x, y), px(hi, x, y));
            for k in 0..3 {
                worst = worst.max((m[k] - 0.5 * (a[k] + b[k])).abs());
            }
        }
    }
    println!("[backglass linear] worst midpoint deviation {worst}");
    assert!(worst <= 2.0, "blend not linear in lamp level: {worst}");
    // … and the lamp actually does something inside the mask.
    assert!(region(hi, R_FULL)[0] - region(lo, R_FULL)[0] > 200.0);

    // Lamp levels clamp to 0..=1.
    let over = render(3.0);
    let under = render(-2.0);
    assert_close(region(&over, R_FULL), lit, 2.0, "lamp 3.0 clamps to 1");
    assert_close(region(&under, R_FULL), base, 2.0, "lamp −2.0 clamps to 0");
    // A lamp slice too short to hold the lamp = lamp off.
    let short = render_backglass_to_image(&ctx, &desc, &[1.0, 1.0], 384, 288).unwrap();
    assert_close(region(&short, R_FULL), base, 2.0, "missing lamp entry = 0");
    let none = render_backglass_to_image(&ctx, &desc, &[], 384, 288).unwrap();
    assert_close(region(&none, R_FULL), base, 2.0, "empty lamp slice = 0");

    snapshot(
        "backglass_lamp_half",
        &render_backglass_to_image(&ctx, &desc, &lamps_with(5, 0.5), 128, 96).unwrap(),
    );
}

#[test]
fn backglass_lamp_layer_alpha_scales_the_blend() {
    let Some(ctx) = context_for_tests("backglass_layer_alpha") else {
        return;
    };
    // Layer alpha 102/255 = 0.4: k = mask · lamp · 0.4.
    let layer = [250, 200, 50, 102];
    let desc = lamp_desc(LayerBlend::Over, layer, BASE);
    let img = render_backglass_to_image(&ctx, &desc, &lamps_with(5, 1.0), 384, 288).unwrap();
    let (base, lit) = (f3(BASE), f3(LIT));
    assert_close(
        region(&img, R_FULL),
        lerp3(base, lit, 0.4),
        2.5,
        "full mask × alpha 0.4",
    );
    assert_close(
        region(&img, R_HALF),
        lerp3(base, lit, 0.4 * 128.0 / 255.0),
        2.5,
        "half mask × alpha 0.4",
    );
    assert_close(region(&img, R_OUT), base, 1.0, "outside mask");
}

#[test]
fn backglass_flashers_are_additive() {
    let Some(ctx) = context_for_tests("backglass_flashers_additive") else {
        return;
    };
    let flash = [100u8, 60, 20, 255];
    let desc = lamp_desc(LayerBlend::Additive, flash, BASE);
    let render = |v: f32| render_backglass_to_image(&ctx, &desc, &lamps_with(5, v), 384, 288).unwrap();
    let (base, add) = (f3(BASE), f3(flash));
    let imgs: Vec<RgbaImage> = [0.0f32, 0.5, 1.0].iter().map(|&v| render(v)).collect();
    for (img, v) in imgs.iter().zip([0.0f32, 0.5, 1.0]) {
        let want = |k: f32| [base[0] + add[0] * k, base[1] + add[1] * k, base[2] + add[2] * k];
        assert_close(region(img, R_FULL), want(v), 2.0, &format!("flasher {v}"));
        assert_close(
            region(img, R_HALF),
            want(v * 128.0 / 255.0),
            2.0,
            &format!("flasher half mask {v}"),
        );
        assert_close(region(img, R_OUT), base, 1.0, "flasher outside mask");
    }
    // Additive: no channel anywhere is ever darker than with the flasher off.
    for y in 0..288 {
        for x in 0..384 {
            let (a, b) = (imgs[0].pixel(x, y), imgs[2].pixel(x, y));
            for k in 0..3 {
                assert!(b[k] >= a[k], "flasher darkened ({x},{y})");
            }
        }
    }
    // Over a bright base it saturates at white instead of wrapping.
    let bright = [220u8, 230, 250, 255];
    let desc = lamp_desc(LayerBlend::Additive, flash, bright);
    let img = render_backglass_to_image(&ctx, &desc, &lamps_with(5, 1.0), 384, 288).unwrap();
    assert_close(
        region(&img, R_FULL),
        [255.0, 255.0, 255.0],
        1.0,
        "flasher clamps at white",
    );
    // Contrast with `Over` for the same inputs: Over moves towards the layer colour (darker
    // than this base), additive only ever adds.
    let desc = lamp_desc(LayerBlend::Over, flash, bright);
    let img = render_backglass_to_image(&ctx, &desc, &lamps_with(5, 1.0), 384, 288).unwrap();
    assert_close(region(&img, R_FULL), add, 2.0, "Over replaces");
}

#[test]
fn backglass_layers_composite_back_to_front_with_alpha() {
    let Some(ctx) = context_for_tests("backglass_layer_order") else {
        return;
    };
    let (w, h) = (96u32, 72u32);
    // Layer 1: opaque green on the left two thirds, transparent elsewhere.
    let l1 = image_fn(w, h, |x, _| if x < 64 { [0, 200, 0, 255] } else { [9, 9, 9, 0] });
    // Layer 2: half-transparent white on the right two thirds, transparent elsewhere.
    let l2 = image_fn(w, h, |x, _| {
        if x >= 32 {
            [255, 255, 255, 128]
        } else {
            [0, 0, 0, 0]
        }
    });
    let desc = BackglassDesc {
        layers: vec![
            BackglassLayer {
                image: solid(w, h, BASE),
                lamp: None,
            },
            BackglassLayer {
                image: l1.clone(),
                lamp: None,
            },
            BackglassLayer {
                image: l2.clone(),
                lamp: None,
            },
        ],
    };
    let img = render_backglass_to_image(&ctx, &desc, &[], 384, 288).unwrap();
    let base = f3(BASE);
    let green = [0.0, 200.0, 0.0];
    let white = [255.0; 3];
    let a = 128.0 / 255.0;
    assert_close(region(&img, R_FULL), green, 2.0, "layer 1 over base");
    assert_close(
        region(&img, R_HALF),
        lerp3(green, white, a),
        2.0,
        "layer 2 (α 0.5) over layer 1",
    );
    assert_close(
        region(&img, R_OUT),
        lerp3(base, white, a),
        2.0,
        "layer 2 (α 0.5) over base; transparent layer-1 texels contribute nothing",
    );
    // Swapped order: the opaque green now hides the white where they overlap.
    let desc = BackglassDesc {
        layers: vec![
            BackglassLayer {
                image: solid(w, h, BASE),
                lamp: None,
            },
            BackglassLayer {
                image: l2,
                lamp: None,
            },
            BackglassLayer {
                image: l1,
                lamp: None,
            },
        ],
    };
    let img = render_backglass_to_image(&ctx, &desc, &[], 384, 288).unwrap();
    assert_close(region(&img, R_FULL), green, 2.0, "swapped: green over base");
    assert_close(region(&img, R_HALF), green, 2.0, "swapped: green hides white");
    assert_close(
        region(&img, R_OUT),
        lerp3(base, white, a),
        2.0,
        "swapped: white over base",
    );
}

#[test]
fn backglass_is_letterboxed_to_the_first_layers_aspect() {
    let Some(ctx) = context_for_tests("backglass_letterbox") else {
        return;
    };
    let desc = lamp_desc(LayerBlend::Over, LIT, BASE); // 96 × 72 = 4:3
    // 400 × 120 → the 4:3 picture is 160 × 120, centred at x = 120..280.
    let img = render_backglass_to_image(&ctx, &desc, &lamps_with(5, 1.0), 400, 120).unwrap();
    assert_eq!(fit_aspect(Rect::new(0, 0, 400, 120), 4, 3), Rect::new(120, 0, 160, 120));
    assert_close(mean(&img, 0, 0, 118, 120), [0.0; 3], 0.5, "left bar is black");
    assert_close(mean(&img, 282, 0, 400, 120), [0.0; 3], 0.5, "right bar is black");
    // Inside: lit third, then base third (mask zero) at the right.
    assert_close(mean(&img, 124, 4, 168, 116), f3(LIT), 2.0, "lit third");
    assert_close(mean(&img, 232, 4, 276, 116), f3(BASE), 2.0, "unlit third");
}

// ---------------------------------------------------------------------------------------
// LCD: layer order, alpha compositing, images, vector text
// ---------------------------------------------------------------------------------------

const LCD_BG: [u8; 3] = [10, 20, 30];

fn lcd_list(items: Vec<LcdItem>) -> LcdDrawList {
    LcdDrawList {
        width: 320,
        height: 180,
        background: LCD_BG,
        items,
    }
}

#[test]
fn lcd_layer_order_and_alpha_compositing() {
    let Some(ctx) = context_for_tests("lcd_layer_order_alpha") else {
        return;
    };
    let spec = LcdSpec::new(320, 180).unwrap();
    let red = LcdItem::Rect {
        rect: [20.0, 20.0, 200.0, 120.0],
        color: [220, 30, 30, 255],
    };
    let blue = LcdItem::Rect {
        rect: [100.0, 60.0, 300.0, 160.0],
        color: [20, 40, 240, 128],
    };
    let bg = [10.0, 20.0, 30.0];
    let (r, b) = ([220.0, 30.0, 30.0], [20.0, 40.0, 240.0]);
    let a = 128.0 / 255.0;
    let images = ImageSet::new();
    // Rendered at 2× (640 × 360): canvas (x, y) → pixel (2x, 2y).
    let m = |img: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32| {
        mean(img, 2 * x0 + 3, 2 * y0 + 3, 2 * x1 - 3, 2 * y1 - 3)
    };

    // Red then translucent blue.
    let list = lcd_list(vec![red.clone(), blue.clone()]);
    let img = render_lcd_to_image(&ctx, spec, &list, &images, 640, 360).unwrap();
    assert_eq!((img.width, img.height), (640, 360));
    assert_close(m(&img, 0, 0, 320, 20), bg, 1.0, "background");
    assert_close(m(&img, 20, 20, 100, 120), r, 1.5, "red only");
    assert_close(m(&img, 100, 60, 200, 120), lerp3(r, b, a), 2.0, "blue α over red");
    assert_close(m(&img, 200, 60, 300, 160), lerp3(bg, b, a), 2.0, "blue α over bg");
    assert_close(m(&img, 300, 0, 320, 180), bg, 1.0, "right margin");
    // Edges are where the draw list says (canvas px → 2× target px).
    assert_close(px(&img, 38, 100), bg, 1.5, "just left of the red rect");
    assert_close(px(&img, 41, 100), r, 1.5, "just inside the red rect");
    assert_close(px(&img, 398, 60), r, 1.5, "just inside the red rect's right edge");
    snapshot("lcd_compositing", &render_lcd_to_image(&ctx, spec, &list, &images, 320, 180).unwrap());

    // Translucent blue then opaque red: red hides blue in the overlap.
    let list = lcd_list(vec![blue.clone(), red.clone()]);
    let img = render_lcd_to_image(&ctx, spec, &list, &images, 640, 360).unwrap();
    assert_close(m(&img, 100, 60, 200, 120), r, 1.5, "red over blue");
    assert_close(m(&img, 200, 60, 300, 160), lerp3(bg, b, a), 2.0, "blue α over bg");
    assert_close(m(&img, 20, 20, 100, 120), r, 1.5, "red only");

    // Three stacked translucent layers: "over" applied back to front.
    let layer = |c: [u8; 4]| LcdItem::Rect {
        rect: [40.0, 40.0, 280.0, 140.0],
        color: c,
    };
    let stack = [[255u8, 0, 0, 102], [0, 255, 0, 153], [0, 0, 255, 51]];
    let list = lcd_list(stack.iter().map(|c| layer(*c)).collect());
    let img = render_lcd_to_image(&ctx, spec, &list, &images, 640, 360).unwrap();
    let mut want = bg;
    for c in stack {
        want = lerp3(want, f3(c), f32::from(c[3]) / 255.0);
    }
    assert_close(m(&img, 40, 40, 280, 140), want, 2.5, "three-layer over");
    // Reversed order gives a different, also exactly predicted, colour.
    let list = lcd_list(stack.iter().rev().map(|c| layer(*c)).collect());
    let img = render_lcd_to_image(&ctx, spec, &list, &images, 640, 360).unwrap();
    let mut want_rev = bg;
    for c in stack.iter().rev() {
        want_rev = lerp3(want_rev, f3(*c), f32::from(c[3]) / 255.0);
    }
    assert!((want_rev[1] - want[1]).abs() > 20.0, "test is order-sensitive");
    assert_close(m(&img, 40, 40, 280, 140), want_rev, 2.5, "three-layer over, reversed");

    // An empty list is just the background.
    let img = render_lcd_to_image(&ctx, spec, &lcd_list(vec![]), &images, 640, 360).unwrap();
    assert_close(mean(&img, 0, 0, 640, 360), bg, 0.6, "empty draw list");
}

#[test]
fn lcd_images_composite_with_item_and_texel_alpha() {
    let Some(ctx) = context_for_tests("lcd_image_alpha") else {
        return;
    };
    let spec = LcdSpec::new(320, 180).unwrap();
    let mut images = ImageSet::new();
    // Left half opaque yellow, right half yellow at α = 128.
    images.insert(
        "sprites/y.png",
        image_fn(16, 16, |x, _| [240, 220, 20, if x < 8 { 255 } else { 128 }]),
    );
    images.insert("sprites/g.png", solid(4, 4, [0, 255, 0, 255]));
    let bg = [10.0, 20.0, 30.0];
    let y = [240.0, 220.0, 20.0];
    let img_item = |alpha: f32| LcdItem::Image {
        file: "sprites/y.png".to_string(),
        rect: [40.0, 20.0, 200.0, 100.0],
        color: [255, 255, 255, (alpha * 255.0f32).round() as u8],
    };
    let m = |img: &RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32| {
        mean(img, 2 * x0 + 8, 2 * y0 + 8, 2 * x1 - 8, 2 * y1 - 8)
    };
    for alpha in [1.0f32, 0.4] {
        let list = lcd_list(vec![img_item(alpha)]);
        let img = render_lcd_to_image(&ctx, spec, &list, &images, 640, 360).unwrap();
        // The image is stretched over its rect: left half x 40..120, right half 120..200.
        assert_close(
            m(&img, 40, 20, 116, 100),
            lerp3(bg, y, alpha),
            2.5,
            &format!("opaque texels × item alpha {alpha}"),
        );
        assert_close(
            m(&img, 124, 20, 200, 100),
            lerp3(bg, y, alpha * 128.0 / 255.0),
            2.5,
            &format!("α-128 texels × item alpha {alpha}"),
        );
        assert_close(m(&img, 200, 100, 320, 180), bg, 1.0, "outside the image");
    }
    // Order between an image and a rect.
    let rect = LcdItem::Rect {
        rect: [0.0, 0.0, 320.0, 180.0],
        color: [200, 0, 200, 255],
    };
    let green = LcdItem::Image {
        file: "sprites/g.png".to_string(),
        rect: [100.0, 50.0, 220.0, 130.0],
        color: [255, 255, 255, 255],
    };
    let img = render_lcd_to_image(
        &ctx,
        spec,
        &lcd_list(vec![rect.clone(), green.clone()]),
        &images,
        640,
        360,
    )
    .unwrap();
    assert_close(m(&img, 100, 50, 220, 130), [0.0, 255.0, 0.0], 1.5, "image over rect");
    let img =
        render_lcd_to_image(&ctx, spec, &lcd_list(vec![green, rect]), &images, 640, 360).unwrap();
    assert_close(m(&img, 100, 50, 220, 130), [200.0, 0.0, 200.0], 1.5, "rect over image");
    // A missing image file must not fail the frame.
    let missing = LcdItem::Image {
        file: "nope.png".to_string(),
        rect: [0.0, 0.0, 50.0, 50.0],
        color: [255, 255, 255, 255],
    };
    let r = render_lcd_to_image(&ctx, spec, &lcd_list(vec![missing]), &images, 640, 360);
    assert!(r.is_ok(), "a missing sprite must not be a render error");
}

#[test]
fn lcd_vector_text_follows_the_5x7_glyphs_and_scales() {
    let Some(ctx) = context_for_tests("lcd_vector_text") else {
        return;
    };
    let spec = LcdSpec::new(320, 180).unwrap();
    let images = ImageSet::new();
    let text = "1,234,560";
    let fg = [255.0, 240.0, 200.0];
    let bg = [10.0, 20.0, 30.0];
    // Two glyph cell heights; the canvas is rendered at 4× so a unit is 8 or 16 px.
    for cell in [14.0f32, 28.0] {
        let origin = [12.0f32, 30.0];
        let list = lcd_list(vec![LcdItem::Text {
            text: text.to_string(),
            origin,
            cell,
            color: [255, 240, 200, 255],
        }]);
        let img = render_lcd_to_image(&ctx, spec, &list, &images, 1280, 720).unwrap();
        let unit = cell / 7.0;
        let mut lit_ok = 0u32;
        let mut lit_n = 0u32;
        let mut unlit_ok = 0u32;
        let mut unlit_n = 0u32;
        for (i, ch) in text.chars().enumerate() {
            let g = FONT_5X7.glyph(ch).unwrap();
            for gy in 0..7u32 {
                for gx in 0..5u32 {
                    let cx = origin[0] + (i as f32 * 6.0 + gx as f32 + 0.5) * unit;
                    let cy = origin[1] + (gy as f32 + 0.5) * unit;
                    let p = px_at(&img, [cx * 4.0, cy * 4.0]);
                    if g.pixel(gx, gy) {
                        lit_n += 1;
                        if (0..3).all(|k| (p[k] - fg[k]).abs() <= 6.0) {
                            lit_ok += 1;
                        }
                    } else {
                        unlit_n += 1;
                        if (0..3).all(|k| (p[k] - bg[k]).abs() <= 6.0) {
                            unlit_ok += 1;
                        }
                    }
                }
            }
        }
        println!("[lcd text cell {cell}] lit {lit_ok}/{lit_n}, unlit {unlit_ok}/{unlit_n}");
        assert_eq!(lit_ok, lit_n, "cell {cell}: every lit glyph dot is inked at its centre");
        assert_eq!(
            unlit_ok, unlit_n,
            "cell {cell}: every unlit glyph dot centre is background"
        );
        // Strokes join neighbouring lit dots (vector font, not separate dots): the
        // midpoint between two horizontally / vertically adjacent lit dots is inked.
        let g = FONT_5X7.glyph('5').unwrap();
        let i5 = text.chars().position(|c| c == '5').unwrap() as f32;
        let mut joins = 0;
        for gy in 0..7u32 {
            for gx in 0..5u32 {
                if !g.pixel(gx, gy) {
                    continue;
                }
                for (dx, dy) in [(1u32, 0u32), (0, 1)] {
                    if gx + dx < 5 && gy + dy < 7 && g.pixel(gx + dx, gy + dy) {
                        let cx = origin[0]
                            + (i5 * 6.0 + gx as f32 + 0.5 + 0.5 * dx as f32) * unit;
                        let cy = origin[1] + (gy as f32 + 0.5 + 0.5 * dy as f32) * unit;
                        let p = px_at(&img, [cx * 4.0, cy * 4.0]);
                        assert_close(p, fg, 6.0, "stroke between adjacent lit dots");
                        joins += 1;
                    }
                }
            }
        }
        assert!(joins >= 8);
        // Nothing is drawn outside the text box (+ half a stroke of slack).
        let x_end = (origin[0] + (text.len() as f32 * 6.0) * unit + 2.0) * 4.0;
        let y_end = (origin[1] + cell + 2.0) * 4.0;
        assert_close(
            mean(&img, x_end as u32, 0, 1280, 720),
            bg,
            0.5,
            "right of the text",
        );
        assert_close(mean(&img, 0, y_end as u32, 1280, 720), bg, 0.5, "below the text");
        assert_close(
            mean(&img, 0, 0, 1280, ((origin[1] - 2.0) * 4.0) as u32),
            bg,
            0.5,
            "above the text",
        );
    }
    // Text colour alpha composites like everything else, and later items cover text.
    let list = lcd_list(vec![
        LcdItem::Text {
            text: "8".to_string(),
            origin: [100.0, 40.0],
            cell: 70.0,
            color: [255, 255, 255, 128],
        },
        LcdItem::Rect {
            rect: [100.0, 75.0, 320.0, 180.0],
            color: [0, 0, 0, 255],
        },
    ]);
    let img = render_lcd_to_image(&ctx, spec, &list, &images, 640, 360).unwrap();
    // '8' has its top bar at row 0, columns 1..=3: centre of dot (2, 0).
    let p = px_at(&img, [(100.0 + 25.0) * 2.0, (40.0 + 5.0) * 2.0]);
    assert_close(p, lerp3(bg, [255.0; 3], 128.0 / 255.0), 3.0, "text α 0.5");
    // Bottom bar (row 6) is under the black rect.
    let p = px_at(&img, [(100.0 + 25.0) * 2.0, (40.0 + 65.0) * 2.0]);
    assert_close(p, [0.0; 3], 2.0, "rect drawn after the text covers it");
}

#[test]
fn lcd_canvas_is_fitted_16_9_with_black_letterbox() {
    let Some(ctx) = context_for_tests("lcd_letterbox") else {
        return;
    };
    let spec = LcdSpec::new(1920, 1080).unwrap();
    let list = LcdDrawList {
        width: 1920,
        height: 1080,
        background: [60, 120, 180],
        items: vec![LcdItem::Rect {
            rect: [0.0, 0.0, 960.0, 540.0],
            color: [250, 250, 0, 255],
        }],
    };
    let images = ImageSet::new();
    // Square target: the 16:9 canvas is 512 × 288, centred (rows 112..400).
    let img = render_lcd_to_image(&ctx, spec, &list, &images, 512, 512).unwrap();
    assert_eq!(fit_aspect(Rect::new(0, 0, 512, 512), 16, 9), Rect::new(0, 112, 512, 288));
    assert_close(mean(&img, 0, 0, 512, 110), [0.0; 3], 0.5, "top letterbox is black");
    assert_close(mean(&img, 0, 402, 512, 512), [0.0; 3], 0.5, "bottom letterbox is black");
    assert_close(mean(&img, 4, 116, 252, 252), [250.0, 250.0, 0.0], 1.5, "top-left quadrant");
    assert_close(mean(&img, 260, 116, 508, 396), [60.0, 120.0, 180.0], 1.5, "right half");
    assert_close(mean(&img, 4, 260, 252, 396), [60.0, 120.0, 180.0], 1.5, "bottom-left");
    // Wide target (4:1 score PiP shape): bars left and right.
    let img = render_lcd_to_image(&ctx, spec, &list, &images, 640, 160).unwrap();
    let r = fit_aspect(Rect::new(0, 0, 640, 160), 16, 9);
    assert_eq!((r.w, r.h), (284, 160));
    assert_close(mean(&img, 0, 0, (r.x - 2) as u32, 160), [0.0; 3], 0.5, "left bar");
    assert_close(
        mean(&img, (r.x + 286) as u32, 0, 640, 160),
        [0.0; 3],
        0.5,
        "right bar",
    );
    assert_close(
        mean(&img, (r.x + 146) as u32, 4, (r.x + 280) as u32, 156),
        [60.0, 120.0, 180.0],
        1.5,
        "canvas right half",
    );
}

// ---------------------------------------------------------------------------------------
// Change detection: no upload / redraw when nothing changed
// ---------------------------------------------------------------------------------------

#[test]
fn dmd_and_lcd_renderers_report_change_only_on_new_content() {
    let Some(ctx) = context_for_tests("change_detection_score") else {
        return;
    };
    let spec = DmdSpec::new(128, 32, DmdColor::Orange).unwrap();
    let mut dmd = DmdRenderer::new(&ctx, FORMAT, spec);
    assert_eq!(dmd.spec(), spec);
    let (a, _) = text_frame(&spec, "1,000", false, 2, 2, 2, [255, 255, 255]);
    let (b, _) = text_frame(&spec, "1,010", false, 2, 2, 2, [255, 255, 255]);
    assert!(dmd.set_frame(ctx.queue(), &a), "first frame is a change");
    assert!(!dmd.set_frame(ctx.queue(), &a), "same frame: no upload");
    assert!(!dmd.set_frame(ctx.queue(), &a.clone()), "equal frame: no upload");
    assert!(dmd.set_frame(ctx.queue(), &b));
    assert!(!dmd.set_frame(ctx.queue(), &b));
    // One dot, one level.
    let mut c = b.clone();
    let d = c.get(127, 31);
    c.set(127, 31, [d[0] ^ 17, d[1], d[2]]);
    assert!(dmd.set_frame(ctx.queue(), &c), "a single-dot change is a change");
    assert!(!dmd.set_frame(ctx.queue(), &c));
    assert!(dmd.set_frame(ctx.queue(), &a), "going back is a change");

    let lspec = LcdSpec::new(320, 180).unwrap();
    let mut lcd = LcdRenderer::new(&ctx, FORMAT, lspec);
    let l1 = lcd_list(vec![LcdItem::Text {
        text: "1,000".into(),
        origin: [10.0, 10.0],
        cell: 28.0,
        color: [255, 255, 255, 255],
    }]);
    let mut l2 = l1.clone();
    l2.items.push(LcdItem::Rect {
        rect: [0.0, 0.0, 5.0, 5.0],
        color: [1, 2, 3, 4],
    });
    assert!(lcd.set_draw_list(ctx.queue(), &l1));
    assert!(!lcd.set_draw_list(ctx.queue(), &l1));
    assert!(!lcd.set_draw_list(ctx.queue(), &l1.clone()));
    assert!(lcd.set_draw_list(ctx.queue(), &l2));
    assert!(!lcd.set_draw_list(ctx.queue(), &l2));
    let mut l3 = l2.clone();
    l3.background = [0, 0, 1];
    assert!(lcd.set_draw_list(ctx.queue(), &l3), "background change");
    assert!(!lcd.set_draw_list(ctx.queue(), &l3));

    // ScoreRenderer::update wraps both.
    let mut sr = ScoreRenderer::Dmd(DmdRenderer::new(&ctx, FORMAT, spec));
    let ca = DisplayContent::Dmd(a.clone());
    let cb = DisplayContent::Dmd(b.clone());
    assert!(sr.update(ctx.queue(), &ca));
    assert!(!sr.update(ctx.queue(), &ca));
    assert!(sr.update(ctx.queue(), &cb));
    assert!(!sr.update(ctx.queue(), &cb));
    let mut sr = ScoreRenderer::Lcd(LcdRenderer::new(&ctx, FORMAT, lspec));
    let c1 = DisplayContent::Lcd(l1);
    let c2 = DisplayContent::Lcd(l2);
    assert!(sr.update(ctx.queue(), &c1));
    assert!(!sr.update(ctx.queue(), &c1));
    assert!(sr.update(ctx.queue(), &c2));
    assert!(!sr.update(ctx.queue(), &c2));
}

#[test]
fn score_model_drives_renderer_updates_only_when_scores_or_scenes_change() {
    let Some(ctx) = context_for_tests("change_detection_model") else {
        return;
    };
    let spec = DmdSpec::new(128, 32, DmdColor::Orange).unwrap();
    let mut model =
        ScoreDisplayModel::new(ScoreDisplayConfig::Dmd(spec), SceneLibrary::placeholders());
    let mut sr = ScoreRenderer::Dmd(DmdRenderer::new(&ctx, FORMAT, spec));
    let mut q = pin_core::script::HostCommandQueue::default();
    assert!(sr.update(ctx.queue(), &model.compose(0)));
    // 500 "frames" of nothing: the static score view never needs a redraw.
    for t in 1..=500u64 {
        model.apply_commands(&q, t);
        assert!(
            !sr.update(ctx.queue(), &model.compose(t)),
            "idle score view redrew at t = {t} ms"
        );
    }
    q.set_score(0, 5_000).unwrap();
    model.apply_commands(&q, 501);
    assert!(sr.update(ctx.queue(), &model.compose(501)), "score change redraws");
    // The same command again (scripts re-send absolute scores): no redraw.
    model.apply_commands(&q, 502);
    assert!(!sr.update(ctx.queue(), &model.compose(502)));
    q.clear();
    let sid = model_scene(&model, "scene_powerball_mania");
    q.show_scene(sid).unwrap();
    model.apply_commands(&q, 503);
    assert!(
        sr.update(ctx.queue(), &model.compose(503)),
        "showing a scene redraws"
    );
}

fn model_scene(_model: &ScoreDisplayModel, name: &str) -> pin_core::SceneId {
    SceneLibrary::placeholders()
        .id_of(name)
        .unwrap_or_else(|| panic!("placeholder scene {name} missing"))
}

fn encode_backglass(
    ctx: &GpuContext,
    r: &mut BackglassRenderer,
    target: &Target,
    viewport: Rect,
    lamps: &[f32],
) {
    let mut enc = ctx.device().create_command_encoder(&Default::default());
    r.encode(&mut enc, &target.view, (target.w, target.h), viewport, lamps);
    ctx.queue().submit([enc.finish()]);
}

#[test]
fn backglass_needs_redraw_only_when_a_referenced_lamp_moves() {
    let Some(ctx) = context_for_tests("change_detection_backglass") else {
        return;
    };
    let desc = lamp_desc(LayerBlend::Over, LIT, BASE);
    let mut r = BackglassRenderer::new(&ctx, FORMAT, &desc).unwrap();
    assert_eq!(r.lamps(), vec![LampId(5)]);
    assert_eq!(r.aspect().0 * 3, r.aspect().1 * 4, "96 × 72 is 4:3");
    assert!((LAMP_REDRAW_EPSILON - 1.0 / 1024.0).abs() < 1e-9);
    let target = Target::new(&ctx, 384, 288, MAGENTA);
    let vp = Rect::new(0, 0, 384, 288);

    let mut lamps = lamps_with(5, 0.25);
    assert!(r.needs_redraw(&lamps), "never drawn → needs a redraw");
    encode_backglass(&ctx, &mut r, &target, vp, &lamps);
    assert!(!r.needs_redraw(&lamps), "just drawn with these lamps");
    assert!(!r.needs_redraw(&lamps), "asking twice changes nothing");

    // Unreferenced lamps moving: no redraw.
    lamps[0] = 0.0;
    lamps[4] = 0.3;
    lamps[6] = 0.9;
    lamps[15] = 1.0;
    assert!(!r.needs_redraw(&lamps), "only lamp 5 is referenced");
    // Below the epsilon: no redraw.
    lamps[5] = 0.25 + 0.4 / 1024.0;
    assert!(!r.needs_redraw(&lamps), "sub-epsilon lamp change");
    // Above it: redraw, until encoded.
    lamps[5] = 0.25 + 2.0 / 1024.0;
    assert!(r.needs_redraw(&lamps));
    assert!(r.needs_redraw(&lamps), "stays dirty until encoded");
    encode_backglass(&ctx, &mut r, &target, vp, &lamps);
    assert!(!r.needs_redraw(&lamps));
    // Slow drift accumulates against the last *drawn* level, not the last asked one.
    let drawn = lamps[5];
    let mut dirty_at = None;
    for i in 1..=8 {
        lamps[5] = drawn + i as f32 * 0.3 / 1024.0;
        if r.needs_redraw(&lamps) {
            dirty_at = Some(i);
            break;
        }
    }
    assert_eq!(
        dirty_at,
        Some(4),
        "0.3 ε steps: dirty once the drift from the drawn level reaches ε (step 4 = 1.2 ε)"
    );
    encode_backglass(&ctx, &mut r, &target, vp, &lamps);
    assert!(!r.needs_redraw(&lamps));
    // mark_dirty forces one redraw.
    r.mark_dirty();
    assert!(r.needs_redraw(&lamps));
    encode_backglass(&ctx, &mut r, &target, vp, &lamps);
    assert!(!r.needs_redraw(&lamps));
    // A shorter lamp slice means lamp 5 = 0: that is a change.
    assert!(r.needs_redraw(&[0.0; 3]));

    // And what was drawn is the picture for the lamps given.
    lamps[5] = 1.0;
    encode_backglass(&ctx, &mut r, &target, vp, &lamps);
    let img = target.read(&ctx);
    assert_close(region(&img, R_FULL), f3(LIT), 2.0, "renderer output, lamp 1");
    assert_close(region(&img, R_OUT), f3(BASE), 1.0, "renderer output outside mask");
    // The offscreen helper and the renderer agree.
    let helper = render_backglass_to_image(&ctx, &desc, &lamps, 384, 288).unwrap();
    for r in [R_FULL, R_HALF, R_OUT] {
        assert_close(region(&img, r), region(&helper, r), 1.0, "helper = renderer");
    }
}

// ---------------------------------------------------------------------------------------
// Routing: PiP regions vs dedicated windows
// ---------------------------------------------------------------------------------------

fn display(index: usize, x: i32, w: u32, h: u32) -> DisplayInfo {
    DisplayInfo {
        index,
        bounds: Rect::new(x, 0, w, h),
        refresh_hz: 60.0,
        content_scale: 1.0,
    }
}

fn all_lit(spec: &DmdSpec) -> DmdFrame {
    let mut f = DmdFrame::new(spec.width, spec.height);
    for y in 0..spec.height {
        for x in 0..spec.width {
            f.set(x, y, [255, 255, 255]);
        }
    }
    f
}

fn display_set(ctx: &GpuContext, spec: DmdSpec) -> DisplaySet {
    // A lamp-free backglass of one flat colour, and an all-lit orange DMD.
    let desc = BackglassDesc {
        layers: vec![BackglassLayer {
            image: solid(64, 48, BASE),
            lamp: None,
        }],
    };
    let backglass = BackglassRenderer::new(ctx, FORMAT, &desc).unwrap();
    let mut dmd = DmdRenderer::new(ctx, FORMAT, spec);
    assert!(dmd.set_frame(ctx.queue(), &all_lit(&spec)));
    DisplaySet {
        backglass: Some(backglass),
        score: Some(ScoreRenderer::Dmd(dmd)),
    }
}

fn encode_window(
    ctx: &GpuContext,
    set: &mut DisplaySet,
    window: usize,
    routes: &[DisplayRoute],
    target: &Target,
) -> RgbaImage {
    let mut enc = ctx.device().create_command_encoder(&Default::default());
    set.encode_window(
        window,
        routes,
        &mut enc,
        &target.view,
        (target.w, target.h),
        &[],
    );
    ctx.queue().submit([enc.finish()]);
    target.read(ctx)
}

fn inside(r: &Rect, x: u32, y: u32) -> bool {
    i64::from(x) >= i64::from(r.x)
        && i64::from(x) < r.right()
        && i64::from(y) >= i64::from(r.y)
        && i64::from(y) < r.bottom()
}

#[test]
fn routing_single_monitor_draws_both_displays_into_their_pip_regions_only() {
    let Some(ctx) = context_for_tests("routing_pip") else {
        return;
    };
    let spec = DmdSpec::new(128, 32, DmdColor::Orange).unwrap();
    let mut set = display_set(&ctx, spec);
    let displays = [display(0, 0, 1024, 768)];
    let plan = plan_topology(
        &displays,
        &TopologyMode::SingleMonitor {
            display: 0,
            backglass: PipPlacement {
                corner: Corner::TopRight,
                width_fraction: 0.25,
            },
            score: PipPlacement {
                corner: Corner::BottomLeft,
                width_fraction: 0.5,
            },
        },
    )
    .unwrap();
    let routes = display_routes(&plan);
    assert_eq!(routes.len(), 2);
    let bg = routes.iter().find(|r| r.role == SurfaceRole::Backglass).unwrap();
    let sc = routes.iter().find(|r| r.role == SurfaceRole::Score).unwrap();
    assert_eq!((bg.window, bg.dedicated), (0, false));
    assert_eq!((sc.window, sc.dedicated), (0, false));
    assert_eq!(bg.viewport, Rect::new(768, 0, 256, 192));
    assert_eq!(sc.viewport, Rect::new(0, 640, 512, 128));

    let target = Target::new(&ctx, 1024, 768, MAGENTA);
    let img = encode_window(&ctx, &mut set, 0, &routes, &target);
    // The playfield (everything outside the two PiPs) is untouched.
    let mut touched = 0u32;
    for y in 0..768 {
        for x in 0..1024 {
            let in_pip = inside(&bg.viewport, x, y) || inside(&sc.viewport, x, y);
            let mag = is_magenta(px(&img, x, y));
            if !in_pip && !mag {
                touched += 1;
            }
            if in_pip {
                assert!(!mag, "PiP pixel ({x},{y}) was not painted");
            }
        }
    }
    assert_eq!(touched, 0, "{touched} pixels outside the PiP regions were drawn");
    // Backglass PiP: the flat base colour (4:3 art in a 4:3 PiP: no letterbox).
    assert_close(
        mean(&img, 770, 2, 1022, 190),
        f3(BASE),
        1.5,
        "backglass PiP content",
    );
    // Score PiP: 128 × 32 dots at pitch 4 fill the 512 × 128 viewport exactly.
    let geo = dmd_geometry(&spec, sc.viewport);
    assert!((geo.pitch - 4.0).abs() < 1e-4);
    let lit = to255(dmd_dot_color(DmdColor::Orange, [255, 255, 255]));
    for (x, y) in [(0u32, 0u32), (127, 0), (0, 31), (127, 31), (64, 16), (5, 20)] {
        let c = geo.dot_center(x, y);
        assert!(
            inside(&sc.viewport, c[0] as u32, c[1] as u32),
            "dot centre {c:?} outside the PiP"
        );
        assert_close(px_at(&img, c), lit, 4.0, &format!("PiP dot ({x},{y})"));
        // Between dots: background.
        if x < 127 && y < 31 {
            assert_close(
                px_at(&img, [c[0] + 2.0 - 0.5, c[1] + 2.0 - 0.5]),
                [0.0; 3],
                4.0,
                "PiP dot gap",
            );
        }
    }
    // Other windows get nothing.
    let other = Target::new(&ctx, 256, 64, MAGENTA);
    let img = encode_window(&ctx, &mut set, 1, &routes, &other);
    assert!(
        (0..64).all(|y| (0..256).all(|x| is_magenta(px(&img, x, y)))),
        "window 1 has no route in single-monitor mode and must stay untouched"
    );
}

#[test]
fn routing_cabinet_draws_each_display_in_its_dedicated_window() {
    let Some(ctx) = context_for_tests("routing_dedicated") else {
        return;
    };
    let spec = DmdSpec::new(128, 32, DmdColor::Orange).unwrap();
    let mut set = display_set(&ctx, spec);
    let displays = [
        display(0, 0, 640, 384),
        display(1, 640, 512, 384),
        display(2, 1152, 512, 128),
    ];
    let plan = plan_topology(
        &displays,
        &TopologyMode::Cabinet {
            playfield: 0,
            backglass: Some(1),
            score: Some(2),
        },
    )
    .unwrap();
    let routes = display_routes(&plan);
    assert_eq!(routes.len(), 2);
    assert_eq!(routes[0].role, SurfaceRole::Backglass);
    assert_eq!(routes[1].role, SurfaceRole::Score);
    assert_eq!((routes[0].window, routes[0].dedicated), (1, true));
    assert_eq!((routes[1].window, routes[1].dedicated), (2, true));
    assert_eq!(routes[0].viewport, Rect::new(0, 0, 512, 384));
    assert_eq!(routes[1].viewport, Rect::new(0, 0, 512, 128));

    // Playfield window: nothing from the displays.
    let t0 = Target::new(&ctx, 640, 384, MAGENTA);
    let img = encode_window(&ctx, &mut set, 0, &routes, &t0);
    assert!(
        (0..384).all(|y| (0..640).all(|x| is_magenta(px(&img, x, y)))),
        "dedicated displays must not draw into the playfield window"
    );
    // Backglass window: the art fills the whole window (4:3 in 4:3), no DMD dots.
    let t1 = Target::new(&ctx, 512, 384, MAGENTA);
    let img = encode_window(&ctx, &mut set, 1, &routes, &t1);
    assert_close(mean(&img, 0, 0, 512, 384), f3(BASE), 1.0, "backglass window");
    // Score window: the DMD fills the whole window at pitch 4.
    let t2 = Target::new(&ctx, 512, 128, MAGENTA);
    let img = encode_window(&ctx, &mut set, 2, &routes, &t2);
    let geo = dmd_geometry(&spec, routes[1].viewport);
    let lit = to255(dmd_dot_color(DmdColor::Orange, [255, 255, 255]));
    for y in (0..32).step_by(5) {
        for x in (0..128).step_by(7) {
            assert_close(px_at(&img, geo.dot_center(x, y)), lit, 4.0, "DMD window dot");
        }
    }
    assert!(
        (0..128).all(|y| (0..512).all(|x| !is_magenta(px(&img, x, y)))),
        "the score window is fully painted"
    );
    let m = mean(&img, 0, 0, 512, 128);
    assert!(m[0] > 90.0 && m[2] < 30.0, "orange dots, mean {m:?}");

    // Mixed: backglass dedicated, score as a PiP on the playfield.
    let plan = plan_topology(
        &displays[..2],
        &TopologyMode::Cabinet {
            playfield: 0,
            backglass: Some(1),
            score: None,
        },
    )
    .unwrap();
    let routes = display_routes(&plan);
    let bg = routes.iter().find(|r| r.role == SurfaceRole::Backglass).unwrap();
    let sc = routes.iter().find(|r| r.role == SurfaceRole::Score).unwrap();
    assert_eq!((bg.window, bg.dedicated), (1, true));
    assert_eq!((sc.window, sc.dedicated), (0, false));
    assert_eq!(Some((0, sc.viewport)), plan.find(SurfaceRole::Score));
    let t0 = Target::new(&ctx, 640, 384, MAGENTA);
    let img = encode_window(&ctx, &mut set, 0, &routes, &t0);
    for y in 0..384 {
        for x in 0..640 {
            let mag = is_magenta(px(&img, x, y));
            assert_eq!(
                !mag,
                inside(&sc.viewport, x, y),
                "pixel ({x},{y}): painted = {} but score PiP = {:?}",
                !mag,
                sc.viewport
            );
        }
    }
    // A set without a score renderer draws only the backglass.
    let mut set = display_set(&ctx, spec);
    set.score = None;
    let t0 = Target::new(&ctx, 640, 384, MAGENTA);
    let img = encode_window(&ctx, &mut set, 0, &routes, &t0);
    assert!((0..384).all(|y| (0..640).all(|x| is_magenta(px(&img, x, y)))));
    let t1 = Target::new(&ctx, 512, 384, MAGENTA);
    let img = encode_window(&ctx, &mut set, 1, &routes, &t1);
    assert_close(mean(&img, 0, 0, 512, 384), f3(BASE), 1.0, "backglass only");
}
