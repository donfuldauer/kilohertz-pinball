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

Note (CodeRabbit r1): the search-path patch described above was replaced; see the next section.

## fix-sdl3-space-path — escape path variables before .pc tokenization (CodeRabbit r1)

**Decision:** In `third_party/sdl3-sys/build-common.rs` (build-from-source branch), open the
generated `.pc` as `mut cfg` and, before `libs_with_private`, re-set `prefix`, `exec_prefix`,
`libdir` and `includedir` with `rpkg_config`'s `set_variable_escaped(var, raw, true)`, using the
raw value read with `cfg.variable(var)`. A variable is re-set only when its value is literal (no
`${`) and contains no backslash and no `"` (double-escape guard). With SDL 3.4.20's `.pc` only
`prefix` is literal; the other three are `${prefix}/...` and inherit the escape. The earlier
unconditional `rustc-link-search` loop over `OUT_DIR/lib` and `lib64` is removed. All edits are
marked `KHZ`; `third_party/sdl3-sys/KHZ_PATCHES.md` is updated.

**Rationale:** CodeRabbit (PR #105, thread r4645815514) pointed out that the search-path loop
does not cover a checkout path with a space followed by `-l`. I checked the diagnosis against the
rpkg-config 0.1.2 source: `${...}` expansion is textual, `UnescapeAndSplit` splits on unescaped
whitespace and honours `\ `, and `Links::next` maps any token starting with `-l` to
`Link::Lib`. The build script's `else` arm then emits it as `rustc-link-lib`. Reproduced from
`/private/tmp/claude-501/khz -local/kh`: the old script emitted
`cargo::rustc-link-lib=ocal/kh/target/release/build/sdl3-sys-.../out/lib` twice (from
`-L${libdir}` and `-Wl,-rpath,${libdir}`) and the `pin_app` link failed. After the fix the same
build emits one full-path `rustc-link-search=native=.../khz -local/kh/.../out/lib`, no bogus
`-l`, and links (exit 0). The root fix is escaping `prefix`. The guard means a future upstream
`.pc` that escapes or quotes its own paths is used unchanged. The search-path loop was removed
because the `-local` build links without it, and it only duplicated the search path the `.pc` now
gives correctly. Keeping it would leave two mechanisms for one bug.

**Alternatives:** (a) Rewrite the `.pc` text (quote or escape `prefix=`) before parsing: needs a
temp file or a `from_bytes` re-parse that loses `pcfiledir`, and depends on the `.pc` text layout.
`set_variable_escaped` is the API rpkg-config provides for this, and `PkgConfig::open` already uses
it for `pcfiledir`. (b) Keep only the search-path loop (the previous patch): it does not prevent
the bogus `-l` library, so the link still fails. (c) Keep the loop as well as the escaping: harmless,
but redundant once the `.pc` parses correctly. (d) Filter `Link::Lib` entries that contain `/`:
treats the symptom, and the search path would still be truncated.
