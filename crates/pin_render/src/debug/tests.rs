use super::*;
use pin_core::snapshot::FlipperState;
use proptest::prelude::*;

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];

#[test]
fn lines_capacity_drop_and_clear() {
    let mut l = DebugLines::new(2);
    assert!(l.is_empty());
    assert_eq!(l.capacity(), 2);
    let line = DebugLine::new([0.0; 3], [1.0; 3], RED);
    assert!(l.push(line));
    assert!(l.push(line));
    assert!(!l.push(line));
    assert!(!l.push(line));
    assert_eq!((l.len(), l.dropped()), (2, 2));
    assert_eq!(l.as_slice(), &[line, line]);
    l.clear();
    assert_eq!((l.len(), l.dropped()), (0, 0));
    assert!(l.push(line));
    let mut z = DebugLines::new(0);
    assert!(!z.push(line));
    assert_eq!(z.dropped(), 1);
}

#[test]
fn line_is_pod_40_bytes() {
    assert_eq!(core::mem::size_of::<DebugLine>(), 40);
    let l = DebugLine::new([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], [7.0, 8.0, 9.0, 10.0]);
    let f: &[f32] = bytemuck::cast_slice(core::slice::from_ref(&l));
    assert_eq!(f, &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0]);
}

#[test]
fn box_has_twelve_axis_aligned_unique_edges() {
    let mut l = DebugLines::new(32);
    box_lines([0.0, 1.0, 2.0], [3.0, 5.0, 7.0], RED, &mut l);
    assert_eq!(l.len(), 12);
    let mut seen = Vec::new();
    for e in l.as_slice() {
        let diff = (0..3).filter(|&i| e.a[i] != e.b[i]).count();
        assert_eq!(diff, 1, "edge {e:?} must change exactly one axis");
        for p in [e.a, e.b] {
            assert!(p[0] == 0.0 || p[0] == 3.0);
            assert!(p[1] == 1.0 || p[1] == 5.0);
            assert!(p[2] == 2.0 || p[2] == 7.0);
        }
        let key = if e.a < e.b { (e.a, e.b) } else { (e.b, e.a) };
        assert!(!seen.contains(&key));
        seen.push(key);
        assert_eq!(e.color, RED);
    }
}

#[test]
fn polyline_segments() {
    let mut l = DebugLines::new(8);
    polyline_lines(&[], RED, &mut l);
    polyline_lines(&[[0.0; 3]], RED, &mut l);
    assert!(l.is_empty());
    let pts = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ];
    polyline_lines(&pts, RED, &mut l);
    assert_eq!(l.len(), 3);
    for (i, s) in l.as_slice().iter().enumerate() {
        assert_eq!((s.a, s.b), (pts[i], pts[i + 1]));
    }
}

#[test]
fn snapshot_draws_one_circle_per_ball_and_skips_flippers() {
    let mut s = FrameStateSnapshot::default();
    s.push_ball(BallState {
        position_m: [0.1, 0.2, 0.0135],
        ..BallState::default()
    });
    s.push_ball(BallState {
        position_m: [0.3, 0.4, 0.0],
        profile: ProfileId(1),
        ..BallState::default()
    });
    s.push_ball(BallState {
        profile: ProfileId(9),
        ..BallState::default()
    });
    s.push_flipper(FlipperState::default());
    let radius = |p: ProfileId| match p.0 {
        0 => 0.0135,
        1 => 0.02,
        _ => f32::NAN,
    };
    let mut l = DebugLines::new(256);
    snapshot_lines(&s, &radius, &mut l);
    assert_eq!(
        l.len(),
        2 * CIRCLE_SEGMENTS,
        "NaN-radius ball skipped, flipper skipped"
    );
    for (b, r) in [(0usize, 0.0135f32), (1, 0.02)] {
        let ball = s.balls()[b];
        let seg = &l.as_slice()[b * CIRCLE_SEGMENTS..(b + 1) * CIRCLE_SEGMENTS];
        for (k, line) in seg.iter().enumerate() {
            assert_eq!(line.color, BALL_COLOR);
            assert_eq!(
                line.b,
                seg[(k + 1) % CIRCLE_SEGMENTS].a,
                "joints shared exactly"
            );
            for p in [line.a, line.b] {
                let d = ((p[0] - ball.position_m[0]).powi(2) + (p[1] - ball.position_m[1]).powi(2))
                    .sqrt();
                assert!((d - r).abs() < 1e-6);
                assert_eq!(p[2], ball.position_m[2], "circle lies in the XY plane");
            }
        }
    }
    // Empty snapshot: nothing.
    l.clear();
    snapshot_lines(&FrameStateSnapshot::default(), &radius, &mut l);
    assert!(l.is_empty());
}

#[test]
fn snapshot_lines_respect_capacity() {
    let mut s = FrameStateSnapshot::default();
    for _ in 0..4 {
        s.push_ball(BallState::default());
    }
    let mut l = DebugLines::new(20);
    snapshot_lines(&s, &|_| 0.01, &mut l);
    assert_eq!(l.len(), 20);
    assert_eq!(l.dropped(), 4 * 16 - 20);
}

#[test]
fn axes_cross_identity_and_rotated() {
    let mut l = DebugLines::new(8);
    let ball = BallState {
        position_m: [1.0, 2.0, 3.0],
        ..BallState::default()
    };
    ball_axes_lines(&ball, 0.5, &mut l);
    let s = l.as_slice();
    assert_eq!(s.len(), 3);
    assert_eq!(s[0].b, [1.5, 2.0, 3.0]);
    assert_eq!(s[1].b, [1.0, 2.5, 3.0]);
    assert_eq!(s[2].b, [1.0, 2.0, 3.5]);
    // 90° about Z: X axis → +Y.
    let h = core::f32::consts::FRAC_1_SQRT_2;
    let rotated = BallState {
        orientation: [0.0, 0.0, h, h],
        ..BallState::default()
    };
    l.clear();
    ball_axes_lines(&rotated, 1.0, &mut l);
    let x = l.as_slice()[0].b;
    assert!(x[0].abs() < 1e-6 && (x[1] - 1.0).abs() < 1e-6 && x[2].abs() < 1e-6);
}

#[test]
fn ortho_camera_maps_corners() {
    let c = Camera::ortho_top_down([0.0, 0.0], [1.0, 1.0]);
    assert_eq!(c.project([0.0, 0.0, 5.0]), [-1.0, -1.0, 0.5, 1.0]);
    assert_eq!(c.project([1.0, 1.0, -5.0]), [1.0, 1.0, 0.5, 1.0]);
    assert_eq!(c.project([0.5, 0.25, 0.0]), [0.0, -0.5, 0.5, 1.0]);
    let d = Camera::ortho_top_down([2.0, 3.0], [2.0, 3.0]);
    assert!(d.view_proj.iter().flatten().all(|v| v.is_finite()));
    assert_eq!(
        Camera::default().project([0.1, 0.2, 0.3]),
        [0.1, 0.2, 0.3, 1.0]
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// The ortho camera maps the rectangle's corners onto clip ±1 and keeps
    /// X right / Y up, for any non-degenerate rectangle.
    #[test]
    fn ortho_maps_rect_to_clip(x0 in -100.0f32..100.0, y0 in -100.0f32..100.0,
                               w in 0.01f32..100.0, h in 0.01f32..100.0,
                               u in 0.0f32..1.0, v in 0.0f32..1.0) {
        let c = Camera::ortho_top_down([x0, y0], [x0 + w, y0 + h]);
        let lo = c.project([x0, y0, 0.0]);
        let hi = c.project([x0 + w, y0 + h, 0.0]);
        let tol = 1e-3;
        prop_assert!((lo[0] + 1.0).abs() < tol && (lo[1] + 1.0).abs() < tol);
        prop_assert!((hi[0] - 1.0).abs() < tol && (hi[1] - 1.0).abs() < tol);
        let p = c.project([x0 + u * w, y0 + v * h, 0.0]);
        prop_assert!((p[0] - (2.0 * u - 1.0)).abs() < tol);
        prop_assert!((p[1] - (2.0 * v - 1.0)).abs() < tol);
        prop_assert_eq!(p[3], 1.0);
    }

    /// Circles: 16 closed segments on the radius, for any centre and radius.
    #[test]
    fn circle_invariants(cx in -10.0f32..10.0, cy in -10.0f32..10.0, cz in -1.0f32..1.0, r in 0.001f32..1.0) {
        let mut l = DebugLines::new(16);
        circle_lines([cx, cy, cz], r, RED, &mut l);
        prop_assert_eq!(l.len(), 16);
        let s = l.as_slice();
        for k in 0..16 {
            prop_assert_eq!(s[k].b, s[(k + 1) % 16].a);
            let d = ((s[k].a[0] - cx).powi(2) + (s[k].a[1] - cy).powi(2)).sqrt();
            prop_assert!((d - r).abs() <= 1e-5 * (1.0 + cx.abs().max(cy.abs())));
        }
    }

    /// Pushing never exceeds capacity; len + dropped = pushes.
    #[test]
    fn push_accounting(cap in 0usize..64, n in 0usize..200) {
        let mut l = DebugLines::new(cap);
        let line = DebugLine::new([0.0; 3], [0.0; 3], RED);
        for _ in 0..n { l.push(line); }
        prop_assert_eq!(l.len(), n.min(cap));
        prop_assert_eq!(l.len() as u64 + l.dropped(), n as u64);
    }
}
