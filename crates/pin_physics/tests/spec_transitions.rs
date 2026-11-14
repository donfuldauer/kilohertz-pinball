//! Independent spec tests (Subphase 2.1): layer-transition trigger zones
//! (ramp entrances, VUKs, drop holes).
//!
//! Black-box against `pin_physics::world::{LayerTransition, TransitionSet}`.

use glam::Vec3;
use pin_physics::world::layers::{MAIN_DECK, SUBWAY, UPPER_DECK, WIREFORM};
use pin_physics::world::{LayerTransition, TransitionSet};
use proptest::prelude::*;

fn ramp_entrance() -> LayerTransition {
    // Zone at the foot of a ramp: x ∈ [0.4, 0.5], y ∈ [-0.05, 0.05], z ∈ [0, 0.1].
    LayerTransition {
        from_layer: MAIN_DECK,
        to_layer: UPPER_DECK,
        min: Vec3::new(0.4, -0.05, 0.0),
        max: Vec3::new(0.5, 0.05, 0.1),
    }
}

fn inside(z: &LayerTransition, p: Vec3) -> bool {
    p.cmpge(z.min).all() && p.cmple(z.max).all()
}

/// Runs a straight path at 1 kHz from `a` to `b` in `steps` ticks and returns
/// (final layer, number of layer changes).
fn run_path(set: &TransitionSet, start_layer: u8, a: Vec3, b: Vec3, steps: usize) -> (u8, usize) {
    let mut layer = start_layer;
    let mut changes = 0;
    let mut prev = a;
    for k in 1..=steps {
        let pos = a.lerp(b, k as f32 / steps as f32);
        let next = set.update(layer, prev, pos);
        if next != layer {
            changes += 1;
        }
        layer = next;
        prev = pos;
    }
    (layer, changes)
}

#[test]
fn entering_zone_switches_layer() {
    let set = TransitionSet::new(vec![ramp_entrance()]);
    let prev = Vec3::new(0.35, 0.0, 0.0135);
    let pos = Vec3::new(0.42, 0.0, 0.0135);
    assert_eq!(set.update(MAIN_DECK, prev, pos), UPPER_DECK);
}

#[test]
fn outside_or_wrong_layer_does_not_switch() {
    let set = TransitionSet::new(vec![ramp_entrance()]);
    let a = Vec3::new(0.0, 0.0, 0.0135);
    let b = Vec3::new(0.1, 0.0, 0.0135);
    assert_eq!(
        set.update(MAIN_DECK, a, b),
        MAIN_DECK,
        "nowhere near the zone"
    );
    let prev = Vec3::new(0.35, 0.0, 0.0135);
    let pos = Vec3::new(0.45, 0.0, 0.0135);
    assert_eq!(set.update(SUBWAY, prev, pos), SUBWAY, "not on from_layer");
    assert_eq!(
        set.update(UPPER_DECK, prev, pos),
        UPPER_DECK,
        "already on to_layer"
    );
    // Passing beside the zone (y outside).
    let prev = Vec3::new(0.35, 0.2, 0.0135);
    let pos = Vec3::new(0.45, 0.2, 0.0135);
    assert_eq!(set.update(MAIN_DECK, prev, pos), MAIN_DECK);
    // Passing above the zone (z outside), e.g. a ball on a wireform over it.
    let prev = Vec3::new(0.35, 0.0, 0.3);
    let pos = Vec3::new(0.45, 0.0, 0.3);
    assert_eq!(set.update(MAIN_DECK, prev, pos), MAIN_DECK);
}

#[test]
fn staying_inside_or_leaving_does_not_switch() {
    let set = TransitionSet::new(vec![ramp_entrance()]);
    let in1 = Vec3::new(0.42, 0.0, 0.0135);
    let in2 = Vec3::new(0.45, 0.0, 0.0135);
    let out = Vec3::new(0.6, 0.0, 0.0135);
    // prev inside, now inside: not an entry.
    assert_eq!(set.update(MAIN_DECK, in1, in2), MAIN_DECK);
    // Leaving.
    assert_eq!(set.update(MAIN_DECK, in2, out), MAIN_DECK);
    assert_eq!(set.update(UPPER_DECK, in2, out), UPPER_DECK);
}

#[test]
fn ball_lingering_in_zone_switches_exactly_once() {
    let set = TransitionSet::new(vec![ramp_entrance()]);
    let mut layer = MAIN_DECK;
    let mut changes = 0;
    let mut prev = Vec3::new(0.3, 0.0, 0.0135);
    // Roll in slowly, jiggle around inside for 500 ticks, then roll out.
    let mut path = Vec::new();
    for k in 0..=200 {
        path.push(Vec3::new(0.3 + 0.15 * k as f32 / 200.0, 0.0, 0.0135));
    }
    for k in 0..500 {
        let j = if k % 2 == 0 { 0.01 } else { -0.01 };
        path.push(Vec3::new(0.45 + j, j, 0.0135 + 0.5 * j.abs()));
    }
    for k in 0..=200 {
        path.push(Vec3::new(0.45 + 0.2 * k as f32 / 200.0, 0.0, 0.0135));
    }
    for pos in path {
        let next = set.update(layer, prev, pos);
        if next != layer {
            changes += 1;
            assert!(inside(&ramp_entrance(), pos) && !inside(&ramp_entrance(), prev));
        }
        layer = next;
        prev = pos;
    }
    assert_eq!(layer, UPPER_DECK);
    assert_eq!(changes, 1);
}

#[test]
fn rolling_back_out_does_not_switch_again() {
    // A ball that fails the ramp and rolls back down through the entrance
    // zone stays on whatever layer it is on; only entries while on from_layer switch.
    let set = TransitionSet::new(vec![ramp_entrance()]);
    let a = Vec3::new(0.3, 0.0, 0.0135);
    let b = Vec3::new(0.48, 0.0, 0.0135);
    let (l1, c1) = run_path(&set, MAIN_DECK, a, b, 50);
    assert_eq!((l1, c1), (UPPER_DECK, 1));
    let (l2, c2) = run_path(&set, l1, b, a, 50);
    assert_eq!((l2, c2), (UPPER_DECK, 0));
}

#[test]
fn round_trip_through_entrance_and_exit_zones() {
    // Ramp: entrance (main -> upper) and an exit zone (upper -> main) at the far end.
    let exit = LayerTransition {
        from_layer: UPPER_DECK,
        to_layer: MAIN_DECK,
        min: Vec3::new(1.0, -0.05, 0.0),
        max: Vec3::new(1.1, 0.05, 0.1),
    };
    let set = TransitionSet::new(vec![ramp_entrance(), exit]);
    let a = Vec3::new(0.2, 0.0, 0.0135);
    let b = Vec3::new(1.3, 0.0, 0.0135);
    // 8 m/s over 1.1 m at 1 kHz ≈ 137 ticks.
    let (layer, changes) = run_path(&set, MAIN_DECK, a, b, 137);
    assert_eq!(layer, MAIN_DECK);
    assert_eq!(changes, 2, "exactly one switch per transition");
}

#[test]
fn drop_hole_and_vuk_chain() {
    // Drop hole: main -> subway; VUK: subway -> wireform.
    let hole = LayerTransition {
        from_layer: MAIN_DECK,
        to_layer: SUBWAY,
        min: Vec3::new(-0.02, -0.02, -0.1),
        max: Vec3::new(0.02, 0.02, 0.0),
    };
    let vuk = LayerTransition {
        from_layer: SUBWAY,
        to_layer: WIREFORM,
        min: Vec3::new(-0.02, 0.3, -0.1),
        max: Vec3::new(0.02, 0.34, 0.3),
    };
    let set = TransitionSet::new(vec![hole, vuk]);
    // Ball drops into the hole.
    let l = set.update(
        MAIN_DECK,
        Vec3::new(0.0, 0.0, 0.0135),
        Vec3::new(0.0, 0.0, -0.01),
    );
    assert_eq!(l, SUBWAY);
    // Travels in the subway to the VUK and enters it.
    let (l, c) = run_path(
        &set,
        l,
        Vec3::new(0.0, 0.0, -0.05),
        Vec3::new(0.0, 0.32, -0.05),
        100,
    );
    assert_eq!((l, c), (WIREFORM, 1));
    // A ball on the main deck rolling over the VUK zone does not enter it.
    let (l, c) = run_path(
        &set,
        MAIN_DECK,
        Vec3::new(0.0, 0.2, 0.0135),
        Vec3::new(0.0, 0.5, 0.0135),
        100,
    );
    assert_eq!((l, c), (MAIN_DECK, 0));
}

#[test]
fn empty_set_never_switches() {
    let set = TransitionSet::new(Vec::new());
    for l in [MAIN_DECK, UPPER_DECK, WIREFORM, SUBWAY] {
        assert_eq!(set.update(l, Vec3::ZERO, Vec3::ONE), l);
    }
}

// ---------------------------------------------------------------- property tests

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    /// For a single zone, the layer changes on a tick iff the ball is on
    /// from_layer and its centre went from outside to inside; it never takes
    /// any value other than current or to_layer.
    #[test]
    fn prop_single_zone_semantics(
        start in (-0.2f32..0.8, -0.2f32..0.2, -0.05f32..0.15),
        steps in prop::collection::vec((-0.03f32..0.03, -0.03f32..0.03, -0.02f32..0.02), 1..400),
        start_layer in prop::sample::select(vec![MAIN_DECK, UPPER_DECK, SUBWAY]),
    ) {
        let zone = ramp_entrance();
        let set = TransitionSet::new(vec![zone]);
        let mut layer = start_layer;
        let mut prev = Vec3::new(start.0, start.1, start.2);
        let mut switches = 0;
        for (dx, dy, dz) in steps {
            let pos = prev + Vec3::new(dx, dy, dz);
            let next = set.update(layer, prev, pos);
            let entered = !inside(&zone, prev) && inside(&zone, pos);
            // Skip boundary-ambiguous ticks (within 1 µm of a face).
            let near_face = |p: Vec3| {
                (p - zone.min).abs().min_element() < 1e-6 || (p - zone.max).abs().min_element() < 1e-6
            };
            if !near_face(prev) && !near_face(pos) {
                if layer == zone.from_layer && entered {
                    prop_assert_eq!(next, zone.to_layer);
                } else {
                    prop_assert_eq!(next, layer);
                }
            }
            prop_assert!(next == layer || next == zone.to_layer);
            if next != layer {
                switches += 1;
            }
            layer = next;
            prev = pos;
        }
        // Once on to_layer the single zone can never switch again.
        prop_assert!(switches <= 1);
    }

    #[test]
    fn prop_straight_crossing_switches_once(
        y in -0.04f32..0.04,
        z in 0.01f32..0.09,
        ticks in 5usize..400,
    ) {
        let set = TransitionSet::new(vec![ramp_entrance()]);
        let (layer, changes) = run_path(&set, MAIN_DECK, Vec3::new(0.0, y, z), Vec3::new(0.9, y, z), ticks.max(10));
        // At ≥10 ticks over 0.9 m the step is ≤ 90 mm < zone depth (100 mm) so
        // some tick lands inside.
        prop_assert_eq!(layer, UPPER_DECK);
        prop_assert_eq!(changes, 1);
    }
}
