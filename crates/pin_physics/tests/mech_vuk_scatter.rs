//! Work item w7pre-vuk-scatter: every VUK eject is scattered by a per-mech
//! `Pcg32` derived from the sim seed (DECISIONS.md "w7pre-vuk-scatter —
//! deterministic VUK eject scatter"): the exit point moves sideways by at
//! most `VUK_SCATTER_LATERAL` (also capped at a quarter of the scoop
//! radius), the horizontal direction turns by at most
//! `VUK_SCATTER_ANGLE_DEG`, the velocity scales by `1 ± VUK_SCATTER_SPEED_FRAC`.
//! Same seed ⇒ bit-identical ejects; different seeds ⇒ different ejects;
//! the spread over many ejects is nonzero and inside the bounds.

#[path = "mech_util.rs"]
mod util;

use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_physics::mech::{
    MechSpec, VUK_SCATTER_ANGLE_DEG, VUK_SCATTER_LATERAL, VUK_SCATTER_SPEED_FRAC,
};
use proptest::prelude::*;
use util::*;

const EJECT_SPEED: f32 = 0.6;
const KICK_SPEED: f32 = 0.5;
/// Exit 80 mm above the scoop, in free air: the eject tick sees no contact.
const EXIT: [f32; 3] = [0.0, 0.0, 80.0];
/// Slack for the one tick of free flight after the eject (gravity, drag).
const SLACK_M: f32 = 1.0e-4;
const SLACK_M_S: f32 = 2.0e-3;

/// A VUK at `pos` (angle 90°: `u = +y`, so `z × u = −x`) on a flat table.
fn vuk(pos: Vec3, coil: u16, radius_mm: Option<f32>) -> MechSpec {
    let mut s = MechSpec::new(MechKind::Vuk, pos)
        .with_coil(CoilId(coil))
        .with_switch(SwitchId(coil));
    s.params.angle_deg = Some(90.0);
    s.params.exit_layer = Some(0);
    s.params.exit_offset = Some(EXIT);
    s.params.eject_speed_m_s = Some(EJECT_SPEED);
    s.params.kick_speed_m_s = Some(KICK_SPEED);
    s.params.radius = radius_mm;
    s
}

/// One eject state: exit point (back-projected one tick) and velocity
/// right after the eject.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Eject {
    pos: Vec3,
    vel: Vec3,
}

/// Captures a ball in VUK `mi` (at `pos`), fires its coil and returns the
/// state after the eject tick; removes the ball.
fn eject_once(t: &mut Table, mi: usize, pos: Vec3, coil: u16) -> Eject {
    let h = t.ball(pos.x, pos.y, Vec3::ZERO);
    t.tick();
    assert_eq!(t.mechs.vuk_ball(mi), Some(h), "captured");
    t.mechs.fire_coil(CoilId(coil), 20);
    t.tick();
    assert_eq!(t.mechs.vuk_ball(mi), None, "ejected");
    let b = t.get(h);
    t.solver.remove_ball(h).expect("ball");
    t.mechs.release_ball(h);
    // Drain the coil pulse so the next capture is held.
    for _ in 0..25 {
        t.tick();
    }
    Eject {
        pos: b.position - b.velocity * DT,
        vel: b.velocity,
    }
}

fn ejects(seed: Option<u64>, n: usize, radius_mm: Option<f32>) -> Vec<Eject> {
    let mut t = Table::new(&[vuk(Vec3::ZERO, 1, radius_mm)], world(&[]), 0.0);
    if let Some(s) = seed {
        t.mechs.set_scatter_seed(s);
    }
    (0..n)
        .map(|_| eject_once(&mut t, 0, Vec3::ZERO, 1))
        .collect()
}

/// (lateral shift along z × u, m; direction turn, deg; speed gain).
fn decompose(e: &Eject) -> (f32, f32, f32) {
    // The nominal exit point is the scoop position (origin) plus EXIT.
    let nominal = Vec3::from(EXIT) * 1.0e-3;
    let d = e.pos - nominal;
    let lateral = -d.x; // side = z × (+y) = −x
    let turn = libm::atan2f(-e.vel.x, e.vel.y).to_degrees();
    let horizontal = libm::sqrtf(e.vel.x * e.vel.x + e.vel.y * e.vel.y);
    let gain = horizontal / KICK_SPEED;
    (lateral, turn, gain)
}

#[test]
fn every_eject_is_within_the_bounds_and_the_spread_is_nonzero() {
    let all: Vec<Eject> = [1u64, 2, 3]
        .into_iter()
        .flat_map(|s| ejects(Some(s), 20, None))
        .collect();
    assert!(all.len() >= 50);
    let parts: Vec<(f32, f32, f32)> = all.iter().map(decompose).collect();
    for (e, &(lat, turn, gain)) in all.iter().zip(&parts) {
        let nominal = Vec3::from(EXIT) * 1.0e-3;
        let d = e.pos - nominal;
        assert!(
            d.y.abs() < SLACK_M && d.z.abs() < SLACK_M,
            "exit moved off the lateral axis: {e:?}"
        );
        assert!(lat.abs() <= VUK_SCATTER_LATERAL + SLACK_M, "lateral {lat}");
        assert!(turn.abs() <= VUK_SCATTER_ANGLE_DEG + 0.05, "turn {turn}");
        assert!(
            (gain - 1.0).abs() <= VUK_SCATTER_SPEED_FRAC + SLACK_M_S,
            "gain {gain}"
        );
        // The vertical part scales by the same factor (one tick of gravity
        // and drag aside).
        let vz0 = e.vel.z + 9.81 * DT;
        assert!(
            (vz0 / EJECT_SPEED - gain).abs() < 0.01,
            "vertical {vz0} vs gain {gain}"
        );
    }
    let spread = |f: &dyn Fn(&(f32, f32, f32)) -> f32| {
        let v: Vec<f32> = parts.iter().map(f).collect();
        let lo = v.iter().copied().fold(f32::INFINITY, f32::min);
        let hi = v.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mean = v.iter().sum::<f32>() / v.len() as f32;
        (hi - lo, mean)
    };
    let (lat_span, lat_mean) = spread(&|p| p.0);
    let (turn_span, turn_mean) = spread(&|p| p.1);
    let (gain_span, gain_mean) = spread(&|p| p.2);
    println!(
        "spans over {} ejects: lateral {lat_span} m, turn {turn_span} deg, gain {gain_span}",
        all.len()
    );
    // Uniform draws: the span of 60 samples covers most of the range and
    // the mean sits near the nominal.
    assert!(lat_span > VUK_SCATTER_LATERAL, "lateral span {lat_span}");
    assert!(turn_span > VUK_SCATTER_ANGLE_DEG, "turn span {turn_span}");
    assert!(gain_span > VUK_SCATTER_SPEED_FRAC, "gain span {gain_span}");
    assert!(lat_mean.abs() < 0.5 * VUK_SCATTER_LATERAL);
    assert!(turn_mean.abs() < 0.5 * VUK_SCATTER_ANGLE_DEG);
    assert!((gain_mean - 1.0).abs() < 0.5 * VUK_SCATTER_SPEED_FRAC);
}

#[test]
fn same_seed_gives_bit_identical_ejects_and_other_seeds_differ() {
    let bits = |v: &[Eject]| -> Vec<u32> {
        v.iter()
            .flat_map(|e| e.pos.to_array().into_iter().chain(e.vel.to_array()))
            .map(f32::to_bits)
            .collect()
    };
    let a = ejects(Some(42), 8, None);
    let b = ejects(Some(42), 8, None);
    assert_eq!(bits(&a), bits(&b), "same seed must replay bit-identically");
    for other in [0u64, 1, 43, u64::MAX] {
        let c = ejects(Some(other), 8, None);
        assert_ne!(bits(&a), bits(&c), "seed {other} gave the same ejects");
    }
    // Consecutive ejects of one run differ too (the path cannot repeat).
    for w in a.windows(2) {
        assert_ne!(w[0], w[1]);
    }
    // Without set_scatter_seed the default seed applies: deterministic.
    assert_eq!(bits(&ejects(None, 4, None)), bits(&ejects(None, 4, None)));
    assert_eq!(
        bits(&ejects(None, 4, None)),
        bits(&ejects(
            Some(pin_physics::mech::DEFAULT_SCATTER_SEED),
            4,
            None
        ))
    );
}

#[test]
fn reseeding_restarts_the_stream() {
    let mut t = Table::new(&[vuk(Vec3::ZERO, 1, None)], world(&[]), 0.0);
    t.mechs.set_scatter_seed(9);
    let first = eject_once(&mut t, 0, Vec3::ZERO, 1);
    let _ = eject_once(&mut t, 0, Vec3::ZERO, 1);
    t.mechs.set_scatter_seed(9);
    let again = eject_once(&mut t, 0, Vec3::ZERO, 1);
    assert_eq!(first, again);
}

#[test]
fn lateral_shift_is_capped_by_a_quarter_of_the_scoop_radius() {
    // radius 8 mm → cap 2 mm (< VUK_SCATTER_LATERAL).
    let cap = 0.25 * 0.008;
    assert!(cap < VUK_SCATTER_LATERAL);
    let all = ejects(Some(5), 60, Some(8.0));
    let lats: Vec<f32> = all.iter().map(|e| decompose(e).0).collect();
    let hi = lats.iter().copied().fold(0.0f32, |a, b| a.max(b.abs()));
    assert!(
        hi <= cap + SLACK_M,
        "lateral {hi} above the radius cap {cap}"
    );
    assert!(hi > 0.5 * cap, "lateral spread {hi} too small");
}

/// Each VUK has its own stream: ejecting a second VUK does not change the
/// first one's ejects (adding a VUK shifts no other consumer's draws).
#[test]
fn each_vuk_draws_from_its_own_stream() {
    let p0 = Vec3::ZERO;
    let p1 = v(0.2, 0.2, 0.0);
    let specs = [vuk(p0, 1, None), vuk(p1, 2, None)];
    let run = |interleave: bool| -> Vec<Eject> {
        let mut t = Table::new(&specs, world(&[]), 0.0);
        t.mechs.set_scatter_seed(77);
        let mut out = Vec::new();
        for _ in 0..6 {
            out.push(eject_once(&mut t, 0, p0, 1));
            if interleave {
                let _ = eject_once(&mut t, 1, p1, 2);
            }
        }
        out
    };
    assert_eq!(run(false), run(true));
}

/// A set without a VUK never draws: its state is independent of the
/// scatter seed.
#[test]
fn a_set_without_a_vuk_ignores_the_scatter_seed() {
    let mut bumper = MechSpec::new(MechKind::Bumper, v(0.0, 0.1, 0.0))
        .with_coil(CoilId(1))
        .with_switch(SwitchId(1));
    bumper.params.angle_deg = Some(0.0);
    let run = |seed: u64| {
        let mut t = Table::new(std::slice::from_ref(&bumper), world(&[]), 6.5);
        t.mechs.set_scatter_seed(seed);
        let h = t.ball(0.0, 0.2, v(0.0, -0.4, 0.0));
        for i in 0..800 {
            if i % 50 == 0 {
                t.mechs.fire_coil(CoilId(1), 20);
            }
            t.tick();
        }
        let b = t.get(h);
        [b.position, b.velocity, b.angular_velocity].map(|x| x.to_array().map(f32::to_bits))
    };
    assert_eq!(run(1), run(2));
    assert_eq!(run(1), run(u64::MAX));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]
    /// Any seed: every eject stays inside the bounds.
    #[test]
    fn any_seed_stays_in_bounds(seed in any::<u64>()) {
        for e in ejects(Some(seed), 4, None) {
            let (lat, turn, gain) = decompose(&e);
            prop_assert!(lat.abs() <= VUK_SCATTER_LATERAL + SLACK_M);
            prop_assert!(turn.abs() <= VUK_SCATTER_ANGLE_DEG + 0.05);
            prop_assert!((gain - 1.0).abs() <= VUK_SCATTER_SPEED_FRAC + SLACK_M_S);
            prop_assert!(e.pos.is_finite() && e.vel.is_finite());
        }
    }
}
