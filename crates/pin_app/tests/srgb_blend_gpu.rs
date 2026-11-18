//! Issue #10 (part 2, `pin_app` wiring): on an `*Srgb` target the table
//! renderer's display layers blend in display-encoded space, exactly as on
//! the non-sRGB twin, along both of `pin_app`'s paths:
//!
//! * the render-thread path: a [`TableRenderer`] built for the `*Srgb`
//!   format with its displays in the twin
//!   ([`TableRenderer::with_layer_format`]) draws them through a
//!   `LayerCompositor` ([`TableRenderer::encode_composited`], here inside
//!   [`TableRenderer::render_to_image`]);
//! * the presenter-thread path: the frame is drawn into an offscreen texture
//!   of the twin format and the presenter's copy or blit
//!   ([`FrameCopy::with_source`]) moves its bytes into the `*Srgb` image
//!   unchanged (no second sRGB encode).
//!
//! The case is a half-transparent white backglass layer (alpha 128) over an
//! opaque mid-grey (128) layer, filling the target. Analytic assertions only
//! (D30): the display-encoded alpha-over is `128 + 128 · (1 − 128/255) =
//! 191.75` per channel; a linear-space blend stores about 154.
//!
//! GPU-only: compiled with `--features gpu-tests` (the CI GPU job, lavapipe),
//! where a missing adapter is a failure, never a skip.
#![cfg(feature = "gpu-tests")]

use pin_app::orchestrator::{
    CopyPath, DisplayAssets, FrameCopy, TableRenderOptions, TableRenderer, TableVisuals,
    offscreen_frame_texture,
};
use pin_core::snapshot::FrameStateSnapshot;
use pin_render::displays::{
    BackglassDesc, BackglassLayer, DMD_128X32, DisplayRoute, RgbaImage, SceneLibrary,
    ScoreDisplayConfig,
};
use pin_render::gpu::{GpuContext, wgpu};
use pin_render::topology::{Rect, SurfaceRole};
use std::sync::OnceLock;
use wgpu::TextureFormat as F;

const W: u32 = 64;
const H: u32 = 32;
const GREY: u8 = 128;
const ALPHA: u8 = 128;
/// 8-bit rounding of the premultiplied upload and of the blend.
const TOLERANCE: f64 = 1.0;
/// `(sRGB target, its non-sRGB twin)`.
const PAIRS: [(F, F); 2] = [
    (F::Rgba8UnormSrgb, F::Rgba8Unorm),
    (F::Bgra8UnormSrgb, F::Bgra8Unorm),
];

/// One context per test binary (lavapipe and many devices, DECISIONS
/// "Wave 5 gate fix"). Panics without an adapter.
fn gpu() -> &'static GpuContext {
    static GPU: OnceLock<Option<GpuContext>> = OnceLock::new();
    GPU.get_or_init(|| pin_render::gpu::context_for_tests("srgb_blend_gpu"))
        .as_ref()
        .expect(
            "srgb_blend_gpu needs a GPU adapter (PIN_REQUIRE_GPU=1 and a working \
             Vulkan/Metal/DX12 adapter, e.g. lavapipe); it never skips",
        )
}

fn solid(rgba: [u8; 4]) -> RgbaImage {
    RgbaImage::from_rgba(W, H, rgba.repeat((W * H) as usize)).expect("solid image")
}

/// No meshes; a backglass of an opaque grey layer under a half-transparent
/// white one, the art the size of the target (no letterbox margin).
fn visuals() -> TableVisuals {
    TableVisuals {
        meshes: Vec::new(),
        flippers: Vec::new(),
        bounds: ([0.0; 3], [0.52, 1.07, 0.0]),
        displays: Some(DisplayAssets {
            score: ScoreDisplayConfig::Dmd(DMD_128X32),
            scenes: SceneLibrary::new(),
            scene_images: Vec::new(),
            backglass: Some(BackglassDesc {
                layers: vec![
                    BackglassLayer {
                        image: solid([GREY, GREY, GREY, 255]),
                        lamp: None,
                    },
                    BackglassLayer {
                        image: solid([255, 255, 255, ALPHA]),
                        lamp: None,
                    },
                ],
            }),
        }),
        debug_lines: Vec::new(),
        warnings: Vec::new(),
    }
}

fn route() -> DisplayRoute {
    DisplayRoute {
        role: SurfaceRole::Backglass,
        window: 0,
        viewport: Rect::new(0, 0, W, H),
        dedicated: true,
    }
}

/// The display-encoded alpha-over of the case, per colour channel.
fn expected() -> f64 {
    let premul = f64::from((255 * u32::from(ALPHA) + 127) / 255);
    premul + f64::from(GREY) * (1.0 - f64::from(ALPHA) / 255.0)
}

/// Mean R, G, B over RGBA bytes.
fn mean_rgb(rgba: &[u8]) -> [f64; 3] {
    let mut sum = [0.0; 3];
    for p in rgba.chunks_exact(4) {
        for k in 0..3 {
            sum[k] += f64::from(p[k]);
        }
    }
    let n = (rgba.len() / 4) as f64;
    sum.map(|s| s / n)
}

fn assert_display_encoded(label: &str, rgba: &[u8]) {
    let got = mean_rgb(rgba);
    let want = expected();
    for (k, g) in got.iter().enumerate() {
        assert!(
            (g - want).abs() <= TOLERANCE,
            "[{label}] channel {k}: mean {g:.2}, display-encoded blend {want:.2} ({got:?})"
        );
    }
}

fn max_diff(a: &[u8], b: &[u8]) -> u8 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0)
}

/// The raw stored bytes of `tex` (8-bit, `W * 4` a multiple of 256), as
/// RGBA.
fn read_rgba(ctx: &GpuContext, tex: &wgpu::Texture) -> Vec<u8> {
    let (w, h) = (tex.width(), tex.height());
    assert_eq!((w * 4) % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT, 0);
    let device = ctx.device();
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("srgb blend readback"),
        size: u64::from(w * h * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
        },
        tex.size(),
    );
    ctx.queue().submit([enc.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buf.map_async(wgpu::MapMode::Read, .., move |r| {
        let _ = tx.send(r);
    });
    pin_render::gpu::wait_for_device(device, pin_render::gpu::READBACK_WAIT_TIMEOUT)
        .expect("device wait");
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .expect("map callback")
        .expect("map");
    let mut bytes = buf.get_mapped_range(..).expect("mapped").to_vec();
    buf.unmap();
    if tex.format().remove_srgb_suffix() == F::Bgra8Unorm {
        for p in bytes.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
    }
    bytes
}

fn renderer(format: F, layer_format: F) -> TableRenderer {
    TableRenderer::with_layer_format(
        gpu(),
        format,
        layer_format,
        (W, H),
        &visuals(),
        TableRenderOptions::default(),
    )
    .expect("table renderer")
}

/// The render-thread path on an `*Srgb` target: display layers built for
/// the twin and composited through `LayerCompositor` store the same bytes
/// as on the twin target itself, the display-encoded blend; the pre-fix
/// renderer (displays built for the `*Srgb` format, drawn through its
/// view) blends in linear space, so the case discriminates.
#[test]
fn srgb_target_displays_blend_in_display_encoded_space() {
    let ctx = gpu();
    let snap = FrameStateSnapshot::default();
    for (srgb, twin) in PAIRS {
        let mut fixed = renderer(srgb, twin);
        assert_eq!((fixed.format(), fixed.layer_format()), (srgb, twin));
        let fixed = fixed
            .render_to_image(ctx, &[route()], &snap)
            .expect("render");
        assert_display_encoded(&format!("{srgb:?} via {twin:?} layers"), &fixed.pixels);
        let plain = renderer(twin, twin)
            .render_to_image(ctx, &[route()], &snap)
            .expect("render");
        assert_display_encoded(&format!("{twin:?}"), &plain.pixels);
        let d = max_diff(&fixed.pixels, &plain.pixels);
        assert!(d <= 1, "{srgb:?}: {d} LSB from the non-sRGB twin");
        let pre_fix =
            TableRenderer::new(ctx, srgb, (W, H), &visuals(), TableRenderOptions::default())
                .expect("table renderer")
                .render_to_image(ctx, &[route()], &snap)
                .expect("render");
        let off = mean_rgb(&pre_fix.pixels)
            .iter()
            .map(|g| (g - expected()).abs())
            .fold(0.0, f64::max);
        assert!(
            off >= 8.0,
            "{srgb:?}: the pre-fix linear blend is only {off:.2} LSB away; the case does \
             not discriminate"
        );
    }
}

/// The presenter-thread path on an `*Srgb` surface: the frame drawn into a
/// twin-format offscreen texture holds the display-encoded blend, and both
/// presenter copy paths reproduce its bytes in an `*Srgb` stand-in for the
/// swap-chain image (the blit decodes before the target's encode).
#[test]
fn presenter_frames_reach_an_srgb_image_without_a_second_encode() {
    let ctx = gpu();
    let device = ctx.device();
    let snap = FrameStateSnapshot::default();
    for (srgb, twin) in PAIRS {
        let mut r = renderer(twin, twin);
        let src = offscreen_frame_texture(device, twin, (W, H));
        let view = src.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let drawn = r.encode(ctx.queue(), &mut enc, &view, false, 0, &[route()], &snap);
        assert_eq!(drawn, 1, "{twin:?}: the backglass was not drawn");
        ctx.queue().submit([enc.finish()]);
        let src_px = read_rgba(ctx, &src);
        assert_display_encoded(&format!("{twin:?} offscreen"), &src_px);
        for path in [CopyPath::CopyDst, CopyPath::Blit] {
            let dst = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("stand-in sRGB swap-chain image"),
                size: src.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: srgb,
                usage: path.target_usages() | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let copy = FrameCopy::with_source(device, twin, srgb, path);
            assert_eq!(copy.path(), path);
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            copy.encode(device, &mut enc, &src, &dst);
            ctx.queue().submit([enc.finish()]);
            let dst_px = read_rgba(ctx, &dst);
            let d = max_diff(&src_px, &dst_px);
            assert!(
                d <= 1,
                "{srgb:?} {path:?}: stored bytes {d} LSB from the offscreen frame"
            );
            assert_display_encoded(&format!("{srgb:?} {path:?}"), &dst_px);
        }
    }
}
