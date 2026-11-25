# Kilohertz Pinball: Technical Specification

This is the normative technical specification of Kilohertz Pinball: architecture decisions, thread model, contracts, component specifications, library findings, presentation requirements and file schemas. It was `docs/PLAN.md` until 2026-11-20. Section numbers, subphase numbers and decision IDs D1–D33 are unchanged, so references such as "PLAN §3.2", "PLAN 2.3", "PLAN Appendix A" or "D9" in code, tests, commits and history resolve to the same place here. The plan is the set of GitHub issues (`work-unit` issues; tracking issues #79 "M1" and #80 "M2"). The process is in `docs/PROCESS.md`. The repository layout and commands are in `docs/DEVELOPMENT.md`.

**History.** v1 to v5 were written as an agent execution plan for an unattended build (subphases 0.1–7.2 in Waves 0–9). v2 settled the v1 audit choices; v3 restored every feature v2 deferred and checked the libraries against current releases (§6); v4 added the written-out schemas and defaults (Appendices A–C), the dependency-spike subphase (0.1), the autoplay soak harness (2.9), the placeholder asset kit (4.6) and automated gates A1 and A2; v5 (from Wave 6) made Fable 5.2 an exception only and added the process decisions D34–D42. That build is recorded in `docs/RUN_LOG.md`. Process material now lives in `docs/PROCESS.md`, `docs/DEVELOPMENT.md` and `docs/BRIEF_TEMPLATE.md`; the D34–D42 rows in §0 point there.


## 0. What Changed From v1 (Decision Log)

Every open choice from the v1 audit is settled here. Agents treat this table as binding.

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
| D9 | Determinism | **Same-build determinism is the gate.** In addition, the `replay-hash` fixture `state_hash` (10 000 ticks) and the seam digests must equal the committed x86_64-linux goldens on aarch64, on Windows x86_64 and on wasm32 under V8, JavaScriptCore and SpiderMonkey. Other cross-OS hashes are reported. The full text is §2.4 | `f32` math differs across platforms/compilers, and `f32` SIMD paths differ by ISA unless pinned |
| D10 | Scripting | **Lua 5.4** (vendored via `mlua`) is the primary backend with hot reload natively, and also runs in the browser (vendored Lua with KHZ 1–38 in the web module). **Wasm (`wasmtime`) restored** in Subphase 4.5, table ports in 6.3. Wasm rules run from a load-time-instrumented module on every platform: `wasmtime` natively and `wasmi` 2.0.2 in the browser; the rewrite's fuel (`wasmtime`'s cost model) and weighted call-depth counter decide every kill point, and the engines' own fuel is a backstop | LuaJIT dropped for its FFI attack surface. Lua/Wasm parity is made testable by keeping rules logic integer-only (§3.4). Exact kill points on every engine |
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
| D25 | Security | Sandboxed Lua stdlib, instruction budget, memory cap, hardened zip extraction. Also: the per-tick budget (D45); logical (LP64) Lua heap accounting with a real-byte backstop at 2× the cap; the web bundle limits (§3.5) | Tables are untrusted downloads |
| D26 | Model routing | *Superseded by D34 from Wave 6.* v4: 5.2 stayed with Opus 5.5; 5.4 and 2.6 went to Fable 5.2; every subphase got a cross-model review | The numerically hard parts of v1's 5.2 and 2.3 now live in 5.4 and 2.6 |
| D27 | Self-validation | An **Independent Tester** task (writes tests from the brief without seeing the code) for every subphase; it no longer has to use a different model. The v4 Reviewer role is replaced by the Work Verifier and CodeRabbit (D35, D36) | Tests written by the implementer alone tend to confirm the implementation rather than the spec |
| D28 | Human involvement | Setup before the run, acceptance after it, nothing between. Ambiguity is resolved by the agent, logged, and reviewed at the end | Requirement: run as unattended as possible |
| D29 | Failure policy | Three fix attempts, then an Issue Investigator pass and one attempt one tier up (never into Fable unless D34 holds), then mark blocked. A blocked restored feature is skipped; a blocked core subphase halts only its dependents | The run should degrade, not stop |
| D30 | Image tests | Analytic pixel assertions first, regression snapshots second, the Work Verifier's visual checklist third | A golden image generated by the code under test proves nothing on its own |
| D31 | Hardware | Simulated in CI (virtual joysticks, recorded sensor traces, mock displays, loopback UDP). Physical checks are listed for final acceptance, never gated | No cabinet is attached to CI |
| D32 | Assets | A code-generated, licence-clean placeholder kit (sounds, fonts, art) with stable IDs | Tables must be complete without sourcing third-party assets; real assets drop in later by ID |
| D33 | Unverifiable features | Ray-query reflections ship **off by default** unless a ray-tracing-capable runner validated them; the DOF bridge ships marked "unverified on hardware" | Honest defaults for anything CI could not exercise |
| D34 | Fable 5.2 use | Process: moved to PROCESS.md P4 | |
| D35 | Agent roles | Process: moved to PROCESS.md P5 | |
| D36 | Code review | Process: moved to PROCESS.md P6 | |
| D37 | Review threads | Process: moved to PROCESS.md P6 | |
| D38 | Triage fast path and per-push batching | Process: moved to PROCESS.md P4 and P5 | |
| D39 | Wave completion record | Process: moved to PROCESS.md P5 | |
| D40 | CI split | Process: moved to PROCESS.md P6 | |
| D41 | Decision and queue entries are one file per unit | Superseded; see `docs/DECISIONS.md`, "Process — One decision log and a P8-only queue (D41 superseded)" | |
| D42 | Fable allowance ask | Process: moved to PROCESS.md P4 | |
| D43 | Browser target | A `wasm32-unknown-unknown` build plays the included tables and player bundles on desktop and mobile browsers. WebGPU is primary, with a WebGL2 fallback module. It is served locally from 127.0.0.1 with a strict CSP and no COOP/COEP. The native-only features are: multi-monitor and presenter threads, DOF, UDP telemetry, SSF 7.1, X-Arcade gamepad auto-detection, hot reload, the latency profiler, ray-query reflections, rumble and accelerometer nudge (an X-Arcade in keyboard-encoder mode works). Specified in Phase 9 (REQ-WEB) | The core crates are portable and the spikes measured exact parity |
| D44 | Web loop | Single-threaded. The main thread runs the due 1 ms ticks per animation frame (at most 50, then drops and counts the rest) and renders the latest snapshot. A hidden tab pauses simulated time. An 8 ms per-frame script-time guard applies. The mixer runs in an AudioWorklet. There is no Worker and no `SharedArrayBuffer` (§2.2) | Same tick order and D5 rules as native, at 1–13 % of a 60 Hz frame |
| D45 | Per-tick script budget | 200 000 units per tick (Lua: exact instructions plus KHZ charges; Wasm: `wasmtime` fuel) are shared by the tick's `on_switch_edge` calls and `on_tick`, natively and on the web. Exhaustion is a `BudgetExceeded` fault (§3.4) | 512 edges per tick multiplied the per-callback budget, and the worst tick took about 1 s natively |

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
* **Tables:** Bundle (`.pinbundle` zip or folder) of `glTF 2.0`, `TOML` manifests and sandboxed rules in **Lua 5.4** (hot reload) or **WebAssembly** (`wasmtime`; `wasmi` in the browser).
* **Rendering:** PBR with per-ball reflection probes, plus hardware ray-query reflections where the GPU supports them. The presentation requirements are in §7.
* **Browser:** A `wasm32` build of the same engine plays the included tables and player bundles on desktop and mobile browsers, with WebGPU and a WebGL2 fallback, served locally; replays are bit-identical to the desktop build. Native-only features are listed in D43; the requirements are Phase 9 (§4).
* **Cabinet:** Single-window PiP or up to three monitors (playfield, backglass, DMD/LCD); X-Arcade profiles; rumble; 7.1 SSF; UDP telemetry; DOF bridge on Windows. Cabinet presets and display layouts are specified in §7 and Appendix E.

---

## 2. Workspace, Threads & Agent Protocol

### 2.1 Cargo Workspace

The workspace layout and crate map are in `docs/DEVELOPMENT.md`, "Workspace and crate map".

### 2.2 Thread Model (D6, D7, D19)

| Thread | Owns | Notes |
|---|---|---|
| **Main** | SDL init, windows, event pump | Blocks in `SDL_WaitEventTimeout(1 ms)`. Forwards every input event, with SDL's nanosecond timestamp, into an SPSC ring to the sim thread. Forwards window events to the render thread. Does no rendering. |
| **Sim (1 kHz)** | `pin_sim::Simulation` | Elevated priority. Hybrid sleep + spin timer. Accumulator with at most 8 catch-up ticks, then drops time. |
| **Render** | `wgpu` device, queue, all surfaces | Surfaces are created on main and handed over. Reads latest `FrameStateSnapshot`. Playfield surface uses `Fifo` (or VRR). Backglass and score surfaces redraw only on change and choose from `SurfaceCapabilities::present_modes`: `Mailbox`, else `Immediate`; if only `Fifo` is offered, that surface gets its own presenter thread so it cannot stall the playfield. |
| **Audio** | SDL audio stream callback | Receives triggers and haptic events through lock-free rings. |
| **I/O worker** | File watching, asset loading, script source loading | The only thread allowed to touch the filesystem at runtime. |

**Web build (D44).** The browser has no sim thread, no presenter threads and no I/O worker; everything runs on one thread in this order.

| Context | Owns | Notes |
|---|---|---|
| **Main thread** | DOM listeners, the `requestAnimationFrame` tick batch, rendering | Input listeners stamp events with `event.timeStamp` and queue them. Each animation frame runs the due 1 ms ticks (at most 50, then drops and counts the rest; the "8 catch-up ticks" cap above is native-only), then renders the latest `FrameStateSnapshot`. A hidden tab pauses simulated time. |
| **AudioWorklet** | `pin_audio::Mixer` | Receives triggers by message. |
| **No I/O worker** | | `fetch` and `File` reads are asynchronous and happen outside the tick batch. There is no Worker and no `SharedArrayBuffer`. |

**Tick order inside `Simulation::step_1ms()`** (fixed, deterministic):
1. Drain input events with timestamp ≤ tick time; update cabinet switch states.
2. Deliver switch edges collected last tick to the script, then call `on_tick`.
3. Apply `HostCommandQueue` (coils, magnets, lamps, spawns), then clear it.
4. Step physics (CCD sub-steps inside the tick).
5. Collect new switch edges and haptic events.
6. Publish `FrameStateSnapshot`.

### 2.3 Agent Execution Rules

These rules are now `docs/PROCESS.md` P2 (R1–R12). The nine rules that stood here (crate boundaries, the contract-change protocol D4, GPU-free core tests, no blocking on the sim thread D5, simulated time D8, no test-shaped special cases, agent roles D34/D35, never waiting on a person D28, no self-review D36/D37) are carried there.

### 2.4 Determinism Scope (D9)

* **Gate (D9):** (1) the same binary on the same machine replays any recording to identical ball state, switch-hit counts and scores; (2) cross-platform equality: the `replay-hash` fixture `state_hash` (10 000 ticks) and the seam digests must equal the committed x86_64-linux goldens on aarch64 (qemu-aarch64 on every pull request touching the sim, Apple Silicon on every push to main), on Windows x86_64 (push to main) and on wasm32 under V8, JavaScriptCore and SpiderMonkey (web workflow).
* **To stay equal:** no fast-math flags, no FMA contraction and no explicit fused or SIMD-width-dependent float ops in the sim crates (quaternions through `pin_physics::quat_ops`, `min`/`max` through `pin_core::fmath`), `libm` for transcendental functions (also in tests that build reference inputs and in Lua, KHZ 32), fixed iteration order everywhere, a sim-owned seeded PCG for all randomness, NaNs hashed as one canonical pattern, no relaxed SIMD, and the pinned web target features (`-C target-feature=+simd128`).
* **Parity exceptions:** NaN bits (canonicalised before hashing) and trap or panic texts (not hashed).
* **Report only:** other cross-OS hashes (the soak `cross-os` job) are recorded and divergence is flagged without failing the build.

### 2.5 Model Routing (D34)

Moved to `docs/PROCESS.md` P4 (tiers, the Fable ask D34/D42 and the fast path D38).

### 2.6 Orchestrator Runbook

Moved to `docs/PROCESS.md` P5 and P7 (flow, batching, branches, subphase and wave gates, failure policy, image-test and hardware-simulation policies D30/D31). Gate commands are in `docs/DEVELOPMENT.md`. Gates A1 and A2 stay normative in Phase 6 of §4.

### 2.7 Pull Request, Review & Merge Workflow (D36, D37)

Moved to `docs/PROCESS.md` P6.

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

* `collectgarbage` is removed. At most 100 nested C calls. The heap cap is counted in logical (LP64) sizes, with a real-byte backstop at 2× the cap. Transcendental math goes through the Rust `libm` (KHZ 32).
* **Instrumented-module rule (D10).** Every Wasm rules module is rewritten at load time with fuel and a weighted call-depth counter (depth limit: 256 KiB of estimated frame bytes). It is run by `wasmtime` natively and by `wasmi` on the web; the rewrite's fuel and depth counter decide every kill point.
* **Per-tick budget (D45).** 200 000 units per tick (Lua: exact instructions plus KHZ charges; Wasm: `wasmtime` fuel) are shared by the tick's `on_switch_edge` calls and `on_tick`. Exhaustion is a `BudgetExceeded` fault and the script drops into the safe tilt state.

### 3.5 Bundle Format

* `table.toml` and `backglass.toml` carry `schema_version`.
* Zip extraction rejects absolute paths, `..` components and symlinks, and enforces caps on total uncompressed size and compression ratio.
* **Web limits (D25, D43).** `File.size` is at most 32 MiB, checked before any copy. The web `ZipLimits` are 64 MiB total, 32 MiB per file, compression ratio 100 and 10 000 entries. Folder bundles are not available on the web. Bundles are never stored.

---

## 4. Component Specifications

Each subphase lists target, dependencies, deliverables and verification, as the specification of the delivered component. Performance budget for the whole tick, script included: $99.9\text{th}$ percentile $< 0.35\text{ ms}$.

### Phase 0: Dependency Spikes

#### Subphase 0.1: Dependency Spikes & Version Pinning
* **Target:** `spikes/`, `docs/DECISIONS.md`, `versions.toml`, `.github/workflows/spikes.yml` · **Dependencies:** none
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
* **Target:** `pin_core`, root `Cargo.toml`, `physics_defaults.toml` · **Dependencies:** 0.1
* **Deliverables:**
  1. Workspace with all 10 crates, each with stubbed module directories per §2.1.
  2. Every contract in §3, plus `InputEvent`, `CabinetInput`, `CabinetAxis`, `HapticsTelemetryEvent`, `MaterialPair` and unit-conversion helpers.
  3. The `table.toml`, `backglass.toml` and `layout.toml` schemas **exactly as specified in Appendix A**, the telemetry packet types from Appendix B, and `physics_defaults.toml` populated from Appendix C. The appendix examples are committed as fixtures and must parse.
  3. `TripleBuffer<T: Copy>` with atomic index swapping.
  4. `FrameStateSnapshot`: `Copy`, fixed capacity (16 balls, 16 flippers, 64 dynamic meshes, 256 lamps), carrying tick number, ball position/orientation/velocity/profile, flipper angle and zipper offset, dynamic mesh transforms, lamp and GI state.
  5. `contracts/CHANGE_REQUESTS.md` with the protocol from §2.3.
* **Verification:** `cargo test -p pin_core`. Stress test with a writer and a reader thread over 100,000 publishes: zero torn reads, monotonic tick numbers. TOML round-trip tests for every schema.

#### Subphase 1.2: CI Matrix Skeleton
* **Target:** `.github/workflows/ci.yml` · **Dependencies:** 1.1
* **Deliverables:**
  1. Build + test on `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`.
  2. Jobs: `core-tests` (GPU-free), `gpu-tests` (lavapipe on Linux, WARP on Windows, Metal on macOS), `clippy -D warnings`, `fmt`.
  3. A placeholder `replay-hash` job that Subphase 2.5 fills in.
* **Verification:** All jobs green on the empty scaffold.

#### Subphase 1.3: SDL3 Windows & Display Topology
* **Target:** `pin_app/platform/`, `pin_render/topology/` · **Dependencies:** 1.1
* **Deliverables:**
  1. SDL3 init for video, audio, gamepad and sensor subsystems.
  2. `DisplayTopologyManager`: single-monitor mode (one window with PiP rectangles for backglass and score display) and cabinet mode (up to three borderless fullscreen windows).
  3. Main-thread event pump per §2.2, forwarding timestamped `InputEvent`s and window events through SPSC rings.
* **Verification:** Mock-display tests for viewport math in 1-, 2- and 3-screen topologies. Ring test proving events arrive in order with timestamps intact.

#### Subphase 1.4: `wgpu` Multi-Surface Context, Thread Orchestrator & Debug Renderer
* **Target:** `pin_render/gpu/`, `pin_render/debug/`, `pin_app/orchestrator/` · **Dependencies:** 1.3
* **Deliverables:**
  1. One `wgpu` instance/adapter/device/queue on the render thread, presenting to 1–3 surfaces with the present modes from §2.2.
  2. Thread orchestrator: spawns sim, render, audio and I/O threads; sim thread drives a `SimStep` trait object (stub until 6.0) with accumulator timing and catch-up cap.
  3. Debug line renderer drawing snapshot contents and collider wireframes.
* **Verification:** `pin_app --headless-ticks 10000` completes on simulated time with monotonic snapshots. `gpu-tests`: an offscreen frame of debug lines matches a golden image within tolerance. Non-gating benchmark: real-time drift below 0.1 % over 10 s.

---

### Phase 2: Physics & Headless Simulation

#### Subphase 2.1: BVH, Collision Layers & C² Spline Colliders
* **Target:** `pin_physics/world/` · **Dependencies:** 1.1
* **Deliverables:**
  1. `parry3d` static BVH for walls, posts, targets and toy meshes with an 8-bit `collision_layer` mask (bit 0 main deck, 1 upper deck, 2 wireform, 3 subway, 4 backbox). Per-collider enable flags for drop targets, gates and diverters.
  2. C² interpolating cubic splines (D13): authored as through-points, stored as cubic Bézier segments. Closest-point by Newton iteration, seeded from the ball's previous parameter with a coarse sample table as fallback.
  3. Wireforms as **two rails**: centerline spline, authored roll angle and rail gauge, framed with a rotation-minimizing frame. Single-curve guides for orbit walls.
  4. Layer-transition trigger zones for ramp entrances, VUKs and drop holes.
  5. Pre-stub `ball/`, `flipper/`, `mech/`, `debug/` modules.
* **Verification:** A ball at $8.0\text{ m/s}$ through a $180^\circ$ guide of radius 150 mm, then up a ramp from $Z = 0$ to $65\text{ mm}$: kinetic + potential energy never increases; normal force changes by less than 5 % between consecutive ticks away from entry and exit; no tunnelling; layer mask switches exactly once per transition.

#### Subphase 2.2: CCD, Spin, Friction, Magnets & Newton Balls
* **Target:** `pin_physics/ball/` · **Dependencies:** 2.1
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
* **Target:** `pin_physics/flipper/` · **Dependencies:** 2.2
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
* **Target:** `pin_physics/mech/` · **Dependencies:** 2.2
* **Deliverables:**
  1. Pop bumpers, slingshots, drop-target banks, kickbacks, spinners (one switch pulse per $180^\circ$), VUK scoops.
  2. Spring plunger ($F = -kx$) with analog position input.
  3. Cabinet nudge impulses and a damped spherical-pendulum tilt bob inside a contact ring.
* **Verification:** Bumper ring response, spinner pulse counts against known spin-down, VUK transfer from layer 0 to layer 1, tilt closure above a nudge threshold and none below it.

#### Subphase 2.5: Headless Simulation, State Injection & Replay Harness
* **Target:** `pin_sim`, `pin_physics/debug/` · **Dependencies:** 2.3, 2.4, 3.1, 4.1, 4.2
* **Deliverables:**
  1. `Simulation` implementing the tick order in §2.2 and the `SimStep` trait.
  2. Test API: spawn a ball at a position and velocity, set switch states, advance N ticks, query ball and score state.
  3. Recorder and replayer: per-tick input events, RNG seed and table hash in a binary file (`--record-inputs`, `--replay-inputs`).
  4. Fill the CI `replay-hash` job; add the allocation guard around `step_1ms()`.
* **Verification:** A 10,000-tick recording replays to an identical state hash 10 times in a row. The allocation guard passes over 60,000 ticks with a script loaded.

#### Subphase 2.6: Curved "Banana" Flipper *(restored)*
* **Target:** `pin_physics/flipper_banana/` · **Dependencies:** 2.3
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
* **Target:** `pin_physics/ball_ext/` · **Dependencies:** 2.2
* **Deliverables:**
  1. Magnus force and remanence per §3.2, registered through the `ExternalForce` hooks.
  2. Per-ball magnetisation state added to the ball arena (via a contract-change request).
  3. `low_rem_steel` profile enabled in validation.
* **Verification:**
  1. With `magnus_coeff = 0` and `remanence = 0`, every 2.2 baseline trajectory is bit-identical.
  2. A spinning Glo-Ball in free flight shows lateral acceleration $S_m |\vec{\omega} \times \vec{v}| / m$ within 1 %, and its kinetic energy is unchanged by the Magnus term.
  3. After a holding magnet switches off, `standard_steel` leaves measurably later than `low_rem_steel`, and `low_rem_steel` matches the zero-remanence baseline exactly.

#### Subphase 2.8: Large-Ball Support & Clearance Validation *(restored)*
* **Target:** `pin_tablegen/clearance/`, `pin_physics/tests/` · **Dependencies:** 2.3, 4.3
* **Deliverables:**
  1. `jumbo_xl_resin` profile (79.4 mm) and support for any diameter between 15.88 mm and 79.4 mm.
  2. Clearance validator: for every ball profile a table declares, check lane widths, flipper gaps, wireform gauge, scoop and VUK diameters, and headroom under upper decks. Violations are bundle-validation errors naming the offending feature. A profile can be restricted to listed layers.
  3. CCD and broad-phase margins that scale with radius.
* **Verification:**
  1. A 79.4 mm ball against a 15.88 mm ball conserves momentum to the 2.2 tolerances.
  2. The validator rejects an unrestricted XL ball on a table with a 65 mm deck gap and accepts it once restricted to the main deck's open area.
  3. No tunnelling for the XL ball against a flipper at maximum angular velocity.

#### Subphase 2.9: Autoplay Bot & Table Soak Harness
* **Target:** `pin_sim/autoplay/` · **Dependencies:** 2.5
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
* **Target:** `pin_input/events/`, `pin_input/staging/` · **Dependencies:** 1.1
* **Deliverables:**
  1. Consume timestamped `InputEvent`s; map to cabinet switches and axes; debounce.
  2. `FlipperStagingRouter`: dedicated buttons, parallel linked, dual-stage threshold (30 % lower, 85 % upper by default).
* **Verification:** Synthetic analog ramps and button sequences produce actuation on the exact expected tick in all three modes.

#### Subphase 3.2: X-Arcade Profiles, Plunger & Nudge Calibration
* **Target:** `pin_input/profiles/`, `pin_input/calibration/` · **Dependencies:** 3.1
* **Deliverables:**
  1. Auto-detected X-Arcade presets: keyboard-encoder scancodes and gamepad modes matched by SDL GUID.
  2. Analog plunger filters: deadzone, rest tracking, release-velocity peak detection.
  3. Accelerometer nudge: high-pass filter, gain curve, digital and thumbstick fallback.
* **Verification:** Profile parser and mapping tests for each mode translate to canonical cabinet switches.

#### Subphase 3.3: Audio Mixer
* **Target:** `pin_audio/mixer/` · **Dependencies:** 1.1
* **Deliverables:**
  1. SDL3 audio-stream mixer on the audio thread with music, callout and effects buses, voice limiting and callout ducking.
  2. WAV and Ogg Vorbis decoding; samples supplied as byte buffers by the I/O worker.
  3. Lock-free ring consuming `sounds` from the command queue.
* **Verification:** Offline render tests: trigger-to-first-sample latency within one audio buffer; ducking curve and voice-limit behaviour match expected output.

#### Subphase 3.4: Haptics Bus, SSF Mixer & UDP Telemetry
* **Target:** `pin_audio/ssf/`, `pin_audio/telemetry/`, `pin_input/haptics/` · **Dependencies:** 3.2, 3.3
* **Deliverables:**
  1. Lock-free `HapticsTelemetryEvent` ring fed by the sim thread (coil fire, EOS hit, bumper, ball drop, ball roll with position, mass and speed).
  2. Gamepad rumble output for flippers, slings and bumpers.
  3. Four-corner SSF panner over the 7.1 exciter channels.
  4. Versioned binary UDP telemetry (switch, coil, lamp states) with a written protocol spec (D22).
* **Verification:** Panner gains for a ball moving from $(-150, 100)$ to $(150, 800)$ mm; packet emitted on the same tick as a simulated flipper pulse.

#### Subphase 3.5: DirectOutput Framework (DOF) Bridge *(restored)*
* **Target:** `tools/dof_bridge/` (C#/.NET, Windows only), `[dof]` schema via contract-change request · **Dependencies:** 3.4
* **Deliverables:**
  1. `[dof]` section in `table.toml`: the ROM-style table name DOF configs key on, and a map from engine coils, lamps and switches to DOF table-element type and number.
  2. Bridge process: listens to the UDP telemetry stream, hosts the user's installed DOF, and forwards each change as a (type, number, value) table-element update, which is the form DOF accepts from its host.
  3. Engine starts and stops the bridge with table load on Windows when `[dof]` is present; a no-op on Linux and macOS.
* **Verification:** Windows CI replays recorded telemetry into the bridge with a mock DOF sink and asserts the exact update sequence, forwarded within 2 ms of packet arrival. Builds on other platforms are unaffected. Real hardware is checked at final Human Acceptance; until then the bridge is labelled "unverified on hardware" (D33).

---

### Phase 4: Assets, Scripting, Table Generator & Menu

#### Subphase 4.1: Bundle VFS, glTF Loader & Manifest Validation
* **Target:** `pin_assets` · **Dependencies:** 1.1, 2.1
* **Deliverables:**
  1. VFS over a directory or a `.pinbundle` / `.zip`, with the hardening in §3.5.
  2. `table.toml` / `backglass.toml` parsing, name-to-ID interning, unit conversion, validation with precise error messages.
  3. `gltf` loading of meshes and PBR textures, plus Blender `extras` for collision layers, switch IDs and lamp inserts.
* **Verification:** A mock bundle loads identically from folder and zip. Malicious archives (path traversal, symlink, oversize) are rejected.

#### Subphase 4.2: Lua 5.4 Backend, Sandbox & Hot Reload
* **Target:** `pin_script/lua/` (plus `load_backend()` and a pre-stubbed `wasm/` module) · **Dependencies:** 1.1
* **Deliverables:**
  1. `LuaScriptBackend` on `mlua` (vendored Lua 5.4) exposing a `pinball` module: `pulse_coil`, `hold_coil`, `set_magnet`, `set_lamp`, `set_gi_tint`, `set_score`, `spawn_ball`, `set_diverter`, `set_zipper`, `show_scene`, `play_sound`, `switch`, `tick`, `balls_in_play`, `random`, `persist_get`, `persist_set`.
  2. Sandbox, memory cap and instruction budget per §3.4. Incremental GC stepped with a fixed budget each tick.
  3. Hot reload: the I/O worker (`notify`) reads and syntax-checks `rules.lua`, then hands the source to the sim thread, which calls `apply_reload` at a tick boundary. Optional script hooks `on_save_state` / `on_restore_state` carry state across.
* **Verification:** Scripted switch sequences produce expected command queues. Sandbox escape attempts fail. An infinite loop is aborted within budget. Reload preserves state through the hooks.

#### Subphase 4.3: Procedural Table Generator
* **Target:** `pin_tablegen` · **Dependencies:** 4.1
* **Deliverables:**
  1. CLI turning a declarative `layout.toml` (playfield outline, walls, posts, lanes, ramps as spline through-points, decks, inserts) into `playfield.glb` with correct `extras` tags and flat-colour materials.
  2. Matching collider and spline declarations emitted into `table.toml`.
* **Verification:** Generated bundles pass 4.1 validation; visual mesh and collision geometry agree within 0.1 mm.

#### Subphase 4.4: Table Browser, Display Setup & Input Menu
* **Target:** `pin_app/ui/`, `pin_render/ui/` · **Dependencies:** 1.4, 3.2, 4.1
* **Deliverables:**
  1. `egui-wgpu` overlay toggled by `Esc` or the controller Menu button.
  2. Table browser (preview art, metadata, feature badges, one-click load).
  3. Display panel (single vs. multi-monitor, PiP placement, camera presets, DMD vs. LCD).
  4. Input panel (profile, staging mode, live plunger and nudge calibration, debug wireframe toggle).
* **Verification:** Headless UI state-machine test for directory scanning, table switch lifecycle and `config.toml` persistence.

#### Subphase 4.5: WebAssembly Scripting Backend *(restored)*
* **Target:** `pin_script/wasm/`, `pin_script/guest-sdk/` · **Dependencies:** 4.2
* **Deliverables:**
  1. `WasmScriptBackend` on `wasmtime` implementing `TableScriptBackend`.
  2. Host interface as plain core-Wasm imports under a `pinball` module, mirroring the Lua API one-to-one with integer and `f32` arguments only. Guest exports: `on_init`, `on_tick`, `on_switch_edge`, and optional `save_state` / `restore_state`.
  3. Sandbox: no WASI, memory cap, fuel metering as the per-callback budget, NaN canonicalisation on. Fuel exhaustion drops the script into the same safe state as an aborted Lua script.
  4. Modules are compiled on the I/O worker and instantiated at a tick boundary, so swap follows the same path as Lua reload.
  5. A `no_std` Rust guest SDK so authors (and Subphase 6.3) can write rules in Rust; C, AssemblyScript and TinyGo work against the same imports.
  6. `.wasm` branch in `load_backend()`.
* **Verification:** An integer-only fixture ruleset in `rules.lua` and `rules.wasm` receives the same switch sequence and yields identical `HostCommandQueue` contents on every tick. Fuel exhaustion and memory growth past the cap are contained. The allocation guard passes on the host side of guest calls.

#### Subphase 4.6: Placeholder Asset Kit
* **Target:** `crates/pin_assetgen` · **Dependencies:** 4.1
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
* **Target:** `pin_render/pbr/` · **Dependencies:** 1.4, 4.1
* **Deliverables:**
  1. Clustered forward PBR in WGSL with up to 128 dynamic lights.
  2. Materials: `pbr_chrome_steel`, `pbr_ceramic_pearl`, `pbr_uv_emissive` (responds to GI tint), clear-coated playfield wood, subsurface light inserts with warm-up curves (15 ms incandescent, 1 ms LED), `pbr_clear_acrylic`.
* **Verification:** `naga` validates every module and translates it to SPIR-V, MSL and HLSL without warnings. `gpu-tests` golden image per material.

#### Subphase 5.2: Ball Reflection Probes & HDR Post-Processing
* **Target:** `pin_render/reflections/`, `pin_render/post/` · **Dependencies:** 5.1
* **Deliverables:**
  1. Octahedral reflection probes behind a `ReflectionSource` trait. Budget: at most 2 probe updates per frame at $128^2$, round-robin across balls; balls without a fresh probe reuse the nearest.
  2. HDR chain: physically based bloom, ACES tone mapping, velocity-based motion blur on balls, MSAA or SMAA.
* **Verification:** `gpu-tests`: a chrome ball beside a lit red insert shows red in the expected reflection region; bloom energy matches the golden image; with 6 balls the per-frame probe count never exceeds the budget.

#### Subphase 5.3: Backglass & Dual-Mode Score Display
* **Target:** `pin_render/displays/` · **Dependencies:** 5.1
* **Deliverables:**
  1. Backglass: layered translite art with lamp-matrix masks and flasher overlays.
  2. DMD mode: $128 \times 32$ or $192 \times 64$ procedural dot grid with orange, red or RGB dots.
  3. LCD mode: 16:9 layered sprite, vector font and animation compositor.
  4. Routing to PiP regions or dedicated windows.
* **Verification:** `gpu-tests` goldens for DMD text, score formatting and lamp-mask blending.

#### Subphase 5.4: Hardware Ray-Query Reflections *(restored)*
* **Target:** `pin_render/raytrace/` · **Dependencies:** 5.2
* **Deliverables:**
  1. First task: confirm on the pinned `wgpu` which backends report `Features::EXPERIMENTAL_RAY_QUERY` (§6) and record the result in the crate docs.
  2. `RayQueryReflections` implementing `ReflectionSource`, created only when the adapter reports the feature. Bottom-level structures per static mesh; a top-level structure refit each frame for balls, flippers and dynamic meshes.
  3. WGSL ray queries for ball reflections at full resolution and clear-coat reflections at half resolution, with a roughness cutoff above which the probe result is used.
  4. Setting: Off / Balls only / Balls and clear-coat. Automatic fallback to probes when the feature is absent or the device is lost.
* **Verification:** With the feature absent, output is identical to the probe path. On a ray-tracing-capable runner, a two-ball scene shows each ball reflected in the other (which probes cannot do) and matches a golden image. Non-gating benchmark: added GPU time recorded per quality setting.

#### Subphase 5.5: Backbox Mini-Playfield, Vertical Flippers & 3D Toys *(restored)*
* **Target:** `pin_render/backbox/`, `pin_tablegen/backbox/`, `pin_physics/tests/` · **Dependencies:** 2.3, 4.3, 5.3
* **Deliverables:**
  1. `[backbox]` section in `backglass.toml`: a 3D scene composited with depth between translite layers on the backglass surface, with its own camera.
  2. Mechanical toys driven by dynamic mesh transforms: score reels and animated mechanisms.
  3. Vertical mini-playfield on collision layer bit 4: $90^\circ$ plane colliders, flippers using the existing staging modes, and a layer transition that hands a ball from the main playfield to the backbox and back.
  4. `pin_tablegen` support for a backbox layout block.
* **Verification:** A flipper on a $90^\circ$ plane produces the shot speed predicted from the horizontal case once gravity along the plane is accounted for. A backbox ball never collides with layers 0–3. Golden image of reels and mini-playfield composited with the translite.

---

### Phase 6: Integration & Tables

#### Subphase 6.0: Integration & Grey-Box Table (Milestone M1)
* **Target:** `pin_app/orchestrator/`, `tables/greybox/` · **Dependencies:** 2.5, 3.4, 4.3, 4.4, 4.6, 5.1
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
* **Target:** `tables/apex_circuit/` · **Dependencies:** Gate A1
* **Deliverables:**
  1. Geometry via `layout.toml`: $6.5^\circ$ playfield, dual 76 mm lower flippers with Zipper mode ("Firewall Save"), 55 mm upper-left flipper at 10 o'clock fed by a wireform return or left-orbit pass, dispenser holding 4 steel balls and 1 Powerball, right-orbit magnet with eddy sensor and opto, 3-bumper cluster, left-orbit spinner, 3-bank drops, centre lock scoop, upper-right cross-shot jackpot ramp.
  2. `rules.lua`: attract → 3-ball game → skill shot → upper-flipper combo multipliers (2×–5×) → Powerball Mania (2× scoring; Powerball detected by opto closing while the eddy sensor stays open) → bonus and high-score entry.
  3. Action switch: when "Overdrive Ready" is lit, either close the Zipper flippers for 5.0 s or cash a decaying "Apex Jackpot."
* **Verification:** Headless scripted game with state injection: skill shot, cross-shot ramp, Powerball dispense, magnet bypass, action switch; assert scores and display scenes.

#### Subphase 6.2: Variant B — "Sky Deck"
* **Target:** `tables/sky_deck/` · **Dependencies:** Gate A1
* **Deliverables:**
  1. Geometry: main deck at $Z = 0$ with lower flippers, slings, lanes, centre VUK and a left ramp with diverter; clear-acrylic upper deck at $Z = 65\text{ mm}$; 52 mm upper-right flipper on the upper deck (`collision_layer = 1`); captive Newton ball guarding "Reactor Core"; elevated 3-bank drops; upper-left wrap-around loop; acrylic return funnel.
  2. `rules.lua`: action switch toggles the "Sky Bridge Diverter" for 8.0 s in normal play; after the Newton ball and drop bank are complete, it starts "Night Overdrive": UV GI tint plus a Glo-Ball on the upper deck alongside a steel ball below.
* **Verification:** Headless scripted run: VUK transition to layer 1, upper-flipper shots, Newton ball strike, drop bank, Glo-Ball launch, and proof that balls on different layers never collide.

#### Subphase 6.4: "Feature Lab" Showcase Table *(restored features, playable)*
* **Target:** `tables/feature_lab/` · **Dependencies:** Gate A1, 2.6, 2.7, 2.8, 5.5
* **Deliverables:**
  1. Geometry: a Banana lower-right flipper opposite a straight lower-left; a backbox mini-playfield with one vertical flipper, fed by a VUK; an open lower area cleared for the 79.4 mm ball; a holding magnet with a dispenser that can serve `standard_steel` or `low_rem_steel`.
  2. `rules.lua`: a mode selector on the action switch cycling through Banana shots, backbox play, XL-ball mode, magnet release comparison, and Glo-Ball mode with Magnus enabled.
* **Verification:** Headless scripted run exercising each mode with state injection; clearance validator passes with the XL ball restricted to its area.

**Gate A2 (automated, after Wave 7): table soak.** For each of the three tables, 1,000 autoplay games at each skill level:
1. Zero stuck, escaped or non-finite balls; zero script errors; zero command-queue overflows.
2. 100 % switch coverage; every declared shot made at least once; every rule mode started at least once.
3. Median game length at the middle skill level between 1 and 6 minutes; scores increase with skill level.
4. Deterministic replay of 20 sampled games per table.

Rules are frozen after A2. Later tuning is data-only (`physics_defaults.toml` and table TOML), and the Lua/Wasm parity test stays in CI so any later rule change is caught. For the reference tables the freeze is lifted until Gate A3 passes (§7, REQ-TBL-003).

#### Subphase 6.3: Wasm Ports of Table Rules *(restored)*
* **Target:** `tables/*/rules-wasm/` · **Dependencies:** 4.5, 6.1, 6.2, 6.4, Gate A2
* **Deliverables:**
  1. Rust ports of the three tables' rules on the guest SDK, each built to `rules.wasm` and shipped beside `rules.lua`.
  2. CI step that rebuilds each `rules.wasm` and compares it with the committed binary.
* **Verification:** For each table, recorded replays produce identical per-tick command queues and identical final scores under the Lua and Wasm backends.

---

### Phase 7: Profiling & Release

#### Subphase 7.1: Latency Profiler & Determinism Soak
* **Target:** `pin_app/profiler/` · **Dependencies:** 6.1, 6.2
* **Deliverables:**
  1. HUD: $99.9\text{th}$-percentile tick time (target $< 0.35\text{ ms}$), input latency from SDL event timestamp to the tick that applies coil torque (target $\le 1.5\text{ ms}$ at the 99th percentile), GPU frame time, command-queue overflow count.
  2. Soak test: 10-minute recorded multiball replays on both tables.
* **Verification:** Soak replays are deterministic on each platform; cross-OS hashes are reported.

#### Subphase 7.2: Packaging & Release
* **Target:** `scripts/package.*`, `.github/workflows/release.yml` · **Dependencies:** 6.3, 7.1
* **Deliverables:**
  1. Release builds for Windows, Linux and macOS (universal `.app`, adding `x86_64-apple-darwin`).
  2. `greybox`, `apex_circuit`, `sky_deck` and `feature_lab` as `.pinbundle` archives beside the executable, each with `rules.lua` and, where ported, `rules.wasm`.
  3. The DOF bridge included in the Windows package only.
  4. `docs/HUMAN_ACCEPTANCE.md`, generated from the run log: the Phase 8 checklist with this run's blocked items, logged decisions, D33 flags and gate reports filled in.
* **Verification:** Clean `cargo build --release --workspace` and `cargo test --release --workspace` with zero warnings on every target.

---

### Phase 8: Final Human Acceptance (the only human step after setup)

Work through `docs/HUMAN_ACCEPTANCE.md`:
1. **Review the run.** Read the open P8 items in `docs/HUMAN_QUEUE.md` and the decisions in `docs/DECISIONS.md`; reverse any decision you disagree with by asking for a new work-unit issue (`docs/PROCESS.md` P1).
2. **Play each table.** Judge flipper feel, ball speed, catches and nudge. Tune only data (`physics_defaults.toml`, table TOML), then rerun CI; gates A1 and A2 revalidate automatically.
3. **Check hardware.** X-Arcade in each mode, analog plunger, accelerometer nudge, rumble, SSF channels, multi-monitor layout, DOF devices.
4. **Check visuals.** Reflections, bloom, DMD and backglass on a real display; ray-query reflections on a capable GPU, then decide whether to enable them by default.
5. **Replace placeholders.** Swap real sounds, music, callouts and art in by asset ID.

### Phase 9: Browser Version (`WEB`)

The browser build is a `wasm32-unknown-unknown` build of the same engine (D43, D44). It is served locally (`node scripts/web/serve.cjs`, 127.0.0.1, strict CSP, no COOP/COEP). WebGPU is primary and WebGL2 is a fallback, shipped as two artifacts. Mobile is in scope. Replay parity is exact across x86_64 Linux and Windows, aarch64 macOS and wasm32 on V8, JavaScriptCore and SpiderMonkey (D9), apart from NaN bits and trap texts. The build host is Linux x86_64 (including WSL2) or macOS 15+ (arm64 and Intel), with pins in `scripts/web/tools.lock`; the cross-host gate is behaviour, not bytes.

#### 1. Engine and parity

- **REQ-WEB-001** The web build plays the included tables and player bundles with the same `pin_sim`, `pin_physics`, `pin_input`, `pin_script`, `pin_assets`, `pin_audio` and `pin_render` code as the desktop app.
- **REQ-WEB-002** For any recording within the web limits, every tick's `state_hash`, `ext_hash` and command-queue hash in the web module equal the desktop x86_64 build's, on V8, JavaScriptCore and SpiderMonkey; NaNs hash canonically and trap texts are not hashed.
- **REQ-WEB-003** `step_1ms` makes 0 heap allocations in the web module over 60 000 ticks per included table, with the Lua and the wasmi backend.
- **REQ-WEB-004** A Rust panic in the web module ends in re-instantiation and reload of the current bundle; the page stays usable.

#### 2. Scripting

The vendored Lua 5.4.9 with KHZ 1–38 is compiled into the engine module; `mlua` and the `pin_script` host code are unchanged. Wasm rules run in `wasmi` 2.0.2 from the load-time-instrumented module (§3.4).

- **REQ-WEB-005** Lua runs from the vendored Lua with KHZ 1–38; the 79-case sandbox corpus, the 13 audit probes and the budget-edge probe give the same lines as native.
- **REQ-WEB-006** Wasm rules run in wasmi from the instrumented module; the 82-line adversarial corpus, per-callback fuel and depth limits equal native wasmtime running the same module.

#### 3. Bundles, page and hosting

Player bundles load through one `File` path for the picker and for drop, and are never stored; `localStorage` holds settings and the last-used table only. REQ-WEB-009 sits here (not in part 1) because it gates the bundle loader.

- **REQ-WEB-007** Player bundles load by picker and by drop through one path; a file over 32 MiB is refused before any byte is copied; the hostile zip corpus is refused with the native texts.
- **REQ-WEB-008** Nothing from a player bundle is stored; `localStorage` holds only settings and the last-used table.
- **REQ-WEB-009** A startup self-test replays the CI fixture, checks the glam operation digests and Lua's deepest legal C recursion. On a stack failure player bundles are refused; an engine without a Wasm JIT is reported as unsupported.
- **REQ-WEB-010** Every response of `scripts/web/serve.cjs` carries the CSP `default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`, serves `.wasm` as `application/wasm`, binds 127.0.0.1 by default and sends no COOP/COEP.
- **REQ-WEB-011** The engine module imports only from its wasm-bindgen glue (no `env.*`, no `wasi_*`), uses legacy Wasm EH only, has no relaxed-SIMD opcode and validates with the pinned feature list.
- **REQ-WEB-012** After `wasm-opt -Oz` the engine module is at most 2.5 MiB brotli (WebGPU) and at most 3.0 MiB (WebGL2).
- **REQ-WEB-013** The loader uses the WebGPU artifact when `requestAdapter()` returns an adapter, otherwise the WebGL2 artifact when WebGL2 and `EXT_color_buffer_float` exist, otherwise an "unsupported browser" page.
- **REQ-WEB-014** Floors: WebGPU artifact on Chrome/Edge 113+, Firefox 141+ (Windows) or 145+ (macOS), Safari/iOS 26+, Chrome 121+ on Android 12+. WebGL2 artifact on Safari 16.4+, Firefox 100+, Chrome 96+.

#### 4. Rendering

- **REQ-WEB-015** The web build presents the desktop single-window scene (§7.1–7.4) with the presets of REQ-VIS-005; in the browser job each table's frame passes the D30 analytic checks, and WebGL2 differs from WebGPU on at most 4 pixels per frame.
- **REQ-WEB-016** Every app-path WGSL module translates to GLSL ES 3.00 in a GPU-free test.

#### 5. Loop, input, audio and mobile

- **REQ-WEB-017** Each animation frame runs the due 1 ms ticks, at most 50, then drops and counts the rest; a hidden tab pauses simulated time and audio; resuming takes a fresh epoch.
- **REQ-WEB-018** Once script time in one frame passes 8 ms, stepping stops until the next frame, and after 2 s of throttling the page says the rules are too slow; replay hashes are unchanged.
- **REQ-WEB-019** Keyboard, pointer and touch inputs are applied in the tick their `event.timeStamp` falls in (REQ-LAT-004 on the browser clock); gamepads are polled once per frame.
- **REQ-WEB-020** In the browser job, at most two presented frames pass from a flipper key or touch event to the first frame showing that bat moved at least 1 degree.
- **REQ-WEB-021** Keyboard uses `KeyboardEvent.code` mapped to the HID usages of the shipped profiles; gamepads use the "standard" mapping; touch zones cover both flippers, launch, start and the menu; cameras (REQ-CAM-002) are selectable through the menu, `V`/`B` and the D-pad without any key the browser reserves.
- **REQ-WEB-022** Audio comes from `pin_audio::Mixer` in an AudioWorklet; its output equals native `Mixer::render` bit for bit for the same trigger sequence; it starts on the first user gesture; stereo; decode budget 64 MiB.
- **REQ-WEB-023** On phones the page fills the viewport in portrait and landscape without scrolling (`touch-action: none`), with the score display and backglass off the flipper area.

#### 6. Gate and release

- **REQ-WEB-024** Gate A4 (automated, web) runs on pull requests touching the web determinism paths and on `main`. It covers: REQ-WEB-002, -003, -005, -006, -007, -011, -012 in Node; the Chromium browser checks; the JavaScriptCore and SpiderMonkey shells. Real Safari (macOS and the iOS Simulator) and Firefox run on web-path pull requests and on dispatch. Gate A4 is not part of `pr-checks`.
- **REQ-WEB-025** Draft prereleases carry `kilohertz-web-<version>.zip` (static site plus `serve.cjs`).
- **REQ-WEB-026** The web build runs on Linux x86_64 and on macOS 15+ (arm64 and x86_64) from the same pins. On each host the build, the module checks and Node per-tick parity against the committed goldens pass; the module sha256 is compared across hosts and reported (W20, W26).

---

## 5. Wave & Dependency Matrix

The build history is recorded in `docs/RUN_LOG.md`; the plan is the set of GitHub issues.

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
## 7. Presentation and Play Experience

Added 2026-11-20 after the owner's first final-acceptance session (Phase 8) rejected the build:
the window showed a flat top-down view with the backglass and score display as picture-in-picture
boxes; the tables did not look like real machines and most mechanisms were not drawn; the
flippers reacted about one second after the key press; and the ball's motion looked wrong. Every
gate in Waves 0–9 ran headless, so none of this was measured. This section states what the
player-facing build must do. Requirements carry IDs (`REQ-<AREA>-<NNN>`); work-unit issues in
milestone `M1 — Play experience` cite them. Appendix E gives the real-machine reference and
Appendix F the table-design and visual rules these requirements point to.

### 7.0 The machine is a build, described in data (`BUILD`)

- **REQ-BUILD-001** A table bundle describes a complete physical machine the way a builder would
  specify a real one from scratch. Every part (cabinet, legs and levelers, backbox, glass,
  lockdown bar, rails, buttons, coin door, plunger, speakers, score display, playfield and its
  art, every playfield part, every lamp, GI bulb and flasher, every plastic, ramp, toy and
  decoration) is defined by its type, dimensions, shape, material and finish, and placement.
  Nothing that can be seen or touched is hard-coded in the generator, the simulation or the
  renderer: they read it all from the bundle.
- **REQ-BUILD-002** Placement uses two documented frames: the playfield frame of Appendix A (mm,
  x across, y up the table, z up from the playfield surface) for everything mounted on or above
  the playfield, and a cabinet frame for the cabinet, legs and backbox and what is mounted on
  them. The playfield's mount in the cabinet (its position and angle relative to the cabinet
  body) is itself data.
- **REQ-BUILD-003** Named presets supply defaults so a table states only what differs: cabinet
  families (REQ-CAB-004), leg styles, and part styles (post, star post, rubber ring sizes and
  colours, pop bumper, slingshot, flipper bat and rubber, drop and stand-up target, spinner, gate,
  lane guide, wireform, plastic ramp, plastic, GI bulb, flasher dome, insert). Any preset field
  can be overridden per table and per part instance.
- **REQ-BUILD-004** Materials: a library of named materials (render parameters, Subphase 5.1)
  that any part can reference and any table can extend. For a part the ball can touch, the same
  entry names its physical material in the material-pair table (Appendix C), so a part that looks
  like rubber also plays like rubber.
- **REQ-BUILD-005** Units are mm, g and degrees in files (D17) and unknown keys are validation
  errors (Appendix A). New schemas and fields are documented in Appendix A by the work unit that
  adds them. Defaults reproduce today's tables where a field is not given.

### 7.1 The machine in 3D (`VIEW`)

- **REQ-VIEW-001** In single-window mode the default presentation is one perspective 3D scene of
  the whole pinball machine, seen from the player's position: the cabinet on its legs, the
  playfield inside the cabinet at the attitude its legs give it (REQ-CAB-008) under the glass, and
  the backbox standing on the rear of the cabinet with the backglass (translite) in its upper part
  and the score display (DMD or LCD, §4 Subphase 5.3) in the speaker panel below it. Its size and
  shape come from the table's cabinet geometry (REQ-CAB-003).
- **REQ-VIEW-002** The backglass and the score display are drawn on the 3D backbox from the
  output of the Subphase 5.3 renderers, as textures sampled in the scene (emissive, behind
  glass). The picture-in-picture layout of Subphase 1.3 stays available as an option and is off
  by default.
- **REQ-VIEW-003** Multi-monitor cabinet mode (Subphase 1.3) keeps dedicated windows for the
  backglass and the score display; the playfield window uses the Cabinet camera (REQ-CAM-004) and
  draws no backbox.
- **REQ-VIEW-004** The machine stands in a simple room (floor, back wall, soft ambient light and
  one key light) so no camera shows an empty black void around it.
- **REQ-VIEW-006** The in-game menu of Subphase 4.4 (`Esc` or the controller Menu button) and its
  `config.toml` are live in the app: the overlay opens over the running game, and settings
  (display, camera, quality, input) apply and persist.
- **REQ-VIEW-005** Moving cabinet parts follow the inputs: the flipper buttons press in with the
  flipper switches, the plunger rod pulls back and releases with the launch input or analog
  plunger, and the start and action buttons light and press.

### 7.2 Cameras (`CAM`)

- **REQ-CAM-001** At least ten desktop camera presets, each a distinct view of the machine
  (different eye position, target, field of view, or framing rule), covering: views that show the
  whole playfield with the backbox above it, views zoomed in on the playfield with no backbox in
  frame, far views that show the whole machine, angled views that show the cabinet sides, and
  ball-following views. The required set is:

  | # | Preset | Kind | Must keep in frame |
  |---|---|---|---|
  | 1 | Standard (default) | static | whole playfield, whole backbox |
  | 2 | Standard Low | static | whole playfield, score display; translite may be cropped |
  | 3 | Full Machine | static | the whole machine including legs and floor contact |
  | 4 | High Overhead | static | whole playfield; backbox out of frame |
  | 5 | Top-Down | static | whole playfield seen along the playfield normal; backbox out of frame |
  | 6 | Close Playfield | static | whole playfield filling the window height; backbox out of frame |
  | 7 | Flipper Zone | ball-following | flippers, slings and inlanes always; follows the ball up the table |
  | 8 | Follow Close | ball-following | the ball and a 300 mm radius around it; backbox out of frame |
  | 9 | Follow Wide | ball-following | the ball and half the playfield; backbox enters when the ball is in the top third |
  | 10 | Left Three-Quarter | static | whole playfield and the left cabinet side art |
  | 11 | Right Three-Quarter | static | whole playfield and the right cabinet side art |
  | 12 | Backbox Showcase | static | whole backbox and the top third of the playfield |

  Exact eye, target and field-of-view values are set by the camera work unit and tuned at final
  acceptance; the "must keep in frame" rule is normative.
- **REQ-CAM-002** Presets are selected in the menu's Display panel (by name, in the order above),
  directly with `F1`–`F12` (presets 1–12), cycled with `V` (next) and `B` (previous), and with
  D-pad right/left on a gamepad, at any time including during play. Camera keys never produce a
  cabinet input and no cabinet input in a shipped input profile uses them. The choice persists in
  `config.toml` globally, with an optional per-table override.
- **REQ-CAM-003** A Custom preset lets the player adjust distance, height, pitch, field of view and
  lateral offset from the menu; it is saved like REQ-CAM-002.
- **REQ-CAM-004** A Cabinet preset for a physical cabinet whose playfield monitor lies flat: the
  playfield only, projected for a horizontal screen viewed from the player's eye point, with
  rotation for portrait-mounted monitors. It is the default in cabinet mode and is not in the
  desktop cycle.
- **REQ-CAM-005** Ball-following presets track the lowest live ball (the one nearest the flippers)
  with critically damped smoothing whose result does not depend on frame rate, never let the
  tracked ball leave the frame, and never move by more than 3 % of the frame height per frame at
  60 Hz.
- **REQ-CAM-006** Switching presets blends eye, target and field of view over at most 300 ms.
- **REQ-CAM-007** Every preset meets its "must keep in frame" rule, with at least a 2 % margin, at
  aspect ratios 16:9, 16:10, 21:9, 4:3 and 9:16 (a rotated monitor), checked analytically by
  projecting the relevant corners.

### 7.3 Cabinet, backbox and art (`CAB`)

- **REQ-CAB-001** A cabinet model generated from the table's cabinet geometry (REQ-CAB-003) with: cabinet body, legs with
  levelers and leg brackets, lockdown bar, side rails, playfield glass, coin door with coin slots
  and lit buttons, start button, flipper buttons on both sides, action button, plunger with knob,
  rod and housing, backbox with its hinge/bracket, translite frame and speaker panel with two
  speaker grilles and the score-display window.
- **REQ-CAB-002** Art on every visible surface, generated from the table's theme (title, palette,
  motif; Subphase 4.6 placeholder kit): cabinet side art (both sides), front art, backbox side
  art, inside art blades on the cabinet walls above the playfield, apron with score and
  instruction cards, and the translite. Every art surface has a stable asset ID so real art
  replaces it without code or schema changes (D32).
- **REQ-CAB-003** Cabinet and backbox geometry is data, like the rest of a table: a per-table
  `cabinet.toml` (schema documented in Appendix A by the owning work unit, units mm and degrees
  per D17) sets every dimension and shape parameter the generator uses: cabinet width, length,
  front and rear body height, leg length and leveler extension (which together with the body
  heights give the playfield pitch's consistency check), lockdown bar profile and height, side
  rail profile, wall thickness, playfield inset and glass channel; backbox width, height, depth,
  front profile (flat or angled), top profile (flat, angled or arched), translite opening, speaker
  panel height, grille positions and score-display window; button, coin door and plunger
  positions; colours and trim (metal finish, leg and rail colour); and art asset overrides. Nothing
  about the cabinet is hard-coded in the generator or the renderer.
- **REQ-CAB-004** `cabinet.toml` may start from a named preset (`preset = "<id>"`) and override any
  field. The engine ships presets whose dimensions match common real machine body styles
  (Appendix E), the three listed there. Preset names are generic. Without a
  `cabinet.toml` the table gets the `classic_standard` preset.
- **REQ-CAB-005** Validation: the cabinet must contain the table (playfield outline, glass height,
  ramps and toys below the glass, the backbox clear of the playfield); a violation is a
  bundle-validation error naming the field.
- **REQ-CAB-007** Legs are parts like any other: per leg, length, cross-section (square, round or
  tapered, with its dimensions), material and finish (for example chrome, brushed steel,
  powder-coated colour), leg bracket and bolt style, and leveler (thread length, extension, foot
  shape). Front and rear legs, and left and right levelers, are set independently; a leg-style
  preset (REQ-BUILD-003) supplies defaults.
- **REQ-CAB-008** The tilt comes from the build. The playfield's attitude (pitch along the table and
  roll across it) is computed from the cabinet geometry, the playfield's mount in the cabinet and
  the effective length of each leg (leg plus leveler extension), standing on a level floor. The
  simulation's gravity direction is this derived attitude, so lengthening the rear levelers
  steepens the game and an unlevel cabinet pulls the ball to one side, as on a real machine.
  `meta.pitch_deg` becomes optional; when given it must agree with the derived pitch within 0.05°
  (validation error otherwise) and documents the intended pitch. Every preset is level side to
  side and gives its family's usual pitch with default levelers. The work unit that introduces the
  derived gravity changes simulation output for every table (very-high blast radius, PROCESS.md
  P3): it re-baselines replay digests once and re-runs Gates A1 and A2, whose analytic checks use
  the derived pitch.
- **REQ-CAB-006** The example tables each use one of the shipped presets (Appendix E) and keep that
  body style's playfield size; the assignment is in Appendix E.

### 7.4 What you see is what you hit (`VIS`)

- **REQ-VIS-001** Every collider and mechanism the simulation uses (walls, posts, rubbers, guides,
  lanes, ramps, wireforms, decks, bumpers, slingshots, drop and stand-up targets, spinners,
  rollovers, scoops, VUKs, kickbacks, gates, diverters, magnets, Newton balls, plungers,
  flippers, troughs and the apron) has a visual mesh generated from the same parameters as its
  collider. Visual and collision surfaces agree within 0.5 mm. A bundle with a collider that has
  no visual is a validation error.
- **REQ-VIS-002** Moving parts are drawn where the simulation has them on every frame: flipper
  bats (true `FlipperShape`, with rubber), bumper rings and skirts, slingshot arms, drop targets,
  spinner flags, gates, diverters, plunger, kickbacks, toys and balls.
- **REQ-VIS-003** The playfield carries its generated artwork (Appendix F) as a texture under a
  clear coat, with inserts set into it and lit from below.
- **REQ-VIS-004** Lighting and materials: shadows from the room key light and the GI onto the
  playfield (the ball casts a visible contact shadow), GI lamps along the playfield edges and
  under plastics, flashers that light their surroundings and bloom, inserts that light the ball,
  and distinct materials for chrome and brushed steel, white and black rubber, clear and
  coloured plastic with printed art, wireform, wood, glass and translite.
- **REQ-VIS-005** Quality presets (Low, Medium, High) so that the Standard camera renders at the
  display refresh rate (60 Hz minimum) at 1920×1080 on the reference integrated GPU class named by
  the render work unit with Low, and on a mid-range discrete GPU with High.

- **REQ-VIS-006** No ball is ever drawn passing through a drawn surface. Walls and guides inside
  the cabinet are as tall as their part style says (default 35 mm, at least 1.2 ball diameters for
  a guide that must contain a rolling ball) and are drawn and collided at that height; only the
  cabinet's inner walls rise to the glass. Ramps, wireforms and decks clear every layer-0 wall
  they pass over by at least one ball diameter, checked by the bundle validator.
- **REQ-VIS-007** A ball inside a scoop, VUK, saucer, subway or trough is drawn sinking into the
  hole and is hidden below the playfield until it is ejected at the exit; it is never drawn
  jumping across the table.

### 7.4a Feel (`FEEL`)

- **REQ-FEEL-001** Flipper and mechanism defaults sit in the middle of the Appendix C feel windows,
  not at their edges: a full shot from a cradled standard ball leaves at 5.0 ± 0.5 m/s for
  contacts between 35 % and 85 % of the bat (it measured 3.2–3.7 m/s at 35–85 % in Wave 4, at the
  floor of the 3–7 m/s window); stroke time stays inside 20–35 ms; bumper and slingshot exit
  medians in play lie between 2.2 and 3.3 m/s. The coil rise and fall time constants become
  Appendix C tunables with bounds. Retuning is data-only (`physics_defaults.toml`) and changes
  simulation output for every table (very-high blast radius): the unit re-baselines replay
  digests and re-runs Gates A1 and A2. Final acceptance may retune within the bounds.
- **REQ-FEEL-002** The physics checks that showed the solver correct stay in CI as regressions:
  deck acceleration within 0.1 % of 5/7·g·(sin θ − c cos θ); slide-to-roll time within 5 % of
  2v/(7 μ g cos θ); normal restitution within ±0.005 of the material-pair table for approaches of
  0.1 m/s or more; no passive energy gain; free-flight energy drift at most 0.1 %; no tunnelling up
  to 20 m/s.

### 7.5 Responsiveness (`LAT`)

- **REQ-LAT-001** Flipper response: from the SDL timestamp of a flipper key or button press to the
  first presented frame in which that bat has moved at least 1°, at most two display refresh
  intervals (33 ms at 60 Hz) at the 99th percentile, in the real windowed app.
  On CI (software renderer) the same path is checked in frames: at most two presented frames
  from the event to the first frame showing the bat moved.
- **REQ-LAT-002** Real time: the simulation advances at wall-clock rate. Over 60 s of play in the
  windowed app on the acceptance machine the ratio of simulated to wall time is at least 0.999
  and no ticks are dropped. On CI under the software renderer, with the Low quality preset and
  the renderer's worker threads capped to leave one core for the simulation, at most 0.1 % of
  ticks are dropped.
- **REQ-LAT-003** The app can record a latency trace (`--latency-trace <file>`): per input event,
  the SDL timestamp, the tick that applied it, the tick the bat first moved, the frame that drew
  it and that frame's present time. The profiler HUD (Subphase 7.1) shows key-to-coil and
  key-to-photon p50/p99, presented frames per second and dropped ticks.
- **REQ-LAT-004** An input is applied in the first executed tick that has received it and whose end
  is not before its timestamp (§2.2 tick step 1 drains every event in the ring stamped at or before
  the tick's end, so an event that reaches the ring after its own tick has run is applied in the
  next executed tick). The same rule holds inside a stall and for time the catch-up cap dropped.
  Neither the time between process start and simulation start nor ticks dropped before an input
  delay it. From the SDL timestamp to the applying tick the latency is at most 1 ms plus the main
  thread's forwarding delay (it waits in `SDL_WaitEventTimeout` and forwards on waking, §2.2) plus
  the sim thread's wake-up latency while the sim thread keeps up. For an input whose timestamp
  falls inside a stall or inside dropped time, the stall is added to that bound. The first bound
  holds again from the first tick after the catch-up ends. (Root cause of the owner's report: the
  app's sim loop and the simulation's input ingest gate the same timestamp against two different
  epochs, delaying every input by the startup time plus the dropped ticks.)
- **REQ-LAT-005** The input-latency metric of the profiler measures from the SDL timestamp to the
  tick in which the input took effect (for a flipper, the tick its coil is energised), not to the
  tick the sim loop received it.
- **REQ-LAT-006** The playfield surface queues at most one frame ahead of the display
  (`desired_maximum_frame_latency` 1). On a CPU (software) graphics adapter the app warns once
  and starts with the Low quality preset.

### 7.6 Reference tables (`TBL`)

- **REQ-TBL-001** Apex Circuit, Sky Deck and Feature Lab are redesigned to the table-design rules
  of Appendix F and look like finished machines under REQ-VIEW, REQ-CAB and REQ-VIS. Each keeps
  the features its §4 subphase (6.1, 6.2, 6.4) requires. The grey box keeps its geometry (it is
  the Gate A1 reference) and gets the cabinet.
- **REQ-TBL-002** A magnet is a visible, deliberate feature: it sits under a marked insert or a
  visible magnet cap, fires only when the rules arm it for a feature the player can see lit, and
  never acts on the plunge path. Powerball detection uses the eddy sensor and opto pair alone.
  (Measured: Apex Circuit's shooter-lane magnet changes a plunged ball's speed by under 0.7 %, so
  it is not why the ball looked wrong; it moves because a magnet on the plunge path is not a
  feature a player can see or use.)
- **REQ-TBL-003** The rule freeze of Gate A2 is lifted for these tables until Gate A3 passes; then
  it applies again. Every rules change ships its Wasm port (Subphase 6.3) in the same PR.

### 7.7 Gate A3: play experience (`GATE`)

- **REQ-GATE-001** Gate A3 (automated) runs on `main` after the M1 table work and before final
  acceptance, and passes only when all hold, in the real windowed app on CI's Linux runner (Xvfb
  and lavapipe) unless stated:
  1. REQ-LAT-001 measured with injected key events (frame counts; at most two presented frames
     between the event and the first frame showing the bat moved), and REQ-LAT-002;
  2. REQ-VIS-001 and REQ-VIS-006 validators clean for every shipped table, and the REQ-FEEL-001
     and REQ-FEEL-002 checks passing;
  3. REQ-CAM-007 framing checks for every preset and aspect;
  4. the Appendix F screenshot set (every table × every preset) rendered, with its D30 analytic
     assertions passing and the images attached to the gate run;
  5. Gate A2 re-run on the redesigned tables, passing.
- **REQ-GATE-002** Final human acceptance (Phase 8) adds: camera presets, flipper response, the
  look of each table and cabinet, and quality presets on the owner's hardware.

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


### A.3a `glass.toml` (optional bundle file)

```toml
schema_version = 1
height = 112.0                 # mm
```

- `height`: mm, the height of the playfield glass underside above the playfield surface.
- Constraint: finite and > 0; the simulator also requires it to be at least the geometry's maximum z.
- The file is optional. When it is absent, the simulator derives the glass from the table geometry (geometry max z + 50 mm).

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

## Appendix D: Subphase Brief Template

Moved to `docs/BRIEF_TEMPLATE.md`.

---

## Appendix E: Cabinet Presets and Real-Machine Reference (normative)

Presets reproduce three common real body styles (REQ-CAB-004). Values marked † are design values
chosen to be consistent with the measured ones where no measurement was available; every value
can be overridden in `cabinet.toml` and may be retuned at final acceptance. Heights are with the
levelers fully in unless stated.

| Field | `classic_standard` | `classic_widebody` | `modern_standard` |
|---|---|---|---|
| Cabinet width | 560 mm | 630 mm | 559 mm |
| Cabinet body length (plunger excluded) | 1315 mm | 1315 mm | 1308 mm |
| Body height, front / rear | 400 / 605 mm | 400 / 605 mm | 413 / 603 mm |
| Legs (all four, same length) | 724 mm, chrome, ribbed | 724 mm, black | 778 mm, wrinkle black |
| Leveler | 3 in (76 mm) bolt; extension 0–44 mm† | as standard | 3/8-16 × 3 in bolt; fully in adds 25 mm, fully out adds a further 44 mm |
| Floor to lockdown bar top | 910 mm | 910 mm | 914 mm |
| Flipper button centres above floor | 860 mm | 860 mm | 864 mm† |
| Backbox width × height × depth | 730 × 730 × 250 mm | 730 × 730 × 250 mm | 705 × 711 × 250† mm |
| Floor to top of backbox | 1930 mm | 1930 mm | 1911 mm |
| Backglass (glass) | 686 × 479 mm | 686 × 479 mm | 658 × 487 × 3.2 mm |
| Translite visible opening | 656 × 449 mm† | 656 × 449 mm† | 628 × 457 mm† |
| Score display window | DMD 128×32, 320 × 80 mm† | DMD 128×32, 320 × 80 mm† | LCD 16:9, 15.6 in diagonal (345 × 194 mm) |
| Playfield glass | 533 × 1092 × 4.8 mm | 603 × 1092 × 4.8 mm | 533 × 1092 × 4.8 mm |
| Playfield (width × length) | 514 × 1070 mm† | 584 × 1070 mm† | 514 × 1070 mm† |
| Pitch with default levelers | 6.5° | 6.5° | 6.5° |
| Side-to-side | level | level | level |

Notes. The playfield size matches the glass less the 19 mm overhang of the standard body
(514 mm is the Appendix A.3 default). The speaker panel fills the backbox front below the
translite opening, with two round grilles left and right of the score-display window†. Legs bolt
to the cabinet corners; the playfield's mount angle in the cabinet is whatever gives the listed
pitch with level legs.

**Example tables (REQ-CAB-006):** grey box and Apex Circuit use `classic_standard`; Sky Deck uses
`modern_standard` and an LCD score display (Subphase 5.3 LCD mode); Feature Lab uses
`classic_widebody`, whose wider playfield gives the 79.4 mm ball and the backbox mini-playfield
room.

Inventory of a real machine that the generated machine reproduces (REQ-CAB-001, REQ-VIS-001):
cabinet body, side art, front art, legs with levelers, leg brackets, lockdown bar, side rails,
glass, coin door, start button, flipper buttons, action button, plunger, backbox, translite,
speaker panel, score display, inside art blades, apron with cards, shooter lane, trough cover,
playfield with art and clear coat, inserts, GI bulbs, flashers, posts with rubber rings, star
posts, metal and plastic guides, lane guides, rollover switches, stand-up and drop targets,
spinners, pop bumpers (base, body, ring, skirt, cap), slingshots (rubber, kicker arm, plastic
cover), scoops and saucers, VUKs, kickbacks, one-way gates, diverters, wireform and plastic ramps
with flaps and supports, plastics (printed acrylic on posts), magnets under caps or inserts,
flippers (bat, rubber, shaft).

## Appendix F: Table Design Rules and Visual Checks (normative)

**F.1 Layout and flow** (REQ-TBL-001)
1. Two 76 mm lower flippers with the Subphase 2.3 geometry; a slingshot directly above each
   flipper; an inlane and an outlane on each side separated by a post.
2. Every shot (orbit, ramp, scoop, target bank, spinner lane, top lanes) is in view from the
   Standard camera and makeable from at least one flipper. Every ramp and orbit returns the ball
   to an inlane or a flipper; no shot drops the ball straight down the centre.
3. Shot entrances are at least 40 mm wide for the 27 mm ball (wider where a profile needs it,
   Subphase 2.8).
4. The plunge feeds a skill shot (top lanes or a lit target) and never passes an armed magnet
   (REQ-TBL-002).
5. Lit arrow inserts in front of each shot show what to shoot; GI frames the playfield.
6. No resting places outside holes, locks and cradled flippers (Gate A2 stuck-ball check).

**F.2 Rules and feedback**
1. Attract mode with a lamp show; game start, ball save after an early drain, extra ball, match,
   bonus count and high-score entry.
2. Every mode has a start (score-display scene and sound), a goal shown with lit inserts, and an
   end.
3. Every scoring switch has a sound and a lamp or display response.

**F.3 Art**
1. Playfield art: a theme background, outlined shot lanes, insert shapes with labels, the table
   logo, and apron cards with the rules summary and scoring.
2. Cabinet, backbox and translite art share the playfield's theme (title, palette, motif).

**F.4 Screenshot set and checks** (REQ-GATE-001 item 4)
- Set: for each shipped table, every desktop preset at 1920×1080, plus Standard at 1080×1920
  (9:16), rendered on CI (lavapipe) from a fixed replay frame with one ball in play.
- Analytic assertions (D30a) per image: the playfield region is not flat (luminance standard
  deviation above a threshold set by the gate work unit); the ball's contact-shadow region is at
  least 20 % darker than the surrounding playfield; in presets with the backbox in frame the
  score-display region's mean colour matches the display texture within tolerance and the
  translite region is non-uniform; no pixel region at the playfield or cabinet location is the
  clear colour.
- Visual checklist (D30c), checked by the work-verifier opening the images: the machine reads as a
  real pinball machine (legs, cabinet, glass, backbox with translite and lit display below it,
  where in frame); every mechanism is drawn; the playfield has art; no z-fighting, missing faces,
  geometry poking through the glass or cabinet, or stretched textures; display text is readable
  in Standard at 1080p.
