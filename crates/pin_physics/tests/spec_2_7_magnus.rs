//! Independent spec tests, Subphase 2.7, Magnus effect (PLAN.md §3.2:
//! `F_magnus = S_m (ω × v)`, `S_m = magnus_coeff`; verification 2).
//!
//! Black box: written from the brief, against `pin_physics::ball_ext` and the
//! public `BallSolver` API. The reference is an `f64` integration of
//! `dv/dt = g + (S_m/m) ω × v` written here.

#[path = "spec_2_7_common.rs"]
mod common;

use common::*;
use glam::{DVec3, Vec3};
use pin_core::{BallProfile, ProfileId};
use pin_physics::ball::{
    Ball, BallEvents, BallHandle, BallSolver, ExternalForce, ForceContext, Geometry,
};
use proptest::prelude::*;

/// Which hooks a free-flight run registers.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Reg {
    None,
    All,
    MagnusOnly,
}

/// Free flight of one ball of `profile` (no geometry): the state after every
/// tick.
fn flight(
    profile: &BallProfile,
    gravity: Vec3,
    v0: Vec3,
    w0: Vec3,
    ticks: usize,
    reg: Reg,
) -> Vec<Ball> {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let profiles = [profile.clone()];
    let mut s: BallSolver = solver(&profiles, gravity);
    match reg {
        Reg::None => {}
        Reg::All => {
            let _ = register(&mut s);
        }
        Reg::MagnusOnly => register_magnus(&mut s),
    }
    let mut b = Ball::new(ProfileId(0), Vec3::new(0.0, 0.0, 1.0));
    b.velocity = v0;
    b.angular_velocity = w0;
    let h = s.add_ball(b).unwrap();
    let mut ev = BallEvents::new();
    let mut out = Vec::with_capacity(ticks);
    for _ in 0..ticks {
        ev.clear();
        s.step(&geo, DT, &mut ev);
        out.push(*s.ball(h).unwrap());
    }
    out
}

/// `f64` reference: velocity after `t` seconds of
/// `dv/dt = g + (S/m) ω × v` (midpoint rule, 10 µs steps).
fn reference_velocity(s_over_m: f64, g: DVec3, w: DVec3, v0: DVec3, t: f64) -> DVec3 {
    let n = (t / 1.0e-5).round() as usize;
    let h = t / n as f64;
    let acc = |v: DVec3| g + s_over_m * w.cross(v);
    let mut v = v0;
    for _ in 0..n {
        let mid = v + 0.5 * h * acc(v);
        v += h * acc(mid);
    }
    v
}

fn bits_equal(a: &[Ball], b: &[Ball]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| state_bits(x) == state_bits(y))
}

/// Checks the mean acceleration over `ticks` against the reference, within
/// 1 % of `S |ω × v0| / m` (plus the `f32` resolution of the velocity).
fn check_lateral(profile: &BallProfile, gravity: Vec3, v0: Vec3, w0: Vec3, ticks: usize, reg: Reg) {
    let run = flight(profile, gravity, v0, w0, ticks, reg);
    let t = ticks as f64 * f64::from(DT);
    let s_over_m = f64::from(profile.magnus_coeff) / f64::from(profile.mass_kg);
    let (g, w, v) = (gravity.as_dvec3(), w0.as_dvec3(), v0.as_dvec3());
    let end = run.last().unwrap();
    // Acceleration with gravity's own contribution removed (it is the same
    // in the reference).
    let a_meas = (end.velocity.as_dvec3() - v - g * t) / t;
    let a_ref = (reference_velocity(s_over_m, g, w, v, t) - v - g * t) / t;
    let a_nominal = s_over_m * w.cross(v).length();
    let v_max = v.length() + g.length() * t;
    // f32 resolution of a velocity difference over t (ulp of the largest
    // component, both ends, plus accumulated rounding of the gravity adds).
    let floor = (4.0 + ticks as f64) * f64::from(f32::EPSILON) * v_max / t;
    let err = (a_meas - a_ref).length();
    assert!(
        err <= 0.01 * a_nominal + floor,
        "S={} m={} v0={v0:?} w0={w0:?} g={gravity:?}: measured {a_meas:?}, expected {a_ref:?} \
         (|a| nominal {a_nominal}, error {err}, floor {floor})",
        profile.magnus_coeff,
        profile.mass_kg
    );
    // The spin is not changed by the Magnus force (it is a force, not a torque).
    assert_eq!(end.angular_velocity, w0, "spin changed in free flight");
}

const SPINS: [Vec3; 4] = [
    Vec3::new(0.0, 0.0, 200.0),
    Vec3::new(0.0, 150.0, 0.0),
    Vec3::new(30.0, -40.0, 120.0),
    Vec3::new(-250.0, 10.0, 60.0),
];
const VELS: [Vec3; 4] = [
    Vec3::new(2.0, 0.0, 0.0),
    Vec3::new(1.0, 1.0, 0.5),
    Vec3::new(0.0, -3.0, 1.0),
    Vec3::new(-0.7, 0.2, 4.0),
];

// ------------------------------------------------------------ verification 2

#[test]
fn v2_globall_lateral_acceleration_within_1_percent_no_gravity() {
    let p = profile("uv_globall");
    assert_eq!(p.magnus_coeff, 1.0e-5);
    for w in SPINS {
        for v in VELS {
            for reg in [Reg::All, Reg::MagnusOnly] {
                check_lateral(&p, Vec3::ZERO, v, w, 100, reg);
            }
        }
    }
}

#[test]
fn v2_globall_lateral_acceleration_first_tick() {
    // The textbook case: ω = +z, v = +x → a = (S/m) ω × v = +y, 0.1667 m/s².
    let p = profile("uv_globall");
    let w = Vec3::new(0.0, 0.0, 200.0);
    let v = Vec3::new(2.0, 0.0, 0.0);
    let run = flight(&p, Vec3::ZERO, v, w, 10, Reg::All);
    let expect = f64::from(p.magnus_coeff) * 400.0 / f64::from(p.mass_kg);
    assert!((expect - 1.0e-5 * 400.0 / 0.024).abs() < 1e-6);
    let a1 = f64::from(run[0].velocity.y) / f64::from(DT);
    assert!(
        (a1 - expect).abs() <= 0.01 * expect,
        "first tick: {a1} vs {expect}"
    );
    let a10 = f64::from(run[9].velocity.y) / (10.0 * f64::from(DT));
    assert!(
        (a10 - expect).abs() <= 0.01 * expect,
        "ten ticks: {a10} vs {expect}"
    );
    // Sign: along +ω × v, not −.
    assert!(run[9].velocity.y > 0.0);
    assert_eq!(run[9].velocity.z, 0.0);
}

#[test]
fn v2_lateral_acceleration_in_free_fall() {
    let p = profile("uv_globall");
    for w in SPINS {
        for v in VELS {
            check_lateral(&p, G, v, w, 100, Reg::All);
        }
    }
}

#[test]
fn v2_lateral_acceleration_scales_with_coefficient_and_mass() {
    // PhysicsDefaults bounds of the Glo-Ball coefficient: 0–5e-5 kg.
    for s in [2.0e-6f32, 5.0e-6, 1.0e-5, 2.5e-5, 5.0e-5] {
        let p = globall_with(s);
        for (w, v) in SPINS.into_iter().zip(VELS) {
            check_lateral(&p, Vec3::ZERO, v, w, 100, Reg::All);
        }
    }
    // Other masses and radii: the force depends on S, ω and v only.
    for name in [
        "standard_steel",
        "mini_steel",
        "jumbo_steel",
        "jumbo_xl_resin",
    ] {
        let mut p = profile(name);
        p.magnus_coeff = 4.0e-5;
        for (w, v) in SPINS.into_iter().zip(VELS) {
            check_lateral(&p, Vec3::ZERO, v, w, 100, Reg::All);
            check_lateral(&p, G, v, w, 50, Reg::All);
        }
    }
}

/// Bound used below, documented: over N ticks of h = 1 ms without gravity the
/// run without Magnus keeps its velocity bit for bit; with Magnus the
/// translational kinetic energy may differ by
/// * the second-order term of adding a perpendicular `F·h/m` to `v`
///   (`|v + a h|² = |v|²(1 + (ω_c h)²)`, `ω_c = S|ω|/m`): `N (ω_c h)²`
///   relative, 7e-6 for the stock Glo-Ball at 200 rad/s over 1 s; and
/// * `f32` rounding of N velocity updates, at worst `2 N ε/2` relative
///   (1.2e-4 for N = 1000, typically a few 1e-6).
fn ke_bound(s_over_m: f64, w: f64, ticks: usize) -> f64 {
    let n = ticks as f64;
    let wc_h = s_over_m * w * f64::from(DT);
    2.0 * n * wc_h * wc_h + n * f64::from(f32::EPSILON)
}

#[test]
fn v2_kinetic_energy_unchanged_by_magnus() {
    let p = profile("uv_globall");
    let m = f64::from(p.mass_kg);
    let s_over_m = f64::from(p.magnus_coeff) / m;
    let ticks = 1000;
    let mut worst = 0.0f64;
    for w in SPINS {
        for v in VELS {
            let with = flight(&p, Vec3::ZERO, v, w, ticks, Reg::All);
            let without = flight(&p, Vec3::ZERO, v, w, ticks, Reg::None);
            // The comparison is not vacuous: the paths differ.
            let dev = (with[ticks - 1].position - without[ticks - 1].position).length();
            assert!(dev > 1.0e-3, "Magnus did not deflect the ball ({dev} m)");
            let bound = ke_bound(s_over_m, f64::from(w.length()), ticks);
            assert!(bound < 1.5e-4);
            for (a, b) in with.iter().zip(&without) {
                let ka = 0.5 * m * a.velocity.as_dvec3().length_squared();
                let kb = 0.5 * m * b.velocity.as_dvec3().length_squared();
                let rel = (ka - kb).abs() / kb;
                worst = worst.max(rel);
                assert!(
                    rel <= bound,
                    "v0={v:?} w0={w:?}: KE {ka} vs {kb} (rel {rel}, bound {bound})"
                );
                // Rotational energy: the spin is untouched.
                assert_eq!(a.angular_velocity, b.angular_velocity);
            }
        }
    }
    println!("worst relative KE difference with/without Magnus over 1 s: {worst:e}");
}

#[test]
fn magnus_turns_velocity_on_a_circle() {
    // v ⟂ ω: uniform circular motion at ω_c = S|ω|/m about ω̂, radius |v|/ω_c.
    let p = globall_with(5.0e-5);
    let w = Vec3::new(0.0, 0.0, 400.0);
    let v = Vec3::new(1.0, 0.0, 0.0);
    let ticks = 2000;
    let run = flight(&p, Vec3::ZERO, v, w, ticks, Reg::All);
    let wc = f64::from(p.magnus_coeff) * 400.0 / f64::from(p.mass_kg);
    let t = ticks as f64 * f64::from(DT);
    let end = run[ticks - 1];
    let angle = f64::from(end.velocity.y).atan2(f64::from(end.velocity.x));
    assert!(
        (angle - wc * t).abs() <= 0.01 * wc * t,
        "turned {angle} rad, expected {}",
        wc * t
    );
    let speed = f64::from(end.velocity.length());
    assert!((speed - 1.0).abs() < 3.0e-3, "speed {speed}");
    assert_eq!(end.velocity.z, 0.0);
    // Centre of the circle: start + R (ω̂ × v̂).
    let radius = 1.0 / wc;
    let centre = DVec3::new(0.0, radius, 1.0);
    for b in &run {
        let d = (b.position.as_dvec3() - centre).length();
        assert!((d - radius).abs() <= 0.01 * radius, "radius {d} vs {radius}");
    }
}

// ------------------------------------------------- zero force: bit identity

#[test]
fn no_force_when_spin_or_velocity_is_zero_or_parallel() {
    let p = globall_with(5.0e-5);
    let cases = [
        (Vec3::new(2.0, -1.0, 0.5), Vec3::ZERO),
        (Vec3::ZERO, Vec3::new(0.0, 300.0, 10.0)),
        (Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, 100.0)),
        (Vec3::new(-2.0, 0.0, 0.0), Vec3::new(64.0, 0.0, 0.0)),
    ];
    for (v, w) in cases {
        let with = flight(&p, Vec3::ZERO, v, w, 200, Reg::All);
        let without = flight(&p, Vec3::ZERO, v, w, 200, Reg::None);
        assert!(bits_equal(&with, &without), "v={v:?} w={w:?}");
    }
}

#[test]
fn zero_coefficient_is_bit_identical_whatever_the_profile_name() {
    // Rule 6: the hook reads the coefficient, not the profile name.
    for name in pin_core::profiles::BUILTIN_PROFILE_NAMES {
        let mut p = profile(name);
        p.magnus_coeff = 0.0;
        for g in [Vec3::ZERO, G] {
            for (w, v) in SPINS.into_iter().zip(VELS) {
                let with = flight(&p, g, v, w, 300, Reg::All);
                let without = flight(&p, g, v, w, 300, Reg::None);
                assert!(bits_equal(&with, &without), "{name} g={g:?}");
                let only = flight(&p, g, v, w, 300, Reg::MagnusOnly);
                assert!(bits_equal(&only, &without), "{name} g={g:?} (Magnus only)");
            }
        }
    }
}

#[test]
fn coefficient_acts_whatever_the_profile_name() {
    for name in pin_core::profiles::BUILTIN_PROFILE_NAMES {
        let mut p = profile(name);
        p.magnus_coeff = 3.0e-5;
        check_lateral(&p, Vec3::ZERO, VELS[1], SPINS[2], 100, Reg::All);
    }
}

// ------------------------------------------------------- the hook, directly

fn ctx(magnets: &[pin_physics::ball::MagnetState]) -> ForceContext<'_> {
    ForceContext {
        magnets,
        step_index: 0,
        tick_dt: DT,
        tick_time: 0.0,
        sim_time: 0.0,
        substep: 0,
        substeps: 1,
    }
}

fn direct_force(p: &BallProfile, v: Vec3, w: Vec3) -> Vec3 {
    let mut hook = magnus_hook();
    let mut b = Ball::new(ProfileId(0), Vec3::new(0.1, 0.2, 0.3));
    b.velocity = v;
    b.angular_velocity = w;
    hook.ball_added(BallHandle(0), &b);
    hook.force_in(&ctx(&[]), BallHandle(0), &b, p, DT)
}

#[test]
fn hook_force_is_s_omega_cross_v() {
    let p = profile("uv_globall");
    let f = direct_force(&p, Vec3::new(2.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 200.0));
    assert!((f - Vec3::new(0.0, 4.0e-3, 0.0)).length() < 4.0e-8, "{f:?}");
    let mut zero = p.clone();
    zero.magnus_coeff = 0.0;
    let f = direct_force(&zero, Vec3::new(2.0, 1.0, -3.0), Vec3::new(50.0, 0.0, 200.0));
    assert_eq!(f, Vec3::ZERO);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    /// The force helper: `F = S (ω × v)` to rounding, perpendicular to `v`
    /// and to `ω`, finite, linear in `S`.
    #[test]
    fn prop_hook_force(
        s in 0.0f32..5.0e-5,
        v in prop::array::uniform3(-10.0f32..10.0),
        w in prop::array::uniform3(-600.0f32..600.0),
        which in 0usize..7,
    ) {
        let mut p = builtin()[which].clone();
        p.magnus_coeff = s;
        let (v, w) = (Vec3::from(v), Vec3::from(w));
        let f = direct_force(&p, v, w);
        prop_assert!(f.is_finite());
        let expect = f64::from(s) * w.as_dvec3().cross(v.as_dvec3());
        let scale = f64::from(s) * f64::from(w.length()) * f64::from(v.length());
        let tol = 1.0e-5 * scale + 1.0e-12;
        prop_assert!((f.as_dvec3() - expect).length() <= tol, "{:?} vs {:?}", f, expect);
        // No work: F · v = 0 (to rounding); no component along ω either.
        prop_assert!(f.as_dvec3().dot(v.as_dvec3()).abs() <= tol * f64::from(v.length()));
        prop_assert!(f.as_dvec3().dot(w.as_dvec3()).abs() <= tol * f64::from(w.length()));
        // Stateless: the same call again gives the same bits.
        let again = direct_force(&p, v, w);
        prop_assert_eq!(f.to_array().map(f32::to_bits), again.to_array().map(f32::to_bits));
    }

    #[test]
    fn prop_zero_coefficient_gives_exactly_zero(
        v in prop::array::uniform3(-10.0f32..10.0),
        w in prop::array::uniform3(-600.0f32..600.0),
        which in 0usize..7,
    ) {
        let mut p = builtin()[which].clone();
        p.magnus_coeff = 0.0;
        prop_assert_eq!(direct_force(&p, Vec3::from(v), Vec3::from(w)), Vec3::ZERO);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, .. ProptestConfig::default() })]

    /// Free flight through the solver: lateral acceleration within 1 %, and
    /// the Magnus term does no work.
    #[test]
    fn prop_free_flight(
        s in 1.0e-6f32..5.0e-5,
        v in prop::array::uniform3(-4.0f32..4.0),
        w in prop::array::uniform3(-300.0f32..300.0),
        which in 0usize..7,
        gravity in any::<bool>(),
    ) {
        let mut p = builtin()[which].clone();
        p.magnus_coeff = s;
        let (v, w) = (Vec3::from(v), Vec3::from(w));
        prop_assume!(v.length() > 0.2);
        let g = if gravity { G } else { Vec3::ZERO };
        check_lateral(&p, g, v, w, 40, Reg::All);
        if !gravity {
            let ticks = 200;
            let run = flight(&p, g, v, w, ticks, Reg::All);
            let k0 = v.as_dvec3().length_squared();
            let k1 = run[ticks - 1].velocity.as_dvec3().length_squared();
            let bound = ke_bound(
                f64::from(s) / f64::from(p.mass_kg),
                f64::from(w.length()),
                ticks,
            );
            prop_assert!(((k1 - k0) / k0).abs() <= bound, "{} vs {} (bound {})", k1, k0, bound);
        }
    }
}
