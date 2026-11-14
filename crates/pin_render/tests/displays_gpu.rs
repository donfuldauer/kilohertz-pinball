//! 5.3 Implementer GPU tests (`gpu-tests`, lavapipe in CI). D30: analytic
//! pixel assertions first, then PNG regression snapshots in
//! `tests/fixtures/displays/` (tolerance: mean |Δ| ≤ 3/255 and ≤ 2 % of
//! channels off by more than 16/255; `PIN_UPDATE_SNAPSHOTS=1` rewrites them).
//! Every rendered image is also written to
//! `$CARGO_TARGET_TMPDIR/5_3_images/` for the Reviewer's visual checklist.
#![cfg(feature = "gpu-tests")]

use pin_assetgen::TableMeta;
use pin_assetgen::art::{self, ColorType, Image};
use pin_core::LampId;
use pin_core::schema::backglass::DmdColor;
use pin_render::displays::*;
use pin_render::gpu::{GpuContext, context_for_tests};
use pin_render::topology::{
    DEFAULT_BACKGLASS_PIP, DEFAULT_SCORE_PIP, DisplayInfo, Rect, TopologyMode, plan_topology,
};
use std::sync::OnceLock;

/// One device per test process, shared by every test (wave 5 gate fix: a
/// device per test thread crashed lavapipe in CI, as in DECISIONS "1.4 — CI
/// fix: lavapipe SIGSEGV and macOS occlusion"; post_gpu, reflections_gpu and
/// gpu_pbr_render share theirs the same way). `name` labels the first log line.
fn ctx(name: &str) -> Option<&'static GpuContext> {
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    CTX.get_or_init(|| context_for_tests(name)).as_ref()
}

fn out_dir() -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("5_3_images");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn save(name: &str, img: &RgbaImage) {
    image::save_buffer(
        out_dir().join(format!("{name}.png")),
        &img.pixels,
        img.width,
        img.height,
        image::ColorType::Rgba8,
    )
    .unwrap();
}

fn snapshot(name: &str, img: &RgbaImage) {
    save(name, img);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/displays")
        .join(format!("{name}.png"));
    if std::env::var("PIN_UPDATE_SNAPSHOTS").is_ok_and(|v| v.trim() == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::save_buffer(
            &path,
            &img.pixels,
            img.width,
            img.height,
            image::ColorType::Rgba8,
        )
        .unwrap();
        return;
    }
    let want = image::open(&path)
        .unwrap_or_else(|e| {
            panic!(
                "[{name}] missing snapshot {} ({e}); run with PIN_UPDATE_SNAPSHOTS=1 after the analytic checks pass",
                path.display()
            )
        })
        .to_rgba8();
    assert_eq!((want.width(), want.height()), (img.width, img.height));
    let (mut sum, mut big) = (0u64, 0usize);
    for (a, b) in want.as_raw().iter().zip(&img.pixels) {
        let d = a.abs_diff(*b);
        sum += u64::from(d);
        big += usize::from(d > 16);
    }
    let n = img.pixels.len();
    let mean = sum as f64 / n as f64;
    assert!(
        mean <= 3.0 && big as f64 <= 0.02 * n as f64,
        "[{name}] snapshot differs: mean {mean:.2}, {big} channels > 16"
    );
}

fn px(img: &RgbaImage, x: f32, y: f32) -> [u8; 4] {
    img.pixel(x.floor() as u32, y.floor() as u32)
}

fn luma(p: [u8; 4]) -> f32 {
    0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])
}

fn close(p: [u8; 4], want: [f32; 3], tol: f32) -> bool {
    (0..3).all(|i| (f32::from(p[i]) - want[i] * 255.0).abs() <= tol)
}

fn text_scene(text: &str, size: f32) -> Scene {
    Scene::from_toml_str(&format!(
        "schema_version = 1\n[[layers]]\nkind = \"text\"\ntext = \"{text}\"\nx = 0.5\ny = 0.5\nsize = {size}\ncolor = \"#ff8040\""
    ))
    .unwrap()
}

// ---------- DMD ----------

/// Checks every dot of `frame` rendered at `pitch` px: centre colour = CPU
/// reference, cell corners = background, round dots (lit at 0.8 r on the
/// axes and diagonals, dark at 1.25 r), lit/unlit luminance ratio.
fn check_dmd(name: &str, spec: DmdSpec, frame: &DmdFrame, pitch: u32) -> RgbaImage {
    let ctx = ctx(name).unwrap();
    let (w, h) = (spec.width * pitch, spec.height * pitch);
    let img = render_dmd_to_image(ctx, spec, frame, w, h).unwrap();
    let g = dmd_geometry(&spec, Rect::new(0, 0, w, h));
    assert_eq!(g.pitch, pitch as f32);
    let (mut lit, mut unlit) = (Vec::new(), Vec::new());
    for y in 0..spec.height {
        for x in 0..spec.width {
            let drive = frame.get(x, y);
            let want = dmd_dot_color(spec.color, drive);
            let [cx, cy] = g.dot_center(x, y);
            let p = px(&img, cx, cy);
            assert!(
                close(p, want, 2.0),
                "{name} dot ({x},{y}) {p:?} vs {want:?}"
            );
            // Cell corner: background.
            let c = px(&img, cx - 0.5 * g.pitch + 0.5, cy - 0.5 * g.pitch + 0.5);
            assert_eq!(&c[..3], &[0, 0, 0], "{name} gap at ({x},{y})");
            if pitch == 8 {
                // Round: pixels whose centres are 2.12 px from the dot centre
                // are fully lit on the axis and the diagonal; pixels at
                // 3.54 px (radius 3.2 + AA) are equally partial on both.
                for (ox, oy) in [(1.5, 1.5), (-1.5, 1.5), (1.5, -1.5), (-1.5, -1.5)] {
                    let inner = px(&img, cx + ox, cy + oy);
                    assert!(close(inner, want, 3.0), "{name} round inside ({x},{y})");
                }
                let axis = luma(px(&img, cx + 3.5, cy + 0.5));
                let diag = luma(px(&img, cx + 2.5, cy + 2.5));
                assert!(
                    (axis - diag).abs() <= 2.0,
                    "{name} round ({x},{y}): {axis} vs {diag}"
                );
                assert!(axis <= 0.5 * luma(p) + 1.0, "{name} edge ({x},{y})");
            }
            if drive == [255; 3] {
                lit.push(luma(p));
            } else if drive == [0; 3] {
                unlit.push(luma(p));
            }
        }
    }
    let mean = |v: &[f32]| v.iter().sum::<f32>() / v.len() as f32;
    if !lit.is_empty() && !unlit.is_empty() {
        let ratio = mean(&lit) / mean(&unlit);
        assert!(
            (8.0..=12.0).contains(&ratio),
            "{name} lit/unlit ratio {ratio}"
        );
        assert!(mean(&unlit) > 2.0, "{name}: unlit dots faintly visible");
    }
    img
}

#[test]
fn dmd_text_dots_pitch_and_colour() {
    if ctx("dmd_text_dots_pitch_and_colour").is_none() {
        return;
    }
    let st = ScoreState::default();
    let im = ImageSet::new();
    for (color, tag) in [
        (DmdColor::Orange, "orange"),
        (DmdColor::Red, "red"),
        (DmdColor::Rgb, "rgb"),
    ] {
        for (w, h, pitch, text) in [(128, 32, 8, "KILOHERTZ"), (192, 64, 6, "SCORE 1,234")] {
            let spec = DmdSpec::new(w, h, color).unwrap();
            let frame = DmdFrame::compose(&spec, &text_scene(text, 0.5), 0, &st, &im);
            assert!(frame.lit_count() > 50);
            let name = format!("dmd_text_{w}x{h}_{tag}");
            let img = check_dmd(&name, spec, &frame, pitch);
            if w == 128 && color == DmdColor::Orange {
                snapshot("dmd_text_128x32_orange", &img);
            } else {
                save(&name, &img);
            }
        }
    }
}

#[test]
fn dmd_letterboxes_in_a_pip() {
    let Some(ctx) = ctx("dmd_letterboxes_in_a_pip") else {
        return;
    };
    let mut frame = DmdFrame::new(128, 32);
    for d in &mut frame.dots {
        *d = [255; 3];
    }
    // 4:1 DMD in a 600 × 300 viewport: pitch 600/128, centred vertically.
    let img = render_dmd_to_image(ctx, DMD_128X32, &frame, 600, 300).unwrap();
    let g = dmd_geometry(&DMD_128X32, Rect::new(0, 0, 600, 300));
    assert!((g.origin[1] - (300.0 - g.pitch * 32.0) / 2.0).abs() < 1e-3);
    assert_eq!(&img.pixel(300, 2)[..3], &[0, 0, 0], "letterbox");
    let [cx, cy] = g.dot_center(64, 16);
    assert!(close(px(&img, cx, cy), dmd_tint(DmdColor::Orange), 2.0));
    assert_eq!(img.pixel(300, 2)[3], 255, "viewport fully painted");
}

#[test]
fn dmd_score_view_golden() {
    let Some(ctx) = ctx("dmd_score_view_golden") else {
        return;
    };
    let mut model = ScoreDisplayModel::new(
        ScoreDisplayConfig::Dmd(DMD_128X32),
        SceneLibrary::placeholders(),
    );
    model.scores_mut().scores[0] = 1_234_567;
    model.scores_mut().ball = 2;
    model.scores_mut().credits = 3;
    let DisplayContent::Dmd(frame) = model.compose(0) else {
        panic!()
    };
    // Analytic: the score text "1,234,567" in 8×16 is centred at y 0.38.
    let text_w = dmd_text_dots("1,234,567", DmdFont::Font8x16, 1).0;
    let x0 = (64.0 - text_w as f32 / 2.0).round() as u32;
    let cols: Vec<u32> = (0..128)
        .filter(|&x| (0..20).any(|y| frame.get(x, y)[0] > 0))
        .collect();
    assert!(*cols.first().unwrap() >= x0 && *cols.last().unwrap() < x0 + text_w);
    let img = check_dmd("dmd_score_view", DMD_128X32, &frame, 4);
    let _ = ctx;
    snapshot("dmd_score_128x32", &img);
    // Multi-player at 192×64, RGB.
    let spec = DmdSpec::new(192, 64, DmdColor::Rgb).unwrap();
    let mut model = ScoreDisplayModel::new(ScoreDisplayConfig::Dmd(spec), SceneLibrary::new());
    let s = model.scores_mut();
    s.scores = [1_000_000, 250_500, 75, 42_000_000];
    s.players = 4;
    s.current_player = 2;
    s.ball = 3;
    let DisplayContent::Dmd(frame) = model.compose(0) else {
        panic!()
    };
    let img = check_dmd("dmd_score_192x64_rgb", spec, &frame, 4);
    save("dmd_score_192x64_rgb", &img);
}

#[test]
fn dmd_placeholder_scenes_render() {
    let Some(ctx) = ctx("dmd_placeholder_scenes_render") else {
        return;
    };
    let lib = SceneLibrary::placeholders();
    let mut st = ScoreState::default();
    st.scores[0] = 4_200_000;
    let im = ImageSet::new();
    for (id, _) in PLACEHOLDER_SCENES {
        let scene = lib.get(lib.id_of(id).unwrap()).unwrap();
        let spec = DmdSpec::new(128, 32, DmdColor::Rgb).unwrap();
        // Some moment of the first 3 s shows content; save a lit one.
        let t = (0..3000)
            .step_by(50)
            .find(|&t| DmdFrame::compose(&spec, scene, t + 450, &st, &im).lit_count() > 20)
            .unwrap_or_else(|| panic!("{id} shows nothing"));
        let frame = DmdFrame::compose(&spec, scene, t + 450, &st, &im);
        let img = render_dmd_to_image(ctx, spec, &frame, 512, 128).unwrap();
        save(&format!("scene_{id}"), &img);
    }
}

// ---------- backglass ----------

fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
    RgbaImage::from_rgba(w, h, c.repeat((w * h) as usize)).unwrap()
}

fn half_mask(w: u32, h: u32) -> RgbaImage {
    let mut v = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w / 2 {
            v[(y * w + x) as usize] = 255;
        }
    }
    RgbaImage::from_gray(w, h, v).unwrap()
}

#[test]
fn lamp_mask_blending_is_linear_and_local() {
    let Some(ctx) = ctx("lamp_mask_blending_is_linear_and_local") else {
        return;
    };
    let (w, h) = (64, 48);
    let base = [40u8, 80, 120, 255];
    let lit = [240u8, 200, 40, 255];
    for blend in [LayerBlend::Over, LayerBlend::Additive] {
        let desc = BackglassDesc {
            layers: vec![
                BackglassLayer {
                    image: solid(w, h, base),
                    lamp: None,
                },
                BackglassLayer {
                    image: solid(w, h, lit),
                    lamp: Some(LayerLamp {
                        lamp: LampId(7),
                        mask: half_mask(w, h),
                        blend,
                    }),
                },
            ],
        };
        let render = |i: f32| {
            let mut lamps = [0.0f32; 16];
            lamps[7] = i;
            render_backglass_to_image(ctx, &desc, &lamps, w, h).unwrap()
        };
        let imgs: Vec<_> = [0.0, 0.5, 1.0].map(render).into();
        for (k, i) in [0.0f32, 0.5, 1.0].iter().enumerate() {
            for y in (0..h).step_by(5) {
                // Masked region (avoid the 1-px filtered seam).
                for x in (0..w / 2 - 1).step_by(5) {
                    let p = imgs[k].pixel(x, y);
                    for c in 0..3 {
                        let (b, l) = (f32::from(base[c]), f32::from(lit[c]));
                        let want = match blend {
                            LayerBlend::Over => b + (l - b) * i,
                            LayerBlend::Additive => (b + l * i).min(255.0),
                        };
                        assert!(
                            (f32::from(p[c]) - want).abs() <= 1.5,
                            "{blend:?} I={i} ({x},{y}) ch{c}: {} vs {want}",
                            p[c]
                        );
                    }
                }
                // Outside the mask: exactly unchanged.
                for x in (w / 2 + 1..w).step_by(3) {
                    assert_eq!(imgs[k].pixel(x, y), imgs[0].pixel(x, y));
                    assert_eq!(&imgs[k].pixel(x, y)[..3], &base[..3]);
                }
            }
        }
        // Linearity: I=0.5 is the midpoint of I=0 and I=1 (unclipped channels).
        for y in 0..h {
            for x in 0..w / 2 - 1 {
                for c in 0..3 {
                    let (a, m, b) = (
                        f32::from(imgs[0].pixel(x, y)[c]),
                        f32::from(imgs[1].pixel(x, y)[c]),
                        f32::from(imgs[2].pixel(x, y)[c]),
                    );
                    if b < 255.0 {
                        assert!((m - (a + b) / 2.0).abs() <= 1.5);
                    }
                }
            }
        }
    }
}

fn kit_image(img: Image) -> RgbaImage {
    match img.color {
        ColorType::Gray => RgbaImage::from_gray(img.width, img.height, img.data),
        ColorType::Rgb => RgbaImage::from_rgb(img.width, img.height, img.data),
        ColorType::Rgba => RgbaImage::from_rgba(img.width, img.height, img.data),
    }
    .unwrap()
}

#[test]
fn kit_backglass_lights_only_inside_mask() {
    let Some(ctx) = ctx("kit_backglass_lights_only_inside_mask") else {
        return;
    };
    let meta = TableMeta {
        title: "Apex Circuit".into(),
        palette: vec![],
        seed: 7,
    };
    let mask = kit_image(art::translite_logo_mask(&meta));
    // A flasher: a soft round glow in the upper right.
    let (fw, fh) = (512u32, 384u32);
    let mut glow = vec![0u8; (fw * fh) as usize];
    for y in 0..fh {
        for x in 0..fw {
            let d = ((x as f32 - 420.0).powi(2) + (y as f32 - 70.0).powi(2)).sqrt();
            glow[(y * fw + x) as usize] = (255.0 * (1.0 - d / 60.0).clamp(0.0, 1.0)) as u8;
        }
    }
    let desc = BackglassDesc {
        layers: vec![
            BackglassLayer {
                image: kit_image(art::translite_base(&meta)),
                lamp: None,
            },
            BackglassLayer {
                image: kit_image(art::translite_logo(&meta)),
                lamp: Some(LayerLamp {
                    lamp: LampId(0),
                    mask: mask.clone(),
                    blend: LayerBlend::Over,
                }),
            },
            BackglassLayer {
                image: solid(fw, fh, [255, 240, 200, 255]),
                lamp: Some(LayerLamp {
                    lamp: LampId(1),
                    mask: RgbaImage::from_gray(fw, fh, glow.clone()).unwrap(),
                    blend: LayerBlend::Additive,
                }),
            },
        ],
    };
    let off = render_backglass_to_image(ctx, &desc, &[0.0, 0.0], fw, fh).unwrap();
    let on = render_backglass_to_image(ctx, &desc, &[1.0, 1.0], fw, fh).unwrap();
    let (mut changed_in, mut changed_out) = (0, 0);
    for y in 0..fh {
        for x in 0..fw {
            let i = (y * fw + x) as usize;
            let masked = mask.pixel(x, y)[0] > 0 || glow[i] > 0;
            // Neighbours of the mask may change through bilinear filtering only
            // when the image is scaled; here it is 1:1.
            let diff = off.pixel(x, y) != on.pixel(x, y);
            if masked {
                changed_in += usize::from(diff);
            } else {
                changed_out += usize::from(diff);
            }
        }
    }
    assert_eq!(changed_out, 0, "pixels outside every mask changed");
    assert!(
        changed_in > 1000,
        "lamps light the masked areas ({changed_in})"
    );
    // Flasher is additive: the glow centre gets brighter in every channel.
    let (a, b) = (off.pixel(420, 70), on.pixel(420, 70));
    assert!((0..3).all(|c| b[c] >= a[c]) && luma(b) > luma(a) + 50.0);
    snapshot("backglass_kit_lit", &on);
    save("backglass_kit_off", &off);
    let half = render_backglass_to_image(ctx, &desc, &[0.5, 0.5], fw, fh).unwrap();
    save("backglass_kit_half", &half);
    // Renderer change detection.
    let mut r = BackglassRenderer::new(ctx, wgpu::TextureFormat::Rgba8Unorm, &desc).unwrap();
    assert_eq!(r.lamps(), vec![LampId(0), LampId(1)]);
    assert_eq!(r.aspect(), (512, 384));
    assert!(r.needs_redraw(&[0.0, 0.0]));
    let tex = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 64,
            height: 48,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    let mut enc = ctx.device().create_command_encoder(&Default::default());
    r.encode(
        &mut enc,
        &view,
        (64, 48),
        Rect::new(0, 0, 64, 48),
        &[0.3, 0.0],
    );
    ctx.queue().submit([enc.finish()]);
    assert!(
        !r.needs_redraw(&[0.3, 0.0, 1.0]),
        "unreferenced lamp ignored"
    );
    assert!(!r.needs_redraw(&[0.3004, 0.0]));
    assert!(r.needs_redraw(&[0.31, 0.0]));
    assert!(r.needs_redraw(&[0.3, 1.0]));
    r.mark_dirty();
    assert!(r.needs_redraw(&[0.3, 0.0]));
}

// ---------- LCD ----------

#[test]
fn lcd_layer_order_and_alpha() {
    let Some(ctx) = ctx("lcd_layer_order_and_alpha") else {
        return;
    };
    let spec = LcdSpec::new(160, 90).unwrap();
    let mut images = ImageSet::new();
    images.insert("blue.png", solid(4, 4, [0, 0, 255, 255]));
    let list = |items: Vec<LcdItem>| LcdDrawList {
        width: 160,
        height: 90,
        background: [0, 0, 0],
        items,
    };
    let red = LcdItem::Rect {
        rect: [0.0, 0.0, 160.0, 90.0],
        color: [255, 0, 0, 255],
    };
    let green_half = LcdItem::Rect {
        rect: [0.0, 0.0, 80.0, 90.0],
        color: [0, 255, 0, 128],
    };
    let blue_img = LcdItem::Image {
        file: "blue.png".into(),
        rect: [120.0, 0.0, 160.0, 45.0],
        color: [255, 255, 255, 64],
    };
    let a = render_lcd_to_image(
        ctx,
        spec,
        &list(vec![red.clone(), green_half.clone(), blue_img.clone()]),
        &images,
        160,
        90,
    )
    .unwrap();
    let al = 128.0 / 255.0;
    assert!(
        close(a.pixel(20, 50), [1.0 - al, al, 0.0], 2.0),
        "{:?}",
        a.pixel(20, 50)
    );
    assert!(close(a.pixel(100, 50), [1.0, 0.0, 0.0], 0.0));
    let bl = 64.0 / 255.0;
    assert!(
        close(a.pixel(140, 20), [1.0 - bl, 0.0, bl], 2.0),
        "{:?}",
        a.pixel(140, 20)
    );
    // Reversed order: the opaque red covers everything.
    let b = render_lcd_to_image(
        ctx,
        spec,
        &list(vec![green_half, blue_img, red]),
        &images,
        160,
        90,
    )
    .unwrap();
    assert!(close(b.pixel(20, 50), [1.0, 0.0, 0.0], 0.0));
    assert!(close(b.pixel(140, 20), [1.0, 0.0, 0.0], 0.0));
    save("lcd_layers", &a);
}

#[test]
fn lcd_score_legible_at_16_9() {
    let Some(ctx) = ctx("lcd_score_legible_at_16_9") else {
        return;
    };
    let spec = LcdSpec::default();
    let mut model = ScoreDisplayModel::new(ScoreDisplayConfig::Lcd(spec), SceneLibrary::new());
    model.scores_mut().scores[0] = 1_234_567;
    model.scores_mut().ball = 1;
    let DisplayContent::Lcd(list) = model.compose(0) else {
        panic!()
    };
    let (w, h) = (640, 360);
    let img = render_lcd_to_image(ctx, spec, &list, &ImageSet::new(), w, h).unwrap();
    // Analytic: text pixels lie inside the text boxes, everything else is the
    // black background; each glyph cell of the score has ink.
    let s = w as f32 / 1920.0;
    let mut boxes = Vec::new();
    for item in &list.items {
        let LcdItem::Text {
            text, origin, cell, ..
        } = item
        else {
            panic!()
        };
        let tw = vector_text_width(text, *cell);
        boxes.push([
            origin[0] * s - 1.0,
            origin[1] * s - 1.0,
            (origin[0] + tw) * s + 1.0,
            (origin[1] + cell) * s + 1.0,
        ]);
    }
    let mut ink = 0;
    for y in 0..h {
        for x in 0..w {
            let p = img.pixel(x, y);
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let inside = boxes
                .iter()
                .any(|b| fx >= b[0] && fx <= b[2] && fy >= b[1] && fy <= b[3]);
            if !inside {
                assert_eq!(&p[..3], &[0, 0, 0], "ink outside text at ({x},{y})");
            } else if p[..3].iter().all(|&c| c >= 250) {
                // Fully covered stroke pixels: crisp, full-brightness text.
                ink += 1;
            }
        }
    }
    assert!(ink > 4000, "score text drawn crisp ({ink} full-white px)");
    // Every non-space glyph of the score has ink in its cell.
    let LcdItem::Text {
        text, origin, cell, ..
    } = &list.items[0]
    else {
        panic!()
    };
    let unit = cell / 7.0;
    for (i, c) in text.chars().enumerate() {
        let x0 = (origin[0] + i as f32 * 6.0 * unit) * s;
        let x1 = x0 + 5.0 * unit * s;
        let (y0, y1) = (origin[1] * s, (origin[1] + cell) * s);
        let n = (y0 as u32..y1 as u32)
            .flat_map(|y| (x0 as u32..x1 as u32).map(move |x| (x, y)))
            .filter(|&(x, y)| luma(img.pixel(x, y)) > 128.0)
            .count();
        assert!(n > 10, "glyph {c:?} has ink ({n})");
    }
    snapshot("lcd_score_640x360", &img);
    // A scene with every layer kind, for the visual checklist.
    let lib = SceneLibrary::placeholders();
    for id in ["scene_powerball_mania", "scene_attract", "scene_jackpot"] {
        let scene = lib.get(lib.id_of(id).unwrap()).unwrap();
        let list = LcdDrawList::compose(&spec, scene, 450, model.scores(), &ImageSet::new());
        let img = render_lcd_to_image(ctx, spec, &list, &ImageSet::new(), w, h).unwrap();
        save(&format!("lcd_{id}"), &img);
    }
}

// ---------- routing ----------

#[test]
fn display_set_draws_into_pip_viewports_only() {
    let Some(ctx) = ctx("display_set_draws_into_pip_viewports_only") else {
        return;
    };
    let displays = [DisplayInfo {
        index: 0,
        bounds: Rect::new(0, 0, 640, 360),
        refresh_hz: 60.0,
        content_scale: 1.0,
    }];
    let plan = plan_topology(
        &displays,
        &TopologyMode::SingleMonitor {
            display: 0,
            backglass: DEFAULT_BACKGLASS_PIP,
            score: DEFAULT_SCORE_PIP,
        },
    )
    .unwrap();
    let routes = display_routes(&plan);
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let desc = BackglassDesc {
        layers: vec![BackglassLayer {
            image: solid(8, 6, [200, 30, 30, 255]),
            lamp: None,
        }],
    };
    let mut set = DisplaySet {
        backglass: Some(BackglassRenderer::new(ctx, format, &desc).unwrap()),
        score: Some(ScoreRenderer::new(
            ctx,
            format,
            ScoreDisplayConfig::Dmd(DMD_128X32),
        )),
    };
    let model = ScoreDisplayModel::new(
        ScoreDisplayConfig::Dmd(DMD_128X32),
        SceneLibrary::placeholders(),
    );
    let content = model.compose(0);
    let score = set.score.as_mut().unwrap();
    assert!(score.update(ctx.queue(), &content));
    assert!(!score.update(ctx.queue(), &content), "unchanged content");
    assert!(!score.update(
        ctx.queue(),
        &DisplayContent::Lcd(LcdDrawList {
            width: 1,
            height: 1,
            background: [0; 3],
            items: vec![]
        })
    ));
    let tex = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 640,
            height: 360,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    let mut enc = ctx.device().create_command_encoder(&Default::default());
    {
        let _p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 1.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    let n = set.encode_window(0, &routes, &mut enc, &view, (640, 360), &[]);
    assert_eq!(n, 2);
    assert_eq!(
        set.encode_window(1, &routes, &mut enc, &view, (640, 360), &[]),
        0
    );
    // Read back through a buffer.
    let row = 640 * 4;
    let buf = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * 360) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(360),
            },
        },
        tex.size(),
    );
    ctx.queue().submit([enc.finish()]);
    buf.map_async(wgpu::MapMode::Read, .., |_| {});
    pin_render::gpu::wait_for_device(ctx.device(), std::time::Duration::from_secs(60)).unwrap();
    let img = RgbaImage::from_rgba(640, 360, buf.get_mapped_range(..).unwrap().to_vec()).unwrap();
    let bg = routes[0].viewport;
    let sc = routes[1].viewport;
    for y in (0..360).step_by(3) {
        for x in (0..640).step_by(3) {
            let r = Rect::new(x as i32, y as i32, 1, 1);
            let p = img.pixel(x, y);
            if bg.contains(&r) {
                // Backglass art fills its 4:3 PiP.
                assert_eq!(&p[..3], &[200, 30, 30], "backglass at ({x},{y})");
            } else if !sc.contains(&r) {
                assert_eq!(&p[..3], &[0, 0, 255], "untouched at ({x},{y})");
            } else {
                assert_ne!(&p[..3], &[0, 0, 255], "score PiP painted at ({x},{y})");
            }
        }
    }
    save("routing_pip_640x360", &img);
}

// ---------- fix round 1 (B1, N1, N4, N7) ----------

#[test]
fn oversized_and_invalid_images_are_refused() {
    let Some(ctx) = ctx("oversized_and_invalid_images_are_refused") else {
        return;
    };
    let max = ctx.device().limits().max_texture_dimension_2d;
    let fmt = wgpu::TextureFormat::Rgba8Unorm;
    let bad = [
        RgbaImage::new(max + 1, 1),
        RgbaImage::new(1, max + 1),
        RgbaImage::new(0, 0),
        RgbaImage {
            width: 8,
            height: 8,
            pixels: vec![0; 10],
        },
    ];
    let mut lcd = LcdRenderer::new(ctx, fmt, LcdSpec::default());
    let base = lcd.texture_count();
    for img in &bad {
        let e = lcd.set_image(ctx, "a.png", img).unwrap_err();
        assert!(
            matches!(e, pin_render::gpu::GpuError::InvalidArgument(_)),
            "{e}"
        );
        let mut set = ImageSet::new();
        set.insert("a.png", img.clone());
        let list = LcdDrawList {
            width: 16,
            height: 9,
            background: [0; 3],
            items: vec![],
        };
        assert!(
            render_lcd_to_image(ctx, LcdSpec::new(16, 16).unwrap(), &list, &set, 16, 9).is_err()
        );
        for (image, mask) in [
            (img.clone(), solid(4, 4, [0; 4])),
            (solid(4, 4, [0; 4]), img.clone()),
        ] {
            let desc = BackglassDesc {
                layers: vec![BackglassLayer {
                    image,
                    lamp: Some(LayerLamp {
                        lamp: LampId(0),
                        mask,
                        blend: LayerBlend::Over,
                    }),
                }],
            };
            assert!(BackglassRenderer::new(ctx, fmt, &desc).is_err());
        }
    }
    assert_eq!(lcd.texture_count(), base, "nothing uploaded");
    // Setting the same file again replaces the texture in place.
    lcd.set_image(ctx, "a.png", &solid(2, 2, [255; 4])).unwrap();
    lcd.set_image(ctx, "a.png", &solid(3, 3, [0, 0, 255, 255]))
        .unwrap();
    lcd.set_image(ctx, "b.png", &solid(1, 1, [255; 4])).unwrap();
    assert_eq!(lcd.texture_count(), base + 2);
    // A refused replacement keeps the previous image.
    assert!(lcd.set_image(ctx, "a.png", &bad[0]).is_err());
    let spec = LcdSpec::new(16, 16).unwrap();
    let list = LcdDrawList {
        width: 16,
        height: 16,
        background: [0; 3],
        items: vec![LcdItem::Image {
            file: "a.png".into(),
            rect: [0.0, 0.0, 16.0, 16.0],
            color: [255; 4],
        }],
    };
    let mut r = LcdRenderer::new(ctx, fmt, spec);
    r.set_image(ctx, "a.png", &solid(2, 2, [255; 4])).unwrap();
    r.set_image(ctx, "a.png", &solid(3, 3, [0, 0, 255, 255]))
        .unwrap();
    r.set_draw_list(ctx.queue(), &list);
    let img = offscreen(ctx, 16, 16, |enc, view| {
        r.encode(enc, view, (16, 16), Rect::new(0, 0, 16, 16));
    });
    assert_eq!(&img.pixel(8, 8)[..3], &[0, 0, 255], "replacement drawn");
}

/// Renders with `draw` into a cleared `w × h` Rgba8Unorm texture and reads it
/// back (test helper).
fn offscreen(
    ctx: &GpuContext,
    w: u32,
    h: u32,
    draw: impl FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView),
) -> RgbaImage {
    let tex = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    let mut enc = ctx.device().create_command_encoder(&Default::default());
    draw(&mut enc, &view);
    let row = (w * 4).div_ceil(256) * 256;
    let buf = ctx.device().create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        tex.size(),
    );
    ctx.queue().submit([enc.finish()]);
    buf.map_async(wgpu::MapMode::Read, .., |_| {});
    pin_render::gpu::wait_for_device(ctx.device(), std::time::Duration::from_secs(60)).unwrap();
    let mapped = buf.get_mapped_range(..).unwrap();
    let mut px = Vec::new();
    for y in 0..h as usize {
        px.extend_from_slice(&mapped[y * row as usize..y * row as usize + (w * 4) as usize]);
    }
    RgbaImage::from_rgba(w, h, px).unwrap()
}

#[test]
fn mark_dirty_forces_a_redraw_report() {
    let Some(ctx) = ctx("mark_dirty_forces_a_redraw_report") else {
        return;
    };
    let fmt = wgpu::TextureFormat::Rgba8Unorm;
    let frame = DmdFrame::new(128, 32);
    let mut d = DmdRenderer::new(ctx, fmt, DMD_128X32);
    assert!(d.set_frame(ctx.queue(), &frame));
    assert!(!d.set_frame(ctx.queue(), &frame));
    d.mark_dirty();
    assert!(d.set_frame(ctx.queue(), &frame));
    assert!(!d.set_frame(ctx.queue(), &frame));
    let list = LcdDrawList {
        width: 16,
        height: 9,
        background: [0; 3],
        items: vec![],
    };
    let mut l = LcdRenderer::new(ctx, fmt, LcdSpec::new(16, 16).unwrap());
    assert!(l.set_draw_list(ctx.queue(), &list));
    assert!(!l.set_draw_list(ctx.queue(), &list));
    l.mark_dirty();
    assert!(l.set_draw_list(ctx.queue(), &list));
    l.set_image(ctx, "x.png", &solid(1, 1, [0; 4])).unwrap();
    assert!(
        l.set_draw_list(ctx.queue(), &list),
        "new image needs a redraw"
    );
    assert!(!l.set_draw_list(ctx.queue(), &list));
    let mut s = ScoreRenderer::new(ctx, fmt, ScoreDisplayConfig::Dmd(DMD_128X32));
    let c = DisplayContent::Dmd(frame);
    assert!(s.update(ctx.queue(), &c));
    assert!(!s.update(ctx.queue(), &c));
    s.mark_dirty();
    assert!(s.update(ctx.queue(), &c));
}
