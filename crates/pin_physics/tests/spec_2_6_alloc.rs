//! Subphase 2.6 spec tests (Independent Tester): gate step 6 for banana
//! bats on the sim thread: the per-tick sequence (`set_energised`,
//! `set_zipper`, `on_switch_edge`, `step_tick`,
//! `BallSolver::step_with_bodies`) with CurvedBanana flippers never
//! allocates (counting global allocator, per thread), and a busy banana
//! scenario ends in bit-identical state over 10 runs.

#[path = "spec_2_6_common.rs"]
mod common;

use common::*;
use glam::{Quat, Vec3};
use pin_core::ids::{FlipperId, SwitchId};
use pin_core::{FlipperShape, FlipperStagingMode};
use pin_physics::ball::{BallEvent, BallHandle, ImpactTarget};
use pin_physics::world::{StaticShape, StaticWorldBuilder, layers};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

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

const THING_TRIGGER: SwitchId = SwitchId(500);

/// Two lower banana flippers (mirror images, zipper), an upper banana
/// Thing Flip on a tilted mount with the convex face leading, four balls
/// on the pitched table with side walls.
fn busy() -> Scene {
    let lb = [-90.0, 0.0, 0.0];
    let rb = [90.0, 0.0, 0.0];
    let mut l = banana_config(0, lb, Vec3::Z, 0.0, 50.0, BANANA);
    l.rest_angle_deg = rest_for_dir(&l, flat_dir(-30.0));
    let mut r = banana_config(1, rb, Vec3::Z, 0.0, -50.0, BANANA);
    r.rest_angle_deg = rest_for_dir(&r, flat_dir(210.0));
    l.zipper_slide_axis_mm = Some([12.0, 0.0, 0.0]);
    r.zipper_slide_axis_mm = Some([-12.0, 0.0, 0.0]);
    l.zipper_slide_speed_mm_s = 200.0;
    r.zipper_slide_speed_mm_s = 200.0;
    let n = v(0.0, -0.05, 1.0).normalize();
    let ub = [-150.0, 250.0, 3.0];
    let mut u = banana_config(2, ub, n, 0.0, 45.0, BANANA);
    u.shape = FlipperShape::CurvedBanana {
        length_mm: 60.0,
        arc_radius_mm: 70.0,
        arc_sweep_deg: -50.0,
        tube_radius_mm: 7.0,
    };
    u.staging = FlipperStagingMode::AutoSensorThingFlip {
        trigger_switch: THING_TRIGGER,
        delay_ms: 12.0,
        hold_duration_ms: 40.0,
    };
    let mut b = StaticWorldBuilder::new();
    let cuboid = |c: Vec3, h: Vec3| StaticShape::Cuboid {
        center: c,
        half_extents: h,
        rotation: Quat::IDENTITY,
    };
    b.add_collider(
        cuboid(v(0.0, 0.0, -0.05), v(0.6, 0.8, 0.05)),
        layers::ALL,
        true,
    );
    for (c, h) in [
        (v(0.31, 0.2, 0.05), v(0.01, 0.6, 0.05)),
        (v(-0.31, 0.2, 0.05), v(0.01, 0.6, 0.05)),
        (v(0.0, 0.71, 0.05), v(0.32, 0.01, 0.05)),
    ] {
        b.add_collider(cuboid(c, h), layers::ALL, true);
    }
    let mut s = Scene::new(b.build(), table_gravity(Quat::IDENTITY), &[l, r, u]);
    for (p, vel) in [
        (v(-0.03, 0.2, R), v(-0.4, -1.5, 0.0)),
        (v(0.05, 0.3, R), v(0.2, -2.5, 0.0)),
        (v(0.0, 0.45, R), v(0.0, -0.5, 0.0)),
        (v(-0.15, 0.35, R), v(0.1, -0.8, 0.0)),
    ] {
        s.solver.add_ball(ball(p, vel)).unwrap();
    }
    s
}

fn drive(s: &mut Scene) {
    let t = s.tick;
    for k in 0..4u8 {
        let h = BallHandle(k);
        if let Some(b) = s.solver.ball_mut(h)
            && b.position.y < -0.12
        {
            let x = -0.12 + 0.08 * f32::from(k);
            *b = ball(v(x, 0.5, R), v(0.3 - 0.2 * f32::from(k), -1.0, 0.0));
        }
    }
    s.set.set_energised(FlipperId(0), (t / 170) % 2 == 1);
    s.set.set_energised(FlipperId(1), (t / 230) % 3 == 1);
    if t % 500 == 100 {
        s.set.set_zipper(FlipperId(0), true);
        s.set.set_zipper(FlipperId(1), true);
    }
    if t % 500 == 400 {
        s.set.set_zipper(FlipperId(0), false);
        s.set.set_zipper(FlipperId(1), false);
    }
    if t % 300 == 50 {
        s.set.on_switch_edge(THING_TRIGGER, true, t);
    }
    if t % 300 == 52 {
        s.set.on_switch_edge(THING_TRIGGER, false, t);
    }
}

fn state_hash(s: &Scene) -> u64 {
    let mut h = DefaultHasher::new();
    for (hd, b) in s.solver.balls() {
        hd.0.hash(&mut h);
        for x in b
            .position
            .to_array()
            .into_iter()
            .chain(b.velocity.to_array())
            .chain(b.angular_velocity.to_array())
        {
            x.to_bits().hash(&mut h);
        }
        b.layer.hash(&mut h);
    }
    for i in 0..3u16 {
        let id = FlipperId(i);
        s.set.angle_deg(id).to_bits().hash(&mut h);
        s.set.angular_velocity(id).to_bits().hash(&mut h);
        s.set.eos_closed(id).hash(&mut h);
        s.set.base_offset_mm(id).to_bits().hash(&mut h);
        s.set.drive_work_j(id).to_bits().hash(&mut h);
    }
    h.finish()
}

fn bat_impacts(s: &Scene) -> u32 {
    s.ev.as_slice()
        .iter()
        .filter(|e| {
            matches!(
                e,
                BallEvent::Impact {
                    other: ImpactTarget::Body { set: 0, .. },
                    ..
                }
            )
        })
        .count() as u32
}

#[test]
fn banana_tick_never_allocates() {
    let mut s = busy();
    for _ in 0..20 {
        drive(&mut s);
        s.advance();
    }
    let before = allocs();
    let mut hits = 0;
    for _ in 0..3000 {
        drive(&mut s);
        s.advance();
        hits += bat_impacts(&s);
    }
    let n = allocs() - before;
    println!("bat impacts in the guarded window: {hits}");
    assert_eq!(n, 0, "{n} heap allocations in 3000 banana ticks");
    assert!(
        hits >= 5,
        "the guarded window barely touches the bats: {hits}"
    );
    let probe = std::hint::black_box(Box::new(7u64));
    assert!(allocs() > before, "allocation counter inactive");
    drop(probe);
}

#[test]
fn banana_scenario_is_bit_identical_over_ten_runs() {
    let run = || {
        let mut s = busy();
        let mut trail = DefaultHasher::new();
        let mut hits = 0u32;
        for _ in 0..4000 {
            drive(&mut s);
            s.advance();
            state_hash(&s).hash(&mut trail);
            hits += bat_impacts(&s);
        }
        (trail.finish(), state_hash(&s), hits)
    };
    let first = run();
    println!("bat impacts per run: {}", first.2);
    assert!(
        first.2 >= 5,
        "the scenario barely touches the bats: {}",
        first.2
    );
    for k in 1..10 {
        assert_eq!(run(), first, "run {k} differs");
    }
}
