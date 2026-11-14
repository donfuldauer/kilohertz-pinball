//! Implementer tests for `pin_physics::world::StaticWorld` (Subphase 2.1).

use arrayvec::ArrayVec;
use glam::{Quat, Vec3};
use pin_physics::world::{
    ColliderHandle, SphereContact, StaticShape, StaticWorld, StaticWorldBuilder, layers,
};
use proptest::prelude::*;

const R: f32 = 0.0135;

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

fn cuboid(center: Vec3, half: Vec3) -> StaticShape {
    StaticShape::Cuboid {
        center,
        half_extents: half,
        rotation: Quat::IDENTITY,
    }
}

fn single(shape: StaticShape) -> (StaticWorld, ColliderHandle) {
    let mut b = StaticWorldBuilder::new();
    let h = b.add_collider(shape, layers::MAIN_DECK, true);
    (b.build(), h)
}

fn contacts(world: &StaticWorld, c: Vec3, r: f32, mask: u8) -> ArrayVec<SphereContact, 16> {
    let mut out = ArrayVec::new();
    world.sphere_contacts(c, r, mask, &mut out);
    out
}

/// A flat square in the z = 0 plane, tessellated into `n × n` quads.
fn grid_mesh(n: usize, size: f32, welded: bool) -> StaticShape {
    let at = |i: usize, j: usize| {
        v(
            size * (i as f32 / n as f32 - 0.5),
            size * (j as f32 / n as f32 - 0.5),
            0.0,
        )
    };
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    if welded {
        for j in 0..=n {
            for i in 0..=n {
                vertices.push(at(i, j));
            }
        }
        let id = |i: usize, j: usize| (j * (n + 1) + i) as u32;
        for j in 0..n {
            for i in 0..n {
                indices.push([id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
                indices.push([id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
            }
        }
    } else {
        for j in 0..n {
            for i in 0..n {
                for tri in [
                    [at(i, j), at(i + 1, j), at(i + 1, j + 1)],
                    [at(i, j), at(i + 1, j + 1), at(i, j + 1)],
                ] {
                    let base = vertices.len() as u32;
                    vertices.extend(tri);
                    indices.push([base, base + 1, base + 2]);
                }
            }
        }
    }
    StaticShape::TriMesh { vertices, indices }
}

#[test]
fn layer_constants() {
    assert_eq!(layers::MAIN_DECK, 1);
    assert_eq!(layers::UPPER_DECK, 2);
    assert_eq!(layers::WIREFORM, 4);
    assert_eq!(layers::SUBWAY, 8);
    assert_eq!(layers::BACKBOX, 16);
    assert_eq!(layers::ALL, 255);
    for i in 0..8u8 {
        assert_eq!(layers::layer_bit(i), 1 << i);
    }
    assert_eq!(layers::layer_bit(8), 0);
    assert_eq!(layers::layer_bit(255), 0);
    assert_eq!(layers::layer_bit(2), layers::WIREFORM);
}

#[test]
fn empty_world() {
    let world = StaticWorldBuilder::new().build();
    assert_eq!(world.collider_count(), 0);
    assert!(
        world
            .cast_sphere(Vec3::ZERO, R, Vec3::X, 1.0, 0xFF)
            .is_none()
    );
    assert!(contacts(&world, Vec3::ZERO, R, 0xFF).is_empty());
}

#[test]
fn head_on_cuboid_time_of_impact_is_analytic() {
    let (world, h) = single(cuboid(v(0.5, 0.0, 0.0), v(0.1, 0.2, 0.3)));
    // Face at x = 0.4; sphere surface starts at x = R.
    let hit = world
        .cast_sphere(Vec3::ZERO, R, v(2.0, 0.0, 0.0), 1.0, layers::MAIN_DECK)
        .unwrap();
    let expected = (0.4 - R) / 2.0;
    assert!(
        (hit.time - expected).abs() < 1e-6,
        "{} vs {expected}",
        hit.time
    );
    assert!((hit.normal - v(-1.0, 0.0, 0.0)).length() < 1e-6);
    assert!((hit.point - v(0.4, 0.0, 0.0)).length() < 1e-6);
    assert_eq!(hit.collider, h);
    // Too short a time window: no hit; exactly long enough: hit.
    assert!(
        world
            .cast_sphere(Vec3::ZERO, R, v(2.0, 0.0, 0.0), 0.19, 0xFF)
            .is_none()
    );
    assert!(
        world
            .cast_sphere(Vec3::ZERO, R, v(2.0, 0.0, 0.0), 0.2, 0xFF)
            .is_some()
    );
    // Moving away or parallel: no hit.
    assert!(
        world
            .cast_sphere(Vec3::ZERO, R, v(-2.0, 0.0, 0.0), 1.0, 0xFF)
            .is_none()
    );
    assert!(
        world
            .cast_sphere(Vec3::ZERO, R, v(0.0, 2.0, 0.0), 1.0, 0xFF)
            .is_none()
    );
    assert!(
        world
            .cast_sphere(Vec3::ZERO, R, Vec3::ZERO, 1.0, 0xFF)
            .is_none()
    );
}

#[test]
fn rotated_cuboid_edge_and_corner() {
    // Rotated 45° about Z: an edge points at -X, at distance 0.5 - 0.1·√2.
    let rot = Quat::from_rotation_z(std::f32::consts::FRAC_PI_4);
    let (world, _) = single(StaticShape::Cuboid {
        center: v(0.5, 0.0, 0.0),
        half_extents: v(0.1, 0.1, 0.1),
        rotation: rot,
    });
    let hit = world
        .cast_sphere(Vec3::ZERO, R, v(1.0, 0.0, 0.0), 1.0, 0xFF)
        .unwrap();
    let edge_x = 0.5 - 0.1 * std::f32::consts::SQRT_2;
    assert!((hit.time - (edge_x - R)).abs() < 2e-6);
    assert!((hit.normal - v(-1.0, 0.0, 0.0)).length() < 1e-4);
    assert!((hit.point - v(edge_x, 0.0, 0.0)).length() < 1e-5);

    // Corner of an axis-aligned box approached along the diagonal.
    let (world, _) = single(cuboid(Vec3::ZERO, Vec3::splat(0.1)));
    let dir = Vec3::splat(-1.0).normalize();
    let start = Vec3::splat(0.1) - dir * 0.3;
    let hit = world.cast_sphere(start, R, dir * 3.0, 1.0, 0xFF).unwrap();
    assert!((hit.time - (0.3 - R) / 3.0).abs() < 2e-6);
    assert!((hit.normal + dir).length() < 1e-4);
    assert!((hit.point - Vec3::splat(0.1)).length() < 1e-5);
}

#[test]
fn cylinder_side_cap_and_rim() {
    let (world, _) = single(StaticShape::Cylinder {
        base: v(0.0, 0.0, 0.0),
        axis: v(0.0, 0.0, 0.03),
        radius: 0.005,
    });
    // Side.
    let hit = world
        .cast_sphere(v(-0.2, 0.0, 0.0135), R, v(4.0, 0.0, 0.0), 1.0, 0xFF)
        .unwrap();
    assert!((hit.time - (0.2 - 0.005 - R) / 4.0).abs() < 1e-6);
    assert!((hit.normal - v(-1.0, 0.0, 0.0)).length() < 1e-5);
    // Glancing side hit: |offset| = 0.012 < R + radius.
    let hit = world
        .cast_sphere(v(-0.2, 0.012, 0.0135), R, v(4.0, 0.0, 0.0), 1.0, 0xFF)
        .unwrap();
    let reach = 0.005 + R;
    let dx = (reach * reach - 0.012f32 * 0.012).sqrt();
    assert!((hit.time - (0.2 - dx) / 4.0).abs() < 2e-6);
    // Passing beside it: no hit.
    assert!(
        world
            .cast_sphere(v(-0.2, 0.0186, 0.0135), R, v(4.0, 0.0, 0.0), 1.0, 0xFF)
            .is_none()
    );
    // Top cap from above.
    let hit = world
        .cast_sphere(v(0.001, 0.0, 0.2), R, v(0.0, 0.0, -1.0), 1.0, 0xFF)
        .unwrap();
    assert!((hit.time - (0.2 - 0.03 - R)).abs() < 1e-6);
    assert!((hit.normal - Vec3::Z).length() < 1e-5);
    // Bottom cap from below.
    let hit = world
        .cast_sphere(v(0.0, 0.001, -0.2), R, v(0.0, 0.0, 1.0), 1.0, 0xFF)
        .unwrap();
    assert!((hit.time - (0.2 - R)).abs() < 1e-6);
    assert!((hit.normal + Vec3::Z).length() < 1e-5);
    // Rim (the circle edge of the top cap): compare with brute force.
    let start = v(-0.1, 0.0, 0.03 + 0.1);
    let vel = v(1.0, 0.0, -1.0);
    let hit = world.cast_sphere(start, R, vel, 1.0, 0xFF).unwrap();
    let rim = v(-0.005, 0.0, 0.03);
    // Sphere centre vs the rim point nearest to the path (2D problem in XZ).
    let mut t = 0.0f64;
    while (start.as_dvec3() + vel.as_dvec3() * t - rim.as_dvec3()).length() > f64::from(R) {
        t += 1e-7;
    }
    assert!(
        (f64::from(hit.time) - t).abs() < 2e-6,
        "{} vs {t}",
        hit.time
    );
    assert!((hit.point - rim).length() < 1e-5);
}

#[test]
fn triangle_face_edge_vertex() {
    let (world, _) = single(StaticShape::TriMesh {
        vertices: vec![v(0.0, 0.0, 0.0), v(0.1, 0.0, 0.0), v(0.0, 0.1, 0.0)],
        indices: vec![[0, 1, 2]],
    });
    // Face from above and from below (two-sided).
    let hit = world
        .cast_sphere(v(0.02, 0.02, 0.1), R, v(0.0, 0.0, -1.0), 1.0, 0xFF)
        .unwrap();
    assert!((hit.time - (0.1 - R)).abs() < 1e-6);
    assert!((hit.normal - Vec3::Z).length() < 1e-6);
    let hit = world
        .cast_sphere(v(0.02, 0.02, -0.1), R, v(0.0, 0.0, 1.0), 1.0, 0xFF)
        .unwrap();
    assert!((hit.time - (0.1 - R)).abs() < 1e-6);
    assert!((hit.normal + Vec3::Z).length() < 1e-6);
    // Edge ab (y = 0), in the plane.
    let hit = world
        .cast_sphere(v(0.05, -0.1, 0.0), R, v(0.0, 1.0, 0.0), 1.0, 0xFF)
        .unwrap();
    assert!((hit.time - (0.1 - R)).abs() < 1e-6);
    assert!((hit.normal + Vec3::Y).length() < 1e-5);
    // Hypotenuse bc.
    let n = v(1.0, 1.0, 0.0).normalize();
    let hit = world
        .cast_sphere(v(0.05, 0.05, 0.0) + n * 0.1, R, -n, 1.0, 0xFF)
        .unwrap();
    assert!((hit.time - (0.1 - R)).abs() < 1e-6);
    assert!((hit.normal - n).length() < 1e-5);
    // Edge ca (x = 0).
    let hit = world
        .cast_sphere(v(-0.1, 0.05, 0.0), R, v(1.0, 0.0, 0.0), 1.0, 0xFF)
        .unwrap();
    assert!((hit.time - (0.1 - R)).abs() < 1e-6);
    // Vertices.
    for (vertex, dir) in [
        (v(0.0, 0.0, 0.0), v(-1.0, -1.0, 0.0)),
        (v(0.1, 0.0, 0.0), v(1.0, -0.2, 0.0)),
        (v(0.0, 0.1, 0.0), v(-0.2, 1.0, 0.0)),
    ] {
        let d = dir.normalize();
        let hit = world
            .cast_sphere(vertex + d * 0.1, R, -d * 2.0, 1.0, 0xFF)
            .unwrap();
        assert!((hit.time - (0.1 - R) / 2.0).abs() < 1e-6);
        assert!((hit.point - vertex).length() < 1e-5);
        assert!((hit.normal - d).length() < 1e-4);
    }
    // Sphere centre exactly on the triangle: contact with a face normal.
    let c = contacts(&world, v(0.02, 0.02, 0.0), R, 0xFF);
    assert_eq!(c.len(), 1);
    assert!((c[0].depth - R).abs() < 1e-7);
    assert!(c[0].normal.z.abs() > 0.999);
}

#[test]
fn layer_mask_and_enable_flags() {
    let mut b = StaticWorldBuilder::new();
    let wall = |x: f32| cuboid(v(x, 0.0, 0.0), v(0.01, 0.5, 0.5));
    let main = b.add_collider(wall(0.2), layers::MAIN_DECK, true);
    let upper = b.add_collider(wall(0.4), layers::UPPER_DECK, true);
    let gate = b.add_collider(wall(0.6), layers::MAIN_DECK | layers::WIREFORM, false);
    let nowhere = b.add_collider(wall(0.1), 0, true);
    let mut world = b.build();
    assert_eq!(world.collider_count(), 4);
    assert_eq!(world.collider_layers(main), layers::MAIN_DECK);
    assert_eq!(world.collider_layers(nowhere), 0);
    assert_eq!((main.index(), upper.index(), gate.index()), (0, 1, 2));

    let cast = |world: &StaticWorld, mask: u8| {
        world
            .cast_sphere(Vec3::ZERO, R, v(1.0, 0.0, 0.0), 10.0, mask)
            .map(|h| h.collider)
    };
    assert_eq!(cast(&world, layers::MAIN_DECK), Some(main));
    assert_eq!(cast(&world, layers::UPPER_DECK), Some(upper));
    assert_eq!(
        cast(&world, layers::MAIN_DECK | layers::UPPER_DECK),
        Some(main)
    );
    assert_eq!(cast(&world, layers::WIREFORM), None); // gate is disabled
    assert_eq!(cast(&world, layers::SUBWAY), None);
    assert_eq!(cast(&world, 0), None);
    assert_eq!(cast(&world, 0xFF), Some(main));

    assert!(world.is_enabled(main) && !world.is_enabled(gate));
    world.set_enabled(gate, true);
    assert!(world.is_enabled(gate));
    assert_eq!(cast(&world, layers::WIREFORM), Some(gate));
    world.set_enabled(main, false);
    assert_eq!(cast(&world, layers::MAIN_DECK), Some(gate));
    assert_eq!(cast(&world, 0xFF), Some(upper));
    world.set_enabled(main, true);
    assert_eq!(cast(&world, layers::MAIN_DECK), Some(main));

    // Contacts obey the same filters.
    let at_gate = v(0.6 - 0.01 - R + 0.001, 0.0, 0.0);
    assert_eq!(contacts(&world, at_gate, R, layers::WIREFORM).len(), 1);
    assert_eq!(contacts(&world, at_gate, R, layers::UPPER_DECK).len(), 0);
    world.set_enabled(gate, false);
    assert_eq!(contacts(&world, at_gate, R, 0xFF).len(), 0);

    // Unknown handles are harmless.
    let other = {
        let mut b = StaticWorldBuilder::new();
        for _ in 0..9 {
            b.add_collider(wall(0.0), 1, true);
        }
        b.add_collider(wall(0.0), 1, true)
    };
    world.set_enabled(other, true);
    assert!(!world.is_enabled(other));
    assert_eq!(world.collider_layers(other), 0);
}

#[test]
fn simultaneous_hits_resolve_to_first_collider() {
    let mut b = StaticWorldBuilder::new();
    let first = b.add_collider(cuboid(v(0.5, 0.0, 0.0), v(0.1, 0.1, 0.1)), 1, true);
    let _second = b.add_collider(cuboid(v(0.5, 0.0, 0.0), v(0.1, 0.1, 0.1)), 1, true);
    let world = b.build();
    let hit = world.cast_sphere(Vec3::ZERO, R, Vec3::X, 1.0, 1).unwrap();
    assert_eq!(hit.collider, first);
}

#[test]
fn initial_overlap_semantics() {
    let (world, _) = single(cuboid(v(0.0, 0.0, -0.05), v(1.0, 1.0, 0.05)));
    // Resting on the floor (top at z = 0).
    let rest = v(0.0, 0.0, R);
    // Pressing in: immediate hit.
    let hit = world
        .cast_sphere(rest, R, v(1.0, 0.0, -0.01), 1.0, 0xFF)
        .unwrap();
    assert_eq!(hit.time, 0.0);
    assert!((hit.normal - Vec3::Z).length() < 1e-6);
    // Sliding or lifting off: none.
    assert!(
        world
            .cast_sphere(rest, R, v(1.0, 0.0, 0.0), 1.0, 0xFF)
            .is_none()
    );
    assert!(
        world
            .cast_sphere(rest, R, v(1.0, 0.0, 0.5), 1.0, 0xFF)
            .is_none()
    );
    // Penetrating by 1 mm, moving out: none; moving in: hit at 0.
    let deep = v(0.0, 0.0, R - 0.001);
    assert!(
        world
            .cast_sphere(deep, R, v(0.0, 0.0, 1.0), 1.0, 0xFF)
            .is_none()
    );
    assert_eq!(
        world
            .cast_sphere(deep, R, v(0.0, 0.0, -1.0), 1.0, 0xFF)
            .unwrap()
            .time,
        0.0
    );
    // Centre inside the solid.
    let inside = v(0.0, 0.0, -0.01);
    let hit = world
        .cast_sphere(inside, R, v(0.0, 0.0, -1.0), 1.0, 0xFF)
        .unwrap();
    assert_eq!(hit.time, 0.0);
    assert!((hit.normal - Vec3::Z).length() < 1e-6);
    let c = contacts(&world, inside, R, 0xFF);
    assert_eq!(c.len(), 1);
    assert!((c[0].depth - (R + 0.01)).abs() < 1e-6);
    assert!((c[0].point - Vec3::ZERO).length() < 1e-6);
}

#[test]
fn contacts_report_depth_point_normal() {
    let mut b = StaticWorldBuilder::new();
    let floor = b.add_collider(cuboid(v(0.0, 0.0, -0.05), v(1.0, 1.0, 0.05)), 1, true);
    let wall = b.add_collider(cuboid(v(0.25, 0.0, 0.05), v(0.05, 1.0, 0.05)), 1, true);
    let post = b.add_collider(
        StaticShape::Cylinder {
            base: v(0.0, 0.3, 0.0),
            axis: v(0.0, 0.0, 0.04),
            radius: 0.004,
        },
        1,
        true,
    );
    let world = b.build();
    // In the corner between floor and wall, 1 mm into each.
    let c = contacts(&world, v(0.2 - R + 0.001, 0.0, R - 0.001), R, 1);
    assert_eq!(c.len(), 2);
    assert_eq!(c[0].collider, floor);
    assert_eq!(c[1].collider, wall);
    for contact in &c {
        assert!((contact.depth - 0.001).abs() < 1e-6);
        assert!((contact.normal.length() - 1.0).abs() < 1e-6);
    }
    assert!((c[0].normal - Vec3::Z).length() < 1e-6);
    assert!((c[1].normal + Vec3::X).length() < 1e-6);
    assert!((c[0].point - v(0.2 - R + 0.001, 0.0, 0.0)).length() < 1e-6);
    assert!((c[1].point - v(0.2, 0.0, R - 0.001)).length() < 1e-6);
    // Against the post.
    let c = contacts(&world, v(0.004 + R - 0.002, 0.3, 0.02), R, 1);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].collider, post);
    assert!((c[0].depth - 0.002).abs() < 1e-6);
    assert!((c[0].normal - Vec3::X).length() < 1e-5);
    // Clear of everything.
    assert!(contacts(&world, v(0.0, 0.0, R + 0.001), R, 1).is_empty());
    // `out` is cleared on every call.
    let mut out = ArrayVec::new();
    world.sphere_contacts(v(0.0, 0.0, R - 0.001), R, 1, &mut out);
    assert_eq!(out.len(), 1);
    world.sphere_contacts(v(0.0, 0.0, 1.0), R, 1, &mut out);
    assert!(out.is_empty());
    world.sphere_contacts(v(f32::NAN, 0.0, 0.0), R, 1, &mut out);
    assert!(out.is_empty());
}

#[test]
fn cylinder_interior_contacts() {
    let (world, _) = single(StaticShape::Cylinder {
        base: Vec3::ZERO,
        axis: v(0.0, 0.0, 0.1),
        radius: 0.05,
    });
    // Near the side.
    let c = contacts(&world, v(0.045, 0.0, 0.05), 0.001, 0xFF);
    assert!((c[0].normal - Vec3::X).length() < 1e-6);
    assert!((c[0].depth - 0.006).abs() < 1e-6);
    // Near the top and bottom caps.
    let c = contacts(&world, v(0.01, 0.0, 0.098), 0.001, 0xFF);
    assert!((c[0].normal - Vec3::Z).length() < 1e-6);
    assert!((c[0].point - v(0.01, 0.0, 0.1)).length() < 1e-6);
    let c = contacts(&world, v(0.0, 0.0, 0.002), 0.001, 0xFF);
    assert!((c[0].normal + Vec3::Z).length() < 1e-6);
    assert!((c[0].depth - 0.003).abs() < 1e-6);
    // On the surface exactly.
    let c = contacts(&world, v(0.05, 0.0, 0.05), 0.0, 0xFF);
    assert_eq!(c.len(), 1);
    assert!(c[0].depth.abs() < 1e-7);
}

#[test]
fn no_ghost_collisions_on_tessellated_flats() {
    for welded in [true, false] {
        let (world, _) = single(grid_mesh(8, 0.4, welded));
        // Rolling across the flat at (or a hair below) the resting height,
        // in many directions, crossing dozens of interior edges and vertices.
        // The only legitimate hit is the face itself when the ball presses in.
        for (height, vz) in [
            (R + 2e-6, 0.0),
            (R, 0.0),
            (R - 5e-6, 0.0),
            (R - 5e-5, 0.0),
            (R, 1e-4),
            (R - 5e-6, -0.02),
        ] {
            for k in 0..64 {
                let a = k as f32 * 0.0981;
                let dir = v(libm::cosf(a), libm::sinf(a), vz);
                let offset = v(
                    0.013 * libm::sinf(3.0 * a),
                    0.011 * libm::cosf(5.0 * a),
                    height,
                );
                let mut pos = offset - v(dir.x, dir.y, 0.0) * 0.15;
                // March in 1 ms steps like a solver would.
                for _ in 0..300 {
                    let hit = world.cast_sphere(pos, R, dir, 1.0e-3, 0xFF);
                    if let Some(hit) = hit {
                        assert!(vz < 0.0, "ghost hit {hit:?} (welded = {welded}, k = {k})");
                        assert!(
                            (hit.normal - Vec3::Z).length() < 1e-6,
                            "tilted hit {hit:?} (welded = {welded}, k = {k})"
                        );
                    }
                    pos += v(dir.x, dir.y, 0.0) * 1.0e-3;
                }
            }
        }
        // Dropping onto interior edges and vertices still collides, with the
        // face normal, exactly once.
        for p in [
            v(0.0, 0.0, 0.0),
            v(0.025, 0.025, 0.0),
            v(0.05, 0.02, 0.0),
            v(0.031, 0.0, 0.0),
        ] {
            let hit = world
                .cast_sphere(p + Vec3::Z * 0.1, R, v(0.0, 0.0, -1.0), 1.0, 0xFF)
                .unwrap();
            assert!((hit.time - (0.1 - R)).abs() < 1e-6);
            assert!((hit.normal - Vec3::Z).length() < 1e-5);
            let c = contacts(&world, p + Vec3::Z * (R - 0.0005), R, 0xFF);
            assert_eq!(c.len(), 1, "at {p} (welded = {welded}): {c:?}");
            assert!((c[0].normal - Vec3::Z).length() < 1e-5);
            assert!((c[0].depth - 0.0005).abs() < 1e-6);
        }
        // The outer boundary is a real edge.
        let hit = world
            .cast_sphere(v(-0.4, 0.01, 0.0), R, v(1.0, 0.0, 0.0), 1.0, 0xFF)
            .unwrap();
        assert!((hit.time - (0.2 - R)).abs() < 1e-6);
        assert!((hit.normal + Vec3::X).length() < 1e-4);
    }
}

#[test]
fn creased_mesh_edges_collide() {
    // A roof: two faces meeting at a ridge along Y at z = 0.05.
    let (world, _) = single(StaticShape::TriMesh {
        vertices: vec![
            v(-0.1, -0.1, 0.0),
            v(0.0, -0.1, 0.05),
            v(0.0, 0.1, 0.05),
            v(-0.1, 0.1, 0.0),
            v(0.1, -0.1, 0.0),
            v(0.1, 0.1, 0.0),
        ],
        indices: vec![[0, 1, 2], [0, 2, 3], [1, 4, 5], [1, 5, 2]],
    });
    let hit = world
        .cast_sphere(v(0.0, 0.0, 0.2), R, v(0.0, 0.0, -1.0), 1.0, 0xFF)
        .unwrap();
    assert!((hit.time - (0.15 - R)).abs() < 1e-6);
    assert!((hit.normal - Vec3::Z).length() < 1e-5);
    assert!((hit.point - v(0.0, 0.0, 0.05)).length() < 1e-5);
}

#[test]
fn invalid_geometry_is_skipped() {
    let mut b = StaticWorldBuilder::new();
    let handles = [
        b.add_collider(cuboid(v(f32::NAN, 0.0, 0.0), Vec3::ONE), 1, true),
        b.add_collider(
            StaticShape::Cuboid {
                center: Vec3::ZERO,
                half_extents: Vec3::ONE,
                rotation: Quat::from_xyzw(0.0, 0.0, 0.0, 0.0),
            },
            1,
            true,
        ),
        b.add_collider(
            StaticShape::Cylinder {
                base: Vec3::ZERO,
                axis: Vec3::ZERO,
                radius: 1.0,
            },
            1,
            true,
        ),
        b.add_collider(
            StaticShape::TriMesh {
                vertices: vec![Vec3::ZERO, Vec3::X, Vec3::X * 2.0, v(0.0, f32::NAN, 0.0)],
                // Collinear, out-of-range and non-finite triangles.
                indices: vec![[0, 1, 2], [0, 1, 9], [0, 1, 3], [0, 0, 1]],
            },
            1,
            true,
        ),
    ];
    let world = b.build();
    assert_eq!(world.collider_count(), 4);
    assert!(handles.iter().all(|h| world.is_enabled(*h)));
    assert!(
        world
            .cast_sphere(v(0.5, 0.0, 5.0), R, v(0.0, 0.0, -10.0), 1.0, 0xFF)
            .is_none()
    );
    assert!(contacts(&world, Vec3::ZERO, 0.5, 0xFF).is_empty());
    // Invalid queries.
    let (world, _) = single(cuboid(Vec3::ZERO, Vec3::splat(0.1)));
    let start = v(-1.0, 0.0, 0.0);
    assert!(
        world
            .cast_sphere(start, R, Vec3::X, f32::NAN, 0xFF)
            .is_none()
    );
    assert!(world.cast_sphere(start, R, Vec3::X, -1.0, 0xFF).is_none());
    assert!(
        world
            .cast_sphere(start, f32::NAN, Vec3::X, 2.0, 0xFF)
            .is_none()
    );
    assert!(world.cast_sphere(start, -1.0, Vec3::X, 2.0, 0xFF).is_none());
    assert!(
        world
            .cast_sphere(start, R, v(f32::INFINITY, 0.0, 0.0), 2.0, 0xFF)
            .is_none()
    );
    assert!(
        world
            .cast_sphere(start, R, v(1e30, 0.0, 0.0), 1e30, 0xFF)
            .is_none()
    );
    assert!(
        world
            .cast_sphere(v(f32::NAN, 0.0, 0.0), R, Vec3::X, 2.0, 0xFF)
            .is_none()
    );
    // Negative half extents and radius are taken as magnitudes.
    let (world, _) = single(cuboid(v(0.5, 0.0, 0.0), v(-0.1, -0.1, -0.1)));
    assert!(
        world
            .cast_sphere(Vec3::ZERO, R, Vec3::X, 1.0, 0xFF)
            .is_some()
    );
}

#[test]
fn keeps_the_deepest_sixteen_contacts() {
    // 24 thin posts in a ring, all overlapped by one big sphere.
    let mut b = StaticWorldBuilder::new();
    for k in 0..24 {
        let a = k as f32 * std::f32::consts::TAU / 24.0;
        let radius = 0.2 + 0.002 * k as f32;
        b.add_collider(
            StaticShape::Cylinder {
                base: v(radius * libm::cosf(a), radius * libm::sinf(a), -0.05),
                axis: v(0.0, 0.0, 0.1),
                radius: 0.002,
            },
            1,
            true,
        );
    }
    let world = b.build();
    let c = contacts(&world, Vec3::ZERO, 0.3, 1);
    assert_eq!(c.len(), 16);
    // The 16 nearest posts (smallest ring radius) are the deepest.
    for (i, contact) in c.iter().enumerate() {
        assert_eq!(contact.collider.index(), i);
    }
    assert!(c.windows(2).all(|w| w[0].depth > w[1].depth));
}

#[test]
fn fast_ball_does_not_tunnel_a_thin_wall() {
    let mut b = StaticWorldBuilder::new();
    // 0.5 mm thick wall and a mesh sheet behind it.
    b.add_collider(cuboid(v(0.3, 0.0, 0.0), v(0.00025, 0.5, 0.5)), 1, true);
    b.add_collider(grid_mesh(4, 1.0, true), 1, true);
    let world = b.build();
    for speed in [1.0f32, 8.0, 50.0, 500.0, 1.0e4] {
        // One 1 ms tick moves the ball up to 10 m.
        let hit = world
            .cast_sphere(
                v(0.25, 0.0, 0.1),
                R,
                v(speed, 0.0, 0.0),
                0.05 / speed * 1.5,
                1,
            )
            .unwrap();
        assert!(
            (hit.time * speed - (0.05 - 0.00025 - R)).abs() < 1e-5,
            "speed {speed}"
        );
        let hit = world
            .cast_sphere(v(0.1, 0.07, 0.3), R, v(0.0, 0.0, -speed), 1.0, 1)
            .unwrap();
        assert!((hit.time * speed - (0.3 - R)).abs() < 1e-5, "speed {speed}");
    }
}

/// Signed distance to an axis-aligned box, for the brute-force reference.
fn box_sdf(p: glam::DVec3, c: glam::DVec3, h: glam::DVec3) -> f64 {
    let q = (p - c).abs() - h;
    q.max(glam::DVec3::ZERO).length() + q.max_element().min(0.0)
}

/// Compares a sphere cast with a brute-force march over the reference
/// distance `dist` (distance from a point to the union of all colliders):
/// the cast may neither tunnel nor report a contact that is not there.
fn check_cast(
    world: &StaticWorld,
    dist: &dyn Fn(glam::DVec3) -> f64,
    start: Vec3,
    vel: Vec3,
    max_time: f32,
) -> Result<(), TestCaseError> {
    let speed = vel.length();
    let sdf = |t: f64| dist(start.as_dvec3() + vel.as_dvec3() * t) - f64::from(R);
    if sdf(0.0) <= 1e-4 {
        return Ok(()); // starts in contact: covered by the overlap tests
    }
    // Brute force: 20 µm steps, then bisection.
    let steps = 50_000;
    let mut expected = None;
    for k in 1..=steps {
        let t = f64::from(max_time) * k as f64 / steps as f64;
        if sdf(t) <= 0.0 {
            let (mut lo, mut hi) = (f64::from(max_time) * (k - 1) as f64 / steps as f64, t);
            for _ in 0..40 {
                let mid = 0.5 * (lo + hi);
                if sdf(mid) <= 0.0 { hi = mid } else { lo = mid }
            }
            expected = Some(hi);
            break;
        }
    }
    let hit = world.cast_sphere(start, R, vel, max_time, 1);
    match (hit, expected) {
        (Some(hit), Some(t)) => {
            prop_assert!(
                (f64::from(hit.time) - t).abs() * f64::from(speed) < 2e-5,
                "time {} vs {}",
                hit.time,
                t
            );
            prop_assert!((hit.normal.length() - 1.0).abs() < 1e-5);
            prop_assert!(hit.normal.dot(vel) < 0.0);
            // The contact point is on the sphere at the time of impact.
            let c = start + vel * hit.time;
            prop_assert!((c.distance(hit.point) - R).abs() < 2e-5);
            prop_assert!(((c - hit.point).normalize() - hit.normal).length() < 1e-2);
        }
        (None, None) => {}
        (Some(hit), None) => {
            // Brute force can miss a graze shallower than its step.
            prop_assert!(
                sdf(f64::from(hit.time)).abs() < 2e-5,
                "false positive {hit:?}"
            );
        }
        (None, Some(t)) => {
            // Only acceptable if the true path merely grazes (within
            // rounding) or the hit is at the very end of the window.
            let deepest = (0..=2000)
                .map(|k| sdf(t + 2e-4 / f64::from(speed) * k as f64 / 2000.0))
                .fold(f64::INFINITY, f64::min);
            prop_assert!(
                deepest > -2e-6 || t > f64::from(max_time) * 0.9999,
                "tunnelled: expected hit at {t}, penetration {deepest}"
            );
        }
    }
    Ok(())
}

/// Distance to a solid cylinder (f64 reference).
fn cylinder_sdf(p: glam::DVec3, base: glam::DVec3, axis: glam::DVec3, radius: f64) -> f64 {
    let height = axis.length();
    let dir = axis / height;
    let w = p - base;
    let h = w.dot(dir);
    let dx = (w - dir * h).length() - radius;
    let dy = (-h).max(h - height);
    dx.max(dy).min(0.0) + (dx.max(0.0) * dx.max(0.0) + dy.max(0.0) * dy.max(0.0)).sqrt()
}

/// Distance to a triangle (f64 reference).
fn triangle_distance(p: glam::DVec3, a: glam::DVec3, b: glam::DVec3, c: glam::DVec3) -> f64 {
    let (ba, cb, ac) = (b - a, c - b, a - c);
    let (pa, pb, pc) = (p - a, p - b, p - c);
    let nor = ba.cross(ac);
    let outside = ba.cross(nor).dot(pa).signum()
        + cb.cross(nor).dot(pb).signum()
        + ac.cross(nor).dot(pc).signum()
        < 2.0;
    if outside {
        let edge = |e: glam::DVec3, q: glam::DVec3| {
            (e * (e.dot(q) / e.length_squared()).clamp(0.0, 1.0) - q).length_squared()
        };
        edge(ba, pa).min(edge(cb, pb)).min(edge(ac, pc)).sqrt()
    } else {
        (nor.dot(pa) * nor.dot(pa) / nor.length_squared()).sqrt()
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, failure_persistence: None, ..ProptestConfig::default() })]

    /// The cast never reports a time later than the true first contact (no
    /// tunnelling) nor earlier (no false positives), against brute force.
    #[test]
    fn prop_cast_matches_brute_force_on_boxes(
        boxes in prop::collection::vec(
            ((-0.3f32..0.3, -0.3f32..0.3, -0.1f32..0.1), (0.005f32..0.08, 0.005f32..0.08, 0.005f32..0.08)),
            1..6),
        start in (-0.5f32..0.5, -0.5f32..0.5, -0.2f32..0.2),
        dir in (-1.0f32..1.0, -1.0f32..1.0, -0.4f32..0.4),
        speed in 0.1f32..20.0,
    ) {
        let mut b = StaticWorldBuilder::new();
        for (c, h) in &boxes {
            b.add_collider(cuboid(v(c.0, c.1, c.2), v(h.0, h.1, h.2)), 1, true);
        }
        let world = b.build();
        let start = v(start.0, start.1, start.2);
        let mut d = v(dir.0, dir.1, dir.2);
        if d.length() < 0.05 { d = Vec3::X; }
        let vel = d.normalize() * speed;
        let max_time = 1.0 / speed; // travel 1 m
        let dist = |p: glam::DVec3| {
            boxes.iter().map(|(c, h)| box_sdf(p, v(c.0, c.1, c.2).as_dvec3(), v(h.0, h.1, h.2).as_dvec3()))
                .fold(f64::INFINITY, f64::min)
        };
        check_cast(&world, &dist, start, vel, max_time)?;
    }

    /// The same for rotated cuboids, cylinders and triangles together.
    #[test]
    fn prop_cast_matches_brute_force_on_mixed_shapes(
        shapes in prop::collection::vec(
            (0u8..3,
             (-0.3f32..0.3, -0.3f32..0.3, -0.1f32..0.1),
             (0.005f32..0.08, 0.005f32..0.08, 0.005f32..0.08),
             (-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0),
             -3.2f32..3.2,
             ((-0.1f32..0.1, -0.1f32..0.1, -0.1f32..0.1), (-0.1f32..0.1, -0.1f32..0.1, -0.1f32..0.1))),
            1..6),
        start in (-0.5f32..0.5, -0.5f32..0.5, -0.2f32..0.2),
        dir in (-1.0f32..1.0, -1.0f32..1.0, -0.4f32..0.4),
        speed in 0.1f32..20.0,
    ) {
        type Reference = Box<dyn Fn(glam::DVec3) -> f64>;
        let mut b = StaticWorldBuilder::new();
        let mut refs: Vec<Reference> = Vec::new();
        for (kind, c, size, axis, angle, (e1, e2)) in &shapes {
            let c = v(c.0, c.1, c.2);
            let mut axis = v(axis.0, axis.1, axis.2);
            if axis.length() < 0.1 { axis = Vec3::Z; }
            let axis = axis.normalize();
            match kind {
                0 => {
                    let rotation = Quat::from_axis_angle(axis, *angle);
                    let half = v(size.0, size.1, size.2);
                    b.add_collider(StaticShape::Cuboid { center: c, half_extents: half, rotation }, 1, true);
                    let inverse = rotation.as_dquat().inverse();
                    let (cd, hd) = (c.as_dvec3(), half.as_dvec3());
                    refs.push(Box::new(move |p| box_sdf(inverse * (p - cd), glam::DVec3::ZERO, hd)));
                }
                1 => {
                    let span = axis * (size.0 * 2.0);
                    let radius = size.1 * 0.5;
                    b.add_collider(StaticShape::Cylinder { base: c, axis: span, radius }, 1, true);
                    let (cd, sd) = (c.as_dvec3(), span.as_dvec3());
                    refs.push(Box::new(move |p| cylinder_sdf(p, cd, sd, f64::from(radius))));
                }
                _ => {
                    let (pa, pb, pc) = (c, c + v(e1.0, e1.1, e1.2), c + v(e2.0, e2.1, e2.2));
                    let area2 = (pb - pa).cross(pc - pa).length();
                    // Skip slivers the builder rejects as degenerate.
                    prop_assume!(area2 > 1e-4);
                    b.add_collider(
                        StaticShape::TriMesh { vertices: vec![pa, pb, pc], indices: vec![[0, 1, 2]] },
                        1,
                        true,
                    );
                    let (ad, bd, cd) = (pa.as_dvec3(), pb.as_dvec3(), pc.as_dvec3());
                    refs.push(Box::new(move |p| triangle_distance(p, ad, bd, cd)));
                }
            }
        }
        let world = b.build();
        let start = v(start.0, start.1, start.2);
        let mut d = v(dir.0, dir.1, dir.2);
        if d.length() < 0.05 { d = Vec3::X; }
        let vel = d.normalize() * speed;
        let max_time = 1.0 / speed; // travel 1 m
        let dist = |p: glam::DVec3| refs.iter().map(|f| f(p)).fold(f64::INFINITY, f64::min);
        check_cast(&world, &dist, start, vel, max_time)?;
    }

    /// Contacts agree with the analytic signed distance of every box.
    #[test]
    fn prop_contacts_match_signed_distance(
        boxes in prop::collection::vec(
            ((-0.1f32..0.1, -0.1f32..0.1, -0.1f32..0.1), (0.005f32..0.08, 0.005f32..0.08, 0.005f32..0.08)),
            1..8),
        p in (-0.2f32..0.2, -0.2f32..0.2, -0.2f32..0.2),
        radius in 0.001f32..0.05,
    ) {
        let mut b = StaticWorldBuilder::new();
        for (c, h) in &boxes {
            b.add_collider(cuboid(v(c.0, c.1, c.2), v(h.0, h.1, h.2)), 1, true);
        }
        let world = b.build();
        let p = v(p.0, p.1, p.2);
        let found = contacts(&world, p, radius, 1);
        for (i, (c, h)) in boxes.iter().enumerate() {
            let d = box_sdf(p.as_dvec3(), v(c.0, c.1, c.2).as_dvec3(), v(h.0, h.1, h.2).as_dvec3());
            let depth = f64::from(radius) - d;
            let contact = found.iter().find(|c| c.collider.index() == i);
            if depth > 1e-5 {
                let contact = contact.expect("missing contact");
                prop_assert!((f64::from(contact.depth) - depth).abs() < 1e-5);
                prop_assert!((contact.normal.length() - 1.0).abs() < 1e-5);
                if d > 1e-4 {
                    prop_assert!((f64::from(contact.point.distance(p)) - d).abs() < 1e-5);
                }
            } else if depth < -1e-5 {
                prop_assert!(contact.is_none());
            }
        }
        prop_assert!(found.windows(2).all(|w| w[0].collider < w[1].collider));
    }
}
