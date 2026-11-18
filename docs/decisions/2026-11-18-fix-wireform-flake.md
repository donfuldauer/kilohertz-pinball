# Decisions: fix-wireform-flake

## fix-wireform-flake — guide cast false contact on closest-point tie

**Decision:** (human-ruled, 2026-11-18) Variant B, in its own PR. In
`GuideCurve::cast_sphere_counted` (`crates/pin_physics/src/world/wireform.rs`), the "touching
and closing" branch now also requires the closing rate taken across the curve to be closing:
`(rate < approach && self.closing_direction(&cp, p, normal).dot(velocity) < approach) ||
(gap < -GUIDE_SKIN && rate < 0.0)`. `hit.normal` and every other use of `normal` are unchanged.
The penetration disjunct is unchanged. The tie figure in the `NORMAL_TANGENT_SLACK` doc
("~3e-4") and in the `closing_direction` doc (`2^-11.5`, the same figure) is corrected to the
measured ~1.1e-3. Regression test:
`crates/pin_physics/tests/wireform.rs::guide_cast_arc_tangent_touching_sample_tie_is_free`.
This changes simulation output on tables with guides (see the evidence below), which is why the
human ruling is needed (PLAN §2.7 step 3).

**Rationale:** `prop_guide_cast_arc_wall` failed in about 0.5 % of runs at the
`res.hit.is_none()` assertion (HUMAN_QUEUE item `[wireform-flake]`). Minimal input on x86_64:
inside=false, gap=0.0, at=117.18233, angle=0.0, speed=0.05, travel=0.001. Before the fix it gives
`Some(GuideHit { time: 0.0, t: 23.4375, .. })`. `CubicSpline::closest_point` refines to
s=0.43646345, but the f32 Bezier evaluation of that point lands about 2.6e-8 m farther away
(d²=1.8225123e-4) than the through-sample at t=23.4375 (d²=1.8225107e-4), which lies 14.8 µm
along the wall: a relative 1.1e-3, not the ~3e-4 the docs claimed. The sample wins, and
`normal_toward` tilts by 1.0e-3 rad. Then `rate = normal·v = -1.0e-3·speed` falls below
`approach = -APPROACH_EPS·speed`, and with gap <= GUIDE_SKIN the cast returns a Contact at t=0,
although the true rate across the curve is +9.2e-5·speed (separating). That breaks the cast
contract (DECISIONS.md "2.1 — Review fix round 1 (guide cast)": a non-overlapping sphere moving
along or away is never a contact). Commit 01d864d fixed the same tie for the step bounds only,
through `closing_direction`. Variant B applies the same across-curve rate to the contact
decision. `closing_direction` is allocation-free and already runs in the cast loop (D5).

Evidence on this branch: the regression test fails on origin/main 04a9a4a and passes here.
`prop_guide_cast_*` passed 1000 of 1000 repeated runs. `cargo test -p pin_physics` passed 831
tests in 107 targets, including `guide_ramp::normal_force_is_smooth_away_from_entry_and_exit`
and `ball_seam_bitident`. The CI replay hash (`pin_sim crates/pin_sim/tests/fixtures/replay_table
--ticks 10000 --record-inputs`, release) is 25097c21c2c3adc1 on both trees, and the recordings
are byte-identical. The frozen `greybox_452eedf` autoplay games did change (standard skill,
seed 1):
- game 6: before, game_over at tick 72,764 (score 101,850); after, game_over at tick 82,935
  (score 97,480). Neither has a finding.
- game 98: before, EndlessBall bumper_1_ring at tick 311,401 and max_ticks at 330,000; after,
  game_over at tick 55,812, with no finding.
- game 334: before, StuckBall walls[3] at tick 30,296; after, game_over at tick 39,400, with no
  finding.

So `pin_sim` `autoplay_endless::the_pre_fix_greybox_traps_are_caught_and_named` fails on this
branch ("game 6 end tick", left 82935, right 72764). That file is outside this unit's paths. A
1000-game scan of the fixture on this branch still finds both loop classes. The bumper loop
shows up as EndlessBall before the cap in games 82, 227, 408, ... The sling pocket shows up as
StuckBall walls[3] in games 60, 461, 613, 663, 749, 763, 960 and 997.

**Alternatives:**
- Variant A (use `closing_direction` as the cast normal everywhere, including `hit.normal`):
  rejected. It breaks `guide_ramp::normal_force_is_smooth_away_from_entry_and_exit`: the rail
  force goes from 1.283 to 1.767.
- Rely only on 01d864d's step-bound fix: rejected. It changes only the bounds used while
  approaching (gap > GUIDE_SKIN). It does not cover the contact decision for a touching sphere,
  which still used the tilted `normal`, and that is exactly the failing case.

## fix-wireform-flake-p2 — greybox_452eedf autoplay expectations re-pinned (2026-11-18)

**Decision:** `pin_sim` `autoplay_endless::the_pre_fix_greybox_traps_are_caught_and_named` is
re-pinned on the frozen `greybox_452eedf` fixture. Three values change: game 6's end tick
72_764 -> 82_935 (still game_over, no EndlessBall, nothing naming `vuk_sw`); the bumper-loop
game 98 -> 82 (EndlessBall `bumper_1_ring` at tick 305,924, end max_ticks at the 330_000 cap);
the sling-pocket game 334 -> 763 (StuckBall `walls[3]` at tick 33,841, end stuck, cap 120_000).
The sling subject string's contact coordinate also changes, from `walls[3] @ (112, 243, 14) mm`
to `walls[3] @ (112, 247, 14) mm` (same wall, same `walls[3] @` prefix, still an exact match):
none of the eight sling candidates (763, 663, 613, 960, 749, 997, 60, 461) names (112, 243, 14);
the named contact point varies from game to game, and game 763 gives (112, 247, 14). Intent,
assertion kinds, trap kinds and caps are unchanged; no assertion is removed or loosened. The
replay hash 25097c21c2c3adc1 is unchanged. Gate A1 re-run on this branch (GAMES=1000 SEED=1,
release): every row PASS, 22/22 switch coverage, 0 stuck/endless/escaped/non-finite, 0
censored; medians 15,050 / 15,505 / 13,822 ms (novice / standard / expert); p99.9 tick 47.4 /
48.1 / 43.0 us; dead-spot depth REPORT 14/28 and 16/28 variants (unchanged).

**Rationale:** Variant B (user ruling 2026-11-18) makes the guide cast contact branch also
require a negative closing rate, which changes guide contact decisions and therefore
simulation output on tables with guides. On the frozen fixture the old trap games 98 and 334
now drain to game_over at t55,812 and t39,400 with no findings, so they no longer exercise the
detectors. A 1000-game scan (standard skill, seed 1) still finds both loop classes in other
games; the first listed candidate of each class that meets every unchanged assertion is used.

**Alternatives:**
- Loosen the sling subject to a prefix match or drop the coordinate: rejected, it would weaken
  the check.
- Edit the frozen fixture or add physics special cases to keep games 98 and 334 trapped:
  rejected; fixtures are frozen and the solver must not branch on test cases.
- Keep games 98 and 334 and assert game_over: rejected; that would stop proving the detectors
  name the traps.
