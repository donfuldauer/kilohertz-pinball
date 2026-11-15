# Autoplay soak fixtures (Subphase 2.9)

Tiny hand-authored bundles for the `autoplay_*` tests. Each folder is a
directory bundle: `layout.toml` (the generator input, also read by the
simulation for the wall height and by the soak for wall/post names),
`table.toml` (base table plus the entries `pin_tablegen` generates),
`playfield.glb` (generated), `rules.lua`, `backglass.toml` and, where the
table has modes, placeholder `scenes/*.toml`. Regenerate the glb and the
generated part of `table.toml` inside a folder with

    pin_tablegen layout.toml --table table.toml --out . --force

All of them share one playfield: 514 x 1070 mm at 6.5 deg, two flippers whose
inlane walls meet the flipper bases, a centre drain into a closed V basin
with the `trough` switch, and a serve from dispenser `trough_exit` at the
top. The rules are a three-ball game started by `start`; a mode start
increments the mode's `mode:<id>` persist key (how the soak observes it).

| bundle | planted defect | expected finding (subject) |
|---|---|---|
| `clean` | none; bumper, `top_rollover`, shot `drop_lane`, modes `warmup`, `frenzy` | none |
| `trap` | a V-bottomed cup (`walls[2]`) under the serve with `trap_pocket` at its bottom | StuckBall `trap_pocket @ ...` |
| `gap` | the left wall is open from y = 750 to 1000 mm (`walls[0]` ends at 750) and the serve rolls left | EscapedBall `walls[0] @ ...` |
| `unreachable` | `hidden_opto` inside a closed diamond of walls (`walls[2]`) | UnreachableSwitch `hidden_opto` |
| `script_error` | mode `frenzy` raises a Lua error 500 ms after it starts; mode `wizard` is never started | ScriptError `frenzy`, ModeCoverage `wizard` |
| `overflow` | the `gap` playfield (same glb) whose rules send six `set_score` commands in one callback (the list holds four) | QueueOverflow `script command queue` (and the gap's EscapedBall) |
