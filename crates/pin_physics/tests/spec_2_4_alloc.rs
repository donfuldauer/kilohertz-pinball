//! Subphase 2.4 spec tests, gate step 6: nothing called from the 1 kHz tick
//! allocates (counting global allocator, per thread).

#[path = "spec_2_4_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_physics::ball::Geometry;
use pin_physics::mech::{TiltBob, TiltBobParams};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn bump() {
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

#[test]
fn counter_sees_allocations() {
    let before = allocs();
    black_box(vec![1u8; 64]);
    assert!(allocs() > before);
}

#[test]
fn busy_tick_never_allocates() {
    let mut rig = busy_rig();
    let mut edges = 0usize;
    let mut impacts = 0usize;
    let before = allocs();
    for t in 0..6000 {
        busy_drive(&mut rig, t);
        rig.step();
        edges += rig.mechs.edges().len();
        impacts += rig.events.as_slice().len();
        let mut sw = [0u64; 4];
        rig.mechs.switch_states(&mut sw);
        black_box((
            sw,
            rig.mechs.tilt_bob_offset(),
            rig.mechs.tilt_closed(),
            rig.mechs.spinner_pulses(ix::SPINNER),
            rig.mechs.transitions().zones().len(),
        ));
    }
    let after = allocs();
    assert_eq!(after - before, 0, "allocations in 6000 busy ticks");
    // The scenario exercised the mechs.
    assert!(edges > 10, "only {edges} switch edges");
    assert!(impacts > 10, "only {impacts} ball events");
    assert!(rig.mechs.spinner_pulses(ix::SPINNER) > 0);
}

#[test]
fn hook_sequence_and_inputs_never_allocate() {
    let mut rig = busy_rig();
    rig.mechs.set_body_set(0);
    let before = allocs();
    for t in 0..3000u32 {
        busy_drive(&mut rig, t);
        // Unknown coils, extreme inputs.
        rig.mechs.fire_coil(pin_core::ids::CoilId(999), 10);
        rig.mechs.hold_coil(pin_core::ids::CoilId(998), t % 2 == 0);
        if t % 97 == 0 {
            rig.mechs.nudge(Vec3::new(f32::NAN, 1.0, 0.0));
            rig.mechs.set_plunger(f32::NAN);
        }
        rig.events.clear();
        let geo = Geometry::new(&rig.world);
        rig.mechs.pre_step(&mut rig.solver, DT);
        rig.solver
            .step_with_bodies(&geo, &mut [&mut rig.mechs], DT, &mut rig.events);
        rig.mechs
            .post_step(&mut rig.solver, rig.events.as_slice(), DT);
    }
    assert_eq!(allocs() - before, 0, "allocations in the hook sequence");
}

#[test]
fn standalone_tilt_bob_never_allocates() {
    let mut bob = TiltBob::new(TiltBobParams::from_defaults(&defaults()));
    let before = allocs();
    for t in 0..5000 {
        if t % 500 == 0 {
            bob.nudge(Vec3::new(0.3, -0.2, 0.0));
        }
        bob.step(DT);
        black_box((bob.offset(), bob.closed(), bob.velocity()));
    }
    assert_eq!(allocs() - before, 0);
}
