//! Independent spec test (Subphase 2.1, gate step 6): per-tick world queries
//! never allocate — closest point (seeded and unseeded), derivatives, wireform
//! frames and rails, sphere casts, sphere contacts and layer transitions.
//! A counting global allocator (per thread) observes the hot loop.

use arrayvec::ArrayVec;
use glam::{Quat, Vec3};
use pin_physics::world::layers::{MAIN_DECK, UPPER_DECK};
use pin_physics::world::{
    CubicSpline, LayerTransition, SphereContact, StaticShape, StaticWorldBuilder, TransitionSet,
    Wireform,
};
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
fn per_tick_queries_do_not_allocate() {
    // ---- setup (allocation allowed)
    let pts: Vec<Vec3> = (0..9)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / 8.0;
            Vec3::new(0.15 * a.cos(), 0.15 * a.sin(), 0.008 * i as f32)
        })
        .collect();
    let spline = CubicSpline::interpolate(&pts).unwrap();
    let wire = Wireform::new(
        CubicSpline::interpolate(&pts).unwrap(),
        &[0.0, 5.0, 10.0, 15.0, 20.0, 15.0, 10.0, 5.0, 0.0],
        0.032,
        Vec3::Z,
    )
    .unwrap();

    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cuboid {
            center: Vec3::new(0.0, 0.0, -0.05),
            half_extents: Vec3::new(1.0, 1.0, 0.05),
            rotation: Quat::IDENTITY,
        },
        MAIN_DECK,
        true,
    );
    let post = b.add_collider(
        StaticShape::Cylinder {
            base: Vec3::new(0.3, 0.0, 0.0),
            axis: Vec3::Z * 0.05,
            radius: 0.01,
        },
        MAIN_DECK,
        true,
    );
    b.add_collider(
        StaticShape::TriMesh {
            vertices: vec![
                Vec3::new(0.5, -0.2, 0.0),
                Vec3::new(0.9, -0.2, 0.065),
                Vec3::new(0.9, 0.2, 0.065),
                Vec3::new(0.5, 0.2, 0.0),
            ],
            indices: vec![[0, 1, 2], [0, 2, 3]],
        },
        UPPER_DECK,
        true,
    );
    let mut world = b.build();
    let transitions = TransitionSet::new(vec![LayerTransition {
        from_layer: MAIN_DECK,
        to_layer: UPPER_DECK,
        min: Vec3::new(0.45, -0.2, 0.0),
        max: Vec3::new(0.5, 0.2, 0.1),
    }]);
    let mut contacts: ArrayVec<SphereContact, 16> = ArrayVec::new();

    // Warm up once (lazy statics etc. in dependencies).
    let _ = spline.closest_point(Vec3::new(0.1, 0.1, 0.0), None);
    let _ = world.cast_sphere(Vec3::new(0.0, 0.0, 0.02), 0.0135, Vec3::X, 1e-3, MAIN_DECK);
    world.sphere_contacts(Vec3::new(0.0, 0.0, 0.01), 0.0135, MAIN_DECK, &mut contacts);

    // ---- hot loop: 2000 ticks, no allocation allowed
    let before = allocs();
    let mut seed = None;
    let mut layer = MAIN_DECK;
    let mut prev = Vec3::new(0.0, 0.0, 0.0135);
    for tick in 0..2000u32 {
        let s = tick as f32 / 2000.0;
        let a = std::f32::consts::PI * s;
        let q = Vec3::new(0.14 * a.cos(), 0.14 * a.sin(), 0.03);
        let cp = spline.closest_point(black_box(q), seed);
        seed = Some(cp.t);
        black_box(spline.closest_point(q, None));
        black_box((
            spline.point(cp.t),
            spline.tangent(cp.t),
            spline.second_derivative(cp.t),
        ));
        black_box(wire.frame_at(cp.t));
        black_box(wire.rail_points(cp.t));

        let pos = Vec3::new(s, 0.0, 0.0135);
        black_box(world.cast_sphere(
            prev,
            0.0135,
            Vec3::new(1.0, 0.1, -0.2),
            1e-3,
            MAIN_DECK | UPPER_DECK,
        ));
        contacts.clear();
        world.sphere_contacts(
            pos - Vec3::Z * 0.001,
            0.0135,
            MAIN_DECK | UPPER_DECK,
            &mut contacts,
        );
        black_box(&contacts);
        if tick == 1000 {
            world.set_enabled(post, false);
        }
        black_box(world.is_enabled(post));
        layer = transitions.update(layer, prev, pos);
        prev = pos;
    }
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "per-tick world queries allocated {} times",
        after - before
    );
    assert_eq!(layer, UPPER_DECK);
}
