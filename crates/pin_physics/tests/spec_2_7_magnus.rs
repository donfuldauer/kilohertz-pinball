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
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// Which hooks a free-flight run registers.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Reg {
    None,
    All,
    MagnusOnly,
}

/// Counts the solver's force calls, i.e. the ball sub-steps (the documented
/// `ExternalForce` contract: `force_in` is called once per ball sub-step and
/// the result times `h / m` is added to the velocity). It returns `−0.0` on
/// every axis: `x + (−0.0) == x` bit for bit for every `x`, signed zeros
/// included (a `+0.0` force would turn a `−0.0` velocity component into
/// `+0.0`), so counting does not change the run
/// (`substep_counter_does_not_change_the_run`).
struct SubstepCounter(Arc<AtomicU64>);

impl ExternalForce for SubstepCounter {
    fn force(&mut self, _: BallHandle, _: &Ball, _: &BallProfile, _: f32) -> Vec3 {
        self.0.fetch_add(1, Ordering::Relaxed);
        Vec3::splat(-0.0)
    }
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
    flight_impl(profile, gravity, v0, w0, ticks, reg, false).0
}

/// [`flight`] with a [`SubstepCounter`] registered after the hooks of `reg`:
/// the states (bit-identical to [`flight`]) and the number of ball sub-steps
/// completed up to and including each tick.
fn flight_counted(
    profile: &BallProfile,
    gravity: Vec3,
    v0: Vec3,
    w0: Vec3,
    ticks: usize,
    reg: Reg,
) -> (Vec<Ball>, Vec<u64>) {
    flight_impl(profile, gravity, v0, w0, ticks, reg, true)
}

fn flight_impl(
    profile: &BallProfile,
    gravity: Vec3,
    v0: Vec3,
    w0: Vec3,
    ticks: usize,
    reg: Reg,
    count: bool,
) -> (Vec<Ball>, Vec<u64>) {
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
    let calls = Arc::new(AtomicU64::new(0));
    if count {
        s.add_external_force(Box::new(SubstepCounter(calls.clone())));
    }
    let mut b = Ball::new(ProfileId(0), Vec3::new(0.0, 0.0, 1.0));
    b.velocity = v0;
    b.angular_velocity = w0;
    let h = s.add_ball(b).unwrap();
    let mut ev = BallEvents::new();
    let mut out = Vec::with_capacity(ticks);
    let mut substeps = Vec::with_capacity(ticks);
    for _ in 0..ticks {
        ev.clear();
        s.step(&geo, DT, &mut ev);
        out.push(*s.ball(h).unwrap());
        substeps.push(calls.load(Ordering::Relaxed));
    }
    (out, substeps)
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
///
/// The resolution floor counts ball sub-steps, not ticks (GR-1): the solver
/// adds the gravity kick and then the hook's increment to the `f32` velocity
/// once per sub-step, each add is off by at most half an ulp per component
/// (`≤ ε|v|/2` as a vector), and because the increments barely change from
/// one sub-step to the next the errors add up instead of averaging out
/// (plain free fall at 4.6 m/s, no hook: 0.4 ulp per add, all one way).
fn check_lateral(profile: &BallProfile, gravity: Vec3, v0: Vec3, w0: Vec3, ticks: usize, reg: Reg) {
    let (run, n_sub) = flight_counted(profile, gravity, v0, w0, ticks, reg);
    let n_sub = n_sub[ticks - 1];
    assert!(n_sub >= ticks as u64, "{n_sub} sub-steps in {ticks} ticks");
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
    // component, both ends, plus accumulated rounding of two adds per
    // sub-step, gravity and hook, ε|v|/2 each).
    let floor = (4.0 + n_sub as f64) * f64::from(f32::EPSILON) * v_max / t;
    let err = (a_meas - a_ref).length();
    assert!(
        err <= 0.01 * a_nominal + floor,
        "S={} m={} v0={v0:?} w0={w0:?} g={gravity:?}: measured {a_meas:?}, expected {a_ref:?} \
         (|a| nominal {a_nominal}, error {err}, floor {floor}, {n_sub} sub-steps)",
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
/// translational kinetic energy may differ, relative to `|v|²`, by
/// * the second-order term of adding a perpendicular `F·h/m` to `v`
///   (`|v + a h|² = |v|²(1 + (ω_c h)²)`, `ω_c = S|ω|/m`, `φ = ω_c h`):
///   `N φ²`, allowed twice (7e-6 for the stock Glo-Ball at 200 rad/s over
///   1 s; less when the tick is split into sub-steps or the hook rotates
///   `v` exactly); and
/// * `f32` rounding of the velocity updates. The solver adds the hook's
///   increment to the `f32` velocity once per ball **sub-step** (the
///   `ExternalForce` contract), not once per tick, and a fast ball takes
///   several sub-steps per tick. One add `v_i ← fl(v_i + d_i)` is off by at
///   most half an ulp, `|e_i| ≤ ε|v_i|/2`, so it changes `|v|²` by
///   `Σ 2 v_i e_i ≤ ε|v|²`: `ε` relative per add, `N_sub ε` for the run
///   (`n_sub` sub-steps, counted by [`SubstepCounter`]). The increments of
///   successive sub-steps are almost equal, so the errors are correlated and
///   a run can come close to this worst case (0.95 of it seen in 20 000
///   random cases; GR-1) instead of the √N of a random walk. The increment
///   itself, `fl(F h/m)` with `F` an `f32` cross product, is off by a few
///   `ε S|ω||v| h/m = ε φ|v|` in any direction, which is `≤ 8 ε φ` relative
///   work per add.
///
/// What the bound still excludes (the property: the Magnus term does no
/// work): a force with a fraction `α` of `S|ω × v|` along `v` changes `|v|²`
/// by `2 α φ` per tick, so over 200 ticks it is caught when
/// `α ⪆ N_sub ε / (400 φ)`: `α = 1e-3` fails 28 % of `prop_free_flight`'s
/// inputs, `α = 1e-2` 91 % (GR-1 throwaway test, DECISIONS "2.7 — spec
/// prop_free_flight bound counts sub-steps (GR-1)").
fn ke_bound(s_over_m: f64, w: f64, ticks: usize, n_sub: u64) -> f64 {
    let n = ticks as f64;
    let phi = s_over_m * w * f64::from(DT);
    2.0 * n * phi * phi + n_sub as f64 * f64::from(f32::EPSILON) * (1.0 + 8.0 * phi)
}

#[test]
fn substep_counter_does_not_change_the_run() {
    let p = globall_with(3.0e-5);
    let steel = {
        let mut p = profile("standard_steel");
        p.magnus_coeff = 3.0e-5;
        p
    };
    let max_substeps = u64::from(config(G).max_substeps);
    let cases = [
        // Signed zeros in the velocity, at rest, slow, and fast enough for
        // several sub-steps per tick.
        (Vec3::new(-0.0, 2.0, 0.0), Vec3::new(0.0, 0.0, 200.0)),
        (Vec3::new(-0.0, -0.0, -0.0), Vec3::new(0.0, 100.0, 0.0)),
        (Vec3::new(1.0, -3.0, 0.5), Vec3::new(30.0, -40.0, 120.0)),
        (Vec3::new(8.0, 9.0, -0.0), Vec3::new(-0.0, 50.0, 0.0)),
    ];
    let ticks = 300;
    let mut several = false;
    for p in [&p, &steel] {
        for g in [Vec3::ZERO, G] {
            for reg in [Reg::None, Reg::All, Reg::MagnusOnly] {
                for (v, w) in cases {
                    let plain = flight(p, g, v, w, ticks, reg);
                    let (counted, n) = flight_counted(p, g, v, w, ticks, reg);
                    assert!(bits_equal(&plain, &counted), "g={g:?} {reg:?} v={v:?}");
                    // At least one sub-step per tick, at most the solver's cap.
                    let mut before = 0;
                    for &c in &n {
                        assert!((1..=max_substeps).contains(&(c - before)));
                        before = c;
                    }
                    several |= n[ticks - 1] > ticks as u64;
                    // Hooks or not, the same number of sub-steps here.
                    let (_, bare) = flight_counted(p, g, v, w, ticks, Reg::None);
                    if g == Vec3::ZERO {
                        assert_eq!(n, bare);
                    }
                }
            }
        }
    }
    assert!(several, "no case with more than one sub-step per tick");
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
            let (with, n_sub) = flight_counted(&p, Vec3::ZERO, v, w, ticks, Reg::All);
            let without = flight(&p, Vec3::ZERO, v, w, ticks, Reg::None);
            // The comparison is not vacuous: the paths differ.
            let dev = (with[ticks - 1].position - without[ticks - 1].position).length();
            assert!(dev > 1.0e-3, "Magnus did not deflect the ball ({dev} m)");
            // At most two sub-steps per tick at these speeds (≤ 4.1 m/s).
            assert!(n_sub[ticks - 1] <= 2 * ticks as u64);
            assert!(ke_bound(s_over_m, f64::from(w.length()), ticks, n_sub[ticks - 1]) < 3.0e-4);
            for (i, (a, b)) in with.iter().zip(&without).enumerate() {
                let bound = ke_bound(s_over_m, f64::from(w.length()), i + 1, n_sub[i]);
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

/// GR-1: the shrunk input on which the per-tick rounding allowance failed
/// (saved seed `7354f3a1…` replays the unshrunk case). The ball takes more
/// than one sub-step per tick, the drift is within the per-sub-step bound,
/// and without Magnus the velocity does not change at all, so the drift is
/// rounding of the Magnus adds.
#[test]
fn gr1_fast_ball_energy_drift_is_within_the_substep_bound() {
    let mut p = profile("standard_steel");
    p.magnus_coeff = 2.793_942_3e-6;
    let v = Vec3::new(-2.270_528_8, 1.038_747_9, 2.491_711_6);
    let w = Vec3::new(-171.763_29, 125.194_756, 150.648_48);
    let ticks = 200;
    let (run, n_sub) = flight_counted(&p, Vec3::ZERO, v, w, ticks, Reg::All);
    let n_sub = n_sub[ticks - 1];
    assert!(n_sub > ticks as u64, "{n_sub} sub-steps");
    let k0 = v.as_dvec3().length_squared();
    let rel = ((run[ticks - 1].velocity.as_dvec3().length_squared() - k0) / k0).abs();
    let s_over_m = f64::from(p.magnus_coeff) / f64::from(p.mass_kg);
    let bound = ke_bound(s_over_m, f64::from(w.length()), ticks, n_sub);
    assert!(rel <= bound, "relative KE change {rel} (bound {bound})");
    println!(
        "GR-1 input: {n_sub} sub-steps, relative KE change {rel:e} = {:.3} of the bound",
        rel / bound
    );
    let without = flight(&p, Vec3::ZERO, v, w, ticks, Reg::None);
    assert_eq!(without[ticks - 1].velocity, v);
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
        assert!(
            (d - radius).abs() <= 0.01 * radius,
            "radius {d} vs {radius}"
        );
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

/// The force the hook returns for one sub-step of `DT`. `gravity`: `None`
/// is the plain `Magnus` (which assumes the default gravity [`G`]),
/// `Some(g)` is `Magnus::with_gravity(g)`.
fn direct_force(p: &BallProfile, v: Vec3, w: Vec3, gravity: Option<Vec3>) -> Vec3 {
    let mut hook = magnus_hook(gravity);
    let mut b = Ball::new(ProfileId(0), Vec3::new(0.1, 0.2, 0.3));
    b.velocity = v;
    b.angular_velocity = w;
    hook.ball_added(BallHandle(0), &b);
    hook.force_in(&ctx(&[]), BallHandle(0), &b, p, DT)
}

/// The velocity the hook's force acts on. The solver hands a hook the
/// velocity after the sub-step's gravity kick, `v + g·h`; the hook takes the
/// sub-step midpoint `v − g·h/2` of what it is given (Orchestrator ruling,
/// DECISIONS "2.7 Magnus: which velocity the hook rotates"), `h` being the
/// `dt` passed to the hook. With zero gravity that is `v` itself.
fn midpoint(v: Vec3, gravity: Vec3) -> DVec3 {
    v.as_dvec3() - gravity.as_dvec3() * (0.5 * f64::from(DT))
}

/// `φ = S|ω|h/m`: the angle the velocity turns in one sub-step. A hook may
/// return the explicit `S (ω × v)` or a force that integrates the same
/// equation more exactly over the sub-step (e.g. the rotation of `v` by `φ`);
/// the two differ by `O(φ)` relative, which is what the direct checks allow.
fn turn_angle(p: &BallProfile, w: Vec3) -> f64 {
    f64::from(p.magnus_coeff) * f64::from(w.length()) * f64::from(DT) / f64::from(p.mass_kg)
}

#[test]
fn hook_force_is_s_omega_cross_v() {
    // The bare formula, through the zero-gravity hook.
    let p = profile("uv_globall");
    let w = Vec3::new(0.0, 0.0, 200.0);
    let v = Vec3::new(2.0, 0.0, 0.0);
    let f = direct_force(&p, v, w, Some(Vec3::ZERO));
    let tol = 4.0e-3 * (1.0e-5 + turn_angle(&p, w));
    assert!(
        (f.as_dvec3() - DVec3::new(0.0, 4.0e-3, 0.0)).length() < tol,
        "{f:?}"
    );
    // The plain hook assumes the default gravity: S·(ω × (v − g·h/2)).
    let w = Vec3::new(0.0, 200.0, 0.0);
    let f = direct_force(&p, v, w, None);
    let expect = 1.0e-5 * w.as_dvec3().cross(midpoint(v, G));
    assert!(
        (f.as_dvec3() - expect).length() < tol,
        "{f:?} vs {expect:?}"
    );
    let same = direct_force(&p, v, w, Some(G));
    assert_eq!(f, same, "plain Magnus is with_gravity(default gravity)");
    let mut zero = p.clone();
    zero.magnus_coeff = 0.0;
    for g in [None, Some(Vec3::ZERO), Some(G)] {
        let f = direct_force(
            &zero,
            Vec3::new(2.0, 1.0, -3.0),
            Vec3::new(50.0, 0.0, 200.0),
            g,
        );
        assert_eq!(f, Vec3::ZERO);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    /// The force helper: `F = S (ω × u)` (to `O(φ)`, see [`turn_angle`]),
    /// `u = v` for the zero-gravity hook (the bare formula) and the sub-step
    /// midpoint `v − g·h/2` for a hook with gravity (plain `Magnus`: the
    /// default gravity); it does no work on `u` over the sub-step, has no
    /// component along `ω`, is finite and stateless.
    #[test]
    fn prop_hook_force(
        s in 0.0f32..5.0e-5,
        v in prop::array::uniform3(-10.0f32..10.0),
        w in prop::array::uniform3(-600.0f32..600.0),
        which in 0usize..7,
        g_other in prop::array::uniform3(-12.0f32..12.0),
    ) {
        let mut p = builtin()[which].clone();
        p.magnus_coeff = s;
        let (v, w) = (Vec3::from(v), Vec3::from(w));
        let g_other = Vec3::from(g_other);
        let phi = turn_angle(&p, w);
        let wd = w.as_dvec3();
        for (hook_gravity, gravity) in [
            (Some(Vec3::ZERO), Vec3::ZERO),
            (None, G),
            (Some(g_other), g_other),
        ] {
            let f = direct_force(&p, v, w, hook_gravity);
            prop_assert!(f.is_finite());
            let fd = f.as_dvec3();
            let u = midpoint(v, gravity);
            let expect = f64::from(s) * wd.cross(u);
            let scale = f64::from(s) * wd.length() * u.length();
            let tol = (1.0e-5 + phi) * scale + 1.0e-12;
            prop_assert!(
                (fd - expect).length() <= tol,
                "gravity {:?}: {:?} vs {:?}", gravity, f, expect
            );
            // No work: the speed after `u += F h/m` is the speed before, up
            // to the explicit update's own second-order term φ² (and rounding).
            let after = u + fd * (f64::from(DT) / f64::from(p.mass_kg));
            let gain = (after.length_squared() - u.length_squared()) / u.length_squared().max(1e-12);
            prop_assert!(
                gain.abs() <= 1.01 * phi * phi + 1.0e-6,
                "gravity {:?}: relative |u|² change {}", gravity, gain
            );
            // No component along the spin axis.
            prop_assert!(fd.dot(wd).abs() <= (1.0e-5 * scale + 1.0e-12) * wd.length());
            // Stateless: the same call again gives the same bits.
            let again = direct_force(&p, v, w, hook_gravity);
            prop_assert_eq!(f.to_array().map(f32::to_bits), again.to_array().map(f32::to_bits));
        }
    }

    #[test]
    fn prop_zero_coefficient_gives_exactly_zero(
        v in prop::array::uniform3(-10.0f32..10.0),
        w in prop::array::uniform3(-600.0f32..600.0),
        which in 0usize..7,
    ) {
        let mut p = builtin()[which].clone();
        p.magnus_coeff = 0.0;
        for g in [None, Some(Vec3::ZERO), Some(G)] {
            prop_assert_eq!(direct_force(&p, Vec3::from(v), Vec3::from(w), g), Vec3::ZERO);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, .. ProptestConfig::default() })]

    /// Free flight through the solver: lateral acceleration within 1 %, and
    /// the Magnus term does no work (see [`ke_bound`]).
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
            let (run, n_sub) = flight_counted(&p, g, v, w, ticks, Reg::All);
            let n_sub = n_sub[ticks - 1];
            // The rounding allowance is per sub-step, so the count must be
            // what the solver documents: 1..=max_substeps per tick.
            let cap = ticks as u64 * u64::from(config(g).max_substeps);
            prop_assert!((ticks as u64..=cap).contains(&n_sub), "{} sub-steps", n_sub);
            let k0 = v.as_dvec3().length_squared();
            let k1 = run[ticks - 1].velocity.as_dvec3().length_squared();
            let bound = ke_bound(
                f64::from(s) / f64::from(p.mass_kg),
                f64::from(w.length()),
                ticks,
                n_sub,
            );
            prop_assert!(
                ((k1 - k0) / k0).abs() <= bound,
                "{} vs {} (bound {}, {} sub-steps)", k1, k0, bound, n_sub
            );
        }
    }
}
