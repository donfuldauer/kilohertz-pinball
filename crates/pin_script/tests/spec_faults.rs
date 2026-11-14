//! Spec: instruction budget and memory cap faults drop the backend into the
//! "tilt" safe state — held coils are released, script functions are no longer
//! called, nothing panics or stalls — until a successful `apply_reload`.

mod spec_support;

use pin_core::{CoilId, TableScriptBackend};
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use proptest::prelude::*;
use spec_support::{Host, backend, backend_with, cfg, names, score};

const FAULTS: &[u8] = include_bytes!("fixtures/spec_faults.lua");
const MIB: usize = 1024 * 1024;

/// Sorted `(coil, on)` pairs currently queued.
fn holds(h: &Host) -> Vec<(u16, bool)> {
    let mut v: Vec<_> = h.cmd.coil_holds.iter().map(|&(c, on)| (c.0, on)).collect();
    v.sort_unstable();
    v
}

/// After a fault no script function may run: every callback leaves the queue empty.
fn assert_script_is_silent(b: &mut LuaScriptBackend, h: &mut Host) {
    let faults = b.fault_count();
    for _ in 0..3 {
        h.tick(b);
        assert!(h.cmd.is_empty(), "on_tick ran after a fault: {:?}", h.cmd);
        h.case(b, 27);
        assert!(h.cmd.is_empty(), "on_switch ran after a fault: {:?}", h.cmd);
        h.case(b, 11);
        assert!(h.cmd.is_empty(), "on_switch ran after a fault: {:?}", h.cmd);
    }
    h.cmd.clear();
    h.init(b);
    assert!(h.cmd.is_empty(), "on_init ran after a fault");
    assert!(b.is_faulted());
    assert_eq!(
        b.fault_count(),
        faults,
        "a silent script cannot fault again"
    );
}

#[test]
fn healthy_script_is_not_faulted() {
    let mut b = backend(FAULTS);
    let mut h = Host::new(1);
    h.init(&mut b);
    h.tick(&mut b);
    h.case(&mut b, 27);
    assert_eq!(h.cmd.coil_pulses.as_slice(), &[(CoilId(2), 20)]);
    assert!(!b.is_faulted());
    assert_eq!(b.fault_count(), 0);
}

#[test]
fn infinite_loop_is_aborted_with_the_default_budget() {
    let mut b = backend(FAULTS);
    let mut h = Host::new(1);
    h.case(&mut b, 1);
    assert!(b.is_faulted());
    assert_eq!(b.fault_count(), 1);
    assert_script_is_silent(&mut b, &mut h);
}

#[test]
fn infinite_loop_in_on_tick_is_aborted() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 101); // arm case 1 for on_tick
    assert!(!b.is_faulted());
    h.tick(&mut b);
    assert!(b.is_faulted());
    assert_eq!(b.fault_count(), 1);
    assert_script_is_silent(&mut b, &mut h);
}

#[test]
fn infinite_loop_in_on_init_is_aborted() {
    let src = b"function on_init() while true do end end
                function on_tick() pinball.pulse_coil(1, 10) end";
    let mut b = backend_with(src, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.init(&mut b);
    assert!(b.is_faulted());
    assert_eq!(b.fault_count(), 1);
    h.tick(&mut b);
    assert!(h.cmd.is_empty(), "on_tick ran after on_init faulted");
}

/// Runs one fault scenario both directly from `on_switch` and armed for `on_tick`.
fn scenario_faults(case: u16, what: &str) {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 50_000));
    let mut h = Host::new(1);
    h.case(&mut b, case);
    assert!(b.is_faulted(), "{what} (on_switch) must fault");
    assert_eq!(b.fault_count(), 1, "{what} (on_switch)");
    assert_script_is_silent(&mut b, &mut h);

    let mut b = backend_with(FAULTS, cfg(16 * MIB, 50_000));
    let mut h = Host::new(1);
    h.case(&mut b, 100 + case);
    h.tick(&mut b);
    assert!(b.is_faulted(), "{what} (on_tick) must fault");
    assert_eq!(b.fault_count(), 1, "{what} (on_tick)");
}

/// One test per scenario, so a stall is attributable to a single case.
macro_rules! fault_scenarios {
    ($($name:ident => ($case:expr, $what:expr);)*) => {$(
        #[test]
        fn $name() {
            scenario_faults($case, $what);
        }
    )*};
}

fault_scenarios! {
    scenario_infinite_loop => (1, "infinite loop");
    scenario_loop_swallowing_errors_with_pcall => (2, "infinite loop swallowing errors with pcall");
    scenario_absurd_allocation => (5, "single absurd allocation");
    scenario_explicit_error => (6, "error()");
    scenario_error_value_with_looping_tostring => (7, "error value with a looping __tostring");
    scenario_argument_type_error => (8, "argument type error");
    scenario_missing_arguments => (9, "missing arguments");
    scenario_undefined_global_call => (10, "call of an undefined global");
    scenario_unbounded_recursion => (14, "unbounded recursion");
    scenario_error_nil => (19, "error(nil)");
    scenario_table_as_lamp_id => (20, "table passed as lamp id");
    scenario_string_as_player => (21, "string passed as player");
    scenario_nil_as_coil_id => (22, "nil passed as coil id");
    scenario_string_as_persist_key => (23, "string passed as persist key");
    scenario_loop_inside_metamethod => (24, "infinite loop inside a metamethod");
    scenario_goto_loop => (26, "goto loop");
}

#[test]
fn memory_bomb_is_aborted_with_the_default_config() {
    let mut b = backend(FAULTS);
    let mut h = Host::new(1);
    h.case(&mut b, 3);
    assert!(b.is_faulted());
    assert_eq!(b.fault_count(), 1);
    assert_script_is_silent(&mut b, &mut h);
}

#[test]
fn memory_bomb_is_aborted_with_a_small_cap() {
    for limit in [2 * MIB, 4 * MIB] {
        let mut b = backend_with(FAULTS, cfg(limit, 1_000_000));
        let mut h = Host::new(1);
        h.case(&mut b, 27);
        assert!(!b.is_faulted(), "a trivial callback fits in {limit} bytes");
        h.case(&mut b, 3);
        assert!(b.is_faulted(), "memory bomb under a {limit} byte cap");
        assert_script_is_silent(&mut b, &mut h);
    }
}

#[test]
fn memory_bomb_that_swallows_errors_still_ends_in_a_fault() {
    let mut b = backend_with(FAULTS, cfg(4 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 4);
    assert!(b.is_faulted());
    assert_script_is_silent(&mut b, &mut h);
}

/// DECISIONS "4.2 re-review round 2": a faulting callback's own commands are
/// dropped.
#[test]
fn commands_of_the_faulting_callback_are_dropped() {
    let mut b = backend(FAULTS);
    let mut h = Host::new(1);
    h.case(&mut b, 17); // pulse_coil(1, 10) then error
    assert!(b.is_faulted());
    assert!(
        h.cmd.is_empty(),
        "the faulting callback's commands leaked: {:?}",
        h.cmd
    );
}

#[test]
fn every_kind_of_command_from_the_faulting_callback_is_dropped() {
    for tail in ["error('x')", "while true do end", "pinball.set_lamp({}, 1)"] {
        let src = format!(
            "function on_tick()
               local p = pinball
               p.pulse_coil(1, 10) p.set_magnet(1, 1) p.set_zipper(1, true)
               p.set_diverter(1, true) p.spawn_ball(1, 1) p.set_lamp(1, 1)
               p.set_gi_tint(1, 1, 1, 1) p.set_score(1, 5) p.show_scene(1)
               p.play_sound(1, 1) p.persist_set(1, 1)
               {tail}
             end"
        );
        let mut b = backend_with(src.as_bytes(), cfg(16 * MIB, 20_000));
        let mut h = Host::new(1);
        h.tick(&mut b);
        assert!(b.is_faulted(), "{tail}");
        assert!(h.cmd.is_empty(), "{tail}: commands leaked: {:?}", h.cmd);
        assert_eq!(b.overflow_count(), 0, "{tail}");
    }
}

/// Commands queued by earlier callbacks (not yet cleared by the host) survive
/// a later callback's fault untouched.
#[test]
fn commands_of_earlier_callbacks_survive_a_later_fault() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.cmd.clear();
    let closed = pin_core::SwitchEdge::Closed;
    b.on_switch_edge(pin_core::SwitchId(27), closed, &mut h.ctx()); // pulse_coil(2, 20)
    b.on_switch_edge(pin_core::SwitchId(11), closed, &mut h.ctx()); // hold 3 and 5
    h.cmd.show_scene(pin_core::SceneId(9)).unwrap(); // something the host itself queued
    let before = h.cmd.clone();
    b.on_switch_edge(pin_core::SwitchId(17), closed, &mut h.ctx()); // pulse, then error
    assert!(b.is_faulted());

    assert_eq!(
        h.cmd.coil_pulses, before.coil_pulses,
        "earlier pulses must be kept"
    );
    assert_eq!(h.cmd.display_scenes, before.display_scenes);
    // Earlier holds are kept, followed by the tilt releases.
    assert_eq!(&h.cmd.coil_holds[..2], &before.coil_holds[..]);
    let mut released: Vec<_> = h.cmd.coil_holds[2..]
        .iter()
        .map(|&(c, on)| (c.0, on))
        .collect();
    released.sort_unstable();
    assert_eq!(released, vec![(3, false), (5, false)]);
    assert_eq!(h.cmd.len(), before.len() + 2);
}

/// Rollback must never strand a held coil (review B8): tick 1 holds the coil,
/// tick 2 releases it and then faults. Whether the release reaches the host
/// from tick 2's queue or from the tilt release, it arrives exactly once.
#[test]
fn release_in_the_faulting_callback_is_not_lost() {
    for tail in ["error('x')", "while true do end", "pinball.pulse_coil()"] {
        let src = format!(
            "local n = 0
             function on_tick()
               n = n + 1
               if n == 1 then
                 pinball.hold_coil(9, true)
                 pinball.hold_coil(4, true)
               else
                 pinball.hold_coil(9, false)
                 {tail}
               end
             end"
        );
        let mut b = backend_with(src.as_bytes(), cfg(16 * MIB, 20_000));
        let mut h = Host::new(1);
        h.tick(&mut b);
        assert_eq!(holds(&h), vec![(4, true), (9, true)]);
        h.tick(&mut b);
        assert!(b.is_faulted(), "{tail}");
        assert_eq!(
            holds(&h),
            vec![(4, false), (9, false)],
            "{tail}: each held coil must be released exactly once"
        );
        h.tick(&mut b);
        assert!(h.cmd.is_empty(), "{tail}: releases are not repeated");
    }
}

/// A coil held and released inside the faulting callback ends released: the
/// host never sees a hold that is not followed by a release.
#[test]
fn coil_held_and_released_within_the_faulting_callback_ends_released() {
    for tail in ["error('x')", "while true do end"] {
        let src = format!(
            "function on_tick()
               pinball.hold_coil(6, true)
               pinball.hold_coil(6, false)
               {tail}
             end"
        );
        let mut b = backend_with(src.as_bytes(), cfg(16 * MIB, 20_000));
        let mut h = Host::new(1);
        h.tick(&mut b);
        assert!(b.is_faulted(), "{tail}");
        let last = h.cmd.coil_holds.iter().rev().find(|(c, _)| *c == CoilId(6));
        assert!(
            last.is_none_or(|&(_, on)| !on),
            "{tail}: coil 6 left held: {:?}",
            h.cmd.coil_holds
        );
        assert!(h.cmd.coil_holds.iter().filter(|&&(_, on)| !on).count() <= 1);
    }
}

/// Released in the faulting callback, then held again before the error: the
/// rolled-back callback leaves the coil held from tick 1, so tilt releases it.
#[test]
fn coil_toggled_within_the_faulting_callback_is_released_once() {
    let src = b"local n = 0
                function on_tick()
                  n = n + 1
                  pinball.hold_coil(9, true)
                  if n == 2 then
                    pinball.hold_coil(9, false)
                    pinball.hold_coil(9, true)
                    error('x')
                  end
                end";
    let mut b = backend(src);
    let mut h = Host::new(1);
    h.tick(&mut b);
    h.tick(&mut b);
    assert!(b.is_faulted());
    assert_eq!(h.cmd.coil_holds.last(), Some(&(CoilId(9), false)));
    assert_eq!(h.cmd.coil_holds.iter().filter(|&&(_, on)| !on).count(), 1);
}

// ---------------------------------------------------------------------------
// Tilt: held coils are released.

#[test]
fn fault_releases_every_held_coil() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 11); // hold 3 and 5
    assert_eq!(holds(&h), vec![(3, true), (5, true)]);
    h.case(&mut b, 1);
    assert!(b.is_faulted());
    assert_eq!(holds(&h), vec![(3, false), (5, false)]);
    assert_eq!(h.cmd.len(), 2, "tilt only releases coils: {:?}", h.cmd);
    // Released once, not every tick.
    h.tick(&mut b);
    assert!(h.cmd.is_empty());
}

#[test]
fn fault_does_not_release_coils_the_script_already_released() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 11); // hold 3 and 5
    h.case(&mut b, 12); // release 3
    assert_eq!(holds(&h), vec![(3, false)]);
    h.case(&mut b, 6); // error("boom")
    assert!(b.is_faulted());
    assert_eq!(holds(&h), vec![(5, false)]);
}

#[test]
fn fault_without_held_coils_queues_nothing() {
    for case in [1, 3, 6, 8] {
        let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
        let mut h = Host::new(1);
        h.case(&mut b, case);
        assert!(b.is_faulted());
        assert!(
            h.cmd.coil_holds.is_empty(),
            "case {case}: {:?}",
            h.cmd.coil_holds
        );
    }
}

#[test]
fn coil_held_in_the_faulting_callback_ends_up_released() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 13); // hold 7, then hang
    assert!(b.is_faulted());
    // The callback's own hold may be dropped with the rest of its commands;
    // if any hold command for coil 7 reaches the host, the last one is "off".
    let last = h.cmd.coil_holds.iter().rev().find(|(c, _)| *c == CoilId(7));
    assert!(
        last.is_none_or(|&(_, on)| !on),
        "coil 7 left held: {:?}",
        h.cmd.coil_holds
    );
}

#[test]
fn coil_held_twice_is_released() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 18);
    h.case(&mut b, 1);
    let v = holds(&h);
    assert!(!v.is_empty());
    assert!(v.iter().all(|&(c, on)| c == 3 && !on), "{v:?}");
}

#[test]
fn memory_fault_releases_held_coils() {
    let mut b = backend_with(FAULTS, cfg(4 * MIB, 1_000_000));
    let mut h = Host::new(1);
    h.case(&mut b, 11);
    h.case(&mut b, 3);
    assert!(b.is_faulted());
    assert_eq!(holds(&h), vec![(3, false), (5, false)]);
}

#[test]
fn argument_error_releases_held_coils() {
    let mut b = backend(FAULTS);
    let mut h = Host::new(1);
    h.case(&mut b, 11);
    h.case(&mut b, 8);
    assert!(b.is_faulted());
    assert_eq!(holds(&h), vec![(3, false), (5, false)]);
}

#[test]
fn fault_in_on_tick_releases_held_coils() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 11);
    h.case(&mut b, 101);
    h.tick(&mut b);
    assert!(b.is_faulted());
    assert_eq!(holds(&h), vec![(3, false), (5, false)]);
}

#[test]
fn fault_with_a_full_hold_queue_does_not_panic() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 11);
    h.cmd.clear();
    for i in 0..32 {
        h.cmd.hold_coil(CoilId(100 + i), true).unwrap();
    }
    b.on_switch_edge(
        pin_core::SwitchId(1),
        pin_core::SwitchEdge::Closed,
        &mut h.ctx(),
    );
    assert!(b.is_faulted());
    assert_eq!(h.cmd.coil_holds.len(), 32);
}

// ---------------------------------------------------------------------------
// The budget is per callback, not cumulative.

#[test]
fn budget_is_per_callback() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 50_000));
    let mut h = Host::new(1);
    // Each call runs a 1000-iteration loop; 500 calls total far more than the
    // budget, but every single callback stays well below it.
    for n in 1..=500i64 {
        h.case(&mut b, 15);
        assert!(!b.is_faulted(), "faulted at call {n}");
        assert_eq!(score(&h.cmd, 0), Some(n * 500_500));
    }
    for _ in 0..500 {
        h.case(&mut b, 115);
        h.tick(&mut b);
        assert!(!b.is_faulted());
    }
    assert_eq!(b.fault_count(), 0);
}

#[test]
fn steady_state_memory_does_not_creep_into_the_cap() {
    // Garbage produced every tick must be collected by the per-tick GC step:
    // each tick allocates ~64 KiB of garbage, 2000 ticks is ~128 MiB in total.
    let src = b"function on_tick()
                  local s = string.rep('g', 65536)
                  pinball.persist_set(0, #s)
                end";
    let mut b = backend_with(src, cfg(4 * MIB, 100_000));
    let mut h = Host::new(1);
    for t in 0..2000 {
        h.tick(&mut b);
        assert!(
            !b.is_faulted(),
            "garbage accumulated into the memory cap at tick {t}"
        );
        assert_eq!(h.cmd.persist_writes.len(), 1);
    }
}

// ---------------------------------------------------------------------------
// Load-time behaviour.

#[test]
fn top_level_infinite_loop_does_not_hang_construction() {
    match LuaScriptBackend::new(b"while true do end", &names(), cfg(16 * MIB, 20_000)) {
        Err(_) => {}
        Ok(mut b) => {
            assert!(
                b.is_faulted(),
                "a chunk that never finished cannot be healthy"
            );
            let mut h = Host::new(1);
            h.tick(&mut b);
            assert!(h.cmd.is_empty());
        }
    }
}

#[test]
fn top_level_memory_bomb_does_not_crash_construction() {
    let src = b"local t = {} while true do t[#t + 1] = string.rep('x', 60000) .. #t end";
    match LuaScriptBackend::new(src, &names(), cfg(4 * MIB, 1_000_000)) {
        Err(_) => {}
        Ok(b) => assert!(b.is_faulted()),
    }
}

#[test]
fn top_level_runtime_error_does_not_yield_a_healthy_backend() {
    match LuaScriptBackend::new(b"error('at load')", &names(), LuaConfig::default()) {
        Err(_) => {}
        Ok(b) => assert!(b.is_faulted()),
    }
}

// ---------------------------------------------------------------------------
// Recovery through reload.

#[test]
fn successful_reload_clears_the_fault_and_counts_new_faults() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 1);
    assert!(b.is_faulted());
    assert!(b.fault_count() >= 1);

    b.apply_reload(FAULTS).expect("valid source reloads");
    assert!(!b.is_faulted(), "a successful reload clears the fault");
    let after_reload = b.fault_count();

    h.case(&mut b, 27);
    assert_eq!(
        h.cmd.coil_pulses.as_slice(),
        &[(CoilId(2), 20)],
        "script runs again"
    );

    // Coils held by the new script are tracked for the next tilt.
    h.case(&mut b, 11);
    h.case(&mut b, 26);
    assert!(b.is_faulted());
    assert!(b.fault_count() > after_reload);
    assert_eq!(holds(&h), vec![(3, false), (5, false)]);
}

#[test]
fn failed_reload_leaves_a_faulted_backend_faulted() {
    let mut b = backend_with(FAULTS, cfg(16 * MIB, 20_000));
    let mut h = Host::new(1);
    h.case(&mut b, 6);
    assert!(b.is_faulted());
    assert!(b.apply_reload(b"this is not lua (").is_err());
    assert!(
        b.is_faulted(),
        "a rejected reload must not resurrect the old script"
    );
    h.case(&mut b, 27);
    assert!(h.cmd.is_empty());
}

#[test]
fn limits_still_apply_after_reload() {
    let mut b = backend_with(b"", cfg(4 * MIB, 20_000));
    b.apply_reload(FAULTS).unwrap();
    let mut h = Host::new(1);
    h.case(&mut b, 1);
    assert!(b.is_faulted(), "instruction budget lost after reload");

    b.apply_reload(FAULTS).unwrap();
    assert!(!b.is_faulted());
    h.case(&mut b, 3);
    assert!(b.is_faulted(), "memory cap lost after reload");
}

// ---------------------------------------------------------------------------
// Properties.

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Any budget aborts an infinite loop, and never the trivial callback.
    #[test]
    fn any_budget_aborts_a_loop_but_not_a_trivial_callback(budget in 2_000u32..200_000) {
        let mut b = backend_with(FAULTS, cfg(16 * MIB, budget));
        let mut h = Host::new(1);
        for _ in 0..20 {
            h.case(&mut b, 27);
            prop_assert!(!b.is_faulted());
            prop_assert_eq!(h.cmd.coil_pulses.len(), 1);
        }
        h.case(&mut b, 11);
        h.case(&mut b, 1);
        prop_assert!(b.is_faulted());
        prop_assert_eq!(b.fault_count(), 1);
        prop_assert_eq!(holds(&h), vec![(3, false), (5, false)]);
    }

    /// A loop of `n` iterations either completes (and reports the right sum)
    /// or faults; it never does anything else, whatever the budget.
    #[test]
    fn bounded_loops_complete_or_fault(n in 0u32..20_000, budget in 1_000u32..100_000) {
        let src = format!(
            "function on_tick() local s = 0 for i = 1, {n} do s = s + i end pinball.set_score(1, s) end"
        );
        let mut b = backend_with(src.as_bytes(), cfg(16 * MIB, budget));
        let mut h = Host::new(1);
        h.tick(&mut b);
        let expected = i64::from(n) * (i64::from(n) + 1) / 2;
        if b.is_faulted() {
            prop_assert!(h.cmd.scores.is_empty());
            // A loop can only fault if it is long relative to the budget.
            prop_assert!(u64::from(n) * 8 > u64::from(budget) / 2,
                "{} iterations faulted under a budget of {}", n, budget);
        } else {
            prop_assert_eq!(score(&h.cmd, 0), Some(expected));
            // And it can only complete if it is not far beyond the budget.
            prop_assert!(n <= budget.saturating_mul(2),
                "{} iterations completed under a budget of {}", n, budget);
        }
    }
}
