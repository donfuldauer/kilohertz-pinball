//! Implementer tests for layer-transition zones (Subphase 2.1).

use glam::Vec3;
use pin_physics::world::{LayerTransition, TransitionSet};
use proptest::prelude::*;

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

fn zone(from: u8, to: u8, min: Vec3, max: Vec3) -> LayerTransition {
    LayerTransition {
        from_layer: from,
        to_layer: to,
        min,
        max,
    }
}

#[test]
fn enter_switches_once() {
    let set = TransitionSet::new(vec![zone(0, 2, v(0.0, 0.0, 0.0), v(0.1, 0.1, 0.1))]);
    assert_eq!(set.zones().len(), 1);
    let outside = v(-0.05, 0.05, 0.05);
    let inside = v(0.05, 0.05, 0.05);
    let deeper = v(0.08, 0.05, 0.05);
    let beyond = v(0.2, 0.05, 0.05);
    // Entering on the from-layer switches.
    assert_eq!(set.update(0, outside, inside), 2);
    // Staying inside does nothing, on either layer.
    assert_eq!(set.update(2, inside, deeper), 2);
    assert_eq!(set.update(0, inside, deeper), 0);
    // Leaving does nothing.
    assert_eq!(set.update(2, deeper, beyond), 2);
    assert_eq!(set.update(0, deeper, beyond), 0);
    // Entering on another layer does nothing.
    assert_eq!(set.update(1, outside, inside), 1);
    // Not reaching the zone does nothing.
    assert_eq!(set.update(0, outside, v(-0.01, 0.05, 0.05)), 0);
    // Bounds are inclusive.
    assert_eq!(set.update(0, outside, v(0.0, 0.1, 0.0)), 2);
    assert!(set.zones()[0].contains(v(0.1, 0.1, 0.1)));
    assert!(!set.zones()[0].contains(v(0.1, 0.1, 0.1001)));
    assert!(!set.zones()[0].contains(v(f32::NAN, 0.05, 0.05)));
    // NaN positions never switch into a zone.
    assert_eq!(set.update(0, outside, v(f32::NAN, 0.0, 0.0)), 0);
}

#[test]
fn first_matching_zone_wins_and_no_chaining() {
    let big = (v(0.0, 0.0, 0.0), v(1.0, 1.0, 1.0));
    let set = TransitionSet::new(vec![
        zone(0, 1, big.0, big.1),
        zone(1, 3, big.0, big.1),
        zone(0, 4, big.0, big.1),
    ]);
    let outside = v(-1.0, 0.5, 0.5);
    let inside = v(0.5, 0.5, 0.5);
    assert_eq!(set.update(0, outside, inside), 1); // not 3, not 4
    assert_eq!(set.update(1, outside, inside), 3);
    assert_eq!(set.update(1, inside, inside), 1);
    assert_eq!(TransitionSet::default().update(5, outside, inside), 5);
}

#[test]
fn up_and_down_a_ramp() {
    // Entrance zone main -> wireform, exit zone wireform -> upper.
    let set = TransitionSet::new(vec![
        zone(0, 2, v(0.0, -0.05, 0.0), v(0.05, 0.05, 0.05)),
        zone(2, 1, v(0.5, -0.05, 0.0), v(0.55, 0.05, 0.1)),
    ]);
    let mut layer = 0u8;
    let mut switches = 0;
    let mut prev = v(-0.2, 0.0, 0.02);
    for k in 1..=1000 {
        let pos = v(-0.2 + k as f32 * 0.001, 0.0, 0.02);
        let next = set.update(layer, prev, pos);
        if next != layer {
            switches += 1;
        }
        layer = next;
        prev = pos;
    }
    assert_eq!(layer, 1);
    assert_eq!(switches, 2);
}

proptest! {
    #![proptest_config(ProptestConfig { failure_persistence: None, ..ProptestConfig::default() })]

    /// Along any polyline, the layer changes only on a step that enters a
    /// zone of the current layer, and exactly once per entry.
    #[test]
    fn prop_switches_exactly_on_entry(
        path in prop::collection::vec((-0.2f32..0.4, -0.2f32..0.4), 2..60),
        from in 0u8..3,
        start_layer in 0u8..3,
    ) {
        let z = zone(from, 7, v(0.0, 0.0, -1.0), v(0.2, 0.2, 1.0));
        let set = TransitionSet::new(vec![z]);
        let mut layer = start_layer;
        for w in path.windows(2) {
            let (a, b) = (v(w[0].0, w[0].1, 0.0), v(w[1].0, w[1].1, 0.0));
            let next = set.update(layer, a, b);
            let entered = !z.contains(a) && z.contains(b);
            if entered && layer == from {
                prop_assert_eq!(next, 7);
            } else {
                prop_assert_eq!(next, layer);
            }
            layer = next;
        }
    }
}
