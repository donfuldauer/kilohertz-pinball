//! Independent spec tests, Subphase 2.7, verification 1 and determinism.
//!
//! Verification 1: with `magnus_coeff = 0` and `remanence = 0`, every 2.2
//! baseline trajectory is bit-identical with the hooks registered. The
//! baseline is the same run with no hooks, compared bit for bit, every tick.
//! Also: a 3 s busy-table scenario with the hooks active is bit-stable over
//! 10 runs (gate step 6).

#[path = "spec_2_7_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_core::{BallProfile, ProfileId};
use pin_physics::ball::{Ball, BallEvent, BallEvents, Geometry};
use pin_physics::world::{StaticShape, StaticWorld, StaticWorldBuilder, layers};
use proptest::prelude::*;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

fn hash_events(h: &mut DefaultHasher, ev: &BallEvents) {
    for e in ev.as_slice() {
        match *e {
            BallEvent::Impact {
                ball, normal_speed, ..
            } => (0u8, ball.0, normal_speed.to_bits()).hash(h),
            BallEvent::SensorEdge { switch, closed } => (1u8, switch, closed).hash(h),
        }
    }
    ev.dropped().hash(h);
}

/// Per-tick digests of a busy-table run: (state bits, switches, events hash).
fn busy_trace(profiles: &[BallProfile], hooks: Hooks, ticks: u32) -> Vec<(Vec<u32>, [u64; 4], u64)> {
    let mut sc = busy(profiles, hooks);
    let mut ev = BallEvents::new();
    let mut out = Vec::with_capacity(ticks as usize);
    let mut sw = [0u64; 4];
    for tick in 0..ticks {
        sc.solver.set_magnet_power(sc.magnet, busy_power(tick));
        if tick % 500 == 250 {
            // Keep the table lively.
            let h = pin_physics::ball::BallHandle(((tick / 500) % 4) as u8);
            sc.solver.apply_impulse(h, Vec3::new(0.03, 0.08, 0.01));
        }
        ev.clear();
        let geo = Geometry {
            world: &sc.world,
            guides: &sc.guides,
            wireforms: &sc.wireforms,
        };
        sc.solver.step(&geo, DT, &mut ev);
        let mut h = DefaultHasher::new();
        hash_events(&mut h, &ev);
        sc.solver.sensor_switches(&mut sw);
        out.push((solver_bits(&sc.solver), sw, h.finish()));
    }
    out
}

fn assert_same_trace(a: &[(Vec<u32>, [u64; 4], u64)], b: &[(Vec<u32>, [u64; 4], u64)], what: &str) {
    assert_eq!(a.len(), b.len());
    for (tick, (x, y)) in a.iter().zip(b).enumerate() {
        assert!(x == y, "{what}: first difference at tick {tick}");
    }
}

// ------------------------------------------------------------ verification 1

#[test]
fn v1_busy_table_with_zero_coefficients_is_bit_identical() {
    let z = zeroed();
    assert!(z.iter().all(|p| p.remanence == 0.0 && p.magnus_coeff == 0.0));
    let baseline = busy_trace(&z, Hooks::None, 5000);
    // The scenario exercises the magnet: with it off the run differs.
    let before = busy_trace(&z, Hooks::Before, 5000);
    assert_same_trace(&before, &baseline, "hooks registered before the balls");
    let after = busy_trace(&z, Hooks::After, 5000);
    assert_same_trace(&after, &baseline, "hooks registered after the balls");
}

#[test]
fn v1_magnetisation_is_tracked_but_exerts_nothing_with_zero_remanence() {
    // The hooks are not simply inert with zeroed profiles: M still follows
    // the magnet, only the force is exactly zero.
    let z = zeroed();
    let mut sc = busy(&z, Hooks::Before);
    let mut ev = BallEvents::new();
    let mut seen = 0.0f32;
    for tick in 0..3000u32 {
        sc.solver.set_magnet_power(sc.magnet, 1.0);
        ev.clear();
        let geo = Geometry {
            world: &sc.world,
            guides: &sc.guides,
            wireforms: &sc.wireforms,
        };
        sc.solver.step(&geo, DT, &mut ev);
        let _ = tick;
        for (h, _) in sc.solver.balls() {
            let m = mag(sc.probe.as_ref().unwrap(), h);
            assert!((0.0..=1.0).contains(&m));
            seen = seen.max(m);
        }
    }
    assert!(seen > 0.5, "no ball was ever magnetised (max M {seen})");
}

/// The world of the 2.2 magnet criterion: floor, a wall and a post.
fn magnet_world() -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        cuboid(Vec3::new(0.0, 0.0, -0.05), Vec3::new(10.0, 10.0, 0.05)),
        layers::ALL,
        true,
    );
    b.add_collider(
        cuboid(Vec3::new(0.35, 0.0, 0.05), Vec3::new(0.01, 0.3, 0.05)),
        layers::ALL,
        true,
    );
    b.add_collider(
        StaticShape::Cylinder {
            base: Vec3::new(0.15, 0.03, 0.0),
            axis: Vec3::new(0.0, 0.0, 0.05),
            radius: 0.01,
        },
        layers::ALL,
        true,
    );
    b.build()
}

/// A ball rolled across a pulsed magnet on a floor (2.2 baseline style).
#[allow(clippy::too_many_arguments)]
fn roll(
    profiles: &[BallProfile],
    which: usize,
    hooks: bool,
    start_xy: (f32, f32),
    v0: Vec3,
    w0: Vec3,
    power: f32,
    ticks: usize,
) -> Vec<[u32; 14]> {
    let world = magnet_world();
    let geo = Geometry::new(&world);
    let mut s = solver(profiles, G);
    let id = s.add_magnet(magnet_at(Vec3::new(0.1, 0.0, -0.005)));
    if hooks {
        let _ = register(&mut s);
    }
    let r = profiles[which].radius_m();
    let mut b = Ball::new(ProfileId(which as u16), Vec3::new(start_xy.0, start_xy.1, r));
    b.velocity = v0;
    b.angular_velocity = w0;
    let h = s.add_ball(b).unwrap();
    let mut ev = BallEvents::new();
    let mut out = Vec::with_capacity(ticks);
    for tick in 0..ticks {
        // On, off (remanence would act here), on again.
        let p = if (tick / 150) % 2 == 0 { power } else { 0.0 };
        s.set_magnet_power(id, p);
        ev.clear();
        s.step(&geo, DT, &mut ev);
        out.push(state_bits(s.ball(h).unwrap()));
    }
    out
}

#[test]
fn v1_every_profile_rolled_across_a_magnet_is_bit_identical() {
    let z = zeroed();
    for which in 0..z.len() {
        for (v0, w0) in [
            (Vec3::new(0.6, 0.0, 0.0), Vec3::ZERO),
            (Vec3::new(1.5, 0.1, 0.0), Vec3::new(20.0, -60.0, 90.0)),
            (Vec3::new(0.25, -0.05, 0.4), Vec3::new(0.0, 0.0, 300.0)),
        ] {
            let with = roll(&z, which, true, (-0.05, 0.0), v0, w0, 1.0, 900);
            let without = roll(&z, which, false, (-0.05, 0.0), v0, w0, 1.0, 900);
            for (tick, (a, b)) in with.iter().zip(&without).enumerate() {
                assert_eq!(a, b, "{} tick {tick}", z[which].name);
            }
        }
    }
}

#[test]
fn v1_builtin_profiles_with_zero_coefficients_need_no_zeroing() {
    // Stock profiles whose own coefficients are zero (low_rem_steel: ρ = 0,
    // S = 0; ceramic_powerball and jumbo_xl_resin: c = 0, ρ = 0, S = 0)
    // behave as in 2.2 next to an active magnet, with the stock list.
    let stock = builtin();
    for name in ["low_rem_steel", "ceramic_powerball", "jumbo_xl_resin"] {
        let which = pid(name).index();
        assert_eq!(stock[which].remanence, 0.0);
        assert_eq!(stock[which].magnus_coeff, 0.0);
        let v0 = Vec3::new(0.9, 0.05, 0.0);
        let w0 = Vec3::new(10.0, 40.0, -80.0);
        let with = roll(&stock, which, true, (-0.05, 0.01), v0, w0, 1.0, 900);
        let without = roll(&stock, which, false, (-0.05, 0.01), v0, w0, 1.0, 900);
        for (tick, (a, b)) in with.iter().zip(&without).enumerate() {
            assert_eq!(a, b, "{name} tick {tick}");
        }
    }
}

#[test]
fn stock_coefficients_do_change_the_run() {
    // Guards against a vacuous verification 1: with the stock profiles the
    // hooks act (standard_steel remanence, uv_globall Magnus).
    let stock = builtin();
    let v0 = Vec3::new(0.6, 0.0, 0.0);
    let steel = pid("standard_steel").index();
    let with = roll(&stock, steel, true, (-0.05, 0.0), v0, Vec3::ZERO, 1.0, 900);
    let without = roll(&stock, steel, false, (-0.05, 0.0), v0, Vec3::ZERO, 1.0, 900);
    assert_ne!(with, without, "remanence had no effect on standard_steel");
    let glo = pid("uv_globall").index();
    let v0 = Vec3::new(0.3, 0.0, 1.5);
    let w0 = Vec3::new(0.0, 0.0, 250.0);
    let with = roll(&stock, glo, true, (-0.05, 0.0), v0, w0, 1.0, 200);
    let without = roll(&stock, glo, false, (-0.05, 0.0), v0, w0, 1.0, 200);
    assert_ne!(with, without, "Magnus had no effect on uv_globall");
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, .. ProptestConfig::default() })]

    #[test]
    fn prop_v1_zero_coefficients_bit_identical(
        which in 0usize..7,
        x0 in -0.1f32..0.0, y0 in -0.03f32..0.03,
        vx in 0.2f32..3.0, vy in -0.5f32..0.5, vz in 0.0f32..1.0,
        w in prop::array::uniform3(-200.0f32..200.0),
        power in 0.05f32..=1.0,
    ) {
        let z = zeroed();
        let v0 = Vec3::new(vx, vy, vz);
        let with = roll(&z, which, true, (x0, y0), v0, Vec3::from(w), power, 400);
        let without = roll(&z, which, false, (x0, y0), v0, Vec3::from(w), power, 400);
        prop_assert_eq!(with, without);
    }
}

// ------------------------------------------------------------- determinism

/// One 3 s run of the busy table with the stock profiles and both hooks:
/// a digest of every tick's state, events, switches and magnetisations.
fn stock_run(hooks: Hooks) -> (u64, Vec<u32>, Vec<u32>) {
    let stock = builtin();
    let mut sc = busy(&stock, hooks);
    let mut ev = BallEvents::new();
    let mut h = DefaultHasher::new();
    let mut sw = [0u64; 4];
    let mut m_bits = Vec::new();
    for tick in 0..3000u32 {
        sc.solver.set_magnet_power(sc.magnet, busy_power(tick));
        if tick == 1200 {
            // A ball leaves and a new one takes its slot.
            let old = sc.solver.remove_ball(pin_physics::ball::BallHandle(0)).unwrap();
            let mut fresh = Ball::new(pid("jumbo_steel"), old.position + Vec3::new(0.0, 0.0, 0.01));
            fresh.velocity = Vec3::new(0.2, 0.3, 0.0);
            assert_eq!(sc.solver.add_ball(fresh), Some(pin_physics::ball::BallHandle(0)));
        }
        ev.clear();
        let geo = Geometry {
            world: &sc.world,
            guides: &sc.guides,
            wireforms: &sc.wireforms,
        };
        sc.solver.step(&geo, DT, &mut ev);
        hash_events(&mut h, &ev);
        sc.solver.sensor_switches(&mut sw);
        sw.hash(&mut h);
        solver_bits(&sc.solver).hash(&mut h);
        if let Some(p) = &sc.probe {
            for (handle, _) in sc.solver.balls() {
                let m = mag(p, handle);
                assert!((0.0..=1.0).contains(&m), "tick {tick}: M = {m}");
                m.to_bits().hash(&mut h);
            }
        }
    }
    if let Some(p) = &sc.probe {
        for (handle, _) in sc.solver.balls() {
            m_bits.push(mag(p, handle).to_bits());
        }
    }
    for (_, b) in sc.solver.balls() {
        assert!(b.position.is_finite() && b.velocity.is_finite());
    }
    (h.finish(), solver_bits(&sc.solver), m_bits)
}

#[test]
fn ten_runs_with_active_hooks_are_bit_identical() {
    let first = stock_run(Hooks::Before);
    for _ in 0..9 {
        assert_eq!(stock_run(Hooks::Before), first);
    }
    // The hooks were active in that scenario.
    let plain = stock_run(Hooks::None);
    assert_ne!(plain.1, first.1, "the hooks changed nothing in the scenario");
}
