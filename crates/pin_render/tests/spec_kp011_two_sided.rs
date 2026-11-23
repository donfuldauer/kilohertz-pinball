//! KP-011 / #91 Independent Tester GPU spec tests: two-sided single-winding
//! meshes (`DrawItem::double_sided` / `.with_double_sided`), under policy D30:
//! analytic pixel radiance assertions first (REQ-VIS-001, docs/SPEC.md §7.4).
#![cfg(feature = "gpu-tests")]
#![allow(clippy::needless_range_loop)]

use std::f32::consts::PI;
use std::sync::OnceLock;

use pin_render::gpu::{GpuContext, context_for_tests};
use pin_render::pbr::math::{IDENTITY, Mat4, PbrCamera};
use pin_render::pbr::{
    DrawItem, Environment, FrameDesc, HdrImage, MaterialKind, MaterialParams, MeshGeometry,
    PbrRenderer, PointLight,
};

const SIZE: u32 = 96;
const CLEAR: [f32; 4] = [0.1, 0.2, 0.3, 1.0];
const ALBEDO: [f32; 3] = [0.8, 0.6, 0.4];
const QUAD_SIZE: f32 = 1.0;

fn gpu() -> Option<&'static GpuContext> {
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    CTX.get_or_init(|| context_for_tests("spec_kp011_two_sided"))
        .as_ref()
}

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn len(a: V3) -> f32 {
    dot(a, a).sqrt()
}

fn norm(a: V3) -> V3 {
    let l = len(a);
    [a[0] / l, a[1] / l, a[2] / l]
}

fn window(d: f32, range: f32) -> f32 {
    let q = (d / range).powi(4);
    (1.0 - q).clamp(0.0, 1.0).powi(2)
}

/// World-space ray direction through the centre of pixel `(x, y)` of a square `size x size` image.
fn pixel_ray(cam: &PbrCamera, x: u32, y: u32, size: u32) -> V3 {
    let t = (0.5 * cam.fov_y).tan();
    let nx = (2.0 * (x as f32 + 0.5) / size as f32 - 1.0) * t * cam.aspect;
    let ny = (1.0 - 2.0 * (y as f32 + 0.5) / size as f32) * t;
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

/// Intersects pixel ray with the z = 0 quad of side `QUAD_SIZE` centred at origin,
/// returning `(hit_point, expected_normal)` when well inside `[-half + margin, half - margin]^2`.
fn hit_quad_interior(
    cam: &PbrCamera,
    x: u32,
    y: u32,
    size: u32,
    expected_normal: V3,
    margin: f32,
) -> Option<(V3, V3)> {
    let d = pixel_ray(cam, x, y, size);
    if d[2].abs() < 1e-4 {
        return None;
    }
    let t = -cam.eye[2] / d[2];
    if t <= 0.0 {
        return None;
    }
    let px = cam.eye[0] + t * d[0];
    let py = cam.eye[1] + t * d[1];
    let half = QUAD_SIZE * 0.5 - margin;
    if px.abs() <= half && py.abs() <= half {
        Some(([px, py, 0.0], expected_normal))
    } else {
        None
    }
}

/// Analytic Lambert radiance at `p` with shading normal `n` under `light`.
fn analytic_lambert(p: V3, n: V3, albedo: [f32; 3], light: &PointLight) -> [f32; 3] {
    let to = sub(light.position, p);
    let d = len(to);
    let cos_theta = dot(n, [to[0] / d, to[1] / d, to[2] / d]).max(0.0);
    let w = window(d, light.range);
    let irr = light.intensity * w * cos_theta / (PI * d * d);
    [
        albedo[0] * light.color[0] * irr,
        albedo[1] * light.color[1] * irr,
        albedo[2] * light.color[2] * irr,
    ]
}

fn assert_finite(img: &HdrImage, what: &str) {
    for (i, p) in img.pixels.iter().enumerate() {
        assert!(
            p.iter().all(|c| c.is_finite() && *c >= 0.0),
            "{what}: non-finite or negative pixel {i}: {p:?}"
        );
    }
}

fn assert_close_rgb(got: [f32; 4], want: [f32; 3], tol_rel: f32, tol_abs: f32, what: &str) {
    for c in 0..3 {
        let diff = (got[c] - want[c]).abs();
        let bound = tol_rel * want[c].abs() + tol_abs;
        assert!(
            diff <= bound,
            "{what} channel {c}: got {} want {} (diff {diff} > {bound})",
            got[c],
            want[c]
        );
    }
}

fn render(
    ctx: &GpuContext,
    renderer: &mut PbrRenderer,
    cam: &PbrCamera,
    lights: &[PointLight],
    draws: &[DrawItem],
) -> HdrImage {
    let mut frame = FrameDesc::new(cam, lights, draws);
    frame.environment = Environment::BLACK;
    frame.gi = [0.0; 4];
    frame.clear = CLEAR;
    let img = renderer
        .render_to_image(ctx, &frame)
        .expect("render_to_image");
    assert_finite(&img, "two-sided render");
    img
}

/// REQ-VIS-001: `DrawItem::new` defaults `double_sided` to `false`, and
/// `DrawItem::with_double_sided` sets it cleanly.
#[test]
fn req_vis_001_draw_item_double_sided_default_and_builder() {
    let Some(ctx) = gpu() else { return };
    let mut renderer = PbrRenderer::new(ctx, 16, 16).expect("PbrRenderer::new");
    let quad = renderer
        .upload_mesh(&MeshGeometry::quad(QUAD_SIZE))
        .expect("upload_mesh");

    let default_item = DrawItem::new(quad, IDENTITY, MaterialKind::CeramicPearl);
    assert!(
        !default_item.double_sided,
        "DrawItem::new must default double_sided to false"
    );
    assert!(!default_item.transparent);
    assert_eq!(default_item.emission, 0.0);

    let two_sided =
        DrawItem::new(quad, IDENTITY, MaterialKind::CeramicPearl).with_double_sided(true);
    assert!(two_sided.double_sided);

    let back_to_single = two_sided.with_double_sided(false);
    assert!(!back_to_single.double_sided);

    let transparent_two_sided =
        DrawItem::new(quad, IDENTITY, MaterialKind::ClearAcrylic).with_double_sided(true);
    assert!(transparent_two_sided.transparent);
    assert!(transparent_two_sided.double_sided);
}

/// REQ-VIS-001 (D30a analytic radiance):
/// A single-winding quad facing +z viewed from -z:
/// 1. With `double_sided: false`, back-face culling culls it -> centre equals `clear`.
/// 2. With `double_sided: true`, back face is drawn and shaded with flipped normal `[0, 0, -1]`,
///    matching analytic Lambert radiance pixel-by-pixel inside the quad.
/// 3. With a light only on the front (+z) side and camera on -z, `double_sided: true` draws
///    the quad with zero direct light (ambient/clear = 0 under Environment::BLACK), proving the
///    normal is genuinely flipped rather than `abs(dot(n, l))`.
#[test]
fn req_vis_001_single_winding_quad_seen_from_behind_matches_flipped_normal_radiance() {
    let Some(ctx) = gpu() else { return };
    let mut renderer = PbrRenderer::new(ctx, SIZE, SIZE).expect("PbrRenderer::new");
    let quad = renderer
        .upload_mesh(&MeshGeometry::quad(QUAD_SIZE))
        .expect("upload quad");

    // Camera on -z looking at the origin (+y up so the quad fills the central view).
    let cam_back = PbrCamera::look_at(
        [0.0, 0.0, -2.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        45.0f32.to_radians(),
        1.0,
        0.1,
        10.0,
    );
    let back_light = PointLight::new([0.15, 0.1, -1.0], [1.0, 1.0, 1.0], 3.0, 10.0);
    let front_light = PointLight::new([0.15, 0.1, 1.0], [1.0, 1.0, 1.0], 3.0, 10.0);

    let mut single_sided = DrawItem::new(quad, IDENTITY, MaterialKind::CeramicPearl);
    single_sided.material = MaterialParams::lambert(ALBEDO);
    single_sided.transparent = false;
    single_sided.emission = 0.0;

    let two_sided = single_sided.with_double_sided(true);

    // 1. Single-sided from behind: culled, every pixel in the centre equals CLEAR.
    let img_culled = render(
        ctx,
        &mut renderer,
        &cam_back,
        &[back_light],
        &[single_sided],
    );
    let mut interior_count = 0usize;
    for y in 0..SIZE {
        for x in 0..SIZE {
            if hit_quad_interior(&cam_back, x, y, SIZE, [0.0, 0.0, -1.0], 0.05).is_some() {
                interior_count += 1;
                let got = img_culled.at(x, y);
                assert_close_rgb(
                    got,
                    [CLEAR[0], CLEAR[1], CLEAR[2]],
                    0.0,
                    2.0e-3,
                    &format!("culled back face at ({x},{y})"),
                );
            }
        }
    }
    assert!(
        interior_count > 400,
        "quad must cover substantial interior area, got {interior_count} px"
    );

    // 2. Two-sided from behind with light on -z: matches analytic Lambert for normal [0, 0, -1].
    let img_back_lit = render(ctx, &mut renderer, &cam_back, &[back_light], &[two_sided]);
    let mut max_channel = 0.0f32;
    let mut compared = 0usize;
    for y in 0..SIZE {
        for x in 0..SIZE {
            if let Some((p, n)) = hit_quad_interior(&cam_back, x, y, SIZE, [0.0, 0.0, -1.0], 0.05) {
                let want = analytic_lambert(p, n, ALBEDO, &back_light);
                let got = img_back_lit.at(x, y);
                max_channel = max_channel.max(got[0]).max(got[1]).max(got[2]);
                assert_close_rgb(
                    got,
                    want,
                    0.02,
                    1.0e-3,
                    &format!("two-sided back face at ({x},{y})"),
                );
                compared += 1;
            }
        }
    }
    assert!(compared > 400);
    assert!(
        max_channel > 0.2,
        "two-sided back face should be brightly lit, peak={max_channel}"
    );

    // 3. Two-sided from behind with light ONLY on +z (front side):
    // Drawn (not CLEAR), and receives ZERO direct light (dot([0,0,-1], to_front_light) < 0).
    let img_front_light_only = render(ctx, &mut renderer, &cam_back, &[front_light], &[two_sided]);
    for y in 0..SIZE {
        for x in 0..SIZE {
            if hit_quad_interior(&cam_back, x, y, SIZE, [0.0, 0.0, -1.0], 0.05).is_some() {
                let got = img_front_light_only.at(x, y);
                assert_close_rgb(
                    got,
                    [0.0, 0.0, 0.0],
                    0.0,
                    1.0e-3,
                    &format!("two-sided back face under +z-only light at ({x},{y})"),
                );
            }
        }
    }
    // Corner outside the quad still shows CLEAR, verifying the quad occluded the background.
    assert_close_rgb(
        img_front_light_only.at(0, 0),
        [CLEAR[0], CLEAR[1], CLEAR[2]],
        0.0,
        2.0e-3,
        "corner clear",
    );

    // 4. Determinism: consecutive renders of the two-sided scene are bit-identical.
    let img_back_lit_repeat = render(ctx, &mut renderer, &cam_back, &[back_light], &[two_sided]);
    assert_eq!(
        img_back_lit.pixels, img_back_lit_repeat.pixels,
        "two consecutive two-sided renders must be bit-identical"
    );
}

/// REQ-VIS-001: Front face of a `double_sided: true` quad viewed from `+z` matches
/// `double_sided: false` pixel-for-pixel and matches analytic Lambert radiance.
#[test]
fn req_vis_001_front_face_double_sided_matches_single_sided_pixel_for_pixel() {
    let Some(ctx) = gpu() else { return };
    let mut renderer = PbrRenderer::new(ctx, SIZE, SIZE).expect("PbrRenderer::new");
    let quad = renderer
        .upload_mesh(&MeshGeometry::quad(QUAD_SIZE))
        .expect("upload quad");

    let cam_front = PbrCamera::look_at(
        [0.0, 0.0, 2.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        45.0f32.to_radians(),
        1.0,
        0.1,
        10.0,
    );
    let front_light = PointLight::new([-0.2, 0.15, 1.1], [1.0, 1.0, 1.0], 3.0, 10.0);

    let mut single = DrawItem::new(quad, IDENTITY, MaterialKind::CeramicPearl);
    single.material = MaterialParams::lambert(ALBEDO);
    let double = single.with_double_sided(true);

    let img_single = render(ctx, &mut renderer, &cam_front, &[front_light], &[single]);
    let img_double = render(ctx, &mut renderer, &cam_front, &[front_light], &[double]);

    assert_eq!(
        img_single.pixels, img_double.pixels,
        "double_sided: true must produce identical front-face pixels to double_sided: false"
    );

    // Analytic check on the front face as well.
    let mut checked = 0usize;
    for y in 0..SIZE {
        for x in 0..SIZE {
            if let Some((p, n)) = hit_quad_interior(&cam_front, x, y, SIZE, [0.0, 0.0, 1.0], 0.05) {
                let want = analytic_lambert(p, n, ALBEDO, &front_light);
                assert_close_rgb(
                    img_double.at(x, y),
                    want,
                    0.02,
                    1.0e-3,
                    &format!("front face at ({x},{y})"),
                );
                checked += 1;
            }
        }
    }
    assert!(checked > 400);
}

/// REQ-VIS-001: Two-sided rendering works cleanly with 4x MSAA and with mirrored
/// transforms (`det(model) < 0`), for both `double_sided: true` and `double_sided: false`.
#[test]
fn req_vis_001_msaa_4x_and_mirrored_transform_two_sided() {
    let Some(ctx) = gpu() else { return };
    let mut renderer = PbrRenderer::new(ctx, SIZE, SIZE).expect("PbrRenderer::new");
    renderer.set_msaa_samples(4).expect("set_msaa_samples(4)");
    let quad = renderer
        .upload_mesh(&MeshGeometry::quad(QUAD_SIZE))
        .expect("upload quad");

    let cam_back = PbrCamera::look_at(
        [0.0, 0.0, -2.0],
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        45.0f32.to_radians(),
        1.0,
        0.1,
        10.0,
    );
    let back_light = PointLight::new([0.0, 0.0, -1.0], [1.0, 1.0, 1.0], 2.5, 10.0);

    // Mirrored transform in X (det = -1). Because mirroring in X swaps screen winding,
    // a quad whose local normal is +z still has world normal +z, and viewed from -z it is
    // still a back face when double_sided=false, and drawn with flipped normal -z when double_sided=true.
    let mut mirrored: Mat4 = IDENTITY;
    mirrored[0][0] = -1.0;

    let mut base = DrawItem::new(quad, IDENTITY, MaterialKind::CeramicPearl);
    base.material = MaterialParams::lambert(ALBEDO);

    for model in [IDENTITY, mirrored] {
        let single = DrawItem { model, ..base };
        let double = single.with_double_sided(true);

        let img_culled = render(ctx, &mut renderer, &cam_back, &[back_light], &[single]);
        let centre_culled = img_culled.at(SIZE / 2, SIZE / 2);
        assert_close_rgb(
            centre_culled,
            [CLEAR[0], CLEAR[1], CLEAR[2]],
            0.0,
            2.0e-3,
            &format!(
                "MSAA 4x single-sided back face culled (model[0][0]={})",
                model[0][0]
            ),
        );

        let img_drawn = render(ctx, &mut renderer, &cam_back, &[back_light], &[double]);
        let mut checked = 0usize;
        for y in 0..SIZE {
            for x in 0..SIZE {
                if let Some((p, n)) =
                    hit_quad_interior(&cam_back, x, y, SIZE, [0.0, 0.0, -1.0], 0.05)
                {
                    let want = analytic_lambert(p, n, ALBEDO, &back_light);
                    assert_close_rgb(
                        img_drawn.at(x, y),
                        want,
                        0.02,
                        1.0e-3,
                        &format!(
                            "MSAA 4x double-sided (model[0][0]={}) at ({x},{y})",
                            model[0][0]
                        ),
                    );
                    checked += 1;
                }
            }
        }
        assert!(checked > 400);

        // Also verify front-only (+z) light yields zero direct light under MSAA 4x and
        // mirrored transform (det < 0), confirming `front_facing` does not invert the normal.
        let front_light = PointLight::new([0.0, 0.0, 1.0], [1.0, 1.0, 1.0], 2.5, 10.0);
        let img_front_only = render(ctx, &mut renderer, &cam_back, &[front_light], &[double]);
        for y in 0..SIZE {
            for x in 0..SIZE {
                if hit_quad_interior(&cam_back, x, y, SIZE, [0.0, 0.0, -1.0], 0.05).is_some() {
                    assert_close_rgb(
                        img_front_only.at(x, y),
                        [0.0, 0.0, 0.0],
                        0.0,
                        1.0e-3,
                        &format!(
                            "MSAA 4x double-sided +z-only light (model[0][0]={}) at ({x},{y})",
                            model[0][0]
                        ),
                    );
                }
            }
        }
    }

    // Also verify a transparent (blended) draw with `double_sided: true` renders cleanly
    // under 4x MSAA (note: ClearAcrylic always uses a two-pass back-then-front blended
    // draw per DECISIONS.md "5.2 — acrylic", so even `double_sided: false` renders its
    // back pass when seen from behind). Verify both render identically and respond to -z light.
    let mut glass_single = DrawItem::new(quad, IDENTITY, MaterialKind::ClearAcrylic);
    glass_single.double_sided = false;
    let glass_double = glass_single.with_double_sided(true);
    let img_glass_single = render(
        ctx,
        &mut renderer,
        &cam_back,
        &[back_light],
        &[glass_single],
    );
    let img_glass_double = render(
        ctx,
        &mut renderer,
        &cam_back,
        &[back_light],
        &[glass_double],
    );
    let gd = img_glass_double.at(SIZE / 2, SIZE / 2);
    let diff = (gd[0] - CLEAR[0]).abs() + (gd[1] - CLEAR[1]).abs() + (gd[2] - CLEAR[2]).abs();
    assert!(
        diff > 0.02,
        "transparent double-sided back face should draw over clear: got {gd:?} vs clear {CLEAR:?}"
    );
    assert_close_rgb(
        img_glass_single.at(SIZE / 2, SIZE / 2),
        [gd[0], gd[1], gd[2]],
        0.02,
        2.0e-3,
        "transparent double-sided matches two-pass acrylic back-face draw",
    );
}
