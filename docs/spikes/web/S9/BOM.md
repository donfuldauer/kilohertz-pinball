# S9 — bill of materials for the browser build (issue text)

Everything below was used to build and test ONE `wasm32-unknown-unknown` engine module
(sim + real Lua + wasmi rules + wgpu WebGPU/WebGL2 renderer + bundle loader) in this spike.
`S` = `${WEB_SPIKES}/S9`.
Base: `main` at `995935f` + S1 patches 0001–0003, S2 0001–0002 (+ spike tooling 0003/0004), S3's
`pin_render` web patches, S4's `s4_rules` (see RESULTS.md §2).

## 1. Toolchain and tools (pinned)

| item | version | how obtained | checksum |
|---|---|---|---|
| rustc / cargo | 1.97.0 (2d8144b78 2026-07-07) / 1.97.0, LLVM 22.1.6 | rustup stable; targets `wasm32-unknown-unknown` (+ `wasm32-wasip1` for S2/S4 suites) | — |
| wasm-bindgen crate + CLI | **=0.2.130** (must match exactly, S3 A22) | `cargo install --locked wasm-bindgen-cli --version 0.2.130` | CLI sha256 `c0b53399…` (S1/S2 record) |
| js-sys / web-sys / wasm-bindgen-futures | =0.3.107 / =0.3.107 / =0.4.80 | Cargo.lock | — |
| wasm-tools | 1.263.0 | `cargo install --locked wasm-tools --version 1.263.0` | `52975ee5…` |
| wasi-sdk (clang 22.1.0 + wasi-libc 2fc32bc81b9f + libsetjmp) | **32.0** | `github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-32/wasi-sdk-32.0-x86_64-linux.tar.gz` (143 157 058 B) | sha256 `55fc523ebfbc98f69d1034fcfcb83d1ff5610cd9ab7eceef6cd097a30ba4ef93` |
| binaryen (wasm-opt), native | **version_132** | `github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-x86_64-linux.tar.gz` (117 057 408 B; `S/scripts/get_binaryen.sh`) | tarball sha256 `195ddc94f9bc89f45abdabb0b9eea86023d727ba90eac8b35b80f2544fc30572` (= the published `.sha256`); wasm-opt binary `1014958e…` |
| Node | v22.23.5 | system | — |
| Playwright / browser | 1.63.0; Chromium **157.0.8103.78** headless shell (`chromium_headless_shell-1243`), its bundled SwiftShader Vulkan ICD | preinstalled (`PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers`) | — |
| Firefox / WebKit for Playwright | **not installed** here (`/opt/pw-browsers` has Chromium only; `~/.cache/ms-playwright` absent) | — | — |

## 2. Crates (Cargo.lock of the scratch copy)

| crate | version | role / features |
|---|---|---|
| wgpu (+ wgpu-core, wgpu-hal, wgpu-types, naga) | =30.0.1 | workspace: `default-features = false, features = ["std", "wgsl"]`; pin_render: native backends on non-wasm32 only; web: `webgpu` + `webgl` (S3) |
| egui / egui-wgpu / epaint | =0.37.0 | egui-wgpu `default-features = false` (its defaults pull `wgpu/webgl`, `wgpu/default`, fragile-send-sync) |
| mlua / mlua-sys / lua-src | =0.12.2 / 0.13.0 / 551.0.2 (vendored, path) | Lua 5.4.9 + KHZ 1–32; pin_script features `lua` + `web-parity` |
| wasmi (+ wasmi_core, wasmi_ir, wasmi_collections) | =2.0.2 | `default-features = false, features = ["std", "validate", "deterministic", "auto-dispatch"]` (S4 F-A2: `auto-dispatch` is required) |
| wasmparser / wasm-encoder | =0.261.0 / =0.261.0 (S4 pre-scan + instrumentation); wasmparser 0.228.0 (wasmi's own) | `wasm-encoder` feature `wasmparser` |
| wat / wast | =1.263.0 / 263.0.0 | WAT text in pin_script's loader and S4's corpus |
| glam | =0.33.12 | built with `+simd128` (S1: bit-identical to x86_64 SSE2) |
| parry3d / libm | 0.31.1 / =0.2.16 | sim; libm also for Lua's math (KHZ 32) |
| zip / getrandom | 9.5.1 / 0.4.3 | `zip` enables `getrandom/wasm_js` itself on wasm32 (no cfg flag) |
| gltf / image / toml / serde_json | 1.4.1 / 0.25.6 / 1.2.1 / =1.0.151 | asset and manifest loading |
| sha2 | =0.10.9 | S4 (hash-pinned module list, harness) |

## 3. Every flag / env / feature that must be set together

| what | value | why (source) |
|---|---|---|
| `RUSTFLAGS` (target only, `--target` given) | `-C target-feature=+simd128 -C link-arg=-zstack-size=4194304` | simd128: glam SSE2-equivalent paths, exact native parity (S1); 4 MiB shadow stack (S1 A28; Lua C recursion needs ≤ 283 KiB, load ≤ 293 KiB measured here) |
| no `+exception-handling` rustflag | — | Rust code has no EH (panic=abort); the Lua objects carry EH; wasm-ld links them without it (S2, confirmed) |
| `CC_wasm32_unknown_unknown` / `AR_wasm32_unknown_unknown` | wasi-sdk 32 `bin/clang` / `bin/llvm-ar` | Lua C for the engine target (S2 route b); system clang-18 fails (C5) |
| `KHZ_WASI_SYSROOT` | `<wasi-sdk>/share/wasi-sysroot` | wasi-libc headers + `libc.a`/`libsetjmp.a` (KHZ 29); missing → build error (C4) |
| `KHZ_WASM_LEGACY_EH` | `true` (default) | legacy Wasm EH (`try/catch`) for setjmp/longjmp; no exnref in the module |
| `KHZ_LUA_MATH` | `1` | S2 recommended "umath" (Lua transcendental math via Rust libm on every target, KHZ 32); S2 asks to make it unconditional |
| pin_script features | `default-features = false, features = ["lua", "web-parity"]` | Lua only (no wasmtime/notify); no `collectgarbage` (S2) — the defaults pull wasmtime → `errno`/`rustix` fail on wasm32 (C6) |
| pin_sim | `default-features = false` (no `native-backends`) | `ExternalScript` + `load_from_vfs_with_script` (S1 0002) |
| pin_render features (web) | `webgpu`, `webgl` (both in one module; or one per artifact) | S3; instance via `wgpu::util::new_instance_with_webgpu_detection` for `auto` |
| `fragile-send-sync-non-atomic-wasm` | **off** | S3 recommendation; consequence: wgpu callbacks must be `Send` when `webgl` is compiled in (use `Arc`+atomics, not `Rc`+`Cell`; found here) |
| wasmi features | `std, validate, deterministic, auto-dispatch` | S4 (auto-dispatch: no browser-stack overflow; deterministic: canonical NaN) |
| harness crates | `default-features = false` (no `exports`, no `global-alloc`) | one `#[global_allocator]` (C1), one wasm-bindgen export set and one `start` (C2, C3) |
| panic strategy | `panic = "abort"` (target default; `CARGO_PROFILE_RELEASE_PANIC=abort`) | stable has no unwind on wasm32 (S1 A7); panics trap → page re-instantiates |
| profile | `opt-level = 3`, `codegen-units = 1`, `debug = false`, **LTO off** | S3's opt-level 3; LTO: cargo ignores `lto` for a `[cdylib, rlib]` crate (C7) and a real fat-LTO build (`cargo rustc --crate-type cdylib`) was **larger** after `-Oz` (9 391 508 vs 8 181 742 B; gzip -9 3 065 493 vs 2 912 358 B; both without the egui overlay) and within noise on speed (Node parity wall times) → ship without LTO, declare `lto = false` explicitly |
| wasm-bindgen | `--target web --out-name pin_web` | ES module glue; `initSync({module})` also works in Node for the CI parity jobs |
| wasm-opt | `-Oz --enable-simd --enable-exception-handling --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals --enable-reference-types --enable-multivalue --strip-debug --strip-producers` | S3 size recommendation; parity re-proven on the optimised module (20/20 Node runs identical before and after `-Oz`) |
| validation | `wasm-tools validate --features=mvp,simd,exceptions,legacy-exceptions,bulk-memory,saturating-float-to-int,sign-extension,mutable-global,reference-types,multi-value` | legacy `try` needs `legacy-exceptions`; the two tag imports need `exceptions` |
| server headers | `Content-Security-Policy: default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; worker-src 'self'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'`; `Content-Type: application/wasm`; **no COOP/COEP** | S4 CSP; nothing needs cross-origin isolation (no threads/SAB); `crossOriginIsolated=false` measured |
| Chromium test flags | `--enable-unsafe-webgpu --enable-features=Vulkan --use-angle=vulkan --ignore-gpu-blocklist` + `VK_ICD_FILENAMES`/`VK_DRIVER_FILES` = the headless shell's `vk_swiftshader_icd.json` | S3; without `--enable-unsafe-webgpu` there is no WebGPU adapter (used to test the WebGL2 fallback) |

## 4. Ordered build / CI steps (what `S/build_web.sh` + `S/run_integration.sh` do)

1. `rustup target add wasm32-unknown-unknown` (rustc 1.97.0).
2. Fetch + verify wasi-sdk 32.0 (sha256 above), extract; set `CC_/AR_wasm32_unknown_unknown`, `KHZ_WASI_SYSROOT`, `KHZ_LUA_MATH=1`.
3. `cargo install --locked wasm-bindgen-cli --version <Cargo.lock's wasm-bindgen>` and `wasm-tools 1.263.0`; fetch binaryen version_132 (sha256 above).
4. `RUSTFLAGS="-C target-feature=+simd128 -C link-arg=-zstack-size=4194304" cargo build --release --target wasm32-unknown-unknown -p pin_web` (opt 3, cgu 1, no LTO).
5. `wasm-bindgen --target web --out-name pin_web --out-dir dist/pkg target/wasm32-unknown-unknown/release/pin_web.wasm`.
6. `wasm-opt -Oz <features> --strip-debug --strip-producers` → `dist/pkg/pin_web_bg.wasm`; `wasm-tools validate` (feature list above).
7. `check_imports.sh` (fails on any non-glue import, any `env.*`/`wasi_*`).
8. Node gates on the final module: per-tick parity 10 scenarios × {Lua, rules.wasm} (`node/parity_node.mjs`), S2 sandbox / S4 adversarial / zip corpus / glam ops / D5 guard (`node/probes_node.mjs`).
9. Playwright Chromium (flags above): replay with rendering (`t_replay`), both backends + auto fallback (`t_backends`), player bundles (`t_player`), failure/recovery (`t_failures`), mobile emulation (`t_mobile`), load time (`t_loadtime`).
10. Size report (`scripts/size_report.sh`) against the budget.

## 5. Not tested here (residual)

Safari/iOS WebKit and Firefox (not installed; browser facts are S5's, `S5/RESEARCH.md` §6 and F1),
real phone GPUs and touch hardware (Chromium device emulation only), WebGPU on real GPUs (SwiftShader
only), GitHub-hosted runners (S3 A23 needs a CI trial), aarch64 browsers (S7 measured wasm on
emulated aarch64 = x86_64 bit for bit for the sim module: `S7/RESULTS.md` C2; not for this combined
module).
