# S3 — web-spike-render: wgpu on the web (WebGPU and WebGL2)

Spike for the web-proposal (`.../web-proposal/docs/decisions/2026-11-20-web-proposal.md`,
§4.1, A1/A3/A4/A5/A22/A23/A24, Q3). Everything ran in this scratch directory against a copy
of `main` at `995935f` (`repo/`); the real tree was not modified (build output only under
`/home/user/kilohertz-pinball/target/web_s3`).

`S` below = `${WEB_SPIKES}/S3`.

## 0. Verdicts at a glance

| # | Assumption (proposal §11) | Verdict | Evidence (section of this file) |
|---|---|---|---|
| A1 | wgpu 30.0.1 supports wasm32-unknown-unknown with a WebGPU backend (`webgpu`; in the default set?) and a WebGL2 backend (`webgl`); egui-wgpu 0.37 works on the web | **Confirmed, with two corrections.** `webgpu` *is* in wgpu 30's default features; egui-wgpu 0.37's *default* features also switch on `wgpu/webgl`, `wgpu/default` and `wgpu/fragile-send-sync-non-atomic-wasm`, so today's workspace already compiles **both** web backends into any wasm build. Both backends coexist in one binary and are chosen at run time, but `Instance::new` picks WebGPU whenever `navigator.gpu` exists, even with no adapter: only `wgpu::util::new_instance_with_webgpu_detection` falls back to WebGL2. egui-wgpu 0.37 painted the real overlay theme on WebGPU (and on WebGL2) | §1, §3.2, §7 |
| A3 | WebGPU allows read-only storage buffers in fragment shaders and fits `downlevel_defaults`; WebGL2 has no storage buffers or compute; WebGPU has no ray queries | **Confirmed.** WebGPU (Chromium 157, SwiftShader): 0 limit violations for the engine's request, `max_storage_buffers_per_shader_stage = 10`, PBR pipelines with fragment storage buffers build and draw with 0 errors. WebGL2: storage buffers 0, storage textures 0, compute 0 (10 limit violations; the engine's device request fails). Ray queries: `enable wgpu_ray_query;` is rejected by the browser's WGSL compiler (Tint) and by naga on WebGL2; no ray-query feature is offered | §4, §5 |
| A4 | WebGPU `rgba16float` renderable, multisampled, blendable; canvas formats `bgra8unorm`/`rgba8unorm`/`rgba16float`, sRGB only via view formats | **Confirmed.** `Rgba16Float`: RENDER_ATTACHMENT, MULTISAMPLE_X4, MULTISAMPLE_RESOLVE, BLENDABLE, FILTERABLE on WebGPU *and* WebGL2 (SwiftShader). Canvas formats offered: WebGPU `[Rgba8Unorm, Bgra8Unorm, Rgba16Float]` (preferred `rgba8unorm` on Linux), no `*Srgb`; an `Rgba8UnormSrgb` view of the canvas texture via `view_formats` renders without error. WebGL2 canvas: `[Rgba8UnormSrgb, Rgba8Unorm, Rgba16Float]` | §4 |
| A5 | Adapter, device, error-scope and buffer-map operations are async; no blocking poll on the main thread; browsers compile WGSL themselves | **Confirmed, and sharpened.** On WebGPU every one of them is a JS promise (`pop` future not ready on first poll; `device.poll` is a no-op returning `QueueEmpty`). The native pattern `pollster::block_on` on the browser main thread **never returns** (busy loop at 100 % CPU, 20 s timeout and a 639 s observation): `thread::park` is a no-op on wasm32 without atomics, and the promise can only resolve when the event loop runs. On WebGL2 the same futures are ready synchronously (wgpu-core), so code that "works on WebGL2" can still hang on WebGPU. Every WGSL module of `pin_render` was compiled by Chromium's Tint (`get_compilation_info`): all pass except the ray-query module | §3.3, §3.4 |
| A22 | The wasm-bindgen CLI must match the crate version exactly | **Confirmed.** CLI 0.2.129 on a module built with crate 0.2.130: exit 1, "rust Wasm file schema version: 0.2.130 / this binary schema version: 0.2.129 … must exactly match". CLI 0.2.130: exit 0 | §2 |
| A23 | GitHub-hosted runners can run headless Chrome/Firefox with a software WebGPU adapter | **Confirmed locally, needs a CI trial.** Playwright 1.63's bundled Chromium 157 (headless shell) ships SwiftShader Vulkan; with `--enable-unsafe-webgpu --enable-features=Vulkan --use-angle=vulkan --ignore-gpu-blocklist` it gives a WebGPU adapter that renders *and presents* and a WebGL2 context, with no GPU, no X server and no system Vulkan driver (verified with the system ICDs hidden). The flags were found by elimination: `--enable-unsafe-webgpu` alone gives an adapter, but presenting a canvas frame destroys the device. Firefox not tried (not installed). Exact workflow in §3.6 | §3.1, §3.6 |
| A24 | Engine `.wasm` 5–15 MiB raw, a few MiB brotli | **Confirmed for the render path; full-engine number pending S1.** The web render path (pin_render + pin_assets loaders + egui + wgpu) is 9.6–17.0 MiB as cargo emits it (incl. a 1.0–4.6 MiB name section and 1.7 MiB of DWARF), **5.3–8.0 MiB after `wasm-opt -Oz`**, **1.46–1.95 MiB brotli** (WebGPU-only 1.46–1.50, WebGL2 1.93, both 1.95). Not included: pin_sim, the physics solver, the rules engine, pin_audio (S1) | §6 |

**Effort answer for Q3 (S6 combines it with S5's reach data):** **WebGL2 is not "huge effort".** Every WebGL2 failure was found from real runs (§5.1: 4 blockers on the app path, all in `pin_render`), the worst one (fragment storage buffers in `pbr_forward`) was prototyped in ~90 lines, and with the prototype all three production tables render on WebGL2 with MSAA 4×, bloom, motion blur and both PiPs, ≤ 4 pixels per frame different from WebGPU (§5.2). Cost: **3 T2 agent-runs one-time** (~7.5 % of Phase 9), ~+10 s per browser CI run plus a GPU-free GLSL ES 3.00 guard in `core-tests`, browser test matrix ×2, low maintenance (automated guard), **+0.47 MiB brotli only for WebGL2 visitors** when shipped as a second artifact. Recommendation on effort alone: **WebGPU primary + WebGL2 fallback as a separate artifact**; S6 weighs it against S5's reach data.


## 1. Building `pin_render` for wasm32 (A1)

### 1.1 wgpu 30.0.1 feature semantics (read from the registry sources)

- `wgpu` default features: `std, parking_lot, dx12, metal, gles, vulkan, wgsl, webgpu`
  (`~/.cargo/registry/src/*/wgpu-30.0.1/Cargo.toml`). So **`webgpu` is a default feature**.
  `webgl` is not.
- `build.rs` cfg aliases: `webgpu = all(not(native), not(Emscripten), feature = "webgpu")`,
  `webgl = all(not(native), not(Emscripten), feature = "webgl")`,
  `wgpu_core = any(native, webgl, dx12, metal, vulkan, gles, noop)`. On
  wasm32-unknown-unknown `gles`/`vulkan`/`dx12`/`metal` are inert (target_os `unknown`), so a
  WebGPU-only build has **no wgpu-core, no wgpu-hal and no naga** (the browser compiles the
  WGSL); `webgl` pulls in wgpu-core + wgpu-hal(gles) + naga (WGSL → GLSL ES 3.0 inside the wasm).
- egui-wgpu 0.37.0 default features: `fragile-send-sync-non-atomic-wasm`,
  `macos-window-resize-jitter-fix` (= `wgpu/metal`), **`wgpu/default`, `wgpu/webgl`**. The
  workspace pins `egui-wgpu = "=0.37.0"` with default features (`Cargo.toml:52`), so
  `cargo tree -p pin_render --target wasm32-unknown-unknown -e features -i wgpu-core` shows
  `wgpu feature "webgl"` ← `egui-wgpu feature "default"`: today every wasm build of the
  workspace contains both backends and naga, whether wanted or not.
- `Instance::new(desc)` with both backends compiled in (wgpu-30.0.1 `src/api/instance.rs:63-99`):
  if `desc.backends` contains `BROWSER_WEBGPU` and `navigator.gpu` *exists*, the instance is a
  WebGPU instance; otherwise it is a wgpu-core (GL) instance. One instance is never both.
  `navigator.gpu` exists in Chromium even when no adapter is available, so a "both" build
  that uses `Instance::new` fails there instead of falling back (measured, §3.2).
  `wgpu::util::new_instance_with_webgpu_detection` (async) first requests a WebGPU adapter and
  only then decides.

### 1.2 Feature sets used, and what compiles

Scratch manifests (diffs in `S/patches/`): workspace `wgpu = { version = "=30.0.1",
default-features = false, features = ["std", "wgsl"] }`, `egui-wgpu = { version = "=0.37.0",
default-features = false }`; `pin_render` adds `[target.'cfg(not(target_arch = "wasm32"))'
.dependencies] wgpu = { workspace = true, features = ["dx12", "metal", "vulkan", "gles",
"parking_lot"] }` (= the former native default set) and the features `webgpu = ["wgpu/webgpu"]`,
`webgl = ["wgpu/webgl"]`, `fragile-send-sync = ["wgpu/fragile-send-sync-non-atomic-wasm"]`.
Resolver 3 does not unify target-specific features across targets, so the native feature set is
unchanged (native lavapipe tests, §3.5). No `--cfg=web_sys_unstable_apis` and no `RUSTFLAGS` are
needed: wgpu 30 ships its own WebGPU bindings (`webgpu_sys`). `getrandom 0.4.3` (via `zip`)
needs no flag either: `zip 9.5.1` enables `getrandom/wasm_js` itself on wasm32
(`zip-9.5.1/Cargo.toml`, `[target.'cfg(target_arch = "wasm32")'.dependencies.getrandom]`).

| Build (wasm32-unknown-unknown) | Result | Command / log |
|---|---|---|
| `pin_render`, **unchanged repo manifests** (wgpu defaults + egui-wgpu defaults) | compiles (both backends + fragile-send-sync come in through egui-wgpu) | `cargo check -p pin_render --target wasm32-unknown-unknown` → `logs/check-pin_render-default.log` |
| `pin_render --features webgpu` (egui-wgpu without defaults, no fragile-send-sync) | **3 errors**, all at one site: the presenter thread `std::thread::Builder::spawn` needs `Send` (`gpu/surfaces.rs:1410` → `spawn_guarded` `:112`; `Rc<DropGuard>`, `dyn WindowHandle`, `Rc<Cell<u32>>` not `Send`) | `logs/check-pin_render-webgpu-nosendsync.log` |
| `pin_render --features webgl` (same) | **46 errors**, all at the same two lines (wgpu-core `dyn Dyn*` types not `Send`/`Sync`) | `logs/check-pin_render-webgl-nosendsync.log` |
| same three, with a 15-line wasm32 stub of `spawn_presenter` (returns `GpuError::Surface`; presenter threads cannot exist on the web) | **compiles** for all three (`webgpu`, `webgl`, `webgpu,webgl`; exit 0, only dead-code warnings for the native-only blit/presenter code) | `logs/check-pin_render-*-nosendsync-with-stub.log` |
| `web_probe` (`pin_render` + `pin_assets` + `egui`) `--features webgpu,webgl,fragile-send-sync` | compiles; 7 dead-code warnings in `pin_render` (blit / presenter code unused on wasm32) | `build_wasm.sh both fast` → `logs/build-both-fast.log` |
| `webgpu,fragile-send-sync` / `webgl,fragile-send-sync` (release, LTO) | compile | §6, `logs/size-*.log` |

Source changes `pin_render` needed for the web (scratch only; `S/patches/pin_render.diff`):
1. `GpuContext::request_async` / `from_parts` (adapter and device without `pollster`).
2. `pop_scope`: error-scope pops in `PostChain::new`, `OctahedralProbes::new`,
   surface configure and the ray-query section went through `pollster::block_on`; on wasm32 the
   helper polls once (wgpu-core: ready) and otherwise defers the pop to the event loop
   (`wasm_bindgen_futures::spawn_local`), reporting late errors through a log.
3. The presenter-thread stub above (only if `fragile-send-sync` is not used).
Nothing else in `pin_render` had to change to render the production frame on WebGPU.
`Instant`, `recv_timeout` and `device.poll(Wait)` remain in the readback helpers
(`gpu/mod.rs:355`, `finish_map`, `render_to_image`); they compile but cannot work on the web
(`poll` returns at once on WebGPU, and `Instant::now` panics), so the web readback is written
async in the probe (`web.rs: map_read`).

## 2. wasm-bindgen pinning (A22)

- `Cargo.lock`: `wasm-bindgen 0.2.130`, `js-sys/web-sys 0.3.107`, `wasm-bindgen-futures 0.4.80`.
  The probe pins `wasm-bindgen = "=0.2.130"` (and the others exactly).
- CLI: `cargo install --locked --root $S/tools/wbg-0.2.130 wasm-bindgen-cli --version =0.2.130`
  (log `logs/install-wbg130.log`, about 6 min with 2 jobs), and 0.2.129 the same way.
- Matched (`logs/a22-match.txt`): `wasm-bindgen --target web --out-name web_probe --out-dir
  wasm/a22/cli130 wasm/web_probe-both-fast.wasm` → exit 0.
- Mismatched (`logs/a22-mismatch.txt`), CLI 0.2.129 on the same file → exit 1:

```
error: failed to read wasm-bindgen data from '.../wasm/web_probe-both-fast.wasm'
Caused by:
    it looks like the Rust project used to create this Wasm file was linked against
    version of wasm-bindgen that uses a different bindgen format than this binary:
      rust Wasm file schema version: 0.2.130
         this binary schema version: 0.2.129
    Currently the bindgen format is unstable enough that these two schema versions
    must exactly match. ...
```

Recommendation: CI reads the version from `Cargo.lock` and installs exactly that CLI
(`--locked`); the web crate pins `wasm-bindgen = "=X"` in `[target.'cfg(target_arch =
"wasm32")'.dependencies]` so a `cargo update` cannot drift from the CLI.


## 3. Browser harness and runs

### 3.1 Harness, environment, flags

- Probe crate `repo/crates/web_probe` (scratch): `scene.rs` ports `pin_app`'s
  `TableVisuals::of` to read the bundle from a `Vfs` (so the browser gets the **zip bytes** of
  the real bundle and runs the real `pin_assets` zip/glTF/image loaders), `renderer.rs` ports
  `pin_app`'s SDL-free `TableRenderer` (PBR → post chain → backglass + score PiPs; no HUD, no
  debug overlay, both off by default in the app). All GPU work goes through the real
  `pin_render` types: shaders and pipelines are the repository's, unchanged. `web.rs` holds
  the `wasm-bindgen` entry points (`probe_adapter`, `render_table`, `pipeline_matrix`,
  `scopes_and_device_lost`, `block_on_main_thread`, `bench_table`).
- Frame input: `s3_dump` (native, scratch crate) builds each bundle through the engine's own
  load path (`validate_table` + `load_checked`), spawns one `standard_steel` ball, runs 250
  ticks and writes the snapshot subset the renderer reads (`dumps/<table>.frame.json`: ball,
  3 flippers, lamps, GI, scene 0 = attract, score). It renders the same frame with
  **`pin_app`'s own `TableRenderer`** and with the probe's port on visuals loaded from the zip:
  `app_vs_probe_differing_bytes = 0` for all three tables on lavapipe
  (`dumps/native_summary.json`), so the port is exact.
- Page: `web/index.html` + `web/main.js` (ES module, `wasm-bindgen --target web` output in
  `web/pkg/<variant>/`), served by `node/server.js` (127.0.0.1, sets COOP/COEP as the planned
  local dev server would; `crossOriginIsolated = true`). Driver: `node/run_probe.js`
  (Playwright). Readback: frame rendered to the canvas texture (or an offscreen copy when the
  surface lacks COPY_SRC, which is the case on both web backends: surface usages are
  `RENDER_ATTACHMENT` only) → `copy_texture_to_buffer` → `map_async` awaited as a promise →
  PNG (`node/png.js`). Cross-check: Playwright's own screenshot of the *presented* canvas equals
  the async-mapped readback exactly (max abs 0, `results/compare_readback_vs_presented_canvas.json`).
- Environment: Linux 6.18, 4 cores, no GPU; Playwright 1.63.0, **Chromium 157.0.8103.78**
  (`chrome-headless-shell`, Playwright's default for `chromium.launch()`); rustc 1.97.0;
  mesa-vulkan-drivers 25.2.8 (lavapipe) installed for the native reference. The driver hides
  the system Vulkan ICDs from Chromium (`VK_ICD_FILENAMES`/`VK_DRIVER_FILES` = Chromium's
  bundled `vk_swiftshader_icd.json`), as on a runner without Mesa; otherwise ANGLE picks
  lavapipe for WebGL2 (measured: `results/flags_probe_after_mesa.txt`). WebGPU (Dawn) uses
  SwiftShader in both cases (`adapter.info.architecture = "swiftshader"`, `isFallbackAdapter`).

Flag sets tried (`node/flags_probe.js`, `node/debug_render.js`; results
`results/flags_probe_after_mesa.txt` and the debug runs quoted here):

| Flags | WebGPU adapter | WebGPU frame presented to a canvas | WebGL2 |
|---|---|---|---|
| none | no (`requestAdapter()` → null; `navigator.gpu` exists) | — | yes (ANGLE, SwiftShader) |
| `--enable-unsafe-webgpu` | yes (SwiftShader, fallback) | **no**: ~6 ms after the first present the device is lost ("Destroyed: Device was destroyed."); offscreen rendering + readback works | yes |
| brief's list (`--enable-unsafe-webgpu --enable-features=Vulkan --use-angle=swiftshader --use-webgpu-adapter=swiftshader --enable-unsafe-swiftshader --ignore-gpu-blocklist --disable-vulkan-surface`) | yes | **no** (same device loss) | yes |
| `--enable-unsafe-webgpu --use-angle=vulkan` or `--enable-unsafe-webgpu --enable-features=Vulkan` | yes | no | — |
| `--enable-unsafe-webgpu --enable-features=Vulkan --use-angle=vulkan` | yes | **yes** | **no** (`getContext('webgl2')` → null) |
| **`--enable-unsafe-webgpu --enable-features=Vulkan --use-angle=vulkan --ignore-gpu-blocklist`** (used for everything below) | yes | **yes** | **yes** (ANGLE-on-Vulkan, SwiftShader) |

The elimination runs above were made with the system Vulkan ICDs visible (Mesa lavapipe
installed); the final set was then re-verified with only Chromium's SwiftShader ICD visible
(WebGPU frame presented, WebGL2 frame rendered), which is the configuration of every result
below. Without `--ignore-gpu-blocklist`, ANGLE-on-Vulkan refused WebGL2 (observed with the system
ICDs visible; not re-tested with the SwiftShader-only ICD).

### 3.2 Backend choice in a build with both backends

`results/detect_both_none.json` (flags: none, so `navigator.gpu` exists but has no adapter):
`Instance::new(BROWSER_WEBGPU | GL)` → `request_adapter` fails ("webgpu found no adapters";
the message also says "gl support not compiled in", because the instance is a WebGPU
instance); `new_instance_with_webgpu_detection` → GL adapter, ANGLE/SwiftShader. With the
working flags both pick WebGPU (`results/detect_both_minimal.json`). A web build with a WebGL2
fallback must use the detection helper.

### 3.3 Frames of the three production tables

`run_render_probe.sh` (= `node run_probe.js render --pkg both --flags minimal --backends
webgpu,webgl2`), 540×960, MSAA 4×, bloom, motion blur, ACES, backglass and DMD PiPs:

- `[render/webgpu] apex_circuit:OK web_webgpu_both_minimal_apex_circuit.png | sky_deck:OK web_webgpu_both_minimal_sky_deck.png | feature_lab:OK web_webgpu_both_minimal_feature_lab.png (6627 ms)`
- `[render/webgl2] apex_circuit:OK web_webgl2_both_minimal_apex_circuit.png | sky_deck:OK web_webgl2_both_minimal_sky_deck.png | feature_lab:OK web_webgl2_both_minimal_feature_lab.png (7371 ms)`

(The WebGL2 line is with the §5.2 prototype. With unpatched `pin_render` every WebGL2 table
failed before drawing: `GPU device request failed: Limit 'max_compute_workgroups_per_dimension'
value 65535 is better than allowed 0`, `results/before_webgl2_proto/render_webgl2_both_minimal.json`.)
The `-Oz`-optimised WebGPU-only module (`web/pkg/webgpu-z-opt`) renders the same three frames
byte-identically (`results/compare_webgpu_fast_vs_zopt.txt`: max abs 0) and reports WebGL2 as
not compiled in.

Screenshots: `shots/web_webgpu_both_minimal_{apex_circuit,sky_deck,feature_lab}.png`; native
references `dumps/native_{apex_circuit,sky_deck,feature_lab}.png` (lavapipe, Vulkan, the
app's renderer). Per-pixel comparison (`scripts/compare.py`, max channel difference per pixel;
diff images ×8 in `shots/diff/`):

WebGPU (Chromium/SwiftShader) vs native (wgpu/Vulkan/lavapipe):

| Table | max abs | mean abs | px > 2 | px > 8 | px > 16 | px > 32 | % px > 8 | playfield-only max / mean / px > 8 |
|---|---|---|---|---|---|---|---|---|
| web_webgpu_both_minimal_apex_circuit.png | 145 | 0.0639 | 2763 | 1764 | 627 | 120 | 0.34 | 145 / 0.075 / 1764 |
| web_webgpu_both_minimal_sky_deck.png | 83 | 0.0729 | 3219 | 1978 | 668 | 94 | 0.382 | 83 / 0.0856 / 1978 |
| web_webgpu_both_minimal_feature_lab.png | 111 | 0.0472 | 1667 | 1586 | 457 | 54 | 0.306 | 111 / 0.0554 / 1586 |

The differences are confined to triangle edges (MSAA coverage/resolve on SwiftShader vs
lavapipe) and ±1–2 LSB banding in smooth gradients (see `shots/diff/*.png`); means are below
0.1/255. Visually the frames are the same. This is the same class of difference D30 already
handles natively between lavapipe/WARP/Metal with analytic (not golden) pixel tests.

### 3.4 Async behaviour (A5): error scopes, uncaptured errors, device loss, blocking

`results/scopes_webgpu_both_minimal.json` (WebGPU):
- `push_error_scope(Validation)` + invalid buffer (`MAP_READ | MAP_WRITE`) → `pop().await`
  returns the validation error ("Buffer usages (BufferUsage::(MapRead|MapWrite)) is invalid…")
  after 3.5 ms; the pop future is **not** ready on the first poll (`pop_future_ready_synchronously:
  false`): it is a JS promise.
- The same error without a scope reaches `on_uncaptured_error` asynchronously (not within 5
  `setTimeout(0)` turns; it was delivered before the end of the call).
- `device.destroy()` → `set_device_lost_callback` fires with `Destroyed: Device was destroyed.`
  after 6.2 ms.
- `device.poll(..)` on WebGPU returns `QueueEmpty` immediately (wgpu-30.0.1
  `backend/webgpu.rs:2698`): it never waits. Every wait in the harness is `poll(Poll)` + a
  `setTimeout(0)` yield until a callback fills a slot (`web.rs: wait_slot`), which works on both
  backends (wgpu-core runs map/submitted-work callbacks inside `poll`).
- `block_on_main_thread` (`pollster::block_on(instance.request_adapter(..))` on the main
  thread): WebGPU → **never returns** (20 s timeout in `run_probe.js blockon`; an earlier run
  showed the renderer at 98 % CPU for 639 s); WebGL2 → returns at once (wgpu-core futures are
  ready synchronously). Cause: `pollster 1.0.1` parks with `thread::park()`, a no-op on
  wasm32 without atomics, so it spins; the promise can only resolve on the event loop it blocks.
- WGSL in the browser: every `pin_render` module compiled by Chromium (`get_compilation_info`,
  `results/matrix_webgpu_both_minimal.json`): `pbr_brdf`, `pbr_forward`, `post_chain`,
  `probe_remap`, `display_quads`, `dmd`, `backbox_scene`, `backbox_composite`: no messages;
  `raytrace_trace`: error `:1:8 expected extension … enable wgpu_ray_query;` (expected; §4).

### 3.5 Native gpu-tests in the scratch copy (lavapipe, as `ci.yml` gpu-tests)

Scratch copy (with every web change and the WebGL2 prototype), `WGPU_BACKEND=vulkan PIN_REQUIRE_GPU=1 xvfb-run -a cargo test -p pin_render --features gpu-tests --test spec_5_1_gpu --test post_gpu --test spec_5_2_gpu --test spec_5_3_gpu --test displays_gpu --test gpu_pbr_render --test ui_overlay_gpu --test srgb_blend_gpu` → **106 passed, 0 failed** (adapter llvmpipe/lavapipe; includes the 5.1/5.2/5.3 golden and analytic tests), and `cargo test -p pin_app --features gpu-tests --test wiring_render` (the app's `TableRenderer`) → **7 passed** (`logs/native-gpu-tests-pin_render.log`, `logs/native-gpu-tests-pin_app.log`). The first attempt failed to compile because of a borrow error in the prototype (fixed, then re-run). The native reference frames (`dumps/native_*.png`) were rendered by the app's own `TableRenderer` on lavapipe.

### 3.6 CI (A23): what is confirmed and the workflow to trial

Confirmed here: headless Chromium from `npx playwright install chromium` (headless shell),
no GPU, no X server, no system Vulkan driver, the flag set above → WebGPU renders and
presents, WebGL2 works. Not confirmed: the GitHub-hosted image itself (needs a CI trial);
Firefox (not installed here; its WebGPU on Linux needs S5's data). Workflow to trial:
`S/ci/web-render-smoke.yml` (pins Playwright 1.63.0, binaryen 132, the CLI version read from
`Cargo.lock`; sets the same four flags; uploads the screenshots).


## 4. Adapter features and limits (A3, A4)

Dumps: `results/adapter_webgpu_both_minimal.json`, `results/adapter_webgl2_both_minimal.json`
(wgpu `adapter.get_info()/features()/limits()/get_downlevel_capabilities()`, surface
capabilities, `get_texture_format_features` for the engine's formats, and every limit of the
engine's request the adapter refuses, via `Limits::check_limits_with_fail_fn`); native lavapipe
for comparison: `dumps/native_adapter.json`; raw WebGL2 `getParameter` values from the mobile
run: `results/mobile_both_minimal.json`.

Inventory of what `pin_render` requests or uses (from the source at `995935f`):
pipelines: `pbr_forward` 6 per sample count × {1, 4} (`pbr/renderer.rs:401`), `post_chain` 6
(`post/chain.rs:405`), `dmd` 1 (`displays/dmd.rs:586`), `display_quads` 2 blend variants
(`displays/quad.rs:481`), debug lines 1 (`debug/renderer.rs:146`), blit 1 (presenter thread
only, `gpu/blit.rs:53`), backbox scene + composite 2 (`backbox/renderer.rs:291`, `:367`; not on
the app path), probe remap 1 (`reflections/probes.rs:271`; not on the app path), ray-query
trace compute 1 (`raytrace/source.rs:502`; only with `EXPERIMENTAL_RAY_QUERY`), plus egui-wgpu's
own. 14 bind group layouts. Device request (`gpu/mod.rs:430`): features = adapter ∩
{`TIMESTAMP_QUERY`, `TIMESTAMP_QUERY_INSIDE_ENCODERS`}; limits =
`Limits::downlevel_defaults().using_resolution(adapter.limits())`. No push constants
(`immediate_size: 0` everywhere), no anisotropy, no comparison samplers, no cube / 3D
textures, no sampled depth, no `textureLoad` on multisampled textures.

| Requested / used | Where | WebGPU (Chromium 157, SwiftShader) | WebGL2 (ANGLE, SwiftShader) |
|---|---|---|---|
| Engine limits `downlevel_defaults().using_resolution(..)` | `gpu/mod.rs:437` | **granted** (0 violations; device created) | **refused**: 10 violations — `max_storage_buffers_per_shader_stage` 4 > 0, `max_dynamic_storage_buffers_per_pipeline_layout` 4 > 0, `max_storage_textures_per_shader_stage` 4 > 0, `max_storage_buffer_binding_size` 134217728 > 0, 6 compute limits (> 0). Device request error: "Limit 'max_compute_workgroups_per_dimension' value 65535 is better than allowed 0". `downlevel_webgl2_defaults()` is granted |
| `TIMESTAMP_QUERY` (optional, profiler) | `gpu/mod.rs:393` | offered (with `--enable-unsafe-webgpu`; S5 to confirm stable-channel exposure) | not offered |
| `TIMESTAMP_QUERY_INSIDE_ENCODERS` (optional) | `gpu/mod.rs:393` | not offered (not a WebGPU feature) | not offered |
| `EXPERIMENTAL_RAY_QUERY` (5.4, optional) | `raytrace/mod.rs:120` | **absent**; `enable wgpu_ray_query;` rejected by Tint ("expected extension") | absent; naga: "the `wgpu_ray_query` extension is not supported" |
| Read-only storage buffers in the fragment stage (2) | `pbr/shaders/forward.wgsl:57-58` | yes (`max_storage_buffers_per_shader_stage` = 10) | **no** (0) |
| Compute shaders | `raytrace/source.rs:502` only | yes (`COMPUTE_SHADERS`) | **no** (downlevel flag missing; trivial compute pipeline → validation error) |
| Storage texture `rgba16float` write | `raytrace/shaders/trace.wgsl:94` only | yes (4/stage) | no (0) |
| `Rgba16Float` HDR target: render, blend, MSAA 4×, resolve, filter | `pbr/renderer.rs:13`, post chain | RENDER_ATTACHMENT, BLENDABLE, MULTISAMPLE_X4, MULTISAMPLE_RESOLVE, FILTERABLE | same flags (needs `EXT_color_buffer_float`, present on SwiftShader) |
| `Depth32Float` attachment, MSAA 4× | `pbr/renderer.rs:16` | yes (MULTISAMPLE_X4) | yes (X2, X4) |
| `Rgba8Unorm` / `Bgra8Unorm` (+`Srgb`) outputs, display textures | post, displays | all render/blend/MSAA/filter | all |
| Canvas formats | surface | `[Rgba8Unorm, Bgra8Unorm, Rgba16Float]`, preferred `rgba8unorm`; `Fifo` only; alpha `Opaque`; usages `RENDER_ATTACHMENT` | `[Rgba8UnormSrgb, Rgba8Unorm, Rgba16Float]`; `Fifo`; `Opaque`; `RENDER_ATTACHMENT` |
| sRGB through `view_formats` | `gpu/surfaces.rs:691-803` | **works**: canvas `Rgba8Unorm` + `view_formats: [Rgba8UnormSrgb]`, sRGB view rendered and presented, no error | **not supported**: `In Surface::configure Downlevel flags DownlevelFlags(SURFACE_VIEW_FORMATS) are required but not supported` (the canvas offers `Rgba8UnormSrgb` directly instead) |
| Bind groups (max used 3, PBR) | `pbr/renderer.rs` | 4 | 8 |
| Uniform binding size (max used 4096 B, lights; post 576 B; frame 320 B) | | 65536 | 65536 |
| Uniform buffers per stage (max used 3) / dynamic uniform (max used 1) | | 12 / 8 | 14 / 14 |
| Sampled textures / samplers per stage (max used 2 / 1) | | 16 / 16 | 16 / 16 |
| Vertex stride (24 B) | `pbr/renderer.rs:409` | 2048 | 255 (`downlevel_webgl2_defaults`) |
| Max 2D texture size (engine: adapter's via `using_resolution`) | | 8192 | 8192 (SwiftShader; raw `MAX_TEXTURE_SIZE` in §8) |
| `texture_2d_array` sampled (probe atlas; 1-layer placeholder) | `pbr/renderer.rs:494-500`, `pbr/shaders/reflection.wgsl:8` | yes | **only with ≥ 2 layers**: wgpu-hal GL infers the view dimension from the layer count; the 1-layer placeholder is a `D2` texture viewed as `D2Array` → `[ERROR wgpu_hal::gles] wgpu-hal heuristics assumed that the view dimension will be equal to D2 rather than D2Array` |
| Downlevel flags | | full set incl. `COMPUTE_SHADERS`, `FRAGMENT_STORAGE`, `VERTEX_STORAGE`, `INDEPENDENT_BLEND`, `ANISOTROPIC_FILTERING` | only `NON_POWER_OF_TWO_MIPMAPPED_TEXTURES, COMPARISON_SAMPLERS, ANISOTROPIC_FILTERING, MULTISAMPLED_SHADING, SHADER_F16_IN_F32, MSL2_1, TEXTURE_COMPRESSION` (no `COMPUTE_SHADERS`, no `FRAGMENT_STORAGE`, no `INDEPENDENT_BLEND`) |

`downlevel_defaults` vs `downlevel_webgl2_defaults`: the engine's request violates the WebGL2
defaults in exactly the 10 storage/compute limits listed above (`webgl2_defaults_failures` is
empty on the WebGL2 adapter; `downlevel_defaults_failures` lists the 10). WebGPU grants
`downlevel_defaults` with room to spare.


## 5. WebGL2 gap analysis (from real runs)

Method: `pipeline_matrix` (`web.rs`) builds every `pin_render` object that owns pipelines,
draws with it, and records the error scopes (validation/OOM/internal) and wgpu's log for each
item separately; every WGSL module is also compiled on its own. On WebGL2 the engine's own
device request fails first, so the matrix was run twice: with the engine's limits and with
`downlevel_webgl2_defaults` (to reach the pipelines). Results:
`results/before_webgl2_proto/matrix_webgl2_both_minimal{,_webgl2limits}.json` (unpatched),
`results/matrix_webgl2_both_minimal*.json` (after the prototype below);
WebGPU for comparison: `results/matrix_webgpu_both_minimal.json` (every item OK except the
ray-query module).

### 5.1 Every failure on WebGL2 (unpatched `pin_render`)

| # | Pipeline / object | Source | Error (verbatim, first line) | Classification | Fix and cost |
|---|---|---|---|---|---|
| W1 | Device creation (all of `pin_render`) | `gpu/mod.rs:437`, `:487` (`Limits::downlevel_defaults()`) | `GPU device request failed: Limit 'max_compute_workgroups_per_dimension' value 65535 is better than allowed 0` (10 storage/compute limits refused, §4) | workaround in Rust | Request `downlevel_webgl2_defaults()` on adapters with `max_storage_buffers_per_shader_stage == 0` (prototype `engine_base_limits`, 10 lines). 0.5 h |
| W2 | `pbr_forward`: frame bind group layout, then all 12 PBR pipelines (opaque / blended back / blended front × CCW / CW × MSAA 1 / 4) | `pbr/renderer.rs:615-632` (`storage(2, 8)`, `storage(3, 4)`), `pbr/shaders/forward.wgsl:57-58`, `:167-171` | `In Device::create_bind_group_layout, label = 'pbr frame' Too many bindings of type StorageBuffers in Stage ShaderStages(FRAGMENT), limit is 0, count was 2.` → `PipelineLayout with 'pbr' label is invalid` → `BindGroup with 'pbr frame' label is invalid`: **nothing of the table is drawn** (the post chain and the displays still are). The same blocker natively, without a browser: naga GLSL ES 3.00 (WebGL) for `pbr_forward::fs_opaque`/`fs_blended` → `The selected version doesn't support Features(BUFFER_STORAGE | DYNAMIC_ARRAY_SIZE)` (`results/glsl_es300_check.txt`) | needs a second shader/pipeline variant | Forward shader without the cluster lists: every light is tested per fragment (`for k < min(light_count, 128)`), bind group 0 keeps only the two uniforms. Prototyped (§5.2). Identical result for the app's scenes (4 key lights); cost O(lights) per fragment instead of O(lights in the cluster). If a table ever drives many dynamic lights, the alternative is cluster lists in `Rg32Uint`/`R32Uint` textures read with `textureLoad` (one shader for both backends). 3–4 h incl. tests |
| W3 | 1-layer placeholder reflection atlas (`texture_2d_array`, bind group 2 of every PBR draw) | `pbr/renderer.rs:494-500` (`empty_reflections`, `depth_or_array_layers: 1` viewed as `D2Array`) | `[ERROR wgpu_hal::gles] wgpu-hal heuristics assumed that the view dimension will be equal to D2 rather than D2Array.` (GL infers a texture's dimension from its layer count) | workaround in Rust | 2 layers on GL (shader clamps the layer; content unchanged). 0.25 h |
| W4 | `OctahedralProbes` cube-face capture texture (6 layers viewed as `D2Array`) — **not on the app path** today | `reflections/probes.rs:154` | `[ERROR wgpu_hal::gles] wgpu-hal heuristics assumed that the view dimension will be equal to Cube rather than D2Array.` | workaround in Rust | 7 layers on GL (or 6 separate 2D targets). 1 h incl. a test, only if probes (5.2) are enabled on the web |
| W5 | `raytrace_trace` (compute + `wgpu_ray_query` + storage texture) | `raytrace/mod.rs:252`, `raytrace/source.rs:502` | `Shader 'raytrace_trace' parsing error: the wgpu_ray_query extension is not supported in the current environment` / compute: `Downlevel flags DownlevelFlags(COMPUTE_SHADERS) are required but not supported` | visual feature absent on **both** web backends | None: already gated on `EXPERIMENTAL_RAY_QUERY` with the D18 probe fallback (`raytrace/select.rs`). 0 h |
| W6 | Canvas sRGB twin view (`view_formats`) | `gpu/surfaces.rs:691-803` | `In Surface::configure Downlevel flags DownlevelFlags(SURFACE_VIEW_FORMATS) are required but not supported` (WebGPU: works) | workaround in Rust | Use the WebGL2 canvas's own `Rgba8UnormSrgb` format, or `Rgba8Unorm` with the shader-side sRGB curve the post chain already has (the probe uses `Rgba8Unorm`, no issue) and the `LayerTarget::Intermediate` path for displays. 1 h |
| W7 | GPU timestamp queries | `gpu/mod.rs:393` | not offered | feature dropped | profiler HUD is native-only anyway. 0 h |

No error, on WebGL2, in: the post chain (6 pipelines; `textureLoad`/`textureDimensions` on
`Rgba16Float`, bloom mip chain, ACES composite), `Rgba16Float` MSAA 4× + `Depth32Float` MSAA 4×
+ resolve, DMD, display quads (LCD, backglass), debug lines, egui-wgpu, backbox scene and
composite. naga's GLSL ES 3.00 back end translated every app-path module (incl.
`@builtin(front_facing)`, `@builtin(vertex_index)`, `textureNumLayers`, dynamic uniform
offsets) without an error. No push constants, MSAA `textureLoad`, sampled depth or 3D textures
exist in the repo, so none of those failure classes occur.

### 5.2 Prototype of the worst blocker (W2, plus W1 and W3)

Scratch `pin_render` patch (`S/patches/pin_render.diff`, `pbr/renderer.rs` +71 non-comment
lines, `gpu/mod.rs` `engine_base_limits` +12): on a device with fewer than 2 fragment storage
buffers the PBR renderer builds `forward_source_without_storage()` (a text substitution of
`FORWARD_SOURCE`: drops bindings 2/3 and replaces the cluster lookup by `let li = k;` over
`frame.cluster_dims.w` lights), a 2-entry frame bind group layout, plain (non-STORAGE)
cluster buffers that are never bound, and the 2-layer placeholder; `request_async` picks
`downlevel_webgl2_defaults` on such adapters. Native behaviour is unchanged (the condition
is never true on Vulkan/Metal/DX12; native gpu-tests pass, §3.5).

Result (`results/render_webgl2_both_minimal.json`, `results/matrix_webgl2_both_minimal.json`):
all three production tables render on WebGL2 with MSAA 4×, bloom, motion blur and both PiPs;
every app-path matrix item is OK; the only remaining errors are W4 (probes, off the app
path), W5 (ray query, also absent on WebGPU) and the generic compute check.

WebGL2 (prototype) vs native lavapipe:

| Table | max abs | mean abs | px > 2 | px > 8 | px > 16 | px > 32 | % px > 8 | playfield-only max / mean / px > 8 |
|---|---|---|---|---|---|---|---|---|
| web_webgl2_both_minimal_apex_circuit.png | 145 | 0.0638 | 2762 | 1764 | 626 | 120 | 0.34 | 145 / 0.075 / 1764 |
| web_webgl2_both_minimal_sky_deck.png | 83 | 0.0729 | 3219 | 1980 | 668 | 94 | 0.382 | 83 / 0.0856 / 1980 |
| web_webgl2_both_minimal_feature_lab.png | 111 | 0.0472 | 1667 | 1586 | 457 | 54 | 0.306 | 111 / 0.0554 / 1586 |

WebGL2 (prototype, no clusters) vs WebGPU (clustered), same browser:

| Table | max abs | mean abs | px > 2 | px > 8 | px > 16 | px > 32 | % px > 8 | playfield-only max / mean / px > 8 |
|---|---|---|---|---|---|---|---|---|
| web_webgl2_both_minimal_apex_circuit.png | 26 | 0.0001 | 4 | 2 | 1 | 0 | 0.0 | 26 / 0.0001 / 2 |
| web_webgl2_both_minimal_sky_deck.png | 19 | 0.0001 | 3 | 3 | 3 | 0 | 0.001 | 19 / 0.0001 / 3 |
| web_webgl2_both_minimal_feature_lab.png | 1 | 0.0 | 0 | 0 | 0 | 0 | 0.0 | 1 / 0.0 / 0 |

Quality loss on the app path, measured: none visible; WebGL2 vs WebGPU differ in at most 4
pixels per frame (≤ 26/255), both against native as in §3.3. The estimate below is validated
by the prototype: the blockers were exactly the four predicted classes, and the fix for the
worst one took about 90 lines.

### 5.3 Effort, CI, test matrix and maintenance of a WebGL2 fallback

| Item | Estimate |
|---|---|
| One-time implementation | **3 T2 agent-runs**: implementer (W1 limits choice, W2 forward-shader variant as a real second WGSL entry or generated variant with a naga test, W3/W4 layer counts, W6 canvas format, `new_instance_with_webgpu_detection`, a `webgl` wasm artifact), Independent Tester (GLSL ES 3.00 translation of every module natively; WebGL2 page in the browser smoke), one fix round. About 7–8 % of Phase 9's 35–45 runs. The prototype covered W1–W3 in ~90 lines in one sitting |
| CI cost | +1 page per browser job (WebGL2 in the same Chromium launch: the 3-table render took 7.4 s here) and +1 wasm build/bindgen/wasm-opt per web CI run for the second artifact (a few minutes with native binaryen); the native GLSL check runs in `core-tests` in seconds (`cargo run -p s3_glsl_check` equivalent as a `pin_render` naga test; naga needs the `glsl-out` feature added to its dev pin) |
| Test matrix | ×2 only for the browser render smoke (WebGPU, WebGL2); everything GPU-free stays single |
| Maintenance | Rule for every new app-path shader/binding: no fragment/vertex storage buffers, storage textures or compute without a GL variant; enforced automatically by the GLSL ES 3.00 test (it reports `doesn't support Features(BUFFER_STORAGE …)` for exactly the browser's blocker, `results/glsl_es300_check.txt`) and the WebGL2 page. Low: a test failure, not a manual review item |
| Quality loss | None measured on the app path (§5.2). Dropped on WebGL2: clustered light culling (replaced by testing every light; same image, more ALU with many lights), GPU timestamps, ray-query reflections (also absent on WebGPU) |



## 6. Wasm size (A24)

What is measured: `web_probe` = the full **web render path** — `pin_render` (PBR, post chain,
displays, debug lines, egui overlay, backbox, probes; `pin_assetgen` fonts), `pin_assets`
(zip with live-ratio inflate, glTF, PNG/JPEG decode, manifest validation via `pin_core`'s TOML
schema), `egui`/`egui-wgpu`, wgpu, plus the probe glue and `serde_json`. **Not included:**
`pin_sim`/`pin_physics` solver/`pin_input`/the rules engine (S1's area; `pin_sim` cannot
compile for wasm32 until `pin_script`'s native backends are optional) and `pin_audio`.
Profiles: `--release` with `opt-level` = z / s / 3, `lto = "fat"`, `codegen-units = 1`,
`panic = "abort"` (already the wasm32-unknown-unknown default), `debug = false` (via
`CARGO_PROFILE_RELEASE_*`, `size_build.sh`); then `wasm-bindgen 0.2.130 --target web`; then
`wasm-opt -Oz` (binaryen 132, npm); gzip `-9` and brotli quality 11 / lgwin 24 (Node zlib,
`node/sizes.js`). `opt-level` s and 3 were measured on the WebGPU-only variant only (each
`wasm-opt` run took 3–10 minutes); the WebGL2 variants were not repeated at s/3.

| Variant | opt | cargo raw | after wasm-bindgen | after wasm-opt -Oz | -Oz gzip -9 | -Oz brotli -11 | JS glue (raw / br) | naga paths in wasm |
|---|---|---|---|---|---|---|---|---|
| webgpu | z | 13.44 | 10.92 | 6.36 | 2.06 | 1.46 | 84 KiB / 12 KiB | 0 |
| webgl | z | 16.34 | 14.13 | 7.92 | 2.69 | 1.93 | 76 KiB / 10 KiB | 55 |
| both | z | 16.95 | 14.46 | 8.00 | 2.72 | 1.95 | 133 KiB / 17 KiB | 55 |
| webgpu | s | 12.40 | 9.73 | 6.43 | 2.10 | 1.50 | 84 KiB / 12 KiB | 0 |
| webgpu | 3 | 9.63 | 6.68 | 5.27 | 2.03 | 1.48 | 84 KiB / 12 KiB | 0 |

(MiB = 2^20 bytes)

Where the bytes are (raw WebGPU-only modules at opt-level z and 3, code section attributed per
crate from the `name` section, `scripts/wasm_attrib.py` + `scripts/attrib_groups.py`; LTO inlining attributes
inlined code to the caller's crate, so read the groups as ±10 %):

opt-level z (`results/attrib_groups_webgpu_z.md`):

| Group | code KiB | % of code |
|---|---|---|
| egui text stack (epaint, vello_cpu/common, fearless_simd, skrifa, read_fonts, harfrust, kurbo, ...) | 3437 | 52.8 |
| egui + egui_wgpu | 228 | 3.5 |
| wgpu (WebGPU front + web bindings) | 156 | 2.4 |
| naga + wgpu_core + wgpu_hal (WebGL2 path) | 12 | 0.2 |
| toml / serde / serde_json | 912 | 14.0 |
| assets: zip, inflate, glTF, image (png, jpeg) | 249 | 3.8 |
| pin_* + probe | 501 | 7.7 |
| physics deps (parry3d, glam, simba, ...) | 9 | 0.1 |
| core / alloc / std / hashbrown | 868 | 13.3 |
| other (color, memchr, unicode_segmentation, <__wbindgen_describe___wbg_setImmediates_>, zmij, accesskit, <__wbindgen_describe___wbg_createImageBit>, foldhash, ...) | 113 | 1.7 |

opt-level 3 (`results/attrib_groups_webgpu_3.md`):

| Group | code KiB | % of code |
|---|---|---|
| egui text stack (epaint, vello_cpu/common, fearless_simd, skrifa, read_fonts, harfrust, kurbo, ...) | 1506 | 29.8 |
| egui + egui_wgpu | 493 | 9.7 |
| wgpu (WebGPU front + web bindings) | 151 | 3.0 |
| naga + wgpu_core + wgpu_hal (WebGL2 path) | 10 | 0.2 |
| toml / serde / serde_json | 857 | 17.0 |
| assets: zip, inflate, glTF, image (png, jpeg) | 290 | 5.7 |
| pin_* + probe | 743 | 14.7 |
| physics deps (parry3d, glam, simba, ...) | 11 | 0.2 |
| core / alloc / std / hashbrown | 723 | 14.3 |
| other (color, unicode_segmentation, <__wbindgen_describe___wbg_setImmediates_>, <__wbindgen_describe___wbg_copyBufferToBu>, <__wbindgen_describe___wbg_createImageBit>, <__wbindgen_describe___wbg_setVertexBuffe>, <__wbindgen_describe___wbg_setIndexBuffer>, <__wbindgen_describe___wbg_new_with_optio>, ...) | 231 | 4.6 |

Why opt-level 3 is the smallest here: `fearless_simd` (vello_cpu's SIMD abstraction, compiled to its scalar fallback because wasm32 has no `simd128` by default) is thousands of tiny generic trait methods. At opt-level z/s they stay out-of-line (21 020 / 14 516 functions, `fearless_simd` ≈ 1.9 / 1.6 MiB of code); at opt-level 3 they are inlined and folded (8 131 functions, `fearless_simd` 70 KiB). After `wasm-opt -Oz`: 3 844 functions / 4.07 MiB code at opt 3 against 8 045 / 5.14 MiB at z.

Findings:
- The raw cargo output carries a 3.3 MiB `name` section, 1.7 MiB of DWARF from the
  precompiled `std`, and 0.55 MiB of `__wasm_bindgen_unstable` metadata; wasm-bindgen drops the
  latter, `wasm-opt -Oz` drops names and DWARF. Only the post-`wasm-opt` number matters for
  download size.
- **naga is not linked in the WebGPU-only build** (0 `naga-30.0.1/src` and 0 `wgpu-core-30.0.1/src` panic paths in the
  binary, against 55 and 44 in the both-backends build; the 12 KiB in that bucket are shared
  small crates: `parking_lot`, `bitflags`, `arrayvec`, `log`), and the whole
  wgpu WebGPU front end plus the web bindings is ~156 KiB of code. The WebGL2 backend adds
  wgpu-core, wgpu-hal(GL) and naga (WGSL → GLSL in the wasm): +1.56 MiB after `wasm-opt -Oz` (6.36 → 7.92 MiB, +25 %) and +0.47 MiB brotli (1.46 → 1.93 MiB, +32 %) at opt-level z; naga + wgpu-core + wgpu-hal are ≈ 1.16 MiB of code (`results/attrib_groups_both_z.md`). A module with both backends costs only 0.08 MiB (0.02 MiB brotli) more than the WebGL-only one.
- The largest single item is **egui 0.37's CPU text stack** (`epaint` → `vello_cpu` +
  `fearless_simd` scalar fallback, `skrifa`/`read_fonts`/`harfrust`): 53 % of the code at opt-level z, 30 % at 3;
  it is a non-optional dependency of `epaint` 0.37 (`[dependencies.vello_cpu]`, no feature
  gate), plus ~1 MiB of default fonts in the data section (`default_fonts`, `Cargo.toml:51`).
- TOML + serde + serde_json ≈ 14–17 % (manifest parsing; serde_json only in the probe).
- `opt-level = 3` gave the smallest module after `wasm-opt` (5.27 MiB vs 6.36 at z) with the
  same brotli size (1.48 vs 1.46 MiB), for the reason given above.


## 7. egui on the web

- egui 0.37.0 + egui-wgpu 0.37.0 with the pinned wgpu 30.0.1 work in the browser: the real
  overlay theme (`pin_render::ui::overlay_style` + `show_overlay` + `OverlayPainter`, the code
  the app's menu and HUD use) was painted over each table frame on WebGPU
  (`shots/egui_web_webgpu_both_minimal_*.png`, 0 scope errors) and its pipeline builds and draws
  on WebGL2 (`results/matrix_webgl2_*`: "egui OverlayPainter (egui-wgpu 0.37) + paint: OK").
  With the §5.2 prototype the same overlay is also painted over the WebGL2 frames (`shots/egui_web_webgl2_both_minimal_*.png`).
- Feature hygiene: egui-wgpu must be used with `default-features = false` in the workspace;
  its defaults turn on `wgpu/default` and `wgpu/webgl` (a WebGL2 backend and naga in every
  wasm build) and `fragile-send-sync-non-atomic-wasm` (§1.1).
- egui 0.37 brings a CPU text stack (`epaint` → `vello_cpu`, `skrifa`, `harfrust`, the bundled
  fonts); it is a visible part of the wasm size (§6).
- Input handling the web shell must add (egui needs events from the host; `pin_render::ui::raw_input`
  already takes `Vec<egui::Event>`, a size and `pixels_per_point`): `pointerdown/move/up` →
  `Event::PointerButton/PointerMoved` (CSS px ÷ `pixels_per_point`), `wheel` →
  `Event::MouseWheel`, `keydown/keyup` → `Event::Key` (from `KeyboardEvent.code`, the same table
  as A26) plus `Event::Text` for printable input, `touchstart/move/end` → `Event::Touch` +
  emulated pointer, `focus/blur` → `Event::WindowFocused`, `devicePixelRatio` and
  `ResizeObserver` → `pixels_per_point` and the canvas size, `platform_output.cursor_icon` →
  `canvas.style.cursor`, optional clipboard (`navigator.clipboard`, needs a user gesture), and a
  hidden `<input>` for the on-screen keyboard on mobile (only if a text field is ever shown; the
  menu is controller/keyboard driven: `pin_app/src/ui/menu.rs`). Estimate: one T2 agent-run
  inside 9.3 (frame loop and input).
- **eframe: not adopted.** eframe's web runner owns the canvas, the `requestAnimationFrame`
  loop, its own wgpu device and the event handling, and draws custom content only through
  `egui_wgpu::Callback` inside egui's render pass. The engine needs the opposite ownership:
  `pin_render` owns the device and the frame (PBR → post → displays → overlay), and the sim's
  fixed-step loop drives the frame. eframe would mean a second device or rewriting the frame as
  an egui callback, and it is not used natively (pin_app uses SDL3 + `OverlayPainter`), so it
  would split the debug UI into two code paths. eframe would only be simpler for a standalone
  web debug page, which is not in scope.


## 8. Mobile angle (emulated in this environment)

`node run_probe.js mobile` (Playwright context: viewport 393×852 CSS px, `deviceScaleFactor: 3`,
`isMobile`, `hasTouch`, a Pixel-7 user agent; `results/mobile_both_minimal.json`):

- Canvas sizing: a `100vw × 100vh` canvas measures 393×852 CSS px at `devicePixelRatio = 3`,
  so the full-resolution drawing buffer is **1179×2556** physical pixels; `maxTouchPoints = 1`.
  The probe sized the canvas from `getBoundingClientRect() × devicePixelRatio` and rendered the
  production frame at 1179×2556 on **both** backends (WebGPU and WebGL2 with the §5.2
  prototype): `shots/mobile_{webgpu,webgl2}_both_minimal_apex_circuit_1179x2556.png`. The
  table camera (`table_camera`) fits the portrait aspect; the PiPs scale with the target.
- Size limits seen here (SwiftShader, not a phone): WebGL2 `MAX_TEXTURE_SIZE` 8192,
  `MAX_RENDERBUFFER_SIZE` 8192, `MAX_VIEWPORT_DIMS` 8192×8192, `MAX_SAMPLES` 4,
  `MAX_UNIFORM_BLOCK_SIZE` 65536, `MAX_FRAGMENT_UNIFORM_BLOCKS` 14, `MAX_TEXTURE_IMAGE_UNITS`
  32, `MAX_ARRAY_TEXTURE_LAYERS` 2048; extensions include `EXT_color_buffer_float`,
  `EXT_color_buffer_half_float`, `EXT_float_blend`, `OES_texture_float_linear`,
  `EXT_disjoint_timer_query_webgl2`. WebGPU `maxTextureDimension2D` 8192. A 1179×2556 target
  fits every one of them. **Needs S5/web:** real phone values (texture/renderbuffer limits,
  `MAX_SAMPLES`, and the coverage of `EXT_color_buffer_float`, which the `Rgba16Float` HDR
  target needs on WebGL2; none of these is measurable here). The
  engine already requests the adapter's real texture limit (`using_resolution`), and
  `check_size` (`pbr/renderer.rs`, `post/chain.rs`) refuses oversized targets, so the web shell
  must clamp the drawing buffer to `max_texture_dimension_2d` (and a render-scale setting).
- Relative pass costs (`results/bench_*_both_minimal.json`; SwiftShader on 4 shared CPU cores;
  **absolute numbers do not predict phone GPUs**, only the share of each pass):

Backend BrowserWebGpu, 3 frames per cell, ms per frame (wall clock to on_submitted_work_done)

| Variant | desktop 1080x1920 | phone 1179x2556 (DPR 3) | phone half-res 590x1278 |
|---|---|---|---|
| full (MSAA4+bloom+blur) | 3830 (100 %) | 5251 (100 %) | 2003 (100 %) |
| no MSAA | 2934 (77 %) | 4500 (86 %) | 1548 (77 %) |
| no bloom | 1791 (47 %) | 3124 (59 %) | 959 (48 %) |
| no motion blur | 3425 (89 %) | 4987 (95 %) | 1517 (76 %) |
| minimal (no MSAA/bloom/blur) | 1077 (28 %) | 1970 (38 %) | 544 (27 %) |

Backend Gl, 3 frames per cell, ms per frame (wall clock to on_submitted_work_done)

| Variant | desktop 1080x1920 | phone 1179x2556 (DPR 3) | phone half-res 590x1278 |
|---|---|---|---|
| full (MSAA4+bloom+blur) | 3226 (100 %) | 3626 (100 %) | 1332 (100 %) |
| no MSAA | 2961 (92 %) | 4002 (110 %) | 878 (66 %) |
| no bloom | 1558 (48 %) | 1881 (52 %) | 459 (34 %) |
| no motion blur | 2597 (80 %) | 4551 (125 %) | 1179 (89 %) |
| minimal (no MSAA/bloom/blur) | 925 (29 %) | 1272 (35 %) | 406 (30 %) |

- Quality ladder for phones (recommendation; the order follows the measured shares above and
  the general behaviour of tile-based mobile GPUs, where full-screen multi-tap passes cost
  bandwidth and MSAA resolves on-tile (not measured here); tune on real devices in 9.11):
  1. **Bloom first**: it is the largest share here (removing it cut 41–53 % of the frame on
     WebGPU). Use `max_levels` 4 instead of 6 (`BloomConfig`, exists) and add a
     half-resolution prefilter option (new, small); off at the lowest tier.
  2. **Render scale**: drawing buffer = CSS size × min(DPR, 2) × scale (default 0.75 on phones,
     floor 0.5). Cost tracks pixel count (phone 3.0 Mpx vs desktop 2.1 Mpx: ×1.4; half-res
     0.75 Mpx: ×0.5 of desktop).
  3. **MSAA off** (`AaMode::Off`) at the low and mid tiers (14–23 % of the frame here).
  4. **Motion blur** off at the low tier (5–24 %).
  5. **Shadows**: `pin_render` has **no shadow pass**, so there is nothing to scale. The
     reflection probes (`OctahedralProbes`, 5.2) are not on the app path; if enabled on the
     web: resolution 64 and 1 capture per frame on phones.
  6. Frame pacing from `requestAnimationFrame`, capped at 60 Hz on phones.

## 9. Recommendations

1. **(i) Q3 on effort alone: fund the WebGL2 fallback; it is not "huge effort".** Measured
   cost: the blockers are W1–W3 (§5.1), the worst one fixed in ~90 lines (§5.2), and with the
   prototype all three production tables render on WebGL2 with the full post stack and no
   visible quality loss (≤ 4 differing pixels per frame against WebGPU). Effort: **3 T2
   agent-runs** (implementer, Independent Tester, one fix round), about 7–8 % of the
   35–45-run Phase 9. Recurring cost: one more page per browser CI run (~10 s) and a
   GPU-free GLSL ES 3.00 translation check of every WGSL module in `pin_render`'s naga tests
   (seconds; `results/glsl_es300_check.txt` shows it flags exactly the browser's blocker).
   Size cost: +0.49 MiB brotli (1.46 → 1.95 MiB) if every visitor downloads one module with both backends; **zero for WebGPU visitors** if the fallback ships as a second artifact (WebGL-only module, 1.93 MiB brotli) that the loader fetches only when `requestAdapter()` fails. Whether the reach justifies it is S5's half (S6 combines).
   Conditions attached: real WebGL2 devices must offer `EXT_color_buffer_float` for the
   `Rgba16Float` target (true here; mobile coverage needs S5); a device without it needs an
   8-bit HDR-less fallback (+1 T2 run, visible loss: no bloom headroom) — recommend refusing
   such devices with the "no WebGPU/WebGL2-float" page instead.
2. **(ii) Feature set and flags.** Workspace: `wgpu = { version = "=30.0.1", default-features =
   false, features = ["std", "wgsl"] }` and `egui-wgpu = { version = "=0.37.0",
   default-features = false }`. `pin_render`: native backends under
   `[target.'cfg(not(target_arch = "wasm32"))'.dependencies] wgpu = { workspace = true,
   features = ["dx12", "metal", "vulkan", "gles", "parking_lot"] }` (today's native set; native
   gpu-tests pass with it); on wasm32 the features `webgpu` (always) and `webgl` (fallback
   artifact only). Do **not** use `fragile-send-sync-non-atomic-wasm`: gate the presenter
   thread on `not(target_arch = "wasm32")` instead (the only `Send` site; checked for webgpu,
   webgl and both). No `--cfg web_sys_unstable_apis`, no `RUSTFLAGS`, no `getrandom` cfg
   (zip enables `wasm_js` itself); no `+simd128` for this crate graph (it would also change the
   physics crates' codegen; S1's determinism call). With both backends, create the instance
   with `wgpu::util::new_instance_with_webgpu_detection`, never `Instance::new`. On GL adapters
   request `Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())`.
3. **(iii) Size budget:** render path (everything measured here: pin_render, pin_assets loaders, egui) ≤ 1.75 MiB brotli and ≤ 6 MiB decompressed for the WebGPU artifact (measured 1.48 / 5.27 MiB at opt-level 3), ≤ 2.25 MiB brotli for the WebGL2 artifact (1.93 MiB at z); the engine total is set after S1 adds pin_sim/physics/rules (proposed ≤ 3 MiB brotli for the whole WebGPU module). How to meet it, all measured: `opt-level = 3` (not z: smaller after `-Oz` here, 5.27 vs 6.36 MiB, brotli equal within 2 %; it is also the speed-optimising level, CPU speed not measured here), `lto = "fat"`, `codegen-units = 1`, `panic = "abort"` (default on this target), `debug = false`, then `wasm-bindgen` and `wasm-opt -Oz` (drops the 3.3 MiB name section and 1.7 MiB of DWARF), served with brotli; one artifact per backend chosen by the JS loader; the remaining big item is egui 0.37's text stack (30–53 % of code depending on opt-level) plus ~1 MiB of default fonts, so the largest further saving would be a lighter menu font set (not pursued: the menu needs readable text, B1 in `Cargo.toml:51`).
4. **(iv) CI harness.** Playwright (pin 1.63.0) Chromium headless shell, launched once with
   `--enable-unsafe-webgpu --enable-features=Vulkan --use-angle=vulkan --ignore-gpu-blocklist`
   and `VK_ICD_FILENAMES`/`VK_DRIVER_FILES` = Chromium's bundled `vk_swiftshader_icd.json`
   (deterministic SwiftShader for both backends regardless of the runner's Mesa). Pages are
   driven through async entry points with a timeout on every call (a wasm panic aborts the
   module and the promise never settles; a blocking wait spins forever). Assertions: analytic
   pixel checks as D30 (no goldens), plus a tolerance check against the native lavapipe frame
   of the same snapshot: mean abs ≤ 0.5/255 and ≤ 1 % of pixels with a channel difference > 8
   (measured: ≤ 0.073 and ≤ 0.38 %). Use native binaryen in CI (the npm/JS `wasm-opt` took
   about 9 minutes per module here under load). Proposed workflow: `S/ci/web-render-smoke.yml`
   (needs a CI trial for A23 on GitHub's image).
5. **Async API in `pin_render` (9.2):** `GpuContext::request_async` + native `pollster`
   wrappers; every error-scope pop through one helper that defers on the web; readback as
   `map_async` + `poll(Poll)` + yield (never `poll(Wait)`, `Instant` or `recv_timeout` on wasm32);
   a browser shader-compile check with `ShaderModule::get_compilation_info` (as the probe did).
6. **egui:** keep egui-wgpu + `OverlayPainter` (works on both backends); no eframe (§7).


## 10. Reproduce (commands the Work Verifier can re-run)

All from `S`; `. ./env.sh` first (CARGO_BUILD_JOBS=2, CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s3,
CARGO_INCREMENTAL=0, line-tables-only, PATH with the 0.2.130 CLI). Node needs
`NODE_PATH=/usr/local/lib/node_modules_global:/opt/node-tools/node_modules` (already the
default here).

| Step | Command | Expected outcome |
|---|---|---|
| Bundles | `for t in apex_circuit sky_deck feature_lab; do sh repo/tables/$t/build.sh bundles/$t; done; (cd bundles/<t> && zip -q -r -9 -X ../<t>.zip .)` | 3 bundles, "clearance ok"; zips ≈ 1.9 MB each (sizes may vary by zip timestamps; contents equal) |
| Native frames + references | `cd repo && cargo build --release -p s3_dump && WGPU_BACKEND=vulkan PIN_REQUIRE_GPU=1 $CARGO_TARGET_DIR/release/s3_dump $S/bundles $S/dumps 540 960` | adapter llvmpipe; `app_vs_probe_differing_bytes: 0` ×3; `dumps/native_*.png` |
| Native gpu-tests | `cd repo && WGPU_BACKEND=vulkan PIN_REQUIRE_GPU=1 xvfb-run -a cargo test -p pin_render --features gpu-tests --test spec_5_1_gpu --test post_gpu --test spec_5_2_gpu --test spec_5_3_gpu --test displays_gpu --test gpu_pbr_render --test ui_overlay_gpu --test srgb_blend_gpu` and `... -p pin_app --features gpu-tests --test wiring_render` | 106 + 7 passed |
| wasm32 checks | `cd repo && cargo check -p pin_render --target wasm32-unknown-unknown` (unchanged manifests: OK) and `--features webgpu` / `webgl` / `webgpu,webgl` (with the presenter stub: OK) | see §1.2 |
| Probe wasm | `./build_wasm.sh both fast bindgen` | `web/pkg/both/web_probe{.js,_bg.wasm}` |
| **TESTS REQUIRED** | `./run_render_probe.sh` | `[render/webgpu] apex_circuit:OK … sky_deck:OK … feature_lab:OK`, `[render/webgl2] …:OK ×3` (WebGL2 OK needs the §5.2 prototype, which the scratch `pin_render` contains), then the 6 PNG paths |
| All stages | `cd node && node run_probe.js adapter,render,egui,scopes,matrix,detect,canvasshot --pkg both --flags minimal`; `node run_probe.js detect --pkg both --flags none --backends webgpu`; `node run_probe.js blockon --pkg both --flags minimal`; `node run_probe.js mobile --pkg both --flags minimal`; `node run_probe.js bench --pkg both --flags minimal --frames 3` | `results/*.json`, `shots/*.png` as quoted above (timings vary; statuses and error texts do not) |
| Pixel diffs | `python3 -I scripts/compare.py dumps/native_<t>.png shots/web_webgpu_both_minimal_<t>.png` | §3.3 numbers (deterministic for the same Chromium/SwiftShader and Mesa builds) |
| GLSL ES 3.00 | `cd repo && cargo run -q -p s3_glsl_check` | `results/glsl_es300_check.txt` |
| A22 | `tools/wbg-0.2.129/bin/wasm-bindgen --target web --out-name web_probe --out-dir wasm/a22/cli129 wasm/web_probe-both-fast.wasm` | exit 1, schema 0.2.130 vs 0.2.129 |
| Sizes | `./size_build.sh webgpu:z webgl:z both:z webgpu:s webgpu:3` then `./size_opt.sh webgpu:z webgl:z both:z webgpu:s webgpu:3`; attribution `python3 -I scripts/wasm_attrib.py wasm/web_probe-webgpu-z.wasm 30` | §6 table (within ±1 %) |

Tolerances for a re-run: the browser PNGs reproduce **byte-exactly** with the same Chromium
build (the WebGPU/WebGL2 shots were regenerated by `run_render_probe.sh` and gave identical
diffs); the native PNGs and frame dumps are expected to (deterministic sim, D9; lavapipe
render; `native_probe_*.png` = `native_*.png` by hash) but `s3_dump` was run once; SHA-256 of
all of them in `results/sha256.txt`; wasm sizes within ±1 % (the
WebGPU-z module was built a few minutes before a no-op visibility change in the scratch
`pin_render`); all timings (ms) vary with load and are reported, never asserted (D8).


## 11. Follow-up effort, residual risks, files

**Effort for the follow-up implementation of what this spike validated** (agent-runs, tier):

| Work | Runs | Tier | Notes |
|---|---|---|---|
| 9.2 `pin_render` web path, WebGPU (manifests per §9 (ii); `request_async` + native `pollster` wrappers; one deferred error-scope helper for the 4 `pollster` pops; presenter-thread gate; async readback; canvas surface and format choice; browser shader-compile check) | 4 (2 implementer, 1 Independent Tester, 1 fix) | T2 | matches the proposal's 2–3 sessions + tester; the probe's `web.rs` is a working reference |
| Web render smoke in CI (part of 9.8): Playwright driver, flags, ICD pin, analytic + tolerance checks | 1 | T1 | `node/run_probe.js`, `ci/web-render-smoke.yml` |
| 9.9 WebGL2 fallback (W1–W3, W6; GLSL ES 3.00 naga test; detection-based instance; second artifact) | 3 | T2 | prototype in `patches/pin_render.diff` |
| egui web input (inside 9.3) | 1 | T2 | §7 list |

**Residual risks (not verified here, and why):**
- Only SwiftShader (Chromium's software Vulkan) was available: no real GPU, no Firefox,
  Safari or Android/iOS browser, no other Tint/WGSL front end than Chromium's. Firefox's WebGPU
  uses naga (validated natively by the repo's tests); Safari's WGSL compiler is untested —
  needs S5/web data and, later, device trials.
- The GitHub-hosted image was not tried (A23 "needs CI trial"); the local run hid the system
  Vulkan ICDs to mimic a runner without Mesa.
- Bench numbers were taken while other spikes built (load average 6–11 on 4 cores): absolute
  times are meaningless and the shares carry about ±10 points of noise (the bloom share was
  stable on both backends; MSAA and blur shares were not on WebGL2).
- The WebGL2 prototype is spike code (text substitution of the forward shader, not reviewed);
  the production change needs a proper variant and tests. Real-device WebGL2 needs
  `EXT_color_buffer_float` for the HDR target (S5).
- Sizes cover the render path (+ asset loading) only: `pin_sim`, `pin_physics` solver code,
  the rules engine and `pin_audio` are not in the measured module (S1's area).
- wasm panics abort the module and leave pending promises unsettled (seen with the WebGL2
  surface misuse): the web shell needs a panic hook that reports to the page and timeouts on
  every awaited call.
- The device loss on canvas present with only `--enable-unsafe-webgpu` is a headless/SwiftShader
  compositor interop effect; whether any real-GPU Chrome configuration shows it is unknown
  (the app should still handle device loss: re-create the context on `DeviceLost`).

**Files** (all under `S`):
- `RESULTS.md` (this file; generated from `body_part*.md` + `subs.json` by `scripts/assemble.py`),
  `DECISIONS_LOCAL.md`, `QUEUE_LINES.md`.
- Scripts: `env.sh`, `build_wasm.sh`, `size_build.sh`, `size_opt.sh`, `run_render_probe.sh`,
  `node/{server,png,run_probe,flags_probe,debug_render,debug_adapter,sizes}.js`,
  `scripts/{compare,tables,wasm_attrib,attrib_groups}.py`, `web/{index.html,main.js}`,
  `ci/web-render-smoke.yml`.
- Scratch code: `repo/crates/web_probe/`, `repo/crates/s3_dump/`, `repo/crates/s3_glsl_check/`;
  patches against `995935f`: `patches/{workspace-Cargo.toml,pin_render-Cargo.toml,pin_render}.diff`.
- Evidence: `results/` (adapter dumps, matrices, scopes, detect, mobile, bench, comparisons,
  attribution, GLSL check; `results/before_webgl2_proto/` = unpatched WebGL2 runs), `shots/`
  (WebGPU/WebGL2/egui/mobile/Playwright screenshots, `diff/`), `dumps/` (frame inputs, native
  lavapipe references, `native_adapter.json`), `wasm/` (modules and `sizes/`), `logs/`.


