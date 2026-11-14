//! Independent spec test, Subphase 2.7, rule 4 / gate step 6: with the Magnus
//! and remanence hooks registered, nothing called from the 1 kHz tick
//! allocates: the step, the ball add/remove notifications (a hook "may not
//! allocate once registered") and reading the magnetisation.

#[path = "spec_2_7_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_physics::ball::{Ball, BallEvents, BallHandle, Geometry};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

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

#[test]
fn tick_with_hooks_does_not_allocate() {
    let stock = builtin();
    let mut sc = busy(&stock, Hooks::Before);
    let probe = sc.probe.take().unwrap();
    let geo = Geometry {
        world: &sc.world,
        guides: &sc.guides,
        wireforms: &sc.wireforms,
    };
    let mut ev = BallEvents::new();
    let mut impacts = 0usize;
    let mut m_max = 0.0f32;
    let mut m_sum = 0.0f32;
    let jumbo = pid("jumbo_steel");
    let before = allocs();
    for tick in 0..4000u32 {
        sc.solver.set_magnet_power(sc.magnet, busy_power(tick));
        if tick % 900 == 450 {
            // Ball lifetime notifications, from the tick.
            let h = BallHandle(((tick / 900) % 4) as u8);
            if let Some(old) = sc.solver.remove_ball(h) {
                let mut fresh = Ball::new(jumbo, old.position + Vec3::new(0.0, 0.0, 0.01));
                fresh.velocity = Vec3::new(0.3, 0.5, 0.0);
                fresh.angular_velocity = Vec3::new(0.0, 0.0, 40.0);
                let _ = sc.solver.add_ball(fresh);
            }
        }
        ev.clear();
        sc.solver.step(&geo, DT, &mut ev);
        impacts += ev.as_slice().len();
        for i in 0..4u8 {
            let m = mag(&probe, BallHandle(i));
            m_max = m_max.max(m);
            m_sum += m;
        }
    }
    let after = allocs();
    assert_eq!(after - before, 0, "the tick allocated with the 2.7 hooks");
    // The scenario exercised the solver and the remanence state.
    assert!(impacts > 20, "{impacts}");
    assert!(m_max > 0.3, "no ball was magnetised (max M {m_max})");
    assert!(m_sum.is_finite());
    for (_, b) in sc.solver.balls() {
        assert!(b.position.is_finite() && b.velocity.is_finite());
    }
}

#[test]
fn free_flight_and_holding_magnet_ticks_do_not_allocate() {
    // Spinning Glo-Ball in free flight (Magnus) and a steel ball released by
    // a holding magnet (remanence), each with both hooks.
    let stock = builtin();
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut s = solver(&stock, G);
    let id = s.add_magnet(magnet_at(Vec3::new(0.0, 0.0, -0.005)));
    let probe = register(&mut s);
    let steel = s
        .add_ball(Ball::new(
            pid("standard_steel"),
            Vec3::new(0.0, 0.0, 0.0135),
        ))
        .unwrap();
    let mut glo = Ball::new(pid("uv_globall"), Vec3::new(1.0, 0.0, 0.5));
    glo.velocity = Vec3::new(2.0, 0.0, 3.0);
    glo.angular_velocity = Vec3::new(0.0, 0.0, 200.0);
    let glo = s.add_ball(glo).unwrap();
    let mut ev = BallEvents::new();
    let mut m_peak = 0.0f32;
    let before = allocs();
    for tick in 0..1500u32 {
        s.set_magnet_power(id, if tick < 700 { 1.0 } else { 0.0 });
        if tick == 700 {
            s.apply_impulse(steel, Vec3::new(0.02, 0.0, 0.0));
        }
        ev.clear();
        s.step(&geo, DT, &mut ev);
        m_peak = m_peak.max(mag(&probe, steel));
    }
    let after = allocs();
    assert_eq!(after - before, 0, "the tick allocated with the 2.7 hooks");
    assert!(m_peak > 0.5, "steel ball was not magnetised ({m_peak})");
    assert!(s.ball(glo).unwrap().velocity.y.abs() > 1.0e-3, "no Magnus");
}
