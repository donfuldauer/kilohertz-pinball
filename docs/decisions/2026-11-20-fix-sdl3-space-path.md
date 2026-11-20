# Decisions: fix-sdl3-space-path (SDL3 build from checkout paths with spaces)

## fix-sdl3-space-path — vendor patched sdl3-sys for spaced checkout paths

**Decision:** Vendor `sdl3-sys 0.7.3+SDL-3.4.20` into `third_party/sdl3-sys`, wired with
`[patch.crates-io]`, with one `// KHZ:` patch in `build-common.rs`: in the build-from-source
branch, always emit `cargo::rustc-link-search=native=` for the CMake install dir's `lib` and
`lib64` (each only if it exists) before the `.pc` file is parsed. The `.pc` parsing and the
fallback branch are unchanged. Patch list: `third_party/sdl3-sys/KHZ_PATCHES.md`.

**Rationale:** The root cause is in the upstream build script: SDL's CMake writes the generated
`sdl3.pc` with an unescaped `prefix=`, and `rpkg_config` splits `Libs: -L${libdir}` on
whitespace, so from a checkout path with a space the emitted search path is truncated at the
first space and `cargo build --release -p pin_app` fails with "could not find native static
library SDL3". The workaround of a space-free `CARGO_TARGET_DIR` is easy to forget and is not
set for local release builds (or `scripts/package.py`). The repository already vendors patched
crates through the same mechanism (`third_party/lua-src`, `third_party/lewton`); precedent:
docs/DECISIONS.md "Orchestrator — 4.2 review: patched vendored Lua". On paths without spaces
(CI) the patch only repeats a search path the `.pc` file already gives.

**Alternatives:** (a) Rewrite the `.pc` quoting before parsing: more intrusive, and fragile
against changes in SDL's CMake output. (b) Document the `CARGO_TARGET_DIR` workaround only:
leaves release builds and `scripts/package.py` broken by default. (c) Report upstream only:
cannot ship now (queued as a follow-up in `docs/queue/fix-sdl3-space-path.md`).
