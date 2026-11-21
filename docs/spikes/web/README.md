# Browser-version research spikes (archive)

## (a) What this is

Research spikes for a browser (WebAssembly + WebGPU/WebGL2) version of Kilohertz Pinball, run on
2026-11-20/21 against `main` at `995935f91ad8ad18fcee201b94df52c5aaf1fb6c`. This folder is a curated,
source-only archive of their reusable outputs: reports, patches, scripts, harnesses, probes and the
delegation briefs.

* Nothing here is built, tested or linted by CI (it lives under `docs/`, not in a Cargo workspace or
  `.github/`), and none of it is to be merged as code.
* Implementation tasks re-create the changes through the normal process (work-classifier, branch, PR,
  CodeRabbit) and use these as a starting point. Patches apply to `995935f`; re-base them on current `main`.
* Each spike folder has `RESULTS.md` (the report), `DECISIONS_LOCAL.md` (choices the spike made on its own),
  `QUEUE_LINES.md` (follow-ups it recorded) and, where it had them, `patches/`, gate scripts and harness sources.
* Paths inside the copied text: the original scratch root is written `${WEB_SPIKES}` (the folder holding
  `S1`...`S10`) and its parent `${SCRATCH}`. In `RESULTS.md`, `S` means `${WEB_SPIKES}/<Sn>`.
* S9 (integration of everything into one wasm module) is archived in `S9/`.

## (b) Spikes

### S1 — sim determinism on wasm32 (`S1/`)

| | |
|---|---|
| Goal | Is `pin_sim` bit-identical native vs wasm32 (wasip1, wasm-bindgen/Node, Chromium)? |
| Headline | Yes, with `-C target-feature=+simd128` and no sim source change: every tick's `state_hash` and `ext_hash` identical for 3 tables x {bot60, pat60, mb8}, a 600 000-tick soak and the CI fixture (`25097c21c2c3adc1`); 40/40 PASS. Without simd128 the first rotating ball diverges by 1 ULP in the orientation quaternion (glam scalar vs SSE2 association). Wasm tick cost 1.1-1.6x native; 0 allocations in `step_1ms`. |
| Reusable | `0001` pin_script native-backends feature gating; `0002` pin_sim `ExternalScript` hook; `0003` pin_sim `apply_game_op`; `0004` `s1_harness` crate (record/playback/hash/web driver; the crate is carried by this patch); `0005` option-a glam `scalar-math` (rejected, kept for reference). `FINAL_HASHES.md`: final `state_hash` per scenario. `web/` minimal page. |
| Gate scripts | `run_parity.sh`, `env.sh`, `harness/` (`run_tests.sh`, `run_bindgen.mjs`, `compare.py`, `bench_*.sh`, `build_size_variants.sh`, `chromium.cjs`, `serve.cjs`) |

### S2 — real Lua 5.4.9 + KHZ patches on wasm (`S2/`)

| | |
|---|---|
| Goal | Run the vendored Lua with all KHZ patches inside the engine's `wasm32-unknown-unknown` module; native = web parity and sandbox behaviour. |
| Headline | Works with the unchanged pin_script host code (wasi-sdk 32 clang + wasi-libc, setjmp/longjmp as wasm EH). Command-stream and per-tick hash parity with S1's native recordings in Node and Chromium. Needs KHZ 29-32; 78/79 sandbox cases identical (the 79th is pointer-size dependent, fixed later by S8). Lost on web: Rust panic inside a Lua callback aborts the instance. Real-Lua `on_tick` 4.2-5.6 us in Chromium. |
| Reusable | `0001` lua-src web arm + KHZ 29-32 (wasm build arm, correctly rounded hex numerals / `%a`, fixed `LUAL_BUFFERSIZE`, Rust `libm` math); `0002` pin_script `lua` feature, libm ABI, `web-parity` (drops `collectgarbage`); `0003` `s2_harness` + `s2_lua_module` crates (carried by this patch, not duplicated under `crates/`). `cases/inject/` fault-injection scripts, `c/` C probes (numtest, libc_cmp, sjlj, clang18), `web/` EH test modules. |
| Gate scripts | `run_lua_parity.sh`, `run_lua_sandbox.sh`, `env.sh`, `wasm_env.sh`, `harness/` (`build.sh`, `build_exnref.sh`, `build_umath.sh`, `run_wasm_suites.sh`, `stack_need.cjs`, `bench.sh`, ...) |

### S3 — wgpu on WebGPU / WebGL2 (`S3/`)

| | |
|---|---|
| Goal | Does `pin_render` (wgpu 30, egui-wgpu 0.37) build and render on the web on both backends; cost of WebGL2. |
| Headline | Confirmed (A1, A3-A5, A22-A24) with corrections: egui-wgpu's default features pull in `webgl`; WebGL2 has 4 blockers in `pin_render` (worst: fragment storage buffers in `pbr_forward`, prototyped in about 90 lines) and "WebGL2 is not huge effort" (`effort_webgl2.md`). Render path 5.3-8.0 MiB after `wasm-opt -Oz`, 1.46-1.95 MiB brotli. Headless Chromium (SwiftShader) gives both backends. |
| Reusable | `patches/pin_render.diff` (WebGL2/wasm changes to `pin_render` sources), `patches/pin_render-Cargo.toml.diff`, `patches/workspace-Cargo.toml.diff` (diffs, not git patches: paths are `repo/crates/pin_render/...`); `ci/web-render-smoke.yml` (proposed workflow, not applied; needs a CI trial); `scripts/` (size attribution, tables); `node/` render probe driver; `web/` probe page sources. |
| Gate scripts | `build_wasm.sh`, `run_render_probe.sh`, `size_build.sh`, `size_matrix.sh`, `size_opt.sh`, `env.sh`, `scripts/` |

### S4 — rules.wasm sandbox via wasmi, fuel/depth instrumentation, zip/VFS hardening (`S4/`)

| | |
|---|---|
| Goal | Run `rules.wasm` (player-supplied) in the browser with native-identical fuel and stack behaviour; harden the bundle zip/VFS loader. |
| Headline | Design A (wasmi 2.0.2 inside the engine wasm, instrumented module) gives exact native/Node/Chromium parity incl. per-callback fuel identical to wasmtime 50.0.1, via a load-time rewrite plus a weighted call-depth counter (no web engine reproduces wasmtime's stack depth otherwise). NaN canonicalisation needed for user modules. 82/82 adversarial lines identical. Zip corpus (33 hostile zips): all classified identically native/Node/Chromium; check `File.size` in JS before copying. |
| Reusable | `crates/s4_rules/` (Cargo.toml, `src/`, `tests/`; **`src/instrument.rs`** = fuel/depth rewriter, **`src/adversarial.rs`** = adversarial corpus, backends `wasmi_backend.rs`, `wasmtime_instr.rs`, `js_backend.rs`); `patches/0001` read-only pin_script probes; `harness/make_zip_corpus.py` generator; `zipcorpus/CORPUS_LISTING.md` (name, size, sha256, intended outcome); `wat/hang.wat`; `web/`, `node/` drivers. |
| Gate scripts | `run_rules_parity.sh`, `run_rules_adversarial.sh`, `env.sh`, `harness/` (`final_runs.sh`, `bench_rules.sh`, `run_wasip1_tests.sh`, `*_table.py`) |

### S5 — browser platform facts (`S5/`)

| | |
|---|---|
| Goal | Sourced browser-version facts: WebGPU/WebGL2 status and share, wasm feature support, input, file picker, audio, storage. |
| Headline | `RESEARCH.md` only (WebFetch/WebSearch, retrieved 2026-11-20, each row with source URL, date and confidence). Example: WebGPU stable in Chrome 113+, Firefox 141 (Windows), Safari 26. |
| Reusable | `RESEARCH.md` |
| Gate scripts | none |

### S6 — audio check from the issue-writing step (`S6/`)

| | |
|---|---|
| Goal | The browser-version issue's own check: does `pin_audio` work on the web. |
| Headline | `pin_audio` builds for wasm32 unchanged (`pin_audio_wasm32_build.log`). `pin_audio::Mixer` behind a 3-function C ABI (+simd128, 32 KiB, no imports) gives output digest `a08428a4c4bc78aa` in a Chromium 157 AudioWorklet (OfflineAudioContext, 48 kHz, 600 quanta), identical to native x86_64. |
| Reusable | `S6/audio_probe/` (Cargo.toml, Cargo.lock, `src/lib.rs`, `src/main.rs`, `web/` page and `processor.js` AudioWorklet, `run.cjs`, `serve.cjs`, `run_chromium.log`). The issue links this directory. |
| Gate scripts | `audio_probe/run.cjs` |

### S7 — aarch64 parity (`S7/`)

| | |
|---|---|
| Goal | Does aarch64 (Apple Silicon, ARM phones) match x86_64, and what is the smallest change that makes it so. |
| Headline | aarch64 native differs from x86_64 by 1 ULP in the ball orientation quaternion (NEON horizontal add order, `normalize` by reciprocal multiply, fused cross product), not by Quat-multiply FMA; fixture ends `ebfb6f43c9a0b859` on aarch64. Wasm `+simd128` on V8 ARM64 (qemu) equals x86_64 on all 11 scenarios. Patch 0006 (portable `Quat` ops, no `cfg`) makes aarch64 == x86_64 and leaves x86_64 bit-identical. All aarch64 numbers are qemu emulation. |
| Reusable | `patches/0006-pin_physics-portable-quat-ops.patch`; experiments `exp-glam-scalar-math-aarch64.patch`, `exp-seam-test-portable-inputs.patch`, `harness-portable-probe.patch`; `mini/` standalone probe crate; `probes/` result tables; `scripts/` |
| Gate scripts | `env.sh`, `scripts/` (`flavour.sh`, `native_arm.sh`, `wasm_arm.sh`, `wasm_run.sh`, `lua_record_arm.sh`, `mini_table.py`, ...) |

### S8 — Lua ILP32/LP64 audit, KHZ 33-37, per-tick cap (`S8/`)

| | |
|---|---|
| Goal | Find every script-reachable platform dependency of Lua on 32-bit wasm vs 64-bit native; close memory-cap parity and address-keyed `pairs`; add a per-tick aggregate script budget. |
| Headline | Six dependencies found (string positions >= 2^32 wrap with an OOB read, GC schedule reaching the budget, cap point, address-indexed string cache, NaN sign x86 vs ARM, address-keyed `pairs`). Five knob-gated patches (KHZ 33-37) make native, Node, Chromium and ARM64 V8 identical on 13 probes, 79/79 sandbox cases and all 11 scenarios. Per-tick cap instrumentation included. |
| Reusable | `0001` lua-src KHZ 33-37 (logical LP64 allocation accounting, per-state creation id for hashing, string-position width, NaN sign, strcache); `0002` pin_script memcap/tick-cap instrumentation; `0003` `s2_harness` S8 tooling (adds `s8.rs`; apply on top of S2 `0003`); `0004` `s4_rules` per-tick-cap tooling (adds `s8adv.rs`; apply on top of the S4 crate). `probes/` Lua and C audit probes. |
| Gate scripts | `run_audit.sh`, `run_memcap_parity.sh`, `run_pairs_order.sh`, `run_tick_budget.sh`, `env.sh`, `wasm_env.sh`, `harness/` |

### S9 — one combined wasm32 engine module in Chromium (`S9/`)

| | |
|---|---|
| Goal | One `wasm32-unknown-unknown` module combining sim (`+simd128`), real Lua (KHZ 1-32), wasmi rules (S4 instrumented), the wgpu renderer (WebGPU and WebGL2) and the bundle loader, run in Chromium. |
| Headline | `run_integration.sh` ALL PASS: replay parity (every tick) while rendering on WebGPU and on WebGL2, including the 600 001-tick soak; player bundles via file picker and drop; 33/33 backends, 28/28 mobile emulation, 31/31 failure/recovery. Imports only wasm-bindgen glue; runs under CSP without COOP/COEP. Size 2.79 MiB brotli combined, 2.16 WebGPU-only, 2.74 WebGL2-only. Integration bugs found and fixed are listed in RESULTS section 0 (`scripts/conflicts.sh` reproduces them). |
| Reusable | `crates/pin_web/` (the web shell crate: Cargo.toml, `src/`); `BOM.md` (build flags, versions, size budget); `build_web.sh`, `check_imports.sh`; `web/` page; `node/` Playwright/Node test drivers; `scripts/` (size attribution, test data, summaries); `patches/` crate diffs (see below). |
| Gate scripts | `run_integration.sh`, `build_web.sh`, `check_imports.sh`, `env.sh`, `wasm_env.sh` |

`S9/patches/*.diff` are `diff -ruN` of the crates S9 changed in its repo copy (`pin_render`, `pin_script`, `pin_sim`) and the workspace `Cargo.toml` (`diff -u`) against the real tree at `main` `995935f`. They are cumulative (S1 patches 0001-0004, S2 0001-0003, S3 render changes plus an aspect-aware layout fix), not S9-only; `pin_render.diff` and the other three overlap the S1/S2/S3 patches. Paths in the `+++` lines are `b/<crate>/...`; apply with `patch -p1` inside `crates/`. Changes to `third_party/lua-src` (S2/S8 KHZ patches) and `Cargo.lock` are not saved here: use the S2 patches. S9's repo copy also contains `crates/web_probe` (S3's probe, not archived separately).

### S10 — JavaScriptCore / SpiderMonkey parity, KHZ 38, startup self-test (`S10/`)

| | |
|---|---|
| Goal | Do Safari/iOS (JSC) and Firefox (SpiderMonkey) engines reproduce the wasm results; what features and stack do the modules need. |
| Headline | Bit-for-bit parity on JSC 327409@main (all tiers) and SpiderMonkey 160.0.1 / 153.7.0esr (x86_64 and aarch64 under qemu): 2 383 PASS, 0 unexplained FAIL, 37.9 M per-tick hash comparisons identical to native. Float micro-test (115 ops) 0 value mismatches; NaN bits differ (JSC BBQ differs from its other tiers), so canonical NaN when hashing is mandatory. The one real difference is stack: JSC needs 1 312 KiB for 200 nested `pcall` vs 176 KiB on V8. Fix: KHZ 38 (`LUAI_MAXCCALLS` 200 -> 100) plus a startup self-test. Legacy EH recommended over exnref. |
| Reusable | KHZ 38 is a one-line `-D LUAI_MAXCCALLS=100` (see RESULTS section 8, `harness/build_ccalls_variant.sh`); `harness/selftest.js` startup self-test sketch; `micro/` wasm op micro-tests (`gen_micro.py`, `micro.wat`, `ref.c`, feature probes); `versions.md` (shell versions, URLs, checksums, tier flags); `mods.sha256`. No patches of its own. |
| Gate scripts | `run_engines.sh`, `harness/` (`job.sh`, `eng.sh`, `features.sh`, `cost.sh`, `summarize.py`, ...) |

## (c) Reuse map

Patches and sources likely to be applied or adapted, in dependency order:

| Item | Where | Use |
|---|---|---|
| S1 `0001`-`0003` | `S1/patches/` | native-backend feature gating of pin_script, `ExternalScript` hook, `apply_game_op`; prerequisite of S2/S4 |
| S1 `0004` | `S1/patches/` | `s1_harness` crate (tooling; recreate as test tooling if wanted) |
| S7 `0006` portable quat ops | `S7/patches/0006-...` | aarch64 == x86_64 for ball orientation; x86_64 unchanged; target-independent |
| S2 `0001`/`0002` | `S2/patches/` | KHZ 29-32 (wasm build arm, hex numerals/`%a`, `LUAL_BUFFERSIZE`, libm) and pin_script `lua`/`web-parity` feature |
| S8 `0001`/`0002` | `S8/patches/` | KHZ 33-37 plus memcap/tick-cap instrumentation (apply after S2) |
| KHZ 38 | S10 RESULTS section 8 | `LUAI_MAXCCALLS` 100 (one define) plus startup self-test |
| S3 pin_render diffs and CI | `S3/patches/`, `S3/ci/web-render-smoke.yml` | WebGL2/wasm changes to `pin_render`; proposed render smoke workflow |
| S4 `s4_rules` | `S4/crates/s4_rules/src/instrument.rs`, `adversarial.rs` | wasmtime-exact fuel and depth-counter rewriter; adversarial corpus for rules modules; plus the zip corpus generator |
| S5 | `S5/RESEARCH.md` | browser-support numbers for scope decisions |
| S9 `pin_web` | `S9/crates/pin_web/` | the web shell crate: sim + Lua + wasmi rules + renderer + bundle loader (owns the allocator and wasm-bindgen exports) |
| S9 build | `S9/build_web.sh`, `S9/check_imports.sh`, `S9/BOM.md` | one-script web build, import check (no `env`/`wasi_*`), the build-flag bill of materials |
| S9 gates | `S9/run_integration.sh`, `S9/node/`, `S9/scripts/` | basis for web CI gates (parity while rendering, backends, mobile, failure/recovery, size report) |

## (d) How to re-run

1. Copy this folder to a writable dir, e.g. `cp -r docs/spikes/web /work/web-spikes`, then
   `export WEB_SPIKES=/work/web-spikes` and `export SCRATCH=/work` (its parent).
2. Toolchain pins used: rustc 1.97.0 (targets `wasm32-unknown-unknown`, `wasm32-wasip1`);
   wasm-bindgen CLI and crate `=0.2.130` (must match exactly); wasm-tools 1.263.0; binaryen/wasm-opt 132
   (npm `binaryen@132.0.0`); wasi-sdk 32.0 (`wasi-sdk-32.0-x86_64-linux.tar.gz`, sha256
   `55fc523ebfbc98f69d1034fcfcb83d1ff5610cd9ab7eceef6cd097a30ba4ef93`, from S2 RESULTS section 2); Node 22 (v22.23.5);
   Playwright Chromium (157.0.8103.78, Playwright 1.63); S7/S8/S10 add qemu-aarch64 8.2.2 and the engine shells
   listed in `S10/versions.md`.
3. The scripts expect the spikes' original scratch layout: a copy of the repo tree at `<Sn>/repo` with the earlier
   spikes' patches applied (S1 `0001`-`0004`, then S2 `0001`-`0002`, ...), tools under `<Sn>/tools`, and recordings
   regenerated (see "Not archived"). `env.sh` hard-codes `CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_sN`:
   adjust it. They were run on Linux x86_64 (4 cores, 15 GB RAM) only.
4. Patches in `patches/` are `git apply`-able onto `995935f` unless noted (S3 files are plain `diff -ru` output; S8
   patches are `diff -ruN` relative to S2/S4 trees: use `patch -p1` from the matching tree).

## (e) Not archived

Excluded (binary, build output, or too large); regenerate with the commands in each `RESULTS.md`:

* Per-tick native hash files (`S1/native/*.hashes`, 40 MB) and recordings/bundles (`rec/`, `bundles/`): regenerate
  with the S1 record/playback commands in `S1/RESULTS.md` (`run_parity.sh`, `s1` harness); final hashes per
  scenario are in `S1/FINAL_HASHES.md`.
* All `*.wasm`, `pkg*/`, executables, `*.o`/`*.a`, `target/`, `dist/`, `bin/`; `S2/c/lmod.wat` (7.9 MB) and
  `S2/c/libc_cmp/{native,wasi}.txt` (1.5 MB each).
* Downloaded code and toolchains (`dl/`, `tools/`, `routes/`, wasi-sdk, engine shells, `node_modules/`) and
  `pristine/`, `repo*/` copies of the repo tree (only `S4/crates/s4_rules` is copied, as it is not in any patch).
* Logs and results: `logs/`, `out/`, `runs/`, `verify/`, `shots/`, `bench*/`, `S3/results`, S3 `body_part*`,
  `report_draft`, `*.orig`, `RESULTS.head.md`, `S10/micro/ref.bin`, `inputs.bin`.
* S9 `dist/`, `results/`, `shots/`, `out/`, `logs/`, `tools/`, `dl/`, `verify/`, `repo/` (except `pin_web` and the crate diffs): regenerate via `S9/run_integration.sh`.
* S4 zip corpus (33 zips, 77 MB, includes zip bombs): generator in `S4/harness/make_zip_corpus.py`, listing with
  sha256 in `S4/zipcorpus/CORPUS_LISTING.md`.
* `S6/audio_probe/web/s6_audio_probe.wasm` (binary): rebuilt with the crate.
* Empty files were not copied.

## (f) briefs/ and proposal-draft.md

* `briefs/` holds the delegation briefs given to each spike executor (`COMMON.txt` shared header, `S1`-`S6`, `S8`-`S10`;
  S7 has no brief file). They record the exact questions and assumptions
  (A-numbers) each spike answered.
* `proposal-draft.md` is the early draft of the web-version proposal (2026-11-20). It is superseded by the GitHub
  issue for the browser version; keep it only for the assumption list (A1...) that RESULTS files cite.
