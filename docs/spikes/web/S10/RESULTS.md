# S10 — web-spike-engines: JavaScriptCore (Safari/iOS) and SpiderMonkey (Firefox) parity of the wasm modules (RESULTS)

Scratch root `S` = `${WEB_SPIKES}/S10`;
`S1`, `S2`, `S4`, `S5`, `S7`, `S8` = the sibling spike directories (read only; S9 not read). Real tree untouched
(`git -C /home/user/kilohertz-pinball status --porcelain` empty). Rebuilds (only the Lua module variants of §6/§8)
used the COMMON cargo env with `CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s10`, `CARGO_BUILD_JOBS=2`;
that directory was removed at the end. Fable was not available (D42 allowance unknown); all work at Opus depth.
Shell versions, checksums and tier flags: **`S/versions.md`**. Machine: 4 cores shared with S9 (load 6-10 during
the runs): wall times are indicative only.

## 0. Bottom line

* **Bit-for-bit parity holds on JavaScriptCore and SpiderMonkey, in every tier the shells have, for every module.**
  `run_engines.sh` ran 2 431 units on V8 (control), JSC 327409@main (IPInt, BBQ, OMG, default tiering, and no-JIT),
  SpiderMonkey 160.0.1 and 153.7.0esr (baseline, Ion, default lazy tiering) on x86_64, and SpiderMonkey 160.0.1 /
  153.7.0esr **linux-aarch64** under qemu (the only ARM browser engine obtainable here): **2 383 PASS, 0 unexplained
  FAIL, 16 rooted differences, 32 info lines** (`runs/summary.md`). That includes 457 sim replays (S1 `+simd128`, S7
  portable `+simd128`, S7 portable no-simd; 11 scenarios incl. the 600 000-tick soak) with **37.9 million per-tick
  `state_hash`+`ext_hash` comparisons, all identical to S1's native x86_64 hashes** (34.8 M on JSC/SM); the S2/S8 Lua
  module (KHZ 29-37) on the production replays (per step and per tick), the 13 S8 audit probes + pairs order, the
  79-case S2 sandbox corpus, budget-edge, the per-tick-cap scenarios, the S2 number/libm/ctype dumps (25 dumps,
  2.2 M libm results); and the S4 wasmi rules design (bot60 x 3, the 82-line adversarial corpus, the depth counter, the
  S8 per-tick cap). **No first differing tick exists anywhere.** The 16 rooted differences are all in S4's *JIT*
  rules design (not recommended): trap-kind labels parsed from engine-specific error texts (§4.3 D1).
* **Float micro-test (115 ops x 6 144 inputs, every float/conversion/SIMD op the modules contain, 0 relaxed-SIMD ops
  in any module): 0 value mismatches and 0 spec violations on all 16 engine/tier combinations.** The only bit
  differences are NaN sign/payload, which the Wasm spec leaves open: min/max NaN bits differ on every engine, ARM
  produces positive default NaNs, and **JSC's BBQ tier propagates the second operand's NaN payload for `add`/`mul`
  where its IPInt/OMG tiers (and V8/SM) propagate the first — so NaN bits can change inside one Safari session as
  code tiers up** (seen as in-process instability). No NaN ever reached hashed state; S1's R-4 (canonical NaN when
  hashing) becomes mandatory (§5).
* **The one engine difference that matters: stack.** JSC's baseline Wasm frames are 6-8x V8's. The deepest legal Lua
  C recursion (Lua's 200-level C-call limit, e.g. 200 nested `pcall`) needs **1 312 KiB of JSC stack in BBQ code
  (1 184 KiB default tiering) vs 176 KiB in V8 and 224 KiB in SpiderMonkey**; on a real 1 MiB thread JSC hits its
  stack limit first and the engine instance terminates (`Error: Module terminated`) where native Lua raises "C stack
  overflow". The JSC shell's 5 MiB budget passes with 3.9x margin; Safari's real budget on iOS is not measurable
  here. Fix found and verified (§8): **KHZ 38, `LUAI_MAXCCALLS` 200 -> 100** (worst JSC tier 728 KiB; a 1 MiB thread
  passes; all production replays unchanged on 5 engines) **plus a startup self-test** that refuses user bundles if
  even that does not fit. No engine lets Wasm `catch_all` catch its own stack overflow (all escape to JS).
* **EH encoding: recommend legacy EH** (as built). The v2 Lua module rebuilt with legacy EH and with exnref gives
  identical results on every engine/tier (21/21 each); exnref raises the floors (Safari 18.4, Firefox 131, Chrome 137)
  and its glue needs `WebAssembly.JSTag`; **JSC rejects modules that mix the two encodings** ("Module uses both legacy
  exceptions and try_table").
* **Minimum browsers implied by the modules** (wasm-tools feature removal, cross-checked with S5): sim modules
  `+simd128` and the Lua module -> **Safari/iOS 16.4, Firefox 100 (Lua; 89 without Lua), Chrome 96**; the no-simd sim
  -> Safari 15, Firefox 79. All far below the WebGPU floor (Safari 26), so WebGPU stays the binding floor.
* **Cost (indicative, §7):** with their JITs, JSC and SpiderMonkey run the sim at **0.93-1.21x V8** per tick, Lua
  callbacks at 1.05-1.14x and wasmi rules at 0.5-0.6x (faster than V8), so S1's budget (main-thread rAF batches,
  1-3 % of a 60 Hz frame at 1-3 balls) holds on Safari and Firefox. **JSC without a JIT (IPInt only) is 41-52x
  slower for the sim and 29-35x for Lua: not real-time** — detect and refuse.
* **Residual (needs a device):** Apple Silicon/iOS JSC (no ARM JSC build exists), Safari's real WebContent stack and
  memory limits, Lockdown Mode. Firm CI recommendation: a macOS runner job with real Safari (`safaridriver`) and the
  iOS Simulator plus a Firefox job, running the startup self-test, `apex_circuit_bot60` and `cstack_pcall_nest` (§9).

## 1. Owned assumptions and claims — verdicts

S10 owns the remaining engine half of "web replays equal desktop replays exactly" and the JSC/SM halves of the
proposal assumptions below (S1/S2/S4/S7/S8 measured V8 only).

| # | claim / assumption (JSC + SpiderMonkey part) | verdict | evidence (§) | reproducing command |
|---|---|---|---|---|
| C-parity | the same wasm modules give bit-identical results on JSC and SpiderMonkey to x86_64 native, every tier | **confirmed** (0 differing ticks/probes/cases in 2 383 units; 37.9 M per-tick comparisons) | §4, `runs/summary.md` | `./run_engines.sh all` |
| A12 | Wasm float arithmetic is IEEE deterministic except NaN bits; no FMA; no relaxed SIMD in the modules; browsers do not canonicalise NaNs | **confirmed for JSC and SM (x86_64, aarch64 SM)**, with one addition: NaN payloads also differ *between tiers of one engine* (JSC BBQ vs IPInt/OMG), not only between engines/CPUs | §5 | `ONLY=micro ./run_engines.sh jobs` then `python3 -I micro/compare_micro.py micro <engine>=runs/micro/<e>_<t>.bin …` |
| A25 | Wasm EH (as used by the Lua module) works in current browsers | **confirmed for JSC trunk and SM 160/153** (legacy and exnref both run the v2 module identically in every tier); **changed**: one module must not mix encodings (JSC CompileError) | §6.3 | `ONLY=eh ./run_engines.sh jobs`; `run_stackmicro.js` |
| A28 (engine stack part) | the module's own limits trip before the engine's stack | **changed**: true for V8 (5.6x margin at 984 KiB) and SpiderMonkey (shell quota 1 MiB, 4.6x); **for JSC only with >= 1.3 MiB of stack** (shell 5 MiB: 3.9x); on a 1 MiB thread JSC terminates the instance first; fixed by KHZ 38 + self-test | §6.2, §8 | `harness/stack_all.sh`; `harness/stack_need.py jsc bbq sandbox:cstack_pcall_nest` |
| A19 (JSC/SM part) | wasmi-in-wasm runs production rules well inside a frame | **confirmed** on JSC/SM JIT tiers; **refuted** for no-JIT JSC (IPInt) at the worst case | §7 | `harness/cost.sh` |
| A20 (shell part) | sync compile of large modules on the main thread is restricted | **not restricted in the shells** (sync `new WebAssembly.Module` of 3.4-8.5 MB modules works in jsc and js); browser main-thread policies need S5/web | §7 | `harness/cost.sh` (load rows) |
| A27 (JSC/SM part) | wasm tick cost 1-3x native | **confirmed for JIT tiers** (§7); **refuted for the interpreter** (IPInt / no-JIT JSC) | §7 | `harness/cost.sh` |
| S2-4 / S8 §11 residual | "JSC/SpiderMonkey not measured" | **closed** for the shells (x86_64 both, aarch64 SM); open only for Apple Silicon/iOS JSC and real browsers (§9) | §4, §9 | `./run_engines.sh all` |

## 2. Layout and reproduction

* `S/run_engines.sh [all|fetch|verify|refs|jobs|summary]` — the test entry point (brief: TESTS REQUIRED).
  `all` = fetch (download or reuse, SHA-256 against pinned values and the upstream checksum files, extract into
  new dirs) -> verify (`mods.sha256`) -> refs (`harness/make_refs.sh`: native x86_64 references made with S8's native
  binaries) -> jobs (`harness/job.sh` per unit, `xargs -P $PAR`) -> summary (`runs/summary.md`, exit 0 iff no
  unexplained FAIL). Env: `PAR` (default 2; the final run used 3), `ARM=0`, `ONLY="sim probe lua eh fix rules micro"`,
  `ENGINES=…`, `KEEP=1` (resume). The final full plan is 2 431 jobs (~1.5 h at PAR=3 on this loaded VM; the
  interpreter and qemu units dominate).
* `S/harness/` — `shellenv.js` (shell I/O, clocks, UTF-8 polyfill, CommonJS loader), runners `run_s1.js` (S1/S7 sim
  modules), `run_s8.js` (S2/S8 Lua module), `run_s4.js` (S4 wasmi module, ES-module glue via dynamic import), `eng.sh`
  (engine x tier -> command line), `job.sh` (one unit + comparison), `make_refs.sh`, `summarize.py`, `skip_done.py`,
  `known_diffs.txt` (rooted differences), `features.sh`, `stack_need.py`, `stack_all.sh`, `cost.sh`, `cost_table.py`,
  `selftest.js` (startup self-test prototype), `build_eh_variants.sh`, `build_ccalls_variant.sh`, `rss.py`.
* `S/micro/` — float micro-test (`gen_micro.py`, `micro.wat/.wasm`, `inputs.bin`, `ref.c`, `run_micro.js`,
  `compare_micro.py`, `nan_examples.py`, `inventory/` op inventories per module), feature-detect modules (`feat/`),
  stack micro-modules (`stack/`), trap-message module (`traps/`), Lua stack probes (`luastack/`).
* `S/mods/` — module copies (bytes the V8 runs used) and the S10 rebuilds; `S/mods.sha256`.
* `S/runs/` — `res/*.res` (one result line per unit), `summary.md`, `out/` (outputs; passing per-tick hash files
  are deleted after comparison, their SHA-256 kept in `manifest/`), `ref/` (native references), `micro/`, `stack/`,
  `cost/`, `res_extra/` (30 extra PASS units from the first plan, outside the final plan), `res_superseded/`
  (31 units invalidated by the harness fix H1 of §4.3 and re-run).
* `S/versions.md`, `S/DECISIONS_LOCAL.md`, `S/QUEUE_LINES.md`, `S/logs/`.

Module identity (brief req 2): none of the earlier spikes recorded module hashes, so identity is by provenance and
by re-running their unchanged V8 harnesses on the copies — the `v8/default` column of every table (S1
`harness/run_bindgen.mjs`, S8 `node/run.cjs` + `harness/run_node.cjs`, S4 `node/run_web.mjs` + `run_stack.mjs`,
S8 `node/s4/run_s4.mjs`): all PASS. The S10 rebuild of the legacy-EH v2 module equals S8's v2 instruction for
instruction (`wasm-tools print`, data segments excluded: 1 385 648 identical lines; only embedded source paths differ).

## 3. Requirement 1 — the shells

Details and checksums in `versions.md`. Summary:

| shell | build | integrity | Wasm tiers forced (verified) |
|---|---|---|---|
| JavaScriptCore | WebKitGTK `jsc` bundle, **WebKit 327409@main** (trunk, 2026-11-20), x86_64 only | zip SHA-256 `c21a415d…42f0` = server's `.sha256sum` | default (IPInt->BBQ->OMG: 465 BBQ + 18 OMG compiles on the fixture), `ipint` (0/0), `bbq` (1 121 BBQ, 0 OMG), `omg` (1 120 BBQ + 1 238 OMG), `nojit` (`--useJIT=0`, IPInt) |
| SpiderMonkey | jsshell **160.0.1** (release) and **153.7.0esr**, linux-x86_64 and linux-aarch64 | zips = release `SHA256SUMS`; `SHA256SUMS.asc` **good GPG signature**, Mozilla release key `14F2 6682 … D98F 0353` | default (`baseline+ion`), `--wasm-compiler=baseline`, `--wasm-compiler=optimizing` (`wasmCompileMode()`); no interpreter exists (`--no-jit-backend` -> no `WebAssembly` at all) |

* No aarch64 JSC build is published (webkitgtk.org `jsc-built-products/` and `built-products/`, wpewebkit.org `built-products/` list only `x86_64/`).
* Relation to shipping browsers: the SM shells are published in the signed Firefox 160.0.1 and 153.7.0esr release directories (the release builds' standalone engine). JSC
  trunk is newer than shipping Safari (26.x, 27.x lines; their exact WebKit build numbers: needs S5/web) but has the
  same three Wasm tiers as Safari (IPInt is Safari 26's in-place interpreter); it differs from Safari in port (GTK,
  Linux), CPU (x86_64 vs Apple Silicon), embedding (no WebContent process, no JIT entitlement/cage) and stack/memory
  limits (§6.2). `--useJIT=0` is the closest model of a JIT-less WebKit (Wasm stays available in IPInt, SIMD included).
* aarch64 SM ran under qemu-aarch64 8.2.2 (S7's cross sysroot; `-E LD_LIBRARY_PATH=<shell dir>`).

## 4. Requirement 2 — parity

### 4.1 Harness adaptation

The modules and their wasm-bindgen glue JS are the exact files of the V8 runs (§2); only the loader differs
(`shellenv.js`, DECISIONS_LOCAL `s10-loader`): CommonJS wrapper for the `--target nodejs` glue, dynamic `import()`
for S4's `--target web` glue, `read(path,'binary')` for inputs (recordings, bundles, hash files passed as files),
`writeFile`/`os.file.writeTypedArrayToFile` for outputs, a WHATWG UTF-8 `TextEncoder`/`TextDecoder` polyfill (neither
shell has them), `performance.now` from `preciseTime()` in JSC (its shell `performance.now` is 20 µs-quantised).
Per-tick hash files are the module's own output bytes, so `harness/compare.py` (S1's comparator, copied) diffs them
mechanically against `S1/native/<scn>.hashes`; S4 hash files are `cmp`'d against `S4/out/native/<scn>.engine.hashes`.

### 4.2 Results per section (PASS/total; full per-item tables in `runs/summary.md`)

| section | V8 | JSC default | JSC ipint | JSC bbq | JSC omg | JSC nojit | SM160 def/base/ion | SM153 def/base/ion | SM160 arm64 def/base/ion | SM153 arm64 def |
|---|---|---|---|---|---|---|---|---|---|---|
| 2a S1 `+simd128` sim (11 scenarios) | 11/11 | 11/11 | 11/11 | 11/11 | 11/11 | 10/10¹ | 11/11 x3 | 11/11 x3 | 11/11, 10/10¹, 10/10¹ | 11/11 |
| 2a S7 portable `+simd128` | 11/11 | 11/11 | 10/10¹ | 11/11 | 11/11 | – | 11/11 x3 | 11/11 x3 | 7/7, 4/4, 4/4 | 7/7 |
| 2a S7 portable no-simd | 11/11 | 11/11 | 10/10¹ | 11/11 | 11/11 | – | 11/11 x3 | 11/11 x3 | 7/7, 4/4, 4/4 | 7/7 |
| op probes (`glam_ops`, `glam_iso`, `quat_ops_iso`, `nan_bits`, `fp_ties`) vs V8 | 10/10 | 10/10 | 10/10 | 10/10 | 10/10 | 10/10 | 10/10 x3 | 10/10 x3 | 6/6 + 4 NaN-sign info, x3 | 6/6 + 4 info |
| 2b Lua v2 (KHZ 29-37) | 75/75 | 75/75 | 63/63² | 75/75 | 75/75 | 25/25 | 75/75 x3 | 75/75 x3 | 38/38, 25/25, 25/25 | 38/38 |
| 2b EH variants legacy / exnref | 21+21 | 21+21 | 17+17 | 21+21 | 21+21 | – | 21+21 x3 | 21+21 x3 | 17+17 (def) | – |
| 2c S4 rules (wasmi + JIT designs) | 20 + 1 R | 20 + 1 R | 16 + 1 R | 20 + 1 R | 20 + 1 R | 4 + 1 R | 20 + 1 R x3 | 20 + 1 R x3 | 14+1R, 5+1R, 5+1R | 14 + 1 R |
| micro (115 ops) | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 | 1/1 x3 | 1/1 x3 | 1/1 x3 | 1/1 |

¹ without the 600 000-tick soak (interpreter / qemu scope, DECISIONS_LOCAL `s10-scope`). ² without the uncapped
513-callback adversarial ticks. R = rooted difference (§4.3 D1). Every rules row also has 1 info line (the engine's own
recursion limit without the depth counter, §6.2).

What each 2b/2c row covers (fast tiers): Lua — `tickstats` on fixture + bot60/mb8 x 3 tables, cap off and 200 000
(S8 run_memcap_parity/run_tick_budget: per step tick/RNG/input digest/queue and per tick state+ext vs S1 native, plus
the per-tick unit statistics text identical to native), 13 audit probes + `pairs_order` (observable lines and the
accounted heap identical to native; S8 run_audit), S2 sandbox corpus 79/79 (outcome, fault tick, queue digests,
message digests, RNG), `budget-edge`, `sp-after-errors 20` (SP delta 0 after 40 000 `pcall` errors + 80 faults),
S2 dumps (`format` 10 000 floats x 23 conversions, `parse` 10 000 strings, `ctype`, `libm` 22 functions x 100 000:
all identical to native glibc build), adversarial ticks 6 mixes x {0, 512 edges} x {cap 200 000, uncapped}.
Rules — `play` bot60 x 3 for `wasmi-instr-depth` (per tick state/ext/cmd hashes, fuel digest = wasmtime's) and
`jit-depth`, the 82-line adversarial corpus for both, the depth-counter limits (2 338/711/229/78/15 frames),
S8's per-tick cap (bot60 x 3 with cap 200 000 vs S1 native; `s8adv` nop/host_tick x {0,512} x {0, 200 000}).

### 4.3 Differences found and their root causes

| # | where | first differing tick / case | root cause | class | resolution |
|---|---|---|---|---|---|
| D1 | S4 JIT design, adversarial corpus, every engine incl. V8 (V8 2 cases, JSC 4, SM 1) | `call_indirect_null`, `call_indirect_type_confusion`, `call_indirect_oob`, `trunc_nan`, `data_segment_out_of_bounds` | trap *kind* parsed from engine error texts, which are not standardised (`runs/traps_messages.txt`, minimal module `micro/traps/traps.wat`): JSC reports null and signature-mismatch `call_indirect` identically and a NaN conversion as "Out of bounds Trunc operation"; SM reports table, memory and data-segment errors all as "index out of bounds"; V8 12.4 merges null/mismatch. Fuel, queue, RNG and guest memory identical | real engine behaviour (message texts), not bits | rooted (`known_diffs.txt`); design A (wasmi) classifies traps itself: 82/82 EXACT on all 16 engine/tiers. Also without the depth counter SM classifies the engine stack overflow as `InternalError: too much recursion` (V8/JSC `RangeError`). Confirms S4's recommendation A |
| D2 | NaN bits (micro-test; probes `nan_bits`/`fp_ties`) | min/max with NaN (all engines vs V8), add/mul with two NaNs (JSC BBQ), every NaN-producing op on aarch64 (positive default NaN) | spec-allowed NaN nondeterminism | real engine behaviour, spec-conforming | INFO; R-4 canonical NaN hashing mandatory (§5) |
| D3 | stack exhaustion point (deep Lua C recursion) | `cstack_pcall_nest` at <= 1.18-1.31 MiB JSC stack | JSC baseline frames 6-8x V8's (§6.2) | real engine behaviour | KHZ 38 + self-test (§8) |
| H1 | SM shell output of Lua probes / adversarial corpus | `d_format` truncated at `int %c`; `bad_magic` | harness: SpiderMonkey's `putstr`/`print` stop at the first NUL character (Lua `%c` of 0; WAT parser error echoing `\0asm`) | JS shell I/O artefact | fixed (`--out=<file>` in `shellenv.js`); 31 affected units re-run (all PASS), originals in `runs/res_superseded/` |
| H2 | `run_s8.js` | dumps | runner lacked the S2 dump commands at first | harness | fixed, re-run |

### 4.4 Rooted differences — reproduction

`node micro/traps/run_traps.js micro/traps` and `harness/eng.sh <engine> default micro/traps/run_traps.js micro/traps`
print the RuntimeError class and text per trap kind (V8, JSC, SM 160, SM 153 in `runs/traps_messages.txt`).

## 5. Requirement 2 — float micro-tests

`micro/gen_micro.py` generates one function per operation (115 ops: scalar f32/f64 add sub mul div min max copysign
abs neg sqrt ceil floor trunc nearest, the 6 comparisons, load/store and reinterpret round trips, demote/promote,
the 8 int->float and 8 saturating float->int conversions, guarded trapping truncations, f32x4/f64x2 add sub mul div
min max pmin pmax compare abs neg sqrt ceil floor trunc nearest, promote_low/demote_zero, i32x4<->f32x4
conversions, lane splat/extract/replace, bitselect): the union of every float/conversion/SIMD operator found in the
five modules (`micro/inventory/union_ops.txt`, 92 ops; 0 relaxed ops anywhere) plus close relatives. Inputs: 55x55
f32 and 58x58 f64 special-value pairs (+-0, +-inf, subnormals, extremes, rounding halfway points, conversion
boundaries, 11 NaNs per width with different signs, payloads, quiet/signalling; 18 i32 and 17 i64 integer specials) + 3 kinds of random pairs; 6 144 records.
Reference: `micro/ref.c` (native x86_64, SSE scalar, `-O0 -ffp-contract=off -frounding-math`, Wasm semantics
coded explicitly). Each op runs 21 times in-process (first pass vs last pass checked: catches tier-dependence).

| engines | records exact | NaN-bits-only | MISMATCH | SPEC violations | ops with NaN-bit differences |
|---|---|---|---|---|---|
| V8 x64 | 697 500 | 9 060 | **0** | **0** | min/max (scalar, SIMD) |
| JSC ipint / omg / nojit | 697 908 | 8 652 | **0** | **0** | min/max |
| JSC bbq | 697 500 | 9 060 | **0** | **0** | min/max + f32/f64 add, mul (102 records each) |
| JSC default | 697 602 (run-dependent) | 8 958 | **0** | **0** | as bbq for whichever functions were in BBQ |
| SM 160/153 x64, all tiers | 697 934 | 8 626 | **0** | **0** | min/max (scalar, SIMD) |
| SM 160/153 aarch64, all tiers | 685 780 | 20 780 | **0** | **0** | every NaN-producing op (ARM default NaN `7fc00000` vs x86 `ffc00000`) |

(`logs/micro_all.txt`; `python3 -I micro/compare_micro.py micro v8_default=runs/micro/v8_default.bin …`.)

* 87 of 115 ops are byte-identical (NaN bits included) across all 16 engine/tiers; the other 28 differ only in NaN
  bits. All non-NaN results (rounding, subnormals, +-0, conversions, saturation, nearest ties-to-even, pmin/pmax and
  every bitwise op incl. NaN payload transparency of abs/neg/copysign/load/store/lanes) equal the reference everywhere.
* JSC tier dependence (minimal repro: record 1 513 `f32.add(0x7fc00000, 0xffc00000)` -> V8/SM/JSC-IPInt/OMG
  `7fc00000`, JSC-BBQ `ffc00000`; `python3 -I micro/nan_examples.py micro f32.add,… v8=… jscB=…`): BBQ evaluates
  `a+b` as `b+a`. The micro runner saw the result change within one process in JSC default and omg tiering
  (`runs/micro/jsc_{default,omg}.log`: `TIER-INSTABILITY f32.add record 1513` etc.). Spec-conforming (NaN payload is
  nondeterministic), so not a bug to report; but a NaN reaching hashed state could hash differently between two
  ticks of the same Safari session.
* Real-module probes agree: S1 `glam_ops`, S7 `glam_iso`/`quat_ops_iso` digests (the SIMD-backed quaternion math the
  sim uses) equal V8's on every engine and tier incl. aarch64; `nan_bits`/`fp_ties` differ only in NaN sign on
  aarch64 (`0/0 = 7fc00000`).
* Consequence: parity rests on "no NaN in hashed state" (true in every run) plus S1 R-4 (hash every NaN as one
  pattern); R-4 must cover every hashed float and is required, not optional (queue line).

## 6. Requirement 3 — features, stack, minimum versions

### 6.1 Features each module needs (`harness/features.sh` -> `logs/features.md`; `wasm-tools validate --features=all,-<f>`)

| module | needs (removing any one fails validation) | EH ops | SIMD ops | memory | imports |
|---|---|---|---|---|---|
| S1 sim `+simd128` | sat-float-to-int, sign-ext, reference-types, multi-value, bulk-memory, **simd128** | none | 27 210 (0 relaxed) | 19 pages, no max, not shared | 4 glue functions |
| S7 portable `+simd128` | same | none | 27 166 | 19 pages | same |
| S7 portable no-simd | sat-float-to-int, sign-ext, reference-types, multi-value, bulk-memory | none | 0 | 19 pages | same |
| S8 v2 Lua (as built) | above + simd128 + **legacy exceptions** (+ tags) | 121 legacy (`try/catch/catch_all/rethrow`), 0 `try_table`; 1 tag + 2 imported tags | 29 016 | 69 pages | glue + `__wbindgen_jstag`, `__wbindgen_wrapped_jstag` (tags) |
| S10 v2 exnref rebuild | above with **exnref** instead of legacy | 38 `try_table`, 0 legacy | same | 69 pages | glue uses `WebAssembly.JSTag` |
| S4 wasmi rules | as S1 `+simd128` | none | 40 558 | 71 pages | glue + khzRules |

(`gc-types`/`floats` appear in wasmparser's list only as its internal names for "reference types exist" and "floats";
mutable-global, tail-call, threads, memory64, multi-memory, extended-const, GC are **not** needed.) JS API needed by
the glue: `WebAssembly.Tag`/`Exception` (EH builds), `TextEncoder/Decoder`, externref tables. Feature-detect run in
every shell (`micro/featdetect.js`, `logs/featdetect.txt`): all 17 feature modules validate and run (incl.
relaxed-SIMD, exnref, tail calls, memory64, GC) in JSC (all tiers, also `--useJIT=0`), SM 160/153 x64 and arm64;
`WebAssembly.JSTag` present everywhere. Memory grows to 65 281 pages (3.98 GiB) in all shells (`runs/memgrow.txt`;
mobile limits need a device).

### 6.2 Stack

Engine behaviour on deep recursion inside Wasm (`micro/stack/run_stackmicro.js`, `runs/stack/engine_limits.txt`):

| engine | error past the limit | catchable by Wasm `catch_all`? | frames of a 0-local recursion at the shell's default stack | native overflow possible? |
|---|---|---|---|---|
| V8 (Node, 984 KiB) | `RangeError: Maximum call stack size exceeded` | no (escapes to JS, legacy and exnref) | 15 686-25 098 (varies run to run: tier-up) | not with Node's default; with a small `--stack-size` Node sometimes aborted (exit 134) instead of throwing |
| JSC (5 MiB on an 8 MiB thread) | `RangeError: Maximum call stack size exceeded.` | no | BBQ 9 676, IPInt 29 033, OMG 638 609 (inlining) | no: limits follow the real thread bounds (`ulimit -s 256..8192` all end in RangeError) |
| SpiderMonkey (fixed shell quota ~1 MiB: `ulimit -s 1088` still throws, `1024` crashes; `runs/stack/sm_quota.txt`) | `InternalError: too much recursion` | no | baseline 16 239, Ion 32 479 | **yes in the shell**: with a thread < quota (`ulimit -s 1024`) deep recursion is SIGSEGV (exit -11) |

Smallest stack at which each probe still gives the native result (`harness/stack_all.sh` -> `runs/stack/needs.txt`;
V8 `--stack-size` KiB, JSC `--maxPerThreadStackUsage` KiB, SM `ulimit -s` KiB = real stack use):

| probe | V8 | JSC default | JSC ipint | JSC bbq | JSC omg | JSC nojit | SM160 default | SM160 baseline | SM160 ion | SM153 default |
|---|---|---|---|---|---|---|---|---|---|---|
| Lua `cstack_pcall_nest` (200 C levels) | 176 | 1 184 | 664 | **1 312** | 416 | 664 | 216 | 224 | 152 | 216 |
| Lua `cstack_tostring` / `sort` / `gsub` | 160/160/136 | 736/824/768 | 600/600/536 | 1 184/1 184/984 | 496/512/376 | 600/600/536 | 192/200/168 | 200/192/176 | 136/144/128 | 192/192/168 |
| Lua `deep_nesting_load` (parser) | 48 | 344 | 248 | 416 | 200 | 248 | 72 | 72 | 72 | 72 |
| Lua `baseline` (no recursion) = floor | 48 | 160 | 144 | 160 | 152 | 144 | 72 | 80 | 72 | 72 |
| sim fixture (10 000 ticks) | 56 | 160 | 160 | 176 | 176 | 160 | 72 | 72 | 72 | 72 |
| production Lua replays (bot60 x 3, mb8) | 48 | – | – | 176 | – | – | – | – | – | – |
| S4 `wasmi-instr-depth` counter probe | 56 | 152 | 144 | 152 | 144 | 144 | 72 | 72 | 72 | 72 |
| S4 `jit-depth` counter probe | 176 | 656 | 648 | **1 488** | 1 488 | 648 | 224 | 224 | 184 | 224 |

* Per C level (pcall nesting, `micro/luastack/pcall_nest_{25..190}.lua`, `runs/stack/pcall_nest_scaling.txt`): JSC
  BBQ 5.9 KiB, JSC OMG 1.6 KiB, V8 0.8 KiB; floor ~150 KiB in JSC (reserved zones count inside the budget).
* Real thread check (`runs/stack/jsc_thread_stack_check.txt`): `ulimit -s 1024` and `1280` -> JSC + v2 module
  `TRAP: Error: Module terminated` on `cstack_pcall_nest`; `1536`/`2048` pass. The failure is contained (only that
  instance dies; S2/S1's "re-instantiate on Module terminated" applies) but it is a parity break: natively that
  script gets a Lua "C stack overflow" error and the game continues with scripts faulted.
* Margins at the shells' defaults: V8 984/176 = 5.6x; JSC 5 120/1 312 = 3.9x; SM quota ~1 024/224 = 4.6x. The sim
  itself, production Lua and the wasmi design need only the floor (<= 176 KiB everywhere). Safari's and Firefox's
  real budgets (WebContent / content-process main thread, Workers) are **not measurable here** (needs a device/S5).
* Not a code-size effect: `wasm-opt 132 --coalesce-locals --reorder-locals` and `-O2` on the Lua module leave the JSC
  BBQ need at 1 312 / 1 328 KiB (`mods/s8_v2_wopt_*`, experiment only); the frame size is intrinsic to JSC's
  baseline compiler.
* The S4 depth counter (sized for wasmtime/V8 frames) is not conservative for JSC baseline frames: the JIT design
  needs 1.49 MiB of JSC stack before the counter trips first; the wasmi design is independent of engine frames.
  Further evidence for S4's choice of design A.

### 6.3 Minimum browser versions and the EH encoding

| module | binding features | Safari/iOS | Firefox | Chrome | source |
|---|---|---|---|---|---|
| S1 sim `+simd128` / S7 portable `+simd128` / S4 wasmi | simd128, reference-types, bulk-memory | **16.4** (simd) | **89** (simd) | 96 (reference-types) | S5 table; sign-ext and sat-float-to-int are not in S5's table (needs S5/web; older than simd in every engine by general knowledge) |
| S7 portable no-simd | reference-types, bulk-memory | **15** | **79** | 96 | S5 |
| S8 v2 Lua, legacy EH (recommended) | + legacy EH | **16.4** (simd; EH 15.2) | **100** (EH) | 96 | S5 |
| v2 Lua with exnref | + exnref, `WebAssembly.JSTag` | **18.4** (WebKit bug 293340, S5 medium) | **131** | **137** | S5 |
| the product (WebGPU) | WebGPU | 26 | 141 (Win) / 145-147 (mac) | 113 | S5 |

Cross-check with S5: no mismatch found — every feature S5 lists for these engines is present and working in the
current shells; the floors come from S5's data (the shells cannot test old versions). The WebKit exnref bug
(293340) was not reproducible in trunk 327409 (the exnref module passes 21/21 in every JSC tier), which does not
clear shipping Safari 18.4-26.

**EH recommendation: keep legacy EH for the Lua module** (S2's choice, now with JSC/SM evidence): identical behaviour
to exnref on every engine/tier (lua_eh_legacy = lua_eh_exnref = 21/21 PASS each on 12 engine/tiers + 17/17 on IPInt and
arm64; `sp-after-errors`: SP delta 0 on both), lower floors (Safari 15.2/Firefox 100/Chrome 95 vs 18.4/131/137),
no `JSTag` dependency, and the S5 note on a Safari exnref bug. Keep the module single-encoding (JSC refuses mixed
modules: `CompileError: Module uses both legacy exceptions and try_table`, `micro/stack/stackmicro.wat` +
`stackmicro_exnref.wat`); add a CI check. Revisit when LLVM/wasm-bindgen drop legacy EH.

## 7. Requirement 4 — cost (indicative only)

`harness/cost.sh` (fast tiers 3 reps, interleaved with Node V8 on the same machine; `jsc ipint/nojit` 1 rep);
`python3 -I harness/cost_table.py runs/cost` -> `runs/cost/cost_tables.md` (all rows). Load average during the fast
reps 5-8 (S9 ran Chromium beside it), during the interpreter rep 5 -> 1.6: numbers are indicative, ratios to V8 are
the useful part. Peak RSS of the whole shell process: 52-116 MB on every engine for the sim.

Sim per tick (S1 `bench`, steady state, µs, median of reps):

| scenario | metric | V8 | JSC default | JSC omg | JSC bbq | JSC ipint | JSC nojit | SM160 default | SM160 ion | SM160 baseline | SM153 default |
|---|---|---|---|---|---|---|---|---|---|---|---|
| apex bot60 (1 ball) | mean / p99 | 6.6 / 19.1 | 6.2 / 19.5 | 5.7 / 16.6 | 11.2 / 26.6 | 272.8 / 488.8 | 265.3 / 508.5 | 7.6 / 24.9 | 7.2 / 21.0 | 10.7 / 27.1 | 8.2 / 25.9 |
| apex bot60 | 16-tick batch p99 | 183 | 174 | 169 | 247 | 7 601 | 7 526 | 213 | 239 | 325 | 220 |
| feature bot60 (1-3 balls) | mean / p99 | 7.6 / 22.4 | 7.7 / 26.4 | 7.6 / 25.4 | 13.1 / 37.6 | 326.3 / 740.2 | 329.2 / 761.2 | 9.2 / 31.0 | 7.6 / 22.0 | 12.5 / 34.2 | 8.5 / 27.1 |
| sky mb8 (6-9 balls) | mean / p99 | 73.8 / 149.6 | 69.0 / 128.7 | 73.6 / 149.9 | 112.3 / 170.9 | 3 836 / 6 610 | 3 759 / 5 904 | 74.8 / 139.2 | 79.4 / 158.9 | 127.3 / 240.0 | 82.5 / 168.0 |
| sky mb8 | 16-tick batch p99 | 1 599 | 1 602 | 1 824 | 2 979 | 96 442 | 90 515 | 1 635 | 2 235 | 3 621 | 1 610 |
| **mean / V8** | (3 scenarios) | 1 | **0.93-1.01** | 0.87-1.00 | 1.52-1.72 | **41-52** | 40-51 | **1.01-1.21** | 1.00-1.09 | 1.63-1.72 | 1.11-1.24 |

Lua callbacks (S2 `play-timed`, µs, mean / p99): V8 `on_tick` 3.5 / 17.7 (apex bot60) and 4.8 / 23.2 (sky mb8); JSC
default 3.9 / 22.9 and 5.1 / 28.6 (**1.05-1.12x V8**), JSC bbq 1.38-1.59x, JSC ipint/nojit 123-143 µs (**29-35x**);
SM160 default 3.7 / 20.0 and 5.5 / 28.8 (**1.06-1.14x**), SM baseline 1.40-1.54x. `on_switch` means 2.4-5.5 µs on the
JIT tiers except where the first calls run in BBQ/OMG compile (JSC omg 18-31 µs, compile-dominated); `on_init`
0.04-2 ms (JSC omg 11-18 ms: eager OMG compile).

Wasm rules in wasmi (S4 `bench`, design A): production `on_tick` callback mean V8 2.9-3.4 µs, **JSC default 1.5-1.9 µs,
SM default 1.6-1.9 µs (0.53-0.62x V8: faster than V8)**, JSC bbq 2.4-2.8, ipint/nojit 43-47 µs (14-15x). Worst-case
hostile callback (16 bodies, each run to the 100 000-fuel budget; max over bodies of the median): V8 10.3 ms, JSC
default **49.3 ms** (`loop_br_only`: the first hostile calls run in IPInt before BBQ), JSC omg/bbq 4.3-5.6 ms, SM160
3.5-8.3 ms, JSC ipint/nojit **91-96 ms**.

Module compile + instantiate (sync `new WebAssembly.Module`/`Instance`, ms): sim 3.36 MB: V8 10.6, JSC 25-42,
SM 33-43 (SM ion-only 463: eager Ion); sim+Lua 4.08 MB: V8 18, JSC 33-63, SM 44-55 (ion 513); sim+wasmi 8.47 MB:
V8 28, JSC 71-133, SM 76-100 (ion 1 071). The shells place no limit on synchronous compilation of an 8.5 MB module.

Feasibility of 1 kHz + rules (extrapolated from these ratios and S1's V8 numbers; desktop x86 shells, phones slower):
* **JIT tiers (Safari/Firefox as shipped): same budget as V8.** JSC default and SM run the sim at 0.9-1.2x V8 and Lua
  at 1.0-1.1x, so S1's conclusions carry over unchanged: 1-3 balls cost ~6-13 µs per tick (16-tick batch p99 0.17-0.51
  ms, 1-3 % of a 60 Hz frame); 9 balls ~70-130 µs per tick (batch p99 1.6-3.6 ms, 10-22 %). Headroom of the 1 ms tick
  at 1-3 balls: 75-160x here, 15-50x on an assumed 3-5x slower phone; the 9-ball stress fits a phone frame only with
  the S1 rAF-batch design and a per-frame cap. JSC's BBQ tier alone is 1.5-1.7x (the start of every session runs there).
* **JSC without JIT (IPInt only: `--useJIT=0`, any JIT-less WebKit): not real-time.** 265-330 µs per 1-ball tick +
  120-140 µs Lua = 0.4-0.5 ms per tick: 16 ticks = 7-8 ms per 60 Hz frame on this desktop (40-50 %), 20-40 ms on a
  phone; 9 balls 3.8 ms per tick. Recommendation: detect it (startup self-test duration, or a 2 000-tick timing probe)
  and show "this browser runs WebAssembly without a JIT; the 1 kHz simulation cannot run in real time" (slow motion
  is deterministic but not playable). A hostile rules callback then costs ~95 ms (S4's 8 ms frame guard still bounds
  the stall per frame).
* The worst JSC case with JIT is a hostile wasmi callback's first execution in IPInt (49 ms here): S4's frame guard
  (stop stepping after 8 ms of script time in a frame) keeps the page responsive; no parity effect.

## 8. Requirement 5 — fixes

No fix is needed for bit parity: nothing differs. Two engine-driven changes are recommended; both keep x86_64 native
and V8 results of the production tables unchanged.

1. **KHZ 38: `LUAI_MAXCCALLS` 200 -> 100 on every target** (one `-D`; `llimits.h:254` honours a predefined value;
   prototype `harness/build_ccalls_variant.sh` = the v2 build + `CFLAGS_wasm32_unknown_unknown=-DLUAI_MAXCCALLS=100`,
   module `mods/s10_v2_ccalls100`). Measured:
   * Stack need of the deepest legal recursion: JSC bbq **728 KiB** (was 1 312), default 680, ipint 400, omg 296;
     V8 96 (was 176); SM 128 (was 216-224) (`runs/stack/needs_ccalls100.txt`). A real 1 MiB thread now passes
     (`jsc_thread_stack_check.txt`, `runs/selftest.txt`).
   * Production unchanged: fixture + bot60/mb8 x 3 tables replay per step and per tick identically to S1 native on
     V8, JSC default/bbq/omg, SM160 default/baseline, SM153 (`lua_ccalls100` 8/8 each in `runs/summary.md`); the
     sandbox corpus is identical across all these engines and differs from v2 only in the 6 `cstack_*` cases that
     hit the limit (traceback text) — `runs/ccalls100/summary.txt`.
   * Native: the change moves only scripts that nest more than 100 C calls (pcall/metamethod/sort/gsub chains or
     > 100 syntactic levels); production tables use ~2 (they run in the 176 KiB JSC floor). **Blast radius: very
     high** (vendored Lua, native replay semantics of such scripts; D34): owner decision, ship with KHZ 33-37.
     Not re-built natively here (the limit is a count, identical by construction on every target, as S8's audit
     showed for every count-based limit); the follow-up's native suite + S2 corpus will show the 6 changed lines.
2. **Startup self-test** (`harness/selftest.js`, `runs/selftest.txt`): before any bundle loads, (a) replay the CI
   fixture (10 000 ticks) and compare the final `state_hash`/`ext_hash` with `25097c21c2c3adc1`/`643e148850cf462b`,
   (b) compare the 15 `glam_ops` digests with the `+simd128` constants, (c) in a separate Lua instance run the
   deepest legal C recursion (`cstack_pcall_nest`) and require Lua's own result. Cost on this VM: 0.3-0.5 s in the default and
   baseline tiers of V8/JSC/SM (1.5-2 s in the eager-compile `omg`/`ion` settings, 2.6-3.6 s for (a)+(b) in the
   interpreter, 6.3 s on aarch64 SM under qemu); PASS on every engine/tier. With a 1 MiB JSC thread and today's
   limit (200) it reports `lua_cstack=INSUFFICIENT(Module terminated) -> refuse user bundles (included tables
   only)`; with KHZ 38 it passes. Recommended in the web shell (queue line); run (c) twice so BBQ code is in place.
   In production the fixture and constants are embedded in the engine module (~250 KB of recorded script stream in
   this spike's format; a native-format input recording is ~5 KB).

Native and V8 unchanged: no shipped module was modified for parity; the rebuilt modules are the KHZ 38 candidate
and the EH variants, and every production result of those equals S1's native hashes on V8 (Node, unchanged S8
harness) and on JSC/SM. Fixes inside the harness only (H1, H2 in §4.3).

## 9. Requirement 6 — residual claim table and CI recommendation

| promise | V8 (x64) | JSC (x64 shell) | SpiderMonkey (x64 shell) | SpiderMonkey arm64 (qemu) | V8 arm64 (S7, qemu) | Safari/iOS on Apple Silicon |
|---|---|---|---|---|---|---|
| web = desktop replay exactly (3 tables, fixture, soak; Lua and Wasm rules) | confirmed (S1/S2/S4/S8, re-run here) | **confirmed** all 5 tier settings | **confirmed** all 3 tiers, release + ESR | **confirmed** (soak in default tier) | confirmed (S7) | not measurable here |
| … for any user bundle (S8 KHZ 33-37 promise) | confirmed (S8, re-run) | **confirmed** for count/byte/format/libm/pairs/heap (§4.2) **if the stack holds**: **changed** -> needs KHZ 38 or >= 1.3 MiB stack | **confirmed** | **confirmed** | confirmed (S8) | not measurable (stack budget unknown) |
| Lua sandbox caps hold (budget, 2 MiB heap, C-stack limit, string caps, corpus 79/79) | confirmed | **confirmed** (corpus 79/79 every tier); C-stack limit **changed**: instance terminates first below 1.3 MiB | **confirmed** | **confirmed** | confirmed (S8 probes) | not measurable |
| wasmi fuel/depth hold (S4 design A) | confirmed | **confirmed** (82/82, fuel = wasmtime's, counter limits identical) | **confirmed** | **confirmed** | – | not measurable |
| JIT rules design (S4 B) parity | trap labels differ (2) | trap labels differ (4); counter needs 1.49 MiB in BBQ | trap labels differ (1) | same | – | – |
| stack budget adequate | 5.6x | 3.9x at 5 MiB; **insufficient at 1 MiB** without KHZ 38 | 4.6x (shell quota) | same | – | **unknown** |
| NaN bits never reach hashed state | confirmed | confirmed (but NaN payloads tier-dependent) | confirmed | confirmed (positive NaN) | confirmed | – |

What still needs a real Safari/iOS device or a real Firefox build: Apple Silicon JSC codegen (no ARM JSC shell
exists), Safari's WebContent main-thread and Worker stack budgets (decides KHZ 38's necessity and margin), iOS
memory/jetsam limits and Lockdown Mode (Wasm availability), Safari 18.4-26 exnref behaviour, Firefox's content-process
stack quota, and browser main-thread compile policies.

**CI recommendation (one firm choice):** add a `web-engines` job on a **GitHub-hosted macOS (Apple Silicon) runner**
that (1) runs the web build in **real Safari via `safaridriver`** (`sudo safaridriver --enable`; WebDriver) and in
the **iOS Simulator's Safari** (`xcrun simctl`), and (2) in **Firefox release** (Playwright's Firefox or geckodriver),
each executing the startup self-test, `apex_circuit_bot60` (per-tick hashes compared with the committed native
file) and `cstack_pcall_nest`, and recording the self-test verdict and per-tick time; weekly and on changes to
`third_party/lua-src`, glam, the web host or the toolchain. Keep `run_engines.sh` (JSC/SM shells, cached and pinned)
as the fast Linux gate on PRs touching the sim, Lua or the rules engine. Do one manual real-iPhone run (or a device
cloud) per release for the stack/memory numbers the simulator cannot give.

## 10. Effort estimate for the follow-up

* CI: Linux `run_engines.sh` subset (fast tiers, sim + Lua corpus + wasmi corpus + micro, ~10 min) — Sonnet 5.5,
  1 agent-run; macOS Safari/iOS-Simulator/Firefox job — Sonnet 5.5, 1-2 agent-runs.
* KHZ 38 — part of the KHZ 33-37 implementer run (Opus 5.5, very-high blast radius), +0.5 run; tests: the S2 corpus
  goldens for the 6 `cstack_*` cases, a JSC stack test with `--maxPerThreadStackUsage=1048576` in CI.
* Startup self-test in the web shell — Sonnet 5.5, 1 agent-run (after 9.5/9.10).
* R-4 canonical NaN for every hashed float (already queued by S1/S7) — Sonnet 5.5, 1 agent-run.
* Single-EH-encoding import/opcode check in the web CI — part of the first item.
Total ≈ 4-5 agent-runs, T2 (KHZ 38 rides the T2/Opus Lua batch).

## 11. Commands index

```sh
S=${WEB_SPIKES}/S10
$S/run_engines.sh all                         # fetch+verify+refs+2 431 jobs+summary (PAR=3 ~1.5 h here); exit 0 = no unexplained FAIL
ONLY="sim" ENGINES="jsc:default sm160:default" ARM=0 $S/run_engines.sh jobs && $S/run_engines.sh summary   # quick subset
$S/harness/eng.sh jsc bbq $S/harness/run_s1.js $S/mods/s1_simd play <bundle> <scn> <scr> <out.hashes>     # one replay
python3 -I $S/micro/gen_micro.py $S/micro && wasm-tools parse $S/micro/micro.wat -o $S/micro/micro.wasm && \
  gcc -O0 -ffp-contract=off -frounding-math -o $S/micro/ref $S/micro/ref.c -lm && $S/micro/ref $S/micro          # micro set + reference
$S/harness/features.sh > $S/logs/features.md ; node $S/micro/featdetect.js $S/micro/feat                     # §6.1
$S/harness/stack_all.sh ; python3 -I $S/harness/stack_need.py jsc bbq sandbox:cstack_pcall_nest              # §6.2
LUAMOD=s10_v2_ccalls100 python3 -I $S/harness/stack_need.py jsc bbq sandbox:cstack_pcall_nest               # §8
$S/harness/build_eh_variants.sh ; $S/harness/build_ccalls_variant.sh 100                                     # rebuilds (target/web_s10)
ETS="v8:default jsc:default sm160:default" $S/harness/cost.sh 3 && python3 -I $S/harness/cost_table.py $S/runs/cost
node $S/harness/selftest.js $S/mods/s1_simd <replay_table.pinbundle> <fx.scn> <fx.scr> $S/mods/s8_v2/pkg_node
```

## 12. Cleanup and tree state

* `git -C /home/user/kilohertz-pinball status --porcelain` -> empty; only read-only git use (`status`). No file of the
  real tree, of S1/S2/S4/S7/S8 or of their tool directories was edited (copies in `S/mods`, `S/repo_exnref`).
* `target/web_s10` (402.5 MiB, the two Lua rebuilds) removed with `cargo clean`; `target/` holds only `web_s9`.
* Kept for re-runs: the downloads with checksum files and signatures (`S/dl`, 101 MB; the JSC zip will disappear from
  webkitgtk.org within ~1 month), the extracted shells (`S/tools`, 252 MB), module copies and rebuilds (`S/mods`),
  references (`S/runs/ref`). Passing per-tick hash files and outputs > 1 MB were deleted after comparison; their
  SHA-256 are in `S/runs/manifest/` (`sim_<module>.txt`, `out_files.sha256`). Micro-test outputs are gzip'ed
  (`runs/micro/*.bin.gz`; `gunzip -k` before `compare_micro.py`, or re-run `ONLY=micro ./run_engines.sh jobs`).
* Scratch total 565 MB; free disk 21 GB.
