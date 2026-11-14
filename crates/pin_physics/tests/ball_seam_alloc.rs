//! Seam task 2.2-S, gate step 6: `BallSolver::step_with_bodies` with two
//! rotating bodies, a kinematic kicker and a context-reading force hook
//! never allocates (counting global allocator, per thread), and the state
//! hash of that scenario is stable over 10 runs.

#[path = "ball_seam_ref.rs"]
mod reference;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

use glam::Vec3;
use pin_core::BallProfile;
use pin_physics::ball::{Ball, BallEvents, BallHandle, ExternalForce, ForceContext, MAX_BALLS};
use reference::*;

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

/// Magnus-like spin force plus a per-ball counter reset on `ball_added`,
/// reading the magnets through the context.
struct SpinHook {
    calls: [u32; MAX_BALLS],
}

impl ExternalForce for SpinHook {
    fn force_in(
        &mut self,
        ctx: &ForceContext<'_>,
        handle: BallHandle,
        ball: &Ball,
        profile: &BallProfile,
        _dt: f32,
    ) -> Vec3 {
        self.calls[usize::from(handle.0)] += 1;
        let mut f = ball.angular_velocity.cross(ball.velocity) * (1.0e-6 * profile.mass_kg);
        for m in ctx.magnets {
            if m.in_range(ball.position, ball.layer) {
                f += m.pull(ball.position) * (0.1 * profile.magnetic_coupling);
            }
        }
        f
    }
    fn ball_added(&mut self, handle: BallHandle, _ball: &Ball) {
        self.calls[usize::from(handle.0)] = 0;
    }
}

fn table() -> FlipperTable {
    let mut t = FlipperTable::new(4);
    t.solver.add_external_force(Box::new(SpinHook {
        calls: [0; MAX_BALLS],
    }));
    let id = t.solver.add_magnet(pin_physics::ball::Magnet {
        position: Vec3::new(0.2, 0.45, -0.005),
        k_coil: 2.0e-5,
        d_min: 0.01,
        range: 0.08,
        layer_mask: pin_physics::world::layers::ALL,
    });
    t.solver.set_magnet_power(id, 0.5);
    t
}

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let x = black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(x);
}

#[test]
fn step_with_bodies_and_hooks_does_not_allocate() {
    let mut t = table();
    let mut ev = BallEvents::new();
    let mut body_events = 0usize;
    let before = allocs();
    for tick in 0..4000u32 {
        t.drive(tick);
        ev.clear();
        if tick == 2000 {
            // Lifecycle calls inside the guarded window too.
            let b = t.solver.remove_ball(BallHandle(1)).unwrap();
            t.solver.add_ball(b).unwrap();
        }
        t.step(&mut ev);
        body_events += ev
            .as_slice()
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    pin_physics::ball::BallEvent::Impact {
                        other: pin_physics::ball::ImpactTarget::Body { .. },
                        ..
                    }
                )
            })
            .count();
    }
    let after = allocs();
    assert_eq!(after - before, 0, "step_with_bodies allocated");
    assert!(body_events > 10, "{body_events}");
}

#[test]
fn bodies_and_hooks_scenario_is_bit_stable_over_ten_runs() {
    let run = || {
        let mut t = table();
        let mut ev = BallEvents::new();
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for tick in 0..3000u32 {
            t.drive(tick);
            ev.clear();
            t.step(&mut ev);
            for w in t.state_bits() {
                hash = (hash ^ u64::from(w)).wrapping_mul(0x0000_0100_0000_01b3);
            }
            hash = (hash ^ ev.as_slice().len() as u64).wrapping_mul(0x0000_0100_0000_01b3);
        }
        (hash, t.state_bits())
    };
    let first = run();
    for _ in 0..9 {
        assert_eq!(run(), first);
    }
}
