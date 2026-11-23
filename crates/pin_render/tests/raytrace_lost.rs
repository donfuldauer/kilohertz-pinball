//! 5.4 ray-query reflections on a lost device (gpu-tests; CodeRabbit PR
//! #22 finding 3).
//!
//! Its own test binary with its own context, because the test destroys the
//! device: no other test may share it. The only test in this binary (it also covers `mark_device_lost` on a
//! second source).
#![cfg(feature = "gpu-tests")]

use pin_render::gpu::{GpuContext, GpuError, GpuOptions, require_gpu, wgpu};
use pin_render::pbr::math::scale_translation;
use pin_render::pbr::{
    DrawItem, FrameDesc, MaterialKind, MaterialParams, MeshGeometry, PbrCamera, PbrRenderer,
    PointLight,
};
use pin_render::raytrace::{
    RAY_QUERY_FEATURE, RayCapability, RayQueryConfig, RayQueryReflections, ReflectionQuality,
};
use pin_render::reflections::{ReflectionSource, ReflectiveBall};

const R: f32 = 0.0135;

/// Which flag governs which failure:
///
/// - [`required`] (`PIN_REQUIRE_GPU=1` or `PIN_REQUIRE_RAY_QUERY=1`): a
///   context that fails to create is fatal instead of skipping.
/// - [`ray_query_missing_is_fatal`]: an adapter that lacks
///   `EXPERIMENTAL_RAY_QUERY` fails the test iff `PIN_REQUIRE_RAY_QUERY=1`
///   (any adapter) or `PIN_REQUIRE_GPU=1` on a Vulkan adapter. WARP and Metal
///   never have the feature, so there it skips with a message.
fn required() -> bool {
    require_gpu() || require_ray_query()
}

/// `PIN_REQUIRE_RAY_QUERY=1`: a missing ray-query feature fails the test.
fn require_ray_query() -> bool {
    std::env::var("PIN_REQUIRE_RAY_QUERY").is_ok_and(|v| v.trim() == "1")
}

/// The missing-feature rule: fatal iff `PIN_REQUIRE_RAY_QUERY=1`, or
/// `PIN_REQUIRE_GPU=1` on a Vulkan adapter (docs/HUMAN_QUEUE.md).
fn ray_query_missing_is_fatal(backend: wgpu::Backend) -> bool {
    require_ray_query() || (require_gpu() && backend == wgpu::Backend::Vulkan)
}

#[test]
fn raytrace_lost_device_reports_the_loss_and_stops_rendering() {
    let ctx =
        match GpuContext::new_headless_with_features(&GpuOptions::from_env(), RAY_QUERY_FEATURE) {
            Ok(ctx) => ctx,
            Err(e) if required() => panic!("[raytrace_lost] GPU required but {e}"),
            Err(e) => {
                println!("[raytrace_lost] SKIP: {e}");
                return;
            }
        };
    if !ctx.supports_ray_query() {
        let info = ctx.adapter_info();
        assert!(
            !ray_query_missing_is_fatal(info.backend),
            "[raytrace_lost] ray queries required but adapter {} ({:?}) lacks EXPERIMENTAL_RAY_QUERY",
            info.name,
            info.backend
        );
        println!(
            "[raytrace_lost] SKIP: adapter {} ({:?}) lacks EXPERIMENTAL_RAY_QUERY",
            info.name, info.backend
        );
        return;
    }

    let mut pbr = PbrRenderer::new(&ctx, 64, 64).unwrap();
    let sphere_geo = MeshGeometry::uv_sphere(R, 32, 24);
    let sphere = pbr.upload_mesh(&sphere_geo).unwrap();
    let make = || {
        RayQueryReflections::new(
            &ctx,
            &pbr,
            ReflectionQuality::BallsOnly,
            &[(sphere, &sphere_geo)],
            &RayQueryConfig::default(),
        )
        .unwrap()
    };
    // `forwarded` is built first: wgpu keeps one device-lost callback per
    // device, so the later `src` owns it and `forwarded` only learns of a
    // loss through `mark_device_lost`.
    let mut forwarded = make();
    let mut src = make();
    let ball = |x: f32| DrawItem {
        mesh: sphere,
        model: scale_translation(1.0, [x, 0.0, R]),
        material: MaterialParams {
            roughness: 0.06,
            ..MaterialKind::ChromeSteel.params()
        },
        transparent: false,
        emission: 0.0,
        double_sided: false,
    };
    let draws = [ball(-0.016), ball(0.016)];
    let balls = [
        ReflectiveBall {
            position: [-0.016, 0.0, R],
            radius: R,
            draw: 0,
        },
        ReflectiveBall {
            position: [0.016, 0.0, R],
            radius: R,
            draw: 1,
        },
    ];
    let cam = PbrCamera::look_at(
        [0.0, -0.085, 0.03],
        [0.0, 0.0, R],
        [0.0, 0.0, 1.0],
        35f32.to_radians(),
        1.0,
        0.005,
        5.0,
    );
    let lights = [PointLight::new([0.0, -0.05, 0.2], [1.0; 3], 0.1, 5.0)];
    let frame = FrameDesc::new(&cam, &lights, &draws);

    // `mark_device_lost` (an application forwarding a loss): the source
    // reports it and every frame returns the loss without touching the GPU
    // (the device is still alive here, yet nothing is traced).
    assert!(!forwarded.device_lost());
    forwarded.mark_device_lost("forwarded by the test");
    assert!(forwarded.device_lost());
    for i in 0..2 {
        match forwarded.prepare(&ctx, &mut pbr, &frame, &balls) {
            Err(GpuError::RequestDevice(m)) => {
                assert_eq!(m, "device lost: forwarded by the test", "frame {i}");
            }
            other => panic!("forwarded frame {i}: expected the device-lost error, got {other:?}"),
        }
        assert_eq!(forwarded.ray_stats().frames, 0);
        assert_eq!(forwarded.stats().frames, 0);
        assert!(forwarded.ray_stats().fallen_back);
    }
    forwarded.mark_device_lost("a later reason is ignored");
    assert!(
        forwarded
            .fallback_reason()
            .is_some_and(|r| r == "device lost: forwarded by the test"),
        "{:?}",
        forwarded.fallback_reason()
    );
    assert!(!src.device_lost(), "the other source is unaffected");

    // One healthy traced frame.
    src.prepare(&ctx, &mut pbr, &frame, &balls).unwrap();
    assert_eq!(src.name(), "ray_query");
    assert!(!src.device_lost());
    assert!(!src.probes_bound());
    let traced_frames = src.ray_stats().frames;
    assert_eq!(traced_frames, 1);

    // Destroy the device. wgpu 30 calls the device-lost callback
    // (reason Destroyed) from the next device maintenance once the queue is
    // empty: poll (bounded) until the source has seen it.
    ctx.device().destroy();
    let mut polls = 0;
    while !src.device_lost() && polls < 8 {
        let _ = ctx.device().poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(60)),
        });
        polls += 1;
    }
    println!("[raytrace_lost] device-lost callback seen after {polls} poll(s)");
    assert!(src.device_lost(), "device-lost callback not delivered");

    // Every further frame reports the loss, never touches the GPU, never
    // panics, and the source reports the probes.
    for i in 0..4 {
        match src.prepare(&ctx, &mut pbr, &frame, &balls) {
            Err(GpuError::RequestDevice(m)) => {
                assert!(m.starts_with("device lost"), "frame {i}: {m}");
                if i == 0 {
                    println!("[raytrace_lost] prepare after the loss: {m}");
                }
            }
            other => panic!("frame {i}: expected the device-lost error, got {other:?}"),
        }
        assert!(src.device_lost());
        assert_eq!(src.name(), "octahedral-probes");
        assert!(src.probes_bound());
        assert!(src.ray_stats().fallen_back);
        assert!(
            src.fallback_reason()
                .is_some_and(|r| r.starts_with("device lost")),
            "{:?}",
            src.fallback_reason()
        );
        // Nothing was traced after the loss, and the inner probes never
        // ran on the dead device (their counters are reported now).
        assert_eq!(src.ray_stats().frames, traced_frames);
        assert_eq!(src.stats().frames, 0);
    }
}
