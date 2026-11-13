//! Subphase 2.3 spec tests (Independent Tester): property tests of the
//! solver invariants: no tunnelling (ball speeds, flip timings, mount
//! normals, bat tip speeds up to 15 m/s), no energy created by the
//! ball–bat impulse beyond the coil's work, and a flipper on a tilted
//! `surface_normal` behaving like the flat case rotated.

#[path = "spec_2_3_common.rs"]
mod common;

use common::*;
use glam::{Quat, Vec3};
use pin_core::ids::FlipperId;
use proptest::prelude::*;

const ID: FlipperId = FlipperId(0);

fn unit(theta: f32, phi: f32) -> Vec3 {
    v(
        libm::sinf(theta) * libm::cosf(phi),
        libm::sinf(theta) * libm::sinf(phi),
        libm::cosf(theta),
    )
}

/// Free-space shot at a bat on an arbitrary mount: returns Err(message) on a
/// tunnel or a deep penetration.
#[allow(clippy::too_many_arguments)]
fn tunnel_case(
    n: Vec3,
    base_mm: [f32; 3],
    rest: f32,
    sweep: f32,
    power: f32,
    speed: f32,
    frac: f32,
    beta_deg: f32,
    from_front: bool,
    flip_offset_ms: i64,
) -> Result<(f32, bool), String> {
    let mut c = config(0, base_mm, n, rest, sweep);
    c.power_torque_nm = power;
    let f = frames(std::slice::from_ref(&c))[0];
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
    // The bat moves toward +m for a positive sweep.
    let m = f.n.cross(f.e_rest) * sweep.signum();
    let side = if from_front { 1.0 } else { -1.0 };
    let a = f.pivot + f.e_rest * (frac * L);
    let b = beta_deg.to_radians();
    let out = (m * side * libm::cosf(b) + f.e_rest * libm::sinf(b)).normalize();
    let dist = (speed * 0.04).max(0.12);
    let arrive = (dist / speed / DT) as i64;
    let flip = (arrive + flip_offset_ms).max(0) as u64;
    let h = s.solver.add_ball(ball(a + out * dist, -out * speed)).unwrap();
    let mut prev: Option<f32> = None;
    let mut max_tip = 0.0f32;
    let mut hit = false;
    let ticks = arrive as u64 + 80;
    while s.tick < ticks {
        if s.tick == flip {
            s.set.set_energised(ID, true);
        }
        s.advance();
        hit |= s.hit(h, 0);
        let ang = s.set.angle_deg(ID);
        max_tip = max_tip.max(s.set.angular_velocity(ID).abs() * L);
        let p = s.ball(h).position;
        if !p.is_finite() {
            return Err(format!("non-finite ball at tick {}", s.tick));
        }
        let (along, across, normal) = f.local(ang, Vec3::ZERO, p);
        if (0.0..=0.8 * L).contains(&along) {
            let r = (across * across + normal * normal).sqrt();
            if r < R + TIP_R - 1.0e-3 {
                return Err(format!(
                    "tick {}: ball centre {r} m from the bat axis at {along} m (bat ≥ {} m)",
                    s.tick,
                    TIP_R
                ));
            }
        }
        let in_range = (0.1 * L..=0.7 * L).contains(&along) && normal.abs() < 0.01;
        let now = if in_range { Some(across.signum()) } else { None };
        if let (Some(a0), Some(a1)) = (prev, now) {
            if a0 != a1 {
                return Err(format!("tick {}: ball crossed the bat (along {along})", s.tick));
            }
        }
        prev = now;
    }
    Ok((max_tip, hit))
}

#[test]
fn no_tunnelling_at_15_m_s_tip_speed_head_on() {
    // Strong coils (3 N·m, the Appendix C maximum, and 8 N·m so the tip
    // passes 15 m/s whatever the coil model), fast balls, hits near the tip, flat mount.
    let mut fastest = 0.0f32;
    for power in [3.0f32, 8.0] {
        for speed in [5.0f32, 15.0, 30.0, 50.0] {
            for off in [-12i64, -8, -4, -2, 0, 2] {
                for frac in [0.5f32, 0.75, 0.85] {
                    let r = tunnel_case(Vec3::Z, [0.0, 0.0, 0.0], 10.0, 50.0, power, speed, frac, 0.0, true, off);
                    match r {
                        Ok((tip, _)) => fastest = fastest.max(tip),
                        Err(e) => panic!("power {power} speed {speed} offset {off} frac {frac}: {e}"),
                    }
                }
            }
        }
    }
    println!("max bat tip (axis end) speed {fastest:.2} m/s");
    assert!(fastest >= 15.0, "scenario never reached a 15 m/s tip: {fastest}");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, .. ProptestConfig::default() })]

    #[test]
    fn no_tunnelling_any_mount_speed_and_timing(
        theta in 0.0f32..3.14159, phi in -3.14159f32..3.14159,
        bx in -200.0f32..200.0, by in -200.0f32..200.0, bz in -100.0f32..100.0,
        rest in -180.0f32..180.0, pos_sweep in any::<bool>(),
        power in 1.2f32..3.0,
        speed in 0.5f32..50.0,
        frac in 0.15f32..0.85,
        beta in -60.0f32..60.0,
        from_front in any::<bool>(),
        off in -30i64..30,
    ) {
        let n = unit(theta, phi);
        let sweep = if pos_sweep { 50.0 } else { -50.0 };
        let r = tunnel_case(n, [bx, by, bz], rest, sweep, power, speed, frac, beta, from_front, off);
        prop_assert!(r.is_ok(), "{}", r.unwrap_err());
    }
}

// ------------------------------------------------------------------ energy

/// Per-tick energy audit on the pitched table: ball + bat mechanical energy
/// may rise by no more than the coil and spring work `(τ_power + τ_spring)·|Δθ|`.
fn energy_case(speed: f32, frac: f32, beta_deg: f32, flip_off: i64, release_after: Option<u64>, damping: f32, restitution: f32) -> Result<(), String> {
    let base = [0.0, 0.0, PIVOT_Z_MM];
    let rest = rest_for(base, Vec3::Z, 50.0, flat_dir(-30.0));
    let mut c = config(0, base, Vec3::Z, rest, 50.0);
    c.rubber_damping = damping;
    c.rubber_restitution = restitution;
    let f = frames(std::slice::from_ref(&c))[0];
    let g = table_gravity(Quat::IDENTITY);
    let mut s = Scene::new(table_world(Quat::IDENTITY), g, std::slice::from_ref(&c));
    let a = f.pivot + f.e_rest * (frac * L);
    let dir = Quat::from_rotation_z(beta_deg.to_radians()) * v(0.0, -1.0, 0.0);
    let start = v(a.x, a.y, R) - dir * 0.15;
    let h = s.solver.add_ball(ball(start, dir * speed)).unwrap();
    let arrive = (0.15 / speed / DT) as i64;
    let flip = (arrive + flip_off).max(0) as u64;
    let rel = release_after.map(|r| flip + r);
    let inertia = [c.inertia_kg_m2];
    let tau = c.power_torque_nm.max(c.hold_torque_nm) + c.return_spring_torque_nm;
    let mut e_prev = energy(&s, g, &inertia);
    let e0 = e_prev;
    let mut a_prev = s.set.angle_deg(ID);
    let mut work = 0.0f64;
    while s.tick < arrive as u64 + 250 {
        if s.tick == flip {
            s.set.set_energised(ID, true);
        }
        if Some(s.tick) == rel {
            s.set.set_energised(ID, false);
        }
        s.advance();
        let e = energy(&s, g, &inertia);
        let a1 = s.set.angle_deg(ID);
        let w = f64::from(tau) * f64::from((a1 - a_prev).abs().to_radians());
        work += w;
        let b = s.ball(h);
        let ke = 0.5 * 0.08 * f64::from(b.velocity.length_squared());
        let tol = 2.0e-4 + 3.0e-3 * ke;
        if e - e_prev > w + tol {
            return Err(format!(
                "tick {}: energy rose by {:.6} J, coil+spring work bound {:.6} J",
                s.tick,
                e - e_prev,
                w
            ));
        }
        e_prev = e;
        a_prev = a1;
    }
    if e_prev - e0 > work + 1.0e-3 {
        return Err(format!("run: energy rose by {:.6} J, work bound {:.6} J", e_prev - e0, work));
    }
    Ok(())
}

#[test]
fn energy_unpowered_bat_never_gains() {
    // No coil: the bat only takes energy from the ball (rest stop engaged
    // or pushed off it), never gives more back.
    for speed in [1.0f32, 3.5, 8.0] {
        for frac in [0.2f32, 0.5, 0.85] {
            for beta in [-30.0f32, 0.0, 30.0] {
                let r = energy_case(speed, frac, beta, 100_000, None, 0.35, 0.65);
                assert!(r.is_ok(), "speed {speed} frac {frac} beta {beta}: {}", r.unwrap_err());
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, .. ProptestConfig::default() })]

    #[test]
    fn energy_never_exceeds_coil_work(
        speed in 0.5f32..8.0,
        frac in 0.15f32..0.85,
        beta in -40.0f32..40.0,
        flip_off in -30i64..30,
        release in proptest::option::of(0u64..60),
        damping in 0.1f32..0.7,
        restitution in 0.3f32..0.9,
    ) {
        let r = energy_case(speed, frac, beta, flip_off, release, damping, restitution);
        prop_assert!(r.is_ok(), "{}", r.unwrap_err());
    }
}

// ------------------------------------------------------------ tilted mount

/// The V1 scene (ball at 3.5 m/s down the table onto the left bat, flip at
/// `flip`) rotated by `rot` as a whole: playfield, gravity, flipper base and
/// normal, bat direction and the ball. Returns per tick (angle − rest,
/// ball position, ball velocity), mapped back to the flat frame.
fn rotated_run(rot: Quat, flip: u64, ticks: u64) -> Vec<(f32, Vec3, Vec3)> {
    let flat_base = v(0.0, 0.0, PIVOT_Z_MM);
    let base = (rot * flat_base).to_array();
    let n = rot * Vec3::Z;
    let rest = rest_for(base, n, 50.0, rot * flat_dir(-30.0));
    let c = config(0, base, n, rest, 50.0);
    let f = frames(std::slice::from_ref(&c))[0];
    let mut s = Scene::new(table_world(rot), table_gravity(rot), std::slice::from_ref(&c));
    // Same flat-frame ball start for every rotation.
    let fp = rot.inverse() * (f.pivot + f.e_rest * (0.5 * L));
    let start = v(fp.x, fp.y + 0.12, R);
    let h = s.solver.add_ball(ball(rot * start, rot * v(0.0, -3.5, 0.0))).unwrap();
    let inv = rot.inverse();
    let mut out = Vec::new();
    while s.tick < ticks {
        if s.tick == flip {
            s.set.set_energised(ID, true);
        }
        s.advance();
        let b = s.ball(h);
        out.push((s.set.angle_deg(ID) - rest, inv * b.position, inv * b.velocity));
    }
    out
}

fn compare_rotated(rot: Quat, flip: u64) -> Result<(), String> {
    let flat = rotated_run(Quat::IDENTITY, flip, 140);
    let tilt = rotated_run(rot, flip, 140);
    for (k, (a, b)) in flat.iter().zip(&tilt).enumerate() {
        let da = (a.0 - b.0).abs();
        let dp = (a.1 - b.1).length();
        let dv = (a.2 - b.2).length();
        if da > 0.05 || dp > 1.0e-3 || dv > 0.05 + 0.02 * a.2.length() {
            return Err(format!(
                "tick {k}: Δangle {da}°, Δpos {dp} m, Δvel {dv} m/s (flat {a:?}, tilted {b:?})"
            ));
        }
    }
    Ok(())
}

#[test]
fn tilted_mount_behaves_like_flat_rotated_fixed_cases() {
    let cases = [
        Quat::from_rotation_x(0.6),
        Quat::from_rotation_y(-1.1),
        Quat::from_rotation_z(2.0),
        Quat::from_axis_angle(v(1.0, 1.0, 1.0).normalize(), 2.5),
        Quat::from_rotation_x(std::f32::consts::FRAC_PI_2), // vertical (backbox) playfield
        Quat::from_rotation_y(std::f32::consts::PI),        // upside down
    ];
    for rot in cases {
        for flip in [10u64, 30, 33] {
            let r = compare_rotated(rot, flip);
            assert!(r.is_ok(), "rotation {rot:?}, flip {flip}: {}", r.unwrap_err());
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 16, .. ProptestConfig::default() })]

    #[test]
    fn tilted_mount_behaves_like_flat_rotated(
        theta in 0.0f32..3.14159, phi in -3.14159f32..3.14159, angle in 0.0f32..3.14159,
        flip in 0u64..60,
    ) {
        let rot = Quat::from_axis_angle(unit(theta, phi), angle);
        let r = compare_rotated(rot, flip);
        prop_assert!(r.is_ok(), "{}", r.unwrap_err());
    }
}
