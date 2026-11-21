# S4 — web-spike-rules: Wasm rules modules on the web, sandbox for player bundles (RESULTS)

Base: `main` at `995935f91ad8ad18fcee201b94df52c5aaf1fb6c` (`BASE_COMMIT.txt`), scratch copy of S1's patched
tree (`S1/repo`: patches 0001–0004, sim gating + `ExternalScript`). Toolchain rustc 1.97.0; Node v22.23.5 (V8 12.4);
Playwright Chromium 157.0.8103.78 (V8 15.7); wasm-bindgen CLI 0.2.130 and wasm-tools 1.263.0 from `S1/tools/bin`
(read-only use). Every cargo command: COMMON env (`env.sh`: `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s4`, `CARGO_INCREMENTAL=0`, line-tables-only debug).
Web builds: `RUSTFLAGS="-C target-feature=+simd128 -C link-arg=-zstack-size=4194304"` (S1's recommendation).
Scratch root below = `S` = `${WEB_SPIKES}/S4`.
The machine was shared with other spikes (load 3–4 on 4 cores): timings are medians, quoted as measured, not asserted.

## 0. Bottom line

* **Exact native↔web parity with real `rules.wasm` execution holds on every design that uses the instrumentation.**
  3 production tables × {bot60, pat60, mb8} + the 10-minute apex soak, per tick `state_hash`, S1's `ext_hash` and a
  hash of the full rules command queue, native vs Node vs Chromium: **all PASS** (§4; `logs/parity_full.log`). The
  wasm-rules runs also equal S1's native **Lua** runs on every tick (rules.wasm ≡ rules.lua end to end).
* **Fuel can be made wasmtime-exact on any engine.** A load-time rewrite (`crates/s4_rules/src/instrument.rs`,
  wasmparser 0.261 + wasm-encoder 0.261, compiled into the engine wasm) replays wasmtime 50.0.1's Cranelift fuel
  accounting op-for-op. Evidence: the per-callback fuel of all production callbacks (60 431 per apex bot60, every
  callback of the 600 000-tick soak) is identical to wasmtime's own counter (`fuel=wasmtime-exact` on every parity line), and every fuel
  kill point in the adversarial corpus (3 bisected budget boundaries incl. constant bulk lengths through locals,
  id_of charges, deferred `memory.fill` of 2 MiB) is identical. wasmi's own fuel is **not** (boundaries 9 091 vs
  8 332 loop iterations; a 2 MiB fill completes on wasmi and exhausts on wasmtime).
* **Stack depth is the one guarantee no web engine reproduces**: wasmtime traps at 256 KiB of *native* stack (5 453
  frames for tiny frames, 31 for frames with 1 000 live locals, x86_64); V8's limit is different **and not even
  deterministic run to run** (same module, three runs in one process: Node 13 950 / 20 898 / 20 898 frames,
  Chromium 13 822 / 62 079 / 62 079 — tier-up changes frame sizes); wasmi has its own. A
  weighted call-depth counter inserted by the same rewrite makes native wasmtime, native wasmi, Node and Chromium
  agree exactly (2 338 / 711 / 229 / 78 / 15 frames for 0/16/64/200/1 000 live locals) and trips before every engine's
  own limit. With the counter on native too, the whole adversarial corpus is identical on all designs (82/82 lines;
  the JIT differs only in V8's trap *text* for a null/mistyped `call_indirect` in Node's V8 12.4, not in Chromium).
* **NaN**: the production modules contain no float arithmetic (only `f32.const`); for user modules, V8 and wasmi
  return non-canonical NaNs (0xffc00000, payload-propagating 0x7fe00001, …) where wasmtime canonicalises to
  0x7fc00000; wasmi's `deterministic` feature and an inserted `x == x ? x : canonical` select (B) restore exact bits.
* **Recommendation (design): A — wasmi 2.0.2 inside the engine wasm, running the instrumented module** (wasmtime-
  exact fuel + depth counter, wasmi fuel kept as a backstop), for *every* rules module, curated and user alike;
  **and make native run the same instrumented module under wasmtime** (very-high blast radius, evidence §10.2:
  production replays unchanged — the `native-instr-depth` lines are identical to the engine path on every tick of
  every scenario). The scratch copy's native pin_script suite passes (471 passed, 0 failed, 5 ignored) with the
  spike's read-only probes. JIT (B) is 3–8× faster per
  callback but adds JS glue, V8 trap-handler and RangeError behaviour to the sandbox boundary; production rules cost
  ≈ 5 µs per tick in wasmi-in-V8 against a 6–7 µs sim tick, so speed does not justify it. Hybrid (C) is kept as the
  fallback if a phone measures the interpreter too slow. Worker (D) is rejected (main thread cannot `Atomics.wait`,
  7–10× round-trip overhead, wall-clock watchdog is non-deterministic).
* **Budget rule**: keep 100 000 fuel per callback everywhere (parity); a hostile callback then costs ≤ 2.5 ms native,
  11–37 ms (Chromium) / 24–27 ms (Node) in wasmi-in-V8 on this VM, i.e. ≈ 35–185 ms on an assumed 3–5× slower phone.
  The web shell must therefore add a deterministic-safe **frame-time guard** (stop stepping for the rest of a frame
  when script time in it passes 8 ms; the sim only pauses, no state changes) so a hostile bundle produces slow
  motion, never a hung tab. Do **not** lower the web budget (it would move kill points).
* **Zip/VFS**: the real `Vfs::from_zip_bytes` path in wasm classifies a 33-file hostile corpus exactly like native
  (33/33 Node, 66/66 Chromium file picker + drop); bombs are refused before inflating (≤ 1 MiB growth). Linear memory
  never shrinks and the loader copies twice, so check `File.size` before copying and cap web bundles at 32 MiB
  compressed / 64 MiB uncompressed. 149 of pin_assets' own zip/VFS tests pass under wasm32-wasip1; 15 need a host
  filesystem (folder bundles: not on the web path); 1 test assertion is not 32-bit clean (`zip64_mutations`).
* **A11** refined: wasmtime cannot compile in a browser (Cranelift does not build for wasm32-unknown-unknown in
  50.0.1); its Pulley runtime *does* link (844 KiB, 2 TLS shims) but only runs natively precompiled artefacts, which
  wasmtime documents as unsafe to load from untrusted sources → no wasmtime route for player bundles.
  **A19** changed (fine for real rules, a full-budget hostile callback is 0.7–2.2 frames in wasmi-in-V8); **A20** changed (Chromium's main-thread sync-compile limit is 8 MB, not 4 KiB);
  **A21** confirmed (`'wasm-unsafe-eval'` needed and sufficient).

## 1. Owned assumptions — verdicts

| # | Verdict | Evidence (pointer) |
|---|---|---|
| A11 (wasmtime cannot run in a browser) | **Confirmed, refined** | wasmtime 50.0.1 `runtime`+`pulley` builds for wasm32-unknown-unknown and links once the embedder supplies `wasmtime_tls_get/set` (`pulley_check/`, 844 210 B, no imports); with `cranelift` it does not compile (`unresolved import crate::runtime::vm::sys::mmap`, `MmapVec::new_mmap`). So modules cannot be compiled in the page; Pulley can only run artefacts precompiled natively and loaded with `Module::deserialize`, which is `unsafe` and documented as not for untrusted input → not usable for player bundles. Time-boxed (~15 min), not run end-to-end (§7.2). "Renderer isolated per site" part: needs S5/web. |
| A12 (rules part: NaN) | **Confirmed; matters for user modules only** | Production modules: 0 float arithmetic/comparison/conversion operators, only `f32.const` (wasm-tools scan, §3.2). Synthetic modules: wasmtime returns 0x7fc00000 / 0x7ff8000000000000 for every arithmetic NaN; V8 and wasmi (no `deterministic`) return 0xffc00000 (x86 default NaN), 0x7fe00001 (payload propagated), 0x7fe00000 (demote), 0x7ffc000020000000 (promote), 0xfff8000000000000 (f64 0/0); V8 vs wasmi also differ for `f32.min` (0xffc00000 vs 0x7fe00001). `neg`, store/load round trips: identical. With wasmi `deterministic` and the inserted canonicalisation: identical bits (`out/adv_v1_nocanon/` vs `out/adv/`, 9 NaN cases). |
| A19 (pure-Rust interpreter with fuel/memory/stack limits builds for wasm32 and runs 100 000 fuel well inside a frame) | **Changed: confirmed for real rules, refuted for the worst case; two build conditions** | wasmi 2.0.2 builds for wasm32-unknown-unknown and passes every parity and adversarial line (instrumented). Production callbacks: mean 4.1–6.0 µs in V8 (p99 6–20 µs), ≈ 5 µs per tick — well inside a frame. But a hostile callback that spends the full 100 000 fuel takes 11–37 ms in Chromium and 24–27 ms in Node (1.6 ms natively), i.e. 0.7–2.2 frames on this VM: *not* "well inside a frame", hence the frame guard (§6.2, §10.3). Conditions: (1) wasmi must be built with `auto-dispatch` or `portable-dispatch` on wasm32 — without it every executed instruction is a real call and long callbacks overflow the browser stack (`RangeError` killed the process, §6.4); (2) its own fuel units are not wasmtime's, so use the instrumentation for exact kill points. The repo's budget is 100 000 per callback (`WasmConfig::engine()`, `crates/pin_script/src/wasm/mod.rs:216-221`; `WasmConfig::default()` = 1 000 000 for tools). |
| A20 (browsers compile off the main thread; sync compile of large modules on the main thread is restricted) | **Changed** | Chromium 157 allows `new WebAssembly.Module(bytes)` on the main thread up to 8 MB ("WebAssembly.Compile is disallowed on the main thread, if the buffer size is larger than 8MB"), not 4 KiB; sync `new WebAssembly.Instance` was allowed for every size tested (4 MiB); no limit in Workers (`out/browser/compile_sizes.txt`). Rules modules are ≤ 1 MiB by the pre-scan (+8–20 % after instrumentation), so sync compile is permitted; the async route (`WebAssembly.compile` at bundle load, then sync instantiate) is what the parity runs used in Chromium and gives identical results (`jit-asynccompile` lines). Firefox/Safari limits: needs S5/web. |
| A21 (a CSP needs `'wasm-unsafe-eval'` to compile wasm) | **Confirmed** | `script-src 'self'`: every compile/instantiate path (`new Module`, `compile`, `instantiate`, `compileStreaming`, engine module, rules module) throws `CompileError: … violates … 'unsafe-eval' is not an allowed source`; `WebAssembly.validate` still works. `script-src 'self' 'wasm-unsafe-eval'`: all wasm paths work and `eval` stays blocked. The full recommended policy runs the engine with JIT rules at exact parity (`out/browser/csp*.txt`, §7.1). |

Reproducing commands (after `. S/env.sh; cd S`; `B=$CARGO_TARGET_DIR/release/s4`; `export S4_PKG=S/pkg`):

| # | command |
|---|---|
| A11 | `cd pulley_check && CARGO_TARGET_DIR=$CARGO_TARGET_DIR/pulley cargo build --release --target wasm32-unknown-unknown` (links) and `… --features compile` (fails); without the two `wasmtime_tls_*` functions in `src/lib.rs` the first fails with `undefined symbol: wasmtime_tls_get` |
| A12 | `$B adv native repo/tables/apex_circuit/rules.wasm \| grep nan_` vs `node node/run_web.mjs adv jit-depth repo/tables/apex_circuit/rules.wasm \| grep nan_` (and `out/adv_v1_nocanon/*` for the run without canonicalisation); production scan: `wasm-tools print repo/tables/<t>/rules.wasm \| grep -oE '\bf(32\|64)\.[a-z_0-9]+' \| sort \| uniq -c` |
| A19 | `./run_rules_parity.sh full`; `harness/bench_rules.sh 3` then `python3 -I harness/bench_summary.py out/bench`; `node node/run_stack.mjs wasmi-instr-depth 3` |
| A20 | `node node/chromium.cjs 'web/compile.html?iso=1' sizes '{"sizes":[4096,4097,1048576,4194304,8388608,16777216]}' -` |
| A21 | `for c in none strict wasm unsafe rec; do node node/chromium.cjs "web/csp.html?csp=$c" csp '{}' -; done` |

## 2. What was built (all under `S/`)

* `repo/` — scratch copy of `S1/repo` + crate `repo/crates/s4_rules` (spike tooling) + `patches/0001-pin_script-s4-readonly-probes.patch`
  (two read-only `#[doc(hidden)]` probes on `WasmScriptBackend`: fuel left, guest memory; behaviour unchanged).
* `s4_rules` modules: `prescan.rs` (pin_script's `limits.rs` verbatim except the `wasmparser` import path and
  visibility — `diff <(sed 's/wasmtime::wasmparser/wasmparser/' repo/crates/pin_script/src/wasm/limits.rs) repo/crates/s4_rules/src/prescan.rs`),
  `abi.rs` (pin_script's import/export/resource checks over wasmparser, same order and texts), `hostcore.rs` (the 18
  `pinball` host functions, `Book`, `QueueMark`, `IdTables`, Lua's `range`, engine-neutral, success paths
  allocation-free), `instrument.rs` (§3.3), `wasmi_backend.rs` (A), `js_backend.rs` + `node/khz_rules.js` (B),
  `wasmtime_instr.rs` (native wasmtime running the instrumented module), `run.rs` (designs, recorder, playback),
  `adversarial.rs`, `worst.rs`, `zipcheck.rs`, `web.rs` (wasm-bindgen exports), `bin/s4.rs` (native CLI).
* Designs (names used in every table):

| design | engine | module | fuel | stack limit | NaN |
|---|---|---|---|---|---|
| `native` | wasmtime 50.0.1 via pin_script (engine path untouched) | original | wasmtime | 256 KiB native stack | Cranelift canonicalisation |
| `native-instr-depth` | wasmtime 50.0.1, pin_script's `Config` copied | instrumented + depth counter | instrumentation (wasmtime fuel = backstop) | counter | Cranelift |
| `wasmi-fuel` (A0) | wasmi 2.0.2 | original | wasmi's own | wasmi's own (1 000 calls / 1 MB value stack) | wasmi `deterministic` |
| `wasmi-instr` / `wasmi-instr-depth` (A) | wasmi 2.0.2 in the engine wasm | instrumented (± counter) | instrumentation (wasmi fuel = 4×budget+4 Mi backstop) | counter (wasmi's above it) | wasmi `deterministic` |
| `jit` / `jit-depth` (B) | the browser's V8 (Node / Chromium) | instrumented + host-error wrappers + NaN canonicalisation (± counter) | instrumentation | counter (V8 `RangeError` beyond it) | inserted canonicalisation |
| `hybrid` (C) | B for the 3 SHA-256-pinned production modules, A (`wasmi-instr-depth`) for any other | | | | |
| D | rules in a Worker, synchronous calls over SAB+`Atomics.wait` from a sim Worker, `terminate()` watchdog | JS prototype only (§9) | | | |

Not built (reported, not hidden): tinywasm / wasm-interp comparison (not cheap within the session; wasmi already
answers A19); an end-to-end Pulley run (time box, §7.2).

## 3. Req 1 — the wasmtime guarantees in use, and what each design keeps

### 3.1 Inventory (source: `crates/pin_script/src/wasm/{mod.rs,limits.rs,host.rs}` at `995935f`)

| # | Guarantee | Where | Mechanism |
|---|---|---|---|
| G1 | Per-callback fuel: 100 000 (engine) | `mod.rs:216-221`, `:745-779` | `consume_fuel(true)`, `set_fuel(budget)` before every callback, start function and state hook; exhaustion → `BudgetExceeded`; size-proportional charges for `memory.fill/copy/init`, `table.*`, `memory.grow` (1 per byte/element/page, constants ≤ 128 up front, larger/run-time after success then checked); `id_of` charges `len + 1` |
| G2 | Instantiation cost platform-independent | `mod.rs:70-75`, `:256-261`, `limits.rs` `init_fuel` | `memory_init_cow(false)`; segment init fuel added on top of the start budget |
| G3 | Memory cap 2 MiB | `mod.rs:54-59`, `:325-345`, `:401-408` | declared initial > cap → `MemoryLimit`; `StoreLimits.memory_size(cap)` + `trap_on_grow_failure(false)` → `memory.grow` returns −1 |
| G4 | Tables ≤ 4, ≤ 65 536 elements; 1 memory; 1 instance | `mod.rs:57-59`, `:401-408` | `StoreLimits`, `check_resources` |
| G5 | Wasm stack ≤ 256 KiB (`max_wasm_stack`), host thread ≥ 2 MiB | `mod.rs:103-106`, `:191-197`, `:262` | Cranelift prologue check against the native stack → trap "call stack exhausted" |
| G6 | NaN canonicalisation | `mod.rs:113-117`, `:241` | `cranelift_nan_canonicalization(true)` |
| G7 | Proposal set = WASM1 + sat-f2i + sign-ext + multi-value + bulk-memory + reference-types; SIMD, relaxed-SIMD, threads, tail calls, ext-const, memory64, multi-memory, GC, exceptions, … off | `mod.rs:107-112`, `:239-266`, `limits.rs:30-35` | wasmparser validation with `FEATURES` before compiling + engine config |
| G8 | Compile-cost limits (D25): module ≤ 1 MiB (WAT ≤ 4 MiB), ≤ 2 048 functions, ≤ 1 024 locals/function, nesting/loops/br_table/operator-cost/merge/branch/pressure/loop-work budgets | `limits.rs` | pre-scan before any compile |
| G9 | Import allowlist: only the 18 `pinball` functions with exact signatures; no WASI, memories, tables, globals | `mod.rs:650-662`, `host.rs:51-70` | checked after compile; linker defines only those |
| G10 | Required exports with exact types, `memory` exported, entry points not re-exported imports | `mod.rs:663-675`, `limits.rs:536-551` | load checks |
| G11 | Host-argument validation (ids, booleans, `[0,1]`, finite gains, players, persist keys, durations, `random` range) → trap; ctx imports unavailable outside callbacks | `host.rs` | each host function |
| G12 | Trap → fault ("tilt"): commands of the callback dropped (holds dropped count as held), held coils released, no more calls until reload | `mod.rs:118-131`, `:536-587`, `:745-779` | `QueueMark`, `Book` |
| G13 | Recovery after a trap: the store stays usable (hot reload creates a new instance); traps never corrupt the host | wasmtime trap handling | n/a on the web: no hot reload; a trapped instance is never called again |
| G14 | No host allocation on callback paths | `mod.rs:157-162`, `tests/wasm_alloc.rs` | pre-resolved `TypedFunc`s, raw ctx pointers |
| G15 | Deterministic float semantics | IEEE + G6; no relaxed SIMD (`relaxed_simd_deterministic`) | engine config |

### 3.2 Facts about the three production modules (`notes/<t>.wat`, `wasm-tools print`)

| table | bytes | SHA-256 (prefix) | funcs | imports | memory | float ops | bulk ops | tables / `call_indirect` | max fuel/callback (bot60; all scenarios) | max depth (calls / est. bytes) |
|---|---|---|---|---|---|---|---|---|---|---|
| apex_circuit | 20 603 | c7fb4b8ddc7bf19b | 16 | 12 | 18 pages, no max | 19 × `f32.const`, nothing else | 1 `memory.fill` (len 168) | none | 4 324 | 4 / 1 872 |
| sky_deck | 11 811 | a3fb121ea7f7b1c6 | 17 | 15 | 18 pages, no max | 25 × `f32.const` | 1 `memory.fill` (288) | none | 1 914 | 3 / 1 216 |
| feature_lab | 15 529 | 40abca342ab1ebb3 | 17 | 14 | 18 pages, no max | 8 × `f32.const` | 2 `memory.fill` (40, 408), 1 `memory.copy` (512) | none | 3 209 | 2 / 1 360 |

The production rules use at most **4.3 % of the 100 000 budget** in any callback of any recorded scenario (the soak
included) and recurse at most 4 calls deep. Bulk lengths are compile-time constants on both sides of wasmtime's
128-unit threshold (40 charged up front; 168/288/408/512 charged after success and checked) — the exact-fuel test
exercises both paths on real code.

### 3.3 The instrumentation (design B's transformer, also used by A and recommended natively)

Per original function, from the original operators: entry `fuel -= 1; if fuel <= 0 trap(1)`; costs 0 for
nop/drop/block/loop/unreachable/return/else/end, 1 otherwise (wasmtime-environ `default_operator_cost`); the
buffered cost is flushed before call/call_indirect/return/unreachable/loop/if/br/br_if/br_table/end/else and checked
at loop headers; bulk operations follow `pre/post_translate_bulk_op` (const ≤ 128 up front, otherwise after success
then checked; `memory.grow`/`table.grow` only on success) with Cranelift-equivalent constant detection (constants
through locals, single-predecessor blocks and trivial merges; loop-header parameters unknown inside the loop);
unreachable code is not charged. Optional: weighted depth counter (`16·(params+locals+max stack)+64`, limit 256 KiB),
memory/table maxima capped, NaN canonicalisation after add/sub/mul/div/min/max/sqrt/ceil/floor/trunc/nearest/
demote/promote (Cranelift `nan_canonicalization::is_fp_arith`), import wrappers that call `khz.host_err` and trap
(JIT), start function exported (S4-4). Growth: apex 20 603 → 22 270 B (fuel) / 23 072 B (fuel+depth+JIT wrappers);
sky 11 811 → 13 253 / 14 149; feature 15 529 → 17 426 / 18 280; all validate under the engine feature set
(`$B instrument …`, `wasm-tools validate --features=mvp,mutable-global,saturating-float-to-int,sign-extension,multi-value,bulk-memory,reference-types`).

### 3.4 Guarantee matrix (evidence: §4, §5, §6; P = preserved exactly, W = weakened, L = lost)

| | A0 wasmi-fuel | **A wasmi + instrumentation** | B JIT + instrumentation | C hybrid | D Worker |
|---|---|---|---|---|---|
| G1 fuel per callback | **W**: bounded, but wasmi units → different kill points (7 DIFF lines incl. all 3 boundaries) | **P** (82/82 incl. boundaries; production fuel identical per callback) | **P** (same) | P | L as a budget (wall-clock timeout only); P if combined with instrumentation |
| G2 instantiation cost | P (not charged) | P (segments copied by the engine, start gets budget−1 like wasmtime) | P | P | P |
| G3 memory cap | P (StoreLimits) | **P** (max rewrite; `memory_grow_*`, `huge_*` EXACT) | P | P | P |
| G4 tables/instances | P | P (`table_grow`, `too_many_tables` EXACT) | P | P | P |
| G5 stack | **W**: deterministic, wasmi's own depth (998 / 122 frames) | **P-deterministic, ≠ native today** (counter; == native once native uses it) | **P with counter**; without it V8's limit, non-deterministic (Node 13 950–20 898, Chromium 13 822–62 079 for one module) | P (counter) | L (V8 limit) |
| G6 NaN | P (`deterministic` feature) | P | P (inserted canonicalisation) | P | L unless instrumented |
| G7 proposals | P (same pre-scan) | P | P | P | P (pre-scan in JS/Rust) |
| G8 compile limits | P (same pre-scan) | P | P | P | P |
| G9 import allowlist | P | P | P (import object = whole world; extra `khz.host_err` is ours) | P | P |
| G10 exports | P | P | P | P | P |
| G11 host validation | P (shared `hostcore`) | P (`host_*` lines EXACT) | P | P | P |
| G12 fault/tilt | P | P (`trap_after_partial_queue` EXACT: queue, holds, RNG, guest memory) | P | P | P |
| G13 recovery | P (instance never called again) | P | **W**: a guest trap unwinds only rules frames (host never traps inside Rust); a V8 `RangeError` inside a host frame would unwind Rust frames (prevented by the counter) | P | W (terminate loses state) |
| G14 no host alloc | P after pre-sizing the value stack (0 allocs/60 000 ticks; 1 without, §4) | P (same) | P on the Rust side (0); JS glue allocates nothing per call; invisible to the guard | P | L (postMessage) |
| G15 float semantics | P | P | P | P | P |
| Extra exposure | wasmi bugs (Rust with internal `unsafe`; guest bounds checked in software) | + instrumentation bugs (backstop: wasmi fuel 4×budget) | + instrumentation, JS glue, V8 JIT bugs, trap handler (crash in this VM, §11), no backstop for an instrumentation bug (infinite loop = hung tab) | B's exposure for 3 known modules only | + SAB, cross-isolation requirement |

## 4. Req 4 — parity (`./run_rules_parity.sh full`; `logs/parity_full.log`)

Method: each scenario is S1's recording (`S1/rec/<scn>.scn`: open-loop bot operations, input events, spawns per
tick), replayed on `bundles/<table>_wasm.pinbundle` (S1's bundle with `script_entry = "rules.wasm"`, the committed
module byte-for-byte). The baseline is the **engine path** natively (`Simulation::load_from_vfs` → `Script::Wasm` →
pin_script's wasmtime backend, config untouched). Every other run goes through `Simulation::load_from_vfs_with_script`
(S1's `ExternalScript`) with the design's backend and writes one line per tick: `tick state_hash ext_hash cmd_hash`
(`cmd_hash` = FNV-1a over S1's encoding of the whole applied `HostCommandQueue`: pulses, holds, magnets, zippers,
diverters, spawns, lamps, GI tint, scores, scenes, sounds, persist writes). PASS = the file is byte-identical to the
baseline (every tick, all three hashes). The fuel note compares the FNV of every callback's fuel use with the
wasmtime backend's own counter (`patches/0001`). Chromium runs use the async-compile route (`mode: cache`) and
COOP/COEP.

| scenario (ticks) | final state_hash / cmd-stream hash | native Lua (S1) = wasm rules | native native | native native-instr-depth | native wasmi-fuel | native wasmi-instr | native wasmi-instr-depth | node wasmi-fuel | node wasmi-instr | node wasmi-instr-depth | node jit | node jit-depth | node hybrid | node jit-asynccompile | chromium jit | chromium jit-depth | chromium wasmi-instr-depth | chromium hybrid |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| apex_circuit_bot60 | 2b7b1eec564b8cdc / 42beeae5a66b58c6 | PASS | PASS | PASS | PASS* | PASS | PASS | PASS* | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| apex_circuit_pat60 | b9c2a55ba04954e9 / 605b39f4b45d3b85 | PASS | PASS | PASS | PASS* | PASS | PASS | PASS* | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| apex_circuit_mb8 | 2c52fb33d519ffb7 / c9af04f73845b7da | PASS | PASS | PASS | PASS* | PASS | PASS | PASS* | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| sky_deck_bot60 | 1f0509f5802dc2e5 / 8bdf4bd62a39893a | PASS | PASS | PASS | PASS* | PASS | PASS | PASS* | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| sky_deck_pat60 | ce9eb11d97fbef81 / 416bba4ce2d9effb | PASS | PASS | PASS | PASS* | PASS | PASS | PASS* | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| sky_deck_mb8 | 777557dae8e5531b / 87ab9b527c654fe4 | PASS | PASS | PASS | PASS* | PASS | PASS | PASS* | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| feature_lab_bot60 | 2040cea9ef77a2e0 / 221251219a31e7e5 | PASS | PASS | PASS | PASS* | PASS | PASS | PASS* | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| feature_lab_pat60 | 865f852fce618b7e / 8e580efa5845321b | PASS | PASS | PASS | PASS* | PASS | PASS | PASS* | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| feature_lab_mb8 | 0c64556eecf5c210 / 3d89d46769e23c17 | PASS | PASS | PASS | PASS* | PASS | PASS | PASS* | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS | PASS |
| apex_circuit_bot600 | a4b944ad641338d1 / 9d70b003550e5eed | PASS | PASS | PASS | – | – | PASS | – | – | PASS | – | PASS | – | PASS | – | PASS | – | – |

161 PASS, 0 FAIL. `*` = per-tick hashes identical but per-callback fuel differs from wasmtime (wasmi's own units).
(Table: `python3 -I harness/parity_table.py logs/parity_full.log`. The per-design hash files were deleted to save
shared disk after their MD5s were recorded in `out/hash_manifest.md5`: all 174 files, every scenario, equal their
engine baseline; `run_rules_parity.sh` regenerates them. A later `quick nobuild` re-run on the final binaries:
`logs/parity_quick.log`, 51 PASS.)

* **rules.wasm ≡ rules.lua**: the wasm-rules baseline's `state_hash` and `ext_hash` columns equal S1's native Lua
  recordings on every tick of all 10 scenarios (column 3) — the cross-check promised by 6.3 holds end to end.
* **No diverging tick anywhere.** The NaN question for these modules is moot: they contain no float arithmetic
  (§3.2), and no NaN reached hashed state (S1's R-4 still applies to the sim).
* **Fuel accounting**: every instrumented design reports exactly wasmtime's per-callback fuel for every callback
  (apex bot60: 60 431 callbacks, digest `f17cda6a414ff89c`, max 4 324; the 600 000-tick soak: digest `243f70e84680632c`); wasmi's own
  fuel differs (`*`: apex max 4 519, sky 2 309, feature 4 321) but no production callback comes near the budget, so
  the hashes still agree.
* `native-instr-depth` (native wasmtime running the instrumented module with the depth counter) is identical to the
  engine path on every tick of every scenario incl. the soak: adopting the instrumentation natively does not change
  production replays.
* D5 allocation guard over 60 000 ticks per table (`out/alloc/alloc_guard.txt`): `allocs_in_step=0` for
  native-instr-depth, wasmi (native and Node) and the JIT (Node). wasmi made **one** allocation at the first
  `on_tick` until its value stack was pre-sized (`Config::set_min_stack_height(256 KiB)`; before: `allocs_in_step=1
  first_alloc_tick=Some(2)` on apex and feature).

## 5. Req 5 — adversarial modules (`./run_rules_adversarial.sh`; `logs/adversarial.log`)

Corpus (`crates/s4_rules/src/adversarial.rs`, 79 modules + 3 bisected fuel boundaries = 82 lines; WAT through the
`wat` crate, byte mutations of apex's `rules.wasm`): termination (back-edge-only loop, loop with calls, loop calling
`random`, loop calling `hold_coil`, looping start function, start function calling a ctx import, start function
using `id_of`), stack (unbounded recursion; recursion drawing `random` at every level; 200-local frames; recursion
of depth n = 100…60 000), memory (`grow` within/past the 2 MiB cap with constant and run-time deltas, initial 33
pages, declared max 65 536, out-of-bounds data segment, 900 KB data segment, 2 MiB deferred `memory.fill`, small
constant `memory.fill` in a loop directly and through a local), pre-scan limits (2 049 functions, 1 025 locals, 33
nested loops, 5 tables), tables (`table.grow` past 65 536, `call_indirect` type confusion / null / out of bounds),
traps (div by zero, `INT_MIN/-1`, `unreachable`, trunc of NaN, OOB load, trap after queueing pulse+lamp+hold+score
and writing guest memory), host-argument errors (coil 70 000, NaN intensity, `id_of` bad kind / OOB name / 60 000-
byte names until out of fuel), imports/exports (unlisted import, WASI import, wrong import signature, imported
memory, wrong export signature, callback re-exporting an import, missing `memory` export, exported mutable globals
named like the instrumentation's, an import re-exported under another name), 9 NaN-payload cases, proposals
(bulk memory, reference types, multi-value, sign-ext/sat — must load; threads/shared memory, SIMD, relaxed SIMD,
tail calls, exceptions, memory64, multi-memory, extended-const, GC — must be refused), malformed bytes (empty, bad
magic, truncated half / minus one byte, trailing garbage, trailing well-formed custom section, WAT text, the
production module). Script per module: load, then `on_init`, `on_tick` ×2, `on_switch_edge(3, closed)`, `on_tick`
(cases with a loop count use switch ids). Per call the line records: outcome (`ok` / `budget` / `runtime:<kind>` /
`skipped` after a fault), fuel (for completed and budget-exhausted calls), FNV of the command queue, the host RNG's
next value, FNV of guest memory, and the scores (where cases report values such as `memory.grow` results or NaN
bits).

**PASS/FAIL table** (`python3 -I harness/adv_table.py logs/adversarial.log`; E = EXACT lines, C = same
classification with different kill point / fuel / bits, D = different classification):

| design (route) | vs (1) native wasmtime today | vs (2) native wasmtime + instrumentation + depth counter |
|---|---|---|
| native-instr-depth | PASS(stack kill point only) [E=80 C=1 D=1] | (baseline) |
| native wasmi-instr / wasmi-instr-depth | PASS(stack kill point only) [E=80 C=1 D=1] | **PASS [E=82]** |
| Node wasmi-instr / wasmi-instr-depth | PASS(stack kill point only) [E=80 C=1 D=1] | **PASS [E=82]** |
| Chromium wasmi-instr-depth | PASS(stack kill point only) [E=80 C=1 D=1] | **PASS [E=82]** |
| Node jit-depth | PASS(stack kill point only) [E=78 C=1 D=3] | PASS(message only) [E=80, D=2: V8 12.4's "null function or function signature mismatch" for both `call_indirect` cases] |
| Node jit (no counter) | PASS(stack kill point only) [E=79 C=1 D=2] | – |
| Chromium jit-depth (trap handler off, S4-5) | PASS(stack kill point only) [E=80 C=1 D=1] | **PASS [E=82]** |
| Node / Chromium hybrid | PASS(stack kill point only) [E=80 C=1 D=1] | **PASS [E=82]** |
| native / Node wasmi-fuel | **FAIL** [E=65 C=10 D=7] | – |

What differs, and why:
* **Fuel (wasmi's own units)**: `memory_fill_2mib_deferred` exhausts the budget on wasmtime but completes on wasmi
  (64 519 wasmi units; wasmi charges bulk bytes far cheaper); constant 100-byte fills in a loop: wasmtime stops at
  893 iterations, wasmi at 7 142; the `random` loop: 9 091 vs 8 332 iterations, so a script killed mid-callback has
  drawn a different number of host RNG values (`rng=28a4673b` vs `d61d054a`) → replays diverge. With the
  instrumentation every design reproduces wasmtime's boundaries exactly: 9 091 / 893 / **892** (the constant fed
  through a local is charged up front like Cranelift sees it) — native, Node and Chromium, JIT and wasmi.
* **Stack kill point** (vs native today): wasmtime traps at 256 KiB of native stack; the counter traps earlier and
  identically everywhere, so `deep_recursion_random_each_level` draws a different number of random values and
  `recursion_depth_n(5 000)` succeeds natively but faults with the counter. Recursion depth limits (largest depth
  that completes, `out/stack_all.txt`, repeated runs in brackets):

| frame (live i32 locals) | native wasmtime | wasmi own limits | V8 JIT, Node, no counter | V8 JIT, Chromium, no counter | **counter (native wasmtime, native wasmi, Node, Chromium, JIT and wasmi)** |
|---|---|---|---|---|---|
| 0 | 5 453 | 998 | 13 950 / 20 898 / 20 898 | 13 822 / 62 079 / 62 079 | **2 338** ×all |
| 16 | 1 486 | 998 | 6 965 / 6 965 / 5 699 | 6 894 ×3 | **711** |
| 64 | 466 | 998 | 2 943 / 1 790 / 1 790 | 2 943 / 1 772 / 1 772 | **229** |
| 200 | 157 | 619 | 1 135 / 607 / 607 | 1 127 / 601 / 601 | **78** |
| 1 000 | 31 | 122 | 244 / 123 / 123 | 242 / 122 / 122 | **15** |

  V8's own limit changes between identical runs in one process (tier-up from Liftoff to TurboFan changes frame
  sizes): it cannot be a replay-stable kill point. The counter's weight (estimated bytes) is ≥ 2× wasmtime's real
  frames and V8's, so it always fires first; production modules peak at 4 calls / 1 872 estimated bytes (0.7 %).
* **Unchanged everywhere (EXACT on every instrumented design)**: all load-time refusals with identical texts (the
  pre-scan and ABI checks are shared code: `many_functions_2049` "module has 2049 functions (at most 2048)",
  `unlisted_import` "unsupported import pinball::launch_missiles …", `export_wrong_signature`,
  `reexport_import_as_callback`, `missing_memory_export`, `huge_initial_memory` → `MemoryLimit`, all refused
  proposals with wasmparser's texts, malformed/truncated/trailing bytes); `start_loops` → `BudgetExceeded` at load;
  `start_calls_ctx_import` → Load("the pinball API is only available inside …"); `memory.grow` → 1 / 11 / −1 /
  size 32 pages; `table.grow` past 65 536 → −1; `trap_after_partial_queue` → queue rolled back, coil 4 released
  `(4, false)`, guest memory keeps the partial write (same FNV), RNG untouched; host-argument errors with the exact
  wasmtime text (`pinball.pulse_coil: coil id 70000 out of range 0..=65535`); `id_of` 60 000-byte names: first call
  60 018 fuel, second exhausts; `mutable_global_export_collision`: the module's own `__khz_fuel`/`__khz_why`
  exports do not touch the instrumentation (its exports are renamed `__khz_fuel_1`…).
* **NaN** (before canonicalisation, `out/adv_v1_nocanon/`): 7 NaN cases CLASS on V8 and wasmi (bits differ, §1 A12);
  after (`deterministic` wasmi, inserted select for V8): EXACT.
* **Trap texts**: V8 12.4 (Node) reports "null function or function signature mismatch" for both a null entry and a
  signature mismatch; V8 15.7 (Chromium) and wasmtime distinguish them. Fault class is identical; only the message
  shown to a player differs. Recommendation: map trap kinds to fixed texts in the shared host logic.

## 6. Req 3 — cost: production callbacks, worst case, load

`harness/bench_rules.sh 3` (3 runs, medians; `out/bench/`), release builds; per callback timed around the
backend call with `Instant` (native) or `performance.now()` (Node: sub-µs; Chromium cross-origin-isolated: 5 µs
quantum, so its p50/p99 are quantised and its mean comes from the sum). Real callbacks of the bot60 scenarios
(≈ 60 430 per table, ≈ 1.007 per tick). Same machine as S1 (sim tick 6–7 µs wasm).

### 6.1 Production callbacks

| route / design | table | mean µs / callback | p50 | p99 | p99.9 | rules ms per 60 000 ticks | ≈ µs per tick | max fuel |
|---|---|---|---|---|---|---|---|---|
| native native | apex_circuit | 0.31 | 0.29 | 0.54 | 1.7 | 19 | 0.32 | 4324 |
| native native | sky_deck | 0.34 | 0.32 | 0.61 | 3.9 | 20 | 0.34 | 1914 |
| native native | feature_lab | 0.36 | 0.32 | 0.80 | 4.2 | 22 | 0.37 | 3209 |
| native native-instr-depth | apex_circuit | 0.40 | 0.38 | 0.78 | 3.8 | 24 | 0.41 | 4324 |
| native native-instr-depth | sky_deck | 0.52 | 0.40 | 1.12 | 6.2 | 31 | 0.52 | 1914 |
| native native-instr-depth | feature_lab | 0.41 | 0.37 | 1.03 | 4.6 | 25 | 0.42 | 3209 |
| native wasmi-instr-depth | apex_circuit | 0.63 | 0.58 | 1.81 | 6.6 | 38 | 0.64 | 4324 |
| native wasmi-instr-depth | sky_deck | 0.65 | 0.55 | 2.08 | 11.8 | 39 | 0.66 | 1914 |
| native wasmi-instr-depth | feature_lab | 0.59 | 0.51 | 2.17 | 5.5 | 36 | 0.60 | 3209 |
| native wasmi-fuel | apex_circuit | 0.51 | 0.47 | 1.38 | 9.5 | 31 | 0.51 | 4519 |
| native wasmi-fuel | sky_deck | 0.56 | 0.47 | 1.46 | 7.5 | 34 | 0.57 | 2309 |
| native wasmi-fuel | feature_lab | 0.55 | 0.49 | 1.71 | 6.4 | 33 | 0.56 | 4321 |
| node jit-depth | apex_circuit | 0.54 | 0.42 | 1.25 | 12.4 | 32 | 0.54 | 4324 |
| node jit-depth | sky_deck | 0.94 | 0.46 | 1.51 | 12.9 | 57 | 0.94 | 1914 |
| node jit-depth | feature_lab | 1.00 | 0.49 | 1.87 | 12.2 | 61 | 1.01 | 3209 |
| node wasmi-instr-depth | apex_circuit | 4.45 | 2.83 | 8.59 | 34.1 | 269 | 4.49 | 4324 |
| node wasmi-instr-depth | sky_deck | 4.14 | 2.26 | 6.32 | 36.3 | 250 | 4.16 | 1914 |
| node wasmi-instr-depth | feature_lab | 4.53 | 2.54 | 10.75 | 42.2 | 274 | 4.56 | 3209 |
| node wasmi-fuel | apex_circuit | 3.44 | 1.70 | 5.15 | 27.1 | 208 | 3.46 | 4519 |
| node wasmi-fuel | sky_deck | 2.63 | 1.28 | 3.59 | 25.5 | 159 | 2.65 | 2309 |
| node wasmi-fuel | feature_lab | 2.96 | 1.53 | 6.01 | 34.8 | 179 | 2.98 | 4321 |
| chromium jit-depth | apex_circuit | 1.07 | 0.00 | 5.00 | 15.0 | 65 | 1.08 | 4324 |
| chromium jit-depth | sky_deck | 1.39 | 0.00 | 5.00 | 15.0 | 84 | 1.40 | 1914 |
| chromium jit-depth | feature_lab | 1.60 | 0.00 | 5.00 | 15.0 | 97 | 1.61 | 3209 |
| chromium wasmi-instr-depth | apex_circuit | 5.96 | 5.00 | 10.00 | 70.0 | 360 | 6.00 | 4324 |
| chromium wasmi-instr-depth | sky_deck | 5.03 | 5.00 | 10.00 | 50.0 | 304 | 5.06 | 1914 |
| chromium wasmi-instr-depth | feature_lab | 4.43 | 5.00 | 20.00 | 50.0 | 268 | 4.47 | 3209 |

Per 60 Hz frame (16–17 ticks) the rules cost ≈ 0.01 ms natively, 0.02–0.03 ms with the JIT and 0.07–0.10 ms with
wasmi-in-V8; at an assumed 3–5× slower phone core: ≤ 0.5 ms (wasmi) — 3 % of a frame. Instrumentation overhead
natively: 0.31→0.40 µs (+20–50 %), invisible at the tick level.

### 6.2 Worst case a hostile module can force: one callback running to 100 000 fuel

`$B worst <design> 5`, `node node/run_web.mjs worst <design> 5`, Chromium via `web/index.html` (`out/bench/worst.*`);
median of 5 fresh instances, ms; every row ends `fault=budget fuel=100000`. Zero-fuel floods (23 000 `nop`s or 1 400
empty blocks per iteration — the largest the D25 pre-scan accepts; 60 000 nops / 4 000 blocks are refused by its
loop-work / branch-work budgets): native 0.05 / 1.43 ms, native wasmi-instr 0.62 / 0.62, Node JIT 0.12 / 0.13,
Chromium JIT 0.23 / 0.22, Node wasmi-instr 26.9 / 20.9, Chromium wasmi-instr 15.8 / 37.2 (`out/bench/worst.flood.txt`).

| hostile on_tick loop body | native wasmtime today | native wasmtime+instr | native wasmi+instr | Node JIT+instr (B) | Node wasmi+instr (A) | Node wasmi-fuel | Chromium JIT+instr (B) | Chromium wasmi+instr (A) |
|---|---|---|---|---|---|---|---|---|
| `loop_br_only` | 0.04 | 0.10 | 0.59 | 0.24 | 23.89 | 1.22 | 0.07 | 9.38 |
| `loop_arith` | 0.03 | 0.07 | 0.13 | 0.07 | 1.79 | 0.60 | 0.06 | 1.72 |
| `loop_call_empty` | 0.10 | 0.15 | 0.75 | 0.29 | 11.08 | 1.52 | 0.15 | 10.19 |
| `loop_call_indirect` | 0.10 | 0.14 | 0.75 | 0.31 | 8.25 | 1.88 | 0.28 | 8.38 |
| `loop_host_tick` | 0.35 | 0.54 | 1.58 | 1.04 | 8.07 | 2.59 | 3.01 | 11.19 |
| `loop_host_random` | 0.77 | 0.71 | 1.30 | 1.44 | 5.21 | 2.36 | 1.67 | 6.42 |
| `loop_host_id_of_len0` | 0.26 | 0.66 | 0.66 | 5.26 | 4.30 | 1.91 | 5.46 | 4.60 |
| `loop_host_hold_coil_overflow` | 0.23 | 0.38 | 0.85 | 0.33 | 4.68 | 1.98 | 1.09 | 8.33 |
| `loop_host_set_lamp_overflow` | 0.24 | 0.35 | 0.89 | 0.40 | 5.16 | 2.08 | 1.14 | 4.80 |
| `loop_memory_grow0` | 2.48 | 2.46 | 0.21 | 7.89 | 2.89 | 0.81 | 2.07 | 2.73 |
| `loop_memory_size` | 0.03 | 0.08 | 0.31 | 0.10 | 4.31 | 1.08 | 0.18 | 4.01 |
| `loop_memory_fill_128` | 0.03 | 0.07 | 0.01 | 0.02 | 0.14 | 0.90 | 0.06 | 0.14 |
| `loop_memory_copy_64k` | 0.07 | 0.10 | 0.01 | 0.06 | 0.01 | 0.17 | 0.10 | 0.02 |
| `loop_load_store` | 0.03 | 0.08 | 0.10 | 0.08 | 1.66 | 0.73 | 0.14 | 1.65 |
| **worst (ms)** | **2.48** | **2.46** | **1.58** | **7.89** | **23.89** | **2.59** | **5.46** | **11.19** |

Derived budget figures (this VM; mobile = assumed 3–5× slower, an assumption for S5/S6):

| | native wasmtime today | B JIT (Chromium / Node) | A wasmi-in-V8 (Chromium / Node) |
|---|---|---|---|
| worst hostile callback (100 000 fuel) | 2.5 ms | 5.5 / 7.9 ms | 11–37 / 24–27 ms |
| on a 3–5× slower phone | 7–12 ms | 16–40 ms | 33–185 ms |
| vs 1 ms tick / 16.7 ms frame | 2.5 ticks / 0.15 frame | up to 8 ticks / 0.5 frame | up to 37 ticks / 2.2 frames |
| fuel that fits 1 ms on the phone (linear) | 8 000–13 000 | 2 500–6 000 | 540–3 000 |
| production need (max per callback) | 4 324 | 4 324 | 4 324 |

So no single per-callback budget both admits the production rules (4 324) and bounds a hostile callback to 1 ms on
a slow phone in every design — which is why the rule below keeps 100 000 for determinism and bounds *wall time per
frame* in the shell instead. The per-tick worst case is also not just one callback: up to `MAX_PENDING_EDGES` = 512
switch edges per tick (`crates/pin_sim/src/sim/mod.rs:56`) each get a fresh 100 000-fuel callback, natively as well
(queue line; the native 8 ms stall target was measured with one edge per tick).

**Rule**: keep 100 000 fuel per callback on every platform (exact kill points). The web shell measures script
time per frame and, once it passes 8 ms in a frame, stops stepping until the next frame (the sim pauses; inputs keep
their timestamps; no state changes, so replays are unaffected); after 2 s of continuous throttling it shows "this
table's rules are too slow for this device". A hostile bundle then yields slow motion (≥ 1 callback per frame),
never a hung tab; production tables (≤ 0.5 ms per frame on a slow phone) never trigger it. Optimisation for A
(not measured): keep the fuel counter in a local and write the global only before calls/returns/checks, as Cranelift
does — the hot loop pays ~9 extra wasmi instructions per back edge today (0.6 ms natively vs 0.1 ms for wasmi's own
fuel).

### 6.3 Load time and compile route (A20)

| module | native wasmtime (Cranelift) | native wasmi (eager, instrumented) | Node wasmi | Node JIT sync / async-cached | Chromium wasmi | Chromium JIT sync / async-cached |
|---|---|---|---|---|---|---|
| apex (20.6 KB) | 19.9 ms | 1.9 ms | 5.4 ms | 3.0 / 2.8 ms | 9.1 ms | 7.2 / 9.4 ms |
| sky (11.8 KB) | 20.4 ms | 1.1 ms | 3.2 ms | 1.8 / 1.5 ms | 5.7 ms | 5.8 / 2.2 ms |
| feature (15.5 KB) | 16.9 ms | 1.6 ms | 4.3 ms | 2.7 / 2.2 ms | 13.0 ms | 8.2 / 4.0 ms |

(median of 20 `load` calls: pre-scan + checks + instrumentation + compile + instantiate + start; `out/bench/load.*`.)
Chromium's main-thread synchronous compile limit is **8 MB** (`out/browser/compile_sizes.txt`: 4 194 299 B compiles
sync in 14.7 ms; 16 777 211 B → "RangeError: WebAssembly.Compile is disallowed on the main thread, if the buffer size
is larger than 8MB"; sync `new WebAssembly.Instance` allowed for every size tried; Workers have no limit). An accepted
rules module is ≤ 1 MiB (pre-scan) → ≤ 1.25 MiB instrumented → always under it. A bundle is loaded before play, so
the async route (`WebAssembly.compile` at bundle load, sync instantiate inside the sim's load) costs nothing in the
loading flow and is what Chromium parity used (identical results, `jit-asynccompile` lines). Design A needs no
browser compile at all.

### 6.4 wasmi build requirement found on the way (F-A2)

With `default-features = false` and without `auto-dispatch`/`portable-dispatch`, wasmi 2.0.2 uses tail-call
dispatch; on wasm32 without the `tail-call` target feature LLVM emits real calls, so every executed instruction
deepens the browser stack: the stack probe aborted the whole Node process with `RangeError: Maximum call stack size
exceeded` inside `wasmi::engine::executor::handler::exec::*` (a trap through the engine module, unrecoverable).
Reproduce: build the wasm module with `--no-default-features` (drops `s4_rules`'s `wasmi-auto-dispatch` feature)
into `pkg_nodispatch/`; `S4_PKG=S/pkg_nodispatch node node/run_stack.mjs wasmi-fuel 1` and even a plain
budget-exhausting loop (`node node/run_web.mjs worst wasmi-fuel 1 loop_br_only`) die with that `RangeError`
(`out/fa2_nodispatch.txt`, `out/fa2_nodispatch_worst.txt`). Production callbacks (≤ 4 324 instructions)
happened to fit, so parity alone would not have caught it. With `auto-dispatch` (portable loop dispatch on wasm32)
the same probes are deterministic (`out/stack_all.txt`).

## 7. CSP (A21) and wasmtime in the browser (A11)

### 7.1 CSP (`web/csp.html`, `node/serve.cjs ?csp=…`; `out/browser/csp.txt`, `out/browser/csp_rec.txt`)

| policy (response header) | `WebAssembly.validate` | `new Module` / `compile` / `instantiate` / `compileStreaming` (tiny, engine module, rules.wasm) | `eval` |
|---|---|---|---|
| none | ok | ok | ok |
| `script-src 'self'` | ok | **CompileError**: "Compiling or instantiating WebAssembly module violates … 'unsafe-eval' is not an allowed source of script" | EvalError |
| `script-src 'self' 'wasm-unsafe-eval'` | ok | ok | EvalError (still blocked) |
| `script-src 'self' 'unsafe-eval'` | ok | ok | ok |
| recommended (below) | ok | ok | EvalError |

**Header the local dev server should send** (with COOP/COEP, which the sim does not need but the owner's local setup
allows): `Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self';
object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`. Under exactly this policy the
engine page loads, compiles the engine module and the instrumented rules module and replays apex bot60 at exact
parity in Chromium (`out/browser/csp_rec_play.out`, compared with `cmp` to the native engine hashes). Design A
compiles nothing at run time besides the engine module; B/C compile the instrumented rules module under the same
`'wasm-unsafe-eval'` permission (no extra source needed). Bundles from the file picker need no `connect-src`.

### 7.2 wasmtime in the browser (A11)

`pulley_check/` (standalone crate, `[workspace]` of its own): `wasmtime = { version = "=50.0.1", default-features =
false, features = ["runtime", "pulley"] }`, target pulley32, an exported entry that deserialises and runs a module.
* Without embedder hooks: link error `undefined symbol: wasmtime_tls_get` (the custom-platform API).
* With `wasmtime_tls_get/set` defined (one static, single-threaded): links, 844 210 B, **no imports**.
* `--features compile` (adds `cranelift`): does not compile for wasm32-unknown-unknown
  (`unresolved import crate::runtime::vm::sys::mmap`, `cannot find type Mmap`, `MmapVec::new_mmap`).
* Consequence: in a page, wasmtime can only *run* modules compiled to Pulley bytecode elsewhere and loaded with
  `Module::deserialize`, which is `unsafe` (wasmtime: only for artefacts you produced and trust). A player bundle
  cannot be compiled in the page and its precompiled artefact cannot be trusted → **no wasmtime route for user
  bundles**; for curated tables it would work but buys nothing over A (wasmi gives exact fuel through the same
  instrumentation and is 0.2 ms to compile per module). Not run end-to-end (time box).

## 8. Req 7 — bundles from a file picker: zip/VFS hardening under wasm

Method: the real loader (`Vfs::from_zip_bytes`, the web path) behind one function (`crates/s4_rules/src/zipcheck.rs`:
open, then read every entry; allocator high-water mark natively; `memory.buffer.byteLength` before/after on wasm),
run natively, in the engine wasm under Node (fresh process per file), and in Chromium from a real
`<input type=file>` (Playwright `setInputFiles`) and from a synthetic drop (`DataTransfer` + `File`), each a fresh
page: `File.arrayBuffer()` → `Uint8Array` → wasm-bindgen copy into linear memory → loader. Corpus: 33 files made by
`harness/make_zip_corpus.py` (stdlib `zipfile` + byte patches) — bombs (per-entry ratio, total size, archive-level
ratio, declared-size lie), honest-but-large (70 MiB stored random), path traversal (`../`, nested `..`, absolute,
backslash, drive letter, NUL, non-UTF-8), 10 001 entries, duplicate names, case collision, symlink entry, nested zip
bomb, encrypted flag, bad CRC, stored method, bzip2 method, forced ZIP64, ZIP64 with a bad locator, truncated central
directory, truncated half, prepended junk, trailing junk, comment-length lie, empty file, not a zip, two end records,
plus the production bundle.

| run | limits | identical classification to native |
|---|---|---|
| Node, engine wasm (`out/zip/node.{default,web}.txt`) | default (512 MiB) and web (64 MiB) | **33/33** and **33/33** |
| Chromium file picker + drop (`out/zip/chromium.web.txt`) | web | **66/66** |

Selected rows (web limits; Chromium picker; memory = wasm linear memory before → after, MiB):

| file | result (same on native) | File size | memory |
|---|---|---|---|
| production bundle | ok, 56 entries, 2 354 580 B read | 1 976 314 | 4.4 → 9.4 |
| 50 MiB single-entry bomb | refused: file too large (web) / ratio 1028:1 (default) | 51 086 | 4.4 → 4.6 |
| 600 × 1 MiB zeros | refused: bundle too large (web) / archive ratio 102:1 (default) | 676 002 | 4.4 → 5.9 |
| 100 × 1 MiB zeros | refused: archive ratio 102:1 | 112 502 | 4.4 → 4.7 |
| declared-size lie | opens; read refused: "decompressed size differs from the declared 1000" | 4 192 | 4.4 → 4.6 |
| `../evil.txt`, `a/../../evil.txt`, `..\evil.txt` | refused: path escapes the bundle | 237–247 | +0.1 |
| `/etc/passwd`, `C:\evil.txt` | refused: absolute path | 237 | +0.1 |
| NUL / non-UTF-8 name | refused: not valid UTF-8 | 229 | +0.1 |
| 10 001 entries | refused: too many entries (checked from the end record before parsing) | 877 892 | 4.4 → 6.2 |
| duplicate names | refused: duplicate entry | 318 | +0.1 |
| `A.txt` + `a.txt` | **accepted** (paths are case-sensitive; no file system on the web, so no collision) | 318 | +0.2 |
| symlink entry | refused | 233 | +0.1 |
| nested zip bomb (stored `inner.zip`) | accepted as an opaque 20 KiB file; never extracted | 20 731 | +0.1 |
| encrypted flag | refused | 238 | +0.1 |
| bad CRC | opens; every read refused: CRC mismatch | 236 | +0.2 |
| bzip2 method | refused: unsupported method | 230 | +0.1 |
| ZIP64 forced / bad locator | ok / refused (offset outside the archive) | 279 / 156 | +0.1 |
| truncated CD / half / prepended / trailing / comment lie / empty / not a zip / two EOCDs | refused (end-of-central-directory checks) | 0 – 1 976 274 | ≤ +3.9 |
| 70 MiB honest stored bundle | refused under web limits (bundle too large); ok under default | 73 406 762 | **4.4 → 144.6** |

Findings:
* **Decompression is capped before inflating**: every bomb is refused from central-directory figures (ratio, size,
  count) or stopped by the live ratio/size check while inflating; peak growth for any bomb ≤ 1.5 MiB of linear
  memory (native allocator peak ≤ 0.8 MiB).
* **Linear memory never shrinks, and the loader copies twice**: a rejected 73 MB file still grows the module to
  144.6 MiB for the rest of the session (one copy from the JS `ArrayBuffer` into linear memory for `&[u8]`, a second
  `to_vec()` into the `Vfs`). Production path: take `Vec<u8>` by value from JS (one copy) and **check `File.size`
  in JS before copying** (reject above the web cap without touching linear memory).
* `File.arrayBuffer()` of the 73 MB file: 91 ms, check 103 ms (Chromium, this VM). Picker and drop produce the same
  `File` and the same results (66/66) — support both through one code path.
* pin_assets' own tests compiled for **wasm32-wasip1** under Node (`harness/run_wasip1_tests.sh`, one test per
  process because `panic=abort` stops a binary at its first panic; `out/zip/wasip1_tests.txt`): lib 43 pass;
  hardening 9 pass (+1 Node crash, below); hardening_round2 19 pass; vfs_zip 15; spec_vfs 26; fuzz 7; spec_fuzz 25;
  review_pr4 4; bundle 1 → **149 pass**. 15 use `std::env::temp_dir()`/folder bundles ("not supported by WASI yet")
  — the folder VFS, which the web does not have. 1 ignored (timing report). Two non-passes:
  `hardening_round2::props::zip64_mutations` — **real 32-bit portability bug in the test**: for a Zip64 count of
  2³² the loader refuses with `TooManyEntries(usize::try_from(count).unwrap_or(usize::MAX))` = 4 294 967 295 on
  wasm32, the test expects `count64 as usize` = 0 (truncated); the archive is refused identically, only the payload
  differs (queue line); `hardening::n4_too_many_entries_of_a_kind` (manifest validation, not zip) — **Node 22
  segfaults** (exit 139) intermittently with this test's large heap even with `--wasm-enforce-bounds-checks`; it
  passed in 1 of 3 runs (harness issue, S1 §12). Building them needed `proptest` without its `fork` feature
  (`wait-timeout` does not build for wasip1) — a scratch-only edit, reverted afterwards.

**Memory model and budgets (recommendation).** wasm32 linear memory is capped at 4 GiB and only grows; iOS Safari
is assumed to kill tabs around 1–1.5 GB (assumption for S5/S6, not measured). Production bundles are 1.9–2.0 MB
compressed and ≈ 2.4 MB uncompressed; the engine module sits at ≈ 4.4 MiB of linear memory before a bundle.
Recommended web limits: **bundle file ≤ 32 MiB (checked on `File.size` before any copy), `ZipLimits {
max_total_uncompressed: 64 MiB, max_file: 32 MiB, max_ratio: 100, max_entries: 10 000 }`** — 25× the production
tables, peak linear memory for a maximal bundle ≈ 32 + 64 + engine + GPU staging < 256 MiB. **Persistent storage:
none in the initial version** (a user bundle is re-picked each session; nothing untrusted is stored).

## 9. Design D — Worker + Atomics + terminate() (`web/worker_d.js`)

Prototype (pure JS, production `rules.wasm` uninstrumented, stub host): a "sim" Worker calls the rules Worker
synchronously through a `SharedArrayBuffer` (`Atomics.store/notify/wait`); every host import makes a round trip
back to the sim Worker; a hanging module is detected by the sim Worker's timed wait and the main thread calls
`terminate()`. Chromium 157, COOP/COEP (`out/browser/worker_d_iso.txt`):

| fact | measured |
|---|---|
| `Atomics.wait` on the main thread | `TypeError: Atomics.wait cannot be called in this context` → the sim would have to leave the main thread (contradicts S1's main-thread rAF recommendation) |
| without COOP/COEP | `SharedArrayBuffer is not defined` (design impossible without isolation) |
| `on_tick` round trip, apex / sky / feature (5 000 calls) | mean 7.1 / 10.9 / 8.7 µs, p99 25 / 45 / 40 µs (5 µs timer) — vs 1.1–1.6 µs mean for B in the same page; each host call adds one cross-thread round trip (≈ 5 µs) |
| watchdog on a module that never returns | timed wait returned `timeout` after 20.3 ms; `terminate()` 0.03 ms; the rules instance and its memory are gone (state lost; recreating it is a reload) |

Verdict: **rejected.** It cannot be made synchronous from the main thread; it needs isolation; the watchdog is
wall-clock, so whether a script survives depends on the device (non-deterministic — it can only be a backstop on top
of exact fuel, never the budget); and every host call costs a thread round trip. The instrumentation already bounds
every callback deterministically, so D adds nothing that A/B need.

## 10. Req 8 — recommendations

### 10.1 Design (informs proposal §4.5, Q6, Q10)

**Choose A: wasmi 2.0.2 compiled into the engine module, running the instrumented module, for every rules module
(the three curated tables and player bundles alike).** Configuration: features `std`, `validate`, `auto-dispatch`
(or `portable-dispatch`), `deterministic`; `CompilationMode::Eager`; `set_min_stack_height(256 KiB)`;
`StoreLimits` as pin_script's; wasmi's own fuel on as a **backstop** at `4 × budget + 4 Mi` so even an
instrumentation bug cannot loop forever; recursion/value-stack limits above the counter. The instrumentation
(fuel, depth counter, caps) is the single definition of the budget.

Why A over B (JIT): both are exact (§4, §5); A keeps the guest in Rust code that the existing native test suite can
exercise with `cargo test` (rule 3), has no JS on the host path, a fuel backstop, no dependence on the browser's
trap handler (which crashed the renderer on any OOB access in this VM) or on V8's `RangeError` semantics, and its
production cost (≈ 5 µs per tick, ≤ 0.1 ms per frame, ≤ 0.5 ms on a slow phone) is irrelevant. B's 3–8× speed only
matters for hostile modules, which the frame guard handles anyway. B's advantage — a separate V8-isolated instance,
so an engine bug cannot reach the sim's memory — is real; A's equivalent risk is a wasmi memory-safety bug inside
the engine module (contained by the browser's wasm sandbox to the page). **C (hybrid)** is the measured fallback if a
target phone shows A's production cost above 2 ms per frame: B for the three SHA-256-pinned curated modules, A for
everything else (implemented and passing: 161 parity lines, 82/82 adversarial). **D** rejected (§9). wasmtime/Pulley
is not a route (§7.2).

### 10.2 Keeping native and web kill points identical (very-high blast radius — decision needed)

**Recommend: native runs the same instrumented module under wasmtime** (pin_script keeps wasmtime and its config;
`consume_fuel` stays on as a backstop at a multiple of the budget; the instrumentation's fuel global and depth counter
decide the kill point, and the `why` global classifies the fault). Blast radius: **very high** — it changes the
untrusted-table security boundary on native (the code that enforces the budget and the stack limit becomes the
rewriter, and deep recursion faults at the counter: 2 338 frames instead of 5 453 for tiny frames). Evidence it is
safe for existing content: `native-instr-depth` reproduces the engine path on every tick of all 10 scenarios incl.
the 10-minute soak (§4), all 82 adversarial lines equal native today except the documented stack kill point (§5),
production fuel per callback is identical and peaks at 4.3 % of the budget, production depth peaks at 4 calls, native
callback cost +20–50 % (0.31→0.40 µs). Not done here: pin_script itself was not switched (the harness backend
`wasmtime_instr.rs` carries the evidence), so its stack-depth tests would need re-baselining.
Why not keep native as is: exact equality would then rest on the rewriter emulating Cranelift's internals for every
module shape (constant detection, SSA merges) — proven on the corpus and the production modules, but not for all
modules — plus the stack limit can never match (frame-layout and ISA dependent). With the instrumentation on both
sides, equality holds by construction. **Alternative** (also very-high): wasmi natively too (one engine everywhere;
native wasmi is 0.6 µs per callback); not preferred because wasmtime is the more scrutinised engine for native.

### 10.3 Budget rule

Keep **100 000 fuel per callback** and the 2 MiB memory cap on every platform; **depth counter limit 256 KiB of
estimated frame bytes** (weight `16·(params+locals+max operand stack)+64`); **web shell frame guard**: ≤ 8 ms of
script time per animation frame, then pause stepping until the next frame (deterministic: no tick is altered or
skipped from the recording's point of view; the sim only runs later); a "rules too slow" notice after 2 s of
throttling. Do not scale the budget per device. Separately (native, queue line): decide a per-tick aggregate cap
because up to 512 edges per tick multiply the per-callback budget.

### 10.4 Residual risk statement (for the issue)

With A + instrumentation + frame guard, a player-loaded bundle's rules can: (1) make its own game run in slow motion
or stop (a 100 000-fuel callback costs up to ~37 ms in wasmi-in-V8 here, up to ~185 ms on a slow phone; the tab
stays responsive between frames); (2) fault itself (tilt) in all the native ways; (3) differ from native only in the
**text** of a trap message if B is used for curated modules (V8 12.4). It cannot exceed 2 MiB of memory, grow tables
beyond 65 536, import anything but the 18 `pinball` functions, use SIMD/threads/GC/exceptions/tail calls/memory64/
multi-memory, or diverge from native replays (exact fuel, exact depth with 10.2, canonical NaN). Remaining exposure:
bugs in the rewriter (mitigated by the wasmi fuel backstop and differential tests), wasmi bugs (Rust; guest memory
is software-bounds-checked; an exploit stays in the page's wasm sandbox but could corrupt sim state), browser bugs
(contained by the renderer sandbox), and the bundle's other assets (glTF/audio decoders under `panic=abort` = tab
DoS, proposal TM-2). The page runs locally; nothing sensitive is reachable from its origin.

### 10.5 What `docs/THREAT_MODEL.md` must say (draft entries)

* **Scope / web build**: bundles come from the three curated `.pinbundle`s or from the player's file picker/drop;
  read with `File.size` checked against 32 MiB first, then `Vfs::from_zip_bytes` with web `ZipLimits` (64 MiB total,
  32 MiB per file, ratio 100, 10 000 entries); no persistence of player bundles.
* **Wasm rules sandbox (all platforms)**: pre-scan (D25 limits, proposal set), ABI checks, then a load-time rewrite
  that enforces the per-callback fuel budget (wasmtime's cost model, exact), a weighted call-depth limit and the
  memory/table caps; engine (wasmtime natively, wasmi on the web) keeps its own fuel as a backstop; NaN results are
  canonical; host functions validate every argument; any trap tilts the script.
* **Accepted risks**: TM-W1 rewriter bugs are part of the boundary (backstop + differential tests; revisit on any
  wasmtime/wasmparser bump); TM-W2 wasmi memory-safety bugs could corrupt sim state inside the page (revisit if wasmi
  publishes an advisory); TM-W3 hostile rules slow their own game (frame guard; revisit if a player-visible hang is
  reported); TM-W4 up to 512 callbacks per tick (native too; revisit with the per-tick cap decision); TM-W5 (only if
  C/B is used) JS glue and V8's trap handling are part of the boundary for curated modules.
* **TM-1 (folder TOCTOU)**: not applicable on the web.

### 10.6 Tests to add (names → assertion)

| test | asserts |
|---|---|
| `pin_script/tests/spec_instr_fuel_exact.rs` | for every callback of the three production modules over the recorded bot60 games, instrumented fuel used == wasmtime fuel used (today's harness check `fuel=wasmtime-exact`) |
| `pin_script/tests/spec_instr_differential.rs` (proptest + wasm-smith) | random valid modules within the D25 limits: outcome class, fuel, queue, RNG and memory digests equal between wasmtime-original and wasmi-instrumented; budget boundaries by bisection equal |
| `pin_script/tests/spec_instr_boundaries.rs` | the corpus boundaries: random loop 9 091/9 092, constant fill 893/894, constant-via-local 892/893, `id_of` 60 000-byte names, 2 MiB deferred fill exhausts |
| `pin_script/tests/spec_depth_counter.rs` | depth limits 2 338/711/229/78/15 for 0/16/64/200/1 000 live locals on every engine; production max depth ≤ 4 calls |
| `pin_script/tests/spec_nan_canonical.rs` | the 9 NaN cases return canonical bits on every engine |
| `pin_script/tests/spec_wasmi_alloc.rs` | `allocs_in_step == 0` over 60 000 ticks per table with the wasmi backend |
| `pin_script/tests/spec_web_sandbox.rs` | the 82-line adversarial corpus (load class, per-call outcome, fuel, queue, RNG, memory) identical between native and wasmi |
| web CI `web-rules-parity` (Node) | per-tick `state_hash`/`ext_hash`/command-queue hash of the wasm build == native engine path for 3 tables × {bot60, pat60, mb8}; fails on any difference |
| web CI `web-rules-adversarial` (Node) | the corpus run on the wasm build equals the native run line for line |
| web CI `web-wasmi-dispatch` | a 100 000-fuel back-edge loop completes as `BudgetExceeded` in the wasm build without `RangeError` (catches a lost `auto-dispatch`) |
| `pin_assets/tests/hardening_round2.rs` fix | `zip64_mutations` uses the same saturating conversion as the loader (passes on wasm32) |
| `pin_web` `bundle_picker_limits` (Playwright) | a 33 MiB file is refused before any copy (linear memory unchanged), the hostile corpus is refused exactly as natively |
| `pin_web` `csp_policy` (Playwright) | the page works under the recommended CSP and fails without `'wasm-unsafe-eval'` |
| `pin_web` `frame_guard` (Node, simulated clock) | a module using 99 999 fuel per callback never makes a frame run more than one callback past the 8 ms guard; replay hashes unchanged |

### 10.7 Other decisions informed

* **A20 / loading flow**: compile rules modules (if B/C) with `WebAssembly.compile` during bundle load; A needs none.
* **CSP**: the header in §7.1 for the local dev server.
* **Bundles from the player**: file picker and drag-and-drop through one `File` → `arrayBuffer()` path; size check
  before copy; pass ownership (`Vec<u8>`) into the loader (one copy); web `ZipLimits` as in §8; no IndexedDB in the
  initial version.
* **Q10 (user bundles)**: technically supportable with the measures above; the residual risk is §10.4.

## 11. Environment findings

* **F-env-1 Chromium wasm trap handler**: in this VM the Playwright Chromium renderer crashes ("Target crashed") on
  any wasm out-of-bounds memory access while the signal-based trap handler is on — bare V8, no engine code
  (`web/oob.html`, 4 variants incl. a 1-page memory with and without a maximum); Node traps normally;
  `--disable-features=WebAssemblyTrapHandler` makes Chromium trap correctly (`RuntimeError: memory access out of
  bounds`); `--js-flags=--no-wasm-trap-handler` and `--no-sandbox` do not help. Treated as an environment artefact
  (real devices: needs S5/web). Design A never relies on it for guest accesses (software bounds checks).
* **F-env-2 node:wasi**: Node 22.23.5 segfaults in `fd_write` for stdout/stderr with large guest memories (S1 §12);
  `node/wasi_test_runner.mjs` writes fds 1/2 itself. One pin_assets test (`n4_too_many_entries_of_a_kind`) still
  segfaults Node intermittently even with `--wasm-enforce-bounds-checks` (exit 139; passed 1 of 3).
* **F-env-3 proptest on wasip1**: proptest's default `fork` feature pulls `wait-timeout`, which does not build for
  wasm32-wasip1 (`cannot find module imp`); the wasip1 test run used `default-features = false, features = ["std",
  "bit-set"]` (scratch, reverted).
* **F-env-4 machine load**: other spikes (S2, S7) ran during every measurement (load 3–4 on 4 cores); timing maxima
  (up to 20 ms single callbacks in V8) are GC/scheduling outliers; medians and p99 are the figures to use.
* **Disk**: the target dir peaked at ≈ 7.7 GB (debug pin_script tests 4.2 GB); removed after use.

## 12. Commands index

```sh
. S/env.sh; cd S
./run_rules_parity.sh full          # builds native + wasm (+ wasm-bindgen), 161 PASS lines expected, exit 0
./run_rules_parity.sh quick nobuild # bot60 only
./run_rules_adversarial.sh          # 82-line corpus on 14 design/route combinations + PASS/FAIL table
harness/bench_rules.sh 3 && python3 -I harness/bench_summary.py out/bench
harness/final_runs.sh               # parity + adversarial + alloc guard + stack probes + max depth
python3 -I harness/make_zip_corpus.py S/zipcorpus S/bundles/apex_circuit_wasm.pinbundle
for f in zipcorpus/*.zip; do $B zip $f web; node node/run_web.mjs zip $f web; done
node node/chromium_files.cjs out/zip/chromium.web.txt web zipcorpus/*.zip
(cd repo && CARGO_TARGET_WASM32_WASIP1_RUNNER="node S/node/wasi_test_runner.mjs" \
  cargo test -p pin_assets --target wasm32-wasip1 --no-run --lib --test hardening --test hardening_round2 \
  --test vfs_zip --test spec_vfs --test fuzz --test spec_fuzz --test review_pr4 --test bundle) # proptest w/o fork, F-env-3
harness/run_wasip1_tests.sh
node node/chromium.cjs 'web/compile.html?iso=1' sizes '{"sizes":[4096,4194304,8388608,16777216]}' -
for c in none strict wasm unsafe rec; do node node/chromium.cjs "web/csp.html?csp=$c" csp '{}' -; done
node node/chromium.cjs 'web/worker_d.html?iso=1' d '{"n":5000,"timeoutMs":20}' -
node node/chromium.cjs 'web/oob.html' oob '{"variants":[[null,1,65533]]}' -     # F-env-1 (crashes); add
S4_CHROMIUM_ARGS=--disable-features=WebAssemblyTrapHandler node node/chromium.cjs 'web/oob.html' oob '{"variants":[[null,1,65533]]}' -
(cd repo && cargo test -p pin_script)   # native suite in the scratch copy: 471 passed, 0 failed, 5 ignored
cd pulley_check && CARGO_TARGET_DIR=$CARGO_TARGET_DIR/pulley cargo build --release --target wasm32-unknown-unknown
$B maxdepth bundles/<t>_wasm.pinbundle $S1/rec/<t>_bot60.scn ; $B stack <design> 2 ; $B instrument <in> <out> [depth] [jit]
```
`$B` = `$CARGO_TARGET_DIR/release/s4`; designs: `native`, `native-instr-depth`, `wasmi-fuel`, `wasmi-instr`,
`wasmi-instr-depth`, `jit`, `jit-depth`, `hybrid` (the JIT ones only via `node node/run_web.mjs …` or the page).
Tolerances for re-runs: hashes and adversarial lines must match exactly; timings vary with load (±50 % typical).

**Effort for the follow-up** (Phase 9 "web rules", replacing proposal 9.5's backend part): (1) move pre-scan, ABI
checks and host logic engine-neutral into pin_script (`wasmparser` direct) — M, 1–2 runs, T2; (2) the rewriter in
pin_script with the exact-fuel, depth, caps, NaN parts and the differential/proptest suite — L, 2–3 runs, T2
(security boundary, D34 (a)); (3) wasmi backend behind a `wasmi` feature + alloc/adversarial/parity tests — M, 1–2
runs, T2; (4) native switch to the instrumented module under wasmtime (10.2) — M, 1–2 runs, T2 with the
very-high-blast-radius sign-off; (5) web shell pieces (frame guard, bundle picker with size check, web
`ZipLimits`, CSP) — M, 1–2 runs, T2; (6) web CI parity + adversarial jobs — S–M, 1 run, T1. ≈ 8–12 implementer runs
plus the usual tester/reviewer runs.
