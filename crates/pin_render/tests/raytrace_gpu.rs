//! 5.4 ray-query reflections on a real adapter (gpu-tests).
//!
//! One shared context per binary, created with
//! `GpuContext::new_headless_with_features(RAY_QUERY_FEATURE)`. Tests that
//! compare against the probe path run on every adapter; tests that need
//! ray queries skip with a printed note when the device lacks the feature,
//! and fail instead when `PIN_REQUIRE_GPU=1` on a Vulkan adapter (the
//! backend the survey found the feature on; DECISIONS "5.4 — ray-query
//! gpu-tests under PIN_REQUIRE_GPU").
//!
//! Image policy D30: analytic region assertions first (mean colour over
//! the pixels where exact mirror geometry puts the other ball), then a
//! regression snapshot `tests/fixtures/raytrace/<name>_<backend>.png` with
//! the per-backend tolerance of `tests/fixtures/raytrace/tolerance.toml`
//! (`PIN_UPDATE_SNAPSHOTS=1` rewrites). Images for the visual checklist go
//! to `<target dir>/tmp/5_4_images/`.
#![cfg(feature = "gpu-tests")]

use pin_render::gpu::{GpuContext, GpuOptions, require_gpu, wait_for_device, wgpu};
use pin_render::pbr::math::scale_translation;
use pin_render::pbr::{
    DrawItem, Environment, FrameDesc, HdrImage, MaterialKind, MaterialParams, MeshGeometry, MeshId,
    PbrCamera, PbrRenderer, PointLight,
};
use pin_render::raytrace::{
    CLEARCOAT_LAYER, RAY_ATLAS_LAYERS, RAY_QUERY_FEATURE, RayCapability, RayCaps, RayQueryConfig,
    RayQueryError, RayQueryReflections, ReflectionQuality, select_reflection_source,
    select_reflection_source_with,
};
use pin_render::reflections::{
    OctahedralProbes, PROBE_RESOLUTION, ReflectionSource, ReflectiveBall, oct_decode,
};
use std::path::PathBuf;
use std::sync::OnceLock;

fn ctx() -> Option<&'static GpuContext> {
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    CTX.get_or_init(|| {
        match GpuContext::new_headless_with_features(&GpuOptions::from_env(), RAY_QUERY_FEATURE) {
            Ok(ctx) => {
                let info = ctx.adapter_info();
                println!(
                    "[raytrace_gpu] adapter: {} ({:?}, {:?}); ray query: {}",
                    info.name,
                    info.backend,
                    info.device_type,
                    ctx.supports_ray_query()
                );
                Some(ctx)
            }
            Err(e) if require_gpu() => panic!("[raytrace_gpu] PIN_REQUIRE_GPU=1 but {e}"),
            Err(e) => {
                println!("[raytrace_gpu] SKIP: {e}; set PIN_REQUIRE_GPU=1 to fail instead");
                None
            }
        }
    })
    .as_ref()
}

/// The context when it can trace rays; `None` (with a note) otherwise.
fn rq_ctx(test: &str) -> Option<&'static GpuContext> {
    let ctx = ctx()?;
    if ctx.supports_ray_query() {
        return Some(ctx);
    }
    let info = ctx.adapter_info();
    if require_gpu() && info.backend == wgpu::Backend::Vulkan {
        panic!(
            "[{test}] PIN_REQUIRE_GPU=1 on Vulkan adapter {} without EXPERIMENTAL_RAY_QUERY",
            info.name
        );
    }
    println!(
        "[{test}] SKIP: adapter {} ({:?}) lacks EXPERIMENTAL_RAY_QUERY",
        info.name, info.backend
    );
    None
}

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}
fn add_scaled(a: V3, d: V3, t: f32) -> V3 {
    [a[0] + t * d[0], a[1] + t * d[1], a[2] + t * d[2]]
}

fn image_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("5_4_images");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn save(name: &str, img: &HdrImage) {
    let path = image_dir().join(format!("{name}.png"));
    image::save_buffer(
        &path,
        &img.to_srgb8(),
        img.width,
        img.height,
        image::ColorType::Rgba8,
    )
    .unwrap();
    println!("[{name}] image: {}", path.display());
}

/// D30 (b): the per-backend snapshot, after the analytic checks.
fn check_snapshot(ctx: &GpuContext, name: &str, img: &HdrImage) {
    let backend = format!("{:?}", ctx.adapter_info().backend).to_lowercase();
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/raytrace");
    let path = dir.join(format!("{name}_{backend}.png"));
    let got = img.to_srgb8();
    if std::env::var("PIN_UPDATE_SNAPSHOTS").is_ok_and(|v| v.trim() == "1") {
        std::fs::create_dir_all(&dir).unwrap();
        image::save_buffer(&path, &got, img.width, img.height, image::ColorType::Rgba8).unwrap();
        println!("[{name}] snapshot written to {}", path.display());
        return;
    }
    let tolerances: toml::Table =
        toml::from_str(&std::fs::read_to_string(dir.join("tolerance.toml")).unwrap()).unwrap();
    let Some(tol) = tolerances.get(&backend).and_then(|t| t.as_table()) else {
        panic!("[{name}] no snapshot tolerance for backend {backend} in tolerance.toml");
    };
    let mean_max = tol["mean"].as_float().unwrap();
    let far_max = tol["far_fraction"].as_float().unwrap();
    let want = image::open(&path)
        .unwrap_or_else(|e| panic!("[{name}] missing snapshot {} ({e})", path.display()))
        .to_rgba8();
    assert_eq!((want.width(), want.height()), (img.width, img.height));
    let want = want.into_raw();
    let (mut sum, mut far) = (0u64, 0usize);
    for (g, w) in got.chunks(4).zip(want.chunks(4)) {
        let d: Vec<u32> = (0..3).map(|k| u32::from(g[k].abs_diff(w[k]))).collect();
        sum += u64::from(d.iter().sum::<u32>());
        far += usize::from(d.iter().any(|&v| v > 16));
    }
    let px = (img.width * img.height) as usize;
    let mean = sum as f64 / (px * 3) as f64;
    println!("[{name}] snapshot ({backend}): mean |diff| {mean:.3}/255, {far} px off by > 16/255");
    assert!(
        mean <= mean_max,
        "[{name}] mean diff {mean:.3} > {mean_max}"
    );
    assert!(
        far as f64 <= far_max * px as f64,
        "[{name}] {far} of {px} px differ"
    );
}

// ---------------------------------------------------------------------
// The scene: two chrome balls side by side (red and blue emission) on a
// clear-coated playfield, a dim environment, one light.
// ---------------------------------------------------------------------

const W: u32 = 256;
const R: f32 = 0.0135;
const A: V3 = [-0.016, 0.0, R];
const B: V3 = [0.016, 0.0, R];
const DIM: Environment = Environment {
    sky: [0.05, 0.05, 0.06],
    horizon: [0.1, 0.1, 0.1],
    ground: [0.02, 0.02, 0.02],
};

fn camera() -> PbrCamera {
    PbrCamera::look_at(
        [0.0, -0.085, 0.03],
        [0.0, 0.0, R],
        [0.0, 0.0, 1.0],
        35f32.to_radians(),
        1.0,
        0.005,
        5.0,
    )
}

fn ball_material(emissive: V3, roughness: f32) -> MaterialParams {
    MaterialParams {
        base_color: [0.9, 0.9, 0.9, 1.0],
        emissive,
        roughness,
        ..MaterialKind::ChromeSteel.params()
    }
}

const RED: V3 = [1.5, 0.03, 0.03];
const BLUE: V3 = [0.03, 0.03, 1.5];

struct Rig {
    pbr: PbrRenderer,
    sphere: MeshId,
    quad: MeshId,
    sphere_geo: MeshGeometry,
    quad_geo: MeshGeometry,
}

impl Rig {
    fn new(ctx: &GpuContext) -> Rig {
        let mut pbr = PbrRenderer::new(ctx, W, W).unwrap();
        let sphere_geo = MeshGeometry::uv_sphere(R, 64, 48);
        let quad_geo = MeshGeometry::quad(1.0);
        let sphere = pbr.upload_mesh(&sphere_geo).unwrap();
        let quad = pbr.upload_mesh(&quad_geo).unwrap();
        Rig {
            pbr,
            sphere,
            quad,
            sphere_geo,
            quad_geo,
        }
    }

    fn meshes(&self) -> [(MeshId, &MeshGeometry); 2] {
        [(self.sphere, &self.sphere_geo), (self.quad, &self.quad_geo)]
    }

    /// Ball A (red, draw 0), ball B (blue, draw 1), the playfield (draw 2).
    fn draws(&self, ball_roughness: f32, cc_roughness: f32) -> Vec<DrawItem> {
        let ball = |c: V3, e: V3| DrawItem {
            mesh: self.sphere,
            model: scale_translation(1.0, c),
            material: ball_material(e, ball_roughness),
            transparent: false,
            emission: 0.0,
        };
        vec![
            ball(A, RED),
            ball(B, BLUE),
            DrawItem {
                mesh: self.quad,
                model: scale_translation(0.5, [0.0; 3]),
                material: MaterialParams {
                    clearcoat_roughness: cc_roughness,
                    ..MaterialKind::PlayfieldWood.params()
                },
                transparent: false,
                emission: 0.0,
            },
        ]
    }

    fn render(
        &mut self,
        ctx: &GpuContext,
        source: &mut dyn ReflectionSource,
        draws: &[DrawItem],
    ) -> HdrImage {
        let cam = camera();
        let lights = [PointLight::new([0.0, -0.05, 0.2], [1.0; 3], 0.1, 5.0)];
        let frame = FrameDesc {
            environment: DIM,
            ..FrameDesc::new(&cam, &lights, draws)
        };
        source
            .prepare(ctx, &mut self.pbr, &frame, &balls())
            .unwrap();
        let binding = source.binding();
        self.pbr
            .render_reflected_to_image(ctx, &frame, Some(&binding))
            .unwrap()
    }
}

fn balls() -> [ReflectiveBall; 2] {
    [
        ReflectiveBall {
            position: A,
            radius: R,
            draw: 0,
        },
        ReflectiveBall {
            position: B,
            radius: R,
            draw: 1,
        },
    ]
}

fn probes(ctx: &GpuContext, rig: &Rig) -> OctahedralProbes {
    OctahedralProbes::new(ctx, &rig.pbr, RayQueryConfig::default().probes).unwrap()
}

/// Renders `frames` frames with a fresh rig and the source `make` builds.
fn run(
    ctx: &GpuContext,
    frames: usize,
    ball_roughness: f32,
    cc_roughness: f32,
    make: impl FnOnce(&Rig) -> Box<dyn ReflectionSource>,
) -> (Vec<HdrImage>, Box<dyn ReflectionSource>) {
    let mut rig = Rig::new(ctx);
    let mut source = make(&rig);
    let draws = rig.draws(ball_roughness, cc_roughness);
    let images = (0..frames)
        .map(|_| rig.render(ctx, source.as_mut(), &draws))
        .collect();
    (images, source)
}

fn assert_identical(what: &str, a: &[HdrImage], b: &[HdrImage]) {
    assert_eq!(a.len(), b.len());
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        let bits = |img: &HdrImage| -> Vec<u32> {
            img.pixels.iter().flatten().map(|v| v.to_bits()).collect()
        };
        assert!(bits(x) == bits(y), "{what}: frame {i} differs");
    }
}

// ---------------------------------------------------------------------
// Mirror geometry of the two-ball scene (exact spheres).
// ---------------------------------------------------------------------

fn pixel_ray(cam: &PbrCamera, x: u32, y: u32) -> V3 {
    let t = (0.5 * cam.fov_y).tan();
    let nx = (2.0 * (x as f32 + 0.5) / W as f32 - 1.0) * t * cam.aspect;
    let ny = (1.0 - 2.0 * (y as f32 + 0.5) / W as f32) * t;
    let v = &cam.view;
    let s = [v[0][0], v[1][0], v[2][0]];
    let u = [v[0][1], v[1][1], v[2][1]];
    let f = [-v[0][2], -v[1][2], -v[2][2]];
    norm([
        s[0] * nx + u[0] * ny + f[0],
        s[1] * nx + u[1] * ny + f[1],
        s[2] * nx + u[2] * ny + f[2],
    ])
}

/// First forward hit distance of the ray `(o, d)` on sphere `(c, r)`.
fn hit_sphere(o: V3, d: V3, c: V3, r: f32) -> Option<f32> {
    let oc = sub(o, c);
    let b = dot(oc, d);
    let disc = b * b - (dot(oc, oc) - r * r);
    if disc < 0.0 {
        return None;
    }
    let s = disc.sqrt();
    [-b - s, -b + s].into_iter().find(|&t| t > 1e-6)
}

/// Where pixel (x, y) shows ball `c`'s mirror: `(surface point, reflected
/// direction)`, or `None` off the ball (with a 6 % rim margin).
fn on_ball(cam: &PbrCamera, x: u32, y: u32, c: V3) -> Option<(V3, V3)> {
    let d = pixel_ray(cam, x, y);
    hit_sphere(cam.eye, d, c, R * 0.94)?;
    let other = if c == A { B } else { A };
    let t = hit_sphere(cam.eye, d, c, R)?;
    // Not hidden behind the other ball.
    if hit_sphere(cam.eye, d, other, R).is_some_and(|u| u < t) {
        return None;
    }
    let p = add_scaled(cam.eye, d, t);
    let n = norm(sub(p, c));
    let k = 2.0 * dot(d, n);
    Some((p, [d[0] - k * n[0], d[1] - k * n[1], d[2] - k * n[2]]))
}

#[derive(Debug, Default)]
struct Region {
    sum: [f64; 3],
    n: usize,
}

impl Region {
    fn add(&mut self, p: [f32; 4]) {
        for (s, v) in self.sum.iter_mut().zip(p) {
            *s += f64::from(v);
        }
        self.n += 1;
    }
    fn mean(&self) -> [f64; 3] {
        self.sum.map(|v| v / self.n.max(1) as f64)
    }
}

/// On ball `c` (other ball `o`): the "parallax" pixels whose exact mirror
/// ray hits the other ball (radius shrunk 15 %) while the same direction
/// from the centre (what a probe stores) misses it (radius grown 15 %),
/// and the "clear" pixels whose exact ray misses the grown other ball.
fn regions(img: &HdrImage, c: V3, o: V3) -> (Region, Region) {
    let cam = camera();
    let (mut parallax, mut clear) = (Region::default(), Region::default());
    for y in 0..W {
        for x in 0..W {
            let Some((p, r)) = on_ball(&cam, x, y, c) else {
                continue;
            };
            let exact = hit_sphere(p, r, o, 0.85 * R).is_some();
            let exact_loose = hit_sphere(p, r, o, 1.15 * R).is_some();
            let centre_loose = hit_sphere(c, r, o, 1.15 * R).is_some();
            if exact && !centre_loose {
                parallax.add(img.at(x, y));
            } else if !exact_loose {
                clear.add(img.at(x, y));
            }
        }
    }
    (parallax, clear)
}

/// The ray-query source for the rig (panics if setup fails on a device
/// that has the feature).
fn ray_source(ctx: &GpuContext, rig: &Rig, quality: ReflectionQuality) -> RayQueryReflections {
    RayQueryReflections::new(
        ctx,
        &rig.pbr,
        quality,
        &rig.meshes(),
        &RayQueryConfig::default(),
    )
    .unwrap()
}

// ---------------------------------------------------------------------
// (a) feature absent / device lost: identical to the probe path.
// ---------------------------------------------------------------------

#[test]
fn raytrace_forced_fallback_is_byte_identical_to_probes() {
    let Some(ctx) = ctx() else { return };
    let (want, p) = run(ctx, 3, 0.06, 0.06, |rig| Box::new(probes(ctx, rig)));
    assert_eq!(p.name(), "octahedral-probes");
    for caps in [
        RayCaps {
            ray_query: false,
            device_lost: false,
        },
        RayCaps {
            ray_query: true,
            device_lost: true,
        },
    ] {
        for q in [
            ReflectionQuality::BallsOnly,
            ReflectionQuality::BallsAndClearCoat,
        ] {
            let (got, src) = run(ctx, 3, 0.06, 0.06, |rig| {
                select_reflection_source_with(
                    &caps,
                    ctx,
                    q,
                    &rig.pbr,
                    &rig.meshes(),
                    &RayQueryConfig::default(),
                )
            });
            assert_eq!(src.name(), "octahedral-probes", "{caps:?} {q}");
            assert_identical(&format!("{caps:?} {q}"), &want, &got);
            assert_eq!(src.stats(), p.stats());
        }
    }
}

/// `RayQueryReflections::new` declines without the feature (only checkable
/// on adapters that lack it) or for quality off.
#[test]
fn raytrace_new_declines_with_a_reason() {
    let Some(ctx) = ctx() else { return };
    let rig = Rig::new(ctx);
    let off = RayQueryReflections::new(
        ctx,
        &rig.pbr,
        ReflectionQuality::Off,
        &rig.meshes(),
        &RayQueryConfig::default(),
    );
    match off {
        Err(RayQueryError::QualityOff) => {}
        Err(RayQueryError::FeatureAbsent { adapter }) => {
            assert!(!ctx.supports_ray_query());
            println!("[new_declines_with_a_reason] feature absent on {adapter}");
        }
        other => panic!("expected a refusal, got {:?}", other.err()),
    }
    if !ctx.supports_ray_query() {
        let e = RayQueryReflections::new(
            ctx,
            &rig.pbr,
            ReflectionQuality::BallsOnly,
            &rig.meshes(),
            &RayQueryConfig::default(),
        )
        .err()
        .unwrap();
        assert!(e.to_string().contains("EXPERIMENTAL_RAY_QUERY"), "{e}");
        // The real device: the selection falls back.
        let src = select_reflection_source(
            ctx,
            ReflectionQuality::BallsAndClearCoat,
            &rig.pbr,
            &rig.meshes(),
            &RayQueryConfig::default(),
        );
        assert_eq!(src.name(), "octahedral-probes");
    } else {
        let mut bad = MeshGeometry::quad(1.0);
        bad.indices[0] = 99;
        let e = RayQueryReflections::new(
            ctx,
            &rig.pbr,
            ReflectionQuality::BallsOnly,
            &[(rig.quad, &bad)],
            &RayQueryConfig::default(),
        )
        .err()
        .unwrap();
        assert!(matches!(e, RayQueryError::InvalidMesh(_)), "{e}");
    }
}

// ---------------------------------------------------------------------
// (d) quality off equals probes.
// ---------------------------------------------------------------------

#[test]
fn raytrace_quality_off_equals_probes() {
    let Some(ctx) = ctx() else { return };
    let (want, _) = run(ctx, 3, 0.06, 0.06, |rig| Box::new(probes(ctx, rig)));
    let (got, src) = run(ctx, 3, 0.06, 0.06, |rig| {
        select_reflection_source(
            ctx,
            ReflectionQuality::default(),
            &rig.pbr,
            &rig.meshes(),
            &RayQueryConfig::default(),
        )
    });
    assert_eq!(src.name(), "octahedral-probes");
    assert_identical("quality off", &want, &got);
}

// ---------------------------------------------------------------------
// (b) two balls reflect each other.
// ---------------------------------------------------------------------

#[test]
fn raytrace_two_balls_reflect_each_other() {
    let Some(ctx) = rq_ctx("two_balls_reflect_each_other") else {
        return;
    };
    let (probe_imgs, _) = run(ctx, 1, 0.06, 0.06, |rig| Box::new(probes(ctx, rig)));
    let (ray_imgs, src) = run(ctx, 2, 0.06, 0.06, |rig| {
        let s = select_reflection_source(
            ctx,
            ReflectionQuality::BallsOnly,
            &rig.pbr,
            &rig.meshes(),
            &RayQueryConfig::default(),
        );
        assert_eq!(s.name(), "ray_query");
        s
    });
    assert_eq!(src.name(), "ray_query");
    let (probe, ray) = (&probe_imgs[0], &ray_imgs[1]);
    // No flicker with static input.
    assert_identical("static input", &ray_imgs[..1], &ray_imgs[1..]);
    assert!(
        ray.pixels
            .iter()
            .all(|p| p.iter().all(|c| c.is_finite() && *c >= 0.0))
    );
    save("raytrace_two_balls_probe", probe);
    save("raytrace_two_balls_ray", ray);

    // Ball A (red) shows blue B; ball B (blue) shows red A.
    for (name, c, o, ch, other_ch) in [("A", A, B, 2usize, 0usize), ("B", B, A, 0, 2)] {
        let (ray_par, ray_clear) = regions(ray, c, o);
        let (probe_par, _) = regions(probe, c, o);
        println!(
            "ball {name}: parallax region {} px: ray {:.3?}, probe {:.3?}; clear region {} px: ray {:.3?}",
            ray_par.n,
            ray_par.mean(),
            probe_par.mean(),
            ray_clear.n,
            ray_clear.mean()
        );
        assert!(
            ray_par.n >= 40,
            "ball {name}: parallax region {} px",
            ray_par.n
        );
        assert!(
            ray_clear.n >= 200,
            "ball {name}: clear region {} px",
            ray_clear.n
        );
        let (rp, pp, rc) = (ray_par.mean(), probe_par.mean(), ray_clear.mean());
        // The ray image shows the other ball's colour there ...
        assert!(
            rp[ch] > 0.5,
            "ball {name}: other ball missing in the ray image: {rp:?}"
        );
        assert!(
            rp[ch] > 3.0 * rc[ch],
            "ball {name}: ray image not tinted by the other ball: {rp:?} vs clear {rc:?}"
        );
        // ... the probe image does not (no parallax).
        assert!(
            pp[ch] < 0.4 * rp[ch],
            "ball {name}: probe image already shows the other ball: {pp:?} vs ray {rp:?}"
        );
        // Each ball keeps its own emission everywhere.
        assert!(rp[other_ch] > 1.0 && rc[other_ch] > 1.0, "{rp:?} {rc:?}");
    }
    check_snapshot(ctx, "two_balls", ray);
}

/// The ball map itself: straight up is the environment sky (a miss), the
/// direction towards the other ball is its colour.
#[test]
fn raytrace_traced_ball_map_holds_the_other_ball_and_the_sky() {
    let Some(ctx) = rq_ctx("traced_ball_map") else {
        return;
    };
    let mut rig = Rig::new(ctx);
    let mut src = ray_source(ctx, &rig, ReflectionQuality::BallsOnly);
    let draws = rig.draws(0.06, 0.06);
    rig.render(ctx, &mut src, &draws);
    let map = src.read_layer(ctx, 0).unwrap();
    assert_eq!(map.width, PROBE_RESOLUTION);
    let texel = |dir: V3| {
        let uv = pin_render::reflections::oct_encode(dir);
        let px = |u: f32| ((u * PROBE_RESOLUTION as f32) as u32).min(PROBE_RESOLUTION - 1);
        map.at(px(uv[0]), px(uv[1]))
    };
    let up = texel([0.0, 0.0, 1.0]);
    assert!(
        (up[0] - DIM.sky[0]).abs() < 0.01 && (up[2] - DIM.sky[2]).abs() < 0.01,
        "sky {up:?}"
    );
    let towards_b = texel([1.0, 0.0, 0.0]);
    assert!(
        towards_b[2] > 1.0 && towards_b[2] > 5.0 * towards_b[0],
        "{towards_b:?}"
    );
    // Nothing black: every texel at least the darkest environment value.
    let darkest = map
        .pixels
        .iter()
        .map(|p| p[0] + p[1] + p[2])
        .fold(f32::INFINITY, f32::min);
    assert!(
        darkest >= 0.9 * 3.0 * DIM.ground[0],
        "darkest texel {darkest}"
    );
    let _ = oct_decode([0.5, 0.5]);
}

// ---------------------------------------------------------------------
// (c) roughness above the cutoff equals the probe output.
// ---------------------------------------------------------------------

#[test]
fn raytrace_rough_surfaces_equal_probe_output() {
    let Some(ctx) = rq_ctx("rough_surfaces_equal_probe_output") else {
        return;
    };
    let cutoff = RayQueryConfig::default().ray_roughness_cutoff;
    for rough in [cutoff + 0.01, 0.6, 1.0] {
        let (want, _) = run(ctx, 3, rough, rough, |rig| Box::new(probes(ctx, rig)));
        for q in [
            ReflectionQuality::BallsOnly,
            ReflectionQuality::BallsAndClearCoat,
        ] {
            let (got, src) = run(ctx, 3, rough, rough, |rig| {
                Box::new(ray_source(ctx, rig, q))
            });
            assert_eq!(src.name(), "ray_query");
            assert_identical(&format!("roughness {rough} {q}"), &want, &got);
        }
    }
    // At the cutoff the ball is traced: the image differs from the probes.
    let (want, _) = run(ctx, 1, cutoff, 1.0, |rig| Box::new(probes(ctx, rig)));
    let (got, _) = run(ctx, 1, cutoff, 1.0, |rig| {
        Box::new(ray_source(ctx, rig, ReflectionQuality::BallsOnly))
    });
    assert!(
        want[0].pixels != got[0].pixels,
        "roughness at the cutoff must be traced"
    );
}

// ---------------------------------------------------------------------
// (e) acceleration-structure budget.
// ---------------------------------------------------------------------

#[test]
fn raytrace_blas_built_once_tlas_refit_per_frame() {
    let Some(ctx) = rq_ctx("blas_built_once_tlas_refit_per_frame") else {
        return;
    };
    let mut rig = Rig::new(ctx);
    let mut src = ray_source(ctx, &rig, ReflectionQuality::BallsAndClearCoat);
    assert_eq!(src.ray_stats().blas_builds, 2);
    let draws = rig.draws(0.06, 0.06);
    const N: u64 = 6;
    for i in 1..=N {
        // Moving balls: only the TLAS changes.
        let mut moved = draws.clone();
        moved[0].model = scale_translation(1.0, [A[0], 0.001 * i as f32, R]);
        rig.render(ctx, &mut src, &moved);
        let s = src.ray_stats();
        assert_eq!(s.tlas_refits, i);
        assert_eq!(s.frames, i);
    }
    let s = src.ray_stats();
    println!("{s:?}");
    assert_eq!(s.blas_builds, 2, "static meshes are built once");
    assert_eq!(s.static_meshes, 2);
    assert_eq!(s.last_instances, 3);
    assert_eq!(s.last_skipped_draws, 0);
    assert_eq!(s.last_traced_layers, 3, "two balls and the clear coat");
    assert_eq!(s.total_traced_layers, 3 * N);
    assert_eq!(s.tlas_allocations, 0);
    assert!(!s.fallen_back);
    let ps = src.stats();
    assert_eq!(ps.frames, N);
    assert_eq!(ps.last_frame_updates, 3);
    assert_eq!(ps.budget, RAY_ATLAS_LAYERS);
    assert_eq!(src.plan().draw_layers, [0, 1, CLEARCOAT_LAYER]);
    // Registering a known mesh again builds nothing.
    let sphere_geo = rig.sphere_geo.clone();
    src.add_mesh(ctx, rig.sphere, &sphere_geo).unwrap();
    assert_eq!(src.ray_stats().blas_builds, 2);
    // Many draws: the TLAS grows once, BLAS count unchanged.
    let many: Vec<DrawItem> = (0..100)
        .map(|i| DrawItem {
            model: scale_translation(1.0, [0.05 + 0.03 * i as f32, 0.2, R]),
            ..draws[0]
        })
        .chain(draws.iter().copied())
        .collect();
    let cam = camera();
    let frame = FrameDesc::new(&cam, &[], &many);
    src.prepare(ctx, &mut rig.pbr, &frame, &[]).unwrap();
    let s = src.ray_stats();
    assert_eq!(s.tlas_allocations, 1);
    assert_eq!(s.last_instances, 103);
    assert_eq!(s.blas_builds, 2);
    assert_eq!(s.tlas_refits, N + 1);
}

// ---------------------------------------------------------------------
// Clear coat (half resolution) and the runtime fallback.
// ---------------------------------------------------------------------

/// The playfield in front of the balls mirrors them with the clear coat
/// traced; balls-only leaves it at the environment.
#[test]
fn raytrace_clear_coat_reflects_the_balls() {
    let Some(ctx) = rq_ctx("clear_coat_reflects_the_balls") else {
        return;
    };
    let (balls_only, _) = run(ctx, 1, 0.06, 0.06, |rig| {
        Box::new(ray_source(ctx, rig, ReflectionQuality::BallsOnly))
    });
    let (coat, _) = run(ctx, 1, 0.06, 0.06, |rig| {
        Box::new(ray_source(ctx, rig, ReflectionQuality::BallsAndClearCoat))
    });
    save("raytrace_clearcoat", &coat[0]);
    let cam = camera();
    let mut on = Region::default();
    let mut off = Region::default();
    let mut ctl_on = Region::default();
    let mut ctl_off = Region::default();
    for y in 0..W {
        for x in 0..W {
            let d = pixel_ray(&cam, x, y);
            if d[2] >= -1e-4 {
                continue;
            }
            let t = -cam.eye[2] / d[2];
            // Not behind a ball.
            if [A, B]
                .iter()
                .any(|&c| hit_sphere(cam.eye, d, c, 1.05 * R).is_some_and(|u| u < t))
            {
                continue;
            }
            let p = add_scaled(cam.eye, d, t);
            if p[0].abs() > 0.2 || p[1].abs() > 0.2 {
                continue;
            }
            let r = [d[0], d[1], -d[2]];
            if hit_sphere(p, r, A, 0.8 * R).is_some() {
                on.add(coat[0].at(x, y));
                off.add(balls_only[0].at(x, y));
            } else if [A, B]
                .iter()
                .all(|&c| hit_sphere(p, r, c, 1.5 * R).is_none())
            {
                ctl_on.add(coat[0].at(x, y));
                ctl_off.add(balls_only[0].at(x, y));
            }
        }
    }
    let (on_m, off_m) = (on.mean(), off.mean());
    let (c_on, c_off) = (ctl_on.mean(), ctl_off.mean());
    println!(
        "clear coat: red-ball mirror {} px: traced {on_m:.4?}, balls only {off_m:.4?}; \
         control {} px: traced {c_on:.4?}, balls only {c_off:.4?}",
        on.n, ctl_on.n
    );
    assert!(on.n >= 30, "mirror region {} px", on.n);
    assert!(ctl_on.n >= 1000, "control region {} px", ctl_on.n);
    // The red ball appears in the traced clear coat ...
    let gain = [0, 1, 2].map(|k| on_m[k] - off_m[k]);
    assert!(gain[0] > 0.05, "no red reflection: {on_m:?} vs {off_m:?}");
    assert!(
        gain[0] > 3.0 * gain[2].abs(),
        "reflection not red: {gain:?}"
    );
    // ... and where rays miss, the coat shows the same environment as
    // without tracing (no black holes).
    for (on_k, off_k) in c_on.iter().zip(c_off) {
        assert!(
            (on_k - off_k).abs() < 0.01 + 0.02 * off_k,
            "control region differs: {c_on:?} vs {c_off:?}"
        );
    }
    assert!(
        coat[0]
            .pixels
            .iter()
            .all(|p| p.iter().all(|c| c.is_finite() && *c >= 0.0))
    );
}

/// After the permanent fallback the source renders exactly what a fresh
/// probe path renders, and reports the probes.
#[test]
fn raytrace_runtime_fallback_keeps_rendering_with_probes() {
    let Some(ctx) = rq_ctx("runtime_fallback_keeps_rendering_with_probes") else {
        return;
    };
    let (want, _) = run(ctx, 2, 0.06, 0.06, |rig| Box::new(probes(ctx, rig)));
    let mut rig = Rig::new(ctx);
    let mut src = ray_source(ctx, &rig, ReflectionQuality::BallsOnly);
    let draws = rig.draws(0.06, 0.06);
    let traced = rig.render(ctx, &mut src, &draws);
    assert!(traced.pixels != want[0].pixels);
    assert!(!src.probes_bound());
    src.fall_back("simulated device loss");
    src.fall_back("again");
    assert_eq!(src.fallback_reason(), Some("simulated device loss"));
    assert_eq!(src.name(), "octahedral-probes");
    let got: Vec<HdrImage> = (0..2).map(|_| rig.render(ctx, &mut src, &draws)).collect();
    assert_identical("after fallback", &want, &got);
    assert!(src.probes_bound());
    assert!(src.ray_stats().fallen_back);
    assert_eq!(src.ray_stats().frames, 1);
    assert_eq!(src.stats().frames, 2);
}

// ---------------------------------------------------------------------
// Non-gating benchmark (verification 3).
// ---------------------------------------------------------------------

/// Records the added time per frame of each quality setting (prepare +
/// main pass, waited for on the device). Prints only; asserts nothing
/// about time (D8). Run with `--ignored --nocapture`.
#[test]
#[ignore = "non-gating benchmark; run with --ignored --nocapture"]
fn raytrace_benchmark_added_gpu_time_per_quality() {
    let Some(ctx) = rq_ctx("benchmark") else {
        return;
    };
    const FRAMES: u32 = 20;
    let mut base = None;
    for q in ReflectionQuality::ALL {
        let mut rig = Rig::new(ctx);
        let mut src =
            select_reflection_source(ctx, q, &rig.pbr, &rig.meshes(), &RayQueryConfig::default());
        let draws = rig.draws(0.06, 0.06);
        rig.render(ctx, src.as_mut(), &draws); // warm-up
        let cam = camera();
        let frame = FrameDesc::new(&cam, &[], &draws);
        let start = std::time::Instant::now();
        for _ in 0..FRAMES {
            src.prepare(ctx, &mut rig.pbr, &frame, &balls()).unwrap();
            let mut enc = ctx
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            let binding = src.binding();
            rig.pbr.encode_reflected(&mut enc, &frame, Some(&binding));
            ctx.queue().submit([enc.finish()]);
            wait_for_device(ctx.device(), std::time::Duration::from_secs(60)).unwrap();
        }
        let ms = start.elapsed().as_secs_f64() * 1e3 / f64::from(FRAMES);
        let added = ms - *base.get_or_insert(ms);
        println!(
            "[benchmark] {} ({}): {ms:.2} ms/frame, added vs off {added:+.2} ms ({})",
            q,
            src.name(),
            ctx.adapter_info().name
        );
    }
}
