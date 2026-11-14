//! Gate step 6 for the mechs: a tick (pre_step, the ball step with the mech
//! bodies, post_step) never allocates (counting global allocator, per
//! thread), and a 3 s scenario with every mech kind ends in bit-identical
//! state over 10 runs.

#[path = "mech_util.rs"]
mod util;

use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_physics::ball::{Ball, Geometry};
use pin_physics::mech::MechSpec;
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

fn spec(kind: MechKind, p: Vec3, angle: f32, n: u16) -> MechSpec {
    let mut s = MechSpec::new(kind, p)
        .with_coil(CoilId(n))
        .with_switch(SwitchId(n));
    s.params.angle_deg = Some(angle);
    s
}

/// Every kind on a walled, pitched playfield with six balls.
fn busy_table() -> Table {
    let specs = [
        spec(MechKind::Bumper, v(-0.1, 0.25, 0.0), 0.0, 1),
        spec(MechKind::Bumper, v(0.1, 0.27, 0.0), 0.0, 2),
        spec(MechKind::Sling, v(-0.2, -0.15, 0.0), 30.0, 3),
        spec(MechKind::Sling, v(0.2, -0.15, 0.0), 150.0, 4),
        spec(MechKind::DropTarget, v(-0.1, 0.4, 0.0), -90.0, 5),
        spec(MechKind::DropTarget, v(-0.07, 0.4, 0.0), -90.0, 5),
        spec(MechKind::Spinner, v(0.0, 0.1, 0.0), 90.0, 6),
        spec(MechKind::Vuk, v(0.2, 0.3, 0.0), 180.0, 7),
        spec(MechKind::Plunger, v(0.3, -0.4, 0.0), 90.0, 8),
        spec(MechKind::Kickback, v(-0.3, -0.4, 0.0), 90.0, 9),
        spec(MechKind::Gate, v(0.0, 0.0, 0.0), 0.0, 10),
    ];
    let wall = |c: Vec3, h: Vec3| (c, h, layers::MAIN_DECK);
    let walls = [
        wall(v(0.36, 0.0, 0.03), v(0.01, 0.5, 0.03)),
        wall(v(-0.36, 0.0, 0.03), v(0.01, 0.5, 0.03)),
        wall(v(0.0, 0.48, 0.03), v(0.36, 0.01, 0.03)),
        wall(v(0.0, -0.48, 0.03), v(0.36, 0.01, 0.03)),
        (v(0.2, 0.4, 0.04), v(0.1, 0.1, 0.0065), layers::UPPER_DECK),
    ];
    let mut t = Table::new(&specs, world(&walls), 6.5);
    t.mechs.set_tilt_switch(Some(SwitchId(100)));
    for (i, (x, y)) in [
        (-0.1, 0.4),
        (0.0, 0.3),
        (0.2, 0.0),
        (0.3, -0.38),
        (-0.3, -0.37),
        (0.15, 0.3),
    ]
    .into_iter()
    .enumerate()
    {
        t.ball(x, y, v(0.3 * (i as f32 - 2.5), -0.5, 0.0));
    }
    t
}

/// Drive for tick `i`: coils, plunger, nudges (all allocation-free).
fn drive(t: &mut Table, i: u32) {
    if i.is_multiple_of(97) {
        t.mechs.fire_coil(CoilId(1 + (i / 97 % 4) as u16), 25);
    }
    if i % 400 == 200 {
        t.mechs.fire_coil(CoilId(5), 60);
        t.mechs.fire_coil(CoilId(7), 20);
        t.mechs.fire_coil(CoilId(9), 30);
    }
    if i % 600 < 300 {
        t.mechs.set_plunger((i % 600) as f32 / 300.0);
    } else {
        t.mechs.set_plunger(0.0);
    }
    if i % 777 == 500 {
        t.mechs.nudge(v(0.15, 0.05, 0.0));
    }
    if i == 1500 {
        t.mechs.hold_coil(CoilId(3), true);
    }
    if i == 1700 {
        t.mechs.hold_coil(CoilId(3), false);
        t.mechs.fire_coil(CoilId(8), 40);
    }
}

fn state_bits(t: &Table) -> Vec<u32> {
    let mut out = Vec::new();
    for (h, b) in t.solver.balls() {
        out.push(u32::from(h.0));
        out.push(u32::from(b.layer));
        for x in b
            .position
            .to_array()
            .into_iter()
            .chain(b.velocity.to_array())
            .chain(b.angular_velocity.to_array())
            .chain(b.orientation.to_array())
        {
            out.push(x.to_bits());
        }
    }
    for i in 0..t.mechs.len() {
        if let Some((x, v)) = t.mechs.slider_state(i) {
            out.extend([x.to_bits(), v.to_bits()]);
        }
        if let Some((a, w)) = t.mechs.spinner_state(i) {
            out.extend([a.to_bits(), w.to_bits()]);
        }
        out.push(t.mechs.spinner_pulses(i) as u32);
        out.push(u32::from(t.mechs.mech_switch_closed(i)));
    }
    out.extend(t.mechs.tilt_bob_offset().to_array().map(f32::to_bits));
    let mut s = [0u64; 4];
    t.mechs.switch_states(&mut s);
    out.extend(s.iter().flat_map(|w| [*w as u32, (*w >> 32) as u32]));
    out
}

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let v = black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(v);
}

#[test]
fn mech_tick_does_not_allocate() {
    let mut t = busy_table();
    let mut edges = 0usize;
    for i in 0..3000u32 {
        drive(&mut t, i);
        let before = allocs();
        t.tick();
        let after = allocs();
        assert_eq!(after, before, "tick {i} allocated");
        edges += t.mechs.edges().len();
    }
    // Also the split hooks with an explicit body slice.
    let mut flipper_stand_in =
        pin_physics::mech::MechSet::new(&[], &pin_core::PhysicsDefaults::builtin()).unwrap();
    for i in 0..200u32 {
        drive(&mut t, i);
        t.events.clear();
        let before = allocs();
        t.mechs.pre_step(&mut t.solver, DT);
        let geo = Geometry::new(&t.world);
        t.solver.step_with_bodies(
            &geo,
            &mut [&mut flipper_stand_in, &mut t.mechs],
            DT,
            &mut t.events,
        );
        t.mechs.post_step(&mut t.solver, t.events.as_slice(), DT);
        assert_eq!(allocs(), before, "split tick {i} allocated");
    }
    assert!(edges > 20, "the scenario exercises the switches: {edges}");
    let _ = Ball::new;
}

#[test]
fn ten_runs_are_bit_identical() {
    let mut first = None;
    for _ in 0..10 {
        let mut t = busy_table();
        for i in 0..3000u32 {
            drive(&mut t, i);
            t.tick();
        }
        let bits = state_bits(&t);
        match &first {
            None => first = Some(bits),
            Some(f) => assert_eq!(f, &bits),
        }
    }
}
