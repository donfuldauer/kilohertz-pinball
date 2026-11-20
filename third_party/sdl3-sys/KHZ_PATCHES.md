# Kilohertz Pinball patches to sdl3-sys 0.7.3+SDL-3.4.20

Vendored from crates.io `sdl3-sys =0.7.3` (`0.7.3+SDL-3.4.20`) and wired in with
`[patch.crates-io]` in the workspace `Cargo.toml`. The package version is
unchanged, so the `=0.7.3` pin and the dependents (`sdl3`, `sdl3-image-sys`,
`sdl3-mixer-sys`, `sdl3-ttf-sys`) resolve to this copy. Every edit is marked
`KHZ` in the source. Rationale: docs/decisions/2026-11-20-fix-sdl3-space-path.md
(precedent: docs/DECISIONS.md, "Orchestrator — 4.2 review: patched vendored Lua").

| # | File | Patch |
|---|------|-------|
| 1 | `build-common.rs` | Build-from-source branch: right after `build_config.build()`, emit `cargo::rustc-link-search=native=<OUT_DIR>/lib` and `<OUT_DIR>/lib64` (each only if the directory exists), before the `.pc` file is parsed. The `.pc` parsing and the no-`.pc` fallback branch are unchanged. |
| — | (package) | `.cargo-checksum.json`, `.cargo_vcs_info.json`, `Cargo.lock` and `Cargo.toml.orig` are not copied. `examples/` is kept because `Cargo.toml` lists both examples. |

## Root cause

With `build-from-source`, `build-common.rs` builds SDL with CMake into `OUT_DIR`
and then reads `OUT_DIR/lib/pkgconfig/sdl3.pc` with `rpkg_config`. SDL's CMake
writes `prefix=` into the `.pc` file without escaping spaces, and the
`Libs: -L${libdir} ...` line is split on whitespace. When the absolute `OUT_DIR`
contains a space (a checkout under `~/Software Projects/...` with
`CARGO_TARGET_DIR` unset), the emitted search path is truncated at the first
space (`cargo::rustc-link-search=native=/Users/<user>/Software`), and linking
fails with `could not find native static library SDL3`. The fallback branch
that adds `OUT_DIR/lib` runs only when no `.pc` file exists, so it never helps.
Cargo's `rustc-link-search` directive is line-based, so the full path printed
with `Path::display()` works with spaces. On paths without spaces (CI) the patch
only adds the same search path the `.pc` file already gives.

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
