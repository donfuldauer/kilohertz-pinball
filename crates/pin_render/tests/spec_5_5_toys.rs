//! Subphase 5.5 spec tests (Independent Tester), GPU-free: `ToyRig` score
//! reels.
//!
//! * A reel commanded to digit `d` settles at `d·36°` (modulo 360) for
//!   d = 0..9.
//! * It settles along the shortest path: 9 → 0 turns +36° through 360/0,
//!   never back through 324 → 0; in general the angle travelled is the
//!   circular distance between the two digits, and the distance to the
//!   target never grows.
//! * Settling takes a bounded number of steps (2 s of simulated time at
//!   1 ms steps for the longest, 5-digit, move) and the reel then stays put.
//! * Transforms are finite and bounded for any command sequence and step
//!   size (proptest).
//!
//! The rig API is reached only through `spec_5_5_api.rs`.

#[path = "spec_5_5_api.rs"]
mod api;

use proptest::prelude::*;

/// Simulation step, seconds.
const DT: f32 = 1.0e-3;
/// Bound on the steps of any settle (2 s at 1 ms).
const MAX_SETTLE_STEPS: usize = 2000;
/// Angle tolerance for "at the target", degrees.
const TOL_DEG: f32 = 0.01;

/// Signed shortest angular difference `b − a`, in (−180, 180].
fn wrap_delta(a: f32, b: f32) -> f32 {
    let mut d = (b - a) % 360.0;
    if d > 180.0 {
        d -= 360.0;
    }
    if d <= -180.0 {
        d += 360.0;
    }
    d
}

fn target(d: u8) -> f32 {
    (f32::from(d) * 36.0) % 360.0
}

/// Steps reel 0 until it is within `TOL_DEG` of digit `d`'s angle; returns
/// the angles seen (one per step) and whether it got there within the
/// bound.
fn settle(r: &mut api::ToyRig, reel: usize, d: u8) -> (Vec<f32>, bool) {
    let mut seen = Vec::new();
    for _ in 0..MAX_SETTLE_STEPS {
        api::step(r, DT);
        let a = api::angle(r, reel);
        seen.push(a);
        if wrap_delta(a, target(d)).abs() <= TOL_DEG {
            return (seen, true);
        }
    }
    (seen, false)
}

#[test]
fn fresh_reel_shows_zero() {
    let r = api::reel_rig(3);
    for i in 0..3 {
        assert!(
            wrap_delta(api::angle(&r, i), 0.0).abs() <= TOL_DEG,
            "reel {i}: {}",
            api::angle(&r, i)
        );
    }
}

#[test]
fn every_digit_settles_at_d_times_36_degrees() {
    for d in 0..10u8 {
        let mut r = api::reel_rig(1);
        api::set_digit(&mut r, 0, d);
        let (seen, ok) = settle(&mut r, 0, d);
        assert!(
            ok,
            "digit {d}: not settled in {MAX_SETTLE_STEPS} steps (last {:?})",
            seen.last()
        );
        let a = api::angle(&r, 0);
        assert!(
            (0.0..360.0).contains(&a),
            "digit {d}: angle {a} outside [0, 360)"
        );
        assert!(
            wrap_delta(a, f32::from(d) * 36.0).abs() <= TOL_DEG,
            "digit {d}: {a}"
        );
        // Stays there.
        for _ in 0..500 {
            api::step(&mut r, DT);
        }
        assert!(
            wrap_delta(api::angle(&r, 0), target(d)).abs() <= TOL_DEG,
            "digit {d} drifted"
        );
    }
}

#[test]
fn nine_to_zero_wraps_forward_by_36_degrees() {
    let mut r = api::reel_rig(1);
    api::set_digit(&mut r, 0, 9);
    assert!(settle(&mut r, 0, 9).1);
    let start = api::angle(&r, 0);
    assert!(wrap_delta(start, 324.0).abs() <= TOL_DEG);
    api::set_digit(&mut r, 0, 0);
    let (seen, ok) = settle(&mut r, 0, 0);
    assert!(ok, "9 -> 0 did not settle");
    let mut prev = start;
    let mut travelled = 0.0f32;
    for &a in &seen {
        // Every intermediate angle lies on the short arc 324° → 360°/0°.
        assert!(
            a >= 324.0 - TOL_DEG || a <= TOL_DEG,
            "9 -> 0 passed through {a}° (the long way round)"
        );
        let d = wrap_delta(prev, a);
        assert!(d >= -TOL_DEG, "9 -> 0 moved backwards by {d}° at {a}°");
        travelled += d;
        prev = a;
    }
    assert!((travelled - 36.0).abs() <= 0.1, "travelled {travelled}°");
    // And back: 0 -> 9 is −36°.
    api::set_digit(&mut r, 0, 9);
    let (seen, ok) = settle(&mut r, 0, 9);
    assert!(ok);
    for &a in &seen {
        assert!(
            a >= 324.0 - TOL_DEG || a <= TOL_DEG,
            "0 -> 9 passed through {a}°"
        );
    }
}

#[test]
fn reels_are_independent() {
    let mut r = api::reel_rig(4);
    for (i, d) in [1u8, 2, 3, 4].into_iter().enumerate() {
        api::set_digit(&mut r, i, d);
    }
    for _ in 0..MAX_SETTLE_STEPS {
        api::step(&mut r, DT);
    }
    for (i, d) in [1u8, 2, 3, 4].into_iter().enumerate() {
        assert!(
            wrap_delta(api::angle(&r, i), target(d)).abs() <= TOL_DEG,
            "reel {i}"
        );
    }
    // Changing one reel moves only that reel.
    api::set_digit(&mut r, 3, 5);
    for _ in 0..MAX_SETTLE_STEPS {
        api::step(&mut r, DT);
        for (i, d) in [1u8, 2, 3].into_iter().enumerate() {
            assert!(
                wrap_delta(api::angle(&r, i), target(d)).abs() <= TOL_DEG,
                "reel {i} moved"
            );
        }
    }
    assert!(wrap_delta(api::angle(&r, 3), target(5)).abs() <= TOL_DEG);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    /// Shortest path and bounded, monotone settle for any pair of digits.
    #[test]
    fn settle_is_shortest_path_monotone_and_bounded(from in 0u8..10, to in 0u8..10) {
        let mut r = api::reel_rig(1);
        api::set_digit(&mut r, 0, from);
        prop_assert!(settle(&mut r, 0, from).1);
        let start = api::angle(&r, 0);
        api::set_digit(&mut r, 0, to);
        let (seen, ok) = settle(&mut r, 0, to);
        prop_assert!(ok, "{} -> {} not settled in {} steps", from, to, MAX_SETTLE_STEPS);
        let short = wrap_delta(target(from), target(to));
        let mut prev = start;
        let mut travelled = 0.0f32;
        let mut dist = wrap_delta(start, target(to)).abs();
        for &a in &seen {
            prop_assert!(a.is_finite() && (0.0..360.0).contains(&a), "angle {}", a);
            travelled += wrap_delta(prev, a);
            let nd = wrap_delta(a, target(to)).abs();
            prop_assert!(nd <= dist + TOL_DEG, "{} -> {}: distance grew {} -> {}", from, to, dist, nd);
            dist = nd;
            prev = a;
        }
        // The signed travel is the shortest arc (a 180° move may go either
        // way).
        if short.abs() < 179.0 {
            prop_assert!((travelled - short).abs() <= 0.1, "{} -> {}: travelled {} want {}", from, to, travelled, short);
        } else {
            prop_assert!((travelled.abs() - 180.0).abs() <= 0.1, "{} -> {}: travelled {}", from, to, travelled);
        }
    }

    /// Transforms stay finite and bounded for any commands and step sizes.
    #[test]
    fn transforms_finite_and_bounded(
        reels in 1usize..8,
        cmds in proptest::collection::vec((0usize..8, 0u8..10, 0.0f32..0.1, 1usize..50), 1..40),
    ) {
        let mut r = api::reel_rig(reels);
        for (reel, d, dt, n) in cmds {
            api::set_digit(&mut r, reel % reels, d);
            for _ in 0..n {
                api::step(&mut r, dt);
            }
            for i in 0..reels {
                let a = api::angle(&r, i);
                prop_assert!(a.is_finite() && (0.0..360.0).contains(&a), "reel {} angle {}", i, a);
            }
            let ts = api::transforms(&r);
            prop_assert!(ts.len() >= reels, "{} transforms for {} reels", ts.len(), reels);
            for m in &ts {
                for (c, col) in m.iter().enumerate() {
                    for (k, &x) in col.iter().enumerate() {
                        prop_assert!(x.is_finite(), "non-finite transform entry {:?}", m);
                        if c < 3 && k < 3 {
                            // Rotation (and any scale) part: bounded.
                            prop_assert!(x.abs() <= 1.0e3, "{:?}", m);
                        } else {
                            prop_assert!(x.abs() <= 1.0e6, "{:?}", m);
                        }
                    }
                }
                // Homogeneous row.
                prop_assert!(m[0][3] == 0.0 && m[1][3] == 0.0 && m[2][3] == 0.0 && m[3][3] == 1.0, "{:?}", m);
            }
        }
    }
}

#[test]
fn reel_transform_rotation_follows_the_reel_angle() {
    // A reel's model matrix is a rotation about one fixed axis whose angle
    // changes by the reel's angle: the rotation part between two digits
    // differs by exactly (digit difference)·36° about that axis.
    let mut r = api::reel_rig(1);
    for _ in 0..10 {
        api::step(&mut r, DT);
    }
    let m0 = api::transforms(&r)[0];
    api::set_digit(&mut r, 0, 3);
    assert!(settle(&mut r, 0, 3).1);
    let m3 = api::transforms(&r)[0];
    // Rotation parts R0, R3 (columns 0..3, rows 0..3); relative rotation
    // Q = R3 · R0ᵀ; its angle from the trace.
    let rot = |m: [[f32; 4]; 4]| {
        [
            [m[0][0], m[0][1], m[0][2]],
            [m[1][0], m[1][1], m[1][2]],
            [m[2][0], m[2][1], m[2][2]],
        ]
    };
    let (a, b) = (rot(m0), rot(m3));
    // Column-major: a[c][r]. Q[r][s] = sum_k R3[r][k] R0[s][k].
    let mut trace = 0.0f32;
    for i in 0..3 {
        for k in 0..3 {
            trace += b[k][i] * a[k][i];
        }
    }
    // A uniform scale (mm → m, say) multiplies the sum by s².
    let s2 = a[0][0] * a[0][0] + a[0][1] * a[0][1] + a[0][2] * a[0][2];
    assert!(s2 > 0.0 && s2.is_finite());
    trace /= s2;
    let ang = libm::acosf(((trace - 1.0) * 0.5).clamp(-1.0, 1.0)).to_degrees();
    assert!(
        (ang - 108.0).abs() < 0.5,
        "relative rotation {ang}°, want 108°"
    );
    // Translation unchanged (reels turn in place).
    for k in 0..3 {
        assert!((m0[3][k] - m3[3][k]).abs() < 1e-5);
    }
}
