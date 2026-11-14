//! Subphase 2.4 spec tests, gate step 6: a busy mech scenario ends in a
//! bit-identical state (and passes through bit-identical states) in 10 runs.

#[path = "spec_2_4_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_core::script::SwitchEdge;
use pin_physics::mech::{TiltBob, TiltBobParams};

struct Summary {
    hash: u64,
    closed_edges: [u32; 256],
    pulses: u64,
}

/// 5 s of the busy scenario; the hash covers every tick.
fn run() -> Summary {
    let mut rig = busy_rig();
    let mut h = Hash::new();
    let mut closed_edges = [0u32; 256];
    for t in 0..5000 {
        busy_drive(&mut rig, t);
        rig.step();
        busy_hash(&rig, &mut h);
        for (s, e) in rig.mechs.edges() {
            if *e == SwitchEdge::Closed {
                closed_edges[s.index()] += 1;
            }
        }
        for (_, b) in rig.solver.balls() {
            assert!(is_finite_ball(b), "tick {t}: non-finite ball");
        }
    }
    Summary {
        hash: h.0,
        closed_edges,
        pulses: rig.mechs.spinner_pulses(ix::SPINNER),
    }
}

#[test]
fn ten_runs_are_bit_identical() {
    let first = run();
    // The scenario really exercises the mechs.
    use ids::*;
    for (name, sw) in [
        ("bumper", BUMPER_SW),
        ("spinner", SPINNER_SW),
        ("tilt", TILT_SW),
    ] {
        assert!(
            first.closed_edges[sw.index()] > 0,
            "{name} switch never closed in the scenario"
        );
    }
    let active = first.closed_edges.iter().filter(|c| **c > 0).count();
    assert!(active >= 5, "only {active} different switches closed");
    assert!(first.pulses > 0);
    for i in 1..10 {
        let r = run();
        assert_eq!(r.hash, first.hash, "run {i} differs");
        assert_eq!(r.closed_edges, first.closed_edges);
        assert_eq!(r.pulses, first.pulses);
    }
}

#[test]
fn runs_on_other_threads_match() {
    let first = run().hash;
    let handles: Vec<_> = (0..3).map(|_| std::thread::spawn(|| run().hash)).collect();
    for h in handles {
        assert_eq!(h.join().unwrap(), first);
    }
}

#[test]
fn tilt_bob_is_deterministic() {
    let run = || {
        let mut bob = TiltBob::new(TiltBobParams::from_defaults(&defaults()));
        let mut h = Hash::new();
        for t in 0..20_000u32 {
            if t % 1700 == 3 {
                let a = t as f32 * 0.01;
                bob.nudge(Vec3::new(0.4 * a.cos(), 0.4 * a.sin(), 0.1));
            }
            bob.step(DT);
            h.vec3(bob.offset());
            h.vec3(bob.velocity());
            h.u32(u32::from(bob.closed()));
        }
        h.0
    };
    let first = run();
    for _ in 0..9 {
        assert_eq!(run(), first);
    }
}
