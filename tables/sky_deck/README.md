# Sky Deck (Subphase 6.2, variant B)

A two-level table on a standard-body playfield (514 × 1070 mm):

* **Main deck** (layer 0, z 0): two 76 mm lower flippers, two slingshots,
  inlanes and 30 mm outlanes (a guard over the left one only), a drain
  funnel into a two-switch trough, a
  shooter lane with plunger and auto-plunger, a centre VUK in a V pocket,
  the captive Newton ball ("Reactor Core" guard) and an upper-left
  wrap-around loop.
* **Upper deck** (layer 1, z 65 mm, clear acrylic) at the upper right: a
  52 mm `upper_right` flipper, an elevated 3-bank of drop targets, deck
  rails, and the acrylic return funnel (the gap beside the flipper's tip
  and the transition over the deck's lower edge) that puts every deck ball
  back on the main deck.
* **Left ramp** (wireform, layer 2) up to a landing at the deck height with
  the **Sky Bridge Diverter**: open, its blade turns the ball onto the Sky
  Bridge and the upper deck; closed, the ball stops at the landing's top
  rail and rolls back down the ramp to the main deck. Every switch of the
  ramp (`ramp_entry`, `ramp_made`) is before the fork.

Design notes and the choices behind them: `docs/decisions/2026-11-18-6.2.md`.

Only the sources live here; the generated `playfield.glb`, the merged
`table.toml` and the placeholder art and sounds are build output and are not
committed.

| File | What |
| --- | --- |
| `layout.toml` | `pin_tablegen` layout: walls, loop lane, ramp, landing, upper deck |
| `glass.toml` | playfield glass underside, 122 mm (22 mm over the 100 mm walls) |
| `table.toml` | base manifest: switches, coils, lamps, flippers, mechs, ramp and rail splines, layer transitions, dispensers, shots, mode, sounds, scenes, persist keys |
| `rules.lua` | rules (integer only): attract → 3-ball game → game over, Sky Bridge, Night Overdrive |
| `backglass.toml` | placeholder translite and a 128 × 32 orange DMD |
| `display/*.toml` | display scenes (5.3 placeholder format) |
| `build.sh` | the one-command build |

## Build

From the repository root:

```sh
tables/sky_deck/build.sh target/sky_deck-bundle
```

It runs `pin_tablegen` (layout → `playfield.glb` + merged `table.toml`),
copies `rules.lua`, `backglass.toml`, `layout.toml`, `glass.toml` and every
file of `display/` unchanged into the bundle, runs `pin_assetgen` (the 4.6
placeholder kit: sounds, fonts, backglass art; title "Sky Deck", the
table's palette, seed 0) and finally `pin_tablegen validate` (ball
clearances for `standard_steel` and `uv_globall`). The tests build the same
bundle with `build_sky_deck_bundle(out_dir)` in
`crates/pin_app/tests/sky_deck_support/`.

## Run

```sh
cargo run -p pin_app -- --table target/sky_deck-bundle            # windows
cargo run -p pin_app -- --headless-ticks 10000 --table target/sky_deck-bundle
cargo run --release -p pin_sim --bin pin_sim -- soak target/sky_deck-bundle \
  --games 200 --seed 1 --skill standard --threads 2 --gate a1 --out target/sky_deck-soak
```

Keys (generic keyboard profile): Left/Right Shift lower flippers, Right
Ctrl the upper-deck flipper (`right_upper`), 1 start, Space or Return
launch, A the action button (Sky Bridge / Night Overdrive), T tilt.

## Rules

* START begins a 3-ball game; each ball is a steel ball served into the
  shooter lane and launched with the launch button (the auto-plunger).
* A ball of the game ends when every ball on the table has drained into
  `trough_1` / `trough_2`; after the third the game is over and the attract
  scene returns 4 s later. The high score is kept in `high_score`.
* **Sky Bridge:** in normal play `action` opens the Sky Bridge Diverter for
  exactly 8.0 s (`bridge_open` = 1); a press while it is open is ignored.
  Open, a ball that makes the left ramp is carried over the bridge onto the
  upper deck.
* **Night Overdrive** ("Reactor Core" completion): strike the Newton ball
  and knock down all three drop targets on the upper deck in one ball to
  arm it (`overdrive_ready` = 1). It starts with the next `action` press or
  the next ball into the centre VUK: scene `scene_night_overdrive`, a UV GI
  tint and a UV Glo-Ball (`uv_globall`) that the VUK lifts onto the upper
  deck within about 2 s (the VUK normally holds a ball 4 s; until the
  Glo-Ball is up it holds only 0.5 s) while the steel ball plays below.
  Upper-deck scoring doubles. It ends at the first drain of the two balls
  or at the end of the ball (GI back to white); once per ball. The Glo-Ball is a bonus ball: it never
  changes the ball count.
* Points: sling 1000, loop entry 1000, loop top 1000 plus a 50000 loop shot
  bonus when it follows `loop_entry` within 3 s, ramp entry 1000, ramp made
  25000 (once per pass), VUK 100, upper deck 100, Newton ball 100, drop
  target 100, drop bank complete 1000 (upper-deck points doubled in Night
  Overdrive). A tilt voids scoring for the rest of the ball. Shot windows
  and the ramp debounce do not carry across balls.

## Names (fixed; tests code against them)

| Kind | Names |
| --- | --- |
| table id | `sky_deck` |
| flippers | `lower_left`, `lower_right`, `upper_right` (layer 1) |
| mechs | `vuk`, `newton`, `drop_1`, `drop_2`, `drop_3`, `bridge_diverter`, `sling_left`, `sling_right`, `plunger` (and the blade `bridge_blade`) |
| switches | `trough_1`, `trough_2`, `vuk_sw`, `newton_sw`, `drop_1_sw`, `drop_2_sw`, `drop_3_sw`, `deck_opto` (layer 1), `loop_entry`, `loop_top`, `ramp_entry`, `ramp_made`, `shooter_lane`, `sling_left_sw`, `sling_right_sw`, `lower_left_eos`, `lower_right_eos`, `upper_right_eos`, `plunger_tip` |
| dispensers | `feed` (steel), `glo_feed` (Glo-Balls) |
| scenes | `scene_attract`, `scene_score`, `scene_game_over`, `scene_night_overdrive` |
| shots | `loop` (`loop_entry` → `loop_top`), `ramp` (`ramp_entry` → `ramp_made`), `deck` (`vuk_sw` → `deck_opto`) |
| mode | `night_overdrive` |
| persist keys | `games_played`, `high_score`, `ball`, `game_state`, `mode:night_overdrive`, `bridge_open`, `overdrive_ready` (in this order) |
