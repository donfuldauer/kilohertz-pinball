# S1 — web-spike-sim: sim determinism and wasm build (RESULTS)

Base: `main` at `995935f91ad8ad18fcee201b94df52c5aaf1fb6c` (`BASE_COMMIT.txt`). Toolchain rustc 1.97.0,
targets `wasm32-unknown-unknown` / `wasm32-wasip1`; Node v22.23.5; Playwright Chromium 157.0.8103.78;
wasm-bindgen crate **and** CLI `=0.2.130` (A22 pin; CLI installed with
`cargo install --root <scratch>/tools wasm-bindgen-cli --version 0.2.130 --locked`); wasm-tools 1.263.0
(`cargo install ... wasm-tools --version 1.263.0 --locked`); binaryen/wasm-opt 132 (npm `binaryen@132.0.0`,
`--ignore-scripts`). Every cargo command used the COMMON env (`env.sh`: `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s1[/<flavour>]`, `CARGO_INCREMENTAL=0`,
`CARGO_PROFILE_{DEV,TEST}_DEBUG=line-tables-only`). Scratch root below = `S` =
`${WEB_SPIKES}/S1`.

## 0. Bottom line

* **Bit-identical native↔web is achieved, with no change to native and no change to any sim source:**
  build the web module with `-C target-feature=+simd128`. Then every tick's `state_hash` **and** a
  stronger supplementary `ext_hash` are identical to native x86_64 for all 3 production tables ×
  3 scenario kinds (60 s bot games, 60 s input-pattern runs, 20 s 8-ball multiball), a 10-minute
  (600 000-tick) soak, and the repo's own CI fixture (`state_hash=25097c21c2c3adc1`, the value documented
  in `docs/decisions/2026-11-19-w8g-e.md`) — on three wasm routes: wasip1 under `node:wasi`,
  wasm-bindgen under Node, and wasm-bindgen in Chromium (isolated and not). 40/40 PASS lines.
* Without `simd128` (rustc's default wasm32 features) the web diverges at the **first tick a ball
  rotates** (tick 6–86), by **1 ULP in the ball orientation quaternion** (e.g. `w` `0x3f7ffde2` native vs
  `0x3f7ffde3` wasm); positions, velocities, switches, scores and the script's inputs stay identical.
  Root cause (proven by bisection): glam 0.33.12 picks its SSE2 code on x86_64 and its *scalar* code on
  wasm32 without simd128; `Quat::mul` and the 4-lane dot in `Quat::normalize` associate differently
  (`(x+z)+(y+w)` vs `((x+y)+z)+w`), used every tick in `pin_physics::ball::contact::integrate_orientation`.
  glam's wasm `simd128` code mirrors its SSE2 code op-for-op, so `+simd128` restores exact parity.
* The alternative (option a: glam `scalar-math` on every target) also gives parity but changes native
  sim output for every table (documented fixture hash → `77b7fda16bae55d2`, all 5 `ball_seam_bitident`
  EXPECTED digests) — very-high blast radius under D34, for no benefit over `+simd128`. Not recommended.
* Wasm tick cost is **1.1–1.6× native** (Node and Chromium alike; A27 confirmed): 5.5–7.7 µs/tick with 1–3
  balls, 66–69 µs worst (Sky Deck, 6.4 balls avg/9 max); a 60 Hz frame's 16-tick batch costs p99 0.15–0.28 ms
  (1–3 balls) and ≤ 2.2 ms (9-ball stress). **Run the sim on the main thread** (rAF batch), not a Worker.
* `step_1ms` performs **0 heap allocations** on wasm (D5 guard works unchanged), `catch_unwind` cannot
  catch on wasm32 (stable has no `panic=unwind`), the shadow stack is 1 MiB, stack-first (overflow
  traps), raised with `-C link-arg=-zstack-size=N`; the sim needs ≤ ~450 KiB at load, ~235 KiB while ticking.

## 1. Owned assumptions — verdicts

| # | Verdict | Evidence (pointer) |
|---|---|---|
| A6 | **Confirmed, with two corrections** | wasm32-unknown-unknown (§7.1, `a6/bindgen_probes*.txt`): `Instant::now()`, `SystemTime::now()` and `thread::sleep` **panic** ("time not implemented on this platform", "can't sleep"); `thread::Builder::spawn` returns `Err(Unsupported)`, `thread::spawn` panics; `fs::read/write/canonicalize`, `current_dir`, `TcpStream::connect`, `UdpSocket::bind` compile and return `Err(Unsupported)`; `Mutex`/`RwLock`/`OnceLock`/atomics work single-threaded. Corrections: (1) `std` `RandomState` keys are **fixed** on wasm32-unknown-unknown (same hash in every process; random natively) — harmless for the sim (parity holds), but HashMaps are not HashDoS-resistant on the web; (2) on **wasip1** time, sleep and fs (with preopens) *work*; only threads and sockets fail (`a6/wasip1_probes.txt`). The sim crates have no wall-clock/thread/fs use on the tick path; the CLI/soak/folder-VFS ones compile and are simply not called. |
| A7 | **Confirmed** | `rustc --print cfg` gives `panic="abort"` for both wasm targets (`a12/cfg_*.txt`); `-C panic=unwind` on stable fails: "the crate `panic_unwind` does not have the panic strategy `unwind`" (`logs/a7_panic_unwind.log`, needs nightly `-Zbuild-std`). A panic inside `step_1ms` under `catch_unwind`: native catches it ("panic caught, process alive"), wasm traps `RuntimeError: unreachable` (wasip1 and wasm-bindgen). Without a hook the message is lost; with `std::panic::set_hook` → `console.error` it is printed. After a trap the instance still answers simple calls, but every trap leaks shadow stack (`__stack_pointer` is not restored): after 2000 traps all calls fail with "memory access out of bounds" (§7.2, `a7/a7_results.txt`). |
| A12 | **Confirmed** (+ one NaN finding) | Default wasm32 target features: `bulk-memory, multivalue, mutable-globals, nontrapping-fptoint, reference-types, sign-ext` — **no `simd128`, no `relaxed-simd`** (`a12/cfg_wasm32-*.txt`). Instruction scan of both modules: 0 `relaxed_*` ops; core wasm has no FMA opcode; the module validates with `--features=-relaxed-simd`. Native x86_64 default features `fxsr, sse, sse2` (no `fma`): the only FMA instructions in the native binaries are the two in `compiler_builtins::…::fma{,f}_with_fma` (explicit `fma` calls only; the sim makes none) — `a12/native_fma_*.txt`. NaNs: hardware-generated NaN bits are equal native vs V8-x86 (`ffc00000`), but `libm::sqrtf(-1.0)` gives `ffc00000` native and `7fc00000` on wasm (`a12/nan_and_sizes.txt`): NaN bits are platform-dependent, so a NaN that reached hashed state would break parity (none did in any run; balls with non-finite state are removed in the same tick). |
| A13 | **Confirmed, cause pinned, fix found** | glam 0.33.12 SSE2 (x86_64) vs scalar (wasm32 default) differ on every SIMD-backed type: per-op digests over 200 000 inputs differ for `Quat` mul/normalize/mul_vec3/from_rotation_arc, `Vec4` dot/length/normalize, `Mat4` mul/transform/inverse, and for `integrate_orientation`; `Vec3` ops and `libm` functions are identical everywhere (§6.2, `a13/glam_ops.txt`). With `+simd128` every op the sim uses is identical to native SSE2; only `Vec4::min/max` differ (±0/NaN tie rule: SSE `minps` vs wasm `f32x4.pmin`), which no sim crate uses. parry3d is used only for its BVH/AABB over `Vec3` (scalar everywhere); `wide`/`simba` and `num-traits` std math are not on the tick path. Proven by toggling: native+`scalar-math` == wasm default (all PASS), native == wasm+`simd128` (all PASS). |
| A14 | **Changed** | No configuration needed: getrandom 0.4.3 needs only its `wasm_js` **crate feature** (no `--cfg getrandom_backend` flag; `getrandom-0.4.3/src/backends.rs:170-178`), and `zip 9.5.1` already enables it for every `wasm32` target (`zip-9.5.1/Cargo.toml:303-306`); unmodified `pin_assets` builds for both wasm targets (`logs/build_unmodified_assets.log`, `cargo tree -i getrandom` in §3). Consequence: wasm-bindgen and js-sys are in the dependency graph of every wasm32-unknown-unknown build of pin_assets; the linked sim module imports nothing from getrandom/crypto (its 4 imports are the harness's `console.error`, `performance.now` and two wasm-bindgen intrinsics: §8 end; `wasm-tools print … | grep '(import'`). |
| A27 | **Confirmed (measured 1.1–1.6×)** | Steady mean per tick, playback backend, median of 3 runs: native 4.4–5.2 µs (1–3 balls), wasm-bindgen/Node 6.4–7.7 µs, Chromium 5.7–6.9 µs; 9-ball stress 7.8–62.5 µs native vs 11–69 µs wasm. Ratio 1.07–1.59 (release, opt-level 3), 1.3–2.0 for a size-optimised build (`opt-level=s` + `wasm-opt -Oz`). Full tables §8. "Streaming round trips" part of A27 not measured (S5/web). |
| A28 | **Confirmed** | Default shadow stack 1 MiB (`__stack_pointer` init `1048576`; SP at entry ≈ 1023 KiB), placed **first** in linear memory, so overflow wraps below 0 and **traps** ("memory access out of bounds"), it does not corrupt data. `recurse_big` (1 KiB/frame): 1000 deep OK, 1100 traps by default; after `-C link-arg=-zstack-size=8388608`: 7000 OK, 8300 traps (`a28/default.txt`, `a28/raised.txt`). Separately V8's own call-stack limit stops small-frame recursion between 10 000 and 20 000 frames (`RangeError: Maximum call stack size exceeded`). Measured sim needs: load 266 731 B and ticking 53 339 B below the caller, plus `Simulation` itself (61 552 B on wasm32) if held on the stack (§7.3). No recursion on the tick path: parry3d's BVH traversal is iterative with an explicit `SmallVec` stack (`parry3d-0.31.1/src/partitioning/bvh/bvh_traverse.rs:11,17,120`); the measured depth is constant across tables and scenarios (53 339 B), i.e. code-path-bound, not data-bound; the deepest frames are large by-value structs at load. |

Reproducing commands (after `. S/env.sh; cd S`; `B=S/runs/simd/pkg_node`; `W=runs/simd/s1_wasip1.wasm`; `T=/home/user/kilohertz-pinball/target/web_s1/release`):

| # | command(s) |
|---|---|
| A6 | `for p in instant systemtime sleep thread_builder thread_spawn fs_read fs_write fs_canonicalize net_tcp net_udp mutex atomics hashmap_keys env; do S1_PANIC_HOOK=1 node harness/run_bindgen.mjs $B probe $p; done` (and the same through `S1_DIR=S node --no-warnings harness/run_wasi.mjs $W probe $p`) |
| A7 | `rustc --print cfg --target wasm32-unknown-unknown \| grep panic`; `RUSTFLAGS="-C panic=unwind" cargo build --release --target wasm32-unknown-unknown --manifest-path a7_unwind/Cargo.toml`; `$T/s1 panic-in-step bundles/apex_circuit.pinbundle`; `node harness/run_bindgen.mjs $B panic-in-step bundles/apex_circuit.pinbundle [--hook]`; `node harness/run_bindgen.mjs $B panic-then-call bundles/apex_circuit.pinbundle` |
| A12 | `rustc --print cfg --target wasm32-unknown-unknown`; `wasm-tools print $B/s1_harness_bg.wasm \| grep -cE '^\s+[a-z0-9]+x[0-9]+\.relaxed_'`; `wasm-tools validate --features=-relaxed-simd $B/s1_harness_bg.wasm`; `objdump -d --no-show-raw-insn -C $T/s1 \| awk -F'\t' '$2 ~ /^v?f(n)?m(add\|sub)[0-9]+[sp][sd]/'`; `$T/s1 probe nan_bits` vs `node harness/run_bindgen.mjs $B probe nan_bits` |
| A13 | `S/run_parity.sh base` (FAIL + first tick), `S/run_parity.sh simd` (PASS), `S/run_parity.sh scalar` (PASS); `$T/s1 dump bundles/apex_circuit.pinbundle rec/apex_circuit_bot60.scn rec/apex_circuit_bot60.scr 86` vs the same via `run_wasi.mjs runs/base/s1_wasip1.wasm dump /s/…`; `$T/s1 probe glam_ops` vs `run_wasi.mjs runs/{simd,base}/s1_wasip1.wasm probe glam_ops` |
| A14 | `cd pristine && cargo build --release --target wasm32-unknown-unknown -p pin_assets`; `cargo tree -p pin_assets --target wasm32-unknown-unknown -e normal,features -i getrandom@0.4.3` |
| A27 | `harness/bench_all.sh` ×3 then `python3 -I harness/bench_median.py bench_r1 bench_r2 bench_r3`; `harness/chromium_bench.sh S/runs/variants/o3/pkg_web chromium/o3_iso1.json 1` (and `… 0`); sizes: `harness/build_size_variants.sh` then `gzip -9 -c <wasm> \| wc -c` |
| A28 | `node harness/run_bindgen.mjs $B recurse-big 1000\|1100`; `node harness/run_bindgen.mjs $B recurse-small 10000\|20000`; rebuild with `RUSTFLAGS="-C target-feature=+simd128 -C link-arg=-zstack-size=8388608"` and rerun `recurse-big 7000\|8300`; `node harness/run_bindgen.mjs $B stack-phases bundles/<t>.pinbundle rec/<scn>.scn rec/<scn>.scr` |

## 2. Layout and reproduction

* `S/repo` — scratch copy of `main` with patches 0001–0004 (sim gating + harness). `S/repo_scalar` — same
  plus patch 0005 (option a). `S/pristine` — untouched copy (patch base).
* `S/patches/*.patch` — `diff -u` against the real tree; each starts with a one-line rationale; all apply
  cleanly to `995935f` (`git -C /home/user/kilohertz-pinball apply --check S/patches/000N-*.patch`,
  0001–0004 together too).
* `S/run_parity.sh [simd|base|scalar]` — builds native + both wasm routes, records natively if needed,
  prints one PASS/FAIL line per scenario and route with the first diverging tick; exit 0 only if all PASS.
  `simd` is the recommended build (expected all PASS), `base` shows the divergence, `scalar` is option (a).
* Harness: `S/repo/crates/s1_harness` (spike tooling only, patch 0004): `s1` binary (native and wasip1) and
  wasm-bindgen exports; `S/harness/*.mjs|*.cjs|*.py|*.sh` drivers (Node WASI, Node bindgen, Chromium page
  `S/web/`, comparison, benchmarks, tests).
* Recordings (reusable by S2/S4): `S/rec/<scn>.scn` (scenario: bot ops / input events / spawns per tick),
  `S/rec/<scn>.scr` (script-output stream: per script step RNG draws, input digest, command queue),
  `S/rec/<scn>.khzrec` (bot scenarios, the repo's own GameRecording format), bundles `S/bundles/<table>/`
  and `S/bundles/<table>.pinbundle`. Native reference hashes: `S/native/<scn>.hashes`
  (`tick state_hash ext_hash`, every tick from 0).

## 3. R1 — building the sim crates for wasm32

Unmodified tree (`S/pristine`, `cargo build --release --target <t> -p <crate>`):

| crate | wasm32-unknown-unknown | wasm32-wasip1 |
|---|---|---|
| pin_core, pin_physics, pin_input | builds | builds |
| pin_assets | builds (getrandom 0.4.3 with `wasm_js` via zip; no flag) | builds |
| pin_sim | **fails**: `errno` "target OS is unknown" ← rustix ← wasmtime-internal-cache ← wasmtime ← pin_script | **fails**: `zstd-sys` C build (`bits/libc-header-start.h` not found) ← wasmtime |
| mlua (pin_script's Lua) | **fails**: `third_party/lua-src/src/lib.rs:100` "don't know how to build Lua for wasm32-unknown-unknown" | (not reached) |

Logs: `logs/build_unmodified_core.log`, `logs/build_unmodified_assets.log`,
`logs/build_unmodified_pin_sim.log`, `logs/build_unmodified_mlua_wasm32.log`.
`cargo tree -p pin_assets --target wasm32-unknown-unknown -e normal,features -i getrandom@0.4.3` shows
`getrandom feature "wasm_js"` ← `zip v9.5.1` ← `pin_assets`.

Source changes (scratch; patches in `S/patches/`):

| patch | rationale |
|---|---|
| 0001-pin_script-native-backends-feature | `mlua`, `notify`, `wasmtime`, `wat` optional behind default feature `native-backends`; `LuaConfig`/`WasmConfig` moved to always-built `config.rs` so `SimConfig` keeps its shape; `load_backend_with` refuses `.lua`/`.wasm` when the backends are not built. |
| 0002-pin_sim-external-script | pin_sim feature `native-backends` (default, forwards to pin_script); public trait `ExternalScript` (fault/overflow accessors with defaults, `alloc_exempt`) and `Simulation::load_from_vfs_with_script(vfs, cfg, factory)`; the `Script` enum keeps its concrete Lua/Wasm variants natively and gains `External`. |
| 0003-pin_sim-apply-game-op | public `replay::apply_game_op` so a driver that steps the sim itself replays bot games exactly like `GameRecording::play`. |
| 0004-s1_harness-spike-tooling | spike tooling only (not for main). |
| 0005-option-a-glam-scalar-math | evaluated alternative, **not recommended**. |

With 0001–0003 all five sim crates build for both wasm targets (`logs/build_patched_wasm.log`) and
`cargo clippy --target wasm32-unknown-unknown -p pin_script -p pin_sim -p pin_core -p pin_physics -p pin_input -p pin_assets --no-default-features -- -D warnings`
is clean (as is `--target wasm32-wasip1`). Native default build unchanged in behaviour: all native suites pass (§9). No `std::time`,
`std::thread` or `std::fs` change was needed (A6: the uses are off the web path and compile).

## 4. R2 — native scenarios and recordings

Recorded natively with the **real Lua rules** (`rules.lua`, `LuaConfig::engine()`), by wrapping the engine's
own `LuaScriptBackend` (same constructor arguments as `pin_sim::load::build_script`) to log the script's
output stream. **The wasm runs (and a native control run) replace the Lua VM by that recorded stream** (the
brief's fallback; Lua on the web is S2's). The playback checks every step: the switch edges and `HostView`
the script would have seen must hash to the recorded digest — every run reported `script_digest_mismatch=none`,
so the replacement is exact up to the first state divergence.

| scenario | content | ticks | max balls |
|---|---|---|---|
| `<table>_bot60` | repo autoplay `Bot` (Standard), soak seed `0x4B485A` game 0; a fresh bot (next game seed) after each game over | 60 000 | 1 / 1 / 3 |
| `<table>_pat60` | `pin_sim --ticks` built-in `pattern_events` + 300 ms `launch` every 5 s, through input ingestion | 60 000 | 1 / 1 / 2 |
| `<table>_mb8` | pattern + 8 balls spawned on a 4×2 grid before tick 1 | 20 000 | 9 |
| `apex_circuit_bot600` | 10-minute soak (5 bot games) | 600 000 | 1 |
| `replay_table_fixture10k` | the CI `replay-hash` fixture, `pin_sim --ticks 10000` pattern | 10 000 | 1 |

Cross-checks (every scenario, `logs/run_parity_simd.log`): native real-Lua hashes == native playback hashes,
every tick; for bot scenarios `pin_sim <bundle> --replay-game <scn>.khzrec` (the repo's own replay path)
prints the same final `state_hash`; the folder bundle and the zipped `.pinbundle` have the same table hash;
the fixture gives the documented `state_hash=25097c21c2c3adc1` natively (patched pin_sim) and in the harness.
Commands: `s1 record-bot <dir> <zip> 0x4B485A 60000 rec/<scn>`, `s1 record-pattern <zip> 0x4B485A 60000 0 rec/<scn>`
(… `20000 8` for mb8, `10000 0 … nolaunch` for the fixture). Sizes: `.scr` 1.44 MB per 60 s (14.4 MB soak),
`.scn` 3–36 KB, bundles 1.90–1.98 MB zipped.

## 5. R3 — parity results

`S/run_parity.sh simd` (final run: `logs/final_run_parity_simd.log`, 40 PASS lines, `== simd: 0 failing line(s)`, exit 0;
Chromium iso/non-iso bench runs also re-checked bot60 + mb8 parity):

| scenario | native real-Lua vs playback | native vs wasip1 (node:wasi) | native vs wasm-bindgen (Node) | native vs Chromium | final state_hash |
|---|---|---|---|---|---|
| replay_table_fixture10k | PASS | PASS | PASS | — | 25097c21c2c3adc1 |
| apex_circuit_bot60 | PASS | PASS | PASS | PASS (iso + non-iso) | 2b7b1eec564b8cdc |
| apex_circuit_pat60 | PASS | PASS | PASS | — | b9c2a55ba04954e9 |
| apex_circuit_mb8 | PASS | PASS | PASS | PASS (iso + non-iso) | 2c52fb33d519ffb7 |
| sky_deck_bot60 | PASS | PASS | PASS | PASS (iso + non-iso) | 1f0509f5802dc2e5 |
| sky_deck_pat60 | PASS | PASS | PASS | — | ce9eb11d97fbef81 |
| sky_deck_mb8 | PASS | PASS | PASS | PASS (iso + non-iso) | 777557dae8e5531b |
| feature_lab_bot60 | PASS | PASS | PASS | PASS (iso + non-iso) | 2040cea9ef77a2e0 |
| feature_lab_pat60 | PASS | PASS | PASS | — | 865f852fce618b7e |
| feature_lab_mb8 | PASS | PASS | PASS | PASS (iso + non-iso) | 0c64556eecf5c210 |
| apex_circuit_bot600 (10 min) | PASS | PASS | PASS | — | a4b944ad641338d1 |

PASS = every tick's `state_hash` **and** `ext_hash` identical (`ext_hash` adds the full snapshot incl. mech
transforms, lamps, GI, magnetisation, counters, persist, the applied command queue and haptics). Chromium:
`S/chromium/o3_iso1.json`, `o3_iso0.json`.

`S/run_parity.sh base` (no simd128; `logs/final_run_parity_base.log`: 15 PASS = native controls, 25 FAIL = every wasm line, exit 1) — every scenario FAILs on every wasm route at the first tick a ball spins:

| scenario | first diverging tick | native state_hash | wasm state_hash |
|---|---|---|---|
| replay_table_fixture10k | 6 | 44599810bd85b72b | 69cb3b4fc6103c64 |
| *_bot60 (all 3), bot600 | 86 | c8ae0e983a841b3d (apex) | 672a7c753c5f335a (apex) |
| *_pat60 (all 3) | 28 | 1f653090d653c715 (apex) | b7cdcb25606d169e (apex) |
| apex/feature mb8 | 8 | 8edeacc2ad425771 (apex) | c51130a3136517ff (apex) |
| sky_deck_mb8 | 20 | 78769dbc47c000bc | 57e429c169449b74 |

The diverging quantity (`s1 dump … <tick>` on both sides, `S/diverge/*.txt`) is always and only the ball
orientation quaternion, 1 ULP at first, e.g. apex bot60 tick 86: native
`rot 3c03befc 00000000 00000000 3f7ffde2`, wasm `… 3f7ffde3`; apex mb8 tick 8: `3be06f19 3f7ffe6c` vs
`3be06f1a 3f7ffe6d`. At ticks 30 000 and 60 000 positions/velocities are still bit-equal and only `rot`
differs (orientation does not feed back into the dynamics); scores and script inputs match. wasip1,
wasm-bindgen/Node and Chromium agree with each other bit for bit in this build too (same final hashes).

## 6. R4 — root cause and the smallest change

### 6.1 Bisection

| candidate | finding |
|---|---|
| glam SIMD vs scalar | **The cause.** native SSE2 vs wasm scalar differ (base FAIL); `+simd128` on wasm → all PASS; `scalar-math` on native → native == wasm-default, all 40 lines PASS (`logs/final_run_parity_scalar.log`; its native fixture hash is `77b7fda16bae55d2`). |
| `wide` (simba) SIMD vs scalar | Not involved: parity is exact once only glam's path is aligned; parry3d's `Vector` is glam `Vec3`. |
| libm vs core intrinsics | Not involved: `libm` is pure Rust (sinf/atan2f/expf digests identical on all builds); `sqrt` is IEEE-exact on both (`sqrtss` / `f32.sqrt`); std transcendentals are clippy-banned in the sim crates. |
| parry3d math | Only BVH/AABB over `Vec3`: identical. |
| fused ops | None on either side (A12 row). |
| f32/f64 mixing | IEEE-identical on both; covered by the parity runs. |
| NaN bit patterns | Not hit; platform-dependent (A12 row) → recommendation R-4 below. |
| HashMap order | wasm keys fixed, native random; parity holds ⇒ nothing depends on iteration order. |
| SIMD-width reductions | LLVM never reassociates FP reductions without fast-math; glam's 4-lane sums are the only width-dependent ones. |

### 6.2 Operation-level evidence (`s1 probe glam_ops`, 200 000 PCG inputs incl. ±0; `a13/glam_ops.txt`, `a13/scalar/native_scalar_probe.txt`)

| op | native SSE2 | wasm +simd128 | wasm default (scalar) | native scalar-math |
|---|---|---|---|---|
| quat_mul | fc16b055f9183578 | = native | 327c5e91c312e3b2 | = wasm default |
| quat_normalize | b883ddc5f62e282e | = | c76908f4fb9b8067 | = |
| quat_mul_vec3 | 5da4fcba241cdaa3 | = | feeb7ee508a2690c | = |
| quat_from_rotation_arc | d51ae0b67788166a | = | 82d114e762547e28 | = |
| integrate_orientation | 0b2a08bb65849acb | = | a77f56be170ad808 | = |
| vec4 dot / length / normalize | 27bd…/01c0…/ee5f… | = | 5a3d…/c4f2…/c769… | = |
| vec4_min_max (±0 ties) | 2865c280c17f7e51 | **b968422b2e284151** | = native | = native |
| mat4 mul / transform_point3 / inverse | 1313…/7768…/3235… | = | ec0c…/cb13…/c4e8… | = |
| vec3_ops, libm_trig | 0c5d…, eb9e… | = | = | = |

glam's `src/sse2.rs` and `src/wasm.rs` keep the same `(x + z) + (y + w)` grouping and the same shuffle/mul/add
sequence for `mul_quat` (`f32/sse2/quat.rs:885`, `f32/wasm/quat.rs:865`); the scalar path computes
`((x·x + y·y) + z·z) + w·w` (`f32/scalar/vec4.rs:246`).

### 6.3 The change and its effect on native

* **Smallest change: none in source — compile the web build with `RUSTFLAGS="-C target-feature=+simd128"`**
  (in the web build script; not a repo-wide `.cargo/config.toml`). Native binary unchanged ⇒ native goldens,
  fixtures and replay digests unchanged: fixture `25097c21c2c3adc1` (documented value) reproduced;
  `ball_seam_bitident` EXPECTED (5 digests) unchanged and passing; all native suites pass (§9). Native tick
  cost delta: 0 by construction. **Golden values regenerated: none.**
* Option (a) measured for comparison (`S/repo_scalar`, patch 0005): changes native output on every table
  with a rotating ball — fixture `25097c21c2c3adc1 → 77b7fda16bae55d2`; all 5 `ball_seam_bitident`
  digests change (`spec_scenario 0xa8e8957e1b4cee40 → 0x207bf150966c7ce7`, `spec_scenario_hook → 0xbb5a1a739cd69f37`,
  `box → 0x4917fc638551777b`, `belt → 0x6182693d6f197f14`, `slope → 0x307d23beff5b756e`) and the test fails
  (`cargo test -p pin_physics --no-fail-fast`: 830 passed, **1 failed**, 14 ignored; pin_core 240, pin_input 334,
  pin_assets 290 pass; pin_sim suite not rerun under (a) for disk reasons — it holds no golden constants);
  every recorded native replay hash in docs and CI notices would move; generated bundles do **not** change
  (Feature Lab bundle bytes identical, so table hashes and recordings stay valid). Native tick cost:
  within noise (0.93–1.13× of SSE2, §8). Blast-radius classification: breaking sim output for every table
  = very-high under D34 (evidence: the 6 pinned values above).
* Recommendation (Q5): **(c) other — keep native as is; build the web with `+simd128`, and gate it**:
  a CI job (new web workflow) runs `run_parity.sh simd`-equivalent native-x86_64-vs-wasm per-tick
  comparisons for the 3 tables and the fixture and **fails** on any difference (parity is a gate, not a
  report), plus the `glam_ops` probe so a glam bump that breaks the SSE2/wasm mirroring is caught at the
  operation level. Rejected: (a) (very-high blast radius, no gain over (c)); (b) web-only scalar path
  (would *create* the divergence `+simd128` removes).
* Limits of (c), stated: parity is with **x86_64** native. aarch64 native (macOS) already differs today
  (`ball_seam_bitident.rs:248-251`; glam's NEON path uses FMA `vfmaq_f32` in `mul_quat` and `vaddvq_f32`
  sums, `glam-0.33.12/src/neon.rs:62-65`, `src/f32/neon/quat.rs:916-918`); browsers on ARM run the same wasm
  and match x86_64 (wasm SIMD semantics are IEEE-exact on every CPU; not measured on ARM hardware here).
  Requires Wasm fixed-width SIMD in the browser (Chromium 157 here validates it: `simd: true`); minimum
  versions for Safari/Firefox/mobile: **needs S5/web**.

## 7. R5 — A6/A7/A28 tests

### 7.1 A6 probe outputs (verbatim, `s1 probe <name>`)

wasm32-unknown-unknown + wasm-bindgen under Node (panic messages via the hook, `a6/bindgen_probes_hook.txt`):
`instant` → panicked at `library/std/src/sys/time/unsupported.rs:13:9: time not implemented on this platform`, then
`RuntimeError: unreachable`; `systemtime` → same at `:35:9`; `sleep` → `sys/thread/unsupported.rs:45:5: can't sleep`;
`thread_builder` → `Err: kind Unsupported: operation not supported on this platform`; `thread_spawn` → panicked
`failed to spawn thread: … Unsupported`; `fs_read`/`fs_write`/`fs_canonicalize`, `net_tcp`/`net_udp` →
`Err(Error { kind: Unsupported, message: "operation not supported on this platform" })`; `mutex` → `Mutex lock ok,
RwLock two readers ok, OnceLock ok, try_lock while held = false`; `atomics` → ok; `hashmap_keys` → identical values
`c468947583dc0c9d / 007ea4558e0bf33d` in two separate processes (native: different every process).
wasip1 under node:wasi: time/sleep/SystemTime work, threads fail (`os error 58`), fs works through preopens.

### 7.2 A7 (`a7/a7_results.txt`, `s1 panic-in-step <bundle>`; harness `PanicScript` panics inside `step_1ms` at tick 5)

native: `catch_unwind around step_1ms returned Err: panic caught, process alive (sim tick 5)`;
wasip1: message on stderr then `TRAP: RuntimeError: unreachable`; wasm-bindgen without hook: only
`RuntimeError: unreachable`; with hook: `s1 panic: panicked at crates/s1_harness/src/playback.rs:167:9: …` then the
trap; `panic-then-call`: next call works once, after 2000 panicking calls → `memory access out of bounds` everywhere.

### 7.3 A28 (`a28/*.txt`)

Default: `stack_layout: sp~0xffeeb (1023 KiB) __data_end=0x12f3aa layout=stack-first`; `recurse_big(1000)` ok,
`(1100)`, `(2000)` → `RuntimeError: memory access out of bounds`; wasip1 identical. `recurse_small(10000)` ok,
`(20000)` → `RangeError: Maximum call stack size exceeded` (V8 limit). With
`RUSTFLAGS="-C target-feature=+simd128 -C link-arg=-zstack-size=8388608"`: `sp~0x7ffeeb (8191 KiB)`, `recurse_big(7000)`
ok, `(8300)` traps. Sim needs (`s1 stack-phases`/`stack_phases` export, painting the free shadow stack):
load 266 731 B, all ticks of every scenario 53 339 B below the caller (same for every table: depth depends on
the code path, not the data); the caller frame held ~181 KiB including the 61 552 B `Simulation`. Peak ≈ 450 KiB of 1 MiB.

## 8. R6 — tick cost (A27) and size

Median of 3 runs (`bench_r{1,2,3}/`, `logs/bench_median.md`; `harness/bench_all.sh`), release builds, playback
backend on every route (the Lua VM is not in the web numbers), one clock read per tick (steady = after 5000
ticks), 16-tick batches timed separately. Machine: this 4-core VM, one other spike process busy on 1 core.

| route | apex bot60 | sky bot60 | feature bot60 | apex mb8 | sky mb8 | feature mb8 |
|---|---|---|---|---|---|---|
| balls mean/max | 0.96/1 | 1.00/1 | 1.91/3 | 6.08/9 | 6.44/9 | 4.99/9 |
| native mean / p99 / p99.9 µs | 4.87 / 11.6 / 38.6 | 4.43 / 10.9 / 26.4 | 5.20 / 14.5 / 36.3 | 14.3 / 37.5 / 79.6 | 62.5 / 114 / 225 | 7.79 / 20.5 / 50.2 |
| native real Lua (context) mean / p99 | 6.50 / 19.4 | 6.81 / 19.6 | 7.63 / 22.2 | 16.9 / 46.6 | 59.5 / 103 | 10.9 / 29.7 |
| wasm-bindgen Node, release (o3) | 6.60 / 19.1 / 51.7 | 6.53 / 19.3 / 43.1 | 7.32 / 20.8 / 46.6 | 19.6 / 48.0 / 81.6 | 69.0 / 135 / 248 | 12.4 / 35.6 / 85.1 |
| wasm-bindgen Node, LTO + wasm-opt -O3 | 6.60 / 19.2 / 86.3 | 6.25 / 18.3 / 47.4 | 7.51 / 20.8 / 51.7 | 18.5 / 47.9 / 119 | 67.4 / 105 / 217 | 11.0 / 28.0 / 64.3 |
| wasip1 node:wasi | 6.41 / 16.7 / 42.3 | 6.58 / 18.7 / 43.5 | 7.63 / 22.8 / 58.4 | 20.5 / 52.8 / 99.4 | 66.7 / 115 / 222 | 11.4 / 31.9 / 60.0 |
| Chromium 157, isolated (5 µs timer) | 6.12 / 20 / 40 | 5.69 / 15 / 30 | 6.85 / 20 / 45 | 13.9 / 30 / 65 | 65.8 / 115 / 180 | 9.20 / 25 / 40 |
| ratio wasm(o3)/native | 1.35 | 1.47 | 1.41 | 1.37 | 1.10 | 1.59 |
| 16-tick batch p99 / max µs, native | 151 / 5619 | 170 / 311 | 183 / 2756 | 488 / 2429 | 1359 / 10291 | 297 / 2938 |
| 16-tick batch p99 / max µs, wasm o3 | 207 / 1009 | 204 / 431 | 241 / 1891 | 810 / 7447 | 1836 / 6463 | 553 / 1420 |
| 16-tick batch p99 / max µs, Chromium iso | 150 / 835 | 280 / 1860 | 225 / 690 | 425 / 980 | 2185 / 4450 | 295 / 395 |

Other variants (ratio to native, same scenarios): no-simd128 1.11–1.49 (simd128 costs nothing),
LTO 1.10–1.48, `opt-level=s`+LTO+`wasm-opt -Oz` 1.30–2.00; native with glam `scalar-math` 0.93–1.13 (noise).
Max values (single ticks up to ~8 ms, batches up to ~10 ms) occur natively too: OS/VM scheduling noise, not sim work.

Frame budget (60 Hz = 16.7 ms, 16–17 ticks per frame; 120 Hz = 8.3 ms, 8–9 ticks):
* 1–3 balls: 16-tick batch p99 0.15–0.28 ms on every web route = 1–2 % of a 60 Hz frame.
* 9-ball stress (sky_deck_mb8, the heaviest): p99 1.7–2.2 ms = 10–13 % of a 60 Hz frame.
* Assumed mobile core 3–5× slower than this VM (assumption, not measured): 1–3 balls 0.5–1.4 ms (3–8 %);
  9-ball worst case 5.5–11 ms per 60 Hz frame (33–66 %) or ~2.8–5.5 ms per 120 Hz frame (8–9 ticks). Fits, tight only
  in the 9-ball stress on the slowest assumed device; production play reached at most 3 balls in these runs.
* "1 kHz on the main thread" as one tick per 1 ms timer does not exist in a page: nested `setTimeout(0)` averages
  4.07 ms in Chromium (clamped), `MessageChannel` round trips 4 µs (would busy-spin the main thread). The tick
  itself (6–70 µs) fits a 1 ms slot ×14–160; the right shape is the rAF batch (§11 ii).
* `performance.now()` step: 5 µs isolated (COOP/COEP), 100 µs otherwise (Chromium); per-tick numbers above for
  the non-isolated page are therefore quantised (raw in `chromium/o3_iso0.json`).

Module size (sim + loader + harness, wasm-bindgen `--target web`, `logs/sizes.txt`):

| build | raw bytes | gzip -9 |
|---|---|---|
| release (opt-level 3, the repo's release profile) | 3 355 422 | 903 032 |
| + `lto=fat`, `codegen-units=1` | 2 729 444 | 829 058 |
| + `wasm-opt -O3 --strip-debug --strip-producers` | 2 147 265 | 779 426 |
| `opt-level=s` + LTO + `wasm-opt -Oz` | 2 119 438 | 743 724 |

(brotli not available on this VM.) The module includes pin_assets' glTF/JSON/PNG/JPEG decoders. Initial memory
19 pages; `__stack_pointer` 1 048 576. Imports of the wasm-bindgen module: `__wbg_error_*` (console.error),
`__wbg_now_*` (performance.now), `__wbindgen_init_externref_table`, one `Ref(String) -> Externref` intrinsic — nothing
from getrandom/crypto. The wasip1 module imports `wasi_snapshot_preview1` clock/fd/path/random_get/proc_exit only.

## 9. Native suites (rule 3) and lint

`harness/run_tests.sh S/repo <target> logs/tests_base.log` = `cargo test -p <c>` for pin_core, pin_input,
pin_assets, pin_physics, pin_sim one at a time (COMMON env) on the scratch copy **with patches 0001–0004**:
pin_core 240 passed; pin_input 334; pin_assets 290 (1 ignored); pin_physics 831 (14 ignored); pin_sim 368
(1 ignored); **0 failed** (`logs/tests_base_summary.txt`; the ignored tests are the pre-existing timing reports).
Golden values regenerated: **none**. `cargo fmt --check` clean on all touched crates. Clippy `-D warnings`
(`logs/clippy_native.log`): native `-p pin_script -p pin_sim -p pin_core -p pin_physics -p pin_input -p pin_assets --all-targets` rc=0;
native `-p pin_script -p pin_sim --no-default-features` rc=0; wasm32 (§3) clean; the harness was fixed for two
style lints (`unusual_byte_groupings`, `manual_is_multiple_of`) and is clean on native, wasm32-unknown-unknown and wasip1.

## 10. R7 — D5 allocation guard on the web

`s1 alloc` / `alloc_check` export wraps every `step_1ms` in `pin_sim::alloc_guard::count_allocs_split` with
`CountingAlloc` as global allocator: on native, wasip1 and wasm-bindgen, all 10 scenarios (incl. the 600 000-tick soak):
`alloc_guard installed=true … allocs_in_step=0 exempt=0` (`d5/alloc_guard.txt`). `is_installed()` is true on wasm,
so the thread-local counters work on single-threaded wasm32; std takes no different allocating path there.
(The Lua VM is not in these web runs; its arena allocations stay exempt via `ExemptScope`, which works the same.)

## 11. R8 — recommendations

**(i) Parity approach (Q5): exact native↔web parity is required and achievable now.** Build the web sim with
`-C target-feature=+simd128` (plus `-C link-arg=-zstack-size=4194304`), keep native unchanged, and make a
native-vs-wasm per-tick hash comparison (3 tables + the CI fixture, `state_hash` and the supplementary hash) a
**failing** CI gate in the web workflow, with the `glam_ops` probe alongside. Any future change that touches glam
(version bump) or adds `Vec3A`/`Vec4`/`Quat`/`Mat4` `min`/`max`/`clamp` to sim code must pass that gate (the ±0 tie
rule is the one known op-level difference under simd128; add those methods to the sim crates' `clippy.toml`
`disallowed-methods`). R-4: make `Fnv64::write_f32` (pin_sim `hash`) hash every NaN as one canonical pattern —
no existing golden changes (no NaN is ever hashed in any run), and it removes the only platform-dependent bit
pattern (A12). R-5: `glam` is already pinned `=0.33.12`; keep it exact.

**(ii) Sim thread: main thread, rAF batch catch-up (not a Worker), even though local COOP/COEP makes
SharedArrayBuffer available** (`crossOriginIsolated: true` verified). Measured: a 60 Hz frame's 16 ticks cost
p99 0.15–0.28 ms with 1–3 balls and ≤ 2.2 ms in the 9-ball stress; ≤ 11 ms even at an assumed 5× slower mobile core.
Batching at rAF time simulates exactly up to the frame time, so the rendered state is as fresh as a Worker's and
input placement stays tick-exact (inputs carry timestamps). A Worker would add a second wasm instance, byte-level
snapshot transport (`FrameStateSnapshot` 4000 B, `repr(Rust)`), and cross-thread script/audio plumbing for no
determinism gain. Keep a per-frame tick cap of its own (native `MAX_CATCH_UP_TICKS = 8` is below one frame; use e.g.
50, drop beyond, as `SimLoop::skip_ticks` does) and pause on `visibilitychange`. Revisit (Subphase 9.12) only if a
target phone measures 16-tick batch p99 > 4 ms in real play.

**(iii) Exact source changes for the sim crates (web):**
1. pin_script: backends optional (patch 0001), then split into `lua` and `wasmtime` features for 9.1 (Lua comes to
   the web, wasmtime does not); move `lua::api::{range, project, unit_float, next_u64}` (pure Pcg32 helpers) to a
   shared module first because `wasm/host.rs:19` imports `lua::api::range`.
2. pin_sim: `native-backends` feature, `ExternalScript` + `Simulation::load_from_vfs_with_script` (patch 0002);
   `replay::apply_game_op` public (patch 0003) for the web's headless replay/frame loop.
3. pin_sim (R-4): canonical NaN in `hash::Fnv64::write_f32`.
4. Sim crates' `clippy.toml`: disallow `glam::{Vec3A,Vec4,Quat}::{min,max,clamp}` (simd128 tie rule).
5. Nothing in pin_core, pin_physics, pin_input or pin_assets (they build unmodified; getrandom needs no flag).
   No pin_core CR is needed.
6. Web host (not sim crates): `Box` the `Simulation` (61.5 KB) instead of holding it on the shadow stack; install a
   panic hook (`console.error` + UI "engine error"), and on a trap discard and re-instantiate the module (state can be
   rebuilt by replaying the recording, which is deterministic); load bundles with `Vfs::from_zip_bytes` only.

## 12. Environment findings

* Node 22.23.5 `node:wasi` segfaults (exit 139) on any `fd_write` once the guest's linear memory exceeds ~32–40 MiB
  (`s1 bigmem /s/logs/tmp/big.bin 40 1` → exit 139; `16 1` → ok; reproduced 5/5 at 32 MiB single writes): a harness issue, not the engine. The wasip1 route
  therefore streams its hash output through a 64 KiB `BufWriter` (memory stays < 32 MiB); the wasm-bindgen route is
  unaffected (it held 40 MB+ for the soak).
* Disk: a single `cargo test -p pin_core -p pin_physics -p pin_input -p pin_assets -p pin_sim` produced 12 GB of
  test executables (≈166 MB each for pin_sim) and filled the shared disk to 100 % at ~17:53 (linker `Bus error`);
  `target/web_s1/debug` was deleted at once and tests were rerun one crate at a time with executables removed in
  between (`harness/run_tests.sh`). Other spikes building at that moment may have seen a failed link.

## 13. Commands index

```sh
. S/env.sh
S/run_parity.sh simd          # recommended build: expect "== simd: 0 failing line(s)"
S/run_parity.sh base          # default wasm features: expect FAIL lines with first divergence
S/run_parity.sh scalar        # option (a), native changed: expect 0 failing lines
S/harness/run_tests.sh S/repo /home/user/kilohertz-pinball/target/web_s1 S/logs/tests_base.log
S/harness/bench_all.sh && python3 -I S/harness/bench_median.py S/bench_r1 S/bench_r2 S/bench_r3
S/harness/chromium_bench.sh S/runs/variants/o3/pkg_web S/chromium/o3_iso1.json 1
S/harness/build_size_variants.sh
# probes (native: target/web_s1/release/s1; wasip1: node harness/run_wasi.mjs runs/simd/s1_wasip1.wasm …;
# bindgen: node harness/run_bindgen.mjs S/runs/simd/pkg_node …):
s1 probe all | glam_ops | nan_bits | sizes ; s1 panic-in-step <zip> ; s1 recurse-big <n> ; s1 alloc <zip> <scn> <scr>
s1 dump <zip> <scn> <scr> <tick> ; s1 stack-phases <zip> <scn> <scr> (wasm) ; s1 bigmem <file> <MiB> <KiB>
```
