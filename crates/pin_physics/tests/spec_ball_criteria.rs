//! Independent spec tests (Subphase 2.2): the five PLAN.md 2.2 verification
//! criteria, with the brief's tolerances and parameter sweeps.
//!
//! Black-box against the FIXED API of `pin_physics::ball`.

#[path = "spec_ball_common.rs"]
mod common;

use common::*;
use glam::{DVec3, Vec3};
use pin_physics::ball::{Ball, BallSolver, Magnet, NewtonChain};
use pin_physics::world::{CubicSpline, StaticWorld, layers};
use proptest::prelude::*;

// ------------------------------------------------------------ criterion 1

/// Identical impulse on a resting Powerball and a resting steel ball on the
/// same flat floor: launch speed ratio (tick after the impulse) is
/// 80/65 = 1.231 ± 0.01. Swept over impulse magnitude and direction.
fn launch_ratio(impulse: Vec3) -> f32 {
    let world = floor_world();
    let g = geo(&world);
    let mut s = solver();
    let r = profile("standard_steel").radius_m();
    let steel = s
        .add_ball(ball("standard_steel", Vec3::new(0.0, -0.5, r)))
        .unwrap();
    let power = s
        .add_ball(ball("ceramic_powerball", Vec3::new(0.0, 0.5, r)))
        .unwrap();
    let mut ev = events();
    // Let both settle on the floor first.
    for _ in 0..20 {
        ev.clear();
        s.step(&g, DT, &mut ev);
    }
    s.apply_impulse(steel, impulse);
    s.apply_impulse(power, impulse);
    ev.clear();
    s.step(&g, DT, &mut ev);
    let vs = s.ball(steel).unwrap().velocity.length();
    let vp = s.ball(power).unwrap().velocity.length();
    assert!(vs > 0.0, "steel ball did not move");
    vp / vs
}

#[test]
fn c1_powerball_launch_ratio_sweep() {
    let expected = 0.080 / 0.065; // 1.2308
    for &speed_steel in &[0.5f32, 1.0, 2.0, 3.0, 4.0] {
        for k in 0..8 {
            let a = k as f32 * std::f32::consts::FRAC_PI_4;
            let j = 0.080 * speed_steel;
            let impulse = Vec3::new(libm::cosf(a), libm::sinf(a), 0.0) * j;
            let ratio = launch_ratio(impulse);
            assert!(
                (ratio - expected).abs() <= 0.01,
                "speed {speed_steel} dir {k}: ratio {ratio} (expected {expected} ± 0.01)"
            );
        }
    }
}

// ------------------------------------------------------------ criterion 2

/// A scenario with a floor, walls and a post so the ball has contacts while
/// it passes over/near the magnet.
fn magnet_world() -> StaticWorld {
    let mut b = pin_physics::world::StaticWorldBuilder::new();
    b.add_collider(floor_shape(), layers::ALL, true);
    b.add_collider(
        cuboid(Vec3::new(0.35, 0.0, 0.05), Vec3::new(0.01, 0.3, 0.05)),
        layers::ALL,
        true,
    );
    b.add_collider(
        pin_physics::world::StaticShape::Cylinder {
            base: Vec3::new(0.15, 0.03, 0.0),
            axis: Vec3::new(0.0, 0.0, 0.05),
            radius: 0.01,
        },
        layers::ALL,
        true,
    );
    b.build()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MagnetMode {
    Absent,
    Off,
    On(u32),
}

/// Runs `ticks` ticks of a ball rolled across a magnet; returns every tick's
/// (position, velocity, angular velocity).
fn magnet_trajectory(
    name: &str,
    mode: MagnetMode,
    start: Vec3,
    v0: Vec3,
    w0: Vec3,
    ticks: usize,
) -> Vec<[Vec3; 3]> {
    let world = magnet_world();
    let g = geo(&world);
    let mut s = solver();
    if mode != MagnetMode::Absent {
        let id = s.add_magnet(Magnet {
            position: Vec3::new(0.1, 0.0, -0.005),
            k_coil: 2.0e-5,
            d_min: 0.01,
            range: 0.08,
            layer_mask: layers::ALL,
        });
        let p = match mode {
            MagnetMode::On(bits) => f32::from_bits(bits),
            _ => 0.0,
        };
        s.set_magnet_power(id, p);
    }
    let mut b = ball(name, start);
    b.velocity = v0;
    b.angular_velocity = w0;
    let h = s.add_ball(b).unwrap();
    let mut ev = events();
    let mut out = Vec::with_capacity(ticks);
    for _ in 0..ticks {
        ev.clear();
        s.step(&g, DT, &mut ev);
        let b = s.ball(h).unwrap();
        out.push([b.position, b.velocity, b.angular_velocity]);
    }
    out
}

fn assert_bit_identical(a: &[[Vec3; 3]], b: &[[Vec3; 3]], what: &str) {
    assert_eq!(a.len(), b.len());
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        for k in 0..3 {
            assert!(
                x[k].x.to_bits() == y[k].x.to_bits()
                    && x[k].y.to_bits() == y[k].y.to_bits()
                    && x[k].z.to_bits() == y[k].z.to_bits(),
                "{what}: tick {i} component {k} differs: {:?} vs {:?}",
                x[k],
                y[k]
            );
            assert!(x[k] == y[k]);
        }
    }
}

#[test]
fn c2_nonmagnetic_balls_ignore_active_magnet_bit_exact() {
    let r = 0.0135;
    let cases = [
        (
            Vec3::new(-0.05, 0.0, r),
            Vec3::new(1.5, 0.0, 0.0),
            Vec3::ZERO,
        ),
        (
            Vec3::new(-0.05, 0.01, r + 0.004),
            Vec3::new(2.0, 0.3, 0.0),
            Vec3::new(0.0, 40.0, 5.0),
        ),
        (
            Vec3::new(0.3, -0.05, r),
            Vec3::new(-1.0, 0.4, 0.0),
            Vec3::new(-10.0, 0.0, 0.0),
        ),
    ];
    for name in ["ceramic_powerball", "uv_globall"] {
        for (k, &(p, v, w)) in cases.iter().enumerate() {
            let on = magnet_trajectory(name, MagnetMode::On(1.0f32.to_bits()), p, v, w, 600);
            let off = magnet_trajectory(name, MagnetMode::Off, p, v, w, 600);
            let absent = magnet_trajectory(name, MagnetMode::Absent, p, v, w, 600);
            assert_bit_identical(&on, &off, &format!("{name} case {k} on/off"));
            assert_bit_identical(&on, &absent, &format!("{name} case {k} on/absent"));
        }
    }
}

#[test]
fn c2_control_steel_ball_is_deflected_by_the_same_magnet() {
    // Guards against a vacuous criterion-2 pass: the magnet in the scenario
    // must actually act on a magnetic ball.
    let r = 0.0135;
    let p = Vec3::new(-0.05, 0.0, r);
    let v = Vec3::new(1.5, 0.0, 0.0);
    let on = magnet_trajectory(
        "standard_steel",
        MagnetMode::On(1.0f32.to_bits()),
        p,
        v,
        Vec3::ZERO,
        200,
    );
    let off = magnet_trajectory("standard_steel", MagnetMode::Off, p, v, Vec3::ZERO, 200);
    assert!(
        on.iter().zip(&off).any(|(a, b)| a[0] != b[0]),
        "steel ball trajectory unchanged by an active magnet"
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 12, .. ProptestConfig::default() })]

    #[test]
    fn c2_sweep_nonmagnetic_bit_exact(
        which in 0usize..2,
        x0 in -0.1f32..0.0, y0 in -0.03f32..0.03,
        vx in 0.2f32..3.0, vy in -0.5f32..0.5,
        wx in -50.0f32..50.0, wy in -50.0f32..50.0, wz in -50.0f32..50.0,
        power in 0.01f32..1.0,
    ) {
        let name = ["ceramic_powerball", "uv_globall"][which];
        let p = Vec3::new(x0, y0, 0.0135);
        let v = Vec3::new(vx, vy, 0.0);
        let w = Vec3::new(wx, wy, wz);
        let on = magnet_trajectory(name, MagnetMode::On(power.to_bits()), p, v, w, 300);
        let off = magnet_trajectory(name, MagnetMode::Off, p, v, w, 300);
        assert_bit_identical(&on, &off, name);
    }
}

// ------------------------------------------------------------ criterion 3

/// A straight horizontal path along +x whose parameter is exactly
/// `x = t · spacing` (evenly spaced collinear knots give a linear spline).
fn straight_path(spacing: f32, knots: usize) -> CubicSpline {
    let pts: Vec<Vec3> = (0..knots)
        .map(|i| Vec3::new(i as f32 * spacing, 0.0, 0.0))
        .collect();
    CubicSpline::interpolate(&pts).unwrap()
}

#[test]
fn c3_strike_reaches_far_ball_within_two_ticks() {
    for n in [3usize, 4, 5, 7] {
        let d = profile("standard_steel").diameter_m;
        let path = straight_path(d, n + 4);
        let ts: Vec<f32> = (1..=n).map(|i| i as f32).collect();
        let mut chain = NewtonChain::new(path, pid("standard_steel"), &ts).unwrap();
        assert_eq!(chain.len(), n);
        assert_eq!(chain.ball_speed(n - 1), 0.0);
        chain.strike(0, 1.0);
        assert_eq!(chain.ball_speed(0), 1.0);
        let mut s = solver();
        let ci = s.add_newton_chain(chain);
        assert_eq!(s.newton_chain(ci).len(), n);
        let world = empty_world();
        let g = geo(&world);
        let mut ev = events();
        for _ in 0..2 {
            ev.clear();
            s.step(&g, DT, &mut ev);
        }
        let c = s.newton_chain(ci);
        assert!(
            c.ball_speed(n - 1) > 0.0,
            "chain of {n}: far ball speed {} after 2 ticks",
            c.ball_speed(n - 1)
        );
        // Kinetic energy along the path never increases (equal masses).
        let e: f32 = (0..n).map(|i| c.ball_speed(i) * c.ball_speed(i)).sum();
        assert!(e <= 1.0 + 1e-5, "chain energy grew: Σv² = {e}");
        // Positions are on the path at the reported parameters.
        for i in 0..n {
            let p = c.ball_position(i);
            assert!(
                (p.x - c.ball_t(i) * d).abs() < 1e-5,
                "ball {i} off path: {p:?}"
            );
            assert!(p.y.abs() < 1e-5 && p.z.abs() < 1e-5);
        }
    }
}

#[test]
fn c3_free_ball_impact_reaches_far_ball_within_two_ticks() {
    for n in [3usize, 5] {
        let prof = profile("standard_steel");
        let d = prof.diameter_m;
        let r = prof.radius_m();
        let path = straight_path(d, n + 4);
        let ts: Vec<f32> = (1..=n).map(|i| i as f32).collect();
        let chain = NewtonChain::new(path, pid("standard_steel"), &ts).unwrap();
        let mut s = solver_with_gravity(Vec3::ZERO);
        let ci = s.add_newton_chain(chain);
        // First chain ball centre at x = d; free ball touches it at x = d - 2r = 0.
        // Start 0.5 mm short, moving +x at 1 m/s: impact inside the first tick.
        let mut fb = ball("standard_steel", Vec3::new(-0.0005, 0.0, 0.0));
        fb.velocity = Vec3::new(1.0, 0.0, 0.0);
        let h = s.add_ball(fb).unwrap();
        let world = empty_world();
        let g = geo(&world);
        let mut ev = events();
        let mut saw_newton_impact = false;
        for _ in 0..2 {
            ev.clear();
            s.step(&g, DT, &mut ev);
            for e in ev.as_slice() {
                if let pin_physics::ball::BallEvent::Impact {
                    ball,
                    other: pin_physics::ball::ImpactTarget::Newton(c),
                    normal_speed,
                } = *e
                {
                    assert_eq!(ball, h);
                    assert_eq!(c, ci);
                    assert!(normal_speed > 0.5, "impact speed {normal_speed}");
                    saw_newton_impact = true;
                }
            }
        }
        let c = s.newton_chain(ci);
        assert!(
            c.ball_speed(n - 1) > 0.0,
            "chain of {n}: far ball speed {} two ticks after impact",
            c.ball_speed(n - 1)
        );
        assert!(
            saw_newton_impact,
            "no Impact{{other: Newton}} event reported"
        );
        // The free ball must not have passed through the chain ball.
        let fbp = s.ball(h).unwrap().position;
        assert!(
            fbp.x <= c.ball_position(0).x - 2.0 * r + 1e-4,
            "free ball overlaps first chain ball: {fbp:?} vs {:?}",
            c.ball_position(0)
        );
        let _ = r;
    }
}

#[test]
fn c3_newton_chain_rejects_descending_parameters() {
    let d = 0.027;
    assert!(
        NewtonChain::new(straight_path(d, 8), pid("standard_steel"), &[3.0, 2.0, 1.0]).is_err()
    );
}

// ------------------------------------------------------------ criterion 4

struct TwoBody {
    p: DVec3,
    l: DVec3,
    ke: f64,
}

fn two_body(s: &BallSolver, profs: &[pin_core::BallProfile]) -> TwoBody {
    let balls: Vec<&Ball> = s.balls().map(|(_, b)| b).collect();
    let mut m_tot = 0.0f64;
    let mut com = DVec3::ZERO;
    let mut p = DVec3::ZERO;
    for b in &balls {
        let m = f64::from(profs[b.profile.index()].mass_kg);
        m_tot += m;
        com += b.position.as_dvec3() * m;
        p += b.velocity.as_dvec3() * m;
    }
    com /= m_tot;
    let mut l = DVec3::ZERO;
    let mut ke = 0.0;
    for b in &balls {
        let pr = &profs[b.profile.index()];
        let m = f64::from(pr.mass_kg);
        let i = f64::from(pr.moment_of_inertia());
        l += (b.position.as_dvec3() - com).cross(b.velocity.as_dvec3() * m);
        l += b.angular_velocity.as_dvec3() * i;
        ke += kinetic(b, pr);
    }
    TwoBody { p, l, ke }
}

/// Mini vs jumbo, zero gravity, no static geometry. `b` is the impact
/// parameter as a fraction of the contact distance, `vrel` the closing speed.
fn mini_jumbo(b: f32, vrel: f32, w_mini: Vec3, w_jumbo: Vec3, v_jumbo: Vec3) {
    let profs = profiles();
    let mini = profile("mini_steel");
    let jumbo = profile("jumbo_steel");
    let reach = mini.radius_m() + jumbo.radius_m();
    let mut s = solver_with_gravity(Vec3::ZERO);
    let mut a = ball("mini_steel", Vec3::new(-0.06, b * reach, 0.0));
    a.velocity = Vec3::new(vrel, 0.0, 0.0) + v_jumbo;
    a.angular_velocity = w_mini;
    let mut c = ball("jumbo_steel", Vec3::ZERO);
    c.velocity = v_jumbo;
    c.angular_velocity = w_jumbo;
    s.add_ball(a).unwrap();
    s.add_ball(c).unwrap();
    let world = empty_world();
    let g = geo(&world);
    let mut ev = events();
    let t0 = two_body(&s, &profs);
    // Characteristic scales so near-zero totals do not make "relative" meaningless.
    let mu = f64::from(mini.mass_kg * jumbo.mass_kg / (mini.mass_kg + jumbo.mass_kg));
    let p_scale = t0.p.length().max(mu * f64::from(vrel));
    let l_scale = t0.l.length().max(mu * f64::from(vrel) * f64::from(reach));
    let mut prev_ke = t0.ke;
    let mut collided = false;
    for tick in 0..250 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        if ev
            .as_slice()
            .iter()
            .any(|e| matches!(e, pin_physics::ball::BallEvent::Impact { .. }))
        {
            collided = true;
        }
        let t = two_body(&s, &profs);
        let dp = (t.p - t0.p).length() / p_scale;
        let dl = (t.l - t0.l).length() / l_scale;
        assert!(
            dp <= 1e-4,
            "tick {tick}: momentum drift {dp:e} (b={b}, v={vrel})"
        );
        assert!(
            dl <= 1e-3,
            "tick {tick}: angular momentum drift {dl:e} (b={b}, v={vrel})"
        );
        assert!(
            t.ke <= prev_ke * (1.0 + 1e-6) + 1e-12,
            "tick {tick}: KE rose {prev_ke:e} -> {:e} (b={b}, v={vrel})",
            t.ke
        );
        prev_ke = t.ke;
    }
    // Sanity: the balls must actually have met (unless the miss is intended).
    if b < 0.95 {
        assert!(collided, "no impact reported (b={b}, v={vrel})");
        // And they must not interpenetrate at the end.
        let ps: Vec<Vec3> = s.balls().map(|(_, x)| x.position).collect();
        assert!((ps[0] - ps[1]).length() >= reach - 1e-4);
    }
}

#[test]
fn c4_mini_jumbo_head_on_and_glancing() {
    for &b in &[0.0f32, 0.3, 0.6, 0.9] {
        for &v in &[0.5f32, 2.0, 8.0] {
            mini_jumbo(b, v, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
        }
    }
}

#[test]
fn c4_mini_jumbo_with_spins() {
    let spins = [
        (Vec3::new(0.0, 0.0, 200.0), Vec3::ZERO),
        (Vec3::new(0.0, 150.0, 0.0), Vec3::new(30.0, 0.0, -40.0)),
        (Vec3::new(-100.0, 50.0, 80.0), Vec3::new(0.0, -60.0, 10.0)),
    ];
    for (wm, wj) in spins {
        for &b in &[0.0f32, 0.5] {
            mini_jumbo(b, 3.0, wm, wj, Vec3::new(0.1, -0.2, 0.05));
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, .. ProptestConfig::default() })]

    #[test]
    fn c4_sweep(
        b in 0.0f32..0.95,
        v in 0.2f32..10.0,
        wm in prop::array::uniform3(-300.0f32..300.0),
        wj in prop::array::uniform3(-100.0f32..100.0),
        vj in prop::array::uniform3(-0.5f32..0.5),
    ) {
        mini_jumbo(b, v, Vec3::from(wm), Vec3::from(wj), Vec3::from(vj));
    }
}

// ------------------------------------------------------------ criterion 5

fn spin_drop(name: &str, height: f32, v0: Vec3, w0: Vec3) {
    let world = floor_world();
    let g = geo(&world);
    let mut s = solver();
    let r = profile(name).radius_m();
    let mut b = ball(name, Vec3::new(0.0, 0.0, r + height));
    b.velocity = v0;
    b.angular_velocity = w0;
    let h = s.add_ball(b).unwrap();
    let mut ev = events();
    // 1.5 s to converge, then it must stay rolling without slipping for 1.5 s.
    for _ in 0..1500 {
        ev.clear();
        s.step(&g, DT, &mut ev);
    }
    for tick in 0..1500 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        let b = s.ball(h).unwrap();
        let sl = slip(b, r, Vec3::Z);
        assert!(
            sl < 1e-3,
            "{name} h={height} v0={v0:?} w0={w0:?}: slip {sl} m/s at tick {} (v={:?} w={:?} z={})",
            1500 + tick,
            b.velocity,
            b.angular_velocity,
            b.position.z
        );
        assert!(
            (b.position.z - r).abs() < 1e-4,
            "ball left the floor: z={} r={r}",
            b.position.z
        );
    }
}

#[test]
fn c5_spinning_drop_converges_to_rolling() {
    let cases = [
        (Vec3::ZERO, Vec3::new(0.0, 100.0, 0.0)),
        (Vec3::ZERO, Vec3::new(-80.0, 0.0, 0.0)),
        (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, -100.0, 0.0)), // strong backspin
        (Vec3::new(0.5, 0.5, 0.0), Vec3::new(30.0, 30.0, 20.0)),
        (Vec3::new(-0.3, 0.8, -0.2), Vec3::new(0.0, 0.0, 150.0)),
    ];
    for name in [
        "standard_steel",
        "ceramic_powerball",
        "uv_globall",
        "mini_steel",
    ] {
        for &(v, w) in &cases {
            spin_drop(name, 0.002, v, w);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 8, .. ProptestConfig::default() })]

    #[test]
    fn c5_sweep(
        which in 0usize..4,
        height in 0.0f32..0.01,
        v in prop::array::uniform2(-1.0f32..1.0),
        w in prop::array::uniform3(-150.0f32..150.0),
    ) {
        let name = ["standard_steel", "ceramic_powerball", "jumbo_steel", "mini_steel"][which];
        spin_drop(name, height, Vec3::new(v[0], v[1], 0.0), Vec3::from(w));
    }
}
