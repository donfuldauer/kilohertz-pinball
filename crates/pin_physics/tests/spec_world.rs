//! Independent spec tests (Subphase 2.1): static collision world, collision
//! layers, per-collider enable flags, sphere casts and sphere contacts.
//!
//! Black-box against `pin_physics::world::{StaticWorldBuilder, StaticWorld,
//! StaticShape, SphereHit, SphereContact, ColliderHandle, layers}`.

use arrayvec::ArrayVec;
use glam::{Quat, Vec3};
use pin_physics::world::layers::{self, BACKBOX, MAIN_DECK, SUBWAY, UPPER_DECK, WIREFORM};
use pin_physics::world::{SphereContact, StaticShape, StaticWorld, StaticWorldBuilder};
use proptest::prelude::*;

const R: f32 = 0.0135; // standard 27 mm (1 1/16") ball
const TIME_TOL: f32 = 1e-4; // seconds at ~1 m/s → 0.1 mm

fn wall(center: Vec3, half: Vec3) -> StaticShape {
    StaticShape::Cuboid {
        center,
        half_extents: half,
        rotation: Quat::IDENTITY,
    }
}

/// A wall whose -X face sits at x = 0.9, spanning y,z ∈ [-0.5, 0.5].
fn x_wall() -> StaticShape {
    wall(Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.1, 0.5, 0.5))
}

fn contacts(w: &StaticWorld, c: Vec3, r: f32, mask: u8) -> ArrayVec<SphereContact, 16> {
    let mut out = ArrayVec::new();
    w.sphere_contacts(c, r, mask, &mut out);
    out
}

// ---------------------------------------------------------------- layers

#[test]
fn layer_constants() {
    assert_eq!(MAIN_DECK, 1);
    assert_eq!(UPPER_DECK, 2);
    assert_eq!(WIREFORM, 4);
    assert_eq!(SUBWAY, 8);
    assert_eq!(BACKBOX, 16);
    assert_eq!(layers::layer_bit(0), MAIN_DECK);
    assert_eq!(layers::layer_bit(1), UPPER_DECK);
    assert_eq!(layers::layer_bit(2), WIREFORM);
    assert_eq!(layers::layer_bit(3), SUBWAY);
    assert_eq!(layers::layer_bit(4), BACKBOX);
    for l in 0u8..8 {
        assert_eq!(layers::layer_bit(l), 1u8 << l);
    }
    for l in 8u8..=255 {
        assert_eq!(layers::layer_bit(l), 0, "layer {l} out of range");
    }
}

// ---------------------------------------------------------------- cast_sphere: analytic TOI

#[test]
fn head_on_sphere_vs_cuboid_time_of_impact() {
    let mut b = StaticWorldBuilder::new();
    let h = b.add_collider(x_wall(), MAIN_DECK, true);
    let w = b.build();
    for &speed in &[0.5f32, 1.0, 2.0, 8.0] {
        let start = Vec3::new(0.0, 0.1, 0.2);
        let hit = w
            .cast_sphere(start, R, Vec3::new(speed, 0.0, 0.0), 10.0, MAIN_DECK)
            .expect("head-on cast must hit");
        let expected = (0.9 - R) / speed;
        assert!(
            (hit.time - expected).abs() * speed < 1e-4,
            "speed {speed}: toi {} vs analytic {expected}",
            hit.time
        );
        assert!(hit.collider == h);
        assert!(
            (hit.normal - Vec3::NEG_X).length() < 1e-3,
            "normal {:?}",
            hit.normal
        );
        assert!(
            (hit.point - Vec3::new(0.9, 0.1, 0.2)).length() < 1e-3,
            "point {:?}",
            hit.point
        );
    }
}

#[test]
fn oblique_cast_time_of_impact() {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(x_wall(), MAIN_DECK, true);
    let w = b.build();
    let v = Vec3::new(3.0, 1.0, 0.0);
    let hit = w
        .cast_sphere(Vec3::ZERO, R, v, 1.0, MAIN_DECK)
        .expect("hit");
    let expected = (0.9 - R) / 3.0;
    assert!(
        (hit.time - expected).abs() < TIME_TOL,
        "toi {} vs {expected}",
        hit.time
    );
    assert!((hit.normal - Vec3::NEG_X).length() < 1e-3);
}

#[test]
fn rotated_cuboid_time_of_impact() {
    // Long axis along local X rotated 90° about Z: world X half-extent is 0.1.
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cuboid {
            center: Vec3::new(1.0, 0.0, 0.0),
            half_extents: Vec3::new(0.5, 0.1, 0.5),
            rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        },
        MAIN_DECK,
        true,
    );
    let w = b.build();
    let hit = w
        .cast_sphere(Vec3::ZERO, R, Vec3::X, 2.0, MAIN_DECK)
        .expect("hit");
    assert!((hit.time - (0.9 - R)).abs() < TIME_TOL, "toi {}", hit.time);
    assert!((hit.normal - Vec3::NEG_X).length() < 1e-3);
}

#[test]
fn cylinder_post_time_of_impact() {
    let mut b = StaticWorldBuilder::new();
    let h = b.add_collider(
        StaticShape::Cylinder {
            base: Vec3::new(1.0, 0.0, 0.0),
            axis: Vec3::Z,
            radius: 0.05,
        },
        MAIN_DECK,
        true,
    );
    let w = b.build();
    let hit = w
        .cast_sphere(Vec3::new(0.0, 0.0, 0.5), R, Vec3::X, 2.0, MAIN_DECK)
        .expect("hit post");
    assert!(
        (hit.time - (1.0 - 0.05 - R)).abs() < TIME_TOL,
        "toi {}",
        hit.time
    );
    assert!(hit.collider == h);
    assert!((hit.normal - Vec3::NEG_X).length() < 1e-3);
    // Above the top of the post (height 1 m): no hit.
    assert!(
        w.cast_sphere(Vec3::new(0.0, 0.0, 1.5), R, Vec3::X, 2.0, MAIN_DECK)
            .is_none()
    );
    // Glancing pass that misses laterally.
    assert!(
        w.cast_sphere(Vec3::new(0.0, 0.2, 0.5), R, Vec3::X, 2.0, MAIN_DECK)
            .is_none()
    );
}

#[test]
fn trimesh_floor_time_of_impact() {
    let vertices = vec![
        Vec3::new(-1.0, -1.0, 0.0),
        Vec3::new(1.0, -1.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(-1.0, 1.0, 0.0),
    ];
    let indices = vec![[0u32, 1, 2], [0, 2, 3]];
    let mut b = StaticWorldBuilder::new();
    let h = b.add_collider(StaticShape::TriMesh { vertices, indices }, MAIN_DECK, true);
    let w = b.build();
    let hit = w
        .cast_sphere(
            Vec3::new(0.2, 0.3, 0.1),
            R,
            Vec3::new(0.0, 0.0, -1.0),
            1.0,
            MAIN_DECK,
        )
        .expect("hit floor");
    assert!((hit.time - (0.1 - R)).abs() < TIME_TOL, "toi {}", hit.time);
    assert!(hit.collider == h);
    assert!(
        (hit.normal - Vec3::Z).length() < 1e-3,
        "normal {:?}",
        hit.normal
    );
    assert!((hit.point - Vec3::new(0.2, 0.3, 0.0)).length() < 1e-3);
}

#[test]
fn cast_respects_max_time_and_direction() {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(x_wall(), MAIN_DECK, true);
    let w = b.build();
    let toi = 0.9 - R;
    assert!(
        w.cast_sphere(Vec3::ZERO, R, Vec3::X, toi * 0.95, MAIN_DECK)
            .is_none(),
        "too short"
    );
    assert!(
        w.cast_sphere(Vec3::ZERO, R, Vec3::X, toi * 1.05, MAIN_DECK)
            .is_some()
    );
    assert!(
        w.cast_sphere(Vec3::ZERO, R, Vec3::NEG_X, 10.0, MAIN_DECK)
            .is_none(),
        "moving away"
    );
    assert!(
        w.cast_sphere(Vec3::ZERO, R, Vec3::Y, 10.0, MAIN_DECK)
            .is_none(),
        "parallel"
    );
    if let Some(hit) = w.cast_sphere(Vec3::ZERO, R, Vec3::X, 10.0, MAIN_DECK) {
        assert!(hit.time >= 0.0 && hit.time <= 10.0);
    }
}

#[test]
fn fast_ball_does_not_tunnel_thin_wall() {
    // 2 mm thick wall, ball at 40 m/s over a 1 ms tick: travel 40 mm.
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        wall(Vec3::new(0.02, 0.0, 0.0), Vec3::new(0.001, 0.5, 0.5)),
        MAIN_DECK,
        true,
    );
    let w = b.build();
    let hit = w
        .cast_sphere(Vec3::ZERO, R, Vec3::new(40.0, 0.0, 0.0), 1e-3, MAIN_DECK)
        .expect("CCD must catch a thin wall");
    let expected = (0.019 - R).max(0.0) / 40.0;
    assert!(
        (hit.time - expected).abs() < 1e-5,
        "toi {} vs {expected}",
        hit.time
    );
}

// ---------------------------------------------------------------- layer filtering & enable flags

#[test]
fn layer_mask_filters_casts() {
    let mut b = StaticWorldBuilder::new();
    let h = b.add_collider(x_wall(), UPPER_DECK, true);
    let w = b.build();
    let v = Vec3::X;
    assert!(w.cast_sphere(Vec3::ZERO, R, v, 2.0, MAIN_DECK).is_none());
    assert!(
        w.cast_sphere(Vec3::ZERO, R, v, 2.0, WIREFORM | SUBWAY | BACKBOX)
            .is_none()
    );
    assert!(
        w.cast_sphere(Vec3::ZERO, R, v, 2.0, 0).is_none(),
        "empty mask hits nothing"
    );
    let hit = w
        .cast_sphere(Vec3::ZERO, R, v, 2.0, UPPER_DECK)
        .expect("same layer");
    assert!(hit.collider == h);
    assert!(
        w.cast_sphere(Vec3::ZERO, R, v, 2.0, MAIN_DECK | UPPER_DECK)
            .is_some()
    );
    assert!(w.cast_sphere(Vec3::ZERO, R, v, 2.0, 0xFF).is_some());
}

#[test]
fn multi_layer_collider_is_hit_from_either_layer() {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(x_wall(), MAIN_DECK | WIREFORM, true);
    let w = b.build();
    assert!(
        w.cast_sphere(Vec3::ZERO, R, Vec3::X, 2.0, MAIN_DECK)
            .is_some()
    );
    assert!(
        w.cast_sphere(Vec3::ZERO, R, Vec3::X, 2.0, WIREFORM)
            .is_some()
    );
    assert!(
        w.cast_sphere(Vec3::ZERO, R, Vec3::X, 2.0, UPPER_DECK)
            .is_none()
    );
}

#[test]
fn collider_with_no_layers_is_never_hit() {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(x_wall(), 0, true);
    let w = b.build();
    assert!(w.cast_sphere(Vec3::ZERO, R, Vec3::X, 2.0, 0xFF).is_none());
    assert!(contacts(&w, Vec3::new(0.89, 0.0, 0.0), R, 0xFF).is_empty());
}

#[test]
fn disabled_colliders_are_never_hit_and_can_be_toggled() {
    let mut b = StaticWorldBuilder::new();
    let near = b.add_collider(
        wall(Vec3::new(0.5, 0.0, 0.0), Vec3::new(0.01, 0.5, 0.5)),
        MAIN_DECK,
        false,
    );
    let far = b.add_collider(x_wall(), MAIN_DECK, true);
    let mut w = b.build();
    assert!(!w.is_enabled(near));
    assert!(w.is_enabled(far));

    // Disabled drop target: the ball passes through to the far wall.
    let hit = w
        .cast_sphere(Vec3::ZERO, R, Vec3::X, 2.0, MAIN_DECK)
        .expect("far wall");
    assert!(hit.collider == far);
    assert!((hit.time - (0.9 - R)).abs() < TIME_TOL);

    // Raise the drop target.
    w.set_enabled(near, true);
    assert!(w.is_enabled(near));
    let hit = w
        .cast_sphere(Vec3::ZERO, R, Vec3::X, 2.0, MAIN_DECK)
        .expect("near target");
    assert!(hit.collider == near);
    assert!((hit.time - (0.49 - R)).abs() < TIME_TOL, "toi {}", hit.time);

    // Disable everything.
    w.set_enabled(near, false);
    w.set_enabled(far, false);
    assert!(!w.is_enabled(far));
    assert!(w.cast_sphere(Vec3::ZERO, R, Vec3::X, 2.0, 0xFF).is_none());
    assert!(contacts(&w, Vec3::new(0.89, 0.0, 0.0), R, 0xFF).is_empty());

    // Idempotent toggles.
    w.set_enabled(far, true);
    w.set_enabled(far, true);
    assert!(w.is_enabled(far));
    assert!(
        w.cast_sphere(Vec3::ZERO, R, Vec3::X, 2.0, MAIN_DECK)
            .is_some()
    );
}

#[test]
fn nearest_of_several_colliders_is_reported() {
    let mut b = StaticWorldBuilder::new();
    let mut handles = Vec::new();
    // Insert in non-sorted order to make sure the result is not "first added".
    for &x in &[0.8f32, 0.3, 0.6, 0.45] {
        handles.push((
            x,
            b.add_collider(
                wall(Vec3::new(x, 0.0, 0.0), Vec3::new(0.01, 0.2, 0.2)),
                MAIN_DECK,
                true,
            ),
        ));
    }
    let w = b.build();
    let hit = w
        .cast_sphere(Vec3::ZERO, R, Vec3::X, 5.0, MAIN_DECK)
        .expect("hit");
    let nearest = handles.iter().find(|(x, _)| *x == 0.3).unwrap().1;
    assert!(hit.collider == nearest);
    assert!((hit.time - (0.29 - R)).abs() < TIME_TOL);
}

#[test]
fn handles_are_distinct() {
    let mut b = StaticWorldBuilder::new();
    let a = b.add_collider(x_wall(), MAIN_DECK, true);
    let c = b.add_collider(x_wall(), MAIN_DECK, true);
    assert!(a != c);
}

#[test]
fn empty_world_hits_nothing() {
    let w = StaticWorldBuilder::new().build();
    assert!(w.cast_sphere(Vec3::ZERO, R, Vec3::X, 100.0, 0xFF).is_none());
    assert!(contacts(&w, Vec3::ZERO, R, 0xFF).is_empty());
}

// ---------------------------------------------------------------- sphere_contacts

fn floor() -> StaticShape {
    // Top face at z = 0.
    wall(Vec3::new(0.0, 0.0, -0.05), Vec3::new(1.0, 1.0, 0.05))
}

#[test]
fn resting_contact_with_floor() {
    let mut b = StaticWorldBuilder::new();
    let h = b.add_collider(floor(), MAIN_DECK, true);
    let w = b.build();
    let c = Vec3::new(0.1, 0.2, R - 0.002);
    let out = contacts(&w, c, R, MAIN_DECK);
    assert_eq!(out.len(), 1);
    let k = &out[0];
    assert!(k.collider == h);
    assert!((k.depth - 0.002).abs() < 1e-5, "depth {}", k.depth);
    assert!(
        (k.normal - Vec3::Z).length() < 1e-4,
        "normal {:?}",
        k.normal
    );
    assert!((k.point.truncate() - c.truncate()).length() < 1e-4);
    assert!(
        k.point.z.abs() <= 0.002 + 1e-5,
        "contact point near surface: {:?}",
        k.point
    );

    // Separated: no contact. Wrong layer / disabled: no contact.
    assert!(contacts(&w, Vec3::new(0.1, 0.2, R + 0.001), R, MAIN_DECK).is_empty());
    assert!(contacts(&w, c, R, UPPER_DECK).is_empty());
    let mut w = w;
    w.set_enabled(h, false);
    assert!(contacts(&w, c, R, MAIN_DECK).is_empty());
}

#[test]
fn corner_gives_two_contacts() {
    let mut b = StaticWorldBuilder::new();
    let f = b.add_collider(floor(), MAIN_DECK, true);
    let side = b.add_collider(
        wall(Vec3::new(0.3, 0.0, 0.1), Vec3::new(0.05, 1.0, 0.1)),
        MAIN_DECK,
        true,
    );
    let up = b.add_collider(
        wall(Vec3::new(-0.3, 0.0, 0.1), Vec3::new(0.05, 1.0, 0.1)),
        UPPER_DECK,
        true,
    );
    let w = b.build();
    // Ball in the corner between floor and the +X side wall (face at x = 0.25).
    let c = Vec3::new(0.25 - R + 0.001, 0.0, R - 0.001);
    let out = contacts(&w, c, R, MAIN_DECK);
    assert_eq!(out.len(), 2, "{out:?}");
    let fc = out.iter().find(|k| k.collider == f).expect("floor contact");
    let sc = out
        .iter()
        .find(|k| k.collider == side)
        .expect("side contact");
    assert!((fc.normal - Vec3::Z).length() < 1e-3);
    assert!((sc.normal - Vec3::NEG_X).length() < 1e-3);
    assert!((fc.depth - 0.001).abs() < 1e-5 && (sc.depth - 0.001).abs() < 1e-5);
    assert!(out.iter().all(|k| k.collider != up));
    // Mirror corner against the upper-deck wall only shows with its layer.
    let c2 = Vec3::new(-0.25 + R - 0.001, 0.0, R + 0.01);
    assert!(contacts(&w, c2, R, MAIN_DECK).is_empty());
    let out2 = contacts(&w, c2, R, UPPER_DECK);
    assert_eq!(out2.len(), 1);
    assert!(out2[0].collider == up);
    assert!((out2[0].normal - Vec3::X).length() < 1e-3);
}

#[test]
fn contact_with_post() {
    let mut b = StaticWorldBuilder::new();
    let h = b.add_collider(
        StaticShape::Cylinder {
            base: Vec3::ZERO,
            axis: Vec3::new(0.0, 0.0, 0.05),
            radius: 0.01,
        },
        MAIN_DECK,
        true,
    );
    let w = b.build();
    let dir = Vec3::new(0.6, 0.8, 0.0);
    let c = dir * (0.01 + R - 0.0005) + Vec3::new(0.0, 0.0, 0.02);
    let out = contacts(&w, c, R, MAIN_DECK);
    assert_eq!(out.len(), 1);
    assert!(out[0].collider == h);
    assert!(
        (out[0].depth - 0.0005).abs() < 2e-5,
        "depth {}",
        out[0].depth
    );
    assert!(
        (out[0].normal - dir).length() < 1e-3,
        "normal {:?}",
        out[0].normal
    );
}

#[test]
fn many_overlaps_do_not_overflow() {
    // More than 16 overlapping colliders: must not panic, at most 16 reported.
    let mut b = StaticWorldBuilder::new();
    for i in 0..24 {
        let a = std::f32::consts::TAU * i as f32 / 24.0;
        let d = Vec3::new(a.cos(), a.sin(), 0.0);
        b.add_collider(
            StaticShape::Cylinder {
                base: d * (R + 0.004) - Vec3::Z * 0.05,
                axis: Vec3::Z * 0.1,
                radius: 0.005,
            },
            MAIN_DECK,
            true,
        );
    }
    let w = b.build();
    let out = contacts(&w, Vec3::ZERO, R, MAIN_DECK);
    assert!(!out.is_empty() && out.len() <= 16);
}

// ---------------------------------------------------------------- property tests

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]

    #[test]
    fn prop_head_on_toi_matches_analytic(
        dist in 0.05f32..1.5,
        speed in 0.2f32..40.0,
        y in -0.15f32..0.15,
        z in -0.15f32..0.15,
        r in 0.005f32..0.04,
        half in 0.005f32..0.2,
    ) {
        let face = dist + r;
        let mut b = StaticWorldBuilder::new();
        b.add_collider(wall(Vec3::new(face + half, 0.0, 0.0), Vec3::new(half, 0.5, 0.5)), MAIN_DECK, true);
        let w = b.build();
        let start = Vec3::new(0.0, y, z);
        let hit = w.cast_sphere(start, r, Vec3::new(speed, 0.0, 0.0), 10.0, MAIN_DECK);
        prop_assert!(hit.is_some());
        let hit = hit.unwrap();
        let expected = dist / speed;
        prop_assert!((hit.time - expected).abs() * speed < 1e-4, "toi {} vs {}", hit.time, expected);
        prop_assert!((hit.normal - Vec3::NEG_X).length() < 1e-3);
        prop_assert!((hit.point.x - face).abs() < 1e-4);
    }

    #[test]
    fn prop_mask_and_enable_filtering(
        coll_layers in any::<u8>(),
        mask in any::<u8>(),
        enabled in any::<bool>(),
        toggle in any::<bool>(),
    ) {
        let mut b = StaticWorldBuilder::new();
        let h = b.add_collider(x_wall(), coll_layers, enabled);
        let mut w = b.build();
        if toggle {
            w.set_enabled(h, !enabled);
        }
        let on = enabled ^ toggle;
        prop_assert_eq!(w.is_enabled(h), on);
        let should_hit = on && (coll_layers & mask) != 0;
        let hit = w.cast_sphere(Vec3::ZERO, R, Vec3::X, 2.0, mask);
        prop_assert_eq!(hit.is_some(), should_hit);
        let out = contacts(&w, Vec3::new(0.9 - R + 0.002, 0.0, 0.0), R, mask);
        prop_assert_eq!(!out.is_empty(), should_hit);
    }

    #[test]
    fn prop_cast_normal_points_toward_sphere(
        start in (-0.3f32..0.3, -0.3f32..0.3, 0.1f32..0.3),
        v in (-0.5f32..0.5, -0.5f32..0.5, -3.0f32..-0.5),
    ) {
        // A floor below and a post field; whatever is hit, the normal points
        // from the surface toward the sphere centre at impact and is unit length.
        let mut b = StaticWorldBuilder::new();
        b.add_collider(floor(), MAIN_DECK, true);
        b.add_collider(StaticShape::Cylinder { base: Vec3::new(0.1, 0.1, 0.0), axis: Vec3::Z * 0.05, radius: 0.02 }, MAIN_DECK, true);
        b.add_collider(StaticShape::Cuboid { center: Vec3::new(-0.1, 0.0, 0.03), half_extents: Vec3::new(0.02, 0.1, 0.03), rotation: Quat::from_rotation_z(0.7) }, MAIN_DECK, true);
        let w = b.build();
        let start = Vec3::new(start.0, start.1, start.2);
        let v = Vec3::new(v.0, v.1, v.2);
        if !contacts(&w, start, R, MAIN_DECK).is_empty() {
            return Ok(());
        }
        let hit = w.cast_sphere(start, R, v, 1.0, MAIN_DECK);
        prop_assert!(hit.is_some(), "falling ball must reach the floor");
        let hit = hit.unwrap();
        prop_assert!(hit.time >= 0.0 && hit.time <= 1.0);
        prop_assert!((hit.normal.length() - 1.0).abs() < 1e-3);
        let centre = start + v * hit.time;
        prop_assert!(hit.normal.dot(centre - hit.point) > 0.0);
        prop_assert!(((centre - hit.point).length() - R).abs() < 1e-3);
        prop_assert!(hit.normal.dot(v) < 0.0, "approaching the surface");
    }
}
