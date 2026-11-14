//! Seam task 2.2-S: with no moving bodies and no force hooks (and with an
//! old-style `force`-only hook), the ball solver is bit-identical to the
//! solver before the seams existed. The expected digests below were recorded
//! by running this file against HEAD 63243da (= 84c88eb for `pin_physics`),
//! before any edit to `ball/`, on x86_64 Linux; they are asserted only there
//! (PLAN.md §2.4, D9: cross-OS/arch digests are report-only). Every target
//! checks run-to-run bit-identity and prints its digests.
//!
//! Run with `PRINT_SEAM_HASHES=1` to print the digests instead of checking.

#[path = "ball_util.rs"]
mod util;

#[path = "spec_ball_common.rs"]
mod spec;

use glam::{Quat, Vec3};
use pin_core::BallProfile;
use pin_physics::ball::{
    Ball, BallEvent, BallEvents, BallHandle, BallSolver, ExternalForce, Geometry, ImpactTarget,
    Magnet, SurfaceVelocity,
};
use pin_physics::world::{GuideCurve, StaticShape, StaticWorld, StaticWorldBuilder, Wireform};
use util::*;

const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn mix(h: &mut u64, word: u64) {
    *h = (*h ^ word).wrapping_mul(FNV_PRIME);
}

fn hash_state(h: &mut u64, s: &BallSolver) {
    for word in state_bits(s) {
        mix(h, u64::from(word));
    }
    for i in 0..s.newton_chain_count() {
        let c = s.newton_chain(i);
        for k in 0..c.len() {
            mix(h, u64::from(c.ball_t(k).to_bits()));
            mix(h, u64::from(c.ball_speed(k).to_bits()));
        }
    }
}

fn hash_events(h: &mut u64, ev: &BallEvents) {
    for e in ev.as_slice() {
        match *e {
            BallEvent::Impact {
                ball,
                other,
                normal_speed,
            } => {
                mix(h, 1);
                mix(h, u64::from(ball.0));
                let (tag, idx) = match other {
                    ImpactTarget::Collider(c) => (0u64, c.index() as u64),
                    ImpactTarget::Guide(i) => (1, i as u64),
                    ImpactTarget::Wireform(i) => (2, i as u64),
                    ImpactTarget::Ball(b) => (3, u64::from(b.0)),
                    ImpactTarget::Newton(i) => (4, i as u64),
                    other => panic!("no moving bodies here: {other:?}"),
                };
                mix(h, tag);
                mix(h, idx);
                mix(h, u64::from(normal_speed.to_bits()));
            }
            BallEvent::SensorEdge { switch, closed } => {
                mix(h, 2);
                mix(h, u64::from(switch.0));
                mix(h, u64::from(closed));
            }
        }
    }
    mix(h, u64::from(ev.dropped()));
}

fn run_hashed(
    s: &mut BallSolver,
    geo: &Geometry<'_>,
    ticks: u32,
    mut each: impl FnMut(u32, &mut BallSolver),
) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    let mut ev = BallEvents::new();
    for tick in 0..ticks {
        each(tick, s);
        ev.clear();
        s.step(geo, DT, &mut ev);
        hash_state(&mut h, s);
        hash_events(&mut h, &ev);
        let mut sw = [0u64; 4];
        s.sensor_switches(&mut sw);
        for w in sw {
            mix(&mut h, w);
        }
    }
    h
}

/// The 2.2 Independent Tester's busy 5 s scenario (spec_ball_common), no hook.
fn spec_scenario(hook: bool) -> u64 {
    struct Drag;
    impl ExternalForce for Drag {
        fn force(&mut self, _h: BallHandle, b: &Ball, p: &BallProfile, _dt: f32) -> Vec3 {
            b.velocity * (-0.02 * p.mass_kg)
        }
    }
    let ext: Option<Box<dyn ExternalForce>> = if hook { Some(Box::new(Drag)) } else { None };
    let mut sc = spec::scenario(ext);
    let geo = Geometry {
        world: &sc.world,
        guides: &sc.guides,
        wireforms: &sc.wireforms,
    };
    let magnet = sc.magnet;
    run_hashed(&mut sc.solver, &geo, 5000, |tick, s| {
        s.set_magnet_power(magnet, spec::scenario_power(tick));
    })
}

/// Eight balls of every profile in a closed box: bounces, ball–ball
/// impacts, stacks settling on the floor.
fn box_scenario() -> u64 {
    let world = box_world(v(0.15, 0.12, 0.1));
    let geo = Geometry::new(&world);
    let mut s = solver();
    for k in 0..8u16 {
        let p = pin_core::ids::ProfileId(k % 6);
        let mut b = ball_at(
            p,
            v(
                -0.1 + 0.028 * f32::from(k),
                0.05 * libm::sinf(f32::from(k)),
                -0.05 + 0.012 * f32::from(k),
            ),
            v(
                1.5 - 0.4 * f32::from(k),
                0.7 * libm::cosf(f32::from(k)),
                0.3,
            ),
        );
        b.angular_velocity = v(5.0 * f32::from(k), -12.0, 3.0);
        s.add_ball(b).unwrap();
    }
    run_hashed(&mut s, &geo, 3000, |tick, s| {
        if tick == 1500 {
            s.apply_impulse(BallHandle(2), v(0.05, 0.08, 0.04));
        }
    })
}

/// Balls rolling on a belt-driven floor (surface velocity), a magnet pulling
/// steel across it, and a ball removed and another added mid-run.
fn belt_scenario() -> u64 {
    struct Belt;
    impl SurfaceVelocity for Belt {
        fn velocity_at(&self, _t: ImpactTarget, p: Vec3) -> Vec3 {
            v(0.2 + 0.5 * p.y, -0.1, 0.0)
        }
    }
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut s = solver();
    s.set_surface_velocity(Some(Box::new(Belt)));
    let m = s.add_magnet(Magnet {
        position: v(0.1, 0.05, -0.005),
        k_coil: 2.0e-5,
        d_min: 0.01,
        range: 0.1,
        layer_mask: pin_physics::world::layers::ALL,
    });
    s.set_magnet_power(m, 0.7);
    for (k, p) in [STEEL, POWERBALL, GLOBALL, MINI].into_iter().enumerate() {
        let r = radius(p);
        let mut b = ball_at(
            p,
            v(-0.1 + 0.06 * k as f32, 0.02 * k as f32, r + 0.002),
            v(0.3, -0.2 + 0.1 * k as f32, 0.0),
        );
        b.angular_velocity = v(0.0, 40.0 - 20.0 * k as f32, 5.0);
        s.add_ball(b).unwrap();
    }
    run_hashed(&mut s, &geo, 3000, |tick, s| {
        if tick == 1000 {
            s.remove_ball(BallHandle(1));
        }
        if tick == 1200 {
            let r = radius(JUMBO);
            s.add_ball(ball_at(JUMBO, v(0.0, -0.1, r + 0.03), v(0.0, 0.5, 0.0)))
                .unwrap();
        }
    })
}

/// A ball sliding along a pitched wall and dropping onto a sloped mesh,
/// rotated world (guide-free, wireform-free).
fn slope_scenario() -> u64 {
    let rot = Quat::from_rotation_y(0.3) * Quat::from_rotation_x(0.12);
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cuboid {
            center: rot * v(0.0, 0.0, -0.02),
            half_extents: v(0.4, 0.4, 0.02),
            rotation: rot,
        },
        pin_physics::world::layers::ALL,
        true,
    );
    b.add_collider(
        StaticShape::Cuboid {
            center: rot * v(0.0, -0.2, 0.02),
            half_extents: v(0.4, 0.01, 0.04),
            rotation: rot,
        },
        pin_physics::world::layers::ALL,
        true,
    );
    let world: StaticWorld = b.build();
    let guides: Vec<GuideCurve> = Vec::new();
    let wireforms: Vec<Wireform> = Vec::new();
    let geo = Geometry {
        world: &world,
        guides: &guides,
        wireforms: &wireforms,
    };
    let mut s = solver();
    for k in 0..3 {
        let p = [STEEL, JUMBO, MINI][k];
        let r = radius(p);
        s.add_ball(ball_at(
            p,
            rot * v(-0.2 + 0.15 * k as f32, 0.1, r + 0.05),
            v(0.1, -0.5, 0.0),
        ))
        .unwrap();
    }
    run_hashed(&mut s, &geo, 4000, |_, _| {})
}

/// Digests recorded at HEAD 63243da before the seam change.
const EXPECTED: [(&str, u64); 5] = [
    ("spec_scenario", 0x360d6fa3b46cb723),
    ("spec_scenario_hook", 0x9896df4948301cd5),
    ("box", 0x7850af7058aace12),
    ("belt", 0x710c25cd832c76c2),
    ("slope", 0x1764f34e6e37436f),
];

/// The digests above were recorded on x86_64 Linux. PLAN.md §2.4 (D9): the
/// determinism gate is the same binary on the same machine; cross-OS/arch
/// results are report-only (aarch64 macOS produces different digests). So
/// the recorded HEAD digests are asserted only on the recording target.
const RECORDING_TARGET: bool = cfg!(all(target_arch = "x86_64", target_os = "linux"));

fn digests() -> [(&'static str, u64); 5] {
    [
        ("spec_scenario", spec_scenario(false)),
        ("spec_scenario_hook", spec_scenario(true)),
        ("box", box_scenario()),
        ("belt", belt_scenario()),
        ("slope", slope_scenario()),
    ]
}

#[test]
fn no_bodies_no_hooks_is_bit_identical_to_head() {
    let got = digests();
    if std::env::var_os("PRINT_SEAM_HASHES").is_some() {
        for (name, h) in got {
            println!("    (\"{name}\", 0x{h:016x}),");
        }
        return;
    }
    // Every target: run-to-run bit-identical (same binary, same machine).
    assert_eq!(digests(), got, "seam digests not stable run to run");
    // Report-only outside the recording target (D9).
    for ((name, h), (_, want)) in got.iter().zip(EXPECTED) {
        println!(
            "[seam digest] {name}: 0x{h:016x} (x86_64-linux HEAD: 0x{want:016x}, {})",
            if *h == want { "same" } else { "differs" }
        );
    }
    if RECORDING_TARGET {
        assert_eq!(got, EXPECTED);
    }
}
