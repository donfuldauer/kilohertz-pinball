# Decisions: web-proposal (a browser build of Kilohertz Pinball)

Proposal only. It changes nothing in `docs/PLAN.md`, `docs/DECISIONS.md`, `docs/THREAT_MODEL.md`,
the CI workflows, `versions.toml`, `contracts/` or any code. Written on 2026-11-20 against `main` at
`995935f`. Line numbers refer to that commit.

Conventions used below:
- A path marked **(new)** is a proposed path that does not exist yet. Every other path exists at
  `995935f`.
- A claim tagged **(Ann)** is about the external web platform or a third-party crate. It is
  external and unverified, and is listed in section 11 "Assumptions to verify".
- "Verified" means checked against files in this repository. No build was run for this proposal.
  `cargo tree --offline` could not resolve the registry (`no matching package named arrayvec`),
  so dependency facts come from the `Cargo.toml` files and `Cargo.lock`.

## web-proposal — Browser (WebAssembly) build that plays the three production tables

**Decision:** PROPOSED, awaiting owner acceptance. Recommended if accepted: **Option B**, a thin web
shell. A new crate `pin_web` (new) sits over the shared crates (`pin_core`, `pin_physics`,
`pin_input`, `pin_assets`, `pin_sim`, `pin_audio`'s mixer and `pin_render`) and is compiled to
`wasm32-unknown-unknown`. It renders with WebGPU only and runs one main-thread fixed-step loop
(no Worker, so no COOP/COEP headers). Rules run from each table's committed `rules.wasm`, so there
is no Lua on the web. The web gate is same-build determinism, and native-to-web hashes are
report-only. Work starts with a de-risking spike (Subphase 9.0), and its go/no-go decides whether
the rest goes ahead.

**Rationale:**
- The core crates are already portable in source. `pin_core`, `pin_physics` and `pin_input` use
  no threads, wall clock or files (section 3).
- The blockers sit in four places: the vendored C Lua, wasmtime, SDL3, and the thread and
  blocking-GPU-init code in `pin_render` and `pin_app`. A shell-level split avoids all four
  without changing native behaviour.
- All three production tables ship a `rules.wasm` that CI proves equal to `rules.lua` on every tick.
- D9 already makes cross-platform hashes report-only.

**Alternatives:** Option A, a full port of `pin_app` behind cfg gates. Option C1, a replay viewer
only (kept as Option B's first milestone). Option C2, server-side streaming. Option D, an
Emscripten whole-app port. Per-area alternatives are also covered: a Worker with
SharedArrayBuffer, a WebGL2 fallback, Lua through Emscripten or a pure-Rust VM, and the browser's
JIT against an in-sim interpreter for rules. Sections 4 and 5 give each one with its reasons.

### Summary

**Recommendation.** Option B, delivered in two milestones:
- **M-web-1, replay viewer.** The web build loads a table and plays back a recording in a browser.
- **M-web-2, playable.** Keyboard, gamepad and touch input, audio and a reduced menu.

All three production tables (`apex_circuit`, `sky_deck`, `feature_lab`) run with `rules.wasm`.
Cabinet features stay native-only:
- multi-monitor (D19)
- DOF
- UDP telemetry
- SSF 7.1
- X-Arcade gamepad auto-detection
- hot reload
- the profiler

**Phases.** New Phase 9, "Browser Build", runs in Waves 10 to 13, plus an optional Wave 14.

| Subphase | What it does | Size | Sessions | Tier |
|---|---|---|---|---|
| 9.0 | Spike | M | 2–3 | T2 |
| 9.1 | Script-backend feature gates | M | 2 | T2 |
| 9.2 | `pin_render` web path | M–L | 2–3 | T2 |
| 9.3 | Frame loop and input | M | 2 | T2 |
| 9.4 | Web audio | M | 2 | T2 |
| 9.5 | Web rules backend and headless web replay | L | 3–4 | T2 |
| 9.6 | `pin_app` native-feature split | M | 2 | T2 |
| 9.7 | Web app integration | L | 3–4 | T2 |
| 9.8 | Web CI and release | M | 1–2 | T1 |

An automated **Gate W** follows the last wave. The whole phase takes roughly 35 to 45 agent runs,
counting Independent Testers, verifiers and fix rounds. No subphase needs T3: the D34 (b)
condition is not met.

**Top blockers per area.**
- **Render.** `pin_render` blocks on futures with `pollster::block_on` (adapter, device and error
  scopes) and uses `Instant` waits and presenter threads (`crates/pin_render/src/gpu/mod.rs:414`,
  `:430`, `crates/pin_render/src/post/chain.rs:458`, `crates/pin_render/src/gpu/surfaces.rs:114`,
  `:661`). The forward shader reads storage buffers in the fragment stage
  (`crates/pin_render/src/pbr/shaders/forward.wgsl:57-58`), which rules out WebGL2 unless it is
  reworked.
- **Threading.** The 1 kHz sim thread, its sleep-and-spin timer and its 8-tick catch-up cap
  (`crates/pin_app/src/orchestrator/mod.rs:113-130`, `:321-356`;
  `crates/pin_core/src/sim.rs:21`) become a per-frame batch on the main thread.
- **Lua.** The vendored `lua-src` build script has no `wasm32-unknown-unknown` arm and stops with
  "don't know how to build Lua" (`third_party/lua-src/src/lib.rs:137-192`).
- **wasmtime and the sandbox.** wasmtime cannot run in a browser (A11). Its fuel metering, stack
  limit and NaN canonicalisation must be rebuilt. The memory cap can be rebuilt exactly. The
  committed `rules.wasm` files declare memory with no maximum (section 4.5).
- **Audio.** SDL3 output becomes an AudioWorklet that runs `pin_audio`'s mixer. A user gesture is
  needed before audio starts, and 7.1 output is dropped.
- **Determinism.** The sim crates use `libm` and no FMA, as on native. glam and simba's `wide`
  pick different SIMD paths per target (A13), so native and web hashes may differ. The proposal
  reports that difference rather than gating on it.

**Top open questions** (section 9):
- Q1: whether to do this now, alongside Phase 8.
- Q3: WebGPU-only or a WebGL2 fallback.
- Q4: Wasm-only rules on the web, or Lua too.
- Q5: native-to-web hash parity, reported or required.
- Q7: whether mobile is in scope.
- Q2: hosting and headers.
- Q11: who maintains the web target.

**Biggest unverified assumptions:**
- A1: wgpu 30.0.1's WebGPU backend works with the pinned features.
- A2: browser WebGPU coverage.
- A6: `Instant` and threads fail on `wasm32-unknown-unknown`.
- A13: the size of the native-to-web float divergence.
- A19: an in-sim interpreter is fast enough, or else A11 plus instrumentation.
- A24: download size.

### 1. Goal and non-goals

**Goal.** A player opens a web page in a desktop browser, picks Apex Circuit, Sky Deck or Feature
Lab, and plays full games. In detail:
- **Mechanisms.** Flippers work, including upper flippers, Feature Lab's Banana flipper and its
  backbox vertical flipper. So do launch and plunger, start, action, and tilt and nudge.
- **Rules.** Scoring, modes, multiball and the XL and Glo balls behave exactly as the rules
  define them.
- **Displays and sound.** The backglass and the DMD or LCD show as picture-in-picture inside one
  canvas. Sound and music play.
- **Menu.** A reduced menu offers a table picker, input settings and display settings.

"Plays the three tables" is defined as:
- **Inputs.**
  - Keyboard: the `generic_keyboard` profile, `crates/pin_input/profiles/generic_keyboard.toml`.
    An X-Arcade in keyboard-encoder mode is a keyboard, so it works through
    `crates/pin_input/profiles/xarcade_keyboard.toml`.
  - Gamepad: the browser Gamepad API "standard" mapping onto the `generic_gamepad` profile
    (`crates/pin_input/profiles/generic_gamepad.toml`), with analog flipper, plunger and nudge axes
    (A17).
  - Touch: on-screen zones for the flippers, launch, start and action. These map onto the existing
    `CabinetInput` and `CabinetAxis` (`crates/pin_core/src/input.rs:14-31`, `:77-88`), so touch
    needs no contract change.
- **Targets.**
  - Current desktop Chrome and Edge, on WebGPU.
  - Desktop Firefox and Safari in versions where WebGPU is enabled (A2).
  - Mobile browsers are an open question (Q7) and not part of the v1 gate.
- **Automated acceptance.** Gate W (section 7).
- **Human acceptance.** Feel, input latency and audio, added to the Phase 8-style checklist.

**Scope table** (web v1):

| Feature | Web v1 | Evidence and reason |
|---|---|---|
| Multi-monitor and secondary displays, presenter threads (D19) | **Out.** One canvas; backglass and score display are PiP (single-monitor mode) | D19 `docs/PLAN.md:35`, thread table `:138`; single-monitor PiP planning is pure geometry in `crates/pin_render/src/topology/mod.rs:1-16`; presenter threads `crates/pin_render/src/gpu/surfaces.rs:100-120` |
| DOF bridge (D22, Subphase 3.5) | **Out** | A Windows .NET companion process started by `pin_app` (`crates/pin_app/src/dof/mod.rs:1-21`, `tools/dof_bridge/`); a web page cannot start processes |
| UDP telemetry (Appendix B) | **Out** | `std::net::UdpSocket` plus a sender thread (`crates/pin_audio/src/telemetry/sender.rs:8-10`, `:216`); browsers cannot send UDP. A WebSocket relay is possible later |
| X-Arcade | **Keyboard-encoder mode in**; gamepad-mode auto-detection **out**; manual profile choice is a stretch goal | Detection matches the SDL GUID (`crates/pin_input/src/profiles/mod.rs:83-85`), which browsers do not expose (A17) |
| SSF 7.1 exciters | **Out**; stereo only | 7.1 block assembly in `crates/pin_app/src/orchestrator/audio.rs:1-17`; `crates/pin_audio/src/ssf/` |
| Rumble | Stretch | `crates/pin_app/src/platform/rumble.rs`; Gamepad `vibrationActuator` is not on every browser (A17) |
| Accelerometer nudge | **Out** in v1 (mobile stretch goal) | `crates/pin_input/src/calibration/nudge.rs` |
| Lua hot reload, Wasm hot swap | **Out** | `notify` watcher and file reads (`crates/pin_script/src/lua/reload.rs:1-15`); developer-only events (`docs/PLAN.md:155`); no Lua on the web (4.4) |
| Dev tools: latency profiler HUD (7.1) | **Out**; a minimal frame-time and tick-time overlay is in | `crates/pin_app/src/profiler/` is SDL-clock and GPU-timestamp based |
| Debug wireframe view | Stretch | `crates/pin_render/src/debug/` (renderer feature) |
| `pin_tablegen` / `pin_assetgen` CLIs | **Out** at run time | Bundles are built natively by `tables/*/build.sh` and `scripts/package.py:378-395`. The libraries are compiled in only where the runtime needs them: the clearance validator (`crates/pin_app/src/orchestrator/table.rs:19`) and the DMD fonts (`crates/pin_render/Cargo.toml:26-28`) |
| Ray-query reflections (5.4, D33) | **Out** | WebGPU has no ray queries (A3); the probe path (D18) is the baseline and is selected at run time already |
| CI gates | **In**: a new web workflow plus Gate W. A1, A2, the soaks and `replay-hash` stay native gates | Section 7 and section 10.4 |
| Persisted high scores (`persist` slots) | **Out** in v1 | Native does not persist them either: `persist` is in-memory only in `crates/pin_sim/src/sim/mod.rs:674-680`, and no persist file I/O exists in `crates/pin_app/src/` |
| User-supplied bundles (file picker) | **Out** in v1 (Q10) | Untrusted-table surface (section 4.5) |
| Table menu (4.4) | **In**, reduced | `crates/pin_app/src/ui/` is GPU-free (`crates/pin_app/src/ui/mod.rs:5-7`) but sits in an SDL crate (section 3) |

### 2. What the engine depends on today

- **Workspace.** Members are `crates/*` plus the guest SDK (`Cargo.toml:5`). Edition 2024, MSRV
  1.97 (`Cargo.toml:10-11`).
- **Toolchain.** Pinned to rustc 1.97.0 (`versions.toml:9-13`).
- **Platform.** `sdl3 =0.22.1` with `build-from-source-static` and `raw-window-handle`
  (`Cargo.toml:23`, `versions.toml:16-19`).
- **Graphics.** `wgpu =30.0.1` with default features: dx12, metal, vulkan, gles, wgsl and more
  (`Cargo.toml:28`, `versions.toml:22-23`, `:53`). Also `pollster =1.0.1`, `egui` and `egui-wgpu
  =0.37.0` (`Cargo.toml:51-52`), and `naga =30.0.1` for shader-validation tests (`Cargo.toml:69`).
  - `Cargo.lock` already resolves wgpu's web dependencies: `wasm-bindgen 0.2.130`,
    `js-sys 0.3.107`, `web-sys 0.3.107`, `wasm-bindgen-futures 0.4.80`.
- **Physics.** `parry3d =0.31.1`, `glam =0.33.12`, `glamx =0.3.1` and `simba =0.10.2`
  (`Cargo.toml:32-35`). `simba` pulls in `wide` (`Cargo.lock`).
- **Scripting.**
  - `mlua =0.12.2` with `lua54` and `vendored` (`Cargo.toml:38`).
  - Lua 5.4.9 comes from a patched `lua-src 551.0.2` (`Cargo.toml:89-90`), with 28 KHZ patches
    (`third_party/lua-src/KHZ_PATCHES.md`).
  - `wasmtime =50.0.1` (`Cargo.toml:39`). It uses `wasmparser 0.261.0` and `wasm-encoder
    0.261.0` (`Cargo.lock`).
- **Assets.**
  - `zip =9.5.1`, deflate only (`Cargo.toml:57`). Its lock entry depends on `getrandom 0.4.3`,
    `nix`, `libc` and `windows-sys`.
  - Also `flate2` with `zlib-rs`, `gltf` (no `import`), `image` (png and jpeg) and `serde_json`.
- **Audio.** `hound`, `lewton` (patched copy in `third_party/lewton`, `Cargo.toml:91`) and `ogg`.

The crate graph is where the blockers bite:
- `pin_sim` depends on `pin_script` (`crates/pin_sim/Cargo.toml:18`).
- `pin_script` always builds `mlua`, `notify` and `wasmtime` (`crates/pin_script/Cargo.toml:10-20`).
- `pin_app` always builds `sdl3` (`crates/pin_app/Cargo.toml:14`).
- `pin_render` depends on `pin_assets` and `pin_assetgen` (`crates/pin_render/Cargo.toml:26-34`).

So no part of the playable engine compiles to `wasm32-unknown-unknown` until `pin_script`'s native
backends become optional.

### 3. Per-crate feasibility

Effort: S is under one agent session, M is one or two, L is three or more. "Confidence" separates
what was verified in the repository from what needs a build or the web platform (unverified).

| Crate | Depends on (evidence) | What blocks or complicates `wasm32` in a browser | Proposed change | Effort | Confidence |
|---|---|---|---|---|---|
| `pin_core` | `serde =1.0.229`, `toml =1.2.1`, `arrayvec =0.7.8`, `rand_pcg =0.10.2`, `rand_core =0.10.1` (`crates/pin_core/Cargo.toml:10-15`, `Cargo.toml:42-46`). Source uses only `core` atomics and `std::sync::Arc` (`crates/pin_core/src/triple_buffer.rs:14-16`, `crates/pin_core/src/spsc.rs:22-25`); no thread, time or file use | Nothing blocks compilation. `TableScriptBackend: Send` (`crates/pin_core/src/script.rs:250`) and `SimStep: Send` (`crates/pin_core/src/sim.rs:24`) clash with JS handles, which are `!Send`; there is precedent for a single-thread `unsafe impl Send` (`crates/pin_script/src/lua/mod.rs:133`). `MAX_CATCH_UP_TICKS = 8` (`crates/pin_core/src/sim.rs:21`) is below one frame of ticks. `InputEvent` documents SDL's clock (`crates/pin_core/src/input.rs:121-125`) | No contract change needed: the web shell keeps its own per-frame cap and uses a single-thread wrapper. Optional additive doc CR-010 (section 10.6) | S | Source verified; build unverified |
| `pin_physics` | `parry3d =0.31.1` (via `glamx`, `simba` → `wide`), `glam =0.33.12`, `arrayvec`, `libm =0.2.16` (`crates/pin_physics/Cargo.toml:10-15`). No thread, time or file use in `crates/pin_physics/src/`. Std transcendentals are banned (`crates/pin_physics/clippy.toml`); no `mul_add` in `crates/pin_physics/src/` | Nothing known blocks compilation. Determinism only: glam and `wide` choose SSE2 on x86_64 and scalar or `simd128` on wasm32 (A13); parry3d may call std math internally (A13) | None to compile. Subphase 9.0 measures bit equality against native | S | Dependencies verified; build and bit equality unverified |
| `pin_input` | `pin_core`, `arrayvec`, `libm`, `toml` (`crates/pin_input/Cargo.toml:10-14`). Pure logic that uses SDL numbering without SDL (`crates/pin_input/src/profiles/mod.rs:7`): `RawInput::Key(u32)` is an SDL scancode (`:300-307`), `SdlGuid` (`:83-85`) | Nothing blocks compilation. The web shell needs a table from `KeyboardEvent.code` to USB HID usage (equal to the SDL scancode) (A26), and from the Gamepad standard mapping to `GamepadButton` and `GamepadAxis`. There is no SDL GUID on the web, so X-Arcade gamepad detection goes | None in the crate. The mapping tables live in `pin_web` (new) | S | Verified; mapping unverified |
| `pin_script` (Lua backend) | `mlua =0.12.2` with `lua54`, `vendored` → `mlua-sys 0.13.0` → patched `lua-src` (`Cargo.toml:38`, `:89-90`); `notify =8.2.2` for hot reload only (`crates/pin_script/src/lua/reload.rs:10`) | **Hard compile blocker.** `lua-src`'s build script matches linux, bsd, illumos, solaris, apple, windows, `*emscripten` and `*wasi*`, and anything else fails with "don't know how to build Lua for {target}" (`third_party/lua-src/src/lib.rs:137-192`). The Lua C code also needs libc and `setjmp`/`longjmp`, and the KHZ thread-local cost counter (`third_party/lua-src/lua-5.4.9/lstring.h:50-55`). Support for the target in `mlua` is unknown (A10) | Add a `lua` cargo feature, default on, so web builds leave it out. Lua on the web is optional Subphase 9.10 only (section 4.4) | L if pursued | Build script verified; `mlua` support unverified |
| `pin_script` (Wasm backend) | `wasmtime =50.0.1` with `reexport-wasmparser`, `wat =1.263.0` (`crates/pin_script/Cargo.toml:16-20`). The pre-scan imports `wasmtime::wasmparser` (`crates/pin_script/src/wasm/limits.rs:18`) | **Hard blocker:** wasmtime cannot execute inside a browser (A11). The sandbox semantics live in `crates/pin_script/src/wasm/mod.rs:51-162`, which the web must reproduce (section 4.5) | Add a `wasmtime` feature, default on. Import `wasmparser =0.261.0` directly so the pre-scan is engine-neutral. Split the engine-neutral host logic out of `crates/pin_script/src/wasm/host.rs`. Add a web backend (9.5) | L | Verified; backend choice unverified (A11, A19) |
| `pin_guest_sdk` (extra crate) | No dependencies (`crates/pin_script/guest-sdk/Cargo.toml:12`); `no_std` | None. It already targets `wasm32-unknown-unknown` (`tables/apex_circuit/rules-wasm/rebuild.sh`) | None | — | Verified |
| `pin_assets` | `zip =9.5.1`, `flate2` (`zlib-rs`), `crc32fast`, `glam`, `serde_json`, `gltf =1.4.1`, `image =0.25.6`, `pin_physics` (`crates/pin_assets/Cargo.toml:10-23`) | The folder VFS uses `std::fs` and `canonicalize` (`crates/pin_assets/src/vfs/mod.rs:69-170`), which the web does not need; the in-memory `Vfs::from_zip_bytes` exists (`:101`). `zip` pulls in `getrandom 0.4.3`, which may need a `wasm_js` backend flag (A14). The glTF loader contains panics with `catch_unwind` (`crates/pin_assets/src/gltf/mod.rs:599`), which does nothing under `panic=abort` (A7) | The web build uses `from_zip_bytes` only, with smaller `ZipLimits` (section 4.7). Configure `getrandom` if 9.0 finds that it is needed | S–M | Verified; build unverified |
| `pin_tablegen` | `pin_physics`, `pin_assets`, `glam`, `libm`, `serde`, `toml` (`crates/pin_tablegen/Cargo.toml:14-27`). The CLI uses `std::fs` (`crates/pin_tablegen/src/layout/mod.rs`, `crates/pin_tablegen/src/layout/cli.rs`, `crates/pin_tablegen/src/backbox/mod.rs`, `crates/pin_tablegen/src/main.rs`) | At run time only the clearance validator is used, in the load path (`crates/pin_app/src/orchestrator/table.rs:19`). `std::fs` compiles on wasm32 but fails at run time (A6) | The library is used as is; the CLI stays native | S | Verified; build unverified |
| `pin_assetgen` | `hound =3.5.1`, `libm`, `flate2`, `crc32fast`, `serde`, `toml` (`crates/pin_assetgen/Cargo.toml:14-21`); its library writes files (`crates/pin_assetgen/src/lib.rs`) | At run time only `pin_assetgen::fonts` is used, by `pin_render` displays (`crates/pin_render/Cargo.toml:26-28`) | None. Generation stays a native build step | S | Verified; build unverified |
| `pin_sim` | `pin_physics`, `pin_input`, `pin_script`, `pin_assets`, `glam`, `arrayvec`, `libm` (`crates/pin_sim/Cargo.toml:14-22`) | **Blocked through `pin_script`.** The `Script` enum names the concrete Lua and Wasm backends (`crates/pin_sim/src/sim/mod.rs:166-169`). `SimConfig` exposes `LuaConfig` and `WasmConfig` (`:56-65`), and `build_script` constructs them (`crates/pin_sim/src/load/mod.rs:170-195`). `Simulation::load(&Path)` (`:515`) reads folders, while `load_from_vfs` (`:541`) does not. The soak runner uses `std::thread::scope` and `Instant` (`crates/pin_sim/src/autoplay/soak.rs:34`, `:486`), but only natively. The allocation guard is thread-local and works on one thread (`crates/pin_sim/src/alloc_guard/mod.rs:31-36`) | Feature-gate the backends (9.1) and add a web-backend variant (9.5). The web build uses `load_from_vfs` | M | Verified; build unverified |
| `pin_render` | `wgpu =30.0.1` (default features), `pollster`, `bytemuck`, `libm`, `egui`, `egui-wgpu`, `pin_assetgen`, `pin_assets`, `serde`, `toml` (`crates/pin_render/Cargo.toml:15-34`) | `pollster::block_on` for adapter and device (`crates/pin_render/src/gpu/mod.rs:414`, `:430`, `:471`, `:501`) and for error scopes (`crates/pin_render/src/post/chain.rs:458`, `crates/pin_render/src/gpu/surfaces.rs:661`, `crates/pin_render/src/raytrace/source.rs:355`). Blocking device poll and `Instant` waits (`crates/pin_render/src/gpu/mod.rs:307`, `:355`; `crates/pin_render/src/gpu/surfaces.rs:481`). Presenter threads (`crates/pin_render/src/gpu/surfaces.rs:114`). Fragment-stage storage buffers (`crates/pin_render/src/pbr/shaders/forward.wgsl:57-58`): fine on WebGPU, not on WebGL2. Surfaces come from SDL raw window handles (`Cargo.toml:25`) | Subphase 9.2: an async init path (native keeps a `pollster` wrapper), a canvas surface, no presenter threads on the web and no `Instant` waits. WebGL2 is optional (9.9) | M–L | Verified; WebGPU behaviour unverified (A1, A3–A5) |
| `pin_audio` | `hound`, `lewton =0.10.2` (patched), `ogg =0.8.0`, `libm`, `arrayvec`; `sdl3` is optional behind the default feature `sdl` (`crates/pin_audio/Cargo.toml:10-24`) | Without `sdl` the mixer is SDL-free, and `render` never allocates, locks or blocks (`crates/pin_audio/src/mixer/mod.rs:8-10`, `:44-45`). Telemetry uses UDP and a thread (`crates/pin_audio/src/telemetry/sender.rs:8-10`). The decoders run under `catch_unwind` (`crates/pin_audio/src/mixer/decode.rs:321`), ineffective under `panic=abort` (A7). The decode budget is 512 MiB (`crates/pin_audio/src/mixer/decode.rs:132`) | Use it with `default-features = false`, as `pin_app` already does (`crates/pin_app/Cargo.toml:28`). Output goes through an AudioWorklet (9.4). Telemetry and SSF are not used on the web. Add a web decode budget | S–M | Verified; build unverified |
| `pin_app` | `sdl3 =0.22.1` (unconditional, `crates/pin_app/Cargo.toml:14`), `pin_render`, `pin_input`, `pin_assets`, `pin_sim`, `pin_tablegen`, `pin_audio` (no `sdl`), `egui`, `image`, `serde`, `toml`, `libm` (`:10-30`) | SDL3 event pump and windows (`crates/pin_app/src/platform/mod.rs:1-20`). Sim, render, audio and I/O threads, plus the sleep-and-spin timer (`crates/pin_app/src/orchestrator/mod.rs:321-356`). `Instant` clock (`crates/pin_app/src/orchestrator/threads.rs:36-44`). DOF launcher (`crates/pin_app/src/dof/`). `config.toml` through `std::fs` (`crates/pin_app/src/ui/config.rs:625-785`). Building SDL3 from source for `wasm32-unknown-unknown` is not supported (A10) | Not ported. Optional 9.6 makes `sdl3` a default feature so `pin_web` can reuse the SDL-free modules. These are `crates/pin_app/src/orchestrator/view.rs` (`TableRenderer`), `crates/pin_app/src/orchestrator/table.rs` (`TableSim`; it spawns a loader thread at `:383`), the `AudioEngine` part of `crates/pin_app/src/orchestrator/audio.rs`, and `crates/pin_app/src/ui/menu.rs` and `crates/pin_app/src/ui/view.rs` | M (9.6) | Verified |
| `pin_testmain` (extra crate) | `std` only, dev-only, for `harness = false` tests (`crates/pin_testmain/Cargo.toml:10-12`) | Not part of any web artifact | None | — | Verified |
| `third_party/lewton` (extra) | Patched `lewton 0.10.2` (`third_party/lewton/KHZ_PATCHES.md`) | Pure Rust; expected to compile (A6) | None | S | Unverified |
| `tools/dof_bridge` (extra) | C#/.NET, Windows only (PLAN Subphase 3.5, `docs/PLAN.md:614-620`) | Not applicable | Out of scope | — | Verified |
| `pin_web` (new) and `pin_web_audio` (new) | Planned: `pin_core`, `pin_input`, `pin_sim`, `pin_assets`, `pin_render`, `pin_audio` (no `sdl`), optionally `pin_app` (no default features); wasm-only `wasm-bindgen`, `js-sys`, `web-sys`, `wasm-bindgen-futures` | — | The web shell: frame loop, input, fetch, storage, app; the audio worklet module | L | Proposed |

### 4. Cross-cutting analysis

#### 4.1 Rendering (wgpu)

**Backends.** WebGPU and WebGL2 compared:

| | WebGPU (recommended for v1) | WebGL2 fallback (optional 9.9) |
|---|---|---|
| wgpu backend | `webgpu` feature (A1) | `webgl` feature; GL backend over WebGL2 (A1) |
| Clustered lights: `var<storage, read>` in the fragment stage (`crates/pin_render/src/pbr/shaders/forward.wgsl:57-58`) | Supported (A3) | Not supported (A3); the cluster lists must move to textures or uniform arrays |
| Limits: `Limits::downlevel_defaults()` (`crates/pin_render/src/gpu/mod.rs:437`, `:487`) | Within WebGPU's guaranteed limits (A3) | Needs `downlevel_webgl2_defaults` (A3) |
| HDR `Rgba16Float` with MSAA 4× (`crates/pin_render/src/post/mod.rs:19`, `:116`) | Renderable, multisampled, blendable (A4) | Needs float colour-buffer extensions; MSAA on float targets doubtful (A4) |
| Compute | Only the ray-query pass (`crates/pin_render/src/raytrace/shaders/trace.wgsl:336`), which is off on the web | No compute |
| Shader path | The browser's own WGSL compiler compiles the WGSL directly (A5) | naga translates to GLSL ES; the root `naga` pin lacks `glsl-out` (`Cargo.toml:69`) |

**Ray queries (D33).** The trace shader uses wgpu's `enable wgpu_ray_query;` extension
(`crates/pin_render/src/raytrace/shaders/trace.wgsl:2`). It is created only when the adapter
reports `EXPERIMENTAL_RAY_QUERY` (`crates/pin_render/src/raytrace/mod.rs:120`). WebGPU has no ray
queries (A3), so the existing run-time fallback to probes (D18, `docs/PLAN.md:34`) applies and no
code change is needed.

**Features and limits.**
- The device requests only the timestamp features the adapter offers, and those are optional
  (`crates/pin_render/src/gpu/mod.rs:393`, `:435`), plus downlevel limits.
- The timestamp queries feed the native GPU frame-time profiler, which is out on the web.

**Texture formats.**
- The renderer uses `Rgba8Unorm`, `Rgba16Float`, `Depth32Float` and an sRGB surface format.
- The post chain can write a non-sRGB 8-bit output and apply the sRGB curve in the shader
  (`crates/pin_render/src/post/chain.rs:300-318`). Surfaces already handle sRGB twin views through
  `view_formats` (`crates/pin_render/src/gpu/surfaces.rs:691-803`).
- That fits WebGPU canvases, which offer `bgra8unorm` or `rgba8unorm` and no sRGB canvas format
  (A4).

**Shader compatibility.**
- Shaders are validated by `naga` 30.0.1 and translated to SPIR-V, MSL and HLSL in tests
  (`Cargo.toml:69`).
- Browsers use their own WGSL front ends (A5), so Subphase 9.2 adds a check that every module
  compiles in a browser.
- The shaders use no `binding_array`, `f16` or push constants. A search of
  `crates/pin_render/src/*/shaders/*.wgsl` found only `textureSampleLevel` and storage textures in
  `crates/pin_render/src/raytrace/shaders/trace.wgsl`.

**Blocking calls.** WebGPU's adapter, device, error-scope and buffer-map calls are asynchronous,
and the browser main thread cannot block (A5).
- `pin_render` blocks with `pollster` in context creation and in pipeline and surface setup (sites
  in the section 3 table).
- It also blocks with `device.poll(PollType::Wait)` (`crates/pin_render/src/gpu/mod.rs:307`).

Subphase 9.2 adds async variants. Native keeps its synchronous API by wrapping them in `pollster`.
On the web, frame-path error scopes are replaced by `on_uncaptured_error` logging, or by an async
pop that does not stall the frame.

**Surfaces and presentation.**
- One canvas surface. The display topology's single-monitor plan already places the backglass and
  score display as PiP rectangles (`crates/pin_render/src/topology/mod.rs:8-14`).
- `choose_present_mode` asks for a presenter thread only for secondary roles that are offered
  `Fifo` only (`crates/pin_render/src/gpu/mod.rs:86-104`). Those roles do not exist with one
  canvas.

**egui-wgpu** works on the web (A1). Menu input comes from DOM pointer and key events.

**Recommendation.** WebGPU only for v1, with a clear "this browser lacks WebGPU" page. WebGL2 is
optional Subphase 9.9, only if the owner wants the wider reach (Q3).

#### 4.2 Windowing and input (SDL3 compared with `web-sys`)

SDL3 owns windows, the event pump, gamepads and audio natively (`docs/PLAN.md:136`;
`crates/pin_app/src/platform/mod.rs:1-20`). SDL3 cannot be used from `wasm32-unknown-unknown`
(A10). The web shell uses `web-sys` DOM events on a canvas instead (winit is not needed; one canvas
and DOM listeners are enough).

| Native (SDL3) | Web |
|---|---|
| Display enumeration and windows (`crates/pin_app/src/platform/display.rs`) | One canvas, single-monitor PiP plan (`crates/pin_render/src/topology/mod.rs`), Fullscreen API on a user gesture |
| Keyboard by scancode (`crates/pin_app/src/platform/keymap.rs:1-30`; profiles use `key:<SDL scancode>`, e.g. `crates/pin_input/profiles/generic_keyboard.toml`) | `KeyboardEvent.code` → USB HID usage, which equals the SDL scancode, by a static table (A26); layout independent like native |
| SDL gamepad buttons and axes | Gamepad API "standard" mapping, polled once per animation frame (A17); `Gamepad.timestamp` places changes on ticks if it is precise enough (A17) |
| SDL GUID → X-Arcade preset (`crates/pin_input/src/profiles/mod.rs:83-85`) | `Gamepad.id` string only; auto-detection is out and a manual profile choice is a stretch goal |
| Event timestamps from `SDL_GetTicksNS` (`crates/pin_core/src/input.rs:121-125`) | `event.timeStamp` on the `performance.now()` timeline (A15), converted to `u64` ns |
| No touch today (`InputEventKind` has `Button` and `Axis` only, `crates/pin_core/src/input.rs:103-119`) | New touch zones mapped to `CabinetInput` and `CabinetAxis` in `pin_web`; no contract change |
| Rumble (`crates/pin_app/src/platform/rumble.rs`) | `vibrationActuator` where it exists (A17); stretch goal |
| Menu toggle (`Esc`, controller Menu) | The same keys and button; pointer events go to egui |

`pin_input`'s abstractions map cleanly. They already speak SDL numbering without depending on SDL
(`crates/pin_input/src/profiles/mod.rs:7`). The `FlipperStagingRouter`, the plunger filters and the
calibration code are pure functions of timestamped inputs.

#### 4.3 Threading, the 1 kHz loop and the triple buffer

**Native today.**
- The main thread pumps SDL.
- A dedicated sim thread runs `step_1ms` with elevated priority, a sleep-and-spin timer and at most
  8 catch-up ticks. A render thread reads the latest snapshot through a lock-free triple buffer
  (`docs/PLAN.md:134-148`; D6, D7 at `docs/PLAN.md:22-23`).
- `SimClock::advance` is a pure accumulator: it takes `now_ns` as a parameter and caps at
  `MAX_CATCH_UP_TICKS` (`crates/pin_app/src/orchestrator/mod.rs:108-130`).
- `SimLoop::run_ticks` runs ticks back to back without allocating, delivering inputs with
  `timestamp_ns <= tick end` (`crates/pin_app/src/orchestrator/mod.rs:213-240`).

**Web option (i): main-thread fixed-step accumulator (recommended).**
- **Loop.** On each `requestAnimationFrame` callback at time `t`, the shell computes the due ticks
  from `t - epoch` and runs them back to back. They go through the same `SimStep::step_1ms` and the
  same `SimLoop` logic, so the tick order (`docs/PLAN.md:142-148`) is unchanged. The shell then
  reads the latest snapshot and renders it.
- **Ticks per frame.** About 16 to 17 at 60 Hz and 8 to 9 at 120 Hz. The native cap of 8 is far
  below one 60 Hz frame. The shell therefore uses its own cap, `WEB_MAX_TICKS_PER_FRAME` (proposed
  50, tuned in 9.3). Time beyond the cap is dropped, as native does with `SimLoop::skip_ticks`
  (`crates/pin_app/src/orchestrator/mod.rs:206-210`).
- **Cost.**
  - Native takes about 5 µs a tick (`docs/RUN_LOG.md:341`). The budget is p99.9 under 0.35 ms a
    tick (`docs/PLAN.md:417`).
  - If wasm runs at 1 to 3 times native cost (A27), a 60 Hz frame spends roughly 0.1 to 0.3 ms
    simulating. The worst case, 17 ticks at the 0.35 ms budget, is about 6 ms, which still fits in
    a 16.7 ms frame.
- **Input latency.**
  - Events with a browser timestamp (keyboard, pointer, touch) still land on the exact 1 ms tick
    they belong to, because `SimLoop` places inputs by timestamp. The physics response is
    tick-accurate.
  - What changes is when the result becomes visible: at the next frame, plus the browser's
    compositor delay (A15).
  - An event dispatched while a frame's batch is running carries a timestamp in the already
    simulated past. It is applied one tick late, so up to one frame late in wall time.
  - Gamepads are polled once a frame, so their changes are quantised to the frame unless
    `Gamepad.timestamp` is precise (A17).
  - The native 7.1 metric (≤ 1.5 ms from SDL timestamp to tick, `docs/PLAN.md:785`) becomes "tick
    placement error". It should be 0 ticks for events delivered before the frame. The frame and
    compositor latency is reported separately.
- **Pausing.** On `visibilitychange` to hidden, simulated time freezes. On resume a fresh epoch is
  taken.
- **D5 and rule 4.**
  - `step_1ms` is unchanged, and the batch loop is allocation-free, as `SimLoop` is today.
  - DOM listeners push `Copy` `InputEvent`s into the fixed-capacity `pin_core::spsc` ring with no
    Rust allocation. The listener closures are created once at start-up.
  - Nothing takes a lock. On a single-threaded wasm build `std::sync::Mutex` is never contended
    (A6).
  - There is no file I/O on the web at all.
  - The panicking-allocator guard (`crates/pin_sim/src/alloc_guard/mod.rs`) is installed in a Node
    build of the web sim and checked over 60 000 ticks, mirroring gate item 6
    (`docs/PLAN.md:228`).
  - The JS heap is outside the guard. The design keeps the per-tick path free of JS allocation (no
    strings, no per-call closures), and review checks this.
- **Triple buffer.** Kept unchanged, with writer and reader both on the main thread.
  `TripleBuffer` and `spsc` use only `core` atomics and `Arc`, which compile and work on a
  single-threaded `wasm32` build (A6). Keeping it leaves the render code identical, and lets a
  later Worker design swap in a shared-memory buffer.
- **Hosting.** No COOP/COEP headers are needed.
- **Stack.** The native sim runs on a 16 MiB stack thread (`crates/pin_sim/src/cli/mod.rs:83`,
  `crates/pin_app/src/orchestrator/table.rs:372-386`). On wasm32 the Rust stack is a region of
  linear memory set at link time (A28). Subphase 9.0 measures what the sim needs.

**Web option (ii): Worker with SharedArrayBuffer and Atomics.**
- **How it works.** The sim runs in a dedicated Worker with a real 1 kHz loop, sleeping with
  `Atomics.wait` timeouts (A9). Inputs arrive through a SharedArrayBuffer ring. Snapshots are
  written into a SharedArrayBuffer triple buffer that the main-thread render instance reads.
- **Thread model.** Closer to native D6. It is immune to main-thread jank (layout, GC, egui).
- **Costs.**
  - SharedArrayBuffer requires cross-origin isolation, so the host must send
    `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp` (or
    `credentialless`). Static hosts such as GitHub Pages cannot set them (A9).
  - Rust threads over shared wasm memory need a nightly `-Z build-std` with `+atomics` (A8), which
    the stable 1.97.0 pin (`versions.toml:12`) rules out. So the Worker and the main thread would be
    two separate single-threaded instances. Rust's `TripleBuffer` (an `Arc` inside one linear
    memory) cannot span them; it would be a byte-level buffer in the SharedArrayBuffer.
  - `FrameStateSnapshot` is `repr(Rust)` (`crates/pin_core/src/snapshot.rs:86-87`; it is under
    8 KiB, `:223`). Copying it as bytes therefore needs either the same binary instantiated twice or
    a stable layout. A stable layout is a breaking `pin_core` change (D4).
  - The rules backend, the audio triggers and the bundle bytes all have to cross the Worker
    boundary. Spinning in a Worker costs battery.

**Recommendation.** Option (i) for v1. Option (ii) is optional Subphase 9.12, used only if 9.7's
measurements show frame-loop jank causing dropped ticks or visible input lag.

**What happens to code that does not run on `wasm32-unknown-unknown` (A6).**

| Use | Sites | Web handling |
|---|---|---|
| `std::thread` | presenter threads `crates/pin_render/src/gpu/surfaces.rs:114`; app threads `crates/pin_app/src/orchestrator/threads.rs`; loader thread `crates/pin_app/src/orchestrator/table.rs:383`; soak `crates/pin_sim/src/autoplay/soak.rs:486`; telemetry `crates/pin_audio/src/telemetry/sender.rs:216` | Not on any web path. Compiled out (`cfg` or feature) or never called |
| `std::time::Instant` | `crates/pin_render/src/gpu/mod.rs:355`, `crates/pin_render/src/gpu/surfaces.rs:481`, `crates/pin_app/src/orchestrator/threads.rs:36-44`, `crates/pin_app/src/orchestrator/handoff.rs:35`, `crates/pin_script/src/lua/reload.rs:121`, `crates/pin_sim/src/autoplay/soak.rs:34` | `Instant::now()` panics on this target (A6). The shell's clock is `performance.now()`; render waits become async (9.2). The sim has no wall-clock use: time is simulated (D8) |
| `std::sync::Mutex` | `crates/pin_render/src/gpu/mod.rs:268-270`, `crates/pin_render/src/gpu/surfaces.rs:26`, `crates/pin_app/src/orchestrator/audio.rs:27` | Uncontended on one thread (A6); not in `step_1ms` |
| `std::fs`, `std::net` | VFS folder mode, CLIs, `config.toml`, UDP telemetry | Compile but fail at run time (A6). The web uses in-memory bundles, browser storage and no UDP |

#### 4.4 Lua

| Route | What it takes | Keeps the 28 KHZ patches (budget, fixed seeds, string caps) | Fits the wasm-bindgen and WebGPU build | Effort | Verdict |
|---|---|---|---|---|---|
| (a) `wasm32-unknown-unknown` with a C toolchain | A new `wasm32-unknown-unknown` arm in the vendored `lua-src` build script (none today: `third_party/lua-src/src/lib.rs:137-192`). A libc for wasm (for example a wasi-libc sysroot) and clang targeting wasm. `setjmp`/`longjmp` through Wasm exception handling, as the WASI arm does with `-wasm-enable-sjlj` (`:184-189`) (A25). `_Thread_local` (`third_party/lua-src/lua-5.4.9/lstring.h:50-55`) on a non-threaded target. `mlua`/`mlua-sys` support for the target (A10). Rust is `panic=abort` there (A7) | Yes (same C) | Yes | L | Possible later (9.10). It edits the vendored Lua, a security boundary (D34 (a), `docs/PLAN.md:182`) |
| (b) `wasm32-unknown-emscripten` | `lua-src` has an Emscripten arm (`third_party/lua-src/src/lib.rs:169-173`). The rest of the app cannot follow, because wasm-bindgen and `web-sys` (wgpu's web backend) do not support Emscripten (A10). That leaves a separate Lua module bridged per call through JS, which means rewriting the `mlua` host bridge (`crates/pin_script/src/lua/bridge.rs`, `crates/pin_script/src/lua/api.rs`) against the C API | Yes | Only as a second module | L+ | Not recommended |
| (c) Pure-Rust Lua VM | A new VM (A10) without the KHZ patches: no fixed string-hash seed (patch 1), no reproducible sort pivot (2), a different cost model and string limits (3–25) (`third_party/lua-src/KHZ_PATCHES.md`). `pairs` order, error texts and budget faults would differ from native, so replays diverge. It is also a new attack surface | No | Yes | L | Rejected |
| (d) Wasm rules only on the web | Run each table's committed `rules.wasm` (`tables/apex_circuit/rules.wasm` 20 603 B, `tables/sky_deck/rules.wasm` 11 811 B, `tables/feature_lab/rules.wasm` 15 529 B). It is proven equal to `rules.lua` per tick in CI: queue bytes, PCG state, scores (`.github/workflows/rules-wasm.yml:130-134`; `docs/decisions/2026-11-19-6.3.md`, "the simulations' own command queues are compared") | Not applicable | Yes | S (packaging) | **Recommended for v1** |

**What route (d) means.**
- Every `table.toml` selects `rules.lua` (`tables/apex_circuit/table.toml:12`,
  `tables/sky_deck/table.toml:17`, `tables/feature_lab/table.toml:16`), and Lua stays the native
  default (D10; `docs/decisions/2026-11-19-6.3.md`, "Lua stays the default backend").
- The web build therefore ships web bundles whose `table.toml` names `rules.wasm`. This is the same
  rewrite the parity tests do (`crates/pin_app/tests/wasm_ports_support/mod.rs:76`), and
  `scripts/package.py:378-395` already requires `rules.wasm` in the ported bundles.
- **What the web cannot run:**
  - the grey-box table, which has no `rules.wasm` (`tables/greybox/`)
  - any Lua-only third-party table
  - hot reload
- **Why the two rule files cannot drift.** Rules are frozen after Gate A2 (`docs/PLAN.md:769`).
  The rules-wasm workflow also re-runs parity on any `tables/*/rules.lua` change
  (`.github/workflows/rules-wasm.yml:13-16`).
- **Replays.** A rewritten bundle has a different `table_hash`. Replays check it
  (`ReplayError::TableMismatch`, `crates/pin_sim/src/replay/mod.rs:112`), so native Lua recordings
  do not load on the web bundle. Native comparisons use the same Wasm bundle under wasmtime.

#### 4.5 Wasm rules without wasmtime: the sandbox

The trait is `pin_core::TableScriptBackend` (`crates/pin_core/src/script.rs:248-259`; PLAN §3.4,
`docs/PLAN.md:390-396`). A web backend implements it like `LuaScriptBackend` and
`WasmScriptBackend` do. Two engines are possible, and Subphase 9.0 measures both before 9.5 picks
one:

- **(W1) The browser's WebAssembly API** (`BrowserWasmBackend`, new name).
  - At load, outside the tick:
    1. Run the same pre-scan with `wasmparser =0.261.0` (`crates/pin_script/src/wasm/limits.rs`).
    2. Rewrite the module with `wasm-encoder =0.261.0`:
       - set the memory maximum to the cap
       - inject a fuel global that is charged at function entry and loop headers, with bulk
         operations charged by their length operand
       - inject a call-depth counter
       - trap with `unreachable` when either runs out
    3. Compile with `WebAssembly.compile` (asynchronous) and instantiate it with an import object
       of the 18 `pinball` functions, built once (`docs/DECISIONS.md:500`).
  - Each callback resets the fuel global and calls a cached export. A JS exception is classified
    as budget, stack or runtime and drops the script into the tilt state.
  - Guest imports re-enter the sim module through JS to reach the `HostCommandQueue`.
  - This is fast, because the browser's JIT runs the rules. The cost is that the sandbox now
    includes instrumentation and JS glue, and the stack and NaN behaviour come from the engine
    unless the instrumentation adds them.
- **(W2) A pure-Rust Wasm interpreter inside the sim module** (new dependency, A19).
  - It has fuel metering and memory limits built in. Everything stays in Rust: no JS boundary, no
    re-entrancy through JS, no JS heap use.
  - It is testable natively with `cargo test`, so rule 3 is kept, and it can run against wasmtime
    with the 6.3 parity tools on native CI.
  - It is slower than a JIT. The engine budget is 100 000 fuel per callback
    (`crates/pin_script/src/wasm/mod.rs:216-221`), which is comparable to the Lua budget
    (`crates/pin_script/src/lua/mod.rs:79-85` documents ≤ 8 ms a tick worst case on native).
  - The shipped rules use far less. 9.0 measures the cost per callback.

Proposed default: W2, if 9.0 shows it fits the frame budget, and W1 otherwise. Either way, the
engine-neutral host logic (argument validation, all-or-nothing queue, tilt, held-coil release)
moves out of `crates/pin_script/src/wasm/host.rs` in 9.1, so both engines share it.

**What happens to each guarantee.** Facts about the shipped modules come from a scratch opcode scan
of the three committed `rules.wasm` files; the script was not committed, and 9.0 reproduces it
with `wasmparser`. Each module defines one memory, 18 pages initial with **no maximum**. The code
holds no float arithmetic, comparison or conversion operators, only `f32.const` literals and bulk
`memory.fill`/`memory.copy`, and no SIMD.

| Guarantee today (wasmtime) | Source | W1: browser API | W2: in-sim interpreter |
|---|---|---|---|
| Import isolation: no WASI, only the `pinball` imports | `crates/pin_script/src/wasm/mod.rs:53`; `docs/DECISIONS.md:500` | **Kept**: the import object is the whole world the module sees | **Kept** |
| Load-time module limits (D25 compile cost) | `crates/pin_script/src/wasm/mod.rs:76-102`; `crates/pin_script/src/wasm/limits.rs` | **Kept** as a pre-scan. The calibration ("compiles in under 1 s and 256 MB") was measured for Cranelift; browser tiers differ and compile off the main thread (A20), so 9.5 re-measures | **Kept**; the interpreter's translation cost is re-measured |
| Per-callback fuel budget (about one unit per operator; bulk operations per byte) | `crates/pin_script/src/wasm/mod.rs:60-75`; spike `docs/DECISIONS.md:57` | **Lost**, then **rebuilt** by instrumentation. Deterministic, but the units are not wasmtime's | **Kept** natively by the interpreter's fuel. Deterministic; units differ from wasmtime |
| Memory cap: `memory.grow` past the cap returns −1, and an oversize initial memory is rejected | `crates/pin_script/src/wasm/mod.rs:54-59`; engine cap 2 MiB (`:216-221`) | **Rebuilt exactly**: rewrite the memory maximum to the cap (2 MiB = 32 pages); the spec makes `grow` past the maximum return −1. Needed because the shipped modules declare none | **Kept** through the interpreter's limiter |
| Table limits (at most 4 tables of 65 536 elements) | `crates/pin_script/src/wasm/mod.rs:57-59` | **Kept** (pre-scan plus maximum rewrite) | **Kept** |
| Stack limit (256 KiB of Wasm stack, then a deterministic trap) | `crates/pin_script/src/wasm/mod.rs:103-106`, `:192`, `:262` | **Lost**: the engine's own limit throws `RangeError` at an engine-dependent depth, which is non-deterministic. **Rebuilt** with an instrumented call-depth counter | Interpreter stack limit, deterministic (A19) |
| NaN canonicalisation | `crates/pin_script/src/wasm/mod.rs:113-117`, `:241` | **Lost** (A12). No effect on the three shipped modules, which have no float operators. For other modules, reject float operators or instrument canonicalisation | Depends on the interpreter (A19) |
| Proposals off (SIMD, relaxed SIMD, threads and others) | `crates/pin_script/src/wasm/mod.rs:107-112`, `:239-262` | **Kept**: the pre-scan validator uses the same feature set | **Kept** |
| Deterministic instantiation cost (data segments metered) | `crates/pin_script/src/wasm/mod.rs:70-75` | Data segments are copied by the engine, unmetered but bounded by the pre-scan limits | Interpreter-specific |
| No host-side allocation on callback paths | `crates/pin_script/src/wasm/mod.rs:157-162`; `crates/pin_script/tests/wasm_alloc.rs` | The Rust side is kept. JS glue must be preallocated: cached functions, numeric arguments, and a guest-memory view re-created only after `grow`. JS heap allocation is invisible to the guard | **Kept**, all in Rust and checked by the guard |
| Fault and tilt semantics | `crates/pin_script/src/wasm/mod.rs:118-131` | **Kept** (shared host logic after 9.1) | **Kept** |

**What the browser adds.**
- A wasm module can call only its imports and cannot touch the DOM, the network or storage.
- An escape from the engine lands in the tab's sandboxed renderer process, which is isolated per
  site (A11).
- A Content-Security-Policy can restrict everything else on the page; it needs
  `'wasm-unsafe-eval'` to compile modules (A21).

**Residual risk for untrusted tables. Read this as a downgrade from native until 9.5's tests pass.**
1. **Instrumentation bugs (W1).** A bug in the instrumentation or JS glue breaks the budget or the
   stack limit. An infinite loop then freezes the tab until the browser's hang prompt; there is no
   watchdog and no Worker to terminate.
2. **Engine bugs.** Browser engine JIT bugs, compared with wasmtime and Cranelift bugs natively.
   The browser contains the damage to the page's origin.
3. **Budget units.** Budget units differ from native, so a script near its budget can fault on one
   platform and not the other. Replays of such a game diverge, and the divergence stays visible.
4. **Hostile assets.** A hostile glTF or Ogg file that panics the Rust loader aborts the whole
   engine instance. `catch_unwind` containment (`crates/pin_assets/src/gltf/mod.rs:599`,
   `crates/pin_audio/src/mixer/decode.rs:321`) does not work under `panic=abort` (A7).

v1 therefore ships only the three curated bundles. User-supplied bundles (Q10) wait until 9.5's
sandbox tests pass and `docs/THREAT_MODEL.md` carries the web entries (section 10.3).

#### 4.6 Audio

**Native audio.**
- `pin_audio`'s mixer renders three buses with voice limiting and ducking into any buffer, without
  allocating or locking (`crates/pin_audio/src/mixer/mod.rs:1-45`). The defaults are 48 kHz and
  256-frame blocks (`crates/pin_audio/src/mixer/engine.rs:89-90`).
- `pin_app`'s `AudioEngine` adds the SSF exciters and writes 7.1 blocks to an SDL3 stream
  (`crates/pin_app/src/orchestrator/audio.rs:1-17`, `:30`, `:293-330`).

**Web replacement (9.4): an AudioWorklet** (A18).
- A small separate wasm module, `pin_web_audio` (new), runs `pin_audio::Mixer` with no default
  features inside the worklet scope.
- At load, the main instance decodes the table's sounds with `pin_audio::decode`. It transfers the
  PCM to the worklet once and registers it there.
- Once per frame, after the tick batch, the main thread posts the frame's `(SoundId, gain)`
  triggers through the worklet's `MessagePort`. That is one small message a frame, sent outside
  `step_1ms`.
- No SharedArrayBuffer is needed, so no COOP/COEP.
- This keeps the 3.3 mixer's semantics (voice stealing, ducking) instead of re-creating them in a
  WebAudio node graph.
- **Autoplay.** An `AudioContext` starts suspended until a user gesture (A18), so the page shows a
  "click or press a key to start" overlay that also unlocks audio.
- **Latency.**
  - A trigger reaches the worklet at the next render quantum after the frame's message (128
    frames, about 2.7 ms at 48 kHz) (A18).
  - On top of that comes the device output latency, often 10 to 40 ms (A18).
  - Native's "within one 256-frame buffer" figure (`docs/PLAN.md:603`) is not comparable. Audio is
    presentation-only (`docs/decisions/2026-11-19-w8g-e.md`), so it does not affect replays.
- **Buffer sizes.** The worklet renders in 128-frame quanta, so the mixer is driven in 128-frame
  calls. The decode budget drops from 512 MiB (`crates/pin_audio/src/mixer/decode.rs:132`) to a web
  value, proposed 64 MiB.
- **SSF 7.1** is out. Telemetry is out.

#### 4.7 Assets and I/O

**Bundles.**
- `tables/*/build.sh` and `scripts/package.py` build the web bundles natively, as deterministic
  `.pinbundle` zip archives with `script_entry = "rules.wasm"`. The site serves them as static
  files.
- The shell fetches a bundle as an `ArrayBuffer` and copies it into a `Vec<u8>`. It then opens it
  with `Vfs::from_zip_bytes` (`crates/pin_assets/src/vfs/mod.rs:101`) and loads with
  `Simulation::load_from_vfs` (`crates/pin_sim/src/load/mod.rs:541`). This path never reads a
  file.
- Optional extras:
  - the Cache API, so a revisit does not download again
  - IndexedDB or `localStorage`, for the settings that `config.toml` holds natively
    (`crates/pin_app/src/ui/config.rs:625-785`)

**Zip-extraction rules carry over unchanged**, because they run on the bytes:
- absolute paths, `..` components and symlinks are rejected (`crates/pin_assets/src/zip/mod.rs:42-76`)
- per-entry and archive ratio caps and size caps apply (`crates/pin_assets/src/zip/mod.rs:18-38`, `:78-98`)

What does not apply on the web:
- **Folder-bundle containment** (`docs/THREAT_MODEL.md:17-19`). There are no folder bundles.
- **TM-1 (TOCTOU, `docs/THREAT_MODEL.md:29-46`).** It has no surface, because there is no shared
  filesystem.

**Budgets.**
- The default `ZipLimits` (512 MiB total, 256 MiB per file, `crates/pin_assets/src/zip/mod.rs:31-38`)
  and the 512 MiB decode budget are sized for desktops. They are too large for a wasm32 linear
  memory that also holds the GPU staging data, and for mobile. A web `ZipLimits` and decode budget
  are proposed (for example 128 MiB and 64 MiB), confirmed by 9.0 measurements.
- The built bundle sizes are unknown: bundles are generated, not committed. 9.0 records them.
- A download-size budget for the `.wasm` plus one table is proposed in section 7 and is an owner
  decision (Q12).

#### 4.8 Determinism

**What carries over** (D9, `docs/PLAN.md:25`, §2.4 `docs/PLAN.md:162-166`):
- The sim crates ban std transcendentals and use `libm`. The ban is `crates/*/clippy.toml` in
  pin_assets, pin_core, pin_physics, pin_sim, pin_input, pin_script, pin_tablegen and pin_assetgen.
- No `mul_add` appears in `crates/pin_physics/src`, `crates/pin_sim/src` or `crates/pin_core/src`.
- Iteration order is fixed, and the PCG is integer-only and seeded.
- On wasm, `f32`/`f64` arithmetic is IEEE-754 with round-to-nearest. Core wasm has no FMA, and
  rustc never fuses a multiply and add on its own. The only non-determinism in core wasm is NaN bit
  patterns (A12).
- The engine build must not enable `relaxed-simd`. Its results may differ by CPU (A12). The pinned
  build flags enable it nowhere, and CI checks the final `.wasm` with a validator that has relaxed
  SIMD disabled.
- **SIMD choice.** Whether to enable `simd128` is one pinned decision for the web build. 9.0 makes
  it from measured speed and bit equality.

**What may differ from native (A13):**
- glam 0.33.12 picks SSE2 code paths on x86_64 and scalar or `simd128` paths on wasm32, and
  simba's `wide` does the same. Summation order inside dot products and matrix code can differ.
- parry3d may call std math through `simba` and `num-traits`. Natively that resolves to the
  platform libm; on wasm32 it resolves to a Rust port of musl libm.
- NaN payloads only matter in a state that is already faulty. `state_hash` hashes float bits
  (`crates/pin_sim/src/sim/mod.rs:743-760`).
- Aligning everything, for example by forcing glam's scalar path on native, would change native
  hashes for every table. That is a D34 (a)-class change to simulation output and is not proposed.

**Recommended position: same-build determinism is the web gate; native-to-web equality is
reported, not required** (extends D9).

**What CI checks** (web workflow, section 10.4):
1. **Gate.** The web build replays a recording 10 times in Node with an identical `state_hash`,
   scores and per-tick command queues. This mirrors the native `replay-hash` job
   (`.github/workflows/ci.yml:427-490`).
2. **Report only.**
   - The same Wasm-rules bundle and recording run natively under wasmtime, and the per-tick hashes
     are compared, printing the first divergent tick.
   - The `ball_seam_bitident` digests (`crates/pin_physics/tests/ball_seam_bitident.rs`, reported
     cross-OS at `.github/workflows/ci.yml:487-490`) are printed from the wasm build.
   - The Node (V8) hash is compared with one other browser engine (A23).
3. **Not compared:** native recordings of the Lua bundle (different `table_hash`, section 4.4).

`state_hash` leaves out mech state, magnetisation, persist and lamps (`docs/HUMAN_ACCEPTANCE.md:127`).
The web gate therefore also compares the per-tick command queues and the scores, as 6.3 does.

**Effect on the existing cross-OS reports.** None:
- The `replay-hash` notices in `.github/workflows/ci.yml` and the `cross-os` job in
  `.github/workflows/soak.yml` are unchanged
  (`docs/decisions/2026-11-19-7.1.md`, "new workflow file").
- The web hash becomes one more report-only column in the web workflow's summary.
- Native and web recordings are not portable unless the report shows equality. That is a product
  consequence for shared replays and leaderboards (Q5).

#### 4.9 Time and timing sources

- **Wall-clock time.** `std::time::Instant::now()` panics on `wasm32-unknown-unknown` (A6). All
  sites are listed in 4.3, and none is in the sim. Simulated time (D8) is untouched.
- **The shell's clock.** `performance.now()`, a millisecond `f64` that browsers coarsen, to about
  100 µs normally and 5 µs when cross-origin isolated, and by other amounts in other engines (A15).
  It is converted to `u64` nanoseconds for `SimClock`-style accounting. `SimClock::advance` already
  takes `now_ns` as an argument (`crates/pin_app/src/orchestrator/mod.rs:113`).
- **Timestamp precision.** Input `event.timeStamp` values share that timeline (A15). Coarsening of
  1 ms or less moves an input by at most one tick.
- **Pacing.** `requestAnimationFrame` follows the display refresh (60, 120 or 144 Hz). The tick
  batch absorbs whatever the frame interval is.
- **Background tabs.** In a hidden tab `requestAnimationFrame` stops and timers are throttled (A16).
  The game therefore pauses on `visibilitychange`, and simulated time resumes from a new epoch.
  Recordings store per-tick inputs, not wall time, so pausing has no effect on determinism.
- **Profiler clock.** The native profiler's SDL clock (7.1) is native-only. The web overlay uses
  `performance.now()`.

#### 4.10 Build tooling and CI

- **Target.** `wasm32-unknown-unknown` for the pinned rustc 1.97.0 (`versions.toml:12`).
  - It is not installed on this agent machine: `rustup target list --installed` printed only
    `x86_64-unknown-linux-gnu` on 2026-11-20.
  - CI already installs it for the rules: `targets: wasm32-unknown-unknown` in
    `.github/workflows/rules-wasm.yml:62-64`. 6.3 also added it locally
    (`docs/decisions/2026-11-19-6.3.md`, "how the wasm32 target was obtained").
  - So the recommended path needs no new toolchain, only the target. Emscripten (emsdk) would be
    new, and is used only by Lua route (b).
- **Tools.**
  - **Proposed.** Plain `cargo build --target wasm32-unknown-unknown`, then the `wasm-bindgen` CLI
    at exactly the crate's version, 0.2.130 from `Cargo.lock` (A22), then `wasm-opt` from a pinned
    binaryen.
  - **Rejected: `wasm-pack`.** It adds npm packaging that is not needed.
  - **Optional: `trunk`.** Allowed as a local dev server only, not in CI. Fewer tools means
    reproducible artifacts, in the spirit of the byte-identical `rules.wasm` rebuilds.
  - **Tests.** `wasm-bindgen-test`, with Node as the GPU-free runner and a headless browser for
    render smoke tests (A23).
- **Build flags.**
  - Web target features and the `getrandom` backend flag (A14) are passed by the web build script
    (`scripts/web/` (new)), not by a repository-wide `.cargo/config.toml` (none exists today).
  - The guest `tables/*/rules-wasm/rebuild.sh` scripts set `CARGO_ENCODED_RUSTFLAGS` and compare bytes
    (`tables/apex_circuit/rules-wasm/rebuild.sh`). A repo-wide target config would be one more
    input to keep away from those byte-for-byte rebuilds.
- **Workspace.** `members = ["crates/*", …]` (`Cargo.toml:5`) picks up `pin_web` automatically.
  - It must build natively as a host-testable library: web-only code sits under
    `cfg(target_arch = "wasm32")`, and web-only crates under
    `[target.'cfg(target_arch = "wasm32")'.dependencies]`.
  - Dependants of `pin_sim`/`pin_script` on the web path must set `default-features = false` and
    forward features. Otherwise Cargo feature unification pulls `mlua` and wasmtime into the wasm
    build.
- **Rule 3, GPU-free core tests (`docs/PLAN.md:154`).** Preserved:
  - `core-tests` keeps running `cargo test --workspace --exclude pin_render --exclude pin_audio
    --exclude pin_app` (`.github/workflows/ci.yml:150`). With default features this still builds
    and tests Lua and wasmtime.
  - `pin_web`'s host-testable modules (frame-loop accounting, input mapping, message encoding) test
    natively with no display or GPU.
  - Wasm-specific tests run in Node, which has no GPU either.
  - Browser render tests run only in the web workflow's browser job, as D20 separates GPU tests.
- **CI additions** are listed in section 10.4.

### 5. Options

| | A: full port behind cfg | **B: thin web shell (recommended)** | C1: replay viewer only | C2: server-side streaming | D: Emscripten whole-app port |
|---|---|---|---|---|---|
| **What** | `pin_app` gains a web platform module beside `crates/pin_app/src/platform/`, and a frame-loop orchestrator under `cfg(target_arch = "wasm32")`; SDL3, threads and DOF are gated in place | New `pin_web` (new) shell over the shared crates. Native backends become default features. Optional 9.6 makes `pin_app`'s SDL-free modules reusable | B's first milestone alone: load a table and play back a recording; no input, menu or audio | The native engine runs on a GPU server; video goes out over WebRTC and inputs come back | Compile everything with Emscripten: SDL3's Emscripten backend, `lua-src`'s Emscripten arm |
| **Pros** | One app; menu, HUD and wiring reused; fewest duplicate paths | Native defaults unchanged; new code isolated in new crates; seams are small and testable; incremental; GPU-free tests stay; web failures cannot break native CI | Smallest web deliverable; shows determinism and rendering; shareable replays and attract mode; no latency questions | No port; full fidelity including Lua and cabinet features | Lua and SDL kept |
| **Cons** | `cfg` sprawl in native-critical code (`pin_app` orchestrator, render present path); rule 1 module ownership blurs; `harness = false` SDL tests split; the D6 thread model forks inside the same files | A second shell to maintain; pin_app and pin_web can drift apart; needs 9.6, or some duplication | Does not meet "plays the tables"; still needs the rules backend, because replays run the scripts | Network round trip plus encode and decode (tens of ms or more, A27) against a 1 ms tick and a 1.5 ms input target (`docs/PLAN.md:785`); GPU servers, operations and per-player cost; infrastructure outside the repo and CI (D28) | wgpu's web backends need wasm-bindgen, which does not support Emscripten (A10); `sdl3` build-from-source for Emscripten is unknown; threads need pthreads plus COOP/COEP |
| **Effort** | L+: about 30 to 45 implementer and tester sessions, plus more fix rounds | L: implementers 17 to 24, testers about 8, plus verifier and fix rounds; about 35 to 45 runs in all | M: about 10 to 14 runs (9.0, 9.1, 9.2, 9.5, part of 9.7, 9.8) | M to L of infrastructure, plus ongoing operations | L+ with high uncertainty |
| **Risk** | High for native | Medium (sandbox rebuild, WebGPU maturity, determinism unknowns) | Low to medium | High (product and operations) | High (toolchain conflict) |
| **Effect on native** | Many native files touched; "unchanged" is hard to prove | Feature flags with native defaults; async-capable render init behind the same sync API; native `replay-hash` value must be unchanged | Same as B's first half | None | A second toolchain; large |

### 6. Recommendation

Option B, delivered as M-web-1 (Option C1, the replay viewer) and then M-web-2 (playable).

**Why:**
1. **The blockers are in the shell.** They sit in SDL3, threads, blocking GPU init, C Lua and
   wasmtime. The physics, input, rules logic and assets are already pure Rust that uses no thread,
   clock or file (section 3). A new shell takes the platform away and leaves the simulation alone,
   so native keeps every default and its replay hash.
2. **The decision log already fits it.**
   - D9 makes cross-platform hashes report-only.
   - D10 already has a second rules backend, with tables ported in 6.3 and parity enforced in CI.
   - D18 and D33 make the probe path the stand-alone baseline.
   - D20 keeps GPU tests out of core tests.
   - D4 is respected: no contract change is needed.
3. **Every web-specific failure stays in web code.** It lives in a new crate or behind a non-default
   feature, and runs under a separate workflow. Option A would put `cfg` branches into the native
   orchestrator and present path, where the native gates (A1, A2, soaks) have the most to lose.
4. **The first milestone pays off early.** It produces a playable-looking artifact and the
   determinism numbers before the input, audio and menu work is committed.

### 7. Phased plan: Phase 9, Browser Build

The subphases use the PLAN's numbering. Ids are numeric because decision-file names must be
lowercase (`spikes/spec_0_1/tests/spec_log_split.rs:43`). Waves 10 to 14 follow the completed
Waves 0 to 9 (`docs/RUN_LOG.md:499`). Tiers follow §2.5 (`docs/PLAN.md:172-177`). Each subphase also
gets an Independent Tester (D27), usually at T1, as in Waves 8 and 9 (`docs/RUN_LOG.md:468`,
`:488`).

**D34 note for Fable (T3).**
- (a) holds for 9.5, and for optional 9.10: they change the untrusted-table boundary
  (`docs/PLAN.md:182`).
- (b) does not hold. Opus 5.5 built the wasmtime backend (4.5, `docs/PLAN.md:829`), and no T2
  attempt has failed. So T3 is not proposed anywhere. If an Opus attempt on 9.5 fails, the
  classifier may re-apply D34 with that failure as evidence.

#### Subphase 9.0: Web feasibility spike
* **Difficulty:** high (unknowns) · **Tier:** T2 Opus 5.5 · **Size:** M, 2–3 sessions
* **Target:**
  - `spikes/web_core/` (new; a member of the spikes workspace)
  - `.github/workflows/web-spike.yml` (new; `workflow_dispatch` only)
  - if the result is go: the `crates/pin_web/` skeleton (new)
* **Dependencies:** Wave 9 gate (7.2); uses the outputs of 2.5, 4.5 and 6.3. It can run while
  Phase 8 waits on the owner.
* **Deliverables:**
  1. **Build matrix.** Run `cargo build --target wasm32-unknown-unknown` per crate: pin_core,
     pin_physics, pin_input, pin_assets, pin_audio (no default features), pin_tablegen (lib),
     pin_assetgen (lib) and pin_render. Record pass or fail and the first blocker per crate, for
     example the `getrandom` backend (A14). pin_script and pin_sim are expected to fail on
     `lua-src`; confirm and record it.
  2. **Physics determinism probe.** A wasm32 build of a harness drives `pin_physics` directly
     through the `ball_seam_bitident` scenarios and a 10 000-tick multi-ball scenario. Print the
     digests on native x86_64, in Node and in one headless browser. Record equal or first
     divergence (A13). Decide whether to enable `simd128`.
  3. **Rules engine probe.** Feed the three committed `rules.wasm` the 6.3 recorded switch-edge
     streams under three engines and compare per-tick queue bytes with wasmtime:
     - (W1) Node's WebAssembly API, through a minimal host shim with an instrumented module
     - (W2) a candidate pure-Rust interpreter compiled to wasm32
     - wasmtime natively, as the reference

     Record the time per callback and the fuel-unit mapping (A11, A19).
  4. **Render probe.** A wgpu WebGPU program in headless Chrome clears a target and draws one PBR
     material. Record the adapter limits against `downlevel_defaults`, the canvas formats, and
     whether the WGSL modules compile in the browser (A1, A3–A5, A23).
  5. **Size probe.** Record the `.wasm` size after `wasm-opt`, and its gzip and brotli sizes,
     for the render probe and the physics harness (A24).
  6. **Report.** `docs/decisions/<date>-9.0.md` gives an answer or a recorded negative result for
     each of A1 to A28 that can be tested, the build flags chosen, the tool versions, and a
     go/no-go with the reasons.
  7. **Skeleton, only if go.** `crates/pin_web/` (new) with `Cargo.toml` and `mod` stubs (D3) for
     `frame/`, `input/`, `audio/`, `headless/`, `app/`, `storage/` and `web/`. It compiles natively
     to an empty, host-testable library.
* **Gate:** every deliverable has a number or a recorded negative result with the fallback chosen,
  as in 0.1 (`docs/PLAN.md:431`). No product crate changes other than the skeleton. The native
  workspace build and tests are unchanged.

#### Subphase 9.1: Script-backend feature gates and host-logic split
* **Difficulty:** medium · **Tier:** T2 · **Size:** M, 2 sessions
* **Target:**
  - `pin_script`: `crates/pin_script/Cargo.toml`, `crates/pin_script/src/lib.rs`, the module roots
    of `crates/pin_script/src/lua/` and `crates/pin_script/src/wasm/`, and the host-logic split out
    of `crates/pin_script/src/wasm/host.rs`
  - `pin_sim`: `crates/pin_sim/Cargo.toml`, `crates/pin_sim/src/sim/`, `crates/pin_sim/src/load/`,
    `crates/pin_sim/src/tick/`
* **Dependencies:** 9.0 (go)
* **Deliverables:**
  1. **Features.** `lua` (mlua, notify) and `wasmtime` (wasmtime, wat), both default on, with
     forwarding features in `pin_sim`.
  2. **Pre-scan.** `crates/pin_script/src/wasm/limits.rs` uses `wasmparser =0.261.0` directly.
  3. **Shared host logic.** Argument validation, the all-or-nothing queue, tilt and held coils move
     into an engine-neutral module used by `WasmScriptBackend`.
  4. **pin_sim.** The `Script` variants and the `SimConfig` fields are `cfg`-gated per feature. A
     web-backend variant slot is left for 9.5.
* **Gate:**
  - With default features nothing changes. All existing `pin_script`, `pin_sim` and `pin_app` tests
    pass unmodified, including the `rules-wasm` parity suite.
  - The `replay-hash` value for the fixture equals `main`'s, on the same runner class.
  - `cargo check -p pin_sim --no-default-features --target wasm32-unknown-unknown` passes.
  - Coverage stays at or above 85 % (`docs/PLAN.md:225`).

#### Subphase 9.2: `pin_render` web path
* **Difficulty:** high · **Tier:** T2 · **Size:** M–L, 2–3 sessions
* **Target:** `crates/pin_render/src/gpu/`, plus the named blocking sites in
  `crates/pin_render/src/post/chain.rs` and `crates/pin_render/src/raytrace/source.rs`
* **Dependencies:** 9.0, and native 1.4, 5.1–5.5
* **Deliverables:**
  1. Async `GpuContext` constructors. Native keeps the current sync API as `pollster` wrappers.
  2. A canvas surface on `wasm32`.
  3. No presenter thread, blocking poll or `Instant` wait on `wasm32`.
  4. Non-blocking error-scope handling on the web.
  5. The wgpu `webgpu` feature (A1) on `wasm32` only.
  6. A browser shader-compile check for every WGSL module.
* **Gate:**
  - Native `gpu-tests` on lavapipe, WARP and Metal are unchanged and green, with the same goldens
    (D30).
  - `cargo check -p pin_render --target wasm32-unknown-unknown` passes.
  - Where CI offers a WebGPU adapter (A23), an offscreen greybox frame passes the analytic pixel
    checks; otherwise this is report-only.
  - Coverage stays at or above 60 %.

#### Subphase 9.3: Web frame loop and input
* **Difficulty:** medium-high (timing) · **Tier:** T2 · **Size:** M, 2 sessions
* **Target:** `crates/pin_web/src/frame/` (new), `crates/pin_web/src/input/` (new)
* **Dependencies:** 9.0; uses 3.1 and 3.2 (`pin_input`)
* **Deliverables:**
  1. The `requestAnimationFrame` accumulator with `WEB_MAX_TICKS_PER_FRAME`, built over a `SimStep`.
  2. Pause and resume on visibility.
  3. Input mapping:
     - `KeyboardEvent.code` → SDL scancode → `pin_input` profiles
     - Gamepad standard mapping → `GamepadButton` and `GamepadAxis`
     - touch zones → `CabinetInput` and `CabinetAxis`
  4. Timestamps from `event.timeStamp`.
* **Gate:**
  - Simulated-time tests (D8) with synthetic frame timestamps and timestamped inputs. Actuation
    lands on the exact expected tick in all three staging modes, mirroring 3.1
    (`docs/PLAN.md:587`). The cap and pause behaviour are tested.
  - The allocation guard passes on the frame loop with a stub `SimStep`.
  - Coverage is at least 60 %.

#### Subphase 9.4: Web audio
* **Difficulty:** medium · **Tier:** T2 (T1 possible) · **Size:** M, 2 sessions
* **Target:** `crates/pin_web_audio/` (new), `crates/pin_web/src/audio/` (new), and the worklet
  loader in `crates/pin_web/web/` (new)
* **Dependencies:** 9.0; uses 3.3
* **Deliverables:**
  1. The `pin_audio::Mixer` in an AudioWorklet module.
  2. Sample transfer at load.
  3. A per-frame trigger message.
  4. Gesture unlock.
  5. A web decode budget.
* **Gate:**
  - Offline Node tests: the worklet's mixer output for a trigger sequence equals `Mixer::render` on
    native for the same blocks, bit for bit (same code, so the test proves the wiring).
  - The message protocol is property-tested.
  - Audible output goes on the human checklist.

#### Subphase 9.5: Web rules backend and headless web replay
* **Difficulty:** high (security boundary) · **Tier:** T2 (D34 (a) holds, (b) does not) · **Size:**
  L, 3–4 sessions
* **Target:**
  - `crates/pin_script/src/web_rules/` (new), or the interpreter module, whichever 9.0 chose
  - `crates/pin_web/src/headless/` (new)
* **Dependencies:** 9.1; uses 4.5 and 6.3
* **Deliverables:**
  1. The web backend implementing `TableScriptBackend` with the guarantees in the 4.5 table:
     memory-maximum rewrite, deterministic budget, stack limit, import isolation, pre-scan, and the
     float-operator policy.
  2. A headless web entry point that loads a bundle from bytes and replays a recording.
  3. Web bundle packaging (`script_entry = "rules.wasm"`).
* **Gate:**
  - Sandbox tests, all run in Node:
    - fuel exhaustion → tilt
    - `grow` past the cap → −1
    - deep recursion → a deterministic fault
    - foreign imports rejected
    - oversize and over-limit modules rejected
  - Per-tick queue bytes equal the wasmtime backend for each table's recorded games, reusing the
    6.3 method.
  - 10 identical replays in Node.
  - The allocation guard passes over 60 000 ticks per table.
  - The native-to-web hash is printed (report only).

#### Subphase 9.6: `pin_app` native-feature split (optional reuse)
* **Difficulty:** medium · **Tier:** T2 · **Size:** M, 2 sessions
* **Target:** `pin_app`: `crates/pin_app/Cargo.toml`, `crates/pin_app/src/lib.rs`, the module roots
  of `crates/pin_app/src/platform/`, `crates/pin_app/src/orchestrator/` and
  `crates/pin_app/src/dof/`, the SDL parts of `crates/pin_app/src/orchestrator/audio.rs`, and
  `crates/pin_app/src/main.rs`
* **Dependencies:** 9.1, for the feature names
* **Deliverables:**
  1. `sdl3`, threads, the presenter, DOF and the profiler window parts move behind a default
     feature `native`.
  2. `pin_sim` is used with `default-features = false`, with the features forwarded.
  3. The SDL-free modules build for `wasm32`: `crates/pin_app/src/orchestrator/view.rs`,
     `crates/pin_app/src/orchestrator/table.rs` without its loader thread, `AudioEngine`,
     `crates/pin_app/src/ui/menu.rs` and `crates/pin_app/src/ui/view.rs`.
* **Gate:**
  - All `pin_app` tests pass unmodified with default features, including `gpu-tests` and the
    `harness = false` SDL targets.
  - `cargo check -p pin_app --no-default-features --target wasm32-unknown-unknown` passes.
  - Coverage is at least 60 %.

#### Subphase 9.7: Web app integration (M-web-1, then M-web-2)
* **Difficulty:** high · **Tier:** T2 · **Size:** L, 3–4 sessions
* **Target:**
  - `crates/pin_web/src/app/` (new), `crates/pin_web/src/storage/` (new), `crates/pin_web/web/`
    (new; HTML and JS loader)
  - `scripts/web/` (new; build script and site assembly)
* **Dependencies:** 9.2, 9.3, 9.4, 9.5, 9.6
* **Deliverables:**
  1. Fetch, validate (clearance) and load the three web bundles.
  2. The `TableRenderer` playfield, plus backglass and DMD/LCD as PiP in one canvas.
  3. The reduced egui menu: table picker, input and display panels.
  4. Settings in browser storage.
  5. The start overlay (gesture and audio unlock) and the "no WebGPU" page.
  6. A minimal frame-time and tick-time overlay.
  7. M-web-1, the replay viewer, ships first. M-web-2, playable, follows.
* **Gate:** Gate W (below). Coverage is at least 60 % on host-testable modules.

#### Subphase 9.8: Web CI and release
* **Difficulty:** low-medium · **Tier:** T1 · **Size:** M, 1–2 sessions
* **Target:** `.github/workflows/web.yml` (new)
* **Dependencies:** 9.5. Its jobs cover 9.7's targets by name.
* **Deliverables:** the workflow in section 10.4, a size-budget check, and an optional release
  artifact (a zip of the static site). No deployment: hosting is the owner's (Q2).
* **Gate:** the workflow runs green on the 9.7 branch. Its spec checks (job list, path filters, no
  `continue-on-error` on gating steps) follow the `tests/spec_ci` style.

#### Gate W (automated, after Wave 13)
1. **Build.** `pin_web` builds with the pinned toolchain, the matching `wasm-bindgen` CLI and
   `wasm-opt`. Artifact size is within budget. Proposed: the engine `.wasm` at most 8 MiB after
   `wasm-opt` and at most 3 MiB brotli. The 9.0 measurements replace these figures, and the owner
   decides (Q12).
2. **Native unchanged.** The full `.github/workflows/ci.yml` is green, and the native `replay-hash` value is unchanged
   against the pre-Phase-9 `main`.
3. **Web determinism.** For each of the three tables, 20 recorded games (the Gate A2 sample size,
   `docs/PLAN.md:767`) replay identically 10 times in Node: hash, scores and per-tick queues.
4. **Rules parity.** The web backend's per-tick `HostCommandQueue` bytes equal the native wasmtime
   backend's for the same games and bundle.
5. **Report only.** Native-to-web hash per tick, with the first divergence. Cross-engine (Node
   compared with a headless browser) hashes.
6. **Sandbox and rule 4.** The 9.5 sandbox tests and the allocation guard (60 000 ticks per table)
   pass.
7. **Frame loop.** The 9.3 simulated-time tests pass.
8. **Render.** The analytic pixel checks on a greybox frame pass where CI offers WebGPU (A23);
   otherwise they are report-only and on the human checklist.
9. **Human checklist.** Added to the human acceptance list: play each table in Chrome, Firefox and
   Safari; judge feel and latency on keyboard, gamepad and touch; check audio.

#### Optional Wave 14 (owner choice after Gate W)
* **9.9 WebGL2 fallback.** `crates/pin_render/src/pbr/` and `crates/pin_render/src/gpu/`: cluster
  lists move out of storage buffers, plus webgl2 limits, the HDR and MSAA path, and naga `glsl-out`
  validation. L, 3–4 sessions, T2.
* **9.10 Lua on the web.** `third_party/lua-src/`, `crates/pin_script/src/lua/`, by route (a). L,
  3–5 sessions, T2. D34 (a) holds, because it changes the vendored Lua patches.
* **9.11 Mobile and touch polish.** `crates/pin_web/src/input/` (new), `crates/pin_web/src/app/`
  (new): layout, orientation, device-motion nudge. M, 2 sessions, T1.
* **9.12 Worker-hosted sim.** `crates/pin_web/src/frame/` (new), with COOP/COEP hosting. Only if
  9.7 measures jank. M–L, 3 sessions, T2.

**Wave and dependency matrix** (proposed rows for PLAN §5). The Crate / module column uses PLAN §5's
`crate/module` shorthand: `pin_render/gpu` means `crates/pin_render/src/gpu/`. Every `pin_web`,
`pin_web_audio`, `web_rules`, `scripts/web` and `web.yml` entry is new.

| Wave | Subphase | Tier | Size (sessions) | Depends on | Crate / module |
|---|---|---|---|---|---|
| **10** | 9.0 Web feasibility spike | T2 | M (2–3) | Wave 9 gate | `spikes/web_core` (new), `pin_web` skeleton (new) |
| **11** | 9.1 Backend gates and host split | T2 | M (2) | 9.0 | `pin_script` (features, `wasm/` host split), `pin_sim/sim`, `load`, `tick` |
| **11** | 9.2 `pin_render` web path | T2 | M–L (2–3) | 9.0 | `pin_render/gpu` (+ named sites in `post/`, `raytrace/`) |
| **11** | 9.3 Frame loop and input | T2 | M (2) | 9.0 | `pin_web/frame`, `pin_web/input` |
| **11** | 9.4 Web audio | T2 | M (2) | 9.0 | `pin_web_audio` (new), `pin_web/audio` |
| **12** | 9.5 Web rules backend and headless replay | T2 | L (3–4) | 9.1 | `pin_script/web_rules` (new), `pin_web/headless` |
| **12** | 9.6 `pin_app` native-feature split | T2 | M (2) | 9.1 | `pin_app` (Cargo.toml, lib.rs, module roots) |
| **13** | 9.7 Web app integration | T2 | L (3–4) | 9.2, 9.3, 9.4, 9.5, 9.6 | `pin_web/app`, `storage`, `web`; `scripts/web` (new) |
| **13** | 9.8 Web CI and release | T1 | M (1–2) | 9.5 | `.github/workflows/web.yml` (new) |
| — | **Gate W** (automated) | Orchestrator | — | 9.7, 9.8 | Section 7 Gate W |
| 14 (opt.) | 9.9, 9.10, 9.11, 9.12 | T2, T2, T1, T2 | L, L, M, M–L | Gate W | as listed above |

**Parallelism under rule 1** (`docs/PLAN.md:152`).
- **Wave 11** runs four subphases at once. They touch disjoint crates or `pin_web` module
  directories pre-stubbed by 9.0 (D3): `pin_script` with `pin_sim`, `pin_render`,
  `pin_web/frame` with `pin_web/input`, and `pin_web_audio` with `pin_web/audio`.
- **Wave 12** runs two at once: `pin_script/web_rules` with `pin_web/headless`, and `pin_app`.
- **Wave 13** runs two at once: `pin_web/app` and the workflow file.
- No subphase shares a wave with one of its dependencies (D2).

**Totals.**
- 9 subphases (9.0 to 9.8) in 4 waves, plus 4 optional ones.
- About 17 to 24 implementer sessions and about 8 Independent Tester sessions, plus verifier and fix
  rounds: roughly 35 to 45 agent runs.
- One automated gate (Gate W) and one human checklist addition.

### 8. Risks

| # | Risk | Kind | Likelihood | Impact | Mitigation |
|---|---|---|---|---|---|
| R1 | Native and web hashes diverge (glam and `wide` SIMD paths, parry3d math) (A13) | technical | medium-high | low (report-only) | D9 extension; seam digests in the report; same-build gate |
| R2 | The rebuilt rules sandbox (fuel, cap, stack) has a gap | security | medium | high for untrusted tables | Prefer the in-sim interpreter (W2); 9.5 sandbox tests; v1 ships only curated bundles; THREAT_MODEL entries |
| R3 | The async and no-thread changes in `pin_render` regress native rendering | technical | medium | high | Native keeps the sync API (`pollster` wrappers); native `gpu-tests` and goldens unchanged as a 9.2 gate |
| R4 | WebGPU gaps or bugs in Firefox, Safari or mobile (A2) | technical, product | medium | medium | WebGPU-only v1 with a clear message; optional 9.9 |
| R5 | Main-thread jank (GC, layout, egui) bursts ticks or drops time | technical | medium | medium | Frame cap, pause, a measurement in 9.7; optional 9.12 Worker |
| R6 | Gamepad polling quantises flipper timing to the frame (A17) | UX | high | medium | `Gamepad.timestamp`; document it; keyboard and touch are timestamped |
| R7 | Audio output latency of 10 to 40 ms (A18) | UX | medium | low-medium | AudioWorklet, small quanta; accepted for v1 |
| R8 | `panic=abort` removes `catch_unwind` containment for hostile assets (A7) | security, robustness | medium | medium (tab DoS) | Curated bundles; new THREAT_MODEL entry; later, decode in a separate instance |
| R9 | `getrandom`, `zip` or `notify` fail to compile for wasm32 (A14) | build | medium | low | Features, cfg, backend flag; found by 9.0 |
| R10 | `.wasm` size and load time (A24) | product | medium | medium | `wasm-opt`, brotli, per-table download, size gate |
| R11 | Desktop-sized budgets (512 MiB zip and decode) exhaust browser memory | technical | medium | medium | Web `ZipLimits` and decode budget |
| R12 | `pin_app` and `pin_web` drift apart (two shells) | maintenance | high | medium | 9.6 reuse; shared tests; the web workflow also runs on shared-crate changes |
| R13 | CI cost and time grow (wasm builds, browsers) | maintenance | high | low-medium | Path filters; not required at first (Q11) |
| R14 | Browser and WebGPU churn breaks the build | maintenance | medium | medium | Exact pins; scheduled web workflow run |
| R15 | Feature unification pulls `mlua` or wasmtime into the wasm build | build | high | low (caught at compile time) | `default-features = false` chains; a `cargo check` in CI |
| R16 | Schedule: 35 to 45 runs, and 9.0 may say no-go; Phase 8 is still pending (`docs/RUN_LOG.md:499`) | schedule | medium | medium | Spike first; M-web-1 shows value early; owner sequencing (Q1) |

### 9. Open questions for the owner

1. **Go and sequencing.** Start Phase 9 now, in parallel with Phase 8 human acceptance, or after it?
2. **Hosting.** Where is the site hosted (GitHub Pages, own domain, a game portal)? The recommended
   design needs no COOP/COEP headers. A Worker sim (9.12) would, and so rules out hosts that cannot
   set headers (A9). Is a Content-Security-Policy wanted (A21)?
3. **Renderer reach.** WebGPU only (recommended), or fund the WebGL2 fallback (9.9) for browsers
   without WebGPU?
4. **Lua on the web.** Accept Wasm-only rules on the web (recommended), so Lua-only and grey-box
   tables do not run there, or fund 9.10?
5. **Replay-hash parity.** Accept native-to-web equality as report-only (recommended, extends D9)?
   Or require it, at the cost of a sim-output change on native?
6. **Rules engine.** Let 9.0's numbers decide between the in-sim interpreter (W2, preferred for the
   sandbox) and the browser JIT with instrumentation (W1)? Or set a preference now?
7. **Mobile.** In v1 or later? This covers touch layout, orientation and device-motion nudge (9.11).
8. **Cabinet scope.** Confirm that multi-monitor, DOF, UDP telemetry, SSF 7.1, X-Arcade gamepad
   auto-detection, rumble and hot reload are native-only.
9. **Input latency target.** What web target replaces the native ≤ 1.5 ms at p99? Proposed: zero
   tick-placement error for timestamped events, with frame latency reported only.
10. **User bundles.** Allow players to load their own `.pinbundle` files in the browser (an
    untrusted-table surface, section 4.5), or only the three curated tables?
11. **Maintenance.** Who keeps the web target green? Should a `wasm32` `cargo check` of the shared
    crates join the required `pr-checks` after Gate W? That raises PR CI cost, and
    `tests/spec_ci/tests/spec_ci_yml.rs:735-765` must be updated.
12. **Size budget.** What download size is acceptable for the engine `.wasm` plus one table?
13. **Browser storage.** Persist settings in browser storage (proposed)? Should high scores persist
    on the web, given that native does not persist them today?

### 10. Changes needed if accepted (NOT APPLIED)

None of the changes below has been made. Each would land through its own unit of work and PR
(§2.7).

#### 10.1 `docs/PLAN.md`
1. **Header.** A "v6 changes" paragraph after the v5 one (`docs/PLAN.md:13`): browser build, Phase
   9, Waves 10 to 14.
2. **§0 decision table.**
   - Update D1 (`docs/PLAN.md:17`): subphase and wave counts, plus Phase 9.
   - Add the rows D43 to D48 drafted in 10.2.
3. **§1 Executive Summary** (`docs/PLAN.md:80-90`). Add a "Browser" bullet: WebGPU, Wasm rules, and
   cabinet features native-only.
4. **§2.1 workspace tree** (`docs/PLAN.md:98-130`). Add `pin_web/` (new), `pin_web_audio/` (new)
   and `spikes/web_core/` (new).
5. **§2.2 Thread model** (`docs/PLAN.md:132-148`). Add a "Web build" table:
   - the main thread runs DOM listeners and the frame loop (due ticks up to the cap, then render)
   - the AudioWorklet runs the mixer
   - there is no I/O worker, because fetch is async outside the frame

   Also state that the tick order is unchanged and that "8 catch-up ticks" is native-only.
6. **§2.3 rule 3** (`docs/PLAN.md:154`). Add `pin_web`'s host-testable code to the GPU-free list.
   Wasm tests run in Node without a GPU.
7. **§2.3 rule 4** (`docs/PLAN.md:155`). Add a web clause:
   - no Rust allocation in `step_1ms` on wasm
   - JS glue preallocated
   - the allocation guard runs in a Node build
   - JS-heap allocation on the per-tick path is a review item
8. **§2.4** (`docs/PLAN.md:162-166`). Add a web bullet:
   - same-build determinism is the gate, checked in Node
   - native-to-web and cross-browser hashes are report-only
   - no relaxed SIMD, and the web target features are pinned
9. **§2.6 subphase gate** (`docs/PLAN.md:221-229`).
   - Item 4: `pin_web` at 60 % or more.
   - Item 6: also run the allocation guard on the web build.
10. **§3.4** (`docs/PLAN.md:355-406`). Add:
    - a web-backend paragraph
    - "Lua is native-only" (if Q4 is accepted)
    - the web sandbox list from 4.5
11. **§3.5** (`docs/PLAN.md:408-411`). Add web zip limits.
12. **§4.** A new "Phase 9: Browser Build" containing Subphases 9.0 to 9.8, optional 9.9 to 9.12,
    and Gate W, in the format of section 7 above.
13. **§5** (`docs/PLAN.md:811-859`). Add rows for Waves 10 to 14 and update the totals line.
14. **§6** (`docs/PLAN.md:863-876`). Add rows for the web library checks, filled from 9.0.
15. **Phase 8** (`docs/PLAN.md:800-807`). Add web play checks, or generate them through
    `scripts/human_acceptance.py`.
16. **New Appendix E: Web hosting and build.** Covers headers, MIME type, CSP, the size budget and
    tool versions.

`crates/pin_core/tests/schema_fixtures.rs:32` includes `docs/PLAN.md` as text, so every edit must
leave Appendix A byte-identical.

#### 10.2 Decision log
`docs/DECISIONS.md` is frozen (D41, `docs/PLAN.md:57`) and gets **no** change. New rows go into the
PLAN §0 table, and each Phase 9 unit records its own `docs/decisions/<date>-<unit>.md`. Draft
wording:

| # | Topic | Decision | Why |
|---|---|---|---|
| D43 | Browser target | A browser build (`wasm32-unknown-unknown`, WebGPU) plays `apex_circuit`, `sky_deck` and `feature_lab` with keyboard, gamepad and touch. It is a second shell (`pin_web`) over the shared crates; the native app stays the reference. Multi-monitor, DOF, UDP telemetry, SSF 7.1, X-Arcade auto-detection, hot reload and the latency profiler are native-only | The core crates are portable; the blockers are confined to SDL3, threads, blocking GPU init, the C Lua VM and wasmtime |
| D44 | Web loop | The browser main thread runs a fixed-step loop. Each animation frame runs the due 1 ms ticks back to back (at most `WEB_MAX_TICKS_PER_FRAME`, then time is dropped) and renders the latest snapshot. Inputs carry the browser event timestamp and land on their tick. A hidden tab pauses simulated time. No Worker or SharedArrayBuffer in v1, so no COOP/COEP requirement | Same tick order and D5 rules as native, with no hosting-header requirement |
| D45 | Web rules | On the web, rules run only from a bundle's `rules.wasm` (6.3); Lua tables do not run in the browser. The web backend enforces D25 in Rust: import isolation, the load-time module limits, a deterministic per-callback budget, the memory cap (`grow` returns −1) and a deterministic stack limit | `lua-src` has no `wasm32-unknown-unknown` build; wasmtime cannot run in a browser; the three tables' Wasm rules are CI-proven equal to their Lua rules |
| D46 | Web determinism | The gate is that the same web build and the same recording give an identical replay, checked in Node. Native-to-web and cross-browser hashes are reported, never required (extends D9). No relaxed SIMD; web target features are pinned in the web build script | Float code paths (SIMD or scalar, libm sources) differ by target |
| D47 | Web rendering | WebGPU only in v1, with no WebGL2 fallback. Ray queries are unavailable, so the D18 probe path applies. Same downlevel limits as native | The forward pass reads storage buffers in the fragment stage, which WebGL2 lacks |
| D48 | Web CI | A separate, path-filtered `web.yml` runs wasm32 clippy, Node tests, the web build with a size budget and a browser smoke test. It is not part of `pr-checks` until the owner promotes it | Follows the 6.3 and 7.1 precedent; keeps native required checks unchanged |

#### 10.3 `docs/THREAT_MODEL.md`
1. **Scope.** Add a "Web build" paragraph. Bundles come only from same-origin fetches of curated
   bundles in v1; user-supplied bundles are out until Q10 is decided.
2. **Existing defences.**
   - Add the Wasm backend's defences, which are missing today: the file lists only the Lua sandbox,
     budget and cap, zip and VFS (`docs/THREAT_MODEL.md:8-19`). The defences are defined in
     `crates/pin_script/src/wasm/mod.rs:51-117`.
   - Add the web equivalents from section 4.5.
3. **Note that TM-1** (`docs/THREAT_MODEL.md:29-46`) does not apply on the web: there are no
   folder bundles.
4. **New accepted risks**, each with a revisit trigger:
   - **TM-2.** `panic=abort` on wasm32: a loader or decoder panic aborts the engine instance, which
     is a tab DoS.
   - **TM-3.** Web budget units differ from wasmtime fuel, so a near-budget script can fault on one
     platform only.
   - **TM-4.** Browser engine bugs, contained by the browser sandbox.
   - **TM-5** (if W1 is chosen). The instrumentation and JS glue are part of the sandbox boundary.

#### 10.4 CI workflows
1. **New `.github/workflows/web.yml` (new).** Path-filtered on `crates/**`, `tables/**`,
   `Cargo.lock`, `versions.toml` and the workflow file itself. It installs the
   `wasm32-unknown-unknown` target as `.github/workflows/rules-wasm.yml` does. Jobs:
   - (a) `wasm32` `cargo clippy -D warnings` for the web-capable crates
   - (b) Node tests: frame loop, input, audio protocol, rules sandbox, allocation guard, and
     determinism (10 replays)
   - (c) the web build with `wasm-bindgen` and `wasm-opt`, plus the size budget
   - (d) a headless-browser smoke test (report-only until A23 is confirmed)
   - (e) the native-to-web hash report, written to the job summary
2. **`.github/workflows/ci.yml`.** Unchanged in v1. If the owner later promotes a wasm32 check into
   `pr-checks`, the `PR_SET` and `run_pr_checks` arity in
   `tests/spec_ci/tests/spec_ci_yml.rs:735-843` change with it.
3. **`.github/workflows/rules-wasm.yml`.** If the web backend lives in `pin_script`, add its parity
   tests to the test step (`.github/workflows/rules-wasm.yml:130-134`). Its path filters already
   include `crates/pin_script/**` (`:18`, `:30`).
4. **`.github/workflows/release.yml`.** Optionally, a static-site zip artifact on dispatch or tag.
   No deployment by agents.
5. **`.github/workflows/spikes.yml`.** Covers `spikes/web_core` (new) automatically if it joins the
   spikes workspace. 9.0's browser parts run in the dispatch-only
   `.github/workflows/web-spike.yml` (new).
6. **`.github/workflows/soak.yml`.** Unchanged.

#### 10.5 `versions.toml` and the root `Cargo.toml`
1. **`[toolchain]`.** No version change: `rust = "1.97.0"` stays. Add a note that the web jobs need
   the `wasm32-unknown-unknown` target.
2. **`[crates]`.**
   - Already in `Cargo.lock` through wgpu: `wasm-bindgen =0.2.130`, `js-sys =0.3.107`,
     `web-sys =0.3.107`, `wasm-bindgen-futures =0.4.80`.
   - New: `wasm-bindgen-test`, at the version from 9.0.
   - If W1: `wasmparser =0.261.0` and `wasm-encoder =0.261.0`, the versions wasmtime 50.0.1
     resolves. If W2: the interpreter crate, at the version from 9.0.
   - `spikes/dep_smoke/tests/versions.rs` asserts that `versions.toml` matches the spikes lockfile.
     Each new pin therefore needs a spike in `spikes/` (`versions.toml:3-7`).
3. **`[features]`.** wgpu `webgpu` (and `webgl` only with 9.9), plus the `web-sys` feature list.
4. **`[notes]`.** The `wasm-bindgen` CLI must equal the crate version. Also record:
   - the binaryen (`wasm-opt`) and Node versions
   - the wasm32 target features and any `getrandom` backend flag, passed by `scripts/web/` (new)
     and not by a repo-wide `.cargo/config.toml`
5. **Root `Cargo.toml`.** The same pins in `[workspace.dependencies]`. `members` is unchanged,
   because `crates/*` picks up the new crates.

#### 10.6 Contracts (`pin_core`)
No contract change is required. Recorded here, **not filed** in `contracts/CHANGE_REQUESTS.md`
(whose last entry is CR-009):
- **CR-010 (draft, optional, additive, doc-only).** `InputEvent::timestamp_ns` is documented as "the
  platform's monotonic nanosecond clock: `SDL_GetTicksNS` natively, `performance.now()`-derived on
  the web" (`crates/pin_core/src/input.rs:121-125`).
- **Only if 9.12 (Worker) is pursued:** a stable byte layout for `FrameStateSnapshot`
  (`crates/pin_core/src/snapshot.rs:86-87`). That is breaking, so it can merge only between waves
  (D4).
- **Only if the single-thread `Send` wrapper proves unsound (9.5):** a `cfg`-dependent
  `TableScriptBackend: Send` bound (`crates/pin_core/src/script.rs:250`). That is breaking, between
  waves only.

#### 10.7 Other documents
- `README.md`: a web section.
- `docs/AUTHORING.md`: a table must ship `rules.wasm` to run on the web.
- `docs/HUMAN_ACCEPTANCE.md`: web checks, through `scripts/human_acceptance.py`.

### 11. Assumptions to verify (external, unverified)

Not checked against any file in this repository. Subphase 9.0 confirms or refutes each one that can
be tested.

| # | Assumption | How 9.0 checks it |
|---|---|---|
| A1 | wgpu 30.0.1 supports `wasm32-unknown-unknown` with a WebGPU backend (`webgpu` feature; unclear whether it is in the default set) and a WebGL2 backend (`webgl`); egui-wgpu 0.37 works on the web | Render probe |
| A2 | WebGPU ships in current desktop Chrome and Edge, and in recent Firefox and Safari releases; mobile support is partial | Browser matrix in the report |
| A3 | WebGPU allows read-only storage buffers in fragment shaders and fits `downlevel_defaults`; WebGL2 has no storage buffers or compute; WebGPU has no ray queries | Adapter limits dump |
| A4 | WebGPU: `rgba16float` is renderable, multisampled and blendable; canvas formats are `bgra8unorm`, `rgba8unorm` or `rgba16float`, with sRGB only through view formats | Render probe |
| A5 | WebGPU adapter, device, error-scope and buffer-map operations are asynchronous, with no blocking poll on the main thread; browsers compile WGSL with their own front ends | Render probe |
| A6 | On `wasm32-unknown-unknown`: `Instant::now()` panics; `std::thread::spawn` fails; `std::fs` and `std::net` compile but return errors; `Mutex` and `core` atomics work single-threaded | Build matrix plus a Node test |
| A7 | `wasm32-unknown-unknown` uses `panic=abort` on stable, so `catch_unwind` does not catch | Node test |
| A8 | Rust threads in the browser (shared memory, `+atomics`) need nightly `-Z build-std` | Toolchain check |
| A9 | SharedArrayBuffer and high-resolution timers need COOP/COEP cross-origin isolation; GitHub Pages cannot set these headers; `Atomics.wait` is allowed in Workers only | Documented check |
| A10 | mlua 0.12.2 and mlua-sys 0.13.0 support Emscripten but not `wasm32-unknown-unknown`; wasm-bindgen and web-sys do not support Emscripten; SDL3 targets Emscripten, not `wasm32-unknown-unknown`; pure-Rust Lua VMs are incomplete or lack these semantics | Build matrix |
| A11 | wasmtime cannot run inside a browser; browsers isolate the renderer process per site | Documented check |
| A12 | Wasm float arithmetic is IEEE-754 deterministic except for NaN bit patterns; core wasm has no FMA; relaxed SIMD is non-deterministic; rustc's wasm32 target enables neither `simd128` nor relaxed SIMD by default; browsers do not canonicalise NaNs | Physics probe; target-feature dump |
| A13 | glam 0.33.12 and `wide` use different SIMD and scalar paths on x86_64 and wasm32, and parry3d's internal math resolves to different libm code per target, so native and web float results can differ | Physics probe |
| A14 | `getrandom 0.4.3` (through `zip 9.5.1`) needs a `wasm_js` backend configuration to compile for `wasm32-unknown-unknown` | Build matrix |
| A15 | `performance.now()` is coarsened (about 100 µs, 5 µs when isolated, engine-dependent); `event.timeStamp` shares its timeline; compositors add about a frame of latency | Browser test |
| A16 | `requestAnimationFrame` stops in hidden tabs and timers are throttled | Browser test |
| A17 | The Gamepad API is polled, has a "standard" mapping and `Gamepad.timestamp`, exposes no SDL GUID, and `vibrationActuator` is not on every browser | Browser test |
| A18 | AudioWorklet renders in 128-frame quanta; an `AudioContext` needs a user gesture to start; typical output latency is 10 to 40 ms | Browser test |
| A19 | A pure-Rust Wasm interpreter with fuel metering, memory and stack limits compiles to wasm32 and runs at most 100 000 fuel units per callback well inside a frame | Rules probe |
| A20 | Browser engines compile wasm off the main thread with `WebAssembly.compile`; synchronous compilation of large modules on the main thread is restricted in some engines | Rules probe |
| A21 | A CSP needs `'wasm-unsafe-eval'` to compile wasm modules | Documented check |
| A22 | The `wasm-bindgen` CLI must match the `wasm-bindgen` crate version exactly | Build |
| A23 | GitHub-hosted runners can run headless Chrome or Firefox with a (software) WebGPU adapter | Spike workflow |
| A24 | The engine `.wasm` (wgpu, naga, egui, parry3d, gltf, image) is in the 5 to 15 MiB range before compression and a few MiB with brotli | Size probe |
| A25 | Wasm exception handling (needed for `setjmp`/`longjmp` in route (a)) is available in current browsers | Only if 9.10 |
| A26 | `KeyboardEvent.code` maps one-to-one to USB HID usages, which equal SDL scancodes, for every key in the shipped profiles | Mapping table test |
| A27 | Wasm sim ticks cost 1 to 3 times native; streaming round trips are tens of ms or more | Physics probe |
| A28 | The Rust shadow stack on wasm32 is a fixed linear-memory region set at link time (default about 1 MiB) and can be raised with a linker flag | Build plus a Node test |

### 12. Choices made while drafting

1. **No changes to `docs/DECISIONS.md`.** It is frozen (D41). The draft decision rows are written
   for the PLAN §0 table instead (10.2).
2. **Numbered "Phase 9" with Waves 10 to 14, not "Wave W".** Waves 0 to 9 are complete
   (`docs/RUN_LOG.md:499`). Unit ids must be lowercase in decision-file names
   (`spikes/spec_0_1/tests/spec_log_split.rs:43`).
3. **Wasm-only rules on the web recommended.** The vendored `lua-src` cannot build for the target,
   and all three tables ship parity-tested `rules.wasm`. D10 keeps Lua primary on native, so this
   is a web-only scope rule, not a reversal.
4. **Main-thread loop recommended over a Worker.** It keeps D5, D7 and D8 as they are, and needs no
   hosting headers.
5. **Native-to-web hashes report-only.** D9's wording on cross-platform hashes is extended as is.
6. **WebGPU-only for v1.** D18 and D33 already make the probe path the stand-alone baseline. The
   storage-buffer use makes WebGL2 a separate piece of work.
7. **A separate `web.yml` workflow.** Follows the 6.3 and 7.1 precedent
   (`docs/decisions/2026-11-19-6.3.md`, "a new workflow file instead of a ci.yml job";
   `docs/decisions/2026-11-19-7.1.md`, "new workflow file").
8. **T2 by default, no T3.** D34 (b) is not met for any subphase.
9. **"Difficulty" means low, medium or high.** The PLAN has no such label today. Session counts are
   rough estimates from Waves 6 to 9.
10. **Evidence from `Cargo.lock`.** `cargo tree --offline` could not resolve the registry. The
    `rules.wasm` facts come from a scratch section and opcode scan that was not committed; 9.0
    reproduces it with `wasmparser`.
11. **The queue line uses the single-line format** of, for example,
    `docs/queue/fix-11-telemetry-min-ring.md`.
