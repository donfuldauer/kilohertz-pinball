//! Subphase 5.5, gate step 6: stepping the backbox scene (a vertical flipper
//! on layer 4, balls on the backbox and the main deck, layer transitions)
//! allocates nothing, and the run is bit-identical over 10 repetitions.

#[path = "backbox_util.rs"]
mod util;

use glam::{Quat, Vec3};
use pin_physics::world::{LayerTransition, StaticShape, StaticWorldBuilder, TransitionSet, layers};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use util::*;

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn bump() {
    // `try_with` so allocations during thread teardown are not a problem.
    let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
}

// SAFETY: forwards to the system allocator, only adding a thread-local counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

/// The backbox scene: a vertical playfield (layer 4) with a flipper, a
/// 6.5° main deck (layer 0) below it, transitions between them, and three
/// balls (two on the backbox, one on the deck).
fn backbox_rig() -> (Rig, TransitionSet) {
    let frame = vertical_frame();
    let deck = pitched_frame(PITCH_DEG);
    let mut b = StaticWorldBuilder::new();
    // Backbox plane at world y = 0.5 (normal −y), shelf, glass.
    let origin = v(0.0, 0.5, 0.3);
    b.add_collider(plane_slab(frame, origin, 0.25, 0.2), layers::BACKBOX, true);
    b.add_collider(
        StaticShape::Cuboid {
            center: origin + v(0.0, -0.035 - 0.005, 0.0),
            half_extents: v(0.25, 0.005, 0.2),
            rotation: Quat::IDENTITY,
        },
        layers::BACKBOX,
        true,
    );
    b.add_collider(
        StaticShape::Cuboid {
            center: origin + v(0.0, -0.02, -0.12),
            half_extents: v(0.25, 0.03, 0.005),
            rotation: Quat::from_rotation_y(0.15),
        },
        layers::BACKBOX,
        true,
    );
    b.add_collider(
        plane_slab(deck, Vec3::ZERO, 1.0, 1.0),
        layers::MAIN_DECK,
        true,
    );
    let cfg = flipper_config(
        frame,
        (frame.inverse() * origin) + v(-0.06, -0.08, 0.0),
        BACKBOX_LAYER,
        dedicated(),
    );
    let mut rig = Rig::new(&[cfg], b.build());
    let slot = origin.y - BALL_R - 0.004;
    rig.add_ball(v(-0.03, slot, 0.30), v(0.2, 0.0, -0.5), BACKBOX_LAYER);
    rig.add_ball(v(0.05, slot, 0.35), v(-0.4, 0.0, 0.3), BACKBOX_LAYER);
    rig.add_ball(deck * v(0.1, 0.2, BALL_R), deck * v(0.0, 1.5, 0.0), 0);
    let zones = TransitionSet::new(vec![
        LayerTransition {
            from_layer: 0,
            to_layer: BACKBOX_LAYER,
            min: v(-1.0, 0.40, -0.1),
            max: v(1.0, 0.45, 0.2),
        },
        LayerTransition {
            from_layer: BACKBOX_LAYER,
            to_layer: 0,
            min: v(-1.0, 0.40, 0.0),
            max: v(1.0, 0.6, 0.12),
        },
    ]);
    (rig, zones)
}

/// One engine-order tick of the backbox scene: inputs, flippers + balls,
/// then the layer transitions for every ball.
fn tick(rig: &mut Rig, zones: &TransitionSet, prev: &mut [Vec3; 3], t: u64) {
    rig.flippers.set_energised(BB_FLIPPER, (t / 120) % 2 == 1);
    rig.step();
    for i in 0..3u8 {
        let h = pin_physics::ball::BallHandle(i);
        if let Some(b) = rig.solver.ball_mut(h) {
            let layer = zones.update(b.layer, prev[usize::from(i)], b.position);
            b.layer = layer;
            prev[usize::from(i)] = b.position;
        }
    }
}

fn run_bits(ticks: u64) -> Vec<u32> {
    let (mut rig, zones) = backbox_rig();
    let mut prev = [Vec3::ZERO; 3];
    for (i, (_, b)) in rig.solver.balls().enumerate() {
        prev[i] = b.position;
    }
    for t in 0..ticks {
        tick(&mut rig, &zones, &mut prev, t);
    }
    let mut out = Vec::new();
    for (h, b) in rig.solver.balls() {
        out.push(u32::from(h.0));
        out.push(u32::from(b.layer));
        for x in b
            .position
            .to_array()
            .into_iter()
            .chain(b.velocity.to_array())
        {
            out.push(x.to_bits());
        }
    }
    out.push(rig.flippers.angle_deg(BB_FLIPPER).to_bits());
    out
}

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let x = black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(x);
}

#[test]
fn stepping_the_backbox_scene_does_not_allocate() {
    let (mut rig, zones) = backbox_rig();
    let mut prev = [Vec3::ZERO; 3];
    for (i, (_, b)) in rig.solver.balls().enumerate() {
        prev[i] = b.position;
    }
    // Warm-up outside the guard (first-use lazy state, if any).
    tick(&mut rig, &zones, &mut prev, 0);
    let before = allocs();
    for t in 1..3000u64 {
        tick(&mut rig, &zones, &mut prev, t);
    }
    let after = allocs();
    black_box(&rig.solver);
    assert_eq!(after - before, 0, "allocations in the backbox step loop");
    for (_, b) in rig.solver.balls() {
        assert!(b.position.is_finite() && b.velocity.is_finite());
    }
}

#[test]
fn backbox_replay_is_bit_identical_over_10_runs() {
    let first = run_bits(3000);
    for _ in 0..9 {
        assert_eq!(run_bits(3000), first);
    }
}
