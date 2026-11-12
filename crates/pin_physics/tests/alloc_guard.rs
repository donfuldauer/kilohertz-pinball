//! Gate step 6: per-tick world queries never allocate. A counting global
//! allocator (per thread) observes the hot loops, as in
//! `pin_core/tests/triple_buffer_alloc.rs`.

use arrayvec::ArrayVec;
use glam::{Quat, Vec3};
use pin_physics::world::{
    CubicSpline, GuideCurve, LayerTransition, SphereContact, StaticShape, StaticWorldBuilder,
    TransitionSet, Wireform, layers,
};
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

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
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
    // --- Setup (allocates). ---
    let pts: Vec<Vec3> = (0..40)
        .map(|i| {
            let a = i as f32 * 0.2;
            v(0.15 * libm::cosf(a), 0.15 * libm::sinf(a), 0.004 * i as f32)
        })
        .collect();
    let spline = CubicSpline::interpolate(&pts).unwrap();
    let rolls: Vec<f32> = (0..40).map(|i| i as f32).collect();
    let wireform = Wireform::new(spline.clone(), &rolls, 0.02, Vec3::Z).unwrap();
    let guide = GuideCurve::new(spline.clone(), 0.03).unwrap();

    let mut builder = StaticWorldBuilder::new();
    builder.add_collider(
        StaticShape::Cuboid {
            center: v(0.0, 0.0, -0.01),
            half_extents: v(0.5, 1.0, 0.01),
            rotation: Quat::IDENTITY,
        },
        layers::MAIN_DECK,
        true,
    );
    let mut gate = None;
    for i in 0..200 {
        let x = -0.45 + 0.09 * (i % 10) as f32;
        let y = -0.9 + 0.09 * (i / 10) as f32;
        let h = builder.add_collider(
            StaticShape::Cylinder {
                base: v(x, y, 0.0),
                axis: v(0.0, 0.0, 0.03),
                radius: 0.004,
            },
            layers::MAIN_DECK | layers::UPPER_DECK,
            i % 7 != 0,
        );
        gate = Some(h);
    }
    // A toy mesh: a 64 × 64 height field (8192 triangles).
    let n = 64usize;
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let (x, y) = (i as f32 / n as f32 - 0.5, j as f32 / n as f32 - 0.5);
            vertices.push(v(
                x * 0.3,
                y * 0.3 + 0.5,
                0.05 + 0.02 * libm::sinf(9.0 * x) * libm::cosf(7.0 * y),
            ));
        }
    }
    for j in 0..n {
        for i in 0..n {
            let id = |i: usize, j: usize| (j * (n + 1) + i) as u32;
            indices.push([id(i, j), id(i + 1, j), id(i + 1, j + 1)]);
            indices.push([id(i, j), id(i + 1, j + 1), id(i, j + 1)]);
        }
    }
    builder.add_collider(
        StaticShape::TriMesh { vertices, indices },
        layers::MAIN_DECK,
        true,
    );
    let mut world = builder.build();
    let gate = gate.unwrap();

    let transitions = TransitionSet::new(
        (0..16)
            .map(|i| LayerTransition {
                from_layer: i % 3,
                to_layer: (i + 1) % 3,
                min: v(-0.5 + 0.06 * f32::from(i), -1.0, 0.0),
                max: v(-0.47 + 0.06 * f32::from(i), 1.0, 0.1),
            })
            .collect(),
    );
    let mut contacts: ArrayVec<SphereContact, 16> = ArrayVec::new();

    // --- Hot loop. ---
    let before = allocs();
    let mut layer = 0u8;
    let mut seed = None;
    let mut checksum = 0.0f32;
    let mut prev = v(-0.5, -1.0, 0.0135);
    for tick in 0..20_000u32 {
        let a = tick as f32 * 0.013;
        let pos = v(
            0.48 * libm::sinf(a),
            0.95 * libm::sinf(0.37 * a),
            0.0135 + 0.05 * (0.5 + 0.5 * libm::sinf(1.3 * a)),
        );
        let vel = (pos - prev) * 1000.0 + v(0.0, 0.0, -3.0);

        let cp = spline.closest_point(pos, seed);
        seed = Some(cp.t);
        checksum += cp.distance;
        checksum += spline.closest_point(pos, None).distance;
        checksum +=
            spline.point(cp.t).x + spline.tangent(cp.t).y + spline.second_derivative(cp.t).z;

        let frame = wireform.frame_at(cp.t);
        let (left, right) = wireform.rail_points(cp.t);
        checksum += frame.normal.z + left.x + right.y + wireform.roll_rad(cp.t);
        let rails = wireform.closest_on_rails(pos, seed);
        checksum += rails[0].distance + rails[1].distance;

        checksum += guide.closest_point(pos, seed).distance;
        if let Some(hit) = guide.cast_sphere(pos, 0.0135, vel, 0.001, seed) {
            checksum += hit.time;
        }

        let mask = layers::layer_bit(layer) | layers::MAIN_DECK;
        if let Some(hit) = world.cast_sphere(pos, 0.0135, vel, 0.001, mask) {
            checksum += hit.time + hit.normal.z;
        }
        // A long cast that crosses the whole table and the mesh.
        if let Some(hit) =
            world.cast_sphere(pos + Vec3::Z * 0.2, 0.0135, v(1.0, 2.0, -0.5), 1.0, 0xFF)
        {
            checksum += hit.time;
        }
        world.sphere_contacts(pos, 0.0135, mask, &mut contacts);
        checksum += contacts.len() as f32;
        world.sphere_contacts(pos, 0.2, 0xFF, &mut contacts);
        checksum += contacts.len() as f32;

        if tick % 64 == 0 {
            let on = world.is_enabled(gate);
            world.set_enabled(gate, !on);
        }
        layer = transitions.update(layer, prev, pos);
        prev = pos;
    }
    let used = allocs() - before;
    assert!(black_box(checksum).is_finite());
    assert_eq!(used, 0, "per-tick world queries allocated");
}
