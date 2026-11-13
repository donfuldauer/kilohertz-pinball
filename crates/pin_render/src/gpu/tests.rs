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
    let Some(ctx) = context_for_tests("blit") else {
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
        let got = crate::debug::read_texture(&ctx, enc, &dst).expect("readback");
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
