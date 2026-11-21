# S11 — replacement text for the browser-version plan (issue #144 and docs/spikes/web/DESIGN.md)

Every statement below is backed by a row of `RESULTS.md` (pointer in brackets) or is an explicit CI
assertion (named job and step). Paste-ready blocks are between the `---8<---` markers.

---

## A. Issue body — "How to run locally" (replaces the whole section)

---8<---
## How to run locally

Supported build hosts:
- **Linux x86_64**, including WSL2.
- **macOS 15 (Sequoia) or later, on Apple silicon or Intel.** macOS 14 meets every pinned binary's floor but is untested (GitHub retired its macOS 14 images).
- Windows natively is not supported; use WSL2.

Other hosts (Linux on arm64, FreeBSD, Git Bash on Windows) are refused with a message. These commands work once W20 and W23 have landed. Until then, the archived S9 `build_web.sh` and `web/serve.cjs` reproduce the spike build on Linux ([S9 BOM][bom] §4).

Prerequisites:
- **Rust 1.97.0** with the `wasm32-unknown-unknown` target, through rustup.
- **Node 22** (22.23.5 tested). Use the nodejs.org installer or tarball, or Homebrew `node@22`.
- **macOS only: the Xcode Command Line Tools** (`xcode-select --install`). Build scripts and proc macros link with the system `cc`. Full Xcode is not needed.
- A POSIX shell. The scripts are POSIX `sh`, so macOS's `/bin/sh` (bash 3.2) works, and so do dash and busybox. GNU coreutils are not needed.

```sh
# once per machine (macOS: xcode-select --install first)
rustup toolchain install 1.97.0 --profile minimal --target wasm32-unknown-unknown
rustup override set 1.97.0                  # inside the checkout
scripts/web/fetch_tools.sh                  # wasi-sdk 32.0, binaryen version_132 (sha256-pinned per platform in scripts/web/tools.lock),
                                            # wasm-bindgen-cli 0.2.130 and wasm-tools 1.263.0 (cargo install --locked) into target/web-tools/
# every build
scripts/web/build_web.sh                    # both artifacts and the three bundles into target/web/dist/, plus SHA256SUMS
node scripts/web/serve.cjs target/web/dist 8790
```

`fetch_tools.sh` picks the tool archives for the host:
- `linux-x86_64`, `darwin-arm64` or `darwin-x86_64`.
- An x86_64 shell under Rosetta 2 on an Apple silicon Mac uses the arm64 tools.

It downloads with `curl` and checks every sha256 against `scripts/web/tools.lock`. On macOS it then removes any `com.apple.quarantine` attribute. Use it rather than a browser download. If you downloaded an archive by hand, either run `xattr -dr com.apple.quarantine target/web-tools`, or point `KHZ_WEB_MIRROR=<dir>` at the archive's directory; the pin is still checked. `fetch_tools.sh --node` also fetches the pinned Node.

Open `http://127.0.0.1:8790/` in Chrome or Edge 113+, Firefox 141+ (Windows) or 145+ (macOS), or Safari 26+. Older browsers get the WebGL2 artifact.

Phones:
- **Android:** run `adb reverse tcp:8790 tcp:8790`, then open `http://localhost:8790/`. localhost is a secure context (S5 §5).
- **iPhone:** run `node scripts/web/serve.cjs target/web/dist 8790 --host 0.0.0.0 --tls <cert.pem> <key.pem>` with a certificate the phone trusts. A plain-HTTP LAN address is not a secure context (S5 §8).

From a release: unzip `kilohertz-web-<version>.zip` and run `node serve.cjs dist 8790` (any OS with Node 22).
---8<---

Backing: hosts and floors [RESULTS §6.8, S11-D5]; Rosetta [§5.3, S11-D1]; CLT [§6.3, M24]; POSIX
scripts [§5, M21]; pins [§2, tools.lock]; quarantine [§6.1, M26 needs web → both cases handled];
toolchain install [§6.4]; Node [§2.6].

## B. Issue body — Decisions table: new row after Q13

---8<---
| Build host | **Linux x86_64 (incl. WSL2) and macOS 15+ on Apple silicon and Intel**, with one pin set per platform in `scripts/web/tools.lock` (wasi-sdk 32.0, binaryen version_132; wasm-bindgen-cli 0.2.130 and wasm-tools 1.263.0 built from crates.io). Windows natively: not supported (WSL2). The hard gate on every host is behaviour, not bytes: the build, the module checks and Node per-tick parity against the committed goldens. The module sha256 is compared with the Linux job's and reported, not required. `build_web.sh` remaps host paths by default. W08 sorts lua-src's source list | Owner ("Add Mac build support"); S11 §0-§4 | The macOS archives carry the same clang (22.1.0-wasi-sdk, llvm 4434dabb) and a byte-identical `libc.a`, `libsetjmp.a` and headers. Every arm64 binary is ad hoc signed. Bytes can still differ between hosts: wasi-sdk's own sysroot shows arm64-host clang emitting different code for one object (compiler-rt `multc3`), and the directory order of the Lua sources changed the module while parity stayed 20/20 (S11 §4.2, §4.4). `$CARGO_HOME` is embedded 540 times unless remapped (§4.3) |
---8<---

## C. Issue body — "Further decisions", **Build** bullet (replace)

---8<---
- **Build** (S9 BOM; S11):
  - `-C target-feature=+simd128 -C link-arg=-zstack-size=4194304`, plus `--remap-path-prefix` for the workspace, target dir, `$CARGO_HOME` and rust-src, passed as `CARGO_ENCODED_RUSTFLAGS`
  - opt-level 3, codegen-units 1, LTO declared off, `panic=abort`
  - wasm-bindgen 0.2.130 `--target web`
  - binaryen 132 `wasm-opt -Oz`
  - Build hosts: Linux x86_64 and macOS 15+ (arm64, x86_64)
---8<---

## D. W20 acceptance criteria (replace the three bullets of "W20 Web build script and local server")

---8<---
- **Owned paths** (add): `scripts/web/lib.sh`, `scripts/web/tools.lock` (platform-keyed: `linux-x86_64`, `darwin-arm64`, `darwin-x86_64`), `scripts/web/wasm_env.sh`.
- **Acceptance:**
  - The script builds both artifacts from a clean checkout, records `SHA256SUMS`, and runs on Linux x86_64 and macOS (arm64, x86_64). On any other host it stops with a message (exit 3).
  - Every `scripts/web/*.sh` is POSIX `sh` and passes ShellCheck `--shell=sh`. A test runs them under dash, bash in POSIX mode and busybox sh, with a PATH missing `sha256sum`, `nproc` and `realpath` and trapping `readlink -f`, `stat -c`, `sed -i`/`-r`, `grep -P`, `date -d` and `xargs -r`. Shimmed `uname`/`sysctl` select `darwin-arm64` (also under Rosetta 2), `darwin-x86_64` and `linux-x86_64`, and refuse Linux/aarch64, MINGW and FreeBSD (S11 `test_portability.sh`).
  - `fetch_tools.sh` rejects an archive whose sha256 or size differs from `tools.lock`: it deletes the archive and extracts nothing. A second run downloads nothing. On macOS it removes `com.apple.quarantine`, and it runs every fetched binary once (`--version`).
  - `wasm_env.sh` removes `CFLAGS`, `TARGET_CFLAGS`, `CPATH`, `C_INCLUDE_PATH`, `CCC_OVERRIDE_OPTIONS` and `RUSTFLAGS` from the build environment (test with Homebrew-style values).
  - On Linux, `build_web.sh --no-remap` reproduces S9's module for S9's tree (`88e06b16…`). Two checkouts with different paths and different `CARGO_HOME`s give the same sha256 with the default remap (S11 `repro_paths.sh`).
  - `check_module.sh` fails on a module with an injected `env` import, on a mixed-EH module, on a relaxed-SIMD module and on an oversize module (test modules generated by `wasm-tools`).
  - A `serve.cjs` test checks the CSP header, the MIME type, the absence of COOP/COEP and that traversal is refused.
---8<---

## E. W08 deliverable (add one bullet; W08 owns `third_party/lua-src/src/lib.rs`)

---8<---
- `add_files_by_ext` sorts the source paths before adding them (S11 `patches/0001-lua-src-sorted-source-order.patch`). `fs::read_dir` returns the file system's order, and the Lua object order decides the link order and so the module bytes. On the same files, ext4 and tmpfs order gave two different modules (S11 §4.4). Native output is unchanged; native binary layout follows the sorted order.
---8<---

## F. W26 acceptance criteria (replace) and deliverables (add)

---8<---
- **Deliverables** (add): `web-build-macos` from the CI design table: an arm64 leg on `macos-15` and an Intel leg on `macos-26-intel`.
- **Acceptance:**
  - `spec_web_yml` checks:
    - the job list and the path filters, exactly as in the CI design
    - no `continue-on-error` on gating steps
    - timeouts ≤ 45 min
    - `pr-checks` needs no web job
    - the pinned tool versions
    - `web-build-macos` runs `/bin/sh scripts/web/*.sh` on `macos-15` for web-path pull requests and on `macos-26-intel` for pushes to `main`, the weekly schedule and dispatch
  - The workflow is green, including both `web-build-macos` legs. Each leg's fetch, build, `check_module` and Node parity (20 runs per artifact against the committed goldens) pass.
  - The "Module sha256 vs Linux (report)" step records equal or different, with both sums, in the job summary. It never fails the job.
---8<---

## G. DESIGN.md — "Toolchain pins": replace the wasm-bindgen row, the binaryen and wasi-sdk rows, and the "Build hosts" paragraph

---8<---
| wasm-bindgen crate and CLI | **=0.2.130** (must match exactly; a 0.2.129 CLI fails with a schema mismatch, S3 §2) | `cargo install --locked wasm-bindgen-cli --version 0.2.130` on every build host, run by `fetch_tools.sh` into `target/web-tools/cargo` (crate checksum `c37f6904…fc7a`). The release assets for aarch64/x86_64-apple-darwin and x86_64-linux-musl are recorded in `tools.lock` but not used (S11 §2.5) |
| wasm-tools | 1.263.0 | `cargo install --locked wasm-tools --version 1.263.0` (crate checksum `14b8ed79…dc96`); no published checksum for its release assets |
| wasi-sdk (clang 22.1.0, wasi-libc 2fc32bc81b9f, libsetjmp) | **32.0** | per host, `https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-32/`: `wasi-sdk-32.0-x86_64-linux.tar.gz` (143 157 058 B, `55fc523e…ef93`), `wasi-sdk-32.0-arm64-macos.tar.gz` (131 307 682 B, `383be7fe…b933d`), `wasi-sdk-32.0-x86_64-macos.tar.gz` (133 615 598 B, `a36c9ab1…1b1b`). No checksum file is published. All three have the same clang commit and a byte-identical `wasm32-wasip1` `libc.a`, `libsetjmp.a` and headers (S11 §2.4). System clang-18 does not work (S9 C5) |
| binaryen (`wasm-opt`) | **version_132**, native | `https://github.com/WebAssembly/binaryen/releases/download/version_132/`: `binaryen-version_132-x86_64-linux.tar.gz` (`195ddc94…0572`), `-arm64-macos.tar.gz` (`98aad827…59aa`), `-x86_64-macos.tar.gz` (`40c3de90…832e`). Each equals its published `.sha256`. On macOS `wasm-opt` loads `lib/libbinaryen.dylib`, so keep the extracted tree whole |
| Node | 22 (pinned 22.23.5 in CI and `tools.lock`) | `actions/setup-node`; locally the nodejs.org build (`node-v22.23.5-{linux-x64,darwin-arm64,darwin-x64}.tar.gz`, sha256 in `tools.lock` = `SHASUMS256.txt`) or Homebrew `node@22` |

Build hosts:
- **Supported:**
  - Linux x86_64, including WSL2.
  - macOS 15 or later on Apple silicon (`darwin-arm64`) and on Intel (`darwin-x86_64`).
- **Pinned binary floors** (S11 §6.8):
  - arm64: macOS 14.0. wasi-sdk's bundled `libedit.0.dylib` sets it; the other binaries need 11.0.
  - x86_64: macOS 11.0, set by Node. The tools themselves need 10.12-10.15.
  - macOS 14 is untested, because GitHub has no macOS 14 runner since 2026-11-02.
- **Signatures:**
  - Every arm64 binary in these archives is ad hoc signed (linker-signed), so no `codesign` step is needed.
  - The x86_64 binaries are unsigned, which Intel Macs allow.
  - Pins are on archives, never on extracted files.
- **macOS prerequisites:** the Xcode Command Line Tools (host linker for build scripts and proc macros).
- **Refused:** Linux on arm64 and Windows outside WSL2 stop with a message.
---8<---

## H. DESIGN.md — "Build recipe" (replace the whole section)

---8<---
### Build recipe (W20; S9 BOM §3-4; S11 for the hosts)

1. Install the toolchain:
   - `rustup toolchain install 1.97.0 --profile minimal --target wasm32-unknown-unknown`
   - on macOS also the Xcode Command Line Tools: `build_web.sh` checks `xcode-select -p`
2. Run `scripts/web/fetch_tools.sh`. It:
   - picks the platform key from `uname -s`/`-m`; `sysctl.proc_translated = 1` (Rosetta 2) selects `darwin-arm64`
   - downloads the `tools.lock` archives for that key with curl into `target/web-tools/`
   - checks each archive's sha256 and size, extracts it, and removes `com.apple.quarantine` on macOS
   - runs `cargo install --locked` for wasm-bindgen-cli and wasm-tools
   - runs every binary once (`--version`)
   - writes `target/web-tools/tools.env`
3. `scripts/web/build_web.sh` sources `wasm_env.sh`, which sets the environment:
   - It first removes host settings that would reach the wasm32 clang or rustc: `CFLAGS`, `CXXFLAGS`, `TARGET_CFLAGS`, `ARFLAGS`, `CPATH`, `C_INCLUDE_PATH`, `CPLUS_INCLUDE_PATH`, `OBJC_INCLUDE_PATH`, `CCC_OVERRIDE_OPTIONS`, `COMPILER_PATH`, `CRATE_CC_NO_DEFAULTS`, `RUSTFLAGS`, `CARGO_BUILD_RUSTFLAGS` and `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS`. cargo is then run under `env -u` for the hyphenated `CC_/CFLAGS_/AR_wasm32-unknown-unknown`.
   - `CC_wasm32_unknown_unknown` and `AR_wasm32_unknown_unknown`: the wasi-sdk `clang` and `llvm-ar`
   - `KHZ_WASI_SYSROOT=<wasi-sdk>/share/wasi-sysroot`, `KHZ_WASM_LEGACY_EH=true`, `KHZ_LUA_MATH=1` (until W09 makes it unconditional)
   - `CARGO_ENCODED_RUSTFLAGS` (0x1f-separated, so paths with spaces survive), in this order:
     - `-C target-feature=+simd128 -C link-arg=-zstack-size=4194304`
     - `--remap-path-prefix=<rustc sysroot>/lib/rustlib/src/rust=/rustc/<commit-hash>`
     - `--remap-path-prefix=<workspace>=/khz`
     - `--remap-path-prefix=<target dir>=/khz/target`
     - `--remap-path-prefix=<CARGO_HOME>=/cargo`
     - each prefix also in its physical (`pwd -P`) spelling when different, e.g. macOS `/tmp` = `/private/tmp`
   - `CC_SHELL_ESCAPED_FLAGS=1` and `CFLAGS_wasm32_unknown_unknown` = the same three prefixes as quoted `-ffile-prefix-map` flags
   - `CARGO_PROFILE_RELEASE_OPT_LEVEL=3`, `CODEGEN_UNITS=1`, `LTO=false`, `PANIC=abort`, `DEBUG=false`
   - Why the remap: without it the module embeds `$CARGO_HOME` 540 times, so the bytes depend on the user's home directory. `--no-remap` gives S9's exact flags.
   - Why LTO is declared off: cargo silently ignores it for a `[cdylib, rlib]` crate, and a real fat-LTO build was **larger** after `-Oz` (S9 C7, S9-Q5).
   - These flags live in `wasm_env.sh`, not in a `.cargo/config.toml`, so native builds and the byte-compared `tables/*/rules-wasm` rebuilds are unaffected.
4. Build each artifact: `cargo build --locked --release --target wasm32-unknown-unknown -p pin_web --no-default-features --features webgpu`, and the same with `--features webgl`.
5. Run `wasm-bindgen --target web --out-name pin_web`.
6. Run `wasm-opt -Oz --enable-simd --enable-exception-handling --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals --enable-reference-types --enable-multivalue --strip-debug --strip-producers`. Parity was re-proven on the optimised module (S9 §5). The output does not depend on the thread count (S11 §3.2).
7. Build the table bundles with `tables/*/build.sh` and `scripts/package.py`.
8. Copy everything to `target/web/dist/` and write `SHA256SUMS`. Hashing goes through Node, `shasum -a 256` or `sha256sum`, whichever exists.

`scripts/web/check_module.sh` fails on any of these:
- **Validation.** `wasm-tools validate --features=mvp,simd,exceptions,legacy-exceptions,bulk-memory,saturating-float-to-int,sign-extension,mutable-global,reference-types,multi-value` fails.
- **Imports.** Any import outside the glue module, or any `env.*` or `wasi_*` import. rustc turns undefined C symbols into silent `env` imports (S2 §8, S9 §4).
- **Exception handling.** `try_table` present, or legacy `try` and `try_table` mixed (JSC rejects mixed modules, S10 §6.3).
- **Relaxed SIMD.** Any relaxed-SIMD opcode (S1 A12).
- **Size.** The brotli size (quality 11, lgwin 24) exceeds 2.5 MiB (WebGPU) or 3.0 MiB (WebGL2).

Reproducibility (S11 §4):
- **Same host, same file system:** the sha256 is reproducible.
- **Different checkout paths or `CARGO_HOME`s:** the sha256 is the same with the default remap.
- **Different directory order:** the same only once W08 sorts lua-src's sources.
- **Across host architectures (Linux x86_64 vs macOS arm64):** it is reported, not required. The arm64-host wasi-sdk clang emits different code for at least one input (compiler-rt `multc3`), and rustc's LLVM may too. Parity is the gate.

Script rules: POSIX `sh` only, so bash 3.2, dash and busybox all work. No GNU-only flags. Quote every path. Use `CDPATH= cd -- … && pwd -P` for directories, and `getconf _NPROCESSORS_ONLN` for the CPU count.

Cost: a cold cargo build takes 4 min 41 s on a 4-core VM and incremental builds 3-6 s (S9 §3). The `cargo install` of the two CLIs takes 2 min 23 s and 3 min 20 s on 2 jobs (S11 §5.4).
---8<---

## I. DESIGN.md — "Local server" (add two bullets; the rest unchanged)

---8<---
- Runs anywhere Node 22 runs: Linux, macOS (arm64, x86_64), and Windows for the release zip. It uses only `http`, `fs` and `path`, and the `path.sep` traversal check is correct on all three.
- On macOS, `--host 0.0.0.0` may trigger the application firewall's "accept incoming connections" prompt for `node`. Answer it, or stay on 127.0.0.1 with `adb reverse` for Android. The prompt is unverified: W27's macOS job records whether it appears.
---8<---

## J. DESIGN.md — "CI design": add this row to the jobs table and these rules

---8<---
| `web-build-macos` (arm64 `macos-15`; Intel `macos-26-intel`) | arm64: same list as `web-build`, on pull requests. Intel: pushes to `main` on those paths, weekly `schedule`, `workflow_dispatch` | Proves the supported macOS build hosts with the same pins. Intel stays off pull requests to halve macOS cost. macOS 26 is expected to be the last Intel macOS (needs web, S11 §9) | Steps:<br>1. checkout<br>2. `dtolnay/rust-toolchain@1.97.0` with `targets: wasm32-unknown-unknown`<br>3. `actions/setup-node` 22.23.5<br>4. cache `target/web-tools`, keyed by OS, arch and the `tools.lock` hash, plus `Swatinem/rust-cache`<br>5. "Xcode CLT present" (`xcode-select -p`)<br>6. `/bin/sh scripts/web/fetch_tools.sh`, which runs the smoke checks<br>7. `/bin/sh scripts/web/build_web.sh` for both artifacts<br>8. `check_module.sh`<br>9. Node parity, 20 runs per artifact, against the committed goldens (hard)<br>10. `size_report.sh` budgets<br>11. "Module sha256 vs Linux (report)": compare with the `web-build` artifact's `SHA256SUMS`, then `::notice` or `::warning` and a summary line, never a failure<br>12. upload `SHA256SUMS` and `build.log` | Estimate: 18-25 min cold, 8-12 min warm. Standard runners are free for public repositories; private repositories pay $0.062/min on macOS (web, Orchestrator fetch) |
---8<---

Rules to add under "Rules for all jobs":

---8<---
- macOS jobs run the scripts with `/bin/sh` (bash 3.2 in POSIX mode) on purpose. They never install GNU coreutils or a newer bash.
- Intel coverage lasts while GitHub offers an Intel image (`macos-26-intel`). After that, Intel is "best effort, same `darwin-x86_64` pins, untested". Rosetta 2 on an arm64 runner cannot stand in for an Intel CPU. Record the change in `docs/DEVELOPMENT.md` when the image is retired.
- The committed goldens come from the Linux native headless run. macOS native replays differ from x86_64 until W02 lands (S7 §4.5), so no macOS native build is used for web parity.
- Safari via `safaridriver` stays in `web-safari-firefox` (W27, S10 §9). It uses the Linux `web-build` artifact, not the macOS build.
---8<---

## K. `scripts/web/tools.lock` (complete file; also at `S11/scripts/web/tools.lock`)

---8<---
# Web build tool pins. One line per tool and build-host platform; whitespace-separated
# columns; '#' starts a comment. Read by fetch_tools.sh (POSIX awk).
#
# columns: tool platform kind version sha256 bytes topdir url upstream_checksum
#   platform: linux-x86_64 | darwin-arm64 | darwin-x86_64 | any
#   kind:     archive = .tar.gz fetched with curl, sha256-verified, extracted under target/web-tools/<platform>/
#             cargo   = `cargo install --locked --root target/web-tools/cargo <tool> --version <version>`
#                       (sha256 = the crates.io index checksum of the .crate, which cargo verifies itself)
#   topdir:   the archive's single top-level directory ('-' for cargo)
#   upstream_checksum: the publisher's checksum file the sha256 was compared with ('none' if none is published)
#
# wasi-sdk 32.0 (clang 22.1.0-wasi-sdk, llvm 4434dabb6991, wasi-libc 2fc32bc81b9f): no checksum file is published
wasi-sdk       linux-x86_64   archive  32.0     55fc523ebfbc98f69d1034fcfcb83d1ff5610cd9ab7eceef6cd097a30ba4ef93  143157058  wasi-sdk-32.0-x86_64-linux  https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-32/wasi-sdk-32.0-x86_64-linux.tar.gz  none
wasi-sdk       darwin-arm64   archive  32.0     383be7fe00ae04691e1859164fd99970a528c55d624b886ed59f7933898b933d  131307682  wasi-sdk-32.0-arm64-macos   https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-32/wasi-sdk-32.0-arm64-macos.tar.gz  none
wasi-sdk       darwin-x86_64  archive  32.0     a36c9ab106a40abe8a051279c233c6bab719a5788ecc0788e8d141796eb55b1b  133615598  wasi-sdk-32.0-x86_64-macos  https://github.com/WebAssembly/wasi-sdk/releases/download/wasi-sdk-32/wasi-sdk-32.0-x86_64-macos.tar.gz  none
# binaryen version_132 (wasm-opt); each archive has a published <archive>.sha256, equal to these
binaryen       linux-x86_64   archive  132      195ddc94f9bc89f45abdabb0b9eea86023d727ba90eac8b35b80f2544fc30572  117057408  binaryen-version_132  https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-x86_64-linux.tar.gz  https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-x86_64-linux.tar.gz.sha256
binaryen       darwin-arm64   archive  132      98aad827847af7ef990ed7098d885725c8e5b5aae75073403635617ae4e259aa  7640117    binaryen-version_132  https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-arm64-macos.tar.gz  https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-arm64-macos.tar.gz.sha256
binaryen       darwin-x86_64  archive  132      40c3de90bb3766bd0282a895e139a6f50253dba49b4f5bb89e66faca162d832e  9069142    binaryen-version_132  https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-x86_64-macos.tar.gz  https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-x86_64-macos.tar.gz.sha256
# wasm-bindgen CLI (must equal Cargo.lock's wasm-bindgen) and wasm-tools: built from crates.io source on every host
wasm-bindgen-cli  any         cargo    0.2.130  c37f6904387d19a319525c74af274c73912f3871c35555aa49f3ea64ebb2fc7a  -          -  https://crates.io/crates/wasm-bindgen-cli/0.2.130  https://index.crates.io/wa/sm/wasm-bindgen-cli
wasm-tools        any         cargo    1.263.0  14b8ed798ec13f0e0939dcc403836ba5fd7fb29ff9772e6438a9bcc227bfdc96  -          -  https://crates.io/crates/wasm-tools/1.263.0  https://index.crates.io/wa/sm/wasm-tools
# Node 22 (prerequisite; fetched only with `fetch_tools.sh --node`); SHASUMS256.txt is GPG-signed (SHASUMS256.txt.asc)
node           linux-x86_64   archive  22.23.5  c54118104acc524633b5abe106bbf44a53126ec8b4e5fa105cb92b5d7de8a1a1  56856114   node-v22.23.5-linux-x64     https://nodejs.org/dist/v22.23.5/node-v22.23.5-linux-x64.tar.gz  https://nodejs.org/dist/v22.23.5/SHASUMS256.txt
node           darwin-arm64   archive  22.23.5  e7131995759b61a0170b646010d2ad35a5d37b4cb0d471f3e8317eac6f742f3d  50004868   node-v22.23.5-darwin-arm64  https://nodejs.org/dist/v22.23.5/node-v22.23.5-darwin-arm64.tar.gz  https://nodejs.org/dist/v22.23.5/SHASUMS256.txt
node           darwin-x86_64  archive  22.23.5  c53383238206b9ae4db23c5d8a63645e2e753bec8ab51a49cc81939aa64e9422  51197448   node-v22.23.5-darwin-x64    https://nodejs.org/dist/v22.23.5/node-v22.23.5-darwin-x64.tar.gz  https://nodejs.org/dist/v22.23.5/SHASUMS256.txt
#
# Alternatives (release binaries; not used):
#   wasm-bindgen-0.2.130-aarch64-apple-darwin.tar.gz       b90ddb9357f584b1af897d58ad77e4cce2b48fc17e2a967bc249a87f72809444  (= its .sha256sum)
#   wasm-bindgen-0.2.130-x86_64-apple-darwin.tar.gz        eb209c115b7cb95832423c1aa3f4d06d6c53f63ef74fc4b6fb79ef0b2eea5f10  (= its .sha256sum)
#   wasm-bindgen-0.2.130-x86_64-unknown-linux-musl.tar.gz  9687f3bb551b9f66f37ed32a40d7913a00f7962db653054e56857bc306c3227f  (= its .sha256sum)
#   wasm-tools-1.263.0-aarch64-macos.tar.gz                b568f04746a699c4068b8d751779b60b501cf2cab6d259a3bc973eb5eeb1f62b  (no checksum file published)
#   wasm-tools-1.263.0-x86_64-macos.tar.gz                 909061aba8c069aeed8cb0933e50973a9c8e869bbd6a7819176b49871a338424  (no checksum file published)
#   wasm-tools-1.263.0-x86_64-linux.tar.gz                 21c036c1740a0552124c79c6e2170cb61c5b016005671a6d5bc06cf642165431  (no checksum file published)
---8<---

Also pin (CI, not tools.lock): Rust 1.97.0 host packages from the channel manifest of 2026-07-09:
`rustc-1.97.0-aarch64-apple-darwin.tar.xz` `53e339f16382bb348566f648d01775797e038fdd6bfc6ddd8859c5a0eac09f68`,
`rustc-1.97.0-x86_64-apple-darwin.tar.xz` `c3c6f6cd95e4546306dfa4beb3cec23dedf10dd02e6e7d99b053bf418d453014`,
`rust-std-1.97.0-wasm32-unknown-unknown.tar.xz` `474f926f2ffb0ed5ff895db92b7d0b2d9c3acb75b6df3229b132db0822594509`
(rustup verifies these itself).

## L. Risks table (issue) — new row, and U-table row

---8<---
| R9 | A macOS build differs in bytes from the Linux build | medium (expected on arm64) | low | The parity gate (Node, committed goldens) is the gate. The sha256 comparison is only reported. Paths are remapped and W08 sorts the Lua sources (S11 §4) |
---8<---

---8<---
| U15 | macOS build host: the pinned macOS binaries and `/bin/sh` scripts have not run on a Mac. The arm64-host clang may emit different code for a Lua function (it did for compiler-rt `multc3`, S11 §4.2) | low | Same clang commit and byte-identical sysroot inputs. Every arm64 binary is signed. The scripts were tested under dash, bash `--posix` and busybox with BSD-like shims | W26 first `web-build-macos` run: expected outputs in S11 §8 |
---8<---

## M. Residual risk and the single confidence-closing step

What only a real Mac settles:
- the binaries run (dyld, signatures, Gatekeeper)
- `/bin/sh` 3.2 executes the scripts
- whether the arm64 module equals Linux's

The confidence-closing step is the **first `web-build-macos` run in W26**. Its expected outputs are listed in RESULTS §8:
- `smoke OK` lines with the pinned versions
- `host darwin-arm64`
- `check_module` pass
- 20/20 Node parity per artifact against the goldens
- a sha256 line, reported but not gating

A parity failure on macOS blocks and is investigated. A sha256 difference alone is recorded.
