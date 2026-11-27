//! #163 gate round 11: captive Newton balls are published as ordinary
//! `FrameStateSnapshot::balls` entries (SPEC Appendix A "Dynamic mesh slot
//! convention": "Captive balls are published as ordinary `balls` entries and
//! take no extra slots"; SPEC Subphase 2.2 captive balls on a 1D spline;
//! REQ-VIS-002). Every check goes through the production publisher
//! (`Simulation::load` + `advance`).

mod spec_kp023_support;

use pin_core::ProfileId;
use pin_core::fmath::max_f32;
use pin_core::snapshot::{BallState, MAX_BALLS};
use pin_sim::{SimConfig, Simulation};
use spec_kp023_support::*;

/// `standard_steel` is the first built-in profile.
const STEEL: ProfileId = ProfileId(0);

/// The Newton mech of `NEWTON` on collision layer 1.
const NEWTON_LAYER_1: &str = r#"
[[mechs]]
id = "k_newton"
kind = "newton_ball"
switch = "newton_sw"
position = [257.0, 300.0, 0.0]
angle_deg = 90.0
length = 50.0
layer = 1
"#;

fn load(b: &Bundle) -> Simulation {
    Simulation::load(&b.path(), SimConfig::default()).expect("load")
}

fn finite(b: &BallState) -> bool {
    b.position_m
        .iter()
        .chain(b.velocity_m_s.iter())
        .chain(b.orientation.iter())
        .chain(b.angular_velocity_rad_s.iter())
        .all(|c| c.is_finite())
}

/// Asserts `b` is the captive ball of chain 0 of `sim` on `layer`: its
/// published position is exactly the chain's path position, on the path
/// line (x, z of the path start (0.257, 0.300, R) within 1e-6 m). At load
/// (`at_load`) it is within 1e-6 m of the path start; at rest afterwards it
/// stays within 1e-3 m of it (it settles about 2e-5 m along the path under
/// the playfield pitch).
fn assert_captive_at_start(sim: &Simulation, b: &BallState, layer: u8, at_load: bool) {
    let tick = sim.tick();
    assert!(finite(b), "tick {tick}: non-finite captive {b:?}");
    let chain = sim.solver().newton_chain(0);
    assert_eq!(
        b.position_m,
        chain.ball_position(0).to_array(),
        "tick {tick}: published position is the chain's"
    );
    let start = [0.257f64, 0.300, f64::from(R)];
    let tol = |k: usize| if k == 1 && !at_load { 1e-3 } else { 1e-6 };
    for k in 0..3 {
        assert!(
            (f64::from(b.position_m[k]) - start[k]).abs() <= tol(k),
            "tick {tick}: captive position {:?} not at the path start {start:?}",
            b.position_m
        );
    }
    assert_eq!(b.profile, STEEL, "tick {tick}");
    assert_eq!(b.layer, layer, "tick {tick}");
    assert_eq!(b.orientation, [0.0, 0.0, 0.0, 1.0], "tick {tick}");
    assert_eq!(b.angular_velocity_rad_s, [0.0; 3], "tick {tick}");
    let [vx, vy, vz] = b.velocity_m_s;
    assert!(
        vx.abs() <= 1e-6 * max_f32(vy.abs(), 1.0) && vz.abs() <= 1e-6 * max_f32(vy.abs(), 1.0),
        "tick {tick}: captive velocity {:?} not along the path (+y)",
        b.velocity_m_s
    );
}

/// SPEC Appendix A: one Newton chain and no free balls publishes exactly
/// one ball (the captive one); `balls_in_play` stays free-only (0).
fn one_captive_no_free(tag: &str, mech: &'static str, layer: u8) {
    let b = bundle(tag, &[mech], QUIET);
    on_sim_thread(move || {
        let mut sim = load(&b);
        assert_eq!(sim.snapshot().ball_count, 1, "at load");
        let first = sim.snapshot().balls()[0];
        assert_captive_at_start(&sim, &first, layer, true);
        for _ in 0..200 {
            sim.advance(1);
            let tick = sim.tick();
            let snap = sim.snapshot();
            assert_eq!(snap.ball_count, 1, "tick {tick}");
            assert_eq!(sim.balls_in_play(), 0, "tick {tick}");
            assert_captive_at_start(&sim, &snap.balls()[0], layer, false);
        }
        assert_eq!(sim.counters().captive_balls_unpublished, 0);
    });
}

/// SPEC Appendix A "Dynamic mesh slot convention" (captive balls are
/// ordinary `balls` entries), layer 0.
#[test]
fn spec_appendix_a_captive_ball_published_layer_0() {
    one_captive_no_free("cap_l0", NEWTON, 0);
}

/// SPEC Appendix A: the captive ball carries the mech's `layer`.
#[test]
fn spec_appendix_a_captive_ball_published_layer_1() {
    one_captive_no_free("cap_l1", NEWTON_LAYER_1, 1);
}

/// SPEC Subphase 2.2 / Appendix A: with one free ball the free ball comes
/// first (index 0) and the captive one second (index 1); after the free
/// ball strikes it, the captive velocity is finite and parallel to the
/// path (+y).
#[test]
fn spec_2_2_captive_after_free_ball_and_struck_velocity_along_path() {
    let b = bundle("cap_strike", &[NEWTON], QUIET);
    on_sim_thread(move || {
        let mut sim = load(&b);
        let h = sim
            .spawn_ball("standard_steel", [0.257, 0.24, R], [0.0, 1.5, 0.0])
            .expect("spawn");
        let mut moved = false;
        for _ in 0..400 {
            sim.advance(1);
            let tick = sim.tick();
            let snap = sim.snapshot();
            let free = sim.balls_in_play();
            assert_eq!(usize::from(snap.ball_count), free + 1, "tick {tick}");
            if free == 1 {
                let fb = sim.ball(h).expect("free ball");
                assert_eq!(snap.balls()[0].position_m, fb.position_m, "tick {tick}");
                assert_eq!(snap.balls()[0].velocity_m_s, fb.velocity_m_s, "tick {tick}");
            }
            let c = &snap.balls()[free];
            assert!(finite(c), "tick {tick}: {c:?}");
            assert_eq!(c.profile, STEEL);
            assert_eq!(c.layer, 0);
            // On the path: x and z fixed at the path start.
            assert!(
                (f64::from(c.position_m[0]) - 0.257).abs() <= 1e-6,
                "tick {tick}: {c:?}"
            );
            assert!(
                (f64::from(c.position_m[2]) - f64::from(R)).abs() <= 1e-6,
                "tick {tick}: {c:?}"
            );
            let [vx, vy, vz] = c.velocity_m_s;
            assert!(
                vx.abs() <= 1e-6 * max_f32(vy.abs(), 1.0)
                    && vz.abs() <= 1e-6 * max_f32(vy.abs(), 1.0),
                "tick {tick}: captive velocity {:?} not along +y",
                c.velocity_m_s
            );
            if vy > 0.05 {
                moved = true;
            }
        }
        assert!(moved, "the struck captive ball never moved along +y");
    });
}

/// SPEC Appendix A / CR-011 capacity: 16 free balls fill the array; the
/// captive ball is dropped (never overwrites a free ball) and counted once
/// per tick in `captive_balls_unpublished`.
#[test]
fn spec_appendix_a_full_array_truncates_and_counts() {
    let b = bundle("cap_full", &[NEWTON], QUIET);
    on_sim_thread(move || {
        let mut sim = load(&b);
        assert_eq!(MAX_BALLS, 16);
        let mut n = 0;
        'outer: for row in 0..4 {
            for col in 0..4 {
                let p = [0.10 + 0.08 * col as f32, 0.50 + 0.08 * row as f32, R];
                sim.spawn_ball("standard_steel", p, [0.0; 3])
                    .expect("spawn");
                n += 1;
                if n == MAX_BALLS {
                    break 'outer;
                }
            }
        }
        assert_eq!(sim.balls_in_play(), MAX_BALLS);
        let mut full_ticks = 0;
        for _ in 0..50 {
            let before = sim.counters().captive_balls_unpublished;
            sim.advance(1);
            let tick = sim.tick();
            let snap = sim.snapshot();
            let free = sim.balls_in_play();
            if free == MAX_BALLS {
                full_ticks += 1;
                assert_eq!(usize::from(snap.ball_count), MAX_BALLS, "tick {tick}");
                assert_eq!(
                    sim.counters().captive_balls_unpublished,
                    before + 1,
                    "tick {tick}"
                );
            } else {
                assert_eq!(usize::from(snap.ball_count), free + 1, "tick {tick}");
            }
        }
        assert!(full_ticks > 0, "the array was never full");
    });
}
