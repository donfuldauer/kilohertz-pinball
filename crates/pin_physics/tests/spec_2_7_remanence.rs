//! Independent spec tests, Subphase 2.7, magnetic remanence (PLAN.md §3.2;
//! verification 3 and the magnetisation invariants).
//!
//! Each ball carries `M ∈ [0, 1]`: it rises toward the commanded power `P`
//! while the ball is within a magnet's range and decays as
//! `M ← M·e^(−Δt/τ)` otherwise; with the coil off the ball still feels
//! `F_res = c·ρ·M·K_coil·d̂ / max(d³, d_min³)`.
//!
//! Black box: written from the brief, against `pin_physics::ball_ext` and the
//! public `BallSolver` API.

#[path = "spec_2_7_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_core::ProfileId;
use pin_physics::ball::{Ball, BallEvents, BallHandle, Geometry};
use proptest::prelude::*;

const TAU: f32 = 0.060;

fn steel_ball(at: Vec3) -> Ball {
    Ball::new(ProfileId(0), at)
}

/// A weak magnet: in-range balls are magnetised but not moved measurably.
const WEAK: f32 = 1.0e-12;

// ------------------------------------------------------------ verification 3

#[test]
fn v3_holding_magnet_holds_and_releases() {
    // The scenario itself: the ball is held while the coil is on and rolls
    // away downhill once it is off.
    let run = hold_run(&profile("low_rem_steel"), HoldSpec::default());
    for s in &run.hold {
        let off_axis = Vec3::new(s.position.x, s.position.y, 0.0).length();
        assert!(off_axis < 0.005, "not held: {:?}", s.position);
    }
    let leave = run.leave_tick().expect("low_rem_steel never left");
    assert!((200..600).contains(&leave), "left at tick {leave}");
    assert!(run.after[leave].position.y < 0.0, "left uphill");
}

#[test]
fn v3_low_rem_steel_is_bit_identical_to_the_zero_remanence_baseline() {
    let low = profile("low_rem_steel");
    assert_eq!(low.remanence, 0.0);
    for canonical_release in [false, true] {
        for power in [1.0f32, 0.6] {
            let spec = HoldSpec {
                canonical_release,
                power,
                ..HoldSpec::default()
            };
            let hooked = hold_run(&low, spec);
            let baseline = hold_run(
                &low,
                HoldSpec {
                    hooks: false,
                    ..spec
                },
            );
            for (phase, a, b) in [
                ("hold", &hooked.hold, &baseline.hold),
                ("after", &hooked.after, &baseline.after),
            ] {
                assert_eq!(a.len(), b.len());
                for (tick, (x, y)) in a.iter().zip(b.iter()).enumerate() {
                    assert_eq!(
                        x.bits, y.bits,
                        "{phase} tick {tick} (canonical {canonical_release}, P {power})"
                    );
                }
            }
            assert_eq!(hooked.leave_tick(), baseline.leave_tick());
            assert!(hooked.leave_tick().is_some());
        }
    }
}

#[test]
fn v3_standard_steel_leaves_measurably_later_than_low_rem_steel() {
    let std = profile("standard_steel");
    let low = profile("low_rem_steel");
    assert_eq!(std.remanence, 0.05);
    // Same ball apart from the remanence.
    assert_eq!(
        (std.mass_kg, std.diameter_m, std.magnetic_coupling),
        (low.mass_kg, low.diameter_m, low.magnetic_coupling)
    );
    let a = hold_run(&std, HoldSpec::default());
    let b = hold_run(&low, HoldSpec::default());
    // The ball was magnetised by the hold.
    let m_hold = a.hold.last().unwrap().m;
    assert!(m_hold > 0.9 && m_hold <= 1.0, "M after the hold: {m_hold}");
    let (ta, tb) = (a.leave_tick().unwrap(), b.leave_tick().unwrap());
    // An f64 model of the release (rolling ball, M·e^(−t/τ)) gives ≈ 8 ms.
    assert!(
        ta >= tb + 2,
        "standard_steel left at tick {ta}, low_rem_steel at {tb}"
    );
    assert!(
        ta <= tb + 60,
        "standard_steel lingered too long: {ta} vs {tb} (ρ = 0.05, τ = 60 ms)"
    );
    // Measurably behind in position too, all the way to the exit.
    for tick in 40..tb {
        let (ya, yb) = (a.after[tick].position.y, b.after[tick].position.y);
        assert!(
            ya > yb + 5.0e-6,
            "tick {tick}: standard_steel at y={ya}, low_rem_steel at y={yb}"
        );
    }
    let lag = a.after[tb].position.y - b.after[tb].position.y;
    assert!(lag > 2.0e-4, "lag at the exit {lag} m");
    // And it does leave: the residual force dies out.
    let m_end = a.after.last().unwrap().m;
    assert!(m_end < 0.01, "M long after the release: {m_end}");
}

#[test]
fn v3_without_canonical_release_standard_steel_still_leaves_later() {
    // No test-side state reset at the switch-off: the plain experiment.
    let spec = HoldSpec {
        canonical_release: false,
        hold_ticks: 1500,
        ..HoldSpec::default()
    };
    let a = hold_run(&profile("standard_steel"), spec);
    let b = hold_run(&profile("low_rem_steel"), spec);
    let (ta, tb) = (a.leave_tick().unwrap(), b.leave_tick().unwrap());
    assert!(ta > tb, "standard_steel {ta}, low_rem_steel {tb}");
}

#[test]
fn release_delay_grows_with_remanence_and_tau() {
    let mut last = hold_run(&steel_with(0.0, 1.0), HoldSpec::default())
        .leave_tick()
        .unwrap();
    let zero = last;
    for rho in [0.05f32, 0.2, 0.5, 1.0] {
        let t = hold_run(&steel_with(rho, 1.0), HoldSpec::default())
            .leave_tick()
            .unwrap_or_else(|| panic!("ρ = {rho}: never left"));
        assert!(t > last, "ρ = {rho}: left at {t}, previous {last}");
        last = t;
    }
    // ρ = 1 holds like the coil until M has decayed: several τ.
    assert!(last >= zero + 50, "ρ = 1 left at {last}, ρ = 0 at {zero}");

    // A longer τ keeps the ball longer; ρ = 0 does not care.
    let mut last = 0;
    for tau in [0.02f32, 0.06, 0.2] {
        let spec = HoldSpec {
            tau_s: Some(tau),
            hold_ticks: 1500,
            after_ticks: 3000,
            ..HoldSpec::default()
        };
        let t = hold_run(&steel_with(0.2, 1.0), spec).leave_tick().unwrap();
        assert!(t > last, "τ = {tau}: left at {t}, previous {last}");
        last = t;
        let z = hold_run(&steel_with(0.0, 1.0), spec).leave_tick().unwrap();
        assert_eq!(z, zero, "τ = {tau} changed a ρ = 0 ball");
    }
}

#[test]
fn zero_coupling_ball_is_untouched_by_the_remanence_hook() {
    // c = 0: no magnet force and no residual force, whatever ρ says.
    let mut p = profile("ceramic_powerball");
    assert_eq!(p.magnetic_coupling, 0.0);
    p.remanence = 0.8;
    let spec = HoldSpec {
        hold_ticks: 100,
        after_ticks: 400,
        canonical_release: false,
        ..HoldSpec::default()
    };
    let hooked = hold_run(&p, spec);
    let baseline = hold_run(
        &p,
        HoldSpec {
            hooks: false,
            ..spec
        },
    );
    for (x, y) in hooked.after.iter().zip(&baseline.after) {
        assert_eq!(x.bits, y.bits);
    }
    for (x, y) in hooked.hold.iter().zip(&baseline.hold) {
        assert_eq!(x.bits, y.bits);
    }
}

// --------------------------------------------------------- magnetisation law

#[test]
fn magnetisation_starts_at_zero_and_rises_toward_power() {
    for power in [0.25f32, 0.6, 1.0] {
        let mut b = bench(&[profile("standard_steel")], WEAK);
        let h = b.solver.add_ball(steel_ball(NEAR)).unwrap();
        assert_eq!(b.m(h), 0.0, "new ball");
        b.solver.set_magnet_power(b.magnet, power);
        let mut prev = 0.0f32;
        for tick in 0..2000 {
            b.pinned(h, NEAR, 1);
            let m = b.m(h);
            assert!(m.is_finite() && (0.0..=1.0).contains(&m), "M = {m}");
            assert!(m <= power + 1.0e-6, "tick {tick}: M {m} above P {power}");
            assert!(m >= prev - 1.0e-6, "tick {tick}: M fell {prev} → {m}");
            prev = m;
        }
        assert!(prev >= 0.9 * power, "after 2 s at P = {power}: M = {prev}");
    }
}

#[test]
fn magnetisation_stays_zero_out_of_range_or_with_the_coil_off() {
    let mut b = bench(&[profile("standard_steel")], WEAK);
    let far = b.solver.add_ball(steel_ball(FAR)).unwrap();
    b.solver.set_magnet_power(b.magnet, 1.0);
    b.pinned(far, FAR, 300);
    assert_eq!(b.m(far), 0.0, "out of range");
    // Just outside the 0.08 m range.
    b.pinned(far, Vec3::new(0.0, 0.0801, 0.0), 300);
    assert_eq!(b.m(far), 0.0, "just out of range");
    b.solver.set_magnet_power(b.magnet, 0.0);
    b.pinned(far, NEAR, 300);
    assert_eq!(b.m(far), 0.0, "in range, coil off");
}

#[test]
fn magnetisation_ignores_magnets_of_other_layers() {
    // `MagnetState::in_range` is the range test: layer bit and 0 < d ≤ range.
    let mut b = bench(&[profile("standard_steel")], WEAK);
    let mut ball = steel_ball(NEAR);
    ball.layer = 3;
    let mut s = solver(&[profile("standard_steel")], Vec3::ZERO);
    let mut m = magnet_at(Vec3::ZERO);
    m.k_coil = WEAK;
    m.layer_mask = pin_physics::world::layers::layer_bit(0);
    b.magnet = s.add_magnet(m);
    b.probe = register(&mut s);
    b.solver = s;
    let h = b.solver.add_ball(ball).unwrap();
    b.solver.set_magnet_power(b.magnet, 1.0);
    b.pinned(h, NEAR, 500);
    assert_eq!(b.m(h), 0.0, "magnetised by a magnet of another layer");
    assert_eq!(b.solver.ball(h).unwrap().layer, 3);
}

/// Magnetises a pinned ball at P = 1 and returns the bench, the handle and M.
fn magnetised(tau: Option<f32>) -> (Bench, BallHandle, f32) {
    let mut b = bench(&[profile("standard_steel")], WEAK);
    if let Some(t) = tau {
        b.solver.set_magnet_remanence_tau(b.magnet, t);
    }
    let h = b.solver.add_ball(steel_ball(NEAR)).unwrap();
    b.solver.set_magnet_power(b.magnet, 1.0);
    b.pinned(h, NEAR, 3000);
    let m0 = b.m(h);
    assert!(m0 > 0.9 && m0 <= 1.0, "M after 3 s at P = 1: {m0}");
    (b, h, m0)
}

#[test]
fn off_field_decay_is_exponential_with_tau() {
    for tau in [None, Some(0.02f32), Some(0.15)] {
        let t = tau.unwrap_or(TAU);
        let (mut b, h, m0) = magnetised(tau);
        // Coil still on: "off-field" here is "outside the range".
        let five_tau = (5.0 * t / DT).ceil() as usize;
        let mut prev = m0;
        for k in 1..=five_tau + 2 {
            b.pinned(h, FAR, 1);
            let m = b.m(h);
            assert!(m <= prev && m >= 0.0, "tick {k}: {prev} → {m}");
            prev = m;
            // M0·e^(−kΔt/τ), allowing the update to be one tick early or late.
            let hi = m0 * libm::expf(-((k - 1) as f32) * DT / t) * 1.01 + 1.0e-6;
            let lo = m0 * libm::expf(-((k + 1) as f32) * DT / t) * 0.99 - 1.0e-6;
            assert!(
                (lo..=hi).contains(&m),
                "τ = {t}, tick {k}: M = {m}, expected within [{lo}, {hi}]"
            );
        }
        assert!(prev < 0.01, "τ = {t}: M = {prev} after 5τ off-field");
    }
}

#[test]
fn decay_below_one_percent_after_five_tau_with_the_coil_off_in_range() {
    // The holding-magnet case: the ball does not move, the coil goes off.
    let (mut b, h, m0) = magnetised(None);
    b.solver.set_magnet_power(b.magnet, 0.0);
    let mut prev = m0;
    for k in 0..302 {
        b.pinned(h, NEAR, 1);
        let m = b.m(h);
        assert!(m <= prev + 1.0e-7 && m >= 0.0, "tick {k}: {prev} → {m}");
        prev = m;
    }
    assert!(prev < 0.01, "M = {prev} 5τ after the coil went off");
    // It lingers: not gone at once.
    let (mut b, h, m0) = magnetised(None);
    b.solver.set_magnet_power(b.magnet, 0.0);
    b.pinned(h, NEAR, 10);
    assert!(b.m(h) > 0.5 * m0, "M collapsed within 10 ms: {}", b.m(h));
}

#[test]
fn zero_tau_is_handled() {
    let (mut b, h, _) = magnetised(Some(0.0));
    b.pinned(h, FAR, 3);
    let m = b.m(h);
    assert!(m.is_finite() && (0.0..=1.0).contains(&m), "M = {m}");
    assert!(m < 0.01, "τ = 0: M = {m} after 3 ms off-field");
    let v = b.solver.ball(h).unwrap();
    assert!(v.position.is_finite() && v.velocity.is_finite());
}

// --------------------------------------------------------------- ball lifetime

#[test]
fn reused_handle_starts_unmagnetised() {
    let (mut b, h, _) = magnetised(None);
    assert!(b.solver.remove_ball(h).is_some());
    // Same slot, in range, coil still on: M is 0 before any step…
    let h2 = b.solver.add_ball(steel_ball(NEAR)).unwrap();
    assert_eq!(h2, h, "the handle is reused");
    assert_eq!(b.m(h2), 0.0, "new ball inherited the old magnetisation");
    // …and with the coil off it stays 0 (nothing to rise toward).
    b.solver.set_magnet_power(b.magnet, 0.0);
    b.pinned(h2, NEAR, 50);
    assert_eq!(b.m(h2), 0.0);

    // Same again with the new ball far away and the coil on.
    let (mut b, h, _) = magnetised(None);
    b.solver.remove_ball(h).unwrap();
    let h2 = b.solver.add_ball(steel_ball(FAR)).unwrap();
    assert_eq!(h2, h);
    assert_eq!(b.m(h2), 0.0);
    b.pinned(h2, FAR, 50);
    assert_eq!(b.m(h2), 0.0);
    assert_eq!(b.solver.ball(h2).unwrap().velocity, Vec3::ZERO);
}

#[test]
fn reused_handle_feels_no_residual_force() {
    // The old ball's magnetisation must not pull the new ball.
    let mut b = bench(&[steel_with(1.0, 1.0)], 2.0e-5);
    let h = b.solver.add_ball(steel_ball(NEAR)).unwrap();
    b.solver.set_magnet_power(b.magnet, 1.0);
    b.pinned(h, NEAR, 2000);
    assert!(b.m(h) > 0.9);
    b.solver.set_magnet_power(b.magnet, 0.0);
    b.solver.remove_ball(h).unwrap();
    let h2 = b.solver.add_ball(steel_ball(NEAR)).unwrap();
    assert_eq!(h2, h);
    let world = empty_world();
    let mut ev = BallEvents::new();
    for _ in 0..20 {
        b.solver.step(&Geometry::new(&world), DT, &mut ev);
    }
    let ball = b.solver.ball(h2).unwrap();
    assert_eq!(ball.velocity, Vec3::ZERO, "new ball was pulled");
    assert_eq!(ball.position, NEAR);
}

#[test]
fn magnetisation_is_per_ball() {
    let mut b = bench(&[profile("standard_steel")], WEAK);
    let near = b.solver.add_ball(steel_ball(NEAR)).unwrap();
    let far = b.solver.add_ball(steel_ball(FAR)).unwrap();
    let third = b
        .solver
        .add_ball(steel_ball(Vec3::new(0.0, 6.0, 0.0)))
        .unwrap();
    b.solver.set_magnet_power(b.magnet, 1.0);
    let world = empty_world();
    let mut ev = BallEvents::new();
    for _ in 0..1500 {
        for (h, at) in [(near, NEAR), (far, FAR)] {
            let ball = b.solver.ball_mut(h).unwrap();
            ball.position = at;
            ball.velocity = Vec3::ZERO;
        }
        b.solver.step(&Geometry::new(&world), DT, &mut ev);
    }
    assert!(b.m(near) > 0.9);
    assert_eq!(b.m(far), 0.0);
    assert_eq!(b.m(third), 0.0);
    // Removing the unmagnetised neighbour does not disturb the other.
    let before = b.m(near);
    b.solver.remove_ball(far).unwrap();
    assert_eq!(b.m(near), before);
}

#[test]
fn hooks_registered_after_the_balls_start_at_zero() {
    let mut s = solver(&[profile("standard_steel")], Vec3::ZERO);
    let mut m = magnet_at(Vec3::ZERO);
    m.k_coil = WEAK;
    let id = s.add_magnet(m);
    s.set_magnet_power(id, 1.0);
    let h0 = s.add_ball(steel_ball(NEAR)).unwrap();
    let h1 = s.add_ball(steel_ball(FAR)).unwrap();
    let world = empty_world();
    let mut ev = BallEvents::new();
    for _ in 0..100 {
        s.step(&Geometry::new(&world), DT, &mut ev);
    }
    let probe = register(&mut s);
    assert_eq!(mag(&probe, h0), 0.0);
    assert_eq!(mag(&probe, h1), 0.0);
    for _ in 0..2000 {
        s.ball_mut(h0).unwrap().position = NEAR;
        s.ball_mut(h0).unwrap().velocity = Vec3::ZERO;
        s.step(&Geometry::new(&world), DT, &mut ev);
    }
    assert!(mag(&probe, h0) > 0.9);
    assert_eq!(mag(&probe, h1), 0.0);
}

// ------------------------------------------------------------ residual force

/// Velocity gained in the first tick after the coil goes off, by a ball of
/// (ρ, c) magnetised at `at` (at rest), with the magnetisation before and
/// after that tick and the magnet's `pull` at `at`.
fn first_tick_after_switch_off(rho: f32, c: f32, at: Vec3) -> (Vec3, f32, f32, Vec3) {
    let mut b = bench(&[steel_with(rho, c)], 2.0e-5);
    let h = b.solver.add_ball(steel_ball(at)).unwrap();
    b.solver.set_magnet_power(b.magnet, 1.0);
    b.pinned(h, at, 2000);
    let m_before = b.m(h);
    b.solver.set_magnet_power(b.magnet, 0.0);
    b.pinned(h, at, 1);
    let m_after = b.m(h);
    let pull = b.solver.magnets()[0].pull(at);
    (b.solver.ball(h).unwrap().velocity, m_before, m_after, pull)
}

#[test]
fn residual_force_matches_the_equation() {
    let mass = profile("standard_steel").mass_kg;
    // In the 1/d² region, at the d_min clamp, and off the axes.
    for at in [
        NEAR,
        Vec3::new(0.0, 0.004, 0.003),
        Vec3::new(0.02, -0.03, 0.04),
    ] {
        for (rho, c) in [
            (0.05f32, 1.0f32),
            (0.2, 1.0),
            (0.5, 1.0),
            (1.0, 1.0),
            (0.5, 0.4),
        ] {
            let (dv, m_before, m_after, pull) = first_tick_after_switch_off(rho, c, at);
            assert!(m_before > 0.9, "M = {m_before}");
            assert!(pull.length() > 0.0);
            // F_res = c·ρ·M·pull, M somewhere between its value before and
            // after the tick.
            let per_m = c * rho * pull.length() * DT / mass;
            let speed = dv.length();
            let (lo, hi) = (per_m * m_after * 0.98, per_m * m_before * 1.02);
            assert!(
                speed >= lo && speed <= hi,
                "ρ={rho} c={c} at {at:?}: Δv = {speed}, expected within [{lo}, {hi}] \
                 (M {m_before} → {m_after})"
            );
            // Toward the pole.
            let cos = dv.normalize().dot(pull.normalize());
            assert!(cos > 0.9999, "direction: cos = {cos}");
        }
    }
}

#[test]
fn residual_force_is_exactly_zero_for_zero_remanence_or_zero_coupling() {
    for (rho, c) in [(0.0f32, 1.0f32), (0.7, 0.0), (0.0, 0.0)] {
        let (dv, _, _, pull) = first_tick_after_switch_off(rho, c, NEAR);
        assert!(pull.length() > 0.0);
        assert_eq!(dv, Vec3::ZERO, "ρ={rho} c={c}");
    }
}

#[test]
fn residual_force_is_zero_outside_the_range() {
    let mut b = bench(&[steel_with(1.0, 1.0)], 2.0e-5);
    let h = b.solver.add_ball(steel_ball(NEAR)).unwrap();
    b.solver.set_magnet_power(b.magnet, 1.0);
    b.pinned(h, NEAR, 2000);
    b.solver.set_magnet_power(b.magnet, 0.0);
    // Magnetised, but beyond the magnet's reach.
    let out = Vec3::new(0.0, 0.0, 0.0805);
    b.pinned(h, out, 1);
    assert!(b.m(h) > 0.8);
    assert_eq!(b.solver.ball(h).unwrap().velocity, Vec3::ZERO);
}

#[test]
fn residual_pull_decays_while_the_ball_is_away_and_comes_back_weaker() {
    // Magnetise, park out of range for one τ, come back with the coil off:
    // the pull is e^(−1) of what it would have been.
    let mass = profile("standard_steel").mass_kg;
    let mut b = bench(&[steel_with(1.0, 1.0)], 2.0e-5);
    let h = b.solver.add_ball(steel_ball(NEAR)).unwrap();
    b.solver.set_magnet_power(b.magnet, 1.0);
    b.pinned(h, NEAR, 2000);
    let m0 = b.m(h);
    b.solver.set_magnet_power(b.magnet, 0.0);
    b.pinned(h, FAR, 60);
    let m1 = b.m(h);
    assert!((m1 / m0 - libm::expf(-1.0)).abs() < 0.02, "{m0} → {m1}");
    b.pinned(h, NEAR, 1);
    let dv = b.solver.ball(h).unwrap().velocity.length();
    let pull = b.solver.magnets()[0].pull(NEAR).length();
    let expect = m1 * pull * DT / mass;
    assert!(
        dv <= expect * 1.02 && dv >= expect * 0.95,
        "Δv {dv} vs {expect}"
    );
}

// ------------------------------------------------------------------ proptests

#[derive(Clone, Debug)]
struct Phase {
    power0: f32,
    power1: f32,
    place: u8,
    ticks: usize,
}

fn phase() -> impl Strategy<Value = Phase> {
    (0.0f32..=1.0, 0.0f32..=1.0, 0u8..5, 1usize..60).prop_map(|(power0, power1, place, ticks)| {
        Phase {
            power0,
            power1,
            place,
            ticks,
        }
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, .. ProptestConfig::default() })]

    /// M stays in [0, 1], finite, and never exceeds the largest power
    /// commanded so far, for any schedule of powers, positions, τ and two
    /// overlapping magnets; the ball state stays finite.
    #[test]
    fn prop_magnetisation_stays_in_unit_interval(
        phases in prop::collection::vec(phase(), 1..12),
        tau0 in prop_oneof![Just(0.0f32), 1.0e-4f32..0.3],
        tau1 in prop_oneof![Just(0.0f32), 1.0e-4f32..0.3],
        rho in 0.0f32..=1.0,
        free in any::<bool>(),
    ) {
        let mut s = solver(&[steel_with(rho, 1.0)], Vec3::ZERO);
        let mut m = magnet_at(Vec3::ZERO);
        m.k_coil = if free { 2.0e-5 } else { WEAK };
        let a = s.add_magnet(m);
        m.position = Vec3::new(0.05, 0.0, 0.0);
        let b = s.add_magnet(m);
        s.set_magnet_remanence_tau(a, tau0);
        s.set_magnet_remanence_tau(b, tau1);
        let probe = register(&mut s);
        let h = s.add_ball(steel_ball(NEAR)).unwrap();
        let world = empty_world();
        let geo = Geometry::new(&world);
        let mut ev = BallEvents::new();
        let mut max_power = 0.0f32;
        for ph in &phases {
            s.set_magnet_power(a, ph.power0);
            s.set_magnet_power(b, ph.power1);
            max_power = max_power.max(ph.power0).max(ph.power1);
            let at = match ph.place {
                0 => NEAR,                          // both magnets
                1 => Vec3::new(-0.06, 0.0, 0.0),    // first only
                2 => Vec3::new(0.11, 0.0, 0.0),     // second only
                3 => Vec3::new(0.0, 0.0799, 0.0),   // edge of the first
                _ => FAR,
            };
            for k in 0..ph.ticks {
                if !free || k == 0 {
                    let ball = s.ball_mut(h).unwrap();
                    ball.position = at;
                    ball.velocity = Vec3::ZERO;
                }
                ev.clear();
                s.step(&geo, DT, &mut ev);
                let mm = mag(&probe, h);
                prop_assert!(mm.is_finite() && (0.0..=1.0).contains(&mm), "M = {}", mm);
                prop_assert!(mm <= max_power + 1.0e-6, "M = {} above every P ({})", mm, max_power);
                let ball = s.ball(h).unwrap();
                prop_assert!(ball.position.is_finite() && ball.velocity.is_finite());
            }
        }
    }

    /// Off-field, M after k ticks is M0·e^(−k·Δt/τ) for any τ and start level.
    #[test]
    fn prop_off_field_decay(
        tau in 0.005f32..0.3,
        power in 0.2f32..=1.0,
        k in 1usize..200,
    ) {
        let mut b = bench(&[profile("standard_steel")], WEAK);
        b.solver.set_magnet_remanence_tau(b.magnet, tau);
        let h = b.solver.add_ball(steel_ball(NEAR)).unwrap();
        b.solver.set_magnet_power(b.magnet, power);
        b.pinned(h, NEAR, 400);
        let m0 = b.m(h);
        prop_assume!(m0 > 0.01);
        b.pinned(h, FAR, k);
        let m = b.m(h);
        let hi = m0 * libm::expf(-((k - 1) as f32) * DT / tau) * 1.01 + 1.0e-6;
        let lo = m0 * libm::expf(-((k + 1) as f32) * DT / tau) * 0.99 - 1.0e-6;
        prop_assert!((lo..=hi).contains(&m), "τ={} k={}: {} → {} not in [{}, {}]", tau, k, m0, m, lo, hi);
    }

    /// The residual force is linear in ρ and c and follows `pull`.
    #[test]
    fn prop_residual_force(
        rho in 0.01f32..=1.0,
        c in 0.05f32..=1.0,
        at in prop::array::uniform3(-0.04f32..0.04),
    ) {
        let at = Vec3::from(at);
        prop_assume!(at.length() > 0.002);
        let mass = profile("standard_steel").mass_kg;
        let (dv, m_before, m_after, pull) = first_tick_after_switch_off(rho, c, at);
        prop_assume!(m_before > 0.5);
        let per_m = c * rho * pull.length() * DT / mass;
        let speed = dv.length();
        prop_assert!(
            speed >= per_m * m_after * 0.98 && speed <= per_m * m_before * 1.02,
            "ρ={} c={} at {:?}: Δv {} vs [{}, {}]", rho, c, at, speed, per_m * m_after, per_m * m_before
        );
        prop_assert!(dv.normalize().dot(pull.normalize()) > 0.999);
    }
}

#[test]
fn magnetisation_of_an_empty_or_unknown_slot_is_zero() {
    let (mut b, h, _) = magnetised(None);
    // Slots that never held a ball, and a handle beyond the arena.
    for slot in [1u8, 7, pin_physics::ball::MAX_BALLS as u8, 200, 255] {
        assert_eq!(b.m(BallHandle(slot)), 0.0, "slot {slot}");
    }
    // A removed ball takes its magnetisation with it.
    b.solver.remove_ball(h).unwrap();
    assert_eq!(b.m(h), 0.0);
}
