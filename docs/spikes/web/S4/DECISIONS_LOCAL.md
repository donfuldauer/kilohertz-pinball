# S4 (web-spike-rules) — local decisions (PLAN §2.3 rule 8; not repo files)

## S4-1 — wasmi version
**Decision:** use wasmi **2.0.2** (crates.io latest stable on 2026-11-20), features `std`, `validate`, `auto-dispatch`, `deterministic`, default features off.
**Rationale:** the brief asks for "the latest stable 0.4x/1.x that builds on wasm32-unknown-unknown"; the latest stable line is now 2.x and it builds for wasm32-unknown-unknown (`logs/build_wasm.log`). Picking an older line would test an engine nobody would ship. `auto-dispatch` is required on wasm32 (see RESULTS F-A2); `deterministic` gives wasmtime-identical NaN canonicalisation.
**Alternatives:** wasmi 1.x / 0.4x (older, same API family, not measured); tinywasm / wasm-interp (not built: time-boxed out, RESULTS §2 note).

## S4-2 — memory cap on the JIT path
**Decision:** cap linear memory by rewriting the module's own memory maximum to `min(declared, cap)` (and table maxima to `min(declared, 65 536)`), not by importing a host `WebAssembly.Memory({maximum})`.
**Rationale:** identical spec semantics (`memory.grow` past the maximum returns −1, as wasmtime's `StoreLimits` with `trap_on_grow_failure(false)`), no import plumbing, same code for wasmi and the JIT; the declared-initial check (`MemoryLimit`) stays the load-time check pin_script makes. Measured identical in the adversarial corpus (`memory_grow_beyond_cap`, `memory_grow_runtime_delta`, `huge_declared_max`, `table_grow`).
**Alternatives:** convert the defined memory into an import of a host-created `WebAssembly.Memory` (works, more glue, no gain).

## S4-3 — the call-depth counter's weight and limit
**Decision:** weighted counter, weight per function = `16 × (params + locals + max operand stack) + 64` (estimated bytes), limit 256 KiB; same instrumentation on every engine (and, recommended, natively).
**Rationale:** a plain call count cannot bound real stack use (frames with 1 000 live locals are ~8 KiB in Cranelift and V8). Measured: the weighted counter trips before wasmtime's own 256 KiB limit and before V8's stack limit for every probed frame shape (0–1 000 live locals), deterministically on all engines (RESULTS §5.3). Production modules peak at 4 calls / 1 872 estimated bytes (0.7 % of the limit).
**Alternatives:** call count only (V8 still overflows first for big frames: measured non-deterministic); raising wasmtime's `max_wasm_stack` (still frame-layout dependent).

## S4-4 — start function on the instrumented path
**Decision:** the instrumentation removes the start section and exports the start function; the host calls it right after instantiation with fuel = budget − 1.
**Rationale:** a failed start then still leaves the instrumentation's `why` global readable (budget vs depth vs trap), which wasmi's `instantiate_and_start` and `new WebAssembly.Instance` cannot give; `budget − 1` reproduces wasmtime's start-up `call` charge (pin_script's own test `start_budget(&start, 1000) == 997`; corpus cases `start_loops`, `start_ok_id_of` EXACT).
**Alternatives:** keep the start section (classification of a fuel-exhausting start would be lost on wasmi/V8).

## S4-5 — Chromium trap handler in this VM
**Decision:** Chromium runs of guest code that may go out of bounds (JIT adversarial corpus, worst-case, stack probes) use `--disable-features=WebAssemblyTrapHandler`.
**Rationale:** in this VM the Playwright Chromium renderer crashes on *any* wasm out-of-bounds memory access when the signal-based trap handler is on, with no engine code involved (`web/oob.html`; Node traps normally). With explicit bounds checks every case traps correctly. Treated as an environment artefact; real-device behaviour needs S5/web.
**Alternatives:** skip OOB cases in Chromium (loses evidence).

## S4-6 — wasm-rules bundles for the parity runs
**Decision:** parity runs use S1's bundle folders with `script_entry` rewritten to `rules.wasm` (the committed module, byte-identical), zipped deterministically (`bundles/<table>_wasm.pinbundle`), and S1's recorded scenarios (`.scn`, open-loop inputs).
**Rationale:** the same rewrite the repo's own wasm parity tests do (`crates/pin_app/tests/wasm_ports_support/mod.rs:76`); the table hash changes but `state_hash` does not include it, so the runs are also compared with S1's native **Lua** hashes (equal on every tick).
**Alternatives:** generate new recordings with Wasm rules (redundant: the scenarios are open-loop).

## S4-7 — hybrid allow-list key
**Decision:** the hybrid (design C) selects the JIT for a module whose SHA-256 is one of the three committed `rules.wasm` digests, wasmi otherwise.
**Rationale:** content hash, not a name or path, so a user bundle cannot claim to be a production table. (The spike matches the first 16 hex digits; a product must match all 64.)
**Alternatives:** signature on bundles (out of scope locally).
