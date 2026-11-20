use super::*;
use wgpu::PresentMode as P;

const ALL_MODES: [P; 6] = [
    P::Fifo,
    P::FifoRelaxed,
    P::Mailbox,
    P::Immediate,
    P::AutoVsync,
    P::AutoNoVsync,
];

/// Independent statement of the §2.2 table.
fn expected(role: SurfaceRole, offered: &[P], vrr: bool) -> PresentChoice {
    let offered: Vec<P> = if offered.is_empty() {
        vec![P::Fifo]
    } else {
        offered.to_vec()
    };
    match role {
        SurfaceRole::Playfield if vrr && offered.contains(&P::FifoRelaxed) => {
            PresentChoice::Direct(P::FifoRelaxed)
        }
        SurfaceRole::Playfield => PresentChoice::Direct(P::Fifo),
        _ if offered.contains(&P::Mailbox) => PresentChoice::Direct(P::Mailbox),
        _ if offered.contains(&P::Immediate) => PresentChoice::Direct(P::Immediate),
        _ => PresentChoice::PresenterThread(P::Fifo),
    }
}

#[test]
fn present_mode_table_every_subset() {
    let roles = [
        SurfaceRole::Playfield,
        SurfaceRole::Backglass,
        SurfaceRole::Score,
    ];
    let mut checked = 0;
    for mask in 0u32..(1 << ALL_MODES.len()) {
        let offered: Vec<P> = ALL_MODES
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, m)| *m)
            .collect();
        // Order must not matter either.
        let mut reversed = offered.clone();
        reversed.reverse();
        for role in roles {
            for vrr in [false, true] {
                let want = expected(role, &offered, vrr);
                assert_eq!(
                    choose_present_mode(role, &offered, vrr),
                    want,
                    "{role:?} {offered:?} vrr={vrr}"
                );
                assert_eq!(choose_present_mode(role, &reversed, vrr), want);
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 64 * 3 * 2);
}

#[test]
fn present_mode_spot_checks() {
    use SurfaceRole::*;
    assert_eq!(
        choose_present_mode(Playfield, &[], true),
        PresentChoice::Direct(P::Fifo)
    );
    assert_eq!(
        choose_present_mode(Playfield, &[P::Fifo, P::FifoRelaxed], true),
        PresentChoice::Direct(P::FifoRelaxed)
    );
    assert_eq!(
        choose_present_mode(Playfield, &[P::Fifo, P::FifoRelaxed], false),
        PresentChoice::Direct(P::Fifo)
    );
    // macOS CI: [Fifo, Immediate].
    assert_eq!(
        choose_present_mode(Backglass, &[P::Fifo, P::Immediate], false),
        PresentChoice::Direct(P::Immediate)
    );
    assert_eq!(
        choose_present_mode(Score, &[P::Fifo], false),
        PresentChoice::PresenterThread(P::Fifo)
    );
    assert_eq!(
        choose_present_mode(Score, &[], false),
        PresentChoice::PresenterThread(P::Fifo)
    );
    let c = PresentChoice::PresenterThread(P::Fifo);
    assert!(c.needs_presenter_thread());
    assert_eq!(c.mode(), P::Fifo);
    let d = PresentChoice::Direct(P::Mailbox);
    assert!(!d.needs_presenter_thread());
    assert_eq!(d.mode(), P::Mailbox);
}

#[test]
fn parse_backend_names() {
    assert_eq!(parse_backends("vulkan"), Some(Backends::VULKAN));
    assert_eq!(parse_backends("DX12"), Some(Backends::DX12));
    assert_eq!(parse_backends(" metal "), Some(Backends::METAL));
    assert_eq!(parse_backends("gl"), Some(Backends::GL));
    assert_eq!(
        parse_backends("vk,gles"),
        Some(Backends::VULKAN | Backends::GL)
    );
    assert_eq!(
        parse_backends("d3d12,mtl,opengl"),
        Some(Backends::DX12 | Backends::METAL | Backends::GL)
    );
    assert_eq!(parse_backends("bogus"), None);
    assert_eq!(parse_backends(""), None);
    assert_eq!(parse_backends("bogus,vulkan"), Some(Backends::VULKAN));
}

fn vars<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |k| {
        pairs
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.to_string())
    }
}

#[test]
fn options_from_vars() {
    let none = GpuOptions::from_vars_on(false, vars(&[]));
    assert_eq!(none, GpuOptions::default());
    assert!(!none.force_fallback_adapter);

    let ci_vk =
        GpuOptions::from_vars_on(false, vars(&[("WGPU_BACKEND", "vulkan"), ("CI", "true")]));
    assert_eq!(ci_vk.backends, Backends::VULKAN);
    assert!(ci_vk.force_fallback_adapter);

    let req_dx = GpuOptions::from_vars_on(
        false,
        vars(&[("WGPU_BACKEND", "dx12"), ("PIN_REQUIRE_GPU", "1")]),
    );
    assert_eq!(req_dx.backends, Backends::DX12);
    assert!(req_dx.force_fallback_adapter);

    let ci_metal = GpuOptions::from_vars_on(false, vars(&[("WGPU_BACKEND", "metal"), ("CI", "1")]));
    assert_eq!(ci_metal.backends, Backends::METAL);
    assert!(
        !ci_metal.force_fallback_adapter,
        "Metal has no fallback adapter"
    );

    let forced_off = GpuOptions::from_vars_on(
        false,
        vars(&[
            ("CI", "true"),
            (FORCE_FALLBACK_ENV, "0"),
            ("WGPU_BACKEND", "vulkan"),
        ]),
    );
    assert!(!forced_off.force_fallback_adapter);
    let forced_on = GpuOptions::from_vars_on(false, vars(&[(FORCE_FALLBACK_ENV, "1")]));
    assert!(forced_on.force_fallback_adapter);

    let bad = GpuOptions::from_vars_on(false, vars(&[("WGPU_BACKEND", "nope"), ("CI", "false")]));
    assert_eq!(bad.backends, GpuOptions::default().backends);
    assert!(!bad.force_fallback_adapter);
    // macOS never forces the fallback adapter from CI variables alone.
    let mac = GpuOptions::from_vars_on(true, vars(&[("CI", "true")]));
    assert!(!mac.force_fallback_adapter);
    assert_eq!(mac.backends, GpuOptions::default().backends);
    let mac_vk = GpuOptions::from_vars_on(true, vars(&[("CI", "1"), ("WGPU_BACKEND", "vulkan")]));
    assert!(!mac_vk.force_fallback_adapter);
    let mac_forced = GpuOptions::from_vars_on(true, vars(&[(FORCE_FALLBACK_ENV, "1")]));
    assert!(mac_forced.force_fallback_adapter);
    assert_eq!(
        GpuOptions::from_vars(vars(&[("CI", "true")])).force_fallback_adapter,
        !cfg!(target_os = "macos")
    );

    // The application ignores CI variables.
    let app = GpuOptions::app_from_vars(vars(&[
        ("CI", "true"),
        ("PIN_REQUIRE_GPU", "1"),
        ("WGPU_BACKEND", "dx12"),
    ]));
    assert_eq!(
        app,
        GpuOptions {
            force_fallback_adapter: false,
            backends: Backends::DX12
        }
    );
    let app_forced = GpuOptions::app_from_vars(vars(&[(FORCE_FALLBACK_ENV, "1")]));
    assert!(app_forced.force_fallback_adapter);
    let _ = GpuOptions::for_app();

    // from_env reads the real environment without panicking.
    let _ = GpuOptions::from_env();
    let _ = require_gpu();
}

#[test]
fn error_display() {
    let cases = [
        (GpuError::NoAdapter("x".into()), "no GPU adapter: x"),
        (
            GpuError::RequestDevice("x".into()),
            "GPU device request failed: x",
        ),
        (GpuError::Surface("x".into()), "GPU surface error: x"),
        (GpuError::Readback("x".into()), "GPU readback failed: x"),
        (GpuError::InvalidArgument("x".into()), "invalid argument: x"),
    ];
    for (e, s) in cases {
        assert_eq!(e.to_string(), s);
        let _: &dyn std::error::Error = &e;
    }
}

#[test]
fn role_colors_differ() {
    let p = role_clear_color(SurfaceRole::Playfield);
    let b = role_clear_color(SurfaceRole::Backglass);
    let s = role_clear_color(SurfaceRole::Score);
    assert_eq!(p, [0.0, 0.0, 0.0, 1.0]);
    assert_ne!(b, s);
    assert_ne!(p, b);
}

proptest::proptest! {
    #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]
    /// Arbitrary (possibly duplicated) mode lists agree with the table.
    #[test]
    fn present_mode_matches_table(idx in proptest::collection::vec(0usize..6, 0..10), role in 0usize..3, vrr: bool) {
        let offered: Vec<P> = idx.iter().map(|i| ALL_MODES[*i]).collect();
        let role = [SurfaceRole::Playfield, SurfaceRole::Backglass, SurfaceRole::Score][role];
        proptest::prop_assert_eq!(choose_present_mode(role, &offered, vrr), expected(role, &offered, vrr));
    }

    /// WGPU_BACKEND parsing never panics on arbitrary input.
    #[test]
    fn parse_backends_total(s in ".{0,40}") {
        let _ = parse_backends(&s);
        let _ = GpuOptions::from_vars(|k| (k == "WGPU_BACKEND").then(|| s.clone()));
    }
}

#[test]
fn pacing_requires_a_presented_playfield() {
    use SurfaceRole::*;
    use SurfaceStatus as S;
    let roles = [Playfield, Score];
    assert!(paced_by(roles, &[S::Presented, S::Unchanged]));
    // A playfield that could not present does not pace: the loop must sleep.
    for st in [S::Skipped, S::Occluded, S::Hidden, S::Busy, S::Unchanged] {
        assert!(!paced_by(roles, &[st, S::Presented]), "{st:?}");
    }
    assert!(!paced_by([Backglass, Score], &[S::Presented, S::Presented]));
    assert!(!paced_by([], &[]));
    assert!(!paced_by([Playfield], &[]));
}

/// N5 / D30: the presenter-thread blit copies its source exactly (analytic
/// readback, no surface needed).
#[cfg(feature = "gpu-tests")]
#[test]
fn blit_copies_source_pixels_exactly() {
    let Some(ctx) = crate::gpu::shared_context_for_tests("blit") else {
        return;
    };
    let device = ctx.device();
    let (w, h) = (7u32, 5u32);
    // A pattern with distinct values per pixel and channel.
    let mut src_px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            src_px.extend_from_slice(&[
                (x * 30) as u8,
                (y * 50) as u8,
                (x * 7 + y * 11) as u8,
                255,
            ]);
        }
    }
    let size = wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    };
    let src = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("blit src"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    ctx.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &src,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &src_px,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        size,
    );
    let src_view = src.create_view(&wgpu::TextureViewDescriptor::default());
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Bgra8Unorm,
    ] {
        let dst = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("blit dst"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let dst_view = dst.create_view(&wgpu::TextureViewDescriptor::default());
        let b = blit::Blit::new(device, format);
        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        b.encode(device, &mut enc, &src_view, &dst_view);
        let got = crate::debug::read_texture(ctx, enc, &dst).expect("readback");
        let want: Vec<u8> = if format == wgpu::TextureFormat::Bgra8Unorm {
            src_px
                .chunks(4)
                .flat_map(|p| [p[2], p[1], p[0], p[3]])
                .collect()
        } else {
            src_px.clone()
        };
        assert_eq!(got, want, "{format:?}");
    }
}

/// 5.1 review N6: waiting for a map result is bounded and reports a
/// callback that never runs or is dropped.
#[test]
fn map_result_wait_is_bounded() {
    use std::time::{Duration, Instant};
    let (tx, rx) = std::sync::mpsc::channel();
    let start = Instant::now();
    let e = super::recv_map(&rx, Duration::from_millis(20)).unwrap_err();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(matches!(e, GpuError::Readback(m) if m.contains("did not run")));
    tx.send(Ok(())).unwrap();
    assert_eq!(super::recv_map(&rx, Duration::from_millis(20)), Ok(()));
    drop(tx);
    let e = super::recv_map(&rx, Duration::from_millis(20)).unwrap_err();
    assert!(matches!(e, GpuError::Readback(m) if m.contains("dropped")));
}

/// A logical resize re-anchors the PiP rectangles (they used to keep their
/// planned position and end up outside a smaller window).
#[test]
fn logical_resize_rescales_pip_viewports() {
    use crate::topology::{DEFAULT_BACKGLASS_PIP, DEFAULT_SCORE_PIP, Rect, pip_rect};
    let plan = |w, h| {
        [
            (SurfaceRole::Playfield, Rect::new(0, 0, w, h)),
            (
                SurfaceRole::Backglass,
                pip_rect(SurfaceRole::Backglass, &DEFAULT_BACKGLASS_PIP, w, h).unwrap(),
            ),
            (
                SurfaceRole::Score,
                pip_rect(SurfaceRole::Score, &DEFAULT_SCORE_PIP, w, h).unwrap(),
            ),
        ]
    };
    let planned = plan(1920, 1080);
    assert_eq!(planned[1].1.x, 1440);
    // Same result as planning the new size, in either direction.
    for to in [(1280, 720), (1920, 1080), (3840, 2160), (800, 600)] {
        let got = surfaces::rescale_viewports(&planned, (1920, 1080), to);
        assert_eq!(got.as_slice(), &plan(to.0, to.1), "{to:?}");
        for (_, r) in &got {
            assert!(Rect::new(0, 0, to.0, to.1).contains(r), "{to:?} {r:?}");
        }
    }
    // A window too flat for a PiP's aspect: shrunk to fit, still anchored.
    let got = surfaces::rescale_viewports(&planned, (1920, 1080), (1600, 100));
    assert_eq!(got[1].1, Rect::new(1600 - 133, 0, 133, 100));
    assert!(Rect::new(0, 0, 1600, 100).contains(&got[2].1));
    assert_eq!(got[2].1.bottom(), 100);
    // Zero sizes leave the plan alone (no division by zero).
    for (from, to) in [((0, 0), (10, 10)), ((1920, 1080), (0, 5))] {
        assert_eq!(
            surfaces::rescale_viewports(&planned, from, to).as_slice(),
            &planned
        );
    }
}

/// A readback larger than `max_buffer_size` is an error, not a buffer
/// creation that fails validation (8192² `Rgba16Float` needs 512 MiB; the
/// downlevel limit is 256 MiB).
#[test]
fn oversized_readback_is_an_error() {
    let limit = wgpu::Limits::downlevel_defaults().max_buffer_size;
    assert_eq!(
        super::checked_readback_size(limit, 128 * 8, 128),
        Ok(131_072)
    );
    assert_eq!(super::checked_readback_size(1024, 256, 4), Ok(1024));
    let e = super::checked_readback_size(limit, 8192 * 8, 8192).unwrap_err();
    assert!(matches!(e, GpuError::Readback(m) if m.contains("max_buffer_size")));
    assert!(super::checked_readback_size(1024, 256, 5).is_err());
    assert!(super::checked_readback_size(u64::MAX, usize::MAX, u32::MAX).is_err());
}

/// w8g-d: the PiP outline helper draws one closed 4-segment loop per PiP
/// viewport (the window's own viewport, the first, gets none), scaled from
/// logical to physical pixels on pixel centres.
#[test]
fn pip_outline_is_one_loop_per_pip() {
    use crate::debug::DebugLines;
    use crate::topology::Rect;
    let mut out = DebugLines::new(64);
    let vps = [
        (SurfaceRole::Playfield, Rect::new(0, 0, 100, 200)),
        (SurfaceRole::Backglass, Rect::new(10, 20, 30, 40)),
        (SurfaceRole::Score, Rect::new(50, 60, 20, 10)),
    ];
    surfaces::pip_outline_lines(&vps, (100, 200), (200, 400), &mut out);
    assert_eq!(out.len(), 8);
    let l = out.as_slice();
    // First PiP at 2x: x 20.5..79.5, y 40.5..119.5, closed.
    assert_eq!(l[0].a[..2], [20.5, 40.5]);
    assert_eq!(l[0].b[..2], [79.5, 40.5]);
    assert_eq!(l[2].b[..2], [20.5, 119.5]);
    assert_eq!(l[3].b, l[0].a);
    // The window's own viewport alone: no outline; the buffer is cleared.
    surfaces::pip_outline_lines(&vps[..1], (100, 200), (200, 400), &mut out);
    assert!(out.is_empty());
    // A zero logical size scales by 1 (no division by zero).
    surfaces::pip_outline_lines(&vps[..2], (0, 0), (200, 400), &mut out);
    assert_eq!(out.as_slice()[0].a[..2], [10.5, 20.5]);
}

/// The default hooks pass a frame command buffer through alone and do
/// nothing else (`SurfaceSet::render` is `render_with(NoFrameHooks)`).
#[cfg(feature = "gpu-tests")]
#[test]
fn no_frame_hooks_pass_one_buffer_through() {
    let Some(ctx) = crate::gpu::shared_context_for_tests("no_frame_hooks") else {
        return;
    };
    let mut h = NoFrameHooks;
    h.begin_frame();
    let cb = ctx
        .device()
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None })
        .finish();
    let cbs = h.wrap_submit(cb);
    assert_eq!(cbs.len(), 1);
    let mut submitted = 0;
    h.end_frame(&mut |_cb| submitted += 1);
    assert_eq!(submitted, 0, "the default end_frame submits nothing");
    ctx.queue().submit(cbs);
}

#[cfg(feature = "gpu-tests")]
mod debug_frame_gpu {
    use super::*;
    use crate::debug::{Camera, DebugLine, DebugLines, DebugRenderer, box_lines};
    use crate::topology::Rect;

    const SIZE: (u32, u32) = (96, 64);
    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    /// The frame drawing of `SurfaceSet::render_slot` before w8g-d (its
    /// `draw` closure, verbatim but for the slot fields passed in): the
    /// reference the hooked drawing must reproduce byte for byte.
    #[allow(clippy::too_many_arguments)]
    fn pre_change_draw(
        role: SurfaceRole,
        lines_r: &mut DebugRenderer,
        overlay_r: &mut DebugRenderer,
        encoder: &mut wgpu::CommandEncoder,
        queue: &wgpu::Queue,
        view: &wgpu::TextureView,
        camera: &Camera,
        lines: &DebugLines,
        overlay_lines: &DebugLines,
        (pw, ph): (u32, u32),
    ) {
        let pixel_camera = Camera::ortho_top_down([0.0, ph as f32], [pw as f32, 0.0]);
        let clear = role_clear_color(role);
        if role == SurfaceRole::Playfield {
            lines_r.draw_with(encoder, queue, view, camera, lines, Some(clear), None);
        } else {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("secondary clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(clear[0]),
                            g: f64::from(clear[1]),
                            b: f64::from(clear[2]),
                            a: f64::from(clear[3]),
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
        if !overlay_lines.is_empty() {
            overlay_r.draw(encoder, queue, view, &pixel_camera, overlay_lines);
        }
    }

    fn target(ctx: &GpuContext) -> wgpu::Texture {
        ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("debug frame"),
            size: wgpu::Extent3d {
                width: SIZE.0,
                height: SIZE.1,
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

    fn scene() -> (DebugLines, Camera, DebugLines) {
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
            [1.0, 0.2, 0.1, 1.0],
        ));
        let camera = Camera::ortho_top_down([0.0, 0.0], [1.0, 1.0]);
        let mut outline = DebugLines::new(64);
        surfaces::pip_outline_lines(
            &[
                (SurfaceRole::Playfield, Rect::new(0, 0, 48, 32)),
                (SurfaceRole::Score, Rect::new(4, 4, 20, 8)),
            ],
            (48, 32),
            SIZE,
            &mut outline,
        );
        assert!(!outline.is_empty());
        (lines, camera, outline)
    }

    /// Records a frame through the pre-change closure (`hooks: None`) or
    /// `encode_debug_frame` with `hooks`, submits it (through
    /// `wrap_submit` for the hooked path, as `render_slot` does) and reads
    /// it back.
    fn frame(
        ctx: &GpuContext,
        role: SurfaceRole,
        with_outline: bool,
        hooks: Option<&mut dyn SurfaceFrameHooks>,
    ) -> Vec<u8> {
        let (lines, camera, outline) = scene();
        let empty = DebugLines::new(1);
        let outline = if with_outline { &outline } else { &empty };
        let mut lines_r = DebugRenderer::new(ctx.device(), FORMAT);
        let mut overlay_r = DebugRenderer::new(ctx.device(), FORMAT);
        let tex = target(ctx);
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("surface frame"),
            });
        match hooks {
            None => pre_change_draw(
                role,
                &mut lines_r,
                &mut overlay_r,
                &mut enc,
                ctx.queue(),
                &view,
                &camera,
                &lines,
                outline,
                SIZE,
            ),
            Some(h) => {
                encode_debug_frame(
                    ctx.queue(),
                    &mut enc,
                    &view,
                    DebugFrame {
                        role,
                        size_px: SIZE,
                        lines: &lines,
                        camera: &camera,
                        outline,
                    },
                    &mut lines_r,
                    &mut overlay_r,
                    h,
                );
                let cbs = h.wrap_submit(enc.finish());
                assert_eq!(cbs.len(), 1);
                ctx.queue().submit(cbs);
                enc = ctx
                    .device()
                    .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            }
        }
        crate::debug::read_texture(ctx, enc, &tex).expect("readback")
    }

    /// A hook that clears the whole target to magenta in its overlay and
    /// counts its calls.
    #[derive(Default)]
    struct Paint {
        overlays: Vec<SurfaceRole>,
        sizes: Vec<(u32, u32)>,
    }

    impl SurfaceFrameHooks for Paint {
        fn overlay(
            &mut self,
            role: SurfaceRole,
            _queue: &wgpu::Queue,
            encoder: &mut wgpu::CommandEncoder,
            view: &wgpu::TextureView,
            size_px: (u32, u32),
        ) {
            self.overlays.push(role);
            self.sizes.push(size_px);
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("test overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 1.0,
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
    }

    /// Byte identity: with no hook (`NoFrameHooks`) the factored frame
    /// drawing is the pre-change closure's, for a playfield and a
    /// secondary surface, with and without PiP outlines.
    #[test]
    fn hooked_debug_frame_without_hooks_matches_the_pre_change_drawing() {
        let Some(ctx) = crate::gpu::shared_context_for_tests("debug_frame_identity") else {
            return;
        };
        for role in [
            SurfaceRole::Playfield,
            SurfaceRole::Backglass,
            SurfaceRole::Score,
        ] {
            for with_outline in [false, true] {
                let before = frame(ctx, role, with_outline, None);
                let after = frame(ctx, role, with_outline, Some(&mut NoFrameHooks));
                assert!(
                    before == after,
                    "{role:?} outline {with_outline}: the hooked frame differs"
                );
                // Something was drawn (not an all-zero readback).
                assert!(before.chunks_exact(4).any(|p| p[3] == 255), "{role:?}");
            }
        }
        // The lines and outline show on the playfield (not just a clear).
        let pf = frame(ctx, SurfaceRole::Playfield, true, None);
        assert!(pf.chunks_exact(4).any(|p| p[1] > 200), "no green line");
        assert!(
            pf.chunks_exact(4)
                .any(|p| p[0] > 100 && p[0] < 160 && p[0] == p[1] && p[1] == p[2]),
            "no grey PiP outline"
        );
    }

    /// The overlay hook runs last (after the lines and the PiP outline: a
    /// full clear in it leaves only its colour) and only for the playfield.
    #[test]
    fn overlay_hook_runs_last_and_only_on_the_playfield() {
        let Some(ctx) = crate::gpu::shared_context_for_tests("debug_frame_overlay") else {
            return;
        };
        let mut h = Paint::default();
        let pf = frame(ctx, SurfaceRole::Playfield, true, Some(&mut h));
        assert_eq!(h.overlays, [SurfaceRole::Playfield]);
        assert_eq!(h.sizes, [SIZE]);
        assert!(
            pf.chunks_exact(4).all(|p| p == [255, 0, 255, 255]),
            "something was drawn after the overlay"
        );
        for role in [SurfaceRole::Backglass, SurfaceRole::Score] {
            let mut h = Paint::default();
            let got = frame(ctx, role, true, Some(&mut h));
            assert!(h.overlays.is_empty(), "{role:?} got an overlay call");
            assert!(got == frame(ctx, role, true, None), "{role:?}");
        }
    }
}
