# Apex Circuit (Subphase 6.1, variant A)

A single-player table built on the grey box's Gate A1 geometry
(`tables/greybox`). It has:

* two 76 mm lower flippers, `left_flipper` and `right_flipper`, with Zipper
  mode ("Firewall Save");
* a 55 mm upper-left flipper, `left_upper`, at 10 o'clock, fed by the left
  orbit's lower end;
* two slingshots and a 3-bumper cluster;
* a left orbit with a spinner (`left_spinner`) and `orbit_top`;
* a right orbit corridor (`right_orbit_entry`) that merges into the
  shooter-lane top through the magnet `apex_magnet` and the Powerball
  detector (`magnet_eddy` + `magnet_opto`);
* an upper-right cross-shot jackpot ramp (wireform, layer 0 → 2 → upper deck
  on layer 1, `ramp_entry` / `ramp_made`);
* a 3-bank of drop targets (`drop_1..3`, one reset coil);
* a centre lock scoop (VUK `lock_scoop`, approach opto `lock_lane`) that
  ejects back into play;
* three skill-shot top lanes (`lane_1..3`) and two inlane rollovers;
* a dispenser `feed` (four steel balls, then the ceramic Powerball), the
  plunger, a two-switch trough and the tilt switch.

Design choices are logged in `docs/decisions/2026-11-18-6.1.md`.

Only the sources live here. The generated `playfield.glb`, the merged
`table.toml` and the placeholder art and sounds are build output and are not
committed.

| File | What |
| --- | --- |
| `layout.toml` | `pin_tablegen` layout: walls, orbit lane, right-orbit corridor, top lanes, ramp, upper deck |
| `glass.toml` | playfield glass underside, 112 mm (22 mm over the 90 mm walls, as the grey box) |
| `table.toml` | base manifest: switches, coils, lamps, flippers, mechs, magnet, ramp spline, layer transitions, dispenser, shots, modes, sounds, scenes, persist keys |
| `rules.lua` | rules (integer only) |
| `backglass.toml` | placeholder translite and a 128 × 32 orange DMD |
| `display/*.toml` | display scenes (5.3 placeholder style) |
| `build.sh` | the one-command build |

## Build

From the repository root:

```sh
tables/apex_circuit/build.sh target/apex-bundle
```

This works like `tables/greybox/build.sh`. It runs `pin_tablegen`
(layout → `playfield.glb` + merged `table.toml`) and copies the rules,
backglass, layout, glass and scenes. Then it runs `pin_assetgen` for the
placeholder kit (title "Apex Circuit", the `[meta]` palette, seed 0).
Last, `pin_tablegen validate` checks the ball clearances. The tests build
the same bundle with `build_apex_bundle(out_dir)` in
`crates/pin_app/tests/apex_support/`.

## Run

```sh
cargo run -p pin_app -- --table target/apex-bundle
cargo run -p pin_app -- --headless-ticks 10000 --table target/apex-bundle
```

Soak (Gate A2 style):

```sh
cargo run --release -p pin_sim --bin pin_sim -- soak target/apex-bundle \
  --games 60 --seed 1 --skill standard --threads 1 --gate a1 --out target/apex-soak
```

## Rules

All points are integers. "Multiplied" means times the playfield
multiplier: the combo level (1..5), times 2 during Powerball Mania, capped
at 10.

* **Game.** START begins a 3-ball game from attract (or game over). Each ball
  is served from `feed` into the shooter lane and launched with LAUNCH (the
  auto-plunger coil). Ball 2 is the ceramic Powerball; balls 1 and 3 are
  steel. A ball ends when it drains into `trough_1` / `trough_2`. After the
  bonus (2 s) the next ball is served. After ball 3 the game is over.
* **High score.** If the final score beats `high_score`, it is stored and
  the `scene_high_score` entry scene shows for 3 s. There is no initials
  input. Then `scene_game_over` shows, and attract returns 4 s later.
* **Skill shot.** A plunge (the ball runs up the shooter lane past the
  `shooter_lane` opto) opens a 6 s window. Lit lane = `lane_((ball − 1) mod 3) + 1`, so it rotates per
  ball. The first top-lane rollover in the window scores 10,000 × ball
  number on the lit lane (`skill_shot_count` + 1, `scene_skill_shot`), or
  1,000 on another lane. These awards are fixed. Outside the window a lane
  scores 1,000, multiplied.
* **Combo (`combo_chain` mode).** A left-orbit pass (a spinner pulse) feeds
  the upper-left flipper. A shot switch within 4 s of the feed or of the
  previous combo hit steps the combo level 1 → 2 → 3 → 4 → 5, so the
  multiplier goes 2x..5x. The shot switches are `orbit_top`,
  `right_orbit_entry`, `ramp_made`, `lock_scoop_sw` and the drop targets.
  After 12 s without a combo hit the level drops one step; a drain resets
  it. The first time a ball reaches 2x, `combo_chain` starts:
  `mode:combo_chain` + 1 and `scene_combo_chain`.
* **Powerball Mania (`powerball_mania` mode).** The magnet opto closes with
  no eddy-sensor edge within 30 ms: the ball is the non-conductive
  Powerball. Then `mode:powerball_mania` + 1, `scene_powerball_mania` shows
  and scoring doubles until that ball drains. A steel ball closes both
  sensors and never starts it. The detector is in the shooter-lane top, so
  every plunge of ball 2 passes it. The rules pulse the magnet for 40 ms
  when the eddy sensor closes.
* **Overdrive and the action button.** 3 ramps (`ramp_made`) or 5 combo
  steps on a ball light Overdrive Ready (`overdrive_lamp`,
  `overdrive_ready` = 1, `scene_overdrive`). Overdrive is lost at the drain.
  Without Overdrive Ready, ACTION does nothing. With it, ACTION uses it up
  and does one of two things. **Selection rule:**
  * **Firewall Save** when `balls_in_play() == 1` and an inlane rollover
    (`left_inlane` / `right_inlane`) closed within the last 3 s (the ball is
    heading for the flippers). Both lower flippers zip closed (`set_zipper`)
    for exactly 5,000 ticks (5.0 s), then reopen. `firewall_save_count`
    + 1, `scene_firewall_save`.
  * **Apex Jackpot** otherwise. It scores 50,000 − 1,000 × (whole 100 ms
    since Overdrive was lit), at least 5,000. This award is fixed.
    `apex_jackpot_count` + 1, `scene_apex_jackpot`.
* **Bonus.** At the drain the ball's counters are summed: ramps made, left
  orbits (`orbit_top`), right orbits, drop targets, scoops and top lanes.
  The bonus is that sum × 1,000 × the combo level, shown with `scene_bonus`.
  The value is published in `last_bonus`.
* **Points (multiplied).** Bumper 100, sling 10, spinner pulse 50, orbit top
  2,000, right-orbit entry 1,000, magnet opto 2,500, ramp entry 500, ramp
  made 10,000, drop target 750, completed bank 10,000 (the bank resets 1 s
  later and at every new ball), lock lane 100, lock scoop 5,000 (held 1 s,
  then ejected), inlane 100.
* **Persist keys.** `ball`, `game_state` (0 attract, 1 game, 2 game over),
  `high_score`, `games_played`, `firewall_save_count`, `apex_jackpot_count`,
  `skill_shot_count`, `mode:powerball_mania`, `mode:combo_chain`,
  `combo_level`, `overdrive_ready` and `last_bonus`.

Keys (generic keyboard profile): Left/Right Shift flippers, Left Ctrl upper
flipper, 1 start, Space or Return launch, T tilt, plus the profile's action
key.
