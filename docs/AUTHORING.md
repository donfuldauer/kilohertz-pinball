# Table authoring rules

## `layout.toml` (input to `pin_tablegen`, PLAN.md Appendix A.3)

Run `pin_tablegen <layout.toml> --table <table.toml> --out <bundle_dir>`. The
base `table.toml` holds switches, coils, lamps, rules and hand-made splines;
the tool writes `<bundle_dir>/playfield.glb` and the merged
`<bundle_dir>/table.toml`. Same inputs give byte-identical outputs. If the
layout cannot be generated validly the tool exits with code 1 and a message
naming the element (`walls[2]: ...`) and writes nothing; bad arguments exit
with code 2.

### Units and ranges
- Layout lengths are mm in the playfield plane (`x` across, `y` up the table).
- Every coordinate lies within ±5000 mm. Playfield width/length are 0.1–5000 mm.
- Sizes (wall height, deck `z`, post radius, lane width, insert size, ramp
  `entry_width`) are 0.1–1000 mm.
- At most 1024 points per wall, lane or deck outline; no point may repeat
  the previous one.

### Elements
- **Walls**: polylines extruded from the floor to `wall_height` (zero
  thickness, both sides collide). `closed = true` adds the closing segment
  (repeating the first point last is allowed); a closed wall must be a
  simple polygon (no crossing or touching edges).
- **Posts**: cylinders to `wall_height`; `rubber = true` gives rubber, else steel.
- **Lanes**: two guide rails `width` apart around a smooth (natural cubic)
  curve through the centre points. A lane whose last point repeats the first
  (within 0.001 mm, at least 4 points) is closed; ends closer than 1 mm
  otherwise are refused. The centre line may not bend tighter than about
  0.63 × the width (1.25 × the half-width) and may not turn back or touch
  itself. Points should be roughly evenly spaced (the curve is uniformly
  parametrised).
- **Ramps**: name a `[[splines]]` entry of the base table. A ramp on a layer
  other than 0 gets a layer transition (0 → its layer) `entry_width` wide
  around its first through-point.
- **Decks**: `layer` 1–7, `z` above the floor, a simple outline (either
  orientation).
- **Inserts**: `lamp` must be a `[[lamps]]` id of the base table. Circle:
  diameter `size`; triangle: side `size`, pointing up the table; rect:
  `size` × `size/2`; arrow: `size` long, `0.6·size` wide, pointing up the table.

### Generated output
- `playfield.glb`: metres, `z` up (table frame), one node per element:
  `floor`, `decks[i]`, `walls[i]`, `posts[i]` (colliders: extras
  `collider: true`, `collision_layer`, `material`), `tablegen:lanes[i].left`
  / `.right` (visual; extras `guide` = the spline id), `ramps[i]` (visual;
  extras `spline`), `inserts[i]` (extras `lamp`, 0.1 mm above the floor).
  Materials are flat-colour PBR named after the physics material
  (`wood`, `rubber`, `steel`, `plastic`, `acrylic`, ...) or `insert #rrggbb`.
- `table.toml`: generated ids start with `tablegen:` and are replaced on
  every run (running on the generated `table.toml` again gives the same
  output); `meta.playfield_mesh` is set to `playfield.glb`.

## `table.toml` `[[splines]]` (loader rules)
- At most **4096 through-points per spline**; split longer rails.
- Every through-point within **10 m** of the origin, and a spline at most
  **65 m** long (errors).
- Keep the through-point spacing below the smallest radius of curvature
  and roughly even (ratio ≤ 3), or the loader warns.
- Guides whose ends are within 1 µm are snapped closed; a closed guide needs
  4 points (3 distinct). Sloped guides need a plan view with no bend under
  ~22 mm (~30 mm for a plain arc) and no stretch within ~44 mm of another;
  helical ramps must be wireforms.
