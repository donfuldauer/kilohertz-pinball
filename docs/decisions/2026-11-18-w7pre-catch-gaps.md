## w7pre-catch-gaps — grey-box catch gaps closed

**Decision:** The grey box's below-floor 2 -> 0 catch under the wire now covers the whole old
catch volume (x 403..447, y 575..890, z -50..9 mm) except the generated ramp entry zone. This
resolves the frozen HUMAN_QUEUE.md item `[w7pre-ramp-entry follow-up]` "grey-box catch gaps"
(user-approved 2026-11-18).

- **Entry zone** (`tablegen:ramps[0].entry`, generated from `tables/greybox`, f32 mm): min
  (405, 595, -6), max (445, 634.7894, 20). The y max is f32 bits `0x441eb286`.
- **Overlap rule:** `pin_tablegen::layout::entry::overlaps` uses inclusive bounds, so two zones
  that only touch count as overlapping, and generation fails with a GenError. A catch face
  towards the entry zone therefore has to lie strictly outside it. Each such face is the
  adjacent f32 (`next_down` of a min face, `next_up` of a max face), written as Rust's shortest
  round-trip decimal. That leaves a sliver one f32 ulp wide in mm. It holds no representable f32
  position, so no mm position is uncovered.
- **In metres** (the loader's `mm_to_m`, `mm / 1000.0` in f32, which is monotonic): the catches
  stay disjoint from the entry zone after conversion. The x, z and y-max faces are one metre ulp
  apart. The y-min face (594.99994 mm against 595 mm) is two metre ulps apart: 0.5949999 m
  (bits `0x3f1851ea`) against 0.595 m (`0x3f1851ec`). Near 0.595 m the metre f32 grid is finer than the mm grid divided by 1000,
  so one metre value between them (about 0.59499997 m) lies in no zone. A ball centre would have
  to land on exactly that f32 at a sub-step end, and `TransitionSet::update` fires on entering a
  zone anyway. No mm literal can close this, because no mm f32 lies between 594.99994 and 595.
  The test asserts at most one ulp in mm and one or two ulps in metres.
- **Zones** (all `from_layer = 2`, `to_layer = 0`, mm, in table order):
  - `ramp_catch_below_mouth`: min (403, 575, -50), max (447, 594.99994, 9). Was max y 594.
  - `ramp_catch_below`: min (403, 634.7895, -50), max (447, 890, 9). Was min y 636.
  - `ramp_catch_below_side_left` (new): min (403, 575, -50), max (404.99997, 890, 9).
  - `ramp_catch_below_side_right` (new): min (445.00003, 575, -50), max (447, 890, 9).
  - `ramp_catch_below_under` (new): min (403, 575, -50), max (447, 890, -6.0000005).

  The side strips and the under band run the full length (y 575..890) rather than only along
  the entry zone. They are clear of the entry zone on x (or z) alone, so they do not depend on
  the entry zone's derived y max. Catches touch and overlap one another, which is allowed. All
  of them return the ball to layer 0, so their order does not matter. Every other layer
  transition is unchanged.
- **Brittleness:** `ramp_catch_below`'s y min 634.7895 is `next_up` of the generated y max,
  which follows from `layout.toml` (the rails' start plus `entry_width` along the wire). If the
  layout moves that bound:
  - Upwards: generation fails with the overlap GenError naming `ramp_catch_below`.
  - Downwards: a gap reopens. `greybox_below_deck_catch_has_no_gaps` fails, both on the sampled
    points (the sample set includes the entry bounds and their f32 neighbours) and on the
    face check, which requires a catch bound exactly at `next_up(entry.max.y)`.

  Either way the literal has to be updated by hand. The 595, 405, 445 and -6 faces come from
  the spline's start point and `entry_width`, and they are protected the same way.
- **Test:** `crates/pin_tablegen/tests/ramp_entry.rs::greybox_below_deck_catch_has_no_gaps`.
  - It generates the grey box and asserts that no 2 -> 0 zone overlaps the entry zone.
  - It samples 1,437,201 points: a deterministic product grid (x every 1 mm, y every 2.5 mm,
    z every 0.5 mm), plus the old volume's bounds, the entry bounds, three f32 steps either side
    of each bound, ±0.001, ±0.5 and ±1 mm around them, the old strips (y 594..595,
    634.79..636, x 403.5..404.5 and 445.5..446.5) and (404, 610, 0).
  - Each point must lie in the entry zone or in a 2 -> 0 zone, both in mm and, converted with
    `mm3_to_m`, under `pin_physics::world::LayerTransition::contains`.
  - Mutation check (run once, then restored; the file was compared with `cmp`). The test failed
    when any one zone was removed: mouth (31,008 points uncovered), lower (220,932), side_left
    (8,816), side_right (8,816), under (137,547). It also failed with the lower min set to
    634.79 (5,814) or the mouth max set to 594.9999 (1,938). With the lower min set to 634.7894
    (touching), generation failed with the overlap error.
- **Gate A1** (`GAMES=1000 SEED=1 scripts/gate_a1.sh`). Baseline: main c4aab01's
  `tables/greybox/table.toml`, swapped into the same worktree and target dir for the run and
  then restored. New: this change. Both exit 0, and every criterion PASSes in both.

  | soak | stuck / endless / escaped / non-finite | switches | median ball time | p99.9 tick (base → new) |
  |---|---|---|---|---|
  | novice | 0 / 0 / 0 / 0 both | 22/22 both | 14,745 ms both (reported) | 30.5 → 32.4 µs |
  | standard | 0 / 0 / 0 / 0 both | 22/22 both | 15,433 ms both (gated, PASS) | 30.3 → 30.7 µs |
  | expert | 0 / 0 / 0 / 0 both | 22/22 both | 13,603 ms both (reported) | 31.9 → 30.3 µs |

  Apart from the table hash and wall-clock timings, the `soak_report.json` files of the two runs
  are identical, game by game. No ball in these 9,000 games reached the closed gaps, which
  matches the earlier combined run. The change is a safety net for balls below the deck, with
  no measured effect on play.
- **Pinned expectations:** none found and none changed. The pin_sim and pin_app tests that read
  `tables/greybox` build the bundle at run time. Their hashes are compared run against run
  (replay, state hash, bundle hash), not against literals. `cargo test -p pin_sim` (326 passed)
  and `cargo test -p pin_app` (454 passed) pass unmodified.
- **Fixture:** `crates/pin_sim/tests/fixtures/autoplay/greybox_452eedf/` is unchanged. It is a
  frozen snapshot of an older grey box that `autoplay_endless.rs` uses to reproduce the
  pre-fix traps by name. Changing its catches would change the games it pins.

**Rationale:**
- The user approved closing the gaps.
- Abutting the entry zone at the adjacent f32 is the closest the unchanged generator rule allows.
- Full-length side and under strips make only one literal (the lower catch's y min) depend on
  a derived value.
- The sampling test covers the boundary values, where gaps hide, and it pins that one literal
  against layout drift.

**Alternatives:**
- Leave as is: rejected by the user's approval of this item.
- Change the generator so that catches may touch the entry zone (a strict overlap test), or so
  that it derives the below-floor catches: out of scope, because `crates/pin_tablegen/src` is not
  owned by this unit and the generator rule stays as is. A touching rule would also make a ball
  exactly on the shared face satisfy both zones.
- Fewer zones:
  - The mouth and lower zones cannot merge with the side strips or the under band, because any
    single box spanning the entry zone's x or z range across its y range overlaps it.
  - Three zones (one under band plus two full-length sides) would still leave the mouth and
    lower strips above z -6 uncovered.
  - Four zones are possible only if one zone covers both the mouth and the lower part, which it
    cannot do without crossing the entry zone. Five is the minimum for an axis-aligned cover of
    a box with an interior box removed when the removed box touches no face of the outer one
    except the top: one zone per uncovered side.
- Extending the existing two zones to y 594.99994 and 634.7895 only, without the new zones:
  rejected, because the side strips and the under band would stay open.
