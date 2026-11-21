# S2 — web-spike-lua: Lua 5.4.9 + Kilohertz patches on the web (RESULTS)

Base: `main` at `995935f91ad8ad18fcee201b94df52c5aaf1fb6c` (S1's `pristine`/`repo` copies; S1 patches 0001–0004
are in `S/repo`). Toolchain rustc 1.97.0 (LLVM 22.1.6); Node v22.23.5 (V8 12.4.254.21-node.57); Playwright
Chromium 157.0.8103.78; wasm-bindgen CLI + crate `=0.2.130` and wasm-tools 1.263.0 (binaries copied from S1's
`tools/bin`, sha256 in §2). Every cargo command used the COMMON env (`env.sh`: `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s2[/<flavour>]`, `CARGO_INCREMENTAL=0`,
`CARGO_PROFILE_{DEV,TEST}_DEBUG=line-tables-only`). `S` =
`${WEB_SPIKES}/S2`,
`S1` = the sibling S1 directory (read only: recordings, native hashes, bundles).

## 0. Bottom line

* **The real vendored Lua 5.4.9 + all 28 KHZ patches runs in the browser inside the engine's own
  `wasm32-unknown-unknown` module, driven by the unchanged pin_script host code (mlua 0.12.2,
  `LuaScriptBackend`, sandbox prelude, budget hook, bridge).** Route: lua-src gets a `wasm32-unknown-unknown`
  arm (clang from **wasi-sdk 32** = LLVM 22.1.0, a **wasi-libc** sysroot, `setjmp`/`longjmp` lowered to **Wasm
  exception handling**), mlua needs no change, wasm-bindgen 0.2.130 processes the module.
* **Command-stream parity holds bit for bit**: for the 3 production tables (bot 60 s, input pattern 60 s,
  8-ball 20 s), the CI fixture and the 10-minute soak, every script step (tick, RNG draws, digest of the
  switch edges + `HostView` seen, encoded command queue) and every tick's `state_hash`+`ext_hash` of the
  real-Lua wasm run equals S1's native recording, in Node and in Chromium (§5). Fault injection (runtime
  error, budget kill, memory kill, switch-handler error) recovers identically, every tick (§7).
* **Exact native = web also for user scripts needs four small KHZ patches, all found and proven by this
  spike** (§6): (29) the wasm build arm; (30) one correctly rounded hex-numeral reader and glibc-style `%a`
  for subnormals on every target (wasi-libc's `strtod` is off by 1 ulp on 463/10 000 long hex numerals and
  musl prints subnormal `%a`/`%q` differently on 1 396/1 396 subnormals); (31) `LUAL_BUFFERSIZE` fixed at
  1024 (it is pointer-size based, and KHZ patch 20 charges buffer growth to the budget: the same script
  fit 2091 iterations natively but 815 on wasm32); (32) Lua's transcendental math through the Rust `libm`
  crate on every target (glibc vs musl differ on 0.001–17.9 % of inputs; after: 0 of 2 200 000). Plus
  pin_script feature `web-parity`: drop `collectgarbage` (its "count" is the heap size, pointer-size
  dependent). None of these changes any production table's native output (all 11 scenarios PASS natively
  against the unpatched recordings). Native x86_64 output does change for user scripts in three places, by
  design: transcendental math now gives the Rust `libm` bits (KHZ 32), `collectgarbage` is gone
  (`web-parity`), and subnormal hex numerals are now correctly rounded where glibc 2.39 is wrong (4 in
  2 000 000 random inputs; Python's `float.fromhex` agrees with the new reader).
* **Sandbox: 78 of 79 adversarial cases identical natively, in Node and in Chromium** in the recommended
  build; the one that differs is by construction pointer-size dependent (the exact allocation at which the
  2 MiB cap is reached: same `MemoryLimit` classification, tick 1971 natively vs 1995 on wasm). Without
  `web-parity`, `collectgarbage("count")` (the heap size) differs too. Every guarantee of today's sandbox is kept except one that is **lost on the web**:
  a Rust panic inside a Lua callback is converted into a Lua error natively (mlua `catch_unwind`) but aborts the
  instance on wasm (panic=abort) — wasm-bindgen 0.2.130 then marks the module terminated (§7, §6 G18).
* **Cost**: real-Lua `on_tick` 4.2–5.6 µs mean in Chromium (native 2.4–3.0 µs), switch callbacks 2.3–7 µs;
  engine module +674 KB raw / +223 KB gzip (recommended build, incl. the S2 harness). Lua in a **separate wasm instance (route f) also works** (parity
  identical) at +1.0–1.9 µs per `on_tick`; it is not needed for any guarantee measured here.
* **A10 confirmed** (unmodified mlua/lua-src refuse `wasm32-unknown-unknown`; mlua itself is fine once Lua is
  built). **A25 confirmed for Chromium 157 and Node 22** (legacy and exnref EH both validate and run; JSTag
  present); Safari/Firefox/mobile minima need S5.

## 1. Owned assumptions — verdicts

| # | Verdict | Evidence (pointer) | Reproducing command |
|---|---|---|---|
| A10 | **Confirmed in part, changed in part.** mlua 0.12.2 + mlua-sys 0.13.0 do **not** build for `wasm32-unknown-unknown` unmodified — but only because the (vendored) lua-src build script refuses the target: `panicked at third_party/lua-src/src/lib.rs:100:25: don't know how to build Lua for wasm32-unknown-unknown`. mlua's Rust code itself compiles and runs there unchanged (§4 route b; `SYS_MIN_ALIGN` 8 on wasm32, `C-unwind` ABI fine under panic=abort). mlua **does** support Emscripten (built and ran, route c). **wasm-bindgen 0.2.130 / js-sys do not work on Emscripten**: `emcc` link fails with `undefined symbol: __wbg___wbindgen_throw_…`, `__wbindgen_describe`, `__wbindgen_externref_table_grow`, … (wasm-bindgen's placeholders are only resolved by its CLI for `wasm32-unknown-unknown`). Pure-Rust Lua VMs do not have the semantics: piccolo 0.3.3 gives `-7 // 2 = -3`, `tostring(7.0 // 2) = "3"`, has no `string.format`/`gsub`/`table.sort`/`string.rep`/`utf8`, cannot add tables with `__add`, and has no allocation cap; hematita 0.1.0 is GPL-3.0. | §4 (c), (d); `logs/a10_pristine_mlua_wasm32.log`, `logs/route_c_build.log`, `runs/route_d_piccolo.txt` | `cd S/pristine && CARGO_TARGET_DIR=…/a10 cargo build --release --target wasm32-unknown-unknown -p mlua`; `S/routes/emscripten/run.sh`; `cd S/routes/piccolo && cargo run --release` |
| A25 | **Confirmed (Chromium 157, Node 22.23.5); browser minima: needs S5.** Playwright Chromium 157.0.8103.78: a legacy-EH module (`try/catch`) and an exnref module (`try_table`) both `validate` and run (return 7); `WebAssembly.Tag`, `WebAssembly.Exception`, `WebAssembly.JSTag` present; the full engine+Lua module (legacy EH, 1 `try/catch` from Lua + 36 `try/catch_all/rethrow` export wrappers added by wasm-bindgen) and the exnref build (37 `try_table`) both pass parity in Chromium (isolated, COOP/COEP) and Node. LLVM ≥ 19 is required for the stack-buffer `__wasm_setjmp` ABI; LLVM 18 (Ubuntu clang-18) lowers to `saveSetjmp`/`testSetjmp`/`getTempRet0` + `malloc`/`free` per call (route a). | §8; `runs/parity/chromium_*.json`, `runs/exnref/`, `web/eh_*.wat` | `S/run_lua_parity.sh` (Chromium lines print `eh_legacy`/`eh_exnref`); `S/harness/build_exnref.sh` + `node harness/chromium.cjs S/runs/web_exnref 8771 1 out.json "scen=apex_circuit_bot60&sandbox=0"` |

S5 must confirm (cannot be tested here; Playwright has Chromium only): minimum Safari (macOS/iOS), Firefox
(desktop/Android) and Chrome Android versions for (1) legacy Wasm EH (`try/catch/rethrow`), (2) the JS API
`WebAssembly.Tag`/`WebAssembly.Exception` (used by the wasm-bindgen glue), (3) exnref (`try_table`) and
`WebAssembly.JSTag` if we ever switch, (4) fixed-width SIMD (S1), (5) the main-thread native stack size in
JSC and SpiderMonkey (the deepest Lua C recursion needs ≤ 176 KB of V8 stack, §7).

## 2. Layout, tools and reproduction

* `S/repo` — S1's scratch tree + S2 changes; `S/pristine` — untouched `main`. `S/repo_clang18` — route (a) tree
  (lua-src knobs `KHZ_WASM_LEGACY_EH=omit`, `KHZ_WASM_SJLJ_RT`); `S/repo_wasip1` — proptest without
  fork/timeout for the wasip1 suite run; `S/repo_em` — harness crates rlib-only for the Emscripten engine run.
* `S/patches/0001-lua-src-web-khz-29-32.patch` (third_party/lua-src, against `main`; `git apply --check` clean on
  the real tree), `0002-pin_script-lua-feature-libm-web-parity.patch` (against `main` + S1 0001–0003),
  `0003-s2-spike-tooling.patch` (harness, not for main). Applying S1 0001–0003 + S2 0001–0002 to `pristine`
  reproduces `S/repo`'s pin_script and lua-src byte for byte (`harness/make_patches.sh`, checked with `patch`).
* Harness: `S/repo/crates/s2_harness` (parity checker over the real `LuaScriptBackend`, sandbox cases,
  float/libm/ctype probes, budget-edge probe, stack probes, route (f) remote backend; native binary `s2` and
  wasm-bindgen exports) and `S/repo/crates/s2_lua_module` (route f). Drivers in `S/harness/`.
* Scripts: `S/run_lua_parity.sh [all|base|umath]`, `S/run_lua_sandbox.sh [all|base|umath]` (PASS/FAIL tables),
  `S/harness/{build,build_umath,build_exnref,bench,run_native_tests,run_wasm_suites,make_patches,stage_web}.sh`.
  Variants: **base** = S2 patches without KHZ_LUA_MATH/web-parity; **umath** = the recommended build
  (+ `KHZ_LUA_MATH=1` + pin_script feature `web-parity`).
* Tools (`S/tools`, downloads in `S/dl`, untrusted, not executed from their directory except the toolchains):

| tool | source | sha256 |
|---|---|---|
| wasi-sdk 32.0 (clang 22.1.0, wasi-libc 2fc32bc81b9f, libsetjmp) | `github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-32/wasi-sdk-32.0-x86_64-linux.tar.gz` (143 157 058 B; no published checksum file) | `55fc523ebfbc98f69d1034fcfcb83d1ff5610cd9ab7eceef6cd097a30ba4ef93` |
| emscripten 7.0.0 binaries (route c) | `storage.googleapis.com/webassembly/emscripten-releases-builds/linux/6afbd83493d647051772d491124187d4e9a0cda9/wasm-binaries.tar.xz` (322 310 412 B; hash from emsdk `emscripten-releases-tags.json`, emsdk commit 708dcbcc) | `6e7c6d26cf8bde31e6cf6a377c113f913cb2d30f4c2d671c1042e1c4bcd16495` |
| rust-std 1.97.0 wasm32-unknown-emscripten (route c, scratch sysroot, rustup untouched) | `static.rust-lang.org/dist/2026-07-09/…tar.gz` | `ce95ce0ee0eb6c62154102ef413f88ca4b98446d5758ad7fe580692b3e31d4f3` (= channel manifest) |
| wasm-bindgen 0.2.130 / wasm-tools 1.263.0 | copied from S1 (`cargo install --locked`) | `c0b53399…6eeb` / `52975ee5…dfc8` |

Disk hygiene (shared 30 GB disk): `target/web_s2` was removed at the end (every script rebuilds); the
emscripten install (1.6 GB) and the downloaded tarballs were deleted after recording their checksums —
`routes/emscripten/run.sh` re-downloads and verifies them (run it before `run_engine.sh`); the extracted
wasi-sdk (`S/tools/wasi-sdk`, 461 MB) is kept for the build scripts.

## 3. Req 1 — what the Lua sandbox consists of today (guarantees G1–G19)

Sources: `crates/pin_script/src/lua/{sandbox,mod,bridge,api}.rs`, `third_party/lua-src/KHZ_PATCHES.md`,
mlua 0.12.2 `src/memory.rs`, DECISIONS "4.2 …", "2.5 — allocation guard and the Lua arena", THREAT_MODEL.

| # | Guarantee (today, native) | Mechanism |
|---|---|---|
| G1 | Library surface: only base/string/table/math/utf8 opened; `io os debug package coroutine jit ffi` = nil; `dofile loadfile load loadstring require warn module` removed; `print` no-op; `collectgarbage` answers only `"count"`; `string.dump/pack/unpack/packsize` removed; `math.randomseed` no-op | `sandbox.rs` `install` |
| G2 | Text chunks only (no bytecode) | `ChunkMode::Text` |
| G3 | `pinball` tables read-only proxies (`__metatable` locked, `rawset` wrapper); string metatable locked | prelude |
| G4 | `setmetatable` refuses `__gc` and `__mode`; `__mode` added later ignored | prelude; KHZ 27 |
| G5 | `pcall`/`xpcall` re-raise memory errors; `xpcall` handler runs after unwinding | prelude |
| G6 | Heap cap (engine 2 MiB): mlua's allocator refuses growth past the limit → Lua memory error. **Fact:** the "arena" is mlua's counting allocator over the global allocator, not a pre-reserved block (no pre-reservation exists in the code) | mlua `memory.rs`; `set_memory_limit` |
| G7 | Instruction budget (engine 100 000/callback): count hook every 1000 instructions spends into the KHZ per-state budget; once exhausted the hook fires every instruction (no `pcall` escape) | `Vm::build`, KHZ 3, 17 |
| G8 | KHZ cost model for C-side work (C calls 32, errors 1024, floats 256, strings/patterns/tables/utf8/buffers/varargs/`__call`/hash chains/full GC by size) | KHZ 3–8, 11–25 |
| G9 | 64 KiB string cap | KHZ 4, 20, 21 |
| G10 | No `<close>`, no `OP_TBC` | KHZ 8, 9 |
| G11 | No addresses (`tostring`, `%p`) | KHZ 5, 10 |
| G12 | Determinism: fixed string-hash seed, fixed sort pivot, host PCG for all randomness | KHZ 1, 2; `api.rs` |
| G13 | Fault semantics: callback all-or-nothing rollback; tilt releases held coils; faulted until a good reload | `mod.rs` `run`/`fault`, `QueueMark` |
| G14 | Error unwinding: `lua_error` → `luaD_throw` → `longjmp` to `luaD_rawrunprotected`; Rust callbacks raise through Rust frames with no live destructors (mlua design) | `ldo.c`, mlua |
| G15 | Stack limits: `LUAI_MAXCCALLS` 200 nested C calls / parser levels → "C stack overflow"; `LUAI_MAXSTACK` 1 000 000 slots; pattern depth 200 | `llimits.h`, `luaconf.h` |
| G16 | API only inside callbacks, strict argument validation, overflow counted not raised | `bridge.rs`, `api.rs` |
| G17 | No host allocation in steady-state callbacks; Lua-VM allocations exempt from the D5 guard | `tests/lua_alloc.rs`, `alloc_guard` |
| G18 | A Rust panic inside a callback becomes a Lua error (`catch_rust_panics`, default on) | mlua |
| G19 | Reload: syntax check and compile under the engine cap; saved state copied under a byte budget | `reload.rs`, `saved.rs` |

## 4. Req 2/3 — routes, real build attempts, outcome

| route | what was built and run | outcome | sandbox kept | parity | per-callback | CI | verdict |
|---|---|---|---|---|---|---|---|
| **(a)** clang-18 (Ubuntu apt) + "small libc" + EH sjlj | Lua C with `clang-18 --target=wasm32-unknown-unknown -mllvm -wasm-enable-sjlj` into the engine module (`S/repo_clang18`). libc surface the Lua core + base/string/table/math/utf8/coroutine libs need (measured with `llvm-nm`, io/os/package/debug excluded): 55 symbols — math `acos asin atan2 cos cosh exp fmod frexp ldexp log log10 log2 pow sin sinh tan tanh`, stdio `snprintf` + `fopen freopen fclose fread getc ferror feof stdin` (luaL_loadfilex), string `memchr memcmp strchr strcmp strcoll strcpy strlen strpbrk strspn strstr strerror`, ctype `isalnum iscntrl ispunct isxdigit tolower toupper`, `strtod realloc free abort errno localeconv time`, plus the sjlj runtime. LLVM 18 needs **`saveSetjmp testSetjmp getTempRet0 __wasm_longjmp` and `malloc`/`free`** (LLVM ≥ 19: `__wasm_setjmp __wasm_setjmp_test __wasm_longjmp`, no malloc). Without a runtime the link **succeeds silently** and the module imports `env.saveSetjmp`, `env.getTempRet0`, `env.testSetjmp` (rustc's wasm link turns undefined C symbols into `env` imports). With a 40-line hand-written LLVM-18 runtime (`c/clang18/sjlj18.c`) + wasi-libc: links, no imports, sandbox output identical to (b), apex bot60 parity PASS. Compiling Lua as C++ (`-fwasm-exceptions`) needs `__cxa_throw`, `__cxa_begin_catch`, `__cxa_end_catch`, `__cxa_allocate_exception`, `__cpp_exception`: wasi-sdk 32's `libc++abi.a` is built without exceptions (only `__cxa_throw_bad_array_new_length`). A Rust `extern "C-unwind"` catch is impossible on stable (panic=abort; S1 A7 `logs/a7_panic_unwind.log`). | works only with a hand-written ABI shim; **`malloc`+`free` on every `luaD_rawrunprotected`** (every `pcall`, every mlua-protected API call) on a second, C heap outside the Lua cap and invisible to the D5 guard (`call $malloc`/`call $free` in its body, vs none in (b)); a hand-written printf/strtod would be the parity risk (§6: even musl and glibc disagree) | all, but G6/G17 weakened (C-heap allocs) | yes | ≈ (b) | apt clang-18 + hand-written runtime | **rejected** |
| **(b)** wasi-sdk 32 clang + wasi-libc sysroot, link into the `wasm32-unknown-unknown` engine module | lua-src arm (KHZ 29): `--sysroot`, `-isystem …/wasm32-wasip1`, `-mllvm -wasm-enable-sjlj -mllvm -wasm-use-legacy-eh=true`, `-ffp-contract=off`, `_WASI_EMULATED_SIGNAL` (header only), io/os/loadlib/dblib not compiled, shim stubs; links `libc.a` + `libsetjmp.a` | **works**: final module imports only wasm-bindgen glue (no `env`, no `wasi_snapshot_preview1`, §8); wasm-bindgen 0.2.130 accepts EH (adds `__wbindgen_jstag`/`__wbindgen_wrapped_jstag` tag imports and per-export SP-restoring `try/catch_all` wrappers) | all but G18 (lost: panic=abort) and G6 point (pointer size) | **bit-identical** (§5) | Chromium on_tick 4.2–5.6 µs | one checksum-pinned tarball (143 MB, cacheable) + env vars | **winner** |
| (b′) wasip1 artifact in the browser with a WASI shim | lua-src's existing `*wasi*` arm, `--target wasm32-wasip1`; used to run the repo's own Lua test suites under `node:wasi` (§12) | builds and runs; but the engine is `wasm32-unknown-unknown` (wasm-bindgen/wgpu), so wasip1 only fits route (f) | — | — | — | same tarball | used for tests only |
| **(c)** `wasm32-unknown-emscripten` | emscripten 7.0.0 obtained (§2); rust-std for the target into a scratch sysroot; `mlua` (vendored KHZ lua-src, its emscripten arm `-sSUPPORT_LONGJMP=wasm`) built and run under Node: Lua 5.4 semantics, `pcall`/`error`, the 2 MiB cap and a budget hook all work. **The whole engine + real Lua** (the S2 CLI, `-sNODERAWFS`, harness crates rlib-only because emcc cannot link their cdylibs: `relocation R_WASM_MEMORY_ADDR_SLEB cannot be used against symbol` in non-PIC C objects) **passes bot60 parity × 3 bit for bit** (`routes/emscripten/run_engine.sh`). Adding `wasm-bindgen =0.2.130` + `js-sys`: **link fails** (`undefined symbol: __wbg___wbindgen_throw_…` and 8 more) | Lua and the sim work; **wasm-bindgen/js-sys/web-sys, hence wgpu's WebGPU backend and winit, cannot be used**: the whole app would need another JS/GPU stack | same mlua/pin_script code (sandbox cases not re-run under emcc) | yes (3 tables) | on_tick 4.2–4.8 µs (Node) | 322 MB SDK + target | **rejected** (blocks S3's render path) |
| **(d)** pure-Rust VM | piccolo 0.3.3 built natively (`S/routes/piccolo`), 18 probes: `goto`, bitwise, int overflow, fuel (stops `while true`) OK; **`-7 // 2` = -3 (Lua: -4)**, `tostring(7.0 // 2)` = "3" (Lua "3.0"), `tostring(1/3)` = 0.3333333333333333 (Lua 0.33333333333333), `1e15` → "1000000000000000" (Lua "1e+15"), no `string.format`/`gsub`/`rep`/`table.sort`/`coroutine.wrap`/`utf8`, `__add` on tables "cannot add values", no allocation cap; hematita 0.1.0 is GPL-3.0 (not built) | the production tables use `//`, `..` with numbers, `pairs`/`next`: piccolo's `//` and number formatting alone break them; none of the 28 KHZ patches exist | no (budget units, string caps, hash seed all missing) | no | — | — | **rejected** |
| **(e)** mlua vs raw `lua-src` + thin binding | mlua 0.12.2 builds and runs on the target unchanged once Lua is built (route b) | keeping mlua keeps pin_script's host logic (prelude, hook, bridge, error classification) identical natively and on the web — the parity evidence depends on it; a raw binding would re-implement `Function::call`, the hook trampoline, error conversion and the memory-limited allocator | — | — | — | — | **keep mlua** |
| **(f)** Lua in its own wasm instance | `s2_lua_module` (pin_script `LuaScriptBackend` + Lua + wasi-libc, raw C-ABI exports `l_buf/l_new/l_call`, 637 KB raw / 205 KB gzip) instantiated beside the engine; per callback the engine writes `HostView` (553 B) + RNG state (16 B) + the current queue, JS copies it into the Lua instance, `l_call` runs the real backend, the queue + RNG + fault state come back | **works**: 3 × bot60 parity bit-identical (every step and tick); budgets and the cap are the same code; a trap stays in the Lua instance (JS catches it, the remote backend reports a fault) | all; trap containment better than (b) | yes | +1.0 … +1.9 µs mean per `on_tick` (Node), whole-run wall within noise of (b) | same toolchain + a second module + JS glue | **not chosen**: no guarantee needs it; keep as the fallback if a C-side trap is ever found (§11) |

Winner by the brief's criteria in order: sandbox guarantees (b) = (f) = (c) > (a) (C-heap allocations) ≫ (d);
(c) is out regardless because wasm-bindgen/web-sys (S3's render path) cannot link under Emscripten; parity
(b) = (f) = (a) = (c); per-callback cost (b) < (f); CI complexity (b) = one pinned tarball, (f) = (b) + a
second module and glue; size (b) smaller. **Route (b), mlua kept.**

## 5. Req 4 — command-stream parity with the real Lua (production tables)

`S/run_lua_parity.sh all` (final: `logs/run_lua_parity_final.log`, 80 PASS lines, 0 FAIL, exit 0): for each scenario the
native S2 build (both variants), the wasm build under Node (both variants) and Chromium (bot60 + mb8 ×
3 tables, both variants) run the **real** `rules.lua` and are compared with S1's native recording **every
script step** (tick, RNG draws, input digest, encoded command queue) and **every tick** (`state_hash` and
the stronger `ext_hash`).

| scenario | steps | queue bytes | ticks hashed | native (base, umath) | wasm Node (base, umath) | Chromium (base, umath) | final state_hash |
|---|---|---|---|---|---|---|---|
| replay_table_fixture10k | 10 001 | 3 868 | 10 001 | PASS, PASS | PASS, PASS | — | 25097c21c2c3adc1 (documented CI value) |
| apex_circuit_bot60 | 60 001 | 3 290 | 60 001 | PASS, PASS | PASS, PASS | PASS, PASS | 2b7b1eec564b8cdc |
| apex_circuit_pat60 | 60 001 | 4 629 | 60 001 | PASS, PASS | PASS, PASS | — | b9c2a55ba04954e9 |
| apex_circuit_mb8 | 20 001 | 4 471 | 20 001 | PASS, PASS | PASS, PASS | PASS, PASS | 2c52fb33d519ffb7 |
| sky_deck_bot60 | 60 001 | 4 123 | 60 001 | PASS, PASS | PASS, PASS | PASS, PASS | 1f0509f5802dc2e5 |
| sky_deck_pat60 | 60 001 | 1 836 | 60 001 | PASS, PASS | PASS, PASS | — | ce9eb11d97fbef81 |
| sky_deck_mb8 | 20 001 | 1 378 | 20 001 | PASS, PASS | PASS, PASS | PASS, PASS | 777557dae8e5531b |
| feature_lab_bot60 | 60 001 | 5 815 | 60 001 | PASS, PASS | PASS, PASS | PASS, PASS | 2040cea9ef77a2e0 |
| feature_lab_pat60 | 60 001 | 1 754 | 60 001 | PASS, PASS | PASS, PASS | — | 865f852fce618b7e |
| feature_lab_mb8 | 20 001 | 3 940 | 20 001 | PASS, PASS | PASS, PASS | PASS, PASS | 0c64556eecf5c210 |
| apex_circuit_bot600 (10 min) | 600 001 | 42 991 | 600 001 | PASS, PASS | PASS, PASS | — | a4b944ad641338d1 |

The final hashes equal S1's table (S1 RESULTS §5), i.e. the native S2 build with KHZ 29–32 + web-parity
reproduces the unpatched engine for all three tables. Route (f) (separate instance) and the exnref build:
bot60 × 3 also PASS (Node; exnref also Chromium). S1's float-parity setting (`+simd128`, 4 MiB stack) is
applied (`wasm_env.sh`). No mismatches.

## 6. Req 5 — sandbox re-verification on the web

### 6.1 Adversarial cases (`S/run_lua_sandbox.sh`, table in `runs/sandbox/<variant>.table.md`)

79 cases through the engine's `LuaScriptBackend` with `LuaConfig::engine()`: infinite loops (plain, in
`pcall`, in an `xpcall` handler, at top level, in a `table.sort` comparator, in a `gsub` callback), budget kill
then reload, memory bombs (`string.rep`, concat doubling, 60/65 KB strings, `pcall`-swallow attempts, table
growth, nested tables, gradual 1 KB/tick), deep Lua recursion (± `pcall`), C-stack recursion through
`__index`, `__tostring`, `gsub`, `sort`, `__lt`, `__concat`, nested `pcall`, `error()` unprotected / with a table
/ with `__tostring` / caught, bad API arguments, errors in metamethods and callbacks, invalid sort order,
load-time errors (syntax, `<close>`, binary chunk, top-level error, deep nesting), `__gc`/`__mode`
refusal, coroutine absence, forbidden libraries, `os.time`, read-only `pinball`, locked string metatable,
pattern backtracking/depth/captures/`%b`, `format` limits and `%p`, address-free `tostring`, `unpack`/`concat`/
`byte` limits, invalid UTF-8, hash-chain charging, the random policy, integer/float semantics, division by
zero, float formatting, NaN, `tonumber` edge strings, transcendental math, `collectgarbage("count")`,
save/restore over reload, failed reload, tilt release of 40 held coils, API outside callbacks, string
coercion limit, vararg charging, cyclic `__call`, late `__mode`. Each line holds the per-callback outcome
(ok / `R:<digest of the message>` / B / M / load error), a digest of every command queue, the fault count
and the RNG state; messages are compared too because scripts can read them through `pcall`.

| variant | cases | identical native = Node = Chromium | differ |
|---|---|---|---|
| base | 79 | 76 | `mem_gradual_keep` (cap point), `collectgarbage_count`, `math_transcendental` (libm) |
| **umath (recommended)** | 79 | **78** | `mem_gradual_keep` only (cap reached at tick 1971 natively, 1995 on wasm and in Chromium; both `MemoryLimit`) |

No case trapped or threw in Node or Chromium (`TRAP` lines: 0).

### 6.2 Number conversion, libm, ctype (exact output, native glibc 2.39 vs wasm wasi-libc)

| probe | base before KHZ 30 | base (KHZ 30, 31) | umath (+ KHZ 32) |
|---|---|---|---|
| 10 000 floats (incl. 1 396 subnormals, ±0, inf, edge values) × `tostring` + 23 `string.format` conversions (`%.14g %.17g %a %.3a %.0a %A %e %.3e %.0e %f %.3f %.0f %g %.3g %.0g %#g %10.4f %-24.10e %+.5g % .2e %.20g %.40e %.60f`) + `%q` + 2 `tonumber` round trips = 367 195 fields | 1 396 values differ, only `%a %.3a %.0a %A %q` of subnormals (glibc `0x0.0000000000001p-1022`, musl `0x1p-1074`) | **identical** | **identical** |
| 10 000 `tonumber` strings (decimal up to 200 chars, near-halfway 26-digit, hex floats, edge) → exact bits | 463 differ, all long hex numerals, wasm off by 1 ulp (glibc = Python `float.fromhex` on all 463) | **identical** (native unchanged) | **identical** |
| 256 bytes × 10 pattern classes + upper/lower | identical | identical | identical |
| `math.*` and `^`, 100 000 inputs each, exact bits | differ: sin 3 036, cos 3 127, tan 3 581, asin 6 021, acos 7 597, atan 2 088, atan2 17 905, exp 52, log 1, log2 8, log10 7 240, log(x,b) 3, pow 71, cosh 73, sinh 9 484, tanh 68; identical: sqrt fmod floor frexp ldexp `%` | same as left | **0 of 2 200 000 differ** (and equal to the Rust `libm` crate) |

The "before KHZ 30" column is reproducible without rebuilding Lua: `c/libc_cmp/libc_cmp.c` calls the C
libraries directly (built natively and with wasi-sdk for wasip1, run under `node:wasi`): `strtod` on the same
10 000 strings differs on 463 (all long hex numerals), `%a`/`%.3a`/`%A` differ on 10 000 of 10 000 subnormals
and on 0 normals (the 10 "normal" lines that differ are subnormals drawn by the random generator).

Musl's transcendental results equal the Rust `libm` crate for sin/cos/tan/asin/acos/atan/atan2/log10/sinh/tanh
(0 differences) — the crate is a musl port; glibc differs (and glibc selects FMA variants at run time on this
CPU, `fma`/`avx2` present, so native results can also depend on the machine). The C self-test of
`lkhznum.c` vs glibc (`c/numtest/numtest 2000000`): `%a` 0 mismatches on 2 000 000 random formats (1 333 333
subnormals, random flags/width/precision); hex parsing 4 mismatches, all subnormal, where **glibc is wrong**
(Python agrees with `khz_strx2number`), `endptr` 0 mismatches.

### 6.3 Budget accounting (`budget-edge` probe: largest iteration count that still fits 100 000)

| operation | native | wasm before KHZ 31 | wasm with KHZ 31 |
|---|---|---|---|
| `table.concat({s700})` | 2091 | **815** | 2091 |
| `string.format('%s', s700)` | 1092 | **602** | 1092 |
| `string.format('%s', s400)` | 1436 | 1436 | 1436 |
| `gsub` on 600 B | 47 | **46** | 47 |
| `string.rep('y', 900)` | 1483 | **692** | 1483 |
| `a..a..a` (900 B), arithmetic, small tables | 4997 / 33664 / 16832 | same | same |

Cause: `LUAL_BUFFERSIZE = 16*sizeof(void*)*sizeof(lua_Number)` is 1024 on x86_64 and 512 on wasm32; KHZ 20
charges each `luaL_Buffer` growth (`newsize >> 6`). No existing test could see it. Files:
`runs/budget/base.{native,wasm}.txt` (before), `runs/budget/fixed.{native,wasm}.txt` (after; native identical
before and after); the final runs repeat it as a gate (`run_lua_sandbox.sh`, "budget-edge" lines).

### 6.4 Guarantee verdicts on the web (recommended build)

| # | verdict | evidence |
|---|---|---|
| G1 | **preserved** (and io/os/package/debug are not even linked: their openers raise) | `forbidden_libs`, `coroutine_absent`, `os_time_absent` |
| G2 | preserved | `error_binary_chunk` |
| G3 | preserved | `pinball_readonly`, `string_mt_locked` |
| G4 | preserved | `gc_meta_refused`, `mode_meta_refused`, `weak_mode_after` |
| G5 | preserved | `mem_pcall_swallow*`, `inf_loop_pcall`, `inf_loop_xpcall_handler` |
| G6 | **preserved as a bound; weakened for parity** — the cap holds and the host survives, but the allocation at which it trips differs (Lua objects are smaller on wasm32) | `mem_gradual_keep` (tick 1971 vs 1995), `deep_recursion*`, `byte_huge` |
| G7 | preserved | `inf_loop*`, budget-edge (after KHZ 31) |
| G8 | preserved after KHZ 31 (weakened before: buffer growth charged differently); one remaining pointer-size term: KHZ 19's full-GC charge (heap bytes/64), reached only by an emergency collection at the cap (G6 regime) | §6.3 |
| G9 | preserved | `mem_string_rep`, `concat_huge`, `byte_huge`, `mem_concat_doubling` |
| G10 | preserved | `error_close_attr` |
| G11 | preserved | `tostring_no_addr`, `format_p` |
| G12 | preserved for string/number keys (`pairs` order identical, `hash_collisions`, `random_policy`: `math.random`/`pinball.random` draw the host PCG identically, `math.randomseed` is a no-op on both; lmathlib's own xoshiro state is seeded from `time()` natively and from the failing clock stub on the web, but the prelude replaces `math.random` before any script runs, so it is unreachable). Tables keyed by **tables/functions** iterate in address order: different on the web **and already different between two native runs** (`runs/sandbox/addr_keys.txt`: 3 native runs give 3 different order checksums because of ASLR; 3 wasm runs give one value, which also moves with the build's heap layout) — pre-existing D9 caveat, no production table does it | command in §13 |
| G13 | preserved | `fault_hold_release`, `budget_recover_reload`, sim-level injection (§7) |
| G14 | preserved: `longjmp` is a Wasm exception (`__c_longjmp` tag) caught only in `luaD_rawrunprotected`, which restores `__stack_pointer`; unwinds Rust frames like native `longjmp` | `sp-after-errors`: SP delta 0 after 40 000 `pcall` errors + 80 faults (native, Node, Chromium) |
| G15 | preserved: same limits, same messages; host stacks have room (shadow stack ≤ 283 KiB (289 798 B, `cstack_gsub`) of 4 MiB; V8 stack ≤ 176 KB of Chromium/Node's ~984 KB) | `cstack_*`, `pattern_depth`, `deep_nesting_load`; §7 |
| G16 | preserved | `api_outside_callback`, `error_api_arg` |
| G17 | preserved: same Rust code; Lua-VM memory goes through mlua's allocator into Rust's global allocator (counted, exempt) on both; no C-heap allocation (route b) | S1 D5 guard on wasm; (a) would break it |
| G18 | **lost**: wasm32 is panic=abort; a Rust panic in a Lua callback (pin_script host function or an mlua internal `expect`) aborts the engine instance; wasm-bindgen then answers every later call with `Error: Module terminated` | `harness/trap_test.cjs` (§7) |
| G19 | preserved (reload logic); the file watcher is native-only by design | `save_restore_reload`, `reload_bad_keeps_old` |

## 7. Req 6 — recovery after errors and traps

* **Engine-level recovery, native vs wasm, every tick** (`run_lua_parity.sh`, "fault injection" lines): the
  table's own `rules.lua` plus an injected runtime error at tick 20 000, an infinite loop at tick 20 000, a
  6.5 MB allocation at tick 20 000, or an error in the 50th `on_switch`, for all 3 tables (12 runs × 2
  variants): every tick's `state_hash`+`ext_hash` identical native vs Node, fault count 1 on both, and the
  stream leaves the unfaulted recording at the same step on both (e.g. apex 22394, sky 20213/8542, feature
  20628/8063). The fallback (rollback, coil release, scripts stop, sim continues) is identical.
* **Traps**: no adversarial case traps the instance (0 `TRAP`/`RangeError` in Node and Chromium). Trap
  sources checked: (1) `unreachable` from C: `abort()` is reached only when a Lua error has no handler
  (`luaD_throw` without `errorJmp`), which mlua never allows; the shim's `proc_exit` stub is unreachable
  (nothing calls `exit`). (2) Out-of-bounds: Lua C uses Lua's own bounds checks; the shim has no buffers.
  (3) Stack: Lua bounds C recursion at 200 levels; measured needs are the shadow stack ≤ 283 KiB (289 798 B, `cstack_gsub`) (gsub
  recursion carries a `luaL_Buffer` and a `MatchState` per level) and V8's native stack ≤ 176 KB
  (`harness/stack_need.cjs`: smallest `--stack-size` per case; pcall nesting 176, tostring/sort 160, gsub
  136, concat 112, lt 96, index 80, all others 48), identical under `--liftoff --no-wasm-tier-up` and
  `--no-liftoff`. (4) Float→int: the Lua module (route f build, same code as in the engine) has 66 saturating
  `trunc_sat` conversions and exactly one trapping `i64.trunc_f64_s`, in mlua's `Value::eq`, preceded by an
  explicit `-2^63 <= x < 2^63` check (NaN and out-of-range values branch away), so it cannot trap; Lua C
  range-checks its own casts (`lua_numbertointeger`).
* **After a trap** (a Rust panic: S1's `panic_in_step`): wasm-bindgen 0.2.130 in EH mode calls its abort hook,
  sets `__instance_terminated`, and every later export call throws `Error: Module terminated` (`runs/trap/
  terminated.txt`); the per-export `try/catch_all` wrappers restore `__stack_pointer` for escaping exceptions,
  which removes S1's "SP leak after traps" for exceptions (not for traps). The web host must therefore
  re-instantiate after a trap (S1's recommendation stands); a Lua script cannot cause one in the cases tested.
* `sp-after-errors` (native / Node / Chromium): delta 0.

## 8. Req 7 — A10, A25, wasm-bindgen ↔ C interaction

* A10/A25: §1.
* **Imports of the final module** (`runs/imports/pkg_web_base.txt`, `WebAssembly.Module.imports`): only
  `./s2_harness_bg.js` functions (`__wbg_error_*` = console.error, `__wbg_now_*` = performance.now,
  `__wbg_s2_remote_*` (route f harness), `__wbg___wbindgen_memory_*`, `__wbindgen_init_externref_table`,
  one `Ref(String) -> Externref` intrinsic) and two tags `__wbindgen_jstag`, `__wbindgen_wrapped_jstag`.
  **No `env.*`, no `wasi_snapshot_preview1.*`.** wasi-libc's syscall wrappers reference
  `__imported_wasi_snapshot_preview1_*`; the shim defines every one Lua reaches (fd_write/read/close/seek/
  renumber/fdstat/filestat/prestat, path_open, clock_time_get, environ, random_get, proc_exit) as failing
  functions, which wasm-ld resolves instead of importing.
* **Symbols**: C symbols keep their C names (`lua_*`, `luaL_*`, `khz_*`); Lua's own are not exported by the
  cdylib; Rust `#[no_mangle] extern "C"` functions (`khz_sin`…, `l_*`) are exported as plain exports (harmless,
  visible). **Danger:** rustc's `wasm32-unknown-unknown` link does not fail on undefined C symbols — they
  become `env` imports and the module fails only at instantiation (route a without runtime: `env.saveSetjmp`,
  `env.getTempRet0`, `env.testSetjmp`). The web CI must fail on any import outside the glue module.
* **Constructors/destructors**: `snprintf` pulls musl's `__stdio_exit` in as a destructor; wasm-ld then wraps
  every export as a "command export" that calls `__wasm_call_dtors` after the call (wasm-bindgen keeps the
  wrappers, adding its SP wrapper around them). Cost: one call that walks an empty FILE list. KHZ 29's shim
  stubs `fopen/freopen/fclose/fread/getc/ferror/feof/stdin` so no FILE code or `path_open` path is linked.
* EH in wasm-bindgen 0.2.130: once the module contains EH it imports two `WebAssembly.Tag`s, wraps exports in
  `try/catch_all` (legacy) or `try_table` (exnref build) that restore the stack pointer, wraps imports to
  rethrow JS exceptions as wasm exceptions, and adds the terminated-instance guard (§7). Both encodings work.

## 9. Benchmarks (per callback, real Lua, recommended build)

`S/harness/bench.sh 3` (median of 3 reps; native and Node per-callback with a clock read around each call;
Chromium from the isolated parity page: `performance.now()` 5 µs granularity, so p99 is quantised; machine
load 5–6 on 4 cores during the runs). Raw: `runs/bench/`. `native`, `wasm_node`, `chromium` = recommended build;
`remote_node` (route f), `route_a_clang18` and `route_c_emscripten` = base build (no `KHZ_LUA_MATH`; the tables
call no math function, so the work is identical); routes (a) and (c) are single runs.

| scenario | route | reps | on_tick n | on_tick mean us | on_tick p99 us | on_tick max us | on_switch n | on_switch mean us | on_switch p99 us | on_init us | run wall ms |
|---|---|---|---|---|---|---|---|---|---|---|---|
| apex_circuit_bot60 | native | 3 | 60000 | 2.44 | 13.5 | 328 | 430 | 1.66 | 6.9 | 27 | 445 |
| apex_circuit_bot60 | wasm_node | 3 | 60000 | 4.58 | 18.8 | 7741 | 430 | 3.85 | 34.1 | 5097 | 804 |
| apex_circuit_bot60 | remote_node | 3 | 60000 | 5.59 | 20.5 | 4699 | 430 | 5.17 | 67.9 | 3262 | 1224 |
| apex_circuit_bot60 | chromium | 1 | 60000 | 5.56 | 25.0 | 5540 | 430 | 7.05 | 45.0 | 495 | 1283 |
| apex_circuit_bot60 | route_a_clang18 | 1 | 60000 | 5.25 | 23.0 | 9654 | 430 | 4.05 | 57.5 | 440 | 817 |
| apex_circuit_bot60 | route_c_emscripten | 1 | 60000 | 4.16 | 17.9 | 8078 | 430 | 3.92 | 61.3 | 549 | 705 |
| apex_circuit_mb8 | native | 3 | 20000 | 2.71 | 13.9 | 94 | 905 | 1.35 | 4.8 | 25 | 354 |
| apex_circuit_mb8 | wasm_node | 3 | 20000 | 5.39 | 24.6 | 5407 | 905 | 2.80 | 11.1 | 462 | 617 |
| apex_circuit_mb8 | remote_node | 3 | 20000 | 7.29 | 35.6 | 1440 | 905 | 4.99 | 27.6 | 881 | 842 |
| apex_circuit_mb8 | chromium | 1 | 20000 | 4.40 | 25.0 | 520 | 905 | 2.65 | 15.0 | 50 | 620 |
| feature_lab_bot60 | native | 3 | 60000 | 2.82 | 13.3 | 2462 | 488 | 2.24 | 7.8 | 25 | 508 |
| feature_lab_bot60 | wasm_node | 3 | 60000 | 4.61 | 17.5 | 8097 | 488 | 4.17 | 28.2 | 553 | 789 |
| feature_lab_bot60 | remote_node | 3 | 60000 | 5.70 | 19.4 | 2112 | 488 | 6.23 | 70.3 | 964 | 1257 |
| feature_lab_bot60 | chromium | 1 | 60000 | 4.58 | 20.0 | 5495 | 488 | 3.61 | 15.0 | 185 | 1120 |
| feature_lab_bot60 | route_c_emscripten | 1 | 60000 | 4.58 | 17.6 | 8106 | 488 | 5.25 | 56.8 | 798 | 838 |
| feature_lab_mb8 | native | 3 | 20000 | 2.77 | 13.1 | 431 | 655 | 1.96 | 6.2 | 22 | 257 |
| feature_lab_mb8 | wasm_node | 3 | 20000 | 5.65 | 22.2 | 4089 | 655 | 3.79 | 17.7 | 559 | 515 |
| feature_lab_mb8 | remote_node | 3 | 20000 | 6.79 | 25.8 | 2615 | 655 | 5.40 | 22.8 | 998 | 649 |
| feature_lab_mb8 | chromium | 1 | 20000 | 4.18 | 20.0 | 405 | 655 | 3.05 | 10.0 | 35 | 501 |
| sky_deck_bot60 | native | 3 | 60000 | 2.43 | 12.1 | 905 | 396 | 1.36 | 4.0 | 17 | 451 |
| sky_deck_bot60 | wasm_node | 3 | 60000 | 3.86 | 15.3 | 4109 | 396 | 3.01 | 29.9 | 450 | 687 |
| sky_deck_bot60 | remote_node | 3 | 60000 | 5.64 | 20.1 | 12098 | 396 | 4.38 | 45.1 | 946 | 1217 |
| sky_deck_bot60 | chromium | 1 | 60000 | 4.42 | 20.0 | 3775 | 396 | 3.07 | 10.0 | 130 | 1109 |
| sky_deck_bot60 | route_c_emscripten | 1 | 60000 | 4.76 | 18.6 | 12102 | 396 | 3.38 | 21.0 | 692 | 813 |
| sky_deck_mb8 | native | 3 | 20000 | 3.04 | 13.4 | 295 | 594 | 1.35 | 3.6 | 17 | 1078 |
| sky_deck_mb8 | wasm_node | 3 | 20000 | 6.21 | 24.1 | 3572 | 594 | 2.46 | 10.1 | 496 | 1546 |
| sky_deck_mb8 | remote_node | 3 | 20000 | 7.55 | 27.9 | 7901 | 594 | 4.47 | 17.7 | 954 | 1545 |
| sky_deck_mb8 | chromium | 1 | 20000 | 4.41 | 20.0 | 380 | 594 | 2.32 | 10.0 | 25 | 1396 |

Native real Lua 2.4–3.0 µs per `on_tick` (mean); wasm in-module 4.2–5.6 µs (Chromium) / 3.9–6.2 µs (Node,
includes the wasm-bindgen-wrapped clock import); route (f) adds 1.0–1.9 µs (Node, same clock); route (c)
(emscripten, Node) 4.2–4.8 µs; route (a) (clang-18 + LLVM-18 runtime, Node, apex bot60) 5.25 µs. At 1–3 balls (S1) a tick costs 6–8 µs with real
Lua natively; a 60 Hz frame (16–17 ticks) therefore spends ≈ 70–80 µs in Lua callbacks in Chromium. Whole-run
wall time in-module vs route (f) (apex bot60, hash checks on both): 1840/1647, 1514/1624, 1622/1529 ms —
equal within noise.

Size (wasm-bindgen `--target web`, release): engine + harness with Lua, recommended build 4 029 602 B raw /
1 125 631 B gzip -9 (base 4 063 268 / 1 147 435), vs S1's same build without Lua 3 355 422 / 903 030
(+674 KB / +223 KB, which includes the S2 harness code); the standalone Lua module of route (f) (pin_script Lua
backend + Lua + wasi-libc + std) is 636 777 / 205 411.

## 10. Req 8 — recommendation

**Route:** (b). Build the vendored Lua for `wasm32-unknown-unknown` inside the engine module with wasi-sdk 32's
clang and wasi-libc, Wasm-EH setjmp/longjmp (legacy encoding until S5 says exnref is everywhere in scope), keep
mlua and pin_script's host logic unchanged. Do not use a separate Lua instance, Emscripten, a pure-Rust VM or
a hand-written libc.

**Exact changes** (diffs in `S/patches/`):
1. `third_party/lua-src` (0001): KHZ 29 (wasm arm + `khz-wasm/khz_wasm_shim.c`), KHZ 30 (`lkhznum.c`:
   `khz_strx2number`, `khz_number2strx`), KHZ 31 (`LUAL_BUFFERSIZE 1024`), KHZ 32 (`KHZ_MATHOP` routing; make
   it unconditional — the spike used the `KHZ_LUA_MATH` env knob only to compare), `KHZ_PATCHED_SOURCES`
   gains the new/changed files (`lkhznum.c`, `luaconf.h`, `lmathlib.c`, the shim), `cargo:rerun-if-env-changed`
   for `KHZ_WASI_SYSROOT`/`KHZ_WASM_LEGACY_EH`, KHZ_PATCHES.md rows 29–32.
2. `crates/pin_script` (0002, after S1's 0001): feature `lua` (mlua + libm) apart from `native-backends`
   (wasmtime + notify); `lua/libm_abi.rs` (`khz_*` via `libm =0.2.16`, needed on every target once KHZ 32 is
   unconditional); drop `collectgarbage` from the sandbox on every target (proposal S2-4: make `web-parity`
   the only behaviour, not a feature).
3. Web host (9.5/9.7): treat `Error: Module terminated` as an engine crash → re-instantiate (S1); CI import
   check.

**Security boundary:** yes, this modifies the untrusted-table boundary (the vendored Lua and the sandbox
environment). Re-run on native **and** on wasm: pin_script `spec_sandbox`, `spec_hardening`, `spec_stall`
(+ `attack_corpus/`), `spec_faults`, `spec_api`, `spec_determinism`, `spec_crossproc`, `cr_lua_budget`,
`cr_weak_tables`, `lua_backend`, `lua_alloc`, `spec_reload`; pin_sim's Lua replay/soak tests. New tests:
(i) the S2 sandbox case list as a native-vs-wasm gate (outcome + queue + message digests), (ii) the
float-format/parse/libm/ctype dumps native vs wasm, (iii) the `budget-edge` probe native vs wasm (catches any
future pointer-size charge), (iv) `lkhznum.c` vs glibc C self-test plus Python-reference vectors for the 4
glibc-wrong subnormal hex inputs, (v) real-Lua command-stream parity for the 3 tables + fixture (this
spike's checker, native vs Node), (vi) fault-injection recovery parity, (vii) the module import check,
(viii) a test that `collectgarbage` is nil.

**CI** (new web workflow, ubuntu-24.04): `rustup target add wasm32-unknown-unknown wasm32-wasip1`; download
`wasi-sdk-32.0-x86_64-linux.tar.gz`, verify sha256 `55fc523e…ef93`, cache it (`actions/cache`, key =
version + sha); `cargo install wasm-bindgen-cli --version 0.2.130 --locked` (cached) and `wasm-tools
1.263.0`; env `CC_wasm32_unknown_unknown`, `AR_wasm32_unknown_unknown`, `KHZ_WASI_SYSROOT`,
`RUSTFLAGS=-C target-feature=+simd128 -C link-arg=-zstack-size=4194304`; Node 22; Playwright Chromium for a
smoke run. Gates as listed above; native CI keeps running the full pin_script suite with KHZ 29–32.

**Effort:** 9.10-Lua implementation: 1 T2 implementer run (lua-src KHZ 29–32 + pin_script feature/ABI +
build plumbing) + 1 T2 independent tester (port the S2 gates into repo tests) + review/fix round (1–2 runs)
+ 1 T1 run for the CI workflow ≈ **4–5 agent runs, T2**, plus the web-host integration already planned in
9.5/9.7.

**Blast radius and T3:** touching the vendored Lua is very-high blast radius (D34 (a)). Evidence for care
from this spike: three independent platform dependencies inside the "deterministic" Lua were invisible to
every existing test (pointer-size buffer charging, libc number conversions — both glibc and musl have
rounding bugs here —, libm), and native `pairs` over address-keyed tables is not even reproducible between
two native runs. Implementation itself can stay T2 because every change is gated by an end-to-end native-vs-
wasm test from this spike. A T3 (Fable) investigation is warranted for one item only: an exhaustive audit of
Lua 5.4.9 + KHZ for other observable 32-bit-vs-64-bit differences (struct-size-dependent growth, GC pacing
reaching the budget, `luaM_` limits, error texts) — the spike found one in KHZ's own cost model that no review
caught. If the owner declines T3, keep the budget-edge and sandbox gates and accept the residual (§11).

## 11. Residual risks

1. **Memory-cap fault point** (S2-6): a script that fills its 2 MiB heap faults at a different allocation on
   wasm32 than on x86_64 (example: tick 1971 vs 1995); classification identical. Replays of such games diverge
   after the fault. Exact fix needs LP64 layout (wasm64/memory64 for a separate Lua instance — browser support:
   S5) or type-aware accounting. Recommendation: accept, document in THREAT_MODEL/D9 notes, keep the test.
2. **G18 lost**: any Rust panic in a callback kills the instance. Mitigation: pin_script's Lua host code has
   no panicking paths by design (validation returns errors); add `clippy::unwrap_used/expect_used/panic` deny
   for `pin_script::lua` and a wasm fuzz run of the API; host re-instantiates on `Module terminated`.
3. Address-keyed `pairs` order (pre-existing, also native-run-to-run): rules must not depend on it (existing
   rule); optional future KHZ patch.
4. Browser engines other than V8 (JSC, SpiderMonkey): EH support versions, stack limits and float behaviour
   are untested here (S5; S1 showed IEEE equality on V8/x86 only).
5. mlua-sys's Rust `LUAL_BUFFERSIZE` stays 512 on wasm32 after KHZ 31; harmless because neither mlua nor
   pin_script uses `luaL_Buffer` from Rust; any future use must use the C value.
6. KHZ 19's full-GC charge is heap-size based (pointer-size dependent) — reachable only by an emergency
   collection at the cap (same regime as 1).

## 12. Native and wasm test suites

* Native `cargo test -p pin_script` on the S2 tree (rule 3; `harness/run_native_tests.sh`):
  base **471 passed, 0 failed**, 5 ignored (the existing report/timing tests; 24 test binaries incl. all Lua and Wasm suites); recommended (`KHZ_LUA_MATH=1`, `--features web-parity`) **469 passed, 2 failed**, 5 ignored — the 2 failures are `lua_backend::sandbox_escapes_fail` (asserts `type(collectgarbage('count')) == 'number'`) and `lua_backend::emergency_gc_is_charged` (its script uses `collectgarbage('count')` to approach the cap): both test the API that S2-4 removes and must be rewritten together with that decision (the repo's own `spec_sandbox.lua:57` already accepts "collectgarbage is either gone or …"); every math, budget, stall and determinism test passes with `KHZ_LUA_MATH=1`.
  Logs `logs/native_tests_{base,umath}.log`.
* The repo's own Lua suites compiled for **wasm32-wasip1** (same Lua C via lua-src's wasi arm, wasi-sdk 32,
  exnref EH) and run under `node:wasi` with `--test-threads=1` (`harness/run_wasm_suites.sh`):
  unit tests 16/16, `cr_lua_budget` 11/11, `cr_weak_tables` 1/1, `lua_alloc` 2/2, `spec_api` 33/33,
  `spec_crossproc` 4/4, `spec_determinism` 14/14, `spec_faults` 48/48, `spec_stall` 75/75 (incl. the whole
  attack corpus) — **204 passed, 0 failed**. Skipped (4): two `cr_lua_budget` tests whose harness needs a
  watchdog thread and two `spec_crossproc` tests that spawn child processes (neither exists on wasip1; their
  behaviour is covered by the sandbox cases `inf_loop_xpcall_handler` and by the run-to-run identical wasm
  output). Not buildable for wasip1 without further splitting: `spec_sandbox`, `spec_hardening`, `lua_backend`
  (they use `lua::reload`, which needs `notify`), `spec_reload`, and the wasmtime suites. Needed one
  scratch-only change: proptest without its `fork`/`timeout` features (`wait-timeout` does not compile for
  wasip1; `S/repo_wasip1/Cargo.toml`). Logs `logs/wasm_suite_base_*.log`.
* Lint on the proposed product code (spike harness code is not product code): `cargo fmt --check -p pin_script`
  clean, `rustfmt --check --edition 2024 third_party/lua-src/src/lib.rs` clean; `cargo clippy -p pin_script
  -- -D warnings` clean for `--all-targets` (default features), `--all-targets --features web-parity`,
  `--no-default-features --features lua`, `--no-default-features`, and `--no-default-features --features lua
  --target wasm32-unknown-unknown`. lua-src is not a workspace member (path `[patch]`), so clippy cannot target
  it; it compiles without warnings in every build. pin_sim's suites were not re-run: no Lua code in the repo
  outside pin_script's own tests calls a transcendental function or `collectgarbage` (`grep`), and the CI
  fixture plus all 11 replays are bit-identical natively with the S2 patches (§5).

## 13. Commands index

```sh
. S/env.sh
S/run_lua_parity.sh all        # builds base+umath, native/Node/Chromium parity + fault injection; expect 0 failing
S/run_lua_sandbox.sh all       # adversarial table + float/parse/ctype/libm/budget-edge + stack probes; exit 0 iff no
                               # FAIL outside the known list (mem_gradual_keep; in base also collectgarbage_count,
                               # math_transcendental and the 16 math.* lines) — final: 0 unexpected, 20 known
S/harness/bench.sh 3           # per-callback timing table
S/harness/run_native_tests.sh base|umath ; S/harness/run_wasm_suites.sh base
S/harness/build_exnref.sh      # A25 exnref variant
node S/harness/stack_need.cjs S/runs/pkg_node          # V8 stack per case
node S/harness/trap_test.cjs S/runs/pkg_node S1/bundles/apex_circuit.pinbundle
$T/release/s2 budget-edge ; node S/harness/run_node.cjs S/runs/pkg_node budget-edge
cc -O2 -ffp-contract=off -I S/repo/third_party/lua-src/lua-5.4.9 -o S/c/numtest/numtest S/c/numtest/numtest.c \
   S/repo/third_party/lua-src/lua-5.4.9/{lkhznum,lctype}.c -lm && S/c/numtest/numtest 2000000
S/routes/emscripten/run.sh ; S/routes/emscripten/run_engine.sh ; S/routes/clang18/run.sh
(cd S/routes/piccolo && CARGO_TARGET_DIR=$CARGO_TARGET_DIR/route_d cargo run --release)
(cd S/c/libc_cmp && cc -O2 -o libc_cmp_native libc_cmp.c && S/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux/bin/clang \
   --target=wasm32-wasip1 -O2 -o libc_cmp.wasm libc_cmp.c && ./libc_cmp_native S/runs/floats/parse_samples.txt 10000 > native.txt \
   && node S/harness/run_wasi_test.mjs S/c/libc_cmp/libc_cmp.wasm S/runs/floats/parse_samples.txt 10000 > wasi.txt)
# address-keyed pairs (native nondeterminism):
$T/release/s2 smoke "function on_tick() local t={} for i=1,64 do t[{}]=i end local o=0 for k,v in pairs(t) do o=(o*31+v)%1000000007 end error(tostring(o),0) end" 1
```
