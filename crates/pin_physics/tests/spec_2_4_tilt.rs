//! Subphase 2.4 spec tests, verification 4: tilt bob (standalone and through
//! `MechSet`) and cabinet nudges.

#[path = "spec_2_4_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_core::ids::SwitchId;
use pin_core::schema::table::MechKind;
use pin_core::script::SwitchEdge;
use pin_physics::mech::{TiltBob, TiltBobParams};
use proptest::prelude::*;

fn default_params() -> TiltBobParams {
    TiltBobParams::from_defaults(&defaults())
}

/// Whether a bob at rest closes the tilt switch within `seconds` of a nudge.
fn closes(params: TiltBobParams, nudge: Vec3, seconds: f32) -> bool {
    let mut bob = TiltBob::new(params);
    bob.nudge(nudge);
    let mut closed = false;
    for _ in 0..(seconds / DT) as u32 {
        bob.step(DT);
        closed |= bob.closed();
        let off = bob.offset();
        assert!(off.is_finite() && bob.velocity().is_finite());
        assert!(
            off.length() <= params.gap * 1.001 + 1.0e-6,
            "bob outside its ring: {} > gap {}",
            off.length(),
            params.gap
        );
    }
    closed
}

/// Undamped analytic threshold: the nudge speed that swings the bob exactly
/// to the ring, `v² = 2·g·L·(1 − cos θ)` with `sin θ = gap / L`.
fn analytic_threshold(p: &TiltBobParams) -> f32 {
    let s = p.gap / p.length;
    let g = p.gravity.length();
    (2.0 * g * p.length * (1.0 - (1.0 - s * s).sqrt())).sqrt()
}

/// Smallest nudge magnitude (along `dir`) that closes the switch, by bisection.
fn threshold(params: TiltBobParams, dir: Vec3) -> f32 {
    let (mut lo, mut hi) = (0.0f32, 5.0f32);
    assert!(closes(params, dir * hi, 3.0));
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if closes(params, dir * mid, 3.0) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

#[test]
fn bob_at_rest_stays_open() {
    let mut bob = TiltBob::new(default_params());
    assert_eq!(bob.offset(), Vec3::ZERO);
    for _ in 0..5000 {
        bob.step(DT);
        assert!(!bob.closed());
        assert!(bob.offset().length() < 1.0e-6);
    }
}

#[test]
fn nudge_sweep_shows_a_single_threshold() {
    let p = default_params();
    for dir in [Vec3::X, Vec3::Y, dir_deg(33.0), dir_deg(250.0)] {
        // 400 magnitudes from 2.5 mm/s to 1 m/s.
        let results: Vec<(f32, bool)> = (1..=400)
            .map(|i| {
                let m = 0.0025 * i as f32;
                (m, closes(p, dir * m, 3.0))
            })
            .collect();
        let first_closed = results.iter().position(|r| r.1).expect("no nudge tilted");
        assert!(first_closed > 0, "the smallest nudge already tilts");
        for (i, (m, c)) in results.iter().enumerate() {
            assert_eq!(
                *c,
                i >= first_closed,
                "dir {dir:?}: nudge {m} m/s closed = {c}, threshold at {} m/s (not monotone)",
                results[first_closed].0
            );
        }
        let thr = results[first_closed].0;
        let analytic = analytic_threshold(&p);
        eprintln!("tilt: dir {dir:?} threshold {thr:.4} m/s (undamped analytic {analytic:.4})");
        assert!(
            thr >= analytic * 0.98 && thr <= analytic * 1.10,
            "threshold {thr} m/s vs analytic {analytic} m/s"
        );
        // Far above: still closed.
        for m in [1.5f32, 3.0, 10.0] {
            assert!(closes(p, dir * m, 3.0), "nudge {m} m/s did not tilt");
        }
    }
}

#[test]
fn threshold_is_the_same_in_every_direction() {
    let p = default_params();
    let reference = threshold(p, Vec3::X);
    for k in 1..12 {
        let t = threshold(p, dir_deg(30.0 * k as f32 + 5.0));
        assert!(
            (t - reference).abs() < 0.01 * reference,
            "direction {k}: threshold {t} vs {reference}"
        );
    }
    // A vertical cabinet velocity change does not swing the bob.
    assert!(!closes(p, Vec3::Z * 0.5, 2.0));
}

#[test]
fn threshold_follows_gap_and_length() {
    let base = default_params();
    for (length, gap) in [
        (0.12f32, 0.006f32),
        (0.12, 0.024),
        (0.06, 0.012),
        (0.20, 0.012),
    ] {
        let p = TiltBobParams {
            length,
            gap,
            damping: 0.0,
            ..base
        };
        let t = threshold(p, Vec3::X);
        let a = analytic_threshold(&p);
        assert!(
            (t - a).abs() < 0.03 * a,
            "L {length} gap {gap}: threshold {t}, analytic {a}"
        );
    }
    // More damping never lowers the threshold.
    let mut last = 0.0;
    for damping in [0.0f32, 0.5, 2.0, 8.0] {
        let t = threshold(TiltBobParams { damping, ..base }, Vec3::X);
        assert!(
            t >= last * 0.999,
            "damping {damping}: threshold {t} < {last}"
        );
        last = t;
    }
}

#[test]
fn small_swing_period_and_decay() {
    let p = TiltBobParams {
        damping: 0.0,
        ..default_params()
    };
    let mut bob = TiltBob::new(p);
    let expected = 2.0 * std::f32::consts::PI * (p.length / 9.81).sqrt();
    assert!((bob.small_swing_period() - expected).abs() < 1.0e-4);
    bob.nudge(Vec3::new(-0.02, 0.0, 0.0));
    // Count sign changes of the x offset over 20 periods.
    let ticks = (20.0 * expected / DT) as u32;
    let mut crossings = 0;
    let mut prev = 0.0f32;
    let mut first = None;
    let mut last = 0;
    for i in 0..ticks {
        bob.step(DT);
        let x = bob.offset().x;
        if prev != 0.0 && x.signum() != prev.signum() {
            crossings += 1;
            first.get_or_insert(i);
            last = i;
        }
        if x != 0.0 {
            prev = x;
        }
        assert!(!bob.closed());
    }
    let period = 2.0 * (last - first.unwrap()) as f32 * DT / (crossings - 1) as f32;
    assert!(
        (period - expected).abs() < 0.02 * expected,
        "period {period} s, 2π√(L/g) = {expected} s"
    );
    // Damped: the swing dies out and the bob returns to rest.
    let p = TiltBobParams {
        damping: 2.0,
        ..default_params()
    };
    let mut bob = TiltBob::new(p);
    bob.nudge(Vec3::new(0.05, 0.03, 0.0));
    let mut peak_early = 0.0f32;
    for _ in 0..500 {
        bob.step(DT);
        peak_early = peak_early.max(bob.offset().length());
    }
    for _ in 0..10_000 {
        bob.step(DT);
    }
    assert!(peak_early > 1.0e-3);
    assert!(
        bob.offset().length() < 0.01 * peak_early,
        "swing did not decay"
    );
    assert!(bob.velocity().length() < 1.0e-3);
}

#[test]
fn hard_nudge_bounces_inside_the_ring_and_settles() {
    let p = default_params();
    let mut bob = TiltBob::new(p);
    bob.nudge(Vec3::new(1.0, 0.4, 0.0));
    let mut closed_ticks = 0;
    for _ in 0..30_000 {
        bob.step(DT);
        closed_ticks += u32::from(bob.closed());
        assert!(bob.offset().length() <= p.gap * 1.001 + 1.0e-6);
    }
    assert!(closed_ticks >= 1);
    assert!(
        !bob.closed(),
        "tilt switch stuck closed 30 s after the nudge"
    );
    assert!(bob.offset().length() < 0.2 * p.gap);
}

#[test]
fn invalid_input_is_ignored() {
    let mut bob = TiltBob::new(default_params());
    bob.nudge(Vec3::new(f32::NAN, 0.0, 0.0));
    bob.nudge(Vec3::new(0.0, f32::INFINITY, 0.0));
    bob.step(f32::NAN);
    bob.step(-1.0);
    bob.step(0.0);
    bob.step(DT);
    assert!(bob.offset().length() < 1.0e-9 && !bob.closed());
    // Invalid parameters fall back to usable ones.
    let bad = TiltBobParams {
        length: -1.0,
        gap: f32::NAN,
        ..default_params()
    };
    assert!(!bad.is_valid());
    let mut bob = TiltBob::new(bad);
    assert!(bob.params().is_valid());
    bob.nudge(Vec3::X);
    for _ in 0..1000 {
        bob.step(DT);
        assert!(bob.offset().is_finite());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Monotone: if a nudge tilts, every stronger nudge in the same direction tilts.
    #[test]
    fn prop_tilt_monotone(angle in 0.0f32..360.0, a in 0.0f32..0.6, b in 0.0f32..0.6,
                          gap in 0.004f32..0.03, damping in 0.0f32..3.0) {
        let p = TiltBobParams { gap, damping, ..default_params() };
        let (lo, hi) = if a < b { (a, b) } else { (b, a) };
        let dir = dir_deg(angle);
        let c_lo = closes(p, dir * lo, 2.0);
        let c_hi = closes(p, dir * hi, 2.0);
        prop_assert!(!c_lo || c_hi, "nudge {lo} tilts but {hi} does not");
        let thr = analytic_threshold(&p);
        if hi < 0.97 * thr { prop_assert!(!c_hi); }
        if lo > thr * (1.05 + 0.2 * damping) { prop_assert!(c_lo); }
    }

    /// The bob never leaves the ring and stays finite under random nudging.
    #[test]
    fn prop_bob_stays_inside_the_ring(seq in prop::collection::vec((-2.0f32..2.0, -2.0f32..2.0, -1.0f32..1.0, 1u32..200), 1..12)) {
        let p = default_params();
        let mut bob = TiltBob::new(p);
        for (x, y, z, ticks) in seq {
            bob.nudge(Vec3::new(x, y, z));
            for _ in 0..ticks {
                bob.step(DT);
                prop_assert!(bob.offset().is_finite());
                prop_assert!(bob.offset().length() <= p.gap * 1.001 + 1.0e-6);
            }
        }
    }
}

// ------------------------------------------------------- through MechSet

const TILT_SW: SwitchId = SwitchId(250);

#[test]
fn mechset_tilt_matches_the_standalone_bob() {
    let p = default_params();
    let thr = threshold(p, Vec3::X);
    for (scale, expect) in [(0.5f32, false), (0.9, false), (1.1, true), (3.0, true)] {
        let mut rig = Rig::in_space(&[]);
        rig.mechs.set_tilt_switch(Some(TILT_SW));
        let mut bob = TiltBob::new(p);
        rig.run(5);
        assert!(!rig.mechs.tilt_closed());
        assert_eq!(rig.mechs.tilt_bob_offset(), Vec3::ZERO);
        let dv = Vec3::new(thr * scale, 0.0, 0.0);
        rig.mechs.nudge(dv);
        bob.nudge(dv);
        let mut closed = false;
        let mut closed_edges = 0;
        let mut opened_edges = 0;
        for _ in 0..3000 {
            rig.step();
            bob.step(DT);
            assert_eq!(rig.mechs.tilt_closed(), bob.closed());
            assert!((rig.mechs.tilt_bob_offset() - bob.offset()).length() < 1.0e-6);
            assert_eq!(rig.switch_bit(TILT_SW), rig.mechs.tilt_closed());
            match rig.edge(TILT_SW) {
                Some(SwitchEdge::Closed) => {
                    assert!(rig.mechs.tilt_closed());
                    closed_edges += 1;
                }
                Some(SwitchEdge::Opened) => {
                    assert!(!rig.mechs.tilt_closed());
                    opened_edges += 1;
                }
                None => {}
            }
            closed |= rig.mechs.tilt_closed();
        }
        assert_eq!(
            closed, expect,
            "nudge {scale}× threshold: closed = {closed}"
        );
        assert_eq!(closed_edges > 0, expect);
        assert_eq!(closed_edges, opened_edges, "every closure reopens");
    }
}

#[test]
fn mechset_nudge_sweep_is_monotone() {
    let mut first_closed = None;
    for i in 1..=60 {
        let m = 0.005 * i as f32;
        let mut rig = Rig::in_space(&[]);
        rig.mechs.nudge(dir_deg(140.0) * m);
        let mut closed = false;
        for _ in 0..1500 {
            rig.step();
            closed |= rig.mechs.tilt_closed();
        }
        if closed {
            first_closed.get_or_insert(m);
        } else {
            assert!(
                first_closed.is_none(),
                "nudge {m} below a tilting nudge {first_closed:?}"
            );
        }
    }
    let thr = first_closed.expect("no nudge tilted");
    let a = analytic_threshold(&default_params());
    assert!(
        thr > 0.9 * a && thr < 1.15 * a,
        "threshold {thr}, analytic {a}"
    );
}

#[test]
fn tilt_switch_is_not_reported_unless_configured() {
    let mut rig = Rig::in_space(&[]);
    rig.mechs.nudge(Vec3::X);
    let mut closed = false;
    for _ in 0..1000 {
        rig.step();
        closed |= rig.mechs.tilt_closed();
        assert!(rig.mechs.edges().is_empty());
        let mut out = [0u64; 4];
        rig.mechs.switch_states(&mut out);
        assert_eq!(out, [0; 4]);
    }
    assert!(closed);
}

#[test]
fn nudge_changes_every_ball_by_minus_dv_once() {
    let mut rig = Rig::in_space(&[spec(MechKind::Gate, Vec3::ZERO)]);
    let starts = [
        (Vec3::new(0.0, 0.0, 0.5), Vec3::ZERO),
        (Vec3::new(0.2, 0.1, 0.5), Vec3::new(1.0, -2.0, 0.3)),
        (Vec3::new(-0.3, 0.4, 0.7), Vec3::new(-0.5, 0.0, 0.0)),
    ];
    let hs: Vec<_> = starts
        .iter()
        .map(|(p, v)| rig.add(steel_ball(*p, *v)))
        .collect();
    rig.run(3);
    // Two nudges between ticks add.
    let dv = Vec3::new(0.3, -0.2, 0.0);
    rig.mechs.nudge(Vec3::new(0.1, -0.2, 0.0));
    rig.mechs.nudge(Vec3::new(0.2, 0.0, 0.0));
    rig.mechs.nudge(Vec3::new(f32::NAN, 0.0, 0.0)); // ignored
    rig.step();
    for (h, (_, v0)) in hs.iter().zip(&starts) {
        let v = rig.ball(*h).velocity;
        assert!(
            (v - (*v0 - dv)).length() < 1.0e-6,
            "ball velocity {v:?}, expected {:?}",
            *v0 - dv
        );
    }
    // Applied once only.
    rig.run(50);
    for (h, (_, v0)) in hs.iter().zip(&starts) {
        assert!((rig.ball(*h).velocity - (*v0 - dv)).length() < 1.0e-6);
    }
}

#[test]
fn nudge_moves_a_resting_ball_against_the_cabinet_motion() {
    let mut rig = Rig::on_floor(&[]);
    let h = rig.add(steel_ball(Vec3::new(0.0, 0.0, R), Vec3::ZERO));
    rig.run(100);
    rig.mechs.nudge(Vec3::new(0.2, 0.0, 0.0));
    rig.run(100);
    let b = rig.ball(h);
    assert!(
        b.position.x < -0.005,
        "ball should lag the cabinet: {:?}",
        b.position
    );
    assert!(b.position.y.abs() < 1.0e-4);
}
