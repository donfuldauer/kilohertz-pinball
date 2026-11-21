# Browser version: design detail

This is the design companion to the GitHub issue "Browser (WebAssembly) version of Kilohertz Pinball". The issue holds the decisions, the SPEC changes and the work-unit table. This document holds:
- the evidence per area
- the full deliverables and acceptance criteria of each work unit (W01-W29)
- the security matrix and threat-model text
- the CI design, toolchain pins, the build recipe and the build hosts (Linux x86_64, macOS 15+ on arm64 and Intel; S11)

`plan-from-spec` copies each W section into its work-unit issue. The issue's work-unit table links each row to its section here.

Section numbers such as "S7 §6" refer to the spike reports in this folder (`S1/RESULTS.md` … `S11/RESULTS.md`, `S5/RESEARCH.md`).

**About the spike code.**
- The spike code in this folder is a starting point. Each unit re-creates what it needs through the normal implementer, Independent Tester and CodeRabbit process (`docs/PROCESS.md` P5, P6); nothing is merged as is.
- The patches were made against `995935f`. S7 0006, S2 0001 and S1 0001-0003 still pass `git apply --check` on `main` at `ba45d5c`. S11 0001 was made later and applies on top of S2 0001; it does not apply to plain `main`.
- Since `995935f`, `main` changed `tables/feature_lab/table.toml`, tests and the vendored `sdl3-sys`. M1 ([#79](https://github.com/donfuldauer/kilohertz-pinball/issues/79)) will change gravity (KP-032), the flipper defaults (KP-007) and all three tables.
- Hash values quoted below are as of `995935f`. **Re-generate reference hashes on the current `main` before use.** The parity gates compare native and web builds of the same commit, so they do not depend on these values.

Every unit ends with: "The unit gate in `docs/PROCESS.md` P5 passes". Coverage floors are P5 unit gate item 4 (85 % for the core crates, 60 % for `pin_render`, `pin_audio`, `pin_app` and `pin_web`).

## Contents

- Architecture
- Sim on wasm32 and exact parity
- Lua in the browser
- Rules sandbox, player bundles and security
- Threat model additions
- Rendering
- Shell, input, audio and mobile
- Build, CI and local hosting
- Gate A4
- Work units: W01-W29, with labels and runs per unit
- Mapping from the draft's subphases, M1 and M2 dependencies, effort

## Architecture

| Area | Design |
|---|---|
| Crates | New `pin_web` (engine cdylib; it alone owns `#[global_allocator]`, the wasm-bindgen exports and `start`) and `pin_web_audio` (worklet module). `pin_script` gains features `lua`, `wasmtime` and `wasmi`; the web build uses `default-features = false, features = ["lua", "wasmi"]`. `pin_sim` gains `ExternalScript` and `load_from_vfs_with_script`. `pin_app` is not ported (S1 §3, S9 §3 C1-C6) |
| Threading | Single-threaded wasm on the main thread. The AudioWorklet runs the mixer. No SharedArrayBuffer (S1 §11, S9 S9-7) |
| Render | `pin_render` gains async init with native `pollster` wrappers, deferred error scopes and a canvas surface. WebGL2 adds a storage-buffer-free forward shader and WebGL2 limits. There are no ray queries on the web, so the D18 probe path applies |
| Input | `KeyboardEvent.code` maps to the HID usage, which equals the SDL scancode in the shipped profiles. Gamepad "standard" mapping. Pointer-event touch zones. All map onto the existing `CabinetInput` and `CabinetAxis` |
| Lua | Route (b) of S2 plus KHZ 29-38. Logical LP64 heap accounting. 100 nested C calls. `collectgarbage` removed |
| Rules | Pre-scan, then a rewrite with wasmtime-exact fuel, a weighted depth counter, memory and table caps and NaN canonicalisation, then wasmi (web) or wasmtime (native). A 200 000-unit per-tick budget |
| Assets | `File` or `fetch` bytes go to `Vfs::from_zip_bytes` with the web `ZipLimits`. No folder bundles |
| Determinism | Exact native = web = aarch64 (next section) |

## Sim on wasm32 and exact parity

### Evidence

- **x86_64 native = wasm32 with `+simd128` and no sim source change** (S1 §0, §5).
  - Tested on 3 tables × {bot60, pat60, mb8}, the 600 000-tick soak and the CI fixture (`25097c21c2c3adc1`).
  - Every tick's `state_hash` and `ext_hash` matched on wasip1, wasm-bindgen/Node and Chromium (40/40 PASS).
  - The final hash of each scenario is in [S1 FINAL_HASHES.md](S1/FINAL_HASHES.md).
  - Without `simd128` the build diverges at the first tick a ball spins, by 1 ULP in the orientation quaternion. Cause: glam 0.33.12 uses SSE2 on x86_64 and scalar code on wasm32, which associate differently (`(x+z)+(y+w)` vs `((x+y)+z)+w`) (S1 §6).
- **aarch64 native differs today on every scenario, in the orientation quaternion only** (S7 §3).
  - Positions, velocities, switches, scores and script inputs stay equal for all 600 000 ticks.
  - The repo's own macOS CI leg prints fixture `ebfb6f43c9a0b859`, which qemu reproduces bit for bit (S7 §4.5).
  - Causes (S7 §4.1-4.2): the NEON 4-lane sum `(x+y)+(z+w)`; `normalize` by reciprocal multiply instead of division; a fused cross product in `Quat * Vec3`.
  - The cause is not the FMA in `Quat::mul`, which multiplies by ±1 and is exact.
- **S7 patch 0006 makes aarch64 = x86_64 and leaves x86_64 bit-identical.**
  - `pin_physics::quat_ops` is plain `f32` code in SSE2 order, used at the 11 `Quat` sites on every target, with no `cfg`.
  - Results:
    - aarch64 qemu: 11/11 scenarios equal per tick, fixture `25097c21c2c3adc1`
    - x86_64: all 11 reference files reproduced, 5 seam digests unchanged, 833 tests pass
    - the replica equals glam SSE2 on 1 000 000 of 1 000 000 inputs per op
  - With 0006, even the no-`simd128` wasm build matches, so `simd128` becomes a speed choice; it is kept (S7 §5).
- **Wasm on ARM equals x86_64.** V8 ARM64 under qemu matched in default tiering, Liftoff-only and TurboFan-only (33/33), and the JIT code contains no fused multiply-add (S7 §7). JSC (all five tiers) and SpiderMonkey 160 and 153esr (x86_64, and aarch64 under qemu) also match: 37.9 M per-tick comparisons identical to native (S10 §0, §4.2).
- **NaN bits are not portable** (S1 §1 A12; S7 §3, §7; S10 §5). The sim never hashed a NaN in any run, but one in hashed state would hash differently per platform. Measured differences:
  - `libm::sqrtf(-1)` gives `ffc00000` natively and `7fc00000` on wasm.
  - ARM's default NaN is positive.
  - JSC's BBQ tier propagates the second operand's payload where its other tiers propagate the first.
- **`f32::min`/`max` ±0 ties differ.** `max(+0,-0)` is `-0` on x86_64 and wasm but `+0` on aarch64 (`fmaxnm`). glam `Vec4::min/max` differ under `simd128`. No run reached such a tie (S1 §6.2, S7 §3).
- **Cost and allocation.** A wasm tick costs 1.1-1.6× native. `step_1ms` makes 0 heap allocations on wasm, so the D5 guard works unchanged. The shadow stack must be raised to 4 MiB; the measured peak is 293 KiB (S1 §8, §10; S9 §10).

### Native blast radius

The blast radius is very-high under P3: it changes simulation output on a supported platform, and it changes the sandbox and budgets.

| Where | Moves | Evidence |
|---|---|---|
| Linux and Windows x86_64 production replays, fixture, seam digests | **no** | S7 §5.3; S2 §5 and S8 §4.3 (Lua patches); S4 §4 (instrumented rules); S10 §8 (KHZ 38) |
| macOS arm64 replays | **yes**, to the x86_64 values: fixture `ebfb6f43c9a0b859` → `25097c21c2c3adc1`, seam digests `0xd79f654b4dd0d3b0, 0xcd0a64cdfdf642ba, 0x23d5ae21c39688ea, 0x67626b0765d4b20a, 0x6f1c95f661c0e1bd` → x86_64 EXPECTED. Old macOS recordings still load (same table hash) | S7 §5.2, §6 |
| Bundles and table hashes | no: byte-identical on x86_64 and aarch64, before and after | S7 §5.6 |
| Native user Lua scripts | only these edge cases: transcendental math gives the Rust `libm` bits (KHZ 32); `collectgarbage` is gone; subnormal hex numerals are correctly rounded (KHZ 30); NaN prints `nan` instead of `-nan` (KHZ 36); the heap cap trips at a slightly different allocation (KHZ 33, one tick earlier in the corpus); address-keyed `pairs` order becomes deterministic (KHZ 34); more than 100 nested C calls raise "C stack overflow" (KHZ 38); more than 200 000 units in one tick fault | S2 §0, S8 §4.4, §6.3, S10 §8 |
| Native user Wasm rules | deep recursion faults at the depth counter (2 338 frames at 0 live locals, not 5 453); more than 200 000 fuel in one tick faults | S4 §10.2, S8 §6 |

**Tests and fixtures affected.** Every other existing test passes unchanged: 471 `pin_script` tests in S2 and S8; 833 `pin_physics` tests in S7.
- `ball_seam_bitident`: `RECORDING_TARGET` widened to aarch64. `slope_scenario` and `spec_ball_common`'s pitched world build inputs with `quat_ops` and `libm` (S7 §5.2). x86_64 EXPECTED is unchanged.
- `lua_backend::sandbox_escapes_fail` and `lua_backend::emergency_gc_is_charged` use `collectgarbage` and are rewritten (S2 §12).
- The Wasm stack-depth tests are re-baselined to the depth counter (S4 §10.2).
- Six `cstack_*` sandbox cases change text with KHZ 38 (S10 §8).
- `pin_assets` `hardening_round2::props::zip64_mutations` is not 32-bit clean (S4 §8); W19 fixes it.
- The CI fixture value recorded in `docs/DECISIONS.md` (sections on the replay hash, for example "w8g-e — std transcendental ban scope" and the earlier gate entries) is unchanged on x86_64.

### D9 amendment text

This is S7 §6, extended by S10. It is proposed for SPEC §2.4 and the D9 row (a P8 `spec` change), together with PROCESS.md R3:

> **D9 — Determinism.** *Gate:* (1) the same binary on the same machine replays any recording to identical ball state, switch-hit counts and scores; (2) cross-platform equality: the `replay-hash` fixture `state_hash` (10 000 ticks) and the seam digests must equal the committed x86_64-linux goldens on aarch64 (qemu-aarch64 on every pull request touching the sim, Apple Silicon on every push to main), on Windows x86_64 (push to main) and on wasm32 under V8, JavaScriptCore and SpiderMonkey (web workflow). *Report only:* other cross-OS hashes (the soak `cross-os` job). *To stay equal:* no fast-math, no FMA contraction and no explicit fused or SIMD-width-dependent float ops in the sim crates (quaternions through `pin_physics::quat_ops`, `min`/`max` through `pin_core::fmath`), `libm` for transcendental functions (also in tests that build reference inputs and in Lua, KHZ 32), fixed iteration order, one sim-owned seeded PCG, NaNs hashed as one canonical pattern.

## Lua in the browser

### Route chosen (S2 §4)

**Route (b).** The vendored Lua C is compiled into the engine's own `wasm32-unknown-unknown` module:
- wasi-sdk 32's clang 22.1.0 and the wasi-libc sysroot
- `setjmp`/`longjmp` lowered to legacy Wasm exception handling (`-mllvm -wasm-enable-sjlj -mllvm -wasm-use-legacy-eh=true`, `-ffp-contract=off`)
- io, os, loadlib and dblib left out

mlua 0.12.2 and pin_script's host code are unchanged. The final module imports only wasm-bindgen glue (0 `env.*`, 0 `wasi_*`; S2 §8).

**Rejected routes:**

| Route | Why rejected |
|---|---|
| (a) Ubuntu clang-18 with a hand-written runtime | `malloc`/`free` on every protected call, outside the cap and the D5 guard |
| (c) Emscripten | wasm-bindgen cannot link, so no wgpu web backend |
| (d) Pure-Rust VMs | piccolo: `-7 // 2 = -3`, no `string.format`, no cap. hematita is GPL-3.0 |
| (f) A separate Lua instance | Works, but costs +1.0-1.9 µs per `on_tick` and no guarantee needs it |

Cost: real-Lua `on_tick` takes 4.2-5.6 µs in Chromium against 2.4-3.0 µs natively (S2 §9). JSC and SpiderMonkey run at 1.05-1.14× V8 (S10 §7).

### Patches KHZ 29-38

| KHZ | What | Why (evidence) | Unit |
|---|---|---|---|
| 29 | wasm32 build arm in `third_party/lua-src/src/lib.rs` and a `khz-wasm` shim of failing WASI stubs | `lua-src` refuses the target today (`lib.rs:137-192`) (S2 §4) | W08 |
| 30 | One correctly rounded hex-numeral reader, and glibc-style `%a` for subnormals, on every target | wasi-libc `strtod` is off by 1 ulp on 463 of 10 000 hex numerals. musl and glibc print subnormal `%a` differently on all 1 396 subnormals. glibc itself is wrong on 4 of 2 000 000 (S2 §6.2) | W09 |
| 31 | `LUAL_BUFFERSIZE` fixed at 1024 | It depends on pointer size, and KHZ 20 charges buffer growth: one script fitted 2091 iterations natively but 815 on wasm32 (S2 §6.3) | W08 |
| 32 | Lua transcendental math through the Rust `libm` crate on every target | glibc and musl differ on 0.001-17.9 % of inputs; with the patch, 0 of 2 200 000 differ (S2 §6.2) | W09 |
| 33 | Logical (LP64) heap accounting at every allocation site: GC debt, pacing, KHZ 19 and the 2 MiB cap use logical bytes. A real-byte backstop at 2× the cap (mlua limit 4 MiB) | The cap point and the GC schedule were pointer-size dependent (S8 F2, F3): `mem_gradual_keep` faulted at tick 1970 natively and 1995 on wasm. Now `ok*1969,M` everywhere (S8 §4) | W10 |
| 34 | `pairs` hashes tables, closures, userdata and threads by a per-state creation id, not by address | Address order changed between two native runs and on the web (S8 F4). The order digest is now `7efbe9fc17a38b51` on every run and engine. No measurable CPU cost (S8 §5) | W04 |
| 35 | String positions ≥ 2^32 saturate where `size_t` is 32-bit | wasm32 wrapped them, and `string.sub(s, 2^32)` read one byte **before** the string (an address byte): a security fix (S8 F1) | W10 |
| 36 | NaN is formatted without its sign | x86 prints `-nan`, ARM prints `nan` (S8 F6) | W09 |
| 37 | The `luaS_new` string cache is indexed by content hash, not address | Units and memory changed run to run natively (S8 F5) | W04 |
| 38 | `LUAI_MAXCCALLS` 200 → 100 | JSC's baseline frames need 1 312 KiB of stack for 200 nested `pcall`s, against 176 KiB in V8. At 100 the worst JSC tier needs 728 KiB, and a 1 MiB thread passes (S10 §6.2, §8) | W10 |

**Combined evidence.** With these patches and the pin_script changes (`collectgarbage` removed because its `"count"` is a pointer-size-dependent heap size, S2-4; `lua/libm_abi.rs`; the cap and backstop wiring), the audit evidence set was identical on native (two runs), Node, Chromium and V8 ARM64 (S8 §3.3), and on JSC and SpiderMonkey in every tier (S10 §4.2). That set is: 13 probes with 1 658 script-visible lines, 804 per-callback heap values, the 79-case sandbox corpus, budget-edge, and pairs order.

All 11 production scenarios stay bit-identical per step and per tick with all patches (S8 §4.3, S10 §8).

### Sandbox guarantees on the web (S2 §6.4, S8 §8, S10 §9)

| Guarantee | Web | Note |
|---|---|---|
| G1-G5 (library surface, text chunks only, read-only proxies, `__gc`/`__mode` refused, `pcall` re-raises memory errors) | preserved | io, os, package and debug are not even linked |
| G6 heap cap 2 MiB | **preserved, exact** with KHZ 33 | was "bound, point differs" (S2) |
| G7 budget, G8 cost model | preserved | KHZ 31 and KHZ 33 remove the pointer-size terms |
| G9-G11 (64 KiB strings, no `<close>`, no addresses) | preserved | KHZ 35 closes the wasm32 address-byte leak |
| G12 determinism | preserved for every key type | KHZ 34, KHZ 37 |
| G13-G14 (fault, tilt, unwinding) | preserved | SP delta 0 after 40 000 `pcall` errors (S2 §6.4) |
| G15 C-stack and parser limits | preserved with KHZ 38 and the startup self-test | without KHZ 38, JSC on a 1 MiB thread terminates the instance first (S10 §6.2) |
| G16, G17, G19 | preserved | Lua VM memory goes through mlua's counting allocator on both targets |
| **G18 a Rust panic in a callback becomes a Lua error** | **lost** (wasm32 is `panic=abort`) | Compensations: deny `unwrap`/`expect`/`panic` in `pin_script::lua`; the shell re-instantiates on `Module terminated` (verified S9 §10); accepted risk TM-2 |

## Rules sandbox, player bundles and security

### Rules design (S4 §2, §3, §10; S10 §4.3)

**Chosen: design A.**
- wasmi 2.0.2 is compiled into the engine module.
- It runs a module rewritten at load time by `instrument.rs` (wasmparser and wasm-encoder 0.261). The rewrite adds:
  - fuel charged exactly as wasmtime 50.0.1's Cranelift does (`default_operator_cost`, bulk operations by length, constants up to 128 charged up front)
  - a weighted call-depth counter: `16·(params+locals+max stack)+64` estimated bytes, limit 256 KiB
  - memory and table maxima capped at 2 MiB and 65 536 elements
  - NaN canonicalisation
- This applies to every rules module, included or player-supplied.
- **Native runs the same instrumented module under wasmtime**, with wasmtime's own fuel kept as a backstop.
- wasmi config:
  - features `std`, `validate`, `auto-dispatch`, `deterministic`
  - `CompilationMode::Eager`
  - `set_min_stack_height(256 KiB)`
  - its own fuel as a backstop at 4 × budget + 4 Mi units

**Results:**
- **Parity.** 3 tables × {bot60, pat60, mb8} plus the 10-minute soak: 161 PASS and 0 FAIL in per-tick `state_hash`, `ext_hash` and command-queue hash across native, Node and Chromium (S4 §4). The JSC and SpiderMonkey tiers also passed (S10 §4.2).
- **Fuel equals wasmtime's own counter.** The apex bot60 digest is `f17cda6a414ff89c`. Production callbacks peak at 4 324 fuel, 4.3 % of the budget.
- **Adversarial corpus.** 82/82 lines exact on every engine. The fuel boundaries are 9 091 / 893 / 892 iterations; the depth limits are 2 338 / 711 / 229 / 78 / 15 frames for 0 / 16 / 64 / 200 / 1 000 live locals (S4 §5, S10 §4.2).
- **`rules.wasm` ≡ `rules.lua` end to end.** The wasm-rules runs equal the native Lua recordings on every tick (S4 §4).
- **Cost.** About 4-6 µs per callback in wasmi-in-V8. JSC and SpiderMonkey are faster (0.53-0.62× V8). A hostile full-budget callback takes 11-37 ms in Chromium (S4 §6, S10 §7), so the shell's 8 ms frame guard bounds it.

**Rejected:**

| Design | Why rejected |
|---|---|
| B: browser JIT plus instrumentation | Exact, but trap kinds are parsed from engine texts that differ on every engine (S10 D1). V8's own stack limit changes run to run (13 950 / 20 898 frames in one process). JS glue and the browser's trap handler would join the boundary |
| C: hybrid | Kept only as the fallback if a phone shows A above 2 ms per frame |
| D: Worker with Atomics | `Atomics.wait` throws on the main thread, it needs isolation, and its watchdog is wall-clock |
| wasmtime or Pulley in the page | Cranelift does not build; Pulley's `Module::deserialize` is unsafe for untrusted input (S4 §7.2) |

### Sandbox guarantee matrix

This is the web build; native matches it after W10 and W15.

| Boundary | Guarantee | Web | Residual risk | Compensating control |
|---|---|---|---|---|
| Lua | G1-G17 and G19 | preserved; G6 exact with KHZ 33 | undiscovered ILP32 differences (U12) | S8 audit gates in CI |
| Lua | G18: a Rust panic in a callback becomes a Lua error | **lost** (`panic=abort`) | denial of service of the player's own game | no panicking paths (clippy deny); re-instantiate (S9 §10); TM-2 |
| Rules | G1 fuel per callback | preserved, wasmtime-exact | a rewriter bug | wasmi fuel backstop; differential proptest; TM-3 |
| Rules | G2 instantiation cost; G3 memory cap (`grow` returns −1); G4 tables ≤ 4 × 65 536 | preserved | none measured | adversarial corpus exact |
| Rules | G5 stack | preserved and deterministic (counter); natively the kill point moves from 5 453 to 2 338 frames | none | the counter trips before every engine's limit (S4 §5, S10 §6.2) |
| Rules | G6 NaN | preserved (wasmi `deterministic`) | none | 9 NaN cases exact |
| Rules | G7 proposals, G8 D25 compile limits, G9 imports (18 `pinball` functions), G10 exports, G11 argument validation, G12 tilt, G13 no reuse after a trap | preserved, shared code | none | corpus exact |
| Rules | G14 no host allocation per callback | preserved | none | 0 allocations over 60 000 ticks per table, with the value stack pre-sized (S4 §4) |
| Rules | wasmi memory safety | new exposure | a wasmi bug could corrupt sim state inside the page | browser wasm sandbox; local hosting; TM-4 |
| Both | per-callback budget | preserved | a hostile table slows its own game | **per-tick budget of 200 000 units** (S8 §6); 8 ms frame guard; TM-5 |
| Zip/VFS | path, size, ratio, entry, CRC, method and symlink rules | preserved: the 33-file hostile corpus is classified exactly as natively (33/33 Node, 66/66 Chromium picker and drop) | linear memory never shrinks | `File.size` ≤ 32 MiB checked before any copy; one copy into the loader; web `ZipLimits` (S4 §8, S9 §6) |
| Zip/VFS | folder containment, TM-1 | not applicable (no folder bundles) | none | none needed |
| Assets | glTF and Ogg decoders under `catch_unwind` | **lost** (`panic=abort`) | denial of service of the player's own game | re-instantiate; TM-2 |
| Loader | `HashMap` keys randomised | **weakened**: keys are fixed on wasm32 (S1 A6) | a crafted bundle could slow loading by hash flooding | the 10 000-entry and size limits; TM-6 |
| Page | browser isolation | Chrome and Firefox desktop are site-isolated; mobile is partial (S5 F6) | browser engine bugs | CSP; local only; nothing sensitive in the origin; TM-7 |

### Player bundles (Q10)

- **Selection.** The file picker (no `accept` attribute, because iOS ignores extensions, S5 F1.2) and drag-and-drop share one `File` path. The page checks the magic bytes.
- **Size check first.** `File.size` must be ≤ 32 MiB before any byte is copied. A 73 MB file is refused with linear memory unchanged (S9 §6).
- **Loading.** `File.arrayBuffer()` is copied once into a `Vec<u8>`, which the loader takes by value. `Vfs::from_zip_bytes` then applies `ZipLimits { max_total_uncompressed: 64 MiB, max_file: 32 MiB, max_ratio: 100, max_entries: 10 000 }` (S4 §8). That is 25× the production tables, and peak linear memory stays under 256 MiB.
- **Backends.** Lua and Wasm rules run under the same sandboxes as on desktop. The startup self-test refuses player bundles on an engine whose stack cannot hold Lua's deepest legal recursion (S10 §8).
- **Storage.** Nothing is stored.

## Threat model additions

W29 adds this text to `docs/THREAT_MODEL.md`. THREAT_MODEL is not a P8 file (R11):

```markdown
## Scope (addition)

Web build (local hosting only): bundles are the three included `.pinbundle` files fetched from the same
origin, or a file the player picks or drops. The page checks `File.size` <= 32 MiB before copying, then
opens the bytes with `Vfs::from_zip_bytes` and the web `ZipLimits` (64 MiB total, 32 MiB per file,
ratio 100, 10 000 entries). Player bundles are never stored.

Existing defences (additions):

- **Wasm rules sandbox (all platforms).** D25 pre-scan and proposal set; import allowlist (the 18
  `pinball` functions) and export checks; then a load-time rewrite that enforces the per-callback fuel
  budget with wasmtime's cost model, a weighted call-depth limit (256 KiB of estimated frame bytes), the
  2 MiB memory and 65 536-element table maxima, and canonical NaN results. Native runs the rewritten
  module under wasmtime, the web under wasmi; both keep their own fuel as a backstop. Any trap tilts the
  script. (`crates/pin_script/src/wasm/`.)
- **Per-tick script budget.** 200 000 units per tick, shared by the tick's switch-edge callbacks and
  `on_tick`, Lua and Wasm alike; exhaustion is a `BudgetExceeded` fault.
- **Lua.** Heap accounting in logical (LP64) sizes with the 2 MiB cap on every target and a real-byte
  backstop at 2x the cap (KHZ 33); no address-, pointer-size- or CPU-dependent behaviour visible to
  scripts (KHZ 1, 2, 5, 10, 30-37); KHZ 35 closes a one-byte out-of-bounds read of string headers on
  32-bit targets; at most 100 nested C calls (KHZ 38); `collectgarbage` removed; a failing
  `lua_newstate` is a load error, never a C-heap fallback.
- **Web page.** `Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval';
  worker-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`;
  bundle content has no network or DOM access; a startup self-test refuses player bundles when the
  engine's stack cannot hold Lua's deepest legal recursion; an 8 ms per-frame script-time guard.

TM-1 does not apply to the web build: there are no folder bundles and no shared filesystem.

### TM-2 — Rust panics abort the web engine instance
`wasm32-unknown-unknown` is `panic=abort`: a panic in a loader, a decoder or a Lua host callback (natively
turned into a Lua error) aborts the instance. Effect: the player's own game stops; the page re-instantiates
the module and reloads the bundle. **Status:** accepted. **Revisit:** a panic is reachable from an included
table, or stable Rust gains `panic=unwind` on wasm32.

### TM-3 — The Wasm-rules rewriter is part of the sandbox boundary
A rewriter bug could break the fuel budget or the depth limit. Backstops: the engine's own fuel at
4 x budget + 4 Mi units, differential tests against wasmtime. **Status:** accepted. **Revisit:** any
bump of wasmtime, wasmparser, wasm-encoder or wasmi.

### TM-4 — A wasmi memory-safety bug could corrupt sim state in the page
Guest memory is bounds-checked in software inside the engine module; an exploit stays inside the
browser's wasm sandbox but could alter the running game. **Status:** accepted (local hosting, nothing
sensitive in the origin). **Revisit:** a wasmi security advisory, or hosting beyond localhost.

### TM-5 — Hostile rules can slow their own game
A callback may spend its whole budget (11-37 ms in Chromium, more on phones). The per-tick budget and the
8 ms frame guard turn this into slow motion, never a hung tab. **Status:** accepted. **Revisit:** a
player-visible hang is reported.

### TM-6 — Hash maps are not flood-resistant on wasm32
`std` `RandomState` uses fixed keys on `wasm32-unknown-unknown`; crafted names can slow a bundle's
loading. Bounded by the entry and size limits. **Status:** accepted. **Revisit:** loading a 10 000-entry
bundle takes more than 5 s on the reference machine.

### TM-7 — Browser engine bugs
Contained by the browser's process sandbox (site isolation on desktop Chrome and Firefox; partial on
mobile). Player bundles run inside our own origin, which holds no credentials or user data.
**Status:** accepted. **Revisit:** hosting beyond localhost.
```

## Rendering

### Evidence (S3, S9)

**Engine frames render on both backends.**
- `pin_render` builds for wasm32 and renders the real production frame of all three tables, from the real zipped bundles, in Chromium 157 (SwiftShader). The frame includes PBR, MSAA 4×, bloom, motion blur, ACES, and the backglass and DMD picture-in-pictures.
- WebGPU against native lavapipe: mean absolute difference < 0.1/255; the differences sit only on triangle edges, the same class D30 handles natively (S3 §3.3).
- With the WebGL2 prototype, WebGL2 differs from WebGPU on ≤ 4 pixels per frame, max 26/255 (S3 §5.2, S9 §7).
- This evidence predates M1's 3D machine scene (residual U13).

**Source changes needed for WebGPU** (S3 §1.2):
1. `GpuContext::request_async`. `pollster::block_on` on the browser main thread **never returns** on WebGPU: it spins at 100 % CPU (S3 §3.4).
2. One deferred error-scope helper for the 4 `pollster` pops.
3. The presenter thread gated to native; it is the only `Send` site.

Readback uses `map_async` + `poll(Poll)` + a yield. `Instant` and `poll(Wait)` cannot be used on wasm32.

**Feature hygiene** (S3 §1.1, §7).
- egui-wgpu 0.37's default features switch on `wgpu/webgl`, `wgpu/default` and `fragile-send-sync-non-atomic-wasm`.
- Use egui-wgpu with `default-features = false`, and wgpu with `default-features = false, features = ["std", "wgsl"]` plus the native backends under `cfg(not(target_arch = "wasm32"))`. Resolver 3 keeps the native feature set unchanged (106 + 7 native GPU tests passed).
- In a both-backends build, `Instance::new` picks WebGPU whenever `navigator.gpu` exists, even with no adapter. Only `wgpu::util::new_instance_with_webgpu_detection` falls back to GL (S3 §3.2).
- Without fragile-send-sync, wgpu callbacks need `Send` data: use `Arc` and atomics (S9 C8).

**WebGL2 blockers, all found in real runs** (S3 §5.1):

| # | Blocker | Fix |
|---|---|---|
| W1 | Device limits: WebGL2 refuses 10 storage and compute limits | `downlevel_webgl2_defaults()` |
| W2 | `pbr_forward` reads two fragment storage buffers, so nothing in the table draws | a forward variant that tests every light, prototyped in about 90 lines; identical image with the scenes' 4 key lights |
| W3 | A 1-layer `texture_2d_array` placeholder | use 2 layers on GL |
| W4 | The probe cube-face texture | off the app path; fix only if probes are enabled |
| W6 | The canvas sRGB view | use the canvas's own format |

Ray queries are absent on both web backends, so the D18 probe path applies unchanged. (W1-W6 here are S3's blocker ids, not work-unit ids.)

**WebGL2 effort.** 3 T2 runs, about 7.5 % of the phase (S3 §5.3). The recurring cost is one more browser page per CI run (about 10 s) and a native GLSL ES 3.00 naga translation test in `core-tests`, which reports exactly the browser's blocker (`doesn't support Features(BUFFER_STORAGE …)`).

**Sizes** (S9 §8, after `wasm-opt -Oz`):

| Artifact | Raw | Brotli |
|---|---|---|
| WebGPU-only engine | 8.28 MiB | 2.16 MiB |
| WebGL2-only engine | 10.39 MiB | 2.74 MiB |

The WebGL2 path adds naga, wgpu-core, wgpu-hal and glow, 15.8 % of the code. The fallback is fetched only when needed.

**Load to first frame** (Chromium on SwiftShader, loaded VM, indicative): WebGPU cold 565 ms / warm 153 ms; WebGL2 cold 865 ms / warm 215 ms (S9 §8).

**Phones** (S3 §8, emulated). The relative pass costs order the quality ladder; M1's quality presets (REQ-VIS-005, KP-035) take their place in the product:
1. bloom (41-53 % of the frame; `max_levels` 4 instead of 6, a half-resolution prefilter, off at the lowest tier)
2. render scale (CSS size × min(DPR, 2) × 0.75, floor 0.5)
3. MSAA off
4. motion blur off
5. 60 Hz cap on phones

Clamp the drawing buffer to `max_texture_dimension_2d`. (The S3 note that `pin_render` has no shadow pass predates M1's KP-016.)

**Device floors** (S5 §1, F1; S10 §6.3).
- WebGPU artifact: Chrome and Edge 113, Firefox 141 (Windows) and 145-147 (macOS), Safari and iOS 26, Chrome 121 on Android 12+ with a Qualcomm, ARM or Intel GPU.
- WebGL2 artifact: what the wasm module needs (Safari 16.4, Firefox 100, Chrome 96), plus `EXT_color_buffer_float` for the `Rgba16Float` HDR target. Without it, the page refuses rather than shipping an HDR-less path (S3 §9).

## Shell, input, audio and mobile

### Evidence

**Frame loop** (S1 §8, §11; S9 §2).
- The sim runs on the main thread in a `requestAnimationFrame` batch. A 16-tick batch costs p99 0.15-0.28 ms with 1-3 balls and ≤ 2.2 ms in the 9-ball stress (Chromium, x86 VM). JSC and SpiderMonkey are 0.93-1.21× V8 (S10 §7).
- A timer-driven 1 kHz loop does not exist in a page: nested `setTimeout(0)` averages 4.07 ms.
- Hidden tabs pause rAF and throttle timers, including Workers (S5 F3), so the game pauses on `hidden`.
- A negative JS tick count reached a `u32` parameter as 2^32 − k and froze the page. The API takes `f64`, validates it, and refuses more than 10 000 ticks per call (S9 C9, S9-Q3).

**Engine crate shape** (S9 §3 C1-C3).
- Only the engine crate defines `#[global_allocator]`, `#[wasm_bindgen(start)]` and the exports; two allocators or duplicate export names fail the build or silently pick one.
- The module is 8.3-10.6 MiB raw, above Chromium's 8 MB main-thread synchronous compile limit, so the page uses the async `init()` only (S9 C10).

**Trap recovery** (S1 §7.2, S2 §7, S9 §10).
- A Rust panic traps. Every later call into that instance throws `Module terminated`, and every trap leaks shadow stack.
- The page re-instantiates through a fresh glue-module URL and reloads the bundle.
- Callbacks still pending from the dead instance throw `Module terminated` and must be swallowed (S9-Q7).

**Startup self-test** (S10 §8). It takes 0.3-0.5 s on the default tiers.
- (a) Replay the CI fixture and compare the final `state_hash`/`ext_hash` (`25097c21c2c3adc1`/`643e148850cf462b` at `995935f`).
- (b) Compare the 15 `glam_ops` digests.
- (c) Run Lua's deepest legal C recursion (`cstack_pcall_nest`) twice, in a separate Lua state.
- If (c) fails, player bundles are refused.
- A JIT-less JavaScriptCore runs the sim 41-52× slower than V8 (265-330 µs per tick). The self-test's timing detects it, and the page says "this browser runs WebAssembly without a JIT".

**Input** (S5 §5, A26, F1; S9 §9).
- `KeyboardEvent.code` (physical position) goes through a static code → HID usage table, which equals the SDL scancodes of the shipped profiles. For example: `ShiftLeft` 225, `ShiftRight` 229, `ControlLeft` 224, `Space` 44, `Enter` 40, `NumpadEnter` 88, `Digit1` 30, `KeyA` 4, `KeyT` 23 (S5 A26 table).
- Gamepads: `getGamepads()` is polled once per frame and needs `mapping === "standard"`; otherwise the menu offers manual mapping.
- Touch: Pointer Events, with `touch-action: none` on the canvas, `body` and `html`; `pointercancel` counts as a release.
- Under Chromium device emulation (Pixel 7, iPhone 14, portrait and landscape), the left half, right half and bottom centre map to flipper 0, flipper 1 and launch. Holding a touch for 400 simulated ms raises only the matching flipper (0 → 0.7854 rad).
- The single-canvas PiP layout must depend on aspect ratio: S3's layout covered the flippers on phones (S9 §9, S9-Q8). After M1 the PiP layout is an option (REQ-VIEW-002), so this applies when the player picks it.
- Not on the web:
  - pointer lock
  - `Keyboard.lock()` (Firefox, Safari)
  - iOS vibration
  - the keys the browser reserves: F11 and `Esc` in fullscreen (S5 §5, §8). These affect the native camera keys (REQ-CAM-002, `F1`-`F12`) and the menu key.

**Mobile platform facts** (S5 F1).
- iOS 26 opens Home Screen sites as standalone apps, so the page recommends "Add to Home Screen" on iPhone.
- There is no orientation lock on iOS, so both orientations are supported.
- Wake Lock works on iOS 16.4+.
- Memory: budget a ~300 MB working set. S9 measured a linear-memory peak of 14.9 MiB after 11 bundle loads and rejections.

### S6 audio check

No spike covered audio, so two checks were run for the issue (archived in `S6/`):
1. **`pin_audio` builds for wasm32 unmodified:** `cargo build --release --offline --locked --target wasm32-unknown-unknown -p pin_audio --no-default-features --lib` → `Finished` in 21.7 s (`S6/pin_audio_wasm32_build.log`).
2. **The mixer in an AudioWorklet equals native bit for bit.**
   - Setup: `pin_audio::Mixer` with 128-frame buffers sits behind a 3-function C ABI (`s6_new`, `s6_trigger`, `s6_render`). The wasm32 build (`+simd128`, 32 KiB, no imports) was instantiated with `new WebAssembly.Module` inside an `AudioWorkletProcessor` in Chromium 157, under the page CSP (`script-src 'self' 'wasm-unsafe-eval'`).
   - Run: an `OfflineAudioContext` at 48 kHz, 600 render quanta, and a scripted trigger sequence using all three buses (overlapping voices, callout ducking, an unknown sound id, a NaN gain).
   - Result: every quantum was 128 frames. The FNV-1a digest of all 76 800 stereo frames was **`a08428a4c4bc78aa`**, both inside the worklet and at the context destination, equal to the native x86_64 run of the same crate, with 0 console errors (`S6/audio_probe/run_chromium.log`).

Audio facts:
- An `AudioContext` starts only after a user gesture (`resume()`).
- iOS Web Audio obeys the mute switch.
- Output latency per device is residual U5 (S5 §4, §5, F5). Audio is presentation-only and cannot affect replays.

## Build, CI and local hosting

### Toolchain pins (S9 BOM, S2 §2, S5 F4, S10 versions, S11 §2)

| Tool | Version | Source and checksum |
|---|---|---|
| rustc / cargo | 1.97.0 (existing pin) + target `wasm32-unknown-unknown` | rustup |
| wasm-bindgen crate and CLI | **=0.2.130** (must match exactly; a 0.2.129 CLI fails with a schema mismatch, S3 §2) | `cargo install --locked wasm-bindgen-cli --version 0.2.130` on every build host, run by `fetch_tools.sh` into `target/web-tools/cargo` (crate checksum `c37f6904…fc7a`). The release assets for aarch64/x86_64-apple-darwin and x86_64-linux-musl are recorded in `tools.lock` but not used (S11 §2.5); no glibc Linux build is published (S5 F4) |
| js-sys, web-sys, wasm-bindgen-futures | 0.3.107, 0.3.107, 0.4.80 | `Cargo.lock` |
| wasm-tools | 1.263.0 | `cargo install --locked wasm-tools --version 1.263.0` (crate checksum `14b8ed79…dc96`); no published checksum for its release assets |
| wasi-sdk (clang 22.1.0, wasi-libc 2fc32bc81b9f, libsetjmp) | **32.0** | per host, `https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-32/`: `wasi-sdk-32.0-x86_64-linux.tar.gz` (143 157 058 B, `55fc523e…ef93`), `wasi-sdk-32.0-arm64-macos.tar.gz` (131 307 682 B, `383be7fe…b933d`), `wasi-sdk-32.0-x86_64-macos.tar.gz` (133 615 598 B, `a36c9ab1…1b1b`). No checksum file is published. All three have the same clang commit and a byte-identical `wasm32-wasip1` `libc.a`, `libsetjmp.a` and headers (S11 §2.4). System clang-18 does not work (S9 C5) |
| binaryen (`wasm-opt`) | **version_132**, native | `https://github.com/WebAssembly/binaryen/releases/download/version_132/`: `binaryen-version_132-x86_64-linux.tar.gz` (`195ddc94…0572`), `-arm64-macos.tar.gz` (`98aad827…59aa`), `-x86_64-macos.tar.gz` (`40c3de90…832e`). Each equals its published `.sha256`. On macOS `wasm-opt` loads `lib/libbinaryen.dylib`, so keep the extracted tree whole. The npm build is about 9 minutes per module; native takes 24-87 s |
| Node | 22 (pinned 22.23.5 in CI and `tools.lock`) | `actions/setup-node`; locally the nodejs.org build (`node-v22.23.5-{linux-x64,darwin-arm64,darwin-x64}.tar.gz`, sha256 in `tools.lock` = `SHASUMS256.txt`) or Homebrew `node@22` |
| Playwright / Chromium | 1.63.0 / 157.0.8103.78 headless shell, with its bundled SwiftShader Vulkan ICD | `npx playwright@1.63.0 install chromium` |
| JavaScriptCore shell | WebKit 327409@main (WebKitGTK `jsc` bundle), x86_64 | zip sha256 `c21a415deebd6e3eb867037d4ca290b9dd21bec12cc9c71f13403da7d8d042f0`. The server keeps builds for about a month, so store the pinned zip as an asset of a `ci-tools-web` release in this repo |
| SpiderMonkey shells | jsshell 160.0.1 and 153.7.0esr, linux-x86_64 | sha256 `b547d9ed0bc564ba4387a32cc09aa7aa490c4bdc32998a807c38d36a8078ad69` and `b453f9a0cf38a7d5087d51710b9d260effff11d09b7a89282897de7f06440f5c`; `SHA256SUMS` is GPG-signed by Mozilla. Stored like JSC |
| qemu (W05 only) | `qemu-user` 8.2.2, `gcc-aarch64-linux-gnu` | apt on `ubuntu-latest` |

Build hosts:
- **Supported:**
  - Linux x86_64, including WSL2.
  - macOS 15 or later on Apple silicon (`darwin-arm64`) and on Intel (`darwin-x86_64`).
- **Pinned binary floors** (S11 §6.8):
  - arm64: macOS 14.0. wasi-sdk's bundled `libedit.0.dylib` sets it; the other binaries need 11.0.
  - x86_64: macOS 11.0, set by Node. The tools themselves need 10.12-10.15.
  - macOS 14 is untested, because GitHub has no macOS 14 runner since 2026-11-02.
- **Signatures:**
  - Every arm64 binary in these archives is ad hoc signed (linker-signed), so no `codesign` step is needed.
  - The x86_64 binaries are unsigned, which Intel Macs allow.
  - Pins are on archives, never on extracted files.
- **macOS prerequisites:** the Xcode Command Line Tools (host linker for build scripts and proc macros).
- **Refused:** Linux on arm64 and Windows outside WSL2 stop with a message.
- **Windows:** wasi-sdk 32 and binaryen 132 also publish x86_64-windows tarballs (HTTP 200, S6 check), but no build ran there; native Windows stays unsupported.

### Build recipe (W20; S9 BOM §3-4; S11 for the hosts)

1. Install the toolchain:
   - `rustup toolchain install 1.97.0 --profile minimal --target wasm32-unknown-unknown`
   - on macOS also the Xcode Command Line Tools: `build_web.sh` checks `xcode-select -p`
2. Run `scripts/web/fetch_tools.sh`. It:
   - picks the platform key from `uname -s`/`-m`; `sysctl.proc_translated = 1` (Rosetta 2) selects `darwin-arm64`
   - downloads the `tools.lock` archives for that key with curl into `target/web-tools/`
   - checks each archive's sha256 and size, extracts it, and removes `com.apple.quarantine` on macOS
   - runs `cargo install --locked` for wasm-bindgen-cli and wasm-tools
   - runs every binary once (`--version`)
   - writes `target/web-tools/tools.env`
3. `scripts/web/build_web.sh` sources `wasm_env.sh`, which sets the environment:
   - It first removes host settings that would reach the wasm32 clang or rustc: `CFLAGS`, `CXXFLAGS`, `TARGET_CFLAGS`, `ARFLAGS`, `CPATH`, `C_INCLUDE_PATH`, `CPLUS_INCLUDE_PATH`, `OBJC_INCLUDE_PATH`, `CCC_OVERRIDE_OPTIONS`, `COMPILER_PATH`, `CRATE_CC_NO_DEFAULTS`, `RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS` and `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS`. cargo is then run under `env -u` for the hyphenated `CC_/CFLAGS_/AR_wasm32-unknown-unknown`.
   - `CC_wasm32_unknown_unknown` and `AR_wasm32_unknown_unknown`: the wasi-sdk `clang` and `llvm-ar`
   - `KHZ_WASI_SYSROOT=<wasi-sdk>/share/wasi-sysroot`, `KHZ_WASM_LEGACY_EH=true`, `KHZ_LUA_MATH=1` (until W09 makes it unconditional)
   - `CARGO_ENCODED_RUSTFLAGS` (0x1f-separated, so paths with spaces survive), in this order:
     - `-C target-feature=+simd128 -C link-arg=-zstack-size=4194304`
     - `--remap-path-prefix=<rustc sysroot>/lib/rustlib/src/rust=/rustc/<commit-hash>`
     - `--remap-path-prefix=<workspace>=/khz`
     - `--remap-path-prefix=<target dir>=/khz/target`
     - `--remap-path-prefix=<CARGO_HOME>=/cargo`
     - each prefix also in its physical (`pwd -P`) spelling when different, e.g. macOS `/tmp` = `/private/tmp`
   - `CC_SHELL_ESCAPED_FLAGS=1` and `CFLAGS_wasm32_unknown_unknown` = the same three prefixes as quoted `-ffile-prefix-map` flags
   - `CARGO_PROFILE_RELEASE_OPT_LEVEL=3`, `CODEGEN_UNITS=1`, `LTO=false`, `PANIC=abort`, `DEBUG=false`
   - Why the remap: without it the module embeds `$CARGO_HOME` 540 times, so the bytes depend on the user's home directory. `--no-remap` gives S9's exact flags.
   - Why LTO is declared off: cargo silently ignores it for a `[cdylib, rlib]` crate, and a real fat-LTO build was **larger** after `-Oz` (S9 C7, S9-Q5).
   - These flags live in `wasm_env.sh`, not in a `.cargo/config.toml`, so native builds and the byte-compared `tables/*/rules-wasm` rebuilds are unaffected.
4. Build each artifact: `cargo build --locked --release --target wasm32-unknown-unknown -p pin_web --no-default-features --features webgpu`, and the same with `--features webgl`.
5. Run `wasm-bindgen --target web --out-name pin_web`.
6. Run `wasm-opt -Oz --enable-simd --enable-exception-handling --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals --enable-reference-types --enable-multivalue --strip-debug --strip-producers`. Parity was re-proven on the optimised module (S9 §5). The output does not depend on the thread count (S11 §3.2).
7. Build the table bundles with `tables/*/build.sh` and `scripts/package.py`.
8. Copy everything to `target/web/dist/` and write `SHA256SUMS`. Hashing goes through Node, `shasum -a 256` or `sha256sum`, whichever exists.

`scripts/web/check_module.sh` fails on any of these:
- **Validation.** `wasm-tools validate --features=mvp,simd,exceptions,legacy-exceptions,bulk-memory,saturating-float-to-int,sign-extension,mutable-global,reference-types,multi-value` fails.
- **Imports.** Any import outside the glue module, or any `env.*` or `wasi_*` import. rustc turns undefined C symbols into silent `env` imports (S2 §8, S9 §4).
- **Exception handling.** `try_table` present, or legacy `try` and `try_table` mixed (JSC rejects mixed modules, S10 §6.3).
- **Relaxed SIMD.** Any relaxed-SIMD opcode (S1 A12).
- **Size.** The brotli size (quality 11, lgwin 24) exceeds 2.5 MiB (WebGPU) or 3.0 MiB (WebGL2).

Reproducibility (S11 §4):
- **Same host, same file system:** the sha256 is reproducible (S9 got the same sha256 from three builds).
- **Different checkout paths or `CARGO_HOME`s:** the sha256 is the same with the default remap (S11 a_remap = b_remap, `9f137f8a…`). The remap changes the Linux reference module itself, from S9's `88e06b16…` to `9f137f8a…`; `--no-remap` reproduces S9's bytes exactly (S11 §5.4).
- **Different directory order:** the same only once W08 sorts lua-src's sources.
- **Across host architectures (Linux x86_64 vs macOS arm64):** it is reported, not required. The arm64-host wasi-sdk clang emits different code for at least one input (compiler-rt `multc3`), and rustc's LLVM may too. Parity is the gate.

Script rules: POSIX `sh` only, so bash 3.2, dash and busybox all work. No GNU-only flags. Quote every path. Use `CDPATH= cd -- … && pwd -P` for directories, and `getconf _NPROCESSORS_ONLN` for the CPU count.

Cost: a cold cargo build takes 4 min 41 s on a 4-core VM and incremental builds 3-6 s (S9 §3). The `cargo install` of the two CLIs takes 2 min 23 s and 3 min 20 s on 2 jobs (S11 §5.4).

### tools.lock

The complete pin file (S11, [`S11/scripts/web/tools.lock`](S11/scripts/web/tools.lock)):

```text
# Web build tool pins. One line per tool and build-host platform; whitespace-separated
# columns; '#' starts a comment. Read by fetch_tools.sh (POSIX awk).
#
# columns: tool platform kind version sha256 bytes topdir url upstream_checksum
#   platform: linux-x86_64 | darwin-arm64 | darwin-x86_64 | any
#   kind:     archive = .tar.gz fetched with curl, sha256-verified, extracted under target/web-tools/<platform>/
#             cargo   = `cargo install --locked --root target/web-tools/cargo <tool> --version <version>`
#                       (sha256 = the crates.io index checksum of the .crate, which cargo verifies itself)
#   topdir:   the archive's single top-level directory ('-' for cargo)
#   upstream_checksum: the publisher's checksum file the sha256 was compared with ('none' if none is published)
#
# wasi-sdk 32.0 (clang 22.1.0-wasi-sdk, llvm 4434dabb6991, wasi-libc 2fc32bc81b9f): no checksum file is published
wasi-sdk       linux-x86_64   archive  32.0     55fc523ebfbc98f69d1034fcfcb83d1ff5610cd9ab7eceef6cd097a30ba4ef93  143157058  wasi-sdk-32.0-x86_64-linux  https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-32/wasi-sdk-32.0-x86_64-linux.tar.gz  none
wasi-sdk       darwin-arm64   archive  32.0     383be7fe00ae04691e1859164fd99970a528c55d624b886ed59f7933898b933d  131307682  wasi-sdk-32.0-arm64-macos   https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-32/wasi-sdk-32.0-arm64-macos.tar.gz  none
wasi-sdk       darwin-x86_64  archive  32.0     a36c9ab106a40abe8a051279c233c6bab719a5788ecc0788e8d141796eb55b1b  133615598  wasi-sdk-32.0-x86_64-macos  https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-32/wasi-sdk-32.0-x86_64-macos.tar.gz  none
# binaryen version_132 (wasm-opt); each archive has a published <archive>.sha256, equal to these
binaryen       linux-x86_64   archive  132      195ddc94f9bc89f45abdabb0b9eea86023d727ba90eac8b35b80f2544fc30572  117057408  binaryen-version_132  https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-x86_64-linux.tar.gz  https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-x86_64-linux.tar.gz.sha256
binaryen       darwin-arm64   archive  132      98aad827847af7ef990ed7098d885725c8e5b5aae75073403635617ae4e259aa  7640117    binaryen-version_132  https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-arm64-macos.tar.gz  https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-arm64-macos.tar.gz.sha256
binaryen       darwin-x86_64  archive  132      40c3de90bb3766bd0282a895e139a6f50253dba49b4f5bb89e66faca162d832e  9069142    binaryen-version_132  https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-x86_64-macos.tar.gz  https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-x86_64-macos.tar.gz.sha256
# wasm-bindgen CLI (must equal Cargo.lock's wasm-bindgen) and wasm-tools: built from crates.io source on every host
wasm-bindgen-cli  any         cargo    0.2.130  c37f6904387d19a319525c74af274c73912f3871c35555aa49f3ea64ebb2fc7a  -          -  https://crates.io/crates/wasm-bindgen-cli/0.2.130  https://index.crates.io/wa/sm/wasm-bindgen-cli
wasm-tools        any         cargo    1.263.0  14b8ed798ec13f0e0939dcc403836ba5fd7fb29ff9772e6438a9bcc227bfdc96  -          -  https://crates.io/crates/wasm-tools/1.263.0  https://index.crates.io/wa/sm/wasm-tools
# Node 22 (prerequisite; fetched only with `fetch_tools.sh --node`); SHASUMS256.txt is GPG-signed (SHASUMS256.txt.asc)
node           linux-x86_64   archive  22.23.5  c54118104acc524633b5abe106bbf44a53126ec8b4e5fa105cb92b5d7de8a1a1  56856114   node-v22.23.5-linux-x64     https://nodejs.org/dist/v22.23.5/node-v22.23.5-linux-x64.tar.gz  https://nodejs.org/dist/v22.23.5/SHASUMS256.txt
node           darwin-arm64   archive  22.23.5  e7131995759b61a0170b646010d2ad35a5d37b4cb0d471f3e8317eac6f742f3d  50004868   node-v22.23.5-darwin-arm64  https://nodejs.org/dist/v22.23.5/node-v22.23.5-darwin-arm64.tar.gz  https://nodejs.org/dist/v22.23.5/SHASUMS256.txt
node           darwin-x86_64  archive  22.23.5  c53383238206b9ae4db23c5d8a63645e2e753bec8ab51a49cc81939aa64e9422  51197448   node-v22.23.5-darwin-x64    https://nodejs.org/dist/v22.23.5/node-v22.23.5-darwin-x64.tar.gz  https://nodejs.org/dist/v22.23.5/SHASUMS256.txt
#
# Alternatives (release binaries; not used):
#   wasm-bindgen-0.2.130-aarch64-apple-darwin.tar.gz       b90ddb9357f584b1af897d58ad77e4cce2b48fc17e2a967bc249a87f72809444  (= its .sha256sum)
#   wasm-bindgen-0.2.130-x86_64-apple-darwin.tar.gz        eb209c115b7cb95832423c1aa3f4d06d6c53f63ef74fc4b6fb79ef0b2eea5f10  (= its .sha256sum)
#   wasm-bindgen-0.2.130-x86_64-unknown-linux-musl.tar.gz  9687f3bb551b9f66f37ed32a40d7913a00f7962db653054e56857bc306c3227f  (= its .sha256sum)
#   wasm-tools-1.263.0-aarch64-macos.tar.gz                b568f04746a699c4068b8d751779b60b501cf2cab6d259a3bc973eb5eeb1f62b  (no checksum file published)
#   wasm-tools-1.263.0-x86_64-macos.tar.gz                 909061aba8c069aeed8cb0933e50973a9c8e869bbd6a7819176b49871a338424  (no checksum file published)
#   wasm-tools-1.263.0-x86_64-linux.tar.gz                 21c036c1740a0552124c79c6e2170cb61c5b016005671a6d5bc06cf642165431  (no checksum file published)
```

Also pinned in CI, though not in `tools.lock`: the Rust 1.97.0 host packages from the channel manifest of 2026-07-09. rustup verifies these itself:
- `rustc-1.97.0-aarch64-apple-darwin.tar.xz` `53e339f16382bb348566f648d01775797e038fdd6bfc6ddd8859c5a0eac09f68`
- `rustc-1.97.0-x86_64-apple-darwin.tar.xz` `c3c6f6cd95e4546306dfa4beb3cec23dedf10dd02e6e7d99b053bf418d453014`
- `rust-std-1.97.0-wasm32-unknown-unknown.tar.xz` `474f926f2ffb0ed5ff895db92b7d0b2d9c3acb75b6df3229b132db0822594509`

### Local server (`scripts/web/serve.cjs`, W20; from S9 `web/serve.cjs`)

- Binds 127.0.0.1 by default. `--host <addr>` binds another address, and `--tls <cert> <key>` serves HTTPS for phones: a plain-HTTP LAN address is not a secure context (S5 §8).
- Every response carries `Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'` (S4 §7.1). `'wasm-unsafe-eval'` is needed and sufficient, and is supported from Chrome 97, Firefox 102 and Safari 16.
- `.wasm` is served as `application/wasm`, with ETag. Paths are kept inside the root.
- **No COOP/COEP.** An opt-in `--coop` flag exists for profiling only (S9-7).
- A test mode, `--results <file>`, accepts result POSTs from CI pages.
- Runs anywhere Node 22 runs: Linux, macOS (arm64, x86_64), and Windows for the release zip. It uses only `http`, `fs` and `path`, and the `path.sep` traversal check is correct on all three.
- On macOS, `--host 0.0.0.0` may trigger the application firewall's "accept incoming connections" prompt for `node`. Answer it, or stay on 127.0.0.1 with `adb reverse` for Android. Whether the prompt appears has not been measured yet; W27's macOS job records it.
- Command: `node scripts/web/serve.cjs target/web/dist 8790`.

### CI design (W05, W26-W28)

`.github/workflows/web.yml` is a separate workflow, like `rules-wasm.yml`. A `changes` job reads the PR diff (as `ci.yml` does) and sets one output per job group. No web job is part of `pr-checks`, so web jobs are **not** required for unrelated PRs (owner, Q11). The aarch64 job (iv) is a desktop check in `ci.yml` (W05). The repository is public (checked 2026-11-21), so standard GitHub-hosted runners, macOS included, cost nothing.

| Job | Trigger paths (trimmable) | Why these paths | Content | Cost (spikes) |
|---|---|---|---|---|
| `web-build` | union of the lists below | every other job needs the artifacts | recipe and `check_module.sh` for both artifacts; uploads `dist` | about 5 min cold build plus 2 × `wasm-opt` |
| `web-build-macos` (arm64 `macos-15`; Intel `macos-26-intel`) | arm64: same list as `web-build`, on pull requests. Intel: pushes to `main` on those paths, weekly `schedule`, `workflow_dispatch` | Proves the supported macOS build hosts with the same pins. Intel stays off pull requests to keep pull-request turnaround short. macOS 26 is expected to be the last Intel macOS (needs web, S11 §9) | Steps:<br>1. checkout<br>2. `dtolnay/rust-toolchain@1.97.0` with `targets: wasm32-unknown-unknown`<br>3. `actions/setup-node` 22.23.5<br>4. cache `target/web-tools`, keyed by OS, arch and the `tools.lock` hash, plus `Swatinem/rust-cache`<br>5. "Xcode CLT present" (`xcode-select -p`)<br>6. `/bin/sh scripts/web/fetch_tools.sh`, which runs the smoke checks<br>7. `/bin/sh scripts/web/build_web.sh` for both artifacts<br>8. `check_module.sh`<br>9. Node parity, 20 runs per artifact, against the committed goldens (hard)<br>10. `size_report.sh` budgets<br>11. "Module sha256 vs Linux (report)": compare with the `web-build` artifact's `SHA256SUMS`, then `::notice` or `::warning` and a summary line, never a failure<br>12. upload `SHA256SUMS` and `build.log` | Estimate per leg: 18-25 min cold, 8-12 min warm (S11 §7). Free: standard runners in a public repository |
| (i) `web-parity` (Node only) | `crates/pin_web/**`, `crates/pin_core/**`, `crates/pin_physics/**`, `crates/pin_input/**`, `crates/pin_sim/**`, `crates/pin_script/**`, `crates/pin_assets/**`, `third_party/lua-src/**`, `tables/**`, `scripts/web/**`, `Cargo.toml`, `Cargo.lock`, `versions.toml`, `.github/workflows/web.yml` | These are compiled into the web module and decide replay results: sim, physics, input staging, Lua, rules engine, bundle loading, build flags (`+simd128`, stack size), dependency versions (glam, wasmi, mlua), table content. A change there changes the web version's behaviour | per-tick parity: 3 tables × {bot60, pat60, mb8}, fixture and soak, × {Lua, wasm} against the native headless run of the same commit; S2 corpus, S8 probes, budget-edge, S4 corpus, per-tick-budget adversarial ticks, zip corpus; D5 guard; self-test | Node runs as in S9 §5 and §11 |
| (ii) `web-browser` (Chromium, SwiftShader) | `crates/pin_web/**`, `crates/pin_web_audio/**`, `scripts/web/**`, `.github/workflows/web.yml` | Web code only. `pin_render`'s WebGL2 rule is guarded natively by the GLSL ES test in `core-tests` (W18) | Playwright tests from W23-W25: replay while rendering on WebGPU and on WebGL2, backends and fallback, player bundles, failures, CSP, mobile emulation, frame guard, flipper frames, audio worklet digest, WGSL compile check, a Chromium per-tick-budget run (closes S8-10) | 3 tables render in about 7 s per backend (S3 §5.3) |
| (iii) `web-safari-firefox` (`macos-latest`) | same as (ii), plus `workflow_dispatch` | only real Safari and iOS answer U1 | Real Safari via `safaridriver` (`sudo safaridriver --enable`), iOS Simulator Safari (`xcrun simctl`), Firefox release via geckodriver. Each runs the self-test, `apex_circuit_bot60` (per-tick hashes against the committed native file, in the page's headless mode so no GPU is needed), `cstack_pcall_nest` and an audio-worklet smoke. Records `crossOriginIsolated`, the self-test verdict and per-tick time | free: standard runners in a public repository (S10 §9) |
| (iv) `replay-hash-aarch64` | in `ci.yml` (W05) | desktop gate | qemu-aarch64 fixture and seam digests; golden asserts on the macOS and Windows legs | 2-4 min warm, 8-12 min cold (S7 §6) |
| (v) `web-engines` (JSC and SpiderMonkey shells, x86_64, fast tiers) | same as (i), plus a weekly `schedule` and `workflow_dispatch` | the parity promise covers JSC and SpiderMonkey | `run_engines.sh` subset: sim 11 scenarios, Lua v2 corpus and probes, wasmi corpus, the 115-op float micro-test | about 10 min (S10 §10) |

Rules for all jobs:
- Each job fits the repository's 45-minute job limit (`tests/spec_ci`).
- No `continue-on-error` on gating steps.
- Chromium flags: `--enable-unsafe-webgpu --enable-features=Vulkan --use-angle=vulkan --ignore-gpu-blocklist`, with `VK_ICD_FILENAMES` and `VK_DRIVER_FILES` set to the headless shell's `vk_swiftshader_icd.json` (S3 §3.1).
- Playwright helpers poll with function `page.evaluate`, never `waitForFunction` and never `bypassCSP` (S9-Q4).
- Tools are cached, keyed by version and sha256.
- `release.yml` gains a `web-dist` job on `v*` tags and dispatch that attaches `kilohertz-web-<version>.zip` (`dist/`, `serve.cjs`, a short readme) to the draft prerelease. Publishing stays P8 `release`.
- macOS jobs run the scripts with `/bin/sh` (bash 3.2 in POSIX mode) on purpose. They never install GNU coreutils or a newer bash.
- Intel coverage lasts while GitHub offers an Intel image (`macos-26-intel`). After that, Intel is "best effort, same `darwin-x86_64` pins, untested". Rosetta 2 on an arm64 runner cannot stand in for an Intel CPU. Record the change in `docs/DEVELOPMENT.md` when the image is retired.
- The committed goldens come from the Linux native headless run. macOS native replays differ from x86_64 until W02 lands (S7 §4.5), so no macOS native build is used for web parity.
- Safari via `safaridriver` stays in `web-safari-firefox` (W27, S10 §9). It uses the Linux `web-build` artifact, not the macOS build.

## Gate A4

Gate A4 is REQ-WEB-024, owned by phase `33-web-ci`. It replaces the draft's automated web gate. Each check:

1. **Build.** Both artifacts build with the pinned toolchain on Linux x86_64, and on macOS: arm64 on web-path PRs, Intel on main, weekly and on dispatch. `wasm-tools validate` passes. The module imports only glue (0 `env.*`, 0 `wasi_*`). It has one EH encoding and 0 relaxed-SIMD ops. Sizes are within the 2.5 / 3.0 MiB brotli budgets. On macOS, Node parity against the committed goldens is hard; the module sha256 vs Linux is reported only (S11 §4.5).
2. **Desktop.** The full `ci.yml` matrix is green. The x86_64 fixture and the 5 seam digests equal the committed goldens, and aarch64 (qemu on PRs, macOS on main) and Windows equal them too.
3. **Web parity (Node).** 3 tables × {bot60, pat60, mb8}, plus the fixture and the 10-minute soak, × {Lua, `rules.wasm`}: per-tick `state_hash`, `ext_hash` and command-queue hash equal native.
4. **Sandbox parity (Node).** These are equal to native, line for line: the 79-case Lua corpus, the 13 S8 audit probes, budget-edge, the 82-line rules corpus with its fuel boundaries, the per-tick-budget adversarial ticks and the 33-file zip corpus.
5. **D5.** 0 allocations in `step_1ms` over 60 000 ticks per table, with Lua and with wasmi.
6. **Browser (Chromium).** All of these pass: replay while rendering on WebGPU and WebGL2 with per-tick parity; automatic backend fallback; player bundles; failure and recovery; CSP; mobile emulation; the frame guard; flipper response ≤ 2 presented frames; the audio worklet digest.
7. **Engines.** The JSC and SpiderMonkey shells (fast tiers) pass sim parity and both corpora. Real Safari (macOS and the iOS Simulator) and Firefox pass the self-test, `apex_circuit_bot60` and `cstack_pcall_nest`.
8. **Frame loop.** The simulated-time frame-loop tests pass (D8).
9. **Human checklist** (generated `docs/HUMAN_ACCEPTANCE.md`). One iPhone (iOS 26) and one Android 12+ phone with a Snapdragon: feel and latency on keyboard, gamepad and touch, audio and its loopback latency, multi-touch, memory, the iOS file picker, and Add to Home Screen.

## Work units

Every section below is the body source of one work-unit issue. The issue's table gives each unit's phase and dependencies.
- Labels are `type:*` and `area:*`. `area:web` and the `phase:3x-web-*` labels are new.
- "Runs" counts implementer, Independent Tester and fix runs.
- **Fable.** "[Fable wanted] candidate" means P3 very-high work where the P4 Fable rule may apply at run time. Otherwise the run continues on Opus 5.5 with a `[Fable wanted]` note in RUN_LOG.

### W01 Contract for portable min and max

- **Expected tier:** T1. **Labels:** `type:infra`, `area:sim`. **Runs:** 1-2.
- **Depends on:** none.
- **Owned paths:** `crates/pin_core/src/fmath.rs`, `crates/pin_core/src/lib.rs` (mod line). The CR-010 entry goes in `contracts/CHANGE_REQUESTS.md` (shared).
- **Start from:** S7 §3 (the `fp_ties` probe values).
- **Deliverables:**
  - CR-010 (additive): `pin_core::fmath::{min_f32, max_f32, min_f64, max_f64}`. They return exactly what `f32::min`/`max` return on x86_64 today, including ±0 ties (the second operand) and NaN handling.
- **Acceptance:**
  - A proptest equals std `min`/`max` on x86_64 for all special values (±0, ±inf, NaN payloads, subnormals) and random inputs.
  - Coverage ≥ 85 %.

### W02 Portable quaternion ops

- **Expected tier:** T2, very-high under P3: sim output changes on macOS arm64.
- **Fable:** [Fable wanted] candidate. S7 §11 judges Fable unnecessary ("the change is small and every effect is measured here"), so Opus 5.5 unless the allowance is already known to be sufficient.
- **Labels:** `type:fix`, `area:physics`. **Runs:** 2-3.
- **Depends on:** none.
- **Owned paths:**
  - `crates/pin_physics/src/quat_ops.rs` (new) and the 11 call sites in `ball/contact.rs`, `flipper/mod.rs`, `world/shapes.rs` and `world/static_world.rs`
  - `crates/pin_physics/tests/ball_seam_bitident.rs`, `spec_ball_common.rs`
- **Start from:** [S7 0006](S7/patches/0006-pin_physics-portable-quat-ops.patch), [S7 seam-input experiment](S7/patches/exp-seam-test-portable-inputs.patch), the S7 `mini/` probe.
- **Deliverables:**
  - `quat_ops` as 0006, with a proptest proving the replica equals glam SSE2 bit for bit (compiled where glam has its SSE2 or `simd128` path).
  - Seam test inputs built with `quat_ops` and `libm::sinf`/`cosf`; `RECORDING_TARGET` covers aarch64.
- **Acceptance:**
  - On x86_64, `pin_sim crates/pin_sim/tests/fixtures/replay_table --ticks 10000` gives the current golden (`25097c21c2c3adc1` at `995935f`).
  - The 5 seam digests equal EXPECTED, and `cargo test -p pin_physics` passes on x86_64.
  - Under qemu-aarch64 (S7 `env.sh` settings), the fixture gives the same value and the 5 seam digests equal EXPECTED.
  - Coverage ≥ 85 %.

### W03 Portable min and max with canonical NaN and clippy bans

- **Expected tier:** T2. **Labels:** `type:fix`, `area:physics`, `area:sim`. **Runs:** 2-3.
- **Depends on:** W01, W02.
- **Owned paths:**
  - the `f32`/`f64` `min`/`max` call sites in the 8 crates under the D9 libm ban (`pin_core`, `pin_physics`, `pin_input`, `pin_sim`, `pin_script`, `pin_assets`, `pin_tablegen`, `pin_assetgen`), and their `clippy.toml`
  - the `pin_sim` hash module (`Fnv64::write_f32`/`write_f64`)
- **Start from:** S1 §11 R-4, S7 §10.
- **Deliverables:**
  1. Every `f32`/`f64` `min`/`max` in those crates goes through `pin_core::fmath`. `f32::clamp` is pure comparisons and stays.
  2. Canonical NaN: every hashed float is written as one NaN pattern (S10 makes this required).
  3. `clippy.toml` `disallowed-types` and `disallowed-methods`:
     - SIMD-backed glam types `Vec3A`, `Vec4`, `Affine3A`
     - `Quat::{lerp, slerp}`, with the `Quat` operator rule as a review item (S7 §10)
     - `glam::{Vec3A, Vec4, Quat}::{min, max, clamp}`
     - `f32`/`f64` `min`/`max`

  If the diff exceeds about 800 lines, split it per crate.
- **Acceptance:**
  - The x86_64 fixture and seam goldens are unchanged.
  - Hashing any NaN payload gives one digest.
  - `clippy -D warnings` is clean.
  - Coverage ≥ 85 %.

### W04 Lua address-free pairs and string cache

- **Expected tier:** T2, very-high under P3 (vendored Lua patches).
- **Fable:** [Fable wanted] candidate; the evidence is under W08.
- **Labels:** `type:fix`, `area:script`. **Runs:** 2-3.
- **Depends on:** none.
- **Owned paths:**
  - `third_party/lua-src/`: table, string, state and object sources, `src/lib.rs` (native flags), `KHZ_PATCHES.md`
  - new `crates/pin_script/tests/spec_lua_addr_free.rs`
- **Start from:** [S8 0001](S8/patches/0001-lua-src-khz-33-37-s8.patch) (the KHZ 34 and 37 parts), S8 `probes/pairs/order.lua`.
- **Deliverables:**
  - KHZ 34: tables, closures, full userdata and threads hash by a per-state creation id (a 32-bit id that wraps deterministically). Light C functions and light userdata share one main position (S8 S8-5).
  - KHZ 37: the `luaS_new` cache bucket is `luaS_hash(content) % STRCACHE_N`.
  - `-ffp-contract=off` on every native lua-src arm. GCC contracts on aarch64 by default; there are 2 sites today, both unobservable (S8 F7).
  - KHZ_PATCHES.md rows. Each patch is unconditional: drop the spike's `KHZ_KEY_ORDER` knob and the KHZ 37 part of `KHZ_S8_FIXES`.
- **Acceptance:**
  - Two native processes give one `pairs` order digest and identical string-cache units and heap.
  - Production replays are unchanged.
  - Existing pin_script suites pass.
  - Coverage ≥ 85 %.
- **Why now.** This fixes a native run-to-run D9 violation that exists today (S8 F4, F5).

### W05 Desktop determinism gate

- **Expected tier:** T1. **Labels:** `type:infra`, `area:ci`. **Runs:** 1-2.
- **Depends on:** W02, W03, and the D9/R3 SPEC change on `main`.
- **Owned paths:**
  - `.github/workflows/ci.yml`: the `replay-hash`, `replay-hash-cross` and `pr-checks` jobs, and a new `replay-hash-aarch64` job
  - `tests/spec_ci/tests/spec_ci_yml.rs`
  - `crates/pin_sim/tests/fixtures/replay_table.golden` (new)
  - the release workflow's existing Rosetta verify leg (golden assert only)
  - the CI table in `docs/DEVELOPMENT.md`
- **Start from:** [S7 RESULTS §6](S7/RESULTS.md) (commands and cost), [S7 env.sh](S7/env.sh).
- **Deliverables:**
  1. A `replay-hash-aarch64` job on `ubuntu-latest`:
     - install `qemu-user` and `gcc-aarch64-linux-gnu`
     - `rustup target add aarch64-unknown-linux-gnu`
     - cross-build `pin_sim` with the linker and runner env vars
     - record the fixture and replay it 10 times under `qemu-aarch64 -L /usr/aarch64-linux-gnu`
     - fail unless the value equals the committed golden
     - run `ball_seam_bitident` for aarch64 under qemu, asserting EXPECTED
     - It runs on PRs touching the paths below and is aggregated by `pr-checks` (skipped counts as success). It is a desktop check, not a web check. Cost: about 2-4 min warm, 8-12 min cold (S7 §6).
  2. The x86_64 `replay-hash` job and the `macos-latest` and `windows-latest` legs assert the golden and EXPECTED instead of only reporting them. This needs no new runner minutes (D40).
  3. Path list for the aarch64 job (trimmable):
     - `crates/pin_core/**`, `crates/pin_physics/**`, `crates/pin_input/**`, `crates/pin_sim/**`, `crates/pin_script/**`, `crates/pin_assets/**`
     - `third_party/lua-src/**`, `tables/**`
     - `Cargo.toml`, `Cargo.lock`, `versions.toml`
     - `.github/workflows/ci.yml`
  4. Every job stays within the 45-minute job limit.
- **Acceptance:**
  - `spec_ci` is updated and green.
  - The aarch64 job passes on the unit's PR, and a temporary wrong golden makes it fail.
  - The first push to main shows the macOS leg asserting the golden (closes U2), and the Rosetta leg asserting it too (closes U11).
- **Note.** A later unit that changes simulation output for every table re-baselines this golden in its own PR (R3).

### W06 Script backend features and shared modules

- **Expected tier:** T2 (crate graph; native defaults unchanged). **Labels:** `type:infra`, `area:script`. **Runs:** 2.
- **Depends on:** [#128](https://github.com/donfuldauer/kilohertz-pinball/issues/128) KP-047. It changes `pin_script` Wasm init fuel and hook reset.
- **Owned paths:**
  - `crates/pin_script/Cargo.toml`, `src/lib.rs`, `src/config.rs` (new), `src/rng.rs` (new), `src/budget.rs` (new), and the module roots of `src/lua/` and `src/wasm/`
  - root `Cargo.toml` `[workspace.dependencies]` (pins under a comment naming the issue), `versions.toml` `[crates]`, `spikes/dep_smoke`, `docs/DEVELOPMENT.md`
- **Start from:** [S1 0001](S1/patches/0001-pin_script-native-backends-feature.patch), the feature part of [S2 0002](S2/patches/0002-pin_script-lua-feature-libm-web-parity.patch), [S9 pin_script.diff](S9/patches/pin_script.diff), [S9 workspace diff](S9/patches/workspace-Cargo.toml.diff).
- **Deliverables:**
  1. Features:
     - `lua` (mlua, libm)
     - `wasmtime` (wasmtime, wat, notify)
     - `wasmi` (wasmi 2.0.2 with `std`, `validate`, `auto-dispatch`, `deterministic`)
     - The default is `lua` + `wasmtime`, so native is unchanged.
     - wasmparser and wasm-encoder 0.261.0 become direct dependencies, so the pre-scan is engine-neutral.
     - The `lua::api` RNG helpers move to `rng.rs`, because `wasm/host.rs` imports them (S1 §11).
  2. `budget.rs`: the shared per-tick allowance (`TICK_BUDGET_UNITS = 200_000`; reset per `HostView::tick`; each callback runs with `min(per_callback, left)`), with unit tests. W11 calls it.
  3. Pins (with `spikes/dep_smoke` entries):
     - wasmi 2.0.2
     - wasmparser and wasm-encoder 0.261.0
     - the target-specific wasm-bindgen family (0.2.130 / 0.3.107 / 0.4.80)
     - a `wasm-smith` dev-dependency for W14's differential test, at the version this unit verifies
- **Acceptance:**
  - With default features, every existing `pin_script`, `pin_sim` and `pin_app` test passes unmodified, `rules-wasm` is green, and the fixture is unchanged.
  - `cargo clippy --target wasm32-unknown-unknown -p pin_script --no-default-features -- -D warnings` is clean.
  - Coverage ≥ 85 %.

### W07 Sim external script hook

- **Expected tier:** T2. **Labels:** `type:infra`, `area:sim`. **Runs:** 2.
- **Depends on:** W06.
- **Owned paths:** `crates/pin_sim/Cargo.toml`, `src/sim/`, `src/load/`, `src/replay/`.
- **Start from:** [S1 0002](S1/patches/0002-pin_sim-external-script.patch), [S1 0003](S1/patches/0003-pin_sim-apply-game-op.patch), [S9 pin_sim.diff](S9/patches/pin_sim.diff).
- **Deliverables:**
  - A `native-backends` feature that forwards the backend features.
  - A public `ExternalScript` trait and `Simulation::load_from_vfs_with_script`.
  - Public `replay::apply_game_op`.
  - `SimConfig` keeps its shape (S1 §3).
- **Acceptance:**
  - `cargo clippy --target wasm32-unknown-unknown -p pin_sim -p pin_script --no-default-features -- -D warnings` is clean. This is the S1 §3 command; S9 C6 shows what fails when defaults leak in.
  - Existing tests pass unmodified, and the fixture is unchanged.
  - Coverage ≥ 85 %.

### W08 Lua builds for wasm32

- **Expected tier:** T2, very-high under P3: "a change at the untrusted-table security boundary … the vendored Lua patches".
- **Fable:** [Fable wanted] candidate, the strongest in the plan:
  - S2 found three platform dependencies "invisible to every existing test".
  - S8's audit found four more (F1, F2, F5, F6), including a one-byte out-of-bounds read, none caught by a test.
  - The audit ran at Opus depth only because Fable was unavailable (S8 S8-8).
  - Fable for W08-W10 (or at least an independent re-audit) is recommended.
- **Labels:** `type:feature`, `area:script`, `area:web`. **Runs:** 2-3.
- **Depends on:** W04, W06.
- **Owned paths:** `third_party/lua-src/src/lib.rs` (wasm arm and env knobs), `khz-wasm/`, `luaconf.h`, `KHZ_PATCHES.md`.
- **Start from:** [S2 0001](S2/patches/0001-lua-src-web-khz-29-32.patch) (KHZ 29, 31), [S2 run_lua_parity.sh](S2/run_lua_parity.sh), [S11 0001](S11/patches/0001-lua-src-sorted-source-order.patch) (sorted source list; applies on top of S2 0001).
- **Deliverables:**
  - KHZ 29: the `wasm32-unknown-unknown` arm (wasi-sdk clang, `--sysroot`, `-mllvm -wasm-enable-sjlj -mllvm -wasm-use-legacy-eh=true`, `-ffp-contract=off`, io/os/loadlib/dblib left out, a shim of failing WASI stubs).
  - KHZ 31: `LUAL_BUFFERSIZE` 1024.
  - The `KHZ_WASI_SYSROOT` and `KHZ_WASM_LEGACY_EH` knobs, with `cargo:rerun-if-env-changed`.
  - `add_files_by_ext` sorts the source paths before adding them (S11 `patches/0001-lua-src-sorted-source-order.patch`, which applies on top of S2 0001). `fs::read_dir` returns the file system's order, and the Lua object order decides the link order and so the module bytes. On the same files, ext4 and tmpfs order gave two different modules (S11 §4.4). Native output is unchanged; native binary layout follows the sorted order. This is S11 finding 3: the directory order changes the module on Linux too.
  - KHZ_PATCHES.md rows.
- **Acceptance:**
  - `cargo build --target wasm32-unknown-unknown -p pin_script --no-default-features --features lua` with the wasi-sdk env succeeds.
  - The import audit finds 0 `env.*` and 0 `wasi_*`, and the module uses only legacy EH (`try`), no `try_table`.
  - The repo's Lua suites built for `wasm32-wasip1` pass under `node:wasi`, as S2 §12 did (204 tests).
  - Budget-edge equals native (2091 / 1092 / 1436 / 47 / 1483 / 4997 / 33664 / 16832).
  - Two checkouts whose file systems list `lua-5.4.9/*.c` in different orders (for example ext4 and tmpfs) give the same module bytes (S11 d_remap = e_remap).
  - Native suites are unchanged.

### W09 Lua number text and math parity

- **Expected tier:** T2, very-high (vendored Lua). **Fable:** [Fable wanted] candidate, as W08.
- **Labels:** `type:feature`, `area:script`. **Runs:** 2-3.
- **Depends on:** W08.
- **Owned paths:**
  - `third_party/lua-src/lua-5.4.9/lkhznum.c` (new), the number and format sources (`lobject.c`, `lstrlib.c`, `lmathlib.c`), `KHZ_PATCHES.md`
  - `crates/pin_script/src/lua/sandbox.rs`, `src/lua/libm_abi.rs` (new), and the pin_script tests below
- **Start from:** [S2 0001](S2/patches/0001-lua-src-web-khz-29-32.patch) (KHZ 30, 32), [S2 0002](S2/patches/0002-pin_script-lua-feature-libm-web-parity.patch), [S8 0001](S8/patches/0001-lua-src-khz-33-37-s8.patch) (KHZ 36).
- **Deliverables:**
  - KHZ 30, 32 and 36, unconditional (drop the spike's `KHZ_LUA_MATH` knob).
  - `libm_abi.rs` (`khz_*` via `libm =0.2.16`).
  - `collectgarbage` removed on every target.
  - Rewrite `lua_backend::sandbox_escapes_fail` and `emergency_gc_is_charged` (S2 §12).
- **Acceptance:**
  - The `lkhznum.c` C self-test passes, plus the 4 glibc-wrong subnormal vectors from Python's `float.fromhex`.
  - `collectgarbage` is nil.
  - The S2 format, parse and libm dumps equal the native golden.
  - The NaN text is `nan`.
  - Production replays are unchanged.
  - Coverage ≥ 85 %.

### W10 Lua memory and width

- **Expected tier:** T2, very-high (vendored Lua, memory cap). **Fable:** [Fable wanted] candidate, as W08.
- **Labels:** `type:feature`, `area:script`. **Runs:** 2-3.
- **Depends on:** W09.
- **Owned paths:**
  - `third_party/lua-src/`: memory, GC, state, auxlib, string and limits sources, plus the new `lkhzmem.h`
  - `crates/pin_script/src/lua/mod.rs`, and new `crates/pin_script/tests/spec_lua_platform.rs` and `spec_lua_corpus.rs`
- **Start from:** [S8 0001](S8/patches/0001-lua-src-khz-33-37-s8.patch) (KHZ 33, 35); the product parts of [S8 0002](S8/patches/0002-pin_script-s8-memcap-tickcap-instrumentation.patch) (`apply_memory_cap`, `S8_BACKSTOP_FACTOR`); KHZ 38 as `-DLUAI_MAXCCALLS=100` (S10 §8, `S10/harness/build_ccalls_variant.sh`); S8 `probes/`, S2 `cases/`; the gate scripts [S8 run_audit.sh](S8/run_audit.sh) and [S2 run_lua_sandbox.sh](S2/run_lua_sandbox.sh).
- **Deliverables:**
  - KHZ 33: logical accounting, the cap and a 2× real backstop.
  - KHZ 35.
  - KHZ 38.
  - All unconditional (drop the `KHZ_LOGICAL_MEM` and `KHZ_S8_FIXES` knobs).
  - A failing `lua_newstate` becomes a **load error** instead of mlua's `luaL_newstate` fallback, which would run Lua on the C heap outside the cap and the D5 guard (S9-Q2).
  - `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]` in `pin_script::lua`.
- **Acceptance:**
  - Golden outputs of the 13 S8 audit probes (`spec_lua_platform`), including the per-callback heap trace of `b_memstep_engine`.
  - The S2 79-case corpus outcome lines (`spec_lua_corpus`), with the 6 `cstack_*` texts changed by KHZ 38.
  - KHZ 33's compile-time LP64 size checks.
  - `string.sub("hello", 2^32)` returns `""`.
  - Production replays are unchanged: the fixture, and pin_sim's Lua replay and soak tests.
  - Coverage ≥ 85 %.

### W11 Per-tick script budget

- **Expected tier:** T2, very-high under P3 (budgets). **Fable:** [Fable wanted] candidate.
- **Labels:** `type:feature`, `area:script`. **Runs:** 2-3.
- **Depends on:** W06, W10.
- **Owned paths:** the Lua callback path in `crates/pin_script/src/lua/mod.rs` (`run()`), the current Wasm backend's callback path in `src/wasm/mod.rs`, and new `crates/pin_script/tests/spec_tick_budget.rs`.
- **Start from:** the tick-cap logic in [S8 0002](S8/patches/0002-pin_script-s8-memcap-tickcap-instrumentation.patch), [S8 0004](S8/patches/0004-s4_rules-s8-tick-cap-tooling.patch), [S8 run_tick_budget.sh](S8/run_tick_budget.sh).
- **Deliverables** (S8 §6.3):
  - 200 000 exact units (Lua: instructions, including the final partial hook interval, plus KHZ charges; Wasm: wasmtime fuel), shared by a tick's `on_switch_edge` calls and `on_tick`, through W06's `budget.rs`.
  - `on_init`, reload hooks and the host GC step do not count.
  - Exhaustion is a `BudgetExceeded` fault: rollback, coil release, scripts stop until a successful reload.
  - `MAX_PENDING_EDGES` stays 512.
- **Context.** Production ticks need ≤ 637 Lua units (314× margin) and ≤ 1 602 Wasm fuel (125×, S8 §6.1). The worst uncapped adversarial Lua tick takes 947 ms natively and 1 006 ms in Chromium; capped, it takes 3.0-3.8 ms (S8 §6).
- **Acceptance:**
  - Lua: 512 edges at 99 % of the budget give `BudgetExceeded` after exactly the S8 unit counts.
  - Wasm: 513 callbacks use exactly 200 000 fuel, then fault.
  - 512 cheap edges plus `on_tick` are not refused, the allowance resets each tick, and `on_init` is uncounted.
  - Production replays with the budget on equal those with it off (Lua and Wasm).
  - Coverage ≥ 85 %.

### W12 Engine-neutral pre-scan and host core

- **Expected tier:** T2. **Labels:** `type:infra`, `area:script`. **Runs:** 2.
- **Depends on:** W11.
- **Owned paths:** `crates/pin_script/src/wasm/` (`limits.rs` → `prescan.rs`, new `abi.rs`, new `host_core.rs`, `host.rs`).
- **Start from:** [S4 `prescan.rs`, `abi.rs`, `hostcore.rs`](S4/crates/s4_rules/src), [S4 0001](S4/patches/0001-pin_script-s4-readonly-probes.patch).
- **Deliverables:**
  - The pre-scan and ABI checks use `wasmparser` directly.
  - The 18 host functions (`Book`, `QueueMark`, ids, RNG) move into an engine-neutral `host_core`.
  - Trap kinds are mapped to fixed texts in the shared host.
- **Acceptance:**
  - Native behaviour is unchanged: the existing Wasm suites pass and `rules-wasm` (pin_app `wasm_ports` parity) is green.
  - Coverage ≥ 85 %.

### W13 Fuel instrumentation

- **Expected tier:** T2. The rewrite is security-boundary code, but it is not on the run-time path until W15.
- **Labels:** `type:feature`, `area:script`. **Runs:** 2-3.
- **Depends on:** W12.
- **Owned paths:** `crates/pin_script/src/wasm/instrument/` (fuel), and new tests `spec_instr_fuel_exact.rs` and `spec_instr_boundaries.rs`.
- **Start from:** [S4 instrument.rs](S4/crates/s4_rules/src/instrument.rs) (fuel parts).
- **Deliverables:**
  - Per function, from the original operators:
    - entry: `fuel -= 1; if fuel <= 0 trap`
    - costs: 0 for nop/drop/block/loop/unreachable/return/else/end, 1 otherwise (wasmtime-environ `default_operator_cost`)
    - the buffered cost is flushed before control flow and checked at loop headers
  - Bulk operations follow `pre/post_translate_bulk_op` (constants ≤ 128 charged up front, otherwise charged after success and then checked), with Cranelift-equivalent constant detection.
  - Unreachable code is not charged.
- **Acceptance:**
  - `spec_instr_fuel_exact`: for every callback of the three production modules over the repo's recorded wasm-port games, instrumented fuel used == wasmtime fuel used.
  - `spec_instr_boundaries`: random loop 9 091/9 092, constant fill 893/894, constant via a local 892/893, `id_of` with 60 000-byte names (first call 60 018 fuel, second exhausts), and a 2 MiB deferred fill exhausts the budget.
  - Coverage ≥ 85 %.

### W14 Depth counter caps and NaN canonicalisation

- **Expected tier:** T2. **Labels:** `type:feature`, `area:script`. **Runs:** 2-3.
- **Depends on:** W13.
- **Owned paths:** `crates/pin_script/src/wasm/instrument/` (depth, caps, nan, start), and new tests `spec_depth_counter.rs`, `spec_nan_canonical.rs` and `spec_instr_differential.rs`.
- **Start from:** [S4 instrument.rs](S4/crates/s4_rules/src/instrument.rs), S4 DECISIONS S4-2, S4-3, S4-4.
- **Deliverables:**
  - The weighted depth counter (`16·(params+locals+max stack)+64`, limit 256 KiB).
  - Memory maxima capped at `min(declared, 2 MiB)` and table maxima at `min(declared, 65 536)`.
  - NaN canonicalisation after add, sub, mul, div, min, max, sqrt, ceil, floor, trunc, nearest, demote and promote.
  - The start function exported and called with budget − 1 (S4-4).
  - Instrumentation globals renamed when the module's exports collide with them.
  - Growth: apex goes from 20 603 to about 23 072 B.
- **Acceptance:**
  - `spec_depth_counter`: 2 338 / 711 / 229 / 78 / 15 frames for 0 / 16 / 64 / 200 / 1 000 live locals; production depth ≤ 4 calls.
  - `spec_nan_canonical`: all 9 NaN cases return canonical bits.
  - `spec_instr_differential` (proptest plus wasm-smith modules within the D25 limits): outcome class, fuel, queue, RNG and memory digests equal between wasmtime on the original module and wasmtime on the instrumented one (wasmi joins in W16), except the documented stack kill point.
  - Every instrumented module validates under the engine feature set.
  - Coverage ≥ 85 %.

### W15 Native rules run the instrumented module

- **Expected tier:** T2, very-high under P3: native sandbox enforcement changes. The rewrite now decides fuel and stack natively, and deep recursion faults at 2 338 instead of 5 453 frames (S4 §10.2).
- **Fable:** [Fable wanted] candidate. Exactness rests on emulating Cranelift's accounting, proven on the corpus and on the production modules but not for every module shape.
- **Labels:** `type:feature`, `area:script`. **Runs:** 2-3.
- **Depends on:** W14.
- **Owned paths:** `crates/pin_script/src/wasm/mod.rs`, the re-baselined existing Wasm stack-depth tests, and new `spec_web_sandbox.rs` (corpus).
- **Start from:** [S4 wasmtime_instr.rs](S4/crates/s4_rules/src/wasmtime_instr.rs), [S4 adversarial.rs](S4/crates/s4_rules/src/adversarial.rs), [S4 worst.rs](S4/crates/s4_rules/src/worst.rs), [S4 run_rules_adversarial.sh](S4/run_rules_adversarial.sh), [S4 run_rules_parity.sh](S4/run_rules_parity.sh).
- **Deliverables:**
  - `WasmScriptBackend` instruments every module before compiling. wasmtime's `consume_fuel` stays on as the backstop at a multiple of the budget.
  - The instrumentation's `why` global classifies the fault.
  - The W11 per-tick budget is preserved.
- **Acceptance:**
  - The 82-line adversarial corpus (load class, per-call outcome, fuel, queue, RNG, memory) against today's native results differs only in the documented stack kill point lines (S4 §5: E=80 C=1 D=1).
  - Production fuel per callback is unchanged.
  - `rules-wasm` is green, so production replays are unchanged.
  - The re-baselined depth tests assert the counter's limits.
  - Coverage ≥ 85 %.

### W16 wasmi rules backend

- **Expected tier:** T2. **Labels:** `type:feature`, `area:script`, `area:web`. **Runs:** 2.
- **Depends on:** W15.
- **Owned paths:** `crates/pin_script/src/wasm/wasmi.rs`, and new tests `spec_wasmi_alloc.rs` and `spec_wasmi_corpus.rs`.
- **Start from:** [S4 wasmi_backend.rs](S4/crates/s4_rules/src/wasmi_backend.rs).
- **Deliverables:** a wasmi backend behind the `wasmi` feature:
  - features `std`, `validate`, `auto-dispatch`, `deterministic`
  - `CompilationMode::Eager`
  - `set_min_stack_height(256 KiB)`
  - `StoreLimits` as pin_script's
  - wasmi fuel as a backstop at 4 × budget + 4 Mi
  - recursion and value-stack limits above the counter
- **Acceptance:**
  - `spec_wasmi_corpus`: the 82-line corpus is identical between native wasmtime-instrumented and native wasmi (82/82).
  - `spec_wasmi_alloc`: 0 allocations over 60 000 ticks per table.
  - A wasm32 build run under Node ends a 100 000-fuel back-edge loop as `BudgetExceeded` without `RangeError`, which catches a lost `auto-dispatch` (S4 §6.4).
  - Coverage ≥ 85 %.

### W17 WebGPU render path

- **Expected tier:** T2. High blast radius: the native render path is touched, but not sim output.
- **Labels:** `type:feature`, `area:render`, `area:web`. **Runs:** 3-4 (2 implementer, 1 Independent Tester, 1 fix).
- **Depends on:** [#96](https://github.com/donfuldauer/kilohertz-pinball/issues/96) KP-016, [#97](https://github.com/donfuldauer/kilohertz-pinball/issues/97) KP-017, [#102](https://github.com/donfuldauer/kilohertz-pinball/issues/102) KP-022, [#116](https://github.com/donfuldauer/kilohertz-pinball/issues/116) KP-035, [#133](https://github.com/donfuldauer/kilohertz-pinball/issues/133) KP-052. These are the M1 and M2 units that change the render paths this unit touches.
- **Owned paths:**
  - `crates/pin_render/`: `Cargo.toml`, `src/gpu/`, and the named `pollster` sites in `src/post/chain.rs`, `src/raytrace/source.rs` and `src/reflections/probes.rs`
  - the root `Cargo.toml` `wgpu`, `egui-wgpu` and `naga` lines (adds `glsl-out` to the naga dev pin for W18)
  - the `versions.toml` `[features]` wgpu line
- **Start from:** [S3 pin_render.diff](S3/patches/pin_render.diff), [S3 pin_render-Cargo.toml.diff](S3/patches/pin_render-Cargo.toml.diff), [S3 workspace-Cargo.toml.diff](S3/patches/workspace-Cargo.toml.diff), [S9 pin_render.diff](S9/patches/pin_render.diff), S3's probe readback (`web.rs` `map_read`), [S3 run_render_probe.sh](S3/run_render_probe.sh).
- **Deliverables:**
  1. The manifests described under Rendering.
  2. `GpuContext::request_async`, with native keeping its sync API through `pollster` wrappers.
  3. Deferred error-scope handling: one helper, polled once on GL and deferred via `spawn_local` on WebGPU.
  4. A canvas surface and format choice: `Rgba8Unorm` plus an `Rgba8UnormSrgb` view on WebGPU.
  5. Presenter threads and blit code gated to native, so the wasm32 build has 0 dead-code warnings.
  6. Async readback.
  7. `Send` callback data.
  8. A browser shader-compile check (`ShaderModule::get_compilation_info`) for every WGSL module; the ray-query module is expected to fail.
- **Acceptance:**
  - Native `gpu-tests` on lavapipe, WARP and Metal are unchanged, with the same goldens (D30).
  - `cargo clippy -p pin_render --target wasm32-unknown-unknown --features webgpu -- -D warnings` is clean, without `fragile-send-sync`.
  - Unit tests for the scope helper.
  - The browser checks, run in W27's job: every WGSL module compiles in Chromium, and the tables' frames pass the D30 analytic pixel checks. For the same snapshot, mean absolute difference against the native lavapipe frame is ≤ 0.5/255 and ≤ 1 % of pixels differ by more than 8 (measured 0.073 and 0.38 % pre-M1, S3 §9).
  - Coverage ≥ 60 %.

### W18 WebGL2 fallback

- **Expected tier:** T2. **Labels:** `type:feature`, `area:render`, `area:web`. **Runs:** 3 (implementer, Independent Tester, one fix round; S3 §5.3).
- **Depends on:** W17.
- **Owned paths:** `crates/pin_render/src/pbr/`, `src/gpu/` (limit choice), the surface format code, the reflections layer count, and new `crates/pin_render/tests/glsl_es300.rs`.
- **Start from:** the WebGL2 prototype in [S3 pin_render.diff](S3/patches/pin_render.diff) (`forward_source_without_storage`, `engine_base_limits`), [S3 effort_webgl2.md](S3/effort_webgl2.md), S3 §5.
- **Deliverables:**
  1. A real second forward shader entry (not text substitution), chosen at run time when the device has fewer than 2 fragment storage buffers.
  2. `downlevel_webgl2_defaults().using_resolution(..)` on such adapters.
  3. A 2-layer placeholder array on GL.
  4. The WebGL2 canvas format (S3 W6).
  5. A `webgl` feature for the second artifact; the instance comes from `new_instance_with_webgpu_detection` when both backends are present.
  6. An `EXT_color_buffer_float` check, reported to the shell.
  7. A GLSL ES 3.00 naga translation test of every app-path module. It fails if anyone adds a fragment or vertex storage buffer, a storage texture or compute without a GL variant (the maintenance rule from S3 §5.3).
- **Acceptance:**
  - Native `gpu-tests` are unchanged, and the GLSL ES test passes in `core-tests`.
  - The wasm32 `webgl` build is clippy-clean.
  - In the browser job, the WebGL2 frames match WebGPU within 4 pixels (≤ 26/255), and the egui overlay paints.
  - Coverage ≥ 60 %.

### W19 Web engine crate and headless replay

- **Expected tier:** T2. **Labels:** `type:feature`, `area:web`. **Runs:** 3.
- **Depends on:** W03, W07, W10, W11, W16.
- **Owned paths:**
  - `crates/pin_web/` (new): `Cargo.toml`, `src/lib.rs`, `src/engine/`, `src/headless/`, `src/loader/`, and module stubs for `frame`, `selftest`, `web`, `input` and `ui`
  - `crates/pin_web/tests/` and its fixtures
  - `crates/pin_assets/tests/hardening_round2.rs` (the 32-bit `zip64_mutations` fix, S4 §8)
- **Start from:** [S9 engine.rs](S9/crates/pin_web/src/engine.rs), [S9 lib.rs](S9/crates/pin_web/src/lib.rs), [S4 zipcheck.rs](S4/crates/s4_rules/src/zipcheck.rs), [S9 parity_node.mjs](S9/node/parity_node.mjs), [S9 probes_node.mjs](S9/node/probes_node.mjs), [S1 s1_harness (patch 0004)](S1/patches/0004-s1_harness-spike-tooling.patch) for the record, playback and `ext_hash` code; the zip corpus [generator](S4/harness/make_zip_corpus.py) and [listing](S4/zipcorpus/CORPUS_LISTING.md).
- **Deliverables:**
  1. **Engine.** One engine crate owns the counting `#[global_allocator]` (the D5 guard and heap statistics) and every export.
     - Bundle bytes go to `Vfs::from_zip_bytes` with the web `ZipLimits`, then to `Simulation::load_from_vfs_with_script`.
     - The backend comes from `script_entry`: Lua, or wasmi with instrumentation.
     - `Simulation` is boxed (61 KB), not held on the shadow stack.
     - A panic hook reports through `console.error`.
  2. **Headless replay** (Node and native, same code):
     - replays `GameRecording`s and pattern recordings
     - emits per-tick `state_hash`, `ext_hash` (S1's definition) and the command-queue hash
     - runs the Lua per-step stream check (tick, RNG draws, input digest, queue)
     - fixtures: recordings for 3 tables × {bot60, pat60, mb8}, about 3-36 KB each (S1 §4); the soak is generated in CI
- **Acceptance:**
  - **Node parity check** (closes U7: KHZ 33-38 inside the combined module): 3 tables × {bot60, pat60, mb8}, plus the fixture and soak, × {Lua, `rules.wasm`} equal the native headless run per tick (20/20).
  - In the module: the S2 79-case corpus, S8 audit probes, budget-edge, the S4 82-line corpus and the 33-file zip corpus equal native.
  - 0 allocations in `step_1ms` over 60 000 ticks per table with both backends.
  - `cargo clippy --target wasm32-unknown-unknown -p pin_web --no-default-features -- -D warnings` is clean (S9 §16).
  - `pin_web` is added to the 60 % coverage floor in `docs/DEVELOPMENT.md`.
  - Coverage ≥ 60 %.

### W20 Web build script and local server

- **Expected tier:** T1. **Labels:** `type:infra`, `area:ci`, `area:web`. **Runs:** 2-3 (macOS support adds one).
- **Depends on:** W19.
- **Owned paths:** `scripts/web/` (`lib.sh`, `tools.lock` platform-keyed: `linux-x86_64`, `darwin-arm64`, `darwin-x86_64`, `fetch_tools.sh`, `wasm_env.sh`, `build_web.sh`, `check_module.sh`, `serve.cjs`, `size_report.sh`, `tests/`), the `versions.toml` `[notes]` entry pointing to `tools.lock`, and `docs/DEVELOPMENT.md`.
- **Start from:** [S11 scripts/web](S11/scripts/web) (portable POSIX `sh` rewrites: `lib.sh`, `tools.lock`, `fetch_tools.sh`, `wasm_env.sh`, `build_web.sh`, `size_report.sh`, `check_imports.sh`), [S11 test_portability.sh](S11/scripts/test_portability.sh), [S11 repro_paths.sh](S11/scripts/repro_paths.sh), [S9 build_web.sh](S9/build_web.sh), [S9 check_imports.sh](S9/check_imports.sh), [S9 BOM.md](S9/BOM.md), [S9 web/serve.cjs](S9/web/serve.cjs), [S9 scripts/](S9/scripts) (`get_binaryen.sh`, `size_report.sh`).
- **Deliverables:**
  - the build recipe, `tools.lock`, `check_module.sh` and the local server described under "Build, CI and local hosting"
  - the scripts are POSIX `sh` and select the platform with `lib.sh`
  - hashing goes Node first, then `shasum -a 256`, then `sha256sum`
  - `fetch_tools.sh` removes `com.apple.quarantine` on macOS
- **Acceptance:**
  - The script builds both artifacts from a clean checkout, records `SHA256SUMS`, and runs on Linux x86_64 and macOS (arm64, x86_64). On any other host it stops with a message (exit 3).
  - Every `scripts/web/*.sh` is POSIX `sh` and passes ShellCheck `--shell=sh`. A test runs them under dash, bash in POSIX mode and busybox sh, with a PATH missing `sha256sum`, `nproc` and `realpath` and trapping `readlink -f`, `stat -c`, `sed -i`/`-r`, `grep -P`, `date -d` and `xargs -r`. Shimmed `uname`/`sysctl` select `darwin-arm64` (also under Rosetta 2), `darwin-x86_64` and `linux-x86_64`, and refuse Linux/aarch64, MINGW and FreeBSD (S11 `test_portability.sh`).
  - `fetch_tools.sh` rejects an archive whose sha256 or size differs from `tools.lock`: it deletes the archive and extracts nothing. A second run downloads nothing. On macOS it removes `com.apple.quarantine`, and it runs every fetched binary once (`--version`).
  - `wasm_env.sh` removes `CFLAGS`, `TARGET_CFLAGS`, `CPATH`, `C_INCLUDE_PATH`, `CCC_OVERRIDE_OPTIONS` and `RUSTFLAGS` from the build environment (test with Homebrew-style values).
  - On Linux, `build_web.sh --no-remap` reproduces S9's module for S9's tree (`88e06b16…`). Two checkouts with different paths and different `CARGO_HOME`s give the same sha256 with the default remap (S11 `repro_paths.sh`).
  - `check_module.sh` fails on a module with an injected `env` import, on a mixed-EH module, on a relaxed-SIMD module and on an oversize module (test modules generated by `wasm-tools`).
  - A `serve.cjs` test checks the CSP header, the MIME type, the absence of COOP/COEP and that traversal is refused.
  - The wasi-sdk pins in `tools.lock` equal the GitHub release asset `digest` fields of the four wasi-sdk 32 assets. wasi-sdk publishes no checksum file (S11 M3, §9).

### W21 Frame loop frame guard and self-test

- **Expected tier:** T2. **Labels:** `type:feature`, `area:web`. **Runs:** 2-3.
- **Depends on:** W19.
- **Owned paths:** `crates/pin_web/src/frame/`, `src/selftest/`, and their tests.
- **Start from:** [S10 selftest.js](S10/harness/selftest.js), S9 `web/main.js` (loop).
- **Deliverables:**
  1. **Frame accounting**, host-testable (D8):
     - due ticks from rAF timestamps
     - at most 50 ticks per frame, with time beyond dropped and counted as `SimLoop::skip_ticks` does
     - pause on hidden, with a fresh epoch on resume
     - inputs placed by `event.timeStamp` (REQ-LAT-004 on the browser clock)
     - the tick API takes `f64`, validates it and refuses more than 10 000 per call
  2. **Frame guard:** stop stepping after 8 ms of script time in a frame, and show "rules too slow" after 2 s of throttling.
  3. **Startup self-test** ((a)-(c) under Shell evidence) with the fixture and constants embedded, plus no-JIT detection.
- **Acceptance:**
  - Host tests: tick placement exact in all three staging modes; the 50-tick cap; pause and resume.
  - Frame guard: a module using 99 999 fuel per callback never runs more than one callback past 8 ms, and the hashes are unchanged.
  - The self-test passes in Node.
  - Coverage ≥ 60 %.

### W22 Web audio worklet

- **Expected tier:** T2. **Labels:** `type:feature`, `area:audio`, `area:web`. **Runs:** 2-3.
- **Depends on:** [#129](https://github.com/donfuldauer/kilohertz-pinball/issues/129) KP-048, [#130](https://github.com/donfuldauer/kilohertz-pinball/issues/130) KP-049 (audio decoding changes).
- **Owned paths:** `crates/pin_web_audio/` (new: a cdylib with the C ABI and the worklet processor JS) and `crates/pin_audio/src/mixer/decode.rs` (the decode budget becomes a parameter).
- **Start from:** [S6 audio_probe](S6/audio_probe): `src/lib.rs`, `web/processor.js`, `web/main.js`, `run.cjs`.
- **Deliverables:**
  1. `pin_audio::Mixer` (no default features) in a worklet module.
     - Samples are registered from transferred PCM.
     - Triggers arrive by message and are applied at the next quantum.
     - Rendering uses 128-frame quanta.
     - Stereo only (SSF 7.1 stays native-only).
  2. Log `baseLatency` and `outputLatency` where available.
  3. **Fallback**, used if a browser cannot instantiate wasm in the worklet (U6): the same mixer runs in the engine module, and a JS-only processor copies its blocks.
- **Acceptance:**
  - An OfflineAudioContext test in Chromium: for a scripted trigger sequence, the worklet output digest equals native `Mixer::render` (the S6 check method).
  - 0 allocations in `render`.
  - The trigger message protocol is property-tested.
  - Coverage ≥ 60 %.

### W23 Web page and loader

- **Expected tier:** T2. **Labels:** `type:feature`, `area:web`. **Runs:** 2-3.
- **Depends on:** W18, W20, W21.
- **Owned paths:** `crates/pin_web/src/web/` (wasm-bindgen surface), `crates/pin_web/www/` (`index.html`, `main.js`, `style.css`), and `crates/pin_web/tests/browser/` (Playwright).
- **Start from:** [S9 web.rs](S9/crates/pin_web/src/web.rs), [S9 web/](S9/web), [S9 node/](S9/node) (`t_replay`, `t_backends`, `t_player`, `t_failures`, `t_loadtime`, `t_synccompile`, `pw.cjs`).
- **Deliverables:**
  1. **Loader script.**
     - Request a WebGPU adapter. Load the WebGPU artifact, or else the WebGL2 artifact (WebGL2 with `EXT_color_buffer_float`). If neither applies, show the "browser not supported" page.
     - Async `init()` only. No inline script or style (CSP).
  2. **Loop.** Drive the engine from rAF. Pace frames on `on_submitted_work_done`: skip drawing while the GPU is busy, but keep stepping the sim.
  3. **Player bundles.** File picker (no `accept` attribute) and drop through one `File` path, with the magic-byte check and `File.size` ≤ 32 MiB checked before the copy. The bundle is passed by value into the loader.
  4. **Crash handling.** A crash banner, re-instantiation through a fresh glue-module URL, and the late `Module terminated` callbacks swallowed (S9-Q7).
  5. **Self-test verdict.** If it fails, player bundles are refused; on a no-JIT engine, the page shows the message.
  6. **Presentation.** The scene as the desktop single-window mode after M1 (REQ-VIEW-001/002), with the PiP option using the aspect-aware layout (portrait top band, landscape side columns; S9-Q8).
- **Acceptance** (Playwright Chromium; R12 windowed evidence is the presented canvas, which S3 §3.1 showed equals the readback):
  - Replay while rendering on WebGPU and on WebGL2 with per-tick parity and 0 GPU, console or page errors.
  - The `auto` backend picks WebGPU, or WebGL2 without an adapter.
  - Player bundles by picker and by drop: hostile zips are rejected with the native texts; a 33 MiB file is refused with linear memory unchanged; the engine stays usable.
  - A Lua error, a budget kill, a fuel kill and an engine trap each end in tilt or re-instantiation, as S9 §10.
  - The page works under the CSP and fails without `'wasm-unsafe-eval'`.
  - The load time is reported.
  - Test helpers poll with function `page.evaluate`; `waitForFunction` hangs under the CSP (S9-Q4).
  - The bundle sizes of the M1 tables are measured against the 32 MiB limit (U14).

### W24 Web input

- **Expected tier:** T2. **Labels:** `type:feature`, `area:input`, `area:web`. **Runs:** 2-3.
- **Depends on:** W23, [#81](https://github.com/donfuldauer/kilohertz-pinball/issues/81) KP-001 (input ingestion), [#101](https://github.com/donfuldauer/kilohertz-pinball/issues/101) KP-021 (camera keys).
- **Owned paths:** `crates/pin_web/src/input/` and the page input script under `crates/pin_web/www/`.
- **Start from:** the S5 A26 table, [S9 node/](S9/node) (`t_mobile`), S3 §7 (egui input events).
- **Deliverables:**
  - The `code` → HID table.
  - Gamepad standard mapping, polled once per frame; manual mapping in the menu otherwise.
  - Pointer touch zones for the left and right flippers, launch, start and menu, kept away from the screen edges.
  - egui events: pointer, wheel, key, text, touch, focus, `devicePixelRatio`, `ResizeObserver`.
  - Cameras (REQ-CAM-002) through the menu, `V`/`B` and the D-pad, without needing keys the browser reserves.
  - The menu toggle also on an on-screen button, because `Esc` leaves fullscreen.
- **Acceptance:**
  - A key-table unit test.
  - Tick placement exact against `event.timeStamp`.
  - Mobile emulation: touch raises flipper 0 or 1 only, and launch works.
  - In the browser job, at most two presented frames from a flipper event to the first frame showing that bat moved ≥ 1° (REQ-WEB-020).

### W25 Web menu settings audio and mobile

- **Expected tier:** T2. **Labels:** `type:feature`, `area:web`. **Runs:** 2.
- **Depends on:** W22, W24, [#84](https://github.com/donfuldauer/kilohertz-pinball/issues/84) KP-004 (menu live), [#115](https://github.com/donfuldauer/kilohertz-pinball/issues/115) KP-034 (displays on the 3D backbox), [#116](https://github.com/donfuldauer/kilohertz-pinball/issues/116) KP-035 (quality presets).
- **Owned paths:** `crates/pin_web/src/ui/`, and the page menu and audio scripts under `crates/pin_web/www/`.
- **Start from:** [S9 node/](S9/node) (`t_mobile`), S5 F1.
- **Deliverables:**
  1. The reduced egui menu (`OverlayPainter`, no eframe):
     - table picker: the three tables, plus "load bundle"
     - input settings
     - display settings: camera presets and the M1 quality presets
     - settings and last table stored in `localStorage`
  2. Start overlay: the first gesture unlocks audio; on iPhone, an "Add to Home Screen" hint.
  3. Audio hook-up: transfer the PCM to the worklet once at load (decoded with `pin_audio::decode` and the 64 MiB web budget); post the frame's `(SoundId, gain)` triggers once per frame through its `MessagePort`.
  4. Suspend the `AudioContext` on hidden. Wake Lock while playing.
  5. Mobile layout in portrait and landscape: no scrolling, `touch-action: none`, the score display and backglass off the flipper area.
- **Acceptance:**
  - Mobile emulation (Pixel 7, iPhone 14, both orientations): layout, PiP rectangles off the flipper area, a frame renders.
  - Settings persist across a reload.
  - The audio worklet smoke passes.
  - Coverage ≥ 60 %.
  - A device trial (U4) goes on the human checklist.

### W26 Web CI build parity and engine shells

- **Expected tier:** T1. **Labels:** `type:infra`, `area:ci`, `area:web`. **Runs:** 3 (macOS support adds one).
- **Depends on:** W20, W21.
- **Owned paths:** `.github/workflows/web.yml` (new: `changes`, `web-build`, `web-build-macos`, `web-parity`, `web-engines`), new `tests/spec_ci/tests/spec_web_yml.rs`, and `docs/DEVELOPMENT.md` (CI table, build hosts).
- **Start from:** [S9 run_integration.sh](S9/run_integration.sh), [S9 node/](S9/node), [S10 run_engines.sh](S10/run_engines.sh), [S10 versions.md](S10/versions.md), [S11 RESULTS §7-§8](S11/RESULTS.md).
- **Deliverables:**
  - jobs (i) and (v) and `web-build` from the CI design table, with the pinned tools
  - `web-build-macos` from the CI design table: an arm64 leg on `macos-15` and an Intel leg on `macos-26-intel`
- **Acceptance:**
  - `spec_web_yml` checks:
    - the job list and the path filters, exactly as in the CI design
    - no `continue-on-error` on gating steps
    - timeouts ≤ 45 min
    - `pr-checks` needs no web job
    - the pinned tool versions
    - `web-build-macos` runs `/bin/sh scripts/web/*.sh` on `macos-15` for web-path pull requests and on `macos-26-intel` for pushes to `main`, the weekly schedule and dispatch
  - The workflow is green, including both `web-build-macos` legs. Each leg's fetch, build, `check_module` and Node parity (20 runs per artifact against the committed goldens) pass.
  - The "Module sha256 vs Linux (report)" step records equal or different, with both sums, in the job summary. It never fails the job.
  - The first runs show the expected outputs of S11 §8:
    - `sha256 OK` and `smoke OK` lines with the pinned versions
    - `host darwin-arm64` or `darwin-x86_64`
    - `check_module` passes
    - Node parity 20/20 per artifact

    This closes U15. A parity failure on macOS is a defect; a sha256 difference alone is recorded.

### W27 Web CI browsers

- **Expected tier:** T2 (the Safari and iOS job). **Labels:** `type:infra`, `area:ci`, `area:web`. **Runs:** 2.
- **Depends on:** W25, W26.
- **Owned paths:** `.github/workflows/web.yml` (`web-browser`, `web-safari-firefox`) and `tests/spec_ci/tests/spec_web_yml.rs`.
- **Start from:** [S3 web-render-smoke.yml](S3/ci/web-render-smoke.yml), [S9 node/](S9/node), [S10 RESULTS §9](S10/RESULTS.md).
- **Deliverables:** jobs (ii) and (iii) from the CI design table.
- **Acceptance:**
  - The jobs are green.
  - The first runs close U1 (Safari and iOS), U3 (runner WebGPU) and U6 (worklet on Safari and Firefox), or record their fallback in `docs/DECISIONS.md`.
  - REQ-WEB-014's floors are checked against the current releases.

### W28 Release web zip

- **Expected tier:** T1. **Labels:** `type:infra`, `area:release`, `area:web`. **Runs:** 1.
- **Depends on:** W20.
- **Owned paths:** the `web-dist` job in `.github/workflows/release.yml`, and `tests/spec_ci`.
- **Start from:** [S9 build_web.sh](S9/build_web.sh).
- **Deliverables:** on `v*` tags and dispatch, attach `kilohertz-web-<version>.zip` (`dist/`, `serve.cjs`, a short readme) to the draft prerelease. Publishing stays P8 `release`.
- **Acceptance:** a dispatch run produces the zip; `spec_ci` is updated.

### W29 Docs

- **Expected tier:** T1. **Labels:** `type:docs`, `area:web`. **Runs:** 1.
- **Depends on:** W16, W19, W25.
- **Owned paths:**
  - `docs/THREAT_MODEL.md`: the text under "Threat model additions"
  - `docs/AUTHORING.md`, portability rules:
    - no `collectgarbage`
    - ≤ 100 nested C calls
    - 200 000 units per tick
    - do not depend on `pairs` order
    - math goes through `libm`
    - WAT and binary rules both run on the web
  - `README.md`: a web section with "How to run locally" from the issue
  - the web checks in `scripts/human_acceptance.py`, which regenerate `docs/HUMAN_ACCEPTANCE.md` (Gate A4 item 9)
- **Acceptance:** `scripts/human_acceptance.py --check` passes, and the docs spec tests pass.

## Mapping, dependencies and effort

**Mapping from the draft's subphases.**

| Draft subphase | Now |
|---|---|
| 9.0 | the issue's SPEC changes (P8 `spec`) |
| 9.1 | W01-W03 |
| 9.3 | W05 |
| 9.4 | W06, W07 |
| 9.5 | W04, W08-W10, Lua half of W11 |
| 9.6 | Wasm half of W11, W12-W16 |
| 9.8 | W17 |
| 9.9 | W18 |
| 9.10 | W19, W21 |
| 9.11 | W23-W25 |
| 9.12 | W22 |
| 9.13 | W20 |
| 9.14 | W26-W28 |
| 9.15 | W29 |

**Dependencies on existing work units.** These are real file and behaviour prerequisites:
- KP-047 changes Wasm init fuel, which the rewrite must match.
- KP-016, KP-017, KP-022, KP-035 and KP-052 change the render paths W17 touches.
- KP-048 and KP-049 change audio decoding.
- KP-001 fixes input ingestion.
- KP-021 adds camera keys.
- KP-004 makes the menu live.
- KP-034 puts the displays on the 3D backbox.

**Desktop fixes.** W01-W05 fix problems that exist today: macOS replays differ from Windows and Linux, and `pairs` and the string cache are not run-to-run deterministic (S8 F4, F5), a D9 violation. They could be attached to M2 ([#80](https://github.com/donfuldauer/kilohertz-pinball/issues/80)) with `phase:20-backlog` instead, if the owner wants them before the web work.

**Effort.** 59-78 implementer, tester and fix runs (macOS build-host support adds one run each to W20 and W26), plus one verifier run per unit (29) and CodeRabbit rounds: about 92-112 agent runs. The draft's 55-70 counted fewer, larger units.

**Optional follow-up.** A Fable re-audit of Lua 5.4.9 + KHZ for ILP32/LP64 differences, if W08-W10 run on Opus (S8 S8-8).

**Optional CR-011** (doc only): `InputEvent::timestamp_ns` is `performance.now()`-derived on the web. No breaking CR is needed.
