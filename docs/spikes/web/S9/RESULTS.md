# S9 — web-spike-integration: ONE engine module in Chromium (RESULTS)

Base: `main` at `995935f`, scratch copy of `S2/repo` (S1 patches 0001–0004 + S2 0001–0003, incl. KHZ
29–32 in `third_party/lua-src`) + S3's `pin_render` web patches (applied with `patch`, result
byte-identical to `S3/repo/crates/pin_render`) + S3's `web_probe` (renderer/scene port) + S4's
`s4_rules` (wasmi backend, pre-scan, instrumentation). New: `repo/crates/pin_web` (the engine crate).
Toolchain rustc 1.97.0 (LLVM 22.1.6), wasm-bindgen 0.2.130, wasm-tools 1.263.0, wasi-sdk 32.0, native
binaryen version_132 (`wasm-opt`), Node v22.23.5, Playwright 1.63.0 with Chromium 157.0.8103.78
(headless shell, SwiftShader). Every cargo command: COMMON env (`env.sh`: `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s9`, `CARGO_INCREMENTAL=0`,
line-tables-only) + `wasm_env.sh` (all web flags, BOM.md §3). `S` =
`${WEB_SPIKES}/S9`.
The machine was shared with other spikes (load average 2–12 on 4 cores during the runs): timings are
reported, never asserted.

## 0. Bottom line

* **The pieces combine.** One `wasm32-unknown-unknown` module (`dist/pkg/pin_web_bg.wasm`) contains
  the sim built with `+simd128`, the real vendored Lua 5.4.9 + KHZ 1–32 (S2 route b, wasi-sdk 32,
  legacy Wasm EH), wasmi 2.0.2 running S4's instrumented rules, the wgpu 30.0.1 renderer with WebGPU
  **and** WebGL2 (S3's port incl. the WebGL2 prototype and the egui overlay), and the bundle loader
  (`File`/`fetch` bytes → `Vfs::from_zip_bytes` with web limits). It builds with one script
  (`build_web.sh`), imports only wasm-bindgen glue (623 imports from `./pin_web_bg.js`, 0 `env`, 0
  `wasi_*`), and runs under S4's CSP **without COOP/COEP**.
* **Exact native = web holds in the combined module while rendering**: in Chromium, the 3 tables'
  bot60 replays with Lua and with rules.wasm (6 × 60 000 ticks) and the 10-minute soak (600 000 ticks)
  match the native recordings on every tick (`state_hash`, S1's `ext_hash`, S4's command-queue hash;
  Lua also S2's per-step stream check: RNG draws, input digest, command bytes) — rendering every frame
  on WebGPU and on WebGL2. In Node the same module passes all 20 scenario × rules runs (incl. pat60,
  mb8, soak) on the shipped `-Oz` module (and did so on the un-optimised output of an earlier build); the S2 sandbox (79 cases), S4 adversarial corpus (82
  lines), S4 depth-counter limits, S4 zip corpus (33 files), S1 glam-op digests and the D5 allocation
  guard all equal their spike's recorded outputs inside the combined module.
* **Integration bugs found and fixed (all small)**: two global allocators, duplicate wasm-bindgen
  exports and a second `start` from the harness crates (fix: only the engine crate owns the allocator
  and the exports); wgpu callbacks need `Send` data without `fragile-send-sync` (Arc + atomics);
  cargo silently skips LTO for `[cdylib, rlib]` crates (and real LTO made the module bigger: ship
  without); a negative JS tick count wraps to 2^32−k in a `u32` parameter and froze the page (clamp +
  Rust guard); the 11.1 MB module cannot be compiled synchronously on Chromium's main thread (8 MB
  limit: use the async `init()`, never `initSync` in the page); Playwright's `waitForFunction` does not
  work under the CSP (test helper polls with `evaluate`); after a trap the dead instance's pending wgpu
  promise callbacks throw `Module terminated` (the page swallows exactly that); S3's single-canvas PiP
  layout covered the flippers on phone aspects (aspect-aware layout). Conflicts reproduced by
  `scripts/conflicts.sh`.
* **Size**: shipped module 10.62 MiB raw / 3.93 MiB gzip / **2.79 MiB brotli** (both backends + harness
  + egui); engine-only: both backends 2.76 MiB br, **WebGPU-only 2.16 MiB br**, WebGL2-only 2.74 MiB br.
  Biggest code contributors: rules engine (wasmi + 2 wasmparser versions + wasm-encoder) 18 %, WebGL2
  path (naga, wgpu-core/hal, glow) 16 %, Rust core/alloc/std 14 %, egui text stack 13 %, toml/serde 8 %,
  WAT parser 7 %.
* **Load to first frame** (Chromium, SwiftShader, loaded VM): navigation start → first presented frame,
  median of 5: WebGPU **cold 565 ms / warm 153 ms**, WebGL2 cold 865 ms / warm 215 ms (§8).
* **Recommendation**: build the web engine exactly as `build_web.sh` does (BOM.md), ship the WebGPU
  artifact as the primary module and the WebGL2 artifact as the fallback (the loader picks by
  `requestAdapter()`), keep the runtime design of S1/S2/S4 unchanged (main-thread rAF loop, Lua
  in-module, wasmi + instrumentation for every rules module), and turn every check of
  `run_integration.sh` into the web CI gates (§12).

## 1. Assumptions touched by the integration — verdicts

| # | Verdict | Evidence (pointer) | Reproduce |
|---|---|---|---|
| A1 (wgpu WebGPU + WebGL2 on wasm32, egui on the web) | **Confirmed in the combined module** | both backends in one module, chosen at run time; `?backend=webgpu|webgl2|auto`; auto → WebGPU with an adapter, → WebGL2 without `navigator.gpu` and with `requestAdapter() = null`; egui overlay painted on both; 0 GPU errors (§7) | `node node/t_backends.cjs results/backends.json` |
| A9 (SAB / precise timers need COOP/COEP) | **Confirmed, and not needed**: nothing in the combined design needs isolation; all tests run with `crossOriginIsolated === false` (§2, §8) | `results/loadtime_*.json` `crossOriginIsolated` | `node node/t_loadtime.cjs webgpu out.json 1` |
| A20 (sync compile of large modules restricted) | **Confirmed for the engine module**: `new WebAssembly.Module(11 140 164 B)` on the main thread → `RangeError: WebAssembly.Compile is disallowed on the main thread, if the buffer size is larger than 8MB`; `WebAssembly.compile` 26 ms, `compileStreaming` 70 ms. Even the WebGPU-only engine (8 680 313 B) is above 8 MiB. The page uses wasm-bindgen's async `init()` (streaming) | `results/sync_compile.json` | `node node/t_synccompile.cjs results/sync_compile.json` |
| A21 (CSP needs `'wasm-unsafe-eval'`) | **Confirmed**: every page in this spike runs under S4's policy (`script-src 'self' 'wasm-unsafe-eval'`, no inline script/style) | `dist/serve.cjs` | any `node node/t_*.cjs` |
| A22 (CLI = crate version) | **Confirmed** (S3); pinned 0.2.130 both | `logs/build_web.dist.log` | `./build_web.sh` |
| A24 (5–15 MiB raw, a few MiB brotli) | **Confirmed for the whole engine**: 10.62 MiB after `-Oz`, 2.79 MiB brotli (both backends + egui + harness); 8.28 MiB / 2.16 MiB brotli WebGPU-only engine | §8, `results/sizes.md` | `scripts/size_report.sh` |
| A25 (Wasm EH in browsers) | **Confirmed for the combined module in Chromium 157**: legacy EH only (`try` 665, `catch_all` 664, `rethrow` 664, `try_table`/`throw_ref` 0, 2 tag imports `__wbindgen_jstag`/`__wbindgen_wrapped_jstag`); Chromium 157 ≥ S5's legacy-EH floor (Chrome 95) and SIMD floor (91) | §3 | `wasm-tools print dist/pkg/pin_web_bg.wasm \| grep -cE '^\s+try'` |
| A28 (shadow stack fixed at link time) | **Confirmed**: `__stack_pointer` = 4 194 304 (`-zstack-size`), stack-first; deepest measured use 293 395 B (bundle load) — 7 % of 4 MiB; 3 500 × 1 KiB frames fit | §10 | `node node/t_failures.cjs webgpu out.json` |
| A6/A7 (panic = abort; trap behaviour) | **Confirmed in the page**: a Rust panic mid-run → `RuntimeError: unreachable`, later calls `Error: Module terminated`; the page re-instantiates (new glue-module URL) and reloads the table | §10 | same |
| A14 (getrandom) | **Confirmed (S1)**: no cfg flag needed; no crypto import in the module | §4 | `./check_imports.sh` |

## 2. Layout and reproduction

* `S/repo` — the combined scratch tree; `S/repo/crates/pin_web` — the engine crate:
  `src/engine.rs` (bundle → VFS → `Simulation` with the Lua or wasmi backend through S1's
  `ExternalScript`; replay with per-tick parity; live input), `src/web.rs` (wasm-bindgen surface:
  `WebEngine`, `create_gpu`/`WebGpu` incl. the aspect-aware PiP layout `page_routes`, probes), `src/lib.rs` (one `#[global_allocator]`).
  Scratch changes to the copied spike crates: features `exports`/`global-alloc` in `s1_harness`,
  `s2_harness`, `s4_rules`, `web_probe` (default on, off for pin_web); `Driver::snapshot`,
  `CheckingScript::new`.
* `S/build_web.sh` → `S/dist/` (`pkg/pin_web_bg.wasm`, `pkg/pin_web.js`, `index.html`, `main.js`,
  `style.css`, `serve.cjs`, `bundles/` = the 3 tables × {Lua, wasm} zip bundles, `test/` = recordings,
  references, player bundles, hostile zips). `S/env.sh`, `S/wasm_env.sh` hold every variable.
* `S/scripts/make_test_data.py` builds `dist/bundles` + `dist/test` from S1/S2/S4 data (S1's bundles
  and recordings; native references: S1's `native/*.hashes` (Lua) and S4's `out/native/*.engine.hashes`
  (rules.wasm, wasmtime engine path); S2's fault-injection hashes); `S/scripts/get_binaryen.sh`.
* Tests: `S/run_integration.sh` (everything + PASS/FAIL table), `S/check_imports.sh`,
  `S/scripts/conflicts.sh`, `S/scripts/size_report.sh`, `S/node/*.mjs|*.cjs` (Node and Playwright).
* Page: `dist/index.html` + `main.js` (ES module; no inline script/style, CSP-clean). Query:
  `backend=webgpu|webgl2|auto`, `table`, `rules=lua|wasm`, `mode=live|idle`, `scale`, `w`/`h`,
  `post=full|min`, `overlay=1`, `autostart=0`. Frame loop: `requestAnimationFrame`; live = 1 ms ticks,
  catch-up capped at 50 ticks per frame (S1), skip a draw while the GPU is busy; replay = fixed ticks per
  rendered frame. Controls: file picker + drag-and-drop (one `File` path), table selector, Start button;
  touch: left/right half = flippers, bottom-centre zone = launch; keys Shift L/R, Space, Enter.
  Errors are shown in a banner (bundle rejected, rules tilt, engine crash/restart).
* Server: `node dist/serve.cjs [port] [--coop] [--no-csp]` (127.0.0.1; CSP on every response;
  `application/wasm`; ETag; no COOP/COEP unless `--coop`).

## 3. Req 1 + 2 (f) — the build and the flag/feature combination

Build: `./build_web.sh` = `cargo build --release --target wasm32-unknown-unknown -p pin_web`
(features `webgpu,webgl,harness`) → `wasm-bindgen --target web` → `wasm-opt -Oz …` → `wasm-tools
validate` → `dist/`. Times on this VM: cargo from scratch 4 min 41 s (`logs/release1.log`), incremental
3–6 s; wasm-bindgen 2 s; native wasm-opt 24–87 s (S3's npm/JS binaryen took ~9 min per module). The
build is reproducible: the final run built `dist` three times (before/after the variant builds and after
`conflicts.sh` rebuilt crates with other features) with the same `pin_web_bg.wasm` sha256
`88e06b166af4781b…` (`logs/int_build.log`, `logs/int_build_restore.log`).

Every flag/feature/env and why: **BOM.md §3**. Module facts: `build_info()` → `simd128: true`,
`panic: abort`, `exception-handling` target feature not set for Rust (`results/build_info.json`);
initial memory 92 pages (5.75 MiB = 4 MiB stack + 1.76 MiB data); `__stack_pointer` 4 194 304; 69 912
SIMD (`v128`/`f32x4`/…) instructions; validates without relaxed-SIMD and without exnref-only opcodes.

**EH encoding: legacy** (`try`/`catch`/`catch_all`/`rethrow`; 0 `try_table`, 0 `throw_ref`), from the
Lua C (`-mllvm -wasm-use-legacy-eh=true`) and wasm-bindgen 0.2.130's per-import/per-export wrappers
(SP-restoring, rethrow JS exceptions). Browser floor (S5 `RESEARCH.md` §6): legacy EH Chrome 95 /
Firefox 100 / Safari 15.2, SIMD128 91 / 89 / 16.4, `wasm-unsafe-eval` 97 / 102 / 16.0; the test
Chromium 157 meets all of them. exnref works too in Chromium (S2) but needs Safari 18.4 (medium
confidence, S5), so stay on legacy.

Conflicts found while combining (each reproduced by `scripts/conflicts.sh`, `results/conflicts/*.log`):

| # | combination | failure (verbatim) | minimal fix (applied) |
|---|---|---|---|
| C1 | two spike crates with `#[global_allocator]` (S1 `CountingAlloc`, S4 `PeakAlloc`) + the engine's | `error: the `#[global_allocator]` in this crate conflicts with global allocator in: s1_harness` (and `s4_rules`) | only the engine crate defines the allocator (here `s4_rules::peak::PeakAlloc` over pin_sim's `CountingAlloc`: D5 guard + heap peak); libraries never do |
| C2 | two crates exporting the same wasm-bindgen names (`play`, `bench`, `alloc_check`, `install_panic_hook`) | **cargo links fine** (rlibs are archives: the first definition wins silently); wasm-bindgen: `error: the name `play` is exported by multiple crates in this build; rename one side with `js_name`/`js_namespace`` | library crates carry no `#[wasm_bindgen]` exports; the engine crate owns the JS API |
| C3 | a second `#[wasm_bindgen(start)]` (S3's `web_probe`) | wasm-bindgen: `the name `start` is exported by multiple crates in this build` | as C2 |
| C4 | Lua C without the wasi-libc sysroot | `KHZ_WASI_SYSROOT must name a wasi-libc sysroot to build Lua for wasm32-unknown-unknown` | `KHZ_WASI_SYSROOT=<wasi-sdk>/share/wasi-sysroot` |
| C5 | system clang-18 as `CC_wasm32_unknown_unknown` | `clang (LLVM option parsing): Unknown command line argument '-wasm-use-legacy-eh=true'` (build fails loudly; S2 route a showed LLVM 18 *without* that option links silently with `env.saveSetjmp`/`testSetjmp`/`getTempRet0` imports — the import audit is the guard) | wasi-sdk 32 clang (LLVM 22) |
| C6 | pin_script default features (`native-backends` = wasmtime + notify) unified into the web graph | `error: could not compile `errno`` ("target OS is unknown", via rustix ← wasmtime) | `pin_script = { default-features = false, features = ["lua", "web-parity"] }`, pin_sim without `native-backends` |
| C7 | `CARGO_PROFILE_RELEASE_LTO=fat` on a `[cdylib, rlib]` crate | none — **silently not applied**: the rustc command for pin_web has no `-C lto` (`results/conflicts/c7_nolto.log`); `cargo rustc --crate-type cdylib` adds `-C lto=fat` (`logs/lto_probe.log`) | ship without LTO, declared explicitly (`lto = false`): fat LTO was larger after `-Oz` (9 391 508 vs 8 181 742 B; gzip 3 065 493 vs 2 912 358 B) and equal in speed (Node parity wall times, `results/parity_node_lto.txt`); parity identical |
| C8 | wgpu callback with `Rc<Cell<_>>` while `webgl` is compiled in and `fragile-send-sync-non-atomic-wasm` is off (S3's recommendation) | `error[E0277]: Rc<std::cell::Cell<bool>> cannot be sent between threads safely` at `queue.on_submitted_work_done` (observed during development) | `Arc<AtomicBool>` (atomics work single-threaded) |
| C9 | `u32` tick count from JS (`engine.step(n)`) with a negative `n` | page froze in `webengine_step` (CDP pause stacks, `logs/t_hang.*.log`): rAF's timestamp preceded the loop's `performance.now()`, the debt went negative, wasm-bindgen's `>>> 0` made it 4 294 967 29x ticks | clamp the debt at ≥ 0 in JS; `WebEngine::step` refuses `n > 10 000` |
| C10 | `initSync`/`new WebAssembly.Module` for the engine module on the main thread | `RangeError: WebAssembly.Compile is disallowed on the main thread, if the buffer size is larger than 8MB` | async `init()` (streaming) in the page and for re-instantiation; `initSync` only in Node/workers |
| C11 | Playwright `page.waitForFunction` / string `page.evaluate` under the CSP | never resolve (Playwright compiles the predicate with `new Function` inside the page) | poll with function `page.evaluate` (`node/pw.cjs` `poll`) |

No conflict appeared for: `+simd128` with the Lua C objects (no `+exception-handling` rustflag needed;
wasm-ld accepts the mixed target-feature sections), EH vs wasm-bindgen's wrappers (wasm-bindgen 0.2.130
handles EH modules), the wasmi/wasmparser/wasm-encoder feature sets (wasmparser 0.228 for wasmi and
0.261 for S4 coexist), `getrandom` (zip enables `wasm_js`), missing libc symbols (none: no `env`
import), and the allocator pair: wasi-libc's `malloc`/`dlmalloc`/`sbrk` **are linked** (5 KiB) but
only reachable through mlua's fallback `luaL_newstate` (C `l_alloc`), taken only if `lua_newstate` with
mlua's Rust allocator returns NULL (`mlua-0.12.2/src/state/raw.rs:143-148`); both allocators grow
memory with `memory.grow`, so they cannot overlap — but that fallback would run Lua outside mlua's
memory cap (native has the same fallback with glibc): queue line S9-Q2.

## 4. Req 3 (d) — import audit (`./check_imports.sh`)

`WebAssembly.Module.imports` and `wasm-tools print` of `dist/pkg/pin_web_bg.wasm`: **623 imports, all
from module `./pin_web_bg.js` with names `__wbg_*` / `__wbindgen_*`** (621 functions = console,
performance, the WebGPU/WebGL2/DOM calls through the glue, and 2 tags `__wbindgen_jstag`,
`__wbindgen_wrapped_jstag`); **0 `env.*`, 0 `wasi_*`**; the raw cargo output imports only
`__wbindgen_placeholder__`/`__wbindgen_externref_xform__` (wasm-bindgen's own). `check_imports.sh`
exits 1 on any other import. Exports (72, `results/exports.txt`): `memory`, wasm-bindgen's
`__abort_handler`/`__instance_terminated` (EH mode), the `WebEngine`/`WebGpu` methods, `create_gpu`,
`build_info`, `heap_*`, `webgpu_layout`, 13 `probe_*` (harness), closure trampolines, `__wbindgen_*` allocator/table
helpers, and 14 `khz_*` (KHZ 32's Rust-libm functions for Lua, `#[no_mangle]`; visible, harmless).

## 5. Req 4 (a) — replay with rendering

Method: `S9.runReplay` loads the table's bundle file (`fetch` → `engine.load_replay(bundle, scn, scr,
refs)` → `Vfs::from_zip_bytes` → `Simulation::load_from_vfs_with_script`), builds the renderer from the
same VFS, then every animation frame steps N ticks and renders the frame (canvas visible, presented);
every tick is compared in-module with the native reference (Lua: S1 native `state_hash`+`ext_hash`, plus
S2's checker on every script step against S1's `.scr`; rules.wasm: S4's native wasmtime engine-path
`state_hash`+`ext_hash`+command-queue hash). WebGPU and WebGL2, 360×640, `post=min`; tables in
Appendix A (generated by `scripts/summarize.py` from `results/replay_*.json`).

Result (final `run_integration.sh`, `logs/int_replay_{webgpu,webgl2}.log`, Appendix A): **WebGPU 7/7
PASS, WebGL2 6/6 PASS**: apex/sky/feature bot60 × {Lua, wasm} on both backends — 60 001 ticks compared
each, 0 state/ext/cmd mismatches, Lua stream 60 001 steps with 0 mismatches, final state hashes
`2b7b1eec564b8cdc` / `1f0509f5802dc2e5` / `2040cea9ef77a2e0` (= S1/S4 native); **10-minute soak
(apex_circuit_bot600, rules.wasm, WebGPU) 600 001 ticks, 0 mismatches, final `a4b944ad641338d1`**.
300 rendered frames × 200 ticks per 60 s scenario, 600 frames × 1000 ticks for the soak (fixed ticks per
frame, DECISIONS S9-2); 13.6–17.2 rendered frames/s on WebGPU and 19.6–22.4 on WebGL2 (SwiftShader, final run;
7.9–16.0 in the previous run under heavier load); median `render()` call 0.7–1.0 ms (encode + submit on the main thread); 0 GPU errors, 0 console errors,
0 page errors; no dropped or diverging tick. The Lua run's per-tick command-queue hash equals the
wasm-rules run's (identical `stream` digests over state+ext+cmd per tick), i.e. rules.lua ≡ rules.wasm end
to end in the combined module too. Node (`results/parity_node_dist.txt`): 20/20 PASS (3 tables ×
{bot60, pat60, mb8} + soak, × {Lua, wasm}) on the shipped `-Oz` module; the same 20/20 on the
un-optimised wasm-bindgen output of an earlier build (`results/parity_node_bindgen_noopt.txt`) and on the
fat-LTO probe (`results/parity_node_lto.txt`); wasmi's per-callback fuel digests equal S4's
wasmtime-exact digests (`f17cda6a414ff89c` apex bot60, `243f70e84680632c` soak).

## 6. Req 5 (b) — player bundles (`node/t_player.cjs`, `results/player_webgpu.json`)

One `File` path for both inputs: `<input type=file>` (`page.setInputFiles`) and a real drop
(`DataTransfer` + `File` built in the page, `dragover` + `drop` dispatched on the canvas). The page
checks `File.size` ≤ 32 MiB before any copy, then `File.arrayBuffer()` → `Uint8Array` →
`engine.load_bundle(bytes)` (one copy into linear memory, moved into the loader as `Vec<u8>`).

* Player bundle = feature_lab with a modified `rules.lua` (sets score 900 009 every tick) and a renamed
  table (`dist/test/player/feature_lab_player.pinbundle`): loads through the picker, plays (ticks
  advance, score 900 009 on the score display, rules not faulted).
* Hostile bundles from S4's corpus are rejected with **exactly the native error text** (S4's
  `out/zip/native.web.txt`): picker — `path escapes the bundle: "../evil.txt"`, `encrypted archive entry
  "table.toml"`, `invalid archive: "table.toml": unsupported compression method Unsupported(12)`; drop —
  `file "zeros.bin" is too large (52428800 bytes)`, `path escapes the bundle: "a/../../evil.txt"`,
  `invalid archive: end of central directory: no end record whose comment ends the archive`. After each,
  the previously loaded table keeps playing; a 73 MB file is refused from `File.size` with linear memory
  unchanged (13 959 168 B before and after); then sky_deck (dropped) and feature_lab_wasm (picker) load
  and play: the engine stays usable. The whole 33-file corpus through the same loader in the combined
  module equals S4's native classification (Node, §11).
* Memory (`results/player_webgpu.json`, Appendix A): linear memory 6.09 MB before the first bundle, 13.1 MB
  after apex_circuit + its renderer, 14.0 MB after the player bundle, unchanged by every rejection,
  **peak 15 663 104 B (14.9 MiB)** after 11 loads/rejections (the wasm-rules table added 1.7 MB);
  Rust heap current 3.9 MB, peak 7.2 MB (`heap_stats`). `File.arrayBuffer()` 0.5–8.3 ms, `load_bundle`
  of a 1.9 MB production bundle 27–114 ms (zip, validation, physics build, rules `on_init`).

## 7. Req 6 (c) — both backends (`node/t_backends.cjs`, `results/backends.json`, `shots/`)

Launch flags (S3): `--enable-unsafe-webgpu --enable-features=Vulkan --use-angle=vulkan
--ignore-gpu-blocklist`, `VK_ICD_FILENAMES`/`VK_DRIVER_FILES` = the headless shell's
`vk_swiftshader_icd.json` (no GPU, no X, no system Vulkan driver). WebGPU adapter (JS
`requestAdapter().info`): vendor `google`, architecture `swiftshader`, `isFallbackAdapter: true`;
wgpu: `BrowserWebGpu`, canvas formats `Rgba8Unorm, Bgra8Unorm, Rgba16Float` (uses `Rgba8Unorm`).
WebGL2: `ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver)`,
WebGL 2.0, `EXT_color_buffer_float` yes, `MAX_SAMPLES` 4; wgpu `Gl`, storage buffers 0 (S3's
`downlevel_webgl2_defaults` path). Same simulated tick (10 000, S1 bot60 replay, parity checked) on
both backends at 540×960 with the full post chain: non-blank (2 812–4 652 distinct colours, 84–85 %
non-background), **WebGPU vs WebGL2 mean abs diff ≤ 0.0001, max 26 on ≤ 4 pixels** (S3 measured the
same); egui overlay painted on both; 0 uncaptured/deferred GPU errors, 0 console errors, 0 page errors.
Selection: `?backend=auto` → WebGPU when an adapter exists; → WebGL2 when `navigator.gpu` is absent
(init script removes it) and when it exists but `requestAdapter()` returns null (Chromium without
`--enable-unsafe-webgpu`). Screenshots: `shots/frame_{webgpu,webgl2}_<table>_tick10000.png`,
`shots/overlay_egui_{webgpu,webgl2}.png`, `shots/page_*_ui.png`, `shots/page_auto_fallback_webgl2_ui.png`.

## 8. Req 7 (e) — size and load time

Sizes (`scripts/size_report.sh`, `results/sizes.md`/`sizes.json`; brotli quality 11, lgwin 24 via
Node zlib as S3; every module through the same native `wasm-opt -Oz`):

| module | raw | gzip -9 | brotli -11 |
|---|---|---|---|
| **S9 shipped** (`dist`, both backends + egui + harness), after `-Oz` | 11 140 164 B (10.62 MiB) | 3.93 MiB | **2.79 MiB** |
| S9 cargo output / after wasm-bindgen | 18.43 / 14.64 MiB | 4.95 / 4.40 | 3.48 / 2.95 |
| S9 engine only (no harness), both backends | 10.50 MiB | 3.88 | 2.76 |
| S9 engine only, **WebGPU-only artifact** | 8.28 MiB | 3.01 | **2.16** |
| S9 engine only, WebGL2-only artifact | 10.39 MiB | 3.84 | 2.74 |
| JS glue (shipped / WebGPU-only / WebGL2-only) | 186 905 / 108 750 / 103 567 B | 0.02 / 0.02 / 0.01 | 0.02 / 0.01 / 0.01 |
| S9 fat-LTO probe (no egui), after `-Oz` | 8.96 MiB | 2.94 | 2.01 |
| S3 render path, both, opt z (`-Oz`) | 8.00 MiB | 2.72 | 1.95 |
| S3 render path, WebGPU-only, opt z | 6.36 MiB | 2.06 | 1.46 |
| S4 rules harness (wasmi + instrumentation + sim) | 5.23 MiB | 1.71 | 1.15 |
| S2 Lua harness (sim + Lua, umath) | 2.64 MiB | 0.98 | 0.71 |
| S1 sim harness (+simd128) | 2.17 MiB | 0.80 | 0.57 |

Where the code is (name section of the cargo output, `results/sizes/attrib_groups_dist.md`; 11 049 KiB
of code, 1.81 MB data): rules engine (wasmi 663 KiB + wasmparser 0.228/0.261 961 KiB + wasm-encoder)
**18.2 %**; WebGL2 path (naga 858, wgpu-core 651, wgpu-hal, glow) **15.8 %**; Rust core/alloc/std/
hashbrown **13.6 %**; egui text stack (harfrust, skrifa, read_fonts, epaint, vello…) **12.7 %**;
toml/serde/serde_json **7.8 %**; WAT parser (wast) **6.7 %**; sim (pin_* + parry3d/glam/libm) 5.7 %;
assets 4.2 %; egui + egui-wgpu 3.4 %; WebGPU front + bindings 3.4 %; spike harness 2.7 %; Lua C +
wasi-libc 2.1 %; mlua 0.7 %; renderer (pin_render + port) 1.3 %. **Top 5 contributors**: wasmparser,
core, naga, wast, wasmi. Growth over S3's render path: the rules engine and WAT parser (S4's design A
inside the module), the sim, Lua, toml (bundle manifests), at the same egui/wgpu cost; harness code is
only 0.03 MiB brotli. Budget check (S3 proposed ≤ 3 MiB brotli for the whole WebGPU module): **met**
(2.16 MiB WebGPU-only engine; 2.79 MiB for this all-in-one spike module). Savings available: drop the
WAT parser on the web (−0.74 MiB code, S9-Q1), one wasmparser version (wasmi's 0.228 vs S4's 0.261).

Load to first frame (`node/t_loadtime.cjs`, `results/loadtime_{webgpu,webgl2}.json`; median of 5;
cold = a fresh persistent profile per run, warm = new navigations in a primed persistent profile (HTTP
disk cache + V8's wasm code cache; the in-memory cache of an incognito context did not keep the 11 MB
module); 540×960, full post chain; ms since navigation start):

| mark | WebGPU cold | WebGPU warm | WebGL2 cold | WebGL2 warm |
|---|---|---|---|---|
| module JS starts | 35 | 19 | 32 | 9 |
| wasm ready (fetch + streaming compile + instantiate) | 136 | 86 | 134 | 76 |
| GPU ready (adapter, device, surface) | 161 | 100 | 171 | 94 |
| bundle loaded (zip + validation + physics + Lua `on_init`) | 351 | 122 | 375 | 127 |
| renderer built (visuals + pipelines) | 424 | 144 | 655 | 206 |
| **first frame presented** | **565** | **153** | **865** | **215** |
| wasm bytes transferred | 11140464 | 0 (cache) | 11140464 | 0 (cache) |

Machine-load caveat: other spikes shared the 4 cores (load average 1.1–2.2 during these runs, up to 12
earlier: a single probe run at load 11 gave a 2.2 s cold first frame); SwiftShader renders on the same
CPUs; local server (no network latency). The numbers show the order and cost of the steps and that
nothing blocks; they do not predict phone or real-GPU timing.

## 9. Req 8 (g) — mobile smoke (`node/t_mobile.cjs`, `results/mobile.json`, `shots/mobile_*.png`)

**Chromium with Playwright device descriptors (viewport, DPR, touch, UA) — not Safari/iOS WebKit, not a
phone GPU: layout and input mapping only.** Pixel 7 (412×839, DPR 2.625) and iPhone 14 (390×664,
DPR 3), portrait and landscape: the canvas fills the viewport, controls and the launch zone are inside
it, no scrolling, `touch-action: none`, `maxTouchPoints` 1; a frame renders (WebGPU via `auto`, drawing
buffer = CSS px × min(DPR, 2) × 0.5); CDP `Input.dispatchTouchEvent` (real touchstart/touchend → pointer
events, `pointerType: touch`) on the left half / right half / bottom centre gives `left`/`right`/`launch`
press+release in the engine; held for 400 simulated ms, the left touch raises flipper 0 only (0 → 0.7854
rad), the right touch flipper 1 only (0 → −0.7854), both return to 0; the live session keeps running
with 8 live input events; 0 console/page errors.

Layout finding (fixed): S3's probe PiP layout (score display as a bottom band over the full width,
backglass top-left) covered the **flippers** in portrait and most of the screen in landscape
(`shots/` of the first run: the score display took 194 of 360 px in Pixel 7 landscape). `pin_web`'s
`page_routes` now places the PiPs by aspect: portrait — score band at the top (below the 52 CSS px
control bar, 9 % of the height) and a small backglass under it; landscape (w ≥ 1.2 h) — backglass and
score display in the side columns left free by the aspect-fitted table. The test asserts the PiP
rectangles (`WebGpu::layout()`) stay above 60 % of the height (portrait) or outside the table column
(landscape). This is a 9.7 UI decision; the measurement here only shows a usable default exists. Playwright Firefox/WebKit: **not available** here
(`/opt/pw-browsers` has Chromium only, `~/.cache/ms-playwright` absent). WebKit/Firefox facts remain
S5's (`S5/RESEARCH.md` §1, §6, F1: iOS/iPadOS 26 for WebGPU, legacy EH Safari 15.2, SIMD 16.4,
`wasm-unsafe-eval` 16.0; Android Chrome 121+ for WebGPU).

## 10. Req 9 — failure and recovery (`node/t_failures.cjs`, `results/failures_webgpu.json`)

* (i) Lua: S2's four fault injections appended to feature_lab's `rules.lua` (player bundles) and
  replayed against S2's native fault-injection hashes while rendering: runtime error at tick 20 000
  (`rules.lua:504: injected fault`), budget kill (infinite loop: `script instruction budget exceeded`),
  memory bomb (charged to the budget first — native too, S2 case `mem_big_strings_fast`), error in the
  50th `on_switch` — each: 1 fault, rules tilt, the sim runs to tick 60 000, **every tick equals S2's
  native run**, the page shows "Rules stopped (tilt): <error> - the table keeps running without its
  rules."
* (ii) rules.wasm fuel kill: a player bundle whose `rules.wasm` loops forever from tick 3 000 loads
  through wasmi + instrumentation, faults with `script instruction budget exceeded` at the wasmtime-exact
  budget, keeps the score its `on_init` set (777), the sim keeps running, the banner shows the error.
* (iii) engine trap: `S9.armTrap(300)` → a Rust panic inside `step` → `RuntimeError: unreachable`
  (panic message via the hook: `pin_web panic: … S9 injected engine trap at tick N`); a later call into
  the old instance → `Error: Module terminated`; the page shows "Engine crashed …", imports a fresh glue
  module (`pin_web.js?instance=2`), creates a new canvas + GPU context, reloads the current bundle bytes
  and continues ("Engine restarted after a crash (instance 2)"). Finding: promise callbacks the dead
  instance registered earlier (wgpu `onSubmittedWorkDone`, device-lost, uncaptured-error) still fire
  after the trap and throw `Module terminated` from wasm-bindgen's closure trampolines (an uncaught page
  error in the first full run, `logs/run1/int_failures.log`); the page now swallows and counts exactly
  that error after a crash (`state.crash.lateCallbacks`: 1 in one run, 0 in another — timing-dependent) — the product shell needs the
  same handler.
* (iv) stack (4 MiB shadow stack, stack-first, `sp~0x3fff9b`): high-water marks with Lua and with wasmi
  on the 9-ball sky_deck_mb8 replay with the full post chain: `load_replay` 293 395 B, `step` 53 419 B,
  `gpu_build` 44 663 B, `gpu_render` 5 495 B (max 7 % of 4 MiB); S2's deepest Lua C recursion
  (`cstack_gsub`) 289 830 B; S4's depth-counter limits 2338/711/229/78/15 with no `RangeError`; S4 F-A2:
  a 100 000-fuel back-edge loop ends as a budget fault (auto-dispatch kept); S1 A28: 3 500 × 1 KiB
  frames fit; S2: SP restored after 20 × (4 faults + 2 000 pcall'd errors) (delta 0). Parity of both
  stack-measured replays held.

## 11. S1/S2/S4 harness checks inside the combined module (Node, `node/probes_node.mjs`)

All PASS (`results/probes_node.json`): S2 sandbox 79 cases = S2's recommended wasm build line for line
(vs native only `mem_gradual_keep`, the documented pointer-size case, with `mem=` ignored); S2
budget-edge = native; S2 SP after 50 × (4 faults + 2 000 pcall errors): delta 0; S4 adversarial corpus
82/82 = S4's wasmi-instr-depth (Node) and = native wasmtime running the instrumented module; S4 stack
probe = S4; S4 zip corpus 33/33 = native classification (web limits); S1 glam ops: every op = native
x86_64 SSE2 except `vec4_min_max` (S1's documented ±0 tie rule, unused by the sim); D5 guard with the
wasmi backend: 0 allocations in `step_1ms` over 60 000 ticks × 3 tables. Harnesses that do **not**
apply unchanged: S1's/S2's/S4's own drivers call their crates' exports (`play`, `lua_play`, …), which
the combined module deliberately does not export (C2); their checks run through `parity_node.mjs` /
`probes_node.mjs` instead (same references, same comparison rules).

## 12. Recommendations

1. **Build recipe (Q-build, 9.8):** exactly BOM.md §3/§4: rustc 1.97.0, `+simd128`, 4 MiB stack,
   wasi-sdk 32 for Lua C (sha256-pinned), wasm-bindgen = Cargo.lock's (0.2.130), `--target web`,
   opt-level 3, cgu 1, **no LTO (declared)**, native binaryen 132 `wasm-opt -Oz` with the feature list,
   `wasm-tools validate`, `check_imports.sh` as a failing gate. Legacy EH, not exnref (Safari floor).
2. **Artifacts (Q3 + size):** two engine modules from the same crate: **WebGPU-only (2.16 MiB brotli)
   primary**, **WebGL2-only (2.74 MiB brotli) fallback**; the JS loader requests a WebGPU adapter first
   and fetches the fallback only when `requestAdapter()` is null or `navigator.gpu` is absent (the
   both-backends module works too, 2.76 MiB brotli, and is what `auto` used here). Always async
   `init()` (8 MB sync-compile limit).
3. **Crate structure (9.5/9.7):** one engine crate owns `#[global_allocator]`, `#[wasm_bindgen(start)]`
   and every export; libraries (pin_sim, pin_script, pin_render, the rules host) export nothing; the
   artifact crate is `crate-type = ["cdylib"]`; pin_script `default-features = false, features =
   ["lua", "web-parity"]`; wasmi with `auto-dispatch` + `deterministic`; no `fragile-send-sync`
   (callbacks use `Arc` + atomics).
4. **Runtime design: keep S1/S2/S4's as integrated here**: sim on the main thread in the rAF loop,
   Lua in the engine module, wasmi + instrumentation for every rules module (curated and player),
   bundles from `File`/`fetch` through `Vfs::from_zip_bytes` with web limits and a 32 MiB `File.size`
   check, re-instantiate the module after any trap, frame pacing on `on_submitted_work_done`; **no
   COOP/COEP** (not needed); S4's CSP.
5. **Web CI gates (9.8)** = this spike's scripts: import audit; Node per-tick parity 10 scenarios ×
   {Lua, wasm} on the final `-Oz` module; S2 sandbox + S4 adversarial + zip corpus + D5 in the module;
   Playwright: replay-while-rendering on WebGPU and WebGL2, backends + fallback, player bundles,
   failures/recovery, size budget (≤ 2.5 MiB brotli WebGPU artifact, ≤ 3 MiB WebGL2).
6. **Size work (optional, 9.8):** drop WAT text support on the web (S9-Q1), unify wasmparser versions.

## 13. Residual risks (not verified here)

* Safari/iOS WebKit and Firefox: not installed; EH/SIMD/WebGPU/CSP availability from S5 only; JSC and
  SpiderMonkey float/NaN behaviour and stack limits untested (S2/S4 residuals).
* Real GPUs and phones: everything rendered on SwiftShader; phone touch latency, multi-touch (2
  flippers + plunger need ≥ 3 points; emulation reports 1), memory per tab and thermal behaviour
  unmeasured (S5 F1).
* GitHub-hosted runners: the Chromium/SwiftShader flags are proven locally only (S3 A23).
* aarch64 browsers: S7 showed the S1 `+simd128` sim module is bit-identical under emulated aarch64
  (`S7/RESULTS.md` C2); the combined module was not run there.
* mlua's `luaL_newstate` fallback to the C heap (S9-Q2); a Rust panic inside a Lua callback still
  aborts the instance (S2 G18) — handled by re-instantiation, verified here.
* Timing numbers are from a loaded 4-core VM with software rendering: they show relative cost and that
  nothing blocks, not phone performance.

## 14. Effort for the follow-up implementation of what this spike validated

| work | runs | tier |
|---|---|---|
| `pin_web` engine crate + JS shell (frame loop, pacing, bundle picker/drop, error banner, re-instantiation, touch/keys) on top of 9.1/9.2/9.5 | 3 (2 implementer, 1 Independent Tester) + 1 fix | T2 |
| Web build script + artifacts (WebGPU / WebGL2) + import audit + size budget in CI | 1 | T1 |
| Web CI jobs from `run_integration.sh` (Node parity/probes; Playwright replay/backends/player/failures/mobile) | 2 | T1–T2 |
| Queue items S9-Q1 (WAT off on web), S9-Q2 (mlua fallback), S9-Q3 (API types) | 1–2 | T2 |

≈ 8–9 agent runs on top of the per-component work S1–S4 already estimated.

## 15. Commands index

```sh
cd S && . ./env.sh
./run_integration.sh            # full: builds dist + 3 engine variants, all checks, PASS/FAIL table (~1 h on this VM)
./run_integration.sh nobuild quick
./build_web.sh                  # dist/ (features webgpu,webgl,harness)
./build_web.sh --no-default-features --features webgpu --out $PWD/out/v_engine_webgpu
./check_imports.sh [module.wasm]
scripts/conflicts.sh [quick]    # C1-C7 reproductions
scripts/size_report.sh          # sizes + attribution
node node/parity_node.mjs dist all lua,wasm      # 20 lines PASS
node node/probes_node.mjs dist results/probes_node.json
node node/t_replay.cjs webgpu 360 640 min 200 results/replay_webgpu.json apex_circuit:lua sky_deck:lua feature_lab:lua apex_circuit:wasm sky_deck:wasm feature_lab:wasm apex_circuit:wasm:apex_circuit_bot600:1000
node node/t_backends.cjs results/backends.json 540 960
node node/t_player.cjs webgpu results/player_webgpu.json
node node/t_failures.cjs webgpu results/failures_webgpu.json
node node/t_mobile.cjs results/mobile.json
node node/t_loadtime.cjs webgpu results/loadtime_webgpu.json 5
node node/t_synccompile.cjs results/sync_compile.json
python3 -I scripts/summarize.py S
node dist/serve.cjs 8790        # then open http://127.0.0.1:8790/?backend=auto (Chromium flags in BOM.md)
```

Tolerances for re-runs: every hash, mismatch count, import/export count and error text must match
exactly; the wasm sha256 is reproducible with the same toolchain; screenshot statistics within a few %;
sizes within ±1 %; timings vary with load (±50 %).

## 16. Lint and native suites

* `rustfmt --check` (edition 2024) on every `pin_web` source: clean. The copied spike crates keep their
  own formatting (S2's `s2_harness` has pre-existing rustfmt differences in files this spike did not
  write; the lines S9 added are formatted).
* `cargo clippy --target wasm32-unknown-unknown -p pin_web --no-deps -- -D warnings`: clean with the
  default features and with `--no-default-features --features webgpu` (`logs/clippy_pin_web*.log`).
  Without `--no-deps`, `-D warnings` fails in `pin_render` on S3's 7 known wasm32 dead-code warnings
  (presenter-thread/blit code unused on the web; S3 §1.2) — not S9 code.
* Native suites: no product crate behaviour was changed by S9 (only feature gates in the copied spike
  crates and the new `pin_web`); the product-crate patches are S1/S2/S3/S4's and were tested there.

## 17. Test assertions corrected during the spike (all in this spike's own new tests)

| test | first assertion | corrected to | evidence |
|---|---|---|---|
| `t_player` drop of sky_deck | `info.files === 56` | `=== 49` (+ `rules === 'lua'`) | `python3 -c zipfile…`: apex 56, sky 49, feature 55 files |
| `t_failures` (i)/(ii) texts | `BudgetExceeded` / `MemoryLimit` (Debug names) | the Display texts, `script instruction budget exceeded` for budget and for S2's memory bomb | S2 `runs/sandbox/umath.native.txt` `mem_big_strings_fast first_msg="BudgetExceeded"`; per-tick parity with S2's native injected run |
| `t_failures` (iii) | no page errors at all | page errors may only be `Module terminated` from the dead instance's late callbacks; the page now swallows and counts them (`lateCallbacks`) | `logs/run1/int_failures.log` |
| `t_mobile` flippers | wall-clock holds (400 ms), "any flipper moved" | holds and returns measured in simulated ticks; left touch must raise flipper 0 only, right touch flipper 1 only | `logs/t_mobile_pips.log` (wall-clock version passed by accident: the left flipper was still returning) |
| `t_mobile` layout | element bounds only | + PiP rectangles off the flipper area | screenshot of S3's layout in Pixel 7 landscape (this file §9) |

## Appendix A — final `run_integration.sh` (started Fri Nov 20 23:45:16 UTC 2026, finished Sat Nov 21 00:07:20 UTC 2026; `logs/run_integration_final.log`)

Shipped module sha256 `88e06b166af4781bacd199d136ea8059077fdd191ddadd023b07b1e1ae16e95b`.

| item | what | result | evidence |
|---|---|---|---|
| a    | replay S1 recordings while rendering, per-tick parity      | PASS | webgpu 7 PASS/0 FAIL; webgl2 6 PASS; node 20/20 PASS |
| b    | player bundles: input + drop, hostile rejected as native   | PASS | 21 PASS / 0 FAIL (logs/int_player.log) |
| c    | WebGPU + WebGL2 frames, auto selection, no GPU errors      | PASS | 33 PASS / 0 FAIL; shots/frame_*.png |
| d    | import audit: glue only, 0 env.*, 0 wasi_*                 | PASS | IMPORT AUDIT: PASS (${WEB_SPIKES}/S9/dist/pkg/pin_web_bg.wasm) |
| e    | sizes + load-to-first-frame (numbers)                      | PASS | results/sizes.md, results/loadtime_*.json |
| f    | flag/feature combination builds; conflicts reproduced      | PASS | == conflicts: 0 unexpected outcome(s); build_info PASS |
| g    | mobile emulation: layout, touch->inputs, frame             | PASS | 28 PASS / 0 FAIL |
| 9    | failure/recovery: Lua error+budget, fuel kill, trap, stack | PASS | 31 PASS / 0 FAIL |
| h    | S1/S2/S4 harness checks in the combined module (Node)      | PASS | == probes_node: 0 failing check(s) |


### replay_webgpu.json (Chromium 157.0.8103.78, flags --enable-unsafe-webgpu --enable-features=Vulkan --use-angle=vulkan --ignore-gpu-blocklist, adapter BrowserWebGpu (WebGPU, SwiftShader), [360, 640])

| scenario | rules | ticks | compared | state/ext/cmd bad | Lua stream steps / mismatches | final state_hash | frames | ticks/frame | fps | render ms (median) | GPU errors | non-bg / colours |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| apex_circuit_bot60 | lua | 60000 | 60001 | 0/0/0 | 60001 / 0 | 2b7b1eec564b8cdc | 300 | 200–200 | 15.8 | 0.8 | 0 | 0.8669 / 1755 |
| sky_deck_bot60 | lua | 60000 | 60001 | 0/0/0 | 60001 / 0 | 1f0509f5802dc2e5 | 300 | 200–200 | 14.6 | 0.8 | 0 | 0.8718 / 2374 |
| feature_lab_bot60 | lua | 60000 | 60001 | 0/0/0 | 60001 / 0 | 2040cea9ef77a2e0 | 300 | 200–200 | 16.8 | 0.8 | 0 | 0.867 / 1468 |
| apex_circuit_bot60 | wasm | 60000 | 60001 | 0/0/0 | – | 2b7b1eec564b8cdc | 300 | 200–200 | 16.2 | 0.8 | 0 | 0.8669 / 1755 |
| sky_deck_bot60 | wasm | 60000 | 60001 | 0/0/0 | – | 1f0509f5802dc2e5 | 300 | 200–200 | 16.0 | 0.7 | 0 | 0.8718 / 2374 |
| feature_lab_bot60 | wasm | 60000 | 60001 | 0/0/0 | – | 2040cea9ef77a2e0 | 300 | 200–200 | 17.2 | 0.7 | 0 | 0.867 / 1468 |
| apex_circuit_bot600 | wasm | 600000 | 600001 | 0/0/0 | – | a4b944ad641338d1 | 600 | 1000–1000 | 13.6 | 0.8 | 0 | 0.8669 / 1752 |

console errors 0, page errors 0, crashed False, fails 0

### replay_webgl2.json (Chromium 157.0.8103.78, flags --enable-unsafe-webgpu --enable-features=Vulkan --use-angle=vulkan --ignore-gpu-blocklist, adapter Gl ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver), [360, 640])

| scenario | rules | ticks | compared | state/ext/cmd bad | Lua stream steps / mismatches | final state_hash | frames | ticks/frame | fps | render ms (median) | GPU errors | non-bg / colours |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| apex_circuit_bot60 | lua | 60000 | 60001 | 0/0/0 | 60001 / 0 | 2b7b1eec564b8cdc | 300 | 200–200 | 20.4 | 1.0 | 0 | 0.8669 / 1756 |
| sky_deck_bot60 | lua | 60000 | 60001 | 0/0/0 | 60001 / 0 | 1f0509f5802dc2e5 | 300 | 200–200 | 19.6 | 1.0 | 0 | 0.8718 / 2374 |
| feature_lab_bot60 | lua | 60000 | 60001 | 0/0/0 | 60001 / 0 | 2040cea9ef77a2e0 | 300 | 200–200 | 21.3 | 0.9 | 0 | 0.867 / 1470 |
| apex_circuit_bot60 | wasm | 60000 | 60001 | 0/0/0 | – | 2b7b1eec564b8cdc | 300 | 200–200 | 20.4 | 1.0 | 0 | 0.8669 / 1756 |
| sky_deck_bot60 | wasm | 60000 | 60001 | 0/0/0 | – | 1f0509f5802dc2e5 | 300 | 200–200 | 20.4 | 0.9 | 0 | 0.8718 / 2374 |
| feature_lab_bot60 | wasm | 60000 | 60001 | 0/0/0 | – | 2040cea9ef77a2e0 | 300 | 200–200 | 22.4 | 0.9 | 0 | 0.867 / 1470 |

console errors 0, page errors 0, crashed False, fails 0

### loadtime_webgpu.json (median of 5; load average during runs 1.1–1.8 on 4 cores)

| mark (ms since navigation start) | cold | warm |
|---|---|---|
| js-start | 34.8 | 18.8 |
| wasm-ready | 136 | 86.4 |
| gpu-ready | 160.9 | 99.5 |
| bundle-fetched | 160.9 | 99.7 |
| bundle-loaded | 351.2 | 122.4 |
| renderer-built | 424.3 | 144 |
| first-frame-submitted | 515.6 | 149.1 |
| first-frame-presented | 564.8 | 153.2 |
| wasm_fetch_ms | 57.7 | 23 |
| wasm_transfer_bytes | 11140464 | 0 |
| bundle_fetch_ms | 11.2 | 5.5 |
| bundle_transfer_bytes | 1976613 | 0 |

### loadtime_webgl2.json (median of 5; load average during runs 1.7–2.2 on 4 cores)

| mark (ms since navigation start) | cold | warm |
|---|---|---|
| js-start | 32.5 | 8.7 |
| wasm-ready | 134.2 | 75.5 |
| gpu-ready | 171 | 94.1 |
| bundle-fetched | 179.2 | 102.4 |
| bundle-loaded | 375.2 | 127 |
| renderer-built | 654.8 | 206 |
| first-frame-submitted | 762.1 | 211.5 |
| first-frame-presented | 864.8 | 215.2 |
| wasm_fetch_ms | 51.2 | 22.7 |
| wasm_transfer_bytes | 11140464 | 0 |
| bundle_fetch_ms | 10.4 | 4.6 |
| bundle_transfer_bytes | 1976613 | 0 |

### backends.json

| frame | tick | state_hash | distinct colours | non-background | mean luma | render ms |
|---|---|---|---|---|---|---|
| webgpu/apex_circuit | 10000 | 18eed44519f45d90 | 3697 | 0.8421 | 105.15 | 1.1 |
| webgpu/sky_deck | 10000 | 60a015f8a11332d3 | 4652 | 0.8487 | 105.19 | 0.9 |
| webgpu/feature_lab | 10000 | e1e9414cd0617bcf | 2812 | 0.8422 | 99.02 | 0.9 |
| webgpu/egui_overlay | – | – | 1955 | 0.8636 | 45.16 | – |
| webgl2/apex_circuit | 10000 | 18eed44519f45d90 | 3698 | 0.8421 | 105.15 | 1.8 |
| webgl2/sky_deck | 10000 | 60a015f8a11332d3 | 4651 | 0.8487 | 105.19 | 1.4 |
| webgl2/feature_lab | 10000 | e1e9414cd0617bcf | 2812 | 0.8422 | 99.02 | 1.3 |
| webgl2/egui_overlay | – | – | 1864 | 0.8636 | 45.06 | – |

| table | WebGPU vs WebGL2: mean abs | max abs | % px > 8 | % px > 32 | 8x8-block max |
|---|---|---|---|---|---|
| apex_circuit | 0.0001 | 26 | 0 | 0 | 0.41 |
| sky_deck | 0.0001 | 19 | 0.001 | 0 | 0.28 |
| feature_lab | 0 | 1 | 0 | 0 | 0.02 |

explicit_webgpu: gpuInfo backend=BrowserWebGpu adapter='' format=Rgba8Unorm surface_formats=['Rgba8Unorm', 'Bgra8Unorm', 'Rgba16Float']; navigator.gpu adapter.info={'vendor': 'google', 'architecture': 'swiftshader', 'device': '', 'description': '', 'isFallbackAdapter': True, 'subgroupMinSize': 4}

explicit_webgl2: gpuInfo backend=Gl adapter='ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver)' format=Rgba8Unorm surface_formats=['Rgba8UnormSrgb', 'Rgba8Unorm', 'Rgba16Float']; navigator.gpu adapter.info={'vendor': 'google', 'architecture': 'swiftshader', 'device': '', 'description': '', 'isFallbackAdapter': True, 'subgroupMinSize': 4}
auto_with_webgpu: backend=BrowserWebGpu adapter=''
auto_without_navigator_gpu: backend=Gl adapter='ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver)'
auto_without_adapter: backend=Gl adapter='ANGLE (Google, Vulkan 1.3.0 (SwiftShader Device (Subzero) (0x0000C0DE)), SwiftShader driver)'

### player_webgpu.json loads

| # | name | via | ok | error / rules | read ms | load ms | linear memory before → after (B) |
|---|---|---|---|---|---|---|---|
| 0 | apex_circuit | builtin | True |  | None | None | 6094848 → 13107200 |
| 1 | feature_lab_player.pinbundle | input | True |  | 3.8 | 40.7 | 13107200 → 13959168 |
| 2 | 20_traversal_dotdot.zip | input | False | path escapes the bundle: "../evil.txt" | 3.2 | None | 13959168 → 13959168 |
| 3 | 35_encrypted_flag.zip | input | False | encrypted archive entry "table.toml" | 1.3 | None | 13959168 → 13959168 |
| 4 | 38_unsupported_method_bzip2.zip | input | False | invalid archive: "table.toml": unsupported compression method Unsupported(12) | 0.9 | None | 13959168 → 13959168 |
| 5 | 10_bomb_ratio_single_50MiB.zip | drop | False | file "zeros.bin" is too large (52428800 bytes) | 5 | None | 13959168 → 13959168 |
| 6 | 21_traversal_nested_dotdot.zip | drop | False | path escapes the bundle: "a/../../evil.txt" | 0.5 | None | 13959168 → 13959168 |
| 7 | 42_truncated_central_directory.zip | drop | False | invalid archive: end of central directory: no end record whose comment ends the archive | 6.8 | None | 13959168 → 13959168 |
| 8 | 13_bomb_70MiB_ratio_ok_web_cap.zip | input | False | bundle file is too large (73406762 bytes; the web limit is 33554432) | None | None | None → 13959168 |
| 9 | sky_deck.pinbundle | drop | True |  | 8.3 | 26.9 | 13959168 → 13959168 |
| 10 | feature_lab_wasm.pinbundle | input | True |  | 3.7 | 113.9 | 13959168 → 15663104 |

final: {'linear_memory': 15663104, 'heap': {'current': 3947067, 'peak': 7161457}}

### failures_webgpu.json stack

{"gpu_build": 44663, "gpu_render": 5495, "load_replay": 293395, "step": 53419} [['cstack_gsub', 289830], ['cstack_pcall_nest', 48511], ['cstack_pcall_nest_err', 48511]] sp~0x3fff9b (4095 KiB) __data_end=0x5ba339 __heap_base=0x5ba340 layout=stack-first (stack below data: overflow goes below 0 and traps)

