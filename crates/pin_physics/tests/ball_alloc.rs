//! Gate step 6 for the ball solver: `BallSolver::step` never allocates
//! (counting global allocator, per thread), and a 5 s scenario ends in
//! bit-identical state over 10 runs.

#[path = "ball_util.rs"]
mod util;

use glam::Vec3;
use pin_core::BallProfile;
use pin_core::ids::SwitchId;
use pin_physics::ball::{
    Ball, BallEvents, BallHandle, BallSolver, ExternalForce, Geometry, Magnet, NewtonChain, Sensor,
    SensorKind,
};
use pin_physics::world::{
    CubicSpline, GuideCurve, StaticShape, StaticWorld, StaticWorldBuilder, Wireform, layers,
};
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

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

struct Breeze;
impl ExternalForce for Breeze {
    fn force(&mut self, _h: BallHandle, ball: &Ball, profile: &BallProfile, _dt: f32) -> Vec3 {
        ball.velocity * (-0.01 * profile.mass_kg)
    }
}

struct Table {
    world: StaticWorld,
    guides: Vec<GuideCurve>,
    wireforms: Vec<Wireform>,
}

/// A small table: sloped floor in a walled box, posts, a mesh wedge, a
/// guide arc, a wireform, magnets, sensors, a Newton chain, seven balls.
fn build() -> (Table, BallSolver) {
    let pitch = glam::Quat::from_rotation_x(0.11);
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cuboid {
            center: pitch * v(0.0, 0.0, -0.02),
            half_extents: v(0.3, 0.6, 0.02),
            rotation: pitch,
        },
        layers::ALL,
        true,
    );
    for (c, he) in [
        (v(0.31, 0.0, 0.0), v(0.01, 0.7, 0.3)),
        (v(-0.31, 0.0, 0.0), v(0.01, 0.7, 0.3)),
        (v(0.0, 0.61, 0.0), v(0.4, 0.01, 0.3)),
        (v(0.0, -0.61, 0.0), v(0.4, 0.01, 0.3)),
    ] {
        b.add_collider(cuboid(c, he), layers::ALL, true);
    }
    for k in 0..12 {
        let a = k as f32 * 0.9;
        b.add_collider(
            StaticShape::Cylinder {
                base: v(0.2 * libm::cosf(a), 0.35 * libm::sinf(a), -0.1),
                axis: v(0.0, 0.0, 0.3),
                radius: 0.008,
            },
            layers::ALL,
            true,
        );
    }
    b.add_collider(
        StaticShape::TriMesh {
            vertices: vec![
                v(-0.1, -0.5, -0.1),
                v(0.1, -0.5, -0.1),
                v(0.0, -0.4, 0.0),
                v(0.0, -0.58, 0.0),
            ],
            indices: vec![[0, 1, 2], [0, 3, 1], [0, 2, 3], [1, 3, 2]],
        },
        layers::ALL,
        true,
    );
    let world = b.build();

    let arc: Vec<Vec3> = (0..=18)
        .map(|k| {
            let a = (10.0 * k as f32).to_radians();
            pitch * v(0.25 * libm::cosf(a), 0.3 + 0.25 * libm::sinf(a), 0.0)
        })
        .map(|p| v(p.x, p.y, 0.0))
        .collect();
    let guides = vec![GuideCurve::new(CubicSpline::interpolate(&arc).unwrap(), 0.2).unwrap()];
    let rail: Vec<Vec3> = (0..=10)
        .map(|k| v(-0.2 + 0.04 * k as f32, -0.2, 0.02 + 0.004 * k as f32))
        .collect();
    let rolls = vec![0.0; rail.len()];
    let wireforms = vec![
        Wireform::new(
            CubicSpline::interpolate(&rail).unwrap(),
            &rolls,
            0.02,
            Vec3::Z,
        )
        .unwrap(),
    ];

    let mut s = solver();
    s.add_external_force(Box::new(Breeze));
    for (k, pos) in [v(0.1, 0.1, 0.0), v(-0.15, -0.3, 0.0)]
        .into_iter()
        .enumerate()
    {
        let id = s.add_magnet(Magnet {
            position: pos,
            k_coil: 4.0e-6,
            d_min: 0.01,
            range: 0.12,
            layer_mask: layers::ALL,
        });
        s.set_magnet_power(id, 0.5 + 0.5 * k as f32);
    }
    for k in 0..6u16 {
        s.add_sensor(Sensor {
            switch: SwitchId(10 + k),
            kind: if k % 2 == 0 {
                SensorKind::Eddy
            } else {
                SensorKind::Opto
            },
            center: v(-0.25 + 0.1 * f32::from(k), -0.1, 0.01),
            radius: 0.03,
            layer_mask: layers::ALL,
        });
    }
    let path: Vec<Vec3> = (0..8)
        .map(|k| pitch * v(-0.1 + 0.027 * k as f32, 0.15, 0.0135))
        .collect();
    s.add_newton_chain(
        NewtonChain::new(
            CubicSpline::interpolate(&path).unwrap(),
            STEEL,
            &[2.0, 3.0, 4.0],
        )
        .unwrap(),
    );
    for k in 0..7u16 {
        let p = pin_core::ids::ProfileId(k % 6);
        let mut ball = ball_at(
            p,
            v(
                -0.24 + 0.08 * f32::from(k),
                0.5 - 0.05 * f32::from(k),
                0.12 + 0.01 * f32::from(k),
            ),
            v(0.3 * f32::from(k) - 1.0, -2.0 + 0.5 * f32::from(k), 0.2),
        );
        ball.angular_velocity = v(20.0, -30.0 * f32::from(k), 10.0);
        s.add_ball(ball).unwrap();
    }
    (
        Table {
            world,
            guides,
            wireforms,
        },
        s,
    )
}

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let x = black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(x);
}

#[test]
fn step_does_not_allocate() {
    let (table, mut s) = build();
    let geo = Geometry {
        world: &table.world,
        guides: &table.guides,
        wireforms: &table.wireforms,
    };
    let mut ev = BallEvents::new();
    let mut switches = [0u64; 4];
    let mut impacts = 0usize;
    let before = allocs();
    for tick in 0..3000 {
        ev.clear();
        if tick == 1000 {
            s.apply_impulse(BallHandle(0), v(0.1, 0.2, 0.05));
            s.set_magnet_power(pin_core::ids::MagnetId(0), 1.0);
            s.newton_chain_mut(0).strike(0, 1.0);
        }
        s.step(&geo, DT, &mut ev);
        s.sensor_switches(&mut switches);
        impacts += ev.as_slice().len();
    }
    let after = allocs();
    assert_eq!(after - before, 0, "step allocated");
    // The scenario really exercised the solver.
    assert!(impacts > 20, "{impacts}");
    for (_, b) in s.balls() {
        assert!(b.position.is_finite() && b.velocity.is_finite());
        assert!(b.position.abs().max_element() < 0.7, "{:?}", b.position);
    }
}

#[test]
fn five_second_scenario_is_bit_stable_over_ten_runs() {
    let run = || {
        let (table, mut s) = build();
        let geo = Geometry {
            world: &table.world,
            guides: &table.guides,
            wireforms: &table.wireforms,
        };
        let mut ev = BallEvents::new();
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for tick in 0..5000 {
            ev.clear();
            if tick % 700 == 350 {
                s.apply_impulse(BallHandle((tick / 700) as u8), v(0.05, 0.1, 0.02));
            }
            s.step(&geo, DT, &mut ev);
            // FNV-1a over the whole state, every tick (a replay hash).
            for word in state_bits(&s) {
                hash = (hash ^ u64::from(word)).wrapping_mul(0x0000_0100_0000_01b3);
            }
            hash = (hash ^ ev.as_slice().len() as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        let chain = s.newton_chain(0);
        let chain_bits: Vec<u32> = (0..chain.len())
            .flat_map(|i| [chain.ball_t(i).to_bits(), chain.ball_speed(i).to_bits()])
            .collect();
        (hash, state_bits(&s), chain_bits)
    };
    let first = run();
    for _ in 0..9 {
        assert_eq!(run(), first);
    }
}
