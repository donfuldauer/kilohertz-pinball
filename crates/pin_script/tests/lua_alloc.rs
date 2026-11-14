//! Gate step 6: steady-state `on_tick` / `on_switch_edge` of a script that does
//! not itself create Lua objects perform zero heap allocations, counting both
//! Rust-side and Lua-VM allocations (mlua routes the VM through the global
//! allocator, so this is the stricter check).

use pin_core::ids::SwitchId;
use pin_core::{
    HostCommandQueue, HostCtx, HostView, Pcg32, SeedableRng, SwitchEdge, TableScriptBackend,
};
use pin_script::NameTables;
use pin_script::lua::{LuaConfig, LuaScriptBackend};
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

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let v = std::hint::black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(v);
}

const SCRIPT: &str = r#"
local n = 0
function on_tick()
  n = n + 1
  if pinball.switch(3) then pinball.pulse_coil(pinball.coil.kick, 20) end
  pinball.set_lamp(n % 8, 1)
  pinball.hold_coil(2, n % 2 == 0)
  pinball.set_score(1, n * 10)
  if pinball.random(100) > 50 then pinball.play_sound(1, 0.5) end
  local r = math.random(1, 6)
  pinball.set_gi_tint(1, 1, 1, 1)
  pinball.persist_set(1, pinball.persist_get(1) + pinball.tick() + pinball.balls_in_play())
end
function on_switch(id, closed)
  if closed then pinball.pulse_coil(id, 10) else pinball.set_diverter(1, false) end
end
"#;

#[test]
fn steady_state_callbacks_do_not_allocate() {
    let names = NameTables {
        coils: vec![("kick".into(), pin_core::CoilId(5))],
        ..NameTables::default()
    };
    let mut b = LuaScriptBackend::new(SCRIPT.as_bytes(), &names, LuaConfig::default()).unwrap();
    let mut view = HostView::default();
    let mut rng = Pcg32::seed_from_u64(7);
    let mut cmd = HostCommandQueue::default();
    let mut run = |view: &mut HostView, ticks: u64| {
        for t in 0..ticks {
            view.tick += 1;
            view.set_switch(SwitchId(3), t % 3 == 0);
            cmd.clear();
            let mut ctx = HostCtx {
                view,
                rng: &mut rng,
                cmd: &mut cmd,
            };
            b.on_tick(&mut ctx);
            let edge = if t % 2 == 0 {
                SwitchEdge::Closed
            } else {
                SwitchEdge::Opened
            };
            b.on_switch_edge(SwitchId((t % 16) as u16), edge, &mut ctx);
        }
    };
    // Warm-up: Lua grows its stacks and CallInfo lists once.
    run(&mut view, 2_000);
    let before = allocs();
    run(&mut view, 10_000);
    let n = allocs() - before;
    assert_eq!(n, 0, "{n} allocations in 10k steady-state ticks");
    assert!(!b.is_faulted());
}
