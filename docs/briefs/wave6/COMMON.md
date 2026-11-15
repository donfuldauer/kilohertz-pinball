RULES (PLAN §2.3, binding)
 1. Crate and module boundaries. An agent edits only its target crate and module directory. Where two subphases share a crate in one wave, each owns a module directory and the earlier subphase pre-stubs the `mod` declarations. (Here all `mod` lines already exist. Edit only OWNED PATHS; anything else you need is a REQUEST in your report.)
 2. Contract-change protocol (D4). Only Subphase 1.1 writes `pin_core` freely. Any later agent that needs a contract change appends a request to `contracts/CHANGE_REQUESTS.md` and submits it as a separate `pin_core`-only PR. Additive changes merge immediately (PLAN wording; like every PR an additive change still passes the §2.7 merge conditions: CI green, review threads resolved); breaking changes merge only between waves.
 3. GPU-free core tests. `pin_core`, `pin_physics`, `pin_input`, `pin_script`, `pin_assets`, `pin_tablegen` and `pin_sim` pass `cargo test` with no display server and no GPU. Render tests live behind the `gpu-tests` feature and run in their own CI job (D20).
 4. No blocking on the sim thread (D5). No heap allocation, mutex, or file I/O in `step_1ms()` in Rust engine code. Tests wrap the tick in a panicking-allocator guard (e.g. `assert_no_alloc`). Two explicit exemptions: the Lua VM allocates from its own capped, pre-reserved arena; and a hot-reload swap tick may allocate and overrun (developer-only event).
 5. Simulated time in tests (D8). Headless tests never sleep and never assert on wall-clock time.
 6. No test-shaped special cases. Solver code must not branch on named behaviours ("live catch", "powerball"). Tests assert emergent results across parameter sweeps.
 7. Agent roles (D34, D35). Every unit of work is classified by the work-classifier (or, for small items, the triage skill, D38), done by a task-executor (Implementer, Independent Tester or Fixer), checked by the work-verifier, and diagnosed by the issue-investigator when something fails for a non-obvious reason. The model is whatever the classifier picked.
 8. Never wait on a person (D28). When the brief is ambiguous, choose the option most consistent with the decision log, record it in `docs/DECISIONS.md` (a `## <id> — <topic>` section with Decision / Rationale / Alternatives), add one line to `docs/HUMAN_QUEUE.md`, and continue. Append only; never rewrite other entries.
 9. No self-review (D36, D37). No agent reviews code for style or design, its own or another agent's. The work-verifier checks objective facts only; code review happens on the pull request by CodeRabbit. The Orchestrator resolves review threads, not you.

DETERMINISM (D9): same binary + same machine => identical replay. No fast-math, no FMA contraction, `libm` for transcendental functions, fixed iteration order (no HashMap iteration), one seeded PCG. UNITS (D17): SI internally; TOML uses mm / g / deg, converted at load.

ENVIRONMENT
 * 4 cores, 15 GB RAM shared by up to 8 agents. EVERY cargo command is prefixed with:
     CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/<TARGET DIR of your brief>
   Check `df -h /home/user` before big builds; under 4 GB free, `cargo clean -p <crate>` for crates you no longer need and say so in your report. Build only the crates you need (`-p <crate>`); never `cargo build --workspace` unless told.
 * Toolchain: rustc 1.97.0, clippy, rustfmt, cargo-llvm-cov, lavapipe (`mesa-vulkan-drivers`), Xvfb, SDL3 headers. Do not install other toolchains. New dependencies: avoid; if unavoidable, exact pin (`=x.y.z`, from the root Cargo.toml or versions.toml where it already exists) under a `# Wave 6 additions (<id>)` comment in the crate's Cargo.toml (workspace pins in root Cargo.toml likewise); `versions.toml` stays unchanged.
 * Stale-build hazard: when a result contradicts another agent's report, `touch` the changed sources and rebuild before calling it a defect.
 * GPU tests: `--features gpu-tests` on pin_render/pin_app, on lavapipe: `WGPU_BACKEND=vulkan`; under Xvfb for SDL windows: `xvfb-run -a cargo test -p pin_render --features gpu-tests`. See `.github/workflows/ci.yml` job `gpu-tests` for the exact CI env. Known: lavapipe can SIGSEGV when many tests each create their own device; share one `&'static GpuContext` per test binary (see tests/displays_gpu.rs for the pattern).
 * You work in your own git worktree. Do NOT commit, push, stash, reset or check out; the Orchestrator does git. Report the list of changed files.

GATE / DONE WHEN (the work-verifier re-runs these on the head commit)
 1. `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` clean on every touched crate.
 2. Implementer tests and every VERIFICATION CRITERION pass; every REQUIREMENT traces to a passing test.
 3. Independent Tester's `tests/spec_*` pass. A disagreement between spec tests and implementation is resolved against the brief, never by deleting or weakening a test.
 4. Line coverage (`cargo llvm-cov`): >= 85% for pin_core, pin_physics, pin_input, pin_script, pin_assets, pin_tablegen, pin_sim; >= 60% for pin_render, pin_audio, pin_app (your owned modules and the crate).
 5. Property tests (proptest) cover every invariant and numeric helper; every parser you add survives a bounded random-bytes run without panicking.
 6. Anything on the sim thread: allocation guard passes and the replay hash is stable over 10 runs.
 7. Path check: the diff touches only OWNED PATHS (plus the explicitly allowed additive hooks).
 8. Render work: analytic pixel assertions first (region mean colour, luminance ratios, symmetry, energy), a regression snapshot only after those pass, never as sole evidence (D30). The verifier also walks the VISUAL CHECKLIST.

WHEN SOMETHING IS UNCLEAR
 Choose the option most consistent with the decision log (PLAN D1–D38, `docs/DECISIONS.md`), record it in docs/DECISIONS.md, add a line to docs/HUMAN_QUEUE.md, and continue. Do not wait. Relevant rows: D4 contract changes via CHANGE_REQUESTS only; D5 no alloc/lock/IO on sim thread; D8 simulated time; D9 determinism; D17 SI units; D19 secondary surfaces / presenter threads; D20 GPU tests separate; D24 tables validated by autoplay and gates A1/A2; D28 never wait on a person; D30 image-test policy; D32 placeholder asset kit with stable IDs; D33 ray-query ships OFF by default unless a runner validated it; D36/D37 no self-review, Orchestrator resolves threads.

ROLE-SPECIFIC
 Implementer: deliver code + unit tests (own `src/` modules and `tests/<your prefix>_*.rs`); do not read `tests/spec_*` of your subphase. Public rustdoc for every public item.
 Independent Tester: you have NOT seen the implementation and must not read it (no reading the subphase's owned `src/` paths, no `git diff` of them, no reading the Implementer's tests). You may read public APIs of already-merged dependency crates and this brief. Write black-box and property tests under `tests/spec_<id>_*.rs` from the brief alone. Where the brief fixes a name or signature, code against it; if the Implementer's API differs when you first compile, report the mismatch (do not read the implementation): the Orchestrator rules.

REPORT FORMAT (your final message)
 status: done | blocked
 gate results: <per step, with numbers: tests run/passed, coverage %, timings>
 files changed: <list>
 decisions logged: <DECISIONS.md section titles>
 contract-change requests: <CR ids or none>
 requests outside owned paths: <list or none>
 findings: <defects found (Tester), anything the verifier should know>
 for any CodeRabbit-related follow-up the Orchestrator resolves threads (D37).
