//! Subphase 2.7 implementer tests: Magnus effect and magnetic remanence
//! through the 2.2-S force hooks (PLAN.md §3.2, verification 1–3).

#[path = "ball_util.rs"]
mod util;

use glam::{Quat, Vec3};
use pin_core::BallProfile;
use pin_core::ids::ProfileId;
use pin_physics::ball::{
    Ball, BallEvents, BallHandle, BallSolver, ExternalForce, ForceContext, Geometry, MAX_BALLS,
    Magnet, MagnetState,
};
use pin_physics::ball_ext::{self, Magnus, Remanence, magnetisation, register_all};
use pin_physics::world::{StaticShape, StaticWorldBuilder, layers};
use util::*;

const LOW_REM: ProfileId = ProfileId(1);

fn bits(b: &Ball) -> [u32; 13] {
    let mut out = [0u32; 13];
    let words = b
        .position
        .to_array()
        .into_iter()
        .chain(b.velocity.to_array())
        .chain(b.angular_velocity.to_array())
        .chain(b.orientation.to_array());
    for (o, w) in out.iter_mut().zip(words) {
        *o = w.to_bits();
    }
    out
}

fn all_bits(s: &BallSolver) -> Vec<(u8, [u32; 13])> {
    s.balls().map(|(h, b)| (h.0, bits(b))).collect()
}

fn magnet_at(position: Vec3) -> Magnet {
    Magnet {
        position,
        k_coil: 2.0e-5,
        d_min: 0.01,
        range: 0.08,
        layer_mask: layers::ALL,
    }
}

// ---------------------------------------------------------------- Magnus

/// A spinning Glo-Ball in free flight (no gravity): the lateral
/// acceleration is `S_m |ω × v| / m` within 1 % and the speed is kept.
#[test]
fn magnus_lateral_acceleration_and_no_work() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let p = profiles()[GLOBALL.index()].clone();
    assert!(p.magnus_coeff > 0.0);
    for (omega, vel) in [
        (v(0.0, 0.0, 200.0), v(3.0, 0.0, 0.0)),
        (v(0.0, 0.0, -150.0), v(1.0, 2.0, 0.0)),
        (v(30.0, -80.0, 60.0), v(-2.0, 0.5, 4.0)),
        (v(0.0, 900.0, 0.0), v(10.0, 0.0, -1.0)),
    ] {
        let mut s = solver_no_gravity();
        register_all(&mut s);
        let mut b = ball_at(GLOBALL, Vec3::ZERO, vel);
        b.angular_velocity = omega;
        let h = s.add_ball(b).unwrap();
        let mut ev = BallEvents::new();
        let speed0 = vel.length();
        for _ in 0..200 {
            let before = *s.ball(h).unwrap();
            s.step(&geo, DT, &mut ev);
            let after = *s.ball(h).unwrap();
            let acc = (after.velocity - before.velocity) / DT;
            let expect =
                p.magnus_coeff * before.angular_velocity.cross(before.velocity) / p.mass_kg;
            assert!(
                (acc - expect).length() <= 0.01 * expect.length(),
                "{acc} vs {expect}"
            );
            // Perpendicular to v: no work.
            assert!(acc.dot(before.velocity).abs() <= 0.01 * expect.length() * speed0);
        }
        let speed = s.ball(h).unwrap().velocity.length();
        // 200 ticks of f32 rounding: a few ulp per tick at most.
        assert!(
            (speed - speed0).abs() <= 200.0 * 4.0 * f32::EPSILON * speed0,
            "{speed} vs {speed0}"
        );
        // The ball really turned.
        let turned = s.ball(h).unwrap().velocity.angle_between(vel);
        assert!(turned > 1.0e-3);
    }
}

/// With gravity and a vertical spin axis the Magnus force is horizontal,
/// so the translational kinetic energy equals that of the run without
/// Magnus up to rounding (bound: 1e-6 relative over 0.5 s).
#[test]
fn magnus_kinetic_energy_matches_the_run_without_it() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let run = |hooks: bool| {
        let mut s = solver();
        if hooks {
            register_all(&mut s);
        }
        let mut b = ball_at(GLOBALL, v(0.0, 0.0, 1.0), v(4.0, 1.0, 2.0));
        b.angular_velocity = v(0.0, 0.0, 400.0);
        let h = s.add_ball(b).unwrap();
        let mut ev = BallEvents::new();
        for _ in 0..500 {
            s.step(&geo, DT, &mut ev);
        }
        *s.ball(h).unwrap()
    };
    let with = run(true);
    let without = run(false);
    let m = f64::from(profiles()[GLOBALL.index()].mass_kg);
    let ke = |b: &Ball| 0.5 * m * b.velocity.as_dvec3().length_squared();
    let (a, b) = (ke(&with), ke(&without));
    assert!((a - b).abs() <= 1.0e-6 * b, "{a} vs {b}");
    // But the path is bent.
    assert!(with.position.distance(without.position) > 1.0e-3);
    assert_eq!(with.velocity.z.to_bits(), without.velocity.z.to_bits());
}

#[test]
fn magnus_helpers() {
    let f = ball_ext::magnus_force(1.0e-5, v(0.0, 0.0, 100.0), v(2.0, 0.0, 0.0));
    assert!((f - v(0.0, 2.0e-3, 0.0)).length() < 1e-9);
    let g = ball_ext::magnus_step_force(1.0e-5, 0.024, v(0.0, 0.0, 100.0), v(2.0, 0.0, 0.0), DT);
    assert!((g - f).length() < 1e-3 * f.length());
    // No force, as -0.0 (the identity of addition).
    for z in [
        ball_ext::magnus_step_force(0.0, 0.024, v(1.0, 2.0, 3.0), v(1.0, 0.0, 0.0), DT),
        ball_ext::magnus_step_force(1.0e-5, 0.024, Vec3::ZERO, v(1.0, 0.0, 0.0), DT),
        ball_ext::magnus_step_force(1.0e-5, 0.0, v(1.0, 2.0, 3.0), v(1.0, 0.0, 0.0), DT),
        ball_ext::magnus_step_force(1.0e-5, 0.024, v(1.0, 2.0, 3.0), v(1.0, 0.0, 0.0), 0.0),
        ball_ext::magnus_step_force(f32::NAN, 0.024, v(1.0, 2.0, 3.0), v(1.0, 0.0, 0.0), DT),
        ball_ext::magnus_step_force(
            1.0e-5,
            0.024,
            v(f32::INFINITY, 0.0, 0.0),
            v(1.0, 0.0, 0.0),
            DT,
        ),
        ball_ext::magnus_step_force(1.0e-5, 0.024, v(1.0, 0.0, 0.0), v(f32::NAN, 0.0, 0.0), DT),
    ] {
        assert_eq!(z, Vec3::ZERO);
        assert!(z.to_array().iter().all(|c| c.is_sign_negative()));
    }
    // v parallel to ω: no force.
    let par = ball_ext::magnus_step_force(1.0e-5, 0.024, v(0.0, 0.0, 50.0), v(0.0, 0.0, 3.0), DT);
    assert_eq!(par, Vec3::ZERO);
    // The hook rotates the sub-step midpoint velocity v − g·h/2.
    let g = v(0.0, 0.0, -9.81);
    let mut m = Magnus::new();
    assert_eq!(ball_ext::DEFAULT_GRAVITY, g);
    assert_eq!(Magnus, m);
    let mut mg = ball_ext::MagnusWithGravity::default();
    assert_eq!(mg.gravity, g);
    let mut b = Ball::new(GLOBALL, Vec3::ZERO);
    b.velocity = v(2.0, 0.0, 0.0);
    b.angular_velocity = v(0.0, 0.0, 100.0);
    let p = &profiles()[GLOBALL.index()];
    let expect = ball_ext::magnus_step_force(
        p.magnus_coeff,
        p.mass_kg,
        b.angular_velocity,
        b.velocity - g * (0.5 * DT),
        DT,
    );
    assert_eq!(m.force(BallHandle(0), &b, p, DT), expect);
    assert_eq!(mg.force(BallHandle(0), &b, p, DT), expect);
    // No gravity: the velocity itself.
    let mut flat = Magnus::with_gravity(Vec3::ZERO);
    assert_eq!(
        flat.force(BallHandle(0), &b, p, DT),
        ball_ext::magnus_step_force(
            p.magnus_coeff,
            p.mass_kg,
            b.angular_velocity,
            b.velocity,
            DT
        )
    );
    // Zero coefficient: exactly no force (as -0.0) even with gravity.
    let steel = &profiles()[STEEL.index()];
    let z = m.force(BallHandle(0), &b, steel, DT);
    assert!(z == Vec3::ZERO && z.to_array().iter().all(|c| c.is_sign_negative()));
}

// ------------------------------------------------------------ Remanence

/// A ball held on a magnet for 300 ms, then the coil is switched off and
/// the ball pushed away at `push` m/s. Returns the tick at which the
/// ball leaves the magnet's range (or `None`), its final state, and `M`
/// right after the switch-off.
fn release(profile: ProfileId, hooks: bool, push: f32) -> (Option<u32>, Ball, f32) {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut s = solver();
    let pole = v(0.0, 0.0, -0.005);
    let id = s.add_magnet(magnet_at(pole));
    s.set_magnet_power(id, 1.0);
    let rem = hooks.then(|| register_all(&mut s));
    let r = radius(profile);
    let h = s
        .add_ball(ball_at(profile, v(0.02, 0.0, r), Vec3::ZERO))
        .unwrap();
    let mut ev = BallEvents::new();
    for _ in 0..300 {
        s.step(&geo, DT, &mut ev);
    }
    let m_on = rem.as_ref().map_or(0.0, |r| magnetisation(r, h));
    s.set_magnet_power(id, 0.0);
    let mass = profiles()[profile.index()].mass_kg;
    s.apply_impulse(h, v(push * mass, 0.0, 0.0));
    let mut left = None;
    for tick in 0..1000 {
        s.step(&geo, DT, &mut ev);
        if left.is_none() && s.ball(h).unwrap().position.distance(pole) > 0.08 {
            left = Some(tick);
        }
    }
    (left, *s.ball(h).unwrap(), m_on)
}

#[test]
fn standard_steel_lingers_low_rem_releases_cleanly() {
    let push = 0.5;
    let (base_left, base, _) = release(LOW_REM, false, push);
    let (low_left, low, low_m) = release(LOW_REM, true, push);
    let (std_left, _, std_m) = release(STEEL, true, push);
    // low_rem_steel with the hooks is bit-identical to no hooks at all.
    assert_eq!(bits(&low), bits(&base));
    assert_eq!(low_left, base_left);
    // Both were magnetised by the held coil.
    assert!(low_m > 0.99 && std_m > 0.99, "{low_m} {std_m}");
    let (l, s) = (low_left.unwrap(), std_left.unwrap());
    // Measurably later: at least 20 ms.
    assert!(
        s > l + 20,
        "standard_steel left at tick {s}, low_rem_steel at {l}"
    );
    eprintln!("left range: low_rem tick {l}, standard tick {s}");
}

#[test]
fn magnetisation_rises_saturates_and_decays() {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut s = solver();
    let tau = 0.030;
    let id = s.add_magnet(magnet_at(v(0.0, 0.0, -0.005)));
    s.set_magnet_remanence_tau(id, tau);
    s.set_magnet_power(id, 0.6);
    let rem = register_all(&mut s);
    let r = radius(STEEL);
    let held = s
        .add_ball(ball_at(STEEL, v(0.0, 0.0, r), Vec3::ZERO))
        .unwrap();
    let far = s
        .add_ball(ball_at(STEEL, v(1.0, 0.0, r), Vec3::ZERO))
        .unwrap();
    let ceramic = s
        .add_ball(ball_at(POWERBALL, v(0.03, 0.03, r), Vec3::ZERO))
        .unwrap();
    let mut ev = BallEvents::new();
    let mut last = 0.0;
    for _ in 0..200 {
        s.step(&geo, DT, &mut ev);
        let m = magnetisation(&rem, held);
        assert!((0.0..=1.0).contains(&m));
        assert!(m >= last); // monotone rise toward P
        assert!(m <= 0.6 + 1e-6);
        last = m;
        assert_eq!(magnetisation(&rem, far), 0.0);
        assert_eq!(magnetisation(&rem, ceramic), 0.0); // c = 0
    }
    // First-order rise: 1 − e^(−0.2/0.03) of the way.
    assert!(
        (last - 0.6 * (1.0 - (-0.2f32 / tau).exp())).abs() < 1e-3,
        "{last}"
    );
    // Raise P to 1: M follows above 0.6.
    s.set_magnet_power(id, 1.0);
    for _ in 0..300 {
        s.step(&geo, DT, &mut ev);
    }
    let m_full = magnetisation(&rem, held);
    assert!(m_full > 0.99 && m_full <= 1.0);
    // Coil off: M decays below 1 % within 5τ (ball still on the pole: in
    // range of a magnet at P = 0, same law).
    s.set_magnet_power(id, 0.0);
    let ticks = (5.0 * tau / DT).round() as u32;
    for _ in 0..ticks {
        s.step(&geo, DT, &mut ev);
    }
    let m5 = magnetisation(&rem, held);
    assert!(m5 < 0.01 * m_full, "{m5}");
    assert!((m5 - m_full * (-5.0f32).exp()).abs() < 1e-4);
    // Unknown handle: zero.
    assert_eq!(magnetisation(&rem, BallHandle(MAX_BALLS as u8)), 0.0);
    assert_eq!(rem.magnetisation(BallHandle(200)), 0.0);
}

/// Out of every magnet's range, `M` decays with the τ of the magnet that
/// last held the ball (read live, so a changed τ applies).
#[test]
fn magnetisation_decays_out_of_range_with_the_source_tau() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let a = s.add_magnet(magnet_at(v(0.0, 0.0, 0.0)));
    let b = s.add_magnet(magnet_at(v(0.0, 0.5, 0.0)));
    s.set_magnet_power(a, 1.0);
    s.set_magnet_remanence_tau(a, 0.010);
    s.set_magnet_remanence_tau(b, 0.5);
    let rem = register_all(&mut s);
    // A low_rem ball (no residual pull) near the edge of magnet a's range,
    // where the coil barely moves it while it magnetises.
    let h = s
        .add_ball(ball_at(LOW_REM, v(0.075, 0.0, 0.0), Vec3::ZERO))
        .unwrap();
    let mut ev = BallEvents::new();
    let mut ticks = 0;
    while magnetisation(&rem, h) < 0.5 {
        s.step(&geo, DT, &mut ev);
        ticks += 1;
        assert!(ticks < 100);
    }
    // Push it fast out of range (removing and re-adding would reset M).
    let m = profiles()[LOW_REM.index()].mass_kg;
    s.set_magnet_power(a, 0.0);
    s.apply_impulse(h, v(50.0 * m, 0.0, 0.0));
    s.step(&geo, DT, &mut ev);
    s.step(&geo, DT, &mut ev);
    assert!(!s.magnets()[0].in_range(s.ball(h).unwrap().position, 0));
    s.set_magnet_remanence_tau(a, 0.020);
    let m0 = magnetisation(&rem, h);
    for _ in 0..20 {
        s.step(&geo, DT, &mut ev);
    }
    let m1 = magnetisation(&rem, h);
    assert!((m1 / m0 - (-1.0f32).exp()).abs() < 1e-3, "{m0} → {m1}");
}

#[test]
fn handle_reuse_starts_with_zero_magnetisation() {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut s = solver();
    let id = s.add_magnet(magnet_at(v(0.0, 0.0, -0.005)));
    s.set_magnet_power(id, 1.0);
    let r = radius(STEEL);
    // Registered after the ball exists: told about it (M = 0).
    let h = s
        .add_ball(ball_at(STEEL, v(0.0, 0.0, r), Vec3::ZERO))
        .unwrap();
    let rem = register_all(&mut s);
    assert_eq!(magnetisation(&rem, h), 0.0);
    let mut ev = BallEvents::new();
    for _ in 0..200 {
        s.step(&geo, DT, &mut ev);
    }
    assert!(magnetisation(&rem, h) > 0.9);
    s.remove_ball(h);
    assert_eq!(magnetisation(&rem, h), 0.0);
    // The new ball, far from the magnet, in the same slot.
    let h2 = s
        .add_ball(ball_at(STEEL, v(2.0, 0.0, r), Vec3::ZERO))
        .unwrap();
    assert_eq!(h2, h);
    assert_eq!(magnetisation(&rem, h2), 0.0);
    s.step(&geo, DT, &mut ev);
    assert_eq!(magnetisation(&rem, h2), 0.0);
}

#[test]
fn remanence_helpers() {
    use ball_ext::{magnetisation_step, remanence_decay, residual_force};
    assert_eq!(remanence_decay(0.001, 0.0), 0.0);
    assert_eq!(remanence_decay(0.001, f32::INFINITY), 1.0);
    assert_eq!(remanence_decay(0.001, f32::NAN), 0.0);
    assert!((remanence_decay(0.001, 0.06) - (-1.0f32 / 60.0).exp()).abs() < 1e-7);
    assert_eq!(magnetisation_step(0.5, 1.0, 0.001, 0.0), 1.0);
    assert_eq!(magnetisation_step(f32::NAN, 1.0, 0.001, 0.06), 0.0);
    assert_eq!(magnetisation_step(0.5, f32::NAN, 0.001, 0.0), 0.0);
    assert_eq!(magnetisation_step(2.0, 5.0, 0.001, 0.0), 1.0);
    let ms = MagnetState {
        magnet: Magnet {
            layer_mask: layers::layer_bit(2),
            ..magnet_at(Vec3::ZERO)
        },
        power: 0.0,
        remanence_tau_s: 0.06,
    };
    let p = v(0.04, 0.0, 0.0);
    let f = residual_force(&ms, p, 2, 1.0, 0.05, 0.5);
    assert!((f - ms.pull(p) * 0.025).length() < 1e-9);
    assert!(f.x < 0.0); // toward the pole
    assert_eq!(residual_force(&ms, p, 1, 1.0, 0.05, 0.5), Vec3::ZERO); // layer
    assert_eq!(
        residual_force(&ms, v(0.2, 0.0, 0.0), 2, 1.0, 0.05, 0.5),
        Vec3::ZERO
    );
    assert_eq!(residual_force(&ms, p, 2, 1.0, 0.0, 0.5), Vec3::ZERO);
    assert_eq!(residual_force(&ms, p, 2, 0.0, 0.05, 0.5), Vec3::ZERO);
    let d = Remanence::default();
    assert_eq!(d.magnetisation(BallHandle(0)), 0.0);
}

/// The residual pull acts with the coil off; the hook's own force is the
/// whole difference.
#[test]
fn residual_force_through_the_hook() {
    let mut rem = Remanence::new();
    let ms = [MagnetState {
        magnet: magnet_at(Vec3::ZERO),
        power: 1.0,
        remanence_tau_s: 0.0, // instant: M = P after one sub-step
    }];
    let ctx = |magnets| ForceContext {
        magnets,
        step_index: 0,
        tick_dt: DT,
        tick_time: 0.0,
        sim_time: 0.0,
        substep: 0,
        substeps: 1,
    };
    let p = profiles()[STEEL.index()].clone();
    let b = Ball::new(STEEL, v(0.03, 0.0, 0.0));
    rem.ball_added(BallHandle(3), &b);
    // M = 0 at the start of the first sub-step: no force yet.
    assert_eq!(
        rem.force_in(&ctx(&ms), BallHandle(3), &b, &p, DT),
        Vec3::ZERO
    );
    assert_eq!(rem.magnetisation(BallHandle(3)), 1.0);
    let off = [MagnetState {
        power: 0.0,
        remanence_tau_s: 0.06,
        ..ms[0]
    }];
    let f = rem.force_in(&ctx(&off), BallHandle(3), &b, &p, DT);
    let expect = ms[0].pull(b.position) * (p.magnetic_coupling * p.remanence);
    assert!((f - expect).length() <= 1e-6 * expect.length());
    // Removal resets; an out-of-range handle is ignored.
    rem.ball_removed(BallHandle(3));
    assert_eq!(rem.magnetisation(BallHandle(3)), 0.0);
    let big = BallHandle(MAX_BALLS as u8);
    rem.ball_added(big, &b);
    assert_eq!(rem.force_in(&ctx(&ms), big, &b, &p, DT), Vec3::ZERO);
}

// ------------------------------------------------- Bit-identity (verif. 1)

fn zeroed_profiles() -> Vec<BallProfile> {
    profiles()
        .into_iter()
        .map(|mut p| {
            p.magnus_coeff = 0.0;
            p.remanence = 0.0;
            p
        })
        .collect()
}

/// Busy scenarios of the 2.2 test suite (spinning balls of every profile
/// in a box, a magnet over a floor with a ball removed and added, a
/// pitched slope): with `magnus_coeff = ρ = 0` everywhere, registering
/// both hooks changes no bit; with the real profiles it does.
#[test]
fn zero_coefficients_are_bit_identical_with_hooks_registered() {
    for scenario in 0..3 {
        let base = run_scenario(scenario, &zeroed_profiles(), false);
        let hooked = run_scenario(scenario, &zeroed_profiles(), true);
        assert_eq!(base, hooked, "scenario {scenario}");
    }
    // Sanity: the real Glo-Ball / standard steel coefficients do act.
    assert_ne!(
        run_scenario(0, &profiles(), false),
        run_scenario(0, &profiles(), true)
    );
    assert_ne!(
        run_scenario(1, &profiles(), false),
        run_scenario(1, &profiles(), true)
    );
}

fn run_scenario(which: u32, profiles: &[BallProfile], hooks: bool) -> Vec<Vec<(u8, [u32; 13])>> {
    let mut s = BallSolver::new(config(), profiles);
    let mut ev = BallEvents::new();
    let mut trace = Vec::new();
    match which {
        0 => {
            let world = box_world(v(0.15, 0.12, 0.1));
            let geo = Geometry::new(&world);
            if hooks {
                register_all(&mut s);
            }
            for k in 0..8u16 {
                let p = ProfileId(k % 6);
                let mut b = ball_at(
                    p,
                    v(
                        -0.1 + 0.028 * f32::from(k),
                        0.05 * libm::sinf(f32::from(k)),
                        -0.05 + 0.012 * f32::from(k),
                    ),
                    v(
                        1.5 - 0.4 * f32::from(k),
                        0.7 * libm::cosf(f32::from(k)),
                        0.3,
                    ),
                );
                b.angular_velocity = v(5.0 * f32::from(k), -12.0, 3.0);
                s.add_ball(b).unwrap();
            }
            for tick in 0..2000 {
                if tick == 1000 {
                    s.apply_impulse(BallHandle(2), v(0.05, 0.08, 0.04));
                }
                s.step(&geo, DT, &mut ev);
                trace.push(all_bits(&s));
            }
        }
        1 => {
            let world = floor_world();
            let geo = Geometry::new(&world);
            let m = s.add_magnet(Magnet {
                range: 0.1,
                ..magnet_at(v(0.1, 0.05, -0.005))
            });
            s.set_magnet_power(m, 0.7);
            for (k, p) in [STEEL, POWERBALL, GLOBALL, MINI].into_iter().enumerate() {
                let r = radius(p);
                let mut b = ball_at(
                    p,
                    v(-0.1 + 0.06 * k as f32, 0.02 * k as f32, r + 0.002),
                    v(0.3, -0.2 + 0.1 * k as f32, 0.0),
                );
                b.angular_velocity = v(0.0, 40.0 - 20.0 * k as f32, 5.0);
                s.add_ball(b).unwrap();
            }
            if hooks {
                register_all(&mut s); // late: told about the balls
            }
            for tick in 0..2000 {
                if tick == 600 {
                    s.set_magnet_power(m, 0.0);
                }
                if tick == 1000 {
                    s.remove_ball(BallHandle(1));
                }
                if tick == 1200 {
                    let r = radius(JUMBO);
                    s.add_ball(ball_at(JUMBO, v(0.0, -0.1, r + 0.03), v(0.0, 0.5, 0.0)))
                        .unwrap();
                }
                s.step(&geo, DT, &mut ev);
                trace.push(all_bits(&s));
            }
        }
        _ => {
            let rot = Quat::from_rotation_y(0.3) * Quat::from_rotation_x(0.12);
            let mut b = StaticWorldBuilder::new();
            b.add_collider(
                StaticShape::Cuboid {
                    center: rot * v(0.0, 0.0, -0.02),
                    half_extents: v(0.4, 0.4, 0.02),
                    rotation: rot,
                },
                layers::ALL,
                true,
            );
            let world = b.build();
            let geo = Geometry::new(&world);
            if hooks {
                register_all(&mut s);
            }
            for k in 0..3 {
                let p = [STEEL, GLOBALL, MINI][k];
                let r = radius(p);
                let mut ball = ball_at(
                    p,
                    rot * v(-0.2 + 0.15 * k as f32, 0.1, r + 0.05),
                    v(0.1, -0.5, 0.0),
                );
                ball.angular_velocity = v(30.0, -10.0, 80.0);
                s.add_ball(ball).unwrap();
            }
            for _ in 0..2000 {
                s.step(&geo, DT, &mut ev);
                trace.push(all_bits(&s));
            }
        }
    }
    trace
}

// ------------------------------------------------------- Determinism

#[test]
fn ten_runs_are_bit_identical() {
    let first = run_scenario(1, &profiles(), true);
    let first_box = run_scenario(0, &profiles(), true);
    for _ in 0..9 {
        assert_eq!(run_scenario(1, &profiles(), true), first);
        assert_eq!(run_scenario(0, &profiles(), true), first_box);
    }
}

// ------------------------------------------------- Review polish (N1, N3)

/// Overlapping magnets: an idle one (τ 20 ms, never powered) and a holder
/// (τ 200 ms, P = 1 for 2 s, then off). After switch-off M decays with the
/// holder's τ whatever the magnets' index order.
#[test]
fn overlapping_idle_magnet_does_not_take_over_the_decay_tau() {
    let run = |idle_first: bool| {
        let world = floor_world();
        let geo = Geometry::new(&world);
        let mut s = solver();
        let pole = v(0.0, 0.0, -0.005);
        let idle_at = v(0.03, 0.0, -0.005);
        let (idle, holder) = if idle_first {
            let i = s.add_magnet(magnet_at(idle_at));
            (i, s.add_magnet(magnet_at(pole)))
        } else {
            let hd = s.add_magnet(magnet_at(pole));
            (s.add_magnet(magnet_at(idle_at)), hd)
        };
        s.set_magnet_remanence_tau(idle, 0.020);
        s.set_magnet_remanence_tau(holder, 0.200);
        s.set_magnet_power(holder, 1.0);
        let rem = register_all(&mut s);
        let h = s
            .add_ball(ball_at(LOW_REM, v(0.0, 0.0, radius(LOW_REM)), Vec3::ZERO))
            .unwrap();
        let mut ev = BallEvents::new();
        for _ in 0..2000 {
            s.step(&geo, DT, &mut ev);
        }
        s.set_magnet_power(holder, 0.0);
        for _ in 0..100 {
            s.step(&geo, DT, &mut ev);
        }
        let b = *s.ball(h).unwrap();
        assert!(s.magnets()[idle.index()].in_range(b.position, b.layer));
        magnetisation(&rem, h)
    };
    let a = run(true);
    let b = run(false);
    assert_eq!(a.to_bits(), b.to_bits(), "{a} vs {b}");
    assert!((a - (-0.5f32).exp()).abs() < 2e-3, "{a}");
}

/// Two clones of one `Remanence` registered: only the first to be called
/// advances M; the second is inert (no double decay, no extra force).
#[test]
fn a_second_registered_clone_is_inert() {
    let run = |twice: bool| {
        let world = floor_world();
        let geo = Geometry::new(&world);
        let mut s = solver();
        let id = s.add_magnet(magnet_at(v(0.0, 0.0, -0.005)));
        s.set_magnet_power(id, 1.0);
        let rem = Remanence::new();
        s.add_external_force(Box::new(rem.clone()));
        if twice {
            s.add_external_force(Box::new(rem.clone()));
        }
        let h = s
            .add_ball(ball_at(STEEL, v(0.0, 0.0, radius(STEEL)), Vec3::ZERO))
            .unwrap();
        let mut ev = BallEvents::new();
        let mut trace = Vec::new();
        for _ in 0..60 {
            s.step(&geo, DT, &mut ev);
            trace.push((magnetisation(&rem, h).to_bits(), bits(s.ball(h).unwrap())));
        }
        trace
    };
    let once = run(false);
    let twice = run(true);
    assert_eq!(once, twice);
    let m60 = f32::from_bits(once.last().unwrap().0);
    assert!((m60 - (1.0 - (-1.0f32).exp())).abs() < 2e-3, "{m60}");
    // A clone registered with a second solver is inert too.
    let rem = Remanence::new();
    let mut s1 = solver();
    let mut s2 = solver();
    s1.add_external_force(Box::new(rem.clone()));
    s2.add_external_force(Box::new(rem.clone()));
    let world = floor_world();
    let geo = Geometry::new(&world);
    let id = s2.add_magnet(magnet_at(v(0.0, 0.0, -0.005)));
    s2.set_magnet_power(id, 1.0);
    s1.add_ball(ball_at(STEEL, v(1.0, 0.0, radius(STEEL)), Vec3::ZERO))
        .unwrap();
    let h2 = s2
        .add_ball(ball_at(STEEL, v(0.0, 0.0, radius(STEEL)), Vec3::ZERO))
        .unwrap();
    let mut ev = BallEvents::new();
    for _ in 0..50 {
        s1.step(&geo, DT, &mut ev);
        s2.step(&geo, DT, &mut ev);
    }
    assert_eq!(magnetisation(&rem, h2), 0.0);
}

// ------------------------------------- Fix task: free-flight accuracy

/// The 2.7 spec failing case (spec_2_7_magnus::prop_free_flight seed):
/// with gravity on, the per-tick lateral acceleration (velocity change
/// minus g) matches `S_m (ω × v̄)/m` within 1 % plus a small floor, with
/// `v̄` the tick's mean velocity. Before the midpoint fix the error was
/// (S_m/m)·ω × (g·h/2) ≈ 1.8e-3 m/s² against a bound of 7.8e-4.
#[test]
fn free_flight_lateral_acceleration_with_gravity() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut profs = profiles();
    let s_m = 3.011_375_2e-5;
    profs[GLOBALL.index()].magnus_coeff = s_m;
    let mut s = BallSolver::new(config(), &profs);
    register_all(&mut s);
    let mut b = ball_at(GLOBALL, v(0.0, 0.0, 1.0), v(-2.42, -1.28, -1.28));
    b.angular_velocity = v(-255.0, -155.0, -127.0);
    let h = s.add_ball(b).unwrap();
    let g = config().gravity;
    let m = profs[GLOBALL.index()].mass_kg;
    let mut ev = BallEvents::new();
    let mut worst = 0.0f32;
    for _ in 0..40 {
        let before = *s.ball(h).unwrap();
        s.step(&geo, DT, &mut ev);
        let after = *s.ball(h).unwrap();
        let acc = (after.velocity - before.velocity) / DT - g;
        let mean = 0.5 * (before.velocity + after.velocity);
        let expect = s_m * before.angular_velocity.cross(mean) / m;
        let err = (acc - expect).length();
        worst = worst.max(err / expect.length());
        assert!(err <= 0.01 * expect.length() + 1e-4, "{err} vs {expect}");
    }
    eprintln!("worst relative error {worst:e}");
    // And against the start-of-tick velocity, as the spec states it.
    let b0 = *s.ball(h).unwrap();
    s.step(&geo, DT, &mut ev);
    let b1 = *s.ball(h).unwrap();
    let acc = (b1.velocity - b0.velocity) / DT - g;
    let expect = s_m * b0.angular_velocity.cross(b0.velocity) / m;
    assert!((acc - expect).length() <= 0.01 * expect.length() + 1e-3);
}

/// `Magnus` (default gravity) registered by hand is bit-identical to the
/// gravity-aware hook `register_all` installs on a default-gravity solver.
#[test]
fn plain_magnus_matches_register_all_at_default_gravity() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let run = |plain: bool| {
        let mut s = solver();
        if plain {
            s.add_external_force(Box::new(Magnus));
        } else {
            register_all(&mut s);
        }
        let mut b = ball_at(GLOBALL, v(0.0, 0.0, 1.0), v(3.0, -1.0, 2.0));
        b.angular_velocity = v(100.0, 300.0, -200.0);
        let h = s.add_ball(b).unwrap();
        let mut ev = BallEvents::new();
        for _ in 0..100 {
            s.step(&geo, DT, &mut ev);
        }
        bits(s.ball(h).unwrap())
    };
    assert_eq!(run(true), run(false));
}
