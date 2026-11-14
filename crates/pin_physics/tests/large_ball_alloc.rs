//! Gate step 6 for Subphase 2.8: a table with the XL, mini and standard
//! balls, a flipper and thin walls steps without allocating (counting
//! global allocator, per thread), and ends bit-identical over 10 runs.

#[path = "flipper_util.rs"]
mod flip;

use glam::Vec3;
use pin_core::ids::ProfileId;
use pin_physics::ball::Ball;
use pin_physics::world::{StaticShape, StaticWorldBuilder, layers};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

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

/// Pitched floor, side walls, a lower flipper flipping every 300 ms, and
/// the XL, mini and standard balls dropped in.
fn rig() -> flip::Rig {
    let mut b = StaticWorldBuilder::new();
    let cuboid = |c: Vec3, h: Vec3| StaticShape::Cuboid {
        center: c,
        half_extents: h,
        rotation: glam::Quat::IDENTITY,
    };
    b.add_collider(
        cuboid(flip::v(0.0, 0.3, -0.05), flip::v(0.4, 0.6, 0.05)),
        layers::ALL,
        true,
    );
    for x in [-0.3f32, 0.3] {
        b.add_collider(
            StaticShape::TriMesh {
                vertices: vec![
                    flip::v(x, -0.3, 0.0),
                    flip::v(x, 0.9, 0.0),
                    flip::v(x, 0.9, 0.1),
                    flip::v(x, -0.3, 0.1),
                ],
                indices: vec![[0, 1, 2], [0, 2, 3]],
            },
            layers::ALL,
            true,
        );
    }
    let mut rig = flip::Rig::with_world(
        &[flip::left_config(0, [-0.05, 0.0, 0.0])],
        b.build(),
        flip::pitched_gravity(6.5),
    );
    for (i, (p, x)) in [(6u16, -0.1f32), (4, 0.05), (0, 0.15)]
        .into_iter()
        .enumerate()
    {
        let mut ball = Ball::new(ProfileId(p), flip::v(x, 0.25 + 0.1 * i as f32, 0.05));
        ball.velocity = flip::v(0.3 * x, -0.5, 0.0);
        rig.solver.add_ball(ball).unwrap();
    }
    rig
}

fn drive(rig: &mut flip::Rig, ticks: u32) {
    for t in 0..ticks {
        rig.flippers.set_energised(flip::LEFT, t % 300 < 120);
        rig.step();
    }
}

#[test]
fn large_ball_steps_do_not_allocate() {
    let mut rig = rig();
    drive(&mut rig, 50); // warm up
    let before = allocs();
    for t in 0..2000u32 {
        rig.flippers.set_energised(flip::LEFT, t % 300 < 120);
        rig.flippers.step_tick(rig.tick);
        rig.events.clear();
        let geo = pin_physics::ball::Geometry::new(&rig.world);
        rig.solver
            .step_with_bodies(&geo, &mut [&mut rig.flippers], flip::DT, &mut rig.events);
        rig.tick += 1;
    }
    assert_eq!(allocs() - before, 0);
    assert!(rig.solver.balls().all(|(_, b)| b.position.is_finite()));
}

#[test]
fn large_ball_replay_is_bit_stable_over_10_runs() {
    let reference = {
        let mut r = rig();
        drive(&mut r, 3000);
        r.state_bits()
    };
    for _ in 0..10 {
        let mut r = rig();
        drive(&mut r, 3000);
        assert_eq!(r.state_bits(), reference);
    }
}
