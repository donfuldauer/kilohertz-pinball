# Grey-box table (milestone M1)

A playable grey-box table that exercises every mechanism the engine
simulates: three flippers (`left_flipper`, `right_flipper`, `left_upper`),
two slingshots, three pop bumpers, a left orbit, a layer-2 wireform ramp onto
an upper deck (layer 1), a VUK that feeds the deck, a magnet with an eddy
sensor (Powerball detector), a Newton ball, a drop target, a plunger in a
shooter lane served by a ball dispenser (steel balls and one ceramic
Powerball), a two-switch trough and the tilt switch. Design notes:
`docs/DECISIONS.md` "6.0 — grey-box table layout".

Gate A1 geometry (`docs/DECISIONS.md` "6.0 fix — Gate A1 grey-box
geometry"): the inlane walls end on the back of the flipper pivot caps; the
right drain-funnel wall meets the cabinet bottom at x 335 and the two trough
zones cover the whole funnel bottom; the drop target faces 290°. The ramp
entry is a 40 mm lane (x 405..445) whose right wall reaches down to y 430,
so it takes balls rolling straight up beside the right inlane and the left
flipper's 25..35° shots, which glance off that wall. Its layer change is the
generated entry zone over a layer-2 floor; the 13 mm gauge rails rise out of
that floor, layer-2 side rails line the wire, catch zones beside and below
it return a stray ball to layer 0, and covers at z 80 mm hold a fast ball on
the wire's level end. The upper deck is at z 50 mm and the Newton ball at
(330, 450).

Gate A1 play length (`docs/DECISIONS.md` "6.0 fix — grey-box play length
(Gate A1 median), iteration 3"): the orbit's lower end bends right and ends
at (85, 615) above the upper-left flipper, so the plunge (which rides the
orbit top first) comes out in mid-field instead of onto the left inlane;
`orbit_entry` sits in the orbit's left leg at (38, 700). A guide wall below
the upper deck's edge drops balls onto `bumper_2`, a short wall closes the
alcove right of the ramp lane, walls are 50 mm tall and both trough zones
have radius 36 mm.

Only the sources live here; the generated `playfield.glb`, the merged
`table.toml` and the placeholder art and sounds are build output and are not
committed.

| File | What |
| --- | --- |
| `layout.toml` | `pin_tablegen` layout: walls, orbit lane, ramp, upper deck |
| `glass.toml` | playfield glass underside, 112 mm (22 mm over the 90 mm walls; DECISIONS "a1-glass-height — grey-box walls to the glass") |
| `table.toml` | base manifest: switches, coils, flippers, mechs, magnet, ramp spline, layer transitions, dispenser, shots, modes, sounds, scenes |
| `rules.lua` | rules (integer only): attract → 3-ball game → game over |
| `backglass.toml` | placeholder translite and a 128 × 32 orange DMD |
| `display/*.toml` | display scenes (copies of the 5.3 placeholders) |
| `build.sh` | the one-command build |

## Build

From the repository root:

```sh
tables/greybox/build.sh target/greybox-bundle
```

It runs `pin_tablegen` (layout → `playfield.glb` + merged `table.toml`),
copies the rules, backglass, layout, glass and scenes, runs `pin_assetgen` (the 4.6
placeholder kit: sounds, fonts, backglass art; title "Grey Box", the table's
palette, seed 0) and finally `pin_tablegen validate` (ball clearances). The
tests build the same bundle with `build_greybox_bundle(out_dir)` in
`crates/pin_app/tests/support/`.

## Run

```sh
cargo run -p pin_app -- --table target/greybox-bundle            # windows
cargo run -p pin_app -- --headless-ticks 10000 --table target/greybox-bundle
```

Keys (generic keyboard profile): Left/Right Shift flippers, Left Ctrl upper
flipper, 1 start, Space or Return launch, T tilt.

## Rules

* START begins a 3-ball game; each ball is served into the shooter lane and
  launched with the launch button (the plunger's auto-plunger coil).
* A ball ends when it drains into `trough_1` / `trough_2`; after the third
  the game is over and the attract scene returns 4 s later.
* Ball 2 is the Powerball. When it crosses the magnet without closing the
  eddy sensor, the `powerball` mode starts (scene
  `scene_powerball_mania`) and scores double until the ball drains.
* Points: bumper 100, sling 10, orbit entry 500, orbit top 5000, ramp entry
  100, ramp made 10000, VUK 2500, upper deck 1500, eddy 250, Newton ball
  2000, drop target 750.
* Game state is published in the `[persist]` keys `ball`, `game_state`,
  `powerball_modes`, `games_played` and `high_score`.
