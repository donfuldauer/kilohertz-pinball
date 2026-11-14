//! The HDR post chain on a real adapter (5.2 verification 2 and the
//! tone-mapping, motion-blur and anti-aliasing criteria; gpu-tests, skips
//! without an adapter unless `PIN_REQUIRE_GPU=1`).
//!
//! Inputs are mostly CPU-made HDR images (`HdrInput`), so every expected
//! value is analytic. Snapshot: `tests/fixtures/post/bloom_spot.png`
//! (`PIN_UPDATE_SNAPSHOTS=1` rewrites). Images for the visual checklist go
//! to `<target dir>/tmp/5_2_images/`.
#![cfg(feature = "gpu-tests")]
#![allow(clippy::needless_range_loop)]

use pin_render::gpu::{GpuContext, context_for_tests};
use pin_render::pbr::math::IDENTITY;
use pin_render::pbr::{
    DrawItem, Environment, FrameDesc, HdrImage, MaterialKind, MaterialParams, MeshGeometry,
    PbrCamera, PbrRenderer, PointLight,
};
use pin_render::post::{
    AaMode, BloomConfig, FrameRenderer, HdrInput, LdrImage, MotionBall, MotionBlurConfig,
    PostChain, PostConfig, PostFrame, SceneBall, ToneMap, aces_film, luminance, screen_motion,
};
use std::path::PathBuf;
use std::sync::OnceLock;

fn ctx() -> Option<&'static GpuContext> {
    static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
    CTX.get_or_init(|| context_for_tests("post_gpu")).as_ref()
}

const N: u32 = 128;

fn image_dir() -> PathBuf {
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

fn flat(w: u32, h: u32, f: impl Fn(u32, u32) -> [f32; 3]) -> HdrImage {
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let c = f(x, y);
            pixels.push([c[0], c[1], c[2], 1.0]);
        }
    }
    HdrImage {
        width: w,
        height: h,
        pixels,
    }
}

fn srgb8(x: f32) -> u8 {
    let x = x.clamp(0.0, 1.0);
    let s = if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0).round() as u8
}

fn total_lum(img: &HdrImage) -> f64 {
    img.pixels
        .iter()
        .map(|p| f64::from(luminance([p[0], p[1], p[2]])))
        .sum()
}

fn chain(ctx: &GpuContext, w: u32, h: u32, config: PostConfig) -> PostChain {
    let mut c = PostChain::new(ctx, w, h, wgpu::TextureFormat::Rgba8Unorm).unwrap();
    c.set_config(config);
    c
}

/// The ACES curve on the GPU matches the reference fit at sample points
/// (through 8-bit sRGB: ±1 code), with exposure applied first.
#[test]
fn aces_matches_the_reference_fit_at_sample_points() {
    let Some(ctx) = ctx() else { return };
    let samples = [
        0.0f32, 0.01, 0.05, 0.1, 0.18, 0.3, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0, 8.0, 16.0, 100.0,
    ];
    let img = flat(16, 4, |x, _| [samples[x as usize]; 3]);
    let input = HdrInput::from_image(ctx, &img).unwrap();
    for exposure in [1.0f32, 0.5, 2.0] {
        let cfg = PostConfig {
            exposure,
            tone_map: ToneMap::AcesFilm,
            ..PostConfig::neutral()
        };
        let mut c = chain(ctx, 16, 4, cfg);
        let cam = camera();
        let out = c
            .render_to_ldr(
                ctx,
                &input.view,
                &PostFrame {
                    camera: &cam,
                    balls: &[],
                },
            )
            .unwrap();
        for (x, &s) in samples.iter().enumerate() {
            let want = srgb8(aces_film(s * exposure));
            for y in 0..4 {
                let got = out.at(x as u32, y);
                for k in 0..3 {
                    assert!(
                        got[k].abs_diff(want) <= 1,
                        "exposure {exposure}, x = {s}: got {got:?}, want {want}"
                    );
                }
                assert_eq!(got[3], 255);
            }
        }
    }
    // The curve itself at the classic reference points.
    assert!((aces_film(0.18) - 0.2670).abs() < 1e-3);
    assert!((aces_film(1.0) - 0.8038).abs() < 1e-3);
}

/// Neutral settings: the chain is the identity (HDR out equals HDR in;
/// LDR equals the plain sRGB encoding); sRGB and BGRA outputs agree.
#[test]
fn neutral_chain_is_the_identity_and_formats_agree() {
    let Some(ctx) = ctx() else { return };
    let img = flat(40, 24, |x, y| {
        [
            x as f32 / 40.0,
            y as f32 / 24.0,
            0.25 + 0.5 * ((x + y) % 2) as f32,
        ]
    });
    let input = HdrInput::from_image(ctx, &img).unwrap();
    let cam = camera();
    let frame = PostFrame {
        camera: &cam,
        balls: &[],
    };
    let mut c = chain(ctx, 40, 24, PostConfig::neutral());
    let hdr = c.render_to_hdr(ctx, &input.view, &frame).unwrap();
    for (a, b) in hdr.pixels.iter().zip(&img.pixels) {
        for k in 0..3 {
            assert!((a[k] - b[k]).abs() <= 1e-3 * (1.0 + b[k]), "{a:?} vs {b:?}");
        }
    }
    let ldr = c.render_to_ldr(ctx, &input.view, &frame).unwrap();
    for (got, p) in ldr.pixels.iter().zip(&img.pixels) {
        for k in 0..3 {
            assert!(got[k].abs_diff(srgb8(p[k])) <= 1, "{got:?} vs {p:?}");
        }
    }
    for format in [
        wgpu::TextureFormat::Rgba8UnormSrgb,
        wgpu::TextureFormat::Bgra8Unorm,
        wgpu::TextureFormat::Bgra8UnormSrgb,
    ] {
        let mut other = PostChain::new(ctx, 40, 24, format).unwrap();
        other.set_config(PostConfig::neutral());
        assert_eq!(other.output_format(), format);
        let o = other.render_to_ldr(ctx, &input.view, &frame).unwrap();
        for (a, b) in o.pixels.iter().zip(&ldr.pixels) {
            for k in 0..4 {
                assert!(a[k].abs_diff(b[k]) <= 1, "{format:?}: {a:?} vs {b:?}");
            }
        }
    }
    assert!(PostChain::new(ctx, 40, 24, wgpu::TextureFormat::Rgba16Float).is_err());
    assert!(PostChain::new(ctx, 0, 24, wgpu::TextureFormat::Rgba8Unorm).is_err());
    assert!(c.resize(0, 3).is_err());
    c.resize(20, 10).unwrap();
    assert_eq!(c.size(), (20, 10));
    assert!(
        HdrInput::from_image(
            ctx,
            &HdrImage {
                width: 2,
                height: 2,
                pixels: vec![]
            }
        )
        .is_err()
    );
}

/// A 4×4 spot centred on the pixel corner (64, 64), which every bloom mip
/// level shares, so the halo is exactly symmetric.
fn spot_image(value: f32, background: f32) -> HdrImage {
    flat(N, N, |x, y| {
        if (62..=65).contains(&x) && (62..=65).contains(&y) {
            [value; 3]
        } else {
            [background; 3]
        }
    })
}

/// Verification 2 (analytic part): bloom moves only the above-threshold
/// light and conserves the total luminance; the halo is centred and smooth.
#[test]
fn bloom_conserves_energy_and_is_centred() {
    let Some(ctx) = ctx() else { return };
    let cam = camera();
    let frame = PostFrame {
        camera: &cam,
        balls: &[],
    };
    let img = spot_image(40.0, 0.1);
    let input = HdrInput::from_image(ctx, &img).unwrap();
    let mut cfg = PostConfig::neutral();
    cfg.bloom = BloomConfig::default();
    let mut c = chain(ctx, N, N, cfg);
    let out = c.render_to_hdr(ctx, &input.view, &frame).unwrap();
    assert_eq!(c.stats().bloom_levels, 6);
    let (before, after) = (total_lum(&img), total_lum(&out));
    let rel = (after - before) / before;
    println!("bloom energy: before {before:.3}, after {after:.3}, relative {rel:+.5}");
    assert!(rel.abs() < 0.01, "bloom changes the total by {rel:+.4}");
    // What moved: intensity × the above-threshold part of the spot.
    let moved = 16.0 * 0.15 * f64::from(luminance([40.0 - 1.0; 3]));
    let spot_after: f64 = (62..=65)
        .flat_map(|y| (62..=65).map(move |x| (x, y)))
        .map(|(x, y)| f64::from(luminance(rgb(out.at(x, y)))))
        .sum();
    let spot_before = 16.0 * f64::from(luminance([40.0; 3]));
    println!(
        "spot lost {:.3}, expected about {moved:.3}",
        spot_before - spot_after
    );
    assert!(spot_before - spot_after > 0.6 * moved && spot_before - spot_after < moved);
    // Far below the threshold and far from the spot: unchanged.
    for (x, y) in [(2, 2), (125, 3), (5, 120)] {
        let p = out.at(x, y);
        assert!((p[0] - 0.1).abs() < 2e-3, "background at {x},{y}: {p:?}");
    }
    // A visible halo, symmetric about the spot (four directions agree).
    for d in [4u32, 8, 16] {
        let s = [
            out.at(64 + d, 64)[0],
            out.at(63 - d, 64)[0],
            out.at(64, 64 + d)[0],
            out.at(64, 63 - d)[0],
        ];
        let mean = s.iter().sum::<f32>() / 4.0;
        println!("halo at {d}px: {s:?}");
        assert!(mean > 0.1 + 1e-3, "no halo at {d}px");
        for v in s {
            assert!(
                (v - mean).abs() < 0.1 * (mean - 0.1) + 2e-3,
                "asymmetric halo at {d}: {s:?}"
            );
        }
    }
    // Smooth: no blocky steps along a row (second difference small).
    let row: Vec<f32> = (40..90).map(|x| out.at(x, 70)[0]).collect();
    for w in row.windows(3) {
        let dd = (w[0] - 2.0 * w[1] + w[2]).abs();
        assert!(
            dd < 0.05 + 0.2 * w[1].abs().min(1.0),
            "step in halo row: {w:?}"
        );
    }

    // Whole-image bloom (threshold 0) conserves energy too.
    cfg.bloom.threshold = 0.0;
    cfg.bloom.knee = 0.0;
    cfg.bloom.firefly_clamp = 0.0;
    c.set_config(cfg);
    let out0 = c.render_to_hdr(ctx, &input.view, &frame).unwrap();
    let rel0 = (total_lum(&out0) - before) / before;
    println!("threshold 0: relative {rel0:+.5}");
    assert!(rel0.abs() < 0.01);
}

fn rgb(p: [f32; 4]) -> [f32; 3] {
    [p[0], p[1], p[2]]
}

/// The chrome-highlight case of 5.1 review N7: a single 3000-radiance pixel
/// feeds at most `firefly_clamp` into the bloom.
#[test]
fn firefly_clamp_bounds_the_halo() {
    let Some(ctx) = ctx() else { return };
    let cam = camera();
    let frame = PostFrame {
        camera: &cam,
        balls: &[],
    };
    let img = flat(N, N, |x, y| {
        if (x, y) == (64, 64) {
            [3000.0; 3]
        } else {
            [0.0; 3]
        }
    });
    let input = HdrInput::from_image(ctx, &img).unwrap();
    let mut cfg = PostConfig::neutral();
    cfg.bloom = BloomConfig::default();
    let mut c = chain(ctx, N, N, cfg);
    let out = c.render_to_hdr(ctx, &input.view, &frame).unwrap();
    let halo: f64 = total_lum(&out) - f64::from(luminance(rgb(out.at(64, 64))));
    let bound = f64::from(cfg.bloom.intensity * cfg.bloom.firefly_clamp);
    println!("firefly halo {halo:.3} (bound {bound:.3})");
    assert!(halo <= bound * 1.01 && halo > 0.5 * bound);
    assert!(out.pixels.iter().all(|p| p.iter().all(|v| v.is_finite())));
    // Tone-mapped: no pixel next to the firefly saturates.
    let ldr = c.render_to_ldr(ctx, &input.view, &frame).unwrap();
    assert!(ldr.at(66, 64)[0] < 255);
}

/// Verification 2 (golden): the bloomed spot through ACES matches the
/// stored image, and the image energies agree.
#[test]
fn bloom_matches_the_golden_image() {
    let Some(ctx) = ctx() else { return };
    let cam = camera();
    let frame = PostFrame {
        camera: &cam,
        balls: &[],
    };
    let img = flat(N, N, |x, y| {
        let d2 = (x as f32 - 50.0).powi(2) + (y as f32 - 70.0).powi(2);
        let e2 = (x as f32 - 90.0).powi(2) + (y as f32 - 40.0).powi(2);
        let mut c = [0.05, 0.06, 0.08];
        if d2 < 16.0 {
            c = [30.0, 20.0, 8.0];
        }
        if e2 < 4.0 {
            c = [4.0, 8.0, 40.0];
        }
        c
    });
    let input = HdrInput::from_image(ctx, &img).unwrap();
    let cfg = PostConfig {
        aa: AaMode::Off,
        motion_blur: MotionBlurConfig {
            enabled: false,
            ..Default::default()
        },
        ..PostConfig::default()
    };
    let mut c = chain(ctx, N, N, cfg);
    let hdr = c.render_to_hdr(ctx, &input.view, &frame).unwrap();
    let rel = (total_lum(&hdr) - total_lum(&img)) / total_lum(&img);
    assert!(rel.abs() < 0.01, "energy {rel:+.4}");
    let ldr = c.render_to_ldr(ctx, &input.view, &frame).unwrap();
    save_ldr("post_bloom_spots", &ldr);
    check_golden("bloom_spot", &ldr);
}

fn check_golden(name: &str, img: &LdrImage) {
    let got = img.to_rgba8();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/post")
        .join(format!("{name}.png"));
    if std::env::var("PIN_UPDATE_SNAPSHOTS").is_ok_and(|v| v.trim() == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::save_buffer(&path, &got, img.width, img.height, image::ColorType::Rgba8).unwrap();
        println!("[{name}] snapshot written to {}", path.display());
        return;
    }
    let want = image::open(&path)
        .unwrap_or_else(|e| panic!("[{name}] missing snapshot {} ({e})", path.display()))
        .to_rgba8()
        .into_raw();
    let (mut sum, mut far) = (0u64, 0usize);
    // Energy of the 8-bit images (linearised) must agree too.
    let lin = |v: u8| {
        let s = f64::from(v) / 255.0;
        if s <= 0.04045 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    let (mut eg, mut ew) = (0.0f64, 0.0f64);
    for (g, w) in got.chunks(4).zip(want.chunks(4)) {
        let d: Vec<u32> = (0..3).map(|k| u32::from(g[k].abs_diff(w[k]))).collect();
        sum += u64::from(d.iter().sum::<u32>());
        far += usize::from(d.iter().any(|&v| v > 16));
        for k in 0..3 {
            eg += lin(g[k]);
            ew += lin(w[k]);
        }
    }
    let px = (img.width * img.height) as usize;
    let mean = sum as f64 / (px * 3) as f64;
    println!(
        "[{name}] snapshot: mean |diff| {mean:.3}/255, {far} px off by > 16/255, energy {eg:.2} vs {ew:.2}"
    );
    assert!(
        mean <= 3.0 && far * 50 <= px,
        "[{name}] differs from the golden"
    );
    assert!(
        (eg - ew).abs() <= 0.02 * ew,
        "[{name}] energy {eg} vs golden {ew}"
    );
}

/// A grey background with a textured disc where `ball` projects.
fn ball_image(cam: &PbrCamera, ball: &MotionBall) -> HdrImage {
    let m = screen_motion(cam, ball, 0.0, N, N).unwrap();
    flat(N, N, |x, y| {
        let (dx, dy) = (
            x as f32 + 0.5 - m.center_px[0],
            y as f32 + 0.5 - m.center_px[1],
        );
        if dx * dx + dy * dy < m.radius_px * m.radius_px {
            [
                0.9 + 0.05 * (dx / m.radius_px),
                0.85,
                0.8 + 0.05 * (dy / m.radius_px),
            ]
        } else {
            [0.1, 0.12, 0.15]
        }
    })
}

/// Mass, centroid and covariance `(sxx, syy, sxy)` of the luminance above
/// the background (`bg`): the ball's footprint.
fn ball_moments(img: &HdrImage, bg: f32) -> (f64, [f64; 2], [f64; 3]) {
    let (mut w, mut mx, mut my) = (0.0f64, 0.0f64, 0.0f64);
    let mut pts = Vec::new();
    for y in 0..N {
        for x in 0..N {
            let d = f64::from(luminance(rgb(img.at(x, y))) - bg);
            if d.abs() > 1e-4 {
                let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
                pts.push((px, py, d));
                w += d;
                mx += d * px;
                my += d * py;
            }
        }
    }
    let (mx, my) = (mx / w, my / w);
    let (mut sxx, mut syy, mut sxy) = (0.0, 0.0, 0.0);
    for (x, y, d) in pts {
        sxx += d * (x - mx) * (x - mx);
        syy += d * (y - my) * (y - my);
        sxy += d * (x - mx) * (y - my);
    }
    (w, [mx, my], [sxx / w, syy / w, sxy / w])
}

/// Motion blur: none for a still ball (bit-identical), and a streak along
/// the screen velocity for a moving one.
#[test]
fn motion_blur_is_zero_when_still_and_follows_the_velocity() {
    let Some(ctx) = ctx() else { return };
    let cam = camera();
    let mut cfg = PostConfig::neutral();
    cfg.motion_blur = MotionBlurConfig {
        enabled: true,
        shutter_s: 1.0 / 60.0,
        samples: 32,
        max_length_px: 64.0,
    };
    let still = MotionBall {
        position: [0.0; 3],
        velocity: [0.0; 3],
        radius: 0.12,
    };
    let img = ball_image(&cam, &still);
    let input = HdrInput::from_image(ctx, &img).unwrap();
    let mut c = chain(ctx, N, N, cfg);
    let out = c
        .render_to_hdr(
            ctx,
            &input.view,
            &PostFrame {
                camera: &cam,
                balls: &[still],
            },
        )
        .unwrap();
    assert_eq!(c.stats().blurred_balls, 0);
    // Bit-identical to the neutral chain without balls.
    let mut plain = chain(ctx, N, N, PostConfig::neutral());
    let reference = plain
        .render_to_hdr(
            ctx,
            &input.view,
            &PostFrame {
                camera: &cam,
                balls: &[],
            },
        )
        .unwrap();
    assert_eq!(out.pixels, reference.pixels, "a still ball must not blur");

    // Directions (pixel frame, y down): +x (screen right), +z (screen up),
    // diagonal.
    for (v, want) in [
        ([6.0f32, 0.0, 0.0], 0.0f32),
        ([0.0, 0.0, 6.0], -core::f32::consts::FRAC_PI_2),
        ([4.0, 0.0, 4.0], -core::f32::consts::FRAC_PI_4),
    ] {
        let ball = MotionBall {
            velocity: v,
            ..still
        };
        let m = screen_motion(&cam, &ball, cfg.motion_blur.shutter_s, N, N).unwrap();
        let out = c
            .render_to_hdr(
                ctx,
                &input.view,
                &PostFrame {
                    camera: &cam,
                    balls: &[ball],
                },
            )
            .unwrap();
        assert_eq!(c.stats().blurred_balls, 1);
        // Analytic: a centred shutter smears the footprint by a box of
        // length s = |displacement| along the velocity, so the footprint
        // covariance grows by (s²/12)·d̂d̂ᵀ (exactly (1 − 1/n²)·s²/12 for n
        // samples) and the centroid and mass stay put.
        let bg = luminance([0.1, 0.12, 0.15]);
        let (m_ref, c_ref, v_ref) = ball_moments(&reference, bg);
        let (m_out, c_out, v_out) = ball_moments(&out, bg);
        let dv = [
            v_out[0] - v_ref[0],
            v_out[1] - v_ref[1],
            v_out[2] - v_ref[2],
        ];
        let angle = (0.5 * (2.0 * dv[2]).atan2(dv[0] - dv[1])) as f32;
        let s = f64::from(m.displacement_px[0].hypot(m.displacement_px[1]));
        let n = f64::from(cfg.motion_blur.samples);
        let want_growth = (1.0 - 1.0 / (n * n)) * s * s / 12.0;
        let (ca, sa) = (f64::from(angle).cos(), f64::from(angle).sin());
        let along = ca * ca * dv[0] + 2.0 * ca * sa * dv[2] + sa * sa * dv[1];
        let across = sa * sa * dv[0] - 2.0 * ca * sa * dv[2] + ca * ca * dv[1];
        let vel_angle = m.displacement_px[1].atan2(m.displacement_px[0]);
        // Axis angle modulo π.
        let mut diff = (angle - vel_angle).rem_euclid(core::f32::consts::PI);
        if diff > core::f32::consts::FRAC_PI_2 {
            diff = core::f32::consts::PI - diff;
        }
        let shift = (c_out[0] - c_ref[0]).hypot(c_out[1] - c_ref[1]);
        println!(
            "velocity {v:?}: streak {:?} px, axis {:.1}° (velocity {:.1}°, want {:.1}°), variance growth along {along:.2} (analytic {want_growth:.2}) across {across:.3} px², centroid shift {shift:.3} px, mass {:+.4}",
            m.displacement_px,
            angle.to_degrees(),
            vel_angle.to_degrees(),
            want.to_degrees(),
            (m_out - m_ref) / m_ref
        );
        // Screen y points down: +z (up) moves towards row 0.
        assert!(
            (vel_angle - want).abs() < 1e-3,
            "screen motion direction {vel_angle}"
        );
        assert!(
            diff < 5f32.to_radians(),
            "streak axis off by {}°",
            diff.to_degrees()
        );
        assert!(
            (along - want_growth).abs() < 0.1 * want_growth,
            "streak variance {along} vs analytic {want_growth}"
        );
        assert!(
            across.abs() < 0.1 * want_growth,
            "blur across the velocity: {across}"
        );
        assert!(shift < 0.25, "centroid moved by {shift} px");
        // Energy roughly conserved (the ball smears, it does not vanish).
        let rel = (total_lum(&out) - total_lum(&reference)) / total_lum(&reference);
        assert!(rel.abs() < 0.03, "blur changes energy by {rel:+.4}");
        if v[0] > 5.0 {
            let mut ldr_cfg = cfg;
            ldr_cfg.tone_map = ToneMap::AcesFilm;
            c.set_config(ldr_cfg);
            let ldr = c
                .render_to_ldr(
                    ctx,
                    &input.view,
                    &PostFrame {
                        camera: &cam,
                        balls: &[ball],
                    },
                )
                .unwrap();
            save_ldr("post_motion_blur_streak", &ldr);
            c.set_config(cfg);
        }
    }
    // Length clamp.
    let fast = MotionBall {
        velocity: [600.0, 0.0, 0.0],
        ..still
    };
    let _ = c
        .render_to_hdr(
            ctx,
            &input.view,
            &PostFrame {
                camera: &cam,
                balls: &[fast],
            },
        )
        .unwrap();
    assert!((c.stats().max_blur_px - 64.0).abs() < 1e-3);
}

fn lum4(p: [f32; 4]) -> f32 {
    luminance(rgb(p))
}

/// Anti-aliasing measurably reduces edge aliasing: against a 4×4
/// supersampled reference, the silhouette error drops by more than half.
#[test]
fn msaa_reduces_edge_aliasing() {
    let Some(ctx) = ctx() else { return };
    let cam = camera();
    let mut white = MaterialParams::lambert([0.0; 3]);
    white.emissive = [0.8, 0.8, 0.8];
    let scene = |pbr: &mut PbrRenderer| {
        let sphere = pbr
            .upload_mesh(&MeshGeometry::uv_sphere(0.5, 96, 64))
            .unwrap();
        vec![DrawItem {
            mesh: sphere,
            model: IDENTITY,
            material: white,
            transparent: false,
            emission: 0.0,
        }]
    };
    // Reference: 4x resolution, box-filtered.
    let mut big = PbrRenderer::new(ctx, N * 4, N * 4).unwrap();
    let draws = scene(&mut big);
    let mut frame = FrameDesc::new(&cam, &[], &draws);
    frame.environment = Environment::BLACK;
    let hi = big.render_to_image(ctx, &frame).unwrap();
    let reference: Vec<f32> = (0..N * N)
        .map(|i| {
            let (x, y) = (i % N, i / N);
            let mut s = 0.0;
            for dy in 0..4 {
                for dx in 0..4 {
                    s += lum4(hi.at(x * 4 + dx, y * 4 + dy));
                }
            }
            s / 16.0
        })
        .collect();
    let mut errors = Vec::new();
    for aa in [AaMode::Off, AaMode::Msaa4] {
        let cfg = PostConfig {
            aa,
            ..PostConfig::neutral()
        };
        let mut fr = FrameRenderer::new(ctx, N, N, cfg).unwrap();
        assert_eq!(fr.pbr().msaa_samples(), aa.samples());
        let draws = scene(fr.pbr_mut());
        let mut frame = FrameDesc::new(&cam, &[], &draws);
        frame.environment = Environment::BLACK;
        let out = fr.render(ctx, &frame, &[]).unwrap();
        let (mut err, mut n) = (0.0f64, 0);
        for i in 0..(N * N) as usize {
            let r = reference[i];
            if r > 0.01 && r < 0.79 {
                err += f64::from((lum4(out.hdr.pixels[i]) - r).abs());
                n += 1;
            }
        }
        let e = err / f64::from(n.max(1));
        println!("{aa:?}: mean edge error {e:.4} over {n} edge px");
        errors.push(e);
        if aa == AaMode::Msaa4 {
            save_ldr("post_msaa_sphere", &out.ldr);
        }
    }
    assert!(
        errors[1] < 0.5 * errors[0],
        "MSAA error {} vs none {}",
        errors[1],
        errors[0]
    );
}

/// 5.1 review N4: with exposure and ACES before 8-bit encoding the lit
/// insert and the pearl sphere have no clipped flat regions.
#[test]
fn tone_mapping_leaves_no_clipped_regions_on_the_51_scenes() {
    let Some(ctx) = ctx() else { return };
    let cam = camera();
    let lights = [PointLight::new([-1.5, -2.5, 2.0], [1.0; 3], 12.0, 50.0)];
    let mut fr = FrameRenderer::new(ctx, N, N, PostConfig::default()).unwrap();
    let sphere = fr
        .pbr_mut()
        .upload_mesh(&MeshGeometry::uv_sphere(0.5, 96, 64))
        .unwrap();
    for (name, kind, emission) in [
        ("light_insert_lit", MaterialKind::LightInsert, 1.0f32),
        ("ceramic_pearl", MaterialKind::CeramicPearl, 0.0),
        ("chrome_steel", MaterialKind::ChromeSteel, 0.0),
    ] {
        let mut d = DrawItem::new(sphere, IDENTITY, kind);
        d.emission = emission;
        let draws = [d];
        let mut frame = FrameDesc::new(&cam, &lights, &draws);
        frame.clear = [0.18, 0.18, 0.18, 1.0];
        let balls = [SceneBall {
            draw: 0,
            position: [0.0; 3],
            velocity: [0.0; 3],
            radius: 0.5,
        }];
        let out = fr.render(ctx, &frame, &balls).unwrap();
        let plain_clipped = out
            .hdr
            .pixels
            .iter()
            .filter(|p| p[0] >= 1.0 || p[1] >= 1.0 || p[2] >= 1.0)
            .count();
        let clipped = out
            .ldr
            .pixels
            .iter()
            .filter(|p| p[0] == 255 || p[1] == 255 || p[2] == 255)
            .count();
        let px = (N * N) as usize;
        println!(
            "{name}: {clipped} px saturated after ACES ({:.2} %), {plain_clipped} px ≥ 1 before",
            100.0 * clipped as f64 / px as f64
        );
        assert!(clipped * 200 <= px, "{name}: {clipped} saturated px");
        save_ldr(&format!("post_tonemapped_{name}"), &out.ldr);
    }
}
