//! Reflection probes on a real adapter (5.2 verification 1 and 3; gpu-tests,
//! skips without an adapter unless `PIN_REQUIRE_GPU=1`).
//!
//! Image policy D30: analytic assertions first (region mean colour and
//! channel ratios over the pixels where mirror geometry puts the insert),
//! then a regression snapshot `tests/fixtures/reflections/*.png`
//! (`PIN_UPDATE_SNAPSHOTS=1` rewrites; tolerance as 5.1: mean |Δ| ≤ 3/255,
//! ≤ 2 % of pixels off by > 16/255). Images for the visual checklist are
//! written to `<target dir>/tmp/5_2_images/`.
#![cfg(feature = "gpu-tests")]

use pin_render::gpu::{GpuContext, context_for_tests};
use pin_render::pbr::math::{IDENTITY, scale_translation};
use pin_render::pbr::{
    DrawItem, Environment, FrameDesc, HdrImage, MaterialKind, MaterialParams, MeshGeometry, MeshId,
    NO_PROBE, PbrCamera, PbrRenderer, PointLight,
};
use pin_render::post::{FrameRenderer, LdrImage, PostConfig, SceneBall};
use pin_render::reflections::{
    EnvironmentReflections, MAX_PROBES, OctahedralProbes, PROBE_BUDGET, PROBE_RESOLUTION,
    ProbeConfig, ReflectionSource, ReflectiveBall, oct_encode,
};
use std::path::PathBuf;
use std::sync::OnceLock;

fn ctx() -> Option<&'static GpuContext> {
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    CTX.get_or_init(|| context_for_tests("reflections_gpu"))
        .as_ref()
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

pub fn image_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("5_2_images");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn save_ldr(name: &str, img: &LdrImage) {
    let path = image_dir().join(format!("{name}.png"));
    image::save_buffer(
        &path,
        &img.to_rgba8(),
        img.width,
        img.height,
        image::ColorType::Rgba8,
    )
    .unwrap();
    println!("[{name}] image: {}", path.display());
}

fn save_hdr(name: &str, img: &HdrImage, scale: f32) {
    let scaled = HdrImage {
        width: img.width,
        height: img.height,
        pixels: img
            .pixels
            .iter()
            .map(|p| [p[0] * scale, p[1] * scale, p[2] * scale, 1.0])
            .collect(),
    };
    let path = image_dir().join(format!("{name}.png"));
    image::save_buffer(
        &path,
        &scaled.to_srgb8(),
        img.width,
        img.height,
        image::ColorType::Rgba8,
    )
    .unwrap();
    println!("[{name}] image: {}", path.display());
}

/// D30 (b): snapshot comparison after the analytic checks.
fn check_snapshot(name: &str, img: &LdrImage) {
    let got = img.to_rgba8();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/reflections")
        .join(format!("{name}.png"));
    if std::env::var("PIN_UPDATE_SNAPSHOTS").is_ok_and(|v| v.trim() == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::save_buffer(&path, &got, img.width, img.height, image::ColorType::Rgba8).unwrap();
        println!("[{name}] snapshot written to {}", path.display());
        return;
    }
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
    println!("[{name}] snapshot: mean |diff| {mean:.3}/255, {far} px off by > 16/255");
    assert!(mean <= 3.0, "[{name}] mean diff {mean:.3}");
    assert!(far * 50 <= px, "[{name}] {far} of {px} px differ");
}

// ---------------------------------------------------------------------
// The test scene: a chrome ball on a grey playfield beside a lit red insert.
// ---------------------------------------------------------------------

const W: u32 = 256;
const BALL_R: f32 = 0.0135;
const BALL: V3 = [0.0, 0.0, BALL_R];
const INSERT_Z: f32 = 0.0004;
/// Insert rectangle (x0, x1, y0, y1).
const INSERT: [f32; 4] = [0.022, 0.052, -0.015, 0.015];

fn insert_camera() -> PbrCamera {
    PbrCamera::look_at(
        [-0.02, -0.11, 0.05],
        [0.006, 0.0, 0.012],
        [0.0, 0.0, 1.0],
        30f32.to_radians(),
        1.0,
        0.005,
        5.0,
    )
}

struct InsertScene {
    draws: Vec<DrawItem>,
    lights: Vec<PointLight>,
}

fn red_insert() -> MaterialParams {
    MaterialParams {
        base_color: [0.9, 0.08, 0.06, 1.0],
        ..MaterialKind::LightInsert.params()
    }
}

fn insert_scene(sphere: MeshId, quad: MeshId) -> InsertScene {
    let floor = DrawItem {
        mesh: quad,
        model: scale_translation(0.6, [0.0, 0.0, 0.0]),
        material: MaterialParams::lambert([0.35, 0.35, 0.35]),
        transparent: false,
        emission: 0.0,
    };
    let mut insert_model = IDENTITY;
    insert_model[0][0] = INSERT[1] - INSERT[0];
    insert_model[1][1] = INSERT[3] - INSERT[2];
    insert_model[3] = [
        0.5 * (INSERT[0] + INSERT[1]),
        0.5 * (INSERT[2] + INSERT[3]),
        INSERT_Z,
        1.0,
    ];
    let insert = DrawItem {
        mesh: quad,
        model: insert_model,
        material: red_insert(),
        transparent: false,
        emission: 1.0,
    };
    let ball = DrawItem::new(
        sphere,
        scale_translation(1.0, BALL),
        MaterialKind::ChromeSteel,
    );
    InsertScene {
        // The ball is draw 0.
        draws: vec![ball, floor, insert],
        lights: vec![PointLight::new([-0.05, -0.08, 0.25], [1.0; 3], 0.08, 5.0)],
    }
}

fn upload(pbr: &mut PbrRenderer) -> (MeshId, MeshId) {
    let sphere = pbr
        .upload_mesh(&MeshGeometry::uv_sphere(BALL_R, 64, 48))
        .unwrap();
    let quad = pbr.upload_mesh(&MeshGeometry::quad(1.0)).unwrap();
    (sphere, quad)
}

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

/// Where the mirror reflection of pixel (x, y) on the ball lands: `Some(hit
/// on the insert plane)` or `None` (misses the ball by `margin`, or the
/// reflected ray goes up).
fn reflection_hit(cam: &PbrCamera, x: u32, y: u32, margin: f32) -> Option<Option<[f32; 2]>> {
    let d = pixel_ray(cam, x, y);
    let o = sub(cam.eye, BALL);
    let b = dot(o, d);
    let c = dot(o, o) - BALL_R * BALL_R;
    let miss = (dot(o, o) - b * b).max(0.0).sqrt();
    if b * b - c < 0.0 || miss > BALL_R * (1.0 - margin) {
        return None;
    }
    let t = -b - (b * b - c).sqrt();
    let p = [
        cam.eye[0] + t * d[0],
        cam.eye[1] + t * d[1],
        cam.eye[2] + t * d[2],
    ];
    let n = norm(sub(p, BALL));
    let k = 2.0 * dot(d, n);
    let r = [d[0] - k * n[0], d[1] - k * n[1], d[2] - k * n[2]];
    if r[2] >= -1e-4 {
        return Some(None);
    }
    let s = (INSERT_Z - p[2]) / r[2];
    Some(Some([p[0] + s * r[0], p[1] + s * r[1]]))
}

fn in_insert(h: [f32; 2], shrink: f32) -> bool {
    let (wx, wy) = (INSERT[1] - INSERT[0], INSERT[3] - INSERT[2]);
    h[0] > INSERT[0] + shrink * wx
        && h[0] < INSERT[1] - shrink * wx
        && h[1] > INSERT[2] + shrink * wy
        && h[1] < INSERT[3] - shrink * wy
}

/// Mean HDR colour over the ball pixels whose reflection hits the insert
/// core, and over those reflecting the floor well away from it.
fn regions(img: &HdrImage, cam: &PbrCamera) -> ([f64; 3], usize, [f64; 3], usize) {
    let (mut red, mut nr, mut ctl, mut nc) = ([0.0f64; 3], 0, [0.0f64; 3], 0);
    for y in 0..W {
        for x in 0..W {
            let Some(hit) = reflection_hit(cam, x, y, 0.06) else {
                continue;
            };
            let p = img.at(x, y);
            match hit {
                Some(h) if in_insert(h, 0.2) => {
                    for k in 0..3 {
                        red[k] += f64::from(p[k]);
                    }
                    nr += 1;
                }
                Some(h) if !in_insert(h, -0.6) && h[0].abs() < 0.25 && h[1].abs() < 0.25 => {
                    for k in 0..3 {
                        ctl[k] += f64::from(p[k]);
                    }
                    nc += 1;
                }
                _ => {}
            }
        }
    }
    let m = |s: [f64; 3], n: usize| s.map(|v| v / n.max(1) as f64);
    (m(red, nr), nr, m(ctl, nc), nc)
}

fn scene_balls() -> [SceneBall; 1] {
    [SceneBall {
        draw: 0,
        position: BALL,
        velocity: [0.0; 3],
        radius: BALL_R,
    }]
}

/// Verification 1: the chrome ball shows the red insert where mirror
/// geometry puts it, and only there.
#[test]
fn chrome_ball_reflects_the_lit_red_insert() {
    let Some(ctx) = ctx() else { return };
    let cam = insert_camera();
    let mut fr = FrameRenderer::new(ctx, W, W, PostConfig::default()).unwrap();
    let (sphere, quad) = upload(fr.pbr_mut());
    let sc = insert_scene(sphere, quad);
    let frame = FrameDesc::new(&cam, &sc.lights, &sc.draws);
    let out = fr.render(ctx, &frame, &scene_balls()).unwrap();
    assert!(
        out.hdr
            .pixels
            .iter()
            .all(|p| p.iter().all(|c| c.is_finite() && *c >= 0.0))
    );
    let (red, nr, ctl, nc) = regions(&out.hdr, &cam);
    println!("probe: insert region {red:.3?} over {nr} px; control {ctl:.3?} over {nc} px");
    assert!(nr >= 30, "expected-red region has only {nr} px");
    assert!(nc >= 100, "control region has only {nc} px");
    assert!(red[0] > 0.8, "insert reflection too dark: {red:?}");
    assert!(
        red[0] > 4.0 * red[1].max(red[2]),
        "insert reflection not red: {red:?}"
    );
    assert!(
        ctl[0] < 1.5 * ctl[1].max(1e-3),
        "control region red: {ctl:?}"
    );
    assert_eq!(out.probes.total_updates, 1);

    // Without probes the same pixels show the grey studio environment.
    let mut env_fr = FrameRenderer::with_reflections(ctx, W, W, PostConfig::default(), |pbr| {
        Ok(Box::new(EnvironmentReflections::new(ctx, pbr)) as Box<dyn ReflectionSource>)
    })
    .unwrap();
    assert_eq!(env_fr.reflections().name(), "environment");
    let (sphere, quad) = upload(env_fr.pbr_mut());
    let sc2 = insert_scene(sphere, quad);
    let frame2 = FrameDesc::new(&cam, &sc2.lights, &sc2.draws);
    let env_out = env_fr.render(ctx, &frame2, &scene_balls()).unwrap();
    let (env_red, _, _, _) = regions(&env_out.hdr, &cam);
    println!("environment only: insert region {env_red:.3?}");
    assert!(env_red[0] < 1.5 * env_red[1], "{env_red:?}");
    assert_eq!(env_out.probes.total_updates, 0);
    assert_eq!(env_out.probes.frames, 1);

    save_ldr("reflections_chrome_insert", &out.ldr);
    save_hdr("reflections_chrome_insert_hdr", &out.hdr, 1.0);
    save_ldr("reflections_chrome_insert_no_probe", &env_out.ldr);
    check_snapshot("chrome_insert", &out.ldr);
}

/// The probe map itself: the insert direction is red, straight up is the
/// environment sky, straight down is the floor.
#[test]
fn probe_map_holds_the_surroundings() {
    let Some(ctx) = ctx() else { return };
    let cam = insert_camera();
    let mut pbr = PbrRenderer::new(ctx, 64, 64).unwrap();
    let (sphere, quad) = upload(&mut pbr);
    let sc = insert_scene(sphere, quad);
    let frame = FrameDesc::new(&cam, &sc.lights, &sc.draws);
    let mut probes = OctahedralProbes::new(ctx, &pbr, ProbeConfig::default()).unwrap();
    assert_eq!(probes.config(), ProbeConfig::default());
    assert_eq!(probes.name(), "octahedral-probes");
    probes
        .prepare(
            ctx,
            &mut pbr,
            &frame,
            &[ReflectiveBall {
                position: BALL,
                radius: BALL_R,
                draw: 0,
            }],
        )
        .unwrap();
    assert_eq!(probes.draw_layers(), &[0, NO_PROBE, NO_PROBE]);
    let map = probes.read_probe(ctx, 0).unwrap();
    assert_eq!(
        (map.width, map.height),
        (PROBE_RESOLUTION, PROBE_RESOLUTION)
    );
    save_hdr("reflections_probe_map", &map, 1.0);
    let texel = |dir: V3| {
        let uv = oct_encode(dir);
        let x = ((uv[0] * PROBE_RESOLUTION as f32) as u32).min(PROBE_RESOLUTION - 1);
        let y = ((uv[1] * PROBE_RESOLUTION as f32) as u32).min(PROBE_RESOLUTION - 1);
        map.region_mean(x.saturating_sub(1), y.saturating_sub(1), x + 2, y + 2)
    };
    let to_insert = norm(sub([0.5 * (INSERT[0] + INSERT[1]), 0.0, INSERT_Z], BALL));
    let red = texel(to_insert);
    println!("probe towards the insert {red:?}");
    assert!(red[0] > 2.0 && red[0] > 5.0 * red[1], "{red:?}");
    let up = texel([0.0, 0.0, 1.0]);
    let sky = Environment::STUDIO.sky;
    for k in 0..3 {
        assert!((up[k] - sky[k]).abs() < 0.02, "up {up:?} vs sky {sky:?}");
    }
    let down = texel([0.0, 0.0, -1.0]);
    println!("probe up {up:?}, down {down:?}");
    assert!(
        (down[0] - down[1]).abs() < 0.05 * down[0].max(1e-3),
        "floor is grey: {down:?}"
    );
    assert!(down[0] > 0.01, "the lit floor is below the ball: {down:?}");
    assert!(probes.read_probe(ctx, MAX_PROBES).is_err());
}

/// Verification 3: six balls, ≥ 60 frames, never more than two captures per
/// frame and every probe at most ⌈6/2⌉ frames old; balls without a probe
/// of their own sample the nearest captured one.
#[test]
fn six_balls_stay_within_the_probe_budget() {
    let Some(ctx) = ctx() else { return };
    let cam = PbrCamera::look_at(
        [0.0, -0.3, 0.2],
        [0.0, 0.0, 0.0],
        [0.0, 0.0, 1.0],
        40f32.to_radians(),
        1.0,
        0.01,
        5.0,
    );
    let mut fr = FrameRenderer::new(ctx, 64, 64, PostConfig::default()).unwrap();
    let (sphere, quad) = upload(fr.pbr_mut());
    let lights = [PointLight::new([0.0, -0.1, 0.4], [1.0; 3], 0.3, 5.0)];
    let mut total = 0u64;
    let frames = 64u64;
    for f in 0..frames {
        let t = f as f32 * 0.05;
        let mut balls = Vec::new();
        let mut draws = vec![DrawItem {
            mesh: quad,
            model: scale_translation(1.0, [0.0; 3]),
            material: MaterialKind::PlayfieldWood.params(),
            transparent: false,
            emission: 0.0,
        }];
        for i in 0..6 {
            let a = t + i as f32 * core::f32::consts::TAU / 6.0;
            let p = [0.08 * a.cos(), 0.08 * a.sin(), BALL_R];
            balls.push(SceneBall {
                draw: draws.len(),
                position: p,
                velocity: [-0.08 * a.sin() * 1.0, 0.08 * a.cos(), 0.0],
                radius: BALL_R,
            });
            draws.push(DrawItem::new(
                sphere,
                scale_translation(1.0, p),
                MaterialKind::ChromeSteel,
            ));
        }
        let frame = FrameDesc::new(&cam, &lights, &draws);
        let st = fr.step(ctx, &frame, &balls).unwrap();
        assert!(
            st.last_frame_updates <= PROBE_BUDGET,
            "frame {f}: {} updates",
            st.last_frame_updates
        );
        assert!(st.max_age <= 3, "frame {f}: probe age {}", st.max_age);
        total += u64::from(st.last_frame_updates);
    }
    let st = fr.reflections().stats();
    println!("six balls: {st:?}");
    assert_eq!(st.frames, frames);
    assert_eq!(st.total_updates, total);
    assert_eq!(total, 2 * frames);
    assert_eq!(st.max_frame_updates, 2);
    assert!(
        st.max_age_ever <= 2,
        "max age {} > ⌈6/2⌉ − 1",
        st.max_age_ever
    );
    assert_eq!(st.balls, 6);
}

#[test]
fn uncaptured_balls_sample_the_nearest_probe() {
    let Some(ctx) = ctx() else { return };
    let cam = insert_camera();
    let mut pbr = PbrRenderer::new(ctx, 32, 32).unwrap();
    let (sphere, _) = upload(&mut pbr);
    let positions = [
        [0.0, 0.0, BALL_R],
        [0.5, 0.0, BALL_R],
        [0.45, 0.0, BALL_R],
        [0.02, 0.0, BALL_R],
    ];
    let draws: Vec<DrawItem> = positions
        .iter()
        .map(|p| {
            DrawItem::new(
                sphere,
                scale_translation(1.0, *p),
                MaterialKind::ChromeSteel,
            )
        })
        .collect();
    // Draw order reversed against ball order.
    let balls: Vec<ReflectiveBall> = positions
        .iter()
        .enumerate()
        .map(|(i, p)| ReflectiveBall {
            position: *p,
            radius: BALL_R,
            draw: 3 - i,
        })
        .collect();
    let lights = [PointLight::new([0.0, 0.0, 0.3], [1.0; 3], 0.3, 5.0)];
    let frame = FrameDesc::new(&cam, &lights, &draws);
    let mut probes = OctahedralProbes::new(ctx, &pbr, ProbeConfig::default()).unwrap();
    probes.prepare(ctx, &mut pbr, &frame, &balls).unwrap();
    // Frame 0 captured balls 0 and 1: ball 2 (near ball 1) uses slot 1,
    // ball 3 (near ball 0) uses slot 0.
    assert_eq!(probes.draw_layers(), &[0, 1, 1, 0]);
    assert_eq!(probes.scheduler().last_updates(), &[0, 1]);
    probes.prepare(ctx, &mut pbr, &frame, &balls).unwrap();
    assert_eq!(probes.draw_layers(), &[3, 2, 1, 0]);
    assert_eq!(probes.scheduler().ages(), &[1, 1, 0, 0]);
    // A ball whose draw index is out of range is ignored.
    let odd = [ReflectiveBall {
        position: positions[0],
        radius: f32::NAN,
        draw: 99,
    }];
    probes.prepare(ctx, &mut pbr, &frame, &odd).unwrap();
    assert_eq!(probes.draw_layers(), &[NO_PROBE; 4]);
}

/// Two chrome balls side by side: each probe sees the other ball, and the
/// whole frame is deterministic.
#[test]
fn neighbouring_balls_reflect_each_other_deterministically() {
    let Some(ctx) = ctx() else { return };
    let cam = PbrCamera::look_at(
        [0.0, -0.09, 0.03],
        [0.0, 0.0, BALL_R],
        [0.0, 0.0, 1.0],
        35f32.to_radians(),
        1.0,
        0.005,
        5.0,
    );
    let mut fr = FrameRenderer::new(ctx, 128, 128, PostConfig::default()).unwrap();
    let (sphere, quad) = upload(fr.pbr_mut());
    let a = [-0.016, 0.0, BALL_R];
    let b = [0.016, 0.0, BALL_R];
    let draws = vec![
        DrawItem::new(sphere, scale_translation(1.0, a), MaterialKind::ChromeSteel),
        DrawItem::new(sphere, scale_translation(1.0, b), MaterialKind::ChromeSteel),
        DrawItem {
            mesh: quad,
            model: scale_translation(0.5, [0.0; 3]),
            material: MaterialKind::PlayfieldWood.params(),
            transparent: false,
            emission: 0.0,
        },
    ];
    let balls = [
        SceneBall {
            draw: 0,
            position: a,
            velocity: [0.0; 3],
            radius: BALL_R,
        },
        SceneBall {
            draw: 1,
            position: b,
            velocity: [0.0; 3],
            radius: BALL_R,
        },
    ];
    let lights = [PointLight::new([0.0, -0.05, 0.2], [1.0; 3], 0.1, 5.0)];
    let frame = FrameDesc::new(&cam, &lights, &draws);
    let first = fr.render(ctx, &frame, &balls).unwrap();
    let second = fr.render(ctx, &frame, &balls).unwrap();
    assert_eq!(
        first.hdr.pixels, second.hdr.pixels,
        "same frame, same image"
    );
    assert_eq!(first.ldr, second.ldr);
    assert_eq!(second.probes.total_updates, 4);
    save_ldr("reflections_two_balls", &second.ldr);
}
