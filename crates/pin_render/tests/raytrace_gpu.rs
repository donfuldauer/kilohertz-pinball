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
    CLEARCOAT_LAYER, RAY_ATLAS_LAYERS, RAY_QUERY_FEATURE, RayCapability, RayCaps, RayJobKind,
    RayQueryConfig, RayQueryError, RayQueryReflections, ReflectionQuality,
    select_reflection_source, select_reflection_source_with,
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
    let side = RayQueryConfig::default().map_resolution;
    assert_eq!((map.width, map.height), (side, side));
    assert!(side > PROBE_RESOLUTION);
    let texel = |dir: V3| {
        let uv = pin_render::reflections::oct_encode(dir);
        let px = |u: f32| ((u * side as f32) as u32).min(side - 1);
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

// ---------------------------------------------------------------------
// (f) second bounce at mirror-like hits: plain chrome balls (no emission)
// under an emissive panel, black environment, no lights. Every colour on
// the balls comes from the panel by reflection, so ball A shows the panel
// through ball B only when B's hit is shaded with a traced mirror term.
// ---------------------------------------------------------------------

/// The panel's emitted radiance (blue) and its plane (facing down).
const PANEL_L: V3 = [0.04, 0.1, 2.0];
const PANEL_Z: f32 = 0.05;
const PANEL_HALF: f32 = 0.1;
const BLACK: Environment = Environment {
    sky: [0.0; 3],
    horizon: [0.0; 3],
    ground: [0.0; 3],
};

/// Pixels of ball A whose exact mirror path A -> B -> panel holds with
/// margins (B shrunk 15 %, panel shrunk 10 %, ball A grown 15 % as an
/// occluder of the second ray).
fn second_bounce_region() -> Vec<(u32, u32)> {
    let cam = camera();
    let mut out = Vec::new();
    for y in 0..W {
        for x in 0..W {
            let Some((p, r)) = on_ball(&cam, x, y, A) else {
                continue;
            };
            if hit_sphere(p, r, B, 0.85 * R).is_none() {
                continue;
            }
            let Some(tb) = hit_sphere(p, r, B, R) else {
                continue;
            };
            let q = add_scaled(p, r, tb);
            let nb = norm(sub(q, B));
            let k = 2.0 * dot(r, nb);
            let r2 = [r[0] - k * nb[0], r[1] - k * nb[1], r[2] - k * nb[2]];
            if r2[2] <= 1e-3 || hit_sphere(q, r2, A, 1.15 * R).is_some() {
                continue;
            }
            let hit = add_scaled(q, r2, (PANEL_Z - q[2]) / r2[2]);
            if hit[0].abs() <= 0.9 * PANEL_HALF && hit[1].abs() <= 0.9 * PANEL_HALF {
                out.push((x, y));
            }
        }
    }
    out
}

/// Renders the panel scene (ball A roughness 0.06; ball B roughness
/// `b_rough`) with the ray-query source (balls only) on its second frame.
fn render_panel_scene(ctx: &GpuContext, b_rough: f32, cutoff: f32) -> HdrImage {
    let mut rig = Rig::new(ctx);
    let cfg = RayQueryConfig {
        ray_roughness_cutoff: cutoff,
        ..RayQueryConfig::default()
    };
    let mut src = RayQueryReflections::new(
        ctx,
        &rig.pbr,
        ReflectionQuality::BallsOnly,
        &rig.meshes(),
        &cfg,
    )
    .unwrap();
    let ball = |c: V3, rough: f32| DrawItem {
        mesh: rig.sphere,
        model: scale_translation(1.0, c),
        material: ball_material([0.0; 3], rough),
        transparent: false,
        emission: 0.0,
    };
    let mut panel_model = scale_translation(2.0 * PANEL_HALF, [0.0, 0.0, PANEL_Z]);
    // Face down: flip the quad's +z normal (mirror y and z).
    for col in [1, 2] {
        for v in &mut panel_model[col][..3] {
            *v = -*v;
        }
    }
    let draws = [
        ball(A, 0.06),
        ball(B, b_rough),
        DrawItem {
            mesh: rig.quad,
            model: panel_model,
            material: MaterialParams {
                emissive: PANEL_L,
                ..MaterialParams::lambert([0.0; 3])
            },
            transparent: false,
            emission: 0.0,
        },
    ];
    let cam = camera();
    let frame = FrameDesc {
        environment: BLACK,
        ..FrameDesc::new(&cam, &[], &draws)
    };
    let mut img = None;
    for _ in 0..2 {
        src.prepare(ctx, &mut rig.pbr, &frame, &balls()).unwrap();
        let binding = src.binding();
        img = Some(
            rig.pbr
                .render_reflected_to_image(ctx, &frame, Some(&binding))
                .unwrap(),
        );
    }
    img.unwrap()
}

#[test]
fn raytrace_second_bounce_shows_the_panel_through_the_other_ball() {
    let Some(ctx) = rq_ctx("second_bounce") else {
        return;
    };
    let region = second_bounce_region();
    assert!(region.len() >= 40, "test geometry: {} px", region.len());
    let mean = |img: &HdrImage| {
        let mut m = Region::default();
        for &(x, y) in &region {
            m.add(img.at(x, y));
        }
        m.mean()
    };
    // Two chrome reflections of the panel: F_A * F_B * L with F >= the base
    // colour 0.9 (Schlick only raises it), each attenuated by at most the
    // (1 - roughness) mirror weight against the black irradiance.
    let expected = f64::from(0.9 * 0.9 * PANEL_L[2]);
    let lean = |m: [f64; 3]| (m[2] - m[0]) / expected;
    let cutoff = RayQueryConfig::default().ray_roughness_cutoff;

    // B mirror-like (chrome default and just below the cutoff): A shows the
    // panel through B.
    for b_rough in [0.06, cutoff - 0.01] {
        let img = render_panel_scene(ctx, b_rough, cutoff);
        assert!(
            img.pixels
                .iter()
                .flatten()
                .all(|c| c.is_finite() && *c >= 0.0)
        );
        save(&format!("raytrace_second_bounce_b{b_rough:.2}"), &img);
        let m = mean(&img);
        println!(
            "B roughness {b_rough}: {} px, mean {m:.4?}, lean {:.3}",
            region.len(),
            lean(m)
        );
        let min = if b_rough <= 0.1 { 0.5 } else { 0.3 };
        assert!(
            lean(m) >= min,
            "B roughness {b_rough}: lean {} < {min}",
            lean(m)
        );
        assert!(m[2] > 4.0 * m[0], "B roughness {b_rough}: {m:?}");
    }

    // B rougher than the cutoff: its hit takes the environment (black) as
    // the mirror term, so there is no second bounce and the one-bounce
    // result is black (no lights, no emission, black irradiance).
    let img = render_panel_scene(ctx, cutoff + 0.01, cutoff);
    let m = mean(&img);
    println!(
        "B roughness {}: mean {m:.4?}, lean {:.3}",
        cutoff + 0.01,
        lean(m)
    );
    for (c, v) in m.iter().enumerate() {
        assert!(
            *v <= 0.02 * expected,
            "B above the cutoff: channel {c} = {v} (one-bounce result is black)"
        );
    }

    // The cutoff is the switch: the same rough B with a cutoff above its
    // roughness gets the second bounce.
    let img = render_panel_scene(ctx, cutoff + 0.01, cutoff + 0.1);
    let m = mean(&img);
    println!(
        "B roughness {} with cutoff {}: lean {:.3}",
        cutoff + 0.01,
        cutoff + 0.1,
        lean(m)
    );
    assert!(lean(m) >= 0.3, "raised cutoff: lean {}", lean(m));
}

// ---------------------------------------------------------------------
// (g) Crisp reflected edges (DECISIONS "5.4 — ray atlas map resolution
// (visual-checklist fix)"). A reflected hard emissive edge must land where
// exact mirror geometry puts it, without the texel-sized steps a coarse
// atlas gives once the PBR pass magnifies it. Analytic measures first,
// snapshots second (D30); each scene is also rendered with the first 5.4
// atlas (128² maps, 2×2 texels per clear-coat ray, one ray per cell) as a
// negative control that the measures must reject.
// ---------------------------------------------------------------------

/// The first 5.4 atlas.
fn old_atlas() -> RayQueryConfig {
    RayQueryConfig {
        map_resolution: 128,
        clearcoat_block: 2,
        supersample: 1,
        ..RayQueryConfig::default()
    }
}

/// Unit ray through the continuous image point `(u, v)` of a `size`²
/// image (pixel `(x, y)` has its centre at `(x + 0.5, y + 0.5)`).
fn ray_at(cam: &PbrCamera, size: u32, u: f32, v: f32) -> V3 {
    let t = (0.5 * cam.fov_y).tan();
    let nx = (2.0 * u / size as f32 - 1.0) * t * cam.aspect;
    let ny = (1.0 - 2.0 * v / size as f32) * t;
    let m = &cam.view;
    let s = [m[0][0], m[1][0], m[2][0]];
    let up = [m[0][1], m[1][1], m[2][1]];
    let f = [-m[0][2], -m[1][2], -m[2][2]];
    norm([
        s[0] * nx + up[0] * ny + f[0],
        s[1] * nx + up[1] * ny + f[1],
        s[2] * nx + up[2] * ny + f[2],
    ])
}

fn luminance(p: [f32; 4]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

/// `MeshGeometry::quad(1.0)` as a `2 hx` × `2 hy` rectangle centred at
/// `c`, facing down (−z), edges along world x and y.
fn down_rect(c: V3, hx: f32, hy: f32) -> [[f32; 4]; 4] {
    [
        [2.0 * hx, 0.0, 0.0, 0.0],
        [0.0, -2.0 * hy, 0.0, 0.0],
        [0.0, 0.0, -1.0, 0.0],
        [c[0], c[1], c[2], 1.0],
    ]
}

fn emissive(l: V3) -> MaterialParams {
    MaterialParams {
        emissive: l,
        ..MaterialParams::lambert([0.0; 3])
    }
}

/// A `size`² renderer with a finely tessellated ball and the unit quad.
struct CrispRig {
    pbr: PbrRenderer,
    size: u32,
    sphere: MeshId,
    quad: MeshId,
    sphere_geo: MeshGeometry,
    quad_geo: MeshGeometry,
}

impl CrispRig {
    fn new(ctx: &GpuContext, size: u32) -> CrispRig {
        let mut pbr = PbrRenderer::new(ctx, size, size).unwrap();
        let sphere_geo = MeshGeometry::uv_sphere(R, 128, 96);
        let quad_geo = MeshGeometry::quad(1.0);
        let sphere = pbr.upload_mesh(&sphere_geo).unwrap();
        let quad = pbr.upload_mesh(&quad_geo).unwrap();
        CrispRig {
            pbr,
            size,
            sphere,
            quad,
            sphere_geo,
            quad_geo,
        }
    }

    /// One frame with a fresh ray-query source (black environment, no
    /// lights); also the wall time of prepare + render (printed only, D8).
    fn render(
        &mut self,
        ctx: &GpuContext,
        quality: ReflectionQuality,
        cfg: &RayQueryConfig,
        cam: &PbrCamera,
        draws: &[DrawItem],
        balls: &[ReflectiveBall],
    ) -> (HdrImage, f64) {
        let meshes = [(self.sphere, &self.sphere_geo), (self.quad, &self.quad_geo)];
        let mut src = RayQueryReflections::new(ctx, &self.pbr, quality, &meshes, cfg).unwrap();
        let frame = FrameDesc {
            environment: BLACK,
            ..FrameDesc::new(cam, &[], draws)
        };
        let start = std::time::Instant::now();
        src.prepare(ctx, &mut self.pbr, &frame, balls).unwrap();
        assert!(!src.probes_bound(), "the frame must be traced");
        let binding = src.binding();
        let img = self
            .pbr
            .render_reflected_to_image(ctx, &frame, Some(&binding))
            .unwrap();
        let ms = start.elapsed().as_secs_f64() * 1e3;
        assert_eq!(img.width, self.size);
        assert!(
            img.pixels
                .iter()
                .flatten()
                .all(|c| c.is_finite() && *c >= 0.0)
        );
        (img, ms)
    }
}

// --- (a) clear-coat straightness ------------------------------------------

const COAT_SIZE: u32 = 160;
const COAT_L: V3 = [8.0, 8.0, 8.0];

/// A camera above a clear-coated playfield (z = 0) and an emissive panel
/// facing down (centre, half sizes along x and y); the panel's back face is
/// culled, so the camera sees only its mirror image in the clear coat.
#[derive(Copy, Clone, Debug)]
struct CoatPose {
    eye: V3,
    target: V3,
    fov: f32,
    centre: V3,
    hx: f32,
    hy: f32,
}

fn coat_poses() -> [CoatPose; 2] {
    [
        // The spec overview camera, a tall panel.
        CoatPose {
            eye: [0.0, -0.22, 0.25],
            target: [0.0, 0.0, 0.01],
            fov: 0.62,
            centre: [0.0, 0.0, 0.032],
            hx: 0.04,
            hy: 0.1,
        },
        // Off to the side and lower: slanted edge images, a narrower panel
        // higher up.
        CoatPose {
            eye: [0.07, -0.2, 0.2],
            target: [0.0, 0.01, 0.0],
            fov: 0.55,
            centre: [0.01, 0.0, 0.05],
            hx: 0.03,
            hy: 0.11,
        },
    ]
}

impl CoatPose {
    fn camera(&self) -> PbrCamera {
        PbrCamera::look_at(
            self.eye,
            self.target,
            [0.0, 0.0, 1.0],
            self.fov,
            1.0,
            0.01,
            5.0,
        )
    }

    /// The panel-plane point the clear coat at image point `(u, v)`
    /// mirrors (the eye mirrored in z = 0, through the playfield point):
    /// `None` off the playfield.
    fn mirrored(&self, cam: &PbrCamera, u: f32, v: f32) -> Option<V3> {
        let d = ray_at(cam, COAT_SIZE, u, v);
        if d[2] >= -1e-4 {
            return None;
        }
        let q = add_scaled(cam.eye, d, -cam.eye[2] / d[2]);
        if q[0].abs() > 0.45 || q[1].abs() > 0.45 {
            return None;
        }
        let r = [d[0], d[1], -d[2]];
        Some(add_scaled(q, r, (self.centre[2] - q[2]) / r[2]))
    }

    fn draws(&self, rig: &CrispRig) -> [DrawItem; 2] {
        [
            DrawItem {
                mesh: rig.quad,
                model: scale_translation(1.0, [0.0; 3]),
                material: MaterialKind::PlayfieldWood.params(),
                transparent: false,
                emission: 0.0,
            },
            DrawItem {
                mesh: rig.quad,
                model: down_rect(self.centre, self.hx, self.hy),
                material: emissive(COAT_L),
                transparent: false,
                emission: 0.0,
            },
        ]
    }
}

/// The straightness of one reflected panel edge (`x = edge_x`, the middle
/// 80 % of its length) over the image rows that cross it.
#[derive(Copy, Clone, Debug, Default)]
struct EdgeFit {
    /// Rows with an analytic and a measured crossing.
    rows: usize,
    /// RMS and largest residual (px) of the measured crossings about their
    /// least-squares line.
    rms: f32,
    max: f32,
    /// Largest |measured − analytic| crossing (px).
    off: f32,
}

fn fit_edge(img: &HdrImage, pose: &CoatPose, edge_x: f32) -> EdgeFit {
    let cam = pose.camera();
    let n = COAT_SIZE;
    // Signed distance (m, along x) of the mirrored point from the edge, or
    // None off the playfield or outside the middle of the edge.
    let g = |u: f32, v: f32| -> Option<f32> {
        let p = pose.mirrored(&cam, u, v)?;
        ((p[1] - pose.centre[1]).abs() <= 0.8 * pose.hy).then_some(p[0] - edge_x)
    };
    let mut pts: Vec<(f32, f32, f32)> = Vec::new(); // (v, measured u, analytic u)
    for y in 0..n {
        let v = y as f32 + 0.5;
        let mut crossings = Vec::new();
        for x in 0..n - 1 {
            let (u0, u1) = (x as f32 + 0.5, x as f32 + 1.5);
            let (Some(a), Some(b)) = (g(u0, v), g(u1, v)) else {
                continue;
            };
            if (a < 0.0) != (b < 0.0) {
                let (mut lo, mut hi, a0) = (u0, u1, a);
                for _ in 0..40 {
                    let mid = 0.5 * (lo + hi);
                    match g(mid, v) {
                        Some(m) if (m < 0.0) == (a0 < 0.0) => lo = mid,
                        _ => hi = mid,
                    }
                }
                crossings.push(0.5 * (lo + hi));
            }
        }
        let [want] = crossings[..] else {
            continue;
        };
        // Measured: the 50 % level of the local luminance range, linearly
        // interpolated between pixel centres; the crossing nearest the
        // analytic one.
        const WIN: i64 = 12;
        let c = want.floor() as i64;
        let (x0, x1) = ((c - WIN).max(0), (c + WIN).min(i64::from(n) - 1));
        let l: Vec<f32> = (x0..=x1).map(|x| luminance(img.at(x as u32, y))).collect();
        let lo = l.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = l.iter().copied().fold(0.0, f32::max);
        assert!(
            hi > 0.05 && hi > 10.0 * lo,
            "row {y}: no reflected edge near u = {want} (lum {lo}..{hi})"
        );
        let level = 0.5 * (lo + hi);
        let best = l
            .windows(2)
            .enumerate()
            .filter_map(|(i, w)| {
                let (a, b) = (w[0] - level, w[1] - level);
                ((a <= 0.0) != (b <= 0.0)).then(|| (x0 + i as i64) as f32 + 0.5 + a / (a - b))
            })
            .min_by(|p, q| (p - want).abs().total_cmp(&(q - want).abs()));
        if let Some(got) = best {
            pts.push((v, got, want));
        }
    }
    let m = pts.len() as f64;
    if pts.len() < 2 {
        return EdgeFit {
            rows: pts.len(),
            ..EdgeFit::default()
        };
    }
    let (sv, su) = pts.iter().fold((0.0, 0.0), |(a, b), p| {
        (a + f64::from(p.0), b + f64::from(p.1))
    });
    let (mv, mu) = (sv / m, su / m);
    let (mut svv, mut svu) = (0.0, 0.0);
    for p in &pts {
        let dv = f64::from(p.0) - mv;
        svv += dv * dv;
        svu += dv * (f64::from(p.1) - mu);
    }
    let slope = svu / svv;
    let res: Vec<f64> = pts
        .iter()
        .map(|p| f64::from(p.1) - (mu + slope * (f64::from(p.0) - mv)))
        .collect();
    EdgeFit {
        rows: pts.len(),
        rms: (res.iter().map(|r| r * r).sum::<f64>() / m).sqrt() as f32,
        max: res.iter().fold(0.0f64, |a, r| a.max(r.abs())) as f32,
        off: pts.iter().fold(0.0f32, |a, p| a.max((p.1 - p.2).abs())),
    }
}

/// Renders `pose` with `cfg` and fits both edges along y.
fn coat_edges(
    ctx: &GpuContext,
    rig: &mut CrispRig,
    pose: &CoatPose,
    cfg: &RayQueryConfig,
) -> (HdrImage, [EdgeFit; 2], f64) {
    let draws = pose.draws(rig);
    let (img, ms) = rig.render(
        ctx,
        ReflectionQuality::BallsAndClearCoat,
        cfg,
        &pose.camera(),
        &draws,
        &[],
    );
    let fits = [-1.0, 1.0].map(|s| fit_edge(&img, pose, pose.centre[0] + s * pose.hx));
    (img, fits, ms)
}

#[test]
fn raytrace_crisp_clear_coat_edges_are_straight() {
    let Some(ctx) = rq_ctx("crisp_clear_coat_edges") else {
        return;
    };
    let mut rig = CrispRig::new(ctx, COAT_SIZE);
    let def = RayQueryConfig::default();
    let mut first = None;
    for (i, pose) in coat_poses().iter().enumerate() {
        let (img, new, ms_new) = coat_edges(ctx, &mut rig, pose, &def);
        let (old_img, old, ms_old) = coat_edges(ctx, &mut rig, pose, &old_atlas());
        save(&format!("raytrace_crisp_clearcoat_pose{i}"), &img);
        save(
            &format!("raytrace_crisp_clearcoat_pose{i}_old_atlas"),
            &old_img,
        );
        println!(
            "[crisp_clear_coat] pose {i}: default {:?} ({ms_new:.0} ms); old atlas {:?} \
             ({ms_old:.0} ms)",
            new, old
        );
        assert!(
            new.iter().any(|f| f.rows >= 60),
            "pose {i}: the test needs one edge over >= 60 rows: {new:?}"
        );
        let measured = |fits: &[EdgeFit; 2]| -> Vec<EdgeFit> {
            fits.iter().copied().filter(|f| f.rows >= 30).collect()
        };
        // Measured on lavapipe at the default: worst edge RMS 0.29 px, max
        // 0.67 px, 0.70 px from the analytic edge; the first atlas: RMS
        // 3.15 / 4.19 px, max 6.6 / 8.3 px (DECISIONS "5.4 — ray atlas map
        // resolution (visual-checklist fix)").
        for (k, f) in measured(&new).iter().enumerate() {
            assert!(
                f.rms <= 0.35,
                "pose {i} edge {k}: RMS residual {} px",
                f.rms
            );
            assert!(f.max <= 1.0, "pose {i} edge {k}: max residual {} px", f.max);
            assert!(
                f.off <= 1.5,
                "pose {i} edge {k}: {} px from the analytic reflection",
                f.off
            );
        }
        // (c) the first 5.4 atlas fails the same measure clearly: its worst
        // edge's RMS residual is at least twice the new worst one (an edge
        // that happens to run along its texel grid stays straight there,
        // only displaced, so the comparison is per pose).
        let worst = |fits: &[EdgeFit]| fits.iter().map(|f| f.rms).fold(0.0f32, f32::max);
        let (w_new, w_old) = (worst(&measured(&new)), worst(&measured(&old)));
        assert!(
            w_old >= 2.0 * w_new,
            "pose {i}: old atlas worst RMS {w_old} not >= 2x the new {w_new}"
        );
        first.get_or_insert(img);
    }
    check_snapshot(ctx, "crisp_clearcoat", &first.unwrap());
}

// --- (b) ball outline fidelity ----------------------------------------------

const BALL_SIZE: u32 = 128;
const BALL_C: V3 = [0.0, 0.0, R];
const BALL_PANEL_Z: f32 = 0.06;
const BALL_L: V3 = [4.0, 4.0, 4.0];
/// Lower bound of chrome's mirror reflectance in this scene: base colour
/// 0.9 (Schlick only raises it) times the 0.94 mirror weight of
/// roughness 0.06 against the black irradiance.
const BALL_MIRROR: f32 = 0.9 * 0.94;

/// A camera 250 mm from the ball along `dir`, zoomed (`fov`) on the point
/// whose mirror ray goes up into the panel's straight edge; the panel
/// (facing down, 60 mm up) lies on one side of that edge.
#[derive(Copy, Clone, Debug)]
struct BallPose {
    dir: V3,
    fov: f32,
    /// `true`: the edge runs along world x (panel on the +y side);
    /// `false`: along world y (panel on the −x side).
    edge_along_x: bool,
}

fn ball_poses() -> [BallPose; 2] {
    [
        BallPose {
            dir: norm([0.25, -1.0, 1.0]),
            fov: 0.03,
            edge_along_x: false,
        },
        BallPose {
            dir: norm([0.35, -0.7, 0.65]),
            fov: 0.04,
            edge_along_x: true,
        },
    ]
}

struct BallScene {
    cam: PbrCamera,
    centre: V3,
    hx: f32,
    hy: f32,
}

impl BallPose {
    fn scene(&self) -> BallScene {
        let eye = add_scaled(BALL_C, self.dir, 0.25);
        let n_mid = norm([self.dir[0], self.dir[1], self.dir[2] + 1.0]);
        let s = add_scaled(BALL_C, n_mid, R);
        let d = norm(sub(s, eye));
        let k = 2.0 * dot(d, n_mid);
        let r = [
            d[0] - k * n_mid[0],
            d[1] - k * n_mid[1],
            d[2] - k * n_mid[2],
        ];
        let p = add_scaled(s, r, (BALL_PANEL_Z - s[2]) / r[2]);
        let (centre, hx, hy) = if self.edge_along_x {
            ([p[0], p[1] + 0.15, BALL_PANEL_Z], 0.3, 0.15)
        } else {
            ([p[0] - 0.15, p[1], BALL_PANEL_Z], 0.15, 0.3)
        };
        let cam = PbrCamera::look_at(eye, s, [0.0, 0.0, 1.0], self.fov, 1.0, 0.01, 5.0);
        BallScene {
            cam,
            centre,
            hx,
            hy,
        }
    }
}

impl BallScene {
    /// Pixel `(x, y)`: `None` off the ball's disc (3 % rim margin), else
    /// whether the exact mirror ray hits the panel.
    fn analytic(&self, x: u32, y: u32) -> Option<bool> {
        let d = ray_at(&self.cam, BALL_SIZE, x as f32 + 0.5, y as f32 + 0.5);
        hit_sphere(self.cam.eye, d, BALL_C, 0.97 * R)?;
        let t = hit_sphere(self.cam.eye, d, BALL_C, R)?;
        let p = add_scaled(self.cam.eye, d, t);
        let n = norm(sub(p, BALL_C));
        let k = 2.0 * dot(d, n);
        let r = [d[0] - k * n[0], d[1] - k * n[1], d[2] - k * n[2]];
        if r[2] <= 1e-6 {
            return Some(false);
        }
        let q = add_scaled(p, r, (BALL_PANEL_Z - p[2]) / r[2]);
        Some((q[0] - self.centre[0]).abs() <= self.hx && (q[1] - self.centre[1]).abs() <= self.hy)
    }
}

/// Pixel classes over the disc and the outline measures.
#[derive(Copy, Clone, Debug, Default)]
struct OutlineFit {
    disc: usize,
    mismatched: usize,
    /// Mismatched pixels over disc pixels.
    fraction: f64,
    /// Mean distance (px) from each traced boundary pixel to the nearest
    /// analytic boundary pixel.
    boundary_mean: f64,
    boundary_px: usize,
}

fn outline_fit(img: &HdrImage, scene: &BallScene) -> OutlineFit {
    let n = BALL_SIZE as i64;
    let thr = 0.5 * BALL_MIRROR * luminance([BALL_L[0], BALL_L[1], BALL_L[2], 1.0]);
    let idx = |x: i64, y: i64| (y * n + x) as usize;
    let mut want = vec![None; (n * n) as usize];
    let mut got = vec![false; (n * n) as usize];
    for y in 0..n {
        for x in 0..n {
            want[idx(x, y)] = scene.analytic(x as u32, y as u32);
            got[idx(x, y)] = luminance(img.at(x as u32, y as u32)) >= thr;
        }
    }
    let boundary = |class: &dyn Fn(usize) -> bool| -> Vec<(i64, i64)> {
        let mut out = Vec::new();
        for y in 0..n {
            for x in 0..n {
                if want[idx(x, y)].is_none() {
                    continue;
                }
                let c = class(idx(x, y));
                let edge = [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| {
                    let (u, v) = (x + dx, y + dy);
                    (0..n).contains(&u)
                        && (0..n).contains(&v)
                        && want[idx(u, v)].is_some()
                        && class(idx(u, v)) != c
                });
                if edge {
                    out.push((x, y));
                }
            }
        }
        out
    };
    let a_b = boundary(&|i| want[i] == Some(true));
    let t_b = boundary(&|i| got[i]);
    let disc = want.iter().filter(|w| w.is_some()).count();
    let mismatched = want
        .iter()
        .zip(&got)
        .filter(|(w, g)| w.is_some_and(|w| w != **g))
        .count();
    let boundary_mean = t_b
        .iter()
        .map(|&(x, y)| {
            a_b.iter()
                .map(|&(u, v)| (((x - u) * (x - u) + (y - v) * (y - v)) as f64).sqrt())
                .fold(f64::INFINITY, f64::min)
        })
        .sum::<f64>()
        / t_b.len().max(1) as f64;
    OutlineFit {
        disc,
        mismatched,
        fraction: mismatched as f64 / disc.max(1) as f64,
        boundary_mean,
        boundary_px: t_b.len(),
    }
}

fn ball_outline(
    ctx: &GpuContext,
    rig: &mut CrispRig,
    pose: &BallPose,
    cfg: &RayQueryConfig,
) -> (HdrImage, OutlineFit, f64) {
    let scene = pose.scene();
    let draws = [
        DrawItem {
            mesh: rig.sphere,
            model: scale_translation(1.0, BALL_C),
            material: ball_material([0.0; 3], 0.06),
            transparent: false,
            emission: 0.0,
        },
        DrawItem {
            mesh: rig.quad,
            model: down_rect(scene.centre, scene.hx, scene.hy),
            material: emissive(BALL_L),
            transparent: false,
            emission: 0.0,
        },
    ];
    let ball = [ReflectiveBall {
        position: BALL_C,
        radius: R,
        draw: 0,
    }];
    let (img, ms) = rig.render(
        ctx,
        ReflectionQuality::BallsOnly,
        cfg,
        &scene.cam,
        &draws,
        &ball,
    );
    let fit = outline_fit(&img, &scene);
    (img, fit, ms)
}

#[test]
fn raytrace_crisp_ball_outline_follows_the_analytic_reflection() {
    let Some(ctx) = rq_ctx("crisp_ball_outline") else {
        return;
    };
    let mut rig = CrispRig::new(ctx, BALL_SIZE);
    let def = RayQueryConfig::default();
    let mut first = None;
    for (i, pose) in ball_poses().iter().enumerate() {
        let (img, new, ms_new) = ball_outline(ctx, &mut rig, pose, &def);
        let (old_img, old, ms_old) = ball_outline(ctx, &mut rig, pose, &old_atlas());
        save(&format!("raytrace_crisp_ball_pose{i}"), &img);
        save(&format!("raytrace_crisp_ball_pose{i}_old_atlas"), &old_img);
        println!(
            "[crisp_ball] pose {i}: default {new:?} ({ms_new:.0} ms); old atlas {old:?} \
             ({ms_old:.0} ms)"
        );
        let px = (BALL_SIZE * BALL_SIZE) as usize;
        assert!(new.disc >= px / 2, "pose {i}: disc {} px", new.disc);
        assert!(
            new.boundary_px >= BALL_SIZE as usize / 2,
            "pose {i}: the outline must cross the view ({} px)",
            new.boundary_px
        );
        // Measured on lavapipe at the default: at most 0.0005 of the disc
        // and 0.031 px; the first atlas: 0.0063 / 0.43 px and 0.0038 /
        // 0.24 px (DECISIONS "5.4 — ray atlas map resolution
        // (visual-checklist fix)").
        assert!(
            new.fraction <= 0.002,
            "pose {i}: {:.4} of the disc mismatched",
            new.fraction
        );
        assert!(
            new.boundary_mean <= 0.2,
            "pose {i}: mean boundary distance {} px",
            new.boundary_mean
        );
        // (c) the first 5.4 atlas is measurably worse.
        assert!(
            old.fraction >= 2.0 * new.fraction && old.mismatched >= 2 * new.mismatched + 20,
            "pose {i}: old atlas mismatch {old:?} not clearly above the new {new:?}"
        );
        assert!(
            old.boundary_mean >= 2.0 * new.boundary_mean,
            "pose {i}: old atlas boundary distance {} not >= 2x the new {}",
            old.boundary_mean,
            new.boundary_mean
        );
        first.get_or_insert(img);
    }
    check_snapshot(ctx, "crisp_ball", &first.unwrap());
}

/// (c) The config reproduces the first 5.4 atlas: 128² layers, and the
/// clear-coat layer in 2×2 blocks of one ray.
#[test]
fn raytrace_old_atlas_config_reproduces_the_first_atlas() {
    let Some(ctx) = rq_ctx("old_atlas_config") else {
        return;
    };
    let mut rig = Rig::new(ctx);
    let mut src = RayQueryReflections::new(
        ctx,
        &rig.pbr,
        ReflectionQuality::BallsAndClearCoat,
        &rig.meshes(),
        &old_atlas(),
    )
    .unwrap();
    let draws = rig.draws(0.06, 0.06);
    rig.render(ctx, &mut src, &draws);
    let coat = src.read_layer(ctx, CLEARCOAT_LAYER).unwrap();
    assert_eq!((coat.width, coat.height), (128, 128));
    for by in 0..64 {
        for bx in 0..64 {
            let p = coat.at(2 * bx, 2 * by);
            for (dx, dy) in [(1, 0), (0, 1), (1, 1)] {
                assert_eq!(coat.at(2 * bx + dx, 2 * by + dy), p, "block {bx},{by}");
            }
        }
    }
    let ball = src.read_layer(ctx, 0).unwrap();
    assert_eq!((ball.width, ball.height), (128, 128));
    // The default traces every clear-coat texel: neighbours differ somewhere
    // inside a 2×2 block.
    let mut src = ray_source(ctx, &rig, ReflectionQuality::BallsAndClearCoat);
    rig.render(ctx, &mut src, &draws);
    let coat = src.read_layer(ctx, CLEARCOAT_LAYER).unwrap();
    let n = RayQueryConfig::default().map_resolution;
    assert_eq!((coat.width, coat.height), (n, n));
    let split = (0..n / 2)
        .flat_map(|by| (0..n / 2).map(move |bx| (bx, by)))
        .any(|(bx, by)| coat.at(2 * bx, 2 * by) != coat.at(2 * bx + 1, 2 * by));
    assert!(split, "default clear coat is not traced per texel");
}

/// An unusable map resolution is declined with the reason, never a panic.
#[test]
fn raytrace_new_declines_an_unusable_map_resolution() {
    let Some(ctx) = rq_ctx("declines_unusable_map_resolution") else {
        return;
    };
    let rig = Rig::new(ctx);
    let max = ctx.device().limits().max_texture_dimension_2d;
    for n in [0, 100, max.next_power_of_two() * 2] {
        let cfg = RayQueryConfig {
            map_resolution: n,
            ..RayQueryConfig::default()
        };
        let e = RayQueryReflections::new(
            ctx,
            &rig.pbr,
            ReflectionQuality::BallsOnly,
            &rig.meshes(),
            &cfg,
        )
        .err()
        .unwrap();
        assert!(matches!(e, RayQueryError::InvalidConfig(_)), "{e}");
        assert!(e.to_string().contains("map_resolution"), "{e}");
        // The selector falls back to probes.
        let src = select_reflection_source(
            ctx,
            ReflectionQuality::BallsOnly,
            &rig.pbr,
            &rig.meshes(),
            &cfg,
        );
        assert_eq!(src.name(), "octahedral-probes");
    }
}

// ---------------------------------------------------------------------
// (h) Mixed roughness (CodeRabbit PR #22 finding 2): a ball above the
// cutoff in a frame that traces another ball gets the probe-equivalent
// one-bounce map, never the second bounce at mirror-like hits.
// ---------------------------------------------------------------------

/// `PIN_REQUIRE_RAY_QUERY=1`: a device without ray queries fails the test.
fn require_ray_query() -> bool {
    std::env::var("PIN_REQUIRE_RAY_QUERY").is_ok_and(|v| v.trim() == "1")
}

/// The mixed scene: chrome ball A (roughness 0.06, at or below the cutoff:
/// traced as BallView) and rough ball B (above the cutoff: BallCentre), no
/// lights, black environment. `bounce`: A plain, plus the emissive panel
/// above (B sees the panel through mirror-like A only by a second bounce).
/// Otherwise A emits the panel radiance itself and there is no panel (B
/// sees A directly; a second bounce at A finds only the black environment
/// or unlit, non-emissive B, so it adds nothing).
fn mixed_draws(rig: &Rig, bounce: bool, b_rough: f32) -> Vec<DrawItem> {
    let ball = |c: V3, e: V3, rough: f32| DrawItem {
        mesh: rig.sphere,
        model: scale_translation(1.0, c),
        material: ball_material(e, rough),
        transparent: false,
        emission: 0.0,
    };
    if !bounce {
        return vec![ball(A, PANEL_L, 0.06), ball(B, [0.0; 3], b_rough)];
    }
    let mut panel_model = scale_translation(2.0 * PANEL_HALF, [0.0, 0.0, PANEL_Z]);
    // Face down: flip the quad's +z normal (mirror y and z).
    for col in [1, 2] {
        for v in &mut panel_model[col][..3] {
            *v = -*v;
        }
    }
    vec![
        ball(A, [0.0; 3], 0.06),
        ball(B, [0.0; 3], b_rough),
        DrawItem {
            mesh: rig.quad,
            model: panel_model,
            material: MaterialParams {
                emissive: PANEL_L,
                ..MaterialParams::lambert([0.0; 3])
            },
            transparent: false,
            emission: 0.0,
        },
    ]
}

/// Renders the mixed scene `frames` times with `source`, returning the last
/// image.
fn render_mixed(
    ctx: &GpuContext,
    rig: &mut Rig,
    source: &mut dyn ReflectionSource,
    draws: &[DrawItem],
    frames: usize,
) -> HdrImage {
    let cam = camera();
    let frame = FrameDesc {
        environment: BLACK,
        ..FrameDesc::new(&cam, &[], draws)
    };
    let mut img = None;
    for _ in 0..frames {
        source.prepare(ctx, &mut rig.pbr, &frame, &balls()).unwrap();
        let binding = source.binding();
        img = Some(
            rig.pbr
                .render_reflected_to_image(ctx, &frame, Some(&binding))
                .unwrap(),
        );
    }
    img.unwrap()
}

/// Pixels of ball B whose reflected direction, traced from B's centre
/// (what B's probe-equivalent map stores), hits ball A (shrunk 15 %).
fn rough_ball_towards_a() -> Vec<(u32, u32)> {
    let cam = camera();
    let mut out = Vec::new();
    for y in 0..W {
        for x in 0..W {
            let Some((_, r)) = on_ball(&cam, x, y, B) else {
                continue;
            };
            if hit_sphere(B, r, A, 0.85 * R).is_some() {
                out.push((x, y));
            }
        }
    }
    out
}

/// All pixels of ball A (6 % rim margin, not hidden behind B).
fn chrome_ball_pixels() -> Vec<(u32, u32)> {
    let cam = camera();
    let mut out = Vec::new();
    for y in 0..W {
        for x in 0..W {
            if on_ball(&cam, x, y, A).is_some() {
                out.push((x, y));
            }
        }
    }
    out
}

fn region_mean(img: &HdrImage, px: &[(u32, u32)]) -> [f64; 3] {
    let mut m = Region::default();
    for &(x, y) in px {
        m.add(img.at(x, y));
    }
    m.mean()
}

/// Largest per-channel difference of the region means.
fn mean_diff(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0, f64::max)
}

/// Mean per-pixel absolute difference (all channels) over `px`.
fn pixel_diff(a: &HdrImage, b: &HdrImage, px: &[(u32, u32)]) -> f64 {
    let mut sum = 0.0;
    for &(x, y) in px {
        let (p, q) = (a.at(x, y), b.at(x, y));
        for c in 0..3 {
            sum += f64::from((p[c] - q[c]).abs());
        }
    }
    sum / (3 * px.len().max(1)) as f64
}

/// Ray-path and probe-path images of the mixed scene.
fn mixed_pair(ctx: &GpuContext, bounce: bool, b_rough: f32) -> (HdrImage, HdrImage) {
    let mut rig = Rig::new(ctx);
    let draws = mixed_draws(&rig, bounce, b_rough);
    let mut src = ray_source(ctx, &rig, ReflectionQuality::BallsOnly);
    let ray = render_mixed(ctx, &mut rig, &mut src, &draws, 2);
    assert_eq!(src.name(), "ray_query");
    assert!(!src.probes_bound(), "the mixed frame must trace");
    let kinds: Vec<_> = src.plan().jobs.iter().map(|j| (j.layer, j.kind)).collect();
    assert!(
        matches!(
            kinds[..],
            [
                (0, RayJobKind::BallView { .. }),
                (1, RayJobKind::BallCentre { .. })
            ]
        ),
        "plan {kinds:?}"
    );
    let mut rig = Rig::new(ctx);
    let mut probe = probes(ctx, &rig);
    // Quality Off: the probe path, settled over the capture budget.
    let want = render_mixed(ctx, &mut rig, &mut probe, &draws, 3);
    (ray, want)
}

#[test]
fn raytrace_mixed_roughness_rough_ball_equals_probe_output() {
    let Some(ctx) = rq_ctx("mixed_roughness") else {
        assert!(
            !require_ray_query(),
            "[mixed_roughness] PIN_REQUIRE_RAY_QUERY=1 but no ray-query device"
        );
        return;
    };
    let cutoff = RayQueryConfig::default().ray_roughness_cutoff;
    let b_rough = cutoff + 0.05;
    let towards_a = rough_ball_towards_a();
    let chrome = chrome_ball_pixels();
    assert!(
        towards_a.len() >= 40,
        "test geometry: {} px",
        towards_a.len()
    );
    assert!(chrome.len() >= 200, "test geometry: {} px", chrome.len());

    // Baseline: B sees emissive A directly; nothing for a second bounce to
    // find. The ray and probe maps differ only in sampling (map
    // resolution, rasterised capture vs rays).
    let (ray0, probe0) = mixed_pair(ctx, false, b_rough);
    save("raytrace_mixed_baseline_ray", &ray0);
    save("raytrace_mixed_baseline_probe", &probe0);
    let (r0, p0) = (
        region_mean(&ray0, &towards_a),
        region_mean(&probe0, &towards_a),
    );
    let base = mean_diff(r0, p0);
    // B shows A's emission there in both paths (the region is meaningful).
    assert!(
        p0[2] > 0.2 && r0[2] > 0.2,
        "baseline: A not seen in B: ray {r0:?} probe {p0:?}"
    );
    let tol = (3.0 * base).max(0.01);

    // The scene under test: A plain, the panel above. A probe capture shades
    // A with the (black) environment as its mirror term, so B's region
    // towards A is dark; a second bounce would show the panel there.
    let (ray, probe) = mixed_pair(ctx, true, b_rough);
    save("raytrace_mixed_bounce_ray", &ray);
    save("raytrace_mixed_bounce_probe", &probe);
    assert!(
        ray.pixels
            .iter()
            .flatten()
            .all(|c| c.is_finite() && *c >= 0.0)
    );
    let (r1, p1) = (
        region_mean(&ray, &towards_a),
        region_mean(&probe, &towards_a),
    );
    let diff = mean_diff(r1, p1);
    let chrome_diff = pixel_diff(&ray, &probe, &chrome);
    println!(
        "[mixed_roughness] B roughness {b_rough}, region towards A {} px: \
         baseline ray {r0:.4?} probe {p0:.4?} diff {base:.4}; tolerance {tol:.4}; \
         bounce scene ray {r1:.4?} probe {p1:.4?} diff {diff:.4}; \
         chrome ball A ({} px) mean |ray - probe| {chrome_diff:.4}",
        towards_a.len(),
        chrome.len()
    );
    assert!(
        diff <= tol,
        "rough ball B differs from the probe output towards mirror-like A: \
         ray {r1:?} probe {p1:?} (diff {diff} > tolerance {tol}; a second bounce?)"
    );
    // The ray path is active: chrome A (traced from its surface, with
    // parallax) differs from its probe image. The images are deterministic,
    // so any difference well above rounding shows the traced map is bound.
    assert!(
        chrome_diff > 1e-3,
        "chrome ball A matches the probe image (mean |ray - probe| {chrome_diff})"
    );
}
