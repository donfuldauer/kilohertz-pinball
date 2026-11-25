//! KP-068 (D9, REQ-WEB-005 prerequisite): the vendored Lua's `pairs` order
//! and its API string cache depend on no memory address (KHZ 34, KHZ 37 in
//! `third_party/lua-src/KHZ_PATCHES.md`).
//!
//! * KHZ 34: tables, closures, full userdata and threads hash as table keys by
//!   a per-state creation id; light C functions and light userdata share one
//!   main position. Two processes, and two states in one process with
//!   different heap layouts, give one `pairs` order.
//! * KHZ 37: `luaS_new` indexes its cache by string content, so whether
//!   `tostring` finds a long `__name` in the cache (no allocation, no KHZ 4
//!   charge) is a function of content and history only: budget units and heap
//!   bytes are the same in every process.
//!
//! The cross-process tests re-run this test executable with `CHILD_ENV` set;
//! the `child_emit` test then prints its results between markers.

use pin_core::{
    CoilId, HostCommandQueue, HostCtx, HostView, Pcg32, PersistKey, SeedableRng, TableScriptBackend,
};
use pin_script::NameTables;
use pin_script::lua::{LuaConfig, LuaScriptBackend};
use std::hint::black_box;
use std::process::Command;

const CHILD_ENV: &str = "PIN_LUA_ADDR_FREE_CHILD";
const BEGIN: &str = "PIN_LUA_ADDR_FREE_BEGIN";
const END: &str = "PIN_LUA_ADDR_FREE_END";

/// The `pairs`-order digest of [`ORDER_SCRIPT`] after 4 ticks, recorded from
/// the patched build. Address-dependent hashing makes this differ from run to
/// run, so a mismatch here means an address leaked back into key hashing.
const ORDER_DIGEST_PIN: i64 = 2_327_233_128_247_603_749;

/// S8 probe `order.lua`, folded into integer digests: 48 table keys, 24
/// closures, 12 host-error userdata keys, light C functions mixed with
/// table, string and float keys, deletion during traversal, a rehash to 120
/// keys and 3 000 garbage tables per tick (so addresses are reused while
/// creation ids are not). Persist key 0 is the running digest, 1..=5 the
/// per-section digests of the current tick, 6 the number of keys visited.
const ORDER_SCRIPT: &str = "
local M = 0x3fffffffffffffff
local function fold(d, t)
  for _, v in pairs(t) do d = (d * 1000003 + v) & M end
  return d
end
local live = {}
local n = 0
local digest = 7
function on_tick()
  n = n + 1
  local kt, ft, ut, mixed = {}, {}, {}, {}
  for i = 1, 48 do local k = {} kt[k] = i live[(n * 48 + i) % 96 + 1] = k end
  for i = 1, 24 do ft[function() return i end] = i end
  for i = 1, 12 do local _, e = pcall(pinball.pulse_coil, 'bad', i) ut[e] = i end
  local cf = {string.format, string.rep, string.sub, table.insert, table.concat,
    math.floor, math.abs, next, rawget, rawequal, select, utf8.char}
  for i, f in ipairs(cf) do mixed[f] = 100 + i end
  for i = 1, 16 do
    mixed[{}] = 200 + i
    mixed['s' .. i] = 300 + i
    mixed[i * 1.5] = 400 + i
  end
  local d1, d2, d3, d4 = fold(1, kt), fold(2, ft), fold(3, ut), fold(4, mixed)
  -- deletion during traversal, then a rehash to 120 keys
  local d5, visited = 5, 0
  for k, v in pairs(kt) do
    if v % 2 == 0 then kt[k] = nil end
    d5 = (d5 * 1000003 + v) & M
    visited = visited + 1
  end
  for i = 49, 120 do kt[{}] = i end
  d5 = fold(d5, kt)
  for _, d in ipairs({d1, d2, d3, d4, d5}) do digest = (digest * 31 + d) & M end
  pinball.persist_set(0, digest)
  pinball.persist_set(1, d1)
  pinball.persist_set(2, d2)
  pinball.persist_set(3, d3)
  pinball.persist_set(4, d4)
  pinball.persist_set(5, d5)
  pinball.persist_set(6, visited)
  -- garbage: frees this tick's keys after the next collections
  for i = 1, 3000 do local g = {i} end
end";

/// S8 probe `k_strcache.lua`: 60 objects whose metatable `__name` is a long
/// string (41+ bytes), pushed through `luaS_new` by `tostring` in loops.
const STRCACHE_SCRIPT: &str = "
local objs = {}
for i = 1, 60 do objs[i] = setmetatable({}, {__name = string.rep('N', 41 + i) .. i}) end
function on_tick()
  local n = 0
  for r = 1, 20 do for i = 1, 60, (r % 7) + 1 do n = n + #tostring(objs[i]) end end
  pinball.persist_set(0, n)
end";

/// 60 objects whose `__name`s are long strings of one length that differ in
/// their first bytes, pushed by `tostring` 100 rounds a tick. `REVERSE`
/// (prepended) creates the name strings in the opposite order (so at other
/// addresses) without changing any name, any allocation size or the layout
/// of `objs`. `NAME_PAD` sets the length: 45 gives 49-byte names, which the
/// cache hashes whole (exactly `luaS_hash(str, strlen(str), seed)`); 3000
/// gives 3 KiB names, hashed by their 64-byte prefix, whose misses cost a
/// KHZ 4 charge of 47 units and allocate 3 KiB.
const STRCACHE_ORDER_SCRIPT: &str = "
local objs = {}
for i = 1, 60 do objs[i] = false end
local o = ({[false] = {1, 60, 1}, [true] = {60, 1, -1}})[REVERSE]
for i = o[1], o[2], o[3] do
  objs[i] = setmetatable({}, {__name = string.format('n%02d_', i) .. string.rep('N', NAME_PAD)})
end
function on_tick()
  local n = 0
  for r = 1, 100 do for i = 1, 60, (r % 7) + 1 do n = n + #tostring(objs[i]) end end
  pinball.persist_set(0, n)
end";

/// [`STRCACHE_ORDER_SCRIPT`] with names of `pad + 4` bytes, created in
/// reverse order when `reverse`.
fn strcache_order(pad: u32, reverse: bool) -> String {
    // Both orders run the same instructions and define the same globals.
    format!("NAME_PAD = {pad}\nREVERSE = {reverse}\n{STRCACHE_ORDER_SCRIPT}")
}

/// KHZ 34 correctness: every key of every type is visited exactly once,
/// lookups succeed after rehash and after 100 000 further table creations,
/// the current key may be deleted during traversal, keys collected while a
/// traversal is suspended do not break `next`. Persist key 0 is the number
/// of failed checks, 1 the number of checks run.
const CORRECTNESS_SCRIPT: &str = "
local fails, checks = 0, 0
local function check(c) checks = checks + 1 if not c then fails = fails + 1 end end
local keys, set = {}, {}
local function add(k, v) keys[#keys + 1] = k set[k] = v end
function on_tick()
  fails, checks, keys, set = 0, 0, {}, {}
  for i = 1, 48 do add({}, i) end
  for i = 1, 24 do add(function() return i end, 100 + i) end
  local errs = 0
  for i = 1, 12 do
    local _, e = pcall(pinball.pulse_coil, 'bad', i)
    if type(e) == 'userdata' then errs = errs + 1 end
    add(e, 200 + i)
  end
  check(errs == 12)
  local cf = {string.format, string.rep, string.sub, string.byte, string.char,
    table.insert, table.concat, table.remove, table.unpack, math.floor,
    math.abs, math.max, next, rawget, rawequal, select, utf8.char, type, pairs,
    ipairs, tostring, tonumber, rawset, rawlen}
  for i, f in ipairs(cf) do add(f, 300 + i) end
  check(#cf == 24)
  for i = 1, 16 do add('s' .. i, 400 + i) add(i * 1.5, 500 + i) add(i * 7919, 600 + i) end
  add(true, 700)
  -- every key visited exactly once, with its own value
  local function full_check()
    local seen, n = {}, 0
    for k, v in pairs(set) do
      n = n + 1
      check(seen[v] == nil)
      seen[v] = true
      check(set[k] == v)
    end
    check(n == #keys)
    for i, k in ipairs(keys) do check(set[k] ~= nil) check(rawget(set, k) == set[k]) end
  end
  full_check()
  -- light C functions share one main position: all findable, in a table of
  -- their own as well
  local ct = {}
  for i, f in ipairs(cf) do ct[f] = i end
  for i, f in ipairs(cf) do check(ct[f] == i) end
  local cn = 0
  for f, i in pairs(ct) do cn = cn + 1 check(cf[i] == f) end
  check(cn == #cf)
  -- delete the current key during traversal (every third value)
  local removed = 0
  for k, v in pairs(set) do
    if v % 3 == 0 then set[k] = nil removed = removed + 1 end
  end
  local kept = {}
  for _, k in ipairs(keys) do if set[k] ~= nil then kept[#kept + 1] = k end end
  check(#kept + removed == #keys)
  keys = kept
  full_check()
  -- rehash: grow to several hundred keys
  for i = 1, 300 do add({}, 1000 + i) end
  full_check()
  -- 100 000 more table creations (garbage collections run, addresses reused)
  for i = 1, 100000 do local g = {i} end
  full_check()
  for i, k in ipairs(keys) do check(set[k] ~= nil) end
  -- keys collected while a traversal is suspended: delete and drop keys,
  -- make garbage until collections run, then continue with next
  local tmp = {}
  for i = 1, 64 do tmp[{}] = i end
  local k, v = next(tmp)
  local steps = 0
  for kk in pairs(tmp) do if kk ~= k then tmp[kk] = nil end end
  for i = 1, 50000 do local g = {i} end
  while k ~= nil do
    steps = steps + 1
    k, v = next(tmp, k)
  end
  check(steps >= 1 and steps <= 64)
  local rest = 0
  for _ in pairs(tmp) do rest = rest + 1 end
  check(rest == 1)
  pinball.persist_set(0, fails)
  pinball.persist_set(1, checks)
end";

/// A few named coils, enough for the scripts (`pulse_coil('bad')` must fail).
fn names() -> NameTables {
    NameTables {
        coils: vec![("knocker".to_owned(), CoilId(1))],
        persist: (0..8u16)
            .map(|i| (format!("p{i}"), PersistKey(i)))
            .collect(),
        ..NameTables::default()
    }
}

struct Host {
    view: HostView,
    rng: Pcg32,
    cmd: HostCommandQueue,
}

impl Host {
    fn new() -> Self {
        Self {
            view: HostView::default(),
            rng: Pcg32::seed_from_u64(99),
            cmd: HostCommandQueue::default(),
        }
    }

    fn tick(&mut self, b: &mut LuaScriptBackend) {
        self.cmd.clear();
        self.view.tick += 1;
        let mut ctx = HostCtx {
            view: &self.view,
            rng: &mut self.rng,
            cmd: &mut self.cmd,
        };
        b.on_tick(&mut ctx);
    }

    /// Persist writes of the last tick, in order.
    fn persisted(&self) -> Vec<(u16, i64)> {
        self.cmd
            .persist_writes
            .iter()
            .map(|&(k, v)| (k.0, v))
            .collect()
    }

    fn persist(&self, key: u16) -> Option<i64> {
        self.cmd
            .persist_writes
            .iter()
            .rev()
            .find(|(k, _)| k.0 == key)
            .map(|&(_, v)| v)
    }
}

fn backend(src: &str, cfg: LuaConfig) -> LuaScriptBackend {
    LuaScriptBackend::new(src.as_bytes(), &names(), cfg).unwrap_or_else(|e| panic!("load: {e}"))
}

/// Runs [`ORDER_SCRIPT`] for 4 ticks; returns every tick's persist writes.
fn order_transcript() -> String {
    let mut b = backend(ORDER_SCRIPT, LuaConfig::default());
    let mut h = Host::new();
    let mut out = String::new();
    for tick in 1..=4 {
        h.tick(&mut b);
        assert!(!b.is_faulted(), "tick {tick}: {:?}", b.last_error());
        assert_eq!(h.persist(6), Some(48), "tick {tick}: keys visited");
        out.push_str(&format!("{:?}\n", h.persisted()));
    }
    out
}

/// The final running digest of an [`order_transcript`].
fn final_digest(transcript: &str) -> i64 {
    let last = transcript.lines().last().expect("4 ticks");
    let start = last.find("(0, ").expect("digest write") + 4;
    let end = start + last[start..].find(')').expect("digest end");
    last[start..end].parse().expect("integer digest")
}

/// True when `on_tick` of a fresh backend under `budget` units runs without
/// fault (a top level over budget counts as a fault).
fn tick_fits(src: &str, budget: u32) -> bool {
    let cfg = LuaConfig {
        instruction_budget: budget,
        ..LuaConfig::default()
    };
    let Ok(mut b) = LuaScriptBackend::new(src.as_bytes(), &names(), cfg) else {
        return false;
    };
    let mut h = Host::new();
    h.tick(&mut b);
    !b.is_faulted()
}

/// The smallest instruction budget under which the first `on_tick` does not
/// fault: the tick's cost in budget units at the budget's granularity.
fn tick_units(src: &str) -> u32 {
    let (mut lo, mut hi) = (1u32, 4_000_000u32);
    assert!(tick_fits(src, hi), "the tick must fit in {hi} units");
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if tick_fits(src, mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    lo
}

/// Heap bytes after 5 ticks under the default limits, plus the tick result.
fn heap_after_ticks(src: &str) -> (usize, Option<i64>) {
    let mut b = backend(src, LuaConfig::default());
    let mut h = Host::new();
    for _ in 0..5 {
        h.tick(&mut b);
        assert!(!b.is_faulted(), "{:?}", b.last_error());
    }
    (b.used_memory(), h.persist(0))
}

/// String-cache observables of one process: units and heap of both cache
/// scripts (and the long one in reverse creation order).
fn strcache_report() -> String {
    let mut out = String::new();
    let scripts = [
        ("probe", STRCACHE_SCRIPT.to_owned()),
        ("short", strcache_order(45, false)),
        ("short_reverse", strcache_order(45, true)),
        ("big", strcache_order(3000, false)),
        ("big_reverse", strcache_order(3000, true)),
    ];
    for (name, src) in &scripts {
        let units = tick_units(src);
        let (heap, n) = heap_after_ticks(src);
        out.push_str(&format!("{name} units={units} heap={heap} n={n:?}\n"));
    }
    out
}

/// Allocates and frees Rust boxes of assorted sizes and builds and drops a
/// throw-away backend, keeping some boxes alive, so the next Lua state is
/// laid out at different addresses than the previous one.
fn perturb_heap() -> Vec<Vec<u8>> {
    let mut keep = Vec::new();
    for i in 0..400usize {
        let size = 16 + (i * 7919) % 5000;
        let v = vec![i as u8; size];
        if i % 3 == 0 {
            keep.push(v);
        } else {
            black_box(v);
        }
    }
    let mut throwaway = backend(ORDER_SCRIPT, LuaConfig::default());
    let mut h = Host::new();
    h.tick(&mut throwaway);
    for i in 0..64usize {
        keep.push(vec![0xA5; 24 + i * 40]);
    }
    drop(throwaway);
    keep
}

/// Child side: inert unless spawned by a parent test.
#[test]
fn child_emit() {
    let Ok(mode) = std::env::var(CHILD_ENV) else {
        return;
    };
    let out = match mode.as_str() {
        "order" => order_transcript(),
        "strcache" => strcache_report(),
        other => panic!("unknown mode {other}"),
    };
    println!("\n{BEGIN}\n{out}{END}");
}

/// Spawns this executable as a fresh process and returns the child's output.
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
    let begin = stdout.find(BEGIN).expect("child printed no output") + BEGIN.len();
    let end = stdout.rfind(END).expect("child output not terminated");
    stdout[begin..end].trim().to_owned()
}

/// T1 (D9, REQ-WEB-005, KHZ 34): two fresh processes give one `pairs` order
/// over table, closure, userdata and light-C-function keys, and the digest is
/// the pinned one.
#[test]
fn d9_pairs_order_over_object_keys_is_identical_across_processes() {
    let a = spawn("order");
    let b = spawn("order");
    assert!(a.contains("(0, "), "no digest in {a}");
    assert_eq!(a, b, "two processes gave different pairs orders");
    assert_eq!(
        final_digest(&a),
        ORDER_DIGEST_PIN,
        "pairs-order digest moved:\n{a}"
    );
}

/// T2 (D9, REQ-WEB-005, KHZ 34): two states in one process, built from the
/// same script and seed with different heap layouts, give one `pairs` order.
#[test]
fn d9_pairs_order_is_identical_across_heap_layouts_in_one_process() {
    let a = order_transcript();
    let keep = perturb_heap();
    let b = order_transcript();
    drop(keep);
    assert_eq!(a, b, "a different heap layout changed the pairs order");
    assert_eq!(final_digest(&a), ORDER_DIGEST_PIN);
}

/// T3 (D9, KHZ 34): hashing by creation id keeps tables correct for every key
/// type, through deletion during traversal, rehash, 100 000 further table
/// creations and keys collected during a suspended traversal.
#[test]
fn d9_object_keys_stay_findable_through_rehash_deletion_and_collection() {
    let cfg = LuaConfig {
        instruction_budget: 50_000_000,
        ..LuaConfig::default()
    };
    let mut b = backend(CORRECTNESS_SCRIPT, cfg);
    let mut h = Host::new();
    for tick in 1..=3 {
        h.tick(&mut b);
        assert!(!b.is_faulted(), "tick {tick}: {:?}", b.last_error());
        let checks = h.persist(1).expect("checks reported");
        assert!(checks > 1000, "tick {tick}: only {checks} checks ran");
        assert_eq!(h.persist(0), Some(0), "tick {tick}: failed checks");
    }
}

/// T4 (D9, REQ-WEB-005, KHZ 37): the string cache's budget units and heap
/// bytes are identical across two processes.
#[test]
fn d9_string_cache_units_and_heap_are_identical_across_processes() {
    let a = spawn("strcache");
    let b = spawn("strcache");
    assert!(a.contains("big units="), "no report in {a}");
    assert_eq!(a, b, "two processes gave different string-cache costs");
    assert_eq!(a, strcache_report().trim(), "the parent process differs");
}

/// T4 (D9, REQ-WEB-005, KHZ 37): two states in one process with different
/// heap layouts give the same string-cache units and heap bytes.
#[test]
fn d9_string_cache_units_and_heap_are_identical_across_heap_layouts() {
    let a = strcache_report();
    let keep = perturb_heap();
    let b = strcache_report();
    drop(keep);
    assert_eq!(a, b);
}

/// T5 (D9, KHZ 37): the cache is keyed by content: the same long `__name`
/// strings created in the opposite order (so at other addresses) cost the
/// same units, and the scripts compute the same result. (With the cache
/// indexed by the name's address the two orders cost different units.)
#[test]
fn d9_string_cache_units_do_not_depend_on_name_creation_order() {
    for pad in [45, 3000] {
        let fwd = strcache_order(pad, false);
        let rev = strcache_order(pad, true);
        assert_eq!(
            tick_units(&fwd),
            tick_units(&rev),
            "names of {} bytes: creation order changed the cache's units",
            pad + 4
        );
        let (_, n_fwd) = heap_after_ticks(&fwd);
        let (_, n_rev) = heap_after_ticks(&rev);
        assert_eq!(n_fwd, n_rev);
        assert!(n_fwd.is_some_and(|n| n > 100_000), "names were not pushed");
    }
}

/// Guards the unit measurement: a tick that pushes 3 KiB names costs more
/// units than one that pushes 49-byte names (cache misses are charged by
/// size), so equal units are not a vacuous result of a coarse budget.
#[test]
fn d9_unit_measurement_resolves_long_string_charges() {
    let short = tick_units(&strcache_order(45, false));
    let big = tick_units(&strcache_order(3000, false));
    assert!(
        big > short + 1000,
        "3 KiB names {big} units, 49-byte names {short}"
    );
}
