//! Subphase 2.6 spec tests (Independent Tester): the four verification
//! criteria of the brief on a CurvedBanana bat driven through the public
//! solver API, on the 2.3 rig (Appendix C defaults, standard_steel, 6.5°
//! playfield, 1 ms ticks).
//!
//! 1. Contacts at 25 / 50 / 85 % along the arc give monotonically changing
//!    release angles, each shifted in one consistent direction against a
//!    straight capsule of equal length (same chord, radii = tube radius).
//! 2. As `arc_radius_mm` grows the banana converges to the StraightCapsule
//!    (geometry, shot and held rebound), with the trend over 8 radii.
//! 3. A ball resting in the concave pocket of a raised bat stays cradled
//!    for 2,000 ticks.
//! 4. The 2.3 timing sweep on the curved bat: dead spot ≤ 50 % of the held
//!    rebound, every step > 0.75 m/s between neighbouring 1 ms flips
//!    reported and named (an unnamed one fails).
//!
//! Every bat here is mounted with its mid-plane at ball-centre height, so a
//! ball rolling on the playfield meets it in-plane.

#[path = "spec_2_6_common.rs"]
mod common;

use common::*;
use glam::{Quat, Vec3};
use pin_core::FlipperConfig;
use pin_core::ids::FlipperId;
use pin_physics::ball::{BallHandle, MovingBodies};

const LEFT: FlipperId = FlipperId(0);
const WINDOW: u64 = 400;

/// One mounted bat: config, centreline model, mid-plane lift.
#[derive(Clone, Debug)]
struct Rig {
    c: FlipperConfig,
    model: ArcModel,
    lift: f32,
}

/// Lower-left flipper on the flat playfield, pivot at the origin, bat
/// direction at −30° from +x at rest, sweep +50°.
fn left_banana(b: Banana) -> Rig {
    let model = arc_model(b, 50.0);
    let (c, lift) = aligned(
        |base, rest| banana_config(0, base, Vec3::Z, rest, 50.0, b),
        &model,
        [0.0, 0.0],
        -30.0,
    );
    Rig { c, model, lift }
}

fn left_straight(b: Banana) -> Rig {
    let model = segment_model(b);
    let (c, lift) = aligned(
        |base, rest| straight_equiv_config(0, base, Vec3::Z, rest, 50.0, b),
        &model,
        [0.0, 0.0],
        -30.0,
    );
    Rig { c, model, lift }
}

fn table(c: &FlipperConfig) -> Scene {
    Scene::new(
        table_world(Quat::IDENTITY),
        table_gravity(Quat::IDENTITY),
        std::slice::from_ref(c),
    )
}

fn pose(s: &Scene, r: &Rig) -> Pose {
    Pose::of(&s.set, LEFT, Vec3::Z, r.lift)
}

fn release_angle(v: Vec3) -> f32 {
    libm::atan2f(v.y, v.x).to_degrees()
}

// ------------------------------------------------------------ criterion 1

/// A ball at rest 0.2 mm off the resting bat's upper (+y, the side the bat
/// swings toward) face at fraction `frac` of the centreline; the flipper
/// fires at tick 0.
fn shot_from(r: &Rig, frac: f32) -> ArcShot {
    let mut s = table(&r.c);
    let p = pose(&s, r);
    let nrm = r.model.side_normal(frac, 1.0);
    let a = r.model.point(frac);
    let off = r.model.tube() + R + 2.0e-4;
    let centre = p.to_world([a[0] + nrm[0] * off, a[1] + nrm[1] * off], 0.0);
    let h = s.solver.add_ball(ball(centre, Vec3::ZERO)).unwrap();
    run_shot_arc(
        &mut s,
        h,
        &r.model,
        Vec3::Z,
        r.lift,
        LEFT,
        0,
        WINDOW,
        |t, set| {
            if t == 0 {
                set.set_energised(LEFT, true);
            }
        },
    )
}

#[test]
fn c1_release_angles_monotone_and_consistently_shifted_vs_straight() {
    let banana = left_banana(BANANA);
    let straight = left_straight(BANANA);
    println!(
        "banana {:?}: concave side {:+} (bat frame y), lift {:.2} mm",
        BANANA,
        banana.model.concave_side(),
        banana.lift * 1e3
    );
    let mut ab = Vec::new();
    let mut shifts = Vec::new();
    println!("contact_% banana_angle_° banana_m_s straight_angle_° straight_m_s shift_°");
    for frac in [0.25f32, 0.5, 0.85] {
        let sb = shot_from(&banana, frac);
        let ss = shot_from(&straight, frac);
        assert!(
            !sb.caught && sb.impact_tick.is_some(),
            "banana {frac}: {sb:?}"
        );
        assert!(
            !ss.caught && ss.impact_tick.is_some(),
            "straight {frac}: {ss:?}"
        );
        let (a, b) = (release_angle(sb.velocity), release_angle(ss.velocity));
        println!(
            "{:>5.0} {a:>8.2} {:>6.3} {b:>8.2} {:>6.3} {:>+7.2}",
            frac * 100.0,
            sb.speed,
            ss.speed,
            a - b
        );
        ab.push(a);
        shifts.push(a - b);
    }
    let inc = ab[0] < ab[1] && ab[1] < ab[2];
    let dec = ab[0] > ab[1] && ab[1] > ab[2];
    assert!(inc || dec, "banana release angles not monotone: {ab:?}");
    let all_pos = shifts.iter().all(|&d| d > 0.0);
    let all_neg = shifts.iter().all(|&d| d < 0.0);
    assert!(
        all_pos || all_neg,
        "shift against the straight capsule changes direction: {shifts:?}"
    );
    // A shift smaller than the angle noise is not a shift.
    assert!(
        shifts.iter().all(|d| d.abs() >= 0.05),
        "a contact shows no measurable shift: {shifts:?}"
    );
}

#[test]
fn c1_shift_grows_with_curvature_report() {
    // Report: the 50 % shift for three curvatures; a tighter arc must shift
    // the release angle further in the same direction.
    let straight = left_straight(BANANA);
    let base = release_angle(shot_from(&straight, 0.5).velocity);
    let mut shifts = Vec::new();
    for rad in [80.0f32, 120.0, 240.0] {
        let b = Banana {
            arc_radius_mm: rad,
            ..BANANA
        };
        let s = shot_from(&left_banana(b), 0.5);
        let d = release_angle(s.velocity) - base;
        println!(
            "arc radius {rad} mm: 50 % release shift {d:+.2}° (speed {:.3})",
            s.speed
        );
        shifts.push(d);
    }
    assert!(
        shifts[0].signum() == shifts[1].signum() && shifts[1].signum() == shifts[2].signum(),
        "shift direction depends on curvature: {shifts:?}"
    );
    assert!(
        shifts[0].abs() > shifts[2].abs(),
        "tighter arc does not shift more: {shifts:?}"
    );
}

// ------------------------------------------------------------ criterion 2

/// Signed distances reported by `closest_point` at probe points around a
/// resting bat (bat-frame offsets from the chord, mid-plane): the same
/// world points are used for both bats.
fn probe_distances(r: &Rig) -> Vec<f32> {
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&r.c));
    s.advance();
    let p = pose(&s, r);
    let l = BANANA.length_mm * 1.0e-3;
    let mut out = Vec::new();
    for k in 0..=10 {
        let x = l * k as f32 / 10.0;
        for y in [-0.03f32, -0.02, 0.02, 0.03] {
            let w = p.to_world([x, y], 0.0);
            out.push(s.set.closest_point(0, 0.0, w).unwrap().distance);
        }
    }
    // Tip cap.
    for deg in [-60.0f32, -30.0, 0.0, 30.0, 60.0] {
        let a = deg.to_radians();
        let w = p.to_world([l + 0.025 * libm::cosf(a), 0.025 * libm::sinf(a)], 0.0);
        out.push(s.set.closest_point(0, 0.0, w).unwrap().distance);
    }
    out
}

fn held_rebound(r: &Rig, frac: f32) -> ArcShot {
    let mut s = table(&r.c);
    let raised = pose(&s, r).turned(r.c.sweep_angle_deg);
    let a = raised.to_world(r.model.point(frac), 0.0);
    let h = s
        .solver
        .add_ball(ball(v(a.x, a.y + 0.20, R), v(0.0, -3.5, 0.0)))
        .unwrap();
    run_shot_arc(
        &mut s,
        h,
        &r.model,
        Vec3::Z,
        r.lift,
        LEFT,
        0,
        WINDOW,
        |t, set| {
            if t == 0 {
                set.set_energised(LEFT, true);
            }
        },
    )
}

/// Exit velocity of a single free-space impact (no gravity, no table) of a
/// 3.5 m/s ball on the face point at 50 %, the bat resting (`flip` None)
/// or fired 5 ms before arrival.
fn free_exit(r: &Rig, flip: Option<u64>) -> Vec3 {
    free_hit(r, 0.5, 3.5, flip, 40).last().unwrap().1
}

/// Criterion 2. Stated tolerance and rate:
/// * geometry (closest-point distances at 49 fixed world points around the
///   resting bat): O(1/R), error ≤ 1.05·L²/2R + 2 µm (the lateral offset
///   of the tip of a bat bent from its pivot tangent), checked from 240 mm;
/// * shot from 50 % of the resting bat (release angle, speed) and single
///   free-space impacts on a resting and a stroking bat: O(1/R); at
///   R ≥ 7.68 m within 0.5° / 1 % / 1 % of the exit velocity;
/// * held rebound on the pitched table (knock-down + EOS re-power): ≤ 5 %
///   at R ≥ 7.68 m: this quantity has a branch switch (2.785 ↔ 2.885 m/s,
///   3.7 %) for aim shifts of ~30 µm on the straight capsule itself
///   (`c2_held_rebound_noise_floor_report`), so it is gated at that floor.
#[test]
fn c2_converges_to_straight_capsule_as_radius_grows() {
    let straight = left_straight(BANANA);
    let d_ref = probe_distances(&straight);
    let shot_ref = shot_from(&straight, 0.5);
    let held_ref = held_rebound(&straight, 0.5);
    let free_ref = [free_exit(&straight, None), free_exit(&straight, Some(5))];
    println!(
        "straight: shot {:.3} m/s at {:.2}°, held rebound {:.3} m/s at {:.2}°, free exits {:?}",
        shot_ref.speed,
        release_angle(shot_ref.velocity),
        held_ref.speed,
        release_angle(held_ref.velocity),
        free_ref
    );
    let radii = [
        60.0f32, 120.0, 240.0, 480.0, 960.0, 1920.0, 3840.0, 7680.0, 30720.0, 122880.0,
    ];
    let l_m = BANANA.length_mm * 1.0e-3;
    let mut geo = Vec::new();
    let mut ang = Vec::new();
    let mut spd = Vec::new();
    let mut held = Vec::new();
    let mut free = Vec::new();
    println!(
        "radius_mm sagitta_mm geo_err_mm L²/2R_mm shot_angle_err_° shot_speed_err_% free_rest_err_m_s free_stroke_err_m_s held_speed_err_% held_angle_err_°"
    );
    for &rad in &radii {
        let b = Banana {
            arc_radius_mm: rad,
            ..BANANA
        };
        let rig = left_banana(b);
        let d = probe_distances(&rig);
        let ge = d
            .iter()
            .zip(&d_ref)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        let sh = shot_from(&rig, 0.5);
        let hd = held_rebound(&rig, 0.5);
        let fe = [
            (free_exit(&rig, None) - free_ref[0]).length(),
            (free_exit(&rig, Some(5)) - free_ref[1]).length(),
        ];
        let ae = (release_angle(sh.velocity) - release_angle(shot_ref.velocity)).abs();
        let se = (sh.speed - shot_ref.speed).abs() / shot_ref.speed;
        let he = (hd.speed - held_ref.speed).abs() / held_ref.speed;
        let ha = (release_angle(hd.velocity) - release_angle(held_ref.velocity)).abs();
        let l = b.length_mm;
        let sag = rad - libm::sqrtf(rad * rad - 0.25 * l * l);
        println!(
            "{rad:>7.0} {sag:>8.3} {:>8.4} {:>8.4} {ae:>8.3} {:>8.3} {:>8.4} {:>8.4} {:>8.3} {ha:>8.3}",
            ge * 1e3,
            l * l / (2.0 * rad),
            se * 100.0,
            fe[0],
            fe[1],
            he * 100.0
        );
        geo.push(ge);
        ang.push(ae);
        spd.push(se);
        held.push(he);
        free.push((fe[0] / free_ref[0].length()).max(fe[1] / free_ref[1].length()));
    }
    for k in 1..radii.len() {
        let q = radii[k] / radii[k - 1];
        println!(
            "R {}→{} (×{q}): error ratios geometry {:.2}, shot angle {:.2}, shot speed {:.2}, free impact {:.2}",
            radii[k - 1],
            radii[k],
            geo[k - 1] / geo[k],
            ang[k - 1] / ang[k],
            spd[k - 1] / spd[k],
            free[k - 1] / free[k]
        );
    }
    for k in 2..radii.len() {
        let bound = 1.05 * l_m * l_m / (2.0 * radii[k] * 1.0e-3) + 2.0e-6;
        assert!(
            geo[k] <= bound,
            "geometry error {} m at R {} mm above {bound}",
            geo[k],
            radii[k]
        );
        // O(1/R): the error falls with the radius ratio (±25 %) until it
        // reaches a few µm.
        if geo[k] > 5.0e-6 {
            let q = radii[k] / radii[k - 1];
            let ratio = geo[k - 1] / geo[k];
            assert!(
                ratio >= 0.75 * q && ratio <= 1.25 * q,
                "geometry error not O(1/R) between {} and {} mm: ratio {ratio}",
                radii[k - 1],
                radii[k]
            );
        }
    }
    // Shot angle is O(1/R) too (it is ∝ the bend at the contact).
    for k in 3..8 {
        let q = radii[k] / radii[k - 1];
        let ratio = ang[k - 1] / ang[k];
        assert!(
            ratio >= 0.6 * q && ratio <= 1.5 * q,
            "shot angle error not O(1/R) between {} and {} mm: ratio {ratio}",
            radii[k - 1],
            radii[k]
        );
    }
    for k in 7..radii.len() {
        assert!(
            ang[k] <= 0.5,
            "R {}: shot release angle error {}°",
            radii[k],
            ang[k]
        );
        assert!(
            spd[k] <= 0.01,
            "R {}: shot speed error {}",
            radii[k],
            spd[k]
        );
        assert!(
            free[k] <= 0.01,
            "R {}: free impact relative error {}",
            radii[k],
            free[k]
        );
        assert!(
            held[k] <= 0.05,
            "R {}: held rebound error {}",
            radii[k],
            held[k]
        );
    }
}

// ------------------------------------------------------------ criterion 3

/// A left flipper whose raised bat holds its concave face up the table with
/// the pocket's deepest point at mid-arc (world +y is up the table).
fn pocket_rig(b: Banana, sweep: f32) -> Rig {
    let model = arc_model(b, sweep);
    let inward = model.side_normal(0.5, model.concave_side());
    // Raised bat direction θ such that the inward mid-arc normal is +y.
    let theta = 90.0 - libm::atan2f(inward[1], inward[0]).to_degrees();
    let (c, lift) = aligned(
        |base, rest| banana_config(0, base, Vec3::Z, rest, sweep, b),
        &model,
        [0.0, 0.0],
        theta - sweep,
    );
    Rig { c, model, lift }
}

fn cradle_case(b: Banana, sweep: f32, frac: f32) {
    let r = pocket_rig(b, sweep);
    let mut s = table(&r.c);
    s.set.set_energised(LEFT, true);
    for _ in 0..80 {
        s.advance();
    }
    assert!(
        (s.set.angle_deg(LEFT) - r.c.end_angle_deg()).abs() < 0.05,
        "bat not raised: {} vs {}",
        s.set.angle_deg(LEFT),
        r.c.end_angle_deg()
    );
    let p = pose(&s, &r);
    let cs = r.model.concave_side();
    let a = r.model.point(frac);
    let nrm = r.model.side_normal(frac, cs);
    let off = r.model.tube() + R + 1.0e-4;
    let start = p.to_world([a[0] + nrm[0] * off, a[1] + nrm[1] * off], 0.0);
    let h = s.solver.add_ball(ball(start, Vec3::ZERO)).unwrap();
    let mut max_gap = 0.0f32;
    let mut max_speed = 0.0f32;
    let mut fr = (1.0f32, 0.0f32);
    for k in 0..2000 {
        s.advance();
        let bl = s.ball(h);
        let p = pose(&s, &r);
        let (q, z) = p.local(bl.position);
        let (d, f) = r.model.distance_2d(q);
        let gap = libm::hypotf(d, z) - R - r.model.tube();
        max_gap = max_gap.max(gap);
        max_speed = max_speed.max(bl.velocity.length());
        fr = (fr.0.min(f), fr.1.max(f));
        // On the concave side (inside the arc circle), in the pocket.
        let side = r.model.arc_side(q, 0.0);
        assert!(
            side == Some(true),
            "tick {k}: ball left the concave face (side {side:?}, frac {f:.3})"
        );
        assert!(
            gap < 1.0e-3,
            "tick {k}: ball {gap} m off the bat (frac {f:.3}, v {:?})",
            bl.velocity
        );
        assert!(
            (0.05..=0.95).contains(&f),
            "tick {k}: ball rolled to the end of the pocket (frac {f:.3})"
        );
    }
    let bl = s.ball(h);
    println!(
        "cradle sweep {sweep:+} start frac {frac}: max gap {:.3} mm, max speed {:.4} m/s, frac range {:.3}–{:.3}, final speed {:.5} m/s, bat {:.3}°",
        max_gap * 1e3,
        max_speed,
        fr.0,
        fr.1,
        bl.velocity.length(),
        s.set.angle_deg(LEFT)
    );
    assert!(
        (s.set.angle_deg(LEFT) - r.c.end_angle_deg()).abs() < 0.05,
        "bat knocked off its stop"
    );
    if (frac - 0.5).abs() < 1.0e-3 {
        assert!(
            (bl.position - start).length() < 3.0e-3,
            "ball placed at the pocket bottom drifted {} m",
            (bl.position - start).length()
        );
        assert!(
            bl.velocity.length() < 0.01,
            "ball not at rest: {:?}",
            bl.velocity
        );
    }
}

#[test]
fn c3_ball_in_concave_pocket_of_raised_bat_stays_2000_ticks() {
    cradle_case(BANANA, 50.0, 0.5);
}

#[test]
fn c3_pocket_holds_off_centre_and_mirrored() {
    for (sweep, frac) in [(50.0f32, 0.35f32), (50.0, 0.65), (-50.0, 0.5), (-50.0, 0.4)] {
        cradle_case(BANANA, sweep, frac);
    }
}

// ------------------------------------------------------------ criterion 4

#[derive(Copy, Clone, Debug, PartialEq)]
enum Phase {
    Rest,
    Stroke,
    AtEnd,
    Back,
}

fn phase(c: &FlipperConfig, s: &ArcShot) -> Phase {
    let (a, w) = s.bat_at_impact.expect("no contact");
    let sign = c.sweep_angle_deg.signum();
    if w * sign < -1.0e-3 {
        Phase::Back
    } else if (a - c.end_angle_deg()).abs() < 0.05 && w.abs() <= 1.0e-3 {
        Phase::AtEnd
    } else if (a - c.rest_angle_deg).abs() < 0.05 && w.abs() <= 1.0e-3 {
        Phase::Rest
    } else {
        Phase::Stroke
    }
}

/// A ball 200 mm up the table, straight down at 3.5 m/s, aimed at `frac`
/// of the raised bat's centreline; flip pressed at `flip` and held.
fn rebound_held_aim(r: &Rig, frac: f32, flip: u64) -> ArcShot {
    let mut s = table(&r.c);
    let raised = pose(&s, r).turned(r.c.sweep_angle_deg);
    let a = raised.to_world(r.model.point(frac), 0.0);
    let h: BallHandle = s
        .solver
        .add_ball(ball(v(a.x, a.y + 0.20, R), v(0.0, -3.5, 0.0)))
        .unwrap();
    run_shot_arc(
        &mut s,
        h,
        &r.model,
        Vec3::Z,
        r.lift,
        LEFT,
        0,
        WINDOW,
        |t, set| {
            if t == flip {
                set.set_energised(LEFT, true);
            }
        },
    )
}

const STEP: f32 = 0.75;

/// Returns (minimum, held rebound, unexplained steps).
fn timing_sweep(r: &Rig, frac: f32, label: &str) -> (f32, f32, Vec<String>) {
    let held = rebound_held_aim(r, frac, 0);
    let th = held.impact_tick.expect("ball missed the held bat");
    assert!(th >= 25, "{label}: impact too early: {th}");
    println!(
        "{label}: held rebound {:.3} m/s (impact tick {th})",
        held.speed
    );
    let mut curve = Vec::new();
    for k in -20i64..=20 {
        let s = rebound_held_aim(r, frac, (th as i64 + k) as u64);
        assert!(s.impact_tick.is_some(), "{label}: offset {k}: no impact");
        curve.push((k, s));
    }
    println!("{label}: offset_ms rebound_m_s bat_phase contact_episodes");
    for (k, s) in &curve {
        println!("{k:>4} {:.3} {:?} {}", s.speed, phase(&r.c, s), s.episodes);
    }
    let mut unexplained = Vec::new();
    for w in curve.windows(2) {
        let ((k0, s0), (k1, s1)) = (&w[0], &w[1]);
        let d = (s1.speed - s0.speed).abs();
        if d <= STEP {
            continue;
        }
        let (p0, p1) = (phase(&r.c, s0), phase(&r.c, s1));
        if p0 != p1 {
            println!(
                "{label}: step {d:.3} m/s between {k0} and {k1} ms: rigid-stop event (bat {p0:?} → {p1:?})"
            );
        } else if s0.episodes != s1.episodes {
            println!(
                "{label}: step {d:.3} m/s between {k0} and {k1} ms: hit/miss of a rebounding ball ({} → {} episodes)",
                s0.episodes, s1.episodes
            );
        } else {
            let m = format!(
                "{label}: UNEXPLAINED step {d:.3} m/s between {k0} and {k1} ms ({:.3} → {:.3}, bat {p0:?}, {} episodes)",
                s0.speed, s1.speed, s0.episodes
            );
            println!("{m}");
            unexplained.push(m);
        }
    }
    let min = curve.iter().fold((0i64, f32::INFINITY), |m, x| {
        if x.1.speed < m.1 { (x.0, x.1.speed) } else { m }
    });
    println!(
        "{label}: minimum {:.3} m/s at {} ms = {:.1} % of held (gate ≤ 50 %; Gate A1 ≤ 25 %: {})",
        min.1,
        min.0,
        100.0 * min.1 / held.speed,
        if min.1 <= 0.25 * held.speed {
            "met"
        } else {
            "not met (report)"
        }
    );
    (min.1, held.speed, unexplained)
}

fn c4_case(frac: f32) {
    let r = left_banana(BANANA);
    let label = format!("banana, contact {:.0} % of the raised arc", frac * 100.0);
    let (min, held, unexplained) = timing_sweep(&r, frac, &label);
    assert!(
        min <= 0.5 * held,
        "{label}: sweep minimum {min:.3} m/s > 50 % of held rebound {held:.3}"
    );
    assert!(unexplained.is_empty(), "{unexplained:#?}");
}

#[test]
fn c4_timing_sweep_banana_50_percent() {
    c4_case(0.5);
}

#[test]
fn c4_timing_sweep_banana_70_percent() {
    c4_case(0.7);
}

#[test]
fn c4_timing_sweep_straight_reference_report() {
    // Report only: the same rig on the equal-length straight capsule.
    let r = left_straight(BANANA);
    for frac in [0.5f32, 0.7] {
        let label = format!("straight equiv, contact {:.0} %", frac * 100.0);
        let _ = timing_sweep(&r, frac, &label);
    }
}

// --------------------------------------------------------- sanity: model

#[test]
fn model_matches_reported_surface() {
    // The centreline model used by these tests agrees with the bat: points
    // at tube + 5 mm from the model centreline on both faces report a
    // 5 mm distance (catches a wrong convention before the criteria do).
    let r = left_banana(BANANA);
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&r.c));
    s.advance();
    let p = pose(&s, &r);
    for k in 0..=20 {
        let f = k as f32 / 20.0;
        for side in [-1.0f32, 1.0] {
            let a = r.model.point(f);
            let n = r.model.side_normal(f, side);
            let off = r.model.tube() + 0.005;
            let w = p.to_world([a[0] + n[0] * off, a[1] + n[1] * off], 0.0);
            let d = s.set.closest_point(0, 0.0, w).unwrap().distance;
            assert!(
                (d - 0.005).abs() < 2.0e-5,
                "frac {f} side {side}: reported distance {d}, model 0.005"
            );
        }
    }
}

#[test]
fn c2_held_rebound_noise_floor_report() {
    // Report: how much the held rebound of the *straight* capsule itself
    // moves for aim shifts of a few µm (the noise floor against which the
    // banana's convergence of the held rebound is judged).
    let r = left_straight(BANANA);
    let base = held_rebound_shifted(&r, 0.5, 0.0);
    println!("straight held rebound (no shift): {:.4} m/s", base.speed);
    let mut worst = 0.0f32;
    for dx in [-5.0e-5f32, -1.0e-5, -2.0e-6, 2.0e-6, 1.0e-5, 5.0e-5] {
        let s = held_rebound_shifted(&r, 0.5, dx);
        let e = (s.speed - base.speed).abs() / base.speed;
        worst = worst.max(e);
        println!(
            "aim shift {:+.0} µm: held rebound {:.4} m/s ({:+.2} %) at {:.2}°",
            dx * 1e6,
            s.speed,
            100.0 * (s.speed - base.speed) / base.speed,
            release_angle(s.velocity)
        );
    }
    println!("noise floor of the held rebound: {:.2} %", worst * 100.0);
}

fn held_rebound_shifted(r: &Rig, frac: f32, dx: f32) -> ArcShot {
    held_rebound_shifted_xy(r, frac, dx, 0.0)
}

fn held_rebound_shifted_xy(r: &Rig, frac: f32, dx: f32, dy: f32) -> ArcShot {
    let mut s = table(&r.c);
    let raised = pose(&s, r).turned(r.c.sweep_angle_deg);
    let a = raised.to_world(r.model.point(frac), 0.0);
    let h = s
        .solver
        .add_ball(ball(v(a.x + dx, a.y + 0.20 + dy, R), v(0.0, -3.5, 0.0)))
        .unwrap();
    run_shot_arc(
        &mut s,
        h,
        &r.model,
        Vec3::Z,
        r.lift,
        LEFT,
        0,
        WINDOW,
        |t, set| {
            if t == 0 {
                set.set_energised(LEFT, true);
            }
        },
    )
}

/// Free space, no gravity: a ball at `speed` straight at the face point at
/// `frac` of a resting bat (or one fired at `flip`). Returns per tick the
/// ball position and velocity for `ticks` ticks after the start.
fn free_hit(
    r: &Rig,
    frac: f32,
    speed: f32,
    flip: Option<u64>,
    ticks: u64,
) -> Vec<(Vec3, Vec3, bool)> {
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&r.c));
    let p = pose(&s, r);
    let a = r.model.point(frac);
    let m = r.model.side_normal(frac, 1.0);
    let d = r.model.tube() + R + 0.05;
    let start = p.to_world([a[0] + m[0] * d, a[1] + m[1] * d], 0.0);
    let h = s
        .solver
        .add_ball(ball(start, -p.vec_to_world(m) * speed))
        .unwrap();
    let mut out = Vec::new();
    for _ in 0..ticks {
        if Some(s.tick) == flip {
            s.set.set_energised(LEFT, true);
        }
        s.advance();
        let b = s.ball(h);
        out.push((b.position, b.velocity, s.hit(h, 0)));
    }
    out
}

#[test]
fn c4_dead_spot_location_report() {
    // Report: where the minimum lies when the sweep is widened to −40 ms
    // (outside the gated ±20 ms), banana and straight, 50 % and 70 %.
    for (name, r) in [
        ("banana", left_banana(BANANA)),
        ("straight equiv", left_straight(BANANA)),
    ] {
        for frac in [0.5f32, 0.6, 0.7, 0.8] {
            let held = rebound_held_aim(&r, frac, 0);
            let th = held.impact_tick.unwrap() as i64;
            let mut row = String::new();
            let mut m = (0i64, f32::INFINITY);
            for k in -40i64..=-15 {
                if th + k < 0 {
                    continue;
                }
                let s = rebound_held_aim(&r, frac, (th + k) as u64);
                row.push_str(&format!(" {k}:{:.2}", s.speed));
                if s.speed < m.1 {
                    m = (k, s.speed);
                }
            }
            println!(
                "{name} {:.0} %: held {:.3}, minimum over −40…−15 ms {:.3} at {} ms ({:.1} %) [{}]",
                frac * 100.0,
                held.speed,
                m.1,
                m.0,
                100.0 * m.1 / held.speed,
                row.trim()
            );
        }
    }
}
