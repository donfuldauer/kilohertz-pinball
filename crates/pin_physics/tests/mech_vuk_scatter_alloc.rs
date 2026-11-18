//! Work item w7pre-vuk-scatter, D5: a tick in which a VUK ejects (and
//! draws its scatter) never allocates (counting global allocator, per
//! thread), over many ejects.

#[path = "mech_util.rs"]
mod util;

use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_physics::mech::MechSpec;
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

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let v = black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(v);
}

#[test]
fn scattered_vuk_eject_ticks_do_not_allocate() {
    let mut spec = MechSpec::new(MechKind::Vuk, Vec3::ZERO)
        .with_coil(CoilId(1))
        .with_switch(SwitchId(1));
    spec.params.exit_layer = Some(0);
    spec.params.exit_offset = Some([0.0, 0.0, 80.0]);
    let mut t = Table::new(&[spec], world(&[]), 6.5);
    t.mechs.set_scatter_seed(3);
    let mut ejected = 0;
    for _ in 0..60 {
        let h = t.ball(0.0, 0.0, Vec3::ZERO);
        let before = allocs();
        t.tick();
        assert_eq!(allocs(), before, "capture tick allocated");
        assert_eq!(t.mechs.vuk_ball(0), Some(h));
        t.mechs.fire_coil(CoilId(1), 20);
        let before = allocs();
        t.tick();
        assert_eq!(allocs(), before, "eject tick allocated");
        assert_eq!(t.mechs.vuk_ball(0), None);
        ejected += 1;
        t.solver.remove_ball(h).expect("ball");
        t.mechs.release_ball(h);
        for _ in 0..25 {
            let before = allocs();
            t.tick();
            assert_eq!(allocs(), before, "tick allocated");
        }
    }
    assert_eq!(ejected, 60);
    // Re-seeding is allocation-free as well.
    let before = allocs();
    t.mechs.set_scatter_seed(4);
    assert_eq!(allocs(), before, "set_scatter_seed allocated");
}
