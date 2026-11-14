//! Subphase 2.5 spec tests (Independent Tester): property tests of the
//! stated invariants of `Simulation` (tick counter, query consistency,
//! determinism of injected runs, hash sensitivity).

#[path = "fixtures/spec_2_5/support.rs"]
mod support;

use pin_core::snapshot::BallState;
use proptest::prelude::*;
use support::*;

/// One scripted operation on the test API.
#[derive(Clone, Debug)]
enum Op {
    Spawn { ceramic: bool, x: f32, y: f32, vx: f32, vy: f32 },
    Switch { which: usize, closed: bool },
    Advance(u64),
}

const SWITCHES: [&str; 6] = [
    "left_flipper",
    "right_flipper",
    "start",
    "action",
    "lane_opto",
    "side_opto",
];

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        2 => (any::<bool>(), 0.03f32..0.48, 0.30f32..1.03, -2.0f32..2.0, -2.0f32..2.0)
            .prop_map(|(ceramic, x, y, vx, vy)| Op::Spawn { ceramic, x, y, vx, vy }),
        2 => (0usize..SWITCHES.len(), any::<bool>())
            .prop_map(|(which, closed)| Op::Switch { which, closed }),
        3 => (0u64..120).prop_map(Op::Advance),
    ]
}

/// What a run leaves behind, for comparison between two runs.
#[derive(Debug, PartialEq)]
struct Trace {
    hashes: Vec<u64>,
    tick: u64,
    balls: Vec<Option<BallState>>,
    scores: Vec<Option<i64>>,
    hits: Vec<Option<u32>>,
}

/// Applies the ops, checking the per-step invariants, and returns the trace.
fn run(seed: u64, ops: &[Op]) -> Result<Trace, TestCaseError> {
    let mut sim = load_at(&bundle(), seed);
    let mut handles = Vec::new();
    let mut hashes = vec![sim.state_hash()];
    let mut expected_tick = 0u64;
    for op in ops {
        match *op {
            Op::Spawn { ceramic, x, y, vx, vy } => {
                let profile = if ceramic { "ceramic_powerball" } else { "standard_steel" };
                let before = sim.balls_in_play();
                match sim.spawn_ball(profile, [x, y, R], [vx, vy, 0.0]) {
                    Ok(h) => {
                        prop_assert_eq!(sim.balls_in_play(), before + 1);
                        let b = sim.ball(h);
                        prop_assert!(b.is_some());
                        prop_assert_eq!(b.unwrap().position_m, [x, y, R]);
                        prop_assert_eq!(b.unwrap().velocity_m_s, [vx, vy, 0.0]);
                        prop_assert!(!handles.contains(&h) || sim.ball(h).is_some());
                        handles.push(h);
                    }
                    Err(_) => prop_assert_eq!(sim.balls_in_play(), before),
                }
            }
            Op::Switch { which, closed } => {
                let name = SWITCHES[which];
                let hits = sim.switch_hits(name).unwrap();
                let was = sim.switch_closed(name).unwrap();
                prop_assert!(sim.set_switch(name, closed).is_ok());
                prop_assert_eq!(sim.switch_closed(name), Some(closed));
                let want = hits + u32::from(closed && !was);
                prop_assert_eq!(sim.switch_hits(name), Some(want), "{}", name);
            }
            Op::Advance(n) => {
                sim.advance(n);
                expected_tick += n;
            }
        }
        // Invariants after every operation.
        prop_assert_eq!(sim.tick(), expected_tick);
        if expected_tick > 0 {
            prop_assert_eq!(sim.snapshot().tick, expected_tick);
        }
        prop_assert!(sim.balls_in_play() <= pin_core::snapshot::MAX_BALLS);
        let live = handles.iter().filter(|h| sim.ball(**h).is_some()).count();
        prop_assert!(live <= sim.balls_in_play());
        if matches!(op, Op::Advance(n) if *n > 0) {
            let snap = sim.snapshot();
            prop_assert_eq!(snap.ball_count as usize, sim.balls_in_play());
            for h in &handles {
                if let Some(b) = sim.ball(*h) {
                    prop_assert!(
                        snap.balls().contains(&b),
                        "ball() is not in the published snapshot"
                    );
                    for c in b.position_m.iter().chain(&b.velocity_m_s).chain(&b.angular_velocity_rad_s) {
                        prop_assert!(c.is_finite());
                    }
                    let q = b.orientation;
                    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
                    prop_assert!((n - 1.0).abs() < 1e-3, "orientation is a unit quaternion: {:?}", q);
                }
            }
        }
        // state_hash is a pure function of the state.
        let h = sim.state_hash();
        prop_assert_eq!(h, sim.state_hash());
        hashes.push(h);
    }
    Ok(Trace {
        hashes,
        tick: sim.tick(),
        balls: handles.iter().map(|h| sim.ball(*h)).collect(),
        scores: (1..=4).map(|p| sim.score(p)).collect(),
        hits: SWITCHES
            .iter()
            .chain(&["trough_1", "bumper_1_ring", "lower_left_eos"])
            .map(|n| sim.switch_hits(n))
            .collect(),
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, max_shrink_iters: 16, ..ProptestConfig::default() })]

    /// The same operations on the same seed give the same hash after every
    /// operation, the same balls, scores and hit counts (determinism of the
    /// injection API), and the per-operation invariants hold.
    #[test]
    fn same_operations_same_trace(
        seed in any::<u64>(),
        ops in prop::collection::vec(arb_op(), 1..30),
    ) {
        let (a, b) = on_sim_thread(move || (run(seed, &ops), run(seed, &ops)));
        let a = a?;
        let b = b?;
        prop_assert_eq!(a, b);
    }

    /// A dead handle stays dead, and hit counts never decrease.
    #[test]
    fn handles_do_not_resurrect_and_hits_are_monotone(
        x in 0.20f32..0.31,
        vy in -1.5f32..-0.2,
        extra in prop::collection::vec((0.05f32..0.45, 0.5f32..1.0), 0..4),
    ) {
        let r: Result<(), TestCaseError> = on_sim_thread(move || {
            let mut sim = load();
            // Aimed at the flipper gap / trough.
            let h = sim.spawn_ball("standard_steel", [x, 0.13, R], [0.0, vy, 0.0]).unwrap();
            let mut dead = false;
            let mut last_hits = 0;
            for i in 0..1500 {
                sim.advance(1);
                if i % 400 == 100 {
                    if let Some((ex, ey)) = extra.get(i / 400) {
                        let _ = sim.spawn_ball("standard_steel", [*ex, *ey, R], [0.0; 3]);
                    }
                }
                let alive = sim.ball(h).is_some();
                prop_assert!(!(dead && alive), "a removed ball's handle became valid again at step {}", i);
                dead |= !alive;
                let hits = sim.switch_hits("trough_1").unwrap();
                prop_assert!(hits >= last_hits);
                last_hits = hits;
            }
            Ok(())
        });
        r?;
    }

    /// The hash separates runs that differ in one injected velocity component.
    #[test]
    fn different_injection_different_hash(
        x in 0.05f32..0.45,
        y in 0.5f32..1.0,
        vx in -1.0f32..1.0,
        dv in prop_oneof![0.001f32..0.5, -0.5f32..-0.001],
        ticks in 0u64..200,
    ) {
        let (a, b) = on_sim_thread(move || {
            let go = |v: f32| {
                let mut sim = load();
                sim.spawn_ball("standard_steel", [x, y, R], [v, 0.0, 0.0]).unwrap();
                sim.advance(ticks);
                sim.state_hash()
            };
            (go(vx), go(vx + dv))
        });
        prop_assert_ne!(a, b);
    }
}
