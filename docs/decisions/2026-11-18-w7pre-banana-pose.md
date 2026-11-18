## w7pre-banana-pose — Banana bat pose is gated by a geometric test, not by eye

**Decision:** The check "the bat leaves its pivot along the flipper angle and the tip curls up the
table" (frozen HUMAN_QUEUE.md item "[2.6 bat pose, by eye — still open]") is now an automated,
GPU-free test, `crates/pin_physics/tests/banana_pose.rs`. A rendered Banana in Feature Lab (6.4) is
no longer needed for this check; a visual look there is optional polish.

- **Target.** The test checks the physics collision shape: `CurvedBanana` built with
  `CurvedBanana::from_config(config, FlipperTuning::default().bat_height_m)`, as `FlipperSet::new`
  builds it. This is the surface the ball hits. No render-side Banana mesh exists, because pin_render
  skips flippers. The world pose is the solver's own (`FlipperSet::pivot_m`, `bat_direction`, and bat
  y = normal × direction). A cross-check also queries the set's own `MovingBodies::closest_point` at
  the probed tip.
- **Convention.** Up the table is world +y. The mounting normal is +Z. A positive `arc_sweep_deg` means
  the concave face leads the swing, so the tip curls up the table on both the left flipper (bat +y)
  and the right flipper (bat −y). A negative `arc_sweep_deg` flips the curl to the trailing side, down
  the table.
- **Probing rule.** The pose comes only from `BatShape::closest_point`, evaluated at mid-height
  z = 15 mm in the bat frame in f64:
  - the pivot end, from the y-axis zero crossings at x = 0 and the outward normals there;
  - the farthest point, from a 0.1° scan of rays marched inward and bisected on the signed distance,
    refined by golden-section search.

  The shape's own helpers (`tip_center`, `centerline_tangent`, `concave_side`) are not used to derive
  any check. They appear only in a cross-check that they agree with what the probe found.
- **Pure checker.** The checks are a pure function `check_pose(pose, expect) -> Result<(), String>`.
  Permanent negative controls feed it wrong poses, built with the same probe, and assert `Err`:
  - a `ConcaveSide::MinusY` bat on the left flipper;
  - the bat direction rotated ±3°;
  - a left pose checked against the right flipper's side;
  - a straight bat of the same chord.

  A further control checks that a flipped-sweep pose passes its own expectation and fails the
  up-table one.
- **Tolerances.**

  | Check | Tolerance |
  | --- | --- |
  | Outline tangent at the pivot (normals vs ±bat y; tangent vs `bat_direction`) | 0.05° |
  | World bat direction vs the configured angle, at rest and on the end stop | 0.01° |
  | Pivot crossings symmetric about the pivot and at ± the tube radius | 10 µm |
  | Farthest-point radius vs length + tube | 50 µm |
  | Farthest-point direction vs ±\|arc_sweep_deg\|/2 | 0.6° (half the 1° lint, plus slack) |
  | Curl magnitude vs 2·L·sin(\|arc_sweep_deg\|/4), the chord offset the authored sweep implies | 1 mm |
  | Left/right mirror agreement of the curl's world-y | 50 µm |

  The curl's world-y component and its bat-y component must have the expected sign, strictly.
- **Parameters.**
  - Chord L: {60, 76, 90} mm.
  - Tube: {6.5, 8, 11} mm.
  - R: {1.1·L/2, 60, 100, 150, 250, 400} mm.
  - Authored |arc_sweep_deg|: the implied 2·asin(L/(2R)) rounded to 0.1°, moved by
    0 / +0.9 / −0.9 / +0.5° in a fixed cycle. This always stays within the 1° sweep lint of
    pin_assets (`TOLERANCE_DEG`).
  - Both signs of `arc_sweep_deg`, both flippers, three rest angles each.
  - Each case is checked at rest and on the end stop. The end-stop pose is reached by energising the
    flipper and stepping 1 ms ticks of simulated time until `angle_deg` equals `end_angle_deg()` and
    the bat is still.

  The grid is 648 flipper cases (1296 poses, 324 mirror pairs). A proptest block draws 64 random valid
  cases from the same ranges, using a fixed ChaCha seed and no persistence, as in
  `pin_app/tests/a1_bundle_passive.rs`.
- **Scope limits.**
  - Mounting normal +Z only. Tilted mounting planes and the −Z singularity stay with the existing
    queue note ("[2.6 review round 2, N5]").
  - "Up the table" is only meaningful while the bat points roughly sideways. Positive
    `arc_sweep_deg` uses left angles in [−60°, 30°] and right angles in [150°, 240°], with sweeps of
    20–60°. Both the rest and end angles stay inside these ranges by construction.
  - Negative `arc_sweep_deg` uses left angles in [−55°, 30°] and right angles in [150°, 235°]; see
    the next entry.
- **Proof of failure.** Three temporary scratch mutations of product code were each applied and then
  restored. After restoring, `git diff --stat -- crates/*/src` was empty.
  1. `ConcaveSide::leading` with PlusY and MinusY swapped: 6 of 8 tests fail.
     - The grid fails with "tip direction: -65.379° in the bat frame, expected 65.400°; curl side
       (bat): bat-y -54.545 mm, expected on bat +y".
     - The proptest fails the same way.
     - So do the reference and flipped-sweep controls, and the wrong-side controls, whose premise
       assertions no longer hold.
  2. `FlipperSet::bat_direction` turned by +3°: 5 of 8 tests fail.
     - The grid fails with "world direction: 3.0000° from the configured -60.000° (tol 0.01°);
       pivot tangent: … 3.0000° from the configured angle".
     - The proptest fails the same way.
     - So do the reference and flipped-sweep controls.
     - The −3° rotation control now cancels the mutation, so it does not fail.
  3. `unmirror` without the `sign()` factor: the grid and the proptest fail on the cross-check:
     "tip_center() (0.0250, +0.0545) is not the probed tip centre (0.0250, −0.0545)" (right flipper,
     MinusY).
     - `closest_point` does not go through `unmirror` (`outline` applies the side itself), so the
       collision shape and the pose checks are unaffected by this mutation.
     - Only the helper cross-check catches it. That is the intended split: the collision query is
       what is gated, and the helpers must agree with it.

**Rationale:** The by-eye check was blocked until the first table with a Banana (Feature Lab, 6.4)
exists, and it was never a gate. A geometric test of the collision shape is deterministic and runs in
about a second. It checks what the ball actually meets, and it separates the three ways the pose can
be wrong: the angle, the side and the sign. The mutations show that it fails on each.

**Alternatives:**
- By eye in 6.4: blocked until that table exists, and never a gate.
- Testing a render mesh: none exists, because pin_render skips flippers.
- Testing through ball contact: banana_criteria already does this. It is indirect and cannot separate
  the side from the angle.

## w7pre-banana-pose — Down-table angle range for a negative arc_sweep_deg

**Decision:** Negative `arc_sweep_deg` cases use left angles in [−55°, 30°], mirrored to right
angles in [150°, 235°], instead of [−60°, 30°] / [150°, 240°].

**Rationale:** On the left flipper the flipped curl points at angle − 90° − φ/4, where φ is the
implied arc angle. It is below the horizontal (down the table) only for angle > −90° + φ/4. The
tightest valid bend, R = 1.1·L/2, has φ = 130.8°, so the condition becomes angle > −57.3°.

At −60° such a bend's flipped curl is 2.7° past horizontal. Its world-y is +3.0 mm (60 mm chord,
6.5 mm tube), although it is on the correct bat side. This is geometry, not a defect. The
range is built so such cases are not sampled, rather than skipped at run time. The brief requires
that out-of-range cases be excluded by how the ranges are built.

**Alternatives:**
- Narrow both signs to [−55°, 30°]: loses the −60° coverage of the up-table cases.
- Drop the world-y sign check for flipped cases: weaker than the brief asks.
- Cap R higher for flipped cases: loses the tightest bends.
