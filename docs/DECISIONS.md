# Decisions

Every ambiguity an agent resolved during the run, with rationale (PLAN.md §2.3 rule 8).
Format: `## <subphase> — <short title>` then **Decision**, **Rationale**, **Alternatives**.

## Orchestrator — Branching under a single-branch session

**Decision:** The run executes in a session restricted to one push branch (`ccr-f24f4063-bgqdfk`). The `wave/N` and `sp/<id>` branches from §2.6 are not created; each subphase lands as one or more commits on that branch, in wave order, and the commit subject is prefixed `[sp X.Y]`. The wave gate is recorded in `docs/RUN_LOG.md` instead of a `wave-N-green` tag.
**Rationale:** The session may push only to its designated branch; commit prefixes keep per-subphase history separable for later splitting.
**Alternatives:** Creating the plan's branch topology (not permitted in this session).

## Orchestrator — Role execution

**Decision:** Implementer and Independent Tester run as parallel sub-agents in the same working tree with disjoint owned paths; the Reviewer runs after both finish. Model routing follows §2.5 (Tester and Reviewer use the model that did not implement).
**Rationale:** A shared working tree is the only workspace available; disjoint paths make this equivalent to separate branches.

## 0.1 — Dependency spike results

Spikes live in `spikes/` (standalone Cargo workspace; run with `cargo test --manifest-path spikes/Cargo.toml --workspace -- --nocapture`, under `xvfb-run -a` on Linux). Evidence below is from a local Linux x86_64 run (Rust 1.97.0, no GPU, Mesa 25.2.8 lavapipe installed from apt, Xvfb). Windows and macOS results: **CI pending** (`.github/workflows/spikes.yml`, matrix windows-latest / ubuntu-latest / macos-latest).

### Q1 — Does `sdl3::event::Event` expose SDL's nanosecond timestamp?

**Answer:** Yes. In `sdl3 =0.22.1` every `Event` variant carries `timestamp: u64` copied verbatim from `SDL_CommonEvent::timestamp` (SDL3's `SDL_GetTicksNS` clock), and `Event::get_timestamp()` returns it for any variant (including `User`/`Unknown`).
**Evidence:** `spikes/sdl_timestamp` — for a pushed `Quit` event and for a real `JoyButtonDown` from a virtual joystick, the high-level timestamp equals the raw `SDL_Event` timestamp read via `sdl3::sys` (`SDL_PeepEvents` peek) and lies between two `SDL_GetTicksNS()` readings taken around the push/update. Linux: pass. Windows/macOS: CI pending.
**Fallback:** not needed. The thin raw-event wrapper (`raw_timestamp_ns`, `peek_raw_events`) is kept in the spike for reference.
**Finding for the input layer:** always drain `EventPump::poll_iter()` to `None`. Stopping early (e.g. `.find()`/`.any()`) leaves SDL3's poll sentinel in the queue and the *next* poll loop returns nothing even though events are queued.

### Q2 — Can SDL virtual joysticks be created headless on each OS (D31)?

**Answer:** Yes on Linux with `SDL_VIDEO_DRIVER=dummy`, `SDL_AUDIO_DRIVER=dummy`, `SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS=1`. Attach, `JoyDeviceAdded`, button press/release events and polled state, axis motion and polled state, detach and `JoyDeviceRemoved` all work with no display.
**Evidence:** `spikes/sdl_virtual_joystick` (virtual gamepad with South/Start/LeftShoulder/RightShoulder + LeftY "plunger"). Linux: pass. Windows/macOS: CI pending.
**Notes:** Joystick button indices are raw indices `0..nbuttons`; `sdl3`'s `with_button(Button::X)` sets `nbuttons = X+1` and a gamepad-mapping mask, so drive buttons by `Button as u32`. The first change of an axis also emits the axis's initial value (0) before the new value; consumers must use the latest value. SDL tests are `harness = false` binaries so SDL runs on the main thread (required by Cocoa) and is initialised once per process.

### Q3 — Which CI adapters report `wgpu::Features::EXPERIMENTAL_RAY_QUERY` (wgpu =30.0.1)?

**Answer (report-only):** Linux lavapipe (Vulkan, llvmpipe, Mesa 25.2.8): **yes**. Linux GL (llvmpipe via EGL): no. Windows WARP / macOS Metal: CI pending (read the `[wgpu Q3]` lines in the CI log).
**Evidence:** `spikes/wgpu_probe` enumerates every adapter on every backend and prints backend, type, name, driver and the feature bit. It never fails on absence.
**Decision:** Ray-query rendering stays an optional path gated on the feature bit at runtime (§6); the baseline renderer must not depend on it. Lavapipe reporting it means CI can exercise the ray-query path on Linux, subject to the CI Mesa version matching.
**Fallback (adapters without the bit, e.g. GL here):** the D18/§6 baseline — octahedral reflection probes — selected at runtime from the adapter's feature set; ray-query reflections stay off by default (D33). Lavapipe reporting the bit may let Subphase 5.4 validate the ray-query path on the Linux CI runner (functional, not performance).

### Q4 — Which present modes does each CI adapter report?

**Answer (report-only):** Linux under Xvfb (SDL `x11` driver, hidden window): lavapipe Vulkan reports `[Immediate, Mailbox, Fifo, FifoRelaxed]`; the GL adapter reports the surface as unsupported (the instance was created without a display handle, which wgpu GLES needs). Without an X server: "no surface available headless" (SDL video init fails; recorded, not an error). Windows/macOS: CI pending.
**Decision:** Only `Fifo` is assumed (§6). The probe only creates surfaces for SDL drivers with native handles (`windows`, `cocoa`, `x11`, `wayland`), because `sdl3`'s raw-window-handle impl panics on `dummy`/`offscreen`. The Linux CI job runs tests under `xvfb-run -a` and installs X11/Wayland dev headers so the vendored SDL build includes those video backends.
**Fallback:** per D19 — the playfield uses `PresentMode::Fifo` (the only guaranteed mode); a secondary surface that is offered only `Fifo` gets its own presenter thread. With no surface (headless CI) rendering is verified offscreen (Q5) instead of presenting.

### Q5 — Does offscreen rendering work on lavapipe (Linux) and WARP (Windows) in CI (Metal on macOS)?

**Answer:** Yes on Linux lavapipe: clearing a 64×64 `Rgba8Unorm` texture to blue, drawing a WGSL full-screen triangle into the left half via viewport, copying to a buffer and mapping it reads back exactly `[0,255,0,255]` / `[0,0,255,255]`. Windows WARP and macOS Metal: CI pending.
**Evidence:** `spikes/wgpu_probe` Q5 section. Adapter selection: Linux `Backends::VULKAN` + `force_fallback_adapter` (name must contain `llvmpipe`), Windows `Backends::DX12` + `force_fallback_adapter` (name must contain `Microsoft Basic Render Driver`), macOS `Backends::METAL`. With `SPIKE_REQUIRE_GPU=1` (set in CI) a missing adapter fails; without it the section prints SKIP so GPU-less local machines degrade gracefully (rule 3).

### Q6 — Smoke tests

- **mlua =0.12.2 (lua54 + vendored, Lua 5.4 from lua-src 551.0.2):** `set_memory_limit(4 MiB)` makes an unbounded allocation fail with `Error::MemoryError`, and the state stays usable afterwards. An `every_nth_instruction(1000)` hook returning `Err` aborts `while true do end` after exactly budget/1000+1 ticks; tick counts for a finite script are identical across runs (instruction-count, not wall-clock, budget). Linux: pass.
- **wasmtime =50.0.1:** `Config::consume_fuel(true)` + `Store::set_fuel` traps an infinite loop with `Trap::OutOfFuel` (fuel left 0); fuel used by a finite function is deterministic (sum(1000) = 12005 units, both runs), grows with work, and a store can be refuelled after a trap. Linux: pass.
- **parry3d =0.31.1:** `query::cast_shapes_nonlinear` for a rotating capsule (flipper, pivot at one end, ω = 30 rad/s about +Z) vs. a ball returns `Ok(Some(hit))` with status `Converged` and `time_of_impact` = 0.008351 s vs. analytic 0.008352 s; a moving ball also hits; a ball behind the sweep returns `Ok(None)`, not `Unsupported`. Linux: pass.
**Fallback:** none needed. Conservative advancement in the flipper's local frame (§6) remains the documented fallback for shape pairs that return `Unsupported` (e.g. non-support-map, non-composite pairs).

### Q7 — `versions.toml`

**Decision:** Pinned to the newest versions that built and passed (newer than the plan's expectations: sdl3 0.22.1 instead of 0.14.x, parry3d 0.31.1 instead of 0.26, mlua 0.12.2 instead of 0.11). Format: `[toolchain]` (rust 1.97.0, edition 2024), `[crates]` (`name = "=x.y.z"`), `[features]` (feature sets the spikes used), `[notes]` (SDL build approach, headless hints, GPU CI notes). `spikes/dep_smoke/tests/versions.rs` checks every pin against `spikes/Cargo.toml` and `spikes/Cargo.lock`, and that every crate the spikes use is pinned.
**Rationale/findings:**
- SDL3 is vendored (sdl3-src 3.4.20) and statically linked via the `sdl3` feature `build-from-source-static`, so CI needs no system SDL; cmake and a C compiler are required, and Linux needs X11/Wayland/audio/udev dev headers at build time for the native backends.
- `parry3d 0.31` has moved from nalgebra to glam via `glamx 0.3.1`, which pins `glam 0.33.12`. The engine must use `glam =0.33.12` (not the newest, 0.34.x) to share math types with parry. nalgebra is not in the tree; `simba 0.10.2` is (scalar traits only).
- `wasmtime 50` declares `rust-version = 1.97`, so the toolchain floor is 1.97.0. The workflow uses `dtolnay/rust-toolchain@stable` as the brief asks; stable must stay >= 1.97.
- `egui-wgpu 0.37.0` depends on `wgpu ^30`, so it matches the wgpu pin; an `egui_wgpu::Renderer` was created on lavapipe.
- `rand_core 0.10` renamed `RngCore` to `Rng`; `toml 1.2.1` is the crate version `1.2.1+spec-1.1.0`; `sdl3-sys` is `0.7.3+SDL-3.4.20` (build metadata left out of the pins because cargo ignores it in requirements).
- `egui` frames that are not painted must call `FullOutput::textures_delta.clear()`, otherwise epaint panics on drop.
**Alternatives:** Plan-era versions (sdl3 0.14, parry3d 0.26 with nalgebra, mlua 0.11). Rejected: the newest versions pass, and parry's math-library switch should be absorbed before any engine code exists.

## 0.1 — Spike workspace layout and CI choices

**Decision:** `spikes/` is its own workspace listing seven crates explicitly (`sdl_timestamp`, `sdl_virtual_joystick`, `wgpu_probe`, `mlua_sandbox`, `wasmtime_fuel`, `parry_nonlinear`, `dep_smoke`) and `exclude`s `spec_0_1` (the Independent Tester's crate). SDL and wgpu-probe tests are `harness = false`. Linux CI installs `mesa-vulkan-drivers`, Xvfb and the SDL build headers and runs tests under `xvfb-run -a`; `cargo fmt --check` runs on Linux only, clippy on all three OSes. `spikes/Cargo.lock` is committed so transitive versions are reproducible.
**Rationale:** Brief layout; main-thread SDL on macOS; one SDL init per process; Xvfb makes Q4 meaningful on Linux.
**Alternatives:** A libtest harness with `#[test]` SDL tests (rejected: off-main-thread SDL video is unsafe on macOS and SDL cannot be re-initialised concurrently).

## 0.1 — CI results (resolves the "CI pending" entries above)

Spikes CI run 43140608468 (commit a417638), all three jobs green, rustc 1.99.1 on the runners.

| Question | Linux (ubuntu-latest) | Windows (windows-latest) | macOS (macos-latest, aarch64) |
|---|---|---|---|
| Adapter | llvmpipe/Vulkan, Mesa 26.0.8, LLVM 21.1.8 (+ GL llvmpipe) | DX12 WARP "Microsoft Basic Render Driver" | Metal "Apple Paravirtual device" |
| Q1 ns timestamp | pass | pass (100 ns clock granularity) | pass |
| Q2 headless virtual joystick | pass | pass | pass |
| Q3 `EXPERIMENTAL_RAY_QUERY` | Vulkan yes, GL no | no | no |
| Q4 present modes | [Immediate, Mailbox, Fifo, FifoRelaxed] (Xvfb) | [Mailbox, Fifo, Immediate] | [Fifo, Immediate] |
| Q5 offscreen render + readback | pass | pass on WARP | pass on Metal |
| Q6 mlua / wasmtime / parry | pass, identical counts on all OSes | pass | pass |

**Consequences.** Only Linux lavapipe can exercise the ray-query path in hosted CI; Windows and macOS validate the probe fallback (D18, D33). macOS reports no `Mailbox`, so secondary surfaces there use `Immediate` or a presenter thread (D19). The local Mesa in Q3 above (25.2.8) differs from CI (26.0.8); answers agree.

## 1.1 — Workspace layout and dependencies

**Decision:** All 11 crates listed in §2.1 are created (the plan's "10 crates" is a miscount). The root workspace uses resolver 3, `members = ["crates/*"]`, `exclude = ["spikes"]`, edition 2024, rust-version 1.97, and copies every `versions.toml` pin unchanged into `[workspace.dependencies]` with the recorded features. Stub crates depend only on `pin_core`; `pin_core` uses serde, toml, arrayvec, rand_pcg and rand_core (proptest as dev-dependency). No new pins were added: `thiserror` and `glam` are not needed (errors implement `Display` by hand; snapshot math is plain `[f32; N]` arrays). `profile.dev`/`profile.test` use `opt-level = 1` so later solver tests stay fast. `pin_script`'s `guest-sdk/` is stubbed as module `guest_sdk` (hyphens are not valid module names); Subphase 4.5 may move it to its own crate.
**Rationale:** Brief: keep `cargo test --workspace` fast and light; versions.toml is 0.1's file.
**Alternatives:** glam types in the snapshot (rejected for 1.1: adds a dependency to every consumer of `pin_core` with no need yet; an additive CR can add conversion helpers).

## 1.1 — On-disk schema interpretation (Appendix A)

**Decision:** On-disk types (`pin_core::schema::{table, backglass, layout, physics}`) are separate from runtime types. Specifics:
- `[[flippers]]` → `FlipperDef`: lengths in mm *without* `_mm` suffix as in A.1 (`base_position`, `shape.length`, `zipper_slide_axis`, `zipper_slide_speed`); non-length physics keys keep §3.3's SI names (`inertia_kg_m2`, `power_torque_nm`, `hold_torque_nm`, `return_spring_torque_nm`, `stop_restitution`, `rubber_restitution`, `rubber_damping`) and are optional, defaulting from `physics_defaults.toml` (+ table `[physics]` overrides). `rubber_restitution` defaults to the steel–rubber pair. `shape` and `staging` are internally tagged by `type`; staging uses `input`/`axis` names (`left_flipper`, ...) and switch names. `FlipperDef::to_config(id, &impl NameResolver, &PhysicsParams)` converts to §3.3 `FlipperConfig`.
- Optional: `[physics]`, `[persist]`, `[dof]`, `[backbox]`, `balls.restrict`, every `[[array]]` section, `meta.palette`, `meta.pitch_deg` (default 6.5), switch `position`/`radius`/`layer`, lamp `color`/`response` (default `led`), magnet `tau_ms`, mech `coil`/`switch`/`position`, spline `gauge`/`roll_deg`, wall `closed`, post `rubber`. `[meta]`, `[balls]`, `[cabinet_switches]` (all 8 keys), `[translite]`, `[score_display]`, `[playfield]` are required.
- `[dof].map` entries carry exactly one of `coil`/`switch`/`lamp` plus `type` (`S`, `W`, `L`, `G`, `E`) and `number`; anything else is `SchemaError::Invalid`.
- Parsing checks syntax, types, unknown keys and `schema_version == 1` only. Name cross-references are *not* checked (the A.1 example itself references undeclared switches such as `lower_left_eos`); that is `pin_assets` (4.1). Mech kind-specific parameters are not yet in the schema; adding them is an additive CR.
- `NumberResolver` (switch/coil name → number as id) is a provisional helper; 4.1 owns final interning.
**Rationale:** Brief implementer notes; Appendix A is normative and its example must parse.

## 1.1 — physics_defaults.toml format

**Decision:** `schema_version = 1`; `[[material_pairs]]` with `materials = [a, b]` (unordered, duplicates rejected), `restitution`, `friction`; `[tunables]` with `{ default, min, max }` per Appendix C parameter (`flipper_inertia`, `flipper_power_torque`, `flipper_hold_torque`, `flipper_return_spring_torque`, `flipper_stop_restitution`, `flipper_rubber_damping`, `rolling_resistance`, `globall_magnus_coeff`, `magnet_remanence_tau_ms`); `[feel_windows]` with `{ min, max }`. A table's `[physics]` overrides any tunable by the same key (A.1's `flipper_rubber_damping` example), plus `flipper_rubber_restitution`. Units SI except `_ms`. Materials are an enum: steel, ceramic, plastic, resin, rubber, wood, acrylic; `BallProfile` gained a `material` field (steel/ceramic/plastic for Glo-Ball/resin) for pair lookup.
**Rationale:** Appendix C lists values but no file format; key names follow the A.1 override example.

## 1.1 — Runtime contract details not fixed by §3

**Decision:**
- IDs are `u16` newtypes with `get()`, `index()`, `From` both ways, serde-transparent.
- Units: `mm_to_m(x) = x / 1000`, `g_to_kg(x) = x / 1000` (division keeps exact results such as `mm_to_m(27.0) == 0.027`), `deg_to_rad(x) = x · π/180`.
- `FrameStateSnapshot`: `tick`, `ball_count` + `[BallState; 16]` (position m, quaternion `[x,y,z,w]`, velocity m/s, angular velocity rad/s, `ProfileId`, layer), `flipper_count` + `[FlipperState; 16]` (id, angle rad relative to rest, angular velocity, zipper offset m), `dynamic_mesh_count` + `[MeshTransform; 64]`, `lamps: [f32; 256]` indexed by `LampId`, `gi: [r, g, b, intensity]`. Default GI is full white.
- `TripleBuffer`: one `AtomicU8` back-slot index + fresh bit, AcqRel swaps; `new` is the only allocation; also `TripleReader::has_update()`.
- `HostCommandQueue` keeps exactly the §3.4 fields; push helpers return `Err(QueueOverflow)` and drop the command. The overflow *counter* shown in the debug HUD is kept by the caller (pin_sim, 2.5), not in the queue, so the struct stays exactly §3.4.
- `ScriptError` variants: `Load`, `Runtime`, `BudgetExceeded`, `MemoryLimit`, `UnsupportedBackend`.
- `InputEvent { timestamp_ns, kind: InputEventKind::{Button{input, pressed}, Axis{axis, value}} }`; `CabinetAxis` = plunger, nudge_x, nudge_y, left_flipper, right_flipper; `HapticsTelemetryEvent { tick, source: u16, strength }` maps to telemetry kind 5.
- Telemetry decode requires exact length `16 + 8·count`, `count ≤ 180`, known kinds; the reserved byte is ignored. `for_each_datagram` splits > 180 entries into same-tick datagrams and sends an empty full-state datagram when asked.
- Extra derives beyond §3.3/§3.4 (`PartialEq`, `Copy` where possible) were added; they are additive.

## 1.1 — Review fix round

**Decision:** Non-blocking review items applied while `pin_core` can still change freely:
- N1: proptests added for `FlipperConfig::eos_angle_deg`/`end_angle_deg`, `FeelWindow::contains`, and `HostView::switch_closed`/`set_switch` (set-then-get, other bits untouched, ids ≥ 256 are no-ops).
- N2: every manifest parser rejects NaN/±inf in any float field. The check runs for all four manifests in the shared parse path, before each manifest's own `validate`. It walks the typed value as TOML and returns `SchemaError::Invalid` naming the key path (e.g. `flippers[0].shape.length`).
- N3: `layout.toml` `walls[].material` and `decks[].material` are now `pin_core::Material` (snake_case: steel, ceramic, plastic, resin, rubber, wood, acrylic), which covers Appendix A and C.
- N6: the redundant `unsafe impl Send` for `TripleWriter`/`TripleReader` is gone. `Send` follows from `Arc<TripleBuffer<T>>`, and the compile-time asserts stay.
**Follow-ups for later subphases (not done in 1.1):**
- N4 (Subphase 4.1): manifest validation must reject switch and lamp numbers > 255. `HostView::switches` is a 256-bit set and the snapshot has 256 lamps; switch and lamp numbers are used as ids, while the on-disk type is `u16`.
- N5 (Subphase 4.1): `score_display.mode = "dmd"` requires a `dmd` table and `"lcd"` requires an `lcd` table.
- N7 (Subphase 2.4): kind-specific `[[mechs]]` parameters (kick strength, spinner damping, ...) are not in the schema yet. File an additive CR early in 2.4 (new `Option` fields or a per-kind table, with defaults in `physics_defaults.toml`).

## Orchestrator — Wave 2 brief decisions

- **3.1 dual-stage semantics.** `FlipperStagingMode` is per flipper and does not say which stage a flipper belongs to, so the staging router takes a `FlipperStage { Lower, Upper }` per flipper (a pin_input type, not a contract change). A `DualStageThreshold` flipper is active when the axis is at or above `lower_threshold` (Lower stage) or `upper_threshold` (Upper stage). `DedicatedButton` and `ParallelLinked` both mean "active while that cabinet input is pressed"; they differ only in which input a table binds. `AutoSensorThingFlip` is scheduled by physics (2.3), not the router. Where a flipper's stage comes from in `table.toml` is left to 4.1 (default: flippers bound to `left_upper`/`right_upper` or with an upper-deck layer are Upper).
- **1.3 SPSC ring in pin_core.** The input and window-event rings cross crates (pin_app → pin_sim / pin_render), so the generic SPSC ring is an additive contract change (`pin_core::spsc`) made by 1.3.
- **1.3 PiP geometry.** Backglass PiP aspect 4:3, score PiP 4:1, sized by a width fraction of the window and anchored to a corner. In cabinet mode a role without its own display falls back to a PiP on the playfield window.
- **4.2 script IDs.** Scripts address hardware by integer IDs looked up from read-only name tables (`pinball.sw.<name>` etc.) built at load, so no strings cross into the sim thread (§3.1). Script callbacks are global functions `on_init`, `on_tick`, `on_switch(id, closed)`, `on_save_state`, `on_restore_state`.
- **Wave 2 working tree.** Five subphases share one working tree; each role uses its own `CARGO_TARGET_DIR` and builds only its own crate.

## 1.2 — CI matrix skeleton

**Decision:** `.github/workflows/ci.yml` has jobs `core-tests`, `gpu-tests`, `clippy` (each on windows-latest / ubuntu-latest / macos-latest = x86_64-pc-windows-msvc / x86_64-unknown-linux-gnu / aarch64-apple-darwin; a step fails the job if `rustc` is older than 1.97 or the host triple differs from the matrix target), `fmt` (Linux), `replay-hash` (no-op placeholder for 2.5) and `coverage` (Linux, `cargo llvm-cov --summary-only` over the GPU-free crates, `continue-on-error`, non-gating). Toolchain is `dtolnay/rust-toolchain@stable` (consistent with spikes.yml; see HUMAN_QUEUE 0.1).
- `core-tests` runs `cargo test --workspace --exclude pin_render --exclude pin_audio --exclude pin_app` with `SDL_VIDEO_DRIVER=dummy`; on Linux it also runs `tests/spec_ci` when that package exists.
- `gpu-tests` runs `cargo test -p pin_render -p pin_audio -p pin_app` (under `xvfb-run -a` with lavapipe on Linux) and sets `WGPU_BACKEND` = vulkan / dx12 / metal plus `PIN_REQUIRE_GPU=1` (the root-workspace counterpart of `SPIKE_REQUIRE_GPU`; pin_render may read it so a missing adapter fails in CI). TODO(1.4): switch to `--features gpu-tests`.
- Both workflows: `timeout-minutes: 45` on every job, `concurrency` group `${{ github.workflow }}-${{ github.ref }}` with cancel-in-progress, `permissions: contents: read`, `workflow_dispatch`. Push runs on every branch; a PR therefore also runs on its merge ref (accepted; different refs, so no cancellation between the two).
- spikes.yml now also runs fmt / clippy -D warnings / test for `spikes/spec_0_1` (Linux).

## 3.1 — Input ingestion & flipper staging (Implementer)
- **Debounce is evaluated on the tick-sampled raw state.** All due events are applied to a raw button state first; the frame's debounced state then takes the raw state at that tick unless the input is inside a window opened by an earlier accepted edge. A press+release inside one tick is therefore no edge at all (consistent with the debounce-0 rule "final state within a tick"), and a glitch shorter than one tick never registers.
- **Window-end settling is itself an accepted edge.** When a window ends with raw ≠ debounced, the raw state is applied at tick t + debounce_ms and opens a new window, so consecutive debounced edges are always ≥ debounce_ms ticks apart.
- **Due cutoff is inclusive and saturating:** an event with `timestamp_ns == epoch_ns + tick·1_000_000` is applied at `tick` (brief wording, "≤"); `epoch + tick·1e6` saturates at `u64::MAX`.
- `FlipperStagingRouter::update` uses `try_push` and drops changes that do not fit in a non-empty `out` (an empty `out` always fits one change per entry); `is_active` returns `false` for unknown flippers. `StagingError` = `TooManyEntries(n) | DuplicateFlipper(id) | InvalidThresholds { flipper, lower, upper }`.
- `arrayvec` (workspace pin) added as a direct pin_input dependency, `proptest` as a dev-dependency.

## Orchestrator — 3.1 sub-tick debounce

**Decision:** With `debounce_ms > 0`, debounce acts on each input's state at the end of the tick, the same rule the brief gives for `debounce_ms = 0`. A press and release inside one tick therefore produce no edge and open no window.
**Rationale:** One rule for both cases; the sim only ever sees per-tick state (§2.2 step 1). The Implementer's final code follows this; the Tester left the case unpinned.

## Orchestrator — 1.2 review B1: CI toolchain pinned to versions.toml

**Decision:** CI (ci.yml and spikes.yml) uses `dtolnay/rust-toolchain@1.97.0`, the toolchain recorded in `versions.toml`, instead of `@stable`. The rustc ≥ 1.97 check becomes an exact-version check. A toolchain bump is a deliberate change to `versions.toml` plus the workflows.
**Rationale:** The 1.2 review found `@stable` (1.99.1 on runners) introduces new clippy lints (`chunks_exact_to_as_chunks` in pin_core/src/telemetry.rs) that turn already-reviewed code red with every Rust release. Same-build determinism (D9) also favours one pinned compiler. Resolves the HUMAN_QUEUE [0.1] toolchain item.
**Also:** the concurrency group includes `github.event_name`, so a `workflow_dispatch` gate run is not cancelled by later push runs (review N2).

## 1.3 — SDL3 windows, display topology and SPSC rings

**Decision:**
- **`pin_core::spsc` (CR-001, additive).** `channel(capacity)` panics on `capacity == 0` (the brief requires ≥ 1). Capacity is exact for any value; head/tail positions run over `0..2·capacity` so full and empty differ without power-of-two rounding. Extra helpers beyond the fixed API: `SpscConsumer::peek`/`is_empty`/`capacity`, `SpscProducer::len`/`is_empty`/`is_full`/`capacity`.
- **Ring overflow.** The main thread never blocks: when the input or window ring is full the event is dropped and counted (`ForwardStats::input_dropped` / `window_dropped`). Default capacities: input 1024, window 256.
- **`pin_app` library target.** `pin_app` was a binary-only crate, so integration tests could not reach `platform`. Added `crates/pin_app/src/lib.rs` declaring the four pre-stubbed modules (`orchestrator`, `platform`, `profiler`, `ui`); `main.rs` now uses the library. Later pin_app subphases (1.4, 4.4, 6.0, 7.1) keep their module directories unchanged. `pin_app` depends on `pin_render` by relative path (no root `[workspace.dependencies]` entry was needed).
- **Topology rules not fixed by the brief.** Display numbers in `TopologyMode` refer to `DisplayInfo::index` (pin_app fills it with the position in SDL's display list). PiP height = `floor(w·3/4)` (backglass) or `floor(w/4)` (score). A PiP that rounds to zero size or does not fit the window → `InvalidPip(role)`; overlapping backglass/score PiPs → `InvalidPip(Score)`; touching edges are allowed. Check order: empty list, unknown displays (playfield, backglass, score), duplicates, PiP geometry. Windows are ordered playfield, backglass, score; viewports are ordered playfield/own role first, then backglass PiP, then score PiP. Extra public items: `pip_rect`, `DEFAULT_BACKGLASS_PIP`, `DEFAULT_SCORE_PIP`, `Rect::{new, right, bottom, overlaps, contains}`, `WindowPlan::{primary_role, viewport}`, `TopologyPlan::find`, `SurfaceRole::pip_aspect`.
- **Window events type.** `pin_render::topology::{WindowEvent, WindowEventKind}` (SDL ns timestamp + index into `TopologyPlan::windows`), so the render thread consumes it without depending on pin_app. Events for windows not registered with `EventForwarder::register_windows` are counted (`window_unknown`) and dropped. Display add/remove/mode changes are reported by `PumpOutcome::displays_changed` for the main thread to re-plan.
- **Event pump.** `Platform::pump(&mut forwarder, timeout)` waits with `SDL_WaitEventTimeout` (`PUMP_TIMEOUT` = 1 ms), then drains with `poll_event` until empty (full drain, DECISIONS 0.1). A zero timeout only drains (tests). Key repeats are ignored; keys map by scancode (layout independent).
- **Default gamepad map** (keyboard map is the brief's): shoulders = flippers, stick clicks = upper flippers, South = launch, East = action, Start = start, Back = tilt; triggers → analog `LeftFlipper`/`RightFlipper` axes 0..1, left stick down → plunger 0..1, right stick → nudge −1..1. Replaced by profiles in 3.2.
- **SDL test.** `crates/pin_app/tests/platform_headless.rs` is a `harness = false` binary (main thread, one SDL init) using the dummy video/audio drivers; it needs no X server. It belongs in the `gpu-tests` CI job per the brief.
**Rationale:** Brief + PLAN §2.2; choices keep the fixed API exact and pick the strictest reading where it was open.

## 2.1 — BVH, collision layers & C² spline colliders (Implementer)

**Decision:** `pin_physics::world` is split into `spline`, `wireform`, `shapes`, `static_world`, `transition`. Dependencies added to `pin_physics`: `parry3d`, `glam`, `arrayvec`, `libm` (dev: `proptest`), all existing workspace pins.
- **Broad phase = parry3d `Bvh`, narrow phase = own exact convex cast.** One BVH leaf per primitive (cuboid, cylinder, or a single mesh triangle). Sphere casts use the fact that the gap of a moving sphere to a convex shape is a convex function of time: Newton from t = 0 approaches the first root monotonically and cannot step over it, so the cast cannot tunnel at any speed, needs no iteration tuning, and hits flat faces in one step (head-on cuboid TOI is exact to rounding). **Alternative:** parry's GJK `cast_shapes` (rejected: lower TOI accuracy in f32, harder to prove allocation-free and bit-reproducible; the BVH is still parry's).
- **Initial-overlap rule.** A sphere that already touches/overlaps at t = 0 is a hit at time 0 only if its closing speed exceeds 1e-5 of its speed; resting, sliding and separating overlaps are left to `sphere_contacts`. The threshold exists because a velocity that a solver has just made tangential keeps a rounding residue that would otherwise be reported as an impact forever (found with the verification integrator).
- **Ghost collisions.** Mesh vertices are welded by position and edges shared by two coplanar triangles are flagged; such interior edges/vertices only collide when the sphere is straight above them, so a ball rolling over a tessellated flat sees one face contact and no edge hits. Creased edges and mesh boundaries collide normally. Meshes are two-sided.
- **`StaticShape::Cylinder::axis`** is read as the vector from the bottom-cap centre to the top-cap centre (direction = axis, length = height), because the brief's comment says both "unit" and "length = height" and there is no height field; a unit vector gives a 1 m cylinder.
- **Invalid geometry** (non-finite, zero height, zero-area or out-of-range triangles) is skipped at build time because `build()` returns no `Result`; the handle stays valid.
- **Splines.** Natural C² cubic through the points, uniform parameter (t = i), tridiagonal solve in f64, stored as f32 Bézier segments plus the knot second derivatives (so `second_derivative` is exactly continuous at knots). `closest_point` is globally correct for any seed: seeded Newton gives an upper bound, then per-segment bounding spheres and a 16-per-segment sample table with arc-length bounds prune the rest, and bracketed Newton/bisection refines. Only exactly identical consecutive points are an error.
- **Uniform parameter caveat.** Through-points must be roughly evenly spaced; a 50 mm → 13 mm spacing change made the guide wiggle enough to kick the ball at 8 m/s. Authoring tools (4.x) should resample or warn.
- **Wireform.** Rotation-minimizing frame by double reflection on the sample table (f64), cubic-Hermite interpolated using the analytic RMF derivative, re-orthogonalised against the exact tangent, then rolled. **Roll angles are interpolated with a natural C² cubic spline** (same reason as D13: a piecewise-linear roll would give rail-curvature jumps); positive roll is right-handed about the tangent. `rail_points` returns the `+binormal` rail first. If `up_hint` is parallel to the start tangent a deterministic fallback axis is used instead of an error.
- **API additions beyond the fixed list** (additive): `layers::ALL`, `CubicSpline::max_t`, `BezierSegment::{point, tangent}`, `Wireform::{centerline, gauge, roll_rad, closest_on_rails}`, `GuideCurve::{new, spline, height, closest_point, cast_sphere}` + `GuideHit`, `StaticWorld::{collider_count, collider_layers}`, `ColliderHandle::index`, `LayerTransition::contains`, `TransitionSet::zones`, `MAX_SPHERE_CONTACTS`, extra `SplineError` variants.
- **`GuideCurve`** is the spline extruded along +Z by the wall height (zero-thickness ribbon, two-sided). Its cast is conservative advancement (never tunnels); it reports contact at a 10 µm skin and, when a graze does not converge in 64 steps, returns the safe time reached so far.
- **`TransitionSet::update`** tests end points only, exactly as the brief words it (prev outside, now inside); zones must be thicker than one step's travel, or the caller updates per CCD sub-step. Layers are compared for equality, so callers may store indices or mask bits.
- **`sphere_contacts`** clears `out`, counts touching (depth 0), merges coincident contacts of one collider, keeps the 16 deepest, and returns them in collider order.
**Verification scenario** (`tests/guide_ramp.rs`): test-only frictionless integrator, 8 CCD sub-steps per 1 ms tick. Energy is checked per tick with a 1e-6 relative allowance for f32 rounding. "Away from entry and exit" is taken as 15°–165° of the arc and from 0.2 m after the ramp start to the ramp top. The ramp is a 1.15 m wireform whose slope eases in over 1 m: at 8 m/s a shorter ramp makes the *physical* normal force change by more than 5 % per tick, and a convex crest would launch the ball, so the 65 mm rise ends on a straight incline. The rails emerge through the floor over 0.3 m so a ball arriving off-centre is picked up without a step.

## 4.2 — Lua 5.4 backend, sandbox & hot reload

- **One fresh Lua state per load/reload.** `apply_reload` builds the new script in a new sandboxed state (compile + top-level run under the budget), calls the old `on_save_state` (skipped when faulted; any failure there means "no state", so a broken save hook cannot block its own fix), copies the result across as plain data (nil/bool/int/float/string/tables ≤ 64 deep; functions/userdata entries skipped; shared or cyclic tables → no state), then calls the new `on_restore_state`. A compile error, top-level error or failing `on_restore_state` returns `Err` and the old script keeps running. `on_init` is not re-run. Coil-hold tracking survives reloads (the coils are still physically held).
- **Host context.** The `pinball` API exists only inside `on_init`/`on_tick`/`on_switch`; at top level and in the reload hooks it raises an error (there is no `HostCtx` then). The context reaches the registered Rust functions as raw pointers in a shared cell, set for the duration of one callback (no per-call closures/scopes, so no allocation).
- **Argument policy.** Out-of-range or mistyped arguments raise Lua errors (→ fault), consistent with "argument errors raise Lua errors": ids must be integers (or integral floats) in 0..=65535 (`pulse_coil(65536, …)` faults), `ms` 0..=65535, booleans must be booleans (no truthiness), `set_lamp`/`set_magnet`/`set_gi_tint` values in [0, 1], `play_sound` gain finite and ≥ 0, `set_score` player 1..=4 and integer score, persist keys 0..64. `switch(id ≥ 256)` returns false like `HostView::switch_closed`. Queue overflow is never an error: dropped and counted (`overflow_count`, cumulative).
- **Tilt.** On a fault the backend queues `(coil, false)` for every held coil; if the hold queue is full the rest are released on the following callbacks. `last_error()` keeps the cause.
- **Budget.** `every_nth_instruction(min(budget, 1000))` hook; granularity is the hook interval (and up to one interval of carry-over between callbacks, as Lua does not reset the hook counter). Once exhausted the hook interval is switched to 1 (via `lua_sethook` keeping mlua's hook), so every further instruction errors and `pcall` loops cannot swallow the abort. Memory errors caught by `pcall`/`xpcall` are re-raised.
- **`xpcall` is a wrapper** that runs the message handler after unwinding (as ordinary hooked code) instead of at the error point: Lua runs a handler for an error raised from inside a hook with hooks disabled, so a looping handler escaped the budget (Tester finding).
- **`__gc` is not available to scripts.** `setmetatable` refuses metatables with a `__gc` field: Lua runs finalizers with hooks disabled, so a looping finalizer would stall a tick, `apply_reload` or drop. (A `__gc` added to a metatable after `setmetatable` is ignored by Lua 5.4.) Alternative of keeping the hook armed is impossible without patching Lua (`GCTM` clears `allowhook`).
- **Read-only `pinball` tables**: empty proxies with `__index` to hidden tables, erroring `__newindex`, `__pairs`/`__len`, `__metatable = "locked"`; the sandbox's `rawset` is a wrapper that refuses these proxies. The string metatable is locked too.
- **Sandbox contents**: base minus `dofile`/`loadfile`/`load`/`require`/`warn`; `print` is a no-op; `collectgarbage` answers only `"count"`; `string.dump` removed; source is loaded in text mode only (bytecode rejected); `math.randomseed` is a no-op; `math.random` is a Rust function on the host PCG with Lua 5.4 semantics (`random()` float with 53 bits, `random(0)` raw integer, `random(m)`, `random(m, n)`, Lua's masked-rejection `project`; three arguments raise "wrong number of arguments"; a trailing explicit `nil` counts as absent). No `coroutine` library (not in §3.4's list).
- **GC.** The automatic incremental collector stays on (allocation-paced, so allocation-heavy scripts cannot outrun it to the cap), and `on_tick` additionally runs one fixed `LUA_GCSTEP` of `GC_STEP_KBYTES = 16` under a fresh budget.
- **`Send` without mlua's `send` feature.** That feature puts a `parking_lot::ReentrantMutex` on every Lua access (rule 4: no mutex per tick). The backend instead `unsafe impl Send`: every `Rc` clone (bridge, mlua handles) lives inside the backend or the Lua state it owns, so the whole graph moves together; mlua (lua54) has no thread-locals.
- **Allocation guard** (`tests/lua_alloc.rs`): 10 000 steady-state ticks + switch edges of a script that calls most of the API perform **zero** heap allocations, counting Lua-VM allocations too (mlua allocates through the global allocator). Scripts that create objects allocate in the capped Lua arena (exempt). Calls through vararg Lua wrappers (`pcall`, `xpcall`) can make Lua regrow its stack after a GC shrink — Lua-arena allocations, exempt.
- **What the hook cannot interrupt**: a single C library call runs to completion. `string.rep`/concatenation/`table.concat` are bounded by the memory cap (verified: `string.rep('x', 1<<28)` → `MemoryLimit`), `table.sort` comparators are Lua code (hooked). Lua pattern matching (`string.find/match/gmatch/gsub`) can backtrack super-linearly inside one C call; recursion is capped by Lua (`pattern too complex` at depth 200) but a crafted pattern on a long subject can still run for seconds. Logged to HUMAN_QUEUE.
- **Determinism caveat**: Lua 5.4 seeds its string hash per state (address + time, not configurable through `lua-src`), so `pairs` order over hashed keys and `tostring(table)` differ between runs; rules must not depend on them (same as the §3.4 parity rule). Host randomness and command queues are otherwise bit-identical for equal seeds/inputs.
- **Reload watcher**: `notify` watches the script's directory non-recursively (editors save by rename), filters by file name, reads + `syntax_check`s on change, suppresses unchanged contents, keeps the last syntax error in `last_error()`; `wait(timeout)` blocks on the channel for the I/O worker and tests (no sleeps); `force_check()` re-reads now.
- **`load_backend`** matches extensions case-insensitively; `.wasm` returns `UnsupportedBackend` naming Subphase 4.5.

## 2.1 — Review fix round 1 (guide cast)

**Decision (B1):** `GuideCurve::cast_sphere` now returns `Option<GuideCast>`: `None` (free for the whole window), `Contact(GuideHit)` (gap ≤ 10 µm **and** closing faster than 1e-5 × speed) or `Advance { time }` (free up to `time`, step budget of 32 exhausted: move without impulse and re-cast). A sphere moving along or away from the wall is never a contact. Steps use the larger of two proven lower bounds of the time to contact: conservative advancement (gap / speed) and, for level curves, a quadratic bound from the wall's reach ρ (`gap'' ≥ -|v|² / (ρ - d)`). ρ is estimated at build time from the sample table with Federer's pair formula (curvature and stretches of the curve approaching each other), times 0.9. A ball running alongside a wall now needs 1–3 steps instead of 64. Non-level or self-crossing curves have ρ = 0 and use conservative advancement only (correct, slower, may return `Advance`).
- **Sliding rule:** a sphere within the skin and not closing in may sink at most a further 10 µm while it slides; when a concave wall curves into its path that is reported as a `Contact` at the time it closes in (not `None` as before).
- **Alternative** (reviewer's suggestion as worded): bound by |v|²·κ_max only. Rejected as stated because curvature alone ignores a second stretch of wall nearby (hairpins); the reach covers both.
**N5:** `GuideCurve::closest_point` is exact for level curves (one spline query); for rising curves the wall-height/parameter alternation now runs to convergence (up to 24 rounds) and is documented as local to the curve's own closest point.
**N4 (for 2.2):** wireform rails have no continuous cast, and `Wireform::closest_on_rails` is a local Newton polish around the centerline's closest parameter. The ball solver must keep per-sub-step travel on a wireform below the ball radius (as `tests/guide_ramp.rs` does) or 2.2 must add a rail cast.
**N7:** `CubicSpline::closest_point` refines one root per sample interval (1/16 of a segment). Two local minima inside one interval can only happen where the curve bends on the scale of the interval; the returned distance is then still within the interval's half arc length of the true minimum, and in practice equal to it because the distance is nearly flat there. Authoring guidance: keep through-point spacing below the smallest radius of curvature.
**N1/N6:** the energy check compares against the running minimum (1e-5 relative allowance); the rail normal force is compared with m(g cos θ + v²κ)/cos φ over the whole checked ramp window; the brute-force cast proptest now also covers rotated cuboids, cylinders and triangles.

## Orchestrator — 4.2 review: patched vendored Lua

**Decision:** pin_script builds Lua from a patched copy of `lua-src =551.0.2` vendored at `third_party/lua-src/` and wired in with `[patch.crates-io]` in the root `Cargo.toml`. The patches are small, each marked `/* KHZ: ... */`, and listed in `third_party/lua-src/KHZ_PATCHES.md`:
1. **Fixed hash seed:** `luai_makeseed` returns a constant, so string hashing and `pairs` order are the same on every run (review B6, D9).
2. **Fixed sort pivot:** `l_randomizePivot` returns 0 (no `clock()`/`time()`), so `table.sort` is reproducible.
3. **String length cap:** creating a string longer than 64 KiB raises a Lua error, which bounds the cost of `..`, `==`, `string.rep`, `upper`, `find` and every other per-byte operation (review B4).
4. **Pattern step limit:** `lstrlib.c` matching counts steps per call and raises "pattern too complex" past a fixed limit, so backtracking patterns cannot stall a tick (review B1).
5. **No `<close>` variables:** the parser rejects the `<close>` attribute, because Lua runs their handlers with hooks off after a budget abort (review B2).

Library loops the instruction hook cannot see (`table.move`, `table.insert`/`remove` driven by `__len`, `table.concat`, `table.unpack`, `table.sort`) are charged against the instruction budget by input size, or reimplemented in Lua, in the sandbox prelude.
**Rationale:** CFLAGS in `.cargo/config.toml` would also reach SDL and every other C build, and function-like defines don't work on MSVC. Patching the source is portable, reviewable and testable on all three CI systems. Every abort stays an ordinary Lua error, which the sandbox already turns into a fault.
**Residual:** single opcodes on strings up to 64 KiB, such as `==` and `..`, are bounded by the cap times the instruction budget. The Implementer measures the worst case and records it here; if it exceeds a few ms, lower the default budget.

## Orchestrator — 4.2 pointer-keyed `pairs` order

**Decision:** Iteration order over tables whose keys are tables, functions or other reference values is not guaranteed to match across processes, because those keys hash by address. Rules scripts must use integer or string keys for anything whose iteration order affects commands. String and integer keys are deterministic, thanks to the fixed hash seed. The `pinball` table itself is read-only, like the name tables.
**Rationale:** Making address hashing stable would need a deterministic allocator or object IDs inside Lua, which is out of proportion for rules code. The parity rule (§3.4) already keeps rules to integers.

## 2.1 — Hardening round (guide cast sliding limit, closed guides, reach bound)

**N1 — fixed sliding limit.** `GuideCurve::cast_sphere` lets a sliding sphere sink to a gap of exactly −10 µm, independent of its starting gap, so repeated casts cannot ratchet deeper. Below the limit any sinking (`rate < 0`) is a `Contact`; at the limit with no possible progress the cast returns a resting `Contact` (time 0, closing speed ≈ 0). A sphere separating faster than 1e-5 × speed is always free. **Consequence for 2.2:** push the ball out with `closest_point` every sub-step and treat near-zero-closing-speed contacts as resting contacts. A caller that only zeroes normal velocity stays within the limit (tested: ≥ −2·skin over 640 sub-steps on a 150 mm arc) but is held there and creeps along a concave wall instead of rolling.
**N2 — closed guides.** Chosen: support them properly rather than only reject. New `CubicSpline::interpolate_closed` (last point == first, periodic end conditions, C² across the seam; cyclic system solved by Gauss–Seidel in f64) and `CubicSpline::is_closed`. `GuideCurve::new` rejects a spline whose first and last through-points coincide unless it is periodic (`SplineError::ClosedGuideNotPeriodic`), because a natural spline closes with a kink, has no reach, and would give a contact every ~10 µm of sliding. New error variants: `NotClosed`, `ClosedGuideNotPeriodic`. The parameter of a closed spline is still clamped, not wrapped. Guides that legitimately have no reach (rising curves, curves that touch or cross themselves) still work through conservative advancement only, with the documented cost; keep them out of rolling paths.
**N3 — reach estimate.** The curvature part is now a proven bound per sample interval (κ ≤ max|C''| / (min|C'| − ½h·max|C''|)², using that C'' is linear per segment). The pair (bottleneck) part is still sampled on every fourth sample with a 0.9 margin; a provable pair bound was not attempted.

## 4.2 — fix round 1 (review B1–B6, N1–N4)

Implements "Orchestrator — 4.2 review: patched vendored Lua". Patch list: `third_party/lua-src/KHZ_PATCHES.md`.
- **One budget for VM and C.** The instruction budget now lives in the patched `global_State` (`khzbudget`). The count hook spends its interval with `lua_khzspend`; library code spends by input size with `lua_khzcharge`, which raises "instruction budget exceeded" when the budget runs out and arms the count hook to fail on every instruction (so `pcall` cannot keep going). A callback that returns while the budget is negative is still a `BudgetExceeded` fault. Costs are in "instruction" units: long strings `len/64` per create/compare (`len/4` extra for per-byte case loops), 4 per table element moved, 2 per pattern step, `n/4` per vararg expansion, 64 per raised error, `n/16` per `string.rep` repetition of empty pieces (non-empty results over 64 KiB are refused up front with an ordinary, catchable error).
- **Table-lookup compares** have no `lua_State`, so `luaS_eqlngstr` adds its cost to a thread-local pending counter drained at the next spend (a state runs on one thread at a time; `lua_khzsetbudget` clears it).
- **64 KiB string cap** is a Lua error (→ `Runtime` fault), not a memory abort. String→number coercion and `tonumber` refuse strings over 200 bytes.
- **No `<close>`** (compile error in `syntax_check`, `new`, `apply_reload`); the generic-`for` closing value is a runtime error. **No weak tables** (`__mode`), in addition to no `__gc`.
- **`string.rep('', huge)`** faults (charged per repetition) instead of returning `""`, matching the Tester's expectation.
- **B3**: `__pairs` returns a closure over the hidden table and the proxy as state, so neither `pairs` nor `next` exposes a writable table.
- **B5**: the save copy charges 32 bytes per value plus string bytes against `memory_limit_bytes`; over the cap the state is dropped (reload proceeds without `on_restore_state`).
- **B6**: fixed hash seed + fixed sort pivot; `tostring` of tables/functions/userdata/threads is the bare type name; `string.format('%p')` raises. `pairs` over string/number/boolean keys is now reproducible across processes (tested with two child processes); over table/function keys it still depends on addresses — rules must not do that.
- **N1**: a failing entry-point lookup faults instead of being treated as "no function". **N2**: any third argument to `math.random` (even `nil`) raises "wrong number of arguments" (`Variadic` tail, which allocates only in that error case).
- **Rebuilds**: cargo tracks a path dependency through rustc's dep-info only, so the vendored `src/lib.rs` `include_str!`s the patched C files.
- **Residual, measured** (release, default config, worst tick of a script looping until the budget aborts): plain `while true` loop 3.2 ms; budget-many `==` on two equal distinct 64 KiB strings 1.2 ms; long-key table lookup 1.3 ms; `<` 1.3 ms; `s:upper()` 1.7 ms; `..` 2.5 ms; `string.format('%s', 64 KiB)` 4.5 ms; `pcall(error)` 3.1 ms; vararg expansion of 100k values 6.2 ms; table growth (`t[i] = i` in fresh tables) 7.8 ms; deep recursion until stack overflow 16 ms (one-off, ends in a fault). The string-specific residual is ~1 ms, so the default budget stays at the brief's fixed 1 000 000; the fact that 1 M plain instructions already take ~3 ms (> one 1 kHz tick) is queued for the humans.
- **Aborted callbacks are all-or-nothing.** When a callback faults, the commands it queued in that call are dropped (each list is truncated to its length at entry; `gi_tint` restored — no allocation), then the tilt releases are queued. Reason: the budget check has hook-interval granularity, so an abort can land after the script's last command (e.g. on the final `return`); without the rollback a script whose loop barely exceeds the budget would both fault and emit its result (Tester proptest `bounded_loops_complete_or_fault`). Commands from earlier, successful callbacks of the same tick are kept.

## Orchestrator — Lua instruction budget in the engine

**Decision:** `LuaConfig::default()` keeps the brief's 1,000,000 instructions per callback (tests and tools). The engine (Subphase 2.5, `pin_sim`) constructs the Lua backend with `instruction_budget = 100_000`. That is about 0.3 ms of plain Lua, which keeps a runaway callback's one-off overrun within the 8-tick catch-up window (§2.2). The budget is a table-overridable setting, in case a table needs more.
**Rationale:** 4.2 measured a 1M-instruction runaway at about 3 ms (worst 7.8 ms for table growth), longer than one tick. Real rules need a few thousand instructions per tick, and the Gate A2 soak checks the tick-time budget per table.

## Orchestrator — 4.2 re-review round 2

- **`string.pack`, `string.unpack` and `string.packsize` are removed from the sandbox** (B7). Rules scripts have no use for binary packing, and removing them is smaller and safer than charging every format option.
- **Stall tolerance (B9):** the worst single tick a script can cause before it faults must be at most **8 ms at the engine budget of 100,000 instructions**, which is the §2.2 catch-up window, measured in release. The fix is a flat charge per C-function call plus extra charges for expensive conversions, float `fmod`, `tonumber`/`tostring` and error raising, as the reviewer proposed.
- **Rollback must never strand a held coil** (B8): if a faulting callback's dropped commands include hold releases, those coils still count as held, so the tilt release emits them.

## 4.2 — fix round 2 (review B7–B9, N5–N8)

Implements "Orchestrator — 4.2 re-review round 2". Patch list (now 17 entries): `third_party/lua-src/KHZ_PATCHES.md`.
- **B7**: `string.pack`, `string.unpack`, `string.packsize` are removed from the sandbox. `string.format` charges `len(fmt)/4` plus per item: 256 for `%a %e %f %g`, `len/16` for `%s`, `2·len` for a `%q` string (256 for a `%q` number).
- **B8**: rolling back a faulting callback marks every coil named by a dropped hold command as held, so the tilt release emits `(coil, false)` for it (a superset: an extra release of a coil that was off is harmless). Test `rollback_never_strands_a_held_coil`.
- **B9**: flat 32 per C-function call (`precallC`; covers every library and `pinball.*` host function), 1024 per raised error (`luaG_errormsg`; covers the mlua error-construction cost of host-function errors), 256 per float `%`/`math.fmod`/float→string, 16 per integer→string, `len + 16` per `tonumber`/string-arithmetic parse (`4·len + 16` with a base), and `len + 16` added to the thread-local pending cost by the VM's string→number coercion (bitwise ops on strings). `collectgarbage` stays "count"-only (anything else is an error, which costs 1024).
- **N5**: `collectgarbage` compares the option's bytes (no Rust `String`). **N6**: a rollback restores `overflow_count`. **N7**: end-to-end reload test with 100 and 20 000 references to a 65 535-byte string. **N8**: KHZ_PATCHES.md rewritten.
- **Measured worst tick at `instruction_budget = 100_000`, release** (Reviewer's attack scripts, 8 ticks each, multi-tick set-ups 30 ticks): every row of the round-2 table and of round 1 is ≤ 8 ms. Worst rows: `n_gsubbuf` 4.8 ms, `n_gsubbuf2` 4.8 ms, `n_concatbuf` 4.8 ms (buffers growing to the memory cap), `deeprec` 3.4 ms, `m_errdeep` 2.8 ms, `m_fmtf` (`%99.99f` of 1e300) 2.2 ms, `m_closure` 2.0 ms, `n_byte` 1.7 ms; everything else < 1 ms, e.g. `m_fmodop` 0.31 ms, `m_fmtg` 0.35 ms, `m_mathf` 0.36 ms, `m_tostrn` 0.18 ms, `m_tonum` 0.10 ms, `m_apierr` 0.78 ms, `m_cgerr` 0.67 ms, `m_rnderr` 0.64 ms, `m_rnd` 0.30 ms, `m_api` 0.13 ms, `m_pow` 0.77 ms, `m_base` (plain loop) 0.26 ms; round-1 rows: equal 64 KiB `==` 0.23 ms, long-key lookup 0.31 ms, `upper` 0.21 ms, `..` 0.98 ms, `%q` of 64 KiB `\0` 0.02 ms, 64 KiB format literal 0.28 ms, table growth 0.70 ms, `sortbig` 0.89 ms, `n_nextscan2` 0.87 ms. Set-ups that exceed 100 000 at load (`n_sortcmp`, `n_tostr`, 200k-element sort, 100k unpack) fail `new` with `BudgetExceeded` within 1.8 ms.

## Orchestrator — 4.2 re-review round 3: engine script config and stall target

- **Engine script config:** the engine (Subphase 2.5) runs rules scripts with `instruction_budget = 100_000` and `memory_limit_bytes = 2 MiB`. `LuaConfig::default()` stays at the brief's 1M and 16 MiB for tools and tests. Rules scripts hold scores, modes and small tables; 2 MiB is ample. It also bounds GC and rehash work that the instruction budget cannot see (review B11, B14).
- **Stall target, binding for 4.2:** under the engine config, in a release build, **every** attack script (faulting or not) keeps every tick at or under 8 ms. Measure with multi-tick set-ups sized to the engine budget (the reviewer's `r_*` scripts), not the 1M-budget set-ups.
- **Charging belongs in central places:** buffer growth in `lauxlib`, which raises the string-cap error as soon as a buffer passes 64 KiB and charges by size; table traversal and resize in `ltable`; emergency full GC in `lgc`; and the `string.rep` loop. That way one fix covers every library function that uses them.
- **Failure policy reminder (D29):** this is the Implementer's third and last fix attempt. If it fails, the other model gets one attempt from a fresh brief, then 4.2 is marked blocked.

## 4.2 — fix round 3 (review B10–B14, N9, N10)

Implements "Orchestrator — 4.2 re-review round 3". All charging is central (KHZ_PATCHES.md rows 17–21):
- **B10** `luaH_next` charges the slots it skips (/8). **B14** `luaH_resize` charges (old + new sizes)/16. **B11** every full GC (also emergency GCs inside allocations) charges heap bytes/64. These three cannot raise where they run (allocation retries, `lua_next` from the host), so they use a *deferred* charge: the budget is debited and, if it went negative, the count hook is armed to abort at the next instruction.
- **B13** `luaL_Buffer` growth stops at the string cap (+1 KiB slack for one formatted item) with "string length exceeds the 64 KiB limit", raised as soon as a buffer would pass it, and charges new size/64 — one place for `table.concat`, `gsub`, `format`, `rep`, `string.char`, `utf8.char`.
- **B12** `string.rep` copies by doubling and charges length/64 + 1 (no per-repetition loop left).
- **Engine config**: `LuaConfig::engine()` = 100 000 instructions, 2 MiB — the one source of truth for Subphase 2.5. `LuaConfig::default()` stays 1 M / 16 MiB.
- **N10** regression tests (tests/lua_backend.rs): `next_charges_skipped_slots`, `emergency_gc_is_charged`, `string_rep_is_charged_by_size` (also checks the doubling copy's output), `buffers_stop_at_the_string_cap`, `table_resize_is_charged`, `engine_config_values`.
- **N9 — measurement.** Release build, `LuaConfig::engine()`, Reviewer's harness (on_tick + one switch edge per tick), 300 ticks per script. Every script in `atk/lua` (round 1, `m_*`, `n_*`, `r_*`) as given; scripts whose set-up only fits the old 1 M budget were also re-run with set-ups split into ≤ 8 000-iteration ticks sized to fit 2 MiB, following the `r_*` pattern (rows marked "split set-up"; e.g. `s_sortcmp`, `s_tostr`, `s_sortbig`, `s_lenscan`, the round-1 string rows). The 800 k / 400 k-entry set-ups of `n_nextscan*`/`sortbig` cannot exist under 2 MiB and end in `MemoryLimit` during set-up; `r_nextscan`/`r_pairsscan` cover the attack at sizes that fit. "fault" = the script faulted (any cause) at some tick; "runs" = never faulted. **Worst tick of all 163 rows: 4.68 ms (`m_errdeep`); none above 8 ms.**

| Script | Worst tick (ms) | Outcome |
|---|---|---|
| `ccost` | 0.268 | new() fails: Load (64 KiB cap) |
| `ccost2` | 0.274 | new() fails: Load (64 KiB cap) |
| `close` | 0.210 | new() fails: Load (no <close>) |
| `close2` | 0.434 | new() fails: Load (no <close>) |
| `close3` | 0.199 | new() fails: Load (no <close>) |
| `close4` | 0.251 | new() fails: Load (no <close>) |
| `close5` | 0.222 | new() fails: Load (no <close>) |
| `close6` | 0.201 | new() fails: Load (no <close>) |
| `concatop` | 0.299 | new() fails: Load (64 KiB cap) |
| `coro` | 0.029 | runs |
| `deepconcat` | 0.124 | fault |
| `deepgsub` | 0.358 | fault |
| `deepmeta` | 0.117 | fault |
| `deeppcall` | 0.159 | runs |
| `deeprec` | 2.804 | fault |
| `det` | 0.044 | runs |
| `eph` | 0.264 | new() fails: Load (__mode) |
| `eqop` | 0.258 | new() fails: Load (64 KiB cap) |
| `errobj` | 0.311 | fault |
| `fmt` | 0.050 | fault |
| `fmtp` | 0.021 | fault |
| `gc1` | 0.164 | runs |
| `gc2` | 0.028 | fault |
| `lo` | 0.024 | runs |
| `m_api` | 0.085 | fault |
| `m_apierr` | 0.750 | fault |
| `m_arith` | 0.116 | fault |
| `m_base` | 0.270 | fault |
| `m_bigtab` | 0.775 | fault |
| `m_cg` | 0.237 | fault |
| `m_cgerr` | 0.767 | fault |
| `m_closure` | 1.975 | fault |
| `m_err` | 0.044 | fault |
| `m_errdeep` | 4.684 | fault |
| `m_fmodop` | 0.341 | fault |
| `m_fmta` | 0.061 | fault |
| `m_fmtf` | 2.367 | fault |
| `m_fmtf2` | 0.597 | fault |
| `m_fmtg` | 0.477 | fault |
| `m_fmtlit` | 0.335 | fault |
| `m_fmtlit2` | 0.224 | new() fails: Load (64 KiB cap) |
| `m_fmtq` | 0.048 | fault |
| `m_gsubf` | 0.581 | fault |
| `m_gsubn` | 0.254 | fault |
| `m_mathf` | 0.337 | fault |
| `m_newtab` | 1.170 | fault |
| `m_packc` | 0.052 | fault |
| `m_packz` | 0.032 | fault |
| `m_pow` | 0.818 | fault |
| `m_rnd` | 0.336 | fault |
| `m_rnderr` | 0.632 | fault |
| `m_setlist` | 0.176 | fault |
| `m_sub` | 0.381 | fault |
| `m_tonum` | 0.104 | fault |
| `m_tonum2` | 0.166 | fault |
| `m_tonum3` | 0.060 | fault |
| `m_tostrn` | 0.169 | fault |
| `m_unpk` | 0.048 | fault |
| `membig` | 0.021 | fault |
| `mempcall` | 0.066 | fault |
| `memxp` | 0.312 | fault |
| `n_after` | 0.287 | fault |
| `n_byte` | 0.665 | fault |
| `n_cmp` | 0.139 | fault |
| `n_concatbuf` | 0.098 | fault |
| `n_fmtbuf` | 0.090 | fault |
| `n_gmatch` | 0.200 | fault |
| `n_gmatch2` | 0.062 | fault |
| `n_gsubbuf` | 0.194 | fault |
| `n_gsubbuf2` | 0.147 | fault |
| `n_holdroll` | 0.029 | fault |
| `n_lenscan` | 1.014 | fault |
| `n_lit` | 0.379 | new() fails: Load (64 KiB cap) |
| `n_litcat` | 0.581 | new() fails: Load (64 KiB cap) |
| `n_nextscan` | 1.984 | fault |
| `n_nextscan2` | 0.642 | fault |
| `n_pack` | 0.057 | fault |
| `n_pack2` | 0.030 | fault |
| `n_sortcmp` | 1.569 | new() fails: BudgetExceeded |
| `n_tblkey` | 0.299 | fault |
| `n_tostr` | 1.149 | new() fails: BudgetExceeded |
| `n_utf8char` | 1.556 | fault |
| `next2` | 0.163 | runs |
| `ok` | 0.010 | runs |
| `pairsleak` | 0.024 | fault |
| `pattern` | 0.235 | fault |
| `r_bigctor` | 1.305 | fault |
| `r_closure` | 2.888 | fault |
| `r_concat80` | 0.115 | fault |
| `r_errmsg` | 0.030 | fault |
| `r_errmsg2` | 0.766 | fault |
| `r_forvars` | 0.490 | fault |
| `r_gcfull` | 1.004 | fault |
| `r_gcfull2` | 1.054 | fault |
| `r_gcfull2b` | 0.971 | fault |
| `r_gcfull3` | 0.975 | fault |
| `r_gcfull4` | 1.091 | fault |
| `r_gcfull4b` | 1.015 | fault |
| `r_gcfull5` | 1.012 | fault |
| `r_gcfull6` | 0.915 | fault |
| `r_gcsmall` | 0.301 | fault |
| `r_gcsmall2` | 0.304 | fault |
| `r_gcsmallb` | 0.395 | fault |
| `r_grow` | 1.075 | fault |
| `r_growarr` | 1.076 | fault |
| `r_gsub80` | 0.260 | fault |
| `r_gsub80t` | 0.120 | fault |
| `r_holdfull` | 0.031 | fault |
| `r_holdvar` | 0.030 | fault |
| `r_len` | 1.366 | fault |
| `r_ltmeta` | 0.208 | fault |
| `r_nextscan` | 0.464 | fault |
| `r_pairsscan` | 0.479 | fault |
| `r_rehash` | 1.296 | fault |
| `r_rehash2` | 1.709 | fault |
| `r_rehash3` | 1.267 | fault |
| `r_rep1` | 0.350 | fault |
| `r_rep1b` | 0.287 | fault |
| `r_rep80` | 0.037 | fault |
| `r_schar` | 1.113 | fault |
| `r_selneg` | 0.373 | fault |
| `r_selunpack` | 0.123 | fault |
| `r_stackgrow` | 0.393 | fault |
| `r_strkey` | 1.347 | fault |
| `r_tailargs` | 0.424 | fault |
| `r_tostr_tab` | 3.515 | fault |
| `r_utf8` | 1.084 | fault |
| `rep_empty` | 0.018 | fault |
| `savebomb` | 0.225 | new() fails: Load (64 KiB cap) |
| `sortbig` | 0.595 | fault |
| `strmeta` | 0.026 | fault |
| `tconcat` | 0.022 | fault |
| `tinsert` | 0.021 | fault |
| `tmove` | 0.030 | fault |
| `tremove` | 0.023 | fault |
| `tunpack` | 0.019 | fault |
| `s_bitstr` (split set-up) | 0.023 | fault |
| `s_byte` (split set-up) | 0.475 | fault |
| `s_cat` (split set-up) | 0.313 | fault |
| `s_concat` (split set-up) | 0.492 | fault |
| `s_eq` (split set-up) | 0.249 | fault |
| `s_err` (split set-up) | 0.031 | fault |
| `s_findp` (split set-up) | 0.178 | fault |
| `s_fmt` (split set-up) | 0.193 | fault |
| `s_fmtlit` (split set-up) | 0.229 | fault |
| `s_fmtq` (split set-up) | 0.019 | fault |
| `s_gsub` (split set-up) | 0.317 | fault |
| `s_gsubbig` (split set-up) | 0.147 | fault |
| `s_key` (split set-up) | 0.308 | fault |
| `s_lenscan` (split set-up) | 0.588 | fault |
| `s_lt` (split set-up) | 0.147 | fault |
| `s_rehash` (split set-up) | 0.668 | fault |
| `s_rep` (split set-up) | 0.246 | fault |
| `s_repsep` (split set-up) | 0.202 | fault |
| `s_sortbig` (split set-up) | 0.586 | fault |
| `s_sortcmp` (split set-up) | 0.555 | fault |
| `s_sortloop` (split set-up) | 0.675 | fault |
| `s_sub` (split set-up) | 0.274 | fault |
| `s_tonum2` (split set-up) | 0.013 | fault |
| `s_tostr` (split set-up) | 0.667 | fault |
| `s_unpackv` (split set-up) | 0.822 | fault |
| `s_upper` (split set-up) | 0.201 | fault |
| `s_utf` (split set-up) | 0.226 | fault |
