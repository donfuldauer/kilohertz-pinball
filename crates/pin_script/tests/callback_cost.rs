//! KP-047 (#128) independent spec tests, Lua side (SPEC 4.2, D5, D9, D25).
//!
//! The per-callback cost of the Lua backend was optimised (pending tilt
//! releases tracked in O(1)); these black-box tests pin the externally
//! visible behaviour that must NOT change:
//!
//! * every coil still held when the script faults is released exactly once,
//!   in ascending coil order within each drain, however little room the
//!   command queue has, and the backend then goes quiet (A1);
//! * a hold issued for a coil whose tilt release is still pending is not
//!   undone by the old drain (A2);
//! * a state whose instruction budget was exhausted, and is then reloaded or
//!   kept, gets its full normal budget back (A3);
//! * the command stream is a pure function of source, seed and input (A4).
//!
//! Only the public API is used (`LuaScriptBackend`, `TableScriptBackend`,
//! `HostCommandQueue`); nothing is timed (R6).

mod spec_support;

use std::collections::{BTreeMap, BTreeSet};

use pin_core::{CoilId, ScriptError, SwitchEdge, SwitchId, TableScriptBackend};
use pin_script::lua::LuaScriptBackend;
use proptest::prelude::*;
use spec_support::{Host, backend_with, cfg, score};

const MIB: usize = 16 * 1024 * 1024;
const BUDGET: u32 = 20_000;

/// Capacity of `HostCommandQueue::coil_holds` (pin_core contract).
const HOLD_CAP: usize = 32;

/// Coil ids every A1 sequence draws from (word and bitset boundaries, a
/// large id and the largest `u16`), extended by random ids per case.
const FIXED_COILS: [u16; 7] = [0, 1, 63, 64, 65, 1000, 65535];

/// Filler holds the host itself queued before the backend runs: they use
/// coil ids no script in this file holds, so they are easy to tell apart.
fn prefill(h: &mut Host, n: usize) {
    h.cmd.clear();
    for i in 0..n {
        h.cmd
            .hold_coil(CoilId(40_000 + i as u16), true)
            .expect("filler fits");
    }
}

/// Runs one tick with `fill` filler holds already in the queue; returns the
/// hold commands the backend appended (the filler must be untouched).
fn tick_with_fill(b: &mut dyn TableScriptBackend, h: &mut Host, fill: usize) -> Vec<(u16, bool)> {
    prefill(h, fill);
    let filler = h.cmd.coil_holds[..fill].to_vec();
    h.view.tick += 1;
    b.on_tick(&mut h.ctx());
    assert!(
        h.cmd.coil_holds.len() >= fill,
        "the host's own holds were dropped: {:?}",
        h.cmd.coil_holds
    );
    assert_eq!(
        &h.cmd.coil_holds[..fill],
        &filler[..],
        "the host's own holds were changed"
    );
    h.cmd.coil_holds[fill..]
        .iter()
        .map(|&(c, on)| (c.0, on))
        .collect()
}

/// Lua source of an A1 sequence: tick `n` replays `ticks[n - 1]` (hold
/// commands); tick `ticks.len() + 1` replays `fault_ops` and then runs
/// `tail`, which faults.
fn a1_source(ticks: &[Vec<(u16, bool)>], fault_ops: &[(u16, bool)], tail: &str) -> String {
    let row = |ops: &[(u16, bool)]| {
        let items: Vec<String> = ops
            .iter()
            .map(|&(c, on)| format!("{{{c},{}}}", u8::from(on)))
            .collect();
        format!("{{{}}}", items.join(","))
    };
    let mut rows: Vec<String> = ticks.iter().map(|t| row(t)).collect();
    rows.push(row(fault_ops));
    format!(
        "local ops = {{{}}}
         local fault_at = {}
         local n = 0
         function on_tick()
           n = n + 1
           local t = ops[n]
           if t then
             for i = 1, #t do pinball.hold_coil(t[i][1], t[i][2] == 1) end
           end
           if n == fault_at then {tail} end
         end",
        rows.join(","),
        ticks.len() + 1
    )
}

/// A1 strategy: a pool of coil ids, then hold/release sequences over it.
fn a1_case() -> impl Strategy<Value = A1Case> {
    proptest::collection::vec(any::<u16>(), 0..4).prop_flat_map(|extra| {
        let mut pool: Vec<u16> = FIXED_COILS.to_vec();
        pool.extend(extra);
        let n = pool.len();
        let op = (0..n, any::<bool>());
        (
            Just(pool),
            proptest::collection::vec(proptest::collection::vec(op.clone(), 0..9), 1..10),
            proptest::collection::vec(op, 0..7),
            any::<bool>(),
            0..=HOLD_CAP - 6,
            proptest::collection::vec(HOLD_CAP - 8..=HOLD_CAP, 0..24),
        )
            .prop_map(|(pool, ticks, fault_ops, loop_tail, fault_fill, fills)| {
                let pick = |ops: Vec<(usize, bool)>| -> Vec<(u16, bool)> {
                    ops.into_iter().map(|(i, on)| (pool[i], on)).collect()
                };
                A1Case {
                    ticks: ticks.into_iter().map(pick).collect(),
                    fault_ops: pick(fault_ops),
                    loop_tail,
                    fault_fill,
                    fills,
                }
            })
    })
}

#[derive(Debug, Clone)]
struct A1Case {
    /// Hold commands of each healthy tick (at most 8, the queue is empty).
    ticks: Vec<Vec<(u16, bool)>>,
    /// Hold commands of the faulting callback before it faults (at most 6).
    fault_ops: Vec<(u16, bool)>,
    /// `while true do end` (budget) instead of `error('x')`.
    loop_tail: bool,
    /// Filler holds before the faulting tick: its own holds always fit.
    fault_fill: usize,
    /// Filler holds before each later tick (room = 32 - fill, may be 0); once
    /// these run out every tick has room for 4.
    fills: Vec<usize>,
}

/// The reference model (tests/spec_faults.rs semantics): tilt releases every
/// coil whose last hold command before the faulting callback was "on", plus
/// every coil the faulting callback itself touched (its commands are rolled
/// back, so the host may have seen neither; releasing it is the safe side:
/// "release_in_the_faulting_callback_is_not_lost",
/// "coil_held_in_the_faulting_callback_ends_up_released").
fn a1_expected(case: &A1Case) -> Vec<u16> {
    let mut held: BTreeMap<u16, bool> = BTreeMap::new();
    for ops in &case.ticks {
        for &(c, on) in ops {
            held.insert(c, on);
        }
    }
    let mut set: BTreeSet<u16> = held
        .iter()
        .filter(|&(_, &on)| on)
        .map(|(&c, _)| c)
        .collect();
    set.extend(case.fault_ops.iter().map(|&(c, _)| c));
    set.into_iter().collect()
}

/// Checks one A1 case against the reference model; `Err` describes a breach.
fn run_a1(case: &A1Case) -> Result<(), TestCaseError> {
    let tail = if case.loop_tail {
        "while true do end"
    } else {
        "error('x')"
    };
    let src = a1_source(&case.ticks, &case.fault_ops, tail);
    let mut b = backend_with(src.as_bytes(), cfg(MIB, BUDGET));
    let mut h = Host::new(47);

    for ops in &case.ticks {
        let emitted = tick_with_fill(&mut b, &mut h, 0);
        prop_assert!(!b.is_faulted(), "healthy tick faulted");
        prop_assert_eq!(&emitted, ops, "a healthy tick forwards its holds in order");
    }
    let expected = a1_expected(case);

    // The faulting tick, then the later ticks: enough of them for every
    // pending release to fit with room for 4 per tick, plus 3 quiet ones.
    let fills: Vec<usize> = std::iter::once(case.fault_fill)
        .chain(case.fills.iter().copied())
        .chain(std::iter::repeat_n(
            HOLD_CAP - 4,
            expected.len().div_ceil(4) + 3,
        ))
        .collect();
    let mut released: BTreeMap<u16, u32> = BTreeMap::new();
    let mut per_tick = Vec::with_capacity(fills.len());
    for (t, &fill) in fills.iter().enumerate() {
        let emitted = tick_with_fill(&mut b, &mut h, fill);
        prop_assert!(b.is_faulted(), "the faulting tick must fault");
        prop_assert!(
            emitted.iter().all(|&(_, on)| !on),
            "tick {}: tilt only releases: {:?}",
            t,
            emitted
        );
        prop_assert!(
            emitted.windows(2).all(|w| w[0].0 < w[1].0),
            "tick {}: each drain is in ascending coil order: {:?}",
            t,
            emitted
        );
        let pending = expected.len() - released.len();
        // No lost room: a drain fills the room it has (or empties the set).
        prop_assert_eq!(
            emitted.len(),
            pending.min(HOLD_CAP - fill),
            "tick {}: drain size (pending {}, room {})",
            t,
            pending,
            HOLD_CAP - fill
        );
        for &(c, _) in &emitted {
            *released.entry(c).or_default() += 1;
        }
        // Tilt queues nothing but coil releases.
        let mut rest = h.cmd.clone();
        rest.coil_holds.clear();
        prop_assert!(rest.is_empty(), "tilt queued other commands: {:?}", rest);
        per_tick.push(emitted);
    }
    let got: Vec<u16> = released.keys().copied().collect();
    prop_assert_eq!(
        &got,
        &expected,
        "the set released by tilt is the set held at the fault: {:?}",
        per_tick
    );
    prop_assert!(
        released.values().all(|&n| n == 1),
        "every pending release is emitted exactly once: {:?}",
        per_tick
    );
    prop_assert!(
        per_tick.iter().rev().take(3).all(Vec::is_empty),
        "the backend goes quiet once every release is out: {:?}",
        per_tick
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// KP-047 A1, SPEC 4.2, D25: after any hold/release sequence and a fault
    /// (error or exhausted budget), each coil held at the fault is released
    /// exactly once, in ascending order within each drain, even when the
    /// queue has little or no free room; afterwards no hold is emitted.
    #[test]
    fn kp047_spec_4_2_tilt_releases_each_held_coil_exactly_once(case in a1_case()) {
        run_a1(&case)?;
    }
}

/// KP-047 A1, SPEC 4.2: the fixed-id sequence (every boundary id held, two
/// released again) under a queue with room for one release per tick, and
/// with a full queue for a while.
#[test]
fn kp047_spec_4_2_boundary_ids_drain_one_per_tick() {
    let all: Vec<(u16, bool)> = FIXED_COILS.iter().map(|&c| (c, true)).collect();
    for loop_tail in [false, true] {
        let case = A1Case {
            ticks: vec![all.clone(), vec![(64, false), (1000, false)]],
            fault_ops: vec![(64, true), (1, false), (7, true)],
            loop_tail,
            fault_fill: HOLD_CAP - 3,
            fills: vec![HOLD_CAP - 1; 12],
        };
        assert_eq!(
            a1_expected(&case),
            vec![0, 1, 7, 63, 64, 65, 65535],
            "model"
        );
        run_a1(&case).unwrap_or_else(|e| panic!("{e}"));
        let mut full = case.clone();
        full.fills = [HOLD_CAP; 5].into_iter().chain([HOLD_CAP - 2; 6]).collect();
        run_a1(&full).unwrap_or_else(|e| panic!("{e}"));
    }
}

// ---------------------------------------------------------------------------
// A2: a re-hold during a pending drain.

/// Holds coils 1..=40 over two ticks, then faults; with a queue of 32 holds
/// the fault releases 32 coils and leaves 8 pending.
const HOLD_40_THEN_FAULT: &[u8] = b"local n = 0
function on_tick()
  n = n + 1
  if n == 1 then for i = 1, 32 do pinball.hold_coil(i, true) end
  elseif n == 2 then for i = 33, 40 do pinball.hold_coil(i, true) end
  else error('x') end
end";

/// Runs `HOLD_40_THEN_FAULT` up to the fault; returns the 32 coils released.
fn fault_with_8_pending(b: &mut dyn TableScriptBackend, h: &mut Host) -> Vec<(u16, bool)> {
    assert_eq!(tick_with_fill(b, h, 0).len(), 32);
    assert_eq!(tick_with_fill(b, h, 0).len(), 8);
    let first = tick_with_fill(b, h, 0);
    assert_eq!(first.len(), 32, "the fault tick fills the queue");
    first
}

/// KP-047 A2, SPEC 4.2: a hold issued (after a successful reload) for a coil
/// whose tilt release is still pending is never undone by the old drain: once
/// the re-hold has reached the host no release of that coil follows, and
/// every re-hold that fits in the queue leaves the coil held at the end of the
/// callback. Coils not re-held, and coils whose re-hold never reached the
/// host (queue full), are each released exactly once (never stranded).
#[test]
fn kp047_spec_4_2_rehold_of_a_pending_coil_is_not_undone_by_the_old_drain() {
    let rehold_sets: [&[u16]; 4] = [
        &[36],
        &[33, 40],
        &[1, 36],
        &[33, 34, 35, 36, 37, 38, 39, 40],
    ];
    for set in rehold_sets {
        for (via_edge, room) in [
            (false, 32usize),
            (false, 3),
            (false, 1),
            (true, 1),
            (true, 4),
            (true, 32),
        ] {
            let what = format!("set {set:?} edge {via_edge} room {room}");
            let mut b = backend_with(HOLD_40_THEN_FAULT, cfg(MIB, BUDGET));
            let mut h = Host::new(1);
            let mut released: BTreeMap<u16, u32> = BTreeMap::new();
            for (c, on) in fault_with_8_pending(&mut b, &mut h) {
                assert!(!on);
                *released.entry(c).or_default() += 1;
            }
            let calls: String = set
                .iter()
                .map(|c| format!("pinball.hold_coil({c}, true) "))
                .collect();
            let new_src = format!(
                "function on_tick() {} end function on_switch(id, closed) {calls} end",
                if via_edge { "" } else { calls.as_str() }
            );
            b.apply_reload(new_src.as_bytes())
                .expect("valid source reloads");
            assert!(!b.is_faulted());

            let mut reached: BTreeSet<u16> = BTreeSet::new();
            for t in 0..12 {
                // A sim step: optional switch edge, then on_tick, one queue.
                let overflows = b.overflow_count();
                prefill(&mut h, HOLD_CAP - room);
                if via_edge {
                    b.on_switch_edge(SwitchId(3), SwitchEdge::Closed, &mut h.ctx());
                }
                h.view.tick += 1;
                b.on_tick(&mut h.ctx());
                let emitted: Vec<(u16, bool)> = h.cmd.coil_holds[HOLD_CAP - room..]
                    .iter()
                    .map(|&(c, on)| (c.0, on))
                    .collect();
                let fitted = b.overflow_count() == overflows;
                for &(c, on) in &emitted {
                    if on {
                        assert!(set.contains(&c), "{what} tick {t}: {emitted:?}");
                        reached.insert(c);
                    } else {
                        assert!(
                            !reached.contains(&c),
                            "{what} tick {t}: coil {c} released after its re-hold \
                             reached the host: {emitted:?}"
                        );
                        *released.entry(c).or_default() += 1;
                    }
                }
                if fitted {
                    for &c in set {
                        let last = emitted.iter().rev().find(|&&(x, _)| x == c);
                        assert_eq!(
                            last,
                            Some(&(c, true)),
                            "{what} tick {t}: a re-hold that fitted must end held: {emitted:?}"
                        );
                    }
                }
            }
            for c in 1..=40u16 {
                let n = released.get(&c).copied().unwrap_or(0);
                if reached.contains(&c) && c > 32 {
                    assert!(n <= 1, "{what}: coil {c} released {n} times");
                } else {
                    assert_eq!(n, 1, "{what}: coil {c} released {n} times");
                }
            }
        }
    }
}

/// KP-047 A2, SPEC 4.2: after a reload mid-drain the drain finishes and a
/// later fault of the new script releases what the new script holds; the old
/// pending releases are not repeated.
#[test]
fn kp047_spec_4_2_second_fault_after_a_mid_drain_reload_releases_only_new_holds() {
    let mut b = backend_with(HOLD_40_THEN_FAULT, cfg(MIB, BUDGET));
    let mut h = Host::new(1);
    fault_with_8_pending(&mut b, &mut h);
    b.apply_reload(
        b"local n = 0
          function on_tick()
            n = n + 1
            if n == 1 then pinball.hold_coil(35, true) pinball.hold_coil(64, true)
            elseif n == 2 then while true do end end
          end",
    )
    .unwrap();
    let mut all = tick_with_fill(&mut b, &mut h, 0);
    assert!(!b.is_faulted());
    assert!(
        all.contains(&(35, true)) && all.contains(&(64, true)),
        "{all:?}"
    );
    for _ in 0..6 {
        all.extend(tick_with_fill(&mut b, &mut h, HOLD_CAP - 1));
    }
    assert!(b.is_faulted());
    let count = |c: u16| all.iter().filter(|&&x| x == (c, false)).count();
    for c in [33u16, 34, 36, 37, 38, 39, 40, 64] {
        assert_eq!(count(c), 1, "coil {c}: {all:?}");
    }
    // 35: perhaps once by the old drain before its re-hold, once by the new fault.
    assert!((1..=2).contains(&count(35)), "coil 35: {all:?}");
    let last35 = all.iter().rev().find(|e| e.0 == 35);
    assert_eq!(last35, Some(&(35, false)), "{all:?}");
    for _ in 0..3 {
        assert!(tick_with_fill(&mut b, &mut h, 0).is_empty(), "no repeats");
    }
}

// ---------------------------------------------------------------------------
// A3: budget recovery.

/// `on_switch(id)` runs a counted loop of `id * 8` iterations and reports the
/// sum; switch 0 never returns. `on_tick` emits a deterministic stream.
const RECOVER: &str = "local n = 0
function on_switch(id, closed)
  if id == 0 then while true do end end
  local s = 0
  for i = 1, id * 8 do s = s + i end
  pinball.set_score(1, s)
end
function on_tick()
  n = n + 1
  pinball.set_score(2, n * n)
  if n % 3 == 0 then pinball.pulse_coil(n % 7, n % 50) end
  pinball.hold_coil(n % 5, n % 2 == 0)
  if n % 4 == 0 then pinball.set_lamp(n % 9, 1) end
end";

fn deliver(b: &mut dyn TableScriptBackend, h: &mut Host, id: u16) {
    h.cmd.clear();
    b.on_switch_edge(SwitchId(id), SwitchEdge::Closed, &mut h.ctx());
}

/// The largest `id` whose `id * 8`-iteration loop completes on a fresh
/// backend under `BUDGET` (bisection; each probe on its own backend).
fn calibrate() -> u16 {
    let completes = |id: u16| {
        let mut b = backend_with(RECOVER.as_bytes(), cfg(MIB, BUDGET));
        let mut h = Host::new(0);
        deliver(&mut b, &mut h, id);
        !b.is_faulted()
    };
    let (mut lo, mut hi) = (1u16, 4096u16);
    assert!(completes(lo) && !completes(hi), "calibration range");
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if completes(mid) {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    assert!(
        lo >= 50,
        "a budget of {BUDGET} runs only {lo} * 8 iterations"
    );
    lo
}

/// 80 % succeeds with the right sum, 50 ticks match `fresh` tick for tick,
/// then 120 % faults with `BudgetExceeded` on both.
fn assert_full_budget(
    a: &mut LuaScriptBackend,
    fresh: &mut LuaScriptBackend,
    max_id: u16,
    what: &str,
) {
    let id80 = (u32::from(max_id) * 8 / 10) as u16;
    let id120 = (u32::from(max_id) * 12 / 10) as u16;
    let n = i64::from(id80) * 8;
    let (mut ha, mut hb) = (Host::new(9), Host::new(9));
    for (b, h) in [(&mut *a, &mut ha), (&mut *fresh, &mut hb)] {
        deliver(b, h, id80);
        assert!(!b.is_faulted(), "{what}: 80 % of the budget must succeed");
        assert_eq!(score(&h.cmd, 0), Some(n * (n + 1) / 2), "{what}");
    }
    for t in 0..50 {
        ha.tick(a);
        hb.tick(fresh);
        assert_eq!(
            ha.cmd, hb.cmd,
            "{what}: tick {t} differs from a fresh backend"
        );
    }
    let faults = a.fault_count();
    for (b, h) in [(&mut *a, &mut ha), (&mut *fresh, &mut hb)] {
        deliver(b, h, id120);
        assert!(b.is_faulted(), "{what}: 120 % of the budget must fail");
        assert_eq!(b.last_error(), Some(&ScriptError::BudgetExceeded), "{what}");
    }
    assert_eq!(a.fault_count(), faults + 1, "{what}");
    // The tilt releases of both match too.
    assert_eq!(ha.cmd, hb.cmd, "{what}: tilt differs");
    for t in 0..5 {
        ha.tick(a);
        hb.tick(fresh);
        assert_eq!(ha.cmd, hb.cmd, "{what}: post-fault tick {t}");
    }
}

/// KP-047 A3, SPEC 4.2, D25: a faulted (budget-exhausted) backend recovered
/// by a reload of the same source gets the full per-callback budget again,
/// exactly like a fresh backend (80 % succeeds, 120 % fails, identical
/// command streams over 50 ticks).
#[test]
fn kp047_spec_4_2_budget_exhausted_then_reloaded_recovers_the_full_budget() {
    let max_id = calibrate();
    for exhaust_in_tick in [false, true] {
        let src = RECOVER;
        let mut a = backend_with(src.as_bytes(), cfg(MIB, BUDGET));
        let mut h = Host::new(1);
        if exhaust_in_tick {
            a.apply_reload(b"function on_tick() while true do end end")
                .unwrap();
            h.tick(&mut a);
        } else {
            deliver(&mut a, &mut h, 0);
        }
        assert!(a.is_faulted());
        assert_eq!(a.last_error(), Some(&ScriptError::BudgetExceeded));
        a.apply_reload(src.as_bytes()).expect("reload");
        assert!(!a.is_faulted());
        let mut fresh = backend_with(src.as_bytes(), cfg(MIB, BUDGET));
        assert_full_budget(&mut a, &mut fresh, max_id, "fault + reload");
    }
}

/// KP-047 A3, SPEC 4.2, D25: a budget exhausted inside a callback that does
/// not fault the backend (a save hook that never returns, during a reload)
/// leaves the kept state with its full budget for the next callback.
#[test]
fn kp047_spec_4_2_state_kept_after_an_exhausted_save_hook_recovers_the_full_budget() {
    let max_id = calibrate();
    let old = format!("{RECOVER}\nfunction on_save_state() while true do end end");
    let mut a = backend_with(old.as_bytes(), cfg(MIB, BUDGET));
    match a.apply_reload(RECOVER.as_bytes()) {
        // The new source was installed without state: compare with a fresh
        // backend of the new source.
        Ok(()) => {
            assert!(!a.is_faulted());
            let mut fresh = backend_with(RECOVER.as_bytes(), cfg(MIB, BUDGET));
            assert_full_budget(&mut a, &mut fresh, max_id, "save hook exhausted, reload ok");
        }
        // The old state was kept: it is healthy and has its full budget.
        Err(_) => {
            assert!(!a.is_faulted());
            let mut fresh = backend_with(old.as_bytes(), cfg(MIB, BUDGET));
            assert_full_budget(&mut a, &mut fresh, max_id, "save hook exhausted, old kept");
        }
    }
}

/// KP-047 A3, SPEC 4.2, D25: a restore hook that exhausts the new state's
/// budget rejects the reload; the kept old state still has its full budget,
/// and so does the next successfully reloaded state.
#[test]
fn kp047_spec_4_2_rejected_reload_with_an_exhausted_restore_hook_keeps_the_full_budget() {
    let max_id = calibrate();
    let old = format!("{RECOVER}\nfunction on_save_state() return 1 end");
    let mut a = backend_with(old.as_bytes(), cfg(MIB, BUDGET));
    let bad = format!("{RECOVER}\nfunction on_restore_state(s) while true do end end");
    let r = a.apply_reload(bad.as_bytes());
    assert!(!a.is_faulted(), "a rejected or accepted reload is no fault");
    let mut fresh = if r.is_err() {
        backend_with(old.as_bytes(), cfg(MIB, BUDGET))
    } else {
        backend_with(bad.as_bytes(), cfg(MIB, BUDGET))
    };
    assert_full_budget(&mut a, &mut fresh, max_id, "exhausted restore hook");
}

/// KP-047 A3, SPEC 4.2, D25: repeated exhaust/reload cycles never erode the
/// budget (the per-callback budget is not cumulative).
#[test]
fn kp047_spec_4_2_repeated_exhaustion_never_erodes_the_budget() {
    let max_id = calibrate();
    let mut a = backend_with(RECOVER.as_bytes(), cfg(MIB, BUDGET));
    let mut h = Host::new(1);
    for cycle in 0..20 {
        deliver(&mut a, &mut h, 0);
        assert!(a.is_faulted(), "cycle {cycle}");
        a.apply_reload(RECOVER.as_bytes()).unwrap();
        deliver(&mut a, &mut h, max_id * 8 / 10);
        assert!(!a.is_faulted(), "cycle {cycle}: 80 % failed after recovery");
    }
    let mut fresh = backend_with(RECOVER.as_bytes(), cfg(MIB, BUDGET));
    a.apply_reload(RECOVER.as_bytes()).unwrap();
    assert_full_budget(&mut a, &mut fresh, max_id, "20 cycles");
}

// ---------------------------------------------------------------------------
// A4: determinism.

const DET: &str = "local n = 0
local total = 0
function on_tick()
  n = n + 1
  local r = pinball.random(1, 100)
  total = total + r
  pinball.set_score(1, total)
  if r > 60 then pinball.hold_coil(r % 70, true) end
  if r < 30 then pinball.hold_coil(r % 70, false) end
  if r % 11 == 0 then pinball.pulse_coil(r % 13, r) end
  if math.random(4) == 1 then pinball.set_lamp(r % 20, 1) end
end
function on_switch(id, closed)
  if id == 200 then while true do end end
  if id == 201 then error('boom') end
  if closed then pinball.hold_coil(id % 70, true) else pinball.hold_coil(id % 70, false) end
  pinball.set_score(2, id * 1000 + pinball.random(0, 999))
end";

#[derive(Debug, Clone)]
enum Step {
    Tick(usize),
    Edge(u16, bool),
    Fault(bool),
    Reload(bool),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        6 => (0usize..=32).prop_map(Step::Tick),
        3 => (0u16..80, any::<bool>()).prop_map(|(s, c)| Step::Edge(s, c)),
        1 => any::<bool>().prop_map(Step::Fault),
        1 => any::<bool>().prop_map(Step::Reload),
    ]
}

fn run_det(steps: &[Step], seed: u64) -> Vec<String> {
    let alt = DET.replace("r > 60", "r > 40");
    let mut b = backend_with(DET.as_bytes(), cfg(MIB, BUDGET));
    let mut h = Host::new(seed);
    let mut out = Vec::with_capacity(steps.len());
    for s in steps {
        match *s {
            Step::Tick(fill) => {
                tick_with_fill(&mut b, &mut h, fill);
            }
            Step::Edge(sw, closed) => {
                h.edge(&mut b, sw, closed);
            }
            Step::Fault(lp) => deliver(&mut b, &mut h, if lp { 200 } else { 201 }),
            Step::Reload(other) => {
                h.cmd.clear();
                let src = if other {
                    alt.as_bytes()
                } else {
                    DET.as_bytes()
                };
                b.apply_reload(src).expect("reload");
            }
        }
        out.push(format!(
            "{:?} faulted={} faults={} overflow={}",
            h.cmd,
            b.is_faulted(),
            b.fault_count(),
            b.overflow_count()
        ));
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// KP-047 A4, D9: two backends from the same source and seed, driven
    /// through the same tick/switch/fault/reload sequence, produce
    /// byte-identical command-queue dumps every step.
    #[test]
    fn kp047_d9_same_source_seed_and_input_give_identical_streams(
        steps in proptest::collection::vec(step(), 1..120),
        seed in any::<u64>(),
    ) {
        let a = run_det(&steps, seed);
        let b = run_det(&steps, seed);
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            prop_assert_eq!(x, y, "step {} ({:?})", i, steps[i]);
        }
    }
}

/// KP-047 A4, D9: a fixed long sequence with a mid-sequence budget fault and
/// reload is reproduced exactly, and the sequence does exercise the fault.
#[test]
fn kp047_d9_fixed_sequence_with_a_mid_sequence_fault_is_reproducible() {
    let mut steps = Vec::new();
    for i in 0..300usize {
        steps.push(match i {
            100 => Step::Fault(true),
            130 => Step::Reload(false),
            200 => Step::Fault(false),
            201 => Step::Reload(true),
            _ if i % 5 == 0 => Step::Edge((i % 80) as u16, i % 10 == 0),
            _ => Step::Tick(i * 7 % 33),
        });
    }
    let a = run_det(&steps, 0xC0FFEE);
    let b = run_det(&steps, 0xC0FFEE);
    assert_eq!(a, b);
    assert!(a[100].contains("faulted=true"), "{}", a[100]);
    assert!(a[131].contains("faulted=false"), "{}", a[131]);
    let c = run_det(&steps, 0xC0FFEF);
    assert_ne!(a, c, "the seed reaches the script");
}
