# Threat model

## Scope

Table bundles (`.pinbundle` zips and folder bundles) are untrusted downloads (PLAN.md D25, §3.5).
Every byte of bundle content is treated as hostile.

Existing defences:

- **Lua sandbox.** Scripts load only `base` (without `dofile`, `loadfile`, `load`), `string`,
  `table`, `math` and `utf8`. `io`, `os`, `debug` and `package` are not loaded. (PLAN.md §3.4,
  "Sandbox (D25)"; `crates/pin_script/src/lua/sandbox.rs`.)
- **Instruction budget and memory cap.** A per-callback instruction budget and a memory cap abort
  runaway scripts. An aborted script drops into a safe "tilt" state. (PLAN.md §3.4.)
- **Hardened zip extraction.** Absolute paths, `..` components and symlinks are rejected. Caps apply
  to total uncompressed size and to compression ratio. (PLAN.md §3.5; `crates/pin_assets/src/zip/mod.rs`.)
- **Folder-bundle VFS containment.** The VFS refuses any path whose canonical form leaves the
  canonical bundle root, and any path that goes through a symlinked folder. (`crates/pin_assets/src/vfs/mod.rs`,
  module doc and `resolve_dir`.)

## Out of scope

- A hostile local filesystem: another local process with write access to the bundle folder while
  the bundle is loaded.

## Accepted risks

### TM-1 — VFS containment check is not bound to the opened file (TOCTOU)

The VFS validates that a path stays inside the bundle (canonicalize, then compare to the root). It
then opens the file by path. A symlink swapped in between the check and the open could therefore
escape the bundle.

Exploiting this needs a concurrent local attacker with write access to the bundle folder. That
attacker is outside the scope above.

- **Status:** accepted.
- **Revisit trigger:** bundles are ever loaded from a shared or downloaded location that another
  process can write to.
- **Fix then:** handle-relative opens (`cap-std`, or `openat2` with `RESOLVE_BENEATH` on Linux),
  which bind validation to the opened file.
- **Source:** GitHub issue #13; review thread https://github.com/donfuldauer/kilohertz-pinball/pull/4#discussion_r4573613058.

New accepted risks are added to this file as `TM-<n>` entries, each with a revisit trigger.
