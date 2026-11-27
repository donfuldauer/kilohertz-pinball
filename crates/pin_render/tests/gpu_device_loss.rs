//! #133 (KP-052): black-box tests of the GPU device-loss work.
//!
//! - Pure (no GPU, always compiled): `device_features`, the one feature
//!   policy every context uses (SPEC §4 Subphase 5.4 item 4, SPEC §4
//!   Subphase 1.4), and the `GpuError::DeviceLost` message.
//! - `gpu-tests` (lavapipe on CI): after `Device::destroy` a ray-query
//!   reflection source returns `GpuError::DeviceLost` from `prepare`,
//!   `add_mesh` and `read_layer` and never panics; headless and windowed
//!   context constructors request the same features; a live context never
//!   reports a loss.
//!
//! Its own test binary, because tests here destroy devices: each destroying
//! test builds its own context and never shares it. Every test that creates
//! or destroys a device holds [`GPU_LOCK`] for its whole body, because
//! lavapipe crashes when devices are created and destroyed concurrently
//! (DECISIONS "1.4 — CI fix: lavapipe SIGSEGV").

use pin_render::gpu::{GpuError, device_features, wgpu};
use proptest::prelude::*;

/// The two timestamp features every context asks for when the adapter has
/// them.
fn timestamps() -> wgpu::Features {
    wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS
}

/// An arbitrary feature set from two raw words (unknown bits truncated).
fn features(native: u64, webgpu: u64) -> wgpu::Features {
    wgpu::Features::from_internal_flags(
        wgpu::FeaturesWGPU::from_bits_truncate(native),
        wgpu::FeaturesWebGPU::from_bits_truncate(webgpu),
    )
}

fn any_features() -> impl Strategy<Value = wgpu::Features> {
    (any::<u64>(), any::<u64>()).prop_map(|(a, b)| features(a, b))
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 512,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// SPEC §4 Subphase 5.4 item 4 / SPEC §4 Subphase 1.4: the requested
    /// device features never exceed the adapter's (a missing optional
    /// feature such as ray queries is dropped, never requested), every
    /// wanted feature the adapter has is kept, the timestamp pair is present
    /// exactly when the adapter offers it, and the policy is idempotent.
    #[test]
    fn device_features_is_the_wanted_plus_timestamps_within_the_adapter(
        adapter in any_features(),
        wanted in any_features(),
    ) {
        let got = device_features(adapter, wanted);
        prop_assert!(adapter.contains(got), "result {got:?} not within adapter {adapter:?}");
        prop_assert!(got.contains(wanted & adapter), "wanted ∩ adapter missing from {got:?}");
        prop_assert_eq!(got & timestamps(), adapter & timestamps());
        prop_assert_eq!(got, (wanted | timestamps()) & adapter);
        // Nothing outside wanted ∪ timestamps is ever added.
        prop_assert!((wanted | timestamps()).contains(got));
        // Idempotent in `wanted`.
        prop_assert_eq!(device_features(adapter, got), got);
        prop_assert_eq!(device_features(adapter, wanted | wanted), got);
        // With nothing wanted, only the adapter's timestamps.
        prop_assert_eq!(device_features(adapter, wgpu::Features::empty()), adapter & timestamps());
    }
}

/// SPEC §4 Subphase 5.4 item 4: fixed points of the feature policy (the
/// full and empty adapter).
#[test]
fn device_features_edges() {
    let all = wgpu::Features::all();
    let none = wgpu::Features::empty();
    assert_eq!(device_features(none, all), none);
    assert_eq!(device_features(all, none), timestamps());
    assert_eq!(device_features(all, all), all);
    assert_eq!(
        device_features(timestamps(), wgpu::Features::EXPERIMENTAL_RAY_QUERY),
        timestamps(),
        "a ray-query wish on an adapter without it is dropped"
    );
    assert_eq!(
        device_features(
            wgpu::Features::EXPERIMENTAL_RAY_QUERY | wgpu::Features::TIMESTAMP_QUERY,
            wgpu::Features::EXPERIMENTAL_RAY_QUERY
        ),
        wgpu::Features::EXPERIMENTAL_RAY_QUERY | wgpu::Features::TIMESTAMP_QUERY
    );
}

/// SPEC §4 Subphase 5.4 item 4: the device-lost error names the loss and
/// its reason.
#[test]
fn device_lost_error_display() {
    assert_eq!(
        GpuError::DeviceLost("x".into()).to_string(),
        "GPU device lost: x"
    );
    assert_eq!(
        GpuError::DeviceLost("Destroyed".into()).to_string(),
        "GPU device lost: Destroyed"
    );
}

#[cfg(feature = "gpu-tests")]
mod gpu {
    use pin_render::gpu::{
        GpuContext, GpuError, GpuOptions, create_instance, device_features, require_gpu, wgpu,
    };
    use pin_render::pbr::math::scale_translation;
    use pin_render::pbr::{
        DrawItem, FrameDesc, MaterialKind, MaterialParams, MeshGeometry, MeshId, PbrCamera,
        PbrRenderer, PointLight,
    };
    use pin_render::raytrace::{
        RAY_QUERY_FEATURE, RayCapability, RayQueryConfig, RayQueryReflections, ReflectionQuality,
    };
    use pin_render::reflections::{ReflectionSource, ReflectiveBall};
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{Mutex, MutexGuard};

    /// Serialises every device creation and destruction in this binary
    /// (lavapipe crashes on concurrent ones).
    static GPU_LOCK: Mutex<()> = Mutex::new(());

    fn gpu_lock() -> MutexGuard<'static, ()> {
        GPU_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    const R: f32 = 0.0135;

    /// Bound on device polls while waiting for the device-lost callback.
    const MAX_LOSS_POLLS: u32 = 8;

    /// `PIN_REQUIRE_RAY_QUERY=1`: a missing ray-query feature fails.
    fn require_ray_query() -> bool {
        std::env::var("PIN_REQUIRE_RAY_QUERY").is_ok_and(|v| v.trim() == "1")
    }

    /// A context that fails to create is fatal under either flag.
    fn required() -> bool {
        require_gpu() || require_ray_query()
    }

    /// The missing-feature rule of `raytrace_lost.rs`: fatal iff
    /// `PIN_REQUIRE_RAY_QUERY=1`, or `PIN_REQUIRE_GPU=1` on Vulkan.
    fn ray_query_missing_is_fatal(backend: wgpu::Backend) -> bool {
        require_ray_query() || (require_gpu() && backend == wgpu::Backend::Vulkan)
    }

    /// A fresh headless context with `wanted`, or `None` (with a note) when
    /// no adapter exists and none is required.
    fn context(test: &str, wanted: wgpu::Features) -> Option<GpuContext> {
        match GpuContext::new_headless_with_features(&GpuOptions::from_env(), wanted) {
            Ok(ctx) => Some(ctx),
            Err(e) if required() => panic!("[{test}] GPU required but {e}"),
            Err(e) => {
                println!("[{test}] SKIP: {e}");
                None
            }
        }
    }

    /// A fresh ray-query context, or `None` (with a note) when the adapter
    /// cannot trace and that is not fatal.
    fn rq_context(test: &str) -> Option<GpuContext> {
        let ctx = context(test, RAY_QUERY_FEATURE)?;
        if ctx.supports_ray_query() {
            return Some(ctx);
        }
        let info = ctx.adapter_info();
        assert!(
            !ray_query_missing_is_fatal(info.backend),
            "[{test}] ray queries required but adapter {} ({:?}) lacks EXPERIMENTAL_RAY_QUERY",
            info.name,
            info.backend
        );
        println!(
            "[{test}] SKIP: adapter {} ({:?}) lacks EXPERIMENTAL_RAY_QUERY",
            info.name, info.backend
        );
        None
    }

    /// Destroys the context's device and polls (bounded) until the context
    /// reports the loss.
    fn destroy_and_wait(test: &str, ctx: &GpuContext) {
        ctx.device().destroy();
        let mut polls = 0;
        while !ctx.device_lost() && polls < MAX_LOSS_POLLS {
            let _ = ctx.device().poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: Some(std::time::Duration::from_secs(60)),
            });
            polls += 1;
        }
        println!("[{test}] device-lost callback seen after {polls} poll(s)");
        assert!(
            ctx.device_lost(),
            "[{test}] device-lost callback not delivered within {MAX_LOSS_POLLS} polls"
        );
        let reason = ctx.device_lost_reason();
        assert!(
            reason.as_deref().is_some_and(|r| r.contains("Destroyed")),
            "[{test}] loss reason {reason:?}"
        );
    }

    /// Runs `f`, failing the test with the panic text if it unwinds.
    fn no_panic<T>(what: &str, f: impl FnOnce() -> T) -> T {
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(v) => v,
            Err(p) => {
                let msg = p
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                    .unwrap_or_else(|| "<non-string panic>".into());
                panic!("{what} panicked after the device loss: {msg}");
            }
        }
    }

    fn expect_lost(what: &str, r: Result<(), GpuError>) {
        match r {
            Err(GpuError::DeviceLost(reason)) => {
                assert!(!reason.is_empty(), "{what}: empty loss reason");
            }
            other => panic!("{what}: expected GpuError::DeviceLost, got {other:?}"),
        }
    }

    /// The two-ball scene of `raytrace_lost.rs`.
    struct Scene {
        pbr: PbrRenderer,
        sphere: MeshId,
        sphere_geo: MeshGeometry,
        /// Uploaded to the renderer but not registered with the source.
        other: MeshId,
        other_geo: MeshGeometry,
        draws: Vec<DrawItem>,
        balls: [ReflectiveBall; 2],
        cam: PbrCamera,
        lights: [PointLight; 1],
    }

    impl Scene {
        fn new(ctx: &GpuContext) -> Self {
            let mut pbr = PbrRenderer::new(ctx, 64, 64).unwrap();
            let sphere_geo = MeshGeometry::uv_sphere(R, 32, 24);
            let sphere = pbr.upload_mesh(&sphere_geo).unwrap();
            let other_geo = MeshGeometry::uv_sphere(R * 0.5, 12, 8);
            let other = pbr.upload_mesh(&other_geo).unwrap();
            let ball = |x: f32| DrawItem {
                mesh: sphere,
                model: scale_translation(1.0, [x, 0.0, R]),
                material: MaterialParams {
                    roughness: 0.06,
                    ..MaterialKind::ChromeSteel.params()
                },
                transparent: false,
                emission: 0.0,
            };
            let draws = vec![ball(-0.016), ball(0.016)];
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
            Self {
                pbr,
                sphere,
                sphere_geo,
                other,
                other_geo,
                draws,
                balls,
                cam,
                lights,
            }
        }

        fn source(&self, ctx: &GpuContext) -> RayQueryReflections {
            RayQueryReflections::new(
                ctx,
                &self.pbr,
                ReflectionQuality::BallsOnly,
                &[(self.sphere, &self.sphere_geo)],
                &RayQueryConfig::default(),
            )
            .unwrap()
        }

        fn prepare(
            &mut self,
            ctx: &GpuContext,
            src: &mut RayQueryReflections,
        ) -> Result<(), GpuError> {
            let frame = FrameDesc::new(&self.cam, &self.lights, &self.draws);
            src.prepare(ctx, &mut self.pbr, &frame, &self.balls)
        }
    }

    /// SPEC §4 Subphase 5.4 item 4 (acceptance 1): after `Device::destroy`
    /// the ray-query source's `prepare`, `add_mesh` (new and registered id)
    /// and `read_layer` return `GpuError::DeviceLost`, never panic, report
    /// the probes, and nothing runs on the dead device.
    #[test]
    fn traced_source_reports_device_lost_after_destroy() {
        const T: &str = "traced_source_reports_device_lost_after_destroy";
        let _g = gpu_lock();
        let Some(ctx) = rq_context(T) else { return };
        let mut scene = Scene::new(&ctx);
        let mut src = scene.source(&ctx);

        scene.prepare(&ctx, &mut src).expect("healthy traced frame");
        assert_eq!(src.name(), "ray_query");
        assert!(!ctx.device_lost());
        assert!(ctx.device_lost_reason().is_none());
        assert!(!src.device_lost());
        let traced = src.ray_stats().frames;
        assert_eq!(traced, 1);

        destroy_and_wait(T, &ctx);
        // The source learns the loss from the context alone (no callback
        // registration by the caller).
        assert!(
            src.device_lost(),
            "source does not report the context's loss"
        );

        let (sphere, other) = (scene.sphere, scene.other);
        for i in 0..4 {
            let r = no_panic(&format!("prepare #{i}"), || scene.prepare(&ctx, &mut src));
            if i == 0 {
                println!("[{T}] prepare after the loss: {r:?}");
            }
            expect_lost(&format!("prepare #{i}"), r);
            assert_eq!(src.name(), "octahedral-probes", "iteration {i}");

            let r = no_panic(&format!("add_mesh(new) #{i}"), || {
                src.add_mesh(&ctx, other, &scene.other_geo)
            });
            expect_lost(&format!("add_mesh(new) #{i}"), r);
            let r = no_panic(&format!("add_mesh(registered) #{i}"), || {
                src.add_mesh(&ctx, sphere, &scene.sphere_geo)
            });
            expect_lost(&format!("add_mesh(registered) #{i}"), r);

            let r = no_panic(&format!("read_layer #{i}"), || src.read_layer(&ctx, 0));
            match r {
                Err(GpuError::DeviceLost(_)) => {}
                Err(e) => panic!("read_layer #{i}: expected GpuError::DeviceLost, got {e:?}"),
                Ok(_) => panic!("read_layer #{i}: read a layer from a destroyed device"),
            }

            assert!(src.device_lost(), "iteration {i}");
            assert!(ctx.device_lost(), "iteration {i}");
            assert_eq!(src.ray_stats().frames, traced, "traced after the loss");
            assert_eq!(src.stats().frames, 0, "probes ran on the dead device");
        }
        drop(src);
        drop(scene);
        drop(ctx);
    }

    /// SPEC §4 Subphase 5.4 item 4 (acceptance 1, the non-traced path): a
    /// source already on the permanent probe fallback also returns
    /// `GpuError::DeviceLost` after the loss and does not run its probes.
    #[test]
    fn fallen_back_source_reports_device_lost_after_destroy() {
        const T: &str = "fallen_back_source_reports_device_lost_after_destroy";
        let _g = gpu_lock();
        let Some(ctx) = rq_context(T) else { return };
        let mut scene = Scene::new(&ctx);
        let mut src = scene.source(&ctx);
        src.fall_back("test");
        assert_eq!(src.name(), "octahedral-probes");
        scene.prepare(&ctx, &mut src).expect("healthy probe frame");
        assert!(!src.device_lost());
        assert_eq!(src.ray_stats().frames, 0, "nothing traced after fall_back");
        let probe_frames = src.stats().frames;
        assert_eq!(probe_frames, 1, "the probes rendered the healthy frame");

        destroy_and_wait(T, &ctx);
        assert!(src.device_lost());
        for i in 0..4 {
            let r = no_panic(&format!("prepare #{i}"), || scene.prepare(&ctx, &mut src));
            expect_lost(&format!("prepare #{i}"), r);
            assert_eq!(
                src.stats().frames,
                probe_frames,
                "probes ran after the loss"
            );
            assert_eq!(src.ray_stats().frames, 0);
            assert_eq!(src.name(), "octahedral-probes");
        }
        let r = no_panic("read_layer", || src.read_layer(&ctx, 0));
        assert!(
            matches!(r, Err(GpuError::DeviceLost(_))),
            "read_layer: {:?}",
            r.err()
        );
        drop(src);
        drop(scene);
        drop(ctx);
    }

    /// SPEC §4 Subphase 1.4 deliverable 1 (acceptance 3): the headless and
    /// the windowed constructor request device features through the same
    /// path, `device_features(adapter, wanted)`, for no wish and for ray
    /// queries; `with_instance` equals the windowed one with no wish.
    #[test]
    fn headless_and_windowed_contexts_request_the_same_features() {
        const T: &str = "headless_and_windowed_contexts_request_the_same_features";
        let _g = gpu_lock();
        let opts = GpuOptions::from_env();
        for wanted in [wgpu::Features::empty(), RAY_QUERY_FEATURE] {
            let (headless, adapter) = {
                let Some(ctx) = context(T, wanted) else {
                    return;
                };
                let f = (ctx.device().features(), ctx.adapter().features());
                if wanted == RAY_QUERY_FEATURE {
                    let info = ctx.adapter_info();
                    let offered = ctx.adapter().features().contains(RAY_QUERY_FEATURE);
                    println!(
                        "[{T}] adapter {} ({:?}) offers ray queries: {offered}",
                        info.name, info.backend
                    );
                    if offered {
                        assert!(
                            ctx.device().features().contains(RAY_QUERY_FEATURE),
                            "adapter offers ray queries but the device lacks them"
                        );
                        assert!(ctx.supports_ray_query());
                    } else {
                        assert!(
                            !ray_query_missing_is_fatal(info.backend),
                            "[{T}] ray queries required but adapter {} lacks them",
                            info.name
                        );
                        assert!(!ctx.supports_ray_query());
                    }
                }
                f
            };
            assert_eq!(
                headless,
                device_features(adapter, wanted),
                "headless context, wanted {wanted:?}"
            );

            let windowed = {
                let ctx = GpuContext::with_instance_features(
                    create_instance(opts.backends),
                    &opts,
                    None,
                    wanted,
                )
                .expect("with_instance_features on the adapter the headless context found");
                assert_eq!(ctx.adapter().features(), adapter, "a different adapter");
                if wanted == RAY_QUERY_FEATURE {
                    assert_eq!(
                        ctx.supports_ray_query(),
                        adapter.contains(RAY_QUERY_FEATURE)
                    );
                }
                ctx.device().features()
            };
            assert_eq!(windowed, headless, "wanted {wanted:?}");
        }

        let plain = {
            let ctx = GpuContext::with_instance(create_instance(opts.backends), &opts, None)
                .expect("with_instance");
            (ctx.device().features(), ctx.adapter().features())
        };
        assert_eq!(
            plain.0,
            device_features(plain.1, wgpu::Features::empty()),
            "with_instance must equal with_instance_features(.., empty)"
        );
    }

    /// SPEC §4 Subphase 5.4 item 4: a live context never reports a loss,
    /// and dropping one is not a loss.
    #[test]
    fn live_context_reports_no_loss() {
        const T: &str = "live_context_reports_no_loss";
        let _g = gpu_lock();
        for _ in 0..2 {
            let Some(ctx) = context(T, wgpu::Features::empty()) else {
                return;
            };
            let _ = ctx.device().poll(wgpu::PollType::Poll);
            let lost = ctx.device_lost();
            let reason = ctx.device_lost_reason();
            drop(ctx);
            assert!(!lost, "a live context reported a loss");
            assert!(
                reason.is_none(),
                "a live context has a loss reason {reason:?}"
            );
        }
    }
}
