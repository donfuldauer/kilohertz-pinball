# Development guide

This file holds the layout, commands, environment, gates and contract owners. The plan is the
set of GitHub issues labelled `work-unit` (tracking issues #79 M1 — Play experience and #80 M2 —
Backlog). Process rules are in `docs/PROCESS.md` (P1–P8, R1–R12); what the engine must do is in
`docs/SPEC.md` (its section, subphase and D-numbers are those of the former `docs/PLAN.md`).
Any issue whose change makes a section here stale updates it in its own PR.

## Toolchain and pins

- Rust is pinned to exactly **1.97.0**: `versions.toml` `[toolchain]` (`rust = "1.97.0"`, edition
  2024) and every workflow (`RUST_VERSION: "1.97.0"` and `dtolnay/rust-toolchain@1.97.0`; each
  job fails unless `rustc --version` is exactly that). There is no `rust-toolchain.toml`. Bump the
  version in `versions.toml` and the workflow lines together, as a recorded decision in
  `docs/DECISIONS.md` ("Orchestrator — 1.2 review B1").
- Workspace (`Cargo.toml`): resolver 3, edition 2024, `rust-version = "1.97"`,
  `license = "MIT OR Apache-2.0"`, `publish = false`; members `crates/*` and
  `crates/pin_script/guest-sdk`; `spikes/` is a separate workspace and excluded.
- Dependency pins: `versions.toml` `[crates]` holds the exact versions that passed the Subphase
  0.1 spikes and stays unchanged (`spikes/dep_smoke/tests/versions.rs` asserts it agrees with
  `spikes/Cargo.toml` and `spikes/Cargo.lock`). The root `[workspace.dependencies]` copies them
  with their feature sets; later pins are exact (`=x.y.z`), one line per crate, under a comment
  naming the work item that added them (for example `# Wave 3 additions …`; from now on
  `# #<N> KP-NNN additions`). Crates use `<name>.workspace = true`.
- Vendored and patched: `third_party/lua-src` (Lua 5.4.9 build crate) and `third_party/lewton`
  (Ogg Vorbis), each with its hardening patches in `KHZ_PATCHES.md`, wired through
  `[patch.crates-io]`. They are part of the untrusted-table boundary (PROCESS.md R11).
- Native build: SDL3 3.4.20 is built from vendored source and linked statically (CMake and a C
  compiler on every OS). On Linux the native video, audio and input backends are compiled in only
  if their headers are present:

  ```sh
  sudo apt-get install -y --no-install-recommends \
    mesa-vulkan-drivers libvulkan1 xvfb xauth cmake \
    libx11-dev libxext-dev libxrandr-dev libxcursor-dev libxfixes-dev \
    libxi-dev libxss-dev libxtst-dev libxkbcommon-dev libwayland-dev \
    libegl-dev libdrm-dev libgbm-dev libudev-dev libdbus-1-dev \
    libasound2-dev libpulse-dev
  ```
- Targets: `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, `aarch64-apple-darwin` (CI);
  release adds `x86_64-apple-darwin` for the universal macOS package; `wasm32-unknown-unknown`
  for the table rule ports (`tables/*/rules-wasm`). The DOF bridge needs the .NET 8 SDK (Windows).

## Agent environment

Every brief's COMMANDS start from this block (from the Wave 5 `COMMON.md` "ENVIRONMENT", with
the target directory per issue).

* 4 cores, 15 GB RAM, shared by up to four units of work. **Every cargo command:**
  `CARGO_BUILD_JOBS=2` and `CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/<issue number>`
  (the issue's dir, shared by its Implementer and Independent Tester; the work-verifier uses
  `target/<issue number>_ver`).
  **Disk (added after a restart filled the allowance):** also export `CARGO_INCREMENTAL=0`,
  `CARGO_PROFILE_DEV_DEBUG=line-tables-only` and `CARGO_PROFILE_TEST_DEBUG=line-tables-only` for
  every cargo command. Check `df -h /home/user` before big builds; if free space is under 4 GB, run
  `cargo clean -p <crate>` for crates you no longer need and tell the Orchestrator in your report.
  Build only the crates you need (`-p <crate>`); never `cargo build --workspace` unless the brief
  says so.
* Toolchain: rustc 1.97.0 (CI pins it), clippy, rustfmt, cargo-llvm-cov + llvm-tools, lavapipe
  (`mesa-vulkan-drivers`), Xvfb, SDL3 build headers, .NET 8 SDK (`dotnet`). Do not install other
  toolchains. Workspace dependency pins live in the root `Cargo.toml`; you may add exact pins under
  a `# #<N> KP-NNN additions` comment, one line per crate naming your issue. `versions.toml` stays
  unchanged.
* Stale-build hazard: when two agents share a target dir and a result contradicts the other's
  report, `touch` the changed sources and rebuild before calling it a defect.
* GPU tests: `--features gpu-tests` on the render crates, run on lavapipe (`WGPU_BACKEND=vulkan`,
  mesa lavapipe ICD). See `.github/workflows/ci.yml` job `gpu-tests` for the exact env the CI uses.
  Anything that opens a window runs under `xvfb-run -a` (the windowed-app evidence of
  PROCESS.md R12).

Each issue works in its own git worktree `../kilohertz-issue-<N>` (`work-issue` skill §2); the
absolute `CARGO_TARGET_DIR` keeps every build under one `target/` for disk checks and cleanup.

## Workspace and crate map

```text
kilohertz-pinball/
├── Cargo.toml                     # Workspace, exact pins, [patch.crates-io] for the vendored crates
├── versions.toml                  # Toolchain and dependency pins from the 0.1 spikes (unchanged)
├── physics_defaults.toml          # Global tunables (SPEC Appendix C); data-only tuning happens here
├── contracts/CHANGE_REQUESTS.md   # Contract-change queue (PROCESS.md R2)
├── CLAUDE.md                      # Agent instructions
├── .claude/agents/                # work-classifier, task-executor, work-verifier, issue-investigator
├── .claude/skills/                # work-issue, run-plan, plan-from-spec, triage
├── .coderabbit.yaml               # CodeRabbit configuration (path filters, path instructions)
├── .github/
│   ├── workflows/                 # ci.yml, release.yml, rules-wasm.yml, soak.yml, spikes.yml
│   ├── ISSUE_TEMPLATE/work-unit.md
│   └── pull_request_template.md
├── docs/
│   ├── SPEC.md                    # Normative specification (was PLAN.md; numbering unchanged)
│   ├── PROCESS.md                 # How work is done: P1–P8, R1–R12
│   ├── DEVELOPMENT.md             # This file
│   ├── BRIEF_TEMPLATE.md          # The delegation brief
│   ├── DECISIONS.md               # Decision log (append only)
│   ├── HUMAN_QUEUE.md             # P8 items for a person (append only)
│   ├── RUN_LOG.md                 # Attempts, gates, merges (append only)
│   ├── HUMAN_ACCEPTANCE.md        # Final acceptance checklist, generated by scripts/human_acceptance.py
│   ├── AUTHORING.md               # Table authoring with layout.toml and pin_tablegen
│   ├── TELEMETRY_PROTOCOL.md      # UDP telemetry protocol v1 (normative copy of Appendix B)
│   ├── THREAT_MODEL.md            # Untrusted bundles and accepted risks
│   └── briefs/                    # Kept delegation briefs (wave5/, wave6/ are history)
├── scripts/                       # gate_a1.sh · gate_a2.sh · gate_a2_verdict.py · human_acceptance.py
│                                  # package.py · req_coverage.py · tests/ (unittest suites)
├── spikes/                        # Subphase 0.1 dependency probes (separate workspace) and spec_0_1 log tests
├── crates/
│   ├── pin_core/       # ALL shared contracts: ids, units, profiles, flipper, script, sim, input, haptics,
│   │                   # telemetry, materials, schema/, snapshot, triple_buffer, spsc
│   ├── pin_physics/    # world/ · ball/ · ball_ext/ (Magnus, remanence) · flipper/ · flipper_banana/ · mech/ · debug/
│   ├── pin_input/      # events/ · staging/ · profiles/ · calibration/ · haptics/
│   ├── pin_script/     # lua/ (Lua 5.4, sandbox, hot reload) · wasm/ (wasmtime) · guest_sdk/ (host side)
│   │   └── guest-sdk/  # crate pin_guest_sdk: no_std Rust SDK for Wasm table rules
│   ├── pin_assets/     # vfs/ · zip/ · gltf/ · validate/
│   ├── pin_tablegen/   # layout/ → .glb · clearance/ · backbox/ (binary pin_tablegen)
│   ├── pin_assetgen/   # sounds/ · fonts/ · art/ (binary pin_assetgen): placeholder kit, assets.toml
│   ├── pin_sim/        # sim/ · tick/ · commands/ · load/ · replay/ · hash/ · alloc_guard/ · autoplay/ · cli/ (binary pin_sim)
│   ├── pin_render/     # topology/ · gpu/ · debug/ · pbr/ · reflections/ · raytrace/ · post/ · displays/ · backbox/ · ui/
│   ├── pin_audio/      # mixer/ · ssf/ · telemetry/
│   ├── pin_app/        # Binary: platform/ · orchestrator/ · ui/ · dof/ · profiler/
│   └── pin_testmain/   # Dev-only: libtest-compatible arguments for `harness = false` test targets
├── tests/
│   ├── spec_ci/        # Standalone crate: workflow conformance (ci.yml, spikes.yml, …) and the proptest tripwire
│   └── spec_7_2/       # Python spec tests: packaging, release workflow, acceptance generator
├── third_party/
│   ├── lua-src/        # Vendored Lua 5.4.9 build crate with hardening patches (KHZ_PATCHES.md)
│   └── lewton/         # Vendored Ogg Vorbis decoder with hardening patches (KHZ_PATCHES.md)
├── tools/
│   └── dof_bridge/     # Windows-only .NET 8 companion: UDP telemetry → DirectOutput Framework
└── tables/
    ├── greybox/        # Gate A1 reference table
    ├── apex_circuit/   # Variant A; rules.lua, rules-wasm/ → rules.wasm
    ├── sky_deck/       # Variant B; rules.lua, rules-wasm/ → rules.wasm
    └── feature_lab/    # Showcase: Banana flipper, backbox playfield, XL ball, Magnus, remanence
```

| Crate | GPU-free (R5) | Coverage floor | Binaries / features | Notes |
|---|---|---|---|---|
| `pin_core` | yes | 85 % | — | Contracts only; changes through `contracts/CHANGE_REQUESTS.md` (R2). Clippy bans std transcendentals (CR-009). Fixtures in `crates/pin_core/fixtures/`; `tests/schema_fixtures.rs` reads SPEC Appendix A verbatim. |
| `pin_physics` | yes | 85 % | — | Module-level ownership (D3): one owner per module directory. |
| `pin_input` | yes | 85 % | — | |
| `pin_script` | yes | 85 % | — | Sandbox and budgets (R11); `tests/attack_corpus/`. |
| `pin_assets` | yes | 85 % | — | VFS, zip hardening, glTF loader, validation and lints. |
| `pin_tablegen` | yes | 85 % | `pin_tablegen` | `<layout.toml> --table <table.toml> --out <dir>`; `validate <bundle>`. |
| `pin_sim` | yes | 85 % | `pin_sim` | Headless simulation, replay, state hash, autoplay and soak. |
| `pin_assetgen` | yes | none set | `pin_assetgen` | `--title <t> --palette <hex,…> --out <dir>`. |
| `pin_render` | no | 60 % | feature `gpu-tests` | Adapter-dependent tests behind the feature. |
| `pin_audio` | no | 60 % | feature `sdl` (default) | SDL output stream behind `sdl`. |
| `pin_app` | no | 60 % | `pin_app`; feature `gpu-tests` | The application. |
| `pin_testmain` | yes | none set | — | Dev-only test helper. |
| `pin_guest_sdk` | yes | none set | — | Built for `wasm32-unknown-unknown` by the rule ports. |

Dependency direction: `pin_core` at the bottom; physics, input, script, assets, tablegen and
assetgen on top of it; `pin_sim` on those; render and audio beside them; `pin_app` on everything.
Only `pin_app` (and `pin_render`'s window tests) use SDL video.

## Tests

- Unit tests sit next to the code (`#[cfg(test)]`). Implementer integration tests are
  `crates/<crate>/tests/<prefix>_*.rs`. Independent Tester spec tests are
  `crates/<crate>/tests/spec_<id>_*.rs` (`<id>` is the subphase for the v5 suites, `spec_2_5_*`,
  and the lower-case plan ID for issues, `spec_kp001_*`), or the paths the brief assigns.
- Tests that name a SPEC requirement put its REQ ID in the test name or doc comment, so
  `scripts/req_coverage.py --tests` finds it.
- Committed fixtures: `crates/*/tests/fixtures/`, `crates/*/tests/spec_fixtures/`,
  `crates/pin_core/fixtures/` (Appendix A examples), the replay-hash fixture
  `crates/pin_sim/tests/fixtures/replay_table`, `tools/dof_bridge/tests/fixtures/` (recorded
  telemetry), and committed `*.proptest-regressions` seed files.
- SDL tests run headless with `SDL_VIDEO_DRIVER=dummy`, `SDL_AUDIO_DRIVER=dummy`,
  `SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS=1`, from `harness = false` targets so SDL runs on the main
  thread (required on macOS). `PIN_REQUIRE_GPU=1` turns a missing GPU adapter into a failure
  instead of a skip.
- Allocation guard (R4): the `*_alloc` test targets of the touched crate (for example
  `cargo test -p pin_sim --test sim_alloc --test spec_2_5_alloc`,
  `cargo test -p pin_app --test orchestrator_alloc --test profiler_alloc`).

## Commands

Run from the repository root, with the agent environment above.

```sh
# Format and lint (unit gate item 1)
cargo fmt --all --check
cargo fmt --manifest-path tests/spec_ci/Cargo.toml --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p <crate> --all-targets -- -D warnings                      # one touched crate
cargo clippy -p pin_render -p pin_app --features gpu-tests --all-targets -- -D warnings
cargo clippy --manifest-path tests/spec_ci/Cargo.toml --all-targets -- -D warnings

# Tests
cargo test --workspace --exclude pin_render --exclude pin_audio --exclude pin_app   # GPU-free crates
xvfb-run -a env WGPU_BACKEND=vulkan PIN_REQUIRE_GPU=1 \
  cargo test -p pin_render --features gpu-tests -p pin_audio -p pin_app            # GPU crates, Linux
cargo test --manifest-path tests/spec_ci/Cargo.toml                                 # workflow conformance
python3 -I -m unittest discover -s scripts/tests -p '*_test.py'                     # script suites
python3 -I -m unittest discover -s tests/spec_7_2 -p 'test_*.py'                    # 7.2 spec suites

# Coverage (unit gate item 4); per-file lines show the owned modules
cargo llvm-cov --summary-only -p <crate>
cargo llvm-cov --summary-only -p pin_core -p pin_physics -p pin_input -p pin_script \
  -p pin_assets -p pin_tablegen -p pin_sim                                           # what CI prints

# Replay hash x10 (unit gate item 6; the CI replay-hash job)
cargo build --release -p pin_sim --bin pin_sim
rec=target/replay_fixture.khzrec
h=$(target/release/pin_sim crates/pin_sim/tests/fixtures/replay_table --ticks 10000 --record-inputs "$rec")
for i in 1 2 3 4 5 6 7 8 9 10; do
  [ "$(target/release/pin_sim crates/pin_sim/tests/fixtures/replay_table --replay-inputs "$rec")" = "$h" ] || echo "replay $i differs"
done

# Table bundles (each runs pin_tablegen, pin_assetgen and validation)
tables/greybox/build.sh target/greybox
tables/apex_circuit/build.sh target/apex_circuit        # likewise sky_deck, feature_lab
tables/<table>/rules-wasm/rebuild.sh [<out.wasm>]      # rebuild rules.wasm (needs the wasm32 target)

# Running
cargo run --release -p pin_sim -- target/greybox --ticks 10000 --record-inputs /tmp/run.khzrec   # prints state_hash=<hex>
cargo run --release -p pin_sim -- target/greybox --replay-inputs /tmp/run.khzrec
cargo run --release -p pin_sim -- soak target/greybox --games 100 --skill standard --out target/soak
cargo run --release -p pin_app -- --table target/greybox                         # windowed app
cargo run -p pin_app -- --headless-ticks 1000 --table target/greybox             # no window, no GPU

# Gates (see "Gate scripts")
GAMES=1000 scripts/gate_a1.sh
scripts/gate_a2.sh
DRY=1 TABLES="apex_circuit" scripts/gate_a2.sh

# Release and acceptance
python3 -I scripts/package.py --target x86_64-unknown-linux-gnu --out target/package --dry-run
python3 -I scripts/human_acceptance.py            # regenerate docs/HUMAN_ACCEPTANCE.md
python3 -I scripts/human_acceptance.py --check    # exit 1 when stale

# Plan traceability (every REQ ID in docs/SPEC.md traced by a work-unit issue)
python3 -I scripts/req_coverage.py
python3 -I scripts/req_coverage.py --milestone 79 --tests crates tests scripts/tests
python3 -I scripts/req_coverage.py --plan-file <draft.md>

# DOF bridge (Windows, .NET 8)
dotnet build tools/dof_bridge/src/KilohertzDofBridge/KilohertzDofBridge.csproj -c Release
dotnet test tools/dof_bridge/tests/<project>/<project>.csproj -c Release
```

**Tick-time benchmark** (phase gate): callgrind instructions per tick (Ir) of `pin_sim` in release
on the replay-hash fixture, 10 000 and 100 000 ticks minus a 100-tick run, compared with the
previous phase gate's commit on `main`'s first-parent line. Wall-clock medians move 5–9 % between
sessions on the agent VM and cannot resolve the 10 % limit (DECISIONS.md "wave6-gate-benchmark").

`scripts/req_coverage.py` (standard library plus the `gh` CLI): reads every
`REQ-<AREA>-<NNN>` ID in `docs/SPEC.md` (lines marked `WITHDRAWN` excepted) and checks each is
named by a `work-unit` issue body (`gh api -X GET search/issues`, explicit paging) or, with
`--plan-file`, by a local draft. `--milestone` takes a tracking issue (`79` or `#79`) or its title;
`--allow` lists IDs deferred by a recorded decision; `--tests [roots]` also requires each ID in
test code. Exit 0 all traced, 1 a missing or unknown ID, 2 a usage, input or GitHub error. It has
no CI job yet (KP-045, #126; DECISIONS.md "Process — req_coverage without a CI job for now").

## CI

Workflows live in `.github/workflows/`. The single required status check on `main` is
**`pr-checks`** (PROCESS.md P6 step 5); changing that set is a repository setting (P8).

**D40 split.** Pull requests run the Linux set only: `fmt`, `clippy` (Linux), `core-tests`
(Linux), `replay-hash` (Linux), and `gpu-tests` (lavapipe) and `dof-bridge` only when their paths
change. Pushes to `main` and `workflow_dispatch` run everything: the Windows and macOS legs (the
`*-cross` jobs), all GPU adapters and coverage. A red `main` stops merges until a fix or revert
lands (P6 step 5). Pull-request runs are cancelled when superseded; `main` and dispatch runs queue
and complete.

| Workflow / job | Runs | What it does |
|---|---|---|
| `ci.yml` `changes` | always | Lists changed files (merge base, `--no-renames`, fail closed). Outputs `code` (false only when every file is `*.md` or `.coderabbit.yaml`, except `docs/SPEC.md`, which a compiled test reads), `gpu` (`crates/pin_render/*`, `crates/pin_app/*`, `*.wgsl`) and `dof` (`tools/dof_bridge/*`); all true on push and dispatch. |
| `ci.yml` `fmt` | code | `cargo fmt --all --check`, and for `tests/spec_ci`. |
| `ci.yml` `clippy` / `clippy-cross` | code; cross on main | `cargo clippy --workspace --all-targets -- -D warnings` on Linux; Windows MSVC and macOS arm64 on main. |
| `ci.yml` `core-tests` / `core-tests-cross` | code; cross on main | GPU-free crates `cargo test --workspace --exclude pin_render --exclude pin_audio --exclude pin_app`; on Linux also `tests/spec_ci` clippy and tests. |
| `ci.yml` `gpu-tests` / `gpu-tests-cross` | code and gpu; cross on main | `cargo test -p pin_render --features gpu-tests -p pin_audio -p pin_app` with `PIN_REQUIRE_GPU=1`: lavapipe under Xvfb on Linux; WARP (Windows) and Metal (macOS) on main. |
| `ci.yml` `replay-hash` / `replay-hash-cross` | code; cross on main | Record the fixture for 10 000 ticks, replay it 10 times, fail on any mismatch on that machine; print the seam digests (cross-OS report only, D9). |
| `ci.yml` `dof-bridge` | code and dof | `dotnet build` and every `tools/dof_bridge/tests/*/*.csproj` on Windows; .NET coverage reported. |
| `ci.yml` `coverage` | push and dispatch | `cargo llvm-cov --summary-only` over the seven core crates; non-gating (`continue-on-error`); floors are enforced by the work-verifier. |
| `ci.yml` `gate-a1` | dispatch only | `scripts/gate_a1.sh`, soak reports uploaded. |
| `ci.yml` `pr-checks` | always | Aggregates `changes`, `fmt`, `clippy`, `core-tests`, `replay-hash`, `gpu-tests`, `dof-bridge`: fails if `changes` did not succeed or any needed job failed or was cancelled (success and skipped pass). The required check. |
| `release.yml` | PRs touching packaging paths; dispatch; `v*` tag push (owner only) | PR: `dry-run` (script unittest suites, `human_acceptance.py --check` as a warning, `package.py --dry-run` with real bundles). Dispatch or tag: `verify` (release build and tests with zero warnings on Windows x64, Linux x64, macOS arm64 and x86_64), `package`, and on a tag a draft prerelease. Never publishes (P8). |
| `rules-wasm.yml` | PRs and main pushes touching rule ports, `pin_script` or the parity tests | Rebuilds each `tables/*/rules.wasm` twice and compares it with the committed binary; fmt, clippy and tests of each `rules-wasm` crate; the Lua/Wasm parity tests. |
| `soak.yml` | weekly, dispatch, main pushes touching sim, physics, script, app or tables | `cargo test --release -p pin_app --test profiler_soak -- --ignored` on each OS: 10-minute multiball replays per table, `SOAK_HASH` lines compared across OSes (report only). |
| `spikes.yml` | pushes and PRs touching `spikes/**`, `versions.toml`, the workflow, `docs/DECISIONS.md` or `docs/HUMAN_QUEUE.md`; dispatch | The 0.1 spikes on three OSes, and the `spec_0_1` tests of the decision and queue files. |

`tests/spec_ci` asserts the workflow structure; a workflow change updates it in the same PR.

## Coverage floors

Line coverage by `cargo-llvm-cov`, measured by the work-verifier on the touched crates and the
owned modules (PROCESS.md P5 unit gate item 4):

| Crates | Floor |
|---|---|
| `pin_core`, `pin_physics`, `pin_input`, `pin_script`, `pin_assets`, `pin_tablegen`, `pin_sim` | 85 % |
| `pin_render`, `pin_audio`, `pin_app` | 60 % |

A new public function with no test fails even when the crate total passes. `pin_assetgen`,
`pin_testmain`, `pin_guest_sdk`, `tests/spec_ci` and the DOF bridge have no floor set; their
numbers are reported. CI's `coverage` job prints the core-crate summary on `main` and does not
gate.

## Gate scripts

- **Gate A1** (SPEC §4 Phase 6): `scripts/gate_a1.sh`. Physics reference suite on the grey box
  (analytic checks, feel windows, 1,000-game soak at three skill levels, tick time). Environment:
  `OUT` (default `target/gate_a1`), `GAMES` (default 1000), `SEED` (default 1); the criteria and
  the test targets behind them are in the script header. CI: `ci.yml` job `gate-a1`
  (`workflow_dispatch`).
- **Gate A2** (SPEC §4 Phase 6): `scripts/gate_a2.sh`, verdict computed by
  `scripts/gate_a2_verdict.py` into `$OUT/gate_a2_verdict.txt`. Every table with a `build.sh`
  except the grey box, 1,000 games at each skill level, sampled replays. Environment: `OUT`,
  `GAMES`, `SEED`, `THREADS`, `TABLES`, `REPLAYS`, `PATTERN_TICKS`, `DRY=1` (smoke run, never a
  pass), `READINESS=0`; `scripts/gate_a2.sh --verdict-only OUT` recomputes the verdict. Only a run
  with `GAMES>=1000`, `REPLAYS>=20` and no `DRY` prints `GATE A2: PASS`.
- **Gate A3** (SPEC §7.7, REQ-GATE-001): to come with KP-043 (#124): `scripts/gate_a3.sh`, a
  workflow on `main` and on demand, the SPEC Appendix F.4 screenshot set as an artifact.

## Owned paths and shared files

Each issue lists its owned paths (PROCESS.md R1); change nothing outside them. The shared
append-only files are `docs/DECISIONS.md`, `docs/HUMAN_QUEUE.md`, `docs/RUN_LOG.md` and
`contracts/CHANGE_REQUESTS.md`; every issue may append to them (P5 unit gate item 8), and a merge
conflict in one of them is resolved by keeping both sides in order. New exact pins go in the root
`Cargo.toml` under a comment naming the issue. `docs/HUMAN_ACCEPTANCE.md` is regenerated, never
hand-edited. Contract changes after the owning unit merges go through
`contracts/CHANGE_REQUESTS.md` (R2). Blast-radius levels are defined in PROCESS.md P3.

## Contracts (PROCESS.md P3) and owners

v5 subphases are SPEC §4 numbers; `CR-NNN` entries are in `contracts/CHANGE_REQUESTS.md`; `KP-NNN`
are work-unit issues.

| Contract | Where | Owner |
|---|---|---|
| IDs, units, ball profiles, flipper contract, scripting contract, `InputEvent`, `CabinetInput`, `CabinetAxis`, `MaterialPair`, unit helpers (SPEC §3) | `crates/pin_core/src/` | 1.1; later changes by CR |
| SPSC ring | `pin_core::spsc` | CR-001 (with 1.3) |
| `SimStep` | `pin_core::sim` | CR-002 (with 1.4) |
| `HapticsTelemetryEvent` | `pin_core::haptics` | 1.1; rich event CR-003 (with 3.4) |
| `FrameStateSnapshot`, `TripleBuffer` | `pin_core::snapshot`, `pin_core::triple_buffer` | 1.1; mech motion slots and lamp positions KP-008 (#88) |
| `HostCommandQueue` clearing rule | `pin_core::script` | 1.1; doc CR-008 |
| `table.toml`, `backglass.toml`, `layout.toml` (SPEC Appendix A.1–A.3) | `pin_core::schema`, fixtures `crates/pin_core/fixtures/` | 1.1; `[[mechs]]` parameters CR-004 (2.4); `[balls] restrict` area CR-005 (2.8); `[dof]` 3.5 |
| `glass.toml` (SPEC Appendix A.3a) | `pin_core::schema::glass` | CR-007 (PR #26) |
| `cabinet.toml`, legs, levelers and the playfield attitude | SPEC Appendix A (added by the owner) | KP-009 (#89) |
| Part styles, material library, table theme | SPEC Appendix A (added by the owner) | KP-010 (#90) |
| `physics_defaults.toml` and bounds (SPEC Appendix C) | repository root | 1.1; `[mechs]` CR-004; steel–acrylic pair CR-006; feel defaults KP-007 (#87), data only |
| UDP telemetry protocol v1 (SPEC Appendix B) | `pin_core::telemetry`, `pin_audio::telemetry`, `docs/TELEMETRY_PROTOCOL.md` | 1.1 (packet types); 3.4 (encoder, sender, written protocol) |
| Replay file format (`*.khzrec`, game recordings) | `pin_sim::replay` | 2.5; game recordings with Gate A2 (PR #61) |
| `.pinbundle` layout and its hardening | `pin_assets::vfs`, `pin_assets::zip`; packages `scripts/package.py` | 4.1; release layout 7.2 |
| Asset manifest IDs (`assets.toml`, D32) | `pin_assetgen` | 4.6 |
| Lua `pinball` module, sandbox, budgets | `pin_script::lua` | 4.2 |
| Wasm `pinball` imports, guest exports, sandbox | `pin_script::wasm` | 4.5 |
| Guest SDK | `crates/pin_script/guest-sdk` (`pin_guest_sdk`) | 4.5; used by 6.3 |
| `config.toml` (application settings) | `pin_app::ui::config` | 4.4; live in the app KP-004 (#84) |
| Autoplay soak report and the Gate A1 verdict | `pin_sim::autoplay` | 2.9; Gate A1 (PR #28) |
| DOF bridge input (`[dof]` map, telemetry replay) | `tools/dof_bridge` | 3.5 |
| Latency trace (`--latency-trace`) | `pin_app` | KP-002 (#82) |
| Toolchain and dependency pins | `versions.toml`, root `Cargo.toml` | 0.1; root pins 1.1 |
| CI workflow and the `pr-checks` aggregator | `.github/workflows/ci.yml`, `tests/spec_ci` | 1.2; `pr-checks` PR #37 (D40) |
| Release workflow and package layout | `.github/workflows/release.yml`, `scripts/package.py` | 7.2 |
| Rule ports and their workflow | `tables/*/rules-wasm`, `.github/workflows/rules-wasm.yml` | 6.3 |
| Soak workflow | `.github/workflows/soak.yml` | 7.1 |
| Final acceptance generator | `scripts/human_acceptance.py` | 7.2; new layout KP-044 (#125) |
| Gate A2 runner and verdict | `scripts/gate_a2.sh`, `scripts/gate_a2_verdict.py` | Gate A2 runner (PR #61) |
| Gate A3 runner | `scripts/gate_a3.sh` (to come) | KP-043 (#124) |
| Plan traceability check | `scripts/req_coverage.py` | process migration; CI job KP-045 (#126) |

## Cache and fixture paths

- `target/` is git-ignored: per-issue build dirs `target/<issue number>` (and `_ver`), table
  bundles `target/<table>`, gate output `target/gate_a1`, `target/gate_a2`, soak output
  `target/soak`.
- Also git-ignored: `*.pinbundle`, `/dist/`, `config.toml.local`, coverage output (`*.profraw`,
  `*.profdata`, `/coverage/`), .NET build output under `tools/dof_bridge`, `__pycache__/`.
- Committed binaries: `tables/*/rules.wasm` (rebuilt and compared by `rules-wasm.yml`), the
  replay-hash fixture's `playfield.glb`, and test fixtures; CodeRabbit's `path_filters` keep
  them out of review.
