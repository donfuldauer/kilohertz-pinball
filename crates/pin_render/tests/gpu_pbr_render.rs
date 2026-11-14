//! PBR renderer on a real adapter (gpu-tests; Verification 2, 3 and 5 of
//! 5.1, image test policy D30: analytic pixel assertions first, then a
//! regression snapshot).
//!
//! Without an adapter every test prints a SKIP note and passes, unless
//! `PIN_REQUIRE_GPU=1`, which turns "no adapter" into a failure.
//!
//! Golden images: one sphere per material, lit by one known light in front
//! of a neutral grey background ([`golden_scene`]). Each is checked
//! analytically (finite, background exact, the material's own property)
//! and then compared with `tests/fixtures/pbr/<material>.png` (sRGB 8-bit of
//! the clamped HDR image). Tolerance, for every backend: mean absolute
//! difference ≤ 3/255 per channel and at most 2 % of pixels off by more than
//! 16/255 in any channel (silhouette rasterisation and shader precision
//! differ between lavapipe, WARP and Metal). `PIN_UPDATE_SNAPSHOTS=1`
//! (re)writes the fixtures; a missing fixture fails otherwise.
#![cfg(feature = "gpu-tests")]
// Per-channel loops (`for k in 0..3`) read clearer than zipped iterators here.
#![allow(clippy::needless_range_loop)]

use pin_render::gpu::{GpuContext, context_for_tests};
use pin_render::pbr::math::{IDENTITY, scale_translation, translation};
use pin_render::pbr::{
    DrawItem, Environment, FrameDesc, HdrImage, LampKind, LampWarmup, MAX_LIGHTS, MaterialKind,
    MaterialParams, MeshGeometry, PbrCamera, PbrRenderer, PointLight,
};
use std::path::PathBuf;
use std::sync::OnceLock;

const N: u32 = 128;
const GREY: [f32; 4] = [0.18, 0.18, 0.18, 1.0];
const SPHERE_R: f32 = 0.5;
/// The golden light: above, left and in front of the sphere.
const LIGHT_POS: [f32; 3] = [-1.5, -2.5, 2.0];
const LIGHT_I: f32 = 12.0;

fn ctx() -> Option<&'static GpuContext> {
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    CTX.get_or_init(|| context_for_tests("gpu_pbr_render"))
        .as_ref()
}

/// Camera on −y looking at the origin, z up.
fn camera() -> PbrCamera {
    PbrCamera::look_at(
        [0.0, -3.0, 0.0],
        [0.0; 3],
        [0.0, 0.0, 1.0],
        30f32.to_radians(),
        1.0,
        0.1,
        20.0,
    )
}

fn golden_light() -> PointLight {
    PointLight::new(LIGHT_POS, [1.0, 1.0, 1.0], LIGHT_I, 50.0)
}

fn lum(p: [f32; 4]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}

/// World-space ray direction through the centre of pixel (`x`, `y`).
fn pixel_ray(cam: &PbrCamera, x: u32, y: u32) -> [f32; 3] {
    let t = (0.5 * cam.fov_y).tan();
    let nx = (2.0 * (x as f32 + 0.5) / N as f32 - 1.0) * t * cam.aspect;
    let ny = (1.0 - 2.0 * (y as f32 + 0.5) / N as f32) * t;
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

/// Hit point and outward normal of the camera ray through pixel (`x`, `y`)
/// on a sphere at the origin of radius `r`, if it hits at least `margin`
/// inside the silhouette (to skip edge pixels).
fn sphere_hit(
    cam: &PbrCamera,
    x: u32,
    y: u32,
    r: f32,
    margin: f32,
) -> Option<([f32; 3], [f32; 3])> {
    let d = pixel_ray(cam, x, y);
    let o = cam.eye;
    let b = dot(o, d);
    let c = dot(o, o) - r * r;
    let disc = b * b - c;
    // Distance of the ray from the centre: sqrt(|o|² − b²) < r − margin.
    let miss = (dot(o, o) - b * b).max(0.0).sqrt();
    if disc < 0.0 || miss > r - margin {
        return None;
    }
    let t = -b - disc.sqrt();
    let p = [o[0] + t * d[0], o[1] + t * d[1], o[2] + t * d[2]];
    Some((p, norm(p)))
}

/// Pixel of a world point.
fn project(cam: &PbrCamera, p: [f32; 3]) -> (f32, f32) {
    let m = cam.view_proj();
    let c: [f32; 4] =
        core::array::from_fn(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r]);
    let (nx, ny) = (c[0] / c[3], c[1] / c[3]);
    ((nx + 1.0) * 0.5 * N as f32, (1.0 - ny) * 0.5 * N as f32)
}

struct Scene {
    renderer: PbrRenderer,
    sphere: pin_render::pbr::MeshId,
    quad: pin_render::pbr::MeshId,
}

fn scene(ctx: &GpuContext) -> Scene {
    let mut renderer = PbrRenderer::new(ctx, N, N).expect("renderer");
    let sphere = renderer
        .upload_mesh(&MeshGeometry::uv_sphere(SPHERE_R, 96, 64))
        .expect("sphere");
    let quad = renderer
        .upload_mesh(&MeshGeometry::quad(1.0))
        .expect("quad");
    Scene {
        renderer,
        sphere,
        quad,
    }
}

fn render(
    ctx: &GpuContext,
    renderer: &mut PbrRenderer,
    draws: &[DrawItem],
    lights: &[PointLight],
    env: Environment,
    gi: [f32; 4],
) -> HdrImage {
    let cam = camera();
    let mut frame = FrameDesc::new(&cam, lights, draws);
    frame.environment = env;
    frame.gi = gi;
    frame.clear = GREY;
    renderer.render_to_image(ctx, &frame).expect("render")
}

fn sphere_draw(mesh: pin_render::pbr::MeshId, material: MaterialParams) -> DrawItem {
    DrawItem {
        mesh,
        model: IDENTITY,
        material,
        transparent: false,
        emission: 0.0,
    }
}

fn assert_clean(img: &HdrImage, what: &str) {
    for (i, p) in img.pixels.iter().enumerate() {
        assert!(
            p.iter().all(|c| c.is_finite() && *c >= 0.0),
            "{what}: pixel {i} = {p:?} (NaN / negative / inf)"
        );
    }
    for (x, y) in [(0, 0), (N - 1, 0), (0, N - 1), (N - 1, N - 1)] {
        let p = img.at(x, y);
        for k in 0..3 {
            assert!(
                (p[k] - GREY[k]).abs() < 1e-3,
                "{what}: background at {x},{y} = {p:?}"
            );
        }
    }
}

/// Mean over pixels whose ray hits the sphere 2 px inside the silhouette.
fn sphere_mean(img: &HdrImage) -> [f32; 4] {
    let cam = camera();
    let mut sum = [0.0f64; 4];
    let mut n = 0;
    for y in 0..N {
        for x in 0..N {
            if sphere_hit(&cam, x, y, SPHERE_R, 0.02).is_some() {
                for (s, v) in sum.iter_mut().zip(img.at(x, y)) {
                    *s += f64::from(v);
                }
                n += 1;
            }
        }
    }
    assert!(n > 1000, "sphere covers {n} px");
    sum.map(|s| (s / f64::from(n)) as f32)
}

/// Brightest sphere pixel (luminance) and its position.
fn sphere_peak(img: &HdrImage) -> (f32, u32, u32) {
    let cam = camera();
    let mut best = (f32::MIN, 0, 0);
    for y in 0..N {
        for x in 0..N {
            if sphere_hit(&cam, x, y, SPHERE_R, 0.01).is_some() {
                let l = lum(img.at(x, y));
                if l > best.0 {
                    best = (l, x, y);
                }
            }
        }
    }
    best
}

/// Pixel of the mirror highlight of `light` on the sphere seen by the
/// camera (the normal bisects the directions to the eye and the light).
fn mirror_pixel(light: [f32; 3]) -> (f32, f32) {
    let cam = camera();
    // Iterate: the bisector depends on the surface point.
    let mut n = norm(sub(cam.eye, [0.0; 3]));
    for _ in 0..20 {
        let p = [n[0] * SPHERE_R, n[1] * SPHERE_R, n[2] * SPHERE_R];
        let h = {
            let a = norm(sub(cam.eye, p));
            let b = norm(sub(light, p));
            norm([a[0] + b[0], a[1] + b[1], a[2] + b[2]])
        };
        n = h;
    }
    project(&cam, [n[0] * SPHERE_R, n[1] * SPHERE_R, n[2] * SPHERE_R])
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/pbr")
        .join(format!("{name}.png"))
}

/// D30 (b): regression snapshot, only called after the analytic checks.
fn check_snapshot(name: &str, img: &HdrImage) {
    let got = img.to_srgb8();
    let path = fixture(name);
    if std::env::var("PIN_UPDATE_SNAPSHOTS").is_ok_and(|v| v.trim() == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::save_buffer(&path, &got, img.width, img.height, image::ColorType::Rgba8)
            .expect("write snapshot");
        println!("[{name}] snapshot written to {}", path.display());
        return;
    }
    let want = image::open(&path)
        .unwrap_or_else(|e| {
            panic!(
                "[{name}] missing snapshot {} ({e}); run with PIN_UPDATE_SNAPSHOTS=1 after the analytic checks pass",
                path.display()
            )
        })
        .to_rgba8();
    assert_eq!(
        (want.width(), want.height()),
        (img.width, img.height),
        "[{name}] size"
    );
    let want = want.into_raw();
    let mut sum = 0u64;
    let mut far = 0usize;
    for (g, w) in got.chunks(4).zip(want.chunks(4)) {
        let d: Vec<u32> = (0..3).map(|k| u32::from(g[k].abs_diff(w[k]))).collect();
        sum += u64::from(d.iter().sum::<u32>());
        if d.iter().any(|&v| v > 16) {
            far += 1;
        }
    }
    let px = (img.width * img.height) as usize;
    let mean = sum as f64 / (px * 3) as f64;
    println!("[{name}] snapshot: mean |diff| {mean:.3}/255, {far} px off by > 16/255");
    assert!(mean <= 3.0, "[{name}] mean diff {mean:.3}/255 > 3/255");
    assert!(
        far * 50 <= px,
        "[{name}] {far} of {px} px differ by > 16/255"
    );
}

// ---------------------------------------------------------------------
// Golden images, one per material.
// ---------------------------------------------------------------------

fn golden(ctx: &GpuContext, kind: MaterialKind, gi: [f32; 4], emission: f32) -> HdrImage {
    let mut sc = scene(ctx);
    let mut d = DrawItem::new(sc.sphere, IDENTITY, kind);
    d.emission = emission;
    render(
        ctx,
        &mut sc.renderer,
        &[d],
        &[golden_light()],
        Environment::STUDIO,
        gi,
    )
}

#[test]
fn golden_chrome_steel() {
    let Some(ctx) = ctx() else { return };
    let img = golden(ctx, MaterialKind::ChromeSteel, [1.0; 4], 0.0);
    assert_clean(&img, "chrome");
    // A sharp highlight at the mirror direction, far above the body.
    let (peak, px, py) = sphere_peak(&img);
    let (mx, my) = mirror_pixel(LIGHT_POS);
    assert!(
        (px as f32 + 0.5 - mx).abs() <= 2.5 && (py as f32 + 0.5 - my).abs() <= 2.5,
        "chrome peak at {px},{py}, mirror direction at {mx:.1},{my:.1}"
    );
    let mean = sphere_mean(&img);
    assert!(peak > 20.0 * lum(mean), "peak {peak} vs mean {mean:?}");
    // Sharp: 6 px away from the peak it has dropped by 10×.
    let off = lum(img.at(px.saturating_sub(6), py));
    assert!(off * 10.0 < peak, "highlight not sharp: {off} vs {peak}");
    // No colour cast (neutral steel under white light and grey environment).
    let p = img.at(px, py);
    assert!(
        (p[0] - p[2]).abs() < 0.1 * p[1],
        "cast at the highlight {p:?}"
    );
    assert!(
        (mean[0] - mean[2]).abs() < 0.1 * mean[1],
        "cast on the body {mean:?}"
    );
    // Mirror-like environment: the bright horizon band shows on the body
    // (the band reflects at the sphere's equator rim, the sky at the top).
    let (cx, cy) = project(&camera(), [0.0; 3]);
    let equator_rim = lum(img.at(cx as u32 + 32, cy as u32));
    let top = lum(img.at(cx as u32, cy as u32 - 32));
    assert!(
        equator_rim > 2.0 * top,
        "horizon {equator_rim} vs sky {top}"
    );
    check_snapshot("chrome_steel", &img);
}

#[test]
fn golden_ceramic_pearl() {
    let Some(ctx) = ctx() else { return };
    let img = golden(ctx, MaterialKind::CeramicPearl, [1.0; 4], 0.0);
    assert_clean(&img, "ceramic");
    let mean = sphere_mean(&img);
    // Bright white body.
    assert!(lum(mean) > 0.3, "ceramic mean {mean:?}");
    assert!(
        (mean[0] - mean[2]).abs() < 0.15 * mean[1],
        "ceramic tint {mean:?}"
    );
    // A soft highlight near the mirror direction: brighter than the body
    // and much broader than chrome's (still > half the peak 3 px away).
    let (peak, px, py) = sphere_peak(&img);
    let (mx, my) = mirror_pixel(LIGHT_POS);
    assert!(
        (px as f32 - mx).hypot(py as f32 - my) <= 6.0,
        "peak {px},{py} vs {mx},{my}"
    );
    assert!(peak > 1.5 * lum(mean), "peak {peak} vs mean {mean:?}");
    let chrome = golden(ctx, MaterialKind::ChromeSteel, [1.0; 4], 0.0);
    let (cpeak, _, _) = sphere_peak(&chrome);
    assert!(
        peak < cpeak,
        "ceramic highlight should be softer than chrome"
    );
    check_snapshot("ceramic_pearl", &img);
}

#[test]
fn golden_uv_emissive() {
    let Some(ctx) = ctx() else { return };
    let tint = [0.3, 0.1, 1.0, 1.0];
    let img = golden(ctx, MaterialKind::UvEmissive, tint, 0.0);
    assert_clean(&img, "uv");
    let lit = sphere_mean(&img);
    // Glows with the tint colour: blue > red > green, like the tint.
    assert!(lit[2] > lit[0] && lit[0] > lit[1], "uv colour {lit:?}");
    assert!(lit[2] > 1.0, "uv glow {lit:?}");
    check_snapshot("uv_emissive", &img);
    // Tint off: nearly black (only the faint dark base).
    let off = golden(ctx, MaterialKind::UvEmissive, [0.0; 4], 0.0);
    let dark = sphere_mean(&off);
    println!("uv lit {lit:?} dark {dark:?}");
    assert!(lum(dark) < 0.08, "uv with tint 0: {dark:?}");
    let sum = |p: [f32; 4]| p[0] + p[1] + p[2];
    assert!(sum(lit) > 10.0 * sum(dark), "lit {lit:?} vs dark {dark:?}");
}

#[test]
fn golden_playfield_wood() {
    let Some(ctx) = ctx() else { return };
    let img = golden(ctx, MaterialKind::PlayfieldWood, [1.0; 4], 0.0);
    assert_clean(&img, "wood");
    // Warm diffuse body.
    let mean = sphere_mean(&img);
    assert!(
        mean[0] > mean[1] && mean[1] > mean[2],
        "wood colour {mean:?}"
    );
    // A separate glossy clear-coat highlight: at the mirror pixel the
    // coated wood is much brighter than the same wood without the coat,
    // and the coat's peak sits at the mirror direction.
    let (mx, my) = mirror_pixel(LIGHT_POS);
    let mut bare = MaterialKind::PlayfieldWood.params();
    bare.clearcoat = 0.0;
    let mut sc = scene(ctx);
    let bare_img = render(
        ctx,
        &mut sc.renderer,
        &[sphere_draw(sc.sphere, bare)],
        &[golden_light()],
        Environment::STUDIO,
        [1.0; 4],
    );
    let at = |im: &HdrImage| lum(im.at(mx as u32, my as u32));
    assert!(
        at(&img) > 5.0 * at(&bare_img),
        "coat {} vs bare {}",
        at(&img),
        at(&bare_img)
    );
    let (_, px, py) = sphere_peak(&img);
    assert!((px as f32 + 0.5 - mx).hypot(py as f32 + 0.5 - my) <= 3.0);
    // The coat highlight is white (the base colour does not tint it).
    let p = img.at(px, py);
    assert!(p[2] > 0.7 * p[0], "coat highlight tinted {p:?}");
    check_snapshot("playfield_wood", &img);
}

#[test]
fn golden_light_insert() {
    let Some(ctx) = ctx() else { return };
    let off = golden(ctx, MaterialKind::LightInsert, [1.0; 4], 0.0);
    let on = golden(ctx, MaterialKind::LightInsert, [1.0; 4], 1.0);
    assert_clean(&off, "insert off");
    assert_clean(&on, "insert on");
    let (m_off, m_on) = (sphere_mean(&off), sphere_mean(&on));
    // Glows when lit: the emission adds base · lamp_emissive everywhere.
    let p = MaterialKind::LightInsert.params();
    for k in 0..3 {
        let added = m_on[k] - m_off[k];
        let want = p.base_color[k] * p.lamp_emissive;
        assert!(
            (added - want).abs() < 0.02 * want + 0.01,
            "ch {k}: +{added} want {want}"
        );
    }
    // Subsurface: the wrapped diffuse lights past the terminator, where a
    // Lambert plastic of the same colour is black.
    let mut lambert = p;
    lambert.subsurface = 0.0;
    let mut sc = scene(ctx);
    let lam = render(
        ctx,
        &mut sc.renderer,
        &[sphere_draw(sc.sphere, lambert)],
        &[golden_light()],
        Environment::BLACK,
        [1.0; 4],
    );
    let sss = render(
        ctx,
        &mut sc.renderer,
        &[sphere_draw(sc.sphere, p)],
        &[golden_light()],
        Environment::BLACK,
        [1.0; 4],
    );
    // Bottom-right of the sphere faces away from the light (light is up-left).
    let (cx, cy) = project(&camera(), [0.0; 3]);
    let (x, y) = ((cx + 22.0) as u32, (cy + 22.0) as u32);
    assert!(
        lum(lam.at(x, y)) < 1e-3,
        "Lambert past the terminator {:?}",
        lam.at(x, y)
    );
    assert!(lum(sss.at(x, y)) > 0.01, "wrap light {:?}", sss.at(x, y));
    // Snapshot at a low lamp level: at full level the glow saturates the
    // 8-bit image (the HDR value is ~4).
    let dim = golden(ctx, MaterialKind::LightInsert, [1.0; 4], 0.08);
    assert_clean(&dim, "insert dim");
    assert!(lum(sphere_mean(&dim)) > lum(m_off) + 0.2);
    check_snapshot("light_insert", &dim);
}

#[test]
fn light_insert_warm_up_visible_across_frames() {
    let Some(ctx) = ctx() else { return };
    let mut sc = scene(ctx);
    let mut lamp = LampWarmup::new(LampKind::Incandescent);
    let mut means = Vec::new();
    for _frame in 0..4 {
        let level = lamp.update(1.0, 0.004); // 4 ms of sim time per frame
        let mut d = DrawItem::new(sc.sphere, IDENTITY, MaterialKind::LightInsert);
        d.emission = level;
        let img = render(
            ctx,
            &mut sc.renderer,
            &[d],
            &[golden_light()],
            Environment::BLACK,
            [1.0; 4],
        );
        means.push(lum(sphere_mean(&img)));
    }
    for w in means.windows(2) {
        assert!(
            w[1] > w[0] * 1.02,
            "insert must brighten frame to frame: {means:?}"
        );
    }
}

#[test]
fn golden_clear_acrylic() {
    let Some(ctx) = ctx() else { return };
    let mut sc = scene(ctx);
    // A red Lambert card behind the sphere.
    let card = DrawItem {
        mesh: sc.quad,
        model: {
            // Quad faces +z; rotate it to face −y (the camera) at y = 1.
            let mut m = scale_translation(2.0, [0.0, 1.0, 0.0]);
            m[1] = [0.0, 0.0, 2.0, 0.0];
            m[2] = [0.0, -1.0, 0.0, 0.0];
            m
        },
        material: MaterialParams::lambert([0.8, 0.05, 0.05]),
        transparent: false,
        emission: 0.0,
    };
    let acrylic = DrawItem::new(sc.sphere, IDENTITY, MaterialKind::ClearAcrylic);
    assert!(acrylic.transparent);
    let lights = [golden_light()];
    let behind = render(
        ctx,
        &mut sc.renderer,
        &[card],
        &lights,
        Environment::STUDIO,
        [1.0; 4],
    );
    let img = render(
        ctx,
        &mut sc.renderer,
        &[acrylic, card],
        &lights,
        Environment::STUDIO,
        [1.0; 4],
    );
    assert_clean(&img, "acrylic");
    // See-through: at the sphere centre the red card dominates, close to
    // the card alone (two surfaces of ~8 % loss each).
    let (cx, cy) = project(&camera(), [0.0; 3]);
    let c = img.region_mean(cx as u32 - 3, cy as u32 - 3, cx as u32 + 3, cy as u32 + 3);
    let b = behind.region_mean(cx as u32 - 3, cy as u32 - 3, cx as u32 + 3, cy as u32 + 3);
    println!("acrylic centre {c:?}, card alone {b:?}");
    // (Green and blue there are the two surfaces' environment reflections.)
    assert!(
        c[0] > 3.0 * c[1] && c[0] > 3.0 * c[2],
        "not see-through: {c:?}"
    );
    assert!(
        c[0] > 0.6 * b[0] && c[0] < 1.3 * b[0],
        "transmission {c:?} vs card {b:?}"
    );
    // A faint highlight at the mirror direction, above what is behind it.
    let (mx, my) = mirror_pixel(LIGHT_POS);
    let h = img.at(mx as u32, my as u32);
    let hb = behind.at(mx as u32, my as u32);
    assert!(
        lum(h) > lum(hb) + 0.2,
        "acrylic highlight {h:?} vs behind {hb:?}"
    );
    check_snapshot("clear_acrylic", &img);
}

// ---------------------------------------------------------------------
// Energy, lights and clusters.
// ---------------------------------------------------------------------

/// Window of a light at distance `d` of range `r`.
fn window(d: f32, r: f32) -> f32 {
    let x = d / r;
    let w = (1.0 - x * x * x * x).clamp(0.0, 1.0);
    w * w
}

#[test]
fn lambert_matches_analytic_radiance_and_never_exceeds_the_bound() {
    let Some(ctx) = ctx() else { return };
    let mut sc = scene(ctx);
    let light = PointLight::new([0.3, -1.2, 0.6], [1.0; 3], 1.0, 100.0);
    let img = render(
        ctx,
        &mut sc.renderer,
        &[sphere_draw(sc.sphere, MaterialParams::lambert([1.0; 3]))],
        &[light],
        Environment::BLACK,
        [0.0; 4],
    );
    assert_clean(&img, "lambert");
    let d_min = dot(light.position, light.position).sqrt() - SPHERE_R;
    let bound = light.intensity / (core::f32::consts::PI * d_min * d_min);
    let cam = camera();
    let mut max = 0.0f32;
    let mut checked = 0;
    for y in 0..N {
        for x in 0..N {
            let p = img.at(x, y);
            if sphere_hit(&cam, x, y, SPHERE_R, 0.0).is_some() {
                max = max.max(p[0]).max(p[1]).max(p[2]);
            }
            let Some((hit, n)) = sphere_hit(&cam, x, y, SPHERE_R, 0.03) else {
                continue;
            };
            let to_l = sub(light.position, hit);
            let d = dot(to_l, to_l).sqrt();
            let cos = (dot(n, to_l) / d).max(0.0);
            let want =
                light.intensity * cos * window(d, light.range) / (core::f32::consts::PI * d * d);
            for k in 0..3 {
                assert!(
                    (p[k] - want).abs() <= 0.03 * want + 2e-3,
                    "pixel {x},{y}: {} vs analytic {want}",
                    p[k]
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 1000);
    assert!(max <= bound * 1.005, "max {max} > analytic bound {bound}");
    assert!(
        max >= 0.9 * bound,
        "max {max} far below the bound {bound} (light faces the sphere)"
    );
}

/// Many short-range lights over a Lambert floor seen at a grazing angle:
/// every pixel matches the analytic sum over all lights, so no light is
/// missing from any cluster (no seams at cluster boundaries).
#[test]
fn clustered_lights_match_the_analytic_sum_everywhere() {
    let Some(ctx) = ctx() else { return };
    let mut renderer = PbrRenderer::new(ctx, N, N).expect("renderer");
    let floor = renderer
        .upload_mesh(&MeshGeometry::quad(1.0))
        .expect("floor");
    let draw = DrawItem {
        mesh: floor,
        model: scale_translation(8.0, [0.0, 3.0, -0.5]),
        material: MaterialParams::lambert([1.0; 3]),
        transparent: false,
        emission: 0.0,
    };
    // 12 × 10 = 120 lights on a grid 5 cm above the floor.
    let mut lights = Vec::new();
    for i in 0..12 {
        for j in 0..10 {
            let x = -2.0 + 4.0 * i as f32 / 11.0;
            let y = 0.0 + 6.0 * j as f32 / 9.0;
            let c = [(i % 3) as f32 * 0.5, 1.0, (j % 2) as f32];
            lights.push(PointLight::new(
                [x, y, -0.45],
                c,
                0.02,
                0.35 + 0.05 * (i % 4) as f32,
            ));
        }
    }
    let cam = camera();
    let mut frame = FrameDesc::new(&cam, &lights, std::slice::from_ref(&draw));
    frame.environment = Environment::BLACK;
    frame.clear = GREY;
    let img = renderer.render_to_image(ctx, &frame).expect("render");
    assert!(renderer.stats().max_cluster_lights >= 2);
    let mut checked = 0;
    for y in 0..N {
        for x in 0..N {
            let d = pixel_ray(&cam, x, y);
            if d[2] >= -1e-3 {
                continue;
            }
            let t = (-0.5 - cam.eye[2]) / d[2];
            let hit = [cam.eye[0] + t * d[0], cam.eye[1] + t * d[1], -0.5];
            if hit[0].abs() > 3.9 || !(-0.9..6.9).contains(&hit[1]) || t > 19.0 {
                continue;
            }
            let mut want = [0.0f32; 3];
            for l in &lights {
                let to_l = sub(l.position, hit);
                let dist = dot(to_l, to_l).sqrt();
                if dist >= l.range {
                    continue;
                }
                let e = l.intensity * (to_l[2] / dist).max(0.0) * window(dist, l.range)
                    / (core::f32::consts::PI * dist * dist);
                for k in 0..3 {
                    want[k] += e * l.color[k];
                }
            }
            let p = img.at(x, y);
            for k in 0..3 {
                assert!(
                    (p[k] - want[k]).abs() <= 0.04 * want[k] + 0.01,
                    "pixel {x},{y} ch {k}: {} vs analytic {}",
                    p[k],
                    want[k]
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 3000, "checked {checked}");
}

#[test]
fn max_lights_render_and_the_next_one_is_counted_not_drawn() {
    let Some(ctx) = ctx() else { return };
    let mut sc = scene(ctx);
    let ring = |k: usize| {
        let a = core::f32::consts::TAU * k as f32 / MAX_LIGHTS as f32;
        PointLight::new(
            [1.2 * a.cos(), -1.0, 1.2 * a.sin()],
            [1.0, 0.8, 0.6],
            0.05,
            3.0,
        )
    };
    let mut lights: Vec<_> = (0..MAX_LIGHTS).map(ring).collect();
    let draws = [sphere_draw(sc.sphere, MaterialParams::lambert([0.8; 3]))];
    let full = render(
        ctx,
        &mut sc.renderer,
        &draws,
        &lights,
        Environment::BLACK,
        [0.0; 4],
    );
    assert_clean(&full, "128 lights");
    let st = sc.renderer.stats();
    assert_eq!(
        (st.frame_lights, st.frame_lights_rejected),
        (MAX_LIGHTS as u32, 0)
    );
    // A very bright 129th light right in front: ignored and counted.
    lights.push(PointLight::new([0.0, -1.0, 0.0], [1.0; 3], 1000.0, 10.0));
    let over = render(
        ctx,
        &mut sc.renderer,
        &draws,
        &lights,
        Environment::BLACK,
        [0.0; 4],
    );
    let st = sc.renderer.stats();
    assert_eq!(st.frame_lights_rejected, 1);
    assert_eq!(st.lights_rejected, 1);
    assert_eq!(st.frame_lights, MAX_LIGHTS as u32);
    assert_eq!(
        full.pixels, over.pixels,
        "the 129th light must not be drawn"
    );
    // Every light contributes: dropping one ring light darkens its side.
    let fewer: Vec<_> = lights[1..MAX_LIGHTS].to_vec();
    let less = render(
        ctx,
        &mut sc.renderer,
        &draws,
        &fewer,
        Environment::BLACK,
        [0.0; 4],
    );
    assert!(lum(sphere_mean(&less)) < lum(sphere_mean(&full)));
}

#[test]
fn transparent_draws_blend_back_to_front_and_resize_works() {
    let Some(ctx) = ctx() else { return };
    let mut sc = scene(ctx);
    let near = DrawItem::new(
        sc.sphere,
        translation([0.0, -0.5, 0.0]),
        MaterialKind::ClearAcrylic,
    );
    let far = DrawItem::new(
        sc.sphere,
        translation([0.0, 0.5, 0.0]),
        MaterialKind::ClearAcrylic,
    );
    let lights = [golden_light()];
    let a = render(
        ctx,
        &mut sc.renderer,
        &[near, far],
        &lights,
        Environment::STUDIO,
        [1.0; 4],
    );
    let b = render(
        ctx,
        &mut sc.renderer,
        &[far, near],
        &lights,
        Environment::STUDIO,
        [1.0; 4],
    );
    assert_clean(&a, "acrylic pair");
    assert_eq!(
        a.pixels, b.pixels,
        "blend order must not depend on draw order"
    );
    // Unknown mesh ids are skipped and counted.
    let mut bogus = near;
    bogus.mesh = {
        let mut other = PbrRenderer::new(ctx, 4, 4).unwrap();
        other.upload_mesh(&MeshGeometry::quad(1.0)).unwrap();
        other.upload_mesh(&MeshGeometry::quad(1.0)).unwrap();
        other.upload_mesh(&MeshGeometry::quad(1.0)).unwrap()
    };
    let _ = render(
        ctx,
        &mut sc.renderer,
        &[bogus],
        &lights,
        Environment::STUDIO,
        [1.0; 4],
    );
    assert_eq!(sc.renderer.stats().draws_skipped, 1);
    sc.renderer.resize(64, 32).unwrap();
    assert_eq!(sc.renderer.size(), (64, 32));
    assert!(sc.renderer.resize(0, 1).is_err());
    assert!(PbrRenderer::new(ctx, 0, 8).is_err());
    let cam = camera();
    let frame = FrameDesc::new(&cam, &lights, &[]);
    let img = sc.renderer.render_to_image(ctx, &frame).unwrap();
    assert_eq!((img.width, img.height, img.pixels.len()), (64, 32, 64 * 32));
    assert!(
        sc.renderer
            .upload_mesh_data(&[[0.0; 3]], &[], &[0, 0, 0])
            .is_err()
    );
    assert!(
        sc.renderer
            .upload_mesh_data(&[[0.0; 3]], &[[0.0; 3]], &[0, 0, 1])
            .is_err()
    );
    assert_eq!(
        sc.renderer.hdr_texture().format(),
        pin_render::pbr::HDR_FORMAT
    );
    let _ = sc.renderer.hdr_view();
    assert!(!sc.renderer.cluster_assignment().ranges().is_empty());
}

#[test]
fn many_draws_grow_the_draw_buffer() {
    let Some(ctx) = ctx() else { return };
    let mut sc = scene(ctx);
    let draws: Vec<_> = (0..40)
        .map(|i| {
            let x = -0.6 + 1.2 * (i % 8) as f32 / 7.0;
            let z = -0.6 + 1.2 * (i / 8) as f32 / 4.0;
            let mut d = DrawItem::new(
                sc.sphere,
                scale_translation(0.15, [x, 0.0, z]),
                MaterialKind::ALL[i % 5],
            );
            d.emission = 0.5;
            d
        })
        .collect();
    let img = render(
        ctx,
        &mut sc.renderer,
        &draws,
        &[golden_light()],
        Environment::STUDIO,
        [0.5, 0.5, 1.0, 1.0],
    );
    assert_clean(&img, "40 spheres");
    // Every sphere is drawn: its centre pixel differs from the background.
    for d in &draws {
        let (x, y) = project(&camera(), [d.model[3][0], d.model[3][1], d.model[3][2]]);
        let p = img.at(x as u32, y as u32);
        assert!(
            (0..3).any(|k| (p[k] - GREY[k]).abs() > 1e-3),
            "sphere at {x},{y} missing"
        );
    }
}

/// 5.1 review N1: the wrapped (subsurface) diffuse is energy-normalised.
/// Under near-uniform illumination (128 equal lights spread evenly around
/// the sphere) a white wrapped surface reads the same as a white Lambert
/// one; before the fix it read ~1.6×.
#[test]
fn wrapped_diffuse_reflects_no_more_than_lambert() {
    let Some(ctx) = ctx() else { return };
    let mut sc = scene(ctx);
    let golden = (1.0 + 5f32.sqrt()) * 0.5;
    let lights: Vec<_> = (0..MAX_LIGHTS)
        .map(|k| {
            // Fibonacci sphere of radius 3 around the ball.
            let z = 1.0 - 2.0 * (k as f32 + 0.5) / MAX_LIGHTS as f32;
            let r = (1.0 - z * z).sqrt();
            let a = core::f32::consts::TAU * k as f32 / golden;
            PointLight::new(
                [3.0 * r * a.cos(), 3.0 * r * a.sin(), 3.0 * z],
                [1.0; 3],
                0.5,
                100.0,
            )
        })
        .collect();
    let lambert = MaterialParams::lambert([1.0; 3]);
    let mut wrapped = MaterialKind::LightInsert.params();
    wrapped.base_color = [1.0, 1.0, 1.0, 1.0];
    wrapped.specular = 0.0;
    wrapped.lamp_emissive = 0.0;
    assert!(wrapped.subsurface > 0.5);
    let lam = render(
        ctx,
        &mut sc.renderer,
        &[sphere_draw(sc.sphere, lambert)],
        &lights,
        Environment::BLACK,
        [0.0; 4],
    );
    let wr = render(
        ctx,
        &mut sc.renderer,
        &[sphere_draw(sc.sphere, wrapped)],
        &lights,
        Environment::BLACK,
        [0.0; 4],
    );
    assert_clean(&wr, "wrapped");
    let (cx, cy) = project(&camera(), [0.0; 3]);
    let centre = |im: &HdrImage| {
        lum(im.region_mean(cx as u32 - 2, cy as u32 - 2, cx as u32 + 2, cy as u32 + 2))
    };
    let ratio_centre = centre(&wr) / centre(&lam);
    let ratio_mean = lum(sphere_mean(&wr)) / lum(sphere_mean(&lam));
    println!("wrapped / Lambert: centre {ratio_centre:.3}, mean {ratio_mean:.3}");
    assert!(
        (0.9..=1.05).contains(&ratio_centre),
        "centre ratio {ratio_centre}"
    );
    assert!(
        (0.9..=1.05).contains(&ratio_mean),
        "mean ratio {ratio_mean}"
    );
}

/// 5.1 review N3: non-finite or negative frame inputs (GI tint, environment,
/// clear colour) are treated as 0 instead of turning every pixel NaN/black.
#[test]
fn non_finite_frame_inputs_are_sanitized() {
    let Some(ctx) = ctx() else { return };
    let mut sc = scene(ctx);
    let d = DrawItem::new(sc.sphere, IDENTITY, MaterialKind::UvEmissive);
    let lights = [golden_light()];
    let good = render(
        ctx,
        &mut sc.renderer,
        &[d],
        &lights,
        Environment::STUDIO,
        [1.0, 1.0, 1.0, 0.0],
    );
    let bad = render(
        ctx,
        &mut sc.renderer,
        &[d],
        &lights,
        Environment::STUDIO,
        [1.0, 1.0, 1.0, f32::INFINITY],
    );
    assert_clean(&bad, "gi intensity inf");
    assert_eq!(good.pixels, bad.pixels, "inf GI intensity must act as 0");
    let nan_gi = render(
        ctx,
        &mut sc.renderer,
        &[d],
        &lights,
        Environment::STUDIO,
        [f32::NAN, 1.0, -1.0, 1.0],
    );
    assert_clean(&nan_gi, "gi NaN");
    let mut env = Environment::STUDIO;
    env.sky = [f32::NAN, f32::INFINITY, -1.0];
    let img = render(ctx, &mut sc.renderer, &[d], &lights, env, [1.0; 4]);
    assert_clean(&img, "environment NaN");
    // A non-finite clear colour becomes black.
    let cam = camera();
    let mut frame = FrameDesc::new(&cam, &lights, &[]);
    frame.clear = [f32::NAN, f32::INFINITY, -1.0, 1.0];
    let img = sc.renderer.render_to_image(ctx, &frame).unwrap();
    assert_eq!(img.at(0, 0), [0.0, 0.0, 0.0, 1.0]);
}
