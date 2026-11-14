//! Subphase 2.7 property tests: the Magnus and remanence force helpers and
//! the hooks inside the solver.

#[path = "ball_util.rs"]
mod util;

use glam::Vec3;
use pin_core::ids::ProfileId;
use pin_physics::ball::{BallEvents, Geometry, Magnet, MagnetState};
use pin_physics::ball_ext::{
    magnetisation, magnetisation_step, magnus_force, magnus_step_force, register_all,
    remanence_decay, residual_force,
};
use pin_physics::world::layers;
use proptest::prelude::*;
use util::*;

fn vec3(range: f32) -> impl Strategy<Value = Vec3> {
    (-range..range, -range..range, -range..range).prop_map(|(x, y, z)| v(x, y, z))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// The sub-step Magnus force rotates `v`: the speed after `F h / m`
    /// equals the speed before up to rounding, and it matches `S_m (ω × v)`
    /// to first order in `φ = S_m |ω| h / m`.
    #[test]
    fn magnus_step_keeps_speed_and_matches_the_force(
        s_m in 0.0f32..5.0e-5,
        mass in 0.01f32..0.5,
        omega in vec3(1000.0),
        vel in vec3(30.0),
        h in 1.0e-5f32..1.0e-3,
    ) {
        let f = magnus_step_force(s_m, mass, omega, vel, h);
        prop_assert!(f.is_finite());
        let after = vel + f * (h / mass);
        let speed = vel.length();
        prop_assert!((after.length() - speed).abs() <= 8.0 * f32::EPSILON * speed.max(1e-6));
        let exact = magnus_force(s_m, omega, vel);
        let phi = s_m * omega.length() * h / mass;
        prop_assert!(
            (f - exact).length() <= exact.length() * (phi + 1e-4) + 1e-9,
            "{f} vs {exact}, phi {phi}"
        );
        // No component along v beyond rounding of the second-order term.
        prop_assert!(f.dot(vel).abs() <= exact.length() * speed * (phi + 1e-4) + 1e-9);
    }

    #[test]
    fn magnus_zero_coefficient_or_spin_is_exactly_no_force(
        mass in 0.01f32..0.5,
        omega in vec3(1000.0),
        vel in vec3(30.0),
    ) {
        for f in [
            magnus_step_force(0.0, mass, omega, vel, DT),
            magnus_step_force(1.0e-5, mass, Vec3::ZERO, vel, DT),
        ] {
            prop_assert_eq!(f, Vec3::ZERO);
            // -0.0: adding it changes no bit of any velocity.
            for x in [vel.x, -0.0, 0.0] {
                prop_assert_eq!((x + f.x).to_bits(), x.to_bits());
            }
        }
    }

    /// `M` stays in [0, 1], moves monotonically toward the target and is
    /// within 1 % of its start after 5τ of decay.
    #[test]
    fn magnetisation_law(
        m0 in 0.0f32..=1.0,
        target in -0.5f32..1.5,
        tau in 1.0e-3f32..1.0,
        h in 1.0e-5f32..1.0e-3,
    ) {
        let t = target.clamp(0.0, 1.0);
        let m1 = magnetisation_step(m0, target, h, tau);
        prop_assert!((0.0..=1.0).contains(&m1));
        prop_assert!((m1 - t).abs() <= (m0 - t).abs() + 1e-7);
        let k = remanence_decay(h, tau);
        prop_assert!((0.0..=1.0).contains(&k));
        let n = (5.0 * tau / h).ceil() as u32;
        let mut m = m0;
        for _ in 0..n.min(500_000) {
            m = magnetisation_step(m, 0.0, h, tau);
        }
        prop_assert!(m <= 0.01 * m0 + 1e-12, "{m0} → {m}");
    }

    /// The residual pull points at the pole, scales with c·ρ·M, is bounded
    /// by `c·ρ·M·K / d_min³`, and is exactly zero with c, ρ or M zero or out
    /// of range.
    #[test]
    fn residual_force_helper(
        pos in vec3(0.12),
        c in 0.0f32..=1.0,
        rho in 0.0f32..=1.0,
        m in 0.0f32..=1.0,
        layer in 0u8..8,
    ) {
        let ms = MagnetState {
            magnet: Magnet {
                position: Vec3::ZERO,
                k_coil: 2.0e-5,
                d_min: 0.01,
                range: 0.08,
                layer_mask: layers::layer_bit(3) | layers::layer_bit(5),
            },
            power: 0.0,
            remanence_tau_s: 0.06,
        };
        let f = residual_force(&ms, pos, layer, c, rho, m);
        if c * rho * m == 0.0 || !ms.in_range(pos, layer) {
            prop_assert_eq!(f, Vec3::ZERO);
        } else {
            prop_assert!(f.dot(pos) <= 0.0);
            prop_assert!(f.length() <= c * rho * m * 2.0e-5 / 1.0e-6 * 1.0001);
            let unit = ms.pull(pos);
            prop_assert!((f - unit * (c * rho * m)).length() <= 1e-6 * f.length());
        }
    }

    /// A spinning Glo-Ball in free flight keeps its speed with the hooks.
    #[test]
    fn free_flight_speed_is_kept(omega in vec3(800.0), vel in vec3(15.0)) {
        let world = empty_world();
        let geo = Geometry::new(&world);
        let mut s = solver_no_gravity();
        register_all(&mut s);
        let mut b = ball_at(GLOBALL, Vec3::ZERO, vel);
        b.angular_velocity = omega;
        let h = s.add_ball(b).unwrap();
        let mut ev = BallEvents::new();
        for _ in 0..100 {
            s.step(&geo, DT, &mut ev);
        }
        let after = s.ball(h).unwrap().velocity.length();
        let speed = vel.length();
        prop_assert!((after - speed).abs() <= 100.0 * 4.0 * f32::EPSILON * speed.max(1e-6));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// In the solver, every ball's M stays in [0, 1] with magnets switching.
    #[test]
    fn magnetisation_stays_in_unit_interval(
        x in -0.06f32..0.06,
        y in -0.06f32..0.06,
        vx in -0.5f32..0.5,
        tau in 0.0f32..0.2,
        pattern in 0u32..64,
        profile in 0u16..7,
    ) {
        let world = floor_world();
        let geo = Geometry::new(&world);
        let mut s = solver();
        let id = s.add_magnet(Magnet {
            position: v(0.0, 0.0, -0.005),
            k_coil: 2.0e-5,
            d_min: 0.01,
            range: 0.08,
            layer_mask: layers::ALL,
        });
        s.set_magnet_remanence_tau(id, tau);
        let rem = register_all(&mut s);
        let p = ProfileId(profile);
        let h = s.add_ball(ball_at(p, v(x, y, radius(p)), v(vx, 0.0, 0.0))).unwrap();
        let mut ev = BallEvents::new();
        for tick in 0..300u32 {
            let bit = (tick / 50) % 6;
            s.set_magnet_power(id, if pattern & (1 << bit) != 0 { 0.8 } else { 0.0 });
            s.step(&geo, DT, &mut ev);
            let m = magnetisation(&rem, h);
            prop_assert!((0.0..=1.0).contains(&m), "{m}");
        }
    }
}
