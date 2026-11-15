//! Subphase 6.0 wiring (PR #21 review #12): the loaded table is drawn with
//! the 5.1 PBR renderer (through the 5.2 post chain) and the sim's display
//! scenes drive the 5.3 score display and backglass, rendered offscreen on
//! the test adapter (lavapipe in CI). Analytic pixel assertions only (D30):
//! region means, luminance and differences between frames.
//!
//! A required GPU target: it exists only under `--features gpu-tests` (the CI
//! GPU job) and without an adapter every test FAILS, it never skips or
//! returns early (PR #21 review). The GPU-free scene-image checks run in the
//! default build from `wiring_scene_images.rs`.

mod support;

use pin_app::orchestrator::{
    CopyPath, DisplayCommand, FrameCopy, TABLE_LINE_COLOR, TableRenderOptions, TableRenderer,
    TableVisuals, load_checked, offscreen_frame_texture, validate_table,
};
use pin_core::snapshot::{BallState, FrameStateSnapshot};
use pin_render::displays::{DisplayRoute, RgbaImage};
use pin_render::gpu::{GpuContext, wgpu};
use pin_render::topology::{Rect, SurfaceRole};
use pin_sim::SimConfig;
use std::sync::OnceLock;
use support::{greybox_bundle, on_big_stack};

const W: u32 = 200;
const H: u32 = 400;
/// Score display (128 × 32 DMD) PiP at the bottom, backglass PiP top-left.
const DMD: Rect = Rect {
    x: 10,
    y: 340,
    w: 180,
    h: 45,
};
const BACKGLASS: Rect = Rect {
    x: 10,
    y: 10,
    w: 60,
    h: 45,
};

/// The shared test adapter. Panics without one (set `PIN_REQUIRE_GPU=1` to
/// make a missing adapter fail inside `context_for_tests` itself).
fn gpu() -> &'static GpuContext {
    // One context per test binary (lavapipe and many devices, DECISIONS
    // "Wave 5 gate fix").
    static GPU: OnceLock<Option<GpuContext>> = OnceLock::new();
    GPU.get_or_init(|| pin_render::gpu::context_for_tests("wiring_render"))
        .as_ref()
        .expect(
            "wiring_render needs a GPU adapter (PIN_REQUIRE_GPU=1 and a working \
             Vulkan/Metal/DX12 adapter, e.g. lavapipe); it never skips",
        )
}

fn routes() -> Vec<DisplayRoute> {
    vec![
        DisplayRoute {
            role: SurfaceRole::Backglass,
            window: 0,
            viewport: BACKGLASS,
            dedicated: false,
        },
        DisplayRoute {
            role: SurfaceRole::Score,
            window: 0,
            viewport: DMD,
            dedicated: false,
        },
    ]
}

/// Mean RGBA (0..255) over a rectangle.
fn mean(img: &RgbaImage, r: Rect) -> [f32; 4] {
    let mut acc = [0.0f64; 4];
    let mut n = 0.0f64;
    for y in r.y as u32..r.y as u32 + r.h {
        for x in r.x as u32..r.x as u32 + r.w {
            let p = img.pixel(x, y);
            for k in 0..4 {
                acc[k] += f64::from(p[k]);
            }
            n += 1.0;
        }
    }
    acc.map(|v| (v / n) as f32)
}

fn luma(c: [f32; 4]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// Standard deviation of the luminance over a rectangle (a flat fill is 0).
fn luma_stddev(img: &RgbaImage, r: Rect) -> f32 {
    let mut v = Vec::new();
    for y in r.y as u32..r.y as u32 + r.h {
        for x in r.x as u32..r.x as u32 + r.w {
            let p = img.pixel(x, y);
            v.push(luma([
                f32::from(p[0]),
                f32::from(p[1]),
                f32::from(p[2]),
                0.0,
            ]));
        }
    }
    let m = v.iter().sum::<f32>() / v.len() as f32;
    (v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32).sqrt()
}

/// Mean absolute per-channel difference over a rectangle.
fn diff(a: &RgbaImage, b: &RgbaImage, r: Rect) -> f32 {
    let mut acc = 0.0f64;
    let mut n = 0.0f64;
    for y in r.y as u32..r.y as u32 + r.h {
        for x in r.x as u32..r.x as u32 + r.w {
            let (p, q) = (a.pixel(x, y), b.pixel(x, y));
            for k in 0..3 {
                acc += f64::from(p[k].abs_diff(q[k]));
            }
            n += 3.0;
        }
    }
    (acc / n) as f32
}

/// Loads the grey-box table (on a big stack) and returns its visuals, the
/// load-time snapshot and the scene ids the tests use.
fn table() -> (TableVisuals, FrameStateSnapshot, [pin_core::SceneId; 2]) {
    let dir = greybox_bundle().to_path_buf();
    on_big_stack(move || {
        let checked = validate_table(&dir).unwrap();
        let sim = load_checked(&checked, SimConfig::default()).unwrap();
        let ids = &checked.table.ids;
        let scenes = [
            ids.scene("scene_attract").unwrap(),
            ids.scene("scene_game_over").unwrap(),
        ];
        (TableVisuals::of(&checked), *sim.snapshot(), scenes)
    })
}

fn renderer(ctx: &GpuContext, v: &TableVisuals, overlay: bool) -> TableRenderer {
    TableRenderer::new(
        ctx,
        wgpu::TextureFormat::Rgba8Unorm,
        (W, H),
        v,
        TableRenderOptions {
            debug_overlay: overlay,
            ..TableRenderOptions::default()
        },
    )
    .unwrap()
}

#[test]
fn playfield_is_pbr_shaded_and_the_dmd_follows_the_scenes() {
    let ctx = gpu();
    let (v, snap, [attract, over]) = table();
    assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    assert!(!v.meshes.is_empty());
    let mut r = renderer(ctx, &v, false);
    assert_eq!(r.has_displays(), (true, true));
    r.apply_display_command(DisplayCommand::Scene(attract), 0);
    let mut s = snap;
    s.tick = 500;
    let img = r.render_to_image(ctx, &routes(), &s).unwrap();

    // Playfield: lit, not black, not the flat debug colour, and shaded
    // (light falls off away from the key lights: rows differ).
    let pf = Rect::new(40, 120, 120, 160);
    let m = mean(&img, pf);
    assert!(luma(m) > 20.0, "playfield too dark: {m:?}");
    let debug = TABLE_LINE_COLOR.map(|c| c * 255.0);
    let dist = (0..3).map(|k| (m[k] - debug[k]).abs()).sum::<f32>();
    assert!(dist > 20.0, "playfield looks like the debug colour: {m:?}");
    let sd = luma_stddev(&img, pf);
    assert!(sd > 1.0, "flat playfield (luminance std dev {sd})");

    // DMD: orange dots lit by the attract scene (red > blue), brighter than
    // the unlit grid.
    let dmd = mean(&img, DMD);
    assert!(dmd[0] > dmd[2] + 5.0, "DMD not orange: {dmd:?}");
    assert!(luma(dmd) > 5.0, "DMD dark: {dmd:?}");
    // Backglass: the translite art is drawn (not the playfield underneath).
    let bg_pf = renderer(ctx, &v, false)
        .render_to_image(ctx, &[], &s)
        .unwrap();
    assert!(
        diff(&img, &bg_pf, BACKGLASS) > 5.0,
        "backglass PiP not drawn"
    );
    assert!(diff(&img, &bg_pf, DMD) > 5.0, "DMD PiP not drawn");

    // A different scene changes the DMD and nothing else of the playfield.
    r.apply_display_command(DisplayCommand::Scene(over), 500);
    let img2 = r.render_to_image(ctx, &routes(), &s).unwrap();
    assert!(diff(&img, &img2, DMD) > 1.0, "DMD did not follow the scene");
    assert!(
        diff(&img, &img2, pf) < 0.5,
        "playfield changed with the scene"
    );
    assert_eq!(r.display_commands(), 2);
}

#[test]
fn a_ball_is_drawn_where_the_snapshot_puts_it() {
    let ctx = gpu();
    let (v, snap, _) = table();
    let mut r = renderer(ctx, &v, false);
    let without = r.render_to_image(ctx, &[], &snap).unwrap();
    let ((x0, y0), (x1, y1)) = (
        (v.bounds.0[0], v.bounds.0[1]),
        (v.bounds.1[0], v.bounds.1[1]),
    );
    let (cx, cy) = (0.5 * (x0 + x1), 0.5 * (y0 + y1));
    let mut s = snap;
    s.push_ball(BallState {
        position_m: [cx, cy, 0.0135],
        ..BallState::default()
    });
    let with = r.render_to_image(ctx, &[], &s).unwrap();
    // The camera looks straight down at the table centre: the ball is in
    // the middle of the image.
    let centre = Rect::new(W as i32 / 2 - 3, H as i32 / 2 - 3, 6, 6);
    assert!(
        diff(&with, &without, centre) > 5.0,
        "ball not visible at the centre"
    );
    let corner = Rect::new(150, 20, 30, 30);
    assert!(
        diff(&with, &without, corner) < 1.0,
        "ball changed a far corner"
    );
}

#[test]
fn the_debug_overlay_is_optional_and_off_by_default() {
    let ctx = gpu();
    let (v, snap, _) = table();
    assert!(!TableRenderOptions::default().debug_overlay);
    let plain = renderer(ctx, &v, false)
        .render_to_image(ctx, &[], &snap)
        .unwrap();
    let lines = renderer(ctx, &v, true)
        .render_to_image(ctx, &[], &snap)
        .unwrap();
    let all = Rect::new(0, 0, W, H);
    assert!(diff(&plain, &lines, all) > 0.05, "overlay drew nothing");
    // The overlay draws over the PBR image, it does not replace it.
    assert!(luma(mean(&lines, Rect::new(40, 120, 120, 160))) > 20.0);
}

/// The colour of the synthetic scene image (display-encoded RGB).
const SOLID: [u8; 3] = [40, 160, 220];
/// The LCD canvas of the synthetic table.
const LCD: (u32, u32) = (160, 90);

/// The grey-box bundle with an LCD score display whose attract scene is one
/// full-canvas `kind = "image"` layer over `display/solid.png` (written
/// only when `with_image`).
fn lcd_image_bundle(with_image: bool) -> std::path::PathBuf {
    let d = support::scratch(if with_image {
        "lcd_image"
    } else {
        "lcd_no_image"
    });
    support::build_greybox_bundle(&d);
    let bg = std::fs::read_to_string(d.join("backglass.toml")).unwrap();
    assert!(bg.contains("mode = \"dmd\""), "{bg}");
    let bg = bg.replace("mode = \"dmd\"", "mode = \"lcd\"").replace(
        "lcd = { width = 1920, height = 1080 }",
        &format!("lcd = {{ width = {}, height = {} }}", LCD.0, LCD.1),
    );
    std::fs::write(d.join("backglass.toml"), bg).unwrap();
    std::fs::write(
        d.join("display/scene_attract.toml"),
        "schema_version = 1\nbackground = \"#000000\"\n\n[[layers]]\nkind = \"image\"\n\
         file = \"display/solid.png\"\nx = 0.0\ny = 0.0\nw = 1.0\nh = 1.0\n",
    )
    .unwrap();
    if with_image {
        let img =
            image::RgbaImage::from_pixel(8, 8, image::Rgba([SOLID[0], SOLID[1], SOLID[2], 255]));
        img.save(d.join("display/solid.png")).unwrap();
    }
    d
}

fn lcd_table(with_image: bool) -> (TableVisuals, FrameStateSnapshot, pin_core::SceneId) {
    let dir = lcd_image_bundle(with_image);
    on_big_stack(move || {
        let checked = validate_table(&dir).unwrap();
        let sim = load_checked(&checked, SimConfig::default()).unwrap();
        let attract = checked.table.ids.scene("scene_attract").unwrap();
        (TableVisuals::of(&checked), *sim.snapshot(), attract)
    })
}

/// PR #21 review (view.rs:515): scene images are decoded from the bundle
/// snapshot and reach the LCD, so an image layer draws its pixels.
#[test]
fn lcd_scene_image_layers_draw_the_bundle_image() {
    let (v, snap, attract) = lcd_table(true);
    assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    let d = v.displays.as_ref().expect("displays");
    assert!(matches!(
        d.score,
        pin_render::displays::ScoreDisplayConfig::Lcd(_)
    ));
    assert_eq!(d.scene_images.len(), 1, "{:?}", d.scene_images.len());
    assert_eq!(d.scene_images[0].0, "display/solid.png");
    assert_eq!(
        d.scene_images[0].1.pixel(3, 3),
        [SOLID[0], SOLID[1], SOLID[2], 255]
    );

    let ctx = gpu();
    let mut r = renderer(ctx, &v, false);
    r.apply_display_command(DisplayCommand::Scene(attract), 0);
    let viewport = Rect::new(10, 300, 180, 90);
    let route = DisplayRoute {
        role: SurfaceRole::Score,
        window: 0,
        viewport,
        dedicated: false,
    };
    let mut s = snap;
    s.tick = 100;
    let img = r.render_to_image(ctx, &[route], &s).unwrap();
    // The LCD keeps its aspect inside the viewport; sample its inner part
    // (2 px away from the edges, filtering).
    let lcd = pin_render::displays::fit_aspect(viewport, LCD.0, LCD.1);
    let inner = Rect::new(lcd.x + 2, lcd.y + 2, lcd.w - 4, lcd.h - 4);
    let m = mean(&img, inner);
    for k in 0..3 {
        assert!(
            (m[k] - f32::from(SOLID[k])).abs() <= 3.0,
            "LCD region mean {m:?}, image {SOLID:?}"
        );
    }
}

/// A scene image missing from the bundle is a warning, not a panic; the
/// renderer still builds and draws (the layer is left out).
#[test]
fn a_missing_scene_image_is_a_warning() {
    let (v, snap, attract) = lcd_table(false);
    assert!(
        v.warnings
            .iter()
            .any(|w| w.starts_with("scene image display/solid.png: ")),
        "{:?}",
        v.warnings
    );
    assert!(v.displays.as_ref().unwrap().scene_images.is_empty());
    let ctx = gpu();
    let mut r = renderer(ctx, &v, false);
    r.apply_display_command(DisplayCommand::Scene(attract), 0);
    let viewport = Rect::new(10, 300, 180, 90);
    let route = DisplayRoute {
        role: SurfaceRole::Score,
        window: 0,
        viewport,
        dedicated: false,
    };
    let img = r.render_to_image(ctx, &[route], &snap).unwrap();
    // The black scene background, not the image colour.
    let lcd = pin_render::displays::fit_aspect(viewport, LCD.0, LCD.1);
    let m = mean(&img, Rect::new(lcd.x + 2, lcd.y + 2, lcd.w - 4, lcd.h - 4));
    assert!(luma(m) < 10.0, "{m:?}");
}

/// Raw texel bytes of an 8-bit texture (row 0 at the top, no swizzle).
fn read_raw(ctx: &GpuContext, tex: &wgpu::Texture) -> Vec<u8> {
    let device = ctx.device();
    let (w, h) = (tex.width(), tex.height());
    let row = w as usize * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize;
    let padded = row.div_ceil(align) * align;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("presenter test readback"),
        size: (padded * h as usize) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded as u32),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue().submit([enc.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    pin_render::gpu::wait_for_device(device, pin_render::gpu::READBACK_WAIT_TIMEOUT)
        .expect("device wait");
    rx.recv().expect("map callback").expect("map");
    let mut out = Vec::with_capacity(row * h as usize);
    {
        let mapped = buffer.get_mapped_range(..).expect("mapped range");
        for y in 0..h as usize {
            out.extend_from_slice(&mapped[y * padded..y * padded + row]);
        }
    }
    buffer.unmap();
    out
}

/// Clears `tex` to `c` (a render pass; the texture is a render target).
fn clear(ctx: &GpuContext, tex: &wgpu::Texture, c: wgpu::Color) {
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let mut enc = ctx
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("presenter test clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(c),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    ctx.queue().submit([enc.finish()]);
}

/// PR #21 round 4 (CodeRabbit thread 4577423193): a presenter-thread score
/// window's frame is drawn into an offscreen pool texture (the score display
/// driven by a display command), and the presenter's copy, along either
/// path (`COPY_DST` copy, or the blit when the surface lacks `COPY_DST`),
/// reproduces it texel for texel in a texture standing in for the
/// swap-chain image.
#[test]
fn presenter_copy_and_blit_reproduce_the_offscreen_frame() {
    let ctx = gpu();
    let (v, snap, [attract, _]) = table();
    let (w, h) = (192u32, 64u32);
    let window = 1;
    let route = DisplayRoute {
        role: SurfaceRole::Score,
        window,
        viewport: Rect::new(0, 0, w, h),
        dedicated: true,
    };
    let magenta = wgpu::Color {
        r: 1.0,
        g: 0.0,
        b: 1.0,
        a: 1.0,
    };
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb,
    ] {
        let mut r =
            TableRenderer::new(ctx, format, (w, h), &v, TableRenderOptions::default()).unwrap();
        r.apply_display_command(DisplayCommand::Scene(attract), 0);
        assert_eq!(r.display_commands(), 1);
        // The render thread's side: clear, then the window's displays.
        let src = offscreen_frame_texture(ctx.device(), format, (w, h));
        assert!(src.usage().contains(
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING
        ));
        clear(ctx, &src, wgpu::Color::BLACK);
        let view = src.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let mut s = snap;
        s.tick = 500;
        let drawn = r.encode(ctx.queue(), &mut enc, &view, false, window, &[route], &s);
        assert_eq!(drawn, 1, "{format:?}: the score display was not drawn");
        ctx.queue().submit([enc.finish()]);
        let src_px = read_raw(ctx, &src);
        // The frame has content: lit (non-black) texels, and no magenta.
        let lit = src_px
            .chunks_exact(4)
            .filter(|p| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) > 30)
            .count();
        assert!(
            lit > 50,
            "{format:?}: score frame is blank ({lit} lit texels)"
        );

        for path in [CopyPath::CopyDst, CopyPath::Blit] {
            let dst = ctx.device().create_texture(&wgpu::TextureDescriptor {
                label: Some("stand-in swap-chain image"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: path.target_usages() | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            clear(ctx, &dst, magenta);
            let copy = FrameCopy::new(ctx.device(), format, path);
            assert_eq!(copy.path(), path);
            let mut enc = ctx
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            copy.encode(ctx.device(), &mut enc, &src, &dst);
            ctx.queue().submit([enc.finish()]);
            let dst_px = read_raw(ctx, &dst);
            assert_eq!(dst_px.len(), src_px.len());
            let differing = src_px
                .chunks_exact(4)
                .zip(dst_px.chunks_exact(4))
                .filter(|(a, b)| a != b)
                .count();
            assert_eq!(
                differing, 0,
                "{format:?} {path:?}: {differing} texels differ from the offscreen frame"
            );
        }
    }
}
