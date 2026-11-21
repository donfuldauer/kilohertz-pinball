# S11 (web-spike-mac-host) — local decisions (PLAN 2.3 rule 8)

## S11-D1 — x86_64 shell under Rosetta 2 on Apple Silicon

**Decision.** `khz_platform` returns `darwin-arm64` when `uname -m` is `x86_64` and
`sysctl -n sysctl.proc_translated` prints `1`, and prints a note; it does not refuse.

**Rationale.** The arm64 tools run natively from a translated shell (the kernel picks the slice
per executable), all arm64 Mach-O files are signed (linker-signed ad hoc, RESULTS §2.3), and the
module does not depend on the build host's architecture except through the residual codegen risk
the macOS CI job measures (§4.5). Refusing would force users of an x86_64 Terminal/Homebrew
setup to re-install their shell for no benefit.

**Alternatives.** Refuse with a message (brief allows it); use the x86_64 tools under Rosetta
(slower; Intel pins then tested only under emulation, which the CI design avoids).

## S11-D2 — wasm-bindgen-cli and wasm-tools: from source on every host

**Decision.** Keep `cargo install --locked <crate> --version <pin>` on all three hosts (now done by
`fetch_tools.sh` into `target/web-tools/cargo`); the release binaries are recorded in tools.lock as
a non-default alternative with their sha256.

**Rationale.** Same mechanism as Linux today (S1-S10 built them this way), cargo verifies the
crate checksum from the index, the linker signs arm64 builds automatically, and it needs nothing
the Mac host does not already need (Xcode CLT is required for the build anyway). wasm-tools
publishes no checksum file; executing downloaded prebuilt binaries was also not permitted in
this session, so a release-binary path could not be validated here.

**Alternatives.** Release binaries pinned by sha256 (faster: no 3-5 min compile; wasm-bindgen
publishes `.sha256sum`, wasm-tools does not).

## S11-D3 — the cross-host gate: behavioural parity hard, byte identity reported

**Decision.** The macOS job's hard gates are: build succeeds, module checks pass, Node per-tick
parity equals the committed goldens. Equality of the module sha256 with the Linux job's is
reported (job summary + `::notice`/`::warning`), not required.

**Rationale.** Byte identity cannot be shown without a Mac, and two measured host inputs break
it: (1) wasi-sdk 32's clang emits different code for the same input on arm64 hosts
(compiler-rt `multc3.c`, arm64-macos and arm64-linux packages vs x86_64; §4.2); (2) lua-src
compiles Lua in `fs::read_dir` order, and a different directory order changed the module
(c_remap, §4.4) while parity stayed 20/20. Behaviour is what the owner's rule requires (native
and web replays produce the same state hash).

**Alternatives.** Byte identity as a hard gate (would fail on the first arm64 run if any Lua or
Rust function hits the host-dependent path; no evidence it would pass); no cross-host check at all.

## S11-D4 — remap recipe on by default

**Decision.** `build_web.sh` maps the workspace, target dir, `$CARGO_HOME` and the rust-src path
to fixed spellings by default (`--no-remap` restores S9's flags and S9's bytes).

**Rationale.** Without it the module embeds `$CARGO_HOME` (540 registry paths), so it differs
between any two machines with different home directories (b_plain vs a_plain). With it two
checkouts differing in every path we control give identical bytes (a_remap = b_remap). The new
Linux reference is `9f137f8a…`; S9's `88e06b16…` stays reproducible with `--no-remap`.

**Alternatives.** Keep S9's flags (byte identity across machines impossible by construction);
cargo `trim-paths` (unstable in cargo 1.97.0: `feature trim-paths is required`).

## S11-D5 — the macOS version the plan states

**Decision.** "macOS 15 (Sequoia) or later, Apple silicon or Intel" is the supported statement;
macOS 14 is named as the technical floor of the pinned binaries, untested.

**Rationale.** The pinned binaries need macOS 14.0 on arm64 (wasi-sdk's bundled `libedit.0.dylib`
has minos 14.0 and `libLLVM.dylib` links it), 11.0 for Node 22.23.5, 10.12-10.15 for the x86_64
tools; Playwright 1.63 supports macOS 14+. GitHub retired every macOS 14 image on 2026-11-02, so
CI can test 15 and 26 only; the owner asked for high confidence, so only tested versions are
claimed.

**Alternatives.** "macOS 14+" (meets the binaries, but untestable in CI); "macOS 26 only".

## S11-D6 — Intel coverage

**Decision.** An Intel leg on the standard `macos-26-intel` runner, run on pushes to `main` touching
web paths, weekly and on `workflow_dispatch`, not on pull requests; the arm64 leg (`macos-15`)
runs on web-path pull requests. When GitHub stops offering Intel images, Intel becomes "best
effort, same x86_64-apple-darwin pins, untested", documented in DEVELOPMENT.md.

**Rationale.** macOS 26 is expected to be the last macOS for Intel Macs (needs web: RESULTS §9),
so `macos-26-intel` covers the newest Intel OS the longest; Intel Macs are the minority host; the arm64 leg already exercises every
script path; Rosetta cannot represent an Intel CPU. Running Intel off the PR path halves PR cost.

**Alternatives.** `macos-15-intel` (older OS, likely retired first); Intel on every PR; no Intel leg.

## S11-D7 — order experiment on a tmpfs mounted inside S11

**Decision.** The directory-order variant (c_remap/e_remap) mounts a 512 MiB tmpfs at
`S11/work/c` (root), unmounted at the end of `repro_paths.sh`.

**Rationale.** It keeps every file under the owned scratch path; ext4 cannot be made to return
another order for the same names, tmpfs returns reverse creation order.

**Alternatives.** `/dev/shm` (outside the owned path); a loop-mounted image file.
