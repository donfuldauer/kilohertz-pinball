# Kilohertz Pinball

A cross-platform pinball simulation engine for Windows, Linux and macOS, written in Rust. The name comes from its 1,000 Hz fixed-timestep simulation loop.

**Status:** Waves 0 to 6 of the plan are implemented and merged, including Milestone M1 (the playable grey-box table) and Gate A1 (automated physics and soak gate). Wave 7 (the three production tables) has not started. See [Implemented](#implemented) and [Remaining work](#remaining-work).

## Overview

The engine is designed in [`docs/PLAN.md`](docs/PLAN.md) and built by autonomous coding agents in 10 waves (0 to 9), with one human setup step before the run and one acceptance step after it. Key design points (PLAN sections 1, 2.2 and 2.4):

- **Language and platform layer:** Rust 2024 workspace (toolchain pinned to 1.97.0); SDL3 for windows, display enumeration, input events and audio output.
- **Simulation:** 1,000 Hz fixed timestep (1 ms) on a dedicated sim thread with an accumulator that catches up at most 8 ticks. The render thread reads the latest snapshot through a lock-free triple buffer. Fixed tick order: drain input, scripts, host commands, physics, switch and haptic edges, publish snapshot. No allocation, locks or file I/O inside `step_1ms()`.
- **Physics:** `parry3d` BVH and shape-casts, C2 spline wireform colliders, a custom impulse, spin and friction solver, torque-driven flippers (including curved "Banana" bats and vertical backbox flippers), Magnus effect and magnetic remanence, data-driven ball profiles (steel, ceramic Powerball, UV Glo-Ball, mini, jumbo, captive Newton balls).
- **Rendering:** `wgpu` (Vulkan, DX12, Metal) from one WGSL codebase; PBR, per-ball reflection probes, HDR post-processing, optional hardware ray-query reflections, backglass and DMD/LCD score displays, backbox mini-playfield.
- **Tables:** bundles (`.pinbundle` zip or folder) of glTF 2.0, TOML manifests and sandboxed rules in Lua 5.4 (hot reload) or WebAssembly (`wasmtime`). Bundles are treated as untrusted input.
- **Cabinet:** single window or up to three monitors, X-Arcade profiles, analog plunger and nudge calibration, rumble, 7.1 SSF audio, UDP telemetry and a Windows DirectOutput Framework (DOF) bridge.
- **Determinism:** the same binary on the same machine replays any recording to identical ball state, switch counts and scores. Randomness comes from a sim-owned seeded PCG; no fast-math or FMA contraction; `libm` for transcendentals. Cross-OS replay hashes are reported by CI but do not fail the build.

## Repository layout

| Path | Purpose |
|---|---|
| `crates/pin_core` | Shared contracts (IDs, units, ball profiles, flipper and script contracts, input events, TOML schemas, `FrameStateSnapshot`, triple buffer, SPSC ring). Changes only through [`contracts/CHANGE_REQUESTS.md`](contracts/CHANGE_REQUESTS.md). |
| `crates/pin_physics` | Collision world and C2 splines (`world`), ball solver with CCD, spin, friction, magnets and Newton balls (`ball`), Magnus and remanence (`ball_ext`), flipper solver (`flipper`), Banana flipper (`flipper_banana`), bumpers, slingshots, targets, plunger and tilt (`mech`), debug geometry (`debug`). |
| `crates/pin_input` | Input ingestion (`events`), flipper staging (`staging`), X-Arcade and device profiles (`profiles`), plunger and nudge calibration (`calibration`), rumble/haptics mapping (`haptics`). |
| `crates/pin_script` | Lua 5.4 backend with sandbox, budgets and hot reload (`lua`); `wasmtime` backend with fuel metering (`wasm`); `pin_guest_sdk` for Rust-authored Wasm rules (`guest-sdk`). |
| `crates/pin_assets` | Bundle VFS, zip hardening, glTF loader, `table.toml` / `backglass.toml` validation and guide lints. |
| `crates/pin_tablegen` | Procedural table generator (`layout.toml` to `playfield.glb`), clearance validation, backbox generator. Has a `pin_tablegen` binary. |
| `crates/pin_assetgen` | Code-generated, licence-clean placeholder sounds, fonts and art. Has a `pin_assetgen` binary. |
| `crates/pin_sim` | Headless deterministic `Simulation`, tick order, command application, record/replay, state hash, autoplay bot and soak harness. Has a `pin_sim` binary. |
| `crates/pin_render` | Display topology, `wgpu` context and layer compositor, debug renderer, PBR, reflection probes, HDR post, ray-query reflections, backglass and score displays, backbox renderer, UI overlay. Feature `gpu-tests` enables adapter-dependent tests. |
| `crates/pin_audio` | Three-bus mixer with decoding and SDL output (`mixer`), SSF channel mixer (`ssf`), UDP telemetry (`telemetry`). Feature `sdl` (default) enables the SDL output stream. |
| `crates/pin_app` | The application binary: SDL platform shell, thread orchestrator, menu UI, DOF launcher, latency profiler (stub until Subphase 7.1). Feature `gpu-tests` enables window and GPU tests. |
| `contracts/` | [`CHANGE_REQUESTS.md`](contracts/CHANGE_REQUESTS.md): queue for changes to `pin_core` contracts. |
| `spikes/` | Subphase 0.1 dependency probes (separate workspace; wgpu, SDL3, mlua, wasmtime, parry3d). |
| `tables/greybox/` | Milestone M1 grey-box table: `table.toml`, `layout.toml`, `rules.lua`, backglass and display scenes, `build.sh` to assemble a bundle. See [`tables/greybox/README.md`](tables/greybox/README.md). |
| `tests/spec_ci/` | Standalone crate that checks `.github/workflows/*.yml` conform to the CI spec and guards the proptest workaround. |
| `tools/dof_bridge/` | Windows-only .NET 8 companion that turns UDP telemetry into DirectOutput Framework updates. |
| `scripts/` | [`gate_a1.sh`](scripts/gate_a1.sh), the Gate A1 runner (physics reference suite plus grey-box soak). |
| `third_party/` | Vendored `lua-src` (Lua 5.4.9) and `lewton` (Ogg Vorbis) with Kilohertz hardening patches (`KHZ_PATCHES.md` in each). |
| `physics_defaults.toml` | Global physics tunables (PLAN Appendix C); data-only tuning happens here. |
| `versions.toml` | Dependency pins and toolchain version produced by the Subphase 0.1 spikes. |
| `docs/` | Plan, decision and run logs, authoring guide, protocol and threat model (see [Development process](#development-process)). |
| `.github/workflows/` | `ci.yml` (the CI matrix) and `spikes.yml`. |

## Implemented

Source: [`docs/RUN_LOG.md`](docs/RUN_LOG.md) (reviewer and verifier pass rows, wave gate rows), the git history and the code in `crates/`. Subphase IDs and names follow PLAN section 4 and the wave matrix in section 5.

| Wave | Subphases | Status |
|---|---|---|
| 0 | 0.1 Dependency spikes and version pinning | Passed; CI green on Windows, Linux and macOS. |
| 1 | 1.1 Workspace, contracts and triple buffer | Passed. |
| 2 | 1.2 CI matrix skeleton; 1.3 SDL3 windows and display topology; 2.1 BVH, collision layers and C2 spline colliders; 3.1 Input ingestion and staging; 4.2 Lua 5.4 backend, sandbox and hot reload | All passed; wave gate green on all three OSes. |
| 3 | 1.4 `wgpu` multi-surface context, thread orchestrator and debug renderer; 2.2 CCD, spin, friction, magnets and Newton balls; 3.2 X-Arcade profiles, plunger and nudge calibration; 3.3 Audio mixer; 4.1 Bundle VFS, glTF loader and manifest validation; 4.5 WebAssembly backend | All passed after review fix rounds; wave gate green (CI run 115). Hardening follow-ups 2.1-B1, 3.3-H and 4.1-H also passed. 3.2 passed by Orchestrator ruling (noisy curved-stroke accuracy ruled non-gating). |
| 4 | 2.3 Flipper solver; 2.4 Bumpers, slingshots, targets, plunger and tilt bob; 2.7 Magnus effect and magnetic remanence; 3.4 Haptics bus, SSF mixer and UDP telemetry; 4.3 Procedural table generator; 4.6 Placeholder asset kit; 5.1 PBR pipeline and pinball materials; seam task 2.2-S (moving-body and force-hook seams) | All passed; wave gate green (CI run 215). |
| 5 | 2.5 Headless simulation, state injection and replay; 2.6 Curved "Banana" flipper; 2.8 Large balls and clearance validation; 3.5 DOF bridge; 4.4 Table browser, display setup and input menu; 5.2 Ball reflection probes and HDR post-processing; 5.3 Backglass and dual-mode score display | All passed; wave gate green (CI run 301; replay hash reproducible 10x per machine on all three OSes). |
| 6 | 6.0 Integration and grey-box table (Milestone M1, PR #21); 5.4 Hardware ray-query reflections (PR #22); 5.5 Backbox mini-playfield, vertical flippers and 3D toys (PR #20); 2.9 Autoplay bot and soak harness (PR #19) | All merged; wave gate green (CI run 610). |
| Gate A1 | Physics reference suite and grey-box soak (`scripts/gate_a1.sh`) | Passed (PR #28 plus ball-solver fixes): zero stuck, endless, escaped or non-finite balls at three skill levels, 100 % switch coverage, p99.9 tick time below 0.35 ms. |

Post-wave fixes recorded in the run log (all merged to `main`):

- **w7pre process and docs:** decision and queue entries become one file per unit under `docs/decisions/` and `docs/queue/` (D41); main-branch CI runs are no longer cancelled; the Fable-allowance ask rule (D42); [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md) with accepted risk TM-1 (fix-13, PR #47); telemetry session-id deferral in [`docs/TELEMETRY_PROTOCOL.md`](docs/TELEMETRY_PROTOCOL.md) (fix-12, PR #46).
- **w7pre tests and tables:** proptest/f32 workaround tripwire in `tests/spec_ci`; geometric Banana bat-pose test; grey-box below-deck catch gaps closed; queue rulings on VUK scatter, backbox escape box and ramp corner-clip limit.
- **fix-10 (PR #50):** sRGB-correct display-layer blending through `LayerTarget` / `LayerCompositor` in `pin_render`, wired into the `pin_app` presenter.
- **fix-11 (PR #49):** telemetry ring capacity clamped to a safe minimum with a best-effort warning.
- **fix-14 (PR #48):** motion-blur occlusion limit documented.
- **fix-wireform-flake (PR #52):** guide cast ignores a touching contact when the true closing rate is non-negative, removing a rare property-test flake; grey-box autoplay expectations re-pinned; Gate A1 still passes.

## Remaining work

**Not started (PLAN sections 4 and 5):**

| Wave | Subphase | Notes |
|---|---|---|
| 7 | 6.1 Variant A, "Apex Circuit" (`tables/apex_circuit`) | Depends on Gate A1 (passed). |
| 7 | 6.2 Variant B, "Sky Deck" (`tables/sky_deck`) | Depends on Gate A1 (passed). |
| 7 | 6.4 "Feature Lab" showcase table (`tables/feature_lab`) | Banana flipper, backbox playfield, XL ball, Magnus, remanence. |
| Gate A2 | Table soak and rules freeze | Automated; needs 6.1, 6.2 and 6.4. |
| 8 | 6.3 WebAssembly ports of table rules | Needs 4.5 (done), the three tables and Gate A2. |
| 8 | 7.1 Latency profiler and determinism soak | `crates/pin_app/src/profiler` is currently a stub. |
| 9 | 7.2 Packaging and release | `scripts/`, `.github/workflows/release.yml` (does not exist yet); also generates `docs/HUMAN_ACCEPTANCE.md`. |
| Phase 8 | Final human acceptance | Play each table and judge feel, check hardware (X-Arcade, analog plunger, accelerometer nudge, rumble, SSF, multi-monitor, DOF), check visuals on real displays, decide whether ray-query reflections default on, replace placeholder assets by asset ID. |

**Current milestone:** Milestone M1 (Subphase 6.0) and Gate A1 are complete; the next step is Wave 7.

**Open items for a person** (summarised from the frozen [`docs/HUMAN_QUEUE.md`](docs/HUMAN_QUEUE.md) and the open lines in [`docs/queue/`](docs/queue/)):

- Hardware acceptance of a Fifo-only secondary display (D31) and other real-device checks that CI cannot do.
- Gate A1 feel tuning notes: flipper stop and dead-flip behaviour, mech feel defaults, residual single-tick ball-bat overlaps for large balls.
- Known limits accepted for now: wireform cast corner-clip gap, motion-blur occlusion (issue #14) and telemetry v2 session id (issue #12) kept open by design, VFS TOCTOU (TM-1, see the threat model), noisy curved-stroke plunger accuracy.
- Open queue lines: `w7pre-banana-pose` (flipped bend range for down-table arcs), `w7pre-catch-gaps` (a hand-maintained literal in the grey-box catch zone), `w7pre-proptest-f32` (remove the proptest workaround when the tripwire trips), `fix-11-telemetry-min-ring` (revisit the `#[doc(hidden)]` unclamped constructor).
- Wave 7 will need integration notes carried over from earlier waves (see the end-of-wave summaries in `docs/HUMAN_QUEUE.md`).

## Building and testing

Prerequisites: Rust 1.97.0 (the version CI pins; see `versions.toml`; the workspace `rust-version` is 1.97), CMake and a C toolchain (SDL3 is built from source). On Linux install the SDL3 build headers and, for GPU tests, a Vulkan software driver and Xvfb:

```sh
sudo apt-get install -y --no-install-recommends \
  mesa-vulkan-drivers libvulkan1 xvfb xauth cmake \
  libx11-dev libxext-dev libxrandr-dev libxcursor-dev libxfixes-dev \
  libxi-dev libxss-dev libxtst-dev libxkbcommon-dev libwayland-dev \
  libegl-dev libdrm-dev libgbm-dev libudev-dev libdbus-1-dev \
  libasound2-dev libpulse-dev
```

Commands CI runs (`.github/workflows/ci.yml`):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --exclude pin_render --exclude pin_audio --exclude pin_app         # GPU-free crates
PIN_REQUIRE_GPU=1 cargo test -p pin_render --features gpu-tests -p pin_audio -p pin_app   # needs a GPU adapter; use xvfb-run -a on Linux; PIN_REQUIRE_GPU=1 makes a missing adapter fail instead of skipping
cargo test --manifest-path tests/spec_ci/Cargo.toml                                       # CI workflow conformance
cargo fmt --manifest-path tests/spec_ci/Cargo.toml --all --check                          # CI formatting check for tests/spec_ci
cargo clippy --manifest-path tests/spec_ci/Cargo.toml --all-targets -- -D warnings        # CI Clippy check for tests/spec_ci
```

On Windows PowerShell, set the variable first:

```powershell
$env:PIN_REQUIRE_GPU = '1'
cargo test -p pin_render --features gpu-tests -p pin_audio -p pin_app
```

The DOF bridge is tested on Windows with .NET 8 (`dotnet build` / `dotnet test` under `tools/dof_bridge`). CI also runs a replay-hash job on all three OSes (record a fixture with `pin_sim`, replay it 10 times), a non-gating `cargo llvm-cov` coverage summary, and a manual `gate-a1` job.

Binaries that exist in the workspace:

```sh
# Build the grey-box table bundle (runs pin_tablegen, pin_assetgen and validation)
tables/greybox/build.sh target/greybox

# Headless deterministic run; prints state_hash=<hex>
cargo run --release -p pin_sim -- target/greybox --ticks 10000 --record-inputs /tmp/run.rec
cargo run --release -p pin_sim -- target/greybox --replay-inputs /tmp/run.rec

# Bot soak (also the core of Gate A1)
cargo run --release -p pin_sim -- soak target/greybox --games 100 --skill standard --out target/soak

# Full gate (see the header of scripts/gate_a1.sh for the criteria)
GAMES=1000 scripts/gate_a1.sh

# The application: windowed, with the grey-box table
cargo run --release -p pin_app -- --table target/greybox
# or without SDL windows or GPU:  cargo run -p pin_app -- --headless-ticks 1000 --table target/greybox
```

`pin_tablegen` (`<layout.toml> --table <table.toml> --out <bundle_dir>`) and `pin_assetgen` (`--title <t> --out <dir>`) can also be run directly; see [`docs/AUTHORING.md`](docs/AUTHORING.md) for table authoring.

## Development process

The project is built by AI coding agents under the rules in PLAN sections 2.3 and 2.5 to 2.7:

- **Model routing (section 2.5):** every unit of work goes through the `work-classifier` agent, which picks a model tier; a triage fast path handles trivial items.
- **Agents (section 2.6):** `.claude/agents/` defines `work-classifier`, `task-executor` (Implementer, Independent Tester and Fixer roles), `work-verifier` and `issue-investigator`. Agent-written decisions go to one file per unit in `docs/decisions/`, items for a person go to `docs/queue/`.
- **Pull requests (section 2.7):** every change is a PR against `main`; CodeRabbit reviews it, every finding is answered and resolved, and the PR merges once CI is green and the merge conditions hold. Agents do not review their own code.
- **Contracts:** `pin_core` changes only through [`contracts/CHANGE_REQUESTS.md`](contracts/CHANGE_REQUESTS.md).

| Document | Contents |
|---|---|
| [`docs/PLAN.md`](docs/PLAN.md) | Architecture, specifications, phased plan, wave matrix, appendices (v5). |
| [`docs/DECISIONS.md`](docs/DECISIONS.md) | Frozen history of ambiguity decisions; newer ones are in `docs/decisions/`. |
| [`docs/RUN_LOG.md`](docs/RUN_LOG.md) | Per-subphase attempts, reviews, gate results and merged PRs. |
| [`docs/HUMAN_QUEUE.md`](docs/HUMAN_QUEUE.md) | Frozen history of items for a person; newer ones are in [`docs/queue/`](docs/queue/README.md). |
| [`docs/AUTHORING.md`](docs/AUTHORING.md) | How to author tables with `layout.toml` and `pin_tablegen`. |
| [`docs/TELEMETRY_PROTOCOL.md`](docs/TELEMETRY_PROTOCOL.md) | UDP telemetry wire protocol. |
| [`docs/THREAT_MODEL.md`](docs/THREAT_MODEL.md) | Threat model for untrusted bundles and accepted risks. |

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this project by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
