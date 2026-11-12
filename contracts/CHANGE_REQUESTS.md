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

_None yet._

### CR-001: `pin_core::spsc` bounded lock-free SPSC ring
- **Requested by:** Subphase 1.3 (Implementer)
- **Status:** open
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
