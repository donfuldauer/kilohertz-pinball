# Decisions: w8h-cr-status

## w8h-cr-status — Final statuses of CR-005 to CR-009

**Decision:** The five requests are recorded as `merged (<commit>)` with the commits named in `contracts/CHANGE_REQUESTS.md`: CR-005 `64ec5f9`; CR-006 `1594b87` (via merge `5afae2e`, PR #24); CR-007 `d21ba7d` (via merge `d639eb1`, PR #26); CR-008 `8f4c545` and `d6c3aff` (via merge `f845d5f`, PR #69); CR-009 `8050c3b` (via merge `2f2c3c6`, PR #74). Interpretations:
(a) CR-005: `64ec5f9` (merge of PR #4, the Wave 0-5.2 stack) has no parent, so it is a parentless commit in main's graph. `git log -S "pub area: Vec<[f32; 2]>"` over main finds no earlier commit that introduces `BallRestriction::area`; `64ec5f9` is the earliest commit that contains the change, and no separate pin_core-only commit exists (the change landed in the working tree with Subphase 2.8). The earlier wording "history before 64ec5f9 is one import" is not what git shows: main has several parentless roots (for example `578c518`, the PR #1 license merge), and `64ec5f9` is not a descendant of them.
(b) CR-006 and CR-007: the commit named first is the work commit; the merge named after `via` is the one that brought it into main. Both are confirmed ancestors of `HEAD`.
(c) CR-007 names the reworked `GlassManifest`/`glass.toml` version (`crates/pin_core/src/schema/glass.rs`, `GLASS_FILE`), `d21ba7d`. The first version `3062450` (`Playfield.glass_height`) is an ancestor of `0f1b0af`, the second parent of `d639eb1`, so it did reach main. `d21ba7d` removes the lines it added (`layout.rs`, `layout_glass_height.rs`); it is a rework commit, not a `git revert`. That version was withdrawn as breaking.
(d) CR-008 and CR-009 were merged by their own pin_core-only PRs (#69 and #74), as the protocol requires; their status lines had simply not been updated.
**Rationale:** Protocol step 5 requires `merged (<commit>)`. A status that stays `open` after the change is in main misleads the next agent into re-requesting it.
**Alternatives:** Leaving `applied` or `open` (violates the protocol). Rewriting history to isolate CR-005 (impossible, the import is one merge commit).
