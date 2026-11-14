# Contract Change Requests

`pin_core` holds every cross-crate contract (PLAN.md §3, decision D4). Only
Subphase 1.1 wrote it freely. From then on, a change to anything in
`crates/pin_core/`, `physics_defaults.toml` or the fixtures in
`crates/pin_core/fixtures/` goes through this file (PLAN.md §2.3 rule 2).

## Protocol

1. **Request.** The agent that needs the change appends a request below, using
   the template. One request per logical change.
2. **Classify.** The request says whether the change is **additive** or
   **breaking** (definitions below). If unsure, call it breaking.
3. **Separate PR.** The change is made in its own `pin_core`-only commit or PR
   (prefix `[cr NNN]`), never mixed with the requesting subphase's own work.
   The PR updates `pin_core` tests and, if the on-disk schema changes, the
   fixtures and PLAN.md Appendix A/B/C reference in the request.
4. **Merge timing.**
   - Additive changes merge **immediately**, once `cargo test --workspace`,
     `cargo clippy --workspace --all-targets -- -D warnings` and
     `cargo fmt --check` pass.
   - Breaking changes merge **only between waves** (after the current wave gate,
     before the next wave starts), so no in-flight subphase has its contract
     changed underneath it. Every crate that uses the changed item is fixed in
     the same PR.
5. **Record.** The merger sets the request's status to `merged (<commit>)` or
   `rejected (<reason>)`, and logs any interpretation in `docs/DECISIONS.md`.
6. **Never wait.** While a request is pending, the requesting agent continues
   with a local workaround that does not edit `pin_core` (PLAN.md §2.3 rule 8).

## Additive vs breaking

**Additive** (existing code and existing data files keep compiling and parsing
unchanged):
- a new type, function, method, constant, module or trait with a default method;
- a new `Option`/`#[serde(default)]` field on an on-disk schema type, or a new
  optional TOML section;
- a new enum variant on an on-disk enum that is only parsed (e.g. a new
  `SwitchKind`), as long as no exhaustive `match` outside `pin_core` breaks;
- a new built-in ball profile or material pair.

**Breaking** (anything else), for example:
- renaming or removing any public item, field, variant or TOML key;
- changing a field's type, units or meaning; changing a capacity
  (`MAX_BALLS`, `ArrayVec` sizes, ...);
- adding a public field to a struct that dependants construct with a literal
  (`FrameStateSnapshot`, `HostView`, `FlipperConfig`, `BallProfile`,
  `HostCommandQueue`, ...), or a variant to an enum that dependants `match`
  (`FlipperShape`, `FlipperStagingMode`, `InputEventKind`, `TelemetryKind`, ...);
- a new trait method without a default;
- making an existing TOML key required, or changing `schema_version`;
- any change to the telemetry wire format (Appendix B), which also needs a
  `VERSION` bump.

## Template

```markdown
### CR-NNN: <short title>
- **Requested by:** Subphase <id> (<role>)
- **Status:** open | merged (<commit>) | rejected (<reason>)
- **Kind:** additive | breaking
- **Items:** <paths/items in pin_core, TOML keys, fixtures>
- **Change:** <exact signatures / TOML keys before → after>
- **Why:** <what the subphase cannot do without it>
- **Affected crates:** <list; "none" for additive>
- **Workaround meanwhile:** <what the requester does until it merges>
```

## Requests


### CR-001: `pin_core::spsc` bounded lock-free SPSC ring
- **Requested by:** Subphase 1.3 (Implementer)
- **Status:** merged (landed across wave-2 WIP snapshots cf246e2…e999444; approved in 1.3 review)
- **Kind:** additive
- **Items:** new file `crates/pin_core/src/spsc.rs`, new line `pub mod spsc;` in `crates/pin_core/src/lib.rs`; new tests `crates/pin_core/tests/spsc_stress.rs`, `crates/pin_core/tests/spsc_alloc.rs`.
- **Change:** new module, nothing existing changes:
  `pub fn channel<T: Copy + Send>(capacity: usize) -> (SpscProducer<T>, SpscConsumer<T>)` (panics if `capacity == 0`);
  `SpscProducer::{push(&mut self, T) -> Result<(), T>, len, is_empty, is_full, capacity}`;
  `SpscConsumer::{pop(&mut self) -> Option<T>, peek(&mut self) -> Option<T>, len, is_empty, capacity}`.
  Exact capacity (not rounded), FIFO, both ends `Send`; push/pop are wait-free, lock-free and allocation-free (only `channel` allocates).
- **Why:** PLAN §2.2: the main thread forwards timestamped `InputEvent`s to the sim thread (pin_app → pin_sim) and window events to the render thread (pin_app → pin_render) through SPSC rings; the ring type crosses crates, so it belongs in pin_core (Orchestrator Wave 2 decision "1.3 SPSC ring in pin_core").
- **Affected crates:** none (additive).
- **Workaround meanwhile:** none needed; the additive change is made in the new file as the brief allows.

### CR-002: `pin_core::sim` — the `SimStep` contract
- **Requested by:** Subphase 1.4 (Implementer)
- **Status:** merged (landed in Wave 3 WIP snapshots; approved with Subphase 1.4, see `[sp 1.4]` commit). Could not be a separate pin_core-only commit because the shared-tree WIP snapshots already carried it (as CR-001 in Wave 2).
- **Kind:** additive
- **Items:** new file `crates/pin_core/src/sim.rs` (with unit tests); in `crates/pin_core/src/lib.rs` one line `pub mod sim;`, one re-export line `pub use sim::{MAX_CATCH_UP_TICKS, SimStep, TICK_NS};` and one module-list doc line.
- **Change:** new module, nothing existing changes:
  `pub trait SimStep: Send { fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot); }`
  (tick is 1-based; `inputs` = events with timestamp ≤ end of the tick, in arrival order; the impl writes the whole snapshot and sets `out.tick = tick`);
  `pub const TICK_NS: u64 = 1_000_000;` `pub const MAX_CATCH_UP_TICKS: u32 = 8;`
  plus `impl<S: SimStep + ?Sized> SimStep for Box<S>` (so `Box<dyn SimStep>` is a `SimStep`) and the helper `pub const fn tick_end_ns(epoch_ns: u64, tick: u64) -> u64` (saturating `epoch_ns + tick·TICK_NS`).
- **Why:** the thread orchestrator (`pin_app`, 1.4) drives the sim through a trait object that `pin_sim` (2.5/6.0) implements, so the trait crosses crates (D4; Orchestrator "Wave 3 brief decisions").
- **Affected crates:** none (additive).
- **Workaround meanwhile:** none needed; the additive change is made in the new file as the brief allows.

### CR-003: `pin_core::haptics` — rich haptic event type
- **Requested by:** Subphase 3.4 (Implementer)
- **Status:** merged (with Subphase 3.4, review round 2 pass; additive, verified by the Reviewer)
- **Kind:** additive
- **Items:** new file `crates/pin_core/src/haptics.rs`, new line `pub mod haptics;` in `crates/pin_core/src/lib.rs`.
- **Change:** new module, nothing existing changes (`HapticsTelemetryEvent` and its `to_telemetry_entry` stay as they are):
  `#[non_exhaustive] pub enum HapticKind { CoilFire { coil: CoilId }, EosHit { flipper: FlipperId }, Bumper { mech: u16 }, BallDrop { ball: u16, position_mm: [f32; 3] }, BallRoll { ball: u16, position_mm: [f32; 3], mass_kg: f32, speed_m_s: f32 }, Impulse { source: u16 } }` (`Copy`, `PartialEq`, `Debug`);
  `pub struct HapticEvent { pub tick: u64, pub kind: HapticKind, pub strength: f32 }` (`Copy`) with `new`, `source() -> u16`, `is_impulse() -> bool`, `position_mm() -> Option<[f32; 3]>`, `to_telemetry_event() -> Option<HapticsTelemetryEvent>`;
  `impl From<HapticsTelemetryEvent> for HapticEvent` (→ `HapticKind::Impulse`);
  source-id constants `SOURCE_CLASS_SHIFT`, `SOURCE_INDEX_MASK`, `SOURCE_CLASS_{COIL,EOS,BUMPER,BALL_DROP,BALL_ROLL}` and `pub const fn class_source(class: u16, index: u16) -> u16`.
- **Why:** deliverable 1 of 3.4 (haptics bus) needs coil fire, EOS hit, bumper, ball drop and ball roll (position, mass, speed) events; the existing event has only `(tick, source, strength)`. Lives in `pin_core` because the producer is the sim (`pin_sim`/`pin_physics`) and the consumers are `pin_input` (rumble), `pin_audio` (SSF, telemetry).
- **Affected crates:** none (additive; `HapticKind` is `#[non_exhaustive]`, so later kinds stay additive too).
- **Workaround meanwhile:** none needed; the brief allows the additive change in the new file.

### CR-004: kind-specific `[[mechs]]` parameters and a `[mechs]` section of `physics_defaults.toml`
- **Requested by:** Subphase 2.4 (Implementer); closes 1.1 review N7
- **Status:** merged (with Subphase 2.4, review round 2 pass; additive incl. the spinner `friction` extension, verified by the Reviewer)
- **Kind:** additive
- **Items:** `crates/pin_core/src/schema/table.rs` (`MechDef`, new `MechParams`), `crates/pin_core/src/schema/physics.rs` (new `MechDefaults` and per-kind default structs, new field `PhysicsDefaults::mechs`), `crates/pin_core/src/schema/mod.rs` and `crates/pin_core/src/lib.rs` (re-exports), `physics_defaults.toml` (new `[mechs.*]` tables).
- **Change:**
  - `MechDef` gains 19 optional fields, all `#[serde(default, skip_serializing_if = "Option::is_none")]`, in table units (lengths mm without a suffix, angles degrees, masses g, other quantities SI with the unit in the key, as `FlipperDef` does): `angle_deg`, `layer: u8`, `radius`, `length`, `height`, `thickness`, `kick_speed_m_s`, `kick_reach`, `mass_g`, `force_n`, `stroke`, `spring_k_n_m`, `threshold_speed_m_s`, `inertia_kg_m2`, `damping_per_s`, `eject_speed_m_s`, `exit_layer: u8`, `exit_offset: [f32; 3]`, `auto_pull`. Each kind reads the subset documented on the field; the others are ignored. New `MechDef::params(&self) -> MechParams`.
  - New `pub struct MechParams` (`Copy`, `Default`, all the same `Option` fields, table units): the per-mech overrides detached from the names, so `pin_physics` can take them without the string fields.
  - New `pub struct MechDefaults { bumper: BumperDefaults, sling: KickerDefaults, kickback: KickerDefaults, drop_target: DropTargetDefaults, spinner: SpinnerDefaults, vuk: VukDefaults, plunger: PlungerDefaults, tilt_bob: TiltBobDefaults }` (all SI, `deny_unknown_fields`), and `PhysicsDefaults::mechs: MechDefaults` with `#[serde(default)]`; `MechDefaults::default()` equals the shipped `[mechs]` section (unit-tested), so a `physics_defaults.toml` without `[mechs]` still parses.
  - Fix round 1 of 2.4 (still additive): `SpinnerDefaults::friction` (pivot friction, rad/s², `#[serde(default)]` = 60, key `friction` in `[mechs.spinner]`); sling defaults force 60 N, stroke 12 mm; drop-target rise speed 0.5 m/s.
  - `physics_defaults.toml`: new `[mechs.bumper]`, `[mechs.sling]`, `[mechs.kickback]`, `[mechs.drop_target]`, `[mechs.spinner]`, `[mechs.vuk]`, `[mechs.plunger]`, `[mechs.tilt_bob]` tables.
- **Why:** PLAN Appendix A.1 says "kind-specific parameters default from physics_defaults.toml", but neither `MechDef` nor `PhysicsDefaults` had any such parameter (1.1 review N7). Subphase 2.4 needs per-mech geometry, coil force, spring, threshold and damping values with table overrides, so exit speeds come from parameters.
- **Affected crates:** none (additive: no `MechDef`/`PhysicsDefaults` struct literal exists outside `pin_core`; the new TOML keys are optional).
- **Workaround meanwhile:** none needed; the brief allows the additive change.

### CR-005: optional `area` of a `[balls] restrict` entry
- **Requested by:** Subphase 2.8 (Implementer)
- **Status:** applied (in the working tree with Subphase 2.8, snapshotted by the Orchestrator; additive, confirmed by the 2.8 Reviewer)
- **Kind:** additive
- **Items:** `crates/pin_core/src/schema/table.rs` (`BallRestriction`), TOML key `balls.restrict[*].area`.
- **Change:** `BallRestriction` gains `#[serde(default, skip_serializing_if = "Vec::is_empty")] pub area: Vec<[f32; 2]>` — a plan-view polygon in mm (≥ 3 points, even-odd rule) the ball centre stays inside on the listed layers; empty / omitted = the whole layer (the old meaning). Existing TOML parses and serialises unchanged.
- **Why:** PLAN 2.8 verification 2 accepts the XL ball once "restricted to the main deck's open area". The main deck is one collision layer (layer 0) that also runs under the upper deck, so a layer list alone cannot keep the XL ball out from under a 65 mm deck; the clearance validator (`pin_tablegen::clearance`) needs the area to know where the ball may be.
- **Affected crates:** none (no `BallRestriction` struct literal exists outside `pin_core`; the key is optional).
- **Workaround meanwhile:** none needed; the additive change is made in the working tree.
