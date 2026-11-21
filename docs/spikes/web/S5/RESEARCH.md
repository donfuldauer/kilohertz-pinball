# S5 research: browser platform facts (retrieved 2026-11-20)

Method: WebFetch/WebSearch only. "Retrieved" = 2026-11-20 for everything. Tool caveat: WebFetch summarises pages with a small model, so numbers below are as the summary reported them; the most decision-critical ones (WebGPU/WebGL2 shares, wasm feature versions) should be re-checked by the Work Verifier by opening the cited URL. caniuse lists "Chrome for Android 157" etc. because it tracks the current version; I treat that as "current".
Data dates: caniuse usage = Oct 2026 (StatCounter-derived); StatCounter = Oct 2026; Steam survey = Oct 2026.

## 1. WebGPU status (item 1)

Sources: gpuweb Implementation Status wiki https://github.com/gpuweb/gpuweb/wiki/Implementation-Status (fetch showed "last updated Oct 2 2026"; a later search snippet showed May 28 2026: unresolved discrepancy, likely cache), web.dev https://web.dev/blog/webgpu-supported-major-browsers (2025-11-25), caniuse https://caniuse.com/webgpu, WebKit https://webkit.org/blog/17333/webkit-features-in-safari-26-0/, Chrome https://developer.chrome.com/blog/new-in-webgpu-146.

| Engine / OS | Status | Version | Notes |
|---|---|---|---|
| Chrome/Edge Windows (x86/x64), macOS, ChromeOS | stable | 113 (Chrome/Edge); Opera 99 | caniuse: flag-gated 80-112 |
| Chrome/Edge Windows ARM64 | flag only | n/a | `chrome://flags/#enable-unsafe-webgpu` |
| Chrome Android | stable, restricted | 121 | Android 12+, ARM/Qualcomm/Intel; Imagination GPUs need Android 16+ (139); Samsung Xclipse and others TBD (wiki) |
| Chrome Linux | restricted | 144 (Intel Gen12+), 147 (NVIDIA >=535.183.01 on Wayland) | everything else needs `--enable-unsafe-webgpu --ozone-platform=x11 --use-angle=vulkan --enable-features=Vulkan,VulkanFromANGLE` |
| Firefox Windows | stable | 141 (Jul 2025) | |
| Firefox macOS | stable | 145 (Apple Silicon, macOS 26+); 147 all macOS versions | wiki also says Intel Macs: Nightly only (conflicting rows on same page; low confidence) |
| Firefox Linux | Nightly only | n/a | Mozilla "expects 2026"; I could not confirm a stable Linux release as of Nov 2026 |
| Firefox Android | disabled; Beta/Nightly via `gfx.webgpu.ignore-blocklist` | n/a | |
| Safari macOS/iOS/iPadOS/visionOS | stable | 26.0 | caniuse: iOS 26.0 supported, desktop Safari "partial" 26.0-27.3; disabled by default 17.4-18.7 |
| Samsung Internet | stable | 24 (caniuse) | caveat: caniuse does not apply Android 12/GPU restrictions |
| Opera / Opera Mobile | 99 / 80 (caniuse) | | |
| Chrome WebView | **not found**: unverified | | queries: caniuse, gpuweb wiki |
| Chrome "compatibility mode" (WebGPU on OpenGL ES 3.1) | shipped on Android | Chrome 146 (Feb 2026) | `requestAdapter({featureLevel:"compatibility"})`; D3D11 and ChromeOS only "exploring" |

caniuse treats Firefox 63-163 as "disabled/not supported", which contradicts the wiki and web.dev (Windows 141 shipped): caniuse undercounts Firefox WebGPU; but it overcounts Chrome Android/Linux/old Windows GPUs. Net sign unknown.
Blockers: Linux (hardware list above), Android <12 or non-ARM/Qualcomm/Intel GPUs, Windows ARM64, Firefox Linux/Android. Hardware blocklists exist (Firefox `gfx.webgpu.ignore-blocklist` pref evidences it); exact lists not retrieved.

## 2. Shares (item 2)

Raw numbers: caniuse WebGPU global 88.63% (86.83 full + 1.8 partial); WebGL2 97.29%; WASM SIMD 96.78%; SharedArrayBuffer 97.13%; AudioWorklet 97.29%; wasm threads 97.18%; COEP credentialless 79.51%. URLs: https://caniuse.com/webgpu, https://caniuse.com/webgl2, https://caniuse.com/wasm-simd, https://caniuse.com/sharedarraybuffer, https://caniuse.com/wasm-threads.

Computed:
- WebGPU supported share (all users): **88.63% upper bound** (caniuse; counts any Chrome 113+, Android 157, Safari 26 as supported; ignores Linux/Android-GPU/driver limits).
- WebGL2 supported share: **97.29%**.
- Have WebGL2 but not WebGPU (the Q3 gain): 97.29 - 88.63 = **8.66 percentage points upper-bound-adjusted floor** (i.e. at least ~8.7 pp; true value is higher because caniuse overcounts WebGPU, lower in the Firefox-Windows part). I found no source that corrects for the overcount, so the real number is **unverified**; no desktop-only / mobile-only caniuse split is published, so (b) and (c) are derived from StatCounter below.

StatCounter Oct 2026 (https://gs.statcounter.com/browser-market-share, /desktop/worldwide, /mobile/worldwide, https://gs.statcounter.com/os-market-share):
- All: Chrome 68.79, Safari 16.59, Edge 5.36, Firefox 2.57, Opera 2.16, Samsung 2.05.
- Desktop: Chrome 66.98, Edge 13.24, Safari 7.83, Firefox 5.72, Opera 2.65, Brave 1.01 (listed sum 97.43). Chromium family 83.88; WebGPU-by-default on Windows/macOS/ChromeOS; Safari needs macOS 26; Firefox needs Windows or macOS (145/147+).
- Mobile: Chrome 70.37, Safari 21.91, Samsung 3.16, Opera 1.88, UC 0.94, Firefox 0.61. Caveat: Chrome/Firefox on iOS are WebKit, so "Chrome" mobile share includes iOS users who follow the iOS version (needs iOS 26). OS: Android 44.56, Windows 29.08, iOS 17.45, OS X 3.74, macOS 2.2, Linux 2.15.
- Desktop without WebGPU (derivable bound): Linux ~2.15% of all OS (hence about 3% of desktop OS by rough division; unverified calc) plus macOS < 26 and unsupported GPUs; not quantifiable from sources.
- Mobile without WebGPU: Firefox Android 0.61 + Android < 12 / non-supported GPU + iOS < 26: not quantifiable from sources; this is where most of the 8.7+ pp sits.

Audience (Steam Hardware Survey Oct 2026, https://store.steampowered.com/hwsurvey/): Windows 11 72.51%, Windows 10 21.11%, Linux 3.95%, macOS 2.35%. Top GPUs: RTX 3060 3.62, RTX 5070 3.56, RTX 4060 Laptop 3.40, RTX 4060 3.32, RTX 3050 3.06 (all D3D12-capable NVIDIA). Vendor totals and DX12 share not retrieved. So ~93.6% of Steam users are on Windows and run D3D12 GPUs: Chrome/Edge/Firefox-Windows all ship WebGPU for them. Linux 3.95% (and 2.15% overall) is the main desktop-gamer group that is WebGPU-disadvantaged (Chrome Linux needs Intel Gen12+ / NVIDIA Wayland or flags; Firefox Linux not stable).

## 3. WebGL2 and wgpu (item 3)

- WebGL2 97.29% global; Chrome 56, Edge 79, Firefox 51, Safari 15 (macOS), iOS Safari 15; Samsung 7.2 (https://caniuse.com/webgl2).
- Vendor deprecation of WebGL: **no deprecation notice found** in my searches (not exhaustively checked); WebKit says WebGPU "supersedes WebGL" for new sites (Safari 26 post) but WebGL stays.
- GPU blocklist behaviour for WebGL2: **not retrieved** (unverified).
- wgpu current: **30.0.2 (2026-11-12)**; 30.0.1 2026-08-21; 30.0.0 2026-07-01; 29.0.4 2026-07-01 (CHANGELOG https://raw.githubusercontent.com/gfx-rs/wgpu/trunk/CHANGELOG.md). 30.0.1 bumped web bindings to wasm-bindgen 0.2.127. Unreleased: wasm64 for web backend, OffscreenCanvas in `copy_external_image_to_texture` on WebGL.
- `Limits::downlevel_webgl2_defaults()` (https://docs.rs/wgpu/latest/wgpu/struct.Limits.html, v30.0.2): all storage-buffer, storage-texture and compute limits = 0 (no compute shaders, no storage buffers), max_texture_dimension_2d 2048, uniform buffers per stage 11, uniform binding size 16 KiB. wgpu's old wiki calls the WebGL2 backend "experimental and missing many features" but it is dated 2022 (stale).
- Implication: any engine renderer using compute or storage buffers needs a separate WebGL2 path (separate shader set). The repo-side check is for S-render spikes.

## 4. Mobile (item 4)

Found:
- iOS memory: no official per-tab number. Apple forum report: 2048 MB "ActiveHard" cap on iPhone class, iPad more generous (single developer, https://developer.apple.com/forums/thread/823061 as returned by search; low confidence). WebKit contributor: kill decisions follow system pressure, not a fixed limit. Three.js forum thread reports memory crashes after iOS 26 (https://discourse.threejs.org/t/memory-limit-crash-on-ios-26/86978). Treat <= ~1 GiB wasm memory on iPhone as the safe working assumption (my recommendation, not a sourced number).
- Safari 26 added an in-place wasm interpreter (lower launch memory) (WebKit 26.0 post).
- Android per-tab memory limit: **not found**. wasm memory64 status: caniuse page content not captured (unverified); wasm.org table (below): Chrome 133, Firefox 134, Safari only in Technology Preview. wasm32 practical max: not retrieved (spec 4 GiB, browsers vary: unverified).
- Thermal throttling, 120 Hz cadence: rAF follows display rate (60/75/120/144 Hz), timestamp precision >= 1 ms (https://developer.mozilla.org/en-US/docs/Web/API/Window/requestAnimationFrame). Thermal numbers: not found.
- `visibilitychange` on mobile: not fetched (unverified); rAF paused in background tabs (MDN).
- Pointer lock: iOS Safari none; Chrome Android none; Samsung none; Firefox Android yes (https://caniuse.com/pointerlock). Fullscreen API: iPhone Safari "partial" 12-27.x (https://caniuse.com/fullscreen). Screen orientation `lock()`: iOS Safari not supported; Chrome Android, Firefox Android 160 yes (https://caniuse.com/mdn-api_screenorientation_lock). Wake Lock: iOS Safari 16.4+, Chrome 84, Firefox 126 (https://caniuse.com/wake-lock). Vibration: not iOS, not Firefox >= 129, supported Chrome Android/Samsung (https://caniuse.com/vibration) (needs sticky user activation per MDN). Gamepad `vibrationActuator`: Chrome 68, Safari desktop 16.4, Samsung 10.1; not Firefox, not iOS Safari (https://caniuse.com/mdn-api_gamepad_vibrationactuator).
- iOS Web Audio and silent switch: WebKit treats Web Audio as ambient and obeys the mute switch; common workaround (unmute-ios-audio) plays a silent `<audio>` element on user gesture (https://www.github.com/feross/unmute-ios-audio; WebKit bug 237322); newer-iOS validity of the hack unverified.
- AudioContext iOS needs a user gesture; `resume()` after gesture (Chrome autoplay doc https://developer.chrome.com/blog/autoplay; Firefox blocks Web Audio until sticky activation per MDN autoplay guide).
- `outputLatency`: Chrome 102, Firefox 70, Safari 18.4 / iOS 18.4 (search result citing caniuse https://caniuse.com/mdn-api_audiocontext_outputlatency). Measured latency per OS: **not found** (a search surfaced only an anecdotal forum post); must be measured with a loopback on devices.
- Touch/pointer multi-touch, `<input type=file>` on iOS, drag-drop, Android gamepad: **not researched** (not found/not fetched). Treat as unverified.
- Feasibility: mobile playability for a 1 kHz sim is a CPU question for S1's numbers (open). Sourced risks: iOS jetsam memory kills, no pointer lock, no vibrate on iOS, Safari memory behaviour, no COEP credentialless on Safari/Firefox Android (but require-corp works).

## 5. Specific APIs (item 5)

- Gamepad: polled via `getGamepads()` each frame; `mapping === "standard"`; `timestamp` (MDN says Firefox lacks it; unverified current); Firefox exposes pads only after the user touches one (anti-fingerprinting) (https://developer.mozilla.org/en-US/docs/Web/API/Gamepad_API/Using_the_Gamepad_API). `vibrationActuator`: see section 4. No SDL GUID exposed: the `id` string only (not re-verified; general knowledge, unverified). Background tabs/Permissions-Policy `gamepad`: not from page, unverified.
- AudioWorklet: Chrome 66, Firefox 76, Safari 14.1, iOS 14.5, Samsung 9.2 (https://caniuse.com/mdn-api_audioworklet). 128-frame quantum: from spec, not re-fetched (high-confidence general knowledge, flag unverified here). `baseLatency` ~128/sampleRate per MDN-derived statement.
- Autoplay: Chrome since 71 needs user gesture, `resume()`; installed PWA/home screen exempt; MEI desktop only (https://developer.chrome.com/blog/autoplay). `navigator.getAutoplayPolicy("audiocontext")` exists (MDN).
- COOP/COEP: isolation = `Cross-Origin-Opener-Policy: same-origin` + COEP `require-corp` or `credentialless` (https://developer.mozilla.org/en-US/docs/Web/API/Window/crossOriginIsolated). `credentialless`: Chrome/Edge 96, Firefox 119, Opera 82, Samsung 17; **not Safari (any version through 27.2), not Firefox Android 160** (https://caniuse.com/mdn-http_headers_cross-origin-embedder-policy_credentialless). Safari support for `require-corp` is not in the fetched pages (the SAB page says Safari 15.2 SAB; cross-origin-isolation specifics unverified). `http://localhost` and 127.0.0.1 are secure contexts (https://developer.mozilla.org/en-US/docs/Web/Security/Defenses/Secure_Contexts). A localhost dev server can set the headers (verified: my 20-line Node server below returned COOP/COEP via curl; crossOriginIsolated=true in a browser was not checked here, needs S-browser test). Same-origin local files and fonts are fine under `require-corp` only if same-origin or served with CORP/CORS: add `Cross-Origin-Resource-Policy: same-origin`.
- `Atomics.wait` throws on the main thread; use in Workers; `Atomics.waitAsync`: Chrome 90, Firefox 145, Safari 16.4 (https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/Atomics/wait, https://caniuse.com/mdn-javascript_builtins_atomics_waitasync).
- `performance.now()` resolution: 100 us normally, 5 us when cross-origin isolated (MDN https://developer.mozilla.org/en-US/docs/Web/API/Performance/now; MDN does not say per browser). Firefox: a Mozilla bug report says default rounding 1 ms (https://bugzilla.mozilla.org/show_bug.cgi?id=1888894 as surfaced by search; medium-low, older reports say 20 us/2 ms). Safari: **not found**. rAF timestamp min precision 1 ms; MDN notes performance.now keeps ticking during sleep only on Windows.
- Timer throttling: hidden >5 min, chain count >= 5, silent 30 s, no WebRTC => once per minute; audio played in the past 30 s exempts (silent track does not count); WebRTC data channel/track => 1 s class (https://developer.chrome.com/blog/timer-throttling-in-chrome-88). The article does not say whether Workers are affected: **unverified**; test in S-browser. rAF paused in hidden tabs (MDN).
- Keyboard: `KeyboardEvent.code` is physical-position, layout independent; named after USB HID conventions but **the spec does not map code values to HID numbers** (https://w3c.github.io/uievents-code/). `Keyboard.lock()`: Chrome 68+, Edge 79+, Opera 55, Samsung 10.1; **not Firefox, not Safari** (https://caniuse.com/mdn-api_keyboard_lock). Pointer lock: desktop all (Chrome 22, Firefox 14, Safari 10.1), none on mobile Chrome/iOS. Browser-reserved shortcuts (Ctrl+W, F11, Esc): MDN says Esc, Alt+Tab, Ctrl+N are UA/OS captured; whether lock() can capture each: unverified, only writing-system keys are lockable per MDN.

## 6. Wasm features (item 6)

Table from the WebAssembly features data (https://webassembly.org/features/, data file fetched via raw GitHub features.json, retrieved 2026-11-20):

| Feature | Chrome | Firefox | Safari |
|---|---|---|---|
| exceptions (legacy) | 95 | 100 | 15.2 |
| exnref / final EH | 137 | 131 | 18.4 |
| JSPI | 137 | 153 | 27 |
| tail call | 112 | 121 | 18.2 |
| GC | 119 | 120 | 18.2 |
| multi-memory | 120 | 125 | not listed |
| memory64 | 133 | 134 | Technology Preview only |
| threads/atomics | 74 | 79 | 14.1 (iOS 14.5) |
| SIMD128 | 91 | 89 | 16.4 |
| relaxed SIMD | 114 | 145 | flag only |
| bulk memory | 75 | 79 | 15 |
| reference types | 96 | 79 | 15 |
| extended const | 114 | 112 | 17.4 |
| multi-value | 85 | 78 | 13.1 |

Notes: the first fetch of this file (a different path) 404'd; the raw features.json path worked once. A search result (caniuse exnref table) is inconsistent on Edge and a WebKit bug (293340) reports an exnref global edge case in JSC: medium confidence for Safari exnref 18.4. Legacy EH is Baseline widely available since 2024-11-03 (web-features). Kotlin still defaults to legacy EH (toolchain caution).
CSP `'wasm-unsafe-eval'`: Chrome/Edge 97, Firefox 102, Safari/iOS 16.0, Samsung 18.0 (https://caniuse.com/mdn-http_headers_content-security-policy_script-src_wasm-unsafe-eval).
Rust default features for wasm32-unknown-unknown (https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html): `generic` CPU: multivalue, mutable-globals, reference-types, sign-ext, plus nontrapping-fptoint and bulk-memory since Rust 1.87 (LLVM 20). Not default: atomics, SIMD128, exception handling, tail-call. `-Ctarget-cpu=mvp` + `-Zbuild-std` for MVP. `std::thread::spawn` panics on this target.
Baseline mobile floor, derived from the table (iOS Safari version = WebKit version): with default Rust features (bulk memory 15, reference types 15, multi-value 13.1) the floor is iOS 15; threads iOS 14.5; SIMD iOS 16.4; WebGPU iOS 26. CSP wasm-unsafe-eval iOS 16.
Sync compile limit: Chrome historically 4 KB on main thread for `new WebAssembly.Module`, a Chromium commit raised to 8 MB, and a 2023 intent proposed deprecating it; current limit **unverified**; Firefox/Safari have no special limit (https://groups.google.com/a/chromium.org/g/blink-dev/c/nJw2zwaiJ2s, https://web.dev/articles/loading-wasm). Use `WebAssembly.compileStreaming`/worker anyway (needs `application/wasm` MIME).

## 7. Tooling (item 7)

- wasm-bindgen: latest 0.2.130 (2026-10-30), 0.2.129 (09-25), 0.2.128 (09-05); 0.2.128 notes: "Requires matching wasm-bindgen and CLI versions (schema bump)". Repo now wasm-bindgen/wasm-bindgen (https://github.com/rustwasm/wasm-bindgen/releases). wgpu 30.0.1 pins 0.2.127, so wgpu and a 0.2.130 CLI will conflict: pick the crate version the Cargo.lock resolves and install the identical CLI `cargo install wasm-bindgen-cli --version <exact> --locked`. Prebuilt CLI binaries for CI: **not confirmed** (docs only show cargo install; https://wasm-bindgen.github.io/wasm-bindgen/reference/cli.html).
- wasm-pack: github.com/rustwasm/wasm-pack shows no archive banner and "maintained by drager" (retrieved page; I could not confirm the rustwasm org sunset I recall). Medium-low. Recommendation: do not depend on it.
- trunk: v0.22.2 (2026-11-18); whether it auto-downloads wasm-bindgen/wasm-opt unconfirmed (https://github.com/trunk-rs/trunk/releases).
- binaryen/wasm-opt: version_133 (2026-09-21), prebuilt tarballs for x86_64-linux and arm64-macos; Windows x86_64 unconfirmed (https://github.com/WebAssembly/binaryen/releases).
- `-Z build-std` for atomics (A8): rustc page says threading unsupported on stable target and std rebuilt needed for non-default features; explicit "atomics require nightly build-std" sentence not on the page (see A8).
- GitHub Pages cannot set COOP/COEP; workaround coi-serviceworker (reload on first load, possible reload loops) (https://github.com/orgs/community/discussions/13309, https://npmjs.org/package/coi-serviceworker). For the record only.
- Local server (verified headers with curl on Node v22, 2026-11-20; no file written in the repo): 

```js
// serve.js : node serve.js <dir> [port]
const http=require("http"),fs=require("fs"),path=require("path");
const root=path.resolve(process.argv[2]||"."),port=+process.argv[3]||8080;
const mime={".html":"text/html; charset=utf-8",".js":"text/javascript",".mjs":"text/javascript",".wasm":"application/wasm",".json":"application/json",".css":"text/css",".png":"image/png",".svg":"image/svg+xml",".woff2":"font/woff2",".zip":"application/zip",".lua":"text/plain; charset=utf-8"};
http.createServer((q,r)=>{let p=path.normalize(path.join(root,decodeURIComponent(q.url.split("?")[0])));
if(!p.startsWith(root)){r.writeHead(403).end();return}
if(fs.existsSync(p)&&fs.statSync(p).isDirectory())p=path.join(p,"index.html");
fs.readFile(p,(e,d)=>{const h={"Cross-Origin-Opener-Policy":"same-origin","Cross-Origin-Embedder-Policy":"require-corp","Cross-Origin-Resource-Policy":"same-origin","Cache-Control":"no-store"};
if(e){r.writeHead(404,h).end("not found");return}
h["Content-Type"]=mime[path.extname(p)]||"application/octet-stream";r.writeHead(200,h).end(d)})}).listen(port,"127.0.0.1",()=>console.log("http://localhost:"+port));
```
Curl output showed COOP/COEP/CORP/no-store and `Content-Type: application/wasm` for a .wasm file. Not tested: `crossOriginIsolated === true` in Chromium, Range requests, path traversal tests beyond the startsWith check (symlinks), speed. The Python `http.server` cannot set headers unless subclassed; miniserve/static-web-server options not evaluated (unverified), so my recommendation is the Node server (no extra install; Node 22 exists).

## 8. What this means for the decisions

- Q3 (WebGPU vs WebGL2): gain of WebGL2 is at least 8.7 percentage points of all web users (97.29 - 88.63, caniuse Oct 2026) and probably more because caniuse counts Chrome 113+/Android as WebGPU even where it is blocked; but for the stated audience (local hobby/enthusiast game, ~94% Windows on Steam, desktop Chrome/Edge 80%+ of desktop) the WebGPU-capable fraction is near total. Recommendation: **WebGPU only for the initial implementation; no WebGL2 fallback**. Reasons: wgpu's WebGL2 limits are zero compute/storage buffers (so a second shader path would be significant effort), and the people it would help are mainly Linux, older Android, and old iOS, which are outside the audience. Add a clear "WebGPU required" message with `navigator.gpu` + `requestAdapter()` feature detect. Revisit if a user-facing need on Linux appears.
- Q7 (mobile): sourced floor: **iOS/iPadOS Safari 26+** (WebGPU) and **Chrome Android 121+ on Android 12+ with ARM/Qualcomm/Intel GPU**; Firefox Android and Samsung Xclipse GPUs out. Mobile is feasible only if S1's sim timing passes and memory stays under roughly 1 GiB wasm on iPhone (unsourced assumption). No pointer lock, no iOS vibrate, no orientation lock on iOS, fullscreen partial on iPhone: touch UI must be designed for these. Recommendation: **mobile in scope as best-effort for iOS 26+/Android 12+ Chrome, not a release gate**, until S1 numbers exist.
- Q2 (local server): use the Node server above (or any server that adds `COOP: same-origin`, `COEP: require-corp`, `CORP: same-origin`, `Cache-Control: no-store` and `application/wasm`). Only `http://localhost` (secure context) works for WebGPU without HTTPS; LAN access from a phone requires HTTPS (a plain-HTTP LAN IP is not a secure context per MDN, so mobile testing needs a TLS-terminating tunnel/cert) - note this for Q7.
- Input mapping: use `KeyboardEvent.code`, never `key`, then a JS table `code -> HID usage` (the generic profile uses SDL scancodes == HID usages: see A26). Do not use `Keyboard.lock()` as a requirement (Chromium only); document that Esc exits fullscreen, F11 and Ctrl+W are browser-owned. Gamepad: poll `getGamepads()` each tick and require `mapping==="standard"` else use a manual-mapping UI; rumble only on Chromium/Safari-desktop.
- Audio: AudioWorklet (all targets) fed from a ring buffer; create the AudioContext on the first user gesture and `resume()`; plan the iOS mute-switch workaround; read `outputLatency` where available (Safari 18.4+) but never rely on it.
- Timing: run the 1 kHz sim in a Worker driven by a clock derived from rAF/audio time, not relying on `setTimeout(1)`; treat `performance.now()` as 100 us (5 us when isolated; Firefox maybe 1 ms) so use accumulator stepping with fixed 1 ms ticks; pause on `visibilitychange`; hidden-tab throttling to 1/min after 5 min (audio exempts). Worker throttling unverified.

## Assumption verdicts

- **A2 (WebGPU coverage)**: confirmed with changes. Chrome/Edge 113 (Win/mac/ChromeOS), Firefox 141 Win / 145-147 macOS, Safari 26; mobile partial (Android 12+, Android GPU limits, iOS 26); Linux restricted; Firefox Linux/Android not stable. Section 1.
- **A8 (atomics needs nightly build-std)**: confirmed, medium. Rustc page: atomics not default, std::thread panics on this target, build-std needs nightly for non-default features; explicit atomics sentence not fetched. Needs a toolchain check (the proposal's own method).
- **A9 (SAB/timers need COOP/COEP; Pages cannot set; Atomics.wait Workers only)**: confirmed. MDN crossOriginIsolated, GitHub discussion 13309, MDN Atomics.wait. Pages claim rests on dated community sources (2023); medium.
- **A11 (wasmtime cannot run in browser; renderer isolation per site)**: wasmtime half: partly confirmed, low-medium. Wasmtime maintainers say much of the embedder API is impossible in browsers; JIT unsupported on wasm32-unknown-unknown; Pulley might run but a 2025 report shows a crash, no official support (zulip archive link surfaced in search: https://bytecodealliance.github.io/zulip-archive/stream/206238-general/topic/pulley.20on.20wasm32-unk-unk.20via.20deserialized.20module.20crashes.html). Per-site renderer isolation half: **not retrieved, unverified**.
- **A15 (performance.now coarsening)**: confirmed (100 us / 5 us isolated per MDN); per-browser values and Safari unverified; Firefox likely 1 ms default; `event.timeStamp` timeline and compositor-frame latency not verified here.
- **A16 (rAF stops hidden; timers throttled)**: confirmed (MDN; Chrome 88 article: hidden >5 min => 1/min, audio exempt). Workers throttling unverified.
- **A17 (Gamepad)**: confirmed for polling, standard mapping, timestamp; `vibrationActuator` not on Firefox or iOS (caniuse). SDL GUID claim unverified (not fetched).
- **A18 (AudioWorklet)**: partly confirmed: gesture requirement and `resume()` confirmed; 128-frame quantum not re-fetched (spec, high prior); "typical output latency 10-40 ms": **not confirmed**, no measured per-OS source found (needs device loopback test); only anecdotal ~41 ms.
- **A21 (CSP wasm-unsafe-eval)**: confirmed with versions: Chrome/Edge 97, Firefox 102, Safari/iOS 16.0.
- **A22 (CLI must match crate exactly)**: confirmed (0.2.128 release note). Current 0.2.130.
- **A25 (wasm exception handling in current browsers)**: confirmed: legacy EH Chrome 95/Firefox 100/Safari 15.2; exnref Chrome 137/Firefox 131/Safari 18.4 (Safari exnref has a reported JSC edge-case bug: medium).
- **A26 (KeyboardEvent.code maps to USB HID == SDL scancodes)**: changed. The UI Events spec names codes after HID but does not give numeric mapping, and notes oddities (Backslash vs IntlBackslash, Mac Meta, NumLock). Expected mapping for the shipped profiles (SDL scancode == HID usage, from the repo's pin_input profiles, read-only): 

| scancode | code | profile |
|---|---|---|
| 225/229 | ShiftLeft / ShiftRight | generic left/right flipper |
| 224/228 | ControlLeft / ControlRight | upper flippers |
| 44 | Space | launch |
| 40 | Enter | launch |
| 88 | NumpadEnter | launch |
| 30 | Digit1 | start |
| 4 | KeyA | action (generic), right_upper (xarcade) |
| 23 | KeyT | tilt (generic) |
| 6 | KeyC | xarcade left flipper |
| 34 | Digit5 | xarcade right flipper |
| 226 | AltLeft | xarcade action |
| 22 | KeyS | xarcade tilt |
| 80/79/82 | ArrowLeft/ArrowRight/ArrowUp | xarcade nudge |
| 92/94/96 | Numpad4/Numpad6/Numpad8 | xarcade nudge |

Needs a real-browser check (S6 follow-up): NumpadEnter and Numpad* with NumLock state, Digit1 vs AZERTY (code should stay Digit1), AltLeft on Windows (menu focus) and macOS (Option), ControlLeft combos with browser shortcuts, ShiftRight on Mac, Space/arrow page scroll (preventDefault), `event.repeat`, IME composition. Table above is theory only.

## Final fact table

| fact | value | source URL | date | confidence |
|---|---|---|---|---|
| caniuse WebGPU global | 88.63% | https://caniuse.com/webgpu | 2026-11-20 | medium (overcounts blockers, undercounts Firefox) |
| caniuse WebGL2 global | 97.29% | https://caniuse.com/webgl2 | 2026-11-20 | high |
| WebGL2-not-WebGPU gain | >= 8.66 pp | derived | 2026-11-20 | medium |
| Chrome/Edge WebGPU | 113 | https://github.com/gpuweb/gpuweb/wiki/Implementation-Status | 2026-11-20 | high |
| Chrome Android | 121, Android 12+ | same | 2026-11-20 | high |
| Chrome Linux | 144 Intel Gen12+, 147 NVIDIA Wayland | same | 2026-11-20 | medium |
| Firefox Win/mac | 141 / 145-147 | same; web.dev | 2026-11-20 | high |
| Firefox Linux/Android | not stable | same | 2026-11-20 | medium |
| Safari | 26.0 (all Apple OS) | https://webkit.org/blog/17333/webkit-features-in-safari-26-0/ | 2026-11-20 | high |
| StatCounter desktop Chromium | 83.88% | https://gs.statcounter.com/browser-market-share/desktop/worldwide | 2026-11-20 | high |
| StatCounter mobile Chrome/Safari | 70.37 / 21.91 | https://gs.statcounter.com/browser-market-share/mobile/worldwide | 2026-11-20 | high |
| Steam OS | Win11 72.51, Win10 21.11, Linux 3.95, macOS 2.35 | https://store.steampowered.com/hwsurvey/ | 2026-11-20 | high |
| wgpu | 30.0.2 (2026-11-12) | https://raw.githubusercontent.com/gfx-rs/wgpu/trunk/CHANGELOG.md | 2026-11-20 | high |
| WebGL2 downlevel limits | compute/storage 0, 2048 tex | https://docs.rs/wgpu/latest/wgpu/struct.Limits.html | 2026-11-20 | high |
| wasm-bindgen | 0.2.130; CLI must match | https://github.com/rustwasm/wasm-bindgen/releases | 2026-11-20 | high |
| Rust wasm defaults | multivalue, reftypes, bulk-mem, nontrapping, sign-ext, mutable-globals | https://doc.rust-lang.org/rustc/platform-support/wasm32-unknown-unknown.html | 2026-11-20 | high |
| wasm EH/exnref | Chrome 95/137, FF 100/131, Safari 15.2/18.4 | https://webassembly.org/features/ | 2026-11-20 | medium-high |
| SIMD / threads | Safari 16.4 / 14.1 | https://caniuse.com/wasm-simd | 2026-11-20 | high |
| wasm-unsafe-eval | Chrome 97, FF 102, Safari 16 | https://caniuse.com/mdn-http_headers_content-security-policy_script-src_wasm-unsafe-eval | 2026-11-20 | high |
| COEP credentialless | not Safari | https://caniuse.com/mdn-http_headers_cross-origin-embedder-policy_credentialless | 2026-11-20 | high |
| localhost secure context | yes | https://developer.mozilla.org/en-US/docs/Web/Security/Defenses/Secure_Contexts | 2026-11-20 | high |
| performance.now | 100 us / 5 us isolated | https://developer.mozilla.org/en-US/docs/Web/API/Performance/now | 2026-11-20 | high (spec-level), low per-browser |
| intensive throttling | hidden >5 min, 1/min, audio exempt | https://developer.chrome.com/blog/timer-throttling-in-chrome-88 | 2026-11-20 | high |
| Vibration | not iOS, not Firefox >=129 | https://caniuse.com/vibration | 2026-11-20 | high |
| vibrationActuator | not Firefox/iOS | https://caniuse.com/mdn-api_gamepad_vibrationactuator | 2026-11-20 | high |
| Keyboard.lock | Chromium only | https://caniuse.com/mdn-api_keyboard_lock | 2026-11-20 | high |
| Pointer lock mobile | none (iOS/Chrome Android) | https://caniuse.com/pointerlock | 2026-11-20 | high |
| Screen orientation lock | not iOS | https://caniuse.com/mdn-api_screenorientation_lock | 2026-11-20 | high |
| Wake Lock | iOS 16.4+ | https://caniuse.com/wake-lock | 2026-11-20 | high |
| AudioWorklet | Safari 14.1, iOS 14.5 | https://caniuse.com/mdn-api_audioworklet | 2026-11-20 | high |
| iOS memory cap | ~2 GB iPhone (one forum post) | https://developer.apple.com/forums/thread/823061 | 2026-11-20 | low |
| trunk / binaryen | 0.22.2 / version_133 | https://github.com/trunk-rs/trunk/releases ; https://github.com/WebAssembly/binaryen/releases | 2026-11-20 | high |
| Pages cannot set headers | yes | https://github.com/orgs/community/discussions/13309 | 2026-11-20 | medium |

## Not found / queries tried
WebView WebGPU (caniuse, gpuweb wiki); Android per-tab memory (search not run); Safari performance.now precision (1 search); output latency per OS (1 search, anecdotal only); mobile touch/pointer events, iOS file input, drag-drop, thermal throttling, visibilitychange mobile (not run); renderer site isolation (not run); Chrome current sync wasm compile limit (conflicting); wasm-bindgen prebuilt CLI binaries (not found); Steam vendor shares/DX12/Vulkan (page summary did not include).

---

# Follow-up (retrieved 2026-11-20, after the coordinator's request)

Scope change recorded: the owner DECIDED mobile is in scope (not best-effort). The first-pass wording "best-effort, not a release gate" in section 8 (Q7) is **superseded** by this section. Verdict tags: confirmed / refuted / still-unknown. Original text above is unchanged.

## F1. Mobile floor and what the implementation must do

### Concrete mobile floor (from sourced WebGPU facts)
- **iPhone/iPad: iOS/iPadOS 26 or newer**, in Safari or any other iOS browser (WebKit based). Sources: https://webkit.org/blog/17333/webkit-features-in-safari-26-0/ (WebGPU ships on iOS/iPadOS 26), https://caniuse.com/webgpu (iOS Safari 26.0; disabled by default 17.4-18.7). Chrome/Firefox for iOS: **confirmed-by-inference only** (they use WebKit, which is general knowledge; the fetched pages do not state it; see F1.7).
- **Android: Chrome 121 or newer on Android 12 or newer, with a Qualcomm, ARM or Intel GPU**; Imagination GPUs need Android 16+ (Chrome 139); Samsung Xclipse (Exynos Galaxy phones) and other GPUs are "TBD" on the wiki, so **treat those phones as unsupported** unless a test proves otherwise (https://github.com/gpuweb/gpuweb/wiki/Implementation-Status). Chrome 121 announcement: https://developer.chrome.com/blog/new-in-webgpu-121.
- **Not supported**: Firefox for Android (WebGPU disabled by default), Samsung Internet on unsupported GPUs, Android 11 and older, iOS 25 and older.
- Option for older Android: Chrome "compatibility mode" (`requestAdapter({featureLevel:"compatibility"})`, WebGPU on OpenGL ES 3.1, shipped in Chrome 146, Android first) (https://developer.chrome.com/blog/new-in-webgpu-146). Whether wgpu 30 exposes this feature level and whether the engine's shaders fit its limits are **still-unknown**; not recommended for the first cut.
- Wasm baseline on those floors is not a constraint: iOS 26 / Chrome 121 exceed every wasm feature in the section 6 table except memory64 (Safari TP only) and relaxed SIMD (Safari flag only), so do not use those two.

### F1.1 Touch / pointer controls
- Use **Pointer Events** (one handler set; `pointerId` per finger). `touch-action: none` on the canvas **and on its ancestors up to the first scroller**; the browser intersects `touch-action` of the element and its ancestors, and changes after a gesture starts do not apply. Without it the browser takes panning/pinch and sends `pointercancel` (https://developer.mozilla.org/en-US/docs/Web/CSS/touch-action, https://developer.mozilla.org/en-US/docs/Web/API/Pointer_events/Multi-touch_interaction). **Confirmed.**
- Treat `pointercancel` like `pointerup` (release the flipper) or a held flipper sticks (MDN multi-touch page). **Confirmed.**
- iOS quirks: WebKit bug 240917 (2022) says `pointercancel` is not dispatched for scrolling on `touch-action: manipulation` containers (https://bugs.webkit.org/show_bug.cgi?id=240917); fix status **still-unknown**, so use `none`, not `manipulation`. A Babylon.js report (iPadOS 13.2, old) needed `touch-action:none` on `<body>` as well as the canvas to stop the "zoom out to tab manager" gesture (https://forum.babylonjs.com/t/univeral-camera-touch-controls-break-on-ipados-safari-13-2/7162): **still-unknown on iOS 26**; set it on body and html too. A May 2025 W3C pointer-events list post reports pinch-zoom resuming on iPhone Safari when two fingers are down again (secondhand, https://lists.w3.org/Archives/Public/public-pointer-events/2025AprJun/0035.html): **needs a real-device test**.
- iOS 350 ms single-tap delay (double-tap zoom) is removed by `width=device-width`, `user-scalable=no`, or `touch-action: manipulation` on the element/ancestors (https://webkit.org/blog/5610/more-responsive-tapping-on-ios/). Taps are not the main input here (flippers want touchstart latency), but menu buttons need it. **Confirmed.** Whether iOS 26 honours `user-scalable=no` against pinch zoom: **still-unknown** (page does not say).
- Max simultaneous touches and `maxTouchPoints` values: **still-unknown** (the spike should print `navigator.maxTouchPoints` on devices; two flippers + plunger/nudge need >= 3).
- Fullscreen: iPhone Safari fullscreen API is only partial (caniuse); iOS 26 opens every Home Screen site as a standalone web app by default, with no manifest required (https://webkit.org/blog/17333/webkit-features-in-safari-26-0/). **Recommendation: tell iPhone users to Add to Home Screen** for a chrome-less play surface; also helps audio autoplay on Chrome (installed/home-screen exempt per https://developer.chrome.com/blog/autoplay).
- System edge-swipe gestures cannot be blocked from the web: place flipper zones away from the bottom edge and left/right edges (design guidance, not sourced).

### F1.2 File picker for a bundle (.zip / .pinbundle)
- HTML spec grammar allows `accept=".pinbundle"` but MDN says `accept` is only a hint (https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Attributes/accept).
- iOS Safari: WebKit bug threads report that **extension tokens are not honoured on iOS, only MIME types**; `accept=".custom"` lets any file through, while `accept=".custom,.json"` shows only .json and greys out .custom; extension-only lists can grey out everything in the Files app (https://bugs.webkit.org/show_bug.cgi?id=279606 [2024], https://bugs.webkit.org/show_bug.cgi?id=190982, https://forums.sabnzbd.org/viewtopic.php?p=128651). Current iOS 26 behaviour: **still-unknown** (reports are old, mostly 2018-2024).
- **Recommendation (firm):** `<input type="file">` with **no `accept`** (or `accept="application/zip"` only if a test on iOS 26 proves it works), validate by magic bytes (`PK\x03\x04`) in code, and name the bundle `.zip` for mobile users. Do not register a custom extension for mobile.
- Drag-and-drop: caniuse shows HTML5 DnD in iOS Safari 15+ and Chrome Android, but not Firefox Android/Samsung (https://caniuse.com/dragndrop). Dropping files from the OS onto a phone browser as a workflow: **still-unknown/not documented**; use the file picker as the only mobile path and drag-drop as a desktop extra.

### F1.3 Memory per tab (sourced figures, or "none exist")
- **No official figure exists for iOS or Android.** What the public record gives:
  - iOS WebContent: WebKit engineer says a "soft" limit around **1.5 GB** on iPhone, above which jetsam may kill it, but not if system memory is plentiful (quoted in the results for https://bugs.webkit.org/show_bug.cgi?id=221530); Keith Miller (same bug): the limit is system-pressure based, not fixed; `memory.grow` may not fail, the tab just reloads. Other reports range from ~200-300 MB reliably (https://bugs.webkit.org/show_bug.cgi?id=269777, 2024) to ~3 GB on iPhone 15 Pro. A 2026 forum post reports a WebContent/GPU process crash on iPhone 15 with iOS 26.4, stable on iPad (https://developer.apple.com/forums/thread/822200; single report). iOS 26 added a wasm in-place interpreter that lowers startup memory (WebKit 26.0 post).
  - Android Chrome: no official number. Reports: ~300 MB reliable (https://github.com/WebAssembly/design/issues/1397), 256 MB wall on a Galaxy S20+ with Chrome while Firefox/Edge reached ~1.9 GB (https://discussions.unity.com/t/android-chromium-unable-to-grow-allocated-memory-above-256mb-confirmed/818820), attributed to a 32-bit renderer; **dated (2020-2022); whether current Chrome Android renderers are 64-bit is still-unknown**.
- **Implementation requirement:** budget the whole page (wasm linear memory, Lua arena, textures, audio buffers) to a hard cap of **about 300 MB working set on mobile** until measured on real devices; set `WebAssembly.Memory({initial, maximum})` explicitly so growth beyond the cap fails inside the sandbox; wrap `new WebAssembly.Memory`/instantiate in try/catch and show a "not enough memory" screen; reuse the Lua/Wasm memory caps already owed (these also bound hostile bundles). Verdict on "iOS per-tab limit": **still-unknown, no sourced figure**; 300 MB budget is my recommendation, not a source.

### F1.4 Thermal throttling
- No browser thermal API. Chrome's Compute Pressure API (`PressureObserver`, Chrome 125, CPU source only) reports pressure states fair/serious/critical (https://developer.chrome.com/docs/web-platform/compute-pressure, https://developer.mozilla.org/docs/Web/API/Compute_Pressure_API); Safari/Firefox support not retrieved: **still-unknown**, assume absent. No sourced frame-rate cap.
- **Requirement:** render at the display rate but cap render work (resolution scale, effects) adaptively from measured frame times; the 1 kHz sim cost is S1's question; pick quality tiers; offer a 30/60 fps cap; rAF follows 60/120 Hz displays (MDN rAF page), so step the sim with a fixed-step accumulator and never tie it to frame count.

### F1.5 visibilitychange / pagehide on mobile
- `visibilitychange` fires on tab switch, minimise, and (mobile) switching to another app; `hidden` is "the last event that is reliably observable" (https://developer.mozilla.org/en-US/docs/Web/API/Document/visibilitychange_event). Closing from the tab switcher/app switcher often fires **no** `beforeunload`/`pagehide`/`unload`; frozen pages suspend timers; discarded pages run nothing; Chrome's Page Lifecycle API is not in Safari (https://developer.chrome.com/docs/web-platform/page-lifecycle-api). **Confirmed.**
- **Requirement:** on `hidden`: stop the sim loop, `audioContext.suspend()`, persist anything worth keeping (settings, last-used table) to IndexedDB/localStorage; on `visible`: resume, re-request Wake Lock if used (Wake Lock re-acquire after hidden: from the spec, not fetched: **unverified**). Never rely on `pagehide`/`unload` for saving. Workers under freezing: unspecified in the article: **still-unknown**.

### F1.6 Touch-related platform gaps (restated with sources)
No pointer lock on iOS/Chrome Android (https://caniuse.com/pointerlock); no `navigator.vibrate` on iOS (https://caniuse.com/vibration), so haptics are Android-only; no screen-orientation lock on iOS (https://caniuse.com/mdn-api_screenorientation_lock), so the UI must work in both orientations or show a "rotate your device" overlay; wake lock OK on iOS 16.4+ (https://caniuse.com/wake-lock). **Confirmed** (unchanged from section 4).

### F1.7 WebGPU in WebViews and Chrome for iOS
- Chrome/Firefox for iOS: not addressed by the WebKit post or the wiki; they are WebKit based on iOS (outside the EU alternative-engine regime): **inference, not sourced here**. They should follow the iOS version (26+).
- WKWebView: Apple forum reply says Safari feature flags do not apply to WKWebView; WebGPU is on by default in WKWebView on iOS 26 (https://developer.apple.com/forums/thread/770862; a separate thread says "WebGPU for WebView on iOS not supported" https://developer.apple.com/forums/thread/781602, older): **likely yes on iOS 26, not directly confirmed**.
- Android WebView: two searches found nothing: **still-unknown**. Do not promise in-app-browser support.

## F2. performance.now() precision
- **Firefox: confirmed.** Default (non-RFP): **1 ms plus jitter**; for COOP/COEP cross-origin-isolated documents it clamps to **20 ns, no jitter** (quote from Mozilla bug 1692609 "RTP - the default for Firefox: 1ms + jitter" and "Unconditional: this clamps to 20 ns ... COOP/COEP documents": https://bugzilla.mozilla.org/show_bug.cgi?id=1692609). MDN/spec say 5 us isolated, 100 us otherwise; Firefox's real value is 1 ms non-isolated and finer than that when isolated. Older 20 us/2 ms figures are historical.
- **Safari: partly resolved.** 2016 WebKit bug proposed truncating to 100 us (https://bugs.webkit.org/show_bug.cgi?id=165503, historical); a 2026 WebKit bug comment states "1ms timer precision for performance.now()" (https://bugs.webkit.org/show_bug.cgi?id=307571, comment by a reporter, not WebKit documentation). Isolated Safari value: **still-unknown** (isolation tier per spec is 5 us). Treat Safari as 1 ms until measured.
- Chrome: 100 us non-isolated, 5 us isolated (MDN). 
- **Implication:** do not use `performance.now()` deltas finer than 1 ms for sim pacing; use the `requestAnimationFrame` timestamp and `AudioContext.currentTime` for clocks; input `event.timeStamp` shares the timeline, so input latency measurement is only to ~1 ms on Firefox/Safari without isolation. **Verdict A15: confirmed with per-browser values above.**

## F3. Worker timers, rAF and postMessage in hidden tabs
- **rAF**: paused in hidden tabs in most browsers (MDN rAF page). **Confirmed.**
- **Main-thread timers**: Chrome intensive throttling after 5 min hidden, once per minute (https://developer.chrome.com/blog/timer-throttling-in-chrome-88). **Confirmed.**
- **Dedicated Worker timers: NOT exempt, in general.**
  - Chrome: a Blink intent (2018) extended background throttling to dedicated workers, rate and shipped state not shown (https://groups.google.com/a/chromium.org/d/topic/blink-dev/8xpEA-iNHwg). Whether it shipped: **still-unknown**.
  - Firefox: worker `setTimeout/setInterval` throttled to at most 1/s except while the tab plays audio; implemented behind `dom.workers.throttling.enabled`, on in Nightly and Beta, release enablement bug 2011100 REOPENED; bug 1774542 "timers not throttled in workers of background tabs" still NEW (https://bugzilla.mozilla.org/show_bug.cgi?id=1818463). So **not in Release as of this snapshot, but coming**.
  - Safari: no public source: **still-unknown**.
  - `postMessage` throttling in hidden tabs: **no source found: still-unknown**.
- **Implication (firm):** a 1 kHz sim in a hidden tab cannot be relied on in any browser; the engine must pause on `hidden` (also required by mobile lifecycle) and must not use `setTimeout(1)` as its clock in the foreground either; in the foreground drive the sim from rAF (or an audio-driven Worker via AudioWorklet message/SAB) with a fixed-step accumulator. A silent audio track does not count as audio for Chrome throttling exemption.
- **Verdict A16: confirmed for rAF and main-thread timers; Worker half refuted as an exemption (workers are throttle candidates), exact per-browser behaviour still-unknown.**

## F4. Tooling status
- **wasm-bindgen CLI prebuilt binaries: confirmed.** Release 0.2.130 assets (https://github.com/wasm-bindgen/wasm-bindgen/releases/expanded_assets/0.2.130, which I fetched): `wasm-bindgen-0.2.130-<target>.tar.gz` plus `.sha256sum` for aarch64-apple-darwin, aarch64-unknown-linux-gnu, aarch64-unknown-linux-musl, riscv64gc-unknown-linux-gnu, x86_64-apple-darwin, x86_64-pc-windows-msvc, x86_64-unknown-linux-musl (14 files). **No x86_64-unknown-linux-gnu asset**; use the x86_64 musl build on Linux GitHub runners. Crate metadata also supports `cargo binstall wasm-bindgen-cli` (search result summary of docs.rs `wasm-bindgen-cli` Cargo.toml; the `pkg-url` moved from the rustwasm org to the wasm-bindgen org between 0.2.96 and 0.2.113). Pin: download the tarball whose version equals the crate version in Cargo.lock.
- **wasm-pack: corrected.** The rustwasm org and Working Group were sunset (Rust blog, 2025-07-21: wasm-pack, gloo, twiggy, walrus etc. "archived in place or transferred", no named maintainers; wasm-bindgen moved to a new neutral org with new maintainers: https://blog.rust-lang.org/inside-rust/2025/07/21/sunsetting-the-rustwasm-github-org). My first-pass note "maintained by drager" is **refuted**: github.com/drager/wasm-pack README says "This repository has moved to wasm-bindgen/wasm-pack" with docs at https://wasm-bindgen.github.io/wasm-pack/. Release/commit activity there: **still-unknown** (page fetch shows no release dates; `gh` was not enabled for these repos in this session). Recommendation: do not depend on wasm-pack; run `cargo build --target wasm32-unknown-unknown` + `wasm-bindgen` CLI + `wasm-opt` directly (the pipeline we need is simple and the engine needs custom Worker/SAB glue anyway).
- **trunk: confirmed active.** github.com/trunk-rs/trunk not archived, 1,060 commits, 119 open issues/19 PRs on the page; v0.22.2 released 2026-11-18 per the releases page (https://github.com/trunk-rs/trunk/releases). Auto-download of wasm-bindgen/wasm-opt: **still-unknown** (README excerpt silent; release note "Retry to download tools on failure" implies it downloads tools). Recommendation unchanged: not needed.
- binaryen version_133 (2026-09-21), prebuilt x86_64-linux and arm64-macos (unchanged from section 7).

## F5. Audio latency figures
- **No per-OS/browser measured table exists in public sources.** Sourced pieces only:
  - `latencyHint` defaults to `"interactive"`; MDN: browser may not honour it, read `baseLatency` after construction (https://developer.mozilla.org/en-US/docs/Web/API/AudioContext/AudioContext). MDN example prints `0.00` for interactive and `0.15` for playback.
  - A 2021 Chromium intent-to-ship thread lists expected buffer sizes: **macOS 128 frames, Windows 10 ms, Android 96 frames**, and a Firefox/macOS test shows outputLatency ~0.0059 s and ~0.0131 s depending on the device (https://groups.google.com/a/chromium.org/g/blink-dev/c/dTQniJNVVMY). Mailing-list figures, not measurements of the full chain; medium-low.
  - `outputLatency`: Chrome/Edge 102, Firefox 70, Safari/iOS 18.4 (https://web.dev/articles/audio-output-latency, https://caniuse.com/mdn-api_audiocontext_outputlatency); `baseLatency`: Chrome 58, Safari 14.1 (https://caniuse.com/mdn-api_audiocontext_baselatency). Bluetooth adds device latency (web.dev), so wired/speakers differ.
  - iOS 15.1 output timestamp behaved differently from Chrome (https://developer.apple.com/forums/thread/696356, old).
- **Verdict A18's "10 to 40 ms typical": still-unknown (plausible for desktop wired, unproven for iOS/Android/Bluetooth).** Requirement: the spike must measure with a loopback (click out, mic in) on one iPhone, one Android and one desktop, and log `baseLatency`/`outputLatency`; design audio-visual sync so that flipper sound is triggered from the input handler immediately (no sim-quantum wait).

## F6. Renderer site isolation (A11, second half)
- **Chrome desktop: confirmed.** Site Isolation default for all sites on Windows/Mac/Linux/ChromeOS since Chrome 67 (https://www.chromium.org/Home/chromium-security/site-isolation/).
- **Chrome Android: partial.** Only devices with >= 2 GB RAM, and only for sites with logins/passwords (Chrome 77), third-party login providers and COOP-header sites (Chrome 92); not in Android WebView (same page and the 9to5google/androidauthority 2019 reports in the search results).
- **Firefox desktop: confirmed.** Fission released to desktop in Firefox 95 (https://wiki.mozilla.org/Project_Fission, https://blog.mozilla.org/security/2021/05/18/introducing-site-isolation-in-firefox/). **Firefox Android: not on in Release** (architecture from 147, isolation Nightly-only/partial and turned off in 147.0.2; Mozilla Connect/support threads, low-medium confidence).
- **Safari/WebKit: still-unknown.** WebKit has a Site Isolation project in active development with 2026 bugs (https://docs.webkit.org/Deep%20Dive/SiteIsolation.html, WebKit bugs 306348/307816); default-on status in shipping iOS 26 is **not confirmed**. Safari has long used separate WebContent processes per tab, but that is general knowledge, not sourced here.
- **Meaning for the design:** the owner's concern (untrusted bundle) is not solved by browser site isolation: a bundle runs inside our own origin's renderer. The relevant guarantees are the in-page ones: one Wasm instance and Lua arena per Worker, `WebAssembly.Memory` maximum, fuel/instruction budget, no `eval`/network from the bundle (CSP `connect-src 'none'` in the page that hosts it; CSP needs `'wasm-unsafe-eval'` only), parse the zip in a Worker with size/entry limits. **A11 verdict: wasmtime-in-browser half: partly confirmed (unchanged); per-site isolation half: confirmed for Chrome/Firefox desktop, partial/absent on mobile Chrome/Firefox Android, unknown for Safari.**

## F7. Updated verdicts (delta)
| Item | Was | Now |
|---|---|---|
| A11 isolation half | unverified | confirmed Chrome/Firefox desktop; partial Chrome Android; absent Firefox Android Release; Safari unknown (F6) |
| A15 | confirmed, per-browser unknown | Firefox 1 ms + jitter (20 ns isolated); Safari ~1 ms reported (low); Chrome 100/5 us (F2) |
| A16 | workers unverified | workers are throttle candidates (Firefox bug 1818463; Chrome intent), no exemption; hidden-tab 1 kHz not reliable (F3) |
| A18 latency | not confirmed | still-unknown, only expected buffer sizes (F5) |
| wasm-pack status | "maintained by drager" | refuted: moved to wasm-bindgen/wasm-pack after rustwasm sunset (F4) |
| wasm-bindgen prebuilt CLI | not confirmed | confirmed: 7 targets (no x86_64 glibc; use musl) (F4) |
| Q7 | best-effort | superseded: mobile floor iOS 26 / Chrome 121 + Android 12 + Qualcomm/ARM/Intel GPU, memory cap ~300 MB (F1) |

## F8. Still unresolved after this pass
Safari isolated `performance.now`; Safari/Chrome worker timer throttling rates; `postMessage` in hidden tabs; Android WebView WebGPU; Chrome for Android renderer bitness/memory today; iOS 26 file-picker `accept` behaviour; `maxTouchPoints` and iOS multi-touch limits; Safari site isolation default; measured audio latency per device; wasm-pack/trunk release cadence. All need real-device tests (iPhone iOS 26, a Pixel/Galaxy-with-Snapdragon Android 12+) rather than further web research.

### Added fact table rows
| fact | value | source URL | date | confidence |
|---|---|---|---|---|
| Firefox performance.now default | 1 ms + jitter; isolated 20 ns | https://bugzilla.mozilla.org/show_bug.cgi?id=1692609 | 2026-11-20 | medium-high |
| Safari performance.now | ~1 ms (reporter comment) | https://bugs.webkit.org/show_bug.cgi?id=307571 | 2026-11-20 | low |
| Firefox worker timer throttling | <= 1/s, release enablement reopened | https://bugzilla.mozilla.org/show_bug.cgi?id=1818463 | 2026-11-20 | medium |
| wasm-bindgen 0.2.130 binaries | 7 targets, musl x86_64 only for Linux x64 | https://github.com/wasm-bindgen/wasm-bindgen/releases/expanded_assets/0.2.130 | 2026-11-20 | high |
| wasm-pack | moved to wasm-bindgen/wasm-pack | https://github.com/drager/wasm-pack | 2026-11-20 | medium |
| rustwasm sunset | org archived Sep 2025 | https://blog.rust-lang.org/inside-rust/2025/07/21/sunsetting-the-rustwasm-github-org | 2026-11-20 | high |
| iOS WebContent soft limit | ~1.5 GB (WebKit engineer, quoted) | https://bugs.webkit.org/show_bug.cgi?id=221530 | 2026-11-20 | low-medium |
| Android Chrome wasm | ~256-300 MB reports (old) | https://github.com/WebAssembly/design/issues/1397 | 2026-11-20 | low |
| Chrome site isolation | desktop all (67); Android partial >= 2 GB | https://www.chromium.org/Home/chromium-security/site-isolation/ | 2026-11-20 | high |
| Firefox Fission | desktop 95; Android off | https://wiki.mozilla.org/Project_Fission | 2026-11-20 | medium |
| iOS file input accept | extensions ignored (old reports) | https://bugs.webkit.org/show_bug.cgi?id=279606 | 2026-11-20 | low-medium |
| touch-action | `none` stops pan/zoom; pointercancel otherwise | https://developer.mozilla.org/en-US/docs/Web/CSS/touch-action | 2026-11-20 | high |
| Page lifecycle | hidden is last reliable event; no pagehide when app-switcher closed | https://developer.chrome.com/docs/web-platform/page-lifecycle-api | 2026-11-20 | high |
| Audio buffers (expected) | mac 128 fr, Win 10 ms, Android 96 fr | https://groups.google.com/a/chromium.org/g/blink-dev/c/dTQniJNVVMY | 2026-11-20 | low-medium |
