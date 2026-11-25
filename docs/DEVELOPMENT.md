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
- Vendored and patched: `third_party/lua-src` (Lua 5.4.9 build crate), `third_party/lewton`
  (Ogg Vorbis) and `third_party/sdl3-sys` (SDL3 bindings; the patch lets SDL3 build from checkout
  paths with spaces), each with its patches in `KHZ_PATCHES.md`, wired through
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
  `CARGO_BUILD_JOBS=2` and `CARGO_TARGET_DIR=<repo-root>/target/<issue number>`
  (for example `/home/user/kilohertz-pinball/target/<issue number>` in Claude Code cloud sessions
  or `/home/antigravity/donfuldauer/kilohertz-pinball/target/<issue number>` in Antigravity remote
  VMs; the issue's dir is shared by its Implementer and Independent Tester, and the work-verifier
  uses `target/<issue number>_ver`).
  **Disk (added after a restart filled the allowance):** also export `CARGO_INCREMENTAL=0`,
  `CARGO_PROFILE_DEV_DEBUG=line-tables-only` and `CARGO_PROFILE_TEST_DEBUG=line-tables-only` for
  every cargo command. Check `df -h .` before big builds; if free space is under 4 GB, run
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

Commit attribution: `scripts/install-git-hooks.sh` sets `core.hooksPath=.githooks` and
`kilohertz.coauthor`; `.githooks/commit-msg` then adds `Co-authored-by:` for the human contributor
to agent-authored commits (`AGENTS.md` "Commit attribution"). Claude Code runs the script at
`SessionStart`; `.agents/setup-env.sh` runs it on Antigravity VMs.

Each issue works in its own git worktree `../kilohertz-issue-<N>` (`work-issue` skill §2; or a
branch inside the checkout when a sandbox restricts sibling directories); the absolute
`CARGO_TARGET_DIR` keeps every build under one `target/` for disk checks and cleanup.

## Workspace and crate map

```text
kilohertz-pinball/
├── Cargo.toml                     # Workspace, exact pins, [patch.crates-io] for the vendored crates
├── versions.toml                  # Toolchain and dependency pins from the 0.1 spikes (unchanged)
├── physics_defaults.toml          # Global tunables (SPEC Appendix C); data-only tuning happens here
├── contracts/CHANGE_REQUESTS.md   # Contract-change queue (PROCESS.md R2)
├── AGENTS.md                      # Canonical agent instructions (Claude Code & Gemini/Antigravity)
├── CLAUDE.md                      # Claude Code entry point (imports AGENTS.md)
├── GEMINI.md                      # Gemini / Antigravity entry point (imports AGENTS.md)
├── .agents/
│   ├── agents/                    # Antigravity custom subagent definitions (forward to .claude/agents/)
│   └── skills/                    # Gemini / Antigravity skills (forward to .claude/skills/, plus pr-watch)
├── .claude/
│   ├── settings.json              # Shared Claude Code sandbox and permission defaults
│   ├── agents/                    # Canonical subagent prompts (work-classifier, task-executor, work-verifier, issue-investigator)
│   └── skills/                    # Canonical skill procedures (work-issue, run-plan, plan-from-spec, triage)
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
│   ├── lewton/         # Vendored Ogg Vorbis decoder with hardening patches (KHZ_PATCHES.md)
│   └── sdl3-sys/       # Vendored SDL3 bindings; builds SDL3 from checkout paths with spaces (KHZ_PATCHES.md)
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
fail=0
cargo build --release -p pin_sim --bin pin_sim || { echo "build failed"; fail=1; }
bin="${CARGO_TARGET_DIR:-target}/release/pin_sim"
rec="${CARGO_TARGET_DIR:-target}/replay_fixture.khzrec"
h=$("$bin" crates/pin_sim/tests/fixtures/replay_table --ticks 10000 --record-inputs "$rec") || { echo "record run failed"; fail=1; }
[ -n "$h" ] || { echo "record run produced no state hash"; fail=1; }
if [ "$fail" = 0 ]; then
  for i in 1 2 3 4 5 6 7 8 9 10; do
    out=$("$bin" crates/pin_sim/tests/fixtures/replay_table --replay-inputs "$rec") || { echo "replay $i failed"; fail=1; continue; }
    [ "$out" = "$h" ] || { echo "replay $i differs"; fail=1; }
  done
fi
[ "$fail" = 0 ]   # nonzero status if the build, the record run or any replay failed or differed

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
sessions on the agent VM and cannot resolve the 10 % limit (DECISIONS.md "wave6-gate-benchmark — tick benchmark baseline and method (2026-11-16)").

`scripts/req_coverage.py` (standard library plus the `gh` CLI): reads every
`REQ-<AREA>-<NNN>` ID in `docs/SPEC.md` (lines marked `WITHDRAWN` excepted) and checks each is
named by a `work-unit` issue body (`gh api -X GET search/issues`, explicit paging) or, with
`--plan-file`, by a local draft. `--milestone` takes a tracking issue (`79` or `#79`) or its title;
`--allow` lists IDs deferred by a recorded decision; `--tests [roots]` also requires each ID in
test code. Exit 0 all traced, 1 a missing or unknown ID, 2 a usage, input or GitHub error. It has
no CI job yet (KP-045, #126; DECISIONS.md "Process — req_coverage without a CI job for now").

## CI

Workflows live in `.github/workflows/`. The single required status check on `main` is
**`pr-checks`** (PROCESS.md P6 routine step 3); changing that set is a repository setting (P8).

**D40 split.** Pull requests run the Linux set only: `fmt`, `clippy` (Linux), `core-tests`
(Linux), `replay-hash` (Linux), and `gpu-tests` (lavapipe) and `dof-bridge` only when their paths
change. Pushes to `main` and `workflow_dispatch` run everything: the Windows and macOS legs (the
`*-cross` jobs), all GPU adapters and coverage. A red `main` stops merges until a fix or revert
lands (P6 routine step 3). Pull-request runs are cancelled when superseded; `main` and dispatch runs queue
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
| `windowed.yml` | PRs touching `crates/pin_app`, `crates/pin_render`, `crates/pin_input`, `tables/` or the workflow; every push to `main`; dispatch | The windowed latency harness (KP-002): the release `pin_app` with the grey-box table for 60 s under Xvfb on lavapipe (`LP_NUM_THREADS=2`), flipper presses through XTest, checks a to d on its latency trace (see "Windowed latency harness"). Artifact `windowed-latency`. Not in `pr-checks`: it becomes required only when the owner makes it a required check (P8 setting). |
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
  workflow on `main` and on demand, the SPEC Appendix F.4 screenshot set as an artifact. Its
  latency part reuses the windowed harness (`windowed.yml`, `scripts/windowed/run.sh` and the
  `--latency-trace` file; see "Windowed latency harness").
- **Windowed latency harness** (KP-002, SPEC §7.5): `scripts/windowed/run.sh [<out_dir>]`, run
  by `windowed.yml`; see the next section.

## Windowed latency harness

What it measures (SPEC §7.5, R12): a player's flipper press in the real windowed app. The release
`pin_app` runs the grey-box table (`--table target/greybox --latency-trace <file> --run-ms
60000`) under Xvfb with lavapipe (`WGPU_BACKEND=vulkan`, `PIN_FORCE_FALLBACK_ADAPTER=1` so wgpu
takes the software adapter or fails, `SDL_VIDEODRIVER=x11`); the driver starts its own Xvfb with
`-displayfd` (Xvfb picks a free display and reports it once it accepts connections), waits for the
app's `pin_app: latency trace: first frame presented` line, focuses the window and injects flipper
presses for the rest of the 60 s through XTest (`xdotool keydown/keyup` of X keycodes 50 and 62,
Shift_L and Shift_R; the keysym `Shift_R` would also press Shift_L).

**Press schedule** (DECISIONS.md "#82 — Windowed press schedule"): seeded and deterministic
(`KP_WINDOWED_SEED`), all times in microseconds so never aligned to ticks. Each key presses on its
own timeline, the right one phase-shifted, so left and right alternate and overlap: every hold is
at least three frame intervals (`KP_WINDOWED_FRAME_MS`, default 125 ms, i.e. 375 ms; capped at
1 s) plus 0 to 170 ms of jitter, every gap 100 to 250 ms. Check b counts only frames that showed
the bat raised while the key was held, and lavapipe draws about 8 frames per second, so a shorter
hold could end before any frame reads a snapshot with the bat up. Over a 60 s run this gives
about 150 presses (at least 100 are needed). After 10 presses the plan waits for both keys to be
up and holds the left key alone for 800 ms with a screenshot, after 20 the right key, after 30 a
rest screenshot; one more rest screenshot comes first. A step that runs late (a slow capture)
shifts every later step by its delay, so holds keep their planned length.

**Driver hygiene.** Every helper process (`xdotool`, `xwd`, `convert`) runs through one bounded
runner (polled `try_wait`, killed and reaped at a 5 s deadline; at the end of the presses a
key whose key-up failed stays marked held and is retried within a 400 ms budget, and one still
held is reported); the screenshot pipes `nice -n 10
xwd -root -silent -display <d>` into `nice -n 10 convert xwd:- png:<path>` with
`MAGICK_THREAD_LIMIT=1`, no shell. A run first removes the previous run's `latency_trace.csv`,
`pin_app_stderr.log`, `summary.txt` and `frame_*.png` from the artifact directory. Every exit path
after the app started (window focus, key injection, first-frame timeout, a hang) kills the app if
it still runs, completes the stderr capture and writes `pin_app_stderr.log`; a failure also writes
`summary.txt` (`verdict: FAIL (windowed driver: <what>)` and the stderr tail). The run fails when
fewer than 3 screenshots were captured or none was taken with a flipper held. Code:
`crates/pin_app/tests/windowed/` (`verdict.rs`: the pure checks, unit-tested in plain `cargo test`;
`driver.rs`: Xvfb, the app, xdotool, screenshots, the press plan). DECISIONS.md "#82 — Windowed
latency check definitions" records the checks.

**The latency trace** (`pin_app --latency-trace <file>`, REQ-LAT-003): CSV, `\n` line ends,
integers only; an event table (one row per delivered button event: `event_seq, input, down,
sdl_ts_ns, apply_tick, apply_tick_end_sdl_ns, prev_tick_end_sdl_ns, drain_sdl_ns,
prev_drain_sdl_ns, dropped_ticks_at_apply, stall, move_tick, frame, frame_snap_tick, present_ns,
frames_to_photon`), an empty line, the frame table (`frame, snap_tick, present_ns`, one row per
presented playfield frame; a backglass-only present is not a frame) and a two-line trailer:
`# overflows: input=N move=N frame=N pending=N` (each counter exactly once) and `# run_end_ns=N`
(the SDL clock read on the main thread when the run loop ended, before the threads were joined).
Every column is documented in the rustdoc of `crates/pin_app/src/orchestrator/trace.rs`; the names
are stable (Gate A3 and KP-043 read them). All times are on SDL's clock (`SDL_GetTicksNS`).
`move_tick` is the first tick at whose end a bat bound to the pressed input is raised at least
1 degree (0.01745 rad) from its angle at the applying tick's start, in its energised direction
(the sign of the flipper's sweep); a bat still returning from an earlier release never sets it,
and a press's watch ends at its release (a raise at the release tick still counts; a later press's
raise never does).
`frame` is the first presented frame, in present order, whose snapshot tick lies between
`move_tick` and the `apply_tick` of the next release of the same input and whose snapshot shows the
bat displaced at least 1 degree in the energised direction from the press's applying-tick base (a
bat that crossed 1 degree and fell back is matched to a later frame that shows it raised, or to
none); `frames_to_photon = frame - F0` counts from the press timestamp (`F0` the last frame
presented at or before it; a press before the first presented frame counts from frame index 0,
so the count is `frame + 1`), and `move_tick` only selects the earliest candidate frame. For a press
with `stall = 1`, `prev_drain_sdl_ns` and `drain_sdl_ns` are the stall's start and end (so their
difference is the stall). On the table presenter path `present_ns` is the SDL clock read
immediately after the playfield `queue.present` returned, before a suboptimal surface is
reconfigured and before the backglass and score surfaces are drawn. The windowed harness runs a
table, so it uses that path; `--debug-overlay` only draws the debug lines over the PBR image
(`TableRenderOptions::debug_overlay`) and does not change the presenter. The debug presenter
(`pin_render::SurfaceSet`) is used only when no table is loaded or the table presenter cannot be
used (the playfield offers no 8-bit format); it lives in `pin_render`, so there `present_ns` is read
after the whole render call, late by that frame's remaining work and, on a suboptimal surface, one
reconfigure. The trace is written to a sibling temp
file and renamed over the destination at shutdown; the destination is never truncated, so a
hard-linked bundle is safe (the `--table` bundle file itself is refused before anything is created;
a value starting with `--` is not a path; a path whose metadata fails other than "not found", a
missing path ending in a separator and an existing FIFO are refused before anything is opened). On
Unix the temp file is created owner-only (`0600`) and, before the rename, given the existing
destination's permission bits, so the rename never widens access (a new destination stays
`0600`). It is written on the main thread after every
sim, render and audio thread was joined, also when the run failed (run cap not met, a thread join
error), so a failed run's trace can be diagnosed; a run that ends before that point (a startup
error after the trace file was prepared, a panic or an early return) removes the temp file and
leaves the destination untouched, and a failed commit is reported and makes the run fail; no thread that runs a tick or draws a frame touches the
filesystem (same class as the startup table load). Records go into rings preallocated before the
threads start, and `pin_app: latency trace: <path>, <n> events, <m> frames, <k> overflows` follows
the shutdown lines. Without the flag nothing is allocated or recorded.

**Checks** (presses = down events of the four flipper inputs; `TICK` = 1 ms):

- **a. Key to coil (REQ-LAT-004, REQ-LAT-005).** (1) `apply_tick_end_sdl_ns >= sdl_ts_ns`; (2)
  lateness `apply_tick_end_sdl_ns - sdl_ts_ns <= TICK + 2 ms` (the 2 ms tolerance covers the main
  thread's forwarding delay and the sim thread's wake-up), plus, for a press with `stall = 1`, the
  stall `drain_sdl_ns - prev_drain_sdl_ns` and no more; (3) `move_tick - apply_tick <= 8`
  ticks (the press takes effect in the applying tick). A press whose bat never moved fails, unless
  its applying tick ended within `8 + 2` ticks of `run_end_ns`. `stall = 1` when the press was
  stamped at or after the start and before the end of a stall the sim thread observed (a tick that started more than 2 ms
  after the previous executed tick, or after dropped ticks) and was applied in that tick or a later
  tick of the same catch-up batch: the sim thread's own scheduling and the timestamp, never the
  lateness being checked. That the applying tick is the first executed tick that received the
  press is verified by the deterministic SimLoop tests, not by this run (DECISIONS.md); the sim
  pops every ring event stamped at or before the tick end up to the per-tick input buffer of 256
  events (`INPUT_BUFFER_CAPACITY`), and eligible events beyond 256 are applied in a later executed
  tick (the next one, or later ones when more than 256 remain; the app input ring holds 1,024).
- **b. Key to photon in frames (REQ-LAT-001, CI form).** For each press except those stamped
  within 500 ms of `run_end_ns` (excluded and counted): the raw count `n_raw = frame - F0`, `F0` the
  last presented frame at or before `sdl_ts_ns` (`n_raw = frame + 1` for a press before the first
  presented frame: the count starts at frame index 0), must be `<= 2`, with one allowance: when the frame
  in flight at the press (the first presented after it) presented by `sdl_ts_ns` + check a's bound
  + `8` ticks, the count may start after it (`n_adj = frame - in_flight <= 2`; never for a
  press that failed check a). The summary prints the raw and adjusted max and p99 and how many
  presses were "in-flight shifted". A press with no move, or no frame that showed the bat raised
  while held, fails. The trace's `frame` is chosen by the app (the drawn angles are not in the
  file); the verdict checks it exists, matches the frame table and is a candidate. Counted in frames, never milliseconds (lavapipe draws about 8 frames per
  second); key-to-photon p50/p99 in ms is printed as information.
- **c. Real time (REQ-LAT-002, CI form).** From `pin_app: N sim ticks (D dropped), ...`:
  `D / (N + D) <= 0.001`, and `N` within 1 % of 60 000. The simulated-to-wall ratio `N / (N + D)`
  is printed. On lavapipe the app now starts lean (REQ-LAT-006, #83: MSAA, bloom and motion blur
  off, one warning line on stderr), and the worker-thread cap also applies. The full Low preset
  (render scale, probe captures) arrives with KP-035.
- **d. Completeness.** No trace overflow, at least 100 presses, as many presses traced as the
  driver injected, and `input dropped 0` on the shutdown line (a dropped input event is a missing
  trace row; a shutdown line without the field is malformed).

**The lavapipe cap.** lavapipe's worker threads are capped with `LP_NUM_THREADS=2` (the workflow
env and `run.sh`'s default), so on a 4-vCPU GitHub runner the sim thread keeps a core next to the
render, main and audio threads and lavapipe's two workers. Measured on the 4-core agent VM, one
60 s run each (dropped ticks of about 60 000 / presented frames / check a): 1 thread 30
(0.050 %) / 277 / pass; 2 threads 8 (0.013 %) / 526 / pass; 3 threads 20 (0.033 %) / 734 / fail
(one press 4 ms late: the main thread starved). To measure: `LP_NUM_THREADS=<n>
scripts/windowed/run.sh target/windowed_lp<n>` for each `n`, and read the `dropped ratio` row of
`summary.txt`; choose the value whose ratio stays well under 0.1 % (DECISIONS.md "#82 —
LP_NUM_THREADS cap").

**Running it locally** (from the repository root, with the agent environment):

```sh
# Prerequisites: mesa-vulkan-drivers libvulkan1 xvfb xauth xdotool x11-apps imagemagick
scripts/windowed/run.sh                     # builds the bundle and the release app, runs 60 s
scripts/windowed/run.sh --print-paths       # prints the resolved paths, builds nothing
# or, with the bundle and the release app already built:
LP_NUM_THREADS=2 PIN_FORCE_FALLBACK_ADAPTER=1 \
  WGPU_BACKEND=vulkan cargo test --release -p pin_app --test windowed -- --ignored --nocapture
cargo test -p pin_app --test windowed       # the verdict, parser, property and allocation tests
python3 -I -m unittest discover -s scripts/windowed/tests -p '*_test.py'   # workflow structure
```

The bundle defaults to `<target>/greybox` after `tables/greybox/build.sh <target>/greybox`;
set `KP_GREYBOX_BUNDLE` only to use another bundle. The driver starts its own Xvfb (1280x720x24, `-displayfd`) when `DISPLAY` is unset. Environment:
`KP_GREYBOX_BUNDLE` (default `<target>/greybox`), `KP_WINDOWED_OUT` (default
`<target>/windowed`), `KP_WINDOWED_RUN_MS` (default 60000), `KP_WINDOWED_SEED`,
`KP_WINDOWED_FRAME_MS` (default 125), `KP_PIN_APP`, `LP_NUM_THREADS` (default 2). `run.sh` takes a
relative `CARGO_TARGET_DIR` or `<out_dir>` relative to the repository root and exports the
absolute `CARGO_TARGET_DIR` before Cargo runs.

**Artifacts** (`windowed.yml` uploads them as `windowed-latency`, also on failure):
`latency_trace.csv` (the trace), `summary.txt` (verdicts and summary table), `pin_app_stderr.log`
(the app's stderr, with the adapter line and the shutdown line), `frame_0_rest.png`,
`frame_1_left_held.png`, `frame_2_right_held.png`, `frame_3_rest.png`. Every file is from the
current run: older ones are removed when a run starts, and a failed run still writes
`pin_app_stderr.log` and a failure `summary.txt`.

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
