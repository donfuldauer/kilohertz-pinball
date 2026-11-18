//! Issue #10 (fix-10-srgb-blend, `gpu-tests`): display layers composite in
//! display-encoded space on an `*Srgb` target (the fallback surface format)
//! exactly as on its non-sRGB twin.
//!
//! Each case draws pre-existing frame content (a clear to a pure colour,
//! whose channels are 0 or 1 so its stored bytes are the same on both
//! formats), then a backglass whose layers are an optional opaque base and
//! a half-transparent layer (alpha 128/255), through the production
//! [`LayerTarget`] / [`LayerCompositor`] path that `SurfaceSet` configures
//! surfaces for. The raw stored bytes of the texture are copied to a buffer
//! (no decode) and compared:
//!
//! * `*Srgb` target vs its non-sRGB twin: per channel within
//!   [`TWIN_TOLERANCE`] (1 LSB). Both paths store the same blend of the
//!   same 8-bit values in the same `*Unorm` arithmetic; 1 LSB allows a
//!   backend that rounds the blend result differently between the twin
//!   view and a plain texture. Linear-space blending differs by up to ~60
//!   LSB (red at 128/255 over blue: blue channel 187 instead of 127), far
//!   outside. (Over black the two agree: the pre-fix shader decodes its
//!   premultiplied output, and `encode(decode(s) + 0)` is `s`; so every
//!   case but the first has a non-black destination.)
//! * Analytic region means (D30): the display-encoded alpha-over result,
//!   within [`ANALYTIC_TOLERANCE`] (1 LSB, 8-bit rounding of the
//!   premultiplied upload and the blend).
//! * The pre-fix path (renderer built for the `*Srgb` format, drawn through
//!   the `*Srgb` view: `LayerTarget::Direct` of an `*Srgb` format) is shown
//!   to match the analytic linear-space blend and to differ from the twin by
//!   at least [`LINEAR_BLEND_MIN_DIFF`] in the red-over-blue case, so the
//!   comparison above discriminates.
#![cfg(feature = "gpu-tests")]

use pin_render::displays::{BackglassDesc, BackglassLayer, BackglassRenderer, RgbaImage};
use pin_render::gpu::{GpuContext, LayerCompositor, LayerTarget, context_for_tests, wgpu};
use pin_render::topology::Rect;
use std::sync::OnceLock;
use wgpu::TextureFormat as F;

/// One device per test process (lavapipe crashes with a device per test
/// thread; DECISIONS "1.4 — CI fix: lavapipe SIGSEGV and macOS occlusion").
fn ctx(name: &str) -> Option<&'static GpuContext> {
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    CTX.get_or_init(|| context_for_tests(name)).as_ref()
}

/// `*Srgb` target vs non-sRGB twin, per stored channel.
const TWIN_TOLERANCE: u8 = 1;
/// Region mean vs the analytic display-encoded blend, per channel (LSB).
const ANALYTIC_TOLERANCE: f64 = 1.0;
/// The linear-space (pre-fix) blend differs from the display-encoded one by
/// at least this much in some channel of the red-over-blue case (analytic
/// gap 60: blue 187 vs 127).
const LINEAR_BLEND_MIN_DIFF: u8 = 40;
/// Region mean of the pre-fix path vs the analytic linear-space blend, per
/// channel (LSB): the shader's decode and the blend unit's encode each
/// round, and hardware sRGB conversion need not be exact.
const LINEAR_ANALYTIC_TOLERANCE: f64 = 2.0;

/// Half transparency: 128/255.
const HALF: u8 = 128;

/// One composition: frame content under the layers, then the layers.
#[derive(Copy, Clone, Debug)]
struct Case {
    name: &'static str,
    /// The frame's clear colour (channels 0 or 1 only).
    clear: [f64; 3],
    /// An opaque base layer, straight RGB.
    base: Option<[u8; 3]>,
    /// The half-transparent layer, straight RGBA.
    layer: [u8; 4],
}

const CASES: [Case; 4] = [
    Case {
        name: "white over black frame",
        clear: [0.0, 0.0, 0.0],
        base: None,
        layer: [255, 255, 255, HALF],
    },
    Case {
        name: "red over blue frame",
        clear: [0.0, 0.0, 1.0],
        base: None,
        layer: [255, 0, 0, HALF],
    },
    Case {
        name: "white over grey base layer",
        clear: [1.0, 0.0, 1.0],
        base: Some([64, 64, 64]),
        layer: [255, 255, 255, HALF],
    },
    Case {
        name: "green over orange base layer",
        clear: [0.0, 0.0, 0.0],
        base: Some([230, 120, 20]),
        layer: [40, 200, 90, HALF],
    },
];

/// Premultiplied 8-bit value as uploaded (`RgbaImage::premultiplied`).
fn premul(c: u8, a: u8) -> f64 {
    f64::from((u32::from(c) * u32::from(a) + 127) / 255)
}

/// The display-encoded alpha-over result, RGBA in 0..=255 (unrounded).
fn expected(case: &Case) -> [f64; 4] {
    let mut dst = case.clear.map(|c| c * 255.0);
    if let Some(b) = case.base {
        dst = b.map(f64::from);
    }
    let a = f64::from(case.layer[3]) / 255.0;
    let mut out = [0.0, 0.0, 0.0, 255.0];
    for k in 0..3 {
        out[k] = premul(case.layer[k], case.layer[3]) + dst[k] * (1.0 - a);
    }
    out
}

fn solid(w: u32, h: u32, rgba: [u8; 4]) -> RgbaImage {
    RgbaImage::from_rgba(w, h, rgba.repeat((w * h) as usize)).expect("solid image")
}

fn desc(case: &Case, w: u32, h: u32) -> BackglassDesc {
    let mut layers = Vec::new();
    if let Some(b) = case.base {
        layers.push(BackglassLayer {
            image: solid(w, h, [b[0], b[1], b[2], 255]),
            lamp: None,
        });
    }
    layers.push(BackglassLayer {
        image: solid(w, h, case.layer),
        lamp: None,
    });
    BackglassDesc { layers }
}

/// Renders `case` onto a fresh `w × h` texture of `target`'s format through
/// `comp` (built for `target`) and returns the raw stored bytes, in the
/// texture's channel order.
fn render(
    ctx: &GpuContext,
    comp: &mut LayerCompositor,
    case: &Case,
    (w, h): (u32, u32),
) -> Vec<u8> {
    assert_eq!((w * 4) % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT, 0);
    let device = ctx.device();
    let target = comp.target();
    let view_formats = target.view_formats();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("srgb blend target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: target.target_format(),
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | target.extra_usages(),
        view_formats: &view_formats,
    });
    let own = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let d = desc(case, w, h);
    let mut bg = BackglassRenderer::new(ctx, target.layer_format(), &d).expect("backglass");
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let [r, g, b] = case.clear;
        let _clear = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("frame content"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &own,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    let layer_view = comp.begin(device, &mut enc, &texture);
    bg.encode_layers(
        &mut enc,
        layer_view.as_ref().unwrap_or(&own),
        (w, h),
        Rect::new(0, 0, w, h),
        &[],
        0..bg.layer_count(),
        false,
    );
    comp.end(&mut enc, &texture);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("srgb blend readback"),
        size: u64::from(w * h * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
        },
        texture.size(),
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
    let bytes = buf.get_mapped_range(..).expect("mapped").to_vec();
    buf.unmap();
    bytes
}

/// RGBA order of stored `px` of `format`.
fn rgba(format: F, px: &[u8]) -> [u8; 4] {
    match format.remove_srgb_suffix() {
        F::Bgra8Unorm => [px[2], px[1], px[0], px[3]],
        _ => [px[0], px[1], px[2], px[3]],
    }
}

/// Mean RGBA over the whole image (every pixel is covered by the layers).
fn mean(format: F, bytes: &[u8]) -> [f64; 4] {
    let mut sum = [0.0; 4];
    for px in bytes.chunks_exact(4) {
        let c = rgba(format, px);
        for k in 0..4 {
            sum[k] += f64::from(c[k]);
        }
    }
    let n = (bytes.len() / 4) as f64;
    sum.map(|s| s / n)
}

fn assert_analytic(label: &str, format: F, bytes: &[u8], case: &Case) {
    let got = mean(format, bytes);
    let want = expected(case);
    for k in 0..4 {
        assert!(
            (got[k] - want[k]).abs() <= ANALYTIC_TOLERANCE,
            "[{label}] {}: mean {got:?}, display-encoded blend {want:?}",
            case.name
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

/// The `*Srgb` targets of the twin pairs, with the layer targets to run:
/// the one `LayerTarget::for_target` picks for a surface of this device
/// (what `SurfaceSet` configures), plus both fallback branches forced so
/// each is covered on every backend (the twin view only where the device
/// allows texture view formats).
fn srgb_targets(ctx: &GpuContext, srgb: F) -> Vec<(&'static str, LayerTarget)> {
    let flags = ctx.adapter().get_downlevel_capabilities().flags;
    let twin = srgb.remove_srgb_suffix();
    let usages = wgpu::TextureUsages::RENDER_ATTACHMENT
        | wgpu::TextureUsages::COPY_SRC
        | wgpu::TextureUsages::COPY_DST;
    let picked = LayerTarget::for_target(srgb, flags, usages);
    assert_ne!(
        picked,
        LayerTarget::Direct(srgb),
        "a target with copy usages never takes the linear-blend path"
    );
    let mut out = vec![
        ("for_target", picked),
        (
            "intermediate",
            LayerTarget::Intermediate { target: srgb, twin },
        ),
    ];
    if flags.contains(wgpu::DownlevelFlags::VIEW_FORMATS) {
        out.push(("twin view", LayerTarget::TwinView { target: srgb, twin }));
    }
    out
}

#[test]
fn srgb_target_layers_match_the_non_srgb_twin() {
    let Some(ctx) = ctx("srgb_target_layers_match_the_non_srgb_twin") else {
        return;
    };
    let size = (64, 16);
    for (srgb, plain) in [
        (F::Rgba8UnormSrgb, F::Rgba8Unorm),
        (F::Bgra8UnormSrgb, F::Bgra8Unorm),
    ] {
        assert_eq!(
            LayerTarget::for_target(
                plain,
                wgpu::DownlevelFlags::all(),
                wgpu::TextureUsages::all()
            ),
            LayerTarget::Direct(plain)
        );
        for case in &CASES {
            let reference = render(
                ctx,
                &mut LayerCompositor::new(LayerTarget::Direct(plain)),
                case,
                size,
            );
            assert_analytic(&format!("{plain:?}"), plain, &reference, case);
            for (label, target) in srgb_targets(ctx, srgb) {
                let got = render(ctx, &mut LayerCompositor::new(target), case, size);
                let d = max_diff(&got, &reference);
                assert!(
                    d <= TWIN_TOLERANCE,
                    "[{srgb:?} {label} {target:?}] {}: stored bytes differ from {plain:?} by {d} LSB \
                     (mean {:?} vs {:?})",
                    case.name,
                    mean(srgb, &got),
                    mean(plain, &reference)
                );
                assert_analytic(&format!("{srgb:?} {label}"), srgb, &got, case);
            }
        }
    }
}

fn srgb_decode(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb_encode(l: f64) -> f64 {
    if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

/// The pre-fix result, RGBA in 0..=255: the shader decodes the
/// premultiplied colour, the blend unit decodes the destination, blends in
/// linear space and encodes.
fn expected_linear(case: &Case) -> [f64; 4] {
    let mut dst = case.clear.map(|c| c * 255.0);
    if let Some(b) = case.base {
        // An opaque layer stores its own bytes on either path.
        dst = b.map(f64::from);
    }
    let a = f64::from(case.layer[3]) / 255.0;
    let mut out = [0.0, 0.0, 0.0, 255.0];
    for k in 0..3 {
        let s = srgb_decode(premul(case.layer[k], case.layer[3]) / 255.0);
        let d = srgb_decode(dst[k] / 255.0);
        out[k] = srgb_encode(s + d * (1.0 - a)) * 255.0;
    }
    out
}

/// The pre-fix path (renderers built for the `*Srgb` format drawing
/// through the `*Srgb` view) blends in linear space: it matches the
/// analytic linear blend and is far off the twin's bytes wherever the
/// destination is not black, so
/// [`srgb_target_layers_match_the_non_srgb_twin`] fails on it.
#[test]
fn linear_blend_on_the_srgb_view_is_detected() {
    let Some(ctx) = ctx("linear_blend_on_the_srgb_view_is_detected") else {
        return;
    };
    let size = (64, 16);
    for (srgb, plain) in [
        (F::Rgba8UnormSrgb, F::Rgba8Unorm),
        (F::Bgra8UnormSrgb, F::Bgra8Unorm),
    ] {
        let mut widest = 0;
        for case in &CASES {
            let reference = render(
                ctx,
                &mut LayerCompositor::new(LayerTarget::Direct(plain)),
                case,
                size,
            );
            let linear = render(
                ctx,
                &mut LayerCompositor::new(LayerTarget::Direct(srgb)),
                case,
                size,
            );
            let m = mean(srgb, &linear);
            let want = expected_linear(case);
            for k in 0..4 {
                assert!(
                    (m[k] - want[k]).abs() <= LINEAR_ANALYTIC_TOLERANCE,
                    "[{srgb:?}] {}: pre-fix mean {m:?}, analytic linear blend {want:?}",
                    case.name
                );
            }
            let encoded = expected(case);
            let gap = (0..3)
                .map(|k| (want[k] - encoded[k]).abs())
                .fold(0.0, f64::max);
            let d = max_diff(&linear, &reference);
            assert!(
                f64::from(d) + 2.0 * LINEAR_ANALYTIC_TOLERANCE >= gap,
                "[{srgb:?}] {}: pre-fix differs by {d} LSB, analytic gap {gap:.1}",
                case.name
            );
            widest = widest.max(d);
        }
        assert!(
            widest >= LINEAR_BLEND_MIN_DIFF,
            "[{srgb:?}] the linear blend differs by at most {widest} LSB"
        );
    }
}

/// One compositor across frames of different sizes (the scratch texture of
/// the intermediate path is recreated on a size change and reused
/// otherwise) keeps matching the twin.
#[test]
fn compositor_is_reused_across_frames_and_sizes() {
    let Some(ctx) = ctx("compositor_is_reused_across_frames_and_sizes") else {
        return;
    };
    let case = &CASES[1];
    for (label, target) in srgb_targets(ctx, F::Rgba8UnormSrgb) {
        let mut comp = LayerCompositor::new(target);
        for size in [(64, 16), (64, 16), (128, 8), (64, 16)] {
            let reference = render(
                ctx,
                &mut LayerCompositor::new(LayerTarget::Direct(F::Rgba8Unorm)),
                case,
                size,
            );
            let got = render(ctx, &mut comp, case, size);
            let d = max_diff(&got, &reference);
            assert!(d <= TWIN_TOLERANCE, "[{label} {size:?}] differs by {d} LSB");
            assert_analytic(label, F::Rgba8UnormSrgb, &got, case);
        }
    }
}
