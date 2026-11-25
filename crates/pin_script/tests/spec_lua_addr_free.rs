//! Spec (KP-068, D9 amended, REQ-WEB-005 prerequisite): the vendored Lua's
//! table iteration order (KHZ 34) and API string cache (KHZ 37) no longer
//! depend on memory addresses.
//!
//! Black-box evidence only, through the public `LuaScriptBackend` API:
//!
//! * `pairs` order over tables keyed by tables, Lua closures, C closures,
//!   light C functions and full userdata is identical in separately spawned
//!   processes (each with its own ASLR layout) and in two states of one
//!   process built under different Rust-side heap noise, including after
//!   garbage is created and collected between ticks (address reuse).
//! * Table semantics are unchanged (every key once, lookups survive rehash
//!   and GC, deletion during traversal, mixed key types, identity keys, dead
//!   keys never match a new object).
//! * The string cache is indexed by content: budget units (the smallest
//!   `instruction_budget` that does not fault) and `used_memory()` of the
//!   long-`__name` probe are identical across processes, heap layouts and
//!   creation orders.
//! * Fixed-seed command streams stay identical across processes.
//!
//! The order itself is an implementation detail; nothing here asserts a
//! specific order, only run-to-run equality (plus one regression pin).
//!
//! The parent tests re-run this test executable with `PIN_ADDR_FREE_CHILD`
//! set; `child_emit` then prints its transcript between markers.

mod spec_support;

use pin_script::lua::{LuaConfig, LuaScriptBackend};
use proptest::prelude::*;
use spec_support::{Host, cfg, names, persisted};
use std::collections::BTreeSet;
use std::hint::black_box;
use std::process::Command;

const CHILD_ENV: &str = "PIN_ADDR_FREE_CHILD";
const BEGIN: &str = "PIN_ADDR_FREE_BEGIN";
const END: &str = "PIN_ADDR_FREE_END";

/// Ticks run by the order and command transcripts.
const TICKS: usize = 8;

// ---------------------------------------------------------------------------
// Scripts
// ---------------------------------------------------------------------------

/// D9 / KHZ 34: `pairs` order over address-identity keys, folded into one
/// integer digest per section and tick. Garbage made at the end of every tick
/// frees earlier keys, so later ticks create keys in reused memory. Result
/// slot 3s holds the digest of section s, 3s+1 its visit count and 3s+2 its
/// failed lookups (read back with [`order_results`]).
const ORDER_SCRIPT: &str = r"
local M = 2305843009213693951
local function fold(h, v) return (h * 1000003 + v) % M end
local function digest(t)
  local h, n, bad = 17, 0, 0
  for k, v in pairs(t) do
    h = fold(h, v); n = n + 1
    if t[k] ~= v then bad = bad + 1 end
  end
  return h, n, bad
end
-- Results are stored in R and paged out by on_switch(page): the persist
-- queue holds 8 writes per callback.
local R = {}
function on_switch(page, closed)
  for j = 0, 7 do
    local v = R[page * 8 + j]
    if v ~= nil then pinball.persist_set(j, v) end
  end
end
local function emit(s, t)
  local h, n, bad = digest(t)
  R[s * 3], R[s * 3 + 1], R[s * 3 + 2] = h, n, bad
end
local live = {}
local n = 0
function on_tick()
  n = n + 1
  local kt, ft, cc, ut, lc, mixed = {}, {}, {}, {}, {}, {}
  for i = 1, 48 do local k = {} kt[k] = i live[(n * 48 + i) % 96 + 1] = k end
  for i = 1, 24 do ft[function() return i end] = i end
  -- C closures (host API functions) as keys
  local api = {pinball.pulse_coil, pinball.set_lamp, pinball.persist_set,
    pinball.random, pinball.play_sound, pinball.set_score, pinball.switch,
    pinball.tick}
  for i, f in ipairs(api) do cc[f] = i end
  for i = 1, 12 do local _, e = pcall(pinball.pulse_coil, 'bad', i) ut[e] = i end
  -- light C functions as keys
  local cf = {string.format, string.rep, string.sub, string.byte, table.insert,
    table.concat, table.unpack, math.floor, math.abs, math.max, pairs, ipairs,
    type, tostring, tonumber, select, rawget, rawequal, next, error, pcall,
    utf8.char}
  for i, f in ipairs(cf) do lc[f] = i end
  for i, f in ipairs(cf) do mixed[f] = 100 + i end
  for i = 1, 16 do
    mixed[{}] = 200 + i
    mixed['s' .. i] = 300 + i
    mixed[i * 1.5] = 400 + i
    mixed[i * 7919] = 500 + i
    mixed[function() return i end] = 600 + i
  end
  mixed[true] = 701; mixed[false] = 702
  emit(1, kt); emit(2, ft); emit(3, cc); emit(4, ut); emit(5, lc); emit(6, mixed)
  -- deletion during traversal, then growth (rehash) of an address-keyed table
  local vh, vn = 17, 0
  for k, v in pairs(kt) do
    vh = fold(vh, v); vn = vn + 1
    if v % 2 == 0 then kt[k] = nil end
  end
  R[21], R[22], R[23] = vh, vn, 0
  for i = 49, 130 do kt[{}] = i end
  emit(8, kt)
  -- garbage: frees this tick's keys over the next collections
  for i = 1, 3000 do local g = {i} end
end";

/// D9 / KHZ 37: 60 objects whose metatable `__name` is a long string (more
/// than 40 bytes); `tostring` pushes the name through the C API, which goes
/// through the string cache. `CREATE` is replaced by the creation loop.
/// [`NameStyle::Suffix`] is the spike probe (names share long prefixes);
/// [`NameStyle::Prefix`] makes names differ in their first bytes, so any
/// content-derived cache index spreads them.
const STRCACHE_TEMPLATE: &str = r"
-- objs is filled with placeholders first so its array part has the same
-- shape whatever order the objects are created in.
local objs = {}
for i = 1, 60 do objs[i] = false end
CREATE
function on_tick()
  local n = 0
  for r = 1, 20 do for i = 1, 60, (r % 7) + 1 do n = n + #tostring(objs[i]) end end
  pinball.persist_set(0, n)
end";

fn strcache_script(style: NameStyle, order: CreationOrder) -> String {
    let make = match style {
        NameStyle::Suffix => "objs[i] = setmetatable({}, {__name = string.rep('N', 41 + i) .. i})",
        NameStyle::Prefix => {
            "objs[i] = setmetatable({}, {__name = i .. '_' .. string.rep('N', 41 + i)})"
        }
    };
    // Every order runs the same bytecode over a permutation literal, so only
    // the creation order (and hence the heap layout) differs.
    let perm: Vec<u32> = match order {
        CreationOrder::Forward => (1..=60).collect(),
        CreationOrder::Reverse => (1..=60).rev().collect(),
        // 37 is coprime with 60: a permutation of 1..60.
        CreationOrder::Strided => (0..60).map(|j| j * 37 % 60 + 1).collect(),
    };
    let perm: Vec<String> = perm.iter().map(u32::to_string).collect();
    let create = format!(
        "local PERM = {{{}}}\nfor j = 1, 60 do local i = PERM[j] {make} end\nPERM = nil",
        perm.join(",")
    );
    STRCACHE_TEMPLATE.replace("CREATE", &create)
}

#[derive(Copy, Clone, Debug)]
enum NameStyle {
    Suffix,
    Prefix,
}

const NAME_STYLES: [NameStyle; 2] = [NameStyle::Suffix, NameStyle::Prefix];

#[derive(Copy, Clone, Debug)]
enum CreationOrder {
    Forward,
    Reverse,
    Strided,
}

/// D9 / §3.4: a fixed-seed script whose command stream depends on random
/// draws and on `pairs` order over object keys.
const COMMANDS_SCRIPT: &str = r"
local held = {}
for i = 1, 30 do held[i] = {} end
function on_tick()
  local set = {}
  for i = 1, 30 do set[held[i]] = i end
  for i = 1, 10 do set[function() return i end] = 30 + i end
  local k = 0
  for key, v in pairs(set) do
    k = k + 1
    if k <= 12 then pinball.pulse_coil(v % 3 == 0 and 1 or 3, pinball.random(1000)) end
    if k <= 2 then pinball.set_lamp(v % 2 == 0 and 2 or 200, v / 64) end
  end
  pinball.persist_set(0, pinball.random(1, 1000000))
  for i = 1, 2000 do local g = {i} end
end";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Rust-side heap noise: live allocations of pseudo-random sizes that shift
/// where the Lua allocator's blocks land. Keep the result alive.
fn heap_noise(seed: u64, count: usize) -> Vec<Vec<u8>> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..count)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let len = 1 + (x % 3000) as usize;
            black_box(vec![(x & 0xff) as u8; len])
        })
        .collect()
}

fn build(src: &str, config: LuaConfig) -> LuaScriptBackend {
    LuaScriptBackend::new(src.as_bytes(), &names(), config).unwrap_or_else(|e| panic!("{e}"))
}

/// Runs `ticks` ticks, interleaving heap noise derived from `noise_seed`
/// (0 = none), and renders every command queue.
fn queue_transcript(src: &str, ticks: usize, noise_seed: u64) -> String {
    let mut keep = heap_noise(noise_seed, if noise_seed == 0 { 0 } else { 64 });
    let mut b = build(src, LuaConfig::default());
    let mut h = Host::new(99);
    let mut out = String::new();
    for t in 0..ticks {
        if noise_seed != 0 {
            keep.extend(heap_noise(noise_seed ^ (t as u64 + 1), 16 + t * 3));
            keep.retain(|v| v.len() % 3 != t % 3);
        }
        h.tick(&mut b);
        out.push_str(&format!("{:?}\n", h.cmd));
        if src == ORDER_SCRIPT {
            out.push_str(&format!("{:?}\n", order_results(&mut h, &mut b)));
        }
    }
    black_box(&keep);
    assert!(!b.is_faulted(), "script faulted: {:?}", b.last_error());
    assert_eq!(b.overflow_count(), 0);
    out
}

/// Result slots of the order script (sections 1..=8, three slots each).
const ORDER_SLOTS: usize = 27;

/// Pages the order script's result slots out through `on_switch(page)`
/// (8 persist writes per callback) after a tick.
fn order_results(h: &mut Host, b: &mut LuaScriptBackend) -> Vec<i64> {
    let mut r = vec![i64::MIN; ORDER_SLOTS];
    for page in 0..ORDER_SLOTS.div_ceil(8) {
        h.case(b, page as u16);
        for &(k, v) in &h.cmd.persist_writes {
            let slot = page * 8 + usize::from(k.0);
            if slot < ORDER_SLOTS {
                r[slot] = v;
            }
        }
    }
    r
}

/// FNV-1a 64 of a transcript.
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// True when the string-cache probe built with `budget` runs its chunk and
/// one tick without a budget fault (fresh backend per probe).
fn strcache_passes(src: &str, budget: u32, noise_seed: u64) -> bool {
    let keep = heap_noise(noise_seed, if noise_seed == 0 { 0 } else { 97 });
    let r = match LuaScriptBackend::new(src.as_bytes(), &names(), cfg(16 << 20, budget)) {
        Err(_) => false,
        Ok(mut b) => {
            let mut h = Host::new(7);
            h.tick(&mut b);
            !b.is_faulted()
        }
    };
    black_box(&keep);
    r
}

/// Budget units of the probe: the smallest `instruction_budget` at which it
/// does not fault (binary search).
fn strcache_units(src: &str, noise_seed: u64) -> u32 {
    let (mut lo, mut hi) = (1u32, 4_000_000u32);
    assert!(
        strcache_passes(src, hi, noise_seed),
        "probe faults even at {hi}"
    );
    assert!(!strcache_passes(src, lo, noise_seed));
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if strcache_passes(src, mid, noise_seed) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    hi
}

/// Units, the per-tick `tostring` length sum, and `used_memory()` after the
/// chunk and after each of 4 ticks.
fn strcache_report(src: &str, noise_seed: u64) -> String {
    let units = strcache_units(src, noise_seed);
    let mut keep = heap_noise(noise_seed, if noise_seed == 0 { 0 } else { 53 });
    let mut b = build(src, LuaConfig::default());
    let mut mem = vec![b.used_memory()];
    let mut sums = Vec::new();
    let mut h = Host::new(7);
    for t in 0..4u64 {
        if noise_seed != 0 {
            keep.extend(heap_noise(noise_seed.wrapping_add(t), 20));
        }
        h.tick(&mut b);
        sums.push(persisted(&h.cmd, 0).expect("probe emitted its sum"));
        mem.push(b.used_memory());
    }
    black_box(&keep);
    assert!(!b.is_faulted());
    format!("units={units} sums={sums:?} mem={mem:?}")
}

fn transcript(mode: &str) -> String {
    match mode {
        "order" => queue_transcript(ORDER_SCRIPT, TICKS, 0),
        "commands" => queue_transcript(COMMANDS_SCRIPT, TICKS, 0),
        "strcache" => NAME_STYLES
            .map(|style| strcache_report(&strcache_script(style, CreationOrder::Forward), 0))
            .join("\n"),
        other => panic!("unknown mode {other}"),
    }
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

fn assert_identical_across_processes(mode: &str, runs: usize) {
    let local = transcript(mode).trim().to_owned();
    for run in 0..runs {
        let child = spawn(mode);
        assert_eq!(
            child, local,
            "D9 {mode}: process {run} differs from the parent process"
        );
    }
}

// ---------------------------------------------------------------------------
// KHZ 34: pairs order
// ---------------------------------------------------------------------------

/// D9 / REQ-WEB-005: `pairs` order over table, closure, C-function and
/// userdata keys is identical in separately spawned processes.
#[test]
fn d9_pairs_order_identical_across_processes() {
    assert_identical_across_processes("order", 3);
}

/// D9 / REQ-WEB-005: the same script in states built under different heap
/// layouts (and with garbage and address reuse between ticks) gives the same
/// `pairs` order.
#[test]
fn d9_pairs_order_independent_of_heap_layout() {
    let reference = queue_transcript(ORDER_SCRIPT, TICKS, 0);
    for noise in [1u64, 2, 3, 0xdead_beef, 77] {
        assert_eq!(
            queue_transcript(ORDER_SCRIPT, TICKS, noise),
            reference,
            "D9: heap noise {noise} changed the pairs order"
        );
    }
}

/// D9: two states alive at the same time, ticked alternately with heap noise
/// between them, give the same `pairs` order.
#[test]
fn d9_pairs_order_identical_for_interleaved_states() {
    let mut keep = Vec::new();
    let mut a = build(ORDER_SCRIPT, LuaConfig::default());
    keep.push(heap_noise(5, 300));
    let mut b = build(ORDER_SCRIPT, LuaConfig::default());
    let (mut ha, mut hb) = (Host::new(99), Host::new(99));
    for t in 0..TICKS {
        ha.tick(&mut a);
        keep.push(heap_noise(t as u64 + 11, 40));
        hb.tick(&mut b);
        assert_eq!(ha.cmd, hb.cmd, "D9: tick {t} differs between states");
        let ra = order_results(&mut ha, &mut a);
        keep.push(heap_noise(t as u64 + 23, 17));
        let rb = order_results(&mut hb, &mut b);
        assert_eq!(ra, rb, "D9: tick {t} pairs order differs between states");
    }
    black_box(&keep);
    assert!(!a.is_faulted() && !b.is_faulted());
}

/// D9 / KHZ 34 regression pin: FNV-1a 64 of the order transcript. This pins
/// the vendored Lua's (deterministic, but unspecified) object-key order; it may
/// change only with a deliberate change to the Lua table implementation.
#[test]
fn d9_pairs_order_digest_regression_pin() {
    let t = queue_transcript(ORDER_SCRIPT, TICKS, 0);
    assert_eq!(
        fnv(&t),
        PAIRS_ORDER_PIN,
        "D9: pairs order digest changed (regression pin)"
    );
}

/// Regression pin, recorded from the first run on the patched tree (KP-068).
/// Change it only together with a deliberate change to the Lua table code.
const PAIRS_ORDER_PIN: u64 = 17_527_598_892_468_026_563;

/// KHZ 34: the order transcript actually exercises every section: each key
/// visited exactly once and found again by lookup.
#[test]
fn d9_pairs_order_script_visits_every_key_once() {
    let mut b = build(ORDER_SCRIPT, LuaConfig::default());
    let mut h = Host::new(99);
    let expected = [
        (1, 48),
        (2, 24),
        (3, 8),
        (4, 12),
        (5, 22),
        (6, 22 + 16 * 5 + 2),
        (7, 48),
        (8, 24 + 82),
    ];
    for t in 0..TICKS {
        h.tick(&mut b);
        assert!(!b.is_faulted(), "{:?}", b.last_error());
        assert_eq!(b.overflow_count(), 0);
        let r = order_results(&mut h, &mut b);
        for (s, n) in expected {
            assert_eq!(r[s * 3 + 1], n, "tick {t} section {s}: keys visited");
            assert_eq!(r[s * 3 + 2], 0, "tick {t} section {s}: failed lookups");
        }
    }
}

// ---------------------------------------------------------------------------
// KHZ 34: table semantics unchanged
// ---------------------------------------------------------------------------

/// Runs `src` for `ticks` ticks and checks that every persist key in
/// `zero_keys` reads 0 and every `(key, value)` in `exact` matches each tick.
fn check_semantics(src: &str, ticks: usize, zero_keys: &[u16], exact: &[(u16, i64)]) {
    let mut b = build(src, LuaConfig::default());
    let mut h = Host::new(1);
    for t in 0..ticks {
        h.tick(&mut b);
        assert!(!b.is_faulted(), "tick {t}: {:?}", b.last_error());
        for &k in zero_keys {
            assert_eq!(persisted(&h.cmd, k), Some(0), "tick {t}: check {k} failed");
        }
        for &(k, v) in exact {
            assert_eq!(persisted(&h.cmd, k), Some(v), "tick {t}: value {k}");
        }
    }
}

/// KHZ 34: lookups of table keys survive growth (48 -> 130 keys) and tens of
/// thousands of further table creations with collected garbage across ticks.
#[test]
fn d9_pairs_semantics_unchanged_lookup_after_rehash_and_gc() {
    let src = r"
local held, index = {}, {}
for i = 1, 48 do held[i] = {} index[held[i]] = i end
local grown = false
function on_tick()
  local bad = 0
  for i = 1, #held do if index[held[i]] ~= i then bad = bad + 1 end end
  pinball.persist_set(0, bad)
  if not grown then
    for i = 49, 130 do held[i] = {} index[held[i]] = i end
    grown = true
  end
  bad = 0
  for i = 1, #held do if index[held[i]] ~= i then bad = bad + 1 end end
  pinball.persist_set(1, bad)
  local n, dup, seen = 0, 0, {}
  for k, v in pairs(index) do
    n = n + 1
    if seen[v] then dup = dup + 1 end
    seen[v] = true
    if held[v] ~= k then dup = dup + 1 end
  end
  pinball.persist_set(2, dup)
  pinball.persist_set(3, n)
  -- garbage: 6000 tables per tick, 36 000 over the run
  for i = 1, 6000 do local g = {i} end
  -- fresh objects never alias a held key
  local alias = 0
  for i = 1, 200 do if index[{}] ~= nil then alias = alias + 1 end end
  pinball.persist_set(4, alias)
end";
    check_semantics(src, 6, &[0, 1, 2, 4], &[(3, 130)]);
}

/// KHZ 34: deleting the current key during traversal neither skips nor
/// repeats the others, for string and object keys.
#[test]
fn d9_pairs_semantics_unchanged_delete_during_traversal() {
    let src = r"
function on_tick()
  for _, objects in ipairs({false, true}) do
    local t, keys = {}, {}
    for i = 1, 50 do
      local k = objects and {} or ('d' .. i)
      keys[i] = k t[k] = i
    end
    local seen, n, dup = {}, 0, 0
    for k, v in pairs(t) do
      n = n + 1
      if seen[v] then dup = dup + 1 end
      seen[v] = true
      if v % 3 == 0 then t[k] = nil end
    end
    local missing, wrong, left = 0, 0, 0
    for i = 1, 50 do
      if not seen[i] then missing = missing + 1 end
      local want = (i % 3 ~= 0) and i or nil
      if t[keys[i]] ~= want then wrong = wrong + 1 end
    end
    for _ in pairs(t) do left = left + 1 end
    local base = objects and 10 or 0
    pinball.persist_set(base + 0, n)
    pinball.persist_set(base + 1, dup + missing + wrong)
    pinball.persist_set(base + 2, left)
  end
end";
    check_semantics(src, 3, &[1, 11], &[(0, 50), (2, 34), (10, 50), (12, 34)]);
}

/// KHZ 34: keys of different types coexist in one table, and distinct
/// tables with equal contents are distinct keys.
#[test]
fn d9_pairs_semantics_unchanged_mixed_types_and_identity() {
    let src = r"
function on_tick()
  local t, ks = {}, {}
  local function put(k, v) t[k] = v ks[#ks + 1] = k end
  for i = 1, 10 do
    put('s' .. i, i)
    put(i + 0.25, 100 + i)
    put(i * 1000003, 200 + i)
    put({1}, 300 + i)            -- equal contents, distinct identity
    put(function() return 1 end, 400 + i)
  end
  put(true, 501) put(false, 502)
  put(string.format, 503) put(math.floor, 504) put(pinball.set_lamp, 505)
  local _, e = pcall(pinball.pulse_coil, 'bad', 1)
  put(e, 506)
  local n, bad, seen = 0, 0, {}
  for k, v in pairs(t) do
    n = n + 1
    if seen[v] then bad = bad + 1 end
    seen[v] = true
  end
  for i = 1, #ks do if t[ks[i]] == nil then bad = bad + 1 end end
  -- a new table with the same contents is not any existing key
  if t[{1}] ~= nil then bad = bad + 1 end
  pinball.persist_set(0, n)
  pinball.persist_set(1, bad)
  pinball.persist_set(2, #ks)
end";
    check_semantics(src, 3, &[1], &[(0, 56), (2, 56)]);
}

/// KHZ 34: a key object that was removed and collected, whose memory is
/// later reused by new objects, is never found under a new object; `next`
/// yields only live keys.
#[test]
fn d9_pairs_semantics_unchanged_dead_key_not_found_under_new_object() {
    let src = r"
local t = {}
local survivors = {}
local round = 0
function on_tick()
  round = round + 1
  -- insert 64 fresh object keys, keep 8, remove the rest
  local fresh = {}
  for i = 1, 64 do local k = {} fresh[i] = k t[k] = round * 100 + i end
  for i = 1, 64 do
    if i % 8 == 0 then survivors[#survivors + 1] = fresh[i] else t[fresh[i]] = nil end
  end
  fresh = nil
  -- garbage so the removed keys are collected and their memory reused
  for i = 1, 4000 do local g = {i} end
  local alias = 0
  for i = 1, 2000 do if rawget(t, {}) ~= nil then alias = alias + 1 end end
  local n, bad = 0, 0
  local live = {}
  for _, k in ipairs(survivors) do live[k] = true end
  for k, v in pairs(t) do
    n = n + 1
    if not live[k] then bad = bad + 1 end
  end
  pinball.persist_set(0, alias)
  pinball.persist_set(1, bad)
  pinball.persist_set(2, n - #survivors)
  pinball.persist_set(3, n)
end";
    let mut b = build(src, LuaConfig::default());
    let mut h = Host::new(1);
    for t in 1..=6i64 {
        h.tick(&mut b);
        assert!(!b.is_faulted(), "{:?}", b.last_error());
        assert_eq!(persisted(&h.cmd, 0), Some(0), "round {t}: dead key aliased");
        assert_eq!(persisted(&h.cmd, 1), Some(0), "round {t}: dead key visited");
        assert_eq!(persisted(&h.cmd, 2), Some(0), "round {t}: live count");
        assert_eq!(persisted(&h.cmd, 3), Some(8 * t), "round {t}: survivors");
    }
}

/// KHZ 34: light C functions as keys are findable and each visited once.
#[test]
fn d9_pairs_semantics_unchanged_light_c_function_keys() {
    let src = r"
function on_tick()
  local cf = {string.format, string.rep, string.sub, string.byte, string.len,
    table.insert, table.concat, table.unpack, table.remove, math.floor,
    math.abs, math.max, math.min, pairs, ipairs, type, tostring, tonumber,
    select, rawget, rawequal, rawlen, next, error, pcall, utf8.char}
  local t = {}
  for i, f in ipairs(cf) do t[f] = i end
  local n, bad, seen = 0, 0, {}
  for k, v in pairs(t) do
    n = n + 1
    if seen[v] or cf[v] ~= k then bad = bad + 1 end
    seen[v] = true
  end
  for i, f in ipairs(cf) do if t[f] ~= i then bad = bad + 1 end end
  pinball.persist_set(0, n)
  pinball.persist_set(1, bad)
  pinball.persist_set(2, #cf)
end";
    check_semantics(src, 2, &[1], &[(0, 26), (2, 26)]);
}

// ---------------------------------------------------------------------------
// KHZ 37: string cache by content
// ---------------------------------------------------------------------------

/// D9 / REQ-WEB-005: budget units and `used_memory()` of the long-`__name`
/// string-cache probe are identical across separately spawned processes.
#[test]
fn d9_string_cache_units_and_heap_identical_across_processes() {
    assert_identical_across_processes("strcache", 2);
}

/// D9: the same probe gives the same units and heap in one process under
/// different heap layouts.
#[test]
fn d9_string_cache_units_and_heap_independent_of_heap_layout() {
    for style in NAME_STYLES {
        let src = strcache_script(style, CreationOrder::Forward);
        let reference = strcache_report(&src, 0);
        for noise in [3u64, 0x5eed] {
            assert_eq!(
                strcache_report(&src, noise),
                reference,
                "D9: heap noise {noise} changed the {style:?} string-cache probe"
            );
        }
    }
}

/// D9: the probe's units and heap do not depend on the order in which the
/// long strings were created.
#[test]
fn d9_string_cache_independent_of_creation_order() {
    for style in NAME_STYLES {
        let reference = strcache_report(&strcache_script(style, CreationOrder::Forward), 0);
        for order in [CreationOrder::Reverse, CreationOrder::Strided] {
            assert_eq!(
                strcache_report(&strcache_script(style, order), 0),
                reference,
                "D9: creation order {order:?} changed the {style:?} string-cache probe"
            );
        }
    }
}

/// The string-cache probe is meaningful: `tostring` really yields the long
/// names, and the units observable resolves real work.
#[test]
fn d9_string_cache_probe_sanity() {
    for style in NAME_STYLES {
        let src = strcache_script(style, CreationOrder::Forward);
        let mut b = build(&src, LuaConfig::default());
        let mut h = Host::new(7);
        h.tick(&mut b);
        let extra = match style {
            NameStyle::Suffix => 0,
            NameStyle::Prefix => 1,
        };
        let mut expected = 0i64;
        for r in 1..=20i64 {
            let mut i = 1i64;
            while i <= 60 {
                expected += 41 + i + i.to_string().len() as i64 + extra;
                i += r % 7 + 1;
            }
        }
        assert_eq!(persisted(&h.cmd, 0), Some(expected), "{style:?}");
        let units = strcache_units(&src, 0);
        assert!(units > 1000, "{style:?}: units {units}");
    }
}

// ---------------------------------------------------------------------------
// Existing behaviour: command streams
// ---------------------------------------------------------------------------

/// D9 / §3.4: a fixed-seed script whose commands depend on random draws and
/// object-key `pairs` order gives identical command queues in two processes.
#[test]
fn d9_command_streams_identical_across_processes() {
    assert_identical_across_processes("commands", 2);
}

// ---------------------------------------------------------------------------
// Property: random operation sequences
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum Op {
    TableKey,
    ClosureKey,
    UserdataKey,
    StrKey(u8),
    IntKey(u16),
    Delete(u16),
    Garbage(u8),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => Just(Op::TableKey),
        2 => Just(Op::ClosureKey),
        1 => Just(Op::UserdataKey),
        2 => any::<u8>().prop_map(Op::StrKey),
        2 => any::<u16>().prop_map(Op::IntKey),
        2 => any::<u16>().prop_map(Op::Delete),
        1 => (1u8..40).prop_map(Op::Garbage),
    ]
}

/// Interprets `OPS` (pairs of opcode, argument) on one table; emits the
/// order digest (key 0), visit count (1), mismatches against the tracked
/// live set (2) and the tracked live count (3), each tick.
const PROP_INTERP: &str = r"
local M = 2305843009213693951
local t, keys, vals = {}, {}, {}
local nextval = 0
local function add(k)
  if t[k] == nil then keys[#keys + 1] = k end
  nextval = nextval + 1
  t[k] = nextval
  vals[k] = nextval
end
function on_tick()
  for i = 1, #OPS, 2 do
    local op, a = OPS[i], OPS[i + 1]
    if op == 1 then add({})
    elseif op == 2 then add(function() return a end)
    elseif op == 3 then local _, e = pcall(pinball.pulse_coil, 'bad', a) add(e)
    elseif op == 4 then add('k' .. a)
    elseif op == 5 then add(a * 7919 + 1000000)
    elseif op == 6 then
      if #keys > 0 then
        local j = a % #keys + 1
        local k = keys[j]
        t[k] = nil vals[k] = nil
        keys[j] = keys[#keys] keys[#keys] = nil
      end
    elseif op == 7 then for g = 1, a * 50 do local x = {g} end
    end
  end
  local h, n, bad = 17, 0, 0
  for k, v in pairs(t) do
    h = (h * 1000003 + v) % M
    n = n + 1
    if vals[k] ~= v then bad = bad + 1 end
  end
  pinball.persist_set(0, h)
  pinball.persist_set(1, n)
  pinball.persist_set(2, bad)
  pinball.persist_set(3, #keys)
end";

fn prop_script(ops: &[Op]) -> String {
    let mut lit = String::from("OPS = {");
    for o in ops {
        let (c, a) = match *o {
            Op::TableKey => (1, 0),
            Op::ClosureKey => (2, 0),
            Op::UserdataKey => (3, 0),
            Op::StrKey(a) => (4, u32::from(a)),
            Op::IntKey(a) => (5, u32::from(a)),
            Op::Delete(a) => (6, u32::from(a)),
            Op::Garbage(a) => (7, u32::from(a)),
        };
        lit.push_str(&format!("{c},{a},"));
    }
    lit.push('}');
    format!("{lit}\n{PROP_INTERP}")
}

/// Rust model of the live key count (object keys are always new).
fn model_live_count(ops: &[Op], ticks: usize) -> Vec<i64> {
    #[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
    enum K {
        Obj(usize),
        Str(u8),
        Int(u16),
    }
    let mut keys: Vec<K> = Vec::new();
    let mut set: BTreeSet<K> = BTreeSet::new();
    let mut obj = 0usize;
    let mut out = Vec::new();
    for _ in 0..ticks {
        for o in ops {
            let k = match *o {
                Op::TableKey | Op::ClosureKey | Op::UserdataKey => {
                    obj += 1;
                    Some(K::Obj(obj))
                }
                Op::StrKey(a) => Some(K::Str(a)),
                Op::IntKey(a) => Some(K::Int(a)),
                Op::Delete(a) => {
                    if !keys.is_empty() {
                        let j = usize::from(a) % keys.len();
                        let k = keys.swap_remove(j);
                        set.remove(&k);
                    }
                    None
                }
                Op::Garbage(_) => None,
            };
            if let Some(k) = k
                && set.insert(k.clone())
            {
                keys.push(k);
            }
        }
        out.push(keys.len() as i64);
    }
    out
}

/// Runs the property script for `ticks` ticks under heap noise `noise`;
/// returns `(digest, visited, mismatches, tracked)` per tick.
fn prop_run(src: &str, ticks: usize, noise: u64) -> Vec<[i64; 4]> {
    let mut keep = heap_noise(noise, if noise == 0 { 0 } else { 40 });
    let mut b = build(src, LuaConfig::default());
    let mut h = Host::new(3);
    let mut out = Vec::new();
    for t in 0..ticks {
        if noise != 0 {
            keep.extend(heap_noise(noise.wrapping_add(t as u64 * 7), 9));
        }
        h.tick(&mut b);
        assert!(!b.is_faulted(), "{:?}", b.last_error());
        let get = |k| persisted(&h.cmd, k).expect("emitted");
        out.push([get(0), get(1), get(2), get(3)]);
    }
    black_box(&keep);
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    /// D9 / KHZ 34 property: random create/insert/delete/garbage sequences
    /// give the same iteration-order digest in two backends built under
    /// different heap noise, and visit exactly the live key set.
    #[test]
    fn d9_pairs_order_property_random_ops_independent_of_heap_layout(
        ops in prop::collection::vec(op(), 1..120),
        noise in 1u64..u64::MAX,
    ) {
        let src = prop_script(&ops);
        let ticks = 3;
        let a = prop_run(&src, ticks, 0);
        let b = prop_run(&src, ticks, noise);
        let model = model_live_count(&ops, ticks);
        for t in 0..ticks {
            prop_assert_eq!(a[t][2], 0, "tick {}: visited a non-live key", t);
            prop_assert_eq!(a[t][1], a[t][3], "tick {}: visit count != live count", t);
            prop_assert_eq!(a[t][3], model[t], "tick {}: live count != model", t);
        }
        prop_assert_eq!(a, b, "D9: heap noise changed the pairs order digest");
    }
}
