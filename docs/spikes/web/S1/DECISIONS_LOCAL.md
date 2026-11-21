# web-spike-sim (S1) — local decisions (rule 8; scratch only, not the repo)

## s1-gate — how the script backends are gated out of the wasm build

**Decision:** One pin_script feature `native-backends` (default on) makes `mlua`, `notify`,
`wasmtime` and `wat` optional; `lua::LuaConfig` and `wasm::WasmConfig` move to always-built
`config.rs` files so `pin_sim::SimConfig` keeps its shape. pin_sim forwards the feature and gets
a new public `ExternalScript` trait plus `Simulation::load_from_vfs_with_script`, so a host can
supply any `TableScriptBackend` (the web rules backend, or here a recorded script stream).

**Rationale:** Smallest change that compiles the five sim crates for wasm32 with native
behaviour unchanged by default (the `Script` enum keeps its concrete Lua/Wasm variants for the
fault/overflow accessors, DECISIONS "2.5 — script backend construction"). The proposal's 9.1
already plans backend features; the factory hook is what 9.5 (web rules) and S2 (web Lua) need.

**Alternatives:** separate `lua` and `wasm` features (needed by 9.1 because Lua goes to the web
and wasmtime does not; `wasm/host.rs` imports `lua::api::range`, so the pure RNG helpers must
move to a shared module first; left to 9.1); make `pin_script` an optional pin_sim dependency
(changes `SimConfig`'s shape: rejected).

## s1-scenarios — what is replayed

**Decision:** Three scenario kinds per table, recorded natively with the real Lua rules:
`bot60` (the repo's autoplay `Bot`, Standard skill, soak seed `DEFAULT_SEED` game 0; a fresh
bot with the next game seed whenever a game ends, so one run spans several games; 60 000 ticks),
`pat60` (pin_sim's built-in `pattern_events` input stream plus a 300 ms `launch` every 5 s,
fed through input ingestion; 60 000 ticks) and `mb8` (the pattern plus 8 balls spawned on a grid
before tick 1; 20 000 ticks). Soak: `apex_circuit_bot600` (600 000 ticks = 10 min). The wasm
runs (and a native control run) replace the Lua VM by the recorded script-output stream
(per step: RNG draw count, a digest of the edges and `HostView` the script saw, the command
queue). The real-Lua native run is the reference.

**Rationale:** Bot games exercise real play (launch, flips, ramps, multiball), the pattern
exercises `pin_input` ingestion and staging, the spawn scenario covers ball counts. The
playback digest proves that, up to the first state divergence, the script would have received
identical inputs, so replacing it is exact.

**Alternatives:** only the `pin_sim --ticks` pattern (balls never launched: weak physics
coverage); running Lua in wasm (S2's job).

## s1-exthash — a supplementary hash

**Decision:** Besides the repo's `state_hash` (unchanged, the gate), every tick also gets an
`ext_hash` over the full published snapshot (mechs' dynamic meshes, zipper offsets, lamps, GI),
magnetisation, counters, persist, the applied command queue and haptics.

**Rationale:** `state_hash` leaves out mechs, magnetisation, persist and lamps
(docs/HUMAN_ACCEPTANCE.md:127); a parity claim must not miss a divergence there.

**Alternatives:** `state_hash` only (weaker; rejected by the brief).

## s1-featurelab — building the Feature Lab bundle

**Decision:** `tables/feature_lab/build.sh` step 1 (`cargo run -p pin_app --example
build_feature_lab`) is replaced in the scratch copy by a harness binary that `include!`s the same
example file verbatim (it only uses pin_tablegen, pin_assetgen and pin_core), followed by the
same `pin_tablegen validate`.

**Rationale:** Building pin_app pulls SDL3-from-source and wgpu (tens of minutes on a 4-core VM
shared with S3/S5) for no difference in the bundle bytes.

**Alternatives:** build pin_app (slow; same bytes).

## s1-parity — how native/web parity is obtained (informs Q5)

**Decision:** Recommend option (c): web build with `-C target-feature=+simd128`, native unchanged, and a
failing CI gate comparing native x86_64 and wasm per-tick hashes (3 tables + the CI fixture) plus the
`glam_ops` operation probe.

**Rationale:** Measured exact per-tick parity on all scenarios and three wasm routes with zero source change
and zero native output change (fixture `25097c21c2c3adc1` and the 5 `ball_seam_bitident` digests unchanged).
The owner requires exact parity; a native output change is acceptable only as a very-high-blast-radius
exception, and (c) needs none.

**Alternatives:** (a) glam `scalar-math` everywhere — also exact, but changes native output for every table
(fixture → `77b7fda16bae55d2`, 5 seam digests) with no gain; (b) a web-only scalar path — that *is* the divergent
default build.

## s1-thread — where the web sim runs (informs Q2)

**Decision:** Main thread, rAF batch catch-up with a web tick cap; no Worker in v1.

**Rationale:** 16-tick batch p99 0.15–0.28 ms (1–3 balls) and ≤ 2.2 ms (9-ball stress) on this VM, ≤ 11 ms at an
assumed 5× slower mobile core; batching at rAF time renders the state at exactly the frame time; a Worker adds a
second instance, byte-level snapshot transport and cross-thread script/audio plumbing for no determinism gain.

**Alternatives:** Worker + SharedArrayBuffer (available locally under COOP/COEP; verified `crossOriginIsolated`):
kept as the fallback if a target phone measures 16-tick batch p99 > 4 ms in real play.

## s1-env — harness adaptations to the VM

**Decision:** (1) The wasip1 route streams hash lines through a 64 KiB buffer; (2) native suites run one crate at
a time with test executables deleted in between; (3) option (a)'s pin_sim suite was not rerun.

**Rationale:** (1) Node 22.23.5 `node:wasi` segfaults on `fd_write` once guest memory exceeds ~32–40 MiB
(reproduced with `s1 bigmem`); (2) one combined `cargo test` produced 12 GB of executables and filled the shared
disk; (3) the same disk budget; pin_sim's tests hold no golden constants (grep), and option (a)'s effect on the
fixture hash was measured directly.

**Alternatives:** smaller hash files (loses per-tick coverage of the soak); `CARGO_PROFILE_TEST_DEBUG=0`
(deviates from the COMMON env).
