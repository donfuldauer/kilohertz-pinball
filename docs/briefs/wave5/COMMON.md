# Wave 5 — common brief sections

Every Wave 5 brief includes this file. Read it in full before your subphase brief. Wave 5 works in one
shared working tree at `/home/user/kilohertz-pinball` (branch `ccr-f24f4063-bgqdfk` stands in for
`wave/5`); the Orchestrator commits. **Agents do not commit, push, stash, reset or check out.**

## RULES (PLAN §2.3, binding)

1. **Crate and module boundaries.** Edit only your OWNED PATHS. Where two subphases share a crate in one
   wave, each owns a module directory; the `mod` lines are already stubbed. Anything outside your owned
   paths that you need changed: write it down in your report as a request, do not edit it.
2. **Contract-change protocol (D4).** `pin_core` is changed only through `contracts/CHANGE_REQUESTS.md`:
   append a numbered CR (next free number), keep the change additive, keep it in `pin_core` files only,
   and name it in your report. Breaking changes are refused during a wave.
3. **GPU-free core tests.** `pin_core`, `pin_physics`, `pin_input`, `pin_script`, `pin_assets`,
   `pin_tablegen` and `pin_sim` pass `cargo test` with no display server and no GPU. Render tests live
   behind the `gpu-tests` feature and run in their own CI job (D20).
4. **No blocking on the sim thread (D5).** No heap allocation, mutex, or file I/O in `step_1ms()` in Rust
   engine code. Tests wrap the tick in a panicking-allocator guard. Exemptions: the Lua VM allocates from
   its own capped, pre-reserved arena; a hot-reload swap tick may allocate and overrun.
5. **Simulated time in tests (D8).** Headless tests never sleep and never assert on wall-clock time.
6. **No test-shaped special cases.** Solver code must not branch on named behaviours. Tests assert
   emergent results across parameter sweeps.
7. **Three roles per subphase (D26, D27).** Implementer, Independent Tester, Reviewer. Tester and
   Reviewer use the model that did not implement.
8. **Never wait on a person (D28).** When the brief is ambiguous, choose the option most consistent with
   the decision log, record it in `docs/DECISIONS.md` (a `## <id> — <topic>` section with Decision /
   Rationale / Alternatives), add one line to `docs/HUMAN_QUEUE.md`, and continue.

Determinism (D9): same binary + same machine ⇒ identical replay. No fast-math, no FMA contraction,
`libm` for transcendental functions, fixed iteration order (no `HashMap` iteration), one seeded PCG.
Units (D17): SI internally; TOML uses mm / g / deg, converted at load.

## ENVIRONMENT

* 4 cores, 15 GB RAM, shared by up to four subphases. **Every cargo command:** `CARGO_BUILD_JOBS=2`
  and `CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/sp_<X_Y>` (your subphase's dir, shared by its
  Implementer and Tester; Reviewers use `target/sp_<X_Y>_rev`).
  **Disk (added after a restart filled the allowance):** also export `CARGO_INCREMENTAL=0`,
  `CARGO_PROFILE_DEV_DEBUG=line-tables-only` and `CARGO_PROFILE_TEST_DEBUG=line-tables-only` for every
  cargo command. Check `df -h /home/user` before big builds; if free space is under 4 GB, run
  `cargo clean -p <crate>` for crates you no longer need and tell the Orchestrator in your report. Build only the crates you need
  (`-p <crate>`); never `cargo build --workspace` unless the brief says so.
* Toolchain: rustc 1.97.0 (CI pins it), clippy, rustfmt, cargo-llvm-cov + llvm-tools, lavapipe
  (`mesa-vulkan-drivers`), Xvfb, SDL3 build headers, .NET 8 SDK (`dotnet`). Do not install other
  toolchains. Workspace dependency pins live in the root `Cargo.toml`; you may add exact pins under a
  `# Wave 5 additions` comment, one line per crate naming your subphase. `versions.toml` stays unchanged.
* Stale-build hazard: when two agents share a target dir and a result contradicts the other's report,
  `touch` the changed sources and rebuild before calling it a defect.
* GPU tests: `--features gpu-tests` on the render crates, run on lavapipe (`WGPU_BACKEND=vulkan`, mesa
  lavapipe ICD). See `.github/workflows/ci.yml` job `gpu-tests` for the exact env the CI uses.

## GATE (PLAN §2.6 "Subphase gate", all must hold)

1. `cargo fmt --check` and `cargo clippy --all-targets -D warnings` (on the touched crates) are clean.
2. Implementer tests and every listed verification criterion pass.
3. Independent Tester's spec tests pass. A disagreement between spec tests and implementation is
   resolved against the brief, not by deleting the test.
4. Line coverage (`cargo llvm-cov`): ≥ 85 % for `pin_core`, `pin_physics`, `pin_input`, `pin_script`,
   `pin_assets`, `pin_tablegen`, `pin_sim`; ≥ 60 % for `pin_render`, `pin_audio`, `pin_app` (measure
   your owned modules and the crate).
5. Property tests (`proptest`) cover every solver invariant and numeric helper; every parser (TOML
   manifests, zip, glTF extras, UDP packets, replay files) survives a bounded random-bytes run without
   panicking.
6. For anything on the sim thread: allocation guard passes and a replay hash is stable over 10 runs.
7. An automated path check confirms the diff touches only owned paths
   (`git status --porcelain` / `git diff --stat` against the wave base, filtered to your paths).
8. Reviewer approves.

Image test policy (D30, render subphases): (a) analytic pixel assertions first (region mean colour,
luminance ratios, symmetry, energy); (b) a regression snapshot stored only after (a) passes, with
per-backend tolerance; (c) the Reviewer's visual checklist, logged. A snapshot is never the sole evidence.

## WHEN SOMETHING IS UNCLEAR

Choose the option most consistent with the decision log (PLAN §0 D1–D33, `docs/DECISIONS.md`), record it
in `docs/DECISIONS.md`, add a line to `docs/HUMAN_QUEUE.md`, and continue. Do not wait. Append to these
two files; never rewrite other subphases' entries.

## ROLE-SPECIFIC

* **Implementer:** deliver code + unit tests (in your owned `src/` modules and `tests/<your prefix>_*.rs`);
  do not read `tests/spec_*` of your subphase. Write public rustdoc for every public item.
* **Independent Tester:** you have NOT seen the implementation and must not read it (no reading your
  subphase's owned `src/` paths, no `git diff` of them). You may read the public API of *dependency*
  subphases (already merged) and the brief. Write black-box and property tests under
  `tests/spec_<id>_*.rs` (in the crate named by the brief) from the brief alone. Where the brief fixes a
  name or signature, code against it; if the Implementer's API differs when you first compile, report
  the mismatch rather than reading the implementation — the Orchestrator rules.
* **Reviewer:** read the diff, check RULES, run every gate step yourself, and for render work open the
  output images against the visual checklist. List findings as **blocking (B#)** or **non-blocking (N#)**
  with file:line and a repro. Do not fix product code.

## REPORT FORMAT (your final message)

```
status: pass | fail | blocked        (Implementer/Tester: done | blocked)
gate results: <per step, with numbers: tests run/passed, coverage %, timings>
files changed: <list>
decisions logged: <DECISIONS.md section titles>
contract-change requests: <CR ids>
requests outside owned paths: <list or none>
findings: <B#/N# list — Reviewer; defects found — Tester>
```
