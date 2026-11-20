# Kilohertz Pinball patches to sdl3-sys 0.7.3+SDL-3.4.20

Vendored from crates.io `sdl3-sys =0.7.3` (`0.7.3+SDL-3.4.20`) and wired in with
`[patch.crates-io]` in the workspace `Cargo.toml`. The package version is
unchanged, so the `=0.7.3` pin and the dependents (`sdl3`, `sdl3-image-sys`,
`sdl3-mixer-sys`, `sdl3-ttf-sys`) resolve to this copy. Every edit is marked
`KHZ` in the source. Rationale: docs/decisions/2026-11-20-fix-sdl3-space-path.md
(precedent: docs/DECISIONS.md, "Orchestrator — 4.2 review: patched vendored Lua").

| # | File | Patch |
|---|------|-------|
| 1 | `build-common.rs` | Build-from-source branch: after `PkgConfig::open` of the generated `.pc` (binding is now `mut cfg`) and before `libs_with_private`, re-set `prefix`, `exec_prefix`, `libdir` and `includedir` with `set_variable_escaped(var, raw, true)` (backslash before space, `"` and `\`). Only literal values are re-set (`${prefix}/...` values inherit the escape), and only when the raw value has no backslash and no quote (double-escape guard). With SDL 3.4.20's `.pc` this escapes `prefix` only. The earlier patch that always emitted `rustc-link-search` for `OUT_DIR/lib` and `lib64` was removed (see Root cause). The `Link` match and the no-`.pc` fallback branch are unchanged. |
| — | (package) | `.cargo-checksum.json`, `.cargo_vcs_info.json`, `Cargo.lock` and `Cargo.toml.orig` are not copied. `examples/` is kept because `Cargo.toml` lists both examples. |

## Root cause

With `build-from-source`, `build-common.rs` builds SDL with CMake into `OUT_DIR`
and then reads `OUT_DIR/lib/pkgconfig/sdl3.pc` with `rpkg_config` 0.1.2. SDL's
CMake writes `prefix=<OUT_DIR>` into the `.pc` file without escaping spaces;
`exec_prefix`, `libdir` and `includedir` are `${prefix}/...`. `rpkg_config`
expands `${...}` textually and then splits `Libs: -L${libdir} -Wl,-rpath,${libdir} -lSDL3`
on unescaped whitespace (`UnescapeAndSplit` honours `\ `, `\"`, `\\` and quotes).
When the absolute `OUT_DIR` contains a space (a checkout under
`~/Software Projects/...` with `CARGO_TARGET_DIR` unset) two things go wrong:

- the search path is truncated at the first space
  (`cargo::rustc-link-search=native=/Users/<user>/Software`), and linking fails
  with `could not find native static library SDL3`;
- if the text after the space starts with `-l` (e.g. `/tmp/SDL -local/project`),
  the fragment `-local/project/.../lib` is parsed by `Links::next` as `-l` +
  `ocal/project/.../lib`, a `Link::Lib`. The `else` arm of the `Link::Lib` match
  emits it as `cargo::rustc-link-lib=ocal/...` (once from `-L${libdir}` and once
  from `-Wl,-rpath,${libdir}`), and the link fails even if the correct search
  path is present.

The fix escapes the install-path variables before tokenization, so every flag
gets the whole path. The fallback branch that adds `OUT_DIR/lib` runs only when
no `.pc` file exists and is unchanged.

**Double-escape guard.** A variable is re-set only if its raw value contains no
backslash and no `"`. If a future SDL/CMake release escapes or quotes its own
paths, the `.pc` is used as written. (A path that really contains a backslash or
quote is also left as written; such paths are not supported by the upstream
script either.)

**Search-path loop removed.** The first version of this patch (PR #105 before
CodeRabbit round 1) always emitted `cargo::rustc-link-search=native=` for
`OUT_DIR/lib` and `lib64`. That fixed the truncated search path but not the
bogus `-l` entry. With the escaping in place, the `.pc` gives the full search
path itself, and a release build of `pin_app` from
`/private/tmp/claude-501/khz -local/kh` (which reproduces the `-l` failure with
the old script) links with the loop removed. The loop only duplicated a search
path, so it was dropped to keep the patch to one mechanism.

## Scope

`build-common.rs` has one `.pc` parse (`PkgConfig::open` followed by one
`libs_with_private` call), in the build-from-source branch; it is the only path
through which this workspace builds SDL (`sdl3` feature `build-from-source-static`).
The non-build-from-source branch uses the `pkg-config` crate against system SDL
(`use-pkg-config` feature, not enabled here) and is not patched.

`sdl3-image-sys`, `sdl3-mixer-sys` and `sdl3-ttf-sys` ship their own copy of the
same `build-common.rs` with the same `.pc` parse. They are not built by this
workspace (the `sdl3` features `image`, `mixer`, `ttf` are off) and are not vendored.

## Removal

Drop this directory and the `sdl3-sys` line in `[patch.crates-io]` once an
upstream sdl3-sys release (rust-sdl3) handles spaces in the CMake install path,
and the workspace pin moves to it.
