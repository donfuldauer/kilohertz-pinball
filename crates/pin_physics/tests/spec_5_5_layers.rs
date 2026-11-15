//! Subphase 5.5 spec tests (Independent Tester): collision layer bit 4
//! (backbox).
//!
//! * `world::layers`: bit 0 main deck, 1 upper deck, 2 wireform, 3 subway,
//!   4 backbox.
//! * A ball on layer 4 placed overlapping layer 0–3 colliders passes through
//!   them with unchanged velocity, and a ball on layers 0–3 passes through
//!   layer-4 colliders (proptest, 1024 random states; static colliders of
//!   every primitive kind and flipper bats).
//! * A `LayerTransition` hands a ball from layer 0 to layer 4 and back; the
//!   ball is preserved, finite and on the expected layer after each.
//!
//! Scenes run with zero gravity so a ball that collides with nothing keeps
//! its velocity exactly; any change of velocity is a collision.

#[path = "spec_5_5_common.rs"]
mod bb;

use arrayvec::ArrayVec;
use bb::c23::{DT, R, ball, solver};
use bb::{BACKBOX_LAYER, Mount};
use glam::{Quat, Vec3};
use pin_core::ids::FlipperId;
use pin_physics::ball::{BallEvent, BallEvents, BallHandle, Geometry};
use pin_physics::flipper::FlipperSet;
use pin_physics::world::{
    LayerTransition, StaticShape, StaticWorld, StaticWorldBuilder, TransitionSet, layers,
};
use proptest::prelude::*;
use std::sync::OnceLock;

#[test]
fn layer_bits_are_the_documented_ones() {
    assert_eq!(layers::MAIN_DECK, 1 << 0);
    assert_eq!(layers::UPPER_DECK, 1 << 1);
    assert_eq!(layers::WIREFORM, 1 << 2);
    assert_eq!(layers::SUBWAY, 1 << 3);
    assert_eq!(layers::BACKBOX, 1 << 4);
    assert_eq!(layers::layer_bit(BACKBOX_LAYER), layers::BACKBOX);
    for l in 0..4u8 {
        assert_eq!(layers::layer_bit(l) & layers::BACKBOX, 0, "layer {l}");
    }
}

/// Spacing of the five collider groups along x, metres.
const GROUP_DX: f32 = 0.5;
/// Cuboid half extent, metres.
const BOX_H: f32 = 0.05;
/// Cylinder radius and height, metres.
const CYL_R: f32 = 0.03;
const CYL_H: f32 = 0.1;
/// Half size of the trimesh quad, metres.
const QUAD_H: f32 = 0.05;

fn group_origin(g: usize) -> Vec3 {
    Vec3::new(g as f32 * GROUP_DX, 0.0, 0.0)
}

/// Five groups of colliders, group `g` on layer `g` (0..=4): a cuboid at
/// the group origin, a cylinder 0.3 m up y, a two-triangle quad (z = 0
/// plane) 0.6 m up y.
fn layered_world() -> &'static StaticWorld {
    static W: OnceLock<StaticWorld> = OnceLock::new();
    W.get_or_init(|| {
        let mut b = StaticWorldBuilder::new();
        for g in 0..5usize {
            let o = group_origin(g);
            let mask = layers::layer_bit(g as u8);
            b.add_collider(
                StaticShape::Cuboid {
                    center: o,
                    half_extents: Vec3::splat(BOX_H),
                    rotation: Quat::from_rotation_z(0.3),
                },
                mask,
                true,
            );
            b.add_collider(
                StaticShape::Cylinder {
                    base: o + Vec3::new(0.0, 0.3, 0.0),
                    axis: Vec3::new(0.0, 0.0, CYL_H),
                    radius: CYL_R,
                },
                mask,
                true,
            );
            let c = o + Vec3::new(0.0, 0.6, 0.0);
            b.add_collider(
                StaticShape::TriMesh {
                    vertices: vec![
                        c + Vec3::new(-QUAD_H, -QUAD_H, 0.0),
                        c + Vec3::new(QUAD_H, -QUAD_H, 0.0),
                        c + Vec3::new(QUAD_H, QUAD_H, 0.0),
                        c + Vec3::new(-QUAD_H, QUAD_H, 0.0),
                    ],
                    indices: vec![[0, 1, 2], [0, 2, 3]],
                },
                mask,
                true,
            );
        }
        b.build()
    })
}

/// A point overlapping shape `shape` (0 cuboid, 1 cylinder, 2 quad) of
/// group `g`, from unit-cube parameters `u`.
fn overlap_point(g: usize, shape: usize, u: [f32; 3]) -> Vec3 {
    let o = group_origin(g);
    let s = |x: f32, h: f32| (2.0 * x - 1.0) * h;
    match shape {
        0 => {
            // Inside the box grown by 0.8 R (rotated with the box).
            let local = Vec3::new(s(u[0], BOX_H + 0.8 * R), s(u[1], BOX_H), s(u[2], BOX_H));
            o + Quat::from_rotation_z(0.3) * local
        }
        1 => {
            let a = u[0] * std::f32::consts::TAU;
            let r = u[1] * (CYL_R + 0.8 * R);
            o + Vec3::new(r * libm::cosf(a), 0.3 + r * libm::sinf(a), u[2] * CYL_H)
        }
        _ => o + Vec3::new(s(u[0], QUAD_H), 0.6 + s(u[1], QUAD_H), s(u[2], 0.8 * R)),
    }
}

/// Steps one ball of layer `layer` at `p` with `vel`, `spin` through one
/// tick of the zero-gravity world; returns the ball after the tick and
/// whether an impact was reported.
fn one_tick(layer: u8, p: Vec3, vel: Vec3, spin: Vec3) -> (pin_physics::ball::Ball, bool) {
    let world = layered_world();
    let mut s = solver(Vec3::ZERO);
    let mut b = ball(p, vel);
    b.angular_velocity = spin;
    b.layer = layer;
    let h = s.add_ball(b).unwrap();
    let mut ev = BallEvents::new();
    s.step(&Geometry::new(world), DT, &mut ev);
    let hit = ev
        .as_slice()
        .iter()
        .any(|e| matches!(e, BallEvent::Impact { .. }));
    (*s.ball(h).unwrap(), hit)
}

fn assert_passed_through(
    before: Vec3,
    vel: Vec3,
    spin: Vec3,
    after: &pin_physics::ball::Ball,
    hit: bool,
) {
    assert!(!hit, "impact reported");
    assert!(after.position.is_finite() && after.velocity.is_finite());
    let dv = (after.velocity - vel).length();
    assert!(
        dv <= 1e-6 * (1.0 + vel.length()),
        "velocity changed by {dv}: {vel} -> {}",
        after.velocity
    );
    let dw = (after.angular_velocity - spin).length();
    assert!(dw <= 1e-5 * (1.0 + spin.length()), "spin changed by {dw}");
    let expect = before + vel * DT;
    let dp = (after.position - expect).length();
    assert!(
        dp <= 1e-6,
        "position {} vs free flight {expect} ({dp})",
        after.position
    );
}

fn vec3(r: f32) -> impl Strategy<Value = Vec3> {
    (-r..r, -r..r, -r..r).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1024, ..ProptestConfig::default() })]

    /// Layer-4 ball inside / against layer 0–3 colliders, and layer 0–3
    /// ball inside / against layer-4 colliders: no collision.
    #[test]
    fn backbox_and_lower_layers_never_collide(
        g in 0usize..5,
        shape in 0usize..3,
        u in (0.0f32..1.0, 0.0f32..1.0, 0.0f32..1.0),
        other in 0u8..4,
        vel in vec3(5.0),
        spin in vec3(100.0),
    ) {
        let p = overlap_point(g, shape, [u.0, u.1, u.2]);
        // The collider group's layer and the ball's layer.
        let (collider_layer, ball_layer) = if g < 4 {
            (g as u8, BACKBOX_LAYER)
        } else {
            (BACKBOX_LAYER, other)
        };
        // The overlap is real on the collider's own layer...
        let mut out = ArrayVec::new();
        layered_world().sphere_contacts(p, R, layers::layer_bit(collider_layer), &mut out);
        prop_assert!(!out.is_empty(), "test point does not overlap group {g} shape {shape}: {p}");
        // ...and invisible on the ball's layer.
        layered_world().sphere_contacts(p, R, layers::layer_bit(ball_layer), &mut out);
        prop_assert!(out.is_empty(), "ball layer {ball_layer} sees group {g}");
        let (after, hit) = one_tick(ball_layer, p, vel, spin);
        assert_passed_through(p, vel, spin, &after, hit);
    }
}

#[test]
fn control_same_layer_ball_does_collide() {
    // Without this the pass-through property could hold vacuously.
    for g in 0..5usize {
        let o = group_origin(g);
        // Approach the top face of the cylinder (axis along z) from above.
        let p = o + Vec3::new(0.0, 0.3, CYL_H + R + 0.0005);
        let vel = Vec3::new(0.0, 0.0, -3.0);
        let (after, hit) = one_tick(g as u8, p, vel, Vec3::ZERO);
        assert!(hit, "group {g}: no impact on its own layer");
        assert!(
            after.velocity.z > 0.0,
            "group {g}: not reflected: {}",
            after.velocity
        );
        // The same approach from the backbox (or main deck for group 4)
        // passes.
        let other = if g < 4 { BACKBOX_LAYER } else { 0 };
        let (after, hit) = one_tick(other, p, vel, Vec3::ZERO);
        assert_passed_through(p, vel, Vec3::ZERO, &after, hit);
    }
}

/// A sweeping flipper bat on `bat_layer` against a ball on `ball_layer`
/// sitting in its path; returns (impact seen, max |Δv| of the ball).
fn bat_sweep(bat_layer: u8, ball_layer: u8, s: f32) -> (bool, f32) {
    let mut cfg = bb::left_flipper(0, Mount::Horizontal, 1.0);
    cfg.collision_layer = bat_layer;
    let mut set = FlipperSet::new(std::slice::from_ref(&cfg)).unwrap();
    let mut empty = StaticWorldBuilder::new();
    empty.add_collider(
        StaticShape::Cuboid {
            center: Vec3::splat(50.0),
            half_extents: Vec3::splat(0.01),
            rotation: Quat::IDENTITY,
        },
        layers::ALL,
        true,
    );
    let world = empty.build();
    set.advance(DT);
    let frame = bb::c23::measure_frame(&set, 0, &cfg);
    // In the bat's path: 25° above the rest direction, at `s` from the
    // pivot, level with the bat's mid-plane.
    let dir = frame.dir(cfg.rest_angle_deg + 25.0);
    let p = frame.pivot + dir * s;
    let mut sol = solver(Vec3::ZERO);
    let mut b = ball(p, Vec3::ZERO);
    b.layer = ball_layer;
    let h: BallHandle = sol.add_ball(b).unwrap();
    set.set_energised(FlipperId(0), true);
    let mut ev = BallEvents::new();
    let mut hit = false;
    let mut dv = 0.0f32;
    for t in 0..80u64 {
        set.step_tick(t);
        ev.clear();
        sol.step_with_bodies(&Geometry::new(&world), &mut [&mut set], DT, &mut ev);
        hit |= ev
            .as_slice()
            .iter()
            .any(|e| matches!(e, BallEvent::Impact { .. }));
        let bl = sol.ball(h).unwrap();
        assert!(bl.position.is_finite() && bl.velocity.is_finite());
        dv = dv.max(bl.velocity.length());
    }
    (hit, dv)
}

#[test]
fn flipper_bats_respect_layer_4() {
    for s in [0.02f32, 0.04, 0.06] {
        // Same layer: the bat drives the ball (control; a push from a bat
        // starting at rest may stay below the impact-report speed, so the
        // ball's speed is the evidence).
        let (_, dv) = bat_sweep(BACKBOX_LAYER, BACKBOX_LAYER, s);
        assert!(dv > 0.5, "s {s}: backbox bat missed a backbox ball ({dv})");
        let (_, dv) = bat_sweep(0, 0, s);
        assert!(dv > 0.5, "s {s}: deck bat missed a deck ball ({dv})");
        // Across bit 4: no contact at all.
        for l in 0..4u8 {
            let (hit, dv) = bat_sweep(BACKBOX_LAYER, l, s);
            assert!(
                !hit && dv == 0.0,
                "s {s}: backbox bat touched a layer-{l} ball ({dv})"
            );
            let (hit, dv) = bat_sweep(l, BACKBOX_LAYER, s);
            assert!(
                !hit && dv == 0.0,
                "s {s}: layer-{l} bat touched a backbox ball ({dv})"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Transition hand-off main -> backbox -> main.

/// A corridor along +y: layer-0 wall at y = 0.45 (between the zones, must
/// be passed on layer 4), layer-4 wall at y = 0.9 (after the return, must
/// be passed on layer 0), layer-0 wall at y = 1.1 (must stop the ball).
fn corridor() -> (StaticWorld, TransitionSet) {
    let mut b = StaticWorldBuilder::new();
    let wall = |y: f32| StaticShape::Cuboid {
        center: Vec3::new(0.0, y, 0.0),
        half_extents: Vec3::new(0.3, 0.01, 0.3),
        rotation: Quat::IDENTITY,
    };
    b.add_collider(wall(0.45), layers::MAIN_DECK, true);
    b.add_collider(wall(0.9), layers::BACKBOX, true);
    b.add_collider(wall(1.1), layers::MAIN_DECK, true);
    let zone = |from: u8, to: u8, y0: f32, y1: f32| LayerTransition {
        from_layer: from,
        to_layer: to,
        min: Vec3::new(-0.3, y0, -0.3),
        max: Vec3::new(0.3, y1, 0.3),
    };
    let zones = TransitionSet::new(vec![
        zone(0, BACKBOX_LAYER, 0.2, 0.3),
        zone(BACKBOX_LAYER, 0, 0.6, 0.7),
    ]);
    (b.build(), zones)
}

struct HandOff {
    layer_after_a: u8,
    layer_after_b: u8,
    speed_before_final_wall: f32,
    bounced: bool,
}

fn hand_off(speed: f32, x: f32, z: f32) -> HandOff {
    let (world, zones) = corridor();
    let mut s = solver(Vec3::ZERO);
    let v0 = Vec3::new(0.0, speed, 0.0);
    let h = s.add_ball(ball(Vec3::new(x, 0.0, z), v0)).unwrap();
    let profile = s.ball(h).unwrap().profile;
    let mut ev = BallEvents::new();
    let mut out = HandOff {
        layer_after_a: u8::MAX,
        layer_after_b: u8::MAX,
        speed_before_final_wall: 0.0,
        bounced: false,
    };
    let ticks = (1.6 / speed / DT) as u32 + 50;
    for _ in 0..ticks {
        let prev = s.ball(h).unwrap().position;
        ev.clear();
        s.step(&Geometry::new(&world), DT, &mut ev);
        bb::apply_transitions(&mut s, &zones, &[(h, prev)]);
        let b = *s.ball(h).expect("ball preserved");
        assert_eq!(s.ball_count(), 1);
        assert_eq!(b.profile, profile);
        assert!(b.position.is_finite() && b.velocity.is_finite(), "{b:?}");
        let y = b.position.y;
        if y > 0.3 + speed * DT && y < 0.6 {
            out.layer_after_a = b.layer;
        }
        if y > 0.7 + speed * DT && y < 1.0 && b.velocity.y > 0.0 {
            out.layer_after_b = b.layer;
            out.speed_before_final_wall = b.velocity.length();
        }
        if b.velocity.y < 0.0 {
            out.bounced = true;
            assert_eq!(
                b.layer, 0,
                "bounced off the final wall on layer {}",
                b.layer
            );
            break;
        }
    }
    out
}

#[test]
fn transition_hands_ball_main_to_backbox_and_back() {
    let r = hand_off(2.0, 0.0, 0.0);
    assert_eq!(
        r.layer_after_a, BACKBOX_LAYER,
        "not on the backbox after zone A"
    );
    assert_eq!(r.layer_after_b, 0, "not back on the main deck after zone B");
    assert!(
        (r.speed_before_final_wall - 2.0).abs() < 1e-5,
        "speed {}",
        r.speed_before_final_wall
    );
    assert!(
        r.bounced,
        "the main-deck wall after the return did not stop the ball"
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn hand_off_round_trip_for_any_entry(
        speed in 0.3f32..8.0,
        x in -0.25f32..0.25,
        z in -0.25f32..0.25,
    ) {
        let r = hand_off(speed, x, z);
        prop_assert_eq!(r.layer_after_a, BACKBOX_LAYER);
        prop_assert_eq!(r.layer_after_b, 0);
        prop_assert!((r.speed_before_final_wall - speed).abs() <= 1e-5 * speed.max(1.0));
        prop_assert!(r.bounced);
    }
}

#[test]
fn transition_zone_fires_once_per_entry() {
    let (_, zones) = corridor();
    let a_in = Vec3::new(0.0, 0.25, 0.0);
    let before = Vec3::new(0.0, 0.1, 0.0);
    assert_eq!(zones.update(0, before, a_in), BACKBOX_LAYER);
    // Staying inside does not fire again; a backbox ball is not affected by
    // the 0 -> 4 zone.
    assert_eq!(
        zones.update(BACKBOX_LAYER, a_in, a_in + Vec3::Y * 0.01),
        BACKBOX_LAYER
    );
    assert_eq!(zones.update(BACKBOX_LAYER, before, a_in), BACKBOX_LAYER);
    // Return zone: only a backbox ball is handed back.
    let b_in = Vec3::new(0.0, 0.65, 0.0);
    let b_before = Vec3::new(0.0, 0.5, 0.0);
    assert_eq!(zones.update(BACKBOX_LAYER, b_before, b_in), 0);
    for l in [0u8, 1, 2, 3] {
        assert_eq!(zones.update(l, b_before, b_in), l);
    }
    // NaN positions never fire.
    assert_eq!(zones.update(0, before, Vec3::NAN), 0);
}
