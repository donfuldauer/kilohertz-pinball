# Human Queue

Items for a person to look at after the run (PLAN.md §2.3 rule 8, §2.6 failure policy).

- [Orchestrator] Branch topology (`wave/N`, `sp/<id>`, `wave-N-green` tags) replaced by `[sp X.Y]`-prefixed commits on the session branch; see DECISIONS.md.
- [0.1] Pinned crates are newer than the plan expected (sdl3 0.22.1, parry3d 0.31.1, mlua 0.12.2, wgpu 30.0.1, wasmtime 50.0.1). parry3d now uses glam (`glam =0.33.12` via glamx), not nalgebra; confirm the engine math layer should standardise on glam 0.33. See DECISIONS.md "0.1 — Dependency spike results".
- ~~[0.1] wasmtime 50 raises the toolchain floor to Rust 1.97.0. CI uses `@stable` per the brief; decide whether CI should pin the exact toolchain from versions.toml instead.~~ Resolved: CI pinned to 1.97.0, see DECISIONS.md "Orchestrator — 1.2 review B1".
- ~~[0.1] Windows (WARP) and macOS (Metal) results for Q1–Q6 were not run locally; check the first `spikes` workflow run and fill in the CI-pending entries in DECISIONS.md.~~ Resolved: CI run 43140608468, see DECISIONS.md "0.1 — CI results".
- [0.1 review, non-blocking] Small spike hygiene items left as-is: egui-wgpu smoke test skips silently without an adapter even under SPIKE_REQUIRE_GPU=1 (spikes/dep_smoke/tests/smoke.rs:72); sdl_timestamp sets only the video hint; spec_0_1 uses caret deps (lockfile committed). Workflow items (run spec_0_1 in CI, timeout, concurrency) are handed to Subphase 1.2.
- [1.1] Appendix A interpretation (on-disk flipper keys: mm without suffix, SI-suffixed torque/inertia keys; optional sections; DOF map entry shape) and the `physics_defaults.toml` format were chosen by the agent; see DECISIONS.md "1.1 — On-disk schema interpretation" and "1.1 — physics_defaults.toml format".
- [1.1] `BallProfile` has an extra `material` field and materials are a closed enum (steel, ceramic, plastic, resin, rubber, wood, acrylic); confirm this set, and that `uv_globall` is "plastic" for pair lookup.
- [1.1 review] Follow-ups handed to later subphases: 4.1 must reject switch/lamp numbers > 255 (N4) and check that `score_display.mode` has its matching `dmd`/`lcd` table (N5); 2.4 needs an early additive CR for kind-specific mech parameters (N7). See DECISIONS.md "1.1 — Review fix round".
- [Orchestrator] Wave 2 interpretive choices (dual-stage flipper semantics, PiP aspect ratios, Lua ID tables): see DECISIONS.md "Orchestrator — Wave 2 brief decisions".
- [3.1] Debounce semantics chosen by the agent (tick-sampled raw state; window-end settling opens a new window); see DECISIONS.md "3.1 — Input ingestion & flipper staging".
- [3.1 review, non-blocking] pin_input: add `const _: () = assert!(CabinetInput::COUNT <= 8)` (events/mod.rs:45) and document "call advance once per tick with non-decreasing ticks" (events/mod.rs:141). Subphase 2.5 must clear the router `out` buffer before every `update` (staging/mod.rs:170).
- [1.3] `pin_app` gained `src/lib.rs` (module declarations moved there from `main.rs`) so SDL integration tests can use `pin_app::platform`; confirm this layout for later pin_app subphases. See DECISIONS.md "1.3 — SDL3 windows, display topology and SPSC rings".
- [1.3] Default gamepad mapping and the drop-and-count policy for full input/window rings were chosen by the agent; confirm or adjust in 3.2.
