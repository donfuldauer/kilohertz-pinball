# Feature Lab (Subphase 6.4)

A playable showcase table that exercises every restored feature, so that
the automatic soak (Gate A2) and final acceptance cover them:

* the **Banana flipper** (2.6): the lower-right flipper is a `CurvedBanana`
  (76 mm chord on a 120 mm bend, 36.92 degree arc, positive sweep: the tip
  curls up the table) opposite a straight lower-left flipper;
* **magnet remanence** (2.7): a holding magnet `hold_magnet` (tau 200 ms)
  with a pole opto, served by the dispenser `magnet_feed` with either a
  `standard_steel` (remanence 0.05) or a `low_rem_steel` (remanence 0) ball;
* the **Glo-Ball with Magnus** (2.7): `uv_globall` is a declared profile and
  `[physics] globall_magnus_coeff = 3e-5` (three times the default); the
  dispenser `glo_feed` serves it into the shooter lane and the rules plunge
  it round the orbit;
* the **79.4 mm XL ball and the clearance validator** (2.8):
  `jumbo_xl_resin` is restricted (`[balls] restrict`) to the XL area, the
  chamber under the left inlane (layout.toml `walls[3]`, `walls[14]`,
  `walls[15]`, plan outline `[[0, 0], [0, 330], [130, 194.9], [130, 0]]`),
  served there by `xl_feed` and drained by its own trough switch
  `xl_trough`;
* the **backbox mini-playfield** (5.5): a 400 x 300 x 35 mm box on
  collision layer 4 with one vertical flipper
  (`tablegen:backbox.flippers[0]`, button `left_upper`), fed by the VUK
  `tablegen:backbox.vuk` at the top of the main playfield's VUK lane and
  draining back onto the playfield.

Design notes and choices: `docs/decisions/2026-11-18-6.4.md`.

Only the sources live here; the generated `playfield.glb`,
`backbox.glb`, `backbox_colliders.glb`, the merged `table.toml` and the
placeholder art and sounds are build output and are not committed.

| File | What |
| --- | --- |
| `layout.toml` | `pin_tablegen` layout of the main playfield: cabinet, shooter lane, inlanes, drain funnel, orbit lane, VUK lane, XL area |
| `backbox_layout.toml` | the backbox: box, vertical flipper, VUK entry, drain, floor wall |
| `table.toml` | base manifest: switches, coils, flippers (Banana), magnet, dispensers, mechs, ball profiles and the XL restriction, shots, modes, scenes, persist keys |
| `rules.lua` | rules (integer only): attract, 3-ball game, mode selector and the five modes, game over |
| `backglass.toml` | placeholder translite, a 128 x 32 DMD and the `[backbox]` scene and camera |
| `glass.toml` | playfield glass underside, 112 mm (22 mm over the 90 mm walls) |
| `display/*.toml` | display scenes (5.3 placeholders): attract, score, game over, one per mode |
| `build.sh` | the one-command build |

## Build

From the repository root:

```sh
tables/feature_lab/build.sh target/feature_lab_bundle
```

The `pin_tablegen` command line does not read `backbox_layout.toml`, so the
generation runs in the `build_feature_lab` example of `pin_app`
(`cargo run -q -p pin_app --example build_feature_lab -- <out_dir>`), which
uses the `pin_tablegen` library: `layout.toml` gives `playfield.glb`,
`backbox_layout.toml` gives `backbox.glb` and `backbox_colliders.glb`, both
merged into one `table.toml` (the generated backbox VUK keeps its generated
exit on layer 4 above the entry hole; `aim_backbox_vuk` in the example only
sets its eject, which the generator leaves unset; see the decisions file).
The example then copies the
rules, backglass, layout, glass and scenes and writes the `pin_assetgen`
placeholder kit (title "Feature Lab", the table's palette, seed 0).
`build.sh` finally runs `pin_tablegen validate <out_dir>` (ball clearances of
every declared ball, with the XL ball restricted to its area). The tests
build the same bundle with `build_feature_lab_bundle(out_dir)`
(`crates/pin_app/tests/feature_lab_support/`), which includes the example.

## Run

```sh
cargo run -p pin_app -- --table target/feature_lab_bundle
cargo run -p pin_app -- --headless-ticks 10000 --table target/feature_lab_bundle
```

## Keys

Generic keyboard profile: Left/Right Shift flippers (the right one is the
Banana), Left Ctrl the backbox flipper (`left_upper`), 1 start, Space or
Return launch, the action button (`action`) advances the mode selector, T
tilt.

## Mechanics

* **Shooter lane and orbit** (grey-box geometry): the plunger serves every
  ball round the orbit top; the shooter-lane wall runs up to y 930, so a ball
  on the playfield cannot drop back into the lane.
* **Shooter-lane gate** (`layout.toml` walls[16], mech `walls[16]` of kind
  `gate` on coil `lane_gate_coil`, number 11): closes the 70 mm gap
  between the lane wall's top (470, 930) and the corner wall at (470,
  1000). It is closed (solid) by default and open while its coil is on; the
  rules pulse it for 1.5 s with every plunger pulse, so a plunged ball
  leaves the lane through it (plunges cross it at y 964..981 well within
  1.5 s). Without it, bumper kicks sent balls up the right side into the
  lane from the top, where the player relaunched the same ball again and
  again (Gate A2 run 1: endless balls, mostly the light Glo-Ball).
* **Flippers**: a straight lower-left flipper and the Banana lower right.
  The Banana's curl sends its shots up the right of the table (mirrored
  cradle headings 96..118 degrees against 77..110 for the straight bat),
  at the VUK lane; there is therefore only one slingshot, on the left.
* **Pop bumpers** as on the grey-box.
* **VUK lane** (x 405..445, right of centre): a ball rolling up it reaches
  the VUK scoop at (425, 622); the rules eject it 0.4 s later into the
  backbox.
* **Backbox** (layer 4): the VUK puts the ball on layer 4 at its generated
  exit, just above the backbox's entry hole, and ejects it at 2.1 m/s up the
  backbox plane plus 0.35 m/s across it; it rises out of the entry channel,
  lands on the vertical flipper (`left_upper`) or the floor, and leaves down
  the sloping floor through the drain (opto `bb_drain`) back onto the
  playfield above `bumper_2`. It drains on its own without a flip. A safety
  transition `bb_catch` returns a ball that falls back through the entry
  hole (a flipped ball that drops into the entry channel) to the
  playfield, also when it drifts towards the player or the back as it
  falls (the zone spans y 780..990). A second zone, `bb_catch_left` (x
  45..59, y 895..935, z 25..96), catches a ball thrown left off the top of
  the backbox's walls[1] that falls through the hole left of `bb_catch`,
  and returns it into the orbit lane (never into the closed strip between
  the cabinet wall and the orbit's outer rail).
* **Left inlane**: the sling (`sling_left`, at (121.6, 310.8)) and its
  guard (walls[12]) sit 30 mm further out than on the grey-box, so the
  inlane is 75.6 mm wide at the guard's top: two balls arriving together
  (the magnet comparison balls) cannot wedge across its mouth.
* **Holding magnet** `hold_magnet` at (370, 420), on the Banana's line below
  the VUK lane; `magnet_feed` serves comparison balls at rest onto its pole.
* **XL area**: the chamber under the left inlane, closed to every other
  ball; the XL ball rolls down past `xl_lane` into `xl_trough`.

## Modes

The selector `mode_sel` advances by one (mod 5) at the start of every ball
and on every action-button press during a game. It is drawn at random
(`pinball.random`) when the table loads and is not reset between games, so
an autoplay bot that never presses the action button still meets every
mode. The selected mode starts when its qualifying shot (the `[[shots]]`
entry of the same index: two switches in order within the window) is made;
each mode starts at most once per ball, and never ends the player's ball.

| mode | selector index | start condition | shot switches | scene |
| --- | --- | --- | --- | --- |
| `banana_shots` | 0 | shot `banana_vuk`: up the VUK lane into the backbox VUK within 3 s (the Banana's shot) | `vuk_lane`, `bb_vuk_sw` | `scene_banana_shots` |
| `backbox_play` | 1 | shot `backbox_run`: a ball through the backbox, VUK to backbox drain within 10 s | `bb_vuk_sw`, `bb_drain` | `scene_backbox_play` |
| `xl_ball` | 2 | shot `pops_down`: `bumper_3` then `bumper_2` within 5 s | `bumper_3_ring`, `bumper_2_ring` | `scene_xl_ball` |
| `magnet_compare` | 3 | shot `magnet_lane`: over the magnet into the VUK lane within 2 s | `magnet_opto`, `vuk_lane` | `scene_magnet_compare` |
| `glo_ball` | 4 | shot `pops`: `bumper_2` then `bumper_3` within 5 s | `bumper_2_ring`, `bumper_3_ring` | `scene_glo_ball` |

* `banana_shots`: further Banana shots up the VUK lane (complete
  `banana_vuk` shots: `vuk_lane`, then `bb_vuk_sw` within 3 s) score
  25 000 for the rest of the ball, paid when the backbox VUK captures the
  ball; a `vuk_lane` hit alone scores its 100.
* `backbox_play`: the backbox drain scores 5 000 instead of 500 and the
  backbox flipper 1 000 a flip for the rest of the ball.
* `xl_ball`: the XL ball is served into the XL area; `xl_lane` scores 500;
  the mode ends when it drains (`xl_trough`) or after 20 s.
* `magnet_compare`: `magnet_feed` serves a `standard_steel` ball at rest onto
  the energised magnet's pole, then, 1.5 s after it has left, a
  `low_rem_steel` ball; the magnet holds each for 1 s and the rules measure
  the time from the coil going off to the ball leaving the pole opto
  (`cmp_release_ms_standard`, `cmp_release_ms_low_rem`; measured 329 ms and
  253 ms). The comparison balls then play on as extra balls.
* `glo_ball`: the Glo-Ball is served into the shooter lane and plunged
  round the orbit 1.5 s later; bumpers score double; the mode ends after
  30 s or with the ball.

Every mode start scores 2 000 and increments the persist key `mode:<id>`.

## Persist keys

`games_played`, `high_score`, `ball` (1..3 in a game, else 0),
`game_state` (0 attract, 1 game, 2 game over), `mode_sel` (0..4),
`mode:banana_shots`, `mode:backbox_play`, `mode:xl_ball`,
`mode:magnet_compare`, `mode:glo_ball` (start counts),
`cmp_release_ms_standard`, `cmp_release_ms_low_rem` (last measured release
time, ms; 0 until measured).

## Rules

* START begins a 3-ball game; each ball is served into the shooter lane and
  launched with the launch button (the plunger's auto-plunger coil).
* Every plunger pulse (the launch button, and the Glo-Ball's automatic
  plunge in `glo_ball`) also pulses `lane_gate_coil` (11) for 1.5 s: the
  shooter-lane gate opens for the plunged ball and is closed the rest of
  the time, so no ball re-enters the lane from the top.
* A ball ends when the last ball in play has drained through a trough
  switch (`trough_1`, `trough_2`, `xl_trough`); after the third the game is
  over and the attract scene returns 4 s later.
* Points: bumper 10 (20 in `glo_ball`), sling 10, orbit entry 100, orbit
  top 5 000 when the orbit is made from the bottom (else 500, e.g. every
  plunge), a flipper shot (orbit entry, VUK lane, magnet opto or a bumper
  within 1 s of a lower flipper reaching its end of stroke) 5 000, VUK lane
  100, backbox VUK 500, backbox drain 500, magnet opto 100, XL lane 500,
  mode start 2 000, a comparison measured 10 000. The orbit entry and the
  flip that qualify an orbit or a flipper shot count only within the same
  player's ball.

## Autoplay soak self-check

Release `pin_sim`, `pin_sim soak <bundle> --games 100 --seed 1 --skill
<skill> --threads 2` (no `--gate`), on the bundle `build.sh` writes:

| skill | result | stuck / endless / escaped / non-finite / script error / queue overflow | never-hit switches | shots never made | modes never started | median game length | median score |
| --- | --- | --- | --- | --- | --- | --- | --- |
| novice | PASS | 0 / 0 / 0 / 0 / 0 / 0 | none | none | none | 87.7 s | 143 730 |
| standard | PASS | 0 / 0 / 0 / 0 / 0 / 0 | none | none | none | 73.9 s | 113 520 |
| expert | PASS | 0 / 0 / 0 / 0 / 0 / 0 | none | none | none | 73.4 s | 116 680 |

Measured 2026-11-19 (6.4 fix round 1: VUK exit on layer 4, entry hole
70 mm, `bb_catch` reaching 100 mm in front of the box). Median score is
the report's `game score` median; median game length is the median of the
per-game ticks. Mode starts per 100 games (banana_shots, backbox_play,
xl_ball, magnet_compare, glo_ball): novice 37, 44, 29, 32, 36; standard
32, 33, 16, 26, 35; expert 35, 38, 22, 26, 28. Every declared shot is made
at every skill (fewest: `magnet_lane`, 248 at standard). The backbox
flipper's EOS switch is closed by the bot's own `left_upper` presses (72,
37 and 44 times per 100 games). The bot models a flipper in plan view, and
this flipper's pivot is at the back panel, 13.7 mm behind a ball in the
box. So the bot sees a backbox ball in the bat's sector only when the ball
is about 120 mm above the pivot, near the top of the VUK's arc.

The standard median game length (74 s) is inside 60..360 s. Scores do
**not** rise with skill: the heuristic novice bot plays longer games on this
table and hits more of everything (VUK lane, backbox, pops); re-weighting
the points towards flipper shots kept the order (DECISIONS file "6.4 —
scores by skill (R11e not met)", queue `docs/queue/6.4.md`).
The same inversion occurs on the Apex and grey-box tables: its cause is the
autoplay bot's flip timing in `pin_sim` (a 30 ms flip lead plus a one-sided
0..jitter delay), fixed in the separate unit fix-2.9-bot-timing; the rules
are not changed for it (queue `docs/queue/6.4.md`).
