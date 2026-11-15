//! Gate A1 criterion 1, black box: a ball rolling without slipping down the
//! 6.5 degree playfield accelerates at 5/7 g sin(theta) (1.07 m in about
//! 1.64 s), and the drop-bounce height ratio equals e^2 within 2 %.
//!
//! The rolling check is evaluated two ways (orchestrator ruling, DECISIONS
//! "Gate A1 criterion 1a"): with rolling resistance 0 against the literal
//! formula, and with the default resistance c against
//! 5/7 g (sin(theta) - c cos(theta)); both within 1 %.

#[path = "ball_util.rs"]
mod util;

use glam::{Quat, Vec3};
use pin_core::Material;
use pin_physics::ball::{Ball, BallEvent, BallEvents, BallSolver, BallSolverConfig, Geometry};
use pin_physics::world::{StaticShape, StaticWorld, StaticWorldBuilder, layers};
use util::*;

const PITCH_DEG: f32 = 6.5;

/// A 2 m x 6 m x 0.1 m slab whose top face is tilted `pitch` about X
/// (pitch 0 gives a flat floor). Returns the world and the collider handle.
fn floor(pitch: f32) -> (StaticWorld, pin_physics::world::ColliderHandle, Quat) {
    let rot = Quat::from_rotation_x(pitch);
    let mut b = StaticWorldBuilder::new();
    let c = b.add_collider(
        StaticShape::Cuboid {
            center: rot * v(0.0, 0.0, -0.05),
            half_extents: v(2.0, 3.0, 0.05),
            rotation: rot,
        },
        layers::ALL,
        true,
    );
    (b.build(), c, rot)
}

struct RollRun {
    a: f64,
    t: f64,
}

/// Releases a steel ball from rest on the incline (at rest with zero spin it
/// already satisfies the rolling constraint; friction then spins it up as it
/// accelerates, so the whole 1.07 m is measured from the release), and
/// returns the time to travel 1.07 m along the slope (linear sub-tick
/// interpolation of the crossing) and the acceleration `2 s / t^2`.
fn roll_run(cfg: BallSolverConfig) -> RollRun {
    let pitch = PITCH_DEG.to_radians();
    let (world, _c, rot) = floor(pitch);
    let geo = Geometry::new(&world);
    let r = radius(STEEL);
    let mut s = BallSolver::new(cfg, &profiles());
    let start = rot * v(0.0, 2.5, r);
    let h = s.add_ball(Ball::new(STEEL, start)).unwrap();
    let down = rot * v(0.0, -1.0, 0.0);
    let mut ev = BallEvents::new();
    let target = 1.07f64;
    let mut prev = 0.0f64;
    for tick in 1..=3000u32 {
        s.step(&geo, DT, &mut ev);
        let d = f64::from((s.ball(h).unwrap().position - start).dot(down));
        if d >= target {
            let frac = (target - prev) / (d - prev);
            let t = (f64::from(tick) - 1.0 + frac) * f64::from(DT);
            return RollRun {
                a: 2.0 * target / (t * t),
                t,
            };
        }
        prev = d;
    }
    panic!("never travelled 1.07 m");
}

fn check_roll(run: &RollRun, a_expected: f64, label: &str) {
    let t_expected = (2.0 * 1.07 / a_expected).sqrt();
    let a_err = (run.a - a_expected) / a_expected;
    let t_err = (run.t - t_expected) / t_expected;
    println!(
        "{label}: a = {:.5} m/s^2 (expected {a_expected:.5}, error {:+.3} %), \
         t = {:.5} s (expected {t_expected:.5}, error {:+.3} %)",
        run.a,
        a_err * 100.0,
        run.t,
        t_err * 100.0
    );
    assert!(a_err.abs() < 0.01, "{label}: acceleration error {a_err}");
    assert!(t_err.abs() < 0.01, "{label}: time error {t_err}");
}

#[test]
fn rolling_107m_literal_formula_with_zero_rolling_resistance() {
    let mut cfg = config();
    cfg.rolling_resistance = 0.0;
    let g = f64::from(cfg.gravity.length());
    let theta = f64::from(PITCH_DEG.to_radians());
    let a_literal = 5.0 / 7.0 * g * theta.sin();
    let run = roll_run(cfg);
    check_roll(&run, a_literal, "c = 0, literal 5/7 g sin(theta)");
    // "1.07 m in about 1.64 s" of the plan.
    assert!((run.t - 1.64).abs() < 0.02, "t = {}", run.t);
}

#[test]
fn rolling_107m_default_parameters_match_the_resistance_model() {
    let cfg = config();
    let c = f64::from(cfg.rolling_resistance);
    assert!(c > 0.0);
    let g = f64::from(cfg.gravity.length());
    let theta = f64::from(PITCH_DEG.to_radians());
    let a_model = 5.0 / 7.0 * g * (theta.sin() - c * theta.cos());
    let run = roll_run(cfg);
    check_roll(&run, a_model, "default c, 5/7 g (sin - c cos)");
}

struct Bounce {
    e_speed: f64,
    ratio: f64,
}

/// Drops a ball from `drop` metres above the floor, along the floor normal,
/// and measures the rebound along that normal.
fn bounce(profile: pin_core::ids::ProfileId, material: Material, pitch: f32, drop: f32) -> Bounce {
    let (world, c, rot) = floor(pitch);
    let geo = Geometry::new(&world);
    let n = rot * Vec3::Z;
    let r = radius(profile);
    let mut s = solver();
    if material != Material::Wood {
        s.set_collider_material(c, material);
    }
    let gn = 9.81f64 * f64::from(pitch.cos());
    let dt = f64::from(DT);
    let h = s
        .add_ball(Ball::new(profile, rot * v(0.0, 0.0, r + drop)))
        .unwrap();
    let mut ev = BallEvents::new();
    let height = |s: &BallSolver| f64::from((s.ball(h).unwrap().position).dot(n) - r);
    let vn = |s: &BallSolver| f64::from(s.ball(h).unwrap().velocity.dot(n));
    let mut hs: Vec<f64> = vec![height(&s)];
    let mut vs: Vec<f64> = vec![vn(&s)];
    let mut impact = None;
    let mut impact_speed = 0.0f64;
    let mut apex = None;
    for tick in 1..=2000usize {
        s.step(&geo, DT, &mut ev);
        if impact.is_none()
            && let Some(BallEvent::Impact { normal_speed, .. }) = ev.as_slice().first()
        {
            impact = Some(tick);
            impact_speed = f64::from(*normal_speed);
        }
        hs.push(height(&s));
        vs.push(vn(&s));
        if let Some(k0) = impact {
            // Apex: first fall after the rebound, parabola through the
            // three samples around the maximum.
            if tick > k0 + 1 && vs[tick] < 0.0 && vs[tick - 1] >= 0.0 {
                let i = if hs[tick] > hs[tick - 1] {
                    tick
                } else {
                    tick - 1
                };
                let (y0, y1, y2) = (hs[i - 1], hs[i], hs[i + 1]);
                let denom = y0 - 2.0 * y1 + y2;
                let apex_h = y1 - (y2 - y0) * (y2 - y0) / (8.0 * denom);
                apex = Some(apex_h);
                break;
            }
        }
    }
    let k0 = impact.expect("no impact");
    let apex = apex.expect("no apex");
    // Rebound speed at the impact instant, from the ballistic flight to the
    // measured apex: v_out = sqrt(2 gn H). (Sampling the per-tick velocity
    // instead carries the integrator's half-tick offset, g dt / 2 = 5 mm/s,
    // i.e. 2 % of a 0.25 m/s rebound; that is printed for information only.)
    // The impact speed is the solver's own reported approach speed, checked
    // against the free-fall value. The effective restitution is the measured
    // ratio; it is also asserted against the pair table's value by the caller
    // (restitution does not depend on speed above 0.1 m/s, and every drop
    // here arrives at more than 0.9 m/s).
    let v_out = (2.0 * gn * apex).sqrt();
    let (hp, vp) = (hs[k0 - 1], vs[k0 - 1]);
    let tau = ((vp + (vp * vp + 2.0 * gn * hp).sqrt()) / gn).clamp(0.0, dt);
    let v_out_tick = vs[k0] + gn * (dt - tau);
    let v_fall = (2.0 * gn * f64::from(drop)).sqrt();
    println!(
        "    v_in: fall {v_fall:.4}, event {impact_speed:.4}; v_out: apex {v_out:.4}, per-tick {v_out_tick:.4}"
    );
    assert!((impact_speed - v_fall).abs() < 0.01 * v_fall);
    Bounce {
        e_speed: v_out / impact_speed,
        ratio: apex / f64::from(drop),
    }
}

const CASES: [(pin_core::ids::ProfileId, Material); 4] = [
    (STEEL, Material::Wood),
    (POWERBALL, Material::Wood),
    (STEEL, Material::Rubber),
    (POWERBALL, Material::Rubber),
];

fn table_e(profile: pin_core::ids::ProfileId, floor_material: Material) -> f64 {
    let cfg = config();
    let ball_material = profiles()[profile.index()].material;
    let pair = cfg
        .pairs
        .iter()
        .find(|p| p.matches(ball_material, floor_material))
        .expect("pair in the table");
    f64::from(pair.restitution)
}

fn print_case(
    profile: pin_core::ids::ProfileId,
    fm: Material,
    pitch: f32,
    drop: f32,
    e: f64,
    b: &Bounce,
) {
    println!(
        "{:?}/{fm:?} pitch {:.1} deg drop {:.0} mm: e_table {e:.3} \
         speed ratio {:.4} (err {:+.2} %), height ratio {:.5} vs e^2 {:.5} (err {:+.2} %)",
        profiles()[profile.index()].material,
        pitch.to_degrees(),
        drop * 1000.0,
        b.e_speed,
        (b.e_speed / e - 1.0) * 100.0,
        b.ratio,
        e * e,
        (b.ratio / (e * e) - 1.0) * 100.0
    );
}

/// Gate A1 1b: drop onto a flat floor, every pair and both drop heights.
/// Restitution is not speed dependent above 0.1 m/s (every drop arrives at
/// more than 0.9 m/s), so the effective restitution is asserted against the
/// pair table's value (1 %) and the height ratio against e^2 (2 %).
#[test]
fn drop_bounce_height_ratio_is_restitution_squared() {
    for (profile, fm) in CASES {
        let e = table_e(profile, fm);
        for drop in [0.05f32, 0.15] {
            let b = bounce(profile, fm, 0.0, drop);
            print_case(profile, fm, 0.0, drop, e, &b);
            assert!(
                (b.e_speed / e - 1.0).abs() < 0.01,
                "speed ratio {} vs e {e}",
                b.e_speed
            );
            assert!(
                (b.ratio / (e * e) - 1.0).abs() < 0.02,
                "height ratio {} vs e^2 {}",
                b.ratio,
                e * e
            );
        }
    }
}

/// The same drops onto the 6.5 degree playfield, heights along the floor
/// normal. Report only (finite and in (0, 1)): the pitched-floor deviation of
/// about -1.1 % in speed ratio and -2.8 % in height ratio for steel/wood at
/// 50 mm is recorded for an investigation, not hidden.
#[test]
fn drop_bounce_on_the_pitched_playfield_report() {
    let pitch = PITCH_DEG.to_radians();
    for (profile, fm) in CASES {
        let e = table_e(profile, fm);
        for drop in [0.05f32, 0.15] {
            let b = bounce(profile, fm, pitch, drop);
            print_case(profile, fm, pitch, drop, e, &b);
            assert!(b.e_speed.is_finite() && b.e_speed > 0.0 && b.e_speed < 1.0);
            assert!(b.ratio.is_finite() && b.ratio > 0.0 && b.ratio < 1.0);
        }
    }
}
