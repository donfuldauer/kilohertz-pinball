//! Spec: "Faults never panic or stall the tick". These scenarios hide script
//! code in places a naive per-callback budget does not reach (message
//! handlers, finalizers). They live in their own binary because a regression
//! here shows up as a hang rather than a failed assertion.

mod spec_support;

use pin_core::TableScriptBackend;
use spec_support::{Host, backend_with, cfg};

const FAULTS: &[u8] = include_bytes!("fixtures/spec_faults.lua");
const MIB: usize = 1024 * 1024;

/// `while true do xpcall(function() error("x") end, <handler that loops>) end`
#[test]
fn xpcall_with_a_looping_message_handler_is_aborted() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 50_000));
    let mut h = Host::new(1);
    h.case(&mut b, 25);
    assert!(b.is_faulted());
    assert_eq!(b.fault_count(), 1);
    h.tick(&mut b);
    assert!(h.cmd.is_empty());
}

/// A single xpcall whose handler loops, without the outer retry loop.
#[test]
fn a_single_looping_message_handler_is_aborted() {
    let src = b"function on_tick()
                  xpcall(function() error('x') end, function() while true do end end)
                  while true do end
                end";
    let mut b = backend_with(src, cfg(16 * MIB, 50_000));
    let mut h = Host::new(1);
    h.tick(&mut b);
    assert!(b.is_faulted());
}

/// 200 objects whose `__gc` never returns become garbage; later ticks (and the
/// per-tick GC step) must still return.
#[test]
fn looping_finalizers_do_not_stall_later_ticks() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 16);
    for _ in 0..300 {
        h.tick(&mut b);
        h.case(&mut b, 27);
    }
}

/// Garbage with looping finalizers created while memory pressure forces
/// collections inside the callback itself.
#[test]
fn looping_finalizers_under_memory_pressure_do_not_stall() {
    let src = b"function on_tick()
                  for i = 1, 50 do
                    setmetatable({}, { __gc = function() while true do end end })
                  end
                  local t = {}
                  for i = 1, 64 do t[i] = string.rep('p', 32768) .. i end
                end";
    let mut b = backend_with(src, cfg(4 * MIB, 50_000));
    let mut h = Host::new(1);
    for _ in 0..200 {
        h.tick(&mut b);
    }
}

#[test]
fn looping_finalizers_do_not_stall_reload() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 16);
    let _ = b.apply_reload(b"function on_tick() pinball.show_scene(1) end");
    for _ in 0..50 {
        h.tick(&mut b);
    }
}

/// Closing a Lua state runs every pending finalizer.
#[test]
fn looping_finalizers_do_not_stall_drop() {
    let src = b"keep = {}
                function on_tick()
                  for i = 1, 20 do
                    keep[#keep + 1] = setmetatable({}, { __gc = function() while true do end end })
                  end
                end";
    let mut b = backend_with(src, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.tick(&mut b);
    drop(b);
}

/// A finalizer that raises instead of looping must not take the host down.
#[test]
fn erroring_finalizers_do_not_panic() {
    let src = b"function on_tick()
                  for i = 1, 100 do
                    setmetatable({}, { __gc = function() error('in gc') end })
                  end
                  pinball.show_scene(1)
                end";
    let mut b = backend_with(src, cfg(16 * MIB, 50_000));
    let mut h = Host::new(1);
    for _ in 0..300 {
        h.tick(&mut b);
    }
}
