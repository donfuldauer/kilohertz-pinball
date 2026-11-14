# Kilohertz Pinball: Technical Architecture & Agent Execution Plan (v5)

> **Run mode: unattended.** One human setup step before the run (§2.6, "Prerequisites"), one human acceptance step after it (Phase 8). Nothing in between waits on a person. Every subphase validates itself through the subphase gate in §2.6.

## 0. What Changed From v1 (Decision Log)

Every open choice from the v1 audit is settled here. Agents treat this table as binding.

**v3 changes over v2:** every feature v2 deferred is back in scope with its own subphase (see "Restored features" below), and the library assumptions have been checked against current releases (§6).

**v4 changes over v3:** built for unattended execution. Adds the orchestrator runbook (§2.6), written-out schemas and defaults (Appendices A–C), a self-contained brief template (Appendix D), a dependency-spike subphase (0.1), an autoplay soak harness (2.9), a placeholder asset kit (4.6), automated gates A1 and A2 in place of the mid-run human gates, and a single human acceptance step at the end (Phase 8).

**v5 changes over v4 (from Wave 6 on):** Fable 5.2 is no longer a default model. Every unit of work is classified and routed to the cheapest tier that can do it (Haiku 5.5, Sonnet 5.5 or Opus 5.5), and Fable 5.2 is used only for very-high-blast-radius or extremely hard-to-troubleshoot work **and** only when the account's current usage limits leave enough Fable allowance (D34, §2.5). The Implementer / Independent Tester / Reviewer trio becomes four agents defined in `.claude/agents/`: Work Classifier, Task Executor, Work Verifier and Issue Investigator (D35, §2.6). Agents no longer review their own code: each unit of work becomes a pull request against `main`, CodeRabbit reviews it, findings are fixed automatically, and the Orchestrator merges once every finding is accounted for and every thread is resolved (D36, §2.7). Rows for Waves 0–5 in the run log and DECISIONS.md describe the v4 process and stay as they are.

| # | Topic | Decision | Why |
|---|---|---|---|
| D1 | Subphase count | **37 subphases** in 10 waves (0–9). Waves 0–5 (27 subphases) ran as Opus 5.5 × 23, Fable 5.2 × 4 (2.1, 2.2, 2.3, 2.6). Later work is routed per D34 | v1 said 18 but listed 20; v2 added CI-first, audio, headless sim, table generator and a grey-box milestone; v3 adds one subphase per restored feature |
| D2 | Wave ordering | Rebuilt so no subphase shares a wave with its own dependency | v1 had 3.3/4.3 and 7.2 in the same wave as their prerequisites |
| D3 | Same-crate parallelism | **Module-level ownership** inside shared crates; `mod` lines pre-stubbed by the earlier subphase | Prevents merge conflicts in `pin_physics` and `pin_render` |
| D4 | Contracts | **All cross-crate types live in `pin_core`**, changed only through the contract-change protocol (§2.3) | v1 had `TableScriptBackend` in `pin_script` while requiring contracts in `pin_core` |
| D5 | Hot-loop allocation | `HostCommandQueue` uses fixed-capacity `ArrayVec` and interned `u16` IDs; enforced by a panicking allocator guard | v1 queue used `Vec`/`String` in a no-alloc loop |
| D6 | Threading | **Main thread = SDL event pump only**; separate render, sim, audio and I/O worker threads | SDL's event pump is main-thread-only, so v1's sim-thread polling could not work |
| D7 | Snapshot handoff | Render uses the **latest snapshot, no interpolation** | A triple buffer exposes one snapshot; at 1 kHz staleness is ≤ 1 ms |
| D8 | Timing tests | Headless runs on **simulated time**; wall-clock accuracy is a non-gating benchmark | Wall-clock assertions flake on CI runners |
| D9 | Determinism | **Same-build determinism is the gate.** Cross-OS equality is reported, not required | `f32` math differs across platforms/compilers |
| D10 | Scripting | **Lua 5.4** (vendored via `mlua`) is the primary backend with hot reload. **Wasm (`wasmtime`) restored** in Subphase 4.5, table ports in 6.3 | LuaJIT dropped for its FFI attack surface. Lua/Wasm parity is made testable by keeping rules logic integer-only (§3.4) |
| D11 | Script API | Read access via `HostCtx`; flipper/action/start buttons are ordinary cabinet switches; scripts own scores | v1 API was write-only and special-cased the action button |
| D12 | Ball-kind detection | Eddy sensors simply **do not close** for non-conductive balls; `detected_kind` removed | v1 specified both mechanisms |
| D13 | Splines | **C² interpolating cubic splines**, stored as cubic Bézier segments; wireforms modelled as **two rails** | Catmull-Rom is only C¹, causing normal-force jumps at knots |
| D14 | Magnetism | `magnetic_coupling ∈ [0,1]` plus `conductive: bool`. **Remanence and `low_rem_steel` restored** with an explicit residual-force model (Subphase 2.7) | v1 "μr" was a flag, not permeability; v1 remanence had no equation, v3 gives it one |
| D15 | Magnus effect | **Restored** as a per-profile coefficient, default 0 for steel (Subphase 2.7) | Negligible for steel but visible on the 24 g Glo-Ball; kept out of the core solver so it cannot destabilise it |
| D16 | Flipper model | Torque-based with explicit moment of inertia; two-body impulse exchange | Catches must emerge from physics, not special cases |
| D17 | Units | **SI internally** (m, kg, s, rad). TOML uses mm/g/deg, converted at load | v1 mixed mm, m/s and N |
| D18 | Reflections | Octahedral probes with a fixed update budget are the baseline on every GPU. **Ray queries restored** as an optional, runtime-detected path (Subphase 5.4) | wgpu ray queries are experimental, so the probe path must always work on its own |
| D19 | Presentation | Playfield surface paces the frame; secondary surfaces pick a non-blocking mode from the surface's reported capabilities, else get their own presenter thread | Three vsynced surfaces on one thread throttle to the slowest display; only `Fifo` is guaranteed everywhere (§6) |
| D20 | GPU tests | Separate CI job on software rasterizers (lavapipe / WARP) and macOS Metal runners | Keeps physics/input/script tests GPU-free |
| D21 | Audio | New subphase for a real mixer; crate renamed `pin_audio` | v1 had no consumer for `audio_triggers` |
| D22 | Cabinet output | Documented UDP telemetry protocol. **DOF bridge restored** as a Windows companion process (Subphase 3.5) | DOF is a .NET plugin fed table-element events, not a UDP consumer, so it needs a bridge |
| D23 | Sequencing | CI in Wave 2, headless sim + replay in Wave 5, **playable grey-box in Wave 6** | Regression tooling and a playable slice must exist before table authoring |
| D24 | Table geometry | Procedural generator (`pin_tablegen`), validated by an autoplay bot and automated gates A1/A2; human playtest moves to the end | Agents cannot judge feel, but they can prove a table is free of traps, leaks and unreachable shots |
| D25 | Security | Sandboxed Lua stdlib, instruction budget, memory cap, hardened zip extraction | Tables are untrusted downloads |
| D26 | Model routing | *Superseded by D34 from Wave 6.* v4: 5.2 stayed with Opus 5.5; 5.4 and 2.6 went to Fable 5.2; every subphase got a cross-model review | The numerically hard parts of v1's 5.2 and 2.3 now live in 5.4 and 2.6 |
| D27 | Self-validation | An **Independent Tester** task (writes tests from the brief without seeing the code) for every subphase; it no longer has to use a different model. The v4 Reviewer role is replaced by the Work Verifier and CodeRabbit (D35, D36) | Tests written by the implementer alone tend to confirm the implementation rather than the spec |
| D28 | Human involvement | Setup before the run, acceptance after it, nothing between. Ambiguity is resolved by the agent, logged, and reviewed at the end | Requirement: run as unattended as possible |
| D29 | Failure policy | Three fix attempts, then an Issue Investigator pass and one attempt one tier up (never into Fable unless D34 holds), then mark blocked. A blocked restored feature is skipped; a blocked core subphase halts only its dependents | The run should degrade, not stop |
| D30 | Image tests | Analytic pixel assertions first, regression snapshots second, the Work Verifier's visual checklist third | A golden image generated by the code under test proves nothing on its own |
| D31 | Hardware | Simulated in CI (virtual joysticks, recorded sensor traces, mock displays, loopback UDP). Physical checks are listed for final acceptance, never gated | No cabinet is attached to CI |
| D32 | Assets | A code-generated, licence-clean placeholder kit (sounds, fonts, art) with stable IDs | Tables must be complete without sourcing third-party assets; real assets drop in later by ID |
| D33 | Unverifiable features | Ray-query reflections ship **off by default** unless a ray-tracing-capable runner validated them; the DOF bridge ships marked "unverified on hardware" | Honest defaults for anything CI could not exercise |
| D34 | Fable 5.2 use | **Exception only.** Work is classified into tiers T0 Haiku 5.5, T1 Sonnet 5.5, T2 Opus 5.5 and T3 Fable 5.2. T3 needs (a) a very-high blast radius or an extremely hard-to-troubleshoot issue, with evidence; (b) a reason T2 is not enough; and (c) enough Fable allowance under the account's current usage limits. Unknown allowance counts as not enough; the work then runs on Opus 5.5 and a line goes to `docs/HUMAN_QUEUE.md` | Fable allowance is scarce and Opus 5.5 handled most of Waves 0–5's hard numerics; the run must not stall or exhaust limits waiting for Fable |
| D35 | Agent roles | Four agents in `.claude/agents/`: **work-classifier** (classifies, picks the tier, writes the delegation brief), **task-executor** (implements, writes spec tests, fixes), **work-verifier** (requirements traced, coverage, passing tests; no style review) and **issue-investigator** (reproduces and root-causes reported issues, judges review findings) | Clear hand-offs with full briefs; cheap models do cheap work |
| D36 | Code review | **No agent reviews its own or a sibling's code.** Every unit of work is a PR against `main`; CodeRabbit reviews it; auto-fix is on from the moment the PR opens; the Orchestrator merges, without waiting for a person, once CI is green and every finding is accounted for with its thread resolved (§2.7) | An external reviewer finds different defects than the model that wrote the code, and the run stays unattended |

### Restored features (each isolated in its own subphase)

| Feature | Subphase | Model | Isolation |
|---|---|---|---|
| Curved "Banana" flippers | 2.6 | Fable 5.2 (complete) | New `FlipperShape` variant behind the `BatShape` trait |
| Magnus effect, remanence, `low_rem_steel` | 2.7 | Opus 5.5 | External-force hooks; zero coefficients reproduce the core solver bit-for-bit |
| Jumbo balls up to 79.4 mm | 2.8 | Opus 5.5 | New profile plus a table clearance validator |
| DOF bridge | 3.5 | Opus 5.5 | Separate Windows process fed by UDP telemetry |
| Wasm scripting backend | 4.5 | Opus 5.5 | Second `TableScriptBackend` implementation |
| Hardware ray-query reflections | 5.4 | Opus 5.5 (Fable 5.2 only under D34) | Second `ReflectionSource` implementation, probes as fallback |
| Backbox mini-playfield, vertical flippers, 3D toys | 5.5 | Opus 5.5 | Collision layer bit 4 and a backbox render pass |
| Wasm ports of table rules | 6.3 | Opus 5.5 | After Gate A2, when rules are frozen |
| Playable showcase of the above | 6.4 | Opus 5.5 | `tables/feature_lab`, so autoplay soak and final acceptance cover them |

The core path (Lua, straight flippers, probes, UDP telemetry) never depends on a restored feature, so a problem in one of these subphases cannot block the reference tables.

---

## 1. Executive Summary

**Kilohertz Pinball** is a cross-platform pinball simulation engine for **Windows**, **Linux** and **macOS**, built by autonomous coding agents in 10 waves (0–9) with no human input between setup and final acceptance.

* **Language & OS layer:** Rust (2024 edition) workspace; `SDL3` for windows, display enumeration, input events and audio output.
* **Graphics:** `wgpu` (Vulkan, DX12, Metal) from a single WGSL codebase.
* **Simulation:** $1{,}000\text{ Hz}$ fixed timestep ($\Delta t = 1\text{ ms}$) on a dedicated thread; render thread reads the latest snapshot through a lock-free triple buffer.
* **Physics:** `parry3d` BVH and shape-casts, C² spline colliders, custom impulse/spin/friction solver, torque-driven flippers.
* **Balls:** Data-driven `BallProfile`: standard and low-remanence steel (80 g), ceramic Powerball (65 g), UV Glo-Ball (24 g), mini (15.88 mm), jumbo (31.75 mm to 79.4 mm), captive Newton balls.
* **Flippers:** Any count on any 3D plane, including vertical backbox playfields; straight-capsule and Curved "Banana" bats; Zipper slides; automated "Thing Flips."
* **Tables:** Bundle (`.pinbundle` zip or folder) of `glTF 2.0`, `TOML` manifests and sandboxed rules in **Lua 5.4** (hot reload) or **WebAssembly** (`wasmtime`).
* **Rendering:** PBR with per-ball reflection probes, plus hardware ray-query reflections where the GPU supports them.
* **Cabinet:** Single-window PiP or up to three monitors (playfield, backglass, DMD/LCD); X-Arcade profiles; rumble; 7.1 SSF; UDP telemetry; DOF bridge on Windows.

---

## 2. Workspace, Threads & Agent Protocol

### 2.1 Cargo Workspace

```text
kilohertz-pinball/
├── Cargo.toml
├── contracts/CHANGE_REQUESTS.md   # Contract-change queue (§2.3)
├── physics_defaults.toml          # Global tunables (Appendix C); data-only tuning happens here
├── docs/
│   ├── RUN_LOG.md                 # Per-subphase status, attempts, gate results
│   ├── DECISIONS.md               # Every ambiguity an agent resolved, with rationale
│   ├── HUMAN_QUEUE.md             # Items for a person to look at after the run
│   └── HUMAN_ACCEPTANCE.md        # Final checklist, generated by Subphase 7.2
├── spikes/                        # Subphase 0.1 dependency probes
├── crates/
│   ├── pin_core/       # ALL shared contracts: IDs, schemas, snapshot, triple buffer, traits
│   ├── pin_physics/    # world/ · ball/ · ball_ext/ (Magnus, remanence) · flipper/ · flipper_banana/ · mech/ · debug/
│   ├── pin_input/      # events/ · staging/ · profiles/ · calibration/ · haptics/
│   ├── pin_script/     # lua/ (Lua 5.4, sandbox, hot reload) · wasm/ (wasmtime) · guest-sdk/
│   ├── pin_assets/     # VFS, zip hardening, glTF loader, TOML validation
│   ├── pin_tablegen/   # layout/ → .glb · clearance/ · backbox/
│   ├── pin_assetgen/   # Code-generated placeholder sounds, fonts and art
│   ├── pin_sim/        # Headless Simulation: tick order, command application, record/replay · autoplay/
│   ├── pin_render/     # topology/ · gpu/ · debug/ · pbr/ · reflections/ · raytrace/ · post/ · displays/ · backbox/ · ui/
│   ├── pin_audio/      # mixer/ · ssf/ · telemetry/
│   └── pin_app/        # Binary: platform/ · orchestrator/ · ui/ · profiler/
├── tools/
│   └── dof_bridge/     # Windows-only .NET companion: UDP telemetry → DirectOutput Framework
└── tables/
    ├── greybox/        # Milestone M1: untextured test table
    ├── apex_circuit/   # Variant A
    ├── sky_deck/       # Variant B
    └── feature_lab/    # Showcase for Banana flipper, backbox playfield, XL ball, Magnus, remanence
```

### 2.2 Thread Model (D6, D7, D19)

| Thread | Owns | Notes |
|---|---|---|
| **Main** | SDL init, windows, event pump | Blocks in `SDL_WaitEventTimeout(1 ms)`. Forwards every input event, with SDL's nanosecond timestamp, into an SPSC ring to the sim thread. Forwards window events to the render thread. Does no rendering. |
| **Sim (1 kHz)** | `pin_sim::Simulation` | Elevated priority. Hybrid sleep + spin timer. Accumulator with at most 8 catch-up ticks, then drops time. |
| **Render** | `wgpu` device, queue, all surfaces | Surfaces are created on main and handed over. Reads latest `FrameStateSnapshot`. Playfield surface uses `Fifo` (or VRR). Backglass and score surfaces redraw only on change and choose from `SurfaceCapabilities::present_modes`: `Mailbox`, else `Immediate`; if only `Fifo` is offered, that surface gets its own presenter thread so it cannot stall the playfield. |
| **Audio** | SDL audio stream callback | Receives triggers and haptic events through lock-free rings. |
| **I/O worker** | File watching, asset loading, script source loading | The only thread allowed to touch the filesystem at runtime. |

**Tick order inside `Simulation::step_1ms()`** (fixed, deterministic):
1. Drain input events with timestamp ≤ tick time; update cabinet switch states.
2. Deliver switch edges collected last tick to the script, then call `on_tick`.
3. Apply `HostCommandQueue` (coils, magnets, lamps, spawns), then clear it.
4. Step physics (CCD sub-steps inside the tick).
5. Collect new switch edges and haptic events.
6. Publish `FrameStateSnapshot`.

### 2.3 Agent Execution Rules

1. **Crate and module boundaries.** An agent edits only its target crate and module directory. Where two subphases share a crate in one wave, each owns a module directory and the earlier subphase pre-stubs the `mod` declarations.
2. **Contract-change protocol (D4).** Only Subphase 1.1 writes `pin_core` freely. Any later agent that needs a contract change appends a request to `contracts/CHANGE_REQUESTS.md` and submits it as a separate `pin_core`-only PR. Additive changes merge immediately; breaking changes merge only between waves.
3. **GPU-free core tests.** `pin_core`, `pin_physics`, `pin_input`, `pin_script`, `pin_assets`, `pin_tablegen` and `pin_sim` pass `cargo test` with no display server and no GPU. Render tests live behind the `gpu-tests` feature and run in their own CI job (D20).
4. **No blocking on the sim thread (D5).** No heap allocation, mutex, or file I/O in `step_1ms()` in Rust engine code. Tests wrap the tick in a panicking-allocator guard (e.g. `assert_no_alloc`). Two explicit exemptions: the Lua VM allocates from its own capped, pre-reserved arena; and a hot-reload swap tick may allocate and overrun (developer-only event).
5. **Simulated time in tests (D8).** Headless tests never sleep and never assert on wall-clock time.
6. **No test-shaped special cases.** Solver code must not branch on named behaviours ("live catch", "powerball"). Tests assert emergent results across parameter sweeps.
7. **Agent roles (D34, D35).** Every unit of work is classified by the work-classifier, done by a task-executor (as Implementer, Independent Tester or Fixer), checked by the work-verifier, and diagnosed by the issue-investigator when something fails for a reason that is not obvious (§2.6). The model is whatever the classifier picked (§2.5).
8. **Never wait on a person (D28).** When the brief is ambiguous, choose the option most consistent with the decision log, record it in `docs/DECISIONS.md`, add a line to `docs/HUMAN_QUEUE.md`, and continue.
9. **No self-review (D36).** No agent reviews code for style or design, its own or another agent's. The work-verifier checks objective facts only; code review happens on the pull request by CodeRabbit (§2.7).

### 2.4 Determinism Scope (D9)

* **Gate:** the same binary on the same machine replays any recording to identical ball state, switch-hit counts and scores.
* **To stay close across platforms:** no fast-math flags, no FMA contraction, `libm` for transcendental functions, fixed iteration order everywhere, a sim-owned seeded PCG for all randomness.
* **Report only:** CI records cross-OS replay hashes and flags divergence without failing the build.

### 2.5 Model Routing (D34)

Every unit of work (a subphase, an Independent Tester task, a fix task, a CodeRabbit finding, a CI or gate failure) goes through the **work-classifier** before anyone works on it. The classifier rates blast radius and difficulty and picks the lowest tier that can do the job:

| Tier | Model | Typical work |
|---|---|---|
| T0 | Haiku 5.5 | Docs-only edits, run-log and queue lines, formatting, renames, a review nit whose fix the comment spells out exactly |
| T1 | Sonnet 5.5 | Well-specified changes in one module, adding tests, CI and workflow tweaks, most review findings, verification runs |
| T2 | Opus 5.5 | New subphase implementations, numerics, solvers, render pipelines, concurrency, cross-module changes, ambiguous work, every investigation |
| T3 | Fable 5.2 | Only under the Fable rule below |

The **Model** column in Phase 4 and §5 is the expected tier for the Implementer; the classifier may route lower, and each fix or finding is classified on its own.

**Fable rule.** Fable 5.2 is used only when all three hold:
1. **Need.** The work has a *very-high blast radius* (a breaking `pin_core` contract change used by three or more crates; a change to simulation output for every table, such as solver numerics in `pin_physics` world/ball/flipper, the sim tick or the triple buffer; a change to the untrusted-table security boundary, such as the Lua/Wasm sandbox, budgets, memory caps, zip extraction or the vendored Lua patches; or a change one revert cannot undo), **or** it is *extremely hard to troubleshoot* (two full issue-investigator passes on Opus 5.5 found no reproducible root cause, or Opus 5.5 used up the D29 fix attempts, or the failure is non-deterministic below the code's control and two Opus passes did not isolate it).
2. **Opus is not enough.** The classifier names the T2 attempt that failed, or says exactly why one would.
3. **Allowance.** Just before routing, the Orchestrator reads the account's current usage limits (the Claude Code `/usage` readout or the latest rate-limit status the session reports) and finds enough Fable allowance left in the current window for the estimated task, with headroom, without going into overage. If the allowance cannot be read, it counts as not enough.

If 1 and 2 hold but 3 does not, the work runs on Opus 5.5 and a line goes to `docs/HUMAN_QUEUE.md`; the run never waits for allowance to return. If a Fable session is throttled or falls back mid-task, the work continues on Opus 5.5. Every Fable use is logged in `docs/RUN_LOG.md` with the classifier's evidence for 1–3.

**Verification and investigation** run on Sonnet 5.5 and Opus 5.5 respectively, by default. Neither needs to differ from the implementing model.

### 2.6 Orchestrator Runbook

A single long-running **Orchestrator** agent (the main session) owns the wave table (§5), routes every unit of work through the work-classifier, spawns the agents the classifier names with the model it picks, opens and drives the pull requests (§2.7), merges, and keeps `docs/RUN_LOG.md` current. It writes no product code and does not review code.

**Prerequisites (the only human step before the run).**
1. A GitHub repository with Actions enabled on Windows, Linux and macOS hosted runners.
2. Branch protection on `main`, and agent credentials with write access to branches and pull requests, including merging.
3. The CodeRabbit GitHub app installed on the repository, configured by `.coderabbit.yaml`. Its `path_filters` keep binary, generated, vendored-build and recorded-data files (images, audio, fonts, models, compiled Wasm, lockfiles, recorded traces and expected-output dumps) out of review; source, shaders, table rules, manifests, CI and docs are reviewed.
4. Optional: a self-hosted runner with a ray-tracing-capable GPU. Without it, D33 applies.

**Agents (D35).** Defined in `.claude/agents/`. Subagents cannot start subagents, so the Orchestrator does every spawn, passing the classifier's brief verbatim and its model as the `model` override.
* **work-classifier** (`work-classifier.md`, Sonnet 5.5): rates blast radius and difficulty, picks the tier (§2.5), names the agent, and writes the complete delegation brief (Appendix D). Runs before every delegation.
* **task-executor** (`task-executor.md`, model from the classifier): does the work as one of three variants. *Implementer* builds the deliverables and their unit tests. *Independent Tester* receives only the brief, never the implementation, and writes black-box tests against the public contract under `tests/spec_*`, including property tests for stated invariants; it runs in parallel with the Implementer. *Fixer* applies one CodeRabbit finding, verifier failure or investigator plan.
* **work-verifier** (`work-verifier.md`, Sonnet 5.5): confirms each requirement has a passing test, re-runs fmt, clippy, every applicable test suite and coverage, checks owned paths, the sim-thread rules and, for render work, the visual checklist. It gives no opinion on style or design.
* **issue-investigator** (`issue-investigator.md`, Opus 5.5): reproduces and root-causes any failure whose cause is not obvious (CI, verifier, flaky tests, wrong output) and judges CodeRabbit findings of unclear validity. It returns a fix plan and a thread reply; it does not fix.

**Flow per unit of work.** classify → execute (Implementer and Independent Tester in parallel for a subphase) → verify → on `fail`, investigate if the cause is unclear, then classify and execute the fix → verify again → open the PR (§2.7) → CodeRabbit findings go through the same loop until the merge conditions hold.

**Briefs.** The work-classifier writes each brief from Appendix D so it is self-contained: the subphase section, §2.3, the relevant parts of §3 and the appendices, and the owned paths. Role agents never need this whole document.

**Branches.** `main` is protected. Each unit of work runs on its own branch from the latest `main` (`sp/<id>`, `fix/<id>`, or the branch the harness assigns) and reaches `main` only through its own pull request (§2.7). A wave is complete when all its PRs are merged and the wave gate passes on `main`; that commit is tagged `wave-N-green`. Wave branches are no longer used.

**Subphase gate (definition of done).** All must hold:
1. `cargo fmt --check` and `clippy -D warnings` are clean.
2. Implementer tests and every listed verification criterion pass.
3. Independent Tester's spec tests pass. A disagreement between spec tests and implementation is resolved against the brief, not by deleting the test.
4. Line coverage (`cargo-llvm-cov`): at least 85 % for `pin_core`, `pin_physics`, `pin_input`, `pin_script`, `pin_assets`, `pin_tablegen`, `pin_sim`; at least 60 % for `pin_render`, `pin_audio`, `pin_app`.
5. Property tests (`proptest`) cover every solver invariant and numeric helper; every parser (TOML manifests, zip, glTF extras, UDP packets, replay files) survives a bounded random-bytes run without panicking.
6. For anything on the sim thread: allocation guard passes and a replay hash is stable over 10 runs.
7. An automated path check confirms the diff touches only owned paths.
8. The work-verifier reports `pass`. The subphase is then *done* only when its PR is merged under §2.7.

**Wave gate.** Full workspace tests on all three operating systems; replay hashes stable; every earlier wave's tests still pass; tick-time benchmark has not regressed by more than 10 %.

**Failure policy (D29).**
1. The executor gets up to three fix attempts, each from a classifier brief built on the verifier's failures, the investigator's report or the CodeRabbit findings.
2. Then the issue-investigator makes a fresh pass, and the classifier routes one more attempt one tier up from a fresh brief plus the failure history. This can reach Fable 5.2 only if D34 holds, including the allowance check; otherwise it stays on Opus 5.5.
3. Then the subphase is marked `blocked` in the run log and added to `docs/HUMAN_QUEUE.md`.
4. A blocked *restored* subphase (2.6, 2.7, 2.8, 3.5, 4.5, 5.4, 5.5, 6.3, 6.4) is skipped along with its dependents; the run continues.
5. A blocked *core* subphase halts only its dependents. All independent work continues, and the run ends with a partial result and a precise account of what is missing.

**Image test policy (D30).** Every render test uses, in order: (a) analytic assertions on pixels, such as region mean colour, luminance ratios, symmetry and energy conservation; (b) a regression snapshot, stored only after (a) passes, with per-backend tolerance; (c) the work-verifier's visual checklist, logged. A snapshot is never the sole evidence.

**Hardware simulation policy (D31).** Gamepad-mode devices are SDL virtual joysticks; keyboard-encoder devices are synthetic key events; plunger and accelerometer inputs are recorded traces stored as fixtures; displays are mocked; audio renders offline to buffers; telemetry uses loopback UDP. Anything that needs a physical device goes into `docs/HUMAN_ACCEPTANCE.md`.

**Automated gates.** Gate A1 (after Wave 6) and Gate A2 (after Wave 7) are defined in Phase 6. A failing gate opens fix tasks against the owning subphases under the failure policy; it never pauses for a person.

### 2.7 Pull Request, Review & Merge Workflow (D36)

Agents do not review their own code. Review comes from CodeRabbit on a pull request, and the Orchestrator resolves it without a person.

1. **Open.** When the work-verifier passes, the Orchestrator pushes the branch and opens a PR against `main`. The body follows the repository's PR template if there is one, and otherwise lists: deliverables with the test that proves each one, gate results with numbers, decisions logged, contract-change requests, and every Fable use with its D34 evidence. Bot names in the body go in backticks so nobody is notified by accident.
2. **Auto-fix on.** Right after opening, the Orchestrator subscribes to the PR's activity (comments, reviews, CI). From then on every CodeRabbit comment and every CI failure is handled as it arrives; nobody polls or sleeps waiting for them.
3. **Every finding is accounted for.** For each CodeRabbit comment (inline, nitpick, outside-diff or summary item), the Orchestrator sends the finding to the work-classifier, or to the issue-investigator first when its validity or cause is unclear. Each one ends in exactly one disposition, recorded as a reply on its thread:
   * **Fixed:** an executor commit addresses it, the verifier re-runs the affected checks, and the reply names the commit. The thread is then resolved.
   * **Not a defect:** the reply gives the evidence (test output, the DECISIONS.md section). The thread is resolved once CodeRabbit accepts or no longer pursues it; if it argues back with new information, the finding goes round again.
   * **Deferred:** real but out of scope, or it changes simulation output and needs a human ruling. The reply links the `docs/HUMAN_QUEUE.md` line (committed on the PR), then the thread is resolved.

   No thread is resolved without a reply. Nitpicks are fixed when cheap (T0/T1), otherwise answered as above.
4. **Re-review.** Each push triggers CodeRabbit's incremental review; new findings go through step 3. If CodeRabbit has not reviewed the head commit (for example it is rate-limited), the Orchestrator asks it once with a PR comment `@coderabbitai review`, waits as long as it says, and asks again at most twice. If it still has not reviewed, the PR stays open, a line goes to `docs/HUMAN_QUEUE.md`, and the run continues with work that does not depend on this PR.
5. **Merge conditions.** The Orchestrator merges, without waiting for a person, when all of these hold on the current head commit:
   * every required CI check is green;
   * CodeRabbit has finished reviewing the head commit, with no actionable comments left unanswered and no outstanding change request;
   * every review conversation has a disposition reply and is resolved, where the thread type allows it;
   * the branch is up to date with `main` (if not, update it and wait for CI again);
   * the work-verifier's last result is `pass` for the head commit.

   Then the Orchestrator merges with a merge commit, deletes the branch, unsubscribes from the PR, and adds a row to `docs/RUN_LOG.md` with the PR number and the merge commit.
6. **Never** to get green: skip, ignore, delete or weaken a test; force-push or rewrite history on a branch it did not create; push an empty commit; close and reopen a PR to restart CI; or merge with a failing required check.

---

## 3. Core Specifications & Contracts (all in `pin_core`)

### 3.1 IDs and Units

All runtime identifiers are interned `u16` newtypes resolved from names at table load: `SwitchId`, `CoilId`, `LampId`, `MagnetId`, `DiverterId`, `FlipperId`, `DispenserId`, `ProfileId`, `SoundId`, `SceneId`, `PersistKey`. No `String` crosses into the sim thread.

Internal units are SI (D17). `table.toml` uses mm, g and degrees; `pin_assets` converts on load.

### 3.2 Ball Profiles (D14)

| Profile ID | Diameter | Mass | Base restitution | `magnetic_coupling` | `remanence` | `conductive` | `magnus_coeff` | Shader |
|---|---|---|---|---|---|---|---|---|
| `standard_steel` | 27.0 mm | 80.0 g | 0.65 | 1.0 | 0.05 | yes | 0 | `pbr_chrome_steel` |
| `low_rem_steel` | 27.0 mm | 80.0 g | 0.65 | 1.0 | 0.00 | yes | 0 | `pbr_chrome_steel` |
| `ceramic_powerball` | 27.0 mm | 65.0 g | 0.78 | 0.0 | 0.00 | no | 0 | `pbr_ceramic_pearl` |
| `uv_globall` | 27.0 mm | 24.0 g | 0.84 | 0.0 | 0.00 | no | provisional (App. C) | `pbr_uv_emissive` |
| `mini_steel` | 15.88 mm | 16.5 g | 0.65 | 1.0 | 0.05 | yes | 0 | `pbr_chrome_steel` |
| `jumbo_steel` | 31.75 mm | 130.0 g | 0.60 | 1.0 | 0.05 | yes | 0 | `pbr_chrome_steel` |
| `jumbo_xl_resin` | 79.4 mm | 445 g | 0.80 | 0.0 | 0.00 | no | 0 | `pbr_ceramic_pearl` |

Base restitution is a fallback; the material-pair table (steel-rubber, ceramic-rubber, plastic-wood, steel-steel, …) takes precedence. Friction coefficients come only from the pair table.

The 79.4 mm ball is specified as phenolic resin (density ≈ 1.7 g/cm³, hence 445 g). A solid steel ball of that size would weigh about 2.04 kg; the engine permits it, but coil torques and table clearances must be authored for it. `remanence` and `magnus_coeff` are consumed only by Subphase 2.7; with both at zero the core solver is unchanged.

**Governing equations**
* Moment of inertia: $I = \tfrac{2}{5} m r^2$
* Contact slip velocity at point $\vec{p}_c$: $\vec{v}_c = \vec{v}_{\text{ball}} + \vec{\omega}_{\text{ball}} \times \vec{r}_c - \vec{v}_{\text{surface}}(\vec{p}_c)$
* Electromagnet force, with $\vec{d}$ from ball to magnet and $c$ = `magnetic_coupling`:
  $$\vec{F}_{\text{mag}} = c \cdot P \cdot K_{\text{coil}} \cdot \frac{\hat{d}}{\max(d^3,\, d_{\min}^3)}$$
  where $P \in [0,1]$ is commanded power. When $c = 0$ the force is exactly zero.
* Remanence (Subphase 2.7): each ball carries a magnetisation $M \in [0,1]$ that rises toward $P$ while the ball is within a magnet's range and decays as $M \leftarrow M e^{-\Delta t/\tau}$ otherwise ($\tau$ per magnet, default 60 ms). With the coil off, the ball still feels a residual pull toward the magnet's pole piece:
  $$\vec{F}_{\text{res}} = c \cdot \rho \cdot M \cdot K_{\text{coil}} \cdot \frac{\hat{d}}{\max(d^3,\, d_{\min}^3)}$$
  where $\rho$ = `remanence`. `low_rem_steel` ($\rho = 0$) releases cleanly; `standard_steel` lingers briefly.
* Magnus force (Subphase 2.7), with $S_m$ = `magnus_coeff`: $\vec{F}_{\text{magnus}} = S_m (\vec{\omega} \times \vec{v})$. It is perpendicular to $\vec{v}$ and therefore does no work.
* Eddy sensors close only when a ball with `conductive = true` is in range (D12). Optos and rollovers close for every ball. Scripts distinguish ball types by comparing the two.
* Gravity is applied in world space; playfield pitch (default $6.5^\circ$) comes from table geometry.

### 3.3 Flipper Contract (D16)

```rust
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub enum FlipperShape {
    StraightCapsule { length_mm: f32, base_radius_mm: f32, tip_radius_mm: f32 },
    CurvedBanana { length_mm: f32, arc_radius_mm: f32, arc_sweep_deg: f32, tube_radius_mm: f32 },
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub enum FlipperStagingMode {
    DedicatedButton(CabinetInput),
    ParallelLinked(CabinetInput),
    DualStageThreshold { axis: CabinetAxis, lower_threshold: f32, upper_threshold: f32 },
    AutoSensorThingFlip { trigger_switch: SwitchId, delay_ms: f32, hold_duration_ms: f32 },
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct FlipperConfig {
    pub id: FlipperId,
    pub coil: CoilId,
    pub eos_switch: SwitchId,
    pub collision_layer: u8,
    pub base_position_mm: [f32; 3],
    pub surface_normal: [f32; 3],
    pub rest_angle_deg: f32,
    pub sweep_angle_deg: f32,          // + = CCW about surface_normal, − = CW
    pub eos_angle_fraction: f32,       // Fraction of sweep where EOS opens (e.g. 0.92)
    pub shape: FlipperShape,
    pub inertia_kg_m2: f32,            // Bat + shaft + plunger, about the pivot
    pub power_torque_nm: f32,          // Power winding
    pub hold_torque_nm: f32,           // Hold winding
    pub return_spring_torque_nm: f32,
    pub stop_restitution: f32,         // Bat against end stop
    pub rubber_restitution: f32,
    pub rubber_damping: f32,           // Velocity-dependent loss in the rubber
    pub staging: FlipperStagingMode,
    pub zipper_slide_axis_mm: Option<[f32; 3]>,
    pub zipper_slide_speed_mm_s: f32,
}
```

The flipper is a one-degree-of-freedom rigid body. Ball contact is a two-body impulse: the ball pushes the bat back as well as the reverse. Live catches, drop catches and tap passes must fall out of this model.

### 3.4 Scripting Contract (D5, D10, D11, D12)

```rust
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SwitchEdge { Closed, Opened }

pub struct HostView {
    pub tick: u64,                    // Sim time in ms
    pub switches: [u64; 4],           // Bitset, 256 switches
    pub balls_in_play: u8,
    pub persist: [i64; 64],           // High scores, audits, settings
}

#[derive(Default)]
pub struct HostCommandQueue {
    pub coil_pulses:    ArrayVec<(CoilId, u16), 32>,       // duration_ms
    pub coil_holds:     ArrayVec<(CoilId, bool), 32>,
    pub magnets:        ArrayVec<(MagnetId, f32), 8>,      // power 0..=1
    pub zippers:        ArrayVec<(FlipperId, bool), 8>,
    pub diverters:      ArrayVec<(DiverterId, bool), 16>,
    pub ball_spawns:    ArrayVec<(DispenserId, ProfileId), 8>,
    pub lamps:          ArrayVec<(LampId, f32), 256>,
    pub gi_tint:        Option<[f32; 4]>,
    pub scores:         ArrayVec<(u8, i64), 4>,            // (player, absolute score)
    pub display_scenes: ArrayVec<SceneId, 8>,
    pub sounds:         ArrayVec<(SoundId, f32), 16>,      // gain
    pub persist_writes: ArrayVec<(PersistKey, i64), 8>,
}

pub struct HostCtx<'a> {
    pub view: &'a HostView,
    pub rng:  &'a mut Pcg32,          // Sim-owned, seeded, recorded in replays
    pub cmd:  &'a mut HostCommandQueue,
}

pub trait TableScriptBackend: Send {
    fn on_init(&mut self, ctx: &mut HostCtx);
    fn on_tick(&mut self, ctx: &mut HostCtx);
    fn on_switch_edge(&mut self, switch: SwitchId, edge: SwitchEdge, ctx: &mut HostCtx);
    /// Called on the sim thread at a tick boundary with source already read by the I/O worker.
    fn apply_reload(&mut self, source: &[u8]) -> Result<(), ScriptError>;
}
```

* Queue overflow drops the command and increments a counter surfaced in the debug HUD.
* Cabinet inputs are reserved switches declared in `[cabinet_switches]`: `left_flipper`, `right_flipper`, `left_upper`, `right_upper`, `action`, `start`, `launch`, `tilt`. There is no separate action-button callback.
* Ball drain is detected the way real machines do it: trough switches.
* Scripts own scoring and player state; the host only displays what `scores` reports.
* The backend is chosen by the extension of `script_entry` in `table.toml`: `.lua` or `.wasm`. Both implement the same trait and see the same host API.
* **Parity rule:** rules logic uses integer arithmetic only. `f32` values (lamp intensity, gain, magnet power) are passed to the host as literal constants or table lookups, never computed. This is what makes Lua and Wasm command streams comparable bit-for-bit.

**Sandbox (D25).** Lua loads only `base` (without `dofile`, `loadfile`, `load`), `string`, `table`, `math` (with `math.random` routed to the host PCG) and `utf8`. No `io`, `os`, `debug`, `package`. A memory cap and a per-callback instruction budget abort runaway scripts; an aborted script drops into a safe "tilt" state without stalling the tick.

### 3.5 Bundle Format

* `table.toml` and `backglass.toml` carry `schema_version`.
* Zip extraction rejects absolute paths, `..` components and symlinks, and enforces caps on total uncompressed size and compression ratio.

---

## 4. Phased Implementation Plan

Each subphase lists model, target, dependencies, deliverables and verification. Performance budget for the whole tick, script included: $99.9\text{th}$ percentile $< 0.35\text{ ms}$.

### Phase 0: Dependency Spikes

#### Subphase 0.1: Dependency Spikes & Version Pinning
* **Model:** Opus 5.5 · **Target:** `spikes/`, `docs/DECISIONS.md`, `versions.toml`, `.github/workflows/spikes.yml` · **Dependencies:** none
* **Deliverables:** One small standalone program per question, each run on all three CI operating systems, with the answer recorded in `docs/DECISIONS.md`:
  1. Does the `sdl3` crate's `Event` expose SDL's nanosecond timestamp? If not, a thin wrapper over the `sys` re-export that does.
  2. Can SDL virtual joysticks be created headless on each OS (needed for D31)?
  3. Which CI adapters report `Features::EXPERIMENTAL_RAY_QUERY` on the pinned `wgpu`?
  4. Which present modes does each CI adapter report?
  5. Does offscreen rendering work on lavapipe and WARP in CI?
  6. Smoke tests: `mlua` memory limit and instruction hook; `wasmtime` fuel metering; `parry3d` `cast_shapes_nonlinear` for capsule vs. ball.
  7. `versions.toml` pinning the exact versions that passed.
* **Verification:** Every spike is a passing test on every OS, or a recorded negative result with the fallback from §6 chosen. Subphase 1.1 consumes `versions.toml` unchanged.

---

### Phase 1: Foundation, CI & Platform Shell

#### Subphase 1.1: Workspace, Contracts & Triple Buffer
* **Model:** Opus 5.5 · **Target:** `pin_core`, root `Cargo.toml`, `physics_defaults.toml` · **Dependencies:** 0.1
* **Deliverables:**
  1. Workspace with all 10 crates, each with stubbed module directories per §2.1.
  2. Every contract in §3, plus `InputEvent`, `CabinetInput`, `CabinetAxis`, `HapticsTelemetryEvent`, `MaterialPair` and unit-conversion helpers.
  3. The `table.toml`, `backglass.toml` and `layout.toml` schemas **exactly as specified in Appendix A**, the telemetry packet types from Appendix B, and `physics_defaults.toml` populated from Appendix C. The appendix examples are committed as fixtures and must parse.
  3. `TripleBuffer<T: Copy>` with atomic index swapping.
  4. `FrameStateSnapshot`: `Copy`, fixed capacity (16 balls, 16 flippers, 64 dynamic meshes, 256 lamps), carrying tick number, ball position/orientation/velocity/profile, flipper angle and zipper offset, dynamic mesh transforms, lamp and GI state.
  5. `contracts/CHANGE_REQUESTS.md` with the protocol from §2.3.
* **Verification:** `cargo test -p pin_core`. Stress test with a writer and a reader thread over 100,000 publishes: zero torn reads, monotonic tick numbers. TOML round-trip tests for every schema.

#### Subphase 1.2: CI Matrix Skeleton
* **Model:** Opus 5.5 · **Target:** `.github/workflows/ci.yml` · **Dependencies:** 1.1
* **Deliverables:**
  1. Build + test on `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`.
  2. Jobs: `core-tests` (GPU-free), `gpu-tests` (lavapipe on Linux, WARP on Windows, Metal on macOS), `clippy -D warnings`, `fmt`.
  3. A placeholder `replay-hash` job that Subphase 2.5 fills in.
* **Verification:** All jobs green on the empty scaffold.

#### Subphase 1.3: SDL3 Windows & Display Topology
* **Model:** Opus 5.5 · **Target:** `pin_app/platform/`, `pin_render/topology/` · **Dependencies:** 1.1
* **Deliverables:**
  1. SDL3 init for video, audio, gamepad and sensor subsystems.
  2. `DisplayTopologyManager`: single-monitor mode (one window with PiP rectangles for backglass and score display) and cabinet mode (up to three borderless fullscreen windows).
  3. Main-thread event pump per §2.2, forwarding timestamped `InputEvent`s and window events through SPSC rings.
* **Verification:** Mock-display tests for viewport math in 1-, 2- and 3-screen topologies. Ring test proving events arrive in order with timestamps intact.

#### Subphase 1.4: `wgpu` Multi-Surface Context, Thread Orchestrator & Debug Renderer
* **Model:** Opus 5.5 · **Target:** `pin_render/gpu/`, `pin_render/debug/`, `pin_app/orchestrator/` · **Dependencies:** 1.3
* **Deliverables:**
  1. One `wgpu` instance/adapter/device/queue on the render thread, presenting to 1–3 surfaces with the present modes from §2.2.
  2. Thread orchestrator: spawns sim, render, audio and I/O threads; sim thread drives a `SimStep` trait object (stub until 6.0) with accumulator timing and catch-up cap.
  3. Debug line renderer drawing snapshot contents and collider wireframes.
* **Verification:** `pin_app --headless-ticks 10000` completes on simulated time with monotonic snapshots. `gpu-tests`: an offscreen frame of debug lines matches a golden image within tolerance. Non-gating benchmark: real-time drift below 0.1 % over 10 s.

---

### Phase 2: Physics & Headless Simulation

#### Subphase 2.1: BVH, Collision Layers & C² Spline Colliders
* **Model:** Fable 5.2 (complete, v4) · **Target:** `pin_physics/world/` · **Dependencies:** 1.1
* **Deliverables:**
  1. `parry3d` static BVH for walls, posts, targets and toy meshes with an 8-bit `collision_layer` mask (bit 0 main deck, 1 upper deck, 2 wireform, 3 subway, 4 backbox). Per-collider enable flags for drop targets, gates and diverters.
  2. C² interpolating cubic splines (D13): authored as through-points, stored as cubic Bézier segments. Closest-point by Newton iteration, seeded from the ball's previous parameter with a coarse sample table as fallback.
  3. Wireforms as **two rails**: centerline spline, authored roll angle and rail gauge, framed with a rotation-minimizing frame. Single-curve guides for orbit walls.
  4. Layer-transition trigger zones for ramp entrances, VUKs and drop holes.
  5. Pre-stub `ball/`, `flipper/`, `mech/`, `debug/` modules.
* **Verification:** A ball at $8.0\text{ m/s}$ through a $180^\circ$ guide of radius 150 mm, then up a ramp from $Z = 0$ to $65\text{ mm}$: kinetic + potential energy never increases; normal force changes by less than 5 % between consecutive ticks away from entry and exit; no tunnelling; layer mask switches exactly once per transition.

#### Subphase 2.2: CCD, Spin, Friction, Magnets & Newton Balls
* **Model:** Fable 5.2 (complete, v4) · **Target:** `pin_physics/ball/` · **Dependencies:** 2.1
* **Deliverables:**
  1. Swept-sphere CCD by conservative advancement, using `parry3d` shape-casts for static geometry and analytic tests for splines and ball-ball pairs.
  2. Angular velocity, Coulomb friction with sliding-to-rolling transition, material-pair restitution and friction tables.
  3. Electromagnets and eddy sensors per §3.2.
  4. Captive Newton balls constrained to a 1D spline; dispenser stacks as FIFO queues.
  5. An `ExternalForce` hook list in the ball integrator (empty by default) and a pre-stubbed `ball_ext/` module for Subphase 2.7.
* **Verification:**
  1. Identical coil impulse: Powerball-to-steel launch speed ratio is $80/65 = 1.231 \pm 0.01$.
  2. Powerball and Glo-Ball trajectories through an active magnet are bit-identical to trajectories with the magnet off.
  3. Newton ball: impulse reaches the far ball within 2 ticks.
  4. Mini vs. jumbo collision: linear momentum conserved to $10^{-4}$ relative; total angular momentum about the system centre of mass conserved to $10^{-3}$; kinetic energy never increases.
  5. A spinning ball dropped on a flat surface converges to rolling without slipping.

#### Subphase 2.3: Flipper Solver (Standard, Upper, Zipper, Thing Flip)
* **Model:** Fable 5.2 (complete, v4) · **Target:** `pin_physics/flipper/` · **Dependencies:** 2.2
* **Deliverables:**
  1. Flipper kinematics on arbitrary mounting planes from `surface_normal`.
  2. Dual-wound coil: power torque until the EOS switch opens at `eos_angle_fraction`, then hold torque; return spring; end-stop restitution.
  3. Rotating-capsule vs. sphere CCD. Use `parry3d`'s nonlinear shape-cast if the pinned version provides it; otherwise conservative advancement in the flipper's local frame.
  4. Two-body ball-bat impulse with rubber restitution and damping.
  5. Zipper slides along `zipper_slide_axis_mm`; Thing Flips scheduled from a trigger switch.
  6. A `BatShape` trait (closest point, surface velocity, swept test) implemented for `StraightCapsule`, with a pre-stubbed `flipper_banana/` module for Subphase 2.6. Loading a `CurvedBanana` flipper before 2.6 lands is a validation error.
* **Verification:**
  1. **Timing sweep:** ball arriving at $3.5\text{ m/s}$, flip timed from −20 ms to +20 ms in 1 ms steps. Rebound speed is continuous across the sweep, and its minimum is at most 25 % of the rebound from a held flipper.
  2. **Drop catch:** releasing the button just before impact yields lower rebound than a held flipper.
  3. **Shot spread:** contact points at 25 %, 50 % and 85 % of bat length give monotonically changing release angles.
  4. **Zipper:** two flippers with a 48 mm tip gap each slide 12 mm inward; a 27 mm ball can no longer pass.
  5. Solver source contains no branch keyed to a named technique (reviewer check).

#### Subphase 2.4: Bumpers, Slingshots, Targets, Plunger & Tilt Bob
* **Model:** Opus 5.5 · **Target:** `pin_physics/mech/` · **Dependencies:** 2.2
* **Deliverables:**
  1. Pop bumpers, slingshots, drop-target banks, kickbacks, spinners (one switch pulse per $180^\circ$), VUK scoops.
  2. Spring plunger ($F = -kx$) with analog position input.
  3. Cabinet nudge impulses and a damped spherical-pendulum tilt bob inside a contact ring.
* **Verification:** Bumper ring response, spinner pulse counts against known spin-down, VUK transfer from layer 0 to layer 1, tilt closure above a nudge threshold and none below it.

#### Subphase 2.5: Headless Simulation, State Injection & Replay Harness
* **Model:** Opus 5.5 · **Target:** `pin_sim`, `pin_physics/debug/` · **Dependencies:** 2.3, 2.4, 3.1, 4.1, 4.2
* **Deliverables:**
  1. `Simulation` implementing the tick order in §2.2 and the `SimStep` trait.
  2. Test API: spawn a ball at a position and velocity, set switch states, advance N ticks, query ball and score state.
  3. Recorder and replayer: per-tick input events, RNG seed and table hash in a binary file (`--record-inputs`, `--replay-inputs`).
  4. Fill the CI `replay-hash` job; add the allocation guard around `step_1ms()`.
* **Verification:** A 10,000-tick recording replays to an identical state hash 10 times in a row. The allocation guard passes over 60,000 ticks with a script loaded.

#### Subphase 2.6: Curved "Banana" Flipper *(restored)*
* **Model:** Fable 5.2 (complete, v4) · **Target:** `pin_physics/flipper_banana/` · **Dependencies:** 2.3
* **Deliverables:**
  1. `CurvedBanana` as a circular-arc tube implementing `BatShape`: analytic closest point (point-to-arc in the bat plane, then tube radius) and surface velocity $\vec{v}_{\text{bat}} = \vec{\omega}_{\text{bat}} \times \vec{r}_{\text{contact}}$.
  2. Rotating-arc vs. sphere CCD by conservative advancement in the flipper's local frame. `parry3d`'s nonlinear cast is not relied on here because it can report a shape pair as unsupported (§6).
  3. Correct handling of both the concave and convex faces and of the tip cap, using the same two-body impulse as 2.3.
* **Verification:**
  1. Contacts at 25 %, 50 % and 85 % along the arc give monotonically changing release angles, each shifted in a consistent direction against a straight capsule of equal length.
  2. As `arc_radius_mm` grows large, results converge to the `StraightCapsule` solver within tolerance.
  3. A ball resting in the concave pocket of a raised bat stays cradled for 2,000 ticks.
  4. The 2.3 timing sweep passes on the curved bat with no discontinuity.

#### Subphase 2.7: Magnus Effect & Magnetic Remanence *(restored)*
* **Model:** Opus 5.5 · **Target:** `pin_physics/ball_ext/` · **Dependencies:** 2.2
* **Deliverables:**
  1. Magnus force and remanence per §3.2, registered through the `ExternalForce` hooks.
  2. Per-ball magnetisation state added to the ball arena (via a contract-change request).
  3. `low_rem_steel` profile enabled in validation.
* **Verification:**
  1. With `magnus_coeff = 0` and `remanence = 0`, every 2.2 baseline trajectory is bit-identical.
  2. A spinning Glo-Ball in free flight shows lateral acceleration $S_m |\vec{\omega} \times \vec{v}| / m$ within 1 %, and its kinetic energy is unchanged by the Magnus term.
  3. After a holding magnet switches off, `standard_steel` leaves measurably later than `low_rem_steel`, and `low_rem_steel` matches the zero-remanence baseline exactly.

#### Subphase 2.8: Large-Ball Support & Clearance Validation *(restored)*
* **Model:** Opus 5.5 · **Target:** `pin_tablegen/clearance/`, `pin_physics/tests/` · **Dependencies:** 2.3, 4.3
* **Deliverables:**
  1. `jumbo_xl_resin` profile (79.4 mm) and support for any diameter between 15.88 mm and 79.4 mm.
  2. Clearance validator: for every ball profile a table declares, check lane widths, flipper gaps, wireform gauge, scoop and VUK diameters, and headroom under upper decks. Violations are bundle-validation errors naming the offending feature. A profile can be restricted to listed layers.
  3. CCD and broad-phase margins that scale with radius.
* **Verification:**
  1. A 79.4 mm ball against a 15.88 mm ball conserves momentum to the 2.2 tolerances.
  2. The validator rejects an unrestricted XL ball on a table with a 65 mm deck gap and accepts it once restricted to the main deck's open area.
  3. No tunnelling for the XL ball against a flipper at maximum angular velocity.

#### Subphase 2.9: Autoplay Bot & Table Soak Harness
* **Model:** Opus 5.5 · **Target:** `pin_sim/autoplay/` · **Dependencies:** 2.5
* **Purpose:** Stand in for human playtesting during the run. It cannot judge feel; it can prove a table is mechanically sound and fully reachable.
* **Deliverables:**
  1. A seeded heuristic bot that reads sim state and presses cabinet switches: plunges, flips when a ball is predicted to cross a flipper's sweep, holds to cradle, nudges occasionally. Three skill levels set by reaction jitter and miss rate.
  2. A soak runner: N seeded games per table, headless, faster than real time, in parallel.
  3. Health checks: stuck ball (below 1 cm/s for more than 10 s outside a trough, lock or held cradle); escaped ball (outside table bounds); non-finite state; switch coverage; shot coverage against the `[[shots]]` declared in `table.toml`; rule-mode coverage; ball-time and score distributions; script errors; command-queue overflows; tick-time budget.
  4. JSON and Markdown reports attached to the run log.
* **Verification:** Fixture tables with planted defects (a ball trap, a gap in a wall, an unreachable switch, a script that errors on one mode) are each caught and named. A clean fixture passes. The bot is deterministic for a given seed.

---

### Phase 3: Input, Audio & Cabinet Output

#### Subphase 3.1: Input Ingestion & Flipper Staging
* **Model:** Opus 5.5 · **Target:** `pin_input/events/`, `pin_input/staging/` · **Dependencies:** 1.1
* **Deliverables:**
  1. Consume timestamped `InputEvent`s; map to cabinet switches and axes; debounce.
  2. `FlipperStagingRouter`: dedicated buttons, parallel linked, dual-stage threshold (30 % lower, 85 % upper by default).
* **Verification:** Synthetic analog ramps and button sequences produce actuation on the exact expected tick in all three modes.

#### Subphase 3.2: X-Arcade Profiles, Plunger & Nudge Calibration
* **Model:** Opus 5.5 · **Target:** `pin_input/profiles/`, `pin_input/calibration/` · **Dependencies:** 3.1
* **Deliverables:**
  1. Auto-detected X-Arcade presets: keyboard-encoder scancodes and gamepad modes matched by SDL GUID.
  2. Analog plunger filters: deadzone, rest tracking, release-velocity peak detection.
  3. Accelerometer nudge: high-pass filter, gain curve, digital and thumbstick fallback.
* **Verification:** Profile parser and mapping tests for each mode translate to canonical cabinet switches.

#### Subphase 3.3: Audio Mixer
* **Model:** Opus 5.5 · **Target:** `pin_audio/mixer/` · **Dependencies:** 1.1
* **Deliverables:**
  1. SDL3 audio-stream mixer on the audio thread with music, callout and effects buses, voice limiting and callout ducking.
  2. WAV and Ogg Vorbis decoding; samples supplied as byte buffers by the I/O worker.
  3. Lock-free ring consuming `sounds` from the command queue.
* **Verification:** Offline render tests: trigger-to-first-sample latency within one audio buffer; ducking curve and voice-limit behaviour match expected output.

#### Subphase 3.4: Haptics Bus, SSF Mixer & UDP Telemetry
* **Model:** Opus 5.5 · **Target:** `pin_audio/ssf/`, `pin_audio/telemetry/`, `pin_input/haptics/` · **Dependencies:** 3.2, 3.3
* **Deliverables:**
  1. Lock-free `HapticsTelemetryEvent` ring fed by the sim thread (coil fire, EOS hit, bumper, ball drop, ball roll with position, mass and speed).
  2. Gamepad rumble output for flippers, slings and bumpers.
  3. Four-corner SSF panner over the 7.1 exciter channels.
  4. Versioned binary UDP telemetry (switch, coil, lamp states) with a written protocol spec (D22).
* **Verification:** Panner gains for a ball moving from $(-150, 100)$ to $(150, 800)$ mm; packet emitted on the same tick as a simulated flipper pulse.

#### Subphase 3.5: DirectOutput Framework (DOF) Bridge *(restored)*
* **Model:** Opus 5.5 · **Target:** `tools/dof_bridge/` (C#/.NET, Windows only), `[dof]` schema via contract-change request · **Dependencies:** 3.4
* **Deliverables:**
  1. `[dof]` section in `table.toml`: the ROM-style table name DOF configs key on, and a map from engine coils, lamps and switches to DOF table-element type and number.
  2. Bridge process: listens to the UDP telemetry stream, hosts the user's installed DOF, and forwards each change as a (type, number, value) table-element update, which is the form DOF accepts from its host.
  3. Engine starts and stops the bridge with table load on Windows when `[dof]` is present; a no-op on Linux and macOS.
* **Verification:** Windows CI replays recorded telemetry into the bridge with a mock DOF sink and asserts the exact update sequence, forwarded within 2 ms of packet arrival. Builds on other platforms are unaffected. Real hardware is checked at final Human Acceptance; until then the bridge is labelled "unverified on hardware" (D33).

---

### Phase 4: Assets, Scripting, Table Generator & Menu

#### Subphase 4.1: Bundle VFS, glTF Loader & Manifest Validation
* **Model:** Opus 5.5 · **Target:** `pin_assets` · **Dependencies:** 1.1, 2.1
* **Deliverables:**
  1. VFS over a directory or a `.pinbundle` / `.zip`, with the hardening in §3.5.
  2. `table.toml` / `backglass.toml` parsing, name-to-ID interning, unit conversion, validation with precise error messages.
  3. `gltf` loading of meshes and PBR textures, plus Blender `extras` for collision layers, switch IDs and lamp inserts.
* **Verification:** A mock bundle loads identically from folder and zip. Malicious archives (path traversal, symlink, oversize) are rejected.

#### Subphase 4.2: Lua 5.4 Backend, Sandbox & Hot Reload
* **Model:** Opus 5.5 · **Target:** `pin_script/lua/` (plus `load_backend()` and a pre-stubbed `wasm/` module) · **Dependencies:** 1.1
* **Deliverables:**
  1. `LuaScriptBackend` on `mlua` (vendored Lua 5.4) exposing a `pinball` module: `pulse_coil`, `hold_coil`, `set_magnet`, `set_lamp`, `set_gi_tint`, `set_score`, `spawn_ball`, `set_diverter`, `set_zipper`, `show_scene`, `play_sound`, `switch`, `tick`, `balls_in_play`, `random`, `persist_get`, `persist_set`.
  2. Sandbox, memory cap and instruction budget per §3.4. Incremental GC stepped with a fixed budget each tick.
  3. Hot reload: the I/O worker (`notify`) reads and syntax-checks `rules.lua`, then hands the source to the sim thread, which calls `apply_reload` at a tick boundary. Optional script hooks `on_save_state` / `on_restore_state` carry state across.
* **Verification:** Scripted switch sequences produce expected command queues. Sandbox escape attempts fail. An infinite loop is aborted within budget. Reload preserves state through the hooks.

#### Subphase 4.3: Procedural Table Generator
* **Model:** Opus 5.5 · **Target:** `pin_tablegen` · **Dependencies:** 4.1
* **Deliverables:**
  1. CLI turning a declarative `layout.toml` (playfield outline, walls, posts, lanes, ramps as spline through-points, decks, inserts) into `playfield.glb` with correct `extras` tags and flat-colour materials.
  2. Matching collider and spline declarations emitted into `table.toml`.
* **Verification:** Generated bundles pass 4.1 validation; visual mesh and collision geometry agree within 0.1 mm.

#### Subphase 4.4: Table Browser, Display Setup & Input Menu
* **Model:** Opus 5.5 · **Target:** `pin_app/ui/`, `pin_render/ui/` · **Dependencies:** 1.4, 3.2, 4.1
* **Deliverables:**
  1. `egui-wgpu` overlay toggled by `Esc` or the controller Menu button.
  2. Table browser (preview art, metadata, feature badges, one-click load).
  3. Display panel (single vs. multi-monitor, PiP placement, camera presets, DMD vs. LCD).
  4. Input panel (profile, staging mode, live plunger and nudge calibration, debug wireframe toggle).
* **Verification:** Headless UI state-machine test for directory scanning, table switch lifecycle and `config.toml` persistence.

#### Subphase 4.5: WebAssembly Scripting Backend *(restored)*
* **Model:** Opus 5.5 · **Target:** `pin_script/wasm/`, `pin_script/guest-sdk/` · **Dependencies:** 4.2
* **Deliverables:**
  1. `WasmScriptBackend` on `wasmtime` implementing `TableScriptBackend`.
  2. Host interface as plain core-Wasm imports under a `pinball` module, mirroring the Lua API one-to-one with integer and `f32` arguments only. Guest exports: `on_init`, `on_tick`, `on_switch_edge`, and optional `save_state` / `restore_state`.
  3. Sandbox: no WASI, memory cap, fuel metering as the per-callback budget, NaN canonicalisation on. Fuel exhaustion drops the script into the same safe state as an aborted Lua script.
  4. Modules are compiled on the I/O worker and instantiated at a tick boundary, so swap follows the same path as Lua reload.
  5. A `no_std` Rust guest SDK so authors (and Subphase 6.3) can write rules in Rust; C, AssemblyScript and TinyGo work against the same imports.
  6. `.wasm` branch in `load_backend()`.
* **Verification:** An integer-only fixture ruleset in `rules.lua` and `rules.wasm` receives the same switch sequence and yields identical `HostCommandQueue` contents on every tick. Fuel exhaustion and memory growth past the cap are contained. The allocation guard passes on the host side of guest calls.

#### Subphase 4.6: Placeholder Asset Kit
* **Model:** Opus 5.5 · **Target:** `crates/pin_assetgen` · **Dependencies:** 4.1
* **Deliverables:** Deterministic, code-generated, licence-clean assets, so no table depends on third-party files (D32):
  1. Synthesised sound effects: flipper, bumper, slingshot, drop target, spinner tick, ball-roll loop, knocker, drain, launch.
  2. A short generated music loop per table, and distinct tone motifs in place of spoken callouts.
  3. Bitmap DMD fonts defined in code (5×7 and 8×16, ASCII 32–126).
  4. Procedural translite, playfield art and insert labels derived from table metadata (title, palette).
  5. An asset manifest with stable IDs, so real assets replace placeholders without touching rules or TOML.
* **Verification:** Outputs load through 4.1 and have stable hashes. Each sound is non-silent and below clipping. Fonts cover the full range. Every `SoundId` and `SceneId` used by the tables resolves.

---

### Phase 5: Rendering

#### Subphase 5.1: PBR Pipeline & Pinball Materials
* **Model:** Opus 5.5 · **Target:** `pin_render/pbr/` · **Dependencies:** 1.4, 4.1
* **Deliverables:**
  1. Clustered forward PBR in WGSL with up to 128 dynamic lights.
  2. Materials: `pbr_chrome_steel`, `pbr_ceramic_pearl`, `pbr_uv_emissive` (responds to GI tint), clear-coated playfield wood, subsurface light inserts with warm-up curves (15 ms incandescent, 1 ms LED), `pbr_clear_acrylic`.
* **Verification:** `naga` validates every module and translates it to SPIR-V, MSL and HLSL without warnings. `gpu-tests` golden image per material.

#### Subphase 5.2: Ball Reflection Probes & HDR Post-Processing
* **Model:** Opus 5.5 · **Target:** `pin_render/reflections/`, `pin_render/post/` · **Dependencies:** 5.1
* **Deliverables:**
  1. Octahedral reflection probes behind a `ReflectionSource` trait. Budget: at most 2 probe updates per frame at $128^2$, round-robin across balls; balls without a fresh probe reuse the nearest.
  2. HDR chain: physically based bloom, ACES tone mapping, velocity-based motion blur on balls, MSAA or SMAA.
* **Verification:** `gpu-tests`: a chrome ball beside a lit red insert shows red in the expected reflection region; bloom energy matches the golden image; with 6 balls the per-frame probe count never exceeds the budget.

#### Subphase 5.3: Backglass & Dual-Mode Score Display
* **Model:** Opus 5.5 · **Target:** `pin_render/displays/` · **Dependencies:** 5.1
* **Deliverables:**
  1. Backglass: layered translite art with lamp-matrix masks and flasher overlays.
  2. DMD mode: $128 \times 32$ or $192 \times 64$ procedural dot grid with orange, red or RGB dots.
  3. LCD mode: 16:9 layered sprite, vector font and animation compositor.
  4. Routing to PiP regions or dedicated windows.
* **Verification:** `gpu-tests` goldens for DMD text, score formatting and lamp-mask blending.

#### Subphase 5.4: Hardware Ray-Query Reflections *(restored)*
* **Model:** Opus 5.5 (Fable 5.2 only under D34) · **Target:** `pin_render/raytrace/` · **Dependencies:** 5.2
* **Deliverables:**
  1. First task: confirm on the pinned `wgpu` which backends report `Features::EXPERIMENTAL_RAY_QUERY` (§6) and record the result in the crate docs.
  2. `RayQueryReflections` implementing `ReflectionSource`, created only when the adapter reports the feature. Bottom-level structures per static mesh; a top-level structure refit each frame for balls, flippers and dynamic meshes.
  3. WGSL ray queries for ball reflections at full resolution and clear-coat reflections at half resolution, with a roughness cutoff above which the probe result is used.
  4. Setting: Off / Balls only / Balls and clear-coat. Automatic fallback to probes when the feature is absent or the device is lost.
* **Verification:** With the feature absent, output is identical to the probe path. On a ray-tracing-capable runner, a two-ball scene shows each ball reflected in the other (which probes cannot do) and matches a golden image. Non-gating benchmark: added GPU time recorded per quality setting.

#### Subphase 5.5: Backbox Mini-Playfield, Vertical Flippers & 3D Toys *(restored)*
* **Model:** Opus 5.5 · **Target:** `pin_render/backbox/`, `pin_tablegen/backbox/`, `pin_physics/tests/` · **Dependencies:** 2.3, 4.3, 5.3
* **Deliverables:**
  1. `[backbox]` section in `backglass.toml`: a 3D scene composited with depth between translite layers on the backglass surface, with its own camera.
  2. Mechanical toys driven by dynamic mesh transforms: score reels and animated mechanisms.
  3. Vertical mini-playfield on collision layer bit 4: $90^\circ$ plane colliders, flippers using the existing staging modes, and a layer transition that hands a ball from the main playfield to the backbox and back.
  4. `pin_tablegen` support for a backbox layout block.
* **Verification:** A flipper on a $90^\circ$ plane produces the shot speed predicted from the horizontal case once gravity along the plane is accounted for. A backbox ball never collides with layers 0–3. Golden image of reels and mini-playfield composited with the translite.

---

### Phase 6: Integration & Tables

#### Subphase 6.0: Integration & Grey-Box Table (Milestone M1)
* **Model:** Opus 5.5 · **Target:** `pin_app/orchestrator/`, `tables/greybox/` · **Dependencies:** 2.5, 3.4, 4.3, 4.4, 4.6, 5.1
* **Deliverables:**
  1. Wire `pin_sim`, input, audio, haptics and rendering into `pin_app`.
  2. `tables/greybox`: two lower flippers, one upper flipper, slings, three bumpers, an orbit, a wireform ramp, a VUK to an upper deck, a magnet with eddy sensor, a Newton ball, a Powerball dispenser. Minimal rules: start, score, drain, ball count.
* **Verification:** Headless scripted run using state injection exercises every mechanism and asserts switch counts. Recorded replay is deterministic.

**Gate A1 (automated, after Wave 6): physics reference suite on the grey-box.** Tables do not start until A1 is green.
1. *Analytic checks.* A ball rolling without slipping down the $6.5^\circ$ playfield accelerates at $\tfrac{5}{7} g \sin\theta$ within 1 % (1.07 m in about 1.64 s). Drop-bounce height ratio equals $e^2$ within 2 %. The tilt bob's small-swing period matches the pendulum formula within 2 %. Energy never increases in any passive scene.
2. *Provisional feel windows* (Appendix C; reviewed at final acceptance): unloaded flipper stroke time and the exit speed of a full shot from a cradled ball fall inside their windows.
3. *Soak.* 1,000 autoplay games: zero stuck, escaped or non-finite balls; 100 % switch coverage; median ball time between 15 s and 120 s; $99.9\text{th}$-percentile tick under 0.35 ms.
4. If a feel window is missed, the Orchestrator may run an automated parameter search over `physics_defaults.toml` within the Appendix C bounds before opening fix tasks.

#### Subphase 6.1: Variant A — "Apex Circuit"
* **Model:** Opus 5.5 · **Target:** `tables/apex_circuit/` · **Dependencies:** Gate A1
* **Deliverables:**
  1. Geometry via `layout.toml`: $6.5^\circ$ playfield, dual 76 mm lower flippers with Zipper mode ("Firewall Save"), 55 mm upper-left flipper at 10 o'clock fed by a wireform return or left-orbit pass, dispenser holding 4 steel balls and 1 Powerball, right-orbit magnet with eddy sensor and opto, 3-bumper cluster, left-orbit spinner, 3-bank drops, centre lock scoop, upper-right cross-shot jackpot ramp.
  2. `rules.lua`: attract → 3-ball game → skill shot → upper-flipper combo multipliers (2×–5×) → Powerball Mania (2× scoring; Powerball detected by opto closing while the eddy sensor stays open) → bonus and high-score entry.
  3. Action switch: when "Overdrive Ready" is lit, either close the Zipper flippers for 5.0 s or cash a decaying "Apex Jackpot."
* **Verification:** Headless scripted game with state injection: skill shot, cross-shot ramp, Powerball dispense, magnet bypass, action switch; assert scores and display scenes.

#### Subphase 6.2: Variant B — "Sky Deck"
* **Model:** Opus 5.5 · **Target:** `tables/sky_deck/` · **Dependencies:** Gate A1
* **Deliverables:**
  1. Geometry: main deck at $Z = 0$ with lower flippers, slings, lanes, centre VUK and a left ramp with diverter; clear-acrylic upper deck at $Z = 65\text{ mm}$; 52 mm upper-right flipper on the upper deck (`collision_layer = 1`); captive Newton ball guarding "Reactor Core"; elevated 3-bank drops; upper-left wrap-around loop; acrylic return funnel.
  2. `rules.lua`: action switch toggles the "Sky Bridge Diverter" for 8.0 s in normal play; after the Newton ball and drop bank are complete, it starts "Night Overdrive": UV GI tint plus a Glo-Ball on the upper deck alongside a steel ball below.
* **Verification:** Headless scripted run: VUK transition to layer 1, upper-flipper shots, Newton ball strike, drop bank, Glo-Ball launch, and proof that balls on different layers never collide.

#### Subphase 6.4: "Feature Lab" Showcase Table *(restored features, playable)*
* **Model:** Opus 5.5 · **Target:** `tables/feature_lab/` · **Dependencies:** Gate A1, 2.6, 2.7, 2.8, 5.5
* **Deliverables:**
  1. Geometry: a Banana lower-right flipper opposite a straight lower-left; a backbox mini-playfield with one vertical flipper, fed by a VUK; an open lower area cleared for the 79.4 mm ball; a holding magnet with a dispenser that can serve `standard_steel` or `low_rem_steel`.
  2. `rules.lua`: a mode selector on the action switch cycling through Banana shots, backbox play, XL-ball mode, magnet release comparison, and Glo-Ball mode with Magnus enabled.
* **Verification:** Headless scripted run exercising each mode with state injection; clearance validator passes with the XL ball restricted to its area.

**Gate A2 (automated, after Wave 7): table soak.** For each of the three tables, 1,000 autoplay games at each skill level:
1. Zero stuck, escaped or non-finite balls; zero script errors; zero command-queue overflows.
2. 100 % switch coverage; every declared shot made at least once; every rule mode started at least once.
3. Median game length at the middle skill level between 1 and 6 minutes; scores increase with skill level.
4. Deterministic replay of 20 sampled games per table.

Rules are frozen after A2. Later tuning is data-only (`physics_defaults.toml` and table TOML), and the Lua/Wasm parity test stays in CI so any later rule change is caught.

#### Subphase 6.3: Wasm Ports of Table Rules *(restored)*
* **Model:** Opus 5.5 · **Target:** `tables/*/rules-wasm/` · **Dependencies:** 4.5, 6.1, 6.2, 6.4, Gate A2
* **Deliverables:**
  1. Rust ports of the three tables' rules on the guest SDK, each built to `rules.wasm` and shipped beside `rules.lua`.
  2. CI step that rebuilds each `rules.wasm` and compares it with the committed binary.
* **Verification:** For each table, recorded replays produce identical per-tick command queues and identical final scores under the Lua and Wasm backends.

---

### Phase 7: Profiling & Release

#### Subphase 7.1: Latency Profiler & Determinism Soak
* **Model:** Opus 5.5 · **Target:** `pin_app/profiler/` · **Dependencies:** 6.1, 6.2
* **Deliverables:**
  1. HUD: $99.9\text{th}$-percentile tick time (target $< 0.35\text{ ms}$), input latency from SDL event timestamp to the tick that applies coil torque (target $\le 1.5\text{ ms}$ at the 99th percentile), GPU frame time, command-queue overflow count.
  2. Soak test: 10-minute recorded multiball replays on both tables.
* **Verification:** Soak replays are deterministic on each platform; cross-OS hashes are reported.

#### Subphase 7.2: Packaging & Release
* **Model:** Opus 5.5 · **Target:** `scripts/package.*`, `.github/workflows/release.yml` · **Dependencies:** 6.3, 7.1
* **Deliverables:**
  1. Release builds for Windows, Linux and macOS (universal `.app`, adding `x86_64-apple-darwin`).
  2. `greybox`, `apex_circuit`, `sky_deck` and `feature_lab` as `.pinbundle` archives beside the executable, each with `rules.lua` and, where ported, `rules.wasm`.
  3. The DOF bridge included in the Windows package only.
  4. `docs/HUMAN_ACCEPTANCE.md`, generated from the run log: the Phase 8 checklist with this run's blocked items, logged decisions, D33 flags and gate reports filled in.
* **Verification:** Clean `cargo build --release --workspace` and `cargo test --release --workspace` with zero warnings on every target.

---

### Phase 8: Final Human Acceptance (the only human step after setup)

Work through `docs/HUMAN_ACCEPTANCE.md`:
1. **Review the run.** Read `docs/HUMAN_QUEUE.md` and `docs/DECISIONS.md`; reverse any decision you disagree with as a follow-up task.
2. **Play each table.** Judge flipper feel, ball speed, catches and nudge. Tune only data (`physics_defaults.toml`, table TOML), then rerun CI; gates A1 and A2 revalidate automatically.
3. **Check hardware.** X-Arcade in each mode, analog plunger, accelerometer nudge, rumble, SSF channels, multi-monitor layout, DOF devices.
4. **Check visuals.** Reflections, bloom, DMD and backglass on a real display; ray-query reflections on a capable GPU, then decide whether to enable them by default.
5. **Replace placeholders.** Swap real sounds, music, callouts and art in by asset ID.

---

## 5. Wave & Dependency Matrix

A subphase never shares a wave with one of its dependencies. Where a crate appears twice in a wave, the module column shows the split. The **Model** column records who ran Waves 0–5 and the expected Implementer tier from Wave 6 on; the work-classifier makes the final choice (§2.5, D34).

| Wave | Subphase | Model | Depends on | Crate / module |
|---|---|---|---|---|
| **0** | 0.1 Dependency spikes & pinning | Opus 5.5 | — | `spikes`, `versions.toml` |
| **1** | 1.1 Contracts & triple buffer | Opus 5.5 | 0.1 | `pin_core` |
| **2** | 1.2 CI skeleton | Opus 5.5 | 1.1 | `.github/workflows` |
| **2** | 1.3 SDL3 windows & topology | Opus 5.5 | 1.1 | `pin_app/platform`, `pin_render/topology` |
| **2** | 2.1 BVH, layers, C² splines | **Fable 5.2** | 1.1 | `pin_physics/world` |
| **2** | 3.1 Input ingestion & staging | Opus 5.5 | 1.1 | `pin_input/events`, `staging` |
| **2** | 4.2 Lua backend & hot reload | Opus 5.5 | 1.1 | `pin_script` |
| **3** | 1.4 wgpu, orchestrator, debug render | Opus 5.5 | 1.3 | `pin_render/gpu`, `debug`; `pin_app/orchestrator` |
| **3** | 2.2 CCD, spin, magnets, Newton | **Fable 5.2** | 2.1 | `pin_physics/ball` |
| **3** | 3.2 X-Arcade, plunger, nudge | Opus 5.5 | 3.1 | `pin_input/profiles`, `calibration` |
| **3** | 3.3 Audio mixer | Opus 5.5 | 1.1 | `pin_audio/mixer` |
| **3** | 4.1 VFS, glTF, validation | Opus 5.5 | 1.1, 2.1 | `pin_assets` |
| **3** | 4.5 Wasm backend *(restored)* | Opus 5.5 | 4.2 | `pin_script/wasm`, `guest-sdk` |
| **4** | 2.3 Flipper solver | **Fable 5.2** | 2.2 | `pin_physics/flipper` |
| **4** | 2.4 Bumpers, plunger, tilt | Opus 5.5 | 2.2 | `pin_physics/mech` |
| **4** | 3.4 Haptics, SSF, telemetry | Opus 5.5 | 3.2, 3.3 | `pin_audio/ssf`, `telemetry`; `pin_input/haptics` |
| **4** | 4.3 Table generator | Opus 5.5 | 4.1 | `pin_tablegen` |
| **4** | 5.1 PBR & materials | Opus 5.5 | 1.4, 4.1 | `pin_render/pbr` |
| **4** | 2.7 Magnus & remanence *(restored)* | Opus 5.5 | 2.2 | `pin_physics/ball_ext` |
| **4** | 4.6 Placeholder asset kit | Opus 5.5 | 4.1 | `pin_assetgen` |
| **5** | 2.5 Headless sim & replay | Opus 5.5 | 2.3, 2.4, 3.1, 4.1, 4.2 | `pin_sim`, `pin_physics/debug` |
| **5** | 4.4 Menu UI | Opus 5.5 | 1.4, 3.2, 4.1 | `pin_app/ui`, `pin_render/ui` |
| **5** | 5.2 Probes & HDR post | Opus 5.5 | 5.1 | `pin_render/reflections`, `post` |
| **5** | 5.3 Backglass & DMD/LCD | Opus 5.5 | 5.1 | `pin_render/displays` |
| **5** | 2.6 Banana flipper *(restored)* | **Fable 5.2** | 2.3 | `pin_physics/flipper_banana` |
| **5** | 2.8 Large balls & clearance *(restored)* | Opus 5.5 | 2.3, 4.3 | `pin_tablegen/clearance`, `pin_physics/tests` |
| **5** | 3.5 DOF bridge *(restored)* | Opus 5.5 | 3.4 | `tools/dof_bridge` |
| **6** | 6.0 Integration & grey-box (M1) | Opus 5.5 | 2.5, 3.4, 4.3, 4.4, 4.6, 5.1 | `pin_app/orchestrator`, `tables/greybox` |
| **6** | 5.4 Ray-query reflections *(restored)* | Opus 5.5 | 5.2 | `pin_render/raytrace` |
| **6** | 5.5 Backbox playfield & toys *(restored)* | Opus 5.5 | 2.3, 4.3, 5.3 | `pin_render/backbox`, `pin_tablegen/backbox` |
| **6** | 2.9 Autoplay bot & soak harness | Opus 5.5 | 2.5 | `pin_sim/autoplay` |
| — | **Gate A1** (automated) | Orchestrator | 6.0, 2.9 | Physics reference suite + grey-box soak |
| **7** | 6.1 Apex Circuit | Opus 5.5 | Gate A1 | `tables/apex_circuit` |
| **7** | 6.2 Sky Deck | Opus 5.5 | Gate A1 | `tables/sky_deck` |
| **7** | 6.4 Feature Lab *(restored)* | Opus 5.5 | Gate A1, 2.6, 2.7, 2.8, 5.5 | `tables/feature_lab` |
| — | **Gate A2** (automated) | Orchestrator | 6.1, 6.2, 6.4 | Table soak; rules frozen |
| **8** | 6.3 Wasm rule ports *(restored)* | Opus 5.5 | 4.5, 6.1, 6.2, 6.4, Gate A2 | `tables/*/rules-wasm` |
| **8** | 7.1 Profiler & soak | Opus 5.5 | 6.1, 6.2 | `pin_app/profiler` |
| **9** | 7.2 Packaging & release | Opus 5.5 | 6.3, 7.1 | `scripts`, `.github/workflows/release.yml` |

| — | **Final Human Acceptance** (Phase 8) | Human | 7.2 | Feel, hardware, visuals, real assets |

**Totals:** 37 subphases · 10 waves (0–9) · Waves 0–5: Opus 5.5 × 23, Fable 5.2 × 4 · Waves 6–9: routed per D34, expected Opus 5.5 or lower for all 10 remaining subphases plus fix work, Fable 5.2 only by exception · 2 automated gates · 1 human step at the end.

---

## 6. Library Verification Results (checked 2026-11-11)

Subphase 0.1 re-runs these checks as executable spikes and pins the versions in `versions.toml`. Items marked **open** are settled there, with the listed fallback chosen automatically if the answer is negative.

| # | Assumption | Result | Effect on the plan |
|---|---|---|---|
| 1 | `sdl3` crate covers events, gamepad, haptics, audio streams and wgpu window handles | **Mostly confirmed.** The crate (0.14.x) has `audio` (streams with callbacks), `event`, `gamepad` and `haptic` modules and a `raw-window-handle` feature for wgpu. Its README says the port from the SDL2 bindings is still in progress and some features may be missing. SDL3 itself stamps events in nanoseconds. **Open:** whether the high-level `Event` type exposes that timestamp | Subphase 1.3 checks first. Fallback: read the raw event through the crate's `sys` re-export (`sdl3-sys`) |
| 2 | `parry3d` has a rotating shape-cast | **Confirmed.** 0.26.0 exports `cast_shapes_nonlinear` and `NonlinearRigidMotion`. It can return an "unsupported" error for some shape pairs | 2.3 uses it for capsule-vs-ball; 2.6 uses its own conservative advancement for the arc shape |
| 3 | `mlua` supports the sandbox controls | **Confirmed.** `lua54` + `vendored` features; `set_memory_limit`; `set_hook` with an every-N-instructions trigger | 4.2 proceeds as written on `mlua` 0.11 |
| 4 | `Mailbox` / `Immediate` present modes are available for secondary surfaces | **Partly.** Only `Fifo` is guaranteed. `Mailbox` is listed for DX12 on Windows 10, NVIDIA on Vulkan and Wayland on Vulkan; `Immediate` for most platforms except older DX12 and Wayland | §2.2 now selects from reported capabilities and falls back to a presenter thread |
| 5 | `wgpu` ray queries work on Vulkan, DX12 and Metal | **Conflicting sources.** The feature is experimental. The official `Features` docs list Vulkan only; the project's tracking issue shows DX12 and Metal work landed; third-party reports on wgpu 30 say DX12, Vulkan and Metal (macOS 15) all expose it. **Open** | 5.4 detects at runtime and its first task is to confirm on the pinned version. Probes remain the baseline |
| 6 | How DOF receives events | **Confirmed.** DOF runs as a plugin of B2S.Server and is fed table-element updates (type, number, value) by its host | Shapes the bridge design in 3.5 |

Sources: docs.rs/sdl3 · docs.rs/crate/parry3d/latest · docs.rs/crate/mlua (tests/hooks.rs, tests/memory.rs) · docs.rs/wgpu-types `PresentMode` · docs.rs/wgpu `Features` · github.com/gfx-rs/wgpu/issues/6762 · directoutput.github.io/DirectOutput

---

## Appendix A: File Schemas (normative)

Subphase 1.1 implements these as `serde` types. Names in quotes are interned to `u16` IDs at load. Lengths are mm, masses g, angles degrees. Unknown keys are validation errors.

### A.1 `table.toml`

```toml
schema_version = 1

[meta]
id = "apex_circuit"
name = "Apex Circuit"
author = "Reference"
version = "1.0.0"
script_entry = "rules.lua"            # or "rules.wasm"
playfield_mesh = "playfield.glb"
pitch_deg = 6.5
players_max = 4
palette = ["#0b1d3a", "#f2a900", "#e63946"]   # used by the placeholder asset kit

[balls]
profiles = ["standard_steel", "ceramic_powerball"]
restrict = [ { profile = "jumbo_xl_resin", layers = [0] } ]   # optional

[physics]                              # optional per-table overrides of physics_defaults.toml
# flipper_rubber_damping = 0.35

[cabinet_switches]
left_flipper = 1
right_flipper = 2
left_upper = 3
right_upper = 4
action = 5
start = 6
launch = 7
tilt = 8

[[switches]]
id = "right_orbit_opto"
number = 24
kind = "opto"        # opto | rollover | eddy | leaf | target | trough | eos | spinner | ring
layer = 0
position = [410.0, 720.0, 0.0]         # omit when bound through glTF extras
radius = 12.0

[[coils]]
id = "left_flipper_coil"
number = 1
kind = "flipper"     # flipper | bumper | sling | kicker | vuk | drop_reset | diverter | autoplunger | knocker | flasher

[[lamps]]
id = "overdrive_ready"
number = 40
kind = "insert"      # insert | gi | flasher
color = "#ff7a00"
response = "led"     # led | incandescent

[[magnets]]
id = "orbit_magnet"
number = 1
position = [400.0, 700.0, 0.0]
k_coil = 2.0e-6      # N·m³ at full power
d_min = 8.0
range = 60.0
tau_ms = 60.0

[[flippers]]                           # fields of FlipperConfig (§3.3)
id = "lower_left"
coil = "left_flipper_coil"
eos_switch = "lower_left_eos"
collision_layer = 0
base_position = [180.0, 120.0, 0.0]
surface_normal = [0.0, 0.0, 1.0]
rest_angle_deg = -30.0
sweep_angle_deg = 50.0
eos_angle_fraction = 0.92
shape = { type = "StraightCapsule", length = 76.0, base_radius = 11.0, tip_radius = 6.5 }
staging = { type = "DedicatedButton", input = "left_flipper" }
# torque, inertia, restitution and damping default from physics_defaults.toml

[[splines]]
id = "return_wireform"
kind = "wireform"    # wireform | guide
layer = 2
gauge = 19.0         # wireform only
points = [[60.0, 900.0, 40.0], [40.0, 600.0, 55.0], [150.0, 260.0, 20.0]]
roll_deg = [0.0, 8.0, 0.0]

[[layer_transitions]]
id = "ramp_entry"
from_layer = 0
to_layer = 2
zone = { min = [240.0, 640.0, 0.0], max = [290.0, 680.0, 30.0] }

[[dispensers]]
id = "power_chamber"
number = 1
exit_position = [300.0, 880.0, 10.0]
exit_velocity = [0.0, -0.6, 0.0]       # m/s
contents = ["standard_steel", "standard_steel", "standard_steel", "standard_steel", "ceramic_powerball"]

[[mechs]]
id = "bumper_1"
kind = "bumper"      # bumper | sling | drop_target | spinner | vuk | plunger | kickback | diverter | newton_ball | gate
coil = "bumper_1_coil"
switch = "bumper_1_ring"
position = [260.0, 820.0, 0.0]
# kind-specific parameters default from physics_defaults.toml

[[shots]]                              # used by the autoplay harness for coverage
id = "cross_shot_ramp"
sequence = ["ramp_entry_opto", "ramp_made_opto"]
window_ms = 2500

[[modes]]                              # rule modes the soak must see start at least once
id = "powerball_mania"
start_scene = "scene_powerball_mania"

[[sounds]]
id = "sfx_bumper"
file = "audio/bumper.ogg"
bus = "sfx"          # music | callout | sfx

[[scenes]]
id = "scene_powerball_mania"
file = "display/powerball_mania.toml"

[persist]
keys = ["high_score_1", "high_score_2", "games_played"]

[dof]                                  # optional (Subphase 3.5)
rom_name = "apexcircuit"
map = [ { coil = "left_flipper_coil", type = "S", number = 1 } ]
```

### A.2 `backglass.toml`

```toml
schema_version = 1

[translite]
layers = [
  { file = "backglass/base.png", depth = 0 },
  { file = "backglass/logo.png", depth = 1, lamp_mask = "bg_logo" },
]

[[lamp_masks]]
id = "bg_logo"
lamp = "backglass_logo"
file = "backglass/logo_mask.png"

[score_display]
mode = "dmd"                   # dmd | lcd
dmd = { width = 128, height = 32, color = "orange" }   # orange | red | rgb
lcd = { width = 1920, height = 1080 }

[backbox]                      # optional (Subphase 5.5)
scene = "backbox.glb"
camera = { position = [0.0, -600.0, 300.0], look_at = [0.0, 0.0, 300.0], fov_deg = 35.0 }
collision_layer = 4
```

### A.3 `layout.toml` (input to `pin_tablegen`)

```toml
schema_version = 1

[playfield]
width = 514.0
length = 1070.0
wall_height = 28.0

[[walls]]
points = [[0.0, 0.0], [0.0, 1070.0], [514.0, 1070.0], [514.0, 0.0]]
closed = false
material = "wood"

[[posts]]
position = [120.0, 300.0]
radius = 5.0
rubber = true

[[lanes]]
kind = "inlane"                # inlane | outlane | shooter | orbit
points = [[40.0, 320.0], [60.0, 180.0]]
width = 32.0

[[ramps]]
spline = "return_wireform"     # references table.toml [[splines]]
entry_width = 60.0

[[decks]]
layer = 1
z = 65.0
outline = [[260.0, 700.0], [514.0, 700.0], [514.0, 1070.0], [260.0, 1070.0]]
material = "acrylic"

[[inserts]]
lamp = "overdrive_ready"
position = [257.0, 240.0]
shape = "circle"               # circle | arrow | triangle | rect
size = 18.0
```

---

## Appendix B: UDP Telemetry Protocol v1 (normative)

* Transport: UDP, default destination `127.0.0.1:35120`, configurable. Little-endian. One datagram per tick that has changes; no datagram otherwise.
* Header (16 bytes): magic `"PNTL"` (4) · version `u8` = 1 · flags `u8` · entry count `u16` · tick `u64`.
* Entry (8 bytes): kind `u8` · reserved `u8` · id `u16` · value `f32`.
* Kinds: 0 switch (0/1) · 1 coil (0/1) · 2 lamp (0..1) · 3 GI (0..1) · 4 magnet (0..1) · 5 haptic impulse (strength 0..1; id = haptic source).
* A full-state datagram (flags bit 0 set) is sent on table load and once per second, so late listeners resynchronise.
* Maximum 180 entries per datagram; overflow splits across datagrams with the same tick.

---

## Appendix C: Provisional Physics Defaults

These populate `physics_defaults.toml`. They are engineering starting points, not measurements: Gate A1 holds the analytic checks fixed and may search within the bounds below; final acceptance may retune any of them.

**Material pairs**

| Pair | Restitution | Sliding friction μ |
|---|---|---|
| steel – rubber | 0.65 | 0.80 |
| ceramic – rubber | 0.78 | 0.70 |
| plastic – rubber | 0.84 | 0.80 |
| steel – wood (playfield) | 0.25 | 0.20 |
| ceramic – wood | 0.30 | 0.18 |
| plastic – wood | 0.35 | 0.25 |
| steel – steel | 0.60 | 0.15 |
| steel – plastic | 0.50 | 0.20 |
| resin – wood | 0.30 | 0.22 |

**Tunables and search bounds**

| Parameter | Default | Bounds |
|---|---|---|
| Flipper inertia (76 mm bat) | $1.0 \times 10^{-4}\ \text{kg·m}^2$ | $0.5$–$3.0 \times 10^{-4}$ |
| Flipper power torque | 1.2 N·m | 0.6–3.0 |
| Flipper hold torque | 0.15 N·m | 0.05–0.4 |
| Return spring torque | 0.03 N·m | 0.01–0.08 |
| End-stop restitution | 0.15 | 0.0–0.4 |
| Rubber damping | 0.35 | 0.1–0.7 |
| Rolling resistance coefficient | 0.002 | 0.0005–0.01 |
| Glo-Ball `magnus_coeff` | $1.0 \times 10^{-5}$ kg | 0–$5 \times 10^{-5}$ |
| Magnet remanence $\tau$ | 60 ms | 20–200 |

**Feel windows checked by Gate A1**

| Measure | Window |
|---|---|
| Unloaded flipper stroke time (rest to EOS) | 20–35 ms |
| Exit speed, full shot from a cradled standard ball | 3–7 m/s |
| Bumper kick exit speed | 1.5–4 m/s |
| Slingshot kick exit speed | 1.5–4 m/s |

---

## Appendix D: Subphase Brief Template

The work-classifier fills this in for each delegation (§2.5, §2.6), and the Orchestrator passes it verbatim to the named agent. A brief must be usable without access to this document.

```text
WORK ITEM <id>: <title>          AGENT: task-executor | work-verifier | issue-investigator
ROLE: Implementer | Independent Tester | Fixer | Verifier | Investigator
MODEL: <Haiku 5.5 | Sonnet 5.5 | Opus 5.5 | Fable 5.2>   TIER: T0–T3 (+ D34 evidence if T3)
BRANCH: <branch>   BASE: main   PR: <number or "not yet open">

OWNED PATHS (you may modify only these):
  <paths>
READ-ONLY CONTEXT:
  <public APIs of dependency subphases, with signatures>

GOAL
  <one paragraph>

REQUIREMENTS / DELIVERABLES
  <numbered, each testable; for a CodeRabbit finding: the quoted comment, thread URL,
   file:lines, and what "fixed" means>

CONTRACTS YOU MUST HONOUR
  <relevant excerpts of §3 and Appendices A–C, inlined in full>

RULES
  <§2.3 rules 1–9, inlined in full>

TESTS REQUIRED
  <tests to add or update; existing suites that must still pass>

COMMANDS
  <exact cargo commands with the environment variables for this run>

VERIFICATION CRITERIA
  <numbered list copied from the plan, with tolerances>

VISUAL CHECKLIST (render subphases only)
  <what a correct image must show>

GATE / DONE WHEN
  <the subphase gate from §2.6, inlined, or the checks the work-verifier will run>

WHEN SOMETHING IS UNCLEAR
  Choose the option most consistent with the decision log excerpts below, record it in
  docs/DECISIONS.md, add a line to docs/HUMAN_QUEUE.md, and continue. Do not wait.
  <relevant decision-log rows>

ROLE-SPECIFIC
  Implementer:        deliver code + unit tests; do not read tests/spec_*.
  Independent Tester: you have NOT seen the implementation and must not ask for it.
                      Write black-box and property tests under tests/spec_* from this brief alone.
  Fixer:              smallest change that meets "fixed"; regression test; name the thread(s).
  Verifier:           re-run every gate step; trace each requirement to a passing test;
                      no style or design opinions (CodeRabbit reviews the PR).
  Investigator:       reproduce, root-cause, verdict, fix plan; change no repository files.

REPORT FORMAT
  status: done | pass | fail | blocked | root-caused | not-found
  requirements: <each → met (test) | not met>
  gate results: <per step>
  decisions logged: <list>
  contract-change requests: <list>
```
