//! Subphase 5.5 spec tests (Independent Tester): gate step 6 for the
//! backbox mini-playfield on the sim thread. The per-tick sequence of a
//! vertical (layer 4) playfield (staging → `set_energised`, `step_tick`,
//! `BallSolver::step_with_bodies` with the flippers, `TransitionSet::update`
//! for every ball) never allocates (counting global allocator, per thread).

#[path = "spec_5_5_common.rs"]
mod bb;

use bb::c23::{DT, R};
use bb::{BACKBOX_LAYER, Frame, Mount, Stage, route};
use glam::{Quat, Vec3};
use pin_core::ids::FlipperId;
use pin_core::{CabinetInput, FlipperStagingMode};
use pin_physics::ball::{BallEvents, BallHandle, Geometry};
use pin_physics::flipper::FlipperSet;
use pin_physics::world::{LayerTransition, StaticShape, StaticWorldBuilder, TransitionSet, layers};
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

#[test]
fn backbox_tick_does_not_allocate() {
    // --- Setup (allocates). ---
    let mount = Mount::Vertical;
    let rot = mount.rot();
    let mut l = bb::left_flipper_at(0, mount, 1.0, Vec3::new(-0.06, 0.0, 0.0));
    let mut r = bb::right_flipper_at(1, mount, Vec3::new(0.06, 0.0, 0.0));
    l.staging = FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper);
    r.staging = bb::dual_stage();
    let cfgs = [l, r];
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cuboid {
            center: rot * Vec3::new(0.0, 0.0, -0.05),
            half_extents: Vec3::new(0.3, 0.4, 0.05),
            rotation: rot,
        },
        layers::BACKBOX,
        true,
    );
    b.add_collider(
        StaticShape::Cuboid {
            center: Vec3::new(0.0, -0.2, -0.05),
            half_extents: Vec3::new(0.5, 0.2, 0.05),
            rotation: Quat::IDENTITY,
        },
        layers::MAIN_DECK,
        true,
    );
    let world = b.build();
    let zones = TransitionSet::new(vec![
        LayerTransition {
            from_layer: 0,
            to_layer: BACKBOX_LAYER,
            min: Vec3::new(-0.2, -0.05, 0.0),
            max: Vec3::new(0.2, 0.0, 0.05),
        },
        LayerTransition {
            from_layer: BACKBOX_LAYER,
            to_layer: 0,
            min: Vec3::new(-0.2, -0.05, -0.2),
            max: Vec3::new(0.2, 0.05, -0.15),
        },
    ]);
    let mut set = FlipperSet::new(&cfgs).unwrap();
    let mut sol = bb::c23::solver(mount.gravity());
    let mut hs = [BallHandle(0); 3];
    let starts = [
        (rot * Vec3::new(-0.03, 0.05, R), BACKBOX_LAYER),
        (rot * Vec3::new(0.04, 0.12, R), BACKBOX_LAYER),
        (Vec3::new(0.0, -0.1, 0.02), 0),
    ];
    for (i, (p, layer)) in starts.into_iter().enumerate() {
        let mut ball = bb::c23::ball(p, Vec3::new(0.0, 0.3, 0.2));
        ball.layer = layer;
        hs[i] = sol.add_ball(ball).unwrap();
    }
    let mut ev = BallEvents::new();
    let mut prev = [(BallHandle(0), Vec3::ZERO); 3];
    let mut tick = |t: u64, sol: &mut pin_physics::ball::BallSolver, set: &mut FlipperSet| {
        let frame = Frame {
            left: (t / 50) % 2 == 1,
            right: false,
            axis: ((t % 200) as f32) / 200.0,
        };
        for (i, id) in [FlipperId(0), FlipperId(1)].into_iter().enumerate() {
            let stage = if i == 0 { Stage::Lower } else { Stage::Upper };
            set.set_energised(id, route(&cfgs[i], stage, &frame));
        }
        for (k, &h) in hs.iter().enumerate() {
            prev[k] = (h, sol.ball(h).map_or(Vec3::ZERO, |b| b.position));
        }
        set.step_tick(t);
        ev.clear();
        sol.step_with_bodies(&Geometry::new(&world), &mut [&mut *set], DT, &mut ev);
        bb::apply_transitions(sol, &zones, &prev);
    };
    // Warm-up (first-use paths), then measure.
    for t in 0..100 {
        tick(t, &mut sol, &mut set);
    }
    let before = allocs();
    for t in 100..3100 {
        tick(t, &mut sol, &mut set);
    }
    let n = allocs() - before;
    assert_eq!(n, 0, "{n} allocations in 3000 backbox ticks");
    for &h in &hs {
        let b = sol.ball(h).unwrap();
        assert!(b.position.is_finite() && b.velocity.is_finite());
    }
}
