use parry_nonlinear::{Flipper, flipper_vs_ball};
use parry3d::math::{Real, Vector};

const FLIPPER: Flipper = Flipper {
    half_len: 0.03,
    radius: 0.01,
    omega: 30.0,
};
const BALL_R: Real = 0.0135;

fn polar(r: Real, a: Real) -> Vector {
    Vector::new(r * a.cos(), r * a.sin(), 0.0)
}

#[test]
fn rotating_capsule_hits_resting_ball() {
    let (d, a0) = (0.045, 0.8);
    let hit = flipper_vs_ball(FLIPPER, BALL_R, polar(d, a0), Vector::ZERO, 0.05)
        .expect("capsule vs ball must be supported")
        .expect("sweep through 1.5 rad must hit the ball");
    // Analytic: contact when the perpendicular distance from the flipper axis
    // equals radius sum, i.e. angle = a0 - asin((r_f + r_b) / d).
    let expected = (a0 - ((FLIPPER.radius + BALL_R) / d).asin()) / FLIPPER.omega;
    println!(
        "[parry3d] capsule vs ball (static): toi={:.6} expected={expected:.6} status={:?}",
        hit.time_of_impact, hit.status
    );
    assert!((hit.time_of_impact - expected).abs() < 2e-4, "{hit:?}");
    // Contact normal on the flipper should point roughly toward the ball (+Y-ish
    // in the flipper's rotated frame) — just check it is unit length.
    assert!((hit.normal1.length() - 1.0).abs() < 1e-3);
}

#[test]
fn rotating_capsule_hits_incoming_ball() {
    // Ball falling toward the flipper along -Y while the flipper swings up.
    let hit = flipper_vs_ball(
        FLIPPER,
        BALL_R,
        Vector::new(0.04, 0.08, 0.0),
        Vector::new(0.0, -2.0, 0.0),
        0.05,
    )
    .expect("supported")
    .expect("must hit");
    println!(
        "[parry3d] capsule vs ball (moving): toi={:.6}",
        hit.time_of_impact
    );
    assert!(hit.time_of_impact > 0.0 && hit.time_of_impact < 0.05);
}

#[test]
fn ball_behind_sweep_is_a_miss_not_unsupported() {
    // Ball at negative angle: the flipper rotates away from it.
    let r = flipper_vs_ball(FLIPPER, BALL_R, polar(0.045, -0.8), Vector::ZERO, 0.05);
    println!("[parry3d] capsule vs ball (miss): {r:?}");
    assert!(matches!(r, Ok(None)), "{r:?}");
}
