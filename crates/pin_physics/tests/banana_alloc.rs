//! Gate step 6 for the banana flipper: a tick with curved bats (inputs,
//! Thing Flip schedule, ball step with the flippers as moving bodies) never
//! allocates, and a busy scenario ends bit-identical over 10 runs.

#[path = "banana_util.rs"]
mod butil;

use butil::*;
use glam::Vec3;
use pin_core::ids::{FlipperId, SwitchId};
use pin_core::{FlipperShape, FlipperStagingMode};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

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

const THING: FlipperId = FlipperId(2);
const BACK: FlipperId = FlipperId(3);

/// A banana on each side (the Feature Lab pairs one with a straight bat;
/// here both hands are curved, one of them a zipper), a Thing Flip banana
/// on a tilted plane with its convex face in front, a straight bat, and
/// three balls raining onto them.
fn busy_rig() -> Rig {
    let mut left = banana_config(0, [-96.3, 0.0, 0.0], RADIUS_MM);
    left.zipper_slide_axis_mm = Some([12.0, 0.0, 0.0]);
    left.zipper_slide_speed_mm_s = 240.0;
    let right = banana_right_config(1, [96.3, 0.0, 0.0], 70.0);
    let mut thing = banana_config(2, [-60.0, 250.0, 0.0], 150.0);
    thing.shape = FlipperShape::CurvedBanana {
        length_mm: 60.0,
        arc_radius_mm: 150.0,
        arc_sweep_deg: -20.0,
        tube_radius_mm: 6.0,
    };
    thing.staging = FlipperStagingMode::AutoSensorThingFlip {
        trigger_switch: SwitchId(9),
        delay_ms: 7.0,
        hold_duration_ms: 60.0,
    };
    let straight = left_config(3, [60.0, 330.0, 0.0]);
    let mut rig = Rig::new(&[left, right, thing, straight]);
    rig.add_ball(v(-0.06, 0.12, BALL_R), v(0.1, -1.0, 0.0));
    rig.add_ball(v(0.05, 0.2, BALL_R), v(-0.2, -2.5, 0.0));
    rig.add_ball(v(-0.02, 0.33, BALL_R), v(0.0, -0.5, 0.0));
    rig
}

/// The inputs of tick `t`.
fn drive(rig: &mut Rig, t: u64) {
    rig.flippers.set_energised(LEFT, (t / 90) % 2 == 1);
    rig.flippers.set_energised(RIGHT, (t / 70) % 3 == 1);
    rig.flippers.set_energised(BACK, (t / 130) % 2 == 0);
    rig.flippers.set_zipper(LEFT, (t / 400) % 2 == 1);
    if t % 250 == 40 {
        rig.flippers.on_switch_edge(SwitchId(9), true, t);
    }
}

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let v = black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(v);
}

#[test]
fn a_tick_with_banana_flippers_never_allocates() {
    let mut rig = busy_rig();
    // Warm up outside the guard (the rig's own hit list is test code).
    for t in 0..10 {
        drive(&mut rig, t);
        rig.step();
    }
    let mut events = pin_physics::ball::BallEvents::new();
    let mut banana_impacts = 0u32;
    let mut thing_moved = false;
    let before = allocs();
    for t in 10..3000u64 {
        drive(&mut rig, t);
        rig.flippers.step_tick(t);
        events.clear();
        let geo = pin_physics::ball::Geometry::new(&rig.world);
        rig.solver
            .step_with_bodies(&geo, &mut [&mut rig.flippers], DT, &mut events);
        for e in events.as_slice() {
            if let pin_physics::ball::BallEvent::Impact {
                other: pin_physics::ball::ImpactTarget::Body { set: 0, body },
                ..
            } = *e
                && body < 3
            {
                banana_impacts += 1;
            }
        }
        thing_moved |= rig.flippers.angle_deg(THING) > -29.0;
        black_box(rig.flippers.eos_edges());
    }
    let after = allocs();
    assert_eq!(after - before, 0, "the tick allocated");
    assert!(
        banana_impacts > 10,
        "the scenario exercises ball–banana contact: {banana_impacts}"
    );
    assert!(thing_moved, "the Thing Flip fired");
}

#[test]
fn ten_runs_are_bit_identical() {
    let run = || {
        let mut rig = busy_rig();
        let mut digest = Vec::new();
        let mut hits = 0usize;
        for t in 0..3000u64 {
            drive(&mut rig, t);
            rig.step();
            hits += rig.hits.len();
            if t % 500 == 499 {
                digest.extend(rig.state_bits());
            }
        }
        // The scenario is not trivial: balls were moved by the bats.
        assert!(hits > 10, "{hits} impacts");
        assert!(rig.solver.balls().any(|(_, b)| b.velocity != Vec3::ZERO));
        digest
    };
    let first = run();
    for i in 1..10 {
        assert_eq!(run(), first, "run {i} differs");
    }
}
