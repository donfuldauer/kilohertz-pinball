//! Independent GPU spec tests for Subphase 5.1 (verification criteria 2, 3 and 5), under
//! the image test policy D30: analytic pixel assertions first; a regression snapshot is
//! compared (and can only be stored) after the analytic assertions of the same image pass.
//!
//! Signatures looked up from the rustdoc of the compiled crate (the brief leaves the GPU
//! entry points open): `PbrRenderer::{new(&GpuContext, w, h), upload_mesh,
//! render_to_image(&GpuContext, &FrameDesc) -> HdrImage, stats}`, `FrameDesc`, `DrawItem`,
//! `MaterialParams`, `Environment`, `MeshGeometry::uv_sphere`, `PbrCamera::look_at`,
//! `PointLight`, `PbrStats`, `HdrImage`. Documented conventions used here: the output is
//! linear radiance (`pixels[y·width + x]`, row 0 = top); a point light of intensity I
//! gives irradiance I/d² (windowed by `(1 − (d/range)⁴)²`), so a white Lambert surface
//! reflects `I·cosθ / (π d²)`.
//!
//! Scene: a sphere of radius 5 cm at the world origin, a perspective camera on the +X
//! axis looking at it with +Z up, so image right = world +Y and image up = world +Z.
//! Each pixel's surface point, normal and view vector are computed here by ray casting,
//! which gives per-pixel analytic expectations.
//!
//! Snapshots: `tests/fixtures/spec_5_1/<name>.rgba8` (raw sRGB8, 64×64). Set
//! `SPEC_5_1_BLESS=1` to (re)write them; they are written only after the analytic checks.
#![cfg(feature = "gpu-tests")]
#![allow(clippy::needless_range_loop)] // channel/axis indices read clearer as ranges
#![allow(clippy::needless_borrow)] // Wave 5 gate fix: `ctx` became a shared `&'static GpuContext`; call sites unchanged

use pin_core::FrameStateSnapshot;
use pin_render::gpu::{GpuContext, GpuOptions, require_gpu};
use pin_render::pbr::math::{IDENTITY, PbrCamera};
use pin_render::pbr::{
    DrawItem, Environment, FrameDesc, HdrImage, LampBank, LampKind, MAX_LIGHTS, MaterialKind,
    MaterialParams, MeshGeometry, PbrRenderer, PointLight,
};
use std::f32::consts::PI;

const R: f32 = 0.05;
const CAM_X: f32 = 0.6;
const FOV_Y: f32 = 0.25;
const SIZE: u32 = 96;

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
fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}
fn norm(a: V3) -> V3 {
    scale(a, 1.0 / len(a))
}
fn lum(p: [f32; 4]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn new_context() -> Option<GpuContext> {
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

/// One context per test process, shared by every test (Wave 5 gate fix: a
/// device created and dropped per test thread crashed lavapipe in CI; see
/// DECISIONS "Wave 5 gate fix").
fn gpu() -> Option<&'static GpuContext> {
    static CTX: std::sync::OnceLock<Option<GpuContext>> = std::sync::OnceLock::new();
    CTX.get_or_init(new_context).as_ref()
}

fn camera() -> PbrCamera {
    PbrCamera::look_at(
        [CAM_X, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        FOV_Y,
        1.0,
        0.1,
        5.0,
    )
}

/// What a pixel sees of the sphere.
#[derive(Copy, Clone)]
struct Hit {
    p: V3,
    n: V3,
    /// Unit vector from the surface to the camera.
    v: V3,
}

/// Ray cast through the centre of pixel (x, y) of a `size`² image.
fn hit(size: u32, x: u32, y: u32) -> Option<Hit> {
    let t = (FOV_Y * 0.5).tan();
    let ndc_x = (x as f32 + 0.5) / size as f32 * 2.0 - 1.0;
    let ndc_y = 1.0 - (y as f32 + 0.5) / size as f32 * 2.0;
    // forward −X, right +Y, up +Z.
    let d = norm([-1.0, ndc_x * t, ndc_y * t]);
    let o = [CAM_X, 0.0, 0.0];
    let b = dot(o, d);
    let c = dot(o, o) - R * R;
    let disc = b * b - c;
    if disc <= 0.0 {
        return None;
    }
    let s = -b - disc.sqrt();
    let p = add(o, scale(d, s));
    Some(Hit {
        p,
        n: norm(p),
        v: scale(d, -1.0),
    })
}

/// Distance in pixels from the image centre at which the silhouette lies.
fn silhouette_px(size: u32) -> f32 {
    let tan_edge = R / (CAM_X * CAM_X - R * R).sqrt();
    tan_edge / (FOV_Y * 0.5).tan() * size as f32 * 0.5
}

fn center_dist(size: u32, x: u32, y: u32) -> f32 {
    let c = size as f32 * 0.5;
    ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt()
}

fn window(d: f32, range: f32) -> f32 {
    let q = (d / range).powi(4);
    (1.0 - q).clamp(0.0, 1.0).powi(2)
}

/// Analytic Lambert radiance (albedo 1) of `h` under `lights`.
fn lambert(h: &Hit, lights: &[PointLight]) -> V3 {
    let mut out = [0.0; 3];
    for l in lights {
        let to = sub(l.position, h.p);
        let d = len(to);
        let cos = dot(h.n, scale(to, 1.0 / d)).max(0.0);
        let e = l.intensity * cos / (d * d) * window(d, l.range) / PI;
        for c in 0..3 {
            out[c] += l.color[c] * e;
        }
    }
    out
}

struct Scene {
    ctx: &'static GpuContext,
    renderer: PbrRenderer,
    sphere: pin_render::pbr::MeshId,
    cam: PbrCamera,
    size: u32,
}

impl Scene {
    fn new(size: u32) -> Option<Scene> {
        let ctx = gpu()?;
        let mut renderer = PbrRenderer::new(&ctx, size, size).expect("PbrRenderer::new");
        let sphere = renderer
            .upload_mesh(&MeshGeometry::uv_sphere(R, 96, 48))
            .expect("upload sphere");
        Some(Scene {
            ctx,
            renderer,
            sphere,
            cam: camera(),
            size,
        })
    }

    fn item(&self, material: MaterialParams) -> DrawItem {
        let mut d = DrawItem::new(self.sphere, IDENTITY, MaterialKind::CeramicPearl);
        d.material = material;
        d.transparent = false;
        d.emission = 0.0;
        d
    }

    fn kind(&self, kind: MaterialKind) -> DrawItem {
        DrawItem::new(self.sphere, IDENTITY, kind)
    }

    /// Renders with a black environment (only the given lights and emission contribute).
    fn render(
        &mut self,
        draws: &[DrawItem],
        lights: &[PointLight],
        gi: [f32; 4],
        clear: [f32; 4],
    ) -> HdrImage {
        self.render_env(draws, lights, gi, clear, Environment::BLACK)
    }

    fn render_env(
        &mut self,
        draws: &[DrawItem],
        lights: &[PointLight],
        gi: [f32; 4],
        clear: [f32; 4],
        environment: Environment,
    ) -> HdrImage {
        let mut frame = FrameDesc::new(&self.cam, lights, draws);
        frame.environment = environment;
        frame.gi = gi;
        frame.clear = clear;
        let img = self
            .renderer
            .render_to_image(&self.ctx, &frame)
            .expect("render_to_image");
        assert_eq!((img.width, img.height), (self.size, self.size));
        assert_eq!(img.pixels.len(), (self.size * self.size) as usize);
        for (i, p) in img.pixels.iter().enumerate() {
            assert!(
                p.iter().all(|c| c.is_finite()),
                "pixel {i} is not finite: {p:?} (NaN/Inf speckle)"
            );
            assert!(
                p[0] >= 0.0 && p[1] >= 0.0 && p[2] >= 0.0,
                "pixel {i} is negative: {p:?}"
            );
        }
        img
    }
}

fn px(img: &HdrImage, x: u32, y: u32) -> [f32; 4] {
    img.pixels[(y * img.width + x) as usize]
}

const NO_GI: [f32; 4] = [0.0, 0.0, 0.0, 0.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const GREY: [f32; 4] = [0.18, 0.18, 0.18, 1.0];

/// Pixels well inside the silhouette (by `margin` px), with their hits.
fn interior(size: u32, margin: f32) -> Vec<(u32, u32, Hit)> {
    let edge = silhouette_px(size);
    let mut v = Vec::new();
    for y in 0..size {
        for x in 0..size {
            if center_dist(size, x, y) < edge - margin {
                v.push((
                    x,
                    y,
                    hit(size, x, y).expect("interior pixel hits the sphere"),
                ));
            }
        }
    }
    v
}

/// Mean RGB over the interior of the sphere.
fn sphere_mean(img: &HdrImage, margin: f32) -> V3 {
    let pts = interior(img.width, margin);
    let mut s = [0.0f64; 3];
    for (x, y, _) in &pts {
        let p = px(img, *x, *y);
        for c in 0..3 {
            s[c] += p[c] as f64;
        }
    }
    let n = pts.len() as f64;
    [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32]
}

/// The background (pixels at least 2 px outside the silhouette) is exactly the clear
/// colour: a neutral background, no halo, no stray fragments.
fn assert_background(img: &HdrImage, clear: [f32; 4]) {
    let edge = silhouette_px(img.width);
    let mut n = 0;
    for y in 0..img.height {
        for x in 0..img.width {
            if center_dist(img.width, x, y) > edge + 2.0 {
                let p = px(img, x, y);
                for c in 0..3 {
                    assert!(
                        (p[c] - clear[c]).abs() <= 2.0e-3,
                        "background pixel ({x},{y}) = {p:?}, clear = {clear:?}"
                    );
                }
                n += 1;
            }
        }
    }
    assert!(n > 100, "the sphere fills the whole image");
}

/// The brightest interior pixel (by luminance).
fn brightest(img: &HdrImage) -> (u32, u32, f32) {
    let mut best = (0, 0, -1.0);
    for (x, y, _) in interior(img.width, 1.5) {
        let l = lum(px(img, x, y));
        if l > best.2 {
            best = (x, y, l);
        }
    }
    best
}

/// Pixel where a mirror-like lobe peaks for `light`: the interior pixel whose normal is
/// closest to the half vector between the view and light directions.
fn mirror_pixel(size: u32, light: V3) -> (u32, u32) {
    let mut best = (0, 0, -1.0);
    for (x, y, h) in interior(size, 1.5) {
        let l = norm(sub(light, h.p));
        if dot(h.n, l) <= 0.0 {
            continue;
        }
        let nh = dot(h.n, norm(add(l, h.v)));
        if nh > best.2 {
            best = (x, y, nh);
        }
    }
    (best.0, best.1)
}

// ---- snapshots (policy step b) ---------------------------------------------------------

fn srgb8(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 {
        12.92 * v
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

/// Compares `img` with the stored snapshot `name`, or stores it when `SPEC_5_1_BLESS=1`.
/// Called only after the analytic assertions on `img` have passed.
fn snapshot(ctx: &GpuContext, name: &str, img: &HdrImage) {
    let bytes: Vec<u8> = img
        .pixels
        .iter()
        .flat_map(|p| [srgb8(p[0]), srgb8(p[1]), srgb8(p[2]), 255])
        .collect();
    let path = format!(
        "{}/tests/fixtures/spec_5_1/{name}.rgba8",
        env!("CARGO_MANIFEST_DIR")
    );
    if std::env::var("SPEC_5_1_BLESS").as_deref() == Ok("1") {
        std::fs::write(&path, &bytes).expect("write snapshot");
        println!("blessed {path}");
        return;
    }
    let want = std::fs::read(&path).unwrap_or_else(|e| {
        panic!("snapshot {path} missing ({e}); run once with SPEC_5_1_BLESS=1")
    });
    assert_eq!(want.len(), bytes.len(), "snapshot {name}: size changed");
    // Per-backend tolerance: the snapshots were stored on lavapipe (llvmpipe); other
    // rasterisers differ at silhouettes and in the last bits of the shading.
    let info = ctx.adapter_info();
    let same = info.name.to_lowercase().contains("llvmpipe");
    let (max_mean, max_outliers) = if same { (0.5, 0.005) } else { (2.0, 0.03) };
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

/// The standard golden scene: one white light up and to the right of the camera, neutral
/// grey background, studio environment, full white GI.
const KEY_LIGHT: V3 = [0.5, 0.25, 0.3];

fn key_light() -> PointLight {
    // Irradiance π at the sphere centre → a white Lambert surface facing it shows ≈ 1.
    let d2 = dot(KEY_LIGHT, KEY_LIGHT);
    PointLight::new(KEY_LIGHT, [1.0, 1.0, 1.0], PI * d2, 10.0)
}

fn golden(scene: &mut Scene, kind: MaterialKind, emission: f32) -> HdrImage {
    let mut item = scene.kind(kind);
    item.emission = emission;
    scene.render_env(
        &[item],
        &[key_light()],
        [1.0, 1.0, 1.0, 1.0],
        GREY,
        Environment::STUDIO,
    )
}

// ---- criterion 5: energy sanity ------------------------------------------------------------

/// A white Lambert-only sphere under a unit light matches the analytic Lambert radiance
/// pixel by pixel and never exceeds the analytic maximum I / (π d_min²).
#[test]
fn white_lambert_under_a_unit_light_matches_the_analytic_radiance() {
    let Some(mut scene) = Scene::new(SIZE) else {
        return;
    };
    let white = scene.item(MaterialParams::lambert([1.0, 1.0, 1.0]));
    for light_pos in [
        [0.5, 0.0, 0.0],
        [0.4, 0.3, 0.2],
        [0.1, -0.35, 0.3],
        [0.2, 0.0, -0.5],
    ] {
        let light = PointLight::new(light_pos, [1.0, 1.0, 1.0], 1.0, 1000.0);
        let img = scene.render(&[white], &[light], NO_GI, BLACK);
        assert_background(&img, BLACK);

        let d_min = len(light_pos) - R;
        let bound = 1.0 / (PI * d_min * d_min);
        let mut peak = 0.0f32;
        for p in &img.pixels {
            for c in 0..3 {
                assert!(
                    p[c] <= bound * 1.01,
                    "light {light_pos:?}: pixel {p:?} exceeds the analytic maximum {bound}"
                );
                peak = peak.max(p[c]);
            }
        }
        let mut compared = 0;
        let mut lit_seen = false;
        for (x, y, h) in interior(SIZE, 3.0) {
            let want = lambert(&h, &[light]);
            let got = px(&img, x, y);
            let cos = dot(h.n, norm(sub(light_pos, h.p)));
            if cos < -0.08 {
                // Past the terminator: no light, no ambient.
                assert!(
                    lum(got) <= 0.002 * bound,
                    "({x},{y}) behind the terminator is lit: {got:?}"
                );
                continue;
            }
            if cos < 0.1 {
                continue; // terminator band: tessellation dominates
            }
            lit_seen = true;
            for c in 0..3 {
                assert!(
                    (got[c] - want[c]).abs() <= 0.04 * want[c] + 0.003 * bound,
                    "light {light_pos:?} pixel ({x},{y}) channel {c}: got {} want {} (cosθ {cos})",
                    got[c],
                    want[c]
                );
            }
            // White light on a white surface stays neutral.
            assert!(
                (got[0] - got[1]).abs() <= 0.01 * got[0] + 1e-4
                    && (got[1] - got[2]).abs() <= 0.01 * got[1] + 1e-4
            );
            compared += 1;
        }
        assert!(
            lit_seen && compared > 200,
            "light {light_pos:?}: only {compared} pixels compared"
        );
        assert!(peak > 0.0);
    }
}

/// Lambert reflection is linear: intensity, colour and albedo scale the image.
#[test]
fn lambert_radiance_is_linear_in_intensity_colour_and_albedo() {
    let Some(mut scene) = Scene::new(64) else {
        return;
    };
    let pos = [0.4, 0.2, 0.2];
    let white = scene.item(MaterialParams::lambert([1.0, 1.0, 1.0]));
    let base = scene.render(
        &[white],
        &[PointLight::new(pos, [1.0; 3], 1.0, 1000.0)],
        NO_GI,
        BLACK,
    );
    let tinted_light = scene.render(
        &[white],
        &[PointLight::new(pos, [1.0, 0.5, 0.25], 3.0, 1000.0)],
        NO_GI,
        BLACK,
    );
    let albedo = scene.item(MaterialParams::lambert([0.2, 0.6, 0.9]));
    let tinted_surface = scene.render(
        &[albedo],
        &[PointLight::new(pos, [1.0; 3], 1.0, 1000.0)],
        NO_GI,
        BLACK,
    );
    let m = sphere_mean(&base, 3.0);
    let a = sphere_mean(&tinted_light, 3.0);
    let b = sphere_mean(&tinted_surface, 3.0);
    assert!(m[0] > 0.0);
    for (c, (kl, ks)) in [(3.0, 0.2), (1.5, 0.6), (0.75, 0.9)]
        .into_iter()
        .enumerate()
    {
        assert!(
            (a[c] - kl * m[c]).abs() <= 0.01 * kl * m[c],
            "light colour/intensity, channel {c}: {} vs {}",
            a[c],
            kl * m[c]
        );
        assert!(
            (b[c] - ks * m[c]).abs() <= 0.01 * ks * m[c],
            "albedo, channel {c}: {} vs {}",
            b[c],
            ks * m[c]
        );
    }
}

/// A light whose range ends before the sphere contributes nothing (the range is what the
/// clustering culls by, so it must be a hard zero).
#[test]
fn a_light_out_of_range_contributes_nothing() {
    let Some(mut scene) = Scene::new(64) else {
        return;
    };
    let white = scene.item(MaterialParams::lambert([1.0, 1.0, 1.0]));
    let light = PointLight::new([0.5, 0.0, 0.0], [1.0; 3], 100.0, 0.4);
    let img = scene.render(&[white], &[light], NO_GI, BLACK);
    for p in &img.pixels {
        assert!(lum(*p) <= 1e-4, "out-of-range light lit a pixel: {p:?}");
    }
}

/// Chrome: a specular highlight at the mirror direction, a dark body, no colour cast.
#[test]
fn chrome_shows_a_highlight_at_the_mirror_direction() {
    let Some(mut scene) = Scene::new(SIZE) else {
        return;
    };
    let chrome = scene.kind(MaterialKind::ChromeSteel);
    assert!(!chrome.transparent);
    for light_pos in [
        [0.5, 0.25, 0.3],
        [0.5, -0.3, 0.1],
        [0.3, 0.1, -0.4],
        [0.6, 0.0, 0.0],
    ] {
        let light = PointLight::new(light_pos, [1.0; 3], 1.0, 1000.0);
        let img = scene.render(&[chrome], &[light], NO_GI, BLACK);
        assert_background(&img, BLACK);
        let (bx, by, peak) = brightest(&img);
        let (mx, my) = mirror_pixel(SIZE, light_pos);
        let off = ((bx as f32 - mx as f32).powi(2) + (by as f32 - my as f32).powi(2)).sqrt();
        assert!(
            peak > 0.0,
            "light {light_pos:?}: chrome shows no highlight at all"
        );
        assert!(
            off <= 2.5,
            "light {light_pos:?}: brightest pixel ({bx},{by}) is {off:.1} px from the mirror direction ({mx},{my})"
        );
        // Sharp: brighter than white Lambert could ever be, and the body is dark.
        let d_min = len(light_pos) - R;
        let lambert_max = 1.0 / (PI * d_min * d_min);
        assert!(
            peak > 3.0 * lambert_max,
            "highlight {peak} is not specular (Lambert max {lambert_max})"
        );
        let mut lums: Vec<f32> = interior(SIZE, 2.0)
            .iter()
            .map(|(x, y, _)| lum(px(&img, *x, *y)))
            .collect();
        lums.sort_by(f32::total_cmp);
        let median = lums[lums.len() / 2];
        assert!(
            median <= 0.02 * peak,
            "chrome body median {median} vs highlight {peak}: not a dark body"
        );
        // No colour cast in the highlight under white light.
        let p = px(&img, bx, by);
        let (lo, hi) = (p[0].min(p[1]).min(p[2]), p[0].max(p[1]).max(p[2]));
        assert!(lo >= 0.85 * hi, "chrome highlight has a colour cast: {p:?}");
        // The highlight falls off monotonically-ish: 12 px away it is far below the peak.
        for (dx, dy) in [(12i32, 0i32), (-12, 0), (0, 12), (0, -12)] {
            let (x, y) = (bx as i32 + dx, by as i32 + dy);
            if center_dist(SIZE, x as u32, y as u32) < silhouette_px(SIZE) - 2.0 {
                assert!(
                    lum(px(&img, x as u32, y as u32)) < 0.25 * peak,
                    "highlight is not sharp"
                );
            }
        }
    }
}

/// Chrome mirrors its environment: in a uniform white environment with no lights the
/// whole ball is bright and neutral; in a black one it is black.
#[test]
fn chrome_reflects_the_environment() {
    let Some(mut scene) = Scene::new(64) else {
        return;
    };
    let chrome = scene.kind(MaterialKind::ChromeSteel);
    let dark = scene.render(&[chrome], &[], NO_GI, BLACK);
    assert!(
        lum([
            sphere_mean(&dark, 2.0)[0],
            sphere_mean(&dark, 2.0)[1],
            sphere_mean(&dark, 2.0)[2],
            1.0
        ]) < 1e-3
    );
    let env = Environment {
        sky: [1.0; 3],
        horizon: [1.0; 3],
        ground: [1.0; 3],
    };
    let img = scene.render_env(&[chrome], &[], NO_GI, BLACK, env);
    let m = sphere_mean(&img, 2.0);
    assert!(
        m[0] > 0.4 && m[0] <= 1.02,
        "chrome in a unit white environment: mean {m:?}"
    );
    assert!(
        m[0].min(m[1]).min(m[2]) >= 0.85 * m[0].max(m[1]).max(m[2]),
        "colour cast: {m:?}"
    );
    for (x, y, _) in interior(64, 2.0) {
        let p = px(&img, x, y);
        assert!(
            p[0] <= 1.02 && p[1] <= 1.02 && p[2] <= 1.02,
            "({x},{y}) reflects more than it receives: {p:?}"
        );
    }
}

/// UV emissive: dark with tint 0, brightens with the GI tint, in the tint's colour.
#[test]
fn uv_emissive_responds_to_the_gi_tint() {
    let Some(mut scene) = Scene::new(64) else {
        return;
    };
    let uv = scene.kind(MaterialKind::UvEmissive);
    // No lights, no environment: only the GI response can show.
    let off = scene.render(&[uv], &[], [0.0, 0.0, 0.0, 0.0], BLACK);
    let off_rgb_only = scene.render(&[uv], &[], [1.0, 1.0, 1.0, 0.0], BLACK);
    let off_int_only = scene.render(&[uv], &[], [0.0, 0.0, 0.0, 1.0], BLACK);
    for (name, img) in [
        ("tint 0", &off),
        ("intensity 0", &off_rgb_only),
        ("colour 0", &off_int_only),
    ] {
        let m = sphere_mean(img, 2.0);
        assert!(
            m[0].max(m[1]).max(m[2]) <= 1e-3,
            "UV emissive with {name} is not dark: {m:?}"
        );
        assert_background(img, BLACK);
    }
    let half = sphere_mean(&scene.render(&[uv], &[], [1.0, 1.0, 1.0, 0.5], BLACK), 2.0);
    let full_img = scene.render(&[uv], &[], [1.0, 1.0, 1.0, 1.0], BLACK);
    assert_background(&full_img, BLACK);
    let full = sphere_mean(&full_img, 2.0);
    let l = |m: V3| lum([m[0], m[1], m[2], 1.0]);
    assert!(
        l(full) > 0.05,
        "UV emissive under full GI barely glows: {full:?}"
    );
    assert!(
        l(half) > 0.01 && l(full) > 1.5 * l(half),
        "glow does not grow with GI intensity: half {half:?} full {full:?}"
    );
    // The glow takes the tint colour.
    for (tint, main) in [
        ([1.0, 0.0, 0.0, 1.0], 0usize),
        ([0.0, 1.0, 0.0, 1.0], 1),
        ([0.0, 0.0, 1.0, 1.0], 2),
    ] {
        let m = sphere_mean(&scene.render(&[uv], &[], tint, BLACK), 2.0);
        assert!(
            m[main] > 0.02,
            "tint {tint:?}: no glow in channel {main}: {m:?}"
        );
        for c in 0..3 {
            if c != main {
                assert!(
                    m[c] <= 0.1 * m[main],
                    "tint {tint:?}: glow is not tint-coloured: {m:?}"
                );
            }
        }
    }
    // Every glowing pixel, not only the mean: the ball glows all over (emission is not
    // a lighting term, so there is no terminator).
    for (x, y, _) in interior(64, 2.0) {
        assert!(
            lum(px(&full_img, x, y)) > 0.3 * l(full),
            "({x},{y}) does not glow"
        );
    }
    // In a lit scene the tint still adds light.
    let key = [key_light()];
    let lit_off = sphere_mean(&scene.render(&[uv], &key, [1.0, 1.0, 1.0, 0.0], BLACK), 2.0);
    let lit_on = sphere_mean(&scene.render(&[uv], &key, [1.0, 1.0, 1.0, 1.0], BLACK), 2.0);
    assert!(
        l(lit_on) > l(lit_off) + 0.5 * l(full),
        "lit: GI on {lit_on:?} vs off {lit_off:?}"
    );
}

// ---- criterion 3: light count ----------------------------------------------------------------

/// Deterministic scatter of `n` lights on the camera side of the sphere, with ranges that
/// partly cut through the sphere (so cluster lists differ across the image).
fn scattered_lights(n: usize) -> Vec<PointLight> {
    let mut s = 0x1234_5678u32;
    let mut rnd = || {
        s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (s >> 8) as f32 / (1u32 << 24) as f32
    };
    (0..n)
        .map(|i| {
            let pos = [0.12 + 0.4 * rnd(), -0.4 + 0.8 * rnd(), -0.4 + 0.8 * rnd()];
            let d = len(pos);
            let range = if i % 3 == 0 {
                d * (0.9 + 0.3 * rnd())
            } else {
                d * (1.5 + 3.0 * rnd())
            };
            let color = [0.3 + 0.7 * rnd(), 0.3 + 0.7 * rnd(), 0.3 + 0.7 * rnd()];
            PointLight::new(pos, color, 0.002 + 0.004 * rnd(), range)
        })
        .collect()
}

fn assert_matches_lambert(img: &HdrImage, lights: &[PointLight], what: &str) {
    let mut scale_ref = 0.0f32;
    let pts = interior(img.width, 3.0);
    for (_, _, h) in &pts {
        let w = lambert(h, lights);
        scale_ref = scale_ref.max(w[0]).max(w[1]).max(w[2]);
    }
    assert!(scale_ref > 0.0);
    for (x, y, h) in &pts {
        let want = lambert(h, lights);
        let got = px(img, *x, *y);
        for c in 0..3 {
            assert!(
                (got[c] - want[c]).abs() <= 0.04 * want[c] + 0.01 * scale_ref,
                "{what}: pixel ({x},{y}) channel {c}: got {} want {} (a light is missing, doubled or cut at a cluster boundary)",
                got[c],
                want[c]
            );
        }
    }
}

/// 128 lights render: the image equals the analytic sum of all 128.
#[test]
fn all_128_lights_contribute() {
    let Some(mut scene) = Scene::new(SIZE) else {
        return;
    };
    let white = scene.item(MaterialParams::lambert([1.0, 1.0, 1.0]));
    let lights = scattered_lights(MAX_LIGHTS);
    let img = scene.render(&[white], &lights, NO_GI, BLACK);
    assert_matches_lambert(&img, &lights, "128 lights");
    let stats = scene.renderer.stats();
    assert_eq!(stats.frame_lights, 128);
    assert_eq!(stats.frame_lights_rejected, 0);
    assert_eq!(stats.lights_rejected, 0);
    assert!(stats.max_cluster_lights as usize <= MAX_LIGHTS);
    // Each light matters: dropping any single one of a few changes the prediction by more
    // than the comparison tolerance somewhere, so the check above is sensitive to it.
    // Verified directly on the image for the last light (index 127):
    let img127 = scene.render(&[white], &lights[..127], NO_GI, BLACK);
    assert_matches_lambert(&img127, &lights[..127], "127 lights");
    let mut differs = false;
    for (x, y, _) in interior(SIZE, 3.0) {
        if (lum(px(&img, x, y)) - lum(px(&img127, x, y))).abs() > 1e-5 {
            differs = true;
        }
    }
    assert!(differs, "light 128 (index 127) contributes nothing");
}

/// 128 identical lights of intensity I/128 look like one light of intensity I.
#[test]
fn co_located_lights_add_up() {
    let Some(mut scene) = Scene::new(64) else {
        return;
    };
    let white = scene.item(MaterialParams::lambert([1.0, 1.0, 1.0]));
    let pos = [0.4, 0.2, 0.25];
    let one = scene.render(
        &[white],
        &[PointLight::new(pos, [1.0; 3], 1.0, 1000.0)],
        NO_GI,
        BLACK,
    );
    let many = vec![PointLight::new(pos, [1.0; 3], 1.0 / 128.0, 1000.0); MAX_LIGHTS];
    let sum = scene.render(&[white], &many, NO_GI, BLACK);
    let peak = brightest(&one).2;
    for (x, y, _) in interior(64, 2.0) {
        let (a, b) = (px(&one, x, y), px(&sum, x, y));
        for c in 0..3 {
            assert!(
                (a[c] - b[c]).abs() <= 0.01 * a[c] + 0.002 * peak,
                "({x},{y}): one {a:?} vs 128 {b:?}"
            );
        }
    }
}

/// A 129th light is ignored with a counter, without a panic, and does not change the image.
#[test]
fn light_129_is_ignored_with_a_counter() {
    let Some(mut scene) = Scene::new(SIZE) else {
        return;
    };
    let white = scene.item(MaterialParams::lambert([1.0, 1.0, 1.0]));
    let mut lights = scattered_lights(MAX_LIGHTS);
    let img128 = scene.render(&[white], &lights, NO_GI, BLACK);
    assert_eq!(scene.renderer.stats().lights_rejected, 0);
    // A very bright green light right in front of the sphere: unmistakable if it rendered.
    lights.push(PointLight::new(
        [0.3, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        100.0,
        1000.0,
    ));
    let img129 = scene.render(&[white], &lights, NO_GI, BLACK);
    let stats = scene.renderer.stats();
    assert_eq!(stats.frame_lights, 128);
    assert_eq!(
        stats.frame_lights_rejected, 1,
        "the 129th light was not counted"
    );
    assert_eq!(stats.lights_rejected, 1);
    assert!(stats.max_cluster_lights as usize <= MAX_LIGHTS);
    assert_matches_lambert(
        &img129,
        &lights[..MAX_LIGHTS],
        "129 lights (the first 128 expected)",
    );
    for (a, b) in img128.pixels.iter().zip(&img129.pixels) {
        for c in 0..3 {
            assert!(
                (a[c] - b[c]).abs() <= 1e-4 + 1e-3 * a[c],
                "the 129th light changed the image: {a:?} vs {b:?}"
            );
        }
    }
    // Many more: still no panic, counters accumulate, and a later frame resets the per-frame count.
    let mut many = lights.clone();
    many.extend(std::iter::repeat_n(
        PointLight::new([0.3, 0.0, 0.0], [1.0, 0.0, 0.0], 100.0, 1000.0),
        499,
    ));
    let _ = scene.render(&[white], &many, NO_GI, BLACK);
    let stats = scene.renderer.stats();
    assert_eq!(stats.frame_lights_rejected, 500);
    assert_eq!(stats.lights_rejected, 501);
    let _ = scene.render(&[white], &lights[..5], NO_GI, BLACK);
    let stats = scene.renderer.stats();
    assert_eq!(stats.frame_lights, 5);
    assert_eq!(stats.frame_lights_rejected, 0);
    assert_eq!(stats.lights_rejected, 501);
}

/// No banding at cluster boundaries: under one smooth light the Lambert image is smooth —
/// neighbouring interior pixels never jump by more than the analytic gradient allows.
#[test]
fn no_discontinuities_across_clusters() {
    let Some(mut scene) = Scene::new(SIZE) else {
        return;
    };
    let white = scene.item(MaterialParams::lambert([1.0, 1.0, 1.0]));
    // Ranges chosen so the sphere straddles the light's cut-off region as well.
    for range in [1000.0, 0.62, 0.5] {
        let light = PointLight::new([0.45, 0.1, 0.15], [1.0; 3], 1.0, range);
        let img = scene.render(&[white], &[light], NO_GI, BLACK);
        let pts = interior(SIZE, 4.0);
        let peak = pts
            .iter()
            .map(|(_, _, h)| lambert(h, &[light])[0])
            .fold(0.0, f32::max);
        if peak <= 0.0 {
            continue;
        }
        for (x, y, h) in &pts {
            let here = px(&img, *x, *y)[0];
            let want_here = lambert(h, &[light])[0];
            for (nx, ny) in [(*x + 1, *y), (*x, *y + 1)] {
                let Some(nh) = hit(SIZE, nx, ny) else {
                    continue;
                };
                if center_dist(SIZE, nx, ny) >= silhouette_px(SIZE) - 4.0 {
                    continue;
                }
                let want_step = (lambert(&nh, &[light])[0] - want_here).abs();
                let step = (px(&img, nx, ny)[0] - here).abs();
                assert!(
                    step <= want_step + 0.03 * peak,
                    "range {range}: jump of {step} between ({x},{y}) and ({nx},{ny}), analytic {want_step}, peak {peak}"
                );
            }
        }
    }
}

// ---- materials: analytic checks, then snapshots (criterion 2) ----------------------------------

/// A light in the vertical plane through the camera axis gives a left-right symmetric image.
#[test]
fn every_material_is_left_right_symmetric_under_a_centred_light() {
    let Some(mut scene) = Scene::new(64) else {
        return;
    };
    let light = PointLight::new([0.5, 0.0, 0.3], [1.0; 3], PI * 0.34, 10.0);
    for kind in [
        MaterialKind::ChromeSteel,
        MaterialKind::CeramicPearl,
        MaterialKind::UvEmissive,
        MaterialKind::PlayfieldWood,
        MaterialKind::LightInsert,
        MaterialKind::ClearAcrylic,
    ] {
        let mut item = scene.kind(kind);
        item.emission = 1.0;
        let img = scene.render_env(&[item], &[light], [1.0; 4], GREY, Environment::STUDIO);
        assert_background(&img, GREY);
        let mut peak = 0.0f32;
        for (x, y, _) in interior(64, 3.0) {
            peak = peak.max(lum(px(&img, x, y)));
        }
        let (mut sum, mut n) = (0.0f64, 0);
        for (x, y, _) in interior(64, 3.0) {
            let (a, b) = (px(&img, x, y), px(&img, 63 - x, y));
            sum += (lum(a) - lum(b)).abs() as f64;
            n += 1;
        }
        let mean = sum / n as f64;
        assert!(
            mean <= 0.01 * peak as f64 + 1e-4,
            "{kind:?}: mean left-right difference {mean} (peak {peak})"
        );
    }
}

fn mirror_highlight_offset(img: &HdrImage) -> f32 {
    let (bx, by, _) = brightest(img);
    let (mx, my) = mirror_pixel(img.width, KEY_LIGHT);
    ((bx as f32 - mx as f32).powi(2) + (by as f32 - my as f32).powi(2)).sqrt()
}

#[test]
fn golden_chrome_steel() {
    let Some(mut scene) = Scene::new(64) else {
        return;
    };
    let img = golden(&mut scene, MaterialKind::ChromeSteel, 0.0);
    assert_background(&img, GREY);
    // Sharp bright highlight at the mirror direction, well above the body.
    let (_, _, peak) = brightest(&img);
    assert!(
        mirror_highlight_offset(&img) <= 2.5,
        "highlight is off the mirror direction"
    );
    let m = sphere_mean(&img, 2.0);
    assert!(
        peak > 5.0 * lum([m[0], m[1], m[2], 1.0]),
        "highlight {peak} vs body {m:?}"
    );
    // No colour cast in a neutral scene.
    assert!(
        m[0].min(m[1]).min(m[2]) >= 0.85 * m[0].max(m[1]).max(m[2]),
        "colour cast: {m:?}"
    );
    snapshot(&scene.ctx, "chrome_steel", &img);
}

#[test]
fn golden_ceramic_pearl() {
    let Some(mut scene) = Scene::new(64) else {
        return;
    };
    let img = golden(&mut scene, MaterialKind::CeramicPearl, 0.0);
    assert_background(&img, GREY);
    // Bright, near-white: the lit side reflects most of what white Lambert would (≈ 1).
    let lit: Vec<(u32, u32, Hit)> = interior(64, 3.0)
        .into_iter()
        .filter(|(_, _, h)| dot(h.n, norm(sub(KEY_LIGHT, h.p))) > 0.8)
        .collect();
    assert!(lit.len() > 20);
    let mut m = [0.0f32; 3];
    for (x, y, _) in &lit {
        let p = px(&img, *x, *y);
        for c in 0..3 {
            m[c] += p[c] / lit.len() as f32;
        }
    }
    assert!(m[1] > 0.5, "ceramic lit side is not bright: {m:?}");
    assert!(
        m[0].min(m[1]).min(m[2]) >= 0.85 * m[0].max(m[1]).max(m[2]),
        "ceramic is not white: {m:?}"
    );
    // Slightly glossy: a highlight near the mirror direction that exceeds the diffuse level,
    // but soft (far below chrome's).
    let (_, _, peak) = brightest(&img);
    assert!(
        mirror_highlight_offset(&img) <= 6.0,
        "ceramic highlight is off the mirror direction"
    );
    assert!(
        peak > 1.05 * lum([m[0], m[1], m[2], 1.0]),
        "ceramic shows no highlight: peak {peak}, lit mean {m:?}"
    );
    let chrome_peak = brightest(&golden(&mut scene, MaterialKind::ChromeSteel, 0.0)).2;
    assert!(
        peak < chrome_peak,
        "ceramic highlight {peak} is not softer than chrome's {chrome_peak}"
    );
    // Brighter overall than chrome's dark body.
    snapshot(&scene.ctx, "ceramic_pearl", &img);
}

#[test]
fn golden_uv_emissive() {
    let Some(mut scene) = Scene::new(64) else {
        return;
    };
    let uv = scene.kind(MaterialKind::UvEmissive);
    let tint = [0.2, 1.0, 0.4, 1.0];
    let on = scene.render_env(&[uv], &[key_light()], tint, GREY, Environment::STUDIO);
    let off = scene.render_env(
        &[uv],
        &[key_light()],
        [0.2, 1.0, 0.4, 0.0],
        GREY,
        Environment::STUDIO,
    );
    assert_background(&on, GREY);
    assert_background(&off, GREY);
    let (a, b) = (sphere_mean(&on, 2.0), sphere_mean(&off, 2.0));
    let glow = sub(a, b);
    assert!(glow[1] > 0.05, "no glow with GI on: on {a:?} off {b:?}");
    // The added light has the tint's colour ratios (0.2 : 1 : 0.4).
    assert!(
        (glow[0] / glow[1] - 0.2).abs() < 0.08 && (glow[2] / glow[1] - 0.4).abs() < 0.08,
        "glow {glow:?} is not tint-coloured"
    );
    // Nearly black with the tint off, even in the lit studio scene.
    assert!(
        lum([b[0], b[1], b[2], 1.0]) < 0.25 * lum([a[0], a[1], a[2], 1.0]),
        "tint off is not dark: {b:?} vs {a:?}"
    );
    snapshot(&scene.ctx, "uv_emissive_on", &on);
    snapshot(&scene.ctx, "uv_emissive_off", &off);
}

#[test]
fn golden_playfield_wood() {
    let Some(mut scene) = Scene::new(64) else {
        return;
    };
    let img = golden(&mut scene, MaterialKind::PlayfieldWood, 0.0);
    assert_background(&img, GREY);
    let wood = scene.kind(MaterialKind::PlayfieldWood);
    assert!(
        wood.material.clearcoat > 0.0,
        "playfield wood has no clear coat"
    );
    // Base layer alone (clear coat off) and both, under the key light only.
    let mut bare = wood;
    bare.material.clearcoat = 0.0;
    let key = [key_light()];
    let coated_img = scene.render(&[wood], &key, NO_GI, BLACK);
    let bare_img = scene.render(&[bare], &key, NO_GI, BLACK);
    // Warm diffuse: away from the highlight R > G > B.
    let (mx, my) = mirror_pixel(64, KEY_LIGHT);
    let mut m = [0.0f32; 3];
    let mut n = 0.0;
    for (x, y, h) in interior(64, 3.0) {
        let far = (x as f32 - mx as f32).hypot(y as f32 - my as f32) > 12.0;
        if far && dot(h.n, norm(sub(KEY_LIGHT, h.p))) > 0.3 {
            let p = px(&bare_img, x, y);
            for c in 0..3 {
                m[c] += p[c];
            }
            n += 1.0;
            // The coat only attenuates the base layer away from its own highlight… but
            // it never adds energy there beyond a small lobe tail.
            let q = px(&coated_img, x, y);
            assert!(
                lum(q) <= 1.15 * lum(p) + 0.01,
                "({x},{y}): coat adds light off-highlight: {q:?} vs {p:?}"
            );
        }
    }
    assert!(n > 50.0);
    assert!(
        m[0] > 1.05 * m[1] && m[1] > 1.05 * m[2],
        "wood diffuse is not warm (R > G > B): {:?}",
        scale(m, 1.0 / n)
    );
    assert!(
        m[0] / n > 0.1,
        "wood diffuse is too dark: {:?}",
        scale(m, 1.0 / n)
    );
    // A separate glossy clear-coat highlight at the mirror direction.
    let (cx, cy, coated_peak) = brightest(&coated_img);
    let off = (cx as f32 - mx as f32).hypot(cy as f32 - my as f32);
    assert!(
        off <= 3.0,
        "clear-coat highlight ({cx},{cy}) is {off:.1} px off the mirror direction ({mx},{my})"
    );
    let bare_there = lum(px(&bare_img, cx, cy));
    assert!(
        coated_peak > 1.3 * bare_there,
        "clear coat adds no highlight: coated {coated_peak} vs bare {bare_there}"
    );
    // The coat's highlight is neutral (dielectric F0), so less saturated than the wood.
    let p = px(&coated_img, cx, cy);
    let b = px(&bare_img, cx, cy);
    let added = [p[0] - b[0], p[1] - b[1], p[2] - b[2]];
    assert!(
        added[2] > 0.5 * added[0],
        "clear-coat highlight is wood-coloured, not a separate lobe: {added:?}"
    );
    snapshot(&scene.ctx, "playfield_wood", &img);
}

#[test]
fn golden_light_insert_glows_and_warms_up() {
    let Some(mut scene) = Scene::new(64) else {
        return;
    };
    let insert = scene.kind(MaterialKind::LightInsert);
    assert!(
        insert.material.lamp_emissive > 0.0,
        "light insert has no lamp emission"
    );
    // Emission only (no lights, black environment): dark when off, glowing when lit.
    let l = |m: V3| lum([m[0], m[1], m[2], 1.0]);
    let render_level = |scene: &mut Scene, level: f32| {
        let mut it = insert;
        it.emission = level;
        sphere_mean(&scene.render(&[it], &[], NO_GI, BLACK), 2.0)
    };
    let off = render_level(&mut scene, 0.0);
    let full = render_level(&mut scene, 1.0);
    assert!(l(off) <= 1e-3, "unlit insert emits: {off:?}");
    assert!(l(full) > 0.1, "lit insert does not glow: {full:?}");
    // Glow colour is the insert's colour.
    let bc = insert.material.base_color;
    let k = full[1] / bc[1].max(1e-6);
    for c in 0..3 {
        assert!(
            (full[c] - k * bc[c]).abs() <= 0.05 * k * bc[c].max(0.05),
            "glow {full:?} is not insert-coloured {bc:?}"
        );
    }
    // Warm-up visible across frames: an incandescent lamp commanded on at tick 0, drawn
    // every 3 ms of sim time, brightens frame by frame along the first-order curve.
    let mut bank = LampBank::new();
    bank.set_kind(5, LampKind::Incandescent);
    let mut snap = FrameStateSnapshot::default();
    snap.lamps[5] = 1.0;
    bank.update(&snap);
    let mut prev = -1.0;
    let mut series = Vec::new();
    for frame in 0..=6u64 {
        snap.tick = frame * 3;
        bank.update(&snap);
        let level = bank.level(5);
        let m = l(render_level(&mut scene, level));
        // Rendered glow is proportional to the warmed-up level.
        assert!(
            (m - level * l(full)).abs() <= 0.02 * l(full),
            "frame {frame}: glow {m} vs level {level} × {}",
            l(full)
        );
        if frame > 0 {
            assert!(m > prev, "frame {frame}: glow did not grow ({prev} -> {m})");
        }
        prev = m;
        series.push(m / l(full));
    }
    assert_eq!(series[0], 0.0, "glow before any sim time has passed");
    assert!(series[3] < 0.95, "9 ms: {}", series[3]);
    assert!(series[5] >= 0.94, "15 ms: {}", series[5]);
    // An LED insert is at full glow after one tick.
    let mut led = LampBank::new();
    led.set_kind(5, LampKind::Led);
    snap.tick = 0;
    led.update(&snap);
    snap.tick = 1;
    led.update(&snap);
    assert!(led.level(5) >= 0.95);

    let lit = golden(&mut scene, MaterialKind::LightInsert, 1.0);
    let unlit = golden(&mut scene, MaterialKind::LightInsert, 0.0);
    assert_background(&lit, GREY);
    assert_background(&unlit, GREY);
    assert!(l(sphere_mean(&lit, 2.0)) > l(sphere_mean(&unlit, 2.0)) + 0.5 * l(full));
    snapshot(&scene.ctx, "light_insert_lit", &lit);
    snapshot(&scene.ctx, "light_insert_unlit", &unlit);
}

#[test]
fn golden_clear_acrylic_is_see_through() {
    let Some(mut scene) = Scene::new(64) else {
        return;
    };
    let acrylic = scene.kind(MaterialKind::ClearAcrylic);
    assert!(
        acrylic.transparent,
        "DrawItem::new(ClearAcrylic) is not in the blended pass"
    );
    // See-through: with no lights and no environment, a coloured background shows through
    // mostly unchanged (and never amplified).
    let back = [0.2, 0.6, 0.1, 1.0];
    let img = scene.render(&[acrylic], &[], NO_GI, back);
    assert_background(&img, back);
    let m = sphere_mean(&img, 6.0);
    for c in 0..3 {
        assert!(
            m[c] >= 0.5 * back[c],
            "acrylic hides the background: channel {c} {} of {}",
            m[c],
            back[c]
        );
        assert!(
            m[c] <= 1.02 * back[c],
            "acrylic amplifies the background: channel {c} {} of {}",
            m[c],
            back[c]
        );
    }
    // … and an opaque object behind it stays visible, while one in front hides it.
    let far = scene
        .renderer
        .upload_mesh(&MeshGeometry::uv_sphere(R * 0.5, 48, 24))
        .expect("upload");
    let mut glow = MaterialParams::lambert([0.0, 0.0, 0.0]);
    glow.emissive = [0.8, 0.1, 0.1];
    let mut behind = DrawItem::new(
        far,
        pin_render::pbr::math::translation([-0.2, 0.0, 0.0]),
        MaterialKind::CeramicPearl,
    );
    behind.material = glow;
    behind.transparent = false;
    // Order in the draw list must not matter (blended draws come after opaque ones).
    for draws in [[acrylic, behind], [behind, acrylic]] {
        let img = scene.render(&draws, &[], NO_GI, BLACK);
        let c = img.region_mean(30, 30, 34, 34);
        assert!(
            c[0] >= 0.4 && c[0] <= 0.82,
            "opaque emitter behind acrylic: {c:?}"
        );
        assert!(c[1] <= 0.25 * c[0], "tinted wrongly: {c:?}");
    }
    let mut front = behind;
    front.model = pin_render::pbr::math::translation([0.2, 0.0, 0.0]);
    let img = scene.render(&[acrylic, front], &[], NO_GI, BLACK);
    let c = img.region_mean(30, 30, 34, 34);
    assert!(
        (c[0] - 0.8).abs() <= 0.01 && (c[1] - 0.1).abs() <= 0.01,
        "opaque emitter in front of acrylic: {c:?}"
    );
    // A faint highlight under the key light, at the mirror direction, on top of the background.
    let lit = scene.render(&[acrylic], &[key_light()], NO_GI, back);
    let (bx, by, peak) = brightest(&lit);
    let (mx, my) = mirror_pixel(64, KEY_LIGHT);
    assert!(
        peak > lum(back) * 1.1,
        "acrylic shows no highlight: peak {peak}"
    );
    assert!(
        (bx as f32 - mx as f32).hypot(by as f32 - my as f32) <= 3.0,
        "acrylic highlight off the mirror direction"
    );
    // Faint: it is a small spot — most of the ball still shows the background.
    let mut near_back = 0;
    let pts = interior(64, 6.0);
    for (x, y, _) in &pts {
        let p = px(&lit, *x, *y);
        if (0..3).all(|c| p[c] >= 0.5 * back[c] && p[c] <= 1.3 * back[c] + 0.02) {
            near_back += 1;
        }
    }
    assert!(
        near_back * 10 >= pts.len() * 7,
        "only {near_back} of {} acrylic pixels still show the background",
        pts.len()
    );

    let img = golden(&mut scene, MaterialKind::ClearAcrylic, 0.0);
    assert_background(&img, GREY);
    snapshot(&scene.ctx, "clear_acrylic", &img);
}

/// Every material kind resolved from its shader name renders finite, non-black output in
/// the golden scene, and repeated renders are identical (deterministic frames).
#[test]
fn every_material_renders_deterministically() {
    let Some(mut scene) = Scene::new(64) else {
        return;
    };
    for name in [
        "pbr_chrome_steel",
        "pbr_ceramic_pearl",
        "pbr_uv_emissive",
        "pbr_playfield_wood",
        "pbr_light_insert",
        "pbr_clear_acrylic",
    ] {
        let kind = MaterialKind::from_shader_name(name).expect("fixed shader name");
        let a = golden(&mut scene, kind, 1.0);
        let b = golden(&mut scene, kind, 1.0);
        assert_eq!(
            a.pixels, b.pixels,
            "{name}: two renders of the same frame differ"
        );
        let m = sphere_mean(&a, 2.0);
        assert!(
            lum([m[0], m[1], m[2], 1.0]) > 0.01,
            "{name}: renders black: {m:?}"
        );
    }
}
