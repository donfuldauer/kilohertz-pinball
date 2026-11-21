# S3 (web-spike-render) local decisions

Recorded under PLAN §2.3 rule 8 (choose the option most consistent with the decision log,
record it, continue). Scratch only; nothing here changes the repository.

## s3-1 — How the "real scene" reaches the browser

**Decision:** The browser loads each production table from the same zipped bundle the
native build produces (`tables/<t>/build.sh` → zip), through the real `pin_assets`
(`Vfs::from_zip_bytes`, `load_table`, `load_gltf`) inside the wasm, and draws it with the
real `pin_render` (`PbrRenderer`, `PostChain`, display renderers; shaders and pipelines
unchanged). The only per-frame data dumped from a native run is the `FrameStateSnapshot`
subset the renderer reads (balls, flippers, lamps, GI, shown scenes, scores), as JSON.
`pin_app`'s `TableVisuals::of` / `TableRenderer` glue (SDL-free code that lives in an SDL
crate) is ported line for line into the probe crate; a native run checks that the port
and the app produce byte-identical frames on lavapipe.

**Rationale:** The brief allows a static dump; loading the bundle in the browser
exercises more of the real web path (zip, glTF, image decode) and makes the size numbers
representative, at no extra cost because `pin_render` already depends on `pin_assets`.

**Alternatives:** Serialise the meshes/display images natively (less of the web path
tested); compile `pin_app` for wasm (impossible: SDL3, `pin_sim` → `pin_script` →
mlua/wasmtime).

## s3-2 — `fragile-send-sync-non-atomic-wasm` in the spike build

**Decision:** The spike builds `pin_render` for wasm32 with wgpu's
`fragile-send-sync-non-atomic-wasm` feature on (exposed as the scratch `pin_render`
feature `fragile-send-sync`). Without it the only failing site is the presenter-thread
spawn (`crates/pin_render/src/gpu/surfaces.rs:1410` → `spawn_guarded` `:112`), checked
separately with a cfg-gated stub (see RESULTS.md §A1).

**Rationale:** egui-wgpu 0.37's default features already turn this feature on for every
wasm build of today's workspace, so it is the current behaviour; it is sound on a
single-threaded (no `+atomics`) wasm module, which is the proposal's design.

**Alternatives:** cfg-gate the presenter thread on wasm32 (validated as the 9.2 fix and
recommended, because `std::thread::spawn` cannot work there anyway).

## s3-3 — Frame size and comparison tolerance

**Decision:** Reference frames are 540×960 RGBA8 (`Rgba8Unorm`, post chain applies the
sRGB curve), the production post defaults (MSAA 4×, bloom, motion blur, ACES). Pixel
differences are reported with thresholds 2/8/16/32 (max channel difference per pixel),
for the whole frame and for the playfield only. Nothing is asserted (D8: performance
and cross-GPU numbers are reported, not gated).

**Rationale:** Portrait table at half of a 1080p panel; the same frame size on every
backend; D30's analytic tests (not goldens) are the gate style for GPU output.

**Alternatives:** Full 1080×1920 (several seconds per frame on SwiftShader).

## s3-4 — Chromium flag set and Vulkan ICD for the harness

**Decision:** All browser runs use `--enable-unsafe-webgpu --enable-features=Vulkan
--use-angle=vulkan --ignore-gpu-blocklist` and restrict Chromium's Vulkan loader to its bundled
SwiftShader ICD (`VK_ICD_FILENAMES`/`VK_DRIVER_FILES`). Exception: the backend-detection run
uses no flags (that is the configuration under test).

**Rationale:** Found by elimination (RESULTS §3.1): it is the smallest set with which WebGPU both
renders and presents and WebGL2 works in the same browser. Pinning the ICD makes WebGPU and
WebGL2 run on the same software device whether or not the machine has Mesa (as a CI runner may
not), so results do not depend on the host's packages.

**Alternatives:** The brief's suggested list (WebGPU presents fail: device destroyed); separate
browser launches per backend (works, but doubles CI launch cost); system lavapipe for ANGLE
(machine dependent).

## s3-5 — WebGL2 prototype inside the scratch `pin_render`

**Decision:** The worst WebGL2 blocker (fragment storage buffers) plus the limit request and the
1-layer array texture were prototyped directly in the scratch `pin_render` behind a run-time
capability check (`max_storage_buffers_per_shader_stage < 2`), so native behaviour is
unchanged; results before the prototype are kept in `results/before_webgl2_proto/`.

**Rationale:** The brief asks to validate the WebGL2 estimate with a prototype of the worst
blocker; a capability switch keeps one binary and lets the native gpu-tests prove native is
unaffected (106 + 7 passed).

**Alternatives:** A cfg(feature) switch (two native builds to test); cluster lists in textures
(one shader for both backends, larger change; kept as the option for many-light scenes).

## s3-6 — wasm-opt binary

**Decision:** `wasm-opt -Oz` from binaryen 132 via npm (`binaryen@132.0.0`, JS/wasm build) for
every size number. Running the native binaryen 132 release binary was refused by the
environment's permission policy, so it was removed unused.

**Rationale:** The brief allows npm or `cargo install wasm-opt`; the npm package is binaryen 132
exactly. Its cost is speed only (about 7–10 minutes per module under load), which affects CI
advice (use native binaryen there), not the sizes.

**Alternatives:** `cargo install wasm-opt` (bundles an older binaryen, version mismatch).

## s3-7 — Bench under load

**Decision:** The pass-cost bench was run once per backend while other spikes were building
(load average 6–11) and is reported as shares with a ±10-point noise caveat instead of being
repeated.

**Rationale:** The brief asks only for relative pass costs; SwiftShader timings do not predict
phone GPUs anyway; the dominant share (bloom ≈ 50 %) was consistent across sizes and backends.

**Alternatives:** Waiting for an idle machine (not controllable from this spike).
