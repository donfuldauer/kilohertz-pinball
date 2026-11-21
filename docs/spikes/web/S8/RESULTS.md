# S8 — web-spike-lua-audit: Lua 32-bit vs 64-bit determinism audit, memory-cap parity, address-free keys, per-tick aggregate budget (RESULTS)

Base: `main` at `995935f` via S2's scratch tree (S1 patches 0001–0004 + S2 0001–0002, KHZ 29–32), copied to `S/repo`
(`S` = `${WEB_SPIKES}/S8`;
`S1`, `S2`, `S4`, `S7` = the sibling spike directories, read only). Toolchain: rustc 1.97.0, wasi-sdk 32 (clang
22.1.0, wasi-libc, sha256 `55fc523e…ef93`, S2's copy), wasm-bindgen 0.2.130, Node v22.23.5, Playwright Chromium 157,
official Node v22.23.5 linux-arm64 (sha256 `2babb9eb…5e09`, checked against S7's `SHASUMS256.txt`) under
qemu-aarch64 8.2.2 for "ARM64 V8". Every cargo command used the COMMON env (`S/env.sh`: `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s8[/native|/wasm|/tests|/s4native|/s4wasm]`,
`CARGO_INCREMENTAL=0`, line-tables-only debug); wasm builds `--target wasm32-unknown-unknown` with
`RUSTFLAGS="-C target-feature=+simd128 -C link-arg=-zstack-size=4194304"` and `S/wasm_env.sh` (S2 route b).
The audit was done at Opus depth: Fable was not available (D42: allowance unknown means no Fable).

## 0. Bottom line

* **Exact native = web for every Lua script, not only the production tables, is reachable with five small,
  knob-gated KHZ patches (33–37), all prototyped and verified here.** With them, the whole evidence set is
  identical natively (x86_64, two runs), in Node and in Chromium — 13 audit probes (1 658 script-observable
  lines plus the accounted heap after every probe and after each of 804 callbacks; these also on V8's ARM64
  backend), S2's 79-case sandbox corpus (79/79, was 78/79), the budget-edge probe, the address-keyed `pairs`
  probe, the adversarial ticks — and all 11
  production scenarios stay bit-identical to S1's native recordings per step and per tick (native, Node,
  Chromium).
* **The audit found six script-reachable platform dependencies — four new (F1, F2, F5, F6), none caught by an
  existing test, plus S2's two residuals (F3, F4):**
  F1 `string.sub/byte/find/match/gmatch` positions ≥ 2^32 wrap on 32-bit `size_t` — wrong results **and a
  one-byte out-of-bounds read** (`string.sub(s, 2^32)` returns `"\0"..s` on wasm32, reading the TString header);
  F2 the GC schedule reaches the **budget** through host-userdata finalizers (32 units per C call) and the KHZ 19
  emergency-GC charge; F3 the cap point and heap headroom (S2's residual); F5 `luaS_new`'s string cache is
  **indexed by a C address**, so `tostring` of a table with a long `__name` costs different units and memory
  natively run to run and on the web; F6 the **sign of NaN** printed by `tostring`/`format` differs between x86
  and ARM (phones, ARM64 V8, aarch64 native); F4 address-keyed `pairs` (S2's residual) also changes the KHZ 25
  chain-walk charges. Fixes: KHZ 35 (F1), KHZ 33 (F2, F3), KHZ 34 (F4), KHZ 37 (F5), KHZ 36 (F6). Everything
  else (limits, error texts, number formatting and parsing — 17 digests over 1 000 000 samples each —, integer
  and float semantics, `#`, the table library, `sort`, `utf8`, string comparison, the Rust API) is identical, or
  unreachable from the sandbox (§3).
* **Memory cap (deliverable 2): closed.** KHZ 33 accounts every allocation in logical (LP64) sizes, drives GC
  debt, pacing, `GCestimate`, KHZ 19 and the cap from them, routes lauxlib buffers through the core, and keeps
  a 2× real-byte backstop in mlua's allocator. `mem_gradual_keep` now faults on the same callback everywhere
  (`ok*1969,M`; was 1970 native vs 1995 wasm). Native change: only scripts that fill their 2 MiB heap move
  (this case by one tick); production peaks are 46–104 KB logical (2–5 % of the cap).
* **Address-keyed `pairs` (deliverable 3): closed.** KHZ 34 hashes tables/closures/userdata/threads by a
  per-state creation id: the order is identical in two native runs, Node and Chromium (was different in all
  four), costs 0 bytes on x86_64 for closures/userdata, +8 B per Table and per thread (logical sizes unchanged),
  and no measurable CPU (production callbacks within run-to-run noise; 2 M address-keyed lookups no slower).
  Recommendation: implement it (the documented rule stays as defence in depth).
* **Per-tick aggregate (deliverable 4): recommend 200 000 units per tick** (2 × the per-callback budget), Lua
  and Wasm alike, charged by exact units over a tick's switch edges and `on_tick`, fault (`BudgetExceeded`,
  tilt) when exhausted. Production ticks need ≤ 637 Lua units / ≤ 1 602 Wasm fuel (headroom 314× / 125×); the
  worst adversarial tick today (512 edges + `on_tick`, each just under 100 000) costs 947 ms native, 1 121 ms
  Node, 1 006 ms Chromium (Lua `alloc` mix; ≈ 3–5 s on a 3–5× slower phone) and, for Wasm rules, ≈ 12–19 s in
  V8 by S4's per-callback worst × 513; with the cap the same tick stops after 2 callbacks + 1 partial at
  3.0 / 4.4 / 3.8 ms (Lua) with the identical outcome on all platforms, and every production replay is
  unchanged (Lua and Wasm, native and Node; Lua also Chromium).
* **Blast radius: very high** for all of it (vendored Lua, the untrusted-table boundary, native replay
  semantics for scripts that hit a cap). Native production replays do not move. Native changes are limited to:
  scripts that fill their heap (cap point), scripts that print NaN (`-nan` → `nan`), scripts whose
  `pairs` order over address keys was already nondeterministic, and scripts that exceed 200 000 units in one
  tick.

## 1. Owned assumptions and claims — verdicts

S8 owns no row of the proposal's §11 table; it owns S2's residual list (§11 of S2) and S4's per-tick queue
line, plus the confidence claim the issue wants to make ("web replays match desktop exactly").

| claim | verdict | evidence | reproducing command |
|---|---|---|---|
| A10 (route) and A25 (Wasm EH) | unchanged (S2's verdicts) | S8 builds and runs the same route (b) module, legacy EH, on Node, Chromium and ARM64 V8 | `S/harness/build_variant.sh v2` |
| "web replays of the 3 production tables match desktop exactly" | **confirmed** (11 scenarios, per step and per tick, native/Node/Chromium; also with the S8 patches and with the per-tick cap on) | §4.3, §6.4 | `S/run_memcap_parity.sh`, `S/run_tick_budget.sh` |
| "… also for any user bundle" | **changed**: true with KHZ 33–37 (+ KHZ 29–32); false without them (F1–F6) | §3 | `S/run_audit.sh v0` vs `v2` |
| S2 residual 1 (cap point), 3 (address-keyed pairs), 6 (KHZ 19) | **closed** by KHZ 33 / 34 | §4, §5 | `S/run_memcap_parity.sh`, `S/run_pairs_order.sh` |
| S4 queue line (per-tick aggregate) | **closed with a recommendation** (200 000 units, fault) and a prototype in both backends | §6 | `S/run_tick_budget.sh` |

## 2. Layout, variants, reproduction

* `S/repo` — copy of `S2/repo` with the S8 changes; `S/s4repo` — copy of `S4/repo` with the S8 Wasm changes.
* Variants (lua-src build knobs, `third_party/lua-src/src/lib.rs`, `cargo:rerun-if-env-changed`):
  **v0** = S2's recommended build (KHZ 29–32, `KHZ_LUA_MATH=1`, pin_script `web-parity`) — reproduces S2's
  sandbox file byte for byte (`mem=` fields included);
  **v1** = v0 + `KHZ_S8_FIXES` (KHZ 35, 36, 37) + `KHZ_LOGICAL_MEM` (KHZ 33);
  **v2** = v1 + `KHZ_KEY_ORDER` (KHZ 34) = **the recommended build**.
  `S/harness/build_variant.sh v0|v1|v2` builds native + wasm and copies the artifacts to `S/runs/bin/<v>/`
  (`s2`, `pkg_node`, `pkg_web`); the S4 copy's artifacts are in `S/runs/bin/s4/` (`s4`, `pkg`).
* Patches (diffs against S2's/S4's scratch trees): `S/patches/0001-lua-src-khz-33-37-s8.patch` (C + build
  script), `0002-pin_script-s8-memcap-tickcap-instrumentation.patch` (logical cap + backstop wiring, per-tick cap
  prototype, `s8_*` spike accessors), `0003-s2_harness-s8-tooling.patch`, `0004-s4_rules-s8-tick-cap-tooling.patch`.
  Product-relevant parts: all of 0001 except the `khzs` counters/`lua_khzstats`/`lua_khzfeatures`
  (instrumentation), and in 0002 `apply_memory_cap`, `S8_BACKSTOP_FACTOR` and the tick-cap logic in `run()`.
* Harness: `S/repo/crates/s2_harness/src/s8.rs` — one dispatcher (`s2 s8 <cmd>` natively, `s8_call` in wasm)
  with `probe` / `memtrace` (runs a Lua probe through the engine's `LuaScriptBackend`, prints the script's
  `S8_OUT`, per-callback outcome/units, the command-queue digest, the accounted heap), `tickstats` (S1 recording
  through the sim with the real rules, checked per step and per tick against S1, per-tick units), `advtick`
  (adversarial tick), `pairs`; Node `S/node/run.cjs`, Chromium `S/node/chromium.cjs` + `S/web/` (runs a
  `jobs.json`), ARM64 V8 = the same Node script on the arm64 Node under qemu.
* Gate scripts (all exit 0 on the final artifacts): `S/run_audit.sh [v0|v1|v2]`, `S/run_memcap_parity.sh`,
  `S/run_pairs_order.sh`, `S/run_tick_budget.sh`, plus `S/probes/c/run_fmt_cmp.sh` and `S/probes/c/build_sizes.sh`.
  Logs of the final runs: `S/logs/run_*.log`.

## 3. Deliverable 1 — the audit

### 3.1 Method and coverage

The vendored Lua was audited from source for every expression whose value depends on `sizeof`, pointer width,
`size_t`/`long`/`ptrdiff_t`/`l_mem` width, an address, the C library, or the GC schedule, and that a script can
reach. Every hit of `grep -n '(size_t)\|cast_sizet\|(long)\|unsigned long\|(ptrdiff_t)\|L_P2I\|point2uint\|
lua_topointer\|%p\|MAX_SIZET\|MAX_SIZE\|MAXSIZE\|MAX_LMEM\|sizeof(void\|sizeof(size_t)\|sizeof(long)\|
LUAI_MAXALIGN\|l_mem'` over the compiled sources, every `luaM_*`/`luaC_newobj*` call site (60 call lines in the compiled core and lauxlib), every libc
call reachable from the base/string/table/math/utf8 libraries, and the size-dependent KHZ cost sites (KHZ 4, 14,
18, 19, 20, 25) were read line by line;
the probes then test each area black-box on four engines. mlua 0.12.2 (`memory.rs`, `util/error.rs`, userdata
push sites, every `target_pointer_width`/`target_arch` cfg) and pin_script's `lua/` (all files) were read for
width/address dependence.

| read line by line | sampled (grep-driven, hits read) | not compiled into the web build / not reachable |
|---|---|---|
| lmem.c, lmem.h, lstate.c (state, threads, CallInfo, KHZ APIs), lstate.h, lobject.h (all structs), lstring.c (createstrobj, luaS_new, luaS_resize, luaS_newudata, luaS_init), ltable.c (hashing, mainposition, new/free/resize), lgc.c (newobj, freeobj, GCTM, fullgc, singlestep work units, incstep, setpause), lfunc.c, lauxlib.c (buffers/boxes, luaL_tolstring, pushglobalfuncname), lstrlib.c (posrelatI, getendpos, sub, byte, rep, find_aux, gmatch, str_format float/%p/%q), lutf8lib.c (u_posrelat, utfint, decode), lobject.c (tostringbuff, %p buffer), llex.c (readutf8esc, buffer limit), lapi.c (GC options, topointer), lbaselib.c (collectgarbage), lmathlib.c (seeding), lvm.c (concat limit, l_strcmp) | lvm.c, ldebug.c, ltm.c, ldo.c, lcode.c, lparser.c, ltablib.c, lctype.c, lkhznum.c (S2), luaconf.h | liolib.c, loslib.c, loadlib.c, ldblib.c (not compiled on wasm, not opened natively), lcorolib.c (`coroutine` removed), lundump.c/ldump.c (text chunks only; `string.dump` removed) |

### 3.2 Examined source locations (real tree, `third_party/lua-src/lua-5.4.9/`) and verdicts

| area | location | what depends on width/address/libc | reachable? | verdict (probe) |
|---|---|---|---|---|
| (a) growth | lmem.c `luaM_growaux_` (all), lmem.h `luaM_limitN` | element sizes (accounting); limits `min(limit, MAX_SIZET/sizeof(t))` | accounting yes; limits differ only for `Instruction`/`AbsLineInfo` vectors (> 2^29 elements) | accounting: KHZ 33; limits unreachable under 2 MiB |
| (a) objects | lgc.c:270 `luaC_newobj`, lfunc.c:28/36/51/66/241, lstring.c:186/307, ltable.c:651, lstate.c (`lua_newthread`, `luaE_extendCI`) | Table 56/32, UpVal 40/32, Proto 128/84, CallInfo 64/36, LX 208/120, TString hdr 24/16, Udata hdr 40/24 (32/16), closure hdrs 32/16, LocVar 16/12, Upvaldesc 16/8, Labeldesc 24/16, `TString*`/`Proto*` 8/4 (x86_64/wasm32, `runs/probes/sizes.table.txt`) | yes (GC debt, cap) | **F3** → KHZ 33 |
| (a) same-size types | TValue 16, StackValue 16, Node 24, Instruction 4, AbsLineInfo 8, Vardesc 24, UValue 16 | — | — | identical (compile-time checked in KHZ 33) |
| (a) limits | luaconf.h:749 `LUAI_MAXSTACK` 1 000 000; llimits.h:255 `LUAI_MAXCCALLS` 200; lstate.c `lua_setcstacklimit` (no-op, returns 200); ltable.c:46–68 `MAXASIZE` 2^31 / 268 435 455, `MAXHSIZE`; lstring.c:28 `MAXSTRTB`; llimits.h:44 `MAX_SIZE`; lvm.c:679, lstring.c:269/305, llex.c:61 | counts identical; byte limits are 4 GiB on wasm32 vs 2^63 | "stack overflow" needs 16 MB of stack > 2 MiB cap; the others need > 2 MiB | identical (`a_limits`: C-stack, pcall nesting, sort/gsub/`__index`/`__tostring`/`__concat` recursion, rep/concat/unpack/format/capture/pattern limits and their texts); byte limits unreachable |
| (a) error texts | "C stack overflow", "not enough memory", "too many results to unpack", "string slice too long", "resulting string too large", "string length exceeds the 64 KiB limit", "too many captures", "pattern too complex", "invalid conversion …" | counts or KHZ constants | yes | identical (`a_limits`, S2 corpus); *where* "not enough memory" happens: KHZ 33 |
| (b) GC pacing | lgc.c `incstep` (`(GCdebt/WORK2MEM)*stepmul`, WORK2MEM = sizeof(TValue) = 16 both), `setpause`, `singlestep` work units (counts: `1 + alimit + 2*sizenode`, `1 + stacksize`, GCSWEEPMAX), lapi.c `LUA_GCSTEP` (pin_script's 16 KB step after `on_tick`) | debt in real bytes (v0) | yes | **F2**: deterministic once debt is logical (32-bit `l_mem` is exact for caps ≤ ~340 MB) |
| (b) GC → budget | ldo.c:551 `lua_khzcharge(L, KHZ_CCALL_COST)` in `precallC`, also for finalizers run by lgc.c `GCTM` (mlua userdata `__gc`, lauxlib box `__gc`); lgc.c:1748 KHZ 19 `gettotalbytes>>6` per full GC (incl. emergency) | when finalizers/emergency GCs run | yes (scripts make host userdata by catching API errors; boxes by building strings) | **F2**: v0 units differ (`c_tostring` 35 378 vs 35 474, `j_api` 254 869 vs 253 237, `b_gc_engine` first fault); identical with KHZ 33 |
| (b) finalizers/weak | `__gc`, `__mode` refused by the prelude; KHZ 27 ignores late `__mode`; KHZ 8/9 no `__close` | — | no script finalizers, no weak or ephemeron tables | unreachable (S2 cases `gc_meta_refused`, `weak_mode_after`) |
| (c) addresses | lauxlib.c:946 `luaL_tolstring` (KHZ 10), lstrlib.c:1403 `%p` (KHZ 5), lobject.c:520 `%p` buffer (no reachable caller), lapi.c:487 `lua_topointer` (pin_script `saved.rs` HashSet membership only) | — | — | identical (`c_tostring`: 9 value kinds × tostring/%s/%p/concat) |
| (c)/(g) string cache | lstring.c:285 `luaS_new`: `point2uint(str) % STRCACHE_N`; callers with script-controlled long strings: lauxlib.c:946 (`__name`), lauxlib.c:87 (global function names in mlua's traceback of a callback error) | cache hit = no allocation and no KHZ 4 charge | yes | **F5** (`k_strcache`: native run 1 ≠ run 2 ≠ wasm) → KHZ 37 |
| (d) format | lstrlib.c `str_format` (`l_sprintf` = snprintf, `MAX_ITEM` 120, `MAX_ITEMF` 110+308, `MAX_FORMAT` 32, 2-digit width/precision), lobject.c:361 `lua_number2str` "%.14g", KHZ 30 `%a` | glibc vs wasi-libc (musl) printf | yes | identical: 15 conversions × 18 ints, 25 × 23 floats, %q, %s, 14 invalid specs (`d_format`); C level 15 formats × 1 000 000 doubles, identical digests (`probes/c/run_fmt_cmp.sh`) |
| (d) NaN | lobject.c:361, lstrlib.c:1387/1399 | **sign of an arithmetic NaN is CPU-dependent** | yes | **F6** → KHZ 36 |
| (e) conversions | lvm.c `luaV_flttointns`/forprep, `luaV_idiv/mod/modf`, shifts, lobject.c `l_str2int`/`l_str2d` (strtod; `nan`/`inf` rejected before strtod), lbaselib `b_str2int`, lmathlib (`tointeger`, `ult`, `fmod`, `abs`, `floor`, `ceil`), KHZ 32 libm | 64-bit `lua_Integer`/double on both; strtod | yes | identical (`e_numbers`, 57 lines; strtod round trip + 1 000 000 random decimals identical) |
| (f) length | ltable.c `luaH_getn`/`hash_search`, ltablib insert/remove/unpack/concat/move/pack | `unsigned int`/`lua_Unsigned` | yes | identical (`f_length`: 10 construction histories, 450 grow/shrink steps, table-library boundaries) |
| (g) hashing | ltable.c:108 `hashint`, :131 `l_hashfloat` (int is 32-bit on both), lstring.c:81 `luaS_hash` (seed fixed, KHZ 1), ltable.c:175/179/183 `hashpointer` | addresses for light userdata, light C functions, GC objects | yes (tables/closures/C functions/host userdata as keys; light userdata: none reachable) | value keys identical; **F4** → KHZ 34 |
| (h) sort | ltablib.c `auxsort`, `choosePivot`, `l_randomizePivot` (KHZ 2 → 0), `IdxT` unsigned int | — | yes | identical (`h_sort`: 8 sizes × 5 comparators incl. invalid; error texts and permutations) |
| (i) positions | lstrlib.c:71 `posrelatI` `(size_t)pos` | **wraps on 32-bit size_t** | yes | **F1** → KHZ 35 |
| (i) utf8 | lutf8lib.c:37 `utfint` = `unsigned long` (32/64), :49 `u_posrelat` `(size_t)pos` | width | yes | identical: every decoded value ≤ 0x7FFFFFFF fits; the negative-position wrap ends in the same argument error (`i_misc` utf8pos/utf8dec lines) |
| (i) sizes | lstrlib.c:164–169 `str_rep` (`MAXSIZE / n` promoted to 64-bit on wasm32), lauxlib.c:555, lvm.c:679 | — | — | identical (`rep_sizes`, `concat_sizes`) |
| (i) env | `string.pack/unpack/packsize` ('l','L','T','j','s' formats), `string.dump`, `load*`, `coroutine`, `io`, `os`, `debug`, `require`, `collectgarbage` (`web-parity`) | — | removed | unreachable: `i_misc` scans `_G` to depth 3 (121 entries): no userdata/thread values, no light userdata |
| (i) RNG | lmathlib.c:632 seed from `(size_t)L` and time | address/time | no (`math.random` replaced by the host PCG before any script runs) | unreachable (S2) |
| (i) strcoll | lvm.c:389 `l_strcmp` → `strcoll` | locale | yes | identical ("C" locale on both; digest over 86×52 byte pairs + embedded zeros) |
| (j) Rust | pin_script `api.rs` (i64/f64/u16 conversions, Rust float `Display`), `saved.rs` (HashSet of `to_pointer`: membership only), `bridge.rs`, `sandbox.rs` (raw state pointer); mlua `value.rs:288` (`as_i64` cfg), `memory.rs` (limit `isize`), userdata payloads (Rust type sizes) | — | yes | identical (`j_api`: 20 argument kinds × 13 API functions, texts and outcomes); payload sizes handled by KHZ 33's constant |
| (e) native FP contraction | GCC (GNU mode) contracts by default; aarch64 has FMA: `luaB_collectgarbage` (`k + b/1024`), `luaK_numberK` (`r + r*q`, a constant-table key) | aarch64 native only | no (collectgarbage replaced; the key is never a value) | unobservable; request `-ffp-contract=off` on every native arm (`runs/probes/fma_aarch64/`) |

### 3.3 Probe results (`S/run_audit.sh`, logs `logs/run_audit_v0.log`, `logs/run_audit_v2.log`)

PASS = the script-observable lines (everything the script printed, the per-callback outcome and budget units,
the command-queue digest) are byte-identical; "heap" = the accounted heap after every probe (and after every
callback for `b_memstep_engine`), harness-only. ARM64 V8 = the same wasm module on the official linux-arm64
Node under qemu-aarch64.

| probe | lines | v0: native run1 = run2 / = Node / = Chromium / = ARM64 V8 / heap | **v2**: native run1 = run2 / = Node / = Chromium / = ARM64 V8 / heap | verdict |
|---|---|---|---|---|
| a_limits | 15 | PASS / PASS / PASS / PASS / FAIL | PASS / PASS / PASS / PASS / PASS | identical (counts) |
| a_unpack_engine | 3 | PASS / PASS / PASS / PASS / FAIL | PASS ×4 / PASS | outcome identical (M, same units); heap differs before KHZ 33 |
| b_gc_engine | 3 | PASS / **FAIL** / FAIL / FAIL / FAIL | PASS ×4 / PASS | F2/F3 → KHZ 33 (incl. 71 emergency GCs at identical points) |
| b_memstep_engine | 804 | PASS / **FAIL** / FAIL / FAIL / FAIL | PASS ×4 / PASS (804 per-callback heap values identical) | F2 → KHZ 33 |
| c_tostring | 46 | PASS / **FAIL** / FAIL / FAIL / FAIL | PASS ×4 / PASS | texts identical; units differ before KHZ 33 (F2) |
| d_format | 109 | PASS / PASS / PASS / **FAIL** / FAIL | PASS ×4 / PASS | F6 → KHZ 36 |
| e_numbers | 57 | PASS / PASS / PASS / **FAIL** / FAIL | PASS ×4 / PASS | identical except NaN text on ARM (F6) |
| f_length | 11 | PASS / PASS / PASS / PASS / FAIL | PASS ×4 / PASS | identical |
| g_pairs | 16 | **FAIL** / FAIL / FAIL / FAIL / FAIL | PASS ×4 / PASS | value keys identical; ADDR lines F4 → KHZ 34 |
| h_sort | 46 | PASS / PASS / PASS / PASS / FAIL | PASS ×4 / PASS | identical |
| i_misc | 161 | PASS / **FAIL** / FAIL / FAIL / FAIL | PASS ×4 / PASS | F1 → KHZ 35 |
| j_api | 1192 | PASS / **FAIL** / FAIL / FAIL / FAIL | PASS ×4 / PASS | texts identical; units differ before KHZ 33 (F2) |
| k_strcache | 4 | **FAIL** / FAIL / FAIL / FAIL / FAIL | PASS ×4 / PASS | F5 → KHZ 37 |
| nan_sign | 6 | PASS / PASS / PASS / **FAIL** / FAIL | PASS ×4 / PASS | F6 → KHZ 36 |
| sizes_c (C) | 88 | 50 of 88 values differ x86_64 vs wasm32 | (layout itself) | by design; neutralised by KHZ 33 |

`run_audit.sh v2`: **0 probes with script-observable differences** on any of the five columns; v1 (no KHZ 34):
only `g_pairs`. Other audit evidence: `probes/c/run_fmt_cmp.sh 1000000` (glibc 2.39 vs wasi-libc, 15 printf formats ×
1 000 000 doubles + strtod round trip + 1 000 000 random decimal strings: 17/17 digests identical; the only
difference, `strtod("nan")` payloads, is unreachable because Lua rejects `n`/`N` before calling strtod).

### 3.4 Findings and fixes (each prototyped and re-verified; diffs in `patches/0001`)

* **F1 (i, wasm32 only) — string positions ≥ 2^32 wrap; out-of-bounds read.** `posrelatI` (lstrlib.c:71)
  returns `(size_t)pos`; on wasm32, `pos = 2^32 + k` becomes `k`, and `2^32` becomes 0, after which `str_sub` /
  `str_byte` read `s[-1]`. Evidence (`runs/audit/v0/{native,node}/i_misc.txt`):
  `string.sub("hello", 4294967296)` → native `""`, wasm `"\0hello"`; `string.byte("hello", 4294967296, -1)` →
  wasm `0,104,…` (the byte before the string is the top byte of the TString header's `hnext` pointer: an
  address byte, against KHZ 10's no-address rule); `string.find("hello", "l", 4294967297)` → native nil, wasm 3;
  `gmatch`/`match` likewise. **KHZ 35**: saturate to `MAX_SIZET` when `size_t` is narrower than `lua_Integer`
  (all callers only compare the position with a string length, so the 64-bit result is kept). Native code
  unchanged (the condition is a compile-time constant false on LP64).
* **F2 (b) — the GC schedule reaches the budget.** Host userdata (mlua's `WrappedFailure` for every caught API
  error, `CallbackUpvalue`, …) and lauxlib boxes have `__gc`; `GCTM` calls them through `precallC`, which charges
  32 units each to the running callback, and KHZ 19 charges heap/64 per emergency collection. With real-byte
  debt, when they run is pointer-size dependent: identical texts but different units (`c_tostring`, `j_api`),
  and a different first fault in `b_gc_engine`. **KHZ 33** makes the schedule identical (verified to the byte:
  `b_memstep_engine` prints the accounted heap and GC debt after 804 callbacks; identical on all engines).
* **F3 (a/b) — the memory cap point** (S2-6): closed by KHZ 33 (§4).
* **F4 (g) — address-keyed `pairs`** (S2 residual 3), plus the budget charges that depend on the same node
  layout (KHZ 18 skipped slots in `next`, KHZ 25 chain walks: `g_pairs` units differ by 2): closed by KHZ 34 (§5).
* **F5 (c/g) — `luaS_new`'s string cache is indexed by a C address.** `tostring(t)` for a table whose
  metatable `__name` is longer than 40 bytes pushes the name with `lua_pushstring`; whether the cached long
  string is found (no allocation, no KHZ 4 charge) depends on `address % 53` of the name's bytes. Evidence
  (`k_strcache`): v0 native run 1 vs run 2 differ (15 328 vs 15 323 units), Node differs again; even with KHZ 33
  and 34 the units and accounted heap still differed (measured before KHZ 37 was added). Same path: global
  function names longer than 40 bytes in mlua's traceback of a callback error (lauxlib.c:87). **KHZ 37**: the
  bucket is `luaS_hash(content) % STRCACHE_N` (seed fixed by KHZ 1). This is also a **native run-to-run** D9
  defect today.
* **F6 (d) — NaN sign.** A NaN produced by arithmetic is negative on x86 and positive on ARM (Wasm leaves the
  sign nondeterministic; S7: `0/0` is `ffc00000` on x64 V8, `7fc00000` on ARM64 V8). Lua prints it through
  printf: `tostring(0/0)` is `-nan` natively on x86_64, in Node and Chromium on x86, but `nan` on ARM64 V8
  (`runs/probes/nan_sign.txt`, measured), and `string.format('%+f', 0/0)` is `-nan` vs `+nan`; Rust-libm results
  such as `math.log(-1)` and `math.acos(2)` inherit the CPU's sign. A script that turns the text into a command
  diverges on phones and on aarch64. **KHZ 36**: NaNs are formatted with the sign cleared (`fabs`) in
  `tostringbuff` and `string.format` %a %A %e %E %f %g %G (`%q` already prints `(0/0)`). Native x86 text
  changes from `-nan` to `nan` (S2 cases `int_float_semantics`, `float_nan_format`).
* **F7 (e, aarch64 native, unobservable)** — GCC contracts two FP expressions on aarch64 (§3.2 last row); request
  `-ffp-contract=off` on the native lua-src arms (queue line).

## 4. Deliverable 2 — platform-independent memory accounting (KHZ 33)

### 4.1 Options assessed

| option | assessment | verdict |
|---|---|---|
| (i) logical size in the `lua_Alloc` wrapper from `osize`/`nsize` (+ type tag on new objects) | Sizes do not identify types: on wasm32 a 32-byte block is a Table, an UpVal, a 15-byte string or a 1-upvalue C closure; frees and reallocs carry no tag; userdata payloads are Rust type sizes (x86_64 8/32/24/16/72 vs wasm32 4/32/12/8/36 bytes: no common ratio, `runs/audit/v2/*/j_api.txt` `REAL` line); lauxlib boxes bypass the core | rejected (cannot be exact) |
| **(ii) logical size passed at the call sites** | of the 60 allocation/free call lines, the ones whose size differs pass `(real, logical)` through new `luaM_*l`/`luaC_newobjl` entry points; the old names become macros with logical = real for same-size types; compile-time checks pin (a) the same-size types (TValue, StackValue, Node, UValue, Instruction, AbsLineInfo, Vardesc) and (b) on LP64 every logical constant against the real `sizeof`/`offsetof` | **chosen**: exact by construction, ~250 lines of C, no change to mlua |
| (iii) size-identical layouts (padding/fixed-width fields) | Padding structs to LP64 sizes is possible for the GC objects (the real size grows ~40 % on wasm32), but not for `TString*`/`Proto*` vectors and `LClosure` upvalue arrays (element = pointer), nor for mlua's Rust userdata payloads, nor for lauxlib boxes; would need mlua changes | rejected (incomplete without patching mlua and pointer vectors) |
| wasm64/memory64 for a separate Lua instance | LP64 C layout, but mlua's Rust userdata would still differ unless the whole host also ran in wasm64; browser support is S5's; costs route (f)'s per-call copies | not needed |

### 4.2 Design (patch 0001, `lkhzmem.h`, `lmem.c`, `lstate.c`, …)

* Logical constants = Lua 5.4.9's LP64 layout: Table 56, UpVal 40, Proto 128, CallInfo 64, LX 208, LG 1 648,
  LocVar 16, Upvaldesc 16, Labeldesc 24, pointer elements 8, TString header 24, Udata0/Udata headers 32/40,
  CClosure/LClosure headers 32/32; full-userdata payload = **constant 64 bytes** (S8-3).
* `g->GCdebt`, `totalbytes`, `GCestimate`, `LUA_GCCOUNT`, KHZ 19's charge: logical bytes.
* The cap (`lua_khzsetmemcap`, pin_script's `LuaConfig::memory_limit_bytes`) is checked in `lmem.c` before the
  allocator call; an over-cap request behaves like an allocator failure (emergency collection, re-check, then
  `LUA_ERRMEM`), so every existing memory path (`memabort`, `pcall` re-raise, classification `MemoryLimit`) is
  unchanged.
* lauxlib `resizebox` allocates through the core (`lua_khzrealloc`), so buffer bytes are part of the logical
  heap and the cap (they used to be visible only to mlua's real counter).
* Real-byte backstop: mlua's limit = 2 × the cap (`S8_BACKSTOP_FACTOR`, 4 MiB for the engine). Measured: the
  largest real peak of any probe is 2 083 354 B (x86_64, `b_gc_engine`, logical peak 2 096 954 B) — the backstop
  was never approached; on wasm32 real < logical everywhere (`REAL` vs `HEAP` lines); every memory fault in the
  evidence set is a logical-cap refusal (`cap_refusals`), identical on all engines.
* 32-bit exactness bound: `l_mem` is 32-bit on wasm32; the GC's `(GCdebt / 16) * 101` stays exact for caps up
  to ≈ 340 MB (engine 2 MiB, tools 16 MiB).

### 4.3 Results (`S/run_memcap_parity.sh`, `logs/run_memcap_parity.log`, exit 0)

S2's sandbox corpus (79 cases, `LuaConfig::engine()`), lines compared with the harness-only `mem=` field stripped:

| variant | native = Node | native = Chromium | `mem_gradual_keep` native / Node / Chromium |
|---|---|---|---|
| v0 (S2 recommended) | 78/79 | 78/79 | `ok*1970,M` / `ok*1995,M` / `ok*1995,M` |
| v1 (KHZ 33 + 35–37) | 79/79 | 79/79 | `ok*1969,M` ×3 |
| **v2** (+ KHZ 34) | **79/79** | **79/79** | **`ok*1969,M` ×3** |

Native lines that changed v0 → v2 (expected, nothing else): `mem_gradual_keep` (cap point one tick earlier:
buffer bytes and the userdata constant are now counted), `int_float_semantics` and `float_nan_format` (`-nan` →
`nan`, KHZ 36). Budget-edge probe: identical in all three variants on all platforms (`concat_700 2091,
format_700 1092, format_400 1436, gsub_600 47, rep_900 1483, concat_op 4997, arith 33664, tables 16832`).

Production replays under v2 (per step: tick, RNG draws, input digest, command queue; per tick: `state_hash` and
`ext_hash`, against S1's native recordings): **all PASS**, 0 mismatches —

| scenario | native | Node | Chromium | final state_hash | peak accounted heap v2 (logical) | peak heap v0 (real, x86_64) |
|---|---|---|---|---|---|---|
| replay_table_fixture10k | PASS | PASS | – | 25097c21c2c3adc1 | 46 953 | 45 977 |
| apex_circuit bot60 / pat60 / mb8 / bot600 | PASS ×4 | PASS ×4 | bot60, mb8 PASS | 2b7b1eec564b8cdc / b9c2a55ba04954e9 / 2c52fb33d519ffb7 / a4b944ad641338d1 | 103 958 | 103 014 |
| sky_deck bot60 / pat60 / mb8 | PASS ×3 | PASS ×3 | bot60, mb8 PASS | 1f0509f5802dc2e5 / ce9eb11d97fbef81 / 777557dae8e5531b | 78 494 | 77 558 |
| feature_lab bot60 / pat60 / mb8 | PASS ×3 | PASS ×3 | bot60, mb8 PASS | 2040cea9ef77a2e0 / 865f852fce618b7e / 0c64556eecf5c210 | 84 625 | 83 689 |

No production table comes close to the cap (peak 5.0 % of 2 MiB); their native behaviour does not change.

### 4.4 Native behaviour change and blast radius

Native x86_64 changes only for scripts whose outcome depends on *where* memory runs out or on GC timing:
heap-filling scripts fault at a slightly different allocation (S2 corpus: one case, one tick earlier), scripts
near the cap see the emergency-GC charge computed from logical bytes, and finalizer charges move with the
(logical) GC schedule. Classification is unchanged (`MemoryLimit`), the cap is still 2 MiB, real memory stays
bounded (backstop). **Blast radius: very high** (vendored Lua, untrusted-table boundary, D34 (a)); the change
is gated by the native-vs-wasm corpus and the per-callback heap trace (§7).

## 5. Deliverable 3 — address-free `pairs` (KHZ 34)

Design: a per-state counter (`global_State.khzseq`, 64-bit; the stored id is its low 32 bits, so it wraps
deterministically) gives every table, Lua/C closure, full userdata and thread an id at creation; `mainposition`
hashes such keys by `id % ((sizenode-1)|1)` instead of the address. Light C functions (code addresses, ~110 in
the sandbox) and light userdata (none reachable) share one main position, so their chain keeps insertion order
(bounded; KHZ 25 charges long walks). Equality, `next` over dead keys and the GC are unchanged (the id is read
only from live keys).

(c) correctness (`S/run_pairs_order.sh`, `logs/run_pairs_order.log`, exit 0; probe `probes/pairs/order.lua`:
48 table keys, 24 closures, 12 host error userdata, 12 C functions + mixed keys, deletion during traversal,
rehash to 120 keys, 3 000 garbage tables per tick so addresses are reused, 4 ticks; v0's native digests change
on every run — three further runs gave a85f080e…/6f3c4914…, fdfa59c7…/0ea74cf4… —, v2's never did;
`logs/run_pairs_order.log`, `logs/run_pairs_order_final.log`):

| variant | native run 1 | native run 2 | Node | Chromium |
|---|---|---|---|---|
| v0 | 93348d38e2b93d0d | de66df4a252e8747 | edd9bba66c7d4447 | edd9bba66c7d4447 |
| **v2** | **7efbe9fc17a38b51** | **7efbe9fc17a38b51** | **7efbe9fc17a38b51** | **7efbe9fc17a38b51** |

(a) header bytes (real; logical sizes unchanged, so KHZ 33's numbers do not move):

| struct | x86_64 before → KHZ 34 | wasm32 before → KHZ 34 |
|---|---|---|
| Table | 56 → 64 | 32 → 36 |
| LClosure (header) | 32 → 32 | 16 → 20 |
| CClosure (header) | 32 → 32 | 16 → 24 |
| Udata0 / Udata (header) | 32 / 40 → 32 / 40 | 16 / 24 → 24 / 24 |
| lua_State | 200 → 208 | 116 → 120 |

(b) CPU: production per-callback means (S2's `play-timed`), median of 3 runs, v1 vs v2, µs, two independent
executions of `run_pairs_order.sh` on the shared VM — first: native on_tick apex 2.34/2.37, sky 2.33/2.39,
feature 2.63/2.76, apex mb8 3.00/3.14; Node 3.10/3.35, 3.69/3.45, 3.86/3.57, 3.55/4.27; second
(`logs/run_pairs_order.log`): native 3.67/2.61, 2.60/2.58, 3.08/3.17, 3.59/2.94; Node 3.77/4.02, 3.37/3.79,
4.41/4.37, 3.84/5.31. The run-to-run spread (up to ±40 %, both directions) dwarfs any difference: no
measurable cost. Micro-benchmark, 2 000 000 lookups/stores keyed by tables and closures
(`harness/bench_addrkeys.sh`, median of 5, whole process, v1 → v2): first native 39 → 37 ms, Node 129 → 101 ms;
second (`logs/bench_addrkeys.log`) native 51 → 41 ms, Node 111 → 108 ms (sequential ids spread keys at least as
well as addresses).

(d) production replays: unchanged (v2 = KHZ 34 in every row of §4.3 and §6).

**Recommendation: implement KHZ 34.** Keep the authoring rule anyway (defence in depth, and for other
backends): *"Do not depend on the iteration order of `pairs`/`next`. With KHZ 34 the order is deterministic
(the same on every platform and run, for every key type) but it is an implementation detail: it depends on the
insertion and deletion history and changes when the table is resized; use an array or sort the keys when order
matters."*

## 6. Deliverable 4 — per-tick aggregate script budget

### 6.1 Production per-tick totals (cap off; identical native and Node; `logs/run_tick_budget.log`)

Lua (exact units = VM instructions including the final partial hook interval + KHZ charges; "charged" = the
repo's hook-charged units, which count instructions only per full 1 000-instruction interval):

| scenario | callbacks/tick max (tick) p99 p50 | hook-charged units/tick max (tick) p99 p50 | exact units/tick max (tick) p99 p50 |
|---|---|---|---|
| fixture 10k | 3 (4911) 2 1 | 96 (672) 32 0 | 142 (672) 83 13 |
| apex bot60 / pat60 / mb8 | 4 (48943) 1 1 / 3 2 1 / 4 2 1 | 320 (78) 0 0 / 320 / 320 | 597 (78) 36 31 / 597 108 31 / 597 108 34 |
| apex bot600 (10 min) | 4 (48943) 1 1 | 352 (377250) 0 0 | **637 (400882)** 44 36 |
| sky bot60 / pat60 / mb8 | 2 / 3 / 3 | 224 / 256 / 256 | 314 / 421 / 421 (p99 101–140, p50 58) |
| feature bot60 / pat60 / mb8 | 3 / 3 / 3 | 320 ×3 | 599 / 600 / 497 (p99 111–277, p50 105) |

Wasm rules (S8 copy of S4's harness, design `wasmi-instr-depth` = wasmtime-exact fuel; fuel digest of apex bot60
`f17cda6a414ff89c` = S4's): fuel/tick max (tick) p99 p50 — apex bot60 1 476 (1829) 167 160, pat60 1 526 (2372)
782 160, mb8 1 526 587 164, bot600 1 485 (337864) 180 167; sky bot60 549 141 103, pat60/mb8 899 287 103; feature
bot60 1 017 129 116, pat60 1 548 358 116, mb8 **1 602** (2305) 841 116; callbacks/tick ≤ 4.

### 6.2 Worst-case adversarial tick (512 edges + `on_tick`, each callback calibrated to 98 % of the largest loop that fits 100 000 units)

Lua (`advtick`, median of 3 fresh backends, ms; this VM, load from concurrent builds possible):

| mix | units (513 callbacks) | native | Node | Chromium | one callback: native / Node / Chromium |
|---|---|---|---|---|---|
| loop (VM arithmetic) | 50 775 714 | 238 | 327 | 311 | 0.45 / 0.82 / 0.86 |
| **alloc** (table creation; GC work is not charged) | 50 772 123 | **947** | **1 121** | **1 006** | 1.83 / 1.96 / 2.01 |
| strings (concat + interning) | 50 450 985 | 735 | 1 110 | 891 | 1.20 / 2.03 / 1.60 |
| ccalls (`math.abs`) | 50 328 891 | 49 | 71 | 60 | 0.09 / 0.49 / 0.14 |
| errors (`pcall(error)`) | 49 886 685 | 22 | 50 | 36 | 0.03 / 0.24 / 0.11 |
| pattern (`gsub`) | 50 380 191 | 290 | 458 | 537 | 0.51 / 0.92 / 0.89 |

With the assumed 3–5× slower mobile core: the worst uncapped Lua tick ≈ 3.0–5.0 s (Chromium 1 006 ms × 3–5), a
single worst Lua callback ≈ 6–10 ms. Wasm rules (`s8adv`, wasmi-instr-depth, ms): `nop` loop 513 callbacks
native 52 / Node 701, `host_tick` 181 / 1 087; S4's worst per-callback bodies (`loop_br_only`, Node 23.9 ms,
Chromium up to 37 ms) give ≈ 12–19 s per uncapped tick in V8 (× 513, callbacks are independent; extrapolated)
and ≈ 37–95 s on a slow phone. Against the 1 ms tick and the 8 ms stall target the uncapped worst case is
2–4 orders of magnitude over on every platform, natively included.

### 6.3 Recommended rule (decision needed; very-high blast radius)

* **Cap: 200 000 units per tick** = 2 × the per-callback budget, the same number for Lua (exact units:
  instructions + KHZ charges) and Wasm (wasmtime fuel), native and web. Headroom over production: 314× (Lua
  637), 125× (Wasm 1 602).
* **Charging:** every `on_switch_edge` and the `on_tick` of one sim tick (same `HostView::tick`) share it;
  `on_init`, reload hooks and the host's GC step after `on_tick` do not. Each callback runs with
  `min(100 000, units left)`; what it used (exact) is subtracted.
* **When it is hit:** the callback that runs out, or the first callback that finds 0 left, is a
  `BudgetExceeded` **fault** exactly like today's per-callback budget kill (its commands are rolled back, held
  coils are released, scripts stop until a successful reload). Edges are neither skipped silently nor
  deferred: they keep being delivered to the faulted backend, which ignores them, as after any fault.
  Rationale: deterministic (units are, on every engine — §6.4), one fault model for authors, no rules that
  observe a world with missing switches, no edge storage that could overflow `MAX_PENDING_EDGES`.
* **`MAX_PENDING_EDGES` (512) unchanged:** the cap bounds a full queue's cost to ≈ 2 callbacks' worth (Lua
  worst 3.8 ms native / 3.8 ms Chromium; Wasm 0.8 ms native / 4.7 ms Node, S4's worst body ≈ 50–75 ms in V8);
  together with S4's 8 ms frame guard in the web shell no tick can hang the tab.
* **Where:** in the backends (pin_script's Lua and Wasm backends), keyed by `HostView::tick`, so no pin_core
  contract change is needed (a distinct `ScriptError::TickBudgetExceeded` would be an additive pin_core CR
  if the owner wants the two kills distinguishable; not proposed).

### 6.4 Prototype results (`S/run_tick_budget.sh`, exit 0)

* Production replays with the cap on (200 000): **all 11 Lua scenarios PASS** per step and per tick natively
  and in Node, and the 10 Wasm scenarios PASS natively (and bot60/mb8 in Node) — identical to cap off; no
  callback was refused.
* Adversarial tick with the cap on — Lua (native = Node = Chromium for every mix): 3 callbacks run (two full,
  the third killed when the tick allowance runs out), 200 024–202 018 units, outcome `BudgetExceeded`; wall
  time native 0.17–3.8 ms, Node 0.43–4.7 ms, Chromium 0.33–3.8 ms. Wasm (native = Node): 3 callbacks, fuel
  exactly 200 000, faulted; native 0.29–0.82 ms, Node 3.0–4.7 ms.
* Tests to add: `pin_script/tests/spec_tick_budget.rs` (Lua and Wasm: 512 edges at 99 % of the budget →
  `BudgetExceeded` after exactly the units above, identical on wasm in CI; 512 cheap edges + `on_tick` are not
  refused; the allowance resets at the next tick; `on_init` is not counted), `pin_sim` replay goldens unchanged
  with the cap (the 3 tables, fixture, soak), and the web CI comparison of the adversarial outcome line.

## 7. Deliverable 5 — what the issue and THREAT_MODEL/D9 should promise, and the CI gates

**Statement (with KHZ 29–37 and pin_script `web-parity`):**
"For a given recording, the web build (wasm32, any browser engine with Wasm EH) produces the same simulation as
the desktop build, bit for bit — every tick's `state_hash`, the script command stream and the script fault
behaviour — for the three production tables (`apex_circuit`, `sky_deck`, `feature_lab`) and for **any bundle**
whose Lua rules run inside the sandbox, including scripts that exhaust the per-callback budget, the per-tick
budget or the 2 MiB heap: budget units, the heap accounting (logical, LP64 sizes), the garbage-collection
schedule, `pairs` order for every key type, number formatting and parsing, and error texts are platform
independent. Not promised: (1) *real* memory use and wall time (the logical heap is identical, the bytes the
browser uses are not); (2) the *text* of a host trap (a Rust panic aborts the web instance: S2 G18); (3) engines
not measured — JavaScriptCore and SpiderMonkey (S5) and real ARM hardware (ARM64 V8 was measured under qemu,
S7 aarch64 likewise)."
For Wasm rules, S4's statement stands (instrumented fuel and depth counter, canonical NaN), plus the per-tick cap.

**D9 note:** add "the vendored Lua contains no address-dependent behaviour visible to scripts (KHZ 1, 2, 5, 10,
34, 37), no pointer-size-dependent behaviour (KHZ 31, 33, 35) and no CPU-dependent number text (KHZ 32, 36)";
THREAT_MODEL: KHZ 35 closes a one-byte out-of-bounds read of string headers on 32-bit targets; the logical cap
plus a 2× real backstop bounds Lua memory on every target.

**CI gates** (web workflow, Node; Chromium smoke; ARM64 V8 weekly or on lua-src/mlua changes):
1. S2's sandbox corpus native vs wasm, lines identical (79/79) — `run_memcap_parity.sh` part 1.
2. The audit probe set (13 probes) native vs Node identical, incl. the per-callback accounted heap
   (`b_memstep_engine`: catches any allocation site that loses its logical size) — `run_audit.sh`.
3. Two native runs of the pairs and string-cache probes identical (catches any new address dependence).
4. Budget-edge probe native vs wasm.
5. Production replays per step and per tick vs the native recordings (3 tables × bot60/pat60/mb8, fixture,
   soak) — S2's parity checker / `tickstats`, cap on.
6. Adversarial per-tick cap outcome native vs wasm (Lua and Wasm rules).
7. Native compile-time checks of KHZ 33 (logical constants = LP64 sizes; same-size types).
8. `probes/c/run_fmt_cmp.sh` (glibc vs wasi-libc printf/strtod) on any wasi-sdk or glibc bump.
9. aarch64: S7/RESULTS.md exists — real Lua on aarch64 (qemu) re-recorded all 10 scenarios byte-identically
   (S7 §3) and ARM64 V8 matches x86_64 (S7 C2); with KHZ 36 the NaN text no longer depends on the CPU; S7's
   qemu aarch64 job should run gates 1–3 natively too. Real ARM hardware remains unverified.

## 8. Deliverable 6 — S2 §11 residuals and the S4 queue line

| item | status | how / statement / cost |
|---|---|---|
| S2-1 memory-cap fault point | **closed** | KHZ 33: `mem_gradual_keep` identical (`ok*1969,M`) native/Node/Chromium; whole corpus 79/79 |
| S2-2 G18 lost (Rust panic → instance abort on wasm) | **accepted** (S2's statement) | unchanged by S8; mitigation stays: no panicking paths in pin_script's Lua host code (`clippy::unwrap_used/expect_used/panic` deny), host re-instantiates on `Module terminated` |
| S2-3 address-keyed `pairs` | **closed** | KHZ 34 (+ KHZ 37 for the related string cache, F5); authoring rule kept (§5) |
| S2-4 non-V8 engines (JSC, SpiderMonkey) | **open** | needs S5 / real devices: run gates 1–3 in Safari and Firefox (≈ 0.5 agent-run once a runner exists); ARM64 V8 now measured (F6 found and fixed) |
| S2-5 mlua-sys's Rust `LUAL_BUFFERSIZE` 512 on wasm32 | **accepted** | neither mlua nor pin_script uses `luaL_Buffer` from Rust (grep gate); the C value is 1 024 everywhere (KHZ 31) |
| S2-6 KHZ 19 full-GC charge is heap-size based | **closed** | KHZ 33: charged from logical bytes; `b_gc_engine` reaches the cap with 71 emergency collections at identical points on every engine |
| S2 (§10, S2-4) `collectgarbage` removed for parity | **closed (unchanged)** | keep the removal; if ever restored, answer from `lua_gc(LUA_GCCOUNT)` (logical with KHZ 33), never mlua's `used_memory()` |
| S4 per-tick aggregate queue line | **closed with a recommendation** | §6: 200 000 units per tick, fault on exhaustion, prototyped in both backends; owner decision required (very-high blast radius) |

## 9. Native test suites (scratch tree)

`S/harness/run_native_tests.sh base|v2` = `cargo test -p pin_script --no-fail-fast` on `S/repo` (default
features: Lua + wasmtime + hot reload; 24 test binaries incl. `spec_sandbox`, `spec_hardening`, `spec_stall` +
attack corpus, `spec_faults`, `spec_api`, `spec_determinism`, `spec_crossproc`, `cr_lua_budget`,
`cr_weak_tables`, `lua_backend`, `lua_alloc`, `spec_reload`, the Wasm suites):

| build | passed | failed | ignored | log |
|---|---|---|---|---|
| base (no knobs: S2's base, KHZ 29–32 compiled, `KHZ_LUA_MATH` off) | 471 | 0 | 5 (the repo's existing timing/report tests) | `logs/native_tests_base.log` |
| v2 knobs (`KHZ_LUA_MATH`, KHZ 33–37; default features, so `collectgarbage("count")` stays) | 471 | 0 | 5 | `logs/native_tests_v2.log` |

No existing test changes with KHZ 33–37: none of them pins the exact allocation at which the cap trips, a GC
schedule, a NaN text, a position ≥ 2^32 or an address-keyed order (which is also why F1–F6 went unseen).
`lua_backend::emergency_gc_is_charged` still ends in `BudgetExceeded` (it fills the heap until
`collectgarbage("count")` — mlua's real counter, ≈ logical on x86_64 — passes 1 700 KiB, then makes 60 KB
strings so that emergency collections are charged; KHZ 19 now charges logical bytes, which are as large). With `web-parity` the two
`collectgarbage` tests S2 listed are expected to fail as in S2 §12 (not re-run here; S8 does not touch them). lua-src has no tests of its own
(KHZ_PATCHES: "test target removed"); the C changes compile warning-free with `gcc -std=gnu99 -Wall -Wextra`
(x86_64, all knob combinations) and with wasi-sdk clang `-Wall -Wextra` (wasm32), and the LP64 compile-time
checks of KHZ 33 hold (x86_64; aarch64 is LP64 with the same layout, S7). Lint: `cargo fmt --check -p
pin_script` clean on the scratch tree; `rustfmt --check --edition 2024 third_party/lua-src/src/lib.rs` clean;
the harness crates (`s2_harness`, `s4_rules`) are spike tooling and were not linted (S2's own harness was not
rustfmt-clean either).

## 10. Recommendations

1. **Adopt KHZ 33–37 in 9.10** together with KHZ 29–32, all unconditional (drop the knobs): KHZ 33 logical
   accounting + cap + 2× backstop (pin_script `apply_memory_cap`), KHZ 34 creation ids, KHZ 35 position
   saturation, KHZ 36 NaN text, KHZ 37 content-indexed string cache; add the KHZ_PATCHES.md rows 33–37 (texts in
   `patches/0001` comments).
2. **Adopt the per-tick aggregate cap** (200 000 units, §6.3) in pin_script's Lua and Wasm backends, natively
   and on the web, at the same time as 9.5/9.10 (one replay-semantics change, not two).
3. **Add the CI gates in §7**; port the probes as `pin_script/tests/spec_lua_platform.rs` (native goldens) and
   the web workflow's native-vs-wasm comparison.
4. Native hardening: `-ffp-contract=off` on every lua-src arm (queue line).
5. Promise statement for the issue: §7.

## 11. Residual risks

* JavaScriptCore/SpiderMonkey and real ARM hardware not measured (S5); ARM64 V8 measured under qemu only.
* The audit is exhaustive for the patterns in §3.1 over the compiled sources, but a future Lua or mlua
  upgrade can add new allocation sites (gate 2 catches unaccounted ones) or new address uses (gate 3).
* Full-userdata logical size is a constant (64): a future mlua userdata larger than ~2× that would let real
  memory exceed logical natively; the 2× backstop still bounds it (classification `MemoryLimit`, point not
  exact) — gate: the `REAL` payload line in the probes.
* Wasm-rules per-tick cap was run natively and in Node, not in Chromium (S8-10); fuel is engine-independent
  (S4) and the rule is shared Rust.
* Timing numbers come from a shared 4-core VM with concurrent builds of other spikes; hashes, units, outcomes,
  digests and heap values re-run exactly; wall times are indicative (observed run-to-run spread up to ±40 %).

## 12. Effort estimate (follow-up implementation)

* lua-src KHZ 33–37 (C, ~350 lines incl. comments) + pin_script cap/backstop wiring + KHZ_PATCHES rows: **1 T2
  implementer run**; independent tester porting the probes/corpus/heap-trace gates: **1 T2 run**; review/fix:
  1–2 runs. Blast radius very high (vendored Lua) → T3 would be warranted for review; Fable is not available
  (D42), so T2 with the gates above.
* Per-tick cap in both backends + tests: **1 T2 run** + tester **1 T2 run**, after the owner's decision.
* CI workflow additions: **1 T1 run**. Total ≈ 6–7 agent-runs, T2.

## 13. Commands index

```sh
. S/env.sh
S/harness/build_variant.sh v0|v1|v2          # native + wasm + wasm-bindgen -> S/runs/bin/<v>/
S/run_audit.sh v2                            # (SKIP_BUILD=1 to reuse runs/bin; ARM=1 adds ARM64 V8 via qemu)
SKIP_BUILD=1 ARM=1 S/run_audit.sh v0         # the "before" table
S/probes/c/run_fmt_cmp.sh 1000000            # glibc vs wasi-libc printf/strtod
S/probes/c/build_sizes.sh                    # sizeof/offsetof x86_64 vs wasm32
S/run_memcap_parity.sh                       # sandbox corpus v0/v1/v2 + budget-edge + production replays (v2)
S/run_pairs_order.sh                         # pairs determinism, header bytes, per-callback cost
S/harness/bench_addrkeys.sh                  # KHZ 34 micro-benchmark
S/run_tick_budget.sh v2                      # per-tick totals, adversarial ticks, cap on/off (Lua + Wasm)
ONLY_WASM=1 S/run_tick_budget.sh v2          # the Wasm-rules part only
S/harness/run_native_tests.sh base|v2        # pin_script's own native suite
# single probe: S/runs/bin/v2/s2 s8 probe @<file.lua> big 1 ; node S/node/run.cjs S/runs/bin/v2/pkg_node probe @<file.lua> big 1
# Wasm rules: S8_TICK_CAP=200000 S/runs/bin/s4/s4 s8adv wasmi-instr-depth nop 512 3
```
