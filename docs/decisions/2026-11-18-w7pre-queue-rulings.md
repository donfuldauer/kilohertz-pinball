# Decisions: w7pre-queue-rulings

User rulings of 2026-11-18 (the user approved the Orchestrator's recommendations) closing three
open items from the frozen `docs/HUMAN_QUEUE.md`.

## w7pre-queue-rulings-vuk-scatter — VUK eject scatter magnitudes accepted

**Decision:** The current engine constants are accepted as the defaults: `VUK_SCATTER_LATERAL`
3 mm (also capped at a quarter of the scoop radius and of the exit-zone half size),
`VUK_SCATTER_ANGLE_DEG` 4 degrees and `VUK_SCATTER_SPEED_FRAC` 4 %. They live in
`crates/pin_physics/src/mech/mod.rs` (about lines 184 to 193). This closes the "magnitudes still
to confirm" part of DECISIONS.md "w7pre-vuk-scatter — deterministic VUK eject scatter". Revisit
if a Wave 7 table's soak shows eject loops or exit misses.

**Rationale:**
- The combined Gate A1 (A1 ramp entry plus A3 scatter, GAMES=1000 SEED=1) passed with 0 stuck,
  endless or escaped balls at all skills and a standard median of 15,433 ms.
- The 1,000-game soak of the frozen `greybox_452eedf` fixture found no VUK-deck loop.
- The values are small next to the 2 mm knife-edge (plus or minus) that caused the loop.

**Alternatives:**
- Per-table tuning fields: not needed, and would require a contract change.
- Larger magnitudes: unneeded, and they risk exit-zone misses.

## w7pre-queue-rulings-backbox-escape-box — backbox escape-box behaviour confirmed

**Decision:** The behaviour of DECISIONS.md "w7pre-backbox-colliders — backbox colliders in the
runtime world" is confirmed as implemented:
- With a `backbox_colliders.glb`, the backbox geometry and backbox-layer flippers grow the single
  union escape box.
- The playfield glass keeps its height and leaves the backbox layers out.
- Tables without the file are unchanged.
- Shared-layer overlaps are a load error.
- Backbox collision layers are restricted to 4..=7.
- The total collider meshes are capped at 4096, enforced both at generation and at load.

**Rationale:** It is covered by the `pin_sim` `backbox_colliders` and `backbox_colliders_hostile`
suites (11 + 11 tests) and the `pin_tablegen` limit tests. It is consistent with PLAN §5.5 (a
backbox ball never touches layers 0 to 3).

**Alternatives:**
- A separate backbox escape box: rejected in the original decision.
- Per-layer glass: rejected as a larger change.

## w7pre-queue-rulings-ramp-entry-corner-clip — ramp-entry corner-clip limit accepted

**Decision:** Keep it as an accepted, stated limit: layer transitions test only sub-step end
points (`TransitionSet::update`), so a sub-step that clips a zone corner can skip it. No swept
segment-vs-AABB test now. Trigger to revisit: any soak finding traced to a missed entry or exit
transition, or a Wave 7 table with steep oblique ramp entries.

**Rationale:**
- Generated ramps funnel the ball along the start tangent, where the chord is the full
  `entry_width`.
- Gate A1's soak detects the symptoms a skip would cause (StuckBall, EscapedBall or EndlessBall),
  and the combined Gate A1 showed none in 3,000 balls.
- A swept test changes simulation output for every table and would need a new determinism and
  Gate A1 baseline for no observed benefit.

**Alternatives:**
- Implement the swept test now: rejected, it changes sim output with no observed defect.
- An authoring lint on entry-zone size versus sub-step length: it does not address corner clips,
  which are width-independent.
