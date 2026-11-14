use super::*;
use proptest::prelude::*;

#[test]
fn aces_film_reference_points() {
    // Narkowicz 2015, evaluated in f64.
    let reference =
        |x: f64| ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0);
    for x in [0.0f32, 0.01, 0.1, 0.18, 0.5, 1.0, 2.0, 4.0, 10.0, 100.0] {
        let got = aces_film(x);
        let want = reference(f64::from(x)) as f32;
        assert!((got - want).abs() < 1e-6, "aces({x}) = {got}, want {want}");
    }
    assert!((aces_film(1.0) - 0.803_797).abs() < 1e-5);
    assert_eq!(aces_film(0.0), 0.0);
    assert_eq!(aces_film(-1.0), 0.0);
    assert_eq!(aces_film(f32::NAN), 0.0);
    assert_eq!(aces_film(f32::INFINITY), 1.0);
    assert_eq!(aces_film(1e6), 1.0);
}

#[test]
fn bloom_prefilter_threshold_knee_and_clamp() {
    let cfg = BloomConfig::default();
    // Below the knee: nothing.
    assert_eq!(bloom_prefilter([0.4, 0.4, 0.4], &cfg), [0.0; 3]);
    // Far above: L − t of the luminance, colour kept.
    let b = bloom_prefilter([3.0, 3.0, 3.0], &cfg);
    assert!((luminance(b) - 2.0).abs() < 1e-4, "{b:?}");
    // Firefly clamp.
    let f = bloom_prefilter([3000.0, 3000.0, 3000.0], &cfg);
    assert!((luminance(f) - cfg.firefly_clamp).abs() < 1e-2, "{f:?}");
    // Threshold 0, knee 0: everything.
    let all = BloomConfig {
        threshold: 0.0,
        knee: 0.0,
        firefly_clamp: 0.0,
        ..cfg
    };
    assert_eq!(bloom_prefilter([0.2, 0.5, 7.0], &all), [0.2, 0.5, 7.0]);
    // Garbage in, zeros out.
    assert_eq!(bloom_prefilter([f32::NAN, -1.0, 0.0], &cfg), [0.0; 3]);
    assert_eq!(
        bloom_prefilter([f32::INFINITY, 0.0, 0.0], &cfg),
        [0.0; 3],
        "inf is treated as 0"
    );
}

proptest! {
    #[test]
    fn bloom_prefilter_never_exceeds_its_input(
        r in 0.0f32..1e4, g in 0.0f32..1e4, b in 0.0f32..1e4,
        t in 0.0f32..10.0, k in 0.0f32..10.0, fc in -1.0f32..200.0,
    ) {
        let cfg = BloomConfig { threshold: t, knee: k, firefly_clamp: fc, ..BloomConfig::default() };
        let c = [r, g, b];
        let o = bloom_prefilter(c, &cfg);
        for i in 0..3 {
            prop_assert!(o[i] >= 0.0 && o[i] <= c[i] * (1.0 + 1e-6), "{c:?} -> {o:?}");
        }
        if fc > 0.0 {
            prop_assert!(luminance(o) <= fc * (1.0 + 1e-4));
        }
        // Monotone in the input scale.
        let o2 = bloom_prefilter([r * 2.0, g * 2.0, b * 2.0], &cfg);
        prop_assert!(luminance(o2) + 1e-4 >= luminance(o));
    }

    #[test]
    fn aces_is_monotone_and_bounded(a in 0.0f32..1e3, d in 0.0f32..10.0) {
        let (x, y) = (aces_film(a), aces_film(a + d));
        prop_assert!((0.0..=1.0).contains(&x));
        prop_assert!(y + 1e-6 >= x);
    }

    #[test]
    fn screen_motion_is_finite_or_none(
        x in -2.0f32..2.0, y in -2.0f32..2.0, z in -2.0f32..2.0,
        vx in -50.0f32..50.0, vy in -50.0f32..50.0, vz in -50.0f32..50.0,
        r in 0.0f32..0.1, shutter in -1.0f32..1.0,
    ) {
        let cam = PbrCamera::look_at([0.0, -3.0, 0.0], [0.0; 3], [0.0, 0.0, 1.0], 0.6, 1.0, 0.1, 20.0);
        let ball = MotionBall { position: [x, y, z], velocity: [vx, vy, vz], radius: r };
        if let Some(m) = screen_motion(&cam, &ball, shutter, 64, 64) {
            prop_assert!(m.radius_px >= 0.0);
            prop_assert!(m.center_px.iter().chain(&m.displacement_px).all(|v| v.is_finite()));
        }
    }
}

fn cam() -> PbrCamera {
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

#[test]
fn screen_motion_geometry() {
    let c = cam();
    let still = MotionBall {
        position: [0.0; 3],
        velocity: [0.0; 3],
        radius: 0.25,
    };
    let m = screen_motion(&c, &still, 1.0 / 60.0, 128, 128).unwrap();
    assert!((m.center_px[0] - 64.0).abs() < 1e-3 && (m.center_px[1] - 64.0).abs() < 1e-3);
    assert_eq!(m.displacement_px, [0.0, 0.0]);
    // Radius: 0.25 m at 3 m with a 30° fov over 128 px.
    let want = 0.25 / 3.0 / (15f32.to_radians().tan()) * 64.0;
    assert!(
        (m.radius_px - want).abs() < 0.05,
        "{} vs {want}",
        m.radius_px
    );
    // Moving +x (screen right) at 6 m/s for 1/60 s: 0.1 m.
    let fast = MotionBall {
        velocity: [6.0, 0.0, 0.0],
        ..still
    };
    let m = screen_motion(&c, &fast, 1.0 / 60.0, 128, 128).unwrap();
    let want_dx = 0.1 / 3.0 / (15f32.to_radians().tan()) * 64.0;
    assert!((m.displacement_px[0] - want_dx).abs() < 0.05, "{m:?}");
    assert!(m.displacement_px[1].abs() < 1e-3);
    // Moving +z (screen up = −y in pixels).
    let up = MotionBall {
        velocity: [0.0, 0.0, 6.0],
        ..still
    };
    let m = screen_motion(&c, &up, 1.0 / 60.0, 128, 128).unwrap();
    assert!(m.displacement_px[1] < -1.0 && m.displacement_px[0].abs() < 1e-3);
    // Behind the camera.
    let behind = MotionBall {
        position: [0.0, -5.0, 0.0],
        ..still
    };
    assert!(screen_motion(&c, &behind, 0.01, 128, 128).is_none());
    // From a snapshot state.
    let s = pin_core::snapshot::BallState {
        position_m: [1.0, 2.0, 3.0],
        velocity_m_s: [4.0, 5.0, 6.0],
        ..Default::default()
    };
    let b = MotionBall::from_state(&s, 0.0135);
    assert_eq!(b.position, [1.0, 2.0, 3.0]);
    assert_eq!(b.velocity, [4.0, 5.0, 6.0]);
    assert_eq!(b.radius, 0.0135);
}

#[test]
fn configs_and_ldr_helpers() {
    let d = PostConfig::default();
    assert_eq!(d.aa, AaMode::Msaa4);
    assert_eq!(d.tone_map, ToneMap::AcesFilm);
    assert!(d.bloom.enabled && d.motion_blur.enabled);
    let n = PostConfig::neutral();
    assert!(!n.bloom.enabled && !n.motion_blur.enabled);
    assert_eq!(n.aa.samples(), 1);
    assert_eq!(AaMode::Msaa4.samples(), 4);
    assert_eq!(AaMode::default(), AaMode::Msaa4);
    assert_eq!(ToneMap::default(), ToneMap::AcesFilm);
    let img = LdrImage {
        width: 2,
        height: 1,
        pixels: vec![[0, 10, 20, 255], [100, 110, 120, 255]],
    };
    assert_eq!(img.at(1, 0), [100, 110, 120, 255]);
    assert_eq!(img.region_mean(0, 0, 2, 1), [50.0, 60.0, 70.0, 255.0]);
    assert_eq!(img.region_mean(3, 3, 4, 4), [0.0; 4]);
    assert_eq!(img.to_rgba8(), vec![0, 10, 20, 255, 100, 110, 120, 255]);
    assert!((luminance([1.0, 1.0, 1.0]) - 1.0).abs() < 1e-6);
}
