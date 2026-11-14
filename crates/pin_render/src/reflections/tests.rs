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

/// The captured slot nearest to `p` (what a ball without a fresh probe of
/// its own must sample).
fn nearest_capture(s: &ProbeScheduler, p: [f32; 3]) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    for slot in 0..MAX_PROBES {
        if let Some(c) = s.capture_position(slot) {
            let d = (c[0] - p[0]).powi(2) + (c[1] - p[1]).powi(2) + (c[2] - p[2]).powi(2);
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, slot));
            }
        }
    }
    best.map(|(_, s)| s)
}

/// 5.2 review N3: a ball draining from the front of the list shifts the
/// others down; their old slots no longer describe them, so they sample the
/// nearest capture (their own previous one where it still exists) instead
/// of a neighbour's, and the round robin stays oldest-first.
#[test]
fn draining_ball_does_not_hand_its_probe_to_the_next_ball() {
    let mut s = ProbeScheduler::new(PROBE_BUDGET);
    let balls: Vec<[f32; 3]> = (0..6).map(|i| [0.03 * i as f32, 0.0, 0.0]).collect();
    for _ in 0..6 {
        s.schedule(&balls);
    }
    assert_eq!(s.ages(), &[2, 2, 1, 1, 0, 0]);
    // Ball 0 drains; the others move a little and shift down one index.
    let after: Vec<[f32; 3]> = balls[1..]
        .iter()
        .map(|p| [p[0] + 0.002, p[1], p[2]])
        .collect();
    let up = s.schedule(&after).to_vec();
    // Oldest first, ages carried over by index: [3, 3, 2, 2, 1] → 0 and 1.
    assert_eq!(up, vec![0, 1]);
    for (j, &p) in after.iter().enumerate() {
        let slot = s.probe_for_ball(j).unwrap();
        if up.contains(&j) {
            assert_eq!(slot, j);
        } else {
            assert!(s.is_stale(j), "ball {j} keeps a neighbour's probe");
            assert_eq!(Some(slot), nearest_capture(&s, p));
            // Its own previous capture (old slot j + 1) where it still exists.
            if j + 1 < after.len() {
                assert_eq!(slot, j + 1);
            }
        }
    }
    assert!(!s.is_captured(5), "slots past the ball count are freed");
    for _ in 0..10 {
        s.schedule(&after);
        assert!(s.stats().max_age <= 2, "ages {:?}", s.ages());
    }
    for j in 0..after.len() {
        assert_eq!(s.probe_for_ball(j), Some(j));
        assert!(!s.is_stale(j));
    }
    // A ball that jumps further than the tracking distance
    // (TRACK_MAX_JUMP_M) samples the
    // probe nearest to where it is now.
    let mut jumped = after.clone();
    jumped[2] = [5.0, 0.0, 0.0];
    s.schedule(&jumped);
    if !s.last_updates().contains(&2) {
        assert!(s.is_stale(2));
        assert_eq!(s.probe_for_ball(2), nearest_capture(&s, jumped[2]));
    }
}

proptest! {
    /// Under random arrivals and departures (balls at fixed, distinct
    /// places, kept in id order), the budget holds every frame, a ball
    /// samples its own slot only while that slot was captured at its own
    /// place and otherwise the nearest capture, and once the set is steady
    /// every age is < ⌈n / budget⌉ again within that many frames.
    #[test]
    fn probes_follow_balls_through_count_changes(
        frames in proptest::collection::vec(proptest::collection::vec(any::<bool>(), 20), 1..40),
        steady in proptest::collection::vec(any::<bool>(), 20),
    ) {
        let mut s = ProbeScheduler::new(PROBE_BUDGET);
        let place = |id: usize| [id as f32, 0.5 * id as f32, 0.0];
        let run = |s: &mut ProbeScheduler, present: &[bool]| -> Result<(), TestCaseError> {
            let pos: Vec<[f32; 3]> = (0..20).filter(|&i| present[i]).map(place).collect();
            let u = s.schedule(&pos).len();
            prop_assert!(u <= PROBE_BUDGET as usize);
            for (j, &p) in pos.iter().enumerate() {
                let Some(slot) = s.probe_for_ball(j) else { continue };
                if slot == j && j < MAX_PROBES && !s.is_stale(j) {
                    prop_assert_eq!(s.capture_position(slot), Some(p), "ball {} owns a foreign probe", j);
                } else {
                    let c = s.capture_position(slot).unwrap();
                    let d = |c: [f32; 3]| (c[0] - p[0]).powi(2) + (c[1] - p[1]).powi(2);
                    let n = s.capture_position(nearest_capture(s, p).unwrap()).unwrap();
                    prop_assert!(d(c) <= d(n), "ball {} samples a farther probe", j);
                }
            }
            Ok(())
        };
        for f in &frames {
            run(&mut s, f)?;
        }
        let n = steady.iter().filter(|&&b| b).count().min(MAX_PROBES);
        let period = n.div_ceil(PROBE_BUDGET as usize).max(1);
        for _ in 0..period {
            run(&mut s, &steady)?;
        }
        for _ in 0..2 * period {
            run(&mut s, &steady)?;
            prop_assert!((s.stats().max_age as usize) < period, "ages {:?}", s.ages());
        }
    }
}

/// Odd ball counts: the cursor wraps (0 1 | 2 0 | 1 2 for three balls), so
/// over any n frames the capture counts differ by at most 1, and a count
/// change keeps the cursor.
#[test]
fn odd_ball_counts_share_the_budget_cyclically() {
    let pos = |n: usize| -> Vec<[f32; 3]> { (0..n).map(|i| [i as f32, 0.0, 0.0]).collect() };
    let mut s = ProbeScheduler::new(2);
    let order: Vec<Vec<usize>> = (0..3).map(|_| s.schedule(&pos(3)).to_vec()).collect();
    assert_eq!(order, vec![vec![0, 1], vec![2, 0], vec![1, 2]]);
    for n in 1..=MAX_PROBES {
        let mut s = ProbeScheduler::new(2);
        let mut counts = vec![0u32; n];
        let mut frames = Vec::new();
        for _ in 0..3 * n {
            frames.push(s.schedule(&pos(n)).to_vec());
            assert!(
                (s.stats().max_age as usize) < n.div_ceil(2),
                "n {n}: ages {:?}",
                s.ages()
            );
        }
        for w in frames.windows(n) {
            counts.iter_mut().for_each(|c| *c = 0);
            for f in w {
                for &i in f {
                    counts[i] += 1;
                }
            }
            let (lo, hi) = (counts.iter().min().unwrap(), counts.iter().max().unwrap());
            assert!(hi - lo <= 1, "n {n}: shares {counts:?}");
        }
    }
    // The cursor survives a count change: 7 balls, 2 frames (next is 4),
    // then 5 balls → 4 0, then 1 2.
    let mut s = ProbeScheduler::new(2);
    s.schedule(&pos(7));
    s.schedule(&pos(7));
    assert_eq!(s.schedule(&pos(5)), &[4, 0]);
    assert_eq!(s.schedule(&pos(5)), &[1, 2]);
    // Growing: 3 balls, [0, 1] then [2, 0] leave the cursor at 1 (the index
    // after the last capture, modulo the count at that frame); 7 balls then
    // continue from 1.
    let mut s = ProbeScheduler::new(2);
    assert_eq!(s.schedule(&pos(3)), &[0, 1]);
    assert_eq!(s.schedule(&pos(3)), &[2, 0]);
    assert_eq!(s.schedule(&pos(7)), &[1, 2]);
    assert_eq!(s.schedule(&pos(7)), &[3, 4]);
}
