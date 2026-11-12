//! Independent spec tests (Subphase 2.2): solver invariants (energy,
//! tunnelling, resting stability) and the §3.2 contracts (material pairs,
//! electromagnets, eddy/opto sensors, gravity, external-force hooks, layers,
//! guides and wireforms).
//!
//! Black-box against the FIXED API of `pin_physics::ball`.

#[path = "spec_ball_common.rs"]
mod common;

use common::*;
use glam::{Quat, Vec3};
use pin_core::{BallProfile, Material, SwitchId};
use pin_physics::ball::{
    Ball, BallEvent, BallHandle, ExternalForce, ImpactTarget, Magnet, Sensor, SensorKind,
};
use pin_physics::world::{
    CubicSpline, GuideCurve, StaticShape, StaticWorld, StaticWorldBuilder, Wireform, layers,
};
use proptest::prelude::*;
use std::sync::{Arc, Mutex};

// ------------------------------------------------------------ tunnelling

/// A 2 mm thin wall (x ∈ [-1 mm, +1 mm]) spanning y, z ∈ [-1, 1] m.
fn thin_wall_world() -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        cuboid(Vec3::ZERO, Vec3::new(0.001, 1.0, 1.0)),
        layers::ALL,
        true,
    );
    b.build()
}

fn shoot_at_thin_wall(name: &str, speed: f32, dir: Vec3, w: Vec3, gravity: bool) {
    let world = thin_wall_world();
    let g = geo(&world);
    let mut s = if gravity {
        solver()
    } else {
        solver_with_gravity(Vec3::ZERO)
    };
    let r = profile(name).radius_m();
    let mut b = ball(name, Vec3::new(-0.05 - r, 0.0, 0.0));
    let dir = dir.normalize();
    b.velocity = dir * speed;
    b.angular_velocity = w;
    let h = s.add_ball(b).unwrap();
    let mut ev = events();
    // Long enough to reach the wall (0.05 m along x) and bounce off it.
    let vx = (dir * speed).x;
    let ticks = (0.05 / vx / DT) as usize + 30;
    for tick in 0..ticks {
        ev.clear();
        s.step(&g, DT, &mut ev);
        let p = s.ball(h).unwrap().position;
        assert!(
            p.x <= -0.001 - r + 1e-4,
            "{name} at {speed} m/s dir {dir:?}: tunnelled/penetrated, x = {} at tick {tick}",
            p.x
        );
    }
    // It bounced (steel–wood pair restitution 0.25): now moving away from the wall.
    let v = s.ball(h).unwrap().velocity;
    assert!(
        v.x < 0.0,
        "{name} at {speed} m/s dir {dir:?}: no rebound off the wall, v = {v:?}"
    );
}

/// A ball whose previous tick ended a few micrometres short of a surface and
/// that is still closing fast must bounce with the pair restitution and
/// report an Impact (brief: only impacts below 0.05 m/s go unreported).
#[test]
fn impact_starting_within_micrometres_of_surface_still_bounces() {
    let world = floor_world();
    let g = geo(&world);
    let e = defaults()
        .material_pair(Material::Steel, Material::Wood)
        .unwrap()
        .restitution;
    for gap in [1e-5f32, 2e-5, 1e-4] {
        for speed in [0.2f32, 0.7, 3.0] {
            let mut s = solver_with_gravity(Vec3::ZERO);
            let mut b = ball("standard_steel", Vec3::new(0.0, 0.0, 0.0135 + gap));
            b.velocity = Vec3::new(0.0, 0.0, -speed);
            let h = s.add_ball(b).unwrap();
            let mut ev = events();
            for _ in 0..3 {
                s.step(&g, DT, &mut ev);
            }
            let vz = s.ball(h).unwrap().velocity.z;
            assert!(
                (vz - e * speed).abs() <= 0.02 * speed,
                "gap {gap} m, {speed} m/s: rebound vz = {vz}, expected {}",
                e * speed
            );
            assert!(
                ev.as_slice()
                    .iter()
                    .any(|x| matches!(x, BallEvent::Impact { .. })),
                "gap {gap} m, {speed} m/s: no Impact event"
            );
        }
    }
}

#[test]
fn no_tunnelling_head_on_up_to_30_m_s() {
    for name in ["standard_steel", "mini_steel", "uv_globall", "jumbo_steel"] {
        for &v in &[1.0f32, 5.0, 10.0, 20.0, 25.0, 30.0] {
            shoot_at_thin_wall(name, v, Vec3::X, Vec3::ZERO, false);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, .. ProptestConfig::default() })]

    #[test]
    fn no_tunnelling_sweep(
        which in 0usize..4,
        v in 0.5f32..30.0,
        dy in -1.0f32..1.0, dz in -1.0f32..1.0,
        w in prop::array::uniform3(-300.0f32..300.0),
        gravity in any::<bool>(),
    ) {
        let name = ["standard_steel", "mini_steel", "ceramic_powerball", "jumbo_steel"][which];
        shoot_at_thin_wall(name, v, Vec3::new(1.0, dy, dz), Vec3::from(w), gravity);
    }
}

#[test]
fn no_tunnelling_through_thin_wall_tilted() {
    // Wall rotated 30° about z; ball aimed along its normal at 30 m/s.
    let rot = Quat::from_rotation_z(std::f32::consts::FRAC_PI_6);
    let n = rot * Vec3::X;
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cuboid {
            center: Vec3::ZERO,
            half_extents: Vec3::new(0.001, 1.0, 1.0),
            rotation: rot,
        },
        layers::ALL,
        true,
    );
    let world = b.build();
    let g = geo(&world);
    let mut s = solver_with_gravity(Vec3::ZERO);
    let r = 0.0135;
    let mut bl = ball("standard_steel", -n * 0.1);
    bl.velocity = n * 30.0;
    let h = s.add_ball(bl).unwrap();
    let mut ev = events();
    for _ in 0..30 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        let p = s.ball(h).unwrap().position;
        assert!(p.dot(n) <= -0.001 - r + 1e-4, "tunnelled: {p:?}");
    }
}

// ------------------------------------------------------------ resting stability

fn rest_drift(name: &str, xy: Vec3, floor_top: f32) -> f32 {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        cuboid(
            Vec3::new(0.0, 0.0, floor_top - 0.05),
            Vec3::new(2.0, 2.0, 0.05),
        ),
        layers::ALL,
        true,
    );
    let world = b.build();
    let g = geo(&world);
    let mut s = solver();
    let r = profile(name).radius_m();
    let p0 = Vec3::new(xy.x, xy.y, floor_top + r);
    let h = s.add_ball(ball(name, p0)).unwrap();
    let mut ev = events();
    let mut max = 0.0f32;
    for _ in 0..10_000 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        let p = s.ball(h).unwrap().position;
        max = max.max((p - p0).length());
        // No impact events from a resting ball (below the 0.05 m/s threshold).
        for e in ev.as_slice() {
            assert!(
                !matches!(e, BallEvent::Impact { .. }),
                "resting ball reported {e:?}"
            );
        }
    }
    max
}

#[test]
fn resting_ball_drift_below_1um_over_10s() {
    for name in [
        "standard_steel",
        "ceramic_powerball",
        "uv_globall",
        "mini_steel",
        "jumbo_xl_resin",
    ] {
        let d = rest_drift(name, Vec3::new(0.1, -0.2, 0.0), 0.0);
        assert!(d < 1e-6, "{name}: drift {d:e} m over 10 s");
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 6, .. ProptestConfig::default() })]

    #[test]
    fn resting_ball_drift_sweep(
        which in 0usize..7,
        x in -1.0f32..1.0, y in -1.0f32..1.0,
        top in -0.2f32..0.2,
    ) {
        let name = pin_core::profiles::BUILTIN_PROFILE_NAMES[which];
        let d = rest_drift(name, Vec3::new(x, y, 0.0), top);
        prop_assert!(d < 1e-6, "{} drift {:e}", name, d);
    }
}

// ------------------------------------------------------------ energy

/// Gravity off: a ball thrown with spin into a box of walls and posts (all
/// frictional, inelastic contacts) never gains kinetic energy.
fn energy_box(name: &str, v: Vec3, w: Vec3, materials: [Material; 3]) {
    let mut b = StaticWorldBuilder::new();
    let walls = [
        b.add_collider(
            cuboid(Vec3::new(0.0, 0.0, -0.11), Vec3::new(0.2, 0.2, 0.01)),
            layers::ALL,
            true,
        ),
        b.add_collider(
            cuboid(Vec3::new(0.21, 0.0, 0.0), Vec3::new(0.01, 0.2, 0.2)),
            layers::ALL,
            true,
        ),
        b.add_collider(
            StaticShape::Cylinder {
                base: Vec3::new(-0.05, 0.05, -0.1),
                axis: Vec3::new(0.0, 0.0, 0.2),
                radius: 0.012,
            },
            layers::ALL,
            true,
        ),
    ];
    b.add_collider(
        cuboid(Vec3::new(-0.21, 0.0, 0.0), Vec3::new(0.01, 0.2, 0.2)),
        layers::ALL,
        true,
    );
    b.add_collider(
        cuboid(Vec3::new(0.0, 0.21, 0.0), Vec3::new(0.2, 0.01, 0.2)),
        layers::ALL,
        true,
    );
    b.add_collider(
        cuboid(Vec3::new(0.0, -0.21, 0.0), Vec3::new(0.2, 0.01, 0.2)),
        layers::ALL,
        true,
    );
    b.add_collider(
        cuboid(Vec3::new(0.0, 0.0, 0.11), Vec3::new(0.2, 0.2, 0.01)),
        layers::ALL,
        true,
    );
    let world = b.build();
    let g = geo(&world);
    let profs = profiles();
    let mut s = solver_with_gravity(Vec3::ZERO);
    for (c, m) in walls.into_iter().zip(materials) {
        s.set_collider_material(c, m);
    }
    let mut bl = ball(name, Vec3::new(0.0, 0.0, 0.0));
    bl.velocity = v;
    bl.angular_velocity = w;
    s.add_ball(bl).unwrap();
    // A second ball so ball-ball contacts are included.
    let mut b2 = ball("mini_steel", Vec3::new(0.1, -0.1, 0.05));
    b2.velocity = -v * 0.5;
    s.add_ball(b2).unwrap();
    let mut ev = events();
    let mut prev = total_kinetic(&s, &profs);
    let mut impacts = 0;
    for tick in 0..1000 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        impacts += ev
            .as_slice()
            .iter()
            .filter(|e| matches!(e, BallEvent::Impact { .. }))
            .count();
        let e = total_kinetic(&s, &profs);
        assert!(
            e <= prev * (1.0 + 1e-6) + 1e-12,
            "{name} v={v:?} w={w:?}: KE rose {prev:e} -> {e:e} at tick {tick}"
        );
        prev = e;
        for (_, b) in s.balls() {
            let p = b.position;
            assert!(
                p.x.abs() < 0.2 && p.y.abs() < 0.2 && p.z.abs() < 0.1,
                "ball escaped the box: {p:?}"
            );
        }
    }
    assert!(impacts > 0, "no contacts happened");
}

#[test]
fn energy_non_increasing_frictional_contacts() {
    energy_box(
        "standard_steel",
        Vec3::new(3.0, 1.0, -2.0),
        Vec3::new(0.0, 200.0, 50.0),
        [Material::Wood, Material::Rubber, Material::Steel],
    );
    energy_box(
        "uv_globall",
        Vec3::new(-2.0, 4.0, 1.0),
        Vec3::new(-100.0, 0.0, 0.0),
        [Material::Rubber, Material::Rubber, Material::Plastic],
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 16, .. ProptestConfig::default() })]

    #[test]
    fn energy_non_increasing_sweep(
        which in 0usize..7,
        v in prop::array::uniform3(-8.0f32..8.0),
        w in prop::array::uniform3(-300.0f32..300.0),
        mats in prop::array::uniform3(0usize..7),
    ) {
        let name = pin_core::profiles::BUILTIN_PROFILE_NAMES[which];
        // jumbo_xl_resin is 79 mm: still fits the 400 mm box.
        let v = Vec3::from(v);
        prop_assume!(v.length() > 0.3);
        let m = mats.map(|i| Material::ALL[i]);
        energy_box(name, v, Vec3::from(w), m);
    }
}

#[test]
fn rolling_ball_on_flat_floor_loses_energy_monotonically() {
    // Gravity on, flat floor (potential energy constant): KE never rises and
    // rolling resistance eventually slows the ball.
    let world = floor_world();
    let g = geo(&world);
    let profs = profiles();
    let mut s = solver();
    let r = 0.0135;
    let mut b = ball("standard_steel", Vec3::new(0.0, 0.0, r));
    b.velocity = Vec3::new(1.0, 0.0, 0.0);
    b.angular_velocity = Vec3::new(0.0, 1.0 / r, 0.0); // rolling: v + ω × (-r z) = 0
    let h = s.add_ball(b).unwrap();
    assert!(slip(s.ball(h).unwrap(), r, Vec3::Z) < 1e-5);
    let mut ev = events();
    let e0 = total_kinetic(&s, &profs);
    let mut prev = e0;
    for tick in 0..2000 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        let e = total_kinetic(&s, &profs);
        assert!(e <= prev * (1.0 + 1e-6), "KE rose at tick {tick}");
        prev = e;
        let bl = s.ball(h).unwrap();
        assert!(bl.velocity.x >= 0.0, "ball reversed");
        assert!(slip(bl, r, Vec3::Z) < 1e-3, "rolling ball started slipping");
    }
    assert!(prev < e0, "rolling resistance had no effect over 2 s");
    let x = s.ball(h).unwrap().position.x;
    assert!(x > 1.5, "rolling ball travelled only {x} m in 2 s");
}

// ------------------------------------------------------------ material pairs

/// Normal restitution off a flat surface of `mat`, gravity off, 2 m/s head-on.
fn rebound(name: &str, mat: Option<Material>) -> (f32, Vec<BallEvent>) {
    let world = floor_world();
    let g = geo(&world);
    let mut s = solver_with_gravity(Vec3::ZERO);
    assert_eq!(world.collider_count(), 1);
    if let Some(m) = mat {
        // The first (only) collider has insertion index 0; obtain its handle
        // from an impact event of a probe run instead of constructing it.
        let h = probe_floor_handle();
        s.set_collider_material(h, m);
    }
    let r = profile(name).radius_m();
    let mut b = ball(name, Vec3::new(0.0, 0.0, r + 0.001));
    b.velocity = Vec3::new(0.0, 0.0, -2.0);
    let h = s.add_ball(b).unwrap();
    let mut ev = events();
    let mut all = Vec::new();
    for _ in 0..5 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        all.extend_from_slice(ev.as_slice());
    }
    (s.ball(h).unwrap().velocity.z / 2.0, all)
}

fn probe_floor_handle() -> pin_physics::world::ColliderHandle {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(floor_shape(), layers::ALL, true)
}

#[test]
fn restitution_comes_from_material_pair_table() {
    let d = defaults();
    let cases = [
        ("standard_steel", None, Material::Steel, Material::Wood),
        (
            "standard_steel",
            Some(Material::Rubber),
            Material::Steel,
            Material::Rubber,
        ),
        (
            "standard_steel",
            Some(Material::Steel),
            Material::Steel,
            Material::Steel,
        ),
        ("ceramic_powerball", None, Material::Ceramic, Material::Wood),
        (
            "ceramic_powerball",
            Some(Material::Rubber),
            Material::Ceramic,
            Material::Rubber,
        ),
        (
            "uv_globall",
            Some(Material::Wood),
            Material::Plastic,
            Material::Wood,
        ),
        (
            "uv_globall",
            Some(Material::Rubber),
            Material::Plastic,
            Material::Rubber,
        ),
        ("jumbo_xl_resin", None, Material::Resin, Material::Wood),
    ];
    for (name, mat, a, b) in cases {
        let e = d.material_pair(a, b).unwrap().restitution;
        let (got, evs) = rebound(name, mat);
        assert!(
            (got - e).abs() <= 0.02,
            "{name} on {mat:?}: rebound ratio {got}, pair restitution {e}"
        );
        // One impact event at ~2 m/s against the floor collider.
        let imp: Vec<_> = evs
            .iter()
            .filter_map(|e| match *e {
                BallEvent::Impact {
                    other,
                    normal_speed,
                    ..
                } => Some((other, normal_speed)),
                _ => None,
            })
            .collect();
        assert_eq!(imp.len(), 1, "{name}: impacts {imp:?}");
        assert_eq!(imp[0].0, ImpactTarget::Collider(probe_floor_handle()));
        assert!((imp[0].1 - 2.0).abs() < 0.05, "normal speed {}", imp[0].1);
    }
}

#[test]
fn missing_pair_falls_back_to_profile_restitution() {
    let d = defaults();
    // resin–rubber and ceramic–steel are not in the table.
    for (name, mat) in [
        ("jumbo_xl_resin", Material::Rubber),
        ("ceramic_powerball", Material::Steel),
        ("ceramic_powerball", Material::Acrylic),
    ] {
        let p = profile(name);
        assert!(d.material_pair(p.material, mat).is_none());
        let (got, _) = rebound(name, Some(mat));
        assert!(
            (got - p.restitution).abs() <= 0.02,
            "{name} on {mat:?}: rebound {got}, profile restitution {}",
            p.restitution
        );
    }
}

// ------------------------------------------------------------ magnets

fn magnet_kick(name: &str, offset: Vec3, power: f32, mask: u8, layer: u8) -> Vec3 {
    let mut s = solver_with_gravity(Vec3::ZERO);
    let id = s.add_magnet(Magnet {
        position: Vec3::ZERO,
        k_coil: 1.0e-5,
        d_min: 0.01,
        range: 0.05,
        layer_mask: mask,
    });
    s.set_magnet_power(id, power);
    let mut b = ball(name, offset);
    b.layer = layer;
    let h = s.add_ball(b).unwrap();
    let world = empty_world();
    let g = geo(&world);
    let mut ev = events();
    s.step(&g, DT, &mut ev);
    s.ball(h).unwrap().velocity
}

#[test]
fn magnet_force_follows_inverse_cube_law() {
    let p = profile("standard_steel");
    let k = 1.0e-5f32;
    for &(d, dir) in &[
        (0.03f32, Vec3::X),
        (0.02, Vec3::new(0.0, -1.0, 0.0)),
        (0.045, Vec3::new(1.0, 1.0, 1.0).normalize()),
    ] {
        for &pw in &[1.0f32, 0.5, 0.25] {
            let v = magnet_kick("standard_steel", dir * d, pw, layers::ALL, 0);
            // Force toward the magnet: F = c·P·K / d³.
            let f = p.magnetic_coupling * pw * k / (d * d * d);
            let expected = -dir * (f / p.mass_kg * DT);
            assert!(
                (v - expected).length() <= 0.03 * expected.length(),
                "d={d} P={pw}: Δv {v:?}, expected {expected:?}"
            );
        }
    }
}

#[test]
fn magnet_force_is_capped_inside_d_min() {
    let p = profile("standard_steel");
    let v = magnet_kick(
        "standard_steel",
        Vec3::new(0.005, 0.0, 0.0),
        1.0,
        layers::ALL,
        0,
    );
    let f = 1.0e-5 / (0.01f32 * 0.01 * 0.01);
    let expected = -f / p.mass_kg * DT;
    assert!(
        (v.x - expected).abs() <= 0.03 * expected.abs(),
        "Δv {v:?}, expected x {expected}"
    );
}

#[test]
fn magnet_has_no_effect_beyond_range_off_layer_or_unpowered() {
    // Beyond range: exactly zero.
    assert_eq!(
        magnet_kick(
            "standard_steel",
            Vec3::new(0.06, 0.0, 0.0),
            1.0,
            layers::ALL,
            0
        ),
        Vec3::ZERO
    );
    // Power 0 and negative power clamp to 0: exactly zero.
    assert_eq!(
        magnet_kick(
            "standard_steel",
            Vec3::new(0.03, 0.0, 0.0),
            0.0,
            layers::ALL,
            0
        ),
        Vec3::ZERO
    );
    assert_eq!(
        magnet_kick(
            "standard_steel",
            Vec3::new(0.03, 0.0, 0.0),
            -1.0,
            layers::ALL,
            0
        ),
        Vec3::ZERO
    );
    // Ball on layer 1 (upper deck), magnet only on the main deck.
    assert_eq!(
        magnet_kick(
            "standard_steel",
            Vec3::new(0.03, 0.0, 0.0),
            1.0,
            layers::MAIN_DECK,
            1
        ),
        Vec3::ZERO
    );
    // c = 0 profiles: exactly zero at any distance.
    for name in ["ceramic_powerball", "uv_globall", "jumbo_xl_resin"] {
        assert_eq!(
            magnet_kick(name, Vec3::new(0.003, 0.0, 0.0), 1.0, layers::ALL, 0),
            Vec3::ZERO
        );
    }
}

#[test]
fn magnet_power_clamps_above_one() {
    let a = magnet_kick(
        "standard_steel",
        Vec3::new(0.03, 0.0, 0.0),
        1.0,
        layers::ALL,
        0,
    );
    let b = magnet_kick(
        "standard_steel",
        Vec3::new(0.03, 0.0, 0.0),
        5.0,
        layers::ALL,
        0,
    );
    assert_eq!(a, b);
}

// ------------------------------------------------------------ sensors

fn sensor_state(name: &str, kind_eddy: bool, pos: Vec3) -> (bool, Vec<BallEvent>) {
    let mut s = solver_with_gravity(Vec3::ZERO);
    s.add_sensor(Sensor {
        switch: SwitchId(70),
        kind: if kind_eddy {
            SensorKind::Eddy
        } else {
            SensorKind::Opto
        },
        center: Vec3::ZERO,
        radius: 0.02,
        layer_mask: layers::ALL,
    });
    s.add_ball(ball(name, pos)).unwrap();
    let world = empty_world();
    let g = geo(&world);
    let mut ev = events();
    s.step(&g, DT, &mut ev);
    let mut out = [0u64; 4];
    s.sensor_switches(&mut out);
    (out[1] & (1 << 6) != 0, ev.as_slice().to_vec())
}

#[test]
fn eddy_sensors_close_only_for_conductive_balls_opto_for_all() {
    for p in profiles() {
        let name = p.name.clone();
        let (eddy, evs) = sensor_state(&name, true, Vec3::ZERO);
        assert_eq!(eddy, p.conductive, "eddy sensor with {name}");
        let edge = evs.iter().any(|e| {
            matches!(e, BallEvent::SensorEdge { switch, closed: true } if *switch == SwitchId(70))
        });
        assert_eq!(edge, p.conductive, "eddy edge event with {name}: {evs:?}");
        let (opto, evs) = sensor_state(&name, false, Vec3::ZERO);
        assert!(opto, "opto sensor with {name}");
        assert!(evs.iter().any(|e| matches!(
            e,
            BallEvent::SensorEdge { switch, closed: true } if *switch == SwitchId(70)
        )));
        // Out of range: open.
        let (far, evs) = sensor_state(&name, false, Vec3::new(0.5, 0.0, 0.0));
        assert!(!far, "{name} far away closed the opto");
        assert!(
            !evs.iter()
                .any(|e| matches!(e, BallEvent::SensorEdge { .. }))
        );
    }
}

#[test]
fn sensor_switches_touches_only_own_bits_and_edges_on_exit() {
    let mut s = solver_with_gravity(Vec3::ZERO);
    s.add_sensor(Sensor {
        switch: SwitchId(3),
        kind: SensorKind::Opto,
        center: Vec3::ZERO,
        radius: 0.02,
        layer_mask: layers::ALL,
    });
    s.add_sensor(Sensor {
        switch: SwitchId(200),
        kind: SensorKind::Eddy,
        center: Vec3::new(5.0, 0.0, 0.0),
        radius: 0.02,
        layer_mask: layers::ALL,
    });
    let mut b = ball("standard_steel", Vec3::new(-0.01, 0.0, 0.0));
    b.velocity = Vec3::new(1.0, 0.0, 0.0);
    s.add_ball(b).unwrap();
    let world = empty_world();
    let g = geo(&world);
    let mut ev = events();
    let mut edges = Vec::new();
    let mut out = [u64::MAX; 4];
    s.step(&g, DT, &mut ev);
    edges.extend_from_slice(ev.as_slice());
    s.sensor_switches(&mut out);
    // Own bits: 3 closed, 200 open; everything else untouched (still set).
    let mut expect = [u64::MAX; 4];
    expect[200 / 64] &= !(1u64 << (200 % 64));
    assert_eq!(out, expect);
    let mut zero = [0u64; 4];
    s.sensor_switches(&mut zero);
    assert_eq!(zero, [1u64 << 3, 0, 0, 0]);
    for _ in 0..100 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        edges.extend_from_slice(ev.as_slice());
    }
    let mut out = [u64::MAX; 4];
    s.sensor_switches(&mut out);
    assert_eq!(
        out[0] & (1 << 3),
        0,
        "opto still closed after the ball left"
    );
    let sw: Vec<_> = edges
        .iter()
        .filter_map(|e| match *e {
            BallEvent::SensorEdge { switch, closed } => Some((switch, closed)),
            _ => None,
        })
        .collect();
    assert_eq!(sw, vec![(SwitchId(3), true), (SwitchId(3), false)]);
}

// ------------------------------------------------------------ gravity, free flight

#[test]
fn free_flight_is_ballistic_and_spin_is_preserved() {
    let gvec = Vec3::new(0.3, -1.2, -9.81);
    let mut s = solver_with_gravity(gvec);
    let mut b = ball("standard_steel", Vec3::ZERO);
    b.velocity = Vec3::new(1.0, 2.0, 3.0);
    b.angular_velocity = Vec3::new(0.0, 0.0, 10.0);
    let h = s.add_ball(b).unwrap();
    let world = empty_world();
    let g = geo(&world);
    let mut ev = events();
    for _ in 0..1000 {
        ev.clear();
        s.step(&g, DT, &mut ev);
    }
    let b = s.ball(h).unwrap();
    let v_exp = Vec3::new(1.0, 2.0, 3.0) + gvec;
    assert!((b.velocity - v_exp).length() < 1e-3, "v {:?}", b.velocity);
    let p_exp = Vec3::new(1.0, 2.0, 3.0) + 0.5 * gvec;
    // Any first-order integrator is within |g|·dt·t/2 ≈ 5 mm after 1 s.
    assert!((b.position - p_exp).length() < 6e-3, "p {:?}", b.position);
    assert!(
        (b.angular_velocity - Vec3::new(0.0, 0.0, 10.0)).length() < 1e-4,
        "spin changed in free flight: {:?}",
        b.angular_velocity
    );
    // 10 rad about z in 1 s.
    let q_exp = Quat::from_rotation_z(10.0);
    let q = b.orientation;
    assert!(
        (q.length() - 1.0).abs() < 1e-4,
        "orientation not unit: {q:?}"
    );
    assert!(
        q.dot(q_exp).abs() > 1.0 - 1e-4,
        "orientation {q:?}, expected {q_exp:?}"
    );
}

#[test]
fn default_gravity_and_config_from_defaults() {
    let d = defaults();
    let c = pin_physics::ball::BallSolverConfig::from_defaults(&d);
    assert_eq!(c.gravity, Vec3::new(0.0, 0.0, -9.81));
    assert_eq!(c.pairs, d.material_pairs);
    assert_eq!(c.rolling_resistance, d.tunables.rolling_resistance.default);
    assert_eq!(c.max_substeps, 64);
}

#[test]
fn apply_impulse_changes_velocity_by_j_over_m() {
    for p in profiles() {
        let mut s = solver_with_gravity(Vec3::ZERO);
        let h = s
            .add_ball(Ball::new(common::pid(&p.name), Vec3::ZERO))
            .unwrap();
        let j = Vec3::new(0.01, -0.02, 0.005);
        s.apply_impulse(h, j);
        let world = empty_world();
        let g = geo(&world);
        let mut ev = events();
        s.step(&g, DT, &mut ev);
        let b = s.ball(h).unwrap();
        let exp = j / p.mass_kg;
        assert!(
            (b.velocity - exp).length() <= 1e-5 * exp.length(),
            "{}: v {:?} expected {exp:?}",
            p.name,
            b.velocity
        );
        assert_eq!(
            b.angular_velocity,
            Vec3::ZERO,
            "impulse through centre spun the ball"
        );
    }
}

// ------------------------------------------------------------ layers

#[test]
fn balls_collide_only_with_colliders_on_their_layer() {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        cuboid(Vec3::new(0.1, 0.0, 0.0), Vec3::new(0.01, 0.5, 0.5)),
        layers::MAIN_DECK,
        true,
    );
    let world = b.build();
    let g = geo(&world);
    for (layer, should_bounce) in [(0u8, true), (1u8, false), (3u8, false)] {
        let mut s = solver_with_gravity(Vec3::ZERO);
        let mut bl = ball("standard_steel", Vec3::ZERO);
        bl.velocity = Vec3::new(2.0, 0.0, 0.0);
        bl.layer = layer;
        let h = s.add_ball(bl).unwrap();
        let mut ev = events();
        for _ in 0..200 {
            ev.clear();
            s.step(&g, DT, &mut ev);
        }
        let b = s.ball(h).unwrap();
        assert_eq!(b.layer, layer);
        if should_bounce {
            assert!(b.velocity.x < 0.0 && b.position.x < 0.09);
        } else {
            assert!(
                b.position.x > 0.3,
                "layer {layer} ball blocked: {:?}",
                b.position
            );
        }
    }
}

// ------------------------------------------------------------ external forces

struct ConstAccel(Vec3);
impl ExternalForce for ConstAccel {
    fn force(&mut self, _h: BallHandle, _b: &Ball, p: &BallProfile, _dt: f32) -> Vec3 {
        self.0 * p.mass_kg
    }
}

struct Recorder(Arc<Mutex<Vec<(u8, f32)>>>);
impl ExternalForce for Recorder {
    fn force(&mut self, h: BallHandle, _b: &Ball, _p: &BallProfile, dt: f32) -> Vec3 {
        self.0.lock().unwrap().push((h.0, dt));
        Vec3::ZERO
    }
}

#[test]
fn external_force_acts_like_extra_acceleration() {
    let world = empty_world();
    let g = geo(&world);
    let a = Vec3::new(0.0, 2.0, -1.0);
    let mut s = solver_with_gravity(Vec3::ZERO);
    s.add_external_force(Box::new(ConstAccel(a)));
    let hs: Vec<_> = ["standard_steel", "jumbo_xl_resin", "mini_steel"]
        .iter()
        .enumerate()
        .map(|(i, n)| s.add_ball(ball(n, Vec3::new(i as f32, 0.0, 0.0))).unwrap())
        .collect();
    let mut ev = events();
    for _ in 0..500 {
        ev.clear();
        s.step(&g, DT, &mut ev);
    }
    for h in hs {
        let v = s.ball(h).unwrap().velocity;
        assert!(
            (v - a * 0.5).length() <= 1e-4 * (a * 0.5).length(),
            "v {v:?}"
        );
    }
}

#[test]
fn external_force_called_per_ball_with_dt_summing_to_tick() {
    let world = floor_world();
    let g = geo(&world);
    let log = Arc::new(Mutex::new(Vec::with_capacity(1 << 16)));
    let mut s = solver();
    s.add_external_force(Box::new(Recorder(log.clone())));
    let r = 0.0135;
    let mut handles = Vec::new();
    for i in 0..3 {
        let mut b = ball("standard_steel", Vec3::new(i as f32 * 0.1, 0.0, r + 0.01));
        b.velocity = Vec3::new(5.0, 0.0, -3.0);
        handles.push(s.add_ball(b).unwrap().0);
    }
    let mut ev = events();
    for tick in 0..50 {
        log.lock().unwrap().clear();
        ev.clear();
        s.step(&g, DT, &mut ev);
        let l = log.lock().unwrap();
        for &h in &handles {
            let sum: f32 = l.iter().filter(|(x, _)| *x == h).map(|(_, d)| *d).sum();
            assert!(
                (sum - DT).abs() < 1e-6,
                "tick {tick}: ball {h} hook dt sum {sum}"
            );
        }
        assert!(l.iter().all(|(x, _)| handles.contains(x)));
        // The first call of the tick is for the lowest handle.
        assert_eq!(l[0].0, handles[0]);
    }
}

#[test]
fn zero_hooks_and_bit_exact_reproduction() {
    // Same scenario twice in one process, no hooks: identical bits.
    let run = || {
        let world = floor_world();
        let g = geo(&world);
        let mut s = solver();
        let mut b = ball("standard_steel", Vec3::new(0.0, 0.0, 0.03));
        b.velocity = Vec3::new(1.0, 0.5, 0.0);
        b.angular_velocity = Vec3::new(5.0, -20.0, 3.0);
        let h = s.add_ball(b).unwrap();
        let mut ev = events();
        for _ in 0..500 {
            ev.clear();
            s.step(&g, DT, &mut ev);
        }
        state_bits(s.ball(h).unwrap())
    };
    assert_eq!(run(), run());
}

// ------------------------------------------------------------ guides

/// Closed circular orbit wall of radius `rad` around the origin, on a floor.
fn orbit(rad: f32) -> GuideCurve {
    let pts: Vec<Vec3> = (0..24)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 24.0;
            Vec3::new(rad * libm::cosf(a), rad * libm::sinf(a), 0.0)
        })
        .chain(std::iter::once(Vec3::new(rad, 0.0, 0.0)))
        .collect();
    GuideCurve::new(CubicSpline::interpolate_closed(&pts).unwrap(), 0.05).unwrap()
}

#[test]
fn ball_cannot_pass_through_guide_wall() {
    let world = floor_world();
    let guides = [orbit(0.2)];
    let g = geo_full(&world, &guides, &[]);
    let r = 0.0135;
    for &v in &[1.0f32, 5.0, 15.0, 30.0] {
        for k in 0..6 {
            let a = k as f32;
            let dir = Vec3::new(libm::cosf(a), libm::sinf(a), 0.0);
            let mut s = solver();
            let mut b = ball("standard_steel", Vec3::new(0.0, 0.0, r));
            b.velocity = dir * v;
            let h = s.add_ball(b).unwrap();
            let mut ev = events();
            let mut guide_hit = false;
            // Reach the wall (≈ 0.19 m) and bounce back.
            let ticks = (0.19 / v / DT) as usize + 100;
            for tick in 0..ticks {
                ev.clear();
                s.step(&g, DT, &mut ev);
                guide_hit |= ev.as_slice().iter().any(|e| {
                    matches!(
                        e,
                        BallEvent::Impact {
                            other: ImpactTarget::Guide(0),
                            ..
                        }
                    )
                });
                let p = s.ball(h).unwrap().position;
                let d = Vec3::new(p.x, p.y, 0.0).length();
                assert!(
                    d <= 0.2 - r + 2e-4,
                    "v={v} dir {k}: ball escaped orbit, d={d} at tick {tick}"
                );
            }
            assert!(guide_hit, "v={v} dir {k}: no Guide impact event");
        }
    }
}

#[test]
fn ball_slides_along_curved_guide_without_freezing() {
    let world = floor_world();
    let guides = [orbit(0.25)];
    let g = geo_full(&world, &guides, &[]);
    let r = 0.0135;
    let mut s = solver();
    let rr = 0.25 - r;
    let mut b = ball("standard_steel", Vec3::new(rr, 0.0, r));
    b.velocity = Vec3::new(0.0, 2.0, 0.0);
    b.angular_velocity = Vec3::new(-2.0 / r, 0.0, 0.0); // rolling on the floor
    let h = s.add_ball(b).unwrap();
    let mut ev = events();
    let mut angle = 0.0f32;
    let mut prev = 0.0f32;
    for _ in 0..300 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        let p = s.ball(h).unwrap().position;
        let a = libm::atan2f(p.y, p.x);
        let mut da = a - prev;
        if da < -std::f32::consts::PI {
            da += std::f32::consts::TAU;
        }
        if da > std::f32::consts::PI {
            da -= std::f32::consts::TAU;
        }
        angle += da;
        prev = a;
        let d = Vec3::new(p.x, p.y, 0.0).length();
        assert!(d <= 0.25 - r + 2e-4, "escaped: d={d}");
    }
    // 0.3 s at up to 2 m/s on a 0.236 m radius: ≤ 2.5 rad. Require real progress.
    assert!(
        angle > 1.0,
        "ball froze against the guide: swept {angle} rad"
    );
}

// ------------------------------------------------------------ wireforms

#[test]
fn ball_rolls_down_wireform_without_falling_through() {
    // Straight ramp falling 30 cm over 1 m along +x, flat rails, gauge 20 mm.
    let pts: Vec<Vec3> = (0..11)
        .map(|i| Vec3::new(i as f32 * 0.1, 0.0, 0.3 - i as f32 * 0.03))
        .collect();
    let gauge = 0.020;
    let wf = Wireform::new(
        CubicSpline::interpolate(&pts).unwrap(),
        &[0.0; 11],
        gauge,
        Vec3::Z,
    )
    .unwrap();
    let world = empty_world();
    let wfs = [wf];
    let g = geo_full(&world, &[], &wfs);
    let r = 0.0135;
    let h_above = (r * r - (gauge / 2.0) * (gauge / 2.0)).sqrt();
    let frame = wfs[0].frame_at(1.0);
    let c = wfs[0].centerline().point(1.0) + frame.normal * h_above;
    let mut s = solver();
    let mut b = ball("standard_steel", c);
    b.layer = 2; // wireform layer index
    let h = s.add_ball(b).unwrap();
    let mut ev = events();
    for tick in 0..400 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        let p = s.ball(h).unwrap().position;
        let rails = wfs[0].closest_on_rails(p, None);
        for (k, cp) in rails.iter().enumerate() {
            assert!(
                cp.distance >= r - 5e-4,
                "tick {tick}: ball sank through rail {k} (distance {} < r)",
                cp.distance
            );
        }
        assert!(
            rails[0].distance <= r + 2e-3 && rails[1].distance <= r + 2e-3,
            "tick {tick}: ball left the rails: {:?}",
            p
        );
    }
    let p = s.ball(h).unwrap().position;
    assert!(p.x > c.x + 0.01, "ball did not roll downhill: {p:?}");
}
