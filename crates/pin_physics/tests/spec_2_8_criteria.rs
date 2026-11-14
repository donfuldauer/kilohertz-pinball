//! Subphase 2.8 spec tests (Independent Tester, black box): the two physics
//! verification criteria of PLAN.md 2.8.
//!
//! 1. A 79.4 mm `jumbo_xl_resin` ball against a 15.88 mm `mini_steel` ball
//!    conserves momentum to the 2.2 tolerances (linear 1e-4 relative, total
//!    angular momentum about the system centre of mass 1e-3 relative,
//!    kinetic energy never increases), over impact parameters, approach
//!    directions, closing speeds and spins.
//! 3. No tunnelling for the XL ball against a flipper at maximum angular
//!    velocity: contact positions along the bat and stroke phases are swept,
//!    with the coil torque at its Appendix C maximum and the bat inertia at
//!    its minimum.
//!
//! Only merged public API is used (`pin_physics::ball`, `::flipper`, the
//! 2.2-S `MovingBodies` trait as the surface oracle).

#[path = "spec_ball_common.rs"]
mod bc;
#[path = "spec_2_3_common.rs"]
mod fc;

use fc::{BatFrame, L, Scene};
use glam::{DVec3, Quat, Vec3};
use pin_core::ids::{FlipperId, ProfileId};
use pin_core::{BallProfile, FlipperConfig, PhysicsDefaults};
use pin_physics::ball::{Ball, BallEvent, BallSolver, MovingBodies};
use proptest::prelude::*;

const DT: f32 = 1.0e-3;
const XL: &str = "jumbo_xl_resin";
const MINI: &str = "mini_steel";

// ------------------------------------------------------------ the profile

#[test]
fn xl_profile_matches_plan_3_2() {
    let p = bc::profile(XL);
    assert!((p.diameter_m - 0.0794).abs() < 1e-6, "{}", p.diameter_m);
    assert!((p.radius_m() - 0.0397).abs() < 1e-6);
    assert!((p.mass_kg - 0.445).abs() < 1e-6, "{}", p.mass_kg);
    assert!((p.restitution - 0.80).abs() < 1e-6);
    assert_eq!(p.magnetic_coupling, 0.0);
    assert_eq!(p.remanence, 0.0);
    assert!(!p.conductive);
    assert_eq!(p.magnus_coeff, 0.0);
    let i = 0.4 * f64::from(p.mass_kg) * f64::from(p.radius_m()).powi(2);
    assert!((f64::from(p.moment_of_inertia()) - i).abs() <= 1e-6 * i);
    let m = bc::profile(MINI);
    assert!((m.diameter_m - 0.01588).abs() < 1e-6);
}

// ------------------------------------------------------------ criterion 1

struct TwoBody {
    p: DVec3,
    l: DVec3,
    ke: f64,
}

fn two_body(s: &BallSolver, profs: &[BallProfile]) -> TwoBody {
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
        ke += bc::kinetic(b, pr);
    }
    TwoBody { p, l, ke }
}

/// Worst relative drifts seen in one collision.
#[derive(Copy, Clone, Debug, Default)]
struct Drift {
    dp: f64,
    dl: f64,
    /// Kinetic energy after / before.
    ke_ratio: f64,
}

/// Mini vs XL, zero gravity, no static geometry. `b` is the impact
/// parameter as a fraction of the contact distance, `vrel` the closing
/// speed; `rot` turns the whole scene (approach direction and offset).
fn xl_mini(
    b: f32,
    vrel: f32,
    rot: Quat,
    w_mini: Vec3,
    w_xl: Vec3,
    v_xl: Vec3,
) -> Result<Drift, String> {
    let profs = bc::profiles();
    let mini = bc::profile(MINI);
    let xl = bc::profile(XL);
    let reach = mini.radius_m() + xl.radius_m();
    let gap = 0.02f32;
    let mut s = bc::solver_with_gravity(Vec3::ZERO);
    let mut a = bc::ball(MINI, rot * Vec3::new(-(reach + gap), b * reach, 0.0));
    a.velocity = rot * Vec3::new(vrel, 0.0, 0.0) + v_xl;
    a.angular_velocity = w_mini;
    let mut c = bc::ball(XL, Vec3::ZERO);
    c.velocity = v_xl;
    c.angular_velocity = w_xl;
    let hm = s.add_ball(a).ok_or_else(|| "add_ball refused the ball".to_owned())?;
    let hx = s.add_ball(c).ok_or_else(|| "add_ball refused the ball".to_owned())?;
    let world = bc::empty_world();
    let g = bc::geo(&world);
    let mut ev = bc::events();
    let t0 = two_body(&s, &profs);
    // Characteristic scales so near-zero totals do not make "relative" meaningless.
    let mu = f64::from(mini.mass_kg * xl.mass_kg / (mini.mass_kg + xl.mass_kg));
    let p_scale = t0.p.length().max(mu * f64::from(vrel));
    let l_scale = t0.l.length().max(mu * f64::from(vrel) * f64::from(reach));
    let mut prev_ke = t0.ke;
    let mut collided = false;
    let mut out = Drift::default();
    let ticks = ((reach + gap) / vrel / DT) as usize + 120;
    let tag = format!("b={b}, v={vrel}, rot={rot:?}");
    for tick in 0..ticks {
        ev.clear();
        s.step(&g, DT, &mut ev);
        collided |= ev
            .as_slice()
            .iter()
            .any(|e| matches!(e, BallEvent::Impact { .. }));
        let t = two_body(&s, &profs);
        if !(t.p.is_finite() && t.l.is_finite() && t.ke.is_finite()) {
            return Err(format!("tick {tick}: non-finite state ({tag})"));
        }
        let dp = (t.p - t0.p).length() / p_scale;
        let dl = (t.l - t0.l).length() / l_scale;
        out.dp = out.dp.max(dp);
        out.dl = out.dl.max(dl);
        if dp > 1e-4 {
            return Err(format!("tick {tick}: linear momentum drift {dp:e} ({tag})"));
        }
        if dl > 1e-3 {
            return Err(format!(
                "tick {tick}: angular momentum drift {dl:e} ({tag})"
            ));
        }
        if t.ke > prev_ke * (1.0 + 1e-6) + 1e-12 {
            return Err(format!(
                "tick {tick}: KE rose {prev_ke:e} -> {:e} ({tag})",
                t.ke
            ));
        }
        prev_ke = t.ke;
        // Never interpenetrating by more than 0.1 mm.
        let pm = s.ball(hm).unwrap().position;
        let px = s.ball(hx).unwrap().position;
        let d = (pm - px).length();
        if d < reach - 1e-4 {
            return Err(format!(
                "tick {tick}: balls overlap, centres {d} m apart, contact distance {reach} ({tag})"
            ));
        }
    }
    out.ke_ratio = prev_ke / t0.ke;
    if b < 0.95 {
        if !collided {
            return Err(format!("no impact reported ({tag})"));
        }
        // The balls separate again: the mini ball has bounced off the 27×
        // heavier ball (relative velocity now points apart).
        let m = s.ball(hm).unwrap();
        let x = s.ball(hx).unwrap();
        let sep = (m.position - x.position).dot(m.velocity - x.velocity);
        if sep < 0.0 {
            return Err(format!("balls still closing at the end ({tag})"));
        }
    }
    Ok(out)
}

#[test]
fn c1_xl_vs_mini_head_on_and_glancing_all_speeds() {
    let mut worst = Drift::default();
    let mut n = 0;
    for &b in &[0.0f32, 0.15, 0.3, 0.5, 0.7, 0.9] {
        for &v in &[0.3f32, 1.0, 3.0, 6.0, 10.0, 20.0] {
            let d = xl_mini(b, v, Quat::IDENTITY, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO)
                .unwrap_or_else(|e| panic!("{e}"));
            worst.dp = worst.dp.max(d.dp);
            worst.dl = worst.dl.max(d.dl);
            worst.ke_ratio = worst.ke_ratio.max(d.ke_ratio);
            n += 1;
        }
    }
    println!(
        "c1 plain: {n} collisions, worst dp {:.3e} (<= 1e-4), worst dL {:.3e} (<= 1e-3), max KE ratio {:.6}",
        worst.dp, worst.dl, worst.ke_ratio
    );
}

#[test]
fn c1_xl_vs_mini_impact_angles_in_3d() {
    // Approach directions all over the sphere, offsets out of the xy-plane.
    let mut worst = Drift::default();
    let mut n = 0;
    for k in 0..12 {
        let axis = Vec3::new(
            libm::sinf(k as f32 * 1.3),
            libm::cosf(k as f32 * 0.7),
            libm::sinf(k as f32 * 2.1 + 0.4),
        )
        .normalize();
        let rot = Quat::from_axis_angle(axis, 0.5 + k as f32 * 0.55);
        for &b in &[0.0f32, 0.4, 0.8] {
            for &v in &[1.5f32, 8.0] {
                let d = xl_mini(b, v, rot, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO)
                    .unwrap_or_else(|e| panic!("{e}"));
                worst.dp = worst.dp.max(d.dp);
                worst.dl = worst.dl.max(d.dl);
                n += 1;
            }
        }
    }
    println!(
        "c1 3d: {n} collisions, worst dp {:.3e}, worst dL {:.3e}",
        worst.dp, worst.dl
    );
}

#[test]
fn c1_xl_vs_mini_with_spins_and_moving_xl() {
    let mut worst = Drift::default();
    for &b in &[0.0f32, 0.35, 0.7] {
        for &(wm, wx) in &[
            (Vec3::new(0.0, 0.0, 200.0), Vec3::ZERO),
            (Vec3::ZERO, Vec3::new(0.0, 0.0, -60.0)),
            (Vec3::new(120.0, -80.0, 50.0), Vec3::new(-20.0, 30.0, 45.0)),
        ] {
            for &vx in &[
                Vec3::new(0.2, -0.3, 0.1),
                Vec3::new(-2.5, 0.0, 0.0),
                Vec3::new(4.0, 1.0, -2.0),
            ] {
                let d = xl_mini(b, 3.0, Quat::IDENTITY, wm, wx, vx)
                    .unwrap_or_else(|e| panic!("{e}"));
                worst.dp = worst.dp.max(d.dp);
                worst.dl = worst.dl.max(d.dl);
            }
        }
    }
    println!(
        "c1 spins: worst dp {:.3e}, worst dL {:.3e}",
        worst.dp, worst.dl
    );
}

/// The XL ball is the projectile and the mini ball rests: the mini ball
/// must leave faster than the XL ball arrived (m_xl ≫ m_mini) and the XL
/// ball must barely slow down; momentum bounds as above (the frame change
/// is covered through `v_xl`).
#[test]
fn c1_xl_strikes_resting_mini() {
    for &v in &[0.5f32, 2.0, 6.0, 12.0] {
        // In the XL ball's frame the mini ball approaches along +x at v.
        xl_mini(
            0.0,
            v,
            Quat::IDENTITY,
            Vec3::ZERO,
            Vec3::ZERO,
            Vec3::new(-v, 0.0, 0.0),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        // Explicit check of the outcome in the lab frame.
        let mut s = bc::solver_with_gravity(Vec3::ZERO);
        let reach = bc::profile(MINI).radius_m() + bc::profile(XL).radius_m();
        let hm = s.add_ball(bc::ball(MINI, Vec3::ZERO)).unwrap();
        let mut x = bc::ball(XL, Vec3::new(reach + 0.02, 0.0, 0.0));
        x.velocity = Vec3::new(-v, 0.0, 0.0);
        let hx = s.add_ball(x).unwrap();
        let world = bc::empty_world();
        let g = bc::geo(&world);
        let mut ev = bc::events();
        for _ in 0..((0.02 / v / DT) as usize + 60) {
            ev.clear();
            s.step(&g, DT, &mut ev);
        }
        let vm = s.ball(hm).unwrap().velocity.x;
        let vx = s.ball(hx).unwrap().velocity.x;
        // 1-D collision with restitution e in [0, 1]:
        //   v_mini' = -(1+e) M v/(M+m)  in [-1.93 v, -0.964 v]
        //   v_xl'   = -v (M - e m)/(M+m) in [-v, -0.928 v]
        assert!(
            vm < -0.96 * v && vm > -1.94 * v,
            "v={v}: mini leaves at {vm} m/s"
        );
        assert!(vx <= -0.92 * v && vx >= -v * 1.0001, "v={v}: XL at {vx}");
        assert!(vm < vx, "v={v}: mini ({vm}) not ahead of the XL ball ({vx})");
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, .. ProptestConfig::default() })]

    #[test]
    fn c1_xl_vs_mini_random(
        b in 0.0f32..0.93,
        v in 0.3f32..20.0,
        axis in prop::array::uniform3(-1.0f32..1.0),
        angle in 0.0f32..6.28,
        wm in prop::array::uniform3(-200.0f32..200.0),
        wx in prop::array::uniform3(-60.0f32..60.0),
        vx in prop::array::uniform3(-3.0f32..3.0),
    ) {
        let axis = Vec3::from(axis);
        let rot = if axis.length() > 0.05 {
            Quat::from_axis_angle(axis.normalize(), angle)
        } else {
            Quat::IDENTITY
        };
        let r = xl_mini(b, v, rot, Vec3::from(wm), Vec3::from(wx), Vec3::from(vx));
        prop_assert!(r.is_ok(), "{}", r.unwrap_err());
    }
}

// ------------------------------------------------------------ criterion 3

const ID: FlipperId = FlipperId(0);

fn xl_id() -> ProfileId {
    bc::pid(XL)
}

fn xl_r() -> f32 {
    bc::profile(XL).radius_m()
}

/// Appendix C tunables: (power torque N·m, inertia kg·m²) for the default
/// bat, the strongest coil, and the strongest coil on the lightest bat (the
/// largest angular velocity the tunable ranges allow).
fn drives() -> [(f32, f32); 3] {
    let t = PhysicsDefaults::builtin().tunables;
    [
        (t.flipper_power_torque.default, t.flipper_inertia.default),
        (t.flipper_power_torque.max, t.flipper_inertia.default),
        (t.flipper_power_torque.max, t.flipper_inertia.min),
    ]
}

fn flipper(power: f32, inertia: f32) -> (FlipperConfig, BatFrame) {
    let base = [0.0, 0.0, fc::PIVOT_Z_MM];
    let rest = fc::rest_for(base, Vec3::Z, 50.0, fc::flat_dir(-30.0));
    let mut c = fc::config(0, base, Vec3::Z, rest, 50.0);
    c.power_torque_nm = power;
    c.inertia_kg_m2 = inertia;
    let f = fc::frames(std::slice::from_ref(&c))[0];
    (c, f)
}

/// Unloaded stroke: (angle °, ω rad/s) after each tick, coil on from tick 0.
fn dry_stroke(c: &FlipperConfig) -> Vec<(f32, f32)> {
    let mut s = Scene::new(fc::empty_world(), Vec3::ZERO, std::slice::from_ref(c));
    s.set.set_energised(ID, true);
    (0..80)
        .map(|_| {
            s.advance();
            (s.set.angle_deg(ID), s.set.angular_velocity(ID))
        })
        .collect()
}

fn surface_gap(s: &Scene, p: Vec3) -> f32 {
    s.set
        .closest_point(0, 0.0, p)
        .map_or(f32::INFINITY, |bp| bp.distance)
}

/// Deepest overlap of ball and bat at a tick end that still counts as a
/// contact transient and not as tunnelling, as a fraction of the ball
/// radius. (The 2.3 spec allowed 1 mm for the 13.5 mm ball, 7.4 %; the
/// measured depths are printed and reported.)
const MAX_DEPTH_FRACTION: f32 = 0.05;

#[derive(Copy, Clone, Debug, Default)]
struct FlipOutcome {
    hit: bool,
    /// |ω| of the bat at the start of the first impact tick, rad/s.
    omega_at_impact: f32,
    /// Smallest (centre-to-bat-surface distance − ball radius), m.
    min_gap: f32,
    exit_speed: f32,
}

/// Free space, zero gravity: the coil goes on at tick 0 and the XL ball is
/// placed so that the *unloaded* bat would meet it at stroke fraction
/// `phase`, at `frac` of the bat length from the pivot. `speed` is the
/// ball's speed toward the bat (0 = waiting at rest).
fn free_case(
    name: &str,
    c: &FlipperConfig,
    f: &BatFrame,
    stroke: &[(f32, f32)],
    phase: f32,
    frac: f32,
    speed: f32,
) -> Result<FlipOutcome, String> {
    let r = bc::profile(name).radius_m();
    let sweep = c.sweep_angle_deg;
    let target = c.rest_angle_deg + phase * sweep;
    // Tick at which the unloaded bat reaches the target angle.
    let t_hit = stroke
        .iter()
        .position(|(a, _)| (a - c.rest_angle_deg) * sweep.signum() >= phase * sweep.abs())
        .unwrap_or(stroke.len() - 1);
    let e = f.dir(target);
    let m = f.n.cross(e) * sweep.signum();
    // Bat radius at this station (capsule tapering from base to tip).
    let bat_r = fc::BASE_R + (fc::TIP_R - fc::BASE_R) * frac.clamp(0.0, 1.0);
    let a = f.pivot + e * (frac * L);
    let travel = speed * (t_hit as f32 + 1.0) * DT;
    let start = a + m * (r + bat_r + 0.0005 + travel);
    let mut s = Scene::new(fc::empty_world(), Vec3::ZERO, std::slice::from_ref(c));
    // The start must be clear of the resting bat.
    s.advance();
    if surface_gap(&s, start) < r + 1.0e-3 {
        return Ok(FlipOutcome {
            min_gap: f32::INFINITY,
            ..FlipOutcome::default()
        });
    }
    let mut s = Scene::new(fc::empty_world(), Vec3::ZERO, std::slice::from_ref(c));
    let mut b = Ball::new(bc::pid(name), start);
    b.velocity = -m * speed;
    let h = s
        .solver
        .add_ball(b)
        .ok_or_else(|| "add_ball refused the ball".to_owned())?;
    s.set.set_energised(ID, true);
    let mut out = FlipOutcome {
        min_gap: f32::INFINITY,
        ..FlipOutcome::default()
    };
    let mut prev_side: Option<f32> = None;
    let tag = format!(
        "torque {} inertia {} phase {phase} frac {frac} speed {speed}",
        c.power_torque_nm, c.inertia_kg_m2
    );
    for _ in 0..(t_hit as u64 + 90) {
        let w0 = s.set.angular_velocity(ID);
        s.advance();
        if !out.hit && s.hit(h, 0) {
            out.hit = true;
            out.omega_at_impact = w0.abs();
        }
        let ball = s.ball(h);
        let p = ball.position;
        if !(p.is_finite() && ball.velocity.is_finite() && ball.angular_velocity.is_finite()) {
            return Err(format!("tick {}: non-finite ball ({tag})", s.tick));
        }
        if ball.velocity.length() > 150.0 {
            return Err(format!(
                "tick {}: ball at {} m/s ({tag})",
                s.tick,
                ball.velocity.length()
            ));
        }
        let gap = surface_gap(&s, p) - r;
        out.min_gap = out.min_gap.min(gap);
        if gap < -MAX_DEPTH_FRACTION * r {
            return Err(format!(
                "tick {}: {name} ball {:.3} mm inside the bat ({tag})",
                s.tick,
                -gap * 1e3
            ));
        }
        // Side of the bat: the ball may not change sides while it is level
        // with the bat between the pivot and 90 % of its length (going
        // round the tip takes many ticks outside that range).
        let ang = s.set.angle_deg(ID);
        let (along, across, _) = f.local(ang, Vec3::ZERO, p);
        let now = (0.0..=0.9 * L).contains(&along).then(|| across.signum());
        if let (Some(a0), Some(a1)) = (prev_side, now)
            && a0 != a1
        {
            return Err(format!(
                "tick {}: XL ball crossed the bat at {along} m from the pivot ({tag})",
                s.tick
            ));
        }
        prev_side = now;
        out.exit_speed = ball.velocity.length();
    }
    Ok(out)
}

#[test]
fn c3_no_tunnelling_xl_ball_vs_flipper_position_and_phase_sweep() {
    let mut cases = 0u32;
    let mut hits = 0u32;
    let mut min_gap = f32::INFINITY;
    let mut top_omega_hit = 0.0f32;
    let mut peak_omega = 0.0f32;
    let mut top_exit = 0.0f32;
    for (power, inertia) in drives() {
        let (c, f) = flipper(power, inertia);
        let stroke = dry_stroke(&c);
        let peak = stroke.iter().map(|s| s.1.abs()).fold(0.0f32, f32::max);
        peak_omega = peak_omega.max(peak);
        let mut top_here = 0.0f32;
        for &phase in &[0.05f32, 0.2, 0.4, 0.6, 0.75, 0.85, 0.92, 0.97, 0.995] {
            for &frac in &[0.1f32, 0.25, 0.4, 0.55, 0.7, 0.85, 0.95, 1.0, 1.1] {
                for &speed in &[0.0f32, 1.0, 4.0, 10.0, 25.0] {
                    let o = free_case(XL, &c, &f, &stroke, phase, frac, speed)
                        .unwrap_or_else(|e| panic!("{e}"));
                    cases += 1;
                    hits += u32::from(o.hit);
                    min_gap = min_gap.min(o.min_gap);
                    if o.hit {
                        top_here = top_here.max(o.omega_at_impact);
                        top_exit = top_exit.max(o.exit_speed);
                    }
                }
            }
        }
        println!(
            "c3 torque {power} N·m inertia {inertia:e}: unloaded peak ω {peak:.1} rad/s \
             (tip {:.2} m/s), fastest bat at an XL impact {top_here:.1} rad/s",
            peak * L
        );
        // The sweep really reaches the bat's maximum angular velocity.
        assert!(
            top_here >= 0.9 * peak,
            "torque {power}: fastest impact at {top_here} rad/s, unloaded peak {peak}"
        );
        top_omega_hit = top_omega_hit.max(top_here);
    }
    println!(
        "c3 free space: {hits}/{cases} cases hit, deepest tick-end overlap {:.4} mm, max ω at impact {top_omega_hit:.1} \
         rad/s (overall unloaded peak {peak_omega:.1}), fastest exit {top_exit:.2} m/s",
        -min_gap * 1e3
    );
    assert!(
        hits * 10 >= cases * 7,
        "scenario mostly misses: {hits}/{cases}"
    );
    // Reference for the report: the same sweep with the 27 mm ball.
    let (power, inertia) = drives()[2];
    let (c, f) = flipper(power, inertia);
    let stroke = dry_stroke(&c);
    let mut std_gap = f32::INFINITY;
    for &phase in &[0.05f32, 0.2, 0.4, 0.6, 0.75, 0.85, 0.92, 0.97, 0.995] {
        for &frac in &[0.1f32, 0.25, 0.4, 0.55, 0.7, 0.85, 0.95, 1.0, 1.1] {
            for &speed in &[0.0f32, 1.0, 4.0, 10.0, 25.0] {
                let o = free_case("standard_steel", &c, &f, &stroke, phase, frac, speed)
                    .unwrap_or_else(|e| panic!("27 mm reference: {e}"));
                std_gap = std_gap.min(o.min_gap);
            }
        }
    }
    println!(
        "c3 reference: 27 mm ball, strongest drive, deepest tick-end overlap {:.4} mm",
        -std_gap * 1e3
    );
}

/// On the pitched table: the XL ball rolls down the playfield into the
/// flipper, which fires at a swept time; the ball must never be inside the
/// bat nor below the playfield, whatever the timing and aim.
fn table_case(
    c: &FlipperConfig,
    f: &BatFrame,
    frac: f32,
    flip: u64,
    v0: f32,
    held: bool,
) -> Result<FlipOutcome, String> {
    let r = xl_r();
    let mut s = Scene::new(
        fc::table_world(Quat::IDENTITY),
        fc::table_gravity(Quat::IDENTITY),
        std::slice::from_ref(c),
    );
    let aim_dir = if held {
        f.dir(c.rest_angle_deg + c.sweep_angle_deg)
    } else {
        f.e_rest
    };
    let aim = f.pivot + aim_dir * (frac * L);
    let mut b = Ball::new(xl_id(), Vec3::new(aim.x, aim.y + 0.25, r));
    b.velocity = Vec3::new(0.0, -v0, 0.0);
    // Rolling without slipping down the table.
    b.angular_velocity = Vec3::new(v0 / r, 0.0, 0.0);
    let h = s
        .solver
        .add_ball(b)
        .ok_or_else(|| "add_ball refused the ball".to_owned())?;
    if held {
        s.set.set_energised(ID, true);
    }
    let mut out = FlipOutcome {
        min_gap: f32::INFINITY,
        ..FlipOutcome::default()
    };
    let tag = format!(
        "torque {} frac {frac} flip {flip} v0 {v0} held {held}",
        c.power_torque_nm
    );
    for _ in 0..450u64 {
        if s.tick == flip {
            s.set.set_energised(ID, !held);
        }
        if s.tick == flip + 40 {
            s.set.set_energised(ID, true);
        }
        let w0 = s.set.angular_velocity(ID);
        s.advance();
        if !out.hit && s.hit(h, 0) {
            out.hit = true;
            out.omega_at_impact = w0.abs();
        }
        let ball = s.ball(h);
        let p = ball.position;
        if !(p.is_finite() && ball.velocity.is_finite()) {
            return Err(format!("tick {}: non-finite ball ({tag})", s.tick));
        }
        let gap = surface_gap(&s, p) - r;
        out.min_gap = out.min_gap.min(gap);
        if gap < -MAX_DEPTH_FRACTION * r {
            return Err(format!(
                "tick {}: XL ball {:.3} mm inside the bat ({tag})",
                s.tick,
                -gap * 1e3
            ));
        }
        // The playfield slab is 1.2 m × 1.6 m; a ball launched off the bat
        // may leave it.
        if p.x.abs() > 0.6 - r || p.y.abs() > 0.8 - r {
            break;
        }
        if p.z < r - 1.0e-3 {
            return Err(format!(
                "tick {}: XL ball centre {:.3} mm above the playfield, radius {:.1} mm ({tag})",
                s.tick,
                p.z * 1e3,
                r * 1e3
            ));
        }
        if ball.velocity.length() > 60.0 {
            return Err(format!(
                "tick {}: ball at {} m/s ({tag})",
                s.tick,
                ball.velocity.length()
            ));
        }
    }
    Ok(out)
}

#[test]
fn c3_no_tunnelling_xl_ball_on_the_table_flip_timing_sweep() {
    let (mut cases, mut hits) = (0u32, 0u32);
    let mut min_gap = f32::INFINITY;
    let mut top = 0.0f32;
    for (power, inertia) in drives() {
        let (c, f) = flipper(power, inertia);
        for &frac in &[0.2f32, 0.5, 0.8, 1.0] {
            for &v0 in &[0.0f32, 1.5, 4.0] {
                for flip in (40u64..=260).step_by(11) {
                    for held in [false, true] {
                        let o = table_case(&c, &f, frac, flip, v0, held)
                            .unwrap_or_else(|e| panic!("{e}"));
                        cases += 1;
                        hits += u32::from(o.hit);
                        min_gap = min_gap.min(o.min_gap);
                        top = top.max(o.omega_at_impact);
                    }
                }
            }
        }
    }
    println!(
        "c3 table: {hits}/{cases} cases hit the bat, deepest tick-end overlap {:.4} mm, fastest bat at first impact {top:.1} rad/s",
        -min_gap * 1e3
    );
    assert!(hits * 2 >= cases, "scenario mostly misses: {hits}/{cases}");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, .. ProptestConfig::default() })]

    #[test]
    fn c3_no_tunnelling_xl_vs_flipper_random(
        drive in 0usize..3,
        phase in 0.02f32..1.0,
        frac in 0.05f32..1.15,
        speed in 0.0f32..30.0,
    ) {
        let (power, inertia) = drives()[drive];
        let (c, f) = flipper(power, inertia);
        let stroke = dry_stroke(&c);
        let r = free_case(XL, &c, &f, &stroke, phase, frac, speed);
        prop_assert!(r.is_ok(), "{}", r.unwrap_err());
    }
}
