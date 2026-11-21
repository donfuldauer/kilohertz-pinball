# S9 (web-spike-integration) — local decisions (PLAN §2.3 rule 8)

## S9-1 — Which rules a table runs in the web engine
* **Decision:** the engine follows the bundle manifest's `script_entry` (`.lua` → the real Lua
  backend, `.wasm` → wasmi + S4's instrumentation). The "both rules" runs use S1's bundle
  (`rules.lua`) and S4's `_wasm` bundle (same 55–56 files, `script_entry = "rules.wasm"`), served
  as files.
* **Rationale:** identical to the native load path (`pin_sim::load` picks the backend from the entry);
  the bundles are byte-for-byte the ones the native references were recorded from.
* **Alternatives:** an engine-side override (`rules=wasm` reads `rules.wasm` from a Lua bundle) —
  rejected: a behaviour native does not have.

## S9-2 — Replay pacing while rendering in a software-GPU browser
* **Decision:** replays render every frame with a fixed number of ticks per rendered frame (200 for
  60 s scenarios, 1000 for the 10-minute soak, 250 for the backend screenshots); live play uses
  S1's real-time catch-up loop (1 ms ticks, ≤ 50 ticks per frame, real time beyond the cap skipped).
* **Rationale:** SwiftShader renders 3–12 frames/s here; real-time replays would take the full
  scenario time × (60 / fps) and the brief asks for every tick checked while rendering, not for
  real-time speed. Per-tick parity does not depend on how ticks are grouped into frames.
* **Alternatives:** real-time pacing for replays (hours of wall time, or skipped frames).

## S9-3 — Post-processing for the long parity runs
* **Decision:** parity runs render at 360×640 with `post=min` (no MSAA, bloom, motion blur); the
  backend screenshots, the egui overlay check and the load-time runs use the full post chain at
  540×960.
* **Rationale:** the same renderer, pipelines and frame path; the full chain costs ~0.5–1 s per frame
  on SwiftShader, which would make the 7 runs × 300–600 frames take hours. The renderer cannot
  influence sim state (it only reads the snapshot).
* **Alternatives:** full post everywhere (no added evidence, 10× time).

## S9-4 — Link-time optimisation
* **Decision:** ship `opt-level = 3`, `codegen-units = 1`, **no LTO**, then `wasm-opt -Oz`; declare
  `lto = false` explicitly in the web profile.
* **Rationale:** cargo silently drops `-C lto` for a crate built as `[cdylib, rlib]` (C7; S3's
  `web_probe` had the same crate types, so S3's "lto=fat" sizes were not LTO either); a real fat-LTO
  build was larger after `-Oz` (9 391 508 vs 8 181 742 B) and equal in speed within noise.
* **Alternatives:** thin engine cdylib crate with `lto = "fat"` (bigger), `opt-level = "z"` (S3:
  larger after `-Oz` at opt 3 for this graph; not re-measured).

## S9-5 — The egui stack in the measured module
* **Decision:** the module links pin_render's egui overlay (`OverlayPainter` + `overlay_style`) and
  draws it on request (`?overlay=1`).
* **Rationale:** the product needs the reduced egui menu (proposal 9.7); without a use the linker
  removes the whole egui text stack (≈1.8 MiB of code) and the size numbers would understate the
  product.
* **Alternatives:** measure without egui and add S3's figure (less exact: S3 used opt-level z).

## S9-6 — Frame pacing in the page
* **Decision:** a submitted frame sets an in-flight flag cleared by `queue.on_submitted_work_done`;
  while set, replays wait for the next animation frame and live play steps the sim without drawing.
* **Rationale:** never queue frames behind a slow GPU (phones, SwiftShader); the browser already
  throttles rAF to the compositor, this keeps the main thread responsive.
* **Alternatives:** draw on every rAF regardless (work piles up on slow GPUs).

## S9-7 — No COOP/COEP
* **Decision:** the local server sends S4's CSP and no `Cross-Origin-*` headers.
* **Rationale:** nothing in the combined design needs cross-origin isolation (no threads, no
  SharedArrayBuffer, sim on the main thread per S1); `crossOriginIsolated === false` and every test
  passes. Only consequence: `performance.now()` is 100 µs-coarse (S1), irrelevant to the fixed-step sim.
* **Alternatives:** send them (allowed locally) — would only buy 5 µs timers for profiling.

## S9-8 — Tick-count guard on the JS→wasm boundary
* **Decision:** `WebEngine::step(n)` refuses `n > 10 000`; the page clamps its frame debt at ≥ 0.
* **Rationale:** a negative JS count reached the `u32` parameter as 2^32−k (wasm-bindgen's `>>> 0`)
  and froze the page in `step` (found here: rAF's timestamp can precede `performance.now()` taken
  when the loop starts).
* **Alternatives:** take `i32`/`f64` and validate (equivalent; recommended for the product API).

## S9-9 — Expected classification of S2's "memory_tick" injection
* **Decision:** the page test expects "script instruction budget exceeded" for S2's memory-bomb
  injection (100 × 65 000-byte strings in one callback), not `MemoryLimit`.
* **Rationale:** KHZ 20 charges string building to the instruction budget, which runs out before the
  2 MiB cap; S2's own sandbox case `mem_big_strings_fast` shows `BudgetExceeded` natively and on wasm,
  and every tick of the injected run equals S2's native run.
* **Alternatives:** none (the native outcome is the reference).

## S9-10 — PiP placement on the single canvas
* **Decision:** `pin_web::web::page_routes` places the backglass and score-display PiPs by aspect
  (portrait: score band at 9 % from the top + small backglass under it; landscape: side columns).
* **Rationale:** the brief requires a usable portrait and landscape layout; S3's probe layout (score
  band at the bottom) covered the flippers on phone aspects.
* **Alternatives:** render the playfield into a sub-viewport with the PiPs outside it (cleaner, a
  pin_render change: left to 9.7).
