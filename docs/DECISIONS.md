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
