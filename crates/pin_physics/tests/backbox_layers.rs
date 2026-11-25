//! Subphase 5.5, verification criterion 2: a backbox ball (layer 4) never
//! collides with layer 0–3 geometry and a main-deck ball never collides with
//! the backbox's layer-4 geometry; a layer-transition hand-off main →
//! backbox → main keeps the ball (no loss, no NaN).
//!
//! World frame: `z` up, gravity `(0, 0, −g)`. The scene ([`scene`]) is a
//! 6.5° main deck (layer 0) with a vertical backbox mini-playfield (layer 4)
//! above its far end, and decoys on layers 0–3 cutting through the backbox
//! interior and the space under it.
//!
//! Isolation is checked two ways for random start states: (1) every
//! reported static impact is with a collider whose mask has the ball's layer
//! bit; (2) the trajectory is bit-identical to the same run in a world that
//! holds only the colliders of the ball's layer (so no other collider took
//! part in any contact, reported or resting).

#[path = "backbox_util.rs"]
mod util;

use glam::{Quat, Vec3};
use pin_core::fmath::max_f32;
use pin_physics::ball::{
    BallEvent, BallEvents, BallSolver, BallSolverConfig, Geometry, ImpactTarget,
};
use pin_physics::world::{
    LayerTransition, StaticShape, StaticWorld, StaticWorldBuilder, TransitionSet, layers,
};
use proptest::prelude::*;
use util::*;

/// Backbox interior (world, m): across.
const BB_X: (f32, f32) = (-0.20, 0.20);
/// Glass and back plane `y`.
const BB_Y: (f32, f32) = (0.500, 0.535);
/// Shelf (bottom) and top `z`.
const BB_Z: (f32, f32) = (0.25, 0.55);
/// Entry hole in the shelf (VUK feed), across.
const ENTRY_X: (f32, f32) = (-0.17, -0.07);
/// Drain hole in the shelf, across (against the right wall).
const DRAIN_X: (f32, f32) = (0.14, 0.20);
/// Slope of the shelf (falls towards +x, the drain), degrees.
const SHELF_DEG: f32 = 8.0;

/// One collider of the scene with its mask (in insertion order).
fn scene() -> Vec<(StaticShape, u8)> {
    let mut out = Vec::new();
    let deck = pitched_frame(PITCH_DEG);
    // Main deck: 2 m wide, 1.4 m long, layer 0.
    out.push((plane_slab(deck, Vec3::ZERO, 1.0, 0.7), layers::MAIN_DECK));
    let cy = 0.5 * (BB_Y.0 + BB_Y.1);
    let cz = 0.5 * (BB_Z.0 + BB_Z.1);
    let hx = 0.5 * (BB_X.1 - BB_X.0);
    let hz = 0.5 * (BB_Z.1 - BB_Z.0);
    let bb = layers::BACKBOX;
    // Back plane and glass: 90° planes (horizontal normals), layer 4.
    let wall = |y: f32, ny: f32| StaticShape::Cuboid {
        center: v(0.0, y + ny * 0.005, cz),
        half_extents: v(hx, 0.005, hz),
        rotation: Quat::IDENTITY,
    };
    out.push((wall(BB_Y.1, 1.0), bb));
    out.push((wall(BB_Y.0, -1.0), bb));
    // Side walls and roof.
    for (x, s) in [(BB_X.0, -1.0f32), (BB_X.1, 1.0)] {
        out.push((
            StaticShape::Cuboid {
                center: v(x + s * 0.005, cy, cz),
                half_extents: v(0.005, 0.03, hz),
                rotation: Quat::IDENTITY,
            },
            bb,
        ));
    }
    out.push((
        StaticShape::Cuboid {
            center: v(0.0, cy, BB_Z.1 + 0.005),
            half_extents: v(hx, 0.03, 0.005),
            rotation: Quat::IDENTITY,
        },
        bb,
    ));
    // Shelf in two pieces around the entry hole, ending at the drain hole,
    // sloped towards the drain.
    let tilt = Quat::from_rotation_y(SHELF_DEG.to_radians());
    let slope = libm::tanf(SHELF_DEG.to_radians());
    let shelf_z = |x: f32| BB_Z.0 - slope * (x - BB_X.0);
    for (x0, x1) in [(BB_X.0, ENTRY_X.0), (ENTRY_X.1, DRAIN_X.0)] {
        let xm = 0.5 * (x0 + x1);
        out.push((
            StaticShape::Cuboid {
                center: v(xm, cy, shelf_z(xm)) + tilt * v(0.0, 0.0, -0.005),
                half_extents: v(
                    0.5 * (x1 - x0) / libm::cosf(SHELF_DEG.to_radians()),
                    0.03,
                    0.005,
                ),
                rotation: tilt,
            },
            bb,
        ));
    }
    // Decoys. Layer 0: a roof plate over the VUK path; layer 1: an upper
    // deck slicing the backbox interior; layer 2: a wireform-height block in
    // the slot; layer 3: a subway box under the drain. A backbox ball passes
    // through all of them.
    out.push((
        StaticShape::Cuboid {
            center: v(-0.125, cy, 0.17),
            half_extents: v(0.06, 0.06, 0.005),
            rotation: Quat::IDENTITY,
        },
        layers::MAIN_DECK,
    ));
    out.push((
        StaticShape::Cuboid {
            center: v(0.0, cy, 0.40),
            half_extents: v(0.25, 0.08, 0.004),
            rotation: Quat::IDENTITY,
        },
        layers::UPPER_DECK,
    ));
    out.push((
        StaticShape::Cuboid {
            center: v(-0.05, cy, 0.33),
            half_extents: v(0.03, 0.05, 0.03),
            rotation: Quat::IDENTITY,
        },
        layers::WIREFORM,
    ));
    out.push((
        StaticShape::Cuboid {
            center: v(0.17, cy, 0.30),
            half_extents: v(0.03, 0.05, 0.02),
            rotation: Quat::IDENTITY,
        },
        layers::SUBWAY,
    ));
    // A layer-4 shelf that slices the main deck's space: a main-deck ball
    // passes through it.
    out.push((
        StaticShape::Cuboid {
            center: v(0.0, 0.2, 0.06),
            half_extents: v(0.3, 0.05, 0.004),
            rotation: Quat::IDENTITY,
        },
        bb,
    ));
    out
}

/// The scene's colliders whose mask has a bit of `keep` (order preserved).
fn world(keep: u8) -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    for (shape, mask) in scene() {
        if mask & keep != 0 {
            b.add_collider(shape, mask, true);
        }
    }
    b.build()
}

fn solver() -> BallSolver {
    let mut cfg = BallSolverConfig::from_defaults(&pin_core::PhysicsDefaults::builtin());
    cfg.gravity = gravity();
    BallSolver::new(cfg, &profiles())
}

/// Runs one ball for `ticks` in `w`; returns the trajectory bits and the
/// masks of the colliders it hit.
fn run(w: &StaticWorld, pos: Vec3, vel: Vec3, layer: u8, ticks: u32) -> (Vec<u32>, Vec<u8>) {
    let mut s = solver();
    let mut ball = pin_physics::ball::Ball::new(STEEL, pos);
    ball.velocity = vel;
    ball.layer = layer;
    let h = s.add_ball(ball).unwrap();
    let mut events = BallEvents::new();
    let mut bits = Vec::new();
    let mut hits = Vec::new();
    for _ in 0..ticks {
        events.clear();
        s.step(&Geometry::new(w), DT, &mut events);
        let b = s.ball(h).expect("the ball is never lost");
        assert!(b.position.is_finite() && b.velocity.is_finite(), "no NaN");
        for x in b
            .position
            .to_array()
            .into_iter()
            .chain(b.velocity.to_array())
        {
            bits.push(x.to_bits());
        }
        for e in events.as_slice() {
            if let BallEvent::Impact {
                other: ImpactTarget::Collider(c),
                ..
            } = *e
            {
                hits.push(w.collider_layers(c));
            }
        }
    }
    (bits, hits)
}

fn start_state() -> impl Strategy<Value = (Vec3, Vec3)> {
    (
        -0.19f32..0.19,
        0.10f32..0.53,
        0.02f32..0.54,
        -3.0f32..3.0,
        -1.0f32..1.0,
        -3.0f32..3.0,
    )
        .prop_map(|(x, y, z, vx, vy, vz)| (v(x, y, z), v(vx, vy, vz)))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    /// A layer-4 ball never touches layer 0–3 geometry.
    #[test]
    fn backbox_ball_ignores_layers_0_to_3((pos, vel) in start_state()) {
        let full = world(layers::ALL);
        let own = world(layers::BACKBOX);
        let (bits, hits) = run(&full, pos, vel, BACKBOX_LAYER, 250);
        for m in &hits {
            prop_assert!(m & layers::BACKBOX != 0, "hit a collider of mask {m:#010b}");
        }
        let (alone, _) = run(&own, pos, vel, BACKBOX_LAYER, 250);
        prop_assert!(bits == alone, "layer 0-3 colliders changed the backbox ball");
    }

    /// A ball on any of layers 0–3 never touches the backbox's layer-4 geometry.
    #[test]
    fn layer_0_to_3_balls_ignore_the_backbox((pos, vel) in start_state(), layer in 0u8..4) {
        let full = world(layers::ALL);
        let own = world(layers::layer_bit(layer));
        let (bits, hits) = run(&full, pos, vel, layer, 250);
        for m in &hits {
            prop_assert!(m & layers::layer_bit(layer) != 0, "layer {layer} hit mask {m:#010b}");
            prop_assert!(m & layers::BACKBOX == 0);
        }
        let (alone, _) = run(&own, pos, vel, layer, 250);
        prop_assert!(bits == alone, "backbox colliders changed a layer-{layer} ball");
    }

    /// Main → backbox → main through the transition zones, for a sweep of
    /// approach and VUK speeds.
    #[test]
    fn hand_off_main_backbox_main_keeps_the_ball(
        approach in 0.05f32..0.4,
        eject in 2.75f32..2.95,
        kick in 0.3f32..0.5,
    ) {
        let r = hand_off(approach, eject, kick);
        prop_assert_eq!(r.layers, vec![0, 4, 0]);
        prop_assert!(r.backbox_ticks > 50, "time on the backbox {}", r.backbox_ticks);
        prop_assert!(r.end.is_finite());
    }
}

/// Outcome of one hand-off run.
#[derive(Debug)]
struct HandOff {
    /// Distinct layers in order.
    layers: Vec<u8>,
    /// Ticks spent on layer 4.
    backbox_ticks: u32,
    /// Final position.
    end: Vec3,
}

/// The table's transition zones: the VUK scoop under the entry hole (0 → 4)
/// and the drain chute under the drain hole (4 → 0).
fn transitions() -> TransitionSet {
    let cy = 0.5 * (BB_Y.0 + BB_Y.1);
    TransitionSet::new(vec![
        LayerTransition {
            from_layer: 0,
            to_layer: BACKBOX_LAYER,
            min: v(-0.15, cy - 0.02, 0.0),
            max: v(-0.13, cy + 0.02, 0.12),
        },
        LayerTransition {
            from_layer: BACKBOX_LAYER,
            to_layer: 0,
            min: v(DRAIN_X.0, cy - 0.04, 0.10),
            max: v(DRAIN_X.1 + 0.08, cy + 0.04, 0.17),
        },
    ])
}

/// A ball rolls across the top of the main deck into the VUK zone; on the
/// 0 → 4 transition the VUK fires (as the generated `vuk` mech with
/// `exit_layer = 4` does: straight up at `eject` m/s with `kick` m/s towards
/// the drain); it rides the backbox shelf to the drain, falls through the
/// 4 → 0 zone and lands back on the main deck.
fn hand_off(approach: f32, eject: f32, kick: f32) -> HandOff {
    let w = world(layers::ALL);
    let zones = transitions();
    let mut s = solver();
    let cy = 0.5 * (BB_Y.0 + BB_Y.1);
    let deck = pitched_frame(PITCH_DEG);
    // On the deck surface at world y = cy, left of the entry hole.
    let local_y = cy / libm::cosf(PITCH_DEG.to_radians());
    let on_deck = deck * v(-0.19, local_y, BALL_R);
    let mut ball = pin_physics::ball::Ball::new(STEEL, on_deck);
    ball.velocity = deck * v(max_f32(approach, 0.05) + 0.6, 0.0, 0.0);
    let h = s.add_ball(ball).unwrap();
    let mut events = BallEvents::new();
    let mut seen = vec![0u8];
    let mut backbox_ticks = 0;
    let mut back_ticks = 0;
    let mut prev = on_deck;
    for _ in 0..3000 {
        events.clear();
        s.step(&Geometry::new(&w), DT, &mut events);
        let b = *s.ball(h).expect("the ball is never lost");
        assert!(b.position.is_finite() && b.velocity.is_finite(), "no NaN");
        for e in events.as_slice() {
            if let BallEvent::Impact {
                other: ImpactTarget::Collider(c),
                ..
            } = *e
            {
                assert!(
                    w.collider_layers(c) & layers::layer_bit(b.layer) != 0,
                    "layer {} ball hit mask {:#010b}",
                    b.layer,
                    w.collider_layers(c)
                );
            }
        }
        let layer = zones.update(b.layer, prev, b.position);
        if layer != b.layer {
            let m = s.ball_mut(h).unwrap();
            m.layer = layer;
            if layer == BACKBOX_LAYER {
                // The VUK eject (test stand-in for the mech).
                m.velocity = v(kick, 0.0, eject);
            }
            seen.push(layer);
        }
        if layer == BACKBOX_LAYER {
            backbox_ticks += 1;
            let p = b.position;
            // Above the walls' lower edge it is inside the backbox; below
            // it, it is in the VUK lift or the drain chute, never lower.
            if p.z > BB_Z.0 {
                assert!(
                    p.x > BB_X.0 && p.x < BB_X.1 && p.y > BB_Y.0 && p.y < BB_Y.1,
                    "a backbox ball stays inside the backbox: {p:?}"
                );
            }
            assert!(p.z > 0.06, "a backbox ball never sinks to the deck: {p:?}");
        }
        prev = b.position;
        if seen.len() == 3 {
            back_ticks += 1;
            if back_ticks == 300 {
                break;
            }
        }
    }
    HandOff {
        layers: seen,
        backbox_ticks,
        end: s.ball(h).unwrap().position,
    }
}

#[test]
fn hand_off_reference_run() {
    let r = hand_off(0.2, 2.85, 0.4);
    println!("{r:?}");
    assert_eq!(r.layers, vec![0, 4, 0]);
    // Back on the main deck surface.
    let deck = pitched_frame(PITCH_DEG).inverse() * r.end;
    assert!((deck.z - BALL_R).abs() < 1.0e-3, "on the deck: {deck:?}");
}
