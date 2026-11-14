//! Spec: determinism across processes. Hash seeds, sort pivots and addresses
//! must not leak into command queues: the same script, seed and inputs give
//! identical queues in separately spawned processes (each with its own ASLR
//! layout and, on stock Lua, its own random hash seed).
//!
//! The parent tests re-run this very test executable with `PIN_SPEC_CHILD`
//! set; the `child_emit` test then prints its queues between markers.

mod spec_support;

use pin_core::{CoilId, HostCommandQueue};
use pin_script::NameTables;
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use spec_support::Host;
use std::process::Command;

const CHILD_ENV: &str = "PIN_SPEC_CHILD";
const BEGIN: &str = "PIN_SPEC_BEGIN";
const END: &str = "PIN_SPEC_END";

/// Draws one random pulse per coil, in `pairs` order.
const PAIRS_SCRIPT: &str = "
function on_tick()
  local n = 0
  for name, id in pairs(pinball.coil) do
    n = n + 1
    if n <= 30 then pinball.pulse_coil(id, pinball.random(1000)) end
  end
  for name, id in pairs(pinball.sw) do
    pinball.set_lamp(id, math.random())
  end
  -- A script-built string-keyed table, iterated in hash order.
  local t = {}
  for i = 1, 40 do t['key_' .. (i * 7919 % 101)] = i end
  local k = 0
  for key, v in pairs(t) do
    k = k + 1
    if k <= 8 then pinball.persist_set(v, pinball.random(1000000)) end
  end
  -- A sparse integer-keyed table (hash part, not array part).
  local s = {}
  for i = 1, 40 do s[i * 7919 % 1009 + 5000] = i end
  k = 0
  for key, v in pairs(s) do
    k = k + 1
    if k <= 16 then pinball.play_sound(key, 0.5) end
  end
end";

/// Sorts 150 records with heavy ties; the resulting permutation is reported
/// through the order of lamp commands.
const SORT_SCRIPT: &str = "
function on_tick()
  local t = {}
  for i = 1, 150 do t[i] = { key = (i * 31) % 7, tag = i } end
  table.sort(t, function(a, b) return a.key < b.key end)
  for i = 1, 150 do pinball.set_lamp(t[i].tag, t[i].key / 8) end
  local u = {}
  for i = 1, 120 do u[i] = pinball.random(5) end
  table.sort(u, function(a, b) return a > b end)
  local acc = 0
  for i = 1, 120 do acc = acc * 6 % 1000003 + u[i] end
  pinball.persist_set(0, acc)
end";

/// Tables and functions used as keys hash by address, so their `pairs` order
/// is not required to be reproducible across processes (DECISIONS
/// "Orchestrator — 4.2 pointer-keyed pairs order"). Iteration must still work:
/// every key is visited exactly once and nothing faults.
const TABLE_KEY_SCRIPT: &str = "
local keys = {}
for i = 1, 24 do keys[i] = {} end
for i = 25, 48 do keys[i] = function() return i end end
function on_tick()
  local set = {}
  for i = 1, 48 do set[keys[i]] = i end
  local seen, n, dup, wrong = {}, 0, 0, 0
  for k, v in pairs(set) do
    n = n + 1
    if seen[v] then dup = dup + 1 end
    seen[v] = true
    if keys[v] ~= k then wrong = wrong + 1 end
  end
  local missing = 0
  for i = 1, 48 do if not seen[i] then missing = missing + 1 end end
  pinball.persist_set(0, n)
  pinball.persist_set(1, dup)
  pinball.persist_set(2, wrong)
  pinball.persist_set(3, missing)
end";

fn script(mode: &str) -> &'static str {
    match mode {
        "pairs" => PAIRS_SCRIPT,
        "sort" => SORT_SCRIPT,
        other => panic!("unknown mode {other}"),
    }
}

/// Many names, so a hash-order change is practically certain to show.
fn many_names() -> NameTables {
    let mut n = spec_support::names();
    n.coils = (0..40u16)
        .map(|i| (format!("coil_{}_{}", i * 37 % 41, i), CoilId(i)))
        .collect();
    n.switches = (0..60u16)
        .map(|i| (format!("sw_{i}_{}", i * 13 % 17), pin_core::SwitchId(i)))
        .collect();
    n
}

/// Runs `mode` for 20 ticks under seed 99 and renders every queue.
fn transcript(mode: &str) -> String {
    let mut b = LuaScriptBackend::new(script(mode).as_bytes(), &many_names(), LuaConfig::default())
        .unwrap_or_else(|e| panic!("{mode}: {e}"));
    let mut h = Host::new(99);
    let mut out = String::new();
    for _ in 0..20 {
        h.tick(&mut b);
        let q: &HostCommandQueue = &h.cmd;
        out.push_str(&format!("{q:?}\n"));
    }
    assert!(!b.is_faulted(), "{mode}: the script faulted");
    assert_eq!(b.overflow_count(), 0, "{mode}");
    out
}

/// Child side: inert unless spawned by a parent test.
#[test]
fn child_emit() {
    let Ok(mode) = std::env::var(CHILD_ENV) else {
        return;
    };
    let t = transcript(&mode);
    println!("\n{BEGIN}\n{t}{END}");
}

/// Spawns this executable as a fresh process and returns the child's transcript.
fn spawn(mode: &str) -> String {
    let exe = std::env::current_exe().expect("test executable path");
    let out = Command::new(exe)
        .args(["--exact", "child_emit", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, mode)
        .output()
        .expect("spawn child test process");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        out.status.success(),
        "child process failed for {mode}:\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let begin = stdout.find(BEGIN).expect("child printed no transcript") + BEGIN.len();
    let end = stdout.rfind(END).expect("child transcript not terminated");
    stdout[begin..end].trim().to_owned()
}

fn assert_reproducible(mode: &str) {
    let local = transcript(mode).trim().to_owned();
    assert!(
        local.contains("CoilId") || local.contains("LampId"),
        "empty transcript"
    );
    for run in 0..4 {
        let child = spawn(mode);
        assert_eq!(
            child, local,
            "{mode}: process {run} produced different command queues"
        );
    }
}

#[test]
fn pairs_order_and_random_draws_are_identical_across_processes() {
    assert_reproducible("pairs");
}

#[test]
fn table_sort_with_ties_is_identical_across_processes() {
    assert_reproducible("sort");
}

#[test]
fn pairs_over_table_and_function_keys_visits_every_key_exactly_once() {
    let mut b = LuaScriptBackend::new(
        TABLE_KEY_SCRIPT.as_bytes(),
        &many_names(),
        LuaConfig::default(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let mut h = Host::new(99);
    for _ in 0..20 {
        h.tick(&mut b);
        assert!(!b.is_faulted());
        assert_eq!(spec_support::persisted(&h.cmd, 0), Some(48), "keys visited");
        assert_eq!(
            spec_support::persisted(&h.cmd, 1),
            Some(0),
            "keys visited twice"
        );
        assert_eq!(
            spec_support::persisted(&h.cmd, 2),
            Some(0),
            "key/value mismatches"
        );
        assert_eq!(
            spec_support::persisted(&h.cmd, 3),
            Some(0),
            "keys never visited"
        );
    }
}

#[test]
fn pairs_order_is_identical_within_one_process_and_across_backends() {
    for mode in ["pairs", "sort"] {
        assert_eq!(transcript(mode), transcript(mode), "{mode}");
    }
}

#[test]
fn the_pairs_transcript_actually_depends_on_iteration() {
    // Guards the fixture: every coil id is pulsed exactly once per tick.
    let mut b = LuaScriptBackend::new(PAIRS_SCRIPT.as_bytes(), &many_names(), LuaConfig::default())
        .unwrap_or_else(|e| panic!("{e}"));
    let mut h = Host::new(99);
    h.tick(&mut b);
    let mut ids: Vec<u16> = h.cmd.coil_pulses.iter().map(|&(c, _)| c.0).collect();
    assert_eq!(ids.len(), 30);
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), 30, "pairs(pinball.coil) repeated a name");
    assert_eq!(
        h.cmd.lamps.len(),
        60,
        "pairs(pinball.sw) must yield all 60 switches"
    );
    assert_eq!(h.cmd.persist_writes.len(), 8);
    assert_eq!(
        h.cmd.sounds.len(),
        16,
        "integer-keyed pairs must yield entries"
    );
}
