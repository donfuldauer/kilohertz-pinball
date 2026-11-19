//! w8f-q6, the GPU side of the profiler wiring (feature `gpu-tests`, the CI
//! GPU job on lavapipe, where a missing adapter is a failure):
//!
//! * The device is built the way the app builds it
//!   (`GpuContext::with_instance`, here through `new_headless`): it enables
//!   the adapter's timestamp features and nothing it lacks.
//! * (e) GPU frame time: frames bracketed by [`GpuFrameTimer`] on that
//!   device. With `TIMESTAMP_QUERY`, samples reach the profiler once the
//!   readback ring drains (a bounded poll loop, no sleeps), each above 0 ns
//!   and below 10 s; without it, no timer, no samples, no panic. The test
//!   prints which branch ran. Every submission of a timed frame is
//!   bracketed by its own start/end pair (3 command buffers); a frame that
//!   finds the ring full (a slow GPU) is untimed and its submissions go
//!   alone (1 buffer). Every timed frame ends as exactly one sample or one
//!   discard. A frame's submissions beyond `MAX_TIMED_SUBMITS` go untimed
//!   (checked deterministically, independent of GPU speed).
//! * (d) HUD: the overlay's text is `hud_lines` of the snapshot it was
//!   given; a HUD-enabled renderer with no text draws the same bytes as a
//!   HUD-less one (the default, pre-change frame); with text it changes
//!   pixels only inside its backdrop rectangle.
#![cfg(feature = "gpu-tests")]

use pin_app::orchestrator::{
    GPU_TIMER_RING, GpuFrameTimer, HudFeed, TableRenderOptions, TableRenderer, TableVisuals,
};
use pin_app::profiler::{Profiler, hud_lines};
use pin_core::snapshot::FrameStateSnapshot;
use pin_render::displays::RgbaImage;
use pin_render::gpu::{GpuContext, wgpu};
use std::sync::OnceLock;

/// One context per test binary (lavapipe and many devices, DECISIONS
/// "Wave 5 gate fix"). Panics without an adapter.
fn gpu() -> &'static GpuContext {
    static GPU: OnceLock<Option<GpuContext>> = OnceLock::new();
    GPU.get_or_init(|| pin_render::gpu::context_for_tests("profiler_gpu"))
        .as_ref()
        .expect(
            "profiler_gpu needs a GPU adapter (PIN_REQUIRE_GPU=1 and a working \
             Vulkan/Metal/DX12 adapter, e.g. lavapipe); it never skips",
        )
}

/// No meshes, no displays: the PBR environment through the post chain.
fn visuals() -> TableVisuals {
    TableVisuals {
        meshes: Vec::new(),
        flippers: Vec::new(),
        bounds: ([0.0; 3], [0.52, 1.07, 0.0]),
        displays: None,
        debug_lines: Vec::new(),
        warnings: Vec::new(),
    }
}

fn renderer(ctx: &GpuContext, size: (u32, u32), hud: bool) -> TableRenderer {
    TableRenderer::new(
        ctx,
        wgpu::TextureFormat::Rgba8Unorm,
        size,
        &visuals(),
        TableRenderOptions {
            profiler_hud: hud,
            ..TableRenderOptions::default()
        },
    )
    .expect("renderer")
}

fn timestamp_features() -> wgpu::Features {
    wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
}

#[test]
fn app_device_enables_the_adapters_timestamp_features() {
    let ctx = gpu();
    let offered = ctx.adapter().features() & timestamp_features();
    let enabled = ctx.device().features();
    println!(
        "[profiler_gpu] adapter {:?}: timestamp features offered {offered:?}, enabled {:?}",
        ctx.adapter_info().name,
        enabled & timestamp_features()
    );
    assert_eq!(enabled & timestamp_features(), offered);
}

#[test]
fn gpu_frame_time_comes_from_timestamp_queries_or_not_at_all() {
    let ctx = gpu();
    let p = Profiler::new();
    let supported = GpuFrameTimer::supported(ctx);
    let size = (64, 48);
    let mut r = renderer(ctx, size, false);
    let snap = FrameStateSnapshot::default();
    let target = ctx.device().create_texture(&wgpu::TextureDescriptor {
        label: Some("timed frame"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    // One frame's command buffer, as the render thread records it.
    let frame = |r: &mut TableRenderer| {
        let mut enc = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        r.encode(ctx.queue(), &mut enc, &view, true, 0, &[], &snap);
        enc.finish()
    };
    let mut submit = |cb: wgpu::CommandBuffer| {
        ctx.queue().submit([cb]);
    };
    match GpuFrameTimer::new(ctx, p.present) {
        Some(mut timer) => {
            println!(
                "[profiler_gpu] branch: TIMESTAMP_QUERY present (period {} ns); expecting samples",
                timer.period_ns()
            );
            assert!(supported);
            const FRAMES: usize = 2 * GPU_TIMER_RING;
            let mut timed_frames = 0u64;
            for k in 0..FRAMES {
                timer.collect();
                timer.begin_frame();
                // Untimed when the readback ring is still full (slow GPU);
                // the first frame always finds a free entry.
                let timed = timer.is_timing();
                assert!(timed || k > 0, "the first frame was not timed");
                timed_frames += u64::from(timed);
                // Odd frames are submitted in two parts (two surfaces).
                let parts = 1 + k % 2;
                for _ in 0..parts {
                    // Timed: start + frame + end, a pair per submission.
                    // Untimed: the frame alone.
                    let cbs = timer.bracket(frame(&mut r));
                    assert_eq!(cbs.len(), if timed { 3 } else { 1 }, "frame {k}");
                    ctx.queue().submit(cbs);
                }
                timer.end_frame(&mut submit);
                assert!(!timer.is_timing());
            }
            // Outside a frame, a command buffer goes alone.
            let cbs = timer.bracket(frame(&mut r));
            assert_eq!(cbs.len(), 1);
            ctx.queue().submit(cbs);
            // A frame that submitted nothing is not timed.
            timer.begin_frame();
            timer.end_frame(&mut submit);
            // Drain the ring: a bounded count of non-blocking polls.
            let mut polls = 0u64;
            while timer.in_flight() > 0 && polls < 50_000_000 {
                timer.collect();
                polls += 1;
                std::thread::yield_now();
            }
            assert_eq!(timer.in_flight(), 0, "readback ring never drained");
            let s = p.reader.snapshot();
            println!(
                "[profiler_gpu] {} samples recorded, {} discarded, after {polls} polls: {}",
                timer.recorded(),
                timer.discarded(),
                hud_lines(&s)[2]
            );
            assert_eq!(s.gpu_frame.count, timer.recorded());
            assert!(s.gpu_frame.count > 0, "no GPU frame sample recorded");
            assert!(
                timer.recorded() + timer.discarded() <= FRAMES as u64,
                "an untimed frame produced a sample"
            );
            assert!(timed_frames >= 1);
            assert_eq!(
                timer.recorded() + timer.discarded(),
                timed_frames,
                "every timed frame is one sample or one discard"
            );
            assert!(s.gpu_frame.min > 0, "a 0 ns sample was recorded");
            assert!(
                s.gpu_frame.max < 10_000_000_000,
                "a sample of {} ns",
                s.gpu_frame.max
            );
            assert!(!hud_lines(&s)[2].ends_with("n 0"));
        }
        None => {
            println!(
                "[profiler_gpu] branch: no TIMESTAMP_QUERY on this device; expecting no samples"
            );
            assert!(!supported);
            for _ in 0..3 {
                ctx.queue().submit([frame(&mut r)]);
            }
            let s = p.reader.snapshot();
            assert_eq!(s.gpu_frame.count, 0);
            assert!(hud_lines(&s)[2].ends_with("n 0"), "{}", hud_lines(&s)[2]);
        }
    }
}

/// A timed frame with more submissions than `MAX_TIMED_SUBMITS`: the first
/// `MAX_TIMED_SUBMITS` are bracketed (3 buffers each), the rest go alone
/// (1 buffer), and the frame still yields one sample or one discard.
/// Deterministic: a fresh timer's first frame always finds a free entry.
#[test]
fn submissions_beyond_the_cap_go_untimed() {
    let ctx = gpu();
    let p = Profiler::new();
    let Some(mut timer) = GpuFrameTimer::new(ctx, p.present) else {
        println!("[profiler_gpu] no TIMESTAMP_QUERY on this device; cap test has nothing to time");
        assert!(!GpuFrameTimer::supported(ctx));
        return;
    };
    let cap = GpuFrameTimer::MAX_TIMED_SUBMITS;
    assert!(cap >= 1);
    // An empty command buffer stands in for one surface's work.
    let empty = || {
        ctx.device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None })
            .finish()
    };
    timer.begin_frame();
    assert!(timer.is_timing(), "a fresh timer's first frame is timed");
    for j in 0..cap + 2 {
        let cbs = timer.bracket(empty());
        let want = if j < cap { 3 } else { 1 };
        assert_eq!(cbs.len(), want, "submission {j} of a timed frame");
        ctx.queue().submit(cbs);
    }
    assert!(timer.is_timing());
    let mut submit = |cb: wgpu::CommandBuffer| {
        ctx.queue().submit([cb]);
    };
    timer.end_frame(&mut submit);
    assert!(!timer.is_timing());
    // Outside a frame, a command buffer goes alone.
    assert_eq!(timer.bracket(empty()).len(), 1);
    let mut polls = 0u64;
    while timer.in_flight() > 0 && polls < 50_000_000 {
        timer.collect();
        polls += 1;
        std::thread::yield_now();
    }
    assert_eq!(timer.in_flight(), 0, "readback ring never drained");
    println!(
        "[profiler_gpu] cap {cap}: {} recorded, {} discarded after {polls} polls",
        timer.recorded(),
        timer.discarded()
    );
    assert_eq!(
        timer.recorded() + timer.discarded(),
        1,
        "one frame, one result"
    );
    assert_eq!(p.reader.snapshot().gpu_frame.count, timer.recorded());
}

/// The pixels of `img` outside `rect` (`[x0, y0, x1, y1]`), and inside.
fn split(img: &RgbaImage, rect: [u32; 4]) -> (Vec<u8>, Vec<u8>) {
    let (mut outside, mut inside) = (Vec::new(), Vec::new());
    for y in 0..img.height {
        for x in 0..img.width {
            let i = 4 * (y * img.width + x) as usize;
            let px = &img.pixels[i..i + 4];
            if x >= rect[0] && x < rect[2] && y >= rect[1] && y < rect[3] {
                inside.extend_from_slice(px);
            } else {
                outside.extend_from_slice(px);
            }
        }
    }
    (outside, inside)
}

#[test]
fn hud_draws_hud_lines_only_when_enabled() {
    let ctx = gpu();
    let size = (768, 160);
    let snap = FrameStateSnapshot::default();

    // A known profiler state, through the render thread's feed.
    let mut p = Profiler::new();
    p.sim.record_tick_ns(123_000);
    p.sim.record_input(0, 900_000);
    p.present.record_frame_ns(2_500_000);
    p.sim_loop.set_input_overflow_ticks(1);
    let mut feed = HudFeed::new(p.reader.clone(), 250_000_000);
    let lines = feed.refresh(0).expect("first refresh");
    assert_eq!(lines, hud_lines(&p.reader.snapshot()));

    // The default frame (no HUD built), the pre-change path.
    let plain = renderer(ctx, size, false)
        .render_to_image(ctx, &[], &snap)
        .expect("plain frame");
    assert!(renderer(ctx, size, false).hud().is_none());

    // HUD enabled, no text yet: the same bytes.
    let mut hud = renderer(ctx, size, true);
    let empty = hud
        .render_to_image(ctx, &[], &snap)
        .expect("empty HUD frame");
    assert!(
        empty.pixels == plain.pixels,
        "an empty HUD changed the frame"
    );

    // With text: the overlay shows exactly `hud_lines(&snapshot)` ...
    hud.set_hud_lines(&lines);
    let shown = hud.render_to_image(ctx, &[], &snap).expect("HUD frame");
    let overlay = hud.hud().expect("HUD built");
    assert_eq!(overlay.lines(), lines.as_slice());
    let rect = overlay.rect_px();
    println!("[profiler_gpu] HUD rect {rect:?} on {size:?}");
    assert!(
        rect[0] < rect[2] && rect[1] < rect[3],
        "empty HUD rect {rect:?}"
    );
    assert!(rect[3] < size.1, "4 lines fit in {} px: {rect:?}", size.1);
    // ... drawn inside its backdrop rectangle, nowhere else.
    let (out_plain, in_plain) = split(&plain, rect);
    let (out_shown, in_shown) = split(&shown, rect);
    assert!(out_shown == out_plain, "the HUD drew outside {rect:?}");
    let changed = in_plain
        .chunks_exact(4)
        .zip(in_shown.chunks_exact(4))
        .filter(|(a, b)| a != b)
        .count();
    // (The backdrop is near-black, so over a dark frame only the glyphs
    // change pixels.)
    assert!(changed > 0, "the HUD changed nothing inside {rect:?}");
    println!(
        "[profiler_gpu] HUD changed {changed} of {} pixels in its rect",
        in_plain.len() / 4
    );
    // Bright text pixels on the dark backdrop.
    assert!(
        in_shown
            .chunks_exact(4)
            .any(|px| px[0] > 150 && px[1] > 150 && px[2] > 120),
        "no text drawn"
    );

    // A renderer without the HUD ignores HUD text.
    let mut off = renderer(ctx, size, false);
    off.set_hud_lines(&lines);
    let still = off.render_to_image(ctx, &[], &snap).expect("frame");
    assert!(
        still.pixels == plain.pixels,
        "HUD text drawn without the flag"
    );
}
