//! w8g-d: the debug view's profiler hooks offscreen (feature `gpu-tests`,
//! the CI GPU job on lavapipe, where a missing adapter is a failure).
//!
//! The debug view's frames are recorded by pin_render's
//! `encode_debug_frame` (what `SurfaceSet::render_slot` records for each
//! drawn surface) and submitted through the hooks' `wrap_submit`, as
//! `SurfaceSet::render_with` does; swap-chain images cannot be read back,
//! so the frames are drawn into an offscreen texture here.
//!
//! * Byte identity: (a) the pre-change drawing (the old `draw` closure:
//!   the lines over the playfield clear, then the PiP outline through a
//!   pixel camera), (b) the hooked drawing with [`DebugViewHooks`] holding
//!   nothing, (c) with an empty [`HudOverlay`] (no text: paints nothing) and
//!   (d) with a [`GpuFrameTimer`] (timestamps in their own command buffers)
//!   are byte-identical. With HUD text the pixels change only inside
//!   `HudOverlay::rect_px()`.
//! * GPU frame time: debug frames timed through the hooks reach the
//!   profiler as samples above 0 ns with `TIMESTAMP_QUERY` (bounded poll
//!   loop, no sleeps); without it there is no timer, no sample and no
//!   panic. The test prints which branch ran.
#![cfg(feature = "gpu-tests")]

use pin_app::orchestrator::{
    DebugViewHooks, GPU_TIMER_RING, GpuFrameTimer, HudFeed, HudOverlay, hud_format_supported,
};
use pin_app::profiler::{Profiler, hud_lines};
use pin_render::debug::{Camera, DebugLine, DebugLines, DebugRenderer, box_lines};
use pin_render::gpu::{
    DebugFrame, GpuContext, NoFrameHooks, SurfaceFrameHooks, encode_debug_frame, role_clear_color,
    wgpu,
};
use pin_render::topology::SurfaceRole;
use std::sync::OnceLock;

/// One context per test binary (lavapipe and many devices, DECISIONS
/// "Wave 5 gate fix"). Panics without an adapter.
fn gpu() -> &'static GpuContext {
    static GPU: OnceLock<Option<GpuContext>> = OnceLock::new();
    GPU.get_or_init(|| pin_render::gpu::context_for_tests("profiler_debug_view"))
        .as_ref()
        .expect(
            "profiler_debug_view needs a GPU adapter (PIN_REQUIRE_GPU=1 and a working \
             Vulkan/Metal/DX12 adapter, e.g. lavapipe); it never skips",
        )
}

/// Wide enough for the HUD's four lines; `W * 4` is a multiple of 256.
const W: u32 = 768;
const H: u32 = 160;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The debug view's content: a box, a diagonal, and one PiP outline (pixel
/// coordinates, as `SurfaceSet` builds it for a PiP viewport).
struct Scene {
    lines: DebugLines,
    camera: Camera,
    outline: DebugLines,
}

fn scene() -> Scene {
    let mut lines = DebugLines::new(64);
    box_lines(
        [0.1, 0.1, 0.0],
        [0.9, 0.9, 0.0],
        [0.0, 1.0, 0.0, 1.0],
        &mut lines,
    );
    lines.push(DebugLine::new(
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [1.0, 0.3, 0.1, 1.0],
    ));
    let mut outline = DebugLines::new(16);
    let (x0, y0, x1, y1) = (500.5, 100.5, 700.5, 150.5);
    let pts = [[x0, y0], [x1, y0], [x1, y1], [x0, y1], [x0, y0]];
    for w in pts.windows(2) {
        outline.push(DebugLine::new(
            [w[0][0], w[0][1], 0.0],
            [w[1][0], w[1][1], 0.0],
            [0.5, 0.5, 0.5, 1.0],
        ));
    }
    Scene {
        lines,
        camera: Camera::ortho_top_down([0.0, 0.0], [1.0, 1.0]),
        outline,
    }
}

fn target(ctx: &GpuContext) -> wgpu::Texture {
    ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("debug view frame"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

/// The bytes of `tex` (RGBA8, `W * 4` a multiple of 256).
fn read(ctx: &GpuContext, tex: &wgpu::Texture) -> Vec<u8> {
    let device = ctx.device();
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("debug view readback"),
        size: u64::from(W * H * 4),
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
                bytes_per_row: Some(W * 4),
                rows_per_image: Some(H),
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
    let bytes = buf.get_mapped_range(..).expect("mapped").to_vec();
    buf.unmap();
    bytes
}

/// Renderers of one surface slot (lines, PiP outline), as `SurfaceSet`
/// keeps them.
struct Slot {
    lines: DebugRenderer,
    outline: DebugRenderer,
}

fn slot(ctx: &GpuContext) -> Slot {
    Slot {
        lines: DebugRenderer::new(ctx.device(), FORMAT),
        outline: DebugRenderer::new(ctx.device(), FORMAT),
    }
}

/// (a) The pre-change playfield frame: the old `render_slot` `draw`
/// closure for a playfield, written against the public renderer API.
fn pre_change_frame(ctx: &GpuContext, s: &Scene) -> Vec<u8> {
    let mut slot = slot(ctx);
    let tex = target(ctx);
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let pixel_camera = Camera::ortho_top_down([0.0, H as f32], [W as f32, 0.0]);
    let mut enc = ctx
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("surface frame"),
        });
    let clear = role_clear_color(SurfaceRole::Playfield);
    slot.lines.draw_with(
        &mut enc,
        ctx.queue(),
        &view,
        &s.camera,
        &s.lines,
        Some(clear),
        None,
    );
    if !s.outline.is_empty() {
        slot.outline
            .draw(&mut enc, ctx.queue(), &view, &pixel_camera, &s.outline);
    }
    ctx.queue().submit([enc.finish()]);
    read(ctx, &tex)
}

/// Draws `frames` playfield frames of `s` through `hooks` the way
/// `SurfaceSet::render_with` does (`begin_frame`, the frame recorded by
/// `encode_debug_frame` with the overlay last, `wrap_submit` into one
/// submit, `end_frame`), into one target, and reads the last one back.
/// Returns the bytes and the buffer count of each submission.
fn hooked_frames(
    ctx: &GpuContext,
    s: &Scene,
    hooks: &mut dyn SurfaceFrameHooks,
    frames: usize,
) -> (Vec<u8>, Vec<usize>) {
    let mut slot = slot(ctx);
    let tex = target(ctx);
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let mut submits = Vec::new();
    let mut submit = |cb: wgpu::CommandBuffer| {
        ctx.queue().submit([cb]);
    };
    for _ in 0..frames {
        hooks.begin_frame();
        let mut enc = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("surface frame"),
            });
        encode_debug_frame(
            ctx.queue(),
            &mut enc,
            &view,
            DebugFrame {
                role: SurfaceRole::Playfield,
                size_px: (W, H),
                lines: &s.lines,
                camera: &s.camera,
                outline: &s.outline,
            },
            &mut slot.lines,
            &mut slot.outline,
            hooks,
        );
        let cbs = hooks.wrap_submit(enc.finish());
        submits.push(cbs.len());
        ctx.queue().submit(cbs);
        hooks.end_frame(&mut submit);
    }
    (read(ctx, &tex), submits)
}

/// The pixels of an RGBA8 `W`-wide image outside `rect` (`[x0, y0, x1,
/// y1]`), and inside.
fn split(px: &[u8], rect: [u32; 4]) -> (Vec<u8>, Vec<u8>) {
    let (mut outside, mut inside) = (Vec::new(), Vec::new());
    for y in 0..H {
        for x in 0..W {
            let i = 4 * (y * W + x) as usize;
            let p = &px[i..i + 4];
            if x >= rect[0] && x < rect[2] && y >= rect[1] && y < rect[3] {
                inside.extend_from_slice(p);
            } else {
                outside.extend_from_slice(p);
            }
        }
    }
    (outside, inside)
}

#[test]
fn debug_frames_are_byte_identical_without_hud_text_and_change_only_in_the_hud_rect() {
    let ctx = gpu();
    let s = scene();
    assert!(hud_format_supported(
        FORMAT,
        ctx.adapter().get_texture_format_features(FORMAT)
    ));

    // (a) the pre-change drawing.
    let before = pre_change_frame(ctx, &s);
    assert!(before.chunks_exact(4).any(|p| p[1] > 200), "no green line");
    assert!(
        before
            .chunks_exact(4)
            .any(|p| p[0] > 100 && p[0] < 160 && p[0] == p[1] && p[1] == p[2]),
        "no grey PiP outline"
    );

    // (b) hooked, with hooks that do nothing.
    let (none, n) = hooked_frames(ctx, &s, &mut NoFrameHooks, 1);
    assert_eq!(n, [1]);
    assert!(none == before, "NoFrameHooks changed the frame");
    let mut empty = DebugViewHooks::new(None, None);
    assert!(empty.is_empty());
    let (b, n) = hooked_frames(ctx, &s, &mut empty, 1);
    assert_eq!(n, [1], "no timer: the frame goes alone");
    assert!(b == before, "empty DebugViewHooks changed the frame");

    // (c) an empty HUD (no text) paints nothing.
    let mut hud = DebugViewHooks::new(None, Some(HudOverlay::new(ctx.device(), FORMAT)));
    let (c, n) = hooked_frames(ctx, &s, &mut hud, 1);
    assert_eq!(n, [1]);
    assert!(c == before, "an empty HUD changed the frame");
    assert_eq!(hud.hud_paints(), 0);

    // (d) the GPU timer brackets the frame in its own buffers: same bytes.
    let p = Profiler::new();
    if let Some(timer) = GpuFrameTimer::new(ctx, p.present) {
        let mut timed = DebugViewHooks::new(Some(timer), None);
        let (d, n) = hooked_frames(ctx, &s, &mut timed, 1);
        assert_eq!(n, [3], "a timed frame: start, frame, end");
        assert!(d == before, "the timer changed the frame");
    } else {
        println!("[profiler_debug_view] no TIMESTAMP_QUERY: timer identity case not run");
    }

    // With text: exactly `hud_lines(&snapshot)`, inside its rect only.
    let mut prof = Profiler::new();
    prof.sim.record_tick_ns(123_000);
    prof.sim.record_input(0, 900_000);
    prof.present.record_frame_ns(2_500_000);
    let mut feed = HudFeed::new(prof.reader.clone(), 250_000_000);
    let lines = feed.refresh(0).expect("first refresh");
    assert_eq!(lines, hud_lines(&prof.reader.snapshot()));
    hud.set_hud_lines(&lines);
    let (shown, _) = hooked_frames(ctx, &s, &mut hud, 1);
    assert_eq!(hud.hud_paints(), 1);
    let overlay = hud.hud().expect("HUD");
    assert_eq!(overlay.lines(), lines.as_slice());
    let rect = overlay.rect_px();
    println!("[profiler_debug_view] HUD rect {rect:?} on {W}x{H}");
    assert!(
        rect[0] < rect[2] && rect[1] < rect[3],
        "empty rect {rect:?}"
    );
    let (out_before, in_before) = split(&before, rect);
    let (out_shown, in_shown) = split(&shown, rect);
    assert!(out_shown == out_before, "the HUD drew outside {rect:?}");
    let changed = in_before
        .chunks_exact(4)
        .zip(in_shown.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    assert!(changed > 0, "the HUD changed nothing inside {rect:?}");
    assert!(
        in_shown
            .chunks_exact(4)
            .any(|px| px[0] > 150 && px[1] > 150 && px[2] > 120),
        "no text drawn"
    );
}

#[test]
fn debug_frames_record_gpu_frame_time_through_the_hooks() {
    let ctx = gpu();
    let s = scene();
    let p = Profiler::new();
    match GpuFrameTimer::new(ctx, p.present) {
        Some(timer) => {
            println!(
                "[profiler_debug_view] branch: TIMESTAMP_QUERY present (period {} ns); \
                 expecting samples",
                timer.period_ns()
            );
            let mut hooks = DebugViewHooks::new(Some(timer), None);
            let (_, n) = hooked_frames(ctx, &s, &mut hooks, 2 * GPU_TIMER_RING);
            // The first frame always finds a free ring entry.
            assert_eq!(n[0], 3);
            assert!(n.iter().all(|&k| k == 1 || k == 3), "{n:?}");
            // Drain: `begin_frame` collects; empty frames submit nothing
            // and are not timed. A bounded count of non-blocking polls.
            let mut polls = 0u64;
            while hooks.gpu_timer().expect("timer").in_flight() > 0 && polls < 50_000_000 {
                hooks.begin_frame();
                hooks.end_frame(&mut |cb| {
                    ctx.queue().submit([cb]);
                });
                polls += 1;
                std::thread::yield_now();
            }
            let t = hooks.gpu_timer().expect("timer");
            assert_eq!(t.in_flight(), 0, "readback ring never drained");
            let snap = p.reader.snapshot();
            println!(
                "[profiler_debug_view] {} samples, {} discarded after {polls} polls: {}",
                t.recorded(),
                t.discarded(),
                hud_lines(&snap)[2]
            );
            assert!(snap.gpu_frame.count > 0, "no GPU frame sample recorded");
            assert_eq!(snap.gpu_frame.count, t.recorded());
            assert!(snap.gpu_frame.min > 0, "a 0 ns sample was recorded");
            assert!(!hud_lines(&snap)[2].ends_with("n 0"));
        }
        None => {
            println!(
                "[profiler_debug_view] branch: no TIMESTAMP_QUERY on this device; expecting no \
                 samples"
            );
            assert!(!GpuFrameTimer::supported(ctx));
            let mut hooks = DebugViewHooks::new(None, None);
            let (_, n) = hooked_frames(ctx, &s, &mut hooks, 3);
            assert_eq!(n, [1, 1, 1]);
            let snap = p.reader.snapshot();
            assert_eq!(snap.gpu_frame.count, 0);
            assert!(hud_lines(&snap)[2].ends_with("n 0"));
        }
    }
}
