use super::*;
use proptest::prelude::*;

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[test]
fn octahedral_landmarks() {
    assert_eq!(oct_encode([0.0, 0.0, 1.0]), [0.5, 0.5]);
    assert_eq!(oct_encode([1.0, 0.0, 0.0]), [1.0, 0.5]);
    assert_eq!(oct_encode([0.0, 1.0, 0.0]), [0.5, 1.0]);
    assert_eq!(oct_encode([-1.0, 0.0, 0.0]), [0.0, 0.5]);
    assert_eq!(oct_encode([0.0, 0.0, 0.0]), [0.5, 0.5]);
    assert_eq!(oct_encode([f32::NAN, 0.0, 1.0]), [0.5, 0.5]);
    let down = oct_encode([0.0, 0.0, -1.0]);
    assert!(down == [1.0, 1.0], "{down:?}");
    for c in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
        let d = oct_decode(c);
        assert!((d[2] + 1.0).abs() < 1e-6, "{c:?} -> {d:?}");
    }
    assert_eq!(oct_decode([0.5, 0.5]), [0.0, 0.0, 1.0]);
    assert_eq!(oct_decode([f32::NAN, f32::NAN]), [0.0, 0.0, 1.0]);
    // Out-of-range input is clamped.
    assert_eq!(oct_decode([7.0, 0.5]), oct_decode([1.0, 0.5]));
}

#[test]
fn face_bases_are_orthonormal_right_handed_and_cover_the_axes() {
    let mut seen = [[0.0f32; 3]; 6];
    for (k, s) in seen.iter_mut().enumerate() {
        let (r, u, f) = face_basis(k);
        for v in [r, u, f] {
            assert!((dot(v, v) - 1.0).abs() < 1e-5);
        }
        assert!(dot(r, u).abs() < 1e-6 && dot(r, f).abs() < 1e-6 && dot(u, f).abs() < 1e-6);
        // look_at: right = forward × up.
        let c = cross(f, u);
        assert!((dot(c, r) - 1.0).abs() < 1e-5, "face {k}");
        *s = f;
    }
    for axis in 0..3 {
        for sign in [1.0f32, -1.0] {
            assert!(
                seen.iter().any(|f| (f[axis] - sign).abs() < 1e-6),
                "axis {axis} sign {sign} missing"
            );
        }
    }
}

proptest! {
    #[test]
    fn octahedral_round_trip(x in -1.0f32..1.0, y in -1.0f32..1.0, z in -1.0f32..1.0) {
        let l = (x * x + y * y + z * z).sqrt();
        prop_assume!(l > 1e-3);
        let d = [x / l, y / l, z / l];
        let uv = oct_encode(d);
        prop_assert!((0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]));
        let back = oct_decode(uv);
        prop_assert!((dot(back, back) - 1.0).abs() < 1e-5);
        prop_assert!(dot(back, d) > 1.0 - 1e-5, "{d:?} -> {uv:?} -> {back:?}");
    }

    #[test]
    fn decode_then_encode_is_identity_on_the_map(u in 0.0f32..=1.0, v in 0.0f32..=1.0) {
        let d = oct_decode([u, v]);
        let uv = oct_encode(d);
        // The fold edges (|x| + |y| = 1) and the corners are shared; compare
        // the directions there instead of the coordinates.
        let back = oct_decode(uv);
        prop_assert!(dot(back, d) > 1.0 - 1e-5);
        let on_edge = ((u * 2.0 - 1.0).abs() + (v * 2.0 - 1.0).abs() - 1.0).abs() < 1e-3;
        if !on_edge {
            prop_assert!((uv[0] - u).abs() < 1e-4 && (uv[1] - v).abs() < 1e-4, "{u},{v} -> {uv:?}");
        }
    }

    #[test]
    fn decode_never_panics_or_returns_non_finite(u in proptest::num::f32::ANY, v in proptest::num::f32::ANY) {
        let d = oct_decode([u, v]);
        prop_assert!(d.iter().all(|c| c.is_finite()));
    }

    /// Budget and age bound for any steady ball count and budget, from any
    /// earlier history.
    #[test]
    fn scheduler_budget_and_age_bound(
        history in proptest::collection::vec(0usize..=20, 0..12),
        n in 0usize..=20,
        budget in 0u32..=4,
    ) {
        let mut s = ProbeScheduler::new(budget);
        let b = budget.clamp(1, PROBE_BUDGET) as usize;
        let pos = |k: usize| -> Vec<[f32; 3]> { (0..k).map(|i| [i as f32, 0.0, 0.0]).collect() };
        for &k in &history {
            let u = s.schedule(&pos(k)).len();
            prop_assert!(u <= b);
        }
        let owned = n.min(MAX_PROBES);
        let period = owned.div_ceil(b).max(1);
        // After one full period every owned ball has been captured.
        for _ in 0..period {
            let u = s.schedule(&pos(n)).to_vec();
            prop_assert!(u.len() <= b && u.len() == b.min(owned));
            prop_assert!(u.iter().all(|&i| i < owned));
        }
        for _ in 0..3 * period {
            s.schedule(&pos(n));
            for (i, &a) in s.ages().iter().enumerate() {
                prop_assert!((a as usize) < period, "ball {i} age {a}, period {period}");
            }
            for i in 0..n {
                let p = s.probe_for_ball(i);
                prop_assert!(p.is_some());
                if i < owned {
                    prop_assert_eq!(p, Some(i));
                }
            }
        }
        let st = s.stats();
        prop_assert!(st.max_frame_updates as usize <= b);
        prop_assert_eq!(st.balls as usize, n);
        prop_assert_eq!(st.budget as usize, b);
    }
}

#[test]
fn six_balls_round_robin_two_per_frame() {
    let mut s = ProbeScheduler::new(PROBE_BUDGET);
    let balls: Vec<[f32; 3]> = (0..6).map(|i| [0.1 * i as f32, 0.0, 0.0]).collect();
    let mut order = Vec::new();
    for _ in 0..6 {
        order.push(s.schedule(&balls).to_vec());
    }
    assert_eq!(
        order,
        vec![
            vec![0, 1],
            vec![2, 3],
            vec![4, 5],
            vec![0, 1],
            vec![2, 3],
            vec![4, 5]
        ]
    );
    assert_eq!(s.ages(), &[2, 2, 1, 1, 0, 0]);
    let st = s.stats();
    assert_eq!(st.frames, 6);
    assert_eq!(st.total_updates, 12);
    assert_eq!(st.max_frame_updates, 2);
    assert_eq!(st.max_age_ever, 2);
    assert_eq!(s.last_updates(), &[4, 5]);
}

#[test]
fn uncaptured_balls_reuse_the_nearest_probe() {
    let mut s = ProbeScheduler::new(1);
    assert_eq!(s.probe_for_ball(0), None);
    let balls = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.2, 0.0, 0.0]];
    assert_eq!(s.schedule(&balls), &[0]);
    // Only slot 0 exists: everyone uses it.
    assert_eq!(s.probe_for_ball(1), Some(0));
    assert_eq!(s.probe_for_ball(2), Some(0));
    assert_eq!(s.schedule(&balls), &[1]);
    // Ball 2 (x = 0.2) is nearer to slot 0 (captured at 0) than slot 1 (at 1).
    assert_eq!(s.probe_for_ball(2), Some(0));
    let moved = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.9, 0.0, 0.0]];
    assert_eq!(s.schedule(&moved), &[2]);
    assert_eq!(s.probe_for_ball(2), Some(2));
    assert!(s.is_captured(2));
    assert_eq!(s.probe_for_ball(7), None);
    // Shrinking forgets the dropped slots.
    s.schedule(&moved[..1]);
    assert!(!s.is_captured(1) && !s.is_captured(2));
    assert_eq!(s.ages().len(), 1);
    // Empty frames schedule nothing.
    assert!(s.schedule(&[]).is_empty());
    assert_eq!(s.stats().max_age, 0);
}

#[test]
fn balls_past_max_probes_share_the_nearest_slot() {
    let mut s = ProbeScheduler::new(PROBE_BUDGET);
    let balls: Vec<[f32; 3]> = (0..MAX_PROBES + 2).map(|i| [i as f32, 0.0, 0.0]).collect();
    for _ in 0..MAX_PROBES {
        assert!(s.schedule(&balls).iter().all(|&i| i < MAX_PROBES));
    }
    assert_eq!(s.probe_for_ball(MAX_PROBES), Some(MAX_PROBES - 1));
    assert_eq!(s.ages().len(), MAX_PROBES + 2);
    let nan = vec![[f32::NAN; 3]; 3];
    let mut t = ProbeScheduler::new(1);
    t.schedule(&nan);
    assert_eq!(t.probe_for_ball(1), Some(0));
}
