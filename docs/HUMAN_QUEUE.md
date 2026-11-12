# Human Queue

Items for a person to look at after the run (PLAN.md §2.3 rule 8, §2.6 failure policy).

- [Orchestrator] Branch topology (`wave/N`, `sp/<id>`, `wave-N-green` tags) replaced by `[sp X.Y]`-prefixed commits on the session branch; see DECISIONS.md.
- [0.1] Pinned crates are newer than the plan expected (sdl3 0.22.1, parry3d 0.31.1, mlua 0.12.2, wgpu 30.0.1, wasmtime 50.0.1). parry3d now uses glam (`glam =0.33.12` via glamx), not nalgebra; confirm the engine math layer should standardise on glam 0.33. See DECISIONS.md "0.1 — Dependency spike results".
- [0.1] wasmtime 50 raises the toolchain floor to Rust 1.97.0. CI uses `@stable` per the brief; decide whether CI should pin the exact toolchain from versions.toml instead.
- [0.1] Windows (WARP) and macOS (Metal) results for Q1–Q6 were not run locally; check the first `spikes` workflow run and fill in the CI-pending entries in DECISIONS.md.
