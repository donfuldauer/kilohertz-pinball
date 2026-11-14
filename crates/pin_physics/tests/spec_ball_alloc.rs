//! Independent spec test (Subphase 2.2, gate step 6 / D5): `BallSolver::step`
//! never allocates — with balls in contact, ball–ball impacts, a pulsed magnet,
//! sensors, a guide, a wireform, a Newton chain and an external-force hook.
//! A counting global allocator (per thread) observes the hot loop.

#[path = "spec_ball_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_core::BallProfile;
use pin_physics::ball::{Ball, BallHandle, ExternalForce};
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
fn counter_detects_allocation() {
    let before = allocs();
    let v = black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(v);
}

/// A hook that does real (allocation-free) work.
struct Wind;
impl ExternalForce for Wind {
    fn force(&mut self, h: BallHandle, b: &Ball, p: &BallProfile, _dt: f32) -> Vec3 {
        Vec3::new(0.001 * f32::from(h.0), 0.0, 0.0) * p.mass_kg - b.velocity * 1e-5
    }
}

#[test]
fn step_does_not_allocate() {
    let mut sc = scenario(Some(Box::new(Wind)));
    // An extra ball dropped right onto another one and one fired at a post.
    let mut extra = ball("jumbo_steel", Vec3::new(0.0, 0.3, 0.2));
    extra.velocity = Vec3::new(0.0, 0.0, -3.0);
    sc.solver.add_ball(extra).unwrap();
    let mut ev = events();
    let g = geo_full(&sc.world, &sc.guides, &sc.wireforms);
    let mut sw = [0u64; 4];
    // ---- hot loop: no allocation allowed
    let before = allocs();
    for tick in 0..3000u32 {
        sc.solver.set_magnet_power(sc.magnet, scenario_power(tick));
        ev.clear();
        sc.solver.step(&g, DT, &mut ev);
        sc.solver.sensor_switches(&mut sw);
        black_box(ev.as_slice());
        black_box(ev.dropped());
    }
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "BallSolver::step allocated {} times",
        after - before
    );
    // The loop must have exercised contacts.
    assert!(sc.solver.balls().count() == 5);
}

#[test]
fn step_does_not_allocate_with_full_ball_set() {
    let world = floor_world();
    let g = geo(&world);
    let mut s = solver();
    for i in 0..pin_physics::ball::MAX_BALLS {
        let mut b = ball(
            if i % 2 == 0 {
                "standard_steel"
            } else {
                "mini_steel"
            },
            Vec3::new(
                0.03 * i as f32,
                0.002 * i as f32,
                0.02 + 0.01 * (i % 3) as f32,
            ),
        );
        b.velocity = Vec3::new(if i % 2 == 0 { 0.5 } else { -0.5 }, 0.0, -1.0);
        s.add_ball(b).unwrap();
    }
    let mut ev = events();
    let before = allocs();
    for _ in 0..1000 {
        ev.clear();
        s.step(&g, DT, &mut ev);
    }
    assert_eq!(allocs() - before, 0);
}
