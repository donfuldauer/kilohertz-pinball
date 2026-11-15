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
    /// End-of-impact-tick normal speed over the Impact event's `normal_speed`.
    e_speed: f64,
    /// Median of `vs[k] + gn (k - k0) dt` for k0..k0+5 over the impact speed.
    e_cross: f64,
    /// Parabolic apex height above the floor.
    apex: f64,
    /// The Impact event's approach speed.
    impact_speed: f64,
}

/// Drops a ball from `drop` metres above the floor, along the floor normal,
/// and measures the rebound along that normal.
///
/// The restitution is the post-impact normal velocity at the end of the impact
/// tick over the Impact event's `normal_speed`. (The apex height carries a
/// tick-phase bias of g h (1/2 - phi) from the semi-implicit Euler integrator,
/// so it is only used for the phase-averaged height ratio.)
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
        if impact.is_none() && !ev.as_slice().is_empty() {
            // Guard: exactly one event, an Impact, and so no other contact
            // between the impact and the end of this tick.
            assert_eq!(ev.as_slice().len(), 1, "extra events on the impact tick");
            let Some(BallEvent::Impact { normal_speed, .. }) = ev.as_slice().first() else {
                panic!("first event is not an Impact");
            };
            impact = Some(tick);
            impact_speed = f64::from(*normal_speed);
        }
        hs.push(height(&s));
        vs.push(vn(&s));
        if let Some(k0) = impact {
            // Apex: first fall after the rebound, parabola through the
            // the last three samples (the vertex lies between the last two).
            if tick > k0 + 1 && vs[tick] < 0.0 && vs[tick - 1] >= 0.0 {
                let i = tick - 1;
                let (y0, y1, y2) = (hs[i - 1], hs[i], hs[i + 1]);
                let denom = y0 - 2.0 * y1 + y2;
                apex = Some(y1 - (y2 - y0) * (y2 - y0) / (8.0 * denom));
                break;
            }
        }
    }
    let k0 = impact.expect("no impact");
    let apex = apex.expect("no apex");
    let v_fall = (2.0 * gn * f64::from(drop)).sqrt();
    assert!(
        (impact_speed - v_fall).abs() < 0.01 * v_fall,
        "impact speed {impact_speed} vs free fall {v_fall}"
    );
    let mut cross: Vec<f64> = (k0..k0 + 5)
        .map(|k| vs[k] + gn * (k - k0) as f64 * dt)
        .collect();
    cross.sort_by(f64::total_cmp);
    Bounce {
        e_speed: vs[k0] / impact_speed,
        e_cross: cross[2] / impact_speed,
        apex,
        impact_speed,
    }
}

/// Phase-averaged height ratio: `N` drops spread evenly over one tick of fall
/// (`drop + k v_impact dt / N`), mean of apex / drop. Also asserts the speed
/// ratios of every drop.
fn mean_height_ratio(
    profile: pin_core::ids::ProfileId,
    fm: Material,
    pitch: f32,
    drop: f32,
    e: f64,
) -> f64 {
    const N: usize = 20;
    let v_impact = (2.0 * 9.81f64 * f64::from(pitch.cos()) * f64::from(drop)).sqrt();
    let step = v_impact * f64::from(DT) / N as f64;
    // Fall distance of one tick at v_impact is v dt; spread phase over it.
    let mut sum = 0.0;
    for k in 0..N {
        let d = f64::from(drop) + k as f64 * step;
        let b = bounce(profile, fm, pitch, d as f32);
        assert!(
            (b.e_speed / e - 1.0).abs() < 0.01,
            "speed ratio {} vs e {e} (drop {d})",
            b.e_speed
        );
        sum += b.apex / d;
    }
    sum / N as f64
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

/// Shared body of the flat and pitched gating tests.
fn check_floor(pitch: f32) {
    for (profile, fm) in CASES {
        let e = table_e(profile, fm);
        for drop in [0.05f32, 0.15] {
            let b = bounce(profile, fm, pitch, drop);
            let mean = mean_height_ratio(profile, fm, pitch, drop, e);
            println!(
                "{:?}/{fm:?} pitch {:.1} deg drop {:.0} mm: e_table {e:.3} \
                 speed ratio {:.5} (err {:+.3} %), cross-check {:.5} (err {:+.3} %), \
                 v_in {:.4}, phase-averaged height ratio {mean:.5} vs e^2 {:.5} (err {:+.3} %)",
                profiles()[profile.index()].material,
                pitch.to_degrees(),
                drop * 1000.0,
                b.e_speed,
                (b.e_speed / e - 1.0) * 100.0,
                b.e_cross,
                (b.e_cross / e - 1.0) * 100.0,
                b.impact_speed,
                e * e,
                (mean / (e * e) - 1.0) * 100.0
            );
            assert!(
                (b.e_speed / e - 1.0).abs() < 0.01,
                "speed ratio {} vs e {e}",
                b.e_speed
            );
            assert!(
                (b.e_cross / e - 1.0).abs() < 0.01,
                "cross-check speed ratio {} vs e {e}",
                b.e_cross
            );
            assert!(
                (mean / (e * e) - 1.0).abs() < 0.02,
                "phase-averaged height ratio {mean} vs e^2 {}",
                e * e
            );
        }
    }
}

/// Gate A1 1b: drop onto a flat floor, every pair and both drop heights.
/// Restitution is not speed dependent above 0.1 m/s (every drop arrives at
/// more than 0.9 m/s): the post-impact normal speed over the impact speed is
/// asserted against the pair table's e (1 %) and the phase-averaged height
/// ratio against e^2 (2 %).
#[test]
fn drop_bounce_height_ratio_is_restitution_squared() {
    check_floor(0.0);
}

/// The same drops onto the 6.5 degree playfield, heights along the floor
/// normal. Investigation result: not a solver defect. The earlier apex-based
/// estimate was biased by the integrator's tick phase (g h (1/2 - phi), about
/// +-1.9 % in speed and +-4.8 % in height ratio over a 2 mm sweep of drop
/// heights, on flat and pitched floors alike); the solver's post-impact normal
/// velocity equals e times the impact speed to under 1e-5. Measured with the
/// post-impact velocity and a phase-averaged apex it matches the table.
#[test]
fn drop_bounce_on_the_pitched_playfield_restitution() {
    check_floor(PITCH_DEG.to_radians());
}

/// Phase sweep: 100 drops from 50.00 to 51.98 mm in 0.02 mm steps, steel on
/// wood, flat and pitched. Every end-of-impact-tick speed ratio is within 1 %
/// of e, whatever the tick phase of the impact.
#[test]
fn drop_bounce_restitution_is_independent_of_tick_phase() {
    let e = table_e(STEEL, Material::Wood);
    for pitch in [0.0f32, PITCH_DEG.to_radians()] {
        for i in 0..100 {
            let drop = (50.0 + 0.02 * i as f64) / 1000.0;
            let b = bounce(STEEL, Material::Wood, pitch, drop as f32);
            assert!(
                (b.e_speed / e - 1.0).abs() < 0.01,
                "pitch {pitch} drop {drop}: speed ratio {} vs e {e}",
                b.e_speed
            );
        }
    }
}
