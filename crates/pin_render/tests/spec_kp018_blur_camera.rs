//! Independent spec tests for KP-018 (issue #98): motion blur follows screen-space motion
//! between the previous and the current camera (REQ-CAM-005, SPEC §7.2; post chain SPEC §5.2).
//!
//! Black-box tests written from the brief and the public API docs only.
//!
//! Scene convention (as in `spec_5_2_cpu.rs`): the camera sits on the +X axis looking at the
//! origin, +Z up, so image right = world +Y and image up = world +Z (pixel y points down).

use pin_render::pbr::PbrCamera;
use pin_render::post::{MotionBall, ScreenMotion, screen_motion, screen_motion_between};
use proptest::prelude::*;

const SIZE: u32 = 128;
const CAM_X: f32 = 0.6;
const FOV_Y: f32 = 0.25;
const NEAR: f32 = 0.1;
const BALL_R: f32 = 0.015;

/// Camera looking along -X from `eye`, orientation independent of `eye` (a pure translation
/// of the reference camera at `(CAM_X, 0, 0)`).
fn cam_at(eye: [f32; 3]) -> PbrCamera {
    PbrCamera::look_at(
        eye,
        [eye[0] - CAM_X, eye[1], eye[2]],
        [0.0, 0.0, 1.0],
        FOV_Y,
        1.0,
        NEAR,
        5.0,
    )
}

fn cam_a() -> PbrCamera {
    cam_at([CAM_X, 0.0, 0.0])
}

fn ball(p: [f32; 3], v: [f32; 3]) -> MotionBall {
    MotionBall {
        position: p,
        velocity: v,
        radius: BALL_R,
    }
}

/// Camera A translated by `v * shutter`.
fn following(v: [f32; 3], shutter: f32) -> PbrCamera {
    cam_at([CAM_X + v[0] * shutter, v[1] * shutter, v[2] * shutter])
}

fn same_bits(a: &ScreenMotion, b: &ScreenMotion) -> bool {
    let eq = |x: f32, y: f32| x.to_bits() == y.to_bits();
    eq(a.center_px[0], b.center_px[0])
        && eq(a.center_px[1], b.center_px[1])
        && eq(a.radius_px, b.radius_px)
        && eq(a.displacement_px[0], b.displacement_px[0])
        && eq(a.displacement_px[1], b.displacement_px[1])
}

// ---------------------------------------------------------------------------------------
// CPU tests
// ---------------------------------------------------------------------------------------

/// REQ-CAM-005: a ball moving with a following camera has zero screen displacement.
#[test]
fn following_camera_gives_zero_displacement() {
    let a = cam_a();
    for v in [
        [0.0, 1.0, 0.0],
        [0.0, 0.0, -2.0],
        [0.0, 3.0, 1.5],
        [0.4, -2.5, 0.5],
        [-0.3, 0.2, 2.9],
    ] {
        for shutter in [1.0 / 240.0, 1.0 / 120.0, 1.0 / 60.0] {
            let b = following(v, shutter);
            let ball = ball([0.002, 0.004, -0.003], v);
            let m = screen_motion_between(&a, &b, &ball, shutter, SIZE, SIZE)
                .expect("ball in front of both cameras");
            assert!(
                m.displacement_px[0].abs() < 1e-3 && m.displacement_px[1].abs() < 1e-3,
                "v {v:?} shutter {shutter}: {:?}",
                m.displacement_px
            );
        }
    }
}

proptest! {
    /// REQ-CAM-005: the following-camera property over a sweep.
    #[test]
    fn following_camera_zero_displacement_sweep(
        p in prop::array::uniform3(-0.02f32..0.02),
        v in prop::array::uniform3(-3.0f32..3.0),
        shutter in (1.0f32 / 240.0)..(1.0f32 / 60.0),
    ) {
        let a = cam_a();
        let b = following(v, shutter);
        let m = screen_motion_between(&a, &b, &ball(p, v), shutter, SIZE, SIZE)
            .expect("in front of both cameras");
        prop_assert!(m.displacement_px[0].abs() < 1e-3, "{:?}", m.displacement_px);
        prop_assert!(m.displacement_px[1].abs() < 1e-3, "{:?}", m.displacement_px);
    }

    /// SPEC §5.2: with a still camera (prev == current) the result equals `screen_motion`
    /// bit for bit.
    #[test]
    fn still_camera_equals_screen_motion_bitwise(
        p in prop::array::uniform3(-0.05f32..0.05),
        v in prop::array::uniform3(-5.0f32..5.0),
        shutter in (1.0f32 / 480.0)..(1.0f32 / 30.0),
    ) {
        let a = cam_a();
        let ball = ball(p, v);
        let old = screen_motion(&a, &ball, shutter, SIZE, SIZE).expect("in view");
        let new = screen_motion_between(&a, &a, &ball, shutter, SIZE, SIZE).expect("in view");
        prop_assert!(same_bits(&old, &new), "{old:?} vs {new:?}");
    }
}

/// REQ-CAM-005: a ball at rest in front of a moving camera moves on screen by exactly the
/// difference of its two projections.
#[test]
fn resting_ball_displacement_is_difference_of_projections() {
    let a = cam_a();
    for (p, d) in [
        ([0.0, 0.0, 0.0], [0.0, 0.01, 0.0]),
        ([0.005, -0.01, 0.01], [0.0, -0.02, 0.01]),
        ([0.0, 0.0, 0.0], [0.05, 0.0, 0.0]),
        ([-0.02, 0.01, 0.0], [-0.1, 0.005, -0.005]),
    ] {
        let b = cam_at([CAM_X + d[0], d[1], d[2]]);
        let still = ball(p, [0.0; 3]);
        let ca = screen_motion(&a, &still, 1.0 / 120.0, SIZE, SIZE)
            .unwrap()
            .center_px;
        let cb = screen_motion(&b, &still, 1.0 / 120.0, SIZE, SIZE)
            .unwrap()
            .center_px;
        let m = screen_motion_between(&a, &b, &still, 1.0 / 120.0, SIZE, SIZE).unwrap();
        let want = [cb[0] - ca[0], cb[1] - ca[1]];
        assert!(
            (m.displacement_px[0] - want[0]).abs() < 1e-3
                && (m.displacement_px[1] - want[1]).abs() < 1e-3,
            "{:?} vs {want:?}",
            m.displacement_px
        );
        if d[1] != 0.0 || d[2] != 0.0 {
            let l = want[0].hypot(want[1]);
            assert!(l > 0.5, "camera move must show on screen ({l} px)");
        }
        // Centre and radius come from the current camera.
        let cur = screen_motion(&b, &still, 1.0 / 120.0, SIZE, SIZE).unwrap();
        assert_eq!(m.center_px, cur.center_px);
        assert_eq!(m.radius_px.to_bits(), cur.radius_px.to_bits());
    }
}

/// SPEC §5.2 / REQ-CAM-005: with different cameras the displacement is not the one-camera
/// displacement; a following-camera ball is not blurred even though `screen_motion` of the
/// current camera alone would blur it.
#[test]
fn different_cameras_are_not_the_one_camera_displacement() {
    let a = cam_a();
    for v in [[0.0, 3.0, 0.0], [0.0, -2.0, 2.0], [0.2, 1.0, -3.0]] {
        let shutter = 1.0 / 120.0;
        let b = following(v, shutter);
        let ball = ball([0.0; 3], v);
        let one = screen_motion(&b, &ball, shutter, SIZE, SIZE)
            .unwrap()
            .displacement_px;
        let two = screen_motion_between(&a, &b, &ball, shutter, SIZE, SIZE)
            .unwrap()
            .displacement_px;
        assert!(
            one[0].hypot(one[1]) > 1.0,
            "one-camera blur must be visible: {one:?}"
        );
        assert!(
            two[0].hypot(two[1]) < 1e-3,
            "two-camera blur must vanish: {two:?}"
        );
    }
}

/// REQ-CAM-005 / SPEC §5.2: `None` when the start is behind the previous camera's near
/// plane, when the end is behind the current camera's, and for non-finite input.
#[test]
fn none_cases() {
    let a = cam_a(); // eye x = 0.6, near 0.1
    let far = cam_at([1.5, 0.0, 0.0]);
    let shutter = 0.01;
    // x = 0.7 is 0.1 m behind `a`'s eye (and so behind its near plane), 0.8 m from `far`.
    let near_a = ball([0.7, 0.0, 0.0], [0.0; 3]);
    assert!(screen_motion_between(&a, &far, &near_a, shutter, SIZE, SIZE).is_none());
    assert!(screen_motion_between(&far, &a, &near_a, shutter, SIZE, SIZE).is_none());
    let ok = ball([0.0; 3], [0.0; 3]);
    assert!(screen_motion_between(&a, &far, &ok, shutter, SIZE, SIZE).is_some());
    // Start (p - v/2 s) behind the previous camera: start x = 0.7, 0.1 m behind it.
    let fast = ball([0.0, 0.0, 0.0], [-140.0, 0.0, 0.0]); // start x = 0.7 (behind a)
    assert!(screen_motion_between(&a, &a, &fast, shutter, SIZE, SIZE).is_none());
    // Only the end point behind the current camera: ball runs towards it.
    let fast_in = ball([0.0, 0.0, 0.0], [140.0, 0.0, 0.0]); // end x = 0.7 (behind a)
    assert!(screen_motion_between(&far, &a, &fast_in, shutter, SIZE, SIZE).is_none());
    // The same ball is fine when the current camera is the far one (end 0.8 m away).
    assert!(screen_motion_between(&a, &far, &fast_in, shutter, SIZE, SIZE).is_some());

    let nan = f32::NAN;
    let inf = f32::INFINITY;
    for bad in [
        ball([nan, 0.0, 0.0], [0.0; 3]),
        ball([0.0, inf, 0.0], [0.0; 3]),
        ball([0.0; 3], [0.0, nan, 0.0]),
        ball([0.0; 3], [inf, 0.0, 0.0]),
    ] {
        assert!(
            screen_motion_between(&a, &a, &bad, shutter, SIZE, SIZE).is_none(),
            "{bad:?}"
        );
    }
}

/// REQ-CAM-005: a non-finite radius is a non-finite input, so the result is `None` (brief:
/// "None if ... any input is non-finite").
#[test]
fn none_for_non_finite_radius() {
    let a = cam_a();
    for r in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let b = MotionBall {
            radius: r,
            ..ball([0.0; 3], [0.0, 1.0, 0.0])
        };
        assert!(
            screen_motion_between(&a, &a, &b, 0.01, SIZE, SIZE).is_none(),
            "radius {r}"
        );
    }
}

/// REQ-CAM-005: a non-finite shutter is a non-finite input, so the result is `None`.
#[test]
fn none_for_non_finite_shutter() {
    let a = cam_a();
    let ok = ball([0.0; 3], [0.0, 1.0, 0.0]);
    for s in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert!(
            screen_motion_between(&a, &a, &ok, s, SIZE, SIZE).is_none(),
            "shutter {s}"
        );
    }
}

// ---------------------------------------------------------------------------------------
// GPU tests
// ---------------------------------------------------------------------------------------

#[cfg(feature = "gpu-tests")]
mod gpu {
    use super::*;
    use pin_render::gpu::{GpuContext, context_for_tests, wait_for_device};
    use pin_render::pbr::HdrImage;
    use pin_render::post::{
        AaMode, BloomConfig, HdrInput, MotionBlurConfig, PostChain, PostConfig, PostFrame, ToneMap,
    };
    use std::sync::OnceLock;

    const BG: f32 = 0.125;
    const BODY: f32 = 0.75;

    fn ctx() -> Option<&'static GpuContext> {
        static CTX: OnceLock<Option<GpuContext>> = OnceLock::new();
        CTX.get_or_init(|| context_for_tests("spec_kp018_blur_camera"))
            .as_ref()
    }

    fn config(blur: bool) -> PostConfig {
        PostConfig {
            exposure: 1.0,
            bloom: BloomConfig {
                enabled: false,
                ..BloomConfig::default()
            },
            tone_map: ToneMap::Linear,
            motion_blur: MotionBlurConfig {
                enabled: blur,
                ..MotionBlurConfig::default()
            },
            aa: AaMode::Off,
        }
    }

    /// A bright disc of `radius_px` on a flat background, 4x4 supersampled edge.
    fn disc_image(center: [f32; 2], radius_px: f32) -> HdrImage {
        let mut pixels = Vec::with_capacity((SIZE * SIZE) as usize);
        for y in 0..SIZE {
            for x in 0..SIZE {
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
                let v = BG + (BODY - BG) * cov;
                pixels.push([v, v, v, 1.0]);
            }
        }
        HdrImage {
            width: SIZE,
            height: SIZE,
            pixels,
        }
    }

    struct Rig {
        ctx: &'static GpuContext,
        chain: PostChain,
        out: wgpu::Texture,
        out_view: wgpu::TextureView,
    }

    impl Rig {
        fn new() -> Option<Rig> {
            let ctx = ctx()?;
            let chain = PostChain::new(ctx, SIZE, SIZE, wgpu::TextureFormat::Rgba8Unorm).unwrap();
            let out = ctx.device().create_texture(&wgpu::TextureDescriptor {
                label: Some("kp018 out"),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let out_view = out.create_view(&Default::default());
            Some(Rig {
                ctx,
                chain,
                out,
                out_view,
            })
        }

        /// Encodes one presented frame (advancing the camera history) and returns the
        /// luminance (red channel, 0..=255) image.
        fn frame(
            &mut self,
            blur: bool,
            cam: &PbrCamera,
            ball: &MotionBall,
            shutter: f32,
        ) -> Vec<u8> {
            self.chain.set_config(PostConfig {
                motion_blur: MotionBlurConfig {
                    shutter_s: shutter,
                    ..config(blur).motion_blur
                },
                ..config(blur)
            });
            let m = screen_motion(cam, ball, shutter, SIZE, SIZE).expect("ball in view");
            let img = disc_image(m.center_px, m.radius_px);
            let input = HdrInput::from_image(self.ctx, &img).unwrap();
            let mut enc = self
                .ctx
                .device()
                .create_command_encoder(&Default::default());
            let balls = [*ball];
            let frame = PostFrame {
                camera: cam,
                balls: &balls,
            };
            self.chain
                .encode(&mut enc, &input.view, &frame, &self.out_view);
            let padded = (SIZE * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
            let buf = self.ctx.device().create_buffer(&wgpu::BufferDescriptor {
                label: Some("kp018 readback"),
                size: u64::from(padded * SIZE),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.out,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(SIZE),
                    },
                },
                wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
            );
            self.ctx.queue().submit([enc.finish()]);
            buf.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
            wait_for_device(self.ctx.device(), std::time::Duration::from_secs(60)).unwrap();
            let mapped = buf.get_mapped_range(..).unwrap();
            let mut lum = Vec::with_capacity((SIZE * SIZE) as usize);
            for y in 0..SIZE as usize {
                for x in 0..SIZE as usize {
                    lum.push(mapped[y * padded as usize + x * 4]);
                }
            }
            lum
        }
    }

    /// Extent in pixels of the samples brighter than `level`, projected on `axis`.
    fn extent(img: &[u8], level: u8, axis: [f64; 2]) -> f64 {
        let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
        for y in 0..SIZE {
            for x in 0..SIZE {
                if img[(y * SIZE + x) as usize] > level {
                    let t = (f64::from(x) + 0.5) * axis[0] + (f64::from(y) + 0.5) * axis[1];
                    lo = lo.min(t);
                    hi = hi.max(t);
                }
            }
        }
        hi - lo + 1.0
    }

    fn bg_level(img: &[u8]) -> u8 {
        img[0]
    }

    const SHUTTER: f32 = 1.0 / 120.0;

    /// REQ-CAM-005, SPEC §5.2, R12a: with a following camera the second frame has no blurred
    /// balls and its pixels equal the blur-disabled render.
    #[test]
    fn following_camera_frame_is_not_blurred() {
        let Some(mut rig) = Rig::new() else { return };
        for v in [[0.0, 3.0, 0.0], [0.0, -2.0, 2.0]] {
            let a = cam_a();
            let b = following(v, SHUTTER);
            let ball = ball([0.0; 3], v);
            // Frame 1: camera A (history: still camera, first frame).
            rig.frame(true, &a, &ball, SHUTTER);
            // Frame 2: camera B, the ball moves with it.
            let blurred = rig.frame(true, &b, &ball, SHUTTER);
            let stats = rig.chain.stats();
            assert_eq!(stats.blurred_balls, 0, "v {v:?}: {stats:?}");
            // The same frame with blur disabled.
            rig.frame(false, &a, &ball, SHUTTER);
            let sharp = rig.frame(false, &b, &ball, SHUTTER);
            for (i, (p, q)) in blurred.iter().zip(&sharp).enumerate() {
                assert!(
                    p.abs_diff(*q) <= 1,
                    "v {v:?}: pixel {i} differs: {p} vs {q}"
                );
            }
        }
    }

    /// REQ-CAM-005 / SPEC §5.2, R12a: a fast ball over a still camera blurs along its
    /// projected velocity and not across it.
    #[test]
    fn still_camera_streak_runs_along_the_velocity() {
        let Some(mut rig) = Rig::new() else { return };
        let a = cam_a();
        for deg in [0.0f32, 45.0, 90.0] {
            let ang = deg.to_radians();
            let v = [0.0, 3.0 * ang.cos(), 3.0 * ang.sin()];
            let ball = ball([0.0; 3], v);
            let m = screen_motion(&a, &ball, SHUTTER, SIZE, SIZE).unwrap();
            let want_len = m.displacement_px[0].hypot(m.displacement_px[1]);
            assert!(want_len > 3.0, "scene must blur visibly: {want_len}");
            rig.frame(true, &a, &ball, SHUTTER);
            let blurred = rig.frame(true, &a, &ball, SHUTTER);
            let stats = rig.chain.stats();
            assert_eq!(stats.blurred_balls, 1, "{deg} deg: {stats:?}");
            assert!(stats.max_blur_px > 1.0, "{deg} deg: {stats:?}");
            let sharp = rig.frame(false, &a, &ball, SHUTTER);
            // Image y points down: world +Z is screen -y.
            let along = [f64::from(ang.cos()), -f64::from(ang.sin())];
            let across = [-along[1], along[0]];
            let lvl = bg_level(&sharp).saturating_add(4);
            let (s_al, s_ac) = (extent(&sharp, lvl, along), extent(&sharp, lvl, across));
            let (b_al, b_ac) = (extent(&blurred, lvl, along), extent(&blurred, lvl, across));
            assert!(
                b_al > s_al + 0.5 * f64::from(want_len),
                "{deg} deg: along {b_al} vs sharp {s_al} (streak {want_len})"
            );
            assert!(
                b_ac < s_ac + 2.0,
                "{deg} deg: across {b_ac} vs sharp {s_ac}"
            );
        }
    }

    /// REQ-CAM-005: after `reset_camera_history` the next frame behaves as a first frame:
    /// the blur equals the one-camera `screen_motion` displacement of the current camera.
    #[test]
    fn reset_camera_history_uses_the_current_camera_for_both_ends() {
        let Some(mut rig) = Rig::new() else { return };
        let a = cam_a();
        let b = cam_at([CAM_X, 0.02, 0.0]); // a 0.02 m camera cut sideways
        let v = [0.0, 1.5, 0.0];
        let ball = ball([0.0; 3], v);
        let want = screen_motion(&b, &ball, SHUTTER, SIZE, SIZE)
            .unwrap()
            .displacement_px;
        let want_len = want[0].hypot(want[1]);
        let max = MotionBlurConfig::default().max_length_px;
        assert!(
            want_len > 3.0 && want_len < max,
            "scene precondition: {want_len}"
        );
        let between = screen_motion_between(&a, &b, &ball, SHUTTER, SIZE, SIZE)
            .unwrap()
            .displacement_px;
        assert!(
            (between[0].hypot(between[1]) - want_len).abs() > 1.0,
            "the cut must matter without a reset"
        );

        rig.frame(true, &a, &ball, SHUTTER);
        rig.chain.reset_camera_history();
        rig.frame(true, &b, &ball, SHUTTER);
        let stats = rig.chain.stats();
        assert_eq!(stats.blurred_balls, 1, "{stats:?}");
        assert!(
            (stats.max_blur_px - want_len.min(max)).abs() < 0.05 + 0.01 * want_len,
            "blur {} vs {}",
            stats.max_blur_px,
            want_len.min(max)
        );
    }
}
