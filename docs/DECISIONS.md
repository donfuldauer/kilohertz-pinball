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

## Orchestrator — Wave 3 brief decisions

- **New dependency pins.** Wave 3 needs crates that Subphase 0.1 did not spike: `zip =9.5.1` (default features off, `deflate` only; 4.1), `hound =3.5.1` and `lewton =0.10.2` (WAV / Ogg Vorbis; 3.3), `bytemuck =1.25.2` with `derive` (1.4 vertex data). They are pinned in the root `[workspace.dependencies]` only. `versions.toml` is left unchanged because `spikes/dep_smoke/tests/versions.rs` requires every `[crates]` pin there to appear in the spikes lockfile; each consuming subphase gate validates its new crate instead.
- **`SimStep` lives in `pin_core::sim` (CR-002, additive, made by 1.4).** The orchestrator (pin_app) drives it and pin_sim (2.5/6.0) implements it, so it crosses crates. `step_1ms(tick, inputs, out)`: tick is 1-based, inputs are the events with timestamp ≤ the end of the tick, the impl writes the whole snapshot.
- **Present-mode policy (1.4).** Playfield: `FifoRelaxed` only when VRR is requested and offered, else `Fifo`. Secondary surfaces: `Mailbox`, else `Immediate`, else `Fifo` on a presenter thread; an empty capability list counts as `[Fifo]`.
- **Ball solver API (2.2).** One `step(geo, dt, events)` per 1 ms tick with CCD sub-steps inside; static-collider material defaults to wood and guide/wireform material to steel, settable per collider; magnets act only within their `range`; an `ExternalForce` hook list (empty = bit-identical core solver) is the 2.7 seam.
- **Input profiles (3.2).** `pin_input` stays SDL-free: profiles bind raw inputs described with SDL's numbering (scancodes, standard gamepad buttons/axes, 16-byte GUIDs) to cabinet inputs; built-in profiles are TOML files. A keyboard cannot be identified, so keyboards get the generic preset unless a user picks the X-Arcade keyboard preset.
- **Audio mixer (3.3).** Mixing is pure Rust and offline-testable; SDL is a thin output shell. Voice stealing takes the oldest voice on the full bus; ducking is a linear ramp (attack 30 ms to 0.35, release 250 ms) applied to music and sfx while any callout plays.
- **Asset ID interning (4.1).** Switches, coils, lamps, magnets and dispensers use their declared `number` as id; flippers, diverters, sounds, scenes and persist keys use declaration order; profiles use the index in `BUILTIN_PROFILE_NAMES`. A flipper is `Upper` when staged from `left_upper`/`right_upper` or on a layer ≠ 0. Near-closed guides (ends within 1e-6 m) are snapped closed; uneven spline spacing (> 3:1) is a warning.
- **Wasm ABI (4.5).** Core-Wasm imports under `pinball` mirroring the Lua API with i32 ids and booleans, f32 unit values, i64 scores/persist; names resolve through `id_of(kind, ptr, len)`; `random(m, n)` draws the same PCG sequence as Lua. The guest SDK is its own `no_std` crate `crates/pin_script/guest-sdk` (`pin_guest_sdk`); CI has no wasm32 target, so tests use committed `.wasm` fixtures or WAT.
- **Shared tree, shared target dirs.** Each subphase's Implementer and Tester share one `target/sp_X_Y` directory to save disk; it is deleted when the subphase passes review.

## 3.2 — X-Arcade presets, profile format, plunger and nudge calibration

- **X-Arcade presets (UNVERIFIED ON HARDWARE).** `xarcade_keyboard` (Mode 1 / MAME layout): side buttons C (scancode 6) and 5 (34) plus LShift/RShift = left/right flipper, P1 button 1 LCtrl = left upper, P2 button 1 A = right upper, P1 button 2 LAlt = action, P1 button 3 Space and Return = launch, P1 start 1 = start, P2 button 2 S = tilt; the P1 stick (arrows and keypad 4/6/8) nudges left/right/forward. It matches `keyboard` but is never auto-picked: built-ins are ordered generic first, so `detect` gives keyboards `generic_keyboard`. `xarcade_gamepad` (DirectInput gamepad mode) matches vendor 0xAA55, products 0x0101 and 0x0102: shoulders = flippers (the triggers are left unbound: the side buttons are assumed to map to the shoulders, and a second binding on the same cabinet input would race with it, review N5), West/North = upper flippers, South = launch, East = action, Start, Back = tilt, d-pad = nudge. In XInput mode the device reports as an Xbox 360 pad (045e:028e) and gets `generic_gamepad`; matching that id would hijack every Xbox pad.
- **Generic presets confirm 1.3.** `generic_keyboard` and `generic_gamepad` reproduce `pin_app::platform::keymap` exactly (tests compare every scancode 0..600, every button, and the axis formula over the i16 range). This answers the 1.3 HUMAN_QUEUE item for the gamepad map (the agent keeps 1.3's choice; still subject to human confirmation).
- **Profile format details.** A cabinet input or nudge direction takes one raw spec or a list; `key:` accepts decimal or `0x` hex scancodes of any u32; a raw input bound twice within `[buttons]` (or within `[nudge]`), a raw axis bound twice in `[[axes]]`, or one bound both in `[[axes]]` and as a button is an error. `axis:<name>` in `[buttons]`/`[nudge]` is a digital input pressed at normalised value ≥ 0.5. `from` defaults to the raw range's low end (−1 stick, 0 trigger), `to` to 1; `from == to` is an error; numbers must be finite with |x| ≤ 1e6. `invert` mirrors the raw value within its own range (sticks −v, triggers 1 − v) before the `[from, to]` mapping. Syntax errors display `line:col: message` (from the TOML parser span); semantic errors display `<dotted key>: <message> (line L, column C)`. Files over 64 KiB (`MAX_PROFILE_BYTES`) are rejected as `profile: file is N bytes, ...`; duplicate detection is hash-based (review N4: no quadratic blow-up on large untrusted files).
- **Plunger filter (revised after reviews B1/N1–N3, B2/N-a–N-d and B3/N-f–N-h).** Full model in the `calibration/plunger.rs` module docs. *Pulls:* a pull starts when raw − rest > deadzone (position reported from then on) and can end in a release only if it qualifies: the filter is *armed*, the pull reached max(2.5 × deadzone, 0.05) (`PULL_QUALIFY_DEADZONES`, `MIN_PULL_DEPTH`) and spent ≥ 3 updates at that depth (`MIN_PULL_TICKS`, counted at qualifying depth since review N-k: 1–2-sample spikes are noise, a 2 ms/2 ms tap still launches). *Arming (B3):* the filter arms after 50 ms continuously at rest (`ARM_REST_MS`) and stays armed through pulls that end without a release, so deadzone-edge chatter, sensor noise and hesitant starts never disarm a launch; it disarms only on a release (spring bounce), a reading more than a deadzone below rest (push-in or overshoot: the spring-back must settle) and a rest re-acquisition. The release is reported on the tick the plunger is back within the deadzone if its speed ≥ `release_min_speed` (default 3/s); then no pull starts for 200 ms (`RELEASE_HOLDOFF_MS`). *Release speed (B3):* the return stroke is the last 90 % of travel from the furthest position within `release_window_ms` (default 100) to the release tick; its peak was the maximum of finite differences over a baseline of a quarter of the stroke (1–8 updates), with the least-squares slope reported for strokes straight within the noise — *superseded (review B4):* that estimator read curved noise-free strokes 1.5–44 % low (not "within ~2.5 %" as first claimed here); see "3.2 — plunger release speed, pull ticks and offset rests (other-model attempt)" below for the current estimator. Measured over 200 seeds per cell (2 s rest, pull 0.3/0.4/1/3 s to 0.1/0.4/0.8, 10–20 ms linear snap, tracking 1 and 0; `tests/calibration_noise.rs`): 0 launches lost in every cell for uniform noise ±0.001/±0.002/±0.004/±0.008 and 8-/10-bit ±1 LSB dither; speed within 5 % in every cell where depth ≥ 200 × noise amplitude, and within max(5 %, 12 × amplitude / depth) elsewhere (worst: ±0.001 7.8 %, ±0.002 15.5 %, ±0.004 31 %, ±0.008 62 %, 8-bit 57 %, 10-bit 17 %, all at depth 0.1). 5 % on a 0.1-deep, 10–20 ms stroke is not reachable at those noise levels by any estimator: even an ideal straight-line fit over the 8–16 samples before the deadzone has a 1σ error of ~3.5 % at ±0.008. When a pull qualifies, the rest tracking of the 128 updates before it is undone. *Rest (all adaptation off when `rest_tracking_per_s` = 0):* tracking at the configured rate on ticks not in a pull and within ±deadzone; rest bounded to ±0.25 (`MAX_REST_OFFSET`). Re-acquisition uses a stable-signal detector (every reading within a band one deadzone wide for 1 s, `STABLE_MS`; level = 50 ms average). Upward: small offsets (< qualifying depth), known-good rests, or ≤ 0.25 in the 3 s power-on window before the first release. Downward: known-good rests after 1 s; any other level (the knob held pushed in) only after 5 s (`PUSH_IN_REACQUIRE_MS`), remembering the rest it left. Known-good rests: nominal 0, the last rest a release returned to or that was acquired upward, and the rest the last downward move left (tolerance max(2 × deadzone, 0.01)). Levels beyond ±0.25 never become rest; re-acquisition cancels a pull without a release. *Why it never gets stuck (B2):* a rest that is too low is the only state that blocks launches (the return never re-enters the deadzone). Rest only goes down to a known-good level or after a 5 s push-in, and every downward move remembers the rest it left, which is restored after 1 s back there; nominal 0 is always known-good. A release while a pushed-in rest is in force does not overwrite the known-good rest (N-f), and within the power-on window a level below nominal (down to −0.25) is acquired after 1 s like one above (N-g). So from any history, once the plunger sits still at a known-good true rest (nominal 0, the power-on rest, the last launch rest, or a rest < 0.05 above the current one) for ≤ 2 s (1 s detection + 50 ms arming), the next normal launch reports exactly one release; randomized tests check this over arbitrary histories, with and without ±0.004 noise on every sample. *Bounds (accepted):* a true-rest step of ≥ 0.05 upward after the power-on window, to a level never seen as rest, is indistinguishable from a held pull and stays one until the plunger returns to a known-good rest (letting go of it reads as a weak launch). A held pull ≤ 0.25 kept still for 1 s in the first 3 s after power-on, before any launch, is taken as the rest; if the true rest is not known-good, correcting that takes 5 s. A shallow push-in within max(2 × deadzone, 0.01) of a known-good rest moves the rest there after 1 s and back 1 s after release. N-b: bounces that never settle 50 ms at rest cannot qualify, but a clean 6 % excursion after ≥ 200 ms + 50 ms of stillness is a new (weak) tap by construction. N-d: very slow shallow pulls (≈ 0.055–0.07 over 3 s) drag the tracked rest along and do not qualify, so they do not launch; accepted (a launch that weak is a dribble). Raw readings are clamped to ±16 full-scale. Position = (raw − rest − dz)/(1 − rest − dz) clamped to 0..1. N-h: re-acquisition needs the rest noise below one deadzone peak-to-peak (e.g. ±0.011 with the default 0.02 deadzone never counts as stable, so a rest moved by a long push-in is not restored); such sensors still get tracking within the deadzone, and need a larger deadzone.
- **Nudge filter.** First-order high-pass discretised with the bilinear (Tustin) transform, with the pole clamped at ≥ 0 so a sample gap longer than 2RC (a stall) cannot make it ring (review N6), so the step decays at least as fast as e^(−t/RC) and 10× cut-off passes with gain ≈ 0.99 even at 60–240 Hz sampling. The deadzone is a hard per-axis cut (no rescale) so the brief's gain-curve formula holds verbatim beyond it; defaults: 2 Hz, gain 1, exponent 1, deadzone 0.01, max 1. `DigitalNudge` keeps up to 16 presses (oldest replaced); `thumbstick_nudge` rescales the radius from `deadzone..1` to `0..1`.

## 4.5 — Wasm backend: sandbox, state protocol and load-time rules

- **Engine.** One process-wide `wasmtime::Engine` (fuel on, NaN canonicalisation on, threads/shared memory, relaxed SIMD (also set deterministic), memory64, multi-memory, GC, function references, exceptions, stack switching, component model, custom page sizes, wide arithmetic and parallel compilation off) and one process-wide `Linker` with the `pinball` imports, so `prepare(bytes, cfg)` (FIXED API takes the config, not an engine) can compile on any thread and any backend can instantiate the result. wasmtime keeps its default cargo features: the workspace pin enables them and a member cannot turn default features off.
- **Required exports.** `on_init`, `on_tick`, `on_switch_edge` and `memory` are required (the brief lists `memory` among the guest exports); `save_state`, `restore_state`, `alloc_state` are optional; a present export of the wrong type is a `Load` error. Imports other than the 18 `pinball` functions with their exact signatures (including memory/table/global imports) are `Load` errors.
- **Memory cap.** `memory.grow` past `memory_limit_bytes` returns −1 to the guest (no trap). A declared initial memory above the cap is rejected at load with `ScriptError::MemoryLimit` (as a Lua top level exceeding the cap is). Tables: at most 4, each ≤ 65 536 elements (`Load` error otherwise). One memory, one instance per store.
- **Start function.** Runs during `new`/`apply_prepared` under a fresh fuel budget: out of fuel → `BudgetExceeded`, any other trap → `Load`. Only `id_of` works there and in the state hooks; the context imports trap ("only available inside on_init, on_tick and on_switch_edge"), mirroring the Lua API at top level and in reload hooks.
- **State protocol.** Order as in Lua: instantiate new → old `save_state()` (skipped when faulted; a trap, blob > 64 KiB or out-of-bounds blob means "no state") → new `alloc_state(len)`, host copies blob, `restore_state(ptr, len)`. An empty blob is delivered as `restore_state(0, 0)` without `alloc_state`. Without `alloc_state` a non-empty blob is not delivered. A trap / fuel exhaustion in `alloc_state`/`restore_state` or an out-of-bounds pointer fails the swap and leaves the old instance running. Fault count, last error, overflow count and held-coil tracking carry over a swap (Lua's bridge is shared across reloads too).
- **`id_of` cost.** One fuel unit per name byte plus one, charged after the bounds check (an out-of-bounds name is a `Runtime` trap, not a budget fault).
- **Guest fixture.** `crates/pin_script/tests/fixtures/wasm/parity_guest` is a standalone cargo workspace (own `[workspace]`), built by `rebuild.sh` with `--remap-path-prefix` so `rules.wasm` is byte-identical regardless of checkout path; `pin_guest_sdk` spells out its package fields instead of inheriting them so such standalone crates can depend on it by path.

## 3.3 — Audio mixer: semantics left open by the brief

- **Voice age and steal ties.** *(Superseded by "3.3 — fix round 1": age is trigger arrival order.)* A voice's age was its start frame, with same-call ties stealing the lowest slot. New voices take the lowest free slot. A bus with `max_voices = 0` ignores its triggers (not counted as stolen). Zero-length samples start no voice.
- **Duck timing.** Frame `k` is mixed with the current multiplier, then the multiplier steps by `|1 − duck_gain| / ramp_frames` toward its target (attack while a callout voice played frame `k`). So a callout starting at frame 0 gives `d(k) = 1 − (1 − g)·k/A` on output frame `k`, and `duck_level()` after `k` frames equals `d(k)`. The state is `f64`. A 0 ms (or non-positive) ramp jumps in one step; a non-finite `duck_gain` disables ducking.
- **Clipping.** Hard clip to [−1, 1] after master gain (NaN → 0); no soft limiter (exact, stateless, keeps mixing tests exact).
- **Decode.** Integer PCM `v / 2^(bits−1)` in f64 rounded once; linear resampling `n_out = ceil(n_in·dst/src)`, `y[j] = x[i](1−f) + x[i+1]f` with `i = floor(j·src/dst)`, `f = (j·src mod dst)/dst`, `x` clamped at the last frame. Mixer rate outside 1..=384 000 → `Unsupported`; truncated or corrupt files → `Malformed` (no partial decode); third-party decoders run under `catch_unwind`. Vorbis output is trimmed to the last page's granule position (lewton leaves single-page files untrimmed). The 10-minute cap applies at the source and the mixer rate.
- **Closing SdlOutput.** `close` pauses the stream, locks it (waits for an in-flight callback) and takes the mixer out of the callback; `Drop` also pauses first, because sdl3 0.22.1 frees the callback box before destroying the stream.
- **Build environment.** SDL's static build needs the X11 dev headers listed in CI (`libxcursor-dev`, `libxrandr-dev`, `libxfixes-dev`, `libxi-dev`, `libxss-dev`, `libxtst-dev`, `libxkbcommon-dev`); these were missing on the agent machine and were installed with apt.

## 1.4 — wgpu multi-surface context, thread orchestrator & debug renderer

**Decision:**
- **CR-002 (`pin_core::sim`, additive).** Besides the fixed `SimStep`/`TICK_NS`/`MAX_CATCH_UP_TICKS`, adds `impl SimStep for Box<S: SimStep + ?Sized>` (so `Box<dyn SimStep>` drives a `SimLoop`) and `tick_end_ns(epoch, tick)` (saturating). `out` holds the previous tick's snapshot on entry; impls must still write the whole state.
- **SimLoop timing.** Tick `k` ends at `epoch + k·TICK_NS` (epoch 0 for `run_ticks` as specified; the real-time sim thread sets the epoch to the clock reading at start). Events are taken FIFO while the head's timestamp ≤ the tick end (an out-of-order later event blocks earlier ones behind it — SDL timestamps are monotone). More than 256 due events in one tick: the rest stay queued for the next tick (counted, `input_overflow_ticks`). The sim's `out.tick` is published as written (not forced), so `run_headless` really checks the sim. When the catch-up cap drops ticks, the epoch moves forward by the dropped ticks (`skip_ticks`) so inputs keep mapping to wall time.
- **SimClock.** `advance` counts whole ticks from the start (remainder kept relative to `start_ns`), returns ≤ 8, drops the excess; `next_due_ns()` gives the next tick boundary for the hybrid timer.
- **Real-time path.** `HybridTimer` sleeps until `spin_ns` (300 µs default) before the deadline, then spins; the sim clock is a `fn() -> u64` (`SDL_GetTicksNS` in the app so input timestamps share the clock; `monotonic_now_ns` (std `Instant`) in tests). Measured drift: 0.052 % over 10 s (debug opt-level 1, loaded 4-core VM, includes 2 start-up drops).
- **Threads.** `spawn_threads(ThreadsConfig) -> Threads` starts `sim`, `render`, `audio`, `io`; audio/io park (100 ms slices) until shutdown; `Threads::shutdown` sets a flag, unparks and joins (reports panicked threads); `Drop` does the same. The render thread without surfaces only reads snapshots/window events and parks `idle_period` (4 ms) per iteration; with GPU it creates the adapter/device itself from the instance the main thread used for the surfaces. A GPU setup failure is logged and rendering disabled (the app keeps running).
- **Surfaces.** Created on the main thread (`gpu::create_surface`, raw handles via sdl3's raw-window-handle) and moved to the render thread (`SurfaceSource`); dropped on the render thread before main destroys the windows. Only native SDL drivers get surfaces (dummy/offscreen panic in sdl3's raw-window-handle impl, 0.1 Q4). Format: `Bgra8Unorm`/`Rgba8Unorm` when offered (debug colours exact), else the first. Secondary surfaces redraw on first frame, resize, expose/restore and `request_redraw`; for the presenter-thread case the render thread draws into an `Rgba8Unorm` texture and hands it to the presenter thread (sync channel of 1), which blits it into the swap-chain image; while busy, redraws stay pending. PiP viewports get a grey placeholder outline until real backglass/score content exists. `SurfaceOptions::force_presenter_thread` exists to exercise that path on adapters offering Mailbox (used by the window smoke test).
- **GpuOptions::from_env.** `WGPU_BACKEND` = comma list of vulkan|dx12|metal|gl (aliases vk, d3d12, mtl, gles, opengl); default `Backends::PRIMARY`. Fallback adapter forced when `CI` is true/1 or `PIN_REQUIRE_GPU=1`, except Metal-only; `PIN_FORCE_FALLBACK_ADAPTER=0|1` overrides. `AutoVsync`/`AutoNoVsync` in an offered list are ignored by the policy (they are wgpu aliases).
- **Debug lines.** Lines are per-instance vertex data (`DebugLine` is the 40-byte vertex), `LineList`, no MSAA/blend. `Camera` is column-major (WGSL `mat4x4`), ortho ignores world Z (depth 0.5). `snapshot_lines` draws only ball circles (16 segments, white); flippers are skipped because the snapshot has no pivot/length; `ball_axes_lines` draws an orientation cross separately. `DebugRenderer::draw` uploads with `write_buffer`, so one draw per renderer per submit.
- **Regression snapshot.** `crates/pin_render/tests/fixtures/debug_scene_256.txt` lists lit pixels of a composite scene (recorded on lavapipe after the analytic checks passed). Tolerance: a pixel matches if a reference pixel within 1 px has the same colour ±2/255; ≤ 2 % of lit pixels may mismatch (absorbs per-backend tie-breaking for lines on pixel boundaries).
- **Close policy (1.3 follow-up).** `ClosePolicy::PlayfieldOnly` default: `CloseRequested` on the playfield window (plan index 0) quits; on backglass/score windows it is ignored (a stray click on a cabinet's secondary screen must not end the game). `AnyWindow` and `Ignore` exist. The render thread sees window events and raises `Threads::close_requested()`, which the main pump loop polls. SDL `Quit` always quits.
- **`input_dropped` (1.3 follow-up).** The main loop logs growth of `ForwardStats::input_dropped` through `orchestrator::DropMonitor` (once per change) and prints the total at exit. Button resync after drops is left to 3.x.
- **gpu-tests.** pin_render feature `gpu-tests` gates `tests/gpu_debug_render.rs`; `gpu::context_for_tests` prints SKIP without an adapter unless `PIN_REQUIRE_GPU=1`. The SDL window smoke test `crates/pin_app/tests/orchestrator_window.rs` (`harness = false`) needs a native video driver (Xvfb in Linux CI) and skips otherwise unless `PIN_REQUIRE_GPU=1`.
**Rationale:** brief + §2.2/D5/D6/D19; choices keep the fixed API exact and the sim path allocation-free.

## 4.1 — Bundle loading choices

> Superseded in part by "4.1 — fix round 1" (archive-size cap, ratio floor and live ratio check).
- **Zip:** the whole central directory is validated at open (names must be strict UTF-8; `\` is a separator; leading `/`/`\`, UNC and `X:` prefixes are `AbsolutePath`; any `..` component is `PathTraversal`, even one that would stay inside; symlinks by Unix mode; encrypted flag; only stored/deflate). Ratio = uncompressed / compressed (empty compressed stream = 1 byte), checked for every entry. Entries decompress into memory on read, capped at the declared size. An archive file larger than 2 × `max_total_uncompressed` + 1 MiB is refused before reading.
- **Wrapper folder:** stripped (zip and folder) when `table.toml` is not at the root and every file lives under one top-level folder that contains `table.toml`.
- **Cabinet switches:** implicitly declared under their input names (`left_flipper` … `tilt`) with their numbers. A declared switch may share a cabinet number (it is that switch); a declared switch may not use a cabinet input name with another number; two cabinet inputs may not share a number.
- **Profiles:** `balls.profiles`, `restrict` and dispenser contents resolve against `BUILTIN_PROFILE_NAMES`; `TableIds` lists all built-ins.
- **Mock bundle:** `tests/fixtures/apex/table.toml` is the Appendix A example plus the switches/coil/lamp it references without declaring (`lower_left_eos`, `bumper_1_ring`, `ramp_entry_opto`, `ramp_made_opto`, `bumper_1_coil`, `backglass_logo`).
- **glTF:** `gltf` 1.4.1 with default features off (no `import`, which reads the real filesystem); buffers/images resolve through the VFS or `data:` URIs; other URI schemes are refused. Images are decoded to RGBA8 with `image =0.25.6` (png + jpeg only, 16384 px / 512 MiB limits). Sparse accessors and non-triangle primitives are not supported (error / skipped). Missing normals are computed from faces. Because `gltf`/`gltf-json` 1.4.1 dereference some indices unchecked, the raw JSON is pre-validated (every accessor/view/buffer/material/texture/image/node/scene index, image source rules); as defence in depth `load_gltf` also wraps parsing in `catch_unwind` and maps a panic to `AssetError::Gltf` (not relied upon; a release build with `panic = "abort"` would lose it).
- **Errors:** manifest parse errors have an empty `path`; `validate_gltf_bindings` reports file `"glTF"` (use `validate_gltf_bindings_in` to name the file); more than 64 persist keys is a warning.

## 3.3 — fix round 1

- **Hostile Vorbis headers (B1).** `decode` no longer uses `OggStreamReader`: it drives `ogg::PacketReader` (`ogg =0.8.0`, the version lewton already uses) and lewton's low-level `read_header_ident` / `read_header_setup` / `read_audio_packet_generic`, and skips the comment header unparsed. lewton 0.10.2 is vendored at `third_party/lewton` (wired via `[patch.crates-io]`) with allocation bounds rather than pre-validating the setup packet in `pin_audio` (a pre-validator would be a second full setup parser; the patch is small and local): header-driven `with_capacity` reservations capped at 4096 elements, a per-setup budget of 2^18 codebook entries and 2^20 VQ lookup scalars (real libvorbis files: ≤ 6 %), comment lengths checked against the packet. See `third_party/lewton/KHZ_PATCHES.md`.
- **Rate-driven amplification (B2).** File sample rates outside 8 000..=384 000 Hz are `Unsupported` (WAV and Vorbis). Resampling is streamed (source frames are never buffered), the 10-minute cap is enforced on source and output frames while decoding, and checked up front from the WAV data-chunk size or the last Ogg page's granule position, so an over-long file is `TooLong` before large allocations. Up-front reservations never exceed what the input bytes (WAV) or the granule (Vorbis) account for. The aggregate decoded-audio budget per bundle is the loader's job (N3, Orchestrator).
- **Chained / multiplexed Ogg (N1).** Any serial-number change (page scan or packet reader) is `Unsupported`, instead of a silently truncated decode.
- **Voice age (N4).** A voice's age is the arrival number of its trigger (monotonic per mixer), so triggers drained in one `render` call steal in arrival order (A, B, C, D on a 2-voice bus in one buffer leaves C and D). Slot order would only break a tie, which cannot occur. Supersedes the start-frame/slot-tie rule above; the HUMAN_QUEUE confirmation request was withdrawn.
- **SDL put (N7).** Documented that `SDL_PutAudioStreamData` may allocate on the audio thread (inherent to SDL3 streams); `Mixer::render` itself stays allocation-free.

## 2.2 — Ball solver: CCD, spin, friction, magnets & Newton balls (Implementer)

**Decision:** `pin_physics::ball` is split into `types`, `contact` (public impulse math), `solver`, `newton`, `dispenser`. No new dependencies.
- **A tick is a small discrete-event simulation.** Every ball has its own sub-step count, chosen at the start of the tick from its own speed and radius only (travel per sub-step ≤ r/4, or ≤ r/16 while its swept box touches a wireform, capped by `max_substeps`). At each sub-step boundary the ball's contacts are gathered, penetration is projected out, forces are integrated (semi-implicit Euler: gravity, then magnets and hooks) and the touching contacts are solved; between boundaries balls move in straight lines to the earliest time of impact (world `cast_sphere`, guide `cast_sphere`, analytic sphere–sphere for balls and captive balls). **Alternative:** one global sub-step count from the fastest ball (rejected: a steel ball accelerated by a magnet would change the sub-steps, and so the rounding, of a Powerball elsewhere on the table; criterion 2 demands bit identity).
- **Contact model.** Impulses at velocity level. Normal impulse with restitution; Coulomb friction on the slip `v + ω × r_c − v_surface`, limited to the impulse that ends the slip (sliding → rolling, the 5/7 rule falls out); rolling resistance as a torque `c · N · r` opposing the spin (rolling and pivoting separately), which decelerates a rolling ball at `5/7 · c · g`; if friction and resistance together can hold the ball still it stops exactly (static rest), so a resting ball has zero velocity, not a creep. Ball–ball: the contact point divides the line of centres in the ratio of the radii whatever the actual distance, which makes the impulse pair conserve angular momentum to rounding. No rolling resistance between balls.
- **Resting vs impact (restitution threshold).** A ball keeps a small set of support directions (contact normals within 20 µm, per ball). A hit along a direction the ball has been supported in for two consecutive boundaries, or was hit in earlier in the same sub-step (within 20°), is a continuing contact: no restitution, no `Impact` event. Everything else is an impact with the pair restitution, faded in linearly between 0.05 and 0.10 m/s approach speed (below 0.05 m/s: inelastic and not reported). This is what lets a ball slide round a concave guide or settle on the floor without a speed-only threshold killing real bounces. **Alternative:** velocity threshold alone (rejected: at 8 m/s a 150 mm guide arc gives 0.13 m/s micro-impacts that must not bounce, while a 0.13 m/s drop must).
- **Touching contacts are solved together** (projected Gauss–Seidel, accumulated impulses, 10 sweeps at most) for all balls whose boundary falls at the same instant; a continuing-contact hit during a sub-step joins the ball's contact list and the list is solved again. Without accumulation a ball in a groove, a corner or a stack never comes to rest (found with the V-groove test).
- **Restitution/friction source.** Pair table first (precomputed 7 × 7). Missing pair: the ball profile's restitution (ball–ball: the smaller of the two) and μ = 0.2. No branch on profile names anywhere.
- **Magnets.** Evaluated at each sub-step boundary; a ball with `magnetic_coupling == 0`, a magnet at power 0, off-layer or beyond `range` contributes no term at all (the force accumulator is not even added), so criterion 2 holds bit for bit, also with other balls present. Force is zero at d < 1 nm (no direction).
- **External forces.** `ExternalForce::force` is called once per ball sub-step (dt = that ball's sub-step), in handle order for equal times, after gravity; with no hook the add is skipped. `ball_ext` stays a stub naming this API.
- **Moving surfaces (for 2.3).** `SurfaceVelocity::velocity_at(target, point)` (set with `BallSolver::set_surface_velocity`) supplies `v_surface` for colliders, guides and wireforms; `ball::contact::{resolve_surface_contact, solve_surface_contact, sphere_toi}` are public so the flipper code can resolve its own swept contacts with the same model.
- **Guides and wireforms.** Guides: cast events plus push-out to the wall each boundary and after a sliding contact (2.1 hand-off). Wireform rails have no cast: they are contacts at boundaries only, hence the r/16 travel limit near wireforms. Both default to steel and to all layers; `set_guide_layers` / `set_wireform_layers` / `set_*_material` are additive setup-time API. Balls on different `layer` indices do not collide with each other. Layer transitions stay with the caller (`TransitionSet::update` + `ball_mut`).
- **Newton chain.** Captive balls have one degree of freedom (curve parameter, path speed). Per tick: gravity along the tangent × 5/7 (rolling), rolling resistance with static hold, equal-mass 1D collisions with the pair restitution swept until quiet, motion with 1D time-of-impact, overlap correction, end stops reflect. A free ball exchanges a frictionless normal impulse with a captive ball through the path tangent (effective inverse mass `(n·t)²/m`) and the chain is swept at once, so an impact reaches the far ball in the same tick. The ball size is bound from the profile when the chain is added; `NewtonChain::new` therefore cannot reject overlaps (they are pushed apart). At most 8 balls per chain.
- **Other choices.** `add_ball` refuses unknown/degenerate profiles; `Dispenser::dispense` leaves its queue untouched when the solver refuses; positions are advanced with a compensated (Kahan) sum because a fast ball takes up to 64 small steps per tick; a ball that exhausts 12 impacts in one sub-step waits for its next sub-step (never reached in the tests); sensors test the ball centre against a sphere and report edges once per tick; switch numbers above 255 are ignored by `sensor_switches`.
- **Known limitation (from 2.1's cast skin).** `StaticWorld::cast_sphere` reports a hit within a 1 µm touch skin, so where two cuboids meet flush the next cuboid's edge is hit with a normal tilted by up to √(2 µm / r) (0.7° for a 27 mm ball): a seam deflects a rolling ball upward by up to 1.2 % of its speed (above ~4 m/s the landing is reported as a ~0.05 m/s impact). A `TriMesh` floor has no such edges (2.1 flags coplanar interior edges). Authoring guidance: one collider or a mesh for each rolling surface.
- **Observed physics worth knowing.** With wall friction, a fast rolling ball (8 m/s, 150 mm arc) climbs the guide wall: its spin cannot follow the turn, the slip at the wall has a vertical component. This is emergent, not special-cased; guide friction (steel–steel 0.15) is the tunable.
**Performance:** 4 balls in a 300-collider world, release: about 5 µs per `step` (target ≤ 100 µs).

## Orchestrator — Wave 3 pins moved to the root (4.1)

`gltf =1.4.1` in the root `[workspace.dependencies]` now has default features off (`names`, `extras`, `utils` only): the default `import` feature reads the real filesystem, which a bundle loader must never do (D25). `image =0.25.6` (`png`, `jpeg`, default features off) and `serde_json =1.0.151` are new root pins for 4.1. As with the other Wave 3 pins, `versions.toml` is unchanged (spikes lockfile check).

## Orchestrator — 4.5 may update two 4.2 tests

Subphase 4.5 deliberately turns `.wasm` from "unsupported" into a working backend, so the two 4.2 tests that pinned the old behaviour (`tests/lua_backend.rs::load_backend_by_extension`, `tests/spec_reload.rs::wasm_extension_is_unsupported_for_now`) are updated by the 4.5 Implementer to the new contract. That is an ownership amendment limited to those two tests; nothing else in 4.2's files changes.

## Orchestrator — 2.2 review round 1 rulings

- **B1 needs a world/ change, so 2.2 gets a limited ownership amendment** to `pin_physics/src/world/{wireform,static_world,shapes}.rs` (Subphase 2.1, merged) for fix round 1: additive only, all 2.1 tests stay green.
- **B3 (hop at the seam of two flush cuboids) is non-blocking.** Rolling surfaces in generated tables are triangle meshes (4.3), where the welded-edge rule applies; authors must not build rolling floors from abutting cuboids. Recorded in HUMAN_QUEUE for 4.3.
- **Mesh-floor normal glitches are raised to required**, because generated playfields are meshes.

## 4.1 — fix round 1

- **Zip data ranges (B1).** At open, every central record's local header is read, and its data range (local header + name + extra + declared compressed size) must lie inside the archive, before the central directory, and not overlap any other entry. The sum of the declared compressed sizes may not exceed the archive. Stored entries must declare equal sizes.
- **Live ratio (B1, N5).** `pin_assets` inflates entries itself (`flate2`, the zlib-rs backend zip already uses) from the validated slice of the archive. For an entry over 1 MiB (`RATIO_FLOOR`) inflation aborts with `CompressionRatio` as soon as output / compressed bytes actually consumed exceeds `max_ratio`. The declared ratio is also checked at open, again only above 1 MiB, so honest small files (silent WAVs, zero-filled data) pass; the per-file and total caps bound the rest. Output never exceeds the declared size, and the CRC-32 is checked.
- **Entry count and archive size (N1).** The entry count is read from every end-of-central-directory candidate in the tail (and its Zip64 record) and checked against `max_entries` before the central directory is parsed. An archive file larger than `max_total_uncompressed` + 1 MiB is refused before it is read.
- **glTF budgets (B2).** `GltfLimits` (per `load_gltf` call; `load_gltf_with_limits` takes one): decoded RGBA8 bytes in total ≤ 256 MiB, image side ≤ 8192, output vertices ≤ 8 M and indices ≤ 24 M in total across every `MeshData` (instances counted again), nodes ≤ 65 536, meshes ≤ 16 384, images ≤ 1024, distinct buffer bytes ≤ 256 MiB. Each is checked before the memory it bounds is allocated: image size from the header (the decoder's own allocation limit is 3 × the RGBA8 size + 1 MiB), vertex/index counts from accessor counts, buffer bytes from declared `byteLength`s; exceeding one is `AssetError::Gltf("… budget …")`.
- **Only used images are decoded, once (B2).** An image is decoded only when a material's base-colour, metallic-roughness, normal or emissive texture uses it. Images and buffers are cached by resolved source (the `data:` URI, the normalised bundle path, or the bufferView); a repeat costs nothing: its `ImageData` is empty (0 × 0) and materials point at the first image with that source. Buffers are truncated to their largest declared `byteLength`.
- **Finite floats (B3).** `GltfScene` holds only finite floats: every world matrix is checked after each product, material factors are checked, attributes already were.
- **Validation (N3, N4, N9).** Duplicate and name lookups use hash maps (output order unchanged). `table.toml` / `backglass.toml` over 4 MiB is a ValidationError at path `""`. More than 65 536 entries of one id kind (or persist keys) is a ValidationError at the section path ("too many <kind> entries"). Switch `radius` ≤ 0, magnet `d_min` ≤ 0 or `range` ≤ 0, and `k_coil` < 0 are ValidationErrors at their paths.
- **OS junk and symlinked folders (N6).** `.DS_Store`, `Thumbs.db`, `desktop.ini` and anything under `__MACOSX/` are ignored when finding the wrapper folder and are never listed or read (zip and folder). In a folder bundle, a path through a symlinked folder is refused by `read`/`exists` (`AssetError::Symlink`, or `PathTraversal` when it leaves the root), matching `list`, which does not descend into them.

## 1.4 — fix round 1 (review B1, N1–N7)

**Decision:**
- **B1/N5 coverage.** pin_render has its own SDL window smoke test `crates/pin_render/tests/gpu_window.rs` (`harness = false`, `required-features = ["gpu-tests"]`, sdl3 dev-dependency; same skip / `PIN_REQUIRE_GPU` rules): playfield `Direct(Fifo)`, a backglass surface once forced onto a presenter thread and once on the policy's own choice, resize, expose, minimise/restore, redraw-on-change, pacing and drop (joins the presenter). The blit used by presenter threads has an analytic offscreen readback test (`gpu::tests::blit_copies_source_pixels_exactly`: exact copy into `Rgba8Unorm`, channel swap into `Bgra8Unorm`).
- **Concurrent configure (found by the new test).** wgpu 30's `Surface::configure` waits for the device to go idle and raises a validation error ("Failed to wait for GPU to come idle") when the queue is not empty afterwards, which happened intermittently with a presenter thread on the same device even with submissions serialised. `SurfaceSet` now serialises all its submissions and (re)configurations with one `Mutex<()>` shared with presenter threads (render side only, not the sim thread), and configures through `try_configure`: an error scope catches the validation error (the default handler panics), the call is retried up to 4 times after `device.poll(wait)`, and a still-failing surface is retried before its next draw (direct) or next hand-off (presenter). 10/10 runs of the smoke test passed after the change.
- **N1 pacing.** The render loop treats a frame as paced only when a playfield surface returned `Presented` (`SurfaceSet::frame_paced(&statuses)` / `gpu::paced_by`); otherwise (occluded, timeout, outdated, hidden, zero-sized) it parks for `idle_period`. `paces_frame()` was removed.
- **N2 adapter choice.** The app uses `GpuOptions::for_app()`: `WGPU_BACKEND` plus only an explicit `PIN_FORCE_FALLBACK_ADAPTER=1`; CI variables are ignored. `GpuOptions::from_env()` (tests/CI) never forces the fallback adapter on macOS or with Metal only (`from_vars_on(macos, ..)` makes this testable on every OS).
- **N3.** `HybridTimer` sleeps in slices of at most 50 ms (`MAX_SLEEP_SLICE_NS`) so `stop()` is seen promptly.
- **N6.** `Camera::ortho_top_down_with_margin`; the default playfield camera has a 1 cm margin so lines exactly on the table bounds stay visible (a line on the left/top bound of a plain ortho camera can fall outside by the pixel tie-break).
- **N4.** Dropping a presenter waits at most 500 ms (`PRESENTER_JOIN_TIMEOUT`) for its thread, then detaches it with a log line, so shutdown cannot hang on a stalled swap chain. Risk: a detached thread still owns its surface while the main thread may destroy the window under it (wgpu/driver behaviour then undefined). Not reproducible on Xvfb/lavapipe; acceptable because it only happens on an already-stalled compositor at shutdown.
- **N7.** `gpu::context_for_tests` is `#[doc(hidden)]` (a test helper, kept public for the integration and spec tests).

## 4.5 — fix round 1 (review B1, N1–N5)

**Decision:**
- **B1 pre-compile limits (D25, criterion 6).** `WasmScriptBackend::prepare` (and so `new`, `load_backend*` and `apply_reload`) checks every module before wasmtime compiles it; each limit is a public constant in `pin_script::wasm`, and anything over one is `ScriptError::Load` with a message naming the limit. Orchestrator-set values: binary ≤ `MAX_MODULE_BYTES` = 1 MiB; WAT text ≤ `MAX_WAT_BYTES` = 4 MiB, and the binary it produces ≤ 1 MiB; then a `wasmparser` pass (wasmtime's own copy via its `reexport-wasmparser` feature, so the exact version wasmtime 50 uses) rejects > `MAX_FUNCTIONS` = 10 000 defined functions (function or code section count), any function body > `MAX_FUNCTION_BODY_BYTES` = 256 KiB, > `MAX_GLOBALS` = 1 000 globals, > `MAX_TOTAL_TABLE_ELEMENTS` = 100 000 initial table elements summed over all tables, and > `MAX_SEGMENTS` = 1 000 data segments or > 1 000 element segments (each counted separately). WAT text is still accepted; it is converted with the `wat` crate (pinned `=1.263.0`, the version wasmtime already resolves to) so the scan always sees binary, and the module is then compiled with `Module::from_binary`.
- **N1 stack.** The engine sets `max_wasm_stack` to `MAX_WASM_STACK_BYTES` = 256 KiB; deeper recursion traps (`Runtime("… call stack exhausted …")`, a normal fault). The thread calling a Wasm backend must have at least `MIN_HOST_STACK_BYTES` = 2 MiB of stack (tested: a 1 MiB thread already survives deep recursion).
- **N3 fuel.** wasmtime 50 charges bulk operations by size (1 unit per byte of `memory.fill/copy/init`, per element of `table.fill/copy/init/grow`, per page of `memory.grow`) after the operation completes, so one operation can overshoot the budget by at most its own size (bounded by the memory cap and table limits). Docs corrected in `wasm/mod.rs` and the guest SDK; looping whole-memory/whole-table fills and copies fault with `BudgetExceeded` under `WasmConfig::engine()`.
- **N4.** A module that exports an imported host function under an entry-point name (`on_init`, `on_tick`, `on_switch_edge`, `save_state`, `restore_state`, `alloc_state`) is rejected at load (`Load("export … re-exports an imported host function")`). Re-exporting an import under any other name is allowed (harmless).
- **N2.** `restore` computes the blob's end with `checked_add`, as `save` does.
- **N5.** `pin_guest_sdk::RulesCell<T>` is `Sync` only for `T: Send` (documented: all access goes through the `unsafe get`, whose contract now also forbids use from another thread meanwhile). Not gated on wasm32, because `export_rules!` statics need `Sync` in the host-side unit tests.

## 2.2 — fix round 1 (world/ amendment)

**Decision:** review round 1 of the ball solver; the Orchestrator allowed edits to `world/wireform.rs` and `world/shapes.rs` (Subphase 2.1) for B1. No public `world/` signature changed.
- **B1, rising/falling guides (`world/wireform.rs`).** A non-level guide now keeps the spline through its projected through-points (its plan view; exact, because the interpolation is linear in the points) and the reach of that plan-view curve. The wall is a strip of the vertical cylinder over the plan view, so its gap can never be smaller than the horizontal gap to that cylinder: the cast applies the existing quadratic reach bound to the horizontal gap and takes the larger of the two proven steps, exactly as for level guides. Beside the wall the two gaps are equal, so a ball sliding along a rising arc advances as on a level one (≥ 95 % of the analytic distance per tick at 1/5/8 m/s for 1/10/150 mm rise, tested, plus a brute-force no-tunnelling proptest). Only a plan view without reach (self-touching, vertical stretch) still falls back to conservative advancement. **Alternative:** more sub-steps or a larger event budget in the solver (rejected: hides the cost instead of removing it). The 2.1 implementer test `guide_cast_on_a_rising_curve_stays_safe` asserted the old fallback (32 steps and an `Advance`) and was updated to assert the new behaviour; it is outside the amendment's file list and is flagged in the report.
- **Mesh floors (`world/shapes.rs`).** The glitch on pitched mesh floors was rounding in the triangle face distance, `(p − a) · n` with a vertex tens of metres away (the probe used 120 m triangles): ±1 µm noise, the size of the cast's touch skin, so a resting contact was sometimes missed for a sub-step. The difference is now taken in f64 (deterministic, no FMA). Cuboids were unaffected because they work in their local frame.
- **B2, bit identity with other balls moving.** Balls no longer share a clock. Each ball's position is advanced only at its own sub-step boundaries and impacts and when another ball comes within contact range (20 µm) or collides with it; pair times of impact are computed from extrapolated positions without touching either ball. The boundary solve iterates each ball until its own contacts have converged, so the sweep count of one ball does not depend on an unrelated cluster. Tested with moving steel balls in the magnet's field (four geometries, both non-magnetic profiles, orientation included) and a property test that adding non-interacting balls never changes a ball's bits.
- **Resting headroom.** (1) World contacts are solved at a boundary within the cast's 1 µm touch skin (was 0.2 µm), and a ball resting inside a surface's skin and not leaving it (≤ 1 µm/s) is seated on the surface: the rounding residue of its velocity, which the compensated position sum integrates faithfully, used to float it through the skin and drop it back about once in ten seconds (the "1 µm drift"). (2) Balls a few micrometres apart may close the gap (`target = −gap / h`); surfaces may not, because their casts stop a ball anywhere inside the skin. (3) The boundary solve is warm-started from the impulses of the previous solve (per ball, matched by contact and normal, scaled by the sub-step) and may take up to 64 sweeps. Results over 10 s after settling: ball against a post on a 6.5° floor ≤ 6 nm, rows of 2/3/5 balls ≤ 15 nm, mini pinned between two jumbos ≤ 0.13 µm (limit in the test: 0.5 µm). Cost in release: row of 4 resting balls 9 µs, row of 6 about 6 µs, row of 16 about 70 µs per step; 4 free balls in 300 colliders 4 µs.
- **B3, flush cuboid seams: documented, not suppressed.** The hop comes from the 1 µm touch skin of `cast_sphere` at the next cuboid's edge (2.3 µm at 1 m/s, growing with speed, a spurious ~0.05 m/s impact from about 4–8 m/s). Suppressing it in the solver would need the hit feature (face or edge), which `SphereHit` does not carry; a heuristic on the normal alone cannot tell a flush edge from the start of a shallow ramp. Rolling surfaces should be meshes (coplanar interior edges are already ghost-free) or single colliders.
- **Wireform contact depth (noted).** Rails have no sphere cast: under lateral velocity a ball sinks below rail contact height by up to its travel per sub-step before the boundary correction (reviewer measured 10 µm steel, 47 µm jumbo, with 4–7 small impacts while it settles). The r/16 travel limit near wireforms bounds it; a rail cast in `world/` would remove it.

## Orchestrator — 3.2 plunger speed accuracy under noise

Verification criterion 3 ("one release within 5 % of the true peak") is binding for noise-free traces and for noisy traces with depth ≥ 200 × noise amplitude. Below that, the release fires after only 8–16 samples of a shallow stroke and no estimator can reach 5 %; the accepted bound there is max(5 %, 12 × amplitude / depth). The binding requirement under noise is that no launch is lost and none is doubled.

## Orchestrator — 2.2 fix round 1: 2.1 test adjusted

`crates/pin_physics/tests/wireform.rs::guide_cast_on_a_rising_curve_stays_safe` (a 2.1 implementer test, not a spec test) asserted the stalled behaviour that 2.2 review B1 removed (32 cast steps and an `Advance` beside a rising curve). The 2.2 Implementer changed only that part to the new behaviour (≤ 8 steps, contact at the wall); the head-on safety assertions are unchanged. Accepted as within the world/ ownership amendment.

## 2.2 — fix round 2 (global closest point of rising guides)

**Decision (B4, `world/wireform.rs`, under the round 1 amendment; no public signature changed):** `GuideCurve::closest_point` for a non-level curve is now a global search. The distance from the query point to the wall's vertical line over `C(t)` is a C¹ function of `t` whose derivative has the sign of `(q(t) − p) · C'(t)`; every sample of the curve (16 per segment) is evaluated, and each sample interval that could hold a point closer than the best so far — the distance cannot change faster than the interval's arc length, bounded from the end speeds and the segment's third derivative — is searched: a sign change of the derivative is bisected, an interval without one is quartered first. The old code alternated between the 3D curve's closest point and the height on the wall, which only finds the minimum next to the curve's own closest point; on tight or steep rising curves it overestimated the distance by centimetres, and the cast's conservative step, taken from that distance, went through the wall. The fault predates 2.2 (same result at e956476); round 1's documentation wrongly presented rising guides as fully supported.
- **What is guaranteed.** The search misses a minimum only if the derivative changes sign twice within 1/64 of a segment, i.e. where the wall bends on that scale; the rule "through-point spacing below the smallest radius of curvature" (already 2.1's rule for `CubicSpline::closest_point`) excludes it. No curve shape is rejected: helices whose plan view overlaps and plan views without reach are handled (the latter slowly, by conservative advancement only). 4.1/4.3 should surface the spacing rule as an authoring warning next to the existing 3:1 spacing warning.
- **Cost.** One query is a scan of all samples (a few arithmetic operations each) plus about 30–60 spline evaluations; level guides are unchanged. Sloped guides should stay short (tens of segments).
- **Tests (`tests/ball_guides.rs`).** Closest point against brute force for ten shapes (arcs r = 15/30/50 mm with rise 1:1 and 3:1, rising and tight falling S-curves, 1.5- and 2.5-turn helices, the 400 mm arc), 600 points each, seeded and unseeded: never more than 1e-6 m above the true distance. The reviewer's tunnelling scenario, 320 runs per shape at 1–30 m/s with three ball sizes: no ball more than 0.1 mm inside a wall at any tick and none changes sides. Both tests fail against the previous `closest_point` (10 mm inside the wall on the sixth run).
**Noted, not changed:**
- **NB1, contact range.** Two balls within 20 µm of each other count as touching at a sub-step boundary (the same tolerance that decides resting contact), so a 5 µm near miss is resolved as a contact a few micrometres early and the balls deflect as in a graze. This is also the range inside which "non-interacting" no longer holds for the bit-identity property.
- **NB3, wireform sink.** Rails have no sphere cast; a jumbo ball with lateral velocity sinks about 45 µm below rail contact height before the boundary correction (steel about 10 µm). A rail cast in `world/` would remove it.
- **NB4, per-ball clocks under budget exhaustion.** If the tick's safety budget of events runs out (never observed), the balls are left at their own clocks and the rest of the tick is dropped for each of them: positions are then valid at slightly different times within that tick. The next tick starts all clocks at zero again; no ball can have passed through anything, only time is lost.

## 3.2 — plunger release speed, pull ticks and offset rests (other-model attempt)

- **Release speed (B4).** The speed is the largest finite difference in `release_window_ms` up to the release tick, over a baseline of k updates. k = 1 (the exact largest 1 ms difference) unless noise would be a visible part of the result; it grows, up to a quarter of the return stroke and 8 updates, until k·peak ≥ max(4·√2·σ / 0.03, q / 0.1) per ms. σ is the sensor noise measured *at rest* (running mean of squared second differences over 256 updates, skipping the first 20 after coming to rest), so stroke curvature is never mistaken for noise; q is the quantisation step, detected when all 1-update changes in the window (≥ 8 of them) are whole multiples of a common step ≥ 1e-5. A return stroke (last 90 % of travel) that is straight within that noise (peak ≤ slope + max(4·√2·σ, q)/k) reports its least-squares slope. Measured, defaults: every row of the B4 table and a sweep of 8 shapes × 3–90 ms × 8 depths noise-free: ≤ 0.001 % from the largest 1 ms difference up to the release tick. Noisy linear grid (2400 launches per cell, 0 lost, 0 doubled), worst error: ±0.001 2.2 %, ±0.002 4.5 %, ±0.004 9.0 %, ±0.008 49 %, 8-bit dither 14.3 %, 10-bit dither 3.0 % (all worst cases at depth 0.1; within 5 % wherever depth ≥ 200 × amplitude). Curved strokes with uniform noise ±depth/1000: ≤ 3.5 %; ±depth/400: ≤ 8.7 %; ±depth/200: ≤ 9.7 % (accelerating strokes; a longer baseline trades noise for curvature bias — 5 % is not reached there). Undithered 8/10/12-bit readings: linear ≤ 0.7 %, curved ≤ 5.3 % / 2.9 % / 0.5 %.
- **Bound from the fixed API (accepted, queued).** The release is reported on the tick the plunger re-enters the deadzone, so speed gained inside the deadzone is not seen: against the peak of the *whole* stroke, an accelerating stroke reads low by up to 1 − √(1 − deadzone/depth) (constant acceleration: −10.8 % at depth 0.1, −5.2 % at 0.2, −3.4 % at 0.3, 0 from 0.6 for strokes ≤ 40 ms; quarter-sine: −3.8 % at 0.1). B4 row "const. accel 35 ms d0.3" is −2.9 % by this measure and exact up to the release tick.
- **Pull ticks (N-k).** `MIN_PULL_TICKS` counts updates at qualifying depth, not outside the deadzone.
- **Offset devices (N-l).** The first rest acquired in the power-on window is a permanent known-good rest, and a stable level within 0.001 beyond ±0.25 counts as at the limit (the level average only approaches it). Measured: 0 of 80 000 random histories (10 offsets in −0.25..0.25, with and without ±0.004 noise) lose or double the launch after 2 s at rest.

## Orchestrator — 3.2 "true peak" for release speed

The fixed API reports a release on the tick the plunger re-enters the deadzone, so motion inside the deadzone after that tick is unobservable. For criterion 3 and the noise ruling above, "true peak" is the largest 1 ms position difference **up to and including the release tick**. Against the whole stroke, shallow accelerating strokes read low (constant acceleration −10.8 % at depth 0.1, −5.2 % at 0.2); that is an accepted, documented bound. With heavy noise on curved strokes (±depth/400 and coarser), the noise rule's max(5 %, 12 × amplitude / depth) applies.

## Orchestrator — 2.2 review round 3: guides with self-overlapping plan views

Guides whose plan view overlaps itself (helices, loops of more than one turn in plan), and any sloped guide for which no positive reach can be given, are rejected by `GuideCurve::new`; helical ramps are modelled as wireforms. Accepted sloped guides must stay exact and fast (targets in the fix-3 brief: ≤ 3 µs per query on 60 segments, ≥ 95 % of v·dt on ≥ 99 % of ticks).

## Orchestrator — 3.2 final ruling: release-speed accuracy under noise is non-gating

The final (other-model) review of 3.2 found noisy accelerating strokes (constant acceleration, preloaded spring) reading 5–10 % low at ±0.001…±0.004 and 10-bit dither, against the 5 % this log's earlier noise ruling set; everything else passed (gates 1–7; noise-free accuracy exact on all shapes; 0 lost / 0 doubled / 0 phantom launches over 28 800 noisy launches per noise model and 384 000 random histories at true rest 0 and offsets ±0.25).
**Decision:** PLAN 3.2's criterion ("synthetic traces … one release within 5 % of the true peak") applies to noise-free traces and is met exactly. Release-speed accuracy under sensor noise was an Orchestrator extension and is **reported, not gated**: measured worst cases (curved strokes ≈ 10 % low at ±depth/200; quarter-sine at depth 0.1 with ±0.002 up to 27 %) are recorded as a documented bound for tuning at final human acceptance with real plungers. The binding requirements under noise remain: no launch lost, none doubled, no phantom launch. The earlier "Orchestrator — 3.2 plunger speed accuracy under noise" bound is superseded by this entry.
**Rationale:** the plan's failure policy would otherwise block 3.2 and its dependent 3.4 on a rule the brief does not contain; the gameplay-critical properties hold with wide margins; plunger feel is a final-acceptance item (D31).
**Alternatives:** mark 3.2 blocked (PLAN §2.6 step 3); or a further fix round with a quadratic end-of-stroke estimator (the reviewer's prototype reached 5 % on most strict cells but adds +3.3 % bias on quarter-sine strokes).

## 2.2 — fix round 3 (sloped guides: acceptance rule and cost)

**Decision (Orchestrator ruling, `world/wireform.rs` + one additive `SplineError` variant in `world/spline.rs`):** a guide whose curve is not level is accepted by `GuideCurve::new` only if a ball can slide along it at full speed; everything else returns `SplineError::SlopedGuidePlanView`. Level guides are unchanged (any level curve is accepted, as in 2.1).
- **Acceptance rule (exact).** Let the plan view be the spline through the through-points with Z set to 0. The guide is accepted iff (1) the plan view exists (no two consecutive through-points coincide in plan, i.e. no vertical run); (2) its reach estimate — 0.9 × the Federer estimate 2.1 already computes: curvature bound per sample interval and the pair formula on every fourth sample — is at least **20 mm** (no plan-view bend tighter than about 22 mm radius); and (3) no two plan-view points, taken on every second sample, are closer than 0.98 × the chord a curve of 20 mm reach keeps between points that far apart along it (`2ρ·sin(arc/2ρ)` up to half a turn, `2ρ` = 40 mm beyond; around a closed curve the shorter way counts). Rule (3) is what rejects helices and loops of more than one turn: their laps lie on top of each other in plan, which as a point set is a plain circle with a perfectly good reach, while numerically they are two sheets micrometres apart. Rejected in the tests: helices of 1.5 and 2.5 turns, a 1.1-turn spiral, a 30 mm hairpin, a figure of eight, a 15 mm arc, a 286° arc of 30 mm radius (ends 36 mm apart), the tight falling S with 7.5 mm bends, a vertical run. Accepted: arcs of 30/50/400 mm radius with rise up to 3:1, rising and falling S-curves with bends of 42 mm, a closed ring that rises and falls.
- **Why 20 mm.** The cast's reach bound applies while the ball centre is closer than 0.9 × reach; 20 mm covers every ball up to 18 mm radius (standard 13.5, jumbo 15.9). The 79.4 mm XL ball (radius 39.7 mm) can use the bound only on guides with a reach above 44 mm, level or sloped; on tighter ones it still collides correctly but slides slowly. That limitation of 2.1's bound for balls larger than the reach is not new and not removed here.
- **Cost.** Beside the wall (the query point level with the wall above the plan view's closest point) the horizontal distance to the plan view is a lower bound of the wall distance and is attained, so one plan-view query is the exact global closest point. Only above or below the wall's edge is the curve searched, and then only in segments whose bounding sphere (stored per segment) could hold a closer point. The reach bound of the cast now uses the horizontal speed, since only horizontal motion changes the horizontal gap; with the full speed a steep wall took several times more steps. Release numbers (60-segment arc, radius 50 mm, rise 3:1; this machine, shared): `closest_point` median 0.69–0.87 µs (level twin 0.66–0.82), `cast_sphere` median 2.3–2.9 µs (level 2.4–3.0, for a ball touching the wall and moving along it); one ball sliding: mean 8.4–9.1 µs per step, p99.9 30–38 µs, worst 31 µs; four balls: mean 20–22 µs, p99.9 52–65 µs, worst 54–59 µs (worst = fastest of three identical repeats, which removes scheduling spikes of up to 1 ms seen in single runs); 0 of 31 109 and 0 of 63 248 ball-ticks below 95 % of v·dt.
- **Tests.** `tests/ball_guides.rs`: rejection (with the same curves accepted as wireforms and, flattened, as level guides), closest point against brute force (≤ 1e-6 m over), the tunnelling scenario (≤ 0.1 mm), and full-speed sliding on the 50 mm 3:1 arc and the falling S (≥ 95 % of v·dt on ≥ 99 % of ticks, both sides, three ball sizes); `tests/ball_perf.rs` `report_sloped_guide_costs` (ignored, release). 2.1's `tests/alloc_guard.rs` used a 1.24-turn rising spiral as a guide; it is now 0.93 turns (flagged in the report).
- **Deviation from the ruling's wording.** The ruling lists "the tight falling S" among shapes expected to slide at full speed. Its plan view bends at 7.5 mm radius, tighter than every ball; no reach bound applies to a ball beside it, so by the ruling's own general clause (never accept a shape to crawl) it is rejected, and the full-speed test uses a falling S with 42 mm bends instead.

## Orchestrator — 2.2 fix round 3: tight falling S rejected

The round-3 ruling listed "the tight falling S" among shapes that must slide at full speed. Its plan view bends at 7.5 mm radius, tighter than any ball, so no reach bound applies beside it; under the same ruling's general clause (never accept a shape that would make the ball crawl) it is rejected by `GuideCurve::new` with `SplineError::SlopedGuidePlanView`. Accepted. The additive `SplineError` variant in `world/spline.rs` and the shortened rising spiral in 2.1's `tests/alloc_guard.rs` (1.24 → 0.93 turns) are accepted under the world/ amendment. pin_assets' 211 tests still pass with the new rule.

## 4.5 — fix round 2 (review B1 compile cost, N7)

**Decision:** `prepare` reads every function body's operator stream (wasmparser, no compilation; allocation bounded by the caps) and rejects, with `ScriptError::Load("… has N <what> (at most M)")`, any module over a limit chosen so that **every module that passes compiles in < 1 s and < 256 MB peak RSS** (release, this machine; Cranelift kept, opt level and register allocator unchanged). Measured cause of the round-2 probes: Cranelift's cost is driven by products, not counts. Loop nesting with many live locals explodes (1 024 locals: 32 nested loops 0.29 s, 96 → 0.9 s, 128 → 54 s), blocks in one function cost quadratically (≈ 0.18 µs × loops²; br_table targets into loop headers become blocks of their own: 16 384 targets → 3.5 s), locals × operators drives register allocation, each loop costs ≈ 35 µs (fuel check), and element `ref.func` expressions cost ≈ 30 µs and 6 KB each. `OptLevel::None` did not help; the single-pass allocator is documented as not production-ready, so it was not used.

Final limits (`pin_script::wasm` constants; Orchestrator's values lowered where measurements required, marked ↓):

| limit | value |
|---|---|
| binary / WAT text | 1 MiB / 4 MiB (unchanged) |
| functions | 2 048 ↓ (10 000 took 1.1 s empty) |
| function body | 256 KiB (unchanged) |
| locals per function / in all | 1 024 / 65 536 |
| operators per function / in all | 65 536 / 262 144 ↓ (1 048 576 was unreachable under the 1 MiB cap) |
| nested blocks | 128 |
| nested loops (new) | 32 |
| blocks, loops and ifs in all (new) | 4 096 |
| br_table targets per instruction / per function | 1 024 / 1 024 ↓ |
| types; params / results per type | 256 ↓; 32 / 8 |
| imports / exports | 64 / 256 ↓ |
| globals, data segments, element segments, table elements | 1 000, 1 000, 1 000, 100 000 (unchanged) |
| element items in all | 1 024 ↓ |
| merge work Σ (params + locals) × (blocks + branches + br_table targets) (new) | 65 536 |
| branch work Σ (blocks + branches + br_table targets)² per function (new) | 2 097 152 |
| pressure work Σ (params + locals) × (operators + params + locals) (new) | 8 388 608 |

"Branches" are `br`, `br_if`, `else` and `return`; "blocks" are `block`, `loop`, `if` (and `try`/`try_table`, which are disabled anyway). Worst-case modules at every cap, and two modules at every module-wide budget at once, are accepted (`tests/wasm_compile_cost.rs`); one over each cap is rejected. Release compile times, worst of two runs, one process per case: every single-cap worst case ≤ 0.33 s and ≤ 46 MB; everything-at-once 0.64 s / 60 MB (nested-loop variant) and 0.72 s / 44 MB (br_table variant). WAT parse errors in `Load` are cut to 256 bytes (`MAX_ERROR_BYTES`) on a character boundary (N7).

## 2.2 — other-model attempt (B6: top and bottom edges of sloped guides; NB1)

**Decision (`world/wireform.rs`, `world/spline.rs` message only; additive `#[doc(hidden)] GuideCurve::reach()`; level guides bit-identical):** a rising or falling guide wall gets a reach *as a set in space* (plan view and edges), and both its edge-region closest point and its sphere cast rely on it.
- **Wall reach.** Federer 1959 Thm 4.18 (`reach = inf |w|² / (2 dist(w, Tan(W, x)))`). The wall lies on the vertical cylinder over the plan view, so for `x` inside the wall or on its vertical end edges every pair is no worse than the plan view's own pair (horizontal normal, `|w| ≥ |w_h|`; at an open end the plan view's tangent cone is now the half-line, as it must be). For `x` on the top or bottom edge the in-plane cone is the half-plane below (above) the edge tangent, so `dist² = (w·n)² + max(0, w·m)²`. `reach(W) ≥ min(ρ_P, ρ_E)`: `ρ_E` has a rigorous local part (the edges are translates of the curve, so the near-pair limit is bounded by the curve's 3D curvature, bounded per sample interval as in 2.1's `curve_reach`) and a sampled far part (every 4th × every 2nd sample, the height on the wall maximised in closed form over a half-line, which covers walls of every height; 0.9 margin, as `curve_reach`). Corners: proved in the doc comment of `wall_reach`.
- **Acceptance (tightened, say-so):** `GuideCurve::new` now also requires the wall reach ≥ 20 mm, i.e. the curve may not bend up or down tighter than about 22 mm. Of all previously accepted test shapes only the falling S with a 20 mm ripple (edges bending at ≈14 mm, less than a ball radius: a ball riding or skimming such an edge touches it at two points, no reach bound can exist) is now rejected; the implementer tests use the same S with a 5 mm ripple. For every other accepted shape the wall reach equals the plan reach. The rule does not depend on the wall height (pin_assets validates with height 0).
- **Closest point above or below an edge.** Safeguarded Newton from the plan view's parameter to a stationary point of the distance to the wall's height-clamped vertical lines (first-order condition = `p − q` in the normal cone). By Federer Thm 4.8 (12), a stationary point closer than the reach is the unique nearest point, so it is returned without any global search; only beyond the reach (the XL ball, far points) does the old bounding-sphere segment search run (seeded with it).
- **Cast.** For sloped guides the quadratic reach bound is replaced by its exact "rolling ball" form: with `a` the nearest point and `n` the unit normal toward the centre, the open ball of radius `ρ` around `a + ρn` misses the wall (Federer 4.8 (12)), so `distance(x) ≥ ρ − |x − c|` everywhere and the time to shrink the gap by `drop` is the positive root of `|v|²τ² − 2R·rate·τ − drop(2R + drop) = 0`, `R = ρ − d`, with no validity window. It is applied to the wall distance (beside the wall and at its edges) and to the horizontal distance to the plan-view cylinder (plan reach). After a cast's first step the nearest point is found from the previous parameter (Federer again), skipping the plan-view query. **Level guides keep 2.1's code path unchanged** (2.1's `guide_ramp` test pins their trajectories: a rail-force rounding jitter in its toy integrator crossed 5 % when the guide contact moved by a rounding).
- **Numbers (release, fastest of 5, this shared machine; table in the report and `tests/ball_guide_edges.rs` `report_edge_region_costs`):** in the edge regions (riding the top edge, skimming over/under both edges at ±35°/±60°, 1–8 m/s, steel/mini/jumbo) of six accepted guides: closest point median 0.45–1.4 µs; one ball 1.8–7.4 µs mean, worst ≤ 58 µs; four balls 6.8–24 µs mean, worst ≤ 172 µs; 2 of 52 000 ball-ticks below 95 % of v·dt (all on the tightest accepted arc, r = 30 mm). Before: up to 0.53 ms mean / 11.9 ms worst and 30–60 % slow ticks. Beside the wall (R3 scenario) one ball 4.7 µs mean (was 8.8), four 12.5 µs (was 20). XL ball (radius 39.7 mm, above the reach of the tighter guides): unchanged slow slide, 0.12–0.62 ms mean, worst 1.5–8.7 ms on the 30/50 mm arcs and the S, never sinks; on guides whose reach exceeds its radius it is as fast as the others.
- **NB1.** The reach estimate is 0.9 × the Federer estimate; a natural spline through points of an arc bends about 27 % tighter than the arc near its free ends, so plain arcs are accepted from about 29–30 mm radius (29 mm only with ≥ 16 points, 30 mm always), not 22 mm. Docs and the `SlopedGuidePlanView` message now say so; the 44 mm figure (two stretches of the plan view: `0.9 · D/2 ≥ 20 mm`) was right.
- **Tests.** New `tests/ball_guide_edges.rs`: full-speed riding/skimming sweep with brute-force sink check (≤ 0.1 mm), XL never sinks, closest point at the edges vs brute force (≤ 1e-6 over, ≤ 2e-6 under, seeded and not), edge casts at 0.2–30 m/s never tunnel (brute force along the swept path), edge acceptance rule (height-independent), cast step count at the edges, and the ignored timing report. A one-off release run of 3 000 random queries and casts per shape on 10 accepted shapes agreed with brute force to 7e-8 m with no swept penetration.
**Noted, not changed:** 2.1's level-guide reach uses the whole tangent line at an open end, which is optimistic behind an end when the curve comes back within twice the reach (a 342° arc of 0.5 m: estimate 0.35 m, true ≤ 78 mm). A probe found no tunnelling (casts are windowed to a sub-step and the nearest point switches ends at the medial ridge, far from either end), and fixing it changes level-guide trajectories that 2.1 tests pin; left for the 2.1 owner.

## Orchestrator — 2.2 other-model attempt: sloped walls bending up/down tighter than ~22 mm rejected

The other-model implementer also rejects sloped walls whose curve bends up or down tighter than about 22 mm (no edge reach bound can exist when the edge bends tighter than a ball radius). Only one previously accepted test shape (falling S with a 20 mm ripple) is affected. Accepted under the round-3 ruling's general clause (never accept a shape that would make the ball crawl).
