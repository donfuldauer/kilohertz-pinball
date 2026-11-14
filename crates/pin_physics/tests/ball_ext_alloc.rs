//! Subphase 2.7 gate step 6: with the Magnus and remanence hooks
//! registered, `BallSolver::step` (and reading the magnetisation) never
//! allocates (counting global allocator, per thread).

#[path = "ball_util.rs"]
mod util;

use pin_core::ids::ProfileId;
use pin_physics::ball::{BallEvents, BallHandle, Geometry, Magnet};
use pin_physics::ball_ext::{magnetisation, register_all};
use pin_physics::world::layers;
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

/// `low_rem_steel` (index 1 of the built-in profiles).
const LOW_REM: ProfileId = ProfileId(1);

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let x = black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(x);
}

#[test]
fn step_with_ball_ext_hooks_does_not_allocate() {
    let world = box_world(v(0.2, 0.15, 0.1));
    let geo = Geometry::new(&world);
    let mut s = solver();
    let floor = -0.1;
    let mut ids = Vec::new();
    for x in [-0.1f32, 0.0, 0.1] {
        ids.push(s.add_magnet(Magnet {
            position: v(x, 0.0, floor - 0.005),
            k_coil: 2.0e-5,
            d_min: 0.01,
            range: 0.08,
            layer_mask: layers::ALL,
        }));
    }
    let rem = register_all(&mut s);
    for (k, p) in [STEEL, GLOBALL, MINI, LOW_REM, JUMBO, GLOBALL]
        .into_iter()
        .enumerate()
    {
        let r = radius(p);
        let mut b = ball_at(
            p,
            v(-0.15 + 0.06 * k as f32, 0.02 * k as f32, floor + r + 0.01),
            v(0.6 - 0.2 * k as f32, 0.3, 0.0),
        );
        b.angular_velocity = v(10.0, -30.0, 120.0 - 40.0 * k as f32);
        s.add_ball(b).unwrap();
    }
    let mut ev = BallEvents::new();
    let mut sum = 0.0f32;
    let before = allocs();
    for tick in 0..3000u32 {
        for (i, &id) in ids.iter().enumerate() {
            let on = (tick / 250 + i as u32).is_multiple_of(3);
            s.set_magnet_power(id, if on { 1.0 } else { 0.0 });
        }
        if tick == 1500 {
            // Lifecycle calls inside the guarded window too.
            let b = s.remove_ball(BallHandle(1)).unwrap();
            s.add_ball(b).unwrap();
        }
        if tick.is_multiple_of(400) {
            s.apply_impulse(BallHandle(0), v(0.02, -0.01, 0.0));
        }
        ev.clear();
        s.step(&geo, DT, &mut ev);
        for h in 0..6u8 {
            sum += magnetisation(&rem, BallHandle(h));
        }
    }
    assert_eq!(allocs(), before, "the step allocated");
    assert!(black_box(sum) > 0.0);
    assert!(s.balls().all(|(_, b)| b.position.is_finite()));
}
