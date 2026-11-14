//! Independent GPU-free spec tests for Subphase 5.2 (reflection probes and HDR post).
//!
//! Written from the brief (`docs/briefs/wave5/5.2.md`) and the published contract
//! (DECISIONS "5.2 — Reflection probes and post public API"), without reading the
//! implementation. Covers the numeric helpers and the probe scheduler:
//!
//! * octahedral encode / decode (round trip, range, documented orientation);
//! * `ProbeScheduler`: budget ≤ 2 updates per frame, round-robin fairness, probe age bound
//!   ⌈n / budget⌉, nearest-probe reuse for balls without a fresh probe (model based);
//! * `aces_film` against the Narkowicz 2015 fit at reference sample points;
//! * `bloom_prefilter` (threshold, soft knee, firefly clamp, hue, energy bound);
//! * `screen_motion` (projection of a ball's motion over the shutter).

#![allow(clippy::needless_range_loop)]

use pin_core::snapshot::BallState;
use pin_render::pbr::PbrCamera;
use pin_render::post::{
    BloomConfig, MotionBall, MotionBlurConfig, PostConfig, ToneMap, aces_film, bloom_prefilter,
    screen_motion,
};
use pin_render::reflections::{
    CUBE_FACE_RESOLUTION, MAX_PROBES, PROBE_BUDGET, PROBE_RESOLUTION, ProbeScheduler, oct_decode,
    oct_encode,
};
use proptest::prelude::*;

type V3 = [f32; 3];

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
fn lum(c: V3) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// A unit vector from two uniform numbers (uniform on the sphere).
fn sphere_dir(u: f32, v: f32) -> V3 {
    let z = 2.0 * u - 1.0;
    let r = (1.0 - z * z).max(0.0).sqrt();
    let phi = std::f32::consts::TAU * v;
    [r * phi.cos(), r * phi.sin(), z]
}

// ---------------------------------------------------------------------------------------
// Constants fixed by the brief
// ---------------------------------------------------------------------------------------

#[test]
fn constants_match_the_brief() {
    // "at most 2 probe updates per frame at 128²".
    assert_eq!(PROBE_BUDGET, 2);
    assert_eq!(PROBE_RESOLUTION, 128);
    assert_eq!(MAX_PROBES, pin_core::snapshot::MAX_BALLS);
    const { assert!(MAX_PROBES >= 6, "6 balls must each be able to own a probe") };
    const { assert!(CUBE_FACE_RESOLUTION >= 16) };
}

// ---------------------------------------------------------------------------------------
// Octahedral mapping
// ---------------------------------------------------------------------------------------

#[test]
fn oct_up_is_the_centre_and_down_is_a_corner() {
    let c = oct_encode([0.0, 0.0, 1.0]);
    assert!(
        (c[0] - 0.5).abs() < 1e-5 && (c[1] - 0.5).abs() < 1e-5,
        "+z must map to the centre, got {c:?}"
    );
    let d = oct_encode([0.0, 0.0, -1.0]);
    for k in d {
        assert!(
            k.abs() < 1e-4 || (k - 1.0).abs() < 1e-4,
            "−z must map to a corner, got {d:?}"
        );
    }
    // Every corner decodes to −z, the centre to +z.
    for corner in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] {
        let v = oct_decode(corner);
        assert!(v[2] < -0.999, "corner {corner:?} decodes to {v:?}");
    }
    let up = oct_decode([0.5, 0.5]);
    assert!(up[2] > 0.999, "centre decodes to {up:?}");
}

#[test]
fn oct_axes_round_trip_and_are_distinct() {
    let axes: [V3; 6] = [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ];
    let mut uvs = Vec::new();
    for a in axes {
        let uv = oct_encode(a);
        let back = oct_decode(uv);
        assert!(dot(a, back) > 0.9999, "axis {a:?} → {uv:?} → {back:?}");
        uvs.push(uv);
    }
    // The four horizontal axes sit on the edge of the inner diamond |u−½| + |v−½| = ½.
    for uv in &uvs[..4] {
        let l1 = (uv[0] - 0.5).abs() + (uv[1] - 0.5).abs();
        assert!((l1 - 0.5).abs() < 1e-4, "horizontal axis at {uv:?}");
    }
    for i in 0..5 {
        for j in i + 1..5 {
            let d = (uvs[i][0] - uvs[j][0]).hypot(uvs[i][1] - uvs[j][1]);
            assert!(d > 0.2, "axes {i} and {j} map to the same texel");
        }
    }
    // Opposite horizontal axes are opposite about the centre (a z-up, non-mirrored-per-axis map).
    for (a, b) in [(0, 1), (2, 3)] {
        assert!((uvs[a][0] + uvs[b][0] - 1.0).abs() < 1e-4);
        assert!((uvs[a][1] + uvs[b][1] - 1.0).abs() < 1e-4);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// decode(encode(d)) ≈ d for every unit direction; uv stays in 0..=1.
    #[test]
    fn oct_round_trip(u in 0.0f32..1.0, v in 0.0f32..1.0) {
        let d = sphere_dir(u, v);
        let uv = oct_encode(d);
        prop_assert!(uv[0].is_finite() && uv[1].is_finite());
        prop_assert!((0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]), "uv {:?}", uv);
        let back = oct_decode(uv);
        prop_assert!((len(back) - 1.0).abs() < 1e-4, "decode not unit: {:?}", back);
        prop_assert!(dot(d, back) > 1.0 - 1e-5, "{:?} → {:?} → {:?}", d, uv, back);
    }

    /// decode gives a unit vector for every uv; the upper hemisphere is the inner diamond;
    /// encode(decode(uv)) ≈ uv away from the fold lines (where the map is 2:1 on the border).
    #[test]
    fn oct_decode_is_unit_and_inverts(u in 0.0f32..=1.0, v in 0.0f32..=1.0) {
        let d = oct_decode([u, v]);
        prop_assert!(d.iter().all(|c| c.is_finite()));
        prop_assert!((len(d) - 1.0).abs() < 1e-4, "decode({}, {}) = {:?}", u, v, d);
        let l1 = (u - 0.5).abs() + (v - 0.5).abs();
        if l1 < 0.49 {
            prop_assert!(d[2] > 0.0, "inside the diamond must be up: {:?}", d);
        }
        if l1 > 0.51 {
            prop_assert!(d[2] < 0.0, "outside the diamond must be down: {:?}", d);
        }
        let border = u.min(v).min(1.0 - u).min(1.0 - v);
        if border > 0.01 {
            let uv = oct_encode(d);
            prop_assert!((uv[0] - u).abs() < 2e-3 && (uv[1] - v).abs() < 2e-3,
                "({}, {}) → {:?} → {:?}", u, v, d, uv);
        }
    }

    /// The map is continuous on the upper hemisphere (the lower hemisphere is unfolded
    /// along the planes x = 0 and y = 0, where the square's border wraps): two directions
    /// 0.5° apart land within a few texels of each other.
    #[test]
    fn oct_is_continuous_above_the_ground(u in 0.51f32..1.0, v in 0.0f32..1.0, a in 0.0f32..std::f32::consts::TAU) {
        let d = sphere_dir(u, v);
        // A perpendicular nudge.
        let t = norm([d[1] - d[2] * a.sin(), d[2] * a.cos() - d[0], d[0] * a.sin() - d[1] * a.cos() + 1e-3]);
        let t = norm([t[0] - d[0] * dot(t, d), t[1] - d[1] * dot(t, d), t[2] - d[2] * dot(t, d)]);
        let e = 0.0087f32;
        let d2 = norm([d[0] + t[0] * e, d[1] + t[1] * e, d[2] + t[2] * e]);
        prop_assume!(d2[2] >= 0.0);
        let (p, q) = (oct_encode(d), oct_encode(d2));
        let dist = (p[0] - q[0]).hypot(p[1] - q[1]);
        prop_assert!(dist < 0.02, "{:?} and {:?} map {} apart", d, d2, dist);
    }
}

// ---------------------------------------------------------------------------------------
// Probe scheduler
// ---------------------------------------------------------------------------------------

fn line(n: usize, frame: usize) -> Vec<V3> {
    (0..n)
        .map(|i| [0.1 * i as f32, 0.01 * frame as f32, 0.0])
        .collect()
}

fn ceil_div(n: usize, b: usize) -> u32 {
    n.div_ceil(b) as u32
}

#[test]
fn scheduler_budget_is_clamped_to_the_brief() {
    assert_eq!(ProbeScheduler::new(0).stats().budget, 1);
    assert_eq!(ProbeScheduler::new(1).stats().budget, 1);
    assert_eq!(ProbeScheduler::new(2).stats().budget, 2);
    assert_eq!(ProbeScheduler::new(3).stats().budget, PROBE_BUDGET);
    assert_eq!(ProbeScheduler::new(u32::MAX).stats().budget, PROBE_BUDGET);
}

/// Verification criterion 3 (scheduler level): 6 balls, ≥ 60 frames, never more than 2
/// updates per frame, every probe age ≤ ⌈6/2⌉ = 3 frames.
#[test]
fn six_balls_never_exceed_the_budget_and_ages_stay_bounded() {
    let n = 6;
    let frames = 120;
    let mut s = ProbeScheduler::new(PROBE_BUDGET);
    let mut counts = [0u32; 6];
    let mut last = [None::<usize>; 6];
    let mut total = 0u64;
    for f in 0..frames {
        let pos = line(n, f);
        let upd = s.schedule(&pos).to_vec();
        assert!(upd.len() <= 2, "frame {f}: {} updates", upd.len());
        let mut seen = [false; 6];
        for &i in &upd {
            assert!(i < n, "frame {f}: ball index {i} out of range");
            assert!(!seen[i], "frame {f}: ball {i} updated twice");
            seen[i] = true;
            counts[i] += 1;
            last[i] = Some(f);
        }
        total += upd.len() as u64;
        let ages = s.ages().to_vec();
        assert_eq!(ages.len(), n, "one age per ball");
        for (i, &a) in ages.iter().enumerate() {
            assert!(a <= 3, "frame {f}: ball {i} age {a} > ⌈6/2⌉");
            // Age = frames since the capture (0 = this frame).
            if let Some(l) = last[i] {
                assert_eq!(a as usize, f - l, "frame {f}: ball {i} age");
            }
        }
        let st = s.stats();
        assert_eq!(st.frames, f as u64 + 1);
        assert_eq!(st.last_frame_updates as usize, upd.len());
        assert!(st.max_frame_updates <= 2);
        assert_eq!(st.total_updates, total);
        assert_eq!(st.balls, 6);
        assert_eq!(st.max_age, *ages.iter().max().unwrap());
        assert!(st.max_age_ever >= st.max_age && st.max_age_ever <= 3);
        if f >= 2 {
            assert!(
                last.iter().all(|l| l.is_some()),
                "after ⌈6/2⌉ frames every ball has its own probe"
            );
        }
    }
    // Round-robin fairness: equal shares.
    let (lo, hi) = (*counts.iter().min().unwrap(), *counts.iter().max().unwrap());
    assert!(hi - lo <= 1, "unfair update counts {counts:?}");
    assert_eq!(
        total,
        2 * frames as u64,
        "6 balls use the whole budget each frame"
    );
    // Strict round robin: any 3 consecutive frames update each ball exactly once.
    let mut s = ProbeScheduler::new(2);
    let mut window: Vec<Vec<usize>> = Vec::new();
    for f in 0..60 {
        window.push(s.schedule(&line(n, f)).to_vec());
        if window.len() >= 3 {
            let mut all: Vec<usize> = window[window.len() - 3..]
                .iter()
                .flatten()
                .copied()
                .collect();
            all.sort_unstable();
            assert_eq!(all, vec![0, 1, 2, 3, 4, 5], "frames {}..={f}", f - 2);
        }
    }
}

#[test]
fn scheduler_edge_counts() {
    // No balls: nothing to do, no panic.
    let mut s = ProbeScheduler::new(2);
    for _ in 0..5 {
        assert!(s.schedule(&[]).is_empty());
    }
    assert_eq!(s.stats().total_updates, 0);
    assert_eq!(s.probe_for_ball(0), None);
    assert!(s.ages().is_empty());

    // Fewer balls than the budget: all fresh every frame.
    for n in 1..=2usize {
        let mut s = ProbeScheduler::new(2);
        for f in 0..10 {
            let mut upd = s.schedule(&line(n, f)).to_vec();
            upd.sort_unstable();
            assert_eq!(upd, (0..n).collect::<Vec<_>>());
            assert!(s.ages().iter().all(|&a| a == 0));
            for i in 0..n {
                assert_eq!(s.probe_for_ball(i), Some(i));
            }
        }
    }

    // Before any capture no ball has a probe.
    let s = ProbeScheduler::new(2);
    assert_eq!(s.probe_for_ball(0), None);
    assert_eq!(s.probe_for_ball(5), None);
}

#[test]
fn balls_past_max_probes_reuse_the_nearest_probe() {
    let n = MAX_PROBES + 4;
    let mut s = ProbeScheduler::new(2);
    let pos: Vec<V3> = (0..n).map(|i| [i as f32, 0.0, 0.0]).collect();
    for f in 0..(4 * n) {
        let upd = s.schedule(&pos).to_vec();
        assert!(upd.len() <= 2);
        for &i in &upd {
            assert!(
                i < MAX_PROBES,
                "frame {f}: ball {i} past MAX_PROBES got its own probe"
            );
        }
    }
    for i in 0..MAX_PROBES {
        assert_eq!(s.probe_for_ball(i), Some(i));
    }
    // Balls on a line past the last probe: the nearest captured probe is the last slot.
    for i in MAX_PROBES..n {
        assert_eq!(s.probe_for_ball(i), Some(MAX_PROBES - 1), "ball {i}");
    }
    // The 16 probe owners keep the bound ⌈16/2⌉.
    for (i, &a) in s.ages().iter().enumerate().take(MAX_PROBES) {
        assert!(a <= ceil_div(MAX_PROBES, 2), "ball {i} age {a}");
    }
}

/// Nearest-probe reuse, spelled out: with a budget of 1 and three balls on a line, the two
/// balls without a probe use the only captured one; then the nearer of the two.
#[test]
fn nearest_probe_reuse_step_by_step() {
    let pos: [V3; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [10.0, 0.0, 0.0]];
    let mut s = ProbeScheduler::new(1);
    let first = s.schedule(&pos).to_vec();
    assert_eq!(first.len(), 1);
    let a = first[0];
    for i in 0..3 {
        assert_eq!(s.probe_for_ball(i), Some(a), "only probe {a} exists");
    }
    let second = s.schedule(&pos).to_vec();
    assert_eq!(second.len(), 1);
    let b = second[0];
    assert_ne!(a, b, "round robin moves on");
    let c = 3 - a - b;
    let d = |i: usize, j: usize| (pos[i][0] - pos[j][0]).abs();
    let want = if d(c, a) < d(c, b) { a } else { b };
    assert_eq!(s.probe_for_ball(c), Some(want));
    assert_eq!(s.probe_for_ball(a), Some(a));
    assert_eq!(s.probe_for_ball(b), Some(b));
    let third = s.schedule(&pos).to_vec();
    assert_eq!(third, vec![c]);
    for i in 0..3 {
        assert_eq!(s.probe_for_ball(i), Some(i));
    }
}

/// "Nearest" is measured to the position the probe was captured at, and ties go to the
/// lower slot.
#[test]
fn nearest_uses_capture_positions_and_ties_take_the_lower_slot() {
    // Tie: ball 2 is equidistant from balls 0 and 1.
    let pos: [V3; 3] = [[-1.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 3.0, 0.0]];
    let mut s = ProbeScheduler::new(2);
    let mut upd = s.schedule(&pos).to_vec();
    upd.sort_unstable();
    if upd == [0, 1] {
        assert_eq!(s.probe_for_ball(2), Some(0), "tie → lower slot");
    }

    // Swap (published staleness rule): four balls, budget 1, capture order 0, 1, 2, 3, …
    // After every slot holds a capture, balls 0 and 1 swap places in a frame that captures
    // neither. Each then samples the probe captured nearest its current position (the
    // other ball's slot), not its own index's slot; captures stay strict round robin.
    let p: [V3; 4] = [
        [0.0, 0.0, 0.0],
        [0.1, 0.0, 0.0],
        [3.0, 0.0, 0.0],
        [6.0, 0.0, 0.0],
    ];
    let swapped: [V3; 4] = [p[1], p[0], p[2], p[3]];
    let mut s = ProbeScheduler::new(1);
    for f in 0..6 {
        assert_eq!(s.schedule(&p).to_vec(), vec![f % 4], "frame {f}");
    }
    for i in 0..4 {
        assert_eq!(s.probe_for_ball(i), Some(i));
        assert!(!s.is_stale(i), "still ball {i} is fresh");
    }
    // Frame 6 captures slot 2 while 0 and 1 swap.
    assert_eq!(s.schedule(&swapped).to_vec(), vec![2]);
    assert!(s.is_stale(0) && s.is_stale(1), "swapped balls are stale");
    assert!(!s.is_stale(2) && !s.is_stale(3));
    assert_eq!(
        s.probe_for_ball(0),
        Some(1),
        "ball 0 sits where slot 1 was captured"
    );
    assert_eq!(
        s.probe_for_ball(1),
        Some(0),
        "ball 1 sits where slot 0 was captured"
    );
    assert_eq!(s.probe_for_ball(2), Some(2));
    assert_eq!(s.probe_for_ball(3), Some(3));
    // Ages are the true frames since capture, stale or not (0 at frame 4, 1 at frame 5).
    assert_eq!(s.ages().to_vec(), vec![2, 1, 0, 3]);
    // Frame 7 captures slot 3; nothing moved, the swapped balls stay stale.
    assert_eq!(s.schedule(&swapped).to_vec(), vec![3]);
    assert_eq!(s.probe_for_ball(0), Some(1));
    assert_eq!(s.probe_for_ball(1), Some(0));
    // Frame 8 captures slot 0 at ball 0's new place: fresh again. Ball 1 is still stale;
    // slots 0 and 1 were both captured 0.1 m from it, the tie goes to the lower slot.
    assert_eq!(s.schedule(&swapped).to_vec(), vec![0]);
    assert!(!s.is_stale(0) && s.is_stale(1));
    assert_eq!(s.probe_for_ball(0), Some(0));
    assert_eq!(s.probe_for_ball(1), Some(0));
    // Frame 9 captures slot 1: everything is fresh.
    assert_eq!(s.schedule(&swapped).to_vec(), vec![1]);
    for i in 0..4 {
        assert_eq!(s.probe_for_ball(i), Some(i));
        assert!(!s.is_stale(i));
    }

    // Jump: a ball that moves further than 0.25 m in one frame (relaunch) samples the
    // probe captured nearest its new position until its own slot is captured again; the
    // capture order does not react to the jump.
    let mut s = ProbeScheduler::new(1);
    for f in 0..4 {
        assert_eq!(s.schedule(&p).to_vec(), vec![f]);
    }
    let mut jumped = p;
    jumped[3] = [3.0, 0.3, 0.0];
    assert_eq!(
        s.schedule(&jumped).to_vec(),
        vec![0],
        "the jump does not jump the queue"
    );
    assert!(s.is_stale(3));
    assert_eq!(
        s.probe_for_ball(3),
        Some(2),
        "nearest capture to the new place"
    );
    assert_eq!(s.schedule(&jumped).to_vec(), vec![1]);
    assert_eq!(s.schedule(&jumped).to_vec(), vec![2]);
    assert_eq!(s.probe_for_ball(3), Some(2));
    assert_eq!(s.schedule(&jumped).to_vec(), vec![3]);
    assert!(!s.is_stale(3));
    assert_eq!(s.probe_for_ball(3), Some(3));
    // A move within the tracking distance keeps the ball's own probe.
    let mut nudged = jumped;
    nudged[2] = [3.0, -0.2, 0.0];
    s.schedule(&nudged);
    assert!(!s.is_stale(2));
    assert_eq!(s.probe_for_ball(2), Some(2));
}

/// Reference model of the documented scheduler behaviour that does not depend on the
/// update order: budget, distinct indices, age bookkeeping, nearest reuse, forgetting.
struct Model {
    /// Capture position per slot.
    captured: Vec<Option<V3>>,
    /// Frames since capture (or since first seen).
    ages: Vec<u32>,
    /// Whether slot i's capture is stale for ball i; `None` = the published rule does not
    /// decide (a tie for the nearest ball of the previous frame).
    stale: Vec<Option<bool>>,
}

fn dist2(a: V3, b: V3) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    dot(d, d)
}

impl Model {
    /// The published capture order: never-captured slots first, then the oldest capture,
    /// ties → lower index. Call before ageing; `first` = the first frame.
    fn expect_updates(&self, budget: usize) -> Vec<usize> {
        let mut order: Vec<usize> = (0..self.captured.len()).collect();
        order.sort_by_key(|&i| {
            let rank = if self.captured[i].is_some() {
                self.ages[i]
            } else {
                u32::MAX
            };
            (std::cmp::Reverse(rank), i)
        });
        order.truncate(budget);
        order
    }

    /// Staleness between two frames (grid positions: a move is 0 or ≥ 1 m > 0.25 m).
    fn track(&mut self, prev: &[V3], pos: &[V3]) {
        for i in 0..pos.len() {
            if dist2(pos[i], prev[i]) > 0.0 {
                self.stale[i] = Some(true);
                continue;
            }
            // Did not move: its own previous position is at distance 0; another ball that
            // was at the same place is a tie the contract does not order.
            if self.stale[i] == Some(false)
                && (0..prev.len()).any(|j| j != i && dist2(prev[j], pos[i]) == 0.0)
            {
                self.stale[i] = None;
            }
        }
    }

    /// `None` = undecided by the contract; `Some(x)` = `probe_for_ball` must return `x`.
    fn expect_probe(&self, i: usize, pos: &[V3]) -> Option<Option<usize>> {
        if self.captured[i].is_some() {
            match self.stale[i] {
                Some(false) => return Some(Some(i)),
                None => return None,
                Some(true) => {}
            }
        }
        let mut best: Option<(f32, usize)> = None;
        for (slot, c) in self.captured.iter().enumerate() {
            let Some(c) = c else { continue };
            let d2 = dist2(*c, pos[i]);
            if best.is_none_or(|(b, _)| d2 < b) {
                best = Some((d2, slot));
            }
        }
        Some(best.map(|(_, s)| s))
    }
}

fn grid_positions(seed: &[u8], n: usize, frame: usize) -> Vec<V3> {
    // Integer coordinates: distances are exact, so "nearest" has no rounding ambiguity.
    (0..n)
        .map(|i| {
            let k = seed[(i * 3 + frame * 7) % seed.len()] as usize;
            [
                ((k + i * 5) % 9) as f32,
                ((k / 3 + i) % 7) as f32,
                ((k / 5 + frame) % 3) as f32,
            ]
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// For every ball count and budget: ≤ budget updates per frame, distinct valid indices,
    /// ages ≤ ⌈n/b⌉ at all times, fair shares, and the documented probe assignment.
    #[test]
    fn scheduler_invariants(
        n in 1usize..=MAX_PROBES,
        budget in 1u32..=2,
        frames in 1usize..80,
        seed in proptest::collection::vec(any::<u8>(), 16..64),
    ) {
        let mut s = ProbeScheduler::new(budget);
        let b = budget as usize;
        let bound = ceil_div(n, b);
        let mut m = Model { captured: vec![None; n], ages: vec![0; n], stale: vec![Some(false); n] };
        let mut prev: Option<Vec<V3>> = None;
        let mut counts = vec![0u32; n];
        for f in 0..frames {
            let pos = grid_positions(&seed, n, f);
            let want_upd = m.expect_updates(b);
            let upd = s.schedule(&pos).to_vec();
            prop_assert!(upd.len() <= b, "frame {}: {} updates > budget {}", f, upd.len(), b);
            let mut got_sorted = upd.clone();
            got_sorted.sort_unstable();
            let mut want_sorted = want_upd.clone();
            want_sorted.sort_unstable();
            prop_assert_eq!(got_sorted, want_sorted, "frame {}: capture order must not depend on positions", f);
            if let Some(prev) = &prev {
                m.track(prev, &pos);
            }
            prev = Some(pos.clone());
            prop_assert_eq!(upd.len(), b.min(n), "round robin uses the budget");
            let mut sorted = upd.clone();
            sorted.sort_unstable();
            sorted.dedup();
            prop_assert_eq!(sorted.len(), upd.len(), "duplicate ball in {:?}", upd);
            for a in m.ages.iter_mut() {
                if f > 0 { *a += 1; }
            }
            for &i in &upd {
                prop_assert!(i < n);
                m.captured[i] = Some(pos[i]);
                m.ages[i] = 0;
                m.stale[i] = Some(false);
                counts[i] += 1;
            }
            let ages = s.ages();
            prop_assert_eq!(ages.len(), n);
            for i in 0..n {
                prop_assert!(ages[i] <= bound, "frame {}: ball {} age {} > {}", f, i, ages[i], bound);
                if m.captured[i].is_some() {
                    prop_assert_eq!(ages[i], m.ages[i], "frame {}: ball {} age", f, i);
                }
                if let Some(want) = m.expect_probe(i, &pos) {
                    prop_assert_eq!(s.probe_for_ball(i), want,
                        "frame {}: ball {} probe (captures {:?}, stale {:?}, positions {:?})", f, i, m.captured, m.stale, pos);
                }
                if let (Some(st), true) = (m.stale[i], m.captured[i].is_some()) {
                    prop_assert_eq!(s.is_stale(i), st, "frame {}: ball {} staleness", f, i);
                }
            }
            let st = s.stats();
            prop_assert!(st.max_frame_updates <= budget);
            prop_assert!(st.max_age_ever <= bound);
            prop_assert_eq!(st.balls as usize, n);
            prop_assert_eq!(st.frames, f as u64 + 1);
        }
        let (lo, hi) = (*counts.iter().min().unwrap(), *counts.iter().max().unwrap());
        prop_assert!(hi - lo <= 1, "unfair shares {:?}", counts);
        if frames >= bound as usize {
            prop_assert!(counts.iter().all(|&c| c >= 1), "a ball was starved: {:?}", counts);
        }
    }

    /// A changing ball count keeps the budget, and after ⌈n/b⌉ frames at a stable count the
    /// age bound holds again; dropped slots are forgotten.
    #[test]
    fn scheduler_survives_a_changing_ball_count(
        counts in proptest::collection::vec(0usize..=10, 2..8),
        budget in 1u32..=2,
    ) {
        let mut s = ProbeScheduler::new(budget);
        let b = budget as usize;
        for (phase, &n) in counts.iter().enumerate() {
            let pos: Vec<V3> = (0..n).map(|i| [i as f32, phase as f32, 0.0]).collect();
            let settle = ceil_div(n.max(1), b) as usize;
            for f in 0..(settle + 3) {
                let upd = s.schedule(&pos).to_vec();
                prop_assert!(upd.len() <= b);
                prop_assert!(upd.iter().all(|&i| i < n), "phase {}: index out of range {:?}", phase, upd);
                prop_assert_eq!(s.ages().len(), n);
                if f + 1 >= settle {
                    let bound = ceil_div(n.max(1), b);
                    prop_assert!(s.ages().iter().all(|&a| a <= bound),
                        "phase {} (n = {}) frame {}: ages {:?}", phase, n, f, s.ages());
                    for i in 0..n {
                        prop_assert_eq!(s.probe_for_ball(i), Some(i));
                    }
                }
                for i in 0..n {
                    if let Some(p) = s.probe_for_ball(i) {
                        prop_assert!(p < n, "phase {}: ball {} uses forgotten slot {}", phase, i, p);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// ACES
// ---------------------------------------------------------------------------------------

/// Narkowicz 2015, "ACES Filmic Tone Mapping Curve", in f64.
fn aces_ref(x: f64) -> f64 {
    ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0)
}

#[test]
fn aces_matches_the_reference_fit_at_sample_points() {
    // Hand-computed values of the fit (independent of `aces_ref`).
    let table: [(f32, f32); 8] = [
        (0.0, 0.0),
        (0.02, 0.010_499),
        (0.18, 0.266_90),
        (0.5, 0.616_307),
        (1.0, 0.803_797),
        (2.0, 0.914_855),
        (4.0, 0.973_417),
        (8.0, 1.0),
    ];
    for (x, want) in table {
        let got = aces_film(x);
        assert!(
            (got - want).abs() < 2e-4,
            "aces_film({x}) = {got}, reference {want}"
        );
        assert!((aces_ref(x as f64) as f32 - want).abs() < 2e-5);
    }
    // Dense comparison over the useful range.
    for i in 0..=4000 {
        let x = i as f32 * 0.004;
        let got = aces_film(x);
        let want = aces_ref(x as f64) as f32;
        assert!(
            (got - want).abs() < 1e-4,
            "aces_film({x}) = {got} vs {want}"
        );
    }
    // Saturation: the fit reaches 1 near x ≈ 7.24 and stays there.
    assert!(aces_film(7.0) < 1.0);
    assert_eq!(aces_film(7.5), 1.0);
    assert_eq!(aces_film(60_000.0), 1.0);
    // Mid grey stays mid: no hidden pre-scale (0.6 × would give 0.17).
    assert!((aces_film(0.18) - 0.2669).abs() < 1e-3);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    #[test]
    fn aces_is_bounded_and_monotone(a in 0.0f32..100.0, b in 0.0f32..100.0) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let (fl, fh) = (aces_film(lo), aces_film(hi));
        prop_assert!((0.0..=1.0).contains(&fl) && (0.0..=1.0).contains(&fh));
        prop_assert!(fl <= fh + 1e-6, "not monotone: f({}) = {} > f({}) = {}", lo, fl, hi, fh);
    }

    /// Any radiance the chain can meet (f16 HDR × exposure, and negative garbage) maps to a
    /// displayable value. (Beyond |x| ≈ 1.8e19 the reference overflows x² and returns NaN:
    /// reported as a non-blocking finding, not asserted here.)
    #[test]
    fn aces_output_is_always_displayable(x in -1.0e6f32..1.0e6) {
        let y = aces_film(x);
        prop_assert!(y.is_finite() && (0.0..=1.0).contains(&y), "aces_film({}) = {}", x, y);
    }
}

// ---------------------------------------------------------------------------------------
// Post configuration defaults and the bloom prefilter
// ---------------------------------------------------------------------------------------

#[test]
fn post_defaults_match_the_published_contract() {
    let c = PostConfig::default();
    assert_eq!(c.exposure, 1.0);
    assert!(c.bloom.enabled);
    assert!(matches!(c.tone_map, ToneMap::AcesFilm));
    assert!(c.motion_blur.enabled);
    let b = BloomConfig::default();
    assert!(
        b.threshold > 0.0,
        "N7: a bloom threshold keeps LDR content out of the bloom"
    );
    assert!(b.firefly_clamp.is_finite() && b.firefly_clamp > b.threshold);
    assert!(
        b.intensity > 0.0 && b.intensity < 1.0,
        "energy-conserving bloom needs intensity < 1"
    );
    assert!(b.radius > 0.0 && b.radius < 1.0);
    assert!(b.max_levels >= 1);
    let m = MotionBlurConfig::default();
    assert!(m.shutter_s > 0.0 && m.shutter_s <= 1.0 / 30.0);
    assert!(m.samples >= 4);
    assert!(m.max_length_px > 0.0);
}

#[test]
fn bloom_prefilter_threshold_and_clamp() {
    let cfg = BloomConfig::default();
    assert_eq!(bloom_prefilter([0.0; 3], &cfg), [0.0; 3]);
    // Below the knee nothing blooms.
    let below = (cfg.threshold - cfg.knee - 1e-3).max(0.0);
    let out = bloom_prefilter([below; 3], &cfg);
    assert!(
        out.iter().all(|&c| c.abs() < 1e-6),
        "grey {below} is below the knee but blooms {out:?}"
    );
    // Well above the knee something blooms, never more than the pixel has.
    let g = cfg.threshold + cfg.knee + 2.0;
    let out = bloom_prefilter([g; 3], &cfg);
    assert!(out[0] > 0.5 && out[0] <= g + 1e-5, "grey {g} → {out:?}");
    // Firefly clamp (5.1 N7): a 3000-nit chrome highlight contributes at most the clamp.
    let out = bloom_prefilter([3000.0; 3], &cfg);
    assert!(
        out[0] <= cfg.firefly_clamp * 1.001,
        "firefly 3000 blooms {out:?} > clamp {}",
        cfg.firefly_clamp
    );
    assert!(
        out[0] >= cfg.firefly_clamp * 0.9,
        "a firefly still blooms at the clamp, got {out:?}"
    );
    let out = bloom_prefilter([60_000.0, 0.0, 0.0], &cfg);
    assert!(
        lum(out) <= cfg.firefly_clamp * 1.02,
        "saturated red: {out:?}"
    );
    // threshold 0, knee 0: the whole image blooms (identity below the clamp).
    let all = BloomConfig {
        threshold: 0.0,
        knee: 0.0,
        ..cfg
    };
    for c in [[0.01, 0.02, 0.03], [0.5, 0.5, 0.5], [3.0, 1.0, 0.2]] {
        let out = bloom_prefilter(c, &all);
        for k in 0..3 {
            assert!(
                (out[k] - c[k]).abs() <= 1e-4 * c[k].max(1.0),
                "threshold 0: {c:?} → {out:?}"
            );
        }
    }
    // Bad input never produces NaN.
    for bad in [f32::NAN, f32::INFINITY, -1.0] {
        let out = bloom_prefilter([bad, 1.0, 1.0], &cfg);
        assert!(
            out.iter().all(|c| c.is_finite() && *c >= 0.0),
            "prefilter({bad}) = {out:?}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// 0 ≤ bright ≤ c per channel (so c − intensity·bright stays non-negative: the energy
    /// accounting of the bloom model), hue is kept, luminance is clamped, and the result
    /// grows with the input.
    #[test]
    fn bloom_prefilter_invariants(
        r in 0.0f32..50.0, g in 0.0f32..50.0, b in 0.0f32..50.0,
        scale in 1.0f32..4.0,
        threshold in 0.0f32..4.0, knee in 0.0f32..2.0, clamp in 4.0f32..200.0,
    ) {
        let cfg = BloomConfig { threshold, knee, firefly_clamp: clamp, ..BloomConfig::default() };
        let c = [r, g, b];
        let out = bloom_prefilter(c, &cfg);
        for k in 0..3 {
            prop_assert!(out[k].is_finite());
            prop_assert!(out[k] >= 0.0 && out[k] <= c[k] * 1.0001 + 1e-6,
                "bright {:?} exceeds the pixel {:?}", out, c);
        }
        prop_assert!(lum(out) <= clamp * 1.02 + 1e-4, "luminance {} > clamp {}", lum(out), clamp);
        // Hue: out ∝ c.
        let (lo, lc) = (lum(out), lum(c));
        if lo > 1e-3 && lc > 1e-3 {
            let k = lo / lc;
            for ch in 0..3 {
                prop_assert!((out[ch] - k * c[ch]).abs() <= 2e-3 * (1.0 + c[ch]),
                    "hue changed: {:?} → {:?}", c, out);
            }
        }
        // Nothing below the knee.
        if lc < threshold - knee - 1e-3 {
            prop_assert!(lo < 1e-5, "{:?} (lum {}) is below the knee but blooms {:?}", c, lc, out);
        }
        // Monotone in brightness.
        let brighter = bloom_prefilter([r * scale, g * scale, b * scale], &cfg);
        prop_assert!(lum(brighter) >= lo - 1e-3 * (1.0 + lo),
            "brighter input blooms less: {} → {}", lo, lum(brighter));
    }
}

// ---------------------------------------------------------------------------------------
// Screen-space ball motion
// ---------------------------------------------------------------------------------------

const CAM_X: f32 = 0.6;
const FOV_Y: f32 = 0.25;

fn camera() -> PbrCamera {
    // On +X looking at the origin, z up: image right = +Y, image up = +Z.
    PbrCamera::look_at(
        [CAM_X, 0.0, 0.0],
        [0.0; 3],
        [0.0, 0.0, 1.0],
        FOV_Y,
        1.0,
        0.1,
        5.0,
    )
}

fn px_per_m(size: u32) -> f32 {
    size as f32 * 0.5 / (CAM_X * (FOV_Y * 0.5).tan())
}

#[test]
fn still_ball_has_zero_screen_motion() {
    let ball = MotionBall {
        position: [0.0; 3],
        velocity: [0.0; 3],
        radius: 0.0135,
    };
    let m = screen_motion(&camera(), &ball, 1.0 / 120.0, 128, 128).expect("ball in view");
    assert_eq!(m.displacement_px, [0.0, 0.0]);
    assert!(
        (m.center_px[0] - 64.0).abs() < 0.51 && (m.center_px[1] - 64.0).abs() < 0.51,
        "centre {:?}",
        m.center_px
    );
    let want = 0.0135 * px_per_m(128);
    assert!(
        (m.radius_px - want).abs() < 0.05 * want + 0.5,
        "radius {} px, expected ≈ {want}",
        m.radius_px
    );
}

#[test]
fn screen_motion_follows_the_velocity() {
    let cam = camera();
    let shutter = 1.0 / 120.0;
    let k = px_per_m(128);
    // (velocity, expected pixel direction): +Y is image right, +Z is image up (row 0 = top).
    let cases: [(V3, [f32; 2]); 6] = [
        ([0.0, 2.0, 0.0], [1.0, 0.0]),
        ([0.0, -2.0, 0.0], [-1.0, 0.0]),
        ([0.0, 0.0, 2.0], [0.0, -1.0]),
        ([0.0, 0.0, -2.0], [0.0, 1.0]),
        ([0.0, 1.5, 1.5], [0.707, -0.707]),
        ([0.0, -1.0, 2.0], [-0.447, -0.894]),
    ];
    for (v, dir) in cases {
        let ball = MotionBall {
            position: [0.0; 3],
            velocity: v,
            radius: 0.0135,
        };
        let m = screen_motion(&cam, &ball, shutter, 128, 128).expect("ball in view");
        let d = m.displacement_px;
        let l = d[0].hypot(d[1]);
        let want = len(v) * shutter * k;
        assert!(
            (l - want).abs() < 0.04 * want + 0.1,
            "v {v:?}: |d| = {l} px, expected {want}"
        );
        let cosang = (d[0] * dir[0] + d[1] * dir[1]) / l;
        assert!(
            cosang > 0.995,
            "v {v:?}: displacement {d:?} not along {dir:?}"
        );
    }
    // Motion along the view axis barely moves the centre.
    let ball = MotionBall {
        position: [0.0; 3],
        velocity: [2.0, 0.0, 0.0],
        radius: 0.0135,
    };
    let m = screen_motion(&cam, &ball, shutter, 128, 128).expect("ball in view");
    assert!(m.displacement_px[0].hypot(m.displacement_px[1]) < 0.5);
}

#[test]
fn screen_motion_centre_tracks_the_position_and_rejects_balls_behind_the_camera() {
    let cam = camera();
    let k = px_per_m(128);
    let ball = MotionBall {
        position: [0.0, 0.02, 0.03],
        velocity: [0.0; 3],
        radius: 0.01,
    };
    let m = screen_motion(&cam, &ball, 0.01, 128, 128).expect("ball in view");
    assert!(
        (m.center_px[0] - (64.0 + 0.02 * k)).abs() < 0.6,
        "{:?}",
        m.center_px
    );
    assert!(
        (m.center_px[1] - (64.0 - 0.03 * k)).abs() < 0.6,
        "{:?}",
        m.center_px
    );
    // Non-square target: x scales with the height (fov is vertical), centre at w/2.
    let wide = PbrCamera::look_at(
        [CAM_X, 0.0, 0.0],
        [0.0; 3],
        [0.0, 0.0, 1.0],
        FOV_Y,
        2.0,
        0.1,
        5.0,
    );
    let m = screen_motion(&wide, &ball, 0.01, 256, 128).expect("ball in view");
    assert!(
        (m.center_px[0] - (128.0 + 0.02 * k)).abs() < 0.6,
        "{:?}",
        m.center_px
    );
    assert!(
        (m.center_px[1] - (64.0 - 0.03 * k)).abs() < 0.6,
        "{:?}",
        m.center_px
    );
    // Behind the camera: nothing to blur.
    let behind = MotionBall {
        position: [CAM_X + 1.0, 0.0, 0.0],
        velocity: [0.0, 3.0, 0.0],
        radius: 0.0135,
    };
    assert!(screen_motion(&cam, &behind, 0.01, 128, 128).is_none());
}

#[test]
fn motion_ball_comes_from_the_snapshot_ball_state() {
    let s = BallState {
        position_m: [0.1, 0.2, 0.3],
        velocity_m_s: [1.0, -2.0, 0.5],
        ..BallState::default()
    };
    let b = MotionBall::from_state(&s, 0.0135);
    assert_eq!(b.position, [0.1, 0.2, 0.3]);
    assert_eq!(b.velocity, [1.0, -2.0, 0.5]);
    assert_eq!(b.radius, 0.0135);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// Displacement is linear in the shutter time and in the speed, finite, and its
    /// direction does not depend on either.
    #[test]
    fn screen_motion_scales_linearly(
        vy in -4.0f32..4.0, vz in -4.0f32..4.0,
        py in -0.03f32..0.03, pz in -0.03f32..0.03,
        shutter in 0.001f32..0.02,
    ) {
        prop_assume!(vy.hypot(vz) > 0.2);
        let cam = camera();
        let ball = MotionBall { position: [0.0, py, pz], velocity: [0.0, vy, vz], radius: 0.0135 };
        let a = screen_motion(&cam, &ball, shutter, 128, 128).expect("in view");
        let b = screen_motion(&cam, &ball, 2.0 * shutter, 128, 128).expect("in view");
        prop_assert!(a.displacement_px.iter().all(|c| c.is_finite()));
        prop_assert!(a.radius_px.is_finite() && a.radius_px > 0.0);
        let (la, lb) = (a.displacement_px[0].hypot(a.displacement_px[1]),
                        b.displacement_px[0].hypot(b.displacement_px[1]));
        prop_assert!((lb - 2.0 * la).abs() < 0.03 * lb + 0.05, "{} vs 2·{}", lb, la);
        let want = vy.hypot(vz) * shutter * px_per_m(128);
        prop_assert!((la - want).abs() < 0.05 * want + 0.1, "|d| = {}, expected {}", la, want);
        let cosang = (a.displacement_px[0] * vy - a.displacement_px[1] * vz) / (la * vy.hypot(vz));
        prop_assert!(cosang > 0.99, "direction off: {:?} for v = (0, {}, {})", a.displacement_px, vy, vz);
        prop_assert_eq!(a.center_px, b.center_px);
    }
}

/// Criterion 3 does not depend on how the balls move: the budget, the round-robin shares
/// and the age bound hold for six balls whatever their per-frame displacement (slow roll,
/// 5 m/s at 60 Hz = 83 mm, a fast ball at a low frame rate, a relaunch across the table).
#[test]
fn round_robin_is_independent_of_ball_motion() {
    for step in [0.0f32, 0.001, 0.027, 0.083, 0.2, 0.25, 0.3, 0.5, 1.0] {
        let n = 6usize;
        let mut s = ProbeScheduler::new(PROBE_BUDGET);
        let mut counts = vec![0u32; n];
        let mut model_age = vec![0u32; n];
        for f in 0..66u32 {
            // Balls 60 mm apart, all travelling along x and bouncing between the walls of a
            // 0.5 m wide table (triangle wave), so every per-frame move is `step`.
            let pos: Vec<V3> = (0..n)
                .map(|i| {
                    let t = (i as f32 * 0.06 + f as f32 * step) % 1.0;
                    let x = if t < 0.5 { t } else { 1.0 - t };
                    [x, 0.1 * i as f32, 0.0135]
                })
                .collect();
            let upd = s.schedule(&pos).to_vec();
            assert!(
                upd.len() <= PROBE_BUDGET as usize,
                "step {step} frame {f}: {upd:?}"
            );
            for a in model_age.iter_mut() {
                *a += 1;
            }
            for &i in &upd {
                counts[i] += 1;
                model_age[i] = 0;
            }
            // Frames since each ball really was scheduled, independent of `ages()`.
            if f >= 2 {
                for (i, a) in model_age.iter().enumerate() {
                    assert!(
                        *a <= 3,
                        "step {step} m/frame, frame {f}: ball {i} not updated for {a} frames (bound ⌈6/2⌉ = 3)"
                    );
                }
            }
            for (i, a) in s.ages().iter().enumerate() {
                assert!(*a <= 3, "step {step} frame {f}: ball {i} age {a}");
            }
        }
        let (lo, hi) = (*counts.iter().min().unwrap(), *counts.iter().max().unwrap());
        assert!(
            hi - lo <= 1,
            "step {step} m/frame: unfair probe shares {counts:?} over 66 frames"
        );
        assert_eq!(
            counts.iter().sum::<u32>(),
            132,
            "step {step}: budget not used"
        );
    }
}

/// Strict round robin for every ball count, odd ones included: over any run the balls'
/// capture counts differ by at most one (with 7 balls and budget 2 the order is
/// 0 1 | 2 3 | 4 5 | 6 0 | 1 2 | …, never favouring the low indices).
#[test]
fn odd_ball_counts_share_the_budget_fairly() {
    for n in [3usize, 5, 7, 9, 15] {
        let pos: Vec<V3> = (0..n).map(|i| [i as f32 * 0.06, 0.0, 0.0135]).collect();
        let mut s = ProbeScheduler::new(PROBE_BUDGET);
        let mut counts = vec![0u32; n];
        for f in 0..(20 * n) {
            for &i in s.schedule(&pos) {
                counts[i] += 1;
            }
            let (lo, hi) = (*counts.iter().min().unwrap(), *counts.iter().max().unwrap());
            assert!(
                hi - lo <= 1,
                "{n} balls, budget 2, after {} frames: capture counts {counts:?}",
                f + 1
            );
        }
    }
}
