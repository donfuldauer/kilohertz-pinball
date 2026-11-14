//! Independent GPU spec tests for Subphase 5.2 (ball reflection probes and HDR post), under
//! the image test policy D30: analytic pixel assertions first; a regression snapshot is
//! compared (and can only be stored) after the analytic assertions of the same image pass.
//!
//! Written from the brief and the published contract (DECISIONS "5.2 — Reflection probes
//! and post public API") without reading the implementation.
//!
//! Scene convention (as in the 5.1 spec tests): the camera sits on the +X axis looking at
//! the origin with +Z up, so image right = world +Y and image up = world +Z. Each pixel's
//! surface point, normal and mirror direction are computed here by ray casting, which gives
//! the *expected reflection region* of an emitter analytically.
//!
//! Snapshots: `tests/fixtures/spec_5_2/<name>.rgba8` (raw display RGBA8). Set
//! `SPEC_5_2_BLESS=1` to (re)write them; they are written only after the analytic checks.
#![cfg(feature = "gpu-tests")]
#![allow(clippy::needless_range_loop)] // channel/axis indices read clearer as ranges

use pin_render::gpu::{GpuContext, GpuOptions, require_gpu};
use pin_render::pbr::math::{Mat4, PbrCamera, translation};
use pin_render::pbr::{
    DrawItem, Environment, FrameDesc, HdrImage, MaterialKind, MaterialParams, MeshGeometry, MeshId,
    PbrRenderer, PointLight,
};
use pin_render::post::{
    AaMode, BloomConfig, FrameRenderer, HdrInput, LdrImage, MotionBall, MotionBlurConfig,
    PostChain, PostConfig, PostFrame, SceneBall, ToneMap, aces_film, bloom_prefilter,
    screen_motion,
};
use pin_render::reflections::{
    OctahedralProbes, PROBE_BUDGET, PROBE_RESOLUTION, ProbeConfig, ReflectionSource,
    ReflectiveBall, oct_decode,
};
use std::f32::consts::PI;

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale(a: V3, s: f32) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}
fn norm(a: V3) -> V3 {
    scale(a, 1.0 / len(a))
}
fn lum3(p: V3) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}
fn lum(p: [f32; 4]) -> f32 {
    lum3([p[0], p[1], p[2]])
}

fn gpu() -> Option<GpuContext> {
    match GpuContext::new_headless(&GpuOptions::from_env()) {
        Ok(ctx) => Some(ctx),
        Err(e) => {
            if require_gpu() {
                panic!("PIN_REQUIRE_GPU=1 but no GPU context: {e}");
            }
            println!("SKIP (no GPU adapter, PIN_REQUIRE_GPU not set): {e}");
            None
        }
    }
}

fn assert_finite(img: &HdrImage, what: &str) {
    for (i, p) in img.pixels.iter().enumerate() {
        assert!(
            p.iter().all(|c| c.is_finite()),
            "{what}: pixel {i} is not finite: {p:?} (NaN/Inf)"
        );
        assert!(
            p[0] >= 0.0 && p[1] >= 0.0 && p[2] >= 0.0,
            "{what}: pixel {i} is negative: {p:?}"
        );
    }
}

fn srgb8(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let s = if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    };
    s * 255.0
}

/// Compares (or with `SPEC_5_2_BLESS=1` stores) a display-image snapshot. Secondary
/// evidence only: call after the analytic assertions of the same image.
fn snapshot(ctx: &GpuContext, name: &str, img: &LdrImage) {
    let bytes = img.to_rgba8();
    assert_eq!(bytes.len(), (img.width * img.height * 4) as usize);
    let dir = format!("{}/tests/fixtures/spec_5_2", env!("CARGO_MANIFEST_DIR"));
    let path = format!("{dir}/{name}_{}x{}.rgba8", img.width, img.height);
    if std::env::var("SPEC_5_2_BLESS").as_deref() == Ok("1") {
        std::fs::create_dir_all(&dir).expect("fixture dir");
        std::fs::write(&path, &bytes).expect("write snapshot");
        println!("blessed {path}");
        return;
    }
    let want = std::fs::read(&path).unwrap_or_else(|e| {
        panic!("snapshot {path} missing ({e}); run once with SPEC_5_2_BLESS=1")
    });
    assert_eq!(want.len(), bytes.len(), "snapshot {name}: size changed");
    // Per-backend tolerance: stored on lavapipe (llvmpipe); other rasterisers differ at
    // silhouettes, in MSAA sample positions and in the last bits of the shading.
    let info = ctx.adapter_info();
    let same = info.name.to_lowercase().contains("llvmpipe");
    let (max_mean, max_outliers) = if same { (0.5, 0.005) } else { (2.5, 0.04) };
    let mut sum = 0u64;
    let mut outliers = 0usize;
    for (a, b) in bytes.iter().zip(&want) {
        let d = a.abs_diff(*b) as u64;
        sum += d;
        if d > 12 {
            outliers += 1;
        }
    }
    let mean = sum as f64 / bytes.len() as f64;
    let frac = outliers as f64 / bytes.len() as f64;
    assert!(
        mean <= max_mean && frac <= max_outliers,
        "snapshot {name} on {}: mean abs diff {mean:.3} (max {max_mean}), {:.2} % channels off by > 12 (max {:.2} %)",
        info.name,
        frac * 100.0,
        max_outliers * 100.0
    );
}

/// Fraction of pixels whose three colour channels are all at the 8-bit ceiling.
fn clipped_fraction(img: &LdrImage) -> f64 {
    let n = img
        .pixels
        .iter()
        .filter(|p| p[0] == 255 && p[1] == 255 && p[2] == 255)
        .count();
    n as f64 / img.pixels.len() as f64
}

// ---------------------------------------------------------------------------------------
// Camera and analytic geometry
// ---------------------------------------------------------------------------------------

/// A pinhole camera on the +X axis looking at the origin, z up, square image.
#[derive(Copy, Clone)]
struct Cam {
    eye_x: f32,
    fov_y: f32,
    size: u32,
}

impl Cam {
    fn pbr(&self) -> PbrCamera {
        PbrCamera::look_at(
            [self.eye_x, 0.0, 0.0],
            [0.0; 3],
            [0.0, 0.0, 1.0],
            self.fov_y,
            1.0,
            0.05,
            10.0,
        )
    }
    fn eye(&self) -> V3 {
        [self.eye_x, 0.0, 0.0]
    }
    /// Unit ray through the centre of pixel (x, y) (sub-pixel coordinates allowed).
    fn ray(&self, x: f32, y: f32) -> V3 {
        let t = (self.fov_y * 0.5).tan();
        let ndc_x = x / self.size as f32 * 2.0 - 1.0;
        let ndc_y = 1.0 - y / self.size as f32 * 2.0;
        norm([-1.0, ndc_x * t, ndc_y * t])
    }
    fn px_per_m_at_origin(&self) -> f32 {
        self.size as f32 * 0.5 / (self.eye_x * (self.fov_y * 0.5).tan())
    }
    /// Pixel position of a world point.
    fn project(&self, p: V3) -> [f32; 2] {
        let t = (self.fov_y * 0.5).tan();
        let depth = self.eye_x - p[0];
        let h = self.size as f32 * 0.5;
        [h + p[1] / (depth * t) * h, h - p[2] / (depth * t) * h]
    }
}

#[derive(Copy, Clone)]
struct Hit {
    p: V3,
    n: V3,
    /// Mirror direction of the view ray.
    r: V3,
}

/// What pixel (x, y) sees of a sphere at `c` with radius `rad`.
fn hit_sphere(cam: &Cam, x: u32, y: u32, c: V3, rad: f32) -> Option<Hit> {
    let d = cam.ray(x as f32 + 0.5, y as f32 + 0.5);
    let o = sub(cam.eye(), c);
    let b = dot(o, d);
    let cc = dot(o, o) - rad * rad;
    let disc = b * b - cc;
    if disc <= 0.0 {
        return None;
    }
    let s = -b - disc.sqrt();
    let local = add(o, scale(d, s));
    let n = norm(local);
    let r = sub(d, scale(n, 2.0 * dot(d, n)));
    Some(Hit {
        p: add(local, c),
        n,
        r,
    })
}

/// A square emitter: centre, unit normal, in-plane axes, half side.
#[derive(Copy, Clone)]
struct Quad {
    c: V3,
    n: V3,
    e1: V3,
    e2: V3,
    half: f32,
}

impl Quad {
    /// A square of side `size` centred at `c` whose front face looks at `target`.
    fn facing(c: V3, target: V3, size: f32) -> Quad {
        let n = norm(sub(target, c));
        let a = if n[2].abs() < 0.9 {
            [0.0, 0.0, 1.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        let e1 = norm(cross(a, n));
        let e2 = cross(n, e1);
        Quad {
            c,
            n,
            e1,
            e2,
            half: size * 0.5,
        }
    }
    /// Object → world for `MeshGeometry::quad` (z = 0 plane, facing +z): a rotation
    /// (e1, e2, n is right-handed) and a translation.
    fn model(&self) -> Mat4 {
        [
            [self.e1[0], self.e1[1], self.e1[2], 0.0],
            [self.e2[0], self.e2[1], self.e2[2], 0.0],
            [self.n[0], self.n[1], self.n[2], 0.0],
            [self.c[0], self.c[1], self.c[2], 1.0],
        ]
    }
    /// Does the ray (o, d) hit the front face within `frac` of the half side?
    fn hit(&self, o: V3, d: V3, frac: f32) -> bool {
        let denom = dot(d, self.n);
        if denom > -1e-4 {
            return false;
        }
        let t = dot(sub(self.c, o), self.n) / denom;
        if t <= 0.0 {
            return false;
        }
        let l = sub(add(o, scale(d, t)), self.c);
        dot(l, self.e1).abs() < frac * self.half && dot(l, self.e2).abs() < frac * self.half
    }
}

/// Classifies the pixels of a mirror ball at `c` against the emitter `q`.
///
/// * `inside`: the mirror ray hits the middle of the emitter both from the surface point
///   (exact) and from the ball centre (what a probe captured at the centre sees), so any
///   correct probe or ray implementation shows the emitter there.
/// * `outside`: the mirror ray misses a 1.6× enlarged emitter from both origins, and the
///   pixel is ≥ 2 px inside the silhouette.
struct Regions {
    inside: Vec<(u32, u32)>,
    outside: Vec<(u32, u32)>,
}

fn regions(cam: &Cam, c: V3, rad: f32, q: &Quad) -> Regions {
    let mut out = Regions {
        inside: Vec::new(),
        outside: Vec::new(),
    };
    for y in 0..cam.size {
        for x in 0..cam.size {
            let Some(h) = hit_sphere(cam, x, y, c, rad) else {
                continue;
            };
            // Skip the rim (grazing angles, silhouette coverage).
            if dot(h.n, scale(cam.ray(x as f32 + 0.5, y as f32 + 0.5), -1.0)) < 0.35 {
                continue;
            }
            let (a, b) = (q.hit(h.p, h.r, 0.55), q.hit(c, h.r, 0.55));
            let (a2, b2) = (q.hit(h.p, h.r, 1.6), q.hit(c, h.r, 1.6));
            if a && b {
                out.inside.push((x, y));
            } else if !a2 && !b2 {
                out.outside.push((x, y));
            }
        }
    }
    out
}

fn mean_rgb(img: &HdrImage, pts: &[(u32, u32)]) -> V3 {
    assert!(!pts.is_empty(), "empty region");
    let mut s = [0.0f64; 3];
    for &(x, y) in pts {
        let p = img.at(x, y);
        for c in 0..3 {
            s[c] += f64::from(p[c]);
        }
    }
    s.map(|v| (v / pts.len() as f64) as f32)
}

fn emitter(rgb: V3) -> MaterialParams {
    MaterialParams {
        emissive: rgb,
        ..MaterialParams::lambert([0.0; 3])
    }
}

const BALL_R: f32 = 0.05;
const CAM: Cam = Cam {
    eye_x: 0.6,
    fov_y: 0.25,
    size: 128,
};
/// Base colour of the red insert; lit, it emits `base · lamp_emissive · level`.
const RED: [f32; 4] = [1.0, 0.05, 0.02, 1.0];

fn red_insert(quad: MeshId, q: &Quad, level: f32) -> (DrawItem, V3) {
    let mut d = DrawItem::new(quad, q.model(), MaterialKind::LightInsert);
    d.material.base_color = RED;
    d.emission = level;
    let k = d.material.lamp_emissive * level;
    (d, [RED[0] * k, RED[1] * k, RED[2] * k])
}

/// The insert beside the ball (off screen for `CAM`), facing the ball.
fn insert_quad(dir: V3) -> Quad {
    Quad::facing(scale(dir, 0.15), [0.0; 3], 0.16)
}

fn no_blur_config() -> PostConfig {
    PostConfig {
        motion_blur: MotionBlurConfig {
            enabled: false,
            ..MotionBlurConfig::default()
        },
        ..PostConfig::default()
    }
}

// ---------------------------------------------------------------------------------------
// Criterion 1: chrome ball beside a lit red insert
// ---------------------------------------------------------------------------------------

struct ReflScene {
    ctx: GpuContext,
    fr: FrameRenderer,
    sphere: MeshId,
    quad: MeshId,
}

impl ReflScene {
    fn new(cfg: PostConfig) -> Option<ReflScene> {
        let ctx = gpu()?;
        let mut fr = FrameRenderer::new(&ctx, CAM.size, CAM.size, cfg).expect("FrameRenderer::new");
        let sphere = fr
            .pbr_mut()
            .upload_mesh(&MeshGeometry::uv_sphere(BALL_R, 96, 48))
            .expect("sphere");
        let quad = fr
            .pbr_mut()
            .upload_mesh(&MeshGeometry::quad(1.0))
            .expect("quad");
        Some(ReflScene {
            ctx,
            fr,
            sphere,
            quad,
        })
    }

    /// Chrome ball at the origin (draw 0) and the red insert `q` at lamp level `level`.
    fn render(&mut self, q: &Quad, level: f32, env: Environment) -> (HdrImage, LdrImage, V3) {
        let cam = CAM.pbr();
        let ball = DrawItem::new(self.sphere, translation([0.0; 3]), MaterialKind::ChromeSteel);
        // The unit quad scaled to the emitter's side: scale the in-plane axes.
        let mut model = q.model();
        for col in 0..2 {
            for row in 0..3 {
                model[col][row] *= q.half * 2.0;
            }
        }
        let (mut insert, radiance) = red_insert(self.quad, q, level);
        insert.model = model;
        let draws = [ball, insert];
        let mut frame = FrameDesc::new(&cam, &[], &draws);
        frame.environment = env;
        frame.clear = [0.0, 0.0, 0.0, 1.0];
        let balls = [SceneBall {
            draw: 0,
            position: [0.0; 3],
            velocity: [0.0; 3],
            radius: BALL_R,
        }];
        let out = self.fr.render(&self.ctx, &frame, &balls).expect("render");
        assert_eq!((out.hdr.width, out.hdr.height), (CAM.size, CAM.size));
        assert_eq!((out.ldr.width, out.ldr.height), (CAM.size, CAM.size));
        assert_finite(&out.hdr, "reflection scene hdr");
        assert!(out.probes.last_frame_updates <= PROBE_BUDGET);
        (out.hdr, out.ldr, radiance)
    }
}

#[test]
fn chrome_ball_reflects_the_lit_red_insert_in_the_expected_region() {
    let Some(mut s) = ReflScene::new(no_blur_config()) else {
        return;
    };
    let chrome = MaterialKind::ChromeSteel.params().base_color;
    // Insert to the right (+Y), left (−Y), above (+Z) and below (−Z) of the ball.
    let dirs: [(V3, &str); 4] = [
        ([0.0, 1.0, 0.0], "right"),
        ([0.0, -1.0, 0.0], "left"),
        ([0.0, 0.0, 1.0], "above"),
        ([0.0, 0.0, -1.0], "below"),
    ];
    for (dir, name) in dirs {
        let q = insert_quad(dir);
        let reg = regions(&CAM, [0.0; 3], BALL_R, &q);
        assert!(
            reg.inside.len() >= 20 && reg.outside.len() >= 500,
            "{name}: analytic regions too small ({} / {})",
            reg.inside.len(),
            reg.outside.len()
        );
        // The expected region sits on the insert's side of the ball image.
        let cx = reg.inside.iter().map(|p| p.0 as f32 + 0.5).sum::<f32>() / reg.inside.len() as f32;
        let cy = reg.inside.iter().map(|p| p.1 as f32 + 0.5).sum::<f32>() / reg.inside.len() as f32;
        let off = [cx - 64.0, 64.0 - cy];
        assert!(
            off[0] * dir[1] + off[1] * dir[2] > 15.0,
            "{name}: test geometry, region centre offset {off:?}"
        );

        let (hdr, _, radiance) = s.render(&q, 1.0, Environment::BLACK);
        let inside = mean_rgb(&hdr, &reg.inside);
        let outside = mean_rgb(&hdr, &reg.outside);
        println!("{name}: inside {inside:?}, outside {outside:?}, insert radiance {radiance:?}");
        // Analytic region colour: a metal mirror shows F · L with F ≈ the chrome base
        // colour at ~45° incidence.
        let want = [
            chrome[0] * radiance[0],
            chrome[1] * radiance[1],
            chrome[2] * radiance[2],
        ];
        assert!(
            inside[0] > 0.6 * want[0] && inside[0] < 1.2 * want[0],
            "{name}: red in the reflection region {} vs F·L = {}",
            inside[0],
            want[0]
        );
        // Red channel ratio: the insert is (1, 0.05, 0.02)-red.
        assert!(
            inside[0] > 8.0 * inside[1] && inside[0] > 8.0 * inside[2],
            "{name}: reflection region is not red: {inside:?}"
        );
        // The rest of the ball reflects the black environment.
        assert!(
            outside[0] < 0.03 * want[0],
            "{name}: red leaks outside the reflection region: {outside:?}"
        );
        // Mirror-image region (point reflection about the ball centre) has no red.
        let mirrored: Vec<(u32, u32)> = reg
            .inside
            .iter()
            .map(|&(x, y)| (CAM.size - 1 - x, CAM.size - 1 - y))
            .collect();
        let opp = mean_rgb(&hdr, &mirrored);
        assert!(
            opp[0] < 0.02 * inside[0],
            "{name}: the opposite side is red too: {opp:?}"
        );

        // Unlit insert: no red reflection.
        let (hdr_off, _, _) = s.render(&q, 0.0, Environment::BLACK);
        // (The probe is refreshed every frame for a single ball.)
        let off_in = mean_rgb(&hdr_off, &reg.inside);
        assert!(
            off_in[0] < 0.05 * inside[0],
            "{name}: unlit insert still reflects red {off_in:?}"
        );
    }
}

#[test]
fn reflection_scene_in_a_studio_is_recognisable_and_not_clipped() {
    let Some(mut s) = ReflScene::new(PostConfig::default()) else {
        return;
    };
    let q = insert_quad([0.0, 1.0, 0.0]);
    let reg = regions(&CAM, [0.0; 3], BALL_R, &q);
    let (hdr, ldr, _) = s.render(&q, 1.0, Environment::STUDIO);
    let inside = mean_rgb(&hdr, &reg.inside);
    let outside = mean_rgb(&hdr, &reg.outside);
    println!("studio: inside {inside:?}, outside {outside:?}");
    // Red where the insert is reflected, a neutral environment elsewhere.
    assert!(inside[0] > 4.0 * inside[1].max(inside[2]), "{inside:?}");
    assert!(
        outside[1] > 0.02 && outside[0] < 2.0 * outside[1],
        "the rest of the ball should show the neutral studio: {outside:?}"
    );
    // A recognisable environment: the studio has a bright horizon band, a dim sky and a
    // dark floor, so the ball's middle rows are brighter than its top and bottom.
    let row = |y0: u32, y1: u32| lum(hdr.region_mean(54, y0, 64, y1));
    let (top, mid, bottom) = (row(34, 40), row(61, 67), row(88, 94));
    println!("studio rows: top {top}, mid {mid}, bottom {bottom}");
    assert!(
        mid > 1.5 * top && mid > 1.5 * bottom,
        "no horizon band in the reflection: top {top}, mid {mid}, bottom {bottom}"
    );
    assert!(top > bottom, "sky should be brighter than the floor");

    // Display image: red region is red, nothing is a clipped flat region.
    let mut r = [0.0f64; 3];
    for &(x, y) in &reg.inside {
        let p = ldr.at(x, y);
        for c in 0..3 {
            r[c] += f64::from(p[c]);
        }
    }
    assert!(r[0] > 1.5 * r[1] && r[0] > 1.5 * r[2], "LDR region {r:?}");
    let clipped = clipped_fraction(&ldr);
    assert!(
        clipped < 0.01,
        "{:.2} % of the display image is clipped white",
        clipped * 100.0
    );
    snapshot(&s.ctx, "chrome_red_insert", &ldr);
}

/// The probe itself: the octahedral map of the ball's slot shows the emitter in the
/// emitter's direction (and nowhere else), for all six axes.
#[test]
fn probe_map_holds_the_emitter_in_its_world_direction() {
    let Some(ctx) = gpu() else { return };
    let mut pbr = PbrRenderer::new(&ctx, 64, 64).expect("pbr");
    let sphere = pbr
        .upload_mesh(&MeshGeometry::uv_sphere(BALL_R, 48, 24))
        .expect("sphere");
    let quad = pbr.upload_mesh(&MeshGeometry::quad(0.16)).expect("quad");
    let mut probes = OctahedralProbes::new(&ctx, &pbr, ProbeConfig::default()).expect("probes");
    assert!(!probes.name().is_empty());
    let cam = CAM.pbr();
    let axes: [V3; 6] = [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ];
    let res = PROBE_RESOLUTION;
    for axis in axes {
        let q = insert_quad(axis);
        let (insert, radiance) = red_insert(quad, &q, 1.0);
        let ball = DrawItem::new(sphere, translation([0.0; 3]), MaterialKind::ChromeSteel);
        let draws = [ball, insert];
        let mut frame = FrameDesc::new(&cam, &[], &draws);
        frame.environment = Environment::BLACK;
        let balls = [ReflectiveBall {
            position: [0.0; 3],
            radius: BALL_R,
            draw: 0,
        }];
        probes
            .prepare(&ctx, &mut pbr, &frame, &balls)
            .expect("prepare");
        assert_eq!(probes.stats().last_frame_updates, 1);
        assert_eq!(probes.scheduler().probe_for_ball(0), Some(0));
        let map = probes.read_probe(&ctx, 0).expect("read_probe");
        assert_eq!((map.width, map.height), (res, res), "probes are 128²");
        assert_finite(&map, "probe map");
        // Texel directions from the documented mapping (texel centres).
        let mut on = ([0.0f64; 3], 0u32);
        let mut off_max = 0.0f32;
        for y in 0..res {
            for x in 0..res {
                let d = oct_decode([
                    (x as f32 + 0.5) / res as f32,
                    (y as f32 + 0.5) / res as f32,
                ]);
                let cosang = dot(d, axis);
                let p = map.at(x, y);
                if cosang > 0.985 {
                    // Within 10° of the axis: well inside the emitter (it subtends ±28°).
                    for c in 0..3 {
                        on.0[c] += f64::from(p[c]);
                    }
                    on.1 += 1;
                } else if cosang < 0.55 {
                    // More than 56° away: outside the emitter (corner at 37°).
                    off_max = off_max.max(p[0]);
                }
            }
        }
        assert!(on.1 >= 20, "axis {axis:?}: only {} texels near the axis", on.1);
        let mean = on.0.map(|v| (v / f64::from(on.1)) as f32);
        println!("axis {axis:?}: probe mean {mean:?} over {} texels, off max {off_max}", on.1);
        for c in 0..3 {
            assert!(
                (mean[c] - radiance[c]).abs() < 0.15 * radiance[c] + 0.02,
                "axis {axis:?}: probe shows {mean:?}, emitter radiance {radiance:?}"
            );
        }
        assert!(
            off_max < 0.05 * radiance[0],
            "axis {axis:?}: red {off_max} far from the emitter direction"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Criterion 3: probe budget, fairness, nearest reuse
// ---------------------------------------------------------------------------------------

#[test]
fn six_balls_stay_within_the_probe_budget_over_64_frames() {
    let Some(ctx) = gpu() else { return };
    let size = 96;
    let mut fr = FrameRenderer::new(&ctx, size, size, PostConfig::default()).expect("renderer");
    let r = 0.027;
    let sphere = fr
        .pbr_mut()
        .upload_mesh(&MeshGeometry::uv_sphere(r, 32, 16))
        .expect("sphere");
    let cam = PbrCamera::look_at(
        [1.2, 0.0, 0.3],
        [0.0; 3],
        [0.0, 0.0, 1.0],
        0.6,
        1.0,
        0.05,
        10.0,
    );
    let lights = [PointLight::new([0.5, 0.0, 0.6], [1.0, 0.9, 0.8], 3.0, 5.0)];
    let frames = 64u64;
    let mut total = 0u64;
    let mut last_hdr = None;
    for f in 0..frames {
        let pos: Vec<V3> = (0..6)
            .map(|i| {
                [
                    0.05 * (f as f32 * 0.3 + i as f32).sin(),
                    -0.25 + 0.1 * i as f32,
                    0.0,
                ]
            })
            .collect();
        let draws: Vec<DrawItem> = pos
            .iter()
            .map(|p| DrawItem::new(sphere, translation(*p), MaterialKind::ChromeSteel))
            .collect();
        let balls: Vec<SceneBall> = pos
            .iter()
            .enumerate()
            .map(|(i, p)| SceneBall {
                draw: i,
                position: *p,
                velocity: [0.3, 0.0, 0.0],
                radius: r,
            })
            .collect();
        let frame = FrameDesc::new(&cam, &lights, &draws);
        let st = if f == 0 || f == frames - 1 {
            let out = fr.render(&ctx, &frame, &balls).expect("render");
            assert_finite(&out.hdr, "six-ball frame");
            let st = out.probes;
            last_hdr = Some((out.hdr, out.ldr));
            st
        } else {
            fr.step(&ctx, &frame, &balls).expect("step")
        };
        assert!(
            st.last_frame_updates <= 2,
            "frame {f}: {} probe updates > budget 2",
            st.last_frame_updates
        );
        assert!(st.max_frame_updates <= 2, "frame {f}: {st:?}");
        assert_eq!(st.budget, 2);
        assert_eq!(st.balls, 6);
        assert_eq!(st.frames, f + 1, "one scheduler frame per rendered frame");
        total += u64::from(st.last_frame_updates);
        assert_eq!(st.total_updates, total, "frame {f}: {st:?}");
        // Every ball's probe age is bounded by ⌈6/2⌉ = 3 frames.
        assert!(st.max_age <= 3, "frame {f}: probe age {} > 3", st.max_age);
        assert!(st.max_age_ever <= 3, "frame {f}: {st:?}");
        assert_eq!(fr.reflections().stats(), st);
    }
    // Round robin: after 64 frames every ball was refreshed about 64·2/6 ≈ 21 times; the
    // age bound above already implies no ball is starved. The budget is used in full.
    assert_eq!(total, 2 * frames, "6 balls use both updates every frame");
    // Every ball is on screen and reflects something (not black, not NaN).
    let (hdr, ldr) = last_hdr.expect("rendered");
    let mean = lum(hdr.region_mean(0, 0, size, size));
    assert!(mean > 1e-3, "six-ball frame is black");
    assert!(clipped_fraction(&ldr) < 0.02);
}

/// Balls without a fresh probe reuse the nearest one: with a budget of one update per
/// frame, the first frame captures one probe, yet both balls show the green ceiling.
#[test]
fn a_ball_without_its_own_probe_reuses_the_nearest_probe() {
    let Some(ctx) = gpu() else { return };
    let cam = Cam {
        eye_x: 0.6,
        fov_y: 0.45,
        size: 128,
    };
    let rad = 0.04;
    let mut pbr = PbrRenderer::new(&ctx, cam.size, cam.size).expect("pbr");
    let sphere = pbr
        .upload_mesh(&MeshGeometry::uv_sphere(rad, 64, 32))
        .expect("sphere");
    let quad = pbr.upload_mesh(&MeshGeometry::quad(0.8)).expect("quad");
    let ceiling = Quad::facing([0.0, 0.0, 0.25], [0.0; 3], 0.8);
    let green = [0.0, 2.0, 0.0];
    let centres: [V3; 2] = [[0.0, -0.06, 0.0], [0.0, 0.06, 0.0]];
    let mut ceiling_draw = DrawItem::new(quad, ceiling.model(), MaterialKind::CeramicPearl);
    ceiling_draw.material = emitter(green);
    let draws = [
        DrawItem::new(sphere, translation(centres[0]), MaterialKind::ChromeSteel),
        DrawItem::new(sphere, translation(centres[1]), MaterialKind::ChromeSteel),
        ceiling_draw,
    ];
    let pcam = cam.pbr();
    let mut frame = FrameDesc::new(&pcam, &[], &draws);
    frame.environment = Environment::BLACK;
    let balls = [
        ReflectiveBall {
            position: centres[0],
            radius: rad,
            draw: 0,
        },
        ReflectiveBall {
            position: centres[1],
            radius: rad,
            draw: 1,
        },
    ];
    let regs: Vec<Regions> = centres
        .iter()
        .map(|c| regions(&cam, *c, rad, &ceiling))
        .collect();
    for r in &regs {
        assert!(r.inside.len() >= 20, "test geometry: {}", r.inside.len());
    }
    let chrome_g = MaterialKind::ChromeSteel.params().base_color[1];

    // Without probes the black placeholder environment gives black balls.
    let plain = pbr
        .render_reflected_to_image(&ctx, &frame, None)
        .expect("render");
    for r in &regs {
        assert!(mean_rgb(&plain, &r.inside)[1] < 0.02, "control is not black");
    }

    let mut probes = OctahedralProbes::new(
        &ctx,
        &pbr,
        ProbeConfig {
            budget: 1,
            ..ProbeConfig::default()
        },
    )
    .expect("probes");
    // Frame 1: one capture.
    probes
        .prepare(&ctx, &mut pbr, &frame, &balls)
        .expect("prepare");
    let st = probes.stats();
    assert_eq!(st.last_frame_updates, 1, "budget 1: {st:?}");
    let owner = probes
        .scheduler()
        .probe_for_ball(0)
        .expect("ball 0 has a probe to sample");
    assert_eq!(
        probes.scheduler().probe_for_ball(1),
        Some(owner),
        "both balls share the only captured probe"
    );
    let img = {
        let binding = probes.binding();
        pbr.render_reflected_to_image(&ctx, &frame, Some(&binding))
            .expect("render")
    };
    assert_finite(&img, "reuse frame 1");
    for (i, r) in regs.iter().enumerate() {
        let m = mean_rgb(&img, &r.inside);
        println!("frame 1 ball {i} (probe owner {owner}): {m:?}");
        assert!(
            m[1] > 0.6 * chrome_g * green[1] && m[1] < 1.2 * chrome_g * green[1],
            "frame 1: ball {i} does not show the ceiling through probe {owner}: {m:?}"
        );
        assert!(m[1] > 8.0 * m[0].max(m[2]), "ball {i} region not green: {m:?}");
    }
    // Frame 2: the other ball gets its own probe; both still show the ceiling.
    probes
        .prepare(&ctx, &mut pbr, &frame, &balls)
        .expect("prepare");
    assert_eq!(probes.stats().last_frame_updates, 1);
    assert_eq!(probes.stats().total_updates, 2);
    assert_eq!(probes.scheduler().probe_for_ball(0), Some(0));
    assert_eq!(probes.scheduler().probe_for_ball(1), Some(1));
    let img = {
        let binding = probes.binding();
        pbr.render_reflected_to_image(&ctx, &frame, Some(&binding))
            .expect("render")
    };
    for (i, r) in regs.iter().enumerate() {
        let m = mean_rgb(&img, &r.inside);
        assert!(
            m[1] > 0.6 * chrome_g * green[1],
            "frame 2: ball {i} lost the ceiling: {m:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// Post chain on CPU-made HDR images
// ---------------------------------------------------------------------------------------

fn image(size: u32, f: impl Fn(u32, u32) -> V3) -> HdrImage {
    let mut pixels = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let c = f(x, y);
            pixels.push([c[0], c[1], c[2], 1.0]);
        }
    }
    HdrImage {
        width: size,
        height: size,
        pixels,
    }
}

struct Chain {
    ctx: GpuContext,
    chain: PostChain,
    cam: PbrCamera,
}

impl Chain {
    fn new(size: u32, format: wgpu::TextureFormat) -> Option<Chain> {
        let ctx = gpu()?;
        let chain = PostChain::new(&ctx, size, size, format).expect("PostChain::new");
        let cam = Cam {
            size,
            ..CAM
        }
        .pbr();
        Some(Chain { ctx, chain, cam })
    }
    fn hdr(&mut self, cfg: PostConfig, img: &HdrImage, balls: &[MotionBall]) -> HdrImage {
        self.chain.set_config(cfg);
        let input = HdrInput::from_image(&self.ctx, img).expect("HdrInput");
        let frame = PostFrame {
            camera: &self.cam,
            balls,
        };
        let out = self
            .chain
            .render_to_hdr(&self.ctx, &input.view, &frame)
            .expect("render_to_hdr");
        assert_eq!((out.width, out.height), (img.width, img.height));
        assert_finite(&out, "post composite");
        out
    }
    fn ldr(&mut self, cfg: PostConfig, img: &HdrImage, balls: &[MotionBall]) -> LdrImage {
        self.chain.set_config(cfg);
        let input = HdrInput::from_image(&self.ctx, img).expect("HdrInput");
        let frame = PostFrame {
            camera: &self.cam,
            balls,
        };
        let out = self
            .chain
            .render_to_ldr(&self.ctx, &input.view, &frame)
            .expect("render_to_ldr");
        assert_eq!((out.width, out.height), (img.width, img.height));
        assert_eq!(out.pixels.len(), (img.width * img.height) as usize);
        out
    }
}

fn plain(tone_map: ToneMap, exposure: f32) -> PostConfig {
    PostConfig {
        exposure,
        bloom: BloomConfig {
            enabled: false,
            ..BloomConfig::default()
        },
        tone_map,
        motion_blur: MotionBlurConfig {
            enabled: false,
            ..MotionBlurConfig::default()
        },
        aa: AaMode::Off,
    }
}

const LEVELS: [f32; 16] = [
    0.0, 0.02, 0.05, 0.1, 0.18, 0.3, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 16.0,
];

/// 64² test chart of 8 px patches: rows 0–1 grey levels, rows 2–3 orange (x, x/2, x/4),
/// rows 4–5 blue-ish (x/4, x/2, x), rows 6–7 pure red.
fn chart_value(x: u32, y: u32) -> V3 {
    let (bx, by) = (x / 8, y / 8);
    let l = LEVELS[((by % 2) * 8 + bx) as usize];
    match by / 2 {
        0 => [l, l, l],
        1 => [l, l * 0.5, l * 0.25],
        2 => [l * 0.25, l * 0.5, l],
        _ => [l, 0.0, 0.0],
    }
}

fn check_chart(ldr: &LdrImage, what: &str, map: impl Fn(f32) -> f32) {
    let mut worst = 0.0f32;
    for by in 0..8u32 {
        for bx in 0..8u32 {
            let c = chart_value(bx * 8 + 4, by * 8 + 4);
            // Patch interior (bloom and blur are off, so every pixel of a patch is equal).
            for (dx, dy) in [(1, 1), (4, 4), (6, 2)] {
                let p = ldr.at(bx * 8 + dx, by * 8 + dy);
                for ch in 0..3 {
                    let want = srgb8(map(c[ch]));
                    let err = (f32::from(p[ch]) - want).abs();
                    worst = worst.max(err);
                    assert!(
                        err <= 2.0,
                        "{what}: patch ({bx}, {by}) channel {ch}: input {} → {} , expected {want:.1}",
                        c[ch],
                        p[ch]
                    );
                }
            }
        }
    }
    println!("{what}: worst 8-bit error {worst:.2}");
}

/// ACES at reference sample points, end to end (shader + display encoding).
#[test]
fn tone_mapping_matches_the_aces_reference_at_sample_points() {
    let Some(mut c) = Chain::new(64, wgpu::TextureFormat::Rgba8Unorm) else {
        return;
    };
    let img = image(64, chart_value);
    let narkowicz = |x: f32| ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0);

    let ldr = c.ldr(plain(ToneMap::AcesFilm, 1.0), &img, &[]);
    check_chart(&ldr, "ACES, exposure 1", narkowicz);
    // The CPU reference agrees with the fit too (ties the shader to `aces_film`).
    check_chart(&ldr, "ACES vs aces_film", aces_film);
    // Mid grey 0.18 → 0.267 → sRGB 141; white 1.0 → 0.804 → 232; 16 → clipped.
    assert!((i32::from(ldr.at(36, 4)[1]) - 141).abs() <= 2, "{:?}", ldr.at(36, 4));
    assert!((i32::from(ldr.at(4, 12)[1]) - 232).abs() <= 2, "{:?}", ldr.at(4, 12));
    assert_eq!(ldr.at(60, 12)[0], 255);
    // Monotone along the grey ramp.
    let ramp: Vec<u8> = (0..16)
        .map(|i| ldr.at((i % 8) * 8 + 4, (i / 8) * 8 + 4)[1])
        .collect();
    assert!(ramp.windows(2).all(|w| w[0] <= w[1]), "grey ramp {ramp:?}");
    assert!(ramp[..14].windows(2).all(|w| w[0] < w[1]), "grey ramp {ramp:?}");

    // Exposure is a linear scale before the curve (5.1 N4: the tone-mapping stage owns it).
    for e in [0.25f32, 0.5, 2.0] {
        let ldr = c.ldr(plain(ToneMap::AcesFilm, e), &img, &[]);
        check_chart(&ldr, &format!("ACES, exposure {e}"), |x| narkowicz(x * e));
    }
    // Linear: clamp only.
    let ldr = c.ldr(plain(ToneMap::Linear, 1.0), &img, &[]);
    check_chart(&ldr, "linear", |x| x.clamp(0.0, 1.0));
    let ldr = c.ldr(plain(ToneMap::Linear, 0.5), &img, &[]);
    check_chart(&ldr, "linear, exposure 0.5", |x| (0.5 * x).clamp(0.0, 1.0));

    // The linear composite before tone mapping is exposure · input.
    let hdr = c.hdr(plain(ToneMap::AcesFilm, 2.0), &img, &[]);
    for (p, q) in hdr.pixels.iter().zip(&img.pixels) {
        for ch in 0..3 {
            assert!(
                (p[ch] - 2.0 * q[ch]).abs() <= 2e-3 * q[ch] * 2.0 + 1e-4,
                "composite {p:?} vs 2 × {q:?}"
            );
        }
    }
}

/// The display bytes do not depend on whether the output format is sRGB-encoded by the
/// hardware or by the shader, nor on the channel order.
#[test]
fn display_encoding_is_the_same_for_every_output_format() {
    let img = image(64, chart_value);
    let mut reference: Option<LdrImage> = None;
    for format in [
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb,
    ] {
        let Some(mut c) = Chain::new(64, format) else {
            return;
        };
        let ldr = c.ldr(plain(ToneMap::AcesFilm, 1.0), &img, &[]);
        check_chart(&ldr, &format!("{format:?}"), aces_film);
        if let Some(r) = &reference {
            for (a, b) in ldr.pixels.iter().zip(&r.pixels) {
                for ch in 0..3 {
                    assert!(a[ch].abs_diff(b[ch]) <= 1, "{format:?}: {a:?} vs {b:?}");
                }
            }
        } else {
            reference = Some(ldr);
        }
    }
}

// --- bloom -----------------------------------------------------------------------------

const BLOOM_SIZE: u32 = 128;
/// Black background: the halo is then stored with the full relative precision of f16 (on a
/// grey background its far tail would drown in the 2⁻¹² quantisation step). That a dim
/// background does not bloom is `bloom_leaves_images_below_the_threshold_untouched`.
const BLOOM_BG: f32 = 0.0;
/// The hot spot: a 4×4 block centred on the image centre.
const SPOT: V3 = [24.0, 12.0, 6.0];

fn in_spot(x: u32, y: u32) -> bool {
    (62..66).contains(&x) && (62..66).contains(&y)
}

fn spot_image() -> HdrImage {
    image(BLOOM_SIZE, |x, y| {
        if in_spot(x, y) {
            SPOT
        } else {
            [BLOOM_BG; 3]
        }
    })
}

fn bloom_only(bloom: BloomConfig) -> PostConfig {
    PostConfig {
        bloom,
        ..plain(ToneMap::AcesFilm, 1.0)
    }
}

fn total_lum(img: &HdrImage) -> f64 {
    img.pixels.iter().map(|p| f64::from(lum(*p))).sum()
}

/// Σ luminance of the CPU prefilter over the image: the energy the bloom may move
/// (before the `intensity` factor).
fn bright_lum(img: &HdrImage, cfg: &BloomConfig) -> f64 {
    img.pixels
        .iter()
        .map(|p| f64::from(lum3(bloom_prefilter([p[0], p[1], p[2]], cfg))))
        .sum()
}

/// Criterion 2, analytic part: the bloom conserves energy and only the above-threshold
/// part moves. Stated tolerance: total luminance before vs after differs by at most 5 % of
/// the moved energy intensity · Σ luminance(bright) (border losses of the widest mip
/// levels; measured 2.4 % on lavapipe, i.e. 0.33 % of the image total); the energy that
/// leaves the hot spot is at most the moved energy.
#[test]
fn bloom_conserves_energy_and_moves_only_the_part_above_the_threshold() {
    let Some(mut c) = Chain::new(BLOOM_SIZE, wgpu::TextureFormat::Rgba8Unorm) else {
        return;
    };
    let cfg = BloomConfig::default();
    assert!(BLOOM_BG < cfg.threshold - cfg.knee, "test background must be below the knee");
    let img = spot_image();
    let out = c.hdr(bloom_only(cfg), &img, &[]);
    let stats = c.chain.stats();
    assert!(stats.bloom_levels >= 2 && stats.bloom_levels <= cfg.max_levels, "{stats:?}");

    let (sum_in, sum_out) = (total_lum(&img), total_lum(&out));
    let moved_max = f64::from(cfg.intensity) * bright_lum(&img, &cfg);
    assert!(moved_max > 1.0, "test image has nothing above the threshold");
    let mut lost = 0.0f64; // leaves the spot
    let mut gained = 0.0f64; // arrives outside
    let mut darkened = 0.0f32;
    for y in 0..BLOOM_SIZE {
        for x in 0..BLOOM_SIZE {
            let d = lum(out.at(x, y)) - lum(img.at(x, y));
            if in_spot(x, y) {
                lost -= f64::from(d);
            } else {
                gained += f64::from(d);
                darkened = darkened.min(d);
            }
        }
    }
    println!(
        "bloom: Σin {sum_in:.3}, Σout {sum_out:.3}, lost {lost:.3}, gained {gained:.3}, intensity·Σbright {moved_max:.3}"
    );
    assert!(
        (sum_out - sum_in).abs() <= 0.05 * moved_max,
        "total luminance changed: {sum_in} → {sum_out}"
    );
    assert!(
        (gained - lost).abs() <= 0.05 * moved_max,
        "energy not conserved: {lost} left the spot, {gained} arrived"
    );
    // Only the above-threshold part moves, and most of it leaves a 4×4 spot.
    assert!(lost <= 1.001 * moved_max, "more energy moved ({lost}) than is above the threshold ({moved_max})");
    assert!(lost >= 0.5 * moved_max, "bloom too weak: {lost} of {moved_max}");
    // Outside the spot the bloom only adds light.
    assert!(darkened > -2e-3, "bloom darkened a background pixel by {darkened}");
    // The spot keeps its colour (out = c − i·bright + i·blur(bright), all ∝ the spot hue).
    let centre = out.at(63, 63);
    assert!(centre[0] < SPOT[0] && centre[0] > (1.0 - cfg.intensity) * SPOT[0] - 0.01, "{centre:?}");
    assert!((centre[1] / centre[0] - 0.5).abs() < 0.02 && (centre[2] / centre[0] - 0.25).abs() < 0.02);

    // Halo shape: centred, symmetric, soft, decaying, with the hue of the source.
    let gain = |x: u32, y: u32| lum(out.at(x, y)) - BLOOM_BG;
    let (mut gx, mut gy, mut gsum) = (0.0f64, 0.0f64, 0.0f64);
    let (mut left, mut right, mut top, mut bottom) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for y in 0..BLOOM_SIZE {
        for x in 0..BLOOM_SIZE {
            if in_spot(x, y) {
                continue;
            }
            let g = f64::from(gain(x, y).max(0.0));
            gx += g * (f64::from(x) + 0.5);
            gy += g * (f64::from(y) + 0.5);
            gsum += g;
            if x < 64 { left += g } else { right += g }
            if y < 64 { top += g } else { bottom += g }
        }
    }
    let (cx, cy) = (gx / gsum, gy / gsum);
    println!("halo centroid ({cx:.2}, {cy:.2}); halves L {left:.3} R {right:.3} T {top:.3} B {bottom:.3}");
    assert!((cx - 64.0).abs() < 0.75 && (cy - 64.0).abs() < 0.75, "halo off centre: ({cx}, {cy})");
    assert!((left - right).abs() < 0.05 * (left + right), "halo lopsided: L {left} R {right}");
    assert!((top - bottom).abs() < 0.05 * (top + bottom), "halo lopsided: T {top} B {bottom}");
    // Ring means decay with distance.
    let radii = [5.0f32, 8.0, 12.0, 18.0, 26.0, 38.0, 54.0];
    let mut rings = Vec::new();
    for w in radii.windows(2) {
        let (mut s, mut n) = (0.0f64, 0u32);
        for y in 0..BLOOM_SIZE {
            for x in 0..BLOOM_SIZE {
                let r = (x as f32 + 0.5 - 64.0).hypot(y as f32 + 0.5 - 64.0);
                if r >= w[0] && r < w[1] {
                    s += f64::from(gain(x, y));
                    n += 1;
                }
            }
        }
        rings.push(s / f64::from(n));
    }
    println!("halo ring means {rings:?}");
    assert!(rings[0] > 0.01, "no visible halo next to the spot: {rings:?}");
    assert!(rings.windows(2).all(|w| w[1] < w[0]), "halo does not decay: {rings:?}");
    // Soft, not blocky: between neighbouring pixels the halo changes by a fraction of its
    // local value (a nearest-neighbour upsample of a coarse mip would jump).
    let mut worst = 0.0f32;
    for y in 0..BLOOM_SIZE {
        for x in 0..BLOOM_SIZE - 1 {
            let r = (x as f32 + 1.0 - 64.0).hypot(y as f32 + 0.5 - 64.0);
            if !(10.0..44.0).contains(&r) {
                continue;
            }
            for (a, b) in [(gain(x, y), gain(x + 1, y)), (gain(y, x), gain(y, x + 1))] {
                let jump = (a - b).abs() / a.max(b).max(1e-4);
                worst = worst.max(jump);
            }
        }
    }
    println!("halo worst neighbour jump {worst:.3}");
    assert!(worst < 0.5, "halo is blocky: neighbouring pixels differ by {:.0} %", worst * 100.0);
    // Hue of the halo = hue of the bright source.
    let h = out.at(72, 64);
    let halo = [h[0] - BLOOM_BG, h[1] - BLOOM_BG, h[2] - BLOOM_BG];
    assert!(
        (halo[1] / halo[0] - 0.5).abs() < 0.05 && (halo[2] / halo[0] - 0.25).abs() < 0.05,
        "halo colour {halo:?} is not the source hue"
    );

    // Intensity is linear: twice the intensity, twice the halo.
    let out2 = c.hdr(
        bloom_only(BloomConfig {
            intensity: 2.0 * cfg.intensity,
            ..cfg
        }),
        &img,
        &[],
    );
    let mut gained2 = 0.0f64;
    for y in 0..BLOOM_SIZE {
        for x in 0..BLOOM_SIZE {
            if !in_spot(x, y) {
                gained2 += f64::from(lum(out2.at(x, y)) - lum(img.at(x, y)));
            }
        }
    }
    assert!((gained2 / gained - 2.0).abs() < 0.05, "halo energy ratio {}", gained2 / gained);
    assert!((total_lum(&out2) - sum_in).abs() <= 0.05 * 2.0 * moved_max);

    // Secondary evidence (D30 b): the golden display image, after the analytic checks.
    let ldr = c.ldr(bloom_only(cfg), &img, &[]);
    snapshot(&c.ctx, "bloom_spot", &ldr);
}

#[test]
fn bloom_leaves_images_below_the_threshold_untouched() {
    let Some(mut c) = Chain::new(BLOOM_SIZE, wgpu::TextureFormat::Rgba8Unorm) else {
        return;
    };
    let cfg = BloomConfig::default();
    // A gradient entirely below threshold − knee.
    let top = (cfg.threshold - cfg.knee) * 0.9;
    let img = image(BLOOM_SIZE, |x, y| {
        let k = top * (x + y) as f32 / (2 * BLOOM_SIZE) as f32;
        [k, k * 0.8, k * 0.6]
    });
    let on = c.hdr(bloom_only(cfg), &img, &[]);
    let off = c.hdr(
        bloom_only(BloomConfig {
            enabled: false,
            ..cfg
        }),
        &img,
        &[],
    );
    for ((a, b), src) in on.pixels.iter().zip(&off.pixels).zip(&img.pixels) {
        for ch in 0..3 {
            assert!((a[ch] - b[ch]).abs() <= 1e-3, "bloom changed a dim pixel: {a:?} vs {b:?}");
            assert!((b[ch] - src[ch]).abs() <= 1e-3, "disabled bloom changed a pixel: {b:?} vs {src:?}");
        }
    }
    // And the display images are identical.
    let a = c.ldr(bloom_only(cfg), &img, &[]);
    let b = c.ldr(
        bloom_only(BloomConfig {
            enabled: false,
            ..cfg
        }),
        &img,
        &[],
    );
    for (p, q) in a.pixels.iter().zip(&b.pixels) {
        for ch in 0..3 {
            assert!(p[ch].abs_diff(q[ch]) <= 1);
        }
    }
}

#[test]
fn bloom_of_a_uniform_image_is_the_image() {
    let Some(mut c) = Chain::new(BLOOM_SIZE, wgpu::TextureFormat::Rgba8Unorm) else {
        return;
    };
    // threshold 0, knee 0 blooms the whole image: out = (1 − i)·c + i·blur(c) = c for a
    // uniform image (the mip weights sum to 1).
    for intensity in [0.15f32, 0.6] {
        let cfg = BloomConfig {
            threshold: 0.0,
            knee: 0.0,
            intensity,
            ..BloomConfig::default()
        };
        let img = image(BLOOM_SIZE, |_, _| [0.5, 0.25, 0.75]);
        let out = c.hdr(bloom_only(cfg), &img, &[]);
        for (i, p) in out.pixels.iter().enumerate() {
            for (ch, want) in [0.5f32, 0.25, 0.75].into_iter().enumerate() {
                assert!(
                    (p[ch] - want).abs() < 0.01 * want + 1e-3,
                    "intensity {intensity}: pixel {i} = {p:?}"
                );
            }
        }
    }
}

/// 5.1 N7: a 3000-nit firefly (chrome highlight, roughness 0.06, no AA) must not turn
/// into a huge halo: the bloom input is clamped.
#[test]
fn a_firefly_pixel_blooms_no_more_than_the_clamp() {
    let Some(mut c) = Chain::new(BLOOM_SIZE, wgpu::TextureFormat::Rgba8Unorm) else {
        return;
    };
    let cfg = BloomConfig::default();
    let fire = 3000.0f32;
    let img = image(BLOOM_SIZE, |x, y| {
        if (x, y) == (64, 64) {
            [fire; 3]
        } else {
            [0.0; 3]
        }
    });
    let out = c.hdr(bloom_only(cfg), &img, &[]);
    let mut gained = 0.0f64;
    let mut peak = 0.0f32;
    for y in 0..BLOOM_SIZE {
        for x in 0..BLOOM_SIZE {
            if (x, y) != (64, 64) {
                let g = lum(out.at(x, y));
                gained += f64::from(g);
                peak = peak.max(g);
            }
        }
    }
    let limit = f64::from(cfg.intensity * cfg.firefly_clamp);
    println!("firefly: halo energy {gained:.3} (limit {limit:.3}), halo peak {peak:.4}, centre {:?}", out.at(64, 64));
    assert!(gained <= 1.02 * limit, "firefly halo energy {gained} > intensity · clamp = {limit}");
    assert!(gained >= 0.3 * limit, "a firefly at the clamp still blooms: {gained}");
    // One pixel of at most 64 spread over at least a 13-tap footprint: a faint halo.
    assert!(peak < f32::from(1u8) * cfg.intensity * cfg.firefly_clamp * 0.25, "halo peak {peak}");
    // The pixel itself only loses what it gave.
    let centre = lum(out.at(64, 64));
    assert!(
        centre <= fire + 2.0 && centre >= fire - cfg.intensity * cfg.firefly_clamp - 2.0,
        "firefly pixel {centre}"
    );
    // The same with the largest f16-safe radiance: still finite everywhere.
    let img = image(BLOOM_SIZE, |x, y| {
        if x % 16 == 3 && y % 16 == 5 {
            [60_000.0, 60_000.0, 0.0]
        } else {
            [0.0; 3]
        }
    });
    let out = c.hdr(bloom_only(cfg), &img, &[]);
    assert_finite(&out, "60 000 fireflies");
    let ldr = c.ldr(bloom_only(cfg), &img, &[]);
    // 49 fireflies of 64 pixels each must not wash the frame out.
    assert!(clipped_fraction(&ldr) < 0.01);
    let bg = ldr.region_mean(8, 8, 12, 12);
    assert!(bg[0] < 128.0, "fireflies washed the image out: {bg:?}");
}

// --- motion blur -----------------------------------------------------------------------

const MB_SIZE: u32 = 128;
const MB_R: f32 = 0.015;
const MB_BG: f32 = 0.125;
const MB_BALL: f32 = 0.75;

fn mb_cam() -> Cam {
    Cam {
        size: MB_SIZE,
        ..CAM
    }
}

/// A disc of the ball's projected size on a flat background (16× supersampled edge).
fn disc_image(center: [f32; 2], radius_px: f32) -> HdrImage {
    image(MB_SIZE, |x, y| {
        let mut cov = 0.0;
        for sy in 0..4 {
            for sx in 0..4 {
                let px = x as f32 + (sx as f32 + 0.5) / 4.0;
                let py = y as f32 + (sy as f32 + 0.5) / 4.0;
                if (px - center[0]).hypot(py - center[1]) < radius_px {
                    cov += 1.0 / 16.0;
                }
            }
        }
        // Dyadic values: exact in f16.
        [MB_BG + (MB_BALL - MB_BG) * cov; 3]
    })
}

fn motion_cfg(enabled: bool) -> PostConfig {
    PostConfig {
        motion_blur: MotionBlurConfig {
            enabled,
            ..MotionBlurConfig::default()
        },
        ..plain(ToneMap::Linear, 1.0)
    }
}

/// Second moments of the excess over the background: (centroid, major-axis unit vector,
/// variance along the major axis, variance along the minor axis, total excess).
fn moments(img: &HdrImage, bg: f32) -> ([f64; 2], [f64; 2], f64, f64, f64) {
    let (mut s, mut sx, mut sy) = (0.0f64, 0.0f64, 0.0f64);
    for y in 0..img.height {
        for x in 0..img.width {
            let w = f64::from((lum(img.at(x, y)) - bg).max(0.0));
            s += w;
            sx += w * (f64::from(x) + 0.5);
            sy += w * (f64::from(y) + 0.5);
        }
    }
    let (cx, cy) = (sx / s, sy / s);
    let (mut xx, mut xy, mut yy) = (0.0f64, 0.0f64, 0.0f64);
    for y in 0..img.height {
        for x in 0..img.width {
            let w = f64::from((lum(img.at(x, y)) - bg).max(0.0));
            let (dx, dy) = (f64::from(x) + 0.5 - cx, f64::from(y) + 0.5 - cy);
            xx += w * dx * dx;
            xy += w * dx * dy;
            yy += w * dy * dy;
        }
    }
    let (xx, xy, yy) = (xx / s, xy / s, yy / s);
    let tr = xx + yy;
    let det = xx * yy - xy * xy;
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let (l1, l2) = (tr / 2.0 + disc, tr / 2.0 - disc);
    let theta = 0.5 * (2.0 * xy).atan2(xx - yy);
    ([cx, cy], [theta.cos(), theta.sin()], l1, l2, s)
}

/// Extent (max − min) of the pixels brighter than `level`, projected on `axis`.
fn extent(img: &HdrImage, level: f32, axis: [f64; 2]) -> f64 {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for y in 0..img.height {
        for x in 0..img.width {
            if lum(img.at(x, y)) > level {
                let t = (f64::from(x) + 0.5) * axis[0] + (f64::from(y) + 0.5) * axis[1];
                lo = lo.min(t);
                hi = hi.max(t);
            }
        }
    }
    hi - lo + 1.0
}

#[test]
fn motion_blur_is_zero_for_a_still_ball() {
    let Some(mut c) = Chain::new(MB_SIZE, wgpu::TextureFormat::Rgba8Unorm) else {
        return;
    };
    let cam = mb_cam();
    let still = MotionBall {
        position: [0.0; 3],
        velocity: [0.0; 3],
        radius: MB_R,
    };
    let m = screen_motion(&c.cam, &still, 1.0 / 120.0, MB_SIZE, MB_SIZE).expect("in view");
    let want_r = MB_R * cam.px_per_m_at_origin();
    assert!((m.radius_px - want_r).abs() < 0.05 * want_r + 0.5);
    let img = disc_image(m.center_px, m.radius_px);

    let off_hdr = c.hdr(motion_cfg(false), &img, &[still]);
    let on_hdr = c.hdr(motion_cfg(true), &img, &[still]);
    assert_eq!(on_hdr.pixels, off_hdr.pixels, "a still ball must leave the image bit-identical");
    let none_hdr = c.hdr(motion_cfg(true), &img, &[]);
    assert_eq!(none_hdr.pixels, off_hdr.pixels, "no balls: nothing to blur");
    let off = c.ldr(motion_cfg(false), &img, &[still]);
    let on = c.ldr(motion_cfg(true), &img, &[still]);
    assert_eq!(on.pixels, off.pixels, "display image changed for a still ball");
    // And the image is the input (linear tone map, exposure 1).
    for (p, q) in off_hdr.pixels.iter().zip(&img.pixels) {
        assert!((p[0] - q[0]).abs() < 1e-3);
    }
    // Shape of the still ball: round.
    let (_, _, l1, l2, _) = moments(&on_hdr, MB_BG);
    assert!(l1 / l2 < 1.05, "still ball is not round: {l1} / {l2}");
}

#[test]
fn motion_blur_streak_is_aligned_with_the_velocity() {
    let Some(mut c) = Chain::new(MB_SIZE, wgpu::TextureFormat::Rgba8Unorm) else {
        return;
    };
    let shutter = MotionBlurConfig::default().shutter_s;
    let speed = 3.0f32;
    let contrast = MB_BALL - MB_BG;
    // Directions in the image plane (world y = image right, world z = image up).
    for deg in [0.0f32, 30.0, 45.0, 90.0, 135.0, 200.0, 270.0] {
        let a = deg.to_radians();
        let ball = MotionBall {
            position: [0.0; 3],
            velocity: [0.0, speed * a.cos(), speed * a.sin()],
            radius: MB_R,
        };
        let m = screen_motion(&c.cam, &ball, shutter, MB_SIZE, MB_SIZE).expect("in view");
        let d = [f64::from(m.displacement_px[0]), f64::from(m.displacement_px[1])];
        let dl = d[0].hypot(d[1]);
        // Independent expectation of the streak direction and length (pixels, y down).
        let want_dir = [f64::from(a.cos()), -f64::from(a.sin())];
        let want_len = f64::from(speed * shutter * mb_cam().px_per_m_at_origin());
        assert!((dl - want_len).abs() < 0.05 * want_len, "{deg}°: |d| {dl} vs {want_len}");
        assert!((d[0] * want_dir[0] + d[1] * want_dir[1]) / dl > 0.995, "{deg}°: {d:?}");

        let img = disc_image(m.center_px, m.radius_px);
        let sharp = c.hdr(motion_cfg(false), &img, &[ball]);
        let blurred = c.hdr(motion_cfg(true), &img, &[ball]);
        let stats = c.chain.stats();
        assert!(stats.blurred_balls >= 1, "{deg}°: {stats:?}");
        assert!(stats.max_blur_px > 0.5 * want_len as f32 && stats.max_blur_px <= 64.0, "{stats:?}");
        assert_ne!(blurred.pixels, sharp.pixels, "{deg}°: a moving ball was not blurred");

        let (c0, _, s1, s2, e0) = moments(&sharp, MB_BG);
        let (c1, axis, l1, l2, e1) = moments(&blurred, MB_BG);
        let cosang = (axis[0] * want_dir[0] + axis[1] * want_dir[1]).abs();
        println!(
            "{deg}°: |d| {dl:.1} px; sharp var {s1:.1}/{s2:.1}; blurred var {l1:.1}/{l2:.1}, axis {axis:?} (cos {cosang:.4}), centroid shift ({:.1}, {:.1}), excess {e0:.1} → {e1:.1}",
            c1[0] - c0[0],
            c1[1] - c0[1]
        );
        // The streak is elongated…
        assert!(l1 > 1.3 * l2, "{deg}°: no streak (variances {l1} / {l2})");
        // …along the velocity (within 8°)…
        assert!(cosang > 0.990, "{deg}°: streak axis {axis:?} vs velocity {want_dir:?}");
        // …and not across it: the width stays the ball's.
        assert!(l2 < 1.25 * s2 + 1.0, "{deg}°: the ball got wider across its velocity: {s2} → {l2}");
        assert!(l1 > 1.3 * s1, "{deg}°: no elongation along the velocity: {s1} → {l1}");
        // Length of the streak ≈ diameter + displacement; width ≈ diameter.
        let level = MB_BG + 0.035 * contrast;
        let along = extent(&blurred, level, want_dir);
        let across = extent(&blurred, level, [-want_dir[1], want_dir[0]]);
        let diameter = 2.0 * f64::from(m.radius_px);
        println!("{deg}°: extent along {along:.1}, across {across:.1}, diameter {diameter:.1}");
        assert!(
            along > diameter + 0.6 * dl && along < diameter + 1.25 * dl + 4.0,
            "{deg}°: streak length {along} vs diameter {diameter} + displacement {dl}"
        );
        assert!(across < diameter + 4.0, "{deg}°: streak width {across} vs diameter {diameter}");
        // The streak stays near the ball: its centroid moves by at most the displacement.
        let shift = (c1[0] - c0[0]).hypot(c1[1] - c0[1]);
        assert!(shift < 0.6 * dl + 1.0, "{deg}°: centroid moved {shift} px");
        // A mean over the shutter never brightens or darkens beyond the input range, and
        // keeps the light of the ball (within 25 %).
        for p in &blurred.pixels {
            assert!(p[0] <= MB_BALL + 2e-3 && p[0] >= MB_BG - 2e-3, "{deg}°: pixel {p:?} out of range");
        }
        assert!((e1 / e0 - 1.0).abs() < 0.25, "{deg}°: ball light changed {e0} → {e1}");
        // Pixels far from the ball are untouched.
        for y in 0..MB_SIZE {
            for x in 0..MB_SIZE {
                let r = (f64::from(x) + 0.5 - c0[0]).hypot(f64::from(y) + 0.5 - c0[1]);
                if r > f64::from(m.radius_px) + dl + 3.0 {
                    assert_eq!(blurred.at(x, y), sharp.at(x, y), "{deg}°: far pixel ({x}, {y}) changed");
                }
            }
        }
    }
}

#[test]
fn motion_blur_grows_with_speed_and_is_clamped() {
    let Some(mut c) = Chain::new(MB_SIZE, wgpu::TextureFormat::Rgba8Unorm) else {
        return;
    };
    let still = MotionBall {
        position: [0.0; 3],
        velocity: [0.0; 3],
        radius: MB_R,
    };
    let m = screen_motion(&c.cam, &still, 1.0 / 120.0, MB_SIZE, MB_SIZE).expect("in view");
    let img = disc_image(m.center_px, m.radius_px);
    let level = MB_BG + 0.035 * (MB_BALL - MB_BG);
    let mut last = 0.0;
    for speed in [0.0f32, 1.0, 2.0, 4.0] {
        let ball = MotionBall {
            velocity: [0.0, speed, 0.0],
            ..still
        };
        let out = c.hdr(motion_cfg(true), &img, &[ball]);
        let along = extent(&out, level, [1.0, 0.0]);
        println!("speed {speed}: streak length {along:.1}");
        assert!(along > last, "streak does not grow with speed: {last} → {along}");
        last = along;
    }
    // A short clamp limits the streak whatever the speed.
    let cfg = PostConfig {
        motion_blur: MotionBlurConfig {
            max_length_px: 8.0,
            ..MotionBlurConfig::default()
        },
        ..motion_cfg(true)
    };
    let fast = MotionBall {
        velocity: [0.0, 40.0, 0.0],
        ..still
    };
    let out = c.hdr(cfg, &img, &[fast]);
    let along = extent(&out, level, [1.0, 0.0]);
    let diameter = 2.0 * f64::from(m.radius_px);
    assert!(along <= diameter + 8.0 + 4.0, "clamped streak is {along} px long (diameter {diameter})");
    assert!(c.chain.stats().max_blur_px <= 8.0 + 1e-3, "{:?}", c.chain.stats());
    assert_finite(&out, "clamped blur");
    // A ball behind the camera or with a NaN velocity cannot poison the frame.
    for bad in [
        MotionBall {
            position: [5.0, 0.0, 0.0],
            ..fast
        },
        MotionBall {
            velocity: [f32::NAN, f32::INFINITY, 0.0],
            ..still
        },
    ] {
        let out = c.hdr(motion_cfg(true), &img, &[bad]);
        assert_finite(&out, "bad motion ball");
    }
}

/// The whole frame: a moving emissive ball rendered by the PBR pass streaks along its
/// velocity in the display image; a still one is identical to the blur-free image.
#[test]
fn frame_renderer_blurs_a_moving_ball_along_its_velocity() {
    let Some(ctx) = gpu() else { return };
    let cam = mb_cam();
    let base = PostConfig {
        aa: AaMode::Msaa4,
        ..plain(ToneMap::Linear, 1.0)
    };
    let mut fr = FrameRenderer::new(&ctx, MB_SIZE, MB_SIZE, base).expect("renderer");
    let sphere = fr
        .pbr_mut()
        .upload_mesh(&MeshGeometry::uv_sphere(MB_R, 48, 24))
        .expect("sphere");
    let mut ball = DrawItem::new(sphere, translation([0.0; 3]), MaterialKind::CeramicPearl);
    ball.material = emitter([0.5; 3]);
    let draws = [ball];
    let pcam = cam.pbr();
    let mut frame = FrameDesc::new(&pcam, &[], &draws);
    frame.environment = Environment::BLACK;
    let shot = |fr: &mut FrameRenderer, blur: bool, v: V3| {
        fr.post_mut().set_config(PostConfig {
            motion_blur: MotionBlurConfig {
                enabled: blur,
                ..MotionBlurConfig::default()
            },
            ..base
        });
        let balls = [SceneBall {
            draw: 0,
            position: [0.0; 3],
            velocity: v,
            radius: MB_R,
        }];
        let out = fr.render(&ctx, &frame, &balls).expect("render");
        assert_finite(&out.hdr, "moving ball hdr");
        out.ldr
    };
    let sharp = shot(&mut fr, false, [0.0; 3]);
    let still = shot(&mut fr, true, [0.0; 3]);
    assert_eq!(still.pixels, sharp.pixels, "still ball: blur must be zero");

    // Moments on the display image (linear tone map: monotone in radiance).
    let as_hdr = |l: &LdrImage| HdrImage {
        width: l.width,
        height: l.height,
        pixels: l
            .pixels
            .iter()
            .map(|p| [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0, 1.0])
            .collect(),
    };
    let (_, _, s1, s2, _) = moments(&as_hdr(&sharp), 0.0);
    assert!(s1 / s2 < 1.05, "sharp ball is not round");
    for deg in [0.0f32, 60.0, 90.0, 150.0] {
        let a = deg.to_radians();
        let v = [0.0, 3.0 * a.cos(), 3.0 * a.sin()];
        let moving = shot(&mut fr, true, v);
        let moved_blur_off = shot(&mut fr, false, v);
        assert_eq!(moved_blur_off.pixels, sharp.pixels, "velocity must only matter to the blur");
        let (_, axis, l1, l2, _) = moments(&as_hdr(&moving), 0.0);
        let want = [f64::from(a.cos()), -f64::from(a.sin())];
        let cosang = (axis[0] * want[0] + axis[1] * want[1]).abs();
        println!("frame {deg}°: variances {l1:.1} / {l2:.1} (sharp {s1:.1}), axis {axis:?}, cos {cosang:.4}");
        assert!(l1 > 1.3 * l2, "{deg}°: no streak");
        assert!(cosang > 0.990, "{deg}°: streak {axis:?} not along the velocity {want:?}");
        assert!(l2 < 1.25 * s2 + 1.0, "{deg}°: wider across the velocity");
        if deg == 0.0 {
            snapshot(&ctx, "motion_blur_right", &moving);
        }
    }
}

// ---------------------------------------------------------------------------------------
// Anti-aliasing
// ---------------------------------------------------------------------------------------

/// Analytic pixel coverage of a disc (8×8 supersampling).
fn disc_coverage(size: u32, cx: f32, cy: f32, r: f32) -> Vec<f32> {
    let mut v = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            let mut cov = 0.0;
            for sy in 0..8 {
                for sx in 0..8 {
                    let px = x as f32 + (sx as f32 + 0.5) / 8.0;
                    let py = y as f32 + (sy as f32 + 0.5) / 8.0;
                    if (px - cx).hypot(py - cy) < r {
                        cov += 1.0 / 64.0;
                    }
                }
            }
            v.push(cov);
        }
    }
    v
}

/// Edge aliasing metrics of an image of a flat disc of radiance `e`: (mean absolute
/// coverage error over the edge pixels, fraction of edge pixels with an intermediate
/// value, interior mean / e).
fn edge_metrics(img: &HdrImage, cov: &[f32], e: f32) -> (f64, f64, f64) {
    let (mut err, mut n, mut mid) = (0.0f64, 0u32, 0u32);
    let (mut inner, mut ni) = (0.0f64, 0u32);
    for (p, &c) in img.pixels.iter().zip(cov) {
        let v = p[1] / e;
        if c > 0.02 && c < 0.98 {
            err += f64::from((v - c).abs());
            n += 1;
            if v > 0.1 && v < 0.9 {
                mid += 1;
            }
        } else if c >= 1.0 {
            inner += f64::from(v);
            ni += 1;
        } else if c <= 0.0 {
            assert!(v.abs() < 1e-3, "background pixel is lit: {v}");
        }
    }
    (
        err / f64::from(n),
        f64::from(mid) / f64::from(n),
        inner / f64::from(ni),
    )
}

#[test]
fn msaa_reduces_edge_aliasing_measurably() {
    let Some(ctx) = gpu() else { return };
    let size = 96u32;
    let cam = Cam {
        size,
        ..CAM
    };
    let e = 0.5f32;
    // Off-centre by a fraction of a pixel so the edge is not symmetric about pixel centres.
    let centre = [0.0, 0.0031, -0.0017];
    let c_px = cam.project(centre);
    // Silhouette radius of a sphere seen in perspective (on-axis approximation is exact
    // to < 0.1 px here).
    let dist = cam.eye_x;
    let r_px = BALL_R / (dist * dist - BALL_R * BALL_R).sqrt() / (cam.fov_y * 0.5).tan()
        * size as f32
        * 0.5;
    let cov = disc_coverage(size, c_px[0], c_px[1], r_px);

    let mut results = Vec::new();
    for aa in [AaMode::Off, AaMode::Msaa4] {
        let cfg = PostConfig {
            aa,
            ..plain(ToneMap::Linear, 1.0)
        };
        let mut fr = FrameRenderer::new(&ctx, size, size, cfg).expect("renderer");
        let want_samples = if matches!(aa, AaMode::Msaa4) { 4 } else { 1 };
        assert_eq!(fr.pbr().msaa_samples(), want_samples, "{aa:?}");
        let sphere = fr
            .pbr_mut()
            .upload_mesh(&MeshGeometry::uv_sphere(BALL_R, 128, 64))
            .expect("sphere");
        let mut d = DrawItem::new(sphere, translation(centre), MaterialKind::CeramicPearl);
        d.material = emitter([e; 3]);
        let draws = [d];
        let pcam = cam.pbr();
        let mut frame = FrameDesc::new(&pcam, &[], &draws);
        frame.environment = Environment::BLACK;
        let out = fr.render(&ctx, &frame, &[]).expect("render");
        assert_finite(&out.hdr, "aa scene");
        let (err, mid, inner) = edge_metrics(&out.hdr, &cov, e);
        let total: f64 = out.hdr.pixels.iter().map(|p| f64::from(p[1] / e)).sum();
        let want_total: f64 = cov.iter().map(|c| f64::from(*c)).sum();
        println!("{aa:?}: edge coverage error {err:.4}, intermediate edge pixels {:.1} %, interior {inner:.4}, area {total:.1} (analytic {want_total:.1})", mid * 100.0);
        assert!((inner - 1.0).abs() < 0.01, "{aa:?}: interior radiance {inner}");
        assert!((total / want_total - 1.0).abs() < 0.01, "{aa:?}: disc area {total} vs {want_total}");
        // The display image shows the same edge (linear tone map → sRGB bytes).
        let mut ldr_mid = 0u32;
        let full = srgb8(e);
        for p in &out.ldr.pixels {
            let v = f32::from(p[1]);
            if v > 0.15 * full && v < 0.9 * full {
                ldr_mid += 1;
            }
        }
        results.push((err, mid, ldr_mid, out.ldr));
    }
    let (off, on) = (&results[0], &results[1]);
    // Without AA an edge pixel is all or nothing: mean |coverage error| ≈ 0.25. Four
    // samples quantise coverage to quarters: ≈ 0.08.
    assert!(off.0 > 0.15, "the no-AA reference is unexpectedly smooth: {}", off.0);
    assert!(
        on.0 < 0.6 * off.0,
        "MSAA does not reduce edge aliasing: coverage error {} → {}",
        off.0,
        on.0
    );
    assert!(off.1 < 0.05, "no-AA edge has intermediate values: {}", off.1);
    assert!(on.1 > 0.4, "MSAA edge has too few intermediate values: {}", on.1);
    assert!(on.2 > off.2 + 50, "display image edge is not smoother: {} vs {}", on.2, off.2);
    snapshot(&ctx, "disc_msaa4", &on.3);
}

#[test]
fn pbr_msaa_setting_resolves_into_the_hdr_target() {
    let Some(ctx) = gpu() else { return };
    let size = 64u32;
    let cam = Cam {
        size,
        ..CAM
    };
    let mut pbr = PbrRenderer::new(&ctx, size, size).expect("pbr");
    assert_eq!(pbr.msaa_samples(), 1, "default is the 5.1 look");
    let sphere = pbr
        .upload_mesh(&MeshGeometry::uv_sphere(BALL_R, 96, 48))
        .expect("sphere");
    let mut d = DrawItem::new(sphere, translation([0.0; 3]), MaterialKind::CeramicPearl);
    d.material = emitter([1.0; 3]);
    let draws = [d];
    let pcam = cam.pbr();
    let mut frame = FrameDesc::new(&pcam, &[], &draws);
    frame.environment = Environment::BLACK;
    let one = pbr.render_to_image(&ctx, &frame).expect("render");
    pbr.set_msaa_samples(4).expect("4× MSAA");
    assert_eq!(pbr.msaa_samples(), 4);
    let four = pbr.render_to_image(&ctx, &frame).expect("render");
    assert_finite(&four, "msaa image");
    let partial = |img: &HdrImage| {
        img.pixels
            .iter()
            .filter(|p| p[0] > 0.1 && p[0] < 0.9)
            .count()
    };
    println!("partial edge pixels: 1× {}, 4× {}", partial(&one), partial(&four));
    assert_eq!(partial(&one), 0, "1× has all-or-nothing edges");
    assert!(partial(&four) > 40, "4× resolve gives fractional edge coverage");
    // Resolved values are multiples of a quarter.
    for p in &four.pixels {
        let q = p[0] * 4.0;
        assert!((q - q.round()).abs() < 0.02, "resolved coverage {} is not n/4", p[0]);
    }
    // Same area.
    let area = |img: &HdrImage| img.pixels.iter().map(|p| f64::from(p[0])).sum::<f64>();
    assert!((area(&four) / area(&one) - 1.0).abs() < 0.01);
    // Back to 1×: the 5.1 image again.
    pbr.set_msaa_samples(1).expect("1×");
    let again = pbr.render_to_image(&ctx, &frame).expect("render");
    assert_eq!(again.pixels, one.pixels);
}

// ---------------------------------------------------------------------------------------
// Whole frame: no NaN/Inf, no clipped flat regions (5.1 N4, N7)
// ---------------------------------------------------------------------------------------

/// The two 5.1 scenes that clipped in 8 bits (a lit insert and the pearl under the key
/// light) and a chrome ball with a 3000-nit highlight, through the default chain.
#[test]
fn tone_mapped_test_scenes_are_finite_and_not_clipped() {
    let Some(ctx) = gpu() else { return };
    let size = 128u32;
    let cam = Cam {
        size,
        ..CAM
    };
    let mut fr = FrameRenderer::new(&ctx, size, size, PostConfig::default()).expect("renderer");
    let sphere = fr
        .pbr_mut()
        .upload_mesh(&MeshGeometry::uv_sphere(BALL_R, 96, 48))
        .expect("sphere");
    let pcam = cam.pbr();
    // The 5.1 golden lighting: one white key light, irradiance π at the sphere centre.
    let key = [0.5f32, 0.25, 0.3];
    let lights = [PointLight::new(key, [1.0; 3], PI * dot(key, key), 10.0)];

    for (kind, emission, name) in [
        (MaterialKind::LightInsert, 1.0f32, "light_insert_lit"),
        (MaterialKind::CeramicPearl, 0.0, "ceramic_pearl"),
        (MaterialKind::ChromeSteel, 0.0, "chrome_steel"),
        (MaterialKind::PlayfieldWood, 0.0, "playfield_wood"),
    ] {
        let mut d = DrawItem::new(sphere, translation([0.0; 3]), kind);
        d.emission = emission;
        let draws = [d];
        let mut frame = FrameDesc::new(&pcam, &lights, &draws);
        frame.clear = [0.18, 0.18, 0.18, 1.0];
        let balls = [SceneBall {
            draw: 0,
            position: [0.0; 3],
            velocity: [0.0; 3],
            radius: BALL_R,
        }];
        let out = fr.render(&ctx, &frame, &balls).expect("render");
        assert_finite(&out.hdr, name);
        let peak = out.hdr.pixels.iter().map(|p| lum(*p)).fold(0.0f32, f32::max);
        let clipped = clipped_fraction(&out.ldr);
        // Largest run of saturated pixels in a row: a "flat clipped region".
        let mut run_max = 0u32;
        for y in 0..size {
            let mut run = 0u32;
            for x in 0..size {
                let p = out.ldr.at(x, y);
                if p[0] == 255 && p[1] == 255 && p[2] == 255 {
                    run += 1;
                    run_max = run_max.max(run);
                } else {
                    run = 0;
                }
            }
        }
        // Any channel at the ceiling over the ball (hue loss).
        let any = out
            .ldr
            .pixels
            .iter()
            .filter(|p| p[0] == 255 || p[1] == 255 || p[2] == 255)
            .count() as f64
            / out.ldr.pixels.len() as f64;
        println!(
            "{name}: HDR peak luminance {peak:.1}, clipped white {:.2} %, any channel at 255 {:.2} %, longest clipped run {run_max} px",
            clipped * 100.0,
            any * 100.0
        );
        // The ball covers ~35 % of the frame; a specular highlight may saturate a few
        // pixels, a flat clipped region may not exist.
        assert!(clipped < 0.01, "{name}: {:.2} % clipped white", clipped * 100.0);
        assert!(any < 0.03, "{name}: {:.2} % of the pixels have a clipped channel", any * 100.0);
        assert!(run_max <= 12, "{name}: a clipped run of {run_max} px");
        // The background grey 0.18 is tone mapped, not clipped or crushed.
        let bg = out.ldr.region_mean(2, 2, 10, 10);
        assert!(bg[1] > 100.0 && bg[1] < 190.0, "{name}: background {bg:?}");
        // The image has content: the ball differs from the background.
        let mid = out.ldr.region_mean(56, 56, 72, 72);
        assert!((mid[0] - bg[0]).abs() + (mid[2] - bg[2]).abs() > 4.0, "{name}: ball invisible");
        snapshot(&ctx, &format!("scene_{name}"), &out.ldr);
    }
}
